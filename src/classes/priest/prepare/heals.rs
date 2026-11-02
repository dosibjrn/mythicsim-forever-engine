//! The healer Priest's heals and heal talents, which Rust alone sims (see `prepare::heals`): every
//! rank of Lesser Heal, Heal, Greater Heal, Flash Heal, Renew, Prayer of Healing and Power Word:
//! Shield, and the talents' Binding Heal and Prayer of Mending, from the client rows of build
//! 1.60.1.70425.
//!
//! Each heal lands on players who need it: Prayer of Healing on the party of five, Binding Heal
//! on its target and the priest, Prayer of Mending on each of the five players its charges reach
//! (under Forever's rules, as the party's damage moves them: `prepare::party_damage`).
//! Power Word: Shield goes up as often as its cooldown lets it, each time on another player, so
//! Weakened Soul never stops it. Penance's heal is a channel of three bolts, Desperate Prayer a
//! self heal on a ten minute cooldown the rotation casts when it asks to, and Inner Focus makes
//! the next heal free through the masks of its two client effects (`talents::inner_focus`).
//!
//! Under Forever's rules three Discipline and Holy talents act on the heals' targets and history
//! (see `talents::renewed_hope` and `talents::litany_of_light`): Divine Aegis (431622) turns a
//! crit of a heal that is not a heal over time into an absorb of 5% a point of the heal, Renewed
//! Hope (425280) and Litany of Light (1317006) read the party's Weakened Soul and the heal
//! before the one cast.

use crate::contracts::prepared_v2::{ActionId, CritAbsorb, LitanyOfLight, RenewedHope, SmartHeal};
use crate::core::time::NS_PER_SECOND;
use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::heals::{
    apply_heal_talent, register_channel_heal, register_heal, register_heal_from, talent_rank,
    ChannelHeal, HealShape,
};
use crate::prepare::party_damage;
use crate::prepare::sim::{Duration, Rules, Sim, UnitId, MILLISECOND};
use crate::prepare::spell::{ProcMask, SpellConfig, SpellFlag};
use crate::prepare::spelldata::store::must_find;
use crate::prepare::spelldata::{Ladder, Spell as Row};

use super::masks;
use super::Priest;

const LESSER_HEAL: &[i32] = &[2050, 2052, 2053];
const HEAL: &[i32] = &[2054, 2055, 6063, 6064];
const GREATER_HEAL: &[i32] = &[2060, 10963, 10964, 10965, 25314];
const FLASH_HEAL: &[i32] = &[2061, 9472, 9473, 9474, 10915, 10916, 10917];
const RENEW: &[i32] = &[
    139, 6074, 6075, 6076, 6077, 6078, 10927, 10928, 10929, 25315,
];
const PRAYER_OF_HEALING: &[i32] = &[596, 996, 10960, 10961, 25316];
const POWER_WORD_SHIELD: &[i32] = &[17, 592, 600, 3747, 6065, 6066, 10898, 10899, 10900, 10901];
/// Binding Heal, a Forever talent: Flash Heal's numbers on the target and the priest.
const BINDING_HEAL: &[i32] = &[401937, 1240770, 1240771, 1240772, 1240773, 1240774];
/// Holy Binding (1328569, push 113554): the priest's heal is 50% of the Flash Heal.
const HOLY_BINDING_SHARE: f64 = 0.5;
/// Prayer of Mending, a Forever talent: the heal is the cast row's dummy effect, and the buff's
/// five charges each heal a player.
const PRAYER_OF_MENDING: &[i32] = &[401859, 1240826, 1240827];
const PRAYER_OF_MENDING_CHARGES: u32 = 5;
/// How long Prayer of Mending's buff waits on each player: the duration of the buff row the
/// cast's tooltip names (401877 for rank 1, 30 seconds).
fn mending_duration(row: &Row) -> Duration {
    row.refs()
        .into_iter()
        .map(|buff| buff.duration())
        .find(|&duration| duration > 0)
        .unwrap_or(30 * NS_PER_SECOND)
}

/// Penance, a Discipline talent, by rank: the cast rows.
const PENANCE: &[i32] = &[402174, 1240720, 1240721, 1316995];
/// Penance's channel ticks after the bolt that lands with the cast.
const PENANCE_HEAL_TICKS: i32 = 2;

/// Desperate Prayer by rank: a self heal on the ranks' shared ten minute cooldown, at no cost.
const DESPERATE_PRAYER: &[i32] = &[13908, 19236, 19238, 19240, 19241, 19242, 19243];

/// Prayer of Healing reaches the priest's party.
const PARTY: u32 = 5;

/// Divine Aegis, a Discipline talent of three ranks: its first effect is the percent of a crit
/// heal the absorb holds (5, 10, 15; the tooltip names no other). The absorb aura is 431624,
/// which lasts 12 seconds. The talent's proc flags name positive spell hits, not periodic ones.
const DIVINE_AEGIS: i32 = 431622;
const DIVINE_AEGIS_ABSORB: i32 = 431624;
const DIVINE_AEGIS_RANKS: i32 = 3;

/// Renewed Hope, a Discipline talent of five ranks: its first effect is the heals' crit chance
/// in points (2 to 10) and its second the milliseconds off Weakened Soul (1,000 to 5,000).
const RENEWED_HOPE: i32 = 425280;
const RENEWED_HOPE_RANKS: i32 = 5;
/// Weakened Soul, which Power Word: Shield leaves for 15 seconds.
const WEAKENED_SOUL: i32 = 6788;
/// The heals Renewed Hope's tooltip names, by client name.
const RENEWED_HOPE_HEALS: &[&str] = &[
    "Lesser Heal",
    "Heal",
    "Greater Heal",
    "Flash Heal",
    "Binding Heal",
    "Penance",
];

/// Litany of Light, a Holy talent of two ranks: its first effect is the percent of the spell's
/// base mana cost that returns (5, 10).
const LITANY_OF_LIGHT: i32 = 1317006;
const LITANY_OF_LIGHT_RANKS: i32 = 2;

/// The heal talents, as (talent, client row, ranks). Each applies every spell modifier its row
/// states to the heals its client masks name: Spiritual Healing to the direct heals and to
/// Renew's ticks, Holy Specialization's crit, Improved Renew, Divine Fury's cast time, Improved
/// Healing's and Mental Agility's cost, Improved Power Word: Shield's absorb, Soul Warding's
/// cooldown and cost, and Twin Disciplines' healing.
const HEAL_TALENTS: &[(&str, i32, i32)] = &[
    ("spiritual_healing", 14898, 3),
    ("holy_specialization", 14889, 5),
    ("improved_renew", 14908, 3),
    ("divine_fury", 18530, 5),
    ("improved_healing", 14912, 3),
    ("mental_agility", 14520, 3),
    ("improved_power_word_shield", 14748, 3),
    ("soul_warding", 402000, 1),
    ("twin_disciplines", 1225132, 5),
];

impl Priest {
    /// Registers every heal rank the healer knows.
    pub(super) fn register_heals(&mut self, sim: &mut Sim, unit: UnitId) {
        let mut ranks = Vec::new();
        // Divine Aegis under Forever's rules: its absorb counts on its own spell, and each heal
        // that is not a heal over time carries its share.
        let aegis = self.divine_aegis(sim, unit);
        let mut each = |sim: &mut Sim,
                        ids: &[i32],
                        shape: HealShape,
                        shared_cd: bool,
                        aegis: Option<CritAbsorb>| {
            let timer = shared_cd.then(|| sim.new_timer(unit));
            for &id in ids {
                let mut rank = register_heal(sim, unit, must_find(id), masks::HEAL, shape, timer);
                rank.crit_absorb = aegis;
                ranks.push(rank);
            }
        };
        each(sim, LESSER_HEAL, HealShape::SINGLE, false, aegis);
        each(sim, HEAL, HealShape::SINGLE, false, aegis);
        each(sim, GREATER_HEAL, HealShape::SINGLE, false, aegis);
        // Push 113554 under Forever's rules replaces the Binding Heal talent with Holy Binding
        // (1328569) in the same field: each Flash Heal on another player heals the priest for
        // 50% as much (1328570), outside Shadowform, which a healer never takes. The healer's
        // Flash Heal lands on a player who needs it, so the priest's half is a second jump at
        // half the heal. Binding Heal's ranks leave the tree.
        let holy_binding = sim.rules == Rules::Forever && self.talents.bool("binding_heal");
        let flash_heal = if holy_binding {
            HealShape {
                targets: 2,
                jump_multiplier: HOLY_BINDING_SHARE,
                smart: None,
            }
        } else {
            HealShape::SINGLE
        };
        each(sim, FLASH_HEAL, flash_heal, false, aegis);
        each(sim, RENEW, HealShape::SINGLE, false, None);
        each(
            sim,
            PRAYER_OF_HEALING,
            HealShape::party(PARTY),
            false,
            aegis,
        );
        each(sim, POWER_WORD_SHIELD, HealShape::SINGLE, true, None);
        if self.talents.bool("binding_heal") && sim.rules == Rules::GoReference {
            each(sim, BINDING_HEAL, HealShape::party(2), false, None);
        }
        // Desperate Prayer: a self heal the rotation casts when it names it, under Forever's
        // rules, where the healer sims' spellbook holds it. Its crit shields as any direct
        // heal's does.
        if sim.rules == Rules::Forever {
            each(sim, DESPERATE_PRAYER, HealShape::SINGLE, true, aegis);
        }
        // Penance on a friend, a Discipline talent: its heal bolt with the cast and on each of
        // the channel's two ticks a second apart, ranks sharing their category cooldown. The
        // cast's tooltip names the damage bolt first, then the heal bolt.
        if self.talents.bool("penance") {
            let timer = sim.new_timer(unit);
            for &id in PENANCE {
                let row = must_find(id);
                let bolt = row.refs()[1].heal_effect();
                let amount = bolt.average(CHARACTER_LEVEL);
                let mut rank = register_channel_heal(
                    sim,
                    unit,
                    row,
                    ChannelHeal {
                        direct: Some((amount, bolt.variance, bolt.coeff())),
                        tick: amount,
                        tick_coefficient: bolt.coeff(),
                        ticks: PENANCE_HEAL_TICKS,
                        period: NS_PER_SECOND,
                        tick_can_crit: true,
                    },
                    masks::HEAL,
                    HealShape::SINGLE,
                    Some(timer),
                );
                // Each bolt of the channel is a heal of its own, so each can shield.
                rank.crit_absorb = aegis.map(|absorb| CritAbsorb {
                    ticks: true,
                    ..absorb
                });
                ranks.push(rank);
            }
        }
        if self.talents.bool("prayer_of_mending") {
            let timer = sim.new_timer(unit);
            for &id in PRAYER_OF_MENDING {
                let row = must_find(id);
                let shape = HealShape::party(PRAYER_OF_MENDING_CHARGES).smart(SmartHeal::Mending {
                    charges: PRAYER_OF_MENDING_CHARGES,
                    duration: mending_duration(row),
                });
                let mut rank = register_heal_from(
                    sim,
                    unit,
                    row,
                    row.effect_n(1),
                    masks::HEAL,
                    shape,
                    Some(timer),
                );
                rank.crit_absorb = aegis;
                ranks.push(rank);
            }
        }
        self.heals = ranks;
    }

    /// Divine Aegis at the rank taken, under Forever's rules: the share of a crit heal its
    /// absorb holds, with the absorb aura's spell registered for the shielding to count on.
    fn divine_aegis(&self, sim: &mut Sim, unit: UnitId) -> Option<CritAbsorb> {
        let rank = talent_rank(&self.talents, "divine_aegis");
        if rank <= 0 || sim.rules != Rules::Forever {
            return None;
        }
        let share = Ladder::talent(DIVINE_AEGIS, DIVINE_AEGIS_RANKS)
            .rank(rank)
            .effect_n(1)
            .base_points
            / 100.0;
        let absorb = must_find(DIVINE_AEGIS_ABSORB);
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: ActionId::spell(absorb.id),
                spell_school: absorb.spell_school(),
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::PROC | SpellFlag::HELPFUL,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        );
        Some(CritAbsorb {
            share,
            spell_id: absorb.id,
            ticks: false,
        })
    }

    /// The `priest_heal_talents` effect under Forever's rules: Renewed Hope and Litany of Light
    /// at the ranks taken, over the heals this priest registered. None when it took neither.
    pub(super) fn heal_talents(&self, sim: &Sim) -> Option<serde_json::Value> {
        if sim.rules != Rules::Forever {
            return None;
        }
        let ids = |keep: &dyn Fn(&crate::prepare::heals::HealRank) -> bool| -> Vec<i32> {
            self.heals
                .iter()
                .filter(|rank| keep(rank))
                .map(|rank| rank.row.id)
                .collect()
        };
        let hope_rank = talent_rank(&self.talents, "renewed_hope");
        let renewed_hope = (hope_rank > 0).then(|| {
            let row = Ladder::talent(RENEWED_HOPE, RENEWED_HOPE_RANKS).rank(hope_rank);
            RenewedHope {
                heal_spell_ids: ids(&|rank| RENEWED_HOPE_HEALS.contains(&rank.row.name.as_str())),
                shield_spell_ids: ids(&|rank| rank.absorb.is_some()),
                crit_percent: row.effect_n(1).base_points,
                cut_ns: -(row.effect_n(2).base_points as i64) * MILLISECOND,
                duration_ns: must_find(WEAKENED_SOUL).duration(),
                hit_rates: party_damage::HIT_RATES.to_vec(),
            }
        });
        let litany_rank = talent_rank(&self.talents, "litany_of_light");
        let litany_of_light = (litany_rank > 0).then(|| {
            let row = Ladder::talent(LITANY_OF_LIGHT, LITANY_OF_LIGHT_RANKS).rank(litany_rank);
            // A heal is a rank that heals, directly or over time; a shield is not one.
            let mut families: Vec<(&str, Vec<i32>)> = Vec::new();
            for rank in self.heals.iter().filter(|rank| rank.absorb.is_none()) {
                match families.iter_mut().find(|(name, _)| *name == rank.row.name) {
                    Some((_, ranks)) => ranks.push(rank.row.id),
                    None => families.push((rank.row.name.as_str(), vec![rank.row.id])),
                }
            }
            LitanyOfLight {
                share: row.effect_n(1).base_points / 100.0,
                families: families.into_iter().map(|(_, ranks)| ranks).collect(),
                metrics_action_id: ActionId::spell(LITANY_OF_LIGHT),
            }
        });
        if renewed_hope.is_none() && litany_of_light.is_none() {
            return None;
        }
        Some(serde_json::json!({
            "kind": "priest_heal_talents",
            "renewed_hope": renewed_hope,
            "litany_of_light": litany_of_light,
        }))
    }

    /// Applies the heal talents the healer took.
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
