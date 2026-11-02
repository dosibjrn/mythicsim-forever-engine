//! Renewed Hope (425280) in MythicSim's healer sims under Forever's rules: Power Word: Shield
//! leaves Weakened Soul (6788, 15 seconds) on a member of the party, and a Lesser Heal, Heal,
//! Greater Heal, Flash Heal, Binding Heal or Penance on a member who has it gains crit chance and
//! takes seconds off the debuff, 2% and 1 second a point.
//!
//! The party is the one in `prepare::party_damage`: a heal goes to a member at random, in
//! proportion to how often that member is hit, as the heal goes to who needs it. A shield goes
//! to the first member in the party's order, the tank first, who has no Weakened Soul, or to
//! the one whose debuff ends first when all have it. Penance's channel stays on the member its
//! cast chose, and each of its bolts reads the debuff again.

use crate::contracts::prepared_v2::RenewedHope as Spec;
use crate::core::fight::{Fight, SpellId};

use super::super::agent::PriestAgent;

/// The talent bound to the fight's spells.
#[derive(Clone, Debug)]
pub(crate) struct RenewedHope {
    /// By spell: a heal that gains the crit and cuts the debuff.
    heals: Vec<bool>,
    /// By spell: a shield that leaves Weakened Soul.
    shields: Vec<bool>,
    /// The crit a heal gains, as a fraction.
    crit: f64,
    cut: i64,
    duration: i64,
    hit_rates: Vec<f64>,
}

/// Who has Weakened Soul now: when it ends on each member, and the member Penance's channel
/// is on.
#[derive(Clone, Debug, Default)]
pub(crate) struct Souls {
    until: Vec<i64>,
    channel: Option<usize>,
}

impl Souls {
    pub(crate) fn clear(&mut self) {
        self.until.iter_mut().for_each(|until| *until = 0);
        self.channel = None;
    }

    /// A shield at `now` on the first member without Weakened Soul, else the one whose debuff
    /// ends first: the debuff is up for `duration` from now.
    fn shield(&mut self, now: i64, duration: i64) -> usize {
        let member = self
            .until
            .iter()
            .position(|&until| until <= now)
            .unwrap_or_else(|| {
                self.until
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, until)| **until)
                    .map_or(0, |(member, _)| member)
            });
        self.until[member] = now + duration;
        member
    }

    /// A heal at `now` on `member`: whether the member has Weakened Soul, which the heal then
    /// cuts by `cut`, to nothing at the least.
    fn read(&mut self, member: usize, now: i64, cut: i64) -> bool {
        let until = &mut self.until[member];
        if *until <= now {
            return false;
        }
        *until = (*until - cut).max(now);
        true
    }
}

/// Binds the talent's heals and shields to the fight's spells, and the party to its souls.
pub(crate) fn bind(
    fight: &mut Fight<PriestAgent>,
    spec: &Spec,
) -> Result<(RenewedHope, Souls), String> {
    if spec.hit_rates.len() < 2
        || spec
            .hit_rates
            .iter()
            .any(|rate| rate.is_nan() || *rate <= 0.0)
    {
        return Err(format!("party hit rates {:?}", spec.hit_rates));
    }
    let mark = |fight: &Fight<PriestAgent>, ids: &[i32]| -> Result<Vec<bool>, String> {
        let mut marked = vec![false; fight.spells.len()];
        for &id in ids {
            let spell = fight
                .spells
                .iter()
                .position(|state| state.id.spell_id == id && state.id.tag == 0)
                .ok_or_else(|| format!("Renewed Hope names spell {id}, no spell"))?;
            marked[spell] = true;
        }
        Ok(marked)
    };
    let talent = RenewedHope {
        heals: mark(fight, &spec.heal_spell_ids)?,
        shields: mark(fight, &spec.shield_spell_ids)?,
        crit: spec.crit_percent / 100.0,
        cut: spec.cut_ns,
        duration: spec.duration_ns,
        hit_rates: spec.hit_rates.clone(),
    };
    let souls = Souls {
        until: vec![0; spec.hit_rates.len()],
        channel: None,
    };
    Ok((talent, souls))
}

impl RenewedHope {
    /// A cast of `spell`: the crit chance, as a fraction, its first heal gains. A shield
    /// leaves Weakened Soul and gains nothing.
    pub(crate) fn on_cast(&self, fight: &mut Fight<PriestAgent>, spell: SpellId) -> f64 {
        if self.shields.get(spell).copied().unwrap_or(false) {
            self.shield(fight);
            return 0.0;
        }
        if !self.heals.get(spell).copied().unwrap_or(false) {
            return 0.0;
        }
        let member = self.needy_member(fight);
        fight.agent.souls.channel = Some(member);
        self.read(fight, member)
    }

    /// A bolt of a channelled heal: what it gains on the member the channel is on.
    pub(crate) fn on_tick(&self, fight: &mut Fight<PriestAgent>, spell: SpellId) -> f64 {
        if !self.heals.get(spell).copied().unwrap_or(false) {
            return 0.0;
        }
        match fight.agent.souls.channel {
            Some(member) => self.read(fight, member),
            None => 0.0,
        }
    }

    /// The heal on `member`: when the member has Weakened Soul, the crit, and the debuff is cut.
    fn read(&self, fight: &mut Fight<PriestAgent>, member: usize) -> f64 {
        let now = fight.now;
        if fight.agent.souls.read(member, now, self.cut) {
            self.crit
        } else {
            0.0
        }
    }

    /// A shield on a member of the party.
    fn shield(&self, fight: &mut Fight<PriestAgent>) {
        let now = fight.now;
        fight.agent.souls.shield(now, self.duration);
    }

    /// The member who needs the next heal, at random in proportion to how often each is hit.
    fn needy_member(&self, fight: &mut Fight<PriestAgent>) -> usize {
        let total: f64 = self.hit_rates.iter().sum();
        let mut roll = fight.random("Renewed Hope Target") * total;
        for (member, rate) in self.hit_rates.iter().enumerate() {
            if roll < *rate {
                return member;
            }
            roll -= rate;
        }
        self.hit_rates.len() - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: i64 = 1_000_000_000;

    fn party() -> Souls {
        Souls {
            until: vec![0; 5],
            channel: None,
        }
    }

    #[test]
    fn shields_go_to_the_first_member_without_weakened_soul() {
        let mut souls = party();
        let members: Vec<usize> = (0..5)
            .map(|n| souls.shield(n * 4 * SECOND, 15 * SECOND))
            .collect();
        // Shields four seconds apart, a debuff of fifteen: the tank again once its own ends.
        assert_eq!(members, [0, 1, 2, 3, 0]);
    }

    #[test]
    fn a_shield_with_every_member_weakened_takes_the_one_ending_first() {
        let mut souls = party();
        souls.until = vec![9 * SECOND, 5 * SECOND, 7 * SECOND, 8 * SECOND, 6 * SECOND];
        assert_eq!(souls.shield(SECOND, 15 * SECOND), 1);
        assert_eq!(souls.until[1], 16 * SECOND);
    }

    #[test]
    fn a_heal_reads_weakened_soul_and_cuts_it() {
        let mut souls = party();
        souls.shield(0, 15 * SECOND);
        assert!(
            !souls.read(1, SECOND, 3 * SECOND),
            "the second member has none"
        );
        assert!(souls.read(0, SECOND, 3 * SECOND));
        assert_eq!(souls.until[0], 12 * SECOND);
        // Cut to nothing at the least: the debuff is gone and the next heal reads none.
        assert!(souls.read(0, 11 * SECOND, 3 * SECOND));
        assert_eq!(souls.until[0], 11 * SECOND);
        assert!(!souls.read(0, 11 * SECOND, 3 * SECOND));
    }
}
