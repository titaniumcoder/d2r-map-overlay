# d2r-map-overlay

Read-only minimap overlay for Diablo II: Resurrected running under Proton on Hyprland.
It reads D2R's memory from Linux and draws a click-through corner minimap: level layout, player,
waypoint, entry/exit, packs, uniques, superuniques. Normal monsters are out of scope.

**`PLAN.md` is the source of truth** for architecture, verified memory offsets, and milestones.
Read it before starting work; tick its milestone checkboxes as work lands.

## Layout (Cargo workspace)

- `crates/core/` (`d2r-core`): snapshot model, game data tables, map render, overlay, session,
  journal, live loop, dashboard core, and the plugin traits (`reader::GameReader`,
  `playstyle::Playstyle`) with the types they exchange (`guidance.rs`).
- `crates/reader-mem/` (`d2r-reader-mem`): the memory reader (structure walk, image, offset cache,
  `Platform`); the Proton platform behind its `proton` feature.
- `crates/playstyle-config/` (`d2r-playstyle-config`): `ConfigDriven`, the playstyle read from
  `playstyles/<name>/` (builds, farm list, route, item rules, its dashboard panels).
- `src/main.rs`: the binary `d2r-map-overlay`, wiring one reader and one playstyle picked by Cargo
  features (`reader-proton`, `playstyle-config`, both default). Kept at the root so the release
  binary stays `target/release/d2r-map-overlay`.

Plugins depend on core, never on each other (a dev-dependency for tests is fine); core depends
on no plugin. Core test helpers for plugin tests sit behind core's `testutil` feature.

## Gate (every commit)

```
cargo fmt --all --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace
```

The lint config (`[workspace.lints]` in the root `Cargo.toml`, inherited by every crate) is strict on purpose (pedantic deny, `unsafe_code = "forbid"`, no `unwrap`).
Satisfy it with code changes; an `#[allow]` needs a one-line reason beside it.
Have the `reviewer` agent run the gate and review the diff before committing.

## Git

Commit small and often: at least one commit per milestone sub-step, each passing the gate.
Remote: `origin` = github.com/titaniumcoder/d2r-map-overlay (public). Push to `origin main` right after each commit; no need to ask.

## Live game

- D2R runs in the Proton prefix `~/Games/battlenet/prefix` (launcher `~/.local/bin/battlenet`).
  The game process's cmdline is `C:\Program Files (x86)\Diablo II Resurrected\D2R.exe`; its comm is `Main`.
- Reading `/proc/<pid>/mem` needs `CAP_SYS_PTRACE` (`ptrace_scope=1`). After a release build the user runs
  `sudo setcap cap_sys_ptrace=ep target/release/d2r-map-overlay`. Ask the user for every sudo command;
  the agent has no password (any passwordless grant is temporary).
- `research/*.py` are the Python probes that established the offsets: run with `sudo python3 -I <script> …`;
  they are the reference to diff the Rust reader against.

## Testing in game: single-screen protocol

The user has one screen and can't read the chat while playing. So:
1. Give all in-game instructions **before** they switch to the game.
2. Record in the background with a long window (minutes), never a short foreground sample.
3. Wait for the user's "done" message, then analyse.

Compare overlay output against the game's Tab automap (the user can save screenshots to
`~/Pictures/Screenshots/`).
