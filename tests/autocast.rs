//! Autocast Other Cooldowns under Forever's rule (`autocast_skips_unready_cooldowns`): it passes
//! over a ready cooldown the GCD holds back and looks at the next, so an explosive no longer
//! waits behind a cooldown that waits for the GCD, nor holds the cooldowns sorted after it
//! back through a run of hardcasts (wowsims/forever 10b0e65f2 and fbf5d98e8).

use forever_engine::contracts::prepared_v2::PreparedV2;
use serde_json::{json, Value};
use std::{fs, path::Path};

const ITERATIONS: u64 = 300;

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/mage/prepared-v2/{name}.prepared.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// The explosives' item ids among the player's major cooldowns.
fn explosives(value: &Value) -> Vec<i64> {
    value["player"]["major_cooldowns"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|cooldown| {
            cooldown["type"]
                .as_array()
                .is_some_and(|kinds| kinds.contains(&json!("explosive")))
        })
        .filter_map(|cooldown| cooldown["action_id"]["item_id"].as_i64())
        .collect()
}

/// Uses per iteration of the given items, and damage per second, with or without the rule.
fn run(mut value: Value, forever: bool, items: &[i64]) -> (Vec<f64>, f64) {
    value["sim"]["iterations"] = json!(ITERATIONS);
    if forever {
        value["player"]["autocast_skips_unready_cooldowns"] = json!(true);
    }
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    let report = forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
    let player = &report.result["raidMetrics"]["parties"][0]["players"][0];
    let uses = items
        .iter()
        .map(|item| {
            player["actions"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|action| action["id"]["itemId"].as_i64() == Some(*item))
                .flat_map(|action| action["targets"].as_array().unwrap().iter())
                .map(|target| target["casts"].as_f64().unwrap_or(0.0))
                .sum::<f64>()
                / ITERATIONS as f64
        })
        .collect();
    (uses, player["dps"]["avg"].as_f64().unwrap())
}

/// The Protection Paladin reference's Dense Dynamite (18641) and Demonic Rune (12662): under the
/// reference's rule a rune waiting for the GCD held the dynamite sorted after it, and the
/// dynamite, waiting for a GCD to start, held the rune; under Forever's both go more often.
#[test]
fn a_cooldown_the_gcd_holds_back_no_longer_holds_the_others() {
    let value = fixture("production-protection-paladin");
    assert_eq!(explosives(&value), vec![18641]);
    let (go, go_dps) = run(value.clone(), false, &[18641, 12662]);
    let (forever, dps) = run(value, true, &[18641, 12662]);
    println!(
        "dynamite {:.3} -> {:.3}, rune {:.3} -> {:.3}, dps {go_dps:.2} -> {dps:.2}",
        go[0], forever[0], go[1], forever[1]
    );
    assert!(
        forever[0] > go[0],
        "dynamite {} against {}",
        forever[0],
        go[0]
    );
    assert!(forever[1] > go[1], "rune {} against {}", forever[1], go[1]);
    assert!(dps > go_dps, "{dps} against {go_dps}");
}

/// Forever preparation writes the rule; the reference's rules leave it out.
#[test]
fn only_forever_preparation_lets_autocast_skip_unready_cooldowns() {
    let request = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/mage/prepared-v2/production-arms-warrior.request.json"),
    )
    .unwrap();
    let forever = forever_engine::prepare_json(&request, "x").unwrap();
    let go = forever_engine::prepare_json_with(&request, "x", forever_engine::Rules::GoReference)
        .unwrap();
    assert_eq!(
        forever["player"]["autocast_skips_unready_cooldowns"],
        json!(true)
    );
    assert!(go["player"]
        .get("autocast_skips_unready_cooldowns")
        .is_none());
}
