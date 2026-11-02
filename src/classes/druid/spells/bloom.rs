//! Bloom (1328621, hotfix push 113531, Forever's rules), a Restoration talent. The client text:
//! "When your Rejuvenation or Regrowth spell expires due to its duration, the target is instantly
//! healed for an amount equal to 100% of your Spirit. If either spell expires due to being
//! dispelled, the target is healed for 50% of your Spirit instead."
//!
//! The client rows (the push's, from upstream wowsims/forever's regenerated store) are two dummy
//! effects of 100 and 50 and no triggered heal spell, so the heal is a server side script and the
//! data states no coefficient, no crit and no modifiers. It is modelled as the text says and
//! nothing more: a flat heal of the druid's current Spirit, which nothing modifies and nothing
//! crits. The sim has no dispels, so the 50% case never happens.
//!
//! A heal over time that ends by its duration blooms once. The sim keeps a Rejuvenation or
//! Regrowth cast again while it runs ticking on its first player (`core::fight::heals`), so a
//! recast blooms the one it leaves, counted when the new one goes up like its ticks left, and
//! the new one blooms when it ends. One cut short blooms nothing: Swiftmend's consume, or the
//! fight ending.

use crate::core::fight::{heals::HealBook, DotId, Fight, SpellId};

use super::super::agent::DruidAgent;

/// The talent, bound to its heal spell and the ranks of the two heals over time.
#[derive(Clone, Debug)]
pub(crate) struct Bloom {
    spell: SpellId,
    /// The prepared Spirit, which a stat aura can change.
    spirit: f64,
    /// The Rejuvenation and Regrowth spells, by client id.
    hots: Vec<i32>,
}

impl Bloom {
    pub(crate) fn bind(
        fight: &Fight<DruidAgent>,
        spell_id: i32,
        spirit: f64,
        hots: &[i32],
    ) -> Result<Self, String> {
        let spell = fight
            .spells
            .iter()
            .position(|spell| spell.id.spell_id == spell_id && spell.id.tag == 0)
            .ok_or_else(|| format!("Bloom {spell_id} has no spell"))?;
        Ok(Bloom {
            spell,
            spirit,
            hots: hots.to_vec(),
        })
    }

    /// Whether the heal over time on `dot` is one Bloom follows.
    fn follows(&self, fight: &Fight<DruidAgent>, dot: DotId) -> bool {
        let id = fight.spells[fight.dots[dot].spell].id.spell_id;
        self.hots.contains(&id)
    }

    /// The heal: 100% of the druid's Spirit now.
    fn bloom(&self, fight: &mut Fight<DruidAgent>) {
        let spirit = fight.player_spirit(self.spirit);
        fight.deal_flat_heal(self.spell, spirit);
    }

    /// A heal over time expiring: it blooms when its duration ran out.
    pub(crate) fn on_dot_expire(&self, fight: &mut Fight<DruidAgent>, dot: DotId) {
        if self.follows(fight, dot) && fight.fading_by_duration(fight.dots[dot].aura) {
            self.bloom(fight);
        }
    }

    /// A cast of heal rank `rank` over its own heal over time still running: the one running
    /// goes on healing its player to its end, where it blooms.
    pub(crate) fn on_recast(&self, fight: &mut Fight<DruidAgent>, book: &HealBook, rank: usize) {
        let Some(dot) = book.heals[rank].dot else {
            return;
        };
        if self.follows(fight, dot) && fight.aura(fight.dots[dot].aura).active {
            self.bloom(fight);
        }
    }
}
