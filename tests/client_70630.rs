//! The non-Mage rows of client 1.60.1.70630 that change what the engine computes, which only
//! Forever's rules prepare (the frozen spell data is client 1.60.1.70425), and the rows that
//! already agree with the engine and are pinned here so a data re-import cannot move them.

use forever_engine::{prepare_json_with, Rules};
use serde_json::Value;
use std::{fs, path::Path};

#[path = "support/warrior_trees_20261104.rs"]
mod warrior_trees_20261104;

fn request(name: &str) -> Value {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/mage/prepared-v2"
    ))
    .join(format!("{name}.request.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn player(request: &mut Value) -> &mut Value {
    &mut request["raid"]["parties"][0]["players"][0]
}

fn prepared(request: &Value, rules: Rules) -> Value {
    // The fixtures' Warrior talents are the old tree's; Forever's rules read the client's order.
    let mut request = request.clone();
    if rules == Rules::Forever {
        warrior_trees_20261104::forever_request(&mut request);
    }
    prepare_json_with(request.to_string().as_bytes(), "rust", rules).unwrap()
}

fn effect<'a>(prepared: &'a Value, kind: &str) -> &'a Value {
    prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == kind)
        .unwrap_or_else(|| panic!("no {kind} effect"))
}

/// Hunter's Mark rank 4 (14325, SpellEffect 690991, aura 127) gives every attacker 143 ranged
/// Attack Power since 70630; the data and the Go reference have 71.
#[test]
fn hunters_mark_gives_143_ranged_attack_power() {
    let request = request("production-beast-mastery-hunter");
    assert_eq!(request["raid"]["debuffs"]["huntersMark"], true);
    let power = |rules| {
        prepared(&request, rules)["melee"]["ranged_state"]["defender_bonus_ranged_attack_power"]
            .as_f64()
            .unwrap()
    };
    assert_eq!(power(Rules::Forever), 143.0);
    assert_eq!(power(Rules::GoReference), 71.0);

    let mut without = request.clone();
    without["raid"]["debuffs"]["huntersMark"] = false.into();
    let none = prepared(&without, Rules::Forever)["melee"]["ranged_state"]
        ["defender_bonus_ranged_attack_power"]
        .as_f64()
        .unwrap();
    assert_eq!(none, 0.0);
}

fn warlock_with_stone(stone: &str) -> Value {
    let mut request = request("destruction-warlock-revelation");
    player(&mut request)["warlock"]["options"]["classOptions"]["weaponStone"] = stone.into();
    request
}

fn school_damage(prepared: &Value, school: &str) -> f64 {
    prepared["player"]["stats"][school].as_f64().unwrap()
}

/// The Spellstone's "Increase Shadow Dam" rows (1233765, 1233768, 1233770) name the Shadow
/// school alone since 70630; they named Fire and Shadow (mask 36). The Firestone is unchanged.
#[test]
fn spellstone_raises_shadow_damage_only() {
    let spellstone = warlock_with_stone("Spellstone");
    let forever = prepared(&spellstone, Rules::Forever);
    let reference = prepared(&spellstone, Rules::GoReference);
    assert_eq!(school_damage(&forever, "FireDamage"), 0.0);
    assert!(school_damage(&reference, "FireDamage") > 0.0);
    assert!(school_damage(&forever, "ShadowDamage") > 0.0);
    assert_eq!(
        school_damage(&forever, "ShadowDamage"),
        school_damage(&reference, "ShadowDamage")
    );

    let firestone = warlock_with_stone("Firestone");
    let forever = prepared(&firestone, Rules::Forever);
    let reference = prepared(&firestone, Rules::GoReference);
    assert!(school_damage(&forever, "FireDamage") > 0.0);
    assert_eq!(
        school_damage(&forever, "FireDamage"),
        school_damage(&reference, "FireDamage")
    );
}

fn spell_threat_multiplier(prepared: &Value, spell_id: i64) -> f64 {
    prepared["player"]["spells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|spell| spell["action_id"]["spell_id"] == spell_id)
        .unwrap_or_else(|| panic!("no spell {spell_id}"))["threat_multiplier"]
        .as_f64()
        .unwrap()
}

/// Blood Craze (16488) carries the No Threat attribute (Attributes_1 0x400) since 70630, so its
/// heal over time puts the warrior on no threat table.
#[test]
fn blood_craze_heals_without_threat() {
    let request = request("protection-warrior-blood-craze-ephemeral-power");
    assert_eq!(
        spell_threat_multiplier(&prepared(&request, Rules::Forever), 16488),
        0.0
    );
    assert_eq!(
        spell_threat_multiplier(&prepared(&request, Rules::GoReference), 16488),
        1.0
    );
}

/// Already modelled before 70630, pinned under both rules.
///
/// Bloodthrill's marker aura (1289681) drops from 6 to 5 seconds; the window the engine opens is
/// Overpower's own 5 seconds (1282733). Elemental Focus moves its 10% from ProcChance to the
/// effect (ProcChance 100, base 10); the data's ProcChance keeps the same 10%. Amplify Curse
/// leaves Bane of Agony's own mask for an aura on the target (class mask 1024, tooltip
/// "$o1*($w2/100+1)"); the engine already raises the whole tick by 50% (patch 177).
#[test]
fn rows_the_engine_already_follows_hold_under_both_rules() {
    let arms = request("production-arms-warrior");
    let shaman = request("elemental-shaman-revelation");
    let warlock = request("affliction-warlock-hellfire-revelation");
    for rules in [Rules::Forever, Rules::GoReference] {
        assert_eq!(
            effect(&prepared(&arms, rules), "bloodthrill")["window_ns"],
            5_000_000_000_i64
        );
        assert_eq!(
            effect(&prepared(&shaman, rules), "elemental_focus")["proc_chance"],
            0.1
        );
        assert_eq!(
            effect(&prepared(&warlock, rules), "bane_of_agony")["amplify"],
            1.5
        );
    }
}
