use libc;
use std::i32;
use std::os;

pub struct Semaphore { handle: libc::HANDLE }

pub static WAIT_FAILED: libc::DWORD = 0xFFFFFFFF;
pub static WAIT_TIMEOUT: libc::DWORD = 0x00000102;

extern "system" {
    fn CreateSemaphoreW(lpSemaphoreAttributes: libc::LPSECURITY_ATTRIBUTES,
                        lInitialCount: libc::LONG,
                        lMaximumCount: libc::LONG,
                        lpName: libc::LPCWSTR) -> libc::HANDLE;
    fn ReleaseSemaphore(hSemaphore: libc::HANDLE,
                        lReleaseCount: libc::LONG,
                        lpPreviousCount: *mut libc::LONG) -> libc::BOOL;
}

impl Semaphore {
    pub unsafe fn new(name: &str, cnt: uint) -> Result<Semaphore, String> {
        let name = format!(r"Global\{}", name);
        let mut name = name.as_slice().utf16_units().collect::<Vec<u16>>();
        name.push(0);
        let handle = CreateSemaphoreW(0 as *mut _,
                                      cnt as libc::LONG,
                                      i32::MAX as libc::LONG,
                                      name.as_ptr());
        if handle.is_null() {
            Err(os::last_os_error())
        } else {
            Ok(Semaphore { handle: handle })
        }
    }

    pub unsafe fn wait(&self) {
        match libc::WaitForSingleObject(self.handle, libc::INFINITE) {
            libc::WAIT_OBJECT_0 => {},
            WAIT_FAILED => fail!("failed to wait: {}", os::last_os_error()),
            n => fail!("bad wait(): {}/{}", n, os::errno()),
        }
    }

    pub unsafe fn try_wait(&self) -> bool {
        match libc::WaitForSingleObject(self.handle, 0) {
            libc::WAIT_OBJECT_0 => true,
            WAIT_TIMEOUT => false,
            WAIT_FAILED => fail!("failed to wait: {}", os::last_os_error()),
            n => fail!("bad wait(): {}/{}", n, os::errno()),
        }
    }

    pub unsafe fn post(&self) {
        match ReleaseSemaphore(self.handle, 1, 0 as *mut _) {
            0 => fail!("failed to release semaphore: {}", os::last_os_error()),
            _ => {}
        }
    }
}

impl Drop for Semaphore {
    fn drop(&mut self) {
        unsafe { libc::CloseHandle(self.handle); }
    }
}

