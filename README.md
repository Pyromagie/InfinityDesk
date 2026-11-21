# InfinityDesk

A Wayland compositor (window manager) built around a **huge, bounded canvas**. Instead of
workspaces or a fixed screen, your windows live on one large 2D surface. You place them
anywhere, dock them to each other to form **clusters**, and pan and zoom across the canvas to
get around. Think of a desk you can spread your work out on, with the screen as a camera above it.

Inspired by [niri](https://github.com/YaLTeR/niri) (freedom to scroll horizontally), taken one
step further into two dimensions. Built in Rust on [Smithay](https://github.com/Smithay/smithay).

> **Status: early development.** InfinityDesk can be chosen as a session in your login screen
> or tried as a window inside your current session. The standalone session is new: one monitor
> for now.

## The idea

- **Canvas:** a very large but finite surface (20000 × 12000 logical pixels by default).
  Your screen is a viewport onto it.
- **Pan and zoom:** move the viewport freely. Zooming in stops at 1:1 (windows at their real
  size), zooming out stops when the whole canvas fits on the screen.
- **Clusters:** drag a window next to another one and it docks edge to edge, like Windows Snap.
  Docked windows form a cluster that moves as one. Clusters can be any shape: rows, columns,
  grids, L-shapes, even a hollow box.
- **Windows stay where you put them.** Nothing is tiled automatically; you build the
  arrangement you want and it stays.

## Installation

### What you need

- A Linux system with a graphics card that supports OpenGL ES (tested so far on NVIDIA)
- A login manager that lists Wayland sessions (greetd greeters, SDDM, GDM, ...)
- On Arch-based systems (Arch, CachyOS, EndeavourOS, Manjaro, ...) the install script sets up
  everything else. Elsewhere you need Rust (https://rustup.rs), a C compiler, `pkg-config`, the
  development files of `wayland`, `libxkbcommon`, `libinput`, `libdrm`, `mesa` (gbm, EGL),
  `libudev` and `libseat`, `kitty` and `fuzzel` for the default shortcuts, and
  `xwayland-satellite` for X11 programs (optional).

### Install

```sh
git clone https://github.com/Pyromagie/InfinityDesk.git
cd InfinityDesk
./install.sh
```

The script:

1. checks the required software and installs what is missing (Arch-based systems, with
   `pacman`; elsewhere it lists what is needed),
2. builds InfinityDesk,
3. installs it to `/usr/local/bin` and adds the session entry
   `/usr/share/wayland-sessions/infinitydesk.desktop`.

It asks for your password (sudo) when it installs something. If everything is already
installed and up to date, it says so and does nothing.

### First start

1. Log out.
2. In the login screen, choose **InfinityDesk** as the session (where you would pick niri or
   Plasma) and log in.
3. A terminal opens in the middle of the canvas. **Super** is the modifier (Mod):
   Super + Enter opens another terminal, Super + Space the app launcher, Super + Shift + E logs
   out again. All controls are listed under [Controls](#controls-provisional).

### Update

```sh
cd InfinityDesk
git pull
./install.sh
```

Then log out and in again.

### Uninstall

```sh
sudo rm /usr/local/bin/infinitydesk /usr/local/bin/infinitydesk-session \
        /usr/share/wayland-sessions/infinitydesk.desktop
```

Your settings (`~/.config/infinitydesk/`) and the log (`~/.local/state/infinitydesk.log`) stay;
delete them too if you like.

### If something goes wrong

- **Super + Shift + E** quits InfinityDesk and brings you back to the login screen.
- **Ctrl + Alt + F2** (or F3 ... F6) switches to a text console. Log in there and run
  `pkill infinitydesk` to end it.
- The log of the session is in `~/.local/state/infinitydesk.log`. Please attach it when you
  report a problem.

### Trying it without installing

Inside your current Wayland session, InfinityDesk opens as a window, and Mod is **Alt**
(the host compositor keeps Super). This is the safest way to look around:

```sh
cargo run --release                  # with a kitty terminal inside
cargo run --release -- -c firefox    # with another program inside
```

## What works today

- Standalone session on the screen (login manager entry, one monitor at its own resolution and
  highest refresh rate, GPU buffers from apps, console switching) or nested as a window
- xdg-shell windows, popups and menus, keyboard, mouse and touchpad, clipboard and drag and drop
  between apps
- Canvas with a dot grid, panning and zooming around the pointer (drawn on the GPU, only
  visible windows are rendered). When a pan ends almost aligned (at 1:1, within a tenth of the
  screen), the view snaps so the windows sit exactly in the screen.
- **Docking:** edges snap together within 15 screen pixels at any zoom level, with a gap
  between the windows, and line up flush (tops or bottoms side by side, left or right edges
  stacked, whichever is closer), so windows of different sizes dock too and there are no offset
  connections. Pushing a window a bit too
  far, onto its neighbor, still docks it at the edge. Spots that are already taken are skipped,
  and windows passing by quickly do not snap.
- **Clusters** move as a whole, split automatically when a window leaves, and merge when they
  touch another window
- **Detaching:** hold a window still for 0.3 s (a blue pulse shows it is unlocked), then drag
  it out of its cluster
- **Marking:** mark several windows (single clicks or a selection box) and pull them out of
  their cluster together; they stay connected among themselves
- **Cluster resize:** a window is half or full the screen's width and height, and resizing
  toggles between the two once the edge is dragged halfway. Neighbors give or take the space, so
  the cluster stays closed; if they cannot (they would get smaller than half or larger than
  full), or with Shift, the rest of the cluster moves instead. Grab any window border (a generous
  zone around it shows resize arrows and lights up the edges that will move), or use Mod +
  right-drag.
- **Layout templates:** Mod + R opens a menu of templates (whole screen, half | half, a half
  and two quarters, four quarters). Picking one arranges the active window's
  cluster so it fills exactly one screen, shown at 1:1. Click the slot the active window should
  go to (the other windows take the slots closest to where they are), or press the template's
  number. Slots left over are filled by the next windows you open. Other clusters in the way
  move aside (right or down, whichever is shorter), together with what they push along.
- **New windows** open half the screen wide and as high as the screen, and dock to the active
  window (right of it, else below, left, above; the view follows if there is no room on the
  screen). Click empty space first for a free window: it goes into the left half of the screen.
  So two new windows fill the screen exactly.
- **Fullscreen:** Mod + F (or the app itself, e.g. F11 in a browser) lets a window cover the
  screen; its place in the cluster and the view are restored afterwards
- **Keyboard navigation:** move the focus to the next window in a direction (also into the
  next cluster, the view follows), swap a window with its neighbor, toggle its size between
  half and full (the cluster stays closed), pan and zoom the view, center the active cluster,
  an overview that zooms out until all windows fit, and Alt + Tab through the recently used
  windows
- **Touchpad gestures:** pinch to zoom, three fingers to pan, four fingers for the overview
- **Lua configuration** with live reload: keyboard, touchpad, mouse, canvas, gaps, snapping,
  colors, shortcuts, programs to start and their environment variables, pointer theme
- wlr-layer-shell, so launchers like fuzzel, bars and wallpaper tools (e.g. awww, swaybg) work;
  the wallpaper stays fixed behind the moving canvas
- **X11 programs** (Steam, older games and tools) through `xwayland-satellite`, if it is
  installed. InfinityDesk claims a free X display for the programs it starts (`DISPLAY`).

## Controls (provisional)

Mod is **Super** in the standalone session and **Alt** while running nested (like niri does).
Keyboard shortcuts can be changed in the config (see [Settings](#settings)); mouse controls
will follow.

| Action                          | Input                                              |
|---------------------------------|----------------------------------------------------|
| Pan the canvas                  | Left-drag or scroll on empty space                 |
| Zoom around the pointer         | Mod + scroll                                       |
| Touchpad: zoom                  | Pinch (anywhere)                                   |
| Touchpad: pan the canvas        | Three-finger swipe (anywhere)                      |
| Touchpad: overview              | Four-finger swipe up (open) / down (close)         |
| Move a window and its cluster   | Mod + left-drag on a window                        |
| Dock windows                    | Drag a window close to another one                 |
| Detach a window from a cluster  | Mod + left-click, hold still briefly, then drag    |
| Mark / unmark a window          | Mod + Shift + click                                |
| Mark windows with a box         | Mod + Shift + drag on empty space                  |
| Detach marked windows together  | Hold and drag one of them (as above)               |
| Clear marks                     | Click on empty space                               |
| Resize (neighbors follow)       | Left-drag on a window border (the edges light up)  |
|                                 | or Mod + right-drag on a window (nearest edges)    |
| Resize, rest of cluster moves   | The same with Shift                                |
| Focus the next window           | Mod + arrow keys                                   |
| Swap with the neighbor          | Mod + Ctrl + arrow keys                            |
| Half / full width or height     | Mod + Shift + arrow keys (moves the right or       |
|                                 | bottom edge: right and down grow, left and up      |
|                                 | shrink)                                            |
| Pan the view                    | Mod + Ctrl + Shift + arrow keys                    |
| Zoom in / out (screen middle)   | Mod + Plus / Mod + Minus                           |
| Center the active cluster       | Mod + C                                            |
| Overview (all windows)          | Mod + Tab or Mod + O; again to go back, or to the  |
|                                 | window chosen meanwhile (click or Mod + arrow)     |
| Recently used windows           | Alt + Tab (hold Alt; each Tab goes one further     |
|                                 | back, the view follows; release to choose)         |
| Open a terminal (kitty)         | Mod + Enter or Mod + T                             |
| App launcher (fuzzel)           | Mod + Space                                        |
| Close the focused window        | Mod + Q                                            |
| Arrange the cluster (templates) | Mod + R, then click a slot or press 1–5 (Esc)      |
| Fullscreen on / off             | Mod + F                                            |
| Switch to a text console        | Ctrl + Alt + F1 … F12 (standalone)                 |
| Quit InfinityDesk               | Mod + Shift + E                                    |

While nested, Mod + Tab and Alt + Tab are the same keys, and the host compositor may keep some
shortcuts for itself (niri keeps Alt + Tab and the three- and four-finger gestures). That is
why the overview is on Mod + O too. To try the recently used windows nested:
`desk.bind("Ctrl+Tab", desk.recent_windows)`.

## Settings

Settings are read from `~/.config/infinitydesk/init.lua` (Lua 5.4; respects
`$XDG_CONFIG_HOME`, and `INFINITYDESK_CONFIG` points to another file). Saving the file applies
it right away, no restart needed. Everything is optional; these are the defaults:

```lua
local desk = require("infinitydesk")

desk.input {
  keyboard = {
    -- xkb names; empty means the system layout (`localectl set-x11-keymap`, else US),
    -- e.g. layout = "de", options = "caps:escape"
    xkb = { layout = "", variant = "", model = "", rules = "", options = nil },
    repeat_delay = 200, -- milliseconds before a held key repeats
    repeat_rate = 25,   -- repeats per second
  },
  -- Standalone only (nested, the host compositor sets up touchpad and mouse)
  touchpad = {
    tap = true,              -- tapping clicks
    natural_scroll = true,   -- content follows the fingers
    dwt = true,              -- off while typing
    accel_speed = 0.0,       -- -1 (slow) to 1 (fast)
    accel_profile = "adaptive", -- or "flat"
  },
  mouse = {
    natural_scroll = false,  -- the wheel scrolls the other way round
    left_handed = false,     -- left and right buttons swapped
    accel_speed = 0.0,       -- -1 (slow) to 1 (fast)
    accel_profile = "adaptive", -- or "flat" (the pointer moves as far as the mouse)
  },
}

desk.canvas { width = 20000, height = 12000, grid = { spacing = 100 } }

desk.layout { gaps = 16 } -- space between windows and around the screen

desk.snapping {
  threshold = 15, -- screen pixels within which edges snap together
  hold_ms = 300,  -- hold a window still this long to pull it out of its cluster
}

-- "#rrggbb" or "#rrggbbaa"; accent is used for frames, lit edges and the template menu
desk.colors { background = "#1a1a1f", grid = "#474752", accent = "#59a6ff" }

-- Shortcuts: the built-in ones (see Controls) stay unless you change or remove them.
-- Modifiers: Mod (Super, or Alt while nested), Super, Alt, Ctrl, Shift. Keys are xkb names
-- in any case ("T", "Return", "F11", "space", "Print", ...). Shift+Q means the Q key.
-- Actions: desk.spawn("command line"), desk.close_window, desk.fullscreen, desk.layout_menu,
-- desk.focus(dir), desk.swap(dir), desk.resize(dir), desk.pan(dir) with dir "left", "right",
-- "up" or "down", desk.zoom_in, desk.zoom_out, desk.center, desk.overview, desk.recent_windows,
-- desk.quit
desk.bind("Mod+B", desk.spawn("firefox"))
desk.bind("Mod+L", desk.focus("right"))
desk.unbind("Mod+Tab")

-- Programs started once with InfinityDesk (after the one given with -c, or the terminal)
desk.spawn_at_startup("awww-daemon")
desk.spawn_at_startup("waybar")

-- Environment variables for every program started from InfinityDesk
desk.env { QT_QPA_PLATFORM = "wayland", MOZ_ENABLE_WAYLAND = "1" }

-- Pointer theme and size (also passed to programs as XCURSOR_THEME / XCURSOR_SIZE)
desk.cursor { theme = "Adwaita", size = 24 }
```

A mistake (a typo in a setting name, a wrong type, a value out of range) is logged with the
setting, file and line. At startup the defaults are used instead; while running, the previous
settings stay. If the canvas gets smaller, clusters outside it move back in as a whole.

## Roadmap

- **More Lua configuration:** window rules and events, designed to feel familiar when coming
  from niri or Hyprland. Maybe a converter for niri configs later.
- **Final default controls**, configurable mouse bindings and gestures
- **Standalone session:** several monitors, monitor settings
- Visual polish: cluster outlines, decorations, animations

## Development

```sh
cargo run                            # nested, with a kitty terminal inside
cargo test                           # the pure logic (canvas, clusters, resize, layouts, config)
```

Started inside a session (`WAYLAND_DISPLAY` or `DISPLAY` set), InfinityDesk opens as a window
there. Started from a text console, it takes over the screen and the input devices:
`target/release/infinitydesk 2> ~/infinitydesk.log`. It uses the first graphics card with a
monitor connected (`INFINITYDESK_DRM_DEVICE=/dev/dri/cardN` picks one). The pointer image comes
from `XCURSOR_THEME` and `XCURSOR_SIZE`.

## Code overview

| File              | Purpose                                                       |
|-------------------|---------------------------------------------------------------|
| `src/canvas.rs`   | Canvas, viewport and zoom (pure logic, tested)                |
| `src/cluster.rs`  | Cluster membership, docking and snapping (pure logic, tested) |
| `src/resize.rs`   | Resizing inside a cluster (pure logic, tested)                |
| `src/layout.rs`   | Layout templates and their menu (pure logic, tested)          |
| `src/arrange.rs`  | Applying templates and fullscreen to windows                  |
| `src/navigate.rs` | Keyboard navigation and the overview (direction search tested)|
| `src/grabs.rs`    | Pointer drags: move, detach, resize, select, pan              |
| `src/input.rs`    | Keyboard shortcuts and mouse handling                         |
| `src/render.rs`   | What gets drawn: windows, layers, dot grid, outlines          |
| `src/state.rs`    | Compositor state                                              |
| `src/config.rs`   | Settings from the Lua config (tested)                         |
| `src/winit.rs`    | Nested backend                                                |
| `src/udev.rs`     | Standalone backend: screen, GPU, input devices, consoles      |
| `src/cursor.rs`   | The pointer image (standalone)                                |
| `src/xwayland.rs` | X11 programs through xwayland-satellite                       |
| `src/handlers/`   | Wayland protocol handlers                                     |

## License

GPL-3.0, see [LICENSE](LICENSE).
