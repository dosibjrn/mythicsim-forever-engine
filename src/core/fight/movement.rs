//! Go movement.go: a unit moving toward or away from its target.
//!
//! A move casts the unit's Movement spell, whose aura stacks count the yards from the target,
//! and a pending action ends it at the time the distance takes at the unit's speed. The
//! unit's position is read lazily: each rotation evaluation and each new move bring it up to
//! date, which updates the aura's stacks and starts or stops the unit's swings as it enters or
//! leaves their range. A pet moves at the speed the exporter read; the player at seven yards a
//! second times its movement speed multiplier, which a class's dash aura changes.

use crate::core::{queue::Handle, time::NS_PER_SECOND};

use super::{Action, Agent, AuraRef, Fight, Side, SpellId, PRIORITY_GCD};

/// Go `MovementUpdateType`: when a unit's movement callbacks run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MovementKind {
    Start,
    Update,
    End,
}

/// Go `MovementAction`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Movement {
    src_position: f64,
    move_distance: f64,
    start: i64,
    /// Go `NextActionAt` of the pending action.
    end: i64,
    speed: f64,
    pub(crate) action: Handle,
}

impl Movement {
    /// Go `MovementAction.NextActionAt`: when the move ends.
    pub(crate) fn end(&self) -> i64 {
        self.end
    }
}

impl<A: Agent> Fight<A> {
    /// The unit's Movement aura and spell, by Go's `OtherActionMove`.
    fn movement_parts(&self, side: Side) -> (AuraRef, SpellId) {
        let index = self.trackers[side.index()]
            .auras
            .iter()
            .position(|aura| {
                aura.action_id
                    .as_ref()
                    .is_some_and(|id| id.other_id == "OtherActionMove")
            })
            .expect("the unit has a Movement aura");
        let spell = (0..self.spells.len())
            .find(|&spell| {
                self.spells[spell].caster == side
                    && self.spells[spell].id.other_id == "OtherActionMove"
            })
            .expect("the unit has a Movement spell");
        (AuraRef { side, index }, spell)
    }

    /// The Movement spell's `ApplyEffects`: the aura, its stacks the yards from the target.
    pub(crate) fn apply_movement(&mut self, side: Side) {
        let (aura, _) = self.movement_parts(side);
        self.activate_aura(aura);
        let yards = (self.unit_config(side).distance as i32).max(1);
        self.set_stacks(aura, yards);
    }

    /// The Movement aura's gain and expiry, which records when the unit stopped.
    pub(crate) fn movement_changed(&mut self, side: Side, gained: bool) {
        self.changed(super::reads::CASTS);
        let now = self.now;
        let unit = self.unit_mut(side);
        unit.moving = gained;
        if !gained {
            unit.movement = None;
            unit.movement_ended_at = now;
        }
    }

    /// Go `GetMovementSpeed`: a pet's exported speed, or the player's seven yards a second times
    /// its multiplier.
    fn movement_speed(&self, side: Side) -> f64 {
        match side {
            Side::Player => 7.0 * self.move_multiplier,
            _ => {
                assert!(side.is_pet(), "only a pet or the player moves");
                self.active_pet(side).movement_speed
            }
        }
    }

    /// Go `MultiplyMovementSpeed` of the player: the multiplier changes, and a move in progress
    /// at a speed is run again to the destination it had, at the new speed (fork patch 121; it
    /// used to run again to the move's length, as if that were the destination).
    pub(crate) fn multiply_movement_speed(&mut self, amount: f64) {
        let old_multiplier = self.move_multiplier;
        let old_speed = self.movement_speed(Side::Player);
        self.move_multiplier *= amount;
        if self.log.is_some() {
            let line = format!(
                "[DEBUG] Movement speed changed from {:.2} ({:.2}%) to {:.2} ({:.2}%)",
                old_speed,
                (old_multiplier - 1.0) * 100.0,
                self.movement_speed(Side::Player),
                (self.move_multiplier - 1.0) * 100.0
            );
            self.player_log(&line);
        }
        if let Some(movement) = self.player.movement {
            if movement.speed != 0.0 {
                let destination = movement.src_position + movement.move_distance;
                self.move_to(Side::Player, destination);
            }
        }
    }

    /// Go `Unit.MoveTo`.
    pub(crate) fn move_to(&mut self, side: Side, range: f64) {
        if range == self.unit_config(side).distance {
            return;
        }
        self.update_position(side, false);
        self.start_attack_for_walk_in(side, range);
        let distance = range - self.unit_config(side).distance;
        let speed = self.movement_speed(side);
        // Go time.Duration(math.Abs(d)/speed*1000) * time.Millisecond.
        let time = (distance.abs() / speed * 1000.0) as i64 * crate::core::time::NS_PER_MILLISECOND;
        let signed = if distance < 0.0 { -speed } else { speed };
        self.register_movement(side, signed, self.now + time, distance);
    }

    /// Go `Unit.MoveDuration`: a move that covers no distance and lasts the duration.
    pub(crate) fn move_duration(&mut self, side: Side, duration: i64) {
        if duration == 0 {
            return;
        }
        self.update_position(side, false);
        self.register_movement(side, 0.0, self.now + duration, 0.0);
    }

    /// Go `registerMovementAction`.
    fn register_movement(&mut self, side: Side, speed: f64, end: i64, distance: f64) {
        match self.unit(side).movement {
            Some(movement) => {
                self.queue.cancel(movement.action);
            }
            None => {
                let (_, spell) = self.movement_parts(side);
                let target = if side == Side::Player {
                    self.current_target_side()
                } else {
                    Side::Target
                };
                self.cast(spell, target);
            }
        }
        let src_position = self.unit_config(side).distance;
        A::on_movement(self, side, MovementKind::Start);
        let action = self.schedule(end, PRIORITY_GCD, Action::MovementEnd(side));
        let now = self.now;
        self.unit_mut(side).movement = Some(Movement {
            src_position,
            move_distance: distance,
            start: now,
            end,
            speed,
            action,
        });
    }

    /// Go `Unit.UpdatePosition`.
    pub(crate) fn update_position(&mut self, side: Side, last: bool) {
        if !self.unit(side).moving {
            return;
        }
        let Some(movement) = self.unit(side).movement else {
            return;
        };
        let old = self.unit_config(side).distance;
        // Forever's rule: a move whose end is now has arrived, even before its end action runs,
        // so the rotation woken at that instant reads the destination, not the walk rounded to
        // the millisecond a hair short of it.
        let arrived = last || (self.move_arrives_at_destination && self.now >= movement.end);
        let new = if arrived && movement.move_distance != 0.0 {
            movement.src_position + movement.move_distance
        } else {
            movement.src_position
                + (self.now - movement.start) as f64 * movement.speed / NS_PER_SECOND as f64
        };
        if old == new {
            return;
        }
        self.unit_config_mut(side).distance = new;
        self.changed(super::reads::CASTS);
        A::on_movement(self, side, MovementKind::Update);
        let in_range = self.main_hand_in_range(side);
        if self.autos_of(side).mh.enabled != in_range {
            if in_range {
                self.cancel_melee_weave_wakeup(side);
                self.enable_melee_swing(side);
                self.hold_arrival_swing_for_rotation(side);
            } else {
                self.cancel_melee_swing(side);
                self.schedule_melee_weave_wakeup(side);
            }
        }
        let in_range = self.ranged_in_range(side);
        if self.autos_of(side).ranged.enabled != in_range {
            if in_range {
                self.enable_ranged_swing(side);
            } else {
                self.cancel_ranged_swing(side);
            }
        }
        let yards = (new as i32).max(1);
        let (aura, _) = self.movement_parts(side);
        if yards != self.aura(aura).stacks {
            self.set_stacks(aura, yards);
        }
    }

    /// Go `Unit.DistanceNow` (fork patch 192): where the unit is at this moment, along its move
    /// if it is moving, without `update_position`'s side effects. In the rotation it equals the
    /// distance the rotation has just brought up to date.
    pub(crate) fn distance_now(&self, side: Side) -> f64 {
        if self.unit(side).moving {
            if let Some(movement) = self.unit(side).movement {
                if self.move_arrives_at_destination
                    && self.now >= movement.end
                    && movement.move_distance != 0.0
                {
                    return movement.src_position + movement.move_distance;
                }
                return movement.src_position
                    + (self.now - movement.start) as f64 * movement.speed / NS_PER_SECOND as f64;
            }
        }
        self.unit_config(side).distance
    }

    /// Go `Unit.FinalizeMovement`.
    pub(crate) fn finalize_movement(&mut self, side: Side) {
        if !self.unit(side).moving {
            return;
        }
        self.update_position(side, true);
        let (aura, _) = self.movement_parts(side);
        self.deactivate_aura(aura);
        A::on_movement(self, side, MovementKind::End);
    }

    pub(crate) fn main_hand_in_range(&self, side: Side) -> bool {
        let weapon = &self.autos_of(side).mh.weapon;
        let distance = self.unit_config(side).distance;
        (weapon.min_range == 0.0 || weapon.min_range < distance)
            && (weapon.max_range == 0.0 || weapon.max_range >= distance)
    }

    /// Go `WeaponAttack.IsInRange` of the ranged slot.
    pub(crate) fn ranged_in_range(&self, side: Side) -> bool {
        let weapon = &self.autos_of(side).ranged.weapon;
        let distance = self.unit_config(side).distance;
        (weapon.min_range == 0.0 || weapon.min_range < distance)
            && (weapon.max_range == 0.0 || weapon.max_range >= distance)
    }
}
