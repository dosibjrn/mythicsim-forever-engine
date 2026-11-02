//! Go sim/priest/talents_holy.go.

use std::rc::Rc;

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::aura_helpers::{CallbackMask, ProcTrigger};
use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::dbcenums;
use crate::prepare::sim::{AuraConfig, EventCallbacks, Sim, UnitId};
use crate::prepare::spell::school;
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::stats::Stat;

use super::super::masks;
use super::super::spell_data::spell_data;
use super::super::Priest;
use super::millis;

impl Priest {
    /// Go `registerHolyTalents`.
    pub(super) fn register_holy_talents(&mut self, sim: &mut Sim, unit: UnitId) {
        // Tier 1
        self.apply_twilight_focus(sim, unit);
        // Improved Renew models nothing in Go either.
        self.apply_holy_specialization(sim, unit);

        // Tier 2
        // Spell Warding models nothing in Go either.
        self.apply_divine_fury(sim, unit);

        // Tier 3
        self.apply_holy_nova(sim, unit);
        // Blessed Recovery and Inspiration model nothing in Go either.

        // Tier 4
        self.apply_holy_reach(sim, unit);
        self.apply_improved_healing(sim, unit);
        self.apply_searing_light(sim, unit);
        // Binding Heal models nothing in Go either; under Forever's rules push 113554 makes the
        // field Holy Binding, which heals.rs puts on Flash Heal.

        // Tier 5
        // Litany of Light and Spirit of Redemption model nothing in Go either; the healer's
        // Litany is in `heals.rs`.
        self.apply_spiritual_guidance(sim, unit);

        // Tier 6: Spiritual Healing models nothing in Go either.

        // Tier 7: Prayer of Mending models nothing in Go either.
    }

    /// Twilight Focus is new in Forever: pushback protection.
    fn apply_twilight_focus(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("twilight_focus");
        if points == 0 {
            return;
        }
        let resist = spell_data().twilight_focus.fraction_at(points);
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::SMITE
                    | masks::HOLY_FIRE
                    | masks::MIND_BLAST
                    | masks::MIND_FLAY
                    | masks::PENANCE
                    | masks::STARSHARDS,
                kind: SpellModType::Custom,
                apply_custom: Some(Rc::new(move |sim: &mut Sim, _, spell| {
                    sim.spell_mut(spell).pushback_resist += resist;
                })),
                remove_custom: Some(Rc::new(move |sim: &mut Sim, _, spell| {
                    sim.spell_mut(spell).pushback_resist -= resist;
                })),
                ..SpellModConfig::default()
            },
        );
    }

    /// Holy Specialization is new in Forever: +1% critical strike per point on Smite, Holy Fire,
    /// Holy Nova and the Penance bolts. The class mask leaves Chastise out.
    fn apply_holy_specialization(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("holy_specialization");
        if points == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::HOLY_SPELLS,
                float_value: spell_data().holy_specialization.value_at(points),
                kind: SpellModType::BonusCritPercent,
                ..SpellModConfig::default()
            },
        );
    }

    fn apply_divine_fury(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("divine_fury");
        if points == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::SMITE | masks::HOLY_FIRE,
                time_value: millis(
                    spell_data()
                        .divine_fury
                        .effect(
                            dbcenums::A_ADD_FLAT_MODIFIER,
                            dbcenums::SPELLMOD_CASTING_TIME,
                        )
                        .value_at(points),
                ),
                kind: SpellModType::CastTimeFlat,
                ..SpellModConfig::default()
            },
        );
    }

    fn apply_holy_nova(&self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("holy_nova") {
            return;
        }
        spell_data().holy_nova.each(|_, rank| {
            Self::register_holy_nova_spell(sim, unit, rank);
        });
    }

    /// Holy Reach is new in Forever: 27789 adds 10/20% to Smite and Holy Fire range (its first
    /// class mask). The second adds as much to Holy Nova and Prayer of Healing radius; the sim
    /// gives Holy Nova no radius, so that half reaches nothing.
    fn apply_holy_reach(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("holy_reach");
        if points == 0 {
            return;
        }
        Self::extend_range(
            sim,
            unit,
            masks::SMITE | masks::HOLY_FIRE,
            spell_data()
                .holy_reach
                .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_RANGE)
                .fraction_at(points),
        );
    }

    /// Improved Healing discounts Lesser Heal, Heal, Greater Heal, Penance and Prayer of
    /// Mending; Penance is the only one of those the sim casts.
    fn apply_improved_healing(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("improved_healing");
        if points == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::PENANCE,
                float_value: spell_data().improved_healing.fraction_at(points),
                kind: SpellModType::PowerCostPctAdd,
                ..SpellModConfig::default()
            },
        );
    }

    /// Searing Light buffs every Holy spell and gives Holy Fire ticks a chance to refund the
    /// next Holy Nova.
    fn apply_searing_light(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("searing_light");
        if points == 0 {
            return;
        }
        let data = spell_data();
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::ALL,
                school: school::HOLY,
                float_value: data.searing_light.effect_at(1).fraction_at(points),
                kind: SpellModType::DamageDonePct,
                ..SpellModConfig::default()
            },
        );

        let free_nova = data.searing_light_triggered.highest();
        let cost_mod = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                class_mask: masks::HOLY_NOVA,
                float_value: free_nova
                    .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_COST)
                    .average(CHARACTER_LEVEL)
                    / 100.0,
                kind: SpellModType::PowerCostPctAdd,
                ..SpellModConfig::default()
            },
        );

        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Searing Light".to_string(),
                action_id: Some(ActionId::spell(free_nova.id)),
                duration: free_nova.duration(),
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.activate_spell_mod(cost_mod);
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.deactivate_spell_mod(cost_mod);
                })),
                events: EventCallbacks {
                    on_cast_complete: true,
                    ..EventCallbacks::default()
                },
                ..AuraConfig::default()
            },
        );
        self.searing_light_aura = Some(aura);

        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Searing Light Trigger".to_string(),
                callback: CallbackMask::ON_PERIODIC_DAMAGE_DEALT,
                class_spell_mask: masks::HOLY_FIRE,
                proc_chance: data.searing_light.effect_at(2).fraction_at(points),
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );
    }

    /// The beta client's curves: damage 1/3/5/6/8% of Spirit, healing 5% per point.
    fn apply_spiritual_guidance(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("spiritual_guidance");
        if points == 0 {
            return;
        }
        let data = &spell_data().spiritual_guidance;
        let damage = data
            .effect(dbcenums::A_MOD_SPELL_DAMAGE_OF_STAT_PERCENT, 126)
            .fraction_at(points);
        let healing = data
            .effect(dbcenums::A_MOD_SPELL_HEALING_OF_STAT_PERCENT, 4)
            .fraction_at(points);
        let sdm = &mut sim.unit_mut(unit).sdm;
        sdm.add_stat_dependency(Stat::Spirit, Stat::SpellDamage, damage);
        sdm.add_stat_dependency(Stat::Spirit, Stat::HealingPower, healing);
    }
}
