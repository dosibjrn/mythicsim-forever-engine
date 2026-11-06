#!/usr/bin/env python3
"""Import Warlock equipment proficiencies from the pinned client DB2 tables.

Share the existing class-mask and unrestricted-race source projection. Wands
are included from native SkillRaceClassInfo, never a Priest/Mage assumption.
"""
import argparse
import json
from pathlib import Path
import sys
from import_paladin_equipment import DATA, EQUIPMENT_SKILL_LINE_NAMES, import_paladin_equipment

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("skill_line_csv", type=Path)
    parser.add_argument("skill_race_class_info_csv", type=Path)
    parser.add_argument("--data-dir", type=Path, default=DATA)
    args = parser.parse_args()
    try:
        result = import_paladin_equipment(
            args.skill_line_csv, args.skill_race_class_info_csv, args.data_dir,
            class_name="warlock", class_id=9,
            equipment_skill_names=EQUIPMENT_SKILL_LINE_NAMES | {"Wands"},
        )
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as error:
        print(f"Warlock equipment import failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0

if __name__ == "__main__":
    sys.exit(main())
