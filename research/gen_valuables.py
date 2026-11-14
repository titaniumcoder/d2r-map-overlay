import json, sys, os
D = sys.argv[1]; OUT = sys.argv[2]
W = json.load(open(f'{D}/weapons.json')); A = json.load(open(f'{D}/armor.json')); M = json.load(open(f'{D}/misc.json'))
U = json.load(open(f'{D}/uniqueitems.json')); S = json.load(open(f'{D}/setitems.json'))
BASE = {**W, **A, **M}
R = lambda a, b: [f'r{i:02d}' for i in range(a, b + 1)]
rules = []
def rule(tier, cat, codes, quality, name, note, sockets=None, eth=None, item_names=None, runewords=None, needs_stats=False):
    r = dict(tier=tier, category=cat, item_codes=codes, quality=quality, sockets=sockets, ethereal=eth, name=name, note=note)
    if item_names: r['item_names'] = item_names
    if runewords: r['runewords'] = runewords
    if needs_stats: r['needs_stats'] = True
    rules.append(r)
def named(tier, quality, names, name, note, eth=None, needs_stats=False):
    src = U if quality == 'unique' else S
    codes = []
    for n in names:
        hits = [v['code' if quality == 'unique' else 'item'] for v in src.values() if v.get('index') == n]
        assert hits, n
        for c in hits:
            if c not in codes: codes.append(c)
    rule(tier, 'trade', codes, quality, name, note, eth=eth, item_names=names, needs_stats=needs_stats)

# ---------------- TRADE: runes ----------------
rule('S','trade',R(26,33),'any','High runes Vex-Zod','Vex..Zod are the high-rune trade currency (maxroll). Ber/Jah top.')
rule('A','trade',R(20,25),'any','Mid runes Lem-Gul','Mid-rune currency for smaller trades (maxroll). Ist/Gul/Mal most liquid.')
# ---------------- TRADE: uniques ----------------
named('S','unique',['Annihilus'],'Annihilus','Dropped by Uber Diablo / DClone. Value depends on roll but always keep.')
named('S','unique',['Hellfire Torch'],'Hellfire Torch','Uber Tristram. Class + roll decide price; every one sells.')
named('S','unique',["Gheed's Fortune",'PreCrafted Cold Rupture','PreCrafted Flame Rift','PreCrafted Crack of the Heavens','PreCrafted Rotting Fissure','PreCrafted Bone Break','PreCrafted Black Cleft'],'Unique grand charms (Latent Sunders, Gheed\'s)','RotW: Sunders drop as latent (PreCrafted) grand charms; every unique cm3 on the ground is a keeper. Lightning/Cold/Fire Sunders highest. Gheed\'s alone is A-tier.')
named('S','unique',['Harlequin Crest'],'Harlequin Crest (Shako)','Staple caster helm, always liquid.')
named('S','unique',["Griffon's Eye"],"Griffon's Eye",'Lightning sorc/LS assassin; high value.')
named('S','unique',['The Stone of Jordan'],'Stone of Jordan','Secondary currency; DClone trigger.')
named('S','unique',["Mara's Kaleidoscope","Highlord's Wrath"],'Top unique amulets','Mara\'s (+2 skills, 20-30 res) and Highlord\'s (Zeal/Fury builds). Shared base "amu" with ~12 other uniques: needs the unique name.')
named('S','unique',["Bul Katho's Wedding Band",'Raven Frost','Wisp'],'Top unique rings','BK (+1 skills), Raven Frost (CBF), Wisp Projector. Shared base "rin": needs the unique name.')
named('S','unique',['Fathom',"Deaths's Web","Crown of Ages",'Arachnid Mesh',"Tyrael's Might","Dracul's Grasp","Verdugo's Hearty Cord","Nightwing's Veil",'Stormshield','Windforce'],'High-value elite uniques','Death\'s Fathom (data index "Fathom"), Death\'s Web (data index "Deaths\'s Web"), CoA, Arachnid, Tyrael\'s, Dracul\'s, Verdugo\'s, Nightwing\'s, Stormshield, Windforce.')
named('A','unique',["Andariel's Visage","The Reaper's Toll","Titan's Revenge",'Lacerator','Deathcleaver',"Skullder's Ire"],'Ethereal-only uniques','Only ethereal copies have real trade value (merc/indestructible).',eth=True)
named('A','unique',['Wartraveler','Gorerider',"Thudergod's Vigor","Herald of Zakarum","Arreat's Face","Jalal's Mane",'Homunculus',"Eschuta's temper",'Rainbow Facet','Steelshade'],'Mid-value uniques','War Traveler, Gore Rider, Thundergod\'s (data typo "Thudergod\'s"), +2 class uniques (Herald/Arreat/Jalal/Homunculus), Eschuta\'s (+3 skill rolls matter), Rainbow Facets (perfect rolls only).',needs_stats=True)
named('B','unique',["Skin of the Vipermagi",'Chance Guards','Magefist','Shaftstop','Goldwrap',"Ormus' Robes","Nosferatu's Coil",'Leviathan',"Arkaine's Valor","Medusa's Gaze","Mang Song's Lesson","Ondal's Wisdom","Kira's Guardian",'Sandstorm Trek','Marrowwalk','Waterwalk',"Headhunter's Glory",'Giantskull','Vampiregaze','Tomb Reaver','Steelrend',"Bloodraven's Charge",'Lidless Wall','Stealskull','Wizardspike'],'Useful build uniques (low/medium value)','Sell to new-ladder players or keep for builds. Several only sell with sockets/eth/good rolls (Headhunter 3os, Giantskull 2os, Tomb Reaver eth 3os, Sandstorm eth).',needs_stats=True)
named('B','unique',["Ars Al'Diablolos","Ars Tor'Baalos","Ars Dul'Mephistos",'Measured Wrath','Dreadfang','Wraithstep','Bloodpact Shard','Sling','Opalvein','Entropy Locket',"Gheed's Wager",'Unique Warlock Helm'],'Reign of the Warlock uniques','New Feb 2026 uniques (grimoires, rings, amulet, belt, boots, dagger). No settled price consensus found; highlight so you notice them. "Unique Warlock Helm" is the data placeholder name on Death Mask.')
named('A','unique',["Defender's Bile","Guardian's Thunder","Protector's Frost","Defender's Fire","Protector's Stone","Guardian's Light"],'Colossal Jewels','Unique Colossal Jewels from the Colossal Ancients fight (lvl 75 req). Reward items, rarely seen on the ground.')
# ---------------- TRADE: sets ----------------
named('A','set',["Tal Rasha's Howling Wind","Tal Rasha's Adjudication","Tal Rasha's Horadric Crest","Tal Rasha's Fire-Spun Cloth","Tal Rasha's Lidless Eye"],"Tal Rasha's Wrappings",'Armor (Guardianship/Howling Wind) and amulet sell best. Base "amu"/"xsk" shared with other sets/uniques: needs the set name.')
named('A','set',["Immortal King's Soul Cage","Immortal King's Stone Crusher","Immortal King's Forge","Immortal King's Detail","Immortal King's Pillar","Immortal King's Will"],'Immortal King','Stone Crusher only tradable with 40% CB; whole set popular early ladder.',needs_stats=True)
named('A','set',["Trang-Oul's Claws","Trang-Oul's Guise","Trang-Oul's Scales","Trang-Oul's Girth","Trang-Oul's Wing"],"Trang-Oul's Avatar",'Claws are used outside the set; rest early-ladder demand.')
named('A','set',['Laying of Hands'],'Laying of Hands','Bramble Mitts; demand from Paladins/Barbs (IAS + dmg vs demons).')
named('B','set',["Griswold's Heart","Griswold's Honor","Griswold's Valor","Griswolds's Redemption","Natalya's Mark","Natalya's Shadow","Natalya's Soul","Natalya's Totem","M'avina's Caster","M'avina's Embrace","M'avina's Icy Clutch","M'avina's Tenet","M'avina's True Sight","Aldur's Advance","Aldur's Deception","Aldur's Gauntlet","Aldur's Stony Gaze","Bul-Kathos' Sacred Charge","Bul-Kathos' Tribal Guardian","Guillaume's Face","Sazabi's Ghost Liberator","Sazabi's Cobalt Redeemer","Sazabi's Mental Sheath"],'Other elite set items','Low-medium value; Griswold\'s Redemption needs 4os, Guillaume\'s / Bul-Kathos used in builds.',needs_stats=True)
# ---------------- TRADE: misc / materials ----------------
rule('A','trade',['pk1','pk2','pk3'],'any','Uber keys','Terror/Hate/Destruction; sold in 3x3 sets.')
rule('A','trade',['dhn','bey','mbr'],'any','Uber organs','Diablo\'s Horn, Baal\'s Eye, Mephisto\'s Brain (Uber Tristram).')
rule('A','trade',['ua1','ua2','ua3','ua4','ua5'],'any','Colossal Ancient Statues','RotW: 5 needed (one per act) to open the Colossal Summit; drop from terrorized act bosses in Hell. Drop rate nerfed in 3.3.')
rule('A','trade',['um1','um2','um3','um4','um5','um6'],'any','Colossal Jewel upgrade materials','RotW Uber Ancient upgrade materials (named Defender\'s Fire, Guardian\'s Light, ...).')
rule('A','trade',['xa1','xa2','xa3','xa4','xa5'],'any','Worldstone Shards','RotW Terror Zone drop; rate reduced in 3.3.')
rule('B','trade',['toa','tes','ceh','bet','fed'],'any','Token of Absolution + essences','Respec token and its essences.')
rule('B','trade',['gpv','gpw','gpr','gpb','gpy','gpg','skz'],'any','Perfect gems','Sell in bulk (40 PGems), Perfect Amethyst needed for caster-amulet crafting.')
# ---------------- TRADE: magic / rare ----------------
rule('A','trade',['cm3'],'magic','Magic grand charms','Skillers (+1 skill tab) and 20+ life skillers are the value; most are trash. Needs identified stats.',needs_stats=True)
rule('B','trade',['cm1'],'magic','Magic small charms','Valuable: 5 all res, 20 life, 7 MF, 3/20/20. Needs stats.',needs_stats=True)
rule('B','trade',['cm2'],'magic','Magic large charms','Only 5-6 max dmg/20-35 life (sharp) sells. Needs stats.',needs_stats=True)
rule('B','trade',['jew'],'magic','Magic jewels','40/15 IAS jewels, 30-40 ED (Fervor), 10-15 all res. Needs stats.',needs_stats=True)
rule('B','trade',['jew'],'rare','Rare jewels','ED/IAS/res/-req rares can sell. Needs stats.',needs_stats=True)
rule('A','trade',['ci0','ci1','ci2','ci3'],'magic','Socketed magic circlets/tiaras/diadems','2-3 os magic circlets (Artisan\'s, +2 class skills/FCR/FRW). Sockets are visible; affixes need stats.',sockets=[2,3],needs_stats=True)
rule('B','trade',['ci0','ci1','ci2','ci3'],'rare','Rare circlets','+2 skills / FCR / FRW / sockets. Needs stats.',needs_stats=True)
rule('A','trade',['uit'],'magic','Jeweler\'s Monarch (magic 4os)','A magic Monarch with 4 sockets is necessarily the Jeweler\'s prefix; "of Deflecting" suffix makes it very high.',sockets=[4,4])
rule('B','trade',['utp','uui'],'magic','Jeweler\'s Archon Plate / Dusk Shroud (magic 4os)','Value comes from the suffix (of the Whale 80-100 life).',sockets=[4,4],needs_stats=True)
rule('B','trade',['rin','amu'],'rare','Rare rings and amulets','Caster (FCR + skills/str/dex/res) and melee (LL/AR/str/dex) rares. Needs stats.',needs_stats=True)
cls = [c for c,v in {**W,**A}.items() if v.get('type') in ('ashd','orb','h2h','h2h2','phlm','pelt','head','ajav','abow','aspe')]
rule('B','trade',sorted(cls),'magic','Class-specific magic items','+3 skill/+6 single skill items (Paladin shields, Sorc orbs, Assassin claws, Barb helms, Druid pelts, Necro heads, Amazon weapons). Only rolls with +3 class / high single skills sell. Needs stats.',needs_stats=True)

# ---------------- TWINK: runeword bases (quality normal => white base, normal OR superior) ----------------
POLE_ELITE = ['7s8','7wc','7pa','7vo','7h7']
rule('S','twink',['utp','uui'],'normal','Archon Plate / Dusk Shroud 3-4os','3os: Enigma (Jah Ith Ber). 4os: Chains of Honor / Fortitude. Superior (+def%) preferred. Unsocketed superior ones also worth keeping: Larzuk gives 4.',sockets=[3,4],runewords=['Enigma','Chains of Honor','Fortitude'])
rule('A','twink',['uar','uea','urs','uul','uld','ult','upl','uth'],'normal','Other elite body armor 3-4os','Enigma/CoH/Fortitude on Sacred Armor, Wyrmhide, Great Hauberk, Shadow/Lacquered/Hellforge/Balrog, Kraken. Superior preferred.',sockets=[3,4],runewords=['Enigma','Chains of Honor','Fortitude'])
rule('A','twink',['uar','urs','uld','ult','uul'],'normal','Ethereal elite armor 4os (merc)','Merc Fortitude / CoH; eth Sacred Armor (850+ def) is the best Fortitude merc base.',sockets=[4,4],eth=True,runewords=['Fortitude','Chains of Honor'])
rule('S','twink',POLE_ELITE+['7br','amd'],'normal','Ethereal elite polearm/spear 4os','Merc Infinity (Ber Mal Ber Ist) or Insight. Ethereal superior Thresher / Giant Thresher / Colossus Voulge / Cryptic Axe / Great Poleaxe / Mancatcher / Matriarchal Spear.',sockets=[4,4],eth=True,runewords=['Infinity','Insight'])
rule('A','twink',['7vo'],'normal','Ethereal Colossus Voulge 0os (Larzuk to 4)','Colossus Voulge max sockets is 4, so Larzuk always yields 4 -> Infinity/Insight. Other elite polearms Larzuk to 5-6 (too many for Infinity).',sockets=[0,0],eth=True,runewords=['Infinity','Insight'])
rule('A','twink',['vou','scy','pax','hal','wsc','9vo','9s8','9pa','9h9','9wc']+POLE_ELITE,'normal','Polearms 4os (any tier)','Insight (Ral Tir Tal Sol) for the Act 2 merc. Normal-difficulty Voulge/Scythe/Poleaxe/Halberd/War Scythe already works; eth preferred but not required.',sockets=[4,4],runewords=['Insight'])
rule('A','twink',['7s8','7wc','7pa','7h7','7br','amd','7st','7p7'],'normal','Ethereal elite polearm/spear 5os','Pride (Cham Sur Io Lo), Obedience (Hel Ko Thul Eth Fal), Doom on polearms.',sockets=[5,5],eth=True,runewords=['Pride','Obedience','Doom'])
rule('S','twink',['7cr'],'normal','Phase Blade 5-6os','5os: Grief (Eth Tir Lo Mal Ral). 6os: Last Wish / BotD. Superior (ED) preferred; no durability so eth irrelevant.',sockets=[5,6],runewords=['Grief','Last Wish','Breath of the Dying'])
rule('A','twink',['7wa','7gd','7gm','6ws','7p7','6lw'],'normal','Berserker Axe / Colossus Blade / Thunder Maul / Archon Staff / War Pike / Hydra Bow 5-6os','5os: Death, Doom, Beast, Grief (axe). 6os: Breath of the Dying, Last Wish (axe/hammer). Ethereal superior strongly preferred (BotD/Death are indestructible).',sockets=[5,6],runewords=['Breath of the Dying','Death','Doom','Beast','Last Wish','Grief'])
rule('A','twink',['crs','9cr','7cr','lsd','9ls','bsd','9bs'],'normal','Swords 4os (Crystal/Dimensional/Phase/Long/Broad)','Spirit sword (Tal Thul Ort Amn); 4os Crystal Sword drops in Normal (ilvl 26+). Also Hand of Justice / Phoenix on Phase Blade.',sockets=[4,4],runewords=['Spirit','Phoenix','Hand of Justice'])
rule('A','twink',['crs','9cr','7cr','fla','9fl','7fl'],'normal','Crystal Sword / Flail 5os','Call to Arms (Amn Ral Mal Ist Ohm) swap weapon. Eth for style only.',sockets=[5,5],runewords=['Call to Arms'])
rule('A','twink',['fla','9fl','7fl'],'normal','Flail/Knout/Scourge 4os','Heart of the Oak (Ko Vex Pul Thul) swap/caster weapon.',sockets=[4,4],runewords=['Heart of the Oak'])
rule('A','twink',['uit'],'normal','Monarch 4os','Spirit shield (or Phoenix). Superior preferred. 0os superior Monarch also keeps value (Larzuk -> 4).',sockets=[4,4],runewords=['Spirit','Phoenix','Dream'])
PALA = [c for c,v in A.items() if v.get('type')=='ashd']
rule('A','twink',sorted(PALA),'normal','Paladin shields 4os (any tier)','Spirit / Exile / Phoenix. Value is in the implicit res/ED roll (needs stats): superior 4os Sacred Targe/Rondache/Vortex with 40+ res = high. Normal-tier Targe/Rondache 4os = great twink Spirit.',sockets=[4,4],runewords=['Spirit','Exile','Phoenix'],needs_stats=True)
rule('A','twink',sorted(PALA),'normal','Ethereal paladin shields 4os','Exile (Vex Ohm Ist Dol) self-repairs, so eth Sacred Targe/Vortex preferred.',sockets=[4,4],eth=True,runewords=['Exile'])
rule('A','twink',['amc','amb'],'normal','Grand Matron / Matriarchal Bow 4os','Faith (Ohm Jah Lem Eld) for Act 2 merc / Rogue, Insight on bows.',sockets=[4,4],runewords=['Faith','Insight'])
rule('B','twink',['am1','am2','am6','am7','amb','amc'],'normal','Amazon bows 5os','Mist (Cham Shael Gul Thul Ith), Wrath/Ice-type builds.',sockets=[5,5],runewords=['Mist'])
rule('B','twink',['uh9','ci3','ci2','ush'],'normal','Bone Visage / Diadem / Tiara / Troll Nest 3os','Dream (Io Jah Pul), Coven (Ist Ral Io, RotW), Flickering Flame, Metamorphosis. Best Dream bases are +skill Barb helms/Druid pelts (see class rule).',sockets=[3,3],runewords=['Dream','Coven','Flickering Flame','Metamorphosis'])
rule('B','twink',sorted([c for c,v in A.items() if v.get('type') in ('phlm','pelt')]),'normal','Barbarian helms / Druid pelts 2-3os','White class helms with +3 skills (e.g. +3 BO Barb helm, +3 Fury/Tornado pelt) for Dream/Lore/Metamorphosis/Coven. Implicit skills need stats.',sockets=[2,3],runewords=['Dream','Lore','Metamorphosis','Coven'],needs_stats=True)
rule('B','twink',['xtp'],'normal','Mage Plate 3os','Treachery, Duress, Smoke, Bone, Hustle, Authority (RotW). Eth for merc Treachery.',sockets=[3,3],runewords=['Treachery','Duress','Smoke','Bone','Hustle','Authority'])
rule('B','twink',['qui','lea','hla','stu','rng','brs'],'normal','Light body armor 2-3os (normal difficulty)','2os: Stealth (Tal Eth). 3os Ring Mail/Breast Plate: Smoke/Treachery/Authority twink.',sockets=[2,3],runewords=['Stealth','Smoke','Treachery','Authority'])
rule('B','twink',['kit','lrg','xit','xrg'],'normal','Kite / Large / Dragon shield 3os','Ancient\'s Pledge (Ral Ort Tal) leveling shield.',sockets=[3,3],runewords=["Ancient's Pledge"])
rule('B','twink',['cap','skp','hlm','fhl','xap','xkp','uap','ukp','ci0','ci1'],'normal','Helms 2os','Lore (Ort Sol) leveling helm.',sockets=[2,2],runewords=['Lore'])
rule('B','twink',['7lw','7tw','9lw','clw'],'normal','Claws 3os','Mosaic (Mal Gul Amn) / Plague. Best with +3 trap/martial arts implicits (needs stats).',sockets=[3,3],runewords=['Mosaic','Plague'],needs_stats=True)
rule('B','twink',['7cr','crs','9cr','72h'],'normal','Phase Blade / Crystal Sword / Legend Sword 3os','Lawbringer / Crescent Moon (merc). Cheap.',sockets=[3,3],runewords=['Lawbringer','Crescent Moon'])
rule('B','twink',['kri','bld','9kr','9bl','7kr','7bl'],'normal','Daggers 3os','RotW Void (Thul Zod Ist) and Ritual (Amn Shael Ohm) for Warlock. Prefer Fanged Knife / Legend Spike; demand not settled.',sockets=[3,3],runewords=['Void','Ritual'])
rule('B','twink',sorted([c for c,v in A.items() if v.get('type')=='grim']),'normal','Grimoires 2os','RotW Vigilance (Dol Gul) on Warlock books; also fits shields/heads.',sockets=[2,2],runewords=['Vigilance'])
rule('B','twink',R(1,19),'any','Low runes El-Fal','Spirit (Tal Thul Ort Amn), Insight (Ral Tir Tal Sol), Stealth/Lore/Ancient\'s Pledge sets; Ko/Fal/Hel/Io/Lum feed mid runewords. Spirit and Insight rune sets trade.')

# ---------------- validate ----------------
for r in rules:
    for c in r['item_codes']:
        assert c in BASE, (r['name'], c)
    if r.get('item_names'):
        for n in r['item_names']:
            src = U if r['quality']=='unique' else S
            assert any(v.get('index')==n and (v.get('code') or v.get('item')) in r['item_codes'] for v in src.values()), n
    if r['sockets']:
        mx = max(BASE[c].get('gemsockets') or 0 for c in r['item_codes'])
        assert r['sockets'][1] <= mx or r['sockets']==[0,0], (r['name'], mx)
        for c in r['item_codes']:
            if (BASE[c].get('gemsockets') or 0) < r['sockets'][0]:
                print('WARN base cannot reach min sockets', r['name'], c, BASE[c]['name'], BASE[c].get('gemsockets'), file=sys.stderr)
for r in rules:
    r['base_names'] = [BASE[c]['name'] for c in r['item_codes']]
doc = {
  'schema': 1,
  'generated': '2026-11-15',
  'game': 'Diablo II: Resurrected 3.x (Reign of the Warlock, ladder season 15/16)',
  'codes_source': 'blizzhackers/d2data json (commit 2026-11-14 "Updated for new patch")',
  'conventions': {
    'quality': 'normal = white runeword base (normal OR superior, not low-quality/cracked); superior = superior only; any = ignore quality',
    'sockets': '[min,max] inclusive; [0,0] = unsocketed; null = ignore',
    'ethereal': 'true/false = required; null = ignore',
    'item_names': 'exact uniqueitems.json / setitems.json "index" strings (incl. data typos); when present the rule should only fire if the resolved unique/set name matches. If the overlay cannot resolve names, fall back to code+quality and expect false positives on shared bases (rin, amu, jew, cm3, xsk, uar, 7cr, ...).',
    'needs_stats': 'true = base/quality/sockets/eth alone cannot decide value; highlight as "check me"',
    'tiers': 'S always highlight, A highlight, B optional',
    'precedence': 'if several rules match, use the highest tier',
  },
  'rules': rules,
}
json.dump(doc, open(OUT,'w'), indent=2, ensure_ascii=False)
print(len(rules), 'rules', {t: sum(r['tier']==t for r in rules) for t in 'SAB'})
