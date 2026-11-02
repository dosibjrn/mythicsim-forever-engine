//! Go sim/priest/talents_discipline.go.

use std::rc::Rc;

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::character::{cooldown_type, MajorCooldown};
use crate::prepare::dbcenums;
use crate::prepare::sim::{AuraConfig, Cooldown, EventCallbacks, Sim, UnitId, UnitType};
use crate::prepare::spell::{school, Cast, CastConfig, CostOptions, SpellConfig, SpellFlag};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::stats::Stat;

use super::super::masks;
use super::super::spell_data::spell_data;
use super::super::spells::longest_cooldown;
use super::super::Priest;

impl Priest {
    /// Go `registerDisciplineTalents`.
    pub(super) fn register_discipline_talents(&mut self, sim: &mut Sim, unit: UnitId) {
        // Tier 1
        self.apply_power_in_light(sim);
        // Wand Specialization models nothing in Go either.
        self.apply_twin_disciplines(sim, unit);

        // Tier 2
        self.apply_silent_resolve(sim, unit);
        self.apply_holy_precision(sim, unit);
        // Improved Power Word: Shield and Martyrdom model nothing in Go either.

        // Tier 3
        self.apply_mental_agility(sim, unit);
        self.apply_inner_focus(sim, unit);
        self.apply_meditation(sim, unit);

        // Tier 4
        // Improved Inner Fire models nothing in Go either.
        self.apply_mental_strength(sim, unit);
        // Soul Warding and Improved Mana Burn model nothing in Go either.

        // Tier 5
        self.apply_penance(sim, unit);
        // Renewed Hope models nothing in Go either; the healer's is in `heals.rs`.

        // Tier 6: Divine Aegis models nothing in Go either; the healer's is in `heals.rs`.

        // Tier 7
        self.apply_power_infusion(sim, unit);
    }

    /// Power in Light is new in Forever: Smite and Penance hit 2% harder per point while this
    /// priest's Holy Fire is burning the target. Each target gets a dynamic damage taken
    /// modifier for it.
    fn apply_power_in_light(&self, sim: &mut Sim) {
        if self.talents.i32("power_in_light") == 0 {
            return;
        }
        for target in sim.env_units.clone() {
            if sim.unit(target).unit_type == UnitType::Enemy {
                sim.add_dynamic_damage_taken_modifier(target);
            }
        }
    }

    /// Twin Disciplines is new in Forever: +1% damage and healing per point on instant spells.
    fn apply_twin_disciplines(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("twin_disciplines");
        if points == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::HOLY_NOVA
                    | masks::CHASTISE
                    | masks::DIVINE_GRACE
                    | masks::CONTINGENCY_PLAN
                    | masks::SHADOW_WORD_PAIN
                    | masks::DEVOURING_PLAGUE
                    // The 16 October 2026 beta notes add these beyond what 1225132's masks name.
                    | masks::SHADOW_WORD_DEATH
                    | masks::MIND_FLAY
                    | masks::STARSHARDS
                    | masks::PENANCE,
                float_value: spell_data()
                    .twin_disciplines
                    .effect_at(1)
                    .fraction_at(points),
                kind: SpellModType::DamageDoneFlat,
                ..SpellModConfig::default()
            },
        );
    }

    fn apply_silent_resolve(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("silent_resolve");
        if points == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            // Client 70425 widened the threat cut from Holy (school mask 2) to Holy and Shadow (34).
            SpellModConfig {
                class_mask: masks::ALL,
                school: school::HOLY | school::SHADOW,
                float_value: spell_data()
                    .silent_resolve
                    .effect(dbcenums::A_MOD_THREAT, 34)
                    .fraction_at(points),
                kind: SpellModType::ThreatMultiplierPct,
                ..SpellModConfig::default()
            },
        );
    }

    /// Holy Precision is new in Forever: +6% hit per point. 1309957 is a miss chance mod on a
    /// class mask, not school hit: Smite, Holy Fire, Holy Nova and the Penance bolts are in,
    /// Chastise is not. The mask also catches Vampiric Embrace and Shadowfiend, which never roll
    /// hit here.
    fn apply_holy_precision(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("holy_precision");
        if points == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::HOLY_SPELLS,
                kind: SpellModType::BonusHitPercent,
                float_value: spell_data()
                    .holy_precision
                    .effect(
                        dbcenums::A_ADD_FLAT_MODIFIER,
                        dbcenums::SPELLMOD_RESIST_MISS_CHANCE,
                    )
                    .value_at(points),
                ..SpellModConfig::default()
            },
        );
    }

    /// Instant spells, and the two cast-time spells the Forever tooltip adds: Smite and Holy
    /// Fire.
    fn apply_mental_agility(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("mental_agility");
        if points == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::SMITE
                    | masks::HOLY_FIRE
                    | masks::HOLY_NOVA
                    | masks::SHADOW_WORD_PAIN
                    | masks::DEVOURING_PLAGUE
                    | masks::VAMPIRIC_EMBRACE
                    | masks::POWER_INFUSION
                    | masks::SHADOWFORM
                    | masks::FADE
                    | masks::CHASTISE
                    | masks::CONFOUNDING_FLASH
                    | masks::CONTINGENCY_PLAN,
                float_value: spell_data().mental_agility.fraction_at(points),
                kind: SpellModType::PowerCostPctAdd,
                ..SpellModConfig::default()
            },
        );
    }

    fn apply_inner_focus(&mut self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("inner_focus") {
            return;
        }

        // The cost cut covers every priest spell. The crit is the client's own list: Devouring
        // Plague and Shadow Word: Pain are off it, and the channels Mind Flay and Starshards,
        // which do not count as periodic, are on it. Shadow Word: Death was never on it.
        let rank = spell_data().inner_focus.highest();
        let crit_mod = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                class_mask: masks::ALL
                    & !(masks::SHADOW_WORD_DEATH
                        | masks::DEVOURING_PLAGUE
                        | masks::SHADOW_WORD_PAIN),
                float_value: rank
                    .effect(
                        dbcenums::A_ADD_FLAT_MODIFIER,
                        dbcenums::SPELLMOD_CRITICAL_CHANCE,
                    )
                    .average(CHARACTER_LEVEL),
                kind: SpellModType::BonusCritPercent,
                ..SpellModConfig::default()
            },
        );

        let cost_percent = rank
            .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_COST)
            .average(CHARACTER_LEVEL) as i32;

        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Inner Focus".to_string(),
                action_id: Some(ActionId::spell(rank.id)),
                duration: crate::prepare::sim::NEVER_EXPIRES,
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.unit_mut(unit).pseudo_stats.spell_cost_percent_modifier += cost_percent;
                    sim.activate_spell_mod(crit_mod);
                })),
                // The expiry also starts the spell's cooldown, which only a fight times.
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.unit_mut(unit).pseudo_stats.spell_cost_percent_modifier -= cost_percent;
                    sim.deactivate_spell_mod(crit_mod);
                })),
                // The buff is spent by the next priest spell, whatever it is.
                events: EventCallbacks {
                    on_cast_complete: true,
                    ..EventCallbacks::default()
                },
                ..AuraConfig::default()
            },
        );
        self.inner_focus_aura = Some(aura);

        let timer = sim.new_timer(unit);
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id: ActionId::spell(rank.id),
                flags: SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::APL,
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
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
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

    /// 17/33/50, not the 17/34/51 that multiplying rank 1 would give: the client states each
    /// rank.
    fn apply_meditation(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("meditation");
        if points == 0 {
            return;
        }
        sim.unit_mut(unit).pseudo_stats.spirit_regen_rate_casting +=
            spell_data().meditation.fraction_at(points);
    }

    /// +3% Intellect per point in the Forever client, where TBC's raised total mana.
    fn apply_mental_strength(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("mental_strength");
        if points == 0 {
            return;
        }
        sim.unit_mut(unit).sdm.multiply_stat(
            Stat::Intellect,
            spell_data().mental_strength.multiplier_at(points),
        );
    }

    fn apply_penance(&mut self, sim: &mut Sim, unit: UnitId) {
        // A healer's Penance heals (heals.rs).
        if !self.talents.bool("penance") || self.healer {
            return;
        }
        spell_data().penance.each(|_, rank| {
            Self::register_penance_spell(sim, unit, rank);
        });
    }

    /// The priest casts Power Infusion on itself, which is the reason the Smite build goes
    /// thirty-one points deep.
    fn apply_power_infusion(&self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("power_infusion") {
            return;
        }

        let rank = spell_data().power_infusion.highest();
        let aura = crate::prepare::buffs::generated::POWER_INFUSIONS.aura(sim, unit, true, 0, 0.0);

        let mana = rank.mana_cost();
        let timer = sim.new_timer(unit);
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id: ActionId {
                    spell_id: rank.id,
                    tag: sim.unit(unit).index,
                    ..ActionId::default()
                },
                spell_school: school::HOLY,
                flags: SpellFlag::HELPFUL | SpellFlag::APL,
                class_spell_mask: masks::POWER_INFUSION,
                cost: CostOptions {
                    mana_base_cost_percent: mana.base_cost_percent,
                    mana_flat_cost: mana.flat_cost,
                    ..CostOptions::default()
                },
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
}
