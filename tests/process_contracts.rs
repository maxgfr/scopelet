#![cfg(unix)]

use scopelet::process::{self, Captured};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, atomic::AtomicBool, mpsc};
use std::thread;
use std::time::{Duration, Instant};

fn capture(mut command: Command, timeout: Duration, limit: usize) -> Captured {
    // A broken implementation must fail this regression rather than hang CI.
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = process::capture(
            &mut command,
            timeout,
            limit,
            Arc::new(AtomicBool::new(false)),
        );
        let _ = sender.send(result);
    });
    receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("capture did not return within five seconds")
        .unwrap()
}

fn shell(script: &str) -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", script]);
    command
}

#[test]
fn preserves_nonzero_exit_and_separate_streams() {
    let result = capture(
        shell("printf exact-out; printf exact-err >&2; exit 7"),
        Duration::from_secs(2),
        1024,
    );
    assert_eq!(process::exit_code(&result), 7);
    assert_eq!(result.stdout, b"exact-out");
    assert_eq!(result.stderr, b"exact-err");
    assert!(!result.capped);
    assert!(!result.drain_incomplete);
}

#[test]
fn cap_discards_excess_without_preventing_command_side_effects() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("completed");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "import os,sys; os.write(1,b'x'*2000000); os.write(2,b'y'*2000000); open(sys.argv[1],'w').write('done')",
        marker.to_str().unwrap(),
    ]);
    let result = capture(command, Duration::from_secs(3), 1024);
    assert_eq!(process::exit_code(&result), 0);
    assert_eq!(result.stdout, vec![b'x'; 1024]);
    assert_eq!(result.stderr, vec![b'y'; 1024]);
    assert!(result.capped);
    assert!(!result.drain_incomplete);
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "done");
}

#[test]
fn timeout_allows_term_handler_then_returns_124() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("term-received");
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "import signal,time,sys; signal.signal(signal.SIGTERM,lambda *_: (open(sys.argv[1],'w').write('term'),sys.exit(0))); time.sleep(30)",
        marker.to_str().unwrap(),
    ]);
    let start = Instant::now();
    let result = capture(command, Duration::from_millis(500), 1024);
    assert_eq!(process::exit_code(&result), 124);
    assert!(result.timed_out);
    assert!(!result.interrupted);
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "term");
    assert!(start.elapsed() < Duration::from_secs(3));
}

#[test]
fn cancellation_returns_130_and_is_bounded_even_when_int_is_ignored() {
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let start = Instant::now();
    let result = process::capture(
        &mut shell("trap '' INT; sleep 30"),
        Duration::from_secs(3),
        1024,
        cancel,
    )
    .unwrap();
    assert_eq!(process::exit_code(&result), 130);
    assert!(result.interrupted);
    assert!(!result.timed_out);
    assert!(start.elapsed() < Duration::from_secs(2));
}

struct Descendant(PathBuf);
impl Drop for Descendant {
    fn drop(&mut self) {
        if let Some(pid) = std::fs::read_to_string(&self.0)
            .ok()
            .and_then(|value| value.parse::<i32>().ok())
        {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
}

fn pipe_holder(detached: bool) {
    let dir = tempfile::tempdir().unwrap();
    let guard = Descendant(dir.path().join("descendant.pid"));
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "import os,sys,time\npid=os.fork()\nif pid==0:\n if sys.argv[2]=='yes': os.setsid()\n open(sys.argv[1],'w').write(str(os.getpid()))\n os.write(1,b'grandchild')\n time.sleep(30)\n os._exit(0)\nwhile not os.path.exists(sys.argv[1]): time.sleep(.001)\nos._exit(7)",
        guard.0.to_str().unwrap(),
        if detached { "yes" } else { "no" },
    ]);
    let start = Instant::now();
    let result = capture(command, Duration::from_secs(3), 1024);
    assert_eq!(process::exit_code(&result), 7);
    assert_eq!(result.stdout, b"grandchild");
    assert!(result.drain_incomplete);
    assert!(!result.timed_out);
    assert!(!result.interrupted);
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn parent_exit_with_inherited_pipes_keeps_parent_status() {
    pipe_holder(false);
}

#[test]
fn detached_pipe_holder_cannot_hang_reader_join() {
    pipe_holder(true);
}
