//! The Paladin tree of live server hotfix push 113554 (30 October 2026, client 1.60.1.70707 by
//! DBCache), which only Forever's rules prepare: the layout read in the client's order, the
//! refusals of strings that no longer fit, the new ranks and the reworked talents. The Go
//! reference keeps the 70425 tree. Rows from upstream wowsims/forever #772's regenerated store,
//! checked against Wowhead's Forever talent data.

use forever_engine::{
    check_prepared, prepare, prepare_json_with, simulate_prepared, PrepareError, Rules,
};
use serde_json::{json, Value};
use std::{fs, path::Path};

const RETRIBUTION: &str = "production-retribution-paladin";
const PROTECTION: &str = "production-protection-paladin";
const HOLY: &str = "holy-paladin-healing";

const HOLY_STRIKE: i64 = 10333;
const HAMMER_OF_WRATH: i64 = 24239;
const SWIFT_JUDGEMENT: i64 = 1310994;
const HOLY_SHIELD_3: i64 = 20928;
const HOLY_SHOCK_HEAL: i64 = 25903;
const HOLY_LIGHT: i64 = 25292;

/// A Retribution build of push 113554's tree (upstream #772's Retribution preset with Precision
/// for Reverence and Pursuit of Justice): Divine Strength 5, Improved Seals 5, Holy Conduit 2,
/// Divine Intellect 5, Precision 3, and Retribution to Unsealed Wrath.
const RET_113554: &str = "050050002-503-05233301001301351";
/// A Protection build to Shieldsworn (upstream #772's Protection preset).
const PROT_113554: &str = "050050002-05302102213301531-502";

fn request(fixture: &str, talents: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2")
        .join(format!("{fixture}.request.json"));
    let mut request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    request["raid"]["parties"][0]["players"][0]["talentsString"] = Value::from(talents);
    request["simOptions"]["iterations"] = json!(20);
    serde_json::to_vec(&request).unwrap()
}

fn prepared(fixture: &str, talents: &str, rules: Rules) -> Value {
    prepare_json_with(&request(fixture, talents), "rust", rules)
        .unwrap_or_else(|err| panic!("{talents}: {err:?}"))
}

fn refusal(fixture: &str, talents: &str) -> String {
    match prepare_json_with(&request(fixture, talents), "rust", Rules::Forever) {
        Err(PrepareError::Refused(refusal)) => {
            assert_eq!(refusal.code, "talents", "{talents}");
            refusal.reason
        }
        other => panic!("{talents}: {:?}", other.map(|_| ())),
    }
}

fn spell(prepared: &Value, spell_id: i64) -> &Value {
    prepared["player"]["spells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|spell| {
            spell["action_id"]["spell_id"] == spell_id && spell["action_id"]["tag"].is_null()
        })
        .unwrap_or_else(|| panic!("no spell {spell_id}"))
}

fn effect<'a>(prepared: &'a Value, kind: &str) -> Option<&'a Value> {
    prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == kind)
}

fn stat(prepared: &Value, name: &str) -> f64 {
    prepared["player"]["stats"][name].as_f64().unwrap()
}

/// The string reads in push 113554's order under Forever's rules: Spiritual Focus first in Holy,
/// Divine Intellect first in Protection, Holy Conduit in Holy's third row and Unyielding Faith in
/// Retribution's. The Go reference reads the same digits by Go's field numbers.
#[test]
fn forever_reads_the_113554_layout_and_the_reference_its_own() {
    let talents = "500000002-5-0000002";
    assert_eq!(
        prepared(RETRIBUTION, talents, Rules::Forever)["player"]["talents"],
        json!({"spiritual_focus": 5, "holy_conduit": 2, "divine_intellect": 5, "unyielding_faith": 2})
    );
    assert_eq!(
        prepared(RETRIBUTION, talents, Rules::GoReference)["player"]["talents"],
        json!({"divine_strength": 5, "purifying_power": 2, "toughness": 5, "sanctified_judgement": 2})
    );
}

/// Every paladin board string of the live 70707 tree is refused under Forever's rules, each for
/// a talent above its push 113554 ranks; the reference still reads them.
#[test]
fn the_70707_board_strings_are_refused_under_forever_rules() {
    for (fixture, old, reason) in [
        (
            RETRIBUTION,
            "52003-503-05225231001330321",
            "Sanctified Judgement has 3 ranks, the string gives 5",
        ),
        (
            RETRIBUTION,
            "550031032001012--052252300012303",
            "Sanctified Judgement has 3 ranks, the string gives 5",
        ),
        (
            PROTECTION,
            "50003-0530213301301551-50203",
            "Shield Specialization has 2 ranks, the string gives 3",
        ),
        (
            PROTECTION,
            "-05302133013-0522533100103032",
            "Shield Specialization has 2 ranks, the string gives 3",
        ),
        (
            PROTECTION,
            "5500300300312125-05302133013-",
            "Infusion of Light has 2 ranks, the string gives 3",
        ),
    ] {
        let refused = refusal(fixture, old);
        assert!(refused.contains(reason), "{old}: {refused}");
        prepared(fixture, old, Rules::GoReference);
    }
}

/// A talent above its ranks in push 113554 is refused, and so is a tree longer than its 17
/// talents (Protection had 16).
#[test]
fn forever_refuses_ranks_above_the_113554_caps() {
    for (talents, reason) in [
        ("0003", "Reverence has 2 ranks, the string gives 3"),
        (
            "000000000300",
            "Divine Precision has 2 ranks, the string gives 3",
        ),
        (
            "-00000003",
            "Shield Specialization has 2 ranks, the string gives 3",
        ),
        (
            "-0000000000000004",
            "Iron Creed has 3 ranks, the string gives 4",
        ),
        ("--0004", "Conviction has 3 ranks, the string gives 4"),
        (
            "--0000000000000000002",
            "tree 3 has 17 talents, the string gives 19",
        ),
        (
            "-000000000000000002",
            "tree 2 has 17 talents, the string gives 18",
        ),
        (
            "--00000000000000002",
            "Unsealed Wrath has 1 rank, the string gives 2",
        ),
    ] {
        let refused = refusal(RETRIBUTION, talents);
        assert!(refused.contains(reason), "{talents}: {refused}");
    }
    // Five ranks where there were two or three.
    prepared(
        RETRIBUTION,
        "5000500000000005--0000000000000005",
        Rules::Forever,
    );
}

/// Conviction 3/3 is A_MOD_CRIT_PCT, 6% crit with every spell and attack (curve 2/4/6); the
/// reference's 5/5 is 5% melee crit.
#[test]
fn conviction_is_crit_with_every_spell_and_attack() {
    let base = prepared(RETRIBUTION, "", Rules::Forever);
    let forever = prepared(RETRIBUTION, "--0003", Rules::Forever);
    let physical = stat(&forever, "PhysicalCritPercent") - stat(&base, "PhysicalCritPercent");
    let spell = stat(&forever, "SpellCritPercent") - stat(&base, "SpellCritPercent");
    assert!((physical - 6.0).abs() < 1e-9, "{physical}");
    assert!((spell - 6.0).abs() < 1e-9, "{spell}");

    let base = prepared(RETRIBUTION, "", Rules::GoReference);
    let reference = prepared(RETRIBUTION, "--00005", Rules::GoReference);
    let physical = stat(&reference, "PhysicalCritPercent") - stat(&base, "PhysicalCritPercent");
    let spell = stat(&reference, "SpellCritPercent") - stat(&base, "SpellCritPercent");
    assert!((physical - 5.0).abs() < 1e-9, "{physical}");
    assert_eq!(spell, 0.0);
}

/// Unsealed Wrath replaces Twist of Light under Forever's rules: its Echoes in upstream's order,
/// no 20% seal cost cut. Champion of the Light 5/5 is 60% of Intellect and Hammer of Wrath's cast
/// time 1 sec shorter (Instrument of Law's, which is gone); Vindication gives the paladin no
/// aura; Improved Seals 5/5 is 15% on Seal of Command.
#[test]
fn retribution_reads_the_reworked_talents() {
    let forever = prepared(RETRIBUTION, RET_113554, Rules::Forever);
    assert!(effect(&forever, "twist_of_light").is_none());
    let wrath = effect(&forever, "unsealed_wrath").expect("unsealed_wrath");
    assert_eq!(
        wrath["echoes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|echo| echo["seal"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "seal_of_command",
            "seal_of_fury",
            "seal_of_righteousness",
            "seal_of_justice"
        ]
    );
    let echo = forever["player"]["auras"]
        .as_array()
        .unwrap()
        .iter()
        .find(|aura| {
            aura["label"]
                .as_str()
                .unwrap()
                .starts_with("Echo (1311703)")
        })
        .expect("Echo of Command");
    assert_eq!(echo["max_stacks"], 3);
    assert_eq!(echo["duration_ns"], 15_000_000_000_i64);

    // The seals cost what Benediction 5/5 leaves: no Twist of Light cut.
    let seal_of_command = spell(&forever, 20920);
    assert_eq!(seal_of_command["cost"]["additive_percent_modifier"], 0.9);

    let hammer = spell(&forever, HAMMER_OF_WRATH);
    let reference_hammer = spell(
        &prepared(
            RETRIBUTION,
            "52003-503-05225231001330301",
            Rules::GoReference,
        ),
        HAMMER_OF_WRATH,
    )
    .clone();
    assert_eq!(
        reference_hammer["default_cast"]["cast_time_ns"]
            .as_i64()
            .unwrap()
            - hammer["default_cast"]["cast_time_ns"].as_i64().unwrap(),
        1_000_000_000
    );

    let vindication = effect(&forever, "vindication").unwrap();
    assert!(vindication.get("aura").is_none());

    // Improved Seals 5/5's 15% (was 3 ranks of 5%) on Seal of Command's weapon share.
    let weapon_percent = |prepared: &Value| {
        effect(prepared, "seal_of_command").unwrap()["weapon_percent"]
            .as_f64()
            .unwrap()
    };
    let without = prepared(
        RETRIBUTION,
        "050000002-503-05233301001301351",
        Rules::Forever,
    );
    let ratio = weapon_percent(&forever) / weapon_percent(&without);
    assert!((ratio - 1.15).abs() < 1e-9, "{ratio}");
}

/// Righteous Fury adds 33% to all threat under Forever's rules; Swift Judgement is a second
/// Judgement on a 60 sec cooldown; Shieldsworn takes 4 sec off Holy Strike's cooldown; Holy
/// Shield Rank 3 deals Rank 2's old 153 for Rank 2's old 195 mana; Redoubt, Shield
/// Specialization, Improved Righteous Fury and Iron Creed read their 113554 curves.
#[test]
fn protection_reads_the_reworked_talents() {
    let forever = prepared(PROTECTION, PROT_113554, Rules::Forever);
    let fury = effect(&forever, "righteous_fury").unwrap();
    assert_eq!(fury["all_threat_multiplier"], 1.33);

    assert!(effect(&forever, "swift_judgement").is_none());
    assert_eq!(
        effect(&forever, "second_judgement").unwrap()["spell_id"],
        SWIFT_JUDGEMENT
    );
    let swift = spell(&forever, SWIFT_JUDGEMENT);
    assert_eq!(swift["class_spell"], "second_judgement");
    assert_eq!(swift["cd"]["duration_ns"], 60_000_000_000_i64);
    assert_eq!(swift["default_cast"]["gcd_ns"], 0);

    assert!(effect(&forever, "shieldsworn").is_some());
    assert_eq!(
        spell(&forever, HOLY_STRIKE)["cd"]["duration_ns"],
        6_000_000_000_i64
    );

    let shield = effect(&forever, "holy_shield").unwrap();
    let rank = shield["ranks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|rank| rank["spell_id"] == HOLY_SHIELD_3)
        .unwrap();
    assert_eq!(rank["damage"], 153.0);
    assert_eq!(spell(&forever, HOLY_SHIELD_3)["cost"]["base_cost"], 195);

    let specialization = effect(&forever, "shield_specialization").unwrap();
    assert_eq!(specialization["proc_chance"], 1.0);
    let taken = effect(&forever, "pseudo_stat_auras").unwrap()["auras"].clone();
    let multiplier = |label: &str| {
        taken
            .as_array()
            .unwrap()
            .iter()
            .find(|aura| aura["aura"].as_str().unwrap().starts_with(label))
            .unwrap()["multiplier"]
            .as_f64()
            .unwrap()
    };
    assert!((multiplier("Righteous Fury") - 0.91).abs() < 1e-9);
    assert!((multiplier("Iron Creed") - 0.94).abs() < 1e-9);

    // The reference keeps the old talent and numbers.
    let reference = prepared(
        PROTECTION,
        "50003-0530213301301551-50203",
        Rules::GoReference,
    );
    assert!(effect(&reference, "swift_judgement").is_some());
    assert!(effect(&reference, "righteous_fury").unwrap()["all_threat_multiplier"].is_null());
    let shield = effect(&reference, "holy_shield").unwrap();
    assert_eq!(
        shield["ranks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|rank| rank["spell_id"] == HOLY_SHIELD_3)
            .unwrap()["damage"],
        221.0
    );
}

/// Holy Power 5/5 is A_MOD_CRITICAL_HEALING_AMOUNT: a heal crit's bonus 50% larger, 150% to 225%,
/// and no crit chance; Boundless Light 2/2 is 30% Holy Shock heal crit on an ally without the
/// paladin's Oath of Light (`healers.rs` casts the Oath). The reference's Holy Power is crit chance.
#[test]
fn holy_power_is_crit_healing_and_boundless_light_holy_shock_crit() {
    // Holy Shock and Boundless Light need Holy's first four rows.
    let talents = "0502521022102125";
    let forever = prepared(HOLY, talents, Rules::Forever);
    let heal = spell(&forever, HOLY_LIGHT);
    assert_eq!(heal["crit_multiplier_pct"], 1.5);
    assert_eq!(heal["bonus_crit_percent"], 0.0);
    let shock = spell(&forever, HOLY_SHOCK_HEAL);
    assert_eq!(shock["crit_multiplier_pct"], 1.5);
    assert_eq!(shock["bonus_crit_percent"], 30.0);

    let reference = prepared(HOLY, "25320213225131051-50323", Rules::GoReference);
    assert_eq!(spell(&reference, HOLY_LIGHT)["crit_multiplier_pct"], 1.0);
    assert_eq!(spell(&reference, HOLY_LIGHT)["bonus_crit_percent"], 5.0);
}

/// The Retribution board's twisting rotation with Unsealed Wrath: Echoes gain stacks and replay
/// their seals, so more Seal of Righteousness hits land than without the talent.
#[test]
fn unsealed_wrath_replays_stored_seals() {
    let hits = |talents: &str| {
        let prepared = prepare(&request(RETRIBUTION, talents), "rust").unwrap();
        check_prepared(&prepared).unwrap();
        let result = simulate_prepared(&prepared).unwrap().result;
        let actions = result["raidMetrics"]["parties"][0]["players"][0]["actions"]
            .as_array()
            .unwrap()
            .clone();
        let count = |id: i64| -> f64 {
            actions
                .iter()
                .filter(|action| action["id"]["spellId"] == id)
                .flat_map(|action| action["targets"].as_array().unwrap().clone())
                .map(|target| {
                    target["hits"].as_f64().unwrap_or(0.0) + target["crits"].as_f64().unwrap_or(0.0)
                })
                .sum()
        };
        (count(25713), count(20424))
    };
    let (with_righteousness, with_command) = hits(RET_113554);
    let without = RET_113554.trim_end_matches('1');
    let (righteousness, command) = hits(without);
    assert!(
        with_righteousness + with_command > righteousness + command,
        "{with_righteousness} + {with_command} vs {righteousness} + {command}"
    );
}
