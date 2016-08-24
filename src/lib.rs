//! Inter-process communication library for Rust
//!
//! This library houses some inter-process communication utilities available
//! across many platforms. The current primary interface is a `Semaphore` which
//! is modeled after the `std::sync::Semaphore` structure.
//!
//! > **Warning**: This crate is only compatible with `libnative` currently.
//! > Crates taking advantage of `libgreen` will not be able to use this crate.
//! > To work around this, a helper thread may be necessary.
//!
//! # Example
//!
//! ```
//! use ipc::Semaphore;
//!
//! let s = match Semaphore::new("my-fun-semaphore", 1) {
//!     Ok(sem) => sem,
//!     Err(s) => panic!("failed to create a semaphore: {}", s)
//! };
//!
//! // lock the semaphore
//! let guard = s.access();
//!
//! // unlock the semaphore
//! drop(guard);
//!
//! // manage the semaphore count manually
//! s.acquire();
//! s.release();
//! ```

#![allow(non_camel_case_types)]

extern crate libc;

use std::io::Result;

/// An atomic counter which can be shared across processes.
///
/// This counter will block the current process in `access` or `acquire` when
/// the count is 0, waiting for a process to invoke `release` through some
/// mechanism.
pub struct Semaphore {
    inner: imp::Semaphore,
}

/// An RAII guard used to release a semaphore automatically when it falls out
/// of scope.
#[must_use]
pub struct Guard<'a> {
    sem: &'a Semaphore,
}

impl Semaphore {
    /// Creates a new semaphore with the given name and count.
    ///
    /// If the current system has no semaphore named `name`, then a new
    /// semaphore will be created with the initial count `cnt`.
    ///
    /// If the current system already has a semaphore named `name`, then a
    /// handle to that semaphore will be returned and `cnt` will be ignored.
    ///
    /// Note that the name provided will be mangled as necessary when passed to
    /// the underlying system, so the name is not necessarily compatible with
    /// other processes using semaphores.
    ///
    /// # Errors
    ///
    /// Any errors which occur when creating a semaphore are returned in string
    /// form.
    ///
    /// # Example
    ///
    /// ```
    /// use ipc::Semaphore;
    ///
    /// // sem1/sem2 are handles to the same semaphore
    /// let sem1 = Semaphore::new("foo", 1).unwrap();
    /// let sem2 = Semaphore::new("foo", 1 /* ignored */).unwrap();
    /// ```
    pub fn new(name: &str, cnt: usize) -> Result<Semaphore> {
        Ok(Semaphore {
            inner: unsafe { try!(imp::Semaphore::new(name, cnt)) }
        })
    }

    /// Acquire a resource of this semaphore.
    ///
    /// This function will block until a resource is available (a count > 0),
    /// and then decrement it and return.
    pub fn acquire(&self) { unsafe { self.inner.wait() } }

    /// Attempt to acquire a resource of this semaphore.
    ///
    /// This function is identical to `acquire` except that it will never
    /// blocked. This function returns `true` if a resource was acquired or
    /// `false` if one could not be acquired.
    pub fn try_acquire(&self) -> bool { unsafe { self.inner.try_wait() } }

    /// Release a resource of this semaphore.
    ///
    /// This function will increment the count of this semaphore, waking up any
    /// waiters who would like the resource.
    pub fn release(&self) { unsafe { self.inner.post() } }

    /// Access a resource of this semaphore in a constrained scope.
    ///
    /// This function will first acquire a resource and then return an RAII
    /// guard structure which will release the resource when it falls out of
    /// scope. For a mutex-like semaphore, it is recommended to use this method
    /// rather than the `acquire` or `release` methods.
    pub fn access(&self) -> Guard {
        self.acquire();
        Guard { sem: self }
    }

    /// Attempt to access a resource of this semaphore.
    ///
    /// This function is identical to `access` except that it will never block.
    pub fn try_access(&self) -> Option<Guard> {
        if self.try_acquire() {
            Some(Guard { sem: self })
        } else {
            None
        }
    }
}

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
        let s = Semaphore::new("smoke", 1).unwrap();
        drop(s.access());
        {
            let _g = s.access();
            assert!(s.try_access().is_none());
            assert!(!s.try_acquire());
        }
        assert!(s.try_access().is_some());
    }

    #[test]
    fn create_twice() {
        let _s1 = Semaphore::new("create_twice", 1).unwrap();
        let _s2 = Semaphore::new("create_twice", 0).unwrap();
    }

    #[test]
    fn check_send() {
        fn send<S: Send>(_: &S) {}
        send(&Semaphore::new("send", 1).unwrap());
    }

    #[test]
    fn check_sync() {
        fn send<S: Sync>(_: &S) {}
        send(&Semaphore::new("sync", 1).unwrap());
    }
}
