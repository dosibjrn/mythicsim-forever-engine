//! Go `sim/paladin/talents_protection.go`.

use std::rc::Rc;

use crate::prepare::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger, PseudoStatField};
use crate::prepare::character::constants::DEFENSE_RATING_PER_DEFENSE_LEVEL;
use crate::prepare::dbcenums;
use crate::prepare::sim::{AuraConfig, EventCallbacks, Rules, Sim, UnitId, NEVER_EXPIRES, SECOND};
use crate::prepare::spell::ProcMask;
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::stats::Stat;

use super::super::hotfix_113554 as hotfix;
use super::super::masks;
use super::super::spell_data::spell_data;
use super::super::util::spell_action;
use super::super::Paladin;
use super::{follow_main_hand, millis};
use crate::prepare::sim::SpellId;
use crate::prepare::spell::SpellFlag;

impl Paladin {
    /// Go `registerProtectionTalents`.
    pub(in super::super) fn register_protection_talents(&mut self, sim: &mut Sim, unit: UnitId) {
        // Tier 1
        self.apply_toughness(sim, unit);
        self.apply_redoubt(sim, unit);

        // Tier 2
        self.apply_precision(sim, unit);
        // Guardian's Favor changes Blessing of Protection and Blessing of Freedom, which are
        // not modelled.
        self.apply_anticipation(sim, unit);

        // Tier 3
        // Improved Seal of Fury attaches to the seal's shield in seal_of_fury.go
        self.apply_improved_righteous_fury(sim, unit);
        self.apply_shield_specialization(sim, unit);
        self.apply_sacred_duty(sim, unit);

        // Tier 4
        // Swift Judgement registered in registerTalentSpells
        self.apply_one_handed_weapon_specialization(sim, unit);
        // Improved Hammer of Justice shortens a cooldown the sim does not model.

        // Tier 5
        // Templar's Bulwark registered in registerTalentSpells
        self.apply_reckoning(sim, unit);

        // Tier 6
        self.apply_iron_creed(sim, unit);

        // Tier 7
        // Holy Shield registered in registerTalentSpells

        // Push 113554's Protection capstone, under Forever's rules. Stoicism shortens Stun and
        // Charm effects, which the sim never suffers.
        self.apply_shieldsworn(sim, unit);
    }

    /// Toughness: increases your armor value from items by 2/4/6/8/10%.
    fn apply_toughness(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("toughness");
        if points == 0 {
            return;
        }
        sim.apply_equip_scaling(
            unit,
            Stat::Armor,
            spell_data()
                .toughness
                .effect(dbcenums::A_MOD_BASE_RESISTANCE_PCT, 1)
                .multiplier_at(points),
        );
    }

    /// Redoubt: damaging melee attacks against you have a chance to increase your chance to
    /// block. Lasts 10 sec or 5 blocks.
    fn apply_redoubt(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("redoubt");
        if points == 0 {
            return;
        }
        let rank = spell_data().redoubt_triggered.highest();
        let label = sim.unit(unit).label.clone();

        let redoubt = sim.register_aura(
            unit,
            AuraConfig {
                label: format!("Redoubt{label}"),
                action_id: Some(spell_action(rank.id)),
                duration: rank.duration(),
                max_stacks: i32::from(rank.proc_charges),
                ..AuraConfig::default()
            },
        );
        // 6% a rank since push 113554 under Forever's rules (was 4%).
        let block = match sim.rules {
            Rules::Forever => hotfix::at(&hotfix::REDOUBT, points),
            Rules::GoReference => spell_data().redoubt.value_at(points),
        };
        sim.attach_stat_buff(redoubt, Stat::BlockPercent, block);
        sim.attach_proc_trigger(
            redoubt,
            &ProcTrigger {
                callback: CallbackMask::ON_SPELL_HIT_TAKEN,
                outcome: HitOutcome::BLOCK,
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );

        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: format!("Redoubt - Trigger{label}"),
                callback: CallbackMask::ON_SPELL_HIT_TAKEN,
                proc_mask: ProcMask::MELEE,
                outcome: HitOutcome::LANDED,
                require_damage_dealt: true,
                // Up with the hit that procs it, like Shield Specialization.
                trigger_immediately: true,
                // The row carries the rank 5 chance on every rank; the beta client's talent
                // curve is 2% a rank.
                proc_chance: 0.02 * f64::from(points),
                ..ProcTrigger::default()
            },
        );
    }

    /// Precision: improves your chance to hit by 1/2/3%, with melee weapons and spells alike.
    fn apply_precision(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("precision");
        if points == 0 {
            return;
        }
        sim.add_stat(
            unit,
            Stat::PhysicalHitPercent,
            spell_data()
                .precision
                .effect(dbcenums::A_MOD_HIT_CHANCE, 0)
                .value_at(points),
        );
        sim.add_stat(
            unit,
            Stat::SpellHitPercent,
            spell_data()
                .precision
                .effect(dbcenums::A_MOD_SPELL_HIT_CHANCE, 0)
                .value_at(points),
        );
    }

    /// Anticipation: increases your Defense Skill by 4/8/12/16/20.
    fn apply_anticipation(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("anticipation");
        if points == 0 {
            return;
        }
        sim.add_stat(
            unit,
            Stat::DefenseRating,
            spell_data().anticipation.value_at(points) * DEFENSE_RATING_PER_DEFENSE_LEVEL,
        );
    }

    /// Improved Righteous Fury: while Righteous Fury is active, all damage taken is reduced by
    /// 2/4/6%.
    fn apply_improved_righteous_fury(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("improved_righteous_fury");
        if points == 0 {
            return;
        }
        // The client states this as a negative percentage per rank: -2 / -4 / -6, so
        // multiplier_at gives 0.98 / 0.96 / 0.94 and the minus is never written here.
        let multiplier = Self::improved_righteous_fury_multiplier(sim, points);

        sim.on_spell_registered(
            unit,
            Rc::new(move |sim: &mut Sim, spell| {
                if sim.spell(spell).matches(masks::RIGHTEOUS_FURY) {
                    if let Some(buff) = sim.spell(spell).related_self_buff {
                        sim.attach_multiplicative_pseudo_stat_buff(
                            buff,
                            PseudoStatField::DamageTakenMultiplier,
                            multiplier,
                        );
                    }
                }
            }),
        );
    }

    /// Improved Righteous Fury's damage taken multiplier: -3/-6/-9% since push 113554 under
    /// Forever's rules, -2/-4/-6% in the reference.
    pub(in super::super) fn improved_righteous_fury_multiplier(sim: &Sim, points: i32) -> f64 {
        match sim.rules {
            Rules::Forever => 1.0 + hotfix::at(&hotfix::IMPROVED_RIGHTEOUS_FURY, points) / 100.0,
            Rules::GoReference => spell_data()
                .improved_righteous_fury
                .effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_EFFECT2)
                .multiplier_at(points),
        }
    }

    /// Shield Specialization: increases the amount of damage absorbed by your shield, and
    /// gives your blocks a chance to restore a share of your maximum Mana. May only occur once
    /// every 3 sec.
    fn apply_shield_specialization(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("shield_specialization");
        if points == 0 {
            return;
        }
        // Two ranks since push 113554 under Forever's rules: 15/30% block value.
        sim.unit_mut(unit).pseudo_stats.block_value_multiplier *= match sim.rules {
            Rules::Forever => {
                1.0 + hotfix::at(&hotfix::SHIELD_SPECIALIZATION_BLOCK_VALUE, points) / 100.0
            }
            Rules::GoReference => spell_data()
                .shield_specialization
                .effect(dbcenums::A_MOD_BLOCK_VALUE_PCT, 0)
                .multiplier_at(points),
        };

        let label = sim.unit(unit).label.clone();
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: format!("Shield Specialization{label}"),
                callback: CallbackMask::ON_SPELL_HIT_TAKEN,
                outcome: HitOutcome::BLOCK,
                proc_chance: Self::shield_specialization_chance(sim, points),
                icd: 3 * SECOND,
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );
    }

    /// Shield Specialization's mana chance: 50/100% since push 113554 under Forever's rules,
    /// 33/66/100% in the reference.
    pub(in super::super) fn shield_specialization_chance(sim: &Sim, points: i32) -> f64 {
        match sim.rules {
            Rules::Forever => hotfix::at(&hotfix::SHIELD_SPECIALIZATION_CHANCE, points) / 100.0,
            Rules::GoReference => spell_data()
                .shield_specialization
                .effect_at(2)
                .fraction_at(points),
        }
    }

    /// Sacred Duty: increases your total Stamina and reduces the cooldown of your Templar's
    /// Bulwark spell.
    fn apply_sacred_duty(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("sacred_duty");
        if points == 0 {
            return;
        }
        // Push 113554 trades the Stamina for a shorter Forbearance under Forever's rules, which
        // register_forbearance reads.
        if sim.rules == Rules::Forever {
            self.forbearance_reduction -=
                hotfix::millis(hotfix::at(&hotfix::SACRED_DUTY_FORBEARANCE_MS, points));
        } else {
            sim.unit_mut(unit).sdm.multiply_stat(
                Stat::Stamina,
                spell_data()
                    .sacred_duty
                    .effect(dbcenums::A_MOD_TOTAL_STAT_PERCENTAGE, 0)
                    .multiplier_at(points),
            );
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::TEMPLARS_BULWARK,
                kind: SpellModType::CooldownFlat,
                time_value: millis(spell_data().sacred_duty.effect_at(2).value_at(points)),
                ..SpellModConfig::default()
            },
        );
    }

    /// One-Handed Weapon Specialization: increases the damage you deal with one-handed melee
    /// weapons by 3/7/10%. The client puts it on the Physical school alone, but in game on client
    /// 70425 it raises the paladin's damage of every school except Consecration and Retribution
    /// Aura (MythicSim patch 143). Heals take none of it.
    fn apply_one_handed_weapon_specialization(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("one_handed_weapon_specialization");
        if points == 0 {
            return;
        }
        // Go's GetMainHandType answers one hand for everything but a two-hander.
        let one_hand = !self.main_hand_is_two_hand(sim, unit);
        let bonus = spell_data()
            .one_handed_weapon_specialization
            .fraction_at(points);
        let mut retribution_aura = Vec::new();
        spell_data()
            .retribution_aura
            .each(|_, rank| retribution_aura.push(rank.id));
        let retribution_aura = Rc::new(retribution_aura);
        let takes_it = move |sim: &Sim, spell: SpellId| {
            let spell = sim.spell(spell);
            !spell.flags.matches(SpellFlag::HELPFUL)
                // Go `OneHandedWeaponSpecializationSkipsHolyShock` (MythicSim patch 149).
                && spell.class_spell_mask
                    & (masks::CONSECRATION
                        | masks::RETRIBUTION_AURA
                        | masks::HOLY_SHOCK
                        | masks::HOLY_SHOCK_HEAL)
                    == 0
                && !retribution_aura.contains(&spell.action_id.spell_id)
        };
        // A damage done mod over the spells it takes rather than a custom one: an item swap (the
        // Manual Crowd Pummeler opener) carries every hand mod, and it carries only built-in kinds.
        let weapon_mod = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                // Go SpellSchoolPhysical | SpellSchoolChaos: every school.
                school: 0x7f,
                kind: SpellModType::DamageDonePct,
                float_value: bonus,
                spell_filter: Some(Rc::new(takes_it)),
                ..SpellModConfig::default()
            },
        );
        follow_main_hand(sim, unit, weapon_mod, one_hand, "HandTypeOneHand");
    }

    /// Reckoning: gives you a chance to gain an extra attack after Blocking a melee attack and
    /// after being the victim of a non-periodic critical strike.
    fn apply_reckoning(&mut self, sim: &mut Sim, unit: UnitId) {
        /// The most Reckoning charges a paladin holds (16 October 2026 beta notes).
        const RECKONING_MAX_CHARGES: i32 = 4;
        let points = self.talents.i32("reckoning");
        if points == 0 {
            return;
        }
        let block_chance = spell_data().reckoning.fraction_at(points);
        let crit_chance = block_chance * 2.5;
        let label = sim.unit(unit).label.clone();

        // 16 Oct build: each proc banks a charge (max 4) and the next melee swing (auto attack)
        // spends them all, 1 extra attack each, a free swing that leaves the swing timer alone. Client
        // 70291+ gives 20177 a 1.5 s proc cooldown, shared by both triggers.
        sim.new_timer(unit);
        sim.register_aura(
            unit,
            AuraConfig {
                label: format!("Reckoning{label}"),
                action_id: Some(spell_action(20178)),
                duration: NEVER_EXPIRES,
                max_stacks: RECKONING_MAX_CHARGES,
                events: EventCallbacks {
                    on_spell_hit_dealt: true,
                    ..EventCallbacks::default()
                },
                ..AuraConfig::default()
            },
        );

        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: format!("Reckoning - Block{label}"),
                callback: CallbackMask::ON_SPELL_HIT_TAKEN,
                outcome: HitOutcome::BLOCK,
                proc_chance: block_chance,
                ..ProcTrigger::default()
            },
        );
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: format!("Reckoning - Crit{label}"),
                callback: CallbackMask::ON_SPELL_HIT_TAKEN,
                outcome: HitOutcome::CRIT,
                proc_chance: crit_chance,
                ..ProcTrigger::default()
            },
        );
    }

    /// Iron Creed's damage taken multiplier: -2/-4/-6% over three ranks since push 113554 under
    /// Forever's rules (the client states it negative), 2-10% over five in the reference.
    pub(in super::super) fn iron_creed_multiplier(sim: &Sim, points: i32) -> f64 {
        match sim.rules {
            Rules::Forever => 1.0 + hotfix::at(&hotfix::IRON_CREED_DAMAGE_TAKEN, points) / 100.0,
            Rules::GoReference => {
                1.0 - spell_data()
                    .iron_creed
                    .effect(dbcenums::A_PROC_TRIGGER_SPELL_WITH_VALUE, 0)
                    .fraction_at(points)
            }
        }
    }

    /// Shieldsworn (1328544, push 113554): reduces the cooldown of your Holy Strike by 4 sec.
    /// Holy Strike now strikes with your shield, triggering your active Holy Shield and Shield
    /// Specialization without consuming their charges or triggering their cooldowns, and lets you
    /// block spells for 6 sec. The triggers are the runtime's (`talents/protection.rs`); the sim
    /// lets only melee and ranged attacks be blocked, so the spell block is not modelled, as in
    /// upstream #772.
    fn apply_shieldsworn(&mut self, sim: &mut Sim, unit: UnitId) {
        if self.new_talent("shieldsworn") == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::HOLY_STRIKE,
                kind: SpellModType::CooldownFlat,
                time_value: hotfix::SHIELDSWORN_HOLY_STRIKE_COOLDOWN,
                ..SpellModConfig::default()
            },
        );
    }

    /// Iron Creed: increases the threat generated by your Holy Strike ability. While
    /// Righteous Fury is active, Holy Strike also reduces your damage taken for 6 sec.
    fn apply_iron_creed(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("iron_creed");
        if points == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::HOLY_STRIKE,
                kind: SpellModType::ThreatMultiplierPct,
                // Three ranks of 10% since push 113554 under Forever's rules (was 5 of 5%).
                float_value: match sim.rules {
                    Rules::Forever => hotfix::at(&hotfix::IRON_CREED_THREAT, points) / 100.0,
                    Rules::GoReference => spell_data()
                        .iron_creed
                        .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_THREAT)
                        .fraction_at(points),
                },
                ..SpellModConfig::default()
            },
        );

        let rank = spell_data().iron_creed_triggered.highest();
        let reduction = 1.0 - Self::iron_creed_multiplier(sim, points);
        let label = sim.unit(unit).label.clone();

        let iron_creed = sim.register_aura(
            unit,
            AuraConfig {
                label: format!("Iron Creed{label}"),
                action_id: Some(spell_action(rank.id)),
                duration: rank.duration(),
                ..AuraConfig::default()
            },
        );
        sim.attach_multiplicative_pseudo_stat_buff(
            iron_creed,
            PseudoStatField::DamageTakenMultiplier,
            1.0 - reduction,
        );

        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: format!("Iron Creed - Trigger{label}"),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                class_spell_mask: masks::HOLY_STRIKE,
                outcome: HitOutcome::LANDED,
                ..ProcTrigger::default()
            },
        );
    }
}
