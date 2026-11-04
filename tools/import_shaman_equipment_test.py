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

    def row(self, row_id, skill_id, race_mask, class_mask=64, min_level=1, availability=1, race_mask_high=0):
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
            {"ID": 55, "DisplayName_lang": "Shield", "CategoryID": 8},
            {"ID": 182, "DisplayName_lang": "Totems", "CategoryID": 7},
            {"ID": 43, "DisplayName_lang": "Swords", "CategoryID": 6},
        ]
        all_races = -1
        rows = [
            self.row(1, 44, all_races, min_level=1),
            self.row(2, 172, all_races, min_level=20),
            self.row(3, 45, all_races, min_level=1),
            # Availability=0 means not granted automatically at character creation;
            # MinLevel still records the class's trainer-gated proficiency threshold.
            self.row(4, 46, all_races, min_level=40, availability=0),
            self.row(5, 415, 0, class_mask=0, min_level=0),
            self.row(6, 55, all_races, min_level=1),
            self.row(7, 182, all_races, min_level=1),
            # Race-restricted rows are not widened into class-wide access.
            self.row(8, 43, 2, race_mask_high=0),
            self.row(9, 43, 32, race_mask_high=0),
            # A non-Shaman class row must not affect the projection.
            self.row(10, 43, all_races, class_mask=256),
        ]

        summary, payload, manifest, main_manifest = self.run_import(skill_lines, rows)
        skills = {skill["name"]: skill for skill in payload["skills"]}

        self.assertEqual(payload["client_build"], "1.60.1.70425")
        self.assertEqual(payload["class"], "shaman")
        self.assertEqual(payload["class_mask"], 64)
        self.assertEqual(
            payload["race_policy"],
            "class-wide only; SkillRaceClassInfo race mask must be unrestricted",
        )
        self.assertEqual(skills["Axes"]["min_level"], 1)
        self.assertEqual(skills["Two-Handed Axes"]["min_level"], 20)
        self.assertEqual(skills["Leather"]["min_level"], 1)
        self.assertEqual(skills["Mail"]["min_level"], 40)
        self.assertEqual(skills["Mail"]["availability_values"], [0])
        self.assertEqual(skills["Cloth"]["min_level"], 1)
        self.assertEqual(skills["Shield"]["min_level"], 1)
        self.assertEqual(skills["Totems"]["min_level"], 1)
        self.assertIsNone(skills["Swords"]["min_level"])
        self.assertFalse(skills["Swords"]["race_unrestricted"])
        self.assertEqual(summary["class_wide_skill_count"], 7)
        self.assertEqual(summary["race_specific_or_unavailable_skill_count"], 1)
        self.assertEqual(manifest["source"]["client_build"], "1.60.1.70425")
        self.assertIn("shaman-equipment-skills.json", main_manifest["files"])
        self.assertIn("shaman-equipment-skills.manifest.json", main_manifest["files"])

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

    def test_normalizes_signed_32_bit_race_mask_words(self):
        skill_lines = [{"ID": 136, "DisplayName_lang": "Staves", "CategoryID": 7}]
        rows = [self.row(1, 136, -1)]
        _, payload, _, _ = self.run_import(skill_lines, rows)
        self.assertEqual(payload["race_mask"], 0xFFFFFFFF)
        self.assertEqual(payload["race_ids"], list(range(1, 33)))
        self.assertEqual(payload["skills"][0]["min_level"], 1)

    def test_normalizes_both_signed_race_mask_words_without_sign_extension(self):
        skill_lines = [{"ID": 136, "DisplayName_lang": "Staves", "CategoryID": 7}]
        rows = [self.row(1, 136, -1)]
        rows[0]["RaceMasks_1"] = -1
        _, payload, _, _ = self.run_import(skill_lines, rows)
        self.assertEqual(payload["race_mask"], (1 << 64) - 1)
        self.assertEqual(payload["race_ids"], list(range(1, 65)))
        skill = payload["skills"][0]
        self.assertEqual(skill["min_level"], 1)
        self.assertEqual(
            list(skill["per_race_min_level"]),
            [str(race_id) for race_id in range(1, 65)],
        )

    def test_rejects_missing_skill_line_reference(self):
        skill_lines = [{"ID": 44, "DisplayName_lang": "Axes", "CategoryID": 6}]
        rows = [self.row(1, 999, -1)]
        with self.assertRaisesRegex(ValueError, "missing SkillLine ID 999"):
            self.run_import(skill_lines, rows)

    def test_rejects_source_without_shaman_rows(self):
        skill_lines = [{"ID": 44, "DisplayName_lang": "Axes", "CategoryID": 6}]
        rows = [self.row(1, 44, -1, class_mask=256)]
        with self.assertRaisesRegex(ValueError, "no rows for Shaman"):
            self.run_import(skill_lines, rows)


if __name__ == "__main__":
    unittest.main()
