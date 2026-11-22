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
// The kind of a build piece, for its colour: explicit "kind" when the build carries one, a
// runeword when it lists runes, else unique (the build files are mostly uniques/sets).
function kindOf(p) {
  if (p.kind) return p.kind;
  if (p.runes) return "runeword";
  return "unique";
}
// The ✕ "not needed" control of a build piece or farm entry (user decision 2026-11-22: the
// player removes pieces by hand); `needed` true is the undo. A clicked one stays pending
// (faded, disabled) over the re-renders until a frame no longer shows it.
const nnPending = new Set();
const nnKey = (build, name, needed) => `${String(build).toLowerCase()}|${String(name).toLowerCase()}|${needed}`;
const nnPend = (build, name, needed) => build && name && nnPending.has(nnKey(build, name, needed)) ? " pending" : "";
function nnButton(build, name, needed) {
  if (!build || !name) return "";
  const label = needed ? `needed again: ${name}` : `not needed: ${name} (hide it for ${build})`;
  return `<button type="button" class="nn${needed ? " undo" : ""}" data-build="${esc(build)}" data-name="${esc(name)}" data-needed="${needed}"`
    + ` title="${esc(label)}" aria-label="${esc(label)}"${nnPend(build, name, needed) ? " disabled" : ""}>${needed ? "↺" : "✕"}</button>`;
}
// A failed save, in a line outside the re-rendered panels.
function nnStatus(text) {
  $("nnstatus").textContent = text;
  $("nnstatus").hidden = !text;
}
// Drops the pending keys whose control the frame just rendered no longer shows.
function nnSettle() {
  const shown = new Set([...document.querySelectorAll("button.nn")].map(b => nnKey(b.dataset.build, b.dataset.name, b.dataset.needed)));
  for (const k of [...nnPending]) if (!shown.has(k)) nnPending.delete(k);
}
function piece(p, build) {
  // Green = worn; yellow = owned elsewhere: go equip it (carried, cube, private stash, shared stash).
  const tick = p.tick === "worn" ? ' <span class="tick">✓</span>'
    : p.tick === "owned" ? ' <span class="tick have">✓ owned</span>'
    : p.tick === "stash" ? ' <span class="tick have">✓ stash</span>' : "";
  // The next stage's pieces: a level above the character's reads "at 75", dimmed.
  const lvl = p.level == null ? "" : p.at ? `<span class="lv at">at ${p.level}</span>` : `<span class="lv">lvl ${p.level}</span>`;
  const sub = p.runes
    ? `<div class="sub">${esc(p.runes.join(" "))}${p.base_hint ? " · " + esc(p.base_hint) : p.bases && p.bases.length ? " · " + esc(p.bases.join(", ")) : ""}</div>`
    : p.base ? `<div class="sub">${esc(p.base)}</div>` : "";
  const stats = p.stats && p.stats.length
    ? `<ul class="st">${p.stats.map(s => `<li>${esc(s)}</li>`).join("")}</ul>` : "";
  const make = (p.craft ? `<div class="craft">${esc(p.craft)}</div>` : "")
    + ((p.missing || []).length ? `<div class="sub miss">missing ${esc(p.missing.join(" · "))}</div>` : "")
    + (p.base_owned ? `<div class="sub">base: ${esc(p.base_owned)}</div>` : "");
  const why = (p.why ? `<div class="why">${esc(p.why)}</div>` : "")
    + (p.notes ? `<div class="note">${esc(p.notes)}</div>` : "");
  const setup = (p.now ? ' <span class="badge now">wear now</span>' : "")
    + (p.setup && p.setup !== "alternative" ? ` <span class="badge">${esc(p.setup)}</span>` : "");
  return `<div class="pc ${esc(kindOf(p))} ${esc(p.state || "better")}${nnPend(build, p.name, false)}"><div class="top"><span class="nm">${esc(p.name)}${tick}</span>${setup}${lvl}${nnButton(build, p.name, false)}</div>${sub}${make}${stats}${why}</div>`;
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
function altFold(key, alt, build) {
  return alt.length
    ? `<details class="alts" data-slot="${esc(key)}"${openAlts.has(key) ? " open" : ""}><summary>${alt.length} alternative${alt.length > 1 ? "s" : ""}</summary>${alt.map(p => piece(p, build)).join("")}</details>`
    : "";
}
function pieceList(pieces, title, build) {
  const main = pieces.filter(p => p.setup !== "alternative");
  const alt = pieces.filter(p => p.setup === "alternative");
  return (main.map(p => piece(p, build)).join("") + altFold(title, alt, build)) || '<div class="empty">no build piece for this slot</div>';
}
function slotCard(title, eqHtml, pieces, build) {
  const eq = eqHtml == null ? "" : `<div class="eq"><span class="lbl">equipped</span>${eqHtml}</div>`;
  return `<div class="slot"><div class="slot-h"><span>${esc(title)}</span></div>${eq}<div class="pieces">${pieceList(pieces || [], title, build)}</div></div>`;
}
// The pieces marked not needed, each with its undo: listed only while "show hidden" is on;
// the toggle names how many there are.
let showHidden = false;
try { showHidden = localStorage.getItem("d2r-show-hidden") === "on"; } catch (e) { /* storage blocked */ }
// Each mark names the build it was made for (an earlier stage's marks hold for the later
// ones); the undo writes there.
function hiddenList(build, marks, what) {
  marks = marks || [];
  if (!marks.length) return "";
  const toggle = `<button type="button" class="showhidden" aria-pressed="${showHidden}">${showHidden ? "hide" : "show"} ${marks.length} not needed</button>`;
  const from = m => String(m.build).toLowerCase() === String(build).toLowerCase() ? "" : ` (${m.build})`;
  const list = showHidden
    ? `<span class="nnlist">${esc(what)}: ${marks.map(m => `<span class="nnitem${nnPend(m.build, m.name, true)}">${esc(m.name + from(m))}${nnButton(m.build, m.name, true)}</span>`).join("")}</span>` : "";
  return `<div class="hidden-row">${toggle}${list}</div>`;
}
document.addEventListener("click", e => {
  const t = e.target instanceof Element ? e.target.closest("button.showhidden, button.nn") : null;
  if (!t) return;
  if (t.classList.contains("showhidden")) {
    showHidden = !showHidden;
    try { localStorage.setItem("d2r-show-hidden", showHidden ? "on" : "off"); } catch (err) { /* storage blocked */ }
    if (lastFrame) render(lastFrame);
    return;
  }
  const { build, name } = t.dataset, needed = t.dataset.needed === "true";
  const key = nnKey(build, name, needed);
  nnPending.add(key);
  nnStatus("");
  if (lastFrame) render(lastFrame);
  fetch("/api/not-needed", { method: "POST", body: JSON.stringify({ build, name, needed }) }).then(async r => {
    if (!r.ok) throw new Error((await r.text()).trim() || r.status);
  }).catch(err => {
    nnPending.delete(key);
    nnStatus(`“${name}” not saved: ${err.message}`);
    if (lastFrame) render(lastFrame);
  });
});
function gear(g) {
  g = g || {};
  $("buildname").textContent = g.build || "";
  $("statsplan").innerHTML = (g.stats_plan || []).map(t => `<li>stats ${esc(t)}</li>`).join("");
  $("gear").innerHTML = (g.columns || []).map(col => {
    const eq = (col.equipped || []).map(e => {
      const unknown = !e.name || e.name === "(unknown)";
      const lvl = e.level ? ` <span class="lv">lvl ${e.level}</span>` : "";
      return `<span class="nm${unknown ? " unknown" : ""}">${esc(unknown ? "unidentified / magic" : e.name)}</span>${lvl}`;
    }).join("<br>") || '<span class="nm unknown">empty</span>';
    const note = col.note ? `<span class="warn">${esc(col.note)}</span>` : "";
    return slotCard(col.title, eq + note, col.pieces, g.build);
  }).join("");
  $("merc").innerHTML = (g.merc || []).map(c => slotCard(c.title, null, c.pieces, g.build)).join("");
  $("extras").innerHTML = (g.extras || []).map(c => slotCard(c.title, null, c.pieces, g.build)).join("");
  $("charms").innerHTML = charmCard(g.charms, g.build);
  $("gearhidden").innerHTML = hiddenList(g.build, g.not_needed, `Not needed for ${g.build || "this build"}`);
  $("mercbox").hidden = !(g.merc || []).length && !(g.extras || []).length && !(g.charms || []).length;
}
// The charm wishlist as one full-width card, its wishes in columns (user 2026-11-22: "the
// charms on the webapp into multiple columns, not one long one").
function charmCard(charms, build) {
  return (charms || []).length
    ? `<div class="slot charmcard"><div class="slot-h"><span>Charms</span></div><div class="pieces">${charms.map(c => charmLine(c, build)).join("")}</div></div>` : "";
}
// The next stage's gear (user 2026-11-22: "a fury gear list while I walk the summoner ... so
// that I can directly see what I could already wear"): per slot the pieces held or craftable
// now, every other one folded under "alternatives"; hidden without a next stage.
function nextGear(n) {
  $("nextpanel").hidden = !n;
  if (!n) {
    for (const id of ["nextgear", "nexthidden", "nextmerc", "nextcharms"]) $(id).innerHTML = "";
    return;
  }
  $("nextname").textContent = n.build;
  $("nextlevel").textContent = n.level != null ? `switch at level ${n.level}` : "";
  const card = c => {
    const ready = c.pieces.filter(p => p.ready), rest = c.pieces.filter(p => !p.ready);
    const shown = ready.map(p => piece(p, n.build)).join("") || '<div class="empty">nothing ready yet</div>';
    return `<div class="slot"><div class="slot-h"><span>${esc(c.title)}</span></div><div class="pieces">${shown}${altFold("next:" + c.title, rest, n.build)}</div></div>`;
  };
  $("nextgear").innerHTML = (n.columns || []).map(card).join("");
  $("nextmerc").innerHTML = (n.merc || []).map(card).join("");
  $("nextcharms").innerHTML = charmCard(n.charms, n.build);
  $("nexthidden").innerHTML = hiddenList(n.build, n.not_needed, `Not needed for ${n.build}`);
  $("nextmercbox").hidden = !(n.merc || []).length && !(n.charms || []).length;
}
// One charm wish (user 2026-11-22: "all of them until found"): ✓ once every wanted copy is
// owned, else have/wanted; a later stage's wish (the summoner sees the Fury's) carries its
// build name; the conditions an owned charm must meet are listed under it.
function charmLine(c, build) {
  const done = c.have >= c.count;
  const got = done ? ` <span class="tick">✓${c.count > 1 ? " " + c.count : ""}</span>`
    : c.have > 0 ? ` <span class="tick part">${c.have}/${c.count}</span>`
    : c.count > 1 ? ` <span class="tick part">0/${c.count}</span>` : "";
  const stage = c.stage ? ` <span class="badge">${esc(c.stage)}</span>` : "";
  const lvl = c.level != null ? `<span class="lv">lvl ${c.level}</span>` : "";
  const sub = c.base || c.size ? `<div class="sub">${esc(c.base || c.size)}</div>` : "";
  const list = (items, cls) => (items || []).length ? `<ul class="${cls}">${items.map(s => `<li>${esc(s)}</li>`).join("")}</ul>` : "";
  const why = (c.why ? `<div class="why">${esc(c.why)}</div>` : "") + (c.notes ? `<div class="note">${esc(c.notes)}</div>` : "");
  return `<div class="pc ${esc(c.kind)} ${done ? "found" : "neutral"}${nnPend(build, c.name, false)}"><div class="top"><span class="nm">${esc(c.name)}${got}</span>${stage}${lvl}${nnButton(build, c.name, false)}</div>${sub}${list(c.need, "st need")}${list(c.stats, "st")}${why}</div>`;
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
  // The runes the farm list's runewords still need (by "<Rune> Rune" stack name): those stacks
  // get the ⚑ marker, a tooltip naming the runewords and have/need when the character's runes
  // (stash and held) fall short; needed runes the stash lacks are added greyed.
  const farmRunes = new Map(((r.farm && r.farm.runes) || []).map(f => [f.rune + " Rune", f]));
  const runeNo = s => parseInt(String(s.code || "").slice(1), 10) || 99;
  const runeList = (m.runes || []).slice();
  for (const [name, f] of farmRunes) {
    if (!runeList.some(s => s.name === name)) runeList.push({ name, qty: 0, code: f.code, absent: true });
  }
  runeList.sort((a, b) => runeNo(a) - runeNo(b));
  const stacks = (list, cls) => (list || []).map(s => {
    const f = cls === "rune" ? farmRunes.get(s.name) : null;
    if (!f) return `<span class="stk ${cls}" title="${esc(s.code || "")}"><span class="nm">${esc(s.name)}</span><span class="q">${esc(s.qty)}</span></span>`;
    // Short only when the character's own runes (held ones too) don't cover the need.
    const short = f.have < f.need;
    const tip = `farm: ${f.for.join(", ")} · need ${f.need} · have ${f.have} (shared stash ${f.stash})`;
    const q = short ? `${esc(f.have)}/${esc(f.need)}` : s.absent ? `${esc(f.have)} held` : esc(s.qty);
    return `<span class="stk ${cls} farm${short ? " short" : ""}${s.absent ? " absent" : ""}" title="${esc(tip)}"><span class="nm">${esc(s.name)}</span>`
      + `<span class="q">${q}</span></span>`;
  }).join("");
  const group = (title, list, cls) => list && list.length
    ? `<div><h3>${title}</h3><div class="stacks">${stacks(list, cls)}</div></div>` : "";
  const mats = group("Runes", runeList, "rune") + group("Gems", m.gems, "gem") + group("Materials", m.other, "mat");
  const loaded = (r.stash_pages || []).some(p => (p.items || []).length)
    || (m.runes || []).length || (m.gems || []).length || (m.other || []).length;
  el.innerHTML = (loaded ? "" : '<div class="empty">open the stash in game once to load it</div>')
    + `<div class="pages">${pages}</div><div class="mats">${mats}</div>`;
}
// The skill trees: the class's three pages side by side, each laid out on the game's
// row/column grid. Allocated skills are gold with their hard points as a badge, the mouse-button
// skills carry an L / R mark, unspent ones are dim and the ones above the character level dimmer.
function skillTrees(s) {
  $("skillsum").textContent = s ? `${s.total} point${s.total === 1 ? "" : "s"} spent · without +skills from gear`
    + (s.plan ? " · badges: yours / guide now → guide final" : "") : "";
  if (!s) { $("skills").innerHTML = '<div class="empty">skill list not read yet</div>'; return; }
  const btn = (b, label) => b ? `<span class="chip"><b>${label}</b>${esc(b.name)}</span>` : "";
  const next = s.next
    ? `<span class="chip next" title="${esc(s.next.note || "")}"><b>next point</b>→ ${esc(s.next.skill)} (guide ${esc(s.next.target)} by lvl ${esc(s.next.by_level)})</span>`
    : s.plan ? '<span class="chip"><b>guide</b>skills on plan</span>' : "";
  const buttons = btn(s.left, "left") + btn(s.right, "right") + next;
  const trees = (s.trees || []).map(t => {
    const cells = (t.skills || []).map(k => {
      let cls = ["sk", k.points ? "on" : k.locked ? "locked" : "", k.left || k.right ? "btn" : "", k.plan || ""].join(" ");
      // The stage's recommendation: actual / target now → final target; a recommended skill
      // without points yet gets a dashed "planned" outline, the rest of the tree stays dim.
      const recommended = k.final != null && k.final > 0;
      if (recommended && !k.points) cls += " planned";
      const tg = recommended || (k.final != null && k.points)
        ? `<span class="tg" title="hard points / guide now → guide final">${esc(k.points)} / ${esc(k.target)} → ${esc(k.final)}${k.plan ? " · " + esc(k.plan) : ""}</span>` : "";
      const badge = k.points ? `<span class="pt">${esc(k.points)}</span>` : "";
      const mark = (k.left ? "L" : "") + (k.right ? "R" : "");
      const place = `grid-row:${Number(k.row) || "auto"};grid-column:${Number(k.col) || "auto"}`;
      return `<div class="${cls}" style="${place}" title="${esc(k.name)}">${badge}<span class="nm">${esc(k.name)}</span>`
        + `<span class="rq">lvl ${esc(k.req)}</span>${tg}${mark ? `<span class="mb">${mark}</span>` : ""}</div>`;
    }).join("");
    return `<div class="tree"><h3><span>${esc(t.name)}</span><span class="pts">${esc(t.points)}</span></h3><div class="tgrid">${cells}</div></div>`;
  }).join("");
  $("skills").innerHTML = (buttons ? `<div class="skbtns">${buttons}</div>` : "") + `<div class="trees">${trees}</div>`;
}
// Sell / Move / Keep · own / Keep · trade / Give away: every entry (one per JSON line), each
// group one comma-separated run that wraps to the panel width. An item in the wrong place
// shows its place in red: "Hunter's Bow 4os (page 4 → 1–2)".
function adviceEntry(e) {
  if (e.place == null) return esc(e.text);
  const place = e.wrong_place ? `<span class="wrong">${esc(e.place)}</span>` : esc(e.place);
  return esc(e.name) + (e.count > 1 ? ` ×${esc(e.count)}` : "") + ` (${place}`
    + (e.target ? ` → ${esc(e.target)}` : "") + (e.reason ? `, ${esc(e.reason)}` : "") + ")";
}
function sell(r) {
  const run = (label, list) => list && list.length
    ? `<div class="line run" style="color:${col(list[0].colour)}"><b>${label}:</b> ${list.map(adviceEntry).join(", ")}</div>` : "";
  $("sell").innerHTML = run("Sell", r.sell) + run("Move", r.move)
    + run("Keep · own (pages 1–2)", r.keep_own) + run("Keep · trade (pages 3–4)", r.keep_trade)
    + run("Give away", r.give)
    || '<div class="empty">nothing to sell or keep right now</div>';
}
// Sounds: the overlay plays them through the speakers (user decision 2026-11-22), whether or
// not this page is open, once per new marked drop on the map (gems excepted); the page only
// sets its switches (`/api/sounds`). The bell switches them all; the ▾ beside it opens one
// checkbox per sound and a ▶ that plays it once.
let soundState = null;
function soundUi() {
  const b = $("bell"), on = !!(soundState && soundState.on);
  b.classList.toggle("on", on);
  b.setAttribute("aria-pressed", on ? "true" : "false");
  b.title = on ? "sounds on (click to mute; ▾ picks which)" : "sounds off (click to enable)";
  $("soundlist").innerHTML = (soundState ? soundState.sounds : []).map(s =>
    `<div class="snd"><label><input type="checkbox" data-sound="${esc(s.name)}"${s.on ? " checked" : ""}> ${esc(s.label)}`
    + `${s.loud ? ' <span class="loud">louder</span>' : ""}</label>`
    + `<button type="button" class="test" data-test="${esc(s.name)}" title="play it once">▶</button></div>`).join("");
}
function soundPost(path, body) {
  return fetch(path, { method: "POST", body: JSON.stringify(body) })
    .then(r => r.ok ? r.json() : Promise.reject(new Error(r.status)));
}
function soundSet(body) {
  soundPost("/api/sounds", body).then(s => { soundState = s; soundUi(); })
    .catch(() => { nnStatus("could not save the sound switch"); soundUi(); });
}
fetch("/api/sounds").then(r => r.json()).then(s => { soundState = s; soundUi(); }).catch(() => {});
$("bell").addEventListener("click", () => {
  if (!soundState) return;
  const on = !soundState.on;
  soundSet({ on });
  if (on) soundPost("/api/sounds/test", { sound: "markedDrop" }).catch(() => {});
});
$("soundlist").addEventListener("change", e => {
  const name = e.target.dataset && e.target.dataset.sound;
  if (name) soundSet({ sound: name, on: e.target.checked });
});
$("soundlist").addEventListener("click", e => {
  const name = e.target.dataset && e.target.dataset.test;
  if (name) soundPost("/api/sounds/test", { sound: name }).catch(() => nnStatus("could not play the sound"));
});
function upgrades(list) {
  list = list || [];
  const el = $("upgrades");
  el.hidden = !list.length;
  el.innerHTML = list.length ? `<div class="wrap"><span class="t">Upgrades available</span>` + list.map(u =>
    `<span class="up ${esc(u.kind)}"><span class="k">${u.kind === "craft" || u.kind === "shop" ? u.kind : "equip"}</span>${esc(u.name)} <span class="sl">${esc(u.slot)}</span> <span class="why">· ${esc(u.why)}</span></span>`).join("") + "</div>" : "";
}
// The recipes the current stage uses, grouped by kind, one card each. Rune and gem upgrades
// fold into one group (only the ones making an input another recipe still misses arrive).
const RECIPE_GROUPS = [["runeword", "Runewords"], ["craft", "Crafting"], ["cube", "Cube"], ["socket", "Sockets and quest services"], ["upgrade", "Upgrades"]];
let recipeFoldOpen = false;
document.addEventListener("toggle", e => {
  const d = e.target;
  if (d instanceof HTMLDetailsElement && d.classList.contains("rgroup")) recipeFoldOpen = d.open;
}, true);
function recipeCard(r) {
  const ins = (r.inputs || []).map(i => {
    const mk = i.status === "have" ? "✓" : i.status === "likely" ? "✓?" : i.status === "missing" ? `${esc(i.have)}/${esc(i.need)}` : "check";
    const hint = i.hint ? `<span class="hint">you have: ${esc(i.hint)}</span>` : "";
    return `<li class="${esc(i.status)}"><span class="mk">${mk}</span><span class="tx">${esc(i.text)}</span>${hint}</li>`;
  }).join("");
  const counted = (r.inputs || []).filter(i => i.status === "have" || i.status === "missing");
  const ready = counted.length && counted.every(i => i.status === "have");
  // Quest-reward recipes: whether the reward waits (used ones never arrive).
  // An unfinished quest needs no tag: the card leaves by itself once the reward is used.
  const quest = r.quest === "ready" ? '<span class="qt ready">reward ready</span>' : "";
  return `<div class="rc ${esc(r.kind)}${ready ? " ready" : ""}">${r.req ? `<div class="rc-req">${esc(r.req)}</div>` : ""}`
    + `<div class="rc-h"><span class="nm">${esc(r.name)}</span>${quest}</div><ul class="ins">${ins}</ul>`
    + `${r.result ? `<div class="res">→ ${esc(r.result)}</div>` : ""}${r.notes ? `<div class="note">${esc(r.notes)}</div>` : ""}</div>`;
}
function recipes(list) {
  list = list || [];
  $("recipepanel").hidden = !list.length;
  const known = RECIPE_GROUPS.map(g => g[0]);
  const groups = RECIPE_GROUPS.concat([["", "Other"]]).map(([kind, title]) => {
    const group = list.filter(r => kind ? r.kind === kind : !known.includes(r.kind));
    const plain = group.filter(r => !(kind === "upgrade" && r.material));
    const folded = kind === "upgrade" ? group.filter(r => r.material) : [];
    let html = plain.length ? `<h3 class="row-h">${title}</h3><div class="rcs">${plain.map(recipeCard).join("")}</div>` : "";
    if (folded.length) {
      html += `<details class="rgroup"${recipeFoldOpen ? " open" : ""}><summary>${folded.length} rune / gem upgrade${folded.length > 1 ? "s" : ""} for inputs you still miss</summary>`
        + `<div class="rcs">${folded.map(recipeCard).join("")}</div></details>`;
    }
    return html;
  });
  $("recipes").innerHTML = groups.join("");
}
// The farm list per build, in farm-list order: one card per still-missing entry (held ones
// are left out, owned runes too), its runes short, base hint, and at the bottom where to farm it.
function farm(f) {
  const groups = (f && f.groups) || [];
  const held = (f && f.held) || [];
  $("farmpanel").hidden = !groups.length;
  $("farm").innerHTML = groups.map(g => {
    const items = g.items || [];
    const done = held.filter(h => h.for === g.for).length;
    const cards = items.map(i => {
      const n = i.count > 1 ? ` ${esc(i.stashed)}/${esc(i.count)}` : "";
      const runes = (i.runes || []).length
        ? `<div class="rn">${i.runes.map(x => `<span class="missing">${esc(x.rune)} ${esc(x.have)}/${esc(x.need)}</span>`).join("")}</div>` : "";
      const bases = i.base_hint ? `<div class="bs">${esc(i.base_hint)}</div>`
        : (i.bases || []).length ? `<div class="bs">${esc(i.bases.join(", "))}</div>` : "";
      // A rolled entry: its slot and the stats it needs ("amulet with Teleport").
      const needs = (i.needs || []).length
        ? `<div class="bs">${esc(i.slot || "any item")} with ${esc(i.needs.join(" + "))}</div>` : "";
      const note = i.note ? `<div class="bs">${esc(i.note)}</div>` : "";
      const where = i.where ? `<div class="wh">${esc(i.where)}</div>` : "";
      return `<div class="fm ${esc(i.kind)} missing${nnPend(g.for, i.name, false)}"><div class="fm-h"><span class="nm">${esc(i.name)}</span>`
        + `<span class="kd">${esc(i.kind)}</span><span class="fst missing">missing${n}</span>${nnButton(g.for, i.name, false)}</div>${needs}${note}${runes}${bases}${where}</div>`;
    }).join("");
    return `<h3 class="row-h">for ${esc(g.for)} · ${done}/${done + items.length} held</h3><div class="fms">${cards}</div>`
      + hiddenList(g.for, g.not_needed, "Not needed");
  }).join("");
}
// The open trade window: one column per store tab, the build's pieces first and bold (with
// their slot), then the items whose stats beat the worn ones ("▲ better" and why), the ones
// above the character level dimmed.
function shop(s) {
  $("shoppanel").hidden = !s;
  if (!s) { $("shop").innerHTML = ""; $("shopnpc").textContent = ""; return; }
  $("shopnpc").textContent = `${s.npc || "trade window"} · bold ▲ = a piece of your build · ▲ better = beats what you wear · faded = above your level`;
  const row = i => `<div class="si${i.slot ? " hit" : ""}${i.high ? " high" : ""}${i.quality === 4 ? " magic" : ""}">`
    + `<span>${esc(i.name)}${i.count > 1 ? ` ×${esc(i.count)}` : ""}</span>${i.slot ? `<span class="sl">${esc(i.slot)}</span>` : ""}`
    + `${i.better ? '<span class="bt">▲ better</span>' : ""}`
    + `${i.req ? `<span class="lv">lvl ${esc(i.req)}</span>` : ""}`
    + `${i.better && i.why ? `<span class="why">${esc(i.why)}</span>` : ""}</div>`;
  $("shop").innerHTML = [["armor", "Armor"], ["weapons", "Weapons"], ["misc", "Misc"]].map(([tab, title]) => {
    const list = (s.items || []).filter(i => i.tab === tab);
    const rank = i => i.slot ? 0 : i.better ? 1 : 2;
    const sorted = [0, 1, 2].flatMap(r => list.filter(i => rank(i) === r));
    return list.length ? `<div><h3 class="row-h">${title}</h3>${sorted.map(row).join("")}</div>` : "";
  }).join("") || '<div class="empty">nothing for sale</div>';
}
// One rendered frame from a live payload (the first-paint fetch and every SSE event).
// The Gear panel follows the server's gear_attention (open while there is an upgrade to wear
// or to find) until the viewer clicks its heading; from then on their choice sticks for this
// page session. Only the body's hidden flag changes: render never rebuilds the wrapper.
let gearChoice = null;
function setGear(open) {
  $("gearbody").hidden = !open;
  $("gearfold").setAttribute("aria-expanded", String(open));
}
$("gearfold").addEventListener("click", () => {
  gearChoice = $("gearbody").hidden;
  setGear(gearChoice);
});
function gearFold(r) {
  const why = r && r.live && r.gear_attention ? (r.gear_attention_why || "") : "";
  $("gearwhy").textContent = why;
  setGear(gearChoice != null ? gearChoice : why !== "");
}
// The frame on screen, re-rendered when "show hidden" flips.
let lastFrame = null;
function render(r) {
  lastFrame = r;
  gearFold(r);
  if (!r || !r.live) {
    $("who").innerHTML = '<span class="waiting">waiting for game…</span>';
    for (const id of ["guide", "gear", "gearhidden", "merc", "extras", "charms", "statsplan", "stash", "sell", "skills", "recipes", "farm"]) $(id).innerHTML = "";
    nextGear(null);
    $("recipepanel").hidden = true;
    $("farmpanel").hidden = true;
    shop(null);
    $("upgrades").hidden = true;
    $("skillsum").textContent = "";
    $("updated").textContent = "";
    document.title = "D2R Dashboard (waiting for game)";
    $("mercbox").hidden = true;
    nnSettle();
    return;
  }
  const p = r.player;
  const seen = new Date(r.updated_at * 1000).toLocaleTimeString();
  $("who").innerHTML = `${esc(p.name)}<small>${esc([p.class, p.level != null ? "level " + p.level : null, p.difficulty, r.area].filter(Boolean).join(" · "))}</small>`
    + (r.offline ? ' <span class="waiting">offline — last seen ' + esc(seen) + '</span>' : "");
  $("updated").textContent = (r.offline ? "last seen " : "updated ") + seen;
  document.title = `${r.offline ? "(offline) " : ""}${p.name}${p.level != null ? " L" + p.level : ""} · ${r.area || "?"} — D2R Dashboard`;
  guide(r.guide);
  upgrades(r.upgrades);
  shop(r.shop);
  gear(r.gear);
  nextGear(r.next_gear);
  skillTrees(r.skills);
  recipes(r.recipes);
  farm(r.farm);
  stash(r);
  sell(r);
  nnSettle();
}
// First paint from the one-shot endpoint, then live updates from the Server-Sent-Events
// stream: the server pushes whenever the JSON changes and the browser reconnects on its own.
fetch("/api/live").then(r => r.json()).then(render).catch(() => {});
const stream = new EventSource("/api/live-stream");
stream.onmessage = e => {
  try { render(JSON.parse(e.data)); } catch (err) { /* a partial event: the next one wins */ }
};
let streamLost = false;
stream.onerror = () => { streamLost = true; };
const reloadAfterOverlayRestart = () => { if (streamLost) location.reload(); };
stream.onopen = reloadAfterOverlayRestart;
