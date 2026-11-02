//! The Priest part of the prepared v2 build gate: Priest effects, the effects of Priest
//! spells, the auras Priest effects claim and the shape of the Shadow Weaving trigger Rust
//! implements. The shared gate is in `engine/coverage.rs`.

use crate::{
    contracts::prepared_v2::{Effect, PreparedV2, Spell},
    engine::coverage::ClassGate,
};

pub(crate) const GATE: ClassGate = ClassGate {
    class: "ClassPriest",
    effects: EFFECTS,
    spell: spell_capability,
    claims,
    limits,
    several_targets: Some(several_targets),
    tanks_several_targets: false,
    other_target_casts: Some(crate::engine::coverage::no_limits),
    self_spells: &[],
};

/// Priest effect kinds implemented in Rust and validated against the pinned Go reference.
const EFFECTS: &[&str] = &[
    "dark_inspiration",
    "dark_sacrifice",
    "devouring_plague",
    "holy_fire",
    "improved_mind_flay",
    "inner_focus",
    "mind_blast",
    "mind_flay",
    "starshards",
    "holy_nova",
    "shadowfiend",
    "penance",
    "power_in_light",
    "searing_light",
    "shadow_weaving",
    "shadow_word_death",
    "shadow_word_pain",
    "shadowform",
    "smite",
    "vampiric_embrace",
    // MythicSim's healer sims (Rust only, checked against the client rows, not Go).
    "heals",
    "priest_heal_talents",
];

/// Go registers Inner Focus (talents_discipline.go) and Dark Sacrifice (dark_sacrifice.go)
/// without a class mask, so they are known by their client spell IDs.
const INNER_FOCUS: i32 = 14751;
const DARK_SACRIFICE: std::ops::RangeInclusive<i32> = 1277324..=1277328;

/// The effect kind whose implementation executes a Priest spell.
fn spell_capability(spell: &Spell) -> Option<&'static str> {
    let Some(class) = spell.class_spell.as_deref() else {
        let id = spell.action_id.clone().unwrap_or_default();
        if id.tag != 0 || id.item_id != 0 {
            return None;
        }
        return match id.spell_id {
            INNER_FOCUS => Some("inner_focus"),
            spell_id if DARK_SACRIFICE.contains(&spell_id) && spell.dot.is_some() => {
                Some("dark_sacrifice")
            }
            _ => None,
        };
    };
    match class {
        "mind_blast" if spell.damage_effect.is_some() => Some("mind_blast"),
        "shadow_word_death" if spell.damage_effect.is_some() => Some("shadow_word_death"),
        "shadow_word_pain" if spell.dot.is_some() => Some("shadow_word_pain"),
        "devouring_plague" if spell.dot.is_some() => Some("devouring_plague"),
        "mind_flay" if spell.dot.as_ref().is_some_and(|dot| dot.channeled) => Some("mind_flay"),
        "starshards" if spell.dot.as_ref().is_some_and(|dot| dot.channeled) => Some("starshards"),
        "holy_nova" if spell.damage_effect.is_some() => Some("holy_nova"),
        "shadowfiend" => Some("shadowfiend"),
        "power_infusion" => Some("power_infusion"),
        "shadowform" => Some("shadowform"),
        "smite" if spell.damage_effect.is_some() => Some("smite"),
        // Chastise deals its roll as Smite does; the agent checks its Humanoid target.
        "chastise" if spell.damage_effect.is_some() => Some("smite"),
        "holy_fire" if spell.damage_effect.is_some() && spell.dot.is_some() => Some("holy_fire"),
        "penance" if spell.dot.as_ref().is_some_and(|dot| dot.channeled) => Some("penance"),
        "heal" => Some("heals"),
        "vampiric_embrace" => Some("vampiric_embrace"),
        _ => None,
    }
}

/// Aura labels a Priest effect takes responsibility for, as (unit, label).
fn claims(effect: &Effect) -> Vec<(&'static str, &str)> {
    match effect {
        Effect::ShadowWeaving {
            trigger_aura, aura, ..
        } => vec![("player", trigger_aura), ("target", aura)],
        Effect::VampiricEmbrace { aura, .. } => vec![("target", aura)],
        Effect::SearingLight {
            trigger_aura, aura, ..
        } => vec![("player", trigger_aura), ("player", aura)],
        Effect::Shadowfiend {
            aura,
            pet,
            mana_restore_aura,
            ..
        } => vec![
            ("player", aura),
            ("pet unit", pet),
            ("pet", mana_restore_aura),
        ],
        Effect::DarkInspiration {
            mind_blast_trigger,
            mind_flay_trigger,
            ..
        } => vec![
            ("player", mind_blast_trigger),
            ("player", mind_flay_trigger),
        ],
        Effect::Shadowform { aura, .. }
        | Effect::InnerFocus { aura, .. }
        | Effect::DarkSacrifice { aura, .. } => vec![("player", aura)],
        _ => Vec::new(),
    }
}

/// Go `OutcomeLanded`, by the names the exporter writes.
const LANDED: &[&str] = &["Hit", "Glance", "Block", "Crit", "Crush"];

/// Rust implements Shadow Weaving's trigger on landed spell hits dealt.
fn limits(prepared: &PreparedV2, _reachable: &[&Spell]) -> Vec<String> {
    let mut reasons = Vec::new();
    for effect in &prepared.effects {
        // Rust implements Searing Light's trigger on periodic damage dealt, any outcome.
        if let Effect::SearingLight {
            callbacks, outcome, ..
        } = effect
        {
            if callbacks != &["on_periodic_damage_dealt"] {
                reasons.push(format!("Searing Light listens to {callbacks:?}"));
            }
            if !outcome.is_empty() {
                reasons.push(format!("Searing Light procs on {outcome:?}"));
            }
        }
        if let Effect::ShadowWeaving {
            callbacks,
            outcome,
            trigger_spells,
            ..
        } = effect
        {
            if callbacks != &["on_spell_hit_dealt"] {
                reasons.push(format!("Shadow Weaving listens to {callbacks:?}"));
            }
            let mut names: Vec<&str> = outcome.iter().map(String::as_str).collect();
            names.sort_unstable();
            let mut landed = LANDED.to_vec();
            landed.sort_unstable();
            if names != landed {
                reasons.push(format!("Shadow Weaving procs on {outcome:?}"));
            }
            let count = prepared.player.spells.len();
            if trigger_spells.iter().any(|&spell| spell >= count) {
                reasons.push("Shadow Weaving names a spell outside the spellbook".into());
            }
        }
    }
    reasons
}

/// The spells that reach a target past the first in Go and not yet in Rust: none. Holy Nova
/// hits each target in unit index order, as Go's area helper does.
fn several_targets(_prepared: &PreparedV2, reachable: &[&Spell]) -> Vec<String> {
    crate::engine::coverage::spells_reaching_other_targets(reachable, &[])
}
