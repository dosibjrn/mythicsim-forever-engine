//! Go sim/druid/items.go and item_sets.go: the idols and the Wolfshead Helm the class wires,
//! and the item sets it registers.

use std::rc::Rc;

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger, PseudoStatField};
use crate::prepare::character::constants::SPELL_CRIT_RATING_PER_CRIT_PERCENT;
use crate::prepare::env::Environment;
use crate::prepare::item_sets::ItemSet;
use crate::prepare::sim::{AuraConfig, AuraId, Rules, Sim, UnitId, MILLISECOND};
use crate::prepare::spell::ProcMask;
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::stats::{Stat, Stats};

use super::{masks, Druid};

/// The idols the druid package registers whose effect needs nothing of a druid (Go
/// `core.NewItemEffect` over `agent.GetCharacter()`): Idol of the Moon (23197), Idol of Ferocity
/// (22397), Idol of Brutality (23198), Howling Idol (272427) and Idol of the Dream (220606). A character of another class wears them as well, and
/// their spell mods find spells by class mask alone. Under Forever's rules the flat shapeshift
/// cost items join them (`shapeshift_cost_spell`). Answers whether the item is one of them.
pub(crate) fn apply_agentless_idol_effect(sim: &mut Sim, unit: UnitId, item: i32) -> bool {
    match item {
        23197 => {
            // Idol of the Moon.
            let aura = sim.register_aura(
                unit,
                AuraConfig {
                    label: "Improved Moonfire".to_string(),
                    ..AuraConfig::default()
                },
            );
            sim.attach_spell_mod(
                aura,
                SpellModConfig {
                    class_mask: masks::MOONFIRE,
                    kind: SpellModType::BaseDamageFlat,
                    float_value: 33.0,
                    ..SpellModConfig::default()
                },
            );
            sim.make_permanent(aura);
            true
        }
        22397 => {
            // Idol of Ferocity: reduces the energy cost of Claw and Rake by 2.
            sim.add_static_mod(
                unit,
                SpellModConfig {
                    class_mask: masks::CLAW | masks::RAKE,
                    kind: SpellModType::PowerCostFlat,
                    int_value: -2,
                    ..SpellModConfig::default()
                },
            );
            true
        }
        23198 => {
            // Idol of Brutality: reduces the rage cost of Maul, Swipe and Primal Bite by 2.
            sim.add_static_mod(
                unit,
                SpellModConfig {
                    class_mask: masks::MAUL | masks::SWIPE | masks::PRIMAL_BITE,
                    kind: SpellModType::PowerCostFlat,
                    int_value: -2,
                    ..SpellModConfig::default()
                },
            );
            true
        }
        272427 => {
            // Howling Idol: "Reduces the cooldown of your Shifting Power ability by 0.5 sec."
            // (1291059: A_ADD_FLAT_MODIFIER, SPELLMOD_COOLDOWN on Shifting Power's class mask).
            // Stacks with Improved Shifting Power. Go `HowlingIdolFromRow` (patch 179): the
            // reduction is the row's, -500 ms at client 70425, not patch 103's -1000 ms.
            let row = crate::prepare::spelldata::store::must_find(1291059);
            sim.add_static_mod(
                unit,
                SpellModConfig {
                    class_mask: masks::SHIFTING_POWER,
                    kind: SpellModType::CooldownFlat,
                    time_value: (row.effect_n(1).base_value() as i64) * MILLISECOND,
                    ..SpellModConfig::default()
                },
            );
            true
        }
        220606 => {
            // Idol of the Dream: Improved Rip (446212, client 1.60.1.70532: A_ADD_FLAT_MODIFIER,
            // SPELLMOD_DURATION on Rip's class mask, +2000 ms). Rip lasts 2 seconds longer with
            // the same tick period and tick damage, so it ticks once more. Ported from
            // wowsims/forever 8d879a401; Go's pinned engine registers no effect for it.
            let row = crate::prepare::spelldata::store::must_find(446212);
            sim.add_static_mod(
                unit,
                SpellModConfig {
                    class_mask: masks::RIP,
                    kind: SpellModType::DurationFlat,
                    time_value: (row.effect_n(1).base_value() as i64) * MILLISECOND,
                    ..SpellModConfig::default()
                },
            );
            true
        }
        _ => apply_shapeshift_cost_item(sim, unit, item),
    }
}

/// The spell of a Forever druid item that lowers the mana cost of shapeshifting by a flat amount:
/// the Feral PvP grips, Shapeshift Cost Reduction (459594, "Reduces the mana cost of your
/// shapeshifts by 150"), and Idol of Shifting Tides, Clever Shapeshifter (1270470, "Reduces the
/// mana cost of shapeshifting spells by 40"). The item to spell links are the client's
/// ItemXItemEffect rows (wago.tools wowf, 4 November 2026). Go's pinned engine registers neither,
/// so only Forever's rules apply them.
pub(crate) fn shapeshift_cost_spell(item: i32) -> Option<i32> {
    match item {
        // Blood Guard's and Knight-Lieutenant's Dragonhide Grips, General's and Marshal's
        // Dragonhide Grips.
        227180 | 227183 | 231688 | 231694 => Some(459594),
        // Idol of Shifting Tides.
        263411 => Some(1270470),
        _ => None,
    }
}

/// A flat shapeshift cost item under Forever's rules (`shapeshift_cost_spell`). Both spells are an
/// `A_ADD_FLAT_MODIFIER` of `SPELLMOD_COST` on the shapeshift class mask Natural Shapeshifter
/// reads (word 0 bits 29 to 31, the idol also word 1 bit 16), which covers Cat, Bear and Moonkin
/// Form and Shifting Power, whose tooltip says its cost "is reduced by effects that reduce the
/// cost of Shapeshifting". A flat cost modifier comes off the base cost before the percentage
/// ones (`finalize_spell`), as the client applies spell mods: with Natural Shapeshifter 3/3 the
/// grips save 150 x 0.7 = 105 mana a shift.
fn apply_shapeshift_cost_item(sim: &mut Sim, unit: UnitId, item: i32) -> bool {
    if sim.rules != Rules::Forever {
        return false;
    }
    let Some(spell) = shapeshift_cost_spell(item) else {
        return false;
    };
    let row = crate::prepare::spelldata::store::must_find(spell);
    sim.add_static_mod(
        unit,
        SpellModConfig {
            class_mask: masks::CAT_FORM
                | masks::BEAR_FORM
                | masks::MOONKIN_FORM
                | masks::SHIFTING_POWER,
            kind: SpellModType::PowerCostFlat,
            int_value: row.effect_n(1).base_value() as i32,
            ..SpellModConfig::default()
        },
    );
    true
}

impl Druid {
    /// The `core.NewItemEffect` registrations of Go's druid package: the idols above and Wolfshead
    /// Helm (8345), which reads the druid agent. Answers whether the class registers an effect for
    /// the item.
    pub(super) fn apply_idol_effect(&mut self, sim: &mut Sim, unit: UnitId, item: i32) -> bool {
        if apply_agentless_idol_effect(sim, unit, item) {
            return true;
        }
        match item {
            8345 => {
                // Wolfshead Helm: the bonus belongs to Shifting Power and Enrage.
                let gain = Rc::clone(&self.st);
                let expire = Rc::clone(&self.st);
                let aura = sim.register_aura(
                    unit,
                    AuraConfig {
                        label: "Wolfshead Helm".to_string(),
                        action_id: Some(ActionId::spell(17768)),
                        on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                            let energy = gain.wolfshead_shifting_power_energy;
                            sim.set_state(energy, sim.state(energy) + 20.0);
                            let rage = gain.wolfshead_enrage_rage;
                            sim.set_state(rage, sim.state(rage) + 5.0);
                        })),
                        on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                            let energy = expire.wolfshead_shifting_power_energy;
                            sim.set_state(energy, sim.state(energy) - 20.0);
                            let rage = expire.wolfshead_enrage_rage;
                            sim.set_state(rage, sim.state(rage) - 5.0);
                        })),
                        ..AuraConfig::default()
                    },
                );
                sim.make_permanent(aura);
                true
            }
            _ => false,
        }
    }
}

fn stats(pairs: &[(Stat, f64)]) -> Stats {
    Stats::from_pairs(pairs)
}

fn feralheart_2(env: &mut Environment, aura: AuraId) {
    env.sim.attach_stats_buff(
        aura,
        stats(&[
            (Stat::ArcaneResistance, 8.0),
            (Stat::FireResistance, 8.0),
            (Stat::FrostResistance, 8.0),
            (Stat::NatureResistance, 8.0),
            (Stat::ShadowResistance, 8.0),
        ]),
    );
}

fn feralheart_3(env: &mut Environment, aura: AuraId) {
    env.sim.attach_stat_buff(aura, Stat::MP5, 8.0);
}

/// Nature's Bounty: 2% chance to restore 200 mana on spell cast, 20 energy on a white hit, or
/// 10 rage when struck.
fn feralheart_4(env: &mut Environment, aura: AuraId) {
    let action = ActionId::spell(450608);
    env.sim.attach_proc_trigger(
        aura,
        &ProcTrigger {
            action_id: action.clone(),
            name: "Nature's Bounty (Mana)".to_string(),
            callback: CallbackMask::ON_CAST_COMPLETE,
            proc_mask: ProcMask::SPELL_DAMAGE | ProcMask::SPELL_HEALING,
            proc_chance: 0.02,
            ..ProcTrigger::default()
        },
    );
    env.sim.attach_proc_trigger(
        aura,
        &ProcTrigger {
            action_id: action.clone(),
            name: "Nature's Bounty (Energy)".to_string(),
            callback: CallbackMask::ON_SPELL_HIT_DEALT,
            outcome: HitOutcome::LANDED,
            proc_mask: ProcMask::MELEE_WHITE_HIT,
            proc_chance: 0.02,
            ..ProcTrigger::default()
        },
    );
    env.sim.attach_proc_trigger(
        aura,
        &ProcTrigger {
            action_id: action,
            name: "Nature's Bounty (Rage)".to_string(),
            callback: CallbackMask::ON_SPELL_HIT_TAKEN,
            proc_mask: ProcMask::MELEE,
            proc_chance: 0.02,
            ..ProcTrigger::default()
        },
    );
}

fn nothing(_: &mut Environment, _: AuraId) {}

fn feralheart_6(env: &mut Environment, aura: AuraId) {
    env.sim.attach_stats_buff(
        aura,
        stats(&[
            (Stat::SpellDamage, 23.0),
            (Stat::HealingPower, 23.0),
            (Stat::AttackPower, 40.0),
            (Stat::RangedAttackPower, 40.0),
        ]),
    );
}

fn cenarion_5(env: &mut Environment, aura: AuraId) {
    env.sim.attach_stat_buff(
        aura,
        Stat::SpellCritRating,
        2.0 * SPELL_CRIT_RATING_PER_CRIT_PERCENT,
    );
}

fn stormrage_3(env: &mut Environment, aura: AuraId) {
    env.sim.attach_additive_pseudo_stat_buff(
        aura,
        PseudoStatField::Custom(|p| &mut p.spirit_regen_rate_casting),
        0.15,
    );
}

fn symbols_3(env: &mut Environment, aura: AuraId) {
    env.sim.attach_proc_trigger(
        aura,
        &ProcTrigger {
            name: "Symbols of Unending Life Finisher Bonus".to_string(),
            callback: CallbackMask::ON_SPELL_HIT_DEALT,
            outcome: HitOutcome::MISS | HitOutcome::DODGE | HitOutcome::BLOCK | HitOutcome::PARRY,
            class_spell_mask: masks::FEROCIOUS_BITE | masks::RIP,
            ..ProcTrigger::default()
        },
    );
}

fn refuge_2(env: &mut Environment, aura: AuraId) {
    env.sim.attach_stats_buff(
        aura,
        stats(&[(Stat::HealingPower, 44.0), (Stat::SpellDamage, 15.0)]),
    );
}

fn stamina_20(env: &mut Environment, aura: AuraId) {
    env.sim.attach_stat_buff(aura, Stat::Stamina, 20.0);
}

fn sanctuary_6(env: &mut Environment, aura: AuraId) {
    env.sim.attach_stats_buff(
        aura,
        stats(&[(Stat::AttackPower, 40.0), (Stat::RangedAttackPower, 40.0)]),
    );
}

/// Go's druid `core.NewItemSet` calls, in Go's registration order.
pub(crate) static ITEM_SETS: &[ItemSet] = &[
    ItemSet {
        id: 0,
        name: "Feralheart Raiment",
        alternative_name: "",
        bonuses: &[
            (2, feralheart_2),
            (3, feralheart_3),
            (4, feralheart_4),
            (5, nothing),
            (6, feralheart_6),
        ],
        required_profession: "",
    },
    ItemSet {
        id: 0,
        name: "Cenarion Raiment",
        alternative_name: "",
        bonuses: &[(3, nothing), (5, cenarion_5), (8, nothing)],
        required_profession: "",
    },
    ItemSet {
        id: 0,
        name: "Stormrage Raiment",
        alternative_name: "",
        bonuses: &[(3, stormrage_3), (5, nothing), (8, nothing)],
        required_profession: "",
    },
    ItemSet {
        id: 0,
        name: "Symbols of Unending Life",
        alternative_name: "",
        bonuses: &[(3, symbols_3)],
        required_profession: "",
    },
    ItemSet {
        id: 0,
        name: "Champion's Refuge",
        alternative_name: "",
        bonuses: &[(2, refuge_2), (4, nothing), (6, stamina_20)],
        required_profession: "",
    },
    ItemSet {
        id: 0,
        name: "Lieutenant Commander's Refuge",
        alternative_name: "",
        bonuses: &[(2, refuge_2), (4, nothing), (6, stamina_20)],
        required_profession: "",
    },
    ItemSet {
        id: 0,
        name: "Field Marshal's Sanctuary",
        alternative_name: "",
        bonuses: &[(2, stamina_20), (3, nothing), (6, sanctuary_6)],
        required_profession: "",
    },
    ItemSet {
        id: 0,
        name: "Warlord's Sanctuary",
        alternative_name: "",
        bonuses: &[(2, stamina_20), (3, nothing), (6, sanctuary_6)],
        required_profession: "",
    },
];
