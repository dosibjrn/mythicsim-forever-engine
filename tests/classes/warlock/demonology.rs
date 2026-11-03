//! The production Demonology Warlock build: coverage regressions around Decimation, Demonic
//! Brand and the inert demons. Its Go result and first-fight log goldens are compared with
//! every other supported case in tests/classes/mage/prepared_v2.rs.

use forever_engine::{
    check_prepared, contracts::prepared_v2::PreparedV2, simulate_prepared, PreparedError,
};
use serde_json::{json, Value};
use std::{fs, path::Path};

fn production() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/production-demonology-warlock.prepared.json");
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

fn effect<'a>(value: &'a mut Value, kind: &str) -> &'a mut Value {
    value["effects"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|effect| effect["kind"] == kind)
        .unwrap()
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
    for kind in ["decimation", "demonic_brand", "warlock_pet", "lash_of_pain"] {
        assert!(kinds.contains(&kind), "missing {kind}");
    }
}

/// Decimation's trigger and Demonic Brand's trigger and consumer listen to hits; each needs
/// its effect.
#[test]
fn triggers_need_their_effects() {
    let mut value = production();
    remove_effect(&mut value, "decimation");
    assert!(reasons(value).contains(
        &"player aura \"Decimation Trigger\" listens to combat events without an effect".into()
    ));
    let mut value = production();
    remove_effect(&mut value, "demonic_brand");
    let reasons = reasons(value);
    assert!(reasons.contains(
        &"player aura \"Demonic Brand Trigger\" listens to combat events without an effect".into()
    ));
    assert!(reasons.contains(
        &"pet aura \"Demonic Brand consumer\" listens to combat events without an effect".into()
    ));
}

/// Only the 35% execute phase is modeled.
#[test]
fn decimation_needs_the_35_percent_phase() {
    let mut value = production();
    effect(&mut value, "decimation")["execute_phase"] = json!(20);
    assert!(reasons(value).contains(&"Decimation's execute phase 20 is unsupported".into()));
}

/// The brand hit reads the warlock's live Shadow damage, which Frozen Heart of the Mountain
/// raises by 29 for its first 15 seconds.
#[test]
fn brand_reads_the_live_school_power() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/demonology-warlock-frozen-heart.prepared.json");
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    value["sim"]["iterations"] = json!(1);
    let first_brand_base = |value: Value| -> f64 {
        let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
        let report = simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
        let logs = report.result["logs"].as_str().unwrap();
        let line = logs
            .lines()
            .find(|line| line.contains("{SpellID: 1293697} [DEBUG]"))
            .unwrap();
        let rest = line.split("BaseDamage:").nth(1).unwrap();
        rest.split(',').next().unwrap().parse().unwrap()
    };
    let raised = first_brand_base(value.clone());
    // The same fight with the trinket leaving Shadow damage alone.
    let combos = effect(&mut value, "stat_auras")["combos"]
        .as_array_mut()
        .unwrap();
    let reset = combos[0]["ShadowDamage"].clone();
    for combo in combos {
        combo["ShadowDamage"] = reset.clone();
    }
    let fixed = first_brand_base(value);
    assert!(
        (raised - fixed - 0.078 * 29.0).abs() < 0.06,
        "{raised} {fixed}"
    );
}

/// The inert demons export their permanent auras, which Go activates at every reset, and
/// the brand marker the rotation's auraIsKnown guard looks up on the first of them.
#[test]
fn inert_demons_carry_their_permanent_auras() {
    let value = production();
    let inert: Vec<&Value> = value["effects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|effect| effect["kind"] == "inert_pet")
        .collect();
    assert_eq!(inert.len(), 3);
    for pet in inert {
        let permanent = pet["permanent_auras"].as_array().unwrap();
        assert!(permanent.contains(&json!({"spell_id": 19028})));
        assert!(pet["auras"]
            .as_array()
            .unwrap()
            .contains(&json!({"spell_id": 1293696})));
    }
}

/// The Voidwalker's sacrifice restores mana from its aura's gain, which needs its effect.
#[test]
fn fel_energy_needs_its_effect() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/demonology-warlock-voidwalker-pact.prepared.json");
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let prepared: PreparedV2 = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(check_prepared(&prepared), Ok(()));
    remove_effect(&mut value, "fel_energy");
    assert!(reasons(value.clone()).contains(&"Fel Energy restores mana without an effect".into()));
    assert!(crate::refusal_codes(value).contains(&(
        "class_limit",
        "Fel Energy restores mana without an effect".into()
    )));
}
