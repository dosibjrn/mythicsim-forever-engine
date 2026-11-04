//! The party's mana totems in the results (README, Rust-only mechanics): Mana Spring Totem's
//! party buff and Mana Tide Totem are MP5 auras whose mana arrives inside the player's regular
//! mana ticks. A tick's share from each totem moves from the regeneration metrics to the
//! totem's own, so the results name the totem. The mana gained is the same; only the metrics it
//! is booked under change.

use super::{Agent, Fight, ResourceKind};
use crate::contracts::prepared_v2::Mp5Source as Exported;

/// A totem whose MP5 the player's mana ticks carry.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Mp5Source {
    metrics: usize,
    mp5: f64,
    /// The stat aura's place in `Fight::stat_mask` (first bit, bits) while the totem comes and
    /// goes; none for a permanent buff.
    place: Option<(u32, u32)>,
}

impl<A: Agent> Fight<A> {
    /// Registers the prepared player's sources, each with its own metrics.
    pub(crate) fn bind_mp5_sources(
        &mut self,
        exported: &[Exported],
        places: &[(u32, u32)],
    ) -> Result<(), String> {
        for source in exported {
            let place = match &source.aura {
                None => None,
                Some(label) => {
                    let index = self
                        .stat_aura_labels
                        .iter()
                        .position(|stat| stat == label)
                        .ok_or_else(|| format!("MP5 source aura {label} is not a stat aura"))?;
                    Some(places[index])
                }
            };
            let metrics = self.new_resource_metrics(source.action_id.clone(), ResourceKind::Mana);
            // Regeneration: a gain that causes no threat.
            self.resources[metrics].is_mana_regen = true;
            self.mp5_sources.push(Mp5Source {
                metrics,
                mp5: source.mp5,
                place,
            });
        }
        Ok(())
    }

    fn mp5_source_up(&self, source: &Mp5Source) -> bool {
        source
            .place
            .is_none_or(|(offset, width)| (self.stat_mask >> offset) & ((1 << width) - 1) != 0)
    }

    /// Moves the share of a player's mana tick the active sources' MP5 gave from the tick's
    /// metrics to theirs. `gain` and `actual` are what the tick's metrics still hold of the tick
    /// after the spirit regeneration sources took theirs. As with those, the rest of the
    /// regeneration fills the mana bar first; only the room left is the sources' actual gain.
    pub(crate) fn credit_mp5_sources(&mut self, gain: f64, actual: f64, metrics: usize) {
        if self.mp5_sources.is_empty() {
            return;
        }
        let per_tick = |mp5: f64, multiplier: f64| (mp5 / 5.0 * multiplier * 2.0).max(0.0);
        let multiplier = self.player.mana_regen_multiplier;
        let total: f64 = self
            .mp5_sources
            .iter()
            .filter(|source| self.mp5_source_up(source))
            .map(|source| per_tick(source.mp5, multiplier))
            .sum();
        if total <= 0.0 {
            return;
        }
        let bonus = total.min(gain.max(0.0));
        if bonus <= 0.0 {
            return;
        }
        let bonus_actual = bonus.min((actual - (gain - bonus)).max(0.0));
        let resource = &mut self.resources[metrics];
        resource.gain -= bonus;
        resource.actual_gain -= bonus_actual;
        for index in 0..self.mp5_sources.len() {
            let source = self.mp5_sources[index];
            if !self.mp5_source_up(&source) {
                continue;
            }
            let share = per_tick(source.mp5, multiplier) / total;
            let resource = &mut self.resources[source.metrics];
            resource.events += 1;
            resource.gain += bonus * share;
            resource.actual_gain += bonus_actual * share;
        }
    }
}
