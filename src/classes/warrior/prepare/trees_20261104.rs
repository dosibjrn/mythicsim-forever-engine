//! The Warrior tree changes of Blizzard's development notes of 2 November 2026, as the client
//! carries them in the hotfix cache of 3 to 4 November (build 70755, pushes up to 113596):
//! Flurry at three ranks, Improved Tactical Mastery at three, Bloodthrill at two, Raging Blows
//! (spell 1310315) turned into Improved Whirlwind, the new Reckless Swings, and Spearing
//! Strike's 75% weapon damage. Only Forever's rules read them; the Go reference keeps the
//! frozen data (client 1.60.1.70425) and its trees.
//!
//! The rows and talent curves come from wowsims/forever #841's regenerated
//! `spell_store_inputs.json` (bc2d045c8d), which states each talent's curve (`TraitPoints`:
//! Flurry 10/20/30, Improved Tactical Mastery 5/10/15, Bloodthrill 10/20, Improved Whirlwind
//! -1/-2 sec and 15/30%), and agree with Wowhead's Forever talent calculator, which lists each
//! talent's rank count and text.

use std::sync::OnceLock;

use crate::data::spells::Spell;
use crate::prepare::sim::Rules;
use crate::prepare::spelldata::{must_find, Ladder};

use super::spell_data::spell_data;

/// Improved Whirlwind (1310315, a talent of two ranks, the spell id Raging Blows had): effect 1
/// is the Whirlwind cooldown (-2000 ms at rank 2, class flag word 1 bit 4) and effect 2 a dummy,
/// the share more damage Whirlwind deals to the closest enemy (30 at rank 2).
const IMPROVED_WHIRLWIND_ROW: &str = r#"{"ID":1310315,"Name":"Improved Whirlwind","School":1,"Attr":[464,0,0,0,0,0,0,0,0,0,0,0,0,0,0,8192,0],"ClassFlags":{"Family":4,"Mask":[0,0,0,0]},"Effects":[{"ID":1339744,"SpellID":1310315,"Type":6,"Aura":107,"BasePoints":-2000,"PvpMult":1,"Misc":11,"ClassFlags":{"Family":0,"Mask":[0,4,0,0]},"ChainAmp":1,"Target":[1,0]},{"ID":1345686,"SpellID":1310315,"Index":1,"Type":6,"Aura":4,"BasePoints":30,"PvpMult":1,"ChainAmp":1,"Target":[1,0]}]}"#;

/// Reckless Swings (1329555): costs 25% of base health, a 15 sec cooldown, a 1 sec global
/// cooldown, an 8 sec aura of 4 charges whose proc (effect 1, a trigger of 1329554) is each
/// melee auto attack.
const RECKLESS_SWINGS_ROW: &str = r#"{"ID":1329555,"Name":"Reckless Swings","School":1,"Attr":[201588752,98304,0,0,0,0,0,0,0,0,0,0,0,0,64,8192,0],"DurationMs":8000,"CooldownMs":15000,"GCDMs":1000,"StartRecoveryCategory":133,"ProcChance":100,"ProcCharges":4,"ProcFlags":[4,0],"ClassFlags":{"Family":4,"Mask":[0,131072,0,0]},"Effects":[{"ID":1369167,"SpellID":1329555,"Type":6,"Aura":42,"PvpMult":1,"TriggerID":1329554,"ChainAmp":1,"Target":[1,0]}],"Powers":[{"Type":-2,"CostPct":25}]}"#;

/// Reckless Swings' proc (1329554): 5 Rage (50 tenths, an energize on the Rage power).
const RECKLESS_SWINGS_PROC_ROW: &str = r#"{"ID":1329554,"Name":"Reckless Swings","School":1,"Attr":[327680,0,0,0,0,0,0,0,0,0,0,0,0,0,0,8192,0],"ClassFlags":{"Family":4,"Mask":[0,0,0,0]},"Effects":[{"ID":1369166,"SpellID":1329554,"Type":30,"BasePoints":50,"PvpMult":1,"Misc":1,"ChainAmp":1,"Target":[1,0]},{"ID":1369168,"SpellID":1329554,"Index":1,"Type":3,"BasePoints":1,"PvpMult":1,"ChainAmp":1,"Target":[1,0]}],"Powers":[{"Type":-2}]}"#;

/// Spearing Strike's SpellEffect ids: the weapon damage share (75, was 40) and the dummy that
/// states how many times that share again it deals to Giants and Dragonkin (1, was 2).
const SPEARING_STRIKE_SHARE: i32 = 1339573;
const SPEARING_STRIKE_BONUS: i32 = 1339768;

/// What Forever's rules read in place of the frozen Warrior data.
pub(super) struct Forever {
    pub flurry: Ladder,
    pub improved_tactical_mastery: Ladder,
    pub bloodthrill: Ladder,
    pub improved_whirlwind: Ladder,
    pub reckless_swings: &'static Spell,
    pub reckless_swings_proc: &'static Spell,
    pub spearing_strike: &'static Spell,
}

fn parse(json: &str) -> &'static Spell {
    let row: Spell = serde_json::from_str(json).expect("a Warrior row of the 4 November data");
    Box::leak(Box::new(row))
}

/// Forever's Warrior data of the 4 November pass, built once.
pub(super) fn forever() -> &'static Forever {
    static FOREVER: OnceLock<Forever> = OnceLock::new();
    FOREVER.get_or_init(|| {
        let improved_whirlwind = parse(IMPROVED_WHIRLWIND_ROW);
        let mut spearing_strike = must_find(1310222).clone();
        for effect in &mut spearing_strike.effects {
            match effect.id {
                SPEARING_STRIKE_SHARE => effect.base_points = 75.0,
                SPEARING_STRIKE_BONUS => effect.base_points = 1.0,
                _ => {}
            }
        }
        Forever {
            flurry: Ladder::talent_with_curve(12319, &[vec![10.0, 20.0, 30.0]]),
            improved_tactical_mastery: Ladder::talent_with_curve(12295, &[vec![5.0, 10.0, 15.0]]),
            bloodthrill: Ladder::talent_with_curve(1289682, &[vec![10.0, 20.0]]),
            improved_whirlwind: Ladder::talent_over_row(
                improved_whirlwind,
                &[vec![-1000.0, -2000.0], vec![15.0, 30.0]],
            ),
            reckless_swings: parse(RECKLESS_SWINGS_ROW),
            reckless_swings_proc: parse(RECKLESS_SWINGS_PROC_ROW),
            spearing_strike: Box::leak(Box::new(spearing_strike)),
        }
    })
}

/// Flurry's ladder: three ranks of 10/20/30% under Forever's rules, five of 5% steps otherwise.
pub(super) fn flurry(rules: Rules) -> &'static Ladder {
    match rules {
        Rules::Forever => &forever().flurry,
        Rules::GoReference => &spell_data().flurry,
    }
}

/// Improved Tactical Mastery's ladder: 5/10/15 Rage under Forever's rules, five ranks of 3 otherwise.
pub(super) fn improved_tactical_mastery(rules: Rules) -> &'static Ladder {
    match rules {
        Rules::Forever => &forever().improved_tactical_mastery,
        Rules::GoReference => &spell_data().improved_tactical_mastery,
    }
}

/// Bloodthrill's ladder: 10/20% under Forever's rules, five ranks of 4% otherwise.
pub(super) fn bloodthrill(rules: Rules) -> &'static Ladder {
    match rules {
        Rules::Forever => &forever().bloodthrill,
        Rules::GoReference => &spell_data().bloodthrill,
    }
}

/// Spearing Strike's row: 75% weapon damage under Forever's rules, 40% otherwise.
pub(super) fn spearing_strike(rules: Rules) -> &'static Spell {
    match rules {
        Rules::Forever => forever().spearing_strike,
        Rules::GoReference => spell_data().spearing_strike.highest(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rows_state_what_the_client_does() {
        let forever = forever();
        let swings = forever.reckless_swings;
        assert_eq!(swings.duration_ms, 8000);
        assert_eq!(swings.cooldown_ms, 15000);
        assert_eq!(swings.gcd_ms, 1000);
        assert_eq!(swings.proc_charges, 4);
        assert_eq!(swings.powers[0].cost_pct, 25.0);
        assert_eq!(forever.reckless_swings_proc.effects[0].base_points, 50.0);
        // Improved Whirlwind: 1/2 sec off the cooldown, 15/30% to the closest enemy.
        let ladder = &forever.improved_whirlwind;
        assert_eq!(ladder.len(), 2);
        assert_eq!(ladder.effect_at(1).value_at(1), -1000.0);
        assert_eq!(ladder.effect_at(1).value_at(2), -2000.0);
        assert_eq!(ladder.effect_at(2).fraction_at(1), 0.15);
        assert_eq!(ladder.effect_at(2).fraction_at(2), 0.30);
        assert_eq!(forever.flurry.len(), 3);
        assert_eq!(forever.flurry.multiplier_at(3), 1.3);
        assert_eq!(forever.improved_tactical_mastery.value_at(3), 15.0);
        assert_eq!(forever.bloodthrill.fraction_at(2), 0.2);
        let spearing = forever.spearing_strike;
        assert_eq!(spearing.effects[1].base_points, 75.0);
        assert_eq!(spearing.effects[2].base_points, 1.0);
    }
}
