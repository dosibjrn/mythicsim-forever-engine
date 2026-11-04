//! MythicSim's Forever results: the party's mana totems whose MP5 the player's mana ticks carry.
//!
//! Mana Spring Totem's party buff and Mana Tide Totem are MP5 auras, so their mana arrives inside
//! the regular mana ticks and the results listed it as "Regen while casting" and "Regen while not
//! casting", which players read as the totem doing nothing (MythicSim's Discord, 3 November
//! 2026). Each source here names the totem and the MP5 it adds; the runtime moves that share of
//! every tick to the totem's own resource metrics. The mana gained is the same: only the row it
//! is reported under changes.

use super::buffs::generated::{MANA_SPRING_TOTEM, MANA_TIDE_TOTEMS};
use super::env::Environment;
use super::sim::AuraId;
use super::stats::Stat;
use crate::contracts::prepared_v2::{ActionId, Mp5Source};

/// The party buff's label, the raid's Mana Spring Totem.
const PARTY_MANA_SPRING: &str = "Mana Spring Totem (External)";
/// The Restoration or Elemental shaman's own Mana Spring Totem, which shares the party buff's
/// exclusive category.
const SELF_MANA_SPRING: &str = "Mana Spring Totem (Self)";
/// The raid's external Mana Tide Totem.
const EXTERNAL_MANA_TIDE: &str = "Mana Tide Totem (External)";
/// The Restoration shaman's own Mana Tide Totem.
const SELF_MANA_TIDE: &str = "Mana Tide Totem (Self)";

/// Each Mana Spring rank's party buff (the totem's periodic energize) and the totem that casts
/// it, from the client's rows: the buff names no totem, so the row reads as the totem's.
const MANA_SPRING_TOTEM_RANKS: [(i32, i32); 4] =
    [(5677, 5675), (10491, 10495), (10493, 10496), (10494, 10497)];

/// The player's MP5 with `aura` up and with it down, from a reset simulation of each.
fn mp5_of(env: &Environment, label: &str) -> Option<f64> {
    let read = |up: bool| -> Option<f64> {
        let mut fresh = env.fresh();
        let player = fresh.player;
        let aura: AuraId = fresh.sim.get_aura(player, label)?;
        if up {
            fresh.sim.activate(aura);
        } else {
            fresh.sim.deactivate(aura);
        }
        Some(fresh.sim.unit(player).stats[Stat::MP5])
    };
    let mp5 = read(true)? - read(false)?;
    (mp5 > 0.0).then_some(mp5)
}

/// The sources of the player's MP5 that the results name, under Forever's rules. `stat_auras`
/// are the auras the stat combinations follow: a source that comes and goes must be one.
pub(crate) fn export(env: &Environment, stat_auras: &[String]) -> Vec<Mp5Source> {
    let player = env.player;
    let has = |label: &str| env.sim.get_aura(player, label).is_some();
    let mut sources = Vec::new();
    // The party buff and the shaman's own totem share an exclusive category, so only the one
    // the player has alone is a plain MP5 bonus.
    match (has(PARTY_MANA_SPRING), has(SELF_MANA_SPRING)) {
        (true, false) => {
            let aura = env
                .sim
                .get_aura(player, PARTY_MANA_SPRING)
                .expect("checked");
            let buff = env
                .sim
                .aura(aura)
                .action_id
                .as_ref()
                .map_or(MANA_SPRING_TOTEM.spell, |id| id.spell_id);
            let totem = MANA_SPRING_TOTEM_RANKS
                .iter()
                .find(|(rank, _)| *rank == buff)
                .map_or(buff, |(_, totem)| *totem);
            if let Some(mp5) = mp5_of(env, PARTY_MANA_SPRING) {
                sources.push(Mp5Source {
                    action_id: ActionId {
                        spell_id: totem,
                        ..ActionId::default()
                    },
                    mp5,
                    aura: None,
                });
            }
        }
        (false, true) => sources.extend(aura_source(env, SELF_MANA_SPRING, stat_auras, None)),
        _ => {}
    }
    // The raid's totem's aura is the totem's party row; the shaman's own is the cast's.
    sources.extend(aura_source(
        env,
        EXTERNAL_MANA_TIDE,
        stat_auras,
        MANA_TIDE_TOTEMS.cast,
    ));
    sources.extend(aura_source(env, SELF_MANA_TIDE, stat_auras, None));
    sources
}

/// A source that is up while its stat aura is, named after the totem: `totem`, or the aura's own
/// action, the totem's cast.
fn aura_source(
    env: &Environment,
    label: &str,
    stat_auras: &[String],
    totem: Option<i32>,
) -> Option<Mp5Source> {
    if !stat_auras.iter().any(|stat| stat == label) {
        return None;
    }
    let aura = env.sim.get_aura(env.player, label)?;
    let action_id = match totem {
        Some(spell_id) => ActionId {
            spell_id,
            ..ActionId::default()
        },
        None => env.sim.aura(aura).action_id.clone()?,
    };
    Some(Mp5Source {
        action_id,
        mp5: mp5_of(env, label)?,
        aura: Some(label.to_string()),
    })
}
