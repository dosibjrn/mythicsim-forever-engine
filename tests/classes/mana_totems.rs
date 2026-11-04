//! The party's mana totems in the results (README, Rust-only mechanics): Mana Spring Totem's
//! party buff and Mana Tide Totem are MP5 auras, and each mana tick's share from them is booked
//! under the totem rather than as regeneration. Only the booking changes: the mana gained, every
//! row's sum and the damage are the same as with the regeneration rows alone.

use forever_engine::RngMode;
use forever_engine::{contracts::prepared_v2::PreparedV2, prepare_json};
use serde_json::{json, Value};
use std::{fs, path::Path};

/// Mana Spring Totem rank 4, which casts the party buff 10494.
const MANA_SPRING_TOTEM: i64 = 10497;
/// Mana Tide Totem rank 4, whose party row is 1325257.
const MANA_TIDE_TOTEM: i64 = 1325256;

/// An Affliction Warlock with the party's buffs set by `buffs`.
fn request(buffs: Value) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/classes/warlock/prepare/affliction-curse-of-doom.request.json");
    let mut request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    request["simOptions"]["iterations"] = json!(50);
    request["simOptions"]["debugFirstIteration"] = json!(false);
    request["raid"]["parties"][0]["buffs"] = buffs;
    // Autocasting other cooldowns calls the raid's Mana Tide Totem, as the presets do.
    let rotation = &mut request["raid"]["parties"][0]["players"][0]["rotation"]["priorityList"];
    rotation
        .as_array_mut()
        .unwrap()
        .insert(0, json!({ "action": { "autocastOtherCooldowns": {} } }));
    request
}

fn prepare(request: &Value) -> Value {
    prepare_json(&serde_json::to_vec(request).unwrap(), "mana-totems").unwrap()
}

fn simulate(prepared: &Value) -> Value {
    let typed: PreparedV2 = serde_json::from_value(prepared.clone()).unwrap();
    forever_engine::simulate_prepared(&typed, RngMode::Stochastic)
        .unwrap()
        .result
}

fn mana_rows(result: &Value) -> Vec<Value> {
    result["raidMetrics"]["parties"][0]["players"][0]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["type"] == "ResourceTypeMana")
        .cloned()
        .collect()
}

fn row(result: &Value, spell: i64) -> Option<Value> {
    mana_rows(result)
        .into_iter()
        .find(|row| row["id"] == json!({ "spellId": spell }))
}

fn sum(rows: &[Value], key: &str) -> f64 {
    // The report leaves a zero out.
    rows.iter()
        .map(|row| row[key].as_f64().unwrap_or(0.0))
        .sum()
}

#[test]
fn mana_spring_and_mana_tide_get_rows_and_the_mana_gained_is_unchanged() {
    let prepared = prepare(&request(json!({
        "manaSpringTotem": "TristateEffectRegular",
        "manaTideTotems": 1,
    })));
    let sources = prepared["player"]["mp5_sources"].as_array().unwrap();
    assert_eq!(sources.len(), 2, "{sources:?}");
    assert_eq!(
        sources[0]["action_id"],
        json!({ "spell_id": MANA_SPRING_TOTEM })
    );
    assert!(sources[0].get("aura").is_none());
    assert_eq!(
        sources[1]["action_id"],
        json!({ "spell_id": MANA_TIDE_TOTEM })
    );
    assert_eq!(sources[1]["aura"], "Mana Tide Totem (External)");
    // 290 mana every 3 sec as MP5.
    assert!((sources[1]["mp5"].as_f64().unwrap() - 290.0 * 5.0 / 3.0).abs() < 1e-9);

    let result = simulate(&prepared);
    let spring = row(&result, MANA_SPRING_TOTEM).expect("a Mana Spring Totem row");
    let tide = row(&result, MANA_TIDE_TOTEM).expect("a Mana Tide Totem row");
    // Every tick of a fight carries Mana Spring, as each 13 sec Mana Tide carries a few.
    assert!(spring["gain"].as_f64().unwrap() > 0.0);
    assert!(spring["events"].as_i64().unwrap() > tide["events"].as_i64().unwrap());
    assert!(tide["actualGain"].as_f64().unwrap() > 0.0);

    // The reference's booking: no sources, all of it regeneration.
    let mut reference = prepared.clone();
    reference["player"]
        .as_object_mut()
        .unwrap()
        .remove("mp5_sources");
    let before = simulate(&reference);
    assert!(row(&before, MANA_SPRING_TOTEM).is_none());
    let (rows, rows_before) = (mana_rows(&result), mana_rows(&before));
    for key in ["gain", "actualGain"] {
        let (now, then) = (sum(&rows, key), sum(&rows_before, key));
        assert!(
            (now - then).abs() < 1e-6 * then.abs(),
            "{key}: {now} against {then}"
        );
    }
    let dps = |result: &Value| result["raidMetrics"]["dps"]["avg"].as_f64().unwrap();
    assert_eq!(dps(&result), dps(&before));
}

#[test]
fn no_totems_write_no_sources() {
    let prepared = prepare(&request(json!({})));
    assert!(prepared["player"].get("mp5_sources").is_none());
}
