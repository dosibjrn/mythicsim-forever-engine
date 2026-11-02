//! Hunter prepared v2 regressions. The Go goldens of the production Hunters are
//! compared with the rest of the fixture family in `mage/prepared_v2.rs`; these tests pin the
//! ranged auto attack, the ranged cast time, physical ticks, the pets and what the gate rejects.

mod prepare;

use forever_engine::{
    check_prepared, contracts::prepared_v2::PreparedV2, simulate_prepared, PreparedError,
};
use serde_json::{json, Value};
use std::{fs, path::Path};

fn production() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/production-marksmanship-hunter.prepared.json");
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

/// The first fight's log of a short run.
fn first_fight_log(mut value: Value) -> String {
    value["sim"]["iterations"] = json!(1);
    value["sim"]["debug_first_iteration"] = json!(true);
    let report = simulate_prepared(&parse(value)).unwrap();
    report.result["logs"].as_str().unwrap().to_string()
}

fn player_line<'a>(logs: &'a str, at: &str, text: &str) -> Option<&'a str> {
    logs.lines().find(|line| {
        line.starts_with(at) && line.contains("[marksmanship-hunter (#1)]") && line.contains(text)
    })
}

#[test]
fn production_marksmanship_hunter_is_supported() {
    assert!(check_prepared(&parse(production())).is_ok());
}

/// Go attack.go: the first Auto Shot fires at the pull and lands after travel, 12 yards at
/// missile speed 40.
#[test]
fn auto_shot_fires_at_the_pull_and_lands_after_travel() {
    let logs = first_fight_log(production());
    assert!(
        player_line(&logs, "[0.00]", "Casting {OtherID: 4}").is_some(),
        "{logs}"
    );
    assert!(
        player_line(&logs, "[0.30]", "{OtherID: 4} Hit for").is_some()
            || player_line(&logs, "[0.30]", "{OtherID: 4} Crit for").is_some()
            || player_line(&logs, "[0.30]", "{OtherID: 4} Miss").is_some(),
        "{logs}"
    );
}

/// A ranged weapon out of range never starts its swing.
#[test]
fn a_ranged_weapon_out_of_range_never_shoots() {
    let mut value = production();
    value["melee"]["ranged"]["max_range"] = json!(10.0);
    let logs = first_fight_log(value);
    assert!(!logs.contains("{OtherID: 4}"), "{logs}");
}

/// Go hunter.go RegisterRangedSpell: Aimed Shot's two second cast divides by the ranged haste
/// multiplier, the quiver's 15%, unrounded.
#[test]
fn aimed_shot_casts_over_the_ranged_hasted_time() {
    let logs = first_fight_log(production());
    let line =
        player_line(&logs, "[-2.50]", "Casting {SpellID: 20904}").expect("prepull Aimed Shot");
    assert!(line.contains("Cast Time = 1.739130434s"), "{line}");
}

/// Go spell_resistances.go: a hawk's physical tick ignores armor.
#[test]
fn a_hawk_tick_ignores_armor() {
    let logs = first_fight_log(production());
    let line = logs
        .lines()
        .find(|line| line.contains("{SpellID: 1293527, Tag: 1} [DEBUG]"))
        .expect("a hawk tick");
    let stage = |name: &str| {
        let start = line.find(name).unwrap() + name.len();
        line[start..].split(',').next().unwrap().to_string()
    };
    assert_eq!(
        stage("AfterAttackerMods:"),
        stage("AfterResistances:"),
        "{line}"
    );
}

#[test]
fn an_unknown_auto_attack_type_is_rejected() {
    let mut value = production();
    let list = value["player"]["rotation"]["priorityList"]
        .as_array_mut()
        .unwrap();
    list[1]["action"]["condition"]["and"]["vals"][0]["cmp"]["lhs"]["autoTimeToNext"]["autoType"] =
        json!("SomeAuto");
    let reasons = reasons(value);
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains("autoTimeToNext autoType")),
        "{reasons:?}"
    );
}

#[test]
fn a_magic_hit_sting_tick_is_rejected() {
    let mut value = production();
    for effect in value["effects"].as_array_mut().unwrap() {
        if effect["kind"] == "serpent_sting" {
            effect["tick_outcome"] = json!("magic_hit");
        }
    }
    assert_eq!(reasons(value), ["Serpent Sting ticks with magic_hit"]);
}

#[test]
fn a_replaceable_main_hand_swing_in_range_is_rejected() {
    let mut value = production();
    value["unrepresented"] = json!(["main hand swings can be replaced"]);
    assert_eq!(
        reasons(value),
        ["unrepresented by the exporter: main hand swings can be replaced"]
    );
}

/// In melee range the main hand swings, Go's Raptor Strike replacement keeping each swing
/// while the rotation never queues one, and the ranged weapon's minimum range stops Auto Shot.
#[test]
fn in_melee_range_the_main_hand_swings_instead_of_auto_shot() {
    let mut value = production();
    value["player"]["distance_yards"] = json!(5.0);
    assert!(check_prepared(&parse(value.clone())).is_ok());
    let logs = first_fight_log(value);
    assert!(logs.contains("Casting {OtherID: 3, Tag: 1}"), "{logs}");
    assert!(!logs.contains("Casting {OtherID: 4}"), "{logs}");
}

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/mage/prepared-v2/{name}.prepared.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// The lines of one unit's log, by its label.
fn unit_lines<'a>(logs: &'a str, unit: &str) -> Vec<&'a str> {
    let tag = format!("[{unit}]");
    logs.lines().filter(|line| line.contains(&tag)).collect()
}

/// A log line's timestamp in seconds.
fn at(line: &str) -> f64 {
    line[1..line.find(']').unwrap()].parse().unwrap()
}

const WHELP: &str = "survival-hunter (#1) - Emerald Dragon Whelp";

#[test]
fn production_pet_and_melee_hunters_are_supported() {
    for name in [
        "production-beast-mastery-hunter",
        "production-survival-hunter",
        "survival-hunter-no-whelp",
    ] {
        assert!(check_prepared(&parse(fixture(name))).is_ok(), "{name}");
    }
}

/// Go pet.go: the Cat starts at its owner's 12 yards and moves in, one yard a stack.
#[test]
fn the_cat_moves_in_from_its_owners_distance() {
    let logs = first_fight_log(fixture("production-beast-mastery-hunter"));
    let cat = unit_lines(&logs, "hunter (#1) - Cat");
    assert!(
        cat.iter()
            .any(|line| line.starts_with("[0.00]") && line.contains("stacks: 0 --> 12")),
        "{logs}"
    );
}

/// Go emerald_dragon_whelp.go: each reset dismisses the whelp; Dragon's Call summons it,
/// it hardcasts Acid Spit, and its timeout dismisses it 15 seconds later.
#[test]
fn dragons_call_summons_the_whelp_for_fifteen_seconds() {
    let logs = first_fight_log(fixture("production-survival-hunter"));
    let whelp = unit_lines(&logs, WHELP);
    assert!(whelp[0].starts_with("[0.00]") && whelp[0].ends_with("Pet dismissed"));
    let summoned = whelp
        .iter()
        .position(|line| line.ends_with("Pet summoned"))
        .expect("Dragon's Call procs in the first fight");
    let dismissed = whelp[summoned..]
        .iter()
        .find(|line| line.ends_with("Pet dismissed"))
        .expect("the timeout dismisses the whelp");
    assert!(
        (at(dismissed) - at(whelp[summoned]) - 15.0).abs() < 1e-9,
        "{logs}"
    );
    assert!(
        whelp[summoned..]
            .iter()
            .any(|line| line.contains("Casting {SpellID: 9591} (Cost = 90.000, Cast Time = 3s")),
        "{logs}"
    );
}

/// The whelp skips a spit it would not live to finish: with a two second summon it only
/// swings.
#[test]
fn the_whelp_skips_a_spit_it_would_not_finish() {
    let mut value = fixture("production-survival-hunter");
    for effect in value["effects"].as_array_mut().unwrap() {
        if effect["kind"] == "emerald_dragon_whelp" {
            effect["duration_ns"] = json!(2_000_000_000i64);
        }
    }
    let logs = first_fight_log(value);
    let whelp = unit_lines(&logs, WHELP);
    assert!(
        whelp.iter().any(|line| line.ends_with("Pet summoned")),
        "{logs}"
    );
    assert!(whelp
        .iter()
        .any(|line| line.contains("Casting {OtherID: 3, Tag: 1}")));
    assert!(
        !whelp.iter().any(|line| line.contains("{SpellID: 9591}")),
        "{logs}"
    );
}

/// Dragon's Call needs the simulated pet to be the guardian it summons.
#[test]
fn dragons_call_without_its_summoned_guardian_is_refused() {
    let mut value = fixture("production-survival-hunter");
    value["pets"][0]["summoned"] = json!(false);
    let error = simulate_prepared(&parse(value)).unwrap_err().to_string();
    assert!(error.contains("is not a simulated summoned pet"), "{error}");
}

/// Go pet.go ExecuteCustomRotation: once less of the fight remains than the pet's uptime
/// allows, its rotation disables it; it acts no more, and later evaluations only log so.
#[test]
fn a_pet_past_its_uptime_is_disabled() {
    let logs = first_fight_log(fixture("beast-mastery-hunter-pet-uptime-half"));
    let end = logs
        .lines()
        .filter_map(|line| line.get(1..).and_then(|rest| rest.split(']').next()))
        .filter_map(|t| t.parse::<f64>().ok())
        .fold(0.0, f64::max);
    let cat = unit_lines(&logs, "hunter (#1) - Cat");
    let dismissed = cat
        .iter()
        .position(|line| line.ends_with("Pet dismissed"))
        .expect("the Cat is disabled");
    let at_dismissal = at(cat[dismissed]);
    // The rotation finds the uptime spent at its next evaluation, which waits on the pet's GCD
    // or focus.
    assert!(
        at_dismissal > end / 2.0 && at_dismissal < end / 2.0 + 2.0,
        "{at_dismissal} of {end}"
    );
    assert!(
        !cat[dismissed..].iter().any(|line| line.contains("Casting")),
        "{logs}"
    );
    assert!(cat[dismissed + 1..]
        .iter()
        .any(|line| line.ends_with("No pet summoned")));
}

/// Go pet_abilities.go newScorpidPoison (community #694): a landed poison on a poisoned target
/// refreshes the dot and adds a stack up to five, and once the stacks are full the Scorpid's
/// rotation spends its focus on Claw. Before, Apply dropped the stack, so each cast was one
/// stack again and the rotation never reached Claw.
#[test]
fn scorpid_poison_stacks_to_five() {
    let logs = first_fight_log(fixture("beast-mastery-hunter-scorpid"));
    assert!(logs.contains("{SpellID: 24587} stacks: 0 --> 1"), "{logs}");
    assert!(logs.contains("{SpellID: 24587} stacks: 4 --> 5"), "{logs}");
    assert!(!logs.contains("{SpellID: 24587} stacks: 5 --> 6"), "{logs}");
    assert!(logs.contains("Casting {SpellID: 3009}"), "{logs}");
}

/// The Tallstrider fixture with nothing holding the target's major armor reduction category:
/// no permanent Expose Armor and no Sunder Armor ramp.
fn tallstrider_without_armor_reductions() -> Value {
    let mut value = fixture("beast-mastery-hunter-tallstrider");
    for aura in value["target"]["auras"].as_array_mut().unwrap() {
        if aura["tag"] == "MajorArmorReduction" {
            aura["active"] = json!(false);
        }
    }
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "sunder_armor_ramp");
    value
}

/// Go newDustCloud: the pet casts it only while its target aura is down (and, since patch 110,
/// no Sunder Armor or Expose Armor holds the major armor reduction category).
#[test]
fn dust_cloud_is_cast_again_only_once_it_falls_off() {
    let logs = first_fight_log(tallstrider_without_armor_reductions());
    let events: Vec<&str> = logs
        .lines()
        .filter(|line| {
            line.contains("Casting {SpellID: 1265904}")
                || line.contains("Aura gained: {SpellID: 1265904}")
                || line.contains("Aura faded: {SpellID: 1265904}")
        })
        .collect();
    assert!(events.len() > 3, "{logs}");
    let mut active = false;
    for line in events {
        if line.contains("Casting") {
            assert!(!active, "{line}");
        } else {
            active = line.contains("gained");
        }
    }
}

/// Go talents_marksmanship.go registerRapidRecuperation: a landed Serpent Sting grants the
/// casting regeneration a spell batch window later.
#[test]
fn rapid_recuperation_follows_a_landed_serpent_sting() {
    let logs = first_fight_log(fixture("marksmanship-hunter-rapid-recuperation"));
    let sting = logs
        .lines()
        .find(|line| line.contains("{SpellID: 25295} Hit for"))
        .expect("Serpent Sting lands");
    let gained = logs
        .lines()
        .find(|line| line.contains("Aura gained: {SpellID: 1242512}"))
        .expect("Rapid Recuperation procs");
    assert!((at(gained) - at(sting) - 0.01).abs() < 1e-9, "{logs}");
}

/// Go arcane_shot.go: an Arcane school hit on the ranged table that lands after travel.
#[test]
fn arcane_shot_lands_after_travel() {
    let logs = first_fight_log(fixture("marksmanship-hunter-arcane-shot"));
    let cast = logs
        .lines()
        .find(|line| line.contains("Casting {SpellID: 14287}"))
        .expect("Arcane Shot is cast");
    let hit = logs
        .lines()
        .find(|line| {
            line.contains("{SpellID: 14287} Hit for") || line.contains("{SpellID: 14287} Crit for")
        })
        .expect("Arcane Shot hits");
    assert!(hit.contains("(SpellSchool: 64)"), "{hit}");
    assert!(at(hit) > at(cast), "{cast} {hit}");
}

/// Go newSwipe: its cast condition needs three active targets, so a Bear never swipes one.
#[test]
fn a_bear_never_swipes_one_target() {
    let logs = first_fight_log(fixture("beast-mastery-hunter-bear"));
    assert!(logs.contains("Casting {SpellID: 3009}"), "{logs}");
    assert!(!logs.contains("Casting {SpellID: 1264502}"), "{logs}");
}

/// Go items.go Renataki's Charm of Beasts: once the prepull Aimed Shot is cooling, the charm
/// resets it and Aimed Shot is cast again at once.
#[test]
fn renatakis_charm_resets_the_aimed_shot_cooldown() {
    let logs = first_fight_log(fixture("marksmanship-hunter-predators-armor"));
    let lines: Vec<&str> = logs.lines().collect();
    let charm = lines
        .iter()
        .position(|line| line.contains("Major cooldown used: {ItemID: 19953}"))
        .expect("the charm is used");
    assert!(
        lines[charm + 1].contains("Casting {SpellID: 20904}")
            && at(lines[charm + 1]) == at(lines[charm]),
        "{logs}"
    );
}

/// Go item_sets.go Cryptstalker Armor (6): a ranged crit restores 50 mana a spell batch window
/// later, and the set bonus trackers it exposes to the APL list no aura metrics.
#[test]
fn cryptstalker_ranged_crits_restore_mana() {
    let value = fixture("marksmanship-hunter-cryptstalker-armor");
    let logs = first_fight_log(value.clone());
    let lines: Vec<&str> = logs.lines().collect();
    let gained = lines
        .iter()
        .position(|line| line.contains("Gained 50.000 mana from {SpellID: 28753}"))
        .expect("a ranged crit restores mana");
    let crit = lines[..gained]
        .iter()
        .rev()
        .find(|line| line.contains("{OtherID: 4} Crit for"))
        .expect("a ranged crit before it");
    assert!((at(lines[gained]) - at(crit) - 0.01).abs() < 1e-9, "{logs}");
    let report = simulate_prepared(&parse(value)).unwrap();
    let auras = report.result["raidMetrics"]["parties"][0]["players"][0]["auras"].to_string();
    assert!(!auras.contains("28755"), "{auras}");
}

/// Two simulated pets: the Cat enabled at each reset and Dragon's Call's whelp summoned
/// mid-fight each swing as their own unit, and each reports its own metrics.
#[test]
fn a_cat_and_the_whelp_act_as_two_units() {
    let value = fixture("survival-hunter-cat-and-whelp");
    let logs = first_fight_log(value.clone());
    for unit in ["survival-hunter (#1) - Cat", WHELP] {
        assert!(
            unit_lines(&logs, unit)
                .iter()
                .any(|line| line.contains("Casting {OtherID: 3, Tag: 1}")),
            "{unit} swings: {logs}"
        );
    }
    let report = simulate_prepared(&parse(value)).unwrap();
    let pets = &report.result["raidMetrics"]["parties"][0]["players"][0]["pets"];
    let names: Vec<&str> = pets
        .as_array()
        .unwrap()
        .iter()
        .map(|pet| pet["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Cat", "Emerald Dragon Whelp"]);
}

fn survival_against(targets: u32) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "fixtures/mage/prepared-v2/production-survival-hunter-{targets}-targets.prepared.json"
    ));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// Against several targets the application's lines reach Explosive Trap and Volley, which
/// need the effects that describe them.
#[test]
fn area_spells_need_their_effects_against_several_targets() {
    for (kind, spell) in [("explosive_trap", 14317), ("volley", 14295)] {
        let mut value = survival_against(2);
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

/// Explosive Trap hits each target in turn, then its burn ticks on every target, and Volley's
/// channel ticks on every target, as Go traps.go and volley.go deal them.
#[test]
fn explosive_trap_and_volley_reach_every_target() {
    let logs = first_fight_log(survival_against(5));
    for target in 1..=5 {
        for (spell, what) in [(14317, " Hit"), (14317, " tick "), (14295, " tick ")] {
            let found = logs.lines().any(|line| {
                line.contains(&format!("[Target {target}] {{SpellID: {spell}}}"))
                    && line.contains(what)
            });
            assert!(found, "Target {target}: no {spell}{what}");
        }
    }
}

/// A Serpent Sting tick outcome the runtime does not implement is a Hunter class limit.
#[test]
fn an_unknown_serpent_sting_tick_is_a_class_limit() {
    let mut value = production();
    for effect in value["effects"].as_array_mut().unwrap() {
        if effect["kind"] == "serpent_sting" {
            effect["tick_outcome"] = json!("magic_hit");
        }
    }
    assert!(crate::refusal_codes(value)
        .contains(&("class_limit", "Serpent Sting ticks with magic_hit".into())));
}

/// Go lacerating_strikes.go (patches 87 and 132, `TestLaceratingStrikesRefreshRollsOver`): a
/// Mongoose Bite on a running bleed rolls what the bleed still owes into the new one and keeps
/// its tick timer, as Forever's Deep Wounds does. The 21 seconds start over from the bite, the
/// next tick lands when it was due, and the owed damage joins the bite's 40%, spread over the
/// ticks that fit before the bleed runs out; a bleed fades only once every tick has landed. The
/// bleed reports under its own id, 1310536, not Mongoose Bite's with a tag.
#[test]
fn a_refreshed_lacerating_strikes_bleed_rolls_over_and_keeps_its_tick_timer() {
    const BLEED: &str = "{SpellID: 1310536}";
    let logs = first_fight_log(fixture("survival-hunter-no-whelp"));
    let amount = |line: &str| -> f64 {
        let damage = line.split(" for ").nth(1).unwrap();
        damage[..damage.find(' ').unwrap()].parse().unwrap()
    };
    // The running bleed: when its tick timer started, the ticks since, the ticks left and the
    // amount of each.
    let mut bleed: Option<(f64, i32, i32, f64)> = None;
    let end = logs
        .lines()
        .filter(|line| line.starts_with('['))
        .map(at)
        .fold(0.0, f64::max);
    let mut bite = None;
    let (mut refreshes, mut hits) = (0, 0);
    for line in logs.lines() {
        if line.contains("[Target 1] {SpellID: 14271} Hit for")
            || line.contains("[Target 1] {SpellID: 14271} Crit for")
        {
            bite = Some((at(line), amount(line)));
        } else if line.contains(&format!("[Target 1] Aura gained: {BLEED}")) {
            let (when, damage) = bite.take().expect("a bite before the bleed");
            assert_eq!(when, at(line), "{line}");
            assert!(bleed.is_none(), "{line}");
            bleed = Some((when, 0, 7, damage * 0.4 / 7.0));
        } else if line.contains(&format!("[Target 1] Aura refreshed: {BLEED}")) {
            let (when, damage) = bite.take().expect("a bite before the refresh");
            let (start, ticked, left, tick) = bleed.expect("a running bleed");
            assert_eq!(when, at(line), "{line}");
            let next = start + 3.0 * f64::from(ticked + 1);
            let fits = ((21.0 - (next - when) + 1e-9) / 3.0).floor() as i32 + 1;
            bleed = Some((
                start,
                ticked,
                fits,
                (tick * f64::from(left) + damage * 0.4) / f64::from(fits),
            ));
            refreshes += 1;
        } else if line.contains(&format!("{BLEED} tick ")) {
            let (start, ticked, left, tick) = bleed.expect("a running bleed");
            assert!(
                (at(line) - (start + 3.0 * f64::from(ticked + 1))).abs() < 1e-6,
                "{line}: off the tick timer started at {start}"
            );
            if line.contains(" tick Hit for ") {
                assert!((amount(line) - tick).abs() < 0.01, "{line}: want {tick:.3}");
                hits += 1;
            }
            bleed = Some((start, ticked + 1, left - 1, tick));
        } else if line.contains(&format!("[Target 1] Aura faded: {BLEED}")) {
            let (_, _, left, _) = bleed.take().expect("a running bleed");
            // The fight's end fades a bleed with ticks left.
            if at(line) < end {
                assert_eq!(left, 0, "{line}: the bleed faded with ticks left");
            }
        }
    }
    assert!(refreshes > 1 && hits > 7, "{logs}");
    assert!(!logs.contains("{SpellID: 14271, Tag: 1}"), "{logs}");
}

/// The seconds of the first fight at which the player's Movement aura (Go `OtherActionMove`)
/// gains and fades, in order, with the player's label.
fn movement_windows(logs: &str, player: &str) -> Vec<(f64, f64)> {
    let tag = format!("[{player}]");
    let mut windows = Vec::new();
    let mut start = None;
    for line in logs.lines().filter(|line| line.contains(&tag)) {
        if line.contains("Aura gained: {OtherID: 20}") {
            start = Some(at(line));
        } else if line.contains("Aura faded: {OtherID: 20}") {
            windows.push((start.take().expect("a gain before the fade"), at(line)));
        }
    }
    windows
}

#[test]
fn a_hunter_that_moves_in_its_rotation_is_supported() {
    for name in ["survival-hunter-weaving-moves", "marksmanship-hunter-moves"] {
        assert!(check_prepared(&parse(fixture(name))).is_ok(), "{name}");
    }
}

/// Go movement.go: the player runs at seven yards a second, so the prepull run from 5 yards
/// out to 20 takes 15 / 7 seconds, and the rotation's moves in the fight take the distance
/// they cover at that speed, 30% faster for the 3 seconds after a Strider Kick.
#[test]
fn a_move_takes_the_distance_at_seven_yards_a_second() {
    let logs = first_fight_log(fixture("survival-hunter-weaving-moves"));
    let windows = movement_windows(&logs, "survival-hunter (#1)");
    let (start, end) = windows[0];
    assert!(start < 0.0, "{windows:?}");
    assert!((end - start - 15.0 / 7.0).abs() < 0.011, "{windows:?}");
    // The last move may be cut short by the end of the fight.
    let in_fight: Vec<_> = windows[..windows.len() - 1]
        .iter()
        .filter(|(start, _)| *start >= 0.0)
        .collect();
    assert!(!in_fight.is_empty(), "{windows:?}");
    assert!(
        in_fight
            .iter()
            .all(|(start, end)| end - start > 0.75 && end - start < 2.2),
        "{windows:?}"
    );
    assert!(logs.contains("[DEBUG] Moving to 5.0 yards"), "{logs}");
    assert!(logs.contains("[DEBUG] Moving to 12.0 yards"), "{logs}");
}

/// Go attack.go `swing`: a ranged auto cannot fire while its unit moves. It looks again every
/// 500ms, so none fires inside a run, and neither does a cast with a cast time.
#[test]
fn the_ranged_auto_waits_for_a_move_to_end() {
    let logs = first_fight_log(fixture("marksmanship-hunter-moves"));
    let windows = movement_windows(&logs, "marksmanship-hunter (#1)");
    assert!(windows.len() >= 3, "{windows:?}");
    for (start, end) in windows {
        for line in logs.lines().filter(|line| {
            line.contains("[marksmanship-hunter (#1)]")
                && (line.contains("Casting {OtherID: 4}")
                    || line.contains("Casting {SpellID: 20904}"))
        }) {
            let when = at(line);
            assert!(when <= start || when >= end, "{line} inside {start}..{end}");
        }
    }
}

const STEPPER: &str = "survival-hunter (#1)";
const STEPPER_SHOT: &str = "Casting {OtherID: 4}";
const STEPPER_SWING: &str = "Casting {OtherID: 3, Tag: 1}";
const STEPPER_RAPTOR: &str = "Casting {SpellID: 14266, Tag: 1}";

/// A rotation move to `yards` while the fight time is from `from` to before `until` and the
/// distance to the target compares with `than` by `op`.
fn step_to(yards: &str, (from, until): (&str, &str), op: &str, than: &str) -> Value {
    let time = |op: &str, val: &str| json!({"cmp": {"op": op, "lhs": {"currentTime": {}}, "rhs": {"const": {"val": val}}}});
    json!({"action": {
        "condition": {"and": {"vals": [
            time("OpGe", from),
            time("OpLt", until),
            {"cmp": {"op": op, "lhs": {"unitDistance": {}}, "rhs": {"const": {"val": than}}}}
        ]}},
        "move": {"rangeFromTarget": {"const": {"val": yards}}}
    }})
}

/// The weaving Survival Hunter with a 3.6 s bow, 12 yards out at the pull, where its first Auto
/// Shot fires. With `step`, it walks in to 5 yards a tenth of a second in, its main hand swings
/// on arrival (Raptor Strike replaces the swing with `raptor`), and it walks back out to 12
/// yards at 1.3 s, standing still well before its next shot is due. The seconds of the first
/// fight's Auto Shots, white main hand swings and Raptor Strikes, under Forever's Auto Shot rule
/// or the reference's.
fn shots_around_a_step_in(step: bool, raptor: bool, forever: bool) -> [Vec<f64>; 3] {
    let mut value = fixture("survival-hunter-weaving-moves");
    value["melee"]["ranged"]["swing_speed"] = json!(3.6);
    value["player"]["rotation"]["prepullActions"][1]["action"]["move"]["rangeFromTarget"]
        ["const"]["val"] = json!("12");
    let mut moves = Vec::new();
    if step {
        moves.push(step_to("5", ("0.1s", "1.3s"), "OpGe", "12"));
        if raptor {
            moves.push(json!({"action": {"castSpell": {"spellId": {"spellId": 14266, "tag": 3}}}}));
        }
        moves.push(step_to("12", ("1.3s", "600s"), "OpLe", "5"));
    }
    value["player"]["rotation"]["priorityList"] = json!(moves);
    if forever {
        value["player"]["melee_keeps_auto_shot_timer"] = json!(true);
    }
    let logs = first_fight_log(value);
    [STEPPER_SHOT, STEPPER_SWING, STEPPER_RAPTOR].map(|needle| {
        unit_lines(&logs, STEPPER)
            .into_iter()
            .filter(|line| line.contains(needle))
            .map(at)
            .collect()
    })
}

/// The one melee hit of the step in, which lands between the first two Auto Shots, and the
/// second shot.
fn hit_between_shots(shots: &[f64], hits: &[f64]) -> (f64, f64) {
    assert!(shots.len() > 2, "{shots:?}");
    assert_eq!(hits.len(), 1, "{hits:?}");
    let hit = hits[0];
    assert!(shots[0] < hit && hit < shots[1], "{hit} against {shots:?}");
    (hit, shots[1])
}

/// Forever's rule, from Forever's bug tracker (issue 14, "Ranged swing timer is reset by melee",
/// which a Forever developer confirmed on 6 October 2026 is a bug with Auto Shot): a white main
/// hand swing leaves the Auto Shot timer where it was, so the shot due 3.13 s into the fight
/// fires then, as it does for a Hunter who never steps in.
#[test]
fn a_melee_swing_leaves_the_auto_shot_timer_alone_in_forever() {
    let [still, _, _] = shots_around_a_step_in(false, false, true);
    let [shots, swings, _] = shots_around_a_step_in(true, false, true);
    let (_, next) = hit_between_shots(&shots, &swings);
    let due = still[1];
    assert!((next - due).abs() < 0.005, "{next}, due at {due}");
}

/// Raptor Strike in place of the swing is a melee swing too, and leaves the timer alone as well.
#[test]
fn raptor_strike_leaves_the_auto_shot_timer_alone_in_forever() {
    let [still, _, _] = shots_around_a_step_in(false, false, true);
    let [shots, swings, raptors] = shots_around_a_step_in(true, true, true);
    assert!(swings.is_empty(), "{swings:?}");
    let (_, next) = hit_between_shots(&shots, &raptors);
    let due = still[1];
    assert!((next - due).abs() < 0.005, "{next}, due at {due}");
}

/// Only Forever preparation writes the rule. Under the reference every melee swing, white or
/// Raptor Strike, restarts the Auto Shot timer as if a shot had just fired, so the next shot
/// comes a full shot after the hit.
#[test]
fn the_reference_restarts_the_auto_shot_timer_on_every_melee_swing() {
    let request = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/mage/prepared-v2/production-survival-hunter.request.json"),
    )
    .unwrap();
    let forever = forever_engine::prepare_json(&request, "x").unwrap();
    let go = forever_engine::prepare_json_with(&request, "x", forever_engine::Rules::GoReference)
        .unwrap();
    assert_eq!(
        forever["player"]["melee_keeps_auto_shot_timer"],
        json!(true)
    );
    assert!(go["player"].get("melee_keeps_auto_shot_timer").is_none());
    let [still, _, _] = shots_around_a_step_in(false, false, false);
    let speed = still[1] - still[0];
    for raptor in [false, true] {
        let [shots, swings, raptors] = shots_around_a_step_in(true, raptor, false);
        let hits = if raptor { raptors } else { swings };
        let (hit, next) = hit_between_shots(&shots, &hits);
        assert!(next > still[1] + 0.5, "{next}, due at {}", still[1]);
        assert!(
            (next - (hit + speed)).abs() < 0.015,
            "{next}, {hit} + {speed}"
        );
    }
}

/// A move in the priority list is supported for every class, and needs the speed the exporter
/// read.
#[test]
fn a_rotation_move_needs_the_exported_speed() {
    let mut value: Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures/mage/prepared-v2/production-warrior.prepared.json"),
        )
        .unwrap(),
    )
    .unwrap();
    value["player"]["rotation"]["priorityList"]
        .as_array_mut()
        .unwrap()
        .insert(
            0,
            json!({"action": {"move": {"rangeFromTarget": {"const": {"val": "5"}}}}}),
        );
    let refusals = super::refusal_codes(value.clone());
    assert!(
        refusals.contains(&(
            "movement_unsupported",
            "a move has no exported movement speed".to_string()
        )),
        "{refusals:?}"
    );
    // The exporter writes the effect when the rotation moves.
    value["effects"].as_array_mut().unwrap().push(json!({
        "kind": "player_movement", "speed_multiplier": 1.0, "speed_auras": []
    }));
    assert!(super::refusal_codes(value).is_empty());
}

/// The auras that change the movement speed are followed through their categories, so a move
/// with one is supported.
#[test]
fn a_move_with_a_speed_aura_is_supported() {
    let mut value = fixture("survival-hunter-weaving-moves");
    let effect = value["effects"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|effect| effect["kind"] == "player_movement")
        .expect("the exporter wrote the movement effect");
    effect["speed_auras"] = json!(["Aspect of the Cheetah"]);
    assert!(super::refusal_codes(value).is_empty());
}

/// A Volley channel is a non-melee cast: it holds the melee swing too, and the swings restart a
/// full swing after it ends (Go volley.go, community #743). The hunter's hands do not swing for
/// the six seconds of the channel, and the first swing after it is not until a swing later.
#[test]
fn volley_holds_the_melee_swing_until_the_channel_ends() {
    let mut value = fixture("production-survival-hunter-2-targets");
    value["sim"]["iterations"] = json!(1);
    let logs = simulate_prepared(&parse(value)).unwrap().result["logs"]
        .as_str()
        .unwrap()
        .to_string();
    let hunter = unit_lines(&logs, "survival-hunter (#1)");
    let volley = hunter
        .iter()
        .copied()
        .find(|line| line.contains("Casting {SpellID: 14295}"))
        .map(at)
        .expect("the hunter channels Volley");
    let channel_end = volley + 6.0;
    let swings: Vec<f64> = hunter
        .iter()
        .copied()
        .filter(|line| {
            line.contains("Completed cast {OtherID: 3, Tag: 1}")
                || line.contains("Completed cast {OtherID: 3, Tag: 2}")
        })
        .map(at)
        .collect();
    assert!(
        swings
            .iter()
            .all(|&swing| swing <= volley + 0.001 || swing >= channel_end),
        "a swing lands in the channel: {swings:?}"
    );
    let first_after = swings
        .iter()
        .copied()
        .find(|&swing| swing >= channel_end)
        .expect("the hunter swings again");
    assert!(first_after - channel_end >= 0.95, "{first_after}");
}

/// Dust Cloud yields to Sunder Armor and Expose Armor (mythicsim patch 110): a Tallstrider
/// against a target with a permanent Expose Armor never casts it, and casts it once nothing
/// holds the major armor reduction category.
#[test]
fn dust_cloud_yields_to_the_major_armor_reductions() {
    let casts = |value: Value| {
        first_fight_log(value)
            .lines()
            .filter(|line| line.contains("Casting {SpellID: 1265904}"))
            .count()
    };
    assert_eq!(casts(fixture("beast-mastery-hunter-tallstrider")), 0);
    assert!(casts(tallstrider_without_armor_reductions()) > 0);
}

/// The lines of a first-fight log at one timestamp.
fn lines_at<'a>(logs: &'a str, time: &str) -> Vec<&'a str> {
    let prefix = format!("[{time}] ");
    logs.lines()
        .filter(|line| line.starts_with(&prefix))
        .collect()
}

fn has_line(logs: &str, time: &str, needle: &str) -> bool {
    lines_at(logs, time)
        .iter()
        .any(|line| line.contains(needle))
}

/// Strider Kick (fork patch 120) activates an aura of 30% movement speed for 3 seconds after its
/// hit, which the speed category gives and takes back with a log line each.
#[test]
fn strider_kick_speeds_the_hunter_up_for_three_seconds() {
    let logs = first_fight_log(fixture("production-survival-hunter"));
    let kick = logs
        .lines()
        .find(|line| line.contains("Casting {SpellID: 1317257}"))
        .expect("a kick");
    let time = at(kick);
    let stamp = format!("{time:.2}");
    assert!(has_line(
        &logs,
        &stamp,
        "[DEBUG] Movement speed changed from 7.00 (0.00%) to 9.10 (30.00%)"
    ));
    assert!(has_line(&logs, &stamp, "Aura gained: {SpellID: 1317257}"));
    let faded = format!("{:.2}", time + 3.0);
    assert!(has_line(&logs, &faded, "Aura faded: {SpellID: 1317257}"));
    assert!(has_line(
        &logs,
        &faded,
        "[DEBUG] Movement speed changed from 9.10 (30.00%) to 7.00 (0.00%)"
    ));
}

/// A kick without its speed aura changes nothing about the hunter's speed, and the effect is
/// what activates it.
#[test]
fn strider_kick_without_its_speed_aura_leaves_the_speed_alone() {
    let mut value = fixture("production-survival-hunter");
    let effect = value["effects"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|effect| effect["kind"] == "strider_kick")
        .unwrap();
    assert_eq!(effect["speed_aura"], "Strider Kick");
    effect.as_object_mut().unwrap().remove("speed_aura");
    let logs = first_fight_log(value);
    assert!(!logs.contains("Movement speed changed"), "{logs}");
}

/// Fork patch 192: the fixture's prepull kick at -3 s comes a second into a walk from 5 yards out
/// to 20, 12 yards from the target. The kick's range check reads where the hunter is, so it is
/// refused and the walk keeps its 7 yards a second to the end, 15 / 7 seconds after it began. It
/// used to read the 5 yards the walk started from and kick from 12.
#[test]
fn a_kick_from_beyond_melee_range_mid_walk_is_refused() {
    let logs = first_fight_log(fixture("survival-hunter-strider-kick-while-moving"));
    assert!(has_line(&logs, "-4.00", "[DEBUG] Moving to 20.0 yards"));
    assert!(
        !logs.contains("Casting {SpellID: 1317257}")
            || at(logs
                .lines()
                .find(|line| line.contains("Casting {SpellID: 1317257}"))
                .unwrap())
                >= 0.0,
        "{logs}"
    );
    assert!(
        !has_line(&logs, "-3.00", "Movement speed changed"),
        "{logs}"
    );
    assert!(
        has_line(&logs, "-1.86", "{OtherID: 20} stacks: 5 --> 20"),
        "{logs}"
    );
    assert!(
        has_line(&logs, "-1.86", "Aura faded: {OtherID: 20}"),
        "{logs}"
    );
}

/// Fork patch 121: a speed change in the middle of a move runs the rest of it to the same
/// destination. The fixture's prepull is changed to walk out to 20 yards at -6 s and back in to
/// the target at -3.5 s, and to kick at -1.2 s, 3.9 yards out: the rest of the walk takes 3.9 /
/// 9.1 seconds and ends at the target at -0.77 s.
#[test]
fn a_speed_change_mid_move_keeps_the_destination() {
    let mut value = fixture("survival-hunter-strider-kick-while-moving");
    value["player"]["rotation"]["prepullActions"] = json!([
        {"action": {"castSpell": {"spellId": {"spellId": 25296}}}, "doAtValue": {"const": {"val": "-10s"}}},
        {"action": {"move": {"rangeFromTarget": {"const": {"val": "20"}}}}, "doAtValue": {"const": {"val": "-6s"}}},
        {"action": {"move": {"rangeFromTarget": {"const": {"val": "0"}}}}, "doAtValue": {"const": {"val": "-3.5s"}}},
        {"action": {"castSpell": {"spellId": {"spellId": 1317257}}}, "doAtValue": {"const": {"val": "-1.2s"}}}
    ]);
    value["player"]["prepull_actions"] = json!(4);
    let logs = first_fight_log(value);
    assert!(
        has_line(&logs, "-3.50", "[DEBUG] Moving to 0.0 yards"),
        "{logs}"
    );
    assert!(
        has_line(&logs, "-1.20", "Casting {SpellID: 1317257}"),
        "{logs}"
    );
    assert!(
        has_line(
            &logs,
            "-1.20",
            "[DEBUG] Movement speed changed from 7.00 (0.00%) to 9.10 (30.00%)"
        ),
        "{logs}"
    );
    assert!(
        has_line(&logs, "-0.77", "Aura faded: {OtherID: 20}"),
        "{logs}"
    );
}

/// Fork patch 119: the Wolf casts Furious Howl for 60 focus whenever its aura is down, giving
/// the pet and, within 15 yards of the target, the hunter 60 seconds of melee attack power.
#[test]
fn the_wolf_howls_for_itself_and_an_owner_within_range() {
    let logs = first_fight_log(fixture("beast-mastery-hunter-wolf"));
    let wolf = "hunter (#1) - Wolf";
    let cast = unit_lines(&logs, wolf)
        .into_iter()
        .find(|line| line.contains("Casting {SpellID: 24597}"))
        .expect("a howl");
    assert!(cast.contains("Cost = 60.000"), "{cast}");
    let stamp = format!("{:.2}", at(cast));
    // The pet's aura, then the hunter's.
    let gained: Vec<&str> = lines_at(&logs, &stamp)
        .into_iter()
        .filter(|line| line.contains("Aura gained: {SpellID: 24597}"))
        .collect();
    assert_eq!(gained.len(), 2, "{gained:?}");
    assert!(
        gained[0].contains(wolf) && !gained[1].contains(wolf),
        "{gained:?}"
    );
    // It recasts once the aura has faded, not before.
    let casts: Vec<f64> = unit_lines(&logs, wolf)
        .into_iter()
        .filter(|line| line.contains("Casting {SpellID: 24597}"))
        .map(at)
        .collect();
    assert!(
        casts.windows(2).all(|pair| pair[1] - pair[0] >= 60.0),
        "{casts:?}"
    );
}

/// The hunter stands at 25 yards, outside the effect's 15 yard radius, so only the pet is given
/// the aura.
#[test]
fn the_wolf_howls_for_itself_alone_when_its_owner_is_too_far() {
    let logs = first_fight_log(fixture("marksmanship-hunter-wolf-out-of-howl-range"));
    let wolf = "hunter (#1) - Wolf";
    let cast = logs
        .lines()
        .find(|line| line.contains(wolf) && line.contains("Casting {SpellID: 24597}"))
        .expect("a howl");
    let stamp = format!("{:.2}", at(cast));
    let gained: Vec<&str> = lines_at(&logs, &stamp)
        .into_iter()
        .filter(|line| line.contains("Aura gained: {SpellID: 24597}"))
        .collect();
    assert_eq!(gained.len(), 1, "{gained:?}");
    assert!(gained[0].contains(wolf), "{gained:?}");
}

/// The howl runs only with its effect, which claims the pet's aura.
#[test]
fn furious_howl_needs_its_effect() {
    let mut value = fixture("beast-mastery-hunter-wolf");
    assert!(check_prepared(&parse(value.clone())).is_ok());
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "hunter_pet_furious_howl");
    assert!(reasons(value).contains(&"the pet's ability at 1 has no behavior".to_string()));
}

/// Go pet.go makeStatInheritance (fork patches 136, 138 and 178): the pet's crit is its master's
/// physical crit plus the higher of the Auto Shot's ranged-only crit and the swing's melee-only
/// crit, held as an offset the owner moves; its hit is its master's melee plus ranged hit; and it
/// inherits its master's Expertise. Its disable callback only clears the offset, so it exports no
/// disable stats.
#[test]
fn the_pet_inherits_its_masters_crit_hit_and_expertise() {
    let value = fixture("production-beast-mastery-hunter");
    let pet = &value["pets"][0];
    assert_eq!(
        pet["inheritance"],
        json!([
            {"owner": "PhysicalHitPercent", "pet": "PhysicalHitPercent", "coefficient": 1},
            {"owner": "PhysicalCritPercent", "pet": "PhysicalCritPercent", "coefficient": 1},
            {"owner": "RangedHitPercent", "pet": "PhysicalHitPercent", "coefficient": 1},
            {"owner": "ExpertisePercent", "pet": "ExpertisePercent", "coefficient": 1},
        ])
    );
    assert_eq!(pet["disable_stats"], Value::Null);
}

/// The player's distance from the target over a first-fight log, from its moves ("Moving to"),
/// their ends (the Movement aura fading) and its speed changes, in log time (two decimals).
struct Track {
    /// From each point on: the time, the distance then, the yards a second toward the
    /// destination (0 standing), the destination.
    segments: Vec<(f64, f64, f64, f64)>,
}

impl Track {
    fn of(logs: &str, player: &str, start: f64) -> Track {
        let mut segments = vec![(f64::NEG_INFINITY, start, 0.0, start)];
        let mut speed = 7.0;
        for line in unit_lines(logs, player) {
            let time = at(line);
            let now = Track::at_time(&segments, time);
            let (_, _, rate, destination) = *segments.last().unwrap();
            if let Some(rest) = line.split("[DEBUG] Moving to ").nth(1) {
                let to: f64 = rest.split(' ').next().unwrap().parse().unwrap();
                segments.push((time, now, speed, to));
            } else if let Some(rest) = line.split("[DEBUG] Movement speed changed from ").nth(1) {
                speed = rest
                    .split(" to ")
                    .nth(1)
                    .unwrap()
                    .split(' ')
                    .next()
                    .unwrap()
                    .parse()
                    .unwrap();
                if rate != 0.0 {
                    segments.push((time, now, speed, destination));
                }
            } else if line.contains("Aura faded: {OtherID: 20}") {
                segments.push((time, destination, 0.0, destination));
            }
        }
        Track { segments }
    }

    fn at_time(segments: &[(f64, f64, f64, f64)], time: f64) -> f64 {
        let &(from, distance, rate, destination) = segments
            .iter()
            .rev()
            .find(|segment| segment.0 <= time)
            .unwrap_or(&segments[0]);
        let covered = rate * (time - from);
        if destination >= distance {
            (distance + covered).min(destination)
        } else {
            (distance - covered).max(destination)
        }
    }

    fn distance(&self, time: f64) -> f64 {
        Track::at_time(&self.segments, time)
    }
}

/// Every melee hit of the hunter lands within 5 yards and every shot from beyond 10.5 yards, to
/// the precision of the log's times. Returns the hits and the shots checked.
fn assert_ranges_hold(logs: &str, player: &str, start: f64) -> (usize, usize) {
    const MELEE: [&str; 4] = [
        "Completed cast {OtherID: 3,",
        "Completed cast {SpellID: 14266, Tag: 1}",
        "Completed cast {SpellID: 14271}",
        "Completed cast {SpellID: 1317257}",
    ];
    const SHOTS: [&str; 4] = [
        "Completed cast {OtherID: 4}",
        "Completed cast {SpellID: 14287}",
        "Completed cast {SpellID: 2643}",
        "Completed cast {SpellID: 25295}",
    ];
    // A hundredth of a second at the 9.1 yards a second of Strider Kick.
    const SLACK: f64 = 0.1;
    let track = Track::of(logs, player, start);
    let (mut hits, mut shots) = (0, 0);
    for line in unit_lines(logs, player) {
        let time = at(line);
        if time < 0.0 {
            continue;
        }
        let distance = track.distance(time);
        if MELEE.iter().any(|needle| line.contains(needle)) {
            hits += 1;
            assert!(distance <= 5.0 + SLACK, "{line} from {distance:.2} yards");
        }
        if SHOTS.iter().any(|needle| line.contains(needle)) {
            shots += 1;
            assert!(distance >= 10.5 - SLACK, "{line} from {distance:.2} yards");
        }
    }
    (hits, shots)
}

/// Fork patch 191: a hunter at 5 yards kicks with 0.3 s left on its swing timer and steps out to
/// 12 yards in the same check. The kick's GCD holds the rotation, so nothing brings the position
/// up to date during the walk; the swing that comes due 2.7 yards out reads where the hunter is
/// and does not land. It used to land from 7.7 yards.
#[test]
fn a_swing_due_on_the_way_out_of_melee_does_not_land() {
    let logs = first_fight_log(fixture("survival-hunter-kick-step-out-swing"));
    let player = "Custom Hunter (#1)";
    assert!(logs.contains("[DEBUG] Moving to 12.0 yards"), "{logs}");
    let (hits, _) = assert_ranges_hold(&logs, player, 5.0);
    assert!(hits > 0, "{logs}");
}

/// Fork patches 190 to 192 on Bae's Survival weave (MythicSim Discord, 21 October, sim 4541dcea
/// at seed 8): no swing lands on the way out (a white hit from 11.5 yards at 3.71 s before 191),
/// no shot goes off inside 10.5 yards (from 8 yards before 190), and a Multi-Shot queued at 16.27
/// s, 10.95 yards out, cannot fire at 16.32 s from 10.25 yards (before 192).
#[test]
fn the_survival_weave_keeps_melee_and_shot_ranges() {
    let logs = first_fight_log(fixture("survival-hunter-weave-bae-queued-shot"));
    let player = "Custom Hunter (#1)";
    assert!(
        has_line(&logs, "16.27", "Queueing up {SpellID: 2643}"),
        "{logs}"
    );
    assert!(
        has_line(
            &logs,
            "16.32",
            "{SpellID: 2643} failed to cast: extra spell condition"
        ),
        "{logs}"
    );
    assert!(
        !has_line(&logs, "16.32", "Completed cast {SpellID: 2643}"),
        "{logs}"
    );
    let (hits, shots) = assert_ranges_hold(&logs, player, 12.0);
    assert!(hits > 20 && shots > 20, "{hits} hits, {shots} shots");
}

/// Forever's rule on the same fight: a player who has queued a spell stays put until it fires,
/// so the shots queued at the end of the global cooldown go off before the step in, and fewer
/// shots fail inside the minimum range. A shot the rotation queues once the walk has started can
/// still fail, as the reference's can.
#[test]
fn a_queued_spell_holds_the_weave_in_place_until_it_fires() {
    let failed = |logs: &str| {
        logs.matches("failed to cast: extra spell condition")
            .count()
    };
    let reference = first_fight_log(fixture("survival-hunter-weave-bae-queued-shot"));
    let mut value = fixture("survival-hunter-weave-bae-queued-shot");
    value["player"]["move_waits_for_queued_cast"] = json!(true);
    value["player"]["move_arrives_at_destination"] = json!(true);
    let logs = first_fight_log(value);
    assert!(
        failed(&logs) < failed(&reference),
        "{} failed casts under Forever's rule, {} under the reference",
        failed(&logs),
        failed(&reference)
    );
    let (hits, shots) = assert_ranges_hold(&logs, "Custom Hunter (#1)", 12.0);
    assert!(hits > 20 && shots > 20, "{hits} hits, {shots} shots");
}

const WEAVER: &str = "survival-hunter (#1)";

/// A rotation move to `range` yards when `condition` holds.
fn move_when(range: &str, condition: Value) -> Value {
    json!({"action": {"condition": condition, "move": {"rangeFromTarget": {"const": {"val": range}}}}})
}

/// The current time compared with a constant.
fn time_is(op: &str, time: &str) -> Value {
    json!({"cmp": {"op": op, "lhs": {"currentTime": {}}, "rhs": {"const": {"val": time}}}})
}

/// The distance to the target compared with a constant.
fn distance_is(op: &str, yards: &str) -> Value {
    json!({"cmp": {"op": op, "lhs": {"unitDistance": {}}, "rhs": {"const": {"val": yards}}}})
}

/// The weaving Survival Hunter with the beta swing probe's 2.33 s off hand: the prepull walk
/// leaves it `start` yards out, its Auto Shot fires at the pull, and its priority list holds
/// only `moves`. The first fight's log, under Forever's off hand rule or the reference's.
fn off_hand_weave(start: &str, moves: Value, forever: bool) -> String {
    let mut value = fixture("survival-hunter-weaving-moves");
    value["melee"]["off_hand"]["swing_speed"] = json!(2.33);
    value["player"]["rotation"]["prepullActions"][1]["action"]["move"]["rangeFromTarget"]
        ["const"]["val"] = json!(start);
    value["player"]["rotation"]["priorityList"] = moves;
    if forever {
        value["player"]["off_hand_waits_from_attack_start"] = json!(true);
    }
    first_fight_log(value)
}

/// One walk in from 20 or 12 yards, a second into the fight.
fn walk_in_once(start: &str, forever: bool) -> String {
    off_hand_weave(
        start,
        json!([move_when("5", time_is("OpGe", "1s"))]),
        forever,
    )
}

/// A walk in from 12 yards a second into the fight, a walk out at 8.5 s and straight back in.
fn walk_out_and_back(forever: bool) -> String {
    let out = json!({"and": {"vals": [
        time_is("OpGe", "8.5s"), time_is("OpLt", "10s"), distance_is("OpLe", "5")
    ]}});
    let back = json!({"and": {"vals": [time_is("OpGe", "8.5s"), distance_is("OpGe", "12")]}});
    let first = json!({"and": {"vals": [time_is("OpGe", "1s"), time_is("OpLt", "8.5s")]}});
    off_hand_weave(
        "12",
        json!([
            move_when("5", first),
            move_when("12", out),
            move_when("5", back)
        ]),
        forever,
    )
}

/// The seconds of the player's off hand swings and Auto Shots in a first-fight log.
fn swings(logs: &str, needle: &str) -> Vec<f64> {
    unit_lines(logs, WEAVER)
        .into_iter()
        .filter(|line| line.contains(needle))
        .map(at)
        .collect()
}

const OFF_HAND: &str = "Casting {OtherID: 3, Tag: 2}";
const AUTO_SHOT: &str = "Casting {OtherID: 4}";

/// The first off hand swing at or after `time`, and half the off hand's swing, from the gap
/// between two swings in melee range. Log times are rounded to the hundredth.
fn off_hand_after(logs: &str, time: f64) -> (f64, f64) {
    let off_hand = swings(logs, OFF_HAND);
    let half = (off_hand[1] - off_hand[0]) / 2.0;
    let first = off_hand
        .into_iter()
        .find(|&swing| swing >= time - 0.005)
        .expect("an off hand swing");
    (first, half)
}

/// Forever's rule, from the beta swing probe: an Auto Shot turns the melee attack off and the
/// walk back in turns it on as it starts, so the off hand's half swing wait runs during the walk.
/// A walk longer than the wait brings the off hand in on arrival, beside the main hand.
#[test]
fn a_walk_in_longer_than_half_the_off_hand_swing_swings_it_on_arrival() {
    let logs = walk_in_once("20", true);
    let (start, arrival) = movement_windows(&logs, WEAVER)[1];
    assert!(swings(&logs, AUTO_SHOT)[0] <= start, "{logs}");
    let (first, half) = off_hand_after(&logs, arrival);
    assert!(
        arrival - start > half + 0.5,
        "{start}..{arrival}, half {half}"
    );
    assert!((first - arrival).abs() < 0.015, "{first} after {arrival}");
}

/// A walk shorter than the off hand's half swing wait: the off hand swings half a swing after the
/// walk began, a little after arrival.
#[test]
fn a_short_walk_in_swings_the_off_hand_half_a_swing_after_it_began() {
    let logs = walk_in_once("12", true);
    let (start, arrival) = movement_windows(&logs, WEAVER)[1];
    assert!(swings(&logs, AUTO_SHOT)[0] <= start, "{logs}");
    let (first, half) = off_hand_after(&logs, arrival);
    assert!(
        arrival - start < half - 0.1,
        "{start}..{arrival}, half {half}"
    );
    assert!(
        (first - (start + half)).abs() < 0.015,
        "{first}, {start} + {half}"
    );
}

/// With no Auto Shot since the attack started, a walk out of melee range and back in adds no new
/// wait: the off hand resumes on arrival, as the main hand does, since its swing came due while
/// the hunter was out.
#[test]
fn walking_out_and_back_in_without_a_shot_adds_no_off_hand_wait() {
    let logs = walk_out_and_back(true);
    let windows = movement_windows(&logs, WEAVER);
    let (out, _) = windows[2];
    let (_, back) = windows[3];
    assert!(
        swings(&logs, AUTO_SHOT)
            .iter()
            .all(|&shot| shot < out || shot > back),
        "{logs}"
    );
    let last = swings(&logs, OFF_HAND)
        .into_iter()
        .rev()
        .find(|&swing| swing < out)
        .expect("an off hand swing before the walk out");
    let (first, half) = off_hand_after(&logs, back);
    assert!(
        last + 2.0 * half < back,
        "{last} + {} before {back}",
        2.0 * half
    );
    assert!((first - back).abs() < 0.015, "{first} after {back}");
}

/// Only Forever preparation writes the rule. Under the reference the off hand swings half a swing
/// after every arrival in melee range: after a long walk, a short one, and a step back in with
/// the attack still on.
#[test]
fn the_reference_swings_the_off_hand_half_a_swing_after_every_arrival() {
    let request = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/mage/prepared-v2/production-survival-hunter.request.json"),
    )
    .unwrap();
    let forever = forever_engine::prepare_json(&request, "x").unwrap();
    let go = forever_engine::prepare_json_with(&request, "x", forever_engine::Rules::GoReference)
        .unwrap();
    assert_eq!(
        forever["player"]["off_hand_waits_from_attack_start"],
        json!(true)
    );
    assert!(go["player"]
        .get("off_hand_waits_from_attack_start")
        .is_none());
    for (logs, walk) in [
        (walk_in_once("20", false), 1),
        (walk_in_once("12", false), 1),
        (walk_out_and_back(false), 3),
    ] {
        let (_, arrival) = movement_windows(&logs, WEAVER)[walk];
        let (first, half) = off_hand_after(&logs, arrival);
        assert!(
            (first - (arrival + half)).abs() < 0.015,
            "{first}, {arrival} + {half}"
        );
    }
}

const WALKER: &str = "marksmanship-hunter (#1)";

/// The moving Marksmanship Hunter, 12 yards out, with a rotation of two lines: a walk out to 20
/// yards once the fight is 0.9 s old, and Arcane Shot, an instant on the 1.5 s global cooldown,
/// which it casts at the pull. The Auto Shot at the pull wakes the rotation under that global
/// cooldown. The seconds of the first fight's Arcane Shot and of the start of its walk, under
/// Forever's rule for a rotation with a move or the reference's.
fn walk_under_the_gcd(forever: bool) -> (f64, f64) {
    let mut value = fixture("marksmanship-hunter-moves");
    value["player"]["rotation"]["priorityList"] = json!([
        {"action": {
            "condition": {"cmp": {
                "op": "OpGe", "lhs": {"currentTime": {}}, "rhs": {"const": {"val": "0.9s"}}
            }},
            "move": {"rangeFromTarget": {"const": {"val": "20"}}}
        }},
        {"action": {"castSpell": {"spellId": {"spellId": 14287}}}}
    ]);
    if forever {
        value["player"]["rotation_with_moves_looks_through_gcd"] = json!(true);
    }
    let logs = first_fight_log(value);
    let arcane_shots: Vec<&str> = unit_lines(&logs, WALKER)
        .into_iter()
        .filter(|line| line.contains("Casting {SpellID: 14287}"))
        .collect();
    assert!(
        arcane_shots[0].contains("Cast Time = 0s") && arcane_shots[0].contains("GCD = 1.5s"),
        "{}",
        arcane_shots[0]
    );
    let walks = movement_windows(&logs, WALKER);
    let walk = walks
        .iter()
        .map(|&(start, _)| start)
        .find(|&start| start >= 0.0)
        .unwrap_or_else(|| panic!("no walk in the fight: {walks:?}"));
    (at(arcane_shots[0]), walk)
}

/// Forever's rule: moving needs no global cooldown, so a rotation with a move keeps looking
/// through it, and the walk
/// starts within one reaction time of 0.9 s, under the Arcane Shot's global cooldown.
#[test]
fn a_rotation_with_a_move_walks_under_the_gcd_in_forever() {
    let (arcane_shot, walk) = walk_under_the_gcd(true);
    assert_eq!(arcane_shot, 0.0);
    assert!((0.9..=1.0).contains(&walk), "walk at {walk}");
}

/// Only Forever preparation writes the rule. Under the reference the idle rotation sleeps until
/// the global cooldown ends, so the walk whose condition turned true at 0.9 s starts at 1.5 s.
#[test]
fn the_reference_starts_a_move_only_when_the_gcd_ends() {
    let request = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/mage/prepared-v2/production-survival-hunter.request.json"),
    )
    .unwrap();
    let forever = forever_engine::prepare_json(&request, "x").unwrap();
    let go = forever_engine::prepare_json_with(&request, "x", forever_engine::Rules::GoReference)
        .unwrap();
    assert_eq!(
        forever["player"]["rotation_with_moves_looks_through_gcd"],
        json!(true)
    );
    assert!(go["player"]
        .get("rotation_with_moves_looks_through_gcd")
        .is_none());
    let (arcane_shot, walk) = walk_under_the_gcd(false);
    assert!((walk - (arcane_shot + 1.5)).abs() < 0.005, "walk at {walk}");
}

/// Under the same rule the rotation goes on from a queued instant as it goes off. The Hunter
/// casts Serpent Sting at the pull, looks through its global cooldown, queues Arcane Shot for its
/// end and steps out to 20 yards once Arcane Shot is on cooldown: as it fires, not when its own
/// global cooldown ends or the next Auto Shot wakes the rotation.
#[test]
fn a_step_out_after_a_queued_instant_starts_as_it_fires_in_forever() {
    let mut value = fixture("marksmanship-hunter-moves");
    value["player"]["rotation"]["priorityList"] = json!([
        {"action": {
            "condition": {"cmp": {
                "op": "OpGt",
                "lhs": {"spellTimeToReady": {"spellId": {"spellId": 14287}}},
                "rhs": {"const": {"val": "0s"}}
            }},
            "move": {"rangeFromTarget": {"const": {"val": "20"}}}
        }},
        {"action": {
            "condition": {"cmp": {
                "op": "OpLt", "lhs": {"currentTime": {}}, "rhs": {"const": {"val": "0.5s"}}
            }},
            "castSpell": {"spellId": {"spellId": 25295}}
        }},
        {"action": {"castSpell": {"spellId": {"spellId": 14287}}}}
    ]);
    value["player"]["rotation_with_moves_looks_through_gcd"] = json!(true);
    // A 120ms reaction keeps the rotation's looks, every reaction time from the Auto Shot at the
    // pull, off the end of the global cooldown, where the rotation would cast Arcane Shot itself.
    value["player"]["reaction_ns"] = json!(120_000_000);
    let logs = first_fight_log(value);
    let lines = unit_lines(&logs, WALKER);
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Queueing up {SpellID: 14287}")),
        "{logs}"
    );
    let arcane_shot = lines
        .iter()
        .find(|line| line.contains("Casting {SpellID: 14287}"))
        .map(|line| at(line))
        .unwrap();
    assert!(
        (arcane_shot - 1.5).abs() < 0.005,
        "Arcane Shot at {arcane_shot}"
    );
    let walks = movement_windows(&logs, WALKER);
    let walk = walks
        .iter()
        .map(|&(start, _)| start)
        .find(|&start| start >= 0.0)
        .unwrap_or_else(|| panic!("no walk in the fight: {walks:?}"));
    assert!((walk - arcane_shot).abs() < 0.005, "walk at {walk}");
}

/// A queued spell that fails to cast does not run the rotation again. The Hunter casts Serpent
/// Sting at the pull, queues Aimed Shot for the end of its global cooldown and starts walking out
/// to 20 yards at 1.3 s, so the queued Aimed Shot, which cannot start while moving, fails. Running
/// the rotation then would queue it again for the same instant, without end.
#[test]
fn a_queued_spell_that_fails_does_not_run_the_rotation_again_in_forever() {
    let mut value = fixture("marksmanship-hunter-moves");
    // Without the prepull Aimed Shot it is ready to queue under the first global cooldown.
    value["player"]["rotation"]["prepullActions"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    value["player"]["prepull_actions"] = json!(1);
    value["player"]["rotation"]["priorityList"] = json!([
        {"action": {
            "condition": {"cmp": {
                "op": "OpGe", "lhs": {"currentTime": {}}, "rhs": {"const": {"val": "1.3s"}}
            }},
            "move": {"rangeFromTarget": {"const": {"val": "20"}}}
        }},
        {"action": {
            "condition": {"cmp": {
                "op": "OpLt", "lhs": {"currentTime": {}}, "rhs": {"const": {"val": "0.5s"}}
            }},
            "castSpell": {"spellId": {"spellId": 25295}}
        }},
        {"action": {"castSpell": {"spellId": {"spellId": 20904}}}}
    ]);
    value["player"]["rotation_with_moves_looks_through_gcd"] = json!(true);
    // Without a log first, so a fight that never ends fails here rather than filling memory.
    let mut quiet = value.clone();
    quiet["sim"]["iterations"] = json!(1);
    quiet["sim"]["debug_first_iteration"] = json!(false);
    quiet["sim"]["debug"] = json!(false);
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || sender.send(simulate_prepared(&parse(quiet)).is_ok()));
    let ended = receiver.recv_timeout(std::time::Duration::from_secs(10));
    assert_eq!(ended, Ok(true), "the fight did not end");
    let logs = first_fight_log(value);
    let lines = unit_lines(&logs, WALKER);
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Queueing up {SpellID: 20904}")),
        "{logs}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("{SpellID: 20904} failed to cast")),
        "{logs}"
    );
}

/// A rotation without a move keeps sleeping until the global cooldown ends under Forever's
/// rule: the turret Marksmanship Hunter's raid metrics are the same to the digit with or without
/// it.
#[test]
fn a_rotation_without_a_move_keeps_its_gcd_wait_in_forever() {
    let request = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/mage/prepared-v2/production-marksmanship-hunter.request.json"),
    )
    .unwrap();
    let mut forever = forever_engine::prepare_json(&request, "x").unwrap();
    assert_eq!(
        forever["player"]["rotation_with_moves_looks_through_gcd"],
        json!(true)
    );
    assert!(!forever["player"]["rotation"].to_string().contains("\"move"));
    forever["sim"]["iterations"] = json!(200);
    let metrics =
        |value: Value| simulate_prepared(&parse(value)).unwrap().result["raidMetrics"].clone();
    let with_rule = metrics(forever.clone());
    forever["player"]
        .as_object_mut()
        .unwrap()
        .remove("rotation_with_moves_looks_through_gcd");
    assert_eq!(with_rule, metrics(forever));
}

/// The player's mana metrics, by action id, and its damage per second, over a short run.
fn mana_rows(mut value: Value, forever: bool) -> (Vec<(Value, f64, f64)>, f64) {
    value["sim"]["iterations"] = json!(200);
    if forever {
        value["player"]["casting_regen_at_once"] = json!(true);
    }
    let report = simulate_prepared(&parse(value)).unwrap();
    let player = &report.result["raidMetrics"]["parties"][0]["players"][0];
    let rows = player["resources"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["type"] == "ResourceTypeMana")
        .map(|row| {
            let amount = |key: &str| row[key].as_f64().unwrap_or(0.0);
            (row["id"].clone(), amount("gain"), amount("actualGain"))
        })
        .collect();
    (rows, player["dps"]["avg"].as_f64().unwrap())
}

fn row(rows: &[(Value, f64, f64)], id: Value) -> Option<(f64, f64)> {
    rows.iter()
        .find(|(row, _, _)| *row == id)
        .map(|&(_, gain, actual)| (gain, actual))
}

/// Bae (23 October 2026): Resourcefulness is up most of a Survival fight and raises the time to
/// OOM, but neither regeneration row moves. The reference adds its casting regeneration without
/// refreshing the cached mana tick, so only Resourcefulness's cost cut ever acted. In the client
/// the mana regeneration interrupt aura acts at once: under Forever's rule the tick follows the
/// buff, and the mana it adds lands in a row of its own, as Innervate's does.
#[test]
fn resourcefulness_mana_lands_in_a_row_of_its_own() {
    let resourcefulness = json!({ "spellId": 1242688 });
    let casting = json!({ "otherId": "OtherActionManaRegen", "tag": 1 });
    let survival = fixture("production-survival-hunter");
    assert!(survival["player"].get("casting_regen_at_once").is_none());

    let (go, go_dps) = mana_rows(survival.clone(), false);
    assert_eq!(row(&go, resourcefulness.clone()), None);
    let (forever, forever_dps) = mana_rows(survival, true);
    let (gain, actual) = row(&forever, resourcefulness).expect("a Resourcefulness row");
    // Judgement of Wisdom keeps this Hunter near full, so the rotation is the same and the
    // casting row keeps the regeneration the reference gave it; the fuller bar caps more of it.
    assert_eq!(forever_dps, go_dps);
    let (go_casting, go_actual) = row(&go, casting.clone()).unwrap();
    let (forever_casting, forever_actual) = row(&forever, casting).unwrap();
    assert!(
        (forever_casting - go_casting).abs() < go_casting * 1e-9,
        "{forever_casting} against {go_casting}"
    );
    assert!(forever_actual <= go_actual && forever_actual + actual > go_actual);
    // 50% of spirit regeneration while casting, up nearly all fight.
    assert!(gain > 1000.0 * 200.0, "{gain}");
    assert!(actual > 0.0 && actual < gain, "{actual} of {gain}");
}

/// Rapid Recuperation takes the same rule: a Marksmanship Hunter short of mana gains damage.
#[test]
fn rapid_recuperation_mana_acts_at_once_in_forever() {
    let marksmanship = fixture("marksmanship-hunter-rapid-recuperation");
    let (go, go_dps) = mana_rows(marksmanship.clone(), false);
    let recuperation = json!({ "spellId": 1242512 });
    assert_eq!(row(&go, recuperation.clone()), None);
    let (forever, forever_dps) = mana_rows(marksmanship, true);
    let (gain, actual) = row(&forever, recuperation).expect("a Rapid Recuperation row");
    assert!(gain > 0.0 && actual > gain * 0.9, "{actual} of {gain}");
    assert!(forever_dps > go_dps, "{forever_dps} against {go_dps}");
}

/// Forever preparation writes the rule; the reference's rules leave it out.
#[test]
fn only_forever_preparation_refreshes_casting_regen_at_once() {
    let request = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/mage/prepared-v2/production-survival-hunter.request.json"),
    )
    .unwrap();
    let forever = forever_engine::prepare_json(&request, "x").unwrap();
    let go = forever_engine::prepare_json_with(&request, "x", forever_engine::Rules::GoReference)
        .unwrap();
    assert_eq!(forever["player"]["casting_regen_at_once"], json!(true));
    assert!(go["player"].get("casting_regen_at_once").is_none());
    assert_eq!(forever["player"]["move_waits_for_queued_cast"], json!(true));
    assert!(go["player"].get("move_waits_for_queued_cast").is_none());
    assert_eq!(
        forever["player"]["move_arrives_at_destination"],
        json!(true)
    );
    assert!(go["player"].get("move_arrives_at_destination").is_none());
    assert_eq!(
        forever["player"]["threat_follows_damage_bonus"],
        json!(true)
    );
    assert!(go["player"].get("threat_follows_damage_bonus").is_none());
}

/// Go `GetAPLCastSpell`: a rotation that names Raptor Strike untagged, as a spell picker writes
/// it, presses the queue spell, since the untagged spell has no APL flag. The gate resolves the
/// cast the way the runtime does, so the rotation is supported, and in melee range the queue
/// replaces a swing with Raptor Strike.
#[test]
fn an_untagged_raptor_strike_presses_its_queue() {
    let mut value = fixture("survival-hunter-strider-kick-while-moving");
    value["player"]["rotation"]["prepullActions"] = json!([]);
    value["player"]["prepull_actions"] = json!(0);
    value["player"]["rotation"]["priorityList"] =
        json!([{"action": {"castSpell": {"spellId": {"spellId": 14266}}}}]);
    assert_eq!(
        forever_engine::prepared_coverage(&parse(value.clone())),
        Vec::<String>::new()
    );
    let logs = first_fight_log(value);
    assert!(
        logs.contains("Casting {SpellID: 14266, Tag: 3}"),
        "the queue was never pressed:\n{logs}"
    );
}

/// The production Marksmanship Hunter's request prepared with `rules`.
fn production_marksmanship_prepared(rules: forever_engine::Rules) -> Value {
    let request = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/mage/prepared-v2/production-marksmanship-hunter.request.json"),
    )
    .unwrap();
    forever_engine::prepare_json_with(&request, "x", rules).unwrap()
}

/// Each Multi-Shot cast of the first fight with the ranged haste up as it starts: whether Rapid
/// Fire (3045) and Quick Shots (6150) are up, and the cast line.
fn multi_shot_casts(logs: &str) -> Vec<(bool, bool, &str)> {
    let (mut rapid_fire, mut quick_shots) = (false, false);
    let mut casts = Vec::new();
    for line in unit_lines(logs, "marksmanship-hunter (#1)") {
        for (aura, up) in [
            ("{SpellID: 3045}", &mut rapid_fire),
            ("{SpellID: 6150}", &mut quick_shots),
        ] {
            if line.ends_with(&format!("Aura gained: {aura}")) {
                *up = true;
            } else if line.ends_with(&format!("Aura faded: {aura}")) {
                *up = false;
            }
        }
        if line.contains("Casting {SpellID: 2643}") {
            casts.push((rapid_fire, quick_shots, line));
        }
    }
    casts
}

/// Forever's rule (README, Rust-only mechanics): Multi-Shot casts over half a second divided by
/// the ranged haste multiplier, as Aimed Shot's two seconds are. Our Forever beta logs: 0.44 to
/// 0.45 seconds with the quiver's 15% (0.5 / 1.15), 0.32 under Rapid Fire as well (0.5 / 1.61).
/// Seed 8's first fight casts it with the quiver alone and with Rapid Fire up.
#[test]
fn multi_shot_casts_half_a_second_over_the_ranged_haste_in_forever() {
    let mut value = production_marksmanship_prepared(forever_engine::Rules::Forever);
    value["sim"]["seed"] = json!(8);
    let logs = first_fight_log(value);
    let casts = multi_shot_casts(&logs);
    let expected = |rapid_fire: bool, quick_shots: bool| match (rapid_fire, quick_shots) {
        (false, false) => "Cast Time = 434.782608ms",
        (true, false) => "Cast Time = 310.559006ms",
        (false, true) => "Cast Time = 334.44816ms",
        (true, true) => "Cast Time = 238.891543ms",
    };
    for &(rapid_fire, quick_shots, line) in &casts {
        assert!(line.contains(expected(rapid_fire, quick_shots)), "{line}");
    }
    for rapid_fire in [false, true] {
        assert!(
            casts.iter().any(|&(rf, qs, _)| rf == rapid_fire && !qs),
            "no Multi-Shot with Rapid Fire {rapid_fire} and Quick Shots down: {casts:?}"
        );
    }
}

/// Only Forever preparation gives Multi-Shot its half second cast; under the reference's rules
/// its row's cast time, none, keeps it instant.
#[test]
fn only_forever_preparation_gives_multi_shot_a_cast_time() {
    let cast_time = |prepared: &Value| {
        let spells = prepared["player"]["spells"].as_array().unwrap();
        let multi_shot = spells
            .iter()
            .find(|spell| spell["action_id"] == json!({ "spell_id": 2643 }))
            .expect("Multi-Shot");
        multi_shot["default_cast"]["cast_time_ns"].clone()
    };
    let forever = production_marksmanship_prepared(forever_engine::Rules::Forever);
    let mut go = production_marksmanship_prepared(forever_engine::Rules::GoReference);
    assert_eq!(cast_time(&forever), json!(500_000_000));
    assert_eq!(cast_time(&go), json!(0));
    go["sim"]["seed"] = json!(8);
    let logs = first_fight_log(go);
    let casts = multi_shot_casts(&logs);
    assert!(!casts.is_empty(), "{logs}");
    for (_, _, line) in casts {
        assert!(line.contains("Cast Time = 0s"), "{line}");
    }
}
