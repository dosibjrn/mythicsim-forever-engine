//! Go spell_result.go healing: `CalcAndDealHealing` with `OutcomeHealing`, and
//! `CalcAndDealPeriodicHealing` with `Dot.OutcomeTick`, on the casting player. The unit's
//! healing pseudo stats and its attack table's healing multiplier, which nothing changes in
//! scope, come with the spell's class effect.

use crate::mechanics::damage::crit_damage_multiplier;

use super::{
    damage::{OUTCOME_CRIT, OUTCOME_HIT, OUTCOME_LANDED},
    log::action_string,
    Agent, DotId, Fight, Side, SpellId, SpellResult,
};

/// The healing modifiers Go reads off the units for a heal on the player.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Healing {
    /// Go `PseudoStats.HealingDealtMultiplier` of the caster.
    pub(crate) dealt_multiplier: f64,
    /// Go `PseudoStats.HealingTakenMultiplier` of the player healed.
    pub(crate) taken_multiplier: f64,
    /// Go `AttackTable.HealingDealtMultiplier` of the caster against the player.
    pub(crate) table_multiplier: f64,
    /// Go `Spell.HealingPower` for the debug line.
    pub(crate) healing_power: f64,
}

impl<A: Agent> Fight<A> {
    /// Go `CalcAndDealHealing` with `OutcomeHealingCrit` on the player or the target: the
    /// bonus coefficient on the caster's healing power and the unit's bonus healing taken,
    /// the caster and unit multipliers, the healing crit roll on spell crit, then
    /// `DealHealing`'s metrics, the health a unit with a health bar gains, the log line and
    /// the caster's `OnHealDealt`. Only the player has a health bar in scope.
    pub(crate) fn calc_and_deal_healing(
        &mut self,
        spell: SpellId,
        target: Side,
        base_healing: f64,
        healing: Healing,
        bonus_healing_taken: f64,
    ) -> SpellResult {
        self.calc_and_deal_healing_scaled(
            spell,
            target,
            base_healing,
            healing,
            bonus_healing_taken,
            1.0,
        )
    }

    /// [`Self::calc_and_deal_healing`] with the heal, healing power share included, scaled
    /// by `scale`: a chain heal's later jumps. A scale of one changes nothing.
    pub(crate) fn calc_and_deal_healing_scaled(
        &mut self,
        spell: SpellId,
        target: Side,
        base_healing: f64,
        healing: Healing,
        bonus_healing_taken: f64,
        scale: f64,
    ) -> SpellResult {
        self.calc_and_deal_healing_with(
            spell,
            target,
            base_healing,
            healing,
            bonus_healing_taken,
            scale,
            None,
        )
    }

    /// [`Self::calc_and_deal_healing_scaled`] with the caster's multiplier given, for a heal
    /// whose amount already holds the caster's healing multipliers (Swiftmend's consumed heal
    /// over time), which then takes only the spell's own.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn calc_and_deal_healing_with(
        &mut self,
        spell: SpellId,
        target: Side,
        base_healing: f64,
        healing: Healing,
        bonus_healing_taken: f64,
        scale: f64,
        caster_multiplier: Option<f64>,
    ) -> SpellResult {
        let state = &self.spells[spell];
        let caster = state.caster;
        let healing_power = self.unit(caster).powers.healing_power + bonus_healing_taken;
        let mut base = base_healing;
        if state.bonus_coefficient > 0.0 {
            // Go fuses `baseHealing += spell.BonusCoefficient * spell.HealingPower(target)`.
            base = state.bonus_coefficient.mul_add(healing_power, base);
        }
        if scale != 1.0 {
            base *= scale;
        }
        let multiplier = if state.flags.ignore_attacker_modifiers {
            1.0
        } else if let Some(multiplier) = caster_multiplier {
            multiplier
        } else {
            state.damage_multiplier
                * state.damage_multiplier_additive
                * (healing.dealt_multiplier * self.healing_dealt_factor)
        };
        let mut amount = base * multiplier;
        let after_caster = amount;
        if !state.flags.ignore_target_modifiers {
            amount = amount * healing.taken_multiplier * healing.table_multiplier;
        }
        let after_target = amount;
        let index = target.index();
        let outcome = if self.proc(self.heal_crit_chance(spell), "Healing Crit Roll") {
            amount *= self.heal_crit_multiplier(spell);
            self.spells[spell].metrics[index].crits += 1;
            OUTCOME_CRIT
        } else {
            self.spells[spell].metrics[index].hits += 1;
            OUTCOME_HIT
        };
        let after_outcome = amount;
        let amount = amount.max(0.0);
        if self.log.is_some() {
            let line = format!(
                "[{}] {} [DEBUG] HealingPower: {:.1}, BaseHealing:{:.1}, AfterCasterMods:{:.1}, AfterTargetMods:{:.1}, AfterOutcome:{:.1}, AfterPostOutcome:{:.1}",
                self.label_of(target),
                action_string(&self.spells[spell].id),
                healing_power,
                base,
                after_caster,
                after_target,
                after_outcome,
                amount
            );
            self.unit_log(caster, &line);
        }
        let (amount, overhealing) = self.landed_healing(spell, index, amount);
        let mut result = SpellResult {
            armor_multiplier: 0.0,
            target,
            attacker: self.spells[spell].caster,
            outcome,
            damage: amount,
            threat: 0.0,
        };
        result.threat = self.threat_of(spell, &result);
        // Go dealHealingInternal.
        let metrics = &mut self.spells[spell].metrics[index];
        if outcome == OUTCOME_CRIT {
            metrics.total_crit_healing += amount;
        }
        metrics.total_healing += amount;
        metrics.total_threat += result.threat;
        if target == Side::Player {
            self.note_burst(amount);
            let health = match self.spells[spell].self_health_metrics {
                Some(index) => index,
                None => {
                    let index = self.new_health_metrics(self.spells[spell].id.clone());
                    self.spells[spell].self_health_metrics = Some(index);
                    index
                }
            };
            self.gain_health(amount, health);
        }
        if self.log.is_some() && !self.spells[spell].flags.no_logs {
            let line = format!(
                "[{}] {} {} for {amount:.3} healing{}. (Threat: {:.3})",
                self.label_of(target),
                action_string(&self.spells[spell].id),
                if outcome == OUTCOME_CRIT {
                    "Crit"
                } else {
                    "Hit"
                },
                overhealing_note(overhealing),
                result.threat
            );
            self.unit_log(caster, &line);
        }
        self.on_heal_dealt(spell, &result);
        result
    }

    /// Go `CalcAndDealHealing` on the player with `OutcomeHealingCrit`: the spell's coefficient
    /// on healing power, the caster and target multipliers, a crit roll on spell crit with a
    /// magic crit's multiplier and no attack table, then `DealHealing`'s metrics, health gain on
    /// the spell's own health metrics and log line.
    pub(crate) fn calc_and_deal_self_healing_crit(
        &mut self,
        spell: SpellId,
        base: f64,
        healing: Healing,
    ) {
        let state = &self.spells[spell];
        let mut base = base;
        if state.bonus_coefficient > 0.0 {
            // Go fuses `baseHealing += spell.BonusCoefficient * spell.HealingPower(target)`.
            base = state.bonus_coefficient.mul_add(healing.healing_power, base);
        }
        let caster = if state.flags.ignore_attacker_modifiers {
            1.0
        } else {
            state.damage_multiplier
                * state.damage_multiplier_additive
                * (healing.dealt_multiplier * self.healing_dealt_factor)
        };
        let mut amount = base * caster;
        let after_caster = amount;
        if !state.flags.ignore_target_modifiers {
            amount = amount * healing.taken_multiplier * healing.table_multiplier;
        }
        let after_target = amount;
        // Go OutcomeHealingCrit: spell crit and the spell's bonus, no suppression or table.
        let target = Side::Player.index();
        let chance =
            (self.unit(state.caster).powers.spell_crit_percent + state.bonus_crit_percent) / 100.0;
        let crit = self.proc(chance, "Healing Crit Roll");
        let outcome = if crit {
            let state = &self.spells[spell];
            let base_multiplier = if state.magic_defense { 1.5 } else { 2.0 };
            let unit = self.unit_config(state.caster).crit_damage_multiplier;
            amount *= (base_multiplier * state.crit_multiplier_pct * unit - 1.0)
                * (state.crit_multiplier_additive + 1.0)
                + 1.0;
            self.spells[spell].metrics[target].crits += 1;
            OUTCOME_CRIT
        } else {
            self.spells[spell].metrics[target].hits += 1;
            OUTCOME_HIT
        };
        let after_outcome = amount;
        let amount = amount.max(0.0);
        if self.log.is_some() {
            let line = format!(
                "[{}] {} [DEBUG] HealingPower: {:.1}, BaseHealing:{:.1}, AfterCasterMods:{:.1}, AfterTargetMods:{:.1}, AfterOutcome:{:.1}, AfterPostOutcome:{:.1}",
                self.config.player_label,
                action_string(&self.spells[spell].id),
                healing.healing_power,
                base,
                after_caster,
                after_target,
                after_outcome,
                amount
            );
            self.player_log(&line);
        }
        let (amount, overhealing) = self.landed_healing(spell, target, amount);
        let result = SpellResult {
            armor_multiplier: 0.0,
            target: Side::Player,
            attacker: self.spells[spell].caster,
            outcome,
            damage: amount,
            threat: 0.0,
        };
        let threat = self.threat_of(spell, &result);
        // Go dealHealingInternal.
        let metrics = &mut self.spells[spell].metrics[target];
        if crit {
            metrics.total_crit_healing += amount;
        }
        metrics.total_healing += amount;
        metrics.total_threat += threat;
        self.note_burst(amount);
        let health = match self.spells[spell].self_health_metrics {
            Some(index) => index,
            None => {
                let index = self.new_health_metrics(self.spells[spell].id.clone());
                self.spells[spell].self_health_metrics = Some(index);
                index
            }
        };
        self.gain_health(amount, health);
        if self.log.is_some() && !self.spells[spell].flags.no_logs {
            let line = format!(
                "[{}] {} {} for {amount:.3} healing{}. (Threat: {threat:.3})",
                self.config.player_label,
                action_string(&self.spells[spell].id),
                if crit { "Crit" } else { "Hit" },
                overhealing_note(overhealing),
            );
            self.player_log(&line);
        }
    }

    /// Go `CalcAndDealHealing` on the player with `OutcomeHealing`: no bonus coefficient in
    /// scope, the caster and target multipliers, a counted hit, then `DealHealing`'s metrics,
    /// health gain on the spell's own health metrics and log line.
    pub(crate) fn calc_and_deal_self_healing(
        &mut self,
        spell: SpellId,
        base: f64,
        healing: Healing,
    ) {
        let result = self.self_healing(spell, base, healing, None);
        // Go DealHealing: the caster's OnHealDealt.
        self.on_heal_dealt(spell, &result);
    }

    /// Go `CalcAndDealPeriodicHealing` on the player with `Dot.OutcomeTick`: the caster's
    /// multipliers include its periodic healing dealt, the tick counts, and the log line names
    /// the tick.
    pub(crate) fn periodic_self_healing_tick(
        &mut self,
        dot: DotId,
        base: f64,
        healing: Healing,
        periodic_dealt_multiplier: f64,
    ) {
        let spell = self.dots[dot].spell;
        self.self_healing(spell, base, healing, Some(periodic_dealt_multiplier));
    }

    /// The shared heal on the player; `periodic` holds Go
    /// `PseudoStats.PeriodicHealingDealtMultiplier` for a hot's tick.
    fn self_healing(
        &mut self,
        spell: SpellId,
        base: f64,
        healing: Healing,
        periodic: Option<f64>,
    ) -> SpellResult {
        assert!(
            self.spells[spell].bonus_coefficient == 0.0,
            "healing spell power is not supported"
        );
        let state = &self.spells[spell];
        let caster = if state.flags.ignore_attacker_modifiers {
            1.0
        } else {
            let multiplier = state.damage_multiplier
                * state.damage_multiplier_additive
                * (healing.dealt_multiplier * self.healing_dealt_factor);
            match periodic {
                Some(periodic) => multiplier * periodic,
                None => multiplier,
            }
        };
        let mut amount = base * caster;
        let after_caster = amount;
        if !state.flags.ignore_target_modifiers {
            amount = amount * healing.taken_multiplier * healing.table_multiplier;
        }
        let after_target = amount;
        // Go OutcomeHealing, or Dot.OutcomeTick for a hot.
        let target = Side::Player.index();
        if periodic.is_some() {
            self.spells[spell].metrics[target].ticks += 1;
        } else {
            self.spells[spell].metrics[target].hits += 1;
        }
        let after_outcome = amount;
        let amount = amount.max(0.0);
        if self.log.is_some() {
            let line = format!(
                "[{}] {} [DEBUG] HealingPower: {:.1}, BaseHealing:{:.1}, AfterCasterMods:{:.1}, AfterTargetMods:{:.1}, AfterOutcome:{:.1}, AfterPostOutcome:{:.1}",
                self.config.player_label,
                action_string(&self.spells[spell].id),
                healing.healing_power,
                base,
                after_caster,
                after_target,
                after_outcome,
                amount
            );
            self.player_log(&line);
        }
        let (amount, overhealing) = self.landed_healing(spell, target, amount);
        let mut result = SpellResult {
            armor_multiplier: 0.0,
            target: Side::Player,
            attacker: self.spells[spell].caster,
            outcome: OUTCOME_HIT,
            damage: amount,
            threat: 0.0,
        };
        let threat = self.threat_of(spell, &result);
        result.threat = threat;
        debug_assert!(result.outcome & OUTCOME_LANDED != 0);
        // Go dealHealingInternal.
        let metrics = &mut self.spells[spell].metrics[target];
        metrics.total_healing += amount;
        metrics.total_threat += threat;
        self.note_burst(amount);
        let health = match self.spells[spell].self_health_metrics {
            Some(index) => index,
            None => {
                let index = self.new_health_metrics(self.spells[spell].id.clone());
                self.spells[spell].self_health_metrics = Some(index);
                index
            }
        };
        self.gain_health(amount, health);
        if self.log.is_some() && !self.spells[spell].flags.no_logs {
            let line = format!(
                "[{}] {}{} Hit for {amount:.3} healing{}. (Threat: {threat:.3})",
                self.config.player_label,
                action_string(&self.spells[spell].id),
                if periodic.is_some() { " tick" } else { "" },
                overhealing_note(overhealing),
            );
            self.player_log(&line);
        }
        result
    }
}

impl<A: Agent> Fight<A> {
    /// Go outcomeHealingCrit's chance: spell crit and the spell's bonus, without an attack table.
    pub(crate) fn heal_crit_chance(&self, spell: SpellId) -> f64 {
        let state = &self.spells[spell];
        (self.unit(state.caster).powers.spell_crit_percent + state.bonus_crit_percent) / 100.0
            + self.heal_crit_bonus
    }

    /// [`Self::heal_crit_chance`] for a heal over time's tick under Forever's rules, less the crit
    /// a dot's tick does not take (Inner Focus), as a damage tick's.
    pub(crate) fn heal_tick_crit_chance(&self, spell: SpellId) -> f64 {
        let mut chance = self.heal_crit_chance(spell);
        for (id, spells, value) in &self.tick_crit_takebacks {
            if spells[spell] && self.mod_active(*id) {
                chance = (chance - value).max(0.0);
            }
        }
        chance
    }

    /// Go outcomeHealingCrit's multiplier: a magic crit's, or a physical one's, with the
    /// spell's and the caster's crit modifiers and no attack table.
    pub(crate) fn heal_crit_multiplier(&self, spell: SpellId) -> f64 {
        let state = &self.spells[spell];
        crit_damage_multiplier(
            state.magic_defense,
            state.crit_multiplier_pct,
            self.unit_config(state.caster).crit_damage_multiplier,
            1.0,
            state.crit_multiplier_additive,
        )
    }

    /// A heal of `amount` on `target` split into what counts as healing and what counts as
    /// overhealing: all of it heals, unless the spell carries an overhealing share (MythicSim's
    /// healer sims under Forever's rules), which goes to the spell's overhealing instead.
    /// Healing, threat, the health gained and the log line then read what heals.
    pub(crate) fn landed_healing(
        &mut self,
        spell: SpellId,
        target: usize,
        amount: f64,
    ) -> (f64, f64) {
        let fraction = self.spells[spell].overheal;
        if fraction == 0.0 {
            return (amount, 0.0);
        }
        let overhealing = amount * fraction;
        self.spells[spell].metrics[target].total_overhealing += overhealing;
        (amount - overhealing, overhealing)
    }
}

impl<A: Agent> Fight<A> {
    /// Healing landed on the player, for a healer sim's burst window.
    pub(crate) fn note_burst(&mut self, amount: f64) {
        let now = self.now;
        if let Some(burst) = &mut self.burst {
            burst.add(now, amount);
        }
    }
}

/// What a heal's log line adds for its overhealing: nothing when it has none, as Go logs.
pub(crate) fn overhealing_note(overhealing: f64) -> String {
    if overhealing > 0.0 {
        format!(", {overhealing:.3} overhealing")
    } else {
        String::new()
    }
}
