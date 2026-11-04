//! The Warrior part of Blizzard's development notes of 2 November 2026, as the client carries it in
//! the hotfix cache of 3 to 4 November (build 70755): the new Arms and Fury trees (Improved
//! Tactical Mastery at three ranks, Bloodthrill at two, Fury's 18 talents in the client's order),
//! Flurry at three ranks, Improved Whirlwind, Reckless Swings and Spearing Strike's 75%. Only
//! Forever's rules read them; the Go reference keeps the frozen data and the proto's tree order.

use forever_engine::{
    check_prepared, contracts::prepared_v2::PreparedV2, prepare_json_with, simulate_prepared,
    PrepareError, Rules,
};
use serde_json::{json, Value};
use std::{fs, path::Path};

/// wowsims/forever #841's own presets, in the client's order: Fury (Improved Whirlwind 2, Reckless
/// Swings, Booming Voice 3) and Arms (Booming Voice 5).
const FURY: &str = "30303213002-350531005122100231";
const ARMS: &str = "30323213132215201-5505-2";
/// The reference builds of the Fury, Fury-prot, Arms and Protection boards before and after.
const FURY_BOARD_OLD: &str = "20305213002-05353005102010501-2";
const FURY_BOARD: &str = "30313213002-053530005120100231-2";
const ARMS_BOARD_OLD: &str = "30325213132515201-0505-2";
const PROTECTION_OLD: &str = "-0525003005-250033120310021251";
const PROTECTION: &str = "-05250035-250033120310021251";

const WHIRLWIND: i64 = 1680;
const BLOODTHIRST: i64 = 23894;
const RECKLESS_SWINGS: i64 = 1329555;

fn production(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "fixtures/mage/prepared-v2/production-{name}.request.json"
    ));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn player_mut(request: &mut Value) -> &mut Value {
    &mut request["raid"]["parties"][0]["players"][0]
}

fn with_talents(name: &str, talents: &str) -> Value {
    let mut request = production(name);
    player_mut(&mut request)["talentsString"] = json!(talents);
    request
}

fn prepared(request: &Value, rules: Rules) -> Value {
    prepare_json_with(request.to_string().as_bytes(), "rust", rules)
        .unwrap_or_else(|err| panic!("{err:?}"))
}

fn refusal(request: &Value, rules: Rules) -> String {
    match prepare_json_with(request.to_string().as_bytes(), "rust", rules) {
        Err(PrepareError::Refused(refusal)) => {
            assert_eq!(refusal.code, "talents");
            refusal.reason
        }
        other => panic!("{:?}", other.map(|_| ())),
    }
}

fn effect<'a>(prepared: &'a Value, kind: &str) -> Option<&'a Value> {
    prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == kind)
}

fn spell(prepared: &Value, spell_id: i64) -> Option<&Value> {
    prepared["player"]["spells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|spell| spell["action_id"]["spell_id"] == spell_id && spell["action_id"]["tag"] != 2)
}

/// A rotation of the named casts, in order of priority.
fn rotation(spells: &[Value]) -> Value {
    let list: Vec<Value> = spells
        .iter()
        .map(|id| json!({"action": {"castSpell": {"spellId": id}}}))
        .collect();
    json!({"type": "TypeAPL", "priorityList": list})
}

/// The first fight of a request as a log: one iteration, a short fight with no variation.
fn first_fight_log(request: &Value, seconds: f64, targets: usize) -> String {
    let mut request = request.clone();
    request["encounter"]["duration"] = json!(seconds);
    request["encounter"]["durationVariation"] = json!(0);
    let template = request["encounter"]["targets"][0].clone();
    request["encounter"]["targets"] = json!(vec![template; targets]);
    let mut value = prepared(&request, Rules::Forever);
    value["sim"]["iterations"] = json!(1);
    value["sim"]["debug_first_iteration"] = json!(true);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    check_prepared(&prepared).unwrap();
    simulate_prepared(&prepared).unwrap().result["logs"]
        .as_str()
        .unwrap()
        .to_string()
}

/// A Fury warrior in Berserker Stance that casts only the given spells.
fn casting(talents: &str, spells: &[Value]) -> Value {
    let mut request = with_talents("warrior", talents);
    player_mut(&mut request)["rotation"] = rotation(spells);
    request
}

/// Forever's client order reads the presets #841 measured, and the Go reference reads the same
/// string in the proto's order, where Fury's ninth talent (Raging Blows, one rank) holds five points.
#[test]
fn the_new_layout_reads_the_client_presets() {
    let fury = prepared(&with_talents("warrior", FURY), Rules::Forever);
    // Flurry has 3 points of 3 and Improved Whirlwind 2 of 2: 30% attack speed, 30% more damage.
    assert_eq!(
        effect(&fury, "warrior_flurry").unwrap()["melee_speed_multiplier"],
        1.3
    );
    assert_eq!(effect(&fury, "whirlwind").unwrap()["closest_bonus"], 0.3);
    let swings = effect(&fury, "reckless_swings").unwrap();
    assert_eq!(swings["spell_id"], RECKLESS_SWINGS);
    assert_eq!(swings["charges"], 4);
    assert_eq!(swings["rage"], 5.0);
    assert!(prepared(&with_talents("arms-warrior", ARMS), Rules::Forever)["effects"].is_array());
    // The Go reference reads the Fury string in the proto's order and refuses it.
    let reason = refusal(&with_talents("warrior", FURY), Rules::GoReference);
    assert!(
        reason.contains("tree 2 has 17 talents, the string gives 18"),
        "{reason}"
    );
}

/// Strings written for the old layout that do not fit the new one are refused with the talent
/// named: the Arms strings put five points in Improved Tactical Mastery (3 ranks) or Bloodthrill
/// (2), the Fury strings five in Reckless Swings (1 rank) where Enrage used to be.
#[test]
fn old_strings_that_do_not_fit_are_refused() {
    for (talents, name, ranks, given) in [
        (FURY_BOARD_OLD, "Improved Tactical Mastery", "3 ranks", 5),
        (ARMS_BOARD_OLD, "Improved Tactical Mastery", "3 ranks", 5),
        ("000000000005", "Bloodthrill", "2 ranks", 5),
        (PROTECTION_OLD, "Reckless Swings", "1 rank", 5),
        (
            "3-35050030152010501-25003012031",
            "Reckless Swings",
            "1 rank",
            5,
        ),
    ] {
        let reason = refusal(&with_talents("warrior", talents), Rules::Forever);
        assert!(
            reason.contains(&format!("{name} has {ranks}, the string gives {given}")),
            "{talents}: {reason}"
        );
    }
    // A Fury tree of 19 talents does not fit the 18.
    let reason = refusal(
        &with_talents("warrior", "-0000000000000000000"),
        Rules::Forever,
    );
    assert!(reason.contains("tree 2 has 18 talents"), "{reason}");
    // Under the reference every one of them reads as it always has (or refuses on its own ranks).
    for talents in [FURY_BOARD_OLD, ARMS_BOARD_OLD, PROTECTION_OLD] {
        prepared(&with_talents("warrior", talents), Rules::GoReference);
    }
}

/// The re-encoded reference builds (the same points where the talent still exists) read under
/// the new layout; Protection keeps its points and moves only Enrage's.
#[test]
fn the_reencoded_reference_builds_prepare() {
    for talents in [
        FURY_BOARD,
        ARMS,
        PROTECTION,
        "3-350500350120100231-25003012031",
    ] {
        prepared(&with_talents("warrior", talents), Rules::Forever);
    }
    let protection = prepared(
        &with_talents("protection-warrior", PROTECTION),
        Rules::Forever,
    );
    assert!(effect(&protection, "warrior_enrage").is_some());
}

/// Flurry: three ranks of 10/20/30% attack speed for 3 swings under Forever's rules; the
/// reference has five ranks of 5%.
#[test]
fn flurry_has_three_ranks() {
    for (flurry, forever, reference) in [("1", 1.1, 1.05), ("2", 1.2, 1.1), ("3", 1.3, 1.15)] {
        // The 17 Fury digits of the proto's order put Flurry at the fifteenth, the client's
        // 18 at the seventeenth.
        let forever_fury = format!("0000000000000000{flurry}");
        let go_fury = format!("00000000000000{flurry}");
        let forever_value = prepared(
            &with_talents("warrior", &format!("-{forever_fury}-")),
            Rules::Forever,
        );
        let reference_value = prepared(
            &with_talents("warrior", &format!("-{go_fury}-")),
            Rules::GoReference,
        );
        let multiplier = |value: &Value| {
            value["effects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["kind"] == "warrior_flurry")
                .unwrap()["melee_speed_multiplier"]
                .as_f64()
                .unwrap()
        };
        assert!(
            (multiplier(&forever_value) - forever).abs() < 1e-12,
            "{flurry}"
        );
        assert!(
            (multiplier(&reference_value) - reference).abs() < 1e-12,
            "{flurry}"
        );
    }
    // A fourth point is past the ranks.
    let reason = refusal(
        &with_talents("warrior", "-00000000000000004"),
        Rules::Forever,
    );
    assert!(
        reason.contains("Flurry has 3 ranks, the string gives 4"),
        "{reason}"
    );
}

/// Improved Whirlwind: Whirlwind's cooldown 1/2 sec shorter and 15/30% more damage to the closest
/// enemy. Absent from the reference, which still has Raging Blows' Rage discount in its place.
#[test]
fn improved_whirlwind_cuts_the_cooldown_and_names_the_bonus() {
    // Improved Whirlwind is the sixteenth Fury talent.
    for (rank, cooldown, bonus) in [(0, 10.0, None), (1, 9.0, Some(0.15)), (2, 8.0, Some(0.3))] {
        let talents = format!("-{}{}-", "0".repeat(15), rank);
        let value = prepared(&with_talents("warrior", &talents), Rules::Forever);
        let seconds = spell(&value, WHIRLWIND).unwrap()["cd"]["duration_ns"]
            .as_f64()
            .unwrap()
            / 1e9;
        assert_eq!(seconds, cooldown, "rank {rank}");
        let whirlwind = effect(&value, "whirlwind").unwrap();
        assert_eq!(
            whirlwind.get("closest_bonus").and_then(Value::as_f64),
            bonus
        );
    }
}

/// The damage of each hit of a spell in a log, by timestamp, as (first target, second target)
/// for the moments both targets were plain hits (a crit or a dodge moves one of them for
/// reasons unrelated to the bonus).
fn hit_pairs(log: &str, tag: i64) -> Vec<(f64, f64)> {
    let marker = format!("{{SpellID: {WHIRLWIND}, Tag: {tag}}} Hit for ");
    let mut pairs: Vec<(f64, f64)> = Vec::new();
    let mut open: Option<(String, f64)> = None;
    for line in log.lines().filter(|line| line.contains(&marker)) {
        let time = line.split(']').next().unwrap().to_string();
        let damage: f64 = line
            .split(&marker)
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .unwrap();
        if line.contains("[Target 1]") {
            open = Some((time, damage));
        } else if let Some((first_time, first)) = open.take() {
            if first_time == time {
                pairs.push((first, damage));
            }
        }
    }
    pairs
}

/// The bonus lands on the first target a Whirlwind strike hits, both hands: when both targets
/// take a plain hit from one strike (one weapon roll serves every target), the first takes 30% more with
/// Improved Whirlwind at 2 points, and the same without it.
#[test]
fn improved_whirlwind_boosts_only_the_first_target() {
    let log_for = |talents: &str| {
        first_fight_log(
            &casting(talents, &[json!({"spellId": WHIRLWIND, "tag": 1})]),
            120.0,
            2,
        )
    };
    let boosted = log_for(&format!("-{}2-", "0".repeat(15)));
    let plain = log_for("-");
    for tag in [1, 2] {
        let with = hit_pairs(&boosted, tag);
        assert!(!with.is_empty(), "no plain pair of hits for tag {tag}");
        for (first, second) in with {
            assert!(
                (first / second - 1.3).abs() < 1e-3,
                "tag {tag}: {first} against {second}"
            );
        }
        for (first, second) in hit_pairs(&plain, tag) {
            assert!(
                (first / second - 1.0).abs() < 1e-3,
                "tag {tag}: {first} against {second}"
            );
        }
    }
    // Its cooldown is 2 sec shorter: the second cast of 8 sec after the first.
    let casts: Vec<f64> = boosted
        .lines()
        .filter(|line| line.contains(&format!("Casting {{SpellID: {WHIRLWIND}, Tag: 1}}")))
        .map(|line| line[1..line.find(']').unwrap()].parse().unwrap())
        .collect();
    assert!((casts[1] - casts[0] - 8.0).abs() < 0.05, "{casts:?}");
}

/// Reckless Swings over a short fight: the cast costs 25% of base health, and each of the next
/// four landed white swings gives 5 Rage, so a fight casts it again after its 15 sec cooldown.
#[test]
fn reckless_swings_gives_rage_on_four_swings() {
    let request = casting(
        FURY,
        &[
            json!({"spellId": RECKLESS_SWINGS}),
            json!({"spellId": BLOODTHIRST}),
        ],
    );
    let log = first_fight_log(&request, 32.0, 1);
    let casts: Vec<&str> = log
        .lines()
        .filter(|line| line.contains(&format!("Casting {{SpellID: {RECKLESS_SWINGS}}}")))
        .collect();
    // At 0, 15 and 30 seconds: its cooldown is 15 sec.
    assert_eq!(
        casts.len(),
        3,
        "{casts:?}\n{}",
        log.lines().take(60).collect::<Vec<_>>().join("\n")
    );
    let gains: Vec<&str> = log
        .lines()
        .filter(|line| {
            line.contains(&format!("{{SpellID: {RECKLESS_SWINGS}}}"))
                && line.contains("Gained 5.000 rage")
        })
        .collect();
    // Each cast leaves the aura at four charges, spent on the next four swings: four gains
    // of 5 Rage between the first cast and the second, and the 32 sec fight's third cast
    // gets its swings too.
    let second_cast = log
        .lines()
        .position(|line| {
            line.contains(&format!("Casting {{SpellID: {RECKLESS_SWINGS}}}"))
                && !line.starts_with("[0.00]")
        })
        .unwrap();
    let before_second = log
        .lines()
        .take(second_cast)
        .filter(|line| {
            line.contains(&format!("{{SpellID: {RECKLESS_SWINGS}}}"))
                && line.contains("Gained 5.000 rage")
        })
        .count();
    assert_eq!(before_second, 4);
    assert!(gains.len() > 4 && gains.len() <= 12, "{gains:?}");
    assert!(log.contains(&format!("Aura gained: {{SpellID: {RECKLESS_SWINGS}}}")));
}

/// The rotation names Reckless Swings by its id, and a warrior without the talent has no such
/// spell (the Go reference never does).
#[test]
fn reckless_swings_is_castable_only_with_the_talent() {
    let with = prepared(&with_talents("warrior", FURY), Rules::Forever);
    let spell = spell(&with, RECKLESS_SWINGS).expect("the spell");
    assert_eq!(spell["cd"]["duration_ns"], 15_000_000_000i64);
    assert_eq!(spell["class_spell"], "reckless_swings");
    let without = prepared(&with_talents("warrior", "-"), Rules::Forever);
    assert!(self::spell(&without, RECKLESS_SWINGS).is_none());
    assert!(effect(&without, "reckless_swings").is_none());
}

/// Spearing Strike deals 75% weapon damage and 150% against Giants and Dragonkin (was 40% and
/// 120%) under Forever's rules.
#[test]
fn spearing_strike_is_seventy_five_percent() {
    let forever = prepared(&with_talents("arms-warrior", ARMS), Rules::Forever);
    let share = |value: &Value| {
        effect(value, "spearing_strike").unwrap()["weapon_share"]
            .as_f64()
            .unwrap()
    };
    assert!((share(&forever) - 0.75).abs() < 1e-12);
    let reference = prepared(&production("arms-warrior"), Rules::GoReference);
    assert!((share(&reference) - 0.40).abs() < 1e-12);
}

/// Improved Tactical Mastery keeps 5/10/15 Rage on a stance change (was five ranks of 3) and
/// Bloodthrill's chance is 10/20% (was five ranks of 4%).
#[test]
fn tactical_mastery_and_bloodthrill_have_fewer_ranks() {
    let retained = |talents: &str, rules| {
        effect(
            &prepared(&with_talents("arms-warrior", talents), rules),
            "warrior_stances",
        )
        .unwrap()["max_retained_rage"]
            .as_f64()
            .unwrap()
    };
    let base = retained("-", Rules::Forever);
    assert_eq!(retained("00003", Rules::Forever) - base, 15.0);
    assert_eq!(retained("00002", Rules::Forever) - base, 10.0);
    assert_eq!(retained("00001", Rules::Forever) - base, 5.0);
    assert_eq!(retained("00005", Rules::GoReference) - base, 15.0);
    // Bloodthrill is the twelfth Arms talent.
    let chance = |rank: u32, rules| {
        effect(
            &prepared(
                &with_talents("arms-warrior", &format!("{}{rank}", "0".repeat(11))),
                rules,
            ),
            "bloodthrill",
        )
        .unwrap()["proc_chance"]
            .as_f64()
            .unwrap()
    };
    assert!((chance(1, Rules::Forever) - 0.10).abs() < 1e-12);
    assert!((chance(2, Rules::Forever) - 0.20).abs() < 1e-12);
    assert!((chance(1, Rules::GoReference) - 0.04).abs() < 1e-12);
    assert!((chance(5, Rules::GoReference) - 0.20).abs() < 1e-12);
}

/// Raging Blows' discount on Cleave and Whirlwind (3 Rage) leaves with the talent: the ninth
/// Fury slot of the proto's order is Raging Blows under the reference and Dual Wield
/// Specialization under Forever's layout, so the same digit takes 3 Rage off Whirlwind in one and not
/// the other. The engine never had Boundless Rage, Improved Cleave or Precision in Fury.
#[test]
fn raging_blows_leaves_the_fury_tree() {
    let flat = |rules| {
        let value = prepared(&with_talents("warrior", "-000000001"), rules);
        spell(&value, WHIRLWIND).unwrap()["cost"]["flat_modifier"].clone()
    };
    assert_eq!(flat(Rules::GoReference), -3);
    assert_eq!(flat(Rules::Forever), 0);
}
