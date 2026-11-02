#!/usr/bin/env python3
"""Translate the reference's generated item and enchant effect registrations into Rust.

The pinned Go engine generates sim/common/forever/stat_bonus_cds_auto_gen.go,
stat_bonus_procs_auto_gen.go and enchants_auto_gen.go from the client's spell rows: one call of
a shared constructor per item or enchant, and a great many commented-out ones the generator could
not resolve. This writes the live calls, in Go's registration order, as the tables in
src/prepare/forever_items_generated.rs, which src/prepare/forever_items.rs applies.

Under Forever's rules it also registers the chance on hit item effects the reference leaves as
"TODO: Manual implementation required" blocks, because the client states no rate for them or the
reference's generator had no constructor for the row: `CHANCE_ON_HIT_ITEMS`, read from those
blocks and the client rows in data/, and docs/forever-chance-on-hit.md, which lists the items it
simulates by kind and the ones it leaves out with the reason.

write   Read the files from a checkout of the pin (--source, default the oracle cache) and write
        src/prepare/forever_items_generated.rs and docs/forever-chance-on-hit.md.
check   Translate again and fail when a committed file differs.
"""

import argparse
from pathlib import Path
import re
import sys

from compare import ROOT

OUTPUT = ROOT / "src" / "prepare" / "forever_items_generated.rs"
REPORT = ROOT / "docs" / "forever-chance-on-hit.md"
SPELL_ROWS = ROOT / "data" / "spells.jsonl"
ITEM_ROWS = ROOT / "data" / "items.jsonl"
DIRECTORY = "sim/common/forever/"
# RegisterAllEffects: RegisterAllOnUseCds, RegisterAllProcs, RegisterAllEnchants.
ITEM_FILES = ["stat_bonus_cds_auto_gen.go", "stat_bonus_procs_auto_gen.go"]
ENCHANT_FILES = ["enchants_auto_gen.go"]

STRING = r'"((?:[^"\\]|\\.)*)"'
ON_USE = {
    "NewSimpleStatActive": "SimpleStatActive",
    "NewSpellDataDamageOnUse": "Damage",
    "NewSpellDataHealOnUse": "Heal",
    "NewSpellDataAbsorbOnUse": "Absorb",
    "NewSpellDataSpeedOnUse": "Speed",
    "NewSpellDataAuraOnUse": "Aura",
    "NewSpellDataEnergizeOnUse": "Energize",
}
PROC = {
    "NewSpellDataProc": "Proc",
    "NewSpellDataDamageProc": "Damage",
    "NewSpellDataHealProc": "Heal",
    "NewSpellDataAbsorbProc": "Absorb",
    "NewSpellDataDebuffProc": "Debuff",
    "NewSpellDataAuraProc": "Aura",
    "NewSpellDataEquipAura": "EquipAura",
}
CALLBACKS = {
    "CallbackOnSpellHitDealt": "ON_SPELL_HIT_DEALT",
    "CallbackOnSpellHitTaken": "ON_SPELL_HIT_TAKEN",
    "CallbackOnPeriodicDamageDealt": "ON_PERIODIC_DAMAGE_DEALT",
    "CallbackOnHealDealt": "ON_HEAL_DEALT",
    "CallbackOnPeriodicHealDealt": "ON_PERIODIC_HEAL_DEALT",
    "CallbackOnCastComplete": "ON_CAST_COMPLETE",
    "CallbackOnApplyEffects": "ON_APPLY_EFFECTS",
    "CallbackOnPeriodicDamageTaken": "ON_PERIODIC_DAMAGE_TAKEN",
}


def snake(name):
    """CamelCase with acronyms as snake_case: MeleeMHAuto is melee_mh_auto, SpellID is spell_id."""
    return re.sub(r"(?<=[a-z0-9])(?=[A-Z])|(?<=[A-Z])(?=[A-Z][a-z])", "_", name).lower()


def rust_string(text):
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"') + '"'


def live_code(text):
    """The file without its comment lines."""
    return "\n".join(line for line in text.split("\n") if not line.lstrip().startswith("//"))


def trim_comment(line):
    return line.split("//")[0]


def terms(value):
    """The names in `core.A | core.B`."""
    return [part.strip().removeprefix("core.") for part in value.split("|")]


def duration(value):
    match = re.fullmatch(r"time\.Millisecond \* (\d+)", value.strip())
    if not match:
        raise ValueError(f"unknown duration {value}")
    return f"{match.group(1)} * MILLISECOND"


def mask(value, rust_type, convert):
    parts = [f"{rust_type}::{convert(part)}.0" for part in terms(value)]
    return f"{rust_type}({' | '.join(parts)})"


def proc_mask_name(part):
    return snake(part.removeprefix("ProcMask")).upper()


def flag_name(part):
    return snake(part.removeprefix("SpellFlag")).upper()


def outcome_name(part):
    return snake(part.removeprefix("Outcome")).upper()


def stacking(body):
    fields = dict(re.findall(r"(\w+):\s+(.*?),\n", body + ",\n"))
    out = []
    for name, value in fields.items():
        value = value.strip()
        if name == "Name":
            out.append(f"name: {value}")
        elif name == "ID":
            out.append(f"id: {value}")
        elif name == "CD":
            out.append(f"cd: {duration(value)}")
        elif name == "Callback":
            parts = [f"CallbackMask::{CALLBACKS[part]}.0" for part in terms(value)]
            out.append(f"callback: CallbackMask({' | '.join(parts)})")
        elif name == "ProcMask":
            out.append("proc_mask: " + mask(value, "ProcMask", proc_mask_name))
        elif name == "SpellFlags":
            out.append("spell_flags: " + mask(value, "SpellFlag", flag_name))
        elif name == "SpellFlagsExclude":
            out.append("spell_flags_exclude: " + mask(value, "SpellFlag", flag_name))
        elif name == "Outcome":
            out.append("outcome: " + mask(value, "HitOutcome", outcome_name))
        elif name in ("RequireDamageDealt", "TrinketLimitsDuration", "CanProcFromProcs"):
            out.append(f"{snake(name)}: {value}")
        else:
            raise ValueError(f"unknown StackingStatBonusCD field {name}")
    return out


def proc_config(body):
    out = []
    for name, value in re.findall(r"(\w+):\s*([^,]+)", body):
        value = value.strip()
        if name == "Name":
            out.append(f"name: {value}")
        elif name in ("EnchantID", "TriggerSpellID", "BuffSpellID"):
            out.append(f"{snake(name)}: {value}")
        elif name == "IsWeaponProc":
            out.append(f"is_weapon_proc: {value}")
        else:
            raise ValueError(f"unknown SpellDataProc field {name}")
    return out


def variants(text):
    return [
        f"ItemVariant {{ item_id: {item}, item_name: {rust_string(name)} }}"
        for item, name in re.findall(r"\{ItemID: (\d+), ItemName: " + STRING + r"\}", text)
    ]


def translate_file(text):
    """The live registrations of one generated file, in file order."""
    code = live_code(text)
    entries = []
    pattern = re.compile(
        r"shared\.(New\w+)\(((?:[^()]|\((?:[^()]|\([^()]*\))*\))*)\)",
        re.S,
    )
    for match in pattern.finditer(code):
        constructor, arguments = match.groups()
        arguments = re.sub(r"\s*//[^\n]*", "", arguments)
        if constructor in ON_USE:
            item = re.fullmatch(r"\s*(\d+)\s*", arguments)
            if not item:
                raise ValueError(f"unknown on-use arguments {arguments}")
            entries.append(f"Registration::OnUse(OnUseKind::{ON_USE[constructor]}, {item.group(1)})")
        elif constructor == "NewStackingStatBonusCD":
            body = re.fullmatch(r"\s*shared\.StackingStatBonusCD\{(.*)\}\s*", arguments, re.S).group(1)
            entries.append(
                "Registration::Stacking(StackingStatBonusCd { " + ", ".join(stacking(body)) + ", ..StackingStatBonusCd::EMPTY })"
            )
        elif constructor in PROC:
            struct = re.match(r"\s*shared\.SpellDataProc\{(.*?)\},\s*(nil|\[\]shared\.ItemVariant\{.*\})\s*$", arguments, re.S)
            fields = proc_config(struct.group(1))
            items = [] if struct.group(2) == "nil" else variants(struct.group(2))
            lines = ", ".join(fields)
            entries.append(
                f"Registration::Proc(ProcKind::{PROC[constructor]}, SpellDataProc {{ {lines}, ..SpellDataProc::EMPTY }}, "
                + "&[" + ", ".join(items) + "])"
            )
        else:
            raise ValueError(f"unknown constructor {constructor}")
    return entries


# ---------------------------------------------------------------------------------------------
# Forever's chance on hit items: the reference's "Manual implementation required" blocks.
# ---------------------------------------------------------------------------------------------

E_APPLY_AURA = 6
A_PERIODIC_DAMAGE = 3
A_DUMMY = 4
A_MOD_DECREASE_SPEED = 33
A_PERIODIC_MANA_LEECH = 64
A_MOD_RESISTANCE = 22
A_MOD_STAT = 29
A_MOD_HIT_CHANCE = 54
A_MOD_ATTACK_POWER = 99
A_MOD_RANGED_ATTACK_POWER = 124
A_MOD_CRIT_PCT = 290
A_MOD_MELEE_HASTE_3 = 319
TARGET_UNIT_CASTER = 1
TARGET_UNIT_TARGET_ENEMY = 6
TARGET_SRC_CASTER = 22
# The effects of a wearer buff the sim reads from its row (prepare/parse_effects.rs).
WEARER_BUFF_AURAS = {A_MOD_STAT, A_MOD_HIT_CHANCE, A_MOD_ATTACK_POWER, A_MOD_RANGED_ATTACK_POWER,
                     A_MOD_CRIT_PCT, A_MOD_MELEE_HASTE_3}
# Effects on the target beside a damage over time that change nothing the sim measures: a dummy,
# a movement slow and a mana drain.
DOT_COMPANIONS = {A_DUMMY, A_MOD_DECREASE_SPEED, A_PERIODIC_MANA_LEECH}
# The client's own reason an effect does nothing for a damage sim, by aura.
SKIPPED_AURAS = {
    4: "a scripted dummy aura the client does not describe", 5: "disorients the target", 7: "fears the target", 12: "stuns the target",
    23: "a periodic attack on enemies around the wielder", 27: "silences the target",
    30: "raises a defensive skill", 31: "changes run speed", 40: "makes the wielder immune",
    47: "raises parry", 51: "raises block", 53: "drains health from the target",
    65: "makes the next spell instant", 67: "disarms the target", 73: "lowers a spell's mana cost",
    103: "lowers threat", 226: "a periodic dummy", 232: "lengthens fear on the target",
    290: "raises the party's crit",
}
SKIPPED_EFFECTS = {
    2: "deals damage when the wearer is struck", 3: "a scripted dummy", 19: "grants an extra attack", 30: "restores mana or rage",
    38: "dispels the target", 63: "lowers threat",
}
# Chance on hit items whose condition the sim cannot read from the row.
SKIPPED_ITEMS = {
    269741: "procs only on attacks against Furbolg",
}
GROUP_A_KINDS = {
    "NewSpellDataProc": "Proc",
    "NewSpellDataDamageProc": "Damage",
    "NewSpellDataDebuffProc": "Debuff",
}


def json_rows(path, key):
    import json
    rows = {}
    for line in path.read_text().splitlines():
        if line.strip():
            row = json.loads(line)
            rows[row[key]] = row
    return rows


def manual_blocks(text):
    """The comment lines of each "Manual implementation required" block, prefixes stripped."""
    lines = text.split("\n")
    blocks, i = [], 0
    while i < len(lines):
        if "TODO: Manual implementation required" not in lines[i]:
            i += 1
            continue
        body, i = [], i + 1
        while i < len(lines) and lines[i].strip().startswith("//"):
            body.append(lines[i].strip()[2:].strip())
            i += 1
        blocks.append(body)
    return blocks


def block_fields(body):
    text = "\n".join(body)
    spell = re.search(r"spell=(\d+)", text)
    reason = re.search(r"unsupported: (.*)", text)
    trigger = re.search(r"trigger \d+ \(([^)]*)\)", text)
    constructor = re.search(r"shared\.(New\w+)\(shared\.\w+\{([^}]*)\}", text)
    description = " ".join(
        line for line in body[3:]
        if line and not re.match(r"(https://|shared\.|unsupported|trigger |[{}\[]|Callback|ProcMask|Outcome|RequireDamage)", line)
    )
    cfg = dict(re.findall(r"(\w+):\s*([^,]+)", constructor.group(2))) if constructor else {}
    return {
        "spell": int(spell.group(1)) if spell else 0,
        "reason": reason.group(1).strip() if reason else None,
        "trigger_mask": trigger.group(1) if trigger else "",
        "constructor": constructor.group(1) if constructor else None,
        "cfg": {name: value.strip() for name, value in cfg.items()},
        "description": description,
        "variants": [(int(item), name) for item, name in re.findall(r"\{ItemID: (\d+), ItemName: " + STRING + r"\}", text)],
    }


def auras(row, target=None):
    return [e.get("Aura", 0) for e in row.get("Effects", [])
            if e.get("Type") == E_APPLY_AURA and (target is None or e.get("Target", [0])[0] == target)]


def enemy_auras(row):
    """The auras the row applies anywhere but on the caster."""
    return [e.get("Aura", 0) for e in row.get("Effects", [])
            if e.get("Type") == E_APPLY_AURA and e.get("Target", [0])[0] != TARGET_UNIT_CASTER]


def chance_on_hit_entry(item, spell):
    """The item's chance on hit effect entry for the spell: the exporter marks them with `proc`."""
    for entry in (item or {}).get("item_effects") or []:
        if entry.get("buff_id") == spell and "proc" in entry:
            return entry
    return None


def is_weapon(item):
    return item.get("type") in ("ItemTypeWeapon", "ItemTypeRanged")


def classify_rateless(fields, item, spells):
    """Group (a): a constructor the reference knows, with no rate. (kind, cfg) or a skip reason."""
    cfg = fields["cfg"]
    if fields["reason"] != "states no rate":
        return fields["reason"]
    if item["id"] in SKIPPED_ITEMS:
        return SKIPPED_ITEMS[item["id"]]
    kind = GROUP_A_KINDS.get(fields["constructor"])
    trigger = int(cfg["TriggerSpellID"])
    buff = int(cfg.get("BuffSpellID", 0))
    row = spells[buff or trigger]
    weapon = cfg.get("IsWeaponProc") == "true"
    if not weapon and not re.search(r"ProcMask(Melee|Ranged)", fields["trigger_mask"]):
        return "procs from spells, which the assumed rate per weapon hit does not cover"
    if spells[trigger].get("ICDMs"):
        return "its trigger states an internal cooldown, which a weapon proc's export does not carry"
    if kind == "Proc":
        if TARGET_UNIT_CASTER not in [e.get("Target", [0])[0] for e in row["Effects"]]:
            return "buffs an ally"
        # A row whose buff also changes speed is read whole from the row.
        if A_MOD_MELEE_HASTE_3 in auras(row, TARGET_UNIT_CASTER):
            kind = "Aura"
    elif kind == "Damage":
        if any(aura not in {A_PERIODIC_DAMAGE} | DOT_COMPANIONS for aura in enemy_auras(row)):
            return "deals damage and debuffs the target"
    elif kind == "Debuff":
        on_target = auras(row, TARGET_UNIT_TARGET_ENEMY)
        armor = [e for e in row["Effects"] if e.get("Aura") == A_MOD_RESISTANCE]
        if A_PERIODIC_DAMAGE in on_target:
            return "deals damage over time and debuffs the target"
        if not armor or any(e.get("Misc", 0) != 1 for e in armor) or row.get("MaxStack"):
            return "weakens the target's attacks, speed or resistances, which only a tank feels"
    else:
        return {
            "NewSpellDataAuraProc": "raises the target's damage taken, which the runtime has no debuff proc for",
            "NewSpellDataHealProc": "heals the wielder",
            "NewSpellDataAbsorbProc": "shields the wielder",
        }[fields["constructor"]]
    return kind, {"trigger_spell_id": trigger, "buff_spell_id": buff, "is_weapon_proc": weapon}


def classify_unclassified(fields, item, spells):
    """Group (b): a row the reference's generator had no constructor for."""
    spell = fields["spell"]
    row = spells.get(spell)
    if row is None:
        return "its spell is not in the client data"
    if not is_weapon(item):
        return "a chance on hit of an armor piece the client states no trigger for"
    effects = row.get("Effects", [])
    for effect in effects:
        if effect.get("Type") != E_APPLY_AURA:
            return SKIPPED_EFFECTS.get(effect.get("Type"), f"client effect {effect.get('Type')}")
    targets = {e.get("Target", [0])[0] for e in effects}
    applied = auras(row)
    cfg = {"trigger_spell_id": spell, "buff_spell_id": spell, "is_weapon_proc": True}
    if A_PERIODIC_DAMAGE in applied:
        if targets == {TARGET_SRC_CASTER} and set(applied) == {A_PERIODIC_DAMAGE}:
            return "Damage", cfg
        if targets == {TARGET_UNIT_TARGET_ENEMY} and set(applied) <= {A_PERIODIC_DAMAGE} | DOT_COMPANIONS:
            return "Damage", cfg
        return "deals damage over time and debuffs the target"
    if targets == {TARGET_UNIT_CASTER} and set(applied) <= WEARER_BUFF_AURAS:
        return "Aura", {"trigger_spell_id": spell, "buff_spell_id": 0, "is_weapon_proc": True}
    for aura in applied:
        if aura in SKIPPED_AURAS:
            return SKIPPED_AURAS[aura]
    return f"client aura {applied}"


def chance_on_hit_items(source):
    """The Forever registrations and the report rows: (registrations, simulated, skipped)."""
    spells = json_rows(SPELL_ROWS, "ID")
    items = json_rows(ITEM_ROWS, "id")
    text = (source / DIRECTORY / "stat_bonus_procs_auto_gen.go").read_text()
    registrations, simulated, skipped = [], [], []
    for body in manual_blocks(text):
        fields = block_fields(body)
        grouped = {}
        for item_id, name in fields["variants"]:
            item = items.get(item_id)
            if item is None or chance_on_hit_entry(item, fields["spell"]) is None:
                continue
            if fields["constructor"] == "NewProcStatBonusEffectWithVariants":
                result = classify_unclassified(fields, item, spells)
            else:
                result = classify_rateless(fields, item, spells)
            ilvl = ((item.get("scaling_options") or {}).get("0") or {}).get("ilvl", 0)
            row = (item_id, name, ilvl, fields["spell"], fields["description"])
            if isinstance(result, str):
                skipped.append(row + (result,))
                continue
            kind, cfg = result
            simulated.append(row + (kind,))
            grouped.setdefault((kind, tuple(sorted(cfg.items()))), []).append((item_id, name))
        for (kind, cfg), variants in grouped.items():
            fields_out = [f"{name}: {str(value).lower() if isinstance(value, bool) else value}"
                          for name, value in cfg if value not in (0, False)]
            fields_out.append("rateless: true")
            names = ", ".join(f"ItemVariant {{ item_id: {item}, item_name: {rust_string(name)} }}" for item, name in variants)
            registrations.append(
                f"Registration::Proc(ProcKind::{kind}, SpellDataProc {{ {', '.join(fields_out)}, ..SpellDataProc::EMPTY }}, &[{names}])"
            )
    return registrations, simulated, skipped


KIND_TITLES = {
    "Damage": "Damage: a hit, a damage over time on the target or on every enemy around the wielder",
    "Aura": "Wearer buffs read from the row: attack speed, crit, hit and stats",
    "Proc": "Wearer stat buffs from the item's own stats",
    "Debuff": "Armor reduction on the target",
}


def report(simulated, skipped):
    def cell(text):
        return text.replace("|", "/")

    lines = [
        "# Forever chance on hit items",
        "",
        "Generated by tools/rust_item_effects.py from the pinned reference's",
        "stat_bonus_procs_auto_gen.go \"Manual implementation required\" blocks and the client rows in",
        "data/. Do not edit.",
        "",
        "Under Forever's rules these items' chance on hit rolls 1.8% per second of the hitting weapon's",
        "speed, 1.08 procs a minute: the rate Classic servers give a `ProcChance` of 101, which the",
        "client states for every one of them with no procs per minute. Blizzard states none for",
        "Forever; it is MythicSim's assumption (README, Rust-only mechanics). The reference leaves them",
        "all unregistered, so before this a sim wearing one ran as if it had no effect.",
        "",
        f"{len(simulated)} items simulated, {len(skipped)} left out.",
        "",
    ]
    for kind in ("Damage", "Aura", "Proc", "Debuff"):
        rows = sorted((r for r in simulated if r[5] == kind), key=lambda r: (-r[2], r[0]))
        if not rows:
            continue
        lines += [f"## {KIND_TITLES[kind]}", "", "| Item | Item level | Spell | Effect |", "| --- | --- | --- | --- |"]
        lines += [f"| {r[1]} ({r[0]}) | {r[2]} | {r[3]} | {cell(r[4])} |" for r in rows]
        lines.append("")
    lines += ["## Left out", "", "| Item | Item level | Spell | Reason | Effect |", "| --- | --- | --- | --- | --- |"]
    for r in sorted(skipped, key=lambda r: (r[5], -r[2], r[0])):
        lines.append(f"| {r[1]} ({r[0]}) | {r[2]} | {r[3]} | {cell(r[5])} | {cell(r[4])} |")
    lines.append("")
    return "\n".join(lines)


def translate(source):
    items, enchants = [], []
    for name in ITEM_FILES:
        items += translate_file((source / DIRECTORY / name).read_text())
    for name in ENCHANT_FILES:
        enchants += translate_file((source / DIRECTORY / name).read_text())
    lines = [
        "//! Generated by tools/rust_item_effects.py from the pinned reference's",
        "//! sim/common/forever stat_bonus_cds_auto_gen.go, stat_bonus_procs_auto_gen.go and",
        "//! enchants_auto_gen.go: the live registrations, in Go's registration order. Do not edit.",
        "",
        "use super::aura_helpers::{CallbackMask, HitOutcome};",
        "use super::forever_items::Registration;",
        "use super::shared_items::{ItemVariant, ProcKind, SpellDataProc};",
        "use super::shared_on_use::{OnUseKind, StackingStatBonusCd};",
        "use super::sim::MILLISECOND;",
        "use super::spell::ProcMask;",
        "",
        "/// `RegisterAllOnUseCds`, then `RegisterAllProcs`.",
        "#[rustfmt::skip]",
        "pub(crate) const ITEMS: &[Registration] = &[",
    ]
    lines += [f"    {entry}," for entry in items]
    lines += ["];", "", "/// `RegisterAllEnchants`.", "#[rustfmt::skip]", "pub(crate) const ENCHANTS: &[Registration] = &["]
    lines += [f"    {entry}," for entry in enchants]
    registrations, simulated, skipped = chance_on_hit_items(source)
    lines += [
        "];",
        "",
        "/// Under Forever's rules: the chance on hit item effects the reference leaves unregistered,",
        "/// read from its \"Manual implementation required\" blocks and the client rows, which roll the",
        "/// rate MythicSim assumes (`shared_items::CHANCE_ON_HIT_PPM`). docs/forever-chance-on-hit.md",
        "/// lists them and the ones left out.",
        "#[rustfmt::skip]",
        "pub(crate) const CHANCE_ON_HIT_ITEMS: &[Registration] = &[",
    ]
    lines += [f"    {entry}," for entry in registrations]
    lines += ["];", ""]
    text = "\n".join(lines)
    if "SpellFlag(" in text:
        text = text.replace("use super::spell::ProcMask;", "use super::spell::{ProcMask, SpellFlag};")
    return text, report(simulated, skipped)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=["write", "check"])
    parser.add_argument("--source", type=Path, default=ROOT / "oracle-cache" / "source",
                        help="a checkout of the pinned reference")
    args = parser.parse_args()
    text, items = translate(args.source)
    if args.command == "write":
        OUTPUT.parent.mkdir(parents=True, exist_ok=True)
        OUTPUT.write_text(text)
        REPORT.write_text(items)
        print(f"Wrote {OUTPUT.relative_to(ROOT)} and {REPORT.relative_to(ROOT)}")
        return 0
    for path, want in ((OUTPUT, text), (REPORT, items)):
        if not path.exists() or path.read_text() != want:
            print(f"{path.relative_to(ROOT)} is not the translation of the pinned registrations", file=sys.stderr)
            return 1
    print("Generated item effects match the pinned reference.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
