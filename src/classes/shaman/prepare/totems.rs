//! The Shaman's totems: Go `sim/shaman` `totems.go` and `registerFlametongueTotemSpell` in
//! `fire_totems.go`. A totem's buff is a spell of its own; the value it gives lives on that
//! spell, not on the totem.

use std::rc::Rc;

use crate::prepare::buffs::flametongue::{
    flametongue_totem_trigger, join_flametongue_totem, windfury_totem_disables_flametongue_totem,
};
use crate::prepare::buffs::generated::{
    GRACE_OF_AIR_TOTEM, MANA_SPRING_TOTEM, MANA_TIDE_TOTEMS, STRENGTH_OF_EARTH_TOTEM,
    WINDFURY_TOTEM,
};
use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::dbcenums;
use crate::prepare::parse_effects::exclusive_stat_category;
use crate::prepare::periodic_action::PeriodicActionOptions;
use crate::prepare::resolve_proc::{chance, proc_trigger};
use crate::prepare::sim::{AuraConfig, Cooldown, EffectId, Sim, UnitId, NEVER_EXPIRES, SECOND};
use crate::prepare::spell::{DefenseType, SpellConfig, SpellFlag};
use crate::prepare::spelldata::Spell as Row;
use crate::prepare::stats::{Stat, Stats};

use super::spell_data::spell_data;
use super::spells::{default_cast, flat_cost, spell_action};
use super::{flags, flametongue_traits, masks, Shaman};

/// The aura of the shaman's own Mana Tide Totem.
pub(super) const MANA_TIDE_TOTEM_AURA: &str = "Mana Tide Totem (Self)";

/// The category of its MP5. It is not the raid's external Mana Tide Totem's ("ManaTideTotem"),
/// whose aura refuses a category that anything else joins: the two stack, as two shamans' totems
/// would if they did not replace each other.
const MANA_TIDE_TOTEM_CATEGORY: &str = "ManaTideTotemSelf";

/// Go `Duration.Seconds`.
fn duration_seconds(duration: i64) -> f64 {
    let sec = duration / SECOND;
    let nsec = duration % SECOND;
    sec as f64 + nsec as f64 / 1e9
}

/// Go `newTotemSpellConfig`.
fn totem_spell_config(flat: i32, spell_id: i32, mask: i64, gcd: i64) -> SpellConfig {
    SpellConfig {
        action_id: spell_action(spell_id),
        defense_type: DefenseType::Magic,
        flags: SpellFlag::APL | flags::INSTANT,
        class_spell_mask: mask,
        cost: flat_cost(flat),
        cast: default_cast(gcd, 0),
        ..SpellConfig::default()
    }
}

/// The totem rank's config: the cost and global cooldown its row states.
fn rank_totem_config(rank: &Row, mask: i64) -> SpellConfig {
    totem_spell_config(rank.cost() as i32, rank.id, mask, rank.gcd())
}

/// A stat a totem's aura adds on gain and removes on expire, whichever aura of the category is
/// the strongest.
fn stat_exclusive_effect(
    sim: &mut Sim,
    aura: crate::prepare::sim::AuraId,
    category: &str,
    stat: Stat,
    value: f64,
) {
    sim.new_exclusive_effect(
        aura,
        &exclusive_stat_category(category, stat.name(), false),
        false,
        value,
        Some(Rc::new(move |sim: &mut Sim, effect: EffectId| {
            let unit = sim.aura(sim.effects[effect.0].aura).unit;
            sim.add_stat_dynamic(unit, stat, value);
        })),
        Some(Rc::new(move |sim: &mut Sim, effect: EffectId| {
            let unit = sim.aura(sim.effects[effect.0].aura).unit;
            sim.add_stat_dynamic(unit, stat, -value);
        })),
    );
}

impl Shaman {
    /// Go `registerWindfuryTotemSpell`.
    pub(super) fn register_windfury_totem_spell(&self, sim: &mut Sim, unit: UnitId) {
        let data = spell_data();
        let rank = data.windfury_totem.highest();
        let buff = data.windfury_totem_triggered.by_id(10610);
        let party_aura = data.windfury_totem_triggered.by_id(10612);
        let duration = rank.duration();
        // Forever drops Improved Weapon Totems, so the buff's own attack power is the whole value.
        let value = buff.effect_n(1).average(CHARACTER_LEVEL);

        let wf_proc_aura = sim
            .new_temporary_stats_aura(
                unit,
                "Windfury Totem Proc (Self)",
                &spell_action(buff.id),
                Stats::from_pairs(&[(Stat::AttackPower, value)]),
                buff.duration(),
            )
            .aura;
        sim.aura_mut(wf_proc_aura).max_stacks = i32::from(buff.proc_charges);
        // The buff row's proc flags say what spends a charge, as for the party's: a melee auto
        // that lands, so a missed, dodged or parried swing keeps it.
        let mut spender = proc_trigger(sim, Some(unit), buff, &[chance(1.0)]);
        spender.name = "Windfury Attack (Self)".to_string();
        spender.trigger_immediately = true;
        sim.attach_proc_trigger(wf_proc_aura, &spender);

        let config = rank_totem_config(rank, masks::BASIC_TOTEM);

        // The party aura's own row: 20% on any melee auto or special, a 100 ms internal
        // cooldown, and the extra attack is always a main-hand one.
        let mut trigger = proc_trigger(sim, Some(unit), party_aura, &[]);
        trigger.name = "Windfury Totem Trigger (Self)".to_string();
        trigger.action_id = Default::default();
        trigger.metrics_action_id = spell_action(rank.id);
        trigger.duration = NEVER_EXPIRES;
        trigger.trigger_immediately = true;
        let wf_proc_trigger = sim.make_proc_trigger_aura(unit, &trigger);

        let intermediate = sim.register_aura(
            unit,
            AuraConfig {
                label: "Windfury Dummy Aura (self)".to_string(),
                duration: 10 * SECOND,
                ..AuraConfig::default()
            },
        );

        let tracking = sim.register_aura(
            unit,
            AuraConfig {
                label: "Windfury Party Weapon Buff Tracking Aura".to_string(),
                duration: 10 * SECOND,
                action_id: Some(crate::contracts::prepared_v2::ActionId {
                    spell_id: rank.id,
                    tag: 1,
                    ..Default::default()
                }),
                ..AuraConfig::default()
            },
        );

        let wf_aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Windfury Totem (Self)".to_string(),
                action_id: Some(config.action_id.clone()),
                duration,
                ..AuraConfig::default()
            },
        );
        windfury_totem_disables_flametongue_totem(sim, wf_aura);
        let buff_id = buff.id;
        sim.apply_on_init(
            wf_aura,
            Rc::new(move |sim: &mut Sim, _| {
                let mut mh_config = sim
                    .unit(unit)
                    .auto_attacks
                    .mh_config
                    .clone()
                    .unwrap_or_default();
                mh_config.action_id.tag = buff_id;
                sim.get_or_register_spell(unit, mh_config);
            }),
        );
        // Since build 70009 the totem's effect is a party aura, so it ends with the totem instead
        // of lingering on the weapon for a twist.
        sim.apply_on_expire(
            wf_aura,
            Rc::new(move |sim: &mut Sim, _| {
                sim.deactivate(intermediate);
                sim.deactivate(tracking);
            }),
        );
        sim.attach_periodic_action(
            wf_aura,
            PeriodicActionOptions {
                period: 5 * SECOND,
                tick_immediately: true,
                ..PeriodicActionOptions::default()
            },
        );

        sim.new_exclusive_effect(
            intermediate,
            WINDFURY_TOTEM.category,
            false,
            value,
            Some(Rc::new(move |sim: &mut Sim, _: EffectId| {
                sim.activate(wf_proc_trigger);
            })),
            Some(Rc::new(move |sim: &mut Sim, _: EffectId| {
                sim.deactivate(wf_proc_trigger);
                sim.deactivate(intermediate);
            })),
        );
        // An item swap takes the intermediate aura down; the totem's next refresh puts it up.
        sim.register_item_swap_callback(
            unit,
            &[crate::prepare::items::slot::MAIN_HAND],
            crate::prepare::item_swap::SwapCallback::Deactivate { aura: intermediate },
        );

        sim.register_spell(unit, config);
    }

    /// Go `registerStrengthOfEarthTotemSpell`.
    pub(super) fn register_strength_of_earth_totem_spell(&self, sim: &mut Sim, unit: UnitId) {
        let data = spell_data();
        let rank = data.strength_of_earth_totem.highest_for(sim.rules);
        let buff = data.strength_of_earth_totem_triggered.highest();
        let duration = rank.duration();
        // Forever drops Enhancing Totems, so the buff's own value is the whole value.
        let value = buff.effect_n(1).average(CHARACTER_LEVEL);
        let config = rank_totem_config(rank, masks::BASIC_TOTEM);
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Strength Of Earth Totem (Self)".to_string(),
                action_id: Some(config.action_id.clone()),
                duration,
                ..AuraConfig::default()
            },
        );
        stat_exclusive_effect(
            sim,
            aura,
            STRENGTH_OF_EARTH_TOTEM.category,
            Stat::Strength,
            value,
        );
        sim.register_spell(unit, config);
    }

    /// Go `registerGraceOfAirTotemSpell`.
    pub(super) fn register_grace_of_air_totem_spell(&self, sim: &mut Sim, unit: UnitId) {
        let data = spell_data();
        let rank = data.grace_of_air_totem.highest_for(sim.rules);
        let buff = data.grace_of_air_totem_triggered.highest();
        let duration = rank.duration();
        let value = buff.effect_n(1).average(CHARACTER_LEVEL);
        let config = rank_totem_config(rank, masks::BASIC_TOTEM);
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Grace Of Air Totem (Self)".to_string(),
                action_id: Some(config.action_id.clone()),
                duration,
                ..AuraConfig::default()
            },
        );
        stat_exclusive_effect(sim, aura, GRACE_OF_AIR_TOTEM.category, Stat::Agility, value);
        sim.register_spell(unit, config);
    }

    /// Go `registerManaSpringTotemSpell`.
    pub(super) fn register_mana_spring_totem_spell(&self, sim: &mut Sim, unit: UnitId) {
        let data = spell_data();
        let rank = data.mana_spring_totem.highest();
        let buff = data.mana_spring_totem_triggered.highest();
        let duration = rank.duration();
        // The buff ticks its value every 2 sec, and MP5 is the form the sim takes; Restorative
        // Totems raises it by the ladder the client states.
        let tick = buff.effect(dbcenums::A_PERIODIC_ENERGIZE, 0);
        let value = tick.average(CHARACTER_LEVEL)
            * (5.0 / duration_seconds(tick.period()))
            * data
                .restorative_totems
                .effect_at(1)
                .multiplier_at(self.talent("restorative_totems"));
        let config = rank_totem_config(rank, masks::BASIC_TOTEM);
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Mana Spring Totem (Self)".to_string(),
                action_id: Some(config.action_id.clone()),
                duration,
                ..AuraConfig::default()
            },
        );
        stat_exclusive_effect(sim, aura, MANA_SPRING_TOTEM.category, Stat::MP5, value);
        sim.register_spell(unit, config);
    }

    /// Mana Tide Totem (16190, rank 4 at level 60 is 1325256), the Restoration shaman's own cast
    /// under Forever's rules (Rust only; the reference never casts it). The totem stands for 13 sec
    /// on a 5 minute cooldown, costs the row's 60 mana and gives 290 mana every 3 sec to its
    /// group (the rank's Mana Tide row, 1325257). The sim models only the shaman's own mana, and
    /// as Mana Spring Totem's ticks and the raid's external Mana Tide Totem are, the ticks are
    /// the form MP5 takes: 290 every 3 sec is 483.3 MP5 for the 13 sec, 1,257 mana against the
    /// 1,160 of four whole ticks. It stands in the water totem slot, so it ends a Healing Stream
    /// Totem and a Healing Stream Totem cast ends it (`ManaTideTotem`'s runtime).
    pub(super) fn register_mana_tide_totem_spell(&self, sim: &mut Sim, unit: UnitId) {
        let data = spell_data();
        let rank = data.mana_tide_totem.highest();
        let duration = rank.duration();
        // The raid's external Mana Tide Totem reads the same row, so it is the same MP5 to the bit.
        let value = MANA_TIDE_TOTEMS.value(0);
        let mut config = rank_totem_config(rank, masks::BASIC_TOTEM);
        config.cast.cd = Cooldown {
            timer: Some(sim.new_timer(unit)),
            duration: rank.category_cooldown(),
        };
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: MANA_TIDE_TOTEM_AURA.to_string(),
                action_id: Some(config.action_id.clone()),
                duration,
                ..AuraConfig::default()
            },
        );
        stat_exclusive_effect(sim, aura, MANA_TIDE_TOTEM_CATEGORY, Stat::MP5, value);
        sim.register_spell(unit, config);
    }

    /// Go `registerFlametongueTotemSpell`: the shaman's own Flametongue Totem, a 5 min fire
    /// totem whose party aura gives the shaman a fire hit on each main-hand auto attack. It
    /// takes the fire slot, so Searing Totem and Magma Totem replace it and it replaces them,
    /// and a main-hand Flametongue Weapon turns the benefit off while the totem stands.
    pub(super) fn register_flametongue_totem_spell(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().flametongue_totem.highest();
        let duration = rank.duration();

        let mut config = rank_totem_config(rank, masks::FLAMETONGUE_TOTEM);
        config.spell_school = rank.spell_school();
        config.flags |= flags::SHAMAN_SPELL;

        // The trigger is registered before the totem aura that switches it on, so that it is
        // reset first.
        flametongue_totem_trigger(sim, unit, flametongue_traits());
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Flametongue Totem (Self)".to_string(),
                action_id: Some(config.action_id.clone()),
                duration,
                ..AuraConfig::default()
            },
        );
        self.flametongue_totem_aura = Some(aura);
        join_flametongue_totem(sim, unit, aura, flametongue_traits());

        sim.register_spell(unit, config);
    }
}
