//! Client 1.60.1.70707's Mage tree (TraitTree 1112), which only Forever's rules prepare: Arcane
//! Focus removed and the Arcane tree at 18 talents, Improved Fire Blast in the Fire tree's third
//! row, Critical Mass at 3 ranks and Incineration at 5, and Precision (29438) as spell hit for
//! every school. The Go reference keeps the 70425 tree.

use forever_engine::{prepare_json_with, PrepareError, Rules};
use serde_json::{json, Value};
use std::{fs, path::Path};

const FIREBALL: i64 = 25306;
const SCORCH: i64 = 10207;

fn request(talents: &str) -> Vec<u8> {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/mage/prepared-v2/frost-reference.request.json"
    ));
    let mut request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    request["raid"]["parties"][0]["players"][0]["talentsString"] = Value::from(talents);
    serde_json::to_vec(&request).unwrap()
}

fn prepared(talents: &str, rules: Rules) -> Value {
    prepare_json_with(&request(talents), "rust", rules)
        .unwrap_or_else(|err| panic!("{talents}: {err:?}"))
}

fn refusal(talents: &str, rules: Rules) -> String {
    match prepare_json_with(&request(talents), "rust", rules) {
        Err(PrepareError::Refused(refusal)) => {
            assert_eq!(refusal.code, "talents", "{talents}");
            refusal.reason
        }
        other => panic!("{talents}: {:?}", other.map(|_| ())),
    }
}

fn bonus_crit(prepared: &Value, spell_id: i64) -> f64 {
    prepared["player"]["spells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|spell| spell["action_id"]["spell_id"] == spell_id)
        .unwrap_or_else(|| panic!("no spell {spell_id}"))["bonus_crit_percent"]
        .as_f64()
        .unwrap()
}

/// The string reads in the client's order under Forever's rules: Improved Channeling in Arcane
/// Focus's old square, Arcane Resilience in Improved Channeling's, Missile Barrage in the second
/// row, Improved Fire Blast after Pyroblast. The Go reference reads the same digits by Go's field
/// numbers.
#[test]
fn forever_reads_the_70707_layout_and_the_reference_its_own() {
    let talents = "0520002-000000002-003";
    assert_eq!(
        prepared(talents, Rules::Forever)["player"]["talents"],
        json!({
            "improved_channeling": 5,
            "arcane_resilience": 2,
            "missile_barrage": 2,
            "improved_fire_blast": 2,
            "elemental_precision": 3,
        })
    );
    assert_eq!(
        prepared(talents, Rules::GoReference)["player"]["talents"],
        json!({
            "arcane_focus": 5,
            "improved_channeling": 2,
            "arcane_concentration": 2,
            "elemental_precision": 3,
        })
    );
}

/// A 19 digit Arcane tree from before 70707 is one talent too long for Forever's tree; the Go
/// reference still reads it.
#[test]
fn a_pre_70707_arcane_string_is_refused_under_forever_rules() {
    let old = "0502205003100311531-03-05350003";
    let reason = refusal(old, Rules::Forever);
    assert!(
        reason.contains("tree 1 has 18 talents, the string gives 19"),
        "{reason}"
    );
    assert_eq!(
        prepared(old, Rules::GoReference)["player"]["talents"]["arcane_focus"],
        5
    );
}

/// Critical Mass has 3 ranks (TraitNodeEntry 130513) and Incineration 5 (130525) under Forever's
/// rules; a string above either is refused, as is any talent above its ranks in 70707.
#[test]
fn forever_refuses_ranks_above_the_70707_caps() {
    for (talents, reason) in [
        (
            "-55000000000004",
            "Critical Mass has 3 ranks, the string gives 4",
        ),
        (
            "-55000000000005",
            "Critical Mass has 3 ranks, the string gives 5",
        ),
        ("-65", "Incineration has 5 ranks, the string gives 6"),
        ("--006", "Precision has 3 ranks, the string gives 6"),
        // An old string read in the new order: Arcane Concentration's 5 lands on Missile
        // Barrage.
        (
            "05012050030000-03-053500000101301251",
            "Missile Barrage has 2 ranks, the string gives 5",
        ),
        ("--0000000002", "Ice Lance has 1 rank, the string gives 2"),
    ] {
        let refused = refusal(talents, Rules::Forever);
        assert!(refused.contains(reason), "{talents}: {refused}");
    }
    // The reference's Critical Mass keeps its five ranks.
    prepared("-03500000000005", Rules::GoReference);
}

/// Critical Mass 3/3 is 6% Fire crit, Incineration 5/5 another 10% on Scorch (curves 83083 and
/// 83095). The reference's 5/5 Critical Mass and 3/3 Incineration give 10% and 6%.
#[test]
fn critical_mass_and_incineration_read_the_70707_ranks() {
    let forever = prepared("-55000000000003", Rules::Forever);
    assert_eq!(forever["player"]["talents"]["incineration"], 5);
    assert_eq!(forever["player"]["talents"]["critical_mass"], 3);
    assert_eq!(bonus_crit(&forever, FIREBALL), 6.0);
    assert_eq!(bonus_crit(&forever, SCORCH), 16.0);

    let reference = prepared("-03500000000005", Rules::GoReference);
    assert_eq!(bonus_crit(&reference, FIREBALL), 10.0);
    assert_eq!(bonus_crit(&reference, SCORCH), 16.0);
}

/// Arcane Focus is gone under Forever's rules, and Precision 3/3 (SpellEffect 704780, aura 55) is
/// 6% spell hit for every magic school, Arcane among them. The reference keeps Arcane Focus's 5%
/// on Arcane and Precision's 6% on Fire and Frost only.
#[test]
fn precision_is_hit_for_every_school_and_arcane_focus_is_gone() {
    let hit =
        |prepared: &Value| prepared["player"]["pseudo_stats"]["school_bonus_hit_chance"].clone();

    let forever = prepared("05-0-003", Rules::Forever);
    assert!(forever["player"]["talents"].get("arcane_focus").is_none());
    assert_eq!(forever["player"]["talents"]["improved_channeling"], 5);
    assert_eq!(
        hit(&forever),
        json!({"arcane": 6.0, "fire": 6.0, "frost": 6.0, "holy": 6.0, "nature": 6.0,
            "none": 0.0, "physical": 0.0, "shadow": 6.0})
    );

    let reference = prepared("05-0-003", Rules::GoReference);
    assert_eq!(reference["player"]["talents"]["arcane_focus"], 5);
    assert_eq!(
        hit(&reference),
        json!({"arcane": 5.0, "fire": 6.0, "frost": 6.0, "holy": 0.0, "nature": 0.0,
            "none": 0.0, "physical": 0.0, "shadow": 0.0})
    );
}
