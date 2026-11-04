#!/usr/bin/env python3
"""Import Shaman equipment skill eligibility from the pinned client's Wago DB2 tables.

The projection is deliberately class-wide: a skill is marked available only when every
race represented by the pinned SkillRaceClassInfo rows for Shaman has that skill.
Race-specific proficiencies are never silently widened to all Shamans.
"""

import argparse
import csv
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / "data"
SHAMAN_CLASS_ID = 7
SHAMAN_CLASS_MASK = 1 << (SHAMAN_CLASS_ID - 1)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n", encoding="utf-8")


def read_csv(path, table):
    with Path(path).open("r", encoding="utf-8-sig", newline="") as source:
        reader = csv.DictReader(source)
        if not reader.fieldnames:
            raise ValueError(f"{table} CSV has no header")
        return reader.fieldnames, list(reader)


def field(row, names, table, line_number, required=True, default=None):
    for name in names:
        if name in row:
            value = row[name]
            if value is not None and str(value).strip() != "":
                return value
    if required:
        raise ValueError(f"{table} row {line_number} is missing one of {', '.join(names)}")
    return default


def integer(row, names, table, line_number, required=True, default=0):
    raw = field(row, names, table, line_number, required=required, default=default)
    try:
        return int(raw)
    except (TypeError, ValueError) as error:
        raise ValueError(f"{table} row {line_number} has invalid {names[0]} value {raw!r}") from error


def race_mask(row, table, line_number):
    if "RaceMask" in row and str(row["RaceMask"] or "").strip():
        return integer(row, ("RaceMask",), table, line_number)
    low = integer(row, ("RaceMask1", "RaceMask_0"), table, line_number, required=False, default=0)
    high = integer(row, ("RaceMask2", "RaceMask_1"), table, line_number, required=False, default=0)
    if not any(name in row for name in ("RaceMask1", "RaceMask_0", "RaceMask2", "RaceMask_1")):
        raise ValueError(f"{table} CSV has no RaceMask, RaceMask1/RaceMask2, or RaceMask_0/RaceMask_1")
    return low | (high << 32)


def import_shaman_equipment(skill_line_csv, skill_race_class_info_csv, data_dir=DATA):
    data_dir = Path(data_dir)
    main_manifest_path = data_dir / "manifest.json"
    main_manifest = json.loads(main_manifest_path.read_text(encoding="utf-8"))
    client_build = main_manifest.get("client_build")
    if not client_build:
        raise ValueError("data/manifest.json has no pinned client_build")

    skill_line_path = Path(skill_line_csv)
    skill_info_path = Path(skill_race_class_info_csv)
    skill_line_fields, skill_line_rows = read_csv(skill_line_path, "SkillLine")
    skill_info_fields, skill_info_rows = read_csv(skill_info_path, "SkillRaceClassInfo")

    skill_lines = {}
    for line_number, row in enumerate(skill_line_rows, 2):
        skill_id = integer(row, ("ID",), "SkillLine", line_number)
        if skill_id in skill_lines:
            raise ValueError(f"duplicate SkillLine ID {skill_id}")
        name = str(field(
            row,
            ("DisplayName_lang", "DisplayName", "Name_lang", "Name", "DisplayName_lang_enUS"),
            "SkillLine",
            line_number,
        )).strip()
        if not name:
            raise ValueError(f"SkillLine ID {skill_id} has an empty display name")
        category_id = integer(row, ("CategoryID",), "SkillLine", line_number, required=False, default=0)
        skill_lines[skill_id] = {"name": name, "category_id": category_id}

    raw_rows = []
    shaman_race_mask = 0
    shaman_class_rows = 0
    for line_number, row in enumerate(skill_info_rows, 2):
        skill_id = integer(row, ("SkillID",), "SkillRaceClassInfo", line_number)
        class_mask = integer(row, ("ClassMask", "ClassMask1", "ClassMask_0"), "SkillRaceClassInfo", line_number)
        mask = race_mask(row, "SkillRaceClassInfo", line_number)
        raw_min_level = integer(row, ("MinLevel",), "SkillRaceClassInfo", line_number)
        if class_mask != 0 and not (class_mask & SHAMAN_CLASS_MASK):
            continue
        if class_mask & SHAMAN_CLASS_MASK:
            shaman_class_rows += 1
            if mask:
                shaman_race_mask |= mask
        raw_rows.append({
            "skill_id": skill_id,
            "class_mask": class_mask,
            "race_mask": mask,
            "source_min_level": raw_min_level,
            "min_level": max(1, raw_min_level),
        })

    if not shaman_class_rows:
        raise ValueError("SkillRaceClassInfo contains no rows for Shaman class mask 64")
    if not skill_lines:
        raise ValueError("SkillLine contains no rows")

    by_skill = {}
    for row in raw_rows:
        if row["skill_id"] not in skill_lines:
            raise ValueError(f"SkillRaceClassInfo references missing SkillLine ID {row['skill_id']}")
        by_skill.setdefault(row["skill_id"], []).append(row)

    race_ids = [
        bit_index + 1
        for bit_index in range(shaman_race_mask.bit_length())
        if shaman_race_mask & (1 << bit_index)
    ]
    skills = []
    for skill_id, rows in by_skill.items():
        if shaman_race_mask:
            per_race = {}
            for race_id in race_ids:
                bit = 1 << (race_id - 1)
                available = [
                    row["min_level"]
                    for row in rows
                    if row["race_mask"] == 0 or row["race_mask"] & bit
                ]
                per_race[str(race_id)] = min(available) if available else None
            min_level = (
                max(per_race.values())
                if per_race and all(value is not None for value in per_race.values())
                else None
            )
        else:
            universal = [row["min_level"] for row in rows if row["race_mask"] == 0]
            per_race = {}
            min_level = min(universal) if universal else None

        skills.append({
            "skill_id": skill_id,
            "name": skill_lines[skill_id]["name"],
            "category_id": skill_lines[skill_id]["category_id"],
            "min_level": min_level,
            "per_race_min_level": per_race,
            "source_rows": len(rows),
        })
    skills.sort(key=lambda item: (item["name"].casefold(), item["skill_id"]))

    output = {
        "schema_version": 1,
        "client_build": client_build,
        "class": "shaman",
        "class_id": SHAMAN_CLASS_ID,
        "class_mask": SHAMAN_CLASS_MASK,
        "race_mask": shaman_race_mask,
        "race_ids": race_ids,
        "skills": skills,
    }
    output_path = data_dir / "shaman-equipment-skills.json"
    write_json(output_path, output)

    source_manifest = {
        "schema_version": 1,
        "source": {
            "provider": "wago.tools",
            "client_build": client_build,
            "tables": {
                "SkillLine": {
                    "url": f"https://wago.tools/db2/SkillLine/csv?build={client_build}",
                    "csv_sha256": digest(skill_line_path),
                    "csv_row_count": len(skill_line_rows),
                    "columns": skill_line_fields,
                },
                "SkillRaceClassInfo": {
                    "url": f"https://wago.tools/db2/SkillRaceClassInfo/csv?build={client_build}",
                    "csv_sha256": digest(skill_info_path),
                    "csv_row_count": len(skill_info_rows),
                    "columns": skill_info_fields,
                },
            },
        },
        "class_id": SHAMAN_CLASS_ID,
        "class_mask": SHAMAN_CLASS_MASK,
        "shaman_race_mask": shaman_race_mask,
        "shaman_race_ids": race_ids,
        "shaman_class_skill_row_count": shaman_class_rows,
        "projected_skill_count": len(skills),
        "class_wide_skill_count": sum(skill["min_level"] is not None for skill in skills),
        "race_specific_or_unavailable_skill_count": sum(skill["min_level"] is None for skill in skills),
    }
    source_manifest_path = data_dir / "shaman-equipment-skills.manifest.json"
    write_json(source_manifest_path, source_manifest)

    main_manifest.setdefault("files", {})
    main_manifest["files"]["shaman-equipment-skills.json"] = digest(output_path)
    main_manifest["files"]["shaman-equipment-skills.manifest.json"] = digest(source_manifest_path)
    main_manifest["files"] = dict(sorted(main_manifest["files"].items()))
    write_json(main_manifest_path, main_manifest)

    return {
        "client_build": client_build,
        "shaman_race_mask": shaman_race_mask,
        "shaman_race_ids": race_ids,
        "shaman_class_skill_row_count": shaman_class_rows,
        "projected_skill_count": len(skills),
        "class_wide_skill_count": source_manifest["class_wide_skill_count"],
        "race_specific_or_unavailable_skill_count": source_manifest["race_specific_or_unavailable_skill_count"],
        "output_sha256": digest(output_path),
        "skill_line_csv_sha256": digest(skill_line_path),
        "skill_race_class_info_csv_sha256": digest(skill_info_path),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("skill_line_csv", type=Path, help="Wago SkillLine CSV for the pinned client build")
    parser.add_argument("skill_race_class_info_csv", type=Path, help="Wago SkillRaceClassInfo CSV for the pinned client build")
    parser.add_argument("--data-dir", type=Path, default=DATA)
    args = parser.parse_args()
    try:
        print(json.dumps(
            import_shaman_equipment(args.skill_line_csv, args.skill_race_class_info_csv, args.data_dir),
            sort_keys=True,
        ))
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as error:
        print(f"Shaman equipment import failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
