//! Rain of Fire and Hellfire, the Warlock's area channels, against one target and several.
//! Their Go result and first-fight log goldens are compared with every other supported case
//! in tests/classes/mage/prepared_v2.rs.

use forever_engine::{
    check_prepared, contracts::prepared_v2::PreparedV2, simulate_prepared, PreparedError,
};
use serde_json::{json, Value};
use std::{fs, path::Path};

fn fixture(case: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/mage/prepared-v2/{case}.prepared.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn reasons(value: Value) -> Vec<String> {
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    match check_prepared(&prepared) {
        Err(PreparedError::Unsupported(reasons)) => reasons,
        other => panic!("expected unsupported, got {other:?}"),
    }
}

fn first_fight_log(mut value: Value) -> String {
    value["sim"]["iterations"] = json!(1);
    value["sim"]["debug_first_iteration"] = json!(true);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    let report = simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
    report.result["logs"].as_str().unwrap().to_string()
}

/// Rain of Fire's channel is an area dot whose every tick casts the tick spell, which hits
/// the target for a fixed amount: four ticks, two seconds apart, for each cast.
#[test]
fn rain_of_fire_ticks_four_times_two_seconds_apart() {
    let logs = first_fight_log(fixture("destruction-warlock-rain-of-fire"));
    let casts: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("Casting {SpellID: 11678}"))
        .collect();
    assert!(casts.len() >= 2, "{logs}");
    let ticks: Vec<&str> = logs
        .lines()
        .filter(|line| {
            line.contains("[Target 1] {SpellID: 1282385}")
                && (line.contains(" Hit ") || line.contains(" Crit "))
        })
        .collect();
    let stamp = |line: &str| line[1..line.find(']').unwrap()].parse::<f64>().unwrap();
    assert!(ticks.len() >= 4, "{logs}");
    for pair in ticks[..4].windows(2) {
        assert!(
            (stamp(pair[1]) - stamp(pair[0]) - 2.0).abs() < 0.015,
            "{pair:?}"
        );
    }
}

/// The simulation has no healer, so Hellfire's burn does not cut the channel short: once the
/// warlock's health is spent the channel keeps ticking, and every tick still burns the warlock
/// (Go `TestHellfireChannelsAtLowHealth`, community #735). Stopping used to clip every channel
/// after about 18 seconds into one-tick recasts.
#[test]
fn hellfire_keeps_channelling_at_low_health() {
    let logs = first_fight_log(fixture("affliction-warlock-hellfire"));
    let lines: Vec<&str> = logs.lines().collect();
    let empty = lines
        .iter()
        .position(|line| line.contains("Spent 210.000 health") && line.contains("--> 0.000)"))
        .expect("the warlock's health runs out");
    let ticks = lines[empty..]
        .iter()
        .filter(|line| line.contains("[Target 1] {SpellID: 11684} tick"))
        .count();
    let burned = lines[empty..]
        .iter()
        .filter(|line| line.contains("Spent 210.000 health from {OtherID: 9}"))
        .count();
    assert!(ticks >= 4, "{logs}");
    assert!(burned >= ticks, "{logs}");
}

/// Hellfire burns the warlock for every tick it deals.
#[test]
fn hellfire_burns_the_warlock_each_tick() {
    let logs = first_fight_log(fixture("affliction-warlock-hellfire"));
    let ticks = logs
        .lines()
        .filter(|line| line.contains("[Target 1] {SpellID: 11684} tick"))
        .count();
    let burns = logs
        .lines()
        .filter(|line| line.contains("Spent 210.000 health from {OtherID: 9}"))
        .count();
    assert!(ticks > 0);
    assert_eq!(ticks, burns, "{logs}");
}

/// A rotation that reaches either channel needs the effect that describes it.
#[test]
fn the_channels_need_their_effects() {
    for (case, kind, spell) in [
        ("destruction-warlock-rain-of-fire", "rain_of_fire", 11678),
        ("affliction-warlock-hellfire", "hellfire", 11684),
    ] {
        let mut value = fixture(case);
        value["effects"]
            .as_array_mut()
            .unwrap()
            .retain(|effect| effect["kind"] != kind);
        assert!(
            reasons(value).contains(&format!(
                "rotation reaches spell {spell} without a known behavior"
            )),
            "{kind}"
        );
    }
}

/// A Hellfire whose client row forbids the burn's crit picks an outcome Rust does not
/// simulate.
#[test]
fn a_hellfire_that_cannot_crit_is_refused() {
    let mut value = fixture("affliction-warlock-hellfire");
    for effect in value["effects"].as_array_mut().unwrap() {
        if effect["kind"] == "hellfire" {
            effect["tick_can_crit"] = json!(false);
        }
    }
    assert!(reasons(value)
        .contains(&"Hellfire's burn cannot crit, which Rust does not simulate".to_string()));
}

/// The tick spell of Rain of Fire hits every target in unit index order, each rolled in turn.
#[test]
fn rain_of_fire_reaches_every_target() {
    let logs = first_fight_log(fixture("affliction-warlock-3-targets-rain-of-fire"));
    let first_tick = logs
        .lines()
        .find(|line| line.contains("{SpellID: 1282385}") && !line.contains("[DEBUG]"))
        .expect("Rain of Fire ticks");
    let stamp = &first_tick[..first_tick.find(']').unwrap() + 1];
    let at_stamp: Vec<&str> = logs
        .lines()
        .filter(|line| {
            line.starts_with(stamp)
                && line.contains("[Target ")
                && line.contains("{SpellID: 1282385}")
                && !line.contains("[DEBUG]")
        })
        .collect();
    assert_eq!(at_stamp.len(), 3, "{at_stamp:?}");
    for (index, line) in at_stamp.iter().enumerate() {
        assert!(line.contains(&format!("[Target {}]", index + 1)), "{line}");
    }
}

/// Hellfire calculates its tick on every target before dealing any, as Go
/// `CalcPeriodicAoeDamage` and `DealBatchedPeriodicDamage` do: both targets' calculations,
/// then both hits.
#[test]
fn hellfire_calculates_every_target_before_dealing_any() {
    let logs = first_fight_log(fixture("demonology-warlock-2-targets-hellfire"));
    let lines: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("{SpellID: 11684}"))
        .collect();
    let first = lines
        .iter()
        .position(|line| line.contains("[DEBUG]"))
        .expect("Hellfire ticks");
    let order: Vec<(&str, bool)> = lines[first..first + 4]
        .iter()
        .map(|line| {
            let target = if line.contains("[Target 1]") {
                "1"
            } else {
                "2"
            };
            (target, line.contains("[DEBUG]"))
        })
        .collect();
    assert_eq!(
        order,
        [("1", true), ("2", true), ("1", false), ("2", false)]
    );
}

/// Bane of Havoc copies 15% of the damage the warlock deals to the other targets onto the
/// baned one, as a hit of the bane's copy spell right after each such hit; the baned first
/// target takes no copy of its own damage (Go `applyBaneOfHavoc`).
#[test]
fn bane_of_havoc_copies_the_damage_to_other_targets_onto_the_baned_one() {
    let logs = first_fight_log(fixture(
        "destruction-warlock-3-targets-bane-of-havoc-rain-of-fire",
    ));
    let lines: Vec<&str> = logs
        .lines()
        .filter(|line| !line.contains("[DEBUG]") && line.contains("[Target "))
        .collect();
    let damage = |line: &str| -> f64 {
        let at = line.find(" for ").unwrap() + " for ".len();
        line[at..line.find(" damage").unwrap()].parse().unwrap()
    };
    let mut copies = 0;
    for (index, line) in lines.iter().enumerate() {
        let is_tick = line.contains("{SpellID: 1282385}");
        if !is_tick || line.contains("[Target 1]") || !line.contains(" for ") {
            continue;
        }
        let copy = lines[index + 1];
        assert!(
            copy.contains("[Target 1] {SpellID: 1225228, Tag: 1} Hit for"),
            "{copy}"
        );
        assert!(
            (damage(copy) - damage(line) * 0.15).abs() < 0.001,
            "{line} {copy}"
        );
        copies += 1;
    }
    assert!(copies >= 2, "{logs}");
    // Nothing is copied from the damage the baned target takes itself.
    for (index, line) in lines.iter().enumerate() {
        if line.contains("[Target 1] {SpellID: 1282385}") {
            assert!(
                !lines[index + 1].contains("Tag: 1}"),
                "{}",
                lines[index + 1]
            );
        }
    }
}

/// A multidot line casts each dot on the first target whose dot is down, so Corruption and
/// Bane of Agony land on the second and third target in turn, each through its own copy of
/// the dot.
#[test]
fn a_multidot_lands_each_dot_on_the_target_it_picks() {
    let logs = first_fight_log(fixture("affliction-warlock-3-targets-multidot-dots"));
    for spell in [25311, 11713] {
        let gained: Vec<&str> = logs
            .lines()
            .filter(|line| line.contains(&format!("Aura gained: {{SpellID: {spell}}}")))
            .map(|line| &line[line.find("[Target ").unwrap()..][..10])
            .collect();
        assert_eq!(
            gained[..3],
            ["[Target 1]", "[Target 2]", "[Target 3]"],
            "{spell}"
        );
    }
}

/// Each target holds its own curse and bane: a bane cast on the second target does not fade
/// the one on the first (Go `currentActiveBane` is an array by unit).
#[test]
fn each_target_keeps_its_own_bane() {
    let logs = first_fight_log(fixture("affliction-warlock-3-targets-multidot-dots"));
    let first_agony_fade = logs
        .lines()
        .position(|line| line.contains("[Target 1] Aura faded: {SpellID: 11713}"));
    let second_agony_gain = logs
        .lines()
        .position(|line| line.contains("[Target 2] Aura gained: {SpellID: 11713}"));
    assert!(
        second_agony_gain.unwrap() < first_agony_fade.unwrap(),
        "{logs}"
    );
}
