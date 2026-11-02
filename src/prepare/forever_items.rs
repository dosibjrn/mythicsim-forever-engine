//! Go sim/common/forever: the item and enchant effects the shared package registers. The
//! hand-written ones (items_trinkets.go, items_weapons.go, enchants.go) come first, as Go's `init`
//! registers them before `RegisterAllEffects` writes the generated ones, and an ID a hand-written
//! effect covers is skipped by the generated registrations: the soft fail the constructors
//! share. The generated tables are in `forever_items_generated.rs`.
//!
//! Item sets (items_sets.go, item_sets_classic.go) and the classic package's effects are not
//! ported here; an equipped ID of theirs is refused by `item_effects`.

use std::rc::Rc;

use crate::contracts::prepared_v2::ActionId;

use super::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger, StackingStatAura};
use super::character::{cooldown_type, MajorCooldown};
use super::env::Environment;
use super::forever_items_generated::{CHANCE_ON_HIT_ITEMS, ENCHANTS, ITEMS};
use super::itemhelpers::{self, WeaponProcTrigger};
use super::parse_effects::{parse_effects, ParseOptions};
use super::periodic_action::PeriodicActionOptions;
use super::shared_items::{
    self, dynamic_legacy_proc_for_enchant, expand_variants, spell_data_proc_damage_spell_config,
    ItemVariant, ProcKind, SpellDataProc,
};
use super::shared_on_use::{self, OnUseKind, StackingStatBonusCd};
use super::sim::{AuraConfig, AuraId, Cooldown, Sim, SpellId, UnitType, NEVER_EXPIRES, SECOND};
use super::spell::{school, CastConfig, DefenseType, ProcMask, SpellConfig, SpellFlag};
use super::spelldata::must_find;
use super::stats::{Stat, Stats};
use super::Refusal;

/// One call of a shared constructor in a generated file.
#[derive(Clone, Copy)]
pub(crate) enum Registration {
    /// A constructor taking an item and registering its on-use effect.
    OnUse(OnUseKind, i32),
    /// `NewStackingStatBonusCD`.
    Stacking(StackingStatBonusCd),
    /// A spell data proc constructor with the items that carry it, or an enchant.
    Proc(ProcKind, SpellDataProc, &'static [ItemVariant]),
}

/// Go `NewItemEffect` for the IDs this package registers: applies the effect to the player and
/// answers true, or answers false for an ID it registers nothing for.
pub(crate) fn apply_item_effect(env: &mut Environment, item: i32) -> Result<bool, Refusal> {
    if apply_hand_written_item(env, item)? {
        return Ok(true);
    }
    for registration in ITEMS {
        match registration {
            Registration::OnUse(kind, id) if *id == item => {
                if shared_on_use::registers(*kind, *id) {
                    shared_on_use::apply(env, *kind, *id);
                    return Ok(true);
                }
            }
            Registration::Stacking(config) if config.id == item => {
                shared_on_use::apply_stacking_stat_bonus_cd(env, config);
                return Ok(true);
            }
            Registration::Proc(kind, cfg, variants) => {
                for expanded in expand_variants(*cfg, variants) {
                    if expanded.item_id == item && shared_items::registers(*kind, &expanded) {
                        shared_items::apply(env, *kind, &expanded);
                        return Ok(true);
                    }
                }
            }
            _ => {}
        }
    }
    if env.sim.rules == super::sim::Rules::Forever {
        return Ok(apply_chance_on_hit_item(env, item));
    }
    Ok(false)
}

/// Under Forever's rules: the chance on hit of an item the reference leaves unregistered for
/// want of a stated rate or a constructor (`CHANCE_ON_HIT_ITEMS`), at the rate MythicSim assumes
/// ([`shared_items::CHANCE_ON_HIT_PPM`]). Answers whether it registered one.
fn apply_chance_on_hit_item(env: &mut Environment, item: i32) -> bool {
    for (kind, cfg) in chance_on_hit_registrations(item) {
        if shared_items::registers(kind, &cfg) {
            shared_items::apply(env, kind, &cfg);
            if matches!(kind, ProcKind::Proc | ProcKind::Aura) {
                chance_on_hit_defense(env, &cfg);
            }
            return true;
        }
    }
    false
}

/// The client's Defense skill aura (`A_MOD_SKILL` of skill line 95), which the effect parser
/// leaves out as the reference's does.
const SKILL_LINE_DEFENSE: i32 = 95;

/// A chance on hit buff's Defense skill, which the parsed aura leaves out: Quel'Serrar's Sanctuary
/// (22850) grants 13 Defense beside its Armor and Attack Power. A point of Defense skill is
/// `DEFENSE_RATING_PER_DEFENSE_LEVEL` of the Defense stat, as the item sets' Defense bonuses add it.
/// The target's swings at a tank read it through the swing rolls, and a Goblin Sapper Charge's
/// hit on its thrower through the crit taken the stat aura combinations carry.
fn chance_on_hit_defense(env: &mut Environment, cfg: &SpellDataProc) {
    use super::character::constants::DEFENSE_RATING_PER_DEFENSE_LEVEL;
    let (_, row) = cfg.rows();
    let level = env.sim.unit(env.player).level;
    let defense: f64 = row
        .effects
        .iter()
        .filter(|effect| {
            effect.aura == super::dbcenums::A_MOD_SKILL && effect.misc == SKILL_LINE_DEFENSE
        })
        .map(|effect| effect.average(level))
        .sum();
    let label = format!("{} Proc", cfg.name);
    let Some(aura) = env.sim.get_aura(env.player, &label) else {
        return;
    };
    if defense != 0.0 {
        env.sim.attach_stat_buff(
            aura,
            Stat::DefenseRating,
            defense * DEFENSE_RATING_PER_DEFENSE_LEVEL,
        );
    }
}

/// The Forever chance on hit registrations of an item, expanded to its variant.
pub(crate) fn chance_on_hit_registrations(item: i32) -> Vec<(ProcKind, SpellDataProc)> {
    let mut found = Vec::new();
    for registration in CHANCE_ON_HIT_ITEMS {
        if let Registration::Proc(kind, cfg, variants) = registration {
            for expanded in expand_variants(*cfg, variants) {
                if expanded.item_id == item {
                    found.push((*kind, expanded));
                }
            }
        }
    }
    found
}

/// Whether Forever's rules register a chance on hit for an item the reference has no effect for.
pub(crate) fn has_chance_on_hit_item(item: i32) -> bool {
    chance_on_hit_registrations(item)
        .iter()
        .any(|(kind, cfg)| shared_items::registers(*kind, cfg))
}

/// Go `NewEnchantEffect` for the IDs this package registers.
pub(crate) fn apply_enchant_effect(env: &mut Environment, enchant: i32) -> Result<bool, Refusal> {
    if apply_hand_written_enchant(env, enchant) {
        return Ok(true);
    }
    for registration in ENCHANTS {
        if let Registration::Proc(kind, cfg, variants) = registration {
            for expanded in expand_variants(*cfg, variants) {
                if expanded.enchant_id == enchant && shared_items::registers(*kind, &expanded) {
                    shared_items::apply(env, *kind, &expanded);
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// The item and enchant IDs these registrations cover, for the test that compares them with the
/// IDs Go registers.
#[cfg(test)]
pub(crate) fn registered_ids() -> (Vec<i32>, Vec<i32>) {
    let mut items: Vec<i32> = HAND_WRITTEN_ITEMS.to_vec();
    items.extend(WEAPON_DAMAGE_PROCS.iter().map(|proc| proc.item_id));
    let mut enchants: Vec<i32> = HAND_WRITTEN_ENCHANTS.to_vec();
    for (registrations, ids) in [(ITEMS, &mut items), (ENCHANTS, &mut enchants)] {
        for registration in registrations {
            match registration {
                Registration::OnUse(kind, id) => {
                    if shared_on_use::registers(*kind, *id) {
                        ids.push(*id);
                    }
                }
                Registration::Stacking(config) => ids.push(config.id),
                Registration::Proc(kind, cfg, variants) => {
                    for expanded in expand_variants(*cfg, variants) {
                        if shared_items::registers(*kind, &expanded) {
                            ids.push(if expanded.enchant_id != 0 {
                                expanded.enchant_id
                            } else {
                                expanded.item_id
                            });
                        }
                    }
                }
            }
        }
    }
    items.sort_unstable();
    items.dedup();
    enchants.sort_unstable();
    enchants.dedup();
    (items, enchants)
}

// ---------------------------------------------------------------------------------------------
// items_trinkets.go and items_weapons.go.
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
const HAND_WRITTEN_ITEMS: [i32; 13] = [
    11815, 23570, 23206, 23207, 19324, 17076, 13204, 13286, 871, 6622, 13246, 2801, 5028,
];
#[cfg(test)]
const HAND_WRITTEN_ENCHANTS: [i32; 8] = [30, 32, 33, 663, 664, 803, 1898, 8217];

/// The weapons that proc a damage spell of the client's rows: items_weapons.go's table of
/// `CreateWeaponProcSpell` calls over `SpellDataProcDamageSpell`.
struct WeaponDamageProc {
    item_id: i32,
    name: &'static str,
    ppm: f64,
    spell_id: i32,
}

const WEAPON_DAMAGE_PROCS: [WeaponDamageProc; 17] = [
    WeaponDamageProc {
        item_id: 272999,
        name: "Barbaric Crossbow",
        ppm: 3.2,
        spell_id: 1291551,
    },
    WeaponDamageProc {
        item_id: 279876,
        name: "Plaguefang",
        ppm: 2.4,
        spell_id: 1309315,
    },
    WeaponDamageProc {
        item_id: 267369,
        name: "Wolfsbane",
        ppm: 2.7,
        spell_id: 1282503,
    },
    WeaponDamageProc {
        item_id: 6469,
        name: "Venomstrike",
        ppm: 1.6,
        spell_id: 29653,
    },
    WeaponDamageProc {
        item_id: 6472,
        name: "Stinging Viper",
        ppm: 3.2,
        spell_id: 1291663,
    },
    WeaponDamageProc {
        item_id: 14555,
        name: "Alcor's Sunrazor",
        ppm: 1.0,
        spell_id: 18833,
    },
    WeaponDamageProc {
        item_id: 11744,
        name: "Bloodfist",
        ppm: 4.0,
        spell_id: 16433,
    },
    WeaponDamageProc {
        item_id: 14487,
        name: "Bonechill Hammer",
        ppm: 1.0,
        spell_id: 18276,
    },
    WeaponDamageProc {
        item_id: 13984,
        name: "Darrowspike",
        ppm: 1.0,
        spell_id: 18276,
    },
    WeaponDamageProc {
        item_id: 10761,
        name: "Coldrage Dagger",
        ppm: 2.2,
        spell_id: 1293790,
    },
    WeaponDamageProc {
        item_id: 19099,
        name: "Glacial Blade",
        ppm: 1.4,
        spell_id: 18398,
    },
    WeaponDamageProc {
        item_id: 11809,
        name: "Flame Wrath",
        ppm: 1.0,
        spell_id: 16559,
    },
    WeaponDamageProc {
        item_id: 12794,
        name: "Masterwork Stormhammer",
        ppm: 0.5,
        spell_id: 16921,
    },
    WeaponDamageProc {
        item_id: 19100,
        name: "Electrified Dagger",
        ppm: 1.4,
        spell_id: 23592,
    },
    WeaponDamageProc {
        item_id: 17074,
        name: "Shadowstrike",
        ppm: 2.2,
        spell_id: 21170,
    },
    WeaponDamageProc {
        item_id: 13401,
        name: "The Cruel Hand of Timmy",
        ppm: 0.65,
        spell_id: 17505,
    },
    WeaponDamageProc {
        item_id: 13361,
        name: "Skullforge Reaver",
        ppm: 1.7,
        spell_id: 17484,
    },
];

/// items_trinkets.go Mark of the Champion: a permanent aura, which an item swap toggles, whose
/// gain and expiry add and take away the attack tables' bonus against Undead and Demons.
fn mark_of_the_champion(env: &mut Environment, item: i32) {
    let unit = env.player;
    let label = if item == 23206 {
        "Mark of the Champion (Physical)"
    } else {
        "Mark of the Champion (Spell)"
    };
    let aura = env.sim.register_aura(
        unit,
        super::sim::AuraConfig {
            label: label.to_string(),
            action_id: Some(ActionId::item(item)),
            ..super::sim::AuraConfig::default()
        },
    );
    let aura = env.sim.make_permanent(aura);
    env.sim
        .apply_on_gain(aura, Rc::new(|_: &mut super::sim::Sim, _| {}));
    env.sim
        .apply_on_expire(aura, Rc::new(|_: &mut super::sim::Sim, _| {}));
    env.sim.register_item_proc(unit, item, aura);
}

fn apply_hand_written_item(env: &mut Environment, item: i32) -> Result<bool, Refusal> {
    match item {
        11815 => hand_of_justice(env),
        23570 => jom_gabbar(env),
        // The permanent aura raises the attack tables' mob type bonus stats against Undead and
        // Demons, which Go's exporter lists as unrepresented. Under Forever's rules the export
        // describes the bonus (`mob_type_bonus`) against targets of those types, and a target
        // of another type never takes it (Go reads the target's own type).
        23206 | 23207 => {
            if env.sim.rules != super::sim::Rules::Forever {
                return Err(Refusal::new(
                    "item_effect",
                    format!("item {item} adds mob type bonus stats, which are not prepared"),
                ));
            }
            mark_of_the_champion(env, item);
        }
        2801 => blade_of_hanna(env),
        // items_70425.go: Lord Sakrasis' Scepter's Holy Smite states no rate, so Go registers it
        // with no effect, which keeps the generator from reading it as a proc on every cast.
        5028 => {}
        19324 => lobotomizer(env),
        17076 => bonereavers_edge(env),
        13204 => puncture_armor(env, 13204, "Bashguuder"),
        13286 => puncture_armor(env, 13286, "Rivenspike"),
        871 => flurry_axe(env),
        6622 => weapon_proc_aura(env, 6622, "Sword of Zeal", 1.8, 8191),
        13246 => weapon_proc_aura(env, 13246, "Argent Avenger", 1.0, 17352),
        _ => {
            let Some(proc) = WEAPON_DAMAGE_PROCS.iter().find(|proc| proc.item_id == item) else {
                return Ok(false);
            };
            weapon_damage_proc(env, proc);
        }
    }
    Ok(true)
}

/// Go `itemhelpers.CreateWeaponProcTrigger` for a "Chance on hit" proc.
fn create_weapon_proc_trigger(
    env: &mut Environment,
    item_id: i32,
    name: &'static str,
    ppm: f64,
    trigger_immediately: bool,
    setup: impl FnOnce(&mut Environment) -> bool,
) {
    itemhelpers::create_weapon_proc_trigger(
        env,
        &WeaponProcTrigger {
            item_id,
            name,
            ppm,
            trigger_immediately,
            ..WeaponProcTrigger::default()
        },
        setup,
    );
}

/// Go `itemhelpers.CreateWeaponProcSpell`.
fn create_weapon_proc_spell(
    env: &mut Environment,
    item_id: i32,
    name: &'static str,
    ppm: f64,
    spell: impl FnOnce(&mut Environment) -> SpellId,
) {
    itemhelpers::create_weapon_proc_spell(env, item_id, name, ppm, |env| Some(spell(env)));
}

fn weapon_damage_proc(env: &mut Environment, proc: &WeaponDamageProc) {
    create_weapon_proc_spell(env, proc.item_id, proc.name, proc.ppm, |env| {
        let unit = env.player;
        let config = spell_data_proc_damage_spell_config(env, must_find(proc.spell_id));
        env.sim.get_or_register_spell(unit, config)
    });
}

/// items_weapons.go The Lobotomizer.
fn lobotomizer(env: &mut Environment) {
    create_weapon_proc_spell(env, 19324, "The Lobotomizer", 0.4, |env| {
        let unit = env.player;
        env.sim.get_or_register_spell(
            unit,
            SpellConfig {
                action_id: ActionId::spell(1290950),
                proc_mask: ProcMask::EMPTY,
                spell_school: school::PHYSICAL,
                defense_type: DefenseType::Melee,
                flags: SpellFlag::PASSIVE_SPELL,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        )
    });
}

/// items_70425.go Blade of Hanna (2801): each landed main hand swing adds a stack of Improving
/// Form (1325509, up to 30); at 30 the stacks go, Perfected Form (1325508) strikes the target for
/// its 490 Physical damage on the melee special table and raises every stat by 20 for 20 sec.
fn blade_of_hanna(env: &mut Environment) {
    let unit = env.player;
    let perfected = super::spelldata::must_find(1325508);
    let improving = super::spelldata::must_find(1325509);
    let all_stats = perfected.effect_n(2).base_points;
    let buff = env.sim.register_aura(
        unit,
        AuraConfig {
            label: "Perfected Form".to_string(),
            action_id: Some(ActionId::spell(perfected.id)),
            duration: perfected.duration(),
            ..AuraConfig::default()
        },
    );
    env.sim.attach_stats_buff(
        buff,
        Stats::from_pairs(&[
            (Stat::Strength, all_stats),
            (Stat::Agility, all_stats),
            (Stat::Stamina, all_stats),
            (Stat::Intellect, all_stats),
            (Stat::Spirit, all_stats),
        ]),
    );
    env.sim.register_spell(
        unit,
        SpellConfig {
            action_id: ActionId::spell(perfected.id),
            spell_school: school::PHYSICAL,
            defense_type: DefenseType::Melee,
            proc_mask: ProcMask::EMPTY,
            flags: SpellFlag::PASSIVE_SPELL,
            damage_multiplier: 1.0,
            threat_multiplier: 1.0,
            ..SpellConfig::default()
        },
    );
    env.sim.register_aura(
        unit,
        AuraConfig {
            label: "Improving Form".to_string(),
            action_id: Some(ActionId::spell(improving.id)),
            duration: NEVER_EXPIRES,
            max_stacks: i32::from(improving.max_stack),
            ..AuraConfig::default()
        },
    );
    let trigger = env.sim.make_proc_trigger_aura(
        unit,
        &ProcTrigger {
            name: "Blade of Hanna".to_string(),
            callback: CallbackMask::ON_SPELL_HIT_DEALT,
            proc_mask: ProcMask::MELEE_MH_AUTO,
            outcome: HitOutcome::LANDED,
            trigger_immediately: true,
            ..ProcTrigger::default()
        },
    );
    // Go registers no item swap callback, so its stacks keep counting whatever weapon the hand
    // holds. Under Forever's rules the trigger is up while the hand holds the blade, as every
    // other weapon proc's is (Go `ItemSwap.RegisterProc`).
    if env.sim.rules == super::sim::Rules::Forever {
        env.sim.register_item_proc(unit, 2801, trigger);
    }
}

/// items_weapons.go Flurry Axe.
fn flurry_axe(env: &mut Environment) {
    create_weapon_proc_spell(env, 871, "Flurry Axe", 1.9, |env| {
        let unit = env.player;
        env.sim.get_or_register_spell(
            unit,
            SpellConfig {
                action_id: ActionId::spell(18797),
                spell_school: school::PHYSICAL,
                proc_mask: ProcMask::EMPTY,
                ..SpellConfig::default()
            },
        )
    });
}

/// items_weapons.go Bonereaver's Edge.
fn bonereavers_edge(env: &mut Environment) {
    create_weapon_proc_trigger(env, 17076, "Bonereaver's Edge", 2.0, false, |env| {
        let unit = env.player;
        env.sim.make_stacking_aura(
            unit,
            StackingStatAura {
                aura: AuraConfig {
                    label: "Bonereaver's Edge".to_string(),
                    action_id: Some(ActionId::spell(21153)),
                    duration: 10 * SECOND,
                    max_stacks: 3,
                    ..AuraConfig::default()
                },
                bonus_per_stack: Stats::from_pairs(&[(Stat::ArmorPenetration, 700.0)]),
            },
        );
        true
    });
}

/// items_weapons.go Bashguuder and Rivenspike: each stack of Puncture Armor on the target
/// takes 100 armor.
fn puncture_armor(env: &mut Environment, item_id: i32, name: &'static str) {
    create_weapon_proc_trigger(env, item_id, name, 2.0, false, |env| {
        for target in env.sim.all_units() {
            if env.sim.unit(target).unit_type != UnitType::Enemy {
                continue;
            }
            env.sim.get_or_register_aura(
                target,
                AuraConfig {
                    label: "Puncture Armor".to_string(),
                    action_id: Some(ActionId::spell(17315)),
                    duration: 30 * SECOND,
                    max_stacks: 3,
                    on_stacks_change: Some(Rc::new(
                        move |sim: &mut Sim, aura: AuraId, old: i32, new: i32| {
                            let unit = sim.aura(aura).unit;
                            sim.add_stat_dynamic(unit, Stat::Armor, -100.0 * f64::from(new - old));
                        },
                    )),
                    ..AuraConfig::default()
                },
            );
        }
        true
    });
}

/// Go `itemhelpers.CreateWeaponProcAura`: a "Chance on hit" weapon proc that activates a
/// custom aura on the wearer: the row's effects parsed onto it.
fn weapon_proc_aura(
    env: &mut Environment,
    item_id: i32,
    name: &'static str,
    ppm: f64,
    spell_id: i32,
) {
    itemhelpers::add_weapon_proc_aura(env, item_id, name, ppm, |env| {
        let unit = env.player;
        let row = must_find(spell_id);
        let aura = env.sim.get_or_register_aura(
            unit,
            AuraConfig {
                label: name.to_string(),
                action_id: Some(ActionId::spell(spell_id)),
                duration: row.duration(),
                ..AuraConfig::default()
            },
        );
        parse_effects(&mut env.sim, Some(unit), aura, row, ParseOptions::default());
        aura
    });
}

/// items_trinkets.go Hand of Justice.
fn hand_of_justice(env: &mut Environment) {
    let unit = env.player;
    env.sim.make_proc_trigger_aura(
        unit,
        &ProcTrigger {
            name: "Hand of Justice".to_string(),
            action_id: ActionId::spell(15600),
            callback: CallbackMask::ON_SPELL_HIT_DEALT,
            proc_mask: ProcMask::MELEE,
            outcome: HitOutcome::LANDED,
            proc_chance: 0.01,
            icd: 2 * SECOND,
            trigger_immediately: true,
            ..ProcTrigger::default()
        },
    );
}

/// items_trinkets.go Jom Gabbar: ten stacks of 65 attack power and ranged attack power, one
/// every two seconds for the 20 second duration.
fn jom_gabbar(env: &mut Environment) {
    let unit = env.player;
    let action_id = ActionId::spell(29602);
    let duration = 20 * SECOND;
    let bonus_per_stack =
        Stats::from_pairs(&[(Stat::AttackPower, 65.0), (Stat::RangedAttackPower, 65.0)]);

    env.sim.get_or_register_aura(
        unit,
        AuraConfig {
            label: "Jom Gabbar".to_string(),
            action_id: Some(action_id.clone()),
            duration,
            max_stacks: 10,
            on_gain: Some(Rc::new(|sim: &mut Sim, _| {
                sim.start_periodic_action(PeriodicActionOptions {
                    period: 2 * SECOND,
                    num_ticks: 10,
                    tick_immediately: true,
                });
            })),
            on_stacks_change: Some(Rc::new(
                move |sim: &mut Sim, _aura: AuraId, old: i32, new: i32| {
                    sim.add_stats_dynamic(unit, &bonus_per_stack.multiply(f64::from(new - old)));
                },
            )),
            ..AuraConfig::default()
        },
    );

    let timer = env.sim.new_timer(unit);
    let offensive_trinket_timer = env.sim.category_timer(unit, 1141);
    let spell = env.sim.register_spell(
        unit,
        SpellConfig {
            action_id,
            flags: SpellFlag::NO_ON_CAST_COMPLETE,
            cast: CastConfig {
                cd: Cooldown {
                    timer: Some(timer),
                    duration: 120 * SECOND,
                },
                shared_cd: Cooldown {
                    timer: Some(offensive_trinket_timer),
                    duration,
                },
                ..CastConfig::default()
            },
            ..SpellConfig::default()
        },
    );
    env.sim.add_major_cooldown(
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

// ---------------------------------------------------------------------------------------------
// enchants.go.
// ---------------------------------------------------------------------------------------------

/// The weapon enchant damage procs enchants.go registers over `SpellDataProcDamageSpell`: Fiery
/// Weapon and Lifestealing.
const WEAPON_ENCHANT_PROCS: [(i32, &str, f64, i32); 2] = [
    (803, "Fiery Weapon", 6.0, 13897),
    (1898, "Lifestealing", 6.66, 20004),
];

fn apply_hand_written_enchant(env: &mut Environment, enchant: i32) -> bool {
    let unit = env.player;
    match enchant {
        super::revelation::REVELATION_ENCHANT => {
            super::revelation::apply_revelation(env);
            true
        }
        // The scopes add to the ranged weapon's damage.
        30 | 32 | 33 | 663 | 664 => {
            let damage = match enchant {
                30 => 1.0,
                32 => 2.0,
                33 => 3.0,
                663 => 5.0,
                _ => 7.0,
            };
            let ranged = &mut env.sim.unit_mut(unit).auto_attacks.ranged;
            ranged.base_damage_min += damage;
            ranged.base_damage_max += damage;
            true
        }
        _ => {
            let Some((id, name, ppm, spell_id)) = WEAPON_ENCHANT_PROCS
                .iter()
                .find(|(id, ..)| *id == enchant)
                .copied()
            else {
                return false;
            };
            let config = spell_data_proc_damage_spell_config(env, must_find(spell_id));
            let proc_spell = env.sim.get_or_register_spell(unit, config);
            env.sim.spell_mut(proc_spell).flags |=
                SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::PASSIVE_SPELL | SpellFlag::PROC;

            let dpm = dynamic_legacy_proc_for_enchant(&env.sim, unit, id, ppm, 0.0);
            let aura = env.sim.make_proc_trigger_aura(
                unit,
                &ProcTrigger {
                    name: format!("Enchant Weapon - {name}"),
                    callback: CallbackMask::ON_SPELL_HIT_DEALT,
                    action_id: ActionId::spell(spell_id),
                    is_weapon_proc: true,
                    dpm: Some(dpm),
                    outcome: HitOutcome::LANDED,
                    trigger_immediately: true,
                    ..ProcTrigger::default()
                },
            );
            // Go `ItemSwap.RegisterEnchantProc`: the proc holds while a hand carries the enchant.
            env.sim.register_enchant_proc(unit, id, aura);
            true
        }
    }
}

#[cfg(test)]
mod tests;
