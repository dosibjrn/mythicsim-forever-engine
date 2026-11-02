//! The Paladin part of the prepared v2 build gate. The shared gate is in
//! `engine/coverage.rs`.

use crate::{
    contracts::prepared_v2::{Effect, PreparedV2, Spell},
    engine::coverage::ClassGate,
};

pub(crate) const GATE: ClassGate = ClassGate {
    class: "ClassPaladin",
    effects: EFFECTS,
    spell: spell_capability,
    claims,
    limits,
    several_targets: Some(several_targets),
    tanks_several_targets: true,
    other_target_casts: None,
    self_spells: &[],
};

/// Paladin effect kinds implemented in Rust and validated against the pinned Go reference.
const EFFECTS: &[&str] = &[
    "consecration",
    "divine_favor",
    "exorcism",
    "eye_for_an_eye",
    "hammer_of_wrath",
    "holy_shield",
    "holy_shock",
    "holy_light_haste",
    "holy_wrath",
    "holy_strike",
    "hammer_of_the_righteous",
    "illumination",
    "iron_creed",
    "judgement",
    "judgement_refresh",
    "lay_on_hands",
    "lights_vigil",
    "lights_vigil_ally",
    "oath_of_light",
    "paladin_aura_cast",
    "paladin_heals",
    "pursuit_of_justice",
    "reckoning",
    "redoubt",
    "righteous_fury",
    "sacred_arbiter",
    "sanctified_judgement",
    "second_judgement",
    "seal_of_command",
    "seal_of_fury",
    "seal_of_justice",
    "seal_of_light",
    "seal_of_righteousness",
    "seal_of_the_crusader",
    "seal_of_wisdom",
    "shield_specialization",
    "shieldsworn",
    "swift_judgement",
    "templars_bulwark",
    "twist_of_light",
    "unsealed_wrath",
    "vengeance",
    "vindication",
];

/// The effect kind whose implementation executes a Paladin spell.
fn spell_capability(spell: &Spell) -> Option<&'static str> {
    match spell.class_spell.as_deref()? {
        "seal_of_command" | "seal_of_command_proc" | "judgement_of_command" => {
            Some("seal_of_command")
        }
        "seal_of_righteousness" | "seal_of_righteousness_proc" | "judgement_of_righteousness" => {
            Some("seal_of_righteousness")
        }
        "seal_of_the_crusader" | "judgement_of_the_crusader" => Some("seal_of_the_crusader"),
        "seal_of_fury" | "seal_of_fury_proc" | "judgement_of_fury" => Some("seal_of_fury"),
        "seal_of_light" | "seal_of_light_proc" | "judgement_of_light" => Some("seal_of_light"),
        "seal_of_wisdom" | "seal_of_wisdom_proc" | "judgement_of_wisdom" => Some("seal_of_wisdom"),
        "seal_of_justice" | "judgement_of_justice" => Some("seal_of_justice"),
        "judgement" => Some("judgement"),
        "holy_strike" => Some("holy_strike"),
        "hammer_of_the_righteous" => Some("hammer_of_the_righteous"),
        "hammer_of_wrath" => Some("hammer_of_wrath"),
        "consecration" => Some("consecration"),
        "exorcism" => Some("exorcism"),
        "holy_wrath" => Some("holy_wrath"),
        "holy_shock" => Some("holy_shock"),
        "lights_vigil" | "lights_vigil_strike" => Some("lights_vigil"),
        "lights_vigil_ally" | "lights_vigil_heal" => Some("lights_vigil_ally"),
        "divine_favor" => Some("divine_favor"),
        "oath_of_light" => Some("oath_of_light"),
        "righteous_fury" => Some("righteous_fury"),
        "swift_judgement" => Some("swift_judgement"),
        "second_judgement" => Some("second_judgement"),
        "templars_bulwark" => Some("templars_bulwark"),
        "holy_shield" | "holy_shield_proc" => Some("holy_shield"),
        "holy_light" | "flash_of_light" | "holy_shock_heal" => Some("paladin_heals"),
        "lay_on_hands" => Some("lay_on_hands"),
        "retribution_aura"
        | "devotion_aura"
        | "concentration_aura"
        | "fire_resistance_aura"
        | "frost_resistance_aura"
        | "shadow_resistance_aura" => Some("paladin_aura_cast"),
        _ => None,
    }
}

/// Aura labels a Paladin effect takes responsibility for, as (unit, label): its listeners and
/// every aura its behavior can activate.
fn claims(effect: &Effect) -> Vec<(&'static str, &str)> {
    match effect {
        Effect::JudgementRefresh { trigger_aura, .. }
        | Effect::SanctifiedJudgement { trigger_aura, .. }
        | Effect::SacredArbiter { trigger_aura, .. }
        | Effect::ShieldSpecialization { trigger_aura, .. }
        | Effect::Illumination { trigger_aura, .. }
        | Effect::EyeForAnEye { trigger_aura, .. }
        | Effect::PursuitOfJustice {
            aura: trigger_aura, ..
        }
        | Effect::DivineFavor {
            aura: trigger_aura, ..
        }
        | Effect::OathOfLight {
            aura: trigger_aura, ..
        }
        | Effect::SwiftJudgement {
            aura: trigger_aura, ..
        } => vec![("player", trigger_aura)],
        Effect::Vengeance {
            trigger_aura, aura, ..
        }
        | Effect::Redoubt {
            trigger_aura, aura, ..
        }
        | Effect::IronCreed {
            trigger_aura, aura, ..
        }
        | Effect::HolyLightHaste {
            trigger_aura, aura, ..
        } => vec![("player", trigger_aura), ("player", aura)],
        Effect::Vindication {
            trigger_aura,
            aura,
            target_aura,
            ..
        } => {
            let mut claimed = vec![("player", trigger_aura.as_str()), ("target", target_aura)];
            if let Some(aura) = aura {
                claimed.push(("player", aura));
            }
            claimed
        }
        Effect::Reckoning {
            block_aura,
            crit_aura,
            charges_aura,
            ..
        } => vec![
            ("player", block_aura),
            ("player", crit_aura),
            ("player", charges_aura),
        ],
        Effect::RighteousFury {
            aura,
            instrument_of_law,
            ..
        } => {
            let mut claimed = vec![("player", aura.as_str())];
            if let Some(law) = instrument_of_law {
                claimed.push(("player", law.aura.as_str()));
            }
            claimed
        }
        Effect::Consecration {
            consecrated_ground: Some(ground),
            ..
        } => vec![("target", ground.aura.as_str())],
        Effect::TwistOfLight {
            trigger_aura,
            echoes,
        }
        | Effect::UnsealedWrath {
            trigger_aura,
            echoes,
        } => std::iter::once(("player", trigger_aura.as_str()))
            .chain(echoes.iter().map(|echo| ("player", echo.aura.as_str())))
            .collect(),
        Effect::LightsVigil { ranks } => ranks
            .iter()
            .map(|rank| ("target", rank.aura.as_str()))
            .collect(),
        Effect::LightsVigilAlly { ranks } => ranks
            .iter()
            .map(|rank| ("player", rank.aura.as_str()))
            .collect(),
        Effect::PaladinAuraCast { ranks } => ranks
            .iter()
            .map(|rank| ("player", rank.aura.as_str()))
            .collect(),
        Effect::HolyShield { ranks, .. } => ranks
            .iter()
            .map(|rank| ("player", rank.aura.as_str()))
            .collect(),
        Effect::SealOfCommand { ranks, .. } => ranks
            .iter()
            .map(|rank| ("player", rank.aura.as_str()))
            .collect(),
        Effect::SealOfRighteousness { ranks, .. } => ranks
            .iter()
            .map(|rank| ("player", rank.aura.as_str()))
            .collect(),
        Effect::TemplarsBulwark { aura, .. } => vec![("player", aura), ("player", "Forbearance")],
        Effect::SealOfFury { ranks, .. } => ranks
            .iter()
            .flat_map(|rank| {
                [
                    ("player", rank.aura.as_str()),
                    ("player", &rank.shield_aura),
                ]
            })
            .collect(),
        Effect::SealOfLight { ranks, .. } => ranks
            .iter()
            .flat_map(|rank| {
                [
                    ("player", rank.aura.as_str()),
                    ("target", &rank.judgement_aura),
                ]
            })
            .collect(),
        Effect::SealOfWisdom { ranks } => ranks
            .iter()
            .flat_map(|rank| {
                [
                    ("player", rank.aura.as_str()),
                    ("target", &rank.judgement_aura),
                ]
            })
            .collect(),
        Effect::SealOfJustice {
            aura,
            judgement_aura,
            ..
        } => vec![("player", aura), ("target", judgement_aura)],
        Effect::SealOfTheCrusader { ranks, .. } => ranks
            .iter()
            .flat_map(|rank| {
                [
                    ("player", rank.aura.as_str()),
                    ("target", &rank.judgement_aura),
                ]
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Class limits: Seal of the Crusader.
fn limits(prepared: &PreparedV2, reachable: &[&Spell]) -> Vec<String> {
    crusader_limits(prepared, reachable)
}

/// Seal of the Crusader's attack power rides on the stat aura combinations, which the
/// exporter builds for the ranks the rotation names; a reachable rank without one is
/// unsupported.
fn crusader_limits(prepared: &PreparedV2, reachable: &[&Spell]) -> Vec<String> {
    let Some(ranks) = prepared.effects.iter().find_map(|effect| match effect {
        Effect::SealOfTheCrusader { ranks, .. } => Some(ranks),
        _ => None,
    }) else {
        return Vec::new();
    };
    let stat_auras: Vec<&String> = prepared
        .effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::StatAuras { auras, .. } => Some(auras),
            _ => None,
        })
        .flatten()
        .collect();
    let mut reasons = Vec::new();
    for spell in reachable {
        if spell.class_spell.as_deref() != Some("seal_of_the_crusader") {
            continue;
        }
        let id = spell.action_id.clone().unwrap_or_default();
        let Some(rank) = ranks.iter().find(|rank| rank.seal_spell_id == id.spell_id) else {
            continue;
        };
        if !stat_auras.contains(&&rank.aura) {
            reasons.push(format!(
                "Seal of the Crusader {} without its stat aura combinations is unsupported",
                id.spell_id
            ));
        }
    }
    reasons
}

/// The spells that reach a target past the first in Go and not yet in Rust: none. Consecration
/// ticks on each target, with the bonus on the first four and Consecrated Ground marked on
/// each, and Holy Wrath rolls each Undead or Demon target, as in Go. A tank has every copy
/// of the boss swing at it, which the runtime follows.
fn several_targets(_prepared: &PreparedV2, reachable: &[&Spell]) -> Vec<String> {
    crate::engine::coverage::spells_reaching_other_targets(reachable, &[])
}
