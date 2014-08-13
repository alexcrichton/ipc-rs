#![allow(non_camel_case_types)]
#![feature(unsafe_destructor)]

extern crate libc;

pub struct Semaphore {
    inner: imp::Semaphore,
}

#[must_use]
pub struct Guard<'a> {
    sem: &'a Semaphore,
}

impl Semaphore {
    pub fn new<T: ToCStr>(name: T, cnt: uint) -> Result<Semaphore, String> {
        Ok(Semaphore {
            inner: unsafe { try!(imp::Semaphore::new(name.to_c_str(), cnt)) }
        })
    }

    pub fn acquire(&self) { unsafe { self.inner.wait() } }
    pub fn try_acquire(&self) -> bool { unsafe { self.inner.try_wait() } }
    pub fn release(&self) { unsafe { self.inner.post() } }

    pub fn access(&self) -> Guard {
        self.acquire();
        Guard { sem: self }
    }
    pub fn try_access(&self) -> Option<Guard> {
        if self.try_acquire() {
            Some(Guard { sem: self })
        } else {
            None
        }
    }
}

#[unsafe_destructor]
impl<'a> Drop for Guard<'a> {
    fn drop(&mut self) {
        unsafe { self.sem.inner.post() }
    }
}

#[cfg(unix)]
mod imp {
    use libc;
    use std::c_str::CString;
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
        pub unsafe fn new(name: CString, cnt: uint) -> Result<Semaphore, String> {
            let sem = sem_open(name.as_ptr(),
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

}

#[cfg(test)]
mod tests {
    use Semaphore;

    #[test]
    fn smoke() {
        let s = Semaphore::new("/ipc-rs-test2", 1).unwrap();
        drop(s.access());
        assert!(s.try_access().is_some());
    }
}
