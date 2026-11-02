//! Priest changes of live server hotfix push 113554 (30 October 2026, client 1.60.1.70707 by
//! DBCache), under Forever's rules, from upstream wowsims/forever #773: Holy Binding (1328569)
//! replaces the Binding Heal talent (401937) in the same field, so a priest string keeps its
//! digits. Each Flash Heal on another player heals the priest for 50% as much (1328570), and
//! Binding Heal's ranks leave the tree. The Go reference keeps Binding Heal.

use forever_engine::{check_prepared, prepare, prepare_json_with, simulate_prepared, Rules};
use serde_json::Value;
use std::{fs, path::Path};

/// Flash Heal's ranks and Binding Heal's.
const FLASH_HEAL: [i64; 7] = [2061, 9472, 9473, 9474, 10915, 10916, 10917];
const BINDING_HEAL: [i64; 6] = [401937, 1240770, 1240771, 1240772, 1240773, 1240774];

/// The healer fixture takes the talent: its Holy tree's twelfth digit is 1.
fn request() -> Vec<u8> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/healers/holy-priest.request.json");
    fs::read(path).unwrap()
}

fn heals(rules: Rules) -> Vec<Value> {
    let prepared = prepare_json_with(&request(), "rust", rules).unwrap();
    assert_eq!(prepared["player"]["talents"]["binding_heal"], 1);
    prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == "heals")
        .unwrap()["heals"]
        .as_array()
        .unwrap()
        .clone()
}

fn rank(heals: &[Value], spell_id: i64) -> Option<&Value> {
    heals.iter().find(|heal| heal["spell_id"] == spell_id)
}

#[test]
fn holy_binding_puts_half_of_each_flash_heal_on_the_priest() {
    let forever = heals(Rules::Forever);
    for id in FLASH_HEAL {
        let heal = rank(&forever, id).unwrap_or_else(|| panic!("Flash Heal {id}"));
        assert_eq!(heal["targets"], 2, "{id}");
        assert_eq!(heal["jump_multiplier"], 0.5, "{id}");
    }
    for id in BINDING_HEAL {
        assert!(rank(&forever, id).is_none(), "Binding Heal {id}");
    }

    let reference = heals(Rules::GoReference);
    for id in FLASH_HEAL {
        let heal = rank(&reference, id).unwrap();
        assert_eq!(heal["targets"], 1, "{id}");
        assert_eq!(heal["jump_multiplier"], 1.0, "{id}");
    }
    for id in BINDING_HEAL {
        assert_eq!(
            rank(&reference, id).unwrap()["targets"],
            2,
            "Binding Heal {id}"
        );
    }
}

/// The healer sim still passes the gate and heals under Forever's rules with Holy Binding.
#[test]
fn the_healer_priest_sims_with_holy_binding() {
    let prepared = prepare(&request(), "rust").unwrap();
    check_prepared(&prepared).unwrap();
    let result = simulate_prepared(&prepared).unwrap().result;
    let hps = result["raidMetrics"]["parties"][0]["players"][0]["hps"]["avg"]
        .as_f64()
        .unwrap();
    assert!(hps > 0.0, "{hps}");
}
