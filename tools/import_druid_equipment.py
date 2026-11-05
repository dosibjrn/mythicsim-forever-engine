#!/usr/bin/env python3
"""Project Druid equipment skill rows from the exact pinned client's DB2 exports.

The existing native class-mask/race-unrestricted projector is shared with
Paladin and Rogue. No Druid equipment eligibility is inferred from item names
or generic WoW class assumptions. The output must be reviewed before it is
used by the native item-options API.
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
            class_name="druid",
            class_id=11,
        )
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as error:
        print(f"Druid equipment import failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
