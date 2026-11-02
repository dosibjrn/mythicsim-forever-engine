//! Live server hotfix pushes 113531 and 113554, which reached client 1.60.1.70707 on 30 October
//! 2026 through the DBCache rather than a client build (wago.tools' CDN tables do not carry
//! them). Only Forever's rules read them; the Go reference keeps the frozen data
//! (client 1.60.1.70425) and its goldens hold.
//!
//! The rows here are the frozen rows with the hotfixed columns changed, each change naming its
//! SpellEffect or Spell column. They come from wowsims/forever #769 (push 113531) and #770 (push
//! 113554), which regenerated the store from Raidbots' `forever` DBCache.bin, and agree with
//! Wowhead's Forever tooltips. Paladin and Priest rows are not here.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::data::spells::{Effect, Spell};

use super::dbcenums;
use super::sim::Rules;
use super::spelldata::must_find;

/// The row a spell has under `rules`: the hotfixed row under Forever's rules where one exists,
/// the frozen row otherwise. Panics, as [`must_find`] does, for a spell the store lacks.
pub(crate) fn row(rules: Rules, id: i32) -> &'static Spell {
    match rules {
        Rules::Forever => hotfixed(id).unwrap_or_else(|| must_find(id)),
        Rules::GoReference => must_find(id),
    }
}

/// The hotfixed row, if the pushes changed one the engine reads.
pub(crate) fn hotfixed(id: i32) -> Option<&'static Spell> {
    static ROWS: OnceLock<HashMap<i32, &'static Spell>> = OnceLock::new();
    ROWS.get_or_init(build).get(&id).copied()
}

/// Ice Lance, every rank: the Frozen bonus and the cooldown below.
pub(crate) const ICE_LANCE_RANKS: [i32; 6] = [1312002, 400640, 1240044, 1240045, 1240046, 1240047];

/// Mage cooldown category 19, Fire Blast's, which Ice Lance joins (push 113531).
pub(crate) const FIRE_BLAST_CATEGORY: i16 = 19;

/// Hunter cooldown category 411, Explosive Trap's and Immolation Trap's, which Freezing Trap and
/// Frost Trap join from their own 2183 (push 113554).
pub(crate) const FIRE_TRAP_CATEGORY: i16 = 411;

fn build() -> HashMap<i32, &'static Spell> {
    let mut rows: HashMap<i32, Spell> = HashMap::new();
    let mut patch = |id: i32, change: &dyn Fn(&mut Spell)| {
        let row = rows.entry(id).or_insert_with(|| must_find(id).clone());
        change(row);
    };

    // Mage (push 113531).
    // Ice Lance: the bonus against Frozen targets (effect 2, a dummy) 200% to 300%, and an 8 sec
    // cooldown on category 19, which Fire Blast shares. Rank 1 (1312002) had the cooldown
    // already.
    for id in ICE_LANCE_RANKS {
        patch(id, &|row| {
            row.category = FIRE_BLAST_CATEGORY;
            row.category_cooldown_ms = 8000;
            row.effects[1].base_points = 300.0;
        });
    }
    // Winter's Chill: Cone of Cold damage taken a stack, SpellEffect 688253, 5% to 20%.
    patch(12579, &|row| effect(row, 688253).base_points = 20.0);
    // Arcane Blast's buff: 12% of Intellect as spell damage a stack (1045737, was 15), Arcane
    // Blast's cost 100% more a stack (1045738, was 175), and 18 sec (was 15).
    patch(400573, &|row| {
        row.duration_ms = 18000;
        effect(row, 1045737).base_points = 12.0;
        effect(row, 1045738).base_points = 100.0;
    });
    // Ice Barrier, every rank: Frost damage while shielded 3% to 5% (hit and periodic).
    for id in [11426, 13031, 13032, 13033] {
        patch(id, &|row| {
            for effect in &mut row.effects {
                if effect.aura == dbcenums::A_ADD_PCT_MODIFIER {
                    effect.base_points = 5.0;
                }
            }
        });
    }
    // Frigid Power: 3% to 15% (SpellEffect 1365007 and 1365008).
    patch(1326891, &|row| {
        effect(row, 1365007).base_points = 15.0;
        effect(row, 1365008).base_points = 15.0;
    });
    // Arctic Reach: the range effect (694455) covers Ice Lance too, the first mask word 160 to 131232.
    patch(16757, &|row| {
        effect(row, 694455).class_flags.mask[0] = 131_232
    });

    // Druid (push 113531).
    // Primal Bite: rank 4 (1238073) is gone, and rank 3 (1238070) is the level 60 rank with
    // rank 4's 77 flat damage (SpellEffect 1235073, was 59 at level 48).
    patch(1238070, &|row| {
        let level_60 = must_find(1238073);
        row.spell_level = level_60.spell_level;
        row.base_level = level_60.base_level;
        for (effect, top) in row.effects.iter_mut().zip(&level_60.effects) {
            effect.base_points = top.base_points;
            effect.spell_level = top.spell_level;
        }
    });
    // Shifting Power: 16 sec cooldown to 8.
    patch(1322605, &|row| row.cooldown_ms = 8000);
    // Wild Growth, every rank: instant to a 3 sec cast.
    for id in [408120, 1238214, 1238215] {
        patch(id, &|row| row.cast_time_ms = 3000);
    }
    // Naturalist: the cast time effect (695065) covers Healing Touch and Wild Growth, the second mask
    // word 33554432 to 67108864. Its damage curve doubles (spell_data in the druid's preparation).
    patch(17069, &|row| {
        effect(row, 695065).class_flags.mask[1] = 67_108_864
    });

    // Hunter (push 113554): Freezing Trap, every rank, and Frost Trap leave category 2183.
    for id in [1499, 14310, 14311, 13809] {
        patch(id, &|row| row.category = FIRE_TRAP_CATEGORY);
    }

    rows.into_iter()
        .map(|(id, row)| (id, &*Box::leak(Box::new(row))))
        .collect()
}

/// The effect with this SpellEffect ID. Panics when the row has none: the patch names a row the
/// frozen data does not have.
fn effect(row: &mut Spell, id: i32) -> &mut Effect {
    let spell = row.id;
    row.effects
        .iter_mut()
        .find(|effect| effect.id == id)
        .unwrap_or_else(|| panic!("spell {spell} has no SpellEffect {id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_patch_finds_its_row() {
        for id in ICE_LANCE_RANKS {
            let row = hotfixed(id).unwrap();
            assert_eq!(row.effects[1].base_points, 300.0);
            assert_eq!((row.category, row.category_cooldown_ms), (19, 8000));
        }
        assert_eq!(hotfixed(12579).unwrap().effects[0].base_points, 20.0);
        let primal_bite = hotfixed(1238070).unwrap();
        assert_eq!(primal_bite.effects[0].base_points, 77.0);
        assert_eq!(primal_bite.spell_level, 60);
        assert_eq!(hotfixed(1322605).unwrap().cooldown_ms, 8000);
        assert!(hotfixed(25306).is_none());
    }

    #[test]
    fn the_reference_reads_the_frozen_rows() {
        assert!(std::ptr::eq(
            row(Rules::GoReference, 12579),
            must_find(12579)
        ));
        assert!(!std::ptr::eq(row(Rules::Forever, 12579), must_find(12579)));
        assert!(std::ptr::eq(row(Rules::Forever, 25306), must_find(25306)));
    }
}
