import csv
import json
import tempfile
import unittest
from pathlib import Path

from import_shaman_equipment import import_shaman_equipment


class ImportShamanEquipmentTests(unittest.TestCase):
    def write_csv(self, path, headers, rows):
        with path.open("w", encoding="utf-8", newline="") as target:
            writer = csv.DictWriter(target, fieldnames=headers)
            writer.writeheader()
            writer.writerows(rows)

    def run_import(self, skill_lines, skill_info, skill_line_headers=None, skill_info_headers=None):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        data = root / "data"
        data.mkdir()
        (data / "manifest.json").write_text(
            json.dumps({"client_build": "1.60.1.70425", "files": {}}),
            encoding="utf-8",
        )
        skill_line_csv = root / "SkillLine.csv"
        skill_info_csv = root / "SkillRaceClassInfo.csv"
        self.write_csv(
            skill_line_csv,
            skill_line_headers or ["ID", "DisplayName_lang", "CategoryID"],
            skill_lines,
        )
        self.write_csv(
            skill_info_csv,
            skill_info_headers or [
                "ID", "SkillID", "ClassMask", "Flags", "Availability", "MinLevel",
                "SkillTierID", "RaceMasks_0", "RaceMasks_1",
            ],
            skill_info,
        )
        summary = import_shaman_equipment(skill_line_csv, skill_info_csv, data)
        payload = json.loads((data / "shaman-equipment-skills.json").read_text(encoding="utf-8"))
        manifest = json.loads((data / "shaman-equipment-skills.manifest.json").read_text(encoding="utf-8"))
        return summary, payload, manifest, json.loads((data / "manifest.json").read_text(encoding="utf-8"))

    def row(self, row_id, skill_id, race_mask, class_mask=64, min_level=1, availability=1):
        return {
            "ID": row_id,
            "SkillID": skill_id,
            "ClassMask": class_mask,
            "Flags": 0,
            "Availability": availability,
            "MinLevel": min_level,
            "SkillTierID": 0,
            "RaceMasks_0": race_mask,
            "RaceMasks_1": 0,
        }

    def test_class_wide_min_level_uses_all_shaman_races(self):
        skill_lines = [
            {"ID": 44, "DisplayName_lang": "Axes", "CategoryID": 7},
            {"ID": 172, "DisplayName_lang": "Two-Handed Axes", "CategoryID": 7},
            {"ID": 45, "DisplayName_lang": "Leather", "CategoryID": 6},
            {"ID": 46, "DisplayName_lang": "Mail", "CategoryID": 6},
            {"ID": 415, "DisplayName_lang": "Cloth", "CategoryID": 6},
            {"ID": 55, "DisplayName_lang": "Shields", "CategoryID": 6},
            {"ID": 182, "DisplayName_lang": "Totems", "CategoryID": 7},
            {"ID": 43, "DisplayName_lang": "Swords", "CategoryID": 7},
        ]
        rows = []
        row_id = 1
        for skill_id, min_level in ((44, 1), (172, 20)):
            for race_mask in (2, 32, 128):
                rows.append(self.row(row_id, skill_id, race_mask, min_level=min_level))
                row_id += 1
        for skill_id, min_level in ((45, 1), (46, 40), (55, 1), (182, 1)):
            rows.append(self.row(row_id, skill_id, 162, min_level=min_level))
            row_id += 1
        rows.append(self.row(row_id, 415, 0, class_mask=0, min_level=0))
        row_id += 1
        # A race-restricted skill is not widened to every Shaman.
        rows.extend([
            self.row(row_id, 43, 2),
            self.row(row_id + 1, 43, 32),
            # A non-Shaman class row must not affect the projection.
            self.row(row_id + 2, 43, 128, class_mask=256),
            # Unavailable rows must not be treated as learned proficiencies.
            self.row(row_id + 3, 43, 162, availability=0),
        ])

        summary, payload, manifest, main_manifest = self.run_import(skill_lines, rows)
        skills = {skill["name"]: skill for skill in payload["skills"]}

        self.assertEqual(payload["client_build"], "1.60.1.70425")
        self.assertEqual(payload["class"], "shaman")
        self.assertEqual(payload["class_mask"], 64)
        self.assertEqual(payload["race_mask"], 162)
        self.assertEqual(payload["race_ids"], [2, 6, 8])
        self.assertEqual(skills["Axes"]["min_level"], 1)
        self.assertEqual(skills["Two-Handed Axes"]["min_level"], 20)
        self.assertEqual(skills["Leather"]["min_level"], 1)
        self.assertEqual(skills["Mail"]["min_level"], 40)
        self.assertEqual(skills["Cloth"]["min_level"], 1)
        self.assertEqual(skills["Shields"]["min_level"], 1)
        self.assertEqual(skills["Totems"]["min_level"], 1)
        self.assertIsNone(skills["Swords"]["min_level"])
        self.assertEqual(skills["Swords"]["per_race_min_level"], {"2": 1, "6": 1, "8": None})
        self.assertEqual(summary["class_wide_skill_count"], 7)
        self.assertEqual(summary["unavailable_skill_race_class_row_count"], 1)
        self.assertEqual(manifest["source"]["client_build"], "1.60.1.70425")
        self.assertIn("shaman-equipment-skills.json", main_manifest["files"])
        self.assertIn("shaman-equipment-skills.manifest.json", main_manifest["files"])

    def test_supports_split_64_bit_race_masks(self):
        skill_lines = [{"ID": 136, "DisplayName_lang": "Staves", "CategoryID": 7}]
        rows = [self.row(1, 136, 2)]
        rows[0]["RaceMasks_1"] = 1
        summary, payload, _, _ = self.run_import(skill_lines, rows)
        self.assertEqual(payload["race_mask"], (1 << 32) | 2)
        self.assertEqual(payload["race_ids"], [2, 33])
        self.assertEqual(payload["skills"][0]["min_level"], 1)

    def test_rejects_missing_skill_line_reference(self):
        skill_lines = [{"ID": 44, "DisplayName_lang": "Axes", "CategoryID": 7}]
        rows = [self.row(1, 999, 0)]
        with self.assertRaisesRegex(ValueError, "missing SkillLine ID 999"):
            self.run_import(skill_lines, rows)

    def test_rejects_source_without_available_shaman_rows(self):
        skill_lines = [{"ID": 44, "DisplayName_lang": "Axes", "CategoryID": 7}]
        rows = [self.row(1, 44, 0, class_mask=256)]
        with self.assertRaisesRegex(ValueError, "no available rows for Shaman"):
            self.run_import(skill_lines, rows)

    def test_rejects_unavailable_only_shaman_rows(self):
        skill_lines = [{"ID": 44, "DisplayName_lang": "Axes", "CategoryID": 7}]
        rows = [self.row(1, 44, 0, availability=0)]
        with self.assertRaisesRegex(ValueError, "no available rows for Shaman"):
            self.run_import(skill_lines, rows)


if __name__ == "__main__":
    unittest.main()
