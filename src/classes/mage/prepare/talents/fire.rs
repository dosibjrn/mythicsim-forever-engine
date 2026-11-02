//! Go sim/mage/talents_fire.go.

use std::rc::Rc;

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use crate::prepare::dbcenums;
use crate::prepare::sim::{AuraConfig, EventCallbacks, Rules, Sim, UnitId, SECOND};
use crate::prepare::spell::{school, DefenseType, DotConfig, ProcMask, SpellConfig, SpellFlag};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};

use super::super::masks;
use super::super::spell_data::spell_data;
use super::super::Mage;
use super::millis;

impl Mage {
    /// Go `registerFireTalents`.
    pub(super) fn register_fire_talents(&self, sim: &mut Sim, unit: UnitId) {
        // Tier 1
        self.register_improved_fire_blast(sim, unit);
        self.register_incineration(sim, unit);
        self.register_improved_fireball(sim, unit);

        // Tier 2: Impact models nothing in Go either.
        self.register_ignite(sim, unit);
        self.register_flame_throwing(sim, unit);

        // Tier 3
        self.register_burning_soul(sim, unit);
        self.register_improved_flamestrike(sim, unit);
        // Pyroblast: pyroblast.go

        // Tier 4: Improved Scorch is in scorch.go, Improved Fire Ward models nothing.
        self.register_hot_streak(sim, unit);
        self.register_master_of_elements(sim, unit);

        // Tier 5
        self.register_critical_mass(sim, unit);
        // Blast Wave: blast_wave.go

        // Tier 6
        self.register_fire_power(sim, unit);

        // Tier 7: Combustion is in combustion.go.
    }

    /// Improved Fire Blast (11078, called Wake of Fire until client 70425): the Fire Blast
    /// cooldown. Its range bonus changes no number.
    fn register_improved_fire_blast(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("improved_fire_blast");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::FIRE_BLAST,
                time_value: millis(
                    spell_data()
                        .improved_fire_blast
                        .effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_COOLDOWN)
                        .value_at(rank),
                ),
                kind: SpellModType::CooldownFlat,
                ..SpellModConfig::default()
            },
        );
    }

    /// Flame Throwing, new in Forever: 11100 adds 3/6 yards to the range of every targeted Fire
    /// spell of the mage's, Blast Wave not among them.
    fn register_flame_throwing(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("flame_throwing");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::FIRE_BLAST
                    | masks::FIREBALL
                    | masks::FLAMESTRIKE
                    | masks::FROSTFIRE_BOLT
                    | masks::PYROBLAST
                    | masks::SCORCH,
                float_value: spell_data()
                    .flame_throwing
                    .effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_RANGE)
                    .value_at(rank),
                kind: SpellModType::RangeFlat,
                ..SpellModConfig::default()
            },
        );
    }

    /// Arcane Blast and Ice Lance as well as TBC's Fire Blast and Scorch. Five ranks under
    /// Forever's rules since client 1.60.1.70707, three in the Go reference.
    fn register_incineration(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("incineration");
        if rank == 0 {
            return;
        }
        let data = spell_data();
        let ladder = match sim.rules {
            Rules::Forever => &data.incineration_70707,
            Rules::GoReference => &data.incineration,
        };
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::ARCANE_BLAST
                    | masks::FIRE_BLAST
                    | masks::ICE_LANCE
                    | masks::SCORCH,
                float_value: ladder.value_at(rank),
                kind: SpellModType::BonusCritPercent,
                ..SpellModConfig::default()
            },
        );
    }

    fn register_improved_fireball(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("improved_fireball");
        if rank == 0 {
            return;
        }
        // Client mask 11069: Fireball and Frostfire Bolt. Client 1.60.1.70707 adds bit 32768 of
        // the second word (SpellEffect 688786), Arcane Wrath and Arcane Barrage; the Go reference
        // keeps the two bolts.
        let mut class_mask = masks::FIREBALL | masks::FROSTFIRE_BOLT;
        if sim.rules == Rules::Forever {
            class_mask |= masks::ARCANE_WRATH;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask,
                time_value: millis(
                    spell_data()
                        .improved_fireball
                        .effect(
                            dbcenums::A_ADD_FLAT_MODIFIER,
                            dbcenums::SPELLMOD_CASTING_TIME,
                        )
                        .value_at(rank),
                ),
                kind: SpellModType::CastTimeFlat,
                ..SpellModConfig::default()
            },
        );
    }

    /// Ignite pays out a share of the fire critical strike that lit it over its ticks.
    fn register_ignite(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("ignite");
        if rank == 0 {
            return;
        }
        let data = spell_data();
        let ignite_rank = data.ignite_triggered.highest();
        let tick_length = 2 * SECOND;
        let number_of_ticks = (ignite_rank.duration() / tick_length) as i32;

        sim.register_spell(
            unit,
            SpellConfig {
                action_id: ActionId {
                    spell_id: ignite_rank.id,
                    ..ActionId::default()
                },
                spell_school: school::FIRE,
                defense_type: DefenseType::Magic,
                proc_mask: ProcMask::SPELL_DAMAGE,
                class_spell_mask: masks::IGNITE,
                flags: SpellFlag::IGNORE_MODIFIERS
                    | SpellFlag::NO_SPELL_MODS
                    | SpellFlag::NO_ON_CAST_COMPLETE
                    | SpellFlag::IGNORE_RESISTS
                    | SpellFlag::PROC,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                dot: DotConfig {
                    aura: AuraConfig {
                        label: "Ignite".to_string(),
                        ..AuraConfig::default()
                    },
                    number_of_ticks,
                    tick_length,
                    ..DotConfig::default()
                },
                ..SpellConfig::default()
            },
        );

        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Ignite Talent".to_string(),
                // Forever's 11119 lacks the bit (Era's ranks carry it).
                can_proc_from_procs: data.ignite.highest().can_proc_from_procs(),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                proc_mask: ProcMask::SPELL_DAMAGE,
                outcome: HitOutcome::CRIT,
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );
    }

    /// The threat half only; the pushback protection has nothing to act on in the sim.
    fn register_burning_soul(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("burning_soul");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::ALL,
                school: school::FIRE,
                float_value: spell_data()
                    .burning_soul
                    .effect(dbcenums::A_MOD_THREAT, 4)
                    .fraction_at(rank),
                kind: SpellModType::ThreatMultiplierPct,
                ..SpellModConfig::default()
            },
        );
    }

    fn register_improved_flamestrike(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("improved_flamestrike");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::FLAMESTRIKE,
                float_value: spell_data().improved_flamestrike.value_at(rank),
                kind: SpellModType::BonusCritPercent,
                ..SpellModConfig::default()
            },
        );
    }

    /// Heating Up: Fireball, Frostfire Bolt, Fire Blast and Scorch crits each take 25% off
    /// Pyroblast's cast time, stacking 3 times.
    fn register_hot_streak(&self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("heating_up") {
            return;
        }
        let buff = spell_data().heating_up_triggered.highest();
        let per_stack = buff
            .effect(
                dbcenums::A_ADD_PCT_MODIFIER,
                dbcenums::SPELLMOD_CASTING_TIME,
            )
            .percent();

        let cast_time_mod = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                class_mask: masks::PYROBLAST,
                kind: SpellModType::CastTimePct,
                ..SpellModConfig::default()
            },
        );

        sim.register_aura(
            unit,
            AuraConfig {
                label: "Heating Up".to_string(),
                action_id: Some(ActionId {
                    spell_id: buff.id,
                    ..ActionId::default()
                }),
                duration: buff.duration(),
                max_stacks: i32::from(buff.max_stack),
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.activate_spell_mod(cast_time_mod)
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.deactivate_spell_mod(cast_time_mod)
                })),
                on_stacks_change: Some(Rc::new(move |sim: &mut Sim, _, _, new_stacks| {
                    sim.update_spell_mod_float_value(
                        cast_time_mod,
                        per_stack * f64::from(new_stacks),
                    )
                })),
                events: EventCallbacks {
                    on_cast_complete: true,
                    ..EventCallbacks::default()
                },
                ..AuraConfig::default()
            },
        );

        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Heating Up Trigger".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                class_spell_mask: masks::FIREBALL
                    | masks::FROSTFIRE_BOLT
                    | masks::FIRE_BLAST
                    | masks::SCORCH,
                outcome: HitOutcome::CRIT,
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );
        // Since the beta build of 2026-10-16 every Fire Blast cast adds a stack on top of its
        // crit's (community #808).
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Hot Streak Fire Blast Trigger".to_string(),
                callback: CallbackMask::ON_CAST_COMPLETE,
                class_spell_mask: masks::FIRE_BLAST,
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );
    }

    fn register_master_of_elements(&self, sim: &mut Sim, unit: UnitId) {
        if self.talents.i32("master_of_elements") == 0 {
            return;
        }
        // 29074's 9 ms ProcCategoryRecovery: an area spell that crits several targets refunds
        // once.
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Master of Elements".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                class_spell_mask: masks::ALL,
                outcome: HitOutcome::CRIT,
                icd: spell_data().master_of_elements.highest().icd(),
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );
    }

    /// Three ranks under Forever's rules since client 1.60.1.70707, five in the Go reference;
    /// the first three keep their 2/4/6.
    fn register_critical_mass(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("critical_mass");
        if rank == 0 {
            return;
        }
        let data = spell_data();
        let ladder = match sim.rules {
            Rules::Forever => &data.critical_mass_70707,
            Rules::GoReference => &data.critical_mass,
        };
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::ALL,
                school: school::FIRE,
                float_value: ladder.value_at(rank),
                kind: SpellModType::BonusCritPercent,
                ..SpellModConfig::default()
            },
        );
    }

    /// Fire Power (client 70425): 1/2/3% more damage for Fire spells per Fire effect the mage has
    /// on the target, up to four, a damage done by caster handler on every attack table that the
    /// fight runs (the `fire_power` effect). Preparation keeps no state of it.
    fn register_fire_power(&self, _sim: &mut Sim, _unit: UnitId) {}
}
