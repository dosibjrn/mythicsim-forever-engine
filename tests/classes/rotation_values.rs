//! Rotation values the live requests use: the reaction time an aura read includes, the dot base
//! duration Go captures as it builds the rotation, and the sets of units a dot or aura read
//! names. Their Go result and first-fight log goldens are compared with every other supported
//! case in `mage/prepared_v2.rs`; the variants compared with the pinned Go engine are in
//! validation/2026-10-08-live-rotation-gaps-sweep.

use forever_engine::{check_prepared, contracts::prepared_v2::PreparedV2, simulate_prepared};
use serde_json::{json, Value};
use std::{fs, path::Path};

fn fixture(case: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/mage/prepared-v2/{case}.prepared.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn parse(value: Value) -> PreparedV2 {
    serde_json::from_value(value).unwrap()
}

/// The first fight's log of a short run.
fn first_fight_log(mut value: Value) -> String {
    value["sim"]["iterations"] = json!(1);
    value["sim"]["debug_first_iteration"] = json!(true);
    let report = simulate_prepared(&parse(value), forever_engine::RngMode::Stochastic).unwrap();
    report.result["logs"].as_str().unwrap().to_string()
}

fn at(line: &str) -> f64 {
    line[1..line.find(']').unwrap()].parse().unwrap()
}

fn casts(logs: &str, spell: i32) -> usize {
    logs.matches(&format!("Casting {{SpellID: {spell}}}"))
        .count()
}

/// The rotation without any `includeReactionTime` field, which Go reads as false.
fn without_reaction_time(mut value: Value) -> Value {
    fn strip(value: &mut Value) {
        match value {
            Value::Object(fields) => {
                fields.remove("includeReactionTime");
                fields.values_mut().for_each(strip);
            }
            Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    strip(&mut value["player"]["rotation"]);
    value
}

#[test]
fn the_new_rotation_values_are_supported() {
    for name in [
        "arcane-mage-aura-is-inactive-reaction-time",
        "arcane-mage-aura-reaction-time-400ms",
        "affliction-warlock-dot-base-duration",
        "marksmanship-hunter-predators-armor-dot-base-duration",
        "shadow-priest-3-targets-all-targets-dot",
    ] {
        assert!(check_prepared(&parse(fixture(name))).is_ok(), "{name}");
    }
}

/// Go `APLValueAuraIsActive` with `includeReactionTime`: a proc that was gained less than the
/// reaction time ago is not active yet, so the rotation casts something else until it is.
#[test]
fn a_proc_is_active_only_after_the_reaction_time() {
    let value = fixture("arcane-mage-aura-reaction-time-400ms");
    let reaction = value["player"]["reaction_ns"].as_i64().unwrap() as f64 / 1e9;
    assert_eq!(reaction, 0.4);
    let logs = first_fight_log(value.clone());
    let gained = logs
        .lines()
        .find(|line| line.contains("Aura gained: {SpellID: 44404}"))
        .expect("Missile Barrage is gained");
    let missiles = logs
        .lines()
        .filter(|line| line.contains("Casting {SpellID: 25345}"))
        .map(at)
        .find(|when| *when >= at(gained))
        .expect("Arcane Missiles is cast after the proc");
    assert!(
        missiles - at(gained) >= reaction - 0.0051,
        "gained at {}, cast at {missiles}\n{logs}",
        at(gained)
    );
    // Without the field the same proc is cast on at once, so the fights differ.
    let plain = first_fight_log(without_reaction_time(value));
    assert_ne!(logs, plain);
}

/// Go `Aura.TimeInactive`: an aura that never was up has been down for ever, so the first
/// cast of a rotation that waits for it to be down is the one it guards.
#[test]
fn an_aura_that_never_was_up_has_been_down_for_ever() {
    let logs = first_fight_log(fixture("arcane-mage-aura-is-inactive-reaction-time"));
    let first = logs
        .lines()
        .find(|line| {
            line.contains("Casting {SpellID:") && !line.contains("Casting {SpellID: 12051}")
        })
        .unwrap();
    assert!(first.contains("Casting {SpellID: 1239700}"), "{logs}");
}

/// Go reads a dot's base duration when it builds the rotation; the runtime reads what the
/// exporter captured, and a spell without a captured dot has no value, which drops the term.
#[test]
fn the_dot_base_duration_is_the_one_the_exporter_captured() {
    let value = fixture("affliction-warlock-dot-base-duration");
    assert!(casts(&first_fight_log(value.clone()), 25311) > 0);
    // A base duration longer than the fight: no dot's term ever holds.
    let mut longer = value.clone();
    for entry in longer["player"]["rotation_dot_base_durations"]
        .as_array_mut()
        .unwrap()
    {
        entry["base_duration_ns"] = json!(3_600_000_000_000_i64);
    }
    let logs = first_fight_log(longer);
    for spell in [11713, 25311, 25309] {
        assert_eq!(casts(&logs, spell), 0, "{spell}\n{logs}");
    }
    // No captured duration: the comparison has no value and the dots are cast as usual.
    let mut none = value;
    none["player"]
        .as_object_mut()
        .unwrap()
        .remove("rotation_dot_base_durations");
    assert!(casts(&first_fight_log(none), 25311) > 0);
}

/// The set bonus' extra tick is added as the reset activates it, after Go built the rotation,
/// so the captured duration is the five ticks of the sting without it.
#[test]
fn the_captured_duration_leaves_out_a_set_bonus_activated_at_the_reset() {
    let value = fixture("marksmanship-hunter-predators-armor-dot-base-duration");
    let captured = value["player"]["rotation_dot_base_durations"][0]["base_duration_ns"]
        .as_i64()
        .unwrap();
    let sting = value["player"]["spells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|spell| spell["action_id"] == json!({"spell_id": 25295}))
        .unwrap();
    let dot = &sting["dot"];
    let reset_duration =
        dot["base_tick_count"].as_i64().unwrap() * dot["base_tick_length_ns"].as_i64().unwrap();
    assert_eq!(captured, 15_000_000_000);
    assert_eq!(reset_duration, 18_000_000_000);
}

/// Go `GetUnit` names no unit for `AllTargets` and `AllPlayers`, so a dot read on them drops out
/// of its condition and the rotation casts as if the term were absent.
#[test]
fn a_dot_read_on_all_targets_drops_out_of_its_condition() {
    let value = fixture("shadow-priest-3-targets-all-targets-dot");
    let item = &value["player"]["rotation"]["priorityList"][1];
    assert_eq!(
        item["action"]["condition"]["and"]["vals"][0]["not"]["val"]["dotIsActive"]["targetUnit"]
            ["type"],
        "AllTargets"
    );
    // Shadow Word: Pain is cast whenever the fight has six seconds left, with the dot up or not.
    let logs = first_fight_log(value);
    assert!(casts(&logs, 10894) > 3, "{logs}");
}

/// A reaction time on the stacks of an aura, or on another unit's aura, is read as Go does.
#[test]
fn reaction_time_on_any_stack_or_activity_read_is_supported() {
    for read in [
        json!({"auraNumStacks": {"auraId": {"spellId": 400573}, "includeReactionTime": true}}),
        json!({"auraIsActive": {"auraId": {"spellId": 400573}, "includeReactionTime": true,
            "sourceUnit": {"type": "CurrentTarget"}}}),
        json!({"auraIsInactive": {"auraId": {"spellId": 400573}, "includeReactionTime": true,
            "sourceUnit": {"type": "Target", "index": 0}}}),
    ] {
        let mut value = fixture("arcane-mage-aura-is-inactive-reaction-time");
        value["player"]["rotation"]["priorityList"][1]["action"]["condition"] = read;
        assert_eq!(check_prepared(&parse(value)), Ok(()));
    }
}
