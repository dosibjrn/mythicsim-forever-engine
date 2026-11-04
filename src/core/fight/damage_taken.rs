//! The player taking damage: Go spell_result.go `CalcDamage` for a magic spell that hits the
//! player, on the player's attack table against itself, and health.go `trackChanceOfDeath`,
//! whose listener removes the health a landed hit takes and marks the player dead at zero.
//!
//! The Goblin Sapper Charge, from Go consumes.go `newBasicExplosiveSpellConfig`, is the one
//! spell in scope that hits the player: its rolled Fire hit lands on the target, then a second
//! roll lands on the thrower.

use crate::{
    contracts::prepared_v2::{AttackTable, PreparedV2, Schools},
    mechanics::damage::{
        crit_damage_multiplier, partial_resist_thresholds, resist_coefficient, spell_chance_to_miss,
    },
};

use super::{
    damage::{OUTCOME_CRIT, OUTCOME_HIT, OUTCOME_MISS, OUTCOME_PARTIAL_1_4, OUTCOME_PARTIAL_2_4},
    damage::{OUTCOME_PARTIAL, OUTCOME_PARTIAL_3_4},
    log::action_string,
    Action, Agent, Fight, Side, SpellId, SpellResult,
};

/// The player as the defender of its own spells: its attack table against itself and its
/// defensive stats, which nothing in scope changes during a fight. Its school damage taken
/// multipliers are the player's live ones.
#[derive(Clone, Debug)]
pub(crate) struct SelfTarget {
    table: AttackTable,
    resistance: [f64; 8],
    school_bonus_spell_damage: [f64; 8],
    bonus_spell_damage_taken: f64,
    bonus_physical_damage_taken: f64,
    /// The crit the player takes, which an item swap can change.
    pub(crate) reduced_crit_taken_percent: f64,
}

/// The Goblin Sapper Charge's two rolls.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GoblinSapper {
    pub(crate) min_damage: f64,
    pub(crate) max_damage: f64,
    pub(crate) aoe_cap_multiplier: f64,
    /// The spell of the half that hits the player.
    pub(crate) self_spell: SpellId,
}

/// Go `UnitMetrics.Died` and the iterations the player died in.
#[derive(Clone, Debug, Default)]
pub(crate) struct Death {
    pub(crate) died: bool,
    pub(crate) iterations_dead: u32,
    /// Go `deathSeeds`: the seed of each iteration the player died in, sorted.
    pub(crate) seeds: Vec<i64>,
}

fn schools(values: &Schools) -> [f64; 8] {
    [
        values.none,
        values.physical,
        values.arcane,
        values.fire,
        values.frost,
        values.holy,
        values.nature,
        values.shadow,
    ]
}

impl SelfTarget {
    pub(crate) fn new(prepared: &PreparedV2, table: &AttackTable) -> Result<Self, String> {
        let player = &prepared.player;
        let stat = |name: &str| {
            player
                .stats
                .get(name)
                .copied()
                .ok_or_else(|| format!("prepared stats lack {name}"))
        };
        let pseudo = &player.pseudo_stats;
        Ok(SelfTarget {
            table: table.clone(),
            resistance: [
                0.0,
                stat("Armor")?,
                stat("ArcaneResistance")?,
                stat("FireResistance")?,
                stat("FrostResistance")?,
                0.0,
                stat("NatureResistance")?,
                stat("ShadowResistance")?,
            ],
            school_bonus_spell_damage: schools(&pseudo.school_bonus_spell_damage),
            bonus_spell_damage_taken: pseudo.bonus_spell_damage_taken,
            bonus_physical_damage_taken: 0.0,
            reduced_crit_taken_percent: pseudo.reduced_crit_taken_percent,
        })
    }
}

impl<A: Agent> Fight<A> {
    fn self_target(&self) -> &SelfTarget {
        self.self_target
            .as_ref()
            .expect("a spell that hits the player has the player's own attack table")
    }

    /// Go `AttachMultiplicativePseudoStatBuff` on the player's school damage taken: multiply on
    /// gain, divide on expiry. Spells that hit the player and the target's swings read it.
    pub(crate) fn multiply_self_damage_taken(
        &mut self,
        multiplier: f64,
        schools: [bool; 8],
        undo: bool,
    ) {
        let live = &mut self.player.school_damage_taken_multiplier;
        for (index, applies) in schools.into_iter().enumerate() {
            if applies {
                if undo {
                    live[index] /= multiplier;
                } else {
                    live[index] *= multiplier;
                }
            }
        }
    }

    /// Go `CalcDamage` of a magic spell on the player with `OutcomeMagicHitAndCrit`, or with
    /// `OutcomeMagicHit` when it cannot crit.
    pub(crate) fn calc_damage_on_player(
        &mut self,
        spell: SpellId,
        base_damage: f64,
        can_crit: bool,
    ) -> SpellResult {
        let defender = self.self_target().clone();
        let state = &self.spells[spell];
        // Go AttackerDamageMultiplier on the player's own table.
        let attacker = if state.flags.ignore_attacker_modifiers {
            1.0
        } else {
            self.player.damage_dealt_multiplier
                * self.school_value(spell, &self.player.school_damage_dealt_multiplier)
                * defender.table.damage_dealt_multiplier
                * state.damage_multiplier
                * (state.damage_multiplier_additive + state.direct_damage_multiplier_additive)
        };
        let mut base = base_damage;
        if state.bonus_coefficient > 0.0 {
            // Go BonusDamage for a magic spell, with the player's school bonus spell damage.
            let bonus = state.bonus_base_damage
                + self.spell_power(spell)
                + self.school_value(spell, &defender.school_bonus_spell_damage);
            base += state.bonus_coefficient * bonus;
        }
        let mut result = SpellResult {
            armor_multiplier: 0.0,
            target: Side::Player,
            attacker: self.spells[spell].caster,
            outcome: 0,
            damage: base * attacker,
            threat: 0.0,
        };
        let after_attacker = result.damage;
        let mut resistance_multiplier = 1.0;
        let state = &self.spells[spell];
        let binary = state.flags.binary;
        if !state.flags.ignore_resists && !binary {
            let resistance = self.player_resistance(&defender.resistance)[state.school_index];
            let roll = self.random("Partial Resist");
            let coefficient = resist_coefficient(
                resistance,
                self.player.powers.spell_piercing,
                self.config.player_level,
                self.config.player_level,
                false,
            );
            let (none, quarter, half) = partial_resist_thresholds(coefficient);
            let (multiplier, outcome) = if roll > none {
                (1.0, 0)
            } else if roll > quarter {
                (0.75, OUTCOME_PARTIAL_1_4)
            } else if roll > half {
                (0.5, OUTCOME_PARTIAL_2_4)
            } else {
                (0.25, OUTCOME_PARTIAL_3_4)
            };
            result.damage *= multiplier;
            resistance_multiplier = multiplier;
            result.outcome |= outcome;
        }
        let after_resistances = result.damage;
        // Go applyResistances' ArmorAndResistanceMultiplier and PostArmorAndResistanceMultiplier,
        // which rage from damage taken reads.
        self.player_hit_resistance = (after_resistances, resistance_multiplier);
        let state = &self.spells[spell];
        if !state.flags.ignore_target_modifiers {
            if state.school & 1 != 0 {
                result.damage += defender.bonus_physical_damage_taken;
            } else if state.school_index > 1 {
                result.damage += defender.bonus_spell_damage_taken;
            }
            result.damage *= self.player.damage_taken_multiplier
                * self.school_value(spell, &self.player.school_damage_taken_multiplier)
                * defender.table.damage_taken_multiplier;
        }
        let after_target = result.damage;
        let partial = result.outcome & OUTCOME_PARTIAL;
        self.outcome_magic_on_player(spell, &mut result, &defender, can_crit);
        if partial != 0 {
            result.outcome |= partial;
        }
        let after_outcome = result.damage;
        // Go ApplyPostOutcomeDamageModifiers: the player's dynamic damage taken modifiers, as
        // an absorb shield, then the floor.
        // Go registers the item shields before the class's modifiers.
        let school = self.spells[spell].school;
        self.item_absorbs_taken(school, &mut result);
        A::player_damage_taken_modifiers(self, &mut result);
        result.damage = result.damage.max(0.0);
        if self.log.is_some() {
            let line = format!(
                "[{}] {} [DEBUG] MAP: {:.1}, RAP: {:.1}, SP: {:.1}, BaseDamage:{:.1}, AfterAttackerMods:{:.1}, AfterResistances:{:.1}, AfterTargetMods:{:.1}, AfterOutcome:{:.1}, AfterPostOutcome:{:.1}",
                self.config.player_label,
                action_string(&self.spells[spell].id),
                self.player.powers.attack_power,
                self.player.powers.ranged_attack_power,
                self.spell_power(spell),
                base,
                after_attacker,
                after_resistances,
                after_target,
                after_outcome,
                result.damage
            );
            self.player_log(&line);
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

    /// Go `outcomeMagicHitAndCrit` with hit counters on the player's own table.
    fn outcome_magic_on_player(
        &mut self,
        spell: SpellId,
        result: &mut SpellResult,
        defender: &SelfTarget,
        can_crit: bool,
    ) {
        let state = &self.spells[spell];
        let binary_hit = state.flags.binary.then(|| {
            let resistance = self.player_resistance(&defender.resistance)[state.school_index];
            let coefficient = resist_coefficient(
                resistance,
                self.player.powers.spell_piercing,
                self.config.player_level,
                self.config.player_level,
                true,
            );
            crate::mechanics::damage::binary_resist_hit(coefficient)
        });
        let miss = spell_chance_to_miss(
            defender.table.base_spell_miss_chance,
            binary_hit,
            self.spell_hit_chance(spell),
        );
        let target = Side::Player.index();
        if !self.proc(1.0 - miss, "Magical Hit Roll") {
            result.outcome = OUTCOME_MISS;
            result.damage = 0.0;
            self.spells[spell].metrics[target].misses += 1;
            return;
        }
        let partial = result.outcome & OUTCOME_PARTIAL != 0;
        let state = &self.spells[spell];
        let crit = (self.player.powers.spell_crit_percent
            + state.bonus_crit_percent
            + defender.table.bonus_spell_crit_percent
            - defender.reduced_crit_taken_percent)
            / 100.0
            - defender.table.spell_crit_suppression;
        if can_crit && self.proc(crit.max(0.0), "Magical Crit Roll") {
            result.outcome = OUTCOME_CRIT;
            let state = &self.spells[spell];
            result.damage *= crit_damage_multiplier(
                state.magic_defense,
                state.crit_multiplier_pct,
                self.config.crit_damage_multiplier,
                defender.table.crit_multiplier,
                state.crit_multiplier_additive,
            );
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

    /// Go `newBasicExplosiveSpellConfig`'s `ApplyEffects` for the Goblin Sapper Charge: one
    /// roll, `CalcAoeDamage` on every target, the batch dealt at once since it has no travel,
    /// then the roll on the player.
    pub(crate) fn apply_goblin_sapper(&mut self, spell: SpellId) {
        let sapper = self
            .goblin_sapper
            .expect("the Goblin Sapper Charge is bound");
        let roll = |fight: &mut Self| fight.go_roll(sapper.min_damage, sapper.max_damage);
        let base = roll(self) * sapper.aoe_cap_multiplier;
        for result in self.calc_aoe_damage(spell, base) {
            self.deal_damage(spell, result, false);
        }
        let base = roll(self);
        let result = self.calc_damage_on_player(sapper.self_spell, base, true);
        self.deal_damage(sapper.self_spell, result, false);
    }

    /// Go `newBasicExplosiveSpellConfig`'s `ApplyEffects` without the self hit: one roll
    /// scaled by the AoE cap, `CalcAoeDamage` on every target, and the batch dealt after
    /// travel when the explosive flies.
    pub(crate) fn apply_basic_explosive(
        &mut self,
        spell: SpellId,
        min: f64,
        max: f64,
        aoe_cap_multiplier: f64,
    ) {
        let base = self.go_roll(min, max) * aoe_cap_multiplier;
        let results = self.calc_aoe_damage(spell, base);
        if self.spells[spell].missile_speed > 0.0 {
            self.deal_damage_after_travel_batch(spell, &results);
        } else {
            for result in results {
                self.deal_damage(spell, result, false);
            }
        }
    }

    /// Go `Spell.CalcAoeDamage` with `OutcomeMagicHitAndCrit`: every target's result on the
    /// same base damage, in unit index order, before any is dealt.
    pub(crate) fn calc_aoe_damage(&mut self, spell: SpellId, base: f64) -> Vec<SpellResult> {
        (0..self.targets.len())
            .map(|position| self.calc_damage(spell, Side::target(position), base))
            .collect()
    }

    /// The Chance of Death listener's `OnSpellHitTaken`: a hit that deals damage removes that
    /// much health, the rotation reacts, and at zero health a pending action marks the player
    /// dead unless health came back first.
    pub(crate) fn chance_of_death_hit_taken(&mut self, result: &SpellResult) {
        if result.damage <= 0.0 {
            return;
        }
        self.remove_health(result.damage);
        self.react_to_event(Side::Player);
        if self.player.health <= 0.0 && !self.death.died {
            self.schedule(self.now, super::PRIORITY_GCD, Action::DeathCheck);
        }
    }

    /// The pending action's check, then Go `Character.Died`.
    pub(crate) fn death_check(&mut self) {
        if self.player.health <= 0.0 && !self.death.died {
            self.death.died = true;
            if self.log.is_some() {
                self.player_log("Dead");
            }
        }
    }
}
