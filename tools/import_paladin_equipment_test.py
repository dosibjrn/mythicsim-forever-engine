import csv
import json
import tempfile
import unittest
from pathlib import Path

from import_paladin_equipment import import_paladin_equipment


class ImportPaladinEquipmentTests(unittest.TestCase):
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
        summary = import_paladin_equipment(skill_line_csv, skill_info_csv, data)
        payload = json.loads((data / "paladin-equipment-skills.json").read_text(encoding="utf-8"))
        manifest = json.loads((data / "paladin-equipment-skills.manifest.json").read_text(encoding="utf-8"))
        return summary, payload, manifest, json.loads((data / "manifest.json").read_text(encoding="utf-8"))

    def row(self, row_id, skill_id, race_mask, class_mask=2, min_level=1, availability=1, race_mask_high=0):
        return {
            "ID": row_id,
            "SkillID": skill_id,
            "ClassMask": class_mask,
            "Flags": 0,
            "Availability": availability,
            "MinLevel": min_level,
            "SkillTierID": 0,
            "RaceMasks_0": race_mask,
            "RaceMasks_1": race_mask_high,
        }

    def test_class_wide_min_level_uses_only_unrestricted_race_rows(self):
        skill_lines = [
            {"ID": 44, "DisplayName_lang": "Axes", "CategoryID": 6},
            {"ID": 172, "DisplayName_lang": "Two-Handed Axes", "CategoryID": 6},
            {"ID": 45, "DisplayName_lang": "Leather", "CategoryID": 8},
            {"ID": 46, "DisplayName_lang": "Mail", "CategoryID": 8},
            {"ID": 415, "DisplayName_lang": "Cloth", "CategoryID": 8},
            {"ID": 433, "DisplayName_lang": "Shield", "CategoryID": 8},
            {"ID": 293, "DisplayName_lang": "Plate Mail", "CategoryID": 7},
            {"ID": 43, "DisplayName_lang": "Swords", "CategoryID": 6},
        ]
        all_races = -1
        rows = [
            self.row(1, 44, all_races, min_level=1),
            self.row(2, 172, all_races, min_level=20),
            self.row(3, 45, all_races, min_level=1),
            # Availability=0 means not granted automatically at character creation;
            # MinLevel still records the class's trainer-gated proficiency threshold.
            self.row(4, 46, all_races, min_level=1, availability=0),
            self.row(5, 415, 0, class_mask=0, min_level=0),
            self.row(6, 433, all_races, min_level=1),
            self.row(7, 293, all_races, min_level=40),
            # Race-restricted rows are not widened into class-wide access.
            self.row(8, 43, 2, race_mask_high=0),
            self.row(9, 43, 32, race_mask_high=0),
            # A non-Paladin class row must not affect the projection.
            self.row(10, 43, all_races, class_mask=256),
        ]

        summary, payload, manifest, main_manifest = self.run_import(skill_lines, rows)
        skills = {skill["name"]: skill for skill in payload["skills"]}

        self.assertEqual(payload["client_build"], "1.60.1.70425")
        self.assertEqual(payload["class"], "paladin")
        self.assertEqual(payload["class_mask"], 2)
        self.assertEqual(payload["skill_line_ids"], sorted(int(line["ID"]) for line in skill_lines if line["DisplayName_lang"] in __import__("import_paladin_equipment").EQUIPMENT_SKILL_LINE_NAMES))
        self.assertEqual(
            payload["race_policy"],
            "class-wide only; SkillRaceClassInfo race mask must be unrestricted",
        )
        self.assertEqual(skills["Axes"]["min_level"], 1)
        self.assertEqual(skills["Two-Handed Axes"]["min_level"], 20)
        self.assertEqual(skills["Leather"]["min_level"], 1)
        self.assertEqual(skills["Mail"]["min_level"], 1)
        self.assertEqual(skills["Mail"]["availability_values"], [0])
        self.assertEqual(skills["Cloth"]["min_level"], 1)
        self.assertEqual(skills["Shield"]["min_level"], 1)
        self.assertEqual(skills["Plate Mail"]["min_level"], 40)
        self.assertIsNone(skills["Swords"]["min_level"])
        self.assertFalse(skills["Swords"]["race_unrestricted"])
        self.assertEqual(summary["class_wide_skill_count"], 7)
        self.assertEqual(summary["race_specific_or_unavailable_skill_count"], 1)
        self.assertEqual(manifest["source"]["client_build"], "1.60.1.70425")
        self.assertIn("paladin-equipment-skills.json", main_manifest["files"])
        self.assertIn("paladin-equipment-skills.manifest.json", main_manifest["files"])

    def test_skill_line_index_marks_absent_class_rows_as_known_unavailable(self):
        skill_lines = [
            {"ID": 43, "DisplayName_lang": "Swords", "CategoryID": 6},
            {"ID": 44, "DisplayName_lang": "Axes", "CategoryID": 6},
            {"ID": 229, "DisplayName_lang": "Polearms", "CategoryID": 6},
        ]
        rows = [self.row(1, 44, -1)]
        _, payload, _, _ = self.run_import(skill_lines, rows)
        self.assertEqual(payload["skill_line_ids"], [43, 44, 229])
        self.assertEqual(
            payload["skill_line_names"],
            [
                {"skill_id": 43, "name": "Swords"},
                {"skill_id": 44, "name": "Axes"},
                {"skill_id": 229, "name": "Polearms"},
            ],
        )
        self.assertEqual(payload["schema_version"], 2)
        skill_ids = {skill["skill_id"] for skill in payload["skills"]}
        self.assertIn(44, skill_ids)
        self.assertNotIn(43, skill_ids)
        self.assertNotIn(229, skill_ids)

    def test_supports_split_64_bit_race_masks_without_widening(self):
        skill_lines = [{"ID": 136, "DisplayName_lang": "Staves", "CategoryID": 6}]
        rows = [self.row(1, 136, 2, race_mask_high=1)]
        summary, payload, _, _ = self.run_import(skill_lines, rows)
        self.assertIsNone(payload["skills"][0]["min_level"])
        self.assertFalse(payload["skills"][0]["race_unrestricted"])
        self.assertEqual(summary["race_specific_or_unavailable_skill_count"], 1)

    def test_accepts_both_zero_and_all_bits_as_unrestricted_race_masks(self):
        skill_lines = [
            {"ID": 136, "DisplayName_lang": "Staves", "CategoryID": 6},
            {"ID": 45, "DisplayName_lang": "Leather", "CategoryID": 8},
        ]
        rows = [
            self.row(1, 136, 0, race_mask_high=0),
            self.row(2, 45, 4294967295, race_mask_high=4294967295),
        ]
        _, payload, _, _ = self.run_import(skill_lines, rows)
        skills = {skill["name"]: skill for skill in payload["skills"]}
        self.assertEqual(skills["Staves"]["min_level"], 1)
        self.assertEqual(skills["Leather"]["min_level"], 1)

    def test_normalizes_signed_32_bit_race_mask_word_as_unrestricted(self):
        skill_lines = [{"ID": 136, "DisplayName_lang": "Staves", "CategoryID": 6}]
        rows = [self.row(1, 136, -1)]
        _, payload, _, _ = self.run_import(skill_lines, rows)
        skill = payload["skills"][0]
        self.assertTrue(skill["race_unrestricted"])
        self.assertEqual(skill["min_level"], 1)

    def test_negative_low_word_does_not_sign_extend_into_high_word(self):
        skill_lines = [{"ID": 136, "DisplayName_lang": "Staves", "CategoryID": 6}]
        rows = [self.row(1, 136, -1, race_mask_high=1)]
        _, payload, _, _ = self.run_import(skill_lines, rows)
        skill = payload["skills"][0]
        self.assertFalse(skill["race_unrestricted"])
        self.assertIsNone(skill["min_level"])

    def test_normalizes_both_negative_words_to_unrestricted_64_bit_mask(self):
        skill_lines = [{"ID": 136, "DisplayName_lang": "Staves", "CategoryID": 6}]
        rows = [self.row(1, 136, -1, race_mask_high=-1)]
        _, payload, _, _ = self.run_import(skill_lines, rows)
        skill = payload["skills"][0]
        self.assertTrue(skill["race_unrestricted"])
        self.assertEqual(skill["min_level"], 1)

    def test_rejects_missing_skill_line_reference(self):
        skill_lines = [{"ID": 44, "DisplayName_lang": "Axes", "CategoryID": 6}]
        rows = [self.row(1, 999, -1)]
        with self.assertRaisesRegex(ValueError, "missing SkillLine ID 999"):
            self.run_import(skill_lines, rows)

    def test_rejects_source_without_paladin_rows(self):
        skill_lines = [{"ID": 44, "DisplayName_lang": "Axes", "CategoryID": 6}]
        rows = [self.row(1, 44, -1, class_mask=256)]
        with self.assertRaisesRegex(ValueError, "no rows for Paladin"):
            self.run_import(skill_lines, rows)


if __name__ == "__main__":
    unittest.main()
