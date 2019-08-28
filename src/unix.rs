//! Bindings to System V semaphores
//!
//! When dealing with unix, there are generally two kinds of IPC semaphores, one
//! is the System V semaphore while the other is a POSIX semaphore. The POSIX
//! semaphore is generally easier to use, but it does not relinquish resources
//! when a process terminates unexpectedly. On the other ahnd a System V
//! semaphore provides the option to do so, so the choice was made to use a
//! System V semaphore rather than a POSIX semaphore.
//!
//! System V semaphores are interesting in that they have an unusual
//! initialization procedure where a semaphore is created and *then*
//! initialized. As in, these two steps are not atomic. This causes some
//! confusion down below, as you'll see in `fn new`.
//!
//! Additionally all semaphores need a `key_t` which originates from an actual
//! existing file, so this implementation ensures that a file exists when
//! creating a semaphore.

#![allow(bad_style)]

use std::env;
use std::fs;
use std::hash::{Hash, Hasher, SipHasher};
use std::io::{Result, Error, ErrorKind};
use std::mem;
use std::path::PathBuf;
use libc;
use libc::consts::os::posix88::{EEXIST, O_RDWR};

use self::consts::{IPC_CREAT, IPC_EXCL, key_t, sembuf, SEM_UNDO, IPC_NOWAIT};
use self::consts::{IPC_STAT, IPC_RMID, SETVAL, semid_ds};

pub struct Semaphore { semid: libc::c_int }

#[cfg(target_os = "linux")]
mod consts {
    use libc;

    pub type key_t = i32;

    pub static IPC_CREAT: libc::c_int = 0o1000;
    pub static IPC_EXCL: libc::c_int = 0o2000;
    pub static IPC_NOWAIT: libc::c_short = 0o4000;
    pub static SEM_UNDO: libc::c_short = 0x1000;
    pub static SETVAL: libc::c_int = 16;
    pub static IPC_STAT: libc::c_int = 2;
    pub static IPC_RMID: libc::c_int = 0;

    #[repr(C)]
    pub struct sembuf {
        pub sem_num: libc::c_ushort,
        pub sem_op: libc::c_short,
        pub sem_flg: libc::c_short,
    }

    #[repr(C)]
    pub struct semid_ds {
        pub sem_perm: ipc_perm,
        pub sem_otime: libc::time_t,
        __glibc_reserved1: libc::c_ulong,
        pub sem_ctime: libc::time_t,
        __glibc_reserved2: libc::c_ulong,
        pub sem_nsems: libc::c_ulong,
        __glibc_reserved3: libc::c_ulong,
        __glibc_reserved4: libc::c_ulong,
    }

    #[repr(C)]
    pub struct ipc_perm {
        pub __key: key_t,
        pub uid: libc::uid_t,
        pub gid: libc::gid_t,
        pub cuid: libc::uid_t,
        pub cgid: libc::gid_t,
        pub mode: libc::c_ushort,
        __pad1: libc::c_ushort,
        pub __seq: libc::c_ushort,
        __pad2: libc::c_ushort,
        __glibc_reserved1: libc::c_ulong,
        __glibc_reserved2: libc::c_ulong,
    }
}


#[cfg(target_os = "macos")]
mod consts {
    use libc;

    pub type key_t = i32;

    pub static IPC_CREAT: libc::c_int = 0o1000;
    pub static IPC_EXCL: libc::c_int = 0o2000;
    pub static IPC_NOWAIT: libc::c_short = 0o4000;
    pub static SEM_UNDO: libc::c_short = 0o10000;
    pub static SETVAL: libc::c_int = 8;
    pub static IPC_STAT: libc::c_int = 2;
    pub static IPC_RMID: libc::c_int = 0;

    #[repr(C)]
    pub struct sembuf {
        pub sem_num: libc::c_ushort,
        pub sem_op: libc::c_short,
        pub sem_flg: libc::c_short,
    }

    // ugh, this is marked as pack(4) on OSX which apparently we can't
    // emulate in rust yet! Instead we mark the structure as packed and then
    // insert our own manual padding.
    #[repr(C, packed)]
    pub struct semid_ds {
        pub sem_perm: ipc_perm,
        _sem_base: i32,
        pub sem_nsems: libc::c_ushort,
        __my_padding_hax_ugh: libc::c_ushort,
        pub sem_otime: libc::time_t,
        _sem_pad1: i32,
        pub sem_ctime: libc::time_t,

        _sem_pad2: i32,
        _sem_pad3: [i32; 4],
    }

    #[repr(C)]
    pub struct ipc_perm {
        pub uid: libc::uid_t,
        pub gid: libc::gid_t,
        pub cuid: libc::uid_t,
        pub cgid: libc::gid_t,
        pub mode: libc::mode_t,
        pub __seq: libc::c_ushort,
        pub __key: key_t,
    }
}

extern {
    fn ftok(pathname: *const libc::c_uchar, proj_id: libc::c_int) -> key_t;
    fn semget(key: key_t, nsems: libc::c_int, semflg: libc::c_int) -> libc::c_int;
    fn semctl(semid: libc::c_int, semnum: libc::c_int,
              cmd: libc::c_int, ...) -> libc::c_int;
    fn semop(semid: libc::c_int, sops: *mut sembuf,
             nsops: libc::c_uint) -> libc::c_int;
}

impl Semaphore {
    pub unsafe fn new(name: &str, cnt: usize) -> Result<Semaphore> {
        let key = try!(Semaphore::key(name));

        // System V semaphores cannot be initialized at creation, and we don't
        // know which process is responsible for creating the semaphore, so we
        // partially assume that we are responsible.
        //
        // In order to get "atomic create and initialization" we have a dirty
        // hack here. First, an attempt is made to exclusively create the
        // semaphore. If we succeed, then we're responsible for initializing it.
        // If we fail, we need to wait for someone's initialization to succeed.
        // We read off the `sem_otime` field in a loop to "wait until a
        // semaphore is initialized." Sadly I don't know of a better way to get
        // around this...
        //
        // see http://beej.us/guide/bgipc/output/html/multipage/semaphores.html
        let mut semid = semget(key, 1, IPC_CREAT | IPC_EXCL | 0o666);
        if semid >= 0 {
            let mut buf = sembuf {
                sem_num: 0,
                sem_op: cnt as libc::c_short,
                sem_flg: 0
            };
            // Be sure to clamp the value to 0 and then add the necessary count
            // onto it. The clamp is necessary as the initial value seems to be
            // generally undefined, and the bump is then necessary to modify
            // sem_otime.
            if semctl(semid, 0, SETVAL, 0) != 0 ||
               semop(semid, &mut buf, 1) != 0 {
                let err = Error::last_os_error();
                semctl(semid, 0, IPC_RMID);
                return Err(err)
            }
        } else {
            match Error::last_os_error() {
                ref e if e.raw_os_error() == Some(EEXIST) => {
                    // Re-attempt to get the semaphore, this should in theory always
                    // succeed?
                    semid = semget(key, 1, 0);
                    if semid < 0 { return Err(Error::last_os_error()) }

                    // Spin in a small loop waiting for sem_otime to become not 0
                    let mut ok = false;
                    for _ in 0..1000 {
                        let mut buf: semid_ds = mem::zeroed();
                        if semctl(semid, 0, IPC_STAT, &mut buf) != 0 {
                            return Err(Error::last_os_error())
                        }
                        if buf.sem_otime != 0 {
                            ok = true;
                            break
                        }
                    }
                    if !ok {
                        return Err(Error::new(ErrorKind::TimedOut, "timed out waiting for sem to be initialized"))
                    }
                }
                e => return Err(e)
            }
        }

        // Phew! That took long enough...
        Ok(Semaphore { semid: semid })
    }

    /// Get value hash
    fn hash<T: Hash>(value: &T) -> u64 {
        let mut h = SipHasher::new();
        value.hash(&mut h);
        h.finish()
    }

    /// Generate the filename which will be passed to ftok, keyed off the given
    /// semaphore name `name`.
    fn filename(name: &str) -> PathBuf {
        let filename = name.chars().filter(|a| {
            (*a as u32) < 128 && a.is_alphanumeric()
        }).collect::<String>();
        env::temp_dir().join("ipc-rs-sems").join(format!("{}-{}", filename, Semaphore::hash::<_>(&(name, "ipc-rs"))))
    }

    /// Generate the `key_t` from `ftok` which will be passed to `semget`.
    ///
    /// This function will ensure that the relevant file is located on the
    /// filesystem and will then invoke ftok on it.
    unsafe fn key(name: &str) -> Result<key_t> {
        let filename = Semaphore::filename(name);
        let dir = filename.parent().unwrap();

        // As long as someone creates the directory we're alright.
        let _ = fs::create_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Make sure that the file exists. Open it in exclusive/create mode to
        // ensure that it's there, but don't overwrite it if it alredy exists.
        //
        // see QSharedMemoryPrivate::createUnixKeyFile in Qt
        let filename = filename.to_str().unwrap().to_string() + "\0";
        let fd = libc::open(filename.as_ptr() as (*const i8),
                            libc::O_EXCL | libc::O_CREAT | O_RDWR,
                            0o640);
        if fd > 0 {
            libc::close(fd);
        } else {
            match Error::last_os_error() {
                ref e if e.raw_os_error() == Some(EEXIST) => {}
                e => return Err(e)
            }
        }

        // Invoke `ftok` with our filename
        let key = ftok(filename.as_ptr(), 'I' as libc::c_int);
        if key != -1 {Ok(key)} else {Err(Error::last_os_error())}
    }

    pub unsafe fn wait(&self) {
        loop {
            if self.modify(-1, true) == 0 { return }

            match Error::last_os_error() {
                ref e if e.raw_os_error() == Some(libc::EINTR) => {}
                e => panic!("unknown wait error: {}", e)
            }
        }
    }

    pub unsafe fn try_wait(&self) -> bool {
        if self.modify(-1, false) == 0 { return true }

        match Error::last_os_error() {
            ref e if e.raw_os_error() == Some(libc::EAGAIN) => return false,
            e => panic!("unknown try_wait error: {}", e)
        }
    }

    pub unsafe fn post(&self) {
        if self.modify(1, true) == 0 { return }
        panic!("unknown post error: {}", Error::last_os_error())
    }

    unsafe fn modify(&self, amt: i16, wait: bool) -> libc::c_int {
        let mut buf = sembuf {
            sem_num: 0,
            sem_op: amt as libc::c_short,
            sem_flg: if wait {0} else {IPC_NOWAIT} | SEM_UNDO,
        };
        semop(self.semid, &mut buf, 1)
    }
}

impl Drop for Semaphore {
    fn drop(&mut self) {}
}

#[cfg(test)]
mod tests {
    extern crate tempdir;

    use std::fs::File;
    use std::io::Write;
    use std::process::Command;
    use std::str;
    use std::mem;

    use self::tempdir::TempDir;

    use super::consts::{sembuf, semid_ds, ipc_perm};

    macro_rules! offset{ ($ty:ty, $f:ident) => (unsafe {
        let f = 0 as *const $ty;
        &(*f).$f as *const _ as usize
    }) }

    #[test]
    fn check_offsets() {
        let td = TempDir::new("test").unwrap();
        let mut f = File::create(&td.path().join("foo.c")).unwrap();
        f.write(&format!(r#"
#include <assert.h>
#include <stdio.h>
#include <stddef.h>
#include <stdlib.h>
#include <unistd.h>
#include <errno.h>
#include <sys/types.h>
#include <sys/ipc.h>
#include <sys/sem.h>

#define assert_eq(a, b) \
    if ((a) != (b)) {{ \
        printf("%s: %d != %d", #a, (int) (a), (int) (b)); \
        return 1; \
    }}

int main() {{
    assert_eq(offsetof(struct sembuf, sem_num), {sem_num});
    assert_eq(offsetof(struct sembuf, sem_op), {sem_op});
    assert_eq(offsetof(struct sembuf, sem_flg), {sem_flg});
    assert_eq(sizeof(struct sembuf), {sembuf});

    assert_eq(offsetof(struct ipc_perm, {key}), {keyfield});
    assert_eq(offsetof(struct ipc_perm, uid), {uid});
    assert_eq(offsetof(struct ipc_perm, gid), {gid});
    assert_eq(offsetof(struct ipc_perm, cuid), {cuid});
    assert_eq(offsetof(struct ipc_perm, cgid), {cgid});
    assert_eq(offsetof(struct ipc_perm, mode), {mode});
    assert_eq(offsetof(struct ipc_perm, {seq}), {seqfield});
    assert_eq(sizeof(struct ipc_perm), {ipc_perm});

    assert_eq(offsetof(struct semid_ds, sem_perm), {sem_perm});
    assert_eq(offsetof(struct semid_ds, sem_otime), {sem_otime});
    assert_eq(offsetof(struct semid_ds, sem_nsems), {sem_nsems});
    assert_eq(sizeof(struct semid_ds), {semid_ds});

    assert_eq(IPC_CREAT, {IPC_CREAT});
    assert_eq(IPC_EXCL, {IPC_EXCL});
    assert_eq(IPC_NOWAIT, {IPC_NOWAIT});
    assert_eq(SEM_UNDO, {SEM_UNDO});
    assert_eq(SETVAL, {SETVAL});
    assert_eq(IPC_STAT, {IPC_STAT});
    assert_eq(IPC_RMID, {IPC_RMID});
    return 0;
}}

"#,
    sem_num = offset!(sembuf, sem_num),
    sem_op = offset!(sembuf, sem_op),
    sem_flg = offset!(sembuf, sem_flg),
    sembuf = mem::size_of::<sembuf>(),

    keyfield = offset!(ipc_perm, __key),
    uid = offset!(ipc_perm, uid),
    gid = offset!(ipc_perm, gid),
    cuid = offset!(ipc_perm, cuid),
    cgid = offset!(ipc_perm, cgid),
    mode = offset!(ipc_perm, mode),
    seqfield = offset!(ipc_perm, __seq),
    ipc_perm = mem::size_of::<ipc_perm>(),

    sem_perm = offset!(semid_ds, sem_perm),
    sem_otime = offset!(semid_ds, sem_otime),
    // sem_ctime = offset!(semid_ds, sem_ctime),
    sem_nsems = offset!(semid_ds, sem_nsems),
    semid_ds = mem::size_of::<semid_ds>(),

    IPC_CREAT = super::consts::IPC_CREAT,
    IPC_EXCL = super::consts::IPC_EXCL,
    IPC_NOWAIT = super::consts::IPC_NOWAIT,
    SEM_UNDO = super::consts::SEM_UNDO,
    SETVAL = super::consts::SETVAL,
    IPC_STAT = super::consts::IPC_STAT,
    IPC_RMID = super::consts::IPC_RMID,

    key = if cfg!(target_os = "macos") {"_key"} else {"__key"},
    seq = if cfg!(target_os = "macos") {"_seq"} else {"__seq"},
).into_bytes()).unwrap();

        let arg = if cfg!(target_word_size = "32") {"-m32"} else {"-m64"};
        let s = Command::new("gcc").arg("-o").arg(td.path().join("foo"))
                                   .arg(td.path().join("foo.c"))
                                   .arg(arg).output().unwrap();
        if !s.status.success() {
            panic!("\n{}\n{}",
                  str::from_utf8(&s.stdout).unwrap(),
                  str::from_utf8(&s.stderr).unwrap());
        }
        let s = Command::new(td.path().join("foo")).output().unwrap();
        if !s.status.success() {
            panic!("\n{}\n{}",
                  str::from_utf8(&s.stdout).unwrap(),
                  str::from_utf8(&s.stderr).unwrap());
        }
    }
}
