//! Go sim/warrior/warrior.go: the Warrior's class mask bits. Go starts the list with an explicit
//! `SpellMaskNone` and numbers the rest with `1 << iota`, so the first bit is `1 << 1`.

#![allow(dead_code)]

pub const BATTLE_SHOUT: i64 = 1 << 1;
pub const BERSERKER_RAGE: i64 = 1 << 2;
pub const RECKLESSNESS: i64 = 1 << 3;
pub const DEATH_WISH: i64 = 1 << 4;
pub const RETALIATION: i64 = 1 << 5;
pub const RETALIATION_HIT: i64 = 1 << 6;
pub const SHIELD_WALL: i64 = 1 << 7;
pub const LAST_STAND: i64 = 1 << 8;
pub const CHARGE: i64 = 1 << 9;
pub const INTERCEPT: i64 = 1 << 10;
pub const DEMORALIZING_SHOUT: i64 = 1 << 11;

pub const BATTLE_STANCE: i64 = 1 << 12;
pub const BERSERKER_STANCE: i64 = 1 << 13;
pub const DEFENSIVE_STANCE: i64 = 1 << 14;

pub const REND: i64 = 1 << 15;
pub const DEEP_WOUNDS: i64 = 1 << 16;
pub const SWEEPING_STRIKES: i64 = 1 << 17;
pub const SWEEPING_STRIKES_HIT: i64 = 1 << 18;
pub const SWEEPING_STRIKES_NORMALIZED_HIT: i64 = 1 << 19;
pub const HEROIC_STRIKE: i64 = 1 << 20;
pub const CLEAVE: i64 = 1 << 21;
pub const EXECUTE: i64 = 1 << 22;
pub const OVERPOWER: i64 = 1 << 23;
pub const REVENGE: i64 = 1 << 24;
pub const SLAM: i64 = 1 << 25;
pub const SUNDER_ARMOR: i64 = 1 << 26;
pub const THUNDER_CLAP: i64 = 1 << 27;
pub const WHIRLWIND: i64 = 1 << 28;
pub const WHIRLWIND_OH: i64 = 1 << 29;
pub const SHIELD_SLAM: i64 = 1 << 30;
pub const CONCUSSION_BLOW: i64 = 1 << 31;
pub const SHIELD_BASH: i64 = 1 << 32;
pub const BLOODTHIRST: i64 = 1 << 33;
pub const MORTAL_STRIKE: i64 = 1 << 34;
pub const SHIELD_BLOCK: i64 = 1 << 35;
pub const HAMSTRING: i64 = 1 << 36;
pub const PUMMEL: i64 = 1 << 37;
pub const MOCKING_BLOW: i64 = 1 << 38;
pub const CHALLENGING_SHOUT: i64 = 1 << 39;
pub const INTIMIDATING_SHOUT: i64 = 1 << 40;
pub const DISARM: i64 = 1 << 41;
pub const TAUNT: i64 = 1 << 42;
pub const VICTORY_RUSH: i64 = 1 << 43;
pub const SPEARING_STRIKE: i64 = 1 << 44;
/// Reckless Swings, new in the notes of 2 November 2026 (Forever's rules only). Go's block ends
/// at Spearing Strike, so the bit is the one Go's next entry would take.
pub const RECKLESS_SWINGS: i64 = 1 << 45;

pub const DIRECT_DAMAGE_SPELLS: i64 = SWEEPING_STRIKES_HIT
    | SWEEPING_STRIKES_NORMALIZED_HIT
    | CLEAVE
    | EXECUTE
    | HEROIC_STRIKE
    | OVERPOWER
    | REVENGE
    | SLAM
    | SHIELD_BASH
    | SUNDER_ARMOR
    | THUNDER_CLAP
    | WHIRLWIND
    | WHIRLWIND_OH
    | SHIELD_SLAM
    | BLOODTHIRST
    | MORTAL_STRIKE
    | INTERCEPT
    | RETALIATION_HIT
    | MOCKING_BLOW
    | VICTORY_RUSH
    | SPEARING_STRIKE
    | HAMSTRING
    | PUMMEL;

pub const DAMAGE_SPELLS: i64 = DIRECT_DAMAGE_SPELLS | DEEP_WOUNDS | REND;

pub const OFFENSIVE_ABILITIES: i64 = HEROIC_STRIKE
    | REND
    | SHIELD_BASH
    | CLEAVE
    | DISARM
    | WHIRLWIND
    | SUNDER_ARMOR
    | SLAM
    | HAMSTRING
    | EXECUTE
    | PUMMEL
    | REVENGE
    | OVERPOWER
    | THUNDER_CLAP
    | MOCKING_BLOW
    | MORTAL_STRIKE
    | CONCUSSION_BLOW
    | SHIELD_SLAM
    | RETALIATION
    | INTERCEPT
    | BLOODTHIRST;

/// Focused Rage's class mask (29787): the offensive abilities less Retaliation, plus these.
pub const FOCUSED_RAGE: i64 = (OFFENSIVE_ABILITIES & !RETALIATION)
    | DEMORALIZING_SHOUT
    | DEATH_WISH
    | SWEEPING_STRIKES
    | SPEARING_STRIKE
    | CHALLENGING_SHOUT
    | INTIMIDATING_SHOUT;

pub const SHOUTS: i64 = BATTLE_SHOUT | DEMORALIZING_SHOUT | INTIMIDATING_SHOUT | CHALLENGING_SHOUT;
