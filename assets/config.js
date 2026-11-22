// Escapes a config-supplied string for interpolation into HTML text and
// double-quoted attribute values (character names and build names are free text).
function esc(s) {
  return String(s).replace(/[&<>"']/g, ch => ({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[ch]));
}
// "played 2 h ago" from unix seconds; "" when never seen.
function playedAgo(t) {
  if (t == null) return "";
  const s = Math.max(0, Date.now() / 1000 - t);
  const ago = s < 120 ? "just now" : s < 7200 ? `${Math.round(s / 60)} min ago`
    : s < 172800 ? `${Math.round(s / 3600)} h ago` : `${Math.round(s / 86400)} days ago`;
  return `played ${ago}`;
}
function capitalize(s) { return s ? s[0].toUpperCase() + s.slice(1) : s; }
// The build options for a character of class `cls` (null: unknown): the builds of its class or
// of any class, labelled with their stage chain; the selected one always stays in the list.
function buildOptions(builds, cls, selected) {
  return ['<option value="">none</option>']
    .concat(builds.filter(b => b.slug === selected || !cls || !b.class || b.class === cls)
      .map(b => `<option value="${esc(b.slug)}">${esc((b.chain || [b.name]).join(" \u2192 "))}</option>`))
    .join("");
}
async function load() {
  const [cfg, buildsResp] = await Promise.all([fetch("/api/config"), fetch("/api/builds")].map(r => r.then(x => x.json())));
  const builds = buildsResp.builds || [];
  const root = document.getElementById("chars");
  root.innerHTML = (cfg.characters || []).length ? ""
    : '<div class="hint">No character seen yet: play one and it shows up here.</div>';
  (cfg.characters || []).forEach(c => {
    const div = document.createElement("div");
    div.className = "char";
    const shown = c.build_name
      ? (c.class_default ? `class default: ${esc(c.build_name)}` : `now: ${esc(c.build_name)}`)
      : "no build selected";
    const meta = [c.class ? esc(capitalize(c.class)) : "", shown, playedAgo(c.last_played)]
      .filter(Boolean).join(" · ");
    div.innerHTML = `<span class="cname">${esc(c.name)}</span>`
      + `<select class="build-select">${buildOptions(builds, c.class, c.build)}</select>`
      + `<span class="meta">${meta}</span>`
      + `<button class="del" type="button">delete</button>`;
    const select = div.querySelector(".build-select");
    select.value = c.build || "";
    select.addEventListener("change", () => saveBuild(c.name, select.value));
    const del = div.querySelector(".del");
    let disarm = null;
    del.addEventListener("click", () => {
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
  const money = document.getElementById("money");
  money.checked = cfg.money_farming !== false;
  money.disabled = false;
}
// Turns money farming on or off (POST /api/money-farming), then reloads the page's lists.
async function saveMoney(on) {
  const r = await fetch("/api/money-farming", {method: "POST", body: JSON.stringify({on})});
  document.getElementById("err").textContent = r.ok ? "" : `money farming not saved: ${await r.text()}`;
  load();
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
// Posts the character's build selection (`[bis.<name>] build`); an empty value clears it.
async function saveBuild(name, build) {
  const r = await fetch("/api/bis", {method: "POST", body: JSON.stringify({name, build: build || null})});
  document.getElementById("err").textContent = r.ok ? "" : `build not saved: ${await r.text()}`;
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
load();
loadFilter();
