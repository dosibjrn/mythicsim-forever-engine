//! Go `sim/paladin/spell_masks.go`: the class mask bits that identify the Paladin's spells to
//! talents, proc triggers and spell mods. The first bit is `1 << 1`, since Go's `iota` counts
//! `SpellMaskNone` first.

pub const JUDGEMENT: i64 = 1 << 1;
pub const HOLY_STRIKE: i64 = 1 << 2;
pub const CONSECRATION: i64 = 1 << 3;
pub const EXORCISM: i64 = 1 << 4;
pub const HAMMER_OF_WRATH: i64 = 1 << 5;
pub const HOLY_WRATH: i64 = 1 << 6;
pub const HOLY_LIGHT: i64 = 1 << 7;
pub const FLASH_OF_LIGHT: i64 = 1 << 8;
pub const LAY_ON_HANDS: i64 = 1 << 9;
pub const RIGHTEOUS_FURY: i64 = 1 << 10;
pub const HAMMER_OF_THE_RIGHTEOUS: i64 = 1 << 11;

pub const DIVINE_FAVOR: i64 = 1 << 12;
pub const HOLY_SHOCK: i64 = 1 << 13;
pub const HOLY_SHOCK_HEAL: i64 = 1 << 14;
pub const HOLY_SHIELD: i64 = 1 << 15;
pub const HOLY_SHIELD_PROC: i64 = 1 << 16;
pub const SWIFT_JUDGEMENT: i64 = 1 << 17;
pub const TEMPLARS_BULWARK: i64 = 1 << 18;
pub const LIGHTS_VIGIL: i64 = 1 << 19;
pub const LIGHTS_VIGIL_STRIKE: i64 = 1 << 20;

pub const SEAL_OF_RIGHTEOUSNESS: i64 = 1 << 21;
pub const SEAL_OF_COMMAND: i64 = 1 << 22;
pub const SEAL_OF_LIGHT: i64 = 1 << 23;
pub const SEAL_OF_WISDOM: i64 = 1 << 24;
pub const SEAL_OF_JUSTICE: i64 = 1 << 25;
pub const SEAL_OF_THE_CRUSADER: i64 = 1 << 26;
pub const SEAL_OF_FURY: i64 = 1 << 27;

pub const SEAL_OF_RIGHTEOUSNESS_PROC: i64 = 1 << 28;
pub const SEAL_OF_COMMAND_PROC: i64 = 1 << 29;
pub const SEAL_OF_LIGHT_PROC: i64 = 1 << 30;
pub const SEAL_OF_WISDOM_PROC: i64 = 1 << 31;
pub const SEAL_OF_FURY_PROC: i64 = 1 << 32;

pub const JUDGEMENT_OF_RIGHTEOUSNESS: i64 = 1 << 33;
pub const JUDGEMENT_OF_COMMAND: i64 = 1 << 34;
pub const JUDGEMENT_OF_LIGHT: i64 = 1 << 35;
pub const JUDGEMENT_OF_WISDOM: i64 = 1 << 36;
pub const JUDGEMENT_OF_JUSTICE: i64 = 1 << 37;
pub const JUDGEMENT_OF_THE_CRUSADER: i64 = 1 << 38;
pub const JUDGEMENT_OF_FURY: i64 = 1 << 39;

pub const DEVOTION_AURA: i64 = 1 << 40;
pub const RETRIBUTION_AURA: i64 = 1 << 41;
pub const CONCENTRATION_AURA: i64 = 1 << 42;
pub const FIRE_RESISTANCE_AURA: i64 = 1 << 43;
pub const FROST_RESISTANCE_AURA: i64 = 1 << 44;
pub const SHADOW_RESISTANCE_AURA: i64 = 1 << 45;

/// Swift Judgement under Forever's rules since push 113554: a second Judgement, which carries
/// Swift Judgement's bit as well for the mods that name it (Sentinel's Libram). Go has no such
/// bit; it is Rust's, past the reference's.
pub const SECOND_JUDGEMENT: i64 = 1 << 46;

/// Oath of Light (1328548, push 113554, Forever's rules). Go has no such bit; it is Rust's, past
/// the reference's.
pub const OATH_OF_LIGHT: i64 = 1 << 47;

/// Light's Vigil on an ally (push 113554, Forever's rules): the cast, and the party heal the next
/// Holy Shock heal triggers. Rust's bits, past the reference's.
pub const LIGHTS_VIGIL_ALLY: i64 = 1 << 48;
pub const LIGHTS_VIGIL_HEAL: i64 = 1 << 49;

pub const ALL_SEALS: i64 = SEAL_OF_RIGHTEOUSNESS
    | SEAL_OF_COMMAND
    | SEAL_OF_LIGHT
    | SEAL_OF_WISDOM
    | SEAL_OF_JUSTICE
    | SEAL_OF_THE_CRUSADER
    | SEAL_OF_FURY;

/// The heals Healing Light, Illumination and Divine Favor name.
pub const HEALING_SPELLS: i64 = HOLY_LIGHT | FLASH_OF_LIGHT | HOLY_SHOCK_HEAL;

/// Benediction's class mask (20101) over the spells the sim registers.
pub const BENEDICTION: i64 = ALL_SEALS
    | RETRIBUTION_AURA
    | JUDGEMENT
    | HOLY_STRIKE
    | CONSECRATION
    | EXORCISM
    | RIGHTEOUS_FURY
    | HOLY_SHOCK
    | HOLY_SHOCK_HEAL
    | HOLY_SHIELD
    | TEMPLARS_BULWARK;

/// Divine Precision's class mask (1310904), as far as the sim casts it.
pub const DIVINE_PRECISION: i64 = CONSECRATION
    | EXORCISM
    | HOLY_SHOCK
    | HOLY_STRIKE
    | HOLY_WRATH
    | LIGHTS_VIGIL
    | LIGHTS_VIGIL_STRIKE;
