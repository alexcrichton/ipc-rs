#![reexport_test_harness_main = "test_main"]

extern crate native;
extern crate ipc;

use std::os;
use std::io::Command;

#[start]
fn start(argc: int, argv: *const *const u8) -> int {
    native::start(argc, argv, main)
}

fn main() {
    match os::args().as_slice().get(1).map(|s| s.as_slice()) {
        Some("__runtest") => {}
        _ => return test_main(),
    }

    match os::args()[2].as_slice() {
        "test1" => {
            let _ = ipc::Semaphore::new("foo2", 0).unwrap().release();
            let _ = ipc::Semaphore::new("foo1", 0).unwrap().access();
        }
        s => fail!("unknown test: {}", s)
    }
}

fn me() -> Command {
    let mut me = Command::new(os::self_exe_name().unwrap());
    me.arg("__runtest");
    me
}

#[test]
fn foo() {
    let sem1 = ipc::Semaphore::new("foo1", 1).unwrap();
    let sem2 = ipc::Semaphore::new("foo2", 0).unwrap();
    let g1 = sem1.access();
    let mut p = me().arg("test1").spawn().unwrap();
    sem2.acquire();
    drop(g1);
    p.wait().unwrap();

    drop(sem1.access());
}
