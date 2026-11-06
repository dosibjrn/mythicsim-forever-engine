//! Regression for selected-native L20 request/export transport, not an independent
//! Warcraft spellbook or legality check. The exported effects must name the same
//! actions that the native preparation actually registered for this actor.

use forever_engine::{prepare_json_with_actor_level, Rules};
use std::collections::BTreeSet;

#[test]
fn warlock_l20_exported_effects_reference_registered_native_actions() {
    let request = include_bytes!("../fixtures/warlock-l20-troll-native.request.json");
    let prepared =
        prepare_json_with_actor_level(request, "warlock-l20-native-registration", Rules::Forever, Some(20))
            .expect("selected Troll Warlock L20 request must prepare");
    assert_eq!(prepared["player"]["level"], 20);
    let spells: BTreeSet<i64> = prepared["player"]["spells"]
        .as_array()
        .expect("prepared spellbook")
        .iter()
        .filter_map(|spell| spell["action_id"]["spell_id"].as_i64())
        .collect();
    let effects = prepared["effects"].as_array().expect("native effects");
    for kind in ["immolate", "corruption", "drain_life", "bane_of_agony", "life_tap"] {
        let effect = effects
            .iter()
            .find(|effect| effect["kind"] == kind)
            .unwrap_or_else(|| panic!("prepared Warlock is missing {kind} effect"));
        let spell_id = effect["spell_id"].as_i64().expect("effect spell_id");
        assert!(
            spells.contains(&spell_id),
            "level-20 {kind} effect references unregistered native spell {spell_id}"
        );
    }
}
