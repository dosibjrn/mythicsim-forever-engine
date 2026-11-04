//! Go sim/rogue/talents_combat.go.

use std::rc::Rc;

use crate::prepare::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use crate::prepare::character::constants::{
    CHARACTER_LEVEL, DODGE_RATING_PER_DODGE_PERCENT, MAX_MELEE_RANGE,
    PARRY_RATING_PER_PARRY_PERCENT,
};
use crate::prepare::character::{cooldown_type, MajorCooldown};
use crate::prepare::dbcenums;
use crate::prepare::env::Environment;
use crate::prepare::sim::{AuraConfig, Cooldown, EventCallbacks, Rules, Sim, UnitId};
use crate::prepare::spell::{school, Cast, CastConfig, ProcMask, SpellConfig, SpellFlag};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::stats::Stat;

use super::super::masks;
use super::super::rework;
use super::super::spell_data::spell_data;
use super::super::util::{energy_cost, longest_cooldown, proc_mask_for_types, spell_action};
use super::super::Rogue;

/// `stats[stat] += value * factor`, which Go's arm64 build fuses into one multiply-add.
fn add_stat_scaled(sim: &mut Sim, unit: UnitId, stat: Stat, value: f64, factor: f64) {
    let stats = &mut sim.unit_mut(unit).stats;
    stats[stat] = value.mul_add(factor, stats[stat]);
}

impl Rogue {
    /// Go `addArmorIgnore`: armor ignored as a share of the target's, which lives on the attack
    /// table rather than on a stat, so it has to wait until the tables exist.
    pub(super) fn add_armor_ignore(&mut self, unit: UnitId, factor: f64) {
        self.post_finalize
            .push(Rc::new(move |env: &mut Environment| {
                let attacker = env.sim.unit(unit).unit_index as usize;
                for table in &mut env.attack_tables[attacker] {
                    table.armor_ignore_factor += factor;
                }
            }));
    }

    /// Go `registerCombatTalents`.
    pub(in super::super) fn register_combat_talents(&mut self, sim: &mut Sim, unit: UnitId) {
        // Tier 1: Improved Gouge models nothing in Go either.
        self.register_improved_sinister_strike(sim, unit);
        self.register_lightning_reflexes(sim, unit);

        // Tier 2
        // Improved Slice and Dice is implemented in slice_and_dice.go.
        self.register_deflection(sim, unit);
        self.register_precision(sim, unit);

        // Tier 3: Endurance models nothing in Go either.
        self.register_riposte(sim, unit);

        // Tier 4: Improved Sprint and Improved Kick model nothing in Go either.
        self.register_flawless_execution(sim, unit);
        self.register_dual_wield_specialization(sim, unit);

        // Tier 5
        self.register_blade_flurry(sim, unit);
        self.register_hack_and_slash(sim, unit);

        // Tier 6
        self.register_weapon_expertise(sim, unit);
        self.register_aggression(sim, unit);

        // Tier 7
        self.register_adrenaline_rush(sim, unit);
    }

    fn register_improved_sinister_strike(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("improved_sinister_strike");
        if rank == 0 {
            return;
        }
        // Efficient Strikes (the notes of 2 November 2026 rename Improved Sinister Strike) takes
        // the Energy off Hemorrhage too.
        let class_mask = match sim.rules {
            Rules::Forever => masks::SINISTER_STRIKE | masks::HEMORRHAGE,
            Rules::GoReference => masks::SINISTER_STRIKE,
        };
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::PowerCostFlat,
                class_mask,
                int_value: spell_data()
                    .improved_sinister_strike
                    .effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_COST)
                    .value_at(rank) as i32,
                ..SpellModConfig::default()
            },
        );
    }

    fn register_lightning_reflexes(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("lightning_reflexes");
        if rank == 0 {
            return;
        }
        add_stat_scaled(
            sim,
            unit,
            Stat::DodgeRating,
            spell_data().lightning_reflexes.value_at(rank),
            DODGE_RATING_PER_DODGE_PERCENT,
        );
    }

    fn register_deflection(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("deflection");
        if rank == 0 {
            return;
        }
        add_stat_scaled(
            sim,
            unit,
            Stat::ParryRating,
            spell_data().deflection.value_at(rank),
            PARRY_RATING_PER_PARRY_PERCENT,
        );
    }

    /// Precision covers Poisons in Forever, and those roll against the spell hit table.
    fn register_precision(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("precision");
        if rank == 0 {
            return;
        }
        let data = &spell_data().precision;
        sim.add_stat(
            unit,
            Stat::PhysicalHitPercent,
            data.effect_at(1).value_at(rank),
        );
        sim.add_stat(
            unit,
            Stat::SpellHitPercent,
            data.effect_at(2).value_at(rank),
        );
    }

    /// Flawless Execution, new in Forever: the client states a flat cost reduction, which our
    /// Forever sim reads as ten energy off Eviscerate, fifteen since the notes of 2 November
    /// 2026.
    fn register_flawless_execution(&self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("flawless_execution") {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::PowerCostFlat,
                class_mask: masks::EVISCERATE,
                int_value: crate::prepare::hotfix_113554::row(sim.rules, 1310711)
                    .effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_COST)
                    .base_points as i32,
                ..SpellModConfig::default()
            },
        );
    }

    fn register_dual_wield_specialization(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("dual_wield_specialization");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::DamageDoneFlat,
                proc_mask: ProcMask::MELEE_OH,
                float_value: rework::talent_ladder(sim.rules, 13715, 2, 5).fraction_at(rank),
                ..SpellModConfig::default()
            },
        );
    }

    fn register_blade_flurry(&mut self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("blade_flurry") {
            return;
        }
        let rank = spell_data().blade_flurry.highest();
        let action = spell_action(rank.id);
        let attack_speed = 1.0
            + rank
                .effect(dbcenums::A_MOD_MELEE_HASTE_3, 0)
                .average(CHARACTER_LEVEL)
                / 100.0;

        sim.get_or_register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(22482),
                spell_school: school::PHYSICAL,
                // No proc mask, so it won't proc itself.
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::IGNORE_RESISTS
                    | SpellFlag::IGNORE_MODIFIERS
                    | SpellFlag::MELEE_METRICS
                    | SpellFlag::PASSIVE_SPELL
                    | SpellFlag::NO_ON_CAST_COMPLETE,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        );

        let aura = sim.get_or_register_aura(
            unit,
            AuraConfig {
                label: "Blade Flurry".to_string(),
                action_id: Some(action.clone()),
                duration: rank.duration(),
                events: EventCallbacks {
                    on_spell_hit_dealt: true,
                    ..EventCallbacks::default()
                },
                ..AuraConfig::default()
            },
        );
        sim.attach_multiply_attack_speed(aura, attack_speed);
        self.auras.blade_flurry = Some(aura);

        let timer = sim.new_timer(unit);
        let spell = sim.get_or_register_spell(
            unit,
            SpellConfig {
                action_id: action,
                flags: SpellFlag::APL,
                class_spell_mask: masks::BLADE_FLURRY,
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: rank.gcd(),
                        ..Cast::default()
                    },
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ignore_haste: true,
                    ..CastConfig::default()
                },
                cost: energy_cost(rank, false),
                ..SpellConfig::default()
            },
        );
        self.spells.blade_flurry = Some(spell);
        sim.add_major_cooldown(
            unit,
            MajorCooldown {
                spell,
                priority: 0,
                cooldown_type: cooldown_type::DPS,
                allow_spell_queueing: false,
                timings: Vec::new(),
            },
        );
    }

    /// Hack and Slash folds the four Classic weapon specialization talents into one and picks
    /// its effect from the weapons equipped: 1% extra attack per rank on axes and swords, 1%
    /// crit per rank on daggers and fists, 3% of the target's armor ignored per rank on maces.
    fn register_hack_and_slash(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("hack_and_slash");
        if points == 0 {
            return;
        }
        let data = &spell_data().hack_and_slash;

        // Axes and swords: extra attack.
        let mask = proc_mask_for_types(sim, unit, &["WeaponTypeAxe", "WeaponTypeSword"]);
        if mask != ProcMask::UNKNOWN {
            sim.make_proc_trigger_aura(
                unit,
                &ProcTrigger {
                    name: "Hack and Slash".to_string(),
                    callback: CallbackMask::ON_SPELL_HIT_DEALT,
                    proc_mask: mask,
                    outcome: HitOutcome::LANDED,
                    proc_chance: data.effect_at(3).value_at(points) / 100.0,
                    icd: data.highest().icd(),
                    trigger_immediately: true,
                    ..ProcTrigger::default()
                },
            );
        }
        let crit = data.effect_at(1).value_at(points);
        let armor_ignore = data.effect_at(2).value_at(points) / 100.0;

        // Daggers and fists: crit. The character pane shows the bonus for the main hand, so an
        // off-hand-only qualifier gets it on off-hand hits alone.
        let crit_mask = proc_mask_for_types(sim, unit, &["WeaponTypeDagger", "WeaponTypeFist"]);
        if crit_mask == ProcMask::MELEE {
            sim.add_stat(unit, Stat::PhysicalCritPercent, crit);
        } else if crit_mask == ProcMask::MELEE_MH || crit_mask == ProcMask::MELEE_OH {
            sim.add_static_mod(
                unit,
                SpellModConfig {
                    kind: SpellModType::BonusCritPercent,
                    proc_mask: crit_mask,
                    float_value: crit,
                    ..SpellModConfig::default()
                },
            );
        }

        // Maces: a share of the target's armor ignored.
        if proc_mask_for_types(sim, unit, &["WeaponTypeMace"]) != ProcMask::UNKNOWN {
            self.add_armor_ignore(unit, armor_ignore);
        }
    }

    /// Weapon Expertise no longer grants weapon skill; it takes the chance for the rogue's
    /// attacks to be dodged or parried off the attack table directly.
    fn register_weapon_expertise(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("weapon_expertise");
        if rank == 0 {
            return;
        }
        sim.add_stat(
            unit,
            Stat::ExpertisePercent,
            spell_data().weapon_expertise.value_at(rank),
        );
    }

    fn register_aggression(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("aggression");
        if rank == 0 {
            return;
        }
        // Sinister Strike, Backstab and Eviscerate by 2/4/6%; Sinister Strike and Eviscerate by
        // 5/10/15% since the notes of 2 November 2026.
        let class_mask = match sim.rules {
            Rules::Forever => masks::SINISTER_STRIKE | masks::EVISCERATE,
            Rules::GoReference => masks::SINISTER_STRIKE | masks::BACKSTAB | masks::EVISCERATE,
        };
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::DamageDoneFlat,
                class_mask,
                float_value: crate::prepare::hotfix_113554::talent(sim.rules, 18427, 3)
                    .fraction_at(rank),
                ..SpellModConfig::default()
            },
        );
    }

    fn register_adrenaline_rush(&mut self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("adrenaline_rush") {
            return;
        }
        let rank = spell_data().adrenaline_rush.highest();
        let action = spell_action(rank.id);
        let regen_multiplier = 1.0
            + rank
                .effect(dbcenums::A_MOD_POWER_REGEN_PERCENT, 3)
                .average(CHARACTER_LEVEL)
                / 100.0;

        let aura = sim.get_or_register_aura(
            unit,
            AuraConfig {
                label: "Adrenaline Rush".to_string(),
                action_id: Some(action.clone()),
                duration: rank.duration(),
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.multiply_energy_regen_speed(unit, regen_multiplier);
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.multiply_energy_regen_speed(unit, 1.0 / regen_multiplier);
                })),
                ..AuraConfig::default()
            },
        );
        self.auras.adrenaline_rush = Some(aura);

        let timer = sim.new_timer(unit);
        let spell = sim.get_or_register_spell(
            unit,
            SpellConfig {
                action_id: action,
                flags: SpellFlag::APL,
                class_spell_mask: masks::ADRENALINE_RUSH,
                cast: CastConfig {
                    ignore_haste: true,
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    default_cast: Cast {
                        gcd: rank.gcd(),
                        ..Cast::default()
                    },
                    ..CastConfig::default()
                },
                ..SpellConfig::default()
            },
        );
        self.spells.adrenaline_rush = Some(spell);
        sim.add_major_cooldown(
            unit,
            MajorCooldown {
                spell,
                priority: 0,
                cooldown_type: cooldown_type::DPS,
                allow_spell_queueing: false,
                timings: Vec::new(),
            },
        );
    }

    /// Riposte answers a parried attack. The rogue is not the one being hit in a DPS sim, so it
    /// only fires when the encounter swings back.
    fn register_riposte(&self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("riposte") {
            return;
        }
        let rank = spell_data().riposte.highest();
        let action = spell_action(rank.id);
        let weapon_damage = rank.effect(dbcenums::A_NONE, 0).average(CHARACTER_LEVEL) / 100.0;

        let timer = sim.new_timer(unit);
        sim.get_or_register_spell(
            unit,
            SpellConfig {
                action_id: action.clone(),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                flags: SpellFlag::MELEE_METRICS | SpellFlag::APL,
                class_spell_mask: masks::RIPOSTE,
                max_range: MAX_MELEE_RANGE,
                cost: energy_cost(rank, false),
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..CastConfig::default()
                },
                has_extra_cast_condition: true,
                damage_multiplier: weapon_damage,
                damage_multiplier_additive: 1.0,
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        );

        sim.register_aura(
            unit,
            AuraConfig {
                label: "Riposte Ready".to_string(),
                action_id: Some(action),
                duration: rank.duration(),
                ..AuraConfig::default()
            },
        );

        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Riposte Trigger".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_TAKEN,
                outcome: HitOutcome::PARRY,
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );
    }
}
