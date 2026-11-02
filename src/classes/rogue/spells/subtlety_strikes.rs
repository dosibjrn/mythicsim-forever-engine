//! Ghostly Strike, Hemorrhage and Garrote, from Go sim/rogue/talents_subtlety.go and
//! garrote.go. Ghostly Strike deals main hand weapon damage and buffs dodge, which has no effect
//! in scope; Hemorrhage deals normalized main hand damage and puts its debuff on the target,
//! which raises Rupture; Garrote opens from Stealth with a bleed of its tick and a share of
//! attack power read again at each tick. Each gives a combo point when it lands and refunds
//! most of its energy when it misses.

use crate::core::fight::{melee::PhysicalOutcome, Agent, AuraRef, Fight, Outcome, Side, SpellId};

use super::{
    finisher::combo_point_metrics,
    rupture::{self, Snapshot},
    stealth::Stealth,
};

/// Ghostly Strike's effect; Stealth has already broken.
pub(crate) fn ghostly_strike<A: Agent>(
    fight: &mut Fight<A>,
    spell: SpellId,
    target: Side,
    dodge: Option<AuraRef>,
) {
    let attack_power = fight.melee_attack_power();
    let damage = fight.mh_weapon_damage(attack_power);
    let result = fight.calc_physical_damage(
        spell,
        target,
        damage,
        PhysicalOutcome::MeleeWeaponSpecialHitAndCrit { count: true },
    );
    fight.deal_damage(spell, result, false);
    if let Some(aura) = dodge {
        fight.activate_aura(aura);
    }
    land_or_refund(fight, spell, result.landed());
}

/// Hemorrhage's effect; Stealth has already broken.
pub(crate) fn hemorrhage<A: Agent>(
    fight: &mut Fight<A>,
    spell: SpellId,
    target: Side,
    debuff: AuraRef,
) {
    let attack_power = fight.melee_attack_power();
    let damage = fight.mh_normalized_weapon_damage(attack_power);
    let result = fight.calc_physical_damage(
        spell,
        target,
        damage,
        PhysicalOutcome::MeleeWeaponSpecialHitAndCrit { count: true },
    );
    fight.deal_damage(spell, result, false);
    if result.landed() {
        let metrics = combo_point_metrics(fight, spell);
        fight.add_combo_points(1, metrics);
        let debuff = fight.aura_on(debuff, target);
        fight.activate_aura(debuff);
    } else {
        fight.issue_refund(spell);
    }
}

fn land_or_refund<A: Agent>(fight: &mut Fight<A>, spell: SpellId, landed: bool) {
    if landed {
        let metrics = combo_point_metrics(fight, spell);
        fight.add_combo_points(1, metrics);
    } else {
        fight.issue_refund(spell);
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Garrote {
    pub(crate) tick_damage: f64,
    pub(crate) attack_power_share: f64,
    pub(crate) tick_outcome: Outcome,
    pub(crate) dirty_deeds: bool,
}

impl Garrote {
    /// Go `ExtraCastCondition`: from Stealth, behind the target unless Dirty Deeds.
    pub(crate) fn can_cast<A: Agent>(&self, fight: &Fight<A>, stealth: Option<Stealth>) -> bool {
        if !stealth.is_some_and(|stealth| stealth.active(fight)) {
            return false;
        }
        self.dirty_deeds || !fight.config.melee.in_front_of_target
    }

    /// Garrote's effect; Stealth has already broken. Returns the bleed's snapshot when it
    /// landed.
    pub(crate) fn apply<A: Agent>(
        &self,
        fight: &mut Fight<A>,
        spell: SpellId,
        target: Side,
    ) -> Option<Snapshot> {
        let result = fight.calc_physical_outcome(
            spell,
            target,
            PhysicalOutcome::MeleeSpecialNoBlockDodgeParryNoCrit { count: true },
        );
        let mut snapshot = None;
        if result.landed() {
            let metrics = combo_point_metrics(fight, spell);
            fight.add_combo_points(1, metrics);
            let dot = fight.spells[spell].dot.expect("Garrote has a dot");
            let dot = fight.dot_on(dot, target);
            fight.apply_dot(dot);
            snapshot = Some(rupture::snapshot_share(
                fight,
                self.tick_damage,
                self.attack_power_share,
            ));
        } else {
            fight.issue_refund(spell);
        }
        fight.deal_damage(spell, result, false);
        snapshot
    }
}
