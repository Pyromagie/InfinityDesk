//! Commands from outside: `infinitydesk msg eval '<lua>'` runs Lua code in the running
//! config's state, like a function given to `desk.bind`, and prints what it returns as JSON
//! (one line per value), e.g. `infinitydesk msg eval 'return desk.windows()'`.
//! `infinitydesk msg events` prints the state and then every event as a line of JSON, for bars
//! (see `InfinityDesk::add_event_stream`).
//!
//! They talk over a Unix socket in `$XDG_RUNTIME_DIR` (a folder only the user can enter, so
//! nobody else can run code in the session; without it there is no socket). Programs started
//! inside find it in `$INFINITYDESK_SOCKET`. The client sends the code and closes its writing side; the answer
//! is `ok` or `error` on the first line, then the text.

use std::{
    ffi::OsStr,
    io::{BufRead, BufReader, ErrorKind, Read, Write},
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
        if request == "events" {
            // The connection stays open for the events (the copy shares the connection).
            match stream.try_clone() {
                Ok(stream) => state.add_event_stream(stream),
                Err(err) => answer(stream, Err(format!("events: {err}"))),
            }
            return Ok(PostAction::Remove);
        }
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
        Err(text) => format!("error\n{text}\n"),
    };
    // A short wait at most, if the client does not read.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let _ = stream.write_all(text.as_bytes());
}

/// `infinitydesk msg ...`: sends the request, prints the answer (for `events` until InfinityDesk
/// quits); returns the exit code.
pub fn send(args: &[String]) -> i32 {
    let usage = "usage: infinitydesk msg eval '<lua code>'\n       infinitydesk msg events";
    let request = match args {
        [what, code] if what == "eval" => format!("eval {code}"),
        [what] if what == "events" => what.clone(),
        _ => return fail(usage),
    };
    let Some(path) = std::env::var_os("INFINITYDESK_SOCKET") else {
        return fail("INFINITYDESK_SOCKET is not set (programs started in InfinityDesk have it)");
    };
    let talk = || -> std::io::Result<i32> {
        let mut stream = UnixStream::connect(&path)?;
        stream.write_all(request.as_bytes())?;
        stream.shutdown(std::net::Shutdown::Write)?;
        let mut reply = BufReader::new(stream);
        let mut status = String::new();
        reply.read_line(&mut status)?;
        // The rest as it comes (stdout writes each line right away). When the reader stops
        // listening, e.g. `| head -1`, writing the next line fails and ends it.
        let (output, code): (&mut dyn Write, i32) = match status.as_str() {
            "ok\n" => (&mut std::io::stdout(), 0),
            "error\n" => (&mut std::io::stderr(), 1),
            _ => return Ok(fail("InfinityDesk gave no answer")),
        };
        let _ = std::io::copy(&mut reply, output);
        Ok(code)
    };
    match talk() {
        Ok(code) => code,
        Err(err) => fail(&format!("could not reach InfinityDesk at {path:?}: {err}")),
    }
}

fn fail(text: &str) -> i32 {
    eprintln!("{text}");
    1
}
