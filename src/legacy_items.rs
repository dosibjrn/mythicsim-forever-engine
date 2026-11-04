//! Read-only source facts from the verified legacy Forever Wowhead decisions database.
//!
//! The original private SQLite snapshot is pinned by SHA-256 in
//! tools/export_legacy_wowhead_items.py. The checked-in JSONL is a projection,
//! *not* an independent item rules engine. The client ItemSparse required level
//! (where available) and native Rust class mechanics take precedence.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

const FACTS: &str = include_str!("../data/legacy-wowhead-items.jsonl");

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LegacyItemFact {
    pub id: i32,
    pub required_level: i32,
    /// "observed" is a source fact; "inferred_median" is an estimate.
    pub origin: String,
    /// Classes captured in the historical decisions table, not a universal
    /// inverse class allowlist. Do not prohibit another class using absence.
    pub class_ids: Vec<i32>,
}

fn facts() -> &'static BTreeMap<i32, LegacyItemFact> {
    static INDEX: OnceLock<BTreeMap<i32, LegacyItemFact>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut rows = BTreeMap::new();
        for line in FACTS.lines().filter(|line| !line.is_empty()) {
            let fact: LegacyItemFact =
                serde_json::from_str(line).expect("pinned legacy item source row");
            assert!(fact.required_level >= 0, "negative required level");
            assert!(
                rows.insert(fact.id, fact.clone()).is_none(),
                "duplicate legacy item ID {}",
                fact.id
            );
        }
        rows
    })
}

/// Item required-level evidence keyed by actual game item ID. A missing result
/// means the legacy catalogue knows nothing about that item; it does not mean
/// that the item has no level requirement.
pub fn legacy_item_fact(id: i32) -> Option<&'static LegacyItemFact> {
    facts().get(&id)
}

/// Number of items in the pinned, read-only legacy Wowhead catalogue.
pub fn legacy_item_count() -> usize {
    facts().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_source_totals_and_provenance() {
        assert_eq!(legacy_item_count(), 7913);
        let observed = facts().values().filter(|f| f.origin == "observed").count();
        let inferred = facts()
            .values()
            .filter(|f| f.origin == "inferred_median")
            .count();
        assert_eq!((observed, inferred), (7348, 565));
        assert_eq!(
            facts().values().map(|f| f.class_ids.len()).sum::<usize>(),
            21266
        );
    }

    #[test]
    fn observed_and_estimated_levels_remain_distinct() {
        let observed = legacy_item_fact(200).unwrap();
        assert_eq!(observed.required_level, 17);
        assert_eq!(observed.origin, "observed");
        let inferred = legacy_item_fact(1254).unwrap();
        assert_eq!(inferred.required_level, 23);
        assert_eq!(inferred.origin, "inferred_median");
        assert!(legacy_item_fact(-1).is_none());
    }
}
