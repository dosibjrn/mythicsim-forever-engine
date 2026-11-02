//! Go sim/rogue's spell files, registered by `Rogue.Initialize`: ambush.go, backstab.go,
//! eviscerate.go, expose_armor.go, kidney_shot.go, garrote.go, rupture.go, sinister_strike.go,
//! slice_and_dice.go, vanish.go and stealth.go.
//!
//! A closure Go gives a spell config (`ApplyEffects`, `OnSnapshot` and the like) only runs in a
//! fight, so what preparation keeps of it is the field that says it is there:
//! `has_extra_cast_condition`, the related self buff and the related aura arrays.

use std::rc::Rc;

use crate::prepare::character::constants::{CHARACTER_LEVEL, MAX_MELEE_RANGE};
use crate::prepare::sim::{AuraConfig, Cooldown, Sim, UnitId, NEVER_EXPIRES};
use crate::prepare::spell::{Cast, CastConfig, DotConfig, ProcMask, SpellConfig, SpellFlag};
use crate::prepare::spelldata::Spell as Row;

use super::armor;
use super::masks;
use super::spell_data::spell_data;
use super::util::{
    aura_array_map, energy_cost, ignore_haste_cast, longest_cooldown, new_enemy_aura_array,
    spell_action,
};
use super::Rogue;

/// Go `SpellFlagBuilder`.
pub(super) const BUILDER: SpellFlag = SpellFlag::AGENT_RESERVED2;
/// Go `SpellFlagFinisher`.
pub(super) const FINISHER: SpellFlag = SpellFlag::AGENT_RESERVED3;

/// Go `RogueBleedTag`.
const ROGUE_BLEED_TAG: &str = "RogueBleed";

/// The fields the Rogue's main hand abilities share: the row's school and defense, a main hand
/// special proc mask and the melee range.
fn melee_ability(row: &Row, mask: i64, flags: SpellFlag) -> SpellConfig {
    SpellConfig {
        action_id: spell_action(row.id),
        spell_school: row.spell_school(),
        defense_type: row.defense_type_core(),
        proc_mask: ProcMask::MELEE_MH_SPECIAL,
        flags,
        class_spell_mask: mask,
        max_range: MAX_MELEE_RANGE,
        ..SpellConfig::default()
    }
}

impl Rogue {
    /// Go `registerAmbushSpell`.
    pub(super) fn register_ambush_spell(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().ambush.highest();
        // The client states the weapon share as a percentage on effect 2 (250, where TBC had
        // 275), counting from 1 by position.
        let weapon_damage = rank.effect_n(2).average(CHARACTER_LEVEL) / 100.0;
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                cost: energy_cost(rank, true),
                cast: ignore_haste_cast(rank.gcd()),
                has_extra_cast_condition: true,
                damage_multiplier: weapon_damage,
                damage_multiplier_additive: 1.0,
                threat_multiplier: 1.0,
                bonus_coefficient: rank.damage_effect().coeff(),
                ..melee_ability(
                    rank,
                    masks::AMBUSH,
                    SpellFlag::MELEE_METRICS | BUILDER | SpellFlag::APL,
                )
            },
        );
        self.spells.ambush = Some(spell);
    }

    /// Go `registerBackstabSpell`.
    pub(super) fn register_backstab_spell(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().backstab.highest_for(sim.rules);
        let weapon_damage = rank.effect_n(2).average(CHARACTER_LEVEL) / 100.0;
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                cost: energy_cost(rank, true),
                cast: ignore_haste_cast(rank.gcd()),
                has_extra_cast_condition: true,
                damage_multiplier: weapon_damage,
                damage_multiplier_additive: 1.0,
                threat_multiplier: 1.0,
                bonus_coefficient: rank.damage_effect().coeff(),
                ..melee_ability(
                    rank,
                    masks::BACKSTAB,
                    SpellFlag::MELEE_METRICS | BUILDER | SpellFlag::APL,
                )
            },
        );
        self.spells.backstab = Some(spell);
    }

    /// Go `registerEviscerate`.
    pub(super) fn register_eviscerate(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().eviscerate.highest();
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                metric_splits: 6,
                cost: energy_cost(rank, true),
                cast: ignore_haste_cast(rank.gcd()),
                has_extra_cast_condition: true,
                damage_multiplier: 1.0,
                damage_multiplier_additive: 1.0,
                threat_multiplier: 1.0,
                bonus_coefficient: rank.damage_effect().coeff(),
                ..melee_ability(
                    rank,
                    masks::EVISCERATE,
                    SpellFlag::MELEE_METRICS | FINISHER | SpellFlag::APL,
                )
            },
        );
        self.spells.eviscerate = Some(spell);
    }

    /// Go `registerExposeArmorSpell`: Forever repurposes Improved Expose Armor, whose combo
    /// points handed back on a full spend live in the cast.
    pub(super) fn register_expose_armor_spell(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().expose_armor.highest();
        let auras = new_enemy_aura_array(sim, armor::expose_armor_aura);
        let related = aura_array_map(sim, &auras);
        self.auras.expose_armor = auras;
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                metric_splits: 6,
                cost: energy_cost(rank, true),
                cast: ignore_haste_cast(rank.gcd()),
                has_extra_cast_condition: true,
                threat_multiplier: 1.0,
                related_aura_arrays: related,
                ..melee_ability(
                    rank,
                    masks::EXPOSE_ARMOR,
                    SpellFlag::MELEE_METRICS | FINISHER | SpellFlag::APL,
                )
            },
        );
        self.spells.expose_armor = Some(spell);
    }

    /// Go `registerKidneyShot`: it stuns for a second and a second a combo point, and raises the
    /// damage the target takes from the rogue with Improved Kidney Shot.
    pub(super) fn register_kidney_shot(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().kidney_shot.highest();
        let action = spell_action(rank.id);
        let damage_taken = 1.0
            + spell_data()
                .improved_kidney_shot
                .value_at(self.talents.i32("improved_kidney_shot"))
                / 100.0;
        let label = format!("Kidney Shot - {}", sim.unit(unit).label);
        let noop: crate::prepare::sim::AuraCallback = Rc::new(|_: &mut Sim, _| {});

        // Go `RegisterVariableStunAura`: a stun aura whose gain and expiry pause the swings.
        let stun_duration = rank.duration();
        let stuns = new_enemy_aura_array(sim, |sim, target| {
            let aura = sim.register_aura(
                target,
                AuraConfig {
                    label: label.clone(),
                    action_id: Some(action.clone()),
                    tag: "Stun".to_string(),
                    duration: stun_duration,
                    on_gain: Some(Rc::clone(&noop)),
                    on_expire: Some(Rc::clone(&noop)),
                    ..AuraConfig::default()
                },
            );
            if damage_taken != 1.0 {
                // The attack table's damage taken multiplier follows the stun.
                sim.apply_on_gain(aura, Rc::clone(&noop));
                sim.apply_on_expire(aura, Rc::clone(&noop));
            }
            aura
        });
        let related = aura_array_map(sim, &stuns);
        let timer = sim.new_timer(unit);
        sim.register_spell(
            unit,
            SpellConfig {
                metric_splits: 6,
                cost: energy_cost(rank, true),
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..ignore_haste_cast(rank.gcd())
                },
                has_extra_cast_condition: true,
                threat_multiplier: 1.0,
                related_aura_arrays: related,
                ..melee_ability(
                    rank,
                    masks::KIDNEY_SHOT,
                    SpellFlag::MELEE_METRICS | FINISHER | SpellFlag::APL,
                )
            },
        );
    }

    /// Go `registerGarrote`.
    pub(super) fn register_garrote(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().garrote.highest();
        let tick = rank.periodic_effect();
        let tick_length = tick.period();
        let spell = sim.get_or_register_spell(
            unit,
            SpellConfig {
                cost: energy_cost(rank, true),
                cast: ignore_haste_cast(rank.gcd()),
                has_extra_cast_condition: true,
                damage_multiplier: 1.0,
                damage_multiplier_additive: 1.0,
                threat_multiplier: 1.0,
                dot: DotConfig {
                    aura: AuraConfig {
                        label: "Garrote".to_string(),
                        tag: ROGUE_BLEED_TAG.to_string(),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: (rank.duration() / tick_length) as i32,
                    tick_length,
                    ..DotConfig::default()
                },
                ..melee_ability(
                    rank,
                    masks::GARROTE,
                    SpellFlag::MELEE_METRICS | BUILDER | SpellFlag::APL,
                )
            },
        );
        self.spells.garrote = Some(spell);
    }

    /// Go `registerRupture`.
    pub(super) fn register_rupture(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().rupture.highest();
        let tick_length = rank.periodic_effect().period();
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                metric_splits: 6,
                cost: energy_cost(rank, true),
                cast: ignore_haste_cast(rank.gcd()),
                has_extra_cast_condition: true,
                damage_multiplier: 1.0,
                damage_multiplier_additive: 1.0,
                threat_multiplier: 1.0,
                dot: DotConfig {
                    aura: AuraConfig {
                        label: "Rupture".to_string(),
                        tag: ROGUE_BLEED_TAG.to_string(),
                        ..AuraConfig::default()
                    },
                    // Set dynamically.
                    number_of_ticks: 0,
                    tick_length,
                    ..DotConfig::default()
                },
                ..melee_ability(
                    rank,
                    masks::RUPTURE,
                    SpellFlag::MELEE_METRICS | FINISHER | SpellFlag::APL,
                )
            },
        );
        self.spells.rupture = Some(spell);
    }

    /// Go `registerSinisterStrikeSpell`.
    pub(super) fn register_sinister_strike_spell(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().sinister_strike.highest();
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                cost: energy_cost(rank, true),
                cast: ignore_haste_cast(rank.gcd()),
                damage_multiplier: 1.0,
                damage_multiplier_additive: 1.0,
                threat_multiplier: 1.0,
                bonus_coefficient: rank.damage_effect().coeff(),
                ..melee_ability(
                    rank,
                    masks::SINISTER_STRIKE,
                    SpellFlag::MELEE_METRICS | BUILDER | SpellFlag::APL,
                )
            },
        );
        self.spells.sinister_strike = Some(spell);
    }

    /// Go `registerSliceAndDice`.
    pub(super) fn register_slice_and_dice(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().slice_and_dice.by_id(6774);
        let action = spell_action(rank.id);
        let state = Rc::clone(&self.state);

        // The client states the attack speed bonus as a percentage on the rank's own effect
        // (30).
        state.slice_and_dice_bonus_flat.set(
            rank.effect(crate::prepare::dbcenums::A_MOD_MELEE_HASTE_3, 0)
                .average(CHARACTER_LEVEL)
                / 100.0,
        );

        let multiplier = sim.new_state_slot(0.0);
        let gain_state = Rc::clone(&state);
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Slice and Dice".to_string(),
                action_id: Some(action.clone()),
                // This will be overridden on cast, but set a non-zero default so it doesn't
                // crash when used in APL prepull.
                duration: super::SLICE_AND_DICE_DURATIONS[5],
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    let factor = 1.0 + gain_state.slice_and_dice_bonus_flat.get();
                    sim.set_state(multiplier, factor);
                    sim.multiply_melee_speed(unit, factor);
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.multiply_melee_speed(unit, 1.0 / sim.state(multiplier));
                })),
                ..AuraConfig::default()
            },
        );
        self.auras.slice_and_dice = Some(aura);

        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id: action,
                flags: FINISHER | SpellFlag::APL,
                metric_splits: 6,
                class_spell_mask: masks::SLICE_AND_DICE,
                cost: energy_cost(rank, false),
                cast: ignore_haste_cast(rank.gcd()),
                has_extra_cast_condition: true,
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
        self.spells.slice_and_dice = Some(spell);
    }

    /// Go `registerVanishSpell`.
    pub(super) fn register_vanish_spell(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().vanish.by_id(1856);
        let timer = sim.new_timer(unit);
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                flags: SpellFlag::APL,
                class_spell_mask: masks::VANISH,
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: 0,
                        ..Cast::default()
                    },
                    ignore_haste: true,
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..CastConfig::default()
                },
                ..SpellConfig::default()
            },
        );
        self.spells.vanish = Some(spell);
    }

    /// Go `registerStealthAura`.
    pub(super) fn register_stealth_aura(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().stealth.by_id(1784);
        let noop: crate::prepare::sim::AuraCallback = Rc::new(|_: &mut Sim, _| {});
        // Stealth breaks on damage taken (if not absorbed); not modelled. Master of Subtlety is
        // not registered, so its aura is never there to follow Stealth.
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Stealth".to_string(),
                action_id: Some(spell_action(rank.id)),
                duration: NEVER_EXPIRES,
                on_gain: Some(Rc::clone(&noop)),
                on_expire: Some(noop),
                ..AuraConfig::default()
            },
        );
        self.auras.stealth = Some(aura);

        let timer = sim.new_timer(unit);
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                flags: SpellFlag::APL,
                class_spell_mask: masks::STEALTH,
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..CastConfig::default()
                },
                has_extra_cast_condition: true,
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
    }
}
