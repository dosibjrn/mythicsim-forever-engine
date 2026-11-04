//! The Rogue rework of the hotfix pushes 113351 (Poised) and 113483 (the talent trees), client
//! 1.60.1.70755 with hotfixes to push 113583, which only Forever's rules read. The Go reference
//! keeps the frozen data (client 1.60.1.70425), so every Go golden holds.
//!
//! The talents new to the trees have no row in the frozen data, so their per rank values are
//! kept here, each from the TraitDefinition curve of the row it names (wowsims/forever #840's
//! store; `tools/dev_notes_check.py` checks every one against a live store). Wowhead's talent
//! calculator and tooltips state the same numbers, except Blood Splatter's spell row, which
//! reads 4 and 6 sec where its curve (and the calculator) read 6 and 9.

use crate::prepare::sim::{Duration, MILLISECOND, SECOND};
use crate::prepare::spelldata::Ladder;

use crate::prepare::hotfix_113554;
use crate::prepare::sim::Rules;

/// Shank at level 44, the rank a level 60 rogue knows: 5 Energy, generating 25, a 10 sec
/// cooldown, 50% off hand weapon damage. It supersedes rank 1 (1329283, level 24: generating 15).
pub(crate) const SHANK_SPELL_ID: i32 = 1_329_549;
/// Shank at level 24.
pub(crate) const SHANK_RANK_ONE_SPELL_ID: i32 = 1_329_283;
/// The provisional id #200 gave Shank before the client stated it. The app's presets name it, so
/// a rotation that does still presses Shank.
pub(crate) const SHANK_PROVISIONAL_SPELL_ID: i32 = 1_399_001;
/// The aura Poised (the passive 1328825, level 18) grants: 1328826, one charge, -10 Energy.
pub(crate) const POISED_SPELL_ID: i32 = 1_328_826;
/// The spell Boot Knife (the talent 1329286) has Shank cast to add its Combo Points.
pub(crate) const BOOT_KNIFE_TRIGGERED_SPELL_ID: i32 = 1_329_285;
/// Ghostly Strike's new buff: the next Ambush or Garrote does not require Stealth (7 sec, one
/// charge).
pub(crate) const GHOSTLY_STRIKE_READY_SPELL_ID: i32 = 1_329_326;

/// Death Mark at level 60 (rank 4); ranks 1 to 3 (levels 40, 48 and 56) press it.
pub(crate) const DEATH_MARK_SPELL_ID: i32 = 1_325_758;
pub(crate) const DEATH_MARK_LOWER_RANKS: [i32; 3] = [1_316_709, 1_325_756, 1_325_757];
/// Death Mark: its cost, cooldown and the time the mark lasts, from the rows of rank 4
/// (Energy 25 and 1 Combo Point at least, 10 sec, 6 sec, 20 yd).
pub(crate) const DEATH_MARK_ENERGY_COST: i32 = 25;
pub(crate) const DEATH_MARK_COOLDOWN: Duration = 10 * SECOND;
pub(crate) const DEATH_MARK_DURATION: Duration = 6 * SECOND;
pub(crate) const DEATH_MARK_RANGE: f64 = 20.0;
/// What the tooltip of rank 4 states (Wowhead, 4 November 2026): 12% of the damage dealt stored
/// for each Combo Point spent, released after the mark's 6 sec up to
/// `(156 + 245 * points + points * 0.04 * attack power) * (crit chance + 1)`. Garrote and Rupture
/// damage is stored at twice the rate, and Coup de Grace multiplies both the rate and the cap.
pub(crate) const DEATH_MARK_STORED_PER_POINT: f64 = 0.12;
pub(crate) const DEATH_MARK_CAP_BASE: f64 = 156.0;
pub(crate) const DEATH_MARK_CAP_PER_POINT: f64 = 245.0;
pub(crate) const DEATH_MARK_ATTACK_POWER_PER_POINT: f64 = 0.04;

/// Poised's cost reduction, Energy: 1328826's effect.
pub(crate) const POISED_COST_REDUCTION: i32 = 10;
/// Composure (1329265, two ranks, curve -5/-10 on the effect that modifies Poised's value).
const COMPOSURE: [i32; 2] = [5, 10];

/// Flash of Steel (1329284, five ranks): Energy Shank generates (effect 0, +3 a rank) and the
/// cooldown it takes off in milliseconds (effect 1, curve -400 a rank; the spell row's own -2500
/// is not what the curve and Wowhead's tooltip state).
const FLASH_OF_STEEL_ENERGY: [f64; 5] = [3.0, 6.0, 9.0, 12.0, 15.0];
const FLASH_OF_STEEL_COOLDOWN_MS: [i64; 5] = [400, 800, 1200, 1600, 2000];

/// Boot Knife (1329286, five ranks): the chance Shank casts 1329285, which adds two Combo
/// Points (the spell's tooltip says one; its rows and Wowhead's talent text say two).
const BOOT_KNIFE_CHANCE_PERCENT: [f64; 5] = [20.0, 40.0, 60.0, 80.0, 100.0];
pub(crate) const BOOT_KNIFE_COMBO_POINTS: i32 = 2;

/// Blood Splatter (1329256, one rank): Rupture lasts 6 sec longer and Garrote 9 sec, the
/// TraitDefinition curve (6000 and 9000 ms).
pub(crate) const BLOOD_SPLATTER_RUPTURE: Duration = 6 * SECOND;
pub(crate) const BLOOD_SPLATTER_GARROTE: Duration = 9 * SECOND;

/// Relentless Strikes (14179, five ranks): 4% a rank chance per Combo Point to restore the 25
/// Energy of 14181.
pub(crate) const RELENTLESS_STRIKES_CHANCE_PER_POINT_PER_RANK: f64 = 0.04;
pub(crate) const RELENTLESS_STRIKES_ENERGY: f64 = 25.0;

/// Venom (talent 1329328): Shank casts the buff 1310703, which raises Poison damage 30% and the chance to apply
/// Poisons 20% for 6 sec.
pub(crate) const VENOM_DAMAGE_BONUS: f64 = 0.30;
pub(crate) const VENOM_CHANCE_BONUS: f64 = 0.20;
pub(crate) const VENOM_DURATION: Duration = 6 * SECOND;

/// Mutilate's flat damage each hand adds, by trainer rank (the three ranks of 399956, 1241582
/// and 1241584, whose hit spells state 44, 63 and 88; the frozen data's 33, 48 and 67). The
/// weapon share multiplies it, as the parent's tooltip says.
pub(crate) fn mutilate_flat_damage(rules: Rules, parent: i32) -> f64 {
    match (rules, parent) {
        (Rules::Forever, 399956) => 44.0,
        (Rules::Forever, 1241582) => 63.0,
        (Rules::Forever, 1241584) => 88.0,
        (Rules::GoReference, 1310707) => 23.0,
        (Rules::GoReference, 399956) => 33.0,
        (Rules::GoReference, 1241582) => 48.0,
        (Rules::GoReference, 1241584) => 67.0,
        _ => 0.0,
    }
}

/// The value at a rank of a per rank table, 0 for an untaken talent.
fn at<T: Copy + Default>(table: &[T], rank: i32) -> T {
    usize::try_from(rank)
        .ok()
        .and_then(|rank| rank.checked_sub(1))
        .and_then(|index| table.get(index))
        .copied()
        .unwrap_or_default()
}

/// How much Composure adds to Poised's cost reduction at a rank.
pub(crate) fn composure_bonus(rank: i32) -> i32 {
    at(&COMPOSURE, rank)
}

/// Energy Flash of Steel adds to Shank's.
pub(crate) fn flash_of_steel_energy(rank: i32) -> f64 {
    at(&FLASH_OF_STEEL_ENERGY, rank)
}

/// Cooldown Flash of Steel takes off Shank's.
pub(crate) fn flash_of_steel_cooldown(rank: i32) -> Duration {
    at(&FLASH_OF_STEEL_COOLDOWN_MS, rank) * MILLISECOND
}

/// Boot Knife's chance, as a fraction.
pub(crate) fn boot_knife_chance(rank: i32) -> f64 {
    at(&BOOT_KNIFE_CHANCE_PERCENT, rank) / 100.0
}

/// A changed talent's ladder: the Forever curve of [`hotfix_113554`] with `forever_ranks` ranks,
/// or the frozen data's with `frozen_ranks`.
pub(crate) fn talent_ladder(
    rules: Rules,
    spell_id: i32,
    forever_ranks: i32,
    frozen_ranks: i32,
) -> Ladder {
    match rules {
        Rules::Forever => hotfix_113554::talent(rules, spell_id, forever_ranks),
        Rules::GoReference => hotfix_113554::talent(rules, spell_id, frozen_ranks),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untaken_talents_give_nothing() {
        assert_eq!(composure_bonus(0), 0);
        assert_eq!(composure_bonus(2), 10);
        assert_eq!(flash_of_steel_energy(0), 0.0);
        assert_eq!(flash_of_steel_energy(5), 15.0);
        assert_eq!(flash_of_steel_cooldown(5), 2 * SECOND);
        assert_eq!(boot_knife_chance(3), 0.6);
        assert_eq!(boot_knife_chance(0), 0.0);
        assert_eq!(boot_knife_chance(9), 0.0);
    }

    #[test]
    fn changed_ladders_have_their_rank_counts() {
        let lethality = talent_ladder(Rules::Forever, 14128, 2, 5);
        assert_eq!(lethality.len(), 2);
        assert_eq!(lethality.fraction_at(2), 0.2);
        assert_eq!(talent_ladder(Rules::GoReference, 14128, 2, 5).len(), 5);
    }
}
