//! The Warlock part of the prepared v2 build gate: Warlock effects, the effects of Warlock
//! spells and the auras Warlock effects claim. The shared gate is in `engine/coverage.rs`.

use crate::{
    contracts::prepared_v2::{Effect, PreparedV2, Spell},
    engine::coverage::ClassGate,
};

pub(crate) const GATE: ClassGate = ClassGate {
    class: "ClassWarlock",
    effects: EFFECTS,
    spell: spell_capability,
    claims,
    limits,
    several_targets: Some(several_targets),
    tanks_several_targets: false,
    // A cast aimed at another target lands its spell, its curse and its debuffs there as in Go,
    // Demonic Brand's included: each target carries its own brand.
    other_target_casts: Some(crate::engine::coverage::no_limits),
    self_spells: &[],
};

/// Warlock effect kinds implemented in Rust and validated against the pinned Go reference.
const EFFECTS: &[&str] = &[
    "amplify_curse",
    "bane_of_agony",
    "bane_of_doom",
    "bane_of_havoc",
    "black_book",
    "conflagrate",
    "corruption",
    "curse_of_recklessness",
    "curse_of_the_elements",
    "death_coil",
    "decimation",
    "drain_life",
    "demonic_brand",
    "fel_energy",
    "firebolt",
    "hellfire",
    "immolate",
    "incinerate",
    "improved_shadow_bolt",
    "lash_of_pain",
    "life_tap",
    "nightfall",
    "rain_of_fire",
    "searing_pain",
    "shadow_and_flame",
    "shadow_bolt",
    "shadowburn",
    "siphon_life",
    "soul_fire",
    "warlock_pet",
    "wrack",
];

/// The effect kind whose implementation executes a Warlock spell.
fn spell_capability(spell: &Spell) -> Option<&'static str> {
    // items.go The Black Book, which the warlock package registers itself.
    let id = spell.action_id.clone().unwrap_or_default();
    if id.item_id == 19337 && id.tag == 0 {
        return Some("black_book");
    }
    let damage = spell.damage_effect.is_some();
    let dot = spell.dot.is_some();
    match spell.class_spell.as_deref()? {
        "shadow_bolt" if damage => Some("shadow_bolt"),
        "immolate" if damage && spell.related_dot_spell.is_some() => Some("immolate"),
        "corruption" if dot => Some("corruption"),
        "bane_of_agony" if dot => Some("bane_of_agony"),
        "bane_of_doom" if dot => Some("bane_of_doom"),
        "siphon_life" if dot => Some("siphon_life"),
        "drain_life" if dot => Some("drain_life"),
        "incinerate" if damage => Some("incinerate"),
        "wrack" if dot => Some("wrack"),
        "hellfire" if dot => Some("hellfire"),
        "rain_of_fire" => Some("rain_of_fire"),
        "death_coil" if !dot => Some("death_coil"),
        "bane_of_havoc" if damage_free(spell) => Some("bane_of_havoc"),
        "curse_of_the_elements" => Some("curse_of_the_elements"),
        "curse_of_recklessness" => Some("curse_of_recklessness"),
        "life_tap" => Some("life_tap"),
        "conflagrate" if damage => Some("conflagrate"),
        "shadowburn" if damage => Some("shadowburn"),
        "searing_pain" if damage => Some("searing_pain"),
        "soul_fire" if damage => Some("soul_fire"),
        "amplify_curse" => Some("amplify_curse"),
        "succubus_lash_of_pain" => Some("lash_of_pain"),
        "imp_firebolt" if damage_free(spell) => Some("firebolt"),
        "demonic_brand" if damage_free(spell) => Some("demonic_brand"),
        _ => None,
    }
}

/// The demon's brand hit computes its own damage; an exported client damage row would mean
/// Go changed it.
fn damage_free(spell: &Spell) -> bool {
    spell.damage_effect.is_none() && spell.dot.is_none()
}

/// Aura labels a Warlock effect takes responsibility for, as (unit, label).
fn claims(effect: &Effect) -> Vec<(&'static str, &str)> {
    match effect {
        Effect::ImprovedShadowBolt {
            trigger_aura, aura, ..
        } => vec![("player", trigger_aura), ("target", aura)],
        Effect::ShadowAndFlame {
            trigger_aura,
            shadow_aura,
            fire_aura,
            ..
        } => vec![
            ("player", trigger_aura),
            ("player", shadow_aura),
            ("player", fire_aura),
        ],
        Effect::CurseOfTheElements { aura, .. } => vec![("target", aura)],
        Effect::CurseOfRecklessness { aura, .. } => vec![("target", aura)],
        Effect::Nightfall {
            trigger_aura, aura, ..
        } => vec![("player", trigger_aura), ("player", aura)],
        Effect::WarlockPet { pet, .. } => vec![("pet unit", pet)],
        Effect::Decimation {
            trigger_aura, aura, ..
        } => vec![("player", trigger_aura), ("player", aura)],
        Effect::BaneOfHavoc {
            aura, copy_aura, ..
        } => vec![("target", aura), ("player", copy_aura)],
        Effect::BlackBook { aura, .. } => vec![("pet", aura)],
        Effect::DemonicBrand {
            trigger_aura,
            consumer_aura,
            ..
        } => {
            let mut claimed = vec![("player", trigger_aura.as_str())];
            if let Some(consumer) = consumer_aura {
                claimed.push(("pet", consumer.as_str()));
            }
            claimed
        }
        _ => Vec::new(),
    }
}

/// The demon's abilities, which its AI reaches, need a behavior too, and the brand hit reads
/// the warlock's school power as fixed.
fn limits(prepared: &PreparedV2, reachable: &[&Spell]) -> Vec<String> {
    let mut reasons = Vec::new();
    // Hellfire's burn picks Go's outcome without a hit counter when its client row cannot
    // crit, which Rust does not simulate.
    let hellfire_cannot_crit = prepared.effects.iter().any(|effect| {
        matches!(
            effect,
            Effect::Hellfire {
                tick_can_crit: false,
                ..
            }
        )
    });
    if hellfire_cannot_crit
        && reachable
            .iter()
            .any(|spell| spell.class_spell.as_deref() == Some("hellfire"))
    {
        reasons.push("Hellfire's burn cannot crit, which Rust does not simulate".into());
    }
    // Curse of Recklessness's attack power reaches a target that swings.
    for effect in &prepared.effects {
        if let Effect::CurseOfRecklessness {
            attack_power_delta, ..
        } = effect
        {
            if *attack_power_delta != 0.0 && prepared.enemy.is_some() {
                reasons.push(
                    "Curse of Recklessness raises the attack power of a target that swings".into(),
                );
            }
        }
    }
    // Fel Energy's aura starts its mana restore on gain, which no combat listener shows.
    let fel_energy = prepared.player.auras.iter().any(|aura| {
        aura.active
            && aura
                .action_id
                .as_ref()
                .is_some_and(|id| id.spell_id == 18792)
    });
    let described = prepared
        .effects
        .iter()
        .any(|effect| matches!(effect, Effect::FelEnergy { .. }));
    if fel_energy && !described {
        reasons.push("Fel Energy restores mana without an effect".into());
    }
    for effect in &prepared.effects {
        if let Effect::Decimation { execute_phase, .. } = effect {
            if *execute_phase != 35 {
                reasons.push(format!(
                    "Decimation's execute phase {execute_phase} is unsupported"
                ));
            }
        }
    }
    for effect in &prepared.effects {
        let Effect::WarlockPet {
            pet,
            autocast_spells,
            ..
        } = effect
        else {
            continue;
        };
        let Some(exported) = prepared.pets.iter().find(|exported| exported.label == *pet) else {
            reasons.push(format!("the demon {pet:?} is not simulated"));
            continue;
        };
        for &position in autocast_spells {
            let spell = exported.spells.get(position);
            let known = spell
                .and_then(spell_capability)
                .is_some_and(|kind| prepared.effects.iter().any(|effect| effect.kind() == kind));
            if !known {
                let id = spell
                    .and_then(|spell| spell.action_id.clone())
                    .unwrap_or_default();
                reasons.push(format!("the demon casts {id} without a known behavior"));
            }
        }
    }
    reasons
}

/// The spells that reach a target past the first in Go and not yet in Rust: none. Rain of
/// Fire and Hellfire hit every target as in Go, Bane of Havoc copies the damage to the other
/// targets onto the baned one, and a multidot lands a dot, its curse and bane slot, Agony
/// ramp, Soul Siphon count and Immolate reader on the target it picks, as in Go.
fn several_targets(_prepared: &PreparedV2, reachable: &[&Spell]) -> Vec<String> {
    crate::engine::coverage::spells_reaching_other_targets(reachable, &[])
}
