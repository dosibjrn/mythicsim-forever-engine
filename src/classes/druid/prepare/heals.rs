//! The Restoration Druid's heals and heal talents, which Rust alone sims (see `prepare::heals`):
//! every rank of Healing Touch, Rejuvenation and Regrowth, and the talents' Wild Growth and
//! Swiftmend, from the client rows of build 1.60.1.70425.
//!
//! Wild Growth's ticks land on the party of five. Swiftmend consumes a Rejuvenation or Regrowth
//! up on the player and heals what all of that heal over time's ticks would, as Forever's
//! Swiftmend does (`classes::druid::spells::swiftmend`). Tranquility heals the party over its
//! channel, and Nature's Swiftness makes the next Healing Touch or Regrowth instant.

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::heals::{
    apply_heal_talent, heal_talent_mod, register_channel_heal, register_heal, ChannelHeal,
    HealShape,
};
use crate::prepare::hotfix_113554;
use crate::prepare::sim::{
    AuraConfig, Cooldown, EventCallbacks, Rules, Sim, NEVER_EXPIRES, SECOND,
};
use crate::prepare::spell::{school, CastConfig, DefenseType, ProcMask, SpellConfig, SpellFlag};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::spelldata::store::must_find;
use crate::prepare::spelldata::Ladder;

use super::masks;
use super::Druid;

pub(crate) const HEALING_TOUCH: &[i32] = &[
    5185, 5186, 5187, 5188, 5189, 6778, 8903, 9758, 9888, 9889, 25297,
];
pub(crate) const REJUVENATION: &[i32] = &[
    774, 1058, 1430, 2090, 2091, 3627, 8910, 9839, 9840, 9841, 25299,
];
pub(crate) const REGROWTH: &[i32] = &[8936, 8938, 8939, 8940, 8941, 9750, 9856, 9857, 9858];
/// Wild Growth, a Forever talent: a heal over time on the target's party.
const WILD_GROWTH: &[i32] = &[408120, 1238214, 1238215];
const SWIFTMEND: i32 = 18562;
/// Bloom, the Restoration talent of hotfix push 113531 (Forever's rules): the client has no
/// separate heal spell for it, so its heals count under the talent's own id.
pub(crate) const BLOOM: i32 = 1_328_621;
/// Tranquility, by rank.
const TRANQUILITY: &[i32] = &[740, 8918, 9862, 9863];
/// Nature's Swiftness, a Restoration talent.
const NATURES_SWIFTNESS: i32 = 17116;
const PARTY: u32 = 5;

/// The heal talents, as (talent, client row, ranks): Gift of Nature's direct and periodic
/// healing, Tranquil Spirit's cost, Improved Rejuvenation, Improved Regrowth's crit, Naturalist's
/// Healing Touch cast time and Gift of the Earthmother's global cooldown. Gift of the Earthmother
/// is gone from Forever's tree since hotfix push 113531, so only the reference sets it.
const HEAL_TALENTS: &[(&str, i32, i32)] = &[
    ("gift_of_nature", 17104, 5),
    ("tranquil_spirit", 24968, 5),
    ("improved_rejuvenation", 17111, 3),
    ("improved_regrowth", 17074, 5),
    ("naturalist", 17069, 5),
    ("gift_of_the_earthmother", 414673, 1),
];

/// Nature's Splendor (1223083) lengthens Rejuvenation by 3 seconds and Regrowth by 6, one and
/// two of their 3 second ticks.
const NATURES_SPLENDOR_TICKS: &[(i64, i32)] = &[(masks::REJUVENATION, 1), (masks::REGROWTH, 2)];

impl Druid {
    /// Registers every heal rank the druid knows.
    pub(super) fn register_heals(&mut self, sim: &mut Sim) {
        let unit = self.unit;
        let mut ranks = Vec::new();
        let mut each =
            |sim: &mut Sim, ids: &[i32], mask: i64, shape: HealShape, shared_cd: bool| {
                let timer = shared_cd.then(|| sim.new_timer(unit));
                for &id in ids {
                    // Hotfix push 113531 gives Wild Growth a 3 sec cast under Forever's rules.
                    let row = hotfix_113554::row(sim.rules, id);
                    ranks.push(register_heal(sim, unit, row, mask, shape, timer));
                }
            };
        each(
            sim,
            HEALING_TOUCH,
            masks::HEALING_TOUCH,
            HealShape::SINGLE,
            false,
        );
        each(
            sim,
            REJUVENATION,
            masks::REJUVENATION,
            HealShape::SINGLE,
            false,
        );
        each(sim, REGROWTH, masks::REGROWTH, HealShape::SINGLE, false);
        if self.talents.bool("wild_growth") {
            each(
                sim,
                WILD_GROWTH,
                masks::WILD_GROWTH,
                HealShape::party(PARTY),
                true,
            );
        }
        if self.talents.bool("swiftmend") {
            each(
                sim,
                &[SWIFTMEND],
                masks::SWIFTMEND,
                HealShape::SINGLE,
                false,
            );
        }
        // Tranquility: a channel healing the party every 2 sec for 10 sec, its ranks sharing a
        // 5 minute cooldown. The row puts the tick on a party aura, which the heal reads.
        {
            let timer = sim.new_timer(unit);
            for &id in TRANQUILITY {
                let row = must_find(id);
                let Some(tick) = row
                    .effects
                    .iter()
                    .find(|e| e.aura == crate::prepare::dbcenums::A_PERIODIC_HEAL)
                else {
                    continue;
                };
                let period = tick.period();
                ranks.push(register_channel_heal(
                    sim,
                    unit,
                    row,
                    ChannelHeal {
                        direct: None,
                        tick: tick.average(CHARACTER_LEVEL),
                        tick_coefficient: tick.coeff(),
                        ticks: (row.duration() / period.max(1)) as i32,
                        period,
                        tick_can_crit: row.periodic_can_crit(),
                    },
                    masks::TRANQUILITY,
                    HealShape::party(PARTY),
                    Some(timer),
                ));
            }
        }
        self.heals = ranks;
    }

    /// Bloom (1328621, hotfix push 113531): the spell its heal counts as. Registered as a
    /// passive heal on the healing proc mask, so the result has a row of its own and the
    /// overhealing share applies; under the reference's rules the talent is not read.
    pub(super) fn register_bloom(&self, sim: &mut Sim) {
        if !self.tal.bloom {
            return;
        }
        sim.register_spell(
            self.unit,
            SpellConfig {
                action_id: ActionId::spell(BLOOM),
                spell_school: school::NATURE,
                defense_type: DefenseType::Magic,
                proc_mask: ProcMask::SPELL_HEALING,
                flags: SpellFlag::HELPFUL
                    | SpellFlag::NO_ON_CAST_COMPLETE
                    | SpellFlag::PASSIVE_SPELL
                    | SpellFlag::PROC,
                class_spell_mask: masks::BLOOM,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        );
    }

    /// The `bloom` effect: the talent's heal and the heals over time it follows.
    pub(super) fn bloom_effect(&self) -> Option<serde_json::Value> {
        self.tal.bloom.then(|| {
            let hots: Vec<i32> = REJUVENATION.iter().chain(REGROWTH).copied().collect();
            serde_json::json!({"kind": "bloom", "spell_id": BLOOM, "hots": hots})
        })
    }

    /// Nature's Swiftness (17116): on use, the next Healing Touch or Regrowth casts at once; a
    /// 3 minute cooldown. Its aura's mods are the row's, on the druid's heals with a cast time.
    pub(super) fn register_natures_swiftness(&self, sim: &mut Sim) {
        if !self.talents.bool("natures_swiftness") {
            return;
        }
        let unit = self.unit;
        let row = must_find(NATURES_SWIFTNESS);
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Nature's Swiftness".to_string(),
                action_id: Some(crate::contracts::prepared_v2::ActionId::spell(row.id)),
                duration: NEVER_EXPIRES,
                // The cast it makes instant spends it.
                events: EventCallbacks {
                    on_cast_complete: true,
                    ..Default::default()
                },
                ..AuraConfig::default()
            },
        );
        sim.attach_spell_mod(
            aura,
            SpellModConfig {
                kind: SpellModType::CastTimePct,
                float_value: row.effect_n(1).percent(),
                class_mask: masks::HEALING_TOUCH | masks::REGROWTH,
                ..SpellModConfig::default()
            },
        );
        let timer = sim.new_timer(unit);
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: crate::contracts::prepared_v2::ActionId::spell(row.id),
                spell_school: row.spell_school(),
                flags: SpellFlag::APL | SpellFlag::NO_ON_CAST_COMPLETE,
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: row.cooldown().max(180 * SECOND),
                    },
                    ..CastConfig::default()
                },
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
    }

    /// Applies the heal talents the druid took.
    pub(super) fn apply_heal_talents(&self, sim: &mut Sim) {
        for &(name, spell_id, max_ranks) in HEAL_TALENTS {
            let rank = crate::prepare::heals::talent_rank(&self.talents, name);
            match hotfix_113554::hotfixed(spell_id) {
                // Hotfix push 113531: Naturalist's cast time effect covers Wild Growth instead of
                // its old second mask bit, under Forever's rules. The ranks keep their curve.
                Some(hotfixed) if sim.rules == Rules::Forever && rank > 0 => {
                    let row = Ladder::talent(spell_id, max_ranks).rank(rank);
                    for effect in &row.effects {
                        let mut effect = effect.clone();
                        if let Some(fixed) = hotfixed.effects.iter().find(|e| e.id == effect.id) {
                            effect.class_flags = fixed.class_flags;
                        }
                        heal_talent_mod(sim, self.unit, &effect);
                    }
                }
                _ => apply_heal_talent(sim, self.unit, spell_id, max_ranks, rank),
            }
        }
        if self.tal.natures_splendor {
            for &(mask, ticks) in NATURES_SPLENDOR_TICKS {
                sim.add_static_mod(
                    self.unit,
                    SpellModConfig {
                        class_mask: mask,
                        spell_flag: SpellFlag::HELPFUL,
                        kind: SpellModType::DotNumberOfTicksFlat,
                        int_value: ticks,
                        ..SpellModConfig::default()
                    },
                );
            }
        }
    }
}
