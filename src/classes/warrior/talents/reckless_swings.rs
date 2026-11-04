//! Reckless Swings (1329555, proc 1329554), from the development notes of 2 November 2026 and
//! wowsims/forever #841's `registerRecklessSwings`: a cast for a share of base health gives an
//! aura of four charges for 8 sec, and each landed white swing that dealt damage gives 5 Rage and
//! spends a charge a spell batch window later, as a proc handler's effects do. Forever's rules
//! only. The row's proc is each melee auto attack, so Heroic Strike and Cleave, which take a
//! swing's place but are melee abilities, do not count, as for Unbridled Wrath (community #692);
//! upstream says no one has checked in game which swings spend the charges.

use crate::core::fight::{Agent, AuraRef, Fight, SpellId, SpellResult};

#[derive(Clone, Copy, Debug)]
pub(crate) struct RecklessSwings {
    pub(crate) aura: AuraRef,
    pub(crate) trigger: AuraRef,
    pub(crate) rage: f64,
    pub(crate) charges: i32,
    pub(crate) health_cost: f64,
    /// Go `NewRageMetrics(actionID)`, the proc's.
    pub(crate) metrics: usize,
}

/// The cast: the health goes, the aura comes up at full charges.
pub(crate) fn apply<A: Agent>(fight: &mut Fight<A>, params: RecklessSwings) {
    fight.remove_health(params.health_cost);
    fight.activate_aura(params.aura);
    fight.set_stacks(params.aura, params.charges);
}

/// The trigger's OnSpellHitDealt: a white swing that landed and dealt damage while the aura is up.
pub(crate) fn on_hit<A: Agent>(
    fight: &mut Fight<A>,
    params: RecklessSwings,
    spell: SpellId,
    result: &SpellResult,
) {
    let state = &fight.spells[spell];
    if state.flags.proc
        || !state.white_hit
        || state.melee_special
        || !result.landed()
        || result.damage == 0.0
        || !fight.aura(params.aura).active
    {
        return;
    }
    fight.schedule_delayed_proc(params.trigger, spell, *result);
}

/// The delayed handler: the Rage, and one charge.
pub(crate) fn grant<A: Agent>(fight: &mut Fight<A>, params: RecklessSwings) {
    fight.add_rage(params.rage, params.metrics);
    fight.remove_stack(params.aura);
}
