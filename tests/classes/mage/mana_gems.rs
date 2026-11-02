//! Mana gems and Demonic Rune share one 2 minute cooldown in Forever. The client
//! (1.60.1.70532, ItemEffect joined to ItemXItemEffect) gives Mana Agate (5405), Mana Jade
//! (10052), Mana Citrine (10057), Mana Ruby (10058), Demonic Rune (16666) and Dark Rune (27869)
//! spell category 1153 with a 120,000 ms category cooldown; the Major Mana Potion (17531) is
//! category 4. The Go reference puts the gems on category 30, so a mage used a gem and a rune
//! back to back; `Rules::GoReference` keeps that.

use std::{fs, path::Path};

use forever_engine::{contracts::prepared_v2::PreparedV2, Rules};
use serde_json::{json, Value};

const GEMS: [i64; 4] = [5514, 5513, 8007, 8008];
const DEMONIC_RUNE: i64 = 12662;
const TWO_MINUTES: f64 = 120.0;

fn arcane(rules: Rules) -> Value {
    let request = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/mage/prepared-v2/arcane-reference.request.json"),
    )
    .unwrap();
    let mut request: Value = serde_json::from_slice(&request).unwrap();
    if rules == Rules::Forever {
        // The fixture's talents in client 1.60.1.70707's order, Arcane Focus's points dropped.
        request["raid"]["parties"][0]["players"][0]["talentsString"] =
            json!("050205103100301531-305000012-003");
    }
    forever_engine::prepare_json_with(&serde_json::to_vec(&request).unwrap(), "gems", rules)
        .unwrap()
}

fn item_spell(prepared: &Value, item: i64) -> &Value {
    prepared["player"]["spells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|spell| spell["action_id"]["item_id"] == item)
        .unwrap_or_else(|| panic!("no spell for item {item}"))
}

/// One 300 second fight with no variation; the times at which each gem and the rune were used.
fn conjured_uses(mut prepared: Value) -> Vec<(f64, i64)> {
    prepared["sim"]["iterations"] = json!(1);
    prepared["sim"]["debug_first_iteration"] = json!(true);
    prepared["encounter"]["duration_ns"] = json!(300_000_000_000_i64);
    prepared["encounter"]["duration_variation_ns"] = json!(0);
    let prepared: PreparedV2 = serde_json::from_value(prepared).unwrap();
    let report = forever_engine::simulate_prepared(&prepared).unwrap();
    let logs = report.result["logs"].as_str().unwrap();
    let mut uses = Vec::new();
    for line in logs
        .lines()
        .filter(|line| line.contains("] Completed cast {ItemID: "))
    {
        let time: f64 = line[1..line.find(']').unwrap()].parse().unwrap();
        let item: i64 = line[line.find("{ItemID: ").unwrap() + 9..line.rfind('}').unwrap()]
            .parse()
            .unwrap();
        if GEMS.contains(&item) || item == DEMONIC_RUNE {
            uses.push((time, item));
        }
    }
    uses
}

fn set_priority(prepared: &mut Value, item: i64, priority: i64) {
    let cooldown = prepared["player"]["major_cooldowns"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|cooldown| cooldown["action_id"]["item_id"] == item)
        .unwrap();
    cooldown["priority"] = json!(priority);
}

#[test]
fn forever_gems_use_the_rune_category_timer() {
    let forever = arcane(Rules::Forever);
    let rune = &item_spell(&forever, DEMONIC_RUNE)["shared_cd"];
    assert_eq!(rune["duration_ns"], json!(120_000_000_000_i64));
    for gem in GEMS {
        let cd = &item_spell(&forever, gem)["cd"];
        assert_eq!(cd["timer"], rune["timer"], "gem {gem}");
        assert_eq!(cd["duration_ns"], json!(120_000_000_000_i64), "gem {gem}");
    }
    // The reference keeps the gems on a timer of their own.
    let go = arcane(Rules::GoReference);
    let rune = &item_spell(&go, DEMONIC_RUNE)["shared_cd"];
    for gem in GEMS {
        assert_ne!(
            item_spell(&go, gem)["cd"]["timer"],
            rune["timer"],
            "gem {gem}"
        );
    }
}

/// A gem blocks the rune for 2 minutes: the gems go first (their priority is their mana), and
/// every later use, gem or rune, waits the whole category cooldown.
#[test]
fn a_gem_blocks_the_rune_for_two_minutes() {
    let uses = conjured_uses(arcane(Rules::Forever));
    assert!(uses.len() >= 2, "{uses:?}");
    assert!(GEMS.contains(&uses[0].1), "{uses:?}");
    for pair in uses.windows(2) {
        assert!(pair[1].0 - pair[0].0 >= TWO_MINUTES, "{uses:?}");
    }
}

/// The rune first, used once at 1 second (its own cooldown set past the fight): with the
/// reference's rules a gem follows as soon as the mana is down; with Forever's, not for 2
/// minutes. The rune is timed rather than waiting on the mana deficit, so how fast the ranks a
/// player knows spend mana does not decide which comes first.
fn rune_first(rules: Rules) -> Vec<(f64, i64)> {
    let mut prepared = arcane(rules);
    set_priority(&mut prepared, DEMONIC_RUNE, 10_000);
    prepared["player"]["major_cooldowns"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|cooldown| cooldown["action_id"]["item_id"] == DEMONIC_RUNE)
        .unwrap()["timings_ns"] = json!([1_000_000_000_i64]);
    let spells = prepared["player"]["spells"].as_array_mut().unwrap();
    let rune = spells
        .iter_mut()
        .find(|spell| spell["action_id"]["item_id"] == DEMONIC_RUNE)
        .unwrap();
    rune["cd"]["duration_ns"] = json!(600_000_000_000_i64);
    let uses = conjured_uses(prepared);
    assert_eq!(uses[0].1, DEMONIC_RUNE, "{uses:?}");
    assert!(GEMS.contains(&uses[1].1), "{uses:?}");
    uses
}

#[test]
fn the_rune_blocks_the_gems_for_two_minutes() {
    let uses = rune_first(Rules::Forever);
    for pair in uses.windows(2) {
        assert!(pair[1].0 - pair[0].0 >= TWO_MINUTES, "{uses:?}");
    }
    let uses = rune_first(Rules::GoReference);
    assert!(uses[1].0 - uses[0].0 < TWO_MINUTES, "{uses:?}");
}

/// Under the reference's rules the gem and the rune keep separate cooldowns, so the rune
/// follows the first gem well inside 2 minutes, as Go does.
#[test]
fn go_rules_use_a_gem_and_the_rune_back_to_back() {
    let uses = conjured_uses(arcane(Rules::GoReference));
    let gem = uses.iter().find(|(_, item)| GEMS.contains(item)).unwrap();
    let rune = uses.iter().find(|(_, item)| *item == DEMONIC_RUNE).unwrap();
    assert!((rune.0 - gem.0).abs() < TWO_MINUTES, "{uses:?}");
}
