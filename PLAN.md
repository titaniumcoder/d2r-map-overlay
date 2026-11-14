# D2R map overlay — Rust, corner minimap

## Context
D2RMH (2022) can't read current D2R (page-encrypted code, moved offsets, hidden map seed).
Live tests on 2026-11-14 (online char BigWolf, build ~3.3.x, Proton prefix `~/Games/battlenet/prefix`)
proved a seed-less route works online:
- The client holds the **full room layout of a level from entry** (verified Tower 1–5, Outer Cloister,
  Barracks 44 rooms, Jail 1; every recorded position lay inside recorded rooms).
- **Markers** (waypoints, exits, superunique/special spawns) are only populated for rooms within
  ~100–200 tiles of the player; they appear as you explore (town and small fixed levels are effectively complete).
- **Monsters** with elite flags are readable (only those near the player).

The user wants: **layout, waypoint, entry and exit, packs, uniques, superuniques** — no normal monsters.
Stack: **Rust, single binary**, drawing a **corner minimap** in the game's 45° view.
Phase 2 (separate, later): recover the map seed → full markers from entry via d2mapapi.

## Project & working setup
Everything lives in `~/Work/d2r_map_overlay/` (Python probes already in `research/`; they document every offset).
The plan is written so work continues **from that directory** in a fresh session:
- `PLAN.md` — this plan (copied verbatim), milestone checkboxes updated as work lands.
- `CLAUDE.md` — standing context for sessions started there: D2R/Proton setup, verified offsets (pointer to PLAN.md),
  tooling rules below, single-screen test protocol (instructions first, background recording, wait for "done"),
  `setcap` step, and "never run docker/sudo yourself — ask the user".
- `.gitignore` (`target/`, captured fixtures over a size limit).
Rust via mise (`mise use rust@stable` pinned in the project's `mise.toml`). Implementation via the `rust-sys` agent.

## Tooling (cargo only, strict)
- Build/test/run only through **cargo**: `cargo build --release`, `cargo test`, `cargo run -- --dump`.
- **Strict lints in `Cargo.toml`**:
  ```toml
  [lints.rust]
  unsafe_code = "forbid"          # no unsafe needed: /proc/<pid>/mem via std::fs::File + read_at
  missing_debug_implementations = "warn"
  unused_must_use = "deny"
  [lints.clippy]
  all = { level = "deny", priority = -1 }
  pedantic = { level = "deny", priority = -1 }
  nursery = { level = "warn", priority = -1 }
  unwrap_used = "deny"
  expect_used = "warn"
  panic = "deny"
  indexing_slicing = "warn"        # memory parsing must use checked access
  cast_possible_truncation = "deny"
  ```
  (If a dependency's API forces `unsafe`, it stays confined to one module with a documented `#[allow]`.)
- **Gate before every commit**: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test`.
  Clippy is the guard against invented APIs: code that doesn't compile/lint clean doesn't get committed.
  The `reviewer` agent runs this gate and reviews the diff before each commit.
- `rustfmt.toml` with `max_width = 110`; `rust-toolchain` pinned via mise.

## Git
- Step 0: `git init` in `~/Work/d2r_map_overlay`, first commit = research probes + PLAN.md + CLAUDE.md + cargo skeleton.
- **Small, regular commits**: at least one per milestone sub-step (e.g. "process: find D2R pid", "image: memfd alias + sig scan"),
  each passing the gate. Conventional, descriptive messages; no push (no remote) unless asked.

```
Cargo.toml
src/main.rs        — arg/config, main loop (reader thread → channel → overlay)
src/process.rs     — find D2R pid, maps parsing, pread on /proc/<pid>/mem
src/image.rs       — locate D2R image (live base + fully readable memfd alias), signature scan
src/game.rs        — structs/offsets → Snapshot {level, player, rooms, markers, elites}
src/markers.rs     — classify presets (waypoint/exit/superunique), per-game accumulation cache
src/overlay.rs     — layer-shell window, tiny-skia drawing, iso projection
src/hypr.rs        — Hyprland socket2: show only while D2R is the active window
```
Crates: `smithay-client-toolkit` (layer-shell, shm), `tiny-skia`, `serde`+`toml` (config), `anyhow`. No async runtime.

## Memory layer (verified offsets; keep in one `offsets` module)
- **Process**: `/proc/*/cmdline` contains `Diablo II Resurrected\D2R.exe`.
- **Image**: two mappings of the same memfd inode with `SizeOfImage≈0x279c000`: the live view
  (has `---s` encrypted pages; pointers refer to its base) and a fully readable `rw-s` alias → scan the alias,
  compute addresses from the live base.
- **Unit hash table**: sig `48 03 C7 49 8B 8C C6`, `rel32` at sig+7, table = liveBase + rel. 128 slots × 8 per unit type
  (0 player, 1 monster). Chain next `+0x150` (verified for monsters; verify for players).
- **Player**: type 0 with `act(+0x20)≠0` and path pos ≠ 0; optional `player_name` in config to disambiguate
  party members (PlayerData `+0x10`, name at +0).
- **Unit**: `+0x04` txt, `+0x08` id, `+0x0C` mode (0/12 dead), `+0x10` unitData, `+0x38` path (x `+0x02`, y `+0x06`, room1 `+0x20`).
- **Level**: room1 `+0x18` → room2; room2 `+0x90` → level; level `+0x1F8` id, `+0x10` first room2.
- **Room2**: `+0x48` next, `+0x58` room1 (loaded), `+0x60/64/68/6C` x/y/w/h in room units (×5 = tiles),
  `+0x78` room-tile list (entry `+0x00` Room2 of the linked level, `+0x08` next), `+0x98` presets.
- **PresetUnit**: `+0x04` txt, `+0x08` x (room-relative), `+0x10` next, `+0x20` type (1 monster, 2 object, 5 tile/exit), `+0x24` y.
- **MonsterData** (unitData): `+0x18` nameSeed, `+0x1A` flag byte (observed 0x08, 0x0C on elites) — bit meaning
  to confirm in M1 against known superunique (Countess/Smith) and champions.

## Markers
- **Waypoint**: object ids table (119, 157 seen; full list from objects.txt waypoint rows).
- **Exits**: type-5 presets + room-tile links (link target level id → label "→ Jail Level 2").
  Entry = the one nearest the arrival position.
- **Superuniques/specials**: type-1 presets (853/757 Tower 5, 771 Smith…) + live monsters with superunique flag.
- **Packs/uniques**: live monsters with unique/champion flag (minions clustered to their leader); dead skipped.
- **Accumulate per game**: markers persist in a cache keyed by (levelId, x, y) once seen, until the game (act ptr / seed) changes — so explored exits/waypoints stay on the map.

## Item drops (added 2026-11-14, user request)
- Show **important drops on the ground**: unique, set, gems, runes. Nothing else.
- Ground items are units of type 4 in the same unit hash table (slot block 4). On the ground = item mode to confirm live
  (classic D2: 3 = on floor). Like monsters, the client only knows items near the player — fine for drops.
- Quality from ItemData (unitData `+0x10`; classic D2 quality at ItemData+0x00: 5 set, 7 unique); gems/runes by base
  item txt id ranges (from items/misc txt). All offsets/ids **to verify live** before use.
- Drawn as distinct icons (unique gold, set green, rune orange, gem by colour), dropped from the map once picked up.

## Overlay
- Layer-shell surface, layer `overlay`, anchored top-right, default 400×400 (config), **empty input region** (click-through), no keyboard.
- Draw current level only: room footprints (loaded rooms brighter), player dot, waypoint (blue), exits (purple + target name),
  superuniques (gold + name if known), champion/unique packs (red), at ~5 Hz from a reader thread.
- Iso projection `sx = x − y, sy = (x + y)/2`, scaled to fit the level bounds.
- Visible only while the Hyprland active window class is `steam_app_battlenet` with title `Diablo II: Resurrected`.

## Permissions
Reading `/proc/<pid>/mem` needs `CAP_SYS_PTRACE` (ptrace_scope=1). After each release build the user runs
`sudo setcap cap_sys_ptrace=ep target/release/d2r-map-overlay` (a `just`/Makefile target). No sudo at runtime.

## Milestones
- [x] **M0 setup**: mise Rust pin, `cargo init`, lint config, rustfmt, PLAN.md + CLAUDE.md + .gitignore, `git init`, first commit.
   From here on the user can start sessions directly in `~/Work/d2r_map_overlay`.
- [ ] **M1 reader** (a: process/image ✓, b: game snapshot + --dump): `d2r-map-overlay --dump` prints one JSON snapshot. Validate against `research/rooms.py` / `presets.py` output in Jail 1 and Tower 5; confirm player chain `+0x150` and the elite flag bits.
- [ ] **M2 overlay**: draw a static snapshot file (no game needed) — layer-shell, click-through, iso render.
- [ ] **M3 live**: reader thread + Hyprland focus hide/show; ~5 Hz redraw.
- [ ] **M4 markers**: classification tables, per-game accumulation, exit labels.
- [ ] **M4b item drops**: verify item unit/quality offsets live, show unique/set/gem/rune drops on the minimap.
- [ ] **Cleanup**: remove D2RMH exe/launcher (`~/.local/bin/d2rmh`); keep classic 1.13c files for phase 2.
- [ ] **Phase 2 (separate plan)**: map seed RE → d2mapapi-backed full markers from entry.

## Verification
- Unit tests: iso projection, preset classification, marker cache, snapshot parsing from a captured byte fixture.
- Live (single-screen protocol: instructions first, background recording, user says "done"):
  Tower 5 (Countess marker at entry), Barracks (Smith appears when near, stays after leaving), Jail 1
  (waypoint + both exits), plus a champion pack — compare overlay screenshot with the game's Tab automap.
