//! User settings. Later they come from `~/.config/infinitydesk/init.lua`; until then the defaults
//! apply, and a few can be switched with environment variables for testing.

pub struct Config {
    /// A docked window takes its neighbor's height (side by side) or width (stacked), so the
    /// cluster becomes a clean grid. Off: windows keep their size and only line up their edges.
    /// Env: `INFINITYDESK_MATCH_SIZE=1`.
    pub match_size: bool,
}

impl Config {
    pub fn load() -> Self {
        Self {
            match_size: std::env::var("INFINITYDESK_MATCH_SIZE").is_ok_and(|v| v == "1"),
        }
    }
}
