//! The Paladin talent strings of the 70425 and 70707 trees read in push 113554's tree, which
//! Forever's rules prepare (`src/classes/paladin/prepare/hotfix_113554.rs`): each talent's
//! points move to its new square, capped at its new ranks; a removed talent's points are dropped
//! and Twist of Light's go to Unsealed Wrath, its rework in the same TraitNode. The result need not
//! respect the tree's tier requirements, which preparation does not check. Tests that prepare an
//! old fixture under Forever's rules read its talents through this.

#![allow(dead_code)]

use serde_json::Value;

/// The Go reference's order (proto field numbers), as the old strings are written.
const OLD: [&[&str]; 3] = [
    &[
        "divine_strength",
        "divine_intellect",
        "healing_light",
        "spiritual_focus",
        "improved_seals",
        "unyielding_faith",
        "voice_of_truth",
        "reverence",
        "purifying_power",
        "infusion_of_light",
        "illumination",
        "divine_favor",
        "divine_precision",
        "holy_shock",
        "consecrated_ground",
        "holy_power",
        "lights_vigil",
    ],
    &[
        "toughness",
        "redoubt",
        "precision",
        "guardians_favor",
        "anticipation",
        "improved_seal_of_fury",
        "improved_righteous_fury",
        "shield_specialization",
        "sacred_duty",
        "swift_judgement",
        "one_handed_weapon_specialization",
        "improved_hammer_of_justice",
        "templars_bulwark",
        "reckoning",
        "iron_creed",
        "holy_shield",
    ],
    &[
        "deflection",
        "benediction",
        "improved_judgement",
        "holy_conduit",
        "conviction",
        "vindication",
        "sanctified_judgement",
        "seal_of_command",
        "pursuit_of_justice",
        "eye_for_an_eye",
        "sacred_arbiter",
        "two_handed_weapon_specialization",
        "vengeance",
        "repentance",
        "champion_of_the_light",
        "instrument_of_law",
        "twist_of_light",
    ],
];

/// Push 113554's tree in the client's order, with each talent's ranks.
const NEW: [&[(&str, u32)]; 3] = [
    &[
        ("spiritual_focus", 5),
        ("divine_strength", 5),
        ("healing_light", 3),
        ("reverence", 2),
        ("improved_seals", 5),
        ("purifying_power", 2),
        ("divine_favor", 1),
        ("illumination", 5),
        ("holy_conduit", 2),
        ("divine_precision", 2),
        ("infusion_of_light", 2),
        ("oath_of_light", 1),
        ("consecrated_ground", 2),
        ("holy_shock", 1),
        ("boundless_light", 2),
        ("holy_power", 5),
        ("lights_vigil", 1),
    ],
    &[
        ("divine_intellect", 5),
        ("redoubt", 5),
        ("precision", 3),
        ("guardians_favor", 2),
        ("sacred_duty", 2),
        ("templars_bulwark", 1),
        ("improved_hammer_of_justice", 3),
        ("shield_specialization", 2),
        ("toughness", 5),
        ("swift_judgement", 1),
        ("improved_righteous_fury", 3),
        ("anticipation", 5),
        ("stoicism", 2),
        ("holy_shield", 1),
        ("reckoning", 5),
        ("iron_creed", 3),
        ("shieldsworn", 1),
    ],
    &[
        ("deflection", 5),
        ("benediction", 5),
        ("improved_judgement", 2),
        ("conviction", 3),
        ("sanctified_judgement", 3),
        ("vindication", 3),
        ("unyielding_faith", 2),
        ("seal_of_command", 1),
        ("pursuit_of_justice", 2),
        ("eye_for_an_eye", 2),
        ("sacred_arbiter", 1),
        ("two_handed_weapon_specialization", 3),
        ("turn_evil", 2),
        ("repentance", 1),
        ("vengeance", 3),
        ("champion_of_the_light", 5),
        ("unsealed_wrath", 1),
    ],
];

/// An old string's talents in push 113554's tree.
pub fn forever_talents(old: &str) -> String {
    let mut points = std::collections::BTreeMap::new();
    for (tree, digits) in old.split('-').enumerate() {
        for (i, digit) in digits.chars().enumerate() {
            let field = match OLD[tree][i] {
                "twist_of_light" => "unsealed_wrath",
                field => field,
            };
            points.insert(field, digit.to_digit(10).unwrap_or(0));
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

/// A request whose player is a paladin, with its talents read in push 113554's tree.
pub fn forever_request(request: &mut Value) {
    for party in request["raid"]["parties"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        for player in party["players"].as_array_mut().into_iter().flatten() {
            if player["class"] == "ClassPaladin" {
                if let Some(old) = player["talentsString"].as_str() {
                    player["talentsString"] = Value::from(forever_talents(old));
                }
            }
        }
    }
}
