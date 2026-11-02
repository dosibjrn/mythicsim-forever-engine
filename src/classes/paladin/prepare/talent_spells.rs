//! The abilities a talent point buys: Go `talents.go` `registerTalentSpells`, `divine_favor.go`,
//! `holy_shock.go`, `lights_vigil.go`, `swift_judgement.go`, `templars_bulwark.go` and
//! `holy_shield.go`.

use crate::prepare::aura_helpers::{AbsorptionAuraConfig, CallbackMask, ProcTrigger};
use crate::prepare::character::{cooldown_type, MajorCooldown};
use crate::prepare::dbcenums;
use crate::prepare::sim::{AuraConfig, Cooldown, Sim, UnitId, NEVER_EXPIRES, SECOND};
use crate::prepare::spell::{
    school, Cast, CastConfig, DefenseType, ProcMask, SpellConfig, SpellFlag, GCD_DEFAULT,
};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::spelldata::Spell as Row;
use crate::prepare::stats::Stat;
use std::rc::Rc;

use super::hotfix_113554 as hotfix;
use super::masks;
use super::spell_data::spell_data;
use super::util::{
    aura_array_to_map, cooldown, gcd_cast, mana_cost, new_enemy_aura_array, shared_timer,
    spell_action, tagged_action,
};
use super::Paladin;

/// Go `holyShockRank`: one rank of Holy Shock, the spell the paladin casts, and the damage and
/// heal it triggers. The client ships the damage and heal chains as two ladders under one name,
/// which the generator refuses, so the table is by hand from the client rows.
pub(super) struct HolyShockRank {
    pub rank: i32,
    pub spell_id: i32,
    pub cost: i32,
    pub damage: [f64; 2],
    pub heal_id: i32,
    pub heal: f64,
}

/// Go `HolyShockRanks`.
pub(super) const HOLY_SHOCK_RANKS: [HolyShockRank; 4] = [
    HolyShockRank {
        rank: 1,
        spell_id: 1311606,
        cost: 160,
        damage: [128.0, 140.0],
        heal_id: 1311605,
        heal: 114.0,
    },
    HolyShockRank {
        rank: 2,
        spell_id: 20473,
        cost: 225,
        damage: [175.0, 189.0],
        heal_id: 25914,
        heal: 156.0,
    },
    HolyShockRank {
        rank: 3,
        spell_id: 20929,
        cost: 275,
        damage: [248.0, 268.0],
        heal_id: 25913,
        heal: 230.0,
    },
    HolyShockRank {
        rank: 4,
        spell_id: 20930,
        cost: 325,
        damage: [334.0, 362.0],
        heal_id: 25903,
        heal: 320.0,
    },
];

/// Go `holyShockCoefficient`, `holyShockCooldown` and `holyShockRange`.
pub(super) const HOLY_SHOCK_COEFFICIENT: f64 = 0.429;
pub(super) const HOLY_SHOCK_COOLDOWN: i64 = 10 * SECOND;
const HOLY_SHOCK_RANGE: f64 = 20.0;

/// The vigil aura, party heal and enemy strike each Light's Vigil rank triggers. They sit in
/// LightsVigilTriggered in the order the client lists them, not by rank, so the pairing is by
/// hand: `(rank, aura, heal, strike)`.
pub(super) const LIGHTS_VIGIL_TRIGGERED: [(i32, i32, i32, i32); 3] = [
    (1, 1310909, 1310912, 1310914),
    (2, 1311593, 1311591, 1311592),
    (3, 1311597, 1311596, 1311598),
];

impl Paladin {
    /// Go `registerTalentSpells`.
    pub(super) fn register_talent_spells(&mut self, sim: &mut Sim, unit: UnitId) {
        // Holy
        if self.talents.bool("divine_favor") {
            self.register_divine_favor(sim, unit);
        }
        if self.talents.bool("holy_shock") {
            self.register_holy_shock(sim, unit);
        }
        if self.talents.bool("lights_vigil") {
            self.register_lights_vigil(sim, unit);
        }
        // Push 113554's Oath of Light: 0 points under the reference's rules.
        if self.new_talent("oath_of_light") > 0 {
            self.register_oath_of_light(sim, unit);
        }

        // Protection
        if self.talents.bool("swift_judgement") {
            self.register_swift_judgement(sim, unit);
        }
        if self.talents.bool("templars_bulwark") {
            self.register_templars_bulwark(sim, unit);
        }
        if self.talents.bool("holy_shield") {
            spell_data()
                .holy_shield
                .each(|_, rank| self.register_holy_shield(sim, unit, rank));
        }

        // Retribution
        if self.talents.bool("seal_of_command") {
            spell_data()
                .seal_of_command
                .each(|_, rank| self.register_seal_of_command(sim, unit, rank));
        }
    }

    /// Go `registerDivineFavor`: gives your next Flash of Light, Holy Light, or Holy Shock
    /// spell a 100% critical effect chance.
    fn register_divine_favor(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().divine_favor.highest();
        let action_id = spell_action(rank.id);
        let label = sim.unit(unit).label.clone();

        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: format!("Divine Favor{label}"),
                action_id: Some(action_id.clone()),
                duration: NEVER_EXPIRES,
                ..AuraConfig::default()
            },
        );
        sim.attach_spell_mod(
            aura,
            SpellModConfig {
                kind: SpellModType::BonusCritPercent,
                class_mask: masks::HEALING_SPELLS | masks::HOLY_SHOCK,
                float_value: rank
                    .effect(
                        dbcenums::A_ADD_FLAT_MODIFIER,
                        dbcenums::SPELLMOD_CRITICAL_CHANCE,
                    )
                    .average(crate::prepare::character::constants::CHARACTER_LEVEL),
                ..SpellModConfig::default()
            },
        );
        sim.attach_proc_trigger(
            aura,
            &ProcTrigger {
                // 20216 carries the bit.
                can_proc_from_procs: true,
                callback: CallbackMask::ON_CAST_COMPLETE,
                class_spell_mask: masks::HEALING_SPELLS | masks::HOLY_SHOCK,
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );

        let timer = sim.new_timer(unit);
        let divine_favor = sim.register_spell(
            unit,
            SpellConfig {
                action_id,
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                flags: SpellFlag::APL | SpellFlag::HELPFUL,
                class_spell_mask: masks::DIVINE_FAVOR,
                cost: mana_cost(rank),
                cast: CastConfig {
                    default_cast: Cast {
                        non_empty: true,
                        ..Cast::default()
                    },
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: cooldown(rank),
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
                spell: divine_favor,
                priority: 0,
                cooldown_type: cooldown_type::DPS,
                allow_spell_queueing: false,
                timings: Vec::new(),
            },
        );
    }

    /// Go `registerHolyShock`.
    fn register_holy_shock(&mut self, sim: &mut Sim, unit: UnitId) {
        for rank in &HOLY_SHOCK_RANKS {
            self.register_holy_shock_rank(sim, unit, rank);
        }
    }

    /// Go `registerHolyShockRank`: blasts the target with Holy energy, causing X Holy damage to
    /// an enemy, or Y healing to an ally.
    fn register_holy_shock_rank(&mut self, sim: &mut Sim, unit: UnitId, rank: &HolyShockRank) {
        let timer = shared_timer(sim, unit, &mut self.timers.holy_shock);

        let config = |action: i32, proc_mask: ProcMask, flags: SpellFlag, mask: i64| SpellConfig {
            action_id: spell_action(action),
            spell_school: school::HOLY,
            defense_type: DefenseType::Magic,
            proc_mask,
            flags,
            class_spell_mask: mask,
            rank: rank.rank,
            max_range: HOLY_SHOCK_RANGE,
            cost: crate::prepare::spell::CostOptions {
                mana_flat_cost: rank.cost,
                ..Default::default()
            },
            cast: gcd_cast(GCD_DEFAULT, 0, Some((timer, HOLY_SHOCK_COOLDOWN))),
            damage_multiplier: 1.0,
            threat_multiplier: 1.0,
            bonus_coefficient: HOLY_SHOCK_COEFFICIENT,
            ..SpellConfig::default()
        };
        sim.register_spell(
            unit,
            config(
                rank.spell_id,
                ProcMask::SPELL_DAMAGE,
                SpellFlag::APL,
                masks::HOLY_SHOCK,
            ),
        );
        sim.register_spell(
            unit,
            config(
                rank.heal_id,
                ProcMask::SPELL_HEALING,
                SpellFlag::APL | SpellFlag::HELPFUL,
                masks::HOLY_SHOCK_HEAL,
            ),
        );
    }

    /// Go `registerLightsVigil`: applies Light's Vigil to the target for 30 sec; the next Holy
    /// Shock on them triggers no cooldown.
    fn register_lights_vigil(&mut self, sim: &mut Sim, unit: UnitId) {
        let ally = self.healer && sim.rules == crate::prepare::sim::Rules::Forever;
        spell_data().lights_vigil.each(|_, rank| {
            if ally {
                self.register_lights_vigil_ally_rank(sim, unit, rank);
            } else {
                self.register_lights_vigil_rank(sim, unit, rank);
            }
        });
    }

    /// Light's Vigil on an ally (push 113554), in the Holy Paladin healer sim under Forever's
    /// rules: the cast puts the vigil on the player, who stands for the heal's target; its enemy
    /// strike and refund are gone. The party heal (the rank's 1310912, 1311591 or 1311596) is a
    /// spell of its own that the next Holy Shock heal on the player triggers.
    fn register_lights_vigil_ally_rank(&mut self, sim: &mut Sim, unit: UnitId, rank: &Row) {
        let (_, aura_id, heal_id, _) = LIGHTS_VIGIL_TRIGGERED
            .iter()
            .copied()
            .find(|(n, _, _, _)| *n == rank.rank_number())
            .unwrap_or((0, 0, 0, 0));
        let aura_rank = spell_data().lights_vigil_triggered.by_id(aura_id);
        let heal_rank = spell_data().lights_vigil_triggered.by_id(heal_id);
        let label = sim.unit(unit).label.clone();

        sim.register_aura(
            unit,
            AuraConfig {
                label: format!("Light's Vigil{label} Rank {}", rank.rank_number()),
                action_id: Some(spell_action(aura_rank.id)),
                duration: aura_rank.duration(),
                ..AuraConfig::default()
            },
        );
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(heal_rank.id),
                spell_school: heal_rank.spell_school(),
                defense_type: heal_rank.defense_type_core(),
                proc_mask: ProcMask::SPELL_HEALING,
                flags: SpellFlag::PASSIVE_SPELL | SpellFlag::HELPFUL,
                class_spell_mask: masks::LIGHTS_VIGIL_HEAL,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                bonus_coefficient: heal_rank.heal_effect().coeff(),
                ..SpellConfig::default()
            },
        );
        let timer = shared_timer(sim, unit, &mut self.timers.lights_vigil);
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::APL | SpellFlag::HELPFUL,
                class_spell_mask: masks::LIGHTS_VIGIL_ALLY,
                rank: rank.rank_number(),
                max_range: f64::from(rank.max_range),
                cost: mana_cost(rank),
                cast: gcd_cast(rank.gcd(), rank.cast_time(), Some((timer, cooldown(rank)))),
                ..SpellConfig::default()
            },
        );
    }

    fn register_lights_vigil_rank(&mut self, sim: &mut Sim, unit: UnitId, rank: &Row) {
        let (_, aura_id, _, strike_id) = LIGHTS_VIGIL_TRIGGERED
            .iter()
            .copied()
            .find(|(n, _, _, _)| *n == rank.rank_number())
            .unwrap_or((0, 0, 0, 0));
        let aura_rank = spell_data().lights_vigil_triggered.by_id(aura_id);
        let strike_rank = spell_data().lights_vigil_triggered.by_id(strike_id);
        let strike_damage = strike_rank.damage_effect();
        let label = sim.unit(unit).label.clone();

        let auras = new_enemy_aura_array(sim, |sim, target| {
            sim.get_or_register_aura(
                target,
                AuraConfig {
                    label: format!("Light's Vigil{label} Rank {}", rank.rank_number()),
                    action_id: Some(spell_action(aura_rank.id)),
                    duration: aura_rank.duration(),
                    ..AuraConfig::default()
                },
            )
        });

        let related_aura_arrays = aura_array_to_map(sim, &auras);
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(strike_rank.id),
                spell_school: strike_rank.spell_school(),
                defense_type: strike_rank.defense_type_core(),
                proc_mask: ProcMask::SPELL_DAMAGE,
                flags: SpellFlag::PASSIVE_SPELL,
                class_spell_mask: masks::LIGHTS_VIGIL_STRIKE,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                bonus_coefficient: strike_damage.coeff(),
                related_aura_arrays: related_aura_arrays.clone(),
                ..SpellConfig::default()
            },
        );

        let timer = shared_timer(sim, unit, &mut self.timers.lights_vigil);
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::APL,
                class_spell_mask: masks::LIGHTS_VIGIL,
                rank: rank.rank_number(),
                max_range: f64::from(rank.max_range),
                cost: mana_cost(rank),
                cast: gcd_cast(rank.gcd(), rank.cast_time(), Some((timer, cooldown(rank)))),
                related_aura_arrays,
                ..SpellConfig::default()
            },
        );
    }

    /// Go `registerSwiftJudgement`: finishes the remaining cooldown on your Judgement ability
    /// and reduces the Mana cost of your next Judgement by 100%.
    fn register_swift_judgement(&mut self, sim: &mut Sim, unit: UnitId) {
        if sim.rules == crate::prepare::sim::Rules::Forever {
            self.register_second_judgement(sim, unit);
            return;
        }
        let rank = spell_data().swift_judgement.highest();
        let action_id = spell_action(rank.id);
        let label = sim.unit(unit).label.clone();

        let free_judgement = sim.register_aura(
            unit,
            AuraConfig {
                label: format!("Swift Judgement{label}"),
                action_id: Some(action_id.clone()),
                duration: NEVER_EXPIRES,
                ..AuraConfig::default()
            },
        );
        sim.attach_spell_mod(
            free_judgement,
            SpellModConfig {
                kind: SpellModType::PowerCostPctAdd,
                class_mask: masks::JUDGEMENT,
                float_value: rank
                    .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_COST)
                    .percent(),
                ..SpellModConfig::default()
            },
        );
        sim.attach_proc_trigger(
            free_judgement,
            &ProcTrigger {
                callback: CallbackMask::ON_CAST_COMPLETE,
                class_spell_mask: masks::JUDGEMENT,
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );

        let timer = sim.new_timer(unit);
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id,
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::APL | SpellFlag::HELPFUL,
                class_spell_mask: masks::SWIFT_JUDGEMENT,
                cast: CastConfig {
                    default_cast: Cast {
                        non_empty: true,
                        ..Cast::default()
                    },
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: cooldown(rank),
                    },
                    ..CastConfig::default()
                },
                related_self_buff: Some(free_judgement),
                ..SpellConfig::default()
            },
        );

        // Our sim fires it as a cooldown, whenever there is a Judgement to finish and a seal
        // to spend the free cast on.
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

    /// Swift Judgement under Forever's rules since push 113554 (1310994, upstream #772's
    /// swift_judgement.go): unleash the energy of a Seal spell upon an enemy, without consuming
    /// the Seal, on a 60 sec cooldown of its own. It casts the active seal's judgement as
    /// Judgement does, free and off the global cooldown, and fires no on-cast callbacks, so
    /// listeners hear the judgement. Upstream leaves it to the rotation; MythicSim fires it as a
    /// major cooldown whenever a seal is up, as it fired the old talent.
    fn register_second_judgement(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().swift_judgement.highest();
        let timer = sim.new_timer(unit);
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: school::HOLY,
                defense_type: DefenseType::None,
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::APL | SpellFlag::PASSIVE_SPELL | SpellFlag::NO_ON_CAST_COMPLETE,
                class_spell_mask: masks::SWIFT_JUDGEMENT | masks::SECOND_JUDGEMENT,
                // 10 yards, push 113554's row.
                max_range: 10.0,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                cast: CastConfig {
                    default_cast: Cast {
                        non_empty: true,
                        ..Cast::default()
                    },
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: 60 * SECOND,
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
                priority: 0,
                cooldown_type: cooldown_type::DPS,
                allow_spell_queueing: false,
                timings: Vec::new(),
            },
        );
    }

    /// Go `registerTemplarsBulwark`: grants an absorb shield equal to 100% of your maximum
    /// health for 8 sec and applies Forbearance.
    fn register_templars_bulwark(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().templars_bulwark.highest();
        let action_id = spell_action(rank.id);
        let label = sim.unit(unit).label.clone();

        let shield = sim.new_damage_absorption_aura(
            unit,
            AbsorptionAuraConfig {
                aura: AuraConfig {
                    label: format!("Templar's Bulwark{label}"),
                    action_id: Some(action_id.clone()),
                    duration: rank.duration(),
                    ..AuraConfig::default()
                },
                shield_strength_calculator: Some(Rc::new(|_, _| 0.0)),
                ..AbsorptionAuraConfig::default()
            },
        );

        let timer = sim.new_timer(unit);
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id,
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::APL | SpellFlag::HELPFUL,
                class_spell_mask: masks::TEMPLARS_BULWARK,
                cost: mana_cost(rank),
                cast: CastConfig {
                    default_cast: Cast {
                        non_empty: true,
                        ..Cast::default()
                    },
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: cooldown(rank),
                    },
                    ..CastConfig::default()
                },
                has_extra_cast_condition: true,
                related_self_buff: Some(shield.aura),
                ..SpellConfig::default()
            },
        );

        sim.add_major_cooldown(
            unit,
            MajorCooldown {
                spell,
                priority: 0,
                cooldown_type: cooldown_type::SURVIVAL,
                allow_spell_queueing: false,
                timings: Vec::new(),
            },
        );
    }

    /// Go `registerHolyShield`: increases chance to block by 20% for 10 sec, and deals Holy
    /// damage for each attack blocked while active.
    fn register_holy_shield(&mut self, sim: &mut Sim, unit: UnitId, rank: &Row) {
        let action_id = spell_action(rank.id);
        // stats.BlockPercent is in percent: 30, not 0.3.
        let block_percent = rank.effect(dbcenums::A_MOD_BLOCK_PERCENT, 0).base_value();
        let charges = i32::from(rank.proc_charges);

        sim.register_spell(
            unit,
            SpellConfig {
                action_id: tagged_action(rank.id, 2),
                spell_school: school::HOLY,
                defense_type: DefenseType::Magic,
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::NO_ON_CAST_COMPLETE
                    | SpellFlag::PASSIVE_SPELL
                    | SpellFlag::BINARY,
                class_spell_mask: masks::HOLY_SHIELD_PROC,
                damage_multiplier: 1.0,
                threat_multiplier: 1.2,
                // Go `HolyShieldSpellPowerFromRow` (MythicSim patch 170): the row's 0.08.
                bonus_coefficient: rank.effect(dbcenums::A_PROC_TRIGGER_DAMAGE, 0).coeff(),
                ..SpellConfig::default()
            },
        );

        let label = sim.unit(unit).label.clone();
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: format!("Holy Shield{label} Rank {}", rank.rank_number()),
                action_id: Some(action_id.clone()),
                duration: rank.duration(),
                max_stacks: charges,
                ..AuraConfig::default()
            },
        );
        sim.attach_proc_trigger(
            aura,
            &ProcTrigger {
                callback: CallbackMask::ON_SPELL_HIT_TAKEN,
                outcome: crate::prepare::aura_helpers::HitOutcome::BLOCK,
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );
        sim.attach_stat_buff(aura, Stat::BlockPercent, block_percent);

        // Steadfast Libram: more block value while the shield is up.
        if self.holy_shield_block_value_multiplier != 1.0 {
            sim.attach_multiplicative_pseudo_stat_buff(
                aura,
                crate::prepare::aura_helpers::PseudoStatField::BlockValueMultiplier,
                self.holy_shield_block_value_multiplier,
            );
        }

        let timer = shared_timer(sim, unit, &mut self.timers.holy_shield);
        sim.register_spell(
            unit,
            SpellConfig {
                action_id,
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::APL | SpellFlag::HELPFUL,
                class_spell_mask: masks::HOLY_SHIELD,
                rank: rank.rank_number(),
                cost: holy_shield_cost(sim, rank),
                cast: gcd_cast(rank.gcd(), 0, Some((timer, cooldown(rank)))),
                has_extra_cast_condition: true,
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
    }
}

/// A Holy Shield rank's mana cost: push 113554's under Forever's rules, where each rank costs
/// what the rank below did (hotfix_113554.rs), the row's in the reference.
fn holy_shield_cost(sim: &Sim, rank: &Row) -> crate::prepare::spell::CostOptions {
    let mut cost = mana_cost(rank);
    if sim.rules == crate::prepare::sim::Rules::Forever {
        if let Some(&(_, _, mana)) = hotfix::HOLY_SHIELD.iter().find(|(id, _, _)| *id == rank.id) {
            cost.mana_flat_cost = mana;
        }
    }
    cost
}
