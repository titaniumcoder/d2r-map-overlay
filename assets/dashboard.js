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
function piece(p) {
  // Green = worn; yellow = owned elsewhere: go equip it (carried, cube, private stash, shared stash).
  const tick = p.tick === "worn" ? ' <span class="tick">✓</span>'
    : p.tick === "owned" ? ' <span class="tick have">✓ owned</span>'
    : p.tick === "stash" ? ' <span class="tick have">✓ stash</span>' : "";
  const lvl = p.level != null ? `<span class="lv">lvl ${p.level}</span>` : "";
  const sub = p.runes
    ? `<div class="sub">${esc(p.runes.join(" "))}${p.base_hint ? " · " + esc(p.base_hint) : p.bases && p.bases.length ? " · " + esc(p.bases.join(", ")) : ""}</div>`
    : p.base ? `<div class="sub">${esc(p.base)}</div>` : "";
  const stats = p.stats && p.stats.length
    ? `<ul class="st">${p.stats.map(s => `<li>${esc(s)}</li>`).join("")}</ul>` : "";
  const why = (p.why ? `<div class="why">${esc(p.why)}</div>` : "")
    + (p.notes ? `<div class="note">${esc(p.notes)}</div>` : "");
  const setup = (p.now ? ' <span class="badge now">wear now</span>' : "")
    + (p.setup && p.setup !== "alternative" ? ` <span class="badge">${esc(p.setup)}</span>` : "");
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
  $("statsplan").innerHTML = (g.stats_plan || []).map(t => `<li>stats ${esc(t)}</li>`).join("");
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
// Upgrade chime: the bell toggles it (the click is the user gesture browsers need before audio
// may play); the choice is remembered. A new upgrade key since the last frame chimes once; the
// first frame after loading only records the keys.
// Upgrade keys already announced for `seenFor` (the character): keys are only ever added, so
// a key that vanishes for one flaky tick doesn't chime again; a new character starts silent.
let soundOn = false, audio = null, seenUpgrades = null, seenFor = null;
try { soundOn = localStorage.getItem("d2r-chime") === "on"; } catch (e) { /* storage blocked */ }
function ensureAudio() {
  const AC = window.AudioContext || window.webkitAudioContext;
  if (!audio && AC) audio = new AC();
  if (audio && audio.state === "suspended") audio.resume().catch(() => {});
}
function chime() {
  if (!audio) return;
  const play = () => {
    const t = audio.currentTime;
    for (const [f, d] of [[659.25, 0], [987.77, 0.16]]) {
      const o = audio.createOscillator(), g = audio.createGain();
      o.type = "sine"; o.frequency.value = f;
      g.gain.setValueAtTime(0.0001, t + d);
      g.gain.exponentialRampToValueAtTime(0.12, t + d + 0.02);
      g.gain.exponentialRampToValueAtTime(0.0001, t + d + 0.24);
      o.connect(g); g.connect(audio.destination);
      o.start(t + d); o.stop(t + d + 0.26);
    }
  };
  if (audio.state === "running") play(); else audio.resume().then(play).catch(() => {});
}
function bellUi() {
  const b = $("bell");
  b.classList.toggle("on", soundOn);
  b.setAttribute("aria-pressed", soundOn ? "true" : "false");
  b.title = soundOn ? "upgrade chime on (click to mute)" : "upgrade chime off (click to enable)";
}
$("bell").addEventListener("click", () => {
  soundOn = !soundOn;
  try { localStorage.setItem("d2r-chime", soundOn ? "on" : "off"); } catch (e) { /* storage blocked */ }
  if (soundOn) { ensureAudio(); chime(); }
  bellUi();
});
// A remembered "on" still needs one gesture on the page before audio may start.
document.addEventListener("pointerdown", () => { if (soundOn) ensureAudio(); }, true);
bellUi();
function upgrades(list, who) {
  list = list || [];
  const keys = list.map(u => u.key);
  if (who !== seenFor) { seenFor = who; seenUpgrades = new Set(keys); }
  const fresh = keys.filter(k => !seenUpgrades.has(k));
  if (fresh.length && soundOn) chime();
  fresh.forEach(k => seenUpgrades.add(k));
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
// The farm list: one group per planned build, one card per item with its cross-character
// status; a runeword still to build lists its runes (have/need) and allowed bases.
function farm(f) {
  const groups = (f && f.groups) || [];
  $("farmpanel").hidden = !groups.length;
  // "Nightmare worth farming: Mephisto → Bloodfist, Heart of the Oak (Ko); Andariel → …"
  const w = f && f.worth, bosses = (w && w.bosses) || [];
  $("worth").innerHTML = bosses.length
    ? `${esc(w.difficulty || "")} worth farming: ` + bosses.map(b => `<b>${esc(b.boss)}</b> → `
      + b.items.map(i => esc(i.name) + (i.runes.length ? ` (${esc(i.runes.join(" "))})` : "")).join(", ")).join("; ")
    : "";
  $("farm").innerHTML = groups.map(g => {
    const items = g.items || [];
    const done = items.filter(i => i.status === "done").length;
    const cards = items.map(i => {
      const n = i.count > 1 ? ` ${esc(i.stashed)}/${esc(i.count)}` : "";
      const carried = i.status !== "done" && i.carried ? `<div class="cr">on this character: ${esc(i.carried)} · put in the shared stash</div>` : "";
      const runes = (i.runes || []).length
        ? `<div class="rn">${i.runes.map(x => `<span class="${esc(x.status)}">${esc(x.rune)} ${x.status === "have" ? "✓" : esc(x.have) + "/" + esc(x.need)}</span>`).join("")}</div>` : "";
      const bases = i.base_hint ? `<div class="bs">${esc(i.base_hint)}</div>`
        : (i.bases || []).length ? `<div class="bs">${esc(i.bases.join(", "))}</div>` : "";
      // A rolled entry: its slot and the stats it needs ("amulet with Teleport").
      const needs = (i.needs || []).length
        ? `<div class="bs">${esc(i.slot || "any item")} with ${esc(i.needs.join(" + "))}</div>` : "";
      const note = i.note ? `<div class="bs">${esc(i.note)}</div>` : "";
      return `<div class="fm ${esc(i.kind)} ${esc(i.status)}"><div class="fm-h"><span class="nm">${esc(i.name)}</span>`
        + `<span class="kd">${esc(i.kind)}</span><span class="fst ${esc(i.status)}">${esc(i.status)}${n}</span></div>${carried}${needs}${note}${runes}${bases}</div>`;
    }).join("");
    return `<h3 class="row-h">for ${esc(g.for)} · ${done}/${items.length} done</h3><div class="fms">${cards}</div>`;
  }).join("");
}
// The route's current step: its title, what to do, why, and the step after it.
function next(n) {
  $("nextpanel").hidden = !n;
  $("next").innerHTML = n
    ? `<div class="step">${esc(n.title)}</div>`
      + ((n.todo || []).length ? `<ul class="todo">${n.todo.map(t => `<li>${esc(t)}</li>`).join("")}</ul>` : "")
      + (n.note ? `<div class="note">${esc(n.note)}</div>` : "")
      + (n.then ? `<div class="then">then: ${esc(n.then)}</div>` : "")
    : "";
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
function render(r) {
  gearFold(r);
  if (!r || !r.live) {
    $("who").innerHTML = '<span class="waiting">waiting for game…</span>';
    for (const id of ["guide", "gear", "merc", "extras", "statsplan", "stash", "sell", "skills", "recipes", "farm", "worth"]) $(id).innerHTML = "";
    $("recipepanel").hidden = true;
    $("farmpanel").hidden = true;
    next(null);
    shop(null);
    // The upgrade keys stay remembered: coming back from the menu must not chime again.
    $("upgrades").hidden = true;
    $("skillsum").textContent = "";
    $("updated").textContent = "";
    document.title = "D2R Dashboard (waiting for game)";
    $("mercbox").hidden = true;
    return;
  }
  const p = r.player;
  const seen = new Date(r.updated_at * 1000).toLocaleTimeString();
  $("who").innerHTML = `${esc(p.name)}<small>${esc([p.class, p.level != null ? "level " + p.level : null, p.difficulty, r.area].filter(Boolean).join(" · "))}</small>`
    + (r.offline ? ' <span class="waiting">offline — last seen ' + esc(seen) + '</span>' : "");
  $("updated").textContent = (r.offline ? "last seen " : "updated ") + seen;
  document.title = `${r.offline ? "(offline) " : ""}${p.name}${p.level != null ? " L" + p.level : ""} · ${r.area || "?"} — D2R Dashboard`;
  guide(r.guide);
  upgrades(r.upgrades, p.name);
  next(r.next);
  shop(r.shop);
  gear(r.gear);
  skillTrees(r.skills);
  recipes(r.recipes);
  farm(r.farm);
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
