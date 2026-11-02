//! Go sim/druid/druid.go's class mask bits and the groups built from them.
//!
//! Go counts the bits from `1 << iota` on the second line of its constant block, so the first
//! spell, Entangling Roots, is bit 1.

pub(crate) const ENTANGLING_ROOTS: i64 = 1 << 1;
pub(crate) const CLAW: i64 = 1 << 2;
pub(crate) const DEMORALIZING_ROAR: i64 = 1 << 3;
pub(crate) const FAERIE_FIRE: i64 = 1 << 4;
pub(crate) const FAERIE_FIRE_FERAL: i64 = 1 << 5;
pub(crate) const HURRICANE: i64 = 1 << 6;
pub(crate) const FEROCIOUS_BITE: i64 = 1 << 7;
pub(crate) const FRENZIED_REGENERATION: i64 = 1 << 8;
pub(crate) const INNERVATE: i64 = 1 << 9;
pub(crate) const INSECT_SWARM: i64 = 1 << 10;
pub(crate) const LACERATE: i64 = 1 << 11;
pub(crate) const PRIMAL_BITE: i64 = 1 << 12;
pub(crate) const MAUL: i64 = 1 << 13;
pub(crate) const MOONFIRE_INITIAL: i64 = 1 << 14;
pub(crate) const MOONFIRE_DOT: i64 = 1 << 15;
pub(crate) const RAKE: i64 = 1 << 16;
pub(crate) const RAVAGE: i64 = 1 << 17;
pub(crate) const RIP: i64 = 1 << 18;
pub(crate) const SHRED: i64 = 1 << 19;
pub(crate) const STARFIRE: i64 = 1 << 20;
pub(crate) const SWIPE: i64 = 1 << 21;
pub(crate) const THORNS: i64 = 1 << 22;
pub(crate) const WRATH: i64 = 1 << 23;
pub(crate) const ENRAGE: i64 = 1 << 24;
pub(crate) const SHIFTING_POWER: i64 = 1 << 25;
pub(crate) const CAT_FORM: i64 = 1 << 26;
pub(crate) const BEAR_FORM: i64 = 1 << 27;
pub(crate) const MOONKIN_FORM: i64 = 1 << 28;
pub(crate) const HEALING_TOUCH: i64 = 1 << 29;
pub(crate) const REGROWTH: i64 = 1 << 30;
pub(crate) const LIFEBLOOM: i64 = 1 << 31;
pub(crate) const REJUVENATION: i64 = 1 << 32;
pub(crate) const TRANQUILITY: i64 = 1 << 33;
pub(crate) const MARK_OF_THE_WILD: i64 = 1 << 34;
pub(crate) const SWIFTMEND: i64 = 1 << 35;
pub(crate) const CENARION_WARD: i64 = 1 << 36;
pub(crate) const REVIVE: i64 = 1 << 37;
const LAST: i64 = 1 << 38;
/// MythicSim's healer sims (Rust only): Wild Growth, which Go never registered. Past Go's bits,
/// so no mask of Go's names it.
pub(crate) const WILD_GROWTH: i64 = 1 << 40;
/// Bloom's heal (hotfix push 113531, Forever's rules), which Go never registered either.
pub(crate) const BLOOM: i64 = 1 << 41;
/// Go `DruidSpellsAll`.
pub(crate) const ALL: i64 = (LAST << 1) - 1;

pub(crate) const MOONFIRE: i64 = MOONFIRE_INITIAL | MOONFIRE_DOT;
pub(crate) const DOT: i64 = MOONFIRE_DOT | INSECT_SWARM;
pub(crate) const BUILDER: i64 = CLAW | PRIMAL_BITE | SHRED | RAKE | RAVAGE;
pub(crate) const ARCANE_SPELLS: i64 = MOONFIRE | MOONFIRE_DOT | STARFIRE;
pub(crate) const NATURE_SPELLS: i64 = WRATH | HURRICANE | INSECT_SWARM;
pub(crate) const DAMAGING_SPELLS: i64 = ARCANE_SPELLS | NATURE_SPELLS;

/// omen_of_clarity.go `clearcastingSpells`.
pub(crate) const CLEARCASTING_SPELLS: i64 = CLAW
    | ENTANGLING_ROOTS
    | DEMORALIZING_ROAR
    | HURRICANE
    | FEROCIOUS_BITE
    | INSECT_SWARM
    | LACERATE
    | PRIMAL_BITE
    | MAUL
    | MOONFIRE
    | RAKE
    | RAVAGE
    | RIP
    | SHRED
    | STARFIRE
    | SWIPE
    | THORNS
    | HEALING_TOUCH
    | REGROWTH
    | LIFEBLOOM
    | REJUVENATION
    | TRANQUILITY
    | SWIFTMEND;
