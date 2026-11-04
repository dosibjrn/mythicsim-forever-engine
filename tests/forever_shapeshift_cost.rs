//! Forever's flat shapeshift cost items (Varolv, 4 November 2026): the Feral PvP grips (Shapeshift
//! Cost Reduction, 459594, 150 mana) and Idol of Shifting Tides (Clever Shapeshifter, 1270470, 40
//! mana) lower the mana cost of Cat, Bear and Moonkin Form and of Shifting Power, whose tooltip
//! says its cost "is reduced by effects that reduce the cost of Shapeshifting". The flat amount
//! comes off before Natural Shapeshifter's percentage. Go's pinned engine registers neither item.

use forever_engine::{
    check_prepared, contracts::prepared_v2::PreparedV2, prepare_json_with, simulate_prepared, Rules,
};
use serde_json::Value;
use std::{fs, path::Path};

const CAT_FORM: i64 = 768;
const SHIFTING_POWER: i64 = 1322605;
const HANDS: usize = 6;
const RANGED: usize = 13;
/// Blood Guard's Dragonhide Grips.
const GRIPS: i64 = 227180;
/// Idol of Shifting Tides.
const IDOL: i64 = 263411;
/// The live Feral board's talents in client 1.60.1.70707's order, without Natural Shapeshifter.
const TALENTS: &str = "050022-55000032023012125001-055";
/// The same with Natural Shapeshifter 3/3 (Restoration's fifth square).
const TALENTS_NS: &str = "050022-55000032023012125001-05503";

fn request(talents: &str, hands: Option<i64>, ranged: Option<i64>) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/production-feral-druid.request.json");
    let mut request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let player = &mut request["raid"]["parties"][0]["players"][0];
    player["talentsString"] = Value::from(talents);
    let items = &mut player["equipment"]["items"];
    if let Some(id) = hands {
        items[HANDS] = serde_json::json!({ "id": id });
    }
    if let Some(id) = ranged {
        items[RANGED] = serde_json::json!({ "id": id });
    }
    request
}

fn prepared(request: &Value, rules: Rules) -> Value {
    prepare_json_with(request.to_string().as_bytes(), "rust", rules)
        .unwrap_or_else(|err| panic!("{err:?}"))
}

fn spell(prepared: &Value, spell_id: i64) -> &Value {
    prepared["player"]["spells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|spell| {
            spell["action_id"]["spell_id"] == spell_id
                && spell["action_id"]["tag"].as_i64().unwrap_or(0) == 0
        })
        .unwrap_or_else(|| panic!("no spell {spell_id}"))
}

/// The flat cost modifier and the cost a cast pays.
fn cost(prepared: &Value, spell_id: i64) -> (i64, f64) {
    let spell = spell(prepared, spell_id);
    (
        spell["cost"]["flat_modifier"].as_i64().unwrap(),
        spell["default_cast"]["cost"].as_f64().unwrap(),
    )
}

#[test]
fn the_grips_and_the_idol_lower_shapeshift_and_shifting_power_costs() {
    let bare = prepared(&request(TALENTS, None, None), Rules::Forever);
    let grips = prepared(&request(TALENTS, Some(GRIPS), None), Rules::Forever);
    let idol = prepared(&request(TALENTS, None, Some(IDOL)), Rules::Forever);
    let both = prepared(&request(TALENTS, Some(GRIPS), Some(IDOL)), Rules::Forever);
    for id in [CAT_FORM, SHIFTING_POWER] {
        let (flat, full) = cost(&bare, id);
        assert_eq!(flat, 0, "{id}");
        assert_eq!(cost(&grips, id), (-150, full - 150.0), "{id}");
        assert_eq!(cost(&idol, id), (-40, full - 40.0), "{id}");
        assert_eq!(cost(&both, id), (-190, full - 190.0), "{id}");
    }
}

/// Natural Shapeshifter's 30% applies to what is left after the flat reduction, as the client
/// applies spell mods (flat, then percent): Varolv found the idol's 40 taken off before the talent.
#[test]
fn the_flat_reduction_comes_before_natural_shapeshifter() {
    let talent = prepared(&request(TALENTS_NS, None, None), Rules::Forever);
    let both = prepared(
        &request(TALENTS_NS, Some(GRIPS), Some(IDOL)),
        Rules::Forever,
    );
    for id in [CAT_FORM, SHIFTING_POWER] {
        let spell = spell(&both, id);
        let base = spell["cost"]["base_cost"].as_f64().unwrap();
        let percent = spell["cost"]["additive_percent_modifier"].as_f64().unwrap();
        assert!((percent - 0.7).abs() < 1e-9, "{id}: {percent}");
        let (_, without) = cost(&talent, id);
        let (flat, with) = cost(&both, id);
        assert_eq!(flat, -190, "{id}");
        assert!(
            (without - base * 0.7).abs() < 1e-6,
            "{id}: {without} {base}"
        );
        assert!(
            (with - (base - 190.0) * 0.7).abs() < 1e-6,
            "{id}: {with} {base}"
        );
    }
}

/// The Go reference has no effect for either item, so its rules leave the costs alone.
#[test]
fn the_reference_rules_ignore_them() {
    let reference = prepared(
        &request("-55000032021", Some(GRIPS), Some(IDOL)),
        Rules::GoReference,
    );
    assert_eq!(cost(&reference, CAT_FORM).0, 0);
}

/// The Feral rotation runs with both and casts Shifting Power at least as often.
#[test]
fn a_feral_with_both_casts_shifting_power_at_least_as_often() {
    let casts = |request: &Value| {
        let prepared: PreparedV2 =
            serde_json::from_value(self::prepared(request, Rules::Forever)).unwrap();
        check_prepared(&prepared).unwrap_or_else(|error| panic!("{error:?}"));
        let result = simulate_prepared(&prepared).unwrap().result;
        let player = &result["raidMetrics"]["parties"][0]["players"][0];
        let total: f64 = player["actions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|action| action["id"]["spellId"] == SHIFTING_POWER)
            .flat_map(|action| action["targets"].as_array().unwrap().iter())
            .map(|target| target["casts"].as_f64().unwrap_or(0.0))
            .sum();
        total
    };
    // Same rotation, same casts or more: the cast is never refused for mana it now has.
    let bare = casts(&request(TALENTS, None, None));
    let both = casts(&request(TALENTS, Some(GRIPS), Some(IDOL)));
    assert!(bare > 0.0, "Shifting Power casts");
    assert!(both >= bare, "{both} against {bare}");
}
