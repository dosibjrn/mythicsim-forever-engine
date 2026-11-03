//! A bounded Forever simulation engine organized by shared systems and class/spec domains.
//!
//! Current simulation support is prepared, single-target Frostbolt only. The prepared v2
//! contract describes complete reset Go simulations; [`check_prepared`] reports which
//! mechanics an input still needs. Character preparation, complete Mage mechanics and
//! production reports remain separate migration work. The root re-exports preserve the
//! original public API and prepared v1 JSON contract.

pub mod contracts;
pub use core::rng::RngMode;
pub mod report;

mod classes;
mod core;
#[allow(dead_code)]
mod data;
mod engine;
mod mechanics;
#[allow(dead_code)]
mod prepare;
mod rotation;

pub use classes::mage::spells::frostbolt::hit_chance;
pub use contracts::{Caster, Request, Spell, Target};
pub use engine::prepared::{
    check as check_prepared, coverage as prepared_coverage, refusals as prepared_refusals,
    native_two_world_targets,
    native_two_pull_windows,
    simulate as simulate_prepared, simulate_gated as simulate_prepared_gated,
    validate as validate_prepared, EngineIdentity, Gated, PreparedError, PreparedReport, Refusal,
    CLIENT_BUILD, REFUSAL_CODES,
};
pub use engine::simulate;
pub use prepare::{
    prepare, prepare_json, prepare_json_with, PrepareError, Refusal as PrepareRefusal, Rules,
};

/// Prepared v2 effect kinds this engine executes. The release manifest must agree.
pub fn implemented_prepared_effects() -> &'static [&'static str] {
    engine::coverage::implemented_effects()
}
pub use report::{Counts, Report, TraceEvent, Work};

/// The pinned Go reference revision, from upstream/sources.json.
pub const SOURCE_REVISION: &str = env!("FOREVER_REFERENCE_REVISION");

/// Return native Druid talent-layout metadata for wire serialization only.
pub fn forever_druid_talent_layout() -> serde_json::Value {
    classes::druid::prepare::forever_talent_layout_json()
}

#[cfg(test)]
mod druid_talent_layout_api_tests {
    #[test]
    fn exports_a_lossless_versioned_layout_projection() {
        let value = crate::forever_druid_talent_layout();
        assert_eq!(value["schema_version"].as_u64(), Some(2));
        assert_eq!(value["class"].as_str(), Some("druid"));
        assert_eq!(value["ruleset"].as_str(), Some("forever"));

        let trees = value["trees"].as_array().expect("layout trees");
        assert_eq!(trees.len(), crate::classes::druid::prepare::FOREVER_TREES.len());
        let keys = trees
            .iter()
            .map(|tree| tree["key"].as_str().expect("tree key"))
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["balance", "feral", "restoration"]);

        // The restoration tree changed positions in the current client data; the
        // native projection must not silently preserve the old Python slot names.
        assert_eq!(trees[2]["talents"][1]["field"].as_str(), Some("heart_of_the_wild"));
        assert_eq!(trees[2]["talents"][1]["row"].as_u64(), Some(0));
        assert_eq!(trees[2]["talents"][1]["column"].as_u64(), Some(2));
        let lacerate = trees[1]["talents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|talent| talent["field"] == "lacerate")
            .expect("Lacerate metadata");
        assert_eq!(lacerate["prerequisites"][0]["field"].as_str(), Some("savage_fury"));
        assert_eq!(lacerate["prerequisites"][0]["min_rank"].as_u64(), Some(2));

        for (tree, source_tree) in trees
            .iter()
            .zip(crate::classes::druid::prepare::FOREVER_TREES.iter())
        {
            let entries = tree["talents"].as_array().expect("talent metadata");
            assert_eq!(entries.len(), source_tree.len());
            for (entry, source) in entries.iter().zip(source_tree.iter()) {
                assert_eq!(entry["field"].as_str(), Some(source.field));
                assert_eq!(entry["name"].as_str(), Some(source.name));
                assert_eq!(entry["max_ranks"].as_u64(), Some(u64::from(source.max_ranks)));
                assert_eq!(entry["in_proto"].as_bool(), Some(source.in_proto));
                assert_eq!(entry["row"].as_u64(), Some(u64::from(source.row)));
                assert_eq!(entry["column"].as_u64(), Some(u64::from(source.column)));
                assert_eq!(
                    entry["tier_points"].as_u64(),
                    Some(u64::from(source.row) * 5)
                );
                let expected_prerequisites = source
                    .prerequisite
                    .map(|(field, min_rank)| {
                        serde_json::json!([{"field": field, "min_rank": min_rank}])
                    })
                    .unwrap_or_else(|| serde_json::json!([]));
                assert_eq!(entry["prerequisites"], expected_prerequisites);
            }
        }
    }
}
