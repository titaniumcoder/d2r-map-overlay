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
- Build/test/run only through **cargo**: `cargo build --release`, `cargo test --workspace`, `cargo run -- --dump`.
- **Strict lints in the root `Cargo.toml`** (`[workspace.lints]`, every crate has `[lints] workspace = true`):
  ```toml
  [workspace.lints.rust]
  unsafe_code = "forbid"          # no unsafe needed: /proc/<pid>/mem via std::fs::File + read_at
  missing_debug_implementations = "warn"
  unused_must_use = "deny"
  [workspace.lints.clippy]
  all = { level = "deny", priority = -1 }
  pedantic = { level = "deny", priority = -1 }
  nursery = { level = "warn", priority = -1 }
  unwrap_used = "deny"
  expect_used = "warn"
  panic = "deny"
  indexing_slicing = "warn"        # memory parsing must use checked access
  cast_possible_truncation = "deny"
  too_long_first_doc_paragraph = "allow"  # nursery; fires on exported items only (step 11)
  ```
  (If a dependency's API forces `unsafe`, it stays confined to one module with a documented `#[allow]`.)
- **Gate before every commit**: `cargo fmt --all --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace`.
  Clippy is the guard against invented APIs: code that doesn't compile/lint clean doesn't get committed.
  The `reviewer` agent runs this gate and reviews the diff before each commit.
- `rustfmt.toml` with `max_width = 110`; `rust-toolchain` pinned via mise.

## Git
- Step 0: `git init` in `~/Work/d2r_map_overlay`, first commit = research probes + PLAN.md + CLAUDE.md + cargo skeleton.
- **Small, regular commits**: at least one per milestone sub-step (e.g. "process: find D2R pid", "image: memfd alias + sig scan"),
  each passing the gate. Conventional, descriptive messages; no push (no remote) unless asked.

```
Cargo.toml         — the workspace (members `crates/*`; `[workspace.lints]`, `[workspace.package]`,
                     `[workspace.dependencies]`) and the binary package `d2r-map-overlay` at the root
                     (so the release binary stays `target/release/d2r-map-overlay`). Features pick
                     the plugins: `reader-proton`, `playstyle-config` (both default).
src/main.rs        — the binary: args, `TheReader` / `ThePlaystyle` from the features, wiring
                     (reader thread → channel → overlay), `--probe` / `--dump` / `--render` / `--show`
playstyles/<name>/ — a playstyle folder (step 7); shipped: `summoner-fury/` = playstyle.toml (name,
                     description, `[valuables]`, `[damages]`), builds/*.toml + *.source.json
                     (research/gen_build.py writes here by default), farm.toml, route.toml,
                     valuables.json (compiled in). Overrides per file: ~/.config/d2r-map-overlay/playstyle/
assets/            — the dashboard pages (`include_str!` from `crates/core/src/web/page.rs`)
crates/core/       — `d2r-core` (lib `d2r_core`): everything both plugins and the binary share
  reader.rs        — the reader interface: `GameReader` (attach / attach_now / snapshot / quest
                     flags / alive / save dir), `Attach`, `ReadMem` (+ `is_user_ptr`)
  playstyle/       — the playstyle interface: `Playstyle` (assoc. types `Wants` (kept by the core
                     between ticks) and `Extra<'a>` (only its panels read it); load / reload(&Files)
                     / mark_drops / evaluate / panels / config_json / builds_json / select_build /
                     remove_character), `Evaluation<'a, W, X>` (guidance, wants, reached, extra),
                     `EvaluationOf<'a, P>`; files.rs: `Files` (shipped / user / old places / data
                     dir), the shipped-folder lookup (`$D2R_PLAYSTYLES_DIR`, else the workspace
                     root's `playstyles/` via CARGO_MANIFEST_DIR/../.., else next to the binary),
                     `fingerprint` for the hot reload
  guidance.rs      — what a playstyle hands the core: `Guidance` (area rating, BiS, item plan,
                     drop marks), `Mark` / `MarkKind` / `Marks`, `Tier`, `Category`, `Spot`, `Side`,
                     `Action`, `Advice`, `Bis`
  game.rs          — the snapshot model (`Snapshot`, `Player`, `Level`, `ItemDrop`, `Equipped`, …;
                     serde = the `--dump` JSON); filled by the reader
  offsets.rs, items.rs (item codes, kinds, the items-table scan), quests.rs (quest record) —
                     game layout knowledge, platform-independent
  build.rs         — damage types and the area `rate` (info line colour)
  live.rs          — live tick loop over a `GameReader` and a `Playstyle` (session, guidance,
                     render, dashboard, hot reload)
  config.rs        — config.toml, core only (`[shrines]`, `playstyle = "<name>"`); old files'
                     `[valuables]` / `[builds]` / `[bis]` accepted unread
  web/             — the dashboard core (step 6): mod.rs (`LiveInfo`, `Panels`, `live_json`),
                     server.rs (HTTP + SSE, `/config` endpoints generic over the `Playstyle`), page.rs
                     (a hash test pins the served bytes), stash.rs, skills.rs (`SkillTarget`),
                     vendor.rs; fixtures.rs (test data, also for the plugin crates' tests)
  render.rs, info.rs, session.rs, journal.rs, overlay.rs, hypr.rs, names.rs, stash.rs, shop.rs, …,
  *_data.rs (generated tables); testutil.rs (`FakeMem`, `temp_dir`; with fixtures.rs behind the
                     `testutil` feature, a dev-dependency feature of the plugin crates)
crates/reader-mem/ — `d2r-reader-mem`: the reader plugin
  lib.rs           — `Platform` (find process, open memory, find image, heap regions, save dir);
                     `ProtonReader = MemoryReader<Proton>`; Windows port notes
  memory_reader.rs — game layer driver over any `Platform`: unit-table retry/backoff + offset cache,
                     background items-table search, quest-global search budget
  game.rs          — the structure walk (generic over `ReadMem`) → `d2r_core::game::Snapshot`
  image.rs         — `find_image` (memfd alias); `Image::unit_table` / `quest_flags_global`
                     (signature, cache, structural `.data` search)
  offset_cache.rs  — per-build offsets in the cache dir
  process.rs       — /proc pid discovery, maps parsing, heap regions, pread on /proc/<pid>/mem
  proton.rs        — (feature `proton`) the Proton/Linux `Platform`: `ProcMem`, memfd image,
                     WINEPREFIX save dir, ptrace hint
crates/playstyle-config/ — `d2r-playstyle-config`: the config-driven playstyle plugin
  config_driven.rs — `ConfigDriven` (+ its `Extra`: stage, chain, route advice)
  files.rs         — load / reload of a `Files` (per-file precedence user > old places > shipped;
                     a broken file keeps its part), bis.toml writes
  config.rs        — `Settings`; playstyle.toml / old config.toml tables / builds/*.toml / farm.toml
                     parsing, `Build`, `SwitchGroup`, `Stage`, the stage chain and latch logic
  marks.rs, stash_plan.rs (`Wants`), valuables.rs, gear_score.rs, route.rs, worth.rs
  web/             — the playstyle's dashboard panels: `PanelInfo` and `panels()` → `Panels`; gear.rs,
                     recipes.rs, farm.rs, config_page.rs (`/config` JSON, selection edits)
```
Paths in the milestone history before step 11 (`src/reader/`, `src/playstyle/`, `src/web/`) now live under `crates/`.

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
  other players in a group game (PlayerData `+0x10`, name at +0).
- **Unit**: `+0x04` txt, `+0x08` id, `+0x0C` mode (0/12 dead), `+0x10` unitData, `+0x38` path (x `+0x02`, y `+0x06`, room1 `+0x20`).
- **Level**: room1 `+0x18` → room2; room2 `+0x90` → level; level `+0x1F8` id, `+0x10` first room2.
- **Room2**: `+0x48` next, `+0x58` room1 (loaded), `+0x60/64/68/6C` x/y/w/h in room units (×5 = tiles),
  `+0x78` room-tile list (entry `+0x00` Room2 of the linked level, `+0x08` next), `+0x98` presets.
- **PresetUnit**: `+0x04` txt, `+0x08` x (room-relative), `+0x10` next, `+0x20` type (1 monster, 2 object, 5 tile/exit), `+0x24` y.
- **MonsterData** (unitData): `+0x18` nameSeed, `+0x1A` flag byte, classic D2 bits: 0x02 superunique, 0x04 champion,
  0x08 unique/boss, 0x10 minion. Live: minions of a killed pack = 0x10, pack leaders seen as 0x08/0x0C.
  Superunique bit 0x02 confirmed from the journal (2026-11-18): Treehead WoodFist (txt 24) logged with flags 0x0A
  at his preset in the Dark Wood; the Cow King (txt 735) reads 0x0A too.
- **Room1** (loaded rooms only): `+0x38` → CollMap: u32 `+0x00/+0x04` world x/y, `+0x08/+0x0C` width/height
  (= Room2 rect), `+0x20` → width×height u16 flags, row-major. Bit `0x0001` = blocked (walls seen as 5/7, floor 0).
  Verified Sewers Level 1, 2026-11-14.
- **Room2 near rooms**: `+0x10` → array of Room2 pointers, **`+0x18` u32 count** (live 2026-11-15, Outer Cloister;
  `+0x20` sometimes reads 9 with trailing nulls and is a different field; the older `+0x50` u16 note is superseded).
  Includes other levels' rooms: top-row rooms of 27 had near rooms in 28, bottom-row ones in 26. Used for
  walk-through border exits (`Snapshot::border_links`, drawn as a triangle on the shared edge). Earlier note: only
  filled for loaded rooms, and no walkable-connection info for unexplored rooms (openings need the seed).

- **Skills** (live 2026-11-17, Warlock clvl 38, probe `skillprobe.py`): unit `+0x100` → skill list header
  (`+0x00` first skill, `+0x08` left-button skill, `+0x10` right-button skill). Skill node: `+0x00` → skills.txt
  record (u16 id at +0), `+0x08` next, **`+0x38` u32 hard points** (class skills summed to 41 = 37 + 4 quest
  points, no unspent `newskills` stat). Gear-granted skills sit in the list with 0 hard points; no bonus-level
  field in the node's first 0x100 bytes. Tree layout from `research/gen_skills.py` → `src/skills_data.rs`.

- **Quest flags** (live 2026-11-18, druid "Elemental" clvl 21 Normal, probe `research/quests.py`):
  PlayerData `+0x40/+0x48/+0x50` point to per-difficulty quest BitBuffers, but on the client they
  stay **all zero** (server copy). The client's record of the current game is a `.data` global
  (rva 0x1edaf78 in build `6ad81202-279c000`; found by structural search: exactly two adjacent
  `.data` slots point to a header `{+0x00 buffer ptr, +0x08 u32 0x300 bits, +0x28 u64 0x60 bytes}`,
  the first is the record, the second an unknown record with bits 13/15 on finished quests).
  Buffer = 48 u16 words in classic d2s order (1–6 Act 1: Den, Burial Grounds, **3 Tools of the
  Trade**, Cain, Tower, Andariel; 9–14 Act 2; 17–22 Act 3; 25–27 Act 4; **35 Siege on Harrogath**
  … 40 Eve of Destruction; 41 Akara respec; intros/completions in 0/7/8/15/16/23/24/28).
  Bits (D2MOO): 0 reward granted, 1 reward pending, 12 primary goal done. Live: Acts 1–2 words all
  bit 0 (Seven Tombs `0x3e25`), Tools of the Trade `0x1049` = imbue used (user confirmed), Blood
  Raven `0x000c` (not done), Act 3 entered (word 16 = 1, quests 0), Act 5 all 0, respec word
  `0x8002` = pending (unused). Assumed to be the current game's difficulty (only Normal seen).
## Markers
- **Waypoint**: object ids table (119, 157 seen; full list from objects.txt waypoint rows).
- **Exits**: type-5 presets + room-tile links (link target level id → label "→ Jail Level 2").
  Entry = the one nearest the arrival position.
- **Superuniques/specials**: type-1 presets (853/757 Tower 5, 771 Smith…) + live monsters with superunique flag.
- **Packs/uniques**: live monsters with unique/champion flag (minions clustered to their leader); dead skipped.
- **Accumulate per game** (done 2026-11-15): presets, shrines, wells, kills, operated seals and arrivals are remembered per level for the game (identity = player unit id; act travel keeps it) and persisted to `~/.cache/d2r-map-overlay/game.json`, so overlay restarts keep them too. Live 2026-11-17: the journal's game id second u32 changed at the Act 1→2 transition (`57f3da70` → `57fa1300`) — per-game identity must not rely on it (it uses the player unit id, which stayed).

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

## Tal Rasha's Tomb symbols (2026-11-18, user request)
- The Canyon of the Magi (46) exits show each tomb's Horadric symbol instead of seven "Tal Rasha's Tomb" names
  (src/tombs.rs, drawn by `render::draw_labels` in the exit's XP colour; the real tomb bigger, gold, with a star).
- Mapping (level → symbol) is fixed per tomb: classic `data\global\ui\MENU\invps.DC6` has 7 frames, frame i =
  tomb level 66+i (1.13c quest log draws frame `staff tomb − 66`, per troytt/diablo2-web
  `kb/ui/d2client-quest-log-true-tomb-symbol-113c.md`). Extracted from the local d2data.mpq with research/mpq.py +
  research/dc6.py: 66 pinwheel, 67 gate (square between posts), 68 crescent, 69 ring, 70 nested chevrons,
  71 triangle in triangle, 72 ring over a crescent. Each tomb has its own entrance warp (lvlwarp 38..44 "Desert to
  Tomb Tal 1..7" = levels.txt Canyon `Warp1..7` beside `Vis1..7` = 66..72), so the doorway symbol goes with the level.
- Not yet checked in game: that the overlay's symbol matches the doorway's (needs a Canyon visit; compare with the
  obelisks / Tab map).

## Flayer Jungle (78), 2026-11-15
- 192 rooms in 9 large piece types (16 or 32 Room2 each): 534, 535, 538, 539, 565, 570, 574, 601, 602.
- **601 = waypoint area** (player and waypoint object 237 inside it). **602 = Gidbinn altar candidate** (only other
  outlier id) — **confirmed**: the Gidbinn altar (object 252) is in 602; landmark rule added.
- **Correction 2026-11-20** (user report, Nightmare): lvlprest 595–604 are "Act 3 - Clearing Pygmy
  W/E/EW/S/SW/SE/N/NW/NE/NS", pygmy-village clearings, one per exit direction, and a jungle holds several.
  That game had 595 (waypoint 237 inside), 600 and 602; the altar was **not** in 602. The rule is now a
  candidate rule: every 595–604 clearing without the waypoint gets a "?", and once object 252's preset is
  loaded only its exact quest-object star shows. Landmark markers are now one per connected piece (rooms of
  one preset id joined by shared edges), so two copies of a preset no longer get one marker between them.
- **Live-confirmed 2026-11-20** (Nightmare): with two candidate stars, the altar's clearing kept the
  marker once the altar came near and the other star disappeared.
- **Elimination 2026-11-20** (user's choice): the candidates are gold stars, not "?". A clearing is ruled
  out once **every** room of it has been seen loaded (Room2 `+0x58` Room1 non-null) with no object 252 in the
  snapshot; the session remembers candidate rooms seen loaded per level for the game (`GameState::loaded_rooms`,
  `Room::seen_loaded`), so the clearing stays ruled out after it unloads, and a new game forgets it. The last
  candidate left stays a star. Signal: a Room2 is initialized (preset list built from the DS1) before its Room1
  exists, so a loaded room's presets are complete. A non-empty preset list alone is not used: the dump shows
  preset lists on unloaded rooms (they survive the Room1), and a list could hold level-generation presets
  before the DS1 units. Not live-verified yet: that the altar's preset is in the list whenever its room's Room1 is.
  The 2026-11-20 dump was taken after the user left 602, so its 602 rooms read unloaded (7 of 16 still with
  presets): from that dump alone 602 stays a candidate; the live session would have seen them loaded.

## The Warlock class (2026-11-17)
- The user's "Demonmaster" reads **class id 7** — outside the classic 0–6. The Basin wiki documents a
  **Warlock**, a D2R patch class (skill trees Chaos/Eldritch/Demon: Miasma Bolt, Apocalypse, Abyss,
  Summon Goatman…), and d2runewizard.com lists its breakpoints: FCR 9/18/30/48/75/125 (as necro/pala),
  FHR 5/10/16/26/39/56/86/152/377, FBR as druid/necro. The config class tables now accept 8 classes.
- **Stat lists (2026-11-17, research/stats.py + itemstats.py)**: the player's stat list at unit +0x88
  is **base-only** (level, strength, life/mana/stamina in 1/256 units, xp — NO item stats). Item-granted
  stats live on the ITEMS: each item has a base list (defence, durability, requirements) at unit +0x88
  and a **magic-affix list chained at StatList +0xD0** (each list's `+0x08/+0x0C` = owner (unit type,
  unit id); the affix list's owner is the item). Verified: the equipped items' affix sums equal the
  character screen exactly (res 6/9/8/7, fire/light/cold/poison; no runewords then — see "Aggregated item
  stats" for why the gear sums now read the top list's `+0xE8` array). `ItemData::OWNER` (+0x0C) separates the
  player's gear from the mercenary's (merc items carry `NO_OWNER`, and two body slots were the merc's
  duplicate armour/weapon). The overlay therefore sums gear stats over equipped items and carried charms
  (charm codes cm1/cm2/cm3) per owner. Monsters keep full lists at +0x88 (a mob's list had its
  res/experience/level inline).
- **Quest resists (2026-11-20, user: Anya done in Normal and Nightmare, guide res 10 too low)**: Anya's
  Scroll of Resistance (+10 all, per difficulty, permanent) is on no item. The reader now adds the
  resists (39/41/43/45) of the player's own base list (`+0x88` entries) to the gear sums; that list is
  item-free (above), so nothing counts twice, and the guide still subtracts the difficulty penalty once.
  If the player's top list has `FLAG_FULL_STATS`, its aggregated array is compared with gear + own and a
  mismatch is a snapshot warning (`--dump`). Live-verified 2026-11-20 (Hell, level 68): gear-only
  110/156/78/129, with the own list 120/166/88/139; after the −100 penalty 20/66/−12/39, exactly the
  character screen (user). The own list carries +10 for two Anya scrolls, so the game stores the
  bonus as one +10, not per difficulty; no warning in `--dump`.

## ActMisc, difficulty and seed lead (2026-11-15)
- Player unit `+0x20` → act; **act `+0x70` → ActMisc** (older builds: +0x78, which now points into code).
- ActMisc `+0x830` u16 = **difficulty**: 0 Normal and 1 Nightmare confirmed live (2026-11-15); Hell (2) still to see.
- ActMisc `+0x120` u32 = **staff tomb level** (`dwStaffTombLevel`, the real Tal Rasha's Tomb, 66..72), from
  jagarop/d2.re `D2R/IDA.h` `D2DrlgStrc` (same struct lists Difficulty at +0x830, matching ours). Live-verified
  2026-11-18 once (Normal, research/tombprobe.py): 72, and the character had entered Duriel's Lair from tomb 72
  (game.json `came_from`). Read as `Snapshot::staff_tomb` (only 66..=72 kept). Second game still to see.
- Phase 2 lead: ActMisc `+0x840` and `+0x860` hold random-looking u32s (e.g. 3943813804 / 3154569827), where the
  seed-selector plugin says the generator stores {seed, 0x29A} and the derived value. Used directly as d2mapapi
  seeds they do **not** reproduce the Flayer Jungle (waypoint/altar positions differ) — likely encoded/derived.
- Monster level is **not** in the client's monster stat lists (only HP%, speed…); the info line uses the static
  levels.txt values (src/levels_data.rs, generated by research/gen_levels.py from blizzhackers/d2data).

## Superunique spawn ids (2026-11-15)
- Monster preset txt of a superunique's spawn point = **751 + hcIdx** (superuniques.txt row order index). Verified
  live for Countess 757, Smith 771, Sarina 775, Icehawk 776, Bloodwitch 762, Coldworm 766 (6/6); 2026-11-17 also
  Corpsefire 791, Bishibosh 751, Rakanishu 754, Treehead WoodFist 755, Griswold 756, Pitspawn Fouldog 758,
  Boneash 760, Radament 761 (14/66 total). All 66 are in
  `SUPERUNIQUES` (src/levels_data.rs, generated by research/gen_levels.py). This replaced the earlier learning
  heuristic, which mislearned generic spawn ids (328/840 next to Witch Doctor Endugu).
- Kurast Sewers 1 (92): lever (object 367) and stairs down (366) in room preset 739 → landmark (one game).

## Item fields and valuables (2026-11-15)
- ItemData `+0x00` quality (2 normal, 4 magic, 5 set, 7 unique verified live), `+0x18` flags (0x10 identified
  verified; 0x4000000 runeword verified live 2026-11-18 (flags 0x04800810 on a Stealth leather armour and a Leaf
  staff, absent on every other equipped item); 0x400000 ethereal = classic bit, unverified), `+0x34` unique/set row index
  (verified: Khalim's Flail = row 127, Hellforge Hammer = row 126, both identified quest items; an UNIDENTIFIED set item
  (Cleglaw's Claw, row 32) read 0 → the index is only filled once identified, so name rules on unidentified items
  stay uncertain). After identifying (Cain or scroll), the index is filled: Darkglow = unique 83, Cleglaw's Claw = set
  *ID 7 (set rows are keyed by `*ID`, not file order) — both verified live 2026-11-15; the inventory keep list relies on this., `+0x54` body slot,
  `+0x55` page (0 inventory, 4 stash, 255 elsewhere; verified over 74 items). Store items: `+0x18` bit 0x2000, page = store tab,
  magic affix ids at `+0x48`/`+0x4E` (see "Vendor trade window"). Inventory = mode 0 + page 0.
- Item stat list: unit `+0x88` → `+0x30` entries / `+0x38` count, entry {u16 layer, u16 id, u32 value}; sockets = 194.
  Chain (live 2026-11-18, level-4 amazon, probe walking `stat_list::NEXT` while type 4 + owner = item id):
  list 0 = BASE list: rolled base defense `31` (Hard Leather Armor 25 = max 24 + 1 with ED, Leather Gloves 4 =
  max 3 + 1; a plain Cap 3), durability 72/73, damage pairs `21/22` one-handed, `23/24` two-handed (Short Bow
  1/4), `159/160` thrown (Javelin 6/14 beside 21/22 = 1/5), quantity 70, attack speed 68 (signed: -5);
  list 1 = affixes (ED% `16` on armour, res `39/41/43/45`, gold find 79, durability% 75); then the chain runs
  into other owners' lists (stop there). So ED% applies to the base-list value; flat bonuses add after.
- **Aggregated item stats** (live 2026-11-19, level-22 druid, Normal; probes rwprobe.py/fullcheck.py run
  from the scratchpad, logic as research/itemstats.py): the owner-matching chain above MISSES runeword and
  socket stats. A socketed item's top list (`unit + 0x88`) chains next to the first socketed rune's own lists
  (other item id), then a type-3 list; the runeword's own stats are in no list of that chain. But every
  item's top list has flag `0x80000000` at `+0x1C` (`stat_list::FLAG_FULL_STATS`; chained affix lists have
  `0x40`) and an **aggregated array at `+0xE8` ptr / `+0xF0` count** (`+0xF8` capacity), entry layout as
  `+0x30`: base + affixes + runeword + socketed runes, modifiers applied (Ancients' Pledge defense 17 → 25,
  Stoutnail damage 7/12 → 14/24 with its 100% ED; ED itself not listed). Ancients' Pledge (Ral Ort Tal):
  fire/light/cold/poison 48/48/43/48; Stealth (Tal Eth): poison 30, FCR/FHR/FRW 25, dex 6, stamina 15, mana
  regen 15, MDR 3. A socketed rune's own top list holds its slot-specific stat (Tal in armour: poison 30, in a
  shield 35; Eth: mana regen 15). Over 168 item units (worn, inventory, stash, the merc's) all had the flag;
  162 had array == chain sum for res/IAS/FHR/FCR, the 6 different ones all runewords/socketed.
  `Reader::accumulate_gear` (guide res and breakpoints) reads only this array (each stat once); a top list
  without the flag (never seen) falls back to the old chain walk with one warning per scan. `item_stats`
  (worn and store items for the vendor "better" mark / `gear_score`) reads the same array too, through
  `ItemStat::from_totals`: the score's base × (100 + ED%) / 100 + flat rule only needs the base/affix split
  for defense and damage, and the array already has them enhanced (rare boots 6 + 23% ED → 7) and lists no
  ED%, so defense and the damage pairs the base list carries come in as base values, ED% entries are dropped
  (never double-counted) and a damage stat the base list lacks (flat bonus written to every pair) is
  dropped so a two-hander stays two-handed. Fallback without the flag: the chain walk (split kept), silent.
  Effect (druid, caster weights): Ancients' Pledge scores 1.7 → 189.5, Stealth 3.2 → 93.7.
  Other list fields seen: `+0x00` owner unit (affix lists), `+0x78` the parent item's top list (on a rune's
  list), `+0x68/+0x70` sibling links (unused).
- Valuables: playstyles/summoner-fury/valuables.json (67 rules, S/A/B, trade/twink; was research/
  until step 7) embedded; tiers per category in the playstyle's `playstyle.toml` `[valuables]`
  (default S+A both). Ground: trade ring by tier
  (S red, A orange, dashed = uncertain); since step 10 also keep (green) and farm (green + star)
  rings, see the plugin refactor's step 10; since 2026-11-22 a short tag beside every ring and a
  gold coin on S/A trade rings (see Overlay). Inventory keep list (dashboard; formerly green lines on the overlay): certain
  S/A, uncertain only S — moved to the dashboard with the 2026-11-17 text-stack migration (see Overlay), the
  reader and its tiers unchanged. Unique/set names from src/item_names_data.rs (research/gen_item_names.py).

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
level (room pieces), arrival, marker (+ piece), link, elite monster, killed, drop, and `buffs` (the
player's state changes with a raw stat-list dump; see the buff tracker backlog item). Files are never rotated;
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
- **Cow Level trail** (same request): in level 39 the session records the player's position into `GameState`
  (a point ≥ 4 tiles from the last, at most 5000, oldest dropped; skips 0,0 loading frames), drawn just above the
  room fill as a 3 px translucent pink polyline, broken at jumps > 30 tiles (teleport, waypoint, portal).

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

## Findings, third live session (Vex, Nightmare, 2026-11-15/16)
- Confirmed landmarks: sewer stairs W 337, Radament S 343, orifice W 460 (drawn as a mini orifice), fake-tomb
  treasure rooms 472–475 (all drawn as stars at the user's request).
- Unique/set `index` keys are internal names; in-game names come from `allstrings-eng.json` (`DISPLAY_NAMES`).
- Player stat 12 = character level (live: 39). Level row suggests the areas whose monster
  level is in the band clvl − 5 … clvl + 4 (user feedback 2026-11-17, example "clvl 23 →
  monsters 18–27"; −5 is the XP-penalty threshold), ranked own level first (its champions are +2
  and its uniques +3 — the efficient XP zone for melee), then downwards, the above-level areas
  last (user spec, 2026-11-17), until clvl 75. The dashboard gets the whole band, one area per
  line, laid out in three below/equal/above columns (user request, 2026-11-17). The seven
  same-named Tal Rasha's Tombs dedupe to one (name+mlvl; same-named
  areas with different levels stay separate); the cow level is never suggested.
- Build colouring uses per-area immunity counts from monstats (`ResDm` = physical). Nightmare has almost no
  immunities, so it is mostly green; Hell is where red shows up.
- Trail is kept per level and game; cow counter in level 39 (cows.json for lifetime stats).
- Valuables rules can say `"keep": "one"` (Sigon's set for twinks).

## Shared stash reader (added 2026-11-17, implemented from research/notes/stash-format.md §3)
- Each shared tab is a **player-type unit without an act, one per page**; the live ones have
  `Inventory(+0x90) + 0x48` = the current player's unit id (stale ones from earlier characters are excluded by that
  check).
- **UI order (live 2026-11-17)**: the PLAYER unit's own inventory holds a linked list of the live tabs in UI order
  (`offsets::inventory::TAB_LIST`: header = first node, last node, u32 count; the header sat at `+0x60` in one game
  and at `+0x68` in the next, so the reader takes the first candidate whose first→last walk matches the count).
  Node: `+0x00` tab unit id, `+0x0A` byte 1 = the gems/runes/materials ("advanced") tab (a hint, one account seen),
  `+0x10` next. Normal pages are numbered 1-based in list order (`stash::TabKind::Page(n)`), the advanced tab is
  `TabKind::Materials`. A listed tab whose unit isn't in the unit table is an empty tab (numbers never shift). No list
  but live tab units: pages in unit-table order plus a snapshot warning. Verified: Sigon's set on page 5, 49 stacks.
- **One advanced unit, three UI tabs** (user screenshot 2026-11-22): the stash window shows `Personal | Shared |
  Gems | Materials | Runes`; in memory Gems, Materials and Runes are the one advanced tab unit (the log's
  "6 tabs" = 5 shared pages + it). The reader reads all its stacks; which UI tab a stack shows in follows
  from its item type (`stash::tab_of`), not from memory. Its stacks already count as held: the recipes'
  runes and gems (`web::recipes::materials_of`) and the dashboard's materials panel.
- **Tab items**: each tab unit's `Inventory + 0x10` = first item unit, then `ItemData + 0xB0` = next item in the
  same inventory (`item_data::NEXT_IN_INVENTORY`), capped at 100 per tab. The advanced tab's stacks have `OWNER`
  `NO_OWNER`, page 4, no stat list, and their stack size at `ItemData + 0x9C` (`item_data::QUANTITY`); zero-size
  stacks stay in the tab (the dashboard drops them). Normal-page items still carry the tab unit id as `OWNER`, so
  `game::place` gives them `Place::Shared` for the spare lists; the stacks (`NO_OWNER`) never reach the
  keep/spare lists (gems/runes are never spare, and the materials have their own panel).
- **Socketed items** (live 2026-11-18, research/sockets.py, `--dump`): an item unit's own `unit + 0x90` Inventory
  chains its socket contents the same way (`Inventory + 0x10` first, `ItemData + 0xB0` next), in socket order:
  Stealth armour r07 → r05 (Tal Eth), Leaf staff r03 → r08 (Tir Ral). The runes are ITEM units with body 0,
  page 255, `OWNER` `NO_OWNER`; unsocketed items have a null inventory or a null first item. The reader names an
  equipped runeword by matching that rune sequence against `runewords_data::RUNEWORDS`
  (`stash::runeword_by_runes`, max 6) and its level req = max(rune `levelreq`, base `levelreq`); the upgrades'
  worn check compares names ignoring case and apostrophes ("Ancient's" vs the builds' "Ancients'").
- **Session caching (user decision)**: D2R fills the stash only after it was opened in the game, and the tab units
  vanish again later; the session therefore caches the captured tabs for the rest of the game (kept while the live
  read is empty) and drops the cache only when the player unit id changes. Not persisted across overlay restarts.
  A degraded live read (tab units present but every tab still empty — a torn mid-load capture, §3.4) never
  downgrades a cache that has items; the trade-off is that a fully emptied stash keeps showing its last capture
  until the next live read with items (or a new game).
- `Snapshot.stash`/`stash_names` feed the dashboard as `stash_pages` (`[{page, items: [{text, colour}]}]`, UI order)
  and `stash_materials` (`{gems, runes, other}` of `{name, qty, code}`; gems by colour then grade, runes by number)
  and the `BiS` ✓: `bis_lines` ticks a line `owned` (character's own) or `stash` from the two name lists; the
  dashboard gets the structured per-line `tick` and marks ✓(stash) distinctly (no string scraping) — the
  `BiS` lines are dashboard-only since the 2026-11-17 text-stack migration.
- **Runewords and worn vs owned ticks** (user report 2026-11-18: a Stealth in the private stash had no ✓):
  runewords are named from their socketed runes (`Reader::runeword_name`: runeword flag + the item's own
  socket chain) for EVERY owned item (inventory, cube, private stash, worn) into `owned_names`, and for
  shared-stash items (the `StashItem` takes the runeword name, so the stash panel shows it too) into
  `stash_names`. `info::Tick` is `worn` (in `game::worn_names(equipped_items)`), `owned` (owned but not
  worn) or `stash`; the dashboard ticks worn pieces green and owned/stash ones yellow ("you have it, go
  equip it"; ✓ owned / ✓ stash). The overlay text keeps one plain ✓. Not yet verified live on a stashed
  runeword (the socket chain of a non-worn item is assumed the same as a worn one's).

## Vendor trade window (added 2026-11-18, user request: "nice to have")
- **Store items are not in the unit table**: they hang off the vendor NPC, a monster unit (type 1):
  `unit + 0x90` Inventory → `+0x10` first item → `ItemData + 0xB0` next (the stash-tab/socket chain).
  Filled only while the trade window is open, empty after closing. Live (python probes, Ormus, ~60 items).
- Store `ItemData`: `OWNER` = `NO_OWNER`, body 0, page `+0x55` = store tab (0 armor, 1/2 weapons, 3 misc).
  **`FLAGS` bit `0x2000`** (`item_data::FLAG_IN_STORE`, classic `ITEMFLAG_INSTORE`) is set on every store
  item and on nothing else: the shop-open signal.
- **Magic affix ids** (live on 14 owned magic items against their stat lists): u16 at `+0x42` rare prefix,
  `+0x44` rare suffix, `+0x48/+0x4A/+0x4C` magic prefix[3], `+0x4E/+0x50/+0x52` magic suffix[3]; 0 = none.
  Suffix id = magicsuffix row (Teleportation = 532), prefix id = 785 (the highest magicsuffix row) +
  magicprefix row. Names via the English string table; several rows share a name, so builds match by
  composed name. Required level = max(base, prefix, suffix `levelreq`). Tables: `research/gen_affixes.py`
  → `src/affixes_data.rs` (affixes, misc bases, vendor names).
- **Not yet live-verified**: that a STORE item's affix words sit at the same offsets (assumed from owned items).
- **Suffix id 0** is read as "no suffix". Settled live 2026-11-18 (4 h shop recording, Acts 1–2, 89 windows):
  9 items "… of Health" (Scale Mail, Kite Shield, …) kept their suffix and none of 1312 magic store items was
  named as a bare base, so "of Health" does not read 0.
- Reader (`game::Reader::shop`, towns only): the first monster with `FLAG_IN_STORE` items in its inventory
  chain (cap 300) → `Snapshot.shop` (`shop::Shop`: vendor name, items with base/prefix/suffix/name/req).
  Only magic (quality 4) names are decoded; others show the base name.
- Dashboard: "Vendor" panel (hidden without a shop), one column per store tab; an item is a build piece
  when the piece names its base plus any subset of its affixes (`shop::matches`: "Coral Heavy Gloves"
  matches Coral Heavy Gloves of Frost, "Russet Heavy Boots of Haste" needs both) in the current stage's
  character or merc slots (outgrown pieces skipped) → bold ▲ with the slot, and an upgrade entry of kind
  `shop` (key `shop:<slot>:<piece>`, so it chimes once per build piece). Required level above clvl → faded.
- **Stat comparison** (user request 2026-11-18, leveling help): each store item and each equipped item
  carries its stat entries (`Reader::item_stats`, the chain above; store items assumed to chain the same
  way, not yet seen live). `gear_score::verdict` scores an item for the character (class, clvl, base
  str/dex `Player::strength/dexterity` (stats 0/2, unverified) + worn gear's, the allocated skills plus the
  stage's skill plan, the weapon kind of the worn weapon else of the best wearable stage weapon piece,
  caster = staff/wand/orb or a weaponless sorc/necro) and compares with the worn item in its body slot:
  rings vs the worse ring, a two-hander vs weapon + off-hand, a one-hander vs the weapon, a shield not
  at all under a two-hander, an empty slot = 0. Weights in `gear_score::WEIGHTS` (one table: +skills
  large, planned skill tabs/skills large, avg weapon damage only for the used weapon kind and not for
  casters, res, life, mana/FCR more for casters, IAS/FHR/FRW, defense and MF small), bar
  `gear_score::MIN_GAIN` = 8 points. Unusable items (class, level, str/dex incl. requirements -%) are
  never better. Live JSON per item: `better` + `why` (the three largest weighted gains, "+15 fire res,
  +4 avg dmg"); the panel shows "▲ better" + why under the build pieces. NOT in `upgrades` (no chime).
  Base slot/kind/class/str/dex: `src/bases_data.rs` (`research/gen_bases.py`). Not counted: set bonuses,
  +damage from dexterity/strength, skill-based damage, the ethereal ×1.5 base damage/defense; the
  replaced item's own +str/+dex still count toward requirements. Unverified: whether socketed
  gems/runes' stats sit in the item's own chain (then they count), the `>> 8` of life/mana (stamina
  checked). Flat +min/+max: the score takes the largest affix value of the three pairs.
  Live 2026-11-18 (`--dump`, Fulmina clvl 6): player str 27 / dex 28 read from stats 0/2
  (plausible); the swap set sat in body slots 11/12 (Hunter's Bow 11, Javelin 4), so slot 4 was the
  active weapon (one character, one weapon set seen: after a weapon swap slot 4 may hold the other set); flat damage lands on the weapon's own pair (Javelin +2 max on 22 and
  160, Hunter's Bow +1 on 24 only); a belt's +10 stamina read 10 after `>> 8`; a magic ring's base list
  is EMPTY (count 0) with its affixes in the next list (the reader must not stop at an empty list).
- [x] Stat comparison ("▲ better" in the Vendor panel): implemented 2026-11-18; NOT yet verified live
  (store items' stat chains, player str/dex stats).
- [x] Vendor panel: implemented 2026-11-18; verified live the same day on Fara (A2): ~65 store items decoded to sensible magic names ("Bronze Double Axe of Maiming", "Helm of the Jackal", "Shimmering Large Shield of Deflecting"), store affix words sit at the same offsets as owned items.

## Build definitions and BiS selection (2026-11-17)
- **Agent-authored build files**: `playstyles/summoner-fury/builds/<slug>.toml` since step 7 (was
  `~/.config/d2r-map-overlay/builds/`, still read as a fallback), one build per file
  (schema: `name`, `damages`, `notes`, `[slots]`, `[merc_slots]` — pieces as `"Name@<required level>"`,
  ordered best→worst; optional `merc_acts = [2, 1]`, primary first (the older single
  `merc_act = N` still reads), default `[2]`, the acts of the build's mercenaries, from the
  source JSON's `merc.acts` / `merc.act` in `gen_build.py`; each merc runeword goes to the first
  act that can carry it, `Build::merc_wearer`). Loaded by `playstyle::files` (missing dir = no builds, no error), keyed by
  lowercased build name — the slug a selection refers to. The webapp does NOT edit them.
- **Selection-only config**: the webapp writes ONLY `build = "<slug>"` into `[bis."<char>"]` (toml_edit,
  comment-preserving; the tables are created if missing) — since step 7 in
  `~/.local/share/d2r-map-overlay/bis.toml` (an entry still in config.toml's `[bis]` is edited there). Resolution order for a character's BiS:
  inline slots (legacy, hand-edited) if present, else the selected build's slots, else none. Same for
  damages: character-level `[builds]` entry if present, else the build's damages. The old
  `POST /api/build` (damages) and slot-editing `POST /api/bis` UI are gone; `POST /api/bis` now writes
  the selection only, `GET /api/builds` lists the available builds, and `GET /api/config` returns the
  resolved view (per character: build slug/name, resolved damages, per-piece annotated slots).
- **Guide stages (2026-11-17, druid)**: a build file may also carry `class`, `default_for_class` (a
  character without a `[bis]` selection gets its class default), a stage chain `next` / `switch_level` /
  `auto_switch` / `switch_note` (`Config::stage`: selected build, else class default, then follow `next`
  while `auto_switch` and clvl ≥ `switch_level` and every `switch_requires` group is met, or the stage
  was reached before (the `played.json` `stage` latch counts every group as met); cycle-guarded).
  `switch_requires` mixes plain names and any-of groups (user request, 2026-11-19):
  `["Harmony", { label = "Cannot Be Frozen", any = ["Raven Frost", "Nosferatu's Coil"] },
  { label = "Physical immunes", any = ["Lawbringer"], count = 2 }]`; a name is a group of one copy
  labelled with it; a group is met when the copies held of its items (unique/set/runeword names) sum
  to `count` (default 1). Copies are counted by `stash::Holdings` (shared with the farm list): the
  shared stash's items, the character's runeword copies and owned names (at most one per unique/set),
  the mercenary's worn runewords and identified uniques/sets (`Snapshot::merc_worn`). Unknown names
  warn (kept), an empty `any` warns and drops the group, `count = 0` warns and becomes 1, a table
  without `label` is labelled "A / B". The live JSON's `stage.next.requires` is
  `[{label, count, have, met, any: [{name, owned}]}]` (no dashboard code reads it since the stage
  strip went). Build files also carry
  `[[skill_plan]]` cumulative hard-point targets (skills panel: yours / guide-now → final, next-point
  hint), `[[recipes]]` (runeword/craft/cube/socket/upgrade; dashboard Recipes panel with ✓ from the
  shared materials + `Snapshot.held_materials`, bases via `white_bases`), `[[tips]]` by level range and
  `stats_plan`. Generated from the guide research JSON by `research/gen_build.py`.
- **Merc gear caveat (2026-11-19, mitigated)**: merc-worn items are recognised only as
  `ItemData::OWNER == NO_OWNER` + a `BODY` slot; nothing links an item to the local player's OWN
  merc. The item side has no verified owner-unit/inventory pointer (classic `pOwnerInventory`, D2R
  offset unknown), and the merc unit can't be identified: the `MonsterData +0x04` owner-id
  hypothesis never fired (`merc` journal events 2026-11-14..19: none). So in a multiplayer game,
  where other players' gear is in the client, their mercs' gear likely is too and would count in
  `runeword_copies`, `merc_worn` and `merc_equipped` — the farm status and a `switch_requires`
  group can be satisfied by someone else's merc, and the played.json latch makes such a switch
  stick. **Chosen mitigation (user decision 2026-11-19): ignore merc gear while other players
  are in the game.** `game::snapshot` keeps `merc_worn` and the merc's (`NO_OWNER`) runeword
  copies only when exactly one live player unit is a candidate; with more, both are empty, so
  neither the farm list nor a stage switch (`stash::Holdings`) can be satisfied by merc gear.
  `merc_equipped` is left as is: display only. To lift the mitigation for good, verify: in a
  2-player game, read each monster unit's `Inventory` (unit `+0x90`): its `inventory::OWNER_ID`
  (`+0x48`) and its `FIRST_ITEM` chain (`+0x10`, next at `ItemData +0xB0`) — if the hired merc's
  inventory chains its worn items and names the hiring player, collect merc gear by walking the
  merc of the chosen player instead.
- **Merc gear grid (user bug, 2026-11-20: merc wore Insight, Smoke, Bulwark; the grid proposed
  Soul Harvest / The Battlebranch as "better", Smoke with "missing Lum", no helm column)**: the
  dashboard's merc columns now tick from `merc_worn` (solo games only) like the player's from
  their worn names: worn piece ✓ (shown even past its leveling window), its build level stands in
  for the equipped requirement, and every piece ranked below it is struck through (`"worse"`, no
  "wear now"); a worn runeword drops its missing-rune note (player columns too).
  The upgrade bar and `gear_attention` never list merc pieces; a vendor match (`shop_match`)
  skips the merc's worn piece and those below it (`gear::merc_open`). The summoner build got a
  merc helm slot (Bulwark@35, `lvl 35-99`); Insight (`lvl 27-99`) and Smoke (`lvl 37-99`) no
  longer leave the merc columns at 40 / 60. Open: the overlay's merc BiS lines
  (`info::bis_lines`) still ignore `merc_worn`.
- **Fury weapons and Eternity (user, 2026-11-20)**: Fury weapon slot = eth Eternity (Archon
  Staff, natural 5os; Maxroll's S16 pick; the 3.4 Eternity is indestructible) → eth Breath of
  the Dying (Champion Axe 6os, fastest on its own) → Ribcracker (`starter`); 2026-11-20
  calculation: BotD eth Champion Axe 100 %, eth Eternity 89–93 %. Eternity is Cannot Be
  Frozen, so Raven Frost is not needed once it is worn (Raven Frost's note); the summoner's
  `switch_requires` "Cannot Be Frozen" group already listed Eternity (the Fury stage has no
  `switch_requires`).
- **Upgrades + chime**: `upgrades` in the live JSON = owned/stash pieces of the current stage wearable now
  and better than the equipped piece (no swap: the swap set is unread), plus runeword recipes whose runes
  are all owned. The page chimes (WebAudio, 🔔 toggle) when a key appears that this character hasn't had.
- [x] Guide stages, recipes panel, upgrade chime, favicon (2026-11-17).
- [x] **Dashboard sounds** (user's picks, 2026-11-20): six more WebAudio sounds beside the upgrade
  chime, each under 0.6 s and once per event. Louder: a farm-list drop on the ground (a `farm`
  mark), ready to switch stage (`stage.next.ready`: every `switch_requires` group met, or the
  stage switched on to its `next`). Quiet: an S / A trade mark on the ground, a farm entry held
  (missing → found / done, rolled entries too; a further copy of a multi-copy entry counts
  again), the route's `next.id` changing, a `farm.worth` boss gone (two frames in a row, per
  difficulty). Server side: the core's live JSON gained `drops: [{id, kind, tier}]` (the
  marked ground drops by unit id, from `Guidance::marks` via `LiveInfo::marks`), the playstyle's
  `stage.next` gained `ready`. The page (`soundEvents` in `assets/dashboard.js`) keys each sound
  by stable ids whose seen sets only grow per character, so a one-tick flicker never repeats;
  the first frame after loading, an SSE error, a menu return or a character change only
  records. Several in one frame queue (at most three, the louder first; a backlog over 2 s is
  dropped). 🔔 stays the master switch (`d2r-chime`); the ▾ popover holds one checkbox per sound
  (`d2r-sounds` in localStorage, all on by default; ticking one previews it).
- **Quest-reward recipes** (user request, 2026-11-18): a `[[recipes]]` entry may name its quest
  (`quest = "Tools of the Trade"`, `"Siege on Harrogath"`, `"Prison of Ice"`; names in
  `src/quests.rs`). The Recipes panel drops it once the reward is used (bit 0 set, bit 1 clear),
  marks it "reward ready" (bit 1) or "quest not done yet", and shows it unmarked while the flags
  are unread. `research/gen_build.py` derives the key from the recipe name (imbue/Larzuk/Anya).
- [x] Quest flags reader + quest-reward recipes (2026-11-18).
- Built runewords (user request, 2026-11-18): a `runeword` recipe is dropped from the Recipes
  panel once the character has as many copies as the current stage wants: needed = the player
  `[slots]` and `[merc_slots]` whose piece list names the runeword (at least 1; Spirit sword +
  Spirit shield needs 2), owned = `Snapshot::runeword_copies` (one entry per copy, worn, carried,
  cubed, private stash and the merc's worn ones) plus the runeword items in the shared-stash
  tabs (`playstyle::web::recipes::runeword_copies`; the sources don't overlap). Other kinds have no reliable
  "built" signal (craft results are randomly named rares, cube recipes repeat, a socketed base
  isn't told apart from a found one): only their quest rewards hide them.
- **Hot reload**: the mtime watcher watches `config.toml` AND every playstyle file
  (`Files::fingerprint`: the three places' playstyle.toml / farm.toml / builds/ listing and bis.toml;
  a cheap `read_dir` per tick — a directory's own mtime would miss in-place edits of a build file).
- **Dashboard "Where to go"**: removed 2026-11-20 (user: "the more I look at it, the more I
  dislike the where to go"); the level band, `info::LevelEntry`, `level_lines_full` and
  `build::bad_reason` are gone and the `levels` JSON field with them. See the backlog item
  "good areas at 75+".
- **Dashboard layout pass** (user requests, 2026-11-20): Sell / Keep full width at the top;
  the sell / keep / give lists carry every entry (one JSON
  line each, the page wraps them as comma runs; the old 3-line `wrap` was dashboard-only and
  is gone). `build::rate`'s Bad applies in Normal too (Good stays Nightmare+). The Gear panel folds; it opens on `gear_attention` (`playstyle::web::gear::gear_attention`: an
  owned upgrade to wear, else an unowned unique/set/runeword findable now: req ≤ clvl, qlvl ≤
  `info::reachable_level` (clvl +2 within the difficulty's area levels; also the cap of the
  natural-socket `base_hint`), a runeword's missing runes' misc.txt qlvl ≤ it; mercenary slots never)
  until the viewer clicks. Farm entries the character wears (≥ count, equipped only) for its
  current stage or a later one (`Config::stage_chain`) are hidden; its mercenary's worn
  uniques/sets and runewords (`Snapshot::merc_worn`, solo games only) count as worn too.
- **Recently played first** (user request, 2026-11-18): `played.json` in the data dir maps the
  lowercased character name → `{name, t, class?, stage?}` (unix secs last seen in a game; `src/played.rs`).
  `stage` is the guide stage slug the character last reached live: it latches a `switch_requires`
  switch (the shared stash is unread each game until opened). The file is its source of truth; the
  config page's delete and a new `POST /api/bis` selection clear it while the overlay runs. The live
  loop records the snapshot's player each tick, writing (temp file + rename) when the character
  changes or at most once a minute. Seeded once when the file is missing from the D2R save dir in the
  game's Proton prefix (`WINEPREFIX` from `/proc/<pid>/environ` → `drive_c/users/*/Saved Games/Diablo II
  Resurrected`): mtime of `<Name><account digits>.ctlo` (online) / `<Name>.d2s` (offline). A corrupt
  file is ignored (no order) and repaired by the next write. `GET /api/config` sorts characters by
  `last_played` desc (stable: the never-seen keep config order) and the page shows "played … ago".
  The dashboard has no other character list.
- [x] Config page lists recently played characters first (2026-11-18).

## Overlay
- Layer-shell surface, layer `overlay`, anchored top-right, default 400×400 (config), **empty input region** (click-through), no keyboard.
- Draw current level only: room footprints (loaded rooms brighter), player dot, waypoint (blue), exits (purple + target name),
  superuniques (gold + name if known), champion/unique packs (red), at ~5 Hz from a reader thread.
- Text stack (migrated 2026-11-17): the overlay draws only the info line (its monster levels still
  coloured by build damages). The breakpoint guide, level suggestions (where to go), keep, sell,
  give-away and `BiS` lines are dashboard-only: `live` publishes them via `GET /api/live`
  (`render::text_blocks`). The overlay hides in towns (the guide line it showed there overlapped
  the game's terror-zone list; user request, 2026-11-17).
- Exit labels (2026-11-18): each linked level's name in 12 px outlined text above its exit (on the
  tile exit preset of the linking room, else the room's centre; on the border triangle for
  walk-through borders); `render::exit_labels`.
- Marker labels (user request, 2026-11-18): short tags beside non-monster markers in the marker's
  colour lifted 35% toward white (`render::marker_label_colour`): waypoint "wp", well "well",
  shrines a 3-capital tag per type (`config::SHRINE_TAGS`, listed in `config.example.toml`),
  the real tomb's mini orifice "orif". None for star markers (quest objects and landmark stars:
  the star says enough, user request 2026-11-18), monsters, drops, stairs or cave exits.
  `info::place_labels` places labels by rank (exit > wp > orifice > shrine > well): above the
  marker, else below, right, left; a label with no free spot is dropped, exit names are always
  drawn.
- Drop tags (user request, 2026-11-22, after an A-tier Key of Terror went unnoticed: "add labels
  but short. 5os for a base with 5 sockets. 3 to 4 letters"): only marked drops (keep / farm /
  trade ring) get one, unmarked drops none. `Mark::label`, computed by the playstyle plugin
  (`d2r_playstyle_config::labels`), at most 4 characters, first match wins: a valuables rule's
  `label` (new optional field in `valuables.json`, only on certain matches: Annihilus `Anni`, Torch
  `Torc`, Shako `Shak`, Griffon's `Grif`, SoJ `SoJ`, Colossal Jewels `CJ`, Tal Rasha's `TalS`, IK
  `IK`, Trang-Oul's `Trng`, Laying of Hands `LoH`, Sigon's `Sigo`, magic / rare jewels `MJ` /
  `RJ`, magic / rare circlets `Circ`, Jeweler's Monarch `JMon`; `""` = no tag); NO tag for gems,
  runes and materials (uber keys, organs, essences, token, Standard, statues, shards, CJ
  materials; user 2026-11-22: the in-game loot filter names them, "just show it"); charms `SC` / `LC` / `GC`, unique ones `Anni` / `Torc` / `Ghee` / `Sund`
  (sunders); a socketed non-unique/set base `5os`, ethereal `e4os`;
  else the first 4 letters of the unique / set name (also for an unidentified one whose base
  carries only that item, `names::sole_name`: an unidentified Shako is Harlequin Crest) or of the
  base name ("The " skipped). Drawn 11 px in the ring's colour with the dark outline, after the
  marker tags (most valuable look first: S, A, B, farm, keep), placed outside the ring
  (`LabelSpec::clear` = ring radius + 3 px) and off the player's dot where another spot is free
  (`LabelSpec::avoid`). Not yet seen live.
- Gold coin (same request: "a gold symbol for something that has a high selling value"): a 3 px
  gold disc with a darker rim on the ring's upper left (the farm star sits at the upper right),
  on a trade-category valuables match of tier `render::COIN_MIN_TIER` (A) or better; there is no
  vendor price model, so "sells for a lot to other players" = trade tier S/A. Twink bases and B
  get none.
- XP colouring (user request, 2026-11-18): exit labels and the info line's area name are grey
  (`info::LOW_XP_TEXT`) when the area's table monster level gives the character less than
  `xp::LOW_XP` = 25% experience, white otherwise; towns, unknown levels and a terrorized current
  level stay white; the info line's monster-level build colouring is unchanged. Rule (`xp.rs`,
  Arreat Summit experience page, verified 2026-11-18): within ±5 levels 100%; monster 6/7/8/9/10+
  below 207/159/110/61/13 of 256 (81/62/43/24/5%); monster above a clvl < 25 character by
  6/7/8/9/10+: 225/174/92/38/5 of 256 (88/68/36/15/2%); clvl ≥ 25: `clvl/mlvl`. No Act 5 rule
  exists: the user's "Act 5 Normal gives ~no XP before 25" is the clvl < 25 table against mlvl
  24+ (grey up to clvl 15). The clvl 70+ penalty (976…5/1024) is the same everywhere and left out
  of the colouring (it would grey every area from clvl 86). No D2R patch is known to change this.
- Visible only while the Hyprland active window class is `steam_app_battlenet` with title `Diablo II: Resurrected`.
- User toggle (user request, 2026-11-18): `SIGUSR1` flips a "hidden by user" switch
  (`overlay::Visibility::user_hidden`); the reader and dashboard go on; a restart starts visible.
  signal-hook's self-pipe handler (safe, process-wide, so no thread can take the signal's default
  terminate action) writes a byte to a socket pair the calloop loop watches; each byte toggles and
  logs `SIGUSR1: map hidden|shown`. Hotkey: `pkill -USR1 -x d2r-map-overlay` (comm is exactly the
  15-char name); suggested binding SUPER+CTRL+M (SUPER+SHIFT+M is Omarchy's Music), see README.

## Permissions
Reading `/proc/<pid>/mem` needs `CAP_SYS_PTRACE` (ptrace_scope=1). After each release build the user runs
`sudo setcap cap_sys_ptrace=ep target/release/d2r-map-overlay` (a `just`/Makefile target). No sudo at runtime.

**Deploy snapshot (2026-11-20).** An edit of the repo's build files (the new `merc_acts` field)
hot-reloaded into the running release overlay, whose parser rejected the field: "playstyle
reloaded (0 builds)", and the stash plan said to sell the Lawbringer bases. Two fixes:
- `scripts/deploy.sh` (the user runs it): release build, copies `playstyles/` to
  `~/.local/share/d2r-map-overlay/playstyles-deployed/` (replacing the old copy), `sudo -n setcap`,
  restarts the overlay with `D2R_PLAYSTYLES_DIR` on that copy (log `~/.local/share/d2r-map-overlay/overlay.log`; it waits for the old process to exit and for the dashboard to answer), opens
  the dashboard. The live overlay never reads files being edited.
- The hot reload keeps every current build while any build file is unreadable or invalid
  (`files::reload`, like the other files' keep-current rule); before, a broken build file was
  only skipped, so two broken files left no builds.

## Plugin refactor (2026-11-20, user-approved)
The map overlay is the base. How the game is read becomes a **reader plugin**; the user's leveling
and farming strategy becomes a **playstyle plugin** (only Summoner Druid → Fury Druid). The
playstyle also owns the route ("where to go next") and the item marks on the map.

**The three parts**
- **Core (map + dashboard)**: draws the map, info line and markers; runs the dashboard page and the
  stash, skills and vendor panels; holds the game data tables, the snapshot model and the two
  plugin traits. Since step 11: `crates/core/` (`d2r-core`).
- **Reader plugin**: finds the game and reads memory (map, monsters, items, stash, quests). Since
  step 11: `crates/reader-mem/` (process, image, game walk, offset cache; the Proton platform behind
  the `proton` feature); the offsets, items table and quest record decoders are core. Split in two: a **platform layer**
  (find the process, read its memory; Proton-only, what Windows would replace) and a **game layer**
  (offsets and the structure walk; platform-independent, Windows would reuse it).
- **Playstyle plugin**: builds and stages, gear choices, farm list, route, keep/sell/trade marks,
  recipes. Since step 11: `crates/playstyle-config/` (config builds and farm, route, worth,
  valuables, item plan, gear score, marks, farm/recipe/upgrade/gear panels).

**How plugins work**: chosen at build time. Each plugin is its own Rust crate in one workspace and
a build setting (Cargo feature) picks which go in. Runtime plugins are ruled out: they need unsafe
code (forbidden) or a heavy WASM runtime. The playstyle is **mostly data files** (route, builds,
farm list, trade marks) that other players can copy and edit; they fork the code only for new logic.

**Route as data**: each step says when it applies and when it's done (conditions: difficulty, level,
quests done); the dashboard shows the current step.
1. Normal: Tristram or Countess until level 15.
2. Minimum quests to open the tombs: Sisters to the Slaughter, Tainted Sun, Arcane Sanctuary, The Summoner.
3. Tombs until 24.
4. Minimum quests up to the Ancients: Horadric Staff, Seven Tombs, Khalim's Will, The Guardian, Terror's End.
5. Baal runs until 35.
6. Valuable quests: Den of Evil, Radament, Izual, Anya, Larzuk (the socket is kept for an eth weapon + Zod), Malus.
7. Nightmare: Cain.
8. Countess until 45.
9. Rush to Mephisto.
10. Mephisto (NM mlvl 59) / Andariel (NM mlvl 49) while **worth farming**, until clvl 57.
11. Diablo (not farmed).
12. Ancients.
13. Baal waves for XP.
14. Hell: Countess.
15. From 75: max areas (level-85 areas without physical immunes, terrorized good ones first).
    Below 75, the Nightmare-style route.

**Worth farming** is computed from the game's drop tables: a boss is worth it while any item still
missing from the farm list has any chance to drop from it at its level; the dashboard names the
item. Runes count only above what the Countess drops in that difficulty (user, 2026-11-20). Needs the drop tables generated into the project (a new data step).
**Quest memory**: the game shows only the current difficulty's quests, so the overlay saves quest
progress per character and difficulty (like the build stage in `played.json`).

**Map marks** (done in step 10)
- **Keep** (green ring): bases for open runeword needs that dropped with exactly the right
  sockets and beat the base held, plus build-piece uniques and sets still wanted; **Farm**
  (green ring with a star): farm-list uniques and sets still missing.
- **Trade**: top and second tier of high-value online bases, e.g. eth elite polearms 4–5os, Monarch
  4os, Archon Plate / Dusk Shroud 3–4os, Phase Blade 5os, superior or eth elite bases. Drawn
  **dashed** while the overlay can't read their enhanced defense (unknown ED).
- **Removed**: the "Ethereal Colossus Voulge 0os (Larzuk to 4)" rule (done in step 1).

**Item places: sell / move / keep (user rule, 2026-11-20)** — playstyle logic, part of step 5's
trait; `src/playstyle/stash_plan.rs`, shown in the dashboard's Sell / Keep panel.
- Shared stash pages 1–2 are the player's own (build pieces, simple bases until something good
  drops), pages 3–4 are for trading.
- **Gems / Materials / Runes tabs** (user, 2026-11-22: "materials all go to Materials, gems to the Gems
  tab, runes to the Runes tab"): `stash::tab_of` sorts gems and skulls (all grades) to Gems, runes to
  Runes, and to Materials the uber keys (pk1–3), organs (dhn, bey, mbr), essences and Token of
  Absolution (tes, ceh, bet, fed, toa), Standard of Heroes (std), Colossal Ancient statues (ua1–5),
  Colossal Jewel upgrade materials (um1–6), worldstone shards (xa1–5) and rejuvenation potions (rvs,
  rvl; the screenshot's Materials tab holds two stacks). Such an item in the inventory, personal
  stash or on pages 1–4 gets `Action::Stow(Tab)` ("Key of Terror (inv → mats)", in the Move list,
  target `gems` / `mats` / `runes`) before any other rule: never a page move, never sold. Exceptions
  (no advice at all, never judged by other rules): anything in the cube (a recipe in progress)
  and rejuvenation potions in the inventory (drunk from there). The tab's own stacks are not judged (no
  action) but count as held; page 5 stays ignored.
- **Page 5 is the "ignore page"** (user, 2026-11-20: "things i keep around for stupidity"): its
  items get no verdict and appear in no list, and nothing is ever moved there. They still count
  as held (`Snapshot::holdings`), so a runeword on page 5 still drops its base need.
- Judged: inventory, cube, personal stash and shared pages 1–4 (pages from the session-cached
  stash; the reader now also reads stored white bases and magic / rare / crafted items with stats).
- **Keep · own**: pieces of the active build and its later stages (`BiS` slots, extras,
  `switch_requires`) and of the farm list, as many copies as needed minus the worn ones; one white
  base per (runeword, wearer, slot) need: allowed type, exactly the rune count of sockets, usable by
  the player class or the merc of the build's act that carries it (`merc_acts`); each copy of the runeword already held (worn, carried,
  stashed, merc) drops one such need, and the narrowest need picks its base first. Each need
  keeps the **best** fitting base held (user, 2026-11-20: a better base for the same runeword
  replaces the old one): higher base damage for weapons (min + max, `weapons_data::DAMAGE` from
  `gen_weapons.py`), higher max defense for armor, helms and shields; ties: superior or eth over
  plain, then the lower str + dex requirement, then the better place. The worse bases are sold,
  even one already kept on pages 1–2. Fine on pages 1–2 and in the personal stash; in the
  inventory or cube it is a move to 1–2 (user, 2026-11-20: "wrong place (inv, stash)").
- **Hands and merc acts** (user, 2026-11-20: "why replace a Broad Sword with a Zweihander? a
  2h sword can't be used by the Act 5 merc for dual wield"). One-/two-handed comes from
  weapons.txt `2handed` and `1or2handed` (`weapons_data`, `gen_weapons.py`; a barbarian player
  holds a `1or2handed` sword in one hand). A need is one-handed for the player's `weapon` slot of a
  stage that also has `offhand` pieces (`Wearer::PlayerWithOffhand`; `swap` keeps both hands) and
  for an Act 3 (shield) or Act 5 (dual wield) merc. Merc weapons per act: 1 bows (no crossbows),
  2 polearms and spears (no javelins), 3 swords + shields, 5 swords; the Act 5 merc's `offhand`
  takes a second sword. Class items (D2R patch 2.4, verified 2026-11-20): the Act 1 rogue also
  uses amazon bows (not their javelins or spears), the Act 5 merc barbarian helms (Arreat's
  Face); Act 2 and 3 none (`MercAct::class_items`).
- **Several merc acts** (user, 2026-11-20: "sometimes I play the act 1 merc especially in normal
  before I have the insight runeword ... or just for fun as she can give me the harmony buff
  too"). `merc_acts = [2, 1]` on the summoner, `[5]` on the Fury. Gear any listed act can wear
  is kept (`Scoring::merc_acts`); the merc grid and farm hints name each runeword's bases for its
  act, prefixed "Act 1 merc:" when it isn't the primary (`web::merc_base_hint`). The summoner's
  merc weapon list ends with Harmony for the rogue. **One Harmony, one base**: an alternative
  act's runeword that another need already wants (the Fury's swap Harmony) adds no second need;
  that need also takes the rogue's bases (`BaseNeed::also`, `Needs::share`): the one Harmony is
  swapped between druid and rogue. Alone, it is a need of its own. The "merc helm with life
  leech" switch group gained Arreat's Face and Demonhorn's Edge (barbarian helms).
- [x] merc acts list + amazon bows / barbarian helms (2026-11-20)
- **Loot filter** (2026-11-20): `playstyles/<name>/loot-filter.json`, a D2R in-game loot filter
  profile (n0130dy v1.5 trimmed for the druid), served byte for byte at `GET /api/loot-filter`
  (`Playstyle::loot_filter`, the user's `~/.config/d2r-map-overlay/playstyle/loot-filter.json`
  wins); the `/config` page's "Loot filter" section shows "<profile name> — loot filter of this
  playstyle" with a "Copy filter JSON" button (clipboard, else a selected read-only textarea).
  The profile is named after the playstyle (user): `playstyle.toml`'s `filter_name = "Fury
  Druid"` is the expected name; load and reload warn on stderr when the JSON's `name` differs.
- [x] loot filter copy on /config (2026-11-20) A base that fits a need except for its hands is sold with the reason
  "two-handed". A slot-less want of a runeword a build places in a slot (a `switch_requires`
  condition, a farm entry) adds no looser need, and a farm runeword its group's build places takes
  that build's needs. Hints, map marks and the merc-wearable rule use the same act.
  Act 5 dual wield: the Frenzy Barbarian merc type (added in D2R patch 2.4) dual-wields one-handed
  swords only (Blizzard forums, 2026-11-20 check); the Bash/Taunt type uses one weapon.
  The Fury fights two-handed only (Ribcracker; user, 2026-11-20: "never one handed"); both
  Lawbringers are its Act 5 merc's (`merc_act = 5`, merc weapon + offhand) until something better,
  very endgame. Live result: the two Broad Sword 3os → Keep · own, the Zweihander 3os → Sell
  "two-handed" (no 3-socket two-handed runeword need in either build).
- **Keep · trade**: valuables rules S/A (certain, or uncertain S), category trade; category twink
  only S for a white base (an eth elite 4os polearm). A twink-A white base (Bill / Poleaxe 4os,
  Crystal Sword 4os, Flail 4os, Paladin shields) is own gear, not a trade good: kept only as a
  base need, else sold. The tradable A bases are category trade in `valuables.json` (user,
  2026-11-20): Monarch 4os, Crystal Sword / Flail 5os (CTA), Berserker Axe … Hydra Bow 5–6os,
  eth elite polearm 5os, eth elite armor 4os, other elite body armor 3–4os, Grand Matron /
  Matriarchal Bow 4os. Twink-A non-bases (Sigon's set, keep one) still go to trade. A trade-worthy base goes
  to trade, never own. Belongs on pages 3–4.
- **Move**: an own keeper on pages 3–4, in the inventory or in the cube → "inv → 1–2"; a trade
  keeper anywhere else → "→ 3–4". Charms in the inventory and the materials tab stay out.
  The dashboard shows the place part of every move in red (user, 2026-11-20); the live JSON
  sends each entry's `name`, `count`, `place`, `target`, `wrong_place`, `reason` beside `text`.
- **Sell**: extra copies of own bases, worse bases for a runeword whose better base is held,
  plain bases nobody needs (twink-A ones included), uniques/sets no rule wants.
  **Give away**: B-only uniques/sets and extra keep-one copies (Sigon's).
- **Magic / rare / crafted** (user, 2026-11-20): trade rule first; then two hard rules
  (user, 2026-11-20): a weapon whose items.txt type no build of the stage chain uses → sell
  "not a build weapon", identified or not ("druids barely use javelins"). The types come from the
  chain's weapon / swap / offhand pieces and its single-item `switch_requires` groups: a unique /
  set its own base (`item_names_data::UNIQUE_CODES` / `SET_CODES`), a runeword every weapon base
  that can carry its socket count for the class (`socket_bases::carries`). Groups with
  alternatives don't count at all: their runewords (Breath of the Dying, Hustle, Eternity …)
  allow almost any weapon, and their uniques (Rune Master, The Jade Tan Do, Tomb Reaver) are
  kept by name, not for their base type. Summoner → Fury today: bow, club, hamm, mace, scep,
  staf, swor, xbow. Axes, daggers, polearms, spears, javelins, claws, orbs, wands are out. The
  merc's weapons (its act's, for any build of the chain) keep their rule (javelins never count). No builds
  known → no weapon rule; neither when a weapon or swap runeword of the chain has no weapon base
  the class can carry (an unknown class, another class's runeword): a partial type set would
  sell legitimate weapons. Identified boots without faster run/walk (stat 96) → sell "no run
  speed", whatever else they roll; unidentified boots stay. Then: unidentified or stats unread → keep own;
  identified → scored with `gear_score` for the build chain: keep own if ≥ 80% of the worn item in
  its slot or if its affixes alone score ≥ 20 (a planned skill, +1 class skill, 20 res/life);
  charms and jewels if they score at all (charms carried in the inventory stay out); else sell with
  a reason ("no build stats", "worse than worn", "other class"). The merc's gear has no stats in
  the snapshot, so a merc-wearable base (helm, body, its act's weapon) failing the player's test
  is kept unscored.
- Left alone: gems, runes, carried charms, finished runewords that aren't build pieces, quest items.
- **Teleport until Enigma** (user, 2026-11-20: "there was an amulet with teleport on it. that
  one is also in the farm list and very high for the summoner until he gets enigma. for the fury
  too, because with both I normally teleport to the endboss and then switch into whatever mode I
  need"). Teleport on an item is an oskill (stat 97, param 54) or charges (stat 204, param
  `54 << 6 | level`); a sorceress-only +Teleport (stat 107) doesn't count
  (`gear_score::grants_skill`). While the player has no Enigma of its own
  (`gear_score::teleport_wanted`: worn, inventory, cube, personal stash or the own shared pages
  1–2; one on the trade pages 3–4, on page 5 or on the merc doesn't count):
  - stash plan: an identified magic / rare / crafted item with Teleport is Keep · own, reason
    "Teleport" (Move to 1–2 from elsewhere), before the trade rules and the boots rule
    (`stash_plan::keeps_teleport`); a weapon of a type no build uses is still sold as "not a
    build weapon";
  - gear score: `Part::Teleport` weighs 40 (= +1 all skills), counted once per item, so the
    vendor panel marks a Teleport amulet better; with Enigma held it falls back to an ordinary
    skill (1);
  - farm list: `kind = "rolled"` entries (`slot`, `needs = ["Teleport"]`, `note`): "Teleport
    Amulet" first in both druid groups. Done once one is in the shared stash; found while the
    character has one (worn, inventory, cube, personal stash; merc gear has no stats); hidden
    once Enigma is held. Worth farming leaves rolled entries out (any amulet-dropping boss
    could drop one).
  - Map marks unchanged: a ground drop's stats aren't read before pickup, so no keep mark.

**User answers (2026-11-20)**
- **Smoke is ALWAYS merc gear.** The summoner keeps Stealth (from 12 until Enigma) for its FCR.
  Both druid build files put Smoke in `[merc_slots] body`; the Fury stage lists Stealth as its
  last body fallback until Fortitude.
- **Nightmare bosses (confirmed)**: Mephisto (NM level 59) and Andariel (NM level 49) are farmed
  while worth it (any chance for a missing farm-list item), until clvl 57. From clvl 55 the route
  notes Andariel as "items only, little XP": XP falls off when the monster is more than 5 levels
  below the character (81/62/43/24/5% at 6/7/8/9/10+ below, `xp.rs`). From 57: Diablo, the
  Ancients, then Shenk (61) / Eldritch (62), who give full XP from 57.
- **No Larzuk for runeword bases** (the Larzuk quest socket is kept for an eth weapon + Zod).

**Steps** (each passes the gate and leaves the overlay working; 1–7 change no visible behaviour,
8–10 are the new features)
- [x] 1. Delete the sorceress builds (only Summoner Druid → Fury Druid stay); drop the Larzuk
  valuables rule; Smoke moved to merc body in both druid builds.
- [x] 2. Small moves that untangle shared pieces: new core modules `objects`, `names`, `item_stats`; dead blood-boil branches gone from `gen_build.py`.
- [x] 3. The reader stops computing keep/sell: it returns raw items, the playstyle sorts them. Snapshot carries `owned_items` (item + `game::Place`) and every coded ground base; new `guidance::Guidance` (damages, BiS, keep/sell/give-away, drop `marks` by unit id) is computed in live.rs / `--render` and passed to render, dashboard and journal. Left for step 4: `cows`/`trail` (session data) and the other session writes into Snapshot.
- [x] 4. Reader plugin trait plus the Proton reader: `src/reader/` (game, image, items, offsets,
  offset_cache, process, quests moved there; the crate root re-exports `game`, `items`, `offsets`,
  `offset_cache`, `quests` under their old paths). `GameReader` is what live.rs and `--probe` /
  `--dump` use; `MemoryReader<P: Platform>` is the platform-independent game layer and
  `ProtonReader = MemoryReader<Proton>` the build's reader. Static dispatch (`impl GameReader`):
  the reader is picked at build time. `played.rs` no longer reads /proc: it seeds from
  `GameReader::save_dir`. Left for step 5: the `Snapshot` / post-session `Frame` split
  (`cows`, `trail` and the session's in-place merges of presets, shrines, wells, links,
  terrorized, cached stash pages), and `offset_cache::cache_dir` (also game.json's dir) moving
  to a core paths module.
- [x] 5. Playstyle plugin trait, wrapping today's builds, farm list and valuables:
  `src/playstyle/` (valuables, stash_plan, gear_score, build, guidance moved there and
  re-exported at the crate root under their old paths; the build-file / farm-file half of
  config.rs split into `playstyle/config.rs`, re-exported from `crate::config`). `Playstyle`:
  `reload(&Config)` on every hot reload, `mark_drops` (before the session merge, for the
  journal), `evaluate(&Snapshot, reached)` → `Evaluation` (guidance, the stage to latch, stage
  and chain for the dashboard), `farm()`. `ConfigDriven` is the build's playstyle (static
  dispatch, `impl Playstyle`). The area rating is computed by the playstyle into
  `Guidance::rating` (it replaced `Guidance::damages`; `info_line` takes the rating). The stage
  latch stays persisted by the core in `played.json` as one opaque string per character: the
  playstyle gets it in and returns the new one, so no separate `state()` / `restore()`. Quests
  are not an input yet (the route, step 9, needs them).
  Left for step 6: web.rs still reads the config files itself for the playstyle panels
  (`/config`, farm, recipes, tips, skills, gear verdicts via `gear_score`) and `LiveInfo` still
  takes `config::Stage` / `FarmGroup`; those panels move behind the trait with the dashboard
  split. Left for step 7: `[valuables]`, `[bis]` and `[builds]` still live in config.toml
  (parsed by `crate::config`; `ConfigDriven` holds a clone of the whole `Config`, shrines
  included) and the build / farm files sit next to it; the `playstyle/<name>/` folder gives the
  playstyle its own files and `reload(dir)`. Step 4's leftovers (the `Snapshot` / `Frame`
  split, `offset_cache::cache_dir` to a core paths module) are still open.
- [x] 6. Split the dashboard into the core page and playstyle panels: `src/web/` (core: server,
  page, stash / skills / vendor panels, `LiveInfo`, `live_json`) and `src/playstyle/web/` (the
  playstyle's panels, `/config` JSON and `[bis]` edits). New trait methods: `panels(&Evaluation,
  &LiveInfo) → Panels` (sections merged into the live JSON by key; serde_json's map is sorted,
  so the bytes are unchanged), `config_json` / `builds_json` (the server builds a fresh `P` from
  the config files per request, as before), `select_build` / `remove_character` (`config.toml`
  text edits); `farm()` is gone (`panels` reads it). `LiveInfo` no longer carries `Bis`,
  `Build`, `Stage`, `FarmGroup` or the chain. The skills panel stays core and marks the trees
  against the playstyle's plan (`SkillTarget` moved to `web::skills`, re-exported from
  `playstyle::config`); the vendor panel stays core and takes the playstyle's per-item
  `ShopNote` (build slot, `gear_score` verdict). The HTML / CSS / JS moved to `assets/`
  (`include_str!`). Verified byte-identical: every dashboard, gear, shop, stage, skills, config
  and builds JSON the test suite produces, before vs after, plus both pages
  (`served_pages_match_their_snapshot` pins the served bytes).
  Left for step 7: the core still loads `config.toml` (with `[valuables]`, `[bis]`, `[builds]`
  and the build / farm files) and hands it to `Playstyle::reload`; the server's `fresh_playstyle`
  does the same per `/config` request. With `playstyle/<name>/` the playstyle should load its own
  files (`reload(dir)` / a `load` constructor) and `select_build` / `remove_character` should edit
  its own file instead of `config.toml`. `render::TextBlocks` (sell / keep / bis lines) is still
  built by the core renderer from `Guidance` and read back by the playstyle's panels.
- [x] 7. New folder layout: `playstyles/<name>/` in the repo, selected by config.toml's
  `playstyle = "<name>"` (default and shipped example: `summoner-fury`): `playstyle.toml` (name,
  description, `[valuables]`, `[damages]`), `builds/*.toml` with their `*.source.json`
  (`research/gen_build.py <stem>` writes there by default; regenerated byte-identical to the
  user's installed files, which held no character names), `farm.toml`, `valuables.json` (moved
  from research/, kept as JSON: still `gen_valuables.py`'s output and compiled in, so it has no
  user override; a TOML conversion would change the generator and the matcher for nothing).
  The shipped folder is read at run time from `CARGO_MANIFEST_DIR` (build files hot-reload
  without a rebuild). `src/playstyle/files.rs`: per file, `~/.config/d2r-map-overlay/playstyle/`
  > the old places (config.toml's `[valuables]` / `[builds]` tables when present, `builds/` and
  `farm.toml` next to it; a notice on stderr names them) > shipped; a broken file keeps what it
  decided on reload. `[bis]` (per-character selection) is state: `bis.toml` in the data dir next
  to played.json (bis.toml wins per character over an old config.toml entry; a selection that
  sits in config.toml is edited there). `Playstyle::load(&Files)` / `reload(&Files)`;
  `select_build` / `remove_character` do their own file writes under the server's lock; the core
  config is `[shrines]` + `playstyle` only (old tables accepted unread). `serve::<P>(&Files)`:
  core web names no concrete playstyle (`main.rs` `ThePlaystyle`). Verified: `--render` of the
  flayer and Jail 1 snapshots byte-identical before / after (with the old files and shipped
  only), the page snapshot test. Route data (step 9) and marks v2 (step 10) will add their files
  to the folder.
- [x] 8. Drop tables plus "worth farming". **Data**: `research/gen_treasure.py` (d2data
  treasureclassex, monstats, superuniques, levels, itemtypes, armor, weapons, misc) →
  `src/drops_data.rs` (~117 KB; 42 deduplicated sorted code lists): `BOSS_DROPS` = per boss
  (name, level per difficulty = the drop's item level, the item codes its TC reaches per
  difficulty, bases + runes + gems + jewelry). Bosses: Andariel, Duriel, Mephisto, Diablo, Baal,
  Blood Raven, Griswold, Radament, Summoner, Izual, Nihlathak, the Countess, Bishibosh, the
  Council (one entry: all six share the `Council` TC; Toorc's Travincal level), Pindleskin,
  Shenk, Eldritch, Thresh Socket. **Rules** (generator docstring): the REPEAT-run TC (monstats
  `TreasureClass` columns; superuniques.json `TC`; Andariel has `Andarielq` in every column in
  D2R, so her repeat runs drop like the quest kill); terror-zone columns left out; a TC with a
  `group` (superuniques' "Act N Super X") is upgraded to the highest TC of its group with
  `level` ≤ mlvl, the boss's own TC only (groups span difficulties: Normal Pindleskin, level
  45, gets "Act 2 (N) Super Bx"); sub-TCs followed through every item with prob > 0; the
  auto groups `armoN` / `weapN` / `bowN` / `meleN` hold the spawnable armor/weapons of qlvl
  N-2..N whose type (with `Equiv1/2`) has itemtypes `TreasureClass = 1`. Levels: quest bosses
  (monstats `boss = 1`) their fixed monstats `Level` columns (= `levels_data::BOSSES`);
  superuniques monstats `Level` + 3 in Normal, area `MonLvlEx(N/H)` + 3 in Nightmare / Hell
  (NM: Countess 45, Shenk 61, Eldritch 62, Pindleskin 66). Unique / set base codes and qlvls
  come from the existing `item_names_data` / `item_levels_data` tables. Countess NM: her rune
  TC (Runes 8) stops at Io, her item TC's "Act 2 (N) Good" (Runes 9) reaches Ko.
  **Worth** (`src/playstyle/worth.rs`, `worth(boss, difficulty, farm, holdings, materials)` →
  `(farm item, Why)`): a unique / set whose base code drops and qlvl ≤ boss level
  (`Why::Drops`); a runeword with a still-missing rune (owned runes < per copy × copies to
  build) that drops (`Why::Runes`, those runes); **rune rule** (the user, 2026-11-20: "uniques
  and sets, plus runes above what the Countess drops in that difficulty"): for any boss but the
  Countess a rune counts only if ranked above her highest droppable rune in the same difficulty
  (Normal: Ral, Nightmare: Ko, Hell: Lo; so NM Mephisto counts Lem and up); the Countess counts every
  missing rune she drops (the rune farm stays worth it while runes are missing); without her
  data for a difficulty every rune counts; entries done or held (`farm_status` ≠
  missing) skipped. **Dashboard**: the farm JSON's `worth` section (difficulty name, bosses
  with items) renders as one line above the farm groups: "Nightmare worth farming: Andariel →
  Stealth (Tal Eth), Bloodfist, …; Mephisto → …". Every boss worth it is listed; picking which
  to run is the route's job (step 9: Mephisto / Andariel in NM until 57, Andariel "items only"
  from 55, then Diablo, the Ancients, Shenk / Eldritch). Page snapshot numbers updated.
- [x] 8b. The Countess's reliable vs rare runes (user, 2026-11-20: "show me which runes I have
  to farm that she drops reliable"). Her TC is her item TC + her own rune TC (`Countess Rune`
  / `(N)` / `(H)`: Picks 3, NoDrop 5, one item `Runes 4` / `Runes 8` / `Runes 12`, prob 15):
  the reliable runes are El..Ral / El..Io / El..Ist; higher ones (NM Ko, Hell Gul..Lo) come only
  rarely through her item TC. **Data**: `gen_treasure.py` (output first checked byte-identical
  to the committed file) adds `COUNTESS_RUNES` to `drops_data.rs`: per difficulty, each rune
  code of her rune TC with the per-kill chance in percent, 1 - (1 - p)^3 with p the chained
  per-pick chance (solo, no /players). Hell: Ko 2.35 % (1 in 43), Lem 1.21 % (1 in 82), Pul
  0.92 % (1 in 108), Um 0.62 % (1 in 162), Ist 0.42 % (1 in 238). **Worth** (`worth.rs`): the
  rules unchanged (she counts every rune she drops); `countess_runes(difficulty, worth)` splits
  her worth items' runes, each once with the entries needing it, reliable (with "1 in N") then
  rare, by rank; `countess_parts` = the text form. **Dashboard**: her farm JSON boss adds
  `reliable` / `rare` (`{rune, one_in, for}`); the worth line shows her on her own line:
  "The Countess (82) — reliable: Ko (1 in 43: Heart of the Oak, Lawbringer), …; rare: Vex
  (Heart of the Oak); items: …". **Route**: `normal-tristram` and `nm-countess` now name her
  as a `[[step.boss]]` too, so Normal and Nightmare show the list while she is worth it.
- [x] 8c. The Countess's Next-panel line, runes only (user, 2026-11-20: "too complicated for
  the next. … just the runes that I am still missing and how many for her"): "The Countess: Ko
  ×3 (1 in 43), Lem ×2 (1 in 82), Pul (1 in 108), Um (1 in 162) · rare: Vex, Lo" (no level, no
  farm entries, no items; ×1 left out; "The Countess: no runes missing" when she is worth it for
  items only). `worth::countess_missing` counts each rune she drops over the whole farm list
  (every still-missing runeword entry's runes × copies still to build, summed, minus the owned
  runes), reliable first then rare, by rank; `countess_next` = the text. The farm panel's worth
  line keeps the detailed `countess_parts` form.
- [x] 9. The route engine and a "Next" panel on the dashboard. **Data**:
  `playstyles/summoner-fury/route.toml` (user override `~/.config/d2r-map-overlay/playstyle/route.toml`
  wins whole; no old place; hot-reloaded, a broken file keeps the current route). Format (the
  file's header documents it too): `[options]` (named bool switches), `[quest_sets]` (set name →
  quest names as in `quests::QUESTS`: canyon, mephisto, ancients, valuable) and `[[step]]` in
  order, each with `id`, `title`, `when` and optional `done_when` conditions (`difficulty`
  normal / nightmare / hell, `level_min`, `level_below`, `quests_done` = set name or list,
  `options` = `["x", "!y"]`, `not_worth` = none of the step's worth bosses is worth farming), an
  action (any mix of `farm` = areas, `quests` = set or list (missing ones listed), `rush` =
  target, `[[step.boss]]` `name` / `worth` (default true: shown while worth farming, with the
  items) / `late_from` + `late_note`, `max_areas = true`) and a `note`. The **current step** is
  the first whose `when` holds and whose `done_when` doesn't; with none left in the difficulty,
  "<difficulty> done: on to <next>". The "then" line is the next later step not outleveled, its
  options on and (same difficulty only) not done. 16 steps: the user's 15, with item 15 split in
  `hell-max-areas` (from 75) and `hell-basic` (below 75: Mephisto / Andariel while worth, Baal
  waves, Shenk / Eldritch). Parse warnings: unknown quests, sets, options, bosses without drop
  data, duplicate ids. **Engine**: `src/playstyle/route.rs`, pure `current(route, Facts{level,
  difficulty, quests per difficulty, worth, max areas}) → Advice{id, title, todo, note, then}`;
  `worth_for` is the route's single call of `worth::worth` (bosses named in the route, current
  difficulty; the pending rune-count refinement changes `worth` only); `max_areas(damages)` =
  Hell areas of monster level 85 rated Good by `build::rate` (no type immune to the primary
  damage; no damages: physical). Terror zones: not readable beyond the current area's state,
  said in the step's note. **Quest memory** (core, `src/quest_log.rs`): `quests.json` in the
  data dir, per character `{name, normal, nightmare, hell}` (quest names done = reward ready or
  taken, `quests::done`); every tick's record is merged in for the current difficulty, only
  adding (temp file + rename, only when something new). A name and difficulty count only after
  3 consecutive equal observations (no half-read names, no one-tick mismatch at a difficulty
  change); until a record with words was read for the character in this difficulty the quests
  are unknown (`observe` → `None`, no Next panel). A `quests.json` that doesn't parse is moved
  to `quests.json.bad` (logged) before a fresh one is written; an unreadable one is never
  written. The live loop reads the quest record
  before `evaluate`, which now takes `Option<&QuestsDone>`; `Evaluation::route` → `PanelInfo::next` →
  the live JSON's `next` section. **Dashboard**: a compact "Next" panel at the top (title, todo
  lines, note, "then: …"); page snapshot updated.
- [x] 10. Keep and trade marks v2. `Playstyle::mark_drops(drops, &Wants)` returns marks of three
  **kinds** (`guidance::Mark { kind, trade: Option<(tier, category)>, rule, uncertain }`), drawn
  by the core per kind (`render::RingLook`): **Keep** (for the player's own builds: a mint-green
  ring), **Farm** (a still-missing farm-list unique / set: the green ring with a small green star
  at its upper right), **Trade** (the valuables tier colours, S red / A orange / B yellow).
  Dashed = uncertain. Logic in `src/playstyle/marks.rs`, reusing the item plan's "Keep · own":
  `stash_plan::plan` also returns `Wants` (computed once per evaluate, `Evaluation::wants`):
  every runeword base need with the `base_rank` of the base the plan keeps for it, and the
  unique / set pieces with copies still wanted (with their `names::named_bases_cached`, resolved
  once per name for the process; farm flag from the farm panel's `farm_status`). Per drop and
  tick: a wanted piece first (identified: by name; unidentified: by quality and base code, a
  guess: dashed, rule "Bloodfist?", several pieces on the base "A? / B?", farm ones only when
  any is farm; an S or A trade match on the same drop wins over a guess); else a white base filling an open need
  with exactly its socket count (natural on the ground) and better than the kept base, never a
  trade-worthy one; else the valuables rules of the enabled tiers, but for a white base twink
  only S (a twink-A base like a plain 4os Crystal Sword is own gear: a Keep mark when a need
  wants it, else no mark and removed, as the item plan sells it). A superior trade base is
  dashed while its roll is unknown (enhanced defense for armor / helms / shields, enhanced
  damage for weapons; ground items carry no stats). Magic / rare drops get no keep mark. `mark_drops` runs before the session merge (the
  journal records the marks), so live mode uses the previous tick's `Wants` (one tick late;
  reset to `Wants::default`, trade only, whenever the player unit differs from the one they
  were computed for: a new game or another character); `--render` / `--dump` evaluate first. Journal drop events
  gain `mark` (`keep` / `farm` / `trade`, `null`; absent in older journals), `tier` / `category`
  only for trade. Verified: `--render` of flayer, Jail 1, flayer-602-loaded and now dumps
  byte-identical before / after; new fixture `tests/fixtures/flayer-marks-snapshot.json`
  (flayer + a 4os Hunter's Bow → Keep "Harmony base", Monarch 4os → Trade A, Long Sword 0os
  → removed, unidentified unique ring → Trade S "Stone of Jordan" dashed (beats the
  "Raven Frost?" guess), unidentified unique Heavy Gloves → Farm "Bloodfist?" dashed; the
  character renamed "Testdruid") with a test in
  `config_driven.rs` and a render test for the green rings and the star.
- [x] 11. Split into a Cargo workspace; update PLAN.md, CLAUDE.md and README. Crates: `crates/core`
  (`d2r-core`), `crates/reader-mem` (`d2r-reader-mem`, feature `proton`), `crates/playstyle-config`
  (`d2r-playstyle-config`), and the binary `d2r-map-overlay` at the root (features `reader-proton`,
  `playstyle-config`, both default; a build without either is a `compile_error!`). Files moved with
  `git mv`; `src/reader/game.rs` split into the model (core `game.rs`) and the walk (reader
  `game.rs`); the crate-root re-export shims and `config.rs`'s re-exports of the playstyle's types
  are gone. To cut the cycles: `Playstyle` got associated types `Wants` and `Extra<'a>` (stage,
  chain and route advice move into `ConfigDriven`'s `Extra`; `Evaluation<'a, W, X>`); the small
  types the map and dashboard read (`Tier`, `Category`, `Spot`, `Side`, `Action`, `Advice`, `Bis`)
  moved into core `guidance.rs` (re-exported from their old playstyle modules), `Spot::page_side`
  became `stash_plan::page_side`; `Files` (where the files are, `fingerprint`, the shipped lookup)
  is core, its loaders stay in the playstyle; `Platform` lives in the reader crate (only it uses
  it), `GameReader` / `ReadMem` in core; `offset_cache::cache_dir` moved to `game_file::cache_dir`;
  the stash pages' item names use the new core `names::display_name` (core tables only) instead
  of `valuables::display_name`; `build.rs` (area rating) stays core: the info line and its tests
  use it and it needs only core tables. Dashboard test fixtures moved to `web/fixtures.rs` and,
  with `testutil` and `info::Band::plain`, are behind the core's `testutil` feature for the plugin
  crates' tests. Library crates export their items, so 102 `#[must_use]` were added and the
  nursery lint `too_long_first_doc_paragraph` (exported items only) is allowed workspace-wide.
  Verified: 500 tests as before; `--render` of jail1, flayer-marks and the scratch flayer dump
  byte-identical; the page snapshot test passes; `cargo build --release` still writes
  `target/release/d2r-map-overlay`.

## Milestones
- [x] **M0 setup**: mise Rust pin, `cargo init`, lint config, rustfmt, PLAN.md + CLAUDE.md + .gitignore, `git init`, first commit.
   From here on the user can start sessions directly in `~/Work/d2r_map_overlay`.
- [x] **M1 reader** (verified live in Jail 1, 2026-11-14; fixture tests/fixtures/jail1-snapshot.json): `d2r-map-overlay --dump` prints one JSON snapshot. Validate against `research/rooms.py` / `presets.py` output in Jail 1 and Tower 5; confirm player chain `+0x150` and the elite flag bits.
- [x] **M2 overlay**: draw a static snapshot file (no game needed) — layer-shell, click-through, iso render.
- [x] **M3 live**: reader thread + Hyprland focus hide/show; ~5 Hz redraw.
- [ ] **M4 markers**: classification tables, per-game accumulation, exit labels (exit labels done 2026-11-18, XP-coloured; marker labels 2026-11-18; Canyon tomb symbols + real tomb 2026-11-18; drop tags and the gold coin on S/A trade rings 2026-11-22, see Overlay; not yet verified live).
- [x] **M4b item drops**: implemented 2026-11-14 (items table found live; static path verified with a gem). Quality values live-confirmed 2026-11-17: set (5) on quest item `qui` and small charm `sml`, unique (7) on quest item `spc` — all with `unique_index` 0 (unidentified/no name), consistent with the index-only-when-identified rule. Named drops confirmed from the journal 2026-11-18: identified dirk `dir` = unique row 40 The Diggler; `bsd`/`gts`/`brs` = set rows 13/14/15 Isenhart's Lightbrand/Parry/Case; `chn` = set row 26 Cathan's Mesh.
- [ ] **Cleanup**: remove D2RMH exe/launcher (`~/.local/bin/d2rmh`); keep classic 1.13c files for phase 2.
- [x] **Map toggle** (2026-11-18): `SIGUSR1` hides/shows the map; key binding left to the user's Hyprland config.
- [ ] **Backlog: buff tracker** (user, 2026-11-18: "later"; v1 approved 2026-11-22): dashboard Buffs panel — active states from the state bits at stat list `+0xB30`, the buff's own stat list (tagged with its state id) for values like Cyclone Armor's remaining absorb or Feral Rage stacks, and a countdown from the list's expiry frame vs the game frame counter (counter not yet found). Needs one recorded session.
  - [x] v1 landed 2026-11-22 (not yet live-verified; the state bits are verified on monsters only, not on the player): `crates/core/src/states_data.rs` (all 232 D2R states.txt rows, `research/gen_states.py` from blizzhackers/d2data); `buffs.rs` (curated set with display names: druid, warcries, auras received, shrines, potions as good; curses and effects on the player as bad; `HIDDEN` per-hit internals 18/86/102/121/185/216); `Player::states` read every tick (`--dump` JSON `player.states`); a core dashboard panel `buffs` under Next (good chips, bad highlighted red, uncurated `state <id> <name>` muted; each line has `stacks` / `seconds_left`, `null` for now). Not per playstyle: the dashboard has no per-playstyle panel switch, so it is a core panel like Skills and Vendor.
  - [x] Research recording: journal event `buffs` (grep `"ev":"buffs"`) whenever the shown states change (the first snapshot of a game too; at most 200 per game): `added` / `removed` (+ `*_names`), all `states`, `states_copy` (the second bitfield at `+0xB50`), `lists` = the player's chain from unit `+0x88` along `+0xD0` (≤ 32 lists and a ~32 KB budget: `addr`, `owner_type`, `owner_id`, `flags`, `head` = hex of the first 0x100 bytes, `count`, `entries` `[layer, stat, value]`, at most 64 per list), `full` = the top list's aggregated array, `truncated` = a cap was hit. The live reader reads the dump only when the shown states changed (`BuffGate` in `memory_reader.rs`), and a failed state read keeps the last states (`Player::states` is `None` when unread), so one bad read neither flickers the panel nor logs every buff as removed.
  - [ ] Live session: cast Oak Sage / Cyclone Armor / Feral Rage (stack it up), take BO and a shrine, get cursed; then check that `states` match the game, whether the buff lists show up in the `+0xD0` chain (else look for them via other pointers in `head`), which header field holds the state id and the expiry frame, and where Feral Rage's stacks and Cyclone Armor's absorb sit.
  - [ ] Then: stacks / absorb values and countdowns (needs the game frame counter, still unknown) into `buffs::Line`'s `stacks` / `seconds_left`.
- [ ] **Backlog: good areas at 75+** (user, 2026-11-20: "we will bring it back when we are level 75 to show us good areas, nothing else, especially when terrorized"): from character level 75 on, a dashboard panel that lists only the areas good for the build, terrorized ones especially. The old level-band "Where to go" panel was removed the same day. Useful leftovers: `levels_data::IMMUNITIES` (per area and difficulty: monster types and immune counts per damage type), `build::rate` (Good / Neutral / Bad for the build, already colours the overlay's info line), and the Terror Zone reading (`game::is_terrorized` from the monsters' `state::DESECRATED`, per visited level only; `Session` keeps the levels seen terrorized this game; no reader for the current zone list yet). `build::bad_reason` (the immune-damage tooltip text) was deleted with the panel; it is in git history (before 2026-11-20's removal commit).
- [ ] **Phase 2 (separate plan)**: map seed RE → d2mapapi-backed full markers from entry.

## Verification
- Unit tests: iso projection, preset classification, marker cache, snapshot parsing from a captured byte fixture.
- Live (single-screen protocol: instructions first, background recording, user says "done"):
  Tower 5 (Countess marker at entry), Barracks (Smith appears when near, stays after leaving), Jail 1
  (waypoint + both exits), plus a champion pack — compare overlay screenshot with the game's Tab automap.
