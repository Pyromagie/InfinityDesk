//! Lua scripting: functions from the config that react to events (`desk.on`) or sit on keys
//! (`desk.bind`). They carry out actions with `desk.run` and look at and steer windows with the
//! functions in `REACHING` (see `config.rs`).
//!
//! Every call into Lua has a time limit, so a script stuck in a loop cannot freeze the desktop.
//! Events are handed to scripts between two turns of the event loop, never in the middle of
//! changing something, and only if a script listens to them. Programs outside get them as lines
//! of JSON through `infinitydesk msg events` (see `ipc.rs`).

use std::{
    cell::RefCell,
    fmt::Write as _,
    io::Write as _,
    os::unix::net::UnixStream,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use mlua::{AnyUserData, Function, HookTriggers, Lua, MultiValue, Table, Value, VmState};
use smithay::{desktop::Window, utils::Point};

use crate::{InfinityDesk, cluster::Rect, config::Action, handlers::app_id_and_title};

/// The events scripts can react to, and what their function gets.
pub const EVENTS: [&str; 10] = [
    "window.open",       // (window)
    "window.close",      // (window)
    "window.focus",      // (window), or nothing when no window is active any more
    "window.title",      // (window): its title or app_id changed
    "window.fullscreen", // (window, on)
    "cluster.dock",      // (window, window): the two docked to each other
    "cluster.undock",    // (window): pulled out of its cluster
    "view.overview",     // (on)
    "config.reloaded",   // ()
    "desk.start",        // (): once, after the programs to start were started
];

/// How long loading the config may take.
pub const LOAD_LIMIT: Duration = Duration::from_millis(1500);
/// How long a function on a key or an event may take.
const CALL_LIMIT: Duration = Duration::from_millis(100);
/// How many rounds of events causing further events are handled at once; beyond that, scripts
/// are probably setting each other off forever.
const MAX_ROUNDS: usize = 32;
/// The `desk` functions that reach into InfinityDesk, so they work only while it calls into Lua
/// (in functions given to `desk.on` and `desk.bind`):
/// - `run(action)`: carries out an action right away
/// - `windows()`: all windows, `focused()`: the active one (or nothing)
/// - `focus_window(id)`, `close(id)`: true if the window is there
/// - `move(id, x, y)`: moves the window's cluster so that the window's top-left corner is at x, y
pub const REACHING: [&str; 6] = ["run", "windows", "focused", "focus_window", "close", "move"];
/// Where, while InfinityDesk calls into Lua, the functions behind `REACHING` are kept.
const REACH: &str = "infinitydesk.reach";

/// The functions listening to events (`desk.on`), kept in the Lua state.
#[derive(Default)]
pub struct Handlers {
    next: u64,
    list: Vec<(u64, &'static str, Function)>,
}

impl Handlers {
    /// Adds a function for `event`; the number given back removes it again (`desk.off`).
    pub fn add(&mut self, event: &'static str, f: Function) -> u64 {
        self.next += 1;
        self.list.push((self.next, event, f));
        self.next
    }

    pub fn remove(&mut self, id: u64) {
        self.list.retain(|(i, ..)| *i != id);
    }

    pub fn listening(&self, event: &str) -> impl Iterator<Item = &Function> {
        (self.list.iter())
            .filter(move |(_, e, _)| *e == event)
            .map(|(.., f)| f)
    }
}

/// Something happened that scripts listen to.
pub struct Event {
    name: &'static str,
    windows: Vec<WindowInfo>,
    on: Option<bool>,
}

/// A window as scripts see it (a table `{ id, app_id, title, x, y, width, height, focused,
/// fullscreen }`), taken when the event happened.
struct WindowInfo {
    id: u64,
    app_id: String,
    title: String,
    /// Where it is, or is about to be while it changes size.
    rect: Rect,
    focused: bool,
    fullscreen: bool,
}

/// A window's number for scripts, the same as long as it is open.
struct WindowId(u64);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn window_id(window: &Window) -> u64 {
    let id = window
        .user_data()
        .get_or_insert(|| WindowId(NEXT_ID.fetch_add(1, Ordering::Relaxed)));
    id.0
}

impl WindowInfo {
    fn table(&self, lua: &Lua) -> mlua::Result<Table> {
        let t = lua.create_table()?;
        t.set("id", self.id)?;
        t.set("app_id", self.app_id.as_str())?;
        t.set("title", self.title.as_str())?;
        t.set("x", self.rect.loc.x)?;
        t.set("y", self.rect.loc.y)?;
        t.set("width", self.rect.size.w)?;
        t.set("height", self.rect.size.h)?;
        t.set("focused", self.focused)?;
        t.set("fullscreen", self.fullscreen)?;
        Ok(t)
    }

    /// The same as JSON, for `infinitydesk msg events`.
    fn json(&self, out: &mut String) {
        let _ = write!(out, "{{\"id\":{},\"app_id\":", self.id);
        json_string(&self.app_id, out);
        out.push_str(",\"title\":");
        json_string(&self.title, out);
        let (loc, size) = (self.rect.loc, self.rect.size);
        let _ = write!(
            out,
            ",\"x\":{},\"y\":{},\"width\":{},\"height\":{},\"focused\":{},\"fullscreen\":{}}}",
            loc.x, loc.y, size.w, size.h, self.focused, self.fullscreen
        );
    }
}

impl Event {
    /// What the function gets: the windows as tables, then `on`.
    fn args(&self, lua: &Lua) -> mlua::Result<MultiValue> {
        let mut args = Vec::new();
        for w in &self.windows {
            args.push(Value::Table(w.table(lua)?));
        }
        args.extend(self.on.map(Value::Boolean));
        Ok(MultiValue::from_vec(args))
    }

    /// One line for `infinitydesk msg events`: `{"event":…,"windows":[…],"on":…}` (`on` only
    /// when the event has it).
    fn json(&self) -> String {
        let mut out = format!("{{\"event\":\"{}\",\"windows\":[", self.name);
        for (i, w) in self.windows.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            w.json(&mut out);
        }
        out.push(']');
        if let Some(on) = self.on {
            let _ = write!(out, ",\"on\":{on}");
        }
        out.push_str("}\n");
        out
    }
}

/// Runs `f`, which calls into Lua; Lua code still running after `limit` is stopped with an error.
pub fn guarded<R>(
    lua: &Lua,
    limit: Duration,
    f: impl FnOnce() -> mlua::Result<R>,
) -> mlua::Result<R> {
    let deadline = Instant::now() + limit;
    let triggers = HookTriggers::new().every_nth_instruction(10_000);
    lua.set_hook(triggers, move |_, _| match Instant::now() < deadline {
        true => Ok(VmState::Continue),
        false => Err(mlua::Error::runtime(format!(
            "took longer than {} ms, stopped",
            limit.as_millis()
        ))),
    })?;
    let result = f();
    lua.remove_hook();
    result
}

/// The function behind `desk.<name>` (one of `REACHING`), while InfinityDesk calls into Lua.
pub fn reach(lua: &Lua, name: &str) -> mlua::Result<Function> {
    let functions: Option<Table> = lua.named_registry_value(REACH)?;
    let message = || format!("{name} works inside functions given to desk.on and desk.bind");
    functions
        .ok_or_else(|| mlua::Error::runtime(message()))?
        .get(name)
}

impl InfinityDesk {
    /// Something happened: scripts listening to `name` and the programs watching events get it
    /// after this turn of the event loop (see `dispatch_events`), with `windows` and `on`.
    pub fn emit(&mut self, name: &'static str, windows: &[&Window], on: Option<bool>) {
        let lua = self.config.lua.as_ref();
        let handlers = lua.and_then(|lua| lua.app_data_ref::<Handlers>());
        let listening = handlers.is_some_and(|h| h.listening(name).next().is_some());
        if listening || !self.event_streams.is_empty() {
            let windows = windows.iter().map(|w| self.window_info(w)).collect();
            self.events.push(Event { name, windows, on });
        }
    }

    fn window_info(&self, window: &Window) -> WindowInfo {
        let (app_id, title) = app_id_and_title(window.toplevel().unwrap());
        WindowInfo {
            id: window_id(window),
            app_id,
            title,
            rect: self.target_rect(window).unwrap_or_default(),
            focused: self.active.as_ref() == Some(window),
            fullscreen: self
                .fullscreen
                .as_ref()
                .is_some_and(|f| &f.window == window),
        }
    }

    fn window_by_id(&self, id: u64) -> Option<Window> {
        self.space.elements().find(|w| window_id(w) == id).cloned()
    }

    /// `desk.move`: the window's cluster moves (gliding) so that the window's top-left corner
    /// ends up at `loc`, staying inside the canvas.
    fn move_cluster(&mut self, window: &Window, loc: (i32, i32)) {
        let members = self.clusters.members(window);
        if (self.fullscreen.as_ref()).is_some_and(|f| members.contains(&f.window)) {
            self.end_fullscreen();
        }
        let (windows, rects) = self.cluster_rects(window);
        let bbox = rects.iter().copied().reduce(|a, b| a.merge(b));
        let (Some(rect), Some(bbox)) = (self.target_rect(window), bbox) else {
            return;
        };
        let to = bbox.loc + (Point::from(loc) - rect.loc);
        let shift = self.canvas.clamp_window_loc(to, bbox.size) - bbox.loc;
        self.shift_windows(windows.into_iter().zip(rects).collect(), shift);
    }

    /// `infinitydesk msg events`: `stream` gets the state first (a `state` event with all windows,
    /// then whether the overview is open, as `view.overview`), then every event as it happens.
    pub fn add_event_stream(&mut self, stream: UnixStream) {
        let windows = self.space.elements().map(|w| self.window_info(w)).collect();
        let state = Event {
            name: "state",
            windows,
            on: None,
        };
        let overview = Event {
            name: "view.overview",
            windows: Vec::new(),
            on: Some(self.overview.is_some()),
        };
        let text = format!("ok\n{}{}", state.json(), overview.json());
        if (&stream).write_all(text.as_bytes()).is_ok() {
            self.event_streams.push(stream);
        }
    }

    /// Between two turns of the event loop: hands the events to the programs watching and to
    /// the functions listening. A program that does not keep up (or is gone) is dropped.
    pub fn dispatch_events(&mut self) {
        for _ in 0..MAX_ROUNDS {
            if self.events.is_empty() {
                return;
            }
            for event in std::mem::take(&mut self.events) {
                if !self.event_streams.is_empty() {
                    let line = event.json();
                    (self.event_streams).retain(|mut s| s.write_all(line.as_bytes()).is_ok());
                }
                let Some(lua) = self.config.lua.clone() else {
                    continue;
                };
                let handlers = lua.app_data_ref::<Handlers>();
                let functions: Vec<Function> =
                    handlers.map_or_default(|h| h.listening(event.name).cloned().collect());
                for f in functions {
                    match event.args(&lua) {
                        Ok(args) => self.call_lua(&f, args, event.name),
                        Err(err) => tracing::error!("Lua ({}): {err}", event.name),
                    }
                }
            }
        }
        let left = self.events.len();
        tracing::warn!("Scripts keep setting off events, dropping the last {left}");
        self.events.clear();
    }

    /// Calls a Lua function from the config, within the time limit. Meanwhile the functions in
    /// `REACHING` work. Errors are logged; `what` says where they came from.
    pub fn call_lua(&mut self, f: &Function, args: MultiValue, what: &str) {
        let Some(lua) = self.config.lua.clone() else {
            return;
        };
        if let Err(err) = self.reaching(&lua, || f.call::<()>(args)) {
            tracing::warn!("Lua ({what}): {err}");
        }
    }

    /// `infinitydesk msg eval`: runs `code` like a function given to `desk.bind` (with `desk`
    /// at hand); what it returns comes back as JSON, one line per value.
    pub fn eval_lua(&mut self, code: &str) -> Result<String, String> {
        let Some(lua) = self.config.lua.clone() else {
            return Err("there is no Lua state (the config could not be read)".into());
        };
        // `desk` is ready, as in the config; on the same line, so line numbers in errors fit.
        let code = format!("local desk = require(\"infinitydesk\") {code}");
        let chunk = lua.load(code).set_name("msg eval");
        let values =
            (self.reaching(&lua, || chunk.eval::<MultiValue>())).map_err(|e| e.to_string())?;
        let mut out = String::new();
        for value in &values {
            json(value, &mut out, 0).map_err(|e| e.to_string())?;
            out.push('\n');
        }
        Ok(out)
    }

    /// Runs `f` (which calls into Lua) within the time limit, with the functions in `REACHING`
    /// working meanwhile.
    fn reaching<R>(&mut self, lua: &Lua, f: impl FnOnce() -> mlua::Result<R>) -> mlua::Result<R> {
        let state = RefCell::new(self);
        let result = lua.scope(|scope| {
            let t = lua.create_table()?;
            let run = scope.create_function(|_, action: AnyUserData| {
                let action = action.borrow::<Action>()?.clone();
                state.borrow_mut().run(action);
                Ok(())
            })?;
            let windows = scope.create_function(|lua, ()| {
                let s = state.borrow();
                let tables = s.space.elements().map(|w| s.window_info(w).table(lua));
                lua.create_sequence_from(tables.collect::<mlua::Result<Vec<_>>>()?)
            })?;
            let focused = scope.create_function(|lua, ()| {
                let s = state.borrow();
                s.active
                    .as_ref()
                    .map(|w| s.window_info(w).table(lua))
                    .transpose()
            })?;
            let focus_window = scope.create_function(|_, id: u64| {
                let mut s = state.borrow_mut();
                let window = s.window_by_id(id);
                if let Some(window) = &window {
                    if (s.fullscreen.as_ref()).is_some_and(|f| &f.window != window) {
                        s.end_fullscreen();
                    }
                    s.focus_window(window);
                    if let Some(rect) = s.target_rect(window) {
                        s.reveal(rect);
                    }
                }
                Ok(window.is_some())
            })?;
            let close = scope.create_function(|_, id: u64| {
                let window = state.borrow().window_by_id(id);
                if let Some(window) = &window {
                    window.toplevel().unwrap().send_close();
                }
                Ok(window.is_some())
            })?;
            let move_ = scope.create_function(|_, (id, x, y): (u64, i32, i32)| {
                let mut s = state.borrow_mut();
                let window = s.window_by_id(id);
                if let Some(window) = &window {
                    s.move_cluster(window, (x, y));
                }
                Ok(window.is_some())
            })?;
            for (name, f) in
                REACHING
                    .into_iter()
                    .zip([run, windows, focused, focus_window, close, move_])
            {
                t.set(name, f)?;
            }
            lua.set_named_registry_value(REACH, t)?;
            guarded(lua, CALL_LIMIT, f)
        });
        let _ = lua.unset_named_registry_value(REACH);
        result
    }
}

/// Writes `value` as JSON. Tables with entries 1..n (and empty ones) become arrays, others
/// objects.
fn json(value: &Value, out: &mut String, depth: usize) -> mlua::Result<()> {
    match value {
        Value::Nil => out.push_str("null"),
        Value::Boolean(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Integer(i) => drop(write!(out, "{i}")),
        Value::Number(n) if n.is_finite() => drop(write!(out, "{n}")),
        Value::Number(_) => out.push_str("null"),
        Value::String(s) => json_string(&s.to_string_lossy(), out),
        Value::Table(t) if depth < 32 => {
            let n = t.raw_len();
            if n > 0 || t.is_empty() {
                out.push('[');
                for i in 1..=n {
                    if i > 1 {
                        out.push(',');
                    }
                    json(&t.raw_get(i)?, out, depth + 1)?;
                }
                out.push(']');
            } else {
                out.push('{');
                for (i, pair) in t.pairs::<Value, Value>().enumerate() {
                    let (key, value) = pair?;
                    if i > 0 {
                        out.push(',');
                    }
                    let key = match &key {
                        Value::String(s) => s.to_string_lossy(),
                        Value::Integer(_) | Value::Number(_) => key.to_string()?,
                        _ => {
                            return Err(mlua::Error::runtime(
                                "JSON keys must be strings or numbers",
                            ));
                        }
                    };
                    json_string(&key, out);
                    out.push(':');
                    json(&value, out, depth + 1)?;
                }
                out.push('}');
            }
        }
        Value::Table(_) => return Err(mlua::Error::runtime("tables nested too deep for JSON")),
        other => {
            let kind = other.type_name();
            return Err(mlua::Error::runtime(format!(
                "a {kind} cannot be turned into JSON"
            )));
        }
    }
    Ok(())
}

fn json_string(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => drop(write!(out, "\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_become_json() {
        let lua = Lua::new();
        let value: Value = (lua
            .load(r#"return { 1, 2.5, "a\"b\n", true, {}, { x = 3 }, { 1 / 0 } }"#))
        .eval()
        .unwrap();
        let mut out = String::new();
        json(&value, &mut out, 0).unwrap();
        assert_eq!(out, r#"[1,2.5,"a\"b\n",true,[],{"x":3},[null]]"#);
        let f: Value = lua.load("return print").eval().unwrap();
        let err = json(&f, &mut String::new(), 0).unwrap_err();
        assert!(
            err.to_string()
                .contains("a function cannot be turned into JSON")
        );
    }

    #[test]
    fn events_become_json_lines() {
        let window = WindowInfo {
            id: 3,
            app_id: "kitty".into(),
            title: "say \"hi\"".into(),
            rect: Rect::new((-10, 20).into(), (300, 400).into()),
            focused: true,
            fullscreen: false,
        };
        let event = Event {
            name: "window.fullscreen",
            windows: vec![window],
            on: Some(true),
        };
        let line = r#"{"event":"window.fullscreen","windows":[{"id":3,"app_id":"kitty","title":"say \"hi\"","x":-10,"y":20,"width":300,"height":400,"focused":true,"fullscreen":false}],"on":true}"#;
        assert_eq!(event.json(), format!("{line}\n"));
        let event = Event {
            name: "config.reloaded",
            windows: Vec::new(),
            on: None,
        };
        assert_eq!(
            event.json(),
            "{\"event\":\"config.reloaded\",\"windows\":[]}\n"
        );
    }

    #[test]
    fn endless_loops_are_stopped() {
        let lua = Lua::new();
        let limit = Duration::from_millis(20);
        let err = guarded(&lua, limit, || lua.load("while true do end").exec()).unwrap_err();
        assert!(err.to_string().contains("took longer than 20 ms"), "{err}");
        // Afterwards Lua runs normally again, without a limit.
        assert_eq!(lua.load("return 1 + 1").eval::<i32>().unwrap(), 2);
    }
}
