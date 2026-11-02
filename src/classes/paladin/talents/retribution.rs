//! Retribution talents with listeners, from Go sim/paladin/talents_retribution.go:
//! Vengeance, Vindication, Sanctified Judgement, Sacred Arbiter, Eye for an Eye and Pursuit
//! of Justice.

use crate::core::fight::{
    AuraRef, Fight, ModId, ModKind, Outcome, Side, SpellId, SpellResult, OUTCOME_CRIT,
    OUTCOME_LANDED,
};

use super::super::{agent::PaladinAgent, spells::seals};

/// Vengeance: non-periodic crits stack a damage done mod on Holy and Physical spells.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Vengeance {
    pub(crate) aura: AuraRef,
    pub(crate) damage_mod: ModId,
    pub(crate) per_stack: f64,
}

impl Vengeance {
    pub(crate) fn bind(
        fight: &mut Fight<PaladinAgent>,
        aura: &str,
        per_stack: f64,
        spells: &[usize],
    ) -> Result<Self, String> {
        let aura = fight.player_aura(aura)?;
        let damage_mod =
            fight.register_mod(ModKind::DamageDonePercent, per_stack, 0, spells.to_vec());
        Ok(Vengeance {
            aura,
            damage_mod,
            per_stack,
        })
    }

    /// The trigger: any crit, procs included, handled a batch window later.
    pub(crate) fn on_spell_hit_dealt(
        fight: &mut Fight<PaladinAgent>,
        trigger: AuraRef,
        spell: SpellId,
        result: &SpellResult,
    ) {
        if result.outcome & OUTCOME_CRIT == 0 {
            return;
        }
        fight.schedule_delayed_proc(trigger, spell, *result);
    }

    pub(crate) fn on_delayed_proc(self, fight: &mut Fight<PaladinAgent>) {
        fight.activate_aura(self.aura);
        fight.add_stack(self.aura);
    }

    pub(crate) fn on_gain(self, fight: &mut Fight<PaladinAgent>) {
        fight.activate_mod(self.damage_mod);
    }

    pub(crate) fn on_expire(self, fight: &mut Fight<PaladinAgent>) {
        fight.deactivate_mod(self.damage_mod);
    }

    pub(crate) fn on_stacks_change(self, fight: &mut Fight<PaladinAgent>, new: i32) {
        fight.update_mod_value(self.damage_mod, self.per_stack * f64::from(new));
    }
}

/// Vindication: landed melee hits weaken the target and raise the paladin's attack power.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Vindication {
    pub(crate) chance: f64,
    /// The paladin's attack power aura, which push 113554 removes under Forever's rules.
    pub(crate) aura: Option<AuraRef>,
    pub(crate) target_aura: AuraRef,
}

impl Vindication {
    /// The trigger: landed melee hits that are not procs, after the chance, handled a batch
    /// window later.
    pub(crate) fn on_spell_hit_dealt(
        self,
        fight: &mut Fight<PaladinAgent>,
        trigger: AuraRef,
        spell: SpellId,
        result: &SpellResult,
    ) {
        let state = &fight.spells[spell];
        if state.flags.proc || !state.melee_proc || result.outcome & OUTCOME_LANDED == 0 {
            return;
        }
        if self.chance != 1.0 && fight.random_for_aura(trigger) > self.chance {
            return;
        }
        fight.schedule_delayed_proc(trigger, spell, *result);
    }

    pub(crate) fn on_delayed_proc(self, fight: &mut Fight<PaladinAgent>) {
        fight.activate_aura(self.target_aura);
        if let Some(aura) = self.aura {
            fight.activate_aura(aura);
        }
    }
}

/// Sanctified Judgement: Judgement can return part of the active seal's last cost.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SanctifiedJudgement {
    pub(crate) chance: f64,
    pub(crate) refund: f64,
    pub(crate) metrics: usize,
}

impl SanctifiedJudgement {
    pub(crate) fn on_cast_complete(
        self,
        fight: &mut Fight<PaladinAgent>,
        trigger: AuraRef,
        spell: SpellId,
    ) {
        let state = &fight.spells[spell];
        if state.flags.proc || state.class_spell.as_deref() != Some("judgement") {
            return;
        }
        if self.chance != 1.0 && fight.random_for_aura(trigger) > self.chance {
            return;
        }
        if let Some(seal) = seals::active_seal(fight) {
            let seal_spell = fight.agent.seals.seals[seal].spell;
            let amount = fight.spells[seal_spell].cur_cast.cost * self.refund;
            fight.add_mana(amount, self.metrics);
        }
    }
}

/// Sacred Arbiter: a landed Holy Strike refreshes every active judgement aura on the target.
pub(crate) fn sacred_arbiter_hit(
    fight: &mut Fight<PaladinAgent>,
    judgements: &[AuraRef],
    spell: SpellId,
    result: &SpellResult,
) {
    let state = &fight.spells[spell];
    if state.flags.proc
        || state.class_spell.as_deref() != Some("holy_strike")
        || result.outcome & OUTCOME_LANDED == 0
    {
        return;
    }
    for &aura in judgements {
        if fight.aura(aura).active {
            fight.refresh_aura(aura);
        }
    }
}

/// Eye for an Eye: a crit taken that dealt damage reflects a share of it, a batch window
/// later, as Holy damage that ignores modifiers and always hits.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EyeForAnEye {
    pub(crate) spell: SpellId,
    pub(crate) share: f64,
    /// The damage the next reflection deals, which Go keeps in the closure.
    pub(crate) reflected: f64,
}

impl EyeForAnEye {
    /// The trigger on the target's swing: a crit, with `RequireDamageDealt`.
    pub(crate) fn trigger(fight: &mut Fight<PaladinAgent>, trigger: AuraRef, result: &SpellResult) {
        if result.outcome & OUTCOME_CRIT == 0 || result.damage <= 0.0 {
            return;
        }
        // The swing that was taken is not a player spell; the handler reads none.
        fight.schedule_delayed_proc(trigger, 0, *result);
    }

    /// The reflection's `ApplyEffects`.
    pub(crate) fn reflect(self, fight: &mut Fight<PaladinAgent>, spell: SpellId, target: Side) {
        let result = fight.calc_damage_with(spell, target, self.reflected, Outcome::AlwaysHit);
        fight.deal_damage(spell, result, false);
    }
}
