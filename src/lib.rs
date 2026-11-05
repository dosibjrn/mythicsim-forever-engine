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
    prepare, prepare_json, prepare_json_with, prepare_json_with_actor_level, PrepareError,
    Refusal as PrepareRefusal, Rules,
};

/// Prepared v2 effect kinds this engine executes. The release manifest must agree.
pub fn implemented_prepared_effects() -> &'static [&'static str] {
    engine::coverage::implemented_effects()
}
pub use prepare::items::{
    native_item_options, native_player_context, NativeItemOption, NativeItemOptionsRefusal, NativeItemOptionsResponse,
};
pub use report::{Counts, Report, TraceEvent, Work};

/// The pinned Go reference revision, from upstream/sources.json.
pub const SOURCE_REVISION: &str = env!("FOREVER_REFERENCE_REVISION");

/// Read-only candidate options from this exact engine's item and enchant tables.
///
/// This is a proposal API, not a second simulator or a gate verdict. It returns
/// only rows the pinned engine itself can resolve, with the engine's base
/// scaling stats and slot metadata. Callers still submit complete candidates
/// to the native simulation; this API does not claim class/phase/profession
/// legality or supported-effect coverage.
pub fn gear_options(item_types: &[String]) -> serde_json::Value {
    use std::collections::{BTreeMap, BTreeSet, HashSet};

    let requested: HashSet<&str> = item_types.iter().map(String::as_str).collect();
    let mut groups: BTreeMap<String, (Vec<serde_json::Value>, Vec<serde_json::Value>)> =
        BTreeMap::new();
    for item_type in item_types {
        groups.entry(item_type.clone()).or_default();
    }

    // Enumerate the native table, not a Forever-side copy or SQLite catalogue.
    for key in data::ITEMS.keys() {
        let Ok(id) = i32::try_from(key) else {
            continue;
        };
        let Some(item) = prepare::items::database_item(id) else {
            continue;
        };
        if !requested.contains(item.item_type.as_str()) {
            continue;
        }
        let Some(row) = item.row.as_ref() else {
            continue;
        };
        let Some(scaling) = prepare::items::scaling_option(row) else {
            continue;
        };
        let stats = prepare::items::stats_from_map(scaling, "stats").0.to_vec();
        let weapon_damage_min = scaling.f64("weapon_damage_min");
        let weapon_damage_max = scaling.f64("weapon_damage_max");
        let rand_prop_points = scaling.i32("rand_prop_points");
        let item_type = item.item_type.clone();
        let option = serde_json::json!({
            "id": item.id,
            "name": item.name,
            "type": item.item_type,
            "armor_type": item.armor_type,
            "weapon_type": item.weapon_type,
            "hand_type": item.hand_type,
            "ranged_weapon_type": item.ranged_weapon_type,
            "class_allowlist": item.class_allowlist,
            "unique": item.unique,
            "limit_category": item.limit_category,
            "item_effect_count": item.item_effects.len(),
            "stats": stats,
            "weapon_damage_min": weapon_damage_min,
            "weapon_damage_max": weapon_damage_max,
            "rand_prop_points": rand_prop_points,
        });
        groups
            .get_mut(&item_type)
            .expect("requested item type has a group")
            .0
            .push(option);
    }

    // Enchants are keyed and typed by the same native data used by preparation.
    // Include explicit extra_types and the EnchantType -> ItemType mapping.
    for key in data::ENCHANTS.keys() {
        let Ok(id) = i32::try_from(key) else {
            continue;
        };
        let Some(enchant) = prepare::items::enchant(id) else {
            continue;
        };
        let mut applicable_types = BTreeSet::new();
        applicable_types.insert(enchant.item_type.clone());
        applicable_types.extend(enchant.extra_types.iter().cloned());
        if let Some(suffix) = enchant.enchant_type.strip_prefix("EnchantType") {
            if suffix != "Unknown" {
                applicable_types.insert(format!("ItemType{suffix}"));
            }
        }
        if enchant.enchant_type.ends_with("All") || enchant.enchant_type.ends_with("Any") {
            applicable_types.extend(item_types.iter().cloned());
        }
        let option = serde_json::json!({
            "id": enchant.effect_id,
            "name": enchant.name,
            "type": enchant.item_type,
            "enchant_type": enchant.enchant_type,
            "extra_types": enchant.extra_types,
            "stats": enchant.stats.0.to_vec(),
            "pseudo_stats": enchant.pseudo_stats,
            "weapon_damage": enchant.weapon_damage,
        });
        for item_type in applicable_types {
            if let Some(group) = groups.get_mut(&item_type) {
                group.1.push(option.clone());
            }
        }
    }

    let item_types: BTreeMap<String, serde_json::Value> = groups
        .into_iter()
        .map(|(item_type, (items, enchants))| {
            (
                item_type,
                serde_json::json!({"items": items, "enchants": enchants}),
            )
        })
        .collect();

    serde_json::json!({
        "engine": "forever-engine",
        "source_revision": SOURCE_REVISION,
        "item_types": item_types,
    })
}

#[cfg(test)]
mod gear_options_tests {
    use super::{gear_options, SOURCE_REVISION};

    #[test]
    fn gear_options_come_from_native_items_and_enchants_including_extra_types() {
        let report = gear_options(&[
            "ItemTypeHead".to_string(),
            "ItemTypeHands".to_string(),
            "ItemTypeWeapon".to_string(),
        ]);
        assert_eq!(report["engine"], "forever-engine");
        assert_eq!(report["source_revision"], SOURCE_REVISION);

        let head_items = report["item_types"]["ItemTypeHead"]["items"]
            .as_array()
            .expect("native head item options");
        assert!(!head_items.is_empty());
        assert!(head_items
            .iter()
            .all(|item| { item["type"] == "ItemTypeHead" && item["stats"].as_array().is_some() }));

        let hand_enchants = report["item_types"]["ItemTypeHands"]["enchants"]
            .as_array()
            .expect("native hand enchant options");
        assert!(hand_enchants.iter().any(|enchant| enchant["id"] == 15));

        let weapon_enchants = report["item_types"]["ItemTypeWeapon"]["enchants"]
            .as_array()
            .expect("native weapon enchant options");
        assert!(!weapon_enchants.is_empty());
    }
}
