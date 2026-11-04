//! Go sim/warrior/stances.go: the three stance auras, each in the exclusive "Stance" category,
//! and the spells that cast them.

use std::rc::Rc;

use crate::prepare::aura_helpers::PseudoStatField;
use crate::prepare::dbcenums;
use crate::prepare::sim::{AuraConfig, AuraId, BuildPhase, Sim, TimerId, UnitId, NEVER_EXPIRES};
use crate::prepare::spell::{SpellConfig, SpellFlag};
use crate::prepare::spelldata::Spell as Row;
use crate::prepare::stats::Stat;

use super::helpers::*;
use super::masks;
use super::spell_data::spell_data;
use super::Warrior;

/// Go `stanceEffectCategory`.
pub(super) const STANCE_EFFECT_CATEGORY: &str = "Stance";

impl Warrior {
    /// The build phase a stance aura registers with: the default stance joins the buffs.
    fn stance_build_phase(&self, stance: &str) -> BuildPhase {
        if self.inputs.default_stance == stance {
            BuildPhase::BUFFS
        } else {
            BuildPhase::NONE
        }
    }

    /// Go `makeStanceSpell`.
    fn make_stance_spell(
        &self,
        sim: &mut Sim,
        unit: UnitId,
        mask: i64,
        rank: &Row,
        aura: AuraId,
        stance_cd: TimerId,
    ) {
        let action = sim.aura(aura).action_id.clone().unwrap_or_default();
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: action,
                defense_type: rank.defense_type_core(),
                class_spell_mask: mask,
                flags: SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::APL,
                cast: crate::prepare::spell::CastConfig {
                    cd: crate::prepare::sim::Cooldown {
                        timer: Some(stance_cd),
                        duration: cooldown_of(rank),
                    },
                    ..Default::default()
                },
                has_extra_cast_condition: true,
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
    }

    fn register_battle_stance_aura(&self, sim: &mut Sim, unit: UnitId) -> AuraId {
        let data = spell_data();
        let rank = data.battle_stance.highest();
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Battle Stance".to_string(),
                action_id: Some(spell_action(rank.id)),
                duration: NEVER_EXPIRES,
                build_phase: self.stance_build_phase("WarriorStanceBattle"),
                ..AuraConfig::default()
            },
        );
        sim.attach_multiplicative_pseudo_stat_buff(
            aura,
            PseudoStatField::ThreatMultiplier,
            data.battle_stance_passive
                .effect(dbcenums::A_MOD_THREAT, 127)
                .multiplier_at(1),
        );
        sim.new_exclusive_effect(aura, STANCE_EFFECT_CATEGORY, true, 0.0, None, None);
        aura
    }

    fn register_defensive_stance_aura(&self, sim: &mut Sim, unit: UnitId) -> AuraId {
        let data = spell_data();
        let rank = data.defensive_stance.highest();
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Defensive Stance".to_string(),
                action_id: Some(spell_action(rank.id)),
                duration: NEVER_EXPIRES,
                build_phase: self.stance_build_phase("WarriorStanceDefensive"),
                ..AuraConfig::default()
            },
        );
        let passive = &data.defensive_stance_passive;
        // The threat bonus under these rules: 80% since the notes of 2 November 2026.
        let threat = crate::prepare::hotfix_113554::row(sim.rules, passive.highest().id)
            .effect(dbcenums::A_MOD_THREAT, 127);
        sim.attach_multiplicative_pseudo_stat_buff(
            aura,
            PseudoStatField::ThreatMultiplier,
            1.0 + threat.base_points / 100.0,
        );
        sim.attach_multiplicative_pseudo_stat_buff(
            aura,
            PseudoStatField::DamageTakenMultiplier,
            passive
                .effect(dbcenums::A_MOD_DAMAGE_PERCENT_TAKEN, 127)
                .multiplier_at(1),
        );
        sim.attach_multiplicative_pseudo_stat_buff(
            aura,
            PseudoStatField::DamageDealtMultiplier,
            passive
                .effect(dbcenums::A_MOD_DAMAGE_PERCENT_DONE, 127)
                .multiplier_at(1),
        );
        let defiance_rank = self.talent("defiance");
        if defiance_rank > 0 {
            let defiance = crate::prepare::hotfix_113554::talent(sim.rules, 12792, 3)
                .effect(dbcenums::A_MOD_THREAT, 127)
                .multiplier_at(defiance_rank);
            // Defiance multiplies threat in Defensive Stance while the warrior can block.
            let applied = sim.new_state_slot(0.0);
            let refresh = move |sim: &mut Sim, unit: UnitId, in_stance: bool| {
                let want = in_stance && sim.unit(unit).pseudo_stats.can_block;
                let was = sim.state_flag(applied);
                let threat = &mut sim.unit_mut(unit).pseudo_stats.threat_multiplier;
                if want && !was {
                    *threat *= defiance;
                } else if !want && was {
                    *threat /= defiance;
                }
                sim.set_state_flag(applied, want);
            };
            let on_gain = refresh;
            sim.apply_on_gain(
                aura,
                Rc::new(move |sim: &mut Sim, aura| {
                    let unit = sim.aura(aura).unit;
                    on_gain(sim, unit, true);
                }),
            );
            sim.apply_on_expire(
                aura,
                Rc::new(move |sim: &mut Sim, aura| {
                    let unit = sim.aura(aura).unit;
                    refresh(sim, unit, false);
                }),
            );
            // The off hand item swap callback only runs when items swap.
        }
        sim.new_exclusive_effect(aura, STANCE_EFFECT_CATEGORY, true, 0.0, None, None);
        aura
    }

    fn register_berserker_stance_aura(&self, sim: &mut Sim, unit: UnitId) -> AuraId {
        let data = spell_data();
        let rank = data.berserker_stance.highest();
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Berserker Stance".to_string(),
                action_id: Some(spell_action(rank.id)),
                duration: NEVER_EXPIRES,
                build_phase: self.stance_build_phase("WarriorStanceBerserker"),
                ..AuraConfig::default()
            },
        );
        let passive = &data.berserker_stance_passive;
        sim.attach_multiplicative_pseudo_stat_buff(
            aura,
            PseudoStatField::ThreatMultiplier,
            passive.effect(dbcenums::A_MOD_THREAT, 127).multiplier_at(1),
        );
        sim.attach_multiplicative_pseudo_stat_buff(
            aura,
            PseudoStatField::DamageTakenMultiplier,
            passive
                .effect(dbcenums::A_MOD_DAMAGE_PERCENT_TAKEN, 127)
                .multiplier_at(1),
        );
        sim.attach_stat_buff(
            aura,
            Stat::PhysicalCritPercent,
            passive.effect(dbcenums::A_MOD_CRIT_PCT, 0).value_at(1),
        );
        sim.new_exclusive_effect(aura, STANCE_EFFECT_CATEGORY, true, 0.0, None, None);
        aura
    }

    /// Go `registerStances`.
    pub(super) fn register_stances(&self, sim: &mut Sim, unit: UnitId) {
        let data = spell_data();
        let stance_cd = sim.new_timer(unit);
        let battle = self.register_battle_stance_aura(sim, unit);
        let defensive = self.register_defensive_stance_aura(sim, unit);
        let berserker = self.register_berserker_stance_aura(sim, unit);
        self.make_stance_spell(
            sim,
            unit,
            masks::BATTLE_STANCE,
            data.battle_stance.highest(),
            battle,
            stance_cd,
        );
        self.make_stance_spell(
            sim,
            unit,
            masks::DEFENSIVE_STANCE,
            data.defensive_stance.highest(),
            defensive,
            stance_cd,
        );
        self.make_stance_spell(
            sim,
            unit,
            masks::BERSERKER_STANCE,
            data.berserker_stance.highest(),
            berserker,
            stance_cd,
        );
        match self.inputs.default_stance.as_str() {
            "WarriorStanceBattle" => {
                sim.make_permanent(battle);
            }
            "WarriorStanceDefensive" => {
                sim.make_permanent(defensive);
            }
            "WarriorStanceBerserker" => {
                sim.make_permanent(berserker);
            }
            _ => {}
        }
    }
}
