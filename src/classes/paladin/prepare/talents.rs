//! The Paladin's talents: Go `talents_holy.go`, `talents_protection.go` and
//! `talents_retribution.go`. The abilities a talent point buys register in
//! `talent_spells.rs`.

mod holy;
mod protection;
mod retribution;

use std::rc::Rc;

use crate::prepare::sim::{AuraId, Duration, Sim, UnitId, MILLISECOND};
use crate::prepare::spell::ProcMask;
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};

/// `time.Duration(value) * time.Millisecond` for a client value in milliseconds.
pub(super) fn millis(value: f64) -> Duration {
    (value as i64).wrapping_mul(MILLISECOND)
}

/// Go `applyWeaponSpecialization`'s mod: a damage bonus on the given schools that follows the
/// weapon in the main hand, active when the weapon is of the hand type, item swaps included.
/// `proc_mask` narrows the spells it applies to; the default, no bits, is every spell.
pub(super) fn apply_weapon_specialization(
    sim: &mut Sim,
    unit: UnitId,
    bonus: f64,
    main_hand_matches: bool,
    hand_type: &str,
    schools: u8,
    proc_mask: ProcMask,
) {
    let weapon_mod = sim.add_dynamic_mod(
        unit,
        SpellModConfig {
            school: schools,
            proc_mask,
            kind: SpellModType::DamageDonePct,
            float_value: bonus,
            ..SpellModConfig::default()
        },
    );
    follow_main_hand(sim, unit, weapon_mod, main_hand_matches, hand_type);
}

/// Go `followMainHand`: keeps a weapon specialization's mod on while the main hand is of its type.
pub(super) fn follow_main_hand(
    sim: &mut Sim,
    unit: UnitId,
    weapon_mod: crate::prepare::spell_mod::ModId,
    main_hand_matches: bool,
    hand_type: &str,
) {
    if main_hand_matches {
        sim.activate_spell_mod(weapon_mod);
    }
    sim.register_item_swap_callback(
        unit,
        &[
            crate::prepare::items::slot::MAIN_HAND,
            crate::prepare::items::slot::OFF_HAND,
        ],
        crate::prepare::item_swap::SwapCallback::HandMod {
            id: weapon_mod,
            hand_type: hand_type.to_string(),
        },
    );
}

/// Go `Unit.NewPassiveMovementSpeedAura`: a permanent aura whose movement speed bonus holds
/// while no stronger passive speed effect shares the category.
pub(super) fn new_passive_movement_speed_aura(
    sim: &mut Sim,
    unit: UnitId,
    label: &str,
    action_id: crate::contracts::prepared_v2::ActionId,
    multiplier: f64,
) -> AuraId {
    let aura = sim.get_or_register_aura(
        unit,
        crate::prepare::sim::AuraConfig {
            label: label.to_string(),
            action_id: Some(action_id),
            ..Default::default()
        },
    );
    sim.make_permanent(aura);
    sim.new_exclusive_effect(
        aura,
        "PassiveMovementSpeed",
        true,
        multiplier,
        Some(Rc::new(move |sim: &mut Sim, effect| {
            let unit = sim.aura(sim.effects[effect.0].aura).unit;
            sim.multiply_movement_speed(unit, 1.0 + multiplier);
        })),
        Some(Rc::new(move |sim: &mut Sim, effect| {
            let unit = sim.aura(sim.effects[effect.0].aura).unit;
            sim.multiply_movement_speed(unit, 1.0 / (1.0 + multiplier));
        })),
    );
    aura
}
