//! Go dot.go: periodic auras and channels.

use crate::{contracts::prepared_v2::Dot as ExportedDot, core::queue::Handle};

use super::{
    cast::MAX_SPELL_QUEUE_WINDOW, Action, Agent, AuraRef, DotId, Fight, Side, SpellBehavior,
    SpellId, PRIORITY_GCD,
};

#[derive(Clone)]
pub(crate) struct Dot {
    pub(crate) spell: SpellId,
    pub(crate) side: Side,
    pub(crate) aura_label: String,
    pub(crate) aura: AuraRef,
    pub(crate) base_tick_count: i32,
    pub(crate) base_tick_length: i64,
    pub(crate) tick_period: i64,
    pub(crate) remaining_ticks: i32,
    pub(crate) base_duration_multiplier: f64,
    pub(crate) base_duration_flat: i64,
    pub(crate) affected_by_haste: bool,
    /// Go `affectedByCastSpeed`; a hasted dot without it is hasted by real ranged haste.
    pub(crate) affected_by_cast_speed: bool,
    /// Go `hasteReducesDuration`, which picks how `TickCount` counts.
    pub(crate) haste_reduces_duration: bool,
    pub(crate) channeled: bool,
    pub(crate) tick_action: Option<Handle>,
    /// Go `tickAction.NextActionAt`, kept after the action runs.
    pub(crate) tick_next_at: i64,
    pub(crate) bonus_coefficient: f64,
    pub(crate) periodic_damage_multiplier: f64,
    /// The base amount `Dot.Snapshot` starts from, for a damage dot its spell sets up.
    pub(crate) tick_base: Option<f64>,
    pub(crate) tick_can_crit: bool,
    pub(crate) snapshot_base: f64,
    pub(crate) snapshot_spell_power: f64,
    pub(crate) reads_spell_power: bool,
    /// Go `attackPowerShare`: the share of the caster's attack power the snapshot holds, which
    /// a tick reads again from the attack power it has then.
    pub(crate) attack_power_share: f64,
    /// Go `attackPowerRanged`: the share is of ranged attack power, not melee.
    pub(crate) attack_power_ranged: bool,
    /// Go `snapshotAttackPower`: that share of the attack power at the snapshot.
    pub(crate) snapshot_attack_power: f64,
}

impl Dot {
    /// A dot of `spell`, cast by `caster`; a dot on "self" is on the caster.
    pub(crate) fn new(spell: SpellId, caster: Side, exported: &ExportedDot) -> Self {
        Dot {
            spell,
            side: if exported.unit == "self" {
                caster
            } else {
                Side::Target
            },
            aura_label: exported.aura_label.clone(),
            aura: AuraRef {
                side: Side::Target,
                index: 0,
            },
            base_tick_count: exported.base_tick_count,
            base_tick_length: exported.base_tick_length_ns,
            tick_period: exported.base_tick_length_ns,
            remaining_ticks: exported.base_tick_count,
            base_duration_multiplier: exported.base_duration_multiplier,
            base_duration_flat: exported.base_duration_flat_ns,
            affected_by_haste: exported.affected_by_cast_speed || exported.affected_by_real_haste,
            affected_by_cast_speed: exported.affected_by_cast_speed,
            haste_reduces_duration: exported.haste_reduces_duration,
            channeled: exported.channeled,
            tick_action: None,
            tick_next_at: 0,
            bonus_coefficient: exported.bonus_coefficient,
            periodic_damage_multiplier: exported.periodic_damage_multiplier,
            tick_base: None,
            tick_can_crit: false,
            snapshot_base: 0.0,
            snapshot_spell_power: 0.0,
            reads_spell_power: false,
            attack_power_share: 0.0,
            attack_power_ranged: false,
            snapshot_attack_power: 0.0,
        }
    }

    /// Go `Dot.TickCount`: the ticks dealt so far. The runtime has no extra ticks and no
    /// hasted dots, so the hasted count is the base duration over the tick period.
    pub(crate) fn tick_count(&self) -> i32 {
        let total = if self.haste_reduces_duration {
            self.base_tick_count
        } else {
            (self.base_duration() as f64 / self.tick_period as f64).round_ties_even() as i32
        };
        total - self.remaining_ticks
    }

    /// Go `Dot.HastedTickCount`: the base duration over the snapshotted tick period.
    pub(crate) fn hasted_tick_count(&self) -> i32 {
        (self.base_duration() as f64 / self.tick_period as f64).round_ties_even() as i32
    }

    /// Go `Dot.BaseDuration`.
    fn base_duration(&self) -> i64 {
        (f64::from(self.base_tick_count)
            * self.base_tick_length as f64
            * self.base_duration_multiplier) as i64
            + self.base_duration_flat
    }
}

impl<A: Agent> Fight<A> {
    /// Go `Dot.Apply`: replace any running copy, snapshot, recompute ticks and activate.
    pub(crate) fn apply_dot(&mut self, dot: DotId) {
        let aura = self.dots[dot].aura;
        self.deactivate_aura(aura);
        if let Some(base) = self.dots[dot].tick_base {
            self.snapshot_dot(dot, base);
        }
        // Go recomputeAuraDuration.
        let period = self.calc_tick_period(dot);
        let state = &mut self.dots[dot];
        state.tick_period = period;
        let ticks = state.base_duration() as f64 / state.base_tick_length as f64;
        state.remaining_ticks = ticks.round_ties_even() as i32;
        if state.affected_by_haste && !state.haste_reduces_duration {
            state.remaining_ticks = state.hasted_tick_count();
        }
        let duration = state.tick_period * i64::from(state.remaining_ticks);
        self.aura_mut(aura).duration = duration;
        self.activate_aura(aura);
    }

    /// Go `Dot.ApplyKeepingTickTimer`: a running dot keeps its tick timer. Its duration starts
    /// over from now, the next tick stays where it was, and the ticks left are the ones that fit
    /// between that tick and the new expiry. A dot that is not running applies as usual. The
    /// aura refreshes, so nothing fades and nothing is gained.
    pub(crate) fn apply_dot_keeping_tick_timer(&mut self, dot: DotId) {
        let aura = self.dots[dot].aura;
        if !self.aura(aura).active || self.dots[dot].tick_action.is_none() {
            self.apply_dot(dot);
            return;
        }
        let until_next_tick = self.dots[dot].tick_next_at - self.now;
        if let Some(base) = self.dots[dot].tick_base {
            self.snapshot_dot(dot, base);
        }
        // Go recomputeAuraDuration.
        let period = self.calc_tick_period(dot);
        let state = &mut self.dots[dot];
        state.tick_period = period;
        let ticks = state.base_duration() as f64 / state.base_tick_length as f64;
        state.remaining_ticks = ticks.round_ties_even() as i32;
        if state.affected_by_haste && !state.haste_reduces_duration {
            state.remaining_ticks = state.hasted_tick_count();
        }
        let duration = state.tick_period * i64::from(state.remaining_ticks);
        // Go keeps the tick timer: the ticks left fit between the next tick and the expiry. A tick
        // due this very moment still lands, one more than a fresh dot has. The runtime counts no
        // extra ticks, since nothing reads them.
        state.remaining_ticks = ((duration - until_next_tick) / state.tick_period) as i32 + 1;
        self.aura_mut(aura).duration = duration;
        self.activate_aura(aura);
    }

    /// Go `Dot.CalcTickPeriod`: a dot affected by cast speed or by real haste ticks faster,
    /// rounded to the millisecond as in game; a channel also takes the spell's cast time
    /// multiplier.
    pub(crate) fn calc_tick_period(&self, dot: DotId) -> i64 {
        let state = &self.dots[dot];
        if !state.affected_by_haste {
            return state.base_tick_length;
        }
        if !state.affected_by_cast_speed {
            // Go `ApplyRealRangedHaste` on the caster: the base over its real ranged haste.
            let haste = self.real_ranged_haste_multiplier_of(self.caster(state.spell));
            return round_to_millisecond((state.base_tick_length as f64 / haste) as i64);
        }
        let hasted = if state.channeled {
            self.apply_cast_speed_for_spell(state.base_tick_length, state.spell)
        } else {
            self.apply_cast_speed_of(self.caster(state.spell), state.base_tick_length)
        };
        round_to_millisecond(hasted)
    }

    /// Go `Dot.Snapshot`: the base amount plus the spell power share it was given now. The
    /// reference arm64 build fuses the share's multiply into that add, though it also stores
    /// the share: Go's ARM64 rules fuse an add of a product without checking its other uses.
    pub(crate) fn snapshot_dot(&mut self, dot: DotId, base: f64) {
        let spell = self.dots[dot].spell;
        let coefficient = self.dots[dot].bonus_coefficient;
        let (spell_power, snapshot) = if coefficient > 0.0 {
            let bonus = self.bonus_damage(spell, self.dots[dot].side);
            (coefficient * bonus, coefficient.mul_add(bonus, base))
        } else {
            (0.0, base)
        };
        let state = &mut self.dots[dot];
        state.reads_spell_power = coefficient > 0.0;
        state.snapshot_spell_power = spell_power;
        state.snapshot_base = snapshot;
        state.attack_power_share = 0.0;
        state.attack_power_ranged = false;
        state.snapshot_attack_power = 0.0;
    }

    /// Go `Dot.SnapshotAttackPowerShare`: `share` of the caster's attack power, ranged when
    /// `ranged` is set, is already in the base handed to [`Fight::snapshot_dot`], so a tick
    /// reads that share from the attack power it has then.
    pub(crate) fn snapshot_dot_attack_power_share(&mut self, dot: DotId, share: f64, ranged: bool) {
        let state = &mut self.dots[dot];
        state.attack_power_share = share;
        state.attack_power_ranged = ranged;
        let attack_power = self.dot_attack_power(dot);
        self.dots[dot].snapshot_attack_power = share * attack_power;
    }

    /// Go `Dot.currentAttackPower`: the caster's ranged or melee attack power, as the dot's
    /// share names.
    pub(crate) fn dot_attack_power(&self, dot: DotId) -> f64 {
        if self.dots[dot].attack_power_ranged {
            self.ranged_attack_power()
        } else {
            self.melee_attack_power_of(self.caster(self.dots[dot].spell))
        }
    }

    /// The dots that share an aura with this one, which Go registers once for each label: the
    /// aura's gain and expiry run every dot's callbacks, in the order the dots were created.
    fn dot_siblings(&self, dot: DotId) -> Vec<DotId> {
        self.dot_siblings.get(dot).cloned().unwrap_or_default()
    }

    /// Go `newDot` OnGain, for the dot and each dot sharing its aura.
    pub(crate) fn dot_on_gain(&mut self, dot: DotId) {
        self.dot_gain(dot);
        for sibling in self.dot_siblings(dot) {
            self.dot_gain(sibling);
        }
    }

    /// Go `newDot` OnExpire, for the dot and each dot sharing its aura.
    pub(crate) fn dot_on_expire(&mut self, dot: DotId) {
        self.dot_expire(dot);
        for sibling in self.dot_siblings(dot) {
            self.dot_expire(sibling);
        }
    }

    /// One dot's `newDot` OnGain: the first tick is one period away.
    fn dot_gain(&mut self, dot: DotId) {
        if let SpellBehavior::Class(behavior) = self.spells[self.dots[dot].spell].behavior {
            A::on_dot_gain(self, dot, behavior);
        }
        let at = self.now + self.dots[dot].tick_period;
        let handle = self.schedule(at, PRIORITY_GCD, Action::DotTick(dot));
        let state = &mut self.dots[dot];
        state.tick_action = Some(handle);
        state.tick_next_at = at;
        if state.channeled {
            self.player.channeled_dot = Some(dot);
            self.changed(super::reads::CASTS);
        }
    }

    /// One dot's `newDot` OnExpire: a tick due now runs first, then the channel ends.
    fn dot_expire(&mut self, dot: DotId) {
        if let SpellBehavior::Class(behavior) = self.spells[self.dots[dot].spell].behavior {
            A::on_dot_expire(self, dot, behavior);
        }
        // A committed lethal world ACK ends target damage. Cleanup still expires
        // auras, but a tick due at that same clock cannot hit the dead target.
        let world_target_is_lethal = matches!(self.dots[dot].side, Side::Target)
            && super::world_bridge::enabled()
            && super::world_bridge::lethal_ack_received();
        if self.dots[dot].tick_next_at == self.now
            && should_process_expiry_tick(self.dots[dot].side, world_target_is_lethal)
        {
            self.dots[dot].remaining_ticks -= 1;
            self.tick_once(dot);
        }
        if let Some(handle) = self.dots[dot].tick_action.take() {
            self.queue.cancel(handle);
        }
        if self.dots[dot].channeled {
            let delay = self.config.channel_clip_delay;
            self.player.channeled_dot = None;
            self.changed(super::reads::CASTS);
            self.forget_channel_interrupt();
            if self.player.gcd <= self.now {
                self.wait_until(self.now + delay);
            }
            let aura = self.dots[dot].aura;
            let channel_time = self.aura(aura).fade_time - self.aura(aura).start;
            let spell = self.dots[dot].spell;
            self.spells[spell].metrics[aura.side.index()].total_cast_time += channel_time;
        }
        // Go clears the stored amount once the dot is gone.
        self.dots[dot].snapshot_base = 0.0;
    }

    /// Go `Dot.TickOnce`.
    pub(crate) fn tick_once(&mut self, dot: DotId) {
        let spell = self.dots[dot].spell;
        match self.spells[spell].behavior.clone() {
            SpellBehavior::Class(behavior) => A::on_dot_tick(self, dot, behavior),
            // Go `CalcAndDealPeriodicSnapshotDamage` with `OutcomeTick`.
            SpellBehavior::SulfurasFireball { .. } => self.snapshot_dot_tick(dot),
            // Go spelldata PeriodicDamageTick: the row's amount on current stats.
            SpellBehavior::OnUseDamage(params) => {
                if let Some(base) = params.tick_base() {
                    self.periodic_damage_tick(dot, base);
                }
            }
            // Go consumes_forever_potions.go: `CalcAndDealPeriodicDamage` with `OutcomeTick`.
            SpellBehavior::DragonfirePotion { tick, .. } => {
                self.periodic_damage_tick_outcome(dot, tick, super::Outcome::Tick);
            }
            // Go `tick * (1 + variance*(2*roll - 1))`, which the arm64 build fuses.
            SpellBehavior::CausticSmogPotion {
                tick,
                variance,
                label,
            } => {
                let roll = self.random(&label).mul_add(2.0, -1.0);
                let damage = tick * variance.mul_add(roll, 1.0);
                self.periodic_damage_tick_outcome(dot, damage, super::Outcome::Tick);
            }
            // Go spelldata `PeriodicDamageTick` on the outcome applier the row names.
            SpellBehavior::ProcDamage(params) => self.proc_damage_tick(dot, params),
            // Go sim/warrior/items.go Diamond Flask: the last tick activates the Strength aura.
            SpellBehavior::DiamondFlask(aura) => {
                if self.dots[dot].remaining_ticks == 0 {
                    self.activate_aura(AuraRef {
                        side: Side::Player,
                        index: aura,
                    });
                }
            }
            // Go spell_data_energize.go: each tick of the self hot rolls the gain.
            SpellBehavior::EnergizeOnUse {
                average,
                variance,
                periodic: true,
                bar,
                ..
            } => {
                let gain = self.effect_roll(average, variance);
                self.energize_bar(spell, bar, gain);
            }
            _ => {}
        }
    }

    /// Go `Dot.periodicTick`.
    pub(crate) fn periodic_tick(&mut self, dot: DotId, handle: Handle) {
        if self.dots[dot].tick_action != Some(handle) {
            return;
        }
        self.dots[dot].remaining_ticks -= 1;
        self.tick_once(dot);
        if self.dots[dot].channeled {
            if self.dots[dot].remaining_ticks == 0 && self.gcd_ready() {
                let delay = self.config.channel_clip_delay;
                self.wait_until(self.now + delay);
            } else if self.should_interrupt_channel()
                // Interrupts the spell queue window alone would trigger wait for the GCD.
                && (self.gcd_ready() || self.gcd_time_to_ready() > MAX_SPELL_QUEUE_WINDOW)
            {
                self.interrupt_channel(dot);
                return;
            }
        }
        let aura = self.dots[dot].aura;
        let state = &self.dots[dot];
        if self.aura(aura).active && (!state.channeled || state.remaining_ticks > 0) {
            let at = self.now + state.tick_period;
            let handle = self.schedule(at, PRIORITY_GCD, Action::DotTick(dot));
            let state = &mut self.dots[dot];
            state.tick_action = Some(handle);
            state.tick_next_at = at;
        }
    }
}

fn should_process_expiry_tick(side: Side, world_target_is_lethal: bool) -> bool {
    !matches!(side, Side::Target) || !world_target_is_lethal
}

/// Go `Duration.Round(time.Millisecond)`: to the nearest millisecond, halves away from zero.
fn round_to_millisecond(duration: i64) -> i64 {
    let unit = crate::core::time::NS_PER_MILLISECOND;
    let mut remainder = duration % unit;
    if duration < 0 {
        remainder = -remainder;
        if remainder + remainder < unit {
            return duration + remainder;
        }
        return duration - unit + remainder;
    }
    if remainder + remainder < unit {
        duration - remainder
    } else {
        duration + unit - remainder
    }
}

#[cfg(test)]
mod tests {
    use super::{round_to_millisecond, should_process_expiry_tick, Side};

    #[test]
    fn lethal_world_death_skips_only_target_final_dot_tick() {
        assert!(!should_process_expiry_tick(Side::Target, true));
        assert!(should_process_expiry_tick(Side::Target, false));
        assert!(should_process_expiry_tick(Side::Player, true));
    }

    #[test]
    fn rounds_like_go_duration_round() {
        assert_eq!(round_to_millisecond(1_000_499_999), 1_000_000_000);
        assert_eq!(round_to_millisecond(1_000_500_000), 1_001_000_000);
        assert_eq!(round_to_millisecond(909_090_909), 909_000_000);
        assert_eq!(round_to_millisecond(-1_500_000), -2_000_000);
    }
}
