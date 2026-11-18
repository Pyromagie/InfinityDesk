//! User settings from `~/.config/infinitydesk/init.lua` (Lua 5.4):
//!
//! ```lua
//! local desk = require("infinitydesk")
//! desk.input { keyboard = { xkb = { layout = "de" }, repeat_delay = 200, repeat_rate = 25 },
//!              touchpad = { tap = true, natural_scroll = true, dwt = true,
//!                           accel_speed = 0.0, accel_profile = "adaptive" } }
//! desk.canvas { width = 20000, height = 12000, grid = { spacing = 100 } }
//! desk.snapping { threshold = 15, match_size = false, hold_ms = 300 }
//! desk.colors { background = "#1a1a1f", grid = "#474752", accent = "#59a6ff" }
//! desk.bind("Mod+B", desk.spawn("firefox"))
//! desk.unbind("Mod+Space")
//! ```
//!
//! Everything is optional. Without the file the defaults apply; if the file has an error, it is
//! logged and the defaults apply as well. Changes to the file are picked up while running; a
//! changed file with an error keeps the previous settings. `INFINITYDESK_CONFIG` points to
//! another file.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime},
};

use mlua::{AnyUserData, FromLua, Lua, Table, UserData, Value};

use crate::navigate::Dir;
use smithay::input::keyboard::{
    Keysym, ModifiersState, XkbConfig,
    xkb::{self, KEYSYM_CASE_INSENSITIVE},
};

pub struct Config {
    pub keyboard: Keyboard,
    pub touchpad: Touchpad,
    /// Keyboard shortcuts: the built-in ones, changed by `desk.bind` and `desk.unbind`.
    pub binds: Vec<Bind>,
    /// Canvas size in logical pixels.
    pub canvas_size: (i32, i32),
    /// Distance between the grid dots on the canvas.
    pub grid_spacing: f64,
    /// Edges closer than this (in screen pixels) snap together.
    pub snap_threshold: f64,
    /// A docked window takes its neighbor's height (side by side) or width (stacked), so the
    /// cluster becomes a clean grid. Off: windows keep their size and only line up their edges.
    pub match_size: bool,
    /// How long a window is held still before it can be pulled out of its cluster.
    pub hold_time: Duration,
    /// Colors as premultiplied RGBA.
    pub background: [f32; 4],
    pub grid_color: [f32; 4],
    /// Frames, lit edges, the chosen slot in the template menu.
    pub accent: [f32; 4],
}

/// Keyboard layout and key repeat.
pub struct Keyboard {
    pub xkb: Xkb,
    /// Milliseconds before a held key repeats.
    pub repeat_delay: i32,
    /// Repeats per second (0: no repeat).
    pub repeat_rate: i32,
}

/// Touchpad settings. Only used standalone; nested, the host compositor sets up the touchpad.
#[derive(Clone, PartialEq)]
pub struct Touchpad {
    /// Tapping clicks.
    pub tap: bool,
    /// Content follows the fingers when scrolling.
    pub natural_scroll: bool,
    /// Disable while typing.
    pub dwt: bool,
    /// Pointer speed, -1 (slow) to 1 (fast).
    pub accel_speed: f64,
    /// Flat: the pointer moves as far as the fingers, however fast they are.
    pub accel_flat: bool,
}

/// Keyboard layout as xkb names; empty means the system default.
#[derive(Clone, Default, PartialEq)]
pub struct Xkb {
    pub rules: String,
    pub model: String,
    pub layout: String,
    pub variant: String,
    pub options: Option<String>,
}

/// Where systemd-localed keeps the system keyboard layout (`localectl set-x11-keymap`).
const SYSTEM_KEYBOARD: &str = "/etc/X11/xorg.conf.d/00-keyboard.conf";

impl Xkb {
    /// The layout to use: what the config sets, the rest from the system layout. Without
    /// `XKB_DEFAULT_LAYOUT` and a system layout, xkbcommon falls back to US.
    pub fn with_system_default(&self) -> Xkb {
        let system = match std::env::var_os("XKB_DEFAULT_LAYOUT") {
            None => std::fs::read_to_string(SYSTEM_KEYBOARD).unwrap_or_default(),
            Some(_) => String::new(),
        };
        self.or(&Xkb::from_x11_conf(&system))
    }

    /// Fills what is not set from `other`. Layout and variant belong together.
    fn or(&self, other: &Xkb) -> Xkb {
        let pick = |a: &String, b: &String| if a.is_empty() { b.clone() } else { a.clone() };
        let (layout, variant) = match self.layout.is_empty() {
            true => (other.layout.clone(), other.variant.clone()),
            false => (self.layout.clone(), self.variant.clone()),
        };
        Xkb {
            rules: pick(&self.rules, &other.rules),
            model: pick(&self.model, &other.model),
            layout,
            variant,
            options: self.options.clone().or(other.options.clone()),
        }
    }

    /// Reads lines like `Option "XkbLayout" "at"` from an X11 keyboard config.
    fn from_x11_conf(text: &str) -> Xkb {
        let mut xkb = Xkb::default();
        for line in text.lines() {
            let parts: Vec<&str> = line.split('"').collect();
            if !line.trim_start().starts_with("Option") || parts.len() < 4 || parts[3].is_empty() {
                continue;
            }
            let value = parts[3].to_string();
            match parts[1] {
                "XkbRules" => xkb.rules = value,
                "XkbModel" => xkb.model = value,
                "XkbLayout" => xkb.layout = value,
                "XkbVariant" => xkb.variant = value,
                "XkbOptions" => xkb.options = Some(value),
                _ => {}
            }
        }
        xkb
    }

    pub fn to_smithay(&self) -> XkbConfig<'_> {
        XkbConfig {
            rules: &self.rules,
            model: &self.model,
            layout: &self.layout,
            variant: &self.variant,
            options: self.options.clone(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            keyboard: Keyboard {
                xkb: Xkb::default(),
                repeat_delay: 200,
                repeat_rate: 25,
            },
            touchpad: Touchpad {
                tap: true,
                natural_scroll: true,
                dwt: true,
                accel_speed: 0.0,
                accel_flat: false,
            },
            canvas_size: (20_000, 12_000),
            grid_spacing: 100.0,
            snap_threshold: 15.0,
            match_size: false,
            hold_time: Duration::from_millis(300),
            background: [0.1, 0.1, 0.12, 1.0],
            grid_color: [0.28, 0.28, 0.32, 1.0],
            accent: [0.35, 0.65, 1.0, 1.0],
            binds: [
                ("Mod+T".to_string(), Action::Spawn("alacritty".into())),
                ("Mod+Return".to_string(), Action::Spawn("alacritty".into())),
                ("Mod+Space".to_string(), Action::Spawn("fuzzel".into())),
                ("Mod+Q".to_string(), Action::CloseWindow),
                ("Mod+F".to_string(), Action::Fullscreen),
                ("Mod+R".to_string(), Action::LayoutMenu),
                ("Mod+C".to_string(), Action::Center),
                // Mod+O too: while nested, Alt+Tab usually belongs to the host compositor.
                ("Mod+Tab".to_string(), Action::Overview),
                ("Mod+O".to_string(), Action::Overview),
                // Alt, not Mod: like everywhere else. (While nested, Mod+Tab is the same keys
                // and comes first; standalone, Mod is Super.)
                ("Alt+Tab".to_string(), Action::RecentWindows),
                ("Mod+Shift+E".to_string(), Action::Quit),
                ("Mod+Minus".to_string(), Action::ZoomOut),
                // "+" is its own key on some layouts (e.g. German), shares "=" on others.
                ("Mod+Plus".to_string(), Action::ZoomIn),
                ("Mod+Equal".to_string(), Action::ZoomIn),
            ]
            .into_iter()
            .chain(Dir::ALL.into_iter().flat_map(|dir| {
                let key = dir.key();
                [
                    (format!("Mod+{key}"), Action::Focus(dir)),
                    (format!("Mod+Ctrl+{key}"), Action::Swap(dir)),
                    (format!("Mod+Shift+{key}"), Action::Resize(dir)),
                    (format!("Mod+Ctrl+Shift+{key}"), Action::Pan(dir)),
                ]
            }))
            .map(|(key, action)| Bind {
                keys: parse_keys(&key).unwrap(),
                action,
            })
            .collect(),
        }
    }
}

/// A keyboard shortcut.
pub struct Bind {
    pub keys: Keys,
    pub action: Action,
}

/// A key with modifiers, e.g. "Mod+Shift+Return".
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Keys {
    pub logo: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
    /// The key as without Shift (lowercase letters).
    pub key: Keysym,
}

impl Keys {
    /// Does a key press (its unshifted symbol and the modifiers held) match?
    pub fn matches(&self, key: Keysym, mods: &ModifiersState) -> bool {
        self.key == key
            && (self.logo, self.alt, self.ctrl, self.shift)
                == (mods.logo, mods.alt, mods.ctrl, mods.shift)
    }
}

/// What a shortcut does.
#[derive(Clone, PartialEq, Debug)]
pub enum Action {
    /// Runs a command line (through `sh -c`, so arguments work).
    Spawn(String),
    CloseWindow,
    Fullscreen,
    LayoutMenu,
    /// Focuses the next window in a direction.
    Focus(Dir),
    /// Swaps the active window with its neighbor in the cluster.
    Swap(Dir),
    /// Moves the right (bottom) edge of the active window towards the arrow.
    Resize(Dir),
    /// Moves the view.
    Pan(Dir),
    ZoomIn,
    ZoomOut,
    /// Centers the view on the active cluster.
    Center,
    /// Zooms out until all windows fit on the screen, or back.
    Overview,
    /// Goes back through the windows in the order they were used, while the modifiers are held.
    RecentWindows,
    /// Ends InfinityDesk (and with it the session).
    Quit,
}

impl UserData for Action {}

/// The actions that need no argument, by their Lua name.
const SIMPLE_ACTIONS: [(&str, Action); 9] = [
    ("close_window", Action::CloseWindow),
    ("fullscreen", Action::Fullscreen),
    ("layout_menu", Action::LayoutMenu),
    ("zoom_in", Action::ZoomIn),
    ("zoom_out", Action::ZoomOut),
    ("center", Action::Center),
    ("overview", Action::Overview),
    ("recent_windows", Action::RecentWindows),
    ("quit", Action::Quit),
];
/// The actions that take a direction ("left", "right", "up", "down").
type DirAction = fn(Dir) -> Action;
const DIR_ACTIONS: [(&str, DirAction); 4] = [
    ("focus", Action::Focus),
    ("swap", Action::Swap),
    ("resize", Action::Resize),
    ("pan", Action::Pan),
];

/// Is Mod the Super key (standalone)? Otherwise it is Alt (nested: Super belongs to the host).
static MOD_IS_SUPER: AtomicBool = AtomicBool::new(false);

/// Chooses what Mod means; set once at startup, before the config is read.
pub fn set_mod_super(on: bool) {
    MOD_IS_SUPER.store(on, Ordering::Relaxed);
}

/// Is Mod held (for the mouse controls, e.g. Mod + drag)?
pub fn mod_held(mods: &ModifiersState) -> bool {
    match MOD_IS_SUPER.load(Ordering::Relaxed) {
        true => mods.logo,
        false => mods.alt,
    }
}

/// "Mod+Shift+Return" -> modifiers and key. Mod is Super standalone and Alt while running nested
/// (Super is taken by the host compositor); key names are xkb names (any case): "T", "Return",
/// "F11", "space", ...
fn parse_keys(text: &str) -> Result<Keys, String> {
    let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
    let name = parts.pop().filter(|n| !n.is_empty());
    let name = name.ok_or_else(|| format!("\"{text}\": no key after the modifiers"))?;
    let key = xkb::keysym_from_name(name, KEYSYM_CASE_INSENSITIVE);
    if key == Keysym::NoSymbol {
        return Err(format!("\"{text}\": unknown key \"{name}\""));
    }
    let mut keys = Keys {
        logo: false,
        alt: false,
        ctrl: false,
        shift: false,
        key,
    };
    for m in parts {
        let flag = match m.to_ascii_lowercase().as_str() {
            "mod" if MOD_IS_SUPER.load(Ordering::Relaxed) => &mut keys.logo,
            "mod" | "alt" => &mut keys.alt,
            "super" => &mut keys.logo,
            "ctrl" | "control" => &mut keys.ctrl,
            "shift" => &mut keys.shift,
            _ => {
                let known = "Mod, Super, Alt, Ctrl, Shift";
                return Err(format!(
                    "\"{text}\": unknown modifier \"{m}\" (known: {known})"
                ));
            }
        };
        *flag = true;
    }
    Ok(keys)
}

impl Config {
    /// Reads the config file at startup (see the module docs).
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        Self::read(&path).unwrap_or_else(|err| {
            tracing::error!("Error in the config, using the defaults:\n{err}");
            Self::default()
        })
    }

    /// Reads the config file; without the file, the defaults.
    pub fn read(path: &Path) -> Result<Self, String> {
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!("No config at {}, using the defaults", path.display());
                return Ok(Self::default());
            }
            Err(err) => return Err(format!("cannot read {}: {err}", path.display())),
        };
        let config = Self::from_lua(&source, &path.display().to_string());
        let config = config.map_err(|err| err.to_string())?;
        tracing::info!("Loaded the config from {}", path.display());
        Ok(config)
    }

    /// `$INFINITYDESK_CONFIG`, else `$XDG_CONFIG_HOME/infinitydesk/init.lua`, else
    /// `~/.config/infinitydesk/init.lua`.
    pub fn path() -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("INFINITYDESK_CONFIG") {
            return Some(path.into());
        }
        let config_dir = (std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
        Some(config_dir.join("infinitydesk").join("init.lua"))
    }

    /// When the config file was last changed (`None`: there is none), to notice changes.
    pub fn stamp() -> Option<SystemTime> {
        Self::path()?.metadata().ok()?.modified().ok()
    }

    /// Runs a config script; `name` shows up in error messages.
    fn from_lua(source: &str, name: &str) -> mlua::Result<Self> {
        let lua = Lua::new();
        lua.set_app_data(Config::default());
        let desk = lua.create_table()?;
        let sections: [(&str, Section); 4] = [
            ("input", input),
            ("canvas", canvas),
            ("snapping", snapping),
            ("colors", colors),
        ];
        for (name, section) in sections {
            let f = lua.create_function(move |lua, t: Table| {
                let mut config = lua.app_data_mut::<Config>().unwrap();
                section(&mut config, &t)
            })?;
            desk.set(name, f)?;
        }
        // Shortcuts.
        for (name, action) in SIMPLE_ACTIONS {
            desk.set(name, lua.create_userdata(action)?)?;
        }
        for (name, action) in DIR_ACTIONS {
            let f = lua.create_function(move |lua, text: String| {
                let dir = Dir::parse(&text).ok_or_else(|| {
                    let known = "\"left\", \"right\", \"up\" or \"down\"";
                    mlua::Error::runtime(format!("desk.{name}(\"{text}\"): expected {known}"))
                })?;
                lua.create_userdata(action(dir))
            })?;
            desk.set(name, f)?;
        }
        let spawn = lua
            .create_function(|lua, command: String| lua.create_userdata(Action::Spawn(command)))?;
        desk.set("spawn", spawn)?;
        let bind = lua.create_function(|lua, (text, action): (String, Value)| {
            let keys = parse_keys(&text).map_err(|e| mlua::Error::runtime(format!("bind {e}")))?;
            let action = (action.as_userdata())
                .and_then(|a: &AnyUserData| a.borrow::<Action>().ok().map(|a| a.clone()));
            let action = action.ok_or_else(|| {
                let expected = "an action like desk.spawn(\"...\") or desk.close_window";
                mlua::Error::runtime(format!("bind \"{text}\": the action must be {expected}"))
            })?;
            let mut config = lua.app_data_mut::<Config>().unwrap();
            config.binds.retain(|b| b.keys != keys);
            config.binds.push(Bind { keys, action });
            Ok(())
        })?;
        desk.set("bind", bind)?;
        let unbind = lua.create_function(|lua, text: String| {
            let keys =
                parse_keys(&text).map_err(|e| mlua::Error::runtime(format!("unbind {e}")))?;
            let mut config = lua.app_data_mut::<Config>().unwrap();
            config.binds.retain(|b| b.keys != keys);
            Ok(())
        })?;
        desk.set("unbind", unbind)?;
        let loaded: Table = lua.globals().get::<Table>("package")?.get("loaded")?;
        loaded.set("infinitydesk", desk)?;
        lua.load(source).set_name(format!("@{name}")).exec()?;
        Ok(lua.remove_app_data::<Config>().unwrap())
    }
}

/// Reads one `desk.<section> { ... }` call into the config.
type Section = fn(&mut Config, &Table) -> mlua::Result<()>;

fn input(config: &mut Config, t: &Table) -> mlua::Result<()> {
    check_keys(t, "input", &["keyboard", "touchpad"])?;
    if let Some(tp) = get::<Table>(t, "input", "touchpad")? {
        touchpad(&mut config.touchpad, &tp)?;
    }
    let Some(kb) = get::<Table>(t, "input", "keyboard")? else {
        return Ok(());
    };
    check_keys(
        &kb,
        "input.keyboard",
        &["xkb", "repeat_delay", "repeat_rate"],
    )?;
    let k = &mut config.keyboard;
    if let Some(xkb) = get::<Table>(&kb, "input.keyboard", "xkb")? {
        let x = &mut k.xkb;
        let section = "input.keyboard.xkb";
        check_keys(
            &xkb,
            section,
            &["rules", "model", "layout", "variant", "options"],
        )?;
        for (key, field) in [
            ("rules", &mut x.rules),
            ("model", &mut x.model),
            ("layout", &mut x.layout),
            ("variant", &mut x.variant),
        ] {
            if let Some(v) = get(&xkb, section, key)? {
                *field = v;
            }
        }
        x.options = get(&xkb, section, "options")?.or(x.options.take());
    }
    if let Some(v) = get_in(&kb, "input.keyboard", "repeat_delay", 1..=5000)? {
        k.repeat_delay = v;
    }
    if let Some(v) = get_in(&kb, "input.keyboard", "repeat_rate", 0..=1000)? {
        k.repeat_rate = v;
    }
    Ok(())
}

fn touchpad(tp: &mut Touchpad, t: &Table) -> mlua::Result<()> {
    let section = "input.touchpad";
    let known = [
        "tap",
        "natural_scroll",
        "dwt",
        "accel_speed",
        "accel_profile",
    ];
    check_keys(t, section, &known)?;
    for (key, field) in [
        ("tap", &mut tp.tap),
        ("natural_scroll", &mut tp.natural_scroll),
        ("dwt", &mut tp.dwt),
    ] {
        *field = get(t, section, key)?.unwrap_or(*field);
    }
    if let Some(v) = get::<f64>(t, section, "accel_speed")? {
        if !(-1.0..=1.0).contains(&v) {
            return Err(setting_error(
                section,
                "accel_speed",
                "must be between -1 and 1",
            ));
        }
        tp.accel_speed = v;
    }
    match get::<String>(t, section, "accel_profile")?.as_deref() {
        None => {}
        Some("adaptive") => tp.accel_flat = false,
        Some("flat") => tp.accel_flat = true,
        Some(_) => {
            let message = r#"expected "adaptive" or "flat""#;
            return Err(setting_error(section, "accel_profile", message));
        }
    }
    Ok(())
}

fn canvas(config: &mut Config, t: &Table) -> mlua::Result<()> {
    check_keys(t, "canvas", &["width", "height", "grid"])?;
    let (w, h) = &mut config.canvas_size;
    *w = get_in(t, "canvas", "width", 1_000..=100_000)?.unwrap_or(*w);
    *h = get_in(t, "canvas", "height", 1_000..=100_000)?.unwrap_or(*h);
    if let Some(grid) = get::<Table>(t, "canvas", "grid")? {
        check_keys(&grid, "canvas.grid", &["spacing"])?;
        if let Some(v) = get_in(&grid, "canvas.grid", "spacing", 10..=10_000)? {
            config.grid_spacing = v as f64;
        }
    }
    Ok(())
}

fn snapping(config: &mut Config, t: &Table) -> mlua::Result<()> {
    check_keys(t, "snapping", &["threshold", "match_size", "hold_ms"])?;
    if let Some(v) = get_in(t, "snapping", "threshold", 0..=100)? {
        config.snap_threshold = v as f64;
    }
    config.match_size = get(t, "snapping", "match_size")?.unwrap_or(config.match_size);
    if let Some(v) = get_in(t, "snapping", "hold_ms", 0..=5000)? {
        config.hold_time = Duration::from_millis(v as u64);
    }
    Ok(())
}

fn colors(config: &mut Config, t: &Table) -> mlua::Result<()> {
    check_keys(t, "colors", &["background", "grid", "accent"])?;
    for (key, field) in [
        ("background", &mut config.background),
        ("grid", &mut config.grid_color),
        ("accent", &mut config.accent),
    ] {
        if let Some(text) = get::<String>(t, "colors", key)? {
            *field = parse_color(&text).ok_or_else(|| {
                setting_error("colors", key, "expected \"#rrggbb\" or \"#rrggbbaa\"")
            })?;
        }
    }
    Ok(())
}

/// "#rrggbb" or "#rrggbbaa" as premultiplied RGBA.
fn parse_color(text: &str) -> Option<[f32; 4]> {
    let hex = (text.strip_prefix('#'))
        .filter(|h| matches!(h.len(), 6 | 8) && h.chars().all(|c| c.is_ascii_hexdigit()))?;
    let channel = |i: usize| Some(u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()? as f32 / 255.0);
    let a = if hex.len() == 8 { channel(6)? } else { 1.0 };
    Some([channel(0)? * a, channel(2)? * a, channel(4)? * a, a])
}

/// Unknown keys (typos) are errors, so a setting is never silently ignored.
fn check_keys(t: &Table, section: &str, known: &[&str]) -> mlua::Result<()> {
    for pair in t.pairs::<Value, Value>() {
        let (key, _) = pair?;
        let name = key.to_string()?;
        if !matches!(key, Value::String(_)) || !known.contains(&name.as_str()) {
            let message = format!("unknown setting (known: {})", known.join(", "));
            return Err(setting_error(section, &name, &message));
        }
    }
    Ok(())
}

/// A setting of the given type, or `None` if it is not set.
fn get<T: FromLua>(t: &Table, section: &str, key: &str) -> mlua::Result<Option<T>> {
    t.get::<Option<T>>(key)
        .map_err(|err| setting_error(section, key, &err.to_string()))
}

/// A whole number setting within `range`.
fn get_in(
    t: &Table,
    section: &str,
    key: &str,
    range: std::ops::RangeInclusive<i32>,
) -> mlua::Result<Option<i32>> {
    let value = get::<i32>(t, section, key)?;
    match value {
        Some(v) if !range.contains(&v) => {
            let message = format!("must be between {} and {}", range.start(), range.end());
            Err(setting_error(section, key, &message))
        }
        _ => Ok(value),
    }
}

fn setting_error(section: &str, key: &str, message: &str) -> mlua::Error {
    mlua::Error::runtime(format!("{section}.{key}: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(source: &str) -> mlua::Result<Config> {
        Config::from_lua(source, "init.lua")
    }

    fn error(source: &str) -> String {
        load(source).err().expect("expected an error").to_string()
    }

    #[test]
    fn all_settings_are_read() {
        let config = load(
            r##"
            local desk = require("infinitydesk")
            desk.input { keyboard = { xkb = { layout = "de", options = "caps:escape" },
                                      repeat_delay = 300, repeat_rate = 40 } }
            desk.input { touchpad = { tap = false, accel_speed = -0.5, accel_profile = "flat" } }
            desk.canvas { width = 30000, height = 15000, grid = { spacing = 200 } }
            desk.snapping { threshold = 20, match_size = true, hold_ms = 500 }
            desk.colors { background = "#000000", accent = "#ff8000" }
            "##,
        )
        .unwrap();
        assert_eq!(config.keyboard.xkb.layout, "de");
        assert_eq!(config.keyboard.xkb.options.as_deref(), Some("caps:escape"));
        assert_eq!(config.keyboard.xkb.variant, "");
        assert_eq!(
            (config.keyboard.repeat_delay, config.keyboard.repeat_rate),
            (300, 40)
        );
        let tp = &config.touchpad;
        assert_eq!((tp.tap, tp.accel_speed, tp.accel_flat), (false, -0.5, true));
        // Not set in the table: default.
        assert!(tp.natural_scroll && tp.dwt);
        assert_eq!(config.canvas_size, (30_000, 15_000));
        assert_eq!(config.grid_spacing, 200.0);
        assert_eq!(config.snap_threshold, 20.0);
        assert!(config.match_size);
        assert_eq!(config.hold_time, Duration::from_millis(500));
        assert_eq!(config.background, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(config.accent, [1.0, 128.0 / 255.0, 0.0, 1.0]);
        // Not set: default.
        assert_eq!(config.grid_color, Config::default().grid_color);
    }

    #[test]
    fn system_keyboard_layout_fills_what_the_config_leaves_open() {
        let system = Xkb::from_x11_conf(
            r#"Section "InputClass"
                    Identifier "system-keyboard"
                    Option "XkbLayout" "at"
                    Option "XkbModel" "pc105"
                    Option "XkbVariant" "nodeadkeys"
                    Option "XkbOptions" "caps:escape"
                EndSection"#,
        );
        // Nothing set: all from the system.
        let xkb = Xkb::default().or(&system);
        assert_eq!(
            (xkb.layout.as_str(), xkb.variant.as_str()),
            ("at", "nodeadkeys")
        );
        assert_eq!(xkb.model, "pc105");
        assert_eq!(xkb.options.as_deref(), Some("caps:escape"));
        // An own layout does not get the system's variant.
        let own = Xkb {
            layout: "us".into(),
            ..Default::default()
        };
        let xkb = own.or(&system);
        assert_eq!((xkb.layout.as_str(), xkb.variant.as_str()), ("us", ""));
        assert_eq!(xkb.model, "pc105");
    }

    #[test]
    fn empty_config_gives_the_defaults() {
        let config = load("").unwrap();
        assert_eq!(config.canvas_size, Config::default().canvas_size);
        assert_eq!(config.keyboard.repeat_rate, 25);
    }

    #[test]
    fn mistakes_name_the_setting() {
        let err = error(r#"require("infinitydesk").canvas { widht = 30000 }"#);
        assert!(
            err.contains("canvas.widht: unknown setting (known: width, height, grid)"),
            "{err}"
        );
        let err = error(r#"require("infinitydesk").snapping { threshold = "big" }"#);
        assert!(err.contains("snapping.threshold:"), "{err}");
        let err = error(r#"require("infinitydesk").canvas { grid = { spacing = 0 } }"#);
        assert!(
            err.contains("canvas.grid.spacing: must be between 10 and 10000"),
            "{err}"
        );
        let err = error(r#"require("infinitydesk").colors { accent = "blue" }"#);
        assert!(err.contains("colors.accent: expected"), "{err}");
        let err =
            error(r#"require("infinitydesk").input { touchpad = { accel_profile = "fast" } }"#);
        assert!(
            err.contains(r#"input.touchpad.accel_profile: expected "adaptive""#),
            "{err}"
        );
        let err = error(r#"require("infinitydesk").input { touchpad = { accel_speed = 2 } }"#);
        assert!(
            err.contains("input.touchpad.accel_speed: must be between -1 and 1"),
            "{err}"
        );
        // Lua errors carry the file name and line.
        let err = error("local x = \n nil + 1");
        assert!(err.contains("init.lua:2"), "{err}");
    }

    fn action(config: &Config, text: &str) -> Option<Action> {
        let keys = parse_keys(text).unwrap();
        let bind = config.binds.iter().find(|b| b.keys == keys)?;
        Some(bind.action.clone())
    }

    #[test]
    fn binds_add_replace_and_remove() {
        let config = load(
            r#"
            local desk = require("infinitydesk")
            desk.bind("Mod+Return", desk.spawn("alacritty -e btop"))
            desk.bind("mod+t", desk.spawn("kitty"))
            desk.bind("Ctrl+Alt+Shift+F11", desk.fullscreen)
            desk.unbind("Mod+Space")
            "#,
        )
        .unwrap();
        let spawn = |c: &str| Some(Action::Spawn(c.into()));
        assert_eq!(action(&config, "Mod+Return"), spawn("alacritty -e btop"));
        // Replaced, not added twice.
        assert_eq!(action(&config, "Mod+T"), spawn("kitty"));
        assert_eq!(
            config
                .binds
                .iter()
                .filter(|b| b.keys.key == Keysym::t)
                .count(),
            1
        );
        assert_eq!(
            action(&config, "Shift+Ctrl+Mod+f11"),
            Some(Action::Fullscreen)
        );
        assert_eq!(action(&config, "Mod+Space"), None);
        // Built-in ones stay.
        assert_eq!(action(&config, "Mod+Q"), Some(Action::CloseWindow));
    }

    #[test]
    fn keys_match_presses() {
        let keys = parse_keys("Mod+Shift+Q").unwrap();
        let mut mods = ModifiersState::default();
        (mods.alt, mods.shift) = (true, true);
        assert!(keys.matches(Keysym::q, &mods));
        mods.ctrl = true;
        assert!(!keys.matches(Keysym::q, &mods));
        assert_eq!(parse_keys("Super+1").unwrap().key, Keysym::_1);
    }

    #[test]
    fn bind_mistakes_are_explained() {
        let err =
            error(r#"local d = require("infinitydesk") d.bind("Mod+Retrun", d.close_window)"#);
        assert!(
            err.contains(r#"bind "Mod+Retrun": unknown key "Retrun""#),
            "{err}"
        );
        let err = error(r#"local d = require("infinitydesk") d.bind("Hyper+T", d.close_window)"#);
        assert!(err.contains(r#"unknown modifier "Hyper""#), "{err}");
        let err = error(r#"local d = require("infinitydesk") d.bind("Mod+T", "alacritty")"#);
        assert!(err.contains("the action must be an action like"), "{err}");
        let err = error(r#"local d = require("infinitydesk") d.bind("Mod+", d.fullscreen)"#);
        assert!(err.contains("no key after the modifiers"), "{err}");
        let err = error(r#"local d = require("infinitydesk") d.bind("Mod+H", d.focus("west"))"#);
        assert!(
            err.contains(r#"desk.focus("west"): expected "left""#),
            "{err}"
        );
    }

    #[test]
    fn navigation_binds() {
        let config = load(
            r#"
            local desk = require("infinitydesk")
            desk.bind("Mod+H", desk.focus("left"))
            desk.bind("Mod+Shift+L", desk.resize("Right"))
            desk.bind("Mod+Shift+O", desk.overview)
        "#,
        )
        .unwrap();
        assert_eq!(action(&config, "Mod+H"), Some(Action::Focus(Dir::Left)));
        assert_eq!(
            action(&config, "Mod+Shift+L"),
            Some(Action::Resize(Dir::Right))
        );
        assert_eq!(action(&config, "Mod+Shift+O"), Some(Action::Overview));
        // The built-in ones.
        assert_eq!(action(&config, "Mod+Left"), Some(Action::Focus(Dir::Left)));
        assert_eq!(
            action(&config, "Mod+Ctrl+Down"),
            Some(Action::Swap(Dir::Down))
        );
        assert_eq!(
            action(&config, "Mod+Ctrl+Shift+Up"),
            Some(Action::Pan(Dir::Up))
        );
        assert_eq!(action(&config, "Mod+Tab"), Some(Action::Overview));
        assert_eq!(action(&config, "Mod+O"), Some(Action::Overview));
        assert_eq!(action(&config, "Mod+R"), Some(Action::LayoutMenu));
        assert_eq!(
            action(&config, "Mod+Return"),
            Some(Action::Spawn("alacritty".into()))
        );
        assert_eq!(action(&config, "Mod+W"), None);
    }

    #[test]
    fn colors_with_alpha_are_premultiplied() {
        let a = 128.0 / 255.0;
        assert_eq!(parse_color("#ffffff80"), Some([a, a, a, a]));
        assert_eq!(parse_color("#ff0000"), Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(parse_color("ff0000"), None);
        assert_eq!(parse_color("#ff00"), None);
        assert_eq!(parse_color("#gg0000"), None);
        assert_eq!(parse_color("#+f0000"), None);
    }
}
