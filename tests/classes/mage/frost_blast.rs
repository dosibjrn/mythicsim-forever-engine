//! Frost Blast (1326053 to 1326055, MythicSim patch 153), a trained Frost spell of client 70425
//! that casts as Frostbolt does. Go's exporter lists no damage row for it, so under the
//! reference's rules a rotation that casts it is refused; under Forever's rules Rust describes
//! each rank's roll. It is one of Go's `MageSpellsAllDamaging`, so it spends a Fingers of Frost
//! charge as the bolts do: missing from that list, it kept Shatter's crit up and read 51% crit
//! where Go's engine reads 30% on a production Arcane request.

use std::{fs, path::Path};

use forever_engine::{contracts::prepared_v2::PreparedV2, prepared_coverage, Rules};
use serde_json::{json, Value};

const FROST_BLAST: i64 = 1326055;
const FINGERS_OF_FROST: &str = "{SpellID: 400669}";

/// The Frost fixture casting nothing but Frost Blast, with Fingers of Frost talented: the talents
/// in Go's order for the reference, in client 1.60.1.70707's for Forever's rules (Arcane Focus's
/// points on Improved Channeling, which takes its square).
fn request(rules: Rules) -> Vec<u8> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/classes/mage/prepare/frost.request.json");
    let mut request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let player = &mut request["raid"]["parties"][0]["players"][0];
    player["talentsString"] = json!(match rules {
        Rules::Forever => "05020520300030053--0535020001013012",
        Rules::GoReference => "050220500300030053--0535020001013012",
    });
    player["rotation"] = json!({
        "type": "TypeAPL",
        "priorityList": [{"action": {"castSpell": {"spellId": {"spellId": FROST_BLAST}}}}],
    });
    request["simOptions"]["iterations"] = json!(1);
    request["simOptions"]["debugFirstIteration"] = json!(true);
    serde_json::to_vec(&request).unwrap()
}

fn frost_blast(prepared: &Value) -> &Value {
    prepared["player"]["spells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|spell| spell["action_id"]["spell_id"] == FROST_BLAST)
        .expect("Frost Blast is registered")
}

#[test]
fn frost_blast_is_described_under_forever_rules_only() {
    let go = forever_engine::prepare_json_with(
        &request(Rules::GoReference),
        "frost-blast",
        Rules::GoReference,
    )
    .unwrap();
    assert!(frost_blast(&go)["damage_effect"].is_null());
    let forever =
        forever_engine::prepare_json_with(&request(Rules::Forever), "frost-blast", Rules::Forever)
            .unwrap();
    assert!(
        frost_blast(&forever)["damage_effect"]["average"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    let prepared: PreparedV2 = serde_json::from_value(forever).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
}

#[test]
fn frost_blast_spends_fingers_of_frost_charges() {
    let forever =
        forever_engine::prepare_json_with(&request(Rules::Forever), "frost-blast", Rules::Forever)
            .unwrap();
    let prepared: PreparedV2 = serde_json::from_value(forever).unwrap();
    let report = forever_engine::simulate_prepared(&prepared).unwrap();
    let logs = report.result["logs"].as_str().unwrap();
    let granted = logs
        .lines()
        .filter(|line| line.contains(&format!("{FINGERS_OF_FROST} stacks: 0 --> 2")))
        .count();
    let spent = logs
        .lines()
        .filter(|line| line.contains(&format!("{FINGERS_OF_FROST} stacks: 2 --> 1")))
        .count();
    assert!(granted > 0, "no Fingers of Frost in the fight");
    // The launching blast spends none; each later Frost Blast spends one on impact.
    assert!(spent > 0, "granted {granted} times, never spent");
}
