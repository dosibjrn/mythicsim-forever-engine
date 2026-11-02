//! The Hunter's spell registrations: Go sim/hunter's `RegisterSpells` and the `register*Spell`
//! functions it calls, in Go's order. A closure Go gives a spell config (`ApplyEffects`,
//! `OnTick` and the like) only runs in a fight, so what preparation keeps of it is the field that
//! says it is there: `has_extra_cast_condition`, the related buff.

use std::rc::Rc;

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use crate::prepare::character::constants::{CHARACTER_LEVEL, MAX_MELEE_RANGE, MIN_RANGED_RANGE};
use crate::prepare::character::{cooldown_type, MajorCooldown};
use crate::prepare::dbcenums;
use crate::prepare::hotfix_113554;
use crate::prepare::sim::SpellId;
use crate::prepare::sim::{
    AuraConfig, AuraId, Cooldown, Duration, EventCallbacks, Rules, Sim, MILLISECOND, NEVER_EXPIRES,
    SECOND,
};
use crate::prepare::spell::{
    school, Cast, CastConfig, CostOptions, DefenseType, DotConfig, ProcMask, SpellConfig,
    SpellFlag, GCD_DEFAULT,
};
use crate::prepare::spelldata::Spell as Row;

use super::spell_data::spell_data;
use super::{masks, Hunter, HUNTER_BASE_MAX_RANGE};

fn spell_action(id: i32) -> ActionId {
    ActionId::spell(id)
}

/// `ManaCost: core.ManaCostOptions{FlatCost: int32(rank.Cost())}`.
fn flat_cost(row: &Row) -> CostOptions {
    CostOptions {
        mana_flat_cost: row.cost() as i32,
        ..CostOptions::default()
    }
}

/// `ManaCost: rank.ManaCost()`.
fn row_mana_cost(row: &Row) -> CostOptions {
    let mana = row.mana_cost();
    CostOptions {
        mana_base_cost_percent: mana.base_cost_percent,
        mana_flat_cost: mana.flat_cost,
        ..CostOptions::default()
    }
}

/// `max(rank.Cooldown(), rank.CategoryCooldown())`.
pub(super) fn longest_cooldown(row: &Row) -> Duration {
    row.cooldown().max(row.category_cooldown())
}

/// Go `RegisterRangedSpell`'s defaults for a hunter's shot or sting.
pub(super) fn ranged_config(mut config: SpellConfig) -> SpellConfig {
    if config.missile_speed == 0.0 {
        config.missile_speed = 40.0;
    }
    config.min_range = MIN_RANGED_RANGE;
    config.max_range = HUNTER_BASE_MAX_RANGE;
    config.cast.default_cast.gcd = GCD_DEFAULT;
    config.cast.ignore_haste = true;
    // A cast time gets Hunter's ranged haste: closures that only run in a fight.
    if config.damage_multiplier == 0.0 && config.damage_multiplier_additive == 0.0 {
        config.damage_multiplier = 1.0;
        if config.threat_multiplier == 0.0 {
            config.threat_multiplier = 1.0;
        }
    }
    config
}

/// The spell power coefficient of Arcane Shot, Serpent Sting and Volley's ticks. Client 70425
/// and 70450 give them none and Bae's in-game tests find no Hunter ability scaling with spell
/// power, so Go takes none (`HunterAbilitiesIgnoreSpellPower`, patch 173). Classic's values,
/// which Go restores with that switch off, were Arcane Shot .204 to .429 by rank, Serpent
/// Sting 1.0 over its duration and Volley .056 a tick.
const HUNTER_ABILITY_SPELL_POWER_COEFFICIENT: f64 = 0.0;

/// Multi-Shot's cast time under Forever's rules, before ranged haste. Its client row (2643) has
/// none, but every Multi-Shot in the Forever beta logs has a cast start: a median 0.44 to 0.45
/// seconds with the quiver's 15% (0.5 / 1.15 is 0.435) and 0.32 under Rapid Fire as well (0.5 /
/// (1.15 x 1.4) is 0.31).
const FOREVER_MULTI_SHOT_CAST_TIME: Duration = 500 * MILLISECOND;

/// Go `arcaneShotRAPShare()`: 0.10 of ranged attack power since patch 174, the Hunter
/// Discord's three dedicated tests (0.10, 0.10, 0.11) taken with the four beta logs' fit
/// (0.10 to 0.12). Go's `ArcaneShotRAPFromHunterTests` switched off restores the logs-only 0.11.
pub(super) const ARCANE_SHOT_RAP_COEFFICIENT: f64 = 0.10;
/// Go `rapPerTick` of Serpent Sting.
pub(super) const SERPENT_STING_RAP_PER_TICK: f64 = 0.035;
/// Summon Hawk's attack power share of the dive bomb.
pub(super) const SUMMON_HAWK_RAP_SHARE: f64 = 0.05;
/// Summon Hawk's swing damage as a share of the rank's dive bomb base (`swingDamage`).
pub(super) const SUMMON_HAWK_SWING_SHARE: f64 = 0.35;
/// Go `volleyTickDamage`.
pub(super) const VOLLEY_TICK_DAMAGE: [f64; 4] = [0.0, 70.0, 91.0, 112.0];
/// Go `trapAttackPowerShare` (patch 147).
pub(super) const TRAP_ATTACK_POWER_SHARE: f64 = 0.13;
/// Go `explosiveTrapRange`.
pub(super) const EXPLOSIVE_TRAP_RANGE: [[f64; 2]; 4] =
    [[0.0, 0.0], [104.0, 135.0], [145.0, 193.0], [208.0, 265.0]];

impl Hunter {
    /// Go `Hunter.RegisterSpells`.
    pub(super) fn register_spells(&mut self, sim: &mut Sim) {
        let unit = self.unit;
        // Forever pairs Aimed Shot's cooldown with Multi-Shot rather than Arcane Shot.
        let multi_shot_timer = sim.new_timer(unit);
        let arcane_shot_timer = sim.new_timer(unit);

        self.register_aspects(sim);
        self.register_arcane_shot_spell(sim, arcane_shot_timer);
        self.register_aimed_shot_spell(sim, multi_shot_timer);
        self.register_multi_shot_spell(sim, multi_shot_timer);
        self.register_sniper_shot_spell(sim);
        self.register_summon_hawk_spell(sim, arcane_shot_timer);
        self.register_serpent_sting_spell(sim);
        self.register_volley_spell(sim);

        self.register_raptor_strike_spell(sim);
        self.register_mongoose_bite_spell(sim);
        self.register_lacerating_strikes_spell(sim);
        self.register_wing_clip_spell(sim);
        self.register_strider_kick_spell(sim);

        self.register_explosive_trap_spell(sim);
        self.register_immolation_trap_spell(sim);
        self.register_freezing_trap_spell(sim);

        self.register_rapid_fire_cd(sim);
    }

    /// Go `registerAimedShotSpell`: Aimed Shot is no longer a talent. It keeps every rank on the
    /// hunter with a 2 sec cast, and Forever puts its cooldown on the Multi-Shot timer.
    fn register_aimed_shot_spell(&mut self, sim: &mut Sim, timer: crate::prepare::sim::TimerId) {
        let rank = spell_data().aimed_shot.highest();
        self.aimed_shot = Some(sim.register_spell(
            self.unit,
            ranged_config(SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                class_spell_mask: masks::AIMED_SHOT,
                proc_mask: ProcMask::RANGED_SPECIAL,
                flags: SpellFlag::MELEE_METRICS | SpellFlag::APL,
                missile_speed: f64::from(rank.speed),
                cost: flat_cost(rank),
                cast: CastConfig {
                    default_cast: Cast {
                        cast_time: rank.cast_time(),
                        ..Cast::default()
                    },
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..CastConfig::default()
                },
                bonus_coefficient: 1.0,
                ..SpellConfig::default()
            }),
        ));
    }

    /// Go `registerArcaneShotSpell`.
    fn register_arcane_shot_spell(&mut self, sim: &mut Sim, timer: crate::prepare::sim::TimerId) {
        let rank = spell_data().arcane_shot.highest();
        self.arcane_shot = Some(sim.register_spell(
            self.unit,
            ranged_config(SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                class_spell_mask: masks::ARCANE_SHOT,
                proc_mask: ProcMask::RANGED_SPECIAL,
                flags: SpellFlag::MELEE_METRICS | SpellFlag::APL,
                missile_speed: f64::from(rank.speed),
                cost: flat_cost(rank),
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..CastConfig::default()
                },
                bonus_coefficient: HUNTER_ABILITY_SPELL_POWER_COEFFICIENT,
                ..SpellConfig::default()
            }),
        ));
    }

    /// Go `registerMultiShotSpell`: the beta client has one rank of Multi-Shot, with a 6 sec
    /// cooldown shared with Aimed Shot. Its row has no cast time, so the reference casts it at
    /// once; under Forever's rules it takes [`FOREVER_MULTI_SHOT_CAST_TIME`], which the ranged
    /// haste multiplier divides as it does Aimed Shot's (README, Rust-only mechanics).
    fn register_multi_shot_spell(&mut self, sim: &mut Sim, timer: crate::prepare::sim::TimerId) {
        let rank = spell_data().multi_shot.highest();
        let cast_time = match sim.rules {
            Rules::Forever => FOREVER_MULTI_SHOT_CAST_TIME,
            Rules::GoReference => rank.cast_time(),
        };
        self.multi_shot = Some(sim.register_spell(
            self.unit,
            ranged_config(SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                class_spell_mask: masks::MULTI_SHOT,
                proc_mask: ProcMask::RANGED_SPECIAL,
                flags: SpellFlag::MELEE_METRICS | SpellFlag::APL,
                missile_speed: f64::from(rank.speed),
                cost: row_mana_cost(rank),
                cast: CastConfig {
                    default_cast: Cast {
                        cast_time,
                        ..Cast::default()
                    },
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..CastConfig::default()
                },
                bonus_coefficient: 1.0,
                ..SpellConfig::default()
            }),
        ));
    }

    /// Go `registerSniperShotSpell`: from the beta client, a 4 sec cast on a 15 sec cooldown.
    fn register_sniper_shot_spell(&mut self, sim: &mut Sim) {
        if !self.flag("sniper_shot") {
            return;
        }
        let unit = self.unit;
        let rank = spell_data().sniper_shot.highest();
        let timer = sim.new_timer(unit);
        self.sniper_shot = Some(sim.register_spell(
            unit,
            ranged_config(SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                class_spell_mask: masks::SNIPER_SHOT,
                proc_mask: ProcMask::RANGED_SPECIAL,
                flags: SpellFlag::MELEE_METRICS | SpellFlag::APL,
                missile_speed: f64::from(rank.speed),
                cost: flat_cost(rank),
                cast: CastConfig {
                    default_cast: Cast {
                        cast_time: rank.cast_time(),
                        ..Cast::default()
                    },
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..CastConfig::default()
                },
                bonus_coefficient: 1.0,
                ..SpellConfig::default()
            }),
        ));
    }

    /// Go `registerSummonHawkSpell`: Summon Hawk shares its cooldown with Arcane Shot. Each
    /// hawk's assault is a dot of its own.
    fn register_summon_hawk_spell(&mut self, sim: &mut Sim, timer: crate::prepare::sim::TimerId) {
        if !self.flag("summon_hawk") {
            return;
        }
        let unit = self.unit;
        let rank = spell_data().summon_hawk.highest();
        let hawk_duration = spell_data().summon_hawk_triggered.by_id(1293248).duration();
        let swing_interval = 2500 * MILLISECOND;
        let label = sim.unit(unit).label.clone();

        let hawk_count = rank.effect_n(3).base_points as i32;
        for i in 0..hawk_count {
            sim.register_spell(
                unit,
                SpellConfig {
                    action_id: ActionId {
                        spell_id: rank.id,
                        tag: i + 1,
                        ..ActionId::default()
                    },
                    spell_school: rank.spell_school(),
                    defense_type: DefenseType::Melee,
                    class_spell_mask: masks::SUMMON_HAWK,
                    proc_mask: ProcMask::EMPTY,
                    flags: SpellFlag::MELEE_METRICS,
                    damage_multiplier: 1.0,
                    threat_multiplier: 1.0,
                    dot: DotConfig {
                        aura: AuraConfig {
                            label: format!("Summon Hawk {}{}", i + 1, label),
                            ..AuraConfig::default()
                        },
                        number_of_ticks: (hawk_duration / swing_interval) as i32,
                        tick_length: swing_interval,
                        affected_by_real_haste: true,
                        ..DotConfig::default()
                    },
                    ..SpellConfig::default()
                },
            );
        }

        self.summon_hawk = Some(sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                defense_type: DefenseType::Melee,
                class_spell_mask: masks::SUMMON_HAWK,
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::MELEE_METRICS | SpellFlag::APL,
                max_range: f64::from(rank.max_range),
                cost: flat_cost(rank),
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: rank.gcd(),
                        ..Cast::default()
                    },
                    ignore_haste: true,
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..CastConfig::default()
                },
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        ));
    }

    /// Go `registerSerpentStingSpell`.
    fn register_serpent_sting_spell(&mut self, sim: &mut Sim) {
        let rank = spell_data().serpent_sting.highest_for(sim.rules);
        let tick = rank.periodic_effect();
        let tick_length = tick.period();
        let number_of_ticks = (rank.duration() / tick_length) as i32;
        self.serpent_sting = Some(sim.register_spell(
            self.unit,
            ranged_config(SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                class_spell_mask: masks::SERPENT_STING,
                proc_mask: ProcMask::RANGED_SPECIAL,
                flags: SpellFlag::APL | SpellFlag::POISON,
                missile_speed: f64::from(rank.speed),
                cost: flat_cost(rank),
                dot: DotConfig {
                    aura: AuraConfig {
                        label: "Serpent Sting".to_string(),
                        tag: "Sting".to_string(),
                        ..AuraConfig::default()
                    },
                    number_of_ticks,
                    tick_length,
                    bonus_coefficient: HUNTER_ABILITY_SPELL_POWER_COEFFICIENT,
                    ..DotConfig::default()
                },
                ..SpellConfig::default()
            }),
        ));
    }

    /// Go `registerVolleySpell`.
    fn register_volley_spell(&mut self, sim: &mut Sim) {
        let rank = spell_data().volley.highest();
        self.volley = Some(sim.register_spell(
            self.unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                class_spell_mask: masks::VOLLEY,
                proc_mask: ProcMask::SPELL_DAMAGE,
                flags: SpellFlag::CHANNELED | SpellFlag::APL,
                max_range: f64::from(rank.max_range),
                cost: flat_cost(rank),
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: rank.gcd(),
                        ..Cast::default()
                    },
                    ignore_haste: true,
                    ..CastConfig::default()
                },
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                dot: DotConfig {
                    is_aoe: true,
                    aura: AuraConfig {
                        label: "Volley".to_string(),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: 6,
                    tick_length: SECOND,
                    // The tick spells have no coefficient and the channel's dummy effect carries
                    // .03, the same placeholder Blizzard and Rain of Fire carry; Go takes none
                    // (patch 173).
                    bonus_coefficient: HUNTER_ABILITY_SPELL_POWER_COEFFICIENT,
                    ..DotConfig::default()
                },
                ..SpellConfig::default()
            },
        ));
    }

    /// Go `registerRaptorStrikeSpell`: Raptor Strike replaces the next main-hand swing rather
    /// than costing a global, so the cast the rotation presses only queues it.
    fn register_raptor_strike_spell(&mut self, sim: &mut Sim) {
        let unit = self.unit;
        let rank = spell_data().raptor_strike.highest();
        self.raptor_strike_hit = Some(sim.register_spell(
            unit,
            SpellConfig {
                action_id: ActionId {
                    spell_id: rank.id,
                    tag: 1,
                    ..ActionId::default()
                },
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                class_spell_mask: masks::RAPTOR_STRIKE,
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                flags: SpellFlag::MELEE_METRICS | SpellFlag::NO_ON_CAST_COMPLETE,
                max_range: MAX_MELEE_RANGE,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                bonus_coefficient: 1.0,
                ..SpellConfig::default()
            },
        ));

        let timer = sim.new_timer(unit);
        let raptor_strike = sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                class_spell_mask: masks::RAPTOR_STRIKE,
                proc_mask: ProcMask::MELEE_MH_SPECIAL | ProcMask::MELEE_MH_AUTO,
                flags: SpellFlag::MELEE_METRICS,
                max_range: MAX_MELEE_RANGE,
                cost: flat_cost(rank),
                cast: CastConfig {
                    default_cast: Cast {
                        non_empty: true,
                        ..Cast::default()
                    },
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..CastConfig::default()
                },
                ..SpellConfig::default()
            },
        );
        self.raptor_strike = Some(raptor_strike);
        self.raptor_strike_id = Some(rank.id);

        self.make_raptor_strike_queue_spell(sim, rank.id);
    }

    /// Go `makeRaptorStrikeQueueSpell`.
    fn make_raptor_strike_queue_spell(&mut self, sim: &mut Sim, raptor_strike_id: i32) {
        let unit = self.unit;
        let queue_id = ActionId {
            spell_id: raptor_strike_id,
            tag: 3,
            ..ActionId::default()
        };
        sim.register_aura(
            unit,
            AuraConfig {
                label: "Raptor Strike Queued".to_string(),
                action_id: Some(queue_id.clone()),
                duration: NEVER_EXPIRES,
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.unit_mut(unit).pseudo_stats.disable_dw_miss_penalty = true;
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.unit_mut(unit).pseudo_stats.disable_dw_miss_penalty = false;
                })),
                ..AuraConfig::default()
            },
        );
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: queue_id,
                class_spell_mask: masks::RAPTOR_STRIKE_QUEUE,
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::MELEE_METRICS | SpellFlag::APL,
                has_extra_cast_condition: true,
                ..SpellConfig::default()
            },
        );
    }

    /// Go `registerMongooseBiteSpell`.
    fn register_mongoose_bite_spell(&mut self, sim: &mut Sim) {
        let unit = self.unit;
        let rank = spell_data().mongoose_bite.highest();
        // The aura is only a pre-requisite for Mongoose Bite: a dodge opens the window, and Expose
        // Prey opens it off any landed hit on a marked target.
        let defensive_window = spell_data().expose_prey_triggered.rank(1);
        self.defensive_state = Some(sim.register_aura(
            unit,
            AuraConfig {
                label: "Defensive State".to_string(),
                action_id: Some(spell_action(defensive_window.id)),
                duration: defensive_window.duration(),
                ..AuraConfig::default()
            },
        ));
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Defensive State - Trigger".to_string(),
                trigger_immediately: true,
                outcome: HitOutcome::DODGE,
                callback: CallbackMask::ON_SPELL_HIT_TAKEN,
                ..ProcTrigger::default()
            },
        );

        let timer = sim.new_timer(unit);
        self.mongoose_bite = Some(sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                class_spell_mask: masks::MONGOOSE_BITE,
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                flags: SpellFlag::MELEE_METRICS | SpellFlag::APL,
                max_range: f64::from(rank.max_range),
                cost: flat_cost(rank),
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: rank.gcd(),
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
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                bonus_coefficient: 1.0,
                ..SpellConfig::default()
            },
        ));
    }

    /// Go `registerLaceratingStrikesSpell`: the bleed lasts 21 sec and carries 40% of the
    /// Mongoose Bite that applied it. It reports under that bleed's own id.
    fn register_lacerating_strikes_spell(&mut self, sim: &mut Sim) {
        if !self.flag("lacerating_strikes") {
            return;
        }
        let unit = self.unit;
        let label = sim.unit(unit).label.clone();
        self.lacerating_strikes = Some(sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(spell_data().lacerating_strikes_triggered.highest().id),
                spell_school: school::PHYSICAL,
                defense_type: DefenseType::Melee,
                class_spell_mask: masks::LACERATING_STRIKES,
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::MELEE_METRICS | SpellFlag::NO_ON_CAST_COMPLETE,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                dot: DotConfig {
                    aura: AuraConfig {
                        label: format!("Lacerating Strikes{label}"),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: 7,
                    tick_length: 3 * SECOND,
                    ..DotConfig::default()
                },
                ..SpellConfig::default()
            },
        ));
    }

    /// Go `registerWingClipSpell`.
    fn register_wing_clip_spell(&mut self, sim: &mut Sim) {
        let rank = spell_data().wing_clip.highest();
        self.wing_clip = Some(sim.register_spell(
            self.unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                class_spell_mask: masks::WING_CLIP,
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                flags: SpellFlag::MELEE_METRICS | SpellFlag::APL | SpellFlag::BINARY,
                max_range: f64::from(rank.max_range),
                cost: flat_cost(rank),
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: rank.gcd(),
                        ..Cast::default()
                    },
                    ignore_haste: true,
                    ..CastConfig::default()
                },
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        ));
    }

    /// Go `registerStriderKickSpell`.
    fn register_strider_kick_spell(&mut self, sim: &mut Sim) {
        if !self.flag("strider_kick") {
            return;
        }
        let unit = self.unit;
        let rank = spell_data().strider_kick.highest();
        // The client row's third effect, `StriderKickSpeedsUp` (the fork's patch 120): 30% movement
        // speed for the spell's 3 sec, an active movement speed effect.
        let speed = rank.effect(dbcenums::A_MOD_INCREASE_SPEED, 0).percent();
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Strider Kick".to_string(),
                action_id: Some(spell_action(rank.id)),
                duration: rank.duration(),
                ..AuraConfig::default()
            },
        );
        sim.new_active_movement_speed_effect(aura, speed);
        let timer = sim.new_timer(unit);
        self.strider_kick = Some(sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                class_spell_mask: masks::STRIDER_KICK,
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                flags: SpellFlag::MELEE_METRICS | SpellFlag::APL,
                max_range: f64::from(rank.max_range),
                cost: row_mana_cost(rank),
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: rank.gcd(),
                        ..Cast::default()
                    },
                    ignore_haste: true,
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..CastConfig::default()
                },
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                bonus_coefficient: 1.0,
                ..SpellConfig::default()
            },
        ));
    }

    /// Go `trapCooldown`: each trap's cooldown is its client category's.
    fn trap_cooldown(&self, sim: &mut Sim, rank: &Row) -> Cooldown {
        Cooldown {
            timer: Some(sim.category_timer(self.unit, i32::from(rank.category))),
            duration: rank.category_cooldown(),
        }
    }

    /// Go `registerExplosiveTrapSpell`.
    fn register_explosive_trap_spell(&mut self, sim: &mut Sim) {
        let rank = spell_data().explosive_trap.highest();
        let cd = self.trap_cooldown(sim, rank);
        self.explosive_trap = Some(sim.register_spell(
            self.unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: school::FIRE,
                defense_type: DefenseType::Magic,
                class_spell_mask: masks::EXPLOSIVE_TRAP,
                proc_mask: ProcMask::SPELL_DAMAGE,
                flags: SpellFlag::APL,
                max_range: MAX_MELEE_RANGE,
                cost: flat_cost(rank),
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: GCD_DEFAULT,
                        ..Cast::default()
                    },
                    ignore_haste: true,
                    cd,
                    ..CastConfig::default()
                },
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                dot: DotConfig {
                    is_aoe: true,
                    aura: AuraConfig {
                        label: "Explosive Trap".to_string(),
                        tag: "ExplosiveTrap".to_string(),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: 10,
                    tick_length: 2 * SECOND,
                    ..DotConfig::default()
                },
                ..SpellConfig::default()
            },
        ));
    }

    /// Go `registerImmolationTrapSpell`.
    fn register_immolation_trap_spell(&mut self, sim: &mut Sim) {
        let rank = spell_data().immolation_trap.highest();
        let effect = spell_data()
            .immolation_trap_triggered
            .rank(rank.rank_number());
        let tick = effect.periodic_effect();
        let cd = self.trap_cooldown(sim, rank);
        self.immolation_trap = Some(sim.register_spell(
            self.unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: school::FIRE,
                defense_type: DefenseType::Magic,
                class_spell_mask: masks::IMMOLATION_TRAP,
                proc_mask: ProcMask::SPELL_DAMAGE,
                flags: SpellFlag::APL,
                max_range: MAX_MELEE_RANGE,
                cost: flat_cost(rank),
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: GCD_DEFAULT,
                        ..Cast::default()
                    },
                    ignore_haste: true,
                    cd,
                    ..CastConfig::default()
                },
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                dot: DotConfig {
                    aura: AuraConfig {
                        label: "Immolation Trap".to_string(),
                        tag: "ImmolationTrap".to_string(),
                        ..AuraConfig::default()
                    },
                    // 5 ticks 3 sec apart in both clients.
                    number_of_ticks: (effect.duration() / tick.period()) as i32,
                    tick_length: tick.period(),
                    ..DotConfig::default()
                },
                ..SpellConfig::default()
            },
        ));
    }

    /// Go `registerFreezingTrapSpell`: Freezing Trap deals no damage; it is registered so the
    /// trap talents and the shared cooldown have something to act on.
    /// Hotfix push 113554 moves it from category 2183 to Explosive and Immolation Trap's 411
    /// under Forever's rules, so the three share one cooldown.
    fn register_freezing_trap_spell(&mut self, sim: &mut Sim) {
        let rank = hotfix_113554::row(sim.rules, spell_data().freezing_trap.rank(1).id);
        let cd = self.trap_cooldown(sim, rank);
        self.freezing_trap = Some(sim.register_spell(
            self.unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: school::FROST,
                defense_type: DefenseType::Magic,
                class_spell_mask: masks::FREEZING_TRAP,
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::APL,
                max_range: MAX_MELEE_RANGE,
                cost: flat_cost(rank),
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: GCD_DEFAULT,
                        ..Cast::default()
                    },
                    ignore_haste: true,
                    cd,
                    ..CastConfig::default()
                },
                ..SpellConfig::default()
            },
        ));
    }

    /// Go `registerRapidFireCD`.
    fn register_rapid_fire_cd(&mut self, sim: &mut Sim) {
        let unit = self.unit;
        let rank = spell_data().rapid_fire.highest();
        let action_id = spell_action(rank.id);
        let haste_multiplier = 1.0
            + rank
                .effect(dbcenums::A_MOD_RANGED_HASTE, 0)
                .average(CHARACTER_LEVEL)
                / 100.0;

        // Forever: ranged and melee attack speed, where Classic's was ranged only.
        let aura: AuraId = sim.register_aura(
            unit,
            AuraConfig {
                label: "Rapid Fire".to_string(),
                action_id: Some(action_id.clone()),
                duration: rank.duration(),
                on_gain: Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
                    let unit = sim.aura(aura).unit;
                    sim.multiply_attack_speed(unit, haste_multiplier);
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
                    let unit = sim.aura(aura).unit;
                    sim.multiply_attack_speed(unit, 1.0 / haste_multiplier);
                })),
                events: EventCallbacks::default(),
                ..AuraConfig::default()
            },
        );
        self.rapid_fire_aura = Some(aura);

        // Rapid Killing takes a minute off a rank (client curve -60000/-120000 ms).
        let rapid_killing = spell_data()
            .rapid_killing
            .effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_COOLDOWN)
            .value_at(self.t("rapid_killing"));
        let cooldown = longest_cooldown(rank) + (rapid_killing as Duration) * MILLISECOND;

        let timer = sim.new_timer(unit);
        let spell: SpellId = sim.register_spell(
            unit,
            SpellConfig {
                action_id,
                spell_school: rank.spell_school(),
                class_spell_mask: masks::RAPID_FIRE,
                proc_mask: ProcMask::EMPTY,
                cost: flat_cost(rank),
                cast: CastConfig {
                    default_cast: Cast {
                        non_empty: true,
                        ..Cast::default()
                    },
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: cooldown,
                    },
                    ..CastConfig::default()
                },
                has_extra_cast_condition: true,
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
        self.rapid_fire = Some(spell);

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
