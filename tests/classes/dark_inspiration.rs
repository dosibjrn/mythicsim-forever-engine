//! A Shadow Priest's Dark Inspiration in the party (`PartyBuffs.dark_inspiration`, the talent's
//! rank 0 to 5), a Rust-only party buff from wowsims/forever ce4dd1a69. No Go golden exists, so
//! these tests pin the buff row's numbers: rank r is round(15 * r / 5)% Spirit (spell 1326168),
//! for the whole fight, on the player alone and never its pets, and the spirit regeneration the
//! fight reads follows the Spirit.

use forever_engine::prepare_json;
use serde_json::{json, Value};
use std::{fs, path::Path};

fn fixture(path: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// The request with the party's Dark Inspiration at `rank`.
fn prepare(mut request: Value, rank: i32) -> Value {
    let party = &mut request["raid"]["parties"][0];
    if party["buffs"].is_null() {
        party["buffs"] = json!({});
    }
    party["buffs"]["darkInspiration"] = json!(rank);
    prepare_json(&serde_json::to_vec(&request).unwrap(), "dark-inspiration").unwrap()
}

fn spirit(prepared: &Value) -> f64 {
    prepared["player"]["stats"]["Spirit"].as_f64().unwrap()
}

fn spirit_regen(prepared: &Value) -> f64 {
    prepared["player"]["mana"]["spirit_regen_per_second"]
        .as_f64()
        .unwrap()
}

fn party_aura(prepared: &Value) -> Option<&Value> {
    prepared["player"]["auras"]
        .as_array()
        .unwrap()
        .iter()
        .find(|aura| aura["label"] == "Dark Inspiration (External)")
}

/// A Destruction Warlock with its Imp: 126 Spirit without the buff, floored from its unfloored
/// Spirit, and 3, 6, 9, 12 and 15% more at ranks 1 to 5, floored again. The ranks pin that Spirit
/// between 126.42 and 126.61. A Warlock regenerates 7.5 mana a second plus a tenth of its Spirit.
#[test]
fn the_partys_dark_inspiration_raises_a_casters_spirit_and_its_regen() {
    let request = fixture("tests/classes/warlock/prepare/destruction-imp.request.json");
    let without = prepare(request.clone(), 0);
    assert!(party_aura(&without).is_none());
    assert_eq!(spirit(&without), 126.0);
    assert!((spirit_regen(&without) - (7.5 + 12.6)).abs() < 1e-9);
    for (rank, want) in [(1, 130.0), (2, 134.0), (3, 137.0), (4, 141.0), (5, 145.0)] {
        let with = prepare(request.clone(), rank);
        let multiplier = 1.0 + (15.0 * f64::from(rank) / 5.0_f64).round() / 100.0;
        assert!(
            (126.0 * multiplier).floor() <= want && want <= (127.0 * multiplier).floor(),
            "rank {rank}: {want} is not 126 Spirit's share"
        );
        assert_eq!(spirit(&with), want, "rank {rank}");
        assert!(
            (spirit_regen(&with) - (7.5 + want / 10.0)).abs() < 1e-9,
            "rank {rank}: {} mana a second",
            spirit_regen(&with)
        );
        let aura = party_aura(&with).expect("the party's copy is on the player");
        assert_eq!(aura["active"], true);
        assert_eq!(aura["action_id"], json!({ "spell_id": 1326168, "tag": -1 }));
        // The Imp is the party's pet, which the buff leaves out.
        assert_eq!(
            with["pets"][0]["stats"], without["pets"][0]["stats"],
            "rank {rank}"
        );
        assert!(
            super::refusal_codes(with).is_empty(),
            "rank {rank} runs in Rust"
        );
    }
    // Past the talent's five ranks is rank 5.
    assert_eq!(spirit(&prepare(request, 9)), 145.0);
}

/// The Spirit a combination of the stat auras reads with the priest's own Dark Inspiration up,
/// None when it leaves the Spirit as it was.
fn spirit_under_own_copy(prepared: &Value) -> Option<f64> {
    let effect = prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == "stat_auras")
        .expect("the priest's own copy is a stat aura");
    assert_eq!(effect["auras"], json!(["Dark Inspiration"]));
    effect["combos"][1]["Spirit"].as_f64()
}

/// A Shadow Priest with 2 points in Dark Inspiration (6%) under another priest's party copy: the
/// two bid in one Spirit category, so the stronger holds the Spirit alone and the party's keeps
/// it on a tie (wowsims/forever TestGeneratedDarkInspirationAndAPriestsCopyDoNotStack).
#[test]
fn a_shadow_priests_own_dark_inspiration_does_not_stack_with_the_partys() {
    let request = fixture("tests/classes/priest/prepare/shadow-human-inner-fire.request.json");
    let alone = prepare(request.clone(), 0);
    assert_eq!(spirit(&alone), 181.0);
    // 181 Spirit is floored from at least 181, so 6% is 191.86 or more: 192.
    let own = spirit_under_own_copy(&alone).expect("the own copy raises the Spirit");
    assert_eq!(own, 192.0);

    // A weaker party copy (3%): the own copy replaces it, never multiplies it.
    let weaker = prepare(request.clone(), 1);
    assert_eq!(spirit(&weaker), 186.0);
    assert_eq!(spirit_under_own_copy(&weaker), Some(own));

    // An equal party copy (6%) keeps the Spirit, and a stronger one (9% and 15%) outbids the own.
    for (rank, want) in [(2, 192.0), (3, 197.0), (5, 208.0)] {
        let with = prepare(request.clone(), rank);
        assert_eq!(spirit(&with), want, "rank {rank}");
        assert_eq!(spirit_under_own_copy(&with), None, "rank {rank}");
    }
}
