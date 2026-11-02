//! The raid's Expose Armor under Forever's rules (`external_expose_armor`): it lands at full 3 s
//! into the fight, so it leaves the same armor-seconds unapplied over the opening as the raid's
//! Sunder Armor ramp (a stack every default GCD from the pull), and a player picking either one
//! gets the same damage (Bae and Vrelk, Discord, 28 October 2026).

use forever_engine::contracts::prepared_v2::PreparedV2;
use serde_json::{json, Value};
use std::{fs, path::Path};

const ITERATIONS: u64 = 3000;

/// The Marksmanship Hunter production request, a physical spec, with the raid's armor debuffs set.
fn request(sunder: bool, expose: bool) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/production-marksmanship-hunter.request.json");
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let debuffs = value["raid"]["debuffs"].as_object_mut().unwrap();
    debuffs.remove("sunderArmor");
    debuffs.remove("exposeArmor");
    if sunder {
        debuffs.insert("sunderArmor".into(), json!(true));
    }
    if expose {
        debuffs.insert("exposeArmor".into(), json!(true));
    }
    serde_json::to_vec(&value).unwrap()
}

fn effect_kinds(prepared: &Value) -> Vec<String> {
    prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["kind"].as_str().map(str::to_owned))
        .collect()
}

fn dps(mut prepared: Value) -> f64 {
    prepared["sim"]["iterations"] = json!(ITERATIONS);
    let prepared: PreparedV2 = serde_json::from_value(prepared).unwrap();
    let report = forever_engine::simulate_prepared(&prepared).unwrap();
    report.result["raidMetrics"]["parties"][0]["players"][0]["dps"]["avg"]
        .as_f64()
        .unwrap()
}

#[test]
fn only_forever_preparation_delays_the_raids_expose_armor() {
    let alone = request(false, true);
    let forever = forever_engine::prepare_json(&alone, "x").unwrap();
    let go =
        forever_engine::prepare_json_with(&alone, "x", forever_engine::Rules::GoReference).unwrap();
    let effect = forever["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "external_expose_armor")
        .expect("the delayed Expose Armor");
    assert_eq!(effect["delay_ns"], json!(3_000_000_000i64));
    // The target opens 2,250 armor above its prepared armor, Expose Armor's whole value.
    let before = effect["armor_before"].as_f64().unwrap();
    let prepared_armor = forever["target"]["stats"]["Armor"].as_f64().unwrap();
    assert!(
        (before - prepared_armor - 2250.0).abs() < 1e-6,
        "{before} against {prepared_armor}"
    );
    assert!(!effect_kinds(&go).contains(&"external_expose_armor".to_owned()));
}

#[test]
fn beside_the_raids_sunder_armor_expose_armor_stays_up_from_the_pull() {
    let both = forever_engine::prepare_json(&request(true, true), "x").unwrap();
    assert!(!effect_kinds(&both).contains(&"external_expose_armor".to_owned()));
}

#[test]
fn expose_armor_and_sunder_armor_give_the_same_damage() {
    let sunder = dps(forever_engine::prepare_json(&request(true, false), "x").unwrap());
    let prepared = forever_engine::prepare_json(&request(false, true), "x").unwrap();
    let expose = dps(prepared.clone());
    // The same input without the delay: Expose Armor up from the pull, as before.
    let mut undelayed = prepared;
    undelayed["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|e| e["kind"] != "external_expose_armor");
    let from_the_pull = dps(undelayed);
    println!("sunder {sunder:.2}, expose {expose:.2}, expose from the pull {from_the_pull:.2}");
    assert!(
        (expose / sunder - 1.0).abs() < 0.002,
        "expose {expose} against sunder {sunder}"
    );
    assert!(from_the_pull > expose, "{from_the_pull} against {expose}");
}
