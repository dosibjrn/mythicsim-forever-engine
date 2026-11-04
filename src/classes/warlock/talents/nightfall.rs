//! Nightfall (18094), from Go sim/warlock/talents_affliction.go `applyNightfall`: periodic
//! damage of Corruption and the drains may grant Shadow Trance (17941), whose spell modifier
//! makes Shadow Bolt instant; an instant Shadow Bolt's completed cast ends it.

use crate::core::fight::{Agent, AuraRef, Fight, ModId, ModKind, Side, SpellId, SpellResult};

#[derive(Clone, Debug)]
pub(crate) struct Nightfall {
    pub(crate) trance: AuraRef,
    proc_chance: f64,
    rng_label: String,
    trigger_spells: Vec<SpellId>,
    consume_spells: Vec<SpellId>,
    cast_time: ModId,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn bind<A: Agent>(
    fight: &mut Fight<A>,
    aura: &str,
    proc_chance: f64,
    rng_label: &str,
    trigger_spells: &[usize],
    consume_spells: &[usize],
    modded_spells: &[usize],
    cast_time_percent: f64,
) -> Result<Nightfall, String> {
    let trance = fight.player_aura(aura)?;
    let cast_time = fight.register_mod(
        ModKind::CastTimePercent,
        cast_time_percent,
        0,
        modded_spells.to_vec(),
    );
    Ok(Nightfall {
        trance,
        proc_chance,
        rng_label: rng_label.to_string(),
        trigger_spells: trigger_spells.to_vec(),
        consume_spells: consume_spells.to_vec(),
        cast_time,
    })
}

impl Nightfall {
    /// The Nightfall trigger's OnPeriodicDamageDealt: Go `AttachProcTriggerCallback` with the
    /// talent's chance and no outcome filter, whose handler waits a spell batch window.
    pub(crate) fn on_periodic_damage_dealt<A: Agent>(
        &self,
        fight: &mut Fight<A>,
        trigger: AuraRef,
        spell: SpellId,
        result: &SpellResult,
    ) {
        if !self.trigger_spells.contains(&spell) {
            return;
        }
        if self.proc_chance != 1.0 && !fight.proc(self.proc_chance, &self.rng_label) {
            return;
        }
        fight.schedule_delayed_proc(trigger, spell, *result);
    }

    /// The trigger's delayed handler: grant Shadow Trance.
    pub(crate) fn on_trigger<A: Agent>(&self, fight: &mut Fight<A>) {
        fight.activate_aura(self.trance);
    }

    /// Shadow Trance's OnCastComplete, a proc trigger too, so its handler also waits a spell
    /// batch window.
    pub(crate) fn on_cast_complete<A: Agent>(&self, fight: &mut Fight<A>, spell: SpellId) {
        if !self.consume_spells.contains(&spell) {
            return;
        }
        let result = SpellResult {
            armor_multiplier: 0.0,
            target: Side::Target,
            attacker: fight.spells[spell].caster,
            outcome: 0,
            damage: 0.0,
            threat: 0.0,
        };
        fight.schedule_delayed_proc(self.trance, spell, result);
    }

    /// Shadow Trance's delayed handler: the cast that completed, if instant, ends it.
    pub(crate) fn on_consume<A: Agent>(&self, fight: &mut Fight<A>, spell: SpellId) {
        if fight.spells[spell].cur_cast.cast_time != 0 {
            return;
        }
        fight.deactivate_aura(self.trance);
    }

    pub(crate) fn on_gain<A: Agent>(&self, fight: &mut Fight<A>) {
        fight.activate_mod(self.cast_time);
    }

    pub(crate) fn on_expire<A: Agent>(&self, fight: &mut Fight<A>) {
        fight.deactivate_mod(self.cast_time);
    }
}
