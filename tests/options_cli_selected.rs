//! Integration smoke for Work1 #960: exact selected native CLI transport.
//! These are discovery / coverage checks, not Warcraft legality or M1 gates.
use std::process::Command;

fn native_options(class: &str, level: u8) -> serde_json::Value {
    let level = level.to_string();
    let output = Command::new(env!("CARGO_BIN_EXE_forever-engine"))
        .args(["options", "--class", class, "--level", &level])
        .output()
        .expect("run the exact selected native executable");
    assert!(
        output.status.success(),
        "native options {class} L{level} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // The optimizer consumes one newline-terminated JSON document from stdout.
    // Pretty-printing thousands of item records is unnecessary transport cost.
    assert!(output.stdout.ends_with(b"\n"));
    assert_eq!(
        output.stdout.iter().filter(|&&byte| byte == b'\n').count(),
        1
    );
    serde_json::from_slice(&output.stdout).expect("typed native options response")
}

#[test]
fn two_contrasting_class_levels_preserve_native_source_coverage() {
    // Druid uses native item eligibility; Warrior only has a source-fact pool.
    // This tests the compiled binary's transport, not a mirrored Python rule set.
    let druid = native_options("druid", 20);
    let warrior = native_options("warrior", 60);

    for (response, class, level) in [(&druid, "druid", 20), (&warrior, "warrior", 60)] {
        assert_eq!(response["schema_version"], 1);
        assert_eq!(response["class"], class);
        assert_eq!(response["level"], level);
        assert!(
            response["source_revision"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "native provenance is required"
        );
        assert_eq!(response["talents"]["status"], "ok");
        assert!(response["talents"]["trees"].as_array().is_some());
        assert!(!response["spells"].as_array().unwrap().is_empty());
        assert!(!response["items"].as_array().unwrap().is_empty());
        assert_eq!(response["coverage"]["spell_learnability_complete"], false);
        assert_eq!(response["coverage"]["talent_prerequisites_complete"], true);
        assert!(
            response["coverage"]["native_item_status"]
                .as_str()
                .is_some(),
            "native item status cannot be collapsed into a pretend legal candidate"
        );
        assert!(response["spells"].as_array().unwrap().iter().all(|spell| {
            spell["learning_requirement"] == "unresolved_trainer_talent_or_trigger"
        }));
    }
    assert_eq!(druid["source_revision"], warrior["source_revision"]);
    assert!(druid["items"].as_array().unwrap().iter().all(|item| {
        item["eligibility"] == "native_eligible"
            && item["native"].is_object()
            && item["unverified"].is_null()
    }));
    assert_eq!(warrior["coverage"]["item_eligibility_complete"], false);
    assert_eq!(
        warrior["coverage"]["native_item_status"],
        "proficiency_unverified"
    );
    assert!(warrior["items"].as_array().unwrap().iter().all(|item| {
        item["eligibility"] == "proficiency_unverified"
            && item["native"].is_null()
            && item["unverified"].is_object()
    }));
}

#[test]
fn invalid_native_options_level_is_not_a_successful_empty_coverage_response() {
    let output = Command::new(env!("CARGO_BIN_EXE_forever-engine"))
        .args(["options", "--class", "druid", "--level", "0"])
        .output()
        .expect("run native options CLI");
    assert!(!output.status.success());
}
