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
- **Unit hash table**: 6 blocks (unit types 0–5) × 128 slots × 8 bytes, in `.data` (rva 0x1ebe5f0 for build
  `6ad81202-279c000`). A unit sits in slot `id & 127`; chain next **`+0x158`** (verified live 2026-11-15 for items, monsters, objects; the earlier `+0x150` only reached slot heads, so units sharing a slot were missed).
  **D2R re-encrypts code pages after use**, so the code sig `48 03 C7 49 8B 8C C6` (rel32 at sig+7) only works
  sometimes. Lookup order: sig → per-build cache (`~/.cache/d2r-map-overlay/offsets.toml`) → structural search of
  `.data` (type + hash-slot check; needs the player in a game).
- **Player**: type 0 with `act(+0x20)≠0` and path pos ≠ 0; optional `player_name` in config to disambiguate
  party members (PlayerData `+0x10`, name at +0).
- **Unit**: `+0x04` txt, `+0x08` id, `+0x0C` mode (0/12 dead), `+0x10` unitData, `+0x38` path (x `+0x02`, y `+0x06`, room1 `+0x20`).
- **Level**: room1 `+0x18` → room2; room2 `+0x90` → level; level `+0x1F8` id, `+0x10` first room2.
- **Room2**: `+0x48` next, `+0x58` room1 (loaded), `+0x60/64/68/6C` x/y/w/h in room units (×5 = tiles),
  `+0x78` room-tile list (entry `+0x00` Room2 of the linked level, `+0x08` next), `+0x98` presets.
- **PresetUnit**: `+0x04` txt, `+0x08` x (room-relative), `+0x10` next, `+0x20` type (1 monster, 2 object, 5 tile/exit), `+0x24` y.
- **MonsterData** (unitData): `+0x18` nameSeed, `+0x1A` flag byte, classic D2 bits: 0x02 superunique, 0x04 champion,
  0x08 unique/boss, 0x10 minion. Live: minions of a killed pack = 0x10, pack leaders seen as 0x08/0x0C.
  Superunique bit (Countess/Smith) still to observe.
- **Room1** (loaded rooms only): `+0x38` → CollMap: u32 `+0x00/+0x04` world x/y, `+0x08/+0x0C` width/height
  (= Room2 rect), `+0x20` → width×height u16 flags, row-major. Bit `0x0001` = blocked (walls seen as 5/7, floor 0).
  Verified Sewers Level 1, 2026-11-14.
- **Room2 near rooms**: `+0x10` → array of Room2 pointers, **`+0x18` u32 count** (live 2026-11-15, Outer Cloister;
  `+0x20` sometimes reads 9 with trailing nulls and is a different field; the older `+0x50` u16 note is superseded).
  Includes other levels' rooms: top-row rooms of 27 had near rooms in 28, bottom-row ones in 26. Used for
  walk-through border exits (`Snapshot::border_links`, drawn as a triangle on the shared edge). Earlier note: only
  filled for loaded rooms, and no walkable-connection info for unexplored rooms (openings need the seed).

## Markers
- **Waypoint**: object ids table (119, 157 seen; full list from objects.txt waypoint rows).
- **Exits**: type-5 presets + room-tile links (link target level id → label "→ Jail Level 2").
  Entry = the one nearest the arrival position.
- **Superuniques/specials**: type-1 presets (853/757 Tower 5, 771 Smith…) + live monsters with superunique flag.
- **Packs/uniques**: live monsters with unique/champion flag (minions clustered to their leader); dead skipped.
- **Accumulate per game** (done 2026-11-15): presets, shrines, wells, kills, operated seals and arrivals are remembered per level for the game (identity = player unit id; act travel keeps it) and persisted to `~/.cache/d2r-map-overlay/game.json`, so overlay restarts keep them too.

## Arcane Sanctuary Summoner (live-verified 2026-11-15, nine games)
- Room2 `+0x40` → level-preset record, u32 at `+0x00` = preset id (lvlprest row).
- The Summoner's arm end uses a dedicated id per direction: **525 (+x / down-right), 526 (−x / up-left),
  527 (−y / up-right), 528 (+y / down-left)** — all four confirmed by the user in game. Normal arm ends use
  other ids (510, 511, 513, 517 seen). Drawn as a gold haloed star from the moment the level is entered.
- Games created in quick succession often share the layout (seed looks time-based); waiting a minute varies it.
- **Tal Rasha's real tomb** (levels 66–72): the orifice chamber is its own preset. Confirmed: 462 in tomb 72 (orifice
  object 152 inside it); a fake tomb's chest chamber was 473 in tomb 71. The level+390 guess was wrong: lvlprest
  names them "Tomb Talrasha W/E/S/N" (460–463, only in the real tomb) and "Tomb Chest W/E/S/N" (472–475);
  456–459 are "Tomb Cube W/E/S/N", the Halls of the Dead 3 cube chamber (459 confirmed with chest object 354).
  Use lvlprest names (blizzhackers/d2data lvlprest.json, `Def` = the Room2 preset id) before guessing patterns.

## Flayer Jungle (78), 2026-11-15
- 192 rooms in 9 large piece types (16 or 32 Room2 each): 534, 535, 538, 539, 565, 570, 574, 601, 602.
- **601 = waypoint area** (player and waypoint object 237 inside it). **602 = Gidbinn altar candidate** (only other
  outlier id) — **confirmed**: the Gidbinn altar (object 252) is in 602; landmark rule added.

## ActMisc, difficulty and seed lead (2026-11-15)
- Player unit `+0x20` → act; **act `+0x70` → ActMisc** (older builds: +0x78, which now points into code).
- ActMisc `+0x830` u16 = **difficulty**: 0 Normal and 1 Nightmare confirmed live (2026-11-15); Hell (2) still to see.
- Phase 2 lead: ActMisc `+0x840` and `+0x860` hold random-looking u32s (e.g. 3943813804 / 3154569827), where the
  seed-selector plugin says the generator stores {seed, 0x29A} and the derived value. Used directly as d2mapapi
  seeds they do **not** reproduce the Flayer Jungle (waypoint/altar positions differ) — likely encoded/derived.
- Monster level is **not** in the client's monster stat lists (only HP%, speed…); the info line uses the static
  levels.txt values (src/levels_data.rs, generated by research/gen_levels.py from blizzhackers/d2data).

## Superunique spawn ids (2026-11-15)
- Monster preset txt of a superunique's spawn point = **751 + hcIdx** (superuniques.txt row order index). Verified
  live for Countess 757, Smith 771, Sarina 775, Icehawk 776, Bloodwitch 762, Coldworm 766 (6/6). All 66 are in
  `SUPERUNIQUES` (src/levels_data.rs, generated by research/gen_levels.py). This replaced the earlier learning
  heuristic, which mislearned generic spawn ids (328/840 next to Witch Doctor Endugu).
- Kurast Sewers 1 (92): lever (object 367) and stairs down (366) in room preset 739 → landmark (one game).

## Item fields and valuables (2026-11-15)
- ItemData `+0x00` quality (2 normal, 4 magic, 5 set, 7 unique verified live), `+0x18` flags (0x10 identified
  verified; 0x400000 ethereal, 0x4000000 runeword = classic bits, unverified), `+0x34` unique/set row index
  (verified: Khalim's Flail = row 127, Hellforge Hammer = row 126, both identified quest items; an UNIDENTIFIED set item
  (Cleglaw's Claw, row 32) read 0 → the index is only filled once identified, so name rules on unidentified items
  stay uncertain). After identifying (Cain or scroll), the index is filled: Darkglow = unique 83, Cleglaw's Claw = set
  *ID 7 (set rows are keyed by `*ID`, not file order) — both verified live 2026-11-15; the inventory keep list relies on this., `+0x54` body slot,
  `+0x55` page (0 inventory, 4 stash, 255 elsewhere; verified over 74 items). Inventory = mode 0 + page 0.
- Item stat list: unit `+0x88` → `+0x30` entries / `+0x38` count, entry {u16 layer, u16 id, u32 value}; sockets = 194.
- Valuables: research/valuables.json (67 rules, S/A/B, trade/twink) embedded; tiers per category in
  `~/.config/d2r-map-overlay/config.toml` (default S+A both; example: config.example.toml). Ground: ring by tier
  (S red, A orange, dashed = uncertain). Inventory keep list in green below the info line, also in towns: certain
  S/A, uncertain only S. Unique/set names from src/item_names_data.rs (research/gen_item_names.py).

## Shrines and wells (2026-11-15)
- Object units: unit block 2, static path position (+0x10/+0x14). ObjectData (unit +0x10) `+0x08` u8 = shrine type
  (shrines.txt code; live: Health 2, Experience 15) — only meaningful for shrine objects (a bone pile had a stray
  value there). Mode 0 = untouched, anything else = used.
- Classification from objects.txt `OperateFn` (2 = shrine, 22 = well) → src/objects_data.rs via
  research/gen_objects.py. Chaos Sanctuary seals (392-396) are quest objects; once a seal's live object leaves
  mode 0 its star is hidden for the rest of the game. Drawn: unused shrines whose type is in config `[shrines] show` (user list: refilling,
  skill, stamina, experience, gem, monster), all wells. Hellforge (376) is a quest object.

## Terror Zones (2026-11-15)
- states.txt 189 = "desecrated" = Terror Zone state. Unit stat list (unit +0x88) holds a 256-bit state field at
  `+0xB30` (8 × u32; bit n in dword n/32). Live in the terrorized Moo Moo Farm: all 24 cows had bit 189, the player
  and the druid's wolves didn't. A level counts as terrorized when ≥ 2 living monsters carry it; the flag holds for
  the current visit (zones rotate hourly). Info line: "terrorized" in violet, table monster levels hidden.
  Confirmed by the user: during the Legacy of Terror event (13–19 Nov 2026) every Normal zone is terrorized after
  Baal (switchable per game); towns read clean. A non-terrorized combat area is still to check after the event.

## Research journal
Live mode appends deduped events to `~/.local/share/d2r-map-overlay/journal/YYYY-MM-DD.jsonl` (UTC date):
level (room pieces), arrival, marker (+ piece), link, elite monster, killed, drop. Files are never rotated;
delete old ones by hand. Use it to find more landmark patterns offline.
- **Junk: assassin traps as elites.** Traps (txt 410) carry MonsterData 0x08, so the journal logged them as elite
  `monster`/`killed` events. Commit 4a31d77 fixed this only in the renderer; the journal recorder still filters by
  flags alone (`journal::ELITE`), so txt-410 `monster`/`killed` entries are junk before *and* after 4a31d77
  (5125 of them by 2026-11-15) until the recorder skips allies too. Ignore them when analysing.
- **Cow counter** (`cows.rs`, user request 2026-11-15): in level 39 the info line shows
  `kill count: N / victims alive: M` instead of monster levels (cows = txt 391 hellbovine and 735, the Cow King's
  live unit, seen with superunique+unique flags at his preset). Lifetime total/best/runs in
  `~/.local/share/d2r-map-overlay/cows.json` (not shown yet), seeded once from the journal: runs = distinct games
  with a level-39 event (3), total/best = elite cows logged killed (2 / 1); normal cows were never logged.

## User feedback, first live session (Act 1, 2026-11-14)
- Minimap "already helps drastically"; ~1% CPU, 7 MB RSS.
- Hide in towns (level ids 1, 40, 75, 103, 109).
- Wanted corridors/openings at first, then **reverted**: the in-game automap shows rooms; the minimap should be a
  coarse orientation shape — rooms as one continuous fill, no outlines, no background panel (fully transparent).
- Waypoints missing in Act 2 → full waypoint object id set (156 = Act 2 outdoor, live-verified).
- Orange monster-preset dots appeared where nothing was (non-superunique presets) → only presets learned as
  superunique (live monster with flag 0x02 near the preset; persisted list); remove once that monster is dead.
- A killed superunique's marker stayed → same fix.
- Andariel's unique spear and gems not shown → items are M4b.

## Item drops (added 2026-11-14, user request)
- Show **important drops on the ground**: unique, set, gems, runes. Nothing else.
- Ground items are units of type 4 in the same unit hash table (slot block 4). On the ground = item mode to confirm live
  (classic D2: 3 = on floor). Like monsters, the client only knows items near the player — fine for drops.
- Quality from ItemData (unitData `+0x10`; classic D2 quality at ItemData+0x00: 5 set, 7 unique); gems/runes by base
  item txt id ranges (from items/misc txt).
- Live (Jail 1, 2026-11-14): ground items have **mode 3**; ItemData+0x00 = 2 (normal) on all 5 ground items — quality
  location consistent.
- **Verified with a dropped chipped emerald** (Act 2, 2026-11-14): item path is a static path, **x = u32 `+0x10`,
  y = u32 `+0x14`** (gem at 5323,5622 next to the player). txt 587 = `gcg`.
- **Items table in memory** (heap, address varies): records of stride **0x1C0**, 4-char code at record start.
  Index 0 `hax`, 1 `axe`, 2 `2ax`, 572 `gcv`, 587 `gcg`, 625 `r01`, 657 `r33` → D2R index = classic + 15. Locate at
  runtime by the `hax`/`axe`/`2ax`/… sequence, then classify gems/skulls/runes by code. Unique/set quality values
  (5/7) still to observe live.
- Drawn as distinct icons (unique gold, set green, rune orange, gem by colour), dropped from the map once picked up.

## Findings, second live session (Vex, Acts 1–5 Normal, 2026-11-15)
- **Landmarks come from lvlprest names** (blizzhackers/d2data `lvlprest.json`, `Def` = Room2 preset id). Confirmed in
  game: cave Next N 90 (exit, star) and Down W 91 (stairs), Inifus 161, Tower 1 163, Cairn Stones 160, Barracks Forge
  S/E 204/203, Sewer Next E 338, Radament's Lair W/N 341/344, Tomb Cube N 459 (+ chest object 354), Lair Tight Spot
  S 509 (Coldworm, drawn as a worm; Staff chest object 356). Unconfirmed directions stay "?".
  Maggot Lair Next 501–504 drawn as stairs without confirmation (pattern held in caves and sewers).
- **Room2 near rooms** (+0x10 array, +0x18 count) give walk-through borders to adjacent levels → border arrows.
- **Allies carry elite flags**: assassin traps have MonsterData 0x08; monstats `Align` set → never elite, never drawn.
- **Player choice is sticky per game** (group games flip-flopped and wiped the per-game memory).
- **Stash**: page 3 = cube, 4 = stash. Personal stash items are owned by the player id. Shared stash pages are
  player-type units without an act, one per page; the shared ones of the current character have
  `Inventory(+0x90) +0x48` = the player's id. Other tab units in memory are stale (earlier characters, offline
  shared stash) and must be ignored (verified by moving Cathan's Seal through pages 1–5).
- Open: Hell difficulty value 2; ethereal bit 0x400000; tomb orifice W/E/N, forge W/N, cube W/E/S, lair E/S.

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
- [x] **M1 reader** (verified live in Jail 1, 2026-11-14; fixture tests/fixtures/jail1-snapshot.json): `d2r-map-overlay --dump` prints one JSON snapshot. Validate against `research/rooms.py` / `presets.py` output in Jail 1 and Tower 5; confirm player chain `+0x150` and the elite flag bits.
- [x] **M2 overlay**: draw a static snapshot file (no game needed) — layer-shell, click-through, iso render.
- [x] **M3 live**: reader thread + Hyprland focus hide/show; ~5 Hz redraw.
- [ ] **M4 markers**: classification tables, per-game accumulation, exit labels.
- [ ] **M4b item drops**: implemented 2026-11-14 (items table found live; static path verified with a gem). Open: confirm unique (7) and set (5) quality with real drops, then tick.
- [ ] **Cleanup**: remove D2RMH exe/launcher (`~/.local/bin/d2rmh`); keep classic 1.13c files for phase 2.
- [ ] **Phase 2 (separate plan)**: map seed RE → d2mapapi-backed full markers from entry.

## Verification
- Unit tests: iso projection, preset classification, marker cache, snapshot parsing from a captured byte fixture.
- Live (single-screen protocol: instructions first, background recording, user says "done"):
  Tower 5 (Countess marker at entry), Barracks (Smith appears when near, stays after leaving), Jail 1
  (waypoint + both exits), plus a champion pack — compare overlay screenshot with the game's Tab automap.
