"""Verify that a Rogue (class mask 8) uses only pinned client DB2 source rows."""

import csv
import json
import tempfile
import unittest
from pathlib import Path

from import_paladin_equipment import import_paladin_equipment


class RogueSourceProjectionTests(unittest.TestCase):
    def test_rogue_mask_and_race_constraints_never_inherit_paladin_skill_rows(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            data = root / "data"
            data.mkdir()
            (data / "manifest.json").write_text(
                json.dumps({"client_build": "1.60.1.70425", "files": {}}),
                encoding="utf-8",
            )
            skill_lines = root / "SkillLine.csv"
            skill_info = root / "SkillRaceClassInfo.csv"
            with skill_lines.open("w", newline="", encoding="utf-8") as target:
                writer = csv.DictWriter(target, fieldnames=["ID", "DisplayName_lang", "CategoryID"])
                writer.writeheader()
                writer.writerows([
                    {"ID": 173, "DisplayName_lang": "Daggers", "CategoryID": 6},
                    {"ID": 43, "DisplayName_lang": "Swords", "CategoryID": 6},
                    {"ID": 414, "DisplayName_lang": "Leather", "CategoryID": 8},
                    {"ID": 433, "DisplayName_lang": "Shield", "CategoryID": 8},
                ])
            with skill_info.open("w", newline="", encoding="utf-8") as target:
                writer = csv.DictWriter(target, fieldnames=[
                    "SkillID", "ClassMask", "MinLevel", "Availability",
                    "RaceMasks_0", "RaceMasks_1",
                ])
                writer.writeheader()
                writer.writerows([
                    {"SkillID": 173, "ClassMask": 8, "MinLevel": 1,
                     "Availability": 1, "RaceMasks_0": -1, "RaceMasks_1": -1},
                    {"SkillID": 43, "ClassMask": 8, "MinLevel": 10,
                     "Availability": 0, "RaceMasks_0": 1, "RaceMasks_1": 0},
                    {"SkillID": 414, "ClassMask": 8, "MinLevel": 1,
                     "Availability": 1, "RaceMasks_0": 0, "RaceMasks_1": 0},
                    {"SkillID": 433, "ClassMask": 2, "MinLevel": 1,
                     "Availability": 1, "RaceMasks_0": -1, "RaceMasks_1": -1},
                ])
            summary = import_paladin_equipment(
                skill_lines, skill_info, data, class_name="rogue", class_id=4
            )
            payload = json.loads((data / "rogue-equipment-skills.json").read_text())
            manifest = json.loads((data / "manifest.json").read_text())
            skills = {row["name"]: row for row in payload["skills"]}
            self.assertEqual(payload["class"], "rogue")
            self.assertEqual(payload["class_id"], 4)
            self.assertEqual(payload["class_mask"], 8)
            self.assertEqual(skills["Daggers"]["min_level"], 1)
            self.assertEqual(skills["Leather"]["min_level"], 1)
            self.assertIsNone(skills["Swords"]["min_level"])  # race-specific, not class-wide
            self.assertNotIn("Shield", skills)  # belongs to Paladin class mask 2
            self.assertEqual(summary["rogue_class_skill_row_count"], 3)
            self.assertIn("rogue-equipment-skills.json", manifest["files"])
            self.assertNotIn("paladin-equipment-skills.json", manifest["files"])


if __name__ == "__main__":
    unittest.main()
