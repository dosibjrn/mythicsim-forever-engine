//! The Mage's spell registrations: Go `sim/mage` `registerSpells` and the `register*Spell`
//! functions it calls, in Go's order. A closure Go gives a spell config (`ApplyEffects`, `OnTick`
//! and the like) only runs in a fight, so what preparation keeps of it is the field that says it
//! is there: `has_extra_cast_condition`, the related buff.

use std::rc::Rc;

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::character::{cooldown_type, MajorCooldown};
use crate::prepare::dbcenums;
use crate::prepare::hotfix_113554;
use crate::prepare::sim::{
    AuraConfig, Cooldown, Duration, EventCallbacks, Rules, Sim, UnitId, MILLISECOND, NEVER_EXPIRES,
    SECOND,
};
use crate::prepare::spell::{
    Cast, CastConfig, CostOptions, DotConfig, ProcMask, SpellConfig, SpellFlag,
};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::spelldata::Spell as Row;
use crate::prepare::stats::Stat;

use super::masks;
use super::spell_data::spell_data;
use super::Mage;

/// Go `pointBlankRadius`: Arcane Explosion, Blast Wave, Cone of Cold and Frost Nova reach the
/// targets within 10 yards of the mage and state no cast range, so the radius is their range.
pub(super) const POINT_BLANK_RADIUS: f64 = 10.0;

fn spell_action(id: i32) -> ActionId {
    ActionId {
        spell_id: id,
        ..ActionId::default()
    }
}

fn item_action(id: i32) -> ActionId {
    ActionId {
        item_id: id,
        ..ActionId::default()
    }
}

/// Go `ManaCost: core.ManaCostOptions{FlatCost: int32(row.Cost())}`.
fn flat_cost(row: &Row) -> CostOptions {
    CostOptions {
        mana_flat_cost: row.cost() as i32,
        ..CostOptions::default()
    }
}

/// `Cast: core.CastConfig{DefaultCast: core.Cast{GCD: gcd, CastTime: cast_time}}`.
fn default_cast(gcd: Duration, cast_time: Duration) -> CastConfig {
    CastConfig {
        default_cast: Cast {
            gcd,
            cast_time,
            ..Cast::default()
        },
        ..CastConfig::default()
    }
}

/// `max(row.Cooldown(), row.CategoryCooldown())`.
fn longest_cooldown(row: &Row) -> Duration {
    row.cooldown().max(row.category_cooldown())
}

/// The fields every damage spell of a row shares: school, defense type, the spell damage proc
/// mask and the multipliers Go's configs state.
fn damage_config(row: &Row, id: i32, class_spell_mask: i64, flags: SpellFlag) -> SpellConfig {
    SpellConfig {
        action_id: spell_action(id),
        spell_school: row.spell_school(),
        defense_type: row.defense_type_core(),
        proc_mask: ProcMask::SPELL_DAMAGE,
        flags,
        class_spell_mask,
        damage_multiplier: 1.0,
        threat_multiplier: 1.0,
        ..SpellConfig::default()
    }
}

impl Mage {
    /// Go `Mage.registerSpells`.
    pub(super) fn register_spells(&self, sim: &mut Sim, unit: UnitId) {
        self.register_arcane_blast_spell(sim, unit);
        self.register_arcane_explosion_spell(sim, unit);
        self.register_arcane_missiles_spell(sim, unit);
        self.register_armor_spells(sim, unit);
        self.register_blizzard_spell(sim, unit);
        self.register_cone_of_cold_spell(sim, unit);
        self.register_frostbolt_spell(sim, unit);
        self.register_frostfire_bolt(sim, unit);
        self.register_evocation(sim, unit);
        self.register_fireball_spell(sim, unit);
        self.register_fire_blast_spell(sim, unit);
        self.register_frost_nova_spell(sim, unit);
        self.register_ice_lance_spell(sim, unit);
        self.register_mana_gems(sim, unit);
        self.register_scorch_spell(sim, unit);

        spell_data().flamestrike_ranks.each(|_, rank| {
            self.register_flamestrike(sim, unit, rank);
        });

        // Talent spells.
        self.register_presence_of_mind_spell(sim, unit);
        self.register_arcane_power_spell(sim, unit);
        self.register_arcane_wrath_spell(sim, unit);
        self.register_frost_blast_spell(sim, unit);

        self.register_blast_wave_spell(sim, unit);
        self.register_pyroblast_spell(sim, unit);
        self.register_combustion_spell(sim, unit);

        self.register_cold_snap_spell(sim, unit);
    }

    fn register_arcane_blast_spell(&self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("arcane_blast") {
            return;
        }
        let rank = spell_data().arcane_blast.highest();
        let mana = rank.mana_cost();
        sim.register_spell(
            unit,
            SpellConfig {
                cost: CostOptions {
                    mana_base_cost_percent: mana.base_cost_percent,
                    mana_flat_cost: mana.flat_cost,
                    ..CostOptions::default()
                },
                cast: default_cast(rank.gcd(), rank.cast_time()),
                bonus_coefficient: rank.damage_effect().coeff(),
                max_range: f64::from(rank.max_range),
                ..damage_config(rank, rank.id, masks::ARCANE_BLAST, SpellFlag::APL)
            },
        );
    }

    fn register_arcane_explosion_spell(&self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().arcane_explosion.highest();
        sim.register_spell(
            unit,
            SpellConfig {
                cost: flat_cost(rank),
                cast: default_cast(rank.gcd(), 0),
                bonus_coefficient: rank.damage_effect().coeff(),
                max_range: POINT_BLANK_RADIUS,
                ..damage_config(rank, rank.id, masks::ARCANE_EXPLOSION, SpellFlag::APL)
            },
        );
    }

    /// Every rank is registered: a rotation can drop to a cheaper rank when mana runs short.
    fn register_arcane_missiles_spell(&self, sim: &mut Sim, unit: UnitId) {
        spell_data().arcane_missiles.each(|_, rank| {
            self.register_arcane_missiles_rank(sim, unit, rank);
        });
    }

    fn register_arcane_missiles_rank(&self, sim: &mut Sim, unit: UnitId, rank: &Row) {
        let missile = spell_data()
            .arcane_missiles_triggered
            .rank(rank.rank_number());

        // One missile a second for the channel; the row states the channel's length.
        let tick_length = SECOND;
        let number_of_ticks = (rank.duration() / tick_length) as i32;

        sim.register_spell(
            unit,
            SpellConfig {
                missile_speed: f64::from(missile.speed),
                bonus_coefficient: missile.damage_effect().coeff(),
                ..damage_config(
                    missile,
                    missile.id,
                    masks::ARCANE_MISSILES_TICK,
                    SpellFlag::NO_ON_CAST_COMPLETE,
                )
            },
        );

        sim.register_spell(
            unit,
            SpellConfig {
                rank: rank.rank_number(),
                cost: flat_cost(rank),
                cast: default_cast(rank.gcd(), 0),
                dot: DotConfig {
                    aura: AuraConfig {
                        label: format!("ArcaneMissiles-{}", rank.rank_number()),
                        ..AuraConfig::default()
                    },
                    number_of_ticks,
                    tick_length,
                    ..DotConfig::default()
                },
                damage_multiplier: 0.0,
                threat_multiplier: 0.0,
                max_range: f64::from(rank.max_range),
                ..damage_config(
                    rank,
                    rank.id,
                    masks::ARCANE_MISSILES_CAST,
                    SpellFlag::CHANNELED | SpellFlag::APL,
                )
            },
        );
    }

    fn register_blizzard_spell(&self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().blizzard.highest();
        // The damage is the spell BlizzardTriggered casts each tick, at the same rank; the tick
        // length is Blizzard's own periodic dummy.
        let tick_spell = spell_data().blizzard_triggered.rank(rank.rank_number());
        let tick_length = rank.effect(dbcenums::A_PERIODIC_DUMMY, 0).period();
        let action = spell_action(rank.id);

        // Improved Blizzard's chill, a separate spell so Fingers of Frost can roll on it.
        if self.talents.i32("improved_blizzard") > 0 {
            let chill = spell_data().improved_blizzard_triggered.highest();
            sim.register_spell(
                unit,
                SpellConfig {
                    action_id: spell_action(chill.id),
                    spell_school: crate::prepare::spell::school::FROST,
                    defense_type: crate::prepare::spell::DefenseType::Magic,
                    proc_mask: ProcMask::SPELL_DAMAGE_PROC,
                    flags: SpellFlag::NO_LOGS
                        | SpellFlag::NO_METRICS
                        | SpellFlag::NO_ON_CAST_COMPLETE,
                    class_spell_mask: masks::IMPROVED_BLIZZARD,
                    ..SpellConfig::default()
                },
            );
        }

        // The tick rows lack Not a Proc, so only listeners that can proc from procs hear them.
        sim.register_spell(
            unit,
            SpellConfig {
                bonus_coefficient: tick_spell.damage_effect().coeff(),
                ..damage_config(
                    rank,
                    tick_spell.id,
                    masks::BLIZZARD,
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
                        label: "Blizzard".to_string(),
                        action_id: Some(action),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: (rank.duration() / tick_length) as i32,
                    tick_length,
                    ..DotConfig::default()
                },
                damage_multiplier: 0.0,
                threat_multiplier: 0.0,
                max_range: f64::from(rank.max_range),
                ..damage_config(
                    rank,
                    rank.id,
                    masks::BLIZZARD,
                    SpellFlag::CHANNELED | SpellFlag::APL,
                )
            },
        );
    }

    fn register_cone_of_cold_spell(&self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().cone_of_cold.highest();
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
                max_range: POINT_BLANK_RADIUS,
                ..damage_config(
                    rank,
                    rank.id,
                    masks::CONE_OF_COLD,
                    SpellFlag::APL | SpellFlag::BINARY,
                )
            },
        );
    }

    /// Every rank is registered: a rotation can drop to a cheaper rank when mana runs short.
    fn register_frostbolt_spell(&self, sim: &mut Sim, unit: UnitId) {
        spell_data().frostbolt.each(|_, rank| {
            sim.register_spell(
                unit,
                SpellConfig {
                    rank: rank.rank_number(),
                    missile_speed: f64::from(rank.speed),
                    cost: flat_cost(rank),
                    cast: default_cast(rank.gcd(), rank.cast_time()),
                    bonus_coefficient: rank.damage_effect().coeff(),
                    max_range: f64::from(rank.max_range),
                    ..damage_config(
                        rank,
                        rank.id,
                        masks::FROSTBOLT,
                        SpellFlag::APL | SpellFlag::BINARY,
                    )
                },
            );
        });
    }

    /// Go `registerFrostBlastSpell` (MythicSim patch 153): a trained Frost spell of client 70425, a
    /// cast that deals Frost damage and slows the target, every rank registered as Frostbolt's are.
    fn register_frost_blast_spell(&self, sim: &mut Sim, unit: UnitId) {
        spell_data().frost_blast.each(|_, rank| {
            sim.register_spell(
                unit,
                SpellConfig {
                    rank: rank.rank_number(),
                    missile_speed: f64::from(rank.speed),
                    cost: flat_cost(rank),
                    cast: default_cast(rank.gcd(), rank.cast_time()),
                    bonus_coefficient: rank.damage_effect().coeff(),
                    max_range: f64::from(rank.max_range),
                    ..damage_config(
                        rank,
                        rank.id,
                        masks::FROST_BLAST,
                        SpellFlag::APL | SpellFlag::BINARY,
                    )
                },
            );
        });
    }

    /// Baseline Forever spell, learned at levels 40, 50 and 60.
    fn register_frostfire_bolt(&self, sim: &mut Sim, unit: UnitId) {
        spell_data().frostfire_bolt.each(|_, rank| {
            let tick = rank.periodic_effect();
            let tick_length = tick.period();
            sim.register_spell(
                unit,
                SpellConfig {
                    rank: rank.rank_number(),
                    missile_speed: f64::from(rank.speed),
                    cost: flat_cost(rank),
                    cast: default_cast(rank.gcd(), rank.cast_time()),
                    dot: DotConfig {
                        aura: AuraConfig {
                            label: format!("FrostfireBoltDoT-{}", rank.rank_number()),
                            ..AuraConfig::default()
                        },
                        number_of_ticks: (rank.duration() / tick_length) as i32,
                        tick_length,
                        bonus_coefficient: tick.coeff(),
                        ..DotConfig::default()
                    },
                    bonus_coefficient: rank.damage_effect().coeff(),
                    max_range: f64::from(rank.max_range),
                    ..damage_config(
                        rank,
                        rank.id,
                        masks::FROSTFIRE_BOLT,
                        SpellFlag::APL | SpellFlag::BINARY,
                    )
                },
            );
        });
    }

    /// Evocation raises spirit regen by the row's 1500% and lets it run in full while channeling.
    fn register_evocation(&self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().evocation.highest();
        let regen_multiplier = rank
            .effect(dbcenums::A_MOD_POWER_REGEN_PERCENT, 0)
            .average(CHARACTER_LEVEL)
            / 100.0;
        let action = spell_action(rank.id);

        // The row states the channel's length, not a period; ticks only mark the channel.
        let tick_length = MILLISECOND * 250;

        let regen_aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Evocation Regen".to_string(),
                action_id: Some(action.clone()),
                duration: NEVER_EXPIRES,
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    let stats = &mut sim.unit_mut(unit).pseudo_stats;
                    stats.spirit_regen_multiplier += regen_multiplier;
                    stats.force_full_spirit_regen = true;
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    let stats = &mut sim.unit_mut(unit).pseudo_stats;
                    stats.spirit_regen_multiplier += -regen_multiplier;
                    stats.force_full_spirit_regen = false;
                })),
                ..AuraConfig::default()
            },
        );
        // Go's RegisterAura hands the aura to the Hot's callbacks below.
        let timer = sim.new_timer(unit);
        let evocation = sim.register_spell(
            unit,
            SpellConfig {
                action_id: action,
                flags: SpellFlag::HELPFUL | SpellFlag::CHANNELED | SpellFlag::APL,
                class_spell_mask: masks::EVOCATION,
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..default_cast(rank.gcd(), 0)
                },
                hot: DotConfig {
                    self_only: true,
                    aura: AuraConfig {
                        label: "Evocation".to_string(),
                        on_gain: Some(Rc::new(move |sim: &mut Sim, _| sim.activate(regen_aura))),
                        on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                            sim.deactivate(regen_aura)
                        })),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: (rank.duration() / tick_length) as i32,
                    tick_length,
                    ..DotConfig::default()
                },
                ..SpellConfig::default()
            },
        );
        sim.add_major_cooldown(
            unit,
            MajorCooldown {
                spell: evocation,
                priority: 0,
                cooldown_type: cooldown_type::MANA,
                allow_spell_queueing: false,
                timings: Vec::new(),
            },
        );
    }

    /// Every rank is registered: a rotation can drop to a cheaper rank when mana runs short.
    fn register_fireball_spell(&self, sim: &mut Sim, unit: UnitId) {
        spell_data().fireball.each(|_, rank| {
            let tick = rank.periodic_effect();
            let tick_length = tick.period();
            sim.register_spell(
                unit,
                SpellConfig {
                    rank: rank.rank_number(),
                    missile_speed: f64::from(rank.speed),
                    cost: flat_cost(rank),
                    cast: default_cast(rank.gcd(), rank.cast_time()),
                    dot: DotConfig {
                        aura: AuraConfig {
                            label: format!("FireballDoT-{}", rank.rank_number()),
                            ..AuraConfig::default()
                        },
                        number_of_ticks: (rank.duration() / tick_length) as i32,
                        tick_length,
                        bonus_coefficient: tick.coeff(),
                        ..DotConfig::default()
                    },
                    bonus_coefficient: rank.damage_effect().coeff(),
                    max_range: f64::from(rank.max_range),
                    ..damage_config(rank, rank.id, masks::FIREBALL, SpellFlag::APL)
                },
            );
        });
    }

    /// Go `registerArcaneWrathSpell`: Arcane Wrath (1326079 to 1326082), an Arcane talent of
    /// client 70425, a 3.5 sec Arcane bolt on a 6 sec cooldown whose spell damage adds 5% of the
    /// mage's maximum mana at the damage effect's coefficient.
    fn register_arcane_wrath_spell(&self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("arcane_wrath") {
            return;
        }
        let rank = spell_data().arcane_wrath.highest();
        let timer = sim.new_timer(unit);
        sim.register_spell(
            unit,
            SpellConfig {
                cost: flat_cost(rank),
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: rank.cooldown(),
                    },
                    ..default_cast(rank.gcd(), rank.cast_time())
                },
                bonus_coefficient: rank.damage_effect().coeff(),
                max_range: f64::from(rank.max_range),
                missile_speed: f64::from(rank.speed),
                ..damage_config(rank, rank.id, masks::ARCANE_WRATH, SpellFlag::APL)
            },
        );
    }

    fn register_fire_blast_spell(&self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().fire_blast.highest();
        // Hotfix push 113531 puts Ice Lance on Fire Blast's category (19), so under Forever's
        // rules the two share the category's timer.
        let timer = match sim.rules {
            Rules::Forever => sim.category_timer(unit, i32::from(rank.category)),
            Rules::GoReference => sim.new_timer(unit),
        };
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
                ..damage_config(rank, rank.id, masks::FIRE_BLAST, SpellFlag::APL)
            },
        );
    }

    fn register_frost_nova_spell(&self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().frost_nova.highest();
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
                max_range: POINT_BLANK_RADIUS,
                ..damage_config(
                    rank,
                    rank.id,
                    masks::FROST_NOVA,
                    SpellFlag::APL | SpellFlag::BINARY,
                )
            },
        );
    }

    fn register_ice_lance_spell(&self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("ice_lance") {
            return;
        }
        let rank = hotfix_113554::row(sim.rules, spell_data().ice_lance.highest().id);
        // The client's damage effect carries no spell power coefficient (the row reads 0), as Season
        // of Discovery's reworked Ice Lance (400640) does. Go fits it to the beta logs: .10 (#726).
        let ice_lance_coefficient = 0.1;
        // Hotfix push 113531 gives every rank an 8 sec cooldown on category 19, which Fire Blast
        // shares; the frozen rows have none at level 60.
        let mut cast = default_cast(rank.gcd(), 0);
        if rank.category > 0 {
            cast.cd = Cooldown {
                timer: Some(sim.category_timer(unit, i32::from(rank.category))),
                duration: longest_cooldown(rank),
            };
        }
        sim.register_spell(
            unit,
            SpellConfig {
                missile_speed: f64::from(rank.speed),
                cost: flat_cost(rank),
                cast,
                bonus_coefficient: ice_lance_coefficient,
                max_range: f64::from(rank.max_range),
                ..damage_config(
                    rank,
                    rank.id,
                    masks::ICE_LANCE,
                    SpellFlag::APL | SpellFlag::BINARY,
                )
            },
        );
    }

    /// One of each gem is carried and all four share the conjured cooldown, which under Forever
    /// rules is also the Demonic Rune's and the healthstones' (category 1153).
    fn register_mana_gems(&self, sim: &mut Sim, unit: UnitId) {
        let data = spell_data();
        let gems: [(i32, &Row); 4] = [
            (5514, data.conjure_mana_agate_triggered.highest()),
            (5513, data.conjure_mana_jade_triggered.highest()),
            (8007, data.conjure_mana_citrine_triggered.highest()),
            (8008, data.conjure_mana_ruby_triggered.highest()),
        ];

        // Go clears the used flags on reset; preparation reads none of them.
        sim.register_reset_effect(unit, Rc::new(|_: &mut Sim| {}));

        for (item, row) in gems {
            let mana_gain = row.energize_effect().average(CHARACTER_LEVEL);
            let conjured = sim.get_conjured_cd(unit);
            let spell = sim.register_spell(
                unit,
                SpellConfig {
                    action_id: item_action(item),
                    flags: SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::APL | SpellFlag::HELPFUL,
                    class_spell_mask: masks::MANA_GEM,
                    cast: CastConfig {
                        // The item's category cooldown, which the client verifies at 2 minutes.
                        cd: Cooldown {
                            timer: Some(conjured),
                            duration: 120 * SECOND,
                        },
                        ..CastConfig::default()
                    },
                    has_extra_cast_condition: true,
                    ..SpellConfig::default()
                },
            );
            sim.add_major_cooldown(
                unit,
                MajorCooldown {
                    spell,
                    priority: mana_gain as i32,
                    cooldown_type: cooldown_type::MANA,
                    allow_spell_queueing: false,
                    timings: Vec::new(),
                },
            );
        }
    }

    /// Every rank is registered: the fire rotation drops to rank 1 when mana runs short.
    fn register_scorch_spell(&self, sim: &mut Sim, unit: UnitId) {
        self.register_improved_scorch(sim, unit);
        spell_data().scorch.each(|_, rank| {
            sim.register_spell(
                unit,
                SpellConfig {
                    rank: rank.rank_number(),
                    cost: flat_cost(rank),
                    cast: default_cast(rank.gcd(), rank.cast_time()),
                    bonus_coefficient: rank.damage_effect().coeff(),
                    max_range: f64::from(rank.max_range),
                    ..damage_config(rank, rank.id, masks::SCORCH, SpellFlag::APL)
                },
            );
        });
    }

    /// In Forever the Fire Vulnerability Improved Scorch stacks only raises the fire damage of
    /// the mage who applied it.
    fn register_improved_scorch(&self, sim: &mut Sim, unit: UnitId) {
        if self.talents.i32("improved_scorch") == 0 {
            return;
        }
        let vulnerability = spell_data().improved_scorch_triggered.highest();
        let damage_per_stack = vulnerability.effect_n(1).average(CHARACTER_LEVEL) / 100.0;

        let damage_mod = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                class_mask: masks::ALL,
                school: crate::prepare::spell::school::FIRE,
                kind: SpellModType::DamageDonePct,
                ..SpellModConfig::default()
            },
        );
        sim.register_aura(
            unit,
            AuraConfig {
                label: "Fire Vulnerability".to_string(),
                action_id: Some(spell_action(vulnerability.id)),
                duration: vulnerability.duration(),
                max_stacks: 5,
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.activate_spell_mod(damage_mod)
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.deactivate_spell_mod(damage_mod)
                })),
                on_stacks_change: Some(Rc::new(move |sim: &mut Sim, _, _, new_stacks| {
                    sim.update_spell_mod_float_value(
                        damage_mod,
                        damage_per_stack * f64::from(new_stacks),
                    )
                })),
                ..AuraConfig::default()
            },
        );
    }

    fn register_flamestrike(&self, sim: &mut Sim, unit: UnitId, rank: &Row) {
        let action = spell_action(rank.id);
        // Flamestrike's periodic damage is the spell FlamestrikeTriggered casts each tick, at
        // the same rank; the tick length is Flamestrike's own periodic dummy.
        let tick = spell_data()
            .flamestrike_triggered
            .rank(rank.rank_number())
            .damage_effect();
        let tick_length = rank.effect(dbcenums::A_PERIODIC_DUMMY, 0).period();
        let label = sim.unit(unit).label.clone();

        sim.register_spell(
            unit,
            SpellConfig {
                rank: rank.rank_number(),
                cost: flat_cost(rank),
                cast: default_cast(rank.gcd(), rank.cast_time()),
                bonus_coefficient: rank.damage_effect().coeff(),
                dot: DotConfig {
                    is_aoe: true,
                    aura: AuraConfig {
                        action_id: Some(action),
                        label: format!("Flamestrike{} {}", label, rank.rank),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: (rank.duration() / tick_length) as i32,
                    tick_length,
                    bonus_coefficient: tick.coeff(),
                    ..DotConfig::default()
                },
                max_range: f64::from(rank.max_range),
                ..damage_config(rank, rank.id, masks::FLAMESTRIKE, SpellFlag::APL)
            },
        );
    }

    /// The next spell with a cast time is instant. The row states no duration, so the buff holds
    /// until that spell is cast.
    fn register_presence_of_mind_spell(&self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("presence_of_mind") {
            return;
        }
        let rank = spell_data().presence_of_mind.highest();
        let has_cast_time =
            masks::ALL & !(masks::INSTANT_CAST | masks::BLIZZARD | masks::EVOCATION);

        let pom_mod = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                class_mask: has_cast_time,
                float_value: rank
                    .effect(
                        dbcenums::A_ADD_PCT_MODIFIER,
                        dbcenums::SPELLMOD_CASTING_TIME,
                    )
                    .average(CHARACTER_LEVEL)
                    / 100.0,
                kind: SpellModType::CastTimePct,
                ..SpellModConfig::default()
            },
        );

        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Presence of Mind".to_string(),
                action_id: Some(spell_action(rank.id)),
                duration: NEVER_EXPIRES,
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.activate_spell_mod(pom_mod)
                })),
                // Go also starts the spell's cooldown here, which only a fight observes.
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.deactivate_spell_mod(pom_mod)
                })),
                events: EventCallbacks {
                    on_cast_complete: true,
                    ..EventCallbacks::default()
                },
                ..AuraConfig::default()
            },
        );

        let timer = sim.category_timer(unit, i32::from(rank.category));
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                flags: SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::APL,
                class_spell_mask: masks::PRESENCE_OF_MIND,
                // Shared with Combustion (client category 1151).
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..CastConfig::default()
                },
                has_extra_cast_condition: true,
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
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

    fn register_arcane_power_spell(&self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("arcane_power") {
            return;
        }
        let rank = spell_data().arcane_power.highest();
        let action = spell_action(rank.id);
        let damage = rank
            .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_DAMAGE)
            .average(CHARACTER_LEVEL)
            / 100.0;

        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Arcane Power".to_string(),
                action_id: Some(action.clone()),
                duration: rank.duration(),
                ..AuraConfig::default()
            },
        );
        sim.attach_spell_mod(
            aura,
            SpellModConfig {
                class_mask: masks::ALL & !masks::FROSTFIRE_BOLT,
                float_value: damage,
                kind: SpellModType::DamageDoneFlat,
                ..SpellModConfig::default()
            },
        );
        // The SPELLMOD_DOT mask leaves out Frostfire Bolt: its hit takes the bonus, its DoT does
        // not.
        sim.attach_spell_mod(
            aura,
            SpellModConfig {
                class_mask: masks::FROSTFIRE_BOLT,
                float_value: damage,
                kind: SpellModType::DirectDamageDoneFlat,
                ..SpellModConfig::default()
            },
        );
        sim.attach_spell_mod(
            aura,
            SpellModConfig {
                class_mask: masks::ALL,
                float_value: rank
                    .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_COST)
                    .average(CHARACTER_LEVEL)
                    / 100.0,
                kind: SpellModType::PowerCostPctAdd,
                ..SpellModConfig::default()
            },
        );

        let timer = sim.new_timer(unit);
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id: action,
                flags: SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::APL,
                class_spell_mask: masks::ARCANE_POWER,
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..CastConfig::default()
                },
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
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

    fn register_blast_wave_spell(&self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("blast_wave") {
            return;
        }
        let rank = spell_data().blast_wave.highest();
        let timer = sim.new_timer(unit);
        sim.register_spell(
            unit,
            SpellConfig {
                bonus_coefficient: rank.damage_effect().coeff(),
                cost: flat_cost(rank),
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..default_cast(rank.gcd(), 0)
                },
                max_range: POINT_BLANK_RADIUS,
                ..damage_config(
                    rank,
                    rank.id,
                    masks::BLAST_WAVE,
                    SpellFlag::APL | SpellFlag::BINARY,
                )
            },
        );
    }

    fn register_pyroblast_spell(&self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("pyroblast") {
            return;
        }
        let rank = spell_data().pyroblast.highest();
        let tick = rank.periodic_effect();
        let tick_length = tick.period();
        sim.register_spell(
            unit,
            SpellConfig {
                missile_speed: f64::from(rank.speed),
                cost: flat_cost(rank),
                cast: default_cast(rank.gcd(), rank.cast_time()),
                dot: DotConfig {
                    aura: AuraConfig {
                        label: "PyroblastDoT".to_string(),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: (rank.duration() / tick_length) as i32,
                    tick_length,
                    bonus_coefficient: tick.coeff(),
                    ..DotConfig::default()
                },
                bonus_coefficient: rank.damage_effect().coeff(),
                max_range: f64::from(rank.max_range),
                ..damage_config(rank, rank.id, masks::PYROBLAST, SpellFlag::APL)
            },
        );
    }

    /// Each fire hit adds a stack of crit until the row's charges of fire crits are spent.
    fn register_combustion_spell(&self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("combustion") {
            return;
        }
        let rank = spell_data().combustion.highest();
        let triggered = spell_data().combustion_triggered.highest();
        let crit_per_stack = triggered
            .effect(
                dbcenums::A_ADD_FLAT_MODIFIER,
                dbcenums::SPELLMOD_CRITICAL_CHANCE,
            )
            .average(CHARACTER_LEVEL);

        let action = spell_action(rank.id);
        // Category 1151 in the client: Combustion and Presence of Mind share one 3 min cooldown.
        let timer = sim.category_timer(unit, i32::from(rank.category));
        let cd = Cooldown {
            timer: Some(timer),
            duration: longest_cooldown(rank),
        };

        let crit_mod = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                class_mask: masks::ALL,
                school: crate::prepare::spell::school::FIRE,
                kind: SpellModType::BonusCritPercent,
                ..SpellModConfig::default()
            },
        );

        let combust_aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Combustion".to_string(),
                action_id: Some(action.clone()),
                duration: NEVER_EXPIRES,
                max_stacks: i32::from(triggered.max_stack),
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.activate_spell_mod(crit_mod)
                })),
                // The expiry starts the cooldown and refreshes the major cooldowns, which only
                // a fight observes.
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.deactivate_spell_mod(crit_mod)
                })),
                on_stacks_change: Some(Rc::new(move |sim: &mut Sim, _, _, new_stacks| {
                    sim.update_spell_mod_float_value(
                        crit_mod,
                        crit_per_stack * f64::from(new_stacks),
                    )
                })),
                events: EventCallbacks {
                    on_spell_hit_dealt: true,
                    ..EventCallbacks::default()
                },
                ..AuraConfig::default()
            },
        );

        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id: action,
                flags: SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::APL,
                class_spell_mask: masks::COMBUSTION,
                cast: CastConfig {
                    cd,
                    ..CastConfig::default()
                },
                has_extra_cast_condition: true,
                related_self_buff: Some(combust_aura),
                ..SpellConfig::default()
            },
        );
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

    /// Resets every frost spell with a cooldown.
    fn register_cold_snap_spell(&self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("cold_snap") {
            return;
        }
        let rank = spell_data().cold_snap.highest();
        let timer = sim.new_timer(unit);
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                flags: SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::APL,
                class_spell_mask: masks::COLD_SNAP,
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..CastConfig::default()
                },
                ..SpellConfig::default()
            },
        );
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
}

/// The stats an armor aura buffs: Go's `stats.Stats` literals in `registerArmorSpells`.
pub(super) struct ArmorBuff {
    pub label: &'static str,
    pub action: i32,
    pub stats: crate::prepare::stats::Stats,
    pub casting_regen: f64,
}

impl Mage {
    /// The chosen armor is up for the whole fight. Forever has no Molten Armor, so that option,
    /// like None, applies nothing; Frost Armor picks the top rank of the line, Ice Armor.
    fn armor_buff(&self) -> Option<ArmorBuff> {
        use crate::prepare::stats::Stats;
        match self.default_mage_armor.as_str() {
            "MageArmorFrostArmor" => {
                let rank = spell_data().ice_armor.highest();
                Some(ArmorBuff {
                    label: "Ice Armor",
                    action: rank.id,
                    stats: Stats::from_pairs(&[
                        (
                            Stat::Armor,
                            rank.effect(dbcenums::A_MOD_RESISTANCE, 1)
                                .average(CHARACTER_LEVEL),
                        ),
                        (
                            Stat::FrostResistance,
                            rank.effect(dbcenums::A_MOD_RESISTANCE, 16)
                                .average(CHARACTER_LEVEL),
                        ),
                    ]),
                    casting_regen: 0.0,
                })
            }
            "MageArmorMageArmor" => {
                let rank = spell_data().mage_armor.highest();
                let resist = rank
                    .effect(dbcenums::A_MOD_RESISTANCE, 126)
                    .average(CHARACTER_LEVEL);
                Some(ArmorBuff {
                    label: "Mage Armor",
                    action: rank.id,
                    stats: Stats::from_pairs(&[
                        (Stat::ArcaneResistance, resist),
                        (Stat::FireResistance, resist),
                        (Stat::FrostResistance, resist),
                        (Stat::NatureResistance, resist),
                        (Stat::ShadowResistance, resist),
                    ]),
                    casting_regen: rank
                        .effect(dbcenums::A_MOD_MANA_REGEN_INTERRUPT, 0)
                        .average(CHARACTER_LEVEL)
                        / 100.0,
                })
            }
            _ => None,
        }
    }

    fn register_armor_spells(&self, sim: &mut Sim, unit: UnitId) {
        let Some(armor) = self.armor_buff() else {
            return;
        };
        let casting_regen = armor.casting_regen;
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: armor.label.to_string(),
                action_id: Some(spell_action(armor.action)),
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.unit_mut(unit).pseudo_stats.spirit_regen_rate_casting += casting_regen;
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.unit_mut(unit).pseudo_stats.spirit_regen_rate_casting -= casting_regen;
                })),
                ..AuraConfig::default()
            },
        );
        let aura = sim.make_permanent(aura);
        sim.attach_stats_buff(aura, armor.stats);
    }
}
