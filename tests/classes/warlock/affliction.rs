//! The production Affliction Warlock build: coverage regressions around Nightfall and its
//! summoned Succubus. Its Go result and first-fight log goldens are compared with every other
//! supported case in tests/classes/mage/prepared_v2.rs.

use forever_engine::{
    check_prepared, contracts::prepared_v2::PreparedV2, simulate_prepared, PreparedError,
};
use serde_json::{json, Value};
use std::{fs, path::Path};

fn production() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/production-affliction-warlock.prepared.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn reasons(value: Value) -> Vec<String> {
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    match check_prepared(&prepared) {
        Err(PreparedError::Unsupported(reasons)) => reasons,
        other => panic!("expected unsupported, got {other:?}"),
    }
}

fn remove_effect(value: &mut Value, kind: &str) {
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != kind);
}

#[test]
fn production_build_is_supported() {
    let prepared: PreparedV2 = serde_json::from_value(production()).unwrap();
    assert_eq!(check_prepared(&prepared), Ok(()));
    let kinds: Vec<&str> = prepared
        .effects
        .iter()
        .map(|effect| effect.kind())
        .collect();
    for kind in [
        "corruption",
        "bane_of_agony",
        "amplify_curse",
        "nightfall",
        "warlock_pet",
        "lash_of_pain",
    ] {
        assert!(kinds.contains(&kind), "missing {kind}");
    }
    assert_eq!(prepared.pets.len(), 1);
}

/// Nightfall's trigger listens to periodic damage; without its effect the listener is
/// unclaimed and the build is refused.
#[test]
fn nightfall_trigger_needs_its_effect() {
    let mut value = production();
    remove_effect(&mut value, "nightfall");
    let reasons = reasons(value);
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains("\"Nightfall\"")),
        "{reasons:?}"
    );
}

/// The demon's AI casts Lash of Pain, which needs a behavior.
#[test]
fn succubus_needs_lash_of_pain() {
    let mut value = production();
    remove_effect(&mut value, "lash_of_pain");
    let reasons = reasons(value);
    assert!(
        reasons
            .iter()
            .any(|reason| reason.starts_with("the demon casts") && reason.contains("11780")),
        "{reasons:?}"
    );
}

/// The simulated demon is a unit only through its AI.
#[test]
fn summoned_demon_needs_its_ai() {
    let mut value = production();
    remove_effect(&mut value, "warlock_pet");
    let reasons = reasons(value);
    assert!(
        reasons.iter().any(|reason| reason.contains("Succubus")),
        "{reasons:?}"
    );
}

/// Several simulated pets are in scope, each its own unit: a pet listed twice is invalid.
#[test]
fn a_pet_listed_twice_is_refused() {
    let mut value = production();
    let pet = value["pets"][0].clone();
    value["pets"].as_array_mut().unwrap().push(pet);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    assert!(matches!(
        check_prepared(&prepared),
        Err(PreparedError::Invalid(_))
    ));
}

/// Golden Banana's Spirit reaches Life Tap and spirit regeneration through the stat
/// combinations; a dynamic pet that would inherit the changing Spirit is unsupported, since
/// a pet's inheritance carries only its tracked powers.
#[test]
fn a_changing_spirit_reaches_the_player_but_not_a_pet() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/affliction-warlock-golden-banana.prepared.json");
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let prepared: PreparedV2 = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(check_prepared(&prepared), Ok(()));
    let label = value["pets"][0]["label"].as_str().unwrap().to_string();
    value["pets"][0]["inheritance"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"owner": "Spirit", "pet": "Spirit", "coefficient": 0.3}));
    assert!(reasons(value).contains(&format!(
        "pet {label} inherits Spirit, which changes during the fight"
    )));
}

/// The Succubus's dismissal line follows the fight: its owner's procs pass spell damage
/// through the inheritance, and taking away the permanent aura's buff and then the
/// inheritance leaves Go's float residue, which the line prints as -0.000. Without the aura's
/// stats the residue is lost.
#[test]
fn the_pet_dismissal_line_keeps_the_float_residue() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/affliction-warlock-pet-dismiss-residue.prepared.json");
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    value["sim"]["iterations"] = json!(1);
    let log = |value: &Value| {
        let prepared: PreparedV2 = serde_json::from_value(value.clone()).unwrap();
        let report = simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
        report.result["logs"].as_str().unwrap().to_string()
    };
    let residue = "\"Intellect\": 70.000,\"SpellDamage\": -0.000,\"Spirit\"";
    assert!(log(&value).contains(residue));
    value["pets"][0]["aura_stats"] = json!([]);
    assert!(!log(&value).contains(residue));
}
