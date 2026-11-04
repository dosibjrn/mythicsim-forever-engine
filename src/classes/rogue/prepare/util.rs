//! Helpers the Rogue registrations share: the pieces of Go's `Character` and `Unit` the Rogue
//! code calls that preparation has no shared port of yet.

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::attack::Weapon;
use crate::prepare::items::slot;
use crate::prepare::sim::{AuraId, Duration, Sim, UnitId, UnitType};
use crate::prepare::spell::{Cast, CastConfig, CostOptions, LabeledAuraArrays, ProcMask};
use crate::prepare::spelldata::Spell as Row;

/// The hand a weapon check asks about, Go `core.Hand`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Hand {
    Main,
    Off,
}

/// The weapon share each Mutilate hit deals, as a fraction: 75%, or 90% under Forever's rules
/// since the notes of 2 November 2026. Every hit rank states the same share, so the first rank's
/// stands for all of them.
pub(super) fn mutilate_weapon_share(rules: crate::prepare::sim::Rules) -> f64 {
    crate::prepare::hotfix_113554::row(rules, 399960)
        .effect_n(2)
        .base_points
        / 100.0
}

/// An action naming a spell.
pub(super) fn spell_action(id: i32) -> ActionId {
    ActionId {
        spell_id: id,
        ..ActionId::default()
    }
}

/// An action naming a spell under a tag.
pub(super) fn tagged_action(id: i32, tag: i32) -> ActionId {
    ActionId {
        spell_id: id,
        tag,
        ..ActionId::default()
    }
}

/// Go `Character.WeaponFromMainHand`.
pub(super) fn weapon_from_main_hand(sim: &Sim, unit: UnitId) -> Weapon {
    match sim.mh_weapon(unit) {
        Some(item) => Weapon::from_item(item, sim.unit(unit).pseudo_stats.bonus_mh_dps),
        None => Weapon::unarmed(),
    }
}

/// Go `Character.WeaponFromOffHand`.
pub(super) fn weapon_from_off_hand(sim: &Sim, unit: UnitId) -> Weapon {
    match sim.oh_weapon(unit) {
        Some(item) => Weapon::from_item(item, sim.unit(unit).pseudo_stats.bonus_oh_dps),
        None => Weapon::default(),
    }
}

/// Go `Rogue.HasDagger`: `MainHand()` and `OffHand()` are the equipment's slots, never nil, so an
/// empty hand is a weapon of an unknown type.
pub(super) fn has_dagger(sim: &Sim, unit: UnitId, hand: Hand) -> bool {
    let equipment = &sim.character(unit).equipment;
    let slot = match hand {
        Hand::Main => slot::MAIN_HAND,
        Hand::Off => slot::OFF_HAND,
    };
    equipment[slot].weapon_type == "WeaponTypeDagger"
}

/// Go `Character.GetProcMaskForTypes`: the hands holding a weapon of one of the types.
pub(super) fn proc_mask_for_types(sim: &Sim, unit: UnitId, types: &[&str]) -> ProcMask {
    let equipment = &sim.character(unit).equipment;
    let holds = |slot: usize| {
        let item = &equipment[slot];
        !item.is_empty() && types.contains(&item.weapon_type.as_str())
    };
    let mut mask = ProcMask::UNKNOWN;
    if holds(slot::RANGED) {
        mask = mask | ProcMask::RANGED;
    }
    if holds(slot::MAIN_HAND) {
        mask = mask | ProcMask::MELEE_MH;
    }
    if holds(slot::OFF_HAND) {
        mask = mask | ProcMask::MELEE_OH;
    }
    mask
}

/// Go `unit.NewEnemyAuraArray`: an aura on each enemy, by unit index, in the order of
/// `Env.AllUnits`.
pub(super) fn new_enemy_aura_array(
    sim: &mut Sim,
    mut make_aura: impl FnMut(&mut Sim, UnitId) -> AuraId,
) -> Vec<Option<AuraId>> {
    let units = sim.all_units();
    let mut auras = vec![None; units.len()];
    for target in units {
        if sim.unit(target).unit_type == UnitType::Enemy {
            let index = sim.unit(target).unit_index as usize;
            auras[index] = Some(make_aura(sim, target));
        }
    }
    auras
}

/// Go `AuraArray.ToMap`: the array under the label of its first aura, or nothing for an array
/// without auras.
pub(super) fn aura_array_map(sim: &Sim, auras: &[Option<AuraId>]) -> LabeledAuraArrays {
    let mut map = LabeledAuraArrays::new();
    if let Some(first) = auras.iter().flatten().next() {
        map.insert(sim.aura(*first).label.clone(), auras.to_vec());
    }
    map
}

/// `core.EnergyCostOptions{Cost: int32(row.Cost()), Refund: refund}`.
pub(super) fn energy_cost(row: &Row, refund: bool) -> CostOptions {
    CostOptions {
        energy_cost: row.cost() as i32,
        energy_refund: if refund { row.miss_refund() } else { 0.0 },
        ..CostOptions::default()
    }
}

/// `max(row.Cooldown(), row.CategoryCooldown())`.
pub(super) fn longest_cooldown(row: &Row) -> Duration {
    row.cooldown().max(row.category_cooldown())
}

/// `Cast: core.CastConfig{DefaultCast: core.Cast{GCD: gcd}, IgnoreHaste: true}`.
pub(super) fn ignore_haste_cast(gcd: Duration) -> CastConfig {
    CastConfig {
        default_cast: Cast {
            gcd,
            ..Cast::default()
        },
        ignore_haste: true,
        ..CastConfig::default()
    }
}
