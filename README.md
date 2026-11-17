# InfinityDesk

A Wayland compositor built around a huge, bounded canvas. Place windows anywhere on it,
dock them to each other to form clusters, and pan around (zoom is coming).

> **Status:** early development. It currently runs *nested* as a window inside your existing
> Wayland session (e.g. niri, Hyprland), which is the safe way to try it.

## Ideas

- **Canvas:** a very large (but finite) 2D surface; your screen is a viewport onto it.
- **Clusters:** windows snap together edge to edge (like Windows Snap) and form groups that move together.
  A quick yank pulls a window out of its cluster.
- **Configuration in Lua**, designed to feel familiar when coming from niri or Hyprland (planned).

## Building and running

Requires Rust and the usual Wayland libraries (`wayland`, `libxkbcommon`, `libinput`, `mesa`).

```sh
cargo run                 # starts nested with an alacritty terminal inside
cargo run -- -c kitty     # start a different program inside
```

## Controls (provisional)

| Action                        | Input                               |
|-------------------------------|-------------------------------------|
| Pan the canvas                | Left-drag on empty space            |
| Move a window                 | Alt + left-drag on a window         |
| Move a whole cluster          | Alt + right-drag on a window        |
| Dock windows                  | Drag a window close to another one  |
| Detach a window from cluster  | Alt + left-drag it out (pull past the resistance) |
| Open a terminal (alacritty)   | Alt + T                             |
| App launcher (fuzzel)         | Alt + D                             |
| Close the focused window      | Alt + Q                             |

Alt is the modifier while running nested (like niri does); standalone it will be Super.
Controls will become configurable.

## License

GPL-3.0, see [LICENSE](LICENSE).
