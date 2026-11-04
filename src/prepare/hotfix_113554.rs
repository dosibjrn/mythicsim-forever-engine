//! Live server hotfix pushes 113531 and 113554, which reached client 1.60.1.70707 on 30 October
//! 2026 through the DBCache rather than a client build (wago.tools' CDN tables do not carry
//! them). Only Forever's rules read them; the Go reference keeps the frozen data
//! (client 1.60.1.70425) and its goldens hold.
//!
//! The rows here are the frozen rows with the hotfixed columns changed, each change naming its
//! SpellEffect or Spell column. They come from wowsims/forever #769 (push 113531) and #770 (push
//! 113554), which regenerated the store from Raidbots' `forever` DBCache.bin, and agree with
//! Wowhead's Forever tooltips. Paladin and Priest rows are not here.
//!
//! The live hotfix cache of 3 November 2026 (still client 1.60.1.70707), which carries the Hunter
//! part of Blizzard's development notes of 2 November, is here too: wowsims/forever #824
//! regenerated the store from it (`--cdn --dbcache --carry-hotfixes`).

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::data::spells::{Effect, Spell};

use super::spelldata::Ladder;

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

/// The Hunter shot rows the live hotfix cache of 3 November raised, as (spell, SpellEffect, base
/// points). wowsims/forever #824's `spells_auto_gen.go` diff.
pub(crate) const HUNTER_SHOTS_20261103: [(i32, i32, f64); 32] = [
    // Aimed Shot ranks 1 to 6: 20, 34, 55, 89, 125, 166 before.
    (19434, 696631, 40.0),
    (20900, 697762, 70.0),
    (20901, 699653, 115.0),
    (20902, 697763, 185.0),
    (20903, 699670, 260.0),
    (20904, 697764, 350.0),
    // Arcane Shot ranks 1 to 8: 20, 26, 39, 65, 94, 134, 170, 217 before.
    (3044, 680797, 25.0),
    (14281, 691213, 35.0),
    (14282, 691984, 50.0),
    (14283, 692127, 85.0),
    (14284, 690859, 120.0),
    (14285, 690933, 175.0),
    (14286, 690976, 220.0),
    (14287, 691069, 280.0),
    // Serpent Sting ranks 1 to 9, a tick: 2, 6, 12, 22, 34, 48, 64, 83, 111 before.
    (1978, 679708, 3.0),
    (13549, 691588, 7.0),
    (13550, 691589, 14.0),
    (13551, 691570, 26.0),
    (13552, 690760, 40.0),
    (13553, 690132, 58.0),
    (13554, 691479, 75.0),
    (13555, 690655, 100.0),
    (25295, 702403, 135.0),
    // Sniper Shot ranks 1 to 3: 160, 225, 295 before.
    (1310687, 1340384, 320.0),
    (1310785, 1340617, 450.0),
    (1310786, 1340619, 590.0),
    // Aspect of the Hawk ranks 1 to 6, ranged Attack Power: 15, 22, 30, 38, 47, 55 before. Rank 6
    // (14322) is the one a level 60 Hunter knows under Forever's rules (rank 7, 25296, is an
    // Ahn'Qiraj 20 book rank, and stays 120).
    (13165, 690400, 20.0),
    (14318, 691884, 35.0),
    (14319, 691886, 50.0),
    (14320, 692212, 70.0),
    (14321, 692607, 90.0),
    (14322, 692138, 110.0),
];

/// Trueshot Aura's ranged Attack Power by rank under the live hotfix cache of 3 November: 20, 35,
/// 50, 75 and 100 (rank 5, 20906, SpellEffect 697683, was 50).
pub(crate) const TRUESHOT_AURA_20261103: [(i32, f64); 5] = [
    (1299346, 20.0),
    (1299348, 35.0),
    (19506, 50.0),
    (20905, 75.0),
    (20906, 100.0),
];

/// Starfire ranks 1 to 7 as Wowhead's Forever tooltips read them from the live client on 4
/// November 2026 (the notes of 2 November say "about 40%" more): the base points and the points
/// per level of the damage effect, as (spell, base points, points per level). Rank 7 (25298)
/// reads 533 on its spell page and a tooltip of 510 to 597 at its level 66 cap (533 plus 6 times
/// 3.4, spread by the row's unchanged variance). Before: 81, 110, 144, 198, 266, 337 and 381 with
/// 1.3, 1.5, 1.6, 1.8, 2.2, 2.3 and 2.4 a level.
pub(crate) const STARFIRE_LIVE_20261104: [(i32, f64, f64); 7] = [
    (2912, 113.0, 1.8),
    (8949, 154.0, 2.1),
    (8950, 201.0, 2.2),
    (8951, 277.0, 2.5),
    (9875, 372.0, 3.0),
    (9876, 471.0, 3.2),
    (25298, 533.0, 3.4),
];

/// Rip rank 6 (9896) as the live client states it on 4 November 2026: 19 a tick (was 15) and
/// 31.9 more a tick per combo point (was 25.5), on SpellEffect 688085. The tooltip's 305, 497,
/// 688, 880 and 1071 over 12 sec are six ticks of 19 + 31.9 a point, and the buff tooltip reads
/// "19 + (32 * Combo Points)" every 2 sec.
pub(crate) const RIP_LIVE_20261104: (f64, f32) = (19.0, 31.9);

/// Rake rank 4 (9904) as the live client states it: a hit of 76 (was 61, SpellEffect 687442) and a
/// bleed of 42 every 3 sec (was 34, SpellEffect 687443), "76 damage plus 126 over 9 sec".
pub(crate) const RAKE_LIVE_20261104: (f64, f64) = (76.0, 42.0);

/// Ferocious Bite rank 5 (31018) as the live client states it: a roll of 102 (was 82,
/// SpellEffect 705079) spread over 76 (the tooltip's 245 to 321 at one combo point is 102 plus 181
/// plus or minus 38), 181 more a combo point (was 147) and 330 for the dummy that sets the damage
/// of each extra Energy (was 270, SpellEffect 1277241), "3.3 additional damage".
pub(crate) const FEROCIOUS_BITE_LIVE_20261104: FerociousBiteRow = FerociousBiteRow {
    base_points: 102.0,
    spread: 76.0,
    points_per_combo_point: 181.0,
    energy_dummy: 330.0,
};

/// See [`FEROCIOUS_BITE_LIVE_20261104`].
pub(crate) struct FerociousBiteRow {
    pub(crate) base_points: f64,
    pub(crate) spread: f64,
    pub(crate) points_per_combo_point: f32,
    pub(crate) energy_dummy: f64,
}

/// Talents whose per rank values Blizzard's development notes of 2 November 2026 change while
/// keeping the rank count, as the curve each takes under Forever's rules: one row per effect, in
/// effect order, as the client states curves. No client row states them yet.
const TALENT_CURVES_20261102: &[(i32, &[&[f64]])] = &[
    // Hunter: Barrage 5/10/15% (was 3/7/10%; the 3 November cache states it).
    (19461, &[&[5.0, 10.0, 15.0]]),
    // Warrior: Two-Handed Weapon Specialization 2/4/6% (was 1/2/3%), Defiance 7/13/20% threat
    // (was 5/10/15%), and Improved Slam's cooldown cut 2.5/5 sec (its third effect, was 3 sec at
    // either rank; the cast time and GCD curves keep their values).
    (12163, &[&[2.0, 4.0, 6.0]]),
    (12792, &[&[7.0, 13.0, 20.0]]),
    (
        12862,
        &[&[-250.0, -500.0], &[-250.0, -500.0], &[-2500.0, -5000.0]],
    ),
    // Warlock: Shadow and Flame's Shadow and Fire damage 4/8/12/16/20% (its third and fourth
    // effects, were 2/4/6/8/10%; Go reads the third). The second, Conflagrate's chance to keep
    // Immolate, and the first keep theirs.
    (
        426316,
        &[
            &[4.0, 8.0, 12.0, 16.0, 20.0],
            &[20.0, 40.0, 60.0, 80.0, 100.0],
            &[4.0, 8.0, 12.0, 16.0, 20.0],
            &[4.0, 8.0, 12.0, 16.0, 20.0],
        ],
    ),
    // Shaman: Lightning Overload 10/20/30% (was 3/7/10%), Mental Dexterity 50/100/150% of
    // Intellect (was 33/67/100%), Flurry 6/12/18/24/30% (was 5/10/15/20/25%).
    (408438, &[&[10.0, 20.0, 30.0]]),
    (415140, &[&[50.0, 100.0, 150.0]]),
    (16256, &[&[6.0, 12.0, 18.0, 24.0, 30.0]]),
    // Rogue: Aggression 5/10/15% (was 2/4/6%), on Sinister Strike and Eviscerate only; the mask
    // is in the Rogue's registration.
    (18427, &[&[5.0, 10.0, 15.0]]),
    // Rogue (the rework of pushes 113351 and 113483, client 1.60.1.70755 with hotfixes to push
    // 113583): Lethality 10/20% (two ranks, was 4 to 20% in five), Seal Fate 33/66/100% (three
    // ranks, was 20 to 100% in five), Opportunity 4 to 20% on both its effects (five ranks, was
    // 5/10%) and Dual Wield Specialization 25/50% (two ranks, was 5 to 25% in five). The
    // TraitDefinition curves of wowsims/forever #840's store; Wowhead's calculator states the
    // same texts.
    (14128, &[&[10.0, 20.0]]),
    (14186, &[&[33.0, 66.0, 100.0]]),
    (
        14057,
        &[&[4.0, 8.0, 12.0, 16.0, 20.0], &[4.0, 8.0, 12.0, 16.0, 20.0]],
    ),
    (13715, &[&[25.0, 50.0]]),
];

/// A talent's ladder under `rules`: the curve of [`TALENT_CURVES_20261102`] under Forever's rules
/// where one is listed, the frozen data's otherwise (Go `Talent`).
pub(crate) fn talent(rules: Rules, spell_id: i32, max_ranks: i32) -> Ladder {
    static LADDERS: OnceLock<HashMap<i32, Ladder>> = OnceLock::new();
    if rules == Rules::Forever {
        let ladders = LADDERS.get_or_init(|| {
            TALENT_CURVES_20261102
                .iter()
                .map(|(id, curve)| {
                    let rows: Vec<Vec<f64>> = curve.iter().map(|row| row.to_vec()).collect();
                    (*id, Ladder::talent_with_curve(*id, &rows))
                })
                .collect()
        });
        if let Some(ladder) = ladders.get(&spell_id) {
            assert_eq!(ladder.len(), max_ranks, "talent {spell_id} rank count");
            return ladder.clone();
        }
    }
    Ladder::talent(spell_id, max_ranks)
}

/// The Shaman damage rows Wowhead's Forever tooltips state on 4 November 2026 (the tooltip data
/// is the live client's, rows the launch build encrypts included), as (spell, SpellEffect, base
/// points, points per level, where the tooltip states it): every Lightning Bolt rank, Chain
/// Lightning's primary target damage and the shocks, which Blizzard's development notes of 2
/// November 2026 changed ("roughly 78%" of Classic Era for Lightning Bolt, Chain Lightning at
/// about 145% of it, shocks "reverted to match Classic Era values"). Each tooltip carries its
/// row's `ppl<spell level>:<max level>:<base>:<points per level x 100>` marker, which is where
/// the numbers come from. The rows keep their levels and their variance, and with both the
/// tooltip's range at the max level comes out within a point of Wowhead's (a test pins that).
/// Lightning Overload's copies are [`SHAMAN_OVERLOAD_COPIES`].
pub(crate) const SHAMAN_ROWS_20261104: &[(i32, i32, f64, Option<f64>)] = &[
    // Lightning Bolt ranks 1 to 10: base 15, 27, 45, 61, 82, 111, 147, 162, 178, 196 and per
    // level 0.1, 0.1, 0.3, 0.4, 0.5, 0.8, 0.8, 1, 1, 1.2 before.
    (403, 678918, 16.0, Some(0.1)),
    (529, 679369, 30.0, Some(0.1)),
    (548, 679030, 52.0, Some(0.3)),
    (915, 679313, 73.0, Some(0.5)),
    (943, 679378, 109.0, Some(0.7)),
    (6041, 684438, 148.0, Some(1.0)),
    (10391, 688309, 194.0, Some(1.0)),
    (10392, 688310, 240.0, Some(1.5)),
    (15207, 691075, 295.0, Some(1.6)),
    (15208, 691780, 353.0, Some(2.1)),
    // Chain Lightning ranks 1 to 4: base 88, 100, 112, 123 and per level 0.6, 0.6, 0.7, 0.8
    // before.
    (421, 679158, 215.0, Some(1.4)),
    (930, 679273, 348.0, Some(2.2)),
    (2860, 681324, 428.0, Some(2.7)),
    (10605, 688920, 510.0, Some(3.3)),
    // Earth Shock ranks 1 to 7: base 18, 33, 49, 81, 132, 205, 301 before. Only rank 7's per
    // level moves (1.9 to 3.4), which a level 60 shaman does not reach: its spell level is 60.
    (8042, 686790, 20.0, Some(0.5)),
    (8044, 685548, 36.0, Some(0.7)),
    (8045, 686227, 67.0, Some(0.9)),
    (8046, 686672, 130.0, Some(1.1)),
    (10412, 687897, 242.0, Some(1.3)),
    (10413, 688149, 383.0, Some(1.6)),
    (10414, 688559, 531.0, Some(3.4)),
    // Flame Shock ranks 1 to 6, the direct hit: 20, 33, 43, 82, 128, 166 before.
    (8050, 686823, 25.0, Some(0.8)),
    (8052, 685531, 51.0, Some(1.0)),
    (8053, 686555, 95.0, Some(1.2)),
    (10447, 688330, 164.0, Some(1.4)),
    (10448, 688934, 245.0, Some(1.7)),
    (29228, 704945, 292.0, Some(3.8)),
    // Flame Shock ranks 2 to 6, a tick: 8, 14, 21, 34, 44 before (rank 1's 7 stays). The
    // tooltips give the totals over 12 sec (48, 96, 168, 256, 320), four ticks each, and no
    // per level marker, so the rows keep theirs.
    (8052, 685532, 12.0, None),
    (8053, 686556, 24.0, None),
    (10447, 688331, 42.0, None),
    (10448, 688935, 64.0, None),
    (29228, 704946, 80.0, None),
    // Frost Shock ranks 1 to 4: 66, 124, 188, 283 before.
    (8056, 686558, 98.0, Some(0.9)),
    (8058, 687459, 222.0, Some(1.3)),
    (10472, 688813, 355.0, Some(1.5)),
    (10473, 688022, 506.0, Some(1.8)),
];

/// Lightning Overload's copies, as (spell, SpellEffect, the spell it copies): each deals half of
/// its rank's base points and points per level under Forever's rules. The talent's text says
/// "half damage" (Wowhead's talent calculator, 4 November 2026). Wowhead's tooltips of the copy
/// rows still state the old rows (rank 10 Lightning Bolt 98 with 0.6 a level, rank 4 Chain
/// Lightning 62 with 1.2, every rank old while every primary row above moved), so the half is
/// taken from the talent text and the new primary rows, which at level 60 gives 180 and 261 for
/// the top ranks, what the copies dealt before.
pub(crate) const SHAMAN_OVERLOAD_COPIES: [(i32, i32, i32); 14] = [
    (408439, 1072563, 403),
    (408440, 1072564, 529),
    (408441, 1072565, 548),
    (408442, 1072567, 915),
    (408443, 1072569, 943),
    (408472, 1072625, 6041),
    (408473, 1072626, 10391),
    (408474, 1072627, 10392),
    (408475, 1072628, 15207),
    (408477, 1072630, 15208),
    (408479, 1072633, 421),
    (408481, 1072635, 930),
    (408482, 1072636, 2860),
    (408484, 1072638, 10605),
];

/// Chain Lightning's spell power coefficient under the notes of 2 November: 2.5 sec of cast at
/// the standard 3.5 sec rate (0.714, was 0.571 and 0.517), and half of it on the Lightning
/// Overload copy, as before. (spell, SpellEffect, coefficient).
pub(crate) const CHAIN_LIGHTNING_COEFFICIENTS_20261102: [(i32, i32, f64); 8] = [
    (421, 679158, 0.714),
    (930, 679273, 0.714),
    (2860, 681324, 0.714),
    (10605, 688920, 0.714),
    (408479, 1072633, 0.357),
    (408481, 1072635, 0.357),
    (408482, 1072636, 0.357),
    (408484, 1072638, 0.357),
];

/// Chain Lightning's mana cost ("approximately 225%" of the
/// comparable Lightning Bolt rank's) under the notes of 2 November, as (spell, mana). The
/// comparable Lightning Bolt ranks are 6, 7, 9 and 10, the nearest by the level each is learnt.
pub(crate) const CHAIN_LIGHTNING_COSTS_20261102: [(i32, i32); 4] =
    [(421, 248), (930, 304), (2860, 428), (10605, 495)];

/// Chain Lightning's cast time under the notes of 2 November.
pub(crate) const CHAIN_LIGHTNING_CAST_MS_20261102: i32 = 2500;
/// Every Mutilate hit spell, a main hand and an off hand strike for each of four ranks, whose
/// weapon damage effect the notes of 2 November raise from 75% to 90%. The flat damage a hit adds
/// is read through the same share, so it grows with it.
pub(crate) const MUTILATE_HIT_RANKS: [i32; 8] = [
    1310705, 1310706, 399960, 399961, 1241585, 1241588, 1241586, 1241590,
];

/// Mutilate's weapon share under the development notes of 2 November: 90% (was 75%).
pub(crate) const MUTILATE_WEAPON_SHARE_20261102: f64 = 90.0;

/// Flawless Execution's cost reduction under the notes of 2 November: 15 Energy (was 10), on
/// SpellEffect 1340441 of 1310711.
pub(crate) const FLAWLESS_EXECUTION_COST_20261102: f64 = -15.0;

/// Riptide's three ranks and their mana cost as Wowhead's Forever tooltips state them on 4
/// November 2026: 245, 300 and 230 (385 before for rank 3). The notes of 2 November cut the cost
/// "by about 40%", which rank 3's 230 is (40.3%); ranks 1 and 2 read their old costs there. The
/// notes also give it a 12 sec cooldown (was 6, on every rank's tooltip) and drop its initial
/// heal, leaving the heal over time (89, 115 and 161 every 3 sec, as the rows had it), as, (spell,
/// mana).
pub(crate) const RIPTIDE_COSTS_20261104: [(i32, i32); 3] =
    [(408521, 245), (1239242, 300), (1239243, 230)];

/// Riptide's cooldown under the notes of 2 November.
pub(crate) const RIPTIDE_COOLDOWN_MS_20261102: i32 = 12000;

/// Every rank of Slam, whose cooldown the notes of 2 November cut from 18 to 15 sec.
pub(crate) const SLAM_RANKS: [i32; 5] = [1240193, 1464, 8820, 11604, 11605];

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

    // Hunter (live hotfix cache of 3 November, development notes of 2 November): every rank of
    // Aimed Shot (about 110% more bonus damage), Arcane Shot (about 30% more), Serpent Sting
    // (about 20% more a tick), Sniper Shot (twice the bonus damage) and Aspect of the Hawk ranks
    // 1 to 6. The old values are 70707's, which the frozen data shares for these rows.
    for (spell, id, points) in HUNTER_SHOTS_20261103 {
        patch(spell, &|row| effect(row, id).base_points = points);
    }

    // Druid (the live client of 4 November, which carries the notes of 2 November): every Starfire
    // rank's base damage and damage per level ("about 40%" more), Rip, Rake and Ferocious Bite.
    // The attack power shares no row states stay with the druid's preparation.
    for (id, base_points, ppl) in STARFIRE_LIVE_20261104 {
        patch(id, &|row| {
            let damage = &mut row.effects[0];
            damage.base_points = base_points;
            damage.ppl = ppl;
        });
    }
    patch(9896, &|row| {
        let tick = effect(row, 688085);
        tick.base_points = RIP_LIVE_20261104.0;
        tick.points_per_resource = RIP_LIVE_20261104.1;
    });
    patch(9904, &|row| {
        effect(row, 687442).base_points = RAKE_LIVE_20261104.0;
        effect(row, 687443).base_points = RAKE_LIVE_20261104.1;
    });
    patch(31018, &|row| {
        let bite = FEROCIOUS_BITE_LIVE_20261104;
        let roll = effect(row, 705079);
        roll.base_points = bite.base_points;
        roll.variance = bite.spread / bite.base_points;
        roll.points_per_resource = bite.points_per_combo_point;
        effect(row, 1277241).base_points = bite.energy_dummy;
    });

    // Warrior (notes of 2 November): Defensive Stance's threat bonus 30% to 80% (Defensive Stance
    // Passive, SpellEffect 686475), and Slam's 18 sec cooldown to 15 on every rank.
    patch(7376, &|row| effect(row, 686475).base_points = 80.0);
    for id in SLAM_RANKS {
        patch(id, &|row| row.category_cooldown_ms = 15000);
    }
    // Warlock (notes of 2 November): Wrack's spell damage coefficient doubles (0.143 to 0.286
    // a tick, "0.286 per second of cast time") and its base damage a second grows 2.5 a level,
    // from 36 at its level 40 to 86 at 60 (was 36 at every level).
    patch(1316697, &|row| {
        let tick = effect(row, 1349556);
        tick.ppl = 2.5;
        tick.sp_coef *= 2.0;
    });
    // Shaman (notes of 2 November): Stormstrike's cooldown 8 sec to 6.
    patch(17364, &|row| row.cooldown_ms = 6000);
    // Rogue (notes of 2 November): Mutilate deals 90% weapon damage with each hand (was 75%), and
    // Flawless Execution takes 15 Energy off Eviscerate (was 10). The client has no rows for
    // either yet.
    for id in MUTILATE_HIT_RANKS {
        patch(id, &|row| {
            for effect in &mut row.effects {
                if effect.effect_type == dbcenums::E_WEAPON_PERCENT_DAMAGE {
                    effect.base_points = MUTILATE_WEAPON_SHARE_20261102;
                }
            }
        });
    }
    patch(1310711, &|row| {
        effect(row, 1340441).base_points = FLAWLESS_EXECUTION_COST_20261102
    });

    // Shaman (notes of 2 November, rows as Wowhead states them on 4 November): Lightning Bolt,
    // Chain Lightning and the shocks, then Lightning Overload's copies at half of their primary,
    // then Chain Lightning's cast time, coefficient and mana from the notes.
    for &(spell, id, base, per_level) in SHAMAN_ROWS_20261104 {
        patch(spell, &|row| {
            let effect = effect(row, id);
            effect.base_points = base;
            if let Some(per_level) = per_level {
                effect.ppl = per_level;
            }
        });
    }
    for (spell, id, parent) in SHAMAN_OVERLOAD_COPIES {
        let &(_, _, base, per_level) = SHAMAN_ROWS_20261104
            .iter()
            .find(|row| row.0 == parent)
            .expect("every overload copy has a primary row");
        patch(spell, &|row| {
            let effect = effect(row, id);
            effect.base_points = base / 2.0;
            effect.ppl = per_level.expect("a primary row states its level scaling") / 2.0;
        });
    }
    for (spell, id, coefficient) in CHAIN_LIGHTNING_COEFFICIENTS_20261102 {
        patch(spell, &|row| effect(row, id).sp_coef = coefficient);
    }
    for (spell, cost) in CHAIN_LIGHTNING_COSTS_20261102 {
        patch(spell, &|row| {
            row.cast_time_ms = CHAIN_LIGHTNING_CAST_MS_20261102;
            row.powers[0].cost = cost;
        });
    }

    // Shaman (notes of 2 November, mana as Wowhead states it on 4 November): Riptide has a 12 sec
    // cooldown (was 6) and no initial heal, only the heal over time. The direct heal (effect 1,
    // a heal) turns into an inert dummy so the effects keep their places: the Chain Heal bonus
    // reads the third.
    for (id, cost) in RIPTIDE_COSTS_20261104 {
        patch(id, &|row| {
            row.powers[0].cost = cost;
            row.category_cooldown_ms = RIPTIDE_COOLDOWN_MS_20261102;
            row.effects[0].effect_type = dbcenums::E_DUMMY;
        });
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
        for (spell, id, points) in HUNTER_SHOTS_20261103 {
            let row = hotfixed(spell).unwrap();
            let effect = row.effects.iter().find(|effect| effect.id == id).unwrap();
            assert_eq!(effect.base_points, points, "spell {spell}");
        }
        let wrack = row(Rules::Forever, 1316697);
        assert_eq!(wrack.effects[0].average(60), 86.0);
        assert_eq!(must_find(1316697).effects[0].average(60), 36.0);
        assert!((wrack.effects[0].sp_coef - 0.286).abs() < 1e-6);
        let stance = hotfixed(7376).unwrap();
        let threat = stance
            .effects
            .iter()
            .find(|effect| effect.id == 686475)
            .unwrap();
        assert_eq!(threat.base_points, 80.0);
        for id in SLAM_RANKS {
            assert_eq!(hotfixed(id).unwrap().category_cooldown_ms, 15000);
        }
        assert_eq!(hotfixed(17364).unwrap().cooldown_ms, 6000);
        for id in MUTILATE_HIT_RANKS {
            let weapon = hotfixed(id).unwrap().effect_n(2);
            assert_eq!(
                weapon.effect_type,
                dbcenums::E_WEAPON_PERCENT_DAMAGE,
                "{id}"
            );
            assert_eq!(weapon.base_points, 90.0, "{id}");
            assert_eq!(must_find(id).effect_n(2).base_points, 75.0, "{id}");
            // The flat damage the hit adds is the frozen row's, scaled by the share.
            assert_eq!(
                hotfixed(id).unwrap().effect_n(1).base_points,
                must_find(id).effect_n(1).base_points,
                "{id}"
            );
        }
        let flawless = hotfixed(1310711).unwrap();
        assert_eq!(flawless.effects[0].base_points, -15.0);
        assert_eq!(must_find(1310711).effects[0].base_points, -10.0);
        for (id, mana) in RIPTIDE_COSTS_20261104 {
            let riptide = hotfixed(id).unwrap();
            assert_eq!(riptide.powers[0].cost, mana, "{id}");
            assert_eq!(riptide.category_cooldown_ms, 12000, "{id}");
            // The heal effect is gone and the heal over time and the Chain Heal bonus keep their
            // places.
            assert!(riptide.heal_effect().is_nil(), "{id}");
            assert_eq!(riptide.effects[1].aura, dbcenums::A_PERIODIC_HEAL, "{id}");
            assert_eq!(riptide.effects[2].base_points, 25.0, "{id}");
            assert_eq!(must_find(id).category_cooldown_ms, 6000, "{id}");
            assert!(!must_find(id).heal_effect().is_nil(), "{id}");
        }
        assert_eq!(must_find(1239243).powers[0].cost, 385);
        for (id, base_points, ppl) in STARFIRE_LIVE_20261104 {
            let starfire = hotfixed(id).unwrap();
            assert_eq!(starfire.effects[0].base_points, base_points, "{id}");
            assert_eq!(starfire.effects[0].ppl as f32, ppl as f32, "{id}");
            assert_ne!(must_find(id).effects[0].base_points, base_points, "{id}");
        }
        assert_eq!(hotfixed(9896).unwrap().effect_n(1).base_points, 19.0);
        assert_eq!(must_find(9896).effect_n(1).base_points, 15.0);
        assert_eq!(hotfixed(9904).unwrap().effect_n(1).base_points, 76.0);
        assert_eq!(hotfixed(9904).unwrap().effect_n(2).base_points, 42.0);
        assert_eq!(hotfixed(31018).unwrap().effect_n(1).base_points, 102.0);
        assert_eq!(hotfixed(31018).unwrap().effect_n(2).base_points, 330.0);
    }

    /// Starfire's rows reproduce Wowhead's live tooltips of 4 November, which show each rank at its
    /// level cap: the average is the tooltip's middle and the spread is the row's variance of the
    /// base at the rank's own level, 510 to 597 for rank 7 (533 plus 6 levels of 3.4, 87 wide).
    #[test]
    fn starfire_rows_reproduce_the_live_tooltips() {
        let tooltips = [
            (2912, 111.0, 133.0),
            (8949, 151.0, 182.0),
            (8950, 196.0, 232.0),
            (8951, 268.0, 316.0),
            (9875, 359.0, 421.0),
            (9876, 451.0, 529.0),
            (25298, 510.0, 597.0),
        ];
        for (id, low, high) in tooltips {
            let damage = &row(Rules::Forever, id).effects[0];
            let at_cap = damage.average(i32::from(damage.max_level));
            let spread = damage.variance * damage.average(i32::from(damage.spell_level));
            assert!(
                ((low + high) / 2.0 - at_cap).abs() <= 0.5,
                "{id} middle {at_cap}"
            );
            assert!((at_cap - spread / 2.0 - low).abs() <= 1.0, "{id} low");
            assert!((at_cap + spread / 2.0 - high).abs() <= 1.0, "{id} high");
        }
    }

    /// The rows' base points and levels reproduce the damage ranges of Wowhead's Forever
    /// tooltips of 4 November 2026, which state each spell's range at its max level (Lightning Bolt
    /// rank 10: 344 to 383), and Lightning Overload's copies are half of their primary.
    #[test]
    fn shaman_rows_reproduce_wowheads_tooltips() {
        let effect_of = |spell: i32, effect_id: i32| -> &'static Effect {
            row(Rules::Forever, spell)
                .effects
                .iter()
                .find(|effect| effect.id == effect_id)
                .unwrap()
        };
        for &(spell, effect_id, base, per_level) in SHAMAN_ROWS_20261104 {
            let patched = effect_of(spell, effect_id);
            let frozen = must_find(spell)
                .effects
                .iter()
                .find(|effect| effect.id == effect_id)
                .unwrap();
            assert_eq!(patched.base_points, base, "spell {spell}");
            if let Some(per_level) = per_level {
                assert!(
                    (patched.ppl - per_level).abs() < 1e-6,
                    "spell {spell}: {}",
                    patched.ppl
                );
            } else {
                assert_eq!(patched.ppl, frozen.ppl, "spell {spell}");
            }
            assert_eq!(patched.variance, frozen.variance, "spell {spell}");
            assert_eq!(
                (patched.spell_level, patched.max_level),
                (frozen.spell_level, frozen.max_level),
                "spell {spell}"
            );
        }

        // (spell, effect, the tooltip's low and high), read from the tooltips' text.
        let tooltips = [
            (403, 678918, 15, 18),
            (529, 679369, 28, 33),
            (548, 679030, 49, 58),
            (915, 679313, 71, 80),
            (943, 679378, 105, 120),
            (6041, 684438, 144, 162),
            (10391, 688309, 188, 210),
            (10392, 688310, 234, 261),
            (15207, 691075, 286, 320),
            (15208, 691780, 344, 383),
            (421, 679158, 208, 236),
            (930, 679273, 339, 379),
            (2860, 681324, 417, 466),
            (10605, 688920, 498, 555),
            (8042, 686790, 21, 24),
            (8044, 685548, 38, 41),
            (8045, 686227, 69, 74),
            (8046, 686672, 131, 140),
            (10412, 687897, 241, 256),
            (10413, 688149, 380, 402),
            (10414, 688559, 534, 562),
            (8056, 686558, 99, 106),
            (8058, 687459, 221, 236),
            (10472, 688813, 352, 373),
            (10473, 688022, 501, 529),
        ];
        for (spell, effect_id, low, high) in tooltips {
            let effect = effect_of(spell, effect_id);
            // The tooltip states the range at the spell's max level, unrounded.
            let at_max = f64::from(effect.base_points as f32)
                + f64::from(i32::from(effect.max_level) - i32::from(effect.spell_level))
                    * f64::from(effect.ppl as f32);
            let (min, max) = (
                at_max * (1.0 - effect.variance / 2.0),
                at_max * (1.0 + effect.variance / 2.0),
            );
            assert!(
                (min.round() - f64::from(low)).abs() <= 1.0,
                "{spell}: {min}"
            );
            assert!(
                (max.round() - f64::from(high)).abs() <= 1.0,
                "{spell}: {max}"
            );
        }
        // Lightning Bolt rank 10 at level 60: 353 and 2.1 a level over 4 levels, 361 (was 200).
        assert_eq!(effect_of(15208, 691780).average(60), 361.0);
        assert_eq!(must_find(15208).effects[0].average(60), 200.0);
        // Chain Lightning rank 4: 510 and 3.3 a level, 523 (was 126).
        assert_eq!(effect_of(10605, 688920).average(60), 523.0);

        // Chain Lightning's mana is about 225% of the comparable Lightning Bolt ranks' (110, 135,
        // 190 and 220 mana, ranks 6, 7, 9 and 10, the nearest by the level each is learnt), the
        // notes' number, which Wowhead's tooltips still state as 145, 200, 250 and 315.
        for ((spell, mana), bolt) in CHAIN_LIGHTNING_COSTS_20261102
            .into_iter()
            .zip([6041, 10391, 15207, 15208])
        {
            let bolt_cost = f64::from(must_find(bolt).powers[0].cost);
            assert_eq!(f64::from(mana), (bolt_cost * 2.25).round(), "spell {spell}");
            let chain = row(Rules::Forever, spell);
            assert_eq!(chain.powers[0].cost, mana);
            assert_eq!(chain.cast_time_ms, 2500);
            assert_eq!(must_find(spell).cast_time_ms, 2000);
        }
        // 2.5 sec of cast at the standard 3.5 sec rate, which Wowhead's spell pages state as
        // "SP mod: 0.714" on all four ranks.
        for (_, _, coefficient) in CHAIN_LIGHTNING_COEFFICIENTS_20261102.iter().take(4) {
            assert!((coefficient - 2.5 / 3.5).abs() < 1e-3);
        }

        // Lightning Overload's copies deal half their rank's base points, level scaling and
        // coefficient, and the top ranks deal what they did before at level 60.
        for (copy, effect_id, parent) in SHAMAN_OVERLOAD_COPIES {
            let parent_effect = &row(Rules::Forever, parent).effects[0];
            let copy_effect = effect_of(copy, effect_id);
            assert_eq!(
                copy_effect.base_points,
                parent_effect.base_points / 2.0,
                "{parent}"
            );
            assert!(
                (copy_effect.ppl - parent_effect.ppl / 2.0).abs() < 1e-6,
                "{parent}"
            );
            assert!(
                (copy_effect.average(60) - parent_effect.average(60) / 2.0).abs() <= 1.0,
                "{parent}"
            );
            assert!(
                (copy_effect.sp_coef - parent_effect.sp_coef / 2.0).abs() < 1e-3,
                "{parent}"
            );
        }
        assert_eq!(effect_of(408477, 1072630).average(60), 180.0);
        assert_eq!(effect_of(408484, 1072638).average(60), 261.0);

        // The shocks' sums over 12 sec: Flame Shock rank 6's 320 is 80 on each of four ticks.
        assert_eq!(effect_of(29228, 704946).base_points * 4.0, 320.0);
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
