//! The Shaman part of Blizzard's development notes of 2 November 2026, which only Forever's rules
//! prepare: Lightning Bolt and Chain Lightning by rank, the shocks back at Classic Era values,
//! and Lava Burst's talent slot giving Call of Thunder. The Go reference keeps the frozen data
//! (client 1.60.1.70425) and its numbers.

use forever_engine::{prepare_json_with, simulate_prepared, Rules};
use serde_json::{json, Value};
use std::{fs, path::Path};

fn request(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/mage/prepared-v2/{name}.request.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn prepared(request: &Value, rules: Rules) -> Value {
    prepare_json_with(request.to_string().as_bytes(), "rust", rules).unwrap()
}

/// The player's prepared spells of one class spell, in rank order.
fn spells<'a>(prepared: &'a Value, class_spell: &str) -> Vec<&'a Value> {
    prepared["player"]["spells"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|spell| spell["class_spell"] == class_spell)
        .collect()
}

/// (spell id, average damage at level 60) of each rank.
fn averages(prepared: &Value, class_spell: &str) -> Vec<(i64, f64)> {
    spells(prepared, class_spell)
        .iter()
        .map(|spell| {
            (
                spell["action_id"]["spell_id"].as_i64().unwrap(),
                spell["damage_effect"]["average"].as_f64().unwrap(),
            )
        })
        .collect()
}

fn base_costs(prepared: &Value, class_spell: &str) -> Vec<f64> {
    spells(prepared, class_spell)
        .iter()
        .map(|spell| spell["cost"]["base_cost"].as_f64().unwrap())
        .collect()
}

fn effect_kinds(prepared: &Value) -> Vec<String> {
    prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|effect| effect["kind"].as_str().unwrap().to_string())
        .collect()
}

/// Lightning Bolt's base damage on every rank, as Wowhead's Forever tooltips state the rows on 4
/// November 2026 (the notes: roughly 78% of the Classic Era rank, the maximum rank "increased by
/// 80%"): rank 10 is 353 base and 2.1 a level, 361 at level 60 (was 200).
#[test]
fn lightning_bolt_by_rank() {
    let request = request("production-elemental-shaman");
    let forever = prepared(&request, Rules::Forever);
    let reference = prepared(&request, Rules::GoReference);
    let ids = [403, 529, 548, 915, 943, 6041, 10391, 10392, 15207, 15208i64];
    let new = [
        16.0, 30.0, 53.0, 75.0, 112.0, 153.0, 199.0, 247.0, 303.0, 361.0,
    ];
    let old = [
        15.0, 27.0, 46.0, 63.0, 84.0, 115.0, 151.0, 167.0, 183.0, 200.0,
    ];
    let forever_bolts = averages(&forever, "lightning_bolt");
    let reference_bolts = averages(&reference, "lightning_bolt");
    for (rank, id) in ids.iter().enumerate() {
        assert_eq!(forever_bolts[rank], (*id, new[rank]), "rank {}", rank + 1);
        assert_eq!(reference_bolts[rank], (*id, old[rank]), "rank {}", rank + 1);
    }
    // The row's variance is a share of the average and does not change, so the roll of rank 10
    // spans 361 times 0.946 to 1.054 (Wowhead's tooltip, at its max level, reads 344 to 383).
    let roll = |prepared: &Value| {
        let spell = spells(prepared, "lightning_bolt")[9];
        let damage = &spell["damage_effect"];
        let (average, variance) = (
            damage["average"].as_f64().unwrap(),
            damage["variance"].as_f64().unwrap(),
        );
        (
            average * (1.0 - variance / 2.0),
            average * (1.0 + variance / 2.0),
        )
    };
    let (new_min, new_max) = roll(&forever);
    let (old_min, old_max) = roll(&reference);
    assert!((new_min / old_min - 361.0 / 200.0).abs() < 1e-9);
    assert!((new_max / old_max - 361.0 / 200.0).abs() < 1e-9);
    // The overload copy deals half of its rank: Lightning Overload's text says "half damage".
    let overloads = averages(&forever, "lightning_bolt_overload");
    assert_eq!(overloads[9], (15208, 180.0));
    for (rank, half) in [8.0, 15.0, 26.0, 37.0, 56.0, 76.0, 99.0, 123.0, 151.0, 180.0]
        .into_iter()
        .enumerate()
    {
        assert_eq!(overloads[rank].1, half, "overload of rank {}", rank + 1);
    }
    assert_eq!(
        averages(&reference, "lightning_bolt_overload")[9],
        (15208, 100.0)
    );
    // The cost and the cast time are not in the notes.
    assert_eq!(
        base_costs(&forever, "lightning_bolt"),
        base_costs(&reference, "lightning_bolt")
    );
}

/// Chain Lightning's cast time (2.5 sec, was 2) and mana cost (about 225% of the comparable
/// Lightning Bolt rank), which the notes state and Wowhead's tooltips still state as 2 sec and
/// 145, 200, 250 and 315 mana, and its primary target damage as Wowhead states the rows on 4
/// November 2026: rank 4 is 510 base and 3.3 a level, 523 at level 60 (was 126).
#[test]
fn chain_lightning_by_rank() {
    let request = request("production-elemental-shaman");
    let forever = prepared(&request, Rules::Forever);
    let reference = prepared(&request, Rules::GoReference);

    assert_eq!(
        averages(&forever, "chain_lightning"),
        [(421, 222.0), (930, 359.0), (2860, 441.0), (10605, 523.0)]
    );
    assert_eq!(
        averages(&reference, "chain_lightning"),
        [(421, 91.0), (930, 103.0), (2860, 115.0), (10605, 126.0)]
    );
    assert_eq!(
        averages(&forever, "chain_lightning_overload"),
        [(421, 111.0), (930, 179.0), (2860, 220.0), (10605, 261.0)]
    );
    assert_eq!(
        base_costs(&forever, "chain_lightning"),
        [248.0, 304.0, 428.0, 495.0]
    );
    assert_eq!(
        base_costs(&reference, "chain_lightning"),
        [145.0, 200.0, 250.0, 315.0]
    );
    // The fixture's Elemental Alacrity takes half a second off either cast.
    let cast_ns = |prepared: &Value| {
        spells(prepared, "chain_lightning")
            .iter()
            .map(|spell| spell["default_cast"]["cast_time_ns"].as_i64().unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(cast_ns(&forever), [2_000_000_000; 4]);
    assert_eq!(cast_ns(&reference), [1_500_000_000; 4]);
    for spell in spells(&forever, "chain_lightning") {
        assert_eq!(spell["bonus_coefficient"], 0.714);
    }
    for spell in spells(&forever, "chain_lightning_overload") {
        assert_eq!(spell["bonus_coefficient"], 0.357);
    }

    // Mana is about 225% of the comparable Lightning Bolt ranks 6, 7, 9 and 10, which the notes
    // state. Damage is no longer a share of Lightning Bolt's: Wowhead's rows put rank 2 well above
    // 145% (348 base against the 189 the notes' ratio gave).
    let bolt_costs = base_costs(&forever, "lightning_bolt");
    let chain_costs = base_costs(&forever, "chain_lightning");
    for (chain, bolt) in [0, 1, 2, 3].into_iter().zip([5, 6, 8, 9]) {
        let mana = chain_costs[chain] / bolt_costs[bolt];
        assert!((mana - 2.25).abs() < 0.01, "mana {mana}");
    }
}

/// The shocks at Classic Era values, as Wowhead's Forever tooltips state the rows on 4 November
/// 2026: Earth Shock rank 7 531 base (301 before), Flame Shock rank 6 292 and 320 over 12 sec
/// (166 and 176), Frost Shock rank 4 506 base (283 before).
#[test]
fn shocks_are_classic_era() {
    let request = request("production-elemental-shaman");
    let forever = prepared(&request, Rules::Forever);
    let reference = prepared(&request, Rules::GoReference);

    assert_eq!(averages(&forever, "earth_shock"), [(10414, 531.0)]);
    assert_eq!(averages(&reference, "earth_shock"), [(10414, 301.0)]);
    assert_eq!(averages(&forever, "flame_shock_direct"), [(29228, 292.0)]);
    assert_eq!(averages(&reference, "flame_shock_direct"), [(29228, 166.0)]);
    // Frost Shock keeps Forever's level scaling on top of the Classic rank: 509 at level 60
    // against Classic's 506.
    assert_eq!(averages(&forever, "frost_shock"), [(10473, 509.0)]);
    assert_eq!(averages(&reference, "frost_shock"), [(10473, 286.0)]);
    let tick = |prepared: &Value| {
        prepared["effects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|effect| effect["kind"] == "flame_shock")
            .unwrap()["tick_base"]
            .clone()
    };
    assert_eq!(tick(&forever), 80.0);
    assert_eq!(tick(&reference), 44.0);
    // Cost, cooldown and coefficient are unchanged.
    for shock in ["earth_shock", "flame_shock_direct", "frost_shock"] {
        assert_eq!(base_costs(&forever, shock), base_costs(&reference, shock));
        assert_eq!(
            spells(&forever, shock)[0]["bonus_coefficient"],
            spells(&reference, shock)[0]["bonus_coefficient"]
        );
    }
}

/// Lava Burst is removed and its talent slot gives Call of Thunder: 5% critical strike chance for
/// Lightning Bolt and Chain Lightning, overloads included. A talent string keeps its positions, so
/// a Lava Burst point now means Call of Thunder, and a build without it gets nothing.
#[test]
fn lava_burst_becomes_call_of_thunder() {
    let request = request("production-elemental-shaman");
    let forever = prepared(&request, Rules::Forever);
    let reference = prepared(&request, Rules::GoReference);

    assert!(spells(&forever, "lava_burst").is_empty());
    assert!(!effect_kinds(&forever).contains(&"lava_burst".to_string()));
    assert_eq!(spells(&reference, "lava_burst").len(), 1);
    assert!(effect_kinds(&reference).contains(&"lava_burst".to_string()));

    // The talent's point is position 13 of the Elemental tree.
    let mut without = request.clone();
    let talents = without["raid"]["parties"][0]["players"][0]["talentsString"]
        .as_str()
        .unwrap()
        .to_string();
    let (elemental, rest) = talents.split_once('-').unwrap();
    assert_eq!(
        elemental.as_bytes()[12],
        b'1',
        "the fixture takes the talent"
    );
    let mut tree = elemental.as_bytes().to_vec();
    tree[12] = b'0';
    without["raid"]["parties"][0]["players"][0]["talentsString"] =
        json!(format!("{}-{}", String::from_utf8(tree).unwrap(), rest));
    let without = prepared(&without, Rules::Forever);

    let crit = |prepared: &Value, class_spell: &str| {
        spells(prepared, class_spell)
            .iter()
            .map(|spell| spell["bonus_crit_percent"].as_f64().unwrap())
            .collect::<Vec<_>>()
    };
    for class_spell in [
        "lightning_bolt",
        "lightning_bolt_overload",
        "chain_lightning",
        "chain_lightning_overload",
    ] {
        let with = crit(&forever, class_spell);
        let none = crit(&without, class_spell);
        let old = crit(&reference, class_spell);
        for ((with, none), old) in with.iter().zip(&none).zip(&old) {
            assert_eq!(with - none, 5.0, "{class_spell}");
            assert_eq!(*none, *old, "{class_spell}");
        }
    }
    // Nothing else gains the crit.
    for class_spell in ["earth_shock", "flame_shock_direct", "fire_nova"] {
        assert_eq!(crit(&forever, class_spell), crit(&without, class_spell));
    }
}

/// A rotation line that names Lava Burst finds no spell under Forever's rules and is dropped:
/// the run equals the same rotation without the line, and Lava Burst never fires. The reference
/// still casts it.
#[test]
fn a_rotation_line_naming_lava_burst_is_dropped() {
    const LAVA_BURST: i64 = 1238300;
    let mut request = request("production-elemental-shaman");
    request["simOptions"]["iterations"] = json!(200);
    let named = |entry: &Value| {
        entry["action"]["castSpell"]["spellId"]["spellId"].as_i64() == Some(LAVA_BURST)
    };
    let mut trimmed = request.clone();
    let list = trimmed["raid"]["parties"][0]["players"][0]["rotation"]["priorityList"]
        .as_array_mut()
        .unwrap();
    let before = list.len();
    list.retain(|entry| !named(entry));
    assert_eq!(list.len(), before - 1, "the fixture names Lava Burst once");

    let dps = |request: &Value, rules: Rules| {
        let prepared = serde_json::from_value(prepared(request, rules)).unwrap();
        let report = simulate_prepared(&prepared).unwrap();
        let player = report.result["raidMetrics"]["parties"][0]["players"][0].clone();
        let lava_burst_casts = player["actions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|action| {
                matches!(
                    action["id"]["spellId"].as_i64(),
                    Some(408490 | 1238299 | 1238300 | 1326047)
                )
            })
            .count();
        (player["dps"]["avg"].as_f64().unwrap(), lava_burst_casts)
    };
    let (with_line, lava_burst) = dps(&request, Rules::Forever);
    let (without_line, _) = dps(&trimmed, Rules::Forever);
    assert_eq!(lava_burst, 0);
    assert_eq!(with_line, without_line);
    let (_, reference_lava_burst) = dps(&request, Rules::GoReference);
    assert!(reference_lava_burst > 0);
}
