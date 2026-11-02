//! Warrior gear from Go sim/warrior/items.go with dynamic behavior.
//!
//! Battlegear of Might's 5 piece bonus: a landed hit taken that dealt damage rolls the proc
//! chance under the trigger's name, and a spell batch window later the warrior gains rage. Go
//! also listens to periodic damage taken, which nothing deals to the player in scope.

use crate::core::fight::{Agent, AuraRef, Fight, SpellId, SpellResult};

#[derive(Clone, Debug)]
pub(crate) struct MightRage {
    pub(crate) rng_label: String,
    pub(crate) proc_chance: f64,
    pub(crate) rage: f64,
    pub(crate) metrics: usize,
}

impl MightRage {
    /// The trigger's OnSpellHitTaken for a hit not flagged a proc.
    pub(crate) fn on_hit_taken<A: Agent>(
        &self,
        fight: &mut Fight<A>,
        aura: AuraRef,
        spell: SpellId,
        result: &SpellResult,
    ) {
        if !result.landed() || result.damage == 0.0 {
            return;
        }
        if self.proc_chance != 1.0 && !fight.proc(self.proc_chance, &self.rng_label) {
            return;
        }
        fight.schedule_delayed_proc(aura, spell, *result);
    }

    /// The delayed handler: `AddRage`.
    pub(crate) fn grant<A: Agent>(&self, fight: &mut Fight<A>) {
        fight.add_rage(self.rage, self.metrics);
    }
}
