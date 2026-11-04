//! The Shaman's talent strings under Forever's rules, which read the launch client's order
//! (`prepare::talent_layout`, Wowhead's talent calculator of 4 November 2026): Enhancement's
//! Improved Ghost Wolf moves from the second row to the fourth, after Stormstrike, and the seven
//! talents between shift one square earlier. Elemental and Restoration keep their positions. The
//! Go reference keeps the proto's order.

use forever_engine::{prepare_json_with, PrepareError, Rules};
use serde_json::{json, Value};
use std::{fs, path::Path};

fn request(fixture: &str, talents: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/mage/prepared-v2/{fixture}.request.json"));
    let mut request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    request["raid"]["parties"][0]["players"][0]["talentsString"] = json!(talents);
    request
}

fn talents(talents: &str, rules: Rules) -> Value {
    let request = request("production-enhancement-shaman", talents);
    let prepared = prepare_json_with(request.to_string().as_bytes(), "rust", rules)
        .unwrap_or_else(|err| panic!("{talents}: {err:?}"));
    prepared["player"]["talents"].clone()
}

/// A talent's points; a talent past the end of the string is not in the message.
fn rank(talents: &Value, field: &str) -> i64 {
    talents[field].as_i64().unwrap_or(0)
}

fn refusal(talents: &str, rules: Rules) -> String {
    let request = request("production-enhancement-shaman", talents);
    match prepare_json_with(request.to_string().as_bytes(), "rust", rules) {
        Err(PrepareError::Refused(refusal)) => {
            assert_eq!(refusal.code, "talents");
            refusal.reason
        }
        other => panic!("{talents}: {:?}", other.map(|_| ())),
    }
}

/// Improved Ghost Wolf is the thirteenth Enhancement talent, after Stormstrike. In the reference's
/// order the same position is Stormstrike's.
#[test]
fn improved_ghost_wolf_moves_to_the_fourth_row() {
    // Stormstrike (twelfth) and Improved Ghost Wolf (thirteenth), with nothing else.
    let forever = talents("-0000000000012", Rules::Forever);
    assert_eq!(forever["stormstrike"], 1);
    assert_eq!(rank(&forever, "improved_ghost_wolf"), 2);
    assert_eq!(rank(&forever, "improved_lightning_shield"), 0);

    // The sixth position is Improved Lightning Shield now, and Improved Ghost Wolf's old place.
    let forever = talents("-000003", Rules::Forever);
    assert_eq!(rank(&forever, "improved_lightning_shield"), 3);
    assert_eq!(rank(&forever, "improved_ghost_wolf"), 0);
    let reference = talents("-000003", Rules::GoReference);
    assert_eq!(rank(&reference, "improved_ghost_wolf"), 3);
    assert_eq!(rank(&reference, "improved_lightning_shield"), 0);
}

/// An old string that puts too many points in a talent of the new order is refused with the
/// reason, where it would otherwise read as a different build.
#[test]
fn an_old_enhancement_string_that_does_not_fit_is_refused() {
    // Elemental Weapons 3 in the old seventh place is Shamanistic Focus's, which has one rank.
    let reason = refusal("-00000003", Rules::Forever);
    assert!(
        reason.contains("Shamanistic Focus has 1 rank, the string gives 3"),
        "{reason}"
    );
    // Flurry 5 in the old twelfth place is Stormstrike's.
    let reason = refusal("-000000000005", Rules::Forever);
    assert!(
        reason.contains("Stormstrike has 1 rank, the string gives 5"),
        "{reason}"
    );
    // The reference reads both in its own order.
    talents("-00000003", Rules::GoReference);
    talents("-000000000005", Rules::GoReference);
    // The app's reference build of the Elemental shaman predates Lava Burst's square: it gives
    // three points to the talent now called Call of Thunder, which has one rank.
    let reason = refusal("5500301320123051-050002-50305", Rules::Forever);
    assert!(
        reason.contains("Call of Thunder has 1 rank, the string gives 3"),
        "{reason}"
    );
}

/// The strings of the live boards and reference builds, re-encoded into the new order, read as
/// the old ones did in the old order. The pairs are (old, new).
#[test]
fn re_encoded_strings_read_as_the_old_ones_did() {
    for (old, new) in [
        // The boards: Elemental, Elehance and Enhancement.
        (
            "5501331020011351-055000001-05301",
            "5501331020011351-05500001-05301",
        ),
        (
            "253233102000134-053030031004102-",
            "253233102000134-053030310041002-",
        ),
        (
            "453003102-055030031005102251",
            "453003102-055030310051002251",
        ),
        // The app's reference build of the Enhancement shaman.
        ("2505131-055030031005112251", "2505131-055030310051012251"),
    ] {
        assert_eq!(
            talents(old, Rules::GoReference),
            talents(new, Rules::Forever),
            "{old} against {new}"
        );
    }
}

/// Elemental keeps Lava Burst's square, now Call of Thunder, and Elemental Mastery's, now
/// Lightning Mastery; Restoration is unchanged, down to Riptide's.
#[test]
fn elemental_and_restoration_keep_their_positions() {
    let elemental = talents("0000000000001100", Rules::Forever);
    assert_eq!(elemental["lava_burst"], 1);
    assert_eq!(elemental["lightning_overload"], 1);
    let elemental = talents("0000000000000001", Rules::Forever);
    assert_eq!(elemental["elemental_mastery"], 1);
    let restoration = talents("--0000000000000001", Rules::Forever);
    assert_eq!(restoration["riptide"], 1);
    assert_eq!(
        restoration,
        talents("--0000000000000001", Rules::GoReference)
    );
}
