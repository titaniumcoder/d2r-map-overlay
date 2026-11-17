//! The companion web page: a small HTTP server on `localhost` that lets the user edit the
//! config (per-character builds and `BiS` gear lists) in a browser form, writing back to
//! `config.toml` with comments preserved (user request, 2026-11-17). The overlay watches the
//! config file and hot-reloads it (see `live`).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
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

/// Edits the `[builds]` entry of `name` in `text` to `damages` (comment-preserving).
/// `None` when `text` is not valid TOML: the caller answers the browser with an error.
#[must_use]
pub fn set_build(text: &str, name: &str, damages: &[&str]) -> Option<String> {
    let mut doc = text.parse::<DocumentMut>().ok()?;
    let values = damages
        .iter()
        .map(|d| toml_edit::Value::from(d.to_owned()))
        .collect::<Vec<_>>();
    if let Some(builds) = doc.get_mut("builds").and_then(|b| b.as_table_mut()) {
        // In-place assignment (not `insert`): the value's leading comment is kept.
        builds[name] = toml_edit::Item::Value(toml_edit::Value::Array(toml_edit::Array::from_iter(values)));
    } else {
        doc.insert("builds", {
            let mut t = toml_edit::Table::new();
            t.insert(
                name,
                toml_edit::Item::Value(toml_edit::Value::Array(toml_edit::Array::from_iter(values))),
            );
            toml_edit::Item::Table(t)
        });
    }
    Some(doc.to_string())
}

/// Edits the `[bis.<name>]` entry in `text` to `build` and the per-slot pieces
/// (comment-preserving). `None` when `text` is not valid TOML or when `bis` (or the entry
/// itself) is not a table: in that case the save would silently do nothing, so the caller
/// must answer the browser with an error instead.
#[must_use]
pub fn set_bis(text: &str, name: &str, build: &str, slots: &[(&str, Vec<String>)]) -> Option<String> {
    let mut doc = text.parse::<DocumentMut>().ok()?;
    if doc.get("bis").is_none() {
        doc.insert("bis", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let bis = doc.get_mut("bis").and_then(|b| b.as_table_mut())?;
    if !bis.contains_key(name) {
        bis.insert(name, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let entry = bis.get_mut(name).and_then(|e| e.as_table_mut())?;
    entry["build"] = toml_edit::value(build);
    // Replace the slots sub-table wholesale: the pieces change as a set.
    entry.remove("slots");
    let mut t = toml_edit::Table::new();
    for (slot, pieces) in slots {
        let values = pieces
            .iter()
            .map(|p| toml_edit::Value::from(p.clone()))
            .collect::<Vec<_>>();
        t.insert(
            slot,
            toml_edit::Item::Value(toml_edit::Value::Array(toml_edit::Array::from_iter(values))),
        );
    }
    entry.insert("slots", toml_edit::Item::Table(t));
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
            ("GET", "/api/config") => api_config(),
            ("POST", "/api/build") => api_build(&String::from_utf8_lossy(&body)),
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

/// `200 OK` with the current config as JSON for the form: the characters (with their `BiS`
/// slots and, per slot, the runeword hints for its pieces) and the runeword table the
/// form's datalist offers.
fn api_config() -> String {
    let config = crate::config::load_from(&crate::config::default_path().unwrap_or_default());
    let characters: Vec<_> = config
        .bis
        .iter()
        .map(|(name, bis)| {
            let hints: BTreeMap<&str, Vec<String>> = bis
                .slots
                .iter()
                .map(|(slot, pieces)| {
                    (
                        slot.as_str(),
                        pieces.iter().filter_map(|p| runeword_hint(p)).collect(),
                    )
                })
                .collect();
            serde_json::json!({
                "name": name,
                "build": bis.build,
                "slots": bis.slots,
                "hints": hints,
            })
        })
        .chain(
            config
                .builds
                .names
                .keys()
                .filter(|n| !config.bis.contains_key(n.as_str()))
                .map(|name| serde_json::json!({ "name": name, "build": "", "slots": {} })),
        )
        .collect();
    respond(
        "200 OK",
        "application/json",
        &serde_json::json!({ "characters": characters, "runewords": runewords_json() }).to_string(),
    )
}

/// The runeword name part of a `BiS` piece string (`"Insight@27"` → `"Insight"`).
fn piece_name(piece: &str) -> &str {
    piece.split_once('@').map_or(piece, |(name, _)| name).trim()
}

/// The runeword hint for a `BiS` piece string (`"Insight@27"` → `"Insight: Ral+Tir+Tal+Sol ·
/// polearms, staves, missile weapons"`), or `None` when it is not a runeword name.
#[must_use]
fn runeword_hint(piece: &str) -> Option<String> {
    let rw = crate::runewords_data::lookup(piece_name(piece))?;
    Some(format!(
        "{}: {} · {}",
        rw.name,
        rw.runes.join("+"),
        rw.bases.join(", ")
    ))
}

/// The runeword table as a JSON object keyed by lowercase name (`{"steel": {"name":
/// "Steel", …}, …}`), for the form's runeword datalist.
fn runewords_json() -> serde_json::Value {
    crate::runewords_data::RUNEWORDS
        .iter()
        .map(|rw| {
            (
                rw.name.to_lowercase(),
                serde_json::json!({ "name": rw.name, "runes": rw.runes, "bases": rw.bases }),
            )
        })
        .collect::<serde_json::Map<String, serde_json::Value>>()
        .into()
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

/// `POST /api/build` with `{"name": …, "damages": [...]}`: applies to `config.toml`.
fn api_build(body: &str) -> String {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return respond("400 Bad Request", "text/plain", "bad json\n");
    };
    let damages = json
        .get("damages")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>());
    let (Some(name), Some(damages)) = (json.get("name").and_then(|v| v.as_str()), damages) else {
        return respond("400 Bad Request", "text/plain", "name and damages required\n");
    };
    save_config(|text| set_build(text, name, &damages))
}

/// `POST /api/bis` with `{"name": …, "build": …, "slots": {slot: [...]}}`.
fn api_bis(body: &str) -> String {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return respond("400 Bad Request", "text/plain", "bad json\n");
    };
    let (Some(name), Some(build)) = (
        json.get("name").and_then(|v| v.as_str()),
        json.get("build").and_then(|v| v.as_str()),
    ) else {
        return respond("400 Bad Request", "text/plain", "name and build required\n");
    };
    let Some(slots) = json.get("slots").and_then(|v| v.as_object()) else {
        return respond("400 Bad Request", "text/plain", "slots required\n");
    };
    let slot_list: Vec<(String, Vec<String>)> = slots
        .iter()
        .map(|(slot, v)| {
            (
                slot.clone(),
                v.as_array()
                    .map_or_default(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()),
            )
        })
        .collect();
    let slot_refs: Vec<(&str, Vec<String>)> =
        slot_list.iter().map(|(s, p)| (s.as_str(), p.clone())).collect();
    save_config(|text| set_bis(text, name, build, &slot_refs))
}

/// The form page's HTML (a single string: no assets, no framework).
const fn config_page() -> &'static str {
    r#"<!doctype html>
<html><head><meta charset="utf-8"><title>d2r-map-overlay config</title>
<style>
body { font-family: sans-serif; background: #1a1a1a; color: #ddd; margin: 2em; }
input { background: #2a2a2a; color: #eee; border: 1px solid #555; padding: 4px; }
button { padding: 4px 10px; }
.char { border: 1px solid #444; padding: 1em; margin-bottom: 1em; }
.slot { margin: 4px 0; }
.hint { color: #9c9; font-size: 0.85em; margin-left: 5.5em; }
</style></head><body>
<h1>d2r-map-overlay config</h1>
<div id="chars"></div>
<datalist id="runewords"></datalist>
<h2>Add character</h2>
<input id="newname" placeholder="character name"> <button onclick="addChar()">Add</button>
<script>
const SLOTS = ["amulet","ring1","ring2","body","helm","gloves","boots","belt","weapon","offhand","swap"];
const DAMAGES = ["physical","fire","cold","lightning","poison","magic"];
async function load() {
  const cfg = await (await fetch("/api/config")).json();
  const chars = cfg.characters;
  const runewords = cfg.runewords || {};
  document.getElementById("runewords").innerHTML =
    Object.values(runewords).map(rw => `<option value="${rw.name}">`).join("");
  const root = document.getElementById("chars");
  root.innerHTML = "";
  for (const c of chars) {
    const div = document.createElement("div");
    div.className = "char";
    let html = `<h2>${c.name}</h2><h3>build damages</h3>`;
    for (const d of DAMAGES) {
      html += `<label style="margin-right:1em"><input type="checkbox" class="dmg-${c.name}" value="${d}"> ${d}</label>`;
    }
    html += `<button onclick="saveBuild('${c.name}')">Save build</button><h3>BiS slots</h3>`;
    html += `<div>build: <input id="build-${c.name}" value="${c.build}" size="30" placeholder="build name"></div>`;
    for (const s of SLOTS) {
      const pieces = (c.slots[s] || []).join(", ");
      const hints = (c.hints?.[s] || []).join("<br>");
      html += `<div class="slot">${s}: <input id="bis-${c.name}-${s}" value="${pieces}" size="50" placeholder="Stealth@17, Enigma@65" list="runewords">`
        + (hints ? `<div class="hint">${hints}</div>` : "") + `</div>`;
    }
    html += `<button onclick="saveBis('${c.name}')">Save BiS</button>`;
    div.innerHTML = html;
    root.appendChild(div);
  }
}
function addChar() {
  const name = document.getElementById("newname").value.trim();
  if (name) { fetch("/api/build", {method: "POST", body: JSON.stringify({name, damages: []})}).then(load); }
}
async function saveBuild(name) {
  const damages = [...document.querySelectorAll(`.dmg-${name}:checked`)].map(x => x.value);
  await fetch("/api/build", {method: "POST", body: JSON.stringify({name, damages})});
  load();
}
async function saveBis(name) {
  const build = document.getElementById(`build-${name}`).value;
  const slots = {};
  for (const s of SLOTS) {
    const v = document.getElementById(`bis-${name}-${s}`).value;
    slots[s] = v.split(",").map(x => x.trim()).filter(Boolean);
  }
  await fetch("/api/bis", {method: "POST", body: JSON.stringify({name, build, slots})});
  load();
}
load();
</script></body></html>
"#
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Result, anyhow};
    use std::io::Cursor;

    use crate::build::Damage;
    use crate::config;

    #[test]
    fn set_build_preserves_comments_and_parses_back() -> Result<()> {
        let text = "# top\n[builds]\n# inner\nSorc = [\"cold\"]\n";
        let out = set_build(text, "Sorc", &["fire", "lightning"]).ok_or_else(|| anyhow!("no edit"))?;
        assert!(out.contains("# top") && out.contains("# inner"), "{out}");
        let (config, _) = config::parse(&out)?;
        assert_eq!(
            config.builds.for_player("sorc", 1),
            [Damage::Fire, Damage::Lightning]
        );
        Ok(())
    }

    #[test]
    fn set_build_creates_a_missing_builds_table() -> Result<()> {
        let text = "# top\n";
        let out = set_build(text, "Sorc", &["cold"]).ok_or_else(|| anyhow!("no edit"))?;
        assert!(out.contains("# top"), "{out}");
        let (config, _) = config::parse(&out)?;
        assert_eq!(config.builds.for_player("Sorc", 1), [Damage::Cold]);
        Ok(())
    }

    #[test]
    fn set_bis_preserves_comments_and_parses_back() -> Result<()> {
        let text = "# top\n[bis.Hork]\n# build line\nbuild = \"Blood Boil\"\n\
                   [bis.Hork.slots]\nweapon = [\"Insight@27\"]\n";
        let out = set_bis(
            text,
            "Hork",
            "Blood Boil",
            &[
                ("weapon", vec!["Insight@27".to_owned()]),
                ("body", vec!["Stealth@17".to_owned()]),
            ],
        )
        .ok_or_else(|| anyhow!("no edit"))?;
        assert!(out.contains("# top") && out.contains("# build line"), "{out}");
        let (config, _) = config::parse(&out)?;
        let bis = config
            .bis_for_player("HORK")
            .ok_or_else(|| anyhow!("no bis entry"))?;
        assert_eq!(bis.build, "Blood Boil");
        assert_eq!(
            bis.slots.get("weapon").map(Vec::as_slice),
            Some(&["Insight@27".to_owned()][..])
        );
        assert_eq!(
            bis.slots.get("body").map(Vec::as_slice),
            Some(&["Stealth@17".to_owned()][..])
        );
        Ok(())
    }

    #[test]
    fn set_bis_creates_a_missing_bis_table() -> Result<()> {
        let text = "";
        let out = set_bis(
            text,
            "Hork",
            "Blood Boil",
            &[("weapon", vec!["Insight@27".to_owned()])],
        )
        .ok_or_else(|| anyhow!("no edit"))?;
        let (config, _) = config::parse(&out)?;
        let bis = config
            .bis_for_player("hork")
            .ok_or_else(|| anyhow!("no bis entry"))?;
        assert_eq!(bis.build, "Blood Boil");
        Ok(())
    }

    #[test]
    fn set_bis_rejects_non_table_bis() {
        // A bare value, and a non-table entry: the save would silently do nothing, so the
        // caller must get `None` and answer the browser with an error instead.
        assert_eq!(
            set_bis("bis = \"oops\"\n", "Hork", "b", &[("weapon", vec![])]),
            None
        );
        assert_eq!(
            set_bis("[bis]\nHork = 5\n", "Hork", "b", &[("weapon", vec![])]),
            None
        );
    }

    #[test]
    fn reads_requests() -> Result<()> {
        let request = b"POST /api/build\r\nContent-Length: 5\r\n\r\nhello";
        let Some(Parsed::Request(method, path, body)) = read_request(&mut Cursor::new(&request[..])) else {
            return Err(anyhow!("request not parsed"));
        };
        assert_eq!((method.as_str(), path.as_str()), ("POST", "/api/build"));
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
    fn piece_names_strip_the_level_suffix() {
        assert_eq!(piece_name("Insight@27"), "Insight");
        assert_eq!(piece_name("Stealth"), "Stealth");
        assert_eq!(piece_name(" White@35 "), "White");
    }

    #[test]
    fn runeword_hints_name_runes_and_bases() -> Result<()> {
        let hint = runeword_hint("Insight@27").ok_or_else(|| anyhow!("Insight is a runeword"))?;
        assert!(hint.starts_with("Insight: Ral+Tir+Tal+Sol · "), "{hint}");
        assert!(hint.contains("staves"), "{hint}");
        assert!(runeword_hint("Crafted Rare").is_none());
        Ok(())
    }

    #[test]
    fn runeword_json_keys_are_lowercase() {
        let json = runewords_json();
        assert!(json.get("steel").is_some(), "lowercase keys");
        assert!(json.get("Steel").is_none(), "no display-name keys");
    }

    #[test]
    fn oversized_bodies_are_rejected_without_allocating() {
        let request = format!("POST /api/build\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        assert!(matches!(
            read_request(&mut Cursor::new(request)),
            Some(Parsed::TooLarge)
        ));
        // At the cap itself the body is still served (a complete 1 MiB request).
        let header = format!("POST /api/build\r\nContent-Length: {MAX_BODY}\r\n\r\n");
        let mut request = header.into_bytes();
        request.resize(request.len() + MAX_BODY, 0);
        assert!(matches!(
            read_request(&mut Cursor::new(request)),
            Some(Parsed::Request(_, _, _))
        ));
    }
}
