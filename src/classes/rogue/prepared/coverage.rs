//! The Rogue part of the prepared v2 build gate: Rogue effects, the effects of Rogue spells
//! and the auras Rogue effects claim. The shared gate is in `engine/coverage.rs`.

use crate::{
    contracts::prepared_v2::{Effect, PreparedV2, Spell},
    engine::coverage::ClassGate,
};

pub(crate) const GATE: ClassGate = ClassGate {
    class: "ClassRogue",
    effects: EFFECTS,
    spell: spell_capability,
    claims,
    limits,
    several_targets: Some(several_targets),
    tanks_several_targets: false,
    other_target_casts: Some(other_target_casts),
    self_spells: &["adrenaline_rush", "blade_flurry", "cold_blood", "vanish"],
};

/// Rogue effect kinds implemented in Rust and validated against the pinned Go reference.
const EFFECTS: &[&str] = &[
    "adrenaline_rush",
    "ambush",
    "backstab",
    "blade_flurry",
    "cold_blood",
    "deadly_poison",
    "eviscerate",
    "instant_poison",
    "mutilate",
    "premeditation",
    "preparation",
    "rogue_finisher",
    "rogue_proc",
    "rupture",
    "sinister_strike",
    "slice_and_dice",
    "stealth",
    "thousand_cuts",
    "venom",
    "wound_poison",
    "ghostly_strike",
    "hemorrhage",
    "garrote",
    "quietus",
    "kidney_shot",
    "expose_armor",
    "riposte",
];

/// The effect kind whose implementation executes a Rogue spell.
fn spell_capability(spell: &Spell) -> Option<&'static str> {
    match spell.class_spell.as_deref()? {
        "sinister_strike" => Some("sinister_strike"),
        "backstab" => Some("backstab"),
        "eviscerate" => Some("eviscerate"),
        "slice_and_dice" => Some("slice_and_dice"),
        "blade_flurry" => Some("blade_flurry"),
        "adrenaline_rush" => Some("adrenaline_rush"),
        "instant_poison" => Some("instant_poison"),
        "deadly_poison" => Some("deadly_poison"),
        "ambush" => Some("ambush"),
        "rupture" => Some("rupture"),
        "mutilate" | "mutilate_hit" => Some("mutilate"),
        "cold_blood" => Some("cold_blood"),
        "premeditation" => Some("premeditation"),
        "preparation" => Some("preparation"),
        "stealth" | "vanish" => Some("stealth"),
        "wound_poison" => Some("wound_poison"),
        "venom" => Some("venom"),
        "ghostly_strike" => Some("ghostly_strike"),
        "hemorrhage" => Some("hemorrhage"),
        "garrote" => Some("garrote"),
        "kidney_shot" => Some("kidney_shot"),
        "expose_armor" => Some("expose_armor"),
        "riposte" => Some("riposte"),
        _ => None,
    }
}

/// Aura labels a Rogue effect takes responsibility for, as (unit, label).
fn claims(effect: &Effect) -> Vec<(&'static str, &str)> {
    match effect {
        Effect::InstantPoison { trigger_aura, .. }
        | Effect::DeadlyPoison { trigger_aura, .. }
        | Effect::WoundPoison { trigger_aura, .. } => {
            vec![("player", trigger_aura)]
        }
        Effect::SliceAndDice { aura, .. }
        | Effect::BladeFlurry { aura, .. }
        | Effect::AdrenalineRush { aura, .. }
        | Effect::ColdBlood { aura, .. }
        | Effect::Venom { aura, .. }
        | Effect::Quietus { aura, .. }
        | Effect::ThousandCuts { aura, .. } => vec![("player", aura)],
        Effect::RogueProc { trigger_aura, .. } => vec![("player", trigger_aura)],
        Effect::ExposeArmor { aura, .. } => vec![("target", aura)],
        Effect::Riposte {
            trigger_aura,
            ready_aura,
            ..
        } => vec![("player", trigger_aura), ("player", ready_aura)],
        _ => Vec::new(),
    }
}

/// Rogue limits: finishers need the finisher effect, and every energy spell an energy bar.
fn limits(prepared: &PreparedV2, reachable: &[&Spell]) -> Vec<String> {
    let mut reasons = Vec::new();
    // The stun needs its aura unless the target is stun immune.
    let stun_supported = prepared.effects.iter().any(|effect| {
        matches!(
            effect,
            Effect::KidneyShot { target_stun_immune, stun_aura, .. }
                if *target_stun_immune || !stun_aura.is_empty()
        )
    });
    let finisher = prepared
        .effects
        .iter()
        .any(|effect| matches!(effect, Effect::RogueFinisher { .. }));
    // Expose Armor sets the target's armor through the major armor category the exporter
    // resolves, unless a permanent member blocks it for good.
    let expose_armor_supported = prepared.effects.iter().any(|effect| {
        matches!(
            effect,
            Effect::ExposeArmor {
                blocking_priority: Some(_),
                ..
            }
        )
    }) || prepared.effects.iter().any(|effect| {
        matches!(effect, Effect::ExclusiveCategory { unit, armor_by_stacks, .. }
            if unit == "target" && !armor_by_stacks.is_empty())
    });
    for spell in reachable {
        let class = spell.class_spell.as_deref();
        if matches!(
            class,
            Some(
                "eviscerate"
                    | "slice_and_dice"
                    | "rupture"
                    | "venom"
                    | "kidney_shot"
                    | "expose_armor"
            )
        ) && !finisher
        {
            reasons.push(format!(
                "rotation reaches finisher {} without the finisher effect",
                spell.action_id.clone().unwrap_or_default()
            ));
        }
        if class == Some("expose_armor") && !expose_armor_supported {
            reasons.push(format!(
                "rotation reaches {} without the target's armor category",
                spell.action_id.clone().unwrap_or_default()
            ));
        }
        if class == Some("kidney_shot") && !stun_supported {
            reasons.push(format!(
                "rotation reaches {}, whose stun has no aura",
                spell.action_id.clone().unwrap_or_default()
            ));
        }
        let energy = spell
            .cost
            .as_ref()
            .is_some_and(|cost| cost.resource == "energy");
        if energy && prepared.player.energy.is_none() {
            reasons.push(format!(
                "rotation reaches {}, which costs energy the player lacks",
                spell.action_id.clone().unwrap_or_default()
            ));
        }
    }
    reasons
}

/// The spells that reach a target past the first in Go and not yet in Rust: none, since Blade
/// Flurry's extra hit on the next target runs as in Go.
fn several_targets(_prepared: &PreparedV2, reachable: &[&Spell]) -> Vec<String> {
    crate::engine::coverage::spells_reaching_other_targets(reachable, &[])
}

/// A cast aimed at another target lands its hit, its dot (Rupture, Garrote, Deadly Poison) and
/// its debuff (Hemorrhage, Wound Poison) there as in Go, with combo points on the rogue. Expose
/// Armor's armor and Kidney Shot's stun stay on the first target, so those are refused.
fn other_target_casts(_prepared: &PreparedV2, spells: &[&Spell]) -> Vec<String> {
    spells
        .iter()
        .filter(|spell| {
            matches!(
                spell.class_spell.as_deref(),
                Some("expose_armor" | "kidney_shot")
            )
        })
        .map(|spell| {
            format!(
                "{} at a target past the first lands on the first target only",
                spell.action_id.clone().unwrap_or_default()
            )
        })
        .collect()
}
