//! Commands from outside: `infinitydesk msg eval '<lua>'` runs Lua code in the running
//! config's state, like a function given to `desk.bind`, and prints what it returns as JSON
//! (one line per value), e.g. `infinitydesk msg eval 'return desk.windows()'`.
//!
//! They talk over a Unix socket in `$XDG_RUNTIME_DIR` (a folder only the user can enter, so
//! nobody else can run code in the session; without it there is no socket). Programs started
//! inside find it in `$INFINITYDESK_SOCKET`. The client sends the code and closes its writing side; the answer
//! is `ok` or `error` on the first line, then the text.

use std::{
    ffi::OsStr,
    io::{ErrorKind, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Duration,
};

use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction, generic::Generic};

use crate::InfinityDesk;

/// Longer requests are refused.
const MAX_REQUEST: usize = 1 << 20;

pub fn socket_path(wayland_socket: &OsStr) -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    let name = format!("infinitydesk.{}.sock", wayland_socket.to_string_lossy());
    Some(dir.join(name))
}

/// Listens for `infinitydesk msg` on `path`.
pub fn listen(handle: &LoopHandle<'static, InfinityDesk>, path: &Path) -> std::io::Result<()> {
    // Left over when InfinityDesk crashed on this Wayland socket before.
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    // Only for the user, also if the folder should be open to others.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let handle2 = handle.clone();
    let source = Generic::new(listener, Interest::READ, Mode::Level);
    handle
        .insert_source(source, move |_, listener, _| {
            while let Ok((stream, _)) = listener.accept() {
                if let Err(err) = serve(&handle2, stream) {
                    tracing::warn!("A message from outside failed: {err}");
                }
            }
            Ok(PostAction::Continue)
        })
        .map_err(|err| err.error)?;
    Ok(())
}

/// Reads one request without blocking the compositor, then answers it.
fn serve(handle: &LoopHandle<'static, InfinityDesk>, stream: UnixStream) -> std::io::Result<()> {
    stream.set_nonblocking(true)?;
    let mut request = Vec::new();
    let source = Generic::new(stream, Interest::READ, Mode::Level);
    let inserted = handle.insert_source(source, move |_, stream, state| {
        let mut stream: &UnixStream = stream;
        let mut buf = [0; 4096];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) if request.len() + n <= MAX_REQUEST => request.extend_from_slice(&buf[..n]),
                Ok(_) => {
                    answer(stream, Err("the request is too long".into()));
                    return Ok(PostAction::Remove);
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => return Ok(PostAction::Continue),
                Err(err) if err.kind() == ErrorKind::Interrupted => {}
                Err(_) => return Ok(PostAction::Remove),
            }
        }
        let request = String::from_utf8_lossy(&request);
        let reply = match request.split_once(' ') {
            Some(("eval", code)) => state.eval_lua(code),
            _ => Err(format!("unknown request: {request}")),
        };
        answer(stream, reply);
        Ok(PostAction::Remove)
    });
    inserted.map(|_| ()).map_err(|err| err.error.into())
}

fn answer(mut stream: &UnixStream, reply: Result<String, String>) {
    let text = match reply {
        Ok(text) => format!("ok\n{text}"),
        Err(text) => format!("error\n{text}"),
    };
    // A short wait at most, if the client does not read.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let _ = stream.write_all(text.as_bytes());
}

/// `infinitydesk msg ...`: sends the request, prints the answer; returns the exit code.
pub fn send(args: &[String]) -> i32 {
    let usage = "usage: infinitydesk msg eval '<lua code>'";
    let request = match args {
        [what, code] if what == "eval" => format!("eval {code}"),
        _ => return fail(usage),
    };
    let Some(path) = std::env::var_os("INFINITYDESK_SOCKET") else {
        return fail("INFINITYDESK_SOCKET is not set (programs started in InfinityDesk have it)");
    };
    let talk = || -> std::io::Result<String> {
        let mut stream = UnixStream::connect(&path)?;
        stream.write_all(request.as_bytes())?;
        stream.shutdown(std::net::Shutdown::Write)?;
        let mut reply = String::new();
        stream.read_to_string(&mut reply)?;
        Ok(reply)
    };
    match talk() {
        Ok(reply) => match reply.split_once('\n') {
            Some(("ok", text)) => {
                print!("{text}");
                0
            }
            Some((_, text)) => fail(text),
            None => fail("InfinityDesk gave no answer"),
        },
        Err(err) => fail(&format!("could not reach InfinityDesk at {path:?}: {err}")),
    }
}

fn fail(text: &str) -> i32 {
    eprintln!("{text}");
    1
}
