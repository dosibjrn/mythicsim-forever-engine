//! Client hotfix push 112753 (the Go fork's patches 186 to 188): the Frost tree at 18 talents,
//! Elemental Precision at three ranks, and Ice Barrier's shield and Frigid Power.

use std::{fs, path::PathBuf};

use forever_engine::{PrepareError, Rules};
use serde_json::Value;

fn frost_request(talents: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/frost-reference.request.json");
    let mut request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    request["raid"]["parties"][0]["players"][0]["talentsString"] = Value::from(talents);
    serde_json::to_vec(&request).unwrap()
}

/// Go `checkTalentsString` and NewMage's Elemental Precision check panic, so the sim fails;
/// Rust refuses the same strings under `talents`, and the request falls back to Go's error. The
/// strings are in Go's order, which the reference reads (Forever's rules read client
/// 1.60.1.70707's, tests/client_70707_mage.rs).
#[test]
fn pre_hotfix_mage_talent_strings_are_refused() {
    for (talents, reason) in [
        // The live frost-mage reference before the hotfix: field 55 is gone.
        (
            "0502105003-03-0555000301001301251",
            "tree 3 has 18 talents, the string gives 19",
        ),
        (
            "0502105003-03-055500030100130125",
            "Elemental Precision has 3 ranks, the string gives 5",
        ),
        (
            "05022050001001234-235500001301350310-003",
            "tree 2 has 17 talents, the string gives 18",
        ),
        ("0-0-0-0", "4 trees, a mage has 3"),
    ] {
        match forever_engine::prepare_json_with(
            &frost_request(talents),
            "talents",
            Rules::GoReference,
        ) {
            Err(PrepareError::Refused(refusal)) => {
                assert_eq!(refusal.code, "talents", "{talents}");
                assert!(
                    refusal.reason.contains(reason),
                    "{talents}: {}",
                    refusal.reason
                );
            }
            other => panic!("{talents}: {:?}", other.map(|_| ())),
        }
    }
    forever_engine::prepare_json_with(
        &frost_request("0502105003-03-053502030101301251"),
        "talents",
        Rules::GoReference,
    )
    .expect("the re-encoded reference prepares");
}

/// Preparation describes Ice Barrier as Go's exporter does: the level 60 absorb of rank 4, Frost
/// Warding's 10% a point on it, and Frost damage on both auras: 3% in hotfix push 112753's rows,
/// 5% on the shield and 15% on Frigid Power since push 113531 (tests/hotfix_113554.rs). The
/// talents are in client 1.60.1.70707's order, which Forever's rules read.
#[test]
fn ice_barrier_effect_reads_the_hotfix_rows() {
    for (talents, warding) in [
        ("050105203-3-053502030101301251", 1.0),
        ("050105203-3-153502030101301251", 1.1),
        ("050105203-3-253502030101301251", 1.2),
    ] {
        let prepared =
            forever_engine::prepare_json(&frost_request(talents), "ice-barrier").unwrap();
        let effect = prepared["effects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|effect| effect["kind"] == "ice_barrier")
            .expect("an ice_barrier effect");
        assert_eq!(effect["spell_id"], 13033);
        assert_eq!(effect["aura"], "Ice Barrier");
        assert_eq!(effect["frigid_power_aura"], "Frigid Power");
        assert_eq!(effect["absorb_base"], 819.0);
        assert!((effect["warding"].as_f64().unwrap() - warding).abs() < 1e-12);
        for (share, value) in [
            ("hit", 0.05),
            ("dot", 0.05),
            ("frigid_hit", 0.15),
            ("frigid_dot", 0.15),
        ] {
            assert_eq!(effect[share], value, "{share}");
        }
    }
    // Without the talent there is no shield to describe.
    let prepared = forever_engine::prepare_json(
        &frost_request("050105203-3-05350203010130125"),
        "no-barrier",
    )
    .unwrap();
    assert!(!prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|effect| effect["kind"] == "ice_barrier"));
}
