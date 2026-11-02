//! The Druid part of the prepared v2 build gate: Druid effects, the effects of Druid spells,
//! the auras Druid effects claim and the forms the supported builds use. The shared gate is
//! in `engine/coverage.rs`.

use crate::{
    contracts::prepared_v2::{Effect, PreparedV2, Spell},
    engine::coverage::ClassGate,
};

pub(crate) const GATE: ClassGate = ClassGate {
    class: "ClassDruid",
    effects: EFFECTS,
    spell: spell_capability,
    claims,
    limits,
    several_targets: Some(several_targets),
    tanks_several_targets: true,
    other_target_casts: Some(other_target_casts),
    self_spells: &[],
};

/// Druid effect kinds implemented in Rust and validated against the pinned Go reference.
const EFFECTS: &[&str] = &[
    "barkskin",
    "bear_form",
    "berserk",
    "blood_frenzy",
    "cat_builders",
    "cat_form",
    "demoralizing_roar",
    "druid_forms",
    "druid_thorns",
    "eclipse",
    "enrage",
    "faerie_fire",
    "ferocious_bite",
    "frenzied_regeneration",
    "hurricane",
    "innervate",
    "insect_swarm",
    "lacerate",
    "maul",
    "moonfire",
    "moonkin_form",
    "natural_reaction",
    "natures_bounty",
    "natures_grace",
    "omen_of_clarity",
    "primal_bite",
    "prowl",
    "rake",
    "rend_and_tear",
    "rip",
    "shifting_power",
    "starfire",
    "swipe",
    "unending_life_refund",
    "wrath",
    // MythicSim's healer sims (Rust only, checked against the client rows, not Go).
    "heals",
    "bloom",
];

/// The effect kind whose implementation executes a Druid spell.
fn spell_capability(spell: &Spell) -> Option<&'static str> {
    match spell.class_spell.as_deref()? {
        "heal" | "swiftmend" => Some("heals"),
        "bloom" => Some("bloom"),
        "moonkin_form" => Some("moonkin_form"),
        "starfire" if spell.damage_effect.is_some() => Some("starfire"),
        "wrath" if spell.damage_effect.is_some() => Some("wrath"),
        "moonfire" if spell.damage_effect.is_some() => Some("moonfire"),
        "insect_swarm" if spell.dot.is_some() => Some("insect_swarm"),
        "innervate" => Some("innervate"),
        "cat_form" => Some("cat_form"),
        "bear_form" => Some("bear_form"),
        "shred" | "claw" | "ravage" => Some("cat_builders"),
        "rip" if spell.dot.is_some() => Some("rip"),
        "rake" if spell.dot.is_some() => Some("rake"),
        "ferocious_bite" if spell.damage_effect.is_some() => Some("ferocious_bite"),
        "shifting_power" => Some("shifting_power"),
        "faerie_fire" => Some("faerie_fire"),
        "enrage" => Some("enrage"),
        "demoralizing_roar" => Some("demoralizing_roar"),
        "maul" => Some("maul"),
        "lacerate" if spell.dot.is_some() => Some("lacerate"),
        "primal_bite" => Some("primal_bite"),
        "swipe" => Some("swipe"),
        "hurricane" => Some("hurricane"),
        "frenzied_regeneration" => Some("frenzied_regeneration"),
        "thorns" => Some("druid_thorns"),
        _ => None,
    }
}

/// Aura labels a Druid effect takes responsibility for, as (unit, label).
fn claims(effect: &Effect) -> Vec<(&'static str, &str)> {
    match effect {
        Effect::OmenOfClarity {
            trigger_aura, aura, ..
        }
        | Effect::NaturesGrace {
            trigger_aura, aura, ..
        }
        | Effect::Eclipse {
            trigger_aura, aura, ..
        } => vec![("player", trigger_aura), ("player", aura)],
        Effect::Innervate { aura, .. }
        | Effect::MoonkinForm { aura, .. }
        | Effect::CatForm { aura, .. }
        | Effect::Prowl { aura, .. }
        | Effect::Berserk { aura, .. }
        | Effect::BearForm { aura, .. }
        | Effect::Enrage { aura, .. }
        | Effect::Barkskin { aura, .. }
        | Effect::DruidThorns { aura, .. }
        | Effect::FrenziedRegeneration { aura, .. } => vec![("player", aura)],
        Effect::NaturalReaction { trigger_aura, .. } => vec![("player", trigger_aura)],
        Effect::NaturesBounty { aura, .. } | Effect::UnendingLifeRefund { aura, .. } => {
            vec![("player", aura)]
        }
        Effect::Maul { queue_aura, .. } => vec![("player", queue_aura)],
        Effect::DemoralizingRoar { aura, .. } => vec![("target", aura)],
        Effect::BloodFrenzy {
            trigger_aura,
            bear_trigger_aura,
            ..
        } => vec![("player", trigger_aura), ("player", bear_trigger_aura)],
        _ => Vec::new(),
    }
}

/// The supported druids start in Moonkin Form and keep it, or start in Cat Form or Bear Form
/// and leave it only for caster form, whose spells and form-breaking consumables clear it. A
/// spell of another form only fails its cast check. The cat's builders and Blood Frenzy read only what Rust models.
fn limits(prepared: &PreparedV2, reachable: &[&Spell]) -> Vec<String> {
    let mut reasons = Vec::new();
    let spells = &prepared.player.spells;
    let cat = prepared
        .effects
        .iter()
        .any(|effect| matches!(effect, Effect::CatForm { .. }));
    let bear = prepared
        .effects
        .iter()
        .any(|effect| matches!(effect, Effect::BearForm { .. }));
    // Spells a rotation action names itself, as opposed to through the cooldown autocast.
    let named: Vec<crate::contracts::prepared_v2::ActionId> =
        crate::rotation::parse(&prepared.player.rotation)
            .map(|rotation| {
                rotation
                    .priority_list
                    .iter()
                    .flat_map(|item| {
                        item.action
                            .spells()
                            .into_iter()
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .chain(rotation.prepull.iter().flat_map(|item| {
                        item.action
                            .spells()
                            .into_iter()
                            .cloned()
                            .collect::<Vec<_>>()
                    }))
                    .collect()
            })
            .unwrap_or_default();
    for effect in &prepared.effects {
        match effect {
            Effect::DruidForms {
                starting_form,
                spells: forms,
            } => {
                // A spell outside the druid's form fails its cast check with a log, unless it
                // allows humanoid form, when casting it leaves the form. The cat and the bear
                // may leave and return; Moonkin Form keeps its form for the fight.
                let (current, leaves): (&str, bool) = match starting_form.as_slice() {
                    [form] if form == "moonkin" => ("moonkin", false),
                    // A Restoration druid heals in caster form and never shifts.
                    [form] if form == "humanoid" => ("humanoid", false),
                    [form] if form == "cat" && cat => ("cat", true),
                    [form] if form == "bear" && bear => ("bear", true),
                    _ => {
                        reasons.push(format!(
                            "druid starting form {starting_form:?} is unsupported"
                        ));
                        continue;
                    }
                };
                for spell in reachable {
                    let Some(index) = spells.iter().position(|s| std::ptr::eq(s, *spell)) else {
                        continue;
                    };
                    // Go leaves Innervate to the rotation: the autocast only checks it.
                    let autocast_only = spell.class_spell.as_deref() == Some("innervate")
                        && !named.iter().any(|id| spell.action_id.as_ref() == Some(id));
                    if bear && autocast_only {
                        continue;
                    }
                    if let Some(entry) = forms.iter().find(|entry| entry.spell == index) {
                        let castable = entry.forms.iter().any(|form| form == current);
                        let clears = entry.forms.iter().any(|form| form == "humanoid");
                        if !castable && clears && !leaves {
                            let id = spell.action_id.clone().unwrap_or_default();
                            reasons.push(format!(
                                "rotation reaches {id}, which the supported forms cannot cast"
                            ));
                        }
                    }
                }
            }
            Effect::OmenOfClarity {
                callbacks,
                outcome,
                require_damage_dealt,
                ..
            } => {
                if callbacks
                    .iter()
                    .any(|callback| callback != "on_spell_hit_dealt" && callback != "on_heal_dealt")
                {
                    reasons.push(format!("Omen of Clarity listens to {callbacks:?}"));
                }
                if !(outcome.iter().any(|o| o == "Hit") && outcome.iter().any(|o| o == "Crit")) {
                    reasons.push(format!("Omen of Clarity procs on {outcome:?}"));
                }
                if *require_damage_dealt {
                    reasons.push("Omen of Clarity requires damage dealt".into());
                }
            }
            Effect::BloodFrenzy { outcome, .. } if outcome != &["Crit"] => {
                reasons.push(format!("Blood Frenzy procs on {outcome:?}"));
            }
            Effect::Rip {
                tick_magic: true, ..
            } => {
                reasons.push("Rip ticks on the magic table".into());
            }
            Effect::Rake {
                tick_magic: true, ..
            } => {
                reasons.push("Rake ticks on the magic table".into());
            }
            Effect::Lacerate {
                tick_magic: true, ..
            } => {
                reasons.push("Lacerate ticks on the magic table".into());
            }
            Effect::NaturalReaction { outcome, .. } if outcome != &["Dodge"] => {
                reasons.push(format!("Natural Reaction procs on {outcome:?}"));
            }
            Effect::FaerieFire { refresh, .. }
                if refresh.as_slice() != ["own"] && refresh.as_slice() != ["never"] =>
            {
                reasons.push(format!(
                    "Faerie Fire's armor reduction reads {refresh:?}, which is not modeled"
                ));
            }
            _ => {}
        }
    }
    reasons
}

/// The spells that reach a target past the first in Go and not yet in Rust: none. Swipe on up
/// to three targets, Hurricane's ticks on every target, Demoralizing Roar's debuff on each and
/// a Berserk Primal Bite on up to three run as in Go. A tank has every copy of the boss
/// swing at it, which the runtime follows.
fn several_targets(_prepared: &PreparedV2, reachable: &[&Spell]) -> Vec<String> {
    crate::engine::coverage::spells_reaching_other_targets(reachable, &[])
}

/// A cast aimed at another target lands its spell, its dot and its debuff there as in Go,
/// except Faerie Fire, whose debuff and armor change the runtime keeps on the first target.
fn other_target_casts(_prepared: &PreparedV2, spells: &[&Spell]) -> Vec<String> {
    spells
        .iter()
        .filter(|spell| spell.class_spell.as_deref() == Some("faerie_fire"))
        .map(|spell| {
            format!(
                "{} at a target past the first lands on the first target only",
                spell.action_id.clone().unwrap_or_default()
            )
        })
        .collect()
}
