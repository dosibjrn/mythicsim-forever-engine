//! Disarm (676), from Go sim/warrior/disarm.go: a special hit roll without damage in Defensive
//! Stance. A landed hit puts up the target's aura, which only tracks its uptime; a miss refunds.

use crate::core::fight::{melee::PhysicalOutcome, Agent, AuraRef, Fight, Side, SpellId};

pub(crate) fn apply<A: Agent>(fight: &mut Fight<A>, spell: SpellId, target: Side, aura: AuraRef) {
    let outcome = PhysicalOutcome::MeleeSpecialHit { count: true };
    let result = fight.calc_physical_outcome(spell, target, outcome);
    fight.deal_damage(spell, result, false);
    if result.landed() {
        let aura = fight.aura_on(aura, target);
        fight.activate_aura(aura);
    } else {
        fight.issue_refund(spell);
    }
}
