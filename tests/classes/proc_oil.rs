//! Shadow Oil, a MythicSim Rust-only weapon imbue beyond the Go reference pin (client 70707,
//! temporary enchantment 25 of item 3824, use spell 3594): the twin of Frost Oil that casts the
//! Shadow Bolt of spell 350026 on a landed hit of the oiled hand, at the flat 15% its row
//! states. No Go golden exists, so these tests pin the client row's numbers.

use forever_engine::{contracts::prepared_v2::PreparedV2, prepare_json};
use serde_json::{json, Value};
use std::{fs, path::Path};

const SHADOW_BOLT: i64 = 350026;
const SHADOW_OIL_USE_SPELL: i64 = 3594;
const SHADOW_OIL_ITEM: i64 = 3824;
const FROST_OIL_ITEM: i64 = 3829;
/// Long enough, over many iterations, for a proc rate to settle within a point or two.
const ITERATIONS: u32 = 40;

/// A Combat Rogue request, dual wielding, with the oils in its imbue slots and `bonus_spell_damage`
/// added to its prepared spell damage, simulated for a long fight over many iterations.
fn simulate(main: i64, off: i64, bonus_spell_damage: f64) -> (Value, Value) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/classes/rogue/prepare/combat-wound-poison.request.json");
    let mut request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    request["encounter"]["duration"] = json!(600);
    request["simOptions"]["iterations"] = json!(ITERATIONS);
    request["simOptions"]["debugFirstIteration"] = json!(false);
    let consumables = &mut request["raid"]["parties"][0]["players"][0]["consumables"];
    consumables["mhImbueId"] = json!(main);
    consumables["ohImbueId"] = json!(off);
    let mut prepared = prepare_json(&serde_json::to_vec(&request).unwrap(), "shadow-oil").unwrap();
    let stats = &mut prepared["player"]["stats"]["SpellDamage"];
    *stats = json!(stats.as_f64().unwrap() + bonus_spell_damage);
    let typed: PreparedV2 = serde_json::from_value(prepared.clone()).unwrap();
    let report = forever_engine::simulate_prepared(&typed).unwrap();
    (prepared, report.result)
}

fn actions(result: &Value) -> &Vec<Value> {
    result["raidMetrics"]["parties"][0]["players"][0]["actions"]
        .as_array()
        .unwrap()
}

/// The sum of a metrics counter over every target of an action.
fn count(action: &Value, key: &str) -> f64 {
    action["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|target| target[key].as_f64().unwrap_or(0.0))
        .sum()
}

/// A prepared spell's action id as a result's metrics write it.
fn metrics_id(prepared_id: &Value) -> Value {
    let mut id = serde_json::Map::new();
    for (key, value) in prepared_id.as_object().unwrap() {
        let zero = value.is_null() || value.as_i64() == Some(0);
        let name = match key.as_str() {
            "spell_id" => "spellId",
            "other_id" => "otherId",
            "item_id" => "itemId",
            other => other,
        };
        if !zero {
            id.insert(name.to_string(), value.clone());
        }
    }
    Value::Object(id)
}

/// The landed hits, over the whole run, of every spell whose proc mask is of the given hand.
fn landed_hits(prepared: &Value, result: &Value, hand: &str) -> f64 {
    prepared["player"]["spells"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|spell| {
            spell["proc_mask"]
                .as_array()
                .unwrap()
                .iter()
                .any(|mask| mask.as_str().unwrap().starts_with(hand))
        })
        .map(|spell| {
            let id = metrics_id(&spell["action_id"]);
            actions(result)
                .iter()
                .filter(|action| action["id"] == id)
                .map(|action| {
                    ["hits", "crits", "glances", "blocks"]
                        .iter()
                        .map(|key| count(action, key))
                        .sum::<f64>()
                })
                .sum::<f64>()
        })
        .sum()
}

fn bolt_action(result: &Value) -> &Value {
    actions(result)
        .iter()
        .find(|action| action["id"] == json!({ "spellId": SHADOW_BOLT }))
        .expect("Shadow Bolt was cast")
}

/// The Shadow Bolts cast over the run, whether they hit or missed.
fn bolts(result: &Value) -> f64 {
    let action = bolt_action(result);
    ["hits", "crits", "misses"]
        .iter()
        .map(|key| count(action, key))
        .sum()
}

fn effect<'a>(prepared: &'a Value, aura: &str) -> Option<&'a Value> {
    prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["trigger_aura"] == aura)
}

/// The proc mask names of the spells one effect listens to.
fn listened_masks(prepared: &Value, effect: &Value) -> Vec<String> {
    effect["trigger_spells"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|position| {
            prepared["player"]["spells"][position.as_u64().unwrap() as usize]["proc_mask"]
                .as_array()
                .unwrap()
                .iter()
                .map(|mask| mask.as_str().unwrap().to_string())
        })
        .collect()
}

/// The oiled hand's listener is the flat chance of the client row, and only the oiled hand has
/// one.
fn assert_listener(prepared: &Value, oiled: &str, other: &str, hand_prefix: &str) {
    let listener = effect(prepared, &format!("Shadow Oil ({oiled})")).expect("the oiled hand");
    assert_eq!(listener["kind"], "spell_data_damage_proc");
    assert_eq!(listener["proc_chance"], 0.15);
    assert_eq!(listener["landed_only"], true);
    assert!(
        listener.get("chances").is_none(),
        "a flat chance, not procs per minute"
    );
    let masks = listened_masks(prepared, listener);
    assert!(!masks.is_empty());
    assert!(
        masks.iter().all(|mask| mask.starts_with(hand_prefix)),
        "{masks:?}"
    );
    assert!(effect(prepared, &format!("Shadow Oil ({other})")).is_none());
}

/// The Shadow Bolt follows about 15% of the landed hits of the oiled hand and none of the other
/// hand's. The expected rate of the oiled hand is 0.15; if the bolt followed both hands it would
/// read the 0.15 over the oiled hand's share of the hits.
fn assert_rate(prepared: &Value, result: &Value, oiled: &str, other: &str) {
    let oiled_hits = landed_hits(prepared, result, oiled);
    let other_hits = landed_hits(prepared, result, other);
    assert!(
        oiled_hits > 3000.0 && other_hits > 3000.0,
        "{oiled_hits} {other_hits}"
    );
    let rate = bolts(result) / oiled_hits;
    assert!(
        (0.13..=0.17).contains(&rate),
        "{} bolts of {oiled_hits} landed {oiled} hits is {rate}",
        bolts(result)
    );
    let both = bolts(result) / (oiled_hits + other_hits);
    assert!(
        both < 0.12,
        "a rate of {both} over both hands' hits follows the other hand"
    );
}

#[test]
fn shadow_oil_casts_a_shadow_bolt_on_15_percent_of_the_main_hand_hits() {
    for id in [SHADOW_OIL_USE_SPELL, SHADOW_OIL_ITEM] {
        let (prepared, result) = simulate(id, 0, 0.0);
        assert_listener(&prepared, "main hand", "off hand", "ProcMaskMeleeMH");
        assert_rate(&prepared, &result, "ProcMaskMeleeMH", "ProcMaskMeleeOH");
    }
}

#[test]
fn shadow_oil_casts_a_shadow_bolt_on_15_percent_of_the_off_hand_hits() {
    for id in [SHADOW_OIL_USE_SPELL, SHADOW_OIL_ITEM] {
        let (prepared, result) = simulate(0, id, 0.0);
        assert_listener(&prepared, "off hand", "main hand", "ProcMaskMeleeOH");
        assert_rate(&prepared, &result, "ProcMaskMeleeOH", "ProcMaskMeleeMH");
    }
}

/// The bolt's damage is its row's roll plus 0.56 of spell power: 1,000 more spell power adds 560
/// to the average hit that does not crit.
#[test]
fn the_shadow_bolt_scales_with_spell_power_at_0_56() {
    let mean_hit = |bonus: f64| {
        let (prepared, result) = simulate(SHADOW_OIL_ITEM, 0, bonus);
        let bolt = prepared["player"]["spells"]
            .as_array()
            .unwrap()
            .iter()
            .find(|spell| spell["action_id"]["spell_id"] == SHADOW_BOLT)
            .unwrap();
        assert!((bolt["bonus_coefficient"].as_f64().unwrap() - 0.56).abs() < 1e-6);
        assert_eq!(bolt["school"], 32, "Shadow");
        let range = &bolt_action(&result)["targets"][0]["hitRange"];
        range["total"].as_f64().unwrap() / range["count"].as_f64().unwrap()
    };
    let slope = (mean_hit(1000.0) - mean_hit(0.0)) / 1000.0;
    assert!(
        (0.54..=0.58).contains(&slope),
        "{slope} damage for each spell power"
    );
}

/// One oil on each hand registers each hand's own listener and bolt, and neither oil's bolt
/// follows the other hand.
#[test]
fn shadow_oil_and_frost_oil_sit_on_different_hands() {
    let (prepared, result) = simulate(SHADOW_OIL_ITEM, FROST_OIL_ITEM, 0.0);
    assert_listener(&prepared, "main hand", "off hand", "ProcMaskMeleeMH");
    let frost = effect(&prepared, "Frost Oil (off hand)").expect("Frost Oil on the off hand");
    assert_eq!(frost["proc_chance"], 0.1);
    assert!(effect(&prepared, "Frost Oil (main hand)").is_none());
    let frost_bolts: f64 = actions(&result)
        .iter()
        .filter(|action| action["id"] == json!({ "spellId": 350025 }))
        .map(|action| {
            ["hits", "crits", "misses"]
                .iter()
                .map(|key| count(action, key))
                .sum::<f64>()
        })
        .sum();
    let rate = |bolts: f64, hand: &str| bolts / landed_hits(&prepared, &result, hand);
    assert!((0.13..=0.17).contains(&rate(bolts(&result), "ProcMaskMeleeMH")));
    assert!((0.08..=0.12).contains(&rate(frost_bolts, "ProcMaskMeleeOH")));
}
