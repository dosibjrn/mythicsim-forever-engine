//! The prepared v2 build gate, shared by every class.
//!
//! An effect is required when an active aura that listens to combat events belongs to
//! it, or when the rotation can reach a spell it implements. A prepared input is
//! supported only when every required effect has a Rust implementation, every active
//! listener is claimed and every reachable spell stays inside the runtime's limits.
//! Each gap becomes one stable reason string. A class supplies its own effects, the
//! effects of its spells and the auras its effects claim through a [`ClassGate`].

use std::{collections::BTreeSet, sync::OnceLock};

use crate::{
    classes,
    contracts::prepared_v2::{
        ActionId, Effect, MajorCooldown, PreparedV2, ProcPeriodic, Spell, SwapAuraState,
        SwapCallback, SwapChange,
    },
    core::fight::{proc_damage::ProcDamage, DIRECT_PROC_MASKS},
    rotation::{compile_condition, Action, FoundAura, Lookup, Rotation, Unit, Value},
};

/// A class's part of the gate.
pub(crate) struct ClassGate {
    /// Go's `Class` enum name, as the exporter writes it.
    pub(crate) class: &'static str,
    /// The class's effect kinds implemented in Rust and validated against Go.
    pub(crate) effects: &'static [&'static str],
    /// The effect kind whose implementation executes a class spell, by its class spell name
    /// and shape.
    pub(crate) spell: fn(&Spell) -> Option<&'static str>,
    /// The aura labels a class effect takes responsibility for, as (unit, label).
    pub(crate) claims: for<'a> fn(&'a Effect) -> Vec<(&'static str, &'a str)>,
    /// Class limits on the input and the spells the rotation can reach.
    pub(crate) limits: Limits,
    /// The class's limits in a fight against several targets: the reachable spells that reach
    /// a target past the first in Go and not yet in Rust. None for a class not yet checked
    /// against several targets.
    pub(crate) several_targets: Option<Limits>,
    /// Whether the class's listeners of the hits its player takes answer the copy of the boss
    /// that swung, so a tank of the class can face several targets, each swinging at it.
    pub(crate) tanks_several_targets: bool,
    /// The class's limits on a rotation cast aimed at a target past the first: the spells it
    /// casts there, whose effects and debuffs must land on that target as in Go. None for a
    /// class whose spells have not been checked against it.
    pub(crate) other_target_casts: Option<Limits>,
    /// The class spells (`class_spell`) that act on the caster alone, whatever unit the rotation
    /// names: Go casts them at the player as at the target, and the runtime does the same.
    pub(crate) self_spells: &'static [&'static str],
}

/// A class check of the input and the spells the rotation can reach: a reason for each
/// thing it does not support.
pub(crate) type Limits = fn(&PreparedV2, &[&Spell]) -> Vec<String>;

/// Go `ItemSwap.IsEnabled`: whether the prepared input swaps items.
fn has_item_swap(prepared: &PreparedV2) -> bool {
    crate::core::fight::item_swap::item_swap_effect(prepared).is_some()
}

/// The player's resource bars as the runtime builds them from the prepared input.
fn rotation_bars(prepared: &PreparedV2) -> crate::rotation::Bars {
    crate::rotation::Bars {
        mana: prepared.player.mana.max > 0.0,
        max_rage: prepared.effects.iter().find_map(|effect| match effect {
            Effect::RageBar { max_rage, .. } => Some(max_rage.max(100.0)),
            _ => None,
        }),
        max_combo_points: prepared
            .player
            .energy
            .as_ref()
            .map(|energy| energy.max_combo_points),
    }
}

/// A [`Limits`] check that finds nothing to refuse.
pub(crate) fn no_limits(_: &PreparedV2, _: &[&Spell]) -> Vec<String> {
    Vec::new()
}

/// Why the engine cannot simulate a valid input: a short code that stays the same however
/// the input varies, which a worker counts fallbacks by, and the text that explains this
/// input. [`REFUSAL_CODES`] lists every code.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Refusal {
    pub code: &'static str,
    pub reason: String,
}

impl Refusal {
    pub(crate) fn new(code: &'static str, reason: impl Into<String>) -> Self {
        debug_assert!(
            REFUSAL_CODES.iter().any(|(known, _)| *known == code),
            "refusal code {code} is not in REFUSAL_CODES"
        );
        Refusal {
            code,
            reason: reason.into(),
        }
    }
}

/// Every refusal code, each with what it covers. docs/prepared-v2.md lists them too.
pub const REFUSAL_CODES: &[(&str, &str)] = &[
    (
        "exporter_unrepresented",
        "the exporter could not describe part of the request",
    ),
    (
        "rotation_unsupported",
        "the rotation uses an action, value or field the parser rejects",
    ),
    (
        "item_swap_unsupported",
        "an item swap changes something Rust does not follow when the weapon changes",
    ),
    ("class_unsupported", "the player's class has no Rust gate"),
    (
        "level_unsupported",
        "the player is not level 60 or the target not level 60 to 63",
    ),
    (
        "target_count_invalid",
        "the target count is neither one nor 2 to 5",
    ),
    (
        "several_targets_unsupported",
        "something reaches a target past the first that Rust does not simulate there",
    ),
    (
        "aura_listener_unclaimed",
        "an aura listens to combat events with no effect that handles it",
    ),
    (
        "pet_unsupported",
        "a pet has no behavior or inherits a stat change Rust does not follow",
    ),
    (
        "tanking_unsupported",
        "the target swings at the player in a way Rust does not simulate",
    ),
    (
        "aura_condition_unsupported",
        "a rotation condition reads an aura as Rust does not",
    ),
    (
        "resource_unsupported",
        "the rotation or a spell reads a resource the player lacks",
    ),
    (
        "prepull_unsupported",
        "a prepull action Rust cannot reproduce",
    ),
    (
        "movement_unsupported",
        "a move of the player in the rotation that Rust cannot simulate: no movement speed exported, or a value Go would panic on",
    ),
    (
        "cooldown_unsupported",
        "a survival cooldown fires at a health threshold Rust does not simulate",
    ),
    (
        "class_limit",
        "a class gate rejects the input or a spell the rotation reaches",
    ),
    (
        "proc_unsupported",
        "a proc listens to hits Rust does not deliver to it",
    ),
    (
        "stat_change_unsupported",
        "an aura changes a stat the runtime holds fixed",
    ),
    (
        "unknown_spell",
        "the rotation reaches a spell without a known behavior",
    ),
    (
        "spell_unsupported",
        "a reachable spell uses a feature the runtime does not implement",
    ),
    (
        "effect_unimplemented",
        "an effect the input needs is not implemented",
    ),
];

/// Each reason of one kind, under its code.
fn coded(code: &'static str, reasons: Vec<String>) -> impl Iterator<Item = Refusal> {
    reasons
        .into_iter()
        .map(move |reason| Refusal::new(code, reason))
}

/// Effects of races, items and raid buffs, which every class shares.
const COMMON_EFFECTS: &[&str] = &[
    "aura_should_refresh",
    "basic_explosive",
    "berserking",
    "blood_fury",
    "chance_of_death",
    "conjured_energy",
    "conjured_mana",
    "crusader",
    "diamond_flask",
    "dragonbreath_chili",
    "dragonfire_potion",
    "revelation",
    "caustic_smog_potion",
    "elemental_purging_potion",
    "energize_on_use",
    "emerald_dragon_whelp",
    "thunderfury",
    "force_reactive_disk",
    "stacking_cd_on_use",
    "stacking_on_use",
    "sulfuras_hand_of_ragnaros",
    "blade_of_hanna",
    "energize_proc",
    "eureka",
    "exclusive_category",
    "extra_attack_proc",
    "external_cooldown",
    "fixed_uptime_aura",
    "goblin_sapper",
    "inert_listener",
    "instant_cast_aura",
    "inert_pet",
    "innervate_regen",
    "item_swap",
    "judgement_of_light",
    "judgement_of_wisdom",
    "health_on_cast",
    "mana_on_cast",
    "mana_proc",
    "parry_haste",
    "player_movement",
    "potion_mana",
    "player_damage_taken",
    "potion_resource",
    "power_infusion",
    "pseudo_stat_auras",
    "pushback_trigger",
    "rage_bar",
    "redoubt",
    "shield_specialization",
    "reckoning",
    "holy_shield",
    "read_ley_line",
    "skysight",
    "shatter_curse",
    "speed_on_use",
    "spell_cost_aura_on_use",
    "spell_mod_aura_on_use",
    "spell_data_damage_proc",
    "spell_data_stat_proc",
    "spell_data_heal_proc",
    "second_wind",
    "shield_on_heal",
    "absorb_on_use",
    "spell_data_absorb_proc",
    "heal_on_use",
    "stat_proc",
    "stat_auras",
    "health_rage_proc",
    "energy_proc",
    "battle_squawk",
    "speed_aura",
    "mob_type_bonus",
    "health_regen_aura",
    "oil_of_immolation",
    "armor_debuff_proc",
    "damage_on_use",
    "stoneform",
    "sunder_armor_ramp",
    "external_expose_armor",
    "temporary_stats",
    "touch_of_the_grave",
    "windfury_totem",
];

/// Every class with an implemented gate.
fn gates() -> [&'static ClassGate; 9] {
    [
        &classes::mage::prepared::GATE,
        &classes::druid::prepared::GATE,
        &classes::shaman::prepared::GATE,
        &classes::paladin::prepared::GATE,
        &classes::warlock::prepared::GATE,
        &classes::priest::prepared::GATE,
        &classes::rogue::prepared::GATE,
        &classes::warrior::prepared::GATE,
        &classes::hunter::prepared::GATE,
    ]
}

/// Every effect kind implemented in Rust, sorted.
pub(crate) fn implemented_effects() -> &'static [&'static str] {
    static EFFECTS: OnceLock<Vec<&'static str>> = OnceLock::new();
    EFFECTS.get_or_init(|| {
        let mut effects: BTreeSet<&'static str> = COMMON_EFFECTS.iter().copied().collect();
        for gate in gates() {
            effects.extend(gate.effects.iter().copied());
        }
        effects.into_iter().collect()
    })
}

/// The gate of the prepared player's class, if Rust implements it.
pub(crate) fn class_gate(class: &str) -> Option<&'static ClassGate> {
    gates().into_iter().find(|gate| gate.class == class)
}

/// The effect kind of a spell that is not a class spell: items and racials.
fn common_spell_capability(spell: &Spell, prepared: &PreparedV2) -> Option<&'static str> {
    let id = spell.action_id.clone().unwrap_or_default();
    let item = id.item_id;
    prepared.effects.iter().find_map(|effect| match effect {
        Effect::PotionMana { item_id, .. } if *item_id == item => Some("potion_mana"),
        Effect::PotionResource { item_id, .. } if *item_id == item => Some("potion_resource"),
        Effect::DragonfirePotion { item_id, .. } if *item_id == item => Some("dragonfire_potion"),
        Effect::CausticSmogPotion { item_id, .. } if *item_id == item => {
            Some("caustic_smog_potion")
        }
        Effect::ElementalPurgingPotion { item_id, .. } if *item_id == item => {
            Some("elemental_purging_potion")
        }
        // Class spells Go registers without a class mask, named by their effect's spell.
        Effect::Bloodrage { spell_id, .. } if *spell_id == id.spell_id && id.tag == 0 => {
            Some("bloodrage")
        }
        Effect::HeroicStrikeQueue { strikes, .. }
            if id.tag == 1 && strikes.iter().any(|strike| strike.spell_id == id.spell_id) =>
        {
            Some("heroic_strike_queue")
        }
        Effect::ConjuredMana { item_id, .. } if *item_id == item => Some("conjured_mana"),
        Effect::ConjuredEnergy { item_id, .. } if *item_id == item => Some("conjured_energy"),
        Effect::GoblinSapper { item_id, .. } if *item_id == item && id.tag == 0 => {
            Some("goblin_sapper")
        }
        Effect::BasicExplosive { item_id, .. } if *item_id == item && id.tag == 0 => {
            Some("basic_explosive")
        }
        Effect::EnergizeOnUse { item_id, .. } if *item_id == item => Some("energize_on_use"),
        Effect::SecondWind { item_id, .. } if *item_id == item && id.tag == 0 => {
            Some("second_wind")
        }
        Effect::AbsorbOnUse { item_id, .. } if *item_id == item && id.tag == 0 => {
            Some("absorb_on_use")
        }
        Effect::HealOnUse { item_id, .. } if *item_id == item && id.tag == 0 => Some("heal_on_use"),
        Effect::SpeedOnUse { item_id, .. } if *item_id == item && id.tag == 0 => {
            Some("speed_on_use")
        }
        Effect::DiamondFlask { item_id, .. } if *item_id == item && id.tag == 0 => {
            Some("diamond_flask")
        }
        Effect::SpellCostAuraOnUse { item_id, .. } if *item_id == item && id.tag == 0 => {
            Some("spell_cost_aura_on_use")
        }
        Effect::SpellModAuraOnUse { item_id, .. } if *item_id == item && id.tag == 0 => {
            Some("spell_mod_aura_on_use")
        }
        Effect::StackingCdOnUse { item_id, .. } if *item_id == item && id.tag == 0 => {
            Some("stacking_cd_on_use")
        }
        Effect::ShieldOnHeal { item_id, .. } if *item_id == item && id.tag == 0 => {
            Some("shield_on_heal")
        }
        // A damage on-use item whose outcome appliers the runtime knows.
        Effect::DamageOnUse {
            item_id,
            direct,
            periodic,
            ..
        } if *item_id == item
            && id.tag == 0
            && crate::core::fight::on_use_damage_known(direct, periodic) =>
        {
            Some("damage_on_use")
        }
        Effect::Eureka { spell_id, .. } if *spell_id == id.spell_id && id.tag == 0 => {
            Some("eureka")
        }
        Effect::InstantCastAura { spell_id, .. }
            if *spell_id == id.spell_id && id.item_id == 0 && id.tag == 0 =>
        {
            Some("instant_cast_aura")
        }
        Effect::Berserking { spell_id, .. } if *spell_id == id.spell_id && id.tag == 0 => {
            Some("berserking")
        }
        Effect::BloodFury { spell_id, .. } if *spell_id == id.spell_id && id.tag == 0 => {
            Some("blood_fury")
        }
        // Spells Go registers for the buffs other players cast on the player on cooldown.
        Effect::ExternalCooldown {
            spell_id,
            spell_tag,
            ..
        } if *spell_id == id.spell_id && *spell_tag == id.tag && id.item_id == 0 => {
            Some("external_cooldown")
        }
        Effect::ShatterCurse { spell_id, .. } if *spell_id == id.spell_id && id.tag == 0 => {
            Some("shatter_curse")
        }
        Effect::Stoneform { spell_id, .. } if *spell_id == id.spell_id && id.tag == 0 => {
            Some("stoneform")
        }
        Effect::ReadLeyLine { spell_id, .. } if *spell_id == id.spell_id && id.tag == 0 => {
            Some("read_ley_line")
        }
        Effect::Skysight { spell_id, .. } if *spell_id == id.spell_id && id.tag == 0 => {
            Some("skysight")
        }
        Effect::TemporaryStats {
            spell_id, item_id, ..
        } if *spell_id == id.spell_id && *item_id == id.item_id && id.tag == 0 => {
            Some("temporary_stats")
        }
        Effect::StackingOnUse { spell_id, .. }
            if *spell_id == id.spell_id && id.item_id == 0 && id.tag == 0 =>
        {
            Some("stacking_on_use")
        }
        // Class spells Go registers without a class mask, named by their effect.
        Effect::Prowl { spell_id, .. } if *spell_id == id.spell_id && id.tag == 0 => Some("prowl"),
        Effect::Berserk { spell_id, .. } if *spell_id == id.spell_id && id.tag == 0 => {
            Some("berserk")
        }
        Effect::Barkskin { spell_id, .. } if *spell_id == id.spell_id && id.tag == 0 => {
            Some("barkskin")
        }
        Effect::Maul { queue_spell, .. }
            if prepared
                .player
                .spells
                .get(*queue_spell)
                .is_some_and(|queue| std::ptr::eq(queue, spell)) =>
        {
            Some("maul")
        }
        _ => None,
    })
}

/// The effect kind whose implementation executes a castable spell.
fn spell_capability(
    spell: &Spell,
    prepared: &PreparedV2,
    gate: &ClassGate,
) -> Option<&'static str> {
    // A class may also implement spells Go registers without a class mask.
    if let Some(kind) = (gate.spell)(spell) {
        return Some(kind);
    }
    if spell.class_spell.is_some() {
        return None;
    }
    common_spell_capability(spell, prepared)
}

/// Aura labels a race, item or raid buff effect takes responsibility for.
fn common_claims(effect: &Effect) -> Vec<(&'static str, &str)> {
    match effect {
        Effect::TouchOfTheGrave { trigger_aura, .. } => vec![("player", trigger_aura)],
        Effect::Eureka { aura, .. }
        | Effect::Berserking { aura, .. }
        | Effect::BloodFury { aura, .. }
        | Effect::ShatterCurse { aura, .. }
        | Effect::Stoneform { aura, .. }
        | Effect::ReadLeyLine { aura, .. }
        | Effect::Skysight { aura, .. }
        | Effect::TemporaryStats { aura, .. }
        | Effect::DiamondFlask { aura, .. }
        | Effect::AbsorbOnUse { aura, .. }
        | Effect::SpeedOnUse { aura, .. }
        | Effect::PowerInfusion { aura, .. }
        | Effect::InnervateRegen { aura, .. }
        | Effect::ExternalCooldown { aura, .. } => vec![("player", aura)],
        Effect::JudgementOfWisdom { aura, .. } | Effect::JudgementOfLight { aura, .. } => {
            vec![("target", aura)]
        }
        Effect::HealthOnCast { trigger_aura, .. } => vec![("player", trigger_aura)],
        Effect::RageBar { aura, .. } => vec![("player", aura)],
        Effect::ExtraAttackProc { trigger_aura, .. } => vec![("player", trigger_aura)],
        Effect::ChanceOfDeath { aura } => vec![("player", aura)],
        Effect::ParryHaste { unit, aura, .. } => match unit.as_str() {
            "player" => vec![("player", aura)],
            "target" => vec![("target", aura)],
            _ => Vec::new(),
        },
        Effect::PushbackTrigger { aura, .. } => vec![("player", aura)],
        Effect::EnergizeProc { trigger_aura, .. } | Effect::ManaProc { trigger_aura, .. } => {
            vec![("player", trigger_aura)]
        }
        Effect::Crusader { trigger_aura, .. }
        | Effect::DragonbreathChili { trigger_aura, .. }
        | Effect::SpellDataDamageProc { trigger_aura, .. }
        | Effect::SpellDataHealProc { trigger_aura, .. } => vec![("player", trigger_aura)],
        Effect::EmeraldDragonWhelp {
            trigger_aura, pet, ..
        } => vec![("player", trigger_aura), ("pet unit", pet)],
        Effect::ForceReactiveDisk { trigger_aura, .. } => vec![("player", trigger_aura)],
        Effect::Revelation {
            trigger_aura,
            ticks_trigger_aura,
            aura,
            ..
        } => vec![
            ("player", trigger_aura),
            ("player", ticks_trigger_aura),
            ("player", aura),
        ],
        Effect::StackingOnUse { aura, .. } => vec![("player", aura)],
        Effect::Thunderfury {
            trigger_aura,
            slow_aura,
            resistance_aura,
            ..
        } => vec![
            ("player", trigger_aura),
            ("target", slow_aura),
            ("target", resistance_aura),
        ],
        Effect::SulfurasHandOfRagnaros {
            trigger_aura,
            immolation_aura,
            ..
        } => vec![("player", trigger_aura), ("player", immolation_aura)],
        Effect::BladeOfHanna {
            trigger_aura,
            stacks_aura,
            ..
        } => vec![("player", trigger_aura), ("player", stacks_aura)],
        Effect::StatProc {
            trigger_aura, aura, ..
        }
        | Effect::SpellDataStatProc {
            trigger_aura, aura, ..
        } => vec![("player", trigger_aura), ("player", aura)],
        Effect::HealthRageProc { trigger_aura, .. } | Effect::EnergyProc { trigger_aura, .. } => {
            vec![("player", trigger_aura)]
        }
        Effect::BattleSquawk { aura, .. }
        | Effect::HealthRegenAura { aura, .. }
        | Effect::OilOfImmolation { aura, .. }
        | Effect::SpeedAura { aura, .. }
        | Effect::MobTypeBonus { aura, .. } => {
            vec![("player", aura)]
        }
        Effect::SpellDataAbsorbProc {
            trigger_aura, aura, ..
        } => vec![("player", trigger_aura), ("player", aura)],
        Effect::ArmorDebuffProc {
            trigger_aura, aura, ..
        } => vec![("player", trigger_aura), ("target", aura)],
        Effect::ExternalExposeArmor { aura, .. } => vec![("target", aura)],
        Effect::SpellCostAuraOnUse { aura, .. } | Effect::SpellModAuraOnUse { aura, .. } => {
            vec![("player", aura)]
        }
        Effect::ManaOnCast { trigger_aura, .. } => vec![("player", trigger_aura)],
        Effect::StackingCdOnUse {
            stat_aura,
            window_aura,
            ..
        } => vec![("player", window_aura), ("player", stat_aura)],
        Effect::ShieldOnHeal { aura, .. } => vec![("player", aura)],
        Effect::InstantCastAura { aura, .. } => {
            vec![("player", aura)]
        }
        Effect::WindfuryTotem {
            trigger_aura,
            proc_aura,
            ..
        } => vec![("player", trigger_aura), ("player", proc_aura)],
        Effect::InertListener { unit, aura, .. } => match unit.as_str() {
            "player" => vec![("player", aura)],
            "target" => vec![("target", aura)],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Proc triggers Rust matches as `ProcMaskDirect`; any other mask is unsupported. So is a
/// damage proc that names an attack table the runtime does not roll for its hit.
fn undirected_procs(prepared: &PreparedV2) -> Vec<String> {
    prepared
        .effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::TouchOfTheGrave {
                trigger_aura,
                proc_mask,
                ..
            } => {
                let mut mask: Vec<&str> = proc_mask.iter().map(String::as_str).collect();
                let mut direct = DIRECT_PROC_MASKS.to_vec();
                mask.sort_unstable();
                direct.sort_unstable();
                (mask != direct)
                    .then(|| format!("{trigger_aura} procs from {proc_mask:?}, not direct hits"))
            }
            // A damage proc names the attack table its hit rolls only for a Go literal range
            // of the melee defense type, and the table must agree with the spell's crit.
            Effect::SpellDataDamageProc {
                trigger_aura,
                spell,
                average,
                variance,
                roll: None,
                can_crit,
                outcome,
                chain,
                area,
                periodic,
                ..
            } if outcome.is_some() || chain.is_some() || area.is_some() || periodic.is_some() => {
                let problem = ProcDamage::new(
                    *average,
                    *variance,
                    *can_crit,
                    outcome.as_deref(),
                    chain.as_ref(),
                    area.as_ref(),
                    periodic.as_ref(),
                )
                .err()
                .or_else(|| {
                    let spell = prepared.player.spells.get(*spell)?;
                    proc_damage_mismatch(spell, outcome.as_deref(), *can_crit, periodic.as_ref())
                });
                problem.map(|problem| format!("{trigger_aura}'s hit is not rolled: {problem}"))
            }
            Effect::SpellDataDamageProc {
                trigger_aura,
                spell,
                roll,
                can_crit,
                outcome: Some(outcome),
                ..
            } => {
                let melee_spell = prepared
                    .player
                    .spells
                    .get(*spell)
                    .is_some_and(|spell| spell.defense_type == "DefenseTypeMelee");
                let table =
                    crate::core::fight::melee::PhysicalOutcome::of_melee_proc(outcome).is_some();
                let crit = outcome != "melee_special_hit";
                (roll.is_none() || !table || !melee_spell || crit != *can_crit).then(|| {
                    format!(
                        "{trigger_aura}'s hit rolls the {outcome} table, which the runtime \
                         rolls only for a melee spell's literal range"
                    )
                })
            }
            _ => None,
        })
        .collect()
}

/// Runeblade of Baron Rivendare's Unholy Aura starts a periodic action as it gains, which gives
/// the wearer 60 health every five seconds; the runtime runs no such action.
fn unholy_aura_regeneration(prepared: &PreparedV2) -> Vec<String> {
    let regenerates = prepared.effects.iter().any(
        |effect| matches!(effect, Effect::HealthRegenAura { aura, .. } if aura == "Unholy Aura"),
    );
    prepared
        .player
        .auras
        .iter()
        .filter(|aura| aura.label == "Unholy Aura" && !regenerates)
        .map(|_| {
            "player aura \"Unholy Aura\" regenerates health every five seconds, which the \
             runtime does not run"
                .to_string()
        })
        .collect()
}

/// Dots of two spells that share one aura on the target. Go registers an aura once per label, so
/// two spells of the same name, as Plaguefang's and Stinging Viper's Poison, put their dots on one
/// aura, whose gain and expiry run both dots' callbacks. The runtime follows that for the dots of
/// weapon procs, which tick a flat amount on current stats and hold no class behavior.
fn shared_dot_auras(prepared: &PreparedV2) -> Vec<String> {
    let spells = &prepared.player.spells;
    let proc_dot = |index: usize| {
        prepared.effects.iter().any(|effect| {
            matches!(effect, Effect::SpellDataDamageProc { spell, periodic: Some(_), .. }
                if *spell == index)
        })
    };
    let mut reasons = Vec::new();
    for (index, spell) in spells.iter().enumerate() {
        let Some(dot) = spell.dot.as_ref() else {
            continue;
        };
        for (earlier, other) in spells[..index].iter().enumerate() {
            if other
                .dot
                .as_ref()
                .is_some_and(|other| other.aura_label == dot.aura_label && other.unit == dot.unit)
                && !(proc_dot(earlier) && proc_dot(index))
            {
                reasons.push(format!(
                    "the dots of {} and {} share the aura {}",
                    other.action_id.clone().unwrap_or_default(),
                    spell.action_id.clone().unwrap_or_default(),
                    dot.aura_label
                ));
            }
        }
    }
    reasons
}

/// Where a weapon proc's exported shape disagrees with the spell it casts: the attack table must
/// be the one the spell's defense type names with the crit the client allows, a damage over time
/// needs the dot the spell carries, and a missile cannot carry only a dot.
fn proc_damage_mismatch(
    spell: &Spell,
    outcome: Option<&str>,
    can_crit: bool,
    periodic: Option<&ProcPeriodic>,
) -> Option<String> {
    let (table, crit) = match outcome {
        Some(name) => (
            if name.starts_with("melee_") {
                "DefenseTypeMelee"
            } else {
                "DefenseTypeRanged"
            },
            name.ends_with("_and_crit") || name.ends_with("_crit_only"),
        ),
        None => ("DefenseTypeMagic", can_crit),
    };
    if periodic.is_some_and(|periodic| !periodic.with_direct) {
        if spell.dot.is_none() || spell.missile_speed != 0.0 {
            return Some("a damage over time alone needs its dot and no missile".to_string());
        }
        return None;
    }
    if spell.defense_type != table || crit != can_crit {
        return Some(format!(
            "the {outcome:?} table disagrees with the spell's {} and crit",
            spell.defense_type
        ));
    }
    if periodic.is_some() && spell.dot.is_none() {
        return Some("a damage over time needs the dot the spell carries".to_string());
    }
    None
}

/// Stats a temporary stat change may set: the runtime reads spell damage, attack powers, crit
/// and maximum mana during a fight, and nothing in scope reads healing power.
const DYNAMIC_STATS: &[&str] = &[
    "SpellDamage",
    "AttackPower",
    "RangedAttackPower",
    "HealingPower",
    "SpellCritPercent",
    "PhysicalCritPercent",
    "MP5",
    "Mana",
    // Physical damage, which a physical spell adds to its base damage: the combinations carry
    // it when one changes it.
    "PhysicalDamage",
    // Armor penetration, which the armor of a hit's target is reduced by: the combinations
    // carry it when one changes it.
    "ArmorPenetration",
    // Spell piercing, which a partial resist takes off the target's resistance.
    "SpellPiercing",
    // Melee and spell hit, which the hit chances read: the combinations carry them when one
    // changes them.
    "PhysicalHitPercent",
    "SpellHitPercent",
    // Spirit, which spirit regeneration, Life Tap and Dark Sacrifice read live.
    "Spirit",
    // Spell power by school, which spell power reads, and the resistances spells that hit
    // the player roll against: the combinations carry them when one changes them.
    "ArcaneDamage",
    "FireDamage",
    "FrostDamage",
    "HolyDamage",
    "NatureDamage",
    "ShadowDamage",
    "ArcaneResistance",
    "FireResistance",
    "FrostResistance",
    "NatureResistance",
    "ShadowResistance",
];

/// Whether a stat a stat aura or temporary stat change moves is one the runtime follows or
/// one nothing reads.
/// The auras an item swap's callbacks can switch on.
fn swap_activated_auras(effect: &Effect) -> Vec<&str> {
    let Effect::ItemSwap { callbacks, .. } = effect else {
        return Vec::new();
    };
    callbacks
        .iter()
        .filter_map(|callback| match callback {
            SwapCallback::Aura {
                aura,
                main,
                swapped,
                ..
            } => [main, swapped]
                .iter()
                .any(|state| matches!(state, SwapAuraState::On | SwapAuraState::Activate))
                .then_some(aura.as_str()),
            SwapCallback::AuraOnChange { aura, .. } => Some(aura.as_str()),
            _ => None,
        })
        .collect()
}

/// The fields of an effect whose values the runtime takes from the swapped state, by kind: a
/// value of the weapon in the hand. A field named with a slash is one inside each element of the
/// field's array.
const SWAP_EFFECT_FIELDS: &[(&str, &[&str])] = &[
    (
        "seal_of_righteousness",
        &["swing_speed", "hand_multiplier", "ranks/hit_coefficient"],
    ),
    ("seal_of_command", &["proc_chance"]),
    // The healing power the heals snapshot, which the swap set's weapon moves.
    ("paladin_heals", &["player", "target"]),
    ("crusader", &["chances"]),
    // Proc managers an item swap rebuilds for the weapons in the hands.
    ("stat_proc", &["chances"]),
    ("sulfuras_hand_of_ragnaros", &["chances"]),
    ("thunderfury", &["chances"]),
    ("spell_data_damage_proc", &["chances", "trigger_spells"]),
    ("extra_attack_proc", &["chances"]),
    ("stacking_cd_on_use", &["chances"]),
    ("armor_debuff_proc", &["chances"]),
    (
        "rage_bar",
        &[
            "main_hand_rage",
            "off_hand_rage",
            "main_hand_crit_rage",
            "off_hand_crit_rage",
        ],
    ),
    ("weaponmaster_sword", &["sword_hands"]),
    // A Warrior's shield, which an off hand swap puts on or takes off.
    ("shield_wall", &["can_block"]),
    ("shield_block", &["can_block"]),
    ("shield_slam", &["can_block"]),
    ("rage_on_avoid", &["can_block"]),
    ("cat_form", &["main_hand", "cat_weapon"]),
    ("bear_form", &["main_hand", "bear_weapon"]),
    ("flametongue_weapon", &["hands/base_damage"]),
    ("flametongue_totem", &["attack_damage"]),
    ("maelstrom_weapon", &["chances"]),
    // The imbue's proc manager rebuilt for the weapon in the hand.
    ("frostbrand_weapon", &["chances"]),
    // The imbue's proc manager rebuilt for the weapon in the hand, and whether a main hand
    // imbue blocks the party Windfury Totem (the trigger's gain takes the totem down).
    (
        "windfury_weapon",
        &["chances", "trigger_spells", "blocks_windfury_totem"],
    ),
    // Whether the shaman's own Windfury Totem is contested: the class gate refuses a reachable
    // one contested under either set.
    ("windfury_totem_self", &["contested"]),
    // Patch 181: the hands that carry Rockbiter and the main hand's speed, as the weapons worn say.
    (
        "rockbiter_threat",
        &["main_hand", "off_hand", "main_hand_swing_speed"],
    ),
];

/// The racial weapon specializations, whose aura also takes its crit back off the ranged auto
/// attack.
const RACIAL_WEAPON_SPECIALIZATIONS: &[&str] = &[
    "Mace Specialization",
    "Sword Specialization",
    "Axe Specialization",
];

/// What an item swap changes that the runtime does not follow.
fn item_swap_limits(
    prepared: &PreparedV2,
    effect: &Effect,
    claims: &dyn Fn(&Effect) -> Vec<(&'static str, String)>,
) -> Vec<String> {
    let Effect::ItemSwap {
        slots,
        main_aura,
        swapped_aura,
        callbacks,
        swapped,
        aura_effects,
        ..
    } = effect
    else {
        return Vec::new();
    };
    let mut reasons = Vec::new();
    let player = &prepared.player;
    // Both Warrior and Druid can swap a paired main/off-hand set when neither
    // set dual-wields. Native preparation still rejects an actual off-hand
    // weapon, and the coverage checks below enforce callback/state fidelity.
    let off_hand = slots.as_slice() == ["ItemSlotMainHand", "ItemSlotOffHand"]
        && matches!(
            prepared.player.class.as_str(),
            "ClassWarrior" | "ClassDruid"
        )
        && !prepared.melee.dual_wielding;
    if slots.as_slice() != ["ItemSlotMainHand"] && !off_hand {
        reasons.push(format!("an item swap of slots {slots:?} is unsupported"));
    }
    // The target's swings read the player's block from tables the reset's shield built.
    if off_hand && prepared.enemy.is_some() {
        reasons
            .push("an off hand swap while the target swings at the player is unsupported".into());
    }
    if !["ClassPaladin", "ClassWarrior", "ClassShaman", "ClassDruid"]
        .contains(&prepared.player.class.as_str())
    {
        reasons.push(format!(
            "an item swap of a {} is unsupported",
            prepared.player.class
        ));
    }
    let stat_labels: Vec<&String> = prepared
        .effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::StatAuras { auras, .. } => Some(auras),
            _ => None,
        })
        .flatten()
        .collect();
    let claimed = |label: &str| {
        prepared.effects.iter().any(|effect| {
            claims(effect)
                .iter()
                .any(|(unit, claimed)| *unit == "player" && claimed == label)
        })
    };
    let aura_of = |label: &str| player.auras.iter().position(|aura| aura.label == label);
    let mut targets: Vec<&str> = vec![main_aura, swapped_aura];
    let mut item_spells = Vec::new();
    // The crit modifiers the swap switches, in callback order: the runtime adds and takes away
    // their crit as Go's callbacks do.
    let mut crit_mods: Vec<(&[usize], f64, bool, bool)> = Vec::new();
    // The damage modifiers it switches, as Two-Handed Weapon Specialization's for a one-hander.
    let mut damage_mods: Vec<(&[usize], f64, bool, bool)> = Vec::new();
    // The threat modifiers it switches, as push 113554's Two-Handed Weapon Specialization's.
    let mut threat_mods: Vec<(&[usize], f64, bool, bool)> = Vec::new();
    for callback in callbacks {
        let (label, toggles) = match callback {
            SwapCallback::Aura {
                aura,
                main,
                swapped,
                ..
            } => (aura, main != swapped),
            SwapCallback::AuraOnChange {
                aura,
                main,
                swapped,
                ..
            } => (aura, main != swapped),
            SwapCallback::ItemUse { spell, aura, .. } => {
                item_spells.push(*spell);
                if let Some(aura) = aura {
                    targets.push(aura);
                }
                continue;
            }
            SwapCallback::SpellMod { modifier, .. } => {
                if !matches!(
                    modifier.kind.as_str(),
                    "damage_done_pct" | "bonus_crit_percent" | "threat_multiplier_pct"
                ) {
                    reasons.push(format!(
                        "an item swap switches a spell modifier of kind {}",
                        modifier.kind
                    ));
                }
                if modifier
                    .spells
                    .iter()
                    .any(|&spell| spell >= player.spells.len())
                {
                    reasons.push("an item swap's spell modifier names an unknown spell".into());
                }
                if let SwapCallback::SpellMod { main, swapped, .. } = callback {
                    match modifier.kind.as_str() {
                        "bonus_crit_percent" => {
                            crit_mods.push((&modifier.spells, modifier.value, *main, *swapped))
                        }
                        "threat_multiplier_pct" => {
                            threat_mods.push((&modifier.spells, modifier.value, *main, *swapped))
                        }
                        _ => damage_mods.push((&modifier.spells, modifier.value, *main, *swapped)),
                    }
                }
                continue;
            }
        };
        targets.push(label);
        let Some(index) = aura_of(label) else {
            reasons.push(format!(
                "an item swap switches aura {label:?}, which is not registered"
            ));
            continue;
        };
        let aura = &player.auras[index];
        // What the aura does as it is gained and fades: its stats, a listener an effect runs,
        // an effect of the swap's own, or nothing.
        let gains = aura
            .callbacks
            .iter()
            .any(|callback| matches!(callback.as_str(), "on_gain" | "on_expire"));
        let followed = stat_labels.contains(&&aura.label)
            || claimed(&aura.label)
            || aura_effects.iter().any(|effect| effect.aura == aura.label);
        if gains && !followed {
            reasons.push(format!(
                "an item swap switches aura {label:?}, whose gain and expiry the runtime does not follow"
            ));
        }
        // Each category the runtime enforces or tracks holds its members' bids as the reset left
        // them; one an effect claims is that effect's to follow.
        if toggles && !aura.exclusive_memberships.is_empty() && !claimed(&aura.label) {
            let flametongue = aura
                .exclusive_memberships
                .iter()
                .all(|membership| membership.category == "FlametongueTotem");
            if !flametongue || !flametongue_totem_held(prepared, &targets) {
                reasons.push(format!(
                    "an item swap switches aura {label:?}, which bids in an exclusive category"
                ));
            }
        }
        if toggles
            && RACIAL_WEAPON_SPECIALIZATIONS.contains(&label.as_str())
            && player.spells.iter().any(|spell| {
                spell
                    .proc_mask
                    .iter()
                    .any(|mask| mask == "ProcMaskRangedAuto")
            })
        {
            reasons.push(format!(
                "an item swap switches {label}, whose crit on the ranged auto attack is not followed"
            ));
        }
    }
    for effect in aura_effects {
        let active = aura_of(&effect.aura).is_some_and(|index| player.auras[index].active);
        let reset = if active {
            effect.armor_ignore_factor
        } else {
            0.0
        };
        if prepared.melee.armor_ignore_factor != reset {
            reasons.push(format!(
                "{} changes an armor ignore factor that other effects also change",
                effect.aura
            ));
        }
    }
    // A main hand Windfury Weapon the swap switches on outbids the party Windfury Totem: its
    // trigger's gain takes the totem and its trigger down, as the swapped state shows them.
    let totem_auras: Vec<&str> = prepared
        .effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::WindfuryTotem {
                totem_aura,
                trigger_aura,
                ..
            } => Some([totem_aura.as_str(), trigger_aura.as_str()]),
            _ => None,
        })
        .flatten()
        .collect();
    // Either way: a swap that puts the blocking imbue on, or takes it off and frees the totem,
    // which comes back at its next refresh as its trigger's expiry allows.
    let imbue_blocks_totem = swapped.iter().any(|change| {
        let parts: Vec<&str> = change.path.split('/').skip(1).collect();
        matches!(parts.as_slice(), ["effects", index, "blocks_windfury_totem"]
            if index.parse::<usize>().ok().and_then(|index| prepared.effects.get(index))
                .is_some_and(|effect| matches!(effect, Effect::WindfuryWeapon { trigger_aura, .. }
                    if targets.contains(&trigger_aura.as_str()))))
    });
    let displaced_totem: Vec<&str> = if imbue_blocks_totem {
        totem_auras
    } else {
        Vec::new()
    };
    for change in swapped {
        if let Some(reason) = swap_change_limit(
            prepared,
            change,
            &targets,
            &item_spells,
            &crit_mods,
            &damage_mods,
            &threat_mods,
            aura_effects,
            &displaced_totem,
        ) {
            reasons.push(reason);
        }
    }
    reasons
}

/// Whether a Flametongue Weapon trigger an item swap switches leaves Flametongue Totem's benefit
/// off: a permanent aura it does not switch outbids the totem in the category, as the party's
/// Windfury Totem does, or there is no totem.
fn flametongue_totem_held(prepared: &PreparedV2, switched: &[&str]) -> bool {
    let totems: Vec<&str> = prepared
        .effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::FlametongueTotem { aura, .. } => Some(aura.as_str()),
            _ => None,
        })
        .collect();
    let bid = |label: &str| {
        prepared
            .player
            .auras
            .iter()
            .filter(|aura| aura.label == label)
            .flat_map(|aura| &aura.exclusive_memberships)
            .filter(|membership| membership.category == "FlametongueTotem")
            .map(|membership| membership.priority)
            .fold(f64::NEG_INFINITY, f64::max)
    };
    if totems.is_empty() {
        return true;
    }
    let totem_bid = totems
        .iter()
        .map(|label| bid(label))
        .fold(f64::NEG_INFINITY, f64::max);
    prepared.player.auras.iter().any(|aura| {
        aura.active
            && aura.duration_ns == crate::core::time::NEVER_EXPIRES
            && !switched.contains(&aura.label.as_str())
            && aura.exclusive_memberships.iter().any(|membership| {
                membership.category == "FlametongueTotem" && membership.priority > totem_bid
            })
    })
}

/// Why the runtime cannot follow one value the swapped state changes, if it cannot.
#[allow(clippy::too_many_arguments)]
fn swap_change_limit(
    prepared: &PreparedV2,
    change: &SwapChange,
    switched: &[&str],
    item_spells: &[usize],
    crit_mods: &[(&[usize], f64, bool, bool)],
    damage_mods: &[(&[usize], f64, bool, bool)],
    threat_mods: &[(&[usize], f64, bool, bool)],
    aura_effects: &[crate::contracts::prepared_v2::SwapAuraEffect],
    displaced_totem: &[&str],
) -> Option<String> {
    let parts: Vec<&str> = change.path.split('/').skip(1).collect();
    let refuse = || {
        Some(format!(
            "an item swap changes {}, which the runtime does not follow",
            change.path
        ))
    };
    match parts.as_slice() {
        // The stats follow the stat auras, the swap's own among them.
        ["player", "stats", _] | ["player", "mana", _] => None,
        // The player's movement speed after the reset, which the speed auras' gain and expiry
        // move: the swap changes it only through auras it switches.
        ["effects", index, "speed_multiplier"]
            if index.parse::<usize>().ok().is_some_and(|index| {
                matches!(prepared.effects.get(index), Some(Effect::PlayerMovement { speed_auras, .. })
                    if speed_auras.iter().all(|aura| switched.contains(&aura.as_str())))
            }) =>
        {
            None
        }
        // The melee speed after the reset, which the item and enchant speed auras the swap
        // switches multiply in as they are gained and divide out as they fade.
        ["melee", "melee_speed_multiplier"] => {
            let mut worn = prepared.melee.melee_speed_multiplier;
            for effect in &prepared.effects {
                if let Effect::SpeedAura {
                    aura,
                    melee_multiplier,
                    ..
                } = effect
                {
                    if !switched.contains(&aura.as_str()) {
                        continue;
                    }
                    let active = prepared
                        .player
                        .auras
                        .iter()
                        .any(|candidate| candidate.label == *aura && candidate.active);
                    worn = if active {
                        worn / melee_multiplier
                    } else {
                        worn * melee_multiplier
                    };
                }
            }
            let follows = change
                .value
                .as_f64()
                .is_some_and(|value| (value - worn).abs() <= 1e-12 * worn.abs());
            if follows {
                None
            } else {
                refuse()
            }
        }
        // The crit the player takes on a Goblin Sapper Charge's half, which the swap sets on the
        // player as its own target. A tank's is read by the target's swings, whose rolls follow
        // the swap's stat aura among the combinations.
        ["player", "pseudo_stats", "reduced_crit_taken_percent"]
            if prepared
                .enemy
                .as_ref()
                .is_none_or(|enemy| enemy.rolls.len() > 1) =>
        {
            None
        }
        // Spirit Weapons' threat, which a Rockbiter aura the swap switches multiplies in as it is
        // gained and divides out as it fades.
        ["player", "pseudo_stats", "threat_multiplier"] => {
            let reset = prepared.player.pseudo_stats.threat_multiplier;
            let follows = prepared.effects.iter().any(|effect| match effect {
                Effect::RockbiterWeapon {
                    aura,
                    threat_multiplier: Some(threat),
                    ..
                } if switched.contains(&aura.as_str()) => {
                    let active = prepared
                        .player
                        .auras
                        .iter()
                        .any(|candidate| candidate.label == *aura && candidate.active);
                    let worn = if active {
                        reset / threat
                    } else {
                        reset * threat
                    };
                    change
                        .value
                        .as_f64()
                        .is_some_and(|value| value.to_bits() == worn.to_bits())
                }
                _ => false,
            });
            if follows {
                None
            } else {
                refuse()
            }
        }
        ["player", "auras", index, field]
            if matches!(*field, "active" | "blocked_at_reset" | "displaced_by") =>
        {
            let label = &prepared
                .player
                .auras
                .get(index.parse::<usize>().ok()?)?
                .label;
            if displaced_totem.contains(&label.as_str()) {
                None
            } else if *field == "displaced_by" {
                refuse()
            } else if switched.contains(&label.as_str()) {
                None
            } else {
                refuse()
            }
        }
        ["player", "spells", index, "flags"] => {
            let index: usize = index.parse().ok()?;
            let mut flags = prepared.player.spells.get(index)?.flags.clone();
            flags.push("SpellFlagSwapped".into());
            let now: Vec<String> = serde_json::from_value(change.value.clone()).unwrap_or_default();
            let sorted = |mut list: Vec<String>| {
                list.sort();
                list
            };
            if item_spells.contains(&index) && sorted(flags) == sorted(now) {
                None
            } else {
                refuse()
            }
        }
        // A weapon specialization's crit, as its modifiers leave it with the swap set worn.
        ["player", "spells", index, "bonus_crit_percent"] => {
            let index: usize = index.parse().ok()?;
            let mut crit = prepared.player.spells.get(index)?.bonus_crit_percent;
            for (spells, value, main, swapped) in crit_mods {
                if !spells.contains(&index) || main == swapped {
                    continue;
                }
                if *swapped {
                    crit += value;
                } else {
                    crit -= value;
                }
            }
            if change
                .value
                .as_f64()
                .is_some_and(|value| value.to_bits() == crit.to_bits())
            {
                None
            } else {
                refuse()
            }
        }
        // A spell's damage multiplier as the damage modifiers the swap switches leave it.
        ["player", "spells", index, field @ ("damage_multiplier" | "threat_multiplier")] => {
            let index: usize = index.parse().ok()?;
            let spell = prepared.player.spells.get(index)?;
            let (mut multiplier, mods) = if *field == "damage_multiplier" {
                (spell.damage_multiplier, damage_mods)
            } else {
                (spell.threat_multiplier, threat_mods)
            };
            for (spells, value, main, swapped) in mods {
                if !spells.contains(&index) || main == swapped {
                    continue;
                }
                if *swapped {
                    multiplier *= 1.0 + value;
                } else {
                    multiplier /= 1.0 + value;
                }
            }
            if change
                .value
                .as_f64()
                .is_some_and(|value| value.to_bits() == multiplier.to_bits())
            {
                None
            } else {
                refuse()
            }
        }
        ["melee", "main_hand", ..] => None,
        // Go updateWeaponSkills: the tables of the hands whose skill the equipment sets.
        ["melee", "main_hand_skill" | "off_hand_skill" | "ranged_skill", ..] => None,
        ["melee", "armor_ignore_factor"] if !aura_effects.is_empty() => None,
        // The effect roll a hit reads the weapon in the hand for, as the party's Flametongue
        // Totem does with the main hand's speed: the runtime takes the swapped average for a
        // plain effect roll only.
        ["effects", index, "average"]
            if matches!(
                prepared.effects.get(index.parse::<usize>().ok()?)?,
                Effect::SpellDataDamageProc {
                    roll: None,
                    outcome: None,
                    chain: None,
                    area: None,
                    periodic: None,
                    ..
                }
            ) =>
        {
            None
        }
        ["effects", index, field, rest @ ..] => {
            let effect = prepared.effects.get(index.parse::<usize>().ok()?)?;
            let fields = SWAP_EFFECT_FIELDS
                .iter()
                .find(|(kind, _)| *kind == effect.kind())
                .map(|(_, fields)| *fields)
                .unwrap_or_default();
            let followed = fields
                .iter()
                .any(|followed| match followed.split_once('/') {
                    None => followed == field,
                    Some((array, inner)) => {
                        array == *field
                            && matches!(rest, [element, last]
                            if element.parse::<usize>().is_ok() && *last == inner)
                    }
                });
            if followed {
                None
            } else {
                refuse()
            }
        }
        _ => refuse(),
    }
}

fn followed_stat(stat: &str) -> bool {
    DYNAMIC_STATS.contains(&stat) || INERT_STATS.contains(&stat)
}

/// Stats a stat aura may change without the runtime reading them: inputs to the stats it
/// reads, and stats nothing in scope reads.
const INERT_STATS: &[&str] = &[
    // Spell crit and maximum mana; mana regeneration reads only spirit and MP5, and the
    // combinations carry Intellect for the regeneration refresh a change of it causes.
    "Intellect",
    // Spirit regeneration, which the combinations carry; a class that reads Spirit itself
    // refuses a change of it.
    "Spirit",
    "Strength",
    "Agility",
    "Stamina",
    // Spell crit rating, whose percent the combinations carry.
    "SpellCritRating",
    // Hit and melee crit ratings, whose percents the combinations carry.
    "MeleeHitRating",
    "SpellHitRating",
    "MeleeCritRating",
    "Health",
    "Armor",
    "BonusArmor",
    "BlockValue",
    "BlockPercent",
    "DodgeRating",
    "DodgePercent",
    "ParryRating",
    "ParryPercent",
    "FeralAttackPower",
];

/// The stats the runtime follows on the player but does not pass on to a dynamic pet, whose
/// inheritance carries only its tracked powers.
const UNINHERITED_STATS: &[&str] = &[
    "Spirit",
    "RangedCritPercent",
    "ExpertisePercent",
    "ArcaneDamage",
    "FireDamage",
    "FrostDamage",
    "HolyDamage",
    "NatureDamage",
    "ShadowDamage",
    "ArcaneResistance",
    "FireResistance",
    "FrostResistance",
    "NatureResistance",
    "ShadowResistance",
];

/// A dynamic pet that would inherit a change of one of those stats.
fn uninherited_stat_changes(prepared: &PreparedV2) -> Vec<String> {
    let mut changed: BTreeSet<&str> = BTreeSet::new();
    for effect in &prepared.effects {
        match effect {
            Effect::StatAuras { changed: stats, .. } => {
                changed.extend(stats.iter().map(String::as_str))
            }
            Effect::BloodFury { active_stats, .. }
            | Effect::DiamondFlask { active_stats, .. }
            | Effect::TemporaryStats { active_stats, .. } => {
                changed.extend(active_stats.keys().map(String::as_str))
            }
            _ => {}
        }
    }
    let mut reasons = Vec::new();
    for pet in &prepared.pets {
        // A hunter pet's crit offset follows the owner's ranged-only crit (patch 138), which
        // the runtime holds fixed.
        if changed.contains("RangedCritPercent")
            && pet
                .inheritance
                .iter()
                .any(|term| term.pet == "PhysicalCritPercent")
        {
            reasons.push(format!(
                "pet {} takes a crit offset from RangedCritPercent, which changes during the fight",
                pet.label
            ));
        }
        for term in &pet.inheritance {
            if UNINHERITED_STATS.contains(&term.owner.as_str())
                && changed.contains(term.owner.as_str())
            {
                reasons.push(format!(
                    "pet {} inherits {}, which changes during the fight",
                    pet.label, term.owner
                ));
            }
        }
    }
    reasons
}

/// Stat aura combinations that change a stat the runtime holds fixed. Ranged crit is read only
/// by ranged attacks (a pet's offset from it is checked on its own), so a player with none of
/// them never reads it.
fn fixed_stat_aura_changes(prepared: &PreparedV2) -> Vec<String> {
    let ranged = prepared.player.spells.iter().any(|spell| {
        spell
            .proc_mask
            .iter()
            .any(|mask| mask == "ProcMaskRangedAuto" || mask == "ProcMaskRangedSpecial")
    });
    // Without a tank nothing swings at the player, so its defense is read by no hit; the crit
    // it takes only by a Goblin Sapper Charge's half, whose value an item swap that changes it
    // sets on the player. A tank's are read by the target's swings, whose rolls preparation
    // reads under every stat aura combination, so they follow the combination there.
    let defense_followed = prepared.enemy.is_none()
        || prepared
            .enemy
            .as_ref()
            .is_some_and(|enemy| enemy.rolls.len() > 1);
    let sapper = prepared
        .effects
        .iter()
        .any(|effect| effect.kind() == "goblin_sapper");
    let swap_sets_crit_taken = prepared.effects.iter().any(|effect| {
        matches!(effect, Effect::ItemSwap { swapped, .. }
            if swapped.iter().any(|change| change.path == "/player/pseudo_stats/reduced_crit_taken_percent"))
    });
    // Under Forever's rules the combinations carry the crit the player takes when one changes
    // it, which the runtime sets on the Goblin Sapper Charge's hit on its thrower.
    let crit_taken_carried = prepared.effects.iter().any(|effect| {
        matches!(effect, Effect::StatAuras { combos, .. }
            if combos.iter().all(|combo| combo.contains_key("PseudoReducedCritTakenPercent")))
    });
    let inert = move |stat: &str| {
        defense_followed
            && (stat == "DefenseRating"
                || (stat == "ReducedCritTakenPercent"
                    && (!sapper || swap_sets_crit_taken || crit_taken_carried)))
    };
    prepared
        .effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::StatAuras { auras, changed, .. } => Some((auras, changed)),
            _ => None,
        })
        .flat_map(move |(auras, changed)| {
            changed
                .iter()
                .filter(move |stat| {
                    !followed_stat(stat)
                        && (ranged || *stat != "RangedCritPercent")
                        && !inert(stat.as_str())
                })
                .map(move |stat| {
                    format!("stat auras {auras:?} change {stat}, which the runtime holds fixed")
                })
        })
        .collect()
}

/// Temporary stat changes to stats the runtime holds fixed.
fn fixed_stat_changes(prepared: &PreparedV2) -> Vec<String> {
    prepared
        .effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::BloodFury {
                aura, active_stats, ..
            }
            | Effect::DiamondFlask {
                aura, active_stats, ..
            }
            | Effect::TemporaryStats {
                aura, active_stats, ..
            } => Some((aura, active_stats)),
            _ => None,
        })
        .flat_map(|(aura, stats)| {
            stats
                .keys()
                .filter(|stat| !followed_stat(stat))
                .map(move |stat| format!("{aura} changes {stat}, which the runtime holds fixed"))
        })
        .collect()
}

/// Stats that change the player's health, which stay fixed while the player tanks. The
/// target's swings carry their rolls for each stat aura combination.
const DEFENDER_STATS: &[&str] = &["Stamina", "Health"];

/// Effects whose behaviors act on the hits the player takes from the target's swings.
const HIT_TAKEN_EFFECTS: &[&str] = &[
    "chance_of_death",
    "sulfuras_hand_of_ragnaros",
    "natural_reaction",
    "natures_bounty",
    "parry_haste",
    "pushback_trigger",
    "rage_bar",
    "redoubt",
    "shield_specialization",
    "reckoning",
    "holy_shield",
    "eye_for_an_eye",
    "inert_listener",
    "revenge",
    "retaliation",
    "rage_on_avoid",
    "shield_block",
    "warrior_enrage",
    "blood_craze",
    "riposte",
    "spell_data_damage_proc",
    "spell_data_absorb_proc",
    "force_reactive_disk",
    "spell_data_heal_proc",
    "spell_data_stat_proc",
    "battlegear_of_might_rage",
];

/// Effects that slow the target's melee speed through its live multiplier, with the target
/// auras they claim.
const ENEMY_SPEED_EFFECTS: &[&str] = &["thunder_clap", "thunderfury"];

/// Callbacks the target's own swings fire on the target.
const TARGET_CASTER_CALLBACKS: &[&str] = &[
    "on_apply_effects",
    "on_cast_complete",
    "on_spell_hit_dealt",
    "on_periodic_damage_dealt",
];

/// Whether a spell is a survival major cooldown Go never fires: no timings, no defensive
/// health threshold and no rotation action that casts it.
fn survival_cooldown_never_fires(prepared: &PreparedV2, spell_id: i32) -> bool {
    let survival = prepared.player.hp_percent_for_defensives == 0.0
        && prepared.player.major_cooldowns.iter().any(|cooldown| {
            cooldown.action_id.spell_id == spell_id
                && cooldown.kind.iter().any(|kind| kind == "survival")
                && cooldown.timings_ns.is_empty()
        });
    let Ok(rotation) = crate::rotation::parse(&prepared.player.rotation) else {
        return false;
    };
    let cast = rotation
        .prepull
        .iter()
        .map(|prepull| &prepull.action)
        .chain(rotation.priority_list.iter().map(|item| &item.action))
        .any(|action| action.spells().iter().any(|id| id.spell_id == spell_id));
    survival && !cast
}

/// What the runtime cannot follow once the target swings at the player: listeners of the
/// swings it does not run, auras something in scope activates that change the swings, and
/// defender stats a stat aura changes.
fn tank_limits(
    prepared: &PreparedV2,
    enemy: &crate::contracts::prepared_v2::Enemy,
    claims: &dyn Fn(&Effect) -> Vec<(&'static str, String)>,
    rotation: Option<&Rotation>,
) -> Vec<String> {
    let mut reasons = Vec::new();
    // An aura only becomes active in the runtime through an effect that names it.
    let mut named: BTreeSet<String> = BTreeSet::new();
    // Labels an effect carries without claiming them, which a class may still activate.
    let mut carried: BTreeSet<String> = BTreeSet::new();
    for effect in &prepared.effects {
        // A racial survival cooldown that never fires leaves its aura inactive.
        if let Effect::Stoneform { spell_id, .. } = effect {
            if survival_cooldown_never_fires(prepared, *spell_id) {
                continue;
            }
        }
        for (unit, label) in claims(effect) {
            named.insert(format!("{unit}:{label}"));
        }
        // A class may activate any aura its effect carries without claiming it, so every label
        // an effect names on either unit counts. The movement effect only lists the auras whose
        // effects change the speed, and a refresh effect only the auras a rotation reads.
        if matches!(
            effect,
            Effect::PlayerMovement { .. } | Effect::AuraShouldRefresh { .. }
        ) {
            continue;
        }
        if let Ok(value) = serde_json::to_value(effect) {
            let mut strings = Vec::new();
            collect_strings(&value, &mut strings);
            for label in strings {
                carried.insert(format!("player:{label}"));
                carried.insert(format!("target:{label}"));
            }
        }
        match effect {
            Effect::StatAuras { auras, .. } => {
                named.extend(auras.iter().map(|label| format!("player:{label}")));
            }
            Effect::FixedUptimeAura { aura, .. } => {
                named.insert(format!("player:{aura}"));
            }
            _ => {}
        }
    }
    // A racial aura whose cast is an untimed survival cooldown the rotation never names never
    // activates at a zero defensive health threshold: Go fires such a cooldown only below it.
    let named_by_rotation = |spell_id: i32| {
        rotation.is_some_and(|rotation| {
            let casts = |action: &Action| {
                action
                    .spells()
                    .into_iter()
                    .any(|id| id.spell_id == spell_id && id.item_id == 0)
            };
            rotation
                .prepull
                .iter()
                .any(|prepull| casts(&prepull.action))
                || rotation
                    .priority_list
                    .iter()
                    .any(|item| casts(&item.action))
        })
    };
    for effect in &prepared.effects {
        if let Effect::Stoneform { spell_id, aura, .. }
        | Effect::ShatterCurse { spell_id, aura, .. } = effect
        {
            let never_fires = prepared.player.hp_percent_for_defensives == 0.0
                && prepared.player.major_cooldowns.iter().any(|cooldown| {
                    cooldown.action_id.spell_id == *spell_id
                        && cooldown.timings_ns.is_empty()
                        && cooldown.kind.iter().any(|kind| kind == "survival")
                });
            if never_fires && !named_by_rotation(*spell_id) {
                named.remove(&format!("player:{aura}"));
                carried.remove(&format!("player:{aura}"));
            }
        }
    }
    let can_be_active = |unit: &str, aura: &crate::contracts::prepared_v2::Aura| {
        aura.active || named.contains(&format!("{unit}:{}", aura.label))
    };
    for aura in &prepared.player.auras {
        if !can_be_active("player", aura)
            || !aura.callbacks.iter().any(|c| c == "on_spell_hit_taken")
        {
            continue;
        }
        let handled = prepared.effects.iter().any(|effect| {
            HIT_TAKEN_EFFECTS.contains(&effect.kind())
                && claims(effect)
                    .iter()
                    .any(|(unit, label)| *unit == "player" && *label == aura.label)
        });
        if !handled {
            reasons.push(format!(
                "player aura {:?} reacts to the target's swings",
                aura.label
            ));
        }
    }
    for aura in &prepared.target.auras {
        if can_be_active("target", aura)
            && aura
                .callbacks
                .iter()
                .any(|c| TARGET_CASTER_CALLBACKS.contains(&c.as_str()))
        {
            reasons.push(format!(
                "target aura {:?} reacts to the target's own swings",
                aura.label
            ));
        }
    }
    for aura in &enemy.changing_auras {
        if named.contains(aura) || carried.contains(aura) {
            reasons.push(format!("{aura} changes the target's swings at the player"));
        }
    }
    // The runtime reads the player's damage taken multiplier live, which only the pseudo stat
    // and damage taken effects multiply.
    let mut tracked: BTreeSet<String> = BTreeSet::new();
    for effect in &prepared.effects {
        match effect {
            Effect::PseudoStatAuras { auras } => tracked.extend(
                auras
                    .iter()
                    .filter(|entry| entry.stat == "damage_taken")
                    .map(|entry| format!("player:{}", entry.aura)),
            ),
            Effect::PlayerDamageTaken { auras } => {
                tracked.extend(auras.iter().map(|entry| format!("player:{}", entry.aura)))
            }
            _ => {}
        }
    }
    for aura in &enemy.damage_taken_auras {
        if named.contains(aura) && !tracked.contains(aura) {
            reasons.push(format!(
                "{aura} changes the player's damage taken, which nothing multiplies"
            ));
        }
    }
    // The runtime reads the player's physical damage taken multiplier live, which only the
    // racial survival effects multiply, while the rolls hold the reset's value.
    let live_school = {
        let physical = prepared
            .player
            .pseudo_stats
            .school_damage_taken_multiplier
            .physical;
        enemy
            .rolls
            .iter()
            .chain(&enemy.reduced_avoidance_rolls)
            .all(|rolls| {
                rolls.school_damage_taken_multiplier == Some(physical)
                    && rolls.table_damage_taken_multiplier.is_some()
            })
    };
    for aura in &enemy.school_damage_taken_auras {
        let multiplied = prepared.effects.iter().any(|effect| match effect {
            Effect::Stoneform { aura: label, .. } | Effect::ShatterCurse { aura: label, .. } => {
                format!("player:{label}") == *aura
            }
            _ => false,
        });
        if named.contains(aura) && !(multiplied && live_school) {
            reasons.push(format!(
                "{aura} changes the player's physical damage taken, which the runtime does not \
                 follow"
            ));
        }
    }
    for aura in &enemy.speed_auras {
        let slowed = prepared.effects.iter().any(|effect| {
            ENEMY_SPEED_EFFECTS.contains(&effect.kind())
                && claims(effect)
                    .iter()
                    .any(|(unit, label)| format!("{unit}:{label}") == *aura)
        });
        if named.contains(aura) && !slowed {
            reasons.push(format!(
                "{aura} slows the target's swings without an effect"
            ));
        }
    }
    // Each attack power aura's value holds while it is the only one active.
    let attack_power: Vec<&str> = enemy
        .attack_power_auras
        .iter()
        .filter(|entry| named.contains(&format!("target:{}", entry.aura)))
        .map(|entry| entry.aura.as_str())
        .collect();
    if attack_power.len() > 1 {
        reasons.push(format!(
            "target auras {attack_power:?} change the target's attack power together"
        ));
    }
    for effect in &prepared.effects {
        if let Effect::StatAuras {
            auras,
            combos,
            changed,
            raw_mp5,
            ..
        } = effect
        {
            for label in raw_mp5.keys().filter(|label| !auras.contains(label)) {
                reasons.push(format!(
                    "stat auras {auras:?} give a raw MP5 bonus to {label:?}, which is not one of them"
                ));
            }
            // Stamina moves only the maximum health the combinations carry.
            let health_tracked = combos.iter().all(|combo| combo.contains_key("Health"));
            for stat in changed {
                if DEFENDER_STATS.contains(&stat.as_str()) && !health_tracked {
                    reasons.push(format!(
                        "stat auras {auras:?} change {stat}, which the player's health holds fixed"
                    ));
                }
            }
        }
    }
    reasons
}

/// Every string in a JSON value, depth first.
fn collect_strings(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => out.push(text.clone()),
        serde_json::Value::Array(items) => items.iter().for_each(|item| collect_strings(item, out)),
        serde_json::Value::Object(map) => map.values().for_each(|item| collect_strings(item, out)),
        _ => {}
    }
}

/// Go `GetAPLSpell`: the first APL-flagged spell with the action ID, otherwise the first
/// registered one, as a spellbook position. The potion action names the first combat
/// potion. A missing spell drops the rotation action in Go.
pub(crate) fn rotation_spell_index(prepared: &PreparedV2, id: &ActionId) -> Option<usize> {
    let spells = &prepared.player.spells;
    if id.other_id == "OtherActionPotion" {
        return spells
            .iter()
            .position(|spell| spell.has_flag("SpellFlagCombatPotion"));
    }
    spells
        .iter()
        .position(|spell| spell.action_id.as_ref() == Some(id) && spell.has_flag("SpellFlagAPL"))
        .or_else(|| {
            spells
                .iter()
                .position(|spell| spell.action_id.as_ref() == Some(id))
        })
        // Go `RankAliases`: an older rank a rotation names presses the known one.
        .or_else(|| {
            spells
                .iter()
                .position(|spell| spell.has_flag("SpellFlagAPL") && spell.has_rank_alias(id))
        })
}

/// Go `Spell.Dot`: the spell holding the dot a spell names, its own or its related dot
/// spell's.
pub(crate) fn dot_owner(prepared: &PreparedV2, index: usize) -> Option<usize> {
    let spells = &prepared.player.spells;
    match &spells[index] {
        spell if spell.dot.is_some() => Some(index),
        spell => spell
            .related_dot_spell
            .filter(|&related| spells.get(related).is_some_and(|s| s.dot.is_some())),
    }
}

/// Go `GetAPLDot` on a unit: the spell holding the dot a spell names. An area or self-only
/// dot is the caster's, whatever unit is named; any other dot sits on the targets, so the
/// player has none.
pub(crate) fn rotation_dot(prepared: &PreparedV2, id: &ActionId, unit: Unit) -> Option<usize> {
    let owner = rotation_spell_index(prepared, id).and_then(|index| dot_owner(prepared, index))?;
    let dot = prepared.player.spells[owner].dot.as_ref()?;
    (dot.unit == "self" || matches!(unit, Unit::Target(_))).then_some(owner)
}

/// Go `GetAPLMultidotSpell`: whether the spell has a dot `Spell.CurDot` returns, its own on a
/// target or its related dot spell's.
fn rotation_multidot(prepared: &PreparedV2, id: &ActionId) -> bool {
    let Some(index) = rotation_spell_index(prepared, id) else {
        return false;
    };
    let spells = &prepared.player.spells;
    match &spells[index].dot {
        Some(dot) if dot.unit != "self" => true,
        _ => spells[index]
            .related_dot_spell
            .is_some_and(|related| spells.get(related).is_some_and(|s| s.dot.is_some())),
    }
}

/// The base duration a `dotBaseDuration` value of the spell captured when Go built the rotation.
pub(crate) fn rotation_dot_base_duration(prepared: &PreparedV2, id: &ActionId) -> Option<i64> {
    prepared
        .player
        .rotation_dot_base_durations
        .iter()
        .find(|entry| &entry.spell == id)
        .map(|entry| entry.base_duration_ns)
}

/// [`rotation_spell_index`] as the exported spell.
pub(crate) fn rotation_spell<'a>(prepared: &'a PreparedV2, id: &ActionId) -> Option<&'a Spell> {
    rotation_spell_index(prepared, id).map(|index| &prepared.player.spells[index])
}

/// Go `GetAPLCastSpell`, as the runtime presses a cast: a registered spell without the APL flag
/// defers to an APL-flagged spell of the same action under another tag (the queue of an
/// on-next-swing ability, such as a Raptor Strike a spell picker names untagged), then to
/// [`rotation_spell`].
pub(crate) fn rotation_cast_spell<'a>(
    prepared: &'a PreparedV2,
    id: &ActionId,
) -> Option<&'a Spell> {
    let spells = &prepared.player.spells;
    if let Some(spell) = spells
        .iter()
        .find(|spell| spell.action_id.as_ref() == Some(id))
    {
        if !spell.has_flag("SpellFlagAPL") {
            let same = |other: &ActionId| {
                other.spell_id == id.spell_id
                    && other.item_id == id.item_id
                    && other.other_id == id.other_id
            };
            if let Some(apl) = spells.iter().find(|spell| {
                spell.has_flag("SpellFlagAPL") && spell.action_id.as_ref().is_some_and(same)
            }) {
                return Some(apl);
            }
        }
    }
    rotation_spell(prepared, id)
}

/// The spell a major cooldown casts: its own when an earlier spell shares its action ID,
/// otherwise the first spell with the ID, as the runtime binds it.
fn cooldown_spell<'a>(prepared: &'a PreparedV2, cooldown: &MajorCooldown) -> Option<&'a Spell> {
    match cooldown.spell {
        Some(index) => prepared.player.spells.get(index),
        None => rotation_spell(prepared, &cooldown.action_id),
    }
}

/// Spell properties the fight runtime does not implement. A class spell computes its own
/// damage, so a physical damage roll is a limit only on other spells.
fn runtime_limits(spell: &Spell, class_spell: bool) -> Vec<&'static str> {
    let mut limits = Vec::new();
    if spell.max_charges != 0 {
        limits.push("charges");
    }
    if spell.has_cast_requirement && spell.requirement_auras.is_none() {
        limits.push("cast requirements");
    }
    if spell.damage_effect.is_some() && spell.school & 1 != 0 && !class_spell {
        limits.push("physical damage");
    }
    limits
}

/// Match only native source-backed actor-level coverage already accepted by
/// native preparation. Unsupported races/builds still receive native refusals.
fn sourced_actor_level(class: &str, level: i32) -> bool {
    // This selected native source prepares genuine Troll Warlock actors
    // for every level 10..59, retaining the separately sourced L20 and legacy L60. Do not reject those prepared actors at the
    // independent combat coverage gate solely for being below level 60.
    if class == "ClassWarlock" && (10..=59).contains(&level) {
        return true;
    }
    match level {
        20 => matches!(
            class,
            "ClassPriest" | "ClassRogue" | "ClassDruid" | "ClassWarlock"
        ),
        30 => class == "ClassDruid",
        _ => false,
    }
}

pub(crate) fn prepared_coverage(
    prepared: &PreparedV2,
    rotation: Option<&Rotation>,
) -> Vec<Refusal> {
    let mut reasons = Vec::new();
    let player = &prepared.player;
    let Some(gate) = class_gate(&player.class) else {
        return vec![Refusal::new(
            "class_unsupported",
            format!("class {} is not supported", player.class),
        )];
    };
    // Native preparation owns source-backed actor identity. Avoid rejecting
    // the already-supported selected checkpoint levels before combat executes.
    let sourced_checkpoint = sourced_actor_level(&player.class, player.level);
    if player.level != 60 && !sourced_checkpoint {
        reasons.push(Refusal::new(
            "level_unsupported",
            format!("player level {} is not supported", player.level),
        ));
    }
    // The exporter writes the count only for a fight against several targets.
    let targets = prepared.encounter.target_count as usize;
    if targets == 1 || targets > crate::core::fight::MAX_TARGETS {
        reasons.push(Refusal::new(
            "target_count_invalid",
            format!(
                "target count {targets}: a fight has one target, or from 2 to {} copies of it",
                crate::core::fight::MAX_TARGETS
            ),
        ));
    }
    let target_level_supported = (60..=63).contains(&prepared.target.level)
        || (sourced_checkpoint && prepared.target.level == player.level);
    if !target_level_supported {
        reasons.push(Refusal::new(
            "level_unsupported",
            format!("target level {} is not supported", prepared.target.level),
        ));
    }

    let claims = |effect: &Effect| {
        let mut claimed = common_claims(effect);
        claimed.extend((gate.claims)(effect));
        claimed
            .into_iter()
            .map(|(unit, label)| (unit, label.to_string()))
            .collect::<Vec<_>>()
    };
    let mut required: BTreeSet<&str> = BTreeSet::new();
    let unit_auras = [
        ("player", &player.auras),
        ("target", &prepared.target.auras),
    ]
    .into_iter()
    .chain(prepared.pets.iter().map(|pet| ("pet", &pet.auras)));
    for (unit, auras) in unit_auras {
        for aura in auras {
            if !aura.active || !aura.has_event_callbacks() {
                continue;
            }
            let claimant = prepared.effects.iter().find(|effect| {
                claims(effect)
                    .iter()
                    .any(|(u, label)| *u == unit && *label == aura.label)
            });
            match claimant {
                Some(effect) => {
                    required.insert(effect.kind());
                }
                None => reasons.push(Refusal::new(
                    "aura_listener_unclaimed",
                    format!(
                        "{unit} aura {:?} listens to combat events without an effect",
                        aura.label
                    ),
                )),
            }
        }
    }

    // An item swap: the auras its callbacks switch on listen as an active aura does, and what
    // it changes must be what the runtime follows.
    if let Some(effect) = crate::core::fight::item_swap::item_swap_effect(prepared) {
        required.insert(effect.kind());
        for label in swap_activated_auras(effect) {
            let Some(aura) = player.auras.iter().find(|aura| aura.label == label) else {
                continue;
            };
            if aura.active || !aura.has_event_callbacks() {
                continue;
            }
            match prepared.effects.iter().find(|effect| {
                claims(effect)
                    .iter()
                    .any(|(u, claimed)| *u == "player" && *claimed == aura.label)
            }) {
                Some(effect) => {
                    required.insert(effect.kind());
                }
                None => reasons.push(Refusal::new(
                    "aura_listener_unclaimed",
                    format!(
                        "player aura {:?} an item swap switches on listens to combat events \
                         without an effect",
                        aura.label
                    ),
                )),
            }
        }
        reasons.extend(coded(
            "item_swap_unsupported",
            item_swap_limits(prepared, effect, &claims),
        ));
    }

    // Go applies Power Infusion's multipliers through its aura's exclusive effects, which only
    // the `power_infusion` effect of each copy of the aura lets the runtime follow.
    for aura in &player.auras {
        let bids = aura
            .exclusive_memberships
            .iter()
            .any(|membership| membership.category.starts_with("PowerInfusion"));
        let described = prepared.effects.iter().any(
            |effect| matches!(effect, Effect::PowerInfusion { aura: label, .. } if *label == aura.label),
        );
        if bids && !described {
            reasons.push(Refusal::new(
                "aura_listener_unclaimed",
                format!(
                    "player aura {:?} bids in Power Infusion's categories without an effect",
                    aura.label
                ),
            ));
        }
    }

    // The aura an external cooldown casts changes the player through an effect of its own:
    // Power Infusion's multipliers, Innervate's spirit regeneration or, for Mana Tide Totem, the
    // mana per 5 seconds of a stat aura. An aura none of them describes would change nothing in
    // the runtime.
    for effect in &prepared.effects {
        let Effect::ExternalCooldown { aura: label, .. } = effect else {
            continue;
        };
        let described = prepared.effects.iter().any(|other| match other {
            Effect::PowerInfusion { aura, .. } | Effect::InnervateRegen { aura, .. } => {
                aura == label
            }
            Effect::StatAuras { auras, .. } => auras.contains(label),
            _ => false,
        });
        if !described {
            reasons.push(Refusal::new(
                "aura_listener_unclaimed",
                format!("player aura {label:?} is cast by an external cooldown without an effect"),
            ));
        }
    }

    // A simulated pet needs a class effect that runs it, which claims the pet by its label.
    for pet in &prepared.pets {
        let claimant = prepared.effects.iter().find(|effect| {
            claims(effect)
                .iter()
                .any(|(u, label)| *u == "pet unit" && *label == pet.label)
        });
        match claimant {
            Some(effect) => {
                required.insert(effect.kind());
            }
            None => reasons.push(Refusal::new(
                "pet_unsupported",
                format!("pet {:?} has no behavior", pet.label),
            )),
        }
        // Go passes the owner's stat changes to a dynamic pet at its next heartbeat; the
        // runtime follows the stats it tracks. Every owner stat change is a stat aura's.
        let changed: Vec<&String> = prepared
            .effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::StatAuras { changed, .. } => Some(changed),
                _ => None,
            })
            .flatten()
            .collect();
        if pet.dynamic_stats {
            for term in &pet.inheritance {
                let tracked = crate::core::fight::pet::power_index(&term.owner).is_some();
                if changed.contains(&&term.owner) && !tracked {
                    reasons.push(Refusal::new(
                        "pet_unsupported",
                        format!(
                            "pet {:?} inherits changes of {} as {}, which is unsupported",
                            pet.label, term.owner, term.pet
                        ),
                    ));
                }
            }
        }
    }
    if let Some(enemy) = &prepared.enemy {
        reasons.extend(coded(
            "tanking_unsupported",
            tank_limits(prepared, enemy, &claims, rotation),
        ));
    }

    if let Some(rotation) = rotation {
        // The groups' actions the references run are read as items of the priority list.
        let flattened = match flatten_groups(prepared, rotation) {
            Ok(flattened) => flattened,
            Err(reason) => {
                reasons.push(Refusal::new("rotation_unsupported", reason));
                Rotation {
                    groups: Vec::new(),
                    ..rotation.clone()
                }
            }
        };
        let rotation = &flattened;
        // The values the rotation reads: the priority list's, and those of a prepull strict
        // sequence, whose steps the rotation runs once it takes control.
        let mut read = flattened.clone();
        read.priority_list
            .extend(
                rotation
                    .prepull
                    .iter()
                    .filter_map(|prepull| match &prepull.action {
                        action @ Action::StrictSequence(_) => Some(crate::rotation::Item {
                            position: prepull.position,
                            condition: None,
                            action: action.clone(),
                        }),
                        _ => None,
                    }),
            );
        reasons.extend(coded(
            "aura_condition_unsupported",
            aura_refresh_conditions(prepared, &read),
        ));
        reasons.extend(coded(
            "rotation_unsupported",
            rotation_read_limits(prepared),
        ));
        reasons.extend(coded(
            "rotation_unsupported",
            dot_increase_limits(prepared, &read),
        ));
        reasons.extend(coded(
            "several_targets_unsupported",
            change_target_limits(prepared, rotation),
        ));
        if player.class != "ClassShaman" {
            for item in &read.priority_list {
                let mut totems = false;
                for read in item_values(item) {
                    read.visit(&mut |value| {
                        totems |= matches!(value, Value::TotemRemainingTime { .. })
                    });
                }
                if totems {
                    reasons.push(Refusal::new(
                        "rotation_unsupported",
                        format!(
                            "rotation item {}: totemRemainingTime needs a Shaman",
                            item.position
                        ),
                    ));
                }
            }
        }
        reasons.extend(coded(
            "resource_unsupported",
            rage_without_bar(prepared, &read),
        ));
        reasons.extend(coded(
            "rotation_unsupported",
            unsummoned_pet_mana(prepared, &read),
        ));
        let mut reachable = Vec::new();
        let mut registered_prepull = 0;
        // The set the prepull's last item swap goes to: Go swaps back to Main before the pull
        // when it is another.
        let mut last_prepull_swap: Option<bool> = None;
        let mut prepull_moves = false;
        let mut rotation_moves = false;
        let mut prepull_sequences = 0;
        // Prepull parsing accepts casts, aura activations and moves.
        for prepull in &rotation.prepull {
            if prepull_pruned(prepared, prepull) {
                continue;
            }
            // Go registers every move: its range is read when it runs.
            if let Action::Move(_) | Action::MoveDuration(_) | Action::PrepullGroupMove(_) =
                &prepull.action
            {
                registered_prepull += 1;
                prepull_moves = true;
            }
            // Go newActionCastSpell: a target that names no unit builds no action. The runtime
            // casts a prepull action on the first target.
            if let Action::CastSpell { spell: id, target } = &prepull.action {
                match target.resolve(prepared.encounter.target_count.max(1) as usize) {
                    None => {}
                    Some(unit) => {
                        if let Some(spell) = rotation_cast_spell(prepared, id) {
                            registered_prepull += 1;
                            reachable.push(spell);
                            if unit != Unit::Target(0) {
                                reasons.push(Refusal::new(
                                    "several_targets_unsupported",
                                    format!(
                                        "prepull action {}: castSpell of {id} at a unit other \
                                         than the first target is unsupported",
                                        prepull.position
                                    ),
                                ));
                            }
                        }
                    }
                }
            }
            // Go GetAPLAura on the player: a known aura registers the action, one that stacks
            // for stacks, and GetAPLICDAura one with an ICD.
            let known = |id: &ActionId, stacks: bool| {
                player.auras.iter().any(|aura| {
                    aura.action_id.as_ref() == Some(id) && (!stacks || aura.max_stacks != 0)
                })
            };
            let targets = prepared.encounter.target_count.max(1) as usize;
            let registers = match &prepull.action {
                Action::ActivateAura(id) | Action::CancelAura(id) => known(id, false),
                Action::ActivateAuraWithStacks { id, .. } => known(id, true),
                Action::TriggerIcd(id) => find_icd_aura(prepared, Unit::Player, id).is_some(),
                Action::CastAtPlayer(id) => {
                    let spell = rotation_cast_spell(prepared, id);
                    reachable.extend(spell);
                    spell.is_some()
                }
                // Go `newActionChannelSpell`: none for a target that names no unit, or with an
                // interrupt condition for a spell that does not channel.
                Action::ChannelSpell {
                    spell: id,
                    target,
                    interrupt_if,
                    ..
                } => {
                    let spell = rotation_spell(prepared, id).filter(|spell| {
                        interrupt_if.is_none() || spell.has_flag("SpellFlagChanneled")
                    });
                    if target.resolve(targets).is_none() {
                        false
                    } else if !matches!(target.resolve(targets), Some(Unit::Target(0))) {
                        reasons.push(Refusal::new(
                            "several_targets_unsupported",
                            format!(
                                "prepull action {}: channelSpell of {id} at a unit other than the \
                                 first target is unsupported",
                                prepull.position
                            ),
                        ));
                        spell.is_some()
                    } else {
                        reachable.extend(spell);
                        spell.is_some()
                    }
                }
                Action::ItemSwap(to_swapped) if has_item_swap(prepared) => {
                    last_prepull_swap = Some(*to_swapped);
                    true
                }
                Action::CastAllStatBuffCooldowns(stats) => {
                    reachable.extend(cast_all_spells(prepared, stats));
                    true
                }
                // Go `newActionStrictSequence`: built only when every step is, and its Execute
                // takes control of the rotation before the pull. The runtime follows steps at the
                // first target, whose conditions the value checks read with the priority list's,
                // and one such sequence.
                Action::StrictSequence(steps) => {
                    let spells: Vec<_> = steps
                        .iter()
                        .filter(|step| !step.change_target)
                        .map(|step| {
                            step.target
                                .resolve(targets)
                                .and_then(|_| rotation_cast_spell(prepared, &step.spell))
                        })
                        .collect();
                    if spells.iter().any(Option::is_none) {
                        false
                    } else {
                        prepull_sequences += 1;
                        let unsupported = steps.iter().any(|step| {
                            step.change_target
                                || step.target.resolve(targets) != Some(Unit::Target(0))
                        });
                        if unsupported || prepull_sequences > 1 {
                            reasons.push(Refusal::new(
                                "prepull_unsupported",
                                format!(
                                    "prepull action {}: a strictSequence at another unit or \
                                     after another is unsupported",
                                    prepull.position
                                ),
                            ));
                        }
                        reachable.extend(spells.into_iter().flatten());
                        true
                    }
                }
                Action::ChangeTarget(unit) => unit.resolve(targets).is_some(),
                // Go `newActionActivateAllStatBuffProcAuras`: none without a proc buff of the
                // stats in the set, or for Swap1 without an item swap.
                Action::ActivateAllStatBuffProcAuras { stats, swap1 } => {
                    let procs = crate::rotation::StatProcs {
                        stats: stats.clone(),
                        min_icd: 0,
                    };
                    (!*swap1 || has_item_swap(prepared))
                        && coverage_item_procs(prepared, &procs)
                            .iter()
                            .any(|(_, swapped)| swapped == swap1)
                }
                _ => false,
            };
            if registers {
                registered_prepull += 1;
            }
        }
        if last_prepull_swap == Some(true) {
            registered_prepull += 1;
        }
        if registered_prepull != player.prepull_actions {
            reasons.push(Refusal::new(
                "prepull_unsupported",
                format!(
                    "Go registered {} prepull actions and the rotation {registered_prepull}; \
                     prepull actions outside the rotation are unsupported",
                    player.prepull_actions
                ),
            ));
        }
        let unreachable = unreachable_items(prepared, rotation);
        for item in &rotation.priority_list {
            if unreachable.contains(&item.position) {
                continue;
            }
            match &item.action {
                // Go registers every move: its range is read when it runs.
                Action::Move(_) | Action::MoveDuration(_) => rotation_moves = true,
                Action::AutocastOtherCooldowns => {
                    for cooldown in &player.major_cooldowns {
                        // A survival cooldown without timings waits for a health threshold Go
                        // never reaches at zero, so the autocast never casts it. Above zero
                        // it fires once health falls to it, also after its timings run out,
                        // which the runtime follows for class survival cooldowns only.
                        let survival = cooldown.kind.iter().any(|kind| kind == "survival");
                        let threshold = player.hp_percent_for_defensives != 0.0;
                        if survival && cooldown.timings_ns.is_empty() && !threshold {
                            continue;
                        }
                        // A survival cooldown the runtime implements, by its class, race,
                        // item or consumable effect; what its aura changes on the target's
                        // swings the tank limits check.
                        let known = cooldown_spell(prepared, cooldown).is_some_and(|spell| {
                            spell_capability(spell, prepared, gate).is_some_and(|kind| {
                                prepared.effects.iter().any(|effect| effect.kind() == kind)
                            })
                        });
                        if survival && threshold && !known {
                            reasons.push(Refusal::new(
                                "cooldown_unsupported",
                                format!(
                                    "survival cooldown {} with a defensive health threshold \
                                     is unsupported",
                                    cooldown.action_id.spell_id
                                ),
                            ));
                            continue;
                        }
                        reachable.extend(cooldown_spell(prepared, cooldown));
                    }
                }
                Action::CastAllStatBuffCooldowns(stats) => {
                    reachable.extend(cast_all_spells(prepared, stats));
                }
                // A Warlock's assigned curse reaches its curse spell.
                Action::CastAssignedCurse(target)
                    if target
                        .resolve(prepared.encounter.target_count.max(1) as usize)
                        .is_some() =>
                {
                    if let Some((_, id)) = assigned_curse(prepared) {
                        reachable.extend(rotation_spell(prepared, &id));
                    }
                }
                // Go builds no cast for a target that names no unit, so it reaches no spell.
                Action::CastSpell { target, .. } | Action::ChannelSpell { target, .. }
                    if target
                        .resolve(prepared.encounter.target_count.max(1) as usize)
                        .is_none() => {}
                action => {
                    for id in
                        action.reachable_spells(prepared.encounter.target_count.max(1) as usize)
                    {
                        reachable.extend(rotation_cast_spell(prepared, id));
                    }
                }
            }
        }
        if prepull_moves {
            reasons.extend(coded(
                "prepull_unsupported",
                player_movement_limits(prepared, true),
            ));
        }
        if rotation_moves {
            reasons.extend(coded(
                "movement_unsupported",
                player_movement_limits(prepared, false),
            ));
            reasons.extend(coded(
                "movement_unsupported",
                moves_without_values(prepared, rotation),
            ));
        }
        reasons.extend(coded("class_limit", (gate.limits)(prepared, &reachable)));
        reasons.extend(coded(
            "several_targets_unsupported",
            other_target_casts(prepared, gate, rotation, &unreachable),
        ));
        reasons.extend(coded(
            "several_targets_unsupported",
            several_target_limits(prepared, gate, &reachable),
        ));
        reasons.extend(coded("proc_unsupported", undirected_procs(prepared)));
        reasons.extend(coded("effect_unimplemented", shared_dot_auras(prepared)));
        reasons.extend(coded(
            "effect_unimplemented",
            unholy_aura_regeneration(prepared),
        ));
        reasons.extend(coded(
            "stat_change_unsupported",
            fixed_stat_changes(prepared),
        ));
        reasons.extend(coded(
            "stat_change_unsupported",
            fixed_stat_aura_changes(prepared),
        ));
        reasons.extend(coded(
            "stat_change_unsupported",
            uninherited_stat_changes(prepared),
        ));
        let mut unknown = BTreeSet::new();
        // Sorted by text, as the reasons always were, each with its code.
        let mut limited: BTreeSet<(String, &'static str)> = BTreeSet::new();
        for spell in reachable {
            let id = spell.action_id.clone().unwrap_or_default().to_string();
            match spell_capability(spell, prepared, gate) {
                Some(kind) if prepared.effects.iter().any(|effect| effect.kind() == kind) => {
                    required.insert(kind);
                }
                _ => {
                    unknown.insert(id.clone());
                }
            }
            // Go newHardcastAction: a tank's hardcast drops its avoidance, whose rolls the exporter
            // reads. A channel without a cast time sets no hardcast, so it keeps its avoidance.
            // Pushback of a channel with a cast time is not modeled. A cast with the pushback
            // flag is pushed back by every damaging hit, which needs the trigger the exporter
            // reads and a chance of one or none: no other chance has been compared with Go.
            if let Some(enemy) = &prepared.enemy {
                if spell.default_cast.cast_time_ns > 0 {
                    let pushback_chance = prepared.effects.iter().find_map(|effect| match effect {
                        Effect::PushbackTrigger { chance, .. } => Some(*chance),
                        _ => None,
                    });
                    let pushes_back = spell.has_flag("SpellFlagPushback");
                    if spell.has_flag("SpellFlagChanneled")
                        || enemy.reduced_avoidance_rolls.is_empty()
                        || (pushes_back && pushback_chance.is_none())
                    {
                        limited.insert((
                            format!(
                                "rotation reaches {id}, a hardcast while the target swings at \
                                 the player"
                            ),
                            "tanking_unsupported",
                        ));
                    } else if let Some(chance) = pushback_chance {
                        let rolled = chance - spell.pushback_resist;
                        if pushes_back && rolled > 0.0 && rolled < 1.0 {
                            limited.insert((
                                format!(
                                    "rotation reaches {id}, a hardcast the target's swings push \
                                     back with a chance that needs a roll"
                                ),
                                "tanking_unsupported",
                            ));
                        }
                    }
                }
            }
            for limit in runtime_limits(spell, (gate.spell)(spell).is_some()) {
                limited.insert((
                    format!("rotation reaches {id}, which uses unsupported {limit}"),
                    "spell_unsupported",
                ));
            }
        }
        reasons.extend(unknown.into_iter().map(|id| {
            Refusal::new(
                "unknown_spell",
                format!("rotation reaches {id} without a known behavior"),
            )
        }));
        reasons.extend(
            limited
                .into_iter()
                .map(|(reason, code)| Refusal::new(code, reason)),
        );
    } else if player.prepull_actions != 0 {
        reasons.push(Refusal::new(
            "prepull_unsupported",
            "prepull actions outside the rotation are unsupported",
        ));
    }

    let implemented = implemented_effects();
    let missing: Vec<Refusal> = required
        .into_iter()
        .filter(|kind| !implemented.contains(kind))
        .map(|kind| {
            Refusal::new(
                "effect_unimplemented",
                format!("effect {kind} is not implemented"),
            )
        })
        .collect();
    let mut all = missing;
    all.extend(reasons);
    all
}

/// Go reaches the targets past the first only through its area and cleave helpers, its loops
/// over the encounter's targets and the rotation's target choices. The exporter refuses the
/// shared ones Rust does not simulate on every target, and each class names its own; a class
/// that has not been checked against several targets is refused.
fn several_target_limits(
    prepared: &PreparedV2,
    gate: &ClassGate,
    reachable: &[&Spell],
) -> Vec<String> {
    let count = prepared.encounter.target_count;
    if count < 2 {
        return Vec::new();
    }
    // Go has every copy of the boss swing at a tank, each on its own timer, and Rust runs each
    // copy's swing. What the tank's class does when it takes a hit must answer the copy that
    // swung, which only the classes that have been checked against it do.
    let mut reasons = Vec::new();
    if prepared.enemy.is_some() && !gate.tanks_several_targets {
        reasons.push(format!(
            "{count} targets: every copy of the boss swings at the tank, which is unsupported \
             for {} yet",
            gate.class
        ));
    }
    match gate.several_targets {
        Some(limits) => reasons.extend(limits(prepared, reachable)),
        None => reasons.push(format!(
            "{count} targets: several targets are not supported for {} yet",
            gate.class
        )),
    }
    reasons
}

/// Whether a cast of the spell at the player is one the runtime follows: a helpful spell, which
/// `castFriendlySpell` casts there, or a class spell that acts on the caster alone.
fn casts_at_player(gate: &ClassGate, spell: &Spell) -> bool {
    spell.has_flag("SpellFlagHelpful")
        || spell
            .class_spell
            .as_deref()
            .is_some_and(|name| gate.self_spells.contains(&name))
}

/// Go's `castSpell` names the unit it casts on. Rust casts a spell on a target past the first
/// when its effects, its debuffs and its dot are all of the target they are given, which the
/// class checks; a channel's dot aura stays on the first target, so a channel with a dot on
/// its target is never cast elsewhere. A cast on the player is supported only as
/// `casts_at_player` allows.
fn other_target_casts(
    prepared: &PreparedV2,
    gate: &ClassGate,
    rotation: &Rotation,
    unreachable: &BTreeSet<usize>,
) -> Vec<String> {
    let targets = prepared.encounter.target_count.max(1) as usize;
    let mut reasons = BTreeSet::new();
    let mut spells = Vec::new();
    // A rotation that changes target casts at the current target, next or previous wherever
    // the current target has moved, so those casts count as casts at other targets.
    let dynamic = rotation_changes_target(rotation);
    let relative = |unit: &crate::rotation::UnitRef| {
        dynamic
            && matches!(
                unit,
                crate::rotation::UnitRef::CurrentTarget
                    | crate::rotation::UnitRef::NextTarget
                    | crate::rotation::UnitRef::PreviousTarget
            )
    };
    for item in &rotation.priority_list {
        // A step of a sequence is cast on its unit as a cast is, with the player only for a
        // helpful spell.
        if let Action::Sequence { steps, .. } | Action::StrictSequence(steps) = &item.action {
            if unreachable.contains(&item.position) {
                continue;
            }
            for step in steps.iter().filter(|step| !step.change_target) {
                let Some(spell) = rotation_spell(prepared, &step.spell) else {
                    continue;
                };
                match step.target.resolve(targets) {
                    Some(Unit::Target(0)) if relative(&step.target) => spells.push(spell),
                    None | Some(Unit::Target(0)) => {}
                    Some(Unit::Player) if casts_at_player(gate, spell) => {}
                    Some(Unit::Player) => {
                        reasons.insert(format!(
                            "rotation item {}: a step of a sequence casting {} on the player is \
                             unsupported",
                            item.position, step.spell
                        ));
                    }
                    Some(Unit::Target(_)) => spells.push(spell),
                }
            }
            continue;
        }
        // A channel runs on the current target only.
        if let Action::ChannelSpell {
            spell: id, target, ..
        } = &item.action
        {
            let resolved = target.resolve(targets);
            if (!matches!(resolved, None | Some(Unit::Target(0)))
                || resolved.is_some() && relative(target))
                && rotation_spell(prepared, id).is_some()
                && !unreachable.contains(&item.position)
            {
                reasons.insert(format!(
                    "rotation item {}: channelSpell of {id} on a unit other than the current \
                     target is unsupported",
                    item.position
                ));
            }
            continue;
        }
        let Action::CastSpell { spell: id, target } = &item.action else {
            continue;
        };
        let Some(unit) = target.resolve(targets) else {
            continue;
        };
        let position = match unit {
            Unit::Target(0) if !relative(target) => continue,
            Unit::Target(position) => position,
            Unit::Player => 0,
        };
        if unreachable.contains(&item.position) {
            continue;
        }
        let Some(spell) = rotation_spell(prepared, id) else {
            continue;
        };
        let place = if unit == Unit::Player {
            "the player".to_string()
        } else {
            format!("target {}", position + 1)
        };
        if unit == Unit::Player {
            // A helpful spell is cast on the player as `castFriendlySpell` casts it, which the
            // class checks with the rest of its spells. A harmful spell whose dot sits on its
            // target never casts there, as the game refuses it, where Go panics on the
            // player's missing dot. Any other spell on the player is unsupported.
            if !casts_at_player(gate, spell) && !harmful_target_dot(spell) {
                reasons.insert(format!(
                    "rotation item {}: castSpell of {id} on the player is unsupported",
                    item.position
                ));
            }
        } else if spell.has_flag("SpellFlagChanneled")
            && spell.dot.as_ref().is_some_and(|dot| dot.unit != "self")
        {
            reasons.insert(format!(
                "rotation item {}: castSpell of {id} on {place}: a channel keeps its dot on the \
                 first target",
                item.position
            ));
        } else {
            spells.push(spell);
        }
    }
    if !spells.is_empty() {
        match gate.other_target_casts {
            Some(limits) => reasons.extend(limits(prepared, &spells)),
            None => {
                reasons.insert(format!(
                    "castSpell on a target past the first is not supported for {} yet",
                    gate.class
                ));
            }
        }
    }
    reasons.into_iter().collect()
}

/// Whether a spell is harmful and keeps its own dot on the targets, which the runtime never
/// casts at the player (`Fight::harmful_target_dot`).
fn harmful_target_dot(spell: &Spell) -> bool {
    !spell.has_flag("SpellFlagHelpful") && spell.dot.as_ref().is_some_and(|dot| dot.unit != "self")
}

/// The reachable spells of the given class spell names, which reach a target past the first
/// in Go: a reason for each, naming what it does there.
pub(crate) fn spells_reaching_other_targets(
    reachable: &[&Spell],
    spells: &[(&str, &str)],
) -> Vec<String> {
    let mut reasons = BTreeSet::new();
    for spell in reachable {
        let Some(class_spell) = spell.class_spell.as_deref() else {
            continue;
        };
        if let Some((_, what)) = spells.iter().find(|(name, _)| *name == class_spell) {
            let id = spell.action_id.clone().unwrap_or_default();
            reasons.insert(format!(
                "rotation reaches {id}, which {what} in a fight against several targets"
            ));
        }
    }
    reasons.into_iter().collect()
}

/// Go gives `currentRage` no value on a unit without a rage bar, which drops the term; the
/// runtime reads the bar, so such a rotation is unsupported.
fn rage_without_bar(prepared: &PreparedV2, rotation: &Rotation) -> Vec<String> {
    let has_bar = prepared
        .effects
        .iter()
        .any(|effect| matches!(effect, Effect::RageBar { .. }));
    if has_bar {
        return Vec::new();
    }
    let mut reasons = Vec::new();
    for item in &rotation.priority_list {
        let mut uses = false;
        for read in item_values(item) {
            read.visit(&mut |value| uses |= matches!(value, Value::CurrentRage));
        }
        if uses {
            reasons.push(format!(
                "rotation item {} reads rage, which the player lacks",
                item.position
            ));
        }
    }
    reasons
}

/// What a move of the player needs that the runtime does not follow: the speed the exporter
/// read, which the auras that change it then multiply as Go does.
fn player_movement_limits(prepared: &PreparedV2, prepull: bool) -> Vec<String> {
    let kind = if prepull { "a prepull move" } else { "a move" };
    let described = prepared
        .effects
        .iter()
        .any(|effect| matches!(effect, Effect::PlayerMovement { .. }));
    if described {
        Vec::new()
    } else {
        vec![format!("{kind} has no exported movement speed")]
    }
}

/// The rotation as the rest of the gate reads it: the groups bound, and the actions of each
/// group a reference runs following the priority list as items of their own, each under the
/// condition of the reference that runs it. The items of the groups nothing references never
/// run, so they follow nothing. Go's references are bound as the rotation compiles, from the
/// same conditions.
fn flatten_groups(prepared: &PreparedV2, rotation: &Rotation) -> Result<Rotation, String> {
    let aura = |id: &ActionId| find_aura(prepared, id);
    let target_aura = |_: usize, id: &ActionId| find_unit_aura(&prepared.target.auras, id);
    let spell = |id: &ActionId| rotation_spell_index(prepared, id);
    let dot = |id: &ActionId, unit: Unit| rotation_dot(prepared, id, unit);
    let dot_base_duration = |id: &ActionId| rotation_dot_base_duration(prepared, id);
    let pet_auras = crate::core::fight::pet::pet_agent_auras(prepared);
    let pet_aura_known =
        |pet: usize, id: &ActionId| pet_auras.get(pet).is_some_and(|auras| auras.contains(id));
    let curse = assigned_curse(prepared);
    let lookup = Lookup {
        aura: &aura,
        target_aura: &target_aura,
        targets: prepared.encounter.target_count.max(1) as usize,
        spell: &spell,
        dot: &dot,
        dot_base_duration: &dot_base_duration,
        pet_aura_known: &pet_aura_known,
        pet_aura: &|pet: usize, id: &ActionId| simulated_pet_aura(prepared, pet, id),
        simulated_pet_with_mana: &|pet: usize| simulated_pet_with_mana(prepared, pet),
        unsummoned_pet_with_mana: &|pet: usize| unsummoned_pet_with_mana(prepared, pet),
        unsummoned_pet_max_mana: &|pet: usize| unsummoned_pet_max_mana(prepared, pet),
        item_swap: has_item_swap(prepared),
        bars: rotation_bars(prepared),
        icd_aura: &|unit: Unit, id: &ActionId| find_icd_aura(prepared, unit, id),
        multidot: &|id: &ActionId| rotation_multidot(prepared, id),
        assigned_curse: curse.as_ref().map(|(name, id)| (name.as_str(), id)),
        tanked: prepared.enemy.is_some(),
        reads: prepared.player.rotation_reads.as_ref(),
        exact_spell: &|id: &ActionId| {
            prepared
                .player
                .spells
                .iter()
                .position(|spell| spell.action_id.as_ref() == Some(id))
        },
        item_procs: &|procs: &crate::rotation::StatProcs| coverage_item_procs(prepared, procs),
        expected_tick: &|id: &ActionId| coverage_expected_tick(prepared, id),
        aura_by_label: &|label: &str| {
            prepared
                .player
                .auras
                .iter()
                .find(|aura| aura.label == label)
                .and_then(|aura| aura.action_id.clone())
        },
        self_buff: &|spell: usize| {
            let id = prepared.player.spells[spell].action_id.as_ref()?;
            prepared
                .player
                .rotation_reads
                .as_ref()?
                .self_buffs
                .iter()
                .any(|buff| &buff.spell == id)
                .then(|| id.clone())
        },
    };
    let live = |index: usize| {
        compile_condition(rotation.priority_list[index].condition.as_ref(), &lookup)
            != crate::rotation::CompiledCondition::Pruned
    };
    let (settled, binding) = crate::rotation::bind(rotation, &live)?;
    let mut flat = Rotation {
        groups: Vec::new(),
        ..settled
    };
    let mut position = flat
        .priority_list
        .iter()
        .map(|item| item.position)
        .max()
        .unwrap_or(0);
    // The items of the instance a reference runs, and of the instances their own references
    // run.
    fn append(
        binding: &crate::rotation::Binding,
        instance: usize,
        condition: Option<Value>,
        depth: usize,
        position: &mut usize,
        flat: &mut Vec<crate::rotation::Item>,
    ) -> Result<(), String> {
        if depth > 32 {
            return Err("a group holds a reference to itself".into());
        }
        for (index, item) in binding.instances[instance].items.iter().enumerate() {
            let own = match (&condition, &item.condition) {
                (Some(outer), Some(inner)) => Some(Value::And(vec![outer.clone(), inner.clone()])),
                (Some(only), None) | (None, Some(only)) => Some(only.clone()),
                (None, None) => None,
            };
            *position += 1;
            flat.push(crate::rotation::Item {
                position: *position,
                condition: own.clone(),
                action: item.action.clone(),
            });
            let reference = crate::rotation::Reference::Nested {
                instance,
                item: index,
            };
            if let Some(nested) = binding.instance(reference) {
                append(binding, nested, own, depth + 1, position, flat)?;
            }
        }
        Ok(())
    }
    for (index, item) in rotation.priority_list.iter().enumerate() {
        if let Some(instance) = binding.instance(crate::rotation::Reference::Top(index)) {
            append(
                &binding,
                instance,
                item.condition.clone(),
                0,
                &mut position,
                &mut flat.priority_list,
            )?;
        }
    }
    // A schedule's inner action runs as an item of its own whose condition is both.
    let mut index = 0;
    while index < flat.priority_list.len() {
        if let Action::Schedule { inner, .. } = &flat.priority_list[index].action {
            let own = match (&flat.priority_list[index].condition, &inner.condition) {
                (Some(outer), Some(inner)) => Some(Value::And(vec![outer.clone(), inner.clone()])),
                (Some(only), None) | (None, Some(only)) => Some(only.clone()),
                (None, None) => None,
            };
            position += 1;
            let item = crate::rotation::Item {
                position,
                condition: own,
                action: inner.action.clone(),
            };
            flat.priority_list.push(item);
        }
        index += 1;
    }
    Ok(flat)
}

/// Go builds a move without a value when the value it names gives none, and panics where it
/// reads the range or the duration, which Rust cannot reproduce.
fn moves_without_values(prepared: &PreparedV2, rotation: &Rotation) -> Vec<String> {
    let aura = |id: &ActionId| find_aura(prepared, id);
    let target_aura = |_: usize, id: &ActionId| find_unit_aura(&prepared.target.auras, id);
    let spell = |id: &ActionId| rotation_spell_index(prepared, id);
    let dot = |id: &ActionId, unit: Unit| rotation_dot(prepared, id, unit);
    let dot_base_duration = |id: &ActionId| rotation_dot_base_duration(prepared, id);
    let pet_auras = crate::core::fight::pet::pet_agent_auras(prepared);
    let pet_aura_known =
        |pet: usize, id: &ActionId| pet_auras.get(pet).is_some_and(|auras| auras.contains(id));
    let curse = assigned_curse(prepared);
    let lookup = Lookup {
        aura: &aura,
        target_aura: &target_aura,
        targets: prepared.encounter.target_count.max(1) as usize,
        spell: &spell,
        dot: &dot,
        dot_base_duration: &dot_base_duration,
        pet_aura_known: &pet_aura_known,
        pet_aura: &|pet: usize, id: &ActionId| simulated_pet_aura(prepared, pet, id),
        simulated_pet_with_mana: &|pet: usize| simulated_pet_with_mana(prepared, pet),
        unsummoned_pet_with_mana: &|pet: usize| unsummoned_pet_with_mana(prepared, pet),
        unsummoned_pet_max_mana: &|pet: usize| unsummoned_pet_max_mana(prepared, pet),
        item_swap: has_item_swap(prepared),
        bars: rotation_bars(prepared),
        icd_aura: &|unit: Unit, id: &ActionId| find_icd_aura(prepared, unit, id),
        multidot: &|id: &ActionId| rotation_multidot(prepared, id),
        assigned_curse: curse.as_ref().map(|(name, id)| (name.as_str(), id)),
        tanked: prepared.enemy.is_some(),
        reads: prepared.player.rotation_reads.as_ref(),
        exact_spell: &|id: &ActionId| {
            prepared
                .player
                .spells
                .iter()
                .position(|spell| spell.action_id.as_ref() == Some(id))
        },
        item_procs: &|procs: &crate::rotation::StatProcs| coverage_item_procs(prepared, procs),
        expected_tick: &|id: &ActionId| coverage_expected_tick(prepared, id),
        aura_by_label: &|label: &str| {
            prepared
                .player
                .auras
                .iter()
                .find(|aura| aura.label == label)
                .and_then(|aura| aura.action_id.clone())
        },
        self_buff: &|spell: usize| {
            let id = prepared.player.spells[spell].action_id.as_ref()?;
            prepared
                .player
                .rotation_reads
                .as_ref()?
                .self_buffs
                .iter()
                .any(|buff| &buff.spell == id)
                .then(|| id.clone())
        },
    };
    rotation
        .priority_list
        .iter()
        .filter_map(|item| match &item.action {
            Action::Move(value) | Action::MoveDuration(value)
                if crate::rotation::compile_raw_value(value, &lookup).is_none() =>
            {
                Some(format!(
                    "rotation item {}: a move whose value gives none panics in Go",
                    item.position
                ))
            }
            _ => None,
        })
        .collect()
}

/// Whether Go prunes a prepull action for a constant false condition, which is the only use
/// Go makes of it.
fn prepull_pruned(prepared: &PreparedV2, prepull: &crate::rotation::Prepull) -> bool {
    let aura = |id: &ActionId| find_aura(prepared, id);
    let target_aura = |_: usize, id: &ActionId| find_unit_aura(&prepared.target.auras, id);
    let spell = |id: &ActionId| rotation_spell_index(prepared, id);
    let dot = |id: &ActionId, unit: Unit| rotation_dot(prepared, id, unit);
    let dot_base_duration = |id: &ActionId| rotation_dot_base_duration(prepared, id);
    let pet_auras = crate::core::fight::pet::pet_agent_auras(prepared);
    let pet_aura_known =
        |pet: usize, id: &ActionId| pet_auras.get(pet).is_some_and(|auras| auras.contains(id));
    let curse = assigned_curse(prepared);
    let lookup = Lookup {
        aura: &aura,
        target_aura: &target_aura,
        targets: prepared.encounter.target_count.max(1) as usize,
        spell: &spell,
        dot: &dot,
        dot_base_duration: &dot_base_duration,
        pet_aura_known: &pet_aura_known,
        pet_aura: &|pet: usize, id: &ActionId| simulated_pet_aura(prepared, pet, id),
        simulated_pet_with_mana: &|pet: usize| simulated_pet_with_mana(prepared, pet),
        unsummoned_pet_with_mana: &|pet: usize| unsummoned_pet_with_mana(prepared, pet),
        unsummoned_pet_max_mana: &|pet: usize| unsummoned_pet_max_mana(prepared, pet),
        item_swap: has_item_swap(prepared),
        bars: rotation_bars(prepared),
        icd_aura: &|unit: Unit, id: &ActionId| find_icd_aura(prepared, unit, id),
        multidot: &|id: &ActionId| rotation_multidot(prepared, id),
        assigned_curse: curse.as_ref().map(|(name, id)| (name.as_str(), id)),
        tanked: prepared.enemy.is_some(),
        reads: prepared.player.rotation_reads.as_ref(),
        exact_spell: &|id: &ActionId| {
            prepared
                .player
                .spells
                .iter()
                .position(|spell| spell.action_id.as_ref() == Some(id))
        },
        item_procs: &|procs: &crate::rotation::StatProcs| coverage_item_procs(prepared, procs),
        expected_tick: &|id: &ActionId| coverage_expected_tick(prepared, id),
        aura_by_label: &|label: &str| {
            prepared
                .player
                .auras
                .iter()
                .find(|aura| aura.label == label)
                .and_then(|aura| aura.action_id.clone())
        },
        self_buff: &|spell: usize| {
            let id = prepared.player.spells[spell].action_id.as_ref()?;
            prepared
                .player
                .rotation_reads
                .as_ref()?
                .self_buffs
                .iter()
                .any(|buff| &buff.spell == id)
                .then(|| id.clone())
        },
    };
    compile_condition(prepull.condition.as_ref(), &lookup)
        == crate::rotation::CompiledCondition::Pruned
}

/// Categories whose members change their bid while the fight runs without a stack count, such
/// as Thunder Clap's on every cast, which the exported bid does not follow.
const MOVING_BID_CATEGORIES: &[&str] = &["AtkSpdReduction"];

/// Whether the runtime reads the aura's exclusive effect at a position as Go does. A single aura
/// category an `exclusive_category` effect of the unit describes is enforced with its members'
/// live bids, stacking ones included; any other single aura category is enforced from the
/// exported memberships, which holds while no member's bid follows its stacks; a category that
/// holds several auras refuses nothing, so the runtime tracks it, which holds while no member's
/// bid follows its stacks or moves some other way.
fn live_category(
    prepared: &PreparedV2,
    unit: &str,
    aura: &crate::contracts::prepared_v2::Aura,
    position: usize,
) -> bool {
    if aura.exclusive_memberships.len() != aura.exclusive_effects as usize {
        return false;
    }
    let Some(membership) = aura.exclusive_memberships.get(position) else {
        return false;
    };
    let described = prepared.effects.iter().any(|effect| {
        matches!(effect, Effect::ExclusiveCategory { unit: u, category, members, .. }
            if u == unit
                && *category == membership.category
                && members.iter().any(|member| member.aura == aura.label))
    });
    if described {
        return true;
    }
    let auras = if unit == "target" {
        &prepared.target.auras
    } else {
        &prepared.player.auras
    };
    // A stacking aura that bids nothing before its first stack is one whose bid follows its
    // stacks, as spelldata's debuffs do, which only a category a class describes carries.
    let stacking_bid = auras.iter().any(|member| {
        member.max_stacks != 0
            && member
                .exclusive_memberships
                .iter()
                .any(|other| other.category == membership.category && other.priority == 0.0)
    });
    !stacking_bid
        && (membership.single_aura
            || !MOVING_BID_CATEGORIES.contains(&membership.category.as_str()))
}

/// The values an item reads: its condition, then those of its action, as a channel's interrupt
/// condition or a sequence's step conditions.
fn item_values(item: &crate::rotation::Item) -> impl Iterator<Item = &Value> {
    item.condition.iter().chain(item.action.values())
}

/// Go `ShouldRefreshExclusiveEffects` depends on the other effects of each exclusive category,
/// which Rust reads from the runtime's categories, or from an exported reading: an aura holding
/// its category alone, or one another aura holds for good. Any other reading is unsupported. An
/// aura the unit lacks gives no value in Go, and a unit that does not exist no aura.
fn aura_refresh_conditions(prepared: &PreparedV2, rotation: &Rotation) -> Vec<String> {
    let mut reasons = Vec::new();
    let targets = prepared.encounter.target_count.max(1) as usize;
    // Go `newAPLAction` builds an action before its condition, and a cast of a spell the player
    // lacks builds nothing, so the condition of such an item is never read.
    let kept = rotation.priority_list.iter().filter(|item| {
        !matches!(&item.action, Action::CastSpell { spell, .. }
            if rotation_spell_index(prepared, spell).is_none())
    });
    let conditions = kept.flat_map(item_values);
    for condition in conditions {
        condition.visit(&mut |value| {
            let Value::AuraShouldRefresh {
                id, unit: reference, ..
            } = value
            else {
                return;
            };
            let (unit, auras) = match reference.resolve(targets) {
                Some(Unit::Player) => ("player", &prepared.player.auras),
                Some(Unit::Target(_)) => ("target", &prepared.target.auras),
                None => return,
            };
            let Some(aura) = auras.iter().find(|aura| aura.action_id.as_ref() == Some(id)) else {
                return;
            };
            let read = prepared.effects.iter().any(|effect| {
                matches!(effect, Effect::AuraShouldRefresh { unit: u, aura: label, modes }
                    if u == unit && *label == aura.label
                        && modes.iter().enumerate().all(|(position, mode)| {
                            mode == "own"
                                || mode == "never"
                                || live_category(prepared, unit, aura, position)
                        }))
            });
            if !read {
                reasons.push(format!(
                    "auraShouldRefresh on {unit} aura {:?} has no supported exclusive effect reading",
                    aura.label
                ));
            }
        });
    }
    reasons
}

fn find_aura(prepared: &PreparedV2, id: &ActionId) -> Option<FoundAura<ActionId>> {
    find_unit_aura(&prepared.player.auras, id)
}

/// sim/warlock/apl_values.go `GetAssignedCurse`: a Warlock's curse option, Agony when the
/// options leave it out, and the action of the spell it names. Only a Warlock has one.
pub(crate) fn assigned_curse(prepared: &PreparedV2) -> Option<(String, ActionId)> {
    if prepared.player.class != "ClassWarlock" {
        return None;
    }
    let option = prepared
        .player
        .class_options
        .get("curseOptions")
        .and_then(|value| value.as_str())
        .unwrap_or("Agony")
        .to_string();
    let class_spell = match option.as_str() {
        "Agony" => "bane_of_agony",
        "Doom" => "bane_of_doom",
        "Elements" => "curse_of_the_elements",
        "Recklessness" => "curse_of_recklessness",
        _ => return None,
    };
    let id = prepared
        .player
        .spells
        .iter()
        .find(|spell| spell.class_spell.as_deref() == Some(class_spell))?
        .action_id
        .clone()?;
    Some((option, id))
}

/// Go `APLActionChangeTarget` moves the player's current target. The runtime follows a change
/// in the priority list or a strict sequence; before the pull the current target stays the
/// first, so a change there to another unit is refused.
fn change_target_limits(prepared: &PreparedV2, rotation: &Rotation) -> Vec<String> {
    let targets = prepared.encounter.target_count.max(1) as usize;
    let mut reasons = Vec::new();
    for prepull in &rotation.prepull {
        if let Action::ChangeTarget(unit) = prepull.action.scheduled() {
            if !matches!(unit.resolve(targets), None | Some(Unit::Target(0))) {
                reasons.push(format!(
                    "prepull action {}: a change of target to a unit other than the first target is unsupported",
                    prepull.position
                ));
            }
        }
    }
    // A change to the player: Go's current target would be the player itself.
    for item in rotation
        .priority_list
        .iter()
        .chain(rotation.groups.iter().flat_map(|group| group.items.iter()))
    {
        let player = match item.action.scheduled() {
            Action::ChangeTarget(unit) => unit.resolve(targets) == Some(Unit::Player),
            Action::StrictSequence(steps) => steps.iter().any(|step| {
                step.change_target && step.target.resolve(targets) == Some(Unit::Player)
            }),
            _ => false,
        };
        if player {
            reasons.push(format!(
                "rotation item {}: a change of target to the player is unsupported",
                item.position
            ));
        }
    }
    reasons
}

/// Whether the rotation can move the player's current target off the first target: a change of
/// target in the priority list, a group or a strict sequence to a unit other than the current
/// target.
pub(crate) fn rotation_changes_target(rotation: &Rotation) -> bool {
    let moves = |unit: &crate::rotation::UnitRef| {
        !matches!(
            unit,
            crate::rotation::UnitRef::CurrentTarget | crate::rotation::UnitRef::Target(0)
        )
    };
    rotation
        .priority_list
        .iter()
        .chain(rotation.groups.iter().flat_map(|group| group.items.iter()))
        .any(|item| match item.action.scheduled() {
            Action::ChangeTarget(unit) => moves(unit),
            Action::StrictSequence(steps) => steps
                .iter()
                .any(|step| step.change_target && moves(&step.target)),
            _ => false,
        })
}

/// Go's dot increase values of Penance read its bolt's damage, which the runtime does not
/// compute, so they are refused rather than dropped.
fn dot_increase_limits(prepared: &PreparedV2, rotation: &Rotation) -> Vec<String> {
    let mut reasons = Vec::new();
    let mut visit = |value: &Value| {
        if let Value::DotPercentIncrease(dot) | Value::DotTickRatePercentIncrease(dot) = value {
            let penance = dot
                .spell
                .as_ref()
                .and_then(|id| rotation_spell_index(prepared, id))
                .and_then(|index| dot_owner(prepared, index))
                .is_some_and(|holder| {
                    prepared.player.spells[holder].class_spell.as_deref() == Some("penance")
                });
            let reason = "a dot increase value of Penance is unsupported".to_string();
            if penance && !reasons.contains(&reason) {
                reasons.push(reason);
            }
        }
    };
    for item in &rotation.priority_list {
        if let Some(condition) = &item.condition {
            condition.visit(&mut visit);
        }
        for value in item.action.values() {
            value.visit(&mut visit);
        }
    }
    reasons
}

/// The spell holding the dot of a spell whose `ExpectedTickDamage` the runtime computes.
fn coverage_expected_tick(prepared: &PreparedV2, id: &ActionId) -> Option<usize> {
    let index = rotation_spell_index(prepared, id)?;
    let holder = dot_owner(prepared, index)?;
    let spell = &prepared.player.spells[holder];
    let known = [
        "rake",
        "rip",
        "shadow_word_pain",
        "devouring_plague",
        "mind_flay",
        "starshards",
        "bane_of_agony",
        "flame_shock_dot",
    ];
    spell
        .class_spell
        .as_deref()
        .is_some_and(|name| known.contains(&name))
        .then_some(holder)
}

/// The spells a `castAllStatBuffCooldowns` of the stats casts, as the input names them.
fn cast_all_spells<'a>(prepared: &'a PreparedV2, stats: &[i32]) -> Vec<&'a Spell> {
    let Some(reads) = &prepared.player.rotation_reads else {
        return Vec::new();
    };
    reads
        .cast_cooldowns
        .iter()
        .filter(|entry| entry.stats == stats)
        .flat_map(|entry| entry.spells.iter())
        .filter_map(|id| {
            prepared
                .player
                .spells
                .iter()
                .find(|spell| spell.action_id.as_ref() == Some(id))
        })
        .collect()
}

/// The item proc buffs an item stat proc value matches, by their aura's action.
fn coverage_item_procs(
    prepared: &PreparedV2,
    procs: &crate::rotation::StatProcs,
) -> Vec<(ActionId, bool)> {
    let Some(reads) = &prepared.player.rotation_reads else {
        return Vec::new();
    };
    reads
        .item_proc_buffs
        .iter()
        .filter(|buff| buff.stats.iter().any(|stat| procs.stats.contains(stat)))
        .filter_map(|buff| {
            let aura = prepared
                .player
                .auras
                .iter()
                .find(|aura| aura.label == buff.aura)?;
            let long_enough = procs.min_icd <= 0
                || aura
                    .icd
                    .as_ref()
                    .is_some_and(|icd| icd.duration_ns > procs.min_icd);
            long_enough.then(|| (aura.action_id.clone().unwrap_or_default(), buff.swapped))
        })
        .collect()
}

/// What the rotation's values read that Rust does not run: a target spell with a cooldown, which
/// only a target AI starts, and a spell `multipleCdUsages` reads without a cooldown, whose
/// division by it Go leaves to the platform or whose timer Go reads without one.
fn rotation_read_limits(prepared: &PreparedV2) -> Vec<String> {
    let Some(reads) = &prepared.player.rotation_reads else {
        return Vec::new();
    };
    let mut reasons = Vec::new();
    for spell in reads
        .target_spells
        .iter()
        .filter(|spell| spell.has_cooldown)
    {
        reasons.push(format!(
            "a boss value reads target spell {}, which has a cooldown",
            spell.spell
        ));
    }
    for buff in reads.self_buffs.iter().filter(|buff| !buff.has_cooldown) {
        reasons.push(format!(
            "multipleCdUsages reads spell {} without a cooldown",
            buff.spell
        ));
    }
    // An item swap swaps a proc buff, or a cooldown, in and out, which the reads leave out.
    let stat_reads = !reads.item_proc_buffs.is_empty()
        || !reads.cooldown_buffs.is_empty()
        || !reads.cast_cooldowns.is_empty();
    if stat_reads && has_item_swap(prepared) {
        reasons.push("stat proc and stat buff cooldown values with an item swap".to_string());
    }
    reasons
}

/// Go `GetIcdAuraByID` on a unit's exported auras: the first with the action, or triggered by
/// it, that has an ICD.
fn find_icd_aura(prepared: &PreparedV2, unit: Unit, id: &ActionId) -> Option<ActionId> {
    let auras = match unit {
        Unit::Player => &prepared.player.auras,
        Unit::Target(_) => &prepared.target.auras,
    };
    auras
        .iter()
        .any(|aura| {
            aura.icd.is_some()
                && (aura.action_id.as_ref() == Some(id)
                    || aura.action_id_for_proc.as_ref() == Some(id))
        })
        .then(|| id.clone())
}

/// Go `GetAuraByID` on a unit's exported auras.
/// Go `GetAuraByID` on the player's pet at a position among its pets (as
/// [`crate::core::fight::pet::pet_agent_auras`] orders them) while the fight simulates it.
fn simulated_pet_aura(
    prepared: &PreparedV2,
    pet: usize,
    id: &ActionId,
) -> Option<FoundAura<ActionId>> {
    let mut pets: Vec<(i32, Option<&[crate::contracts::prepared_v2::Aura]>)> = prepared
        .pets
        .iter()
        .map(|pet| (pet.index, Some(pet.auras.as_slice())))
        .collect();
    for effect in &prepared.effects {
        if let Effect::InertPet { unit_index, .. } = effect {
            pets.push((*unit_index, None));
        }
    }
    pets.sort_by_key(|(index, _)| *index);
    find_unit_aura(pets.get(pet)?.1?, id)
}

/// The player's pet at a position among its pets, while the fight simulates it and it has a mana
/// bar: its place among the simulated pets, as the runtime numbers them.
fn simulated_pet_with_mana(prepared: &PreparedV2, pet: usize) -> Option<u8> {
    let mut pets: Vec<(i32, Option<usize>)> = prepared
        .pets
        .iter()
        .enumerate()
        .map(|(slot, pet)| (pet.index, Some(slot)))
        .collect();
    for effect in &prepared.effects {
        if let Effect::InertPet { unit_index, .. } = effect {
            pets.push((*unit_index, None));
        }
    }
    pets.sort_by_key(|(index, _)| *index);
    let slot = pets.get(pet)?.1?;
    (prepared.pets[slot].mana.max > 0.0).then(|| u8::try_from(slot).ok())?
}

/// Whether the player's pet at a position among its pets is one the fight never summons that
/// has a mana bar.
fn unsummoned_pet_with_mana(prepared: &PreparedV2, pet: usize) -> bool {
    let mut pets: Vec<(i32, Option<bool>)> =
        prepared.pets.iter().map(|pet| (pet.index, None)).collect();
    for effect in &prepared.effects {
        if let Effect::InertPet {
            unit_index,
            mana_bar,
            ..
        } = effect
        {
            pets.push((*unit_index, Some(*mana_bar)));
        }
    }
    pets.sort_by_key(|(index, _)| *index);
    pets.get(pet).and_then(|(_, mana_bar)| *mana_bar) == Some(true)
}

/// The full mana a pet the fight never summons states, by its position among the pets.
fn unsummoned_pet_max_mana(prepared: &PreparedV2, pet: usize) -> Option<f64> {
    let mut pets: Vec<(i32, Option<f64>)> =
        prepared.pets.iter().map(|pet| (pet.index, None)).collect();
    for effect in &prepared.effects {
        if let Effect::InertPet {
            unit_index,
            max_mana,
            ..
        } = effect
        {
            pets.push((*unit_index, *max_mana));
        }
    }
    pets.sort_by_key(|(index, _)| *index);
    pets.get(pet).and_then(|(_, max)| *max)
}

/// Rotation items that read the mana of a pet the fight never summons, whose mana Go reads off
/// a unit the runtime does not keep. Its mana percent is a full bar, which the runtime knows.
fn unsummoned_pet_mana(prepared: &PreparedV2, rotation: &Rotation) -> Vec<String> {
    let simulated = |pet: usize| {
        let mut pets: Vec<(i32, bool)> =
            prepared.pets.iter().map(|pet| (pet.index, true)).collect();
        for effect in &prepared.effects {
            if let Effect::InertPet { unit_index, .. } = effect {
                pets.push((*unit_index, false));
            }
        }
        pets.sort_by_key(|(index, _)| *index);
        pets.get(pet).is_none_or(|(_, simulated)| *simulated)
    };
    let mut reasons = Vec::new();
    for item in &rotation.priority_list {
        let mut unsummoned = None;
        for read in item_values(item) {
            read.visit(&mut |value| {
                if let Value::PetCurrentMana(pet) = value {
                    if !simulated(*pet) && unsummoned_pet_max_mana(prepared, *pet).is_none() {
                        unsummoned = Some(*pet);
                    }
                }
            });
        }
        if let Some(pet) = unsummoned {
            reasons.push(format!(
                "rotation item {} reads the mana of pet {pet}, which the fight never summons",
                item.position
            ));
        }
    }
    reasons
}

fn find_unit_aura(
    auras: &[crate::contracts::prepared_v2::Aura],
    id: &ActionId,
) -> Option<FoundAura<ActionId>> {
    auras
        .iter()
        .find(|aura| aura.action_id.as_ref() == Some(id))
        .map(|aura| FoundAura {
            aura: id.clone(),
            max_stacks: aura.max_stacks,
        })
}

/// Rotation items, by position, whose condition can never hold against the one target the
/// runtime supports, such as a `numberTargets` of two or more. Go still evaluates them, without
/// side effects, but never runs their action.
fn unreachable_items(prepared: &PreparedV2, rotation: &Rotation) -> BTreeSet<usize> {
    let targets = prepared.encounter.target_count.max(1) as usize;
    let aura = |id: &ActionId| find_aura(prepared, id);
    let target_aura = |_: usize, id: &ActionId| find_unit_aura(&prepared.target.auras, id);
    let spell = |id: &ActionId| rotation_spell_index(prepared, id);
    let dot = |id: &ActionId, unit: Unit| rotation_dot(prepared, id, unit);
    let dot_base_duration = |id: &ActionId| rotation_dot_base_duration(prepared, id);
    let pet_auras = crate::core::fight::pet::pet_agent_auras(prepared);
    let pet_aura_known =
        |pet: usize, id: &ActionId| pet_auras.get(pet).is_some_and(|auras| auras.contains(id));
    let curse = assigned_curse(prepared);
    let lookup = Lookup {
        aura: &aura,
        target_aura: &target_aura,
        targets: prepared.encounter.target_count.max(1) as usize,
        spell: &spell,
        dot: &dot,
        dot_base_duration: &dot_base_duration,
        pet_aura_known: &pet_aura_known,
        pet_aura: &|pet: usize, id: &ActionId| simulated_pet_aura(prepared, pet, id),
        simulated_pet_with_mana: &|pet: usize| simulated_pet_with_mana(prepared, pet),
        unsummoned_pet_with_mana: &|pet: usize| unsummoned_pet_with_mana(prepared, pet),
        unsummoned_pet_max_mana: &|pet: usize| unsummoned_pet_max_mana(prepared, pet),
        item_swap: has_item_swap(prepared),
        bars: rotation_bars(prepared),
        icd_aura: &|unit: Unit, id: &ActionId| find_icd_aura(prepared, unit, id),
        multidot: &|id: &ActionId| rotation_multidot(prepared, id),
        assigned_curse: curse.as_ref().map(|(name, id)| (name.as_str(), id)),
        tanked: prepared.enemy.is_some(),
        reads: prepared.player.rotation_reads.as_ref(),
        exact_spell: &|id: &ActionId| {
            prepared
                .player
                .spells
                .iter()
                .position(|spell| spell.action_id.as_ref() == Some(id))
        },
        item_procs: &|procs: &crate::rotation::StatProcs| coverage_item_procs(prepared, procs),
        expected_tick: &|id: &ActionId| coverage_expected_tick(prepared, id),
        aura_by_label: &|label: &str| {
            prepared
                .player
                .auras
                .iter()
                .find(|aura| aura.label == label)
                .and_then(|aura| aura.action_id.clone())
        },
        self_buff: &|spell: usize| {
            let id = prepared.player.spells[spell].action_id.as_ref()?;
            prepared
                .player
                .rotation_reads
                .as_ref()?
                .self_buffs
                .iter()
                .any(|buff| &buff.spell == id)
                .then(|| id.clone())
        },
    };
    rotation
        .priority_list
        .iter()
        .filter(|item| {
            let condition = item
                .condition
                .as_ref()
                .map(|value| value.with_targets(targets));
            compile_condition(condition.as_ref(), &lookup).never_holds()
        })
        .map(|item| item.position)
        .collect()
}

#[cfg(test)]
mod sourced_level_gate_tests {
    use super::sourced_actor_level;

    #[test]
    fn selected_native_checkpoint_levels_reach_coverage() {
        for class in ["ClassDruid", "ClassPriest", "ClassRogue", "ClassWarlock"] {
            assert!(sourced_actor_level(class, 20), "{class} L20");
        }
        assert!(sourced_actor_level("ClassDruid", 30));
        for level in 10..=59 {
            assert!(sourced_actor_level("ClassWarlock", level), "Warlock L{level}");
        }
        assert!(!sourced_actor_level("ClassWarlock", 9));
        assert!(!sourced_actor_level("ClassWarlock", 61));
        assert!(!sourced_actor_level("ClassRogue", 30));
        assert!(!sourced_actor_level("ClassMage", 20));
        assert!(!sourced_actor_level("ClassDruid", 40));
    }
}
