#![reexport_test_harness_main = "test_main"]

extern crate ipc;

use std::env;
use std::process::Command;

fn main() {
    let mut args = env::args();
    args.next().unwrap();
    for arg in args {
        println!("Enter: {}", arg);
        match &arg as &str {
            "test1_inner" => {
                let sem1 = ipc::Semaphore::new("foo1", 0).unwrap();
                let sem2 = ipc::Semaphore::new("foo2", 0).unwrap();
                let _ = sem2.release();
                let _ = sem1.access();
            }
            "test1" => first_pass(),
            v => panic!("Unknown test: {}", v),
        }
        println!("Leave: {}", arg);
    }
}

fn me() -> Command {
    Command::new(env::current_exe().unwrap())
}

fn first_pass() {
    let sem1 = ipc::Semaphore::new("foo1", 1).unwrap();
    let sem2 = ipc::Semaphore::new("foo2", 0).unwrap();
    let g1 = sem1.access();
    let mut p = me().arg("test1_inner").spawn().unwrap();
    sem2.acquire();
    drop(g1);
    p.wait().unwrap();
    drop(sem1.access());
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::process::Command;
    use std::str;

    #[test]
    fn test1() {
        let test_exe = env::current_exe().unwrap();
        let mut bin = test_exe.with_file_name("test");
        bin = match test_exe.extension() {
            Some(v) => bin.with_extension(v),
            None => bin,
        };
        let output = Command::new(bin).arg("test1").output().unwrap();
        assert! (output.status.success());
        assert_eq! (str::from_utf8(&output.stdout).unwrap(), 
r#"Enter: test1
Enter: test1_inner
Leave: test1_inner
Leave: test1
"#);
    }
}