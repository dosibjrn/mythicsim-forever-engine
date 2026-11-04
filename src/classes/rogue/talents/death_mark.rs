//! Death Mark (1316709 and ranks to 1325758), from the rework of push 113483 (Forever's rules
//! only): a finishing move that marks the target for 6 sec. The client's rows state its cost,
//! cooldown and duration, and its tooltip the rest: 12% of the damage the rogue's attacks deal
//! the target is stored for each Combo Point spent (Garrote and Rupture damage at twice the
//! rate), and when the mark ends the stored damage is dealt, up to a cap of
//! `(156 + points * (245 + 0.04 * attack power)) * (crit chance + 1)`. Coup de Grace multiplies
//! the rate and the cap.
//!
//! The tooltip does not say which damage counts or how the release rolls. Here every hit and tick
//! of the rogue's other spells counts (the damage dealt after armor, as a hit lands), a miss
//! stores nothing, and the release is a hit that cannot miss or crit (the cap holds the crit
//! factor) and takes no armor or multiplier again. A mark still up when the fight ends releases
//! nothing. Both are assumptions, flagged in the README.

use crate::core::fight::{Agent, AuraRef, Fight, Side, SpellId, SpellResult, OUTCOME_HIT};

use crate::classes::rogue::spells::finisher::Finisher;

#[derive(Clone, Debug)]
pub(crate) struct DeathMark {
    pub(crate) aura: AuraRef,
    spell: SpellId,
    stored_per_point: f64,
    cap_base: f64,
    cap_per_point: f64,
    attack_power_per_point: f64,
    coup_de_grace: f64,
    /// By spell, whether its damage is stored at twice the rate.
    doubled: Vec<bool>,
}

/// The mark on the target: the Combo Points spent, the damage stored so far and the target.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Mark {
    points: i32,
    stored: f64,
    target: Option<Side>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn bind<A: Agent>(
    fight: &mut Fight<A>,
    spell: SpellId,
    aura: &str,
    stored_per_point: f64,
    cap_base: f64,
    cap_per_point: f64,
    attack_power_per_point: f64,
    coup_de_grace: f64,
    doubled_class_spells: &[String],
) -> Result<DeathMark, String> {
    let aura = fight.player_aura(aura)?;
    let names: Vec<&str> = doubled_class_spells.iter().map(String::as_str).collect();
    let mut doubled = vec![false; fight.spells.len()];
    for id in fight.spells_with_class(&names) {
        doubled[id] = true;
    }
    Ok(DeathMark {
        aura,
        spell,
        stored_per_point,
        cap_base,
        cap_per_point,
        attack_power_per_point,
        coup_de_grace,
        doubled,
    })
}

impl DeathMark {
    /// The cast's effect; Stealth has already broken. The finisher spends the points and rolls
    /// its talents, then the mark starts empty (a recast restarts it).
    pub(crate) fn apply<A: Agent>(
        &self,
        fight: &mut Fight<A>,
        spell: SpellId,
        target: Side,
        finisher: &Finisher,
    ) -> Mark {
        let points = fight.energy_bar().combo_points;
        finisher.apply(fight, spell);
        fight.deactivate_aura(self.aura);
        fight.activate_aura(self.aura);
        Mark {
            points,
            stored: 0.0,
            target: Some(target),
        }
    }

    /// A hit or tick of the rogue's while the mark is up: a share of what it dealt is stored.
    pub(crate) fn store(&self, mark: &mut Mark, spell: SpellId, result: &SpellResult) {
        if spell == self.spell || mark.target != Some(result.target) || !result.landed() {
            return;
        }
        let twice = if self.doubled.get(spell).copied().unwrap_or(false) {
            2.0
        } else {
            1.0
        };
        let rate = self.stored_per_point * f64::from(mark.points) * self.coup_de_grace * twice;
        mark.stored += result.damage * rate;
    }

    /// The mark ending by its duration: the stored damage, up to the cap, is dealt.
    pub(crate) fn release<A: Agent>(&self, fight: &mut Fight<A>, mark: Mark) {
        let Some(target) = mark.target else {
            return;
        };
        if !fight.fading_by_duration(self.aura) {
            return;
        }
        let points = f64::from(mark.points);
        let power = fight.melee_attack_power();
        let crit = fight.physical_crit_chance(self.spell);
        let cap = (self.cap_base
            + points * (self.cap_per_point + self.attack_power_per_point * power))
            * (1.0 + crit)
            * self.coup_de_grace;
        let damage = mark.stored.min(cap);
        if damage <= 0.0 {
            return;
        }
        let mut result = SpellResult {
            target,
            attacker: Side::Player,
            outcome: OUTCOME_HIT,
            damage,
            threat: 0.0,
            armor_multiplier: 1.0,
        };
        result.threat = fight.threat_of(self.spell, &result);
        fight.deal_damage(self.spell, result, false);
    }
}
