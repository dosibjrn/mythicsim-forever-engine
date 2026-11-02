//! Go sim/hunter/aspects.go and the quiver bonus of hunter.go: Aspect of the Hawk is the ranged
//! aspect and Aspect of the Beast the melee one. A hunter holds one aspect at a time.

use std::rc::Rc;

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::dbcenums;
use crate::prepare::sim::{AuraConfig, AuraId, BuildPhase, EventCallbacks, Sim, NEVER_EXPIRES};
use crate::prepare::spell::{Cast, CastConfig, CostOptions, ProcMask, SpellConfig, SpellFlag};
use crate::prepare::stats::Stat;

use super::spell_data::spell_data;
use super::{masks, Hunter};

/// Go `quiverHasteMultipliers`.
fn quiver_haste_multiplier(bonus: &str) -> f64 {
    match bonus {
        "Speed10" => 1.1,
        "Speed11" => 1.11,
        "Speed12" => 1.12,
        "Speed13" => 1.13,
        "Speed14" => 1.14,
        "Speed15" => 1.15,
        _ => 0.0,
    }
}

/// Go `quiverHasteSpellIDs`.
fn quiver_haste_spell_id(bonus: &str) -> i32 {
    match bonus {
        "Speed10" => 29418,
        "Speed11" => 29417,
        "Speed12" => 29416,
        "Speed13" => 29413,
        "Speed14" => 29415,
        "Speed15" => 29414,
        _ => 0,
    }
}

/// Go `quiverHasteMultipliers[Speed15]`: Thori'dal's legendary bow haste.
pub(super) const SPEED_15_MULTIPLIER: f64 = 1.15;

impl Hunter {
    /// Go `applyQuiverBonus`.
    pub(super) fn apply_quiver_bonus(&mut self, sim: &mut Sim, ranged_item_id: Option<i32>) {
        let bonus = self.options.enum_name("quiver_bonus");
        if bonus == "QuiverNone" {
            return;
        }
        let is_thoridal_equipped = ranged_item_id == Some(super::THORIDAL_THE_STARS_FURY);
        let build_phase = if is_thoridal_equipped {
            BuildPhase::NONE
        } else {
            BuildPhase::GEAR
        };
        let multiplier = quiver_haste_multiplier(&bonus);
        let unit = self.unit;
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Haste".to_string(),
                action_id: Some(ActionId::spell(quiver_haste_spell_id(&bonus))),
                duration: NEVER_EXPIRES,
                build_phase,
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.unit_mut(unit).pseudo_stats.ranged_speed_multiplier *= multiplier;
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.unit_mut(unit).pseudo_stats.ranged_speed_multiplier /= multiplier;
                })),
                ..AuraConfig::default()
            },
        );
        self.quiver_bonus_aura = Some(aura);
        if !is_thoridal_equipped {
            sim.make_permanent(aura);
        }
    }

    /// Go `registerAspects`.
    pub(super) fn register_aspects(&mut self, sim: &mut Sim) {
        self.register_aspect_of_the_hawk_spell(sim);
        self.register_aspect_of_the_beast_spell(sim);
    }

    fn register_aspect_of_the_hawk_spell(&mut self, sim: &mut Sim) {
        let data = spell_data();
        let unit = self.unit;
        let hawk_rank = data.aspect_of_the_hawk.highest_for(sim.rules);
        let action_id = ActionId::spell(hawk_rank.id);
        let deadly_aspects = self.t("deadly_aspects");

        // Every rank of Deadly Aspects triggers the same Quick Shots (6150): 30% ranged haste for
        // 12 sec. The points buy only the proc chance, 2% a rank.
        if deadly_aspects > 0 {
            let rank = data.aspect_of_the_hawk_triggered.highest();
            let haste_multiplier = 1.0
                + rank
                    .effect(dbcenums::A_MOD_RANGED_HASTE, 0)
                    .average(crate::prepare::character::constants::CHARACTER_LEVEL)
                    / 100.0;
            sim.get_or_register_aura(
                unit,
                AuraConfig {
                    label: "Quick Shots".to_string(),
                    action_id: Some(ActionId::spell(rank.id)),
                    duration: rank.duration(),
                    on_gain: Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
                        let unit = sim.aura(aura).unit;
                        sim.multiply_ranged_speed(unit, haste_multiplier);
                    })),
                    on_expire: Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
                        let unit = sim.aura(aura).unit;
                        sim.multiply_ranged_speed(unit, 1.0 / haste_multiplier);
                    })),
                    ..AuraConfig::default()
                },
            );
        }

        let rap = hawk_rank
            .effect(dbcenums::A_MOD_RANGED_ATTACK_POWER, 0)
            .average(crate::prepare::character::constants::CHARACTER_LEVEL);
        let aura = sim.get_or_register_aura(
            unit,
            AuraConfig {
                label: "Aspect of the Hawk".to_string(),
                action_id: Some(action_id.clone()),
                duration: NEVER_EXPIRES,
                build_phase: BuildPhase::NONE,
                on_gain: Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
                    let unit = sim.aura(aura).unit;
                    sim.add_stat_dynamic(unit, Stat::RangedAttackPower, rap);
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
                    let unit = sim.aura(aura).unit;
                    sim.add_stat_dynamic(unit, Stat::RangedAttackPower, -rap);
                })),
                events: EventCallbacks {
                    on_spell_hit_dealt: true,
                    ..EventCallbacks::default()
                },
                ..AuraConfig::default()
            },
        );
        sim.new_exclusive_effect(aura, "Aspect", true, 0.0, None, None);
        self.aspect_of_the_hawk_aura = Some(aura);

        self.aspect_of_the_hawk = Some(sim.register_spell(
            unit,
            SpellConfig {
                action_id,
                spell_school: hawk_rank.spell_school(),
                defense_type: hawk_rank.defense_type_core(),
                class_spell_mask: masks::ASPECT_OF_THE_HAWK,
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::APL,
                cost: CostOptions {
                    mana_flat_cost: hawk_rank.cost() as i32,
                    ..CostOptions::default()
                },
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: hawk_rank.gcd(),
                        ..Cast::default()
                    },
                    ignore_haste: true,
                    ..CastConfig::default()
                },
                has_extra_cast_condition: true,
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        ));
    }

    fn register_aspect_of_the_beast_spell(&mut self, sim: &mut Sim) {
        let data = spell_data();
        let unit = self.unit;
        let beast_rank = data.aspect_of_the_beast.highest();
        let action_id = ActionId::spell(beast_rank.id);
        let deadly_aspects = self.t("deadly_aspects");

        // Deadly Aspects' second effect is Beast's proc chance, 2% a rank, and it triggers Quick
        // Strikes (1299448): 30% melee haste for 12 sec. Beast states no chance of its own, so
        // the proc needs the talent.
        if deadly_aspects > 0 {
            let rank = data.aspect_of_the_beast_triggered.highest();
            let haste_multiplier = 1.0
                + rank
                    .effect(dbcenums::A_MOD_MELEE_HASTE_3, 0)
                    .average(crate::prepare::character::constants::CHARACTER_LEVEL)
                    / 100.0;
            sim.get_or_register_aura(
                unit,
                AuraConfig {
                    label: "Quick Strikes".to_string(),
                    action_id: Some(ActionId::spell(rank.id)),
                    duration: rank.duration(),
                    on_gain: Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
                        let unit = sim.aura(aura).unit;
                        sim.multiply_melee_speed(unit, haste_multiplier);
                    })),
                    on_expire: Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
                        let unit = sim.aura(aura).unit;
                        sim.multiply_melee_speed(unit, 1.0 / haste_multiplier);
                    })),
                    ..AuraConfig::default()
                },
            );
        }

        let ap = beast_rank
            .effect(dbcenums::A_MOD_ATTACK_POWER, 0)
            .average(crate::prepare::character::constants::CHARACTER_LEVEL);
        let aura = sim.get_or_register_aura(
            unit,
            AuraConfig {
                label: "Aspect of the Beast".to_string(),
                action_id: Some(action_id.clone()),
                duration: NEVER_EXPIRES,
                build_phase: BuildPhase::NONE,
                on_gain: Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
                    let unit = sim.aura(aura).unit;
                    sim.add_stat_dynamic(unit, Stat::AttackPower, ap);
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
                    let unit = sim.aura(aura).unit;
                    sim.add_stat_dynamic(unit, Stat::AttackPower, -ap);
                })),
                // Beast's proc flags are melee auto attacks only (0x4), so a Raptor Strike,
                // which takes a main-hand swing's place as a special attack, never procs it.
                events: EventCallbacks {
                    on_spell_hit_dealt: true,
                    ..EventCallbacks::default()
                },
                ..AuraConfig::default()
            },
        );
        sim.new_exclusive_effect(aura, "Aspect", true, 0.0, None, None);
        self.aspect_of_the_beast_aura = Some(aura);

        self.aspect_of_the_beast = Some(sim.register_spell(
            unit,
            SpellConfig {
                action_id,
                spell_school: beast_rank.spell_school(),
                defense_type: beast_rank.defense_type_core(),
                class_spell_mask: masks::ASPECT_OF_THE_BEAST,
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::APL,
                cost: CostOptions {
                    mana_flat_cost: beast_rank.cost() as i32,
                    ..CostOptions::default()
                },
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: beast_rank.gcd(),
                        ..Cast::default()
                    },
                    ignore_haste: true,
                    ..CastConfig::default()
                },
                has_extra_cast_condition: true,
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        ));
    }
}
