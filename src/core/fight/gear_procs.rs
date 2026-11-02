//! Shared gear procs of Go sim/common/forever: Battlegear of Valor's Warrior's Resolve, which
//! heals and gives rage, and the armor debuff weapon procs of Bashguuder and Rivenspike (Puncture
//! Armor) and Annihilator (Armor Shatter), whose stacks lower the target's armor.

use crate::contracts::prepared_v2::Effect;

use super::{aura::AuraBehavior, Agent, AuraRef, Fight, ResourceKind, Side, SpellId, SpellResult};

/// item_sets_classic.go Warrior's Resolve: each spell's chance from the set's proc manager,
/// the roll's label, the heal range, the rage and their metrics.
#[derive(Clone, Debug)]
pub(crate) struct HealthRageProc {
    chances: Vec<Option<f64>>,
    rng_label: String,
    heal_min: f64,
    heal_max: f64,
    rage: f64,
    health_metrics: usize,
    rage_metrics: Option<usize>,
}

/// items.go Rogue Armor Energize: each spell's chance from the set's proc manager, the roll's
/// label, the energy and its metrics.
#[derive(Clone, Debug)]
pub(crate) struct EnergyProc {
    chances: Vec<Option<f64>>,
    rng_label: String,
    energy: f64,
    metrics: usize,
}

/// items_weapons.go Puncture Armor and Annihilator's Armor Shatter: each spell's chance from the
/// weapon's proc manager, the roll's label, the target's aura and the target's armor change at
/// each stack count.
#[derive(Clone, Debug)]
pub(crate) struct ArmorDebuffProc {
    /// Each spell's chance, which an item swap rebuilds for the weapons in the hands.
    pub(crate) chances: Vec<Option<f64>>,
    rng_label: String,
    aura: AuraRef,
    armor_by_stacks: Vec<f64>,
    /// Go `TriggerImmediately`: the handler runs on the hit, not a batch window later.
    immediate: bool,
}

/// The behavior of a player or target aura these procs own.
pub(crate) fn behavior<K>(effects: &[Effect], side: Side, label: &str) -> Option<AuraBehavior<K>> {
    let mut health_rage = 0;
    let mut armor = 0;
    let mut energy = 0;
    for effect in effects {
        match effect {
            Effect::HealthRageProc { trigger_aura, .. } => {
                if side == Side::Player && trigger_aura == label {
                    return Some(AuraBehavior::HealthRageProc(health_rage));
                }
                health_rage += 1;
            }
            Effect::EnergyProc { trigger_aura, .. } => {
                if side == Side::Player && trigger_aura == label {
                    return Some(AuraBehavior::EnergyProc(energy));
                }
                energy += 1;
            }
            Effect::ArmorDebuffProc {
                trigger_aura, aura, ..
            } => {
                if side == Side::Player && trigger_aura == label {
                    return Some(AuraBehavior::ArmorDebuffTrigger(armor));
                }
                if side == Side::Target && aura == label {
                    return Some(AuraBehavior::ArmorDebuff(armor));
                }
                armor += 1;
            }
            _ => {}
        }
    }
    None
}

fn by_spell(
    chances: &[crate::contracts::prepared_v2::SpellChance],
    spells: usize,
) -> Vec<Option<f64>> {
    let mut by_spell = vec![None; spells];
    for entry in chances {
        if let Some(slot) = by_spell.get_mut(entry.spell) {
            *slot = Some(entry.chance);
        }
    }
    by_spell
}

impl<A: Agent> Fight<A> {
    /// Bind the gear procs, in effect order as [`behavior`] counts them.
    pub(crate) fn bind_gear_procs(&mut self, effects: &[Effect]) -> Result<(), String> {
        for effect in effects {
            match effect {
                Effect::HealthRageProc {
                    rng_label,
                    chances,
                    heal_min,
                    heal_max,
                    rage,
                    metrics_action_id,
                    ..
                } => {
                    // Go registers the health metrics, then the rage metrics.
                    let health_metrics =
                        self.new_resource_metrics(metrics_action_id.clone(), ResourceKind::Health);
                    let rage_metrics = (*rage > 0.0).then(|| {
                        self.new_resource_metrics(metrics_action_id.clone(), ResourceKind::Rage)
                    });
                    self.health_rage_procs.push(HealthRageProc {
                        chances: by_spell(chances, self.spells.len()),
                        rng_label: rng_label.clone(),
                        heal_min: *heal_min,
                        heal_max: *heal_max,
                        rage: *rage,
                        health_metrics,
                        rage_metrics,
                    });
                }
                Effect::EnergyProc {
                    rng_label,
                    chances,
                    energy,
                    metrics_action_id,
                    ..
                } => {
                    let metrics =
                        self.new_resource_metrics(metrics_action_id.clone(), ResourceKind::Energy);
                    self.energy_procs.push(EnergyProc {
                        chances: by_spell(chances, self.spells.len()),
                        rng_label: rng_label.clone(),
                        energy: *energy,
                        metrics,
                    });
                }
                Effect::ArmorDebuffProc {
                    rng_label,
                    chances,
                    aura,
                    armor_by_stacks,
                    immediate,
                    ..
                } => {
                    let index = self.trackers[Side::Target.index()]
                        .find(aura)
                        .ok_or_else(|| format!("target aura {aura} is not registered"))?;
                    self.armor_debuff_procs.push(ArmorDebuffProc {
                        chances: by_spell(chances, self.spells.len()),
                        rng_label: rng_label.clone(),
                        aura: AuraRef {
                            side: Side::Target,
                            index,
                        },
                        armor_by_stacks: armor_by_stacks.clone(),
                        immediate: *immediate,
                    });
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Go `AttachProcTriggerCallback` for Warrior's Resolve: landed hits the manager hears,
    /// its roll under the trigger's name, then the handler a batch window later.
    pub(crate) fn health_rage_proc_callback(
        &mut self,
        aura: AuraRef,
        proc: usize,
        spell: SpellId,
        result: &SpellResult,
    ) {
        if result.outcome & super::OUTCOME_LANDED == 0 {
            return;
        }
        let Some(chance) = self.health_rage_procs[proc].chances[spell] else {
            return;
        };
        if self
            .rng
            .proc(chance, &self.health_rage_procs[proc].rng_label)
        {
            self.schedule_delayed_proc(aura, spell, *result);
        }
    }

    /// Go `AttachProcTriggerCallback` for Rogue Armor Energize: a landed hit the manager hears,
    /// its roll under the trigger's name, then the handler a batch window later.
    pub(crate) fn energy_proc_callback(
        &mut self,
        aura: AuraRef,
        proc: usize,
        spell: SpellId,
        result: &SpellResult,
    ) {
        if result.outcome & super::OUTCOME_LANDED == 0 {
            return;
        }
        let Some(chance) = self.energy_procs[proc].chances[spell] else {
            return;
        };
        if self.rng.proc(chance, &self.energy_procs[proc].rng_label) {
            self.schedule_delayed_proc(aura, spell, *result);
        }
    }

    /// Rogue Armor Energize's handler: the energy, with an energy bar.
    pub(crate) fn energy_proc_handler(&mut self, proc: usize) {
        let (energy, metrics) = (
            self.energy_procs[proc].energy,
            self.energy_procs[proc].metrics,
        );
        if self.energy.is_some() {
            self.add_energy(energy, metrics);
        }
    }

    /// Warrior's Resolve's handler: `GainHealth(Roll(min, max))`, then the rage with a bar.
    pub(crate) fn health_rage_proc_handler(&mut self, proc: usize) {
        let state = &self.health_rage_procs[proc];
        let (min, max, rage) = (state.heal_min, state.heal_max, state.rage);
        let (health_metrics, rage_metrics) = (state.health_metrics, state.rage_metrics);
        let heal = self.go_roll(min, max);
        self.gain_health(heal, health_metrics);
        if let Some(metrics) = rage_metrics {
            self.add_rage(rage, metrics);
        }
    }

    /// Go `AttachProcTriggerCallback` for an armor debuff weapon proc: the handler runs a batch
    /// window later, or at once for Annihilator's (`TriggerImmediately`).
    pub(crate) fn armor_debuff_proc_callback(
        &mut self,
        aura: AuraRef,
        proc: usize,
        spell: SpellId,
        result: &SpellResult,
    ) {
        if result.outcome & super::OUTCOME_LANDED == 0 {
            return;
        }
        let Some(chance) = self.armor_debuff_procs[proc].chances[spell] else {
            return;
        };
        if self
            .rng
            .proc(chance, &self.armor_debuff_procs[proc].rng_label)
        {
            if self.armor_debuff_procs[proc].immediate {
                self.armor_debuff_proc_handler(proc, result.target);
            } else {
                self.schedule_delayed_proc(aura, spell, *result);
            }
        }
    }

    /// The handler: activate the hit target's aura and add a stack.
    pub(crate) fn armor_debuff_proc_handler(&mut self, proc: usize, target: Side) {
        let aura = self.aura_on(self.armor_debuff_procs[proc].aura, target);
        self.activate_aura(aura);
        self.add_stack(aura);
    }

    /// The aura's `OnStacksChange`: Go `AddStatDynamic` on its target's armor by the change.
    pub(crate) fn armor_debuff_stacks_changed(
        &mut self,
        proc: usize,
        target: Side,
        old: i32,
        new: i32,
    ) {
        let armor = &self.armor_debuff_procs[proc].armor_by_stacks;
        let at = |stacks: i32| armor[(stacks.max(0) as usize).min(armor.len() - 1)];
        let delta = at(new) - at(old);
        self.add_target_armor(target, delta);
    }
}
