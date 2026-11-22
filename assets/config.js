const SLOTS = ["amulet","ring1","ring2","body","helm","gloves","boots","belt","weapon","offhand","swap"];
// Escapes a config-supplied string for interpolation into HTML text and
// double-quoted attribute values (character names and build names are free text).
function esc(s) {
  return String(s).replace(/[&<>"']/g, ch => ({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[ch]));
}
// One piece line: name (level), tick, strike-through, runeword hint.
function pieceHtml(p) {
  const level = p.level != null ? ` (${p.level})` : "";
  // Green = worn; yellow = owned elsewhere (carried, cube, private or shared stash).
  const tick = p.tick === "worn" ? ` <span class="owned">\u2713</span>`
    : p.tick === "owned" ? ` <span class="owned have">\u2713</span>`
    : p.tick === "stash" ? ` <span class="owned have">\u2713</span><span class="hint">(stash)</span>` : "";
  const strike = p.better ? " better" : "";
  const hint = p.runes
    ? `<div class="rwhint">${esc(p.runes.join(" + "))} · ${esc((p.bases || []).join(", "))}</div>` : "";
  return `<div class="piece${strike}">${esc(p.name)}${esc(level)}${tick}</div>` + hint;
}
// "played 2 h ago" from unix seconds; "" when never seen.
function playedAgo(t) {
  if (t == null) return "";
  const s = Math.max(0, Date.now() / 1000 - t);
  const ago = s < 120 ? "just now" : s < 7200 ? `${Math.round(s / 60)} min ago`
    : s < 172800 ? `${Math.round(s / 3600)} h ago` : `${Math.round(s / 86400)} days ago`;
  return `played ${ago}`;
}
// Open/closed per lowercased name, remembered in localStorage (may be unavailable).
const OPEN_KEY = "d2r-config-open";
function openStates() {
  try { return JSON.parse(localStorage.getItem(OPEN_KEY) || "{}") || {}; } catch (e) { return {}; }
}
function rememberOpen(name, open) {
  try {
    const states = openStates();
    states[name.toLowerCase()] = open;
    localStorage.setItem(OPEN_KEY, JSON.stringify(states));
  } catch (e) { /* not remembered */ }
}
function capitalize(s) { return s ? s[0].toUpperCase() + s.slice(1) : s; }
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
  const states = openStates();
  (cfg.characters || []).forEach((c, i) => {
    const div = document.createElement("details");
    div.className = "char";
    // The character name goes in a data attribute, not in classes/ids: names with spaces would
    // otherwise break the selectors.
    div.dataset.name = c.name;
    // Collapsed by default except the most recently played one (the list is ordered by it).
    const remembered = states[c.name.toLowerCase()];
    div.open = remembered != null ? remembered : (i === 0 && c.last_played != null);
    const sel = `<select class="build-select">${options()}</select>`;
    const shown = c.build_name
      ? (c.class_default ? `class default: ${esc(c.build_name)}` : esc(c.build_name))
      : "no build selected";
    const meta = [c.class ? esc(capitalize(c.class)) : "", shown, playedAgo(c.last_played)]
      .filter(Boolean).join(" · ");
    const damages = (c.damages || []).length ? `<div class="hint">damages: ${esc(c.damages.join(", "))}</div>` : "";
    div.innerHTML = `<summary><span class="cname">${esc(c.name)}</span><span class="meta">${meta}</span>`
      + `<button class="del" type="button">delete</button></summary>`
      + `build: ${sel} <button class="save">Save</button>${damages}`
      + `<h3>BiS</h3>` + slotsHtml(c.slots)
      + `<h3>Mercenary</h3>` + slotsHtml(c.merc_slots);
    const select = div.querySelector(".build-select");
    select.value = c.build || "";
    div.querySelector(".save").addEventListener("click", () => saveBis(c.name, select.value));
    div.addEventListener("toggle", () => rememberOpen(c.name, div.open));
    const del = div.querySelector(".del");
    let disarm = null;
    del.addEventListener("click", ev => {
      // Inside the summary: don't toggle the section.
      ev.preventDefault();
      ev.stopPropagation();
      if (!del.classList.contains("armed")) {
        del.classList.add("armed");
        del.textContent = "really delete?";
        del.title = "a character in the current game comes back within a minute";
        disarm = setTimeout(() => { del.classList.remove("armed"); del.textContent = "delete"; }, 4000);
        return;
      }
      clearTimeout(disarm);
      deleteChar(c.name);
    });
    root.appendChild(div);
  });
  renderNotNeeded(cfg.not_needed || []);
}
// Every not-needed mark by build (cfg.not_needed), each with a "bring back" button.
function renderNotNeeded(groups) {
  const root = document.getElementById("notneeded");
  root.innerHTML = groups.length ? "" : '<div class="hint">Nothing is marked not needed.</div>';
  groups.forEach(g => {
    const head = document.createElement("div");
    head.className = "nn-build";
    head.innerHTML = esc(g.build) + (g.known ? "" : ' <span class="hint">(no such build any more)</span>');
    root.appendChild(head);
    g.names.forEach(name => {
      const row = document.createElement("div");
      row.className = "nn-piece";
      row.innerHTML = `${esc(name)} <button type="button">bring back</button>`;
      const button = row.querySelector("button");
      button.addEventListener("click", () => { button.disabled = true; bringBack(g.build, name); });
      root.appendChild(row);
    });
  });
}
// Unmarks one piece (POST /api/not-needed with needed: true), then reloads the page's lists.
async function bringBack(build, name) {
  const r = await fetch("/api/not-needed", {method: "POST", body: JSON.stringify({build, name, needed: true})});
  document.getElementById("err").textContent = r.ok ? "" : `bring back failed: ${await r.text()}`;
  load();
}
// Forgets the character (config entries and last-played time), then reloads the list.
async function deleteChar(name) {
  const r = await fetch("/api/character/delete", {method: "POST", body: JSON.stringify({name})});
  if (!r.ok) { document.getElementById("err").textContent = `delete failed: ${await r.text()}`; }
  load();
}
// Posts ONLY the `[bis.<name>]` build selection; an empty value clears it.
async function saveBis(name, build) {
  await fetch("/api/bis", {method: "POST", body: JSON.stringify({name, build: build || null})});
  load();
}
// The playstyle's loot filter (GET /api/loot-filter): its profile name and a copy button. The
// copied text is the response body as served (the exact file), never re-serialised.
let filterText = null;
async function loadFilter() {
  const name = document.getElementById("filtername");
  const button = document.getElementById("copyfilter");
  try {
    const r = await fetch("/api/loot-filter");
    if (!r.ok) {
      name.textContent = r.status === 404 ? "no loot filter in this playstyle" : `loot filter: ${await r.text()}`;
      return;
    }
    filterText = await r.text();
    let profile = "";
    try { profile = JSON.parse(filterText).name || ""; } catch (e) { /* shown without a name */ }
    name.textContent = `${profile || "unnamed profile"} \u2014 loot filter of this playstyle`;
    button.hidden = false;
  } catch (e) {
    name.textContent = "loot filter not loaded (overlay not running?)";
  }
}
let copiedTimer = null;
// The click handler itself calls writeText (the clipboard needs the user's gesture; localhost
// is a secure context). Refused: the JSON goes in a read-only, selected textarea instead.
async function copyFilter() {
  if (filterText == null) return;
  const status = document.getElementById("filterstatus");
  const box = document.getElementById("filterbox");
  clearTimeout(copiedTimer);
  try {
    await navigator.clipboard.writeText(filterText);
    box.hidden = true;
    status.textContent = "Copied";
    copiedTimer = setTimeout(() => { status.textContent = ""; }, 3000);
  } catch (e) {
    box.value = filterText;
    box.hidden = false;
    box.focus();
    box.select();
    status.textContent = "copy refused: the JSON is selected below, press Ctrl+C";
  }
}
async function addChar() {
  const name = document.getElementById("newname").value.trim();
  const build = document.getElementById("newbuild").value;
  if (name && build) { await saveBis(name, build); }
}
load();
loadFilter();
