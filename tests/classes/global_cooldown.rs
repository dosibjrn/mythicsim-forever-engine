//! Haste shortens the global cooldown down to a floor of 1 second, as Forever's developers
//! confirmed (MythicSim Discord, 24 October 2026) and as the Go reference already does
//! (`Cast.GCDTime`, `GCDMin` defaulting to a second). Cast times keep shrinking below it.

use std::{fs, path::Path};

use forever_engine::contracts::prepared_v2::PreparedV2;
use serde_json::{json, Value};

/// Arcane Blast (1239700), 1.5 second global cooldown. The fixture opens with Presence of Mind,
/// so the first is instant and the second starts when its global cooldown ends.
const ARCANE_BLAST: &str = "{SpellID: 1239700}";

/// The first two Arcane Blasts of a fight at the given cast speed multiplier: their start times
/// and the global cooldown the log gives the first.
fn arcane_blasts(cast_speed_multiplier: f64) -> (f64, f64, String) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/arcane-arcane-explosion.prepared.json");
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(
        value["player"]["stats"]["SpellHasteRating"].as_f64(),
        Some(0.0)
    );
    value["player"]["pseudo_stats"]["cast_speed_multiplier"] = json!(cast_speed_multiplier);
    value["player"]["cast_speed"] = json!(1.0 / cast_speed_multiplier);
    value["sim"]["iterations"] = json!(1);
    value["sim"]["debug_first_iteration"] = json!(true);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    let report =
        forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
    let logs = report.result["logs"].as_str().unwrap();
    let casts: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains(&format!("Casting {ARCANE_BLAST}")))
        .take(2)
        .collect();
    assert!(casts[0].contains("Cast Time = 0s"), "{}", casts[0]);
    let time = |line: &str| line[1..line.find(']').unwrap()].parse::<f64>().unwrap();
    let gcd = casts[0][casts[0].find("GCD = ").unwrap() + 6..]
        .split(',')
        .next()
        .unwrap()
        .to_string();
    (time(casts[0]), time(casts[1]), gcd)
}

#[test]
fn haste_shortens_the_global_cooldown_to_a_one_second_floor() {
    // 25% haste: 1.5 s becomes 1.2 s.
    let (first, second, gcd) = arcane_blasts(1.25);
    assert_eq!(gcd, "1.2s");
    assert!((second - first - 1.2).abs() < 1e-9, "{first} {second}");
    // 1.5 s at 0.6 of its length would be 0.9 s; the floor holds it at 1 s.
    let (first, second, gcd) = arcane_blasts(1.0 / 0.6);
    assert_eq!(gcd, "1s");
    assert!((second - first - 1.0).abs() < 1e-9, "{first} {second}");
}
