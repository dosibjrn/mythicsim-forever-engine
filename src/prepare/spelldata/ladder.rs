//! Go `spelldata/ladder.go`: a spell's ranks in rank order, whether the client states them as one
//! spell per rank or as one spell with a curve.
//!
//! Ranks are the store's own rows, or, for a trait talent, copies kept for the process: Go hands
//! out pointers to them, so a rank is a `&'static Spell` here and never written through.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::book_ranks;
use super::store::must_find;
use super::{nil, nil_effect, Effect, Spell};
use crate::data::spells as rows;
use crate::prepare::sim::Rules;

/// The kept ranks of each talent ladder, by spell id and rank count.
type TalentRanks = HashMap<(i32, i32), Vec<&'static Spell>>;

/// A spell's ranks in rank order, whether the client states them as one spell per rank or as
/// one spell with a curve. Rank 0 is untaken, so the readers below answer 0 there instead of
/// panicking.
#[derive(Clone, Debug, Default)]
pub(crate) struct Ladder {
    ranks: Vec<&'static Spell>,
}

/// Which effect of a rank a [`LadderEffect`] reads (Go's `pick` closure).
#[derive(Clone, Copy, Debug)]
enum Pick {
    /// The rank's only effect, which panics when it has several.
    Only,
    /// The effect at a position counted from 1.
    Position(i32),
    /// The effect with this aura and misc value.
    Aura(i32, i32),
}

/// One named effect across the ladder's ranks.
#[derive(Clone, Debug)]
pub(crate) struct LadderEffect {
    ladder: Ladder,
    pick: Pick,
}

impl Ladder {
    /// A ladder of one spell per rank, lowest first. Every id has to be in the store: a rank
    /// named by typo has to fail loudly rather than register nothing. Go `Ranked`.
    pub(crate) fn ranked(ids: &[i32]) -> Ladder {
        Ladder {
            ranks: ids.iter().map(|&id| must_find(id)).collect(),
        }
    }

    /// A ladder of a trait-tree talent, which is one spell whose per-rank numbers live in a
    /// curve. Each rank is a copy of the spell with the curve's value on the effects the curve
    /// covers; an effect the curve has no row for keeps the spell's own base value. Go `Talent`.
    pub(crate) fn talent(spell_id: i32, max_ranks: i32) -> Ladder {
        // The copies are built once per talent and kept, so a rank is a stable pointer as it is
        // in Go. The lock is not held while building, since a building panic would poison it.
        static CACHE: OnceLock<Mutex<TalentRanks>> = OnceLock::new();
        let cache = CACHE.get_or_init(Default::default);
        let key = (spell_id, max_ranks);
        let cached = cache
            .lock()
            .expect("talent ladder cache")
            .get(&key)
            .cloned();
        if let Some(ranks) = cached {
            return Ladder { ranks };
        }

        let copies = talent_ranks(
            must_find(spell_id),
            rows::curve(spell_id).unwrap_or(&[]),
            max_ranks,
        );
        let ranks = Ladder::keep(copies).ranks;
        cache
            .lock()
            .expect("talent ladder cache")
            .insert(key, ranks.clone());
        Ladder { ranks }
    }

    /// A ladder of a trait-tree talent over a curve stated by the caller instead of the imported
    /// data's: a client build after the frozen data that changed the talent's rank count. The
    /// ranks are the curve rows' length. Not cached; the caller builds it once and keeps it.
    pub(crate) fn talent_with_curve(spell_id: i32, curve: &[Vec<f64>]) -> Ladder {
        let max_ranks = curve.first().map_or(0, |row| row.len() as i32);
        Ladder::keep(talent_ranks(must_find(spell_id), curve, max_ranks))
    }

    /// A ladder over rank copies, kept for the process.
    fn keep(copies: Vec<Spell>) -> Ladder {
        Ladder {
            ranks: copies
                .into_iter()
                .map(|rank| &*Box::leak(Box::new(rank)))
                .collect(),
        }
    }

    /// The spell at a rank. Rank 0 is untaken and answers Nil, as does a rank the ladder does
    /// not have.
    pub(crate) fn rank(&self, n: i32) -> &'static Spell {
        if n <= 0 || n > self.len() {
            return nil();
        }
        self.ranks[(n - 1) as usize]
    }

    pub(crate) fn highest(&self) -> &'static Spell {
        match self.ranks.last() {
            Some(rank) => rank,
            None => nil(),
        }
    }

    /// The highest rank a player knows under these rules: under Forever's, a rank taught by an
    /// Ahn'Qiraj 20 book is not known (see [`super::book_ranks`]).
    pub(crate) fn highest_for(&self, rules: Rules) -> &'static Spell {
        match self
            .ranks
            .iter()
            .rev()
            .find(|spell| rules != Rules::Forever || !book_ranks::is_book_rank(spell.id))
        {
            Some(rank) => rank,
            None => nil(),
        }
    }

    /// The rank with this spell id. Panics on an id the ladder does not carry, the way
    /// `must_find` does on a spell the store does not carry.
    pub(crate) fn by_id(&self, id: i32) -> &'static Spell {
        match self.ranks.iter().find(|spell| spell.id == id) {
            Some(spell) => spell,
            None => panic!(
                "spelldata: spell {id} is not one of the {} ranks of this ladder",
                self.ranks.len()
            ),
        }
    }

    pub(crate) fn len(&self) -> i32 {
        self.ranks.len() as i32
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.ranks.is_empty()
    }

    /// Calls `f` with each rank, counted from 1.
    pub(crate) fn each(&self, mut f: impl FnMut(i32, &'static Spell)) {
        for (i, spell) in self.ranks.iter().enumerate() {
            f((i + 1) as i32, spell);
        }
    }

    /// The rank's only effect, in the client's units. Rank 0 is untaken and answers 0.
    pub(crate) fn value_at(&self, rank: i32) -> f64 {
        self.only().value_at(rank)
    }

    /// The client states a percentage as an integer: 16, not 0.16.
    pub(crate) fn fraction_at(&self, rank: i32) -> f64 {
        self.only().fraction_at(rank)
    }

    /// The sign comes from the data: Improved Righteous Fury states -2/-4/-6, so rank 3 gives
    /// 0.94.
    pub(crate) fn multiplier_at(&self, rank: i32) -> f64 {
        self.only().multiplier_at(rank)
    }

    /// The client states rage on a 0-1000 bar.
    pub(crate) fn tenths_at(&self, rank: i32) -> f64 {
        self.only().tenths_at(rank)
    }

    /// The effect at a position, counted from 1, for the ranks that carry more than one.
    pub(crate) fn effect_at(&self, index: i32) -> LadderEffect {
        LadderEffect {
            ladder: self.clone(),
            pick: Pick::Position(index),
        }
    }

    /// The effect with this aura and misc value, which panics when the rank has two of them.
    pub(crate) fn effect(&self, aura: i32, misc: i32) -> LadderEffect {
        LadderEffect {
            ladder: self.clone(),
            pick: Pick::Aura(aura, misc),
        }
    }

    /// Nothing named means nothing to choose between: reading the first of several silently is
    /// the bug this shape exists to prevent.
    fn only(&self) -> LadderEffect {
        LadderEffect {
            ladder: self.clone(),
            pick: Pick::Only,
        }
    }
}

impl LadderEffect {
    pub(crate) fn value_at(&self, rank: i32) -> f64 {
        self.at(rank).base_points
    }

    pub(crate) fn fraction_at(&self, rank: i32) -> f64 {
        self.at(rank).percent()
    }

    pub(crate) fn multiplier_at(&self, rank: i32) -> f64 {
        1.0 + self.fraction_at(rank)
    }

    pub(crate) fn tenths_at(&self, rank: i32) -> f64 {
        self.at(rank).tenths()
    }

    /// The effect at a rank. Rank 0 is untaken and answers [`nil_effect`], which reads as zero.
    pub(crate) fn at(&self, rank: i32) -> &'static Effect {
        if rank <= 0 {
            return nil_effect();
        }
        if rank > self.ladder.len() {
            panic!("rank {rank} in a ladder of {} ranks", self.ladder.len());
        }
        let spell = self.ladder.rank(rank);
        match self.pick {
            Pick::Only => {
                if spell.effects.len() != 1 {
                    panic!(
                        "spell {} rank {rank} has {} effects - name the one you mean with Effect(aura, misc)",
                        spell.id,
                        spell.effects.len()
                    );
                }
                &spell.effects[0]
            }
            Pick::Position(index) => {
                let e = spell.effect_n(index);
                if e.is_nil() {
                    panic!(
                        "spell {} has no effect at position {index}, in {} effects",
                        spell.id,
                        spell.effects.len()
                    );
                }
                e
            }
            Pick::Aura(aura, misc) => spell.effect(aura, misc),
        }
    }
}

/// The ranks of a talent over a base row and its curve: each a copy of the row with the curve's
/// value for that rank on the effects the curve covers.
fn talent_ranks(base: &Spell, curve: &[Vec<f64>], max_ranks: i32) -> Vec<Spell> {
    // A curve row states one value per rank. A row of another length means the curve and the
    // talent disagree about how many ranks there are, and the ranks past the shorter of the two
    // would silently keep the base value.
    for (i, row) in curve.iter().enumerate() {
        if row.len() as i32 != max_ranks {
            panic!(
                "spelldata: spell {} curve row {i} has {} ranks, want {max_ranks}",
                base.id,
                row.len()
            );
        }
    }

    (1..=max_ranks)
        .map(|n| {
            // Only the effects differ by rank in base points; the powers, the labels and the
            // tooltip references are the same whatever the rank.
            let mut rank = base.clone();
            for (i, effect) in rank.effects.iter_mut().enumerate() {
                if i < curve.len() {
                    effect.base_points = curve[i][(n - 1) as usize];
                }
            }
            rank
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepare::{dbcenums, spelldata::store::must_find};

    /// Go `TestRankedLadder`, and `ExampleLadder` on the real Frostbolt ranks.
    #[test]
    fn a_ranked_ladder_answers_nil_off_its_ends() {
        let ladder = Ladder::ranked(&[116]);
        assert_eq!(ladder.highest().id, 116);
        assert!(ladder.rank(0).is_nil());
        assert!(ladder.rank(2).is_nil());
        assert_eq!(ladder.len(), 1);
        assert_eq!(ladder.effect_at(2).value_at(1), 19.0);
        assert_eq!(ladder.value_at(0), 0.0);

        let frostbolt = Ladder::ranked(&[116, 205, 837]);
        assert_eq!(frostbolt.len(), 3);
        assert_eq!(frostbolt.rank(1).rank, "Rank 1");
        assert_eq!(frostbolt.highest().rank, "Rank 3");
        assert!(frostbolt.rank(0).is_nil());
        assert_eq!(frostbolt.by_id(205).rank, "Rank 2");

        let mut seen = Vec::new();
        frostbolt.each(|rank, spell| seen.push((rank, spell.id)));
        assert_eq!(seen, [(1, 116), (2, 205), (3, 837)]);
        assert!(Ladder::default().highest().is_nil());
    }

    #[test]
    #[should_panic(expected = "is not one of the 1 ranks of this ladder")]
    fn by_id_panics_on_a_stranger() {
        Ladder::ranked(&[116]).by_id(205);
    }

    #[test]
    #[should_panic(expected = "is not in the store")]
    fn ranked_panics_on_a_typo() {
        Ladder::ranked(&[116, 1]);
    }

    /// Go `TestEffectPanics`'s last case: two effects and none of them named, the ladder refuses
    /// to pick.
    #[test]
    #[should_panic(expected = "name the one you mean")]
    fn a_ladder_with_several_effects_will_not_pick() {
        Ladder::ranked(&[116]).value_at(1);
    }

    #[test]
    #[should_panic(expected = "rank 2 in a ladder of 1 ranks")]
    fn a_rank_past_the_end_panics_when_read() {
        Ladder::ranked(&[116]).effect_at(1).value_at(2);
    }

    #[test]
    #[should_panic(expected = "has no effect at position 3")]
    fn an_effect_past_the_end_panics_when_read() {
        Ladder::ranked(&[116]).effect_at(3).value_at(1);
    }

    /// Go `TestGeneratedTalentCurve`: Flurry is a trait talent, one spell, five ranks, and the
    /// per-rank numbers on a curve.
    #[test]
    fn a_talent_ladder_carries_the_curves_values() {
        let curve = rows::curve(12319).expect("Flurry has a curve");
        assert_eq!(curve.len(), 1);
        assert_eq!(curve[0].len(), 5);

        let ladder = Ladder::talent(12319, 5);
        assert_eq!(ladder.len(), 5);
        assert_eq!(ladder.rank(3).effect_n(1).base_value(), curve[0][2]);
        assert!(ladder.rank(0).is_nil());
        assert_eq!(ladder.effect_at(1).value_at(5), curve[0][4]);
        assert_eq!(ladder.value_at(2), curve[0][1]);
        // The store's own row keeps the client's value.
        assert_ne!(must_find(12319).effect_n(1).base_value(), curve[0][4]);
        // A second ask answers the same ranks.
        let again = Ladder::talent(12319, 5);
        assert!(std::ptr::eq(ladder.rank(3), again.rank(3)));
    }

    /// Go `TestTalentLadder`: each rank of a trait talent is a copy carrying the curve's value.
    #[test]
    fn each_rank_is_a_copy_with_its_curve_value() {
        let curve = vec![vec![-40.0, -45.0, -50.0], vec![19.0, 20.0, 21.0]];
        let ladder = Ladder::keep(talent_ranks(must_find(116), &curve, 3));
        assert_eq!(ladder.rank(2).effect_n(2).base_value(), 20.0);
        assert_eq!(ladder.effect_at(1).value_at(3), -50.0);
        assert_eq!(must_find(116).effect_n(2).base_value(), 19.0);
    }

    /// Go `TestTalentLadderWithoutCurve`: an effect the curve has no row for keeps the spell's
    /// base value.
    #[test]
    fn an_effect_without_a_curve_row_keeps_its_base_value() {
        let ladder = Ladder::keep(talent_ranks(must_find(116), &[], 2));
        assert_eq!(ladder.rank(2).effect_n(2).base_value(), 19.0);
        let partial = Ladder::keep(talent_ranks(must_find(116), &[vec![-1.0, -2.0]], 2));
        assert_eq!(partial.rank(2).effect_n(1).base_value(), -2.0);
        assert_eq!(partial.rank(2).effect_n(2).base_value(), 19.0);
    }

    /// Go `TestTalentCurveLengthPanics`.
    #[test]
    #[should_panic(expected = "spelldata: spell 116 curve row 1 has 2 ranks, want 3")]
    fn a_curve_that_does_not_state_every_rank_panics() {
        let curve = vec![vec![-40.0, -45.0, -50.0], vec![19.0, 20.0]];
        Ladder::keep(talent_ranks(must_find(116), &curve, 3));
    }

    #[test]
    fn units_follow_the_effect() {
        // Frostbolt's slow effect, read through its aura.
        let ladder = Ladder::ranked(&[116]);
        let slow = ladder.effect(dbcenums::A_MOD_DECREASE_SPEED, 0);
        let want = must_find(116)
            .effect(dbcenums::A_MOD_DECREASE_SPEED, 0)
            .base_points;
        assert_eq!(slow.value_at(1), want);
        assert_eq!(slow.fraction_at(1), want / 100.0);
        assert_eq!(slow.multiplier_at(1), 1.0 + want / 100.0);
        assert_eq!(slow.tenths_at(1), want / 10.0);
        assert_eq!(slow.value_at(0), 0.0);
    }
}
