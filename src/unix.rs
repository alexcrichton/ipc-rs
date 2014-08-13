use libc;
use std::os;
use std::io;

enum sem_t {}

pub struct Semaphore { ptr: *mut sem_t }

extern {
    fn sem_open(name: *const libc::c_char, oflag: libc::c_int,
                mode: libc::mode_t, value: libc::c_uint) -> *mut sem_t;
    fn sem_close(sem: *mut sem_t) -> libc::c_int;
    fn sem_wait(sem: *mut sem_t) -> libc::c_int;
    fn sem_trywait(sem: *mut sem_t) -> libc::c_int;
    fn sem_post(sem: *mut sem_t) -> libc::c_int;
}

impl Semaphore {
    pub unsafe fn new(name: &str, cnt: uint) -> Result<Semaphore, String> {
        let name = format!("/{}\0", name);
        let sem = sem_open(name.as_slice().as_ptr() as *const libc::c_char,
                           libc::O_CREAT,
                           io::UserRWX.bits() as libc::mode_t,
                           cnt as libc::c_uint);
        if sem.is_null() {
            Err(os::last_os_error())
        } else {
            Ok(Semaphore { ptr: sem })
        }
    }

    pub unsafe fn wait(&self) {
        loop {
            match sem_wait(self.ptr) {
                0 => return,
                libc::EINTR => {}
                n => fail!("unknown error in sem_wait: [{}] {}", n,
                           os::last_os_error())
            }
        }
    }

    pub unsafe fn try_wait(&self) -> bool {
        loop {
            match sem_trywait(self.ptr) {
                0 => return true,
                libc::EINTR => {}
                libc::EAGAIN => return false,
                n => fail!("unknown error in sem_wait: [{}] {}", n,
                           os::last_os_error())
            }
        }
    }

    pub unsafe fn post(&self) {
        match sem_post(self.ptr) {
            0 => {},
            n => fail!("unknown error in sem_post: [{}] {}", n,
                       os::last_os_error())
        }
    }
}

impl Drop for Semaphore {
    fn drop(&mut self) {
        unsafe { sem_close(self.ptr); }
    }
}
