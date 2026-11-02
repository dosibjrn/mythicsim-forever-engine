//! Go sim/common/forever/items_sets.go and item_sets_classic.go: the class-independent item sets.
//!
//! A set bonus is applied to the status aura its piece count registers
//! (`item_sets::apply_item_set_bonus_effects`). Handlers that only run in a fight are not
//! carried: a proc trigger records the callbacks it listens to, its chance and its cooldown.

use std::rc::Rc;

use crate::contracts::prepared_v2::ActionId;

use super::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use super::character::constants::{
    DEFENSE_RATING_PER_DEFENSE_LEVEL, EXPERTISE_RATING_PER_EXPERTISE_PERCENT,
};
use super::env::Environment;
use super::item_sets::ItemSet;
use super::resolve_proc::{proc_trigger, stated_chance};
use super::shared_items::spell_data_proc_damage_spell_config;
use super::sim::{AuraId, Sim, SECOND};
use super::spell::{school, DefenseType, ProcMask, SpellConfig, SpellFlag};
use super::spelldata::must_find;
use super::stats::{Stat, Stats};

/// Go `setHitPercent`.
fn set_hit_percent(pct: f64) -> Stats {
    Stats::from_pairs(&[
        (Stat::PhysicalHitPercent, pct),
        (Stat::SpellHitPercent, pct),
    ])
}

/// Go `setCritPercent`.
fn set_crit_percent(pct: f64) -> Stats {
    Stats::from_pairs(&[
        (Stat::PhysicalCritPercent, pct),
        (Stat::SpellCritPercent, pct),
    ])
}

/// Go `setResistances`.
fn set_resistances(n: f64) -> Stats {
    Stats::from_pairs(&[
        (Stat::ArcaneResistance, n),
        (Stat::FireResistance, n),
        (Stat::FrostResistance, n),
        (Stat::NatureResistance, n),
        (Stat::ShadowResistance, n),
    ])
}

/// Go `setSpellPower`: master's SpellPower is damage and healing.
fn set_spell_power(n: f64) -> Stats {
    Stats::from_pairs(&[(Stat::SpellDamage, n), (Stat::HealingPower, n)])
}

/// Go `setAttackPower`.
fn set_attack_power(n: f64) -> Stats {
    Stats::from_pairs(&[(Stat::AttackPower, n), (Stat::RangedAttackPower, n)])
}

/// Go `setStats`: a bonus that is a stat block.
macro_rules! set_stats {
    ($name:ident, $stats:expr) => {
        fn $name(env: &mut Environment, aura: AuraId) {
            env.sim.attach_stats_buff(aura, $stats);
        }
    };
}

/// Go `setNoop`: undescribed or unsimulated bonuses (movement, CC breaks, PvP).
fn set_noop(_env: &mut Environment, _aura: AuraId) {}

/// Go `setUndeadSlaying`: increases your damage against undead by 2%. Go changes the character's
/// attack tables on the aura's gain, which a reset runs once they exist; the change is the same
/// at the end of finalization.
fn set_undead_slaying(env: &mut Environment, aura: AuraId) {
    let unit = env.player;
    env.sim.apply_on_gain(aura, Rc::new(|_: &mut Sim, _| {}));
    env.sim.apply_on_expire(aura, Rc::new(|_: &mut Sim, _| {}));
    env.post_finalize
        .push(Rc::new(move |env: &mut Environment| {
            for defender in env.sim.all_units() {
                if env.sim.unit(defender).mob_type == "MobTypeUndead" {
                    let table = env.attack_table_mut(unit, defender);
                    table.damage_dealt_multiplier *= 1.02;
                    table.crit_multiplier *= 1.02;
                }
            }
        }));
}

/// Go `setHumanoidAttackPower`: attack power against humanoids, which the attack tables carry as
/// mob type bonus stats and the export lists as unrepresented.
fn set_humanoid_attack_power(env: &mut Environment, aura: AuraId, n: f64) {
    let unit = env.player;
    env.sim.apply_on_gain(aura, Rc::new(|_: &mut Sim, _| {}));
    env.sim.apply_on_expire(aura, Rc::new(|_: &mut Sim, _| {}));
    env.post_finalize
        .push(Rc::new(move |env: &mut Environment| {
            for defender in env.sim.all_units() {
                let table = env.attack_table_mut(unit, defender);
                let entry = table
                    .mob_type_bonus_stats
                    .entry("MobTypeHumanoid".to_string())
                    .or_default();
                *entry = entry.add(&set_attack_power(n));
            }
        }));
}

/// Go `setMeleeDamageProc`: a melee proc that deals damage of the given school, at the trigger's
/// client chance.
fn set_melee_damage_proc(
    env: &mut Environment,
    aura: AuraId,
    name: &str,
    trigger_id: i32,
    spell_id: i32,
    spell_school: u8,
) {
    let unit = env.player;
    env.sim.register_spell(
        unit,
        SpellConfig {
            action_id: ActionId::spell(spell_id),
            spell_school,
            defense_type: DefenseType::Magic,
            proc_mask: ProcMask::EMPTY,
            flags: SpellFlag::PASSIVE_SPELL | SpellFlag::PROC,
            damage_multiplier: 1.0,
            threat_multiplier: 1.0,
            ..SpellConfig::default()
        },
    );
    env.sim.attach_proc_trigger(
        aura,
        &ProcTrigger {
            name: name.to_string(),
            action_id: ActionId::spell(trigger_id),
            callback: CallbackMask::ON_SPELL_HIT_DEALT,
            outcome: HitOutcome::LANDED,
            proc_mask: ProcMask::MELEE,
            proc_chance: stated_chance(must_find(trigger_id)),
            ..ProcTrigger::default()
        },
    );
}

/// Go `setStatProc`: a proc granting a temporary stat buff.
fn set_stat_proc(
    env: &mut Environment,
    aura: AuraId,
    config: ProcTrigger,
    aura_label: &str,
    aura_id: i32,
    stats: Stats,
    duration: i64,
) {
    let unit = env.player;
    env.sim
        .new_temporary_stats_aura(unit, aura_label, &ActionId::spell(aura_id), stats, duration);
    env.sim.attach_proc_trigger(aura, &config);
}

/// Go `suddenInsight`: the 5 piece shared by Magister's Regalia and Vestments of the Devout: a
/// spellcast has a 5% chance to restore 200 mana.
fn sudden_insight(env: &mut Environment, aura: AuraId, spell_id: i32) {
    env.sim.attach_proc_trigger(
        aura,
        &ProcTrigger {
            action_id: ActionId::spell(spell_id),
            name: "Sudden Insight".to_string(),
            callback: CallbackMask::ON_CAST_COMPLETE,
            proc_mask: ProcMask::SPELL_DAMAGE | ProcMask::SPELL_HEALING,
            proc_chance: 0.05,
            ..ProcTrigger::default()
        },
    );
}

/// The proc a trigger that listens for a spell cast or a hit taken registers on the set bonus,
/// with the action and chance Go states.
fn trigger(name: &str, spell_id: i32, callback: CallbackMask, proc_mask: ProcMask) -> ProcTrigger {
    ProcTrigger {
        action_id: ActionId::spell(spell_id),
        name: name.to_string(),
        callback,
        proc_mask,
        ..ProcTrigger::default()
    }
}

// Go's `dungeonResistBonus`, `dungeonManaPerFive` and `dungeonVitality`.
set_stats!(dungeon_resist_bonus, set_resistances(8.0));
set_stats!(
    dungeon_mana_per_five,
    Stats::from_pairs(&[(Stat::MP5, 8.0)])
);

// ---------------------------------------------------------------------------------------------
// items_sets.go: Blacksmithing plate and Defias Leather.
// ---------------------------------------------------------------------------------------------

fn imperial_plate_2(env: &mut Environment, aura: AuraId) {
    env.sim.attach_stat_buff(
        aura,
        Stat::DefenseRating,
        7.0 * DEFENSE_RATING_PER_DEFENSE_LEVEL,
    );
    env.sim.expose_to_apl(aura, 13385);
}

fn imperial_plate_3(env: &mut Environment, aura: AuraId) {
    env.sim.attach_stats_buff(aura, set_hit_percent(1.0));
    env.sim.expose_to_apl(aura, 1251990);
}

fn imperial_plate_5(env: &mut Environment, aura: AuraId) {
    env.sim.attach_stat_buff(aura, Stat::Strength, 20.0);
    env.sim.expose_to_apl(aura, 1251984);
}

fn imperial_plate_6(env: &mut Environment, aura: AuraId) {
    env.sim.attach_stats_buff(aura, set_crit_percent(1.0));
    env.sim.expose_to_apl(aura, 1251991);
}

fn blessed_plate_2(env: &mut Environment, aura: AuraId) {
    env.sim.attach_stat_buff(aura, Stat::ShadowResistance, 10.0);
    env.sim.expose_to_apl(aura, 14673);
}

fn blessed_plate_3(env: &mut Environment, aura: AuraId) {
    env.sim.attach_stat_buff(aura, Stat::Strength, 20.0);
    env.sim.expose_to_apl(aura, 1252009);
}

fn blessed_plate_5(env: &mut Environment, aura: AuraId) {
    env.sim.attach_stats_buff(aura, set_hit_percent(2.0));
    env.sim.expose_to_apl(aura, 1252013);
}

fn blessed_plate_6(env: &mut Environment, aura: AuraId) {
    env.sim.attach_stat_buff(aura, Stat::HolyDamage, 29.0);
    env.sim.expose_to_apl(aura, 21518);
}

set_stats!(
    defias_leather_2,
    Stats::from_pairs(&[(Stat::ArcaneResistance, 5.0)])
);

fn defias_leather_3(env: &mut Environment, aura: AuraId) {
    set_humanoid_attack_power(env, aura, 15.0);
}

/// Devious Strike (1292028): white hits from behind have a 5% chance to make the target bleed.
fn defias_leather_4(env: &mut Environment, aura: AuraId) {
    let unit = env.player;
    if env.sim.unit(unit).pseudo_stats.in_front_of_target {
        return;
    }
    let bleed = spell_data_proc_damage_spell_config(env, must_find(1292029));
    env.sim.register_spell(unit, bleed);
    let mut listener = proc_trigger(&env.sim, Some(unit), must_find(1292028), &[]);
    listener.trigger_immediately = true;
    env.sim.attach_proc_trigger(aura, &listener);
}

/// Increased Daggers +1 (7534), the old 3 piece bonus Forever moved to five pieces (patch 135).
fn defias_leather_5(env: &mut Environment, aura: AuraId) {
    env.sim
        .attach_weapon_skill(aura, super::weapon_skill::WeaponSkill::Daggers, 1.0);
}

// ---------------------------------------------------------------------------------------------
// item_sets_classic.go: crafted, PvP and dungeon sets.
// ---------------------------------------------------------------------------------------------

/// Increased Hit Chance 01 (15464). Client 1.60.1.70707 gives the row a spell hit effect beside
/// its melee hit (SpellEffect 1366108, aura 55, 1%), as every "Increased Hit Chance" row; the Go
/// reference keeps melee hit alone.
fn black_dragon_mail_2(env: &mut Environment, aura: AuraId) {
    let stats = match env.sim.rules {
        super::sim::Rules::Forever => set_hit_percent(1.0),
        super::sim::Rules::GoReference => Stats::from_pairs(&[(Stat::PhysicalHitPercent, 1.0)]),
    };
    env.sim.attach_stats_buff(aura, stats);
}
set_stats!(
    black_dragon_mail_3,
    Stats::from_pairs(&[(Stat::PhysicalCritPercent, 2.0)])
);
set_stats!(
    black_dragon_mail_4,
    Stats::from_pairs(&[(Stat::FireResistance, 10.0)])
);
set_stats!(blue_dragon_mail_2, set_resistances(4.0));
set_stats!(blue_dragon_mail_3, set_spell_power(28.0));
// Forever gives the bonus at 3 pieces (client ItemSetSpell 443: 21636 at 3), not Era's 2.
set_stats!(bloodsoul_embrace_3, Stats::from_pairs(&[(Stat::MP5, 12.0)]));
set_stats!(
    bloodvine_garb_3,
    Stats::from_pairs(&[(Stat::SpellCritPercent, 2.0)])
);
set_stats!(blood_tiger_harness_2, set_crit_percent(1.0));
set_stats!(devilsaur_armor_2, set_hit_percent(2.0));
set_stats!(green_dragon_mail_2, Stats::from_pairs(&[(Stat::MP5, 3.0)]));

/// Allows 15% of your Mana regeneration to continue while casting.
fn green_dragon_mail_3(env: &mut Environment, aura: AuraId) {
    let unit = env.player;
    env.sim.apply_on_gain(
        aura,
        Rc::new(move |sim: &mut Sim, _| {
            sim.unit_mut(unit).pseudo_stats.spirit_regen_rate_casting += 0.15;
        }),
    );
    env.sim.apply_on_expire(
        aura,
        Rc::new(move |sim: &mut Sim, _| {
            sim.unit_mut(unit).pseudo_stats.spirit_regen_rate_casting -= 0.15;
        }),
    );
}

set_stats!(ironfeather_armor_2, set_spell_power(20.0));

/// 10% chance of dealing 15 to 25 Nature damage on a successful melee attack.
fn stormshroud_armor_2(env: &mut Environment, aura: AuraId) {
    set_melee_damage_proc(env, aura, "Lightning", 18979, 18980, school::NATURE);
}

/// 4% chance on melee attack of restoring 30 energy.
fn stormshroud_armor_3(env: &mut Environment, aura: AuraId) {
    if !env.sim.unit(env.player).energy_bar.enabled {
        return;
    }
    env.sim.attach_proc_trigger(
        aura,
        &ProcTrigger {
            name: "Revitalize".to_string(),
            action_id: ActionId::spell(23863),
            callback: CallbackMask::ON_SPELL_HIT_DEALT,
            outcome: HitOutcome::LANDED,
            proc_mask: ProcMask::MELEE,
            proc_chance: stated_chance(must_find(23863)),
            ..ProcTrigger::default()
        },
    );
}

set_stats!(stormshroud_armor_4, set_attack_power(14.0));
// Forever gives the bonus at 3 pieces (client ItemSetSpell 444: 21416 at 3), not Era's 2.
set_stats!(
    the_darksoul_3,
    Stats::from_pairs(&[(Stat::DefenseRating, 20.0 * DEFENSE_RATING_PER_DEFENSE_LEVEL)])
);

/// 30 to 50 Fire damage, doubled from Era's 15 to 25.
fn volcanic_armor_3(env: &mut Environment, aura: AuraId) {
    set_melee_damage_proc(
        env,
        aura,
        "Firebolt Trigger (Volcanic Armor)",
        9233,
        9057,
        school::FIRE,
    );
}

set_stats!(
    highlanders_fortitude_2,
    Stats::from_pairs(&[(Stat::Stamina, 5.0)])
);
set_stats!(
    highlanders_fortitude_3,
    Stats::from_pairs(&[(Stat::SpellCritPercent, 1.0)])
);

set_stats!(
    wildheart_raiment_3,
    set_attack_power(30.0).add(&set_spell_power(18.0))
);

/// Nature's Bounty: a spellcast returns 200 mana, a melee attack 4 energy a second for 5 sec,
/// and being struck 10 rage.
fn wildheart_raiment_5(env: &mut Environment, aura: AuraId) {
    env.sim.attach_proc_trigger(
        aura,
        &ProcTrigger {
            proc_chance: 0.02,
            ..trigger(
                "Nature's Bounty",
                450608,
                CallbackMask::ON_SPELL_HIT_TAKEN,
                ProcMask::MELEE,
            )
        },
    );
}

set_stats!(beaststalker_armor_3, set_attack_power(30.0));

/// Melee and ranged autoattacks have a 5% chance to restore 200 mana.
fn beaststalker_armor_5(env: &mut Environment, aura: AuraId) {
    env.sim.attach_proc_trigger(
        aura,
        &ProcTrigger {
            outcome: HitOutcome::LANDED,
            proc_chance: 0.05,
            ..trigger(
                "Hunter Armor Energize",
                450577,
                CallbackMask::ON_SPELL_HIT_DEALT,
                ProcMask::WHITE_HIT,
            )
        },
    );
}

set_stats!(magisters_regalia_3, set_spell_power(18.0));

fn magisters_regalia_5(env: &mut Environment, aura: AuraId) {
    sudden_insight(env, aura, 450527);
}

set_stats!(lightforge_armor_3, set_spell_power(18.0));

/// Crusader's Wrath, moved down from 6 pieces and cut from 95 to 65 spell power.
fn lightforge_armor_5(env: &mut Environment, aura: AuraId) {
    let unit = env.player;
    // Go NewLegacyPPMManager, which an item swap rebuilds for the weapons in the hands.
    let dpm = Rc::new(
        env.sim
            .new_ppm_manager(unit, 3.0, ProcMask::MELEE_WHITE_HIT)
            .rebuilt_by_swap(
                &env.sim,
                unit,
                super::item_swap::ManagerChance::Ppm(3.0),
                super::item_swap::ManagerMask::Static(ProcMask::MELEE_WHITE_HIT),
            ),
    );
    set_stat_proc(
        env,
        aura,
        ProcTrigger {
            outcome: HitOutcome::LANDED,
            dpm: Some(dpm),
            ..trigger(
                "Item - Crusader's Wrath Proc - Lightforge Armor",
                450625,
                CallbackMask::ON_SPELL_HIT_DEALT,
                ProcMask::MELEE_WHITE_HIT,
            )
        },
        "Crusader's Wrath",
        27499,
        set_spell_power(65.0),
        10 * SECOND,
    );
}

set_stats!(vestments_of_the_devout_3, set_spell_power(18.0));

fn vestments_of_the_devout_5(env: &mut Environment, aura: AuraId) {
    sudden_insight(env, aura, 450576);
}

set_stats!(shadowcraft_armor_3, set_attack_power(30.0));

/// Chance on melee attack to restore energy, moved down from 6 pieces and cut 35 to 20.
fn shadowcraft_armor_5(env: &mut Environment, aura: AuraId) {
    let unit = env.player;
    let dpm = Rc::new(
        env.sim
            .new_ppm_manager(unit, 1.0, ProcMask::MELEE_WHITE_HIT),
    );
    env.sim.attach_proc_trigger(
        aura,
        &ProcTrigger {
            outcome: HitOutcome::LANDED,
            dpm: Some(dpm),
            ..trigger(
                "Rogue Armor Energize",
                27787,
                CallbackMask::ON_SPELL_HIT_DEALT,
                ProcMask::MELEE_WHITE_HIT,
            )
        },
    );
}

set_stats!(the_elements_3, set_spell_power(18.0));

/// The Furious Storm, moved down from 6 pieces and cut from 95 to 65 spell power.
fn the_elements_5(env: &mut Environment, aura: AuraId) {
    set_stat_proc(
        env,
        aura,
        ProcTrigger {
            proc_chance: 0.04,
            ..trigger(
                "Item - The Furious Storm Proc",
                450626,
                CallbackMask::ON_CAST_COMPLETE,
                ProcMask::SPELL_DAMAGE | ProcMask::SPELL_HEALING,
            )
        },
        "The Furious Storm",
        27775,
        set_spell_power(65.0),
        10 * SECOND,
    );
}

set_stats!(dreadmist_raiment_3, set_spell_power(18.0));

/// Spellcasts have a 5% chance to heal you for 270 to 330.
fn dreadmist_raiment_5(env: &mut Environment, aura: AuraId) {
    env.sim.attach_proc_trigger(
        aura,
        &ProcTrigger {
            proc_chance: 0.05,
            ..trigger(
                "Dark Reward",
                450585,
                CallbackMask::ON_CAST_COMPLETE,
                ProcMask::SPELL_DAMAGE | ProcMask::SPELL_HEALING,
            )
        },
    );
}

set_stats!(
    battlegear_of_valor_3,
    Stats::from_pairs(&[(Stat::Strength, 15.0)])
);

/// Warrior's Resolve, moved down from 6 pieces: it now returns 10 Rage with the heal.
fn battlegear_of_valor_5(env: &mut Environment, aura: AuraId) {
    let unit = env.player;
    let dpm = Rc::new(env.sim.new_ppm_manager(unit, 1.0, ProcMask::MELEE));
    env.sim.attach_proc_trigger(
        aura,
        &ProcTrigger {
            outcome: HitOutcome::LANDED,
            dpm: Some(dpm),
            ..trigger(
                "Warrior's Resolve",
                450587,
                CallbackMask::ON_SPELL_HIT_DEALT,
                ProcMask::MELEE,
            )
        },
    );
}

// ---------------------------------------------------------------------------------------------
// item_sets_classic.go: Scholomance, Stratholme and the other PvE sets.
// ---------------------------------------------------------------------------------------------

set_stats!(necropile_raiment_2, set_hit_percent(0.5));
set_stats!(necropile_raiment_4, set_resistances(5.0));
set_stats!(necropile_raiment_5, set_spell_power(23.0));
set_stats!(
    ironweave_battlesuit_2,
    Stats::from_pairs(&[(Stat::Armor, 200.0)])
);
set_stats!(
    ironweave_battlesuit_3,
    Stats::from_pairs(&[(Stat::SpellPiercing, 5.0)])
);
set_stats!(ironweave_battlesuit_5, set_spell_power(23.0));
set_stats!(the_postmaster_3, set_spell_power(23.0));
set_stats!(the_postmaster_5, set_hit_percent(1.0));
set_stats!(cadaverous_garb_3, set_attack_power(10.0));
set_stats!(cadaverous_garb_4, set_resistances(5.0));
set_stats!(cadaverous_garb_5, set_hit_percent(2.0));
set_stats!(bloodmail_regalia_2, set_attack_power(10.0));
set_stats!(bloodmail_regalia_4, set_resistances(5.0));
set_stats!(bloodmail_regalia_5, set_crit_percent(1.5));
set_stats!(
    deathbone_guardian_2,
    Stats::from_pairs(&[(Stat::DefenseRating, 3.0 * DEFENSE_RATING_PER_DEFENSE_LEVEL)])
);

/// Deathbone Surge: 15% chance when struck to gain 70 spell damage and healing for 10 sec, once
/// a minute.
fn deathbone_guardian_3(env: &mut Environment, aura: AuraId) {
    set_stat_proc(
        env,
        aura,
        ProcTrigger {
            proc_chance: 0.15,
            icd: 60 * SECOND,
            ..trigger(
                "Deathbone Surge",
                1299743,
                CallbackMask::ON_SPELL_HIT_TAKEN,
                ProcMask::MELEE,
            )
        },
        "Deathbone Surge",
        1299746,
        set_spell_power(70.0),
        10 * SECOND,
    );
}

set_stats!(deathbone_guardian_4, set_resistances(5.0));
set_stats!(
    deathbone_guardian_5,
    Stats::from_pairs(&[(
        Stat::ExpertiseRating,
        2.0 * EXPERTISE_RATING_PER_EXPERTISE_PERCENT
    )])
);

/// Chance on Hit: immobilizes the target and lowers their armor by 100 for 10 sec; master applies
/// the -100 armor to the wearer, which is kept so the engines agree.
fn spiders_kiss_2(env: &mut Environment, aura: AuraId) {
    set_stat_proc(
        env,
        aura,
        ProcTrigger {
            outcome: HitOutcome::LANDED,
            proc_chance: 0.05,
            ..trigger(
                "Spider's Kiss",
                17333,
                CallbackMask::ON_SPELL_HIT_DEALT,
                ProcMask::MELEE,
            )
        },
        "Spider's Kiss",
        17333,
        Stats::from_pairs(&[(Stat::Armor, -100.0)]),
        10 * SECOND,
    );
}

set_stats!(dal_rends_arms_2, set_attack_power(50.0));
set_stats!(shard_of_the_gods_2, set_resistances(15.0));

fn spirit_of_eskhandar_2(env: &mut Environment, aura: AuraId) {
    set_humanoid_attack_power(env, aura, 30.0);
}

set_stats!(spirit_of_eskhandar_3, set_crit_percent(1.0));

// ---------------------------------------------------------------------------------------------
// The sets.
// ---------------------------------------------------------------------------------------------

macro_rules! set {
    ($id:expr, $name:expr, $bonuses:expr) => {
        ItemSet {
            id: $id,
            name: $name,
            alternative_name: "",
            bonuses: $bonuses,
            required_profession: "",
        }
    };
}

/// Every set `sim/common/forever` registers, in Go's order.
pub(crate) static ITEM_SETS: &[ItemSet] = &[
    set!(
        321,
        "Imperial Plate",
        &[
            (2, imperial_plate_2),
            (3, imperial_plate_3),
            (5, imperial_plate_5),
            (6, imperial_plate_6),
        ]
    ),
    set!(
        1968,
        "Blessed Plate",
        &[
            (2, blessed_plate_2),
            (3, blessed_plate_3),
            (5, blessed_plate_5),
            (6, blessed_plate_6),
        ]
    ),
    set!(
        161,
        "Defias Leather",
        &[
            (2, defias_leather_2),
            (3, defias_leather_3),
            (4, defias_leather_4),
            (5, defias_leather_5),
        ]
    ),
    set!(
        489,
        "Black Dragon Mail",
        &[
            (2, black_dragon_mail_2),
            (3, black_dragon_mail_3),
            (4, black_dragon_mail_4),
        ]
    ),
    set!(
        491,
        "Blue Dragon Mail",
        &[(2, blue_dragon_mail_2), (3, blue_dragon_mail_3)]
    ),
    set!(0, "Bloodsoul Embrace", &[(3, bloodsoul_embrace_3)]),
    ItemSet {
        id: 0,
        name: "Bloodvine Garb",
        alternative_name: "",
        bonuses: &[(3, bloodvine_garb_3)],
        required_profession: "Tailoring",
    },
    ItemSet {
        id: 0,
        name: "Blood Tiger Harness",
        alternative_name: "",
        bonuses: &[(2, blood_tiger_harness_2)],
        required_profession: "Leatherworking",
    },
    set!(143, "Devilsaur Armor", &[(2, devilsaur_armor_2)]),
    set!(
        490,
        "Green Dragon Mail",
        &[(2, green_dragon_mail_2), (3, green_dragon_mail_3)]
    ),
    set!(0, "Ironfeather Armor", &[(2, ironfeather_armor_2)]),
    set!(
        0,
        "Stormshroud Armor",
        &[
            (2, stormshroud_armor_2),
            (3, stormshroud_armor_3),
            (4, stormshroud_armor_4),
        ]
    ),
    ItemSet {
        id: 0,
        name: "The Darksoul",
        alternative_name: "",
        bonuses: &[(3, the_darksoul_3)],
        required_profession: "Blacksmithing",
    },
    set!(141, "Volcanic Armor", &[(3, volcanic_armor_3)]),
    set!(
        0,
        "The Highlander's Fortitude",
        &[(2, highlanders_fortitude_2), (3, highlanders_fortitude_3)]
    ),
    set!(
        0,
        "Wildheart Raiment",
        &[
            (2, dungeon_resist_bonus),
            (3, wildheart_raiment_3),
            (4, set_noop),
            (5, wildheart_raiment_5),
            (6, dungeon_mana_per_five),
        ]
    ),
    set!(
        0,
        "Beaststalker Armor",
        &[
            (2, dungeon_resist_bonus),
            (3, beaststalker_armor_3),
            (4, set_noop),
            (5, beaststalker_armor_5),
            (6, dungeon_mana_per_five),
        ]
    ),
    set!(
        0,
        "Magister's Regalia",
        &[
            (2, dungeon_resist_bonus),
            (3, magisters_regalia_3),
            (4, set_noop),
            (5, magisters_regalia_5),
            (6, dungeon_mana_per_five),
        ]
    ),
    set!(
        0,
        "Lightforge Armor",
        &[
            (2, dungeon_resist_bonus),
            (3, lightforge_armor_3),
            (4, set_noop),
            (5, lightforge_armor_5),
            (6, dungeon_mana_per_five),
        ]
    ),
    set!(
        0,
        "Vestments of the Devout",
        &[
            (2, dungeon_resist_bonus),
            (3, vestments_of_the_devout_3),
            (4, set_noop),
            (5, vestments_of_the_devout_5),
            (6, dungeon_mana_per_five),
        ]
    ),
    set!(
        0,
        "Shadowcraft Armor",
        &[
            (2, dungeon_resist_bonus),
            (3, shadowcraft_armor_3),
            (4, set_noop),
            (5, shadowcraft_armor_5),
            (6, set_noop),
        ]
    ),
    set!(
        0,
        "The Elements",
        &[
            (2, dungeon_resist_bonus),
            (3, the_elements_3),
            (4, set_noop),
            (5, the_elements_5),
            (6, dungeon_mana_per_five),
        ]
    ),
    set!(
        0,
        "Dreadmist Raiment",
        &[
            (2, dungeon_resist_bonus),
            (3, dreadmist_raiment_3),
            (4, set_noop),
            (5, dreadmist_raiment_5),
            (6, dungeon_mana_per_five),
        ]
    ),
    set!(
        0,
        "Battlegear of Valor",
        &[
            (2, dungeon_resist_bonus),
            (3, battlegear_of_valor_3),
            (4, set_noop),
            (5, battlegear_of_valor_5),
            (6, set_noop),
        ]
    ),
    set!(
        0,
        "Necropile Raiment",
        &[
            (2, necropile_raiment_2),
            (3, set_noop),
            (4, necropile_raiment_4),
            (5, necropile_raiment_5),
        ]
    ),
    set!(
        0,
        "Ironweave Battlesuit",
        &[
            (2, ironweave_battlesuit_2),
            (3, ironweave_battlesuit_3),
            (4, set_noop),
            (5, ironweave_battlesuit_5),
            (6, set_noop),
        ]
    ),
    set!(
        0,
        "The Postmaster",
        &[
            (2, set_noop),
            (3, the_postmaster_3),
            (4, set_noop),
            (5, the_postmaster_5),
        ]
    ),
    set!(
        0,
        "Cadaverous Garb",
        &[
            (3, cadaverous_garb_3),
            (4, cadaverous_garb_4),
            (5, cadaverous_garb_5),
        ]
    ),
    set!(
        0,
        "Bloodmail Regalia",
        &[
            (2, bloodmail_regalia_2),
            (3, set_noop),
            (4, bloodmail_regalia_4),
            (5, bloodmail_regalia_5),
        ]
    ),
    set!(
        0,
        "Deathbone Guardian",
        &[
            (2, deathbone_guardian_2),
            (3, deathbone_guardian_3),
            (4, deathbone_guardian_4),
            (5, deathbone_guardian_5),
        ]
    ),
    set!(0, "Spider's Kiss", &[(2, spiders_kiss_2)]),
    set!(0, "Dal'Rend's Arms", &[(2, dal_rends_arms_2)]),
    set!(0, "Shard of the Gods", &[(2, shard_of_the_gods_2)]),
    set!(
        0,
        "Spirit of Eskhandar",
        &[
            (2, spirit_of_eskhandar_2),
            (3, spirit_of_eskhandar_3),
            (4, set_noop),
        ]
    ),
    set!(0, "Regalia of Undead Cleansing", &[(3, set_undead_slaying)]),
    set!(0, "Undead Slayer's Armor", &[(3, set_undead_slaying)]),
    set!(0, "Garb of the Undead Slayer", &[(3, set_undead_slaying)]),
    set!(
        0,
        "Battlegear of Undead Slaying",
        &[(3, set_undead_slaying)]
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::tables::tables;

    #[test]
    fn every_set_is_one_go_registers_with_the_bonuses_it_registers() {
        for set in ITEM_SETS {
            let go = tables()
                .item_sets
                .iter()
                .find(|row| row.name == set.name && row.id == set.id)
                .unwrap_or_else(|| panic!("Go registers no set {} ({})", set.name, set.id));
            let ours: Vec<i32> = set.bonuses.iter().map(|(pieces, _)| *pieces).collect();
            assert_eq!(ours, go.bonus_pieces, "{}", set.name);
        }
    }

    #[test]
    fn the_zul_gurub_crafted_sets_follow_the_client() {
        // Client ItemSet RequiredSkill at rank 300: 197 Tailoring, 165 Leatherworking,
        // 164 Blacksmithing, 0 for Bloodsoul Embrace. Pieces from ItemSetSpell.
        let expected = [
            ("Bloodvine Garb", 421, "Tailoring"),
            ("Blood Tiger Harness", 442, "Leatherworking"),
            ("Bloodsoul Embrace", 443, ""),
            ("The Darksoul", 444, "Blacksmithing"),
        ];
        for (name, client_id, profession) in expected {
            let set = ITEM_SETS
                .iter()
                .find(|set| set.name == name)
                .unwrap_or_else(|| panic!("no set {name}"));
            assert_eq!(set.required_profession, profession, "{name}");
            let mut client: Vec<i32> = super::super::forever_set_spells_generated::SET_BONUSES
                .iter()
                .filter(|(id, _, _)| *id == client_id)
                .map(|(_, pieces, _)| *pieces)
                .collect();
            client.dedup();
            let ours: Vec<i32> = set.bonuses.iter().map(|(pieces, _)| *pieces).collect();
            assert_eq!(ours, client, "{name}");
        }
    }

    #[test]
    fn the_sets_of_these_files_are_all_there() {
        // items_sets.go and item_sets_classic.go define 38 sets, in this order.
        let names: Vec<&str> = ITEM_SETS.iter().map(|set| set.name).collect();
        assert_eq!(names.len(), 38);
        assert_eq!(names[0], "Imperial Plate");
        assert_eq!(names[37], "Battlegear of Undead Slaying");
    }
}
