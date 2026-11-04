//! Go sim/warlock/talents_destruction.go.

use std::rc::Rc;

use crate::prepare::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger, PseudoStatField};
use crate::prepare::dbcenums;
use crate::prepare::resolve_spell::{flags, spell_config};
use crate::prepare::sim::{AuraConfig, Sim, UnitId};
use crate::prepare::spell::{ProcMask, SpellConfig, SpellFlag};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::stats::SchoolIndex;

use super::super::curses::{aura_array_to_map, new_enemy_aura_array};
use super::super::masks;
use super::super::spell_data::spell_data;
use super::super::spells::{spell_action, target_units};
use super::super::Warlock;
use super::millis;

impl Warlock {
    pub(in super::super) fn register_destruction_talents(&mut self, sim: &mut Sim, unit: UnitId) {
        // Tier 1
        self.apply_destructive_reach(sim, unit);
        self.apply_improved_shadow_bolt(sim, unit);
        self.apply_bane(sim, unit);

        // Tier 2
        self.apply_molten_skin(sim, unit);
        self.apply_cataclysm(sim, unit);
        self.apply_aftermath(sim, unit);

        // Tier 3
        self.apply_ruin(sim, unit);
        self.apply_shadowburn(sim, unit);

        // Tier 4
        // Intensity models nothing in Go either.
        self.apply_agonizing_flames(sim, unit);
        self.apply_conflagrate(sim, unit);

        // Tier 5
        // Pyroclasm models nothing in Go either.
        self.apply_bane_of_havoc(sim, unit);
        self.apply_fire_and_brimstone(sim, unit);

        // Tier 6
        self.apply_shadow_and_flame(sim, unit);

        // Tier 7
        // Incinerate: incinerate.go
    }

    /// Destructive Reach: 17917 adds 10/20% to the range of every warlock spell its class mask
    /// reaches, Affliction ones too but not Curse of Recklessness. The threat half Classic carried
    /// is gone, and its radius half reaches no spell the sim registers. The sim's range mod is
    /// flat yards, so the share is taken of each spell's own range as it registers. A spell with no
    /// range (0) keeps none.
    fn apply_destructive_reach(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("destructive_reach");
        if points == 0 {
            return;
        }
        let share = spell_data()
            .destructive_reach
            .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_RANGE)
            .fraction_at(points);
        let reached = masks::SHADOW_BOLT
            | masks::IMMOLATE
            | masks::INCINERATE
            | masks::SOUL_FIRE
            | masks::SHADOW_BURN
            | masks::CONFLAGRATE
            | masks::SEARING_PAIN
            | masks::RAIN_OF_FIRE
            | masks::CORRUPTION
            | masks::CURSE_OF_AGONY
            | masks::CURSE_OF_DOOM
            | masks::CURSE_OF_ELEMENTS
            | masks::CURSE_OF_WEAKNESS
            | masks::SIPHON_LIFE
            | masks::DRAIN_LIFE
            | masks::DRAIN_SOUL
            | masks::DEATH_COIL
            | masks::WRACK;
        sim.on_spell_registered(
            unit,
            Rc::new(move |sim: &mut Sim, spell| {
                if sim.spell(spell).matches(reached) {
                    sim.spell_mut(spell).max_range *= 1.0 + share;
                }
            }),
        );
    }

    /// A Shadow Bolt crit leaves a debuff that only the warlock's own shadow damage benefits
    /// from, and it has no charges. 4% a point.
    fn apply_improved_shadow_bolt(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("improved_shadow_bolt");
        if points == 0 {
            return;
        }
        let triggered = spell_data().improved_shadow_bolt_triggered.highest();
        let label = format!("Improved Shadow Bolt-{}", sim.unit(unit).label);
        let duration = triggered.duration();
        let action_id = spell_action(triggered.id);

        self.improved_shadow_bolt_auras = new_enemy_aura_array(sim, |sim, target| {
            sim.register_aura(
                target,
                AuraConfig {
                    label: label.clone(),
                    action_id: Some(action_id.clone()),
                    duration,
                    ..AuraConfig::default()
                },
            )
        });

        for target in target_units(sim) {
            sim.add_dynamic_damage_taken_modifier(target);
        }

        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Improved Shadow Bolt Trigger".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                class_spell_mask: masks::SHADOW_BOLT,
                outcome: HitOutcome::CRIT,
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );
    }

    /// 3/6/10% cheaper Destruction spells in Forever, not 3% a point (17778).
    fn apply_cataclysm(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("cataclysm");
        if points == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::PowerCostPctAdd,
                float_value: spell_data().cataclysm.fraction_at(points),
                class_mask: masks::DESTRUCTION_SPELLS,
                ..SpellModConfig::default()
            },
        );
    }

    /// 0.1 sec off Shadow Bolt, Immolate and Incinerate and 0.4 sec off Soul Fire, a point
    /// (17788).
    fn apply_bane(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("bane");
        if points == 0 {
            return;
        }
        let data = &spell_data().bane;
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::CastTimeFlat,
                time_value: millis(data.effect_at(1).value_at(points)),
                class_mask: masks::SHADOW_BOLT | masks::IMMOLATE | masks::INCINERATE,
                ..SpellModConfig::default()
            },
        );
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::CastTimeFlat,
                time_value: millis(data.effect_at(2).value_at(points)),
                class_mask: masks::SOUL_FIRE,
                ..SpellModConfig::default()
            },
        );
    }

    fn apply_shadowburn(&mut self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("shadowburn") {
            return;
        }
        self.register_shadow_burn(sim, unit);
    }

    /// Forever grows Ruin from one rank to five: 20% more critical damage a point (17959).
    fn apply_ruin(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("ruin");
        if points == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::CritMultiplierFlat,
                float_value: spell_data().ruin.fraction_at(points),
                class_mask: masks::DESTRUCTION_SPELLS,
                ..SpellModConfig::default()
            },
        );
    }

    fn apply_conflagrate(&mut self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("conflagrate") {
            return;
        }
        self.register_conflagrate(sim, unit);
    }

    /// Conflagrate leaves 2% more Shadow damage a point and Shadowburn 2% more Fire, for 20 sec
    /// (426311 and 1293816).
    fn apply_shadow_and_flame(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("shadow_and_flame");
        if points == 0 {
            return;
        }
        let data = spell_data();
        let fire_row = data.shadow_and_flame_triggered.by_id(426311);
        let shadow_row = data.shadow_and_flame_triggered.by_id(1293816);
        let multiplier = 1.0
            + crate::prepare::hotfix_113554::talent(sim.rules, 426316, 5)
                .effect_at(3)
                .fraction_at(points);

        let shadow_aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Shadow and Flame (Shadow)".to_string(),
                action_id: Some(spell_action(shadow_row.id)),
                duration: shadow_row.duration(),
                ..AuraConfig::default()
            },
        );
        sim.attach_multiplicative_pseudo_stat_buff(
            shadow_aura,
            PseudoStatField::SchoolDamageDealtMultiplier(SchoolIndex::Shadow),
            multiplier,
        );

        let fire_aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Shadow and Flame (Fire)".to_string(),
                action_id: Some(spell_action(fire_row.id)),
                duration: fire_row.duration(),
                ..AuraConfig::default()
            },
        );
        sim.attach_multiplicative_pseudo_stat_buff(
            fire_aura,
            PseudoStatField::SchoolDamageDealtMultiplier(SchoolIndex::Fire),
            multiplier,
        );

        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Shadow and Flame Trigger".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                class_spell_mask: masks::CONFLAGRATE | masks::SHADOW_BURN,
                outcome: HitOutcome::LANDED,
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );
    }

    /// 2% less damage taken a point (1225220).
    fn apply_molten_skin(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("molten_skin");
        if points == 0 {
            return;
        }
        sim.unit_mut(unit).pseudo_stats.damage_taken_multiplier *=
            spell_data().molten_skin.multiplier_at(points);
    }

    /// 10% more Immolate impact damage a point (18119, second effect).
    fn apply_aftermath(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("aftermath");
        if points == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::DamageDoneFlat,
                float_value: spell_data().aftermath.effect_at(2).fraction_at(points),
                class_mask: masks::IMMOLATE,
                ..SpellModConfig::default()
            },
        );
    }

    /// 3/7/10% more Destruction damage, and the same again as Searing Pain crit (17927). The
    /// direct half leaves out Immolate's dot and Hellfire; the dot half is Immolate's dot alone
    /// of what the sim casts, so its ticks take the bonus once.
    fn apply_agonizing_flames(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("agonizing_flames");
        if points == 0 {
            return;
        }
        let data = &spell_data().agonizing_flames;

        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::DamageDoneFlat,
                float_value: data
                    .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_DAMAGE)
                    .fraction_at(points),
                class_mask: masks::DESTRUCTION_SPELLS & !(masks::IMMOLATE_DOT | masks::HELLFIRE),
                ..SpellModConfig::default()
            },
        );
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::DotDamageDonePct,
                float_value: data
                    .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_DOT)
                    .fraction_at(points),
                class_mask: masks::IMMOLATE_DOT,
                ..SpellModConfig::default()
            },
        );
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::BonusCritPercent,
                float_value: data
                    .effect(
                        dbcenums::A_ADD_FLAT_MODIFIER,
                        dbcenums::SPELLMOD_CRITICAL_CHANCE,
                    )
                    .value_at(points),
                class_mask: masks::SEARING_PAIN,
                ..SpellModConfig::default()
            },
        );
    }

    /// 8/17/25% Conflagrate crit, the talent's second effect (412751).
    fn apply_fire_and_brimstone(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("fire_and_brimstone");
        if points == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::BonusCritPercent,
                float_value: spell_data()
                    .fire_and_brimstone
                    .effect_at(2)
                    .value_at(points),
                class_mask: masks::CONFLAGRATE,
                ..SpellModConfig::default()
            },
        );
    }

    /// 1225228: a 5 min bane on one target (A_DUMMY 15) that copies 15% of the warlock's damage
    /// to other targets onto the baned one. It takes the bane slot, so it replaces Agony or
    /// Doom there. The row states no GCD category, so the cast is off the GCD.
    fn apply_bane_of_havoc(&self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("bane_of_havoc") {
            return;
        }
        let rank = spell_data().bane_of_havoc.highest();
        let label = format!("Bane of Havoc-{}", sim.unit(unit).label);
        let duration = rank.duration();
        let action_id = spell_action(rank.id);

        let havoc_auras = new_enemy_aura_array(sim, |sim, target| {
            sim.register_aura(
                target,
                AuraConfig {
                    label: label.clone(),
                    action_id: Some(action_id.clone()),
                    duration,
                    on_gain: Some(Rc::new(|_: &mut Sim, _| {})),
                    on_expire: Some(Rc::new(|_: &mut Sim, _| {})),
                    ..AuraConfig::default()
                },
            )
        });

        let mut config = spell_config(sim, unit, rank, &[flags(SpellFlag::APL)]);
        config.proc_mask = ProcMask::EMPTY;
        config.threat_multiplier = 1.0;
        config.related_aura_arrays = aura_array_to_map(sim, &havoc_auras);
        sim.register_spell(unit, config);

        sim.register_spell(
            unit,
            SpellConfig {
                action_id: crate::contracts::prepared_v2::ActionId {
                    spell_id: rank.id,
                    tag: 1,
                    ..Default::default()
                },
                spell_school: rank.spell_school(),
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::IGNORE_MODIFIERS
                    | SpellFlag::IGNORE_RESISTS
                    | SpellFlag::NO_ON_DAMAGE_DEALT
                    | SpellFlag::PASSIVE_SPELL
                    | SpellFlag::NO_ON_CAST_COMPLETE,
                damage_multiplier: 1.0,
                damage_multiplier_additive: 1.0,
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        );

        let copy = sim.register_aura(
            unit,
            AuraConfig {
                label: "Bane of Havoc - Copy".to_string(),
                events: crate::prepare::sim::EventCallbacks {
                    on_spell_hit_dealt: true,
                    on_periodic_damage_dealt: true,
                    ..Default::default()
                },
                ..AuraConfig::default()
            },
        );
        sim.make_permanent(copy);
    }
}
