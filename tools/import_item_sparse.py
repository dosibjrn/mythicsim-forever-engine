#!/usr/bin/env python3
"""Import RequiredLevel from the pinned client's Wago ItemSparse table.

This reads only native MythicSim item IDs and the pinned client DB2 export.
It does not read Forever's SQLite catalogue or infer a level from item level.
"""

import argparse
import csv
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / "data"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n", encoding="utf-8")


def import_item_sparse(csv_path, data_dir=DATA):
    data_dir = Path(data_dir)
    main_manifest_path = data_dir / "manifest.json"
    main_manifest = json.loads(main_manifest_path.read_text(encoding="utf-8"))
    client_build = main_manifest.get("client_build")
    if not client_build:
        raise ValueError("data/manifest.json has no pinned client_build")

    native_items_path = data_dir / "items.jsonl"
    native_ids = set()
    for line_number, line in enumerate(native_items_path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip():
            continue
        row = json.loads(line)
        item_id = int(row["id"])
        if item_id in native_ids:
            raise ValueError(f"duplicate native item id {item_id} in {native_items_path}:{line_number}")
        native_ids.add(item_id)
    if not native_ids:
        raise ValueError("native data/items.jsonl contains no item IDs")

    csv_path = Path(csv_path)
    required_levels = {}
    csv_row_count = 0
    with csv_path.open("r", encoding="utf-8-sig", newline="") as source:
        reader = csv.DictReader(source)
        if not reader.fieldnames or not {"ID", "RequiredLevel"} <= set(reader.fieldnames):
            raise ValueError("ItemSparse CSV must contain ID and RequiredLevel columns")
        for line_number, row in enumerate(reader, 2):
            csv_row_count += 1
            raw_id = row.get("ID")
            if raw_id is None or not raw_id.strip():
                raise ValueError(f"ItemSparse row {line_number} has no ID")
            item_id = int(raw_id)
            if item_id not in native_ids:
                continue
            if item_id in required_levels:
                raise ValueError(f"duplicate ItemSparse ID {item_id}")
            raw_level = row.get("RequiredLevel")
            if raw_level is None or not raw_level.strip():
                raise ValueError(f"ItemSparse ID {item_id} has no RequiredLevel value")
            required_level = int(raw_level)
            if required_level < 0:
                raise ValueError(f"ItemSparse ID {item_id} has negative RequiredLevel {required_level}")
            required_levels[item_id] = required_level

    if not required_levels:
        raise ValueError("ItemSparse produced no rows matching native data/items.jsonl")

    jsonl = "".join(
        json.dumps({"id": item_id, "required_level": required_levels[item_id]},
                   separators=(",", ":")) + "\n"
        for item_id in sorted(required_levels)
    )
    output_path = data_dir / "item-required-levels.jsonl"
    output_path.write_text(jsonl, encoding="utf-8")

    source_manifest = {
        "schema_version": 1,
        "source": {
            "provider": "wago.tools",
            "table": "ItemSparse",
            "field": "RequiredLevel",
            "client_build": client_build,
            "url": f"https://wago.tools/db2/ItemSparse/csv?build={client_build}",
            "csv_sha256": digest(csv_path),
        },
        "native_item_count": len(native_ids),
        "matched_item_count": len(required_levels),
        "native_items_without_item_sparse_row": len(native_ids) - len(required_levels),
        "csv_row_count": csv_row_count,
    }
    source_manifest_path = data_dir / "item-required-levels.manifest.json"
    write_json(source_manifest_path, source_manifest)

    main_manifest.setdefault("files", {})
    main_manifest["files"]["item-required-levels.jsonl"] = digest(output_path)
    main_manifest["files"]["item-required-levels.manifest.json"] = digest(source_manifest_path)
    main_manifest["files"] = dict(sorted(main_manifest["files"].items()))
    write_json(main_manifest_path, main_manifest)

    return {
        "client_build": client_build,
        "native_item_count": len(native_ids),
        "matched_item_count": len(required_levels),
        "missing_native_item_count": len(native_ids) - len(required_levels),
        "csv_row_count": csv_row_count,
        "output_sha256": digest(output_path),
        "csv_sha256": digest(csv_path),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("csv", type=Path, help="Wago ItemSparse CSV for the pinned client build")
    parser.add_argument("--data-dir", type=Path, default=DATA)
    args = parser.parse_args()
    try:
        print(json.dumps(import_item_sparse(args.csv, args.data_dir), sort_keys=True))
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as error:
        print(f"ItemSparse import failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
