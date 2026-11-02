//! Go sim/warrior/talents_fury.go.

use crate::prepare::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger, PseudoStatField};
use crate::prepare::character::cooldown_type;
use crate::prepare::dbcenums;
use crate::prepare::sim::{AuraConfig, Rules, Sim, UnitId};
use crate::prepare::spell::{
    school, CastConfig, DotConfig, ProcMask, SpellConfig, SpellFlag as F, GCD_DEFAULT,
};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::stats::Stat;

use super::super::helpers::*;
use super::super::masks;
use super::super::spell_data::spell_data;
use super::super::spells::add_cooldown;
use super::super::Warrior;
use super::millis;

impl Warrior {
    /// Go `registerFuryTalents`.
    pub(in super::super) fn register_fury_talents(&self, sim: &mut Sim, unit: UnitId) {
        // Tier 1
        self.register_booming_voice(sim, unit);
        self.register_cruelty(sim, unit);

        // Tier 2
        // Lingering Rage delays out-of-combat Rage decay, which the sim does not model.
        self.register_unbridled_wrath(sim, unit);

        // Tier 3
        self.register_furious_precision(sim, unit);
        self.register_piercing_howl(sim, unit);
        self.register_blood_craze(sim, unit);

        // Tier 4
        self.register_dual_wield_specialization(sim, unit);
        self.register_raging_blows(sim, unit);
        self.register_enrage(sim, unit);
        self.register_improved_execute(sim, unit);

        // Tier 5
        // Improved Berserker Rage: berserker_rage.go
        self.register_death_wish(sim, unit);
        self.register_improved_intercept(sim, unit);

        // Tier 6
        self.register_flurry(sim, unit);

        // Tier 7
        self.register_bloodthirst(sim, unit);
    }

    fn register_cruelty(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("cruelty");
        if rank == 0 {
            return;
        }
        sim.add_stat(
            unit,
            Stat::PhysicalCritPercent,
            spell_data().cruelty.value_at(rank),
        );
    }

    fn register_unbridled_wrath(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("unbridled_wrath");
        if rank == 0 {
            return;
        }
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Unbridled Wrath".to_string(),
                proc_mask: ProcMask::MELEE_WHITE_HIT,
                // Heroic Strike and Cleave replace a main-hand swing but never proc it.
                proc_mask_exclude: ProcMask::MELEE_SPECIAL,
                proc_chance: spell_data().unbridled_wrath.fraction_at(rank),
                require_damage_dealt: true,
                outcome: HitOutcome::LANDED,
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                ..ProcTrigger::default()
            },
        );
    }

    fn register_dual_wield_specialization(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("dual_wield_specialization");
        if rank == 0 {
            return;
        }
        let ladder = &spell_data().dual_wield_specialization;
        sim.add_static_mod(
            unit,
            SpellModConfig {
                proc_mask: ProcMask::MELEE_OH,
                kind: SpellModType::DamageDonePct,
                float_value: ladder
                    .effect(dbcenums::A_MOD_OFFHAND_DAMAGE_PCT, 0)
                    .fraction_at(rank),
                ..SpellModConfig::default()
            },
        );
        // The 70170 hotfixes brought the off-hand Rage bonus back as a dummy effect.
        sim.unit_mut(unit).rage_bar.off_hand_rage_multiplier =
            ladder.effect(dbcenums::A_DUMMY, 0).multiplier_at(rank);
    }

    fn register_improved_execute(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("improved_execute");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::EXECUTE,
                kind: SpellModType::PowerCostFlat,
                int_value: spell_data().improved_execute.tenths_at(rank) as i32,
                ..SpellModConfig::default()
            },
        );
    }

    fn register_enrage(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("enrage");
        if rank == 0 {
            return;
        }
        let data = spell_data();
        let buff = data.enrage_triggered.highest();
        let aura = sim.get_or_register_aura(
            unit,
            AuraConfig {
                label: "Enrage".to_string(),
                action_id: Some(spell_action(buff.id)),
                duration: buff.duration(),
                ..AuraConfig::default()
            },
        );
        sim.attach_spell_mod(
            aura,
            SpellModConfig {
                school: school::PHYSICAL,
                kind: SpellModType::DamageDonePct,
                float_value: data.enrage.fraction_at(rank),
                ..SpellModConfig::default()
            },
        );
        sim.new_exclusive_effect(aura, "Enrage", true, data.enrage.value_at(rank), None, None);
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Enrage - Trigger".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_TAKEN,
                outcome: HitOutcome::LANDED,
                require_damage_dealt: true,
                proc_chance: f64::from(data.enrage.rank(rank).proc_chance) / 100.0,
                ..ProcTrigger::default()
            },
        );
    }

    fn register_flurry(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("flurry");
        if rank == 0 {
            return;
        }
        let data = spell_data();
        let forever = sim.rules == Rules::Forever;
        let buff = data.flurry_triggered.highest();
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Flurry".to_string(),
                action_id: Some(spell_action(buff.id)),
                duration: buff.duration(),
                max_stacks: i32::from(buff.proc_charges),
                ..AuraConfig::default()
            },
        );
        sim.attach_multiply_melee_speed(aura, data.flurry.multiplier_at(rank));
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Flurry - Trigger".to_string(),
                action_id: spell_action(12319),
                proc_mask: ProcMask::MELEE,
                trigger_immediately: true,
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                // Under Forever's rules a missed, dodged or parried white swing spends a charge too.
                outcome: if forever {
                    HitOutcome::EMPTY
                } else {
                    HitOutcome::LANDED
                },
                ..ProcTrigger::default()
            },
        );
    }

    /// Furious Precision: off-hand hit chance, in the same shape as Dual Wield Specialization's.
    fn register_furious_precision(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("furious_precision");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                proc_mask: ProcMask::MELEE_OH,
                kind: SpellModType::BonusHitPercent,
                float_value: spell_data()
                    .furious_precision
                    .effect(dbcenums::A_MOD_HIT_CHANCE, 0)
                    .value_at(rank),
                ..SpellModConfig::default()
            },
        );
    }

    fn register_bloodthirst(&self, sim: &mut Sim, unit: UnitId) {
        if !self.has_talent("bloodthirst") {
            return;
        }
        let rank = spell_data().bloodthirst.highest();
        let cd = new_cooldown(sim, unit, cooldown_of(rank));
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                rank: rank.rank_number(),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                flags: F::MELEE_METRICS | F::APL,
                class_spell_mask: masks::BLOODTHIRST,
                max_range: f64::from(rank.max_range),
                cost: rage_cost_with_refund(rank),
                cast: cast_config(rank.gcd(), true, cd),
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        );
    }

    fn register_piercing_howl(&self, sim: &mut Sim, unit: UnitId) {
        if !self.has_talent("piercing_howl") {
            return;
        }
        let rank = spell_data().piercing_howl.highest();
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: school::PHYSICAL,
                proc_mask: ProcMask::EMPTY,
                flags: F::APL,
                class_spell_mask: 0,
                cost: rage_cost(rank),
                cast: cast_config(rank.gcd(), true, Default::default()),
                ..SpellConfig::default()
            },
        );
    }

    fn register_blood_craze(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("blood_craze");
        if rank == 0 {
            return;
        }
        let hot = spell_data().blood_craze_triggered.highest();
        let tick = hot.periodic_effect();
        let tick_length = tick.period();
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(hot.id),
                spell_school: school::PHYSICAL,
                proc_mask: ProcMask::SPELL_HEALING,
                // A heal of max health: the Physical damage-done mods do not raise it.
                flags: F::PASSIVE_SPELL | F::HELPFUL | F::NO_ON_CAST_COMPLETE | F::NO_SPELL_MODS,
                damage_multiplier: 1.0,
                // Client 1.60.1.70630 flags Blood Craze (16488, SpellMisc 327352) No Threat
                // (Attributes_1 bit 0x400): its healing puts the warrior on no threat table. The
                // Go reference keeps the heal's full threat.
                threat_multiplier: if sim.rules == Rules::Forever {
                    0.0
                } else {
                    1.0
                },
                hot: DotConfig {
                    aura: AuraConfig {
                        label: "Blood Craze".to_string(),
                        ..AuraConfig::default()
                    },
                    self_only: true,
                    number_of_ticks: (hot.duration() / tick_length) as i32,
                    tick_length,
                    ..DotConfig::default()
                },
                ..SpellConfig::default()
            },
        );
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Blood Craze - Damage Taken".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_TAKEN,
                outcome: HitOutcome::LANDED,
                require_damage_dealt: true,
                ..ProcTrigger::default()
            },
        );
        // No Bloodthirst trigger: the 1 October beta notes ("Blood Craze no longer activates off
        // of Bloodthirst casts") and the client 70374 tooltip dropped it (community #797).
    }

    /// Booming Voice also takes 5% a point off the Rage cost of the shouts.
    fn register_booming_voice(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("booming_voice");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::SHOUTS,
                kind: SpellModType::PowerCostPctAdd,
                float_value: spell_data().booming_voice.effect_at(2).fraction_at(rank),
                ..SpellModConfig::default()
            },
        );
    }

    fn register_raging_blows(&self, sim: &mut Sim, unit: UnitId) {
        if !self.has_talent("raging_blows") {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::CLEAVE | masks::WHIRLWIND,
                kind: SpellModType::PowerCostFlat,
                int_value: spell_data().raging_blows.effect_at(2).tenths_at(1) as i32,
                ..SpellModConfig::default()
            },
        );
    }

    fn register_death_wish(&self, sim: &mut Sim, unit: UnitId) {
        if !self.has_talent("death_wish") {
            return;
        }
        let rank = spell_data().death_wish.highest();
        let action = spell_action(rank.id);
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Death Wish".to_string(),
                action_id: Some(action.clone()),
                duration: rank.duration(),
                ..AuraConfig::default()
            },
        );
        sim.attach_multiplicative_pseudo_stat_buff(
            aura,
            PseudoStatField::SchoolDamageDealtMultiplier(
                crate::prepare::stats::SchoolIndex::Physical,
            ),
            1.0 + rank
                .effect(dbcenums::A_MOD_DAMAGE_PERCENT_DONE, 1)
                .percent(),
        );
        sim.attach_multiplicative_pseudo_stat_buff(
            aura,
            PseudoStatField::DamageTakenMultiplier,
            1.0 + rank
                .effect(dbcenums::A_MOD_DAMAGE_PERCENT_TAKEN, 127)
                .percent(),
        );
        attach_fear_immunity(sim, aura);

        let cd = new_cooldown(sim, unit, cooldown_of(rank));
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id: action,
                class_spell_mask: masks::DEATH_WISH,
                flags: F::CAST_WHILE_INCAPACITATED,
                cost: rage_cost(rank),
                cast: CastConfig {
                    cd,
                    ..cast_config(GCD_DEFAULT, true, Default::default())
                },
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
        add_cooldown(sim, unit, spell, cooldown_type::DPS);
    }

    fn register_improved_intercept(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("improved_intercept");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::INTERCEPT,
                kind: SpellModType::CooldownFlat,
                time_value: millis(spell_data().improved_intercept.value_at(rank)),
                ..SpellModConfig::default()
            },
        );
    }
}
