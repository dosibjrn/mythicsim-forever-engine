#!/usr/bin/env python3
"""Import Paladin equipment skill eligibility from the pinned client's Wago DB2 tables.

The native item-options API has no race parameter, so this projection only treats a skill
as class-wide when SkillRaceClassInfo has an unrestricted race mask for Paladin. A
race-restricted skill is preserved as unavailable instead of being widened.
"""

import argparse
import csv
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / "data"
PALADIN_CLASS_ID = 2
PALADIN_CLASS_MASK = 1 << (PALADIN_CLASS_ID - 1)
EQUIPMENT_SKILL_LINE_NAMES = {
    "Axes", "Swords", "Maces", "Daggers", "Fist Weapons",
    "Two-Handed Axes", "Two-Handed Swords", "Two-Handed Maces", "Polearms",
    "Staves", "Bows", "Guns", "Crossbows", "Thrown",
    "Cloth", "Leather", "Mail", "Plate Mail", "Shield",
}


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
        text = str(raw).strip()
        try:
            return int(text, 0)
        except ValueError:
            return int(text, 10)
    except (TypeError, ValueError) as error:
        raise ValueError(f"{table} row {line_number} has invalid {names[0]} value {raw!r}") from error


def race_mask(row, table, line_number):
    if "RaceMask" in row and str(row["RaceMask"] or "").strip():
        # Wago can render signed DB2 bitmasks as negative decimal integers.
        # Normalize to the unsigned 64-bit representation before bit iteration.
        return integer(row, ("RaceMask",), table, line_number) & ((1 << 64) - 1)
    low_names = ("RaceMasks_0", "RaceMask1", "RaceMask_0")
    high_names = ("RaceMasks_1", "RaceMask2", "RaceMask_1")
    has_mask_fields = any(name in row for name in (*low_names, *high_names))
    if not has_mask_fields:
        raise ValueError(f"{table} CSV has no RaceMask or split race-mask columns")
    # The DB2 array is two signed 32-bit words. Mask each word independently;
    # shifting a negative Python integer would sign-extend and corrupt race IDs.
    low = integer(row, low_names, table, line_number, required=False, default=0) & 0xFFFFFFFF
    high = integer(row, high_names, table, line_number, required=False, default=0) & 0xFFFFFFFF
    return low | (high << 32)


def race_mask_is_unrestricted(mask):
    # SkillRaceClassInfo uses both an empty mask and an all-bits mask for no race restriction.
    return mask in (0, -1, 0xFFFFFFFF, 0xFFFFFFFFFFFFFFFF)


def import_paladin_equipment(skill_line_csv, skill_race_class_info_csv, data_dir=DATA, *,
                             class_name="paladin", class_id=PALADIN_CLASS_ID,
                             equipment_skill_names=None):
    """Project source-backed, class-wide item proficiencies for one selected class."""
    if not class_name.isalpha() or not 1 <= class_id <= 12:
        raise ValueError("invalid class identity for equipment projection")
    class_mask = 1 << (class_id - 1)
    equipment_skill_names = (EQUIPMENT_SKILL_LINE_NAMES if equipment_skill_names is None
                             else set(equipment_skill_names))
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
            required=False,
            default="",
        )).strip()
        category_id = integer(row, ("CategoryID",), "SkillLine", line_number, required=False, default=0)
        skill_lines[skill_id] = {"name": name, "category_id": category_id}

    by_skill = {}
    class_skill_rows = 0
    availability_counts = {}
    for line_number, row in enumerate(skill_info_rows, 2):
        skill_id = integer(row, ("SkillID",), "SkillRaceClassInfo", line_number)
        row_class_mask = integer(row, ("ClassMask", "ClassMask1", "ClassMask_0"), "SkillRaceClassInfo", line_number)
        mask = race_mask(row, "SkillRaceClassInfo", line_number)
        raw_min_level = integer(row, ("MinLevel",), "SkillRaceClassInfo", line_number)
        availability = integer(row, ("Availability",), "SkillRaceClassInfo", line_number)
        availability_counts[str(availability)] = availability_counts.get(str(availability), 0) + 1

        if row_class_mask != 0 and not (row_class_mask & class_mask):
            continue
        if row_class_mask & class_mask:
            class_skill_rows += 1
        by_skill.setdefault(skill_id, []).append({
            "class_mask": row_class_mask,
            "race_mask": mask,
            "race_unrestricted": race_mask_is_unrestricted(mask),
            "source_min_level": raw_min_level,
            "min_level": max(1, raw_min_level),
            "availability": availability,
        })

    if not class_skill_rows:
        raise ValueError(f"SkillRaceClassInfo contains no rows for {class_name.capitalize()} class mask {class_mask}")
    if not skill_lines:
        raise ValueError("SkillLine contains no rows")

    skills = []
    for skill_id, rows in by_skill.items():
        if skill_id not in skill_lines:
            raise ValueError(f"SkillRaceClassInfo references missing SkillLine ID {skill_id}")
        name = skill_lines[skill_id]["name"]
        if not name:
            raise ValueError(f"SkillLine ID {skill_id} has no display name")
        class_rows = [
            row for row in rows
            if row["class_mask"] == 0 or row["class_mask"] & class_mask
        ]
        unrestricted_rows = [row for row in class_rows if row["race_unrestricted"]]
        min_level = min((row["min_level"] for row in unrestricted_rows), default=None)
        skills.append({
            "skill_id": skill_id,
            "name": name,
            "category_id": skill_lines[skill_id]["category_id"],
            "min_level": min_level,
            "race_unrestricted": bool(unrestricted_rows),
            "availability_values": sorted({row["availability"] for row in class_rows}),
            "source_rows": len(class_rows),
            "unrestricted_race_rows": len(unrestricted_rows),
            "race_restricted_rows": len(class_rows) - len(unrestricted_rows),
        })
    skills.sort(key=lambda item: (item["name"].casefold(), item["skill_id"]))

    projected_skill_lines = {
        skill_id: info
        for skill_id, info in skill_lines.items()
        if info["name"] in equipment_skill_names
    }
    skills = [skill for skill in skills if skill["name"] in equipment_skill_names]

    output = {
        "schema_version": 2,
        "client_build": client_build,
        "class": class_name,
        "class_id": class_id,
        "class_mask": class_mask,
        "race_policy": "class-wide only; SkillRaceClassInfo race mask must be unrestricted",
        # The source CSV is hashed and row-counted in the manifest; the runtime projection
        # retains only the complete equipment proficiency index used by ItemSparse filtering.
        "skill_line_ids": sorted(projected_skill_lines),
        "skill_line_names": [
            {"skill_id": skill_id, "name": projected_skill_lines[skill_id]["name"]}
            for skill_id in sorted(projected_skill_lines)
        ],
        "skills": skills,
    }
    output_path = data_dir / f"{class_name}-equipment-skills.json"
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
        "class_id": class_id,
        "class_mask": class_mask,
        f"{class_name}_class_skill_row_count": class_skill_rows,
        "skill_race_class_availability_counts": dict(sorted(availability_counts.items())),
        "source_skill_line_id_count": len(skill_lines),
        "projected_skill_line_id_count": len(projected_skill_lines),
        "projected_skill_count": len(skills),
        "class_wide_skill_count": sum(skill["min_level"] is not None for skill in skills),
        "race_specific_or_unavailable_skill_count": sum(skill["min_level"] is None for skill in skills),
    }
    source_manifest_path = data_dir / f"{class_name}-equipment-skills.manifest.json"
    write_json(source_manifest_path, source_manifest)

    main_manifest.setdefault("files", {})
    main_manifest["files"][f"{class_name}-equipment-skills.json"] = digest(output_path)
    main_manifest["files"][f"{class_name}-equipment-skills.manifest.json"] = digest(source_manifest_path)
    main_manifest["files"] = dict(sorted(main_manifest["files"].items()))
    write_json(main_manifest_path, main_manifest)

    return {
        "client_build": client_build,
        f"{class_name}_class_skill_row_count": class_skill_rows,
        "source_skill_line_id_count": len(skill_lines),
        "projected_skill_line_id_count": len(projected_skill_lines),
        "projected_skill_count": len(skills),
        "class_wide_skill_count": source_manifest["class_wide_skill_count"],
        "race_specific_or_unavailable_skill_count": source_manifest["race_specific_or_unavailable_skill_count"],
        "availability_counts": source_manifest["skill_race_class_availability_counts"],
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
            import_paladin_equipment(args.skill_line_csv, args.skill_race_class_info_csv, args.data_dir),
            sort_keys=True,
        ))
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as error:
        print(f"Paladin equipment import failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
