//! Preparing MythicSim's healer sims, which Rust runs alone (no Go counterpart; see
//! `core::fight::heals`). A class registers each heal rank from its client row here, applies its
//! heal talents with [`heal_talent_mod`], and exports the ranks as one [`heals_effect`].
//!
//! A rank is a helpful spell on the healing proc mask, costed, cast and cooled down as its row
//! states, with the direct heal's or the shield's coefficient on healing power. A heal over time
//! carries a dot on the player: the row's ticks and period, each tick's coefficient, and under
//! Forever's rules a crit roll on each tick where the row states Periodic Can Crit. A talent's
//! client spell modifier lands on the spells its own client mask names, so a heal registers with
//! the client flags of its row.

use serde_json::{json, Value};

use super::dbcenums::{
    A_ADD_FLAT_MODIFIER, A_ADD_PCT_MODIFIER, SPELLMOD_CASTING_TIME, SPELLMOD_COOLDOWN,
    SPELLMOD_COST, SPELLMOD_CRITICAL_CHANCE, SPELLMOD_DAMAGE, SPELLMOD_DOT, SPELLMOD_EFFECT1,
    SPELLMOD_GLOBAL_COOLDOWN,
};
use super::env::Environment;
use super::sim::{Cooldown, Duration, Rules, Sim, SpellId, TimerId, UnitId};
use super::spell::{Cast, CastConfig, CostOptions, DotConfig, ProcMask, SpellConfig, SpellFlag};
use super::spell_mod::{SpellModConfig, SpellModType};
use super::spelldata::{Effect, Ladder, Spell as Row};
use crate::contracts::prepared_v2::{ActionId, CritAbsorb, SmartHeal};
use crate::contracts::request::{Message, Value as RequestValue};
use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::sim::AuraConfig;
use crate::prepare::stats::Stat;

/// What a heal rank does, beside what its row states.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HealShape {
    /// Players the direct heal, or each tick of a party heal over time, lands on.
    pub targets: u32,
    /// Each further player's share of the heal before it: a chain heal's falloff.
    pub jump_multiplier: f64,
    /// Under Forever's rules, a heal that follows the party's damage (`prepare::party_damage`).
    pub smart: Option<SmartHeal>,
}

impl HealShape {
    /// One player.
    pub(crate) const SINGLE: HealShape = HealShape {
        targets: 1,
        jump_multiplier: 1.0,
        smart: None,
    };

    /// A party heal: each of `targets` players takes the full heal.
    pub(crate) fn party(targets: u32) -> HealShape {
        HealShape {
            targets,
            jump_multiplier: 1.0,
            smart: None,
        }
    }

    /// The shape with a heal that follows the party's damage under Forever's rules.
    pub(crate) fn smart(self, smart: SmartHeal) -> HealShape {
        HealShape {
            smart: Some(smart),
            ..self
        }
    }
}

/// One registered heal rank, for the export.
#[derive(Clone, Debug)]
pub(crate) struct HealRank {
    pub spell: SpellId,
    pub row: &'static Row,
    pub shape: HealShape,
    /// The direct heal's roll, when the rank heals at once.
    pub direct: Option<(f64, f64)>,
    /// Each tick's amount, when the rank heals over time.
    pub tick: Option<f64>,
    /// The heal's row states Periodic Can Crit, which Forever's rules follow for its ticks.
    pub tick_can_crit: bool,
    /// The shield's absorb, when the rank shields.
    pub absorb: Option<f64>,
    /// Under Forever's rules, the share of a crit of this heal that becomes an absorb.
    pub crit_absorb: Option<CritAbsorb>,
}

/// Registers one heal rank of `row` for `unit`: its cost, cast, cooldown and the parts its row
/// states. `class_mask` is the class's own mask for the spell, for its other code to name it;
/// `cd_timer` is a cooldown the ranks share. Returns the rank for the export.
pub(crate) fn register_heal(
    sim: &mut Sim,
    unit: UnitId,
    row: &'static Row,
    class_mask: i64,
    shape: HealShape,
    cd_timer: Option<TimerId>,
) -> HealRank {
    register_heal_from(
        sim,
        unit,
        row,
        row.heal_effect(),
        class_mask,
        shape,
        cd_timer,
    )
}

/// [`register_heal`] with the direct heal read from `heal`, for a row that states it on another
/// effect than a heal: Prayer of Mending's heal is a dummy effect of its cast row.
pub(crate) fn register_heal_from(
    sim: &mut Sim,
    unit: UnitId,
    row: &'static Row,
    heal: &'static Effect,
    class_mask: i64,
    shape: HealShape,
    cd_timer: Option<TimerId>,
) -> HealRank {
    let periodic = row.periodic_effect();
    let absorb = row.absorb_effect();
    let direct = (!heal.is_nil()).then(|| (heal.average(CHARACTER_LEVEL), heal.variance));
    let tick = (!periodic.is_nil()).then(|| periodic.average(CHARACTER_LEVEL));
    let shield = (!absorb.is_nil()).then(|| absorb.average(CHARACTER_LEVEL));
    let coefficient = if direct.is_some() {
        heal.coeff()
    } else if shield.is_some() {
        absorb.coeff()
    } else {
        0.0
    };
    let mana = row.mana_cost();
    let cooldown = row.cooldown().max(row.category_cooldown());
    let mut config = SpellConfig {
        action_id: ActionId {
            spell_id: row.id,
            ..ActionId::default()
        },
        spell_school: row.spell_school(),
        defense_type: row.defense_type_core(),
        proc_mask: ProcMask::SPELL_HEALING,
        flags: SpellFlag::APL | SpellFlag::HELPFUL,
        class_spell_mask: class_mask,
        class_flags: row.class_flags,
        rank: row.rank_number(),
        max_range: f64::from(row.max_range),
        cost: CostOptions {
            mana_base_cost_percent: mana.base_cost_percent,
            mana_flat_cost: mana.flat_cost,
            ..CostOptions::default()
        },
        cast: CastConfig {
            default_cast: Cast {
                gcd: row.gcd(),
                cast_time: row.cast_time(),
                ..Cast::default()
            },
            cd: if cooldown > 0 {
                Cooldown {
                    timer: Some(cd_timer.unwrap_or_else(|| sim.new_timer(unit))),
                    duration: cooldown,
                }
            } else {
                Cooldown::default()
            },
            ..CastConfig::default()
        },
        damage_multiplier: 1.0,
        threat_multiplier: 1.0,
        bonus_coefficient: coefficient,
        ..SpellConfig::default()
    };
    if tick.is_some() {
        let period = periodic.period();
        let duration = row.duration();
        config.hot = DotConfig {
            // One aura a rank: the runtime keeps one dot an aura.
            aura: AuraConfig {
                label: format!("{} {}", row.name, row.id),
                ..AuraConfig::default()
            },
            tick_length: period,
            number_of_ticks: ticks(duration, period),
            self_only: true,
            bonus_coefficient: periodic.coeff(),
            ..DotConfig::default()
        };
    }
    let spell = sim.register_spell(unit, config);
    HealRank {
        spell,
        row,
        shape,
        direct,
        tick,
        tick_can_crit: tick.is_some() && row.periodic_can_crit(),
        absorb: shield,
        crit_absorb: None,
    }
}

/// A channelled heal's numbers, which its rows state across several spells: the direct heal,
/// if any, and each tick.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ChannelHeal {
    /// The heal that lands with the cast: its roll and coefficient.
    pub direct: Option<(f64, f64, f64)>,
    /// Each tick's amount and coefficient, how many and how far apart.
    pub tick: f64,
    pub tick_coefficient: f64,
    pub ticks: i32,
    pub period: Duration,
    pub tick_can_crit: bool,
}

/// A channelled heal (Penance on a friend, Tranquility) as a heal rank: `row` states the cost,
/// the cooldown and its category; the heal lands as its ticks, and the channel holds the
/// healer for its whole length, read as a global cooldown that long. `cd_timer` is a cooldown
/// the ranks share.
pub(crate) fn register_channel_heal(
    sim: &mut Sim,
    unit: UnitId,
    row: &'static Row,
    heal: ChannelHeal,
    class_mask: i64,
    shape: HealShape,
    cd_timer: Option<TimerId>,
) -> HealRank {
    let mana = row.mana_cost();
    let cooldown = row.cooldown().max(row.category_cooldown());
    let channel = heal.period * Duration::from(heal.ticks);
    let config = SpellConfig {
        action_id: ActionId {
            spell_id: row.id,
            ..ActionId::default()
        },
        spell_school: row.spell_school(),
        defense_type: row.defense_type_core(),
        proc_mask: ProcMask::SPELL_HEALING,
        flags: SpellFlag::APL | SpellFlag::HELPFUL,
        class_spell_mask: class_mask,
        class_flags: row.class_flags,
        rank: row.rank_number(),
        max_range: f64::from(row.max_range),
        cost: CostOptions {
            mana_base_cost_percent: mana.base_cost_percent,
            mana_flat_cost: mana.flat_cost,
            ..CostOptions::default()
        },
        cast: CastConfig {
            default_cast: Cast {
                gcd: row.gcd().max(channel),
                cast_time: 0,
                ..Cast::default()
            },
            cd: if cooldown > 0 {
                Cooldown {
                    timer: Some(cd_timer.unwrap_or_else(|| sim.new_timer(unit))),
                    duration: cooldown,
                }
            } else {
                Cooldown::default()
            },
            ..CastConfig::default()
        },
        damage_multiplier: 1.0,
        threat_multiplier: 1.0,
        bonus_coefficient: heal.direct.map_or(0.0, |(_, _, coefficient)| coefficient),
        hot: DotConfig {
            aura: AuraConfig {
                label: format!("{} {}", row.name, row.id),
                ..AuraConfig::default()
            },
            tick_length: heal.period,
            number_of_ticks: heal.ticks,
            self_only: true,
            bonus_coefficient: heal.tick_coefficient,
            ..DotConfig::default()
        },
        ..SpellConfig::default()
    };
    let spell = sim.register_spell(unit, config);
    HealRank {
        spell,
        row,
        shape,
        direct: heal
            .direct
            .map(|(average, variance, _)| (average, variance)),
        tick: Some(heal.tick),
        tick_can_crit: heal.tick_can_crit,
        absorb: None,
        crit_absorb: None,
    }
}

/// A totem that heals the party while it stands (Healing Stream Totem): the totem's row states
/// the cost, the cast and how long it stands, the heal's row the amount, period and coefficient
/// of each tick. The spell carries both rows' client flags, so a talent naming either (a totem's
/// cost, the heal's ticks) reaches it.
pub(crate) fn register_totem_heal(
    sim: &mut Sim,
    unit: UnitId,
    totem: &'static Row,
    heal: &'static Row,
    class_mask: i64,
    shape: HealShape,
) -> HealRank {
    let periodic = heal
        .effects
        .iter()
        .find(|effect| effect.aura == super::dbcenums::A_PERIODIC_HEAL)
        .unwrap_or_else(|| super::spelldata::nil_effect());
    let tick = periodic.average(CHARACTER_LEVEL);
    let mana = totem.mana_cost();
    let mut flags = totem.class_flags;
    for (word, other) in flags.mask.iter_mut().zip(heal.class_flags.mask) {
        *word |= other;
    }
    let config = SpellConfig {
        action_id: ActionId {
            spell_id: totem.id,
            ..ActionId::default()
        },
        spell_school: totem.spell_school(),
        defense_type: totem.defense_type_core(),
        proc_mask: ProcMask::SPELL_HEALING,
        flags: SpellFlag::APL | SpellFlag::HELPFUL,
        class_spell_mask: class_mask,
        class_flags: flags,
        rank: totem.rank_number(),
        cost: CostOptions {
            mana_base_cost_percent: mana.base_cost_percent,
            mana_flat_cost: mana.flat_cost,
            ..CostOptions::default()
        },
        cast: CastConfig {
            default_cast: Cast {
                gcd: totem.gcd(),
                cast_time: totem.cast_time(),
                ..Cast::default()
            },
            ..CastConfig::default()
        },
        damage_multiplier: 1.0,
        threat_multiplier: 1.0,
        hot: DotConfig {
            aura: AuraConfig {
                label: format!("{} {}", totem.name, totem.id),
                ..AuraConfig::default()
            },
            tick_length: periodic.period(),
            number_of_ticks: ticks(totem.duration(), periodic.period()),
            self_only: true,
            bonus_coefficient: periodic.coeff(),
            ..DotConfig::default()
        },
        ..SpellConfig::default()
    };
    let spell = sim.register_spell(unit, config);
    HealRank {
        spell,
        row: totem,
        shape,
        direct: None,
        tick: Some(tick),
        tick_can_crit: heal.periodic_can_crit(),
        absorb: None,
        crit_absorb: None,
    }
}

/// The ticks of a duration at a period, at least one.
fn ticks(duration: Duration, period: Duration) -> i32 {
    if period <= 0 {
        return 1;
    }
    ((duration as f64 / period as f64).round() as i32).max(1)
}

/// A heal talent's client spell modifier at its rank, as a static mod on the spells the effect's
/// own client mask names: a percent on the direct heal (`SPELLMOD_DAMAGE`), on the heal over
/// time (`SPELLMOD_DOT`), on the shield (`SPELLMOD_EFFECT1`), on the cost, the cast time or the
/// cooldown, or a flat change to the cast time, the global cooldown, the crit chance, the cost
/// or the cooldown. A longer duration is ticks on a heal over time, which the class adds itself
/// at each spell's period. The direct and periodic percents are kept
/// apart, as the client keeps them: Gift of Nature names Regrowth in both, once for each part.
/// Only helpful spells take it. Returns false for a modifier this does not apply.
pub(crate) fn heal_talent_mod(sim: &mut Sim, unit: UnitId, effect: &Effect) -> bool {
    // Heals only: a class's damage spells keep the mods its own code gives them, so a talent
    // that names both is not applied to a damage spell twice.
    let base = SpellModConfig {
        class_flags: effect.class_flags,
        spell_flag: SpellFlag::HELPFUL,
        ..SpellModConfig::default()
    };
    let config = match (effect.aura, effect.misc) {
        (A_ADD_PCT_MODIFIER, SPELLMOD_DAMAGE) => SpellModConfig {
            kind: SpellModType::DamageDoneFlat,
            float_value: effect.percent(),
            ..base
        },
        (A_ADD_PCT_MODIFIER, SPELLMOD_DOT) => SpellModConfig {
            kind: SpellModType::DotDamageDonePct,
            float_value: effect.percent(),
            ..base
        },
        // The first effect's amount: a shield's absorb (Improved Power Word: Shield).
        (A_ADD_PCT_MODIFIER, SPELLMOD_EFFECT1) => SpellModConfig {
            kind: SpellModType::DamageDoneFlat,
            float_value: effect.percent(),
            ..base
        },
        (A_ADD_PCT_MODIFIER, SPELLMOD_COST) => SpellModConfig {
            kind: SpellModType::PowerCostPctAdd,
            float_value: effect.percent(),
            ..base
        },
        (A_ADD_PCT_MODIFIER, SPELLMOD_COOLDOWN) => SpellModConfig {
            kind: SpellModType::CooldownMultiplier,
            float_value: 1.0 + effect.percent(),
            ..base
        },
        (A_ADD_PCT_MODIFIER, SPELLMOD_CASTING_TIME) => SpellModConfig {
            kind: SpellModType::CastTimePct,
            float_value: effect.percent(),
            ..base
        },
        (A_ADD_FLAT_MODIFIER, SPELLMOD_CASTING_TIME) => SpellModConfig {
            kind: SpellModType::CastTimeFlat,
            time_value: effect.time_value(),
            ..base
        },
        (A_ADD_FLAT_MODIFIER, SPELLMOD_GLOBAL_COOLDOWN) => SpellModConfig {
            kind: SpellModType::GlobalCooldownFlat,
            time_value: effect.time_value(),
            ..base
        },
        (A_ADD_FLAT_MODIFIER, SPELLMOD_CRITICAL_CHANCE) => SpellModConfig {
            kind: SpellModType::BonusCritPercent,
            float_value: effect.base_points,
            ..base
        },
        (A_ADD_FLAT_MODIFIER, SPELLMOD_COST) => SpellModConfig {
            kind: SpellModType::PowerCostFlat,
            int_value: effect.base_points as i32,
            ..base
        },
        (A_ADD_FLAT_MODIFIER, SPELLMOD_COOLDOWN) => SpellModConfig {
            kind: SpellModType::CooldownFlat,
            time_value: effect.time_value(),
            ..base
        },
        _ => return false,
    };
    sim.add_static_mod(unit, config);
    true
}

/// A talent's rank in the talents message: a one-point talent is a bool there.
pub(crate) fn talent_rank(talents: &Message, name: &str) -> i32 {
    match talents.get(name) {
        Some(RequestValue::Bool(taken)) => i32::from(*taken),
        _ => talents.i32(name),
    }
}

/// Every client spell modifier a talent's row states at `rank`, on the heals its masks name
/// (see [`heal_talent_mod`]). Rank 0 is untaken.
pub(crate) fn apply_heal_talent(
    sim: &mut Sim,
    unit: UnitId,
    spell_id: i32,
    max_ranks: i32,
    rank: i32,
) {
    if rank <= 0 {
        return;
    }
    let row = Ladder::talent(spell_id, max_ranks).rank(rank);
    for effect in &row.effects {
        heal_talent_mod(sim, unit, effect);
    }
}

/// The `heals` effect: every registered rank, and the player's healing modifiers at reset with
/// its bonus healing taken.
pub(crate) fn heals_effect(env: &Environment, ranks: &[HealRank]) -> Value {
    let sim = &env.sim;
    let pseudo = &sim.unit(env.player).pseudo_stats;
    let heals: Vec<Value> = ranks
        .iter()
        .map(|rank| {
            let mut heal = json!({
                "spell_id": rank.row.id,
                "targets": rank.shape.targets,
                "jump_multiplier": rank.shape.jump_multiplier,
            });
            if let Some((average, variance)) = rank.direct {
                heal["direct"] = json!({"average": average, "variance": variance});
            }
            if let Some(tick) = rank.tick {
                heal["tick"] = json!(tick);
                // Forever's heal ticks crit where the row says so (README, Rust-only
                // mechanics); the reference's never do.
                if rank.tick_can_crit && env.rules == Rules::Forever {
                    heal["tick_can_crit"] = json!(true);
                }
            }
            if let Some(absorb) = rank.absorb {
                heal["absorb"] = json!(absorb);
            }
            if let (Some(absorb), Rules::Forever) = (rank.crit_absorb, env.rules) {
                heal["crit_absorb"] = json!(absorb);
            }
            // Forever's party damage decides how far these heals reach (README, Rust-only
            // mechanics); the reference heals every target at once.
            if let (Some(smart), Rules::Forever) = (rank.shape.smart, env.rules) {
                heal["smart"] = json!(smart);
            }
            // A healer's gear under Forever's rules (README, Rust-only mechanics).
            super::healer_gear::heal_shape(env, rank.row, &mut heal);
            heal
        })
        .collect();
    let smart = env.rules == Rules::Forever && heals.iter().any(|heal| heal.get("smart").is_some());
    let mut effect = json!({
        "kind": "heals",
        "heals": heals,
        "player": {
            "healing_dealt_multiplier": pseudo.healing_dealt_multiplier,
            "periodic_healing_dealt_multiplier": pseudo.periodic_healing_dealt_multiplier,
            "healing_taken_multiplier": pseudo.healing_taken_multiplier,
            "table_healing_dealt_multiplier": env.attack_table(env.player, env.player).healing_dealt_multiplier,
            "healing_power": sim.stat(env.player, Stat::HealingPower) + pseudo.bonus_healing_taken,
        },
        "bonus_healing_taken": pseudo.bonus_healing_taken,
    });
    if smart {
        effect["party"] = super::party_damage::export();
    }
    effect
}
