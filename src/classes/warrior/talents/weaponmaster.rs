//! Weaponmaster with a sword (1290261), from Go sim/warrior/talents_arms.go
//! `registerWeaponmaster`: a landed melee hit of a sword hand, other than a proc's or the
//! extra attack's own, may cast an extra main hand attack at once, which a queued Heroic
//! Strike replaces.

use crate::core::fight::{Agent, AuraRef, Fight, SpellId, SpellResult};

#[derive(Clone, Copy, Debug)]
pub(crate) struct WeaponmasterSword {
    pub(crate) trigger: AuraRef,
    pub(crate) proc_chance: f64,
    pub(crate) extra_attack: SpellId,
}

/// The trigger's OnSpellHitDealt. `sword` is whether the spell's proc mask holds a bit of a
/// hand that wields a sword.
pub(crate) fn on_hit<A: Agent>(
    fight: &mut Fight<A>,
    params: WeaponmasterSword,
    spell: SpellId,
    sword: bool,
    result: &SpellResult,
) {
    let state = &fight.spells[spell];
    if state.flags.proc || !state.melee_proc || !result.landed() {
        return;
    }
    if !sword || spell == params.extra_attack {
        return;
    }
    if params.proc_chance != 1.0 && fight.random_for_aura(params.trigger) > params.proc_chance {
        return;
    }
    let mut attack = params.extra_attack;
    if fight.config.melee.replace_main_hand_swing {
        attack = A::replace_mh_swing(fight, attack);
        fight.note_extra_attack_strike(params.extra_attack, attack);
    }
    fight.cast(attack, result.target);
}
