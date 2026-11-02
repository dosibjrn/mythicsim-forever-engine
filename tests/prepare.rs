//! Rust preparation against the accepted prepared v2 fixtures: every fixture's request is
//! read from the application's protojson and checked against what the Go exporter wrote.

use forever_engine::contracts::request::Request;
use serde::Deserialize;
use serde_json::Value;
use std::{fs, path::Path};

#[path = "support/paladin_113554.rs"]
mod paladin_113554;

#[derive(Deserialize)]
struct Manifest {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    request: RequestSource,
    prepared: String,
}

#[derive(Deserialize)]
struct RequestSource {
    path: String,
    #[serde(default)]
    pointer: String,
}

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn manifest() -> Manifest {
    let path = root().join("fixtures/mage/prepared-v2/manifest.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// The request bytes a case names, the whole file or the value at its JSON pointer.
fn request_bytes(case: &Case) -> Vec<u8> {
    let bytes = fs::read(root().join(&case.request.path)).unwrap();
    if case.request.pointer.is_empty() {
        return bytes;
    }
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    serde_json::to_vec(value.pointer(&case.request.pointer).unwrap()).unwrap()
}

fn prepared(case: &Case) -> Value {
    let path = root()
        .join("fixtures/mage/prepared-v2")
        .join(&case.prepared);
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn every_fixture_request_digest_matches_go() {
    let mut failures = Vec::new();
    let cases = manifest().cases;
    for case in &cases {
        let request = match Request::from_json(&request_bytes(case)) {
            Ok(request) => request,
            Err(err) => {
                failures.push(format!("{}: {err}", case.id));
                continue;
            }
        };
        let expected = prepared(case)["request_sha256"]
            .as_str()
            .unwrap()
            .to_string();
        if request.sha256() != expected {
            failures.push(format!(
                "{}: digest {} != {expected}",
                case.id,
                request.sha256()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {}:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

/// The paths at which two JSON values differ, at most `limit` of them.
fn differences(go: &Value, rust: &Value, path: &str, out: &mut Vec<String>, limit: usize) {
    if out.len() >= limit {
        return;
    }
    match (go, rust) {
        (Value::Object(a), Value::Object(b)) => {
            for (key, value) in a {
                match b.get(key) {
                    Some(other) => differences(value, other, &format!("{path}/{key}"), out, limit),
                    None => out.push(format!("{path}/{key}: missing in Rust")),
                }
            }
            for key in b.keys() {
                if !a.contains_key(key) && out.len() < limit {
                    out.push(format!("{path}/{key}: only in Rust"));
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                out.push(format!(
                    "{path}: {} items in Go, {} in Rust",
                    a.len(),
                    b.len()
                ));
            }
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                differences(x, y, &format!("{path}/{i}"), out, limit);
            }
        }
        (Value::Number(a), Value::Number(b)) if a.as_f64() == b.as_f64() => {}
        (a, b) if a == b => {}
        (a, b) => out.push(format!("{path}: Go {a}, Rust {b}")),
    }
}

/// Every fixture Rust prepares with the reference's rules must give the prepared state the Go
/// exporter wrote; the rest must be refused, never prepared wrongly. Set PREPARE_REPORT to
/// print every case.
#[test]
fn rust_preparation_matches_go_or_refuses() {
    let report = std::env::var("PREPARE_REPORT").is_ok();
    let mut matched = 0;
    let mut refused = std::collections::BTreeMap::<String, usize>::new();
    let mut mismatched = Vec::new();
    let cases = manifest().cases;
    for case in &cases {
        let rules = forever_engine::Rules::GoReference;
        match forever_engine::prepare_json_with(&request_bytes(case), &case.id, rules) {
            Ok(rust) => {
                let go = prepared(case);
                let mut diffs = Vec::new();
                differences(&go, &rust, "", &mut diffs, 12);
                if diffs.is_empty() {
                    matched += 1;
                    if report {
                        println!("match   {}", case.id);
                    }
                } else {
                    mismatched.push(format!("{}:\n  {}", case.id, diffs.join("\n  ")));
                }
            }
            Err(forever_engine::PrepareError::Refused(refusal)) => {
                *refused.entry(refusal.code.to_string()).or_default() += 1;
                if report {
                    println!("refuse  {}: {refusal}", case.id);
                }
            }
            Err(err) => mismatched.push(format!("{}: {err}", case.id)),
        }
    }
    println!(
        "prepared in Rust and equal to Go: {matched} of {}",
        cases.len()
    );
    println!("refused: {refused:?}");
    assert!(
        mismatched.is_empty(),
        "{} prepared differently from Go:\n{}",
        mismatched.len(),
        mismatched.join("\n")
    );
}

/// The fields the UI sends with every request whatever the user chose: five active parties,
/// tank slots that no player fills, an empty healing model, zero bonus pseudo stats and the
/// `TypeAuto` of a preset the user has not edited. Go simulates the request as it does without
/// them, so preparation gives the same state but for the request digest and the rotation's
/// recorded type. The cases cover pets, a player who tanks, a bear and the other classes.
#[test]
fn the_ui_default_fields_prepare_as_without_them() {
    let ids = [
        "frost-reference",
        "warrior-heroic-strike",
        "demonology-warlock-voidwalker-pact",
        "beast-mastery-hunter-pet-uptime-half",
        "tank-ret-autos",
        "protection-paladin-no-dynamite",
        "feral-druid-faerie-fire-armor",
        "feral-bear-druid-defensives",
    ];
    let cases = manifest().cases;
    for id in ids {
        let case = cases
            .iter()
            .find(|case| case.id == id)
            .expect("a fixture case");
        let mut request: Value = serde_json::from_slice(&request_bytes(case)).unwrap();
        // A paladin's talents read in push 113554's tree, which Forever's rules prepare.
        paladin_113554::forever_request(&mut request);
        if id == "frost-reference" {
            // The fixture's talents in client 1.60.1.70707's order, which Forever's rules read:
            // Arcane Focus's points on Improved Channeling, which takes its square.
            request["raid"]["parties"][0]["players"][0]["talentsString"] =
                "050205103-3-053500000101301251".into();
        }
        // The Druid fixtures' talents in hotfix push 113531's order, which Forever's rules read.
        let druid_talents = match id {
            "feral-druid-faerie-fire-armor" => Some("050022-55000032023012125001-055"),
            "feral-bear-druid-defensives" => Some("-5500033212310212551-055"),
            _ => None,
        };
        if let Some(talents) = druid_talents {
            request["raid"]["parties"][0]["players"][0]["talentsString"] = talents.into();
        }
        let base = serde_json::to_vec(&request).unwrap();
        request["raid"]["numActiveParties"] = 5.into();
        let tanks = request["raid"]["tanks"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let slots = [2, 9].map(|index| serde_json::json!({"type": "Player", "index": index}));
        request["raid"]["tanks"] = tanks.into_iter().chain(slots).collect();
        let player = &mut request["raid"]["parties"][0]["players"][0];
        player["healingModel"] = serde_json::json!({});
        player["bonusStats"]["pseudoStats"] = vec![0.0; 28].into();
        if player["rotation"]["type"] == "TypeAPL" {
            player["rotation"]["type"] = "TypeAuto".into();
        }
        let without =
            forever_engine::prepare_json(&base, id).unwrap_or_else(|err| panic!("{id}: {err}"));
        let mut with = forever_engine::prepare_json(&serde_json::to_vec(&request).unwrap(), id)
            .unwrap_or_else(|err| panic!("{id} with the UI's fields: {err}"));
        with["request_sha256"] = without["request_sha256"].clone();
        with["player"]["rotation"]["type"] = without["player"]["rotation"]["type"].clone();
        let mut diffs = Vec::new();
        differences(&without, &with, "", &mut diffs, 12);
        assert!(
            diffs.is_empty(),
            "{id}:
  {}",
            diffs.join(
                "
  "
            )
        );
    }
}

/// Go imbue_swap.go (fork patch 133): `mh_imbue_swap_id` names the stone or oil on the item
/// swap's main hand weapon, and counts only with an item swap that has a main hand weapon of its
/// own. Rust refuses item swapping, so the field changes nothing it prepares: an Enhancement
/// Shaman with an Elemental Sharpening Stone prepares the same with Brilliant Wizard Oil named
/// for a swap weapon it does not have, and an item swap of a slot other than the main hand is
/// refused.
#[test]
fn the_swap_weapons_stone_needs_an_item_swap() {
    let id = "production-enhancement-shaman";
    let cases = manifest().cases;
    let case = cases.iter().find(|case| case.id == id).unwrap();
    let mut request: Value = serde_json::from_slice(&request_bytes(case)).unwrap();
    request["raid"]["parties"][0]["players"][0]["consumables"]["mhImbueId"] = 18262.into();
    let stone = forever_engine::prepare_json(&serde_json::to_vec(&request).unwrap(), id)
        .unwrap_or_else(|err| panic!("{id} with a stone: {err}"));
    request["raid"]["parties"][0]["players"][0]["consumables"]["mhImbueSwapId"] = 25122.into();
    let mut swap = forever_engine::prepare_json(&serde_json::to_vec(&request).unwrap(), id)
        .unwrap_or_else(|err| panic!("{id} with a swap weapon's oil: {err}"));
    swap["request_sha256"] = stone["request_sha256"].clone();
    let mut diffs = Vec::new();
    differences(&stone, &swap, "", &mut diffs, 12);
    assert!(diffs.is_empty(), "{diffs:?}");

    let player = &mut request["raid"]["parties"][0]["players"][0];
    player["enableItemSwap"] = true.into();
    player["itemSwap"] = serde_json::json!({"items": [{"id": 17182}]});
    let refused = forever_engine::prepare_json(&serde_json::to_vec(&request).unwrap(), id)
        .expect_err("a head slot item swap is refused");
    assert!(
        refused
            .to_string()
            .contains("an item swap of a slot other than the main hand alone is unsupported"),
        "{refused}"
    );
}

/// Go pet_buffs.go (fork patch 131, `TestImpGetsManaSpringTotemOnly`): of a raid's mana buffs, a
/// summoned Imp gets only the party's Mana Spring Totem, whose aura the client lets reach a pet:
/// 10 mana every 2 seconds, kept while it casts. Arcane Brilliance, Prayer of Spirit, Gift of the
/// Wild and the Greater Blessings land on players only.
#[test]
fn a_summoned_imp_gets_mana_spring_totem_only() {
    let id = "destruction-warlock-broad";
    let cases = manifest().cases;
    let case = cases.iter().find(|case| case.id == id).unwrap();
    let request: Value = serde_json::from_slice(&request_bytes(case)).unwrap();
    assert_eq!(
        request["raid"]["parties"][0]["buffs"]["manaSpringTotem"],
        "TristateEffectRegular"
    );
    let imp = |request: &Value| -> Value {
        let prepared = forever_engine::prepare_json(&serde_json::to_vec(request).unwrap(), id)
            .unwrap_or_else(|err| panic!("{id}: {err}"));
        prepared["pets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|pet| pet["name"] == "Imp")
            .unwrap()
            .clone()
    };
    let with = imp(&request);
    let mut without_totem = request.clone();
    without_totem["raid"]["parties"][0]["buffs"]
        .as_object_mut()
        .unwrap()
        .remove("manaSpringTotem");
    let without = imp(&without_totem);
    assert_eq!(with["stats"]["MP5"], 25.0);
    assert_eq!(without["stats"]["MP5"], 0.0);
    for stat in ["Intellect", "Spirit", "Stamina", "Mana"] {
        assert_eq!(with["stats"][stat], without["stats"][stat], "{stat}");
    }
    assert_eq!(with["mana"]["regen_per_second_casting"], 5.0);
    assert_eq!(without["mana"]["regen_per_second_casting"], 0.0);
    let labels = |pet: &Value| -> Vec<String> {
        pet["auras"]
            .as_array()
            .unwrap()
            .iter()
            .map(|aura| aura["label"].as_str().unwrap().to_string())
            .collect()
    };
    assert!(labels(&with).contains(&"Mana Spring Totem (External)".to_string()));
    for buff in [
        "Arcane Brilliance",
        "Prayer of Spirit",
        "Gift of the Wild",
        "Greater Blessing",
    ] {
        assert!(
            !labels(&with).iter().any(|label| label.starts_with(buff)),
            "{buff}: {:?}",
            labels(&with)
        );
    }
}

/// Go buffs/meta.go `newDamageShield`: the party's Retribution Aura and Thorns answer the
/// physical hits the player takes, and nothing physical hits a player the target does not swing
/// at, so both are inert listeners there and the gate supports them (production fallbacks to Go,
/// 24 and 25 October 2026).
#[test]
fn party_damage_shields_are_inert_without_a_tank() {
    let id = "production-subtlety-rogue";
    let cases = manifest().cases;
    let case = cases.iter().find(|case| case.id == id).unwrap();
    let mut request: Value = serde_json::from_slice(&request_bytes(case)).unwrap();
    request["raid"]["parties"][0]["buffs"]["retributionAura"] = true.into();
    request["raid"]["buffs"]["thorns"] = true.into();
    let prepared = forever_engine::prepare_json(&serde_json::to_vec(&request).unwrap(), id)
        .unwrap_or_else(|err| panic!("{id}: {err}"));
    for label in ["Retribution Aura (External)", "Thorns (External)"] {
        assert!(
            prepared["effects"]
                .as_array()
                .unwrap()
                .iter()
                .any(|effect| {
                    effect["kind"] == "inert_listener"
                        && effect["unit"] == "player"
                        && effect["aura"] == label
                }),
            "{label}"
        );
    }
    let prepared: forever_engine::contracts::prepared_v2::PreparedV2 =
        serde_json::from_value(prepared).unwrap();
    assert!(forever_engine::prepared_refusals(&prepared).is_empty());
}
