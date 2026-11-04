//! Go sim/rogue/talents_subtlety.go.

use std::rc::Rc;

use crate::prepare::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use crate::prepare::character::constants::{
    CHARACTER_LEVEL, DODGE_RATING_PER_DODGE_PERCENT, MAX_MELEE_RANGE,
};
use crate::prepare::character::{cooldown_type, MajorCooldown};
use crate::prepare::dbcenums;
use crate::prepare::sim::{
    AuraConfig, Cooldown, EventCallbacks, Rules, Sim, UnitId, NEVER_EXPIRES, SECOND,
};
use crate::prepare::spell::{Cast, CastConfig, ProcMask, SpellConfig, SpellFlag};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::spelldata::Spell as Row;

use super::super::masks;
use super::super::rework;
use super::super::spell_data::spell_data;
use super::super::spells::BUILDER;
use super::super::util::{
    aura_array_map, energy_cost, has_dagger, ignore_haste_cast, longest_cooldown,
    new_enemy_aura_array, spell_action, Hand,
};
use super::super::Rogue;
use super::millis;

/// Go `hemorrhageRank`: Hemorrhage has no rank subtext, so the generator gives it one row.
fn hemorrhage_rank() -> &'static Row {
    spell_data().hemorrhage.by_id(16511)
}

impl Rogue {
    /// Go `registerSubtletyTalents`.
    pub(in super::super) fn register_subtlety_talents(&mut self, sim: &mut Sim, unit: UnitId) {
        // Tier 1: Master of Deception models nothing in Go either.
        self.register_opportunity(sim, unit);

        // Tier 2: Setup and Camouflage model nothing in Go either.

        // Tier 3
        self.register_initiative(sim, unit);
        self.register_ghostly_strike(sim, unit);
        self.register_improved_ambush(sim, unit);
        // Improved Distract models nothing in Go either.

        // Tier 4
        self.register_elusiveness(sim, unit);
        self.register_serrated_blades(sim, unit);
        // Dirty Tricks models nothing in Go either.

        // Tier 5
        // Heightened Senses models nothing in Go either.
        self.register_preparation(sim, unit);
        self.register_dirty_deeds(sim, unit);
        self.register_hemorrhage(sim, unit);

        // Tier 6
        self.register_quietus(sim, unit);
        self.register_cutthroat(sim, unit);

        // Tier 7
        self.register_premeditation(sim, unit);
        self.register_thousand_cuts(sim, unit);
        self.register_death_mark(sim, unit);
    }

    /// Death Mark (the rework of push 113483, Forever's rules only): a finishing move for 25
    /// Energy and a Combo Point or more, 10 sec cooldown and 20 yd range, that marks the target
    /// for 6 sec. The fight stores a share of the damage the rogue deals to it and releases it
    /// when the mark ends. A level 60 rogue knows rank 4; the lower ranks press it.
    fn register_death_mark(&mut self, sim: &mut Sim, unit: UnitId) {
        if sim.rules != Rules::Forever || self.extra_points("death_mark") == 0 {
            return;
        }
        let action = spell_action(rework::DEATH_MARK_SPELL_ID);
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Death Mark".to_string(),
                action_id: Some(action.clone()),
                duration: rework::DEATH_MARK_DURATION,
                events: EventCallbacks {
                    on_spell_hit_dealt: true,
                    on_periodic_damage_dealt: true,
                    ..EventCallbacks::default()
                },
                ..AuraConfig::default()
            },
        );
        self.auras.death_mark = Some(aura);
        let timer = sim.new_timer(unit);
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id: action,
                spell_school: crate::prepare::spell::school::PHYSICAL,
                flags: super::super::spells::FINISHER | SpellFlag::APL,
                metric_splits: 6,
                class_spell_mask: masks::DEATH_MARK,
                max_range: rework::DEATH_MARK_RANGE,
                cost: crate::prepare::spell::CostOptions {
                    energy_cost: rework::DEATH_MARK_ENERGY_COST,
                    energy_refund: 0.0,
                    ..crate::prepare::spell::CostOptions::default()
                },
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: rework::DEATH_MARK_COOLDOWN,
                    },
                    ..ignore_haste_cast(SECOND)
                },
                has_extra_cast_condition: true,
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
        sim.spell_mut(spell).rank_aliases = rework::DEATH_MARK_LOWER_RANKS
            .iter()
            .map(|id| spell_action(*id))
            .collect();
        self.spells.death_mark = Some(spell);
    }

    fn register_opportunity(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("opportunity");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::DamageDoneFlat,
                class_mask: masks::BACKSTAB
                    | masks::MUTILATE
                    | masks::MUTILATE_HIT
                    | masks::AMBUSH
                    | masks::GARROTE,
                float_value: rework::talent_ladder(sim.rules, 14057, 5, 2)
                    .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_DAMAGE)
                    .fraction_at(rank),
                ..SpellModConfig::default()
            },
        );
    }

    fn register_initiative(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("initiative");
        if rank == 0 {
            return;
        }
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Initiative Trigger".to_string(),
                action_id: spell_action(spell_data().initiative.highest().id),
                // The beta rounds rank 2 up to 67% rather than doubling rank 1's 33%; the
                // row's ProcChance would read the flat 100 the talent spell carries.
                proc_chance: spell_data().initiative.fraction_at(rank),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                outcome: HitOutcome::LANDED,
                class_spell_mask: masks::GARROTE | masks::AMBUSH,
                ..ProcTrigger::default()
            },
        );
    }

    fn register_ghostly_strike(&mut self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("ghostly_strike") {
            return;
        }
        let rank = spell_data().ghostly_strike.highest();
        let action = spell_action(rank.id);
        let data = &spell_data().ghostly_strike;

        // Effect 1 is the plain weapon share, effect 4 the larger one a dagger gets.
        let mut weapon_damage = data.effect_at(1).value_at(1) / 100.0;
        if has_dagger(sim, unit, Hand::Main) {
            weapon_damage = data.effect_at(4).value_at(1) / 100.0;
        }

        let dodge_aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Ghostly Strike Buff".to_string(),
                action_id: Some(action.clone()),
                duration: rank.duration(),
                ..AuraConfig::default()
            },
        );
        sim.attach_stat_buff(
            dodge_aura,
            crate::prepare::stats::Stat::DodgeRating,
            rank.effect(dbcenums::A_MOD_DODGE_PERCENT, 0)
                .average(CHARACTER_LEVEL)
                * DODGE_RATING_PER_DODGE_PERCENT,
        );

        // The notes of 2 November 2026 (Forever's rules, 1329326): the next Ambush or Garrote
        // within 7 sec needs no Stealth, which is what Cutthroat gave Ambush.
        if sim.rules == Rules::Forever {
            let ready = sim.register_aura(
                unit,
                AuraConfig {
                    label: "Ghostly Strike Ready".to_string(),
                    action_id: Some(spell_action(rework::GHOSTLY_STRIKE_READY_SPELL_ID)),
                    duration: 7 * SECOND,
                    ..AuraConfig::default()
                },
            );
            self.auras.ghostly_ready = Some(ready);
        }

        let timer = sim.new_timer(unit);
        let spell = sim.get_or_register_spell(
            unit,
            SpellConfig {
                action_id: action,
                class_spell_mask: masks::GHOSTLY_STRIKE,
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                flags: SpellFlag::APL | SpellFlag::MELEE_METRICS | BUILDER,
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                max_range: MAX_MELEE_RANGE,
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
                cost: energy_cost(rank, true),
                damage_multiplier: weapon_damage,
                damage_multiplier_additive: 1.0,
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        );
        self.spells.ghostly_strike = Some(spell);
    }

    fn register_improved_ambush(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("improved_ambush");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::BonusCritPercent,
                class_mask: masks::AMBUSH,
                float_value: spell_data().improved_ambush.value_at(rank),
                ..SpellModConfig::default()
            },
        );
    }

    fn register_elusiveness(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("elusiveness");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::CooldownFlat,
                class_mask: masks::VANISH,
                time_value: millis(
                    spell_data()
                        .elusiveness
                        .effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_COOLDOWN)
                        .value_at(rank),
                ),
                ..SpellModConfig::default()
            },
        );
    }

    /// Serrated Blades ignores a share of the target's Armor rather than a flat amount, and
    /// raises the rogue's own Rupture.
    fn register_serrated_blades(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("serrated_blades");
        if rank == 0 {
            return;
        }
        let data = &spell_data().serrated_blades;
        self.add_armor_ignore(unit, data.effect_at(1).value_at(rank) / 100.0);
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::DamageDoneFlat,
                class_mask: masks::RUPTURE,
                float_value: data
                    .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_DOT)
                    .fraction_at(rank),
                ..SpellModConfig::default()
            },
        );
    }

    fn register_preparation(&mut self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("preparation") {
            return;
        }
        let rank = spell_data().preparation.highest();
        let timer = sim.new_timer(unit);
        let spell = sim.get_or_register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                flags: SpellFlag::APL,
                class_spell_mask: masks::PREPARATION,
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
                ..SpellConfig::default()
            },
        );
        self.spells.preparation = Some(spell);
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

    /// Forever states only an energy discount on Dirty Deeds; the Classic damage bonus below 35%
    /// health is gone. It also drops Garrote's positional requirement, handled in garrote.go.
    fn register_dirty_deeds(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("dirty_deeds");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::PowerCostFlat,
                class_mask: masks::GARROTE,
                int_value: spell_data()
                    .dirty_deeds
                    .effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_COST)
                    .value_at(rank) as i32,
                ..SpellModConfig::default()
            },
        );
    }

    fn register_hemorrhage(&mut self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("hemorrhage") {
            return;
        }
        let rank = hemorrhage_rank();
        let action = spell_action(rank.id);

        // Hemorrhage no longer weakens the target for the whole raid: effect 2 makes it take
        // more of the rogue's own Rupture (mask 0x100000), a damage-taken effect on the target.
        let label = format!("Hemorrhage-{}", sim.unit(unit).label);
        let auras = new_enemy_aura_array(sim, |sim, target| {
            let aura = sim.get_or_register_aura(
                target,
                AuraConfig {
                    label: label.clone(),
                    action_id: Some(action.clone()),
                    duration: rank.duration(),
                    ..AuraConfig::default()
                },
            );
            sim.attach_ddbc(aura, 0, 1, unit)
        });
        let related = aura_array_map(sim, &auras);
        self.auras.hemorrhage = auras;

        // The plain weapon share is effect 4 (100%); effect 5 is the larger one a dagger gets.
        let data = &spell_data().hemorrhage;
        let mut weapon_damage = data.effect_at(4).value_at(1) / 100.0;
        if has_dagger(sim, unit, Hand::Main) {
            weapon_damage = data.effect_at(5).value_at(1) / 100.0;
        }

        let spell = sim.get_or_register_spell(
            unit,
            SpellConfig {
                action_id: action,
                class_spell_mask: masks::HEMORRHAGE,
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                flags: SpellFlag::APL | SpellFlag::MELEE_METRICS | BUILDER,
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                max_range: MAX_MELEE_RANGE,
                cast: ignore_haste_cast(rank.gcd()),
                cost: energy_cost(rank, true),
                damage_multiplier: weapon_damage,
                damage_multiplier_additive: 1.0,
                threat_multiplier: 1.0,
                bonus_coefficient: rank.damage_effect().coeff(),
                related_aura_arrays: related,
                ..SpellConfig::default()
            },
        );
        self.spells.hemorrhage = Some(spell);
    }

    fn register_premeditation(&mut self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("premeditation") {
            return;
        }
        let rank = spell_data().premeditation.highest();
        let timer = sim.new_timer(unit);
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                flags: SpellFlag::APL | SpellFlag::NO_ON_CAST_COMPLETE,
                class_spell_mask: masks::PREMEDITATION,
                cast: CastConfig {
                    default_cast: Cast {
                        cost: 0.0,
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
                has_extra_cast_condition: true,
                ..SpellConfig::default()
            },
        );
        self.spells.premeditation = Some(spell);
        sim.add_major_cooldown(
            unit,
            MajorCooldown {
                spell,
                priority: crate::prepare::major_cooldown::COOLDOWN_PRIORITY_LOW,
                cooldown_type: cooldown_type::DPS,
                allow_spell_queueing: true,
                timings: Vec::new(),
            },
        );
    }

    /// Quietus, new in Forever: Sinister Strike, Ghostly Strike and Hemorrhage hit harder once
    /// the target is in execute range.
    fn register_quietus(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("quietus");
        if rank == 0 {
            return;
        }
        let aura = sim.get_or_register_aura(
            unit,
            AuraConfig {
                label: "Quietus".to_string(),
                action_id: Some(spell_action(spell_data().quietus.highest().id)),
                duration: NEVER_EXPIRES,
                ..AuraConfig::default()
            },
        );
        sim.attach_spell_mod(
            aura,
            SpellModConfig {
                kind: SpellModType::DamageDoneFlat,
                class_mask: masks::QUIETUS,
                float_value: spell_data().quietus.effect_at(1).fraction_at(rank),
                ..SpellModConfig::default()
            },
        );
        // The execute phase callback it registers runs in a fight.
        sim.register_reset_effect(unit, Rc::new(move |sim: &mut Sim| sim.deactivate(aura)));
    }

    /// Cutthroat, new in Forever: a Backstab can let the next Ambush be used outside of
    /// Stealth.
    fn register_cutthroat(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("cutthroat");
        if rank == 0 {
            return;
        }
        let triggered = spell_data().cutthroat_triggered.highest();
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Cutthroat".to_string(),
                action_id: Some(spell_action(triggered.id)),
                duration: triggered.duration(),
                ..AuraConfig::default()
            },
        );
        self.auras.cutthroat = Some(aura);

        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Cutthroat Trigger".to_string(),
                action_id: spell_action(spell_data().cutthroat.highest().id),
                proc_chance: spell_data().cutthroat.fraction_at(rank),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                outcome: HitOutcome::LANDED,
                class_spell_mask: masks::BACKSTAB,
                ..ProcTrigger::default()
            },
        );
    }

    /// Thousand Cuts, new in Forever: Rupture's ticks discount the next Hemorrhage or
    /// Backstab.
    fn register_thousand_cuts(&mut self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("thousand_cuts") {
            return;
        }
        // 1310723 takes its energy off per stack, up to 5 stacks; the talent 1310721 has a
        // 1.9 s proc ICD.
        let buff = spell_data().thousand_cuts_triggered.highest();
        let cost_per_stack = buff
            .effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_COST)
            .average(CHARACTER_LEVEL) as i32;
        let cost_mod = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::PowerCostFlat,
                class_mask: masks::BACKSTAB | masks::HEMORRHAGE,
                ..SpellModConfig::default()
            },
        );

        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Thousand Cuts".to_string(),
                action_id: Some(spell_action(buff.id)),
                duration: buff.duration(),
                max_stacks: i32::from(buff.max_stack),
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.activate_spell_mod(cost_mod);
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.deactivate_spell_mod(cost_mod);
                })),
                on_stacks_change: Some(Rc::new(move |sim: &mut Sim, _, _, new_stacks| {
                    sim.update_spell_mod_int_value(
                        cost_mod,
                        cost_per_stack.wrapping_mul(new_stacks),
                    );
                })),
                events: EventCallbacks {
                    on_apply_effects: true,
                    ..EventCallbacks::default()
                },
                ..AuraConfig::default()
            },
        );
        self.auras.thousand_cuts = Some(aura);

        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Thousand Cuts Trigger".to_string(),
                callback: CallbackMask::ON_PERIODIC_DAMAGE_DEALT,
                class_spell_mask: masks::RUPTURE,
                icd: spell_data().thousand_cuts.highest().icd(),
                ..ProcTrigger::default()
            },
        );
    }
}
