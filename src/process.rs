use anyhow::{Context, Result, ensure};
use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(10);
const GRACE: Duration = Duration::from_millis(250);

pub struct Captured {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: ExitStatus,
    pub interrupted: bool,
    pub timed_out: bool,
    pub capped: bool,
    /// A pipe stayed open after the command exited, or a pipe read failed.
    pub drain_incomplete: bool,
}

struct Drained {
    saved: Vec<u8>,
    incomplete: bool,
}

fn drain(
    mut reader: impl Read,
    limit: usize,
    overflow: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
) -> Drained {
    let mut saved = Vec::new();
    let mut buf = [0; 8192];
    let incomplete = loop {
        if stop.load(Ordering::SeqCst) {
            break true;
        }
        match reader.read(&mut buf) {
            Ok(0) => break false,
            Ok(n) => {
                let take = n.min(limit.saturating_sub(saved.len()));
                saved.extend_from_slice(&buf[..take]);
                // Continue draining excess bytes so a chatty command can still finish.
                if take < n {
                    overflow.store(true, Ordering::SeqCst);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(_) => break true,
        }
    };
    Drained { saved, incomplete }
}

#[cfg(unix)]
fn nonblocking(pipe: &impl std::os::fd::AsRawFd) -> Result<()> {
    let fd = pipe.as_raw_fd();
    // These descriptors are owned child-output pipes; preserve their other flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error()).context("set capture pipe nonblocking");
    }
    Ok(())
}

#[cfg(unix)]
fn signal_group(pid: u32, signal: i32) {
    // The child starts a dedicated group; this never targets the caller's group.
    unsafe { libc::kill(-(pid as i32), signal) };
}

/// Capture at most `limit` bytes per stream. The child receives closed stdin.
///
/// Supported targets are Unix: nonblocking pipes allow bounded cleanup even if
/// a detached descendant inherits them. Descendants should not outlive commands
/// wrapped by this API; lingering same-group descendants are terminated.
#[cfg(unix)]
pub fn capture(
    command: &mut Command,
    timeout: Duration,
    limit: usize,
    cancel: Arc<AtomicBool>,
) -> Result<Captured> {
    use std::os::unix::process::CommandExt;
    ensure!(limit > 0, "capture limit must be positive");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command
        .spawn()
        .context("start command (check doctor for missing adapters)")?;
    let pid = child.id();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    if let Err(error) = nonblocking(&stdout).and_then(|()| nonblocking(&stderr)) {
        signal_group(pid, libc::SIGKILL);
        let _ = child.wait();
        return Err(error);
    }
    let overflow = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let (flag, halt) = (overflow.clone(), stop.clone());
    let out = thread::spawn(move || drain(stdout, limit, flag, halt));
    let (flag, halt) = (overflow.clone(), stop.clone());
    let err = thread::spawn(move || drain(stderr, limit, flag, halt));
    let start = Instant::now();
    let mut interrupted = false;
    let mut timed_out = false;
    let mut status = None;
    let mut exited_at = None;
    let mut shutdown_at = None;
    let mut forced_drain_stop = false;
    loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(Some(done)) => {
                    status = Some(done);
                    exited_at = Some(Instant::now());
                }
                Ok(None) => {}
                Err(error) => {
                    signal_group(pid, libc::SIGKILL);
                    let _ = child.wait();
                    stop.store(true, Ordering::SeqCst);
                    let _ = out.join();
                    let _ = err.join();
                    return Err(error).context("wait for captured command");
                }
            }
        }
        if shutdown_at.is_none() {
            if let Some(exited) = exited_at {
                if out.is_finished() && err.is_finished() {
                    break;
                }
                // Do not turn an already completed command into a timeout because
                // an orphan (possibly in another session) still holds its pipes.
                if exited.elapsed() >= GRACE {
                    forced_drain_stop = true;
                    stop.store(true, Ordering::SeqCst);
                    signal_group(pid, libc::SIGTERM);
                    shutdown_at = Some(Instant::now());
                }
            } else {
                interrupted = cancel.load(Ordering::SeqCst);
                timed_out = !interrupted && start.elapsed() >= timeout;
                if interrupted || timed_out {
                    signal_group(
                        pid,
                        if interrupted {
                            libc::SIGINT
                        } else {
                            libc::SIGTERM
                        },
                    );
                    shutdown_at = Some(Instant::now());
                }
            }
        }
        if shutdown_at.is_some_and(|at| at.elapsed() >= GRACE) {
            signal_group(pid, libc::SIGKILL);
            stop.store(true, Ordering::SeqCst);
            if status.is_some() {
                break;
            }
        }
        thread::sleep(POLL);
    }
    let stdout = out
        .join()
        .map_err(|_| anyhow::anyhow!("stdout capture thread panicked"))?;
    let stderr = err
        .join()
        .map_err(|_| anyhow::anyhow!("stderr capture thread panicked"))?;
    Ok(Captured {
        stdout: stdout.saved,
        stderr: stderr.saved,
        status: status.expect("capture completes only after reaping its child"),
        interrupted,
        timed_out,
        capped: overflow.load(Ordering::SeqCst),
        drain_incomplete: forced_drain_stop || stdout.incomplete || stderr.incomplete,
    })
}

#[cfg(not(unix))]
pub fn capture(
    _command: &mut Command,
    _timeout: Duration,
    _limit: usize,
    _cancel: Arc<AtomicBool>,
) -> Result<Captured> {
    anyhow::bail!("command capture currently requires Unix nonblocking pipes")
}

pub fn exit_code(result: &Captured) -> i32 {
    if result.interrupted {
        return 130;
    }
    if result.timed_out {
        return 124;
    }
    result.status.code().unwrap_or_else(|| {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            128 + result.status.signal().unwrap_or(1)
        }
        #[cfg(not(unix))]
        {
            1
        }
    })
}
