# d2r-map-overlay

A map tool for **Diablo II: Resurrected** (D2R) on **Linux**.

It reads the running game's memory (read-only) and draws a small, click-through minimap in the corner of
the screen. The minimap shows:

- **Layout:** the level layout, waypoints, entrances and exits (including the arrows on open level borders),
  and your trail.
- **Monsters:** superuniques, bosses, champion and unique packs.
- **Landmarks:** quest spots such as the Summoner, the real Tal Rasha tomb's orifice, the Horadric Cube,
  Radament and the Cairn Stones.
- **Shrines and wells, and marked drops:** a green ring for what your own builds need (a runeword
  base with the right sockets, better than the one you hold; a build unique or set), a small star on
  it for a still-missing farm-list item; red / orange / yellow rings for trade value (top, second,
  third tier). Dashed: not sure yet (an unidentified unique, a superior base's unknown enhanced
  defense or damage).
- **Info line** under the map: area, monster level (coloured by the build's damage types), terror.
  The breakpoint guide (cast/hit-recovery frames, resistances), level suggestions, gear and
  keep / sell / give-away lists live on the web dashboard (`/`, served by the overlay's localhost
  web app; `/api/live`). The overlay hides in towns.
- **Sounds**: the overlay plays them through the speakers (`pw-play`, else `paplay` or `aplay`),
  whether or not the dashboard is open: a sound whenever a drop the minimap marks with a ring
  appears (user: "In principle whenever something drops on the map that you mark, then you make
  a sound. much simpler."). Louder for a farm-list item, a short quiet tone for any other marked
  drop; never for gems and skulls (the game sounds those itself). Each drop plays once; nothing
  sounds on the first frame after the overlay starts, a menu return or a character change. The
  🔔 in the dashboard's top bar switches them all; the ▾ beside it picks which of the two play
  (▶ plays one to try it).

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

`scripts/deploy.sh` does the build, `setcap` (`sudo -n`: run `sudo -v` first) and a restart in one
go, with the playstyle files read from a snapshot (`~/.local/share/d2r-map-overlay/playstyles-deployed/`
via `D2R_PLAYSTYLES_DIR`), so editing `playstyles/` doesn't hot-reload into the running overlay.

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
[`config.example.toml`](config.example.toml) for which shrines are shown and which playstyle runs.

### Playstyles

The leveling and farming strategy (builds and their stages, farm list, which drops get a mark) is a
**playstyle**: a folder under [`playstyles/`](playstyles/). The repo ships one as the example,
[`playstyles/summoner-fury/`](playstyles/summoner-fury/) (Summoner Druid, switching to Fury Druid from
level 75):

- `playstyle.toml`: name, description, which valuables tiers are highlighted, damage types for the
  area colouring
- `builds/*.toml`: the build definitions (generated by `research/gen_build.py` from the
  `*.source.json` next to them); `merc_acts = [2, 1]` (acts 1, 2, 3, 5, primary first; the
  single `merc_act = N` still works; default `[2]`) names the acts of the build's mercenaries,
  which decide the weapons its `[merc_slots]` take (each runeword goes to the first act that can
  carry it; Act 1: bows and amazon bows; Act 5: swords, one-handed, with a second sword in
  `offhand`, and barbarian helms); gear any listed act can use is kept
- `farm.toml`: a cross-character farm list for a planned build (the dashboard's Farm list panel).
  Each entry is `{ name, kind, count?, merc?, slot?, note? }` with `kind = "runeword" | "unique" |
  "set"` (counted by name), or `kind = "rolled"` with `slot` and `needs` (skill names given as an
  oskill or as charges), for example
  `{ name = "Teleport Amulet", kind = "rolled", slot = "amulet", needs = ["Teleport"], note = "until Enigma" }`:
  done once such an item is in the shared stash, found while the character has one; an entry
  needing Teleport is hidden once Enigma is held. An unknown `needs` name fails the file.
- `route.toml`: the route's bosses (`bosses = [...]`): the farm list's "where:" line names
  those that drop a missing entry
- `valuables.json`: the drop mark rules (compiled in)
- `loot-filter.json`: a D2R in-game loot filter profile; the `/config` page's "Copy filter JSON"
  button copies it (served at `/api/loot-filter`) for import in the game. `filter_name` in
  `playstyle.toml` is the profile `name` it should have (a mismatch is warned about at load)

`playstyle = "<folder>"` in `config.toml` picks one (default `summoner-fury`). The folder is read
at run time from `playstyles/` at the root of the source tree the binary was built in, else from `playstyles/`
next to the binary; set `D2R_PLAYSTYLES_DIR` to the folder holding the playstyle folders to use
another place (then only that one is searched). If none is found, the overlay prints an `error:`
line on start and runs without builds, stages or farm list (unless your own or old files provide
them). To change a file
without touching the repo, copy it to `~/.config/d2r-map-overlay/playstyle/` under the same relative
path (for example `playstyle/farm.toml` or `playstyle/builds/fury-druid.toml`): a file there replaces
the shipped one. The build chain each of your characters follows is chosen on the dashboard's
`/config` page (it lists the characters the overlay has seen you play) and saved in `~/.local/share/d2r-map-overlay/bis.toml`. The page's "Money farming" switch (marks and
stash keeps for the wands, scepters, sorceress orbs and daggers vendors pay well for) is saved in
`~/.local/share/d2r-map-overlay/settings.toml`.

Older setups kept these in the config folder (`[valuables]`, `[builds]` and `[bis]` in
`config.toml`, `builds/` and `farm.toml` next to it). They are still read and win over the shipped
files; the overlay names them on start. Once they match the shipped ones, delete them (or move the
files to `playstyle/` to keep them as your overrides).

## How it works (short)

- Finds the D2R process and a fully readable alias of its (partly encrypted) image.
- Locates the unit hash table by signature, then walks it: player → path → room → level → rooms and
  presets, monsters, objects and items.
- All verified offsets and research notes are in [`PLAN.md`](PLAN.md). The Python probes in `research/`
  are how they were found.

## Code structure

A Cargo workspace. The overlay itself is the **core**; how the game is read and what the player's
strategy makes of it are **plugins**, separate crates picked at build time with Cargo features:

| Crate | Path | What it holds |
| --- | --- | --- |
| `d2r-map-overlay` (binary) | `src/main.rs` | Command line, wires one reader and one playstyle into the core |
| `d2r-core` | `crates/core/` | Snapshot model, game data tables, map renderer, overlay window, session, journal, dashboard core, and the plugin traits `GameReader` and `Playstyle` |
| `d2r-reader-mem` | `crates/reader-mem/` | Reads the game's memory over a platform layer; the Proton/Linux platform behind the `proton` feature |
| `d2r-playstyle-config` | `crates/playstyle-config/` | The playstyle read from a `playstyles/<name>/` folder: builds, farm list, route, item marks, its dashboard panels |

The binary's features are `reader-proton` and `playstyle-config` (both on by default). The plugins
depend on the core and never on each other.

```sh
cargo fmt --all --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace
```

### Adding a playstyle

Most changes need no code: copy `playstyles/summoner-fury/` to `playstyles/<your-name>/`, edit its
files and set `playstyle = "<your-name>"` in `config.toml`. Write code only for new logic:

1. Add a crate under `crates/` that depends on `d2r-core` and implements
   `d2r_core::playstyle::Playstyle` (`crates/playstyle-config/src/config_driven.rs` is the example:
   `evaluate` returns the `Guidance` the map and dashboard show, `mark_drops` the ground-drop
   rings, `panels` its dashboard sections).
2. In the root `Cargo.toml`, add it as an optional dependency with a feature, e.g.
   `playstyle-mine = ["dep:d2r-playstyle-mine"]`.
3. In `src/main.rs`, point `ThePlaystyle` at it under that feature (and adjust the
   `compile_error!` guard), then build with
   `cargo build --release --no-default-features --features reader-proton,playstyle-mine`.

### Adding a reader

A new operating system usually needs only a new platform layer: implement
`d2r_reader_mem::Platform` (find the process, open its memory, locate the image, list heap regions,
find the save directory; `crates/reader-mem/src/proton.rs` is the example, the notes for Windows
are in `crates/reader-mem/src/lib.rs`) behind a new feature of `d2r-reader-mem`, and use
`MemoryReader<YourPlatform>`. A reader that works differently altogether is its own crate
implementing `d2r_core::reader::GameReader` and filling a `d2r_core::game::Snapshot`; wire it in
like a playstyle (`TheReader` in `src/main.rs`, a feature in the root `Cargo.toml`).

## License

[MIT](LICENSE).
