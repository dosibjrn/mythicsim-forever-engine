#!/usr/bin/env python3
"""Export source-observed Forever item facts for the Rust get_options API.

Reads the preserved Wowhead decisions database *without changing it* and writes a
small, deterministic item-id keyed JSONL projection. Native Rust SimItem remains
the source for simulation mechanics and stats; this source adds item required
level, evidence origin, provenance, and observed class eligibility only.

Example:
    python3 tools/export_legacy_wowhead_items.py \
        /tmp/items_2026-09-29.sqlite data/legacy-wowhead-items.jsonl
"""
from __future__ import annotations

import argparse
import hashlib
import json
import sqlite3
from pathlib import Path

# The original 2026-09-29 catalogue in dosibjrn/mage-level-sim. Matches the
# git-lfs SHA-256 pointer for this source in dosibjrn/forever-sim.
SOURCE_SHA256 = "d7d5a8519b227a761312bf4d1da56955b58184239a5a2ed7a06f84eb134dc854"
EXPECTED_ITEMS = 7597
EXPECTED_OBSERVED = 7054
EXPECTED_INFERRED = 543
EXPECTED_ELIGIBILITY_LINKS = 14931


def export(source: Path, output: Path, *, verify_hash: bool = True) -> dict[str, int]:
    if verify_hash:
        digest = hashlib.sha256(source.read_bytes()).hexdigest()
        if digest != SOURCE_SHA256:
            raise ValueError(f"legacy item database SHA-256 {digest} != pinned {SOURCE_SHA256}")
    uri = source.absolute().as_uri() + "?mode=ro"
    conn = sqlite3.connect(uri, uri=True)
    conn.row_factory = sqlite3.Row
    try:
        conn.execute("PRAGMA query_only=ON")
        eligible: dict[int, list[dict[str, object]]] = {}
        for entry in conn.execute(
            "SELECT item_id, class_id, provenance "
            "FROM item_class_eligibility ORDER BY item_id, class_id"
        ):
            eligible.setdefault(entry["item_id"], []).append(
                {"class_id": entry["class_id"], "provenance": entry["provenance"]}
            )
        rows = list(conn.execute(
            "SELECT item_id, required_level, required_level_origin, item_level, "
            "source_url, last_verified_at, fetch_status "
            "FROM items ORDER BY item_id"
        ))
        counts = {
            "items": len(rows),
            "observed": sum(r["required_level_origin"] == "observed" for r in rows),
            "inferred": sum(
                r["required_level_origin"] in ("inferred_median", "inferred_interpolated")
                for r in rows
            ),
            "class_links": sum(map(len, eligible.values())),
        }
        expected = {
            "items": EXPECTED_ITEMS,
            "observed": EXPECTED_OBSERVED,
            "inferred": EXPECTED_INFERRED,
            "class_links": EXPECTED_ELIGIBILITY_LINKS,
        }
        if counts != expected:
            raise ValueError(f"unexpected source catalogue totals {counts}, expected {expected}")
        if any(r["required_level"] is None for r in rows):
            raise ValueError("pinned catalogue must explicitly resolve every required level")
        output.parent.mkdir(parents=True, exist_ok=True)
        with output.open("w", encoding="utf-8", newline="\n") as f:
            for row in rows:
                fact = {
                    "item_id": row["item_id"],
                    "required_level": row["required_level"],
                    "required_level_origin": row["required_level_origin"],
                    "item_level": row["item_level"],
                    "source_url": row["source_url"] or "",
                    "last_verified_at": row["last_verified_at"],
                    "fetch_status": row["fetch_status"],
                    "class_eligibility": eligible.get(row["item_id"], []),
                }
                f.write(json.dumps(fact, ensure_ascii=False, separators=(",", ":")) + "\n")
        return counts
    finally:
        conn.close()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="pinned legacy Wowhead SQLite source")
    parser.add_argument("output", type=Path, help="Rust-embedded JSONL output")
    args = parser.parse_args()
    counts = export(args.source, args.output)
    print(json.dumps(counts, sort_keys=True))


if __name__ == "__main__":
    main()
