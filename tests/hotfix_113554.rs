//! Live server hotfix pushes 113531 and 113554 (client 1.60.1.70707, 30 October 2026), which only
//! Forever's rules prepare: the Druid and Hunter trees in the client's order, the hotfixed Druid,
//! Mage, Hunter and Warlock rows, Moonkin Aura and Leader of the Pack as one 3% crit, and the
//! blessings the game no longer has. The Go reference keeps the frozen data and its goldens.
//! Paladin and Priest are not here.

use forever_engine::{prepare_json_with, PrepareError, Rules};
use serde_json::{json, Value};
use std::{fs, path::Path};

const BERSERK: i64 = 417141;
const LACERATE: i64 = 1235827;
const SHIFTING_POWER: i64 = 1322605;
const ICE_LANCE: i64 = 1240047;
const FIRE_BLAST: i64 = 10199;
const FREEZING_TRAP: i64 = 1499;
const EXPLOSIVE_TRAP: i64 = 14317;
/// The live Fire board's string, in client 1.60.1.70707's order, which Forever's rules read; the
/// reference reads the fixture's own string in Go's order.
const FIRE_TALENTS: &str = "0502052001001-55500001230133031-003";

/// The Fire mage fixture with talents the rules read.
fn fire_request(rules: Rules) -> Value {
    match rules {
        Rules::Forever => with_talents("production-fire", FIRE_TALENTS),
        Rules::GoReference => request("production-fire"),
    }
}

fn request(name: &str) -> Value {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/mage/prepared-v2"
    ))
    .join(format!("{name}.request.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn with_talents(name: &str, talents: &str) -> Value {
    let mut request = request(name);
    request["raid"]["parties"][0]["players"][0]["talentsString"] = Value::from(talents);
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

fn spell(prepared: &Value, spell_id: i64) -> Option<&Value> {
    prepared["player"]["spells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|spell| {
            spell["action_id"]["spell_id"] == spell_id
                && spell["action_id"]["tag"].as_i64().unwrap_or(0) == 0
        })
}

fn effect<'a>(prepared: &'a Value, kind: &str) -> Option<&'a Value> {
    prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == kind)
}

fn has_aura(prepared: &Value, label: &str) -> bool {
    prepared["player"]["auras"]
        .as_array()
        .unwrap()
        .iter()
        .any(|aura| aura["label"].as_str().is_some_and(|l| l.starts_with(label)))
}

// Druid.

/// Feral reads Furor where Heart of the Wild was, Restoration Heart of the Wild where Furor was,
/// and Reflection and Subtlety swap; the reference reads Go's field numbers.
#[test]
fn druid_trees_read_the_hotfix_order() {
    let request = with_talents("production-feral-druid", "-050000000030000025-0513");
    assert_eq!(
        prepared(&request, Rules::Forever)["player"]["talents"],
        json!({
            "furor": 5, "predatory_strikes": 3, "rend_and_tear": 2, "natural_reaction": 5,
            "heart_of_the_wild": 5, "naturalist": 1, "reflection": 3,
        })
    );
    assert_eq!(
        prepared(&request, Rules::GoReference)["player"]["talents"],
        json!({
            "heart_of_the_wild": 5, "natural_instinct": 2, "natural_reaction": 5,
            "furor": 5, "naturalist": 1, "subtlety": 3,
        })
    );
}

/// The live boards' strings from before the hotfix put points in a talent the new order makes
/// smaller, so Forever's rules refuse them under `talents`; the reference still reads them.
#[test]
fn pre_hotfix_druid_strings_are_refused() {
    for (fixture, old, reason) in [
        (
            "production-feral-druid",
            "050022-55000032021032212051-053",
            "Predatory Discipline has 1 rank, the string gives 3",
        ),
        (
            "production-feral-bear-druid",
            "-55000332120132012551-055",
            "Predatory Discipline has 1 rank, the string gives 3",
        ),
    ] {
        let request = with_talents(fixture, old);
        let reason_given = refusal(&request, Rules::Forever);
        assert!(reason_given.contains(reason), "{old}: {reason_given}");
        prepared(&request, Rules::GoReference);
    }
    let long = with_talents("production-feral-druid", "-550000320210322120511");
    let reason = refusal(&long, Rules::Forever);
    assert!(
        reason.contains("tree 2 has 20 talents, the string gives 21"),
        "{reason}"
    );
}

/// Berserk and Improved Shifting Power have no square any more, so no Forever string reaches
/// them; Shifting Power's cooldown is 8 sec (16 in the frozen row).
#[test]
fn berserk_is_gone_and_shifting_power_is_8_seconds() {
    let forever = prepared(
        &with_talents("production-feral-druid", "-55000000023010100001"),
        Rules::Forever,
    );
    assert!(spell(&forever, BERSERK).is_none());
    assert!(effect(&forever, "berserk").is_none());
    assert_eq!(
        spell(&forever, SHIFTING_POWER).unwrap()["cd"]["duration_ns"],
        8_000_000_000_i64
    );

    let reference = prepared(&request("production-feral-druid"), Rules::GoReference);
    assert!(spell(&reference, BERSERK).is_some());
    // Shifting Power without Improved Shifting Power, in Go's order.
    let reference = prepared(
        &with_talents("production-feral-druid", "-55000032021"),
        Rules::GoReference,
    );
    assert_eq!(
        spell(&reference, SHIFTING_POWER).unwrap()["cd"]["duration_ns"],
        16_000_000_000_i64
    );
}

/// Predatory Discipline (1328613): a landed Ferocious Bite keeps the energy left after its cost.
#[test]
fn predatory_discipline_keeps_the_bite_energy() {
    let keeps = |talents: &str| {
        let prepared = prepared(
            &with_talents("production-feral-druid", talents),
            Rules::Forever,
        );
        effect(&prepared, "ferocious_bite").unwrap()["keeps_energy"].clone()
    };
    assert_eq!(keeps("-55000000023010100001"), json!(true));
    assert_eq!(keeps("-550000000230001"), Value::Null);
}

/// Lacerate is a talent (1328617): without it a Forever bear has no Lacerate. Primal Bite's
/// level 60 rank is 1238070 with rank 4's 77 flat damage; the reference keeps 1238073.
#[test]
fn lacerate_is_a_talent_and_primal_bite_is_rank_3() {
    let with = prepared(
        &with_talents("production-feral-bear-druid", "-5500000200310010001"),
        Rules::Forever,
    );
    assert!(spell(&with, LACERATE).is_some());
    assert!(spell(&with, 1238070).is_some());
    assert!(spell(&with, 1238073).is_none());
    assert_eq!(effect(&with, "primal_bite").unwrap()["flat_damage"], 77.0);

    let without = prepared(
        &with_talents("production-feral-bear-druid", "-5500000200300010001"),
        Rules::Forever,
    );
    assert!(spell(&without, LACERATE).is_none());
    assert!(effect(&without, "lacerate").is_none());

    let reference = prepared(&request("production-feral-bear-druid"), Rules::GoReference);
    assert!(spell(&reference, LACERATE).is_some());
    assert!(spell(&reference, 1238073).is_some());
    assert!(spell(&reference, 1238070).is_none());
}

/// Naturalist's damage is 2% a rank (10% at 5/5, 5% in the reference) and Genesis' periodic
/// bonus 2% a rank.
#[test]
fn naturalist_and_genesis_double() {
    let multiplier = |prepared: &Value| {
        prepared["player"]["pseudo_stats"]["damage_dealt_multiplier"]
            .as_f64()
            .unwrap()
    };
    let forever = prepared(
        &with_talents("production-feral-druid", "--005"),
        Rules::Forever,
    );
    let reference = prepared(
        &with_talents("production-feral-druid", "--005"),
        Rules::GoReference,
    );
    assert_eq!(forever["player"]["talents"]["naturalist"], 5);
    assert_eq!(reference["player"]["talents"]["naturalist"], 5);
    assert!((multiplier(&forever) / multiplier(&reference) - 1.10 / 1.05).abs() < 1e-12);
}

// Raid buffs.

/// Moonkin Aura and Leader of the Pack are each 3% of every kind of crit and one counts: a party
/// with both still gives 3% physical and 3% spell crit, and Leader of the Pack alone now gives
/// spell crit too. The reference keeps them narrowed and stacking.
#[test]
fn moonkin_aura_and_leader_of_the_pack_are_one_crit() {
    let crit = |party: Value, rules: Rules| {
        let mut request = fire_request(rules);
        request["raid"]["parties"][0]["buffs"] = party;
        let prepared = prepared(&request, rules);
        let stats = &prepared["player"]["stats"];
        (
            stats["SpellCritPercent"].as_f64().unwrap(),
            stats["PhysicalCritPercent"].as_f64().unwrap(),
        )
    };
    for rules in [Rules::Forever, Rules::GoReference] {
        let (spell, physical) = crit(json!({}), rules);
        let (both_spell, both_physical) =
            crit(json!({"moonkinAura": true, "leaderOfThePack": true}), rules);
        assert!((both_spell - spell - 3.0).abs() < 1e-9, "{rules:?}");
        assert!((both_physical - physical - 3.0).abs() < 1e-9, "{rules:?}");
        let (pack_spell, pack_physical) = crit(json!({"leaderOfThePack": true}), rules);
        assert!((pack_physical - physical - 3.0).abs() < 1e-9, "{rules:?}");
        let expected = if rules == Rules::Forever { 3.0 } else { 0.0 };
        assert!((pack_spell - spell - expected).abs() < 1e-9, "{rules:?}");
    }
}

/// Greater Blessing of Salvation and Greater Blessing of Light are gone from the game: under
/// Forever's rules a saved setting asking for them applies nothing.
#[test]
fn removed_blessings_apply_nothing() {
    let blessed = |rules: Rules| {
        let mut request = fire_request(rules);
        request["raid"]["parties"][0]["players"][0]["buffs"] =
            json!({"greaterBlessingOfSalvation": true, "greaterBlessingOfLight": true});
        prepared(&request, rules)
    };
    let forever = blessed(Rules::Forever);
    let reference = blessed(Rules::GoReference);
    assert!(!has_aura(&forever, "Mass Salvation"));
    assert!(!has_aura(&forever, "Greater Blessing of Light"));
    assert!(has_aura(&reference, "Mass Salvation"));
    assert!(has_aura(&reference, "Greater Blessing of Light"));
    let threat = |prepared: &Value| {
        prepared["player"]["pseudo_stats"]["threat_multiplier"]
            .as_f64()
            .unwrap()
    };
    assert!((threat(&forever) * 0.7 - threat(&reference)).abs() < 1e-12);
}

// Mage.

/// Arcane Resilience has 3 ranks (25/50/75% of Intellect as armor); a fourth point is refused.
#[test]
fn arcane_resilience_has_three_ranks() {
    let armor = |talents: &str, rules: Rules| {
        prepared(&with_talents("frost-reference", talents), rules)["player"]["stats"]["Armor"]
            .as_f64()
            .unwrap()
    };
    let base = armor("", Rules::Forever);
    let two = armor("003", Rules::Forever) - base;
    let three = armor("003", Rules::Forever) - armor("002", Rules::Forever);
    assert!(two > 0.0);
    assert!((three * 3.0 - two).abs() < 1e-6, "{two} {three}");
    let reason = refusal(&with_talents("frost-reference", "004"), Rules::Forever);
    assert!(
        reason.contains("Arcane Resilience has 3 ranks, the string gives 4"),
        "{reason}"
    );
}

/// Ice Lance: 8 sec on category 19, Fire Blast's, with a 300% Frozen bonus; Winter's Chill 20%
/// a stack; Arcane Blast's buff 12% of Intellect, 100% more cost and 18 sec; Ice Barrier 5% and
/// Frigid Power 15% Frost damage.
#[test]
fn mage_rows_read_the_hotfix() {
    let talents = "0502052-5-053502030101301251";
    let forever = prepared(&with_talents("frost-reference", talents), Rules::Forever);
    let lance = spell(&forever, ICE_LANCE).unwrap();
    let blast = spell(&forever, FIRE_BLAST).unwrap();
    assert_eq!(lance["cd"]["duration_ns"], 8_000_000_000_i64);
    assert_eq!(lance["cd"]["timer"], blast["cd"]["timer"]);
    assert_eq!(
        effect(&forever, "ice_lance").unwrap()["frozen_multiplier"],
        4.0
    );
    assert_eq!(
        effect(&forever, "winters_chill").unwrap()["damage_per_stack"],
        0.2
    );
    let barrier = effect(&forever, "ice_barrier").unwrap();
    assert_eq!(barrier["hit"], 0.05);
    assert_eq!(barrier["frigid_hit"], 0.15);

    // The live Frost string of before 70707, in Go's order.
    let reference = prepared(
        &with_talents("frost-reference", "0502105003-03-053502030101301251"),
        Rules::GoReference,
    );
    assert!(spell(&reference, ICE_LANCE).unwrap()["cd"].is_null());
    assert_eq!(
        effect(&reference, "ice_lance").unwrap()["frozen_multiplier"],
        3.0
    );
    assert_eq!(
        effect(&reference, "winters_chill").unwrap()["damage_per_stack"],
        0.05
    );

    let arcane = prepared(
        &with_talents("frost-reference", "050205203100311531-3-05350003"),
        Rules::Forever,
    );
    let buff = effect(&arcane, "arcane_blast").unwrap();
    assert_eq!(buff["intellect_per_stack"], 0.12);
    assert_eq!(buff["cost_per_stack"], 1.0);
}

// Hunter.

/// Pathfinding and Improved Revive Pet swap squares: the fifth Beast Mastery digit is Improved
/// Revive Pet and the sixth Pathfinding under Forever's rules.
#[test]
fn hunter_beast_mastery_reads_the_hotfix_order() {
    let request = with_talents("production-beast-mastery-hunter", "000012");
    assert_eq!(
        prepared(&request, Rules::Forever)["player"]["talents"],
        json!({"improved_revive_pet": 1, "pathfinding": 2})
    );
    assert_eq!(
        prepared(&request, Rules::GoReference)["player"]["talents"],
        json!({"pathfinding": 1, "improved_revive_pet": 2})
    );
}

/// Freezing Trap joins category 411, so it shares Explosive Trap's cooldown.
#[test]
fn freezing_trap_shares_the_fire_traps_cooldown() {
    let timers = |rules: Rules| {
        let prepared = prepared(&request("production-survival-hunter"), rules);
        (
            spell(&prepared, FREEZING_TRAP).unwrap()["cd"]["timer"].clone(),
            spell(&prepared, EXPLOSIVE_TRAP).unwrap()["cd"]["timer"].clone(),
        )
    };
    let (freezing, explosive) = timers(Rules::Forever);
    assert_eq!(freezing, explosive);
    let (freezing, explosive) = timers(Rules::GoReference);
    assert_ne!(freezing, explosive);
}
