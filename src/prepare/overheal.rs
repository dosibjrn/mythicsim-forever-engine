//! MythicSim's healer sims under Forever's rules: the share of each heal that real healers
//! overheal, so healing per second counts the healing that lands, not every heal in full.
//!
//! The shares are measured on Forever's level 30 beta logs on foreverlogs.gg (1 to 24 October
//! 2026, 74 public reports, `validation/2026-10-24-healer-overheal.json`): each heal's
//! overhealing over its total healing, pooled over every Holy and Discipline Priest, Holy
//! Paladin, Restoration Druid and Restoration Shaman who cast it. A heal those logs do not hold,
//! above level 30 or a talent nobody took yet, takes the pooled share of heals of its kind:
//! one player at once, over time, or a group.
//!
//! Shields keep their whole absorb: Power Word: Shield's absorbs per cast in the same logs come
//! to about 85% to 98% of what a cast can absorb. A heal of a healer's that the table does not
//! name (a potion, a trinket) counts in full, as does every heal of a damage or tank spec.

use serde_json::{json, Value};

use super::spelldata::store::find;
use crate::contracts::request::Message;

/// The player specs whose sims are MythicSim's healer sims.
const HEALER_SPECS: &[&str] = &[
    "holy_paladin",
    "healer_priest",
    "restoration_druid",
    "restoration_shaman",
];

/// Measured shares, by the heal's client name. Regrowth's direct heal and its ticks share one
/// log row, so they share one share; Healing Stream Totem's is its heal's.
const MEASURED: &[(&str, f64)] = &[
    ("Renew", 0.1606),
    ("Rejuvenation", 0.1776),
    ("Heal", 0.0388),
    ("Healing Wave", 0.0544),
    ("Regrowth", 0.1115),
    ("Flash of Light", 0.0699),
    ("Holy Light", 0.1024),
    ("Healing Touch", 0.0542),
    ("Lesser Heal", 0.0539),
    ("Healing Stream Totem", 0.6346),
    ("Flash Heal", 0.0102),
    ("Lesser Healing Wave", 0.0608),
    ("Prayer of Healing", 0.2300),
    ("Swiftmend", 0.0160),
    ("Holy Shock", 0.0658),
    ("Lay on Hands", 0.1459),
];

/// The pooled share of the measured heals on one player at once: Heal, Healing Wave, Flash of
/// Light, Holy Light, Healing Touch, Lesser Heal, Flash Heal, Lesser Healing Wave, Swiftmend and
/// Holy Shock.
const DIRECT: f64 = 0.0537;
/// The pooled share of the measured heals over time on one player: Renew and Rejuvenation.
const OVER_TIME: f64 = 0.1671;
/// The pooled share of the measured group heals: Prayer of Healing and Tranquility.
const GROUP: f64 = 0.2458;

/// Heals the logs do not hold, by the kind of heal they are. Prayer of Mending heals the next
/// player hurt, a heal on one player at once.
const ESTIMATED: &[(&str, f64)] = &[
    ("Greater Heal", DIRECT),
    ("Binding Heal", DIRECT),
    ("Prayer of Mending", DIRECT),
    ("Riptide", OVER_TIME),
    ("Chain Heal", GROUP),
    ("Tranquility", GROUP),
    ("Penance", DIRECT),
    ("Desperate Prayer", DIRECT),
    ("Wild Growth", GROUP),
    ("Light's Vigil", GROUP),
    // Bloom heals the player a heal over time just ended on, at once and for its Spirit: the
    // logs hold no Bloom yet (a push 113531 talent), so it takes the share of a heal at once.
    ("Bloom", DIRECT),
];

/// Heals whose spell id has no client row of its own, by their class spell: the Holy Paladin's
/// Holy Shock heal runs under the Go reference's 25903, and the Druid's Bloom under its
/// talent's id (1328621), which the store lacks.
const CLASS_SPELLS: &[(&str, &str)] = &[("holy_shock_heal", "Holy Shock"), ("bloom", "Bloom")];

/// Whether the player's spec is one of MythicSim's healer sims.
pub(crate) fn is_healer(player: &Message) -> bool {
    player
        .oneof("spec")
        .is_some_and(|(spec, _)| HEALER_SPECS.contains(&spec))
}

/// The overhealing share of a heal by its client name, when the table names it.
pub(crate) fn share(name: &str) -> Option<f64> {
    MEASURED
        .iter()
        .chain(ESTIMATED)
        .find(|(heal, _)| *heal == name)
        .map(|&(_, share)| share)
}

/// The player's `overheal` entries: one for each heal of a healer sim's spellbook whose client
/// name, or class spell, the table holds, none for any other spec.
pub(crate) fn export(player: &Message, spells: &Value) -> Vec<Value> {
    if !is_healer(player) {
        return Vec::new();
    }
    let mut entries: Vec<Value> = Vec::new();
    let mut seen = Vec::new();
    for spell in spells.as_array().into_iter().flatten() {
        let Some(id) = spell["action_id"]["spell_id"].as_i64() else {
            continue;
        };
        let Ok(id) = i32::try_from(id) else {
            continue;
        };
        let heals = spell["proc_mask"]
            .as_array()
            .is_some_and(|masks| masks.iter().any(|mask| mask == "ProcMaskSpellHealing"));
        if id == 0 || !heals || seen.contains(&id) {
            continue;
        }
        seen.push(id);
        let class_spell = spell["class_spell"].as_str().and_then(|class_spell| {
            CLASS_SPELLS
                .iter()
                .find(|(name, _)| *name == class_spell)
                .map(|&(_, heal)| heal)
        });
        if let Some(share) = share(&find(id).name).or_else(|| class_spell.and_then(share)) {
            entries.push(json!({"spell_id": id, "fraction": share}));
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pooled_shares_match_the_measured_heals() {
        // Healing and overhealing of each measured heal in the validation record.
        let direct: &[(f64, f64)] = &[
            (435_228.0, 16_892.0),
            (348_959.0, 18_981.0),
            (216_228.0, 15_108.0),
            (210_415.0, 21_539.0),
            (209_199.0, 11_348.0),
            (186_503.0, 10_061.0),
            (162_102.0, 1_660.0),
            (155_338.0, 9_441.0),
            (50_142.0, 803.0),
            (22_565.0, 1_484.0),
        ];
        let over_time: &[(f64, f64)] = &[(861_198.0, 138_321.0), (531_241.0, 94_367.0)];
        let group: &[(f64, f64)] = &[(112_806.0, 25_947.0), (13_888.0, 5_195.0)];
        let pooled = |rows: &[(f64, f64)]| {
            let (healing, over) = rows.iter().fold((0.0, 0.0), |(h, o), (healing, over)| {
                (h + healing, o + over)
            });
            (over / healing * 10_000.0).round() / 10_000.0
        };
        assert_eq!(pooled(direct), DIRECT);
        assert_eq!(pooled(over_time), OVER_TIME);
        assert_eq!(pooled(group), GROUP);
    }

    #[test]
    fn every_share_is_a_fraction() {
        for &(name, share) in MEASURED.iter().chain(ESTIMATED) {
            assert!((0.0..1.0).contains(&share), "{name} {share}");
        }
    }
}
