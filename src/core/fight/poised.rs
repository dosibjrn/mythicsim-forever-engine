//! Poised, from the development notes of 2 November 2026 (Forever's rules): out of combat at
//! maximum Energy a rogue, and a druid in Cat Form, gains Poised, which takes 10 Energy off the
//! next Energy ability. The fight starts out of combat at maximum Energy, so the aura is up at
//! the reset and the first Energy ability that applies its effects spends it, through a cost
//! modifier the aura holds. Both classes bind it with the names of their Energy abilities.

use crate::core::fight::{Agent, AuraRef, Fight, ModId, ModKind, SpellId};

#[derive(Clone, Debug)]
pub(crate) struct Poised {
    pub(crate) aura: AuraRef,
    cost_mod: ModId,
    spends: Vec<bool>,
}

pub(crate) fn bind<A: Agent>(
    fight: &mut Fight<A>,
    aura: &str,
    cost_reduction: i32,
    class_spells: &[String],
) -> Result<Poised, String> {
    let aura = fight.player_aura(aura)?;
    let names: Vec<&str> = class_spells.iter().map(String::as_str).collect();
    let cost_mod = fight.register_mod(
        ModKind::PowerCostFlat,
        -f64::from(cost_reduction),
        0,
        fight.spells_with_class(&names),
    );
    let spends = fight
        .spells
        .iter()
        .map(|spell| {
            spell
                .class_spell
                .as_deref()
                .is_some_and(|class| names.contains(&class))
        })
        .collect();
    Ok(Poised {
        aura,
        cost_mod,
        spends,
    })
}

impl Poised {
    pub(crate) fn on_gain<A: Agent>(&self, fight: &mut Fight<A>) {
        fight.activate_mod(self.cost_mod);
    }

    pub(crate) fn on_expire<A: Agent>(&self, fight: &mut Fight<A>) {
        fight.deactivate_mod(self.cost_mod);
    }

    /// The aura's `OnApplyEffects`: the first Energy ability spends it.
    pub(crate) fn on_apply_effects<A: Agent>(&self, fight: &mut Fight<A>, spell: SpellId) {
        if self.spends[spell] {
            fight.deactivate_aura(self.aura);
        }
    }

    /// The fight's reset: out of combat at maximum Energy.
    pub(crate) fn on_reset<A: Agent>(&self, fight: &mut Fight<A>) {
        fight.activate_aura(self.aura);
    }
}
