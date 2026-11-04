//! The Warrior's talent trees, each talent with its ranks, so a talent string with more points
//! in a talent than it has ranks is refused with the reason before any rank is read. An old or
//! foreign string read in the wrong order can put five points in a talent of two ranks, which
//! the rank ladders panic on.
//!
//! The Go reference reads the proto's order (Go's field numbers: Arms, Fury, Protection). Forever's
//! rules read the client's own order since Blizzard's development notes of 2 November 2026
//! (row, then column, as Wowhead's Forever talent calculator lists it): Arms keeps its order with
//! Improved Tactical Mastery at three ranks and Bloodthrill at two, Fury has 18 talents with
//! Raging Blows gone and Reckless Swings, Gore Drinker and Improved Whirlwind placed among the
//! others, and Protection is unchanged.

use crate::prepare::sim::Rules;
use crate::prepare::talent_layout::{new_talent, talent, LayoutTalent};

const GO_ARMS: &[LayoutTalent] = &[
    talent("improved_heroic_strike", "Improved Heroic Strike", 3),
    talent("deflection", "Deflection", 5),
    talent("improved_rend", "Improved Rend", 3),
    talent("improved_charge", "Improved Charge", 2),
    talent("improved_tactical_mastery", "Improved Tactical Mastery", 5),
    talent("improved_overpower", "Improved Overpower", 2),
    talent("anger_management", "Anger Management", 1),
    talent("deep_wounds", "Deep Wounds", 3),
    talent("spearing_strike", "Spearing Strike", 1),
    talent(
        "two_handed_weapon_specialization",
        "Two-Handed Weapon Specialization",
        3,
    ),
    talent("impale", "Impale", 2),
    talent("bloodthrill", "Bloodthrill", 5),
    talent("sweeping_strikes", "Sweeping Strikes", 1),
    talent("weaponmaster", "Weaponmaster", 5),
    talent("improved_slam", "Improved Slam", 2),
    talent("improved_hamstring", "Improved Hamstring", 3),
    talent("mortal_strike", "Mortal Strike", 1),
];

const GO_FURY: &[LayoutTalent] = &[
    talent("booming_voice", "Booming Voice", 5),
    talent("cruelty", "Cruelty", 5),
    talent("lingering_rage", "Lingering Rage", 5),
    talent("unbridled_wrath", "Unbridled Wrath", 5),
    talent("furious_precision", "Furious Precision", 3),
    talent("piercing_howl", "Piercing Howl", 1),
    talent("blood_craze", "Blood Craze", 3),
    talent("dual_wield_specialization", "Dual Wield Specialization", 5),
    talent("raging_blows", "Raging Blows", 1),
    talent("enrage", "Enrage", 5),
    talent("improved_execute", "Improved Execute", 2),
    talent("improved_berserker_rage", "Improved Berserker Rage", 2),
    talent("death_wish", "Death Wish", 1),
    talent("improved_intercept", "Improved Intercept", 2),
    talent("flurry", "Flurry", 5),
    talent("gore_drinker", "Gore Drinker", 2),
    talent("bloodthirst", "Bloodthirst", 1),
];

/// Forever's Arms: the order is the proto's; Improved Tactical Mastery has 3 ranks (was 5, the
/// same top value of 15 Rage) and Bloodthrill 2 (10/20%, was 5).
const FOREVER_ARMS: &[LayoutTalent] = &[
    talent("improved_heroic_strike", "Improved Heroic Strike", 3),
    talent("deflection", "Deflection", 5),
    talent("improved_rend", "Improved Rend", 3),
    talent("improved_charge", "Improved Charge", 2),
    talent("improved_tactical_mastery", "Improved Tactical Mastery", 3),
    talent("improved_overpower", "Improved Overpower", 2),
    talent("anger_management", "Anger Management", 1),
    talent("deep_wounds", "Deep Wounds", 3),
    talent("spearing_strike", "Spearing Strike", 1),
    talent(
        "two_handed_weapon_specialization",
        "Two-Handed Weapon Specialization",
        3,
    ),
    talent("impale", "Impale", 2),
    talent("bloodthrill", "Bloodthrill", 2),
    talent("sweeping_strikes", "Sweeping Strikes", 1),
    talent("weaponmaster", "Weaponmaster", 5),
    talent("improved_slam", "Improved Slam", 2),
    talent("improved_hamstring", "Improved Hamstring", 3),
    talent("mortal_strike", "Mortal Strike", 1),
];

/// Forever's Fury, in the client's row and column order: Booming Voice, Cruelty (row 1), Lingering
/// Rage, Unbridled Wrath (row 2), Furious Precision, Piercing Howl, Blood Craze, Enrage (row 3),
/// Dual Wield Specialization, Reckless Swings, Improved Execute (row 4), Improved Berserker Rage,
/// Death Wish, Gore Drinker, Improved Intercept (row 5), Improved Whirlwind, Flurry (row 6) and
/// Bloodthirst (row 7). Reckless Swings and Improved Whirlwind have no field in the pinned proto.
const FOREVER_FURY: &[LayoutTalent] = &[
    talent("booming_voice", "Booming Voice", 5),
    talent("cruelty", "Cruelty", 5),
    talent("lingering_rage", "Lingering Rage", 5),
    talent("unbridled_wrath", "Unbridled Wrath", 5),
    talent("furious_precision", "Furious Precision", 3),
    talent("piercing_howl", "Piercing Howl", 1),
    talent("blood_craze", "Blood Craze", 3),
    talent("enrage", "Enrage", 5),
    talent("dual_wield_specialization", "Dual Wield Specialization", 5),
    new_talent("reckless_swings", "Reckless Swings", 1),
    talent("improved_execute", "Improved Execute", 2),
    talent("improved_berserker_rage", "Improved Berserker Rage", 2),
    talent("death_wish", "Death Wish", 1),
    talent("gore_drinker", "Gore Drinker", 2),
    talent("improved_intercept", "Improved Intercept", 2),
    new_talent("improved_whirlwind", "Improved Whirlwind", 2),
    talent("flurry", "Flurry", 3),
    talent("bloodthirst", "Bloodthirst", 1),
];

const PROTECTION: &[LayoutTalent] = &[
    talent("improved_bloodrage", "Improved Bloodrage", 2),
    talent("shield_specialization", "Shield Specialization", 5),
    talent("iron_will", "Iron Will", 5),
    talent("anticipation", "Anticipation", 5),
    talent("improved_revenge", "Improved Revenge", 3),
    talent("improved_thunder_clap", "Improved Thunder Clap", 3),
    talent("last_stand", "Last Stand", 1),
    talent("master_of_defense", "Master of Defense", 2),
    talent("improved_disarm", "Improved Disarm", 3),
    talent("defiance", "Defiance", 3),
    talent("improved_sunder_armor", "Improved Sunder Armor", 3),
    talent("vanguard", "Vanguard", 1),
    talent("improved_shield_bash", "Improved Shield Bash", 2),
    talent("improved_shield_wall", "Improved Shield Wall", 2),
    talent("concussion_blow", "Concussion Blow", 1),
    talent("focused_rage", "Focused Rage", 3),
    talent("bastion", "Bastion", 5),
    talent("shield_slam", "Shield Slam", 1),
];

/// The three trees under `rules`, in the order a talent string lists them.
pub(super) fn trees(rules: Rules) -> [&'static [LayoutTalent]; 3] {
    match rules {
        Rules::Forever => [FOREVER_ARMS, FOREVER_FURY, PROTECTION],
        Rules::GoReference => [GO_ARMS, GO_FURY, PROTECTION],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classes::warrior::prepare::spell_data::spell_data;
    use crate::classes::warrior::prepare::trees_20261104::forever;

    #[test]
    fn the_trees_have_go_sizes() {
        // Go warrior.TalentTreeSizes.
        assert_eq!(
            trees(Rules::GoReference).map(<[LayoutTalent]>::len),
            [17, 17, 18]
        );
    }

    /// Forever's Fury has 18 talents (wowsims/forever #841's TalentTreeSizes follows).
    #[test]
    fn forever_fury_has_eighteen() {
        assert_eq!(
            trees(Rules::Forever).map(<[LayoutTalent]>::len),
            [17, 18, 18]
        );
    }

    /// The ranks of the talents the 4 November pass changed are the length of their ladders.
    #[test]
    fn forever_ranks_match_the_ladders() {
        let forever = forever();
        for (field, len) in [
            ("flurry", forever.flurry.len()),
            (
                "improved_tactical_mastery",
                forever.improved_tactical_mastery.len(),
            ),
            ("bloodthrill", forever.bloodthrill.len()),
            ("improved_whirlwind", forever.improved_whirlwind.len()),
        ] {
            let ranks = trees(Rules::Forever)
                .iter()
                .flat_map(|tree| tree.iter())
                .find(|talent| talent.field == field)
                .unwrap_or_else(|| panic!("{field} is in no tree"))
                .max_ranks;
            assert_eq!(ranks as i32, len, "{field}");
        }
    }

    /// The ranks of a talent are the length of its ladder, where it has one.
    #[test]
    fn ranks_match_the_ladders() {
        let data = spell_data();
        let ladders = [
            ("improved_heroic_strike", data.improved_heroic_strike.len()),
            ("deflection", data.deflection.len()),
            ("improved_rend", data.improved_rend.len()),
            ("improved_charge", data.improved_charge.len()),
            (
                "improved_tactical_mastery",
                data.improved_tactical_mastery.len(),
            ),
            ("improved_overpower", data.improved_overpower.len()),
            ("deep_wounds", data.deep_wounds.len()),
            (
                "two_handed_weapon_specialization",
                data.two_handed_weapon_specialization.len(),
            ),
            ("impale", data.impale.len()),
            ("bloodthrill", data.bloodthrill.len()),
            ("weaponmaster", data.weaponmaster.len()),
            ("improved_slam", data.improved_slam.len()),
            ("improved_hamstring", data.improved_hamstring.len()),
            ("booming_voice", data.booming_voice.len()),
            ("cruelty", data.cruelty.len()),
            ("lingering_rage", data.lingering_rage.len()),
            ("unbridled_wrath", data.unbridled_wrath.len()),
            ("furious_precision", data.furious_precision.len()),
            ("blood_craze", data.blood_craze.len()),
            (
                "dual_wield_specialization",
                data.dual_wield_specialization.len(),
            ),
            ("enrage", data.enrage.len()),
            ("improved_execute", data.improved_execute.len()),
            (
                "improved_berserker_rage",
                data.improved_berserker_rage.len(),
            ),
            ("improved_intercept", data.improved_intercept.len()),
            ("flurry", data.flurry.len()),
            ("gore_drinker", data.gore_drinker.len()),
            ("improved_bloodrage", data.improved_bloodrage.len()),
            ("shield_specialization", data.shield_specialization.len()),
            ("iron_will", data.iron_will.len()),
            ("anticipation", data.anticipation.len()),
            ("improved_revenge", data.improved_revenge.len()),
            ("improved_thunder_clap", data.improved_thunder_clap.len()),
            ("master_of_defense", data.master_of_defense.len()),
            ("improved_disarm", data.improved_disarm.len()),
            ("defiance", data.defiance.len()),
            ("improved_sunder_armor", data.improved_sunder_armor.len()),
            ("improved_shield_bash", data.improved_shield_bash.len()),
            ("improved_shield_wall", data.improved_shield_wall.len()),
            ("focused_rage", data.focused_rage.len()),
            ("bastion", data.bastion.len()),
        ];
        for (field, len) in ladders {
            let ranks = trees(Rules::GoReference)
                .iter()
                .flat_map(|tree| tree.iter())
                .find(|talent| talent.field == field)
                .unwrap_or_else(|| panic!("{field} is in no tree"))
                .max_ranks;
            assert_eq!(ranks as i32, len, "{field}");
        }
    }
}
