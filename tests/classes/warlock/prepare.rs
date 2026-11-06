//! Rust preparation of a Warlock against the pinned Go exporter.
//!
//! Each case is a Warlock fixture request without buffs, debuffs, consumables, professions and
//! gear (see tools/warlock_prepare_goldens.py), so what it prepares is the Warlock's own: its
//! spells, talent auras, demons, class effects and stats. The goldens keep a digest of every spell, aura and effect Go
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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/classes/warlock/prepare")
}

/// The text a digest is taken of: tools/warlock_prepare_goldens.py `canonical`.
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
        "pets" => &prepared["pets"],
        "player.major_cooldowns" => &prepared["player"]["major_cooldowns"],
        "player.stats" => &prepared["player"]["stats"],
        "player.pseudo_stats" => &prepared["player"]["pseudo_stats"],
        "player.mana" => &prepared["player"]["mana"],
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
                // A prepared state without demons has no pets list.
                let actual = match actual {
                    Value::Null => &Vec::new(),
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
fn every_warlock_case_prepares_as_go_does() {
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
    assert!(cases >= 25, "the goldens are missing: {cases}");
    assert!(
        failures.is_empty(),
        "{} of {cases} differ from the Go exporter:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// An unsupported Warlock option is refused with a stable code, never approximated.
#[test]
fn a_warlock_without_class_options_is_refused() {
    let request = fs::read(directory().join("affliction-succubus.request.json")).unwrap();
    let mut value: Value = serde_json::from_slice(&request).unwrap();
    value["raid"]["parties"][0]["players"][0]["warlock"] = serde_json::json!({});
    match forever_engine::prepare_json(&serde_json::to_vec(&value).unwrap(), "x") {
        Err(forever_engine::PrepareError::Refused(refusal)) => {
            assert_eq!(refusal.code, "class_option")
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// Forever's demon hit, from its developers' tracker (22 October 2026): "Warlocks will give spell
/// hit equal to their spell hit, and melee hit equal to 8/17ths of their spell hit." Rust only:
/// the reference gives a demon its master's whole spell hit as melee hit.
#[test]
fn a_demons_melee_hit_is_eight_seventeenths_of_its_masters_spell_hit() {
    let request = fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/mage/prepared-v2/production-demonology-warlock.request.json"),
    )
    .unwrap();
    let forever = forever_engine::prepare_json(&request, "x").unwrap();
    let go = forever_engine::prepare_json_with(&request, "x", forever_engine::Rules::GoReference)
        .unwrap();
    let spell_hit = forever["player"]["stats"]["SpellHitPercent"]
        .as_f64()
        .unwrap();
    assert!(spell_hit > 0.0, "the production build carries spell hit");
    let stat = |prepared: &Value, name: &str| prepared["pets"][0]["stats"][name].as_f64().unwrap();
    assert_eq!(
        forever["pets"][0]["label"],
        "demonology-warlock (#1) - Succubus"
    );
    assert_eq!(
        stat(&forever, "SpellHitPercent"),
        stat(&go, "SpellHitPercent")
    );
    assert_eq!(stat(&go, "PhysicalHitPercent"), spell_hit);
    assert_eq!(
        stat(&forever, "PhysicalHitPercent"),
        spell_hit * (8.0 / 17.0)
    );
    let melee_term = |prepared: &Value| {
        prepared["pets"][0]["inheritance"]
            .as_array()
            .unwrap()
            .iter()
            .find(|term| term["pet"] == "PhysicalHitPercent")
            .map(|term| term["coefficient"].as_f64().unwrap())
    };
    assert_eq!(melee_term(&forever), Some(8.0 / 17.0));
    assert_eq!(melee_term(&go), Some(1.0));
}

/// A wand-equipped Warlock must use the same OtherActionShoot native ranged
/// auto-attack as the shared engine. An empty ranged slot cannot create a
/// phantom Shoot, and no Python/game-rule mirror is involved.
#[test]
fn warlock_wand_registers_native_shoot_for_apl_and_combat() {
    use serde_json::json;
    let request = fs::read(directory().join("affliction-imp.request.json")).unwrap();
    let mut source: Value = serde_json::from_slice(&request).unwrap();
    let player = &mut source["raid"]["parties"][0]["players"][0];
    // Equipment follows proto.ItemSlot (ranged is index 16).
    let mut gear = vec![json!({}); 17];
    gear[16] = json!({"id": 5211}); // Dusk Wand, source item.
    player["equipment"]["items"] = json!(gear);
    player["rotation"]["priorityList"] = json!([
        {"action": {"castSpell": {"spellId": {"otherId": "OtherActionShoot"}}}}
    ]);

    let prepared = forever_engine::prepare_json(
        &serde_json::to_vec(&source).unwrap(),
        "warlock_wand_shoot",
    ).unwrap();
    assert_eq!(prepared["melee"]["auto_swing_ranged"], true);
    assert_eq!(prepared["melee"]["auto_swing_melee"], false);
    assert!(prepared["melee"]["ranged"]["swing_speed"].as_f64().unwrap() > 0.0);
    assert!(prepared["player"]["spells"].as_array().unwrap().iter().any(|spell| {
        spell["action_id"]["other_id"] == "OtherActionShoot"
    }), "Shoot must be a native prepared action");

    // Identical Warlock and APL but without a wand: no fabricated attack.
    source["raid"]["parties"][0]["players"][0]["equipment"]["items"][16] = json!({});
    let empty = forever_engine::prepare_json(
        &serde_json::to_vec(&source).unwrap(),
        "warlock_no_wand",
    ).unwrap();
    assert_eq!(empty["melee"]["auto_swing_ranged"], false);
    assert!(!empty["player"]["spells"].as_array().unwrap().iter().any(|spell| {
        spell["action_id"]["other_id"] == "OtherActionShoot"
    }));
}
