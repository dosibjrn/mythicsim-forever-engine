//! The client rows the Warlock builds from: Go `sim/warlock/spell_data_auto_gen.go`, the ladders
//! the Warlock code reads.
//!
//! Generated with an effect held at its base points, the talent tree stating no rank value for it:
//! Decimation (effect 2 of 440870), Demonic Embrace (effect 1 of 18697), Demonic Pact and
//! Pyroclasm (effect 1 of 18073).

use std::sync::OnceLock;

use crate::prepare::spelldata::Ladder;

/// Go `generatedSpellData`, for the ladders `sim/warlock` uses.
pub(crate) struct SpellData {
    pub aftermath: Ladder,
    pub agonizing_flames: Ladder,
    pub amplify_curse: Ladder,
    pub bane: Ladder,
    pub bane_of_agony: Ladder,
    pub bane_of_doom: Ladder,
    pub bane_of_havoc: Ladder,
    pub cataclysm: Ladder,
    pub conflagrate: Ladder,
    pub corruption: Ladder,
    pub create_firestone_triggered: Ladder,
    pub create_spellstone_triggered: Ladder,
    pub curse_of_recklessness: Ladder,
    pub curse_of_the_elements: Ladder,
    pub death_coil: Ladder,
    pub decimation: Ladder,
    pub decimation_triggered: Ladder,
    pub demon_armor: Ladder,
    pub demonic_aegis: Ladder,
    pub demonic_brand: Ladder,
    pub demonic_brand_triggered: Ladder,
    pub demonic_embrace: Ladder,
    pub demonic_energies: Ladder,
    pub demonic_knowledge: Ladder,
    pub demonic_sacrifice_triggered: Ladder,
    pub destructive_reach: Ladder,
    pub drain_life: Ladder,
    pub fel_vitality: Ladder,
    pub fire_and_brimstone: Ladder,
    pub hellfire: Ladder,
    pub immolate: Ladder,
    pub improved_bane_of_agony: Ladder,
    pub improved_corruption: Ladder,
    pub improved_drains: Ladder,
    pub improved_imp: Ladder,
    pub imp_firebolt: Ladder,
    pub improved_life_tap: Ladder,
    pub improved_sayaad: Ladder,
    pub improved_shadow_bolt: Ladder,
    pub improved_shadow_bolt_triggered: Ladder,
    pub incinerate: Ladder,
    pub life_tap: Ladder,
    pub malevolence: Ladder,
    pub master_demonologist: Ladder,
    pub molten_skin: Ladder,
    pub nightfall: Ladder,
    pub nightfall_triggered: Ladder,
    pub pandemic: Ladder,
    pub rain_of_fire: Ladder,
    pub rain_of_fire_triggered: Ladder,
    pub ruin: Ladder,
    pub searing_pain: Ladder,
    pub shadow_bolt: Ladder,
    pub shadow_and_flame: Ladder,
    pub shadow_and_flame_triggered: Ladder,
    pub shadowburn: Ladder,
    pub siphon_life: Ladder,
    pub soul_fire: Ladder,
    pub soul_link_triggered: Ladder,
    pub succubus_lash_of_pain: Ladder,
    pub soul_siphon: Ladder,
    pub suppression: Ladder,
    pub unholy_power: Ladder,
    pub wrack: Ladder,
}

/// Go `spellData`.
pub(crate) fn spell_data() -> &'static SpellData {
    static DATA: OnceLock<SpellData> = OnceLock::new();
    DATA.get_or_init(|| SpellData {
        aftermath: Ladder::talent(18119, 5),
        agonizing_flames: Ladder::talent(17927, 3),
        amplify_curse: Ladder::ranked(&[18288]),
        bane: Ladder::talent(17788, 5),
        bane_of_agony: Ladder::ranked(&[980, 1014, 6217, 11711, 11712, 11713]),
        bane_of_doom: Ladder::ranked(&[603]),
        bane_of_havoc: Ladder::ranked(&[1225228]),
        cataclysm: Ladder::talent(17778, 3),
        conflagrate: Ladder::ranked(&[1293817, 1293818, 17962, 18930, 18931, 18932]),
        corruption: Ladder::ranked(&[172, 6222, 6223, 7648, 11671, 11672, 25311]),
        create_firestone_triggered: Ladder::ranked(&[23480, 23481, 23482, 23483]),
        create_spellstone_triggered: Ladder::ranked(&[1237162, 1237164, 1237165]),
        curse_of_recklessness: Ladder::ranked(&[704, 7658, 7659, 11717]),
        curse_of_the_elements: Ladder::ranked(&[440892, 1311676, 1311677, 1311680]),
        death_coil: Ladder::ranked(&[6789, 17925, 17926]),
        decimation: Ladder::talent(440870, 2),
        decimation_triggered: Ladder::ranked(&[440873]),
        demon_armor: Ladder::ranked(&[706, 1086, 11733, 11734, 11735]),
        demonic_aegis: Ladder::talent(1235316, 2),
        demonic_brand: Ladder::talent(1293695, 3),
        demonic_brand_triggered: Ladder::ranked(&[1293696]),
        demonic_embrace: Ladder::talent(18697, 5),
        demonic_energies: Ladder::talent(1225214, 2),
        demonic_knowledge: Ladder::talent(412732, 3),
        demonic_sacrifice_triggered: Ladder::ranked(&[18789, 18790, 18791, 18792]),
        destructive_reach: Ladder::talent(17917, 2),
        drain_life: Ladder::ranked(&[689, 699, 709, 7651, 11699, 11700]),
        fel_vitality: Ladder::talent(18731, 3),
        fire_and_brimstone: Ladder::talent(412751, 3),
        hellfire: Ladder::ranked(&[1949, 11683, 11684]),
        immolate: Ladder::ranked(&[348, 707, 1094, 2941, 11665, 11667, 11668, 25309]),
        improved_bane_of_agony: Ladder::talent(18827, 2),
        improved_corruption: Ladder::talent(17810, 5),
        improved_drains: Ladder::talent(403511, 3),
        improved_imp: Ladder::talent(18694, 3),
        // Source-owned client ranks. Lower-level demons no longer inherit L60 casts.
        imp_firebolt: Ladder::ranked(
            &[3110, 7799, 7800, 7801, 7802, 11762, 11763]
                .into_iter().filter(|&id| crate::data::spells::find(id).is_some())
                .collect::<Vec<_>>().as_slice()),
        improved_life_tap: Ladder::talent(18182, 2),
        improved_sayaad: Ladder::talent(18754, 3),
        improved_shadow_bolt: Ladder::talent(17793, 5),
        improved_shadow_bolt_triggered: Ladder::ranked(&[17794]),
        incinerate: Ladder::ranked(&[412758, 1293812, 1293813]),
        life_tap: Ladder::ranked(&[1454, 1455, 1456, 11687, 11688, 11689]),
        malevolence: Ladder::talent(1310949, 5),
        master_demonologist: Ladder::talent(23785, 5),
        molten_skin: Ladder::talent(1225220, 5),
        nightfall: Ladder::talent(18094, 2),
        nightfall_triggered: Ladder::ranked(&[17941]),
        pandemic: Ladder::talent(427712, 3),
        rain_of_fire: Ladder::ranked(&[5740, 6219, 11677, 11678]),
        rain_of_fire_triggered: Ladder::ranked(&[1282380, 1282383, 1282384, 1282385]),
        ruin: Ladder::talent(17959, 5),
        searing_pain: Ladder::ranked(&[5676, 17919, 17920, 17921, 17922, 17923]),
        shadow_bolt: Ladder::ranked(&[686, 695, 705, 1088, 1106, 7641, 11659, 11660, 11661, 25307]),
        shadow_and_flame: Ladder::talent(426316, 5),
        shadow_and_flame_triggered: Ladder::ranked(&[426311, 1293816]),
        shadowburn: Ladder::ranked(&[17877, 18867, 18868, 18869, 18870, 18871]),
        siphon_life: Ladder::ranked(&[18265, 18879, 18880, 18881]),
        soul_fire: Ladder::ranked(&[6353, 17924]),
        soul_link_triggered: Ladder::ranked(&[18814, 25228]),
        succubus_lash_of_pain: Ladder::ranked(
            &[7814, 7815, 7816, 11778, 11779, 11780]
                .into_iter().filter(|&id| crate::data::spells::find(id).is_some())
                .collect::<Vec<_>>().as_slice()),
        soul_siphon: Ladder::talent(17804, 3),
        suppression: Ladder::talent(18174, 5),
        unholy_power: Ladder::talent(18769, 5),
        wrack: Ladder::ranked(&[1316697]),
    })
}

/// Select a rank for the *actual* prepared actor. A level-60 request retains
/// the already frozen Go-reference rank, while a lower-level request uses only
/// ranks present at that level in the pinned native spell data. No parallel
/// spellbook or training rules are introduced here.
pub(super) fn prepared_rank(
    ladder: &Ladder,
    sim: &crate::prepare::sim::Sim,
    unit: crate::prepare::sim::UnitId,
) -> Option<&'static crate::prepare::spelldata::Spell> {
    let rank = if sim.unit(unit).level == 60 {
        ladder.highest()
    } else {
        ladder.highest_for_level(sim.unit(unit).level, sim.rules)
    };
    (!rank.is_nil()).then_some(rank)
}

#[cfg(test)]
mod level_actor_tests {
    use super::*;
    use crate::prepare::sim::{Sim, Unit, UnitType};

    #[test]
    fn lower_level_actor_does_not_register_high_level_warlock_actions() {
        let mut sim = Sim::new();
        let mut actor = Unit::new(UnitType::Player, "Warlock".to_string());
        actor.level = 20;
        let id = sim.add_unit(actor);
        let spells = spell_data();
        assert!(prepared_rank(&spells.death_coil, &sim, id).is_none());
        assert!(prepared_rank(&spells.soul_fire, &sim, id).is_none());
        assert!(prepared_rank(&spells.bane_of_agony, &sim, id).is_some());

        sim.unit_mut(id).level = 60;
        assert_eq!(
            prepared_rank(&spells.death_coil, &sim, id).unwrap().id,
            spells.death_coil.highest().id
        );
        assert_eq!(
            prepared_rank(&spells.bane_of_agony, &sim, id).unwrap().id,
            spells.bane_of_agony.highest().id
        );
    }
}
