//! The Warlock's spell registrations: Go `sim/warlock` `corruption.go`, `death_coil.go`,
//! `drain_life.go`, `hellfire.go`, `immolate.go`, `incinerate.go`, `lifetap.go`,
//! `rain_of_fire.go`, `searing_pain.go`, `shadowbolt.go`, `siphon_life.go`, `soulfire.go` and
//! `wrack.go`, and the Destruction talents' Conflagrate and Shadowburn. A closure Go gives a
//! spell config (`ApplyEffects`, `OnTick` and the like) only runs in a fight, so what
//! preparation keeps of it is the field that says it is there: `has_extra_cast_condition`.

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::dbcenums;
use crate::prepare::sim::{AuraConfig, Cooldown, Duration, Sim, UnitId, UnitType};
use crate::prepare::spell::{
    Cast, CastConfig, CostOptions, DefenseType, DotConfig, ProcMask, SpellConfig, SpellFlag,
    GCD_DEFAULT,
};
use crate::prepare::spelldata::Spell as Row;

use super::masks;
use super::spell_data::spell_data;
use super::Warlock;

pub(super) fn spell_action(id: i32) -> ActionId {
    ActionId {
        spell_id: id,
        ..ActionId::default()
    }
}

/// `ManaCost: core.ManaCostOptions{FlatCost: int32(rank.Cost())}`.
pub(super) fn flat_cost(row: &Row) -> CostOptions {
    CostOptions {
        mana_flat_cost: row.cost() as i32,
        ..CostOptions::default()
    }
}

/// `Cast: core.CastConfig{DefaultCast: core.Cast{GCD: gcd, CastTime: cast_time}}`.
pub(super) fn default_cast(gcd: Duration, cast_time: Duration) -> CastConfig {
    CastConfig {
        default_cast: Cast {
            gcd,
            cast_time,
            ..Cast::default()
        },
        ..CastConfig::default()
    }
}

/// `max(rank.Cooldown(), rank.CategoryCooldown())`.
pub(super) fn longest_cooldown(row: &Row) -> Duration {
    row.cooldown().max(row.category_cooldown())
}

/// The fields every damage spell of a row shares: school, defense type, the spell damage proc
/// mask and the multipliers Go's configs state (`DamageMultiplierAdditive: 1,
/// DamageMultiplier: 1, ThreatMultiplier: 1`).
pub(super) fn damage_config(row: &Row, class_spell_mask: i64, flags: SpellFlag) -> SpellConfig {
    SpellConfig {
        action_id: spell_action(row.id),
        spell_school: row.spell_school(),
        defense_type: row.defense_type_core(),
        proc_mask: ProcMask::SPELL_DAMAGE,
        flags,
        class_spell_mask,
        damage_multiplier: 1.0,
        damage_multiplier_additive: 1.0,
        threat_multiplier: 1.0,
        ..SpellConfig::default()
    }
}

/// Go `Environment.Encounter.AllTargetUnits`: the enemies, in unit order.
pub(super) fn target_units(sim: &Sim) -> Vec<UnitId> {
    sim.env_units
        .iter()
        .copied()
        .filter(|unit| sim.unit(*unit).unit_type == UnitType::Enemy)
        .collect()
}

impl Warlock {
    pub(super) fn register_corruption(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().corruption.highest_for(sim.rules);
        let tick = rank.periodic_effect();
        sim.register_spell(
            unit,
            SpellConfig {
                cost: flat_cost(rank),
                cast: default_cast(rank.gcd(), rank.cast_time()),
                bonus_coefficient: tick.coeff(),
                dot: DotConfig {
                    aura: AuraConfig {
                        label: "Corruption".to_string(),
                        tag: "Affliction".to_string(),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: (rank.duration() / tick.period()) as i32,
                    tick_length: tick.period(),
                    bonus_coefficient: tick.coeff(),
                    ..DotConfig::default()
                },
                max_range: f64::from(rank.max_range),
                ..damage_config(rank, masks::CORRUPTION, SpellFlag::APL)
            },
        );
    }

    /// The generator files Death Coil's damage under E_HEALTH_LEECH, which it has no Direct role
    /// for, so the row's Direct is nil and the damage and its coefficient are read off the effect.
    pub(super) fn register_death_coil(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().death_coil.highest();

        sim.get_or_register_spell(
            unit,
            SpellConfig {
                action_id: ActionId {
                    spell_id: rank.id,
                    tag: 1,
                    ..ActionId::default()
                },
                spell_school: crate::prepare::spell::school::PHYSICAL,
                proc_mask: ProcMask::SPELL_HEALING,
                flags: SpellFlag::PASSIVE_SPELL | SpellFlag::HELPFUL,
                damage_multiplier: 1.0,
                threat_multiplier: 0.0,
                ..SpellConfig::default()
            },
        );

        let timer = sim.new_timer(unit);
        sim.register_spell(
            unit,
            SpellConfig {
                missile_speed: f64::from(rank.speed),
                max_range: f64::from(rank.max_range),
                cost: flat_cost(rank),
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..default_cast(rank.gcd(), 0)
                },
                bonus_coefficient: rank.effect_n(1).coeff(),
                ..damage_config(rank, masks::DEATH_COIL, SpellFlag::APL | SpellFlag::BINARY)
            },
        );
    }

    /// Improved Drains rides on the talent as a dot SpellMod. Soul Siphon has to be counted per
    /// tick, so it stays in the tick.
    pub(super) fn register_drain_life(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().drain_life.highest();
        let tick = rank.periodic_effect();
        sim.register_spell(
            unit,
            SpellConfig {
                cost: flat_cost(rank),
                cast: default_cast(rank.gcd(), 0),
                bonus_coefficient: tick.coeff(),
                dot: DotConfig {
                    aura: AuraConfig {
                        label: "Drain Life".to_string(),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: (rank.duration() / tick.period()) as i32,
                    tick_length: tick.period(),
                    affected_by_cast_speed: true,
                    haste_reduces_duration: true,
                    bonus_coefficient: tick.coeff(),
                    ..DotConfig::default()
                },
                max_range: f64::from(rank.max_range),
                ..damage_config(
                    rank,
                    masks::DRAIN_LIFE,
                    SpellFlag::CHANNELED | SpellFlag::APL,
                )
            },
        );
    }

    pub(super) fn register_hellfire(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().hellfire.highest();
        // The self-burn tick: effect 1 is the periodic trigger that fires Hellfire Effect.
        let tick = rank.effect(dbcenums::A_PERIODIC_DAMAGE, 0);
        let mut config =
            damage_config(rank, masks::HELLFIRE, SpellFlag::CHANNELED | SpellFlag::APL);
        config.defense_type = DefenseType::Magic;
        sim.register_spell(
            unit,
            SpellConfig {
                cost: flat_cost(rank),
                cast: default_cast(rank.gcd(), 0),
                dot: DotConfig {
                    aura: AuraConfig {
                        label: "Hellfire".to_string(),
                        ..AuraConfig::default()
                    },
                    is_aoe: true,
                    tick_length: tick.period(),
                    number_of_ticks: (rank.duration() / tick.period()) as i32,
                    haste_reduces_duration: true,
                    affected_by_cast_speed: true,
                    bonus_coefficient: tick.coeff(),
                    ..DotConfig::default()
                },
                ..config
            },
        );
    }

    pub(super) fn register_immolate(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().immolate.highest_for(sim.rules);
        let tick = rank.periodic_effect();
        let immolate = sim.register_spell(
            unit,
            SpellConfig {
                cost: flat_cost(rank),
                cast: default_cast(rank.gcd(), rank.cast_time()),
                bonus_coefficient: rank.damage_effect().coeff(),
                max_range: f64::from(rank.max_range),
                ..damage_config(rank, masks::IMMOLATE, SpellFlag::APL)
            },
        );
        let dot_spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id: ActionId {
                    spell_id: rank.id,
                    tag: 1,
                    ..ActionId::default()
                },
                dot: DotConfig {
                    aura: AuraConfig {
                        label: "Immolate (DoT)".to_string(),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: (rank.duration() / tick.period()) as i32,
                    tick_length: tick.period(),
                    bonus_coefficient: tick.coeff(),
                    ..DotConfig::default()
                },
                ..damage_config(rank, masks::IMMOLATE_DOT, SpellFlag::PASSIVE_SPELL)
            },
        );
        sim.spell_mut(immolate).related_dot_spell = Some(dot_spell);
    }

    /// Forever's Incinerate is a Destruction talent (412758 and up), 2.5 sec, and hits 25%
    /// harder on a target carrying Immolate.
    pub(super) fn register_incinerate(&mut self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("incinerate") {
            return;
        }
        let rank = spell_data().incinerate.highest();
        sim.register_spell(
            unit,
            SpellConfig {
                missile_speed: f64::from(rank.speed),
                cost: flat_cost(rank),
                cast: default_cast(rank.gcd(), rank.cast_time()),
                bonus_coefficient: rank.damage_effect().coeff(),
                max_range: f64::from(rank.max_range),
                ..damage_config(rank, masks::INCINERATE, SpellFlag::APL)
            },
        );
    }

    /// Life Tap is a plain mana gain, not damage.
    pub(super) fn register_life_tap(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().life_tap.highest();
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                defense_type: DefenseType::Magic,
                proc_mask: ProcMask::SPELL_DAMAGE,
                flags: SpellFlag::APL,
                class_spell_mask: masks::LIFE_TAP,
                cast: default_cast(GCD_DEFAULT, 0),
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        );
    }

    /// Forever's Rain of Fire is an area trigger, like Blizzard: a channel whose periodic dummy
    /// casts a direct Fire hit on every enemy in the area every 2 seconds.
    pub(super) fn register_rain_of_fire(&mut self, sim: &mut Sim, unit: UnitId) {
        let data = spell_data();
        let rank = data.rain_of_fire.highest();
        let tick_spell = data.rain_of_fire_triggered.rank(rank.rank_number());
        let tick = tick_spell.damage_effect();
        let tick_length = rank.effect(dbcenums::A_PERIODIC_DUMMY, 0).period();
        let action_id = spell_action(rank.id);

        // The tick rows lack Not a Proc, so only listeners that can proc from procs hear them.
        sim.register_spell(
            unit,
            SpellConfig {
                bonus_coefficient: tick.coeff(),
                ..damage_config(
                    tick_spell,
                    masks::RAIN_OF_FIRE,
                    SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::PROC,
                )
            },
        );

        sim.register_spell(
            unit,
            SpellConfig {
                cost: flat_cost(rank),
                cast: default_cast(rank.gcd(), 0),
                dot: DotConfig {
                    is_aoe: true,
                    aura: AuraConfig {
                        label: "Rain of Fire".to_string(),
                        action_id: Some(action_id),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: (rank.duration() / tick_length) as i32,
                    tick_length,
                    ..DotConfig::default()
                },
                max_range: f64::from(rank.max_range),
                ..damage_config(
                    rank,
                    masks::RAIN_OF_FIRE,
                    SpellFlag::CHANNELED | SpellFlag::APL,
                )
            },
        );
    }

    pub(super) fn register_searing_pain(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().searing_pain.highest();
        sim.register_spell(
            unit,
            SpellConfig {
                max_range: f64::from(rank.max_range),
                cost: flat_cost(rank),
                cast: default_cast(rank.gcd(), rank.cast_time()),
                threat_multiplier: 2.0,
                bonus_coefficient: rank.damage_effect().coeff(),
                ..damage_config(rank, masks::SEARING_PAIN, SpellFlag::APL)
            },
        );
    }

    /// Every rank is registered so a rotation can drop to a cheaper one when mana runs short;
    /// the talents and procs reach them all through the Shadow Bolt mask.
    pub(super) fn register_shadow_bolt(&mut self, sim: &mut Sim, unit: UnitId) {
        spell_data().shadow_bolt.each(|_, rank| {
            sim.register_spell(
                unit,
                SpellConfig {
                    rank: rank.rank_number(),
                    missile_speed: f64::from(rank.speed),
                    cost: flat_cost(rank),
                    cast: default_cast(rank.gcd(), rank.cast_time()),
                    bonus_coefficient: rank.damage_effect().coeff(),
                    max_range: f64::from(rank.max_range),
                    ..damage_config(rank, masks::SHADOW_BOLT, SpellFlag::APL)
                },
            );
        });
    }

    /// Siphon Life is a Forever Affliction talent: a 30 sec shadow dot that heals the warlock
    /// for what it deals.
    pub(super) fn register_siphon_life_spell(&mut self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("siphon_life") {
            return;
        }
        let rank = spell_data().siphon_life.highest();
        let tick = rank.periodic_effect();
        sim.register_spell(
            unit,
            SpellConfig {
                cost: flat_cost(rank),
                cast: default_cast(rank.gcd(), 0),
                bonus_coefficient: tick.coeff(),
                dot: DotConfig {
                    aura: AuraConfig {
                        label: "Siphon Life".to_string(),
                        tag: "Affliction".to_string(),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: (rank.duration() / tick.period()) as i32,
                    tick_length: tick.period(),
                    bonus_coefficient: tick.coeff(),
                    ..DotConfig::default()
                },
                max_range: f64::from(rank.max_range),
                ..damage_config(rank, masks::SIPHON_LIFE, SpellFlag::APL | SpellFlag::BINARY)
            },
        );
    }

    /// Bane's cast time cut and Decimation's cooldown cut ride on the talents as SpellMods.
    pub(super) fn register_soulfire(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().soul_fire.highest();
        let timer = sim.new_timer(unit);
        sim.register_spell(
            unit,
            SpellConfig {
                missile_speed: f64::from(rank.speed),
                cost: flat_cost(rank),
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..default_cast(rank.gcd(), rank.cast_time())
                },
                bonus_coefficient: rank.damage_effect().coeff(),
                max_range: f64::from(rank.max_range),
                ..damage_config(rank, masks::SOUL_FIRE, SpellFlag::APL)
            },
        );
    }

    /// Wrack is the Forever Affliction capstone: a six second shadow channel that also makes the
    /// warlock's Corruption and Bane of Agony on the target tick 10% harder while it runs.
    pub(super) fn register_wrack(&mut self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("wrack") {
            return;
        }
        let rank = spell_data().wrack.highest();
        let tick = rank.periodic_effect();
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                cost: flat_cost(rank),
                cast: default_cast(rank.gcd(), 0),
                bonus_coefficient: tick.coeff(),
                dot: DotConfig {
                    aura: AuraConfig {
                        label: "Wrack".to_string(),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: (rank.duration() / tick.period()) as i32,
                    tick_length: tick.period(),
                    affected_by_cast_speed: true,
                    haste_reduces_duration: true,
                    bonus_coefficient: tick.coeff(),
                    ..DotConfig::default()
                },
                max_range: f64::from(rank.max_range),
                ..damage_config(rank, masks::WRACK, SpellFlag::CHANNELED | SpellFlag::APL)
            },
        );
        self.wrack = Some(spell);

        for target in target_units(sim) {
            sim.add_dynamic_damage_taken_modifier(target);
        }
    }

    /// Conflagrate burns the Immolate on the target. Shadow and Flame's second effect is the
    /// chance it survives.
    pub(super) fn register_conflagrate(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().conflagrate.highest();
        let timer = sim.new_timer(unit);
        sim.register_spell(
            unit,
            SpellConfig {
                cost: flat_cost(rank),
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..default_cast(rank.gcd(), 0)
                },
                has_extra_cast_condition: true,
                bonus_coefficient: rank.damage_effect().coeff(),
                max_range: f64::from(rank.max_range),
                ..damage_config(rank, masks::CONFLAGRATE, SpellFlag::APL)
            },
        );
    }

    pub(super) fn register_shadow_burn(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().shadowburn.highest();
        let timer = sim.new_timer(unit);
        sim.register_spell(
            unit,
            SpellConfig {
                cost: flat_cost(rank),
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..default_cast(rank.gcd(), 0)
                },
                bonus_coefficient: rank.damage_effect().coeff(),
                max_range: f64::from(rank.max_range),
                ..damage_config(rank, masks::SHADOW_BURN, SpellFlag::APL | SpellFlag::BINARY)
            },
        );
    }
}
