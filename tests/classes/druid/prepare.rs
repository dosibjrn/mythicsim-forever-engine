//! Rust preparation of a Druid against the pinned Go exporter.
//!
//! Each case is a Druid fixture request without buffs, debuffs, consumables, professions and tanks
//! (see tools/druid_prepare_goldens.py), so what it prepares is the Druid's own: its spells, forms,
//! talent auras, class effects, energy and stats. The goldens keep a digest of every spell, aura and effect Go
//! exported, so a failure names the item that changed. Regenerate them only from the Go exporter.

use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::PathBuf};

#[derive(Deserialize)]
struct Golden {
    case: String,
    request_sha256: String,
    sections: BTreeMap<String, Value>,
}

fn directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/classes/druid/prepare")
}

/// The text a digest is taken of: tools/mage_prepare_goldens.py `canonical`.
fn canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push('z'),
        Value::Bool(true) => out.push('t'),
        Value::Bool(false) => out.push('f'),
        Value::Number(number) => {
            let bits = number.as_f64().expect("a finite number").to_bits();
            out.push('n');
            out.push_str(&format!("{bits:016x}"));
        }
        Value::String(text) => out.push_str(&format!("s{}:{text}", text.len())),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(&Value::String(key.clone()), out);
                out.push(':');
                canonical(&map[key], out);
            }
            out.push('}');
        }
    }
}

/// The engine's own SHA-256, which it keeps private to avoid a dependency.
use crate::sha256;

fn digest(value: &Value) -> String {
    let mut text = String::new();
    canonical(value, &mut text);
    sha256::hex(text.as_bytes())
}

fn label(section: &str, item: &Value) -> String {
    match section {
        "player.auras" | "target.auras" => item["label"].as_str().unwrap_or("?").to_string(),
        "effects" => item["kind"].as_str().unwrap_or("?").to_string(),
        _ => serde_json::to_string(&item["action_id"]).unwrap(),
    }
}

fn section<'a>(prepared: &'a Value, name: &str) -> &'a Value {
    match name {
        "player.spells" => &prepared["player"]["spells"],
        "player.auras" => &prepared["player"]["auras"],
        "target.auras" => &prepared["target"]["auras"],
        "effects" => &prepared["effects"],
        "player.major_cooldowns" => &prepared["player"]["major_cooldowns"],
        "player.stats" => &prepared["player"]["stats"],
        "player.pseudo_stats" => &prepared["player"]["pseudo_stats"],
        "player.mana" => &prepared["player"]["mana"],
        "player.energy" => &prepared["player"]["energy"],
        "player.health_at_reset" => &prepared["player"]["health_at_reset"],
        "melee" => &prepared["melee"],
        "player.talents" => &prepared["player"]["talents"],
        "encounter" => &prepared["encounter"],
        "unrepresented" => &prepared["unrepresented"],
        other => panic!("unknown golden section {other}"),
    }
}

/// What differs from the golden: one line per part.
fn differences(golden: &Golden, prepared: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if prepared["request_sha256"].as_str() != Some(golden.request_sha256.as_str()) {
        out.push("request digest".to_string());
    }
    for (name, expected) in &golden.sections {
        let actual = section(prepared, name);
        match expected {
            Value::String(want) => {
                if &digest(actual) != want {
                    out.push(format!("{name}: differs ({actual})"));
                }
            }
            Value::Array(items) => {
                let actual = actual.as_array().expect("a list section");
                if actual.len() != items.len() {
                    out.push(format!(
                        "{name}: {} items in Go, {} in Rust",
                        items.len(),
                        actual.len()
                    ));
                }
                for (want, got) in items.iter().zip(actual) {
                    if digest(got) != want[1].as_str().unwrap() {
                        out.push(format!(
                            "{name}: {} differs, Go labels it {}",
                            label(name, got),
                            want[0]
                        ));
                    }
                }
            }
            other => panic!("golden section {name} is {other}"),
        }
    }
    out
}

/// Every case prepared with the reference's rules (`Rules::GoReference`), where Rust departs
/// from it on purpose (hotfix push 113531's tree and rows), gives what the Go exporter wrote.
#[test]
fn every_druid_case_prepares_as_go_does() {
    let mut cases = 0;
    let mut failures = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(directory()).unwrap().flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(case) = name.strip_suffix(".golden.json") else {
            continue;
        };
        cases += 1;
        let golden: Golden = serde_json::from_slice(&fs::read(entry.path()).unwrap()).unwrap();
        assert_eq!(golden.case, case);
        let request = fs::read(directory().join(format!("{case}.request.json"))).unwrap();
        let rules = forever_engine::Rules::GoReference;
        match forever_engine::prepare_json_with(&request, case, rules) {
            Ok(prepared) => {
                let diffs = differences(&golden, &prepared);
                if !diffs.is_empty() {
                    failures.push(format!("{case}:\n  {}", diffs.join("\n  ")));
                }
            }
            Err(err) => failures.push(format!("{case}: {err}")),
        }
    }
    assert!(cases >= 24, "the goldens are missing: {cases}");
    assert!(
        failures.is_empty(),
        "{} of {cases} differ from the Go exporter:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// A Restoration druid prepares in caster form with its heals, which Rust alone sims (the
/// healer sims in `tests/healers.rs`).
#[test]
fn a_restoration_druid_is_prepared_with_its_heals() {
    let request = fs::read(directory().join("cat.request.json")).unwrap();
    let mut value: Value = serde_json::from_slice(&request).unwrap();
    let player = &mut value["raid"]["parties"][0]["players"][0];
    player.as_object_mut().unwrap().remove("feralCatDruid");
    player["restorationDruid"] = serde_json::json!({"options": {"classOptions": {}}});
    // The fixture's talents in hotfix push 113531's order, which Forever's rules read.
    player["talentsString"] = serde_json::json!("050022-55000032023012125001-055");
    let prepared = forever_engine::prepare_json(&serde_json::to_vec(&value).unwrap(), "x")
        .expect("a Restoration druid is prepared");
    let heals = prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == "heals")
        .expect("the heals are exported");
    // Every rank of Healing Touch, Rejuvenation, Regrowth and Tranquility.
    assert_eq!(heals["heals"].as_array().unwrap().len(), 11 + 11 + 9 + 4);
}

/// A bear tanking the target is prepared with the target's swing at it; the fixture harness
/// compares the tanking fixtures with Go.
#[test]
fn a_tanking_bear_is_prepared_with_the_target_swing() {
    let request = fs::read(directory().join("bear.request.json")).unwrap();
    let mut value: Value = serde_json::from_slice(&request).unwrap();
    value["raid"]["tanks"] = serde_json::json!([{"type": "Player", "index": 0}]);
    // The fixture's talents in hotfix push 113531's order, which Forever's rules read.
    value["raid"]["parties"][0]["players"][0]["talentsString"] =
        serde_json::json!("-5500033212310212551-055");
    value["encounter"]["targets"][0]["tankIndex"] = serde_json::json!(0);
    let prepared = forever_engine::prepare_json(&serde_json::to_vec(&value).unwrap(), "x")
        .expect("a tanking bear is prepared");
    assert!(prepared["enemy"].is_object());
}

#[test]
fn forever_roots_preparation_uses_native_spell_data_and_improved_rank() {
    let original: Value = serde_json::from_slice(
        &fs::read(directory().join("balance-every-talent-at-max.request.json")).unwrap()
    ).unwrap();
    for (rank, multiplier) in [(0, 1.0), (1, 1.25), (2, 1.5), (3, 1.75)] {
        let mut request = original.clone();
        let player = &mut request["raid"]["parties"][0]["players"][0];
        // The all-talents-max fixture is intentionally illegal under Forever:
        // Predatory Discipline has one rank, not the three in that Go fixture.
        // Exercise the real legal Forever talent layout with only Roots selected.
        player["talentsString"] = Value::String(format!("000000{rank}"));
        let prepared = forever_engine::prepare_json(
            &serde_json::to_vec(&request).unwrap(),
            "native-roots-rank",
        ).expect("Forever Druid prepares");
        let roots = prepared["player"]["spells"].as_array().unwrap().iter()
            .find(|spell| spell["action_id"]["spell_id"] == 9853)
            .expect("source-native Rank 6 Entangling Roots");
        assert_eq!(roots["class_spell"], "entangling_roots");
        let dot = &roots["dot"];
        assert_eq!(dot["base_tick_count"], 9);
        assert_eq!(dot["base_tick_length_ns"], 3_000_000_000_i64);
        assert_eq!(dot["periodic_damage_multiplier"].as_f64(), Some(multiplier));
    }
}

#[test]
fn forever_roots_native_periodic_ticks_change_with_rank() {
    // Production Rust executor, not a GameIR/source-catalogue assertion.
    // Same encounter, seed and rotation; only the legally allocated rank changes.
    fn run(rank: i32) -> (f64, String) {
        let mut request: Value = serde_json::from_slice(
            &fs::read(directory().join("balance-every-talent-at-max.request.json")).unwrap()
        ).unwrap();
        let player = &mut request["raid"]["parties"][0]["players"][0];
        player["talentsString"] = Value::String(format!("000000{rank}"));
        player["rotation"]["priorityList"] = serde_json::json!([
            {"action": {
                "castSpell": {"spellId": {"spellId": 9853}},
                "condition": {"not": {"val": {
                    "dotIsActive": {"spellId": {"spellId": 9853}}
                }}}
            }},
            {"action": {"castSpell": {"spellId": {"spellId": 9912}}}}
        ]);
        request["simOptions"]["iterations"] = serde_json::json!(32);
        request["encounter"]["duration"] = serde_json::json!(60.0);
        request["encounter"]["durationVariation"] = serde_json::json!(0.0);
        let prepared = forever_engine::prepare_json(
            &serde_json::to_vec(&request).unwrap(), "native-roots-periodic-control"
        ).expect("Forever source-native request prepares");
        let prepared: forever_engine::contracts::prepared_v2::PreparedV2 =
            serde_json::from_value(prepared).expect("native prepared contract");
        let report = forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic)
            .expect("selected genuine Rust combat executes Roots");
        let dps = report.result["raidMetrics"]["dps"]["avg"].as_f64()
            .expect("native real combat DPS");
        let first_log = report.result["logs"].as_str().unwrap_or("").to_owned();
        (dps, first_log)
    }

    let (base, base_log) = run(0);
    let (improved, improved_log) = run(3);
    assert!(base_log.contains("Casting {SpellID: 9853}"),
        "native Roots must actually cast, not merely be prepared: {base_log}");
    assert!(improved_log.contains("Casting {SpellID: 9853}"),
        "rank-3 build must actually cast Roots: {improved_log}");
    assert!(base.is_finite() && base > 0.0 && improved.is_finite(),
        "native combat must produce finite positive damage: {base}, {improved}");
    assert!(improved > base,
        "same-seed native combat with +75% Roots tick damage should improve DPS: {base} -> {improved}");
}
