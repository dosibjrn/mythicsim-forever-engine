//! The Shaman's three trees in the client's own order (row, then column), which Forever's rules
//! read talent strings in (`prepare::talent_layout`). The client's launch build (Wowhead's talent
//! calculator, 4 November 2026) keeps Elemental and Restoration in the Go reference's order and
//! moves one talent of Enhancement: Improved Ghost Wolf from the second row's last square to the
//! fourth row's last, after Stormstrike, so the seven talents between shift one place earlier.
//! Elemental's Lava Burst square (position 13) is Call of Thunder and Elemental Mastery's
//! (position 16) is Lightning Mastery; their proto fields keep the old names, as saved strings
//! keep their positions.

use crate::prepare::talent_layout::{talent, LayoutTalent};

/// Elemental (tree 261): the reference's order.
const ELEMENTAL: &[LayoutTalent] = &[
    talent("convection", "Convection", 5),
    talent("concussion", "Concussion", 5),
    talent("elemental_devastation", "Elemental Devastation", 3),
    talent("reverberation", "Reverberation", 5),
    talent("elemental_alacrity", "Elemental Alacrity", 3),
    talent("call_of_flame", "Call of Flame", 3),
    talent("elemental_focus", "Elemental Focus", 1),
    talent("elemental_warding", "Elemental Warding", 3),
    talent("trial_by_fire", "Trial by Fire", 2),
    talent("eye_of_the_storm", "Eye of the Storm", 3),
    talent("earthbound", "Earthbound", 1),
    talent("elemental_reach", "Elemental Reach", 2),
    talent("lava_burst", "Call of Thunder", 1),
    talent("lightning_overload", "Lightning Overload", 3),
    talent("elemental_fury", "Elemental Fury", 5),
    talent("elemental_mastery", "Lightning Mastery", 1),
];

/// Enhancement (tree 263). Improved Ghost Wolf (16262) moves from position 6 to 13.
const ENHANCEMENT: &[LayoutTalent] = &[
    talent("earths_grasp", "Earth's Grasp", 2),
    talent("thundering_strikes", "Thundering Strikes", 5),
    talent("ancestral_knowledge", "Ancestral Knowledge", 5),
    talent("guardian_totems", "Guardian Totems", 2),
    talent("mental_dexterity", "Mental Dexterity", 3),
    talent("improved_lightning_shield", "Improved Lightning Shield", 3),
    talent("elemental_weapons", "Elemental Weapons", 3),
    talent("shamanistic_focus", "Shamanistic Focus", 1),
    talent("anticipation", "Anticipation", 3),
    talent("toughness", "Toughness", 5),
    talent("flurry", "Flurry", 5),
    talent("stormstrike", "Stormstrike", 1),
    talent("improved_ghost_wolf", "Improved Ghost Wolf", 2),
    talent("spirit_weapons", "Spirit Weapons", 1),
    talent("mental_quickness", "Mental Quickness", 2),
    talent("improved_stormstrike", "Improved Stormstrike", 2),
    talent("maelstrom_weapon", "Maelstrom Weapon", 5),
    talent("rage_of_the_farseer", "Rage of the Farseer", 1),
];

/// Restoration (tree 262): the reference's order.
const RESTORATION: &[LayoutTalent] = &[
    talent("improved_healing_wave", "Improved Healing Wave", 5),
    talent("tidal_focus", "Tidal Focus", 5),
    talent("mindfulness", "Mindfulness", 3),
    talent("natural_grace", "Natural Grace", 3),
    talent("totemic_focus", "Totemic Focus", 5),
    talent("improved_reincarnation", "Improved Reincarnation", 2),
    talent("ancestral_healing", "Ancestral Healing", 3),
    talent("healing_focus", "Healing Focus", 3),
    talent("water_shield", "Water Shield", 1),
    talent("tidal_mastery", "Tidal Mastery", 5),
    talent("restorative_totems", "Restorative Totems", 5),
    talent("mana_tide_totem", "Mana Tide Totem", 1),
    talent("healing_way", "Healing Way", 3),
    talent("natures_swiftness", "Nature's Swiftness", 1),
    talent("purification", "Purification", 5),
    talent("riptide", "Riptide", 1),
];

pub(crate) const FOREVER_TREES: [&[LayoutTalent]; 3] = [ELEMENTAL, ENHANCEMENT, RESTORATION];

#[cfg(test)]
mod tests {
    use super::*;

    /// The trees keep the pinned proto's sixteen, eighteen and sixteen talents.
    #[test]
    fn trees_have_the_protos_sizes() {
        let sizes: Vec<usize> = FOREVER_TREES.iter().map(|tree| tree.len()).collect();
        assert_eq!(sizes, [16, 18, 16]);
    }
}
