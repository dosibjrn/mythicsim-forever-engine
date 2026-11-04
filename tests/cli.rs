use std::{fs, process::Command};

#[test]
fn invalid_input_returns_nonzero_and_does_not_write_a_report() {
    let directory =
        std::env::temp_dir().join(format!("forever-rust-cli-test-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let input = directory.join("request.json");
    let output = directory.join("result.json");
    fs::write(&input, "{\"raid\":{}}").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_forever-engine"))
        .args(["sim", "--infile"])
        .arg(input)
        .arg("--outfile")
        .arg(&output)
        .output()
        .unwrap();
    let report_exists = output.exists();
    fs::remove_dir_all(directory).unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("request rejected"));
    assert!(!report_exists);
}

#[test]
fn version_identifies_prototype_and_source_revision() {
    let result = Command::new(env!("CARGO_BIN_EXE_forever-engine"))
        .arg("version")
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains(forever_engine::SOURCE_REVISION));
}

#[test]
fn shaman_l20_item_options_cli_returns_pinned_client_legal_candidates() {
    let result = Command::new(env!("CARGO_BIN_EXE_forever-engine"))
        .args([
            "item-options",
            "--class",
            "shaman",
            "--level",
            "20",
            "--spec",
            "unrestricted",
            "--slot",
            "all",
        ])
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let response: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(response["class"], "shaman");
    assert_eq!(response["level"], 20);
    assert_eq!(response["status"], "partial_level_unknown");
    assert_eq!(response["required_level_supported"], true);
    assert!(response["unverified_candidate_count"].as_u64().unwrap() > 0);
    assert!(response["source"]
        .as_str()
        .unwrap()
        .contains("client:SkillRaceClassInfo"));
    let items = response["items"].as_array().unwrap();
    assert!(!items.is_empty());
    assert!(items.iter().all(|item| item["required_level"].as_i64().unwrap() <= 20));
    assert!(items.iter().all(|item| {
        item["class_allowlist"].as_array().unwrap().is_empty()
            || item["class_allowlist"]
                .as_array()
                .unwrap()
                .iter()
                .any(|class| class == "ClassShaman")
    }));
    assert!(items.iter().any(|item| item["armor_type"] == "ArmorTypeLeather"));
    assert!(items.iter().any(|item| item["armor_type"] == "ArmorTypeCloth"));
    assert!(items.iter().all(|item| {
        item["armor_type"] != "ArmorTypeMail" && item["armor_type"] != "ArmorTypePlate"
    }));
    assert!(items.iter().any(|item| {
        item["weapon_type"] == "WeaponTypeAxe" && item["hand_type"] == "HandTypeTwoHand"
    }));
    assert!(items.iter().any(|item| {
        item["weapon_type"] == "WeaponTypeMace" && item["hand_type"] == "HandTypeTwoHand"
    }));
    assert!(items.iter().any(|item| {
        item["slot"] == "relic" && item["ranged_weapon_type"] == "RangedWeaponTypeTotem"
    }));
}

#[test]
fn shaman_dual_wield_and_relic_slots_are_filtered_by_client_profile() {
    let run = |slot: &str| {
        let result = Command::new(env!("CARGO_BIN_EXE_forever-engine"))
            .args([
                "item-options",
                "--class",
                "shaman",
                "--level",
                "20",
                "--spec",
                "unrestricted",
                "--slot",
                slot,
            ])
            .output()
            .unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        serde_json::from_slice::<serde_json::Value>(&result.stdout).unwrap()
    };
    let offhand = run("offhand");
    let offhand_items = offhand["items"].as_array().unwrap();
    assert!(offhand_items.iter().any(|item| {
        item["slot"] == "one_hand"
            && (item["weapon_type"] == "WeaponTypeAxe"
                || item["weapon_type"] == "WeaponTypeMace")
    }));
    let relic = run("relic");
    let relic_items = relic["items"].as_array().unwrap();
    assert!(!relic_items.is_empty());
    assert!(relic_items.iter().all(|item| {
        item["ranged_weapon_type"] == "RangedWeaponTypeTotem"
    }));
}

#[test]
fn benchmark_reports_only_requested_samples_after_warmup() {
    let input =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/static-frost-60.rust.json");
    let result = Command::new(env!("CARGO_BIN_EXE_forever-engine"))
        .args(["bench", "--infile"])
        .arg(input)
        .args(["--warmups", "2", "--samples", "3"])
        .output()
        .unwrap();
    assert!(result.status.success());
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["warmups"], 2);
    assert_eq!(value["samples"], 3);
    assert_eq!(value["elapsed_ns_samples"].as_array().unwrap().len(), 3);
    let reports = value["reports"].as_array().unwrap();
    assert_eq!(reports.len(), 3);
    for report in reports {
        assert_eq!(
            report["work"]["cast_completions"],
            report["counts"]["casts"]
        );
        assert_eq!(report["work"], reports[0]["work"]);
        assert_eq!(report["dps_mean"], reports[0]["dps_mean"]);
    }
}

#[test]
fn prepared_benchmark_repeats_one_result() {
    let input = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/reference-clearcasting.prepared.json");
    let result = Command::new(env!("CARGO_BIN_EXE_forever-engine"))
        .args(["bench", "--infile"])
        .arg(input)
        .args(["--warmups", "1", "--samples", "2"])
        .output()
        .unwrap();
    assert!(result.status.success());
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["warmups"], 1);
    assert_eq!(value["samples"], 2);
    assert_eq!(value["elapsed_ns_samples"].as_array().unwrap().len(), 2);
    assert_eq!(value["report"]["scenario_id"], "reference-clearcasting");
    assert!(
        value["report"]["result"]["raidMetrics"]["dps"]["avg"]
            .as_f64()
            .unwrap()
            > 0.0
    );
}

/// `check` keeps its fields and lists each refusal with its stable code, in the order of
/// the reasons.
#[test]
fn check_reports_refusal_codes_beside_the_reasons() {
    let family = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/mage/prepared-v2");
    let check = |path: &std::path::Path| -> serde_json::Value {
        let result = Command::new(env!("CARGO_BIN_EXE_forever-engine"))
            .args(["check", "--infile"])
            .arg(path)
            .output()
            .unwrap();
        assert!(result.status.success());
        serde_json::from_slice(&result.stdout).unwrap()
    };
    // A Serpent Sting tick outcome the runtime does not implement is a Hunter class limit.
    let mut hunter: serde_json::Value = serde_json::from_slice(
        &std::fs::read(family.join("production-marksmanship-hunter.prepared.json")).unwrap(),
    )
    .unwrap();
    for effect in hunter["effects"].as_array_mut().unwrap() {
        if effect["kind"] == "serpent_sting" {
            effect["tick_outcome"] = serde_json::json!("magic_hit");
        }
    }
    let path = std::env::temp_dir().join(format!("cli-refusal-{}.json", std::process::id()));
    std::fs::write(&path, serde_json::to_vec(&hunter).unwrap()).unwrap();
    let refused = check(&path);
    std::fs::remove_file(&path).unwrap();
    let reason = "Serpent Sting ticks with magic_hit";
    assert_eq!(refused["supported"], false);
    assert_eq!(refused["reasons"], serde_json::json!([reason]));
    assert_eq!(
        refused["refusals"],
        serde_json::json!([{"code": "class_limit", "reason": reason}])
    );
    let supported = check(&family.join("frost-reference.prepared.json"));
    assert_eq!(supported["supported"], true);
    assert_eq!(supported["refusals"], serde_json::json!([]));
}

/// A scratch folder holding the accepted shield wall fixture with `change` applied to it.
fn changed_prepared(
    label: &str,
    change: impl FnOnce(&mut serde_json::Value),
) -> (std::path::PathBuf, std::path::PathBuf) {
    let directory =
        std::env::temp_dir().join(format!("forever-gate-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/protection-warrior-premier-shield-wall.prepared.json");
    let mut prepared: serde_json::Value =
        serde_json::from_slice(&fs::read(source).unwrap()).unwrap();
    change(&mut prepared);
    let input = directory.join("input.prepared.json");
    fs::write(&input, serde_json::to_vec(&prepared).unwrap()).unwrap();
    (directory, input)
}

fn sim_gate(input: &std::path::Path, output: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_forever-engine"))
        .args(["sim", "--gate", "--infile"])
        .arg(input)
        .arg("--outfile")
        .arg(output)
        .output()
        .unwrap()
}

#[test]
fn sim_with_the_gate_simulates_a_supported_input_in_one_process() {
    let (directory, input) = changed_prepared("supported", |_| {});
    let output = directory.join("report.json");
    let result = sim_gate(&input, &output);
    let report: serde_json::Value = serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
    fs::remove_dir_all(directory).unwrap();
    assert_eq!(result.status.code(), Some(0));
    assert!(report["result"]["raidMetrics"].is_object());
}

#[test]
fn sim_with_the_gate_reports_a_refusal_as_check_does_and_writes_nothing() {
    let (directory, input) = changed_prepared("refused", |prepared| {
        prepared["unrepresented"] = serde_json::json!(["something unmodeled"]);
    });
    let output = directory.join("report.json");
    let result = sim_gate(&input, &output);
    let checked = Command::new(env!("CARGO_BIN_EXE_forever-engine"))
        .args(["check", "--infile"])
        .arg(&input)
        .output()
        .unwrap();
    let written = fs::read_dir(&directory).unwrap().count();
    fs::remove_dir_all(directory).unwrap();
    assert_eq!(result.status.code(), Some(3));
    assert_eq!(result.stdout, checked.stdout);
    let verdict: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(verdict["supported"], false);
    assert_eq!(verdict["refusals"][0]["code"], "exporter_unrepresented");
    // Only the input remains: no report and no temporary file.
    assert_eq!(written, 1);
}

#[test]
fn sim_with_the_gate_rejects_an_invalid_input_with_its_own_status() {
    let (directory, input) = changed_prepared("invalid", |prepared| {
        prepared["reference"]["engine_revision"] = "0".repeat(40).into();
    });
    let output = directory.join("report.json");
    let result = sim_gate(&input, &output);
    let report_exists = output.exists();
    fs::remove_dir_all(directory).unwrap();
    assert_eq!(result.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&result.stderr).contains("prepared input rejected"));
    assert!(!report_exists);
}

#[test]
fn sim_without_the_gate_still_fails_a_refusal_as_an_error() {
    let (directory, input) = changed_prepared("ungated", |prepared| {
        prepared["unrepresented"] = serde_json::json!(["something unmodeled"]);
    });
    let result = Command::new(env!("CARGO_BIN_EXE_forever-engine"))
        .args(["sim", "--infile"])
        .arg(&input)
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("prepared input unsupported"));
}
