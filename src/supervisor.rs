use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const GRACE_PERIOD: Duration = Duration::from_secs(3);
const DRAIN_INTERVAL_MS: i32 = 10;
const FORWARDED: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];
const WAKE: u8 = 0;

static SIGNAL_PIPE: AtomicI32 = AtomicI32::new(-1);

pub struct Supervisor {
    signals: File,
    wake: libc::c_int,
}

#[cfg(target_os = "linux")]
unsafe fn errno_location() -> *mut libc::c_int {
    unsafe { libc::__errno_location() }
}

#[cfg(not(target_os = "linux"))]
unsafe fn errno_location() -> *mut libc::c_int {
    unsafe { libc::__error() }
}

extern "C" fn record_signal(signal: libc::c_int) {
    unsafe {
        let saved = *errno_location();
        let byte = signal as u8;
        libc::write(
            SIGNAL_PIPE.load(Ordering::Relaxed),
            (&byte as *const u8).cast(),
            1,
        );
        *errno_location() = saved;
    }
}

fn check(result: libc::c_int) -> io::Result<()> {
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn set_disposition(signal: libc::c_int, handler: libc::sighandler_t) -> io::Result<()> {
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = handler;
        action.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut action.sa_mask);
        check(libc::sigaction(signal, &action, std::ptr::null_mut()))
    }
}

fn is_ignored(signal: libc::c_int) -> io::Result<bool> {
    unsafe {
        let mut current: libc::sigaction = std::mem::zeroed();
        check(libc::sigaction(signal, std::ptr::null(), &mut current))?;
        Ok(current.sa_sigaction == libc::SIG_IGN)
    }
}

fn with_sigttou_blocked<T>(action: impl FnOnce() -> T) -> T {
    unsafe {
        let mut blocked: libc::sigset_t = std::mem::zeroed();
        let mut previous: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut blocked);
        libc::sigaddset(&mut blocked, libc::SIGTTOU);
        libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, &mut previous);
        let result = action();
        libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut());
        result
    }
}

fn owns_terminal() -> bool {
    unsafe { libc::isatty(0) == 1 && libc::tcgetpgrp(0) == libc::getpgrp() }
}

fn give_terminal(group: libc::pid_t) {
    with_sigttou_blocked(|| unsafe { libc::tcsetpgrp(0, group) });
}

fn group_exists(group: libc::pid_t) -> bool {
    let alive = unsafe { libc::kill(-group, 0) } == 0;
    alive || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

impl Supervisor {
    pub fn install() -> io::Result<Self> {
        let mut ends = [0; 2];
        check(unsafe { libc::pipe(ends.as_mut_ptr()) })?;
        let signals = unsafe { File::from_raw_fd(ends[0]) };
        for end in ends {
            unsafe {
                check(libc::fcntl(end, libc::F_SETFD, libc::FD_CLOEXEC))?;
                check(libc::fcntl(end, libc::F_SETFL, libc::O_NONBLOCK))?;
            }
        }
        SIGNAL_PIPE.store(ends[1], Ordering::Relaxed);
        set_disposition(libc::SIGCHLD, libc::SIG_DFL)?;
        for signal in FORWARDED {
            if !is_ignored(signal)? {
                set_disposition(signal, record_signal as *const () as libc::sighandler_t)?;
            }
        }
        Ok(Self {
            signals,
            wake: ends[1],
        })
    }

    fn received(&self) -> Vec<libc::c_int> {
        let mut bytes = [0; 64];
        let mut received = Vec::new();
        while let Ok(count @ 1..) = (&self.signals).read(&mut bytes) {
            received.extend(
                bytes[..count]
                    .iter()
                    .filter(|byte| **byte != WAKE)
                    .map(|byte| libc::c_int::from(*byte)),
            );
        }
        received
    }

    pub fn pending(&self) -> Option<libc::c_int> {
        self.received().first().copied()
    }

    fn wait_readable(&self, timeout_ms: libc::c_int) {
        let mut descriptor = libc::pollfd {
            fd: self.signals.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
    }

    pub fn run(&self, mut command: Command) -> io::Result<ExitStatus> {
        let terminal = owns_terminal();
        command.process_group(0);
        if terminal {
            unsafe {
                command.pre_exec(|| {
                    give_terminal(libc::getpid());
                    Ok(())
                });
            }
        }
        let mut child = command.spawn()?;
        let group = child.id() as libc::pid_t;
        if terminal {
            give_terminal(group);
        }
        let (sender, receiver) = mpsc::channel();
        let wake = self.wake;
        std::thread::spawn(move || {
            let _ = sender.send(child.wait());
            unsafe { libc::write(wake, (&WAKE as *const u8).cast(), 1) };
        });
        let status = self.supervise(group, &receiver);
        if terminal {
            give_terminal(unsafe { libc::getpgrp() });
        }
        status
    }

    fn supervise(
        &self,
        group: libc::pid_t,
        receiver: &mpsc::Receiver<io::Result<ExitStatus>>,
    ) -> io::Result<ExitStatus> {
        let mut status = None;
        let mut deadline: Option<Instant> = None;
        loop {
            if status.is_none() {
                status = receiver.try_recv().ok();
            }
            match (&status, deadline) {
                (Some(_), None) => break,
                (Some(_), Some(_)) if !group_exists(group) => break,
                (_, Some(deadline)) if Instant::now() >= deadline => {
                    unsafe { libc::kill(-group, libc::SIGKILL) };
                    if status.is_none() {
                        status = receiver.recv().ok();
                    }
                    break;
                }
                _ => {}
            }
            self.wait_readable(if deadline.is_some() {
                DRAIN_INTERVAL_MS
            } else {
                -1
            });
            for signal in self.received() {
                unsafe { libc::kill(-group, signal) };
                deadline.get_or_insert_with(|| Instant::now() + GRACE_PERIOD);
            }
        }
        status.unwrap_or_else(|| Err(io::Error::other("command waiter failed")))
    }
}
