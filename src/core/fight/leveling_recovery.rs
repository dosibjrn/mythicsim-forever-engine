//! Decisions' ordinary vendor food/water and threshold-based between-pull recovery.
//!
//! Ported from forever-sim src/rulesets/forever/shared/recovery.py and
//! src/game/recovery_between_pulls.py. The Rust actor and native event queue
//! remain the only authorities for health, mana, cooldowns and recovery time.
//! Neither food nor water is applied during a 2D world movement step.

use super::{world_bridge, reads, Action, Agent, Fight, ResourceKind};
use crate::contracts::prepared_v2::ActionId;
use crate::core::time::NS_PER_SECOND;

const STEP_NS: i64 = 100_000_000;
const MAX_REST_NS: i64 = 300 * NS_PER_SECOND;
const RESOURCE_EPSILON: f64 = 1e-6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum WorldRecoveryStrategy {
    Fixed(u32),
    Decisions { health_fraction: f64, mana_fraction: f64 },
}

impl WorldRecoveryStrategy {
    /// Scenario input validation, before any paid native/world execution.
    /// The same thresholds are checked defensively by the recovery executor.
    pub(crate) fn validate(self) -> Result<(), &'static str> {
        match self {
            Self::Fixed(seconds) if !(1..=120).contains(&seconds) =>
                Err("RUST_WORLD_RECOVERY_SECONDS_INVALID"),
            Self::Decisions { health_fraction, mana_fraction }
                if !health_fraction.is_finite()
                    || !mana_fraction.is_finite()
                    || !(0.0..=1.0).contains(&health_fraction)
                    || !(0.0..=1.0).contains(&mana_fraction) =>
                Err("RUST_WORLD_DECISIONS_THRESHOLDS_INVALID"),
            _ => Ok(()),
        }
    }
}


#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VendorTier {
    pub(crate) required_level: i32,
    pub(crate) amount: f64,
    pub(crate) duration_s: f64,
}

impl VendorTier {
    pub(crate) fn rate_per_second(self) -> f64 {
        self.amount / self.duration_s
    }
}

const FOOD: [VendorTier; 7] = [
    VendorTier { required_level: 1, amount: 61.2, duration_s: 18.0 },
    VendorTier { required_level: 5, amount: 243.6, duration_s: 21.0 },
    VendorTier { required_level: 15, amount: 552.0, duration_s: 24.0 },
    VendorTier { required_level: 25, amount: 874.8, duration_s: 27.0 },
    VendorTier { required_level: 35, amount: 1392.0, duration_s: 30.0 },
    VendorTier { required_level: 45, amount: 2148.0, duration_s: 30.0 },
    VendorTier { required_level: 55, amount: 2452.0, duration_s: 30.0 },
];
const WATER: [VendorTier; 6] = [
    VendorTier { required_level: 1, amount: 151.2, duration_s: 18.0 },
    VendorTier { required_level: 5, amount: 436.8, duration_s: 21.0 },
    VendorTier { required_level: 15, amount: 835.2, duration_s: 24.0 },
    VendorTier { required_level: 25, amount: 1344.6, duration_s: 27.0 },
    VendorTier { required_level: 35, amount: 1992.0, duration_s: 30.0 },
    VendorTier { required_level: 45, amount: 2934.0, duration_s: 30.0 },
];

fn tier(rows: &[VendorTier], level: i32) -> Option<VendorTier> {
    rows.iter().copied().rfind(|tier| tier.required_level <= level)
}

pub(crate) fn food_tier(level: i32) -> Option<VendorTier> {
    tier(&FOOD, level)
}

pub(crate) fn water_tier(level: i32) -> Option<VendorTier> {
    tier(&WATER, level)
}

pub(crate) fn needs_full_recovery(
    health: f64, maximum_health: f64, mana: f64, maximum_mana: f64,
    health_fraction: f64, mana_fraction: f64,
) -> bool {
    (maximum_health > 0.0 && health < maximum_health * health_fraction)
        || (maximum_mana > 0.0 && mana < maximum_mana * mana_fraction)
}

/// A world boundary supplies movement at the end of a paid step. A movement
/// transition to stationary does not make the preceding interval stationary:
/// never grant food/water for a step that includes world travel.
fn vendor_recovery_step_stationary(moving_before: bool, moving_after: bool) -> bool {
    !moving_before && !moving_after
}

#[derive(Default, Clone, Copy, Debug)]
pub(crate) struct WorldRecoveryReceipt {
    pub(crate) passive_ticks: usize,
    pub(crate) elapsed_ns: i64,
    pub(crate) food_ns: i64,
    pub(crate) water_ns: i64,
    pub(crate) food_health_gain: f64,
    pub(crate) water_mana_gain: f64,
    pub(crate) full_recovery: bool,
}

impl<A: Agent> Fight<A> {
    /// Decisions' threshold recovery: below either configured fraction,
    /// regenerate to full, using level-appropriate food and water together.
    /// Otherwise travel immediately, preserving current HP/mana. Unlike the
    /// old fixed twelve-second rest, all elapsed time is earned on the actor
    /// clock; native scheduled passive ticks continue during both travel/rest.
    ///
    /// The 2D controller schedules travel after each lethal ACK. We observe its
    /// moving flag on every step and only credit food/water once movement ends,
    /// since eating/drinking while walking is not legal in-game.
    pub(crate) fn recover_native_world_actor_decisions(
        &mut self, health_fraction: f64, mana_fraction: f64,
        food_metrics: usize, water_metrics: usize,
    ) -> Result<WorldRecoveryReceipt, &'static str> {
        if !health_fraction.is_finite() || !mana_fraction.is_finite()
            || !(0.0..=1.0).contains(&health_fraction)
            || !(0.0..=1.0).contains(&mana_fraction)
        {
            return Err("RUST_WORLD_DECISIONS_THRESHOLDS_INVALID");
        }
        let food = food_tier(self.config.player_level)
            .ok_or("RUST_WORLD_DECISIONS_FOOD_TIER_UNAVAILABLE")?;
        let water = water_tier(self.config.player_level)
            .ok_or("RUST_WORLD_DECISIONS_WATER_TIER_UNAVAILABLE")?;
        let initial_ns = self.now;
        let full = needs_full_recovery(
            self.player.health, self.player_max_health(),
            self.player.mana, self.player.powers.max_mana,
            health_fraction, mana_fraction);
        let mut receipt = WorldRecoveryReceipt {
            full_recovery: full, ..WorldRecoveryReceipt::default()
        };

        loop {
            if self.now - initial_ns >= MAX_REST_NS {
                return Err("RUST_WORLD_DECISIONS_RECOVERY_TIMEOUT");
            }
            let boundary = self.now.checked_add(STEP_NS)
                .ok_or("RUST_WORLD_TARGET_CLOCK_OVERFLOW")?;
            while let Some((at, handle, action)) = self.queue.pop_next_matching_through(
                boundary,
                |action| matches!(action, Action::ManaTick | Action::HealthRegenTick { .. }),
            ) {
                if at > self.now {
                    self.advance_to(at);
                }
                if self.handle_still_valid(handle, &action) {
                    self.run_action(handle, action);
                    receipt.passive_ticks += 1;
                }
            }
            self.advance_to(boundary);
            let moving_before_step = self.player.moving;
            let (distance, front, moving) =
                world_bridge::exchange(boundary, self.duration, self.encounter_damage_taken);
            if distance != self.config.distance {
                self.config.distance = distance;
                self.changed(reads::CASTS);
            }
            if self.config.melee.in_front_of_target != front {
                self.config.melee.in_front_of_target = front;
                self.changed(reads::CASTS);
            }
            if self.player.moving != moving {
                self.player.moving = moving;
                self.changed(reads::CASTS);
            }

            if full && vendor_recovery_step_stationary(moving_before_step, moving) {
                // Each channel is credited only while it has a deficit and
                // the actor is stationary. Passive native mana/health ticks
                // have already run on this same step and count toward full.
                let hp_deficit = (self.player_max_health() - self.player.health).max(0.0);
                if hp_deficit > RESOURCE_EPSILON {
                    let before = self.player.health;
                    self.gain_health((food.rate_per_second() * STEP_NS as f64
                        / NS_PER_SECOND as f64).min(hp_deficit), food_metrics);
                    receipt.food_health_gain += self.player.health - before;
                    receipt.food_ns += STEP_NS;
                }
                let mana_deficit =
                    (self.player.powers.max_mana - self.player.mana).max(0.0);
                if mana_deficit > RESOURCE_EPSILON {
                    let before = self.player.mana;
                    self.add_mana((water.rate_per_second() * STEP_NS as f64
                        / NS_PER_SECOND as f64).min(mana_deficit), water_metrics);
                    receipt.water_mana_gain += self.player.mana - before;
                    receipt.water_ns += STEP_NS;
                }
            }

            // No fixed downtime: the first safe target-ready instant ends rest.
            // This also means an undamaged actor can go directly to the next
            // mob after finishing its actual 2D travel.
            if !moving && (!full ||
                (self.player.health >= self.player_max_health() - RESOURCE_EPSILON
                 && self.player.mana >=
                    self.player.powers.max_mana - RESOURCE_EPSILON))
            {
                break;
            }
        }
        receipt.elapsed_ns = self.now - initial_ns;
        Ok(receipt)
    }

    pub(crate) fn register_world_food_water_metrics(&mut self) -> (usize, usize) {
        let food_metrics = self.new_resource_metrics(ActionId {
            other_id: "OtherActionFood".into(), ..ActionId::default()
        }, ResourceKind::Health);
        let water_metrics = self.new_resource_metrics(ActionId {
            other_id: "OtherActionWater".into(), ..ActionId::default()
        }, ResourceKind::Mana);
        (food_metrics, water_metrics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_world_recovery_scenario_is_refused_before_first_combat() {
        assert_eq!(WorldRecoveryStrategy::Fixed(0).validate(),
            Err("RUST_WORLD_RECOVERY_SECONDS_INVALID"));
        assert_eq!(WorldRecoveryStrategy::Fixed(121).validate(),
            Err("RUST_WORLD_RECOVERY_SECONDS_INVALID"));
        for fraction in [-1.0, 1.1, f64::NAN, f64::INFINITY] {
            assert_eq!(WorldRecoveryStrategy::Decisions {
                health_fraction: fraction, mana_fraction: 0.5,
            }.validate(), Err("RUST_WORLD_DECISIONS_THRESHOLDS_INVALID"));
            assert_eq!(WorldRecoveryStrategy::Decisions {
                health_fraction: 0.5, mana_fraction: fraction,
            }.validate(), Err("RUST_WORLD_DECISIONS_THRESHOLDS_INVALID"));
        }
        assert!(WorldRecoveryStrategy::Fixed(1).validate().is_ok());
        assert!(WorldRecoveryStrategy::Fixed(120).validate().is_ok());
        assert!(WorldRecoveryStrategy::Decisions {
            health_fraction: 0.0, mana_fraction: 1.0,
        }.validate().is_ok());
    }

    #[test]
    fn vendor_levels_are_the_same_as_decisions() {
        assert_eq!(food_tier(20).unwrap().amount, 552.0);
        assert_eq!(water_tier(20).unwrap().amount, 835.2);
        assert_eq!(food_tier(30).unwrap().amount, 874.8);
        assert_eq!(water_tier(30).unwrap().amount, 1344.6);
        assert_eq!(food_tier(60).unwrap().amount, 2452.0);
        assert_eq!(water_tier(60).unwrap().amount, 2934.0);
        assert!(food_tier(0).is_none());
    }

    #[test]
    fn decisions_recovery_is_threshold_driven_not_every_mob() {
        assert!(!needs_full_recovery(600.0, 1000.0, 400.0, 1000.0, 0.5, 0.1));
        assert!(needs_full_recovery(499.0, 1000.0, 400.0, 1000.0, 0.5, 0.1));
        assert!(needs_full_recovery(600.0, 1000.0, 99.0, 1000.0, 0.5, 0.1));
    }

    #[test]
    fn vendor_recovery_never_credits_a_moving_interval() {
        assert!(vendor_recovery_step_stationary(false, false));
        assert!(!vendor_recovery_step_stationary(false, true));
        // Arrival only changes movement state at the boundary: the interval
        // ending at that boundary was still travel, not paid vendor recovery.
        assert!(!vendor_recovery_step_stationary(true, false));
        assert!(!vendor_recovery_step_stationary(true, true));
    }

    #[test]
    fn food_water_rates_are_concurrent_and_level_scoped() {
        let food = food_tier(20).unwrap();
        let water = water_tier(20).unwrap();
        let food_time = 552.0 / food.rate_per_second();
        let water_time = 835.2 / water.rate_per_second();
        assert_eq!(food_time, 24.0);
        assert_eq!(water_time, 24.0);
        assert_eq!(food_time.max(water_time), 24.0);
    }
}
