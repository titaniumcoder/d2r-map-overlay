//! The HTTP server: one thread per connection, the dashboard's Server-Sent-Events stream, the
//! live and gear cells `live` publishes into, and the `config.toml` writes of the `/config`
//! page.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::playstyle::Playstyle;

use super::page;

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
    /// Names of the worn pieces ([`crate::game::worn_names`]): their ✓ is green.
    pub worn: Vec<String>,
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

/// The pages' icon (served at `/favicon.ico`; the pages inline the same SVG, percent-encoded,
/// as a data URI): a gold isometric diamond — the overlay's level map — on the dashboard's
/// dark panel colour, with the player as a red-orange dot. Simple shapes, so it reads at 16 px.
const FAVICON_SVG: &str = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'><rect width='32' height='32' rx='7' fill='#1a1815'/><path d='M16 4 28 16 16 28 4 16Z' fill='#2b2721' stroke='#c8a85a' stroke-width='3' stroke-linejoin='round'/><circle cx='16' cy='16' r='4' fill='#ff6b3d'/></svg>";

/// The answer of `GET /api/live` while no game is attached (or between games).
const NO_LIVE_JSON: &str = "{\"live\": false}";

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

/// The game is gone or in the menu: the last snapshot stays on the dashboard, marked
/// `"offline": true`, until the next game publishes a fresh one. Nothing published yet stays
/// "waiting for game".
pub fn clear_live() {
    // See [`publish_live`] for the poison handling.
    let mut cell = live_cell().write().unwrap_or_else(PoisonError::into_inner);
    // Already marked: the menu calls this every tick, don't re-serialise the snapshot.
    if let Some(json) = cell.as_deref().filter(|j| !j.contains(OFFLINE_MARK)) {
        *cell = mark_offline(json);
    }
}

/// What [`mark_offline`] adds, as `serde_json` writes it (compact, no spaces).
const OFFLINE_MARK: &str = "\"offline\":true";

/// `json` with `"offline": true` added; `None` when it is not a JSON object (nothing worth
/// keeping).
fn mark_offline(json: &str) -> Option<String> {
    let mut value: serde_json::Value = serde_json::from_str(json).ok()?;
    value
        .as_object_mut()?
        .insert("offline".to_owned(), serde_json::Value::Bool(true));
    Some(value.to_string())
}

/// Serves the page on `localhost:PORT` until the process ends. Errors are logged, not fatal:
/// the map keeps working without the page. `P` is the build's playstyle: the `/config` JSON
/// and the build selection writes are its (each request loads it fresh from the config files,
/// [`fresh_playstyle`]).
pub fn serve<P: Playstyle + Default + 'static>() {
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
                    .spawn(move || handle::<P>(stream))
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
fn handle<P: Playstyle + Default>(stream: TcpStream) {
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
            ("GET", "/favicon.ico") => respond("200 OK", "image/svg+xml", FAVICON_SVG),
            ("GET", "/api/config") => api_config::<P>(),
            ("GET", "/api/builds") => api_builds::<P>(),
            ("GET", "/api/live") => api_live(),
            // The stream owns the connection from here on; nothing is written after it.
            ("GET", "/api/live-stream") => {
                api_live_stream(stream);
                return;
            }
            ("POST", "/api/bis") => api_bis::<P>(&String::from_utf8_lossy(&body)),
            ("POST", "/api/character/delete") => api_character_delete::<P>(&String::from_utf8_lossy(&body)),
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
        "/" | "/index.html" | "/dashboard" => respond("200 OK", "text/html", page::DASHBOARD_PAGE),
        "/config" => config_page(),
        _ => respond("404 Not Found", "text/plain", "not found\n"),
    }
}

/// The form page (see [`page::CONFIG_PAGE`] for the embedded source).
fn config_page() -> String {
    respond("200 OK", "text/html", page::CONFIG_PAGE)
}

/// `200 OK` with the latest live snapshot's JSON verbatim (published by `live`), or
/// [`NO_LIVE_JSON`] before the first game was seen (after that the last snapshot stays,
/// marked offline). The page handles both.
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
/// writes, no chunked encoding (the message boundary is the SSE `\n\n`); when the game detaches
/// the last frame is sent once more, marked offline.
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
        // marks the last snapshot offline — goes out as one event.
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
/// characters only known from `[builds]`, their saved damage types
/// ([`Playstyle::config_json`]).
fn api_config<P: Playstyle + Default>() -> String {
    let playstyle = fresh_playstyle::<P>();
    let gear = current_gear();
    let played =
        crate::played::default_path().map_or_else(crate::played::Played::new, |p| crate::played::load(&p));
    respond(
        "200 OK",
        "application/json",
        &playstyle.config_json(&gear, &played).to_string(),
    )
}

/// `200 OK` with the available build definitions as JSON for the page's selects
/// ([`Playstyle::builds_json`]).
fn api_builds<P: Playstyle + Default>() -> String {
    let playstyle = fresh_playstyle::<P>();
    respond("200 OK", "application/json", &playstyle.builds_json().to_string())
}

/// The playstyle as the config files stand right now: the page must show a selection just
/// saved, before `live`'s hot reload picks it up.
fn fresh_playstyle<P: Playstyle + Default>() -> P {
    let mut playstyle = P::default();
    playstyle.reload(&crate::config::load_from(
        &crate::config::default_path().unwrap_or_default(),
    ));
    playstyle
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
/// gear itself lives in the agent-authored `builds/*.toml`; the webapp doesn't edit it). Then
/// forgets the stage the character reached ([`crate::played::clear_stage`]): an explicit
/// selection wins over the stage latch, which restarts from it.
fn api_bis<P: Playstyle>(body: &str) -> String {
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
    let response = save_config(|text| P::select_build(text, name, build));
    if !response.starts_with("HTTP/1.1 200") {
        return response;
    }
    // The selection is saved either way; a failed reset only leaves the old latch in place.
    if let Some(path) = crate::played::default_path()
        && let Err(e) = crate::played::clear_stage(&path, name)
    {
        eprintln!("web: selection saved, but the reached stage of {name} wasn't reset: {e:#}");
    }
    response
}

/// `POST /api/character/delete` with `{"name": …}`: forgets the character — its last-played
/// entry ([`crate::played::remove`]) and, in `config.toml`, every `[bis.<name>]` table and its
/// `[builds]` damage entry ([`Playstyle::remove_character`]), all case-insensitively. D2R's own save
/// files are never touched; playing the character again brings it back.
fn api_character_delete<P: Playstyle>(body: &str) -> String {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return respond("400 Bad Request", "text/plain", "bad json\n");
    };
    let Some(name) = json
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|n| !n.is_empty())
    else {
        return respond("400 Bad Request", "text/plain", "name required\n");
    };
    // Config first: a failed config edit leaves played.json untouched, so a 500 deletes nothing.
    // A played-only character needs no config edit; without a config file there is none.
    if crate::config::default_path().is_some_and(|p| p.exists()) {
        let response = save_config(|text| P::remove_character(text, name));
        if !response.starts_with("HTTP/1.1 200") {
            return response;
        }
    }
    if let Some(path) = crate::played::default_path()
        && let Err(e) = crate::played::remove(&path, name)
    {
        return respond("500 Internal Server Error", "text/plain", &format!("{e:#}\n"));
    }
    respond("200 OK", "application/json", "{\"saved\": true}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Result, anyhow};
    use std::io::{Cursor, Read};

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
        // A changed frame goes out; a detached game marks the last one offline.
        publish_live("{\"live\": true, \"n\": 2}".to_owned());
        assert_eq!(read_chunk(&mut client)?, "data: {\"live\": true, \"n\": 2}\n\n");
        clear_live();
        assert_eq!(
            read_chunk(&mut client)?,
            "data: {\"live\":true,\"n\":2,\"offline\":true}\n\n"
        );
        Ok(())
    }

    #[test]
    fn mark_offline_adds_the_flag_to_objects_only() {
        assert_eq!(
            mark_offline("{\"live\": true}").as_deref(),
            Some("{\"live\":true,\"offline\":true}")
        );
        assert!(mark_offline("{\"live\":true,\"offline\":true}").is_some_and(|j| j.contains(OFFLINE_MARK)));
        assert_eq!(mark_offline("[1]"), None);
        assert_eq!(mark_offline("not json"), None);
    }

    #[test]
    fn favicon_is_served_and_linked_from_both_pages() -> Result<()> {
        let page_link = |page: &str| {
            page.contains("<link rel=\"icon\" type=\"image/svg+xml\" href=\"data:image/svg+xml,%3Csvg")
        };
        assert!(page_link(page::DASHBOARD_PAGE), "dashboard links the icon");
        assert!(page_link(page::CONFIG_PAGE), "config page links the icon");
        // The inline copy is the served SVG, percent-encoded.
        let encoded = FAVICON_SVG
            .replace('#', "%23")
            .replace('<', "%3C")
            .replace('>', "%3E")
            .replace(' ', "%20");
        assert!(
            page::DASHBOARD_PAGE.contains(&encoded),
            "same SVG inline and served"
        );
        // The route: a real request through `handle`.
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        let server = thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                handle::<crate::playstyle::ConfigDriven>(stream);
            }
        });
        let mut client = std::net::TcpStream::connect(addr)?;
        client.write_all(b"GET /favicon.ico HTTP/1.1\r\nHost: x\r\n\r\n")?;
        let mut answer = String::new();
        client.read_to_string(&mut answer)?;
        server.join().map_err(|_| anyhow!("server thread panicked"))?;
        assert!(answer.starts_with("HTTP/1.1 200 OK"), "{answer}");
        assert!(answer.contains("Content-Type: image/svg+xml"), "{answer}");
        assert!(
            answer.ends_with(FAVICON_SVG) && FAVICON_SVG.starts_with("<svg"),
            "{answer}"
        );
        Ok(())
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

    /// FNV-1a, 64 bit: a hash that stays the same across Rust versions (unlike
    /// `DefaultHasher`).
    fn fnv1a(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        })
    }

    /// The served pages, headers included, byte for byte: the snapshot taken from the
    /// single-file pages before they moved to `assets/` (refactor step 6). A deliberate page
    /// change updates these numbers (the failure message prints the new ones).
    #[test]
    fn served_pages_match_their_snapshot() {
        let dashboard = html_page("/");
        let config = html_page("/config");
        assert_eq!(
            (dashboard.len(), format!("{:#018x}", fnv1a(dashboard.as_bytes()))),
            (41_668, "0xb5460938b1662255".to_owned()),
            "dashboard page"
        );
        assert_eq!(
            (config.len(), format!("{:#018x}", fnv1a(config.as_bytes()))),
            (8_385, "0xd40f46a49bba9c05".to_owned()),
            "config page"
        );
    }
}
