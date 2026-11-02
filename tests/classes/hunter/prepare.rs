//! Rust preparation of a Hunter against the pinned Go exporter.
//!
//! Each case is a Hunter fixture request without buffs, debuffs, consumables and professions, with
//! weapons that carry no item effect (see tools/hunter_prepare_goldens.py), so what it prepares is
//! the Hunter's own: its spells, talent auras, class effects, pet and stats. The goldens keep a
//! digest of every spell, aura, effect and pet Go exported, so a failure names the item that
//! changed. Regenerate them only from the Go exporter.

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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/classes/hunter/prepare")
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

/// FNV-1a over the bytes from one offset basis.
fn fnv1a(bytes: &[u8], basis: u64) -> u64 {
    bytes.iter().fold(basis, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The goldens' digest, tools/hunter_prepare_goldens.py `digest`: two FNV-1a streams, which only
/// have to notice a change, not resist one.
fn digest(value: &Value) -> String {
    let mut text = String::new();
    canonical(value, &mut text);
    format!(
        "{:016x}{:016x}",
        fnv1a(text.as_bytes(), 0xcbf2_9ce4_8422_2325),
        fnv1a(text.as_bytes(), 0x8422_2325_cbf2_9ce4)
    )
}

fn label(section: &str, item: &Value) -> String {
    match section {
        "player.auras" | "target.auras" => item["label"].as_str().unwrap_or("?").to_string(),
        "effects" => item["kind"].as_str().unwrap_or("?").to_string(),
        "pets" => item["label"].as_str().unwrap_or("?").to_string(),
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
        "encounter" => &prepared["encounter"],
        "melee" => &prepared["melee"],
        "pets" => &prepared["pets"],
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
                // A build without a pet has no pets section at all, where the golden has none listed.
                let actual = match actual {
                    Value::Null => &[][..],
                    other => other.as_array().expect("a list section"),
                };
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
/// from it on purpose, gives what the Go exporter wrote.
#[test]
fn every_hunter_case_prepares_as_go_does() {
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
    assert!(cases >= 13, "the goldens are missing: {cases}");
    assert!(
        failures.is_empty(),
        "{} of {cases} differ from the Go exporter:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// A Hunter without options has no ammo, quiver or pet to read, so it is refused rather than
/// prepared as some default.
#[test]
fn a_hunter_without_options_is_refused() {
    let request = fs::read(directory().join("beast-mastery-cat.request.json")).unwrap();
    let mut value: Value = serde_json::from_slice(&request).unwrap();
    value["raid"]["parties"][0]["players"][0]["hunter"] = serde_json::json!({});
    match forever_engine::prepare_json(&serde_json::to_vec(&value).unwrap(), "x") {
        Err(forever_engine::PrepareError::Refused(refusal)) => {
            assert_eq!(refusal.code, "request")
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// The pet is the hunter's second unit after the target, enabled by the reset with a full focus
/// bar, and a build without a pet type or without an uptime has none.
#[test]
fn a_pet_exists_only_with_a_type_and_an_uptime() {
    let request = fs::read(directory().join("beast-mastery-cat.request.json")).unwrap();
    let prepared = forever_engine::prepare_json(&request, "x").unwrap();
    let pets = prepared["pets"].as_array().unwrap();
    assert_eq!(pets.len(), 1);
    assert_eq!(pets[0]["label"], "hunter (#1) - Cat");
    assert_eq!(pets[0]["focus"]["max"], 100.0);

    for edit in [
        serde_json::json!({"petType": "PetNone"}),
        serde_json::json!({"petUptime": 0}),
    ] {
        let mut value: Value = serde_json::from_slice(&request).unwrap();
        let options =
            &mut value["raid"]["parties"][0]["players"][0]["hunter"]["options"]["classOptions"];
        for (key, change) in edit.as_object().unwrap() {
            options[key] = change.clone();
        }
        let prepared =
            forever_engine::prepare_json(&serde_json::to_vec(&value).unwrap(), "x").unwrap();
        assert!(prepared.get("pets").is_none(), "{edit}");
    }
}

/// Thunderstomp cleaves every active target. Go's exporter names the ability unrepresented
/// against two targets; under Forever's rules Rust strikes both, with a roll each, up to its
/// row's four targets.
#[test]
fn a_gorilla_against_two_targets_thunderstomps_both() {
    let request = fs::read(directory().join("beast-mastery-gorilla.request.json")).unwrap();
    let mut value: Value = serde_json::from_slice(&request).unwrap();
    let targets = value["encounter"]["targets"].as_array_mut().unwrap();
    targets.push(targets[0].clone());
    let bytes = serde_json::to_vec(&value).unwrap();
    match forever_engine::prepare_json_with(&bytes, "x", forever_engine::Rules::GoReference) {
        Err(forever_engine::PrepareError::Refused(refusal)) => {
            assert_eq!(refusal.code, "unrepresented");
            assert!(refusal.reason.contains("1264455"), "{}", refusal.reason);
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    let prepared = forever_engine::prepare_json(&bytes, "x").unwrap();
    let strike = prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["spell_id"] == 1264455)
        .expect("Thunderstomp");
    assert_eq!(strike["cleave_targets"], 4);
}

/// Forever's pet hit, from its developers' tracker (22 October 2026): "Hunters give melee hit
/// equal to their ranged hit, and their spell hit to their pet equal to 17/8ths of their ranged
/// hit." The pet's melee hit is already the owner's melee plus ranged hit (Go patch 178); the
/// spell hit is Rust only, so the reference's rules leave the pet none.
#[test]
fn a_pets_spell_hit_is_seventeen_eighths_of_its_masters_ranged_hit() {
    let request = fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/mage/prepared-v2/production-beast-mastery-hunter.request.json"),
    )
    .unwrap();
    let forever = forever_engine::prepare_json(&request, "x").unwrap();
    let go = forever_engine::prepare_json_with(&request, "x", forever_engine::Rules::GoReference)
        .unwrap();
    let owner = &forever["player"]["stats"];
    let melee = owner["PhysicalHitPercent"].as_f64().unwrap();
    let ranged = owner["RangedHitPercent"].as_f64().unwrap();
    assert!(melee + ranged > 0.0, "the production build carries hit");
    let stat = |prepared: &Value, name: &str| prepared["pets"][0]["stats"][name].as_f64().unwrap();
    assert_eq!(
        stat(&forever, "PhysicalHitPercent"),
        stat(&go, "PhysicalHitPercent")
    );
    assert_eq!(
        stat(&forever, "SpellHitPercent") - stat(&go, "SpellHitPercent"),
        melee * 2.125 + ranged * 2.125
    );
    let spell_terms = |prepared: &Value| -> Vec<(String, f64)> {
        prepared["pets"][0]["inheritance"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|term| term["pet"] == "SpellHitPercent")
            .map(|term| {
                let owner = term["owner"].as_str().unwrap().to_string();
                (owner, term["coefficient"].as_f64().unwrap())
            })
            .collect()
    };
    assert_eq!(
        spell_terms(&forever),
        [
            ("PhysicalHitPercent".to_string(), 2.125),
            ("RangedHitPercent".to_string(), 2.125)
        ]
    );
    assert!(spell_terms(&go).is_empty());
}

/// Forever's Engineering ammo, Swiftfeather Arrow (274387) and Swiftstrike Shot (274388), each
/// "Adds 24.5 damage per second" on its Wowhead Forever tooltip. The ammo's damage per second
/// times the bow's speed is added to each end of the ranged weapon's damage, under either rule
/// set, and the reference's ammo keeps its values.
#[test]
fn forevers_engineering_ammo_adds_twenty_four_and_a_half_damage_per_second() {
    let request = fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/mage/prepared-v2/production-marksmanship-hunter.request.json"),
    )
    .unwrap();
    let ranged = |ammo: &str, rules| {
        let mut value: Value = serde_json::from_slice(&request).unwrap();
        value["raid"]["parties"][0]["players"][0]["hunter"]["options"]["classOptions"]["ammo"] =
            serde_json::json!(ammo);
        let prepared =
            forever_engine::prepare_json_with(&serde_json::to_vec(&value).unwrap(), "x", rules)
                .unwrap();
        let weapon = &prepared["melee"]["ranged"];
        let damage = |end: &str| weapon[end].as_f64().unwrap();
        let speed = weapon["swing_speed"].as_f64().unwrap();
        (damage("base_damage_min"), damage("base_damage_max"), speed)
    };
    for rules in [
        forever_engine::Rules::Forever,
        forever_engine::Rules::GoReference,
    ] {
        let (min, max, speed) = ranged("AmmoNone", rules);
        for (ammo, dps) in [
            ("SwiftfeatherArrow", 24.5),
            ("SwiftstrikeShot", 24.5),
            ("ThoriumHeadedArrow", 17.5),
            ("MiniatureCannonBalls", 20.5),
        ] {
            let (ammo_min, ammo_max, _) = ranged(ammo, rules);
            assert_eq!(ammo_min - min, dps * speed, "{ammo}");
            assert_eq!(ammo_max - max, dps * speed, "{ammo}");
        }
    }
}
