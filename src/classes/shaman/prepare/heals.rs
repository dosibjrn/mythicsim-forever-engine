//! The Restoration Shaman's heals and heal talents, which Rust alone sims (see
//! `prepare::heals`): every rank of Healing Wave, Lesser Healing Wave, Chain Heal and Healing
//! Stream Totem, and the talent's Riptide, from the client rows of build 1.60.1.70425.
//!
//! Chain Heal lands on three players, each jump at half of the one before, as its row states;
//! under Forever's rules it jumps only to players the party's damage hit lately
//! (`prepare::party_damage`).
//! Healing Stream Totem heals the party of five every two seconds while it stands. Riptide
//! (408521) also makes a Chain Heal cast directly on its target heal 25% more
//! ([`Shaman::riptide_chain_heal_bonus`]); Nature's Swiftness makes the next heal with a cast
//! time instant (`prepare::healer_gear`).

use serde_json::{json, Value};

use crate::contracts::prepared_v2::SmartHeal;
use crate::prepare::env::Environment;
use crate::prepare::heals::{
    apply_heal_talent, register_heal, register_totem_heal, talent_rank, HealShape,
};
use crate::prepare::sim::Rules;
use crate::prepare::sim::{Sim, UnitId};
use crate::prepare::spelldata::store::must_find;

use super::masks;
use super::Shaman;

const HEALING_WAVE: &[i32] = &[331, 332, 547, 913, 939, 959, 8005, 10395, 10396, 25357];
const LESSER_HEALING_WAVE: &[i32] = &[8004, 8008, 8010, 10466, 10467, 10468];
const CHAIN_HEAL: &[i32] = &[1064, 10622, 10623];
/// Riptide, a Forever talent: a heal and a heal over time on one player.
const RIPTIDE: &[i32] = &[408521, 1239242, 1239243];
/// Healing Stream Totem's totems and the heal each one's aura ticks, by rank.
const HEALING_STREAM: &[(i32, i32)] = &[
    (5394, 5672),
    (6375, 6371),
    (6377, 6372),
    (10462, 10460),
    (10463, 10461),
];
const PARTY: u32 = 5;

/// Healing Stream Totem's totem spells, by rank: the ones Mana Tide Totem replaces in the water
/// totem slot.
pub(super) fn healing_stream_totems() -> Vec<i32> {
    HEALING_STREAM.iter().map(|&(totem, _)| totem).collect()
}
/// Riptide's third effect: a dummy aura whose points are the percent a Chain Heal on the target
/// gains (the talent's tooltip, "increases the effectiveness of your Chain Heal casts directly
/// on that target by $s3%"). All three ranks state 25.
const RIPTIDE_CHAIN_HEAL_EFFECT: i32 = 3;

/// The heal talents, as (talent, client row, ranks): Improved Healing Wave's cast time, Tidal
/// Focus's cost, Tidal Mastery's crit, Healing Way, Purification's direct and Riptide periodic
/// healing, Restorative Totems' Healing Stream ticks and Totemic Focus's totem cost.
const HEAL_TALENTS: &[(&str, i32, i32)] = &[
    ("improved_healing_wave", 16182, 5),
    ("tidal_focus", 16179, 5),
    ("tidal_mastery", 16194, 5),
    ("healing_way", 29206, 3),
    ("purification", 16178, 5),
    ("restorative_totems", 16187, 5),
    ("totemic_focus", 16173, 5),
];

impl Shaman {
    /// Registers every heal rank the shaman knows.
    pub(super) fn register_heals(&mut self, sim: &mut Sim, unit: UnitId) {
        let mut ranks = Vec::new();
        for &id in HEALING_WAVE.iter().chain(LESSER_HEALING_WAVE) {
            ranks.push(register_heal(
                sim,
                unit,
                must_find(id),
                masks::HEAL,
                HealShape::SINGLE,
                None,
            ));
        }
        for &id in CHAIN_HEAL {
            let row = must_find(id);
            let heal = row.heal_effect();
            let shape = HealShape {
                targets: u32::try_from(heal.chain_targets.max(1)).unwrap_or(1),
                jump_multiplier: f64::from(heal.chain_amp),
                smart: Some(SmartHeal::Chain),
            };
            ranks.push(register_heal(sim, unit, row, masks::HEAL, shape, None));
        }
        if self.has_talent("riptide") {
            let timer = sim.new_timer(unit);
            for &id in RIPTIDE {
                ranks.push(register_heal(
                    sim,
                    unit,
                    must_find(id),
                    masks::HEAL,
                    HealShape::SINGLE,
                    Some(timer),
                ));
            }
        }
        for &(totem, heal) in HEALING_STREAM {
            ranks.push(register_totem_heal(
                sim,
                unit,
                must_find(totem),
                must_find(heal),
                masks::HEAL,
                HealShape::party(PARTY),
            ));
        }
        self.heals = ranks;
    }

    /// Riptide's bonus to Chain Heal, under Forever's rules: a Chain Heal's first player heals
    /// 25% more while a Riptide of the shaman's is up.
    ///
    /// The row says "directly on that target", and the sim has no per-player auras (a heal
    /// lands on a player who needs it), so the first Chain Heal player is read as the Riptide
    /// holder: a healer swings Chain Heal at the player it Riptided, and the party model's
    /// Chain Heal starts on the tank, where Riptide goes. The jumps keep their falloff of the
    /// unboosted heal. Riptide does not spend: the row states no consumption.
    pub(super) fn riptide_chain_heal_bonus(&self, env: &Environment, heals: &mut Value) {
        if env.rules != Rules::Forever || !self.has_talent("riptide") {
            return;
        }
        let row = must_find(RIPTIDE[RIPTIDE.len() - 1]);
        let multiplier = 1.0 + row.effect_n(RIPTIDE_CHAIN_HEAL_EFFECT).base_points / 100.0;
        for heal in heals["heals"].as_array_mut().into_iter().flatten() {
            if heal["spell_id"]
                .as_i64()
                .is_some_and(|id| CHAIN_HEAL.iter().any(|rank| i64::from(*rank) == id))
            {
                heal["hot_bonus"] = json!({
                    "spell_ids": RIPTIDE,
                    "multiplier": multiplier,
                    "first_target_only": true,
                });
            }
        }
    }

    /// Applies the heal talents the shaman took.
    pub(super) fn apply_heal_talents(&self, sim: &mut Sim, unit: UnitId) {
        for &(name, spell_id, max_ranks) in HEAL_TALENTS {
            apply_heal_talent(
                sim,
                unit,
                spell_id,
                max_ranks,
                talent_rank(&self.talents, name),
            );
        }
    }
}
