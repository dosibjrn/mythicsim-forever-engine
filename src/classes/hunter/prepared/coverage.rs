//! The Hunter part of the prepared v2 build gate: Hunter effects, the effects of Hunter spells
//! and the auras Hunter effects claim. The shared gate is in `engine/coverage.rs`.

use crate::{
    classes::hunter::pet::PetAbility,
    contracts::prepared_v2::{Effect, PreparedV2, Spell},
    engine::coverage::ClassGate,
};

pub(crate) const GATE: ClassGate = ClassGate {
    class: "ClassHunter",
    effects: EFFECTS,
    spell: spell_capability,
    claims,
    limits,
    several_targets: Some(several_targets),
    tanks_several_targets: false,
    other_target_casts: None,
    self_spells: &[],
};

/// Hunter effect kinds implemented in Rust and validated against the pinned Go reference.
const EFFECTS: &[&str] = &[
    "aimed_shot",
    "arcane_shot",
    "aspect_of_the_beast",
    "aspect_of_the_hawk",
    "explosive_trap",
    "expose_prey",
    "freezing_trap",
    "immolation_trap",
    "mongoose_bite",
    "hunter_set_mana_proc",
    "rapid_recuperation",
    "renatakis_charm",
    "raptor_strike",
    "resourcefulness",
    "strider_kick",
    "wing_clip",
    "bestial_wrath",
    "frenzy",
    "hunter_pet",
    "hunter_pet_bleed",
    "hunter_pet_dust_cloud",
    "hunter_pet_furious_howl",
    "hunter_pet_scorpid_poison",
    "hunter_pet_strike",
    "hunter_pet_swipe",
    "intimidation",
    "multi_shot",
    "rapid_fire",
    "serpent_sting",
    "sniper_shot",
    "summon_hawk",
    "volley",
];

/// The effect kind whose implementation executes a Hunter spell.
fn spell_capability(spell: &Spell) -> Option<&'static str> {
    let id = spell.action_id.clone().unwrap_or_default();
    if id.item_id != 0 {
        // items.go Renataki's Charm of Beasts, which the hunter package registers itself.
        return (id.item_id == 19953 && id.tag == 0).then_some("renatakis_charm");
    }
    if id.tag != 0 {
        return (id.tag == 3 && spell.class_spell.as_deref() == Some("raptor_strike_queue"))
            .then_some("raptor_strike");
    }
    match spell.class_spell.as_deref()? {
        "aimed_shot" => Some("aimed_shot"),
        "arcane_shot" => Some("arcane_shot"),
        "sniper_shot" => Some("sniper_shot"),
        "multi_shot" => Some("multi_shot"),
        "serpent_sting" if spell.dot.is_some() => Some("serpent_sting"),
        "aspect_of_the_hawk" => Some("aspect_of_the_hawk"),
        "rapid_fire" => Some("rapid_fire"),
        "summon_hawk" => Some("summon_hawk"),
        "intimidation" => Some("intimidation"),
        "bestial_wrath" => Some("bestial_wrath"),
        "aspect_of_the_beast" => Some("aspect_of_the_beast"),
        "mongoose_bite" => Some("mongoose_bite"),
        "strider_kick" => Some("strider_kick"),
        "wing_clip" => Some("wing_clip"),
        "freezing_trap" => Some("freezing_trap"),
        "immolation_trap" if spell.dot.is_some() => Some("immolation_trap"),
        "explosive_trap" if spell.dot.is_some() => Some("explosive_trap"),
        "volley" if spell.dot.as_ref().is_some_and(|dot| dot.channeled) => Some("volley"),
        _ => None,
    }
}

/// Aura labels a Hunter effect takes responsibility for, as (unit, label).
fn claims(effect: &Effect) -> Vec<(&'static str, &str)> {
    match effect {
        Effect::AspectOfTheHawk {
            aura, proc_aura, ..
        } => {
            let mut claimed = vec![("player", aura.as_str())];
            claimed.extend(proc_aura.iter().map(|label| ("player", label.as_str())));
            claimed
        }
        Effect::RapidFire { aura, .. } => vec![("player", aura)],
        Effect::HunterPet { pet, .. } => vec![("pet unit", pet)],
        Effect::AspectOfTheBeast {
            aura, proc_aura, ..
        } => {
            let mut claimed = vec![("player", aura.as_str())];
            claimed.extend(proc_aura.iter().map(|label| ("player", label.as_str())));
            claimed
        }
        Effect::RaptorStrike { queue_aura, .. } => vec![("player", queue_aura)],
        Effect::Resourcefulness {
            trigger_aura, aura, ..
        } => vec![("player", trigger_aura), ("player", aura)],
        Effect::ExposePrey { trigger_aura, .. } => vec![("player", trigger_aura)],
        Effect::RapidRecuperation {
            trigger_aura, aura, ..
        } => vec![("player", trigger_aura), ("player", aura)],
        Effect::HunterSetManaProc { trigger_aura, .. } => vec![("player", trigger_aura)],
        Effect::Frenzy { trigger_aura, .. } => vec![("pet", trigger_aura)],
        Effect::Intimidation { aura, .. } | Effect::BestialWrath { aura, .. } => {
            vec![("pet", aura)]
        }
        Effect::HunterPetDustCloud { aura, .. } => vec![("target", aura)],
        Effect::HunterPetFuriousHowl { pet_aura, .. } => vec![("pet", pet_aura)],
        _ => Vec::new(),
    }
}

/// Shapes the Hunter runtime implements.
fn limits(prepared: &PreparedV2, _reachable: &[&Spell]) -> Vec<String> {
    let mut reasons = Vec::new();
    for effect in &prepared.effects {
        match effect {
            Effect::SerpentSting { tick_outcome, .. }
                if crate::classes::hunter::spells::serpent_sting::tick_outcome(tick_outcome)
                    .is_none() =>
            {
                reasons.push(format!("Serpent Sting ticks with {tick_outcome}"));
            }
            Effect::SummonHawk { hawk_spells, .. }
                if hawk_spells.is_empty()
                    || hawk_spells.iter().any(|&spell| {
                        prepared
                            .player
                            .spells
                            .get(spell)
                            .is_none_or(|spell| spell.dot.is_none())
                    }) =>
            {
                reasons.push("Summon Hawk names a hawk without a dot".into());
            }
            Effect::AspectOfTheHawk {
                proc_aura,
                haste_multiplier,
                proc_chance,
                ..
            } if proc_aura.is_some() != haste_multiplier.is_some()
                || proc_aura.is_some() != proc_chance.is_some() =>
            {
                reasons.push("Deadly Aspects is incomplete".into());
            }
            Effect::HunterPet {
                pet,
                rotation,
                special_ability,
                focus_dump,
                extra_ability,
                ..
            } => {
                if rotation != "cat" && rotation != "default" && rotation != "scorpid" {
                    reasons.push(format!("the pet rotation {rotation} is unsupported"));
                }
                let pet_spells = prepared
                    .pets
                    .iter()
                    .find(|exported| exported.label == *pet)
                    .map_or(&[][..], |exported| &exported.spells[..]);
                // Go's Scorpid rotation reads its special ability's dot.
                let scorpid_special = usize::try_from(*special_ability)
                    .ok()
                    .and_then(|position| pet_spells.get(position))
                    .is_some_and(|spell| {
                        let id = spell.action_id.clone().unwrap_or_default();
                        prepared.effects.iter().any(|effect| {
                            matches!(effect, Effect::HunterPetScorpidPoison { spell_id, .. } if *spell_id == id.spell_id)
                        })
                    });
                if rotation == "scorpid" && !scorpid_special {
                    reasons.push(
                        "the Scorpid rotation's special ability is not Scorpid Poison".into(),
                    );
                }
                for slot in [special_ability, focus_dump, extra_ability] {
                    let Ok(position) = usize::try_from(*slot) else {
                        continue;
                    };
                    let strike = pet_spells.get(position).is_some_and(|spell| {
                        let id = spell.action_id.clone().unwrap_or_default();
                        prepared.effects.iter().any(|effect| {
                            PetAbility::spell_id(effect) == Some(id.spell_id)
                                && PetAbility::from_effect(effect).is_some()
                                && (!matches!(
                                    effect,
                                    Effect::HunterPetBleed { .. }
                                        | Effect::HunterPetScorpidPoison { .. }
                                ) || spell.dot.is_some())
                        })
                    });
                    if !strike {
                        reasons.push(format!("the pet's ability at {position} has no behavior"));
                    }
                }
            }
            _ => {}
        }
    }
    reasons
}

/// The spells that reach a target past the first in Go and not yet in Rust: none. Multi-Shot,
/// Volley, Explosive Trap and Serpent Sting on each target run as in Go, and the exporter
/// refuses the pet abilities that cleave.
fn several_targets(_prepared: &PreparedV2, reachable: &[&Spell]) -> Vec<String> {
    crate::engine::coverage::spells_reaching_other_targets(reachable, &[])
}
