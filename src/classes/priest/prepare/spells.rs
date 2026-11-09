//! The Priest's spell registrations: Go `sim/priest` `Initialize` and the `register*Spell`
//! functions it calls, in Go's order. A closure Go gives a spell config (`ApplyEffects`,
//! `OnTick`, `OnSnapshot` and the like) only runs in a fight, so what preparation keeps of it is
//! the field that says it is there: `has_extra_cast_condition`, the related buff.

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::character::{cooldown_type, MajorCooldown};
use crate::prepare::resolve_aura::dot_config;
use crate::prepare::resolve_spell::{cast_requirement, flags, spell_config};
use crate::prepare::sim::{AuraConfig, Cooldown, Duration, Sim, SpellId, TimerId, UnitId, SECOND};
use crate::prepare::spell::{
    school, Cast, CastConfig, CostOptions, DefenseType, DotConfig, ProcMask, SpellConfig,
    SpellFlag, GCD_DEFAULT,
};
use crate::prepare::spelldata::Spell as Row;

use super::masks;
use super::spell_data::spell_data;
use super::Priest;

/// Go `PenanceTicks`.
const PENANCE_TICKS: i32 = 3;

fn spell_action(id: i32) -> ActionId {
    ActionId {
        spell_id: id,
        ..ActionId::default()
    }
}

/// Use the pinned native client row when registering Priest spells for a
/// lower-level actor. The established level-60 spellbook is unchanged.
pub(super) fn rank_available_at_level(rank: &Row, level: i32) -> bool {
    level == 60 || (rank.spell_level > 0 && i32::from(rank.spell_level) <= level)
}

/// Go `ManaCost: core.ManaCostOptions{FlatCost: int32(rank.Cost())}`.
pub(super) fn flat_cost(row: &Row) -> CostOptions {
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

/// `max(rank.Cooldown(), rank.CategoryCooldown())`.
pub(super) fn longest_cooldown(row: &Row) -> Duration {
    row.cooldown().max(row.category_cooldown())
}

/// The cooldown a ladder's ranks share: `core.Cooldown{Timer: timer, Duration: ...}`.
fn shared_cooldown(timer: TimerId, row: &Row) -> Cooldown {
    Cooldown {
        timer: Some(timer),
        duration: longest_cooldown(row),
    }
}

/// The fields every rank of a Priest damage spell states: the row's school, defense type and
/// range, the spell damage proc mask and the multipliers.
fn rank_config(rank: &Row, class_spell_mask: i64, flags: SpellFlag) -> SpellConfig {
    SpellConfig {
        action_id: spell_action(rank.id),
        spell_school: rank.spell_school(),
        defense_type: rank.defense_type_core(),
        proc_mask: ProcMask::SPELL_DAMAGE,
        flags,
        class_spell_mask,
        rank: rank.rank_number(),
        max_range: f64::from(rank.max_range),
        cost: flat_cost(rank),
        damage_multiplier: 1.0,
        threat_multiplier: 1.0,
        ..SpellConfig::default()
    }
}

/// The dot a Priest damage-over-time rank carries: `Dot: core.DotConfig{Aura: {Label: label},
/// NumberOfTicks: int32(rank.Duration() / tickLength), TickLength: tickLength,
/// BonusCoefficient: tick.Coeff()}`.
fn rank_dot(rank: &Row, label: String) -> DotConfig {
    let tick = rank.periodic_effect();
    let tick_length = tick.period();
    DotConfig {
        aura: AuraConfig {
            label,
            ..AuraConfig::default()
        },
        number_of_ticks: (rank.duration() / tick_length) as i32,
        tick_length,
        affected_by_cast_speed: false,
        bonus_coefficient: tick.coeff(),
        ..DotConfig::default()
    }
}

impl Priest {
    /// Go `Priest.Initialize`.
    pub(super) fn register_spells(&mut self, sim: &mut Sim, unit: UnitId) {
        let data = spell_data();
        let level = sim.unit(unit).level;
        let mind_blast_cd = sim.new_timer(unit);
        let shadow_word_death_cd = sim.new_timer(unit);

        data.mind_blast.each(|_, rank| {
            if rank_available_at_level(rank, level) {
                Self::register_mind_blast_spell(sim, unit, rank, mind_blast_cd);
            }
        });
        let mut shadow_word_pain = Vec::new();
        data.shadow_word_pain.each(|_, rank| {
            if rank_available_at_level(rank, level) {
                shadow_word_pain.push(Self::register_shadow_word_pain_spell(sim, unit, rank));
            }
        });
        self.shadow_word_pain.extend(shadow_word_pain);
        data.shadow_word_death.each(|_, rank| {
            if rank_available_at_level(rank, level) {
                Self::register_shadow_word_death_spell(sim, unit, rank, shadow_word_death_cd);
            }
        });
        data.smite.each(|_, rank| {
            if rank_available_at_level(rank, level) {
                Self::register_smite_spell(sim, unit, rank);
            }
        });
        let mut holy_fire = Vec::new();
        data.holy_fire.each(|_, rank| {
            if rank_available_at_level(rank, level) {
                holy_fire.push(Self::register_holy_fire_spell(sim, unit, rank));
            }
        });
        self.holy_fire.extend(holy_fire);
        data.chastise.each(|_, rank| {
            if rank_available_at_level(rank, level) {
                Self::register_chastise_spell(sim, unit, rank);
            }
        });
        self.register_shadowfiend_spell(sim, unit);

        // Dark Sacrifice is the undead priest's race ability.
        let character = sim.character(unit);
        let race = character.race.clone();
        let racials_disabled = character.disable_racials;
        if race == "RaceUndead" && !racials_disabled {
            Self::register_dark_sacrifice_spell(sim, unit);
        }

        if race == "RaceNightElf" && !racials_disabled {
            let starshards_cd = sim.new_timer(unit);
            data.starshards.each(|_, rank| {
                if rank_available_at_level(rank, level) {
                    Self::register_starshards_spell(sim, unit, rank, starshards_cd);
                }
            });
        }

        // Devouring Plague is baseline here: the Forever client teaches it to every race.
        let devouring_plague_cd = sim.new_timer(unit);
        data.devouring_plague.each(|_, rank| {
            if rank_available_at_level(rank, level) {
                Self::register_devouring_plague_spell(sim, unit, rank, devouring_plague_cd);
            }
        });
    }

    fn register_mind_blast_spell(sim: &mut Sim, unit: UnitId, rank: &Row, cd_timer: TimerId) {
        sim.register_spell(
            unit,
            SpellConfig {
                cast: CastConfig {
                    cd: shared_cooldown(cd_timer, rank),
                    ..default_cast(rank.gcd(), rank.cast_time())
                },
                bonus_coefficient: rank.damage_effect().coeff(),
                ..rank_config(rank, masks::MIND_BLAST, SpellFlag::APL)
            },
        );
    }

    fn register_shadow_word_pain_spell(sim: &mut Sim, unit: UnitId, rank: &Row) -> SpellId {
        sim.register_spell(
            unit,
            SpellConfig {
                cast: default_cast(rank.gcd(), 0),
                dot: rank_dot(rank, format!("ShadowWordPain-{}", rank.rank_number())),
                ..rank_config(rank, masks::SHADOW_WORD_PAIN, SpellFlag::APL)
            },
        )
    }

    fn register_shadow_word_death_spell(
        sim: &mut Sim,
        unit: UnitId,
        rank: &Row,
        cd_timer: TimerId,
    ) {
        sim.register_spell(
            unit,
            SpellConfig {
                cast: CastConfig {
                    // The 16 October 2026 beta notes raise the cooldown to 18 sec; the client row
                    // still states 15.
                    cd: Cooldown {
                        timer: Some(cd_timer),
                        duration: 18 * SECOND,
                    },
                    ..default_cast(rank.gcd(), 0)
                },
                bonus_coefficient: rank.damage_effect().coeff(),
                ..rank_config(rank, masks::SHADOW_WORD_DEATH, SpellFlag::APL)
            },
        );
    }

    /// Every rank is registered: the Smite rotation drops to rank 7 when mana runs short.
    fn register_smite_spell(sim: &mut Sim, unit: UnitId, rank: &Row) {
        sim.register_spell(
            unit,
            SpellConfig {
                cast: default_cast(rank.gcd(), rank.cast_time()),
                bonus_coefficient: rank.damage_effect().coeff(),
                ..rank_config(rank, masks::SMITE, SpellFlag::APL)
            },
        );
    }

    fn register_holy_fire_spell(
        sim: &mut Sim,
        unit: UnitId,
        rank: &Row,
    ) -> crate::prepare::sim::SpellId {
        sim.register_spell(
            unit,
            SpellConfig {
                cast: default_cast(rank.gcd(), rank.cast_time()),
                bonus_coefficient: rank.damage_effect().coeff(),
                dot: rank_dot(rank, format!("HolyFire-{}", rank.rank_number())),
                ..rank_config(rank, masks::HOLY_FIRE, SpellFlag::APL)
            },
        )
    }

    /// Chastise is new in Forever: a Holy nuke the ranks share a cooldown on, from the client
    /// rows. Only a humanoid target can be cast on.
    fn register_chastise_spell(sim: &mut Sim, unit: UnitId, rank: &'static Row) {
        let mut config = spell_config(
            sim,
            unit,
            rank,
            &[crate::prepare::resolve_spell::magic(ProcMask::SPELL_DAMAGE)],
        );
        config.class_spell_mask = masks::CHASTISE;
        config.has_extra_cast_condition = true;
        sim.register_spell(unit, config);
    }

    /// The summon, only when the class options ask for it.
    fn register_shadowfiend_spell(&mut self, sim: &mut Sim, unit: UnitId) {
        if !self.use_shadowfiend {
            return;
        }
        let rank = spell_data().shadowfiend.highest();
        if !rank_available_at_level(rank, sim.unit(unit).level) {
            return;
        }
        let action = spell_action(rank.id);

        // Timeline aura, and what the tier 4 two piece lengthens.
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Shadowfiend".to_string(),
                action_id: Some(action.clone()),
                duration: rank.duration(),
                ..AuraConfig::default()
            },
        );
        self.shadowfiend_aura = Some(aura);

        // Client 401977 has no power cost row: the summon is free.
        let timer = sim.new_timer(unit);
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id: action,
                spell_school: school::SHADOW,
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::APL,
                class_spell_mask: masks::SHADOWFIEND,
                cast: CastConfig {
                    cd: shared_cooldown(timer, rank),
                    ..default_cast(GCD_DEFAULT, 0)
                },
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
        self.shadowfiend = Some(spell);
        sim.add_major_cooldown(
            unit,
            MajorCooldown {
                spell,
                priority: 0,
                cooldown_type: cooldown_type::MANA,
                allow_spell_queueing: false,
                timings: Vec::new(),
            },
        );
    }

    /// Dark Sacrifice is new in Forever: the undead priest's race ability, a free instant self
    /// buff that turns health into mana over its ticks.
    fn register_dark_sacrifice_spell(sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().dark_sacrifice.highest();
        if !rank_available_at_level(rank, sim.unit(unit).level) {
            return;
        }
        let effect = rank.proc_energize_effect();

        let mut config = spell_config(sim, unit, rank, &[flags(SpellFlag::APL)]);
        config.proc_mask = ProcMask::EMPTY;
        config.hot = dot_config(rank, effect, &[]);
        config.hot.self_only = true;
        let spell = sim.register_spell(unit, config);
        sim.add_major_cooldown(
            unit,
            MajorCooldown {
                spell,
                priority: 0,
                cooldown_type: cooldown_type::MANA,
                allow_spell_queueing: false,
                timings: Vec::new(),
            },
        );
    }

    /// Starshards is the Night Elf racial: an Arcane channel, free, on a 30 second cooldown.
    fn register_starshards_spell(sim: &mut Sim, unit: UnitId, rank: &Row, cd_timer: TimerId) {
        sim.register_spell(
            unit,
            SpellConfig {
                cast: CastConfig {
                    cd: shared_cooldown(cd_timer, rank),
                    ..default_cast(rank.gcd(), 0)
                },
                dot: rank_dot(rank, format!("Starshards-{}", rank.rank_number())),
                ..rank_config(
                    rank,
                    masks::STARSHARDS,
                    SpellFlag::APL | SpellFlag::CHANNELED,
                )
            },
        );
    }

    /// Devouring Plague: Shadow Word: Pain's shape; each tick heals the priest for its damage.
    fn register_devouring_plague_spell(sim: &mut Sim, unit: UnitId, rank: &Row, cd_timer: TimerId) {
        sim.register_spell(
            unit,
            SpellConfig {
                cast: CastConfig {
                    // The 16 October 2026 beta notes cut the cooldown to 24 sec; the client row still
                    // states a 60 sec category cooldown.
                    cd: Cooldown {
                        timer: Some(cd_timer),
                        duration: 24 * SECOND,
                    },
                    ..default_cast(rank.gcd(), 0)
                },
                dot: rank_dot(rank, format!("DevouringPlague-{}", rank.rank_number())),
                ..rank_config(rank, masks::DEVOURING_PLAGUE, SpellFlag::APL)
            },
        );
    }

    /// Penance: three Holy bolts over the channel, on a 12 second cooldown the ranks share
    /// (category 2414). Every rank is registered. One bolt lands with the cast and two are
    /// channel ticks.
    pub(super) fn register_penance_spell(sim: &mut Sim, unit: UnitId, rank: &Row) {
        // The cast's tooltip names its damage bolt first, then the heal bolt.
        let bolt = rank.refs()[0];

        let timer = sim.category_timer(unit, i32::from(rank.category));
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: school::HOLY,
                defense_type: DefenseType::Magic,
                proc_mask: ProcMask::SPELL_DAMAGE,
                flags: SpellFlag::APL | SpellFlag::CHANNELED,
                class_spell_mask: masks::PENANCE,
                rank: rank.rank_number(),
                max_range: f64::from(rank.max_range),
                cost: flat_cost(rank),
                cast: CastConfig {
                    cd: shared_cooldown(timer, rank),
                    ..default_cast(rank.gcd(), 0)
                },
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                dot: DotConfig {
                    aura: AuraConfig {
                        label: format!("Penance-{}", rank.rank_number()),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: PENANCE_TICKS - 1,
                    tick_length: SECOND,
                    affected_by_cast_speed: false,
                    bonus_coefficient: bolt.damage_effect().coeff(),
                    ..DotConfig::default()
                },
                ..SpellConfig::default()
            },
        );
    }

    /// Holy Nova: damage to everything in range and a heal on the priest's own party, both at
    /// the same coefficient. The heal is registered first.
    pub(super) fn register_holy_nova_spell(sim: &mut Sim, unit: UnitId, rank: &Row) {
        let heal = spell_data().holy_nova_triggered.rank(rank.rank_number());

        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(heal.id),
                spell_school: school::HOLY,
                defense_type: heal.defense_type_core(), // a crit needs it for its multiplier
                proc_mask: ProcMask::SPELL_HEALING,
                flags: SpellFlag::HELPFUL
                    | SpellFlag::NO_ON_CAST_COMPLETE
                    | SpellFlag::PASSIVE_SPELL,
                damage_multiplier: 1.0,
                threat_multiplier: 0.0,
                bonus_coefficient: rank.damage_effect().coeff(),
                ..SpellConfig::default()
            },
        );

        sim.register_spell(
            unit,
            SpellConfig {
                cast: default_cast(rank.gcd(), 0),
                threat_multiplier: 0.0,
                bonus_coefficient: rank.damage_effect().coeff(),
                // Not in Shadowform.
                cast_requirement: cast_requirement(rank),
                ..rank_config(rank, masks::HOLY_NOVA, SpellFlag::APL)
            },
        );
    }

    /// A three tick channel. Forever's ticks are not hastened. Binary, as on master: the row
    /// slows, so it resists whole or not at all.
    pub(super) fn register_mind_flay_spell(sim: &mut Sim, unit: UnitId, rank: &Row) {
        sim.register_spell(
            unit,
            SpellConfig {
                cast: default_cast(rank.gcd(), 0),
                dot: rank_dot(rank, format!("MindFlay-{}", rank.rank_number())),
                ..rank_config(
                    rank,
                    masks::MIND_FLAY,
                    SpellFlag::APL | SpellFlag::CHANNELED | SpellFlag::BINARY,
                )
            },
        );
    }
}
