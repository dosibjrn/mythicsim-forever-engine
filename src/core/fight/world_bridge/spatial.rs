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
        if target_count == 0 || target_count > 100 {
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
