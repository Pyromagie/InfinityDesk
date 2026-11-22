//! X11 programs (Steam, older games and tools) run through xwayland-satellite, like in niri.
//!
//! InfinityDesk claims a free X display `:N` itself (its lock file and sockets) and hands the
//! sockets to xwayland-satellite. Programs started right away (autostart) can connect at once:
//! their connection waits in the socket until Xwayland is up.
//!
//! If xwayland-satellite stops (e.g. it crashed), it is started again with the same sockets;
//! programs that connect meanwhile wait there.
//!
//! X11 programs connect to the abstract socket `@/tmp/.X11-unix/XN` first, which any user can
//! create. So a display only counts as free if we get that one too; otherwise another user could
//! listen in on our programs.

use std::{
    ffi::OsStr,
    fs,
    io::Write,
    os::{
        fd::{AsRawFd, BorrowedFd},
        linux::net::SocketAddrExt,
        unix::{
            fs::MetadataExt,
            net::{SocketAddr, UnixListener},
            process::CommandExt,
        },
    },
    path::PathBuf,
    process::{Child, Command},
    time::{Duration, Instant},
};

use smithay::reexports::rustix::io::{FdFlags, fcntl_setfd};

/// If xwayland-satellite stops sooner after starting, it is not started again (it would likely
/// stop again right away, e.g. for a missing library).
const MIN_RUN: Duration = Duration::from_secs(10);

/// The X display of the programs started from InfinityDesk. Removes its files when dropped.
pub struct XDisplay {
    /// `:N`, for `DISPLAY`.
    pub name: String,
    lock: PathBuf,
    socket: PathBuf,
    /// The abstract socket and the one in `/tmp/.X11-unix`.
    listeners: [UnixListener; 2],
}

impl XDisplay {
    /// `None`: xwayland-satellite is missing or there is no free display (logged).
    pub fn start(wayland_socket: &OsStr) -> Option<Self> {
        // Others cannot replace our socket file only in root's directory with the sticky bit.
        let dir = fs::metadata("/tmp/.X11-unix");
        if !dir.is_ok_and(|d| d.is_dir() && d.uid() == 0 && d.mode() & 0o1000 != 0) {
            tracing::warn!("No X11 programs: /tmp/.X11-unix is not root's with the sticky bit");
            return None;
        }
        for n in 0..64 {
            let lock = PathBuf::from(format!("/tmp/.X{n}-lock"));
            // A lock left behind by a process that is gone does not count.
            if let Ok(pid) = fs::read_to_string(&lock)
                && let Ok(pid) = pid.trim().parse::<u32>()
                && !PathBuf::from(format!("/proc/{pid}")).exists()
            {
                let _ = fs::remove_file(&lock);
            }
            let Ok(mut file) = (fs::OpenOptions::new().write(true).create_new(true)).open(&lock)
            else {
                continue; // taken
            };
            let _ = writeln!(file, "{:>10}", std::process::id());
            let socket = PathBuf::from(format!("/tmp/.X11-unix/X{n}"));
            let name = SocketAddr::from_abstract_name(socket.as_os_str().as_encoded_bytes());
            let Ok(abstract_listener) = name.and_then(|name| UnixListener::bind_addr(&name)) else {
                let _ = fs::remove_file(&lock);
                continue; // taken by someone without a lock file
            };
            // The display is ours now; a socket file still there is left over.
            let _ = fs::remove_file(&socket);
            let listener = match UnixListener::bind(&socket) {
                Ok(listener) => listener,
                Err(err) => {
                    tracing::warn!("No X11 programs: cannot create {socket:?}: {err}");
                    let _ = fs::remove_file(&lock);
                    return None;
                }
            };
            let display = Self {
                name: format!(":{n}"),
                lock,
                socket,
                listeners: [abstract_listener, listener],
            };
            return display.run_satellite(wayland_socket).then_some(display);
        }
        tracing::warn!("No X11 programs: no free X display");
        None
    }

    /// Starts xwayland-satellite, and again whenever it stops (see `MIN_RUN`).
    fn run_satellite(&self, wayland_socket: &OsStr) -> bool {
        // The sockets stay open as long as InfinityDesk runs (`self` lives until it ends).
        let fds = self.listeners.each_ref().map(|l| l.as_raw_fd());
        let (name, wayland_socket) = (self.name.clone(), wayland_socket.to_owned());
        let mut child = match spawn_satellite(&name, fds, &wayland_socket) {
            Ok(child) => child,
            Err(err) => {
                tracing::warn!("No X11 programs: cannot start xwayland-satellite: {err}");
                return false;
            }
        };
        tracing::info!("X11 programs on DISPLAY={name}");
        drop(std::thread::spawn(move || {
            loop {
                let started = Instant::now();
                let status = (child.wait()).map_or_else(|e| e.to_string(), |s| s.to_string());
                if started.elapsed() < MIN_RUN {
                    tracing::warn!(
                        "xwayland-satellite stopped right after starting ({status}), \
                         no new X11 programs until InfinityDesk is restarted"
                    );
                    return;
                }
                tracing::warn!("xwayland-satellite stopped ({status}), starting it again");
                child = match spawn_satellite(&name, fds, &wayland_socket) {
                    Ok(child) => child,
                    Err(err) => {
                        tracing::warn!("No X11 programs: cannot start xwayland-satellite: {err}");
                        return;
                    }
                };
            }
        }));
        true
    }
}

fn spawn_satellite(name: &str, fds: [i32; 2], wayland_socket: &OsStr) -> std::io::Result<Child> {
    let mut command = Command::new("xwayland-satellite");
    command.arg(name);
    for fd in fds {
        command.args(["-listenfd", &fd.to_string()]);
    }
    command.env("WAYLAND_DISPLAY", wayland_socket);
    // The sockets must stay open in xwayland-satellite (Rust closes them on exec by default).
    // SAFETY: only fcntl calls between fork and exec.
    unsafe {
        command.pre_exec(move || {
            for fd in fds {
                fcntl_setfd(BorrowedFd::borrow_raw(fd), FdFlags::empty())?;
            }
            Ok(())
        });
    }
    command.spawn()
}

impl Drop for XDisplay {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket);
        let _ = fs::remove_file(&self.lock);
    }
}
