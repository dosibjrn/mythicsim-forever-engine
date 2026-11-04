//! Go cast.go, spell.go, spell_queueing.go, gcd.go, mana.go and major_cooldown.go.

use crate::{
    contracts::prepared_v2::CastKind,
    core::time::{go_string, round, NS_PER_MILLISECOND, NS_PER_SECOND, STARTING_CD_TIME},
};

use super::{
    log::action_string, Action, Agent, AuraRef, EnergizedBar, Fight, ResourceKind, Side,
    SpellBehavior, SpellId, PRIORITY_GCD, SPELL_PUSHBACK_DURATION,
};

/// Go `MaxSpellQueueWindow`.
pub(crate) const MAX_SPELL_QUEUE_WINDOW: i64 = 400 * NS_PER_MILLISECOND;

impl<A: Agent> Fight<A> {
    /// Go `MultiplyCastSpeed` and `updateCastSpeed`. The undo multiplies by the reciprocal,
    /// as Go does, so the multiplier can drift from its starting value by rounding.
    pub(crate) fn multiply_cast_speed(&mut self, amount: f64) {
        self.player.cast_speed_multiplier *= amount;
        self.cast_speed = crate::mechanics::haste::cast_speed(
            self.player.cast_speed_multiplier,
            self.config.spell_haste_rating,
        );
    }

    /// Go `ApplyCastSpeed` of an acting unit.
    pub(crate) fn apply_cast_speed_of(&self, side: Side, duration: i64) -> i64 {
        (duration as f64 * self.unit_cast_speed(side)) as i64
    }

    /// Go `ApplyCastSpeedForSpell`.
    pub(crate) fn apply_cast_speed_for_spell(&self, duration: i64, spell: SpellId) -> i64 {
        (duration as f64
            * self.unit_cast_speed(self.caster(spell))
            * self.spells[spell].cast_time_multiplier.max(0.0)) as i64
    }

    /// A class spell's own `Spell.CastTime`, when its class replaces Go's default.
    pub(crate) fn class_cast_time(&self, spell: SpellId) -> Option<i64> {
        match self.spells[spell].behavior {
            SpellBehavior::Class(behavior) => A::cast_time(self, spell, behavior),
            _ => None,
        }
    }

    /// Go `SpellCost.GetCurrentCost`, with Go's int32 percentage arithmetic.
    pub(crate) fn current_cost(&self, spell: SpellId) -> f64 {
        let Some(cost) = self.spells[spell].cost else {
            return 0.0;
        };
        let mut value = (cost.base + cost.flat_modifier).max(0);
        value = (value * self.unit(self.caster(spell)).spell_cost_percent_modifier / 100).max(0);
        (f64::from(value) * cost.percent_modifier * cost.additive_percent_modifier).max(0.0)
    }

    fn timer_ready(&self, timer: Option<(usize, i64)>) -> bool {
        timer.is_none_or(|(id, _)| self.timers[id] <= self.now)
    }

    /// Go `BothTimersReady` for a spell's cooldown and shared cooldown.
    pub(crate) fn spell_ready(&self, spell: SpellId) -> bool {
        self.timer_ready(self.spells[spell].cd) && self.timer_ready(self.spells[spell].shared_cd)
    }

    /// Go `Spell.ReadyAt` through `BothTimersReadyAt`: the first timer's value replaces the
    /// zero default, so an unused cooldown reads as Go's negative starting time.
    pub(crate) fn spell_ready_at(&self, spell: SpellId) -> i64 {
        let mut ready = 0;
        if let Some((timer, _)) = self.spells[spell].cd {
            ready = self.timers[timer];
        }
        if let Some((timer, _)) = self.spells[spell].shared_cd {
            ready = ready.max(self.timers[timer]);
        }
        ready
    }

    /// Go `MaxTimeToReady`.
    pub(crate) fn spell_time_to_ready(&self, spell: SpellId) -> i64 {
        let remaining = |timer: Option<(usize, i64)>| {
            timer.map_or(0, |(id, _)| (self.timers[id] - self.now).max(0))
        };
        remaining(self.spells[spell].cd).max(remaining(self.spells[spell].shared_cd))
    }

    /// Go `GCD.IsReady`.
    pub(crate) fn gcd_ready(&self) -> bool {
        self.gcd_ready_of(Side::Player)
    }

    /// Go `GCD.IsReady` of an acting unit.
    pub(crate) fn gcd_ready_of(&self, side: Side) -> bool {
        self.unit(side).gcd <= self.now
    }

    /// Go `GCD.TimeToReady`.
    pub(crate) fn gcd_time_to_ready(&self) -> i64 {
        (self.player.gcd - self.now).max(0)
    }

    /// Go `Unit.CanQueueSpell`: one queued spell per timestep.
    pub(crate) fn can_queue_spell(&self) -> bool {
        self.can_queue_spell_of(Side::Player)
    }

    /// Go `Unit.CanQueueSpell` of an acting unit.
    pub(crate) fn can_queue_spell_of(&self, side: Side) -> bool {
        self.unit(side)
            .queued
            .is_none_or(|queued| queued.initiated_at != self.now)
    }

    /// The energy at which a spell's cost stops refusing it as energy rises: its energy cost,
    /// and no limit for a cost in another resource or none.
    pub(crate) fn energy_cost_limit(&self, spell: SpellId) -> f64 {
        if !A::CACHE_OVER_ENERGY {
            return f64::INFINITY;
        }
        match self.spells[spell].cost.map(|cost| cost.kind) {
            Some(ResourceKind::Energy) => self.current_cost(spell),
            _ => f64::INFINITY,
        }
    }

    /// For a spell whose extra cast condition just failed: until when the clock alone cannot
    /// make it pass, and what could (the range, then the class's word for its own condition).
    pub(crate) fn extra_condition_proof(&self, spell: SpellId) -> Option<(i64, u8)> {
        let state = &self.spells[spell];
        let distance = self.distance_now(state.caster);
        if (state.min_range != 0.0 && distance < state.min_range)
            || (state.max_range != 0.0 && distance > state.max_range)
        {
            return Some((crate::core::time::NEVER_EXPIRES, super::reads::CASTS));
        }
        match state.behavior {
            SpellBehavior::Class(behavior) if state.has_extra_cast_condition => {
                A::extra_condition_proof(self, spell, behavior)
            }
            _ => None,
        }
    }

    /// Go RegisterSpell's range check: the caster is within the spell's range now.
    pub(crate) fn in_cast_range(&self, spell: SpellId) -> bool {
        let state = &self.spells[spell];
        let distance = self.distance_now(state.caster);
        !((state.min_range != 0.0 && distance < state.min_range)
            || (state.max_range != 0.0 && distance > state.max_range))
    }

    pub(crate) fn extra_cast_condition(&mut self, spell: SpellId) -> bool {
        // Go RegisterSpell wraps the condition with the range check, which runs first.
        if !self.in_cast_range(spell) {
            return false;
        }
        match self.spells[spell].behavior {
            SpellBehavior::Class(behavior) if self.spells[spell].has_extra_cast_condition => {
                A::extra_cast_condition_logged(self, spell, behavior)
            }
            SpellBehavior::ExternalCooldown => self.external_cooldown_castable(spell),
            _ => true,
        }
    }

    /// Go `ResourceCostImpl.MeetsRequirement` for the spell's resource.
    fn meets_cost(&mut self, spell: SpellId) -> bool {
        let Some(kind) = self.spells[spell].cost.map(|cost| cost.kind) else {
            return true;
        };
        let cost = self.current_cost(spell);
        self.spells[spell].cur_cast.cost = cost;
        match kind {
            ResourceKind::Energy => self.energy_bar().current >= cost,
            ResourceKind::Focus => self.current_focus(self.caster(spell)) >= cost,
            ResourceKind::Rage => self.current_rage() >= cost,
            _ => self.meets_mana_cost(spell, cost),
        }
    }

    /// Go `ManaCost.MeetsRequirement`, including its out-of-mana bookkeeping.
    fn meets_mana_cost(&mut self, spell: SpellId, cost: f64) -> bool {
        let side = self.caster(spell);
        let now = self.now;
        let unit = self.unit_mut(side);
        let meets = unit.mana >= cost;
        // Go's OOM events leave a pet's metrics alone; only its end of iteration counts.
        let is_pet = side.is_pet();
        if cost > 0.0 {
            if meets {
                if unit.waiting_for_mana != 0.0 {
                    let duration = now - unit.waiting_for_mana_start;
                    unit.waiting_for_mana_start = 0;
                    unit.waiting_for_mana = 0.0;
                    if !is_pet {
                        self.add_oom_time(side, duration);
                    }
                }
            } else if unit.waiting_for_mana != 0.0 {
                unit.waiting_for_mana = unit.waiting_for_mana.min(cost);
            } else {
                unit.waiting_for_mana_start = now;
                unit.waiting_for_mana = cost;
                if !is_pet {
                    self.mark_oom(side);
                }
            }
        }
        meets
    }

    fn add_oom_time(&mut self, side: Side, duration: i64) {
        if duration > 0 {
            self.unit_mut(side).oom_time += duration;
            self.mark_oom(side);
        }
    }

    fn mark_oom(&mut self, side: Side) {
        let now = self.now;
        let unit = self.unit_mut(side);
        if !unit.went_oom {
            unit.went_oom = true;
            unit.first_oom = now;
        }
    }

    fn cost_failure(&self, spell: SpellId) -> String {
        let cost = self.spells[spell].cur_cast.cost;
        match self.spells[spell].cost.map(|cost| cost.kind) {
            Some(ResourceKind::Rage) => format!(
                "not enough rage (Current Rage = {:.3}, Rage Cost = {cost:.3})",
                self.current_rage()
            ),
            Some(ResourceKind::Focus) => format!(
                "not enough focus (Current Focus = {:.3}, Focus Cost = {cost:.3})",
                self.current_focus(self.caster(spell))
            ),
            Some(ResourceKind::Energy) => format!(
                "not enough energy (Current Energy = {:.3}, Energy Cost = {cost:.3})",
                self.energy_bar().current
            ),
            _ => format!(
                "not enough mana (Current Mana = {:.3}, Mana Cost = {cost:.3})",
                self.unit(self.caster(spell)).mana
            ),
        }
    }

    /// Go `castRequirementFailure`: the unit is in a form the requirement refuses, which is
    /// while one of the spell's requirement auras is active.
    pub(crate) fn wrong_form(&self, spell: SpellId) -> bool {
        self.spells[spell]
            .requirement_auras
            .iter()
            .any(|&aura| self.aura(aura).active)
    }

    /// Go `Spell.CanCompleteCast`. Cost checks have side effects, as in Go.
    pub(crate) fn can_complete_cast(&mut self, spell: SpellId, log_failure: bool) -> bool {
        if self.wrong_form(spell) {
            if log_failure {
                self.cast_failure(spell, |_| "wrong form".into());
            }
            return false;
        }
        if !self.extra_cast_condition(spell) {
            if log_failure {
                self.cast_failure(spell, |_| "extra spell condition".into());
            }
            return false;
        }
        if self.innervate_blocks(spell) {
            if log_failure {
                self.cast_failure(spell, |_| "Innervate is active".into());
            }
            return false;
        }
        if !self.meets_cost(spell) {
            if log_failure {
                self.cast_failure(spell, |fight| fight.cost_failure(spell));
            }
            return false;
        }
        true
    }

    /// Go `Spell.CanCast`.
    pub(crate) fn can_cast(&mut self, spell: SpellId) -> bool {
        if !self.can_complete_cast(spell, false) {
            return false;
        }
        let state = &self.spells[spell];
        let side = state.caster;
        if state.flags.swapped {
            return false;
        }
        // While moving only instant casts are possible.
        if !state.flags.can_cast_while_moving
            && state.default_cast.cast_time > 0
            && self.unit(side).moving
        {
            return false;
        }
        if self.unit(side).hardcast.expires > self.now {
            return false;
        }
        let in_sequence = side == Side::Player && self.in_sequence();
        let needs_gcd = state.default_cast.gcd > 0 || (state.flags.mcd && in_sequence);
        if needs_gcd && !self.gcd_ready_of(side) {
            return false;
        }
        self.spell_ready(spell)
    }

    /// Go `Rotation.inSequence`.
    fn in_sequence(&self) -> bool {
        self.apl.in_sequence
    }

    /// Go `Spell.CanQueue`.
    fn can_queue(&mut self, spell: SpellId) -> bool {
        if !self.can_complete_cast(spell, false) {
            return false;
        }
        let state = &self.spells[spell];
        let unit = self.unit(state.caster);
        if state.flags.swapped {
            return false;
        }
        if state.flags.channeled && unit.hardcast.expires > self.now + MAX_SPELL_QUEUE_WINDOW {
            return false;
        }
        if state.default_cast.gcd > 0 && (unit.gcd - self.now).max(0) > MAX_SPELL_QUEUE_WINDOW {
            return false;
        }
        self.spell_time_to_ready(spell) <= MAX_SPELL_QUEUE_WINDOW
    }

    /// Go `Unit.CanQueueSpell` and `Spell.CanQueue`.
    pub(crate) fn can_cast_or_queue(&mut self, spell: SpellId) -> bool {
        self.can_queue_spell_of(self.caster(spell)) && self.can_queue(spell)
    }

    /// Go `Spell.CastOrQueue`.
    pub(crate) fn cast_or_queue(&mut self, spell: SpellId, target: Side) {
        if self.can_cast(spell) {
            self.cast(spell, target);
        } else if self.can_queue(spell) {
            let unit = self.unit(self.caster(spell));
            let (expires, gcd) = (unit.hardcast.expires, unit.gcd);
            let mut queue_time = expires.max(self.spell_ready_at(spell));
            let state = &self.spells[spell];
            if state.default_cast.gcd > 0 || state.flags.mcd {
                queue_time = queue_time.max(gcd);
            }
            self.queue_spell(spell, target, queue_time);
        } else {
            self.cast(spell, target);
        }
    }

    /// Go `Unit.QueueSpell` and `QueuedSpell.InitiateQueue`.
    fn queue_spell(&mut self, spell: SpellId, target: Side, queue_at: i64) {
        let side = self.caster(spell);
        self.cancel_queued_spell_of(side);
        let fire_at = queue_at + 1;
        let action = self.schedule(fire_at, PRIORITY_GCD, Action::QueuedCast(side));
        let now = self.now;
        self.changed(super::reads::CASTS);
        self.unit_mut(side).queued = Some(super::QueuedSpell {
            spell,
            target,
            action: Some(action),
            initiated_at: now,
            fire_at,
        });
        if self.log.is_some() {
            let line = format!(
                "Queueing up {} to cast at {}.",
                action_string(&self.spells[spell].id),
                go_string(fire_at)
            );
            self.unit_log(side, &line);
        }
    }

    /// Go `QueuedSpell.Cancel` for an acting unit: the queue initiation time becomes
    /// -NeverExpires.
    pub(crate) fn cancel_queued_spell_of(&mut self, side: Side) {
        self.changed(super::reads::CASTS);
        if let Some(queued) = self.unit_mut(side).queued.as_mut() {
            let action = queued.action.take();
            queued.initiated_at = -crate::core::time::NEVER_EXPIRES;
            if let Some(action) = action {
                self.queue.cancel(action);
            }
        }
    }

    /// Go `castFailureHelper`. The reason is only formatted when logging.
    fn cast_failure(&mut self, spell: SpellId, reason: impl FnOnce(&Self) -> String) -> bool {
        if self.now >= 0 && !self.spells[spell].flags.no_logs && self.log.is_some() {
            let reason = reason(self);
            let line = format!(
                "{} failed to cast: {reason}",
                action_string(&self.spells[spell].id)
            );
            self.unit_log(self.caster(spell), &line);
        }
        false
    }

    /// Go `Spell.Cast`.
    pub(crate) fn cast(&mut self, spell: SpellId, target: Side) -> bool {
        if self.spells[spell].default_cast.effective_time() > 0 {
            self.cancel_queued_spell_of(self.caster(spell));
        }
        match self.spells[spell].cast_kind {
            CastKind::Full => self.cast_full(spell, target),
            CastKind::Simple => self.cast_simple(spell, target),
            CastKind::AutosOrProcs => {
                self.log_instant_cast(spell);
                self.apply_effects(spell, target);
                if !self.spells[spell].flags.no_on_cast_complete {
                    self.on_cast_complete(spell);
                }
                true
            }
        }
    }

    fn log_instant_cast(&mut self, spell: SpellId) {
        if self.log.is_some() && !self.spells[spell].flags.no_logs {
            let id = action_string(&self.spells[spell].id);
            let side = self.caster(spell);
            self.unit_log(
                side,
                &format!(
                    "Casting {id} (Cost = 0.000, Cast Time = 0s, GCD = 0s, Effective Time = 0s)"
                ),
            );
            self.unit_log(side, &format!("Completed cast {id}"));
        }
    }

    /// Go `makeCastFuncSimple`.
    fn cast_simple(&mut self, spell: SpellId, target: Side) -> bool {
        if self.spells[spell].flags.swapped {
            return self.cast_failure(spell, |_| "spell attached to an un-equipped item".into());
        }
        if self.wrong_form(spell) {
            return self.cast_failure(spell, |_| "wrong form".into());
        }
        if !self.extra_cast_condition(spell) {
            return self.cast_failure(spell, |_| "extra spell condition".into());
        }
        if let Some((timer, _)) = self.spells[spell].cd {
            if self.timers[timer] > self.now {
                return self.cast_failure(spell, |fight| {
                    format!(
                        "still on cooldown for {}, curTime = {}",
                        go_string((fight.timers[timer] - fight.now).max(0)),
                        go_string(fight.now)
                    )
                });
            }
        }
        if let Some((timer, _)) = self.spells[spell].shared_cd {
            if self.timers[timer] > self.now {
                return self.cast_failure(spell, |fight| {
                    format!(
                        "still on shared cooldown for {}, curTime = {}",
                        go_string((fight.timers[timer] - fight.now).max(0)),
                        go_string(fight.now)
                    )
                });
            }
        }
        self.log_instant_cast(spell);
        self.trigger_cooldowns(spell);
        self.apply_effects(spell, target);
        if !self.spells[spell].flags.no_on_cast_complete {
            self.on_cast_complete(spell);
        }
        true
    }

    /// Go `triggerCooldown` and the shared cooldown set.
    fn trigger_cooldowns(&mut self, spell: SpellId) {
        let state = &self.spells[spell];
        let multiplier = state.cd_multiplier;
        if let Some((timer, duration)) = state.cd {
            let cd = (duration as f64 * multiplier) as i64;
            if cd > 0 {
                self.timers[timer] = self.now + cd;
            }
        }
        if let Some((timer, duration)) = state.shared_cd {
            self.timers[timer] = self.now + (duration as f64 * multiplier) as i64;
        }
        self.changed(super::reads::TIMERS);
    }

    /// Go `Spell.SetMetricsSplit`: the spell's metrics, and the tag of its action ID in logs,
    /// follow the split until the next one.
    pub(crate) fn set_metrics_split(&mut self, spell: SpellId, split: usize) {
        let state = &mut self.spells[spell];
        assert!(
            !state.split_metrics.is_empty(),
            "spell has no metric splits"
        );
        state.split_metrics[state.split] = state.metrics;
        state.metrics = state.split_metrics[split];
        state.split = split;
        state.id.tag = split as i32;
        state.action = Some(state.split_actions[split]);
        // Go retags the spell's dot too, so its aura logs line up in the timeline.
        if let Some(dot) = state.dot {
            let aura = self.dots[dot].aura;
            let id = self.spells[spell].id.clone();
            if let Some(aura_id) = self.aura_mut(aura).action_id.as_mut() {
                if aura_id.spell_id == id.spell_id
                    && aura_id.item_id == id.item_id
                    && aura_id.other_id == id.other_id
                {
                    aura_id.tag = split as i32;
                }
            }
        }
    }

    /// Go `makeCastFunc`.
    fn cast_full(&mut self, spell: SpellId, target: Side) -> bool {
        self.spells[spell].cur_cast = self.spells[spell].default_cast;
        // Go `ModifyCast`, before any cast check.
        // Go CastConfig.ModifyCast, which a class may set to its own cast time.
        if let Some(cast_time) = self.class_cast_time(spell) {
            self.spells[spell].cur_cast.cast_time = cast_time;
        }
        if let SpellBehavior::Class(behavior) = self.spells[spell].behavior {
            A::modify_cast(self, spell, behavior);
        }
        if self.spells[spell].flags.swapped {
            return self.cast_failure(spell, |_| "spell attached to an un-equipped item".into());
        }
        if self.wrong_form(spell) {
            return self.cast_failure(spell, |_| "wrong form".into());
        }
        if !self.extra_cast_condition(spell) {
            return self.cast_failure(spell, |_| "extra spell condition".into());
        }
        if self.spells[spell].cost.is_some() && !self.meets_cost(spell) {
            return self.cast_failure(spell, |fight| fight.cost_failure(spell));
        }
        // Go hastes both the GCD and the cast time unless the spell ignores haste. A spell
        // with no GCD and no cast time ignores it implicitly.
        let cur = self.spells[spell].cur_cast;
        let default_cast = self.spells[spell].default_cast;
        let implicit = default_cast.gcd <= 0 && default_cast.cast_time == 0;
        let side = self.caster(spell);
        if !self.spells[spell].ignore_haste && !implicit {
            self.spells[spell].cur_cast.gcd = round(
                self.apply_cast_speed_of(side, cur.gcd).max(0),
                NS_PER_MILLISECOND,
            );
            self.spells[spell].cur_cast.cast_time = round(
                self.apply_cast_speed_for_spell(cur.cast_time, spell),
                NS_PER_MILLISECOND,
            );
        }
        if let Some((timer, _)) = self.spells[spell].cd {
            if self.timers[timer] > self.now {
                return self.cast_failure(spell, |fight| {
                    format!(
                        "still on cooldown for {}, curTime = {}",
                        go_string((fight.timers[timer] - fight.now).max(0)),
                        go_string(fight.now)
                    )
                });
            }
        }
        if let Some((timer, _)) = self.spells[spell].shared_cd {
            if self.timers[timer] > self.now {
                return self.cast_failure(spell, |fight| {
                    format!(
                        "still on shared cooldown for {}, curTime = {}",
                        go_string((fight.timers[timer] - fight.now).max(0)),
                        go_string(fight.now)
                    )
                });
            }
        }
        if self.spells[spell].cur_cast.gcd > 0 && !self.gcd_ready_of(side) {
            return self.cast_failure(spell, |fight| {
                format!(
                    "GCD on cooldown for {}, curTime = {}",
                    go_string((fight.unit(side).gcd - fight.now).max(0)),
                    go_string(fight.now)
                )
            });
        }
        if self.unit(side).hardcast.expires > self.now {
            return self.cast_failure(spell, |fight| {
                let hardcast = fight.unit(side).hardcast;
                let id = hardcast
                    .spell
                    .map(|s| action_string(&fight.spells[s].id))
                    .unwrap_or_else(|| "{}".into());
                format!(
                    "casting/channeling {id} for {}, curTime = {}",
                    go_string(hardcast.expires - fight.now),
                    go_string(fight.now)
                )
            });
        }
        if !self.spells[spell].flags.can_cast_while_moving
            && self.spells[spell].cur_cast.cast_time > 0
            && self.unit(side).moving
        {
            return self.cast_failure(spell, |_| {
                "casting/channeling while moving not allowed!".into()
            });
        }

        let cur = self.spells[spell].cur_cast;
        let channeled = self.spells[spell].flags.channeled;
        let effective = cur.effective_time();
        if effective != 0 {
            if !channeled {
                self.spells[spell].metrics[target.index()].total_cast_time += effective;
            }
            let ready = (self.now + effective).max(self.unit(side).gcd);
            self.set_gcd_timer_of(side, ready);
        }

        if cur.cast_time > 0 {
            if self.log.is_some() && !self.spells[spell].flags.no_logs {
                self.log_casting(spell);
            }
            let expires = self.now + cur.cast_time;
            self.changed(super::reads::CASTS);
            self.unit_mut(side).hardcast = super::Hardcast {
                expires,
                spell: Some(spell),
                target,
                cast_time: cur.cast_time,
                pushback: self.spells[spell].flags.pushback,
            };
            self.new_hardcast_action(side);
            return true;
        }

        if self.log.is_some() && !self.spells[spell].flags.no_logs {
            self.log_casting(spell);
            let line = format!("Completed cast {}", action_string(&self.spells[spell].id));
            self.unit_log(side, &line);
        }
        self.spend_cost(spell);
        self.trigger_cooldowns(spell);
        self.apply_effects(spell, target);
        if !self.spells[spell].flags.no_on_cast_complete {
            self.on_cast_complete(spell);
        }
        true
    }

    fn log_casting(&mut self, spell: SpellId) {
        let cur = self.spells[spell].cur_cast;
        let line = format!(
            "Casting {} (Cost = {:.3}, Cast Time = {}, GCD = {}, Effective Time = {})",
            action_string(&self.spells[spell].id),
            cur.cost.max(0.0),
            go_string(cur.cast_time),
            go_string(cur.gcd_time().max(0)),
            go_string(cur.effective_time())
        );
        self.unit_log(self.caster(spell), &line);
    }

    /// The hardcast `OnComplete` closure from `makeCastFunc`.
    fn complete_hardcast(&mut self, spell: SpellId, target: Side) {
        if self.log.is_some() && !self.spells[spell].flags.no_logs {
            let line = format!("Completed cast {}", action_string(&self.spells[spell].id));
            self.unit_log(self.caster(spell), &line);
        }
        // Go deactivates the casting unit's own hardcast avoidance aura: a pet's cast leaves the
        // tank's alone.
        if self.caster(spell) == Side::Player {
            self.player.reduced_avoidance = false;
        }
        if !self.can_complete_cast(spell, true) {
            return;
        }
        self.spend_cost(spell);
        self.trigger_cooldowns(spell);
        self.apply_effects(spell, target);
        if !self.spells[spell].flags.no_on_cast_complete {
            self.on_cast_complete(spell);
        }
        // A hardcast shorter than the GCD leaves a weaver free to move before the GCD frees.
        let caster = self.caster(spell);
        self.weave_wakeup_after_cast(caster);
    }

    /// Run an acting unit's due hardcast completion, as both its rotation and hardcast
    /// actions do.
    pub(crate) fn complete_due_hardcast_of(&mut self, side: Side) {
        let hardcast = self.unit(side).hardcast;
        if hardcast.expires != STARTING_CD_TIME && hardcast.expires <= self.now {
            self.unit_mut(side).hardcast.expires = STARTING_CD_TIME;
            self.changed(super::reads::CASTS);
            if let Some(spell) = hardcast.spell {
                self.complete_hardcast(spell, hardcast.target);
            }
        }
    }

    /// Go `ResourceCostImpl.SpendCost` for the spell's resource.
    fn spend_cost(&mut self, spell: SpellId) {
        let Some(kind) = self.spells[spell].cost.map(|cost| cost.kind) else {
            return;
        };
        let cost = self.spells[spell].cur_cast.cost;
        if kind == ResourceKind::Rage {
            if cost > 0.0 {
                let metrics = self.spells[spell].mana_metrics.expect("rage metrics");
                self.spend_rage(cost, metrics);
            }
            return;
        }
        if kind == ResourceKind::Focus {
            // Go FocusCost.SpendCost spends even a zero cost.
            let metrics = self.spells[spell].mana_metrics.expect("focus metrics");
            self.spend_focus(self.caster(spell), cost, metrics);
            return;
        }
        if kind == ResourceKind::Energy {
            // Go EnergyCost.SpendCost spends even a zero cost.
            let (metrics, _) = self.spells[spell].energy_metrics.expect("energy metrics");
            self.spend_energy(cost, metrics);
            return;
        }
        if cost > 0.0 {
            let metrics = self.spells[spell].mana_metrics.expect("mana metrics");
            self.spend_mana(cost, metrics);
            let now = self.now;
            let unit = self.unit_mut(self.spells[spell].caster);
            unit.five_second_rule_refresh = (now + 5 * NS_PER_SECOND).max(unit.hardcast.expires);
        }
    }

    /// Go `Spell.IssueRefund`: an energy cost gives back its refund share of the cost paid.
    pub(crate) fn issue_refund(&mut self, spell: SpellId) {
        let Some(cost) = self.spells[spell].cost else {
            return;
        };
        if cost.kind == ResourceKind::Rage {
            self.issue_rage_refund(spell);
            return;
        }
        if cost.kind != ResourceKind::Energy {
            return;
        }
        let paid = self.spells[spell].cur_cast.cost;
        if cost.refund > 0.0 && paid > 0.0 {
            let metrics = self.energy_bar().refund_metrics;
            self.add_energy(cost.refund * paid, metrics);
        }
    }

    /// Go `Spell.applyEffects`.
    pub(crate) fn apply_effects(&mut self, spell: SpellId, target: Side) {
        self.spells[spell].metrics[target.index()].casts += 1;
        self.spells[spell].casts += 1;
        // Go runs OnApplyEffects first, unless the spell skips cast completion callbacks.
        if !self.spells[spell].flags.no_on_cast_complete {
            self.on_apply_effects(spell, target);
        }
        match self.spells[spell].behavior.clone() {
            SpellBehavior::Class(behavior) => A::apply_effects(self, spell, target, behavior),
            SpellBehavior::PotionMana {
                label,
                min,
                spread,
                stone_multiplier,
                ..
            } => {
                // Go min + roll * spread, which the arm64 build fuses.
                let gain = self.random(&label).mul_add(spread, min) * stone_multiplier;
                // Go ExecuteResourceGain: a unit without a mana bar gains nothing.
                if self.has_mana_bar() {
                    let metrics = self.item_metrics(spell);
                    self.execute_mana_gain(gain, metrics);
                }
            }
            SpellBehavior::PotionResource {
                label,
                gains,
                stone_multiplier,
                aura,
                ..
            } => {
                if let Some(index) = aura {
                    self.activate_aura(AuraRef {
                        side: Side::Player,
                        index,
                    });
                }
                for (kind, min, spread) in gains {
                    let gain = self.random(&label).mul_add(spread, min) * stone_multiplier;
                    let metrics = self.potion_metrics(spell, kind);
                    self.execute_resource_gain(kind, gain, metrics);
                }
            }
            SpellBehavior::ConjuredMana {
                label, min, spread, ..
            } => {
                // Go's TernaryFloat64 evaluates both arguments, so the roll always happens.
                let rolled = self.random(&label) * spread;
                let gain = min + if spread > 1.0 { rolled } else { spread };
                let metrics = self.item_metrics(spell);
                self.execute_mana_gain(gain, metrics);
            }
            SpellBehavior::ConjuredEnergy {
                label,
                min,
                spread,
                metrics,
                ..
            } => {
                // Go's TernaryFloat64 evaluates both arguments, so the roll always happens.
                let rolled = self.random(&label) * spread;
                let gain = min + if spread > 1.0 { rolled } else { spread };
                // Go ExecuteResourceGain for energy, which gains nothing on a unit without the
                // bar.
                if self.energy.is_some() {
                    if gain > 0.0 {
                        self.add_energy(gain, metrics);
                    } else if gain < 0.0 {
                        self.spend_energy(-gain, metrics);
                    }
                }
            }
            SpellBehavior::EnergizeOnUse {
                average,
                variance,
                periodic,
                bar,
                ..
            } => {
                if periodic {
                    // Go `spell.SelfHot().Apply`: the ticks gain the resource.
                    let dot = self.spells[spell]
                        .dot
                        .expect("a periodic energize has its self hot");
                    self.apply_dot(dot);
                } else {
                    let gain = self.effect_roll(average, variance);
                    self.energize_bar(spell, bar, gain);
                }
            }
            SpellBehavior::DiamondFlask(_) => {
                // Go `spell.SelfHot().Apply`.
                let dot = self.spells[spell]
                    .dot
                    .expect("the Diamond Flask has its self hot");
                self.apply_dot(dot);
            }
            SpellBehavior::TouchOfTheGraveDrain {
                health_fraction,
                metrics,
            } => self.touch_of_the_grave_drain(spell, target, health_fraction, metrics),
            SpellBehavior::Eureka => {
                let aura = self.eureka.as_ref().expect("Eureka! is bound").aura;
                self.activate_aura(aura);
            }
            SpellBehavior::ActivateAura(index) => self.activate_aura(AuraRef {
                side: Side::Player,
                index,
            }),
            SpellBehavior::MeleeAuto(hand) => self.apply_melee_auto(spell, target, hand),
            SpellBehavior::GoblinSapper => self.apply_goblin_sapper(spell),
            SpellBehavior::Move => self.apply_movement(self.spells[spell].caster),
            SpellBehavior::BasicExplosive {
                min,
                max,
                aoe_cap_multiplier,
            } => self.apply_basic_explosive(spell, min, max, aoe_cap_multiplier),
            SpellBehavior::AreaFixedDamage(damage) => {
                // Go `CalcAndDealAoeDamage`: each target's hit is calculated and dealt in turn.
                for position in 0..self.targets.len() {
                    let result = self.calc_damage(spell, Side::target(position), damage);
                    self.deal_damage(spell, result, false);
                }
            }
            SpellBehavior::ThunderfuryStrike(damage) => {
                self.thunderfury_strike(spell, target, damage)
            }
            SpellBehavior::ThunderfuryBounce => self.thunderfury_bounce(spell, target),
            SpellBehavior::SulfurasFireball { min, max } => {
                self.sulfuras_fireball(spell, target, min, max)
            }
            SpellBehavior::FixedTableDamage { damage, outcome } => {
                // Go `CalcAndDealDamage(damage, outcomeApplier)`: no roll.
                let result = self.calc_physical_damage(spell, target, damage, outcome);
                self.deal_damage(spell, result, false);
            }
            SpellBehavior::FixedHit(base) => {
                let result = self.calc_damage_with(spell, target, base, super::Outcome::AlwaysHit);
                self.deal_damage(spell, result, false);
            }
            SpellBehavior::SelfHeal(heal) => self.apply_self_heal(spell, heal),
            SpellBehavior::AbsorbOnUse(index) => self.apply_item_absorb(index),
            SpellBehavior::PeriodicMana {
                amount,
                ticks,
                period,
                metrics,
                ..
            } => {
                let periodic = super::Periodic {
                    tag: 0,
                    period,
                    num_ticks: ticks,
                    done: 0,
                    priority: super::PRIORITY_AUTO,
                };
                self.schedule(
                    self.now + period,
                    super::PRIORITY_AUTO,
                    super::Action::ItemManaTick {
                        amount,
                        metrics,
                        periodic,
                    },
                );
            }
            // Go consumes.go: the potion casts its spell at the current target, which a cast on
            // the player is too.
            SpellBehavior::CastsSpell(inner) => {
                let target = if target == Side::Player {
                    self.current_target_side()
                } else {
                    target
                };
                self.cast(inner, target);
            }
            SpellBehavior::DragonfirePotion { direct, .. } => {
                let dot = self.spells[spell]
                    .dot
                    .expect("Dragonfire Potion has its dot");
                for position in 0..self.targets.len() {
                    let side = Side::target(position);
                    let result = self.calc_damage(spell, side, direct);
                    if result.landed() {
                        let copy = self.dot_on(dot, side);
                        self.apply_dot(copy);
                    }
                    self.deal_damage(spell, result, false);
                }
            }
            SpellBehavior::CausticSmogPotion { .. } => {
                let dot = self.spells[spell]
                    .dot
                    .expect("Caustic Smog Potion has its dot");
                for position in 0..self.targets.len() {
                    let side = Side::target(position);
                    let result = self.calc_outcome(spell, side, super::Outcome::MagicHit);
                    if result.landed() {
                        let copy = self.dot_on(dot, side);
                        self.apply_dot(copy);
                    }
                    self.deal_damage(spell, result, false);
                }
            }
            SpellBehavior::AreaRollDamage { min, max } => {
                for position in 0..self.targets.len() {
                    let base = self.go_roll(min, max);
                    let result = self.calc_damage(spell, Side::target(position), base);
                    self.deal_damage(spell, result, false);
                }
            }
            SpellBehavior::RollDamage { min, max, can_crit } => {
                let base = self.go_roll(min, max);
                // Go `CalcDamage` adds a physical spell's bonus damage; The Lobotomizer's Brain
                // Damage is one that rolls the magic table.
                let result = if self.spells[spell].school & 1 != 0 {
                    let outcome = if can_crit {
                        super::Outcome::MagicHitAndCrit
                    } else {
                        super::Outcome::MagicHit
                    };
                    self.calc_damage_with(spell, target, base, outcome)
                } else if can_crit {
                    self.calc_damage(spell, target, base)
                } else {
                    self.calc_damage_hit_only(spell, target, base)
                };
                self.deal_damage(spell, result, false);
            }
            SpellBehavior::RollTableDamage { min, max, outcome } => {
                // Go `CalcAndDealDamage(sim.Roll(min, max), outcomeApplier)`: the roll comes
                // before the table's.
                let base = self.go_roll(min, max);
                let result = self.calc_physical_damage(spell, target, base, outcome);
                self.deal_damage(spell, result, false);
            }
            SpellBehavior::EffectRoll {
                average,
                variance,
                can_crit,
            } => {
                let base = self.effect_roll(average, variance);
                let result = if can_crit {
                    self.calc_damage(spell, target, base)
                } else {
                    self.calc_damage_hit_only(spell, target, base)
                };
                self.deal_damage(spell, result, false);
            }
            SpellBehavior::ProcDamage(params) => self.apply_proc_damage(spell, target, params),
            // Go `AutoAttacks.ExtraMHAttack` / `ExtraMHAttacks`.
            SpellBehavior::ExtraAttack(attacks) => self.extra_mh_attacks(attacks),
            SpellBehavior::OnUseDamage(params) => self.apply_on_use_damage(spell, target, params),
            SpellBehavior::ExternalCooldown => self.external_cooldown_cast(spell),
            SpellBehavior::None => panic!("spell {} has no behavior", self.spells[spell].id),
        }
        A::after_apply_effects(self, spell);
    }

    /// Go `ExecuteResourceGain`: a resource the player has no bar for is not gained, and rage
    /// amounts are in tenths.
    fn execute_resource_gain(&mut self, kind: ResourceKind, amount: f64, metrics: usize) {
        match kind {
            ResourceKind::Rage if self.rage.is_some() && amount > 0.0 => {
                self.add_rage(amount / 10.0, metrics)
            }
            ResourceKind::Rage if self.rage.is_some() && amount < 0.0 => {
                self.spend_rage(-amount / 10.0, metrics)
            }
            ResourceKind::Mana if self.has_mana_bar() => self.execute_mana_gain(amount, metrics),
            _ => {}
        }
    }

    /// The resource metrics of a potion, one per resource it restores.
    fn potion_metrics(&mut self, spell: SpellId, kind: ResourceKind) -> usize {
        if let SpellBehavior::PotionResource { metrics, .. } = &self.spells[spell].behavior {
            if let Some(&(_, index)) = metrics.iter().find(|(k, _)| *k == kind) {
                return index;
            }
        }
        let index = self.new_resource_metrics(self.spells[spell].id.clone(), kind);
        if let SpellBehavior::PotionResource { metrics, .. } = &mut self.spells[spell].behavior {
            metrics.push((kind, index));
        }
        index
    }

    /// Go `ExecuteResourceGain` for mana: only a positive amount is gained.
    fn execute_mana_gain(&mut self, amount: f64, metrics: usize) {
        if amount > 0.0 {
            self.add_mana(amount, metrics);
        }
    }

    /// The mana metrics registered for an item or class spell action.
    /// Go spell_data_energize.go `energizedBar.add`: the gain on the bar the item fills.
    pub(crate) fn energize_bar(&mut self, spell: SpellId, bar: EnergizedBar, gain: f64) {
        match bar {
            EnergizedBar::Mana => {
                let metrics = self.item_metrics(spell);
                self.add_mana(gain, metrics);
            }
            EnergizedBar::Rage(metrics) => self.add_rage(gain, metrics),
            EnergizedBar::Energy(metrics) => self.add_energy(gain, metrics),
        }
    }

    pub(crate) fn item_metrics(&mut self, spell: SpellId) -> usize {
        if let Some(metrics) = self.spells[spell].mana_metrics {
            return metrics;
        }
        let index = self.new_mana_metrics_of(self.caster(spell), self.spells[spell].id.clone());
        self.spells[spell].mana_metrics = Some(index);
        index
    }

    /// Go `Unit.NewManaMetrics`: every call registers a new metric.
    pub(crate) fn new_mana_metrics(
        &mut self,
        id: crate::contracts::prepared_v2::ActionId,
    ) -> usize {
        self.new_mana_metrics_of(Side::Player, id)
    }

    /// Go `Unit.NewManaMetrics` of an acting unit.
    pub(crate) fn new_mana_metrics_of(
        &mut self,
        unit: Side,
        id: crate::contracts::prepared_v2::ActionId,
    ) -> usize {
        self.new_resource_metrics_of(unit, id, super::ResourceKind::Mana)
    }

    /// Go `UnitMetrics.NewResourceMetrics`: every call registers a new metric.
    pub(crate) fn new_resource_metrics(
        &mut self,
        id: crate::contracts::prepared_v2::ActionId,
        kind: super::ResourceKind,
    ) -> usize {
        self.new_resource_metrics_of(Side::Player, id, kind)
    }

    /// Go `UnitMetrics.NewResourceMetrics` of an acting unit.
    pub(crate) fn new_resource_metrics_of(
        &mut self,
        unit: Side,
        id: crate::contracts::prepared_v2::ActionId,
        kind: super::ResourceKind,
    ) -> usize {
        self.resources.push(super::ResourceMetrics {
            id,
            unit,
            kind,
            events: 0,
            gain: 0.0,
            actual_gain: 0.0,
            previous_events: 0,
            previous_actual_gain: 0.0,
            is_mana_regen: false,
            no_threat: false,
        });
        self.resources.len() - 1
    }

    /// Go `UnitMetrics.NewResourceMetrics` for a metric Go registers just before the spell's
    /// own cost metric: the two swap places, so they keep Go's order.
    pub(crate) fn new_resource_metrics_before_cost(
        &mut self,
        spell: SpellId,
        id: crate::contracts::prepared_v2::ActionId,
        kind: super::ResourceKind,
    ) -> usize {
        let index = self.new_resource_metrics(id, kind);
        let Some(cost) = self.spells[spell].mana_metrics else {
            return index;
        };
        self.resources.swap(cost, index);
        self.spells[spell].mana_metrics = Some(index);
        cost
    }

    /// Go `Unit.NewHealthMetrics`: every call registers a new metric.
    pub(crate) fn new_health_metrics(
        &mut self,
        id: crate::contracts::prepared_v2::ActionId,
    ) -> usize {
        self.new_resource_metrics(id, super::ResourceKind::Health)
    }

    /// Go `Unit.SetGCDTimer`.
    pub(crate) fn set_gcd_timer(&mut self, ready: i64) {
        self.set_gcd_timer_of(Side::Player, ready);
    }

    /// Go `Unit.SetGCDTimer` of an acting unit.
    pub(crate) fn set_gcd_timer_of(&mut self, side: Side, ready: i64) {
        self.unit_mut(side).gcd = ready;
        self.changed(super::reads::TIMERS);
        self.set_rotation_timer_of(side, ready);
    }

    /// Go `Unit.SetRotationTimer`.
    pub(crate) fn set_rotation_timer(&mut self, ready: i64) {
        self.set_rotation_timer_of(Side::Player, ready);
    }

    /// Go `Unit.SetRotationTimer` of an acting unit.
    pub(crate) fn set_rotation_timer_of(&mut self, side: Side, ready: i64) {
        self.unit_mut(side).rotation_timer = ready;
        if let Some(action) = self.unit_mut(side).rotation_action.take() {
            self.queue.cancel(action);
        }
        let action = self.schedule(ready, PRIORITY_GCD, Action::Rotation(side));
        self.unit_mut(side).rotation_action = Some(action);
    }

    /// Go `Unit.WaitUntil`.
    pub(crate) fn wait_until(&mut self, ready: i64) {
        self.wait_until_of(Side::Player, ready);
    }

    /// Go `Unit.WaitUntil` of an acting unit.
    pub(crate) fn wait_until_of(&mut self, side: Side, ready: i64) {
        assert!(ready >= self.now, "cannot wait negative time");
        self.set_rotation_timer_of(side, ready);
        if self.log.is_some() && ready > self.now {
            let line = format!(
                "Pausing rotation for {} due to resources / CDs.",
                go_string(ready - self.now)
            );
            self.unit_log(side, &line);
        }
    }

    /// The "Pushback trigger" handler, which Go runs a spell batch window after the hit that
    /// passed its conditions. Since the fork's patch 89 it leaves a hardcast that finished in
    /// between alone, before the pushback roll. The gate admits only casts that are not
    /// channeled.
    pub(crate) fn pushback_handler(&mut self, chance: f64) {
        let hardcast = self.player.hardcast;
        let Some(spell) = hardcast.spell else {
            return;
        };
        if hardcast.expires <= self.now {
            return;
        }
        let resist = self.spells[spell].pushback_resist;
        if !self.proc(chance - resist, "Pushback") {
            return;
        }
        // Go `Hardcast.pushBack`.
        let pushback =
            SPELL_PUSHBACK_DURATION.min(self.now + hardcast.cast_time - hardcast.expires);
        if pushback <= 0 {
            return;
        }
        self.player.hardcast.expires += pushback;
        self.changed(super::reads::CASTS);
        if self.log.is_some() {
            let line = format!(
                "{} pushed back {} while casting",
                action_string(&self.spells[spell].id),
                go_string(pushback)
            );
            self.unit_log(Side::Player, &line);
        }
        // Re-schedule the cast at the new expiry.
        self.new_hardcast_action(Side::Player);
    }

    /// Go `Unit.newHardcastAction`.
    fn new_hardcast_action(&mut self, side: Side) {
        // Go: while casting, a tank's dodge, parry and block fall to zero.
        if side == Side::Player && self.tanked() {
            self.player.reduced_avoidance = true;
        }
        if let Some(action) = self.unit_mut(side).hardcast_action.take() {
            self.queue.cancel(action);
        }
        let expires = self.unit(side).hardcast.expires;
        let action = self.schedule(expires, PRIORITY_GCD, Action::Hardcast(side));
        self.unit_mut(side).hardcast_action = Some(action);
    }

    /// Go `Unit.AddMana` on the unit whose metrics these are.
    pub(crate) fn add_mana(&mut self, amount: f64, metrics: usize) {
        assert!(amount >= 0.0, "negative mana gain");
        let side = self.resources[metrics].unit;
        let max = self.unit(side).powers.max_mana;
        let old = self.unit(side).mana;
        let new = (old + amount).min(max);
        let resource = &mut self.resources[metrics];
        resource.events += 1;
        resource.gain += amount;
        resource.actual_gain += new - old;
        if self.log.is_some() {
            let line = format!(
                "Gained {amount:.3} mana from {} ({old:.3} --> {new:.3}) of {max:.0} total.",
                action_string(&self.resources[metrics].id),
            );
            self.unit_log(side, &line);
        }
        self.changed(super::reads::RESOURCES);
        let unit = self.unit_mut(side);
        unit.mana = new;
        unit.mana_gained += new - old;
    }

    /// Go `Unit.SpendMana` on the unit whose metrics these are.
    pub(crate) fn spend_mana(&mut self, amount: f64, metrics: usize) {
        let side = self.resources[metrics].unit;
        let max = self.unit(side).powers.max_mana;
        let old = self.unit(side).mana;
        let new = old - amount;
        let resource = &mut self.resources[metrics];
        resource.events += 1;
        resource.gain -= amount;
        resource.actual_gain -= amount;
        if self.log.is_some() {
            let line = format!(
                "Spent {amount:.3} mana from {} ({old:.3} --> {new:.3}) of {max:.0} total.",
                action_string(&self.resources[metrics].id),
            );
            self.unit_log(side, &line);
        }
        self.changed(super::reads::RESOURCES);
        let unit = self.unit_mut(side);
        unit.mana = new;
        unit.mana_spent += amount;
    }

    /// Go `Unit.ManaTick` for a pet, whose regeneration has no attribution in scope.
    pub(crate) fn pet_mana_tick(&mut self, side: Side) {
        let pet = self.active_pet(side);
        let casting = self.now < pet.state.five_second_rule_refresh;
        let (regen, metrics) = if casting {
            (pet.state.mana_tick_casting, pet.mana_regen_casting)
        } else {
            (pet.state.mana_tick_not_casting, pet.mana_regen_not_casting)
        };
        self.add_mana(regen.max(0.0), metrics);
    }

    /// Go `Unit.ManaTick`, with its spirit regeneration attribution.
    pub(crate) fn mana_tick(&mut self) {
        let casting = self.now < self.player.five_second_rule_refresh;
        let (regen, metrics) = if casting {
            (self.player.mana_tick_casting, self.mana_regen_casting)
        } else {
            (
                self.player.mana_tick_not_casting,
                self.mana_regen_not_casting,
            )
        };
        let regen = regen.max(0.0);
        let before = self.player.mana;
        self.add_mana(regen, metrics);
        // What the tick's metrics still hold of it once each source below takes its share.
        let mut left = (regen, self.player.mana - before);
        if let Some(source) = self.player.spirit_attribution {
            let mut spirit = self.player.powers.spirit_regen_per_second * source.multiplier;
            if casting && !source.force_full {
                spirit *= self.player.spirit_regen_rate_casting;
            }
            let baseline =
                ((self.player.powers.mp5 / 5.0 + spirit) * self.player.mana_regen_multiplier * 2.0)
                    .max(0.0);
            let bonus = (regen - baseline).max(0.0);
            if bonus > 0.0 {
                // Go credits ordinary regeneration first; only the remaining room in the mana
                // bar is an actual gain caused by the bonus.
                let actual = self.player.mana - before;
                let bonus_actual = bonus.min((actual - regen.min(baseline)).max(0.0));
                let resource = &mut self.resources[metrics];
                resource.gain -= bonus;
                resource.actual_gain -= bonus_actual;
                let source_metrics = &mut self.resources[source.metrics];
                source_metrics.events += 1;
                source_metrics.gain += bonus;
                source_metrics.actual_gain += bonus_actual;
                left = (left.0 - bonus, left.1 - bonus_actual);
            }
        }
        if casting {
            let actual = self.player.mana - before;
            let moved = self.credit_casting_regen(regen, actual, metrics);
            left = (left.0 - moved.0, left.1 - moved.1);
        }
        self.credit_mp5_sources(left.0, left.1, metrics);
    }

    /// Go `StartSpiritRegenAttribution`, before the source applies its own effect.
    pub(crate) fn start_spirit_attribution(&mut self, metrics: usize) {
        assert!(
            self.player.spirit_attribution.is_none(),
            "overlapping spirit regeneration attribution"
        );
        self.resources[metrics].is_mana_regen = true;
        self.player.spirit_attribution = Some(super::SpiritAttribution {
            metrics,
            multiplier: self.player.spirit_regen_multiplier,
            force_full: self.player.force_full_spirit_regen,
        });
    }

    /// Go `StopSpiritRegenAttribution`.
    pub(crate) fn stop_spirit_attribution(&mut self) {
        self.player.spirit_attribution = None;
    }

    /// Go `MultiplySpiritRegenMultiplier`: another effect moves the attribution baseline too.
    pub(crate) fn multiply_spirit_regen_multiplier(&mut self, multiplier: f64) {
        self.player.spirit_regen_multiplier *= multiplier;
        if let Some(source) = self.player.spirit_attribution.as_mut() {
            source.multiplier *= multiplier;
        }
    }

    /// Go `DivideSpiritRegenMultiplier`.
    pub(crate) fn divide_spirit_regen_multiplier(&mut self, divisor: f64) {
        self.player.spirit_regen_multiplier /= divisor;
        if let Some(source) = self.player.spirit_attribution.as_mut() {
            source.multiplier /= divisor;
        }
    }

    /// Go `AddSpiritRegenMultiplier`.
    pub(crate) fn add_spirit_regen_multiplier(&mut self, amount: f64) {
        self.player.spirit_regen_multiplier += amount;
        if let Some(source) = self.player.spirit_attribution.as_mut() {
            source.multiplier += amount;
        }
    }

    /// Go `SetForceFullSpiritRegen`.
    pub(crate) fn set_force_full_spirit_regen(&mut self, full: bool) {
        self.player.force_full_spirit_regen = full;
        if let Some(source) = self.player.spirit_attribution.as_mut() {
            source.force_full = full;
        }
    }

    /// Go `majorCooldownManager.reset`: copies in initial order, then a stable sort.
    pub(crate) fn reset_cooldown_manager(&mut self) {
        for cooldown in &mut self.major_cooldowns {
            cooldown.uses = 0;
        }
        self.cooldown_order = (0..self.major_cooldowns.len()).collect();
        self.update_major_cooldowns();
    }

    /// Go `UpdateMajorCooldowns`: sort by ready time, then higher priority first.
    pub(crate) fn update_major_cooldowns(&mut self) {
        if self.cooldown_order.is_empty() {
            self.cooldown_min_ready = crate::core::time::NEVER_EXPIRES;
            return;
        }
        let mut order = std::mem::take(&mut self.cooldown_order);
        order.sort_by(|&a, &b| {
            let (ra, rb) = (
                self.spell_ready_at(self.major_cooldowns[a].spell),
                self.spell_ready_at(self.major_cooldowns[b].spell),
            );
            ra.cmp(&rb).then(
                self.major_cooldowns[b]
                    .priority
                    .cmp(&self.major_cooldowns[a].priority),
            )
        });
        self.cooldown_min_ready = self.spell_ready_at(self.major_cooldowns[order[0]].spell);
        self.cooldown_order = order;
    }

    /// Go `MajorCooldown.ShouldActivate` for the non-class cooldowns.
    fn cooldown_should_activate(&self, spell: SpellId) -> bool {
        if !A::cooldown_activation_condition(self, spell) {
            return false;
        }
        let max = self.player.powers.max_mana;
        let mana = self.player.mana;
        // Read only by the mana gains, so the others skip the regeneration arithmetic.
        let casting_regen =
            || crate::mechanics::mana::regen_per_second_casting(self.regen_inputs());
        match &self.spells[spell].behavior {
            SpellBehavior::Class(behavior) => A::should_activate(self, spell, *behavior),
            SpellBehavior::PotionMana {
                min,
                spread,
                stone_multiplier,
                regen_window,
                ..
            } => {
                // The arm64 build fuses the regeneration window into the current mana.
                let gain = (min + spread) * stone_multiplier;
                max - casting_regen().mul_add(*regen_window, mana) >= gain
            }
            SpellBehavior::ConjuredMana {
                min,
                spread,
                selected,
                regen_window,
                ..
            } => max - casting_regen().mul_add(*regen_window, mana) >= min + spread && *selected,
            SpellBehavior::ConjuredEnergy {
                min,
                spread,
                selected,
                spill,
                ..
            } => {
                // Go reads an empty energy bar on a class without one, so it never fires.
                let (max, current) = self
                    .energy
                    .as_ref()
                    .map_or((0.0, 0.0), |bar| (bar.max, bar.current));
                max - current >= (min + spread) - spill && *selected
            }
            SpellBehavior::EnergizeOnUse { whole, bar, .. } => {
                let room = match bar {
                    EnergizedBar::Mana => max - mana,
                    EnergizedBar::Rage(_) => self
                        .rage
                        .as_ref()
                        .map_or(0.0, |rage| rage.max - rage.current),
                    EnergizedBar::Energy(_) => self
                        .energy
                        .as_ref()
                        .map_or(0.0, |energy| energy.max - energy.current),
                };
                room >= *whole
            }
            SpellBehavior::PeriodicMana { min_deficit, .. } => max - mana >= *min_deficit,
            // Go: only a mana gain asks whether the mana fits.
            SpellBehavior::PotionResource {
                gains,
                stone_multiplier,
                ..
            } => gains.iter().all(|(kind, min, spread)| {
                *kind != ResourceKind::Mana
                    || max - casting_regen().mul_add(5.0, mana) >= (min + spread) * stone_multiplier
            }),
            SpellBehavior::ExternalCooldown => self.external_cooldown_should_activate(spell),
            // Go's default ShouldActivate.
            SpellBehavior::Eureka
            | SpellBehavior::CastsSpell(_)
            | SpellBehavior::DragonfirePotion { .. }
            | SpellBehavior::CausticSmogPotion { .. }
            | SpellBehavior::ActivateAura(_)
            | SpellBehavior::GoblinSapper
            | SpellBehavior::BasicExplosive { .. }
            | SpellBehavior::OnUseDamage(_)
            | SpellBehavior::SelfHeal(_)
            | SpellBehavior::AbsorbOnUse(_) => true,
            // The flask's major cooldown never activates.
            SpellBehavior::DiamondFlask(_)
            | SpellBehavior::TouchOfTheGraveDrain { .. }
            | SpellBehavior::MeleeAuto(_)
            | SpellBehavior::Move
            | SpellBehavior::RollDamage { .. }
            | SpellBehavior::RollTableDamage { .. }
            | SpellBehavior::ProcDamage(_)
            | SpellBehavior::ExtraAttack(_)
            | SpellBehavior::AreaRollDamage { .. }
            | SpellBehavior::SulfurasFireball { .. }
            | SpellBehavior::AreaFixedDamage(_)
            | SpellBehavior::ThunderfuryStrike(_)
            | SpellBehavior::ThunderfuryBounce
            | SpellBehavior::FixedHit(_)
            | SpellBehavior::FixedTableDamage { .. }
            | SpellBehavior::EffectRoll { .. }
            | SpellBehavior::None => false,
        }
    }

    /// Go `shouldActivateHelper`.
    fn should_activate_helper(&mut self, cooldown: usize) -> bool {
        let spell = self.major_cooldowns[cooldown].spell;
        if !self.can_cast(spell) {
            return false;
        }
        let uses = self.major_cooldowns[cooldown].uses;
        if let Some(&timing) = self.major_cooldowns[cooldown].timings.get(uses) {
            return self.now >= timing;
        }
        // Survival cooldowns wait for health to fall to the defensive threshold; at zero Go
        // never fires them on its own.
        let threshold = self.hp_percent_for_defensives;
        if self.major_cooldowns[cooldown].survival
            && (threshold == 0.0 || self.player.health / self.player_max_health() > threshold)
        {
            return false;
        }
        self.cooldown_should_activate(spell)
    }

    /// Go `getFirstReadyMCD`. Under Forever's rule (`autocast_skips_unready_cooldowns`) it
    /// passes over a cooldown that may not fire at this point of the GCD, as wowsims/forever
    /// does since 21 and 22 October 2026 (10b0e65f2, fbf5d98e8).
    pub(crate) fn first_ready_cooldown(&mut self) -> Option<usize> {
        if self.now < self.cooldown_min_ready {
            return None;
        }
        for position in 0..self.cooldown_order.len() {
            let cooldown = self.cooldown_order[position];
            if !self.spell_ready(self.major_cooldowns[cooldown].spell) {
                continue;
            }
            if self.autocast_skips_unready_cooldowns && !self.gcd_allows_cooldown(cooldown) {
                continue;
            }
            if self.should_activate_helper(cooldown) {
                return Some(cooldown);
            }
        }
        None
    }

    /// Whether a major cooldown may fire at this point of the GCD. Cooldowns go just before
    /// the next spell, so once the GCD is ready; explosives, off the GCD with a short cast,
    /// right after one starts, and one whose cast fits in the shortest GCD only while what is
    /// left of the GCD still covers it, so it never delays the next spell. A reactive one goes
    /// either way.
    fn gcd_allows_cooldown(&self, cooldown: usize) -> bool {
        let spell = self.major_cooldowns[cooldown].spell;
        if self.spells[spell].flags.reactive {
            return true;
        }
        let left = self.gcd_time_to_ready();
        if !self.major_cooldowns[cooldown].explosive {
            return left == 0;
        }
        let cast = self.spell_cast_time(spell);
        left > 0 && !(left < cast && cast <= crate::prepare::spell::GCD_MIN)
    }

    /// Go `Spell.CastTime`: the default cast time with cast speed, unless the spell ignores
    /// haste.
    fn spell_cast_time(&self, spell: SpellId) -> i64 {
        let state = &self.spells[spell];
        if state.ignore_haste {
            state.default_cast.cast_time
        } else {
            self.apply_cast_speed_for_spell(state.default_cast.cast_time, spell)
        }
    }

    /// Go `APLActionAutocastOtherCooldowns.IsReady`.
    pub(crate) fn autocast_ready(&mut self) -> Option<usize> {
        let cooldown = self.first_ready_cooldown()?;
        if self.autocast_skips_unready_cooldowns {
            return Some(cooldown);
        }
        let spell = self.major_cooldowns[cooldown].spell;
        let explosive = self.major_cooldowns[cooldown].explosive;
        (self.gcd_ready() != explosive || self.spells[spell].flags.reactive).then_some(cooldown)
    }

    /// For autocasting that is not ready: until when it stays so while nothing a cached look
    /// reads changes, where every part of that is known and asking again leaves no trace. A
    /// cooling down cooldown waits for its timer, an accepted one only for the GCD, and a
    /// refused one for what [`Fight::refusal_until`] says. With it, the energy below which that
    /// holds as energy rises.
    pub(crate) fn autocast_until(&mut self) -> Option<(i64, f64)> {
        if self.now < self.cooldown_min_ready {
            return Some((self.cooldown_min_ready, f64::INFINITY));
        }
        let mut until = crate::core::time::NEVER_EXPIRES;
        let mut energy = f64::INFINITY;
        for position in 0..self.cooldown_order.len() {
            let cooldown = self.cooldown_order[position];
            let spell = self.major_cooldowns[cooldown].spell;
            if !self.spell_ready(spell) {
                until = until.min(self.spell_ready_at(spell));
                continue;
            }
            if self.autocast_skips_unready_cooldowns && !self.gcd_allows_cooldown(cooldown) {
                // Passed over until the GCD lets it: a plain one until the GCD runs out, an
                // explosive one until a cast starts the GCD.
                if !self.major_cooldowns[cooldown].explosive {
                    until = until.min(self.player.gcd);
                }
                continue;
            }
            if !self.asking_is_quiet(spell) {
                return None;
            }
            if self.should_activate_helper(cooldown) {
                if self.autocast_skips_unready_cooldowns {
                    // Accepted and allowed: autocasting is ready, nothing to cache.
                    return None;
                }
                // Accepted, so only the GCD holds it back: a plain one until the GCD runs
                // out, an explosive one until a cast starts the GCD.
                if !self.major_cooldowns[cooldown].explosive {
                    until = until.min(self.player.gcd);
                }
                return Some((until, energy.min(self.accepted_energy_limit(spell))));
            }
            let (refused_until, refused_energy) = self.refusal_until(cooldown)?;
            until = until.min(refused_until);
            energy = energy.min(refused_energy);
        }
        Some((until, energy))
    }

    /// The energy at which rising energy makes [`Fight::should_activate_helper`] refuse a
    /// cooldown it accepts now: a conjured energy item's deficit closes, and a class's own rule
    /// is not followed. Rising energy keeps the cost met, and nothing else it asks reads energy.
    fn accepted_energy_limit(&self, spell: SpellId) -> f64 {
        if !A::CACHE_OVER_ENERGY {
            return f64::INFINITY;
        }
        match self.spells[spell].behavior {
            SpellBehavior::ConjuredEnergy {
                min, spread, spill, ..
            } => {
                let (max, current) = self
                    .energy
                    .as_ref()
                    .map_or((0.0, 0.0), |bar| (bar.max, bar.current));
                let need = (min + spread) - spill;
                // The least energy whose deficit falls short, found from the rounded estimate.
                let mut at = max - need;
                while max - at >= need {
                    at = at.next_up();
                }
                while at.next_down() > current && max - at.next_down() < need {
                    at = at.next_down();
                }
                at
            }
            SpellBehavior::Class(_) => self.energy_now(),
            _ => f64::INFINITY,
        }
    }

    /// Until when a ready major cooldown's rules keep refusing it while nothing a cached look
    /// reads changes, following [`Fight::should_activate_helper`]. Health, mana and energy
    /// change only with the resources and auras a look reads; a class's own rule is not
    /// known this way. Rising energy only makes the energy rules refuse more.
    fn refusal_until(&mut self, cooldown: usize) -> Option<(i64, f64)> {
        let spell = self.major_cooldowns[cooldown].spell;
        if !self.can_cast(spell) {
            return self.cast_refusal_until(spell);
        }
        let uses = self.major_cooldowns[cooldown].uses;
        if let Some(&timing) = self.major_cooldowns[cooldown].timings.get(uses) {
            return Some((timing, f64::INFINITY));
        }
        let threshold = self.hp_percent_for_defensives;
        if self.major_cooldowns[cooldown].survival
            && (threshold == 0.0 || self.player.health / self.player_max_health() > threshold)
        {
            return Some((crate::core::time::NEVER_EXPIRES, f64::INFINITY));
        }
        if !A::cooldown_activation_condition(self, spell) {
            return None;
        }
        match self.spells[spell].behavior {
            SpellBehavior::PotionMana { .. }
            | SpellBehavior::ConjuredMana { .. }
            | SpellBehavior::ConjuredEnergy { .. }
            | SpellBehavior::EnergizeOnUse { .. }
            | SpellBehavior::PeriodicMana { .. }
            | SpellBehavior::PotionResource { .. } => {
                Some((crate::core::time::NEVER_EXPIRES, f64::INFINITY))
            }
            SpellBehavior::Class(behavior) => {
                A::should_activate_until(self, spell, behavior).map(|until| (until, f64::INFINITY))
            }
            _ => None,
        }
    }

    /// Until when [`Fight::can_cast`] keeps refusing a spell whose cooldowns are ready while
    /// nothing a cached look reads changes: the form, cost, swap and movement change only with
    /// what a look reads, the hardcast and the GCD run out with the clock, and a failed extra
    /// condition needs its own proof. The cost stops refusing once energy reaches it.
    fn cast_refusal_until(&mut self, spell: SpellId) -> Option<(i64, f64)> {
        if self.wrong_form(spell) {
            return Some((crate::core::time::NEVER_EXPIRES, f64::INFINITY));
        }
        if !self.extra_cast_condition(spell) {
            return self
                .extra_condition_proof(spell)
                .map(|(until, _)| (until, f64::INFINITY));
        }
        let state = &self.spells[spell];
        let side = state.caster;
        let needs_gcd = state.default_cast.gcd > 0
            || (state.flags.mcd && side == Side::Player && self.in_sequence());
        let unit = self.unit(side);
        let mut until = crate::core::time::NEVER_EXPIRES;
        if unit.hardcast.expires > self.now {
            until = until.min(unit.hardcast.expires);
        }
        if needs_gcd && unit.gcd > self.now {
            until = until.min(unit.gcd);
        }
        Some((until, self.energy_cost_limit(spell)))
    }

    /// Go `tryActivateHelper` followed by `UpdateMajorCooldowns`.
    pub(crate) fn autocast(&mut self, cooldown: usize) {
        if self.should_activate_helper(cooldown) {
            let spell = self.major_cooldowns[cooldown].spell;
            let target = if self.spells[spell].flags.helpful {
                Side::Player
            } else {
                self.current_target_side()
            };
            self.cast(spell, target);
            self.major_cooldowns[cooldown].uses += 1;
            if self.log.is_some() {
                let line = format!(
                    "Major cooldown used: {}",
                    action_string(&self.spells[spell].id)
                );
                self.player_log(&line);
            }
        }
        self.update_major_cooldowns();
    }
}
