//! The client's spell rows as the sim reads them, mirroring Go's `sim/core/spelldata`: one
//! [`Spell`] per spell id with its effects and power costs, and accessors that turn the client's
//! units into the sim's.
//!
//! The rows themselves are [`crate::data::spells`]; this module is Go's accessor half. Values
//! stay in the units the client states them in (a percentage is the integer 16, rage is on a
//! 0-1000 bar, times are milliseconds) and the conversion happens in the accessors.
//!
//! Go's `Nil` and `NilEffect` are the zero row and zero effect every accessor answers on, so a
//! caller can chain through a spell the store does not carry. They are shared statics here
//! too: [`nil`] and [`nil_effect`], recognised by [`Spell::is_nil`] and [`Effect::is_nil`]
//! (Go compares the pointers). [`find`] answers [`nil`] where Go's `Find` answers `Nil`.
//!
//! Go panics where a row a build depends on is missing or ambiguous (`MustFind`, `Effect`,
//! ladder ranks); these functions panic with Go's messages.

pub(crate) mod area;
pub(crate) mod attributes;
pub(crate) mod book_ranks;
pub(crate) mod debuff;
pub(crate) mod effect;
pub(crate) mod ladder;
pub(crate) mod speed;
pub(crate) mod spell;
pub(crate) mod store;

use std::sync::OnceLock;

pub(crate) use crate::data::spells::{ClassFlags, Effect, Power, Spell};

#[allow(unused_imports)]
pub(crate) use ladder::{Ladder, LadderEffect};
#[allow(unused_imports)]
pub(crate) use store::{all, by_name, find, must_find};

use super::sim::{Duration, MILLISECOND};

/// Go `core.DurationFromMillis` for an `int32` column: `time.Duration(ms) * time.Millisecond`.
pub(crate) fn duration_from_millis(ms: i32) -> Duration {
    i64::from(ms).wrapping_mul(MILLISECOND)
}

/// Go `core.DurationFromMillis` for a `float64`: a float is truncated to whole milliseconds,
/// which is how the client's millisecond columns read.
pub(crate) fn duration_from_millis_f64(ms: f64) -> Duration {
    (ms as i64).wrapping_mul(MILLISECOND)
}

/// Go `ProcChanceSource`: where the proc's chance is stated, since the `ProcChance` column alone
/// cannot tell a roll from a condition. `Spell::proc_chance_source` holds one of these.
pub(crate) mod proc_chance_source {
    /// `ProcChance` is the roll: the tooltip renders it as "$h%".
    pub const COLUMN: u8 = 0;
    /// The value of effect `ProcChanceEffect` is the roll: the tooltip renders it as "$mN%".
    pub const EFFECT_N: u8 = 1;
    /// The column reads 100 or 101 and the tooltip's trigger clause states no chance at all, so
    /// the aura fires whenever its condition is met.
    pub const ALWAYS: u8 = 2;
    /// The client states no chance anywhere, or states 100 or 101 beside a trigger clause saying
    /// the effect only sometimes happens, so the rate has to come from an override into `RPPM`.
    pub const PPM: u8 = 3;
}

/// Go `Nil`: what an unknown spell answers with. Every accessor reads zero off it.
pub(crate) fn nil() -> &'static Spell {
    static NIL: OnceLock<Spell> = OnceLock::new();
    NIL.get_or_init(Spell::default)
}

/// Go `NilEffect`: what an out-of-range effect answers with.
pub(crate) fn nil_effect() -> &'static Effect {
    static NIL: OnceLock<Effect> = OnceLock::new();
    NIL.get_or_init(Effect::default)
}

impl Spell {
    /// Whether this is [`nil`] itself, Go's `s == Nil`. A real row, even one of all zeroes, is
    /// not.
    pub(crate) fn is_nil(&self) -> bool {
        std::ptr::eq(self, nil())
    }
}

impl Effect {
    /// Whether this is [`nil_effect`] itself, Go's `e == NilEffect`.
    pub(crate) fn is_nil(&self) -> bool {
        std::ptr::eq(self, nil_effect())
    }
}

/// Go `ClassFlags.IsZero` and `ClassFlags.Matches` (sim/core/class_flags.go), which the
/// accessors here read through.
impl ClassFlags {
    /// Whether the spell carries no class options at all.
    pub(crate) fn is_zero(&self) -> bool {
        self.family == 0 && self.mask == [0; 4]
    }

    /// Whether the two sets name at least one spell in common. Families are separate
    /// namespaces, so the same bit means a different spell in each and a cross-family overlap
    /// is never a match.
    pub(crate) fn matches(&self, other: &ClassFlags) -> bool {
        if self.family != other.family {
            return false;
        }
        self.mask.iter().zip(&other.mask).any(|(a, b)| a & b != 0)
    }
}
