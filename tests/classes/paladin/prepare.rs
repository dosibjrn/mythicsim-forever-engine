//! Rust preparation of a Paladin against the pinned Go exporter.
//!
//! Each case is a Paladin fixture request without buffs, debuffs, consumables, professions and
//! effect gear (see tools/paladin_prepare_goldens.py), so what it prepares is the Paladin's own:
//! its spells, seals, talent auras, item effects, set bonuses, class effects and stats. The
//! goldens keep a hash of every spell, aura and effect Go exported, so a failure names the item
//! that changed. Regenerate them only from the Go exporter.

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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/classes/paladin/prepare")
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
    sha256::hex(text.as_bytes())[..16].to_string()
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
        "player.talents" => &prepared["player"]["talents"],
        "player.attack_table" => &prepared["player"]["attack_table"],
        "melee" => &prepared["melee"],
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

#[test]
fn every_paladin_case_prepares_as_go_does() {
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
        // The goldens are the Go exporter's: prepare with the reference rules (the 26 October
        // racials are Forever's own).
        match forever_engine::prepare_json_with(&request, case, forever_engine::Rules::GoReference)
        {
            Ok(prepared) => {
                let diffs = differences(&golden, &prepared);
                if !diffs.is_empty() {
                    failures.push(format!("{case}:\n  {}", diffs.join("\n  ")));
                }
            }
            Err(err) => failures.push(format!("{case}: {err}")),
        }
    }
    assert!(cases >= 5, "the goldens are missing: {cases}");
    assert!(
        failures.is_empty(),
        "{} of {cases} differ from the Go exporter:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn request_value(case: &str) -> Value {
    let request = fs::read(directory().join(format!("{case}.request.json"))).unwrap();
    serde_json::from_slice(&request).unwrap()
}

fn refusal_code(value: &Value) -> &'static str {
    match forever_engine::prepare_json(&serde_json::to_vec(value).unwrap(), "x") {
        Err(forever_engine::PrepareError::Refused(refusal)) => refusal.code,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// An unsupported Paladin option is refused with a stable code, never approximated.
#[test]
fn a_paladin_without_its_options_is_refused() {
    let mut value = request_value("retribution");
    value["raid"]["parties"][0]["players"][0]["retributionPaladin"] = serde_json::json!({});
    assert_eq!(refusal_code(&value), "class_option");
}

/// A set bonus Rust does not describe is refused: the Justice Battlegear 4-piece adds attack
/// power against Undead to the attack tables, which the exporter reports as unrepresented.
#[test]
fn the_justice_battlegear_four_piece_is_refused() {
    let mut value = request_value("retribution");
    // The talents read in push 113554's tree, which Forever's rules prepare.
    crate::paladin_113554::forever_request(&mut value);
    let items = value["raid"]["parties"][0]["players"][0]["equipment"]["items"]
        .as_array_mut()
        .unwrap();
    for (slot, id) in [(2, 273910), (6, 273909), (8, 273908), (9, 273911)] {
        items[slot] = serde_json::json!({ "id": id });
    }
    assert_eq!(refusal_code(&value), "item_set");
}

/// A Paladin tanking the target is prepared with the target's swing at it; the fixture
/// harness compares the tanking fixtures with Go.
#[test]
fn a_paladin_tanking_the_target_is_prepared_with_its_swing() {
    let mut value = request_value("protection");
    crate::paladin_113554::forever_request(&mut value);
    value["raid"]["tanks"] = serde_json::json!([{"type": "Player", "index": 0}]);
    value["encounter"]["targets"][0]["tankIndex"] = serde_json::json!(0);
    let prepared = forever_engine::prepare_json(&serde_json::to_vec(&value).unwrap(), "x")
        .expect("a tanking Paladin is prepared");
    assert!(prepared["enemy"].is_object());
}
