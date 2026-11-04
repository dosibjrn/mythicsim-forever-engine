//! Druid item set bonuses with dynamic behavior, from Go sim/druid/item_sets.go: Feralheart
//! Raiment's four piece Nature's Bounty and Symbols of Unending Life's three piece finisher
//! refund. Each proc's handler runs one spell batch window later.

use crate::{
    contracts::prepared_v2::ActionId,
    core::fight::{AuraRef, Fight, ResourceKind, SpellId, SpellResult},
};

use super::agent::DruidAgent;

/// Go `OutcomeMiss | OutcomeDodge | OutcomeBlock | OutcomeParry`.
const OUTCOME_AVOIDED: u16 = 1 | (1 << 6) | (1 << 9) | (1 << 8);
/// Go `OutcomeLanded`.
use crate::core::fight::OUTCOME_LANDED;

/// The delayed Rage proc's spell: the target's swing, which has no spell of the player's.
const TARGET_SWING: SpellId = usize::MAX;

fn mask(len: usize, spells: &[usize]) -> Vec<bool> {
    let mut mask = vec![false; len];
    for &spell in spells {
        if let Some(slot) = mask.get_mut(spell) {
            *slot = true;
        }
    }
    mask
}

/// Nature's Bounty: 2% to restore mana on a spell's cast, energy on a landed white hit, or Rage
/// when a melee attack strikes the druid.
#[derive(Clone, Debug)]
pub(crate) struct NaturesBounty {
    aura: AuraRef,
    proc_chance: f64,
    labels: [String; 3],
    amounts: [f64; 3],
    mana_spells: Vec<bool>,
    energy_spells: Vec<bool>,
    metrics: [usize; 3],
}

/// The exported parameters.
pub(crate) struct BountyParams<'a> {
    pub(crate) aura: &'a str,
    pub(crate) proc_chance: f64,
    pub(crate) labels: [&'a str; 3],
    pub(crate) amounts: [f64; 3],
    pub(crate) mana_spells: &'a [usize],
    pub(crate) energy_spells: &'a [usize],
    pub(crate) metrics_action_id: &'a ActionId,
}

/// Go registers the mana, energy and Rage metrics in that order.
pub(crate) fn bind_bounty(
    fight: &mut Fight<DruidAgent>,
    params: BountyParams,
) -> Result<NaturesBounty, String> {
    let aura = fight.player_aura(params.aura)?;
    let id = params.metrics_action_id;
    let mana = fight.new_resource_metrics(id.clone(), ResourceKind::Mana);
    let energy = fight.new_resource_metrics(id.clone(), ResourceKind::Energy);
    let rage = fight.new_resource_metrics(id.clone(), ResourceKind::Rage);
    let len = fight.spells.len();
    Ok(NaturesBounty {
        aura,
        proc_chance: params.proc_chance,
        labels: params.labels.map(str::to_string),
        amounts: params.amounts,
        mana_spells: mask(len, params.mana_spells),
        energy_spells: mask(len, params.energy_spells),
        metrics: [mana, energy, rage],
    })
}

impl NaturesBounty {
    fn roll(&self, fight: &mut Fight<DruidAgent>, which: usize) -> bool {
        let label = self.labels[which].clone();
        fight.proc(self.proc_chance, &label)
    }

    /// The mana trigger's `OnCastComplete`.
    pub(crate) fn on_cast_complete(&self, fight: &mut Fight<DruidAgent>, spell: SpellId) {
        if !self.mana_spells[spell] || !self.roll(fight, 0) {
            return;
        }
        let result = SpellResult {
            armor_multiplier: 0.0,
            target: crate::core::fight::Side::Player,
            attacker: fight.spells[spell].caster,
            outcome: 0,
            damage: 0.0,
            threat: 0.0,
        };
        fight.schedule_delayed_proc(self.aura, spell, result);
    }

    /// The energy trigger's `OnSpellHitDealt`.
    pub(crate) fn on_spell_hit_dealt(
        &self,
        fight: &mut Fight<DruidAgent>,
        spell: SpellId,
        result: &SpellResult,
    ) {
        if !self.energy_spells[spell] || result.outcome & OUTCOME_LANDED == 0 {
            return;
        }
        if !self.roll(fight, 1) {
            return;
        }
        fight.schedule_delayed_proc(self.aura, spell, *result);
    }

    /// The Rage trigger's `OnSpellHitTaken` for the target's melee swing.
    pub(crate) fn on_enemy_hit_taken(&self, fight: &mut Fight<DruidAgent>, result: &SpellResult) {
        if !self.roll(fight, 2) {
            return;
        }
        fight.schedule_delayed_proc(self.aura, TARGET_SWING, *result);
    }

    /// The handlers, told apart by the spell that procced them.
    pub(crate) fn on_delayed_proc(&self, fight: &mut Fight<DruidAgent>, spell: SpellId) {
        if spell == TARGET_SWING {
            if fight.rage.is_some() {
                fight.add_rage(self.amounts[2], self.metrics[2]);
            }
        } else if self.energy_spells[spell] {
            if fight.energy.is_some() {
                fight.add_energy(self.amounts[1], self.metrics[1]);
            }
        } else if fight.has_mana_bar() {
            fight.add_mana(self.amounts[0], self.metrics[0]);
        }
    }
}

/// Symbols of Unending Life's finisher refund: energy when Ferocious Bite or Rip misses, is
/// dodged, blocked or parried.
#[derive(Clone, Debug)]
pub(crate) struct UnendingLifeRefund {
    aura: AuraRef,
    energy: f64,
    spells: Vec<bool>,
    metrics: usize,
}

pub(crate) fn bind_unending_life(
    fight: &mut Fight<DruidAgent>,
    aura: &str,
    energy: f64,
    spells: &[usize],
    metrics_action_id: &ActionId,
) -> Result<UnendingLifeRefund, String> {
    let aura = fight.player_aura(aura)?;
    let metrics = fight.new_resource_metrics(metrics_action_id.clone(), ResourceKind::Energy);
    Ok(UnendingLifeRefund {
        aura,
        energy,
        spells: mask(fight.spells.len(), spells),
        metrics,
    })
}

impl UnendingLifeRefund {
    pub(crate) fn on_spell_hit_dealt(
        &self,
        fight: &mut Fight<DruidAgent>,
        spell: SpellId,
        result: &SpellResult,
    ) {
        if self.spells[spell] && result.outcome & OUTCOME_AVOIDED != 0 {
            fight.schedule_delayed_proc(self.aura, spell, *result);
        }
    }

    pub(crate) fn on_delayed_proc(&self, fight: &mut Fight<DruidAgent>) {
        fight.add_energy(self.energy, self.metrics);
    }
}
