//! Auras that let more spirit regeneration continue while casting for a while: Resourcefulness,
//! Rapid Recuperation and Improved Stormstrike.
//!
//! The Go reference adds to `SpiritRegenRateCasting` without `UpdateManaRegenRates`, so the
//! cached tick ignores the buff until something else refreshes it (a change of MP5, Spirit or
//! Intellect). In the client the mana regeneration interrupt aura (type 134) acts at once, so
//! Forever's rule (README, Rust-only mechanics) refreshes the rates as the aura changes, and the
//! mana the aura adds to a tick lands in metrics of its own, named after it, as Innervate's does.

use super::{Agent, AuraRef, Fight, ResourceKind};

/// One aura's casting regeneration and, under Forever's rule, its metrics.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CastingRegen {
    regen: f64,
    metrics: Option<usize>,
}

/// An active casting regeneration aura that a mana tick credits.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CastingRegenSource {
    metrics: usize,
    regen: f64,
}

impl CastingRegen {
    /// The regeneration of `aura`, the share of spirit regeneration it lets continue.
    pub(crate) fn bind<A: Agent>(
        fight: &mut Fight<A>,
        aura: AuraRef,
        regen: f64,
    ) -> Result<Self, String> {
        let mut metrics = None;
        if fight.casting_regen_at_once {
            let id = fight.aura(aura).action_id.clone().ok_or_else(|| {
                format!(
                    "casting regeneration aura {} has no action id",
                    fight.aura(aura).label
                )
            })?;
            let index = fight.new_resource_metrics(id, ResourceKind::Mana);
            // Regeneration: a gain that causes no threat.
            fight.resources[index].is_mana_regen = true;
            metrics = Some(index);
        }
        Ok(Self { regen, metrics })
    }

    /// The aura's `OnGain` or `OnExpire`.
    pub(crate) fn changed<A: Agent>(self, fight: &mut Fight<A>, gained: bool) {
        if gained {
            fight.player.spirit_regen_rate_casting += self.regen;
        } else {
            fight.player.spirit_regen_rate_casting -= self.regen;
        }
        let Some(metrics) = self.metrics else {
            return;
        };
        let sources = &mut fight.player.casting_regen_sources;
        if gained {
            sources.push(CastingRegenSource {
                metrics,
                regen: self.regen,
            });
        } else if let Some(at) = sources.iter().position(|s| s.metrics == metrics) {
            sources.remove(at);
        }
        fight.update_mana_regen_rates();
    }
}

impl<A: Agent> Fight<A> {
    /// Moves the part of a casting mana tick the active casting regeneration auras added from the
    /// tick's metrics to theirs. As with Go's spirit regeneration attribution, ordinary
    /// regeneration fills the mana bar first; only the room left is the auras' actual gain.
    /// Returns the gain and actual gain moved.
    pub(crate) fn credit_casting_regen(
        &mut self,
        regen: f64,
        actual: f64,
        metrics: usize,
    ) -> (f64, f64) {
        if self.player.casting_regen_sources.is_empty() || self.player.force_full_spirit_regen {
            return (0.0, 0.0);
        }
        let per_rate = self.player.powers.spirit_regen_per_second
            * self.player.spirit_regen_multiplier
            * self.player.mana_regen_multiplier
            * 2.0;
        let total: f64 = self
            .player
            .casting_regen_sources
            .iter()
            .map(|source| (per_rate * source.regen).max(0.0))
            .sum();
        if total <= 0.0 {
            return (0.0, 0.0);
        }
        let bonus = total.min(regen);
        let bonus_actual = bonus.min((actual - (regen - bonus)).max(0.0));
        let resource = &mut self.resources[metrics];
        resource.gain -= bonus;
        resource.actual_gain -= bonus_actual;
        for index in 0..self.player.casting_regen_sources.len() {
            let source = self.player.casting_regen_sources[index];
            let share = (per_rate * source.regen).max(0.0) / total;
            let resource = &mut self.resources[source.metrics];
            resource.events += 1;
            resource.gain += bonus * share;
            resource.actual_gain += bonus_actual * share;
        }
        (bonus, bonus_actual)
    }
}
