//! The on-use trinkets of MythicSim's healer sims under Forever's rules (`prepare::healer_gear`):
//! a window aura that moves a stacking stats aura's stacks while it is up
//! ([`Effect::StackingCdOnUse`](crate::contracts::prepared_v2::Effect)), and an aura that
//! shields the player each direct heal lands on ([`Effect::ShieldOnHeal`](crate::contracts::prepared_v2::Effect)).

use super::{Agent, AuraRef, Fight, SpellId, SpellResult};

/// A stacking on-use trinket's window and the stats aura it drives.
#[derive(Clone, Debug)]
pub(crate) struct StackingCd {
    pub(crate) stat_aura: AuraRef,
    pub(crate) decay: bool,
    /// The chance each spell it hears rolls, by spellbook position.
    pub(crate) chances: Vec<Option<f64>>,
    pub(crate) hits: bool,
    pub(crate) heals: bool,
    pub(crate) casts: bool,
    pub(crate) landed_only: bool,
    /// A hit that dealt no damage is not heard.
    pub(crate) require_damage: bool,
    /// The label the chance rolls under: Go's trigger name, else the window's.
    pub(crate) rng_label: String,
}

/// An aura that shields the player for a share of each direct heal while it is up.
#[derive(Clone, Debug)]
pub(crate) struct ShieldOnHeal {
    pub(crate) share: f64,
    /// The item's spell, which the shields count on.
    pub(crate) spell: SpellId,
    pub(crate) chance: f64,
    /// Heals the healer instead of shielding.
    pub(crate) heal: bool,
    pub(crate) label: String,
}

/// Which of a window's events a callback is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StackEvent {
    Hit,
    Heal,
    Cast,
}

impl<A: Agent> Fight<A> {
    /// The window going up: the stats aura with it, at its most stacks when they decay.
    pub(crate) fn stacking_cd_gained(&mut self, index: usize) {
        let state = &self.stacking_cds[index];
        let (stat, decay) = (state.stat_aura, state.decay);
        self.activate_aura(stat);
        if decay {
            let max = self.aura(stat).max_stacks;
            self.set_stacks(stat, max);
        }
    }

    /// The window's end takes the stats aura down.
    pub(crate) fn stacking_cd_expired(&mut self, index: usize) {
        let stat = self.stacking_cds[index].stat_aura;
        self.deactivate_aura(stat);
    }

    /// An event the window hears: a spell it names rolls its chance, and the stacks move a
    /// spell batch window later.
    pub(crate) fn stacking_cd_callback(
        &mut self,
        aura: AuraRef,
        index: usize,
        spell: SpellId,
        event: StackEvent,
        result: Option<&SpellResult>,
    ) {
        if !self.aura(aura).active {
            return;
        }
        let state = &self.stacking_cds[index];
        let heard = match event {
            StackEvent::Hit => state.hits,
            StackEvent::Heal => state.heals,
            StackEvent::Cast => state.casts,
        };
        if !heard
            || (state.landed_only && result.is_some_and(|result| !result.landed()))
            || (state.require_damage && result.is_some_and(|result| result.damage == 0.0))
        {
            return;
        }
        let Some(chance) = state.chances.get(spell).copied().flatten() else {
            return;
        };
        // Go `Proc(chance, label)`, reading the label in place.
        if !self.rng.proc(chance, &self.stacking_cds[index].rng_label) {
            return;
        }
        let result = result.copied().unwrap_or(SpellResult {
            armor_multiplier: 0.0,
            target: super::Side::Player,
            attacker: super::Side::Player,
            outcome: 0,
            damage: 0.0,
            threat: 0.0,
        });
        self.schedule_delayed_proc(aura, spell, result);
    }

    /// The handler: a stack more, or one fewer when they decay, while the stats aura is up.
    pub(crate) fn stacking_cd_handler(&mut self, index: usize) {
        let state = &self.stacking_cds[index];
        let (stat, decay) = (state.stat_aura, state.decay);
        if !self.aura(stat).active {
            return;
        }
        if decay {
            self.remove_stack(stat);
        } else {
            self.add_stack(stat);
        }
    }

    /// A direct heal while the aura is up shields the player for its share of the healing.
    pub(crate) fn shield_on_heal(&mut self, index: usize, spell: SpellId, result: &SpellResult) {
        let state = &self.shields_on_heal[index];
        if spell == state.spell || result.damage <= 0.0 {
            return;
        }
        let (target, amount, heal) = (state.spell, state.share * result.damage, state.heal);
        // Go `Proc(chance, label)`, reading the label in place.
        if !self
            .rng
            .proc(state.chance, &self.shields_on_heal[index].label)
        {
            return;
        }
        if heal {
            self.deal_flat_healing(target, amount);
        } else {
            self.deal_shield(target, amount);
        }
    }

    /// Healing of a fixed amount on the player, counted on `spell`: no coefficient, multiplier
    /// or crit, less the spell's overhealing share.
    pub(crate) fn deal_flat_healing(&mut self, spell: SpellId, amount: f64) {
        let index = super::Side::Player.index();
        let (amount, _) = self.landed_healing(spell, index, amount);
        let metrics = &mut self.spells[spell].metrics[index];
        metrics.hits += 1;
        metrics.total_healing += amount;
        self.note_burst(amount);
        if self.log.is_some() {
            let line = format!(
                "[{}] {} Hit for {amount:.3} healing.",
                self.config.player_label,
                super::log::action_string(&self.spells[spell].id),
            );
            self.player_log(&line);
        }
    }
}

/// An instant cast aura: its mods while up, and the spells whose cast spends it.
#[derive(Clone, Debug)]
pub(crate) struct InstantCast {
    pub(crate) mods: Vec<super::ModId>,
    pub(crate) spends: Vec<bool>,
}

impl<A: Agent> Fight<A> {
    pub(crate) fn instant_cast_gained(&mut self, index: usize) {
        for position in 0..self.instant_casts[index].mods.len() {
            self.activate_mod(self.instant_casts[index].mods[position]);
        }
    }

    pub(crate) fn instant_cast_expired(&mut self, index: usize) {
        for position in 0..self.instant_casts[index].mods.len() {
            self.deactivate_mod(self.instant_casts[index].mods[position]);
        }
    }

    /// A cast the aura made instant spends it.
    pub(crate) fn instant_cast_spent(&mut self, aura: AuraRef, index: usize, spell: SpellId) {
        if self.aura(aura).active
            && self.instant_casts[index]
                .spends
                .get(spell)
                .copied()
                .unwrap_or(false)
        {
            self.deactivate_aura(aura);
        }
    }
}
