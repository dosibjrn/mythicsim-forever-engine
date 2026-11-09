//! Paladin tests: the prepared v2 gate on the production Retribution Paladin request. Its Go
//! result and first-fight log are compared with the rest of the fixture family.

use forever_engine::{
    check_prepared, contracts::prepared_v2::PreparedV2, prepared_coverage, PreparedError,
};
use serde_json::{json, Value};
use std::{fs, path::Path};

mod prepare;

fn retribution_json() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/production-retribution-paladin.prepared.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn reasons(value: Value) -> Vec<String> {
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    match check_prepared(&prepared) {
        Err(PreparedError::Unsupported(reasons)) => reasons,
        other => panic!("expected unsupported, got {other:?}"),
    }
}

fn shockadin_json() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/production-shockadin-paladin.prepared.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn tank_json(case: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/mage/prepared-v2/{case}.prepared.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn tanking_paladins_are_supported() {
    for case in [
        "protection-paladin-no-dynamite",
        "ret-protection-paladin-no-dynamite",
        "holy-protection-paladin-no-dynamite",
    ] {
        let prepared: PreparedV2 = serde_json::from_value(tank_json(case)).unwrap();
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new(), "{case}");
    }
}

/// A tank's hardcast is followed only through the reduced avoidance rolls.
#[test]
fn a_tanking_hardcast_needs_reduced_avoidance_combinations() {
    let mut value = tank_json("protection-paladin-no-dynamite");
    value["enemy"]
        .as_object_mut()
        .unwrap()
        .remove("reduced_avoidance_rolls");
    assert!(reasons(value).contains(
        &"rotation reaches spell 24239, a hardcast while the target swings at the player".into()
    ));
}

/// Templar's Bulwark fires below the defensive health threshold or from the rotation; its
/// absorb shield and Forbearance run only through its effect.
#[test]
fn templars_bulwark_is_supported_with_its_effect() {
    for case in [
        "protection-bulwark-threshold",
        "protection-bulwark-rotation",
    ] {
        let prepared: PreparedV2 = serde_json::from_value(tank_json(case)).unwrap();
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new(), "{case}");
        let mut value = tank_json(case);
        value["effects"]
            .as_array_mut()
            .unwrap()
            .retain(|effect| effect["kind"] != "templars_bulwark");
        assert!(
            reasons(value)
                .iter()
                .any(|reason| reason.contains("1311015")
                    && (reason.contains("without a known behavior")
                        || reason.contains("with a defensive health threshold is unsupported"))),
            "{case}"
        );
    }
}

/// Stoneform below the defensive health threshold changes the physical damage the target's
/// swings deal, which the runtime follows only through the live school multiplier.
#[test]
fn stoneform_below_a_health_threshold_needs_the_live_school_multiplier() {
    let prepared: PreparedV2 =
        serde_json::from_value(tank_json("protection-paladin-dwarf-stoneform")).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
    let mut value = tank_json("protection-paladin-dwarf-stoneform");
    assert_eq!(
        value["enemy"]["school_damage_taken_auras"],
        json!(["player:Stoneform"])
    );
    for rolls in value["enemy"]["rolls"].as_array_mut().unwrap() {
        rolls
            .as_object_mut()
            .unwrap()
            .remove("school_damage_taken_multiplier");
    }
    assert!(reasons(value).contains(
        &"player:Stoneform changes the player's physical damage taken, which the runtime does \
          not follow"
            .into()
    ));
}

/// A survival cooldown neither a class nor a racial effect describes is not followed below
/// the defensive health threshold.
#[test]
fn an_undescribed_survival_cooldown_with_a_health_threshold_is_unsupported() {
    let mut value = tank_json("protection-paladin-dwarf-stoneform");
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "stoneform");
    assert!(reasons(value)
        .iter()
        .any(|reason| reason.contains("with a defensive health threshold is unsupported")));
}

#[test]
fn production_shockadin_request_is_supported() {
    let prepared: PreparedV2 = serde_json::from_value(shockadin_json()).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
}

/// The Storm Gauntlets' proc is a listener only its item effect describes.
#[test]
fn storm_gauntlets_need_their_damage_proc() {
    let mut value = shockadin_json();
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "spell_data_damage_proc");
    assert!(reasons(value).contains(
        &"player aura \"Storm Gauntlets\" listens to combat events without an effect".into()
    ));
}

#[test]
fn holy_shock_and_divine_favor_need_their_effects() {
    for (kind, spell) in [("holy_shock", 20930), ("divine_favor", 20216)] {
        let mut value = shockadin_json();
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

#[test]
fn production_retribution_request_is_supported() {
    let prepared: PreparedV2 = serde_json::from_value(retribution_json()).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
}

/// Leader of the Pack and Moonkin Aura no longer share a category since the 16 October 2026 beta
/// notes: both stay up from the reset.
#[test]
fn leader_of_the_pack_and_moonkin_aura_stack() {
    let value = retribution_json();
    for label in ["Leader of the Pack (External)", "Moonkin Aura (External)"] {
        let aura = value["player"]["auras"]
            .as_array()
            .unwrap()
            .iter()
            .find(|aura| aura["label"] == label)
            .unwrap();
        assert_eq!(aura["active"], true, "{label}");
        assert_eq!(aura["displaced_by"], Value::Null, "{label}");
    }
}

#[test]
fn seal_listeners_need_their_talent_effects() {
    for kind in [
        "sanctified_judgement",
        "vengeance",
        "vindication",
        "sacred_arbiter",
        "twist_of_light",
    ] {
        let mut value = retribution_json();
        value["effects"]
            .as_array_mut()
            .unwrap()
            .retain(|effect| effect["kind"] != kind);
        assert!(
            reasons(value)
                .iter()
                .any(|reason| reason.contains("listens to combat events without an effect")),
            "{kind}"
        );
    }
}

#[test]
fn a_seal_without_its_effect_has_no_behavior() {
    let mut value = retribution_json();
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "seal_of_righteousness");
    assert!(
        reasons(value).contains(&"rotation reaches spell 20293 without a known behavior".into())
    );
}

/// The builds beyond the presets: Exorcism and Holy Wrath, a lower Holy Shield rank, Light's
/// Vigil, Seal of the Crusader, and the talents and libram the talent sweep rejected.
#[test]
fn broadened_paladin_builds_are_supported() {
    for case in [
        "ret-undead-exorcism",
        "ret-exorcism-humanoid",
        "protection-holy-shield-rank2",
        "shockadin-lights-vigil",
        "ret-crusader-opener",
        "ret-crusader-fervor-over-raid",
        "ret-crusader-held",
        "protection-eye-for-an-eye",
        "shockadin-infusion-of-light",
        "shockadin-holy-alacrity",
        "ret-pursuit-of-justice",
    ] {
        let prepared: PreparedV2 = serde_json::from_value(tank_json(case)).unwrap();
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new(), "{case}");
    }
}

/// Seal of the Crusader's attack power rides on the stat aura combinations of the ranks the
/// rotation names.
#[test]
fn seal_of_the_crusader_needs_its_stat_aura() {
    let mut value = tank_json("ret-crusader-held");
    for effect in value["effects"].as_array_mut().unwrap() {
        if effect["kind"] == "stat_auras" {
            effect["auras"]
                .as_array_mut()
                .unwrap()
                .retain(|aura| !aura.as_str().unwrap().starts_with("Seal of the Crusader"));
        }
    }
    assert!(reasons(value.clone()).contains(
        &"Seal of the Crusader 20308 without its stat aura combinations is unsupported".into()
    ));
    assert!(crate::refusal_codes(value).contains(&(
        "class_limit",
        "Seal of the Crusader 20308 without its stat aura combinations is unsupported".into()
    )));
}

/// Holy Light, Flash of Light and Holy Shock's heal on the target or the player, with
/// Illumination, Infusion of Light, Holy Alacrity and Blessing of Light, and the Goblin
/// Sapper Charge's hit on a paladin whose absorb shields are inactive or Templar's Bulwark.
#[test]
fn paladin_heals_are_supported() {
    for case in [
        "shockadin-holy-light-illumination",
        "shockadin-holy-light-self",
        "shockadin-flash-of-light-blessed",
        "shockadin-holy-shock-heal",
        "shockadin-holy-light-infusion",
        "protection-flash-of-light-self",
        "protection-paladin-sapper",
        "retribution-paladin-sapper",
        "protection-bulwark-sapper",
    ] {
        let prepared: PreparedV2 = serde_json::from_value(tank_json(case)).unwrap();
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new(), "{case}");
    }
}

/// A heal runs only through its effect, and Illumination's listener only through its own.
#[test]
fn heals_and_illumination_need_their_effects() {
    let mut value = tank_json("shockadin-holy-light-illumination");
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "paladin_heals");
    assert!(
        reasons(value).contains(&"rotation reaches spell 25292 without a known behavior".into())
    );
    let mut value = tank_json("shockadin-holy-light-illumination");
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "illumination");
    assert!(reasons(value)
        .iter()
        .any(|reason| reason.contains("listens to combat events without an effect")));
}

#[test]
fn holy_light_haste_and_eye_for_an_eye_need_their_effects() {
    for (case, kind) in [
        ("protection-eye-for-an-eye", "eye_for_an_eye"),
        ("shockadin-infusion-of-light", "holy_light_haste"),
        ("shockadin-holy-alacrity", "holy_light_haste"),
    ] {
        let mut value = tank_json(case);
        value["effects"]
            .as_array_mut()
            .unwrap()
            .retain(|effect| effect["kind"] != kind);
        assert!(
            reasons(value)
                .iter()
                .any(|reason| reason.contains("listens to combat events without an effect")),
            "{case}"
        );
    }
}

/// A class may activate an aura its effect carries without claiming it, so a swing-changing
/// aura any effect names is refused while the target swings at the player.
#[test]
fn a_carried_aura_that_changes_the_swing_is_refused() {
    let mut value = tank_json("tank-protection-paladin-autos");
    value["enemy"]["changing_auras"]
        .as_array_mut()
        .unwrap()
        .push(json!("player:Phantom Ward"));
    value["effects"].as_array_mut().unwrap().push(json!({
        "kind": "inert_listener",
        "unit": "pet",
        "aura": "Phantom Ward",
        "reason": "carried, not claimed"
    }));
    assert!(reasons(value)
        .contains(&"player:Phantom Ward changes the target's swings at the player".to_string()));
}

/// Seal of Fury on the tanks, with its shield, Improved Seal of Fury and Templar's Bulwark,
/// and twisted with Seal of Command through Twist of Light.
#[test]
fn seal_of_fury_is_supported() {
    for case in [
        "protection-paladin-seal-of-fury",
        "ret-protection-paladin-seal-of-fury",
        "holy-protection-paladin-seal-of-fury",
        "retribution-paladin-seal-of-fury-twist",
        "protection-paladin-seal-of-fury-bulwark-sapper",
    ] {
        let prepared: PreparedV2 = serde_json::from_value(tank_json(case)).unwrap();
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new(), "{case}");
    }
    let mut value = tank_json("protection-paladin-seal-of-fury");
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "seal_of_fury");
    assert!(
        reasons(value).contains(&"rotation reaches spell 20423 without a known behavior".into())
    );
}

/// Lay on Hands, which the upstream P5 Protection rotation casts below 10% health, runs only
/// through its effect.
#[test]
fn lay_on_hands_needs_its_effect() {
    let prepared: PreparedV2 =
        serde_json::from_value(tank_json("protection-paladin-lay-on-hands")).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
    let mut value = tank_json("protection-paladin-lay-on-hands");
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "lay_on_hands");
    assert!(
        reasons(value).contains(&"rotation reaches spell 10310 without a known behavior".into())
    );
}

fn first_fight_log(mut value: Value) -> String {
    value["sim"]["iterations"] = json!(1);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    let report =
        forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
    report.result["logs"].as_str().unwrap().to_string()
}

/// The same accepted request against another number of copies of the boss, which Rust builds
/// from the count.
fn against_targets(case: &str, targets: u64) -> Value {
    let mut value = tank_json(case);
    value["encounter"]["target_count"] = json!(targets);
    value
}

fn lines_for(log: &str, target: u64, spell: i32, what: &str) -> usize {
    let prefix = format!("[Target {target}] {{SpellID: {spell}}} {what}");
    log.lines().filter(|line| line.contains(&prefix)).count()
}

/// Consecration ticks on every target in unit index order, and only the first four targets
/// take its bonus and have Consecrated Ground marked, as Go consecration.go does.
#[test]
fn consecration_ticks_every_target_and_marks_the_first_four() {
    let log = first_fight_log(against_targets("shockadin-paladin-5-targets", 5));
    for target in 1..=5 {
        assert!(
            lines_for(&log, target, 20924, "tick") > 0,
            "Target {target}: no Consecration tick"
        );
    }
    for target in 1..=4 {
        assert!(
            log.contains(&format!(
                "[Target {target}] Aura gained: {{SpellID: 1310905}}"
            )),
            "Target {target}: not marked"
        );
    }
    assert!(!log.contains("[Target 5] Aura gained: {SpellID: 1310905}"));
}

/// Holy Wrath rolls each Undead or Demon target before any bolt lands, then deals the hits
/// together; any other target takes nothing, as Go holy_wrath.go does.
#[test]
fn holy_wrath_hits_every_undead_target() {
    let case = "retribution-paladin-holy-wrath-3-targets";
    let log = first_fight_log(tank_json(case));
    for target in 1..=3 {
        let hits = ["Hit", "Crit"]
            .iter()
            .map(|what| lines_for(&log, target, 10318, what))
            .sum::<usize>();
        assert!(hits > 0, "Target {target}: no Holy Wrath hit");
    }
    let mut living = tank_json(case);
    living["target"]["mob_type"] = Value::Null;
    let log = first_fight_log(living);
    assert!(log.contains("Casting {SpellID: 10318}"));
    assert!(!log.contains("] {SpellID: 10318} Hit"));
}

/// Consecration is the paladin's area dot, which a spell's dot lookup does not return, so a
/// multidot line for it is dropped as in Go and the fight is the one without the line.
#[test]
fn a_multidot_line_for_consecration_is_dropped() {
    let with_line = first_fight_log(tank_json(
        "retribution-paladin-multidot-consecration-3-targets",
    ));
    let without = first_fight_log(tank_json("retribution-paladin-3-targets"));
    assert_eq!(with_line, without);
}

/// Every copy of the boss swings at a tank in Go, each on its own timer, and Holy Shield's
/// damage answers the copy whose swing was blocked: the proc is cast at the spell's unit.
#[test]
fn every_copy_swings_at_the_tank_and_holy_shield_answers_the_one_that_swung() {
    let log = first_fight_log(tank_json("production-protection-paladin-3-targets"));
    for target in 1..=3 {
        assert!(
            log.contains(&format!("[Target {target}] Casting {{OtherID: 3, Tag: 1}}")),
            "Target {target}: it does not swing"
        );
        assert!(
            log.contains(&format!(
                "] [Target {target}] {{SpellID: 20928, Tag: 2}} Hit for"
            )),
            "Target {target}: Holy Shield's damage never reached it"
        );
    }
}

/// Sulfuras' Immolation hits back whichever copy of the boss landed the melee hit.
#[test]
fn immolation_hits_the_copy_that_landed_the_hit() {
    let log = first_fight_log(tank_json("protection-paladin-sulfuras-3-targets"));
    for target in 1..=3 {
        assert!(
            log.contains(&format!("] [Target {target}] {{SpellID: 21142}} Hit for")),
            "Target {target}: no Immolation"
        );
    }
}

/// Eye for an Eye answers the unit whose spell made the crit, as Go casts the reflection at
/// `spell.Unit`: the half of the Goblin Sapper Charge that hits the paladin is the paladin's
/// own spell, so a crit of it is reflected onto the paladin itself.
#[test]
fn eye_for_an_eye_reflects_a_sapper_crit_onto_the_paladin() {
    let prepared: PreparedV2 =
        serde_json::from_value(tank_json("protection-eye-for-an-eye-sapper")).unwrap();
    let report =
        forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
    let paladin = &report.result["raidMetrics"]["parties"][0]["players"][0];
    let reflection = paladin["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["id"]["spellId"] == 9799)
        .expect("Eye for an Eye reflects");
    // The targets are the boss, then the paladin.
    assert!(reflection["targets"][0]["hits"].as_f64().unwrap() > 0.0);
    assert!(
        reflection["targets"][1]["hits"].as_f64().unwrap() > 0.0,
        "no reflection landed on the paladin"
    );
}

/// Eye for an Eye reflects a share of a crit taken onto the copy that dealt it.
#[test]
fn eye_for_an_eye_reflects_onto_the_copy_that_crit() {
    let log = first_fight_log(tank_json("protection-eye-for-an-eye-3-targets"));
    for target in 1..=3 {
        assert!(
            log.contains(&format!("] [Target {target}] {{SpellID: 9799}} Hit for")),
            "Target {target}: no reflection"
        );
    }
}

/// The seconds a log line was stamped at.
fn stamp(line: &str) -> f64 {
    line[1..line.find(']').unwrap()].parse().unwrap()
}

/// A completed hard cast restarts the melee swing timer in Forever (Go `HoldMeleeForCast`, beta
/// logs 2701 and 2712): the next main hand swing comes a full weapon speed after the cast ends,
/// even when one came due during the cast. Before the rule that swing landed as the cast
/// completed, so a Holy Light chain of 2.5 seconds each ended with a swing at once. Holy Light,
/// Holy Wrath and Light's Vigil hold the swing; Hammer of Wrath does too, unless Instrument of
/// Law has made it instant.
#[test]
fn a_completed_hard_cast_restarts_the_swing_a_full_weapon_speed_later() {
    for (case, spell) in [
        ("shockadin-holy-light-self", 25292),
        ("retribution-paladin-holy-wrath-3-targets", 10318),
        ("shockadin-lights-vigil", 1311595),
    ] {
        let value = tank_json(case);
        let speed = value["melee"]["main_hand"]["swing_speed"].as_f64().unwrap();
        let log = first_fight_log(value);
        let swings: Vec<f64> = log
            .lines()
            .filter(|line| line.contains("Completed cast {OtherID: 3, Tag: 1}"))
            .map(stamp)
            .collect();
        let ends: Vec<f64> = log
            .lines()
            .filter(|line| line.contains(&format!("Completed cast {{SpellID: {spell}}}")))
            .map(stamp)
            .collect();
        assert!(!ends.is_empty(), "{case}: no cast of {spell}");
        let mut later = 0;
        for end in ends {
            if let Some(swing) = swings.iter().find(|&&swing| swing > end + 0.001) {
                later += 1;
                assert!(
                    swing - end >= speed - 0.015,
                    "{case}: a swing at {swing} follows the cast that ended at {end}"
                );
            }
        }
        assert!(later > 0, "{case}: no swing follows a cast");
    }
}
