//! The rows of client 1.60.1.70707 that change what the engine computes, which only Forever's
//! rules prepare (the frozen spell data is client 1.60.1.70425, and the Go reference keeps it).

use forever_engine::{prepare_json_with, Rules};
use serde_json::Value;
use std::{fs, path::Path};

fn request(name: &str) -> Value {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/mage/prepared-v2"
    ))
    .join(format!("{name}.request.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn prepared(request: &Value, rules: Rules) -> Value {
    prepare_json_with(request.to_string().as_bytes(), "rust", rules).unwrap()
}

fn has_aura(prepared: &Value, spell_id: i64) -> bool {
    prepared["player"]["auras"]
        .as_array()
        .unwrap()
        .iter()
        .any(|aura| aura["action_id"]["spell_id"] == spell_id)
}

/// Trueshot Aura rank 4 (20905) dropped from 75 to 40 ranged Attack Power in 70707 (SpellEffect
/// 698474), below rank 5's (20906) 50, so a level 60 Hunter's aura is rank 5. The Go reference
/// keeps rank 4 at the frozen data's 75.
#[test]
fn trueshot_aura_is_rank_5() {
    let request = request("production-marksmanship-hunter");
    let forever = prepared(&request, Rules::Forever);
    let reference = prepared(&request, Rules::GoReference);
    assert!(has_aura(&forever, 20906));
    assert!(!has_aura(&forever, 20905));
    assert!(has_aura(&reference, 20905));
    let rap = |prepared: &Value| {
        prepared["player"]["stats"]["RangedAttackPower"]
            .as_f64()
            .unwrap()
    };
    // 25 from Trueshot Aura; 26 more from the raid's Grace of Air Totem, rank II (77 Agility)
    // under Forever's rules for rank III's 89, as rank III is an Ahn'Qiraj 20 book.
    assert_eq!(rap(&reference) - rap(&forever), 51.0);
}
