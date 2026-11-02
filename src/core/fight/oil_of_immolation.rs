//! Oil of Immolation (MythicSim Rust only, client 70707 item 8956): the character uses the oil
//! half a second before the pull and again each time its Fire Shield ends, and every period of
//! the shield casts the trigger spell, a flat Fire hit on each target.
//!
//! The use has no global cooldown and shares no cooldown with the potions, so the cycle is a
//! chain of actions that needs nothing of the rotation. The shield's last tick falls on the
//! moment it ends, before the next use. The fight models one distance between the player and the
//! targets, so the shield reaches all of them or none: a character beyond the radius gets no
//! tick.
//!
//! A use stalls the swings: each melee swing timer that is running when the oil is used (a hand
//! that is swinging and not yet due) is pushed back by the effect's `swing_delay_ns`. The first
//! use comes `prepot_ns` before the pull, when no swing is running (they start with the
//! encounter), so only the uses that renew the shield in combat cost swing time. The delay is
//! the same one the pull's random melee timing adds: the swing's time moves, nothing else.
//! Ranged auto shots are not affected.

use crate::contracts::prepared_v2::Effect;

use super::{Action, Agent, AuraRef, Fight, Side, SpellId, PRIORITY_AUTO, PRIORITY_PREPULL};

/// The bound oil: its aura, the tick spell and the shield's shape.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OilOfImmolation {
    aura: AuraRef,
    spell: SpellId,
    period: i64,
    ticks: i32,
    radius: f64,
    /// How far each running melee swing is pushed back by a use.
    swing_delay: i64,
    /// How long before the pull the first use happens.
    prepot: i64,
}

impl<A: Agent> Fight<A> {
    /// Bind the oil's aura and its tick spell.
    pub(crate) fn bind_oil_of_immolation(&mut self, effects: &[Effect]) -> Result<(), String> {
        for effect in effects {
            if let Effect::OilOfImmolation {
                aura,
                spell,
                period_ns,
                ticks,
                radius,
                swing_delay_ns,
                prepot_ns,
                ..
            } = effect
            {
                self.oil = Some(OilOfImmolation {
                    aura: self.player_aura(aura)?,
                    spell: *spell,
                    period: *period_ns,
                    ticks: *ticks,
                    radius: *radius,
                    swing_delay: *swing_delay_ns,
                    prepot: *prepot_ns,
                });
            }
        }
        Ok(())
    }

    /// The first use, `prepot` before the pull: the shield is up from the pull. The ticks fall
    /// 3 s apart from the use, so the first lands `prepot` before 3 s after the pull.
    pub(crate) fn oil_of_immolation_reset(&mut self) {
        if let Some(oil) = self.oil {
            self.schedule(-oil.prepot, PRIORITY_PREPULL, Action::OilUse);
        }
    }

    /// The moment the fight's clock starts for the oil: its first use, if that comes before
    /// anything else in the prepull.
    pub(crate) fn oil_of_immolation_start(&self) -> Option<i64> {
        self.oil.map(|oil| -oil.prepot)
    }

    /// The character uses the oil: the shield goes up, or is refreshed, the running melee swings
    /// are stalled and its first period starts.
    pub(crate) fn oil_use(&mut self) {
        let Some(oil) = self.oil else { return };
        self.delay_running_melee_swings(oil.swing_delay);
        self.activate_aura(oil.aura);
        self.schedule(self.now + oil.period, PRIORITY_AUTO, Action::OilTick(1));
    }

    /// One period of the shield: the trigger spell on every target while the player stands within
    /// the radius, then the next period or, after the last, the next use.
    pub(crate) fn oil_tick(&mut self, tick: i32) {
        let Some(oil) = self.oil else { return };
        if self.distance_now(Side::Player) <= oil.radius {
            self.cast(oil.spell, Side::Player);
        }
        if tick < oil.ticks {
            self.schedule(
                self.now + oil.period,
                PRIORITY_AUTO,
                Action::OilTick(tick + 1),
            );
        } else {
            self.oil_use();
        }
    }
}
