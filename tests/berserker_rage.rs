//! Berserker Rage (18499) under Forever's rules, against the live client rows Wowhead Forever
//! shows on 4 November 2026: instant, a 1.5 second GCD in the Normal category, a 30 second
//! cooldown and Berserker Stance only; Bloodrage (2687) has no GCD. Improved Berserker Rage
//! (Fury, row 5, two ranks of 20500) makes it generate 5 or 10 Rage.
//!
//! A Fury reference that takes Improved Berserker Rage over Improved Bloodrage only gains if its
//! rotation presses Berserker Rage: the Automatic (Arms or Fury) preset hides that line, and its
//! survival major cooldown never autocasts above the defensive health threshold, as in Go.

use forever_engine::{
    check_prepared, contracts::prepared_v2::PreparedV2, prepare_json_with, simulate_prepared, Rules,
};
use serde_json::{json, Value};
use std::{fs, path::Path};

const BERSERKER_RAGE: i64 = 18499;
const HAMSTRING: i64 = 7373;
/// The Fury board's tree with Improved Berserker Rage at 2 of 2 in place of Improved Bloodrage.
const FURY_BERSERKER_RAGE: &str = "30313213002-053530005122100231-";

fn production(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "fixtures/mage/prepared-v2/production-{name}.request.json"
    ));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn player_mut(request: &mut Value) -> &mut Value {
    &mut request["raid"]["parties"][0]["players"][0]
}

fn prepared(request: &Value) -> Value {
    prepare_json_with(request.to_string().as_bytes(), "rust", Rules::Forever)
        .unwrap_or_else(|err| panic!("{err:?}"))
}

fn rage_gain(talents: &str) -> f64 {
    let mut request = production("warrior");
    player_mut(&mut request)["talentsString"] = json!(talents);
    prepared(&request)["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == "berserker_rage")
        .expect("a berserker_rage effect")["rage_gain"]
        .as_f64()
        .unwrap()
}

/// A Fury warrior with the given stance and priority list, no prepull, one fight with no
/// variation, and its log.
fn first_fight_log(stance: &str, list: Value, seconds: f64) -> String {
    let mut request = production("warrior");
    request["encounter"]["duration"] = json!(seconds);
    request["encounter"]["durationVariation"] = json!(0);
    let player = player_mut(&mut request);
    player["talentsString"] = json!(FURY_BERSERKER_RAGE);
    player["dpsWarrior"]["options"]["classOptions"]["defaultStance"] = json!(stance);
    player["rotation"] = json!({"type": "TypeAPL", "priorityList": list});
    let mut value = prepared(&request);
    value["sim"]["iterations"] = json!(1);
    value["sim"]["debug_first_iteration"] = json!(true);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    check_prepared(&prepared).unwrap();
    simulate_prepared(&prepared).unwrap().result["logs"]
        .as_str()
        .unwrap()
        .to_string()
}

fn cast(spell: i64) -> Value {
    json!({"action": {"castSpell": {"spellId": {"spellId": spell}}}})
}

/// The times of the casts of a spell.
fn casts(logs: &str, spell: i64) -> Vec<f64> {
    let needle = format!("Casting {{SpellID: {spell}}}");
    logs.lines()
        .filter(|line| line.contains(&needle))
        .map(|line| line[1..line.find(']').unwrap()].parse().unwrap())
        .collect()
}

/// Improved Berserker Rage: 5 Rage at rank 1, 10 at rank 2, none untalented. Its square is the
/// twelfth of the client's 18 Fury talents.
#[test]
fn improved_berserker_rage_gives_5_or_10_rage() {
    assert_eq!(rage_gain("-000000000000000000-"), 0.0);
    assert_eq!(rage_gain("-000000000001000000-"), 5.0);
    assert_eq!(rage_gain("-000000000002000000-"), 10.0);
}

/// In Berserker Stance Berserker Rage goes on its 30 second cooldown, gives Improved Berserker
/// Rage's 10 Rage each time and takes the 1.5 second GCD: the Hamstring after it waits for it.
#[test]
fn berserker_rage_takes_the_gcd_every_30_seconds() {
    let logs = first_fight_log(
        "WarriorStanceBerserker",
        json!([cast(BERSERKER_RAGE), cast(HAMSTRING)]),
        70.0,
    );
    assert_eq!(casts(&logs, BERSERKER_RAGE), vec![0.0, 30.0, 60.0]);
    let gains = logs
        .lines()
        .filter(|line| line.contains("rage from {SpellID: 18499}"))
        .count();
    assert_eq!(gains, 3, "{logs}");
    assert!(logs.contains(
        "Casting {SpellID: 18499} (Cost = 0.000, Cast Time = 0s, GCD = 1.5s, Effective Time = 1.5s)"
    ));
    let hamstrings = casts(&logs, HAMSTRING);
    assert_eq!(hamstrings.first(), Some(&1.5), "{logs}");
    assert!(!hamstrings.contains(&30.0) && hamstrings.contains(&31.5));
}

/// Out of Berserker Stance it is never cast.
#[test]
fn berserker_rage_needs_berserker_stance() {
    for stance in ["WarriorStanceBattle", "WarriorStanceDefensive"] {
        let logs = first_fight_log(stance, json!([cast(BERSERKER_RAGE)]), 40.0);
        assert!(casts(&logs, BERSERKER_RAGE).is_empty(), "{stance}");
    }
}

/// The preset's line is hidden, and Autocast Other Cooldowns never fires its survival major
/// cooldown at full health, so the talent's Rage never arrives unless the line is shown.
#[test]
fn a_hidden_line_leaves_berserker_rage_unused() {
    let mut hidden = cast(BERSERKER_RAGE);
    hidden["hide"] = json!(true);
    let autocast = json!({"action": {"autocastOtherCooldowns": {}}});
    let logs = first_fight_log(
        "WarriorStanceBerserker",
        json!([autocast, hidden, cast(HAMSTRING)]),
        70.0,
    );
    assert!(casts(&logs, BERSERKER_RAGE).is_empty());
    hidden.as_object_mut().unwrap().remove("hide");
    let logs = first_fight_log(
        "WarriorStanceBerserker",
        json!([autocast, hidden, cast(HAMSTRING)]),
        70.0,
    );
    // Shown, it goes every 30 seconds once the autocast cooldowns ahead of it have had the GCD.
    let shown = casts(&logs, BERSERKER_RAGE);
    assert_eq!(shown.len(), 3, "{shown:?}");
    assert_eq!(shown[1] - shown[0], 30.0);
}
