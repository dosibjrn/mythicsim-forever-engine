//! Mage area spells against several targets. The Go result and first-fight log goldens of the
//! accepted fixtures are compared with every other supported case in `prepared_v2.rs`; these
//! tests pin what each area spell does on each target and what the gate asks of it.

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

fn first_fight_log(case: &str) -> String {
    let mut value = fixture(case);
    value["sim"]["iterations"] = json!(1);
    value["sim"]["debug_first_iteration"] = json!(true);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    let report = simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
    report.result["logs"].as_str().unwrap().to_string()
}

/// The damage lines of a spell on each target, with their timestamps, in log order.
fn hits(logs: &str, id: i32, what: &str) -> Vec<(String, usize)> {
    hits_of(logs, id, &[what])
}

/// The same for several outcomes at once, still in log order.
fn hits_of(logs: &str, id: i32, whats: &[&str]) -> Vec<(String, usize)> {
    logs.lines()
        .filter(|line| !line.contains("[DEBUG]"))
        .filter_map(|line| {
            let target = line.find("[Target ")?;
            let rest = &line[target + "[Target ".len()..];
            let number: usize = rest[..rest.find(']')?].parse().ok()?;
            whats
                .iter()
                .any(|what| line.contains(&format!("{{SpellID: {id}}} {what}")))
                .then(|| (line[..line.find(']').unwrap() + 1].to_string(), number))
        })
        .collect()
}

/// A rolled area hit is calculated and dealt on each target in unit index order, one cast
/// after another (Go `CalcAndDealAoeDamageWithVariance`).
#[test]
fn rolled_area_hits_strike_every_target_in_order() {
    // From 8 yards: Cone of Cold and Arcane Explosion reach the targets within 10 yards of the
    // mage only, so the same line from 20 yards never casts them.
    let logs = first_fight_log("frost-mage-3-targets-point-blank-area-lines");
    for id in [10216, 10161, 10202] {
        let strikes: Vec<_> = ["Hit", "Crit", "Miss"]
            .iter()
            .flat_map(|what| hits(&logs, id, what))
            .collect();
        assert!(!strikes.is_empty(), "no hit of {id}");
        let mut by_time = strikes;
        by_time.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let (first_time, _) = by_time[0].clone();
        let first_cast: Vec<usize> = by_time
            .iter()
            .filter(|(time, _)| *time == first_time)
            .map(|(_, target)| *target)
            .collect();
        assert_eq!(first_cast, [1, 2, 3], "spell {id}");
    }
}

/// Flamestrike's area dot sits on the mage, and each tick hits every target in turn.
#[test]
fn flamestrike_ticks_on_every_target() {
    let logs = first_fight_log("frost-mage-3-targets-area-lines");
    let ticks = hits(&logs, 10216, "tick");
    assert!(ticks.len() >= 3, "{logs}");
    let first = &ticks[0].0;
    let at_first: Vec<usize> = ticks
        .iter()
        .filter(|(time, _)| time == first)
        .map(|(_, target)| *target)
        .collect();
    assert_eq!(at_first, [1, 2, 3]);
}

/// Blizzard's tick spell hits every target each period, and crits like any spell: its tick
/// rows lack Cannot Crit. Improved Blizzard's chill follows on each target the tick landed on.
#[test]
fn blizzard_ticks_hit_every_target_each_period() {
    let logs = first_fight_log("frost-mage-3-targets-blizzard");
    let ticks = hits_of(&logs, 1279949, &["Hit", "Crit"]);
    assert!(ticks.len() >= 9, "{logs}");
    for period in ticks[..9].chunks(3) {
        let targets: Vec<usize> = period.iter().map(|(_, target)| *target).collect();
        assert_eq!(targets, [1, 2, 3]);
        assert!(period.iter().all(|(time, _)| *time == period[0].0));
    }
}

/// Blast Wave's binary hit reaches both targets.
#[test]
fn a_binary_area_hit_reaches_every_target() {
    // From 8 yards, inside the 10 yard radius Blast Wave reaches.
    let logs = first_fight_log("fire-mage-2-targets-point-blank-blast-wave");
    let mut targets: Vec<usize> = ["Hit", "Crit", "Miss"]
        .iter()
        .flat_map(|what| hits(&logs, 13021, what))
        .map(|(_, target)| target)
        .collect();
    targets.sort_unstable();
    targets.dedup();
    assert_eq!(targets, [1, 2]);
}

/// Ignite burns on the target whose crit lit it, so a Flamestrike that crits several targets
/// leaves a dot on each.
#[test]
fn ignite_burns_on_each_target_a_crit_struck() {
    let logs = first_fight_log("fire-mage-5-targets-flamestrike");
    let mut targets: Vec<usize> = hits(&logs, 412538, "tick")
        .into_iter()
        .map(|(_, target)| target)
        .collect();
    targets.sort_unstable();
    targets.dedup();
    assert!(targets.len() >= 3, "{targets:?}");
    assert!(targets.iter().any(|&target| target > 1), "{targets:?}");
}

/// Against several targets a rotation that reaches an area spell needs the effect that
/// describes it.
#[test]
fn area_spells_need_their_effects_against_several_targets() {
    for (case, kind, spell) in [
        ("frost-mage-3-targets-blizzard", "blizzard", 10187),
        ("fire-mage-5-targets-flamestrike", "flamestrike", 10216),
        ("fire-mage-2-targets-blast-wave", "blast_wave", 13021),
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

/// A dynamic modifier of a spell's dot, such as the Gnome racial Eureka's cancel of its damage
/// bonus on ticks, reaches the dot on every target: each is a dot of its own in Go. The
/// Frostfire Bolt dots on the second target tick as Go's golden does.
#[test]
fn dynamic_dot_modifiers_reach_the_dot_on_every_target() {
    let case = "frostfire-mage-2-targets-multidot";
    let ticks = |logs: &str| -> Vec<String> {
        logs.lines()
            .filter(|line| line.contains("{SpellID: 1237313}") && line.contains("tick"))
            .map(str::to_string)
            .collect()
    };
    let golden = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("fixtures/mage/prepared-v2/{case}.go-log.txt")),
    )
    .unwrap();
    let expected = ticks(&golden);
    assert!(expected.iter().any(|line| line.contains("[Target 2]")));
    assert_eq!(ticks(&first_fight_log(case)), expected);
}

/// Go drops a multidot line for a spell with an area dot: the dot belongs to the caster, and
/// `Spell.CurDot`, which `GetAPLMultidotSpell` reads, does not return it. The rotation is the
/// plain one, which never casts Flamestrike.
#[test]
fn a_multidot_line_for_an_area_dot_is_dropped() {
    let logs = first_fight_log("fire-mage-3-targets-multidot-flamestrike");
    assert!(!logs.contains("Casting {SpellID: 10216}"), "{logs}");
    assert!(logs.contains("Casting {SpellID: 25306}"), "{logs}");
}

/// A rotation condition reads an area dot: `GetAPLDot` returns the caster's `AOEDot` first, so
/// Blizzard is cast again once its channel is down.
#[test]
fn a_condition_reads_an_area_dot() {
    let logs = first_fight_log("frost-mage-3-targets-blizzard-dot-condition");
    let casts = logs.matches("Casting {SpellID: 10187}").count();
    assert!(casts >= 3, "{casts} casts of Blizzard");
}
