//! User settings from `~/.config/infinitydesk/init.lua` (Lua 5.4):
//!
//! ```lua
//! local desk = require("infinitydesk")
//! desk.input { keyboard = { xkb = { layout = "de" }, repeat_delay = 200, repeat_rate = 25 } }
//! desk.canvas { width = 20000, height = 12000, grid = { spacing = 100 } }
//! desk.snapping { threshold = 15, match_size = false, hold_ms = 300 }
//! desk.colors { background = "#1a1a1f", grid = "#474752", accent = "#59a6ff" }
//! ```
//!
//! Everything is optional. Without the file the defaults apply; if the file has an error, it is
//! logged and the defaults apply as well. Changes to the file are picked up while running; a
//! changed file with an error keeps the previous settings. `INFINITYDESK_CONFIG` points to
//! another file.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use mlua::{FromLua, Lua, Table, Value};
use smithay::input::keyboard::XkbConfig;

pub struct Config {
    pub keyboard: Keyboard,
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

/// Keyboard layout as xkb names; empty means the system default.
#[derive(Clone, Default, PartialEq)]
pub struct Xkb {
    pub rules: String,
    pub model: String,
    pub layout: String,
    pub variant: String,
    pub options: Option<String>,
}

impl Xkb {
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
            canvas_size: (20_000, 12_000),
            grid_spacing: 100.0,
            snap_threshold: 15.0,
            match_size: false,
            hold_time: Duration::from_millis(300),
            background: [0.1, 0.1, 0.12, 1.0],
            grid_color: [0.28, 0.28, 0.32, 1.0],
            accent: [0.35, 0.65, 1.0, 1.0],
        }
    }
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
        let loaded: Table = lua.globals().get::<Table>("package")?.get("loaded")?;
        loaded.set("infinitydesk", desk)?;
        lua.load(source).set_name(format!("@{name}")).exec()?;
        Ok(lua.remove_app_data::<Config>().unwrap())
    }
}

/// Reads one `desk.<section> { ... }` call into the config.
type Section = fn(&mut Config, &Table) -> mlua::Result<()>;

fn input(config: &mut Config, t: &Table) -> mlua::Result<()> {
    check_keys(t, "input", &["keyboard"])?;
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
        // Lua errors carry the file name and line.
        let err = error("local x = \n nil + 1");
        assert!(err.contains("init.lua:2"), "{err}");
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
