//! Rust preparation of a Rogue against the pinned Go exporter.
//!
//! Each case is a Rogue fixture request without buffs, debuffs, consumables but the poisons,
//! professions and effect gear (see tools/rogue_prepare_goldens.py), so what it prepares is the
//! Rogue's own: its poisons, spells, talent auras and mods, set bonuses, class effects and
//! stats. The goldens keep a hash of every spell, aura and effect Go exported, so a failure
//! names the item that changed. Regenerate them only from the Go exporter.

use crate::sha256;
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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/classes/rogue/prepare")
}

/// The text a digest is taken of: tools/rogue_prepare_goldens.py `canonical`.
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
        "player.energy" => &prepared["player"]["energy"],
        "player.talents" => &prepared["player"]["talents"],
        "player.attack_table" => &prepared["player"]["attack_table"],
        "melee" => &prepared["melee"],
        "unrepresented" => &prepared["unrepresented"],
        other => panic!("unknown golden section {other}"),
    }
}

/// An effect Rust preparation writes where the pinned exporter, frozen since Forever work moved
/// to Rust, writes none: Nightslayer Armor's eight piece heal on Vanish (`health_on_cast`), which
/// the runtime checks against Go's simulation instead.
fn beyond_the_exporter(effect: &Value) -> bool {
    effect["kind"] == "health_on_cast"
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
                let actual: Vec<&Value> = actual
                    .as_array()
                    .expect("a list section")
                    .iter()
                    .filter(|item| name != "effects" || !beyond_the_exporter(item))
                    .collect();
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
fn every_rogue_case_prepares_as_go_does() {
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
    assert!(cases >= 6, "the goldens are missing: {cases}");
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

/// Go reads the Rogue's options without a check, so a request without them is one Go cannot
/// prepare: Rust refuses it with a stable code.
#[test]
fn a_rogue_without_its_options_is_refused() {
    let mut value = request_value("assassination");
    value["raid"]["parties"][0]["players"][0]["rogue"] = serde_json::json!({});
    assert_eq!(refusal_code(&value), "class_option");
}

/// The subtlety request with `items` in its slots (proto `ItemSlot` order) and nothing in the
/// off hand unless `items` fills it, prepared.
fn prepared_with(items: &[(usize, i64)]) -> Value {
    let mut value = request_value("subtlety");
    let equipment = &mut value["raid"]["parties"][0]["players"][0]["equipment"]["items"];
    equipment[15] = serde_json::json!({});
    for (slot, id) in items {
        equipment[*slot] = serde_json::json!({ "id": id });
    }
    crate::rogue_113483::forever_request(&mut value);
    forever_engine::prepare_json(&serde_json::to_vec(&value).unwrap(), "weapon-skill")
        .unwrap_or_else(|err| panic!("{items:?} is not prepared: {err}"))
}

/// The skill each hand's table was resolved with, zero where the table is the base one.
fn hand_skills(prepared: &Value) -> [f64; 3] {
    ["main_hand_skill", "off_hand_skill", "ranged_skill"]
        .map(|hand| prepared["melee"][hand]["skill"].as_f64().unwrap_or(0.0))
}

const HANDS: usize = 6;
const MAIN_HAND: usize = 14;
const OFF_HAND: usize = 15;

/// Go `TestCharacterWeaponSkillPerHand`: each hand attacks with the skill of its own weapon
/// type, summed over the equipment (Go patch 135).
#[test]
fn each_hand_takes_the_skill_of_its_weapon_type() {
    // Redfall's test: Alcor's Sunrazor and Distracting Dagger with Aged Core Leather Gloves.
    let daggers = prepared_with(&[(MAIN_HAND, 14555), (OFF_HAND, 18392), (HANDS, 18823)]);
    assert_eq!(hand_skills(&daggers), [11.0, 11.0, 0.0]);
    // Against the level 63 boss, a 4 point gap: 5.4% miss and dodge, no hit suppression, and a
    // glance keeps 91% to 99%. Glancing chance stays on the base table.
    let main_hand = &daggers["melee"]["main_hand_skill"];
    for (field, value) in [
        ("miss_chance", 0.054),
        ("hit_suppression", 0.0),
        ("defender_dodge", 0.054),
        ("glance_multiplier", 0.95),
        ("glance_spread", 0.04),
    ] {
        let got = main_hand[field].as_f64().unwrap();
        assert!((got - value).abs() < 1e-9, "{field}: {got}, want {value}");
    }
    assert_eq!(daggers["melee"]["base_glance_chance"], 0.4);
    assert_eq!(daggers["melee"]["base_miss_chance"], 0.08);
    // Edgemaster's Handguards raise swords and daggers alike; Distracting Dagger only daggers.
    let sword = prepared_with(&[(MAIN_HAND, 18832), (OFF_HAND, 18392), (HANDS, 14551)]);
    assert_eq!(hand_skills(&sword), [7.0, 13.0, 0.0]);
    // Dagger skill does nothing for an axe, and the axe's table is the base one.
    let axe = prepared_with(&[(MAIN_HAND, 17068), (HANDS, 18823)]);
    assert_eq!(hand_skills(&axe), [0.0, 0.0, 0.0]);
    assert!(axe["melee"].get("main_hand_skill").is_none());
}
