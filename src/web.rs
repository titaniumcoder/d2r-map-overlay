//! The companion web app: a small HTTP server on `localhost` that lets the user select each
//! character's build in a browser form, writing ONLY the `[bis.<name>]` `build` selection back
//! to `config.toml` with comments preserved (user decision, 2026-11-17). The gear itself is
//! authored by the agent in `builds/*.toml` and shown read-only, annotated with the live ✓
//! (owned / in the shared stash) and strike-through where the equipped piece is already
//! better. The overlay watches the config file and the `builds/` directory and hot-reloads
//! them (see `live`). The app also serves the live dashboard: the level, keep, sell, give-away
//! and `BiS` blocks and the breakpoint guide (the overlay draws only the map and info line),
//! published by `live` each tick and read by `GET /api/live`.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use toml_edit::DocumentMut;

/// The port the page listens on (localhost only).
pub const PORT: u16 = 8471;

/// Requests whose `Content-Length` is larger are rejected before the body is read or
/// allocated: the client controls that header, and a huge allocation aborts the process.
const MAX_BODY: usize = 1 << 20;
/// Read timeout per request step: a stalled client must not hold a thread forever.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Concurrent `/api/live-stream` handlers: each runs a thread, and a stuck tab (the reader
/// stops but the writes never fail — a long-lived SSE write loop has no read timeout to
/// catch it) must not multiply threads unboundedly. Above the cap a new stream is refused
/// with 503; the browser's `EventSource` retries until a slot frees up.
const MAX_SSE_CLIENTS: usize = 8;
/// How often the SSE loop polls the live cell (a cheap string compare).
const SSE_POLL: Duration = Duration::from_millis(250);
/// How often the SSE loop writes a `: keepalive` comment.
const SSE_KEEPALIVE: Duration = Duration::from_secs(15);
/// Write timeout for SSE streams: a vanished client releases its cap slot here instead of
/// holding it until TCP retransmission gives up.
const SSE_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Concurrent SSE handlers right now (see [`MAX_SSE_CLIENTS`]).
static SSE_CLIENTS: AtomicUsize = AtomicUsize::new(0);

/// Serializes the read-edit-write of `config.toml` across connection threads, so two saves
/// cannot both start from the same old text and lose one of the updates.
static CONFIG_WRITE: Mutex<()> = Mutex::new(());

/// The pre-serialized JSON of the latest live snapshot, published by `live` each tick and read
/// by `GET /api/live`. `None` while no game is attached: the dashboard then shows
/// [`NO_LIVE_JSON`]. A [`RwLock`] in a [`OnceLock`]: written ~5 Hz by the live loop, read on
/// request; both hold it only for the swap/clone.
static LIVE: OnceLock<RwLock<Option<String>>> = OnceLock::new();

/// The live gear state the config page's resolved `BiS` view is annotated from (published by
/// `live` each tick, [`publish_gear`]): the names the character owns, the shared-stash names,
/// and the equipped `(body slot, quality)` pairs of the character and its mercenary. Stays at
/// the last game's values after the game exits (ownership changes slowly); empty before the
/// first game, so nothing is ticked or struck through then.
#[derive(Debug, Clone, Default)]
pub struct GearInfo {
    pub owned: Vec<String>,
    pub stash: Vec<String>,
    pub equipped: Vec<(u8, u32)>,
    pub merc_equipped: Vec<(u8, u32)>,
}

/// The shared cell behind [`current_gear`]/[`publish_gear`].
static GEAR: OnceLock<RwLock<GearInfo>> = OnceLock::new();

/// Replaces the gear state the config page annotates its resolved `BiS` view from. Called by
/// `live` every tick; a brief write lock over a few short strings.
pub fn publish_gear(gear: GearInfo) {
    // A poisoned lock still holds a working cell; a poisoned writer can only mean a panic
    // elsewhere while writing, and the next tick overwrites the value anyway.
    *gear_cell().write().unwrap_or_else(PoisonError::into_inner) = gear;
}

/// The shared cell behind [`GEAR`].
fn gear_cell() -> &'static RwLock<GearInfo> {
    GEAR.get_or_init(|| RwLock::new(GearInfo::default()))
}

/// A snapshot of [`GearInfo`] (cloned out under a brief read lock).
fn current_gear() -> GearInfo {
    // See [`publish_gear`] for the poison handling.
    gear_cell().read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// The answer of `GET /api/live` while no game is attached (or between games).
const NO_LIVE_JSON: &str = "{\"live\": false}";

/// The character classes by txt row id, in [`crate::breakpoints`] per-class table order.
const CLASS_NAMES: [&str; 8] = [
    "amazon",
    "sorceress",
    "necromancer",
    "paladin",
    "barbarian",
    "druid",
    "assassin",
    "warlock",
];

/// The shared cell behind [`LIVE`].
fn live_cell() -> &'static RwLock<Option<String>> {
    LIVE.get_or_init(|| RwLock::new(None))
}

/// Replaces the live snapshot JSON the dashboard serves. Called by `live` every tick; a brief
/// write lock, a short string. A byte-identical value is not stored: the SSE handlers compare
/// the cell against what they last wrote, so an unchanged tick would only cost them a poll.
pub fn publish_live(json: String) {
    // A poisoned lock still holds a working cell; a poisoned writer can only mean a panic
    // elsewhere while writing, and the next tick overwrites the value anyway. Read-compare-
    // write under one lock so a concurrent publish can't slip between the two steps.
    let mut cell = live_cell().write().unwrap_or_else(PoisonError::into_inner);
    if cell.as_deref() != Some(json.as_str()) {
        *cell = Some(json);
    }
}

/// Drops the live snapshot JSON: the game is gone or in the menu, so the dashboard must show
/// "waiting for game" instead of stale data.
pub fn clear_live() {
    // See [`publish_live`] for the poison handling.
    *live_cell().write().unwrap_or_else(PoisonError::into_inner) = None;
}

/// Writes the `[bis.<name>]` build selection into `text` (comment-preserving): `Some(slug)`
/// sets `build = "<slug>"` and creates the `[bis]`/`[bis.<name>]` tables when missing;
/// `None` removes the selection again — the `build` key, and the whole entry when that leaves
/// it empty. `None` (the return) when `text` is not valid TOML or the `bis` entry is not a
/// table: the save would silently do nothing, so the caller must answer the browser with an
/// error instead.
#[must_use]
pub fn set_bis_selection(text: &str, name: &str, build: Option<&str>) -> Option<String> {
    let mut doc = text.parse::<DocumentMut>().ok()?;
    if let Some(slug) = build {
        if doc.get("bis").is_none() {
            doc.insert("bis", toml_edit::Item::Table(toml_edit::Table::new()));
        }
        let bis = doc.get_mut("bis").and_then(|b| b.as_table_mut())?;
        if !bis.contains_key(name) {
            bis.insert(name, toml_edit::Item::Table(toml_edit::Table::new()));
        }
        let entry = bis.get_mut(name).and_then(|e| e.as_table_mut())?;
        entry["build"] = toml_edit::value(slug);
    } else if let Some(bis) = doc.get_mut("bis").and_then(|b| b.as_table_mut()) {
        let Some(entry) = bis.get_mut(name).and_then(|e| e.as_table_mut()) else {
            // Same contract as the set path: a non-table `[bis.<name>]` would silently no-op,
            // so the caller must answer the browser with an error instead.
            return None;
        };
        entry.remove("build");
        if entry.is_empty() {
            bis.remove(name);
        }
    }
    Some(doc.to_string())
}

/// Serves the page on `localhost:PORT` until the process ends. Errors are logged, not fatal:
/// the map keeps working without the page.
pub fn serve() {
    let Ok(listener) = TcpListener::bind(("127.0.0.1", PORT)) else {
        eprintln!("web: could not bind localhost:{PORT}; the page is off");
        return;
    };
    eprintln!("web: dashboard at http://localhost:{PORT} (config at /config)");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                // One thread per connection: a stalled or slow client cannot hang the accept
                // loop (and so every later request) behind it.
                if let Err(e) = thread::Builder::new()
                    .name("web".into())
                    .spawn(move || handle(stream))
                {
                    eprintln!("web: could not start a handler thread: {e}");
                }
            }
            Err(e) => eprintln!("web: {e}"),
        }
    }
}

/// One parsed request, or the reason it must not be served.
enum Parsed {
    /// method, path, body.
    Request(String, String, Vec<u8>),
    /// `Content-Length` above [`MAX_BODY`]: answer 413 without allocating.
    TooLarge,
}

/// Reads one request from `reader`: method, path, body. `Content-Length` is matched
/// case-insensitively. `None` on a malformed or unreadable request: the connection is
/// dropped without an answer.
fn read_request(reader: &mut impl BufRead) -> Option<Parsed> {
    let mut request = String::new();
    reader.read_line(&mut request).ok()?;
    let mut words = request.split_whitespace();
    let (Some(method), Some(path)) = (words.next(), words.next()) else {
        return None;
    };
    let mut length = 0usize;
    for line in reader.by_ref().lines().map_while(Result::ok) {
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = v.trim().parse().unwrap_or(0);
        }
    }
    if length > MAX_BODY {
        return Some(Parsed::TooLarge);
    }
    let mut body = vec![0; length];
    if length > 0 && reader.read_exact(&mut body).is_err() {
        return None;
    }
    Some(Parsed::Request(method.to_owned(), path.to_owned(), body))
}

/// Reads one request and answers it; the thread ends with the connection.
fn handle(stream: TcpStream) {
    let peer = stream
        .peer_addr()
        .map_or_else(|_| String::new(), |a| a.to_string());
    // Per-step read timeout: a client that connects and sends nothing (or half a request)
    // times out instead of holding its thread (and its [`CONFIG_WRITE`] slot) forever.
    if let Err(e) = stream.set_read_timeout(Some(READ_TIMEOUT)) {
        eprintln!("web: {peer}: no read timeout: {e}");
    }
    let Ok(mut reader) = stream.try_clone().map(BufReader::new) else {
        return;
    };
    let Some(parsed) = read_request(&mut reader) else {
        return;
    };
    let response = match parsed {
        Parsed::TooLarge => respond("413 Content Too Large", "text/plain", "request body too large\n"),
        Parsed::Request(method, path, body) => match (method.as_str(), path.as_str()) {
            ("GET", "/" | "/index.html" | "/dashboard" | "/config") => html_page(&path),
            ("GET", "/api/config") => api_config(),
            ("GET", "/api/builds") => api_builds(),
            ("GET", "/api/live") => api_live(),
            // The stream owns the connection from here on; nothing is written after it.
            ("GET", "/api/live-stream") => {
                api_live_stream(stream);
                return;
            }
            ("POST", "/api/bis") => api_bis(&String::from_utf8_lossy(&body)),
            _ => respond("404 Not Found", "text/plain", "not found\n"),
        },
    };
    let mut stream = stream;
    if stream.write_all(response.as_bytes()).is_err() {
        eprintln!("web: {peer}: could not answer");
    }
}

/// `200 OK` with `body` as `content_type`; `Connection: close` because one request per
/// connection is all the page does.
fn respond(status: &str, content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// The response for one of the HTML pages: the dashboard is the start page (user request,
/// 2026-11-17; `/dashboard` stays as an alias for old bookmarks), `/config` the build
/// selection page; any other path is a 404.
fn html_page(path: &str) -> String {
    match path {
        "/" | "/index.html" | "/dashboard" => respond("200 OK", "text/html", &dashboard_page()),
        "/config" => page(),
        _ => respond("404 Not Found", "text/plain", "not found\n"),
    }
}

/// The form page (see `config_page` for the embedded source).
fn page() -> String {
    respond("200 OK", "text/html", config_page())
}

/// `200 OK` with the latest live snapshot's JSON verbatim (published by `live`), or
/// [`NO_LIVE_JSON`] while no game is attached. The page handles both.
fn api_live() -> String {
    let json = live_cell()
        .read()
        // See [`publish_live`] for the poison handling.
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    // `no-store`: the snapshot is per-tick, caching it would show stale game state.
    respond_no_store(
        "200 OK",
        "application/json",
        json.as_deref().unwrap_or(NO_LIVE_JSON),
    )
}

/// `GET /api/live-stream`: the live JSON as a Server-Sent-Events stream. The headers go out
/// once, then the loop writes `data: <json>\n\n` whenever the published JSON differs from
/// the last one written (a cheap string compare every [`SSE_POLL`]; [`publish_live`] skips
/// byte-identical stores), a `: keepalive\n\n` comment every [`SSE_KEEPALIVE`], and exits on
/// the first write error — the browser's `EventSource` then reconnects on its own. Plain raw
/// writes, no chunked encoding (the message boundary is the SSE `\n\n`); a `live: false`
/// event is sent once when the game detaches, so the page clears its stale frame.
/// Refused with 503 above [`MAX_SSE_CLIENTS`] concurrent handlers.
fn api_live_stream(mut stream: TcpStream) {
    /// Decrements [`SSE_CLIENTS`] on drop — also on an early return or unwind, so a
    /// handler's slot is always freed. The fetch-add itself is the caller's first step.
    struct Slot;
    impl Drop for Slot {
        fn drop(&mut self) {
            SSE_CLIENTS.fetch_sub(1, Ordering::Relaxed);
        }
    }
    let slot = Slot;
    if SSE_CLIENTS.fetch_add(1, Ordering::Relaxed) >= MAX_SSE_CLIENTS {
        drop(slot);
        let _ = stream.write_all(
            b"HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\nContent-Length: 9\r\nConnection: close\r\n\r\nno slots\n",
        );
        return;
    }
    // Unbuffered: every `data:` line must reach the browser as it is written.
    let mut stream = stream;
    if let Err(e) = stream.set_nodelay(true) {
        eprintln!("live-stream: no nodelay: {e}");
    }
    // A write timeout releases the slot of a vanished client (laptop sleep, dropped
    // network) instead of holding it until TCP retransmission gives up.
    if let Err(e) = stream.set_write_timeout(Some(SSE_WRITE_TIMEOUT)) {
        eprintln!("live-stream: no write timeout: {e}");
    }
    if stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
    )
    .is_err()
    {
        return;
    }
    let mut last: Option<String> = None;
    let mut keepalive = Instant::now();
    loop {
        thread::sleep(SSE_POLL);
        let current = live_cell()
            .read()
            // See [`publish_live`] for the poison handling.
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        // Equal to what was last written (nothing published yet, same JSON, or detached
        // twice): nothing to send. A changed value — including the game detaching, which
        // publishes the `live: false` marker — goes out as one event.
        let send = if current == last {
            None
        } else {
            Some(current.as_deref().unwrap_or(NO_LIVE_JSON).to_owned())
        };
        if let Some(body) = send {
            if write_chunk(&mut stream, &format!("data: {body}\n\n")).is_err() {
                return;
            }
            last = current;
            keepalive = Instant::now();
        } else if keepalive.elapsed() >= SSE_KEEPALIVE {
            // Reset only after a successful write, or an idle stream would keepalive every
            // poll interval (250 ms) instead of every [`SSE_KEEPALIVE`].
            if write_chunk(&mut stream, ": keepalive\n\n").is_err() {
                return;
            }
            keepalive = Instant::now();
        }
    }
}

/// Writes one SSE chunk and flushes it; an error is the connection's end.
fn write_chunk(stream: &mut TcpStream, chunk: &str) -> std::io::Result<()> {
    stream.write_all(chunk.as_bytes())?;
    stream.flush()
}

/// [`respond`] plus `Cache-Control: no-store`, for endpoints whose answer is only valid
/// for the instant it was produced.
fn respond_no_store(status: &str, content_type: &str, body: &str) -> String {
    let mut response = respond(status, content_type, body);
    // Insert before the blank line that ends the headers; the leading `\r\n` opens a new
    // header line, so the blank line (and the body offset it implies) stays intact.
    let split = response.find("\r\n\r\n").unwrap_or(0);
    response.insert_str(split, "\r\nCache-Control: no-store");
    response
}

/// `200 OK` with the current config as JSON for the page: each character with its resolved
/// `BiS` view (selected build, resolved damages and per-piece annotated slots) and, for
/// characters only known from `[builds]`, their saved damage types ([`config_json`]).
fn api_config() -> String {
    let config = crate::config::load_from(&crate::config::default_path().unwrap_or_default());
    let gear = current_gear();
    respond(
        "200 OK",
        "application/json",
        &config_json(&config, &gear).to_string(),
    )
}

/// `200 OK` with the available build definitions as JSON for the page's selects
/// ([`builds_json`]).
fn api_builds() -> String {
    let config = crate::config::load_from(&crate::config::default_path().unwrap_or_default());
    respond("200 OK", "application/json", &builds_json(&config).to_string())
}

/// The config page's JSON ([`api_config`]), pure over the parsed config and the live gear
/// state so tests need no file and no game.
///
/// Each character carries:
/// - `name`, and `build`: the selected build's slug as written in `[bis.<name>]` (`""` when
///   none),
/// - `build_name`: the selected build's display name (`null` when none or unknown),
/// - `damages`: the resolved damage types (character-level `[builds]` entry, else the
///   selected build's), as lowercased names,
/// - `slots`/`merc_slots`: the resolved per-slot piece lists — inline slots when the config
///   has any, else the selected build's ([`crate::config::Config::resolved_bis`]) — each piece
///   annotated with its required level, its ✓ (`tick`: `"owned"`/`"stash"`/`null`,
///   [`crate::info::piece_tick`]), whether the equipped piece is already better (`better`,
///   [`crate::info::slot_equipped_better`]) and, for a runeword piece, its `runes` and
///   `bases`.
#[must_use]
fn config_json(config: &crate::config::Config, gear: &GearInfo) -> serde_json::Value {
    let damages = |name: &str| {
        config
            .damages_for_player(name, None)
            .iter()
            .map(|d| d.name())
            .collect::<Vec<_>>()
    };
    // The resolved `BiS` view plus the selection as written (the slug the page's select
    // sets back; `resolved_bis` reports the display name, not the slug).
    let bis_named = |name: &str| {
        let resolved = config.resolved_bis(name).unwrap_or_else(|| crate::config::Bis {
            build: String::new(),
            slots: BTreeMap::new(),
            merc_slots: BTreeMap::new(),
        });
        let build = config.bis.get(name).map_or(String::new(), |b| b.build.clone());
        Some((
            build,
            slot_tables_json(&resolved.slots, &gear.equipped, gear),
            slot_tables_json(&resolved.merc_slots, &gear.merc_equipped, gear),
        ))
    };
    let characters: Vec<_> = config
        .bis
        .keys()
        .chain(
            config
                .builds
                .names
                .keys()
                // Case-insensitive: [`crate::config::parse`] lowercases both `builds` names and
                // `bis` keys; the filter is also correct for programmatically built `Config`s.
                .filter(|n| !config.bis.keys().any(|b| b.eq_ignore_ascii_case(n))),
        )
        .map(|name| {
            let (build, slots, merc_slots) = bis_named(name)
                .unwrap_or_else(|| (String::new(), serde_json::json!({}), serde_json::json!({})));
            let build_name = config
                .selected_build(name)
                .map_or(serde_json::Value::Null, |def| serde_json::json!(def.name));
            serde_json::json!({
                "name": name,
                "build": build,
                "build_name": build_name,
                "damages": damages(name),
                "slots": slots,
                "merc_slots": merc_slots,
            })
        })
        .collect();
    serde_json::json!({ "characters": characters })
}

/// A resolved slot table as JSON: slot name → annotated pieces ([`piece_json`]).
fn slot_tables_json(
    slots: &BTreeMap<String, Vec<String>>,
    equipped: &[(u8, u32)],
    gear: &GearInfo,
) -> serde_json::Value {
    slots
        .iter()
        .map(|(slot, pieces)| {
            (
                slot.clone(),
                serde_json::json!(
                    pieces
                        .iter()
                        .map(|piece| piece_json(piece, slot, equipped, gear))
                        .collect::<Vec<_>>()
                ),
            )
        })
        .collect::<serde_json::Map<String, serde_json::Value>>()
        .into()
}

/// One resolved `BiS` piece as JSON (see [`config_json`] for the fields).
fn piece_json(piece: &str, slot: &str, equipped: &[(u8, u32)], gear: &GearInfo) -> serde_json::Value {
    let name = crate::info::piece_name(piece);
    let mut json = serde_json::json!({
        "piece": piece,
        "name": name,
        "level": crate::info::piece_level(piece),
        "tick": crate::info::piece_tick(piece, &gear.owned, &gear.stash)
            .map(crate::info::Tick::as_str),
        "better": crate::info::slot_equipped_better(slot, equipped),
    });
    if let Some(rw) = crate::runewords_data::lookup(name)
        && let Some(obj) = json.as_object_mut()
    {
        obj.insert("runes".to_owned(), serde_json::json!(rw.runes));
        obj.insert("bases".to_owned(), serde_json::json!(rw.bases));
    }
    json
}

/// The `/api/builds` JSON ([`api_builds`]), pure over the parsed config: every build
/// definition with its slug, display name, damages, notes and per-slot piece counts, sorted by
/// slug.
#[must_use]
fn builds_json(config: &crate::config::Config) -> serde_json::Value {
    let counts = |slots: &BTreeMap<String, Vec<String>>| {
        slots
            .iter()
            .map(|(slot, pieces)| (slot.clone(), serde_json::json!(pieces.len())))
            .collect::<serde_json::Map<String, serde_json::Value>>()
    };
    let builds: Vec<_> = config
        .build_defs
        .iter()
        .map(|(slug, def)| {
            serde_json::json!({
                "slug": slug,
                "name": def.name,
                "damages": def.damages.iter().map(|d| d.name()).collect::<Vec<_>>(),
                "notes": def.notes,
                "slots": counts(&def.slots),
                "merc_slots": counts(&def.merc_slots),
            })
        })
        .collect();
    serde_json::json!({ "builds": builds })
}

/// One tick's dashboard data, gathered by `live` from the snapshot and the dashboard's text
/// blocks ([`crate::render::TextBlocks`]) and serialized by [`live_json`].
#[derive(Debug)]
pub struct LiveInfo<'a> {
    pub player_name: &'a str,
    /// Character class txt row (see [`CLASS_NAMES`]).
    pub class: u32,
    /// `None` when the level stat was unreadable.
    pub char_level: Option<u8>,
    /// `None` when unreadable (in the menu no snapshot exists at all).
    pub difficulty: Option<u8>,
    /// The current area's name (or a `Level <id>` fallback).
    pub area: &'a str,
    pub blocks: &'a crate::render::TextBlocks,
    /// The character's resolved `BiS` slots (`None` without a build): the gear grid lists
    /// each slot's pieces.
    pub bis: Option<&'a crate::config::Bis>,
    /// The selected build definition (`None` without one): the per-piece details, the
    /// targets and the extras (charms, jewels, runes) the gear panel shows.
    pub build: Option<&'a crate::config::Build>,
    /// The equipped `(body slot, quality)` pairs, shown under the `BiS` chase list.
    pub equipped: &'a [(u8, u32)],
    /// The equipped items enriched for the gear grid's column headers
    /// ([`crate::game::Equipped`]): per body slot code, name and runeword flag.
    pub equipped_items: &'a [crate::game::Equipped],
    /// Display names of the identified unique/set items the character owns (equipped,
    /// carried, personally stashed): the grid pieces' ✓ ticks.
    pub owned: &'a [String],
    /// The (session-cached) shared-stash tabs, shown in their own panel.
    pub stash: &'a [crate::stash::StashTab],
    /// Names of the identified unique/set items in the shared stash, emitted as
    /// `stash_owned` for reference; the per-`BiS`-line `tick` ([`info::Tick`]) is what the
    /// page marks ✓(stash) from.
    pub stash_owned: &'a [String],
    /// Unix seconds of the tick that produced this data.
    pub updated_at: u64,
}

/// Serializes one tick's dashboard data. Pure: the same input gives the same JSON, so the
/// tests need no game. Shape:
/// `{ "live": true, "updated_at": <unix secs>, "player": {"name", "class", "level",
/// "difficulty"}, "area": …, "guide": […], "levels": […], "keep": […], "sell": […],
/// "give": […], "bis": […], "equipped": […], "stash_pages": […], "stash_materials": {…},
/// "stash_owned": […] }`, where
/// each line is `{ "text": …, "colour": "#rrggbb" }` (the overlay line's colour as hex; the
/// guide's per-span colours are flattened to the base colour) — except the `bis` lines, which
/// carry a structured `"tick": "owned" | "stash" | null` (computed in Rust from the owned and
/// stash name lists, [`info::Tick`]): the page marks ✓(stash) from it instead of scraping the
/// line text, and, for a runeword piece, `"runes"` and `"bases"` for the hint line under the
/// piece. The `levels` lines are the whole level-suggestion band, one area per line, each as
/// `{ "text", "colour", "mlvl", "bucket" }` ([`crate::info::LevelEntry`]: the own-level-first
/// order in `text`, and the `below`/`equal`/`above` bucket the dashboard lays its "Where to
/// go" panel out in three columns from). The give-away lines have no panel of their own, so
/// they ride with `sell` in
/// the dashboard's Sell panel. The `gear` section is the 2×5 `BiS` grid: one column per
/// gear slot ([`gear_json`]), each with the equipped piece in that slot and the build's
/// pieces ordered best→worst, their `state` computed here (see [`grid_pieces`]). The shared
/// stash comes as `stash_pages` (the normal tabs in the game's UI order, [`stash_pages`]) and
/// `stash_materials` (the gems/runes/materials tab's stacks, [`stash_materials`]).
#[must_use]
pub fn live_json(info: &LiveInfo) -> String {
    let class = CLASS_NAMES
        .get(usize::try_from(info.class).unwrap_or(usize::MAX))
        .map_or_else(|| format!("class {}", info.class), |name| (*name).to_owned());
    let difficulty = info.difficulty.and_then(crate::info::difficulty_name);
    serde_json::json!({
        "live": true,
        "updated_at": info.updated_at,
        "player": {
            "name": info.player_name,
            "class": class,
            "level": info.char_level,
            "difficulty": difficulty,
        },
        "area": info.area,
        "guide": info.blocks.guide.iter().map(band_line).collect::<Vec<_>>(),
        "levels": level_lines_full(&info.blocks.level_full, crate::info::LEVEL_TEXT),
        "keep": text_lines(&info.blocks.keep, crate::info::KEEP_TEXT),
        "sell": text_lines(&info.blocks.sell, crate::info::SELL_TEXT),
        "give": text_lines(&info.blocks.give, crate::info::GIVE_TEXT),
        "bis": info.blocks.bis.iter().map(bis_line).collect::<Vec<_>>(),
        "gear": gear_json(info),
        "equipped": text_lines(&equipped_lines(info.equipped), [255, 255, 255, 255]),
        "stash_pages": stash_pages(info.stash),
        "stash_materials": stash_materials(info.stash),
        "stash_owned": info.stash_owned,
    })
    .to_string()
}

/// The normal shared-stash pages as `[{ "page": n, "items": [{ "text", "colour" }] }]`, UI
/// order, 1-based page numbers ([`crate::stash::TabKind::Page`]); the materials tab is left
/// to [`stash_materials`].
fn stash_pages(tabs: &[crate::stash::StashTab]) -> Vec<serde_json::Value> {
    tabs.iter()
        .filter_map(|tab| match tab.kind {
            crate::stash::TabKind::Page(page) => Some(serde_json::json!({
                "page": page,
                "items": tab.items.iter().map(stash_item_line).collect::<Vec<_>>(),
            })),
            crate::stash::TabKind::Materials => None,
        })
        .collect()
}

/// One stash item with its quality tag as `{ "text", "colour" }`. Uniques are gold and sets
/// green (the overlay's drop colours); magic and rare get their classic colours, everything
/// else stays plain.
fn stash_item_line(item: &crate::stash::StashItem) -> serde_json::Value {
    use crate::offsets::item_data;
    let (tag, colour) = match u32::from(item.quality) {
        item_data::QUALITY_UNIQUE => ("unique", "#ffd278"),
        item_data::QUALITY_SET => ("set", "#5ae65a"),
        item_data::QUALITY_RARE => ("rare", "#ffff5f"),
        item_data::QUALITY_MAGIC => ("magic", "#8f88ff"),
        _ => ("", "#dddddd"),
    };
    let mut tag = tag.to_owned();
    // Base-name items already carry an "eth " prefix from the keep-list naming; only the
    // unique/set names need the tag.
    if item.ethereal
        && matches!(
            u32::from(item.quality),
            item_data::QUALITY_UNIQUE | item_data::QUALITY_SET
        )
    {
        tag.push_str(" eth");
    }
    if item.runeword {
        tag.push_str(" runeword");
    }
    let text = if tag.is_empty() {
        item.name.clone()
    } else {
        format!("{} ({tag})", item.name)
    };
    serde_json::json!({ "text": text, "colour": colour })
}

/// The gems/runes/materials tab's stacks as `{ "gems": [...], "runes": [...], "other": [...] }`,
/// each entry `{ "name", "qty", "code" }`: gems by colour (amethyst … skull) then grade
/// (chipped … perfect), runes by rune number, the rest by name. Emptied stacks (quantity 0)
/// are left out; a stack whose size was unreadable keeps `"qty": null`.
fn stash_materials(tabs: &[crate::stash::StashTab]) -> serde_json::Value {
    use crate::items::{self, DropKind};
    // A fn, not a closure: closures can't be generic over the three key types.
    fn values<K>(v: Vec<(K, serde_json::Value)>) -> Vec<serde_json::Value> {
        v.into_iter().map(|(_, e)| e).collect()
    }
    let mut gems = Vec::new();
    let mut runes = Vec::new();
    let mut other = Vec::new();
    let stacks = tabs
        .iter()
        .filter(|t| t.kind == crate::stash::TabKind::Materials)
        .flat_map(|t| &t.items)
        .filter(|i| i.quantity != Some(0));
    for item in stacks {
        let bytes = item.code.as_deref().and_then(items::code_bytes);
        let entry = serde_json::json!({ "name": item.name, "qty": item.quantity, "code": item.code });
        if let Some(gem) = bytes.and_then(items::gem) {
            gems.push((gem, entry));
        } else if let Some(DropKind::Rune(n)) = bytes.and_then(items::code_kind) {
            runes.push((n, entry));
        } else {
            other.push((item.name.clone(), entry));
        }
    }
    gems.sort_by_key(|(k, _)| *k);
    runes.sort_by_key(|(k, _)| *k);
    other.sort_by(|a, b| a.0.cmp(&b.0));
    serde_json::json!({ "gems": values(gems), "runes": values(runes), "other": values(other) })
}

/// One `BiS` line as `{ "text", "colour", "tick" }`: the tick is the structural ✓ source
/// ([`info::Tick`]) — `null` when the piece is owned by neither the character nor the stash.
/// A piece that is a known runeword also carries `"runes"` and `"bases"` (the dashboard renders
/// them as a dim hint line under the piece).
fn bis_line(line: &crate::info::BisLine) -> serde_json::Value {
    let mut json = serde_json::json!({
        "text": line.text,
        "colour": hex(crate::info::BIS_TEXT),
        "tick": line.tick.map(crate::info::Tick::as_str),
    });
    if let Some(rw) = &line.runeword
        && let Some(obj) = json.as_object_mut()
    {
        obj.insert("runes".to_owned(), serde_json::json!(rw.runes));
        obj.insert("bases".to_owned(), serde_json::json!(rw.bases));
    }
    json
}

/// The 2×5 gear grid (`live_json`'s `gear` section): `{ "columns": [...] }` — the dashboard
/// lays them out two rows of five, in the given order (Helm, Body, Hands, Belt, Foot /
/// Amulet, Ring, Weapons, Off-Hand, Swap; the user's row split). Each column is
/// `{ "title", "equipped": [...], "note": null, "pieces": [...] }` where `equipped` holds
/// the equipped piece(s) in that slot — two for the merged Ring column — as
/// `{ "name", "level" }` ([`equipped_name`], [`crate::game::Equipped::required_level`]),
/// `note` is the off-hand's two-handed remark, and `pieces` are the build's pieces for that
/// slot ([`grid_pieces`]).
#[must_use]
fn gear_json(info: &LiveInfo) -> serde_json::Value {
    // (column title, build slot name, `ItemData::BODY` slots the header reads). The Swap
    // column shares body 4 with Weapons: only the active weapon set is equipped, and its
    // items carry body 4 — the inactive set's pieces are unread.
    const COLUMNS: [(&str, &str, &[u8]); 9] = [
        ("Helm", "helm", &[1]),
        ("Body", "body", &[3]),
        ("Hands", "gloves", &[10]),
        ("Belt", "belt", &[8]),
        ("Foot", "boots", &[9]),
        ("Amulet", "amulet", &[2]),
        ("Weapons", "weapon", &[4]),
        ("Off-Hand", "offhand", &[5]),
        ("Swap", "swap", &[4]),
    ];
    let mut columns: Vec<_> = COLUMNS
        .iter()
        .map(|&(title, slot, bodies)| {
            grid_column(title, info.bis.and_then(|b| b.slots.get(slot)), bodies, info)
        })
        .collect();
    // The Ring column merges ring1 and ring2 (the build stores them as two slots): pieces
    // from both, deduplicated, in stored order (best→worst).
    let mut ring_pieces: Vec<String> = Vec::new();
    for slot in ["ring1", "ring2"] {
        for piece in info
            .bis
            .and_then(|b| b.slots.get(slot))
            .map_or(&[][..], Vec::as_slice)
        {
            if !ring_pieces.contains(piece) {
                ring_pieces.push(piece.clone());
            }
        }
    }
    let ring = grid_column("Ring", Some(&ring_pieces), &[6, 7], info);
    columns.insert(6, ring);
    // The mercenary's slots (its equipped items are not attributed yet: no header) and the
    // build's extras (charms, jewels, runes), with the same per-piece state and details.
    let merc: Vec<_> = [
        ("Merc weapon", "weapon"),
        ("Merc body", "body"),
        ("Merc helm", "helm"),
    ]
    .iter()
    .filter_map(|&(title, slot)| {
        let pieces = info.bis.and_then(|b| b.merc_slots.get(slot))?;
        Some(serde_json::json!({
            "title": title,
            "pieces": grid_pieces(pieces, None, info),
        }))
    })
    .collect();
    let extras: Vec<_> = info
        .build
        .map(|b| &b.extras)
        .into_iter()
        .flatten()
        .map(|(group, pieces)| {
            serde_json::json!({
                "title": group,
                "pieces": grid_pieces(pieces, None, info),
            })
        })
        .collect();
    serde_json::json!({
        "columns": columns,
        "build": info.bis.map(|b| b.build.as_str()),
        "targets": info.build.map_or(&[][..], |b| b.targets.as_slice()),
        "merc": merc,
        "extras": extras,
    })
}

/// One gear-grid column: the equipped header piece(s) for `bodies`, the build's `pieces`
/// with their server-side state, and the off-hand's two-handed note.
fn grid_column(
    title: &str,
    pieces: Option<&Vec<String>>,
    bodies: &[u8],
    info: &LiveInfo,
) -> serde_json::Value {
    let equipped = |bodies: &[u8]| {
        bodies
            .iter()
            .filter_map(|&body| info.equipped_items.iter().find(|e| e.body == body))
            .collect::<Vec<&crate::game::Equipped>>()
    };
    let header = equipped(bodies);
    // The column's comparison base: the strongest equipped piece's requirement. Two rings
    // with different levels compare against the higher one (a piece better than one ring
    // but worse than the other is still worth listing as better).
    let equipped_level = header.iter().filter_map(|e| e.required_level()).max();
    // The off-hand is unusable while a two-handed weapon is equipped (weapons.txt
    // `2handed`, looked up by the weapon's item code).
    let note = (title == "Off-Hand")
        .then(|| {
            equipped(&[4])
                .into_iter()
                .next()
                .and_then(|w| w.code.as_deref())
                .and_then(crate::weapons_data::two_handed)
                .filter(|&two| two)
                .map(|_| "(two-handed weapon equipped)")
        })
        .flatten();
    serde_json::json!({
        "title": title,
        "equipped": header
            .iter()
            .map(|e| serde_json::json!({ "name": equipped_name(e), "level": e.required_level() }))
            .collect::<Vec<_>>(),
        "note": note,
        "pieces": grid_pieces(pieces.map_or(&[][..], Vec::as_slice), equipped_level, info),
    })
}

/// The build's pieces of one grid column with their state, best→worst (they are stored
/// ordered). `state`:
/// - `"worse"`: the piece's required level is at or below the equipped piece's (grey +
///   strike-through) — not worth chasing even when above the level gate;
/// - `"too_high"`: it would be better but is above the clvl + 5 gate (grey, no strike);
/// - `"better"`: its level is above the equipped piece's, or the slot has no comparable
///   equipped piece (nothing equipped, or an unnameable one — a runeword or an
///   unidentified unique/set), or the piece's level is unknown; never struck through.
///
/// A column whose equipped piece is unnameable therefore shows no strike-throughs.
fn grid_pieces(pieces: &[String], equipped_level: Option<u32>, info: &LiveInfo) -> Vec<serde_json::Value> {
    let lvl = u32::from(info.char_level.unwrap_or(0));
    pieces
        .iter()
        .map(|piece| {
            let req = crate::info::piece_level(piece);
            let state = match req {
                Some(req) if equipped_level.is_some_and(|eq| req <= eq) => "worse",
                Some(req) if req > lvl + 5 => "too_high",
                _ => "better",
            };
            let mut json = serde_json::json!({
                "piece": piece,
                "name": crate::info::piece_name(piece),
                "level": req,
                "state": state,
                "tick": crate::info::piece_tick(piece, info.owned, info.stash_owned)
                    .map(crate::info::Tick::as_str),
            });
            if let Some(rw) = crate::runewords_data::lookup(crate::info::piece_name(piece))
                && let Some(obj) = json.as_object_mut()
            {
                obj.insert("runes".to_owned(), serde_json::json!(rw.runes));
                obj.insert("bases".to_owned(), serde_json::json!(rw.bases));
            }
            add_piece_details(&mut json, crate::info::piece_name(piece), info.build);
            json
        })
        .collect()
}

/// Adds the build file's details of piece `name` (`[pieces."<name>"]`) to its JSON: `kind`,
/// `setup`, `base`, `stats`, `why`, `notes` — each only when set, so the page can test for
/// presence.
fn add_piece_details(json: &mut serde_json::Value, name: &str, build: Option<&crate::config::Build>) {
    let Some(info) = build.and_then(|b| b.pieces.get(name)) else {
        return;
    };
    let Some(obj) = json.as_object_mut() else { return };
    for (key, value) in [
        ("kind", &info.kind),
        ("setup", &info.setup),
        ("base", &info.base),
        ("why", &info.why),
        ("notes", &info.notes),
    ] {
        if !value.is_empty() {
            obj.insert(key.to_owned(), serde_json::json!(value));
        }
    }
    if !info.stats.is_empty() {
        obj.insert("stats".to_owned(), serde_json::json!(info.stats));
    }
}

/// The name shown for an equipped piece in a grid column header: the identified unique/set
/// name, else (normal/superior quality) the base item's name from the armor/weapons
/// tables, else — for a runeword flag (the runes are unread, so no word can be named) and
/// for magic/rare (the real name comes from the affixes) — the placeholders below.
fn equipped_name(equipped: &crate::game::Equipped) -> String {
    if let Some(name) = &equipped.name {
        return name.clone();
    }
    if equipped.runeword {
        return "runeword (unnamed)".to_owned();
    }
    if matches!(
        equipped.quality,
        crate::offsets::item_data::QUALITY_NORMAL | crate::offsets::item_data::QUALITY_SUPERIOR
    ) && let Some(code) = equipped.code.as_deref()
    {
        let base = crate::item_levels_data::armor_name(code).or_else(|| crate::weapons_data::name(code));
        if let Some(name) = base {
            return name.to_owned();
        }
    }
    "(unknown)".to_owned()
}

/// `levels`/`keep`/… lines as `{ "text", "colour" }` objects in the block's colour.
fn text_lines(lines: &[String], colour: [u8; 4]) -> Vec<serde_json::Value> {
    let hex = hex(colour);
    lines
        .iter()
        .map(|text| serde_json::json!({ "text": text, "colour": hex }))
        .collect()
}

/// The whole level-suggestion band as `{ "text", "colour", "mlvl", "bucket" }` objects
/// ([`crate::info::LevelEntry`]): the own-level-first reading order in `text`, and the
/// structured `mlvl` + `below`/`equal`/`above` bucket the dashboard re-groups its "Where to
/// go" panel's three columns from.
fn level_lines_full(entries: &[crate::info::LevelEntry], colour: [u8; 4]) -> Vec<serde_json::Value> {
    let hex = hex(colour);
    entries
        .iter()
        .map(|e| serde_json::json!({ "text": e.text, "colour": hex, "mlvl": e.mlvl, "bucket": e.bucket }))
        .collect()
}

/// A guide band as a `{ "text", "colour" }` object: the band's base colour as hex, spans
/// flattened away (the dashboard keeps the values readable, not the per-span red).
fn band_line(band: &crate::info::Band) -> serde_json::Value {
    serde_json::json!({ "text": band.text, "colour": hex(band.colour) })
}

/// An overlay `[u8; 4]` RGBA colour as `#rrggbb` (the alpha is the same 255 everywhere).
fn hex([r, g, b, _]: [u8; 4]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// The equipped items as dashboard lines: `helm: unique`, `ring2: rare`, … (the `BiS` "already
/// better" pairs; see [`crate::game::Snapshot::equipped`]). Unknown slots and qualities are
/// shown as such rather than dropped.
fn equipped_lines(equipped: &[(u8, u32)]) -> Vec<String> {
    equipped
        .iter()
        .map(|&(body, quality)| {
            let slot =
                crate::info::body_slot_name(body).map_or_else(|| format!("slot {body}"), str::to_owned);
            format!("{slot}: {}", quality_name(quality))
        })
        .collect()
}

/// The classic quality name of an `ItemData` quality (see [`crate::offsets::item_data`]).
fn quality_name(quality: u32) -> String {
    use crate::offsets::item_data;
    match quality {
        item_data::QUALITY_SET => "set".to_owned(),
        item_data::QUALITY_UNIQUE => "unique".to_owned(),
        item_data::QUALITY_RARE => "rare".to_owned(),
        item_data::QUALITY_MAGIC => "magic".to_owned(),
        item_data::QUALITY_SUPERIOR => "superior".to_owned(),
        item_data::QUALITY_NORMAL => "normal".to_owned(),
        _ => format!("quality {quality}"),
    }
}

/// Reads `config.toml`, applies `edit`, and writes it back atomically. The whole
/// read-edit-write runs under [`CONFIG_WRITE`]: concurrent saves must not both start from the
/// same old text. Answers the browser with a JSON receipt or a plain-text error.
fn save_config(edit: impl Fn(&str) -> Option<String>) -> String {
    let Some(path) = crate::config::default_path() else {
        return respond("500 Internal Server Error", "text/plain", "no config path\n");
    };
    // A poisoned lock still holds a working mutex value; use it rather than lose the save.
    let _guard = CONFIG_WRITE.lock().unwrap_or_else(PoisonError::into_inner);
    match std::fs::read_to_string(&path) {
        Ok(text) => edit(&text).map_or_else(
            || {
                respond(
                    "500 Internal Server Error",
                    "text/plain",
                    "config is not valid TOML\n",
                )
            },
            |text| match write_atomic(&path, &text) {
                Ok(()) => respond("200 OK", "application/json", "{\"saved\": true}"),
                Err(e) => respond("500 Internal Server Error", "text/plain", &format!("{e}\n")),
            },
        ),
        Err(e) => respond("500 Internal Server Error", "text/plain", &format!("{e}\n")),
    }
}

/// Writes `text` over `path` atomically: to `<path>.tmp` first, then a rename. A rename over
/// an existing file is atomic on POSIX, so the hot-reload reader never sees a truncated or
/// empty `config.toml` and stick on the defaults.
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text)?;
    // Don't leave the temp file behind when the rename fails.
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// `POST /api/bis` with `{"name": …, "build": "<slug>"}` — `"build": null`, `""` or omitted
/// clears the selection: writes ONLY the `[bis.<name>]` `build` value into `config.toml` (the
/// gear itself lives in the agent-authored `builds/*.toml`; the webapp doesn't edit it).
fn api_bis(body: &str) -> String {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return respond("400 Bad Request", "text/plain", "bad json\n");
    };
    let Some(name) = json.get("name").and_then(|v| v.as_str()) else {
        return respond("400 Bad Request", "text/plain", "name required\n");
    };
    // `build` must be a string (the selection) or null/absent (clear it); any other JSON
    // type is a client error, not a silent clear.
    let build = match json.get("build") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.trim()).filter(|slug| !slug.is_empty()),
        Some(_) => {
            return respond(
                "400 Bad Request",
                "text/plain",
                "build must be a string or null\n",
            );
        }
    };
    save_config(|text| set_bis_selection(text, name, build))
}

/// The config page's HTML (a single string: no assets, no framework). The user picks a build
/// per character (selection-only config); the gear itself is agent-authored and shown
/// read-only, annotated with the live ✓ and strike-through.
const fn config_page() -> &'static str {
    r#"<!doctype html>
<html><head><meta charset="utf-8"><title>d2r-map-overlay config</title>
<style>
body { font-family: sans-serif; background: #1a1a1a; color: #ddd; margin: 2em; }
input, select { background: #2a2a2a; color: #eee; border: 1px solid #555; padding: 4px; }
button { padding: 4px 10px; }
.char { border: 1px solid #444; padding: 1em; margin-bottom: 1em; }
.piece { margin: 2px 0; }
.hint { color: #9c9; font-size: 0.85em; }
.rwhint { color: #888; font-size: 0.8em; margin-left: 1.5em; }
.owned { color: #6f6; }
.stash { color: #6f6; }
.better { text-decoration: line-through; color: #777; }
</style></head><body>
<h1>d2r-map-overlay config &nbsp; <a style="font-size:0.6em" href="/">live dashboard →</a></h1>
<p class="hint">Pick a build per character. The gear lists live in
<code>~/.config/d2r-map-overlay/builds/*.toml</code> (agent-authored) and are shown read-only:
✓ = owned (identified set/unique pieces), ✓(stash) = in the shared stash, struck through =
the equipped piece is already better.</p>
<div id="chars"></div>
<h2>Add character</h2>
<input id="newname" placeholder="character name"> <select id="newbuild"></select>
<button onclick="addChar()">Add</button>
<script>
const SLOTS = ["amulet","ring1","ring2","body","helm","gloves","boots","belt","weapon","offhand","swap"];
// Escapes a config-supplied string for interpolation into HTML text and
// double-quoted attribute values (character names and build names are free text).
function esc(s) {
  return String(s).replace(/[&<>"']/g, ch => ({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[ch]));
}
// One piece line: name (level), tick, strike-through, runeword hint.
function pieceHtml(p) {
  const level = p.level != null ? ` (${p.level})` : "";
  const tick = p.tick === "owned" ? ` <span class="owned">\u2713</span>`
    : p.tick === "stash" ? ` <span class="owned">\u2713</span><span class="hint">(stash)</span>` : "";
  const strike = p.better ? " better" : "";
  const hint = p.runes
    ? `<div class="rwhint">${esc(p.runes.join(" + "))} · ${esc((p.bases || []).join(", "))}</div>` : "";
  return `<div class="piece${strike}">${esc(p.name)}${esc(level)}${tick}</div>` + hint;
}
// One slot table (slots or merc_slots) as an ordered read-only list.
function slotsHtml(slots) {
  return SLOTS.filter(s => slots && slots[s] && slots[s].length)
    .map(s => `<div class="slot"><b>${s}</b>:</div>` + slots[s].map(pieceHtml).join(""))
    .join("") || '<div class="hint">none</div>';
}
async function load() {
  const [cfg, buildsResp] = await Promise.all([fetch("/api/config"), fetch("/api/builds")].map(r => r.then(x => x.json())));
  const builds = buildsResp.builds || [];
  const options = () => ['<option value="">none</option>']
    .concat(builds.map(b => `<option value="${esc(b.slug)}">${esc(b.name)}</option>`))
    .join("");
  const root = document.getElementById("chars");
  root.innerHTML = "";
  document.getElementById("newbuild").innerHTML = options();
  for (const c of cfg.characters || []) {
    const div = document.createElement("div");
    div.className = "char";
    // The character name goes in a data attribute, not in classes/ids: names with spaces would
    // otherwise break the selectors.
    div.dataset.name = c.name;
    const sel = `<select class="build-select">${options()}</select>`;
    const buildName = c.build_name ? `<div class="hint">build: ${esc(c.build_name)}</div>` : "";
    const damages = (c.damages || []).length ? `<div class="hint">damages: ${esc(c.damages.join(", "))}</div>` : "";
    div.innerHTML = `<h2>${esc(c.name)}</h2>`
      + `build: ${sel} <button class="save">Save</button>${buildName}${damages}`
      + `<h3>BiS</h3>` + slotsHtml(c.slots)
      + `<h3>Mercenary</h3>` + slotsHtml(c.merc_slots);
    const select = div.querySelector(".build-select");
    select.value = c.build || "";
    div.querySelector(".save").addEventListener("click", () => saveBis(c.name, select.value));
    root.appendChild(div);
  }
}
// Posts ONLY the `[bis.<name>]` build selection; an empty value clears it.
async function saveBis(name, build) {
  await fetch("/api/bis", {method: "POST", body: JSON.stringify({name, build: build || null})});
  load();
}
async function addChar() {
  const name = document.getElementById("newname").value.trim();
  const build = document.getElementById("newbuild").value;
  if (name && build) { await saveBis(name, build); }
}
load();
</script></body></html>
"#
}

/// The live dashboard page (a single string: no assets, no framework), dark-themed. Layout:
/// a sticky character bar with the breakpoint chips, the gear grid (equipped piece on top of
/// each slot card, the build's pieces below with their stats), then "Where to go" beside the
/// shared stash. The script follows in [`DASHBOARD_SCRIPT`].
const DASHBOARD_HEAD: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>D2R Dashboard</title>
<style>
:root {
  --bg: #0f0e0c; --panel: #1a1815; --panel2: #221f1b; --line: #3a342c; --line2: #2b2721;
  --text: #e4ddd0; --dim: #9a8f7f; --faint: #6b6255;
  --gold: #c8a85a; --unique: #c7b377; --set: #3fcf5a; --rune: #e0a040; --magic: #7a8cff;
  --good: #6fdc6f; --bad: #ff6b5b; --accent: #d49a3a;
  --radius: 8px;
}
* { box-sizing: border-box; }
html { background: var(--bg); }
body { margin: 0; background: var(--bg); color: var(--text);
  font: 14px/1.4 system-ui, -apple-system, "Segoe UI", Roboto, sans-serif; }
a { color: var(--gold); text-decoration: none; } a:hover { text-decoration: underline; }
.wrap { max-width: 1680px; margin: 0 auto; padding: 16px; }

/* Character bar */
.bar { position: sticky; top: 0; z-index: 5; background: rgba(15,14,12,.94);
  backdrop-filter: blur(6px); border-bottom: 1px solid var(--line); }
.bar .wrap { display: flex; flex-wrap: wrap; align-items: center; gap: 10px 18px; padding: 10px 16px; }
.who { font-size: 18px; font-weight: 650; color: var(--gold); letter-spacing: .02em; }
.who small { color: var(--dim); font-weight: 400; font-size: 13px; margin-left: 8px; }
.chips { display: flex; flex-wrap: wrap; gap: 6px; flex: 1; }
.chip { background: var(--panel2); border: 1px solid var(--line); border-radius: 999px;
  padding: 2px 10px; font-size: 12.5px; font-variant-numeric: tabular-nums; white-space: nowrap; }
.chip b { color: var(--dim); font-weight: 500; margin-right: 4px; text-transform: uppercase; font-size: 11px; }
.meta { color: var(--faint); font-size: 12px; }
.waiting { color: var(--dim); font-style: italic; }

/* Panels */
.grid { display: grid; gap: 16px; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); margin-top: 16px; }
.panel { background: var(--panel); border: 1px solid var(--line2); border-radius: var(--radius); padding: 14px 16px; min-width: 0; }
.panel.full { grid-column: 1 / -1; }
.panel h2 { margin: 0 0 10px; font-size: 13px; font-weight: 650; letter-spacing: .08em;
  text-transform: uppercase; color: var(--gold); display: flex; align-items: baseline; gap: 10px; }
.panel h2 .sub { color: var(--faint); font-weight: 400; letter-spacing: 0; text-transform: none; font-size: 12px; }
.targets { margin: 0 0 12px; padding: 0; list-style: none; display: flex; flex-wrap: wrap; gap: 6px; }
.targets li { font-size: 12px; background: rgba(200,168,90,.08); border: 1px solid rgba(200,168,90,.3);
  color: var(--text); border-radius: 6px; padding: 3px 8px; }
.row-h { margin: 16px 0 8px; font-size: 11px; text-transform: uppercase; letter-spacing: .08em; color: var(--dim); font-weight: 600; }
#mercbox .gear + .gear { margin-top: 10px; }
details.alts { margin-top: 2px; }
details.alts > summary { cursor: pointer; font-size: 11.5px; color: var(--faint); padding: 2px 0; }
details.alts[open] > summary { margin-bottom: 5px; }
details.alts .pc { margin-bottom: 5px; }
.pc .note { font-size: 11px; color: var(--faint); margin-top: 2px; }
.pc.rare .nm { color: #f0e060; } .pc.rare { border-left-color: #b8a640; }
.pc.magic .nm { color: var(--magic); } .pc.magic { border-left-color: var(--magic); }
.pc.crafted .nm { color: #e08a3c; } .pc.crafted { border-left-color: #e08a3c; }
.pc.base .nm { color: var(--text); }
details.help { color: var(--dim); font-size: 12px; margin-bottom: 10px; }
details.help summary { cursor: pointer; color: var(--faint); }
.empty { color: var(--faint); font-style: italic; font-size: 12.5px; }

/* Gear */
.gear { display: grid; gap: 10px; grid-template-columns: repeat(5, minmax(0, 1fr)); }
.slot { background: var(--panel2); border: 1px solid var(--line2); border-radius: var(--radius);
  display: flex; flex-direction: column; min-width: 0; }
.slot-h { display: flex; justify-content: space-between; padding: 7px 10px 0; font-size: 11px;
  text-transform: uppercase; letter-spacing: .08em; color: var(--dim); }
.eq { margin: 6px 8px 8px; padding: 7px 9px; border-radius: 6px; background: #12110f;
  border: 1px dashed var(--line); font-size: 13px; }
.eq .lbl { display: block; font-size: 10px; color: var(--faint); text-transform: uppercase; letter-spacing: .08em; }
.eq .nm { color: var(--text); }
.eq .nm.unknown { color: var(--dim); }
.eq .warn { color: var(--bad); font-size: 11.5px; display: block; }
.pieces { padding: 0 8px 8px; display: flex; flex-direction: column; gap: 5px; }
.pc { border-left: 3px solid var(--line); padding: 4px 8px; border-radius: 0 5px 5px 0; background: rgba(255,255,255,.015); }
.pc .top { display: flex; gap: 6px; align-items: baseline; }
.pc .nm { font-weight: 600; flex: 1; min-width: 0; overflow-wrap: anywhere; }
.pc .lv { font-size: 11px; color: var(--dim); font-variant-numeric: tabular-nums; white-space: nowrap; }
.pc .tick { color: var(--good); font-weight: 700; font-size: 12px; }
.pc .sub { font-size: 11.5px; color: var(--dim); }
.pc ul.st { margin: 3px 0 0; padding: 0; list-style: none; font-size: 11.5px; color: #b9c4ff; }
.pc ul.st li::before { content: "· "; color: var(--faint); }
.pc .why { font-size: 11.5px; color: var(--dim); font-style: italic; margin-top: 2px; }
.pc.unique { border-left-color: var(--unique); } .pc.unique .nm { color: var(--unique); }
.pc.set { border-left-color: var(--set); } .pc.set .nm { color: var(--set); }
.pc.runeword { border-left-color: var(--rune); } .pc.runeword .nm { color: var(--rune); }
.pc.better { box-shadow: inset 0 0 0 1px rgba(111,220,111,.25); }
.pc.better .lv::before { content: "▲ "; color: var(--good); }
.pc.too_high { opacity: .5; }
.pc.worse { opacity: .38; } .pc.worse .nm { text-decoration: line-through; }
.badge { font-size: 10px; padding: 0 5px; border-radius: 4px; border: 1px solid var(--line); color: var(--dim); }

/* Where to go */
.lv3 { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 10px; }
.lvc { background: var(--panel2); border: 1px solid var(--line2); border-radius: var(--radius); padding: 8px 10px; min-width: 0; }
.lvc.equal { border-color: var(--accent); }
.lvc h3 { margin: 0 0 6px; font-size: 11px; text-transform: uppercase; letter-spacing: .08em; color: var(--dim); }
.lvc.equal h3 { color: var(--accent); }
.area { display: flex; justify-content: space-between; gap: 8px; font-size: 13px; padding: 1px 0; }
.area .n { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.area .m { font-variant-numeric: tabular-nums; color: var(--dim); font-size: 12px; }

/* Stash */
.pages { display: grid; grid-template-columns: repeat(5, minmax(0, 1fr)); gap: 8px; }
.page { background: var(--panel2); border: 1px solid var(--line2); border-radius: 6px; padding: 6px 8px; min-height: 54px; min-width: 0; }
.page h3 { margin: 0 0 4px; font-size: 11px; color: var(--dim); text-transform: uppercase; letter-spacing: .06em; display: flex; justify-content: space-between; }
.page .it { font-size: 12.5px; overflow-wrap: anywhere; }
.mats { margin-top: 12px; display: grid; gap: 10px; }
.mats h3 { margin: 0 0 6px; font-size: 11px; color: var(--dim); text-transform: uppercase; letter-spacing: .06em; }
.stacks { display: flex; flex-wrap: wrap; gap: 5px; }
.stk { display: inline-flex; gap: 5px; align-items: baseline; background: var(--panel2);
  border: 1px solid var(--line2); border-radius: 5px; padding: 2px 7px; font-size: 12.5px; }
.stk .q { color: var(--gold); font-variant-numeric: tabular-nums; font-weight: 650; }
.stk.rune .nm { color: var(--rune); }
.lines .line { white-space: pre-wrap; font-size: 13px; }

@media (max-width: 1200px) { .gear { grid-template-columns: repeat(3, minmax(0, 1fr)); } }
@media (max-width: 900px) {
  .grid { grid-template-columns: minmax(0, 1fr); }
  .gear, .pages { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  .lv3 { grid-template-columns: minmax(0, 1fr); }
}
@media (max-width: 520px) { .gear, .pages { grid-template-columns: minmax(0, 1fr); } .bar { position: static; } }
</style></head><body>
<div class="bar"><div class="wrap">
  <div class="who" id="who"><span class="waiting">waiting for game…</span></div>
  <div class="chips" id="guide"></div>
  <div class="meta"><span id="updated"></span> · <a href="/config">config</a></div>
</div></div>
<div class="wrap">
<section class="panel full"><h2>Gear <span class="sub" id="buildname"></span></h2>
<details class="help"><summary>How to read this</summary>
Each card is a slot. The dashed box shows what is equipped (name and required level), and below it are the build's pieces, best first.
▲ / green outline = an upgrade you can wear now, faded = above the level gate (character level + 5), struck through = already beaten by what you wear.
✓ = owned, ✓ stash = in the shared stash. A runeword the reader can't decode shows as “runeword (unnamed)”. The Swap card mirrors the active weapon set.
</details>
<ul class="targets" id="targets"></ul>
<div class="gear" id="gear"></div>
<div id="mercbox"><h3 class="row-h">Mercenary · charms · sockets</h3>
<div class="gear" id="merc"></div><div class="gear extras" id="extras"></div></div></section>
<div class="grid">
<section class="panel"><h2>Where to go <span class="sub">monster level around yours</span></h2><div id="levels"></div></section>
<section class="panel"><h2>Shared stash</h2><div id="stash"></div></section>
<section class="panel full" id="sellpanel"><h2>Sell / Keep</h2><div id="sell" class="lines"></div></section>
</div></div>
"#;

/// The dashboard page's script: first-paint fetch, then the SSE stream (`render` is the
/// onmessage handler), the level columns, the gear cards and the stash panel.
const DASHBOARD_SCRIPT: &str = r#"<script>
// HTML-escape: texts come from item and area names read out of the game. Numeric character
// references (&#38; …), so the replacement strings stay plain ASCII here.
function esc(s) {
  return String(s == null ? "" : s).replace(/[&<>"']/g, c => "&#" + c.charCodeAt(0) + ";");
}
const $ = id => document.getElementById(id);
// A server colour as a CSS value: only a #rrggbb hex passes, anything else inherits (so a future
// build-sourced colour can never break out of the style attribute).
const col = c => /^#[0-9a-fA-F]{6}$/.test(c || "") ? c : "inherit";
// The guide line ("fcr (15 / 0 / 9) · fhr … · res 21/0/44/7") as one chip per part.
function guide(entries) {
  $("guide").innerHTML = (entries || []).flatMap(e => e.text.split(" · ").map(part => {
    const m = part.match(/^(\S+)\s+(.*)$/);
    const body = m ? `<b>${esc(m[1])}</b>${esc(m[2])}` : esc(part);
    return `<span class="chip" style="color:${col(e.colour)}">${body}</span>`;
  })).join("");
}
// "Where to go": three cards (below / equal / above the character level). The server sends
// the entries structured ("mlvl" + "bucket"); the text's trailing "(N)" moves to the right.
function levels(entries) {
  const cards = ["below", "equal", "above"].map(bucket => {
    const rows = (entries || []).filter(e => e.bucket === bucket).map(e => {
      const name = e.text.replace(/\s*\(\d+\)\s*$/, "");
      return `<div class="area" style="color:${col(e.colour)}"><span class="n" title="${esc(name)}">${esc(name)}</span><span class="m">${e.mlvl != null ? e.mlvl : ""}</span></div>`;
    }).join("");
    return `<div class="lvc ${bucket}"><h3>${bucket}</h3>${rows || '<div class="empty">none</div>'}</div>`;
  }).join("");
  $("levels").innerHTML = `<div class="lv3">${cards}</div>`;
}
// The kind of a build piece, for its colour: explicit "kind" when the build carries one, a
// runeword when it lists runes, else unique (the build files are mostly uniques/sets).
function kindOf(p) {
  if (p.kind) return p.kind;
  if (p.runes) return "runeword";
  return "unique";
}
function piece(p) {
  const tick = p.tick === "owned" ? ' <span class="tick">✓</span>'
    : p.tick === "stash" ? ' <span class="tick">✓ stash</span>' : "";
  const lvl = p.level != null ? `<span class="lv">lvl ${p.level}</span>` : "";
  const sub = p.runes
    ? `<div class="sub">${esc(p.runes.join(" "))}${p.bases && p.bases.length ? " · " + esc(p.bases.join(", ")) : ""}</div>`
    : p.base ? `<div class="sub">${esc(p.base)}</div>` : "";
  const stats = p.stats && p.stats.length
    ? `<ul class="st">${p.stats.map(s => `<li>${esc(s)}</li>`).join("")}</ul>` : "";
  const why = (p.why ? `<div class="why">${esc(p.why)}</div>` : "")
    + (p.notes ? `<div class="note">${esc(p.notes)}</div>` : "");
  const setup = p.setup && p.setup !== "alternative" ? ` <span class="badge">${esc(p.setup)}</span>` : "";
  return `<div class="pc ${esc(kindOf(p))} ${esc(p.state || "better")}"><div class="top"><span class="nm">${esc(p.name)}${tick}</span>${setup}${lvl}</div>${sub}${stats}${why}</div>`;
}
// A slot's pieces: the guide's own picks (starter / standard, and pieces without a setup)
// stay visible; the alternatives fold into a <details> so a 14-weapon list stays readable.
// Slot titles whose "alternatives" fold the viewer opened: every SSE event re-renders the
// cards, so the open state is kept here and written back as the `open` attribute.
const openAlts = new Set();
document.addEventListener("toggle", e => {
  const d = e.target;
  if (!(d instanceof HTMLDetailsElement) || !d.classList.contains("alts")) return;
  if (d.open) openAlts.add(d.dataset.slot); else openAlts.delete(d.dataset.slot);
}, true);
function pieceList(pieces, title) {
  const main = pieces.filter(p => p.setup !== "alternative");
  const alt = pieces.filter(p => p.setup === "alternative");
  const shown = main.map(piece).join("");
  const folded = alt.length
    ? `<details class="alts" data-slot="${esc(title)}"${openAlts.has(title) ? " open" : ""}><summary>${alt.length} alternative${alt.length > 1 ? "s" : ""}</summary>${alt.map(piece).join("")}</details>`
    : "";
  return (shown + folded) || '<div class="empty">no build piece for this slot</div>';
}
function slotCard(title, eqHtml, pieces) {
  const eq = eqHtml == null ? "" : `<div class="eq"><span class="lbl">equipped</span>${eqHtml}</div>`;
  return `<div class="slot"><div class="slot-h"><span>${esc(title)}</span></div>${eq}<div class="pieces">${pieceList(pieces || [], title)}</div></div>`;
}
function gear(g) {
  g = g || {};
  $("buildname").textContent = g.build || "";
  $("targets").innerHTML = (g.targets || []).map(t => `<li>${esc(t)}</li>`).join("");
  $("gear").innerHTML = (g.columns || []).map(col => {
    const eq = (col.equipped || []).map(e => {
      const unknown = !e.name || e.name === "(unknown)";
      const lvl = e.level ? ` <span class="lv">lvl ${e.level}</span>` : "";
      return `<span class="nm${unknown ? " unknown" : ""}">${esc(unknown ? "unidentified / magic" : e.name)}</span>${lvl}`;
    }).join("<br>") || '<span class="nm unknown">empty</span>';
    const note = col.note ? `<span class="warn">${esc(col.note)}</span>` : "";
    return slotCard(col.title, eq + note, col.pieces);
  }).join("");
  $("merc").innerHTML = (g.merc || []).map(c => slotCard(c.title, null, c.pieces)).join("");
  $("extras").innerHTML = (g.extras || []).map(c => slotCard(c.title, null, c.pieces)).join("");
  $("mercbox").hidden = !(g.merc || []).length && !(g.extras || []).length;
}
// The shared stash: five page cards in UI order, then the gem/rune/material stacks.
function stash(r) {
  const el = $("stash");
  const pages = (r.stash_pages || []).map(p => {
    const items = (p.items || []).map(i => `<div class="it" style="color:${col(i.colour)}">${esc(i.text)}</div>`).join("")
      || '<div class="empty">empty</div>';
    return `<div class="page"><h3><span>page ${esc(p.page)}</span><span>${(p.items || []).length || ""}</span></h3>${items}</div>`;
  }).join("");
  const m = r.stash_materials || {};
  const stacks = (list, cls) => (list || []).map(s =>
    `<span class="stk ${cls}" title="${esc(s.code || "")}"><span class="nm">${esc(s.name)}</span><span class="q">${esc(s.qty)}</span></span>`).join("");
  const group = (title, list, cls) => list && list.length
    ? `<div><h3>${title}</h3><div class="stacks">${stacks(list, cls)}</div></div>` : "";
  const mats = group("Runes", m.runes, "rune") + group("Gems", m.gems, "gem") + group("Materials", m.other, "mat");
  const loaded = (r.stash_pages || []).some(p => (p.items || []).length) || mats;
  el.innerHTML = (loaded ? "" : '<div class="empty">open the stash in game once to load it</div>')
    + `<div class="pages">${pages}</div><div class="mats">${mats}</div>`;
}
function sell(r) {
  const all = (r.sell || []).concat(r.keep || []).concat(r.give || []);
  $("sell").innerHTML = all.map(e => `<div class="line" style="color:${col(e.colour)}">${esc(e.text)}</div>`).join("")
    || '<div class="empty">nothing to sell or keep right now</div>';
}
// One rendered frame from a live payload (the first-paint fetch and every SSE event).
function render(r) {
  if (!r || !r.live) {
    $("who").innerHTML = '<span class="waiting">waiting for game…</span>';
    for (const id of ["guide", "levels", "gear", "merc", "extras", "targets", "stash", "sell"]) $(id).innerHTML = "";
    $("updated").textContent = "";
    document.title = "D2R Dashboard (waiting for game)";
    $("mercbox").hidden = true;
    return;
  }
  const p = r.player;
  $("who").innerHTML = `${esc(p.name)}<small>${esc([p.class, p.level != null ? "level " + p.level : null, p.difficulty, r.area].filter(Boolean).join(" · "))}</small>`;
  $("updated").textContent = "updated " + new Date(r.updated_at * 1000).toLocaleTimeString();
  document.title = `${p.name}${p.level != null ? " L" + p.level : ""} · ${r.area || "?"} — D2R Dashboard`;
  guide(r.guide);
  levels(r.levels);
  gear(r.gear);
  stash(r);
  sell(r);
}
// First paint from the one-shot endpoint, then live updates from the Server-Sent-Events
// stream: the server pushes whenever the JSON changes and the browser reconnects on its own.
fetch("/api/live").then(r => r.json()).then(render).catch(() => {});
const stream = new EventSource("/api/live-stream");
stream.onmessage = e => {
  try { render(JSON.parse(e.data)); } catch (err) { /* a partial event: the next one wins */ }
};
stream.onerror = () => { /* EventSource retries on its own */ };
</script></body></html>
"#;

/// The whole dashboard page: [`DASHBOARD_HEAD`] plus [`DASHBOARD_SCRIPT`].
fn dashboard_page() -> String {
    format!("{DASHBOARD_HEAD}{DASHBOARD_SCRIPT}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Result, anyhow};
    use std::io::{Cursor, Read};

    use crate::build::Damage;
    use crate::config;

    #[test]
    fn set_bis_selection_preserves_comments_and_round_trips() -> Result<()> {
        let text = "# top\n[bis.Hork]\n# build line\nbuild = \"Blood Boil\"\n";
        let out = set_bis_selection(text, "Hork", Some("blood boil")).ok_or_else(|| anyhow!("no edit"))?;
        assert!(out.contains("# top") && out.contains("# build line"), "{out}");
        let (config, _) = config::parse(&out)?;
        assert_eq!(
            config.bis.get("hork").map(|b| b.build.as_str()),
            Some("blood boil")
        );
        Ok(())
    }

    #[test]
    fn set_bis_selection_creates_missing_tables_and_names_with_spaces() -> Result<()> {
        let out =
            set_bis_selection("", "Demon Master", Some("blood boil")).ok_or_else(|| anyhow!("no edit"))?;
        let (config, warnings) = config::parse(&out)?;
        assert_eq!(
            config.bis.get("demon master").map(|b| b.build.as_str()),
            Some("blood boil")
        );
        assert_eq!(warnings, Vec::<String>::new());
        Ok(())
    }

    #[test]
    fn clearing_a_selection_removes_the_build_and_empty_entries() -> Result<()> {
        // A selection-only entry vanishes entirely; an inline one only loses its build.
        let out = set_bis_selection("[bis.Hork]\nbuild = \"b\"\n", "Hork", None)
            .ok_or_else(|| anyhow!("no edit"))?;
        assert_eq!(out, "", "the emptied entry is removed: {out}");
        let inline = "[bis.Hork]\nbuild = \"b\"\n[bis.Hork.slots]\nweapon = [\"Insight@27\"]\n";
        let out = set_bis_selection(inline, "Hork", None).ok_or_else(|| anyhow!("no edit"))?;
        let (config, _) = config::parse(&out)?;
        let bis = config.bis.get("hork").ok_or_else(|| anyhow!("gone"))?;
        assert_eq!(bis.build, "");
        assert_eq!(
            bis.slots.get("weapon").map(Vec::as_slice),
            Some(&["Insight@27".to_owned()][..])
        );
        Ok(())
    }

    #[test]
    fn set_bis_selection_rejects_non_table_bis() {
        // A bare value, and a non-table entry: the save would silently do nothing, so the
        // caller must get `None` and answer the browser with an error instead.
        assert_eq!(set_bis_selection("bis = \"oops\"\n", "Hork", Some("b")), None);
        assert_eq!(set_bis_selection("[bis]\nHork = 5\n", "Hork", Some("b")), None);
    }

    /// A config with one build definition and a selection-only character, plus a builds-only
    /// character, for the resolved-view tests.
    fn resolved_config() -> crate::config::Config {
        let mut config = crate::config::Config::default();
        config.build_defs.insert(
            "blood boil".to_owned(),
            crate::config::Build {
                name: "Blood Boil".to_owned(),
                damages: vec![Damage::Physical, Damage::Fire],
                notes: Some("guide".to_owned()),
                slots: [("weapon".to_owned(), vec!["Heart of the Oak@55".to_owned()])].into(),
                merc_slots: [("weapon".to_owned(), vec!["Insight@27".to_owned()])].into(),
                ..crate::config::Build::default()
            },
        );
        config.bis.insert(
            "demonmaster".to_owned(),
            crate::config::Bis {
                build: "blood boil".to_owned(),
                slots: BTreeMap::new(),
                merc_slots: BTreeMap::new(),
            },
        );
        config
    }

    #[test]
    fn config_json_resolves_the_selected_build() {
        // The resolved view: the selection-only character gets the build's display name,
        // damages and annotated pieces; a builds-only character rides along for its select.
        let json = config_json(&resolved_config(), &GearInfo::default());
        let chars = json
            .get("characters")
            .and_then(|v| v.as_array())
            .map_or_else(Vec::new, Vec::clone);
        assert_eq!(chars.len(), 1, "one character: {chars:?}");
        let char = chars.first().map_or(serde_json::Value::Null, Clone::clone);
        assert_eq!(char.get("name"), Some(&serde_json::json!("demonmaster")));
        assert_eq!(char.get("build"), Some(&serde_json::json!("blood boil")));
        assert_eq!(char.get("build_name"), Some(&serde_json::json!("Blood Boil")));
        assert_eq!(
            char.get("damages"),
            Some(&serde_json::json!(["physical", "fire"]))
        );
        // The piece carries its level, tick and runeword info.
        let weapon = char
            .pointer("/slots/weapon/0")
            .map_or(serde_json::Value::Null, Clone::clone);
        assert_eq!(
            weapon.get("piece"),
            Some(&serde_json::json!("Heart of the Oak@55"))
        );
        assert_eq!(weapon.get("name"), Some(&serde_json::json!("Heart of the Oak")));
        assert_eq!(weapon.get("level"), Some(&serde_json::json!(55)));
        assert_eq!(weapon.get("tick"), Some(&serde_json::Value::Null));
        assert_eq!(weapon.get("better"), Some(&serde_json::json!(false)));
        assert_eq!(
            weapon.get("runes"),
            Some(&serde_json::json!(["Ko", "Vex", "Pul", "Thul"]))
        );
        assert_eq!(
            char.pointer("/merc_slots/weapon/0/name"),
            Some(&serde_json::json!("Insight"))
        );
    }

    #[test]
    fn config_json_marks_owned_stash_and_better_pieces() {
        // The live gear state annotates the pieces: owned ✓, stash ✓(stash), and a set/unique
        // equipped piece strikes the slot's chase pieces through.
        let gear = GearInfo {
            owned: vec!["Heart of the Oak".to_owned()],
            stash: vec!["Insight".to_owned()],
            equipped: vec![(4, 7)], // body slot 4 = weapon, quality 7 = unique
            merc_equipped: vec![],
        };
        let json = config_json(&resolved_config(), &gear);
        let char = json
            .pointer("/characters/0")
            .map_or(serde_json::Value::Null, Clone::clone);
        assert_eq!(
            char.pointer("/slots/weapon/0/tick"),
            Some(&serde_json::json!("owned"))
        );
        assert_eq!(
            char.pointer("/slots/weapon/0/better"),
            Some(&serde_json::json!(true)),
            "a unique equipped in the weapon slot beats the chase pieces"
        );
        assert_eq!(
            char.pointer("/merc_slots/weapon/0/tick"),
            Some(&serde_json::json!("stash"))
        );
    }

    /// The gear-grid JSON of one column, by title.
    fn column_of<'a>(gear: &'a serde_json::Value, title: &str) -> &'a serde_json::Value {
        gear.get("columns")
            .and_then(|c| c.as_array())
            .and_then(|cols| {
                cols.iter()
                    .find(|c| c.get("title").and_then(|t| t.as_str()) == Some(title))
            })
            .unwrap_or(&serde_json::Value::Null)
    }

    /// The `(name, level, state, tick)` of a column's pieces.
    fn piece_states(col: &serde_json::Value) -> Vec<(String, serde_json::Value, String, serde_json::Value)> {
        col.get("pieces")
            .and_then(|p| p.as_array())
            .map_or_else(Vec::new, |pieces| {
                pieces
                    .iter()
                    .map(|p| {
                        (
                            p.get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_owned(),
                            p.get("level").cloned().unwrap_or_default(),
                            p.get("state")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_owned(),
                            p.get("tick").cloned().unwrap_or_default(),
                        )
                    })
                    .collect()
            })
    }

    #[test]
    fn gear_grid_states_and_equipped_headers() {
        use crate::offsets::item_data;
        // clvl 42: the gate is at 47.
        let equipped = [
            // A magic Shako (base uap, levelreq 43): unnameable, but comparable.
            crate::game::Equipped {
                body: 1,
                quality: item_data::QUALITY_MAGIC,
                code: Some("uap".to_owned()),
                name: None,
                file_index: None,
                runeword: false,
            },
            // A finished runeword on a Crystal Sword: uncomparable.
            crate::game::Equipped {
                body: 4,
                quality: item_data::QUALITY_NORMAL,
                code: Some("crs".to_owned()),
                name: None,
                file_index: None,
                runeword: true,
            },
            // An identified Harlequin Crest: exact name and level.
            equipped_unique(2),
        ];
        let slots = [
            (
                "helm",
                vec!["Peasant Crown@25".to_owned(), "Valkyrie Wing@71".to_owned()],
            ),
            ("amulet", vec!["Harlequin Crest@62".to_owned()]),
            ("weapon", vec!["Heart of the Oak@55".to_owned()]),
            ("ring1", vec!["Naj's Rite@30".to_owned()]),
            ("ring2", vec!["Dwarf Star@45".to_owned()]),
            ("offhand", vec!["Spirit Monarch@35".to_owned()]),
        ];
        let gear = gear_grid(&slots, &equipped, &["Heart of the Oak".to_owned()]);
        // 10 columns in the user's two rows of five.
        let titles: Vec<&str> = gear.get("columns").map_or_default(|v| {
            v.as_array()
                .iter()
                .flat_map(|pieces| pieces.iter())
                .filter_map(|x| x.get("title").and_then(|val| val.as_str()))
                .collect()
        });
        assert_eq!(
            titles,
            [
                "Helm", "Body", "Hands", "Belt", "Foot", "Amulet", "Ring", "Weapons", "Off-Hand", "Swap"
            ]
        );
        // Helm: a magic equipped piece is unnameable but comparable — the base level 43
        // strikes the below pieces; the 71 one is better but above the 47 gate.
        let helm = column_of(&gear, "Helm");
        assert_eq!(
            helm.get("equipped").cloned().unwrap_or_default(),
            serde_json::json!([{ "name": "(unknown)", "level": 43 }])
        );
        assert_eq!(
            piece_states(helm),
            vec![
                (
                    "Peasant Crown".to_owned(),
                    serde_json::json!(25),
                    "worse".to_owned(),
                    serde_json::Value::Null
                ),
                (
                    "Valkyrie Wing".to_owned(),
                    serde_json::json!(71),
                    "too_high".to_owned(),
                    serde_json::Value::Null
                ),
            ]
        );
        // Amulet: the unique equipped piece is named with its exact level; a below piece
        // is worse, one above (and inside the gate) is better.
        let amulet = column_of(&gear, "Amulet");
        assert_eq!(
            amulet.get("equipped").cloned().unwrap_or_default(),
            serde_json::json!([{ "name": "Harlequin Crest", "level": 62 }])
        );
        // 62 ≤ 62 → worse even though the piece is the same item.
        assert_eq!(piece_states(amulet).first().map(|p| p.2.as_str()), Some("worse"),);
        // Weapons: the equipped runeword is uncomparable, so nothing is struck through
        // (and the page's hint says so); the 55 piece would be better but sits above the
        // 47 gate — too_high; the owned piece is ticked.
        let weapons = column_of(&gear, "Weapons");
        assert_eq!(
            weapons.get("equipped").cloned().unwrap_or_default(),
            serde_json::json!([{ "name": "runeword (unnamed)", "level": serde_json::Value::Null }])
        );
        assert_eq!(
            piece_states(weapons),
            vec![(
                "Heart of the Oak".to_owned(),
                serde_json::json!(55),
                "too_high".to_owned(),
                serde_json::json!("owned")
            )]
        );
        // Off-hand: nothing equipped, so the piece is better; no two-handed note (the
        // equipped Crystal Sword is one-handed).
        let offhand = column_of(&gear, "Off-Hand");
        assert_eq!(
            piece_states(offhand).first().map(|p| p.2.as_str()),
            Some("better"),
        );
        assert_eq!(offhand.get("note"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn gear_grid_merges_the_ring_slots() {
        // Two equipped rings: the unique Dwarf Star (row 274, lvl req 45) and a magic one;
        // the merged column compares against the higher of the two requirements.
        let equipped = [
            crate::game::Equipped {
                body: 6,
                quality: crate::offsets::item_data::QUALITY_UNIQUE,
                code: Some("rin".to_owned()),
                name: Some("Dwarf Star".to_owned()),
                file_index: Some(274),
                runeword: false,
            },
            crate::game::Equipped {
                body: 7,
                quality: crate::offsets::item_data::QUALITY_MAGIC,
                code: Some("rin".to_owned()),
                name: None,
                file_index: None,
                runeword: false,
            },
        ];
        let slots = [
            ("ring1", vec!["Dwarf Star@45".to_owned(), "Ring A@20".to_owned()]),
            ("ring2", vec!["Ring A@20".to_owned(), "Ring C@46".to_owned()]),
        ];
        let gear = gear_grid(&slots, &equipped, &[]);
        let ring = column_of(&gear, "Ring");
        // Both equipped rings in the header, deduplicated pieces from both slots in order.
        assert_eq!(
            ring.get("equipped").map(|e| e.as_array().map_or(0, Vec::len)),
            Some(2)
        );
        let names: Vec<&str> = ring.get("pieces").map_or_default(|v| {
            v.as_array()
                .iter()
                .flat_map(|pieces| pieces.iter())
                .filter_map(|x| x.get("name").and_then(|val| val.as_str()))
                .collect()
        });
        assert_eq!(names, ["Dwarf Star", "Ring A", "Ring C"]);
        // Equipped base: the stronger ring's level (45); the 46 piece is better (the 47
        // gate still holds at clvl 42), the rest worse.
        let states: Vec<&str> = ring.get("pieces").map_or_default(|v| {
            v.as_array()
                .iter()
                .flat_map(|pieces| pieces.iter())
                .filter_map(|x| x.get("state").and_then(|val| val.as_str()))
                .collect()
        });
        assert_eq!(states, ["worse", "worse", "better"]);
    }

    #[test]
    fn gear_grid_notes_two_handed_weapons() {
        // An equipped Elder Staff (code 6cs, weapons.txt `2handed` 1): the off-hand is
        // unusable, and the note says so. With the one-handed Crystal Sword: no note.
        for (code, expected) in [("6cs", Some("(two-handed weapon equipped)")), ("crs", None)] {
            let equipped = [crate::game::Equipped {
                body: 4,
                quality: crate::offsets::item_data::QUALITY_NORMAL,
                code: Some(code.to_owned()),
                name: None,
                file_index: None,
                runeword: false,
            }];
            let gear = gear_grid(
                &[("offhand", vec!["Spirit Monarch@35".to_owned()])],
                &equipped,
                &[],
            );
            assert_eq!(
                column_of(&gear, "Off-Hand").get("note").and_then(|n| n.as_str()),
                expected,
                "weapon code {code}"
            );
        }
    }

    #[test]
    fn builds_json_lists_slug_name_damages_and_counts() {
        let json = builds_json(&resolved_config());
        let builds = json
            .get("builds")
            .and_then(|v| v.as_array())
            .map_or_else(Vec::new, Vec::clone);
        assert_eq!(builds.len(), 1, "{builds:?}");
        let build = builds.first().map_or(serde_json::Value::Null, Clone::clone);
        assert_eq!(build.get("slug"), Some(&serde_json::json!("blood boil")));
        assert_eq!(build.get("name"), Some(&serde_json::json!("Blood Boil")));
        assert_eq!(
            build.get("damages"),
            Some(&serde_json::json!(["physical", "fire"]))
        );
        assert_eq!(build.get("notes"), Some(&serde_json::json!("guide")));
        assert_eq!(
            build.get("slots"),
            Some(&serde_json::json!({"weapon": 1})),
            "per-slot piece counts"
        );
        assert_eq!(build.get("merc_slots"), Some(&serde_json::json!({"weapon": 1})));
    }

    #[test]
    fn reads_requests() -> Result<()> {
        let request = b"POST /api/bis\r\nContent-Length: 5\r\n\r\nhello";
        let Some(Parsed::Request(method, path, body)) = read_request(&mut Cursor::new(&request[..])) else {
            return Err(anyhow!("request not parsed"));
        };
        assert_eq!((method.as_str(), path.as_str()), ("POST", "/api/bis"));
        assert_eq!(body, b"hello");
        Ok(())
    }

    #[test]
    fn content_length_header_is_case_insensitive() -> Result<()> {
        let request = b"POST /api/bis\r\ncontent-length: 2\r\n\r\nok";
        let Some(Parsed::Request(_, _, body)) = read_request(&mut Cursor::new(&request[..])) else {
            return Err(anyhow!("request not parsed"));
        };
        assert_eq!(body, b"ok");
        Ok(())
    }

    #[test]
    fn oversized_bodies_are_rejected_without_allocating() {
        let request = format!("POST /api/bis\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        assert!(matches!(
            read_request(&mut Cursor::new(request)),
            Some(Parsed::TooLarge)
        ));
        // At the cap itself the body is still served (a complete 1 MiB request).
        let header = format!("POST /api/bis\r\nContent-Length: {MAX_BODY}\r\n\r\n");
        let mut request = header.into_bytes();
        request.resize(request.len() + MAX_BODY, 0);
        assert!(matches!(
            read_request(&mut Cursor::new(request)),
            Some(Parsed::Request(_, _, _))
        ));
    }

    /// A stash item for the panel tests (identified, not a runeword).
    fn stash_item(name: &str, code: &str, quality: u32, quantity: Option<u32>) -> crate::stash::StashItem {
        crate::stash::StashItem {
            name: name.to_owned(),
            code: Some(code.to_owned()),
            quality: u8::try_from(quality).unwrap_or_default(),
            identified: true,
            ethereal: name.starts_with("eth "),
            runeword: false,
            quantity,
        }
    }

    /// Page 1 with an identified unique and a plain ethereal base, an empty page 2, and the
    /// materials tab with gems, runes and potions in memory order (one emptied stack).
    fn test_stash() -> Vec<crate::stash::StashTab> {
        use crate::offsets::item_data::{QUALITY_NORMAL, QUALITY_UNIQUE};
        use crate::stash::{StashTab, TabKind};
        let n = QUALITY_NORMAL;
        vec![
            StashTab {
                kind: TabKind::Page(1),
                items: vec![
                    stash_item("Harlequin Crest", "uap", QUALITY_UNIQUE, None),
                    stash_item("eth Crystal Sword 4os", "crs", n, None),
                ],
            },
            StashTab {
                kind: TabKind::Page(2),
                items: vec![],
            },
            StashTab {
                kind: TabKind::Materials,
                items: vec![
                    stash_item("Rejuvenation Potion", "rvs", n, Some(50)),
                    stash_item("Io Rune", "r16", n, Some(1)),
                    stash_item("Perfect Topaz", "gpy", n, Some(2)),
                    stash_item("El Rune", "r01", n, Some(8)),
                    stash_item("Chipped Topaz", "gcy", n, Some(19)),
                    stash_item("Flawless Amethyst", "gzv", n, Some(0)),
                    stash_item("Chipped Amethyst", "gcv", n, Some(3)),
                    stash_item("Full Rejuvenation Potion", "rvl", n, Some(43)),
                ],
            },
        ]
    }

    /// A `LiveInfo` over borrowed test data: difficulty 2 is hell.
    fn live_info<'a>(
        class: u32,
        blocks: &'a crate::render::TextBlocks,
        equipped: &'a [(u8, u32)],
        stash: &'a [crate::stash::StashTab],
        stash_owned: &'a [String],
    ) -> LiveInfo<'a> {
        LiveInfo {
            player_name: "Hork",
            class,
            char_level: Some(42),
            difficulty: Some(2),
            area: "Durance of Hate Level 3",
            blocks,
            bis: None,
            build: None,
            equipped,
            equipped_items: &[],
            owned: &[],
            stash,
            stash_owned,
            updated_at: 1_700_000_000,
        }
    }

    /// A gear grid over a one-slot build, one equipped piece and one owned name; the
    /// pieces' per-column state is what the tests assert on.
    fn gear_grid<'a>(
        slots: &[(&str, Vec<String>)],
        equipped_items: &'a [crate::game::Equipped],
        owned: &'a [String],
    ) -> serde_json::Value {
        let bis = crate::config::Bis {
            build: "test".to_owned(),
            slots: slots
                .iter()
                .map(|(slot, pieces)| ((*slot).to_owned(), pieces.clone()))
                .collect(),
            merc_slots: BTreeMap::new(),
        };
        let info = LiveInfo {
            player_name: "Hork",
            class: 0,
            char_level: Some(42),
            difficulty: Some(2),
            area: "test",
            blocks: &crate::render::TextBlocks::default(),
            bis: Some(&bis),
            build: None,
            equipped: &[],
            equipped_items,
            owned,
            stash: &[],
            stash_owned: &[],
            updated_at: 1_700_000_000,
        };
        gear_json(&info)
    }

    /// A unique equipped piece: Harlequin Crest's live-verified row (248 → lvl req 62).
    fn equipped_unique(body: u8) -> crate::game::Equipped {
        crate::game::Equipped {
            body,
            quality: crate::offsets::item_data::QUALITY_UNIQUE,
            code: Some("uap".to_owned()),
            name: Some("Harlequin Crest".to_owned()),
            file_index: Some(248),
            runeword: false,
        }
    }

    /// The `{ "text", "colour" }` pairs of a block's lines in the live JSON.
    fn lines_of(json: &serde_json::Value, key: &str) -> Vec<(String, String)> {
        json.get(key)
            .and_then(|v| v.as_array())
            .map_or_else(Vec::new, |lines| {
                lines
                    .iter()
                    .map(|l| {
                        (
                            l.get("text")
                                .and_then(|t| t.as_str())
                                .unwrap_or_default()
                                .to_owned(),
                            l.get("colour")
                                .and_then(|c| c.as_str())
                                .unwrap_or_default()
                                .to_owned(),
                        )
                    })
                    .collect()
            })
    }

    #[test]
    fn live_json_has_the_documented_shape() -> Result<()> {
        let blocks = crate::render::TextBlocks {
            guide: vec![crate::info::Band::plain("fcr (7 / 105)", crate::info::GUIDE_TEXT)],
            level_full: vec![
                crate::info::LevelEntry {
                    text: "Level 3 → a (3)".to_owned(),
                    mlvl: 3,
                    bucket: "equal",
                },
                crate::info::LevelEntry {
                    text: "b (5)".to_owned(),
                    mlvl: 5,
                    bucket: "above",
                },
            ],
            keep: vec!["Keep: Harlequin Crest (S)".to_owned()],
            sell: vec!["Sell: Gravenspine (inv)".to_owned()],
            give: vec!["Give away: Lem Rune (stash)".to_owned()],
            bis: vec![crate::info::BisLine {
                text: "helm: Harlequin Crest (82)".to_owned(),
                tick: None,
                runeword: None,
            }],
        };
        let equipped = [(1, 7)];
        let stash = test_stash();
        let stash_owned = ["Harlequin Crest".to_owned()];
        let json = serde_json::from_str::<serde_json::Value>(&live_json(&live_info(
            1,
            &blocks,
            &equipped,
            &stash,
            &stash_owned,
        )))
        .map_err(|e| anyhow!("valid json: {e}"))?;
        assert_eq!(json.get("live"), Some(&serde_json::json!(true)));
        assert_eq!(json.get("updated_at"), Some(&serde_json::json!(1_700_000_000)));
        assert_eq!(
            json.get("player"),
            Some(&serde_json::json!({
                "name": "Hork", "class": "sorceress", "level": 42, "difficulty": "Hell",
            }))
        );
        assert_eq!(
            json.get("area").and_then(|v| v.as_str()),
            Some("Durance of Hate Level 3")
        );
        // Each block becomes `{ "text", "colour" }` lines in the block's overlay colour.
        let colour_of = |key: &str| -> Vec<(String, String)> { lines_of(&json, key) };
        assert_eq!(
            colour_of("guide"),
            vec![("fcr (7 / 105)".to_owned(), "#c882ff".to_owned())]
        );
        // The level-suggestion band: structured entries with the monster level and the
        // below/equal/above bucket, in the own-level-first reading order.
        let levels = json
            .get("levels")
            .and_then(|v| v.as_array())
            .map_or_else(Vec::new, Vec::clone);
        assert_eq!(
            levels,
            vec![
                serde_json::json!({"text": "Level 3 → a (3)", "colour": "#78d2ff", "mlvl": 3, "bucket": "equal"}),
                serde_json::json!({"text": "b (5)", "colour": "#78d2ff", "mlvl": 5, "bucket": "above"}),
            ],
            "{levels:?}"
        );
        assert_eq!(
            colour_of("keep"),
            vec![("Keep: Harlequin Crest (S)".to_owned(), "#5ae65a".to_owned())]
        );
        assert_eq!(
            colour_of("sell"),
            vec![("Sell: Gravenspine (inv)".to_owned(), "#afafaf".to_owned())]
        );
        assert_eq!(
            colour_of("give"),
            vec![("Give away: Lem Rune (stash)".to_owned(), "#ffe13c".to_owned())]
        );
        assert_eq!(
            colour_of("bis"),
            vec![("helm: Harlequin Crest (82)".to_owned(), "#ffd278".to_owned())]
        );
        assert_eq!(
            colour_of("equipped"),
            vec![("helm: unique".to_owned(), "#ffffff".to_owned())]
        );
        assert_eq!(
            json.get("stash_owned"),
            Some(&serde_json::json!(["Harlequin Crest"]))
        );
        Ok(())
    }

    #[test]
    fn live_json_has_the_stash_pages_and_materials() -> Result<()> {
        let blocks = crate::render::TextBlocks::default();
        let stash = test_stash();
        let json =
            serde_json::from_str::<serde_json::Value>(&live_json(&live_info(1, &blocks, &[], &stash, &[])))
                .map_err(|e| anyhow!("valid json: {e}"))?;
        // The normal pages in UI order with quality-tagged item lines (uniques gold); the
        // materials tab is not a page.
        assert_eq!(
            json.get("stash_pages"),
            Some(&serde_json::json!([
                {"page": 1, "items": [
                    {"text": "Harlequin Crest (unique)", "colour": "#ffd278"},
                    {"text": "eth Crystal Sword 4os", "colour": "#dddddd"},
                ]},
                {"page": 2, "items": []},
            ]))
        );
        // Gems by colour then grade, runes by number, the rest by name; the empty stack is out.
        assert_eq!(
            json.get("stash_materials"),
            Some(&serde_json::json!({
                "gems": [
                    {"name": "Chipped Amethyst", "qty": 3, "code": "gcv"},
                    {"name": "Chipped Topaz", "qty": 19, "code": "gcy"},
                    {"name": "Perfect Topaz", "qty": 2, "code": "gpy"},
                ],
                "runes": [
                    {"name": "El Rune", "qty": 8, "code": "r01"},
                    {"name": "Io Rune", "qty": 1, "code": "r16"},
                ],
                "other": [
                    {"name": "Full Rejuvenation Potion", "qty": 43, "code": "rvl"},
                    {"name": "Rejuvenation Potion", "qty": 50, "code": "rvs"},
                ],
            }))
        );
        assert_eq!(json.get("stash"), None, "the flat line list is gone");
        Ok(())
    }

    #[test]
    fn live_json_bis_lines_carry_structural_ticks() -> Result<()> {
        let blocks = crate::render::TextBlocks {
            bis: vec![
                crate::info::BisLine {
                    text: "helm: Harlequin Crest (82)".to_owned(),
                    tick: Some(crate::info::Tick::Owned),
                    runeword: None,
                },
                crate::info::BisLine {
                    text: "merc body: ✓ Treachery (23)".to_owned(),
                    tick: Some(crate::info::Tick::Stash),
                    runeword: None,
                },
                crate::info::BisLine {
                    text: "weapon: Mang Song's Lesson (82)".to_owned(),
                    tick: None,
                    runeword: None,
                },
            ],
            ..crate::render::TextBlocks::default()
        };
        let json =
            serde_json::from_str::<serde_json::Value>(&live_json(&live_info(1, &blocks, &[], &[], &[])))
                .map_err(|e| anyhow!("valid json: {e}"))?;
        let ticks: Vec<String> = json
            .get("bis")
            .and_then(|v| v.as_array())
            .map_or_else(Vec::new, |lines| {
                lines
                    .iter()
                    .map(|l| {
                        l.get("tick").map_or_else(
                            || "missing".to_owned(),
                            |t| {
                                if t.is_null() {
                                    String::new()
                                } else {
                                    t.as_str().unwrap_or("not a string").to_owned()
                                }
                            },
                        )
                    })
                    .collect()
            });
        assert_eq!(ticks, ["owned", "stash", ""]);
        Ok(())
    }

    #[test]
    fn live_json_bis_lines_carry_runeword_runes_and_bases() -> Result<()> {
        // A runeword piece carries its runes and bases for the dashboard's hint line; a
        // non-runeword piece carries neither field.
        let treachery = crate::runewords_data::lookup("Treachery").copied();
        let blocks = crate::render::TextBlocks {
            bis: vec![
                crate::info::BisLine {
                    text: "merc body: Treachery (43)".to_owned(),
                    tick: None,
                    runeword: treachery,
                },
                crate::info::BisLine {
                    text: "helm: Harlequin Crest (82)".to_owned(),
                    tick: None,
                    runeword: None,
                },
            ],
            ..crate::render::TextBlocks::default()
        };
        let json =
            serde_json::from_str::<serde_json::Value>(&live_json(&live_info(7, &blocks, &[], &[], &[])))
                .map_err(|e| anyhow!("valid json: {e}"))?;
        let bis = json
            .get("bis")
            .and_then(|v| v.as_array())
            .map_or_else(Vec::new, Vec::clone);
        assert_eq!(bis.len(), 2, "{bis:?}");
        let first = bis.first().map_or(serde_json::Value::Null, Clone::clone);
        assert_eq!(
            first.get("runes"),
            Some(&serde_json::json!(["Shael", "Thul", "Lem"]))
        );
        assert_eq!(first.get("bases"), Some(&serde_json::json!(["armor"])));
        let second = bis.get(1).map_or(serde_json::Value::Null, Clone::clone);
        assert!(second.get("runes").is_none(), "no runes for uniques: {second:?}");
        assert!(second.get("bases").is_none(), "no bases for uniques: {second:?}");
        Ok(())
    }

    #[test]
    fn unknown_classes_slots_and_qualities_are_shown_not_dropped() -> Result<()> {
        let blocks = crate::render::TextBlocks::default();
        let json = serde_json::from_str::<serde_json::Value>(&live_json(&live_info(
            13,
            &blocks,
            &[(13, 42)],
            &[],
            &[],
        )))
        .map_err(|e| anyhow!("valid json: {e}"))?;
        assert_eq!(
            json.pointer("/player/class").and_then(|v| v.as_str()),
            Some("class 13")
        );
        assert_eq!(
            json.pointer("/equipped/0/text").and_then(|v| v.as_str()),
            Some("slot 13: quality 42")
        );
        Ok(())
    }

    #[test]
    fn no_live_json_parses_with_live_false() -> Result<()> {
        let json = serde_json::from_str::<serde_json::Value>(NO_LIVE_JSON)
            .map_err(|e| anyhow!("valid json: {e}"))?;
        assert_eq!(json.get("live"), Some(&serde_json::json!(false)));
        Ok(())
    }

    /// Reads one `\n\n`-terminated SSE chunk (the first call returns the header block);
    /// times out like [`std::io::Read`] does when nothing arrives within the socket's
    /// read timeout.
    fn read_chunk(client: &mut std::net::TcpStream) -> std::io::Result<String> {
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            let read = client.read(&mut byte)?;
            if read == 0 {
                return Err(std::io::Error::other("connection closed"));
            }
            buf.push(byte[0]);
            // Data frames end `\n\n`, the header block `\r\n\r\n`.
            if buf.ends_with(b"\n\n") || buf.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8(buf).map_err(std::io::Error::other)
    }

    #[test]
    fn live_stream_pushes_frames_and_skips_identical_ones() -> Result<()> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        // The handler thread runs until the client hangs up (write error); not joined.
        thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                api_live_stream(stream);
            }
        });
        let mut client = std::net::TcpStream::connect(addr)?;
        client.set_read_timeout(Some(Duration::from_secs(1)))?;
        let head = read_chunk(&mut client)?;
        assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
        assert!(
            head.contains("text/event-stream") && head.contains("no-store"),
            "{head}"
        );
        // Nothing published yet: no event (the poll only sends changes).
        assert!(read_chunk(&mut client).is_err(), "no frame before a publish");
        publish_live("{\"live\": true, \"n\": 1}".to_owned());
        assert_eq!(read_chunk(&mut client)?, "data: {\"live\": true, \"n\": 1}\n\n");
        // The same JSON published again ([`publish_live`] skips it, the loop compares):
        // no duplicate frame within the poll interval + margin.
        publish_live("{\"live\": true, \"n\": 1}".to_owned());
        assert!(read_chunk(&mut client).is_err(), "no duplicate frame");
        // A changed frame goes out; a detached game publishes the live:false marker.
        publish_live("{\"live\": true, \"n\": 2}".to_owned());
        assert_eq!(read_chunk(&mut client)?, "data: {\"live\": true, \"n\": 2}\n\n");
        clear_live();
        assert_eq!(read_chunk(&mut client)?, "data: {\"live\": false}\n\n");
        Ok(())
    }

    #[test]
    fn gear_carries_build_details_merc_and_extras() {
        let build = crate::config::Build {
            name: "Blood Boil".to_owned(),
            targets: vec!["75% FCR".to_owned()],
            extras: [("charms".to_owned(), vec!["Annihilus@70".to_owned()])].into(),
            pieces: [(
                "Harlequin Crest".to_owned(),
                crate::config::PieceInfo {
                    kind: "unique".to_owned(),
                    base: "Shako".to_owned(),
                    stats: vec!["+2 to All Skills".to_owned()],
                    ..crate::config::PieceInfo::default()
                },
            )]
            .into(),
            ..crate::config::Build::default()
        };
        let bis = crate::config::Bis {
            build: "Blood Boil".to_owned(),
            slots: [("helm".to_owned(), vec!["Harlequin Crest@62".to_owned()])].into(),
            merc_slots: [("weapon".to_owned(), vec!["Insight@27".to_owned()])].into(),
        };
        let blocks = crate::render::TextBlocks::default();
        let mut info = live_info(0, &blocks, &[], &[], &[]);
        info.bis = Some(&bis);
        info.build = Some(&build);
        let gear = gear_json(&info);
        let at = |path: &str| gear.pointer(path).cloned().unwrap_or_default();
        assert_eq!(at("/build"), "Blood Boil");
        assert_eq!(at("/targets"), serde_json::json!(["75% FCR"]));
        let helm = at("/columns/0/pieces/0");
        assert_eq!(
            (
                at("/columns/0/pieces/0/name"),
                at("/columns/0/pieces/0/kind"),
                at("/columns/0/pieces/0/base")
            ),
            (
                serde_json::json!("Harlequin Crest"),
                serde_json::json!("unique"),
                serde_json::json!("Shako")
            )
        );
        assert_eq!(
            at("/columns/0/pieces/0/stats"),
            serde_json::json!(["+2 to All Skills"])
        );
        assert!(helm.get("why").is_none(), "unset details are left out");
        assert_eq!(at("/merc/0/title"), "Merc weapon");
        assert_eq!(at("/merc/0/pieces/0/name"), "Insight");
        assert_eq!(at("/extras/0/title"), "charms");
        assert_eq!(at("/extras/0/pieces/0/level"), 70);
    }

    #[test]
    fn root_is_the_dashboard_and_config_moved() {
        for path in ["/", "/index.html", "/dashboard"] {
            let r = html_page(path);
            assert!(
                r.starts_with("HTTP/1.1 200") && r.contains("<title>D2R Dashboard</title>"),
                "{path}"
            );
        }
        let config = html_page("/config");
        assert!(config.contains("d2r-map-overlay config"), "{config}");
        assert!(html_page("/nope").starts_with("HTTP/1.1 404"));
    }
}
