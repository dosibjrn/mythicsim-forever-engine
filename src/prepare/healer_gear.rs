//! MythicSim's healer sims under Forever's rules (README, Rust-only mechanics): the healing gear
//! the Go reference leaves out, which counts for nothing there.
//!
//! - **Set bonuses the reference does not register.** The client's own bonus rows
//!   (`forever_set_spells_generated`) are read for every set whose name or ID the reference does
//!   not know, as Vestments of Prophecy, Vestments of Faith or the PvP sets: each bonus the
//!   player reaches is a permanent status aura carrying the row's healing power, spell damage,
//!   mana per 5 seconds, spell crit, stats, casting regeneration and spell mods. Anything else a
//!   row states (procs, conditional bonuses, resistances, movement) is left out.
//! - **Healing bonuses of sets the reference leaves empty**: Stormrage Raiment's Regrowth cast
//!   time and Rejuvenation duration, The Earthfury's Mana Surge and chaining Healing Wave, and
//!   The Ten Storms' Chain Heal jumps.
//! - **Healing items the reference registers nothing for**: the healing idols and totems, and
//!   Hazza'rah's Charm of Healing and Wushoolay's Charm of Nature on use.
//! - **Mana on cast**: Sudden Insight, the five piece of Vestments of the Devout and Magister's
//!   Regalia, which the reference registers but Rust left unclaimed, and the items above that
//!   return mana on a heal ([`Effect::ManaOnCast`](crate::contracts::prepared_v2::Effect)).
//! - **The Furious Storm**, the five piece of The Elements and four piece of The Five Thunders:
//!   a cast's chance at 65 spell power for 10 seconds, as a spell data stat proc.
//!
//! None of it reaches a sim that is not a healer's, so the damage specs keep the reference's
//! gear.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::contracts::prepared_v2::ActionId;
use crate::data::spells::ClassFlags;
use crate::data::tables::tables;

use super::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger, StackingStatAura};
use super::character::{cooldown_type, MajorCooldown};
use super::consumable_effects::proc_trigger_spells;
use super::consumable_effects::{action_id_string, flat_string};
use super::dbcenums;
use super::env::Environment;
use super::forever_items::Registration;
use super::parse_effects::{parse_effects, ParseOptions};
use super::shared_items::{dpm_for_mask, EffectSource};
use super::shared_on_use::StackingStatBonusCd;
use super::shared_on_use::{self, OnUseKind};
use super::sim::{AuraConfig, NEVER_EXPIRES, SECOND};
use super::sim::{AuraId, Rules, Sim, SpellId, UnitId};
use super::spell::{ProcMask, Resource};
use super::spell::{SpellConfig, SpellFlag};
use super::spelldata::store::must_find;
use super::spelldata::Spell as Row;
use super::stats::{Stat, Stats};
use super::Refusal;

/// What the healer's gear changes in the heals' export and the listeners it exports.
#[derive(Clone, Default)]
pub(crate) struct HealerGear {
    /// Healing added to the direct heal of the heals each client mask names: Totem of Life.
    pub direct_bonus: Vec<(ClassFlags, f64)>,
    /// A heal that now jumps, as (client mask, players, falloff): The Earthfury's Healing Wave.
    pub chains: Vec<(ClassFlags, u32, f64)>,
    /// A multiplier on each player past the first: The Ten Storms' Chain Heal.
    pub past_first: Vec<(ClassFlags, f64)>,
    /// Listeners on casts that restore mana.
    pub mana_listeners: Vec<ManaListener>,
    /// What a heal gains, by the client name of its rows.
    pub extras: Vec<(&'static str, HealExtra)>,
    /// Listeners that heal the healer for a share of a heal.
    pub heal_shares: Vec<HealShare>,
    /// Rotmender's Raiment's set aura, whose trigger casts Rotmending.
    pub rotmending: Option<AuraId>,
}

/// A listener on heals that heals the healer for a share of each, at a chance.
#[derive(Clone)]
pub(crate) struct HealShare {
    pub aura: AuraId,
    pub spell_id: i32,
    pub chance: f64,
    pub share: f64,
}

/// What a set or item adds to a heal.
#[derive(Clone)]
pub(crate) enum HealExtra {
    /// A heal over time rank, by spell ID, that goes up with the heal at no cost.
    ThenHot(i32),
    /// An absorb on the player a crit of the direct heal lands on.
    CritShield(f64),
    /// The direct heal's multiplier while any of these heal over time ranks is up.
    HotBonus(&'static [i32], f64),
    /// What each heal over time up takes off the spell's cooldown when it is cast.
    CooldownPerHot(i64),
}

/// A listener on casts that restores mana.
#[derive(Clone)]
pub(crate) struct ManaListener {
    pub aura: AuraId,
    pub rng_label: &'static str,
    pub chance: f64,
    /// The casts it hears.
    pub trigger: ProcTrigger,
    pub mana: Mana,
    pub metrics_spell_id: i32,
    /// Only a heal's crit restores the mana.
    pub crit_only: bool,
}

/// How much mana a cast returns.
#[derive(Clone, Copy)]
pub(crate) enum Mana {
    Flat(f64),
    /// A share of the spell's base cost: The Earthfury's Mana Surge.
    BaseCostShare(f64),
    /// A share of what the spell costs: Dreamwalker Raiment's Healing Touch refund.
    CostShare(f64),
}

/// Whether the sim is a healer's under Forever's rules, the only sims this module reaches.
pub(crate) fn applies(env: &Environment) -> bool {
    env.rules == Rules::Forever && player_message(env).is_some_and(super::overheal::is_healer)
}

fn player_message(env: &Environment) -> Option<&crate::contracts::request::Message> {
    let raid = env.request.message("raid")?;
    let party = *raid.messages("parties").first()?;
    party.messages("players").first().copied()
}

/// The aura types a client bonus row's effect carries over: the stat blocks and the spell mods
/// a healer's gear states. Percent healing and damage, procs and dummies are left out, as the
/// rows of later sets state conditional bonuses with them.
const READ_AURAS: &[i32] = &[
    dbcenums::A_MOD_DAMAGE_DONE,
    dbcenums::A_MOD_HEALING_DONE,
    dbcenums::A_MOD_POWER_REGEN,
    dbcenums::A_MOD_SPELL_CRIT_CHANCE,
    dbcenums::A_MOD_STAT,
    dbcenums::A_MOD_RATING,
    dbcenums::A_ADD_FLAT_MODIFIER,
    dbcenums::A_ADD_PCT_MODIFIER,
];

/// Applies the healer's gear after the reference's item effects and set bonuses, in the gear
/// build phase.
pub(crate) fn apply(env: &mut Environment) -> Result<(), Refusal> {
    if !applies(env) {
        return Ok(());
    }
    apply_client_set_bonuses(env);
    apply_empty_set_bonuses(env);
    apply_items(env);
    sudden_insight(env);
    Ok(())
}

/// The equipped items' sets, as (set ID, the name the items carry, pieces), in slot order.
fn equipped_sets(sim: &Sim, unit: UnitId) -> Vec<(i32, String, i32)> {
    let mut sets: Vec<(i32, String, i32)> = Vec::new();
    for item in sim.character(unit).equipment.iter() {
        if item.id == 0 || (item.set_id == 0 && item.set_name.is_empty()) {
            continue;
        }
        match sets
            .iter_mut()
            .find(|(id, name, _)| (*id != 0 && *id == item.set_id) || *name == item.set_name)
        {
            Some(set) => set.2 += 1,
            None => sets.push((item.set_id, item.set_name.clone(), 1)),
        }
    }
    sets
}

/// The client's bonus rows of each set the reference does not register.
fn apply_client_set_bonuses(env: &mut Environment) {
    let unit = env.player;
    let go = &tables().item_sets;
    for (set_id, name, pieces) in equipped_sets(&env.sim, unit) {
        if set_id == 0 || super::item_sets::find_set(go, set_id, &name).is_some() {
            continue;
        }
        let label = set_name(set_id).unwrap_or(&name).to_string();
        for &(_, needed, spell) in set_bonuses(set_id) {
            if needed > pieces {
                continue;
            }
            let aura = env
                .sim
                .register_permanent_gear_aura(unit, format!("{label} {needed}P"));
            apply_bonus_row(env, aura, must_find(spell));
        }
    }
}

fn set_bonuses(set_id: i32) -> impl Iterator<Item = &'static (i32, i32, i32)> {
    super::forever_set_spells_generated::SET_BONUSES
        .iter()
        .filter(move |(id, _, _)| *id == set_id)
}

fn set_name(set_id: i32) -> Option<&'static str> {
    super::forever_set_spells_generated::SET_NAMES
        .iter()
        .find(|(id, _)| *id == set_id)
        .map(|(_, name)| *name)
}

/// A bonus row's effects on its status aura: casting regeneration as the reference's Green
/// Dragon Mail does, and the [`READ_AURAS`] through the client parser.
fn apply_bonus_row(env: &mut Environment, aura: AuraId, row: &'static Row) {
    let unit = env.player;
    let mut only = Vec::new();
    for (i, effect) in row.effects.iter().enumerate() {
        if !super::parse_effects::applies_aura(effect.effect_type) {
            continue;
        }
        if effect.aura == dbcenums::A_MOD_MANA_REGEN_INTERRUPT {
            casting_regen(&mut env.sim, unit, aura, effect.percent());
        } else if READ_AURAS.contains(&effect.aura) {
            only.push(i as i32 + 1);
        }
    }
    if !only.is_empty() {
        parse_effects(
            &mut env.sim,
            Some(unit),
            aura,
            row,
            ParseOptions {
                only,
                ..ParseOptions::default()
            },
        );
    }
}

/// Allows a share of the mana regeneration to continue while casting, while the aura is up.
fn casting_regen(sim: &mut Sim, unit: UnitId, aura: AuraId, share: f64) {
    sim.apply_on_gain(
        aura,
        std::rc::Rc::new(move |sim: &mut Sim, _| {
            sim.unit_mut(unit).pseudo_stats.spirit_regen_rate_casting += share;
        }),
    );
    sim.apply_on_expire(
        aura,
        std::rc::Rc::new(move |sim: &mut Sim, _| {
            sim.unit_mut(unit).pseudo_stats.spirit_regen_rate_casting -= share;
        }),
    );
}

/// A client mask of one family.
fn flags(family: i32, mask: [u32; 4]) -> ClassFlags {
    ClassFlags { family, mask }
}

const SHAMAN: i32 = 11;
const DRUID: i32 = 7;
/// The Shaman's Healing Wave and Lesser Healing Wave, and Chain Heal (client 70425).
const HEALING_WAVE: [u32; 4] = [64, 0, 0, 0];
const LESSER_HEALING_WAVE: [u32; 4] = [128, 0, 0, 0];
const CHAIN_HEAL: [u32; 4] = [256, 0, 0, 0];
/// The Druid's Healing Touch.
const HEALING_TOUCH: [u32; 4] = [32, 0, 0, 0];

/// The healing bonuses of sets the reference registers with an empty bonus, read from their
/// client rows where the parser carries them.
fn apply_empty_set_bonuses(env: &mut Environment) {
    let unit = env.player;
    for (_, name, pieces) in equipped_sets(&env.sim, unit) {
        let reached = |needed: i32| pieces >= needed;
        match name.as_str() {
            // Faster Regrowth Cast (21872) and Increased Rejuvenation Duration (21871).
            "Stormrage Raiment" => {
                for (needed, spell) in [(5, 21872), (8, 21871)] {
                    if reached(needed) {
                        let aura = forever_status_aura(env, &name, needed);
                        apply_bonus_row(env, aura, must_find(spell));
                    }
                }
            }
            "The Earthfury" => {
                // Mana Surge (23572): after a Healing Wave or Lesser Healing Wave, a 25% chance
                // to gain mana equal to 35% of the spell's base cost.
                if reached(5) {
                    let aura = forever_status_aura(env, &name, 5);
                    let trigger = cast_trigger(
                        "Mana Surge",
                        23572,
                        flags(SHAMAN, or(HEALING_WAVE, LESSER_HEALING_WAVE)),
                    );
                    env.sim.attach_proc_trigger(aura, &trigger);
                    env.healer_gear.mana_listeners.push(ManaListener {
                        aura,
                        rng_label: "Mana Surge",
                        chance: 0.25,
                        trigger,
                        mana: Mana::BaseCostShare(0.35),
                        metrics_spell_id: 23572,
                        crit_only: false,
                    });
                }
                // Chain Healing Wave (23573): Healing Wave jumps to up to two more players,
                // each jump at 20% of the heal before it.
                if reached(8) {
                    env.healer_gear
                        .chains
                        .push((flags(SHAMAN, HEALING_WAVE), 3, 0.2));
                }
            }
            // Greater Heal Renew (22010): a Greater Heal also puts a rank 5 Renew up.
            // Oracle Healing Bonus (26169): a heal's 20% chance to heal the healer for 10% of
            // it, as the heal lands on another player. Counted on its own spell.
            "Garments of the Oracle" if reached(3) => {
                let unit = env.player;
                let aura = forever_status_aura(env, &name, 3);
                let trigger = ProcTrigger {
                    name: "Oracle Healing Bonus".to_string(),
                    callback: CallbackMask::ON_HEAL_DEALT,
                    proc_mask: ProcMask::SPELL_HEALING,
                    ..ProcTrigger::default()
                };
                env.sim.attach_proc_trigger(aura, &trigger);
                passive_heal_spell(env, unit, 26169);
                env.healer_gear.heal_shares.push(HealShare {
                    aura,
                    spell_id: 26169,
                    chance: f64::from(must_find(26169).proc_chance) / 100.0,
                    share: 0.1,
                });
            }
            // Rotmending (1321587, 1321586): a heal's cast, at most every 2 sec, puts a heal over
            // time of 40 every 3 sec for 15 sec on its target, counted at once as its five ticks.
            "Rotmender's Raiment" if reached(5) => {
                let unit = env.player;
                let aura = forever_status_aura(env, &name, 5);
                let proc = must_find(1321587);
                let trigger = ProcTrigger {
                    name: "Rotmending".to_string(),
                    callback: CallbackMask::ON_CAST_COMPLETE,
                    proc_mask: ProcMask::SPELL_HEALING,
                    icd: i64::from(proc.icd_ms) * super::sim::MILLISECOND,
                    ..ProcTrigger::default()
                };
                env.sim.attach_proc_trigger(aura, &trigger);
                passive_heal_spell(env, unit, 1321586);
                env.healer_gear.rotmending = Some(aura);
            }
            // Totemic Energy is parsed from its row (2); Strong Current (29171): Mana Spring
            // totems 25% more mana (4), as the party's Mana Spring buff; Lightning Shield's 15
            // mana per 5 sec (8), read as Lightning Shield kept up, as a healer wearing the set
            // keeps it.
            "The Earthshatterer" => {
                if reached(4) {
                    let share = must_find(29171).effect_n(1).percent();
                    let unit = env.player;
                    env.pre_finalize
                        .push(std::rc::Rc::new(move |env: &mut Environment| {
                            if env.sim.get_aura(unit, "Mana Spring Totem").is_none() {
                                return;
                            }
                            // Rank 4's 10 mana every 2 sec, the party buff's totem (10494).
                            let row = must_find(10494);
                            let mp5 = row.effect_n(1).base_points * 5.0 / 2.0 * share;
                            let aura = env
                                .sim
                                .register_permanent_gear_aura(unit, "Strong Current".to_string());
                            env.sim
                                .attach_stats_buff(aura, Stats::from_pairs(&[(Stat::MP5, mp5)]));
                        }));
                }
                if reached(8) {
                    let aura = forever_status_aura(env, &name, 8);
                    let mp5 = must_find(28820).effect_n(1).base_points;
                    env.sim
                        .attach_stats_buff(aura, Stats::from_pairs(&[(Stat::MP5, mp5)]));
                }
            }
            // Lay Hands (28774): Lay on Hands's cooldown 12 min shorter (4). The Holy Paladin's
            // spells carry the class's own masks rather than the client's.
            "Redemption Armor" if reached(4) => {
                let unit = env.player;
                let row = must_find(28774);
                env.sim.add_static_mod(
                    unit,
                    super::spell_mod::SpellModConfig {
                        class_mask: crate::classes::paladin::prepare::masks::LAY_ON_HANDS,
                        kind: super::spell_mod::SpellModType::CooldownFlat,
                        time_value: row.effect_n(1).time_value(),
                        ..super::spell_mod::SpellModConfig::default()
                    },
                );
            }
            "Vestments of Transcendence" if reached(8) => {
                env.healer_gear
                    .extras
                    .push(("Greater Heal", HealExtra::ThenHot(6077)));
            }
            "Vestments of Faith" => {
                // Armor of Faith (28809, 28810): a Greater Heal crit shields its player for 500.
                if reached(4) {
                    let absorb = must_find(28810).effect_n(1).base_points;
                    env.healer_gear
                        .extras
                        .push(("Greater Heal", HealExtra::CritShield(absorb)));
                }
                // Epiphany (28802, 28804): a spell cast's 5% chance at 24 mana per 5 sec for
                // 30 sec.
                if reached(8) {
                    let unit = env.player;
                    let aura = forever_status_aura(env, &name, 8);
                    let row = must_find(28804);
                    env.sim.new_temporary_stats_aura(
                        unit,
                        EPIPHANY,
                        &ActionId::spell(row.id),
                        Stats::from_pairs(&[(Stat::MP5, row.effect_n(1).base_points)]),
                        row.duration(),
                    );
                    let trigger = ProcTrigger {
                        proc_chance: f64::from(must_find(28802).proc_chance) / 100.0,
                        ..spell_cast_trigger("Epiphany")
                    };
                    env.sim.attach_proc_trigger(aura, &trigger);
                }
            }
            // Healing Touch (28719): a Healing Touch crit returns 30% of its cost.
            "Dreamwalker Raiment" if reached(8) => {
                let aura = forever_status_aura(env, &name, 8);
                let trigger = ProcTrigger {
                    callback: CallbackMask::ON_HEAL_DEALT,
                    ..cast_trigger("Healing Touch Refund", 28719, flags(DRUID, HEALING_TOUCH))
                };
                env.sim.attach_proc_trigger(aura, &trigger);
                env.healer_gear.mana_listeners.push(ManaListener {
                    aura,
                    rng_label: "",
                    chance: 1.0,
                    trigger,
                    mana: Mana::CostShare(0.3),
                    metrics_spell_id: 28719,
                    crit_only: true,
                });
            }
            // Improved Chain Heal (21899): 30% more healing on each player past the first.
            "The Ten Storms" if reached(3) => {
                let row = must_find(21899);
                env.healer_gear
                    .past_first
                    .push((flags(SHAMAN, CHAIN_HEAL), 1.0 + row.effect_n(1).percent()));
            }
            _ => {}
        }
    }
}

fn or(a: [u32; 4], b: [u32; 4]) -> [u32; 4] {
    [a[0] | b[0], a[1] | b[1], a[2] | b[2], a[3] | b[3]]
}

/// The status aura of a set bonus the reference leaves empty: the one it registered.
fn forever_status_aura(env: &mut Environment, set: &str, pieces: i32) -> AuraId {
    let unit = env.player;
    env.sim
        .register_permanent_gear_aura(unit, format!("{set} {pieces}P"))
}

/// A listener on the casts of the spells a client mask names.
fn cast_trigger(name: &str, spell_id: i32, class_flags: ClassFlags) -> ProcTrigger {
    ProcTrigger {
        name: name.to_string(),
        action_id: ActionId::spell(spell_id),
        callback: CallbackMask::ON_CAST_COMPLETE,
        proc_mask: ProcMask::SPELL_HEALING,
        class_flags,
        ..ProcTrigger::default()
    }
}

/// The healing items the reference registers nothing for.
fn apply_items(env: &mut Environment) {
    let unit = env.player;
    let registered = &tables().item_effect_ids;
    let mut seen = Vec::new();
    let class = env.sim.character(unit).class.clone();
    for slot in 0..env.sim.character(unit).equipment.len() {
        let item = env.sim.character(unit).equipment[slot].clone();
        let usable = item.class_allowlist.is_empty() || item.class_allowlist.contains(&class);
        if item.id == 0 || seen.contains(&item.id) || registered.contains(&item.id) || !usable {
            continue;
        }
        seen.push(item.id);
        match item.id {
            // Idol of Health (27846): Healing Touch's cast time -0.15 sec. Tidal Totem
            // (1291076): Healing Wave costs 5% less. Totem of Urgency (1306448): Lesser
            // Healing Wave +4% crit. Idol of Swiftness (1306488): Swiftmend's cooldown -3 sec.
            22399 | 272431 | 279249 | 279250 => {
                let buff = equip_buff(item.id);
                let aura = env
                    .sim
                    .register_permanent_gear_aura(unit, must_find(buff).name.clone());
                apply_bonus_row(env, aura, must_find(buff));
            }
            // Totem of Life (27855): Lesser Healing Wave heals for up to 80 more, a class script
            // the reference's Libram of Light treats as healing on the roll.
            22396 => {
                let row = must_find(27855);
                env.healer_gear
                    .direct_bonus
                    .push((row.effect_n(1).class_flags, row.effect_n(1).base_points));
            }
            // Idol of Longevity (28847): each Healing Touch returns 25 mana (28848).
            23004 => mana_item(
                env,
                "Healing Touch Refund",
                28847,
                flags(DRUID, HEALING_TOUCH),
                must_find(28848).effect_n(1).base_points,
            ),
            // Totem of Flowing Water (28849): each Lesser Healing Wave returns 10 mana (28850).
            // The row's own mask names another spell; the tooltip names Lesser Healing Wave.
            23005 => mana_item(
                env,
                "Lesser Healing Wave Refund",
                28849,
                flags(SHAMAN, LESSER_HEALING_WAVE),
                must_find(28850).effect_n(1).base_points,
            ),
            // Hazza'rah's Charm of Healing (Rapid Healing, 24546) and Wushoolay's Charm of
            // Nature (Nimble Healing Touch, 24542): on use, 15 sec of their rows' cast time
            // and cost mods.
            19958 | 19955 => shared_on_use::apply(env, OnUseKind::Aura, item.id),
            // Blessed Prayer Beads (24354): on use, heals for 648. The shared constructor makes
            // it a survival cooldown, which waits on the healer's own health; a healer presses
            // it for its heal.
            19990 => on_use_item_spell(env, item.id, cooldown_type::DPS, None),
            // Enamored Water Spirit (24854): on use, a Mana Spring Totem for 24 sec.
            20503 => on_use_item_spell(env, item.id, cooldown_type::MANA, None),
            // Idol of Synthesis (1291075): each heal over time up takes 1 sec off Swiftmend's
            // cooldown as it is cast.
            272429 => {
                let cut =
                    must_find(1291075).effect_n(1).base_points as i64 * super::sim::MILLISECOND;
                env.healer_gear
                    .extras
                    .push(("Swiftmend", HealExtra::CooldownPerHot(cut)));
            }
            // Idol of Rejuvenation (27853): Regrowth's direct heal 5% more on a target under
            // the druid's Rejuvenation, read as any Rejuvenation of the druid's up.
            22398 => {
                let bonus = 1.0 + must_find(27853).effect_n(1).base_points / 100.0;
                env.healer_gear
                    .extras
                    .push(("Regrowth", HealExtra::HotBonus(REJUVENATION, bonus)));
            }
            19950 => zandalarian_hero_charm(env),
            21625 => scarab_brooch(env),
            _ => {}
        }
    }
}

/// An item's on-use spell on its effect's cooldowns, a major cooldown, with the aura it brings
/// up as its buff.
fn on_use_item(env: &mut Environment, item: i32, buff: AuraId) {
    on_use_item_spell(env, item, cooldown_type::DPS, Some(buff));
}

/// An item's on-use spell on its effect's cooldowns, a major cooldown of the type given.
fn on_use_item_spell(env: &mut Environment, item: i32, cooldown_type: u32, buff: Option<AuraId>) {
    let unit = env.player;
    let Some(effect) = shared_on_use::on_use_effects_for(item).into_iter().next() else {
        return;
    };
    let cast = shared_on_use::on_use_cast(&mut env.sim, unit, &effect);
    let school = must_find(effect.i32("buff_id")).spell_school();
    let spell = env.sim.register_spell(
        unit,
        SpellConfig {
            action_id: ActionId::item(item),
            spell_school: if school == 0 {
                super::spell::school::HOLY
            } else {
                school
            },
            flags: SpellFlag::NO_ON_CAST_COMPLETE,
            cast,
            related_self_buff: buff,
            // A heal on use (Blessed Prayer Beads) deals through the spell's multipliers.
            proc_mask: ProcMask::EMPTY,
            damage_multiplier: 1.0,
            threat_multiplier: 1.0,
            ..SpellConfig::default()
        },
    );
    env.sim.add_major_cooldown(
        unit,
        MajorCooldown {
            spell,
            priority: 0,
            cooldown_type,
            allow_spell_queueing: false,
            timings: Vec::new(),
        },
    );
}

/// A heal a set bonus deals on its own, registered so its healing counts on its own spell.
fn passive_heal_spell(env: &mut Environment, unit: UnitId, spell_id: i32) -> SpellId {
    let row = must_find(spell_id);
    env.sim.register_spell(
        unit,
        SpellConfig {
            action_id: ActionId::spell(spell_id),
            spell_school: row.spell_school(),
            proc_mask: ProcMask::EMPTY,
            flags: SpellFlag::PROC | SpellFlag::HELPFUL,
            damage_multiplier: 1.0,
            threat_multiplier: 1.0,
            ..SpellConfig::default()
        },
    )
}

/// Zandalarian Hero Charm: on use, Unstable Power (24658) for 20 sec, up to 204 spell damage and
/// 408 healing, 17 and 34 less with each spell cast: twelve stacks that decay a cast at a time.
/// The client row holds the amounts in its tooltip and a dummy, so they are the tooltip's.
const UNSTABLE_POWER: &str = "Unstable Power";
const UNSTABLE_POWER_WINDOW: &str = "Unstable Power Window";

fn zandalarian_hero_charm(env: &mut Environment) {
    let unit = env.player;
    let stat = env.sim.make_stacking_aura(
        unit,
        StackingStatAura {
            aura: AuraConfig {
                label: UNSTABLE_POWER.to_string(),
                action_id: Some(ActionId::spell(24658)),
                duration: NEVER_EXPIRES,
                max_stacks: 12,
                ..AuraConfig::default()
            },
            bonus_per_stack: Stats::from_pairs(&[
                (Stat::SpellDamage, 17.0),
                (Stat::HealingPower, 34.0),
            ]),
        },
    );
    let window = env.sim.register_aura(
        unit,
        AuraConfig {
            label: UNSTABLE_POWER_WINDOW.to_string(),
            action_id: Some(ActionId::item(19950)),
            duration: 20 * SECOND,
            ..AuraConfig::default()
        },
    );
    env.sim
        .attach_proc_trigger(window, &spell_cast_trigger("Unstable Power"));
    on_use_item(env, 19950, stat.aura);
}

/// The casts of the spells a damage or healing spell cast reaches.
fn spell_cast_trigger(name: &str) -> ProcTrigger {
    ProcTrigger {
        name: name.to_string(),
        callback: CallbackMask::ON_CAST_COMPLETE,
        proc_mask: ProcMask::SPELL_DAMAGE | ProcMask::SPELL_HEALING,
        ..ProcTrigger::default()
    }
}

/// Scarab Brooch: on use, Persistent Shield (26467) for 30 sec: each magical heal shields its
/// target for 15% of the healing it did.
const PERSISTENT_SHIELD: &str = "Persistent Shield";

fn scarab_brooch(env: &mut Environment) {
    let unit = env.player;
    let aura = env.sim.register_aura(
        unit,
        AuraConfig {
            label: PERSISTENT_SHIELD.to_string(),
            action_id: Some(ActionId::spell(26467)),
            duration: must_find(26467).duration(),
            ..AuraConfig::default()
        },
    );
    env.sim.attach_proc_trigger(
        aura,
        &ProcTrigger {
            name: PERSISTENT_SHIELD.to_string(),
            callback: CallbackMask::ON_HEAL_DEALT,
            proc_mask: ProcMask::SPELL_HEALING,
            ..ProcTrigger::default()
        },
    );
    on_use_item(env, 21625, aura);
}

/// The buff of an item's equip effect.
fn equip_buff(item: i32) -> i32 {
    super::items::database_item(item)
        .and_then(|db| db.item_effects.first().map(|effect| effect.i32("buff_id")))
        .unwrap_or(0)
}

/// An item that returns mana on the casts its mask names.
fn mana_item(env: &mut Environment, name: &str, spell_id: i32, class_flags: ClassFlags, mana: f64) {
    let unit = env.player;
    let trigger = cast_trigger(name, spell_id, class_flags);
    let aura = env.sim.make_proc_trigger_aura(unit, &trigger);
    env.healer_gear.mana_listeners.push(ManaListener {
        aura,
        rng_label: "",
        chance: 1.0,
        trigger,
        mana: Mana::Flat(mana),
        metrics_spell_id: spell_id,
        crit_only: false,
    });
}

/// Sudden Insight, the reference's five piece of Vestments of the Devout and Magister's Regalia:
/// a spell cast's 5% chance to restore 200 mana.
fn sudden_insight(env: &mut Environment) {
    let unit = env.player;
    for (set, spell_id) in [
        ("Vestments of the Devout 5P", 450576),
        ("Magister's Regalia 5P", 450527),
    ] {
        let Some(aura) = env.sim.get_aura(unit, set) else {
            continue;
        };
        env.healer_gear.mana_listeners.push(ManaListener {
            aura,
            rng_label: "Sudden Insight",
            chance: 0.05,
            trigger: ProcTrigger {
                callback: CallbackMask::ON_CAST_COMPLETE,
                proc_mask: ProcMask::SPELL_DAMAGE | ProcMask::SPELL_HEALING,
                ..ProcTrigger::default()
            },
            mana: Mana::Flat(must_find(450554).effect_n(1).base_points),
            metrics_spell_id: spell_id,
            crit_only: false,
        });
    }
}

/// Vestments of Faith's Epiphany stats aura.
const EPIPHANY: &str = "Epiphany";
/// Every Rejuvenation rank (client 70425).
const REJUVENATION: &[i32] = &[
    774, 1058, 1430, 2090, 2091, 3627, 8910, 9839, 9840, 9841, 25299,
];

/// The Furious Storm's set auras and the stats aura each one's proc activates.
const FURIOUS_STORM: &[&str] = &["The Elements 5P", "The Five Thunders 4P"];

/// The stat auras this module's procs activate, for the stat aura combinations.
pub(crate) fn stat_auras(env: &Environment) -> Vec<String> {
    let unit = env.player;
    let furious_storm = env.rules == Rules::Forever
        && FURIOUS_STORM
            .iter()
            .any(|set| env.sim.get_aura(unit, set).is_some());
    if !applies(env) {
        if env.rules != Rules::Forever {
            return Vec::new();
        }
        let mut labels = stacking_stat_auras(env);
        if furious_storm {
            labels.push("The Furious Storm".to_string());
        }
        return labels;
    }
    let mut labels = Vec::new();
    if furious_storm {
        labels.push("The Furious Storm".to_string());
    }
    for label in [UNSTABLE_POWER, EPIPHANY] {
        if env.sim.get_aura(unit, label).is_some() {
            labels.push(label.to_string());
        }
    }
    labels.extend(stacking_stat_auras(env));
    labels
}

/// The stacks of a stacking on-use trinket the reference registers.
fn stacking_stat_auras(env: &Environment) -> Vec<String> {
    let mut labels = Vec::new();
    for registration in super::forever_items_generated::ITEMS {
        if let Registration::Stacking(config) = registration {
            let label = format!("{} Proc", config.name);
            if env.sim.get_aura(env.player, &label).is_some() {
                labels.push(label);
            }
        }
    }
    labels
}

/// The healer gear's listeners: mana on cast and The Furious Storm.
pub(crate) fn export_effects(env: &Environment, unrepresented: &mut Vec<String>) -> Vec<Value> {
    if !applies(env) {
        // The Furious Storm acts on damage spells too, so Forever's rules describe it for any
        // spec.
        return if env.rules == Rules::Forever {
            furious_storm_effects(env)
        } else {
            Vec::new()
        };
    }
    let unit = env.player;
    let book = &env.sim.unit(unit).spellbook;
    let mut effects = Vec::new();
    for listener in &env.healer_gear.mana_listeners {
        let spells: Vec<Value> = proc_trigger_spells(env, &listener.trigger)
            .into_iter()
            .filter_map(|position| {
                let spell = env.sim.spell(book[position]);
                let mana = match listener.mana {
                    Mana::Flat(mana) => mana,
                    Mana::BaseCostShare(share) => match &spell.cost {
                        Some(cost) if cost.resource == Resource::Mana => {
                            share * f64::from(cost.base_cost)
                        }
                        _ => return None,
                    },
                    Mana::CostShare(share) => match &spell.cost {
                        Some(cost) if cost.resource == Resource::Mana => {
                            share * spell.default_cast.cost
                        }
                        _ => return None,
                    },
                };
                (mana > 0.0).then(|| json!({"spell": position, "mana": mana}))
            })
            .collect();
        let aura = env.sim.aura(listener.aura);
        effects.push(json!({
            "kind": "mana_on_cast", "trigger_aura": aura.label,
            "rng_label": if listener.rng_label.is_empty() { aura.label.as_str() } else { listener.rng_label },
            "proc_chance": listener.chance, "spells": spells,
            "metrics_action_id": {"spell_id": listener.metrics_spell_id},
            "crit_only": listener.crit_only,
        }));
    }
    for share in &env.healer_gear.heal_shares {
        effects.push(json!({
            "kind": "shield_on_heal", "item_id": 0, "aura": env.sim.aura(share.aura).label,
            "share": share.share, "chance": share.chance, "heal": true, "spell_id": share.spell_id,
        }));
    }
    // Rotmender's Raiment (5): Rotmending, its five ticks at once.
    if let Some(aura) = env.healer_gear.rotmending {
        let row = must_find(1321586);
        let tick = row.effect_n(1);
        let ticks = (row.duration() / tick.period()).max(1) as f64;
        let position = book
            .iter()
            .position(|spell| env.sim.spell(*spell).action_id == ActionId::spell(row.id));
        let listener = ProcTrigger {
            callback: CallbackMask::ON_CAST_COMPLETE,
            proc_mask: ProcMask::SPELL_HEALING,
            ..ProcTrigger::default()
        };
        let pseudo = &env.sim.unit(unit).pseudo_stats;
        if let Some(position) = position {
            effects.push(json!({
                "kind": "spell_data_heal_proc", "trigger_aura": env.sim.aura(aura).label,
                "trigger_spells": proc_trigger_spells(env, &listener), "outcome": [],
                "require_damage": false, "proc_chance": 1.0, "spell": position, "can_crit": false,
                "healing_dealt_multiplier": pseudo.healing_dealt_multiplier,
                "healing_taken_multiplier": pseudo.healing_taken_multiplier,
                "table_healing_dealt_multiplier": env.attack_table(unit, unit).healing_dealt_multiplier,
                "bonus_healing_taken": pseudo.bonus_healing_taken,
                "average": tick.base_points * ticks, "variance": 0.0, "casts": true,
            }));
        } else {
            unrepresented.push("Rotmending has no spell".to_string());
        }
    }
    // Nature's Swiftness (the Shaman's 16188, the Druid's 17116): the next heal with a cast
    // time is instant, and its cast spends the aura.
    for spell_id in [16188, 17116] {
        let id = ActionId::spell(spell_id);
        let cast = book
            .iter()
            .any(|spell| env.sim.spell(*spell).action_id == id);
        if !cast || env.sim.get_aura(unit, "Nature's Swiftness").is_none() {
            continue;
        }
        let Some(changes) = aura_spell_changes(env, "Nature's Swiftness") else {
            continue;
        };
        let changes: Vec<Value> = changes
            .into_iter()
            .filter(|change| {
                change["kind"] == "cast_time_percent"
                    && change["spell"]
                        .as_u64()
                        .and_then(|position| book.get(position as usize))
                        .is_some_and(|spell| env.sim.spell(*spell).default_cast.cast_time > 0)
            })
            .collect();
        effects.push(json!({
            "kind": "instant_cast_aura", "spell_id": spell_id, "aura": "Nature's Swiftness",
            "changes": changes,
        }));
    }
    // Vestments of Faith (8): Epiphany.
    if let (Some(trigger), Some(aura)) = (
        env.sim.get_aura(unit, "Vestments of Faith 8P"),
        env.sim.get_aura(unit, EPIPHANY),
    ) {
        let row = must_find(28804);
        let bonus = Stats::from_pairs(&[(Stat::MP5, row.effect_n(1).base_points)]);
        let id = ActionId::spell(row.id);
        effects.push(json!({
            "kind": "spell_data_stat_proc", "trigger_aura": env.sim.aura(trigger).label,
            "aura": env.sim.aura(aura).label,
            "trigger_spells": proc_trigger_spells(env, &spell_cast_trigger("")),
            "callbacks": ["on_cast_complete"], "landed_only": false, "require_damage": false,
            "proc_chance": f64::from(must_find(28802).proc_chance) / 100.0,
            "gain_log": format!("Gained {} from {}.", flat_string(&bonus), action_id_string(&id)),
            "expire_log": format!("Lost {} from fading {}.", flat_string(&bonus), action_id_string(&id)),
        }));
    }
    effects.extend(furious_storm_effects(env));
    effects
}

/// The Furious Storm (The Elements 5, The Five Thunders 4): a damage or healing spell's cast
/// has a 4% chance at 65 spell power for 10 seconds.
fn furious_storm_effects(env: &Environment) -> Vec<Value> {
    let unit = env.player;
    let mut effects = Vec::new();
    for set in FURIOUS_STORM {
        let (Some(trigger), Some(aura)) = (
            env.sim.get_aura(unit, set),
            env.sim.get_aura(unit, "The Furious Storm"),
        ) else {
            continue;
        };
        let listener = ProcTrigger {
            callback: CallbackMask::ON_CAST_COMPLETE,
            proc_mask: ProcMask::SPELL_DAMAGE | ProcMask::SPELL_HEALING,
            ..ProcTrigger::default()
        };
        let bonus = Stats::from_pairs(&[(Stat::SpellDamage, 65.0), (Stat::HealingPower, 65.0)]);
        let id = env.sim.aura(aura).action_id.clone().unwrap_or_default();
        effects.push(json!({
            "kind": "spell_data_stat_proc", "trigger_aura": env.sim.aura(trigger).label,
            "aura": env.sim.aura(aura).label,
            "trigger_spells": proc_trigger_spells(env, &listener),
            "callbacks": ["on_cast_complete"], "landed_only": false, "require_damage": false,
            "proc_chance": 0.04,
            "gain_log": format!("Gained {} from {}.", flat_string(&bonus), action_id_string(&id)),
            "expire_log": format!("Lost {} from fading {}.", flat_string(&bonus), action_id_string(&id)),
        }));
    }
    effects
}

/// An on-use item of this module whose aura carries spell mods: each spell's changes while the
/// aura is up, read from a separate reset simulation with it active.
pub(crate) fn use_item_effect(
    env: &Environment,
    spell: SpellId,
    item: i32,
    unrepresented: &mut Vec<String>,
) -> Option<Value> {
    if !applies(env) {
        // A stacking on-use trinket under Forever's rules, for any spec: Badge of the
        // Swarmguard is a rogue's or a warrior's. Go's exporter has no effect for it.
        if env.rules == Rules::Forever {
            return stacking_cd_effect(env, spell, item, unrepresented);
        }
        return None;
    }
    match item {
        19958 | 19955 | 19956 => {}
        // Enamored Water Spirit's totem: its Mana Spring row's mana every period, for the
        // totem's duration, on the totem's metrics.
        20503 => {
            let tick = must_find(24853).effect_n(1);
            let ticks = (must_find(24854).duration() / tick.period()).max(1);
            return Some(json!({
                "kind": "second_wind", "item_id": item, "mana": tick.base_points,
                "ticks": ticks, "period_ns": tick.period(), "metrics_spell_id": 24854,
                "min_deficit": tick.base_points * ticks as f64,
            }));
        }
        19950 => {
            return Some(json!({
                "kind": "stacking_cd_on_use", "item_id": item, "stat_aura": UNSTABLE_POWER,
                "window_aura": UNSTABLE_POWER_WINDOW, "decay": true,
                "trigger_spells": proc_trigger_spells(env, &spell_cast_trigger("")),
                "callbacks": ["on_cast_complete"], "landed_only": false, "proc_chance": 1.0,
            }))
        }
        21625 => {
            return Some(json!({
                "kind": "shield_on_heal", "item_id": item, "aura": PERSISTENT_SHIELD,
                "share": 0.15,
            }))
        }
        19344 => return Some(natural_alignment_crystal(env, item)),
        _ => return stacking_cd_effect(env, spell, item, unrepresented),
    }
    let db = super::items::database_item(item)?;
    let row = db
        .item_effects
        .iter()
        .find(|effect| effect.has("on_use"))
        .map(|effect| must_find(effect.i32("buff_id")))?;
    let Some(changes) = aura_spell_changes(env, &row.name) else {
        unrepresented.push(format!("item {item} has no spell mod aura"));
        return Some(Value::Null);
    };
    Some(json!({
        "kind": "spell_mod_aura_on_use", "item_id": env.sim.spell(spell).action_id.item_id,
        "aura": row.name, "changes": changes,
    }))
}

/// Natural Alignment Crystal: on use, Nature Aligned (23734) for 20 sec, 20% more damage and
/// healing from spells and 20% more mana spent on them (the row's percent damage, percent
/// healing and school cost auras, which the parser keeps on the player's pseudo stats). A
/// healer's heals take it as a 20% bonus on the heal and its ticks, every magic spell the cost.
fn natural_alignment_crystal(env: &Environment, item: i32) -> Value {
    let row = must_find(23734);
    let share = row.effect_n(1).percent();
    let book = &env.sim.unit(env.player).spellbook;
    let mut changes = Vec::new();
    for (position, id) in book.iter().enumerate() {
        let spell = env.sim.spell(*id);
        if spell.spell_school & !super::spell::school::PHYSICAL == 0 {
            continue;
        }
        if spell.proc_mask.matches(ProcMask::SPELL_HEALING) {
            changes.push(json!({"spell": position, "kind": "damage_done_percent", "value": share}));
            if spell.dots.iter().any(Option::is_some) {
                changes.push(
                    json!({"spell": position, "kind": "dot_damage_done_percent", "value": share}),
                );
            }
        }
        if spell
            .cost
            .as_ref()
            .is_some_and(|cost| cost.resource == Resource::Mana)
        {
            changes
                .push(json!({"spell": position, "kind": "power_cost_percent_add", "value": share}));
        }
    }
    json!({
        "kind": "spell_mod_aura_on_use", "item_id": item, "aura": row.name, "changes": changes,
    })
}

/// A stacking on-use trinket the reference registers (`NewStackingStatBonusCD`), as Talisman of
/// Ascendance: its window and stats auras, and the spells its trigger hears at the stack proc's
/// chance. A trigger on a proc rate is not described.
fn stacking_cd_effect(
    env: &Environment,
    spell: SpellId,
    item: i32,
    unrepresented: &mut Vec<String>,
) -> Option<Value> {
    let config = super::forever_items_generated::ITEMS
        .iter()
        .find_map(|registration| match registration {
            Registration::Stacking(config) if config.id == item => Some(*config),
            _ => None,
        })?;
    let unit = env.player;
    let stat = env.sim.spell(spell).related_self_buff?;
    let stat_label = env.sim.aura(stat).label.clone();
    let window = env
        .sim
        .unit(unit)
        .auras
        .iter()
        .map(|aura| env.sim.aura(*aura).label.clone())
        .find(|label| label.starts_with(&format!("{} Limit Aura", config.name)))
        .unwrap_or_else(|| stat_label.clone());
    let effect = shared_on_use::on_use_effects_for(item).into_iter().next()?;
    let stack_proc = effect.message("stack_proc");
    let trigger = stacking_trigger(&config);
    let trigger_spells = proc_trigger_spells(env, &trigger);
    let ppm = stack_proc.map_or(0.0, |proc| proc.f64("ppm"));
    let icd = stack_proc.is_some_and(|proc| proc.i32("icd_ms") != 0);
    if window == stat_label || (icd && !trigger_spells.is_empty()) {
        unrepresented.push(format!(
            "stacking trinket {item} stacks on a cooldown or its own window"
        ));
        return Some(Value::Null);
    }
    // Go `attachStackingCDTrigger`: a rate rolls the item's procs per minute manager over the
    // trigger's mask (`dpmForMask`), as Badge of the Swarmguard's 10 a minute on melee and
    // ranged hits, and no fixed chance before it.
    let manager = if ppm != 0.0 && !trigger_spells.is_empty() {
        let source = EffectSource {
            id: item,
            is_enchant: false,
        };
        let Some(dpm) = dpm_for_mask(&env.sim, unit, source, ppm, config.proc_mask) else {
            unrepresented.push(format!("stacking trinket {item} has no proc manager"));
            return Some(Value::Null);
        };
        let book = &env.sim.unit(unit).spellbook;
        let chances: Vec<Value> = trigger_spells
            .iter()
            .filter_map(|&position| {
                let mask = env.sim.spell(book[position]).proc_mask;
                dpm.proc_masks
                    .iter()
                    .zip(&dpm.proc_chances)
                    .find(|(manager_mask, _)| manager_mask.matches(mask))
                    .map(|(_, chance)| json!({"spell": position, "chance": chance}))
            })
            .collect();
        Some(chances)
    } else {
        None
    };
    let chance = stack_proc
        .map(|proc| proc.f64("proc_chance"))
        .filter(|chance| *chance != 0.0)
        .unwrap_or(1.0);
    let mut callbacks = Vec::new();
    for (bit, name) in [
        (CallbackMask::ON_SPELL_HIT_DEALT, "on_spell_hit_dealt"),
        (CallbackMask::ON_HEAL_DEALT, "on_heal_dealt"),
        (CallbackMask::ON_CAST_COMPLETE, "on_cast_complete"),
    ] {
        if config.callback.0 & bit.0 != 0 {
            callbacks.push(name);
        }
    }
    let mut effect = json!({
        "kind": "stacking_cd_on_use", "item_id": item, "stat_aura": stat_label,
        "window_aura": window, "decay": false,
        "trigger_spells": trigger_spells, "callbacks": callbacks,
        "landed_only": config.outcome == HitOutcome::LANDED, "proc_chance": chance,
    });
    if let Some(chances) = manager {
        effect["proc_chance"] = json!(1.0);
        effect["chances"] = json!(chances);
        effect["require_damage"] = json!(config.require_damage_dealt);
        effect["rng_label"] = json!(config.name);
    }
    Some(effect)
}

fn stacking_trigger(config: &StackingStatBonusCd) -> ProcTrigger {
    ProcTrigger {
        callback: config.callback,
        proc_mask: config.proc_mask,
        spell_flags: config.spell_flags,
        spell_flags_exclude: config.spell_flags_exclude,
        can_proc_from_procs: config.can_proc_from_procs,
        outcome: config.outcome,
        ..ProcTrigger::default()
    }
}

/// What an aura changes on each spell while it is up, by spellbook position, as the runtime's
/// mods name the changes.
fn aura_spell_changes(env: &Environment, label: &str) -> Option<Vec<Value>> {
    let mut fresh = env.fresh();
    let unit = fresh.player;
    let aura = fresh.sim.get_aura(unit, label)?;
    let book = fresh.sim.unit(unit).spellbook.clone();
    let read = |sim: &Sim| -> Vec<BTreeMap<&'static str, f64>> {
        book.iter()
            .map(|id| {
                let spell = sim.spell(*id);
                let mut fields = BTreeMap::new();
                fields.insert("cast_time_percent", spell.cast_time_multiplier);
                fields.insert("cast_time_flat", spell.default_cast.cast_time as f64);
                fields.insert("global_cooldown_flat", spell.default_cast.gcd as f64);
                fields.insert("bonus_crit_percent", spell.bonus_crit_percent);
                fields.insert("damage_done_flat", spell.damage_multiplier_additive);
                if let Some(cost) = &spell.cost {
                    fields.insert("power_cost_percent_add", cost.additive_percent_modifier);
                    fields.insert("power_cost_flat", f64::from(cost.flat_modifier));
                }
                fields
            })
            .collect()
    };
    let before = read(&fresh.sim);
    fresh.sim.activate(aura);
    let after = read(&fresh.sim);
    let mut changes = Vec::new();
    for (position, (was, now)) in before.iter().zip(&after).enumerate() {
        for (kind, value) in now {
            // The difference of two sums, to the nanosecond or a billionth, as the row states.
            let change = ((value - was.get(kind).copied().unwrap_or(0.0)) * 1e9).round() / 1e9;
            if change != 0.0 {
                changes.push(json!({"spell": position, "kind": kind, "value": change}));
            }
        }
    }
    Some(changes)
}

/// The heals' export with the healer's gear: a rank's direct bonus, its jumps and its bonus past
/// the first player, by its client mask.
pub(crate) fn heal_shape(env: &Environment, row: &Row, heal: &mut Value) {
    if !applies(env) {
        return;
    }
    let gear = &env.healer_gear;
    let names = |mask: &ClassFlags| super::spell_mod::class_flags_matches(mask, &row.class_flags);
    if let Some(direct) = heal.get_mut("direct") {
        let bonus: f64 = gear
            .direct_bonus
            .iter()
            .filter(|(mask, _)| names(mask))
            .map(|(_, bonus)| bonus)
            .sum();
        if bonus != 0.0 {
            direct["average"] = json!(direct["average"].as_f64().unwrap_or(0.0) + bonus);
        }
        if let Some((_, targets, falloff)) = gear.chains.iter().find(|(mask, _, _)| names(mask)) {
            if heal["targets"].as_u64() == Some(1) {
                heal["targets"] = json!(targets);
                heal["jump_multiplier"] = json!(falloff);
                heal["smart"] = json!("chain");
            }
        }
    }
    for (name, extra) in &gear.extras {
        if *name != row.name {
            continue;
        }
        match extra {
            HealExtra::ThenHot(id) if heal.get("direct").is_some() => heal["then_hot"] = json!(id),
            HealExtra::CritShield(absorb) if heal.get("direct").is_some() => {
                heal["crit_shield"] = json!(absorb)
            }
            HealExtra::HotBonus(ids, multiplier) if heal.get("direct").is_some() => {
                heal["hot_bonus"] = json!({"spell_ids": ids, "multiplier": multiplier})
            }
            HealExtra::CooldownPerHot(ns) => heal["cooldown_per_hot_ns"] = json!(ns),
            _ => {}
        }
    }
    let past: f64 = gear
        .past_first
        .iter()
        .filter(|(mask, _)| names(mask))
        .map(|(_, multiplier)| multiplier)
        .product();
    if past != 1.0 {
        heal["past_first_multiplier"] = json!(past);
    }
}
