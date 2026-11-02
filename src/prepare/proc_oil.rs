//! Go sim/common/forever/frost_oil.go: Frost Oil as a weapon imbue (the fork's patch 126), and
//! Shadow Oil beside it. Shadow Oil is a MythicSim Rust-only addition beyond the Go reference
//! pin (client 1.60.1.70707, temporary enchantment 25): the exact twin of Frost Oil, so both are
//! one table of proc oils registered the same way for each hand.

use super::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use super::env::Environment;
use super::shared_items::spell_data_proc_damage_spell_config;
use super::spell::{ProcMask, SpellFlag};
use super::spelldata::must_find;

/// A temporary enchantment on a melee weapon whose client row casts a bolt on a landed hit of the
/// oiled weapon, at the flat chance the bolt's row states (not procs per minute).
#[derive(Debug)]
pub(crate) struct ProcOil {
    /// The oil's name; its proc auras are `"{name} (main hand)"` and `"{name} (off hand)"`.
    pub name: &'static str,
    /// The oil's use spell and item: the imbue IDs a request may name it by.
    pub use_spell: i32,
    pub item: i32,
    /// The spell the temporary enchantment casts, whose `ProcChance` is the enchantment's chance.
    pub bolt: i32,
}

/// Every proc oil, in the order its effects are exported.
pub(crate) const PROC_OILS: [ProcOil; 2] = [
    // Go frost_oil.go (patch 126): the Frostbolt temporary enchantment 26 casts, at the 10% the
    // client states for it.
    ProcOil {
        name: "Frost Oil",
        use_spell: 3595,
        item: 3829,
        bolt: 350025,
    },
    // MythicSim Rust-only, beyond the Go reference pin: the Shadow Bolt temporary enchantment 25
    // casts (item 3824, use spell 3594, client 70707), at the 15% the client states for it.
    ProcOil {
        name: "Shadow Oil",
        use_spell: 3594,
        item: 3824,
        bolt: 350026,
    },
];

impl ProcOil {
    /// The name of the proc aura for `hand`, `ProcMask::MELEE_MH` or `ProcMask::MELEE_OH`.
    pub(crate) fn aura_name(&self, hand: ProcMask) -> String {
        let side = if hand == ProcMask::MELEE_OH {
            "off hand"
        } else {
            "main hand"
        };
        format!("{} ({side})", self.name)
    }
}

/// Go `core.StaticImbueProcs`: the proc oil an imbue ID names, a temporary enchantment whose
/// effect is a combat proc.
pub(crate) fn static_imbue_proc(imbue_id: i32) -> Option<&'static ProcOil> {
    PROC_OILS
        .iter()
        .find(|oil| imbue_id == oil.use_spell || imbue_id == oil.item)
}

/// Go `registerFrostOil`: a flat chance on each landed hit of the oiled weapon to cast the oil's
/// bolt at the target. `hand` is `ProcMask::MELEE_MH` or `ProcMask::MELEE_OH`.
pub(crate) fn register_proc_oil(env: &mut Environment, oil: &ProcOil, hand: ProcMask) {
    let unit = env.player;
    let row = must_find(oil.bolt);
    let config = spell_data_proc_damage_spell_config(env, row);
    let bolt = env.sim.get_or_register_spell(unit, config);
    env.sim.spell_mut(bolt).flags |=
        SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::PASSIVE_SPELL | SpellFlag::PROC;
    let chance = f64::from(row.proc_chance) / 100.0;

    env.sim.make_proc_trigger_aura(
        unit,
        &ProcTrigger {
            name: oil.aura_name(hand),
            callback: CallbackMask::ON_SPELL_HIT_DEALT,
            proc_mask: hand,
            is_weapon_proc: true,
            outcome: HitOutcome::LANDED,
            proc_chance: chance,
            trigger_immediately: true,
            ..ProcTrigger::default()
        },
    );
}
