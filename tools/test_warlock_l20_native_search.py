"""Source-derived APL proposal regression; no mock DPS or Warcraft rules."""
import unittest

from warlock_l20_native_search import (
    native_spell_candidates, cast_apl, multi_apl, mana_conditioned_apl,
)


class NativeWarlockAplSourceTests(unittest.TestCase):
    def test_discovers_from_rust_options_and_prefers_highest_level_rank(self):
        options = {"level": 20, "spells": [
            {"spell_id": 10, "name": "Native Bolt", "minimum_level": 1},
            {"spell_id": 20, "name": "Native Bolt", "minimum_level": 20},
            {"spell_id": 30, "name": "Native Curse", "minimum_level": 15},
            {"spell_id": 40, "name": "Too High", "minimum_level": 30},
        ]}
        request = {"raid": {"parties": [{"players": [{
            "rotation": {"priorityList": [{"action": {
                "castSpell": {"spellId": {"spellId": 90}}}}]}
        }]}]}}
        found = native_spell_candidates(options, request)
        ids = [row["spell_id"] for row in found]
        self.assertEqual(set(ids), {20, 30, 90})
        self.assertNotIn(10, ids)
        self.assertNotIn(40, ids)

    def test_multispell_dot_maintenance_guard_and_filler(self):
        apl = multi_apl([(300, "dot_missing"), (400, None)])
        self.assertEqual(apl[0], {"action": {
            "castSpell": {"spellId": {"spellId": 300}},
            "condition": {"not": {"val": {
                "dotIsActive": {"spellId": {"spellId": 300}}
            }}}
        }})
        self.assertEqual(apl[1], cast_apl(400))

    def test_resource_condition_is_native_apl_schema(self):
        action = mana_conditioned_apl(500, 40)
        self.assertEqual(action["action"]["condition"], {"cmp": {
            "lhs": {"currentManaPercent": {}}, "op": "OpLt",
            "rhs": {"const": {"val": "40%"}}
        }})

    def test_aura_maintenance_uses_rust_target_semantics(self):
        action = cast_apl(600, guard="aura_missing")
        self.assertEqual(
            action["action"]["condition"]["not"]["val"]["auraIsActive"]["sourceUnit"],
            {"type": "CurrentTarget"})


if __name__ == "__main__":
    unittest.main()
