//! Intercept (20617), from Go sim/warrior/intercept.go: a cast at range in Berserker Stance
//! whose dash aura, Charge's, lasts the spell's cooldown, triples the movement speed and ends
//! with the movement. The cast runs the warrior to 3.5 yards inside the minimum range, and the
//! aura's end lands the stun's damage on the cast's target with an outcome that always hits.
//! The spell has no damage multiplier, so the hit deals nothing.

use crate::core::fight::{movement::MovementKind, Agent, AuraRef, Fight, Outcome, Side, SpellId};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Intercept {
    pub(crate) aura: AuraRef,
    /// What the dash aura multiplies the movement speed by, a Go literal.
    pub(crate) speed_multiplier: f64,
    /// How far inside the minimum range the cast runs to, a Go literal.
    pub(crate) overshoot: f64,
    pub(crate) min_range: f64,
    /// The stun's damage effect.
    pub(crate) damage: f64,
    /// Go `interceptTarget`, with the spell that dealt it.
    pub(crate) cast: Option<(SpellId, Side)>,
}

/// The cast's `ApplyEffects`: the target, the aura for the spell's cooldown, then the run.
pub(crate) fn apply<A: Agent>(
    fight: &mut Fight<A>,
    spell: SpellId,
    target: Side,
    params: &mut Intercept,
) {
    params.cast = Some((spell, target));
    let duration = fight.spells[spell].cd.map_or(0, |(_, duration)| duration);
    fight.aura_mut(params.aura).duration = duration;
}

/// The run after the aura is up: Go `MoveTo(chargeMinRange-3.5)`.
pub(crate) fn run<A: Agent>(fight: &mut Fight<A>, params: Intercept) {
    fight.move_to(Side::Player, params.min_range - params.overshoot);
}

/// The dash aura's `OnGain`: Go `MultiplyMovementSpeed` by its multiplier.
pub(crate) fn on_gain<A: Agent>(fight: &mut Fight<A>, params: Intercept) {
    fight.multiply_movement_speed(params.speed_multiplier);
}

/// The dash aura's `OnExpire`: the multiplier undone, then the stun's damage.
pub(crate) fn on_expire<A: Agent>(fight: &mut Fight<A>, params: Intercept) {
    fight.multiply_movement_speed(1.0 / params.speed_multiplier);
    if let Some((spell, target)) = params.cast {
        let result = fight.calc_damage_with(spell, target, params.damage, Outcome::AlwaysHit);
        fight.deal_damage(spell, result, false);
    }
}

/// The movement callback the dash registers: the end of the movement ends the aura.
pub(crate) fn on_movement<A: Agent>(fight: &mut Fight<A>, params: Intercept, kind: MovementKind) {
    if kind == MovementKind::End && fight.aura(params.aura).active {
        fight.deactivate_aura(params.aura);
    }
}
