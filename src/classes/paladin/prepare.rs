//! Paladin preparation: Go sim/paladin's construction and initialization, and the exporter's
//! Paladin description (tools/oracle-v2/paladin.go).
//!
//! A closure Go gives a spell, an aura or a proc trigger only runs in a fight. What preparation
//! keeps of it is what Go's registration and the exporter read: the fields a spell carries, the
//! callbacks an aura lists and the exclusive categories it joins.

mod abilities;
mod auras;
mod export;
mod export_tank;
mod heals;
pub(crate) mod hotfix_113554;
pub(crate) mod items;
pub(crate) mod masks;
mod oath_of_light;
mod seals;
pub(crate) mod sets;
mod spell_data;
mod talent_spells;
mod talents;
mod util;

use crate::contracts::request::Message;
use crate::prepare::agent::{
    fill_talents, fill_talents_by_layout_with_extras, ClassSpellName, PrepAgent,
};
use crate::prepare::attack::{AutoAttackOptions, Weapon};
use crate::prepare::character::constants::DODGE_RATING_PER_DODGE_PERCENT;
use crate::prepare::env::Environment;
use crate::prepare::sim::{AuraId, Duration, Rules, Sim, SpellId, TimerId, UnitId};
use crate::prepare::stats::Stat;
use crate::prepare::Refusal;

use self::spell_data::spell_data;

/// Go `RighteousFuryHolyThreat`: the extra threat Righteous Fury puts on Holy attacks (16 October
/// 2026 beta notes; was the row's 60%).
pub(super) const RIGHTEOUS_FURY_HOLY_THREAT: f64 = 0.9;

/// Go paladin.TalentTreeSizes.
const TALENT_TREE_SIZES: [usize; 3] = [17, 16, 17];

/// tools/oracle-v2/paladin.go `paladinClassSpells`.
static CLASS_SPELLS: &[ClassSpellName] = &[
    ClassSpellName {
        mask: masks::JUDGEMENT,
        name: "judgement",
    },
    ClassSpellName {
        mask: masks::HOLY_STRIKE,
        name: "holy_strike",
    },
    ClassSpellName {
        mask: masks::CONSECRATION,
        name: "consecration",
    },
    ClassSpellName {
        mask: masks::EXORCISM,
        name: "exorcism",
    },
    ClassSpellName {
        mask: masks::HAMMER_OF_WRATH,
        name: "hammer_of_wrath",
    },
    ClassSpellName {
        mask: masks::HOLY_WRATH,
        name: "holy_wrath",
    },
    ClassSpellName {
        mask: masks::HOLY_LIGHT,
        name: "holy_light",
    },
    ClassSpellName {
        mask: masks::FLASH_OF_LIGHT,
        name: "flash_of_light",
    },
    ClassSpellName {
        mask: masks::LAY_ON_HANDS,
        name: "lay_on_hands",
    },
    ClassSpellName {
        mask: masks::RIGHTEOUS_FURY,
        name: "righteous_fury",
    },
    ClassSpellName {
        mask: masks::HAMMER_OF_THE_RIGHTEOUS,
        name: "hammer_of_the_righteous",
    },
    ClassSpellName {
        mask: masks::DIVINE_FAVOR,
        name: "divine_favor",
    },
    ClassSpellName {
        mask: masks::OATH_OF_LIGHT,
        name: "oath_of_light",
    },
    ClassSpellName {
        mask: masks::HOLY_SHOCK,
        name: "holy_shock",
    },
    ClassSpellName {
        mask: masks::HOLY_SHOCK_HEAL,
        name: "holy_shock_heal",
    },
    ClassSpellName {
        mask: masks::HOLY_SHIELD,
        name: "holy_shield",
    },
    ClassSpellName {
        mask: masks::HOLY_SHIELD_PROC,
        name: "holy_shield_proc",
    },
    ClassSpellName {
        mask: masks::SWIFT_JUDGEMENT,
        name: "swift_judgement",
    },
    ClassSpellName {
        mask: masks::SWIFT_JUDGEMENT | masks::SECOND_JUDGEMENT,
        name: "second_judgement",
    },
    ClassSpellName {
        mask: masks::TEMPLARS_BULWARK,
        name: "templars_bulwark",
    },
    ClassSpellName {
        mask: masks::LIGHTS_VIGIL,
        name: "lights_vigil",
    },
    ClassSpellName {
        mask: masks::LIGHTS_VIGIL_STRIKE,
        name: "lights_vigil_strike",
    },
    ClassSpellName {
        mask: masks::LIGHTS_VIGIL_ALLY,
        name: "lights_vigil_ally",
    },
    ClassSpellName {
        mask: masks::LIGHTS_VIGIL_HEAL,
        name: "lights_vigil_heal",
    },
    ClassSpellName {
        mask: masks::SEAL_OF_RIGHTEOUSNESS,
        name: "seal_of_righteousness",
    },
    ClassSpellName {
        mask: masks::SEAL_OF_COMMAND,
        name: "seal_of_command",
    },
    ClassSpellName {
        mask: masks::SEAL_OF_LIGHT,
        name: "seal_of_light",
    },
    ClassSpellName {
        mask: masks::SEAL_OF_WISDOM,
        name: "seal_of_wisdom",
    },
    ClassSpellName {
        mask: masks::SEAL_OF_JUSTICE,
        name: "seal_of_justice",
    },
    ClassSpellName {
        mask: masks::SEAL_OF_THE_CRUSADER,
        name: "seal_of_the_crusader",
    },
    ClassSpellName {
        mask: masks::SEAL_OF_FURY,
        name: "seal_of_fury",
    },
    ClassSpellName {
        mask: masks::SEAL_OF_RIGHTEOUSNESS_PROC,
        name: "seal_of_righteousness_proc",
    },
    ClassSpellName {
        mask: masks::SEAL_OF_COMMAND_PROC,
        name: "seal_of_command_proc",
    },
    ClassSpellName {
        mask: masks::SEAL_OF_LIGHT_PROC,
        name: "seal_of_light_proc",
    },
    ClassSpellName {
        mask: masks::SEAL_OF_WISDOM_PROC,
        name: "seal_of_wisdom_proc",
    },
    ClassSpellName {
        mask: masks::SEAL_OF_FURY_PROC,
        name: "seal_of_fury_proc",
    },
    ClassSpellName {
        mask: masks::JUDGEMENT_OF_RIGHTEOUSNESS,
        name: "judgement_of_righteousness",
    },
    ClassSpellName {
        mask: masks::JUDGEMENT_OF_COMMAND,
        name: "judgement_of_command",
    },
    ClassSpellName {
        mask: masks::JUDGEMENT_OF_LIGHT,
        name: "judgement_of_light",
    },
    ClassSpellName {
        mask: masks::JUDGEMENT_OF_WISDOM,
        name: "judgement_of_wisdom",
    },
    ClassSpellName {
        mask: masks::JUDGEMENT_OF_JUSTICE,
        name: "judgement_of_justice",
    },
    ClassSpellName {
        mask: masks::JUDGEMENT_OF_THE_CRUSADER,
        name: "judgement_of_the_crusader",
    },
    ClassSpellName {
        mask: masks::JUDGEMENT_OF_FURY,
        name: "judgement_of_fury",
    },
    ClassSpellName {
        mask: masks::DEVOTION_AURA,
        name: "devotion_aura",
    },
    ClassSpellName {
        mask: masks::RETRIBUTION_AURA,
        name: "retribution_aura",
    },
    ClassSpellName {
        mask: masks::CONCENTRATION_AURA,
        name: "concentration_aura",
    },
    ClassSpellName {
        mask: masks::FIRE_RESISTANCE_AURA,
        name: "fire_resistance_aura",
    },
    ClassSpellName {
        mask: masks::FROST_RESISTANCE_AURA,
        name: "frost_resistance_aura",
    },
    ClassSpellName {
        mask: masks::SHADOW_RESISTANCE_AURA,
        name: "shadow_resistance_aura",
    },
];

/// Go `Paladin`: what its registrations leave on the agent and what its spells read as they
/// register.
#[derive(Clone)]
pub(crate) struct Paladin {
    talents: Message,
    /// Under Forever's rules, the points of the talents push 113554 adds, which the pinned proto
    /// does not have, by upstream #772's field name.
    new_talents: std::collections::BTreeMap<&'static str, i64>,
    /// Whether the sim is MythicSim's Holy Paladin healer sim, the only one that models Light's
    /// Vigil on an ally (push 113554).
    healer: bool,
    /// Go `Paladin.Judgement`.
    judgement: Option<SpellId>,
    /// Go `Paladin.Forbearance`.
    forbearance: Option<AuraId>,
    /// Go `Paladin.RighteousFuryAura`.
    righteous_fury_aura: Option<AuraId>,
    /// Go `Paladin.JudgementAuras`: per judgement, its debuff on each unit by unit index.
    judgement_auras: Vec<Vec<Option<AuraId>>>,
    /// Go `Paladin.consecratedGroundAuras`.
    consecrated_ground_auras: Option<Vec<Option<AuraId>>>,
    /// What gear adds to numbers the spells read as they register.
    seal_of_the_crusader_bonus_attack_power: f64,
    judgement_of_the_crusader_bonus: f64,
    flash_of_light_bonus_healing: f64,
    holy_shield_block_value_multiplier: f64,
    forbearance_reduction: Duration,
    /// Timers shared by the ranks of one ability.
    timers: Timers,
    /// Effects on the attack tables, which exist only once the environment finalizes.
    post_finalize: Vec<crate::prepare::env::FinalizeEffect>,
}

/// Go's `judgementTimer` and the other timers the ranks of an ability share.
#[derive(Clone, Default)]
struct Timers {
    judgement: Option<TimerId>,
    holy_strike: Option<TimerId>,
    consecration: Option<TimerId>,
    exorcism: Option<TimerId>,
    hammer_of_wrath: Option<TimerId>,
    holy_wrath: Option<TimerId>,
    holy_shock: Option<TimerId>,
    holy_shield: Option<TimerId>,
    lights_vigil: Option<TimerId>,
}

/// Go `NewPaladin` through the spec's factory (`NewRetributionPaladin` and its siblings).
pub(crate) fn new_paladin(
    sim: &mut Sim,
    unit: UnitId,
    player: &Message,
) -> Result<Box<dyn PrepAgent>, Refusal> {
    // Go reads `options.GetXPaladin().Options.ClassOptions` without a check, so a spec message
    // without its options panics.
    let (spec, _) = player
        .oneof("spec")
        .filter(|(name, _)| {
            matches!(
                *name,
                "retribution_paladin" | "protection_paladin" | "holy_paladin"
            )
        })
        .ok_or_else(|| {
            Refusal::new(
                "spec",
                "a paladin without a paladin spec is not prepared".to_string(),
            )
        })?;
    if player
        .message(spec)
        .and_then(|spec| spec.message("options"))
        .is_none()
    {
        return Err(Refusal::new(
            "class_option",
            "a paladin without its options".to_string(),
        ));
    }
    let talents_string = player.str("talents_string");
    let (talents, new_talents) = match sim.rules {
        // Push 113554's tree, read in the client's order (hotfix_113554.rs).
        Rules::Forever => check_talents_string(talents_string)
            .and_then(|()| hotfix_113554::check_ranks(talents_string))
            .and_then(|()| {
                fill_talents_by_layout_with_extras(
                    "proto.PaladinTalents",
                    talents_string,
                    |tree, i| Some(hotfix_113554::TREES.get(tree)?.get(i)?.field),
                )
            }),
        Rules::GoReference => {
            fill_talents("proto.PaladinTalents", talents_string, TALENT_TREE_SIZES)
                .map(|talents| (talents, Default::default()))
        }
    }
    .map_err(|err| Refusal::new("talents", err))?;

    // The attack table already holds the base 5% parry and block (sim/core/target.go); the
    // base dodge 0.7% and 1% dodge per 19.8 Agility are the Classic paladin's.
    let pseudo = &mut sim.unit_mut(unit).pseudo_stats;
    pseudo.can_parry = true;
    pseudo.base_dodge_chance += 0.007;

    sim.enable_mana_bar(unit);

    let main_hand = match sim.mh_weapon(unit) {
        Some(item) => Weapon::from_item(item, sim.unit(unit).pseudo_stats.bonus_mh_dps),
        None => Weapon::unarmed(),
    };
    sim.enable_auto_attacks(
        unit,
        AutoAttackOptions {
            main_hand,
            auto_swing_melee: true,
            ..AutoAttackOptions::default()
        },
    );

    let crit_per_agi = sim.crit_per_agi_max_level(unit);
    let sdm = &mut sim.unit_mut(unit).sdm;
    sdm.add_stat_dependency(Stat::Strength, Stat::AttackPower, 2.0);
    // Block value from Strength is Classic's Str/20 - 1, as the warrior's and master's.
    sdm.add_stat_dependency(Stat::Strength, Stat::BlockValue, 1.0 / 20.0);
    sim.add_stat(unit, Stat::BlockValue, -1.0);
    let sdm = &mut sim.unit_mut(unit).sdm;
    sdm.add_stat_dependency(Stat::Agility, Stat::PhysicalCritPercent, crit_per_agi);
    sdm.add_stat_dependency(
        Stat::Agility,
        Stat::DodgeRating,
        crit_per_agi * DODGE_RATING_PER_DODGE_PERCENT,
    );
    sdm.add_stat_dependency(Stat::BonusArmor, Stat::Armor, 1.0);

    Ok(Box::new(Paladin {
        talents,
        new_talents,
        healer: spec == "holy_paladin",
        judgement: None,
        forbearance: None,
        righteous_fury_aura: None,
        judgement_auras: Vec::new(),
        consecrated_ground_auras: None,
        seal_of_the_crusader_bonus_attack_power: 0.0,
        judgement_of_the_crusader_bonus: 0.0,
        flash_of_light_bonus_healing: 0.0,
        holy_shield_block_value_multiplier: 1.0,
        forbearance_reduction: 0,
        timers: Timers::default(),
        post_finalize: Vec::new(),
    }))
}

/// Under Forever's rules, a string with more trees than three or a tree longer than push
/// 113554's 17 talents is refused, as Go refuses a talent past its proto.
fn check_talents_string(talents: &str) -> Result<(), String> {
    let trees: Vec<&str> = talents.split('-').collect();
    if trees.len() > hotfix_113554::TREES.len() {
        return Err(format!(
            "paladin talents {talents:?}: {} trees, the paladin has {}",
            trees.len(),
            hotfix_113554::TREES.len()
        ));
    }
    for (tree, digits) in trees.iter().enumerate() {
        let size = hotfix_113554::TREES[tree].len();
        if digits.chars().count() > size {
            return Err(format!(
                "paladin talents {talents:?}: tree {} has {size} talents, the string gives {}",
                tree + 1,
                digits.chars().count()
            ));
        }
    }
    Ok(())
}

impl Paladin {
    /// The points in a talent push 113554 adds (`hotfix_113554`), which only Forever's rules
    /// read: 0 under the reference.
    pub(super) fn new_talent(&self, field: &str) -> i32 {
        self.new_talents
            .get(field)
            .map_or(0, |&points| points as i32)
    }
}

impl PrepAgent for Paladin {
    /// Go `Paladin.ApplyTalents`.
    fn apply_talents(&mut self, sim: &mut Sim, unit: UnitId) {
        self.register_talent_spells(sim, unit);

        self.register_holy_talents(sim, unit);
        self.register_protection_talents(sim, unit);
        self.register_retribution_talents(sim, unit);
    }

    /// Go `Paladin.Initialize`.
    fn initialize(&mut self, sim: &mut Sim, unit: UnitId) {
        self.register_forbearance(sim, unit);
        self.register_righteous_fury(sim, unit);

        self.register_judgement(sim, unit);
        self.register_seals(sim, unit);
        self.register_auras(sim, unit);

        spell_data()
            .holy_strike
            .each(|_, rank| self.register_holy_strike(sim, unit, rank));
        self.register_hammer_of_the_righteous(sim, unit);
        spell_data()
            .consecration
            .each(|n, rank| self.register_consecration(sim, unit, n, rank));
        spell_data()
            .exorcism
            .each(|_, rank| self.register_exorcism(sim, unit, rank));
        spell_data()
            .hammer_of_wrath
            .each(|_, rank| self.register_hammer_of_wrath(sim, unit, rank));
        spell_data()
            .holy_wrath
            .each(|_, rank| self.register_holy_wrath(sim, unit, rank));

        spell_data()
            .holy_light
            .each(|_, rank| self.register_holy_light(sim, unit, rank));
        spell_data()
            .flash_of_light
            .each(|_, rank| self.register_flash_of_light(sim, unit, rank));
        spell_data()
            .lay_on_hands
            .each(|_, rank| self.register_lay_on_hands(sim, unit, rank));
    }

    fn apply_item_effect(&mut self, sim: &mut Sim, unit: UnitId, item: i32) -> bool {
        self.apply_class_item_effect(sim, unit, item)
    }

    fn take_post_finalize_effects(&mut self) -> Vec<crate::prepare::env::FinalizeEffect> {
        std::mem::take(&mut self.post_finalize)
    }

    fn talents(&self) -> &Message {
        &self.talents
    }

    fn class_spells(&self) -> &'static [ClassSpellName] {
        CLASS_SPELLS
    }

    fn export_effects(&self, env: &Environment, notes: &mut Vec<String>) -> Vec<serde_json::Value> {
        self.export_effects_with_notes(env, notes)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn damage_effect(&self, sim: &Sim, spell: SpellId) -> Option<serde_json::Value> {
        export::damage_effect(sim, spell)
    }

    fn stat_auras(&self, sim: &Sim, unit: UnitId) -> Vec<String> {
        self.export_stat_auras(sim, unit)
    }
}
