//! Go spell_result.go, spell_outcome.go and spell_resistances.go for magic spells.

use crate::mechanics::damage::{
    binary_resist_hit, crit_damage_multiplier, partial_resist_thresholds, resist_coefficient,
    spell_chance_to_miss,
};

use super::{log::action_string, Action, Agent, Fight, Side, SpellId, PRIORITY_GCD};

pub(crate) const OUTCOME_MISS: u16 = 1;
pub(crate) const OUTCOME_HIT: u16 = 1 << 1;
pub(crate) const OUTCOME_CRIT: u16 = 1 << 2;
pub(crate) const OUTCOME_PARTIAL_1_4: u16 = 1 << 3;
pub(crate) const OUTCOME_PARTIAL_2_4: u16 = 1 << 4;
pub(crate) const OUTCOME_PARTIAL_3_4: u16 = 1 << 5;
pub(crate) const OUTCOME_PARTIAL: u16 =
    OUTCOME_PARTIAL_1_4 | OUTCOME_PARTIAL_2_4 | OUTCOME_PARTIAL_3_4;
pub(crate) const OUTCOME_DODGE: u16 = 1 << 6;
pub(crate) const OUTCOME_GLANCE: u16 = 1 << 7;
pub(crate) const OUTCOME_PARRY: u16 = 1 << 8;
pub(crate) const OUTCOME_BLOCK: u16 = 1 << 9;
pub(crate) const OUTCOME_CRUSH: u16 = 1 << 10;
/// Go outcome appliers the runtime implements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    MagicHitAndCrit,
    MagicHit,
    /// Go `Spell.OutcomeMagicCrit`: no hit roll, only the crit roll.
    MagicCrit,
    Tick,
    TickMagicCrit,
    /// Go `OutcomeAlwaysHitNoHitCounter`.
    AlwaysHitNoHitCounter,
    /// Go `OutcomeAlwaysHit`: a hit that counts, a partial resist included.
    AlwaysHit,
    /// Go `OutcomeMagicHitNoHitCounter`: a miss still counts.
    MagicHitNoHitCounter,
    /// Go `OutcomeTickPhysicalCrit`.
    TickPhysicalCrit,
    /// Go `Spell.OutcomeTickMagicHitAndCrit`: a tick that rolls to hit, then to crit.
    TickMagicHitAndCrit,
    /// A melee or ranged attack table applier, for a spell of any school.
    Table(super::melee::PhysicalOutcome),
    /// Go `OutcomeExpectedMagicHit`: the damage scaled by the chance to hit, with no roll.
    ExpectedMagicHit,
    /// Go `OutcomeExpectedMagicAlwaysHit`: the damage as it is.
    ExpectedMagicAlwaysHit,
    /// Go `OutcomeExpectedMagicCrit`: the damage scaled by the average crit.
    ExpectedMagicCrit,
}

impl Outcome {
    /// spelldata `Spell.TickOutcomeHitRolled`: the tick of a dot whose hit was rolled when it was
    /// applied, a crit only where the row states Periodic Can Crit, on the crit of the dot
    /// spell's defense type. A tick that would roll a magic hit again is unsupported.
    pub(crate) fn tick_hit_rolled(can_crit: bool, magic: bool) -> Result<Outcome, String> {
        match (can_crit, magic) {
            (true, true) => Ok(Outcome::TickMagicCrit),
            (true, false) => Ok(Outcome::TickPhysicalCrit),
            (false, false) => Ok(Outcome::Tick),
            (false, true) => Err("dot ticks that roll a magic hit are unsupported".into()),
        }
    }
}

/// Go `OutcomeLanded`.
pub(crate) const OUTCOME_LANDED: u16 =
    OUTCOME_HIT | OUTCOME_CRIT | OUTCOME_CRUSH | OUTCOME_GLANCE | OUTCOME_BLOCK;

/// Go `SpellResultSlice` of an area hit: one result per target, in unit index order.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AoeResults {
    results: [SpellResult; super::MAX_TARGETS],
    len: usize,
}

impl Default for AoeResults {
    fn default() -> Self {
        AoeResults {
            results: [SpellResult {
                target: Side::Target,
                attacker: Side::Player,
                outcome: 0,
                damage: 0.0,
                threat: 0.0,
                armor_multiplier: 0.0,
            }; super::MAX_TARGETS],
            len: 0,
        }
    }
}

impl AoeResults {
    fn push(&mut self, result: SpellResult) {
        self.results[self.len] = result;
        self.len += 1;
    }

    /// The results, in unit index order.
    pub(crate) fn as_slice(&self) -> &[SpellResult] {
        &self.results[..self.len]
    }
}

/// Go `SpellResult`, carried by value until its damage is dealt.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SpellResult {
    pub(crate) target: Side,
    /// Go `spell.Unit`: the unit whose spell made the hit. A hit-taken listener reads it to
    /// answer the copy of the boss that swung at the player, as Holy Shield's damage and Eye
    /// for an Eye's reflection do.
    pub(crate) attacker: Side,
    pub(crate) outcome: u16,
    pub(crate) damage: f64,
    pub(crate) threat: f64,
    /// Go `ArmorAndResistanceMultiplier`: the factor armor or a partial resist took from the
    /// damage when the result was calculated, set by the paths that calculate damage.
    pub(crate) armor_multiplier: f64,
}

impl SpellResult {
    pub(crate) fn landed(&self) -> bool {
        self.outcome & OUTCOME_LANDED != 0
    }

    pub(crate) fn crit(&self) -> bool {
        self.outcome & OUTCOME_CRIT != 0
    }

    /// Go `HitOutcome.String` for spell outcomes.
    pub(crate) fn outcome_string(&self) -> String {
        let partial = if self.outcome & OUTCOME_PARTIAL_1_4 != 0 {
            " (25% Resist)"
        } else if self.outcome & OUTCOME_PARTIAL_2_4 != 0 {
            " (50% Resist)"
        } else if self.outcome & OUTCOME_PARTIAL_3_4 != 0 {
            " (75% Resist)"
        } else {
            ""
        };
        if self.outcome & OUTCOME_MISS != 0 {
            "Miss".into()
        } else if self.outcome & OUTCOME_DODGE != 0 {
            "Dodge".into()
        } else if self.outcome & OUTCOME_PARRY != 0 {
            "Parry".into()
        } else if self.outcome & OUTCOME_BLOCK != 0 && self.outcome & OUTCOME_CRIT != 0 {
            "BlockedCrit".into()
        } else if self.outcome & OUTCOME_BLOCK != 0 {
            "Block".into()
        } else if self.outcome & OUTCOME_GLANCE != 0 {
            format!("Glance{partial}")
        } else if self.outcome & OUTCOME_CRIT != 0 {
            format!("Crit{partial}")
        } else if self.outcome & OUTCOME_HIT != 0 {
            format!("Hit{partial}")
        } else if self.outcome & OUTCOME_CRUSH != 0 {
            "Crush".into()
        } else {
            "Empty".into()
        }
    }

    /// Go `SpellResult.DamageString`.
    pub(crate) fn damage_string(&self) -> String {
        if self.landed() {
            format!("{} for {:.3} damage", self.outcome_string(), self.damage)
        } else {
            self.outcome_string()
        }
    }
}

impl<A: Agent> Fight<A> {
    /// Go `Spell.schoolValue`: one school's entry, or the larger of Fire and Frost for
    /// Frostfire so an effect on both schools never counts twice.
    pub(crate) fn school_value(&self, spell: SpellId, values: &[f64; 8]) -> f64 {
        let state = &self.spells[spell];
        if state.frostfire {
            values[super::SCHOOL_INDEX_FIRE].max(values[super::SCHOOL_INDEX_FROST])
        } else {
            values[state.school_index]
        }
    }

    /// Go `Unit.GetSpellDamageValue`: generic plus school spell damage.
    pub(crate) fn spell_power(&self, spell: SpellId) -> f64 {
        let state = &self.spells[spell];
        let school_damage = if state.caster == Side::Player {
            self.player_school_damage()
        } else {
            &self.unit_config(state.caster).school_damage
        };
        self.unit(state.caster).powers.spell_damage
            + state.bonus_spell_damage
            + self.school_value(spell, school_damage)
    }

    /// Go `Spell.ThreatFromDamage`, whose damage times the threat multiplier plus the flat
    /// bonus the arm64 build fuses.
    /// Raises a calculated result's damage by `factor`. A bonus applied after the calculation
    /// (Improved Mind Flay's ramp, Ice Lance on a frozen target, Soul Siphon, Amplify Curse,
    /// Incinerate on an Immolated target) is damage the target takes, so under Forever's rules
    /// (`threat_follows_damage_bonus`) it is threat too. Go scaled only the damage, which left a
    /// Shadow Priest's threat below its damage with no threat talents (Vrelk, 26 October 2026).
    pub(crate) fn scale_damage(&self, spell: SpellId, result: &mut SpellResult, factor: f64) {
        result.damage *= factor;
        if self.threat_follows_damage_bonus {
            result.threat = self.threat_of(spell, result);
        }
    }

    pub(crate) fn threat_of(&self, spell: SpellId, result: &SpellResult) -> f64 {
        if result.landed() {
            let state = &self.spells[spell];
            result
                .damage
                .mul_add(state.threat_multiplier, state.flat_threat_bonus)
                * self.unit(state.caster).threat_multiplier
        } else {
            0.0
        }
    }

    /// Go `AttackTable.MobTypeBonusStats[Defender.MobType][SpellDamage]` on the spell's caster's
    /// table: the player's, while the Potion of Elemental Purging holds against its mob type.
    fn mob_type_spell_damage(&self, spell: SpellId) -> f64 {
        if self.spells[spell].caster != Side::Player {
            return 0.0;
        }
        let potion = match self.mob_type_spell_damage {
            Some((aura, bonus)) if self.aura(aura).active => bonus,
            _ => 0.0,
        };
        let marks: f64 = self
            .mob_type_bonuses
            .iter()
            .filter(|(aura, _, _)| self.aura(*aura).active)
            .map(|(_, _, spell_damage)| spell_damage)
            .sum();
        potion + marks
    }

    /// Go `Spell.BonusDamage` for a magic spell against a target.
    pub(crate) fn bonus_damage(&self, spell: SpellId, target: Side) -> f64 {
        let state = &self.spells[spell];
        let mut bonus = state.bonus_base_damage;
        bonus += self.spell_power(spell)
            + self.mob_type_spell_damage(spell)
            + self.school_value(
                spell,
                &self.target_unit(target).state.school_bonus_spell_damage,
            );
        bonus
    }

    /// Go `Spell.AttackerDamageMultiplier` for a direct hit.
    pub(crate) fn attacker_multiplier(&self, spell: SpellId, periodic: bool) -> f64 {
        let state = &self.spells[spell];
        if state.flags.ignore_attacker_modifiers {
            return 1.0;
        }
        let config = self.unit_config(state.caster);
        let additive = if periodic {
            state.damage_multiplier_additive + config.dot_damage_multiplier_additive - 1.0
        } else {
            state.damage_multiplier_additive + state.direct_damage_multiplier_additive
        };
        let internal = self.unit(state.caster).damage_dealt_multiplier
            * self.school_value(
                spell,
                &self.unit(state.caster).school_damage_dealt_multiplier,
            )
            * config.table.damage_dealt_multiplier;
        internal * state.damage_multiplier * additive
    }

    /// Go `Spell.TargetDamageMultiplier` against a target.
    pub(crate) fn target_multiplier(&self, spell: SpellId, target: Side) -> f64 {
        let state = &self.spells[spell];
        if state.flags.ignore_target_modifiers {
            return 1.0;
        }
        let multiplier = self.config.target_damage_taken_multiplier
            * self.school_value(
                spell,
                &self
                    .target_unit(target)
                    .state
                    .school_damage_taken_multiplier,
            )
            * self.unit_config(state.caster).table.damage_taken_multiplier;
        // Go's DamageDoneByCasterExtraMultiplier handlers, multiplied in after the rest.
        match A::caster_damage_multiplier(self, spell, target) {
            Some(caster) => multiplier * caster,
            None => multiplier,
        }
    }

    /// Go `ResistanceMultiplier`'s magic branch against a target: the partial resist roll, its
    /// multiplier and outcome.
    pub(crate) fn partial_resist(&mut self, spell: SpellId, target: Side) -> (f64, u16) {
        let roll = self.random("Partial Resist");
        let (none, quarter, half) = partial_resist_thresholds(self.resist(spell, target, false));
        if roll > none {
            (1.0, 0)
        } else if roll > quarter {
            (0.75, OUTCOME_PARTIAL_1_4)
        } else if roll > half {
            (0.5, OUTCOME_PARTIAL_2_4)
        } else {
            (0.25, OUTCOME_PARTIAL_3_4)
        }
    }

    /// Go `spellVulnerabilityMultiplier` (MythicSim patch 146): the caster's Spell Piercing past
    /// the target's resistance, floored at 0, adds 1 / (5 x target level) of damage a point, on
    /// binary spells too. Schools with no resistance take none.
    pub(crate) fn spell_vulnerability(&self, spell: SpellId, target: Side) -> f64 {
        let state = &self.spells[spell];
        let level = self.unit_config(state.caster).target_level;
        if state.school_index <= 1 || level <= 0 {
            return 1.0;
        }
        // Go ResistanceStat: Arcane, Fire, Frost, Nature and Shadow alone have one; Frostfire
        // takes the lower of its two.
        if !state.frostfire && !matches!(state.school, 4 | 8 | 16 | 32 | 64) {
            return 1.0;
        }
        let resistance = &self.target_unit(target).state.resistance;
        let raw = if state.frostfire {
            resistance[super::SCHOOL_INDEX_FIRE].min(resistance[super::SCHOOL_INDEX_FROST])
        } else {
            resistance[state.school_index]
        };
        let beyond = self.unit(state.caster).powers.spell_piercing - raw.max(0.0);
        if beyond <= 0.0 {
            return 1.0;
        }
        1.0 + beyond / (5.0 * f64::from(level))
    }

    fn resist(&self, spell: SpellId, target: Side, binary: bool) -> f64 {
        let state = &self.spells[spell];
        // Go resistCoeff: a physical spell, as a binary Thunder Clap, resists nothing.
        if state.school_index <= 1 {
            return 0.0;
        }
        // Go resistCoeff: Frostfire Bolt checks the lower resistance.
        let resistance = &self.target_unit(target).state.resistance;
        let resistance = if state.frostfire {
            resistance[super::SCHOOL_INDEX_FIRE].min(resistance[super::SCHOOL_INDEX_FROST])
        } else {
            resistance[state.school_index]
        };
        let config = self.unit_config(state.caster);
        // Forever: the target's level adds no partial resist, so only its resistance counts.
        // Binary spells take none from level either way.
        let target_level = if config.level_partial_resist {
            config.target_level
        } else {
            config.target_level.min(config.player_level)
        };
        resist_coefficient(
            resistance,
            self.unit(state.caster).powers.spell_piercing,
            config.player_level,
            target_level,
            binary,
        )
    }

    /// Go `Spell.SpellHitChance`.
    pub(crate) fn spell_hit_chance(&self, spell: SpellId) -> f64 {
        let state = &self.spells[spell];
        let config = self.unit_config(state.caster);
        let mut hit = self.unit(state.caster).powers.spell_hit_percent + state.bonus_hit_percent;
        if state.class_spell_mask {
            hit += self.school_value(spell, &config.school_bonus_hit_chance);
        }
        hit / 100.0
    }

    /// Go `Spell.SpellCritChance`.
    pub(crate) fn spell_crit_chance(&self, spell: SpellId) -> f64 {
        let state = &self.spells[spell];
        let config = self.unit_config(state.caster);
        let crit = self.unit(state.caster).powers.spell_crit_percent
            + state.bonus_crit_percent
            + config.table.bonus_spell_crit_percent
            - config.target_reduced_crit_taken_percent;
        (crit / 100.0 - config.table.spell_crit_suppression).max(0.0)
    }

    /// Go `CalcDamage` with `OutcomeMagicHitAndCrit`.
    pub(crate) fn calc_damage(
        &mut self,
        spell: SpellId,
        target: Side,
        base_damage: f64,
    ) -> SpellResult {
        let attacker = self.attacker_multiplier(spell, false);
        let mut base = base_damage;
        if self.spells[spell].bonus_coefficient > 0.0 {
            // The arm64 build fuses the share's multiply into the add.
            base = self.spells[spell]
                .bonus_coefficient
                .mul_add(self.bonus_damage(spell, target), base);
        }
        self.calc_damage_internal(spell, target, base, attacker, Outcome::MagicHitAndCrit)
    }

    /// Go `CalcDamage` with `OutcomeMagicHit`: no crit roll.
    pub(crate) fn calc_damage_hit_only(
        &mut self,
        spell: SpellId,
        target: Side,
        base_damage: f64,
    ) -> SpellResult {
        let attacker = self.attacker_multiplier(spell, false);
        let mut base = base_damage;
        if self.spells[spell].bonus_coefficient > 0.0 {
            base = self.spells[spell]
                .bonus_coefficient
                .mul_add(self.bonus_damage(spell, target), base);
        }
        self.calc_damage_internal(spell, target, base, attacker, Outcome::MagicHit)
    }

    /// Go `Spell.CalcAndDealAoeDamageWithVariance`: each target in unit index order, its base
    /// damage rolled, calculated and dealt in turn, so a proc on one hit can change the next
    /// roll. `calc` is the outcome applier's calculation, such as [`Self::calc_damage`]; a
    /// fixed amount, as Go `CalcAndDealAoeDamage`, passes a closure that rolls nothing.
    pub(crate) fn calc_and_deal_aoe_damage_with_variance(
        &mut self,
        spell: SpellId,
        mut base_damage: impl FnMut(&mut Self) -> f64,
        calc: impl Fn(&mut Self, SpellId, Side, f64) -> SpellResult,
    ) -> AoeResults {
        let mut results = AoeResults::default();
        for position in 0..self.targets.len() {
            let base = base_damage(self);
            let result = calc(self, spell, Side::target(position), base);
            self.deal_damage(spell, result, false);
            results.push(result);
        }
        results
    }

    /// Go `Spell.BonusDamage` against a target: physical bonus damage for a physical spell,
    /// spell power and the target's school bonus otherwise.
    pub(crate) fn school_bonus_damage(&self, spell: SpellId, target: Side) -> f64 {
        if self.spells[spell].school & 1 != 0 {
            self.spells[spell].bonus_base_damage + self.player.powers.physical_damage
        } else {
            self.bonus_damage(spell, target)
        }
    }

    /// Go `CalcDamage` with any outcome applier, for a spell of any school: armor for a
    /// physical spell, the partial resist roll and spell damage taken otherwise.
    pub(crate) fn calc_damage_with(
        &mut self,
        spell: SpellId,
        target: Side,
        base_damage: f64,
        outcome: Outcome,
    ) -> SpellResult {
        let attacker = self.attacker_multiplier(spell, false);
        let mut base = base_damage;
        let coefficient = self.spells[spell].bonus_coefficient;
        if coefficient > 0.0 {
            base = coefficient.mul_add(self.school_bonus_damage(spell, target), base);
        } else if self.spells[spell].school & 1 != 0 {
            base += self.school_bonus_damage(spell, target);
        }
        self.calc_damage_internal(spell, target, base, attacker, outcome)
    }

    /// Go `CalcDamage` with a given outcome applier.
    pub(crate) fn calc_damage_with_outcome(
        &mut self,
        spell: SpellId,
        target: Side,
        base_damage: f64,
        outcome: Outcome,
    ) -> SpellResult {
        self.calc_damage_with(spell, target, base_damage, outcome)
    }

    /// Go `calcDamageInternal` for a direct spell of any school.
    fn calc_damage_internal(
        &mut self,
        spell: SpellId,
        target: Side,
        base: f64,
        attacker: f64,
        outcome: Outcome,
    ) -> SpellResult {
        let mut result = SpellResult {
            armor_multiplier: 0.0,
            target,
            attacker: self.spells[spell].caster,
            outcome: 0,
            damage: base,
            threat: 0.0,
        };
        result.damage *= attacker;
        let after_attacker = result.damage;

        // Go applyResistances: binary spells and ignored resists take no partial roll; a
        // physical hit takes armor instead, and a physical tick takes none.
        let binary = self.spells[spell].flags.binary;
        let physical = self.spells[spell].school & 1 != 0;
        // The expected outcomes are read only of ticks, `CalcPeriodicDamage` and
        // `CalcSnapshotDamage`.
        let periodic = matches!(
            outcome,
            Outcome::Tick
                | Outcome::TickMagicCrit
                | Outcome::TickMagicHitAndCrit
                | Outcome::ExpectedMagicHit
                | Outcome::ExpectedMagicAlwaysHit
                | Outcome::ExpectedMagicCrit
        );
        result.armor_multiplier = 1.0;
        if physical {
            if !self.spells[spell].flags.ignore_resists && !periodic {
                result.armor_multiplier = self.armor_modifier(self.caster(spell), target);
                result.damage *= result.armor_multiplier;
            }
        } else if !self.spells[spell].flags.ignore_resists {
            // Spell Vulnerability multiplies the partial resist, and a binary spell's whole hit.
            let vulnerability = self.spell_vulnerability(spell, target);
            let (multiplier, outcome) = if binary {
                (vulnerability, 0)
            } else {
                let (partial, outcome) = self.partial_resist(spell, target);
                (partial * vulnerability, outcome)
            };
            result.armor_multiplier = multiplier;
            result.damage *= multiplier;
            result.outcome |= outcome;
        }
        let after_resistances = result.damage;

        // Go applyTargetModifiers.
        if !self.spells[spell].flags.ignore_target_modifiers {
            if physical {
                result.damage += self.config.melee.defender_bonus_physical_damage_taken;
            } else if self.spells[spell].school_index > 1 {
                result.damage += self.config.target_bonus_spell_damage_taken;
            }
            result.damage *= self.target_multiplier(spell, target);
        }
        let after_target = result.damage;

        let partial = result.outcome & super::damage::OUTCOME_PARTIAL;
        self.apply_outcome(spell, &mut result, binary, outcome);
        if partial != 0 {
            result.outcome |= partial;
        }
        let after_outcome = result.damage;
        self.apply_post_outcome_modifiers(spell, &mut result);

        if self.log.is_some() {
            self.log_damage_debug(
                spell,
                result.target,
                base,
                [
                    after_attacker,
                    after_resistances,
                    after_target,
                    after_outcome,
                ],
                result.damage,
            );
        }

        result.threat = self.threat_of(spell, &result);
        result
    }

    /// Go `calcDamageInternal`'s debug line, which names the target.
    pub(crate) fn log_damage_debug(
        &mut self,
        spell: SpellId,
        target: Side,
        base: f64,
        stages: [f64; 4],
        damage: f64,
    ) {
        let line = format!(
            "[{}] {} [DEBUG] MAP: {:.1}, RAP: {:.1}, SP: {:.1}, BaseDamage:{:.1}, AfterAttackerMods:{:.1}, AfterResistances:{:.1}, AfterTargetMods:{:.1}, AfterOutcome:{:.1}, AfterPostOutcome:{:.1}",
            self.label_of(target),
            action_string(&self.spells[spell].id),
            self.unit(self.caster(spell)).powers.attack_power,
            self.unit(self.caster(spell)).powers.ranged_attack_power,
            self.spell_power(spell),
            base,
            stages[0],
            stages[1],
            stages[2],
            stages[3],
            damage
        );
        self.unit_log(self.caster(spell), &line);
    }

    /// Go `Spell.CritDamageMultiplier`.
    pub(crate) fn crit_multiplier(&self, spell: SpellId) -> f64 {
        let state = &self.spells[spell];
        let config = self.unit_config(state.caster);
        crit_damage_multiplier(
            state.magic_defense,
            state.crit_multiplier_pct,
            config.crit_damage_multiplier,
            config.table.crit_multiplier,
            state.crit_multiplier_additive,
        )
    }

    /// Go `CalcOutcome`: an outcome with no damage, no modifiers and no debug line.
    pub(crate) fn calc_outcome(
        &mut self,
        spell: SpellId,
        target: Side,
        outcome: Outcome,
    ) -> SpellResult {
        let mut result = SpellResult {
            armor_multiplier: 0.0,
            target,
            attacker: self.spells[spell].caster,
            outcome: 0,
            damage: 0.0,
            threat: 0.0,
        };
        let binary = self.spells[spell].flags.binary;
        self.apply_outcome(spell, &mut result, binary, outcome);
        result.threat = self.threat_of(spell, &result);
        result
    }

    /// Run an outcome applier on a result.
    fn apply_outcome(
        &mut self,
        spell: SpellId,
        result: &mut SpellResult,
        binary: bool,
        outcome: Outcome,
    ) {
        match outcome {
            Outcome::MagicHitAndCrit => {
                self.outcome_magic_hit_and_crit(spell, result, binary, true, true)
            }
            Outcome::MagicHit => {
                self.outcome_magic_hit_and_crit(spell, result, binary, false, true)
            }
            Outcome::MagicHitNoHitCounter => {
                self.outcome_magic_hit_and_crit(spell, result, binary, false, false)
            }
            Outcome::MagicCrit => self.outcome_magic_crit(spell, result),
            Outcome::ExpectedMagicHit => {
                let binary_hit =
                    binary.then(|| binary_resist_hit(self.resist(spell, result.target, true)));
                let miss = spell_chance_to_miss(
                    self.unit_config(self.caster(spell))
                        .table
                        .base_spell_miss_chance,
                    binary_hit,
                    self.spell_hit_chance(spell),
                );
                result.damage *= 1.0 - miss;
            }
            Outcome::ExpectedMagicAlwaysHit => {}
            // Go's arm64 build fuses the average crit into the 1.
            Outcome::ExpectedMagicCrit => {
                let crit = self.spell_crit_chance(spell);
                result.damage *= crit.mul_add(self.crit_multiplier(spell) - 1.0, 1.0);
            }
            Outcome::Tick => self.outcome_tick(spell, result, false),
            Outcome::TickPhysicalCrit => self.outcome_tick_physical_crit(spell, result),
            Outcome::TickMagicCrit => self.outcome_tick(spell, result, true),
            Outcome::TickMagicHitAndCrit => {
                let binary_hit =
                    binary.then(|| binary_resist_hit(self.resist(spell, result.target, true)));
                let miss = spell_chance_to_miss(
                    self.config.table.base_spell_miss_chance,
                    binary_hit,
                    self.spell_hit_chance(spell),
                );
                if self.proc(1.0 - miss, "Magical Hit Roll") {
                    self.outcome_tick(spell, result, true);
                } else {
                    result.outcome = OUTCOME_MISS;
                    result.damage = 0.0;
                    self.spells[spell].metrics[result.target.index()].misses += 1;
                }
            }
            Outcome::Table(table) => self.apply_physical_outcome(spell, result, table),
            Outcome::AlwaysHitNoHitCounter => result.outcome = OUTCOME_HIT,
            Outcome::AlwaysHit => {
                let partial = result.outcome & OUTCOME_PARTIAL != 0;
                result.outcome = OUTCOME_HIT;
                let metrics = &mut self.spells[spell].metrics[result.target.index()];
                metrics.hits += 1;
                if partial {
                    metrics.resisted_hits += 1;
                }
            }
        }
    }

    /// Go `outcomeMagicHitAndCrit` with hit counters, or `outcomeMagicHit` without the crit
    /// roll, and without the hit counter when `count_hits` is false.
    fn outcome_magic_hit_and_crit(
        &mut self,
        spell: SpellId,
        result: &mut SpellResult,
        binary: bool,
        can_crit: bool,
        count_hits: bool,
    ) {
        let binary_hit = binary.then(|| binary_resist_hit(self.resist(spell, result.target, true)));
        let miss = spell_chance_to_miss(
            self.unit_config(self.caster(spell))
                .table
                .base_spell_miss_chance,
            binary_hit,
            self.spell_hit_chance(spell),
        );
        let target = result.target.index();
        if self.proc(1.0 - miss, "Magical Hit Roll") {
            let partial = result.outcome & OUTCOME_PARTIAL != 0;
            if can_crit && self.proc(self.spell_crit_chance(spell), "Magical Crit Roll") {
                result.outcome = OUTCOME_CRIT;
                result.damage *= self.crit_multiplier(spell);
                let metrics = &mut self.spells[spell].metrics[target];
                metrics.crits += 1;
                if partial {
                    metrics.resisted_crits += 1;
                }
            } else {
                result.outcome = OUTCOME_HIT;
                if count_hits {
                    let metrics = &mut self.spells[spell].metrics[target];
                    metrics.hits += 1;
                    if partial {
                        metrics.resisted_hits += 1;
                    }
                }
            }
        } else {
            result.outcome = OUTCOME_MISS;
            result.damage = 0.0;
            self.spells[spell].metrics[target].misses += 1;
        }
    }

    /// Go `outcomeMagicCrit`: a crit roll against the spell's crit chance, otherwise a hit; the
    /// spell cannot miss.
    fn outcome_magic_crit(&mut self, spell: SpellId, result: &mut SpellResult) {
        let partial = result.outcome & OUTCOME_PARTIAL != 0;
        let target = result.target.index();
        if self.proc(self.spell_crit_chance(spell), "Magical Crit Roll") {
            result.outcome = OUTCOME_CRIT;
            result.damage *= self.crit_multiplier(spell);
            let metrics = &mut self.spells[spell].metrics[target];
            metrics.crits += 1;
            if partial {
                metrics.resisted_crits += 1;
            }
        } else {
            result.outcome = OUTCOME_HIT;
            let metrics = &mut self.spells[spell].metrics[target];
            metrics.hits += 1;
            if partial {
                metrics.resisted_hits += 1;
            }
        }
    }

    /// A tick's crit chance: the spell's, less each active crit modifier that does not reach
    /// ticks (Go priest `tickOutcome`, community #793), never below zero.
    fn tick_crit_chance(&self, spell: SpellId) -> f64 {
        let mut chance = self.spell_crit_chance(spell);
        for (id, spells, value) in &self.tick_crit_takebacks {
            if spells[spell] && self.mod_active(*id) {
                chance = (chance - value).max(0.0);
            }
        }
        chance
    }

    /// Go `Dot.OutcomeTick`, or `OutcomeTickMagicCrit` when the tick can crit: a tick never
    /// rolls to hit, since the dot did that when it landed.
    fn outcome_tick(&mut self, spell: SpellId, result: &mut SpellResult, can_crit: bool) {
        let partial = result.outcome & OUTCOME_PARTIAL != 0;
        let target = result.target.index();
        if can_crit && self.proc(self.tick_crit_chance(spell), "Magical Crit Roll") {
            result.outcome = OUTCOME_CRIT;
            result.damage *= self.crit_multiplier(spell);
            let metrics = &mut self.spells[spell].metrics[target];
            metrics.crit_ticks += 1;
            if partial {
                metrics.resisted_crit_ticks += 1;
            }
        } else {
            result.outcome = OUTCOME_HIT;
            let metrics = &mut self.spells[spell].metrics[target];
            metrics.ticks += 1;
            if partial {
                metrics.resisted_ticks += 1;
            }
        }
    }

    /// Go `ApplyPostOutcomeDamageModifiers`: the target's dynamic modifiers in order, then no
    /// negative damage.
    pub(crate) fn apply_post_outcome_modifiers(&self, spell: SpellId, result: &mut SpellResult) {
        // Go registers the modifiers on every target alike, each reading its own auras.
        for modifier in &self.damage_taken_modifiers {
            if self.spells[spell].caster == modifier.source
                && self.spells[spell].school & modifier.school_mask != 0
                && self.aura(self.aura_on(modifier.aura, result.target)).active
            {
                result.damage *= match modifier.per_stack {
                    Some(per_stack) => {
                        let stacks = self.aura(self.aura_on(modifier.aura, result.target)).stacks;
                        1.0 + per_stack * f64::from(stacks)
                    }
                    None => modifier.multiplier,
                };
            }
        }
        for modifier in &self.spell_damage_taken_modifiers {
            if modifier.spells[spell]
                && modifier
                    .auras
                    .iter()
                    .any(|&aura| self.aura(self.aura_on(aura, result.target)).active)
            {
                result.damage *= modifier.multiplier;
            }
        }
        // Go's built-in max keeps a NaN, as an empty weapon slot's damage is.
        if !result.damage.is_nan() {
            result.damage = result.damage.max(0.0);
        }
    }

    /// Go `OutcomeTickPhysicalCrit`: a tick that rolls the physical crit, keeping a partial
    /// resist in its counters.
    pub(crate) fn outcome_tick_physical_crit(&mut self, spell: SpellId, result: &mut SpellResult) {
        let partial = result.outcome & OUTCOME_PARTIAL != 0;
        let target = result.target.index();
        if self.proc(self.physical_crit_chance(spell), "Physical Crit Roll") {
            result.outcome = OUTCOME_CRIT;
            result.damage *= self.crit_multiplier(spell);
            let metrics = &mut self.spells[spell].metrics[target];
            metrics.crit_ticks += 1;
            if partial {
                metrics.resisted_crit_ticks += 1;
            }
        } else {
            result.outcome = OUTCOME_HIT;
            let metrics = &mut self.spells[spell].metrics[target];
            metrics.ticks += 1;
            if partial {
                metrics.resisted_ticks += 1;
            }
        }
    }

    /// Go `calcDamageInternal` for a periodic tick of any school: a physical tick ignores armor
    /// and takes the defender's physical bonus, a magic tick rolls its partial resist.
    pub(crate) fn calc_tick_damage(
        &mut self,
        spell: SpellId,
        target: Side,
        base: f64,
        attacker: f64,
        outcome: Outcome,
    ) -> SpellResult {
        if self.spells[spell].school & 1 == 0 {
            return self.calc_damage_internal(spell, target, base, attacker, outcome);
        }
        let mut result = SpellResult {
            armor_multiplier: 0.0,
            target,
            attacker: self.spells[spell].caster,
            outcome: 0,
            damage: base * attacker,
            threat: 0.0,
        };
        let after_attacker = result.damage;
        // Go ResistanceMultiplier: every physical dot ignores armor.
        let after_resistances = result.damage;
        if !self.spells[spell].flags.ignore_target_modifiers {
            result.damage += self.config.melee.defender_bonus_physical_damage_taken;
            result.damage *= self.target_multiplier(spell, target);
        }
        let after_target = result.damage;
        let binary = self.spells[spell].flags.binary;
        self.apply_outcome(spell, &mut result, binary, outcome);
        let after_outcome = result.damage;
        self.apply_post_outcome_modifiers(spell, &mut result);
        if self.log.is_some() {
            self.log_damage_debug(
                spell,
                result.target,
                base,
                [
                    after_attacker,
                    after_resistances,
                    after_target,
                    after_outcome,
                ],
                result.damage,
            );
        }
        result.threat = if result.landed() {
            let state = &self.spells[spell];
            (result.damage * state.threat_multiplier + state.flat_threat_bonus)
                * self.player.threat_multiplier
        } else {
            0.0
        };
        result
    }

    /// Go `Spell.CalcAndDealPeriodicDamage` for a dot's tick on a base amount.
    pub(crate) fn periodic_damage_tick(&mut self, dot: super::DotId, base: f64) {
        let outcome = if self.dots[dot].tick_can_crit {
            Outcome::TickMagicCrit
        } else {
            Outcome::Tick
        };
        self.periodic_damage_tick_outcome(dot, base, outcome);
    }

    /// The same on a given outcome applier, as a damage proc's dot names its row's.
    pub(crate) fn periodic_damage_tick_outcome(
        &mut self,
        dot: super::DotId,
        base: f64,
        outcome: Outcome,
    ) {
        let state = &self.dots[dot];
        let (spell, side) = (state.spell, state.side);
        let mut base = base;
        if state.bonus_coefficient > 0.0 {
            // Go CalcPeriodicDamage, whose share the arm64 build fuses into the add.
            base = state
                .bonus_coefficient
                .mul_add(self.bonus_damage(spell, side), base);
        }
        let attacker =
            self.attacker_multiplier(spell, true) * self.dots[dot].periodic_damage_multiplier;
        let result = self.calc_damage_internal(spell, side, base, attacker, outcome);
        self.deal_damage(spell, result, true);
    }

    /// Go `Spell.CalcPeriodicDamage` for a dot's tick on a base amount against a target, not
    /// yet dealt.
    pub(crate) fn calc_periodic_damage(
        &mut self,
        dot: super::DotId,
        side: Side,
        base: f64,
        outcome: Outcome,
    ) -> SpellResult {
        let state = &self.dots[dot];
        let spell = state.spell;
        let mut base = base;
        if state.bonus_coefficient > 0.0 {
            // Go CalcPeriodicDamage, whose share the arm64 build fuses into the add.
            base = state
                .bonus_coefficient
                .mul_add(self.school_bonus_damage(spell, side), base);
        }
        let attacker =
            self.attacker_multiplier(spell, true) * self.dots[dot].periodic_damage_multiplier;
        self.calc_damage_internal(spell, side, base, attacker, outcome)
    }

    /// Go `Spell.CalcAndDealPeriodicDamage` for a dot's tick on a base amount against a
    /// target, with a given outcome applier; an area dot on the caster names its target.
    pub(crate) fn periodic_damage_tick_with(
        &mut self,
        dot: super::DotId,
        side: Side,
        base: f64,
        outcome: Outcome,
    ) -> SpellResult {
        let result = self.calc_periodic_damage(dot, side, base, outcome);
        let spell = self.dots[dot].spell;
        self.deal_damage(spell, result, true);
        result
    }

    /// Go `Spell.CalcPeriodicAoeDamage`: a fixed amount calculated on each target in unit
    /// index order, none dealt yet, so a hit cannot change the next one's calculation.
    pub(crate) fn calc_periodic_aoe_damage(
        &mut self,
        dot: super::DotId,
        base: f64,
        outcome: Outcome,
    ) -> AoeResults {
        let mut results = AoeResults::default();
        for position in 0..self.targets.len() {
            results.push(self.calc_periodic_damage(dot, Side::target(position), base, outcome));
        }
        results
    }

    /// Go `Spell.DealBatchedPeriodicDamage`: each result of an earlier calculation, in order.
    pub(crate) fn deal_batched_periodic_damage(&mut self, spell: SpellId, results: &AoeResults) {
        for &result in results.as_slice() {
            self.deal_damage(spell, result, true);
        }
    }

    /// Go `Dot.CalcAndDealPeriodicSnapshotDamage` for a dot built by `Snapshot`, which ticks
    /// on the caster's current spell power and attacker multiplier.
    pub(crate) fn snapshot_dot_tick(&mut self, dot: super::DotId) {
        self.snapshot_dot_tick_result(dot);
    }

    /// [`Self::snapshot_dot_tick`], returning the tick's result as Go does.
    pub(crate) fn snapshot_dot_tick_result(&mut self, dot: super::DotId) -> SpellResult {
        let result = self.snapshot_dot_tick_calc(dot);
        let spell = self.dots[dot].spell;
        self.deal_damage(spell, result, true);
        result
    }

    /// Go `Dot.CalcSnapshotDamage` for a dot built by `Snapshot`: the tick's result, not yet
    /// dealt.
    pub(crate) fn snapshot_dot_tick_calc(&mut self, dot: super::DotId) -> SpellResult {
        let side = self.dots[dot].side;
        self.snapshot_dot_tick_calc_on(dot, side)
    }

    /// The same against a target, as an area dot on its caster ticks on each target: the
    /// spell power share is the target's.
    pub(crate) fn snapshot_dot_tick_calc_on(
        &mut self,
        dot: super::DotId,
        side: Side,
    ) -> SpellResult {
        let outcome = if self.dots[dot].tick_can_crit {
            Outcome::TickMagicCrit
        } else {
            Outcome::Tick
        };
        self.snapshot_dot_tick_calc_with(dot, side, outcome)
    }

    /// The same with the outcome applier the dot's spell names, as Volley's ticks roll the
    /// ranged table.
    pub(crate) fn snapshot_dot_tick_calc_with(
        &mut self,
        dot: super::DotId,
        side: Side,
        outcome: Outcome,
    ) -> SpellResult {
        let state = &self.dots[dot];
        let spell = state.spell;
        let mut base = state.snapshot_base;
        if state.reads_spell_power {
            // Go currentTickInputs: the share less the snapshot's, fused.
            base += state
                .bonus_coefficient
                .mul_add(self.bonus_damage(spell, side), -state.snapshot_spell_power);
        }
        if state.attack_power_share != 0.0 {
            base += state
                .attack_power_share
                .mul_add(self.dot_attack_power(dot), -state.snapshot_attack_power);
        }
        let attacker =
            self.attacker_multiplier(spell, true) * self.dots[dot].periodic_damage_multiplier;
        self.calc_damage_internal(spell, side, base, attacker, outcome)
    }

    /// Go `Spell.CalcPeriodicDamage` with an expected outcome: the spell's dot's share of the
    /// bonus damage on the base, the attacker and periodic multipliers, the damage only.
    pub(crate) fn expected_periodic_damage(
        &mut self,
        spell: SpellId,
        dot: super::DotId,
        target: Side,
        mut base: f64,
        outcome: Outcome,
    ) -> f64 {
        let coefficient = self.dots[dot].bonus_coefficient;
        if coefficient > 0.0 {
            // Go `baseDamage += coefficient * BonusDamage`, which the arm64 build fuses.
            base = coefficient.mul_add(self.bonus_damage(spell, target), base);
        }
        let attacker =
            self.attacker_multiplier(spell, true) * self.dots[dot].periodic_damage_multiplier;
        self.calc_damage_internal(spell, target, base, attacker, outcome)
            .damage
    }

    /// Go `Spell.TravelTime`.
    pub(crate) fn travel_time(&self, spell: SpellId) -> i64 {
        let speed = self.spells[spell].missile_speed;
        if speed == 0.0 {
            0
        } else {
            (crate::core::time::NS_PER_SECOND as f64
                * self.unit_config(self.caster(spell)).distance
                / speed) as i64
        }
    }

    /// Go `WaitTravelTime` followed by `DealDamage` on arrival. Each travel counts its spell in
    /// flight until it lands, as Go `RegisterTravelTimeCallback` does.
    pub(crate) fn deal_damage_after_travel(&mut self, spell: SpellId, result: SpellResult) {
        let at = self.now + self.travel_time(spell);
        self.spells[spell].in_flight += 1;
        self.schedule(
            at,
            PRIORITY_GCD,
            Action::Travel {
                spell,
                result,
                dot: None,
            },
        );
    }

    /// Go `WaitTravelTime` around `DealDamage` of each of a cast's results, in order: one
    /// pending action for them all.
    pub(crate) fn deal_damage_after_travel_batch(
        &mut self,
        spell: SpellId,
        results: &[SpellResult],
    ) {
        debug_assert!(
            !results.is_empty(),
            "a travel batch carries its spell's results"
        );
        let at = self.now + self.travel_time(spell);
        self.spells[spell].in_flight += 1;
        let batch = match self.free_travel_batches.pop() {
            Some(batch) => batch,
            None => {
                self.travel_batches.push(Vec::new());
                self.travel_batches.len() - 1
            }
        };
        self.travel_batches[batch].extend(results.iter().map(|&result| (spell, result)));
        self.schedule(at, PRIORITY_GCD, Action::TravelBatch(batch));
    }

    /// Go `WaitTravelTime` with a class callback, run by [`Agent::on_travel`] on arrival.
    pub(crate) fn class_after_travel(&mut self, spell: SpellId, result: SpellResult) {
        let at = self.now + self.travel_time(spell);
        self.spells[spell].in_flight += 1;
        self.schedule(at, PRIORITY_GCD, Action::ClassTravel { spell, result });
    }

    /// The same, then `Dot.Apply` when the hit landed.
    pub(crate) fn deal_damage_after_travel_then_dot(
        &mut self,
        spell: SpellId,
        result: SpellResult,
        dot: super::DotId,
    ) {
        let at = self.now + self.travel_time(spell);
        self.spells[spell].in_flight += 1;
        self.schedule(
            at,
            PRIORITY_GCD,
            Action::Travel {
                spell,
                result,
                dot: Some(dot),
            },
        );
    }

    /// Go `dealDamageInternal`.
    pub(crate) fn deal_damage(&mut self, spell: SpellId, result: SpellResult, periodic: bool) {
        let partial = result.outcome & OUTCOME_PARTIAL != 0;
        if self.now >= 0 {
            let metrics = &mut self.spells[spell].metrics[result.target.index()];
            metrics.total_damage += result.damage;
            if partial {
                metrics.total_resisted_damage += result.damage;
            }
            if periodic {
                metrics.total_tick_damage += result.damage;
                if partial {
                    metrics.total_resisted_tick_damage += result.damage;
                }
            }
            let blocked = result.outcome & OUTCOME_BLOCK != 0;
            if blocked && result.crit() {
                metrics.total_blocked_crit_damage += result.damage;
            } else if result.crit() {
                metrics.total_crit_damage += result.damage;
                if partial {
                    metrics.total_resisted_crit_damage += result.damage;
                }
                if periodic {
                    metrics.total_crit_tick_damage += result.damage;
                    if partial {
                        metrics.total_resisted_crit_tick_damage += result.damage;
                    }
                }
            } else if result.outcome & OUTCOME_GLANCE != 0 {
                metrics.total_glance_damage += result.damage;
            } else if blocked {
                metrics.total_block_damage += result.damage;
            }
            metrics.total_threat += result.threat;
            // A landed result with no damage is an application (a dot's or a debuff's), not a
            // hit. Glancing, blocked and crushing blows have totals of their own.
            if result.landed() && result.damage > 0.0 {
                if blocked || result.outcome & (OUTCOME_GLANCE | OUTCOME_CRUSH) != 0 {
                } else if result.crit() && periodic {
                    metrics.crit_tick_range.add(result.damage);
                } else if result.crit() {
                    metrics.crit_range.add(result.damage);
                } else if periodic {
                    metrics.tick_range.add(result.damage);
                } else {
                    metrics.hit_range.add(result.damage);
                }
            }
        }
        if result.target == Side::Target {
            self.encounter_damage_taken += result.damage;
            if result.damage > 0.0 {
                // Pre-pull source hits are genuine target damage. Keep their
                // negative native clock; gsim applies them at the first t=0
                // boundary, before the next native combat decision.
                // Source-native Fight owns spell caster; the gsim world
                // must not misattribute a real pet hit to its player.
                let caster = self.caster(spell);
                let (caster_type, caster_index, caster_label) = match caster {
                    Side::Player => ("", 0, String::new()),
                    Side::Pet(index) => ("pet", index as u32, self.label_of(caster)),
                    _ => panic!("RUST_WORLD_UNSUPPORTED_DAMAGE_CASTER"),
                };
                super::world_bridge::record_damage(
                    self.now, result.damage, result.crit(), result.landed(), periodic,
                    format!("{:?}", self.spells[spell].id), self.label_of(result.target),
                    caster_type, caster_index, caster_label,
                );
            }
        }
        if self.log.is_some() && !self.spells[spell].flags.no_logs {
            let label = self.label_of(result.target);
            let line = format!(
                "[{}] {}{} {} (SpellSchool: {}). (Threat: {:.3})",
                label,
                action_string(&self.spells[spell].id),
                if periodic { " tick" } else { "" },
                result.damage_string(),
                self.spells[spell].school,
                result.threat
            );
            self.unit_log(self.caster(spell), &line);
        }
        if !self.spells[spell].flags.no_on_damage_dealt {
            if periodic {
                self.on_periodic_damage(spell, &result);
            } else {
                self.on_spell_hit(spell, &result);
            }
        }
    }
}

impl<A: Agent> Fight<A> {
    /// Go `spelldata.Effect.Roll` for a spell's exported client damage effect.
    pub(crate) fn roll_damage_effect(&mut self, spell: SpellId) -> f64 {
        let (average, variance) = self.spells[spell]
            .damage_effect
            .expect("spell has a damage effect");
        if variance == 0.0 {
            return average;
        }
        self.effect_roll(average, variance)
    }

    /// Go `Simulation.Roll`: min + (max - min) * roll, which the arm64 build fuses into one
    /// rounding wherever it inlines it.
    pub(crate) fn go_roll(&mut self, min: f64, max: f64) -> f64 {
        let roll = self.random("Damage Roll");
        (max - min).mul_add(roll, min)
    }

    /// Go spelldata `Effect.Roll` as the arm64 build computes it: the bounds' factors
    /// 1 -+ variance/2 fused, the span the upper bound less the fused average times the lower
    /// factor, and the roll fused into the lower bound plus span. No draw without a variance.
    pub(crate) fn effect_roll(&mut self, average: f64, variance: f64) -> f64 {
        if variance == 0.0 {
            return average;
        }
        let low = (-variance).mul_add(0.5, 1.0);
        let high = variance.mul_add(0.5, 1.0);
        let (min, max) = (average * low, high * average);
        let span = (-average).mul_add(low, max);
        let roll = self.random("Damage Roll");
        span.mul_add(roll, min)
    }
}
