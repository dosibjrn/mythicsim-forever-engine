//! The live hotfix cache of 3 November 2026 (client 1.60.1.70707) and Blizzard's development
//! notes of 2 November, which only Forever's rules prepare. The Go reference keeps the frozen
//! data (client 1.60.1.70425) and its numbers.

use forever_engine::{prepare_json_with, Rules};
use serde_json::Value;
use std::{fs, path::Path};

#[path = "support/shaman_layout.rs"]
mod shaman_layout;
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

/// The ranged Attack Power of each stat aura combination that holds Aspect of the Hawk, minus
/// the same combination without it.
fn hawk_ranged_attack_power(prepared: &Value) -> f64 {
    let table = prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| {
            effect["combos"].is_array()
                && effect["auras"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|aura| aura == "Aspect of the Hawk")
        })
        .expect("a stat aura table with Aspect of the Hawk");
    let bit = table["auras"]
        .as_array()
        .unwrap()
        .iter()
        .position(|aura| aura == "Aspect of the Hawk")
        .unwrap();
    let combos = table["combos"].as_array().unwrap();
    let rap = |i: usize| combos[i]["RangedAttackPower"].as_f64().unwrap();
    rap(1 << bit) - rap(0)
}

/// Aimed Shot, Arcane Shot, Sniper Shot and Serpent Sting at the ranks a level 60 Hunter casts,
/// with the notes' attack power shares for Arcane Shot (15%) and Serpent Sting (5% a tick).
#[test]
fn hunter_shots_take_the_hotfix() {
    let request = request("production-marksmanship-hunter");
    let forever = prepared(&request, Rules::Forever);
    let reference = prepared(&request, Rules::GoReference);

    assert_eq!(effect(&forever, "aimed_shot")["flat_bonus"], 350.0);
    assert_eq!(effect(&reference, "aimed_shot")["flat_bonus"], 166.0);
    assert_eq!(effect(&forever, "arcane_shot")["base_damage"], 280.0);
    assert_eq!(effect(&forever, "arcane_shot")["rap_coefficient"], 0.15);
    assert_eq!(effect(&reference, "arcane_shot")["base_damage"], 217.0);
    assert_eq!(effect(&reference, "arcane_shot")["rap_coefficient"], 0.1);
    assert_eq!(effect(&forever, "sniper_shot")["flat_bonus"], 590.0);
    assert_eq!(effect(&reference, "sniper_shot")["flat_bonus"], 295.0);
    // Serpent Sting rank 8 under Forever's rules (rank 9 is an Ahn'Qiraj 20 book rank).
    let sting = effect(&forever, "serpent_sting");
    assert_eq!(sting["spell_id"], 13555);
    assert_eq!(sting["tick_base"], 100.0);
    assert_eq!(sting["attack_power_share"], 0.05);
    assert_eq!(effect(&reference, "serpent_sting")["tick_base"], 111.0);
    assert_eq!(
        effect(&reference, "serpent_sting")["attack_power_share"],
        0.035
    );
}

/// Aspect of the Hawk rank 6 (the rank without the book) gives 110 ranged Attack Power, was 55;
/// the reference casts rank 7 at 120.
#[test]
fn aspect_of_the_hawk_rank_6_is_110() {
    let request = request("production-marksmanship-hunter");
    assert_eq!(
        effect(&prepared(&request, Rules::Forever), "aspect_of_the_hawk")["spell_id"],
        14322
    );
    assert_eq!(
        hawk_ranged_attack_power(&prepared(&request, Rules::Forever)),
        110.0
    );
    assert_eq!(
        hawk_ranged_attack_power(&prepared(&request, Rules::GoReference)),
        120.0
    );
}

/// Trueshot Aura rank 5 gives 100 ranged Attack Power (was 50); the reference reads rank 4 at
/// 75. Measured on a party that gains the aura against the same party without it.
#[test]
fn trueshot_aura_is_100() {
    let without = request("production-arms-warrior");
    let mut with = without.clone();
    with["raid"]["parties"][0]["buffs"]["trueshotAura"] = Value::Bool(true);
    let rap = |request: &Value, rules: Rules| {
        prepared(request, rules)["player"]["stats"]["RangedAttackPower"]
            .as_f64()
            .unwrap()
    };
    assert_eq!(
        rap(&with, Rules::Forever) - rap(&without, Rules::Forever),
        100.0
    );
    assert_eq!(
        rap(&with, Rules::GoReference) - rap(&without, Rules::GoReference),
        75.0
    );
}

fn spell<'a>(prepared: &'a Value, class_spell: &str) -> Vec<&'a Value> {
    prepared["player"]["spells"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|spell| spell["class_spell"] == class_spell)
        .collect()
}

/// The stat aura multipliers a stance or talent leaves on threat, in table order.
fn threat_multipliers(prepared: &Value, aura: &str) -> Vec<f64> {
    prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|effect| effect["auras"].as_array())
        .flatten()
        .filter(|entry| entry["aura"] == aura && entry["stat"] == "threat")
        .map(|entry| entry["multiplier"].as_f64().unwrap())
        .collect()
}

/// Defensive Stance's threat 80% (was 30%) and Defiance 20% at 3 points (was 15%).
#[test]
fn defensive_stance_and_defiance_threat() {
    let request = request("production-protection-warrior");
    let forever = prepared(&request, Rules::Forever);
    let reference = prepared(&request, Rules::GoReference);
    assert_eq!(threat_multipliers(&forever, "Defensive Stance"), [1.8, 1.2]);
    assert_eq!(
        threat_multipliers(&reference, "Defensive Stance"),
        [1.3, 1.15]
    );
}

/// Slam's cooldown 15 sec (was 18), less Improved Slam's 5 sec at 2 points (was 3), and
/// Two-Handed Weapon Specialization 6% at 3 points (was 3%).
#[test]
fn slam_and_two_handed_specialization() {
    let request = request("production-arms-warrior");
    let forever = prepared(&request, Rules::Forever);
    let reference = prepared(&request, Rules::GoReference);
    let cooldown = |prepared: &Value| spell(prepared, "slam")[0]["cd"]["duration_ns"].clone();
    assert_eq!(cooldown(&forever), 10_000_000_000i64);
    assert_eq!(cooldown(&reference), 15_000_000_000i64);
    let two_handed =
        |prepared: &Value| spell(prepared, "mortal_strike")[0]["damage_multiplier"].clone();
    assert_eq!(two_handed(&forever), 1.06);
    assert_eq!(two_handed(&reference), 1.03);
}

/// Shadow and Flame 20% at 5 points (was 10%); Conflagrate's chance to keep Immolate keeps its
/// value.
#[test]
fn shadow_and_flame_is_twenty_percent() {
    let request = request("production-destruction-warlock");
    let forever = prepared(&request, Rules::Forever);
    let reference = prepared(&request, Rules::GoReference);
    assert_eq!(effect(&forever, "shadow_and_flame")["multiplier"], 1.2);
    assert_eq!(effect(&reference, "shadow_and_flame")["multiplier"], 1.1);
    assert_eq!(
        effect(&forever, "conflagrate")["keep_immolate_chance"],
        effect(&reference, "conflagrate")["keep_immolate_chance"]
    );
}

/// Lightning Overload 30% at 3 points (was 10%).
#[test]
fn lightning_overload_is_thirty_percent() {
    let request = request("production-elemental-shaman");
    let forever = prepared(&request, Rules::Forever);
    let reference = prepared(&request, Rules::GoReference);
    assert_eq!(effect(&forever, "lightning_bolt")["overload_chance"], 0.3);
    assert_eq!(effect(&reference, "lightning_bolt")["overload_chance"], 0.1);
}

/// Stormstrike 150% weapon damage on a 6 sec cooldown (was 100% and 8 sec), Flurry 30% at 5
/// points (was 25%) and Maelstrom Weapon's doubled chance.
#[test]
fn enhancement_talents_and_stormstrike() {
    let request = request("production-enhancement-shaman");
    // The fixture's talents in the launch client's order, which Forever's rules read.
    let mut forever_request = request.clone();
    shaman_layout::forever_request(&mut forever_request);
    let forever = prepared(&forever_request, Rules::Forever);
    let reference = prepared(&request, Rules::GoReference);
    for (prepared, multiplier, cooldown) in [
        (&forever, 1.5, 6_000_000_000i64),
        (&reference, 1.0, 8_000_000_000),
    ] {
        let strikes = spell(prepared, "stormstrike_damage");
        assert_eq!(strikes.len(), 2);
        assert!(strikes
            .iter()
            .all(|strike| strike["damage_multiplier"] == multiplier));
        assert_eq!(
            spell(prepared, "stormstrike_cast")[0]["cd"]["duration_ns"],
            cooldown
        );
    }
    let chance = |prepared: &Value| {
        effect(prepared, "maelstrom_weapon")["chances"][0]["chance"]
            .as_f64()
            .unwrap()
    };
    assert!((chance(&forever) - 2.0 * chance(&reference)).abs() < 1e-12);
    assert_eq!(effect(&forever, "flurry")["melee_speed_multiplier"], 1.3);
    assert_eq!(effect(&reference, "flurry")["melee_speed_multiplier"], 1.25);
}
