//! The 26 October 2026 development notes' racial pass, which only Forever's rules prepare: Orc
//! Command on Hunter and Warlock pets, and Read Leyline beside Innervate.

use forever_engine::{
    contracts::prepared_v2::PreparedV2, prepare_json_with, simulate_prepared, Rules,
};
use serde_json::Value;
use std::{fs, path::Path};

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
    prepare_json_with(request.to_string().as_bytes(), "rust", rules).unwrap()
}

fn pet_damage_multipliers(prepared: &Value) -> Vec<f64> {
    prepared["pets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pet| {
            pet["pseudo_stats"]["damage_dealt_multiplier"]
                .as_f64()
                .unwrap()
        })
        .collect()
}

#[test]
fn command_raises_hunter_and_warlock_pet_damage_by_five_percent() {
    for name in [
        "production-beast-mastery-hunter",
        "affliction-warlock-lash-of-pain-rounding",
    ] {
        let mut orc = request(name);
        player(&mut orc)["race"] = "RaceOrc".into();
        let mut without = orc.clone();
        player(&mut without)["disableRacials"] = true.into();
        let with = pet_damage_multipliers(&prepared(&orc, Rules::Forever));
        let base = pet_damage_multipliers(&prepared(&without, Rules::Forever));
        assert!(!with.is_empty(), "{name}: no pets");
        for (with, base) in with.iter().zip(&base) {
            assert!(
                (with / base - 1.05).abs() < 1e-12,
                "{name}: {with} vs {base}"
            );
        }
        // The Go reference keeps the 16 October Orc, with no Command.
        let go = pet_damage_multipliers(&prepared(&orc, Rules::GoReference));
        let go_base = pet_damage_multipliers(&prepared(&without, Rules::GoReference));
        assert_eq!(go, go_base, "{name}");
    }
}

/// The first iteration's log lines of a Skyborne Fire Mage who receives an external Innervate
/// and reads Ley Lines when `condition` holds.
fn ley_line_log(rules: Rules, condition: Value) -> (Value, Vec<String>) {
    let mut request = request("fire-skyborne-read-ley-line");
    request["simOptions"]["debugFirstIteration"] = true.into();
    request["simOptions"]["iterations"] = 1.into();
    let player = player(&mut request);
    if rules == Rules::Forever {
        // The fixture's talents in client 1.60.1.70707's order: Arcane Focus's points on
        // Improved Channeling, which takes its square, and Critical Mass at its 3 ranks.
        player["talentsString"] = "230205-35520201030133031-003".into();
    }
    player["buffs"]["innervates"] = 1.into();
    for entry in player["rotation"]["priorityList"].as_array_mut().unwrap() {
        let action = &mut entry["action"];
        if action["castSpell"]["spellId"]["spellId"] == 1259705 {
            action["condition"] = condition.clone();
        }
    }
    let prepared = prepared(&request, rules);
    let typed: PreparedV2 = serde_json::from_value(prepared.clone()).unwrap();
    let report = simulate_prepared(&typed).unwrap();
    let logs = report.result["logs"].as_str().unwrap().to_string();
    (prepared, logs.lines().map(str::to_string).collect())
}

fn mana_below(percent: &str) -> Value {
    serde_json::json!({"cmp": {"lhs": {"currentManaPercent": {}}, "op": "OpLt",
        "rhs": {"const": {"val": percent}}}})
}

fn read_ley_line_effect(prepared: &Value) -> Value {
    prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == "read_ley_line")
        .expect("read_ley_line")
        .clone()
}

/// The time, in seconds, of the first line that names `id` and contains `event`.
fn first(lines: &[String], id: &str, event: &str) -> Option<f64> {
    lines
        .iter()
        .find(|line| line.contains(id) && line.contains(event))
        .map(|line| line[1..line.find(']').unwrap()].parse().unwrap())
}

const INNERVATE: &str = "{SpellID: 29166, Tag: -1}";
const ENERGIZED: &str = "{SpellID: 1270842}";
const READ_LEY_LINE: &str = "Casting {SpellID: 1259705}";

#[test]
fn read_ley_line_is_half_the_regen_under_forever_rules() {
    let (forever, _) = ley_line_log(Rules::Forever, mana_below("60%"));
    let effect = read_ley_line_effect(&forever);
    assert_eq!(effect["regen_multiplier"], 1.5);
    assert_eq!(effect["innervate_replaces"], true);
    let (go, _) = ley_line_log(Rules::GoReference, mana_below("60%"));
    let effect = read_ley_line_effect(&go);
    assert_eq!(effect["regen_multiplier"], 2.0);
    assert!(effect.get("innervate_replaces").is_none());
}

#[test]
fn an_innervate_replaces_energized() {
    // Read below 40% mana, Energized is up when the Innervate lands.
    let (_, lines) = ley_line_log(Rules::Forever, mana_below("40%"));
    let innervate = first(&lines, INNERVATE, "Aura gained").expect("the Innervate lands");
    assert!(first(&lines, ENERGIZED, "Aura gained").unwrap() < innervate);
    assert_eq!(first(&lines, ENERGIZED, "Aura faded"), Some(innervate));
    // The Go reference keeps both.
    let (_, lines) = ley_line_log(Rules::GoReference, mana_below("40%"));
    let innervate = first(&lines, INNERVATE, "Aura gained").unwrap();
    assert!(first(&lines, ENERGIZED, "Aura faded").unwrap() > innervate);
}

#[test]
fn read_ley_line_cannot_be_cast_during_an_innervate() {
    let during = serde_json::json!({"auraIsActive": {"auraId": {"spellId": 29166, "tag": -1}}});
    let (_, lines) = ley_line_log(Rules::Forever, during.clone());
    assert!(first(&lines, INNERVATE, "Aura gained").is_some());
    assert_eq!(first(&lines, READ_LEY_LINE, ""), None);
    let (_, lines) = ley_line_log(Rules::GoReference, during);
    assert!(first(&lines, READ_LEY_LINE, "").is_some());
}
