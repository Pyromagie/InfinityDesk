# InfinityDesk

A Wayland compositor (window manager) built around a **huge, bounded canvas**. Instead of
workspaces or a fixed screen, your windows live on one large 2D surface. You place them
anywhere, dock them to each other to form **clusters**, and pan and zoom across the canvas to
get around. Think of a desk you can spread your work out on, with the screen as a camera above it.

Inspired by [niri](https://github.com/YaLTeR/niri) (freedom to scroll horizontally), taken one
step further into two dimensions. Built in Rust on [Smithay](https://github.com/Smithay/smithay).

> **Status: early development.** InfinityDesk currently runs *nested* as a window inside an
> existing Wayland session (e.g. niri, Hyprland, Sway, GNOME, KDE). That is the safe way to try
> it; running it as your real session comes later.

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

## What works today

- Nested Wayland compositor (winit backend) with xdg-shell windows, popups and menus,
  keyboard and pointer input, clipboard and drag and drop between apps
- Canvas with a dot grid, panning and zooming around the pointer (drawn on the GPU, only
  visible windows are rendered)
- **Docking:** edges snap together within 15 screen pixels at any zoom level and line up flush
  (tops or bottoms side by side, left or right edges stacked, whichever is closer), so windows
  of different sizes dock too and there are no offset connections. Pushing a window a bit too
  far, onto its neighbor, still docks it at the edge. Spots that are
  already taken are skipped, and windows passing by quickly do not snap.
- **Clusters** move as a whole, split automatically when a window leaves, and merge when they
  touch another window
- **Detaching:** hold a window still for 0.3 s (a blue pulse shows it is unlocked), then drag
  it out of its cluster
- **Marking:** mark several windows (single clicks or a selection box) and pull them out of
  their cluster together; they stay connected among themselves
- **Cluster resize:** resizing a window takes its neighbors along, so the cluster stays closed.
  With Shift, the rest of the cluster moves instead. Grab any window border (a generous zone
  around it shows resize arrows and lights up the edges that will move), or use Alt + right-drag.
  The dragged edge snaps to the edges of other visible windows, so they line up exactly.
- **Layout templates:** Alt + R opens a menu of templates (half | half, a half and two
  quarters, three columns, four quarters, ...). Picking one arranges the active window's
  cluster so it fills exactly one screen, shown at 1:1. Click the slot the active window should
  go to (the other windows take the slots closest to where they are), or press the template's
  number. Slots left over are filled by the next windows you open.
- **New windows dock to the active window** (right of it, else below, left, above; the view
  follows if there is no room on the screen). Click empty space first for a free window.
- **Fullscreen:** Alt + F (or the app itself, e.g. F11 in a browser) lets a window cover the
  screen; its place in the cluster and the view are restored afterwards
- **Keyboard navigation:** move the focus to the next window in a direction (also into the
  next cluster, the view follows), swap a window with its neighbor, resize in steps (the
  cluster stays closed), pan and zoom the view, center the active cluster, and an overview
  that zooms out until all windows fit
- wlr-layer-shell, so launchers like fuzzel, bars and wallpaper tools (e.g. awww, swaybg) work;
  the wallpaper stays fixed behind the moving canvas
- Optional setting: docked windows take their neighbor's height or width (clean grid)

## Roadmap

- **More Lua configuration:** window rules and events, designed to feel familiar when coming
  from niri or Hyprland. Maybe a converter for niri configs later.
- **Final default controls**, configurable mouse bindings, touchpad gestures
- **Standalone session** (DRM/KMS backend, own login entry) with Super as the modifier,
  hardware-accelerated client buffers (dmabuf), multi-monitor support
- Visual polish: cluster outlines, decorations, animations

## Building and running

Requires Rust (edition 2024) and the usual Wayland libraries (`wayland`, `libxkbcommon`,
`libinput`, `mesa`). The default terminal is `alacritty` and the default launcher is `fuzzel`.

```sh
cargo run                         # starts nested with an alacritty terminal inside
cargo run -- -c kitty             # start a different program inside
```

## Controls (provisional)

Alt is the modifier while running nested (like niri does); standalone it will be Super.
Keyboard shortcuts can be changed in the config (see Settings); mouse controls will follow.

| Action                          | Input                                              |
|---------------------------------|----------------------------------------------------|
| Pan the canvas                  | Left-drag on empty space                           |
| Zoom around the pointer         | Alt + scroll                                       |
| Move a window and its cluster   | Alt + left-drag on a window                        |
| Dock windows                    | Drag a window close to another one                 |
| Detach a window from a cluster  | Alt + left-click, hold still briefly, then drag    |
| Mark / unmark a window          | Alt + Shift + click                                |
| Mark windows with a box         | Alt + Shift + drag on empty space                  |
| Detach marked windows together  | Hold and drag one of them (as above)               |
| Clear marks                     | Click on empty space                               |
| Resize (neighbors follow)       | Left-drag on a window border (the edges light up)  |
|                                 | or Alt + right-drag on a window (nearest edges)    |
| Resize, rest of cluster moves   | The same with Shift                                |
| Focus the next window           | Alt + arrow keys                                   |
| Swap with the neighbor          | Alt + Ctrl + arrow keys                            |
| Wider / narrower, taller / lower| Alt + Shift + → / ←, ↓ / ↑ (a tenth of the screen) |
| Pan the view                    | Alt + Ctrl + Shift + arrow keys                    |
| Zoom in / out (screen middle)   | Alt + Plus / Alt + Minus                           |
| Center the active cluster       | Alt + C                                            |
| Overview (all windows)          | Alt + O or Alt + Tab; again to go back, or to the  |
|                                 | window chosen meanwhile (click or Alt + arrow)     |
| Open a terminal (alacritty)     | Alt + T                                            |
| App launcher (fuzzel)           | Alt + Space                                        |
| Close the focused window        | Alt + Q                                            |
| Arrange the cluster (templates) | Alt + R, then click a slot or press 1–8 (Esc)      |
| Fullscreen on / off             | Alt + F                                            |

While nested, the host compositor may keep some shortcuts for itself (niri: Alt + Tab), which
is why the overview is on Alt + O too. Keyboard resizing moves the window's right (bottom)
edge, or its left (top) edge if only that one has a neighbor, so a row that fills the screen
keeps filling it.

### Settings

Settings are read from `~/.config/infinitydesk/init.lua` (Lua 5.4; respects
`$XDG_CONFIG_HOME`, and `INFINITYDESK_CONFIG` points to another file). Saving the file applies
it right away, no restart needed. Everything is optional; these are the defaults:

```lua
local desk = require("infinitydesk")

desk.input {
  keyboard = {
    -- xkb names; empty means the system default (e.g. layout = "de", options = "caps:escape")
    xkb = { layout = "", variant = "", model = "", rules = "", options = nil },
    repeat_delay = 200, -- milliseconds before a held key repeats
    repeat_rate = 25,   -- repeats per second
  },
}

desk.canvas { width = 20000, height = 12000, grid = { spacing = 100 } }

desk.snapping {
  threshold = 15,     -- screen pixels within which edges snap together
  match_size = false, -- a docked window takes its neighbor's height or width (clean grid)
  hold_ms = 300,      -- hold a window still this long to pull it out of its cluster
}

-- "#rrggbb" or "#rrggbbaa"; accent is used for frames, lit edges and the template menu
desk.colors { background = "#1a1a1f", grid = "#474752", accent = "#59a6ff" }

-- Shortcuts: the built-in ones (see Controls) stay unless you change or remove them.
-- Modifiers: Mod (Alt while nested, Super later), Super, Alt, Ctrl, Shift. Keys are xkb names
-- in any case ("T", "Return", "F11", "space", "Print", ...). Shift+Q means the Q key.
-- Actions: desk.spawn("command line"), desk.close_window, desk.fullscreen, desk.layout_menu,
-- desk.focus(dir), desk.swap(dir), desk.resize(dir), desk.pan(dir) with dir "left", "right",
-- "up" or "down", desk.zoom_in, desk.zoom_out, desk.center, desk.overview
desk.bind("Mod+Return", desk.spawn("alacritty -e btop"))
desk.bind("Mod+L", desk.focus("right"))
desk.unbind("Mod+Tab")
```

A mistake (a typo in a setting name, a wrong type, a value out of range) is logged with the
setting, file and line. At startup the defaults are used instead; while running, the previous
settings stay. If the canvas gets smaller, clusters outside it move back in as a whole.

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
| `src/handlers/`   | Wayland protocol handlers                                     |

## License

GPL-3.0, see [LICENSE](LICENSE).
