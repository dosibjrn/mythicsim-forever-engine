//! Go sim/rogue/talents_assassination.go.

use std::rc::Rc;

use crate::prepare::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use crate::prepare::character::constants::{CHARACTER_LEVEL, MAX_MELEE_RANGE};
use crate::prepare::character::{cooldown_type, MajorCooldown};
use crate::prepare::dbcenums;
use crate::prepare::env::Environment;
use crate::prepare::sim::Cooldown;
use crate::prepare::sim::{AuraConfig, EventCallbacks, Rules, Sim, UnitId, MILLISECOND};
use crate::prepare::spell::{Cast, CastConfig, ProcMask, SpellConfig, SpellFlag};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::stats::Stat;

use super::super::masks;
use super::super::rework;
use super::super::spell_data::spell_data;
use super::super::spells::{BUILDER, FINISHER};
use super::super::util::{
    energy_cost, ignore_haste_cast, longest_cooldown, mutilate_weapon_share, spell_action,
    tagged_action,
};
use super::super::{Rogue, SLICE_AND_DICE_DURATIONS};

impl Rogue {
    /// Go `registerAssassinationTalents`.
    pub(in super::super) fn register_assassination_talents(&mut self, sim: &mut Sim, unit: UnitId) {
        // Tier 1
        self.register_improved_eviscerate(sim, unit);
        // Remorseless Attacks models nothing in Go either.
        self.register_malice(sim, unit);

        // Tier 2
        // Ruthlessness is implemented in ApplyFinisher.
        self.register_murder(unit);
        self.register_puncturing_wounds(sim, unit);

        // Tier 3
        // Relentless Strikes is implemented in ApplyFinisher.
        self.register_improved_expose_armor(sim, unit);
        self.register_lethality(sim, unit);

        // Tier 4
        self.register_vile_poisons(sim, unit);
        // Improved Poisons is implemented in poisons.go.

        // Tier 5
        self.register_cold_blood(sim, unit);
        // Improved Kidney Shot is implemented in kidney_shot.go.

        // Tier 6
        self.register_seal_fate(sim, unit);

        // Tier 7
        // Vigor is implemented in rogue.go.
        self.register_venom(sim, unit);

        // Tier 9
        self.register_mutilate(sim, unit);
    }

    fn register_improved_eviscerate(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("improved_eviscerate");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::EVISCERATE,
                kind: SpellModType::DamageDoneFlat,
                float_value: spell_data().improved_eviscerate.fraction_at(rank),
                ..SpellModConfig::default()
            },
        );
    }

    /// Malice covers Poisons in Forever, and those roll against the spell hit table.
    fn register_malice(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("malice");
        if rank == 0 {
            return;
        }
        let crit = spell_data().malice.value_at(rank);
        sim.add_stat(unit, Stat::PhysicalCritPercent, crit);
        sim.add_stat(unit, Stat::SpellCritPercent, crit);
    }

    /// 14158 is MOD_DAMAGE_DONE_VERSUS on creature mask 80, Humanoid and Giant, with no crit
    /// damage part.
    fn register_murder(&mut self, unit: UnitId) {
        let rank = self.talents.i32("murder");
        if rank == 0 {
            return;
        }
        let multiplier = spell_data().murder.multiplier_at(rank);
        self.post_finalize
            .push(Rc::new(move |env: &mut Environment| {
                let attacker = env.sim.unit(unit).unit_index as usize;
                for defender in env.sim.all_units() {
                    let mob_type = env.sim.unit(defender).mob_type.as_str();
                    if mob_type == "MobTypeHumanoid" || mob_type == "MobTypeGiant" {
                        let index = env.sim.unit(defender).unit_index as usize;
                        env.attack_tables[attacker][index].damage_dealt_multiplier *= multiplier;
                    }
                }
            }));
    }

    /// Effect 2 is the combo point trigger, handled in backstab.go. The two crit modifiers share
    /// an aura and misc pair, so each has to be named by position: 1 is Backstab, 3 is Mutilate.
    fn register_puncturing_wounds(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("puncturing_wounds");
        if rank == 0 {
            return;
        }
        let data = &spell_data().puncturing_wounds;
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::BonusCritPercent,
                class_mask: masks::BACKSTAB,
                float_value: data.effect_at(1).value_at(rank),
                ..SpellModConfig::default()
            },
        );
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::BonusCritPercent,
                class_mask: masks::MUTILATE | masks::MUTILATE_HIT,
                float_value: data.effect_at(3).value_at(rank),
                ..SpellModConfig::default()
            },
        );
    }

    /// Forever repurposes Improved Expose Armor: the energy discount is a SpellMod, and the
    /// combo points it hands back on a full spend live in expose_armor.go.
    fn register_improved_expose_armor(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("improved_expose_armor");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::PowerCostFlat,
                class_mask: masks::EXPOSE_ARMOR,
                int_value: spell_data()
                    .improved_expose_armor
                    .effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_COST)
                    .value_at(rank) as i32,
                ..SpellModConfig::default()
            },
        );

        // A dummy aura so the APL can ask whether the talent is taken.
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Improved Expose Armor".to_string(),
                action_id: Some(spell_action(
                    spell_data().improved_expose_armor.highest().id,
                )),
                ..AuraConfig::default()
            },
        );
        sim.make_permanent(aura);
    }

    fn register_lethality(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("lethality");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::CritMultiplierFlat,
                class_mask: masks::LETHALITY,
                float_value: rework::talent_ladder(sim.rules, 14128, 2, 5).fraction_at(rank),
                ..SpellModConfig::default()
            },
        );
    }

    fn register_vile_poisons(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("vile_poisons");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::DamageDoneFlat,
                class_mask: masks::POISONS,
                float_value: spell_data()
                    .vile_poisons
                    .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_DAMAGE)
                    .fraction_at(rank),
                ..SpellModConfig::default()
            },
        );
    }

    fn register_cold_blood(&mut self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("cold_blood") {
            return;
        }
        let rank = spell_data().cold_blood.highest();
        let action = spell_action(rank.id);

        let aura = sim.get_or_register_aura(
            unit,
            AuraConfig {
                label: "Cold Blood".to_string(),
                action_id: Some(action.clone()),
                duration: crate::prepare::sim::NEVER_EXPIRES,
                events: EventCallbacks {
                    on_spell_hit_dealt: true,
                    ..EventCallbacks::default()
                },
                ..AuraConfig::default()
            },
        );
        sim.attach_spell_mod(
            aura,
            SpellModConfig {
                kind: SpellModType::BonusCritPercent,
                class_mask: masks::COLD_BLOODED,
                float_value: rank
                    .effect(
                        dbcenums::A_ADD_FLAT_MODIFIER,
                        dbcenums::SPELLMOD_CRITICAL_CHANCE,
                    )
                    .average(CHARACTER_LEVEL),
                ..SpellModConfig::default()
            },
        );

        let timer = sim.new_timer(unit);
        let spell = sim.get_or_register_spell(
            unit,
            SpellConfig {
                action_id: action,
                flags: SpellFlag::APL,
                class_spell_mask: masks::COLD_BLOOD,
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ignore_haste: true,
                    ..CastConfig::default()
                },
                ..SpellConfig::default()
            },
        );
        self.spells.cold_blood = Some(spell);
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

    fn register_seal_fate(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("seal_fate");
        if rank == 0 {
            return;
        }
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Seal Fate Trigger".to_string(),
                action_id: spell_action(spell_data().seal_fate.highest().id),
                // Forever puts the real per-rank chance on the effect; the row's ProcChance
                // reads a flat 100%.
                proc_chance: rework::talent_ladder(sim.rules, 14186, 3, 5).fraction_at(rank),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                outcome: HitOutcome::CRIT,
                spell_flags: BUILDER,
                icd: 500 * MILLISECOND,
                ..ProcTrigger::default()
            },
        );
    }

    /// Venom is a finisher that raises poison damage and application chance for a combo point
    /// scaled duration, on the same 9-21 second ladder as Slice and Dice. Under Forever's rules
    /// the finisher is gone (the notes of 2 November 2026): the talent (1329328) makes Shank
    /// cast the aura of 1310703, 30% poison damage and 20% poison chance for 6 sec, so only the
    /// aura is registered.
    fn register_venom(&mut self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("venom") {
            return;
        }
        let forever = sim.rules == Rules::Forever;
        let rank = spell_data().venom.highest();
        let action = spell_action(rank.id);
        let state = Rc::clone(&self.state);

        let (damage_bonus, chance_bonus) = if forever {
            (rework::VENOM_DAMAGE_BONUS, rework::VENOM_CHANCE_BONUS)
        } else {
            (
                rank.effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_DAMAGE)
                    .average(CHARACTER_LEVEL)
                    / 100.0,
                rank.effect(
                    dbcenums::A_ADD_FLAT_MODIFIER,
                    dbcenums::SPELLMOD_CHANCE_OF_SUCCESS,
                )
                .average(CHARACTER_LEVEL)
                    / 100.0,
            )
        };

        let gain_state = Rc::clone(&state);
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Venom".to_string(),
                action_id: Some(action.clone()),
                // Overridden on cast; a non-zero default keeps an APL prepull from crashing.
                duration: if forever {
                    rework::VENOM_DURATION
                } else {
                    SLICE_AND_DICE_DURATIONS[5]
                },
                on_gain: Some(Rc::new(move |_: &mut Sim, _| {
                    gain_state
                        .additive_poison_bonus_chance
                        .set(gain_state.additive_poison_bonus_chance.get() + chance_bonus);
                })),
                on_expire: Some(Rc::new(move |_: &mut Sim, _| {
                    state
                        .additive_poison_bonus_chance
                        .set(state.additive_poison_bonus_chance.get() - chance_bonus);
                })),
                ..AuraConfig::default()
            },
        );
        sim.attach_spell_mod(
            aura,
            SpellModConfig {
                kind: SpellModType::DamageDoneFlat,
                class_mask: masks::POISONS,
                float_value: damage_bonus,
                ..SpellModConfig::default()
            },
        );
        self.auras.venom = Some(aura);
        if forever {
            return;
        }

        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id: action,
                flags: FINISHER | SpellFlag::APL,
                metric_splits: 6,
                class_spell_mask: masks::VENOM,
                cost: energy_cost(rank, false),
                cast: ignore_haste_cast(rank.gcd()),
                has_extra_cast_condition: true,
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
        self.spells.venom = Some(spell);
    }

    fn register_mutilate(&mut self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("mutilate") {
            return;
        }
        // Was 34413; Forever reworked Mutilate onto an entirely new set of spell ids, so this
        // follows the highest rank the client actually ships.
        let rank = spell_data().mutilate.highest();
        let spell_id = rank.id;

        // The client splits Mutilate into a parent and two triggered hit spells: the main hand
        // hit first, then the off hand's.
        for (tag, proc_mask) in [
            (1, ProcMask::MELEE_MH_SPECIAL),
            (2, ProcMask::MELEE_OH_SPECIAL),
        ] {
            sim.register_spell(
                unit,
                SpellConfig {
                    action_id: tagged_action(spell_id, tag),
                    spell_school: rank.spell_school(),
                    defense_type: rank.defense_type_core(),
                    proc_mask,
                    flags: SpellFlag::MELEE_METRICS | BUILDER,
                    class_spell_mask: masks::MUTILATE_HIT,
                    // Every hit rank states the same weapon share: 75%, or 90% under Forever's
                    // rules since the notes of 2 November 2026.
                    damage_multiplier: mutilate_weapon_share(sim.rules),
                    damage_multiplier_additive: 1.0,
                    threat_multiplier: 1.0,
                    ..SpellConfig::default()
                },
            );
        }

        let mutilate = sim.register_spell(
            unit,
            SpellConfig {
                action_id: tagged_action(spell_id, 0),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                // The cast deals no damage; its two hand strikes do, and they are what poisons
                // and on-hit effects roll on.
                flags: SpellFlag::MELEE_METRICS | SpellFlag::APL | SpellFlag::NO_ON_DAMAGE_DEALT,
                class_spell_mask: masks::MUTILATE,
                max_range: MAX_MELEE_RANGE,
                cost: energy_cost(rank, true),
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: rank.gcd(),
                        ..Cast::default()
                    },
                    ignore_haste: true,
                    ..CastConfig::default()
                },
                // Every rank, and both hand strikes, require a Dagger in the beta client.
                has_extra_cast_condition: true,
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        );
        self.spells.mutilate = Some(mutilate);
    }
}
