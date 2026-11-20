# D2R valuable drops: what to highlight

Researched 2026-11-15. Game state: **Reign of the Warlock** (patch 3.0, 2026-02-11), now patch 3.3 (2026-08-18, ladder season 15). Patch 3.4 "Legacy of Terror" event runs 2026-11-13 to 11-19, with season 16 expected right after it.
The machine-readable rules are in `valuables.json` (67 rules: 13 S, 29 A, 25 B). It is generated from blizzhackers/d2data (commit 2026-11-14, "Updated for new patch"), and every item code and unique/set name was checked against that data.

Tiers: **S** = always highlight, **A** = highlight, **B** = optional.

There are no live prices here on purpose. Maxroll removed its rune price values in February 2026, and prices swing a lot over a ladder season. The tiers below are the consensus on what is worth picking up, not what it sells for.

---

## How the overlay should read the rules

- `quality: "normal"` on a runeword base means a **white base, either normal or superior**. Magic or rare items with sockets cannot take a runeword (d2db, maxroll). Use `"superior"` only when superior is required.
- Unique and set names are decided when the item drops, and memory readers show them before the item is identified. That is why rules carry `item_names` with the exact `index` strings from the data, including Blizzard's typos: `"Thudergod's Vigor"`, `"Deaths's Web"`, `"Griswolds's Redemption"`, `"Fathom"` (= Death's Fathom), `"Wisp"` (= Wisp Projector).
- Some base codes host several uniques or sets: `rin` (11 uniques), `amu` (13), `jew` (8 Rainbow Facets), `cm3` (Gheed's plus the Sunders), and also `xsk`, `uar`, `7cr`, `uhm`. On these bases, matching on code and quality alone gives false positives.
- `needs_stats: true` means base, quality, sockets and ethereal are not enough to decide value. Show these as "check me", not as a jackpot.
- If several rules match, use the highest tier.

### Socket rules (from d2data `itemtypes.json`)
The most sockets an item can have is `min(base gemsockets, ilvl cap)`. The ilvl cap applies to sockets on drops, to the cube socket recipe (random count, white non-superior items only) and to Larzuk (always the maximum).

| item type | ilvl 1–25 | 26–40 | 41+ |
|---|---|---|---|
| Body armor, swords, polearms, spears, maces, hammers, bows | 3 | 4 | 6 |
| Axes | 4 | 5 | 6 |
| Shields | 3 | 3 | 4 |
| Paladin shields, Amazon bows | 3 | 4 | 4 / 5 |
| Helms | 2 | 2 | 3 |
| Circlets | 1 | 2 | 3 |
| Daggers | 2 | 3 | 3 |
| Grimoires | 2 | 3 | 3 (base max 2) |

What this means in practice:
- Larzuk's socket quest is not used for runeword bases (user, 2026-11-20), so unsocketed bases are not kept for it.
- A 4-socket Crystal Sword can already drop in Normal from ilvl 26.

---

## a) Trade value (main characters)

### Runes
| tier | runes | codes |
|---|---|---|
| S | Vex, Ohm, Lo, Sur, Ber, Jah, Cham, Zod (high runes; Ber and Jah are the top) | r26–r33 |
| A | Lem, Pul, Um, Mal, Ist, Gul (mid runes, used for smaller trades; Ist and Gul trade most easily) | r20–r25 |
| B | El–Fal. Little value on their own, but complete Spirit sets (Tal Thul Ort Amn + Hel) and Insight sets (Ral Tir Tal Sol + Hel) do trade | r01–r19 |

Threshold: always pick up **Lem (r20) and above**. Ko, Fal, Hel, Io and Lum are worth taking on a twink or leveling character.

### Uniques
**S tier:**
- Annihilus (`cm1`)
- Hellfire Torch (`cm2`)
- Unique grand charms (`cm3`). Since RotW, Sunder Charms drop as *latent* uniques (`PreCrafted Cold Rupture/Flame Rift/Crack of the Heavens/Rotting Fissure/Bone Break/Black Cleft`) and are then crafted into `cs2` "Crafted Sunder Charm". Gheed's Fortune shares the `cm3` base. Patch 3.3 limited Latent Sunder drops to Hell difficulty.
- Harlequin Crest (`uap` Shako)
- Griffon's Eye (`ci3`)
- Stone of Jordan (`rin`, the second trade currency)
- Mara's Kaleidoscope and Highlord's Wrath (`amu`)
- Bul-Kathos' Wedding Band, Raven Frost and Wisp Projector (`rin`)
- Death's Fathom (`obf`)
- Death's Web (`7gw`)
- Crown of Ages (`urn`)
- Arachnid Mesh (`ulc`)
- Tyrael's Might (`uar`)
- Dracul's Grasp (`uvg`)
- Verdungo's Hearty Cord (`umc`)
- Nightwing's Veil (`uhm`)
- Stormshield (`uit`)
- Windforce (`6lw`)

**A tier:**
- Ethereal only: Andariel's Visage (`usk`), Reaper's Toll (`7s8`), Titan's Revenge (`ama`), Lacerator (`7b8`), Death Cleaver (`7wa`), Skullder's Ire (`xpl`).
- Mid value: War Traveler (`xtb`), Gore Rider (`xhb`), Thundergod's Vigor (`zhb`), and the +2 class uniques Herald of Zakarum (`pa9`), Arreat's Face (`baa`), Jalal's Mane (`dra`) and Homunculus (`nea`). Value depends on rolls for Eschuta's Temper (`obc`, needs +3), Rainbow Facets (`jew`, perfect rolls only) and Steelshade (`ulm`).
- RotW Colossal Jewels (`cjw`): Defender's Bile/Fire, Guardian's Thunder/Light, Protector's Frost/Stone. These are rewards for the Colossal Ancient fight.

**B tier:** build uniques with low to medium value, such as Vipermagi, Chance Guards, Magefist, Shaftstop, Goldwrap, Ormus', Nosferatu's, Leviathan, Arkaine's, Medusa's, Mang Song's, Ondal's, Kira's, Sandstorm/Marrowwalk/Waterwalk, Bloodraven's and Wizardspike. Some only sell with the right sockets or ethereal: Headhunter's Glory needs 3 sockets, Giant Skull 2 sockets, Tomb Reaver ethereal with 3 sockets, Sandstorm Trek ethereal. The new RotW uniques are also B: Ars Al'Diablolos/Tor'Baalos/Dul'Mephistos, Measured Wrath, Dreadfang, Wraithstep, Bloodpact Shard, Sling, Opalvein, Entropy Locket, Gheed's Wager, and the Warlock Death Mask unique. I found no price consensus for them yet, so they are highlighted to be noticed.

### Sets
- **A tier:**
  - Tal Rasha's Wrappings: the armor (`uth`) and amulet sell best.
  - Immortal King: the Stone Crusher only trades with 40% Crushing Blow.
  - Trang-Oul's Avatar: the Claws are used outside the set.
  - Laying of Hands (`ulg`).
- **B tier:** Griswold's (the Redemption needs 4 sockets), Natalya's, M'avina's, Aldur's, Bul-Kathos' Children, Guillaume's Face, Sazabi's.

### Materials and keys
- **A tier:**
  - Uber keys `pk1–pk3` (sold as 3x3 sets) and organs `dhn/bey/mbr`.
  - **RotW:** Colossal Ancient Statues `ua1–ua5`. You need one per act to open the Colossal Summit, and they drop from terrorized act bosses in Hell. Their drop rate was cut in 3.3.
  - Colossal upgrade materials `um1–um6`.
  - Worldstone Shards `xa1–xa5`, which drop in Terror Zones (drop rate cut in 3.3).
- **B tier:** Token of Absolution `toa` and its essences `tes/ceh/bet/fed`; perfect gems `gpv gpw gpr gpb gpy gpg skz`.

### Magic and rare items (simple rules only)
- **Jeweler's Monarch:** any magic `uit` with 4 sockets is necessarily the Jeweler's prefix. **A tier.** It is "very high" value with the "of Deflecting" suffix.
- **Socketed magic circlets:** magic circlets, coronets, tiaras and diadems with 2–3 sockets, e.g. Artisan's Tiara of Speed. **A tier**, needs stats.
- **Magic grand charms:** **A tier**, needs stats. The value is skillers, especially ones with 20+ life.
- **B tier, all need stats:**
  - Small charms: 5 all res, 20 life, 7 MF, 3/20/20.
  - Large charms: sharp ones with 5–6 max damage and 20–35 life.
  - Jewels: 40/15 IAS, 30–40 ED, 10–15 all res.
  - Rare rings, amulets, circlets and jewels.
  - Jeweler's Archon Plate or Dusk Shroud (of the Whale).
  - Class items with +3 or +6 skills: Paladin shields, orbs, claws, Barbarian helms, pelts, Necromancer heads, Amazon weapons.
- Not in the rules because they cannot be told apart without stats: gloves (Lancer's/Archer's/Kenshi's of Alacrity), Rings of Fortune with 31–40% MF, rare amulets with life.

---

## b) Twink and runeword bases

Category **trade** marks the bases worth keeping for trade without a build need (user, 2026-11-20); **twink** A bases are cheap or player-specific and the item plan (`src/stash_plan.rs`) keeps them only as a build's runeword base, else sells them. S bases are kept for trade either way.

All of these need a **white (normal or superior)** base. Superior means more defense or ED and is preferred for main-hand and armor bases. Use ethereal for **mercenary** gear and for runewords with *Indestructible* or *Repairs durability* (BotD, Death, Exile).

| tier | category | base (code) | sockets | eth | runewords |
|---|---|---|---|---|---|
| S | twink | Archon Plate `utp`, Dusk Shroud `uui` (superior) | 3 / 4 | – | Enigma (3), CoH / Fortitude (4) |
| A | trade | Other elite armor: Sacred Armor `uar`, Wyrmhide `uea`, Great Hauberk `urs`, Shadow Plate `uul`, Kraken `uld`, Hellforge `ult`, Balrog `upl`, Lacquered `uth` | 3–4 | – | Enigma, CoH, Fortitude |
| A | trade | Elite armor for the merc (eth Sacred Armor is best) | 4 | yes | Fortitude, CoH |
| S | twink | Thresher `7s8`, Giant Thresher `7wc`, Cryptic Axe `7pa`, Colossus Voulge `7vo`, Great Poleaxe `7h7`, Mancatcher `7br`, Matriarchal Spear `amd` | 4 | yes | Infinity, Insight (merc) |
| A | twink | Any polearm 4os, including Normal Voulge `vou`, Scythe `scy`, Poleaxe `pax`, Halberd `hal`, War Scythe `wsc` and the exceptional tier | 4 | preferred | Insight |
| A | trade | Elite polearms and spears (also Ghost Spear `7st`, War Pike `7p7`) | 5 | yes | Pride, Obedience, Doom |
| S | twink | Phase Blade `7cr` (superior) | 5 / 6 | n/a (no durability) | Grief (5), Last Wish / BotD (6) |
| A | trade | Berserker Axe `7wa`, Colossus Blade `7gd`, Thunder Maul `7gm`, Archon Staff `6ws`, War Pike `7p7`, Hydra Bow `6lw` | 5–6 | strongly | BotD (6), Death (5), Doom, Beast (5), Last Wish |
| A | twink | Crystal Sword `crs` / Dimensional `9cr` / Phase `7cr`, Long Sword `lsd/9ls`, Broad Sword `bsd/9bs` | 4 | – | Spirit sword, Phoenix, Hand of Justice |
| A | trade | Crystal Sword family, Flail `fla` / Knout `9fl` / Scourge `7fl` | 5 | style | Call to Arms |
| A | twink | Flail / Knout / Scourge | 4 | style | Heart of the Oak |
| A | trade | Monarch `uit` (superior) | 4 | – | Spirit shield, Phoenix |
| A | twink | Paladin shields `pa1–paf` (all tiers; value depends on the implicit res roll, needs stats) | 4 | – | Spirit, Phoenix |
| A | twink | Paladin shields, eth Sacred Targe `pab` / Vortex `paf` preferred | 4 | yes | Exile |
| A | trade | Grand Matron Bow `amc`, Matriarchal Bow `amb` | 4 | – | Faith, Insight |
| B | twink | Amazon bows `am1 am2 am6 am7 amb amc` | 5 | – | Mist |
| B | twink | Bone Visage `uh9`, Diadem `ci3`, Tiara `ci2`, Troll Nest `ush` | 3 | – | Dream, Coven (RotW), Flickering Flame, Metamorphosis |
| B | twink | Barbarian helms / Druid pelts with +3 skills (needs stats) | 2–3 | – | Dream, Lore, Metamorphosis, Coven |
| B | twink | Mage Plate `xtp` | 3 | eth for merc Treachery | Treachery, Duress, Smoke, Bone, Hustle, Authority (RotW) |
| B | twink | Quilted / Leather / Hard Leather / Studded (2os), Ring Mail / Breast Plate (3os) | 2–3 | – | Stealth; Smoke / Treachery / Authority twink |
| B | twink | Kite `kit`, Large `lrg`, Dragon `xit` shield and the exceptional Large Shield | 3 | – | Ancient's Pledge |
| B | twink | 2os helms (cap, skull cap, helm, full helm, war hat, Shako, Hydraskull, circlets) | 2 | – | Lore |
| B | twink | Claws `7lw 7tw 9lw clw` (+3 implicits need stats) | 3 | – | Mosaic, Plague |
| B | twink | Phase / Crystal / Legend Sword `72h` | 3 | – | Lawbringer, Crescent Moon |
| B | twink | Daggers `kri bld 9kr 9bl 7kr 7bl` | 3 | – | Void, Ritual (RotW Warlock) |
| B | twink | Grimoires `wa1–waf` | 2 | – | Vigilance (RotW) |

RotW runeword recipes, from d2data `runes.json`:
- Authority = Hel Shael Ral (body armor)
- Coven = Ist Ral Io (helm)
- Void = Thul Zod Ist (dagger)
- Ritual = Amn Shael Ohm (dagger)
- Vigilance = Dol Gul (grimoire, shield, Necromancer head, Paladin shield)

Patch 3.3 moved Mania, Hysteria, Metamorphosis, Ground, Temper, Hearth, Cure and Bulwark to Non-Ladder and replaced them with new Ladder-only runewords. Those new ones are not covered here; see the caveats.

---

## Caveats and open questions
1. **There is no live price data.** The tiers are pickup consensus. The maxroll unique/set and magic-item value pages were last updated in 2024 and have not been re-rated for RotW. Early in a ladder season, everything in B is worth more.
2. **The RotW items have no price consensus yet:** the new uniques, Colossal Jewels, statues, shards and the new runewords. Statue and shard tiers are my judgement, based on how scarce they are and the 3.3 drop-rate nerfs.
3. **`Warlord's Glory`** is a set in d2data (lvl 100, on Gothic-tier bases: Plated Belt, Gauntlets, Greaves, Great Helm, Full Plate). I could not confirm what it is or what it is worth, so it is left out of the rules.
4. **Patch 3.3 ladder-only runewords.** The replacements for Mania, Hysteria, Ground, Temper and the others were not researched, so a newly ladder-only runeword may need a base this file does not cover.
5. **These need stats, not just code, quality, sockets and ethereal:**
   - superior ED or defense %
   - implicit res on Paladin shields
   - +skills on class items
   - charm, jewel and rare affixes
   - perfect rolls on Annihilus, Torch, Facets and Sunders
   - 40% Crushing Blow on the IK Stone Crusher
   - item level, which decides how many sockets Larzuk or the cube can add to an unsocketed base
6. **Unsocketed bases** (other than the eth Colossus Voulge rule) are not in the rules. Superior or ethereal 0os elite bases (Monarch, Archon, Dusk, eth Sacred Armor, eth Thresher) also have value once socketed.
7. **D2R's built-in loot filter** (added in RotW) cannot filter on sockets, ilvl or stats (Icy Veins). That gap is exactly what the overlay rules fill.

## Sources
- Maxroll, Runewords (updated 2026-06-02): https://maxroll.gg/d2/items/runewords
- Maxroll, Trade Guide (updated 2026-02-10): https://maxroll.gg/d2/resources/trade-guide
- Maxroll, Runes (updated 2026-02-10): https://maxroll.gg/d2/items/runes
- Maxroll, Valuable Unique and Set Items (updated 2024-03-06): https://maxroll.gg/d2/items/valuable-unique-set-items
- Maxroll, Valuable Magic Items (updated 2024-03-06): https://maxroll.gg/d2/items/valuable-magic-items
- DiabloBytes, Patch 3.0 notes (2026-02-11): https://www.diablobytes.com/d2-resurrected/news/patch-notes-3-0/
- ggseason.com (formerly seasontimer.live), patch notes 3.2 / 3.3: https://ggseason.com/d2r/patch-notes/
- mp1st, Patch 3.3 (2026-08-18): https://mp1st.com/title-updates-and-patches/diablo-2-resurrected-update-3-3-released-august-18-patch-1-39-1-039
- Icy Veins, Loot Filter: https://www.icy-veins.com/d2/loot-filter (limits as quoted in search results; the page itself returned 403)
- itemd2r.com economy guide (low trust, seller blog): https://www.itemd2r.com/blog/d2r/diablo-2-resurrected-economy-items-worth-keeping-trading
- d2db runeword guide (white bases only): https://d2db.net/runeword-guide
- Item data: https://github.com/blizzhackers/d2data (json/weapons, armor, misc, uniqueitems, setitems, runes, itemtypes, allstrings-eng)
