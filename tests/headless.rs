//! End-to-end tests without a screen: each test starts InfinityDesk headless (`--headless`, a
//! 1920×1080 output) in a folder of its own, opens test windows that behave like real programs,
//! and looks and steers through `infinitydesk msg`. No GPU, display or input devices needed.

use std::{
    fs::File,
    io::{BufRead, BufReader},
    os::{
        fd::AsFd,
        unix::{fs::PermissionsExt, net::UnixStream},
    },
    path::PathBuf,
    process::{Child, ChildStdout, Command, Stdio},
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, delegate_noop,
    protocol::{wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface},
};
use wayland_protocols::xdg::{
    decoration::zv1::client::{zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1},
    shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base},
};

const BIN: &str = env!("CARGO_BIN_EXE_infinitydesk");
/// Window sizes InfinityDesk gives on the 1920×1080 output with the default gaps of 16: half
/// the screen wide, as high as the screen.
const HALF: (i64, i64) = ((1920 - 3 * 16) / 2, 1080 - 2 * 16);

/// Lua the tests use in `eval`: `find(app_id)` gives that window.
const HELPERS: &str = r#"
local desk = require("infinitydesk")
function find(app_id)
  for _, w in ipairs(desk.windows()) do
    if w.app_id == app_id then return w end
  end
end
"#;

/// A running InfinityDesk without a screen; stopped (and its folder removed) when dropped.
struct Desk {
    child: Child,
    dir: PathBuf,
    wayland: PathBuf,
    socket: PathBuf,
}

impl Desk {
    /// Starts it with `config` (Lua, after `local desk = require("infinitydesk")`).
    fn start(config: &str) -> Desk {
        static RUNS: AtomicU32 = AtomicU32::new(0);
        // Short: socket paths have a length limit.
        let n = RUNS.fetch_add(1, Ordering::Relaxed);
        let dir =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Like a real XDG_RUNTIME_DIR: only for the user.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let config_path = dir.join("init.lua");
        std::fs::write(&config_path, format!("{HELPERS}\n{config}")).unwrap();
        let child = Command::new(BIN)
            .arg("--headless")
            .env("XDG_RUNTIME_DIR", &dir)
            .env("INFINITYDESK_CONFIG", &config_path)
            .env("RUST_LOG", "warn,infinitydesk=debug")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .env_remove("INFINITYDESK_SOCKET")
            .stdout(Stdio::null())
            .stderr(File::create(dir.join("log")).unwrap())
            .spawn()
            .unwrap();
        let mut desk = Desk {
            child,
            wayland: PathBuf::new(),
            socket: PathBuf::new(),
            dir,
        };
        // Ready once the command socket is there: infinitydesk.<wayland socket>.sock.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let names = std::fs::read_dir(&desk.dir).unwrap().flatten();
            let wayland = names
                .filter_map(|e| e.file_name().into_string().ok())
                .find_map(|n| {
                    Some(
                        n.strip_prefix("infinitydesk.")?
                            .strip_suffix(".sock")?
                            .to_string(),
                    )
                });
            if let Some(wayland) = wayland {
                desk.socket = desk.dir.join(format!("infinitydesk.{wayland}.sock"));
                desk.wayland = desk.dir.join(wayland);
                return desk;
            }
            assert!(
                Instant::now() < deadline && desk.child.try_wait().unwrap().is_none(),
                "InfinityDesk did not start:\n{}",
                desk.log()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("log")).unwrap_or_default()
    }

    /// `infinitydesk msg <args>`, with its exit status, output and errors.
    fn msg(&self, args: &[&str]) -> (bool, String, String) {
        let out = Command::new(BIN)
            .arg("msg")
            .args(args)
            .env("INFINITYDESK_SOCKET", &self.socket)
            .output()
            .unwrap();
        let text = |b: Vec<u8>| String::from_utf8(b).unwrap();
        (out.status.success(), text(out.stdout), text(out.stderr))
    }

    /// Runs Lua inside (`infinitydesk msg eval`); what it returns, one value per line (JSON).
    fn eval(&self, code: &str) -> Vec<String> {
        let (ok, out, err) = self.msg(&["eval", code]);
        assert!(ok, "eval {code}: {err}\n{}", self.log());
        out.lines().map(String::from).collect()
    }

    /// `eval` for code that returns numbers.
    fn numbers(&self, code: &str) -> Vec<i64> {
        let values = self.eval(code);
        values.iter().map(|v| v.parse().unwrap()).collect()
    }

    /// Where the window of `app_id` is: x, y, width, height.
    fn rect(&self, app_id: &str) -> Vec<i64> {
        self.numbers(&format!(
            "local w = find('{app_id}') return w.x, w.y, w.width, w.height"
        ))
    }

    /// Replaces the config file (InfinityDesk looks for changes once a second) and waits until
    /// the new one is read: it must set the global `version` to `version`.
    fn rewrite_config(&self, config: &str, version: i64) {
        let path = self.dir.join("init.lua");
        std::fs::write(&path, format!("{HELPERS}\n{config}\nversion = {version}")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.numbers("return version or 0") != [version] {
            assert!(
                Instant::now() < deadline,
                "the new config was not read:\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// `infinitydesk msg events`, read line by line.
    fn watch(&self) -> Events {
        let mut child = Command::new(BIN)
            .args(["msg", "events"])
            .env("INFINITYDESK_SOCKET", &self.socket)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout: ChildStdout = child.stdout.take().unwrap();
        let (send, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = send.send(line);
            }
        });
        Events { child, lines }
    }
}

impl Drop for Desk {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::thread::panicking() {
            eprintln!("InfinityDesk's log:\n{}", self.log());
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A running `infinitydesk msg events`.
struct Events {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl Events {
    /// The next line, waiting a while for it.
    fn next(&self) -> String {
        (self.lines.recv_timeout(Duration::from_secs(5))).expect("no event came")
    }
}

impl Drop for Events {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A program with windows: like kitty, it draws each window in the size InfinityDesk asks for,
/// and closes a window when asked to.
struct Client {
    queue: EventQueue<Windows>,
    windows: Windows,
}

#[derive(Default)]
struct Windows {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    decorations: Option<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
    /// Where the window contents (shared memory) are created.
    dir: PathBuf,
    list: Vec<TestWindow>,
}

struct TestWindow {
    surface: wl_surface::WlSurface,
    xdg: xdg_surface::XdgSurface,
    toplevel: xdg_toplevel::XdgToplevel,
    /// What InfinityDesk asked for last (0: up to the program).
    size: (i32, i32),
    /// Whether the program should draw its own titlebar, as InfinityDesk said last.
    titlebar: Option<bool>,
    /// InfinityDesk counts the window as tiled on all sides.
    tiled: bool,
    decoration: zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
    open: bool,
}

impl Client {
    fn connect(desk: &Desk) -> Client {
        let connection = Connection::from_socket(UnixStream::connect(&desk.wayland).unwrap());
        let connection = connection.unwrap();
        let mut queue = connection.new_event_queue();
        connection.display().get_registry(&queue.handle(), ());
        let mut windows = Windows {
            dir: desk.dir.clone(),
            ..Default::default()
        };
        queue.roundtrip(&mut windows).unwrap();
        assert!(windows.compositor.is_some() && windows.shm.is_some());
        assert!(windows.wm_base.is_some() && windows.decorations.is_some());
        Client { queue, windows }
    }

    /// Opens a window and waits until it is drawn and placed.
    fn open(&mut self, app_id: &str, title: &str) {
        self.open_with(app_id, title, None);
    }

    /// The same, as a dialog of window `parent`.
    fn open_with(&mut self, app_id: &str, title: &str, parent: Option<usize>) {
        let qh = self.queue.handle();
        let w = &mut self.windows;
        let index = w.list.len();
        let surface = w.compositor.as_ref().unwrap().create_surface(&qh, ());
        let xdg = (w.wm_base.as_ref().unwrap()).get_xdg_surface(&surface, &qh, index);
        let toplevel = xdg.get_toplevel(&qh, index);
        toplevel.set_app_id(app_id.into());
        toplevel.set_title(title.into());
        toplevel.set_parent(parent.map(|p| &w.list[p].toplevel));
        // Like kitty: it can do without a titlebar of its own.
        let decorations = w.decorations.as_ref().unwrap();
        let decoration = decorations.get_toplevel_decoration(&toplevel, &qh, index);
        surface.commit();
        w.list.push(TestWindow {
            surface,
            xdg,
            toplevel,
            size: (0, 0),
            titlebar: None,
            tiled: false,
            decoration,
            open: true,
        });
        self.sync();
    }

    /// Closes a window from the program's side, like quitting it.
    fn close(&mut self, index: usize) {
        self.windows.list[index].close();
        self.sync();
    }

    /// The program asks for fullscreen or to leave it (like F11 in a browser).
    fn fullscreen(&mut self, index: usize, on: bool) {
        let toplevel = &self.windows.list[index].toplevel;
        match on {
            true => toplevel.set_fullscreen(None),
            false => toplevel.unset_fullscreen(),
        }
        self.sync();
    }

    /// The program says whether it would rather draw its own titlebar.
    fn prefer_titlebar(&mut self, index: usize, own: bool) {
        let decoration = &self.windows.list[index].decoration;
        decoration.set_mode(match own {
            true => zxdg_toplevel_decoration_v1::Mode::ClientSide,
            false => zxdg_toplevel_decoration_v1::Mode::ServerSide,
        });
        self.sync();
    }

    fn is_open(&self, index: usize) -> bool {
        self.windows.list[index].open
    }

    /// Lets InfinityDesk and the program catch up with each other: what InfinityDesk asked
    /// for (sizes, closing) is done and InfinityDesk has seen it.
    fn sync(&mut self) {
        for _ in 0..3 {
            self.queue.roundtrip(&mut self.windows).unwrap();
        }
    }
}

impl TestWindow {
    fn close(&mut self) {
        if self.open {
            self.decoration.destroy();
            self.toplevel.destroy();
            self.xdg.destroy();
            self.surface.destroy();
            self.open = false;
        }
    }
}

impl Windows {
    /// Draws window `index` (plain black) in the size asked for.
    fn draw(&mut self, index: usize, qh: &QueueHandle<Self>) {
        let (w, h) = match self.list[index].size {
            (w, h) if w > 0 && h > 0 => (w, h),
            _ => (640, 480),
        };
        let path = self.dir.join(format!("shm-{index}-{w}x{h}"));
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        file.set_len((w * h * 4) as u64).unwrap();
        let _ = std::fs::remove_file(&path);
        let pool = (self.shm.as_ref().unwrap()).create_pool(file.as_fd(), w * h * 4, qh, ());
        let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Xrgb8888, qh, ());
        pool.destroy();
        let surface = &self.list[index].surface;
        surface.attach(Some(&buffer), 0, 0);
        surface.damage(0, 0, w, h);
        surface.commit();
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for Windows {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => state.compositor = Some(registry.bind(name, 1, qh, ())),
                "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                // Version 2 knows the tiled states.
                "xdg_wm_base" => state.wm_base = Some(registry.bind(name, 2, qh, ())),
                "zxdg_decoration_manager_v1" => {
                    state.decorations = Some(registry.bind(name, 1, qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Windows {
    fn event(
        _: &mut Self,
        wm_base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, usize> for Windows {
    fn event(
        state: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        index: &usize,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event
            && state.list[*index].open
        {
            xdg.ack_configure(serial);
            state.draw(*index, qh);
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, usize> for Windows {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                state.list[*index].size = (width, height);
                let tiled = xdg_toplevel::State::TiledLeft as u32;
                let mut states = states
                    .chunks(4)
                    .map(|c| u32::from_ne_bytes(c.try_into().unwrap()));
                state.list[*index].tiled = states.any(|s| s == tiled);
            }
            xdg_toplevel::Event::Close => state.list[*index].close(),
            _ => {}
        }
    }
}

impl Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, usize> for Windows {
    fn event(
        state: &mut Self,
        _: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Event::Configure { mode } = event {
            let own =
                mode == wayland_client::WEnum::Value(zxdg_toplevel_decoration_v1::Mode::ClientSide);
            state.list[*index].titlebar = Some(own);
        }
    }
}

delegate_noop!(Windows: ignore zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);
delegate_noop!(Windows: ignore wl_compositor::WlCompositor);
delegate_noop!(Windows: ignore wl_surface::WlSurface);
delegate_noop!(Windows: ignore wl_shm::WlShm);
delegate_noop!(Windows: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Windows: ignore wl_buffer::WlBuffer);

#[test]
fn two_new_windows_fill_the_screen_side_by_side() {
    let desk = Desk::start("");
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    client.open("two", "Two");
    let (one, two) = (desk.rect("one"), desk.rect("two"));
    assert_eq!((one[2], one[3]), HALF, "{one:?}");
    assert_eq!((two[2], two[3]), HALF, "{two:?}");
    // Docked to the right of the first one, a gap apart.
    assert_eq!((two[0], two[1]), (one[0] + HALF.0 + 16, one[1]));
    // The program drew them in the sizes it was asked for.
    assert_eq!(client.windows.list[1].size, (HALF.0 as i32, HALF.1 as i32));
    let focused = desk.eval("return desk.focused().app_id");
    assert_eq!(focused, [r#""two""#]);
}

#[test]
fn scripts_get_the_events_in_order() {
    let desk = Desk::start(
        r#"
        seen = {}
        for _, name in ipairs({ "window.open", "window.close", "window.focus", "window.title",
                                "cluster.dock" }) do
          desk.on(name, function(w) table.insert(seen, name .. ":" .. (w and w.app_id or "-")) end)
        end
        "#,
    );
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    client.open("two", "Two");
    client.close(1);
    let seen = desk.eval("return table.concat(seen, ' ')");
    let expected = "window.open:one window.focus:one window.open:two cluster.dock:two \
        window.focus:two window.close:two window.focus:-";
    assert_eq!(seen, [format!("\"{expected}\"")]);
}

#[test]
fn fullscreen_covers_the_screen_and_gives_the_place_back() {
    let desk = Desk::start("");
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    let before = desk.rect("one");
    desk.eval("desk.run(desk.fullscreen)");
    client.sync();
    assert_eq!(desk.rect("one")[2..], [1920, 1080]);
    assert_eq!(client.windows.list[0].size, (1920, 1080));
    let flag = desk.eval("return find('one').fullscreen");
    assert_eq!(flag, ["true"]);
    desk.eval("desk.run(desk.fullscreen)");
    client.sync();
    assert_eq!(desk.rect("one"), before);
    assert_eq!(client.windows.list[0].size, (HALF.0 as i32, HALF.1 as i32));
}

#[test]
fn moving_a_window_takes_its_cluster_along_and_stays_in_the_canvas() {
    let desk = Desk::start("");
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    client.open("two", "Two");
    let (one, two) = (desk.rect("one"), desk.rect("two"));
    let moved = desk.eval("local w = find('one') return desk.move(w.id, w.x + 300, w.y + 200)");
    assert_eq!(moved, ["true"]);
    assert_eq!(desk.rect("one")[..2], [one[0] + 300, one[1] + 200]);
    assert_eq!(desk.rect("two")[..2], [two[0] + 300, two[1] + 200]);
    // Far away: the cluster stops at the canvas' corner (60000 × 36000).
    desk.eval("desk.move(find('one').id, 200000, 200000)");
    let two = desk.rect("two");
    assert_eq!((two[0] + two[2], two[1] + two[3]), (60000, 36000));
    // A window that is not there.
    assert_eq!(desk.eval("return desk.move(999, 0, 0)"), ["false"]);
}

#[test]
fn closing_by_number_asks_the_program() {
    let desk = Desk::start("");
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    let closed = desk.eval("return desk.close(find('one').id)");
    assert_eq!(closed, ["true"]);
    client.sync();
    assert!(!client.is_open(0));
    assert_eq!(desk.numbers("return #desk.windows()"), [0]);
}

#[test]
fn a_script_stuck_in_a_loop_is_stopped() {
    let desk = Desk::start("");
    let (ok, _, err) = desk.msg(&["eval", "while true do end"]);
    assert!(!ok);
    assert!(err.contains("took longer than 100 ms"), "{err}");
    // And everything goes on.
    assert_eq!(desk.numbers("return 1 + 1"), [2]);
}

#[test]
fn the_event_stream_starts_with_the_state() {
    let desk = Desk::start("");
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    let events = desk.watch();
    let state = events.next();
    assert!(
        state.starts_with(r#"{"event":"state","windows":[{"id":"#),
        "{state}"
    );
    assert!(state.contains(r#""app_id":"one","title":"One""#), "{state}");
    let overview = events.next();
    assert_eq!(
        overview,
        r#"{"event":"view.overview","windows":[],"on":false}"#
    );
    client.open("two", "Two");
    let open = events.next();
    assert!(
        open.starts_with(r#"{"event":"window.open","windows":[{"id":"#),
        "{open}"
    );
    assert!(open.contains(r#""app_id":"two""#), "{open}");
}

#[test]
fn window_rules_set_the_size_fullscreen_or_a_free_place() {
    let desk = Desk::start(
        r#"
        desk.window_rule { match = { app_id = "^wide$" }, width = "full" }
        desk.window_rule { match = { app_id = "^tool$" }, dock = false }
        desk.window_rule { match = { app_id = "^video$" }, open_fullscreen = true }
        "#,
    );
    let mut client = Client::connect(&desk);
    client.open("wide", "Wide");
    let wide = desk.rect("wide");
    assert_eq!((wide[2], wide[3]), (1920 - 2 * 16, HALF.1));
    // The screen's corner, a gap away from the first window.
    let (left, top) = (wide[0] - 16, wide[1] - 16);
    // Free in the middle of the screen, in its own size (the test program's 640 × 480).
    client.open("tool", "Tool");
    assert_eq!(desk.rect("tool"), [left + 640, top + 300, 640, 480]);
    // It docks next to the active window first (the view follows), then covers the screen.
    client.open("video", "Video");
    assert_eq!(desk.rect("video")[2..], [1920, 1080]);
    assert_eq!(desk.eval("return find('video').fullscreen"), ["true"]);
}

#[test]
fn a_program_can_ask_for_fullscreen_and_leave_it() {
    let desk = Desk::start("");
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    let before = desk.rect("one");
    client.fullscreen(0, true);
    assert_eq!(desk.rect("one")[2..], [1920, 1080]);
    assert_eq!(client.windows.list[0].size, (1920, 1080));
    client.fullscreen(0, false);
    assert_eq!(desk.rect("one"), before);
}

#[test]
fn dialogs_open_in_the_middle_and_do_not_dock() {
    let desk = Desk::start("");
    let mut client = Client::connect(&desk);
    client.open("main", "Main");
    client.open_with("dialog", "Save as", Some(0));
    let (main, dialog) = (desk.rect("main"), desk.rect("dialog"));
    assert_eq!(dialog, [main[0] - 16 + 640, main[1] - 16 + 300, 640, 480]);
    // Not part of the main window's cluster: it stays when that moves.
    desk.eval("local w = find('main') desk.move(w.id, w.x + 500, w.y)");
    assert_eq!(desk.rect("dialog"), dialog);
}

#[test]
fn a_changed_config_is_read_while_running_and_a_broken_one_keeps_the_old() {
    let desk = Desk::start("version = 1");
    desk.rewrite_config("desk.layout { gaps = 40 }", 2);
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    assert_eq!(desk.rect("one")[2..], [(1920 - 3 * 40) / 2, 1080 - 2 * 40]);
    // A mistake: the old settings (and Lua state) stay.
    std::fs::write(desk.dir.join("init.lua"), "desk.layout { gaps = ").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !desk
        .log()
        .contains("Error in the config, keeping the settings")
    {
        assert!(Instant::now() < deadline, "no warning:\n{}", desk.log());
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(desk.numbers("return version"), [2]);
    client.open("two", "Two");
    assert_eq!(desk.rect("two")[2..], [(1920 - 3 * 40) / 2, 1080 - 2 * 40]);
}

#[test]
fn with_a_scale_of_two_windows_get_half_the_size() {
    let desk = Desk::start(r#"desk.output { name = "headless", scale = 2 }"#);
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    // The screen counts as 960 × 540 then.
    let half = ((960 - 3 * 16) / 2, 540 - 2 * 16);
    assert_eq!(desk.rect("one")[2..], [half.0, half.1]);
    assert_eq!(client.windows.list[0].size, (half.0 as i32, half.1 as i32));
}

#[test]
fn programs_leave_out_their_titlebar_unless_the_config_lets_them_draw_it() {
    let desk = Desk::start("");
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    let one = &client.windows.list[0];
    assert_eq!((one.titlebar, one.tiled), (Some(false), true));
    // Asking for its own titlebar does not change it.
    client.prefer_titlebar(0, true);
    assert_eq!(client.windows.list[0].titlebar, Some(false));

    let desk = Desk::start("desk.layout { titlebars = true }");
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    let one = &client.windows.list[0];
    assert_eq!((one.titlebar, one.tiled), (Some(true), false));
    client.prefer_titlebar(0, false);
    assert_eq!(client.windows.list[0].titlebar, Some(false));
    client.prefer_titlebar(0, true);
    assert_eq!(client.windows.list[0].titlebar, Some(true));
}

#[test]
fn a_size_change_glides_instead_of_jumping() {
    let desk = Desk::start("");
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    // Mod+Shift+Up: the bottom edge moves up, the window gets half as high.
    desk.eval("desk.run(desk.resize('up'))");
    let mut heights = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(600);
    while Instant::now() < deadline {
        client.sync();
        heights.push(desk.rect("one")[3]);
        std::thread::sleep(Duration::from_millis(15));
    }
    let half = (1080 - 3 * 16) / 2;
    assert_eq!(heights.last(), Some(&half), "{heights:?}");
    // On the way: sizes in between, always getting smaller.
    let between = heights.iter().filter(|&&h| half < h && h < HALF.1).count();
    assert!(between >= 3, "{heights:?}");
    assert!(heights.windows(2).all(|w| w[0] >= w[1]), "{heights:?}");
    // The program drew the end size.
    assert_eq!(client.windows.list[0].size, (HALF.0 as i32, half as i32));
    // Moving it while it glides back: the glide ends at once, the move stays.
    let x = desk.rect("one")[0];
    desk.eval(
        "desk.run(desk.resize('down')) local w = find('one') desk.move(w.id, w.x + 300, w.y)",
    );
    std::thread::sleep(Duration::from_millis(600));
    client.sync();
    assert_eq!(desk.rect("one")[0], x + 300);
    assert_eq!(desk.rect("one")[3], HALF.1);
}

#[test]
fn a_window_shrinks_at_its_other_edge_to_keep_the_cluster_together() {
    let desk = Desk::start(r#"desk.window_rule { match = { app_id = "^low$" }, height = "half" }"#);
    let mut client = Client::connect(&desk);
    let settle = |client: &mut Client| {
        std::thread::sleep(Duration::from_millis(400));
        client.sync();
    };
    // one | two, then two half as high and "low" below it; two goes.
    client.open("one", "One");
    client.open("two", "Two");
    desk.eval("desk.run(desk.resize('up'))");
    settle(&mut client);
    client.open("low", "Low");
    client.close(1);
    settle(&mut client);
    let (one, low) = (desk.rect("one"), desk.rect("low"));
    let half = (1080 - 3 * 16) / 2;
    assert_eq!((low[1], low[3]), (one[1] + half + 16, half));
    // Mod+Shift+Up on one: its bottom edge going up would leave low at a corner, so its top
    // edge comes down, and one ends up next to low.
    desk.eval("desk.focus_window(find('one').id) desk.run(desk.resize('up'))");
    settle(&mut client);
    assert_eq!(desk.rect("one"), [one[0], low[1], one[2], half]);
    assert_eq!(desk.rect("low"), low);
}
