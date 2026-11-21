//! X11 programs (Steam, older games and tools) run through xwayland-satellite, like in niri.
//!
//! InfinityDesk claims a free X display `:N` itself (its lock file and socket) and hands the socket
//! to xwayland-satellite. Programs started right away (autostart) can connect at once: their
//! connection waits in the socket until Xwayland is up.

use std::{
    ffi::OsStr,
    fs,
    io::Write,
    os::{
        fd::{AsRawFd, BorrowedFd},
        unix::{net::UnixListener, process::CommandExt},
    },
    path::PathBuf,
    process::Command,
};

use smithay::reexports::rustix::io::{FdFlags, fcntl_setfd};

/// The X display of the programs started from InfinityDesk. Removes its files when dropped.
pub struct XDisplay {
    /// `:N`, for `DISPLAY`.
    pub name: String,
    lock: PathBuf,
    socket: PathBuf,
    listener: UnixListener,
}

impl XDisplay {
    /// `None`: xwayland-satellite is missing or there is no free display (logged).
    pub fn start(wayland_socket: &OsStr) -> Option<Self> {
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
            // The display is ours now; a socket file still there is left over.
            let socket = PathBuf::from(format!("/tmp/.X11-unix/X{n}"));
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
                listener,
            };
            return display.spawn_satellite(wayland_socket).then_some(display);
        }
        tracing::warn!("No X11 programs: no free X display");
        None
    }

    fn spawn_satellite(&self, wayland_socket: &OsStr) -> bool {
        let fd = self.listener.as_raw_fd();
        let mut command = Command::new("xwayland-satellite");
        command.args([&self.name, "-listenfd", &fd.to_string()]);
        command.env("WAYLAND_DISPLAY", wayland_socket);
        // The socket must stay open in xwayland-satellite (Rust closes it on exec by default).
        // SAFETY: only an fcntl call between fork and exec.
        unsafe {
            command
                .pre_exec(move || Ok(fcntl_setfd(BorrowedFd::borrow_raw(fd), FdFlags::empty())?));
        }
        match command.spawn() {
            Ok(mut child) => {
                tracing::info!("X11 programs on DISPLAY={}", self.name);
                drop(std::thread::spawn(move || child.wait()));
                true
            }
            Err(err) => {
                tracing::warn!("No X11 programs: cannot start xwayland-satellite: {err}");
                false
            }
        }
    }
}

impl Drop for XDisplay {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket);
        let _ = fs::remove_file(&self.lock);
    }
}
