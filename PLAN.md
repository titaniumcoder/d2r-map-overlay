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
  other players in a group game (PlayerData `+0x10`, name at +0).
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

## Flayer Jungle (78), 2026-11-15
- 192 rooms in 9 large piece types (16 or 32 Room2 each): 534, 535, 538, 539, 565, 570, 574, 601, 602.
- **601 = waypoint area** (player and waypoint object 237 inside it). **602 = Gidbinn altar candidate** (only other
  outlier id) — **confirmed**: the Gidbinn altar (object 252) is in 602; landmark rule added.

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
  character screen exactly (res 6/9/8/7, fire/light/cold/poison). `ItemData::OWNER` (+0x0C) separates the
  player's gear from the mercenary's (merc items carry `NO_OWNER`, and two body slots were the merc's
  duplicate armour/weapon). The overlay therefore sums gear stats over equipped items and carried charms
  (charm codes cm1/cm2/cm3) per owner. Monsters keep full lists at +0x88 (a mob's list had its
  res/experience/level inline).

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
  `+0x55` page (0 inventory, 4 stash, 255 elsewhere; verified over 74 items). Inventory = mode 0 + page 0.
- Item stat list: unit `+0x88` → `+0x30` entries / `+0x38` count, entry {u16 layer, u16 id, u32 value}; sockets = 194.
- Valuables: research/valuables.json (67 rules, S/A/B, trade/twink) embedded; tiers per category in
  `~/.config/d2r-map-overlay/config.toml` (default S+A both; example: config.example.toml). Ground: ring by tier
  (S red, A orange, dashed = uncertain). Inventory keep list (dashboard; formerly green lines on the overlay): certain
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

## Build definitions and BiS selection (2026-11-17)
- **Agent-authored build files**: `~/.config/d2r-map-overlay/builds/<slug>.toml`, one build per file
  (schema: `name`, `damages`, `notes`, `[slots]`, `[merc_slots]` — pieces as `"Name@<required level>"`,
  ordered best→worst). Loaded by `config::load_builds(dir)` (missing dir = no builds, no error), keyed by
  lowercased build name — the slug a selection refers to. The webapp does NOT edit them.
- **Selection-only config**: the webapp writes ONLY `build = "<slug>"` into `[bis."<char>"]` (toml_edit,
  comment-preserving; the tables are created if missing). Resolution order for a character's BiS:
  inline slots (legacy, hand-edited) if present, else the selected build's slots, else none. Same for
  damages: character-level `[builds]` entry if present, else the build's damages. The old
  `POST /api/build` (damages) and slot-editing `POST /api/bis` UI are gone; `POST /api/bis` now writes
  the selection only, `GET /api/builds` lists the available builds, and `GET /api/config` returns the
  resolved view (per character: build slug/name, resolved damages, per-piece annotated slots).
- **Guide stages (2026-11-17, druid)**: a build file may also carry `class`, `default_for_class` (a
  character without a `[bis]` selection gets its class default), a stage chain `next` / `switch_level` /
  `auto_switch` / `switch_note` (`Config::stage`: selected build, else class default, then follow `next`
  while `auto_switch` and clvl ≥ `switch_level`; cycle-guarded; the user chose all-manual switches),
  `[[skill_plan]]` cumulative hard-point targets (skills panel: yours / guide-now → final, next-point
  hint), `[[recipes]]` (runeword/craft/cube/socket/upgrade; dashboard Recipes panel with ✓ from the
  shared materials + `Snapshot.held_materials`, bases via `white_bases`), `[[tips]]` by level range and
  `stats_plan`. Generated from the guide research JSON by `research/gen_build.py`.
- **Upgrades + chime**: `upgrades` in the live JSON = owned/stash pieces of the current stage wearable now
  and better than the equipped piece (no swap: the swap set is unread), plus runeword recipes whose runes
  are all owned. The page chimes (WebAudio, 🔔 toggle) when a key appears that this character hasn't had.
- [x] Guide stages, recipes panel, upgrade chime, favicon (2026-11-17).
- **Quest-reward recipes** (user request, 2026-11-18): a `[[recipes]]` entry may name its quest
  (`quest = "Tools of the Trade"`, `"Siege on Harrogath"`, `"Prison of Ice"`; names in
  `src/quests.rs`). The Recipes panel drops it once the reward is used (bit 0 set, bit 1 clear),
  marks it "reward ready" (bit 1) or "quest not done yet", and shows it unmarked while the flags
  are unread. `research/gen_build.py` derives the key from the recipe name (imbue/Larzuk/Anya).
- [x] Quest flags reader + quest-reward recipes (2026-11-18).
- **Hot reload**: the mtime watcher watches `config.toml` AND the `builds/` directory (a cheap
  `read_dir` per tick — a directory's own mtime would miss in-place edits of a build file).
- **Dashboard "Where to go"**: the level-suggestion band is structured (`mlvl` + `bucket`:
  `below`/`equal`/`above` clvl, `info::LevelEntry` at the `live_json` layer, `level_lines_full` the
  source) and the panel lays it out in three labelled columns (user request, 2026-11-17).

## Overlay
- Layer-shell surface, layer `overlay`, anchored top-right, default 400×400 (config), **empty input region** (click-through), no keyboard.
- Draw current level only: room footprints (loaded rooms brighter), player dot, waypoint (blue), exits (purple + target name),
  superuniques (gold + name if known), champion/unique packs (red), at ~5 Hz from a reader thread.
- Text stack (migrated 2026-11-17): the overlay draws only the info line (its monster levels still
  coloured by build damages). The breakpoint guide, level suggestions (where to go), keep, sell,
  give-away and `BiS` lines are dashboard-only: `live` publishes them via `GET /api/live`
  (`render::text_blocks`). The overlay hides in towns (the guide line it showed there overlapped
  the game's terror-zone list; user request, 2026-11-17).
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
- [ ] **M4b item drops**: implemented 2026-11-14 (items table found live; static path verified with a gem). Quality values live-confirmed 2026-11-17: set (5) on quest item `qui` and small charm `sml`, unique (7) on quest item `spc` — all with `unique_index` 0 (unidentified/no name), consistent with the index-only-when-identified rule. Ticked once a named unique/set drop shows a nonzero index.
- [ ] **Cleanup**: remove D2RMH exe/launcher (`~/.local/bin/d2rmh`); keep classic 1.13c files for phase 2.
- [ ] **Phase 2 (separate plan)**: map seed RE → d2mapapi-backed full markers from entry.

## Verification
- Unit tests: iso projection, preset classification, marker cache, snapshot parsing from a captured byte fixture.
- Live (single-screen protocol: instructions first, background recording, user says "done"):
  Tower 5 (Countess marker at entry), Barracks (Smith appears when near, stays after leaving), Jail 1
  (waypoint + both exits), plus a champion pack — compare overlay screenshot with the game's Tab automap.
