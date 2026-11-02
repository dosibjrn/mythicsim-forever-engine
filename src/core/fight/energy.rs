//! Go energy.go: the energy bar, its regeneration ticks and combo points.
//!
//! Go runs energy ticks as a simulation task, outside the pending-action queue. The step loop
//! checks tasks after weapon swings and before the next pending action, as Go `Step` does,
//! through [`Fight::due_task`] and [`Fight::advance_tasks`].

use crate::{
    contracts::prepared_v2::{ActionId, Energy},
    core::time::NEVER_EXPIRES,
};

use super::{log::action_string, Agent, Fight, ResourceKind, Side};

/// Go `energyBar`.
#[derive(Clone, Debug)]
pub(crate) struct EnergyBar {
    pub(crate) max: f64,
    pub(crate) current: f64,
    pub(crate) max_combo_points: i32,
    pub(crate) combo_points: i32,
    pub(crate) next_tick: i64,
    pub(crate) tick_duration: i64,
    pub(crate) per_tick: f64,
    pub(crate) regen_multiplier: f64,
    pub(crate) regen_metrics: usize,
    pub(crate) refund_metrics: usize,
    /// Go `EncounterStartMetrics`, which only ResetComboPoints credits; nothing in scope does.
    #[allow(dead_code)]
    pub(crate) encounter_start_metrics: usize,
}

/// A listener on the player's casts that restores mana a spell batch window later, in
/// MythicSim's healer sims under Forever's rules: Sudden Insight, Idol of Longevity, The
/// Earthfury's Mana Surge.
#[derive(Clone, Debug)]
pub(crate) struct ManaOnCast {
    pub(crate) label: String,
    pub(crate) chance: f64,
    /// The mana each spell it hears restores, by spellbook position.
    pub(crate) mana: Vec<Option<f64>>,
    pub(crate) metrics: usize,
    /// Only a heal that crits restores the mana.
    pub(crate) crit_only: bool,
}

/// An item proc trigger that restores energy, such as Shadowcraft Armor's, or mana, such as
/// Beaststalker Armor's on a class other than a Hunter.
#[derive(Clone, Debug)]
pub(crate) struct EnergizeProc {
    pub(crate) label: String,
    /// The proc manager's chance for each spell it hears, by spellbook position.
    pub(crate) chances: Vec<Option<f64>>,
    /// Energy or mana.
    pub(crate) resource: super::ResourceKind,
    pub(crate) energy: f64,
    pub(crate) metrics: usize,
    pub(crate) delay: i64,
}

impl<A: Agent> Fight<A> {
    /// Go `AttachProcTriggerCallback` for an energize proc: a landed hit from a spell the proc
    /// manager hears rolls its chance, then the handler waits its delay.
    pub(crate) fn energize_proc_callback(
        &mut self,
        aura: super::AuraRef,
        index: usize,
        spell: super::SpellId,
        result: &super::SpellResult,
    ) {
        if !result.landed() {
            return;
        }
        let proc = &self.energize_procs[index];
        let Some(chance) = proc.chances[spell] else {
            return;
        };
        let delay = proc.delay;
        // Go `Proc(chance, label)`, reading the label in place.
        if !self.rng.proc(chance, &self.energize_procs[index].label) {
            return;
        }
        self.schedule_delayed_proc_at(self.now + delay, aura, spell, *result);
    }

    /// A cast the mana listener hears rolls its chance, then the handler waits a spell batch
    /// window.
    pub(crate) fn mana_on_cast_callback(
        &mut self,
        aura: super::AuraRef,
        index: usize,
        spell: super::SpellId,
    ) {
        let proc = &self.mana_on_cast[index];
        if proc.crit_only || proc.mana.get(spell).copied().flatten().is_none() {
            return;
        }
        let chance = proc.chance;
        // Go `Proc(chance, label)`, reading the label in place.
        if !self.rng.proc(chance, &self.mana_on_cast[index].label) {
            return;
        }
        let result = super::SpellResult {
            armor_multiplier: 0.0,
            target: Side::Player,
            attacker: Side::Player,
            outcome: 0,
            damage: 0.0,
            threat: 0.0,
        };
        self.schedule_delayed_proc(aura, spell, result);
    }

    /// A heal's crit the crit-only mana listener hears: the mana a spell batch window later.
    pub(crate) fn mana_on_crit_callback(
        &mut self,
        aura: super::AuraRef,
        index: usize,
        spell: super::SpellId,
        result: &super::SpellResult,
    ) {
        let proc = &self.mana_on_cast[index];
        if !proc.crit_only
            || result.outcome & super::damage::OUTCOME_CRIT == 0
            || proc.mana.get(spell).copied().flatten().is_none()
        {
            return;
        }
        if !self.rng.proc(proc.chance, &self.mana_on_cast[index].label) {
            return;
        }
        self.schedule_delayed_proc(aura, spell, *result);
    }

    /// The mana listener's handler: the cast's mana, for a character with a mana bar.
    pub(crate) fn mana_on_cast_handler(&mut self, index: usize, spell: super::SpellId) {
        if !self.has_mana_bar() {
            return;
        }
        let proc = &self.mana_on_cast[index];
        if let Some(mana) = proc.mana.get(spell).copied().flatten() {
            let metrics = proc.metrics;
            self.add_mana(mana, metrics);
        }
    }

    /// The energize proc's handler: energy for a character with an energy bar, or mana for one
    /// with a mana bar.
    pub(crate) fn energize_proc_handler(&mut self, index: usize) {
        let proc = &self.energize_procs[index];
        let (resource, amount, metrics) = (proc.resource, proc.energy, proc.metrics);
        match resource {
            super::ResourceKind::Mana if self.has_mana_bar() => self.add_mana(amount, metrics),
            super::ResourceKind::Energy if self.energy.is_some() => {
                self.add_energy(amount, metrics)
            }
            _ => {}
        }
    }

    /// Go `EnableEnergyBar`: the bar and its three metrics, registered in Go's order.
    pub(crate) fn enable_energy_bar(&mut self, energy: &Energy) {
        let other = |name: &str| ActionId {
            other_id: name.into(),
            ..ActionId::default()
        };
        let regen_metrics =
            self.new_resource_metrics(other("OtherActionEnergyRegen"), ResourceKind::Energy);
        let refund_metrics =
            self.new_resource_metrics(other("OtherActionRefund"), ResourceKind::Energy);
        let encounter_start_metrics =
            self.new_resource_metrics(other("OtherActionEncounterStart"), ResourceKind::Energy);
        self.energy = Some(EnergyBar {
            max: energy.max_energy.max(10.0),
            current: 0.0,
            max_combo_points: energy.max_combo_points,
            combo_points: 0,
            next_tick: 0,
            tick_duration: energy.tick_duration_ns,
            per_tick: energy.energy_per_tick,
            regen_multiplier: 1.0,
            regen_metrics,
            refund_metrics,
            encounter_start_metrics,
        });
    }

    /// The player's energy bar. Only a player with one casts energy spells.
    pub(crate) fn energy_bar(&self) -> &EnergyBar {
        self.energy.as_ref().expect("the player has an energy bar")
    }

    /// The player's energy, or none without an energy bar.
    pub(crate) fn energy_now(&self) -> f64 {
        self.energy.as_ref().map_or(0.0, |bar| bar.current)
    }

    fn energy_bar_mut(&mut self) -> &mut EnergyBar {
        self.energy.as_mut().expect("the player has an energy bar")
    }

    /// Go `energyBar.reset` and `enable`: a full bar, no combo points and the first tick at a
    /// random point of one tick period after the prepull starts.
    pub(crate) fn reset_energy(&mut self, prepull_start: i64) {
        let Some(bar) = self.energy.as_mut() else {
            return;
        };
        bar.current = bar.max;
        bar.combo_points = 0;
        bar.regen_multiplier = 1.0;
        let duration = self.energy_bar().tick_duration;
        let roll = self.random("Energy Tick");
        let next = prepull_start + (roll * duration as f64) as i64;
        self.energy_bar_mut().next_tick = next;
        self.reschedule_task(next);
    }

    /// Go `energyBar.RunTask`.
    pub(crate) fn run_energy_task(&mut self) -> i64 {
        let bar = self.energy_bar();
        if self.now < bar.next_tick {
            return bar.next_tick;
        }
        let (amount, metrics) = (bar.per_tick * bar.regen_multiplier, bar.regen_metrics);
        self.add_energy(amount, metrics);
        let next = self.now + self.energy_bar().tick_duration;
        self.energy_bar_mut().next_tick = next;
        next
    }

    /// Go `energyBar.AddEnergy`.
    pub(crate) fn add_energy(&mut self, amount: f64, metrics: usize) {
        assert!(amount >= 0.0, "Trying to add negative energy!");
        let bar = self.energy_bar();
        let (old, max) = (bar.current, bar.max);
        let new = (old + amount).min(max);
        let resource = &mut self.resources[metrics];
        resource.events += 1;
        resource.gain += amount;
        resource.actual_gain += new - old;
        if self.log.is_some() {
            let line = format!(
                "Gained {amount:.3} energy from {} ({old:.3} --> {new:.3}) of {max:.0} total.",
                action_string(&self.resources[metrics].id)
            );
            self.player_log(&line);
        }
        self.energy_bar_mut().current = new;
        self.changed(super::reads::RESOURCES);
        self.gained_energy();
    }

    /// Go `energyBar.SpendEnergy`.
    pub(crate) fn spend_energy(&mut self, amount: f64, metrics: usize) {
        assert!(amount >= 0.0, "Trying to spend negative energy!");
        let bar = self.energy_bar();
        let (old, max) = (bar.current, bar.max);
        let new = old - amount;
        let resource = &mut self.resources[metrics];
        resource.events += 1;
        resource.gain -= amount;
        resource.actual_gain -= amount;
        if self.log.is_some() {
            let line = format!(
                "Spent {amount:.3} energy from {} ({old:.3} --> {new:.3}) of {max:.0} total.",
                action_string(&self.resources[metrics].id)
            );
            self.player_log(&line);
        }
        self.energy_bar_mut().current = new;
        self.changed(super::reads::RESOURCES);
    }

    /// Go `energyBar.AddComboPoints`. Go formats the int32 maximum with `%0.0f`, which prints
    /// as a bad verb.
    pub(crate) fn add_combo_points(&mut self, points: i32, metrics: usize) {
        let bar = self.energy_bar();
        let (old, max) = (bar.combo_points, bar.max_combo_points);
        let new = (old + points).min(max);
        let resource = &mut self.resources[metrics];
        resource.events += 1;
        resource.gain += f64::from(points);
        resource.actual_gain += f64::from(new - old);
        if self.log.is_some() {
            let line = format!(
                "Gained {points} combo points from {} ({old} --> {new}) of %!f(int32={max}) total.",
                action_string(&self.resources[metrics].id)
            );
            self.player_log(&line);
        }
        self.energy_bar_mut().combo_points = new;
        self.changed(super::reads::RESOURCES);
    }

    /// Go `energyBar.SpendComboPoints`: every point.
    pub(crate) fn spend_combo_points(&mut self, metrics: usize) {
        let bar = self.energy_bar();
        let (old, max) = (bar.combo_points, bar.max_combo_points);
        let spent = old;
        let new = old - spent;
        if self.log.is_some() {
            let line = format!(
                "Spent {spent} combo points from {} ({old} --> {new}) of %!f(int32={max}) total.",
                action_string(&self.resources[metrics].id)
            );
            self.player_log(&line);
        }
        let resource = &mut self.resources[metrics];
        resource.events += 1;
        resource.gain -= f64::from(spent);
        resource.actual_gain -= f64::from(spent);
        self.energy_bar_mut().combo_points = new;
        self.changed(super::reads::RESOURCES);
    }

    /// Go `energyBar.IsTicking`: the bar ticks once reset, within one period of its next tick.
    fn energy_ticking(&self) -> bool {
        let bar = self.energy_bar();
        bar.next_tick != 0
            && bar.next_tick - self.now <= bar.tick_duration
            && self.now <= bar.next_tick
    }

    /// Go `energyBar.ResetEnergyTick`: an immediate partial tick, then a fresh period.
    pub(crate) fn reset_energy_tick(&mut self) {
        if !self.energy_ticking() {
            return;
        }
        let bar = self.energy_bar();
        let since = (self.now - (bar.next_tick - bar.tick_duration)).max(0);
        let partial =
            bar.per_tick * bar.regen_multiplier * (since as f64 / bar.tick_duration as f64);
        let metrics = bar.regen_metrics;
        self.add_energy(partial, metrics);
        let next = self.now + self.energy_bar().tick_duration;
        self.energy_bar_mut().next_tick = next;
        self.reschedule_task(next);
    }

    /// Go `energyBar.MultiplyEnergyRegenSpeed`; a multiplier of one changes nothing.
    pub(crate) fn multiply_energy_regen_speed(&mut self, multiplier: f64) {
        if multiplier == 1.0 {
            return;
        }
        self.reset_energy_tick();
        self.energy_bar_mut().regen_multiplier *= multiplier;
    }

    /// Go `energyBar.TimeToTargetEnergy`: none while the bar already holds the target, never
    /// when it does not tick or cannot hold it, otherwise until the first tick that reaches it.
    pub(crate) fn time_to_target_energy(&self, target: f64) -> i64 {
        let bar = self.energy_bar();
        if bar.current >= target {
            return 0;
        }
        let per_tick = bar.per_tick * bar.regen_multiplier;
        if !self.energy_ticking() || per_tick <= 0.0 || target > bar.max {
            return NEVER_EXPIRES;
        }
        let ticks = ((target - bar.current) / per_tick).ceil();
        self.time_to_next_energy_tick() + ((ticks - 1.0) * bar.tick_duration as f64) as i64
    }

    /// Go `energyBar.TimeToNextEnergyTick`.
    pub(crate) fn time_to_next_energy_tick(&self) -> i64 {
        (self.energy_bar().next_tick - self.now).max(0)
    }

    /// Go `Simulation.RescheduleTask`.
    pub(crate) fn reschedule_task(&mut self, time: i64) {
        self.min_task_time = self.min_task_time.min(time);
    }

    /// Go `Simulation.Step`'s task check: whether a task is due before the next action at
    /// `next`.
    pub(crate) fn due_task(&self, next: i64) -> bool {
        next >= self.min_task_time
    }

    /// Go `advanceTasks`: the player's energy bar, then a pet's focus bar.
    pub(crate) fn advance_tasks(&mut self) {
        if self.min_task_time > self.now {
            let time = self.min_task_time;
            self.advance_to(time);
        }
        self.min_task_time = NEVER_EXPIRES;
        if self.energy.is_some() {
            let next = self.run_energy_task();
            self.min_task_time = self.min_task_time.min(next);
        }
        // An enabled pet's focus task joins after the player's.
        for index in 0..self.active_pets.len() {
            let side = Side::Pet(index as u8);
            if self.pet_focus_task_due(side).is_some() {
                let next = self.run_focus_task(side);
                self.min_task_time = self.min_task_time.min(next);
            }
        }
    }
}
