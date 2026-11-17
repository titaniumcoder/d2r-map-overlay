# Handoff — agent switch notes (2026-11-17)

**`PLAN.md` is still the source of truth** for architecture, verified memory offsets, and
milestones — read it first. This file is the working-state snapshot for the next agent.

## What the user plays

Battle.net **Online** Warlock "Demonmaster" (class id 7, a real D2R patch class), level ~23 in
Nightmare, currently around Act 3 (Travincal). Online ⇒ characters and shared-stash contents are
**server-side**; the only local stash file is a 556-byte `ModernSharedStashSoftCoreV2.d2i` stub.
Game data comes from D2R process memory. The user is **fullscreen in game** — never ask blocking
chat questions; use the Plannotator Inbox (thread `msg_01M7MPZ1TVV9PV43BDQHDZFSR4`).

## Current architecture (all live)

- **Overlay** (this binary, Hyprland layer-shell, reads D2R memory): draws **only the map +
  the breakpoint guide line** (`fcr (frames/cur/next) · fhr (…) · ias N (wsm M) · res f/c/l/p`,
  res < 75 in red, monster levels coloured by build damage types). Everything else moved to the
  webapp by user decision.
- **Companion webapp** inside the same binary on `http://localhost:8471`:
  - `/dashboard` — the main surface: Where-to-go (3 columns below/equal/above clvl, band
    clvl−5…clvl+4), Gear grid (2×5: Helm Body Hands Belt Foot / Amulet Ring Weapons Off-Hand Swap),
    Sell/Keep panels, Shared-stash panel (6 tab units found; chronicle tab not shown, user doesn't
    care). Live updates via **SSE** (`/api/live-stream`, push-on-change, 8-client cap).
  - `/` — config page: per-character build **selection only** (dropdown), read-only resolved
    build view with ✓ owned / ✓(stash) / strike-through annotations.
- **Builds are agent-authored**: `~/.config/d2r-map-overlay/builds/<slug>.toml` (name, damages,
  notes, `[slots]`, `[merc_slots]`, pieces as `"Name@reqlevel"` ordered best→worst). The user's
  model: *agents author builds, the app only selects them*. `blood-boil.toml` exists (physical+fire,
  from the Maxroll Blood Boil Warlock guide prose — its planner tables are JS-rendered and NOT in
  the saved mhtml). Config selection: `[bis."Demonmaster"] build = "blood boil"`.
  There is a config backup at `~/.config/d2r-map-overlay/config.toml.bak` from the migration.
- **Shared-stash reader** (`src/stash.rs`): live tab units = player-type units whose
  `Inventory(+0x90)+0x48 == player unit id`; page-4 items attributed by `ItemData+0x0C` owner;
  session cache captured on first appearance, invalidated on player-id change; journal events
  `stash captured/invalidated`. Currently captures 6 tabs, 2 items (Sigon's set pieces in "tab 6").

## Key runtime commands

```
# build + gate (MUST pass fully before any commit — no | tail masking)
mise exec -- cargo fmt && mise exec -- cargo clippy --all-targets --all-features -- -D warnings && mise exec -- cargo test

# deploy (needs CAP_SYS_PTRACE; ask the user for every sudo command)
sudo setcap cap_sys_ptrace=ep target/release/d2r-map-overlay
nohup target/release/d2r-map-overlay > /tmp/d2r-overlay.log 2>&1 &

# watch
tail -f /tmp/d2r-overlay.log
~/.local/share/d2r-map-overlay/journal/<date>.jsonl
```

D2R runs under Proton (launcher `~/.local/bin/battlenet`, prefix `~/Games/battlenet/prefix`).
Never kill/restart D2R itself. Config: `~/.config/d2r-map-overlay/config.toml` (hot-reloaded,
toml_edit comment-preserving writes; builds dir re-read per tick).

## Workflow conventions (established, keep them)

- Strict lint config is on purpose: no `unwrap`/`expect` on Result, `unsafe` forbidden,
  `#[allow]` needs a one-line reason.
- **Reviewer subagent before every commit** — it caught real P1s repeatedly. Reviewers here have
  no shell: run the gate yourself and paste the output to them via the supervisor reply.
- Prefer **worker subagents** for implementation; the user explicitly wants subagent delegation.
  One writer per repo at a time; steer mid-run for scope changes.
- In-game testing: single-screen protocol — give ALL instructions before the user switches to the
  game, record long background windows, wait for their "done", compare against the Tab automap
  (screenshots land in `~/Pictures/Screenshots/`).

## Verification status (memory-side)

- Verified live: all offsets in `src/offsets.rs` (unit table, ItemData 0x10, QUALITY 0x00, FLAGS
  0x18 identified/eth, FILE_INDEX 0x34 — uniques AND sets, BODY 0x54, PAGE 0x55, stat-list
  chaining at StatList+0xD0, shared-tab discriminator Inventory+0x48).
- **Unverified / approximations** (documented in code):
  - runeword FLAGS bit (`0x4000000`) on equipped items — D2R value unconfirmed; failure mode is
    benign ("runeword (unnamed)", no strike-throughs in that column).
  - Stash tab numbering = unit-table walk order, NOT confirmed against the UI's tab order
    (asked the user; pending a one-line answer).
  - magic/rare equipped "required level" is the **base item's** levelreq (affixes can raise the
    real one) — grid comparison is req-level-based, not stat-based.
  - "Better/worse" = required-level proxy, per user agreement; real stat comparison is future work.

## Open work items (user-aware, none urgent)

1. **Personal stash panel** (user said "say the word" — not yet requested).
2. **Runeword socket decoding** — would name equipped runewords and enable ✓ on runeword pieces.
3. **Merc unit detection** — merc items are detectable but the merc unit/type isn't; blocks
   automatic merc-weapon filtering (e.g. Insight only on Act-2 mercs).
4. **Stat-based item comparison** to replace the req-level proxy.
5. Confirm stash tab numbering with the user; optionally the gem/rune specialized tabs.
6. IAS frames need per-class attack animation lengths (maxroll/planner data) — Warlock anim
   lengths only in planner data.

## Probes / research

`research/*.py` are read-only Python probes (`sudo python3 -I research/<x>.py <pid> <addr>` —
ask for sudo each time; passwordless sudo is currently available). Generated data lives in
`src/*_data.rs` via `research/gen_*.py` (deterministic, re-runnable; run `cargo fmt` after).
`research/notes/stash-format.md` documents the on-disk `.d2i` format (offline path) and the
in-memory tab-unit strategy (the one we use).

## Commits from today's session (2026-11-17)

651c047→5440a6a (breakpoints/BiS groundwork), 6c48d27/ca3891e/15cb8cc/8cbed9e (level band),
74bfdbb/5a70d68 (landmarks/town frame), 8c23659 (party on map), 5ad000d (WSM), b1f9be3 (res red),
b6eb768 (✓ owned), 936cc1f/491e929 (webapp + runeword hints), 33b477b (live dashboard),
22c0344/1392628 (stash), 0e89209 (SSE header fix), 810dbb2 (band +5/−4, checkbox fixes),
0cb57cc (agent-authored builds), baff977 (overlay = map + guide), 5169775 (SSE + 2×5 gear grid).

Every commit passed the full gate + an independent reviewer pass. 295 unit tests currently green.
