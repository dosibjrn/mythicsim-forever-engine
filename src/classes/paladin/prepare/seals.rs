//! The Paladin's seals: Go `seals.go` and the `seal_of_*.go` files, with the judgements each
//! seal unleashes.

use std::rc::Rc;

use crate::prepare::aura_helpers::{AbsorptionAuraConfig, CallbackMask, HitOutcome, ProcTrigger};
use crate::prepare::buffs::paladin::{
    judgement_of_light_rank_aura, judgement_of_the_crusader_aura, judgement_of_wisdom_rank_aura,
    JudgementRank, JUDGEMENT_AURA_TAG,
};
use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::dbcenums;
use crate::prepare::sim::{AuraConfig, AuraId, Sim, UnitId, SECOND};
use crate::prepare::spell::{school, DefenseType, ProcMask, SpellConfig, SpellFlag};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::spelldata::Spell as Row;
use crate::prepare::stats::Stat;

use super::masks;
use super::spell_data::spell_data;
use super::util::{
    aura_array_to_map, gcd_cast, mana_cost, new_enemy_aura_array, spell_action, tagged_action,
};
use super::Paladin;

/// Go `SealCategory`: only one seal is up at a time.
pub(super) const SEAL_CATEGORY: &str = "PaladinSeal";

/// Go `sealDuration`.
const SEAL_DURATION: i64 = 30 * SECOND;

/// The damage spell each Seal of Righteousness rank fires on a hit. They carry no rank
/// subtext, so no ladder holds them.
const SEAL_OF_RIGHTEOUSNESS_PROC_IDS: [(i32, i32); 8] = [
    (1, 25741),
    (2, 25740),
    (3, 25739),
    (4, 25738),
    (5, 25737),
    (6, 25736),
    (7, 25735),
    (8, 25713),
];

/// The heal each rank's Judgement of Light grants attackers. It carries no rank subtext, so the
/// pairing is by hand.
pub(super) const JUDGEMENT_OF_LIGHT_HEAL_IDS: [(i32, i32); 4] =
    [(1, 20267), (2, 20341), (3, 20342), (4, 20343)];

/// The mana each rank's Judgement of Wisdom grants attackers.
pub(super) const JUDGEMENT_OF_WISDOM_MANA_IDS: [(i32, i32); 3] =
    [(1, 20268), (2, 20352), (3, 20353)];

pub(super) fn paired(table: &[(i32, i32)], rank: i32) -> i32 {
    table
        .iter()
        .find(|(n, _)| *n == rank)
        .map_or(0, |(_, id)| *id)
}

/// Go `sealLabel`: Seal of Justice has one rank and no rank subtext, so its label carries no
/// rank.
pub(super) fn seal_label(name: &str, label: &str, rank: &Row) -> String {
    if rank.rank_number() == 0 {
        return format!("{name}{label}");
    }
    format!("{name}{label} Rank {}", rank.rank_number())
}

/// The fields every judgement spell of a seal shares: Melee in SpellCategories, so it rolls on
/// the melee table and cannot be dodged, parried or blocked.
fn judgement_config(
    row: &Row,
    class_spell_mask: i64,
    flags: SpellFlag,
    proc_mask: ProcMask,
) -> SpellConfig {
    SpellConfig {
        action_id: spell_action(row.id),
        spell_school: row.spell_school(),
        defense_type: row.defense_type_core(),
        proc_mask,
        flags,
        class_spell_mask,
        damage_multiplier: 1.0,
        threat_multiplier: 1.0,
        ..SpellConfig::default()
    }
}

impl Paladin {
    /// Go `registerSeals`.
    pub(super) fn register_seals(&mut self, sim: &mut Sim, unit: UnitId) {
        spell_data()
            .seal_of_righteousness
            .each(|_, rank| self.register_seal_of_righteousness(sim, unit, rank));
        spell_data()
            .seal_of_light
            .each(|_, rank| self.register_seal_of_light(sim, unit, rank));
        spell_data()
            .seal_of_wisdom
            .each(|_, rank| self.register_seal_of_wisdom(sim, unit, rank));
        self.register_seal_of_justice(sim, unit);
        spell_data()
            .seal_of_the_crusader
            .each(|_, rank| self.register_seal_of_the_crusader(sim, unit, rank));
        spell_data()
            .seal_of_fury
            .each(|_, rank| self.register_seal_of_fury(sim, unit, rank));
    }

    /// Go `makeSealExclusive`: every seal aura joins the seal category, so casting one seal
    /// drops the other.
    fn make_seal_exclusive(&self, sim: &mut Sim, aura: AuraId) -> AuraId {
        sim.new_exclusive_effect(aura, SEAL_CATEGORY, true, 0.0, None, None);
        aura
    }

    /// Go `registerSealSpell`: the castable seal spell, instant, on the GCD, priced as the row
    /// says.
    fn register_seal_spell(
        &self,
        sim: &mut Sim,
        unit: UnitId,
        rank: &Row,
        class_mask: i64,
        aura: AuraId,
    ) {
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::APL,
                class_spell_mask: class_mask,
                rank: rank.rank_number(),
                cost: mana_cost(rank),
                cast: gcd_cast(rank.gcd(), 0, None),
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
    }

    /// Go `newJudgementAuras`: the judgement debuffs Light, Wisdom and the Crusader put up.
    fn new_judgement_auras(
        &mut self,
        sim: &mut Sim,
        make_aura: impl FnMut(&mut Sim, UnitId) -> AuraId,
    ) -> Vec<Option<AuraId>> {
        let auras = new_enemy_aura_array(sim, make_aura);
        self.judgement_auras.push(auras.clone());
        auras
    }

    /// Go `registerSealOfRighteousness`: fills the Paladin with holy spirit for 30 sec,
    /// granting each melee attack an additional X to Y Holy damage.
    fn register_seal_of_righteousness(&mut self, sim: &mut Sim, unit: UnitId, rank: &Row) {
        let judge_rank = spell_data()
            .judgement_of_righteousness
            .by_id(rank.effect_n(2).base_value() as i32);
        let per_hit = judge_rank.effect_n(2);

        let judgement = SpellConfig {
            // Go `JudgementOfRighteousnessSpellPowerFromRow` (MythicSim patch 168): the row's 0.429.
            bonus_coefficient: judge_rank.damage_effect().coeff(),
            ..judgement_config(
                judge_rank,
                masks::JUDGEMENT_OF_RIGHTEOUSNESS,
                SpellFlag::MELEE_METRICS | SpellFlag::BINARY,
                ProcMask::MELEE_MH_SPECIAL,
            )
        };
        sim.register_spell(unit, judgement);

        let coefficient =
            righteousness_hit_coefficient(per_hit.coeff(), self.main_hand_is_two_hand(sim, unit));
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(paired(
                    &SEAL_OF_RIGHTEOUSNESS_PROC_IDS,
                    rank.rank_number(),
                )),
                spell_school: school::HOLY,
                defense_type: DefenseType::Melee,
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                // The damage spells are procs, so auras without Can Proc From Procs never
                // hear them.
                flags: SpellFlag::MELEE_METRICS | SpellFlag::PASSIVE_SPELL | SpellFlag::PROC,
                class_spell_mask: masks::SEAL_OF_RIGHTEOUSNESS_PROC,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                bonus_coefficient: coefficient,
                ..SpellConfig::default()
            },
        );

        let label = sim.unit(unit).label.clone();
        let aura = sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: seal_label("Seal of Righteousness", &label, rank),
                action_id: spell_action(rank.id),
                metrics_action_id: spell_action(rank.id),
                duration: SEAL_DURATION,
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                proc_mask: ProcMask::MELEE_WHITE_HIT,
                outcome: HitOutcome::LANDED,
                ..ProcTrigger::default()
            },
        );
        let aura = self.make_seal_exclusive(sim, aura);
        self.register_seal_spell(sim, unit, rank, masks::SEAL_OF_RIGHTEOUSNESS, aura);
    }

    /// Whether the main hand holds a two-hander: Go `GetMainHandType() == HandTypeTwoHand`.
    pub(super) fn main_hand_is_two_hand(&self, sim: &Sim, unit: UnitId) -> bool {
        sim.mh_weapon(unit)
            .is_some_and(|item| item.hand_type == "HandTypeTwoHand")
    }

    /// Go `registerSealOfCommand`: gives the Paladin a chance to deal additional Holy damage
    /// equal to 70% of normal weapon damage.
    pub(super) fn register_seal_of_command(&mut self, sim: &mut Sim, unit: UnitId, rank: &Row) {
        let judge_rank = spell_data().judgement_of_command.rank(rank.rank_number());
        let judge_damage = spell_data()
            .seal_of_command_triggered
            .by_id(judge_rank.effect_n(1).base_value() as i32)
            .damage_effect();

        let judgement = SpellConfig {
            bonus_coefficient: judge_damage.coeff(),
            ..judgement_config(
                judge_rank,
                masks::JUDGEMENT_OF_COMMAND,
                SpellFlag::MELEE_METRICS,
                ProcMask::MELEE_MH_SPECIAL,
            )
        };
        sim.register_spell(unit, judgement);

        let proc_rank = spell_data().seal_of_command_triggered.rank(1);
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(proc_rank.id),
                spell_school: school::HOLY,
                defense_type: DefenseType::Melee,
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                // 20424 carries Not a Proc: it is an ability hit to every listener.
                flags: SpellFlag::MELEE_METRICS | SpellFlag::PASSIVE_SPELL,
                class_spell_mask: masks::SEAL_OF_COMMAND_PROC,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                // No BonusCoefficient: the spell power goes through the weapon percent.
                ..SpellConfig::default()
            },
        );

        // The seal and its Echo roll the same chance: the proc manager and the one second
        // cooldown between procs.
        let _ = sim.new_timer(unit);

        let label = sim.unit(unit).label.clone();
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: seal_label("Seal of Command", &label, rank),
                action_id: Some(spell_action(rank.id)),
                duration: SEAL_DURATION,
                ..AuraConfig::default()
            },
        );
        sim.attach_proc_trigger(
            aura,
            &ProcTrigger {
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                proc_mask: ProcMask::MELEE_WHITE_HIT,
                outcome: HitOutcome::LANDED,
                ..ProcTrigger::default()
            },
        );
        let aura = self.make_seal_exclusive(sim, aura);
        self.register_seal_spell(sim, unit, rank, masks::SEAL_OF_COMMAND, aura);
    }

    /// Go `registerSealOfLight`: gives each melee attack a chance to heal the Paladin.
    fn register_seal_of_light(&mut self, sim: &mut Sim, unit: UnitId, rank: &Row) {
        let judgement_id = rank.effect_n(2).base_value() as i32;
        let heal_rank = spell_data()
            .seal_of_light_triggered
            .by_id(paired(&JUDGEMENT_OF_LIGHT_HEAL_IDS, rank.rank_number()));
        let judgement_rank = JudgementRank {
            spell_id: judgement_id,
            rank: rank.rank_number(),
            value: heal_rank.heal_effect().average(CHARACTER_LEVEL),
        };
        let judgement_auras = self.new_judgement_auras(sim, |sim, target| {
            judgement_of_light_rank_aura(sim, target, &judgement_rank)
        });

        // Melee in SpellCategories and Always Hit: the debuff lands without a roll.
        let related_aura_arrays = aura_array_to_map(sim, &judgement_auras);
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(judgement_id),
                spell_school: school::HOLY,
                defense_type: DefenseType::Melee,
                // Go `debuffJudgementProcMask` and `debuffJudgementFlags` (MythicSim patch 169).
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                flags: SpellFlag::MELEE_METRICS | SpellFlag::PROC,
                class_spell_mask: masks::JUDGEMENT_OF_LIGHT,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                related_aura_arrays,
                ..SpellConfig::default()
            },
        );

        let proc_rank = rank.refs()[0];
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(proc_rank.id),
                spell_school: school::HOLY,
                proc_mask: ProcMask::SPELL_HEALING,
                // The heals lack Not a Proc.
                flags: SpellFlag::HELPFUL | SpellFlag::PASSIVE_SPELL | SpellFlag::PROC,
                class_spell_mask: masks::SEAL_OF_LIGHT_PROC,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        );

        let label = sim.unit(unit).label.clone();
        let aura = sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: seal_label("Seal of Light", &label, rank),
                // Go `SealOfLightFifteenPPM` (MythicSim patch 169): NewLegacyPPMManager(15, melee),
                // which an item swap rebuilds for the weapons in the hands.
                dpm: Some(std::rc::Rc::new(
                    sim.new_ppm_manager(unit, 15.0, ProcMask::MELEE)
                        .rebuilt_by_swap(
                            sim,
                            unit,
                            crate::prepare::item_swap::ManagerChance::Ppm(15.0),
                            crate::prepare::item_swap::ManagerMask::Static(ProcMask::MELEE),
                        ),
                )),
                action_id: spell_action(rank.id),
                metrics_action_id: spell_action(rank.id),
                duration: SEAL_DURATION,
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                proc_mask: ProcMask::MELEE,
                outcome: HitOutcome::LANDED,
                ..ProcTrigger::default()
            },
        );
        let aura = self.make_seal_exclusive(sim, aura);
        self.register_seal_spell(sim, unit, rank, masks::SEAL_OF_LIGHT, aura);
    }

    /// Go `registerSealOfWisdom`: gives each melee attack a chance to restore mana to the
    /// Paladin.
    fn register_seal_of_wisdom(&mut self, sim: &mut Sim, unit: UnitId, rank: &Row) {
        let judgement_id = rank.effect_n(2).base_value() as i32;
        let mana_rank = spell_data()
            .seal_of_wisdom_triggered
            .by_id(paired(&JUDGEMENT_OF_WISDOM_MANA_IDS, rank.rank_number()));
        let judgement_rank = JudgementRank {
            spell_id: judgement_id,
            rank: rank.rank_number(),
            value: mana_rank.energize_effect().average(CHARACTER_LEVEL),
        };
        let judgement_auras = self.new_judgement_auras(sim, |sim, target| {
            judgement_of_wisdom_rank_aura(sim, target, &judgement_rank)
        });

        let related_aura_arrays = aura_array_to_map(sim, &judgement_auras);
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(judgement_id),
                spell_school: school::HOLY,
                defense_type: DefenseType::Melee,
                // Go `debuffJudgementProcMask` and `debuffJudgementFlags` (MythicSim patch 169).
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                flags: SpellFlag::MELEE_METRICS | SpellFlag::PROC,
                class_spell_mask: masks::JUDGEMENT_OF_WISDOM,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                related_aura_arrays,
                ..SpellConfig::default()
            },
        );

        let proc_rank = rank.refs()[0];
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(proc_rank.id),
                spell_school: school::HOLY,
                proc_mask: ProcMask::EMPTY,
                // The restores lack Not a Proc.
                flags: SpellFlag::HELPFUL | SpellFlag::PASSIVE_SPELL | SpellFlag::PROC,
                class_spell_mask: masks::SEAL_OF_WISDOM_PROC,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        );

        let label = sim.unit(unit).label.clone();
        let aura = sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: seal_label("Seal of Wisdom", &label, rank),
                action_id: spell_action(rank.id),
                metrics_action_id: spell_action(rank.id),
                duration: SEAL_DURATION,
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                proc_mask: ProcMask::MELEE,
                outcome: HitOutcome::LANDED,
                ..ProcTrigger::default()
            },
        );
        let aura = self.make_seal_exclusive(sim, aura);
        self.register_seal_spell(sim, unit, rank, masks::SEAL_OF_WISDOM, aura);
    }

    /// Go `registerSealOfJustice`: gives each melee attack a chance to stun for 2 sec.
    fn register_seal_of_justice(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = spell_data().seal_of_justice.highest();
        let judgement_rank = spell_data()
            .seal_of_justice_triggered
            .by_id(rank.effect_n(2).base_value() as i32);
        let judgement_auras = self.new_judgement_auras(sim, |sim, target| {
            sim.get_or_register_aura(
                target,
                AuraConfig {
                    label: "Judgement of Justice".to_string(),
                    action_id: Some(spell_action(judgement_rank.id)),
                    tag: JUDGEMENT_AURA_TAG.to_string(),
                    duration: judgement_rank.duration(),
                    ..AuraConfig::default()
                },
            )
        });

        let related_aura_arrays = aura_array_to_map(sim, &judgement_auras);
        sim.register_spell(
            unit,
            SpellConfig {
                related_aura_arrays,
                ..judgement_config(
                    judgement_rank,
                    masks::JUDGEMENT_OF_JUSTICE,
                    // Go `debuffJudgementFlags` and `debuffJudgementProcMask` (MythicSim patch 169).
                    SpellFlag::MELEE_METRICS | SpellFlag::PROC,
                    ProcMask::MELEE_MH_SPECIAL,
                )
            },
        );

        let label = sim.unit(unit).label.clone();
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: seal_label("Seal of Justice", &label, rank),
                action_id: Some(spell_action(rank.id)),
                duration: SEAL_DURATION,
                ..AuraConfig::default()
            },
        );
        let aura = self.make_seal_exclusive(sim, aura);
        self.register_seal_spell(sim, unit, rank, masks::SEAL_OF_JUSTICE, aura);
    }

    /// Go `registerSealOfTheCrusader`: grants melee attack power and 40% attack speed, at less
    /// damage per attack.
    fn register_seal_of_the_crusader(&mut self, sim: &mut Sim, unit: UnitId, rank: &Row) {
        let judgement_rank = spell_data()
            .seal_of_the_crusader_triggered
            .by_id(rank.effect_n(3).base_value() as i32);
        let value = judgement_rank
            .effect(dbcenums::A_MOD_DAMAGE_TAKEN, 2)
            .average(CHARACTER_LEVEL)
            + self.judgement_of_the_crusader_bonus;
        let crusader_rank = JudgementRank {
            spell_id: judgement_rank.id,
            rank: rank.rank_number(),
            value,
        };
        let judgement_auras = self.new_judgement_auras(sim, |sim, target| {
            judgement_of_the_crusader_aura(sim, target, &crusader_rank)
        });

        let related_aura_arrays = aura_array_to_map(sim, &judgement_auras);
        sim.register_spell(
            unit,
            SpellConfig {
                related_aura_arrays,
                ..judgement_config(
                    judgement_rank,
                    masks::JUDGEMENT_OF_THE_CRUSADER,
                    // Go `debuffJudgementFlags` and `debuffJudgementProcMask` (MythicSim patch 169).
                    SpellFlag::MELEE_METRICS | SpellFlag::PROC,
                    ProcMask::MELEE_MH_SPECIAL,
                )
            },
        );

        // The high end of the attack power, which is the number the game shows where the two
        // differ. Go fuses the high end's multiply with the bonus.
        let effect = rank.effect(dbcenums::A_MOD_ATTACK_POWER, 0);
        let attack_power = effect.average(CHARACTER_LEVEL).mul_add(
            1.0 + effect.variance / 2.0,
            self.seal_of_the_crusader_bonus_attack_power,
        );
        let speed = 1.0 + rank.effect(dbcenums::A_MOD_ATTACKSPEED, 0).percent();

        let label = sim.unit(unit).label.clone();
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: seal_label("Seal of the Crusader", &label, rank),
                action_id: Some(spell_action(rank.id)),
                duration: SEAL_DURATION,
                ..AuraConfig::default()
            },
        );
        sim.attach_stat_buff(aura, Stat::AttackPower, attack_power);
        sim.attach_multiply_melee_speed(aura, speed);
        sim.attach_spell_mod(
            aura,
            SpellModConfig {
                proc_mask: ProcMask::MELEE_MH_AUTO,
                kind: SpellModType::DamageDonePct,
                float_value: 1.0 / speed - 1.0,
                ..SpellModConfig::default()
            },
        );
        let aura = self.make_seal_exclusive(sim, aura);
        self.register_seal_spell(sim, unit, rank, masks::SEAL_OF_THE_CRUSADER, aura);
    }

    /// Go `registerSealOfFury`: melee attacks deal additional Holy damage, and with a shield
    /// each attack grants an absorb shield.
    fn register_seal_of_fury(&mut self, sim: &mut Sim, unit: UnitId, rank: &Row) {
        let judgement_rank = spell_data()
            .seal_of_fury_triggered
            .by_id(rank.effect_n(3).base_value() as i32);
        let proc_rank = rank.refs()[0];

        let judgement = SpellConfig {
            // Measured in game on client 70425 (MythicSim patch 143); the rows carry 0.429.
            // Go `SealOfFurySpellPowerFromRows` (MythicSim patch 168): the judgement's row.
            bonus_coefficient: judgement_rank.damage_effect().coeff(),
            ..judgement_config(
                judgement_rank,
                masks::JUDGEMENT_OF_FURY,
                SpellFlag::MELEE_METRICS | SpellFlag::BINARY,
                ProcMask::MELEE_MH_SPECIAL,
            )
        };
        sim.register_spell(unit, judgement);

        let label = sim.unit(unit).label.clone();
        let _shield = sim.new_damage_absorption_aura(
            unit,
            AbsorptionAuraConfig {
                aura: AuraConfig {
                    label: format!("Seal of Fury Shield{label} Rank {}", rank.rank_number()),
                    action_id: Some(tagged_action(rank.id, 1)),
                    duration: SEAL_DURATION,
                    ..AuraConfig::default()
                },
                shield_strength_calculator: Some(Rc::new(|_, _| 0.0)),
                ..AbsorptionAuraConfig::default()
            },
        );
        // Improved Seal of Fury attaches a callback to the shield that only runs in a fight.

        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(proc_rank.id),
                spell_school: school::HOLY,
                defense_type: DefenseType::Melee,
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                // The damage spells carry the same flags as Seal of Righteousness's.
                flags: SpellFlag::MELEE_METRICS | SpellFlag::PASSIVE_SPELL | SpellFlag::PROC,
                class_spell_mask: masks::SEAL_OF_FURY_PROC,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                // Measured in game on client 70425 (MythicSim patch 143); the rows carry 0.10.
                // Go `SealOfFurySpellPowerFromRows` (MythicSim patch 168): the proc's row.
                bonus_coefficient: proc_rank.damage_effect().coeff(),
                ..SpellConfig::default()
            },
        );

        let aura = sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: seal_label("Seal of Fury", &label, rank),
                action_id: spell_action(rank.id),
                metrics_action_id: spell_action(rank.id),
                duration: SEAL_DURATION,
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                proc_mask: ProcMask::MELEE_WHITE_HIT,
                outcome: HitOutcome::LANDED,
                ..ProcTrigger::default()
            },
        );
        let aura = self.make_seal_exclusive(sim, aura);
        self.register_seal_spell(sim, unit, rank, masks::SEAL_OF_FURY, aura);
    }
}

/// Go `sealOfRighteousnessHitCoefficient` with `SealOfRighteousnessSpellPowerFixed` on (MythicSim
/// patch 158): the judgement dummy's share, x1.2 on a two-hander, halved.
pub(super) fn righteousness_hit_coefficient(dummy: f64, two_hand: bool) -> f64 {
    let mut coefficient = dummy;
    if two_hand {
        coefficient *= 1.2;
    }
    coefficient / 2.0
}
