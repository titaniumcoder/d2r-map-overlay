# d2r-map-overlay

A map tool for **Diablo II: Resurrected** (D2R) on **Linux**.

It reads the running game's memory (read-only) and draws a small, click-through minimap in the corner of
the screen. The minimap shows:

- **Layout:** the level layout, waypoints, entrances and exits (including the arrows on open level borders),
  and your trail.
- **Monsters:** superuniques, bosses, champion and unique packs.
- **Landmarks:** quest spots such as the Summoner, the real Tal Rasha tomb's orifice, the Horadric Cube,
  Radament and the Cairn Stones.
- **Shrines and wells, and valuable drops.**
- **Info line** under the map: area, monster level (coloured by the build's damage types), terror.
  The breakpoint guide (cast/hit-recovery frames, resistances), level suggestions, gear and
  keep / sell / give-away lists live on the web dashboard (`/`, served by the overlay's localhost
  web app; `/api/live`). The overlay hides in towns.

## ⚠️ Use at your own risk: it could get your account banned

Reading the game's memory is against Blizzard's terms of service. **Using this tool could get your
Battle.net account banned.** You use it entirely at your own risk; **I am not responsible** for any
ban, suspension or other consequence.

## Linux only. It does not work on Windows

This tool is built for one specific setup:

- D2R running under **Proton/Wine** on Linux. The game process is found through `/proc`.
- The **Hyprland** Wayland compositor. The overlay is a `wlr-layer-shell` surface, and it is shown only
  while D2R has focus, which it learns from Hyprland's IPC socket.
- Reading `/proc/<pid>/mem`, which needs the `CAP_SYS_PTRACE` capability on the binary.

**It will not work on Windows,** and it is untested on other compositors. If you want it on Windows or
anywhere else, you are free to try: fork it and send a pull request.

It is **free to use as is**, with no warranty and no support promise. Memory offsets change with game
patches; the ones here were found on D2R build 3.x in November 2026.

## Building and running

Requirements: Rust (see `mise.toml`), a Wayland session on Hyprland, and D2R running under Proton.

```sh
cargo build --release
# Reading another process's memory needs ptrace rights (ptrace_scope=1):
sudo setcap cap_sys_ptrace=ep target/release/d2r-map-overlay
target/release/d2r-map-overlay            # live overlay
target/release/d2r-map-overlay --dump     # print one JSON snapshot of the game state
```

### Hiding the map with a key

The overlay takes no keyboard input (it is click-through), so it toggles on `SIGUSR1`: each signal
hides or shows the map, while the reader and the web dashboard keep running. Every start is visible.
Bind a compositor key that the game never sees, for example SUPER+CTRL+M:

```sh
pkill -USR1 -x d2r-map-overlay      # the command to bind
```

- Omarchy (`~/.config/hypr/bindings.lua`):
  `o.bind("SUPER + CTRL + M", "Toggle D2R map", "pkill -USR1 -x d2r-map-overlay")`
- Plain `hyprland.conf`: `bind = SUPER CTRL, M, exec, pkill -USR1 -x d2r-map-overlay`

`-x` matches the process name exactly (`d2r-map-overlay` is exactly the 15-character limit of a
process name). Don't use `pkill -f`, which matches any command line containing the text.

Optional configuration lives in `~/.config/d2r-map-overlay/config.toml`. See
[`config.example.toml`](config.example.toml) for:

- which valuables are highlighted
- which shrines are shown
- per-character builds for the area colouring
- `farm.toml`: a cross-character farm list for a planned build (the dashboard's Farm list panel)

## How it works (short)

- Finds the D2R process and a fully readable alias of its (partly encrypted) image.
- Locates the unit hash table by signature, then walks it: player → path → room → level → rooms and
  presets, monsters, objects and items.
- All verified offsets and research notes are in [`PLAN.md`](PLAN.md). The Python probes in `research/`
  are how they were found.

## License

[MIT](LICENSE).
