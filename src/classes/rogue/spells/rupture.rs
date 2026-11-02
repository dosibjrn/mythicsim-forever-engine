//! Rupture, from Go sim/rogue/rupture.go: a finisher whose metrics split by the combo points
//! spent. It rolls the special hit table without a crit; a landed hit applies a bleed lasting
//! a tick more a point, which snapshots its tick and a share of attack power a point, then the
//! finisher applies. Each tick reads the attack power share again, ignores armor and rolls
//! the tick outcome the client row states. Hemorrhage raises each tick it is up for, as damage
//! the target takes from the rogue.

use crate::core::fight::{
    melee::PhysicalOutcome, Agent, AuraRef, DotId, Fight, Outcome, Side, SpellId,
};

use super::finisher::Finisher;

#[derive(Clone, Debug)]
pub(crate) struct Rupture {
    pub(crate) tick_damage: f64,
    pub(crate) damage_per_combo_point: f64,
    pub(crate) base_tick_count: i32,
    pub(crate) attack_power_shares: Vec<f64>,
    pub(crate) tick_outcome: Outcome,
    pub(crate) hemorrhage: Option<AuraRef>,
    pub(crate) hemorrhage_multiplier: f64,
}

/// Go `Dot.SnapshotPhysical` and `SnapshotAttackPowerShare`: what the bleed stored.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Snapshot {
    base: f64,
    share: f64,
    attack_power: f64,
}

impl Rupture {
    /// Rupture's effect; Stealth has already broken. Returns the bleed's snapshot when it
    /// landed.
    pub(crate) fn apply<A: Agent>(
        &self,
        fight: &mut Fight<A>,
        spell: SpellId,
        target: Side,
        finisher: &Finisher,
    ) -> Option<Snapshot> {
        let result = fight.calc_physical_outcome(
            spell,
            target,
            PhysicalOutcome::MeleeSpecialHit { count: true },
        );
        let mut snapshot = None;
        if result.landed() {
            let points = fight.energy_bar().combo_points;
            let dot = fight.spells[spell].dot.expect("Rupture has a dot");
            let dot = fight.dot_on(dot, target);
            fight.dots[dot].base_tick_count = self.base_tick_count + points;
            fight.apply_dot(dot);
            snapshot = Some(self.snapshot(fight, points));
            finisher.apply(fight, spell);
        } else {
            fight.issue_refund(spell);
        }
        fight.deal_damage(spell, result, false);
        snapshot
    }

    /// The target debuff's `AttachDDBC` handler while Hemorrhage is up on the target the tick
    /// lands on: it counts on every tick that lands, not only on a Rupture cast under it.
    pub(crate) fn caster_multiplier<A: Agent>(
        &self,
        fight: &Fight<A>,
        spell: SpellId,
        target: Side,
    ) -> Option<f64> {
        let debuff = self.hemorrhage?;
        fight.aura(fight.aura_on(debuff, target)).active.then(|| {
            if fight.spells[spell].class_spell.as_deref() == Some("rupture") {
                self.hemorrhage_multiplier
            } else {
                1.0
            }
        })
    }

    /// The dot's `OnSnapshot`: the flat damage and the share of attack power a point.
    /// Hemorrhage is no part of it.
    fn snapshot<A: Agent>(&self, fight: &Fight<A>, points: i32) -> Snapshot {
        let share = self.attack_power_shares[points as usize];
        // Go's arm64 build fuses each multiply into its add.
        let flat = self
            .damage_per_combo_point
            .mul_add(f64::from(points), self.tick_damage);
        let attack_power = fight.melee_attack_power();
        Snapshot {
            base: share.mul_add(attack_power, flat),
            share,
            attack_power: share * attack_power,
        }
    }

    /// The dot's `OnTick`.
    pub(crate) fn tick<A: Agent>(&self, fight: &mut Fight<A>, dot: DotId, snapshot: Snapshot) {
        bleed_tick(fight, dot, snapshot, self.tick_outcome);
    }
}

/// Go `SnapshotPhysical` of `flat` plus a share of attack power, and
/// `SnapshotAttackPowerShare` of that share, as Garrote takes them.
pub(crate) fn snapshot_share<A: Agent>(fight: &Fight<A>, flat: f64, share: f64) -> Snapshot {
    let attack_power = fight.melee_attack_power();
    Snapshot {
        base: attack_power.mul_add(share, flat),
        share,
        attack_power: share * attack_power,
    }
}

/// A bleed's tick: Go `CalcAndDealPeriodicSnapshotDamage` on the current share of attack power
/// and attacker multiplier.
pub(crate) fn bleed_tick<A: Agent>(
    fight: &mut Fight<A>,
    dot: DotId,
    snapshot: Snapshot,
    outcome: Outcome,
) {
    let (spell, side) = (fight.dots[dot].spell, fight.dots[dot].side);
    let mut base = snapshot.base;
    if snapshot.share != 0.0 {
        // Go currentTickInputs: the share less the snapshot's, fused.
        base += snapshot
            .share
            .mul_add(fight.melee_attack_power(), -snapshot.attack_power);
    }
    let attacker =
        fight.attacker_multiplier(spell, true) * fight.dots[dot].periodic_damage_multiplier;
    let result = fight.calc_tick_damage(spell, side, base, attacker, outcome);
    fight.deal_damage(spell, result, true);
}

/// spelldata `TickOutcome` for a dot whose hit was rolled when it was applied.
pub(crate) fn tick_outcome(can_crit: bool, magic: bool) -> Result<Outcome, String> {
    Ok(match (can_crit, magic) {
        (true, true) => Outcome::TickMagicHitAndCrit,
        (true, false) => Outcome::TickPhysicalCrit,
        (false, true) => return Err("bleed ticks that roll a magic hit are unsupported".into()),
        (false, false) => Outcome::Tick,
    })
}
