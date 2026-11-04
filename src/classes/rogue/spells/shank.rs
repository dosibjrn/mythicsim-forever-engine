//! Shank, from the development notes of 2 November 2026 (Forever's rules): a baseline strike
//! with the off hand weapon for half its weapon damage, which generates Energy. It cannot miss,
//! so it rolls like each hand of Mutilate: blocked or critical, never missed, dodged or parried.
//! It gives no combo point of its own. Boot Knife gives it a chance to add two (the spell of
//! 1329285), and Venom has it restart the poison aura (the agent does that, as the aura is the
//! class's).

use crate::contracts::prepared_v2::ActionId;
use crate::core::fight::{melee::PhysicalOutcome, Agent, Fight, ResourceKind, Side, SpellId};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Shank {
    pub(crate) energy_gain: f64,
    /// Boot Knife: the chance a landed Shank adds `boot_knife_combo_points`, and the metrics of
    /// the combo points it adds; none without the talent.
    pub(crate) boot_knife_chance: f64,
    pub(crate) boot_knife_combo_points: i32,
    pub(crate) boot_knife_metrics: Option<usize>,
}

/// Binds the exported effect: Boot Knife's combo points have metrics of their own.
pub(crate) fn bind<A: Agent>(
    fight: &mut Fight<A>,
    energy_gain: f64,
    boot_knife_chance: f64,
    boot_knife_combo_points: i32,
    boot_knife_action: Option<&ActionId>,
) -> Shank {
    let boot_knife_metrics = boot_knife_action
        .filter(|_| boot_knife_chance > 0.0)
        .map(|action| fight.new_resource_metrics(action.clone(), ResourceKind::ComboPoints));
    Shank {
        energy_gain,
        boot_knife_chance,
        boot_knife_combo_points,
        boot_knife_metrics,
    }
}

impl Shank {
    /// Shank's effect; Stealth has already broken. The spell's damage multiplier holds the
    /// weapon share, and the off hand's damage penalty is in the weapon roll.
    pub(crate) fn apply<A: Agent>(&self, fight: &mut Fight<A>, spell: SpellId, target: Side) {
        let attack_power = fight.melee_attack_power();
        let damage = fight.oh_weapon_damage(attack_power);
        let result = fight.calc_physical_damage(
            spell,
            target,
            damage,
            PhysicalOutcome::MeleeSpecialBlockAndCrit { count: true },
        );
        fight.deal_damage(spell, result, false);
        let (energy_metrics, _) = fight.spells[spell]
            .energy_metrics
            .expect("Shank costs energy");
        fight.add_energy(self.energy_gain, energy_metrics);
        if let Some(metrics) = self.boot_knife_metrics {
            if result.landed() && fight.proc(self.boot_knife_chance, "Boot Knife") {
                fight.add_combo_points(self.boot_knife_combo_points, metrics);
            }
        }
    }
}
