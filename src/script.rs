//! Lua scripting: functions from the config that react to events (`desk.on`) or sit on keys
//! (`desk.bind`), and carry out actions with `desk.run` (see `config.rs`).
//!
//! Every call into Lua has a time limit, so a script stuck in a loop cannot freeze the desktop.
//! Events are handed to scripts between two turns of the event loop, never in the middle of
//! changing something, and only if a script listens to them.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use mlua::{AnyUserData, Function, HookTriggers, Lua, MultiValue, Value, VmState};
use smithay::desktop::Window;

use crate::{InfinityDesk, config::Action, handlers::app_id_and_title};

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
/// Where, while InfinityDesk calls into Lua, the function behind `desk.run` is kept.
const ACT: &str = "infinitydesk.act";

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

/// A window as scripts see it, taken when the event happened.
struct WindowInfo {
    id: u64,
    app_id: String,
    title: String,
}

/// A window's number for scripts, the same as long as it is open.
struct WindowId(u64);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl WindowInfo {
    fn of(window: &Window) -> Self {
        let id = (window.user_data())
            .get_or_insert(|| WindowId(NEXT_ID.fetch_add(1, Ordering::Relaxed)))
            .0;
        let (app_id, title) = app_id_and_title(window.toplevel().unwrap());
        WindowInfo { id, app_id, title }
    }
}

impl Event {
    /// What the function gets: the windows as tables, then `on`.
    fn args(&self, lua: &Lua) -> mlua::Result<MultiValue> {
        let mut args = Vec::new();
        for w in &self.windows {
            let t = lua.create_table()?;
            t.set("id", w.id)?;
            t.set("app_id", w.app_id.as_str())?;
            t.set("title", w.title.as_str())?;
            args.push(Value::Table(t));
        }
        args.extend(self.on.map(Value::Boolean));
        Ok(MultiValue::from_vec(args))
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

/// `desk.run`: carries out an action right away.
pub fn run(lua: &Lua, action: Action) -> mlua::Result<()> {
    let act: Option<Function> = lua.named_registry_value(ACT)?;
    let act = act.ok_or_else(|| {
        mlua::Error::runtime("run works inside functions given to desk.on and desk.bind")
    })?;
    act.call(lua.create_userdata(action)?)
}

impl InfinityDesk {
    /// Something happened: scripts listening to `name` get it after this turn of the event
    /// loop (see `dispatch_events`), with `windows` and `on`.
    pub fn emit(&mut self, name: &'static str, windows: &[&Window], on: Option<bool>) {
        let lua = self.config.lua.as_ref();
        let handlers = lua.and_then(|lua| lua.app_data_ref::<Handlers>());
        if handlers.is_some_and(|h| h.listening(name).next().is_some()) {
            let windows = windows.iter().map(|w| WindowInfo::of(w)).collect();
            self.events.push(Event { name, windows, on });
        }
    }

    /// Between two turns of the event loop: hands the events to the functions listening.
    pub fn dispatch_events(&mut self) {
        for _ in 0..MAX_ROUNDS {
            if self.events.is_empty() {
                return;
            }
            for event in std::mem::take(&mut self.events) {
                let Some(lua) = self.config.lua.clone() else {
                    return;
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

    /// Calls a Lua function from the config, within the time limit. Meanwhile `desk.run`
    /// carries out actions. Errors are logged; `what` says where they came from.
    pub fn call_lua(&mut self, f: &Function, args: MultiValue, what: &str) {
        let Some(lua) = self.config.lua.clone() else {
            return;
        };
        let result = lua.scope(|scope| {
            let act = scope.create_function_mut(|_, action: AnyUserData| {
                let action = action.borrow::<Action>()?.clone();
                self.run(action);
                Ok(())
            })?;
            lua.set_named_registry_value(ACT, act)?;
            guarded(&lua, CALL_LIMIT, || f.call::<()>(args))
        });
        let _ = lua.unset_named_registry_value(ACT);
        if let Err(err) = result {
            tracing::warn!("Lua ({what}): {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
