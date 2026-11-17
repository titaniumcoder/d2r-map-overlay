# D2R shared stash: what is on disk here, and how to read stash items at all

**Status:** research note, 2026-11-17. Verified against this machine's files and three
independent community parsers. No repo code changed.

**Bottom line:** the account in play is **Battle.net Online**, so shared-stash *contents* are
server-side and there is **nothing item-bearing on disk**. The only stash file in the prefix is a
556-byte, 7-tab, all-empty template. The offline `.d2i` format below is fully documented (needed
the moment the user plays `Wolfie` offline, and it is the only format we can parse *exactly*), but
for an online session the stash must come from **process memory**, which is what `PLAN.md` has
already been probing.

---

## 1. What is on disk on this machine

Save directory (verified: reading `Settings.json` in it returns real D2R settings JSON):

```
~/Games/battlenet/prefix/drive_c/users/steamuser/Saved Games/Diablo II Resurrected/
```

`drive_c/users/titaniumcoder/...` resolves to the same directory (Proton/umu user-dir alias), so
either spelling works; `steamuser` is the one the launcher's `WINEPREFIX` actually creates
(`~/.local/bin/battlenet` sets `WINEPREFIX=$HOME/Games/battlenet/prefix`).

Directory contents (supervisor's `ls`, cross-checked by direct file probes):

| File | Meaning |
|---|---|
| `BigWolf223396301.ctlo/.keyo`, `Vex223650631.*`, `Demonmaster224000229.*` | **Online** characters (`.ctlo/.keyo` = client token/key; no `.d2s`) |
| `Wolfie.d2s`, `Wolfie.ma0`, `Wolfie.ctl` (Nov 14) | one **offline** character |
| `Settings.json`, loot filter files | settings |
| **`ModernSharedStashSoftCoreV2.d2i` — 556 bytes** | shared stash, **empty** (analysed in §2) |

Absent (probed by path): `SharedStashSoftCoreV2.d2i`, `SharedStashHardCoreV2.d2i`,
`BigWolf.d2s`, `Vex.d2s`, `Demonmaster.d2s`.

Two file names exist in the wild for the *same* container format:

* `SharedStashSoftCoreV2.d2i` / `SharedStashHardCoreV2.d2i` — classic D2R (2.x … 3.0) naming.
  D2SSharp ships both `Resources/99/SharedStashSoftCoreV2.d2i` and
  `Resources/105/SharedStashSoftCoreV2.d2i` plus `Resources/105/ModernSharedStashSoftCoreV2.d2i`,
  i.e. the `Modern…` prefix appears with the v105 (RotW) format.
* `ModernSharedStashSoftCoreV2.d2i` — what this install actually writes.

**The personal stash is not a separate file.** It lives inside the character's `.d2s` item list with
`StorePage = 4` (stash). Only the *shared* tabs are in the `.d2i`.

---

## 2. The stub we do have: `ModernSharedStashSoftCoreV2.d2i` (556 bytes)

Hand-decoded from the actual bytes (`read` of the file; the tool returns raw bytes 1:1):

```
556 bytes = 5 x 68  (Normal tabs)  +  1 x 68 (AdvancedStash tab)  +  1 x 148 (Chronicle tab)
```

Every tab starts with magic `55 AA 55 AA`, `StashFormat = 2`, `SaveVersion = 0x69 = 105`,
`Gold = 0`, `Size = 0x44 = 68`, `Season = 0`, then `TabType`:

| # | `+0x14` TabType | Size | Body |
|---|---|---|---|
| 0–4 | 0 (`Normal`) | 68 | `4A 4D 00 00` = `"JM"` + count `0` → no items |
| 5 | 1 (`AdvancedStash`) | 68 | `"JM"` + `0` → no items |
| 6 | 2 (`Chronicle`) | 148 | `C0 EA ED C0` magic, version `1`, three zero counts, 8 reserved zero bytes, then **64 bytes of stale buffer** (`FF FF 00 00` ×5, etc.) |

That is *exactly* the layout D2SSharp describes (64-byte tab header, `ChronicleSection` body =
4 + 2 + 3×2 + 8 bytes + 10-byte entries + trailing), and its test suite explicitly warns that the
game's chronicle writer appends 64 bytes of stale memory because of a size bug
(`add ax, 40h` at `0x140311e98`) — our 148-byte chronicle tab (64 + 20 + 64) confirms it.

**What this file can tell you:** tab count (7), tab types, per-tab gold (all 0). Nothing about
items, because an online account's stash never reaches disk. It is a template the client rewrites
(its mtime is current). **Parsing it is worth ~nothing for the overlay** — 30 lines of code, zero
item data. It is however a perfect zero-risk unit-test fixture for the container parser (§6).

It also proves the container is *not* what the item format needs for our goal: the whole
"header + first page" question collapses to "there are no items here".

---

## 3. Primary path: stash items from D2R process memory (online session)

### 3.1 What this repo already established (live, direct evidence)

From `PLAN.md` (2026-11-15 and 2026-11-17 sessions, all live-verified):

| Fact | Value / note |
|---|---|
| Item units | unit hash-table block **4**; slot `id & 127`; chain next at **`+0x158`** |
| `ItemData` | `unit + 0x10` |
| Quality | `ItemData + 0x00` — 1 inferior, 2 normal, 3 superior, 4 magic, 5 set, 6 rare, 7 unique (5/7 verified live) |
| Flags | `ItemData + 0x18` — `0x10` identified, `0x400000` ethereal, `0x4000000` runeword |
| Unique/set row | `ItemData + 0x34` — **`*ID` column** value; only filled once identified (Cleglaw's Claw read 0 while unidentified, `set *ID 7` after) |
| Body slot / page | `ItemData + 0x54` body slot, `ItemData + 0x55` page: **0 inventory, 3 cube, 4 stash, 255 elsewhere** (verified over 74 items) |
| Stats | unit `+0x88` → base list; **magic affixes on the chained list at `StatList + 0xD0`** |
| **Shared stash tabs** | "Shared stash pages are **player-type units without an act, one per page**; the shared ones of the current character have `Inventory(+0x90) + 0x48` = the player's id. Other tab units in memory are stale (earlier characters, offline shared stash) and must be ignored (verified by moving Cathan's Seal through pages 1–5)." |
| Personal stash | items owned by the player id, `page == 4` |

So the in-memory model is: **each shared tab is a `Player`-like unit whose `Inventory` holds the
items**, and items are ordinary type-4 units with `page == 4` and an owner (tab unit id) — not a
flat global "stash list". Eligibility filter = `Inventory(+0x90)+0x48 == player unit id`.

### 3.2 Community corroboration (and how thin it is)

* **`soarqin/D2RMH`** (C++, reads D2R memory over `ReadProcessMemory`, no injection) is the closest
  analogue to this project. Its source tree has `src/d2r/d2rdefs.h` (19 KB of unit/level structs),
  `processdata.cpp`, `mapstructs.h` — but a case-insensitive search of the whole tree for
  `inventory` returns **no match**: it models levels/units/rooms/presets and never touches
  inventory or stash. *Conclusion (interpretation):* the D2R memory-reading community that
  publishes code does **not** have a documented, reusable stash-unit layout; there is no upstream
  spec to port. Confidence: medium-high (absence of evidence in the one large public D2R reader).
* **Classic D2 (1.13, `d2bs`/`bh`/`kolbot`) has the analogue** — items of the player's stash are
  reachable through the player unit's `Inventory` grid structs, and bots must open the stash window
  before `getItemsInStash()` returns anything. I could **not** fetch the two `d2bs`/`bh` source
  files I wanted (search engine returned nothing usable for those queries), so treat the
  "grids live in `Inventory`" statement as **inference by analogy**, not a cited fact.
* **`axiom0x0.sh` D2R memory-trainer series (Linux/Proton)** is a good independent sanity source for
  *how* to read a Proton-hosted D2R from Linux (memfd `wine-mapping` regions, `process_vm_readv`,
  pattern scanning, `/proc/<pid>/mem` writes silently failing). It confirms
  **"D2R online is server-authoritative"** and that all its memory editing targets offline play.
  It does not cover inventory/stash. *Direct evidence for the server-authoritative claim.*
* **`OneXDeveloper/MapAssist`** was the other candidate reader (it does decode item units); the
  GitHub tree API 404'd on the paths I tried, so it is **not** cited here.

### 3.3 Practical strategy for the Rust reader (researcher inference)

1. Keep the existing unit-hash-table scan (`block 4`, `+0x158` chain) and the `ItemData` offsets —
   they already give quality, code (`+0x04 txt` is the *txt id*, the 4-char code needs the items
   table the reader already has) and unique/set `*ID`.
2. Enumerate **block 0 (players)** for units with `act == 0` and a non-null `Inventory(+0x90)`,
   read `Inventory + 0x48`. Keep as "live tabs" only those whose `+0x48` equals the real player's
   unit id — `PLAN.md` already found this the discriminator, and it survived moving an item through
   pages 1–5.
3. Attribute each `page == 4` item to a tab via its owner/tab-unit id, so the overlay can say
   "tab 3" and show 7 tabs' contents at once without the UI being open (see 3.4).
4. Because stale tab units from earlier characters linger, tag tabs by `(player id, tab unit id)`
   and re-validate every poll — do not cache tab pointers across games.
5. Cheap validation: the tab-unit's grid should contain exactly the items you attributed to it;
   a mismatch means you are looking at a stale tab.

### 3.4 Unknown: are stash tabs in memory when the stash UI is closed?

No community source states it, and `PLAN.md` does not record whether the tabs were observed with
the stash closed. Two of our own data points suggest *loaded*: the tab units were described as
present and "stale ones must be ignored", which implies several tab units coexist at once; and the
items are ordinary units, so they are in the hash table whenever the client has them.

Open question to test (one game, stash untouched for 10 s, then opened):
count player-units with `act == 0` and `Inventory+0x48 == player id` at both moments. If the count
goes 0 → 7 on open, the overlay must ask the user to open the stash once.

---

## 4. When does the game write the stash file?

| Claim | Evidence | Confidence |
|---|---|---|
| D2R keeps save/character files **open** while running, so external tools must close the game before writing | GoMule-d2r README: "Close D2R when using GoMule. D2R keeps character data open since it displays character gear in the menus and this prevents GoMule from saving files and moving items properly." | high (direct) |
| Online characters/stashes never reach disk | This machine: only `.ctlo/.keyo` for online chars, `.d2i` stub with 0 items; `axiom0x0`: online is server-authoritative | high |
| Single-player saves are written on explicit save/exit rather than continuously | Community guidance is consistently "quit the game, then copy the save folder to back up"; no Blizzard statement found. Reported "lost shared stash" threads are consistent with non-continuous writes but are anecdotal | **low** — no primary source found |
| You can parse `.d2i` mid-session and see recent changes | Not answerable for online (no file). For offline: unknown flush cadence; treat as stale-until-exit | low |

Practical rule for us: **never** rely on the `.d2i` for a live session. Read memory; the file is for
offline bookkeeping only.

---

## 5. Appendix A — offline `.d2i` item format (complete spec, for `Wolfie`-style play)

Container (verified by three independent implementations + our own 556-byte file):

```
file := tab*                       # NO file-level header; offset 0 is tab 0
tab  := header[64] body[size-64]
header (little-endian):
  0x00 u32  magic        0xAA55AA55
  0x04 u32  stashFormat  1 = D2R 2.x (v97..99), 2 = D2R 3.x (v100+)
  0x08 u32  saveVersion  97..105 — also the item bitstream version of this tab
  0x0C u32  gold         gold in THIS tab
  0x10 u16  size         total tab size in bytes (header included)
  0x12 u16  season       season number, 0 = non-seasonal
  0x14 u8   tabType      0 Normal, 1 AdvancedStash (stackables), 2 Chronicle
  0x15 [3]  pad          0
  0x18 [20] withdrawOnly (first u32 != 0 → withdraw-only tab; game never creates it)
  0x2C [20] pad          0
body:
  tabType 0/1: "JM" (4A 4D) + u16 rootItemCount + items
  tabType 2  : 0xC0EAEDC0 + u16 version(1) + u16 setCount + u16 uniqCount + u16 rwCount
               + 8 reserved + entries[10] (u32 id, u16 source, u32 timestamp) + trailing junk
```

Cross-checks: magic `55 AA 55 AA` ≈ u32 `0xAA55AA55` — *dschu012/d2s* (`src/d2/stash.ts`) and
GoMule-d2r (`src/gomule/d2s/D2SharedStash.java`) both branch on `0xaa55aa55` and then read
`version` at 8, `gold` at 12, "size of sector" at 16 + 44 bytes of padding = 64-byte header.
GoMule notes "`JM` at byte pos 64 for 1st stash tab/item block".

Item bitstream (D2R, i.e. `saveVersion > 96`). All fields LSB-first, no per-item `"JM"` (that
header only exists for v ≤ 96 — GoMule: "D2R d2s file doesn't have JM for each item, just 1 JM at
stat and 1 JM at end of item list"). Each item is **byte-aligned at its end**; socketed items follow
their parent immediately.

```
u32 flags          0x10 identified, 0x800 socketed, 0x10000 ear, 0x20000 starter,
                   0x200000 compact/simple, 0x400000 ethereal, 0x1000000 personalized,
                   0x4000000 runeword, 0x10000000 chronicle   (low bit → high)
3   itemFormat     v<=96: 10 bits; D2R: 1 bit + 2 bits (high → +99). Read & ignore.
3   mode           0 stored, 1 equipped, 2 belt, 3 ground, 4 cursor, 5 dropping, 6 socketed
  mode 3/5:  16 x, 16 y
  else:      4 bodyLoc, 4 invX, 4 invY, 3 (page+1; 0 = none, 5 = stash → page 4)
--- compact items (flag 0x200000) stop here, after:
  possible gold (code "gld": 1 bit big, then 32 or 12 bits, then 1 bit playerGold),
  possible quest-difficulty stat, 1 bit hasRealmData (+4 x u32 if set), v>=102: 1 bit + 8 bits stack
--- complete items continue:
4-char item code   Huffman (D2R) / 32 raw bits (v<=96)
3   socketedItemCount
32  itemSeed
7   itemLevel
4   quality        1 inferior, 2 normal, 3 superior, 4 magic, 5 set, 6 rare, 7 unique,
                   8 craft, 9 tempered
1   hasVariableGfx (+3 id)
1   hasAutoAffix   (+11 id, + automagic offset)
quality data:
  inferior  3   fileIndex
  superior  3   fileIndex
  magic     11  prefixId, 11 suffixId
  set       12  setItemID          -> setitems.txt *ID
  unique    12  uniqueID           -> uniqueitems.txt *ID
  rare/craft  8 rarePrefix, 8 rareSuffix, then 3 x (1 bit + 11 prefix [+offset], 1 bit + 11 suffix)
16  runewordID (if flag 0x4000000)  — d2itemreader/dschu012: 12-bit id + 4 unknown;
                                     D2SSharp reads 16. Same bit count, so no desync; use low 12.
str personalized name (if flag 0x1000000): D2R 8-bit chars, NUL-terminated; pre-98 7-bit
1 bit hasRealmData + 4 x u32 (v>96; always 0 in single-player → just the 0 bit)
armor:  AC (ItemStatCost armorclass saveBits), maxDur, [curDur if maxDur>0]
weapon: maxDur, [curDur if maxDur>0]
gold:   1 bit big + 32 or 12 bits + 1 bit playerGold
v>=105: 1 bit hasQuantity + 9 bits
v<=104: quantity (9 bits) only for stackable codes
1+4   socket count (if flag 0x800)
5     setItemMask (set items only)
stat list (see below)
5 x stat list (one per set bit of setItemMask)
stat list (runeword stats, if flag 0x4000000)
chronicle fields (v>=100)
v>=102: 1 bit + 8 bits advanced-stash stack size
```

Stat list: repeat { `9` statID; if statID == 0x1FF stop; `saveParamBits` param (ItemStatCost.txt);
`saveBits` value; value = (raw − saveAdd) << valShift }. A few stats are *paired* (the follower's
value is written without its own id — e.g. poison damage), so `ItemStatCost` data is mandatory for
a correct walk. `ItemStatCost.txt`/`.json` (or D2SSharp `Data/Txt/105/itemstatcost.txt`) supplies
`SaveBits/SaveParamBits/SaveAdd/ValShift`.

**How names are obtained:** the save file contains **no name table**. `uniqueID` and `setItemID`
are indices into the static tables:
`uniqueitems.txt` row whose `*ID` equals the value, and `setitems.txt` row whose `*ID` equals the
value (dschu012: `item.unique_name = constants.unq_items[item.unique_id].n`). The `index` key is
internal ("Piercerib"); the in-game English name comes from `allstrings-eng.json`. **This repo
already has that mapping** — `src/item_names_data.rs` (`UNIQUE_NAMES`, `SET_NAMES` keyed by `*ID`)
plus `DISPLAY_NAMES`, generated by `research/gen_item_names.py` from `blizzhackers/d2data`; the same
`*ID` is what the *memory* reader reads at `ItemData + 0x34`. So item naming needs **no new tables**
and works identically for the file and memory paths. Runewords: determine from the socketed runes,
not from `runewordID` (d2itemreader's own warning).

**Base type / item code** comes from the 4-char code in the record (`ItemCode >> …`, space-padded);
map code → `armor.txt` / `weapons.txt` / `misc.txt` row for "base type". The repo's items table
already maps txt ids ↔ 4-char codes.

---

## 6. Appendix B — minimal Python sketch (tab loop + item headers)

Not executed here (this subagent has no shell). It is derived line-by-line from D2SSharp
(`D2StashTab.cs`, `Item.cs`, `ItemsSection.cs`) and dschu012/d2s (`src/d2/stash.ts`,
`src/d2/items.ts`), which agree on every field width except the two noted in §5. The `size` field
gives a hard integrity check: after parsing a tab, the bit reader must land exactly on
`tabStart + size` — if it does not, the item walk desynced (this is the cheapest possible test and
worth porting to Rust first).

With no `itemstatcost.json` given, it prints tab headers only (which is all the 556-byte stub can
prove). With the json it walks item headers and stops the tab on the first desync.

```python
#!/usr/bin/env python3
"""Read a D2R shared stash (.d2i): tabs, then item code/quality/unique-set id per tab.

  python3 -I stash_read.py ModernSharedStashSoftCoreV2.d2i [itemstatcost.json]

Layout sources: D2SSharp (C#, Model/D2StashTab.cs + Item.cs) and dschu012/d2s
(TS, src/d2/stash.ts + items.ts). See research/notes/stash-format.md.
"""
import json
import struct
import sys

MAGIC = 0xAA55AA55
TABTYPES = {0: "Normal", 1: "AdvancedStash", 2: "Chronicle"}
QUALITY = {1: "inferior", 2: "normal", 3: "superior", 4: "magic",
           5: "set", 6: "rare", 7: "unique", 8: "craft", 9: "tempered"}
# item flags (u32, D2R)
F_IDENT, F_SOCKET, F_EAR, F_COMPACT = 0x10, 0x800, 0x10000, 0x200000
F_ETH, F_PERS, F_RW = 0x400000, 0x1000000, 0x4000000

# item code Huffman table, (value, bits) with bits read LSB-first (dschu012/d2s)
HUFF = {" ": (1, 2), "0": (223, 8), "1": (31, 7), "2": (12, 6), "3": (91, 7),
        "4": (95, 8), "5": (104, 8), "6": (123, 7), "7": (30, 5), "8": (8, 6),
        "9": (14, 5), "a": (15, 5), "b": (10, 4), "c": (2, 5), "d": (35, 6),
        "e": (3, 6), "f": (50, 6), "g": (11, 5), "h": (24, 5), "i": (63, 7),
        "j": (232, 9), "k": (18, 6), "l": (23, 5), "m": (22, 5), "n": (44, 6),
        "o": (127, 7), "p": (19, 5), "q": (155, 8), "r": (7, 5), "s": (4, 4),
        "t": (6, 5), "u": (16, 5), "v": (59, 7), "w": (0, 5), "x": (28, 5),
        "y": (40, 7), "z": (27, 8)}
HDEC = {(ln, v): ch for ch, (v, ln) in HUFF.items()}


class Bits:
    def __init__(self, buf, byte_off=0):
        self.b, self.bit = buf, byte_off * 8

    def u(self, n):
        v = 0
        for i in range(n):
            v |= ((self.b[self.bit >> 3] >> (self.bit & 7)) & 1) << i
            self.bit += 1
        return v

    def align(self):
        self.bit = (self.bit + 7) & ~7

    @property
    def byte(self):
        return self.bit >> 3


def item_code(br):
    v = 0
    for ln in range(1, 10):
        v |= br.u(1) << (ln - 1)
        if (ln, v) in HDEC:
            return HDEC[(ln, v)]
    raise ValueError("bad huffman code")


def skip_stats(br, st):
    while True:
        sid = br.u(9)
        if sid == 0x1FF:
            return
        info = st.get(sid)
        if info is None:
            raise KeyError("stat %d not in itemstatcost" % sid)
        if info["sp"]:
            br.u(info["sp"])
        br.u(info["sb"])


def read_item(br, ver, st, depth=0):
    flags = br.u(32)
    comp = bool(flags & F_COMPACT)
    # item format version: 1 bit + 2 bits on D2R, 10 bits pre-D2R
    if ver > 96:
        hi, lo = br.u(1), br.u(2)
        iv = lo + 99 if hi else lo
    else:
        iv = br.u(10)
    mode = br.u(3)
    if mode in (3, 5):
        pos = {"x": br.u(16), "y": br.u(16)}
    else:
        bl, x, y, page = br.u(4), br.u(4), br.u(4), br.u(3)
        pos = {"body": bl, "x": x, "y": y, "page": page - 1 if page else None}
    it = {"flags": flags, "ver": iv, "mode": mode, "pos": pos, "sockets": []}

    if flags & F_EAR:                      # ear: no code
        br.u(3), br.u(7)
        while br.u(8):
            pass
        br.align()
        return it

    it["code"] = "".join(item_code(br) for _ in range(4)).rstrip()

    if comp:                              # simple item: stops here
        if it["code"] == "gld":
            big = br.u(1)
            it["gold"] = br.u(32) if big else br.u(12)
            br.u(1)                              # playerGold
        if br.u(1):                             # hasRealmData
            [br.u(32) for _ in range(4)]
        if ver >= 102:
            it["stack"] = br.u(8) if br.u(1) else None
        br.align()
        return it

    nsock = br.u(3)
    it["seed"] = br.u(32)
    it["ilvl"] = max(br.u(7), 1)
    it["quality"] = QUALITY.get(br.u(4), "?")
    if br.u(1):
        br.u(3)                                 # variable gfx id
    if br.u(1):
        br.u(11)                                # automagic id (+ offset)
    q = it["quality"]
    if q == "inferior" or q == "superior":
        br.u(3)
    elif q == "magic":
        br.u(11), br.u(11)
    elif q in ("set", "unique"):
        it["set_unique_id"] = br.u(12)          # *ID in setitems.txt / uniqueitems.txt
    elif q in ("rare", "craft"):
        br.u(8), br.u(8)
        for _ in range(3):
            if br.u(1):
                br.u(11)
            if br.u(1):
                br.u(11)
    elif q == "tempered":
        br.u(8), br.u(8)                        # rare name prefix/suffix
    if flags & F_RW:
        it["runeword_low12"] = br.u(16) & 0x0FFF
    if flags & F_PERS:
        while br.u(8):
            pass
    if br.u(1):                                 # hasRealmData
        [br.u(32) for _ in range(4)]
    if st is None:                              # can't continue without stat widths
        raise SystemExit("need itemstatcost.json past the item header "
                         "(item %r)" % (it,))

    def dur(width, add):
        return br.u(width) - add

    # durability / quantity widths come from ItemStatCost; use the canonical D2R widths
    # (armorclass 11/10, maxdurability 9/0, durability 9/0, quantity 9/0/1) — override via json.
    if q != "?" and it["ilvl"]:
        pass
    # NOTE: armor-vs-weapon test needs items.txt; the safe superset is: skip nothing here and
    # rely on the tab size check to catch a wrong guess.
    if flags & F_SOCKET:
        it["total_sockets"] = br.u(4)           # ItemStatCost numSockets saveBits
    if q == "set":
        it["set_mask"] = br.u(5)
    skip_stats(br, st)
    if q == "set":
        for i in range(5):
            if (it.get("set_mask", 0) >> i) & 1:
                skip_stats(br, st)
    if flags & F_RW:
        skip_stats(br, st)
    br.align()
    for _ in range(nsock):
        it["sockets"].append(read_item(br, ver, st, depth + 1))
    return it


def read_tab(buf, off, st):
    magic, sfmt, ver, gold = struct.unpack_from("<IIII", buf, off)
    if magic != MAGIC:
        raise ValueError("bad tab magic 0x%08X at %d" % (magic, off))
    size, season = struct.unpack_from("<HH", buf, off + 0x10)
    ttype = buf[off + 0x14]
    if size < 64 or off + size > len(buf):
        raise ValueError("bad tab size %d at %d" % (size, off))
    tab = {"off": off, "format": sfmt, "ver": ver, "gold": gold,
           "size": size, "season": season, "type": ttype, "items": []}
    if st is not None and ttype in (0, 1):      # Chronicle tabs have no item list
        br = Bits(buf, off + 64)
        if br.u(16) != 0x4D4A:                  # "JM"
            raise ValueError("missing JM at tab %d" % off)
        n = br.u(16)
        for _ in range(n):
            tab["items"].append(read_item(br, ver, st))
        br.align()
        tab["balance"] = br.byte - (off + size)  # 0 == clean parse
    return tab


def main():
    buf = open(sys.argv[1], "rb").read()
    st = None
    if len(sys.argv) > 2:
        raw = json.load(open(sys.argv[2]))
        rows = raw.values() if isinstance(raw, dict) else raw
        st = {}
        for i, r in enumerate(rows):
            st[int(r.get("*ID", i))] = {"sb": int(r["Save Bits"]),
                                        "sa": int(r.get("Save Add", 0)),
                                        "sp": int(r.get("Save Param Bits", 0))}
    off = 0
    while off + 64 <= len(buf):
        tab = read_tab(buf, off, st)
        print("tab %d  off=0x%04X size=%d ver=%d format=%d type=%s gold=%d items=%d%s"
              % (len(sys.argv), tab["off"], tab["size"], tab["ver"], tab["format"],
                 TABTYPES.get(tab["type"], tab["type"]), tab["gold"], len(tab["items"]),
                 "" if tab.get("balance") in (None, 0) else "  DESYNC %d bytes"
                 % tab["balance"]))
        for it in tab["items"]:
            print("   %-4s %-8s ilvl=%d%s" % (it["code"], it["quality"], it["ilvl"],
                  "  id=%d" % it["set_unique_id"] if "set_unique_id" in it else ""))
        off += tab["size"]
    if off != len(buf):
        print("trailing %d bytes" % (len(buf) - off))


if __name__ == "__main__":
    main()
```

**Ground truth for the one fixture that exists** (`ModernSharedStashSoftCoreV2.d2i`, 556 bytes):
the table-only run must print 7 tabs — five `Normal`, one `AdvancedStash`, one `Chronicle` — all
`size=68/68/148`, `ver=105`, `format=2`, `gold=0`, `items=0`, and 0 trailing bytes. The chronicle
tab stops the item walk by construction, so `balance` is never computed there.

---

## 7. Existing parsers (port/call candidates)

| Language | Project | Covers `.d2i`? | Items? | Use |
|---|---|---|---|---|
| C# | **ResurrectedTrader/D2SSharp** (MIT) | yes, v97–v105, chronicle + advanced tabs, byte-exact round-trip tested on real files | yes, full (stats, sockets, runewords, set bonuses) | **reference implementation** — clearest offsets, names the C++ functions (`ITEMS_DecodeItemBitstream`) |
| TypeScript | **dschu012/d2s** (MIT) | yes (`src/d2/stash.ts`) | yes incl. unique/set **names** via generated data | reference + independent confirmation; ships `examples/stash/*.d2i` fixtures for v96/97/99 |
| Java | **pairofdocs/gomule-d2r** (GPL) | yes (`D2SharedStash.java`), **3 tabs hardcoded** (`NUM_SHARED_TABS = 3`) | yes | historic; confirms "no per-item JM in D2R" |
| Rust | **`halbu`** crate (docs.rs) | **no** — `.d2s` v99/v105 only, and its `items` module is documented as *"Item section placeholder/raw-preserving support"* | no item decoding | not usable for items; only interesting for its v105 header/format-enum handling |
| Python | — | none found | — | nothing to call; the sketch above is the starting point |

No Rust crate parses `.d2i` **or** decodes D2R item bitstreams as of this research (searched
crates.io/docs.rs/lib.rs and the D2R tool repos above). A port is required for the file path.

---

## 8. Contradictions and open evidence

1. **3-bit item format version.** D2SSharp: `1 bit (high) + 2 bits` with `high → value + 99`.
   dschu012: writes/reads 3 raw bits. Same field width, different interpretation; the value is
   irrelevant to us — read 3 bits and ignore. (Both agree it replaces the 1.09–1.14
   `8 bits version + 2 unknown`.)
2. **Runeword id width.** d2itemreader (docs) and dschu012: `12 bits id + 4 unknown`.
   D2SSharp: `16 bits`. Identical bit count, so no desync — take the low 12.
3. **Realm data.** D2SSharp reads `1 bit hasRealmData` then `4 x u32` for v > 96; dschu012 reads
   only the 1 bit (never the dwords). Compatible only because single-player items always have 0
   there. If a file ever has the bit set, dschu012-style parsers desync — D2SSharp's version is the
   safe one.
4. **`+0x04` in the tab header.** dschu012 names it `hardcore`; D2SSharp and our own file show it is
   `stashFormat` (1 for v97–99, **2** for v100+). GoMule ignores it entirely. Trust `stashFormat`
   (our fixture reads 2 with version 105).
5. **Stash items in memory (community silence).** `PLAN.md`'s observation that shared tabs are
   player-type units with `Inventory+0x48` is a **first-party** finding; no public D2R reader
   models it. Not a contradiction — a gap (§3.2).

## 9. Missing evidence

* Whether stash-tab units exist in memory **without** opening the stash (test in §3.4).
* D2R's offline flush cadence (no primary source; only "quit before copying saves" folklore).
* Whether the `Modern…` file name is version-dependent or account-dependent (D2SSharp ships both
  names for v105; no doc explains the switch).
* No `.d2i` **containing items** exists on this machine, so the item walk in §6 is untested against
  a real item-bearing stash — the 556-byte stub only exercises the tab loop.

## 10. Sources

Kept:
* D2SSharp — `Model/D2StashTab.cs`, `Model/Item.cs`, `Model/ItemsSection.cs`,
  `Model/ItemQualityData.cs`, `Model/ChronicleSection.cs`, `Enums/{ItemFlags,ItemMode,StorePage,
  StashTabType,ItemQuality}.cs`, `HuffmanEncoding.cs`, `Tests/IntegrationTests.cs` —
  https://github.com/ResurrectedTrader/D2SSharp — primary: reverse-engineered C++ item/stash
  decode + byte-exact round-trip tests on real v105 stash (11 tabs: 9 normal, 1 advanced, 1 chronicle).
* dschu012/d2s — `src/d2/stash.ts`, `src/d2/items.ts`, `tests/d2/stash.spec.ts`,
  `examples/stash/*.d2i` — https://github.com/dschu012/d2s — independent confirmation of the D2R tab
  header and the Huffman item codes (test: pageCount 3, sharedGold 2500000, byte-identical rewrite).
* pairofdocs/gomule-d2r — `src/gomule/d2s/D2SharedStash.java` + README —
  https://github.com/pairofdocs/gomule-d2r — third implementation; "no per-item JM in D2R"; "close
  D2R, it keeps character files open".
* squeek502/d2itemreader — https://squeek502.github.io/d2itemreader/formats/d2.html — authoritative
  description of the 1.09–1.14 item bit layout (field names/bit offsets that D2R kept) and the
  runeword/name caveats.
* pairofdocs/atma-stash-d2r README — https://github.com/pairofdocs/atma-stash-d2r — `.d2x`/`.sss`
  headers (contrast only; `.sss` is PlugY, **not** D2R).
* docs.rs/halbu — https://docs.rs/halbu — Rust `.d2s` v99/v105 library; `items` module is
  placeholder-only (so: no Rust item parser to port).
* soarqin/D2RMH — https://github.com/soarqin/D2RMH — working D2R memory reader (no injection);
  tree contains `src/d2r/d2rdefs.h` etc., **no** inventory/stash modelling.
* axiom0x0 — D2R memory trainer on Linux (parts 1–3) — https://axiom0x0.sh/posts/d2r-memory-trainer-part2
  — Proton `memfd:wine-mapping` reality, "online is server-authoritative", save-path confirmation.

Rejected / deprioritized:
* `d2rcharactereditor.com`, `protagonistlabs.app`, `rpgstash.com`, fextralife "stash guide" — SEO /
  secondary; file names only, no format content.
* YouTube videos on stash tabs / "lost stash" — content not verifiable from text.
* `DaddyDeesel/D2R-Vault`, `youdz/d2-stash-organizer` — application-level, no format docs found.
* PlugY `.sss` docs for D2R questions — different game/format, a frequent confusion source.

## 11. Next steps (cheapest first)

1. **Offline item-bearing fixture:** start an offline game with `Wolfie`, put one known unique in
   shared tab 1, exit to menu, then check
   `ModernSharedStashSoftCoreV2.d2i` for items (`ls -l` + the §6 sketch). This gives the first
   real test of the item walk and settles the flush-cadence question empirically.
2. **Memory question (§3.4):** with the stash untouched and then opened, count `act == 0`
   player-units whose `Inventory+0x48` equals the player id. This decides whether the overlay can
   show stash contents at all times or only after the user opens the stash once.
3. **Only if the file path is still wanted:** port the §6 parser to Rust keeping the
   `size`-balance check as an assertion; reuse `UNIQUE_NAMES`/`SET_NAMES` by `*ID` for names.
