//! Go sim/core/database.go: items, enchants and random suffixes from the imported database,
//! and the equipment a request equips.

use crate::contracts::request::{Message, Value};
use crate::data::{self, ENCHANTS, ITEMS, RANDOM_SUFFIXES};

use super::stats::{Stat, Stats};
use super::Refusal;

/// Go `proto.ItemSlot` order.
pub(crate) const NUM_ITEM_SLOTS: usize = 17;
pub(crate) mod slot {
    pub const HEAD: usize = 0;
    pub const NECK: usize = 1;
    pub const SHOULDER: usize = 2;
    pub const BACK: usize = 3;
    pub const CHEST: usize = 4;
    pub const WRIST: usize = 5;
    pub const HANDS: usize = 6;
    pub const WAIST: usize = 7;
    pub const LEGS: usize = 8;
    pub const FEET: usize = 9;
    pub const FINGER1: usize = 10;
    pub const FINGER2: usize = 11;
    pub const TRINKET1: usize = 12;
    pub const TRINKET2: usize = 13;
    pub const MAIN_HAND: usize = 14;
    pub const OFF_HAND: usize = 15;
    pub const RANGED: usize = 16;
}

/// Go `Enchant`.
#[derive(Clone, Debug, Default)]
pub(crate) struct Enchant {
    pub effect_id: i32,
    pub stats: Stats,
    pub pseudo_stats: Vec<f64>,
    pub weapon_damage: f64,
    pub enchant_effects: Vec<Message>,
    pub name: String,
    pub item_type: String,
    pub enchant_type: String,
    pub extra_types: Vec<String>,
}

/// Go `RandomSuffix`.
#[derive(Clone, Debug, Default)]
pub(crate) struct RandomSuffix {
    pub id: i32,
    pub name: String,
    pub stats: Stats,
}

/// Go `Item` as `NewItem` builds it for one equipped item.
#[derive(Clone, Debug, Default)]
pub(crate) struct Item {
    pub id: i32,
    pub name: String,
    pub item_type: String,
    pub armor_type: String,
    pub weapon_type: String,
    pub hand_type: String,
    pub ranged_weapon_type: String,
    pub weapon_damage_min: f64,
    pub weapon_damage_max: f64,
    pub swing_speed: f64,
    pub quality_modifier: f64,
    pub stats: Stats,
    pub pseudo_stats: Vec<f64>,
    pub unique: bool,
    pub limit_category: i32,
    pub set_name: String,
    pub set_id: i32,
    pub class_allowlist: Vec<String>,
    pub random_suffix: RandomSuffix,
    pub enchant: Enchant,
    pub temp_enchant: i32,
    pub rand_prop_points: i32,
    pub item_effects: Vec<Message>,
    /// The item's whole database row, for its scaling options and area stats.
    pub row: Option<std::rc::Rc<Message>>,
}

impl Item {
    pub fn is_empty(&self) -> bool {
        self.id == 0
    }
}

pub(crate) type Equipment = [Item; NUM_ITEM_SLOTS];

/// Go `EnchantFromProto`, by effect ID.
pub(crate) fn enchant(effect_id: i32) -> Option<Enchant> {
    let row = data::proto_row(&ENCHANTS, "proto.SimEnchant", i64::from(effect_id))?;
    Some(Enchant {
        effect_id: row.i32("effect_id"),
        stats: Stats::from_proto_array(&row.f64s("stats")),
        pseudo_stats: row.f64s("pseudo_stats"),
        weapon_damage: row.f64("weapon_damage"),
        enchant_effects: row
            .messages("enchant_effects")
            .into_iter()
            .cloned()
            .collect(),
        name: row.str("name").to_string(),
        item_type: row.enum_name("type"),
        enchant_type: row.enum_name("enchant_type"),
        extra_types: row.enum_names("extra_types"),
    })
}

fn random_suffix(id: i32) -> Option<RandomSuffix> {
    let row = data::proto_row(&RANDOM_SUFFIXES, "proto.ItemRandomSuffix", i64::from(id))?;
    Some(RandomSuffix {
        id: row.i32("id"),
        name: row.str("name").to_string(),
        stats: Stats::from_proto_array(&row.f64s("stats")),
    })
}

/// A database item's scaling option at key 0, which `NewItem` reads.
pub(crate) fn scaling_option(row: &Message) -> Option<&Message> {
    row.get("scaling_options").and_then(|value| match value {
        Value::Map(entries) => entries.iter().find_map(|(key, value)| match (key, value) {
            (Value::Int(0), Value::Message(message)) => Some(message),
            _ => None,
        }),
        _ => None,
    })
}

/// Go `stats.FromProtoMap` over a `map<int32, double>` field.
pub(crate) fn stats_from_map(message: &Message, field: &str) -> Stats {
    let mut out = Stats::default();
    if let Some(Value::Map(entries)) = message.get(field) {
        for (key, value) in entries {
            if let (Value::Int(key), Value::Double(value)) = (key, value) {
                out.0[*key as usize] = *value;
            }
        }
    }
    out
}

/// Go `GetItemByID` with `ItemFromProto`: the database item without per-instance data.
pub(crate) fn database_item(id: i32) -> Option<Item> {
    let row = data::proto_row(&ITEMS, "proto.SimItem", i64::from(id))?;
    Some(Item {
        id: row.i32("id"),
        name: row.str("name").to_string(),
        item_type: row.enum_name("type"),
        armor_type: row.enum_name("armor_type"),
        weapon_type: row.enum_name("weapon_type"),
        hand_type: row.enum_name("hand_type"),
        ranged_weapon_type: row.enum_name("ranged_weapon_type"),
        swing_speed: row.f64("weapon_speed"),
        quality_modifier: row.f64("quality_modifier"),
        pseudo_stats: row.f64s("pseudo_stats"),
        unique: row.bool("unique"),
        limit_category: row.i32("limit_category"),
        set_name: row.str("set_name").to_string(),
        set_id: row.i32("set_id"),
        class_allowlist: row.enum_names("class_allowlist"),
        item_effects: row.messages("item_effects").into_iter().cloned().collect(),
        row: Some(row),
        ..Item::default()
    })
}

/// Go `NewItem` for one `ItemSpec`.
pub(crate) fn new_item(spec: &Message) -> Result<Item, Refusal> {
    let id = spec.i32("id");
    let mut item = database_item(id)
        .ok_or_else(|| Refusal::new("unknown_item", format!("no item with id {id}")))?;
    if !spec.ints("gems").iter().all(|gem| *gem == 0) {
        return Err(Refusal::new(
            "gems",
            format!("item {id} has gems, which the database has none of"),
        ));
    }
    let row = item.row.clone().expect("a database item keeps its row");
    let scaling = scaling_option(&row).ok_or_else(|| {
        Refusal::new(
            "unknown_item",
            format!("item {id} has no base scaling option"),
        )
    })?;
    item.stats = stats_from_map(scaling, "stats");
    item.weapon_damage_max = scaling.f64("weapon_damage_max");
    item.weapon_damage_min = scaling.f64("weapon_damage_min");
    item.rand_prop_points = scaling.i32("rand_prop_points");
    let suffix = spec.i32("random_suffix");
    if suffix != 0 {
        item.random_suffix = random_suffix(suffix).ok_or_else(|| {
            Refusal::new("unknown_item", format!("no random suffix with id {suffix}"))
        })?;
    }
    let enchant_id = spec.i32("enchant");
    if enchant_id != 0 {
        // Go silently ignores an enchant it does not know.
        if let Some(found) = enchant(enchant_id) {
            item.enchant = found;
        }
    }
    Ok(item)
}

/// Go `ItemTypeToSlot`.
fn item_type_to_slot(item_type: &str) -> Option<usize> {
    Some(match item_type {
        "ItemTypeHead" => slot::HEAD,
        "ItemTypeNeck" => slot::NECK,
        "ItemTypeShoulder" => slot::SHOULDER,
        "ItemTypeBack" => slot::BACK,
        "ItemTypeChest" => slot::CHEST,
        "ItemTypeWrist" => slot::WRIST,
        "ItemTypeHands" => slot::HANDS,
        "ItemTypeWaist" => slot::WAIST,
        "ItemTypeLegs" => slot::LEGS,
        "ItemTypeFeet" => slot::FEET,
        "ItemTypeFinger" => slot::FINGER1,
        "ItemTypeTrinket" => slot::TRINKET1,
        "ItemTypeWeapon" => slot::MAIN_HAND,
        "ItemTypeRanged" => slot::RANGED,
        _ => return None,
    })
}

/// Go `Equipment.EquipItem`.
fn equip_item(equipment: &mut Equipment, item: Item) -> Result<(), Refusal> {
    match item.item_type.as_str() {
        "ItemTypeFinger" => {
            let target = if equipment[slot::FINGER1].is_empty() {
                slot::FINGER1
            } else {
                slot::FINGER2
            };
            equipment[target] = item;
        }
        "ItemTypeTrinket" => {
            let target = if equipment[slot::TRINKET1].is_empty() {
                slot::TRINKET1
            } else {
                slot::TRINKET2
            };
            equipment[target] = item;
        }
        "ItemTypeWeapon" => {
            if item.weapon_type == "WeaponTypeShield"
                && equipment[slot::MAIN_HAND].hand_type != "HandTypeTwoHand"
            {
                equipment[slot::OFF_HAND] = item;
            } else if item.hand_type == "HandTypeMainHand" || item.hand_type == "HandTypeUnknown" {
                equipment[slot::MAIN_HAND] = item;
            } else if item.hand_type == "HandTypeOffHand" {
                equipment[slot::OFF_HAND] = item;
            } else if item.hand_type == "HandTypeOneHand" || item.hand_type == "HandTypeTwoHand" {
                if equipment[slot::MAIN_HAND].is_empty() {
                    equipment[slot::MAIN_HAND] = item;
                } else if equipment[slot::OFF_HAND].is_empty() {
                    equipment[slot::OFF_HAND] = item;
                }
            }
        }
        other => {
            let index = item_type_to_slot(other).ok_or_else(|| {
                Refusal::new(
                    "unknown_item",
                    format!("item {} has type {other}, which has no slot", item.id),
                )
            })?;
            equipment[index] = item;
        }
    }
    Ok(())
}

/// Go `ProtoToEquipment(...).inArea(areaTypes)`.
pub(crate) fn equipment(
    spec: Option<&Message>,
    area_types: &[String],
) -> Result<Equipment, Refusal> {
    let mut equipment: Equipment = Default::default();
    if let Some(spec) = spec {
        for (index, item) in spec.messages("items").into_iter().enumerate() {
            if index >= NUM_ITEM_SLOTS {
                return Err(Refusal::new(
                    "unknown_item",
                    "more items than slots".to_string(),
                ));
            }
            if item.i32("id") != 0 {
                equip_item(&mut equipment, new_item(item)?)?;
            }
        }
    }
    if !area_types.is_empty() {
        for item in equipment.iter_mut() {
            *item = in_area(std::mem::take(item), area_types);
        }
    }
    Ok(equipment)
}

/// Go `Item.inArea`: the item's stats with those of its base scaling for the areas.
pub(crate) fn in_area(mut item: Item, area_types: &[String]) -> Item {
    if item.is_empty() || area_types.is_empty() {
        return item;
    }
    let row = item.row.clone().expect("a database item keeps its row");
    if let Some(scaling) = scaling_option(&row) {
        for area in scaling.messages("area_stats") {
            if area_types.contains(&area.enum_name("area_type")) {
                item.stats = item.stats.add(&stats_from_map(area, "stats"));
            }
        }
    }
    item
}

/// Go `PercentPseudoStats` and `FromPseudoStatsProto`.
pub(crate) fn stats_from_pseudo_stats(pseudo: &[f64]) -> Stats {
    let value = |name: &str| {
        let index = crate::contracts::request::enum_number("proto.PseudoStat", name)
            .unwrap_or_else(|| panic!("proto.PseudoStat has no {name}"))
            as usize;
        pseudo.get(index).copied().unwrap_or(0.0)
    };
    let mut out = Stats::default();
    out[Stat::PhysicalHitPercent] = value("PseudoStatMeleeHitPercent");
    out[Stat::SpellHitPercent] = value("PseudoStatSpellHitPercent");
    out[Stat::PhysicalCritPercent] = value("PseudoStatMeleeCritPercent");
    out[Stat::SpellCritPercent] = value("PseudoStatSpellCritPercent");
    out[Stat::BlockPercent] = value("PseudoStatBlockPercent");
    out[Stat::RangedHitPercent] =
        value("PseudoStatRangedHitPercent") - value("PseudoStatMeleeHitPercent");
    out[Stat::RangedCritPercent] =
        value("PseudoStatRangedCritPercent") - value("PseudoStatMeleeCritPercent");
    out[Stat::DodgePercent] = value("PseudoStatDodgePercent");
    out[Stat::ParryPercent] = value("PseudoStatParryPercent");
    out[Stat::ExpertisePercent] = value("PseudoStatExpertisePercent");
    out
}

/// Go `ItemEquipmentBaseStats`.
fn item_base_stats(item: &Item) -> Stats {
    if item.is_empty() {
        return Stats::default();
    }
    Stats::default()
        .add(&item.stats)
        .add(&stats_from_pseudo_stats(&item.pseudo_stats))
        .add(&item.random_suffix.stats)
}

/// Go `ItemEquipmentGemAndEnchantStats`, without gems, which the database has none of.
fn item_enchant_stats(item: &Item) -> Stats {
    if item.is_empty() {
        return Stats::default();
    }
    Stats::default()
        .add(&item.enchant.stats)
        .add(&stats_from_pseudo_stats(&item.enchant.pseudo_stats))
}

/// Go forever_rules.go `unifyGearHitAndCrit`.
fn unify_gear_hit_and_crit(mut equip: Stats) -> Stats {
    use super::character::constants::*;
    let hit = equip[Stat::MeleeHitRating] / PHYSICAL_HIT_RATING_PER_HIT_PERCENT
        + equip[Stat::SpellHitRating] / SPELL_HIT_RATING_PER_HIT_PERCENT;
    let crit = equip[Stat::MeleeCritRating] / PHYSICAL_CRIT_RATING_PER_CRIT_PERCENT
        + equip[Stat::SpellCritRating] / SPELL_CRIT_RATING_PER_CRIT_PERCENT;
    equip[Stat::MeleeHitRating] = hit * PHYSICAL_HIT_RATING_PER_HIT_PERCENT;
    equip[Stat::SpellHitRating] = hit * SPELL_HIT_RATING_PER_HIT_PERCENT;
    equip[Stat::MeleeCritRating] = crit * PHYSICAL_CRIT_RATING_PER_CRIT_PERCENT;
    equip[Stat::SpellCritRating] = crit * SPELL_CRIT_RATING_PER_CRIT_PERCENT;
    equip
}

/// Go `Equipment.Stats`.
pub(crate) fn equipment_stats(equipment: &Equipment) -> Stats {
    let mut out = Stats::default();
    for item in equipment {
        out = out.add(&item_base_stats(item));
        out = out.add(&item_enchant_stats(item));
    }
    unify_gear_hit_and_crit(out)
}

/// `stats.FromProtoMap(effect.GetScalingOptions()[int32(0)].GetStats())`: the stats an item or
/// enchant effect carries at the base scaling, zero where the effect names none.
pub(crate) fn effect_stats(effect: &Message) -> Stats {
    scaling_option(effect).map_or_else(Stats::default, |option| stats_from_map(option, "stats"))
}


/// One read-only candidate projected from MythicSim's embedded SimItem database.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct NativeItemOption {
    pub item_id: i32,
    pub name: String,
    /// The embedded database's base scaling-option item level, not required level.
    pub item_level: Option<i32>,
    /// Native equipment slot, e.g. chest, finger, one_hand, off_hand or ranged.
    pub slot: String,
    pub item_type: String,
    pub armor_type: String,
    pub weapon_type: String,
    pub hand_type: String,
    pub ranged_weapon_type: String,
    /// Base-scaling item stats, keyed by MythicSim stat name.
    pub stats: std::collections::BTreeMap<String, f64>,
    /// Raw proto pseudo-stat vector, kept separate from ordinary stats.
    pub pseudo_stats: Vec<f64>,
    pub weapon_damage_min: Option<f64>,
    pub weapon_damage_max: Option<f64>,
    pub weapon_speed: Option<f64>,
    pub quality_modifier: f64,
    pub unique: bool,
    pub limit_category: i32,
    pub set_id: i32,
    pub set_name: String,
    pub class_allowlist: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct NativeItemOptionsRefusal {
    pub code: String,
    pub reason: String,
}

/// Result of the native read-only item-options query.
///
/// SimItem currently has no required_level field. This response therefore
/// reports the source item level and native class/slot/proficiency eligibility,
/// but deliberately does not pretend item level is a character-level requirement.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct NativeItemOptionsResponse {
    pub status: String,
    pub class: String,
    pub level: i32,
    pub spec: String,
    pub slot: String,
    pub source: String,
    pub required_level_supported: bool,
    pub items: Vec<NativeItemOption>,
    pub refusal: Option<NativeItemOptionsRefusal>,
}

fn native_item_source_slot(item: &Item) -> Option<&'static str> {
    Some(match item.item_type.as_str() {
        "ItemTypeHead" => "head",
        "ItemTypeNeck" => "neck",
        "ItemTypeShoulder" => "shoulder",
        "ItemTypeBack" => "back",
        "ItemTypeChest" => "chest",
        "ItemTypeWrist" => "wrist",
        "ItemTypeHands" => "hands",
        "ItemTypeWaist" => "waist",
        "ItemTypeLegs" => "legs",
        "ItemTypeFeet" => "feet",
        "ItemTypeFinger" => "finger",
        "ItemTypeTrinket" => "trinket",
        "ItemTypeWeapon" => match item.weapon_type.as_str() {
            "WeaponTypeOffHand" => "off_hand",
            _ => match item.hand_type.as_str() {
                "HandTypeOneHand" => "one_hand",
                "HandTypeTwoHand" => "two_hand",
                "HandTypeOffHand" => "off_hand",
                "HandTypeMainHand" | "HandTypeUnknown" => "main_hand",
                _ => return None,
            },
        },
        "ItemTypeRanged" => match item.ranged_weapon_type.as_str() {
            "RangedWeaponTypeIdol"
            | "RangedWeaponTypeLibram"
            | "RangedWeaponTypeTotem"
            | "RangedWeaponTypeSigil" => "relic",
            "RangedWeaponTypeThrown" => "thrown",
            "RangedWeaponTypeUnknown" => return None,
            _ => "ranged",
        },
        _ => return None,
    })
}

fn native_item_matches_slot(item: &Item, source_slot: &str, requested_slot: &str) -> bool {
    match requested_slot {
        "all" => true,
        "ring" | "finger" => source_slot == "finger",
        "weapon" => matches!(source_slot, "main_hand" | "one_hand" | "two_hand"),
        "main_hand" => matches!(source_slot, "main_hand" | "one_hand" | "two_hand"),
        "one_hand" | "two_hand" => source_slot == requested_slot,
        "offhand" | "off_hand" => source_slot == "off_hand",
        "wand" => {
            source_slot == "ranged" && item.ranged_weapon_type == "RangedWeaponTypeWand"
        }
        "ranged" => {
            source_slot == "ranged" && item.ranged_weapon_type != "RangedWeaponTypeWand"
        }
        "trinket" | "relic" => source_slot == requested_slot,
        "head" | "neck" | "shoulder" | "back" | "chest" | "wrist" | "hands"
        | "waist" | "legs" | "feet" | "shirt" | "tabard" => source_slot == requested_slot,
        _ => false,
    }
}

fn native_item_class_eligible(item: &Item, class: &str, class_proto: &str) -> bool {
    if !item.class_allowlist.is_empty()
        && !item.class_allowlist.iter().any(|allowed| allowed == class_proto)
    {
        return false;
    }

    let slot = native_item_source_slot(item);
    if matches!(
        slot,
        Some("head" | "shoulder" | "chest" | "wrist" | "hands" | "waist" | "legs" | "feet")
    ) && !matches!(item.armor_type.as_str(), "ArmorTypeUnknown" | "ArmorTypeCloth")
    {
        return false;
    }

    match item.item_type.as_str() {
        "ItemTypeWeapon" => match item.weapon_type.as_str() {
            "WeaponTypeOffHand" => true,
            "WeaponTypeDagger" | "WeaponTypeStaff" => true,
            "WeaponTypeMace" => class == "priest",
            "WeaponTypeSword" => class == "mage",
            _ => false,
        },
        "ItemTypeRanged" => item.ranged_weapon_type == "RangedWeaponTypeWand",
        _ => true,
    }
}

/// Enumerate the native embedded item choices for the supported L20 P3 caster slice.
///
/// This is a read-only projection over data/items.jsonl; it does not open the
/// Forever SQLite catalogue. Priest and Mage have fixed cloth/weapon proficiencies
/// for this slice. The native SimItem schema has no required-level property, so
/// callers receive required_level_supported=false rather than a fabricated
/// required-level estimate derived from item level.
pub fn native_item_options(
    class: &str,
    level: i32,
    spec: &str,
    slot: &str,
) -> NativeItemOptionsResponse {
    let class_input = class.trim().to_ascii_lowercase();
    let spec_input = spec.trim().to_ascii_lowercase();
    let slot_input = slot.trim().to_ascii_lowercase().replace('-', "_");
    let normalized_class = match class_input.as_str() {
        "priest" | "classpriest" => Some(("priest", "ClassPriest")),
        "mage" | "classmage" => Some(("mage", "ClassMage")),
        _ => None,
    };
    let canonical_slot = match slot_input.as_str() {
        "all" | "head" | "neck" | "shoulder" | "back" | "chest" | "wrist"
        | "hands" | "waist" | "legs" | "feet" | "shirt" | "tabard" | "relic"
        | "ring" | "finger" | "trinket" | "weapon" | "main_hand" | "one_hand"
        | "two_hand" | "offhand" | "off_hand" | "ranged" | "wand" => {
            Some(slot_input.as_str())
        }
        _ => None,
    };
    let refusal = if normalized_class.is_none() {
        Some(("NATIVE_ITEM_OPTIONS_UNSUPPORTED_CLASS", format!(
            "native item-options currently supports priest and mage, not {class}"
        )))
    } else if !(1..=60).contains(&level) {
        Some(("NATIVE_ITEM_OPTIONS_INVALID_LEVEL", format!(
            "level must be between 1 and 60, received {level}"
        )))
    } else if spec_input != "unrestricted" {
        Some(("NATIVE_ITEM_OPTIONS_UNSUPPORTED_SPEC", format!(
            "native item-options currently supports the unrestricted profile, not {spec}"
        )))
    } else if canonical_slot.is_none() {
        Some(("NATIVE_ITEM_OPTIONS_UNSUPPORTED_SLOT", format!(
            "unsupported item slot {slot}"
        )))
    } else {
        None
    };

    let mut response = NativeItemOptionsResponse {
        status: "refused".to_string(),
        class: class_input,
        level,
        spec: spec_input,
        slot: slot_input,
        source: "mythicsim:data/items.jsonl".to_string(),
        required_level_supported: false,
        items: Vec::new(),
        refusal: refusal.map(|(code, reason)| NativeItemOptionsRefusal {
            code: code.to_string(),
            reason,
        }),
    };
    if response.refusal.is_some() {
        return response;
    }

    let (class, class_proto) = normalized_class.expect("validated native item class");
    let requested_slot = canonical_slot.expect("validated native item slot");
    for key in ITEMS.keys() {
        let Ok(id) = i32::try_from(key) else {
            continue;
        };
        let Some(item) = database_item(id) else {
            continue;
        };
        let Some(source_slot) = native_item_source_slot(&item) else {
            continue;
        };
        if !native_item_matches_slot(&item, source_slot, requested_slot)
            || !native_item_class_eligible(&item, class, class_proto)
        {
            continue;
        }
        let Some(row) = item.row.as_deref() else {
            continue;
        };
        let Some(scaling) = scaling_option(row) else {
            continue;
        };
        let source_stats = stats_from_map(scaling, "stats");
        let mut stats = std::collections::BTreeMap::new();
        for stat in Stat::ALL {
            let value = source_stats[stat];
            if value != 0.0 {
                stats.insert(stat.name().to_string(), value);
            }
        }
        response.items.push(NativeItemOption {
            item_id: item.id,
            name: item.name,
            item_level: scaling.has("ilvl").then(|| scaling.i32("ilvl")),
            slot: source_slot.to_string(),
            item_type: item.item_type,
            armor_type: item.armor_type,
            weapon_type: item.weapon_type,
            hand_type: item.hand_type,
            ranged_weapon_type: item.ranged_weapon_type,
            stats,
            pseudo_stats: item.pseudo_stats,
            weapon_damage_min: scaling.has("weapon_damage_min").then(|| scaling.f64("weapon_damage_min")),
            weapon_damage_max: scaling.has("weapon_damage_max").then(|| scaling.f64("weapon_damage_max")),
            weapon_speed: (item.swing_speed > 0.0).then_some(item.swing_speed),
            quality_modifier: item.quality_modifier,
            unique: item.unique,
            limit_category: item.limit_category,
            set_id: item.set_id,
            set_name: item.set_name,
            class_allowlist: item.class_allowlist,
        });
    }
    response.items.sort_by(|a, b| {
        b.item_level
            .unwrap_or(-1)
            .cmp(&a.item_level.unwrap_or(-1))
            .then_with(|| a.item_id.cmp(&b.item_id))
    });
    response.status = if response.items.is_empty() {
        "empty".to_string()
    } else {
        "ok".to_string()
    };
    response
}

#[cfg(test)]
mod native_item_options_tests {
    use super::native_item_options;

    fn ids(class: &str, slot: &str) -> Vec<i32> {
        let response = native_item_options(class, 20, "unrestricted", slot);
        assert_eq!(response.status, "ok", "{response:?}");
        assert!(response.refusal.is_none());
        assert!(!response.items.is_empty());
        response.items.into_iter().map(|item| item.item_id).collect()
    }

    #[test]
    fn priest_and_mage_receive_native_cloth_options_but_not_leather() {
        for class in ["priest", "mage"] {
            let chest = ids(class, "chest");
            assert!(chest.contains(&200), "{class} should receive Thick Cloth Vest");
            assert!(!chest.contains(&60), "{class} must not receive Layered Tunic");
        }
    }

    #[test]
    fn native_class_and_weapon_proficiencies_distinguish_priest_from_mage() {
        let priest_weapons = ids("priest", "weapon");
        let mage_weapons = ids("mage", "weapon");
        assert!(priest_weapons.contains(&36), "Priest can use Worn Mace");
        assert!(!priest_weapons.contains(&25), "Priest cannot use Worn Shortsword");
        assert!(mage_weapons.contains(&25), "Mage can use Worn Shortsword");
        assert!(!mage_weapons.contains(&36), "Mage cannot use Worn Mace");
    }

    #[test]
    fn native_class_allowlist_and_wand_options_are_respected() {
        let priest_trinkets = ids("priest", "trinket");
        let mage_trinkets = ids("mage", "trinket");
        assert!(priest_trinkets.contains(&2919), "Priest-only relic remains Priest-only");
        assert!(!mage_trinkets.contains(&2919), "Priest-only relic is excluded for Mage");

        let mage_offhands = ids("mage", "offhand");
        let priest_offhands = ids("priest", "offhand");
        assert!(mage_offhands.contains(&7507), "Mage-only Arcane Orb remains Mage-only");
        assert!(!priest_offhands.contains(&7507), "Mage-only Arcane Orb is excluded for Priest");

        assert!(ids("priest", "wand").contains(&5069));
        assert!(ids("mage", "wand").contains(&5069));
        let no_ranged = native_item_options("priest", 20, "unrestricted", "ranged");
        assert_eq!(no_ranged.status, "empty");
        assert!(no_ranged.items.is_empty());
    }

    #[test]
    fn native_options_return_stats_and_explicit_refusals() {
        let response = native_item_options("mage", 20, "unrestricted", "chest");
        let robe = response.items.iter().find(|item| item.item_id == 200).unwrap();
        assert_eq!(robe.item_level, Some(22));
        assert!(robe.stats.contains_key("Armor"));
        assert!(!response.required_level_supported);

        let unknown = native_item_options("warrior", 20, "unrestricted", "chest");
        assert_eq!(unknown.status, "refused");
        assert_eq!(unknown.refusal.unwrap().code, "NATIVE_ITEM_OPTIONS_UNSUPPORTED_CLASS");

        let bad_level = native_item_options("priest", 61, "unrestricted", "chest");
        assert_eq!(bad_level.status, "refused");
        assert_eq!(bad_level.refusal.unwrap().code, "NATIVE_ITEM_OPTIONS_INVALID_LEVEL");
    }
}
