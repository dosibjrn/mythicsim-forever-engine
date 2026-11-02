//! Go sim/common/classic: the weapons items_weapons.go registers, Dragon's Call from
//! emerald_dragon_whelp.go, and (in `classic_enchants.rs`) the enchants of enchants.go.
//!
//! Go `init` registers them in file order, which only matters between IDs the registry would
//! reject twice; every ID here is distinct from the forever package's.
//!
//! Each weapon's proc aura registers with the item swap as Go's `ItemSwap.RegisterProc` does, so a
//! swap turns it on and off with the weapon.

use std::rc::Rc;

use crate::contracts::prepared_v2::ActionId;

use super::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use super::env::Environment;
use super::itemhelpers::{create_weapon_proc_spell, create_weapon_proc_trigger, WeaponProcTrigger};
use super::sim::{AuraConfig, AuraId, Cooldown, Sim, UnitId, MILLISECOND, SECOND};
use super::spell::{school, CastConfig, DefenseType, DotConfig, ProcMask, SpellConfig, SpellFlag};
use super::stats::{Stat, Stats};
use super::Refusal;

/// The Dragon's Call item ID: `classic.DragonsCall`.
pub(crate) const DRAGONS_CALL: i32 = 10847;

/// Go `NewItemEffect` for the IDs sim/common/classic registers: applies the effect to the player
/// and answers true, or answers false for an ID it registers nothing for.
pub(crate) fn apply_item_effect(env: &mut Environment, item: i32) -> Result<bool, Refusal> {
    match item {
        19019 => thunderfury(env),
        DRAGONS_CALL => dragons_call(env),
        11684 => ironfoe(env),
        13505 => runeblade_of_baron_rivendare(env),
        13937 => headmasters_charge(env),
        14576 => ebon_hilt_of_marduk(env),
        17182 => sulfuras_hand_of_ragnaros(env),
        _ => return Ok(false),
    }
    Ok(true)
}

/// Go `NewEnemyAuraArray`: one aura per enemy, in unit order.
pub(crate) fn new_enemy_aura_array(
    sim: &mut Sim,
    mut make_aura: impl FnMut(&mut Sim, UnitId) -> AuraId,
) -> Vec<Option<AuraId>> {
    let units = sim.all_units();
    let mut auras = vec![None; units.len()];
    for target in units {
        if sim.unit(target).unit_type == super::sim::UnitType::Enemy {
            let index = sim.unit(target).unit_index as usize;
            auras[index] = Some(make_aura(sim, target));
        }
    }
    auras
}

/// Go `NewPassiveMovementSpeedAura`.
fn new_passive_movement_speed_aura(
    sim: &mut Sim,
    unit: UnitId,
    label: &str,
    action_id: ActionId,
    multiplier: f64,
) -> AuraId {
    let aura = sim.get_or_register_aura(
        unit,
        AuraConfig {
            label: label.to_string(),
            action_id: Some(action_id),
            ..AuraConfig::default()
        },
    );
    sim.make_permanent(aura);
    sim.new_passive_movement_speed_effect(aura, multiplier);
    aura
}

/// Thunderfury, Blessed Blade of the Windseeker: a weapon proc at 6 PPM that casts a nature hit
/// that slows the target's attacks and a bounce that lowers its nature resistance.
fn thunderfury(env: &mut Environment) {
    create_weapon_proc_trigger(
        env,
        &WeaponProcTrigger {
            item_id: 19019,
            name: "Thunderfury",
            ppm: 6.0,
            ..WeaponProcTrigger::default()
        },
        |env| {
            let unit = env.player;
            let proc_action = ActionId::spell(21992);

            let attack_speed_debuff = new_enemy_aura_array(&mut env.sim, |sim, target| {
                let aura = sim.get_or_register_aura(
                    target,
                    AuraConfig {
                        label: "Cyclone".to_string(),
                        action_id: Some(ActionId::spell(27648)),
                        duration: 12 * SECOND,
                        ..AuraConfig::default()
                    },
                );
                sim.atk_speed_reduction_effect(
                    aura,
                    super::shared_auras::slowed_time_multiplier(-20.0),
                );
                aura
            });
            // Go keeps the arrays for the handler, which only runs in a fight.
            let _ = attack_speed_debuff;

            env.sim.register_spell(
                unit,
                SpellConfig {
                    action_id: tagged(&proc_action, 1),
                    spell_school: school::NATURE,
                    defense_type: DefenseType::Magic,
                    proc_mask: ProcMask::SPELL_DAMAGE,
                    flags: SpellFlag::PROC,
                    damage_multiplier: 1.0,
                    threat_multiplier: 0.5,
                    ..SpellConfig::default()
                },
            );

            let resistance_debuff = new_enemy_aura_array(&mut env.sim, |sim, target| {
                sim.get_or_register_aura(
                    target,
                    AuraConfig {
                        label: "Thunderfury".to_string(),
                        action_id: Some(proc_action.clone()),
                        duration: 12 * SECOND,
                        on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                            sim.add_stat_dynamic(target, Stat::NatureResistance, -25.0);
                        })),
                        on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                            sim.add_stat_dynamic(target, Stat::NatureResistance, 25.0);
                        })),
                        ..AuraConfig::default()
                    },
                )
            });
            let _ = resistance_debuff;

            env.sim.register_spell(
                unit,
                SpellConfig {
                    action_id: tagged(&proc_action, 2),
                    spell_school: school::NATURE,
                    defense_type: DefenseType::Magic,
                    proc_mask: ProcMask::EMPTY,
                    threat_multiplier: 1.0,
                    flat_threat_bonus: 63.0,
                    ..SpellConfig::default()
                },
            );
            true
        },
    );
}

/// An action ID with a tag: Go `ActionID.WithTag`.
pub(crate) fn tagged(id: &ActionId, tag: i32) -> ActionId {
    let mut tagged = id.clone();
    tagged.tag = tag;
    tagged
}

/// Dragon's Call: a weapon proc at 1 PPM, behind the cooldown of the summon's category, that
/// summons the Emerald Dragon Whelp. A character without the whelp (the pet exists only for a
/// Dragon's Call equipped in a hand at the start) opts out of the proc.
fn dragons_call(env: &mut Environment) {
    let icd = super::spelldata::must_find(13049).category_cooldown();
    create_weapon_proc_trigger(
        env,
        &WeaponProcTrigger {
            item_id: DRAGONS_CALL,
            name: "Emerald Dragon Whelp",
            ppm: 1.0,
            icd,
            ..WeaponProcTrigger::default()
        },
        |env| super::classic_whelp::has_whelp(&env.sim, env.player),
    );
}

/// Ironfoe: the equip Fury of Forgewright, a 6% chance on a landed melee hit, behind a 100 ms
/// cooldown, to grant two extra main hand attacks.
fn ironfoe(env: &mut Environment) {
    let unit = env.player;
    let aura = env.sim.make_proc_trigger_aura(
        unit,
        &ProcTrigger {
            name: "Fury of Forgewright".to_string(),
            action_id: ActionId::spell(15494),
            callback: CallbackMask::ON_SPELL_HIT_DEALT,
            proc_mask: ProcMask::MELEE,
            outcome: HitOutcome::LANDED,
            proc_chance: 0.06,
            icd: 100 * MILLISECOND,
            trigger_immediately: true,
            ..ProcTrigger::default()
        },
    );
    env.sim.register_item_proc(unit, 11684, aura);
}

/// Runeblade of Baron Rivendare: Unholy Aura, 8% movement speed, and 60 health every 5 sec.
fn runeblade_of_baron_rivendare(env: &mut Environment) {
    let unit = env.player;
    let action_id = ActionId::spell(17625);
    let aura = new_passive_movement_speed_aura(&mut env.sim, unit, "Unholy Aura", action_id, 0.08);
    // The regeneration is a periodic action a fight runs; the callbacks are what the reset sees.
    env.sim.apply_on_gain(
        aura,
        Rc::new(|sim: &mut Sim, _| {
            sim.start_periodic_action(super::periodic_action::PeriodicActionOptions {
                period: 5 * SECOND,
                ..Default::default()
            });
        }),
    );
    env.sim.apply_on_expire(aura, Rc::new(|_: &mut Sim, _| {}));
    env.sim.register_item_proc(unit, 13505, aura);
}

/// Headmaster's Charge: use, 20 Intellect for 15 min on a 10 min cooldown.
fn headmasters_charge(env: &mut Environment) {
    let unit = env.player;
    let timer = env.sim.new_timer(unit);
    env.sim.register_temporary_stats_on_use_cd(
        unit,
        "Headmaster's Charge",
        Stats::from_pairs(&[(Stat::Intellect, 20.0)]),
        15 * 60 * SECOND,
        SpellConfig {
            action_id: ActionId::item(13937),
            cast: CastConfig {
                cd: Cooldown {
                    timer: Some(timer),
                    duration: 10 * 60 * SECOND,
                },
                ..CastConfig::default()
            },
            ..SpellConfig::default()
        },
    );
}

/// Ebon Hilt of Marduk: a 1 PPM chance on hit for Corruption, 28 Shadow every 3 sec for 9 sec,
/// and the equip that takes 1% off the wearer's threat.
fn ebon_hilt_of_marduk(env: &mut Environment) {
    create_weapon_proc_spell(env, 14576, "Ebon Hilt of Marduk", 1.0, |env| {
        let unit = env.player;
        let threat_aura = env.sim.register_aura(
            unit,
            AuraConfig {
                label: "Decrease Threat All 01".to_string(),
                action_id: Some(ActionId::spell(1298501)),
                ..AuraConfig::default()
            },
        );
        env.sim.make_permanent(threat_aura);
        env.sim.attach_multiplicative_pseudo_stat_buff(
            threat_aura,
            super::aura_helpers::PseudoStatField::ThreatMultiplier,
            0.99,
        );

        Some(env.sim.get_or_register_spell(
            unit,
            SpellConfig {
                action_id: ActionId::spell(18656),
                spell_school: school::SHADOW,
                defense_type: DefenseType::Magic,
                proc_mask: ProcMask::EMPTY,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                dot: DotConfig {
                    aura: AuraConfig {
                        label: "Corruption (Ebon Hilt of Marduk)".to_string(),
                        ..AuraConfig::default()
                    },
                    tick_length: 3 * SECOND,
                    number_of_ticks: 3,
                    ..DotConfig::default()
                },
                ..SpellConfig::default()
            },
        ))
    });
}

/// Sulfuras, Hand of Ragnaros: a 1 PPM chance on hit for Fireball, 273 to 333 Fire plus 15 every
/// 2 sec for 10 sec, and the equip Immolation, 5 Fire to every melee attacker.
fn sulfuras_hand_of_ragnaros(env: &mut Environment) {
    create_weapon_proc_spell(env, 17182, "Sulfuras, Hand of Ragnaros", 1.0, |env| {
        let unit = env.player;
        env.sim.register_spell(
            unit,
            SpellConfig {
                action_id: ActionId::spell(21142),
                spell_school: school::FIRE,
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::PASSIVE_SPELL,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        );
        env.sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Immolation (Hand of Ragnaros)".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_TAKEN,
                proc_mask: ProcMask::MELEE,
                outcome: HitOutcome::LANDED,
                ..ProcTrigger::default()
            },
        );

        Some(env.sim.get_or_register_spell(
            unit,
            SpellConfig {
                action_id: ActionId::spell(21162),
                spell_school: school::FIRE,
                defense_type: DefenseType::Magic,
                proc_mask: ProcMask::EMPTY,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                dot: DotConfig {
                    aura: AuraConfig {
                        label: "Fireball (Hand of Ragnaros)".to_string(),
                        ..AuraConfig::default()
                    },
                    tick_length: 2 * SECOND,
                    number_of_ticks: 5,
                    ..DotConfig::default()
                },
                ..SpellConfig::default()
            },
        ))
    });
}

/// The item IDs this module registers, for the test against Go's registered IDs.
#[cfg(test)]
const ITEMS: [i32; 7] = [19019, DRAGONS_CALL, 11684, 13505, 13937, 14576, 17182];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepare::item_test_support::{aura, environment};

    /// The items of a request that wears `item` in the main hand and `enchant` on it.
    fn main_hand(item: i32, enchant: i32) -> Vec<(i32, i32)> {
        let mut items = vec![(0, 0); 14];
        items.push((item, enchant));
        items
    }

    #[test]
    fn every_item_is_one_go_registers() {
        let tables = crate::data::tables::tables();
        for item in ITEMS {
            assert!(
                tables.item_effect_ids.contains(&item),
                "item {item} is not registered by Go"
            );
        }
        for enchant in [2613, 2621, super::super::classic_enchants::CRUSADER] {
            assert!(
                tables.enchant_effect_ids.contains(&enchant),
                "enchant {enchant} is not registered by Go"
            );
        }
    }

    #[test]
    fn ironfoes_fury_of_forgewright_hears_landed_melee_hits_behind_a_100_ms_cooldown() {
        let env = environment(&main_hand(11684, 0));
        let aura = env.sim.aura(aura(&env, "Fury of Forgewright"));
        assert_eq!(aura.action_id_for_proc, Some(ActionId::spell(15494)));
        assert_eq!(aura.icd.map(|icd| icd.duration), Some(100 * MILLISECOND));
        assert_eq!(
            aura.callback_names(),
            vec!["on_reset", "on_spell_hit_dealt"]
        );
    }

    #[test]
    fn crusader_keeps_a_strength_buff_for_each_hand_and_one_trigger() {
        let env = environment(&main_hand(35, 1900));
        for (label, tag) in [("Holy Strength (MH)", 1), ("Holy Strength (OH)", 2)] {
            let buff = env.sim.aura(aura(&env, label));
            assert_eq!(buff.action_id, Some(tagged(&ActionId::spell(20007), tag)));
            assert_eq!(buff.duration, 15 * SECOND);
        }
        let trigger = env.sim.aura(aura(&env, "Enchant Weapon - Crusader"));
        assert_eq!(trigger.action_id_for_proc, Some(ActionId::spell(20007)));
        let dpm = trigger.dpm.as_ref().expect("a proc manager");
        assert_eq!(dpm.proc_masks, vec![ProcMask::MELEE_MH]);
    }

    #[test]
    fn the_threat_enchants_scale_threat_through_permanent_auras() {
        let mut items = vec![(0, 0); 15];
        items[6] = (14615, 2613);
        items[3] = (13340, 2621);
        let env = environment(&items);
        assert!(env.sim.aura(aura(&env, "Increase Threat")).active);
        assert!(env.sim.aura(aura(&env, "Decrease Threat")).active);
        let threat = env.sim.unit(env.player).pseudo_stats.threat_multiplier;
        assert_eq!(threat, 1.0 * 1.02 * 0.98);
    }

    #[test]
    fn the_runeblade_raises_movement_speed_by_eight_percent() {
        let env = environment(&main_hand(13505, 0));
        assert!(env.sim.aura(aura(&env, "Unholy Aura")).active);
        let speed = env
            .sim
            .unit(env.player)
            .pseudo_stats
            .movement_speed_multiplier;
        assert_eq!(speed, 1.08);
    }
}
