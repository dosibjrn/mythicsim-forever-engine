//! Whirlwind (1680), from Go sim/warrior/whirlwind.go: a normalized main hand strike on the
//! weapon special table against each target up to the row's cap, from the cast target on. One
//! weapon roll serves every target, and the results are dealt together once calculated; with
//! an off hand weapon the off hand's strike follows as its own cast (hotfix 112347).

use super::sweeping_strikes::{self, SweepingStrikes};
use crate::core::fight::{melee::PhysicalOutcome, Agent, Fight, Side, SpellId};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Whirlwind {
    /// The off hand's spell when the warrior holds an off hand weapon.
    pub(crate) off_hand: Option<SpellId>,
    /// The row's target cap.
    pub(crate) max_targets: usize,
    /// Improved Whirlwind's share more damage the first target of each strike takes, which is
    /// the closest enemy; 0 without the talent (Forever's rules).
    pub(crate) closest_bonus: f64,
}

/// Improved Whirlwind's dummy, applied as wowsims/forever #841's `scaleClosest` does: the first
/// result of a strike (the cast target's) takes the share and its threat follows the damage.
fn scale_closest<A: Agent>(
    fight: &Fight<A>,
    spell: SpellId,
    params: Whirlwind,
    results: &mut [crate::core::fight::SpellResult],
) {
    if params.closest_bonus == 0.0 {
        return;
    }
    if let Some(first) = results.first_mut() {
        first.damage *= 1.0 + params.closest_bonus;
        first.threat = fight.threat_of(spell, first);
    }
}

/// The main hand strike.
pub(crate) fn apply<A: Agent>(
    fight: &mut Fight<A>,
    spell: SpellId,
    target: Side,
    params: Whirlwind,
    sweeping: Option<SweepingStrikes>,
) {
    let attack_power = fight.melee_attack_power();
    let base = fight.mh_normalized_weapon_damage(attack_power);
    let outcome = PhysicalOutcome::MeleeWeaponSpecialHitAndCrit { count: true };
    let mut results: Vec<_> = fight
        .cleave_targets(target, params.max_targets)
        .into_iter()
        .map(|hit| fight.calc_physical_damage(spell, hit, base, outcome))
        .collect();
    scale_closest(fight, spell, params, &mut results);
    sweeping_strikes::cast_normalized(fight, sweeping, &results);
    fight.deal_batched_aoe_damage(spell, &results, false);
    if let Some(off_hand) = params.off_hand {
        fight.cast(off_hand, target);
    }
}

/// The off hand strike, Whirlwind's tagged spell.
pub(crate) fn apply_off_hand<A: Agent>(
    fight: &mut Fight<A>,
    spell: SpellId,
    target: Side,
    params: Whirlwind,
) {
    let attack_power = fight.melee_attack_power();
    let base = fight.oh_normalized_weapon_damage(attack_power);
    let outcome = PhysicalOutcome::MeleeWeaponSpecialHitAndCrit { count: true };
    let mut results: Vec<_> = fight
        .cleave_targets(target, params.max_targets)
        .into_iter()
        .map(|hit| fight.calc_physical_damage(spell, hit, base, outcome))
        .collect();
    scale_closest(fight, spell, params, &mut results);
    fight.deal_batched_aoe_damage(spell, &results, false);
}
