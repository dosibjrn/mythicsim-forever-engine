//! Rogue tests: the prepared v2 gate on the production Combat Rogue request. Its Go result and
//! first-fight log are compared with the rest of the fixture family.

use forever_engine::{
    check_prepared, contracts::prepared_v2::PreparedV2, prepared_coverage, PreparedError,
};
use serde_json::Value;
use std::{fs, path::Path};

mod prepare;

fn combat_json() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/production-combat-rogue.prepared.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn reasons(value: Value) -> Vec<String> {
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    match check_prepared(&prepared) {
        Err(PreparedError::Unsupported(reasons)) => reasons,
        other => panic!("expected unsupported, got {other:?}"),
    }
}

#[test]
fn production_combat_request_is_supported() {
    let prepared: PreparedV2 = serde_json::from_value(combat_json()).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
}

#[test]
fn poison_listener_needs_its_effect() {
    let mut value = combat_json();
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "deadly_poison");
    assert!(reasons(value).contains(
        &"player aura \"Deadly Poison\" listens to combat events without an effect".into()
    ));
}

/// A spell that costs energy needs the energy bar. Energy and combo point values without one
/// give no value, as Go's do, so they are not refused themselves.
#[test]
fn energy_values_need_an_energy_bar() {
    let mut value = combat_json();
    value["player"].as_object_mut().unwrap().remove("energy");
    assert!(crate::refusal_codes(value.clone())
        .iter()
        .any(|(code, reason)| *code == "class_limit"
            && reason.ends_with("which costs energy the player lacks")));
    let reasons = reasons(value);
    assert!(!reasons
        .iter()
        .any(|reason| reason.contains("reads energy or combo points")));
    assert!(reasons
        .iter()
        .any(|reason| reason.ends_with("which costs energy the player lacks")));
}

/// Once the sapper can hit the player, Chance of Death acts and needs its effect.
#[test]
fn chance_of_death_acts_once_the_player_can_take_damage() {
    let mut value = combat_json();
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "chance_of_death");
    assert!(reasons(value).contains(
        &"player aura \"Chance of Death\" listens to combat events without an effect".into()
    ));
}

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/mage/prepared-v2/{name}.prepared.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn production_assassination_and_subtlety_requests_are_supported() {
    for name in [
        "production-assassination-rogue",
        "production-subtlety-rogue",
    ] {
        let prepared: PreparedV2 = serde_json::from_value(fixture(name)).unwrap();
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new(), "{name}");
    }
}

/// Each talent proc trigger is claimed by its own effect.
#[test]
fn talent_proc_triggers_need_their_effects() {
    let mut value = fixture("production-subtlety-rogue");
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "rogue_proc");
    let reasons = reasons(value);
    for label in [
        "Initiative Trigger",
        "Cutthroat Trigger",
        "Thousand Cuts Trigger",
    ] {
        assert!(reasons.contains(&format!(
            "player aura \"{label}\" listens to combat events without an effect"
        )));
    }
}

/// Expose Armor sets the target's armor through the major armor category, unless a permanent
/// member blocks it for good.
#[test]
fn expose_armor_needs_the_armor_category() {
    for name in [
        "combat-expose-armor",
        "combat-improved-expose-armor",
        "combat-expose-armor-raid-expose",
    ] {
        let prepared: PreparedV2 = serde_json::from_value(fixture(name)).unwrap();
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new(), "{name}");
    }
    let mut value = fixture("combat-expose-armor");
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "exclusive_category");
    assert!(reasons(value)
        .iter()
        .any(|reason| reason.ends_with("without the target's armor category")));
}

/// Against a target that tanks the player, Kidney Shot's stun pauses its swings, Ghostly
/// Strike's dodge is a stat aura in its rolls and parries ready Riposte, whose trigger needs
/// its effect.
#[test]
fn tanking_rogue_handles_stuns_and_parries() {
    for name in [
        "combat-kidney-shot-tank",
        "combat-riposte-tank",
        "subtlety-ghostly-strike-tank",
    ] {
        let prepared: PreparedV2 = serde_json::from_value(fixture(name)).unwrap();
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new(), "{name}");
    }
    let mut value = fixture("combat-riposte-tank");
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "riposte");
    let reasons = reasons(value);
    assert!(
        reasons.contains(&"player aura \"Riposte Trigger\" reacts to the target's swings".into())
    );
}

/// The pinned engine registers neither Evasion nor Sprint, so a rotation or cooldown timing that
/// names them reaches nothing, and Vanish casts from the rotation while tanking.
#[test]
fn defensive_cooldowns_reach_only_vanish() {
    let value = fixture("combat-rogue-defensive-cooldowns-tank");
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
    let ids: Vec<i32> = prepared
        .player
        .spells
        .iter()
        .filter_map(|spell| spell.action_id.as_ref().map(|id| id.spell_id))
        .collect();
    assert!(ids.contains(&1856));
    assert!(!ids.contains(&5277) && !ids.contains(&11305));
}

fn against(spec: &str, targets: u32) -> Value {
    fixture(&format!("production-{spec}-rogue-{targets}-targets"))
}

fn first_fight_log(mut value: Value) -> String {
    value["sim"]["iterations"] = serde_json::json!(1);
    value["sim"]["debug_first_iteration"] = serde_json::json!(true);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    let report =
        forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
    report.result["logs"].as_str().unwrap().to_string()
}

/// Blade Flurry's extra hit lands on the target after the one each melee hit struck. Only the
/// first target takes melee hits, and the extra hit has no proc mask to strike again, so the
/// second target takes every one.
#[test]
fn blade_flurry_strikes_the_next_target() {
    let logs = first_fight_log(against("combat", 3));
    assert!(logs
        .lines()
        .any(|line| line.contains("[Target 2] {SpellID: 22482} Hit")));
    assert!(!logs
        .lines()
        .any(|line| line.contains("[Target 1] {SpellID: 22482}")));
}

/// The Goblin Sapper Charge and Dragonbreath Chili hit every target, though a Subtlety Rogue's
/// rotation gains no AoE lines.
#[test]
fn area_consumables_reach_every_target() {
    let logs = first_fight_log(against("subtlety", 5));
    for target in 1..=5 {
        for id in ["{ItemID: 10646}", "{SpellID: 15851}"] {
            assert!(
                logs.lines()
                    .any(|line| line.contains(&format!("[Target {target}] {id}"))),
                "Target {target}: no {id}"
            );
        }
    }
}

/// Thistle Tea is drunk on its own once all but `core.ThistleTeaSpill` (15, mythicsim patch
/// 102) of its flat 100 fits in the bar. With 10 allowed to spill a Combat Rogue waits for a
/// bar under 10 energy, which a 1 second global cooldown at 10 energy a second rarely leaves.
/// Pinned to the first fight of the production request: the first drink starts above 10.
#[test]
fn thistle_tea_may_spill_fifteen_energy() {
    let value = combat_json();
    let spill = value["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == "conjured_energy")
        .map(|effect| effect["spill"].as_f64().unwrap());
    assert_eq!(spill, Some(15.0));
    let logs = first_fight_log(value);
    let gained = logs
        .lines()
        .find(|line| line.contains("Gained 100.000 energy from {ItemID: 7676}"))
        .expect("the first fight drinks Thistle Tea");
    let before: f64 = gained
        .split("} (")
        .nth(1)
        .and_then(|rest| rest.split(" -->").next())
        .and_then(|energy| energy.parse().ok())
        .expect("the log states the energy before the drink");
    assert!(before > 10.0 && before <= 15.0, "{gained}");
}

/// Fork patch 127: three Battle Squawk stacks raise the melee speed 5% each from the reset, which
/// the exported melee state holds, and they fall off with the aura's four minutes, after the
/// fight. The aura holds its stacks from the reset, as the first-fight log shows.
#[test]
fn battle_squawks_hold_their_stacks_from_the_reset() {
    let value = fixture("combat-rogue-battle-squawks");
    let multiplier = value["melee"]["melee_speed_multiplier"].as_f64().unwrap();
    assert!(
        (multiplier - 1.05_f64.powi(3)).abs() < 1e-12,
        "{multiplier}"
    );
    let aura = value["player"]["auras"]
        .as_array()
        .unwrap()
        .iter()
        .find(|aura| aura["label"] == "Battle Squawk")
        .unwrap();
    assert_eq!(aura["stacks"], 3);
    assert_eq!(aura["duration_ns"], 240_000_000_000_u64);
    let logs = first_fight_log(value);
    assert!(logs.contains("[0.00] [combat-rogue (#1)] Aura gained: {SpellID: 23060}"));
    assert!(logs.contains("[0.00] [combat-rogue (#1)] {SpellID: 23060} stacks: 0 --> 3"));
    assert!(logs.contains("{SpellID: 23060} stacks: 3 --> 0"));
}

/// The production Combat Rogue run for `iterations`, its result.
fn combat_result(value: Value, iterations: u64) -> Value {
    let mut value = value;
    value["sim"]["iterations"] = serde_json::json!(iterations);
    value["sim"]["debug_first_iteration"] = serde_json::json!(false);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic)
        .unwrap()
        .result
}

/// The white swings of one hand (`OtherActionAttack` tag 1 or 2) against the target: the count
/// of one outcome field.
fn white_swings(result: &Value, tag: u64, field: &str) -> f64 {
    result["raidMetrics"]["parties"][0]["players"][0]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|action| {
            action["id"]["otherId"] == "OtherActionAttack" && action["id"]["tag"] == tag
        })
        .flat_map(|action| action["targets"].as_array().unwrap())
        .map(|target| target[field].as_f64().unwrap_or(0.0))
        .sum()
}

/// Go patch 135: a hand whose weapon skill moves the table reads its own miss, hit suppression,
/// dodge and glance; a table equal to the base one changes nothing, bit for bit.
#[test]
fn a_hand_reads_its_weapon_skill_table() {
    let base = combat_json();
    let base_table = |value: &Value| {
        let melee = &value["melee"];
        serde_json::json!({
            "skill": 1.0,
            "miss_chance": melee["base_miss_chance"],
            "hit_suppression": melee["hit_suppression"],
            "defender_dodge": melee["defender_dodge"],
            "glance_multiplier": melee["glance_multiplier"],
            "glance_spread": melee["glance_spread"],
        })
    };
    let mut same = base.clone();
    for hand in ["main_hand_skill", "off_hand_skill", "ranged_skill"] {
        same["melee"][hand] = base_table(&base);
    }
    let reference = combat_result(base.clone(), 20);
    let same = combat_result(same, 20);
    for part in ["raidMetrics", "encounterMetrics"] {
        assert_eq!(same[part], reference[part], "{part}");
    }

    // A main hand that cannot be dodged and whose glances keep all the damage, beside an off
    // hand on the base table.
    let mut moved = base.clone();
    let mut table = base_table(&base);
    table["defender_dodge"] = serde_json::json!(0.0);
    table["glance_multiplier"] = serde_json::json!(1.0);
    table["glance_spread"] = serde_json::json!(0.0);
    moved["melee"]["main_hand_skill"] = table;
    let result = combat_result(moved, 20);
    assert!(white_swings(&reference, 1, "dodges") > 0.0);
    assert_eq!(white_swings(&result, 1, "dodges"), 0.0);
    assert!(white_swings(&result, 2, "dodges") > 0.0);
    // A glance now deals what a hit would, so the main hand's glances average its hits'.
    let glances = white_swings(&result, 1, "glances");
    assert!(glances > 0.0);
    let glance = white_swings(&result, 1, "glanceDamage") / glances;
    let hit = (white_swings(&result, 1, "damage")
        - white_swings(&result, 1, "critDamage")
        - white_swings(&result, 1, "glanceDamage"))
        / white_swings(&result, 1, "hits");
    assert!(
        (glance / hit - 1.0).abs() < 0.05,
        "glances keep {glance}, hits {hit}"
    );
}

/// The game refuses a harmful spell on a friendly unit, so a Rupture line aimed at the player
/// never casts and the fight runs as it would without it. Go builds the line and panics on the
/// player's missing Rupture dot, the Subtlety Rogue sim of 25 October 2026 (2990f120), which the
/// gate used to refuse as a cast on the player.
#[test]
fn rupture_aimed_at_the_player_never_casts() {
    let base = fixture("production-subtlety-rogue");
    let mut aimed = base.clone();
    aimed["player"]["rotation"]["priorityList"]
        .as_array_mut()
        .unwrap()
        .insert(
            0,
            serde_json::json!({"action": {"castSpell": {
                "spellId": {"spellId": 11275}, "target": {"type": "Self"}}}}),
        );
    let prepared: PreparedV2 = serde_json::from_value(aimed.clone()).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
    let reference = combat_result(base, 20);
    let result = combat_result(aimed, 20);
    for part in ["raidMetrics", "encounterMetrics"] {
        assert_eq!(result[part], reference[part], "{part}");
    }
}
