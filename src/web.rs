//! The companion web app: a small HTTP server on `localhost` that lets the user select each
//! character's build in a browser form, writing ONLY the `[bis.<name>]` `build` selection back
//! to `config.toml` with comments preserved (user decision, 2026-11-17). The gear itself is
//! authored by the agent in `builds/*.toml` and shown read-only, annotated with the live ✓
//! (owned / in the shared stash) and strike-through where the equipped piece is already
//! better. The overlay watches the config file and the `builds/` directory and hot-reloads
//! them (see `live`). The app also serves the live dashboard: the level, keep, sell, give-away
//! and `BiS` blocks (the overlay itself draws only the breakpoint guide), published by `live`
//! each tick and read by `GET /api/live`.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError, RwLock};
use std::thread;
use std::time::Duration;

use toml_edit::DocumentMut;

/// The port the page listens on (localhost only).
pub const PORT: u16 = 8471;

/// Requests whose `Content-Length` is larger are rejected before the body is read or
/// allocated: the client controls that header, and a huge allocation aborts the process.
const MAX_BODY: usize = 1 << 20;
/// Read timeout per request step: a stalled client must not hold a thread forever.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

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
/// write lock, a short string.
pub fn publish_live(json: String) {
    // A poisoned lock still holds a working cell; a poisoned writer can only mean a panic
    // elsewhere while writing, and the next tick overwrites the value anyway.
    *live_cell().write().unwrap_or_else(PoisonError::into_inner) = Some(json);
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
    eprintln!("web: config page at http://localhost:{PORT}");
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
            ("GET", "/" | "/index.html") => page(),
            ("GET", "/dashboard") => respond("200 OK", "text/html", dashboard_page()),
            ("GET", "/api/config") => api_config(),
            ("GET", "/api/builds") => api_builds(),
            ("GET", "/api/live") => api_live(),
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
    /// The equipped `(body slot, quality)` pairs, shown under the `BiS` chase list.
    pub equipped: &'a [(u8, u32)],
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
/// "give": […], "bis": […], "equipped": […], "stash": […], "stash_owned": […] }`, where
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
/// the dashboard's Sell panel.
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
        "equipped": text_lines(&equipped_lines(info.equipped), [255, 255, 255, 255]),
        "stash": stash_lines(info.stash),
        "stash_owned": info.stash_owned,
    })
    .to_string()
}

/// The shared-stash tabs as dashboard lines: a header per tab, then one line per item with
/// its quality tag. Uniques are gold and sets green (the overlay's drop colours); magic and
/// rare get their classic colours, everything else stays plain.
fn stash_lines(tabs: &[crate::stash::StashTab]) -> Vec<serde_json::Value> {
    use crate::offsets::item_data;
    let mut out = Vec::new();
    for tab in tabs {
        out.push(serde_json::json!({
            "text": format!("tab {} ({} items)", tab.index, tab.items.len()),
            "colour": "#afafaf",
        }));
        for item in &tab.items {
            let (tag, colour) = match u32::from(item.quality) {
                item_data::QUALITY_UNIQUE => ("unique", "#ffd278"),
                item_data::QUALITY_SET => ("set", "#5ae65a"),
                item_data::QUALITY_RARE => ("rare", "#ffff5f"),
                item_data::QUALITY_MAGIC => ("magic", "#8f88ff"),
                _ => ("", "#dddddd"),
            };
            let mut tag = tag.to_owned();
            // Base-name items already carry an "eth " prefix from the keep-list naming; only
            // the unique/set names need the tag.
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
            out.push(serde_json::json!({ "text": text, "colour": colour }));
        }
    }
    out
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
<h1>d2r-map-overlay config &nbsp; <a style="font-size:0.6em" href="/dashboard">live dashboard →</a></h1>
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

/// The live dashboard page (a single string: no assets, no framework). Polls `/api/live` once
/// a second and renders the dashboard's text blocks in their colours: where to go, gear and
/// `BiS`, sell and keep (the overlay itself draws only the breakpoint guide). Dark-themed like
/// the config page.
const fn dashboard_page() -> &'static str {
    r##"<!doctype html>
<html><head><meta charset="utf-8"><title>d2r-map-overlay live</title>
<style>
body { font-family: sans-serif; background: #1a1a1a; color: #ddd; margin: 2em; }
a { color: #7cf; }
h1 { font-size: 1.3em; }
h2 { font-size: 1.05em; border-bottom: 1px solid #444; padding-bottom: 4px; }
.panel { border: 1px solid #444; padding: 1em; margin-bottom: 1em; }
.cols { display: flex; gap: 1.5em; flex-wrap: wrap; }
.col { min-width: 12em; }
.col h3 { font-size: 0.85em; color: #888; margin: 0 0 4px; }
.line { white-space: pre-wrap; font-size: 0.95em; }
.waiting { color: #999; font-style: italic; }
.note { color: #888; font-size: 0.72em; margin-bottom: 4px; }
.hint { color: #888; font-size: 0.75em; margin-left: 1.5em; }
</style></head><body>
<h1>d2r-map-overlay live &nbsp; <a style="font-size:0.6em" href="/">config →</a></h1>
<h2 id="header"><span class="waiting">waiting for game…</span></h2>
<div class="panel"><h2>Where to go</h2><div id="levels"></div></div>
<div class="panel"><h2>Gear / BiS</h2><div id="gear"></div></div>
<div class="panel"><h2>Shared stash</h2><div class="note">tab numbers are memory order, not verified against the UI yet</div><div id="stash"></div></div>
<div class="panel"><h2>Sell / Keep</h2><div id="sell"></div></div>
<script>
// HTML-escape: line texts come from item and area names read out of the game. Numeric
// character references (&#38; …), so the replacement strings stay plain ASCII here.
function esc(s) {
  return s.replace(/[&<>"']/g, c => "&#" + c.charCodeAt(0) + ";");
}
function lines(el, entries) {
  el.innerHTML = (entries || []).map(e => {
    // A BiS line that is a runeword carries its runes and bases: a dim hint under the piece.
    const hint = e.runes
      ? `<div class="hint">${esc(e.runes.join(" + "))} · ${esc((e.bases || []).join(", "))}</div>`
      : "";
    return `<div class="line" style="color:${e.colour || "#ddd"}">${esc(e.text)}</div>` + hint;
  }).join("");
}
// The "Where to go" band in three columns: below / equal / above the character level. The
// server sends the level entries structured ("mlvl" + "bucket"), so the grouping is a filter,
// not a parse of the "Level N → Area (M)" text; each column keeps the band's own order.
function levelColumns(el, entries) {
  const groups = ["below", "equal", "above"].map(bucket => ({
    bucket,
    lines: (entries || []).filter(e => e.bucket === bucket),
  }));
  el.innerHTML = `<div class="cols">` + groups.map(g =>
    `<div class="col"><h3>${g.bucket}</h3>`
    + g.lines.map(e => `<div class="line" style="color:${e.colour || "#ddd"}">${esc(e.text)}</div>`).join("")
    + `</div>`
  ).join("") + `</div>`;
}
async function poll() {
  try {
    const r = await (await fetch("/api/live")).json();
    const header = document.getElementById("header");
    if (!r.live) {
      header.innerHTML = '<span class="waiting">waiting for game…</span>';
      for (const id of ["levels", "gear", "stash", "sell"]) document.getElementById(id).innerHTML = "";
    } else {
      const p = r.player;
      header.textContent = [p.name, p.class, p.level != null ? "level " + p.level : null,
        p.difficulty, r.area].filter(Boolean).join(" · ")
        + "  (updated " + new Date(r.updated_at * 1000).toLocaleTimeString() + ")";
      levelColumns(document.getElementById("levels"), r.levels);
      // The breakpoint guide belongs with the gear it describes. A BiS line whose tick is
      // "stash" (the ✓ comes from the shared stash) is marked ✓(stash); the tick comes
      // structured per line ("owned" | "stash" | null), not scraped from the line text.
      const bis = (r.bis || []).map(l => {
        let text = l.text;
        if (l.tick === "stash" && typeof l.text === "string" && l.text.includes("\u2713")) {
          text = l.text.replace("\u2713", "\u2713(stash)");
        }
        return { text, colour: l.colour, runes: l.runes, bases: l.bases };
      });
      lines(document.getElementById("gear"), (r.guide || []).concat(bis).concat(r.equipped || []));
      lines(document.getElementById("stash"), r.stash || []);
      lines(document.getElementById("sell"), (r.sell || []).concat(r.keep || []).concat(r.give || []));
    }
  } catch (e) { /* server gone or mid-restart: keep the last frame, try again */ }
  setTimeout(poll, 1000);
}
poll();
</script></body></html>
"##
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Result, anyhow};
    use std::io::Cursor;

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

    /// One tab with an identified unique and a plain ethereal base, for the stash panel.
    fn test_stash() -> Vec<crate::stash::StashTab> {
        vec![crate::stash::StashTab {
            index: 1,
            items: vec![
                crate::stash::StashItem {
                    name: "Harlequin Crest".to_owned(),
                    quality: u8::try_from(crate::offsets::item_data::QUALITY_UNIQUE).unwrap_or_default(),
                    identified: true,
                    ethereal: false,
                    runeword: false,
                },
                crate::stash::StashItem {
                    name: "eth Crystal Sword 4os".to_owned(),
                    quality: u8::try_from(crate::offsets::item_data::QUALITY_NORMAL).unwrap_or_default(),
                    identified: true,
                    ethereal: true,
                    runeword: false,
                },
            ],
        }]
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
            equipped,
            stash,
            stash_owned,
            updated_at: 1_700_000_000,
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
        // The stash panel: a tab header, then the items with their quality tags (uniques gold).
        // Lowercase `tab`: unit-table order, not verified against the UI's tab numbering.
        assert_eq!(
            colour_of("stash"),
            vec![
                ("tab 1 (2 items)".to_owned(), "#afafaf".to_owned()),
                ("Harlequin Crest (unique)".to_owned(), "#ffd278".to_owned()),
                ("eth Crystal Sword 4os".to_owned(), "#dddddd".to_owned()),
            ]
        );
        assert_eq!(
            json.get("stash_owned"),
            Some(&serde_json::json!(["Harlequin Crest"]))
        );
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
}
