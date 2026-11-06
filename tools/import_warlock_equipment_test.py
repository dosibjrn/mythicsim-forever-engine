"""Source-verified Warlock (class bit 8, mask 256) proficiency projection."""
import csv
import json
import tempfile
import unittest
from pathlib import Path

from import_paladin_equipment import import_paladin_equipment


class WarlockSourceProjectionTests(unittest.TestCase):
    def test_class_mask_and_troll_only_skill_never_become_class_wide(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            data = root / "data"
            data.mkdir()
            (data / "manifest.json").write_text(
                json.dumps({"client_build": "1.60.1.70425", "files": {}}),
                encoding="utf-8",
            )
            lines = root / "SkillLine.csv"
            info = root / "SkillRaceClassInfo.csv"
            with lines.open("w", newline="", encoding="utf-8") as dst:
                writer = csv.DictWriter(
                    dst, fieldnames=["ID", "DisplayName_lang", "CategoryID"])
                writer.writeheader()
                writer.writerows([
                    {"ID": 415, "DisplayName_lang": "Cloth", "CategoryID": 8},
                    {"ID": 136, "DisplayName_lang": "Staves", "CategoryID": 6},
                    {"ID": 173, "DisplayName_lang": "Daggers", "CategoryID": 6},
                    {"ID": 293, "DisplayName_lang": "Plate Mail", "CategoryID": 8},
                ])
            with info.open("w", newline="", encoding="utf-8") as dst:
                writer = csv.DictWriter(dst, fieldnames=[
                    "SkillID", "ClassMask", "MinLevel", "Availability",
                    "RaceMasks_0", "RaceMasks_1",
                ])
                writer.writeheader()
                writer.writerows([
                    {"SkillID": 415, "ClassMask": 256, "MinLevel": 1,
                     "Availability": 1, "RaceMasks_0": -1, "RaceMasks_1": -1},
                    {"SkillID": 136, "ClassMask": 256, "MinLevel": 1,
                     "Availability": 1, "RaceMasks_0": 0, "RaceMasks_1": 0},
                    # A race-only skill is not legal class-wide (Troll is not all races).
                    {"SkillID": 173, "ClassMask": 256, "MinLevel": 10,
                     "Availability": 0, "RaceMasks_0": 1 << 7, "RaceMasks_1": 0},
                    {"SkillID": 293, "ClassMask": 2, "MinLevel": 1,
                     "Availability": 1, "RaceMasks_0": -1, "RaceMasks_1": -1},
                ])
            summary = import_paladin_equipment(
                lines, info, data, class_name="warlock", class_id=9)
            payload = json.loads(
                (data / "warlock-equipment-skills.json").read_text())
            manifest = json.loads(
                (data / "warlock-equipment-skills.manifest.json").read_text())
            index = json.loads((data / "manifest.json").read_text())
            skills = {row["name"]: row for row in payload["skills"]}
            self.assertEqual(payload["class"], "warlock")
            self.assertEqual(payload["class_id"], 9)
            self.assertEqual(payload["class_mask"], 256)
            self.assertEqual(skills["Cloth"]["min_level"], 1)
            self.assertEqual(skills["Staves"]["min_level"], 1)
            self.assertIsNone(skills["Daggers"]["min_level"])
            self.assertNotIn("Plate Mail", skills)
            self.assertEqual(summary["warlock_class_skill_row_count"], 3)
            self.assertEqual(manifest["class_mask"], 256)
            self.assertIn("warlock-equipment-skills.json", index["files"])

    def test_missing_warlock_source_rows_fail_closed(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            data = root / "data"
            data.mkdir()
            (data / "manifest.json").write_text(
                json.dumps({"client_build": "1.60.1.70425", "files": {}}))
            lines = root / "SkillLine.csv"
            info = root / "SkillRaceClassInfo.csv"
            lines.write_text(
                "ID,DisplayName_lang,CategoryID\n415,Cloth,8\n", encoding="utf-8")
            info.write_text(
                "SkillID,ClassMask,MinLevel,Availability,RaceMasks_0,RaceMasks_1\n"
                "415,2,1,1,-1,-1\n", encoding="utf-8")
            with self.assertRaisesRegex(
                ValueError, "no rows for Warlock class mask 256"):
                import_paladin_equipment(
                    lines, info, data, class_name="warlock", class_id=9)


if __name__ == "__main__":
    unittest.main()
