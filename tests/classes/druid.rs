//! Druid prepared v2 regressions. The Go goldens of every accepted Druid case, including the
//! production Feral (cat) Druid, are compared with the rest of the fixture family in
//! `mage/prepared_v2.rs`; these tests cover what the gate rejects and what the cat's forms
//! log.

mod prepare;

use forever_engine::{
    check_prepared, contracts::prepared_v2::PreparedV2, simulate_prepared, PreparedError,
};
use serde_json::{json, Value};
use std::{fs, path::Path};

fn feral() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/production-feral-druid.prepared.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn parse(value: Value) -> PreparedV2 {
    serde_json::from_value(value).unwrap()
}

fn reasons(value: Value) -> Vec<String> {
    match check_prepared(&parse(value)) {
        Err(PreparedError::Unsupported(reasons)) => reasons,
        other => panic!("expected unsupported, got {other:?}"),
    }
}

fn effect<'a>(value: &'a mut Value, kind: &str) -> &'a mut Value {
    value["effects"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|effect| effect["kind"] == kind)
        .unwrap()
}

fn first_fight_log(mut value: Value) -> String {
    value["sim"]["iterations"] = json!(1);
    let report = simulate_prepared(&parse(value)).unwrap();
    report.result["logs"].as_str().unwrap().to_string()
}

#[test]
fn production_feral_druid_is_supported() {
    assert!(check_prepared(&parse(feral())).is_ok());
}

#[test]
fn a_starting_form_needs_its_form_effect() {
    let mut value = feral();
    effect(&mut value, "druid_forms")["starting_form"] = json!(["bear"]);
    assert!(reasons(value.clone())
        .contains(&"druid starting form [\"bear\"] is unsupported".to_string()));
    assert!(crate::refusal_codes(value).contains(&(
        "class_limit",
        "druid starting form [\"bear\"] is unsupported".into()
    )));
}

#[test]
fn faerie_fire_readings_beyond_own_and_never_are_rejected() {
    let mut value = feral();
    effect(&mut value, "faerie_fire")["refresh"] = json!(["unknown"]);
    assert!(reasons(value).contains(
        &"Faerie Fire's armor reduction reads [\"unknown\"], which is not modeled".into()
    ));
}

#[test]
fn aura_should_refresh_needs_a_supported_reading() {
    // A reading from the category the runtime keeps holds for an aura with a fixed bid.
    let mut value = feral();
    effect(&mut value, "aura_should_refresh")["modes"] = json!(["unknown"]);
    assert!(check_prepared(&parse(value)).is_ok());
    // An aura whose bid follows its stacks needs a class to describe the category.
    let mut value = feral();
    effect(&mut value, "aura_should_refresh")["modes"] = json!(["unknown"]);
    for aura in value["target"]["auras"].as_array_mut().unwrap() {
        if aura["label"] == "Faerie Fire (Player)" {
            aura["max_stacks"] = json!(2);
            aura["exclusive_memberships"][0]["priority"] = json!(0.0);
        }
    }
    let reasons = reasons(value);
    assert!(
        reasons.contains(
            &"auraShouldRefresh on target aura \"Faerie Fire (Player)\" has no supported exclusive effect reading"
                .to_string()
        ),
        "{reasons:?}"
    );
}

#[test]
fn blood_frenzy_on_other_outcomes_is_rejected() {
    let mut value = feral();
    effect(&mut value, "blood_frenzy")["outcome"] = json!(["Hit", "Crit"]);
    assert!(reasons(value).contains(&"Blood Frenzy procs on [\"Hit\", \"Crit\"]".to_string()));
}

#[test]
fn innervate_clears_the_form_and_cat_spells_then_fail_with_a_log() {
    // The first fight of the production fixture no longer casts Innervate, since its potion
    // leaves Cat Form alone (patch 111); this one casts it in form.
    let log = first_fight_log(fixture("feral-druid-mighty-rage-potion"));
    let innervate = log
        .find("Casting {SpellID: 29166}")
        .expect("the first fight casts Innervate");
    let before = &log[..innervate];
    assert!(before.ends_with(
        "Movement speed changed from 8.92 (27.50%) to 7.14 (2.00%)\n[86.72] [feral-druid (#1)] "
    ));
    assert!(log[innervate..].contains("[86.72] Failed cast to spell {SpellID: 9830}, wrong form"));
}

#[test]
fn threat_follows_the_form() {
    let log = first_fight_log(feral());
    // Faerie Fire's flat threat in Cat Form, and an auto attack's at Cat Form's 0.71.
    assert!(log.contains("{SpellID: 9907} Hit for 0.000 damage (SpellSchool: 8). (Threat: 85.200)"));
    assert!(log.contains("Hit for 143.563 damage (SpellSchool: 1). (Threat: 101.930)"));
}

fn bear() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/production-feral-bear-druid.prepared.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn production_feral_bear_druid_is_supported() {
    assert!(check_prepared(&parse(bear())).is_ok());
}

/// A queued Maul replaces one main hand swing and is gone: a Windfury Totem extra attack its own
/// hit procs is a white swing, so each queue is spent by one Maul and one rage cost (Ambiwar,
/// MythicSim Discord #bugs, 21 October 2026). The first fight had five queues spent twice before.
#[test]
fn a_queued_maul_is_spent_once() {
    let logs = first_fight_log(bear());
    let (gained, faded) = (
        "Aura gained: {SpellID: 9881, Tag: 1}",
        "Aura faded: {SpellID: 9881, Tag: 1}",
    );
    let mut queues = Vec::new();
    let mut open = None;
    for line in logs.lines() {
        if line.contains(gained) {
            open = Some(0);
        } else if line.contains(faded) {
            queues.extend(open.take());
        } else if let Some(casts) = open.as_mut() {
            if line.contains("Casting {SpellID: 9881} ") {
                *casts += 1;
            }
        }
    }
    assert!(queues.contains(&1));
    assert!(queues.iter().all(|&casts| casts <= 1), "{queues:?}");
}

#[test]
fn a_bear_rotation_may_leave_bear_form_and_shift_back() {
    // Innervate clears the form; the bear casts Bear Form again once out of it.
    let mut value = bear();
    let list = value["player"]["rotation"]["priorityList"]
        .as_array_mut()
        .unwrap();
    let not_bear = json!({"not": {"val": {"auraIsActive": {"auraId": {"spellId": 9634}}}}});
    list.insert(
        0,
        json!({"action": {"castSpell": {"spellId": {"spellId": 29166}}}}),
    );
    list.insert(
        0,
        json!({"action": {"castSpell": {"spellId": {"spellId": 9634}}, "condition": not_bear}}),
    );
    assert!(check_prepared(&parse(value.clone())).is_ok());
    let log = first_fight_log(value);
    let faded = log
        .find("Aura faded: {SpellID: 9634}")
        .expect("Innervate clears the form");
    let cast = log[faded..]
        .find("Casting {SpellID: 9634}")
        .expect("the bear shifts back");
    assert!(log[faded + cast..].contains("Aura gained: {SpellID: 9634}"));
}

#[test]
fn natural_reaction_on_other_outcomes_is_rejected() {
    let mut value = bear();
    effect(&mut value, "natural_reaction")["outcome"] = json!(["Parry"]);
    assert!(reasons(value).contains(&"Natural Reaction procs on [\"Parry\"]".to_string()));
}

#[test]
fn a_second_attack_power_aura_is_rejected() {
    let mut value = bear();
    let auras = value["enemy"]["attack_power_auras"].as_array_mut().unwrap();
    let copy = auras[0].clone();
    auras.push(copy);
    let reasons = reasons(value);
    assert!(
        reasons
            .iter()
            .any(|reason| reason.ends_with("change the target's attack power together")),
        "{reasons:?}"
    );
}

#[test]
fn demoralizing_roar_lowers_the_target_swing() {
    let log = first_fight_log(bear());
    let roar = log
        .find("[Target 1] Aura gained: {SpellID: 9898}")
        .expect("the first fight roars");
    let swing = "[Target 1] [feral-bear-druid (#1)] {OtherID: 3, Tag: 1} [DEBUG] MAP: 600.0";
    assert!(log[roar..].contains(swing));
    assert!(!log[..roar].contains("MAP: 600.0"));
}

/// The raid's permanent Demoralizing Roar or Shout debuff bids as much as the roar and outlasts
/// any refresh window, so Go's `ShouldRefreshExclusiveEffects` never asks for the roar and the
/// bear never casts it, however many targets it tanks. Without the debuff it roars at the pull.
#[test]
fn a_permanent_debuff_keeps_the_bear_from_roaring() {
    for suffix in ["", "-3-targets", "-5-targets"] {
        for tag in ["-over-shout-debuff", "-over-roar-debuff"] {
            let case = format!("feral-bear-druid-demoralizing-roar{suffix}{tag}");
            let log = first_fight_log(accepted(&case));
            assert!(
                !log.contains("Casting {SpellID: 9898}"),
                "{case}: the bear roared"
            );
        }
    }
    for case in [
        "production-feral-bear-druid",
        "production-feral-bear-druid-3-targets",
        "production-feral-bear-druid-5-targets",
    ] {
        let log = first_fight_log(accepted(case));
        assert!(log.contains("Casting {SpellID: 9898}"), "{case}: no roar");
    }
}

/// The roar's aura shares the Demoralizing category with the raid's debuffs, which the export
/// must describe for the gate to read the refresh.
#[test]
fn the_roars_refresh_needs_its_category() {
    let case = "feral-bear-druid-demoralizing-roar-over-shout-debuff";
    assert!(check_prepared(&parse(accepted(case))).is_ok());
    // Without the description the runtime derives the single aura category from what the
    // auras export, and plays the same fight.
    let mut value = accepted(case);
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "exclusive_category");
    assert!(check_prepared(&parse(value.clone())).is_ok());
    assert_eq!(first_fight_log(value), first_fight_log(accepted(case)));
    // A roar whose bid follows its stacks needs the description.
    let mut value = accepted(case);
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "exclusive_category");
    for aura in value["target"]["auras"].as_array_mut().unwrap() {
        if aura["label"] == "Demoralizing Roar (Player)" {
            aura["max_stacks"] = json!(3);
            aura["exclusive_memberships"][0]["priority"] = json!(0.0);
        }
    }
    let reasons = reasons(value);
    assert!(
        reasons.contains(
            &"auraShouldRefresh on target aura \"Demoralizing Roar (Player)\" has no supported exclusive effect reading"
                .to_string()
        ),
        "{reasons:?}"
    );
}

/// Go's rule reads the bids: a permanent debuff that bids less than the roar leaves the
/// category to the roar, which is then cast. No Go golden holds this, since every raid debuff
/// bids the roar's own cut; it checks the rule the runtime reads the category with.
#[test]
fn a_weaker_permanent_debuff_leaves_the_roar_to_be_cast() {
    let mut value = accepted("feral-bear-druid-demoralizing-roar-over-shout-debuff");
    for effect in value["effects"].as_array_mut().unwrap() {
        if effect["kind"] == "exclusive_category" {
            effect["members"][0]["priority"] = json!(100.0);
        }
    }
    for aura in value["target"]["auras"].as_array_mut().unwrap() {
        if aura["label"] == "Demoralizing Shout (External)" {
            aura["exclusive_memberships"][0]["priority"] = json!(100.0);
        }
    }
    let log = first_fight_log(value);
    assert!(
        log.contains("Casting {SpellID: 9898}"),
        "the bear did not roar"
    );
}

/// An aura that stacks weighs its bid by its stacks: in a category the export describes, the
/// runtime reads the live bid, as Go's rule does for Go's full stacks.
#[test]
fn the_roars_refresh_reads_a_stacking_aura_in_a_described_category() {
    let case = "feral-bear-druid-demoralizing-roar-over-roar-debuff";
    let mut value = accepted(case);
    for aura in value["target"]["auras"].as_array_mut().unwrap() {
        if aura["label"] == "Demoralizing Roar (Player)" {
            aura["max_stacks"] = json!(3);
        }
    }
    assert!(check_prepared(&parse(value)).is_ok());
}

/// A cat never learns Demoralizing Roar. Go drops a cast of a spell the player lacks before it
/// builds the cast's condition, so the cat plays on without it, with or without a raid debuff.
#[test]
fn a_cat_drops_the_roar_it_never_learns() {
    for suffix in ["", "-3-targets", "-5-targets"] {
        for tag in ["", "-over-shout-debuff", "-over-roar-debuff"] {
            let case = format!("feral-druid-demoralizing-roar{suffix}{tag}");
            assert!(check_prepared(&parse(accepted(&case))).is_ok(), "{case}");
            let log = first_fight_log(accepted(&case));
            assert!(
                !log.contains("Casting {SpellID: 9898}"),
                "{case}: the cat roared"
            );
        }
    }
}

#[test]
fn leaving_bear_form_at_the_end_keeps_the_health_fraction() {
    // A bear alive when the fight ends loses health with the form's maximum; a dead one has
    // none to lose.
    let mut alive = bear();
    alive["encounter"]["duration_ns"] = json!(4_000_000_000_i64);
    alive["encounter"]["duration_variation_ns"] = json!(0);
    let in_form = alive["player"]["stats"]["Health"].as_f64().unwrap();
    let after_form = |log: &str| {
        let faded = log
            .find("Aura faded: {SpellID: 9634}")
            .expect("the form fades");
        log[faded..].lines().nth(1).unwrap().to_string()
    };
    let line = after_form(&first_fight_log(alive));
    let total: f64 = line
        .rsplit("of ")
        .next()
        .and_then(|rest| rest.strip_suffix(" total."))
        .and_then(|number| number.parse().ok())
        .expect("a health line");
    assert!(line.contains("Spent ") && total < in_form, "{line}");
    let line = after_form(&first_fight_log(bear()));
    assert!(!line.contains(" health "), "{line}");
}

/// Wildheart Raiment's five piece Nature's Bounty hears only melee hits the player takes, so
/// it is inert on a cat nothing attacks; without the description it is refused.
#[test]
fn wildheart_raiment_is_inert_on_a_cat() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/feral-druid-wildheart-raiment.prepared.json");
    let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(check_prepared(&parse(value.clone())), Ok(()));
    let mut refused = value;
    refused["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["aura"] != "Wildheart Raiment 5P");
    assert!(reasons(refused).contains(
        &"player aura \"Wildheart Raiment 5P\" listens to combat events without an effect".into()
    ));
}

fn accepted(case: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/mage/prepared-v2/{case}.prepared.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// A tank's absorb proc hears the target's swings, so it needs its effect.
#[test]
fn absorb_procs_need_their_effect() {
    let mut value = accepted("feral-bear-druid-uthers-strength");
    assert!(check_prepared(&parse(value.clone())).is_ok());
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "spell_data_absorb_proc");
    assert!(reasons(value)
        .contains(&"player aura \"Uther's Strength\" reacts to the target's swings".into()));
}

/// Enchant Chest - Absorption's 25% chance waits out the trigger's 5 second cooldown.
#[test]
fn absorption_enchant_waits_for_its_cooldown() {
    let value = accepted("feral-bear-druid-absorption");
    // The shortest time between two shields in the first fight.
    let shortest_gap = |value: Value| {
        let times: Vec<f64> = first_fight_log(value)
            .lines()
            .filter(|line| line.ends_with("Aura gained: {SpellID: 1249073}"))
            .map(|line| line[1..line.find(']').unwrap()].parse().unwrap())
            .collect();
        times
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .fold(f64::INFINITY, f64::min)
    };
    let mut uncooled = value.clone();
    for aura in uncooled["player"]["auras"].as_array_mut().unwrap() {
        if aura["label"] == "Enchant Chest - Absorption" {
            aura["icd"] = Value::Null;
        }
    }
    assert!(shortest_gap(value) >= 5.0);
    assert!(shortest_gap(uncooled) < 5.0);
}

/// Essence of the Pure Flame's damage shield hits each tank's attacker on its landed swings,
/// for a fixed amount that cannot crit.
#[test]
fn damage_shield_hits_the_attacker_without_crits() {
    let value = accepted("feral-bear-druid-essence-of-the-pure-flame");
    let mut without = value.clone();
    without["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["trigger_aura"] != "Essence of the Pure Flame");
    assert!(reasons(without).contains(
        &"player aura \"Essence of the Pure Flame\" reacts to the target's swings".into()
    ));
    let log = first_fight_log(value);
    let hits: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("[Target 1] {SpellID: 23266} Hit for 13.650 damage"))
        .collect();
    assert!(!hits.is_empty());
    assert!(!log.contains("{SpellID: 23266} Crit"));
}

/// The Lion Horn of Stormwind hears the target's swings on a tank, and a batch window after
/// one lands raises armor, which the target's later swings read through the stat auras.
#[test]
fn lion_horn_raises_armor_after_a_swing() {
    let value = accepted("feral-bear-druid-lion-horn");
    let mut without = value.clone();
    without["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["trigger_aura"] != "The Lion Horn of Stormwind");
    assert!(reasons(without).contains(
        &"player aura \"The Lion Horn of Stormwind\" reacts to the target's swings".into()
    ));
    // A struck proc hears only hits taken.
    let mut misheard = value.clone();
    for effect in misheard["effects"].as_array_mut().unwrap() {
        if effect["trigger_aura"] == "The Lion Horn of Stormwind" {
            effect["callbacks"] = json!(["on_spell_hit_dealt"]);
        }
    }
    assert!(simulate_prepared(&parse(misheard)).is_err());
    let log = first_fight_log(value);
    let lines: Vec<&str> = log.lines().collect();
    let gain = lines
        .iter()
        .position(|line| line.ends_with("Aura gained: {SpellID: 18946}"))
        .unwrap();
    let time = |line: &str| -> f64 { line[1..line.find(']').unwrap()].parse().unwrap() };
    // The last swing before the gain landed one spell batch window earlier.
    let swing = lines[..gain]
        .iter()
        .rev()
        .find(|line| {
            line.contains("[Target 1] [feral-bear-druid (#1)] {OtherID: 3, Tag: 1}")
                && line.contains(" damage (SpellSchool: 1)")
        })
        .unwrap();
    assert!((time(lines[gain]) - time(swing) - 0.01).abs() < 1e-9);
}

const BOOMERANG_REFUSAL: &str =
    "rotation reaches item 11905, a hardcast while the target swings at the player";

/// A tank's hardcast with the pushback flag runs through the "Pushback trigger" the exporter
/// reads: without it, or with a chance that needs a roll, the gate still refuses it.
#[test]
fn a_tanking_pushback_hardcast_needs_the_trigger_and_a_certain_chance() {
    let value = accepted("feral-bear-druid-boomerang-pushback");
    assert_eq!(check_prepared(&parse(value.clone())), Ok(()));

    let mut untriggered = value.clone();
    untriggered["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "pushback_trigger");
    assert!(reasons(untriggered).contains(&BOOMERANG_REFUSAL.into()));

    let mut rolled = value.clone();
    effect(&mut rolled, "pushback_trigger")["chance"] = json!(0.65);
    assert!(reasons(rolled).contains(
        &"rotation reaches item 11905, a hardcast the target's swings push back with a chance \
          that needs a roll"
            .into()
    ));

    // A chance the cast's resist cancels never rolls and never pushes back.
    let mut resisted = value.clone();
    effect(&mut resisted, "pushback_trigger")["chance"] = json!(0.0);
    assert_eq!(check_prepared(&parse(resisted.clone())), Ok(()));
    assert!(!first_fight_log(resisted).contains("pushed back"));

    // A hardcast without the reduced avoidance rolls keeps its refusal.
    let mut unrolled = value;
    unrolled["enemy"]
        .as_object_mut()
        .unwrap()
        .remove("reduced_avoidance_rolls");
    assert!(reasons(unrolled).contains(&BOOMERANG_REFUSAL.into()));
}

/// Go pushes back by the time the cast has run, never more than half a second. Since the fork's
/// patch 89 a hit in the batch window before the cast completes leaves the finished cast alone,
/// so it completes once.
#[test]
fn a_hit_pushes_the_hardcast_back_by_the_time_it_has_run() {
    let log = first_fight_log(accepted("feral-bear-druid-boomerang-pushback"));
    assert!(log.contains("{ItemID: 11905} pushed back 475.418331ms while casting"));
    let log = first_fight_log(accepted("feral-bear-druid-boomerang-pushback-after-cast"));
    assert!(!log.contains("pushed back"));
    assert_eq!(log.matches("Completed cast {ItemID: 11905}").count(), 1);
}

/// The same accepted request against another number of copies of the boss, which Rust builds
/// from the count.
fn against_targets(case: &str, targets: u64) -> Value {
    let mut value = accepted(case);
    value["encounter"]["target_count"] = json!(targets);
    value
}

fn hit_lines(log: &str, target: u64, spell: i32) -> usize {
    let prefix = format!("[Target {target}] {{SpellID: {spell}}}");
    log.lines()
        .filter(|line| line.contains(&prefix) && !line.contains("[DEBUG]"))
        .count()
}

/// Demoralizing Roar rolls a hit on every target and debuffs each it lands on, and Swipe hits
/// the first three targets in unit index order, as Go demoralizing_roar.go and swipe.go do.
#[test]
fn roar_reaches_every_target_and_swipe_the_first_three() {
    let log = first_fight_log(against_targets("feral-bear-druid-swipe-3-targets", 5));
    for target in 1..=5 {
        assert!(
            hit_lines(&log, target, 9898) > 0,
            "Target {target}: no Roar"
        );
        assert!(
            log.contains(&format!("[Target {target}] Aura gained: {{SpellID: 9898}}")),
            "Target {target}: no Roar debuff"
        );
    }
    for target in 1..=3 {
        assert!(
            hit_lines(&log, target, 9908) > 0,
            "Target {target}: no Swipe"
        );
    }
    for target in 4..=5 {
        assert_eq!(
            hit_lines(&log, target, 9908),
            0,
            "Target {target}: Swipe reached it"
        );
    }
}

/// A Primal Bite under Berserk strikes up to three targets, calculating and dealing each hit
/// in turn, and lifts its cooldown, as Go primal_bite.go does.
#[test]
fn primal_bite_under_berserk_strikes_three_targets() {
    let log = first_fight_log(against_targets("feral-bear-druid-primal-bite-3-targets", 5));
    for target in 1..=3 {
        assert!(
            hit_lines(&log, target, 1238073) > 0,
            "Target {target}: no Primal Bite"
        );
    }
    for target in 4..=5 {
        assert_eq!(hit_lines(&log, target, 1238073), 0, "Target {target}");
    }
}

/// Hurricane's channel casts its tick spell every period, which hits each target in turn.
#[test]
fn hurricane_ticks_on_every_target() {
    let log = first_fight_log(against_targets("balance-druid-hurricane-3-targets", 5));
    for target in 1..=5 {
        assert!(
            hit_lines(&log, target, 1278759) > 0,
            "Target {target}: no Hurricane tick"
        );
    }
}

/// Hurricane and Swipe run only through their effects.
#[test]
fn hurricane_and_swipe_need_their_effects() {
    for (case, kind, spell) in [
        ("balance-druid-hurricane-3-targets", "hurricane", 17402),
        ("feral-bear-druid-swipe-3-targets", "swipe", 9908),
    ] {
        let mut value = accepted(case);
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

/// A copy that parries a Primal Bite has the swing timer its reset opened for its parry haste
/// to read, though it never swings: a few fights parry hundreds of hits on the copies.
#[test]
fn a_copy_that_parries_does_not_break_the_fight() {
    let mut value = accepted("feral-bear-druid-primal-bite-3-targets");
    value["sim"]["iterations"] = json!(50);
    let report = simulate_prepared(&parse(value)).unwrap();
    let parries: f64 = report.result["raidMetrics"]["parties"][0]["players"][0]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|action| action["id"]["spellId"] == 1238073)
        .flat_map(|action| action["targets"].as_array().unwrap())
        .filter(|target| target["unitIndex"] == 1 || target["unitIndex"] == 2)
        .map(|target| target["parries"].as_f64().unwrap_or(0.0))
        .sum();
    assert!(parries > 0.0, "no Primal Bite was parried on a copy");
}

/// A multidot line casts on the first target whose dot is down, so Moonfire and Insect Swarm
/// each end up on every target with their own aura, debuff and ticks, as Go does.
#[test]
fn multidot_puts_a_balance_dot_on_every_target() {
    let log = first_fight_log(accepted("balance-druid-multidot-3-targets"));
    for target in 1..=3 {
        for spell in ["{SpellID: 9835, Tag: 1}", "{SpellID: 24977}"] {
            let ticks = log
                .lines()
                .filter(|line| line.contains(&format!("[Target {target}] {spell} tick")))
                .count();
            assert!(ticks > 0, "Target {target}: no {spell} tick");
        }
    }
}

/// Each Rip keeps the amounts it snapshotted on its own target.
#[test]
fn multidot_puts_a_rip_on_another_target() {
    let log = first_fight_log(accepted("feral-druid-multidot-3-targets"));
    assert!(log.contains("[Target 2] {SpellID: 9896} tick"));
    assert!(log.contains("[Target 2] Aura gained: {SpellID: 9896}"));
}

/// Every copy of the boss swings at the bear, and each swing reads the Demoralizing Roar that
/// copy holds: its attack power falls with that copy's debuff, as Go reads it from the unit
/// that swings.
#[test]
fn each_copy_swings_with_the_roar_it_holds() {
    let log = first_fight_log(accepted("production-feral-bear-druid-3-targets"));
    for target in 1..=3 {
        let roar = log
            .find(&format!("[Target {target}] Aura gained: {{SpellID: 9898}}"))
            .unwrap_or_else(|| panic!("Target {target}: no Roar"));
        let swing = format!(
            "[Target {target}] [feral-bear-druid (#1)] {{OtherID: 3, Tag: 1}} [DEBUG] MAP: 600.0"
        );
        assert!(
            log[roar..].contains(&swing),
            "Target {target}: no swing under the Roar"
        );
        assert!(
            !log[..roar].contains(&swing),
            "Target {target}: a swing under a Roar it did not hold yet"
        );
        assert!(
            log.contains(&format!("[Target {target}] Casting {{OtherID: 3, Tag: 1}}")),
            "Target {target}: it does not swing"
        );
    }
}

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/mage/prepared-v2/{name}.prepared.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn logged(mut value: Value) -> String {
    value["sim"]["debug_first_iteration"] = json!(true);
    first_fight_log(value)
}

/// A potion no longer drops Bear Form (mythicsim patch 111): the bear drinks its Greater
/// Stoneshield Potion at 4 seconds and again at 124.52 and keeps the form through both. Giving
/// the potion back to the form-breaking spells, as the old exporter did, drops the form.
#[test]
fn a_potion_leaves_the_bear_in_form() {
    let value = fixture("feral-bear-druid-potion-shift");
    let drops_form = |logs: &str| {
        logs.lines().any(|line| {
            line.contains("Aura faded: {SpellID: 9634}")
                && line
                    .trim_start_matches('[')
                    .split(']')
                    .next()
                    .and_then(|time| time.parse::<f64>().ok())
                    .is_some_and(|time| time < 120.0)
        })
    };
    assert!(!drops_form(&logged(value.clone())));
    let mut old = value;
    let spells = old["player"]["spells"].as_array().unwrap();
    let index = spells
        .iter()
        .position(|spell| spell["action_id"]["item_id"] == 13455)
        .expect("the potion is a player spell");
    effect(&mut old, "bear_form")["form_breaking_spells"] = json!([index]);
    assert!(drops_form(&logged(old)));
}

/// Rake adds 5% of attack power to its hit and every tick (community #769), so its
/// projected tick falls without the share.
#[test]
fn rake_scales_with_attack_power() {
    let snapshot = |share: f64| -> f64 {
        let mut value = fixture("feral-druid-rake");
        effect(&mut value, "rake")["attack_power_share"] = json!(share);
        logged(value)
            .lines()
            .find_map(|line| line.split("Rake Snapshot Power: ").nth(1))
            .and_then(|power| power.trim().parse().ok())
            .expect("the first fight applies Rake")
    };
    let with = snapshot(0.05);
    let without = snapshot(0.0);
    assert!(with > without + 10.0, "{with} against {without}");
}

/// Idol of the Dream (220606): Improved Rip (446212, client 1.60.1.70532) adds 2 seconds to Rip,
/// with the same tick period, so Rip ticks once more; ported from wowsims/forever 8d879a401.
#[test]
fn idol_of_the_dream_extends_rip_by_two_seconds() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/production-feral-druid.request.json");
    let mut request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    // The fixture's talents in hotfix push 113531's order, which Forever's rules read.
    request["raid"]["parties"][0]["players"][0]["talentsString"] =
        json!("050022-55000032023012125001-055");
    let rip = |idol: Option<i64>| {
        let mut request = request.clone();
        let items = request["raid"]["parties"][0]["players"][0]["equipment"]["items"]
            .as_array_mut()
            .unwrap();
        if let Some(idol) = idol {
            let relic = items
                .iter_mut()
                .find(|item| item["id"] == json!(22397))
                .expect("the reference's Idol of Ferocity");
            *relic = json!({ "id": idol });
        }
        request["simOptions"]["iterations"] = json!(200);
        let prepared =
            forever_engine::prepare_json(&serde_json::to_vec(&request).unwrap(), "x").unwrap();
        let dot = prepared["player"]["spells"]
            .as_array()
            .unwrap()
            .iter()
            .find(|spell| spell["action_id"]["spell_id"] == json!(9896))
            .and_then(|spell| spell["dot"].as_object().cloned())
            .expect("Rip's dot");
        let flat = dot["base_duration_flat_ns"].as_i64().unwrap_or(0);
        let period = dot["base_tick_length_ns"].clone();
        let dps = simulate_prepared(&parse(prepared)).unwrap().result["raidMetrics"]["parties"][0]
            ["players"][0]["dps"]["avg"]
            .as_f64()
            .unwrap();
        (flat, period, dps)
    };
    let (flat, period, dps) = rip(None);
    let (idol_flat, idol_period, idol_dps) = rip(Some(220606));
    assert_eq!(idol_flat - flat, 2_000_000_000);
    assert_eq!(idol_period, period);
    assert!(idol_dps > dps, "{idol_dps} against {dps}");
}
