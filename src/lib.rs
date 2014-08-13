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
    pub fn new(name: &str, cnt: uint) -> Result<Semaphore, String> {
        Ok(Semaphore {
            inner: unsafe { try!(imp::Semaphore::new(name, cnt)) }
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

#[cfg(unix)] #[path = "unix.rs"] mod imp;
#[cfg(windows)] #[path = "windows.rs"] mod imp;

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
