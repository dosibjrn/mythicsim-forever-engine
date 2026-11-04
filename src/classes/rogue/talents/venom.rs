//! Venom (1310703), from Go sim/rogue/talents_assassination.go `registerVenom`: a finisher
//! whose metrics split by the combo points spent. It applies the finisher, then restarts its
//! aura for the Slice and Dice duration of the points spent. While the aura lasts, an attached
//! modifier raises poison damage and the poisons' chance rises by a running total Go keeps.

use crate::core::fight::{Agent, AuraRef, Fight, ModId, ModKind, SpellId};

use crate::classes::rogue::spells::finisher::Finisher;

#[derive(Clone, Debug)]
pub(crate) struct Venom {
    pub(crate) aura: AuraRef,
    durations: Vec<i64>,
    pub(crate) chance_bonus: f64,
    damage_mod: ModId,
}

pub(crate) fn bind<A: Agent>(
    fight: &mut Fight<A>,
    aura: &str,
    durations: &[i64],
    damage_bonus: f64,
    chance_bonus: f64,
    class_spells: &[String],
) -> Result<Venom, String> {
    let aura = fight.player_aura(aura)?;
    let names: Vec<&str> = class_spells.iter().map(String::as_str).collect();
    let damage_mod = fight.register_mod(
        ModKind::DamageDoneFlat,
        damage_bonus,
        0,
        fight.spells_with_class(&names),
    );
    Ok(Venom {
        aura,
        durations: durations.to_vec(),
        chance_bonus,
        damage_mod,
    })
}

impl Venom {
    /// Venom's effect; Stealth has already broken.
    pub(crate) fn apply<A: Agent>(
        &self,
        fight: &mut Fight<A>,
        spell: SpellId,
        finisher: &Finisher,
    ) {
        let points = fight.energy_bar().combo_points as usize;
        finisher.apply(fight, spell);
        fight.deactivate_aura(self.aura);
        fight.aura_mut(self.aura).duration = self.durations[points];
        fight.activate_aura(self.aura);
    }

    /// Forever's rules: Shank puts the aura up for its fixed duration (the first of the ladder,
    /// every entry the same), refreshing it when it is already up.
    pub(crate) fn restart<A: Agent>(&self, fight: &mut Fight<A>) {
        fight.deactivate_aura(self.aura);
        fight.aura_mut(self.aura).duration = self.durations[0];
        fight.activate_aura(self.aura);
    }

    /// The attached modifier, after the aura's own OnGain.
    pub(crate) fn on_gain<A: Agent>(&self, fight: &mut Fight<A>) {
        fight.activate_mod(self.damage_mod);
    }

    pub(crate) fn on_expire<A: Agent>(&self, fight: &mut Fight<A>) {
        fight.deactivate_mod(self.damage_mod);
    }
}
