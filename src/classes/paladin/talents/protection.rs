//! The tanking spells and Protection talents, from Go sim/paladin/righteous_fury.go,
//! swift_judgement.go, templars_bulwark.go, holy_shield.go and talents_protection.go.

use crate::{
    contracts::prepared_v2::Effect,
    core::{
        fight::{
            AuraRef, Fight, ModId, ModKind, Side, SpellId, SpellResult, OUTCOME_BLOCK,
            OUTCOME_CRIT, OUTCOME_LANDED,
        },
        time::STARTING_CD_TIME,
    },
};

use super::super::{agent::PaladinAgent, spells::seals};

/// The stat aura bit of a class aura whose gain changes the stats or the target's swings.
fn stat_bit(effects: &[Effect], fight: &Fight<PaladinAgent>, aura: AuraRef) -> Option<u32> {
    Fight::<PaladinAgent>::stat_aura_bit(effects, &fight.aura(aura).label)
}

/// Go `AddStatsDynamic` for a class aura that is also a stat aura.
fn toggle_stats(fight: &mut Fight<PaladinAgent>, bit: Option<u32>, active: bool) {
    if let Some(bit) = bit {
        fight.set_stat_aura(bit, active);
    }
}

/// Righteous Fury: a Holy threat mod while it holds; Improved Righteous Fury's damage taken
/// rides on the stat aura combinations; Instrument of Law's threat reduction holds only while
/// it is down.
#[derive(Clone, Debug)]
pub(crate) struct RighteousFury {
    pub(crate) aura: AuraRef,
    threat_mod: ModId,
    stat_bit: Option<u32>,
    /// Instrument of Law's aura and its multiplier on the paladin's threat.
    pub(crate) law: Option<(AuraRef, f64)>,
    /// Instrument of Law's first gain each fight is its reset activation, whose threat the
    /// prepared value already holds.
    pub(crate) law_at_reset: bool,
    /// Push 113554 under Forever's rules: the multiplier on all the paladin's threat while the
    /// aura holds, 1 otherwise.
    pub(crate) all_threat: f64,
}

impl RighteousFury {
    pub(crate) fn bind(
        fight: &mut Fight<PaladinAgent>,
        effects: &[Effect],
        aura: &str,
        threat_percent: f64,
        law: Option<(&str, f64)>,
    ) -> Result<Self, String> {
        let aura = fight.player_aura(aura)?;
        // The mod's School: every Holy spell, mods allowed.
        let affected = (0..fight.spells.len())
            .filter(|&spell| {
                let state = &fight.spells[spell];
                !state.flags.no_spell_mods && state.school & 2 != 0
            })
            .collect();
        let threat_mod = fight.register_mod(
            ModKind::ThreatMultiplierPercent,
            threat_percent,
            0,
            affected,
        );
        let law = match law {
            Some((label, multiplier)) => Some((fight.player_aura(label)?, multiplier)),
            None => None,
        };
        Ok(RighteousFury {
            aura,
            threat_mod,
            stat_bit: stat_bit(effects, fight, aura),
            law,
            law_at_reset: true,
            all_threat: 1.0,
        })
    }

    pub(crate) fn on_gain(&self, fight: &mut Fight<PaladinAgent>) {
        fight.activate_mod(self.threat_mod);
        toggle_stats(fight, self.stat_bit, true);
        if self.all_threat != 1.0 {
            fight.player.threat_multiplier *= self.all_threat;
        }
        if let Some((law, _)) = self.law {
            fight.deactivate_aura(law);
        }
    }

    pub(crate) fn on_expire(&self, fight: &mut Fight<PaladinAgent>) {
        fight.deactivate_mod(self.threat_mod);
        toggle_stats(fight, self.stat_bit, false);
        if self.all_threat != 1.0 {
            fight.player.threat_multiplier /= self.all_threat;
        }
        if let Some((law, _)) = self.law {
            fight.activate_aura(law);
        }
    }
}

/// Instrument of Law's threat aura gained: the reset activation is already in the prepared
/// threat multiplier.
pub(crate) fn law_gain(fight: &mut Fight<PaladinAgent>, multiplier: f64) {
    let fury = fight
        .agent
        .righteous_fury
        .as_mut()
        .expect("Righteous Fury is bound");
    if fury.law_at_reset {
        fury.law_at_reset = false;
        return;
    }
    fight.player.threat_multiplier *= multiplier;
}

pub(crate) fn law_expire(fight: &mut Fight<PaladinAgent>, multiplier: f64) {
    fight.player.threat_multiplier /= multiplier;
}

/// Swift Judgement: finishes Judgement's cooldown and makes the next Judgement free.
#[derive(Clone, Debug)]
pub(crate) struct SwiftJudgement {
    pub(crate) aura: AuraRef,
    cost_mod: ModId,
    judgement: Option<SpellId>,
}

impl SwiftJudgement {
    pub(crate) fn bind(
        fight: &mut Fight<PaladinAgent>,
        aura: &str,
        cost_percent_add: f64,
    ) -> Result<Self, String> {
        let aura = fight.player_aura(aura)?;
        let affected = fight.spells_with_class(&["judgement"]);
        let judgement = fight
            .spells
            .iter()
            .position(|spell| spell.class_spell.as_deref() == Some("judgement"));
        let cost_mod =
            fight.register_mod(ModKind::PowerCostPercentAdd, cost_percent_add, 0, affected);
        Ok(SwiftJudgement {
            aura,
            cost_mod,
            judgement,
        })
    }

    /// The major cooldown fires when there is a Judgement cooldown to finish and a seal up.
    pub(crate) fn should_activate(&self, fight: &Fight<PaladinAgent>) -> bool {
        let judgement = self.judgement.expect("Swift Judgement needs Judgement");
        !fight.spell_ready(judgement) && seals::active_seal(fight).is_some()
    }

    pub(crate) fn apply(&self, fight: &mut Fight<PaladinAgent>) {
        let judgement = self.judgement.expect("Swift Judgement needs Judgement");
        if let Some((timer, _)) = fight.spells[judgement].cd {
            fight.timers[timer] = STARTING_CD_TIME;
        }
        fight.activate_aura(self.aura);
    }

    pub(crate) fn on_gain(&self, fight: &mut Fight<PaladinAgent>) {
        fight.activate_mod(self.cost_mod);
    }

    pub(crate) fn on_expire(&self, fight: &mut Fight<PaladinAgent>) {
        fight.deactivate_mod(self.cost_mod);
    }

    /// A Judgement cast spends the free cast.
    pub(crate) fn on_cast_complete(&self, fight: &mut Fight<PaladinAgent>, spell: SpellId) {
        let state = &fight.spells[spell];
        if !state.flags.proc && state.class_spell.as_deref() == Some("judgement") {
            fight.deactivate_aura(self.aura);
        }
    }
}

/// Redoubt: landed melee hits taken can raise block chance for a number of blocks.
#[derive(Clone, Debug)]
pub(crate) struct Redoubt {
    pub(crate) aura: AuraRef,
    chance: f64,
    stat_bit: Option<u32>,
}

impl Redoubt {
    pub(crate) fn bind(
        fight: &Fight<PaladinAgent>,
        effects: &[Effect],
        aura: &str,
        chance: f64,
    ) -> Result<Self, String> {
        let aura = fight.player_aura(aura)?;
        Ok(Redoubt {
            aura,
            chance,
            stat_bit: stat_bit(effects, fight, aura),
        })
    }

    /// The trigger: a landed melee hit that dealt damage, at once.
    pub(crate) fn trigger(
        &self,
        fight: &mut Fight<PaladinAgent>,
        trigger: AuraRef,
        result: &SpellResult,
    ) {
        if result.outcome & OUTCOME_LANDED == 0 || result.damage == 0.0 {
            return;
        }
        if self.chance != 1.0 && fight.random_for_aura(trigger) > self.chance {
            return;
        }
        fight.activate_aura(self.aura);
        let max = fight.aura(self.aura).max_stacks;
        fight.set_stacks(self.aura, max);
    }

    /// The aura's own trigger: a block spends a stack.
    pub(crate) fn block(&self, fight: &mut Fight<PaladinAgent>, result: &SpellResult) {
        if result.outcome & OUTCOME_BLOCK != 0 {
            fight.remove_stack(self.aura);
        }
    }

    pub(crate) fn toggle(&self, fight: &mut Fight<PaladinAgent>, active: bool) {
        toggle_stats(fight, self.stat_bit, active);
    }
}

/// Shield Specialization: a block can restore a share of maximum mana, behind its cooldown.
#[derive(Clone, Debug)]
pub(crate) struct ShieldSpecialization {
    pub(crate) chance: f64,
    pub(crate) mana_share: f64,
    pub(crate) metrics: usize,
}

impl ShieldSpecialization {
    pub(crate) fn trigger(
        &self,
        fight: &mut Fight<PaladinAgent>,
        trigger: AuraRef,
        result: &SpellResult,
    ) {
        if result.outcome & OUTCOME_BLOCK == 0 {
            return;
        }
        let icd = fight.aura(trigger).icd;
        if let Some((timer, _)) = icd {
            if fight.timers[timer] > fight.now {
                return;
            }
        }
        if self.chance != 1.0 && fight.random_for_aura(trigger) > self.chance {
            return;
        }
        if let Some((timer, duration)) = icd {
            fight.timers[timer] = fight.now + duration;
        }
        let amount = fight.player.powers.max_mana * self.mana_share;
        fight.add_mana(amount, self.metrics);
    }
}

/// Reckoning: a block or a crit taken can pull the next main hand swing to now, a batch window
/// later.
#[derive(Clone, Debug)]
pub(crate) struct Reckoning {
    pub(crate) block_chance: f64,
    pub(crate) crit_chance: f64,
    /// The charges (16 October 2026 beta notes), spent by the next main hand auto.
    pub(crate) charges: AuraRef,
    /// The triggers' shared cooldown: its timer and duration.
    pub(crate) icd: (usize, i64),
    /// The spells that spend the charges, by spellbook position.
    pub(crate) spends: Vec<bool>,
}

impl Reckoning {
    pub(crate) fn trigger(
        &self,
        fight: &mut Fight<PaladinAgent>,
        trigger: AuraRef,
        block: bool,
        result: &SpellResult,
    ) {
        let (outcome, chance) = if block {
            (OUTCOME_BLOCK, self.block_chance)
        } else {
            (OUTCOME_CRIT, self.crit_chance)
        };
        if result.outcome & outcome == 0 {
            return;
        }
        if chance != 1.0 && fight.random_for_aura(trigger) > chance {
            return;
        }
        // The swing that was taken is not a player spell; the handler reads none.
        fight.schedule_delayed_proc(trigger, 0, *result);
    }

    /// The trigger's handler: a charge more, up to the aura's maximum, behind the shared
    /// cooldown.
    pub(crate) fn add_charge(&self, fight: &mut Fight<PaladinAgent>) {
        let (timer, duration) = self.icd;
        if fight.timers[timer] > fight.now {
            return;
        }
        fight.timers[timer] = fight.now + duration;
        fight.activate_aura(self.charges);
        fight.add_stack(self.charges);
    }

    /// The charges' OnSpellHitDealt: a main hand auto, landed or not, spends them after its own
    /// swing resolves, in an action now, so the other hit listeners keep their order.
    pub(crate) fn on_charges_hit(
        &self,
        fight: &mut Fight<PaladinAgent>,
        spell: SpellId,
        result: &SpellResult,
    ) {
        if !self.spends[spell] {
            return;
        }
        fight.schedule(
            fight.now,
            crate::core::fight::PRIORITY_GCD,
            crate::core::fight::Action::DelayedProc {
                aura: self.charges,
                spell,
                result: *result,
                token: None,
            },
        );
    }

    /// The spend: every charge goes, then one free main hand swing each, which leaves the swing
    /// timer alone (Go `InstantMHAttack`).
    pub(crate) fn spend(&self, fight: &mut Fight<PaladinAgent>) {
        let charges = fight.aura(self.charges).stacks;
        fight.deactivate_aura(self.charges);
        for _ in 0..charges {
            fight.instant_mh_attack();
        }
    }
}

/// Iron Creed: a landed Holy Strike under Righteous Fury lowers damage taken, a batch window
/// later.
#[derive(Clone, Debug)]
pub(crate) struct IronCreed {
    pub(crate) aura: AuraRef,
    stat_bit: Option<u32>,
}

impl IronCreed {
    pub(crate) fn bind(
        fight: &Fight<PaladinAgent>,
        effects: &[Effect],
        aura: &str,
    ) -> Result<Self, String> {
        let aura = fight.player_aura(aura)?;
        Ok(IronCreed {
            aura,
            stat_bit: stat_bit(effects, fight, aura),
        })
    }

    pub(crate) fn trigger(
        &self,
        fight: &mut Fight<PaladinAgent>,
        trigger: AuraRef,
        spell: SpellId,
        result: &SpellResult,
    ) {
        let state = &fight.spells[spell];
        if state.flags.proc
            || state.class_spell.as_deref() != Some("holy_strike")
            || result.outcome & OUTCOME_LANDED == 0
        {
            return;
        }
        let fury = fight.agent.righteous_fury.as_ref().map(|fury| fury.aura);
        if !fury.is_some_and(|fury| fight.aura(fury).active) {
            return;
        }
        fight.schedule_delayed_proc(trigger, spell, *result);
    }

    pub(crate) fn toggle(&self, fight: &mut Fight<PaladinAgent>, active: bool) {
        toggle_stats(fight, self.stat_bit, active);
    }
}

/// Holy Shield: block chance and charges that blocks spend to deal Holy damage.
#[derive(Clone, Debug)]
pub(crate) struct HolyShield {
    pub(crate) aura: AuraRef,
    pub(crate) proc_spell: SpellId,
    pub(crate) charges: i32,
    pub(crate) damage: f64,
    stat_bit: Option<u32>,
}

impl HolyShield {
    pub(crate) fn bind(
        fight: &Fight<PaladinAgent>,
        effects: &[Effect],
        aura: &str,
        proc_spell: SpellId,
        charges: i32,
        damage: f64,
    ) -> Result<Self, String> {
        let aura = fight.player_aura(aura)?;
        Ok(HolyShield {
            aura,
            proc_spell,
            charges,
            damage,
            stat_bit: stat_bit(effects, fight, aura),
        })
    }

    pub(crate) fn apply(&self, fight: &mut Fight<PaladinAgent>) {
        fight.activate_aura(self.aura);
        fight.set_stacks(self.aura, self.charges);
    }

    /// A blocked swing: the Holy damage on the attacker, then a charge spent.
    pub(crate) fn block(&self, fight: &mut Fight<PaladinAgent>, result: &SpellResult) {
        if result.outcome & OUTCOME_BLOCK == 0 {
            return;
        }
        fight.cast(self.proc_spell, result.attacker);
        fight.remove_stack(self.aura);
    }

    /// The proc's damage: a binary magic hit.
    pub(crate) fn proc_damage(
        &self,
        fight: &mut Fight<PaladinAgent>,
        spell: SpellId,
        target: Side,
    ) {
        let result = fight.calc_damage_hit_only(spell, target, self.damage);
        fight.deal_damage(spell, result, false);
    }

    pub(crate) fn toggle(&self, fight: &mut Fight<PaladinAgent>, active: bool) {
        toggle_stats(fight, self.stat_bit, active);
    }
}

/// Templar's Bulwark: an absorb shield of a share of maximum health, which spends its strength
/// on the damage the target's swings deal after their outcome, and Forbearance.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TemplarsBulwark {
    pub(crate) aura: AuraRef,
    pub(crate) forbearance: AuraRef,
    pub(crate) health_share: f64,
    /// Go `DamageAbsorptionAura.ShieldStrength`.
    pub(crate) strength: f64,
}

impl TemplarsBulwark {
    /// The cast: the shield, whose strength sets its stacks, then Forbearance.
    pub(crate) fn apply(mut self, fight: &mut Fight<PaladinAgent>) {
        fight.activate_aura(self.aura);
        self.strength = fight.player_max_health() * self.health_share;
        let stacks = (self.strength as i32).max(1);
        fight.aura_mut(self.aura).max_stacks = stacks;
        fight.agent.templars_bulwark = Some(self);
        fight.set_stacks(self.aura, stacks);
        fight.activate_aura(self.forbearance);
    }

    /// The shield's damage taken modifier: while it holds, it absorbs what it can of a hit
    /// that deals damage, and fades when spent.
    pub(crate) fn absorb(fight: &mut Fight<PaladinAgent>, result: &mut SpellResult) {
        let Some(mut shield) = fight.agent.templars_bulwark else {
            return;
        };
        if !fight.aura(shield.aura).active || result.damage <= 0.0 {
            return;
        }
        let absorbed = shield.strength.min(result.damage);
        result.damage -= absorbed;
        shield.strength -= absorbed;
        fight.agent.templars_bulwark = Some(shield);
        if fight.log.is_some() {
            let line = format!(
                "{} absorbed {absorbed:.1} damage, new shield strength: {:.1}",
                fight.aura(shield.aura).label,
                shield.strength
            );
            fight.player_log(&line);
        }
        if shield.strength <= 0.0 {
            fight.deactivate_aura(shield.aura);
            return;
        }
        fight.set_stacks(shield.aura, shield.strength as i32);
    }
}
