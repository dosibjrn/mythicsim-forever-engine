//! Litany of Light (1317006) in MythicSim's healer sims under Forever's rules: casting a heal
//! when the heal before it was a different spell restores 5% of the new spell's base mana cost
//! at rank 1 and 10% at rank 2. A rank of the same spell is the same spell, so Greater Heal after
//! Heal is a different one and Greater Heal after Greater Heal is not. The first heal of a fight
//! follows no heal, which reads as a different one.
//!
//! The mana returns as the cast completes, under the talent's own resource metrics.

use crate::contracts::prepared_v2::LitanyOfLight as Spec;
use crate::core::fight::{Fight, ResourceKind, SpellId};

use super::super::agent::PriestAgent;

/// The talent bound to the fight's spells.
#[derive(Clone, Debug)]
pub(crate) struct LitanyOfLight {
    share: f64,
    /// By spell: the spell its heal ranks are, as a position among the families.
    family: Vec<Option<usize>>,
    metrics: usize,
}

/// Binds the families of heals to the fight's spells.
pub(crate) fn bind(fight: &mut Fight<PriestAgent>, spec: &Spec) -> Result<LitanyOfLight, String> {
    let mut family = vec![None; fight.spells.len()];
    for (index, ranks) in spec.families.iter().enumerate() {
        for &id in ranks {
            let spell = fight
                .spells
                .iter()
                .position(|state| state.id.spell_id == id && state.id.tag == 0)
                .ok_or_else(|| format!("Litany of Light names spell {id}, no spell"))?;
            family[spell] = Some(index);
        }
    }
    let metrics = fight.new_resource_metrics(spec.metrics_action_id.clone(), ResourceKind::Mana);
    Ok(LitanyOfLight {
        share: spec.share,
        family,
        metrics,
    })
}

impl LitanyOfLight {
    /// A cast of `spell` completing: the mana it returns, when it is a heal that follows a
    /// different one. Every heal is now the one before the next.
    pub(crate) fn on_cast(&self, fight: &mut Fight<PriestAgent>, spell: SpellId) {
        let Some(family) = self.family.get(spell).copied().flatten() else {
            return;
        };
        let before = fight.agent.last_heal.replace(family);
        if before == Some(family) || !fight.has_mana_bar() {
            return;
        }
        if let Some(cost) = &fight.spells[spell].cost {
            let mana = self.share * f64::from(cost.base);
            if mana > 0.0 {
                fight.add_mana(mana, self.metrics);
            }
        }
    }
}
