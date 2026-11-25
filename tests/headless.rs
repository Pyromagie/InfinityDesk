//! End-to-end tests without a screen: each test starts InfinityDesk headless (`--headless`, a
//! 1920×1080 output) in a folder of its own, opens test windows that behave like real programs,
//! and looks and steers (also with a made-up mouse and keyboard) through `infinitydesk msg`. No
//! GPU, display or input devices needed.

use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Write},
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
    protocol::{
        wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_registry,
        wl_seat, wl_shm, wl_shm_pool, wl_surface,
    },
};
use wayland_protocols::{
    ext::{
        data_control::v1::client::{
            ext_data_control_device_v1, ext_data_control_manager_v1, ext_data_control_offer_v1,
            ext_data_control_source_v1,
        },
        idle_notify::v1::client::{ext_idle_notification_v1, ext_idle_notifier_v1},
        session_lock::v1::client::{
            ext_session_lock_manager_v1, ext_session_lock_surface_v1, ext_session_lock_v1,
        },
    },
    wp::{
        idle_inhibit::zv1::client::{zwp_idle_inhibit_manager_v1, zwp_idle_inhibitor_v1},
        primary_selection::zv1::client::{
            zwp_primary_selection_device_manager_v1, zwp_primary_selection_device_v1,
            zwp_primary_selection_offer_v1,
        },
    },
    xdg::{
        decoration::zv1::client::{zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1},
        shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base},
    },
};
use wayland_protocols_wlr::{
    foreign_toplevel::v1::client::{
        zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
        zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
    },
    layer_shell::v1::client::{
        zwlr_layer_shell_v1::{self, Layer},
        zwlr_layer_surface_v1::{self, KeyboardInteractivity},
    },
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
        Desk::start_with(1, config)
    }

    /// The same with several monitors.
    fn start_with(monitors: u32, config: &str) -> Desk {
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
            .args(["--headless", &monitors.to_string()])
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

    /// Acts like a mouse and a keyboard (see `src/test_input.rs`).
    fn input(&self, events: &str) {
        let (ok, _, err) = self.msg(&["input", events]);
        assert!(ok, "input {events}: {err}\n{}", self.log());
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
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    lock_manager: Option<ext_session_lock_manager_v1::ExtSessionLockManagerV1>,
    idle_notifier: Option<ext_idle_notifier_v1::ExtIdleNotifierV1>,
    inhibit_manager: Option<zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1>,
    data_control: Option<ext_data_control_manager_v1::ExtDataControlManagerV1>,
    primary: Option<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1>,
    /// What a middle click would paste.
    offer: Option<zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1>,
    seat: Option<wl_seat::WlSeat>,
    /// Where the window list is (bound only by `watch_windows`).
    window_list: Option<(wl_registry::WlRegistry, u32)>,
    outputs: Vec<wl_output::WlOutput>,
    /// Where the window contents (shared memory) are created.
    dir: PathBuf,
    list: Vec<TestWindow>,
    /// The surface with the keyboard, if it is one of this program.
    focus: Option<wl_surface::WlSurface>,
    /// The surface under the pointer, if it is one of this program.
    pointer: Option<wl_surface::WlSurface>,
    /// The keys pressed and let go here.
    keys: Vec<u32>,
    /// Like swaylock: what it heard about the lock it asked for last.
    locked: bool,
    /// Like a dock: the windows it was told about.
    listed: Vec<Listed>,
    /// Like swayidle: nobody is there.
    idle: bool,
    /// Frames shown since `watch_frames`.
    frames: u32,
    lock_refused: bool,
}

/// A window in the list of a dock.
struct Listed {
    handle: ZwlrForeignToplevelHandleV1,
    app_id: String,
    title: String,
    /// 2: active, 3: fullscreen.
    states: Vec<u8>,
    outputs: Vec<wl_output::WlOutput>,
    parent: Option<ZwlrForeignToplevelHandleV1>,
    done: bool,
}

type Lock = ext_session_lock_v1::ExtSessionLockV1;
type Bar = (
    wl_surface::WlSurface,
    zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
);

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

    /// Asks to lock the screen, with a surface on each monitor (in the order of `outputs`).
    fn lock(&mut self) -> (Lock, Vec<wl_surface::WlSurface>) {
        let qh = self.queue.handle();
        let w = &mut self.windows;
        (w.locked, w.lock_refused) = (false, false);
        let lock = w.lock_manager.as_ref().unwrap().lock(&qh, ());
        let mut surfaces = Vec::new();
        for output in &w.outputs {
            let surface = w.compositor.as_ref().unwrap().create_surface(&qh, ());
            lock.get_lock_surface(&surface, output, &qh, surface.clone());
            surfaces.push(surface);
        }
        // The answer comes after a few frames.
        let start = Instant::now();
        while !self.windows.locked && !self.windows.lock_refused {
            assert!(start.elapsed() < Duration::from_secs(5), "no answer");
            self.sync();
        }
        (lock, surfaces)
    }

    /// Opens a bar along the top edge, 40 high.
    fn open_bar(&mut self, keyboard: KeyboardInteractivity) -> Bar {
        let qh = self.queue.handle();
        let w = &mut self.windows;
        let surface = w.compositor.as_ref().unwrap().create_surface(&qh, ());
        let bar = w.layer_shell.as_ref().unwrap().get_layer_surface(
            &surface,
            None,
            Layer::Top,
            "bar".into(),
            &qh,
            surface.clone(),
        );
        use zwlr_layer_surface_v1::Anchor;
        bar.set_anchor(Anchor::Top | Anchor::Left | Anchor::Right);
        bar.set_size(0, 40);
        bar.set_keyboard_interactivity(keyboard);
        surface.commit();
        self.sync();
        (surface, bar)
    }

    /// How many frames window `index` gets in 200 ms.
    fn frames(&mut self, index: usize) -> u32 {
        let surface = self.windows.list[index].surface.clone();
        surface.frame(&self.queue.handle(), surface.clone());
        surface.commit();
        self.sync();
        self.windows.frames = 0;
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(200) {
            self.sync();
            std::thread::sleep(Duration::from_millis(5));
        }
        self.windows.frames
    }

    /// Whether the program hears within half a second that nobody is there.
    fn gets_idle(&mut self) -> bool {
        let start = Instant::now();
        while !self.windows.idle && start.elapsed() < Duration::from_millis(500) {
            self.sync();
            std::thread::sleep(Duration::from_millis(10));
        }
        self.windows.idle
    }

    /// Watches the windows of all programs, like a dock.
    fn watch_windows(&mut self) -> ZwlrForeignToplevelManagerV1 {
        let (registry, name) = self.windows.window_list.as_ref().unwrap();
        let manager = registry.bind(*name, 3, &self.queue.handle(), ());
        self.sync();
        manager
    }

    /// What the dock knows about `app_id`.
    fn listed(&mut self, app_id: &str) -> &Listed {
        self.sync();
        let listed = self.windows.listed.iter().find(|l| l.app_id == app_id);
        let listed = listed.unwrap();
        assert!(listed.done);
        listed
    }

    /// Whether window `index` has the keyboard.
    fn has_keyboard(&mut self, index: usize) -> bool {
        self.sync();
        self.windows.focus.as_ref() == Some(&self.windows.list[index].surface)
    }

    /// The keys that came here since the last call.
    fn keys(&mut self) -> Vec<u32> {
        self.sync();
        std::mem::take(&mut self.windows.keys)
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
        self.fill(&self.list[index].surface, w, h, qh);
    }

    /// Draws a surface plain black.
    fn fill(&self, surface: &wl_surface::WlSurface, w: i32, h: i32, qh: &QueueHandle<Self>) {
        let path = self.dir.join(format!("shm-{}-{w}x{h}", std::process::id()));
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
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(registry.bind(name, 1, qh, ()));
                }
                "ext_session_lock_manager_v1" => {
                    state.lock_manager = Some(registry.bind(name, 1, qh, ()));
                }
                "zwlr_foreign_toplevel_manager_v1" => {
                    state.window_list = Some((registry.clone(), name));
                }
                "ext_data_control_manager_v1" => {
                    state.data_control = Some(registry.bind(name, 1, qh, ()));
                }
                "zwp_primary_selection_device_manager_v1" => {
                    state.primary = Some(registry.bind(name, 1, qh, ()));
                }
                "ext_idle_notifier_v1" => {
                    state.idle_notifier = Some(registry.bind(name, 1, qh, ()));
                }
                "zwp_idle_inhibit_manager_v1" => {
                    state.inhibit_manager = Some(registry.bind(name, 1, qh, ()));
                }
                "wl_output" => state.outputs.push(registry.bind(name, 1, qh, ())),
                "wl_seat" => {
                    let seat: wl_seat::WlSeat = registry.bind(name, 1, qh, ());
                    seat.get_keyboard(qh, ());
                    seat.get_pointer(qh, ());
                    state.seat = Some(seat);
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for Windows {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Enter { surface, .. } => state.focus = Some(surface),
            wl_keyboard::Event::Leave { .. } => state.focus = None,
            wl_keyboard::Event::Key { key, .. } => state.keys.push(key),
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for Windows {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { surface, .. } => state.pointer = Some(surface),
            wl_pointer::Event::Leave { .. } => state.pointer = None,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for Windows {
    fn event(
        state: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
            state.listed.push(Listed {
                handle: toplevel,
                app_id: String::new(),
                title: String::new(),
                states: Vec::new(),
                outputs: Vec::new(),
                parent: None,
                done: false,
            });
        }
    }

    wayland_client::event_created_child!(Windows, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for Windows {
    fn event(
        state: &mut Self,
        handle: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Event;
        let i = state.listed.iter().position(|l| &l.handle == handle);
        let i = i.unwrap();
        let listed = &mut state.listed[i];
        match event {
            Event::AppId { app_id } => listed.app_id = app_id,
            Event::Title { title } => listed.title = title,
            Event::State { state } => {
                listed.states = state.chunks(4).map(|c| c[0]).collect();
            }
            Event::OutputEnter { output } => listed.outputs.push(output),
            Event::OutputLeave { output } => listed.outputs.retain(|o| o != &output),
            Event::Parent { parent } => listed.parent = parent,
            Event::Done => listed.done = true,
            Event::Closed => {
                state.listed.remove(i);
            }
            _ => {}
        }
    }
}

impl Dispatch<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, ()> for Windows {
    fn event(
        state: &mut Self,
        _: &zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
        event: zwp_primary_selection_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_primary_selection_device_v1::Event::Selection { id } = event {
            state.offer = id;
        }
    }

    wayland_client::event_created_child!(Windows, zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, [
        zwp_primary_selection_device_v1::EVT_DATA_OFFER_OPCODE =>
            (zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1, ()),
    ]);
}

impl Dispatch<ext_data_control_device_v1::ExtDataControlDeviceV1, ()> for Windows {
    fn event(
        _: &mut Self,
        _: &ext_data_control_device_v1::ExtDataControlDeviceV1,
        _: ext_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    wayland_client::event_created_child!(Windows, ext_data_control_device_v1::ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE =>
            (ext_data_control_offer_v1::ExtDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ext_data_control_source_v1::ExtDataControlSourceV1, ()> for Windows {
    fn event(
        _: &mut Self,
        _: &ext_data_control_source_v1::ExtDataControlSourceV1,
        event: ext_data_control_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_data_control_source_v1::Event::Send { fd, .. } = event {
            File::from(fd).write_all(b"pasted").unwrap();
        }
    }
}

impl Dispatch<ext_idle_notification_v1::ExtIdleNotificationV1, ()> for Windows {
    fn event(
        state: &mut Self,
        _: &ext_idle_notification_v1::ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.idle = matches!(event, ext_idle_notification_v1::Event::Idled);
    }
}

impl Dispatch<wl_callback::WlCallback, wl_surface::WlSurface> for Windows {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        surface: &wl_surface::WlSurface,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        state.frames += 1;
        surface.frame(qh, surface.clone());
        surface.commit();
    }
}

impl Dispatch<ext_session_lock_v1::ExtSessionLockV1, ()> for Windows {
    fn event(
        state: &mut Self,
        _: &ext_session_lock_v1::ExtSessionLockV1,
        event: ext_session_lock_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_session_lock_v1::Event::Locked => state.locked = true,
            _ => state.lock_refused = true,
        }
    }
}

impl Dispatch<ext_session_lock_surface_v1::ExtSessionLockSurfaceV1, wl_surface::WlSurface>
    for Windows
{
    fn event(
        state: &mut Self,
        lock: &ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        surface: &wl_surface::WlSurface,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let ext_session_lock_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        {
            lock.ack_configure(serial);
            state.fill(surface, width as i32, height as i32, qh);
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, wl_surface::WlSurface> for Windows {
    fn event(
        state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        surface: &wl_surface::WlSurface,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        {
            layer.ack_configure(serial);
            state.fill(surface, width as i32, height as i32, qh);
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
delegate_noop!(Windows: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
delegate_noop!(Windows: ignore ext_data_control_manager_v1::ExtDataControlManagerV1);
delegate_noop!(Windows: ignore ext_data_control_offer_v1::ExtDataControlOfferV1);
delegate_noop!(Windows: ignore zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1);
delegate_noop!(Windows: ignore zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1);
delegate_noop!(Windows: ignore ext_idle_notifier_v1::ExtIdleNotifierV1);
delegate_noop!(Windows: ignore zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1);
delegate_noop!(Windows: ignore zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1);
delegate_noop!(Windows: ignore ext_session_lock_manager_v1::ExtSessionLockManagerV1);
delegate_noop!(Windows: ignore wl_output::WlOutput);
delegate_noop!(Windows: ignore wl_seat::WlSeat);
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

/// Two monitors: 1920×1080 (`headless`) and 1280×720 (`headless-2`), without animations.
const TWO: &str = r#"
desk.output { name = "headless-2", mode = "1280x720" }
desk.animation { duration_ms = 0 }
function view(i)
  local o = desk.outputs()[i]
  return math.floor(o.view.x), math.floor(o.view.y), o.view.zoom, o.focused
end
"#;

#[test]
fn each_monitor_has_its_own_view_of_the_canvas() {
    let desk = Desk::start_with(2, TWO);
    let outputs = "local r = {} for _, o in ipairs(desk.outputs()) do
        table.insert(r, { o.name, o.x, o.y, o.width, o.height }) end return r";
    assert_eq!(
        desk.eval(outputs),
        [r#"[["headless",0,0,1920,1080],["headless-2",1920,0,1280,720]]"#]
    );
    // The views start side by side, the first one's in the middle of the canvas.
    let (x, y) = (30000 - 960, 18000 - 540);
    let first = [x.to_string(), y.to_string(), "1".into(), "true".into()];
    let second = [
        (x + 1920).to_string(),
        y.to_string(),
        "1".into(),
        "false".into(),
    ];
    assert_eq!(desk.eval("return view(1)"), first);
    assert_eq!(desk.eval("return view(2)"), second);
    // Keys act on the current monitor, the other view stays.
    desk.eval("desk.focus_output('headless-2') desk.run(desk.pan('right'))");
    let moved = (x + 1920 + 1280 / 4).to_string();
    assert_eq!(desk.eval("return view(1)")[..3], first[..3]);
    assert_eq!(desk.eval("return view(2)")[0], moved);
    desk.eval("desk.run(desk.zoom_out)");
    assert_eq!(desk.eval("return view(1)")[..3], first[..3]);
    assert_ne!(desk.eval("return view(2)")[2], "1");
    assert_eq!(desk.eval("return desk.focus_output('none')"), ["false"]);
}

#[test]
fn new_windows_open_on_the_current_monitor_in_its_sizes() {
    let desk = Desk::start_with(2, TWO);
    let mut client = Client::connect(&desk);
    let x = 30000 - 960;
    let y = 18000 - 540;
    client.open("one", "One");
    assert_eq!(desk.rect("one"), [x + 16, y + 16, HALF.0, HALF.1]);
    desk.eval("desk.focus_output('headless-2')");
    // Not docked to the active window, which is on the other monitor.
    client.open("two", "Two");
    let half = ((1280 - 3 * 16) / 2, 720 - 2 * 16);
    let left = x + 1920 + 16;
    assert_eq!(desk.rect("two"), [left, y + 16, half.0, half.1]);
    client.open("three", "Three");
    let right = left + half.0 + 16;
    assert_eq!(desk.rect("three"), [right, y + 16, half.0, half.1]);
    // Focusing a window on the other monitor goes there instead of moving the view.
    desk.eval("desk.focus_window(find('one').id)");
    assert_eq!(desk.eval("return view(1)")[3], "true");
    assert_eq!(desk.numbers("return (view(1))"), [x]);
    assert_eq!(desk.numbers("return (view(2))"), [x + 1920]);
    // Sizes change in steps of the window's monitor, wherever the pointer is.
    desk.eval("desk.focus_output('headless-2') desk.run(desk.resize('right'))");
    client.sync();
    assert_eq!(desk.rect("one"), [x + 16, y + 16, 1920 - 32, HALF.1]);
}

#[test]
fn fullscreen_covers_only_its_monitor() {
    let desk = Desk::start_with(2, TWO);
    let mut client = Client::connect(&desk);
    let (x, y) = (30000 - 960, 18000 - 540);
    client.open("one", "One");
    desk.eval("desk.focus_output('headless-2')");
    client.open("two", "Two");
    let before = desk.rect("two");
    // Also asked for while the pointer is on the other monitor.
    desk.eval("desk.focus_output('headless')");
    client.fullscreen(1, true);
    assert_eq!(desk.rect("two")[2..], [1280, 720]);
    // The other monitor goes on as before; moving around there does not end it.
    desk.eval("desk.run(desk.pan('down'))");
    assert_eq!(desk.numbers("local _, y = view(1) return y"), [y + 270]);
    assert_eq!(desk.eval("return find('two').fullscreen"), ["true"]);
    client.fullscreen(1, false);
    assert_eq!(desk.rect("two"), before);
    assert_eq!(desk.numbers("return (view(2))"), [x + 1920]);
}

/// Mod (Super) as the tests press it.
const MOD: u32 = 125;

#[test]
fn windows_are_dragged_dock_and_come_out_by_holding() {
    let desk = Desk::start("desk.animation { duration_ms = 0 }");
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    client.open("two", "Two");
    let (one, two) = (desk.rect("one"), desk.rect("two"));
    // The pointer is in the middle of the screen, in the gap between the two. Dragging two
    // without holding still, both move: 300 up, 200 left.
    desk.input(&format!(
        "key {MOD} down; move 100 0; button left down; move -100 -150; move -100 -150; button left up"
    ));
    assert_eq!(
        desk.rect("one")[..2],
        [one[0] - 200, one[1] - 300],
        "the cluster moves"
    );
    // Held still first, only two comes out: far away, it docks nowhere.
    desk.input("button left down");
    std::thread::sleep(Duration::from_millis(400));
    desk.input("move 300 200; move 200 100; button left up");
    assert_eq!(desk.rect("one")[..2], [one[0] - 200, one[1] - 300]);
    assert_eq!(desk.rect("two")[..2], [two[0] + 300, two[1]]);
    // Slowly back to 10 px beside the old place: it snaps with the gap and docks.
    let events = desk.watch();
    events.next();
    events.next();
    desk.input("button left down; move -200 -100");
    for _ in 0..28 {
        desk.input("move -10 -7");
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(100));
    desk.input("move -10 -1; button left up");
    desk.input(&format!("key {MOD} up"));
    assert_eq!(desk.rect("two")[..2], [two[0] - 200, two[1] - 300]);
    assert!(events.next().contains("cluster.dock"));
}

#[test]
fn the_border_between_two_windows_is_dragged_with_the_mouse() {
    let desk = Desk::start("desk.animation { duration_ms = 0 }");
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    client.open("two", "Two");
    let (one, two) = (desk.rect("one"), desk.rect("two"));
    // Grabbed at the right edge of one (4 px left of the middle of the 16 px gap): pulled left,
    // two gets full width. Less than half the way goes back.
    let full = 2 * HALF.0 + 16;
    desk.input("move -6 0; button left down; move -100 0; move -100 0; button left up");
    client.sync();
    assert_eq!((desk.rect("one"), desk.rect("two")), (one.clone(), two));
    desk.input("move 200 0; button left down; move 100 0; move 600 0; button left up");
    client.sync();
    assert_eq!(desk.rect("one"), [one[0], one[1], full, one[3]]);
}

#[test]
fn keys_focus_the_next_window_and_alt_tab_goes_back() {
    let desk = Desk::start("");
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    client.open("two", "Two");
    let focused = || desk.eval("return desk.focused().app_id");
    // Mod+Left (105).
    desk.input(&format!(
        "key {MOD} down; key 105 down; key 105 up; key {MOD} up"
    ));
    assert_eq!(focused(), [r#""one""#]);
    // Alt (56) + Tab (15).
    desk.input("key 56 down; key 15 down; key 15 up; key 56 up");
    assert_eq!(focused(), [r#""two""#]);
}

#[test]
fn a_click_in_the_overview_chooses_the_window() {
    let desk = Desk::start("desk.animation { duration_ms = 0 }");
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    client.open("two", "Two");
    desk.eval("desk.run(desk.overview)");
    desk.input("move -200 0; button left down; button left up");
    assert_eq!(desk.eval("return desk.focused().app_id"), [r#""one""#]);
    assert_eq!(desk.numbers("return desk.outputs()[1].view.zoom"), [1]);
}

/// The name of the monitor the pointer is on.
fn current(desk: &Desk) -> String {
    let code = "for _, o in ipairs(desk.outputs()) do if o.focused then return o.name end end";
    desk.eval(code).concat().replace('"', "")
}

#[test]
fn the_pointer_goes_over_to_the_monitor_beside() {
    let desk = Desk::start_with(2, TWO);
    // From the middle of the first monitor over its right edge and back.
    desk.input("move 1000 0");
    assert_eq!(current(&desk), "headless-2");
    desk.input("move -100 0");
    assert_eq!(current(&desk), "headless");
    // Beside the lower part there is no monitor: the pointer stays at the edge.
    desk.input("move -900 400; move 1000 0");
    assert_eq!(current(&desk), "headless");
    // Dragging the canvas stays on its monitor, also beyond its edge.
    desk.input("move 0 -400; move -100 0; button left down; move 500 0");
    assert_eq!(current(&desk), "headless");
    desk.input("button left up; move 10 0");
    assert_eq!(current(&desk), "headless-2");
    // Placed to the left in the config, at once; the views stay.
    let view = desk.eval("return view(2)");
    desk.rewrite_config(
        &format!(
            "{TWO} desk.output {{ name = 'headless-2', position = {{ x = -1280, y = 100 }} }}"
        ),
        1,
    );
    assert_eq!(
        desk.numbers("local o = desk.outputs()[2] return o.x, o.y"),
        [-1280, 100]
    );
    assert_eq!(desk.eval("return view(2)"), view);
    desk.input("move 2000 0");
    assert_eq!(current(&desk), "headless");
}

#[test]
fn a_button_held_on_a_program_keeps_the_pointer_on_its_monitor() {
    let desk = Desk::start_with(2, TWO);
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    desk.input("move -300 0; button left down; move 1500 0");
    assert_eq!(current(&desk), "headless");
    desk.input("button left up; move 10 0");
    assert_eq!(current(&desk), "headless-2");
}

#[test]
fn windows_dragged_to_another_monitor_take_its_sizes() {
    let desk = Desk::start_with(2, TWO);
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    client.open("two", "Two");
    let (x, y) = (30000 - 960, 18000 - 540);
    let before = (desk.rect("one"), desk.rect("two"));
    // Grabbed in one, 400 px left of the middle; over to 140 px from the second's left edge.
    desk.input(&format!(
        "move -400 0; key {MOD} down; button left down; move 1480 0; move 20 0"
    ));
    assert_eq!(current(&desk), "headless-2");
    client.sync();
    let (one, two) = (desk.rect("one"), desk.rect("two"));
    let half = ((1280 - 3 * 16) / 2, 720 - 2 * 16);
    assert_eq!((one[2], one[3]), half);
    assert_eq!((two[2], two[3]), half);
    // Still side by side, and held at the same spot of the window (the 544 px from its left
    // edge became 361).
    assert_eq!(two[..2], [one[0] + half.0 + 16, one[1]]);
    assert_eq!(one[..2], [x + 1920 + 140 - 361, y + 193]);
    // Back: as before.
    desk.input("move -1500 0; button left up");
    client.sync();
    assert_eq!((desk.rect("one"), desk.rect("two")), before);
    // A window with a size of its own (in the middle of the first monitor) keeps it.
    client.open_with("dialog", "Dialog", Some(0));
    let dialog = desk.rect("dialog");
    desk.input("move 400 0; button left down; move 1000 0; button left up");
    desk.input(&format!("key {MOD} up"));
    client.sync();
    let moved = [dialog[0] + 1000, dialog[1], dialog[2], dialog[3]];
    assert_eq!(desk.rect("dialog"), moved);
}

#[test]
fn monitors_come_and_go_while_running() {
    let desk = Desk::start_with(2, TWO);
    let mut client = Client::connect(&desk);
    let (x, y) = (30000 - 960, 18000 - 540);
    client.open("one", "One");
    desk.eval("desk.focus_output('headless-2') desk.run(desk.pan('right'))");
    client.open("two", "Two");
    client.fullscreen(1, true);
    let view = desk.eval("return view(1)");
    let events = desk.watch();
    events.next();
    events.next();
    // Its fullscreen window goes back to its place, the pointer to the other monitor, whose
    // view stays.
    desk.input("unplug headless-2");
    client.sync();
    assert_eq!(current(&desk), "headless");
    assert_eq!(desk.numbers("return #desk.outputs()"), [1]);
    assert_eq!(desk.eval("return find('two').fullscreen"), ["false"]);
    assert_eq!(desk.rect("two")[2..], [(1280 - 48) / 2, 720 - 32]);
    assert_eq!(desk.eval("return view(1)")[..3], view[..3]);
    assert!(events.next().contains("window.fullscreen"));
    assert!(events.next().contains("output.changed"));
    // Back: with the view it had.
    desk.input("plug headless-2");
    let second = "local x, y = view(2) return x, y";
    assert_eq!(desk.numbers(second), [x + 1920 + 320, y]);
    // The first one goes: the second one moves to its place, its view stays.
    desk.input("unplug headless");
    assert_eq!(current(&desk), "headless-2");
    let place = "local o = desk.outputs()[1] return o.x, o.width";
    assert_eq!(desk.numbers(place), [0, 1280]);
    assert_eq!(desk.eval("return view(1)")[0], (x + 1920 + 320).to_string());
    // The last one, too: everything goes on without a screen, also the pointer.
    desk.input("unplug headless-2; move 10 10; button left down; button left up");
    client.open("three", "Three");
    assert_eq!(
        desk.numbers("return #desk.outputs(), #desk.windows()"),
        [0, 3]
    );
    desk.eval("desk.run(desk.zoom_out) desk.run(desk.overview) desk.run(desk.overview)");
    desk.input("plug headless-2");
    assert_eq!(current(&desk), "headless-2");
    desk.eval("desk.focus_window(find('one').id)");
}

/// Presses and lets go a key (its code as in linux/input-event-codes.h).
fn press(desk: &Desk, key: u32) {
    desk.input(&format!("key {key} down; key {key} up"));
}

#[test]
fn a_locked_screen_gives_input_only_to_the_lock() {
    let desk = Desk::start_with(2, TWO);
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    desk.input("move -300 0");
    assert!(client.has_keyboard(0) && client.windows.pointer.is_some());
    press(&desk, 30);
    assert_eq!(client.keys(), [30, 30]);

    let mut locker = Client::connect(&desk);
    // Held since before: the move ends.
    desk.input(&format!("key {MOD} down; button left down; move 10 0"));
    let (lock, surfaces) = locker.lock();
    assert!(locker.windows.locked);
    let rect = desk.rect("one");
    desk.input(&format!(
        "move 500 0; button left up; key {MOD} up; move -500 0"
    ));
    assert_eq!(desk.rect("one"), rect);
    assert_eq!(locker.windows.focus.as_ref(), Some(&surfaces[0]));
    client.keys();
    locker.keys();

    // Keys and clicks go to the lock, no shortcut works: Mod+O would open the overview, Mod+drag
    // move the window.
    press(&desk, 30);
    assert_eq!(locker.keys(), [30, 30]);
    desk.input(&format!(
        "key {MOD} down; key 24 down; key 24 up; button left down; move 50 0; \
         button left up; key {MOD} up; move -50 0; button left down; button left up"
    ));
    assert!(!client.has_keyboard(0) && client.windows.pointer.is_none());
    locker.sync();
    assert_eq!(locker.windows.pointer.as_ref(), Some(&surfaces[0]));
    assert_eq!(desk.numbers("return desk.outputs()[1].view.zoom"), [1]);
    assert_eq!(client.keys(), [0; 0]);
    assert_eq!(desk.rect("one"), rect);
    // Neither new windows nor scripts nor launchers get the keyboard.
    client.open("two", "Two");
    desk.eval("desk.focus_window(find('one').id)");
    client.open_bar(KeyboardInteractivity::Exclusive);
    assert!(client.windows.focus.is_none());
    // The lock on the other monitor.
    desk.input("move 1500 0");
    locker.sync();
    assert_eq!(locker.windows.focus.as_ref(), Some(&surfaces[1]));
    desk.input("move -1500 0");

    // Only one can lock.
    let mut second = Client::connect(&desk);
    second.lock();
    assert!(second.windows.lock_refused);
    // It crashes: the screen stays locked until another one takes over and unlocks.
    drop(lock);
    drop(locker);
    press(&desk, 30);
    assert_eq!(client.keys(), [0; 0]);
    assert!(client.windows.focus.is_none());
    let (lock, surfaces) = second.lock();
    assert!(second.windows.locked);
    second.sync();
    assert_eq!(second.windows.focus.as_ref(), Some(&surfaces[0]));
    lock.unlock_and_destroy();
    second.sync();
    assert!(second.windows.focus.is_none());
    press(&desk, 30);
    assert_eq!(client.keys(), [30, 30]);
}

#[test]
fn the_screen_can_be_locked_while_monitors_come_and_go() {
    let desk = Desk::start_with(1, TWO);
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    let mut locker = Client::connect(&desk);
    desk.input("unplug headless");
    locker.windows.outputs.clear();
    let (lock, _) = locker.lock();
    assert!(locker.windows.locked);
    desk.input("plug headless-2");
    press(&desk, 30);
    assert_eq!(client.keys(), [0; 0]);
    // The lock gets a surface on the new monitor.
    locker.sync();
    let surface =
        (locker.windows.compositor.as_ref().unwrap()).create_surface(&locker.queue.handle(), ());
    let output = locker.windows.outputs.last().unwrap();
    lock.get_lock_surface(&surface, output, &locker.queue.handle(), surface.clone());
    locker.sync();
    assert_eq!(locker.windows.focus.as_ref(), Some(&surface));
    lock.unlock_and_destroy();
    locker.sync();
    assert!(client.has_keyboard(0));
}

#[test]
fn bars_and_launchers_get_the_keyboard_as_they_ask() {
    let desk = Desk::start("");
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    // A bar that takes it on a click, until another one.
    let (bar, _) = client.open_bar(KeyboardInteractivity::OnDemand);
    assert!(client.has_keyboard(0));
    desk.input("move 0 -530; button left down; button left up");
    client.sync();
    assert_eq!(client.windows.focus.as_ref(), Some(&bar));
    desk.input("move -300 530; button left down; button left up");
    assert!(client.has_keyboard(0));
    // A launcher keeps it, whatever is clicked, until it closes.
    let mut other = Client::connect(&desk);
    let (launcher, layer) = other.open_bar(KeyboardInteractivity::Exclusive);
    desk.input("button left down; button left up");
    other.sync();
    assert_eq!(other.windows.focus.as_ref(), Some(&launcher));
    layer.destroy();
    launcher.destroy();
    other.sync();
    assert!(client.has_keyboard(0));
}

#[test]
fn programs_learn_when_nobody_is_there() {
    let desk = Desk::start("desk.animation { duration_ms = 0 }");
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    let qh = client.queue.handle();
    let w = &client.windows;
    let (notifier, seat) = (w.idle_notifier.clone().unwrap(), w.seat.clone().unwrap());
    let manager = w.inhibit_manager.clone().unwrap();
    notifier.get_idle_notification(100, &seat, &qh, ());
    assert!(client.gets_idle());
    desk.input("move 1 0");
    client.sync();
    assert!(!client.windows.idle);
    // A video keeps it awake, as long as it can be seen.
    let surface = client.windows.list[0].surface.clone();
    let inhibitor = manager.create_inhibitor(&surface, &qh, ());
    client.sync();
    assert!(!client.gets_idle());
    for _ in 0..4 {
        desk.eval("desk.run(desk.pan('right'))");
    }
    assert!(client.gets_idle());
    for _ in 0..4 {
        desk.eval("desk.run(desk.pan('left'))");
    }
    client.sync();
    assert!(!client.gets_idle());
    inhibitor.destroy();
    assert!(client.gets_idle());
}

#[test]
fn screens_are_off_until_the_next_input() {
    let desk = Desk::start("desk.bind('Mod+X', desk.screens_off)");
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    assert!(client.frames(0) > 5);
    // Letting go the keys (Mod+X) does not switch them on again.
    desk.input(&format!(
        "key {MOD} down; key 45 down; key 45 up; key {MOD} up"
    ));
    assert!(client.frames(0) < 2);
    // The screen can be locked meanwhile.
    let mut locker = Client::connect(&desk);
    let (lock, _) = locker.lock();
    assert!(locker.windows.locked);
    lock.unlock_and_destroy();
    locker.sync();
    desk.input("move 1 0");
    assert!(client.frames(0) > 5);
}

#[test]
fn docks_get_the_windows_and_can_steer_them() {
    let desk = Desk::start_with(2, &format!("{TWO} desk.animation {{ duration_ms = 0 }}"));
    let mut client = Client::connect(&desk);
    client.open("one", "One");
    let mut dock = Client::connect(&desk);
    let manager = dock.watch_windows();
    let first = dock.windows.outputs[0].clone();
    let one = dock.listed("one");
    assert_eq!((one.title.as_str(), &one.states[..]), ("One", &[2][..]));
    assert!(one.outputs.contains(&first));
    // Opened later, changed, as a dialog of the first one.
    client.open_with("two", "Two", Some(0));
    assert_eq!(dock.listed("one").states, [0; 0]);
    let one = dock.listed("one").handle.clone();
    let two = dock.listed("two");
    assert_eq!(
        (&two.states[..], &two.parent),
        (&[2][..], &Some(one.clone()))
    );
    let two = two.handle.clone();
    client.windows.list[1].toplevel.set_title("New".into());
    client.sync();
    assert_eq!(dock.listed("two").title, "New");
    // The dock activates, also far away: the view goes there.
    let view = desk.eval("return view(1)");
    for _ in 0..5 {
        desk.eval("desk.run(desk.pan('right'))");
    }
    assert!(dock.listed("one").outputs.is_empty());
    one.activate(dock.windows.seat.as_ref().unwrap());
    assert!(dock.listed("one").outputs.contains(&first));
    assert_eq!(desk.eval("return desk.focused().app_id"), [r#""one""#]);
    assert_eq!(desk.eval("return view(1)")[1..], view[1..]);
    // Fullscreen and back.
    one.set_fullscreen(None);
    assert_eq!(dock.listed("one").states, [2, 3]);
    assert_eq!(desk.eval("return find('one').fullscreen"), ["true"]);
    one.unset_fullscreen();
    assert_eq!(dock.listed("one").states, [2]);
    // A monitor that comes later: the windows seen there are told when the dock knows it.
    desk.eval("desk.focus_output('headless-2')");
    for _ in 0..4 {
        desk.eval("desk.run(desk.pan('left'))");
    }
    assert_eq!(dock.listed("one").outputs.len(), 2);
    desk.input("unplug headless-2");
    assert_eq!(dock.listed("one").outputs, [first]);
    desk.input("plug headless-2");
    dock.sync();
    let second = dock.windows.outputs.last().unwrap().clone();
    assert!(dock.listed("one").outputs.contains(&second));
    // Closed, by the dock and by the program.
    two.close();
    dock.sync();
    client.sync();
    assert!(!client.is_open(1));
    dock.sync();
    assert_eq!(dock.windows.listed.len(), 1);
    // Told to stop, it hears about no new windows, but still about the ones it knows.
    manager.stop();
    dock.sync();
    client.open("three", "Three");
    assert_eq!(dock.listed("one").states, [0; 0]);
    assert_eq!(dock.windows.listed.len(), 1);
}

#[test]
fn a_clipboard_tool_gives_a_window_what_it_pastes() {
    let desk = Desk::start("");
    let mut client = Client::connect(&desk);
    let qh = client.queue.handle();
    let w = &client.windows;
    let seat = w.seat.clone().unwrap();
    w.primary.as_ref().unwrap().get_device(&seat, &qh, ());
    client.open("one", "One");
    assert!(client.windows.offer.is_none());

    let mut tool = Client::connect(&desk);
    let qh = tool.queue.handle();
    let w = &tool.windows;
    let manager = w.data_control.clone().unwrap();
    let source = manager.create_data_source(&qh, ());
    source.offer("text/plain".into());
    let device = manager.get_data_device(w.seat.as_ref().unwrap(), &qh, ());
    device.set_primary_selection(Some(&source));
    tool.sync();

    client.sync();
    let offer = client.windows.offer.clone().unwrap();
    let (mut read, write) = std::io::pipe().unwrap();
    offer.receive("text/plain".into(), write.as_fd());
    client.sync();
    drop(write);
    tool.sync();
    let mut text = String::new();
    read.read_to_string(&mut text).unwrap();
    assert_eq!(text, "pasted");
}
