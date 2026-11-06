"""Source-derived APL proposal regression; no mock DPS or Warcraft rules."""
import unittest

from warlock_l20_native_search import (
    native_spell_candidates, cast_apl, multi_apl, mana_conditioned_apl,
    choose_paid_gear_upgrade, native_gear_refinement_proposals,
)


class NativeWarlockAplSourceTests(unittest.TestCase):
    def test_refinement_only_changes_one_source_item_and_preserves_joint_axes(self):
        original = {"raid": {"parties": [{"players": [{
            "talentsString": "native_selected_talents",
            "rotation": {"priorityList": [{"action": {
                "castSpell": {"spellId": {"spellId": 1088}}}}]},
            "equipment": {"items": [{"id": 101}, {"id": 202}, {"id": 303}]},
        }]}]}}
        proposals = list(native_gear_refinement_proposals(
            original, 1, [202, 404, 404, 505]))
        self.assertEqual([item_id for item_id, _ in proposals], [404, 505])
        self.assertEqual([row["id"] for row in
                          original["raid"]["parties"][0]["players"][0]
                          ["equipment"]["items"]], [101, 202, 303])
        for item_id, proposal in proposals:
            actor = proposal["raid"]["parties"][0]["players"][0]
            self.assertEqual(actor["talentsString"], "native_selected_talents")
            self.assertEqual(actor["rotation"], original["raid"]["parties"][0]
                             ["players"][0]["rotation"])
            self.assertEqual([row["id"] for row in actor["equipment"]["items"]],
                             [101, item_id, 303])

    def test_native_paid_gear_selection_never_degrades_current_build(self):
        # Real Rust responses may refuse an item or pay less DPS than naked.
        # Neither is evidence of an improved equipped candidate.
        item_id, dps = choose_paid_gear_upgrade(12.5, [
            (100, None), (101, {"dps": 8.1}), (102, {"dps": 12.5})])
        self.assertIsNone(item_id)
        self.assertEqual(dps, 12.5)

    def test_native_paid_gear_selection_uses_strict_best_upgrade(self):
        item_id, dps = choose_paid_gear_upgrade(12.5, [
            (100, {"dps": 12.8}), (101, {"dps": 14.0}),
            (102, {"dps": 13.1}), (103, {"dps": 14.0})])
        self.assertEqual((item_id, dps), (101, 14.0))

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
