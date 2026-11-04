//! Source-backed option discovery for Forever search clients.
//!
//! This inventories what the *selected Rust source* knows; it deliberately does
//! not pretend that a client spell-data row is a learned spell or that every
//! database item is wearable at a checkpoint. Unknown prerequisites remain
//! explicit, so consumers cannot silently turn an incomplete source into a
//! larger, illegal search space. No second build validator is introduced.

mod talents;

use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::data::{self, ITEMS};
use crate::legacy_items::legacy_item_fact;
use crate::prepare::items::{scaling_option, stats_from_map};
use crate::prepare::spelldata;
use crate::SOURCE_REVISION;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    Druid,
    Hunter,
    Mage,
    Paladin,
    Priest,
    Rogue,
    Shaman,
    Warlock,
    Warrior,
}

impl Class {
    /// Class family identifiers of the pinned client spell-data rows.
    /// These are not proto.Class numbers (e.g. ClassMage is proto enum 8).
    fn spell_family(self) -> i32 {
        match self {
            Self::Mage => 3,
            Self::Warrior => 4,
            Self::Warlock => 5,
            Self::Priest => 6,
            Self::Druid => 7,
            Self::Rogue => 8,
            Self::Hunter => 9,
            Self::Paladin => 10,
            Self::Shaman => 11,
        }
    }

    fn proto_name(self) -> &'static str {
        match self {
            Self::Druid => "ClassDruid",
            Self::Hunter => "ClassHunter",
            Self::Mage => "ClassMage",
            Self::Paladin => "ClassPaladin",
            Self::Priest => "ClassPriest",
            Self::Rogue => "ClassRogue",
            Self::Shaman => "ClassShaman",
            Self::Warlock => "ClassWarlock",
            Self::Warrior => "ClassWarrior",
        }
    }
}

impl FromStr for Class {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "druid" => Ok(Self::Druid),
            "hunter" => Ok(Self::Hunter),
            "mage" => Ok(Self::Mage),
            "paladin" => Ok(Self::Paladin),
            "priest" => Ok(Self::Priest),
            "rogue" => Ok(Self::Rogue),
            "shaman" => Ok(Self::Shaman),
            "warlock" => Ok(Self::Warlock),
            "warrior" => Ok(Self::Warrior),
            _ => Err(format!("unsupported class: {value}")),
        }
    }
}

/// Only prerequisites supported by the native source are asserted as known.
/// Missing unlock rules stay unresolved instead of being treated as `true`.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Prerequisite {
    MinimumLevel {
        level: u8,
    },
    ClassAllowlist {
        classes: Vec<String>,
    },
    /// Five points in this tree for each earlier tier.
    PointsInTree {
        tree: String,
        points: u8,
    },
    /// Native tree position of the parent and the required parent rank.
    TalentRank {
        source_field: String,
        position: usize,
        required_rank: u32,
    },
    Unresolved {
        key: &'static str,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct SpellOption {
    pub spell_id: i32,
    pub name: String,
    pub rank: String,
    pub prerequisites: Vec<Prerequisite>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ItemOption {
    pub item_id: i32,
    pub name: String,
    pub item_type: String,
    pub hand_type: String,
    pub armor_type: String,
    pub weapon_type: String,
    /// Nonzero entries of the native proto scaling-option-0 stat vector.
    /// Indices are native sim.Stats enum positions, not invented stat weights.
    pub stats: Vec<(usize, f64)>,
    pub unique: bool,
    /// From the pinned historical Wowhead decisions catalogue, when available.
    pub required_level: Option<i32>,
    /// "observed" is a sourced fact; "inferred_median" is not a legality proof.
    pub required_level_origin: Option<String>,
    /// Captured eligibility IDs; absence is not evidence of a class restriction.
    pub historical_class_ids: Vec<i32>,
    pub prerequisites: Vec<Prerequisite>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TalentOption {
    pub tree: String,
    /// Index of the talent in the native client's tree order, starting at zero.
    pub position: usize,
    pub field: String,
    /// Identifier in the pinned TalentsForever talent tree.
    pub source_field: String,
    pub name: String,
    pub max_ranks: u32,
    pub in_proto: bool,
    pub prerequisites: Vec<Prerequisite>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Coverage {
    /// Rust-native catalogues have some usable rows, but do not yet expose
    /// complete trainer/class spell unlocks, item level restrictions or talent
    /// dependencies. A search must *not* treat these rows as all legal builds.
    pub eligibility_complete: bool,
    pub missing: Vec<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CharacterOptions {
    pub schema_version: u32,
    pub ruleset: &'static str,
    pub source_revision: &'static str,
    pub level: u8,
    pub class: Class,
    pub talent_points_available: u8,
    pub talent_source_revision: &'static str,
    pub spells: Vec<SpellOption>,
    pub items: Vec<ItemOption>,
    pub talents: Vec<TalentOption>,
    pub coverage: Coverage,
}

/// Inventory native source options for one class and checkpoint level.
///
/// This is a deliberately *partial* discovery API until the native unlock
/// sources are migrated. It never silently declares an unproven option legal.
pub fn get_options(level: u8, class: Class) -> Result<CharacterOptions, String> {
    if !(1..=60).contains(&level) {
        return Err(format!(
            "level {level} is outside the supported range 1..=60"
        ));
    }

    let mut spells = Vec::new();
    for spell in spelldata::all() {
        if spell.class_flags.family != class.spell_family()
            || spell.name.is_empty()
            || spell.spell_level > i16::from(level)
        {
            continue;
        }
        let mut prerequisites = Vec::new();
        if let Ok(min_level) = u8::try_from(spell.spell_level) {
            if min_level > 0 {
                prerequisites.push(Prerequisite::MinimumLevel { level: min_level });
            } else {
                prerequisites.push(Prerequisite::Unresolved {
                    key: "spell_minimum_level",
                });
            }
        } else {
            prerequisites.push(Prerequisite::Unresolved {
                key: "spell_minimum_level",
            });
        }
        // Class-family matching is source metadata, not proof of trainer
        // learnability: this family includes procs and talent-granted effects.
        prerequisites.push(Prerequisite::Unresolved {
            key: "spell_learnability",
        });
        spells.push(SpellOption {
            spell_id: spell.id,
            name: spell.name.clone(),
            rank: spell.rank.clone(),
            prerequisites,
        });
    }

    let mut items = Vec::new();
    for id in ITEMS.keys() {
        let Some(row) = data::proto_row(&ITEMS, "proto.SimItem", id) else {
            continue;
        };
        let item_type = row.enum_name("type");
        // The engine does not equip an unknown item type.
        if item_type != "ItemTypeWeapon"
            && crate::prepare::items::item_type_to_slot(&item_type).is_none()
        {
            continue;
        }
        let allowlist = row.enum_names("class_allowlist");
        if !allowlist.is_empty() && !allowlist.iter().any(|entry| entry == class.proto_name()) {
            continue;
        }
        let Some(scaling) = scaling_option(&row) else {
            continue;
        };
        let stats = stats_from_map(scaling, "stats")
            .0
            .iter()
            .enumerate()
            .filter(|(_, amount)| **amount != 0.0)
            .map(|(index, amount)| (index, *amount))
            .collect();
        let historical = legacy_item_fact(row.i32("id"));
        // A source-observed requirement excludes items above this checkpoint;
        // inferred values are still only hints and never become hard rules.
        if historical.is_some_and(|fact| {
            fact.origin == "observed" && fact.required_level > i32::from(level)
        }) {
            continue;
        }
        let mut prerequisites = Vec::new();
        if !allowlist.is_empty() {
            prerequisites.push(Prerequisite::ClassAllowlist { classes: allowlist });
        }
        match historical {
            Some(fact) if fact.origin == "observed" => {
                if let Ok(required) = u8::try_from(fact.required_level) {
                    if required > 0 {
                        prerequisites.push(Prerequisite::MinimumLevel { level: required });
                    }
                } else {
                    prerequisites.push(Prerequisite::Unresolved {
                        key: "item_required_level_out_of_range",
                    });
                }
            }
            Some(_) => prerequisites.push(Prerequisite::Unresolved {
                key: "item_required_level_inferred",
            }),
            None => prerequisites.push(Prerequisite::Unresolved {
                key: "item_required_level",
            }),
        }
        prerequisites.push(Prerequisite::Unresolved {
            key: "item_equip_proficiency",
        });
        items.push(ItemOption {
            item_id: row.i32("id"),
            name: row.str("name").to_owned(),
            item_type,
            hand_type: row.enum_name("hand_type"),
            armor_type: row.enum_name("armor_type"),
            weapon_type: row.enum_name("weapon_type"),
            stats,
            unique: row.bool("unique"),
            required_level: historical.map(|fact| fact.required_level),
            required_level_origin: historical.map(|fact| fact.origin.clone()),
            historical_class_ids: historical
                .map(|fact| fact.class_ids.clone())
                .unwrap_or_default(),
            prerequisites,
        });
    }

    // The exact pinned TalentsForever client tree owns talent slots and arrows.
    // This is discovery metadata, never a new simulation legality guard.
    let talents = talents::options(class);
    let mut missing = vec![
        "spellbook_training_and_talent_grants",
        "item_level_and_equipment_constraints",
    ];
    if !matches!(class, Class::Druid | Class::Mage) {
        missing.push("native_class_talent_layout");
    }
    Ok(CharacterOptions {
        schema_version: 1,
        ruleset: "forever",
        source_revision: SOURCE_REVISION,
        level,
        class,
        talent_points_available: level.saturating_sub(9),
        talent_source_revision: talents::source_revision(),
        spells,
        items,
        talents,
        coverage: Coverage {
            eligibility_complete: false,
            missing,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_spells_are_level_bounded_and_class_specific() {
        let mage = get_options(10, Class::Mage).unwrap();
        assert!(mage.spells.iter().any(|s| s.spell_id == 116)); // Frostbolt R1 (L4)
        assert!(!mage.spells.iter().any(|s| s.spell_id == 10)); // Blizzard R1 (L20)
        let druid = get_options(20, Class::Druid).unwrap();
        assert!(druid.spells.iter().any(|s| s.spell_id == 339)); // Roots R1 (L8)
        assert!(!druid.spells.iter().any(|s| s.spell_id == 116));
    }

    #[test]
    fn native_items_respect_recorded_class_allowlists() {
        let mage = get_options(20, Class::Mage).unwrap();
        let priest = get_options(20, Class::Priest).unwrap();
        assert!(mage.items.iter().any(|i| i.item_id == 25)); // Worn Shortsword, unrestricted
        assert!(!mage.items.iter().any(|i| i.item_id == 2919)); // Priest relic
        assert!(priest.items.iter().any(|i| i.item_id == 2919));
        assert!(mage
            .items
            .iter()
            .all(|i| {
                let has_missing = i.prerequisites.iter().any(|p| matches!(
                    p,
                    Prerequisite::Unresolved {
                        key: "item_required_level"
                    }
                ));
                i.required_level_origin.is_some() || has_missing
            }));
    }

    #[test]
    fn observed_required_levels_filter_options_but_inferred_levels_remain_hints() {
        let mage20 = get_options(20, Class::Mage).unwrap();
        let mage60 = get_options(60, Class::Mage).unwrap();
        let observed20 = mage20
            .items
            .iter()
            .filter(|item| item.required_level_origin.as_deref() == Some("observed"))
            .count();
        let observed60 = mage60
            .items
            .iter()
            .filter(|item| item.required_level_origin.as_deref() == Some("observed"))
            .count();
        assert!(observed20 > 0);
        assert!(observed60 > observed20);
        assert!(mage20.items.iter().all(|item| {
            item.required_level_origin.as_deref() != Some("observed")
                || item.required_level.unwrap_or(0) <= 20
        }));
        assert!(mage60.items.iter().filter(|item| {
            item.required_level_origin.as_deref() == Some("inferred_median")
        }).all(|item| {
            item.prerequisites.iter().any(|p| matches!(
                p,
                Prerequisite::Unresolved {
                    key: "item_required_level_inferred"
                }
            ))
        }));
    }

    #[test]
    fn native_talent_layout_is_reused_without_a_second_rank_table() {
        let druid = get_options(30, Class::Druid).unwrap();
        assert!(druid
            .talents
            .iter()
            .any(|t| t.field == "lacerate" && t.position == 11 && t.max_ranks == 1 && !t.in_proto));
        let mage = get_options(30, Class::Mage).unwrap();
        assert!(mage
            .talents
            .iter()
            .any(|t| t.field == "elemental_precision" && t.max_ranks == 3));
    }

    #[test]
    fn gaps_cannot_be_misread_as_a_complete_legal_build_catalogue() {
        assert!(get_options(0, Class::Mage).is_err());
        assert!(get_options(61, Class::Mage).is_err());
        let options = get_options(20, Class::Shaman).unwrap();
        assert!(!options.coverage.eligibility_complete);
        assert!(options
            .coverage
            .missing
            .contains(&"native_class_talent_layout"));
        assert!(!options.spells.is_empty());
        let json = serde_json::to_value(options).unwrap();
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["class"], "shaman");
    }
}
