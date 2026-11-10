#!/usr/bin/env python3
"""Import the game data Rust preparation reads from the pinned Go reference.

check   Offline audit: every file data/manifest.json lists exists with its digest, and the
        manifest names the current pin. Needs Python only.
import  Build tools/rust-data in the oracle's checkout of the pin, run it into scratch and
        write data/ from its output, with data/manifest.json. Needs Go and Git.

The importer keeps Go's meaning where the raw export would not: the item database keeps the
first row of each id, as core/database.go addToDatabase does when leftover_db.bin repeats
one, and the client spell rows drop zero fields, which Rust reads back as zero.
"""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys

from compare import ROOT, PIN, CLIENT_BUILD, build_oracle, command, go_pin_flags, load

DATA = ROOT / "data"
TOOL = ROOT / "tools" / "rust-data"
MANIFEST = DATA / "manifest.json"

# These externally sourced projections are not emitted by the Go rust-data tool.
# Reimport must retain them *and* their hashes, including the playable L20 actor.
PRESERVED_NATIVE_DATA = (
    "item-required-levels.jsonl",
    "item-required-levels.manifest.json",
    "shaman-equipment-skills.json",
    "shaman-equipment-skills.manifest.json",
    "druid-equipment-skills.json",
    "druid-equipment-skills.manifest.json",
    "paladin-equipment-skills.json",
    "paladin-equipment-skills.manifest.json",
    "rogue-equipment-skills.json",
    "rogue-equipment-skills.manifest.json",
    "warlock-equipment-skills.json",
    "warlock-equipment-skills.manifest.json",
    "paladin-l20-source-context.json",
    "warlock-l10-l40-actor-source.json",
    "warlock-l41-l59-actor-source.json",
    "warlock-l20-actor-source.json",
    "priest-l20-base-stats.json",
    "rogue-l20-base-stats.json",
    "druid-l20-base-stats.json",
    "druid-l30-base-stats.json",
)

# SimDatabase list, output file and the key each row is found by.
DATABASE_TABLES = [
    ("items", "items.jsonl", "id"),
    ("random_suffixes", "random-suffixes.jsonl", "id"),
    ("enchants", "enchants.jsonl", "effect_id"),
    ("gems", "gems.jsonl", "id"),
    ("item_effect_rand_prop_points", "rand-prop-points.jsonl", "ilvl"),
    ("consumables", "consumables.jsonl", "id"),
    ("spell_effects", "spell-effects.jsonl", "id"),
]


def tool_files():
    return sorted(path for path in TOOL.iterdir() if path.is_file())


def tool_digest():
    digest = hashlib.sha256()
    for path in tool_files():
        digest.update(path.name.encode() + b"\0" + path.read_bytes() + b"\0")
    return digest.hexdigest()


def file_digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def compact(value):
    """A spell row without its zero fields: Rust reads an absent field as zero."""
    if isinstance(value, dict):
        out = {}
        for key, item in value.items():
            item = compact(item)
            if item in (0, "", None, False, [], {}):
                continue
            if isinstance(item, list) and all(entry == 0 for entry in item):
                continue
            out[key] = item
        return out
    if isinstance(value, list):
        return [compact(item) for item in value]
    return value


def json_lines(rows, key):
    """One row per line, sorted by key, each line starting with its key so Rust can index
    the file without parsing the rest of the row."""
    lines = []
    for row in sorted(rows, key=lambda row: row.get(key, 0)):
        ordered = {key: row.get(key, 0)}
        ordered.update((name, value) for name, value in row.items() if name != key)
        lines.append(json.dumps(ordered, separators=(",", ":"), ensure_ascii=False))
    return "\n".join(lines) + "\n"


def first_of_each(rows, key):
    """Go keeps the first row of each key; a later one is ignored."""
    seen, out = set(), []
    for row in rows:
        value = row.get(key, 0)
        if value in seen:
            continue
        seen.add(value)
        out.append(row)
    return out


def build_tool(cache, source):
    build_oracle(cache, source)
    checkout = cache / "source"
    target = checkout / "cmd" / "rust-data"
    target.mkdir(exist_ok=True)
    for stale in target.glob("*.go"):
        stale.unlink()
    shutil.copy2(TOOL / "main.go", target / "main.go")
    shutil.copy2(TOOL / "spelldata_export.go.in",
                 checkout / "sim" / "core" / "spelldata" / "zz_rust_data_export.go")
    shutil.copy2(TOOL / "core_export.go.in", checkout / "sim" / "core" / "zz_rust_data_export.go")
    binary = (cache / "rust-data").resolve()
    command(["go", "build", "-trimpath", "--tags=with_db", *go_pin_flags(), "-o", str(binary),
             "./cmd/rust-data"], cwd=checkout)
    return binary


def write_data(raw):
    DATA.mkdir(exist_ok=True)
    files = {}

    def emit(name, text):
        path = DATA / name
        path.write_text(text, encoding="utf-8")
        files[name] = file_digest(path)

    emit("proto-schema.json", (raw / "proto-schema.json").read_text(encoding="utf-8"))
    database = load(raw / "sim-database.json")
    unknown = set(database) - {table for table, _, _ in DATABASE_TABLES}
    if unknown:
        raise ValueError(f"unknown SimDatabase lists: {sorted(unknown)}")
    for table, name, key in DATABASE_TABLES:
        emit(name, json_lines(first_of_each(database.get(table, []), key), key))
    spells = load(raw / "spells.json")
    if set(spells) != {"spells", "curves", "hand_triggers"}:
        raise ValueError(f"unexpected spell export sections: {sorted(spells)}")
    emit("spells.jsonl", json_lines([compact(row) for row in spells["spells"]], "ID"))
    extras = {"curves": spells["curves"], "hand_triggers": spells["hand_triggers"]}
    emit("spell-extras.json", json.dumps(extras, sort_keys=True, separators=(",", ":")) + "\n")
    emit("go-tables.json", json.dumps(load(raw / "go-tables.json"), indent=1, sort_keys=True) + "\n")
    # Preserve separately sourced native actor and client projections across reimports.
    # Their authority is the referenced source, not the Go database exporter.
    for name in PRESERVED_NATIVE_DATA:
        path = DATA / name
        if path.exists():
            files[name] = file_digest(path)
    manifest = {
        "schema_version": 1,
        "engine_revision": PIN,
        "client_build": CLIENT_BUILD,
        "tool": "tools/rust-data",
        "tool_sha256": tool_digest(),
        "files": dict(sorted(files.items())),
    }
    MANIFEST.write_text(json.dumps(manifest, indent=2) + "\n")
    return manifest


def check():
    manifest = load(MANIFEST)
    errors = []
    if manifest.get("engine_revision") != PIN:
        errors.append(f"data was imported at {manifest.get('engine_revision')}, the pin is {PIN}")
    if manifest.get("client_build") != CLIENT_BUILD:
        errors.append(f"data client build {manifest.get('client_build')} is not {CLIENT_BUILD}")
    if manifest.get("tool_sha256") != tool_digest():
        errors.append("tools/rust-data changed since the data was imported")
    source_manifest_path = DATA / "item-required-levels.manifest.json"
    if not source_manifest_path.exists():
        errors.append("missing native ItemSparse.RequiredLevel source manifest")
    else:
        source_manifest = load(source_manifest_path)
        source = source_manifest.get("source", {})
        if source.get("provider") != "wago.tools":
            errors.append("native item-level source is not the pinned Wago DB2 export")
        if source.get("client_build") != CLIENT_BUILD:
            errors.append("native ItemSparse source build differs from the pinned client build")
        if source.get("table") != "ItemSparse" or source.get("field") != "RequiredLevel":
            errors.append("native item-level source is not ItemSparse.RequiredLevel")
        if not source.get("csv_sha256"):
            errors.append("native ItemSparse source manifest lacks its CSV digest")
        if "item-required-levels.manifest.json" not in manifest.get("files", {}):
            errors.append("native ItemSparse source manifest is not pinned by data/manifest.json")
        levels_path = DATA / "item-required-levels.jsonl"
        if not levels_path.exists() or not levels_path.read_text(encoding="utf-8").strip():
            errors.append("native ItemSparse.RequiredLevel projection is empty")
        else:
            rows = [json.loads(line) for line in levels_path.read_text(encoding="utf-8").splitlines()
                    if line.strip()]
            ids = [int(row["id"]) for row in rows]
            if len(ids) != len(set(ids)):
                errors.append("native ItemSparse.RequiredLevel projection contains duplicate IDs")
            if ids != sorted(ids):
                errors.append("native ItemSparse.RequiredLevel projection is not sorted by item ID")
            if any(int(row["required_level"]) < 0 for row in rows):
                errors.append("native ItemSparse.RequiredLevel projection contains a negative level")
            if int(source_manifest.get("matched_item_count", -1)) != len(rows):
                errors.append("native ItemSparse source manifest row count differs from the projection")
            native_count = int(source_manifest.get("native_item_count", 0))
            missing_count = int(source_manifest.get("native_items_without_item_sparse_row", -1))
            if native_count <= 0 or missing_count != native_count - len(rows):
                errors.append("native ItemSparse source manifest item counts are inconsistent")
    shaman_manifest_path = DATA / "shaman-equipment-skills.manifest.json"
    shaman_skills_path = DATA / "shaman-equipment-skills.json"
    if not shaman_manifest_path.exists():
        errors.append("missing native Shaman SkillLine/SkillRaceClassInfo source manifest")
    else:
        shaman_source_manifest = load(shaman_manifest_path)
        shaman_source = shaman_source_manifest.get("source", {})
        if shaman_source.get("provider") != "wago.tools":
            errors.append("native Shaman equipment source is not the pinned Wago DB2 export")
        if shaman_source.get("client_build") != CLIENT_BUILD:
            errors.append("native Shaman equipment source build differs from the pinned client build")
        tables = shaman_source.get("tables", {})
        for table in ("SkillLine", "SkillRaceClassInfo"):
            table_source = tables.get(table, {})
            if not table_source.get("csv_sha256") or int(table_source.get("csv_row_count", 0)) <= 0:
                errors.append(f"native Shaman equipment source manifest lacks {table} provenance")
        if "shaman-equipment-skills.manifest.json" not in manifest.get("files", {}):
            errors.append("native Shaman equipment source manifest is not pinned by data/manifest.json")
        if "shaman-equipment-skills.json" not in manifest.get("files", {}):
            errors.append("native Shaman equipment projection is not pinned by data/manifest.json")
        if not shaman_skills_path.exists() or not shaman_skills_path.read_text(encoding="utf-8").strip():
            errors.append("native Shaman equipment projection is empty")
        else:
            shaman_skills = load(shaman_skills_path)
            if int(shaman_skills.get("schema_version", 0)) != 2:
                errors.append("native Shaman equipment projection lacks the complete SkillLine name index")
            if shaman_skills.get("class") != "shaman" or int(shaman_skills.get("class_id", 0)) != 7:
                errors.append("native Shaman equipment projection has the wrong class identity")
            if int(shaman_skills.get("class_mask", 0)) != 64:
                errors.append("native Shaman equipment projection has the wrong class mask")
            if shaman_skills.get("client_build") != CLIENT_BUILD:
                errors.append("native Shaman equipment projection has the wrong client build")
            if shaman_skills.get("race_policy") != "class-wide only; SkillRaceClassInfo race mask must be unrestricted":
                errors.append("native Shaman equipment projection has an unknown race policy")
            skill_line_ids = shaman_skills.get("skill_line_ids", [])
            skill_line_names = shaman_skills.get("skill_line_names", [])
            indexed_skill_ids = [int(line["skill_id"]) for line in skill_line_names]
            if (
                not skill_line_ids
                or skill_line_ids != sorted(set(int(value) for value in skill_line_ids))
                or indexed_skill_ids != skill_line_ids
                or len({line["name"] for line in skill_line_names}) < 1
            ):
                errors.append("native Shaman equipment projection has an incomplete SkillLine name index")
            skill_names_by_id = {
                int(line["skill_id"]): line["name"] for line in skill_line_names
            }
            required_skill_lines = {
                "Axes", "Swords", "Maces", "Daggers", "Fist Weapons",
                "Two-Handed Axes", "Two-Handed Swords", "Two-Handed Maces",
                "Polearms", "Staves", "Bows", "Guns", "Crossbows", "Thrown",
                "Cloth", "Leather", "Mail", "Plate Mail", "Shield",
            }
            if not required_skill_lines.issubset({line["name"] for line in skill_line_names}):
                errors.append("native Shaman equipment projection is missing an equipment SkillLine name")
            skills = shaman_skills.get("skills", [])
            skill_ids = [int(skill["skill_id"]) for skill in skills]
            if not skills or len(skill_ids) != len(set(skill_ids)):
                errors.append("native Shaman equipment projection has no skills or duplicate skill IDs")
            for skill in skills:
                if skill_names_by_id.get(int(skill["skill_id"])) != skill.get("name"):
                    errors.append(f"native Shaman skill {skill.get('name')} does not match the SkillLine index")
            for skill in skills:
                if skill.get("min_level") is not None and (
                    not skill.get("race_unrestricted")
                    or not 1 <= int(skill["min_level"]) <= 60
                ):
                    errors.append(f"native Shaman skill {skill.get('name')} is not a valid class-wide level")

    for name, digest in manifest["files"].items():
        path = DATA / name
        if not path.exists():
            errors.append(f"missing data/{name}")
        elif file_digest(path) != digest:
            errors.append(f"data/{name} differs from its manifest digest")
    listed = set(manifest["files"]) | {"manifest.json"}
    for path in DATA.iterdir():
        if path.name not in listed:
            errors.append(f"data/{path.name} is not in the manifest")
    if errors:
        raise ValueError("\n".join(errors))
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=["check", "import"], nargs="?", default="check")
    parser.add_argument("--source", default="https://github.com/sage3648/mythicsim-forever-engine-go.git",
                        help="local Git repository or clone URL")
    parser.add_argument("--cache", type=Path, default=ROOT / "oracle-cache")
    args = parser.parse_args()
    try:
        if args.command == "import":
            binary = build_tool(args.cache, args.source)
            raw = args.cache / "rust-data-output"
            if raw.exists():
                shutil.rmtree(raw)
            command([str(binary), str(raw)])
            manifest = write_data(raw)
            print(f"Imported {len(manifest['files'])} data files at {PIN}")
        else:
            manifest = check()
            print(f"Rust data checked: {len(manifest['files'])} files at {manifest['engine_revision']}.")
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        print(f"rust data failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
