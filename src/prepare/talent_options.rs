//! Read-only talent tree discovery from the pinned TalentsForever client tree projection.
//!
//! The simulator deliberately accepts some builds with broken tier/prerequisite links
//! for comparative studies. This metadata endpoint does NOT reject those builds or
//! claim that every listed effect executes in a particular scenario.

use serde_json::{json, Value};
use std::sync::OnceLock;

const TREE_DATA: &str = include_str!("../../data/talentsforever_talent_options.json");

fn catalog() -> &'static Value {
    static CATALOG: OnceLock<Value> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(TREE_DATA).expect("checked-in TalentsForever talent projection")
    })
}

/// Lists all talent slots, ranks, tiers, and explicit maximum-rank prerequisites
/// for a class. Returns the complete tree even when a given level cannot reach
/// deeper rows; clients use `points_available` and per-talent thresholds to
/// generate candidates. This API does not validate or alter simulated builds.
pub fn native_talent_options(class: &str, level: i32) -> Value {
    let normalized = class.trim().to_ascii_lowercase();
    let canonical = normalized.strip_prefix("class").unwrap_or(&normalized);
    let data = catalog();
    let available = data["classes"].get(canonical);
    let refusal = if !(1..=60).contains(&level) {
        Some((
            "NATIVE_TALENT_OPTIONS_INVALID_LEVEL",
            format!("level must be between 1 and 60, received {level}"),
        ))
    } else if available.is_none() {
        Some((
            "NATIVE_TALENT_OPTIONS_UNSUPPORTED_CLASS",
            format!("unsupported class {class}"),
        ))
    } else {
        None
    };
    if let Some((code, reason)) = refusal {
        return json!({
            "status": "refused",
            "class": canonical,
            "level": level,
            "refusal": {"code": code, "reason": reason}
        });
    }
    json!({
        "status": "ok",
        "class": canonical,
        "level": level,
        "points_available": (level - 9).max(0),
        "points_per_tier": 5,
        "source": data["source"],
        "source_commit": data["source_commit"],
        "trees": available.expect("validated class"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const CLASSES: [&str; 9] = [
        "druid", "hunter", "mage", "paladin", "priest", "rogue", "shaman", "warlock", "warrior",
    ];

    #[test]
    fn every_class_has_three_complete_trees_and_resolved_prerequisites() {
        let mut talents = 0;
        let mut edges = 0;
        for class in CLASSES {
            let options = native_talent_options(class, 60);
            assert_eq!(options["status"], "ok", "{class}");
            let trees = options["trees"].as_array().unwrap();
            assert_eq!(trees.len(), 3, "{class}");
            for tree in trees {
                let entries = tree["talents"].as_array().unwrap();
                let fields: HashMap<&str, u64> = entries
                    .iter()
                    .map(|talent| {
                        (
                            talent["field_name"].as_str().unwrap(),
                            talent["max_rank"].as_u64().unwrap(),
                        )
                    })
                    .collect();
                assert_eq!(fields.len(), entries.len(), "{class}: {}", tree["name"]);
                for (index, talent) in entries.iter().enumerate() {
                    talents += 1;
                    let row = talent["row"].as_i64().unwrap();
                    assert_eq!(talent["index"].as_u64().unwrap(), index as u64);
                    assert_eq!(talent["points_required_in_tree"].as_i64().unwrap(), row * 5);
                    assert_eq!(talent["minimum_level"].as_i64().unwrap(), 10 + row * 5);
                    if let Some(prerequisite) = talent.get("prerequisite") {
                        edges += 1;
                        let field = prerequisite["field_name"].as_str().unwrap();
                        let required_rank = prerequisite["required_rank"].as_u64().unwrap();
                        assert_eq!(
                            Some(&required_rank),
                            fields.get(field),
                            "{class}: {} needs {field}",
                            talent["field_name"]
                        );
                    }
                }
            }
        }
        assert_eq!(talents, 467);
        assert_eq!(edges, 72);
    }

    #[test]
    fn level_budget_and_same_row_prerequisite_are_exposed() {
        assert_eq!(
            native_talent_options("ClassDruid", 20)["points_available"],
            11
        );
        assert_eq!(native_talent_options("druid", 60)["points_available"], 51);
        assert_eq!(native_talent_options("druid", 1)["points_available"], 0);
        let options = native_talent_options("paladin", 40);
        let holy = &options["trees"][0]["talents"];
        let precision = holy
            .as_array()
            .unwrap()
            .iter()
            .find(|talent| talent["field_name"] == "boundlessLight")
            .unwrap();
        assert_eq!(precision["prerequisite"]["field_name"], "holyShock");
        assert_eq!(precision["prerequisite"]["required_rank"], 1);
        assert_eq!(precision["row"], 4);
    }

    #[test]
    fn invalid_queries_are_explicitly_refused() {
        assert_eq!(
            native_talent_options("druid", 61)["refusal"]["code"],
            "NATIVE_TALENT_OPTIONS_INVALID_LEVEL"
        );
        assert_eq!(
            native_talent_options("evoker", 60)["refusal"]["code"],
            "NATIVE_TALENT_OPTIONS_UNSUPPORTED_CLASS"
        );
    }
}
