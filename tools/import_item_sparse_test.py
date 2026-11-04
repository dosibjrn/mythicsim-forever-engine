import csv
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from import_item_sparse import import_item_sparse


class ImportItemSparseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.data = Path(self.temp.name)
        (self.data / "items.jsonl").write_text(
            '{"id":100,"name":"A"}\n{"id":200,"name":"B"}\n',
            encoding="utf-8",
        )
        (self.data / "manifest.json").write_text(
            json.dumps({
                "schema_version": 1,
                "engine_revision": "pinned-revision",
                "client_build": "1.60.1.70425",
                "tool": "tools/rust-data",
                "tool_sha256": "tool-digest",
                "files": {},
            }),
            encoding="utf-8",
        )
        self.csv = self.data / "ItemSparse.csv"
        with self.csv.open("w", encoding="utf-8", newline="") as handle:
            writer = csv.DictWriter(handle, fieldnames=["ID", "RequiredLevel", "ItemLevel"])
            writer.writeheader()
            writer.writerows([
                {"ID": 200, "RequiredLevel": 20, "ItemLevel": 25},
                {"ID": 300, "RequiredLevel": 30, "ItemLevel": 35},
                {"ID": 100, "RequiredLevel": 0, "ItemLevel": 1},
            ])

    def tearDown(self):
        self.temp.cleanup()

    def test_imports_only_native_ids_and_preserves_exact_required_level(self):
        result = import_item_sparse(self.csv, self.data)
        rows = [
            json.loads(line)
            for line in (self.data / "item-required-levels.jsonl").read_text(encoding="utf-8").splitlines()
        ]
        self.assertEqual(rows, [
            {"id": 100, "required_level": 0},
            {"id": 200, "required_level": 20},
        ])
        self.assertEqual(result["client_build"], "1.60.1.70425")
        self.assertEqual(result["matched_item_count"], 2)
        self.assertEqual(result["missing_native_item_count"], 0)
        self.assertEqual(result["unknown_required_level_count"], 0)
        manifest = json.loads((self.data / "manifest.json").read_text(encoding="utf-8"))
        self.assertEqual(
            manifest["files"]["item-required-levels.jsonl"],
            hashlib.sha256((self.data / "item-required-levels.jsonl").read_bytes()).hexdigest(),
        )
        source = json.loads((self.data / "item-required-levels.manifest.json").read_text(encoding="utf-8"))
        self.assertEqual(source["source"]["field"], "RequiredLevel")
        self.assertEqual(source["source"]["client_build"], "1.60.1.70425")

    def test_preserves_a_native_item_with_unknown_required_level_as_null(self):
        self.csv.write_text("ID,RequiredLevel\n100,\n300,30\n", encoding="utf-8")
        result = import_item_sparse(self.csv, self.data)
        rows = [
            json.loads(line)
            for line in (self.data / "item-required-levels.jsonl").read_text(encoding="utf-8").splitlines()
        ]
        self.assertEqual(rows, [{"id": 100, "required_level": None}])
        self.assertEqual(result["matched_item_count"], 1)
        self.assertEqual(result["missing_native_item_count"], 1)
        self.assertEqual(result["unknown_required_level_count"], 1)

    def test_excludes_native_ids_absent_from_the_pinned_client(self):
        self.csv.write_text("ID,RequiredLevel\n100,10\n300,30\n", encoding="utf-8")
        result = import_item_sparse(self.csv, self.data)
        rows = [
            json.loads(line)
            for line in (self.data / "item-required-levels.jsonl").read_text(encoding="utf-8").splitlines()
        ]
        self.assertEqual(rows, [{"id": 100, "required_level": 10}])
        self.assertEqual(result["missing_native_item_count"], 1)
        self.assertEqual(result["unknown_required_level_count"], 0)

    def test_rejects_duplicate_native_item_rows(self):
        self.csv.write_text("ID,RequiredLevel\n100,1\n100,2\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "duplicate ItemSparse ID"):
            import_item_sparse(self.csv, self.data)

    def test_rejects_a_table_without_the_required_level_field(self):
        self.csv.write_text("ID,ItemLevel\n100,1\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "ID and RequiredLevel"):
            import_item_sparse(self.csv, self.data)


if __name__ == "__main__":
    unittest.main()
