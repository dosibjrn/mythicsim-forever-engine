//! The Paladin tree of live server hotfix push 113554 (applied 30 October 2026 to client
//! 1.60.1.70707 through DBCache), which only Forever's rules read. The Go reference keeps the
//! 70425 tree, so its goldens hold.
//!
//! The rows are the client's as the push states them; wago.tools' CDN tables do not carry the
//! push, so they are taken from upstream wowsims/forever #772's regenerated store
//! (`sim/core/spelldata/spells_auto_gen.go`, the TraitDefinitionEffectPoints curves) and its
//! tree (`ui/sim/talents/trees/paladin.json`), checked against Wowhead's Forever talent data
//! (trees 382 Holy, 383 Protection and 381 Retribution, row, column and ranks per talent).

use std::sync::OnceLock;

use crate::prepare::dbcenums;
use crate::prepare::sim::{Duration, MILLISECOND, SECOND};
use crate::prepare::spelldata::{Effect, Power, Spell};

/// A talent of push 113554's Paladin tree: its field, its client name and its ranks. The field
/// is the pinned proto's name where the talent is there; a talent the push adds has upstream
/// #772's proto name, which the pinned proto lacks, and its points are kept beside the message.
pub(crate) struct ForeverTalent {
    pub field: &'static str,
    pub name: &'static str,
    pub max_ranks: u32,
}

const fn talent(field: &'static str, name: &'static str, max_ranks: u32) -> ForeverTalent {
    ForeverTalent {
        field,
        name,
        max_ranks,
    }
}

/// Holy (tree 382), in the client's order (row, then column). Divine Intellect moves to
/// Protection, Unyielding Faith to Retribution and Voice of Truth (1310897) is removed; Holy
/// Conduit comes from Retribution; Oath of Light (1328548) and Boundless Light (1328550) are new.
pub(crate) const HOLY: &[ForeverTalent] = &[
    talent("spiritual_focus", "Spiritual Focus", 5),
    talent("divine_strength", "Divine Strength", 5),
    talent("healing_light", "Healing Light", 3),
    talent("reverence", "Reverence", 2),
    talent("improved_seals", "Improved Seals", 5),
    talent("purifying_power", "Purifying Power", 2),
    talent("divine_favor", "Divine Favor", 1),
    talent("illumination", "Illumination", 5),
    talent("holy_conduit", "Holy Conduit", 2),
    talent("divine_precision", "Divine Precision", 2),
    talent("infusion_of_light", "Infusion of Light", 2),
    talent("oath_of_light", "Oath of Light", 1),
    talent("consecrated_ground", "Consecrated Ground", 2),
    talent("holy_shock", "Holy Shock", 1),
    talent("boundless_light", "Boundless Light", 2),
    talent("holy_power", "Holy Power", 5),
    talent("lights_vigil", "Light's Vigil", 1),
];

/// Protection (tree 383). Improved Seal of Fury (1314103) and One-Handed Weapon Specialization
/// (20196) are removed; Divine Intellect comes from Holy; Stoicism (1328546) and the capstone
/// Shieldsworn (1328544) are new.
pub(crate) const PROTECTION: &[ForeverTalent] = &[
    talent("divine_intellect", "Divine Intellect", 5),
    talent("redoubt", "Redoubt", 5),
    talent("precision", "Precision", 3),
    talent("guardians_favor", "Guardian's Favor", 2),
    talent("sacred_duty", "Sacred Duty", 2),
    talent("templars_bulwark", "Templar's Bulwark", 1),
    talent(
        "improved_hammer_of_justice",
        "Improved Hammer of Justice",
        3,
    ),
    talent("shield_specialization", "Shield Specialization", 2),
    talent("toughness", "Toughness", 5),
    talent("swift_judgement", "Swift Judgement", 1),
    talent("improved_righteous_fury", "Improved Righteous Fury", 3),
    talent("anticipation", "Anticipation", 5),
    talent("stoicism", "Stoicism", 2),
    talent("holy_shield", "Holy Shield", 1),
    talent("reckoning", "Reckoning", 5),
    talent("iron_creed", "Iron Creed", 3),
    talent("shieldsworn", "Shieldsworn", 1),
];

/// Retribution (tree 381). Holy Conduit moves to Holy and Instrument of Law (1311085) is
/// removed; Unyielding Faith comes from Holy; Turn Evil (1328370) is new; Twist of Light
/// (1310735) is reworked into the capstone Unsealed Wrath, upstream's `unsealed_wrath`.
pub(crate) const RETRIBUTION: &[ForeverTalent] = &[
    talent("deflection", "Deflection", 5),
    talent("benediction", "Benediction", 5),
    talent("improved_judgement", "Improved Judgement", 2),
    talent("conviction", "Conviction", 3),
    talent("sanctified_judgement", "Sanctified Judgement", 3),
    talent("vindication", "Vindication", 3),
    talent("unyielding_faith", "Unyielding Faith", 2),
    talent("seal_of_command", "Seal of Command", 1),
    talent("pursuit_of_justice", "Pursuit of Justice", 2),
    talent("eye_for_an_eye", "Eye for an Eye", 2),
    talent("sacred_arbiter", "Sacred Arbiter", 1),
    talent(
        "two_handed_weapon_specialization",
        "Two-Handed Weapon Specialization",
        3,
    ),
    talent("turn_evil", "Turn Evil", 2),
    talent("repentance", "Repentance", 1),
    talent("vengeance", "Vengeance", 3),
    talent("champion_of_the_light", "Champion of the Light", 5),
    talent("unsealed_wrath", "Unsealed Wrath", 1),
];

pub(crate) const TREES: [&[ForeverTalent]; 3] = [HOLY, PROTECTION, RETRIBUTION];

/// Under Forever's rules a talent above its ranks in push 113554 is refused, as Go refuses a
/// talent past its proto: an old string read in the new order can put five points in a two
/// rank talent, which a ladder has no rank for.
pub(crate) fn check_ranks(talents: &str) -> Result<(), String> {
    for (tree, digits) in talents.split('-').enumerate() {
        let Some(layout) = TREES.get(tree) else {
            break;
        };
        for (talent, digit) in layout.iter().zip(digits.chars()) {
            // As Go reads a non-digit: 0.
            let rank = digit.to_digit(10).unwrap_or(0);
            if rank > talent.max_ranks {
                let ranks = if talent.max_ranks == 1 {
                    "rank"
                } else {
                    "ranks"
                };
                return Err(format!(
                    "paladin talents {talents:?}: {} has {} {ranks}, the string gives {rank}",
                    talent.name, talent.max_ranks
                ));
            }
        }
    }
    Ok(())
}

/// The value of a per rank table at a talent's points; 0 untaken.
pub(crate) fn at(table: &[f64], points: i32) -> f64 {
    match usize::try_from(points) {
        Ok(0) | Err(_) => 0.0,
        Ok(n) => table[(n - 1).min(table.len() - 1)],
    }
}

/// Spiritual Focus (20205): 5 ranks, curve 14/28/42/56/70% pushback resistance (was 35/70).
pub(crate) const SPIRITUAL_FOCUS: [f64; 5] = [14.0, 28.0, 42.0, 56.0, 70.0];
/// Reverence (1310899): 2 ranks, curve 15/30% of mana regeneration while casting (was 10/20/30).
pub(crate) const REVERENCE: [f64; 2] = [15.0, 30.0];
/// Improved Seals (20224): 5 ranks, curve 3/6/9/12/15% (was 5/10/15).
pub(crate) const IMPROVED_SEALS: [f64; 5] = [3.0, 6.0, 9.0, 12.0, 15.0];
/// Divine Precision (1310904, effect 1): 2 ranks, curve 4/8% Holy hit (was 6/12/18). Effect 2,
/// silence duration -10/-20%, is nothing the sim suffers.
pub(crate) const DIVINE_PRECISION: [f64; 2] = [4.0, 8.0];
/// Boundless Light (1328550): 2 ranks, curve 15/30% Holy Shock crit on an ally without the
/// paladin's Oath of Light.
pub(crate) const BOUNDLESS_LIGHT: [f64; 2] = [15.0, 30.0];
/// Holy Power (5923, effect 683158): A_MOD_CRITICAL_HEALING_AMOUNT on the Holy school, curve
/// 10/20/30/40/50% (was crit chance 1-5% and 3-15%).
pub(crate) const HOLY_POWER: [f64; 5] = [10.0, 20.0, 30.0, 40.0, 50.0];

/// Redoubt (20127): curve 6/12/18/24/30% block (was 4/8/12/16/20).
pub(crate) const REDOUBT: [f64; 5] = [6.0, 12.0, 18.0, 24.0, 30.0];
/// Shield Specialization (20150): 2 ranks, curves 15/30% block value and 50/100% mana chance
/// (was 3 ranks, 10/20/30 and 33/66/100).
pub(crate) const SHIELD_SPECIALIZATION_BLOCK_VALUE: [f64; 2] = [15.0, 30.0];
pub(crate) const SHIELD_SPECIALIZATION_CHANCE: [f64; 2] = [50.0, 100.0];
/// Sacred Duty (1224697): effect 1213490 is now A_ADD_FLAT_MODIFIER on Forbearance's duration,
/// curve -5000/-10000 ms (was +2/4% Stamina); effect 2's cooldown curve stays -30000/-60000.
pub(crate) const SACRED_DUTY_FORBEARANCE_MS: [f64; 2] = [-5000.0, -10000.0];
/// Improved Righteous Fury (20468): curve -3/-6/-9% damage taken (was -2/-4/-6).
pub(crate) const IMPROVED_RIGHTEOUS_FURY: [f64; 3] = [-3.0, -6.0, -9.0];
/// Iron Creed (1311034): 3 ranks, curves 10/20/30% Holy Strike threat and -2/-4/-6% damage
/// taken (was 5 ranks, 5-25 and 2-10).
pub(crate) const IRON_CREED_THREAT: [f64; 3] = [10.0, 20.0, 30.0];
pub(crate) const IRON_CREED_DAMAGE_TAKEN: [f64; 3] = [-2.0, -4.0, -6.0];
/// Shieldsworn (1328544, effect 1367611): Holy Strike's cooldown, A_ADD_FLAT_MODIFIER -4000 ms.
pub(crate) const SHIELDSWORN_HOLY_STRIKE_COOLDOWN: Duration = -4 * SECOND;
/// Righteous Fury (25780) effect 1367626: A_MOD_THREAT +33% on every school.
pub(crate) const RIGHTEOUS_FURY_ALL_THREAT: f64 = 0.33;

/// Conviction (20117, effect 698173): 3 ranks of A_MOD_CRIT_PCT, curve 2/4/6% crit with every
/// spell and attack (was 5 ranks of 1-5% weapon crit).
pub(crate) const CONVICTION: [f64; 3] = [2.0, 4.0, 6.0];
/// Two-Handed Weapon Specialization (20111) effect 1367246: A_MOD_THREAT on every school, curve
/// -7/-14/-20%, with the two-hander the talent requires. Its damage curve stays 2/4/6.
pub(crate) const TWO_HANDED_THREAT: [f64; 3] = [-7.0, -14.0, -20.0];
/// Champion of the Light (1311084): 5 ranks, curve 12/24/36/48/60% of Intellect as spell
/// damage (was 3 ranks), and effect 1367233, Hammer of Wrath's cast time, curve
/// -200/-400/-600/-800/-1000 ms (Instrument of Law's, which is removed).
pub(crate) const CHAMPION_OF_THE_LIGHT: [f64; 5] = [12.0, 24.0, 36.0, 48.0, 60.0];
pub(crate) const CHAMPION_OF_THE_LIGHT_HAMMER_OF_WRATH_MS: [f64; 5] =
    [-200.0, -400.0, -600.0, -800.0, -1000.0];

/// Oath of Light (1328548), a spell the push adds: wago.tools has no row for it, so it is
/// upstream wowsims/forever #772's, from the DBCache. An instant 8% base mana cast on the global
/// cooldown (category 133), 40 yards, 5 minutes, holy. Its three effects state the bonus to Holy
/// Light, Flash of Light and Holy Shock as points per level (6.67, 1.92 and 3.83), which read at
/// level 60 give the tooltip's 400, 115 and 229.
pub(crate) const OATH_OF_LIGHT: i32 = 1_328_548;

/// The Oath of Light row.
pub(crate) fn oath_of_light() -> &'static Spell {
    static ROW: OnceLock<Spell> = OnceLock::new();
    ROW.get_or_init(|| {
        let effect = |id: i32, index: u8, aura: i32, ppl: f64| Effect {
            id,
            spell_id: OATH_OF_LIGHT,
            index,
            effect_type: dbcenums::E_APPLY_AURA,
            aura,
            ppl,
            pvp_mult: 1.0,
            chain_amp: 1.0,
            ..Effect::default()
        };
        Spell {
            id: OATH_OF_LIGHT,
            name: "Oath of Light".to_string(),
            school: 2,
            duration_ms: 300_000,
            max_range: 40.0,
            gcd_ms: 1500,
            start_recovery_category: 133,
            defense_type: 1,
            max_targets: 1,
            effects: vec![
                effect(
                    1367618,
                    0,
                    dbcenums::A_PERIODIC_DUMMY,
                    6.666_669_845_581_055,
                ),
                effect(1367619, 1, dbcenums::A_DUMMY, 1.916_669_964_790_344_2),
                effect(1367620, 2, dbcenums::A_DUMMY, 3.833_329_916_000_366),
            ],
            powers: vec![Power {
                power_type: dbcenums::POWER_MANA,
                cost_pct: 8.0,
                ..Power::default()
            }],
            ..Spell::default()
        }
    })
}

/// The Echoes Unsealed Wrath (1310735) stores, one per seal. Echo of Fury (1311701) states 15
/// sec and 3 stacks; upstream #772 reads it for all four.
pub(crate) const ECHO_DURATION: Duration = 15 * SECOND;
pub(crate) const ECHO_MAX_STACKS: i32 = 3;

/// Holy Shield's ranks in push 113554: levels 30/40/50 (were 40/50/60), each with the damage
/// (A_PROC_TRIGGER_DAMAGE) and mana cost of the rank below; Rank 1 is new at 75 damage and 100
/// mana. As (spell id, damage, cost). Rank 4 (1239570, 221 damage) states no school, cost,
/// level or shield requirement, and upstream #772 does not register it, so neither does this.
pub(crate) const HOLY_SHIELD: [(i32, f64, i32); 3] =
    [(20925, 75.0, 100), (20927, 110.0, 150), (20928, 153.0, 195)];

/// `time.Duration(value) * time.Millisecond` for a curve value in milliseconds.
pub(crate) fn millis(value: f64) -> Duration {
    (value as i64).wrapping_mul(MILLISECOND)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trees_are_seventeen_talents_each() {
        assert_eq!(TREES.map(<[ForeverTalent]>::len), [17, 17, 17]);
    }

    #[test]
    fn rank_tables_match_the_layout() {
        let ranks = |field: &str| {
            TREES
                .iter()
                .flat_map(|tree| tree.iter())
                .find(|talent| talent.field == field)
                .map(|talent| talent.max_ranks as usize)
                .unwrap()
        };
        assert_eq!(ranks("spiritual_focus"), SPIRITUAL_FOCUS.len());
        assert_eq!(ranks("reverence"), REVERENCE.len());
        assert_eq!(ranks("improved_seals"), IMPROVED_SEALS.len());
        assert_eq!(ranks("divine_precision"), DIVINE_PRECISION.len());
        assert_eq!(ranks("boundless_light"), BOUNDLESS_LIGHT.len());
        assert_eq!(ranks("holy_power"), HOLY_POWER.len());
        assert_eq!(ranks("redoubt"), REDOUBT.len());
        assert_eq!(
            ranks("shield_specialization"),
            SHIELD_SPECIALIZATION_CHANCE.len()
        );
        assert_eq!(ranks("sacred_duty"), SACRED_DUTY_FORBEARANCE_MS.len());
        assert_eq!(
            ranks("improved_righteous_fury"),
            IMPROVED_RIGHTEOUS_FURY.len()
        );
        assert_eq!(ranks("iron_creed"), IRON_CREED_THREAT.len());
        assert_eq!(ranks("conviction"), CONVICTION.len());
        assert_eq!(
            ranks("two_handed_weapon_specialization"),
            TWO_HANDED_THREAT.len()
        );
        assert_eq!(ranks("champion_of_the_light"), CHAMPION_OF_THE_LIGHT.len());
    }

    #[test]
    fn oath_of_light_reads_the_tooltips_bonuses_at_level_60() {
        let row = oath_of_light();
        let bonus = |n| {
            row.effect_n(n)
                .average(crate::prepare::character::constants::CHARACTER_LEVEL)
        };
        // "up to 400 ... up to 115 ... up to 229" (Holy Light, Flash of Light, Holy Shock).
        assert_eq!([bonus(1), bonus(2), bonus(3)], [400.0, 115.0, 229.0]);
        assert_eq!(row.mana_cost().base_cost_percent, 8.0);
        assert_eq!(row.duration(), 300 * SECOND);
        assert_eq!(row.gcd(), 1500 * MILLISECOND);
        assert_eq!(row.cast_time(), 0);
    }

    #[test]
    fn at_reads_the_rank() {
        assert_eq!(at(&CONVICTION, 0), 0.0);
        assert_eq!(at(&CONVICTION, 2), 4.0);
        assert_eq!(at(&IMPROVED_SEALS, 5), 15.0);
    }
}
