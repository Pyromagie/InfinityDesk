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
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

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
        assert!(windows.wm_base.is_some());
        Client { queue, windows }
    }

    /// Opens a window and waits until it is drawn and placed.
    fn open(&mut self, app_id: &str, title: &str) {
        let qh = self.queue.handle();
        let w = &mut self.windows;
        let index = w.list.len();
        let surface = w.compositor.as_ref().unwrap().create_surface(&qh, ());
        let xdg = (w.wm_base.as_ref().unwrap()).get_xdg_surface(&surface, &qh, index);
        let toplevel = xdg.get_toplevel(&qh, index);
        toplevel.set_app_id(app_id.into());
        toplevel.set_title(title.into());
        surface.commit();
        w.list.push(TestWindow {
            surface,
            xdg,
            toplevel,
            size: (0, 0),
            open: true,
        });
        self.sync();
    }

    /// Closes a window from the program's side, like quitting it.
    fn close(&mut self, index: usize) {
        self.windows.list[index].close();
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
                "xdg_wm_base" => state.wm_base = Some(registry.bind(name, 1, qh, ())),
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
            xdg_toplevel::Event::Configure { width, height, .. } => {
                state.list[*index].size = (width, height);
            }
            xdg_toplevel::Event::Close => state.list[*index].close(),
            _ => {}
        }
    }
}

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
    // Far away: the cluster stops at the canvas' corner (20000 × 12000).
    desk.eval("desk.move(find('one').id, 100000, 100000)");
    let two = desk.rect("two");
    assert_eq!((two[0] + two[2], two[1] + two[3]), (20000, 12000));
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
