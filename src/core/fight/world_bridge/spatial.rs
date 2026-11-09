//! Deterministic 2D world layout for the in-process leveling evaluator.
//!
//! Port of forever-sim/src/scenarios/spaced_pulls.py geometry, not a second
//! damage, hit, spell, food, or mana simulator. Exact positional parity must
//! pass before replacing the external gsim event bridge in production.
//! The combat core remains the only owner of Warcraft mechanics.

use std::cmp::Ordering;
use std::f64::consts::TAU;

pub(super) const PITCH_YARDS: f64 = 20.0;
pub(super) const MOVEMENT_SPEED_YARDS_S: f64 = 7.0;
pub(super) const MELEE_RADIUS_YARDS: f64 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn distance(self, other: Point) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

#[derive(Debug, Clone)]
pub(super) struct SpacedWorld {
    /// Actual 2D Chebyshev ring coordinates, in original source order.
    pub positions: Vec<Point>,
    /// Nearest-unvisited mob index sequence, starting with zero.
    pub route: Vec<usize>,
}

impl SpacedWorld {
    /// Identical ring sorting and tie rules as the gsim scenario source.
    pub fn new(target_count: usize) -> Result<Self, &'static str> {
        if !(1..=100).contains(&target_count) {
            return Err("NATIVE_SPATIAL_TARGET_COUNT_INVALID");
        }
        let mut positions = vec![Point { x: 0.0, y: 0.0 }];
        let mut radius = 1_i32;
        while positions.len() < target_count {
            let mut ring = Vec::new();
            for q in -radius..=radius {
                for r in -radius..=radius {
                    if q.abs().max(r.abs()) == radius {
                        ring.push(Point {
                            x: f64::from(q) * PITCH_YARDS,
                            y: f64::from(r) * PITCH_YARDS,
                        });
                    }
                }
            }
            let angle = |p: &Point| p.y.atan2(p.x).rem_euclid(TAU);
            ring.sort_by(|a, b| {
                angle(a)
                    .partial_cmp(&angle(b))
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| a.x.partial_cmp(&b.x).unwrap_or(Ordering::Equal))
                    .then_with(|| a.y.partial_cmp(&b.y).unwrap_or(Ordering::Equal))
            });
            positions.extend(ring.into_iter().take(target_count - positions.len()));
            radius += 1;
        }

        let mut route = vec![0_usize];
        let mut visited = vec![false; target_count];
        visited[0] = true;
        while route.len() < target_count {
            let from = *route.last().expect("route starts with zero");
            let next = (0..target_count)
                .filter(|i| !visited[*i])
                .min_by(|a, b| {
                    positions[from]
                        .distance(positions[*a])
                        .partial_cmp(&positions[from].distance(positions[*b]))
                        .unwrap_or(Ordering::Equal)
                        .then_with(|| a.cmp(b))
                })
                .expect("unvisited mob exists");
            visited[next] = true;
            route.push(next);
        }

        Ok(Self { positions, route })
    }

    /// The source scenario's physical travel distance in 2D, not a
    /// manufactured fixed 20-yard cost per mob.
    pub fn route_distance_yards(&self) -> f64 {
        self.route.windows(2).map(|pair| {
            self.positions[pair[0]].distance(self.positions[pair[1]])
        }).sum()
    }

    /// Lower bound of approach walking; no invented combat/travel overlap.
    pub fn approach_distance_yards(&self) -> f64 {
        self.route.windows(2).map(|pair| {
            (self.positions[pair[0]].distance(self.positions[pair[1]])
                - MELEE_RADIUS_YARDS).max(0.0)
        }).sum()
    }

    pub fn initial_melee_targets(&self) -> Vec<usize> {
        self.positions.iter().enumerate()
            .filter_map(|(index, p)| {
                (p.distance(self.positions[0]) <= MELEE_RADIUS_YARDS + 1e-9)
                    .then_some(index)
            })
            .collect()
    }
}


/**
 * One in-process serial target ledger. The only damage supplied to this
 * object is damage already emitted by the real native Fight scheduler.
 * Navigation pays for actual Euclidean movement; it does not synthesize
 * spells, enemy attacks, regeneration, or a kill.
 */
#[derive(Debug)]
pub(super) struct NativeSpatialWorld {
    layout: SpacedWorld,
    route_cursor: usize,
    player: Point,
    engagement_range_yards: f64,
    max_target_health: f64,
    target_health: f64,
    last_walk_ns: i64,
}

impl NativeSpatialWorld {
    pub fn new(
        targets: usize,
        target_health: f64,
        engagement_range_yards: f64,
    ) -> Result<Self, &'static str> {
        if !target_health.is_finite() || target_health <= 0.0 {
            return Err("NATIVE_SPATIAL_TARGET_HEALTH_INVALID");
        }
        if !engagement_range_yards.is_finite()
            || !(0.0..=100.0).contains(&engagement_range_yards)
        {
            return Err("NATIVE_SPATIAL_ENGAGEMENT_RANGE_INVALID");
        }
        let layout = SpacedWorld::new(targets)?;
        Ok(Self {
            layout,
            route_cursor: 0,
            player: Point { x: engagement_range_yards, y: 0.0 },
            engagement_range_yards,
            max_target_health: target_health,
            target_health,
            last_walk_ns: 0,
        })
    }

    pub fn target_index(&self) -> usize {
        self.layout.route[self.route_cursor]
    }

    pub fn target_health(&self) -> f64 {
        self.target_health
    }

    pub fn distance_yards(&self) -> f64 {
        self.player.distance(self.layout.positions[self.target_index()])
    }

    pub fn last_walk_ns(&self) -> i64 {
        self.last_walk_ns
    }

    /// Ingest a real native damage event, not an inferred DPS or enemy reset.
    /// Multiple native effects arriving at one scheduler boundary may overkill.
    pub fn apply_native_damage(&mut self, damage: f64) -> Result<(), &'static str> {
        if !damage.is_finite() || damage <= 0.0 {
            return Err("NATIVE_SPATIAL_DAMAGE_INVALID");
        }
        self.target_health = (self.target_health - damage).max(0.0);
        Ok(())
    }

    /// Advance only after a native lethal ACK. The player stops at its supplied
    /// engagement range, then the native actor must pay for this walking time.
    pub fn next_target(&mut self) -> Result<(), &'static str> {
        if self.target_health > 0.0 {
            return Err("NATIVE_SPATIAL_TARGET_NOT_LETHAL");
        }
        if self.route_cursor + 1 >= self.layout.route.len() {
            return Err("NATIVE_SPATIAL_ROUTE_COMPLETE");
        }
        self.route_cursor += 1;
        let target = self.layout.positions[self.target_index()];
        let dx = target.x - self.player.x;
        let dy = target.y - self.player.y;
        let distance = dx.hypot(dy);
        let walking_yards = (distance - self.engagement_range_yards).max(0.0);
        if distance > 0.0 {
            self.player.x += dx * walking_yards / distance;
            self.player.y += dy * walking_yards / distance;
        }
        let ns = (walking_yards / MOVEMENT_SPEED_YARDS_S * 1_000_000_000.0).ceil();
        if !ns.is_finite() || ns > i64::MAX as f64 {
            return Err("NATIVE_SPATIAL_TRAVEL_TIME_OVERFLOW");
        }
        self.last_walk_ns = ns as i64;
        self.target_health = self.max_target_health;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_gsim_eight_target_chebyshev_ring_and_route() {
        // Source: forever-sim/src/scenarios/spaced_pulls.py.
        let world = SpacedWorld::new(8).unwrap();
        assert_eq!(
            world.positions,
            vec![
                Point { x: 0., y: 0. },
                Point { x: 20., y: 0. },
                Point { x: 20., y: 20. },
                Point { x: 0., y: 20. },
                Point { x: -20., y: 20. },
                Point { x: -20., y: 0. },
                Point { x: -20., y: -20. },
                Point { x: 0., y: -20. },
            ]
        );
        assert_eq!(world.route, vec![0, 1, 2, 3, 4, 5, 6, 7]);
        assert!((world.route_distance_yards() - 140.).abs() < 1e-9);
        assert!((world.approach_distance_yards() - 105.).abs() < 1e-9);
        assert_eq!(world.initial_melee_targets(), vec![0]);
    }

    #[test]
    fn hundred_target_world_is_a_2d_ring_not_a_1d_row() {
        let world = SpacedWorld::new(100).unwrap();
        assert_eq!(world.positions.len(), 100);
        assert_eq!(world.route.len(), 100);
        let mut ids = world.route.clone();
        ids.sort_unstable();
        assert_eq!(ids, (0..100).collect::<Vec<_>>());
        assert!(world.positions.iter().any(|p| p.x != 0. && p.y != 0.));
        assert!(world.positions.iter().any(|p| p.x < 0. && p.y < 0.));
        assert!(world.route_distance_yards().is_finite());
        assert!(world.route_distance_yards() >= world.approach_distance_yards());
    }

    #[test]
    fn near_target_tie_breaks_by_source_index() {
        let world = SpacedWorld::new(2).unwrap();
        assert_eq!(world.route, vec![0, 1]);
        assert!((world.positions[1].distance(world.positions[0]) - 20.).abs() < 1e-9);
    }

    #[test]
    fn reject_non_source_target_counts() {
        assert_eq!(SpacedWorld::new(0).unwrap_err(), "NATIVE_SPATIAL_TARGET_COUNT_INVALID");
        assert_eq!(SpacedWorld::new(101).unwrap_err(), "NATIVE_SPATIAL_TARGET_COUNT_INVALID");
    }
}
