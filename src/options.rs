//! Unified native-source option discovery for Forever build search.
//!
//! No second rules engine or build validator: trained spell legality is not
//! inferred from spell-family rows, and class equipment proficiency is not
//! inferred from armor/weapon labels. Existing native item options and the
//! pinned TalentsForever client tree projection are reused as-is.

use std::collections::BTreeMap;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::data::{self, ITEMS};
use crate::legacy_items::{legacy_item_fact, LegacyItemFact};
use crate::prepare::items::{native_item_options, NativeItemOption};
use crate::prepare::spelldata;
use crate::prepare::stats::Stat;
use crate::prepare::talent_options::native_talent_options;
use crate::SOURCE_REVISION;

/// The nine playable classes; these are not proto.Class numbers.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Druid => "druid",
            Self::Hunter => "hunter",
            Self::Mage => "mage",
            Self::Paladin => "paladin",
            Self::Priest => "priest",
            Self::Rogue => "rogue",
            Self::Shaman => "shaman",
            Self::Warlock => "warlock",
            Self::Warrior => "warrior",
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

    fn has_native_item_eligibility(self) -> bool {
        matches!(
            self,
            Self::Priest | Self::Mage | Self::Shaman | Self::Paladin | Self::Rogue | Self::Druid | Self::Warlock
        )
    }
}

impl FromStr for Class {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "druid" | "classdruid" => Ok(Self::Druid),
            "hunter" | "classhunter" => Ok(Self::Hunter),
            "mage" | "classmage" => Ok(Self::Mage),
            "paladin" | "classpaladin" => Ok(Self::Paladin),
            "priest" | "classpriest" => Ok(Self::Priest),
            "rogue" | "classrogue" => Ok(Self::Rogue),
            "shaman" | "classshaman" => Ok(Self::Shaman),
            "warlock" | "classwarlock" => Ok(Self::Warlock),
            "warrior" | "classwarrior" => Ok(Self::Warrior),
            _ => Err(format!("unsupported class: {value}")),
        }
    }
}

/// A class-family spell-data entry is not necessarily a learned/castable spell:
/// class-family rows also contain passives, triggered effects and talent grants.
#[derive(Clone, Debug, Serialize)]
pub struct SpellOption {
    pub spell_id: i32,
    pub name: String,
    pub rank: String,
    pub minimum_level: Option<u8>,
    pub source: &'static str,
    pub learning_requirement: &'static str,
}

/// Raw, client-level-bounded item record for a class that does not yet have a
/// complete native equipment-proficiency projection. It is NOT marked legal.
#[derive(Clone, Debug, Serialize)]
pub struct UnverifiedItem {
    pub name: String,
    pub item_type: String,
    pub hand_type: String,
    pub weapon_type: String,
    pub armor_type: String,
    pub class_allowlist: Vec<String>,
    pub required_level: i32,
    pub stats: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ItemOption {
    pub item_id: i32,
    /// "native_eligible" means the native item-options source accepted it;
    /// "proficiency_unverified" must not enter legal candidate generation.
    pub eligibility: &'static str,
    pub native: Option<NativeItemOption>,
    pub unverified: Option<UnverifiedItem>,
    /// Historical Wowhead evidence only; never overrides ItemSparse level.
    pub legacy_wowhead: Option<LegacyItemFact>,
}

/// A proposal taken from the native Warlock pet executor and request proto enum.
/// Only source-native preparation and combat may determine whether it works.
#[derive(Clone, Debug, Serialize)]
pub struct PetOption {
    pub summon: &'static str,
    pub proto_enum_value: i32,
    pub eligibility: &'static str,
    pub source: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct Coverage {
    pub spell_learnability_complete: bool,
    pub item_eligibility_complete: bool,
    pub talent_prerequisites_complete: bool,
    pub native_item_status: String,
    pub notes: Vec<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CharacterOptions {
    pub schema_version: u32,
    pub source_revision: &'static str,
    pub level: u8,
    pub class: Class,
    pub spells: Vec<SpellOption>,
    pub items: Vec<ItemOption>,
    /// Native TalentsForever source: three trees, tier points, parent/rank
    /// requirements, point budget, and original source revision.
    pub talents: serde_json::Value,
    pub pets: Vec<PetOption>,
    pub coverage: Coverage,
}

fn class_spells(level: u8, class: Class) -> Vec<SpellOption> {
    let mut spells = spelldata::all()
        .filter(|spell| {
            spell.class_flags.family == class.spell_family()
                && !spell.name.is_empty()
                && spell.spell_level <= i16::from(level)
        })
        .map(|spell| SpellOption {
            spell_id: spell.id,
            name: spell.name.clone(),
            rank: spell.rank.clone(),
            minimum_level: u8::try_from(spell.spell_level)
                .ok()
                .filter(|level| *level > 0),
            source: "native_class_spell_family",
            learning_requirement: "unresolved_trainer_talent_or_trigger",
        })
        .collect::<Vec<_>>();
    spells.sort_by_key(|spell| spell.spell_id);
    spells
}

fn native_class_items(level: u8, class: Class) -> Result<(Vec<ItemOption>, String), String> {
    let options = native_item_options(class.as_str(), i32::from(level), "unrestricted", "all");
    if let Some(refusal) = options.refusal {
        return Err(format!("{}: {}", refusal.code, refusal.reason));
    }
    let status = options.status;
    let mut items = options
        .items
        .into_iter()
        .map(|item| ItemOption {
            item_id: item.item_id,
            eligibility: "native_eligible",
            legacy_wowhead: legacy_item_fact(item.item_id).cloned(),
            native: Some(item),
            unverified: None,
        })
        .collect::<Vec<_>>();
    items.sort_by_key(|item| item.item_id);
    Ok((items, status))
}

/// Other classes have source-backed client level and proto class allowlist
/// information, but do not yet have a native equipment proficiency projection.
/// Do not guess which weapons/armor they can use.
fn unverified_class_items(level: u8, class: Class) -> Vec<ItemOption> {
    let mut items = Vec::new();
    for key in ITEMS.keys() {
        let Ok(id) = i32::try_from(key) else {
            continue;
        };
        let Some(required_level) = data::item_required_levels().get(&id).copied() else {
            continue;
        };
        if required_level > i32::from(level) {
            continue;
        }
        let Some(item) = crate::prepare::items::database_item(id) else {
            continue;
        };
        if !matches!(
            item.item_type.as_str(),
            "ItemTypeHead"
                | "ItemTypeNeck"
                | "ItemTypeShoulder"
                | "ItemTypeBack"
                | "ItemTypeChest"
                | "ItemTypeWrist"
                | "ItemTypeHands"
                | "ItemTypeWaist"
                | "ItemTypeLegs"
                | "ItemTypeFeet"
                | "ItemTypeFinger"
                | "ItemTypeTrinket"
                | "ItemTypeWeapon"
                | "ItemTypeRanged"
        ) {
            continue;
        }
        if !item.class_allowlist.is_empty()
            && !item
                .class_allowlist
                .iter()
                .any(|name| name == class.proto_name())
        {
            continue;
        }
        let Some(scaling) = item
            .row
            .as_deref()
            .and_then(crate::prepare::items::scaling_option)
        else {
            continue;
        };
        let source_stats = crate::prepare::items::stats_from_map(scaling, "stats");
        let stats = Stat::ALL
            .into_iter()
            .filter_map(|stat| {
                let value = source_stats[stat];
                (value != 0.0).then(|| (stat.name().to_owned(), value))
            })
            .collect();
        items.push(ItemOption {
            item_id: item.id,
            eligibility: "proficiency_unverified",
            legacy_wowhead: legacy_item_fact(item.id).cloned(),
            native: None,
            unverified: Some(UnverifiedItem {
                name: item.name,
                item_type: item.item_type,
                hand_type: item.hand_type,
                weapon_type: item.weapon_type,
                armor_type: item.armor_type,
                class_allowlist: item.class_allowlist,
                required_level,
                stats,
            }),
        });
    }
    items.sort_by_key(|item| item.item_id);
    items
}

/// Only enumerate the four demons registered in the actual native Warlock
/// executor, plus the proto's zero/no-pet choice if it exists. This is an
/// executable candidate domain, not a Python trainer or level-unlock model.
fn native_pet_options(class: Class) -> Vec<PetOption> {
    if class != Class::Warlock {
        return Vec::new();
    }
    const ENUM: &str = "proto.WarlockOptions.Summon";
    let registered = crate::classes::warlock::prepare::registered_native_summons();
    let mut choices = Vec::new();
    if let Some(no_pet) = crate::contracts::request::enum_name(ENUM, 0) {
        if !registered.contains(&no_pet) {
            choices.push(no_pet);
        }
    }
    choices.extend(registered);
    choices
        .into_iter()
        .filter_map(|summon| {
            crate::contracts::request::enum_number(ENUM, summon).map(|proto_enum_value| {
                PetOption {
                    summon,
                    proto_enum_value,
                    eligibility: "native_prepare_and_combat_required",
                    source: "native_warlock_executor_and_proto_enum",
                }
            })
        })
        .collect()
}

/// Inventory the native source's item, spell and talent options together.
/// No new validation gate is introduced. Unknown item/spell eligibility stays
/// explicit, never "legal by construction" without a known prerequisite.
pub fn get_options(level: u8, class: Class) -> Result<CharacterOptions, String> {
    if !(1..=60).contains(&level) {
        return Err(format!("level {level} must be in 1..=60"));
    }
    let talents = native_talent_options(class.as_str(), i32::from(level));
    if talents["status"] != "ok" {
        return Err(format!("native talent source refused {}", class.as_str()));
    }
    let spells = class_spells(level, class);
    let pets = native_pet_options(class);
    let (items, native_item_status, item_eligibility_complete) =
        if class.has_native_item_eligibility() {
            let (items, status) = native_class_items(level, class)?;
            let complete = status == "ok" || status == "empty";
            (items, status, complete)
        } else {
            (
                unverified_class_items(level, class),
                "proficiency_unverified".to_owned(),
                false,
            )
        };
    let mut notes = vec!["spell_class_family_rows_include_unlearnable_or_triggered_spells"];
    if !item_eligibility_complete {
        notes.push("item_eligibility_incomplete_for_requested_class_or_level");
    }
    if class == Class::Warlock {
        notes.push("pet_choices_require_native_execution_not_level_unlock_certification");
    }
    Ok(CharacterOptions {
        schema_version: 1,
        source_revision: SOURCE_REVISION,
        level,
        class,
        spells,
        items,
        talents,
        pets,
        coverage: Coverage {
            spell_learnability_complete: false,
            item_eligibility_complete,
            talent_prerequisites_complete: true,
            native_item_status,
            notes,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLASSES: [Class; 9] = [
        Class::Druid,
        Class::Hunter,
        Class::Mage,
        Class::Paladin,
        Class::Priest,
        Class::Rogue,
        Class::Shaman,
        Class::Warlock,
        Class::Warrior,
    ];

    #[test]
    fn native_talent_trees_and_level_points_for_all_nine_classes() {
        for class in CLASSES {
            let options = get_options(30, class).unwrap();
            assert_eq!(options.talents["status"], "ok");
            assert_eq!(options.talents["points_available"], 21);
            assert_eq!(options.talents["trees"].as_array().unwrap().len(), 3);
            assert!(options.coverage.talent_prerequisites_complete);
        }
        let druid = get_options(30, Class::Druid).unwrap();
        let feral = &druid.talents["trees"][1]["talents"];
        let lacerate = feral
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["field_name"] == "lacerate")
            .unwrap();
        assert_eq!(lacerate["prerequisite"]["field_name"], "savageFury");
        assert_eq!(lacerate["prerequisite"]["required_rank"], 2);
    }

    #[test]
    fn spell_families_are_class_scoped_and_level_bounded() {
        let mage = get_options(10, Class::Mage).unwrap();
        assert!(mage.spells.iter().any(|spell| spell.spell_id == 116));
        assert!(!mage.spells.iter().any(|spell| spell.spell_id == 10));
        let druid = get_options(20, Class::Druid).unwrap();
        assert!(druid.spells.iter().any(|spell| spell.spell_id == 339));
        assert!(!druid.spells.iter().any(|spell| spell.spell_id == 116));
        assert!(!druid.coverage.spell_learnability_complete);
    }

    #[test]
    fn supported_classes_reuse_native_item_eligibility() {
        for class in [
            Class::Priest,
            Class::Mage,
            Class::Shaman,
            Class::Paladin,
            Class::Rogue,
            Class::Druid,
            Class::Warlock,
        ] {
            let result = get_options(20, class).unwrap();
            assert!(!result.items.is_empty(), "{}", class.as_str());
            assert!(result.items.iter().all(|item| {
                item.eligibility == "native_eligible"
                    && item.native.as_ref().is_some_and(|native| native.required_level <= 20)
                    && item.unverified.is_none()
            }));
        }
    }

    #[test]
    fn other_classes_have_source_facts_but_no_invented_proficiencies() {
        for class in [Class::Hunter, Class::Warrior] {
            let result = get_options(20, class).unwrap();
            assert!(!result.items.is_empty(), "{}", class.as_str());
            assert!(!result.coverage.item_eligibility_complete);
            assert!(result.items.iter().all(|item| {
                item.eligibility == "proficiency_unverified"
                    && item.unverified.as_ref().is_some_and(|row| row.required_level <= 20)
                    && item.native.is_none()
            }));
        }
    }

    #[test]
    fn warlock_l20_uses_pinned_client_skills_and_item_sparse() {
        let source = crate::data::warlock_equipment_skills().unwrap();
        assert_eq!(source.min_level("Cloth"), Some(1));
        assert_eq!(source.min_level("Daggers"), Some(1));
        assert_eq!(source.min_level("Staves"), Some(1));
        assert_eq!(source.min_level("Wands"), Some(1));
        assert_eq!(source.min_level("Swords"), None);
        assert_eq!(source.min_level("Leather"), None);

        let options = get_options(20, Class::Warlock).unwrap();
        assert!(options.coverage.item_eligibility_complete);
        assert_eq!(options.coverage.native_item_status, "ok");
        assert!(!options.items.is_empty());
        assert!(options.items.iter().all(|row| row.eligibility == "native_eligible"
            && row.unverified.is_none()
            && row.native.as_ref().is_some_and(|item| item.required_level <= 20)));
        assert!(options.items.iter().any(|row| row.native.as_ref()
            .is_some_and(|item| item.ranged_weapon_type == "RangedWeaponTypeWand")));
    }

    #[test]
    fn native_warlock_pet_candidates_are_actual_registered_proto_choices() {
        let options = get_options(20, Class::Warlock).unwrap();
        let registered = crate::classes::warlock::prepare::registered_native_summons();
        for summon in registered {
            let option = options.pets.iter().find(|pet| pet.summon == summon)
                .expect("registered native demon must appear in options");
            assert_eq!(
                crate::contracts::request::enum_number("proto.WarlockOptions.Summon", summon),
                Some(option.proto_enum_value)
            );
            assert_eq!(option.eligibility, "native_prepare_and_combat_required");
        }
        assert!(options.coverage.notes.contains(
            &"pet_choices_require_native_execution_not_level_unlock_certification"
        ));
        assert!(get_options(20, Class::Druid).unwrap().pets.is_empty());
    }

    #[test]
    fn historical_required_levels_do_not_replace_client_item_sparse() {
        let result = get_options(60, Class::Mage).unwrap();
        let joined = result
            .items
            .iter()
            .filter(|item| item.legacy_wowhead.is_some())
            .count();
        assert!(joined > 0);
        assert!(result.items.iter().all(|item| {
            item.native.as_ref().is_none_or(|native| {
                native.required_level_origin == "client_ItemSparse.RequiredLevel"
            })
        }));
        assert_eq!(crate::legacy_item_count(), 7913);
    }

    #[test]
    fn request_validation_is_not_a_second_build_validation_api() {
        assert!(get_options(0, Class::Mage).is_err());
        assert!(get_options(61, Class::Mage).is_err());
        assert!("ClassDruid".parse::<Class>().is_ok());
        assert!("evoker".parse::<Class>().is_err());
    }
}
