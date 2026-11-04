//! Mutilate's two hand strikes, the main hand's 1241586 and the off hand's 1241590 under the
//! parent 1241584, each roll their own crit, and the client's crit modifiers name both by their
//! shared class flag (word 1, bit 1): Puncturing Wounds' third effect (1224716, mask word 1 = 6)
//! and Cold Blood (14177, mask word 1 = 6). PastelStorm found the Go fork's off hand short of
//! both (mythicsim-forever-engine-go #31 and #32); these pin that both hands take them here.

use forever_engine::RngMode;
use forever_engine::{contracts::prepared_v2::PreparedV2, prepare_json_with, Rules};
use serde_json::{json, Value};
use std::{fs, path::Path};

const MUTILATE: i64 = 1241584;
const COLD_BLOOD: i64 = 14177;

/// The Assassination preparation case on these talents.
fn request(talents: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/classes/rogue/prepare/assassination.request.json");
    let mut request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    request["raid"]["parties"][0]["players"][0]["talentsString"] = json!(talents);
    request
}

/// The case's talents with Puncturing Wounds, the fourth Combat talent, at `rank`.
fn puncturing_wounds(rank: u32) -> String {
    format!("0053031055110105-302{rank}0320201401")
}

fn prepared(request: &Value) -> Value {
    let mut request = request.clone();
    crate::rogue_113483::forever_request(&mut request);
    prepare_json_with(request.to_string().as_bytes(), "rust", Rules::Forever).unwrap()
}

/// The prepared crit bonus of Mutilate's strike with this tag.
fn strike_crit(prepared: &Value, tag: i64) -> f64 {
    prepared["player"]["spells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|spell| {
            spell["action_id"]["spell_id"] == MUTILATE && spell["action_id"]["tag"] == tag
        })
        .unwrap_or_else(|| panic!("no Mutilate strike with tag {tag}"))["bonus_crit_percent"]
        .as_f64()
        .unwrap()
}

/// The landed outcomes of Mutilate's strike with this tag over every fight: (hits, crits).
fn strike_outcomes(result: &Value, tag: i64) -> (f64, f64) {
    let mut outcomes = (0.0, 0.0);
    for action in result["raidMetrics"]["parties"][0]["players"][0]["actions"]
        .as_array()
        .unwrap()
    {
        if action["id"]["spellId"] != MUTILATE || action["id"]["tag"] != tag {
            continue;
        }
        for target in action["targets"].as_array().unwrap() {
            outcomes.0 += target["hits"].as_f64().unwrap_or(0.0);
            outcomes.1 += target["crits"].as_f64().unwrap_or(0.0);
        }
    }
    outcomes
}

fn simulate(mut prepared: Value, iterations: u32, log: bool) -> Value {
    prepared["sim"]["iterations"] = json!(iterations);
    prepared["sim"]["debug_first_iteration"] = json!(log);
    let typed: PreparedV2 = serde_json::from_value(prepared).unwrap();
    forever_engine::simulate_prepared(&typed, RngMode::Stochastic)
        .unwrap()
        .result
}

/// Puncturing Wounds' 5, 10 and 15% Mutilate crit reach both hand strikes, and each hand crits
/// that much more often in the fights.
#[test]
fn puncturing_wounds_raises_both_hands_crit() {
    let base = prepared(&request(&puncturing_wounds(0)));
    for rank in 1..=3 {
        let ranked = prepared(&request(&puncturing_wounds(rank)));
        for tag in [1, 2] {
            assert_eq!(
                strike_crit(&ranked, tag) - strike_crit(&base, tag),
                5.0 * f64::from(rank),
                "rank {rank}, tag {tag}"
            );
        }
    }

    let crit_rate = |result: &Value, tag: i64| {
        let (hits, crits) = strike_outcomes(result, tag);
        crits / (hits + crits)
    };
    let without = simulate(base, 100, false);
    let with = simulate(prepared(&request(&puncturing_wounds(3))), 100, false);
    for tag in [1, 2] {
        let gain = crit_rate(&with, tag) - crit_rate(&without, tag);
        assert!((0.12..0.18).contains(&gain), "tag {tag}: {gain}");
    }
}

/// Cold Blood crits both hand strikes of the Mutilate after it, then the off hand strike spends
/// it. The rotation casts Cold Blood whenever it is ready and Mutilate otherwise, over a fight
/// long enough for several charges. A charge whose Mutilate missed is spent by the parent.
#[test]
fn cold_blood_crits_both_mutilate_hands() {
    let mut request = request(&puncturing_wounds(0));
    let cast = |id: i64| json!({"action": {"castSpell": {"spellId": {"spellId": id}}}});
    request["raid"]["parties"][0]["players"][0]["rotation"] =
        json!({"type": "TypeAPL", "priorityList": [cast(COLD_BLOOD), cast(MUTILATE)]});
    request["encounter"]["duration"] = json!(600);
    let result = simulate(prepared(&request), 1, true);
    let logs = result["logs"].as_str().unwrap();

    let main = format!("{{SpellID: {MUTILATE}, Tag: 1}}");
    let off = format!("{{SpellID: {MUTILATE}, Tag: 2}}");
    let outcome = |line: &str, strike: &str| {
        line.split_once(&format!("[Target 1] {strike} "))
            .map(|(_, rest)| rest.split(' ').next().unwrap().to_string())
            .filter(|result| result != "[DEBUG]")
    };
    let gained = format!("Aura gained: {{SpellID: {COLD_BLOOD}}}");
    let faded = format!("Aura faded: {{SpellID: {COLD_BLOOD}}}");
    let mut charges = 0;
    let mut active = false;
    let mut strikes = Vec::new();
    for line in logs.lines() {
        if line.contains(&gained) {
            active = true;
            strikes.clear();
        } else if line.contains(&faded) && active {
            // A landed parent: two strikes, both crits, the off hand's last.
            if !strikes.is_empty() {
                assert_eq!(strikes, ["MH Crit", "OH Crit"], "{line}");
                charges += 1;
            }
            active = false;
        } else if active {
            if let Some(result) = outcome(line, &main) {
                strikes.push(format!("MH {result}"));
            } else if let Some(result) = outcome(line, &off) {
                strikes.push(format!("OH {result}"));
            }
        }
    }
    assert!(charges >= 2, "only {charges} Cold Blood charges spent");
}
