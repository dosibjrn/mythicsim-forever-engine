//! Ferocious Bite (31018), from Go sim/druid/ferocious_bite.go: a finisher that rolls the
//! rank's damage effect and adds damage and attack power per combo point and damage per point
//! of the energy left after its cost. A landed bite spends that energy and the combo points;
//! one that does not land refunds its share of the cost. With Predatory Discipline (hotfix push
//! 113531, Forever's rules) a landed bite keeps that energy and still deals its damage.

use crate::core::fight::{melee::PhysicalOutcome, Fight, Side, SpellId};

use super::super::agent::DruidAgent;

#[derive(Clone, Copy, Debug)]
pub(crate) struct FerociousBite {
    pub(crate) damage_per_energy: f64,
    pub(crate) damage_per_combo_point: f64,
    pub(crate) attack_power_per_combo_point: f64,
    /// Predatory Discipline: a landed bite does not spend the energy left after its cost.
    pub(crate) keeps_energy: bool,
}

impl FerociousBite {
    /// `ExtraCastCondition`: at least one combo point.
    pub(crate) fn can_cast(&self, fight: &Fight<DruidAgent>) -> bool {
        fight.energy_bar().combo_points > 0
    }

    pub(crate) fn apply(&self, fight: &mut Fight<DruidAgent>, spell: SpellId, target: Side) {
        let combo_points = f64::from(fight.energy_bar().combo_points);
        let excess_energy = fight.energy_bar().current;
        let roll = fight.roll_damage_effect(spell);
        let attack_power = fight.melee_attack_power();
        // Go's arm64 build fuses each multiply into its add.
        let bite = (self.attack_power_per_combo_point * combo_points).mul_add(
            attack_power,
            self.damage_per_combo_point.mul_add(combo_points, roll),
        );
        let base = self.damage_per_energy.mul_add(excess_energy, bite);
        let result = fight.calc_physical_damage(
            spell,
            target,
            base,
            PhysicalOutcome::MeleeWeaponSpecialHitAndCrit { count: true },
        );
        fight.deal_damage(spell, result, false);
        if result.landed() {
            let (energy, combo) = fight.spells[spell].energy_metrics.expect("energy cost");
            if !self.keeps_energy {
                fight.spend_energy(excess_energy, energy);
            }
            fight.spend_combo_points(combo);
        } else {
            fight.issue_refund(spell);
        }
    }
}
