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

use libc;
use libc::consts::os::posix88::{EEXIST, O_RDWR};
use std::os;
use std::mem;
use std::hash;
use std::io::{mod, IoResult, IoError, fs};
use std::task;

use self::consts::{IPC_CREAT, IPC_EXCL, key_t, sembuf, SEM_UNDO, IPC_NOWAIT};
use self::consts::{IPC_STAT, IPC_RMID, SETVAL, semid_ds};

pub struct Semaphore { semid: libc::c_int }

#[cfg(target_os = "linux")]
mod consts {
    use libc;

    pub type key_t = i32;

    pub static IPC_CREAT: libc::c_int = 01000;
    pub static IPC_EXCL: libc::c_int = 02000;
    pub static IPC_NOWAIT: libc::c_short = 04000;
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

extern {
    fn ftok(pathname: *const libc::c_char, proj_id: libc::c_int) -> key_t;
    fn semget(key: key_t, nsems: libc::c_int, semflg: libc::c_int) -> libc::c_int;
    fn semctl(semid: libc::c_int, semnum: libc::c_int,
              cmd: libc::c_int, ...) -> libc::c_int;
    fn semop(semid: libc::c_int, sops: *mut sembuf,
             nsops: libc::c_uint) -> libc::c_int;
}

impl Semaphore {
    pub unsafe fn new(name: &str, cnt: uint) -> IoResult<Semaphore> {
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
        let mut semid = semget(key, 1, IPC_CREAT | IPC_EXCL | 0666);
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
            if semctl(semid, 0, SETVAL, 0u) != 0 ||
               semop(semid, &mut buf, 1) != 0 {
                let err = IoError::last_error();
                semctl(semid, 0, IPC_RMID);
                return Err(err)
            }
        } else if os::errno() as libc::c_int == EEXIST {
            // Re-attempt to get the semaphore, this should in theory always
            // succeed?
            semid = semget(key, 1, 0);
            if semid < 0 { return Err(IoError::last_error()) }

            // Spin in a small loop waiting for sem_otime to become not 0
            let ok = range(0u, 1000).any(|_| {
                let mut buf: semid_ds = mem::zeroed();
                semctl(semid, 0, IPC_STAT, &mut buf);
                if buf.sem_otime == 0 {
                    task::deschedule();
                    false
                } else {
                    true
                }
            });
            if !ok {
                return Err(IoError {
                    kind: io::TimedOut,
                    desc: "timed out waiting for sem to be initialized",
                    detail: None,
                })
            }
        } else {
            return Err(IoError::last_error())
        }

        // Phew! That took long enough...
        Ok(Semaphore { semid: semid })
    }

    /// Generate the filename which will be passed to ftok, keyed off the given
    /// semaphore name `name`.
    fn filename(name: &str) -> Path {
        let hash = hash::hash(&(name, "ipc-rs"));
        let filename = name.chars().filter(|a| {
            (*a as u32) < 128 && a.is_alphanumeric()
        }).collect::<String>();
        os::tmpdir().join("ipc-rs-sems").join(format!("{}-{}", filename, hash))
    }

    /// Generate the `key_t` from `ftok` which will be passed to `semget`.
    ///
    /// This function will ensure that the relevant file is located on the
    /// filesystem and will then invoke ftok on it.
    unsafe fn key(name: &str) -> IoResult<key_t> {
        let filename = Semaphore::filename(name);
        if !filename.dir_path().exists() {
            // As long as someone creates the directory we're alright.
            let _ = fs::mkdir(&filename.dir_path(), io::UserRWX);
        }

        // Make sure that the file exists. Open it in exclusive/create mode to
        // ensure that it's there, but don't overwrite it if it alredy exists.
        //
        // see QSharedMemoryPrivate::createUnixKeyFile in Qt
        let filename = filename.to_c_str();
        let fd = libc::open(filename.as_ptr(),
                            libc::O_EXCL | libc::O_CREAT | O_RDWR,
                            0640);
        if fd > 0 {
            libc::close(fd);
        } else if os::errno() as libc::c_int != EEXIST {
            return Err(IoError::last_error())
        }

        // Invoke `ftok` with our filename
        let key = ftok(filename.as_ptr(), 'I' as libc::c_int);
        if key != -1 {Ok(key)} else {Err(IoError::last_error())}
    }

    pub unsafe fn wait(&self) {
        loop {
            if self.modify(-1, true) == 0 { return }

            match os::errno() as libc::c_int {
                libc::EINTR => {}
                n => fail!("unknown wait error: [{}] {}", n, os::last_os_error())
            }
        }
    }

    pub unsafe fn try_wait(&self) -> bool {
        if self.modify(-1, false) == 0 { return true }

        match os::errno() as libc::c_int {
            libc::EAGAIN => return false,
            n => fail!("unknown try_wait error: [{}] {}", n, os::last_os_error())
        }
    }

    pub unsafe fn post(&self) {
        if self.modify(1, true) == 0 { return }
        fail!("unknown post error: [{}] {}", os::errno(), os::last_os_error())
    }

    unsafe fn modify(&self, amt: int, wait: bool) -> libc::c_int {
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
    use std::io::{TempDir, Command, File};
    use std::str;
    use std::mem;

    use super::consts::{sembuf, semid_ds, ipc_perm};

    macro_rules! offset( ($ty:ident, $f:ident) => (unsafe {
        let f = 0 as *const $ty;
        &(*f).$f as *const _ as uint
    }) )

    #[test]
    fn check_offsets() {
        let td = TempDir::new("test").unwrap();
        let mut f = File::create(&td.path().join("foo.c")).unwrap();
        f.write_str(format!(r#"
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
    assert_eq(offsetof(struct sembuf, sem_num), {});
    assert_eq(offsetof(struct sembuf, sem_op), {});
    assert_eq(offsetof(struct sembuf, sem_flg), {});
    assert_eq(sizeof(struct sembuf), {});

    assert_eq(offsetof(struct ipc_perm, __key), {});
    assert_eq(offsetof(struct ipc_perm, uid), {});
    assert_eq(offsetof(struct ipc_perm, gid), {});
    assert_eq(offsetof(struct ipc_perm, cuid), {});
    assert_eq(offsetof(struct ipc_perm, cgid), {});
    assert_eq(offsetof(struct ipc_perm, mode), {});
    assert_eq(offsetof(struct ipc_perm, __seq), {});
    assert_eq(sizeof(struct ipc_perm), {});

    assert_eq(offsetof(struct semid_ds, sem_perm), {});
    assert_eq(offsetof(struct semid_ds, sem_otime), {});
    assert_eq(offsetof(struct semid_ds, sem_ctime), {});
    assert_eq(offsetof(struct semid_ds, sem_nsems), {});
    assert_eq(sizeof(struct semid_ds), {});
    return 0;
}}

"#,
    offset!(sembuf, sem_num),
    offset!(sembuf, sem_op),
    offset!(sembuf, sem_flg),
    mem::size_of::<sembuf>(),

    offset!(ipc_perm, __key),
    offset!(ipc_perm, uid),
    offset!(ipc_perm, gid),
    offset!(ipc_perm, cuid),
    offset!(ipc_perm, cgid),
    offset!(ipc_perm, mode),
    offset!(ipc_perm, __seq),
    mem::size_of::<ipc_perm>(),

    offset!(semid_ds, sem_perm),
    offset!(semid_ds, sem_otime),
    offset!(semid_ds, sem_ctime),
    offset!(semid_ds, sem_nsems),
    mem::size_of::<semid_ds>(),
).as_slice()).unwrap();

        let arg = if cfg!(target_word_size = "32") {"-m32"} else {"-m64"};
        let s = Command::new("gcc").arg("-o").arg(td.path().join("foo"))
                                   .arg(td.path().join("foo.c"))
                                   .arg(arg).output().unwrap();
        if !s.status.success() {
            fail!("\n{}\n{}",
                  str::from_utf8(s.output.as_slice()).unwrap(),
                  str::from_utf8(s.error.as_slice()).unwrap());
        }
        let s = Command::new(td.path().join("foo")).output().unwrap();
        if !s.status.success() {
            fail!("\n{}\n{}",
                  str::from_utf8(s.output.as_slice()).unwrap(),
                  str::from_utf8(s.error.as_slice()).unwrap());
        }
    }
}
