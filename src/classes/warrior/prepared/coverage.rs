//! The Warrior part of the prepared v2 build gate: Warrior effects, the effects of Warrior
//! spells and the auras Warrior effects claim. The shared gate is in `engine/coverage.rs`.

use crate::{
    contracts::prepared_v2::{Effect, PreparedV2, Spell},
    engine::coverage::ClassGate,
};

pub(crate) const GATE: ClassGate = ClassGate {
    class: "ClassWarrior",
    effects: EFFECTS,
    spell: spell_capability,
    claims,
    limits,
    several_targets: Some(crate::engine::coverage::no_limits),
    tanks_several_targets: true,
    other_target_casts: Some(crate::engine::coverage::no_limits),
    self_spells: &[],
};

/// Warrior effect kinds implemented in Rust and validated against the pinned Go reference.
const EFFECTS: &[&str] = &[
    "anger_management",
    "battle_shout",
    "battlegear_of_might_rage",
    "blood_craze",
    "berserker_rage",
    "bloodthrill",
    "bloodrage",
    "bloodthirst",
    "challenging_shout",
    "death_wish",
    "deep_wounds",
    "demoralizing_shout",
    "execute",
    "warrior_flurry",
    "hamstring",
    "pummel",
    "heroic_strike_queue",
    "intimidating_shout",
    "concussion_blow",
    "mortal_strike",
    "overpower",
    "overpower_window",
    "rage_on_avoid",
    "recklessness",
    "rend",
    "retaliation",
    "revenge",
    "shield_block",
    "shield_slam",
    "shield_wall",
    "last_stand",
    "slam",
    "spearing_strike",
    "sunder_armor",
    "sweeping_strikes",
    "taunt",
    "thunder_clap",
    "unbridled_wrath",
    "warrior_enrage",
    "improved_hamstring",
    "warrior_charge",
    "warrior_stances",
    "weaponmaster_sword",
    "whirlwind",
    "mocking_blow",
    "shield_bash",
    "victory_rush",
    "warrior_disarm",
    "warrior_intercept",
];

/// The effect kind whose implementation executes a Warrior spell.
fn spell_capability(spell: &Spell) -> Option<&'static str> {
    // Bloodrage and the strikes' queue casts carry no class mask; the shared gate names them
    // by their effects' spells.
    match spell.class_spell.as_deref()? {
        "bloodthirst" => Some("bloodthirst"),
        "charge" => Some("warrior_charge"),
        "whirlwind" | "whirlwind_off_hand" => Some("whirlwind"),
        "execute" => Some("execute"),
        "hamstring" => Some("hamstring"),
        "pummel" => Some("pummel"),
        "shield_bash" => Some("shield_bash"),
        "mocking_blow" => Some("mocking_blow"),
        "disarm" => Some("warrior_disarm"),
        "victory_rush" => Some("victory_rush"),
        "intercept" => Some("warrior_intercept"),
        "berserker_rage" => Some("berserker_rage"),
        "death_wish" => Some("death_wish"),
        "recklessness" => Some("recklessness"),
        "sunder_armor" => Some("sunder_armor"),
        "deep_wounds" => Some("deep_wounds"),
        "heroic_strike" | "cleave" => Some("heroic_strike_queue"),
        "battle_shout" => Some("battle_shout"),
        "rend" => Some("rend"),
        "overpower" => Some("overpower"),
        "mortal_strike" => Some("mortal_strike"),
        "spearing_strike" => Some("spearing_strike"),
        "slam" => Some("slam"),
        "revenge" => Some("revenge"),
        "shield_slam" => Some("shield_slam"),
        "thunder_clap" => Some("thunder_clap"),
        "demoralizing_shout" => Some("demoralizing_shout"),
        "challenging_shout" => Some("challenging_shout"),
        "taunt" => Some("taunt"),
        "intimidating_shout" => Some("intimidating_shout"),
        "concussion_blow" => Some("concussion_blow"),
        "retaliation" | "retaliation_hit" => Some("retaliation"),
        "sweeping_strikes" => Some("sweeping_strikes"),
        "battle_stance" | "berserker_stance" | "defensive_stance" => Some("warrior_stances"),
        "shield_wall" => Some("shield_wall"),
        "shield_block" => Some("shield_block"),
        "last_stand" => Some("last_stand"),
        _ => None,
    }
}

/// Aura labels a Warrior effect takes responsibility for, as (unit, label).
fn claims(effect: &Effect) -> Vec<(&'static str, &str)> {
    match effect {
        Effect::DeepWounds { trigger_aura, .. }
        | Effect::UnbridledWrath { trigger_aura, .. }
        | Effect::Bloodthrill { trigger_aura, .. }
        | Effect::WeaponmasterSword { trigger_aura, .. }
        | Effect::OverpowerWindow { trigger_aura, .. }
        | Effect::ImprovedHamstring { trigger_aura, .. }
        | Effect::BattlegearOfMightRage { trigger_aura, .. } => vec![("player", trigger_aura)],
        Effect::WarriorFlurry {
            trigger_aura, aura, ..
        }
        | Effect::Revenge {
            trigger_aura, aura, ..
        }
        | Effect::WarriorEnrage {
            trigger_aura, aura, ..
        } => vec![("player", trigger_aura), ("player", aura)],
        Effect::Retaliation { aura, .. }
        | Effect::WarriorCharge { aura, .. }
        | Effect::WarriorIntercept { aura, .. }
        | Effect::VictoryRush { aura, .. }
        | Effect::ShieldWall { aura, .. }
        | Effect::ShieldBlock { aura, .. }
        | Effect::LastStand { aura, .. }
        | Effect::SweepingStrikes { aura, .. } => {
            vec![("player", aura)]
        }
        Effect::ThunderClap { aura, .. }
        | Effect::DemoralizingShout { aura, .. }
        | Effect::WarriorDisarm { aura, .. } => {
            vec![("target", aura)]
        }
        Effect::RageOnAvoid { triggers, .. } => triggers
            .iter()
            .map(|trigger| ("player", trigger.aura.as_str()))
            .collect(),
        Effect::BloodCraze {
            damage_taken_aura, ..
        } => vec![("player", damage_taken_aura)],
        _ => Vec::new(),
    }
}

/// Spells whose Rust behavior never runs: Shield Wall, which a warrior casts only by hand,
/// and Retaliation without its effect. The gate rejects a rotation that casts them itself or
/// times their major cooldowns, a stance change without the stance categories, and the
/// warrior's own Sunder Armor that could stack without the target's armor category.
fn limits(prepared: &PreparedV2, reachable: &[&Spell]) -> Vec<String> {
    let has = |kind: &str| prepared.effects.iter().any(|effect| effect.kind() == kind);
    let default_stance = prepared.effects.iter().find_map(|effect| match effect {
        Effect::WarriorStances { default_stance, .. } => Some(default_stance.as_str()),
        _ => None,
    });
    let stance_category = prepared.effects.iter().any(|effect| {
        matches!(effect, Effect::ExclusiveCategory { category, .. } if category == "Stance")
    });
    let sunder_blocked = prepared
        .effects
        .iter()
        .any(|effect| matches!(effect, Effect::SunderArmor { blocked: true, .. }));
    // The warrior's own stacks set the target's armor through the armor category the
    // exporter resolves with the armor at each stack count.
    let sunder_category = prepared.effects.iter().any(|effect| {
        matches!(effect, Effect::ExclusiveCategory { unit, armor_by_stacks, .. }
            if unit == "target" && !armor_by_stacks.is_empty())
    });
    // The spells the rotation names itself, prepull included, and the timed major cooldowns.
    let mut cast_by_hand = Vec::new();
    if let Ok(rotation) = crate::rotation::parse(&prepared.player.rotation) {
        for prepull in &rotation.prepull {
            if let crate::rotation::Action::CastSpell { spell: id, .. } = &prepull.action {
                cast_by_hand.push(id.clone());
            }
        }
        for item in &rotation.priority_list {
            if let crate::rotation::Action::CastSpell { spell: id, .. } = item.action.scheduled() {
                cast_by_hand.push(id.clone());
            }
        }
    }
    for cooldown in &prepared.player.major_cooldowns {
        if !cooldown.timings_ns.is_empty() {
            cast_by_hand.push(cooldown.action_id.clone());
        }
    }
    let mut reasons = Vec::new();
    // Magic ticks would roll a magic hit or crit, which the bleeds' path lacks.
    for effect in &prepared.effects {
        match effect {
            Effect::Rend {
                tick_magic: true, ..
            }
            | Effect::DeepWounds {
                tick_magic: true, ..
            } => reasons.push(format!("{} ticks roll on the magic table", effect.kind())),
            _ => {}
        }
    }
    for spell in reachable {
        let id = spell.action_id.clone().unwrap_or_default();
        let Some(class_spell) = spell.class_spell.as_deref() else {
            continue;
        };
        match class_spell {
            "battle_stance" | "berserker_stance" | "defensive_stance"
                if stance_spell(class_spell) != default_stance
                    && !(stance_category && has("pseudo_stat_auras")) =>
            {
                reasons.push(format!("rotation reaches {id}, a stance change"));
            }
            "retaliation" if cast_by_hand.contains(&id) && !has("retaliation") => {
                reasons.push(format!("rotation casts {id}, which has no behavior"));
            }
            // The block value comes from the target's rolls, which only a tank's input has.
            "shield_slam" if prepared.enemy.is_none() => {
                reasons.push(format!(
                    "rotation reaches {id}, which reads the block value without a tanked target"
                ));
            }
            "sunder_armor" if !sunder_blocked && !sunder_category => {
                reasons.push(format!(
                    "rotation reaches {id}, which stacks the warrior's own Sunder Armor"
                ));
            }
            _ => {}
        }
    }
    reasons.sort();
    reasons.dedup();
    reasons
}

/// The stance a stance cast enters.
fn stance_spell(class_spell: &str) -> Option<&'static str> {
    match class_spell {
        "battle_stance" => Some("battle"),
        "berserker_stance" => Some("berserker"),
        "defensive_stance" => Some("defensive"),
        _ => None,
    }
}
