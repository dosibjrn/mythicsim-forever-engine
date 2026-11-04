//! Go `sim/rogue` class masks (`RogueSpell*`, rogue.go), the bits the exporter names.

/// Go's constant block starts `RogueSpellFlagNone int64 = 0` and then `1 << iota`, so the first
/// spell is bit 1, not bit 0.
pub(crate) const AMBUSH: i64 = 1 << 1;
pub(crate) const BACKSTAB: i64 = 1 << 2;
pub(crate) const EVISCERATE: i64 = 1 << 3;
pub(crate) const EXPOSE_ARMOR: i64 = 1 << 4;
pub(crate) const FEINT: i64 = 1 << 5;
pub(crate) const GARROTE: i64 = 1 << 6;
pub(crate) const GOUGE: i64 = 1 << 7;
pub(crate) const RUPTURE: i64 = 1 << 8;
pub(crate) const SINISTER_STRIKE: i64 = 1 << 9;
pub(crate) const SLICE_AND_DICE: i64 = 1 << 10;
pub(crate) const STEALTH: i64 = 1 << 11;
pub(crate) const VANISH: i64 = 1 << 12;
pub(crate) const HEMORRHAGE: i64 = 1 << 13;
pub(crate) const PREMEDITATION: i64 = 1 << 14;
pub(crate) const PREPARATION: i64 = 1 << 15;
pub(crate) const SHADOWSTEP: i64 = 1 << 16;
pub(crate) const ADRENALINE_RUSH: i64 = 1 << 17;
pub(crate) const BLADE_FLURRY: i64 = 1 << 18;
pub(crate) const COLD_BLOOD: i64 = 1 << 19;
pub(crate) const MUTILATE: i64 = 1 << 20;
pub(crate) const MUTILATE_HIT: i64 = 1 << 21;
pub(crate) const GHOSTLY_STRIKE: i64 = 1 << 22;
pub(crate) const INSTANT_POISON: i64 = 1 << 23;
pub(crate) const WOUND_POISON: i64 = 1 << 24;
pub(crate) const DEADLY_POISON: i64 = 1 << 25;
pub(crate) const VENOM: i64 = 1 << 26;
pub(crate) const RIPOSTE: i64 = 1 << 27;
pub(crate) const KIDNEY_SHOT: i64 = 1 << 28;

/// Shank, new in the notes of 2 November 2026 (Forever's rules only). Go's block ends at
/// Kidney Shot, so the bit is the one Go's `RogueSpellLast` would take.
pub(crate) const SHANK: i64 = 1 << 29;
/// Death Mark, new in the rework of push 113483 (Forever's rules only), the bit after Shank's.
pub(crate) const DEATH_MARK: i64 = 1 << 30;

const LAST: i64 = 1 << 30;
/// Go `RogueSpellsAll = RogueSpellLast<<1 - 1`.
pub(crate) const ALL: i64 = (LAST << 1) - 1;

pub(crate) const POISONS: i64 = WOUND_POISON | DEADLY_POISON | INSTANT_POISON;
pub(crate) const LETHALITY: i64 =
    SINISTER_STRIKE | GOUGE | BACKSTAB | GHOSTLY_STRIKE | MUTILATE | MUTILATE_HIT | HEMORRHAGE;
pub(crate) const FINISHER: i64 =
    EVISCERATE | SLICE_AND_DICE | RUPTURE | EXPOSE_ARMOR | VENOM | KIDNEY_SHOT | DEATH_MARK;
/// Every ability that costs Energy, the ones Poised's discount can fall on.
pub(crate) const ENERGY_ABILITIES: i64 = AMBUSH
    | BACKSTAB
    | EVISCERATE
    | DEATH_MARK
    | EXPOSE_ARMOR
    | GARROTE
    | GHOSTLY_STRIKE
    | HEMORRHAGE
    | KIDNEY_SHOT
    | MUTILATE
    | RIPOSTE
    | RUPTURE
    | SHANK
    | SINISTER_STRIKE
    | SLICE_AND_DICE
    | VENOM;
/// Quietus names only these three in its tooltip.
pub(crate) const QUIETUS: i64 = SINISTER_STRIKE | GHOSTLY_STRIKE | HEMORRHAGE;
/// Cold Blood's class mask (14177): Mutilate's two hits, not the parent cast.
pub(crate) const COLD_BLOODED: i64 =
    SINISTER_STRIKE | BACKSTAB | AMBUSH | EVISCERATE | MUTILATE_HIT;
