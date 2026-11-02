//! `sim --rotation-stats` against Go goldens: the rotation the application shows and Go's
//! validation messages of each of its lines (fixtures/rotation-stats/README.md).
//!
//! The golden comparison runs under the Go reference's rules (`--go-rules`), as the goldens do:
//! Forever's rules read a rotation's Ahn'Qiraj 20 book ranks (Corruption VII, Serpent Sting IX
//! and the like) as the rank below and the fixtures' talents in the 70707 client's order, so the
//! rotation they show names other spell ids than Go's.

use serde_json::Value;
use std::{fs, path::Path, process::Command};

const CASES: [&str; 14] = [
    "apl-shadow-cooldown-reads",
    "shadow-priest-casts-at-no-unit",
    "beast-mastery-hunter-nested-groups",
    "combat-rogue-group-referenced-twice",
    "affliction-warlock-groups-unused-and-unfound",
    "apl-shadow-schedule",
    "apl-warrior-prepull-stat-buffs",
    "apl-warrior-cast-all-stat-buff-cooldowns",
    "production-warrior-prepull-move-dropped",
    "fire-mage-3-targets-multidot-flamestrike",
    "affliction-warlock-3-targets-multidot-dots",
    "arms-warrior-sweeping-strikes",
    "apl-warlock-assigned-agony",
    "shadow-priest-line-checks",
];

const KEYS: [&str; 3] = [
    "effectiveRotation",
    "rotationStats",
    "rotationPresentationVersion",
];

fn run(name: &str, rotation_stats: bool, go_rules: bool) -> Value {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let own = root.join(format!("rotation-stats/{name}.request.json"));
    let request = if own.exists() {
        own
    } else {
        root.join(format!("mage/prepared-v2/{name}.request.json"))
    };
    let mut request: Value = serde_json::from_str(&fs::read_to_string(request).unwrap()).unwrap();
    request["simOptions"] = serde_json::json!({"iterations": 1, "randomSeed": "1"});
    let directory = std::env::temp_dir().join(format!(
        "forever-rotation-stats-{}-{name}-{rotation_stats}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let input = directory.join("request.json");
    let output = directory.join("result.json");
    fs::write(&input, request.to_string()).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_forever-engine"));
    command
        .args(["sim", "--scenario", "rotation-stats", "--gate", "--request"])
        .arg(&input)
        .arg("--outfile")
        .arg(&output);
    if rotation_stats {
        command.arg("--rotation-stats");
    }
    if go_rules {
        command.arg("--go-rules");
    }
    let done = command.output().unwrap();
    assert!(
        done.status.success(),
        "{name}: {}{}",
        String::from_utf8_lossy(&done.stdout),
        String::from_utf8_lossy(&done.stderr)
    );
    let report: Value = serde_json::from_str(&fs::read_to_string(&output).unwrap()).unwrap();
    fs::remove_dir_all(directory).unwrap();
    report["result"]["raidMetrics"]["parties"][0]["players"][0].clone()
}

#[test]
fn rotation_line_checks_match_go() {
    let mut failures = Vec::new();
    for name in CASES {
        let golden_file = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("fixtures/rotation-stats/{name}.go.json"));
        let golden: Value =
            serde_json::from_str(&fs::read_to_string(golden_file).unwrap()).unwrap();
        let player = run(name, true, true);
        for key in KEYS {
            if player[key] != golden[key] {
                failures.push(format!(
                    "{name} {key}:\n  go   {}\n  rust {}",
                    golden[key], player[key]
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn line_checks_are_reported_only_on_request() {
    let player = run("apl-shadow-cooldown-reads", false, false);
    for key in KEYS {
        assert!(player.get(key).is_none(), "{key} without --rotation-stats");
    }
}
