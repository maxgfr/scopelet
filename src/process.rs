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

#[cfg(unix)]
struct Drained {
    saved: Vec<u8>,
    incomplete: bool,
}

/// Wait until the pipe has bytes, or POLL elapses so cancellation stays responsive.
/// Sleeping instead would cap a stream at one pipe buffer per poll interval.
#[cfg(unix)]
fn wait_readable(fd: std::os::fd::RawFd) {
    let mut watch = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    unsafe { libc::poll(&mut watch, 1, POLL.as_millis() as libc::c_int) };
}

#[cfg(unix)]
fn drain(
    mut reader: impl Read + std::os::fd::AsRawFd,
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
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                wait_readable(reader.as_raw_fd())
            }
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

/// Turn SIGINT, SIGTERM and SIGHUP into `cancel`, replacing whatever the
/// caller left (an ignored SIGHUP under `nohup`, say), so a wrapped command is
/// stopped and its status reported (130) instead of this process dying first.
///
/// A direct `sigaction` rather than a signal-handling crate: on Apple
/// platforms that crate links the Foundation framework, whose loading cost
/// most of a millisecond on every invocation, every hook included.
#[cfg(unix)]
pub fn cancel_on_termination(cancel: Arc<AtomicBool>) -> Result<()> {
    static CANCEL: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();
    extern "C" fn handle(_: libc::c_int) {
        // An atomic load and an atomic store: async-signal-safe.
        if let Some(cancel) = CANCEL.get() {
            cancel.store(true, Ordering::SeqCst);
        }
    }
    ensure!(
        CANCEL.set(cancel).is_ok(),
        "termination handler already installed"
    );
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: a zeroed sigaction is a valid starting value, and the
        // handler only touches an initialized atomic.
        let installed = unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handle as extern "C" fn(libc::c_int) as libc::sighandler_t;
            action.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, std::ptr::null_mut())
        };
        if installed != 0 {
            return Err(std::io::Error::last_os_error()).context("install termination handler");
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn cancel_on_termination(cancel: Arc<AtomicBool>) -> Result<()> {
    ctrlc::set_handler(move || cancel.store(true, Ordering::SeqCst))?;
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
    // Any command comes through here, a user's own as well as an adapter's:
    // the error names what could not start; callers add their own advice.
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .spawn()
        .with_context(|| format!("start {program}"))?;
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
    // Each reader says when its stream ends, which is when a command finishes:
    // the loop below wakes for it instead of finding out up to POLL later.
    let (ended_tx, ended_rx) = std::sync::mpsc::channel::<()>();
    let (flag, halt, ended) = (overflow.clone(), stop.clone(), ended_tx.clone());
    let out = thread::spawn(move || {
        let drained = drain(stdout, limit, flag, halt);
        let _ = ended.send(());
        drained
    });
    let (flag, halt, ended) = (overflow.clone(), stop.clone(), ended_tx);
    let err = thread::spawn(move || {
        let drained = drain(stderr, limit, flag, halt);
        let _ = ended.send(());
        drained
    });
    // Streams known to have ended, and the wait once both have while the
    // command still runs (it closed its outputs): short, then up to POLL.
    let mut streams_ended = 0;
    let mut idle = Duration::from_millis(1);
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
                if streams_ended == 2 || (out.is_finished() && err.is_finished()) {
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
        // At most POLL, so a timeout or an interruption stays responsive; less
        // when a stream ends, so a command of a millisecond does not cost ten.
        match ended_rx.recv_timeout(POLL) {
            Ok(()) => streams_ended += 1,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                streams_ended = 2;
                thread::sleep(idle);
                idle = (idle * 2).min(POLL);
            }
        }
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
