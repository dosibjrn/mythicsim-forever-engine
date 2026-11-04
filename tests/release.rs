use forever_engine::{
    implemented_prepared_effects, simulate_prepared, EngineIdentity, CLIENT_BUILD, SOURCE_REVISION,
};
use serde_json::Value;
use std::fs;

fn manifest() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("release/manifest.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn release_identity_matches_the_engine() {
    let manifest = manifest();
    assert_eq!(manifest["package_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(manifest["go_reference"]["revision"], SOURCE_REVISION);
    assert_eq!(manifest["client_build"], CLIENT_BUILD);
    assert_eq!(
        manifest["contracts"]["prepared_inputs"],
        serde_json::json!([1, 2])
    );
}

#[test]
fn declared_capabilities_match_implemented_effects() {
    let manifest = manifest();
    let declared: Vec<&str> = manifest["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|capability| capability["id"] == "prepared-v2-contract")
        .unwrap()["implemented_effects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|effect| effect.as_str().unwrap())
        .collect();
    assert_eq!(declared, implemented_prepared_effects());
}

#[test]
fn every_prepared_result_records_the_engine_identity() {
    let manifest = manifest();
    let identity = EngineIdentity::current();
    assert_eq!(manifest["engine"], identity.engine.as_str());
    assert_eq!(
        manifest["package_version"],
        identity.package_version.as_str()
    );
    assert_eq!(
        manifest["go_reference"]["revision"],
        identity.go_reference_revision.as_str()
    );
    assert_eq!(manifest["client_build"], identity.client_build.as_str());
    assert_eq!(
        manifest["schema_version"],
        identity.release_manifest_schema_version
    );
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/frost-reference.prepared.json");
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    value["sim"]["iterations"] = serde_json::json!(1);
    let report = simulate_prepared(&serde_json::from_value(value).unwrap(), forever_engine::RngMode::Stochastic).unwrap();
    let written = serde_json::to_value(&report).unwrap();
    assert_eq!(
        written["identity"],
        serde_json::to_value(&identity).unwrap()
    );
    // The fields that came before stay as they were.
    assert_eq!(
        written["engine"],
        format!("forever-rust-{}", identity.package_version)
    );
    assert_eq!(written["source_revision"], SOURCE_REVISION);
}
