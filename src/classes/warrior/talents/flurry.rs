//! Flurry (12319, buff 12966): a landed melee crit, other than Whirlwind's off hand strike,
//! grants melee speed for a number of charges. Under Forever's rules every white swing spends
//! one whatever its outcome (hit, glance, miss, dodge or parry), and a Heroic Strike or Cleave
//! spends one only when it misses. Go's `registerFlurry`, which `--go-rules` keeps, spends one
//! on each landed white hit, Heroic Strike and Cleave included.
//!
//! Evidence: three level 30 Forever logs on build 1.60.1.70450 with Flurry 1/5
//! (magey/forever-warrior discussion 30, counted in wowsims/forever PR 745). With Flurry up,
//! white misses spent a charge 21/21, 83/83 and 76/76, dodges 27/27, 28/28 and 20/20, parries
//! 25/25, 24/24 and 23/23; missed Heroic Strikes 9/9, 4/4 and 10/10; landed Heroic Strikes
//! 0/109 and 0/74 (and 3/82, each sharing its moment with a white swing), dodged or parried
//! ones 0/38. The logs hold one Cleave, so Cleave following Heroic Strike is an assumption.

use crate::core::fight::{Agent, AuraRef, Fight, SpellId, SpellResult, OUTCOME_MISS};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Flurry {
    pub(crate) aura: AuraRef,
    pub(crate) melee_speed_multiplier: f64,
    pub(crate) charges: i32,
    /// Forever's rules (below); without them, as in Go, a landed white hit spends a charge.
    pub(crate) every_swing_spends: bool,
}

/// The trigger's OnSpellHitDealt, which acts at once.
pub(crate) fn on_hit<A: Agent>(
    fight: &mut Fight<A>,
    params: Flurry,
    spell: SpellId,
    result: &SpellResult,
) {
    let state = &fight.spells[spell];
    if state.flags.proc || !state.melee_proc || !(params.every_swing_spends || result.landed()) {
        return;
    }
    let class_spell = state.class_spell.as_deref();
    if class_spell == Some("whirlwind_off_hand") {
        return;
    }
    if result.crit() {
        fight.activate_aura(params.aura);
        fight.set_stacks(params.aura, params.charges);
        return;
    }
    if !fight.aura(params.aura).active {
        return;
    }
    // Heroic Strike and Cleave carry the main hand's auto mask too (ProcMask::MELEE_MH), so
    // they are told apart from the white swing by name.
    let spends = if !params.every_swing_spends {
        state.white_hit
    } else if matches!(class_spell, Some("heroic_strike" | "cleave")) {
        result.outcome & OUTCOME_MISS != 0
    } else {
        state.white_hit
    };
    if spends {
        fight.remove_stack(params.aura);
    }
}

/// Go `AttachMultiplyMeleeSpeed`: the gain multiplies, the expiry multiplies by the
/// reciprocal.
pub(crate) fn on_gain<A: Agent>(fight: &mut Fight<A>, params: Flurry) {
    fight.multiply_melee_speed(params.melee_speed_multiplier);
}

pub(crate) fn on_expire<A: Agent>(fight: &mut Fight<A>, params: Flurry) {
    fight.multiply_melee_speed(1.0 / params.melee_speed_multiplier);
}
