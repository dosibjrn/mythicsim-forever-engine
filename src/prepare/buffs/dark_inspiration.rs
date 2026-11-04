//! A Shadow Priest's Dark Inspiration in the player's party, a Rust-only party buff
//! (`PartyBuffs.dark_inspiration`, README, Rust-only mechanics) translated from wowsims/forever
//! at ce4dd1a69: `darkInspirationMeta` in sim/core/buffs/buffs_auto_gen.go and
//! `NewSpellDataDarkInspirationAuraFor` in sim/core/buffs/buffs.go.
//!
//! The talent (1326159, 5 ranks, client 70425) has Mind Blast and a full Mind Flay channel raise
//! the Spirit of the priest's party for 10 sec. The buff row (1326168) states the whole 15% and
//! the talent's tooltip prices it by rank, so the party is worth round(15 * rank / 5)% Spirit:
//! 3, 6, 9, 12 and 15. The pinned reference has no such buff, so the party's copy is permanent,
//! as upstream's is: a raid keeps it up on a priest's casters. It reaches the party's players,
//! never their pets.

use std::rc::Rc;

use super::super::env::Environment;
use super::super::parse_effects::exclusive_stat_category;
use super::super::sim::{AuraId, Rules, Sim, UnitId};
use super::super::stats::Stat;
use super::super::Refusal;
use super::{permanent, Meta, MetaKind};
use crate::contracts::request::Message;

/// The talent's ranks, which the request's rank is held to.
pub(crate) const DARK_INSPIRATION_RANKS: i32 = 5;

pub(crate) static DARK_INSPIRATION: Meta = Meta {
    kind: MetaKind::Buff,
    label: "Dark Inspiration",
    spell: 1326168,
    category: "DarkInspiration",
    talent: Some((1326159, DARK_INSPIRATION_RANKS)),
    talent_effect: 1,
    talent_divisor: DARK_INSPIRATION_RANKS,
    ..Meta::DEFAULT
};

/// Go `applyGeneratedBuffs`' `party.DarkInspiration` case: the party's copy at the rank asked
/// for, held to the talent's five.
pub(crate) fn apply_party_dark_inspiration(
    env: &mut Environment,
    unit: UnitId,
    party: &Message,
) -> Result<(), Refusal> {
    let rank = party.i32("dark_inspiration");
    if rank > 0 {
        permanent(
            env,
            unit,
            &DARK_INSPIRATION,
            false,
            rank.min(DARK_INSPIRATION_RANKS),
        )?;
    }
    Ok(())
}

/// Go `NewSpellDataDarkInspirationAuraFor`: a priest's own copy, `aura`, multiplies the
/// priest's Spirit by `multiplier` while it holds the party copy's Spirit category. It yields
/// ties, so a copy no stronger than the party's leaves the party's holding the Spirit while it is
/// up, and the two never stack. The pinned reference has no party copy and attaches the
/// multiplier to the aura alone, as `Rules::GoReference` does.
pub(crate) fn bid_own_dark_inspiration(sim: &mut Sim, unit: UnitId, aura: AuraId, multiplier: f64) {
    let spirit = sim.new_dynamic_multiply_stat(unit, Stat::Spirit, multiplier);
    if sim.rules == Rules::GoReference {
        sim.attach_stat_dependency(aura, spirit);
        return;
    }
    let category = exclusive_stat_category(DARK_INSPIRATION.category, Stat::Spirit.name(), true);
    let effect = sim.new_exclusive_effect(
        aura,
        &category,
        false,
        multiplier,
        Some(Rc::new(move |sim: &mut Sim, _| {
            sim.enable_build_phase_stat_dep(unit, spirit)
        })),
        Some(Rc::new(move |sim: &mut Sim, _| {
            sim.disable_build_phase_stat_dep(unit, spirit)
        })),
    );
    sim.effects[effect.0].yields_ties = true;
}
