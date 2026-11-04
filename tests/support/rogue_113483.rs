//! The Rogue talent strings of the 70425 and 70707 trees read in push 113483's trees, which
//! Forever's rules prepare (`classes::rogue::prepare::FOREVER_TREES`): each talent's points move to
//! its new square, capped at its new ranks; a removed talent's points are dropped. The result need
//! not respect the trees' tier requirements, which preparation does not check. Tests that prepare
//! an old fixture under Forever's rules read its talents through this.

#![allow(dead_code)]

use serde_json::Value;

/// The Go reference's order (proto field numbers), as the old strings are written.
const OLD: [&[&str]; 3] = [
    &[
        "improved_gouge",
        "remorseless_attacks",
        "malice",
        "ruthlessness",
        "murder",
        "improved_slice_and_dice",
        "relentless_strikes",
        "improved_expose_armor",
        "lethality",
        "vile_poisons",
        "cold_blood",
        "improved_poisons",
        "vigor",
        "mutilate",
        "improved_kidney_shot",
        "seal_fate",
        "venom",
    ],
    &[
        "improved_eviscerate",
        "improved_sinister_strike",
        "lightning_reflexes",
        "puncturing_wounds",
        "deflection",
        "precision",
        "endurance",
        "riposte",
        "improved_sprint",
        "improved_kick",
        "flawless_execution",
        "dual_wield_specialization",
        "blade_flurry",
        "hack_and_slash",
        "weapon_expertise",
        "aggression",
        "adrenaline_rush",
    ],
    &[
        "camouflage",
        "master_of_deception",
        "opportunity",
        "setup",
        "elusiveness",
        "dirty_tricks",
        "improved_ambush",
        "initiative",
        "ghostly_strike",
        "improved_distract",
        "heightened_senses",
        "premeditation",
        "serrated_blades",
        "dirty_deeds",
        "preparation",
        "hemorrhage",
        "quietus",
        "cutthroat",
        "thousand_cuts",
    ],
];

/// Push 113483's trees in the client's order, with each talent's ranks.
const NEW: [&[(&str, u32)]; 3] = [
    &[
        ("improved_gouge", 3),
        ("remorseless_attacks", 2),
        ("malice", 5),
        ("ruthlessness", 3),
        ("improved_expose_armor", 2),
        ("improved_slice_and_dice", 3),
        ("vigor", 2),
        ("blood_splatter", 1),
        ("lethality", 2),
        ("vile_poisons", 5),
        ("cold_blood", 1),
        ("improved_poisons", 5),
        ("relentless_strikes", 5),
        ("venom", 1),
        ("improved_kidney_shot", 2),
        ("seal_fate", 3),
        ("mutilate", 1),
    ],
    &[
        ("improved_eviscerate", 3),
        ("improved_sinister_strike", 2),
        ("lightning_reflexes", 5),
        ("puncturing_wounds", 3),
        ("deflection", 3),
        ("precision", 3),
        ("improved_sprint", 2),
        ("endurance", 2),
        ("riposte", 1),
        ("composure", 2),
        ("improved_kick", 2),
        ("flawless_execution", 1),
        ("weapon_expertise", 2),
        ("aggression", 3),
        ("dual_wield_specialization", 2),
        ("blade_flurry", 1),
        ("hack_and_slash", 5),
        ("flash_of_steel", 5),
        ("adrenaline_rush", 1),
    ],
    &[
        ("setup", 3),
        ("master_of_deception", 3),
        ("opportunity", 5),
        ("dirty_deeds", 2),
        ("camouflage", 3),
        ("improved_ambush", 3),
        ("dirty_tricks", 2),
        ("initiative", 3),
        ("premeditation", 1),
        ("serrated_blades", 3),
        ("elusiveness", 2),
        ("ghostly_strike", 1),
        ("quick_getaway", 2),
        ("heightened_senses", 2),
        ("preparation", 1),
        ("hemorrhage", 1),
        ("boot_knife", 5),
        ("death_mark", 1),
    ],
];

/// An old string's talents in push 113483's trees.
pub fn forever_talents(old: &str) -> String {
    let mut points = std::collections::BTreeMap::new();
    for (tree, digits) in old.split('-').enumerate().take(3) {
        for (i, digit) in digits.chars().enumerate() {
            if let Some(field) = OLD[tree].get(i) {
                points.insert(*field, digit.to_digit(10).unwrap_or(0));
            }
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
    trees.join("-").trim_end_matches('-').to_string()
}

/// A request whose player is a rogue, with its talents read in push 113483's trees.
pub fn forever_request(request: &mut Value) {
    for party in request["raid"]["parties"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        for player in party["players"].as_array_mut().into_iter().flatten() {
            if player["class"] == "ClassRogue" {
                if let Some(old) = player["talentsString"].as_str() {
                    player["talentsString"] = Value::from(forever_talents(old));
                }
            }
        }
    }
}
