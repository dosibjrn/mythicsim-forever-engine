//! Riptide and the Druid's Poised, from Blizzard's development notes of 2 November 2026, which
//! only Forever's rules prepare (the Go reference keeps the frozen data and its numbers), and the
//! Warrior talent strings that put more points in a talent than it has ranks, which are refused
//! with the reason in place of panicking.

use forever_engine::RngMode;
use forever_engine::{
    check_prepared, contracts::prepared_v2::PreparedV2, prepare_json_with, simulate_prepared,
    PrepareError, Rules,
};
use serde_json::{json, Value};
use std::{fs, path::Path};

#[path = "support/warrior_trees_20261104.rs"]
mod warrior_trees_20261104;
use warrior_trees_20261104::forever_talents as forever_order;

const RIPTIDE_RANKS: [(i64, i64, i64); 3] =
    [(408521, 245, 245), (1239242, 300, 300), (1239243, 230, 385)];
const RIPTIDE: i64 = 1239243;
/// The Druid's Poised aura (`prepare::feral`), the row of hotfix push 113578 with the discount.
const POISED: i64 = 1_328_827;
const RAVAGE: i64 = 9867;
/// The live reference builds' talents in Forever's order (the fixtures' strings are the old tree's,
/// which Forever's rules refuse).
const FERAL_CAT: &str = "050022-55000032023012125001-055";
const BALANCE: &str = "5532220115501351--055";

/// A production druid fixture on these talents.
fn druid(name: &str, talents: &str) -> Value {
    let mut request = production(name);
    player_mut(&mut request)["talentsString"] = json!(talents);
    request
}

fn fixture(path: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn production(name: &str) -> Value {
    fixture(&format!(
        "fixtures/mage/prepared-v2/production-{name}.request.json"
    ))
}

fn player_mut(request: &mut Value) -> &mut Value {
    &mut request["raid"]["parties"][0]["players"][0]
}

fn prepared(request: &Value, rules: Rules) -> Value {
    prepare_json_with(request.to_string().as_bytes(), "rust", rules)
        .unwrap_or_else(|err| panic!("{err:?}"))
}

fn effect<'a>(prepared: &'a Value, kind: &str) -> &'a Value {
    prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == kind)
        .unwrap_or_else(|| panic!("no {kind} effect"))
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

fn simulate(prepared: &Value) -> Value {
    let prepared: PreparedV2 = serde_json::from_value(prepared.clone()).unwrap();
    check_prepared(&prepared).unwrap();
    simulate_prepared(&prepared, RngMode::Stochastic).unwrap().result
}

/// The log of the first fight of a prepared request.
fn first_fight_log(request: &Value, rules: Rules) -> String {
    let mut value = prepared(request, rules);
    value["sim"]["iterations"] = json!(1);
    value["sim"]["debug_first_iteration"] = json!(true);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    check_prepared(&prepared).unwrap();
    let report = simulate_prepared(&prepared, RngMode::Stochastic).unwrap();
    report.result["logs"].as_str().unwrap().to_string()
}

/// The Restoration shaman reference build's request, casting only Riptide on the player.
fn riptide_request() -> Value {
    let mut request = fixture("fixtures/healers/restoration-shaman.request.json");
    let player = player_mut(&mut request);
    player["talentsString"] = json!("-055-5531500015513151");
    player["rotation"] = json!({"type": "TypeAPL", "priorityList": [
        {"action": {"castSpell": {
            "spellId": {"spellId": RIPTIDE}, "target": {"type": "Self"}
        }}},
    ]});
    request["simOptions"]["iterations"] = json!(40);
    request
}

/// What Riptide did in every fight together, summed over its targets.
fn sum(result: &Value, key: &str) -> f64 {
    result["raidMetrics"]["parties"][0]["players"][0]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|action| action["id"]["spellId"] == RIPTIDE)
        .flat_map(|action| action["targets"].as_array().unwrap().iter())
        .map(|target| target[key].as_f64().unwrap_or(0.0))
        .sum()
}

/// Riptide costs what Wowhead's tooltips state on 4 November (245, 300 and 230 mana: rank 3 is
/// the notes' "about 40%" less, ranks 1 and 2 read their old costs), waits 12 sec (was 6) and has
/// no initial heal: its heal row carries only the heal over time (89, 115 and 161 every 3 sec,
/// which Wowhead's buff tooltips state too), and the Go reference keeps the whole frozen spell.
#[test]
fn riptide_is_a_cheaper_slower_heal_over_time() {
    let request = riptide_request();
    let forever = prepared(&request, Rules::Forever);
    let reference = prepared(&request, Rules::GoReference);
    for (id, mana, frozen) in RIPTIDE_RANKS {
        assert_eq!(spell(&forever, id)["cost"]["base_cost"], mana, "{id}");
        assert_eq!(spell(&reference, id)["cost"]["base_cost"], frozen, "{id}");
        assert_eq!(spell(&forever, id)["cd"]["duration_ns"], 12_000_000_000i64);
        assert_eq!(spell(&reference, id)["cd"]["duration_ns"], 6_000_000_000i64);
    }
    let heal = |prepared: &Value| -> Value {
        effect(prepared, "heals")["heals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|heal| heal["spell_id"] == RIPTIDE)
            .unwrap()
            .clone()
    };
    let (forever_heal, reference_heal) = (heal(&forever), heal(&reference));
    assert!(forever_heal["direct"].is_null(), "{forever_heal}");
    assert_eq!(forever_heal["tick"], 161.0);
    assert_eq!(reference_heal["direct"]["average"], 841.0);
    assert_eq!(reference_heal["tick"], 161.0);
    // The Chain Heal bonus on the Riptide holder stays: its third effect is not the heal.
    let chain = effect(&forever, "heals")["heals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|heal| heal["spell_id"] == 10623)
        .map(|heal| heal["hot_bonus"]["multiplier"].clone());
    assert_eq!(chain, Some(json!(1.25)));
    // Every rank's heal over time is the row's: Wowhead's buff tooltips read "Heals 89 every 3
    // sec", 115, and 191 at rank 3's max level 68 (161 at level 60, plus 8 levels of 3.7).
    for (id, tick) in [(408521i64, 89.0), (1239242, 115.0), (RIPTIDE, 161.0)] {
        let rank = effect(&forever, "heals")["heals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|heal| heal["spell_id"] == id)
            .unwrap()
            .clone();
        assert_eq!(rank["tick"], tick, "{id}");
        assert!(rank["direct"].is_null(), "{id}");
    }
}

/// In the fights: a Riptide rotation casts half as often, and what it heals is its ticks alone,
/// about five a cast on each player (the 15 sec heal over time outlasts the 12 sec cooldown).
#[test]
fn riptide_casts_every_twelve_seconds_and_heals_only_over_time() {
    let request = riptide_request();
    let forever = simulate(&prepared(&request, Rules::Forever));
    let reference = simulate(&prepared(&request, Rules::GoReference));
    let (forever_casts, reference_casts) = (sum(&forever, "casts"), sum(&reference, "casts"));
    assert!(
        (forever_casts * 2.0 - reference_casts).abs() < reference_casts * 0.2,
        "{forever_casts} casts against {reference_casts}"
    );
    let ticks = sum(&forever, "ticks") + sum(&forever, "critTicks");
    assert!(
        ticks > 4.5 * forever_casts,
        "{ticks} ticks for {forever_casts} casts"
    );
    // Every point Riptide heals under Forever's rules comes from a tick (a direct heal would add
    // a hit): the reference heals more a cast, 841 up front.
    let reference_ticks = sum(&reference, "ticks") + sum(&reference, "critTicks");
    assert!(reference_ticks > 0.0);
    assert!(
        sum(&forever, "healing") / forever_casts < sum(&reference, "healing") / reference_casts,
        "a cast heals less without its direct heal"
    );
}

/// The Cat Form druid's first Energy ability costs 10 less once: Ravage from Prowl is 50 under
/// Forever's rules (Poised is up at the pull) and 60 under the reference.
#[test]
fn the_druids_poised_discounts_the_opener_once() {
    let forever = first_fight_log(&druid("feral-druid", FERAL_CAT), Rules::Forever);
    let reference = first_fight_log(&production("feral-druid"), Rules::GoReference);
    let ravage = |log: &str| {
        log.lines()
            .find(|line| line.contains(&format!("Casting {{SpellID: {RAVAGE}}}")))
            .unwrap()
            .to_string()
    };
    assert!(
        ravage(&forever).contains("Cost = 50.000"),
        "{}",
        ravage(&forever)
    );
    assert!(ravage(&reference).contains("Cost = 60.000"));
    assert!(forever.contains(&format!("Aura gained: {{SpellID: {POISED}}}")));
    assert!(forever.contains(&format!("Aura faded: {{SpellID: {POISED}}}")));
    assert!(!reference.contains(&POISED.to_string()));
    // Gone after the opener: no later Energy ability pays 10 less than its cost.
    let faded = forever
        .lines()
        .position(|line| line.contains(&format!("Aura faded: {{SpellID: {POISED}}}")))
        .unwrap();
    let after: Vec<&str> = forever
        .lines()
        .skip(faded)
        .filter(|line| line.contains(&format!("Casting {{SpellID: {RAVAGE}}}")))
        .collect();
    assert!(after.iter().all(|line| !line.contains("Cost = 50.000")));
}

/// The export names the aura, the discount and the Cat Form abilities that spend it, under
/// Forever's rules only, and a bear or a caster has none.
#[test]
fn the_druids_poised_effect_names_the_energy_abilities() {
    let forever = prepared(&druid("feral-druid", FERAL_CAT), Rules::Forever);
    let poised = effect(&forever, "poised");
    assert_eq!(poised["aura"], "Poised");
    assert_eq!(poised["cost_reduction"], 10);
    let mut names: Vec<&str> = poised["class_spells"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["claw", "ferocious_bite", "rake", "ravage", "rip", "shred"]
    );
    // The reference reads the fixture's own talents, written in Go's order.
    let reference = prepared(&production("feral-druid"), Rules::GoReference);
    assert!(reference["effects"]
        .as_array()
        .unwrap()
        .iter()
        .all(|effect| effect["kind"] != "poised"));
    let balance = prepared(&druid("balance-druid", BALANCE), Rules::Forever);
    assert!(balance["effects"]
        .as_array()
        .unwrap()
        .iter()
        .all(|effect| effect["kind"] != "poised"));
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

fn warrior_with(talents: &str) -> Value {
    let mut request = production("warrior");
    player_mut(&mut request)["talentsString"] = json!(talents);
    request
}

/// Community Warrior builds whose strings do not fit the proto's tree order panicked in the rank
/// ladders ("rank 5 in a ladder of 2 ranks"); they are refused with the reason. Of 509
/// benchmarked community builds, 35 give Fury 18 talents (the proto's tree has 17, the reference
/// refuses them; the client's Fury has 18 since the notes of 2 November, tests/warrior_trees_20261104.rs),
/// and seven put more points in a talent than it has ranks.
#[test]
fn a_warrior_string_that_does_not_fit_is_refused_not_a_panic() {
    // Community builds 12c39bd7 and d32b9aab: an 18 talent Fury string.
    for talents in [
        "30305013-250500035150310051-",
        "20305013-250500035151310051-",
    ] {
        let reason = refusal(&warrior_with(talents), Rules::GoReference);
        assert!(
            reason.contains("tree 2 has 17 talents, the string gives 18"),
            "{reason}"
        );
        assert!(reason.contains(talents), "{reason}");
        // Forever's Fury has 18 talents: the length is not what refuses them.
        if let Err(PrepareError::Refused(refusal)) = prepare_json_with(
            warrior_with(talents).to_string().as_bytes(),
            "rust",
            Rules::Forever,
        ) {
            assert!(!refusal.reason.contains("tree 2 has 17 talents"));
        }
    }
    // Community build 370a0012: Improved Execute (two ranks) at 3, after Arms points that all fit
    // their ranks.
    let reason = refusal(
        &warrior_with("203052030325041-05052003003201-"),
        Rules::GoReference,
    );
    assert!(
        reason.contains("Improved Execute has 2 ranks, the string gives 3"),
        "{reason}"
    );
    // Five points where the tree has two ranks, in each tree.
    let reason = refusal(&warrior_with("-00000000005"), Rules::Forever);
    assert!(
        reason.contains("Improved Execute has 2 ranks, the string gives 5"),
        "{reason}"
    );
    let reason = refusal(&warrior_with("--5"), Rules::Forever);
    assert!(
        reason.contains("Improved Bloodrage has 2 ranks, the string gives 5"),
        "{reason}"
    );
    // A one rank talent given two points is refused too, rather than read as untaken.
    let reason = refusal(&warrior_with("000000200"), Rules::Forever);
    assert!(
        reason.contains("Anger Management has 1 rank, the string gives 2"),
        "{reason}"
    );
}

/// A string within every talent's ranks prepares as before, under both rules.
#[test]
fn a_warrior_string_within_its_ranks_still_prepares() {
    for rules in [Rules::Forever, Rules::GoReference] {
        let mut request = production("warrior");
        if rules == Rules::Forever {
            // The fixture's talents are the old tree's; Forever's rules read the client's order.
            let old = player_mut(&mut request)["talentsString"]
                .as_str()
                .unwrap()
                .to_string();
            player_mut(&mut request)["talentsString"] = json!(forever_order(&old));
        }
        let value = prepared(&request, rules);
        assert!(value["player"]["spells"].as_array().unwrap().len() > 10);
    }
}
