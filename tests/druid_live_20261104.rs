//! The Druid numbers of Blizzard's development notes of 2 November 2026 as the live client states
//! them on 4 November (Wowhead's Forever tooltips and spell pages, which decode the launch rows),
//! which only Forever's rules prepare. The Go reference keeps the frozen data (client
//! 1.60.1.70425) and its numbers.

use forever_engine::{prepare_json_with, Rules};
use serde_json::{json, Value};
use std::{fs, path::Path};

/// The live reference builds' talents in Forever's order (the fixtures' strings are the old tree's,
/// which Forever's rules refuse).
const FERAL_CAT: &str = "050022-55000032023012125001-055";
const BALANCE: &str = "5532220115501351--055";

fn production(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "fixtures/mage/prepared-v2/production-{name}.request.json"
    ));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// A production druid fixture on these talents.
fn druid(name: &str, talents: &str) -> Value {
    let mut request = production(name);
    request["raid"]["parties"][0]["players"][0]["talentsString"] = json!(talents);
    request
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

fn number(value: &Value) -> f64 {
    value.as_f64().unwrap()
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-5, "{a} against {b}");
}

/// Rip's tooltip reads 305, 497, 688, 880 and 1071 damage over 12 sec at 1 to 5 combo points
/// (0 attack power), six ticks of 19 plus 31.9 a point; the frozen row gives 243 to 855.
#[test]
fn rip_reproduces_its_live_tooltip() {
    let forever = prepared(&druid("feral-druid", FERAL_CAT), Rules::Forever);
    let rip = effect(&forever, "rip");
    let tick =
        |points: f64| number(&rip["tick_base"]) + number(&rip["tick_per_combo_point"]) * points;
    for (points, tooltip) in [
        (1.0, 305.0),
        (2.0, 497.0),
        (3.0, 688.0),
        (4.0, 880.0),
        (5.0, 1071.0),
    ] {
        assert_eq!(
            (6.0 * tick(points)).round(),
            tooltip,
            "{points} combo points"
        );
    }
    // The buff tooltip, "19 + (32 * Combo Points)", is the same tick.
    assert_eq!(number(&rip["tick_base"]), 19.0);
    assert_eq!(number(&rip["tick_per_combo_point"]).round(), 32.0);
    // The attack power share, 1% a point up to four, takes the notes' 25% (no row states it); the
    // placeholder multiplier on the whole spell is gone.
    close(number(&rip["attack_power_share_per_combo_point"]), 0.0125);
    assert_eq!(number(&rip["attack_power_share_max_points"]), 4.0);
    assert_eq!(number(&spell(&forever, 9896)["damage_multiplier"]), 1.0);

    let reference = prepared(&production("feral-druid"), Rules::GoReference);
    let rip = effect(&reference, "rip");
    assert_eq!(number(&rip["tick_base"]), 15.0);
    assert_eq!(number(&rip["tick_per_combo_point"]), 25.5);
    assert_eq!(number(&rip["attack_power_share_per_combo_point"]), 0.01);
    assert_eq!(number(&spell(&reference, 9896)["damage_multiplier"]), 1.0);
}

/// Rake's tooltip reads "76 damage plus 126 over 9 sec": a hit of 76 and three ticks of 42 (the
/// frozen row gives 61 and 3 times 34).
#[test]
fn rake_reproduces_its_live_tooltip() {
    let forever = prepared(&druid("feral-druid", FERAL_CAT), Rules::Forever);
    let rake = effect(&forever, "rake");
    assert_eq!(number(&rake["flat_damage"]), 76.0);
    assert_eq!(3.0 * number(&rake["tick_base"]), 126.0);
    close(number(&rake["attack_power_share"]), 0.0625);
    assert_eq!(number(&spell(&forever, 9904)["damage_multiplier"]), 1.0);

    let reference = prepared(&production("feral-druid"), Rules::GoReference);
    let rake = effect(&reference, "rake");
    assert_eq!(number(&rake["flat_damage"]), 61.0);
    assert_eq!(3.0 * number(&rake["tick_base"]), 102.0);
    assert_eq!(number(&rake["attack_power_share"]), 0.05);
}

/// Ferocious Bite's tooltip reads 245-321, 426-502, 607-683, 788-864 and 969-1045 damage at 1 to 5
/// combo points and 3.3 damage for each extra point of Energy; the frozen row gives 787-847 at five
/// points and 2.7 an Energy. The roll is the row's average and spread, the combo points add 181
/// each.
#[test]
fn ferocious_bite_reproduces_its_live_tooltip() {
    let forever = prepared(&druid("feral-druid", FERAL_CAT), Rules::Forever);
    let bite = effect(&forever, "ferocious_bite");
    let roll = &spell(&forever, 31018)["damage_effect"];
    let (average, variance) = (number(&roll["average"]), number(&roll["variance"]));
    let (low, high) = (
        average * (1.0 - variance / 2.0),
        average * (1.0 + variance / 2.0),
    );
    let per_point = number(&bite["damage_per_combo_point"]);
    for (points, from, to) in [
        (1.0, 245.0, 321.0),
        (2.0, 426.0, 502.0),
        (3.0, 607.0, 683.0),
        (4.0, 788.0, 864.0),
        (5.0, 969.0, 1045.0),
    ] {
        assert_eq!(
            (low + per_point * points).round(),
            from,
            "{points} combo points, low"
        );
        assert_eq!(
            (high + per_point * points).round(),
            to,
            "{points} combo points, high"
        );
    }
    close(number(&bite["damage_per_energy"]), 3.3);
    close(number(&bite["attack_power_per_combo_point"]), 0.0375);
    assert_eq!(number(&spell(&forever, 31018)["damage_multiplier"]), 1.0);

    let reference = prepared(&production("feral-druid"), Rules::GoReference);
    let bite = effect(&reference, "ferocious_bite");
    let roll = &spell(&reference, 31018)["damage_effect"];
    assert_eq!(number(&roll["average"]), 82.0);
    assert_eq!(number(&bite["damage_per_combo_point"]), 147.0);
    assert_eq!(number(&bite["damage_per_energy"]), 2.7);
    assert_eq!(number(&bite["attack_power_per_combo_point"]), 0.03);
    let (average, variance) = (number(&roll["average"]), number(&roll["variance"]));
    let (low, high) = (
        average * (1.0 - variance / 2.0),
        average * (1.0 + variance / 2.0),
    );
    assert_eq!(
        ((low + 735.0).round(), (high + 735.0).round()),
        (787.0, 847.0)
    );
}

/// Shred, Claw and Ravage state 200%, 140% and 435% weapon damage on their live spell pages and
/// the notes agree, over the frozen row's flat damage (80, 115 and 98) that the tooltips of Shred
/// and Claw print as "plus 80" and "plus 115".
#[test]
fn the_builders_keep_the_live_weapon_shares() {
    let forever = prepared(&druid("feral-druid", FERAL_CAT), Rules::Forever);
    for (id, share) in [(9830, 2.0), (9850, 1.4), (9867, 4.35)] {
        close(number(&spell(&forever, id)["damage_multiplier"]), share);
    }
    let builders = effect(&forever, "cat_builders")["builders"]
        .as_array()
        .unwrap();
    let flat = |kind: &str| {
        builders
            .iter()
            .find(|builder| builder["kind"] == kind)
            .map(|builder| number(&builder["flat_damage"]))
            .unwrap()
    };
    assert_eq!(
        (flat("shred"), flat("claw"), flat("ravage")),
        (80.0, 115.0, 98.0)
    );
    let reference = prepared(&production("feral-druid"), Rules::GoReference);
    for (id, share) in [(9830, 1.55), (9850, 1.1), (9867, 3.5)] {
        close(number(&spell(&reference, id)["damage_multiplier"]), share);
    }
}

/// Starfire at level 60, every rank the engine prepares: the live base points and points per
/// level, capped at each rank's own maximum level (rank 1 stops at 25). The frozen rows give 87
/// up to 381.
#[test]
fn starfire_takes_the_live_rows_on_every_rank() {
    let forever = prepared(&druid("balance-druid", BALANCE), Rules::Forever);
    let reference = prepared(&production("balance-druid"), Rules::GoReference);
    // (spell, Forever, frozen) average at level 60.
    let ranks = [
        (2912, 122.0, 87.0),
        (8949, 166.0, 119.0),
        (8950, 214.0, 153.0),
        (8951, 292.0, 208.0),
        (9875, 390.0, 279.0),
        (9876, 477.0, 341.0),
        (25298, 533.0, 381.0),
    ];
    for (id, live, frozen) in ranks {
        let average = |prepared: &Value| number(&spell(prepared, id)["damage_effect"]["average"]);
        assert_eq!(average(&forever), live, "Starfire {id}");
        assert_eq!(average(&reference), frozen, "Starfire {id}, frozen");
    }
    // The spread is the row's, unchanged.
    let variance = |prepared: &Value| number(&spell(prepared, 25298)["damage_effect"]["variance"]);
    assert_eq!(variance(&forever), variance(&reference));
}

/// Poised keeps its behaviour under the real id: the aura is the row of push 113578 (1328827).
#[test]
fn poised_uses_the_clients_ids() {
    let forever = prepared(&druid("feral-druid", FERAL_CAT), Rules::Forever);
    assert_eq!(effect(&forever, "poised")["cost_reduction"], 10);
    let text = serde_json::to_string(&forever).unwrap();
    assert!(!text.contains("1399003"));
}
