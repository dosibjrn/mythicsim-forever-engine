#!/usr/bin/env python3
"""Import Rogue native equipment proficiency from the exact pinned client DB2 CSVs.

Shares the existing class-mask/race-wide source projection (not a gameplay
legality map). No default Rogue proficiencies are invented when rows are absent.
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
            args.skill_line_csv, args.skill_race_class_info_csv,
            args.data_dir, class_name="rogue", class_id=4,
        )
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as error:
        print(f"Rogue equipment import failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
