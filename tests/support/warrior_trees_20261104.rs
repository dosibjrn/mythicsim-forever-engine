//! The Warrior talent strings of the 70425 tree (the Go proto's order) read in the client's order
//! of the development notes of 2 November 2026, which Forever's rules prepare
//! (`src/classes/warrior/prepare/talent_ranks.rs`): each talent's points move to its new square,
//! capped at its new ranks; Raging Blows' point is dropped. Freed points are not spent anywhere and
//! the result need not respect the tree's tier requirements, which preparation does not check.
//! Tests that prepare an old fixture under Forever's rules read its talents through this.

#![allow(dead_code)]

use serde_json::Value;

/// The Go reference's order (proto field numbers), as the old strings are written.
const OLD: [&[&str]; 3] = [
    &[
        "improved_heroic_strike",
        "deflection",
        "improved_rend",
        "improved_charge",
        "improved_tactical_mastery",
        "improved_overpower",
        "anger_management",
        "deep_wounds",
        "spearing_strike",
        "two_handed_weapon_specialization",
        "impale",
        "bloodthrill",
        "sweeping_strikes",
        "weaponmaster",
        "improved_slam",
        "improved_hamstring",
        "mortal_strike",
    ],
    &[
        "booming_voice",
        "cruelty",
        "lingering_rage",
        "unbridled_wrath",
        "furious_precision",
        "piercing_howl",
        "blood_craze",
        "dual_wield_specialization",
        "raging_blows",
        "enrage",
        "improved_execute",
        "improved_berserker_rage",
        "death_wish",
        "improved_intercept",
        "flurry",
        "gore_drinker",
        "bloodthirst",
    ],
    &[
        "improved_bloodrage",
        "shield_specialization",
        "iron_will",
        "anticipation",
        "improved_revenge",
        "improved_thunder_clap",
        "last_stand",
        "master_of_defense",
        "improved_disarm",
        "defiance",
        "improved_sunder_armor",
        "vanguard",
        "improved_shield_bash",
        "improved_shield_wall",
        "concussion_blow",
        "focused_rage",
        "bastion",
        "shield_slam",
    ],
];

/// The client's order with each talent's ranks. Arms and Protection keep their talents' places.
const NEW: [&[(&str, u32)]; 3] = [
    &[
        ("improved_heroic_strike", 3),
        ("deflection", 5),
        ("improved_rend", 3),
        ("improved_charge", 2),
        ("improved_tactical_mastery", 3),
        ("improved_overpower", 2),
        ("anger_management", 1),
        ("deep_wounds", 3),
        ("spearing_strike", 1),
        ("two_handed_weapon_specialization", 3),
        ("impale", 2),
        ("bloodthrill", 2),
        ("sweeping_strikes", 1),
        ("weaponmaster", 5),
        ("improved_slam", 2),
        ("improved_hamstring", 3),
        ("mortal_strike", 1),
    ],
    &[
        ("booming_voice", 5),
        ("cruelty", 5),
        ("lingering_rage", 5),
        ("unbridled_wrath", 5),
        ("furious_precision", 3),
        ("piercing_howl", 1),
        ("blood_craze", 3),
        ("enrage", 5),
        ("dual_wield_specialization", 5),
        ("reckless_swings", 1),
        ("improved_execute", 2),
        ("improved_berserker_rage", 2),
        ("death_wish", 1),
        ("gore_drinker", 2),
        ("improved_intercept", 2),
        ("improved_whirlwind", 2),
        ("flurry", 3),
        ("bloodthirst", 1),
    ],
    &[
        ("improved_bloodrage", 2),
        ("shield_specialization", 5),
        ("iron_will", 5),
        ("anticipation", 5),
        ("improved_revenge", 3),
        ("improved_thunder_clap", 3),
        ("last_stand", 1),
        ("master_of_defense", 2),
        ("improved_disarm", 3),
        ("defiance", 3),
        ("improved_sunder_armor", 3),
        ("vanguard", 1),
        ("improved_shield_bash", 2),
        ("improved_shield_wall", 2),
        ("concussion_blow", 1),
        ("focused_rage", 3),
        ("bastion", 5),
        ("shield_slam", 1),
    ],
];

/// An old string's talents in the client's order.
pub fn forever_talents(old: &str) -> String {
    let mut points = std::collections::BTreeMap::new();
    for (tree, digits) in old.split('-').enumerate() {
        for (i, digit) in digits.chars().enumerate() {
            points.insert(OLD[tree][i], digit.to_digit(10).unwrap_or(0));
        }
    }
    let trees: Vec<String> = NEW
        .iter()
        .map(|tree| {
            let digits: String = tree
                .iter()
                .map(|(field, ranks)| {
                    char::from_digit(points.get(field).copied().unwrap_or(0).min(*ranks), 10)
                        .unwrap()
                })
                .collect();
            digits.trim_end_matches('0').to_string()
        })
        .collect();
    trees.join("-")
}

/// A request whose player is a warrior, with its talents read in the client's order.
pub fn forever_request(request: &mut Value) {
    for party in request["raid"]["parties"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        for player in party["players"].as_array_mut().into_iter().flatten() {
            if player["class"] == "ClassWarrior" {
                if let Some(old) = player["talentsString"].as_str() {
                    player["talentsString"] = Value::from(forever_talents(old));
                }
            }
        }
    }
}

/// The same for a request in bytes.
pub fn forever_bytes(request: &[u8]) -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(request).unwrap();
    forever_request(&mut value);
    serde_json::to_vec(&value).unwrap()
}
