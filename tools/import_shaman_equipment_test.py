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
            skill_info_headers or ["ID", "SkillID", "RaceMask", "ClassMask", "MinLevel"],
            skill_info,
        )
        summary = import_shaman_equipment(skill_line_csv, skill_info_csv, data)
        payload = json.loads((data / "shaman-equipment-skills.json").read_text(encoding="utf-8"))
        manifest = json.loads((data / "shaman-equipment-skills.manifest.json").read_text(encoding="utf-8"))
        return summary, payload, manifest, json.loads((data / "manifest.json").read_text(encoding="utf-8"))

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
                rows.append({
                    "ID": row_id, "SkillID": skill_id, "RaceMask": race_mask,
                    "ClassMask": 64, "MinLevel": min_level,
                })
                row_id += 1
        for skill_id, min_level in ((45, 1), (46, 40), (55, 1), (182, 1)):
            rows.append({
                "ID": row_id, "SkillID": skill_id, "RaceMask": 162,
                "ClassMask": 64, "MinLevel": min_level,
            })
            row_id += 1
        rows.append({
            "ID": row_id, "SkillID": 415, "RaceMask": 0,
            "ClassMask": 0, "MinLevel": 0,
        })
        row_id += 1
        # A race-restricted skill is not widened to every Shaman.
        rows.extend([
            {"ID": row_id, "SkillID": 43, "RaceMask": 2, "ClassMask": 64, "MinLevel": 1},
            {"ID": row_id + 1, "SkillID": 43, "RaceMask": 32, "ClassMask": 64, "MinLevel": 1},
            # A non-Shaman class row must not affect the projection.
            {"ID": row_id + 2, "SkillID": 43, "RaceMask": 128, "ClassMask": 256, "MinLevel": 1},
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
        self.assertEqual(manifest["source"]["client_build"], "1.60.1.70425")
        self.assertIn("shaman-equipment-skills.json", main_manifest["files"])
        self.assertIn("shaman-equipment-skills.manifest.json", main_manifest["files"])

    def test_supports_split_64_bit_race_masks(self):
        skill_lines = [{"ID": 136, "DisplayName_lang": "Staves", "CategoryID": 7}]
        rows = [{
            "ID": 1, "SkillID": 136, "RaceMask1": 2, "RaceMask2": 1,
            "ClassMask": 64, "MinLevel": 1,
        }]
        summary, payload, _, _ = self.run_import(
            skill_lines,
            rows,
            skill_info_headers=["ID", "SkillID", "RaceMask1", "RaceMask2", "ClassMask", "MinLevel"],
        )
        self.assertEqual(payload["race_mask"], (1 << 32) | 2)
        self.assertEqual(payload["race_ids"], [2, 33])
        self.assertEqual(payload["skills"][0]["min_level"], 1)

    def test_rejects_missing_skill_line_reference(self):
        skill_lines = [{"ID": 44, "DisplayName_lang": "Axes", "CategoryID": 7}]
        rows = [{"ID": 1, "SkillID": 999, "RaceMask": 0, "ClassMask": 64, "MinLevel": 1}]
        with self.assertRaisesRegex(ValueError, "missing SkillLine ID 999"):
            self.run_import(skill_lines, rows)

    def test_rejects_source_without_shaman_rows(self):
        skill_lines = [{"ID": 44, "DisplayName_lang": "Axes", "CategoryID": 7}]
        rows = [{"ID": 1, "SkillID": 44, "RaceMask": 0, "ClassMask": 256, "MinLevel": 1}]
        with self.assertRaisesRegex(ValueError, "no rows for Shaman"):
            self.run_import(skill_lines, rows)


if __name__ == "__main__":
    unittest.main()
