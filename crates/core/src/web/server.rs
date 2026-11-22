//! The HTTP server: one thread per connection, the dashboard's SSE stream, the live and gear
//! cells `live` publishes into, and the `/config` page's writes.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::playstyle::{Files, Playstyle};

use super::page;

/// The port the page listens on (localhost only).
pub const PORT: u16 = 8471;

/// Larger `Content-Length`s are refused before allocating: the client controls that header.
const MAX_BODY: usize = 1 << 20;

const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Each SSE stream holds a thread, and a stuck tab's writes may never fail; above this, 503.
const MAX_SSE_CLIENTS: usize = 8;

const SSE_POLL: Duration = Duration::from_millis(250);

const SSE_KEEPALIVE: Duration = Duration::from_secs(15);

/// Frees a vanished client's SSE slot before TCP retransmission gives up.
const SSE_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

static SSE_CLIENTS: AtomicUsize = AtomicUsize::new(0);

/// Serializes the `/config` saves so two cannot start from the same old text.
static CONFIG_WRITE: Mutex<()> = Mutex::new(());

/// The latest live snapshot's JSON; `None` while no game was seen yet.
static LIVE: OnceLock<RwLock<Option<String>>> = OnceLock::new();

/// The live gear state the config page's `BiS` view is annotated from; keeps the last game's
/// values after it exits.
#[derive(Debug, Clone, Default)]
pub struct GearInfo {
    pub worn: Vec<String>,
    pub owned: Vec<String>,
    pub stash: Vec<String>,
    pub equipped: Vec<(u8, u32)>,
    pub merc_equipped: Vec<(u8, u32)>,
}

static GEAR: OnceLock<RwLock<GearInfo>> = OnceLock::new();

pub fn publish_gear(gear: GearInfo) {
    // A poisoned cell still holds a usable value, and the next tick overwrites it anyway.
    *gear_cell().write().unwrap_or_else(PoisonError::into_inner) = gear;
}

fn gear_cell() -> &'static RwLock<GearInfo> {
    GEAR.get_or_init(|| RwLock::new(GearInfo::default()))
}

fn current_gear() -> GearInfo {
    gear_cell().read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// The pages inline the same SVG, percent-encoded, as their icon.
const FAVICON_SVG: &str = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'><rect width='32' height='32' rx='7' fill='#1a1815'/><path d='M16 4 28 16 16 28 4 16Z' fill='#2b2721' stroke='#c8a85a' stroke-width='3' stroke-linejoin='round'/><circle cx='16' cy='16' r='4' fill='#ff6b3d'/></svg>";

const NO_LIVE_JSON: &str = "{\"live\": false}";

fn live_cell() -> &'static RwLock<Option<String>> {
    LIVE.get_or_init(|| RwLock::new(None))
}

fn current_live() -> Option<String> {
    live_cell().read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Stores only a changed value, so the SSE handlers' compare stays cheap.
pub fn publish_live(json: String) {
    let mut cell = live_cell().write().unwrap_or_else(PoisonError::into_inner);
    if cell.as_deref() != Some(json.as_str()) {
        *cell = Some(json);
    }
}

/// The game is gone or in the menu: the last snapshot stays, marked `"offline": true`.
pub fn clear_live() {
    let mut cell = live_cell().write().unwrap_or_else(PoisonError::into_inner);
    if let Some(json) = cell.as_deref().filter(|j| !j.contains(OFFLINE_MARK)) {
        *cell = mark_offline(json);
    }
}

/// As `serde_json` writes it (compact).
const OFFLINE_MARK: &str = "\"offline\":true";

/// `None` when `json` is not a JSON object.
fn mark_offline(json: &str) -> Option<String> {
    let mut value: serde_json::Value = serde_json::from_str(json).ok()?;
    value
        .as_object_mut()?
        .insert("offline".to_owned(), serde_json::Value::Bool(true));
    Some(value.to_string())
}

/// Serves the pages on `localhost:PORT` until the process ends; errors are logged, not fatal.
/// Each request loads the playstyle fresh, so a just-saved selection shows before hot reload.
pub fn serve<P: Playstyle + 'static>(files: &Files) {
    let Ok(listener) = TcpListener::bind(("127.0.0.1", PORT)) else {
        eprintln!("web: could not bind localhost:{PORT}; the page is off");
        return;
    };
    eprintln!("web: dashboard at http://localhost:{PORT} (config at /config)");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let files = files.clone();
                if let Err(e) = thread::Builder::new()
                    .name("web".into())
                    .spawn(move || handle::<P>(stream, &files))
                {
                    eprintln!("web: could not start a handler thread: {e}");
                }
            }
            Err(e) => eprintln!("web: {e}"),
        }
    }
}

enum Parsed {
    /// method, path, body.
    Request(String, String, Vec<u8>),
    TooLarge,
}

/// `None` on a malformed or unreadable request: the connection is dropped without an answer.
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

fn handle<P: Playstyle>(stream: TcpStream, files: &Files) {
    let peer = stream
        .peer_addr()
        .map_or_else(|_| String::new(), |a| a.to_string());
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
            ("GET", "/api/config") => api_config::<P>(files),
            ("GET", "/api/builds") => api_builds::<P>(files),
            ("GET", "/api/loot-filter") => loot_filter_response(P::loot_filter(&files.quiet())),
            ("GET", "/api/live") => api_live(),
            ("GET", "/api/live-stream") => {
                api_live_stream(stream);
                return;
            }
            ("POST", "/api/bis") => api_bis::<P>(files, &String::from_utf8_lossy(&body)),
            ("POST", "/api/character/delete") => {
                api_character_delete::<P>(files, &String::from_utf8_lossy(&body))
            }
            _ => respond("404 Not Found", "text/plain", "not found\n"),
        },
    };
    let mut stream = stream;
    if stream.write_all(response.as_bytes()).is_err() {
        eprintln!("web: {peer}: could not answer");
    }
}

fn respond(status: &str, content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// `/dashboard` is kept as an alias of `/` for old bookmarks.
fn html_page(path: &str) -> String {
    match path {
        "/" | "/index.html" | "/dashboard" => respond("200 OK", "text/html", page::DASHBOARD_PAGE),
        "/config" => config_page(),
        _ => respond("404 Not Found", "text/plain", "not found\n"),
    }
}

fn config_page() -> String {
    respond("200 OK", "text/html", page::CONFIG_PAGE)
}

fn api_live() -> String {
    respond_no_store(
        "200 OK",
        "application/json",
        current_live().as_deref().unwrap_or(NO_LIVE_JSON),
    )
}

/// Holds one of the [`MAX_SSE_CLIENTS`] slots; freed on drop, also on unwind.
struct SseSlot;

impl SseSlot {
    fn acquire() -> Option<Self> {
        let slot = Self;
        (SSE_CLIENTS.fetch_add(1, Ordering::Relaxed) < MAX_SSE_CLIENTS).then_some(slot)
    }
}

impl Drop for SseSlot {
    fn drop(&mut self) {
        SSE_CLIENTS.fetch_sub(1, Ordering::Relaxed);
    }
}

/// `GET /api/live-stream`: writes `data: <json>\n\n` whenever the live JSON changes and a
/// keepalive comment when idle, until the first write error (`EventSource` reconnects).
fn api_live_stream(mut stream: TcpStream) {
    let Some(_slot) = SseSlot::acquire() else {
        let _ = stream.write_all(
            b"HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\nContent-Length: 9\r\nConnection: close\r\n\r\nno slots\n",
        );
        return;
    };
    if open_event_stream(&mut stream).is_err() {
        return;
    }
    let mut last: Option<String> = None;
    let mut keepalive = Instant::now();
    loop {
        thread::sleep(SSE_POLL);
        let current = current_live();
        if current != last {
            let body = current.as_deref().unwrap_or(NO_LIVE_JSON);
            if write_chunk(&mut stream, &format!("data: {body}\n\n")).is_err() {
                return;
            }
            last = current;
            keepalive = Instant::now();
        } else if keepalive.elapsed() >= SSE_KEEPALIVE {
            if write_chunk(&mut stream, ": keepalive\n\n").is_err() {
                return;
            }
            keepalive = Instant::now();
        }
    }
}

/// Sets the socket up for unbuffered streaming and writes the SSE response headers.
fn open_event_stream(stream: &mut TcpStream) -> std::io::Result<()> {
    if let Err(e) = stream.set_nodelay(true) {
        eprintln!("live-stream: no nodelay: {e}");
    }
    if let Err(e) = stream.set_write_timeout(Some(SSE_WRITE_TIMEOUT)) {
        eprintln!("live-stream: no write timeout: {e}");
    }
    stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
    )
}

fn write_chunk(stream: &mut TcpStream, chunk: &str) -> std::io::Result<()> {
    stream.write_all(chunk.as_bytes())?;
    stream.flush()
}

fn respond_no_store(status: &str, content_type: &str, body: &str) -> String {
    let mut response = respond(status, content_type, body);
    let end_of_headers = response.find("\r\n\r\n").unwrap_or(0);
    response.insert_str(end_of_headers, "\r\nCache-Control: no-store");
    response
}

fn api_config<P: Playstyle>(files: &Files) -> String {
    let playstyle = P::load(&files.quiet());
    let gear = current_gear();
    let played =
        crate::played::default_path().map_or_else(crate::played::Played::new, |p| crate::played::load(&p));
    respond(
        "200 OK",
        "application/json",
        &playstyle.config_json(&gear, &played).to_string(),
    )
}

fn api_builds<P: Playstyle>(files: &Files) -> String {
    let playstyle = P::load(&files.quiet());
    respond("200 OK", "application/json", &playstyle.builds_json().to_string())
}

/// Served byte for byte, not re-serialised: the game imports exactly this text.
fn loot_filter_response(filter: Option<Vec<u8>>) -> String {
    match filter.map(String::from_utf8) {
        Some(Ok(json)) => respond_no_store("200 OK", "application/json", &json),
        Some(Err(_)) => respond(
            "500 Internal Server Error",
            "text/plain",
            "the loot filter file is not UTF-8\n",
        ),
        None => respond(
            "404 Not Found",
            "text/plain",
            "no loot filter in this playstyle\n",
        ),
    }
}

/// Runs one of the playstyle's saves under [`CONFIG_WRITE`].
fn save(write: impl FnOnce() -> anyhow::Result<()>) -> String {
    let _guard = CONFIG_WRITE.lock().unwrap_or_else(PoisonError::into_inner);
    match write() {
        Ok(()) => respond("200 OK", "application/json", "{\"saved\": true}"),
        Err(e) => respond("500 Internal Server Error", "text/plain", &format!("{e:#}\n")),
    }
}

/// Writes `<path>.tmp`, then renames it over `path`, so the hot-reload reader never sees a
/// truncated file.
///
/// # Errors
/// Writing the temporary file or the rename failed.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// `POST /api/bis` with `{"name": …, "build": "<slug>" | null}`: saves the selection, then
/// forgets the reached stage, since an explicit selection restarts the stage latch.
fn api_bis<P: Playstyle>(files: &Files, body: &str) -> String {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return respond("400 Bad Request", "text/plain", "bad json\n");
    };
    let Some(name) = json.get("name").and_then(|v| v.as_str()) else {
        return respond("400 Bad Request", "text/plain", "name required\n");
    };
    let Ok(build) = selected_build(json.get("build")) else {
        return respond(
            "400 Bad Request",
            "text/plain",
            "build must be a string or null\n",
        );
    };
    let response = save(|| P::select_build(files, name, build));
    if !response.starts_with("HTTP/1.1 200") {
        return response;
    }
    if let Some(path) = crate::played::default_path()
        && let Err(e) = crate::played::clear_stage(&path, name)
    {
        eprintln!("web: selection saved, but the reached stage of {name} wasn't reset: {e:#}");
    }
    response
}

/// The `build` field: a slug, or `None` (null, absent or blank) to clear the selection.
/// Any other JSON type is an error, not a silent clear.
fn selected_build(build: Option<&serde_json::Value>) -> Result<Option<&str>, ()> {
    match build {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s.trim()).filter(|slug| !slug.is_empty())),
        Some(_) => Err(()),
    }
}

/// `POST /api/character/delete` with `{"name": …}`: forgets the character's last-played entry
/// and the playstyle's entries for it. D2R's own save files are never touched.
fn api_character_delete<P: Playstyle>(files: &Files, body: &str) -> String {
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
    // The playstyle first, so a failed edit leaves played.json untouched.
    let response = save(|| P::remove_character(files, name));
    if !response.starts_with("HTTP/1.1 200") {
        return response;
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

    /// One SSE chunk; the first call returns the header block.
    fn read_chunk(client: &mut std::net::TcpStream) -> std::io::Result<String> {
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            let read = client.read(&mut byte)?;
            if read == 0 {
                return Err(std::io::Error::other("connection closed"));
            }
            buf.push(byte[0]);
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
        assert!(read_chunk(&mut client).is_err(), "no frame before a publish");
        publish_live("{\"live\": true, \"n\": 1}".to_owned());
        assert_eq!(read_chunk(&mut client)?, "data: {\"live\": true, \"n\": 1}\n\n");
        publish_live("{\"live\": true, \"n\": 1}".to_owned());
        assert!(read_chunk(&mut client).is_err(), "no duplicate frame");
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

    /// The core names no concrete playstyle; the routes under test need none.
    struct NoPlaystyle;

    impl Playstyle for NoPlaystyle {
        type Wants = ();
        type Extra<'a> = ();

        fn load(_: &Files) -> Self {
            Self
        }

        fn reload(&mut self, _: &Files) {}

        fn mark_drops(&self, _: &mut Vec<crate::game::ItemDrop>, (): &()) -> crate::guidance::Marks {
            crate::guidance::Marks::new()
        }

        fn evaluate(
            &self,
            _: &crate::game::Snapshot,
            _: Option<&str>,
            _: Option<&crate::quest_log::QuestsDone>,
        ) -> crate::playstyle::EvaluationOf<'_, Self> {
            crate::playstyle::Evaluation {
                guidance: crate::guidance::Guidance::default(),
                wants: (),
                reached: None,
                extra: (),
            }
        }

        fn panels<'a>(
            &'a self,
            _: &'a crate::playstyle::EvaluationOf<'_, Self>,
            _: &crate::web::LiveInfo<'a>,
        ) -> crate::web::Panels<'a> {
            crate::web::Panels::default()
        }

        fn config_json(&self, _: &GearInfo, _: &crate::played::Played) -> serde_json::Value {
            serde_json::Value::Null
        }

        fn builds_json(&self) -> serde_json::Value {
            serde_json::Value::Null
        }

        fn select_build(_: &Files, _: &str, _: Option<&str>) -> Result<()> {
            Ok(())
        }

        fn loot_filter(files: &Files) -> Option<Vec<u8>> {
            std::fs::read(files.shipped.as_ref()?.join("loot-filter.json")).ok()
        }

        fn remove_character(_: &Files, _: &str) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn favicon_is_served_and_linked_from_both_pages() -> Result<()> {
        let page_link = |page: &str| {
            page.contains("<link rel=\"icon\" type=\"image/svg+xml\" href=\"data:image/svg+xml,%3Csvg")
        };
        assert!(page_link(page::DASHBOARD_PAGE), "dashboard links the icon");
        assert!(page_link(page::CONFIG_PAGE), "config page links the icon");
        let encoded = FAVICON_SVG
            .replace('#', "%23")
            .replace('<', "%3C")
            .replace('>', "%3E")
            .replace(' ', "%20");
        assert!(
            page::DASHBOARD_PAGE.contains(&encoded),
            "same SVG inline and served"
        );
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        let server = thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                handle::<NoPlaystyle>(stream, &Files::default());
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

    fn get(path: &str, files: Files) -> Result<Vec<u8>> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        let server = thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                handle::<NoPlaystyle>(stream, &files);
            }
        });
        let mut client = std::net::TcpStream::connect(addr)?;
        client.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())?;
        let mut answer = Vec::new();
        client.read_to_end(&mut answer)?;
        server.join().map_err(|_| anyhow!("server thread panicked"))?;
        Ok(answer)
    }

    #[test]
    fn the_loot_filter_is_served_byte_for_byte() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("d2r-loot-filter-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        // Odd spacing and non-ASCII, so a re-serialisation would show.
        let file = "{\n  \"name\": \"Dru\u{ef}d \u{2014} v1\",\n\t\"rules\": [ ]\n}\n";
        std::fs::write(dir.join("loot-filter.json"), file)?;
        let files = Files {
            shipped: Some(dir.clone()),
            ..Files::default()
        };
        let answer = get("/api/loot-filter", files)?;
        let text = String::from_utf8(answer.clone())?;
        assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
        assert!(text.contains("Content-Type: application/json"), "{text}");
        assert!(text.contains("Cache-Control: no-store"), "{text}");
        assert!(answer.ends_with(file.as_bytes()), "{text}");
        assert!(
            text.contains(&format!("Content-Length: {}\r\n", file.len())),
            "{text}"
        );
        std::fs::remove_dir_all(&dir)?;
        let none = String::from_utf8(get("/api/loot-filter", Files::default())?)?;
        assert!(none.starts_with("HTTP/1.1 404"), "{none}");
        assert!(none.ends_with("no loot filter in this playstyle\n"), "{none}");
        assert!(loot_filter_response(Some(vec![0xff, b'{'])).starts_with("HTTP/1.1 500"));
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

    /// FNV-1a 64: unlike `DefaultHasher`, stable across Rust versions.
    fn fnv1a(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        })
    }

    /// The served pages byte for byte, headers included; a deliberate page change updates them.
    #[test]
    fn served_pages_match_their_snapshot() {
        let dashboard = html_page("/");
        let config = html_page("/config");
        assert_eq!(
            (dashboard.len(), format!("{:#018x}", fnv1a(dashboard.as_bytes()))),
            (49_228, "0xcb886646ccb696ef".to_owned()),
            "dashboard page"
        );
        assert_eq!(
            (config.len(), format!("{:#018x}", fnv1a(config.as_bytes()))),
            (10_613, "0xd31264b357429ae1".to_owned()),
            "config page"
        );
    }
}
