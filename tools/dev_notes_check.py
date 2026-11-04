#!/usr/bin/env python3
"""Check the numbers Forever's rules take from Blizzard's development notes of 2 November 2026
against a client spell store, so each change is confirmed once the live data carries it.

    python3 tools/dev_notes_check.py STORE [STORE ...]

STORE is a `spell_store_inputs.json` (gzip or plain), the file wowsims/forever regenerates from
the live client and its hotfix cache: `assets/db_inputs/spell_store_inputs.json` at any of its
commits, most usefully each daily "[DB] Update items to ... with hotfixes as of ..." commit.

Each check prints one of:

- `live`: the store states the value Forever's rules use.
- `old`: the store still states the frozen value, so the engine runs ahead of the live data.
- `DIFFERS`: the store states a third value. The live game disagrees with the engine, which needs
  the store's number (`prepare::hotfix_113554` or the reader named in the check).

A change no client row can state (an attack power share, a proc rate, a form's threat) is listed
as `not in the data`, with what it takes to check it in game. The exit status is 1 when any check
reads DIFFERS.

Uses only Python's standard library.
"""

import argparse
import gzip
import json
import sys
from pathlib import Path

# (label, spell id, effect id or None for a cooldown, field, Forever's value, frozen value)
ROWS = [
    ("Aimed Shot rank 6", 20904, 697764, "BasePoints", 350, 166),
    ("Arcane Shot rank 8", 14287, 691069, "BasePoints", 280, 217),
    ("Serpent Sting rank 8", 13555, 690655, "BasePoints", 100, 83),
    ("Sniper Shot rank 3", 1310786, 1340619, "BasePoints", 590, 295),
    ("Aspect of the Hawk rank 6", 14322, 692138, "BasePoints", 110, 55),
    ("Trueshot Aura rank 5", 20906, 697683, "BasePoints", 100, 50),
    ("Starfire rank 1 base", 2912, 680527, "BasePoints", 113, 81),
    ("Starfire rank 2 base", 8949, 685525, "BasePoints", 154, 110),
    ("Starfire rank 3 base", 8950, 687888, "BasePoints", 201, 144),
    ("Starfire rank 4 base", 8951, 685632, "BasePoints", 277, 198),
    ("Starfire rank 5 base", 9875, 687806, "BasePoints", 372, 266),
    ("Starfire rank 6 base", 9876, 688325, "BasePoints", 471, 337),
    ("Starfire rank 7 base", 25298, 702704, "BasePoints", 533, 381),
    ("Starfire rank 1 a level", 2912, 680527, "PPL", 1.8, 1.3),
    ("Starfire rank 2 a level", 8949, 685525, "PPL", 2.1, 1.5),
    ("Starfire rank 3 a level", 8950, 687888, "PPL", 2.2, 1.6),
    ("Starfire rank 4 a level", 8951, 685632, "PPL", 2.5, 1.8),
    ("Starfire rank 5 a level", 9875, 687806, "PPL", 3.0, 2.2),
    ("Starfire rank 6 a level", 9876, 688325, "PPL", 3.2, 2.3),
    ("Starfire rank 7 a level", 25298, 702704, "PPL", 3.4, 2.4),
    ("Rip rank 6 tick", 9896, 688085, "BasePoints", 19, 15),
    ("Rip rank 6 tick a combo point", 9896, 688085, "PointsPerResource", 31.9, 25.5),
    ("Rake rank 4 hit", 9904, 687442, "BasePoints", 76, 61),
    ("Rake rank 4 tick", 9904, 687443, "BasePoints", 42, 34),
    ("Ferocious Bite roll", 31018, 705079, "BasePoints", 102, 82),
    ("Ferocious Bite roll spread (76 over 102)", 31018, 705079, "Variance", 76 / 102, 0.7317073),
    ("Ferocious Bite a combo point", 31018, 705079, "PointsPerResource", 181, 147),
    ("Ferocious Bite extra Energy (330 for 3.3)", 31018, 1277241, "BasePoints", 330, 270),
    ("Shred weapon damage %", 9830, 688179, "BasePoints", 200, 155),
    ("Claw weapon damage %", 9850, 687219, "BasePoints", 140, 110),
    ("Ravage weapon damage %", 9867, 687812, "BasePoints", 435, 350),
    ("Defensive Stance threat %", 7376, 686475, "BasePoints", 80, 30),
    ("Slam cooldown (ms)", 11605, None, "CategoryCooldownMs", 15000, 18000),
    ("Wrack damage a level", 1316697, 1349556, "PPL", 2.5, 0),
    ("Wrack spell power coefficient", 1316697, 1349556, "SPCoef", 0.286, 0.143),
    ("Stormstrike cooldown (ms)", 17364, None, "CooldownMs", 6000, 8000),
    ("Improved Slam cooldown cut (ms), rank 2", 12862, 1339521, "BasePoints", -5000, -3000),
    # Wowhead's Forever tooltips of 4 November 2026 state these (the `ppl` marker of each).
    ("Lightning Bolt rank 10 base", 15208, 691780, "BasePoints", 353, 196),
    ("Lightning Bolt rank 10 per level", 15208, 691780, "PPL", 2.1, 1.2),
    ("Lightning Bolt rank 5 base", 943, 679378, "BasePoints", 109, 82),
    ("Lightning Bolt rank 1 base", 403, 678918, "BasePoints", 16, 15),
    # The copies deal half their rank ("half damage" in the talent's text); Wowhead's copy rows
    # still state the old 98 and 62, so a store that states them reads "old".
    ("Lightning Bolt overload rank 10 base (half of 353)", 408477, 1072630, "BasePoints", 176.5, 98),
    ("Chain Lightning rank 4 base", 10605, 688920, "BasePoints", 510, 123),
    ("Chain Lightning rank 4 per level", 10605, 688920, "PPL", 3.3, 0.8),
    ("Chain Lightning rank 2 base", 930, 679273, "BasePoints", 348, 100),
    ("Chain Lightning overload rank 4 base (half of 510)", 408484, 1072638, "BasePoints", 255, 62),
    ("Chain Lightning rank 4 spell power coefficient", 10605, 688920, "SPCoef", 0.714, 0.571),
    ("Earth Shock rank 7", 10414, 688559, "BasePoints", 531, 301),
    ("Flame Shock rank 6, direct", 29228, 704945, "BasePoints", 292, 166),
    ("Flame Shock rank 6, a tick", 29228, 704946, "BasePoints", 80, 44),
    ("Frost Shock rank 4", 10473, 688022, "BasePoints", 506, 283),
    ("Earth Shock rank 7 per level", 10414, 688559, "PPL", 3.4, 1.9),
    ("Flawless Execution Energy", 1310711, 1340441, "BasePoints", -15, -10),
    ("Mutilate rank 4 main hand weapon %", 1241586, 1240654, "BasePoints", 90, 75),
    ("Mutilate rank 4 off hand weapon %", 1241590, 1240661, "BasePoints", 90, 75),
    ("Mutilate rank 1 weapon %", 399960, 1173799, "BasePoints", 90, 75),
    ("Riptide rank 3 cooldown (ms)", 1239243, None, "CategoryCooldownMs", 12000, 6000),
    # The Rogue rework (pushes 113351 and 113483): None is a row the frozen data lacks.
    ("Mutilate rank 3 main hand flat damage", 1241586, 1240653, "BasePoints", 88, 67),
    ("Mutilate rank 3 off hand flat damage", 1241590, 1240660, "BasePoints", 88, 67),
    ("Mutilate rank 2 main hand flat damage", 1241585, 1240651, "BasePoints", 63, 48),
    ("Mutilate rank 1 main hand flat damage", 399960, 1044484, "BasePoints", 44, 33),
    ("Shank rank 2 weapon %", 1329549, 1369159, "BasePoints", 50, None),
    ("Shank rank 2 Energy generated", 1329549, 1369160, "BasePoints", 25, None),
    ("Shank rank 1 Energy generated", 1329283, 1368711, "BasePoints", 15, None),
    ("Shank rank 2 cooldown (ms)", 1329549, None, "CooldownMs", 10000, None),
    ("Death Mark rank 4 cooldown (ms)", 1325758, None, "CooldownMs", 10000, None),
    ("Poised cost reduction", 1328826, 1368039, "BasePoints", -10, None),
    ("Venom aura poison damage %", 1310703, 1340416, "BasePoints", 30, None),
    ("Venom aura poison chance (flat, last effect)", 1310703, 1340418, "BasePoints", 20, None),
    ("Boot Knife Combo Points", 1329285, 1368713, "BasePoints", 2, None),
    ("Relentless Strikes Energy", 14181, 691778, "BasePoints", 25, None),
    ("Spearing Strike weapon damage %", 1310222, 1339573, "BasePoints", 75, 40),
    ("Spearing Strike bonus against Giants and Dragonkin (times)", 1310222, 1339768, "BasePoints", 1, 2),
    ("Bloodthirst rank 4 attack power share % (unchanged)", 23894, 1339740, "BasePoints", 45, 45),
    ("Reckless Swings cooldown (ms)", 1329555, None, "CooldownMs", 15000, 0),
    ("Reckless Swings charges' Rage (tenths)", 1329554, 1369166, "BasePoints", 50, 0),
]

# (label, spell id, power cost field, Forever's value, frozen value): see POWERS below.

# (label, spell id, power cost field, Forever's value, frozen value): the mana cost a row states.
POWERS = [
    ("Riptide rank 1 mana (about 40% less)", 408521, "Cost", 147, 245),
    ("Riptide rank 2 mana (about 40% less)", 1239242, "Cost", 180, 300),
    ("Riptide rank 3 mana (about 40% less)", 1239243, "Cost", 231, 385),
    ("Shank rank 2 Energy cost", 1329549, "Cost", 5, None),
    ("Death Mark rank 4 Energy cost", 1325758, "Cost", 25, None),
    ("Mutilate rank 3 Energy cost", 1241584, "Cost", 60, 60),
    ("Reckless Swings health cost (% of base health)", 1329555, "CostPct", 25, 0),
    # Wowhead's tooltips of 4 November 2026: ranks 1 and 2 still read their old costs.
    ("Riptide rank 1 mana", 408521, "Cost", 245, 245),
    ("Riptide rank 2 mana", 1239242, "Cost", 300, 300),
    ("Riptide rank 3 mana (about 40% less)", 1239243, "Cost", 230, 385),
]

# (label, spell id, Forever's value, frozen value): the stacks an aura row states.
STACKS = [
    ("Vengeance stacks (Paladin, 5 in the notes)", 20050, 5, 3),
]

# (label, spell id, effect id, class flag bit, whether Forever's rules have it set)
CLASS_FLAGS = [
    ("Hemorrhage's debuff covers Garrote", 16511, 693309, 256, True),
    ("Aggression leaves Backstab out", 18427, 695769, 4, False),
    ("Efficient Strikes covers Hemorrhage", 13732, 691146, 33554432, True),
]

# (label, talent spell id, curve row, Forever's values, frozen values)
TALENTS = [
    ("Barrage", 19461, 0, [5, 10, 15], [3, 7, 10]),
    ("Two-Handed Weapon Specialization", 12163, 0, [2, 4, 6], [1, 2, 3]),
    ("Defiance", 12792, 0, [7, 13, 20], [5, 10, 15]),
    ("Shadow and Flame damage", 426316, 2, [4, 8, 12, 16, 20], [2, 4, 6, 8, 10]),
    ("Lightning Overload", 408438, 0, [10, 20, 30], [3, 7, 10]),
    ("Mental Dexterity", 415140, 0, [50, 100, 150], [33, 67, 100]),
    ("Flurry (Shaman)", 16256, 0, [6, 12, 18, 24, 30], [5, 10, 15, 20, 25]),
    ("Aggression", 18427, 0, [5, 10, 15], [2, 4, 6]),
    ("Rend and Tear", 1223246, 0, [3, 6, 9, 12, 15], [2, 4, 6, 8, 10]),
    # The Rogue trees (push 113483). Curves of talents whose rank count or values changed.
    ("Lethality", 14128, 0, [10, 20], [4, 8, 12, 16, 20]),
    ("Seal Fate", 14186, 0, [33, 66, 100], [20, 40, 60, 80, 100]),
    ("Opportunity damage", 14057, 0, [4, 8, 12, 16, 20], [5, 10]),
    ("Opportunity Garrote", 14057, 1, [4, 8, 12, 16, 20], [5, 10]),
    ("Dual Wield Specialization", 13715, 0, [25, 50], [5, 10, 15, 20, 25]),
    ("Relentless Strikes chance a Combo Point", 14179, 0, [4, 8, 12, 16, 20], None),
    ("Coup de Grace Eviscerate", 14162, 0, [7, 13, 20], [7, 13, 20]),
    ("Coup de Grace Death Mark", 14162, 1, [7, 13, 20], None),
    ("Efficient Strikes", 13732, 0, [-3, -5], [-3, -5]),
    ("Composure", 1329265, 0, [-5, -10], None),
    ("Flash of Steel Energy", 1329284, 0, [3, 6, 9, 12, 15], None),
    ("Flash of Steel cooldown (ms)", 1329284, 1, [-400, -800, -1200, -1600, -2000], None),
    ("Boot Knife chance", 1329286, 0, [20, 40, 60, 80, 100], None),
    ("Blood Splatter Rupture (ms)", 1329256, 0, [6000], None),
    ("Blood Splatter Garrote (ms)", 1329256, 1, [9000], None),
    ("Flurry (Warrior)", 12319, 0, [10, 20, 30], [5, 10, 15, 20, 25]),
    ("Improved Tactical Mastery", 12295, 0, [5, 10, 15], [3, 6, 9, 12, 15]),
    ("Bloodthrill", 1289682, 0, [10, 20], [4, 8, 12, 16, 20]),
    ("Improved Whirlwind cooldown (ms)", 1310315, 0, [-1000, -2000], []),
    ("Improved Whirlwind closest enemy damage", 1310315, 1, [15, 30], []),
    ("Furious Precision (unchanged)", 1323963, 0, [4, 7, 10], [4, 7, 10]),
]

NOT_IN_DATA = [
    ("Defensive Stance threat 80% and Defiance 7/13/20% (Warrior)",
     "no row yet: the 4 November store still states 30 (effect 686475) and 5/10/15, and Wowhead's "
     "tooltips read 30% and 15%; Forever's rules keep the notes' values until a row or a blue post "
     "says otherwise"),
    ("Gore Drinker and Lingering Rage (Warrior)",
     "healing and the Rage decay out of combat, which the sim does not model"),
    ("Riptide has no initial heal, only the heal over time",
     "the tooltip names a heal and a heal over time; the first effect leaves the row as a heal"),
    ("Poised for Druids (level 32, 10 Energy off the next Energy ability out of combat)",
     "ids 1328829 (passive) and 1328827 (the -10 Energy aura, one charge) are in the store since "
     "hotfix push 113578; the engine's rule that the fight starts at maximum Energy, out of "
     "combat, is not in a row"),
    ("Instant Poison 1% and Deadly Poison 0.9% a tick of attack power",
     "poison hits at two attack power values: each grows 0.01 and 0.009 a point (a stack for Deadly)"),
    ("Shank: never missed, dodged or parried; only blocked or critical",
     "the row's Always Hit attribute; check the combat log for Shank misses on a target"),
    ("Poised appears once, at the pull, and is spent by the first Energy ability",
     "the first cast of a pull in the combat log; a rogue that regains full Energy out of combat "
     "gets it again, which a fixed duration fight never has"),
    ("Death Mark: 12% a Combo Point stored (twice for Garrote and Rupture), cap (156 + 245 a point "
     "+ 0.04 attack power a point) times (1 + crit chance), Coup de Grace on both",
     "tooltip text of Wowhead's Forever data (4 November 2026); the rows state only the cost, "
     "cooldown and the 6 sec, and which damage counts and how the release rolls are script"),
    ("Quick Getaway, Camouflage, Setup (nothing a fight reads)", "no sim effect"),
    ("Arcane Shot 15% of ranged attack power",
     "Arcane Shot on a training dummy at two ranged attack power values: the damage grows 0.15 a point"),
    ("Serpent Sting 5% of ranged attack power a tick",
     "Serpent Sting ticks at two ranged attack power values: each tick grows 0.05 a point"),
    ("Rip 1% a combo point (up to four), Rake 5% and Ferocious Bite 3% a combo point of attack "
     "power, each times the notes' 1.25",
     "no row or tooltip states the shares (server side): ticks and hits at two attack power "
     "values, the damage grows by the share a point"),
    ("Pounce rank 3 bleed 31 a tick (186 over 18 sec, was 25)",
     "the engine does not cast Pounce; its bleed row 9826 reads 25 in the frozen data"),
    ("Rend and Tear 3/6/9/12/15% (Wowhead's talent spell tooltip reads 20% at 5/5)",
     "the tooltip prints the row's own base points, 20 in the frozen data too; the trait curve "
     "check above reads the per rank value"),
    ("Bear Form threat 215%", "threat meter on a dummy with a known damage hit"),
    ("Maelstrom Weapon chance doubled (20 PPM)",
     "count stacks over a few minutes of melee with a known weapon speed"),
    ("Stormstrike 150% weapon damage", "Stormstrike hits against a dummy with a known weapon"),
    ("Chain Lightning cast time 2.5 sec and mana 248/304/428/495 (225% of Lightning Bolt)",
     "the spell tooltips: Wowhead still states 2 sec and 145/200/250/315 mana on 4 November 2026, "
     "so the notes stand until it changes"),
    ("Call of Thunder, +5% crit for Lightning Bolt and Chain Lightning, in Lava Burst's slot",
     "the talent's tooltip once it has a row; Lava Burst (1326047) leaves the store with it"),
]


def load(path):
    raw = Path(path).read_bytes()
    if raw[:2] == b"\x1f\x8b":
        raw = gzip.decompress(raw)
    return json.loads(raw)


def close(a, b):
    return abs(float(a) - float(b)) < 1e-3


def verdict(value, forever, frozen):
    """`frozen` is None for a row the frozen data does not have."""
    if value is None:
        return "missing"
    if close(value, forever):
        return "live"
    if frozen is not None and close(value, frozen):
        return "old"
    return "DIFFERS"


def show(value):
    return "none" if value is None else f"{value:g}"


def row_value(store, spell, effect_id, field):
    if effect_id is None:
        return (store["Cooldowns"].get(str(spell)) or {}).get(field)
    for effect in store["Effects"].get(str(spell), []):
        if effect["ID"] == effect_id:
            return effect.get(field)
    return None


def talent_curve(store, spell, row):
    nodes = {node["SpellID"]: node for node in store["TraitNodes"]}
    node = nodes.get(spell)
    if node is None:
        return None
    points = store["TraitPoints"].get(str(node["DefinitionID"]), {}).get(str(row))
    if not points:
        return None
    return [points[str(rank)] for rank in range(1, node["MaxRanks"] + 1)]


def curve_verdict(values, forever, frozen):
    if values is None:
        return "missing"
    if len(values) == len(forever) and all(map(close, values, forever)):
        return "live"
    if frozen is not None and len(values) == len(frozen) and all(map(close, values, frozen)):
        return "old"
    return "DIFFERS"


def check(path):
    store = load(path)
    builds = sorted(set(store.get("Builds", {}).values()))
    hotfixes = sorted(set(store.get("Hotfixes", {}).values()))
    print(f"{path}: client {', '.join(builds)}, hotfix push {', '.join(map(str, hotfixes))}")
    differs = 0
    for label, spell, effect_id, field, forever, frozen in ROWS:
        value = row_value(store, spell, effect_id, field)
        state = verdict(value, forever, frozen)
        differs += state == "DIFFERS"
        print(f"  {state:8} {label}: store {value}, Forever {forever:g}, frozen {show(frozen)}")
    for label, spell, field, forever, frozen in POWERS:
        powers = store.get("Powers", {}).get(str(spell)) or [{}]
        value = powers[0].get(field)
        state = verdict(value, forever, frozen)
        differs += state == "DIFFERS"
        print(f"  {state:8} {label}: store {value}, Forever {forever:g}, frozen {show(frozen)}")
    for label, spell, forever, frozen in STACKS:
        options = store.get("AuraOptions", {}).get(str(spell)) or {}
        value = options.get("MaxStack")
        state = verdict(value, forever, frozen)
        differs += state == "DIFFERS"
        print(f"  {state:8} {label}: store {value}, Forever {forever:g}, frozen {show(frozen)}")
    for label, spell, effect_id, bit, forever in CLASS_FLAGS:
        mask = None
        for effect in store["Effects"].get(str(spell), []):
            if effect["ID"] == effect_id:
                mask = effect["ClassFlags"]["Mask"][0]
        state = "missing" if mask is None else "live" if bool(mask & bit) == forever else "old"
        print(f"  {state:8} {label}: store mask {mask}, bit {bit}")
    for label, spell, row, forever, frozen in TALENTS:
        values = talent_curve(store, spell, row)
        state = curve_verdict(values, forever, frozen)
        differs += state == "DIFFERS"
        print(f"  {state:8} {label}: store {values}, Forever {forever}, frozen {frozen or 'none'}")
    return differs


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("stores", nargs="+")
    args = parser.parse_args()
    differs = sum(check(path) for path in args.stores)
    print("not in the data, to check in game:")
    for label, how in NOT_IN_DATA:
        print(f"  {label}: {how}")
    return 1 if differs else 0


if __name__ == "__main__":
    sys.exit(main())
