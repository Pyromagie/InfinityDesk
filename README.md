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
- wlr-layer-shell, so launchers like fuzzel (and bars or wallpaper tools) work
- Optional setting: docked windows take their neighbor's height or width (clean grid)

## Roadmap

- **Cluster focus:** jump the viewport to a cluster so its top-left window sits in the
  top-left corner of the screen, zooming out if the cluster does not fit
- **New windows dock to the focused cluster** (right of the last used window, or the next
  free side) instead of opening in the middle of the screen
- **Lua configuration** in `~/.config/infinitydesk/init.lua`, designed to feel familiar when
  coming from niri or Hyprland: settings first, then keybindings, window rules and events.
  Maybe a converter for niri configs later.
- **Final default controls**, configurable bindings, touchpad gestures
- **Background layer** fixed to the screen behind the moving canvas (maybe with parallax)
- **Standalone session** (DRM/KMS backend, own login entry) with Super as the modifier,
  hardware-accelerated client buffers (dmabuf), multi-monitor support
- Visual polish: cluster outlines, decorations, animations

## Building and running

Requires Rust (edition 2024) and the usual Wayland libraries (`wayland`, `libxkbcommon`,
`libinput`, `mesa`). The default terminal is `alacritty` and the default launcher is `fuzzel`.

```sh
cargo run                         # starts nested with an alacritty terminal inside
cargo run -- -c kitty             # start a different program inside
XKB_DEFAULT_LAYOUT=de cargo run   # keyboard layout (until the config exists)
```

## Controls (provisional)

Alt is the modifier while running nested (like niri does); standalone it will be Super.
All controls will become configurable.

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
| Open a terminal (alacritty)     | Alt + T                                            |
| App launcher (fuzzel)           | Alt + D                                            |
| Close the focused window        | Alt + Q                                            |

### Settings

Until the Lua config exists, settings are switched with environment variables:

| Variable                     | Effect                                                                 |
|------------------------------|------------------------------------------------------------------------|
| `INFINITYDESK_MATCH_SIZE=1`  | A docked window takes its neighbor's height or width (clean grid)      |

## Code overview

| File              | Purpose                                                       |
|-------------------|---------------------------------------------------------------|
| `src/canvas.rs`   | Canvas, viewport and zoom (pure logic, tested)                |
| `src/cluster.rs`  | Cluster membership, docking and snapping (pure logic, tested) |
| `src/resize.rs`   | Resizing inside a cluster (pure logic, tested)                |
| `src/grabs.rs`    | Pointer drags: move, detach, resize, select, pan              |
| `src/input.rs`    | Keyboard shortcuts and mouse handling                         |
| `src/render.rs`   | What gets drawn: windows, layers, dot grid, outlines          |
| `src/state.rs`    | Compositor state                                              |
| `src/config.rs`   | Settings                                                      |
| `src/winit.rs`    | Nested backend                                                |
| `src/handlers/`   | Wayland protocol handlers                                     |

## License

GPL-3.0, see [LICENSE](LICENSE).
