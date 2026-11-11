#!/usr/bin/env python3
"""Project Warlock native equipment skills from pinned SkillLine/SkillRaceClassInfo.

This is source acquisition, not a replacement WoW rules engine. The existing
shared importer applies the client class bitmask and unrestricted-race policy.
In particular, Troll-only skills must not become class-wide Warlock skills.

Usage: python tools/import_warlock_equipment.py SkillLine.csv SkillRaceClassInfo.csv
"""
import argparse
import json
from pathlib import Path
import sys

from import_paladin_equipment import DATA, import_paladin_equipment


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("skill_line_csv", type=Path)
    parser.add_argument("skill_race_class_info_csv", type=Path)
    parser.add_argument("--data-dir", type=Path, default=DATA)
    args = parser.parse_args()
    try:
        result = import_paladin_equipment(
            args.skill_line_csv,
            args.skill_race_class_info_csv,
            args.data_dir,
            class_name="warlock",
            class_id=9,  # pinned client ClassWarlock bit 8, mask 256
        )
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as error:
        print(f"Warlock equipment import failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
