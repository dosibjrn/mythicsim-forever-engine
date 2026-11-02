//! Go apl.go: compile the parsed rotation and run `DoNextAction`.

use crate::{
    contracts::prepared_v2::ActionId,
    core::time::NEVER_EXPIRES,
    rotation::{
        bind, compile_bool_value, compile_condition, compile_duration_value, compile_raw_value,
        Action as ParsedAction, CompareOp, CompiledCondition, DotAt, FoundAura, Lookup, MathOp,
        Reference, Rotation, Step, Unit, Value, ValueType,
    },
};

use super::{
    cast::MAX_SPELL_QUEUE_WINDOW, exclusive::RefreshReading, Agent, AuraRef, DotId, Fight,
    ResourceKind, Side, SpellId,
};

use std::{ops::Range, rc::Rc};

mod lowered;

use super::log::action_string;
use super::reads;
use super::rotation_checks::Level;
use lowered::Cond;

pub(crate) type Compiled = crate::rotation::Compiled<AuraRef>;

#[derive(Clone, Debug)]
pub(crate) enum Act {
    /// A cast on the unit the action names: the current target, or the player for
    /// `castFriendlySpell` at the player.
    Cast(SpellId, Side),
    Autocast,
    /// Go `APLActionStrictSequence`: its casts, the units they are cast on, and the next one
    /// to run.
    StrictSequence {
        /// Each step's cast; [`NO_SPELL`] for a change of target.
        spells: Vec<SpellId>,
        targets: Vec<Side>,
        /// Each step that is Go `APLActionChangeTarget`: the target position it changes to, and
        /// when it last ran.
        changes: Vec<Option<(usize, i64)>>,
        /// Each step's own condition, `None` for one without.
        conditions: Vec<Option<Rc<Cond>>>,
        next: usize,
    },
    /// Go `APLActionSequence`: its name, its casts, the units they are cast on, and the next
    /// one to run.
    Sequence {
        name: Rc<str>,
        spells: Vec<SpellId>,
        targets: Vec<Side>,
        /// Each step's own condition.
        conditions: Vec<Option<Rc<Cond>>>,
        next: usize,
    },
    /// Go `APLActionChannelSpell` with an interrupt condition.
    Channel {
        spell: SpellId,
        interrupt: Rc<Compiled>,
        allow_recast: bool,
    },
    /// Go `APLActionMove`: the range from the target the player runs to, read as a float.
    Move(Rc<Compiled>),
    /// Go `APLActionMoveDuration`: the duration of the move, read as a duration.
    MoveDuration(Rc<Compiled>),
    /// Go `APLActionGroupReference`: the first of an instance's actions that is ready runs. A
    /// reference that bound no instance is never ready.
    Group {
        instance: Option<usize>,
    },
    /// Go `APLActionMultidot`: its dot count after the encounter's target count capped it,
    /// and its overlap.
    Multidot {
        spell: SpellId,
        /// The spell whose dot it reads: its own, or its related dot spell's.
        dot_spell: SpellId,
        max_dots: i32,
        overlap: Option<Rc<Compiled>>,
    },
    /// Go `APLActionItemSwap` to Swap1 (true) or Main.
    ItemSwap(bool),
    /// Go `APLActionWait`: the duration, and when the rotation it controls waits until.
    Wait {
        duration: Rc<Compiled>,
        until: i64,
    },
    /// Go `APLActionWaitUntil`: the condition the rotation it controls waits for.
    WaitUntil(Rc<Compiled>),
    /// Go `APLActionSchedule`: the times, the next one, and the item of the inner action.
    Schedule {
        timings: Rc<[i64]>,
        next: usize,
        inner: usize,
    },
    /// Go `APLActionStrictMultidot`: the spell it casts and the one whose dot it reads, its dot
    /// count and overlap, and the targets the last readiness check found, which its strict
    /// sequence runs in order.
    StrictMultidot {
        spell: SpellId,
        dot_spell: SpellId,
        max_dots: i32,
        overlap: Rc<Compiled>,
        ready: Vec<Side>,
        next: usize,
    },
    /// Go `APLActionCancelAura`.
    CancelAura(AuraRef),
    /// Go `APLActionChangeTarget` to the first target, which the current target already is:
    /// when it last ran, which lets it run once a timestep.
    ChangeTarget {
        last: i64,
        /// The target position the current target moves to.
        to: usize,
    },
    /// Go `APLActionCastWarlockAssignedCurse`: the curse, cast without queueing.
    CastAssignedCurse(SpellId, Side),
    /// Go `APLActionCastAllStatBuffCooldowns`: its spells and those the last readiness check
    /// found ready.
    CastAllStatBuffCooldowns {
        spells: Rc<[SpellId]>,
        ready: Vec<SpellId>,
    },
    /// Go `APLActionResetSequence`: the name, and the sequence of that name the rotation's
    /// finalizing found.
    ResetSequence {
        name: Rc<str>,
        sequence: Option<usize>,
    },
}

/// A strict sequence step as compiled: its cast and the unit, or its change of target.
type StepCast = (SpellId, Side, Option<(usize, i64)>);

/// A step that casts nothing: a strict sequence's change of target.
pub(crate) const NO_SPELL: SpellId = SpellId::MAX;

impl Act {
    /// Takes the state Go keeps in an action from the same item of another copy of the rotation,
    /// compiled for another current target.
    fn carry_state(&mut self, from: &Act) {
        match (self, from) {
            (
                Act::StrictSequence { next, changes, .. },
                Act::StrictSequence {
                    next: from_next,
                    changes: from_changes,
                    ..
                },
            ) => {
                *next = *from_next;
                for (change, from_change) in changes.iter_mut().zip(from_changes) {
                    if let (Some(change), Some(from_change)) = (change, from_change) {
                        change.1 = from_change.1;
                    }
                }
            }
            (
                Act::Sequence { next, .. },
                Act::Sequence {
                    next: from_next, ..
                },
            )
            | (
                Act::Schedule { next, .. },
                Act::Schedule {
                    next: from_next, ..
                },
            ) => {
                *next = *from_next;
            }
            (
                Act::Wait { until, .. },
                Act::Wait {
                    until: from_until, ..
                },
            ) => *until = *from_until,
            (
                Act::ChangeTarget { last, .. },
                Act::ChangeTarget {
                    last: from_last, ..
                },
            ) => {
                *last = *from_last;
            }
            (
                Act::StrictMultidot { ready, next, .. },
                Act::StrictMultidot {
                    ready: from_ready,
                    next: from_next,
                    ..
                },
            ) => {
                ready.clone_from(from_ready);
                *next = *from_next;
            }
            (
                Act::CastAllStatBuffCooldowns { ready, .. },
                Act::CastAllStatBuffCooldowns {
                    ready: from_ready, ..
                },
            ) => ready.clone_from(from_ready),
            _ => {}
        }
    }
}

/// The compiled rotation: the priority list's items first, then those of each group instance.
#[derive(Debug)]
pub(crate) struct Compilation {
    pub(crate) items: Vec<Item>,
    /// How many of the items are the priority list's.
    pub(crate) priority: usize,
    /// Where each group instance's items are.
    pub(crate) groups: Vec<Range<usize>>,
    /// The priority list's sequences by name, in the order Go's finalizing finds them.
    pub(crate) sequences: Vec<(Rc<str>, usize)>,
}

impl Compilation {
    /// Whether a Move sits anywhere in the rotation: the priority list, a group instance or a
    /// schedule's inner action. A prepull move does not count.
    pub(crate) fn has_move(&self) -> bool {
        self.items
            .iter()
            .any(|item| matches!(item.action, Act::Move(_)))
    }
}

/// A prepull action: a cast, or Go `APLActionActivateAura`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PrepullAct {
    Cast(SpellId),
    /// Go `APLActionCastFriendlySpell` at the player.
    CastAtPlayer(SpellId),
    /// Go `APLActionChannelSpell` with an interrupt condition: the channel, and the item
    /// holding the condition the rotation then reads.
    Channel(SpellId, usize),
    /// Go `APLActionItemSwap` to Swap1 (true) or Main.
    ItemSwap(bool),
    /// The swap back to Main Go registers when the prepull's last item swap is to another set.
    SwapBackToMain,
    /// Go `APLActionCastAllStatBuffCooldowns.Execute` before the pull: every one of its spells,
    /// by the position of its list.
    CastAllStatBuffCooldowns(usize),
    /// Go `APLActionActivateAllStatBuffProcAuras.Execute`: the set it runs for (Swap1 when true)
    /// and the position of its auras' list.
    ActivateAllStatBuffProcAuras(bool, usize),
    ActivateAura(AuraRef),
    /// Go `APLActionMove`: the player runs to the range from the target.
    Move(f64),
    /// Go `APLActionMoveDuration`: the player moves for the duration.
    MoveDuration(i64),
    /// Go `APLActionGroupReference.Execute` of a group of moves: the player runs to the range
    /// unless it is already moving or casting.
    GroupMove(f64),
    /// Go `APLActionCancelAura`.
    CancelAura(AuraRef),
    /// Go `APLActionActivateAuraWithStacks`, its stacks capped at the aura's maximum.
    ActivateAuraWithStacks(AuraRef, i32),
    /// Go `APLActionTriggerICD`.
    TriggerIcd(AuraRef),
    /// Go `APLActionChangeTarget` to the first target.
    ChangeTarget,
    /// Go `APLActionStrictSequence.Execute`, which a prepull runs without asking whether it is
    /// ready: the item holding the sequence takes control of the rotation.
    StrictSequence(usize),
}

/// The spells or auras a prepull action that runs several of them holds.
#[derive(Clone, Debug)]
pub(crate) enum PrepullList {
    Spells(Vec<SpellId>),
    Auras(Vec<AuraRef>),
}

/// A ready action. Go keeps the cooldown found by `IsReady` for `Execute`.
enum Ready {
    Cast(SpellId, Side),
    Autocast(usize),
    /// A strict sequence taking control of the rotation.
    Sequence(usize),
    /// A sequence's next step.
    SequenceStep(usize),
    /// A channel and the item whose interrupt condition it carries.
    Channel(usize, SpellId),
    /// A move to the range of the item's value from the target.
    Move(usize),
    /// A move for the duration of the item's value.
    MoveDuration(usize),
    /// A group reference and the item of its instance that runs.
    Group(usize),
    /// An item swap to Swap1 (true) or Main.
    ItemSwap(bool),
    /// A wait taking control of the rotation.
    Wait(usize),
    /// A wait until a condition taking control of the rotation.
    WaitUntil(usize),
    /// A schedule takes its next time, then its inner action runs.
    Scheduled(usize, Box<Ready>),
    /// A sequence starting over.
    ResetSequence(usize),
    /// A strict multidot taking control of the rotation.
    StrictMultidot(usize),
    /// An aura cancelled.
    CancelAura(AuraRef),
    /// The target changed to the first.
    ChangeTarget(usize),
    /// A strict sequence's change of target: the item and the step.
    SequenceChangeTarget(usize, usize),
    /// The assigned curse cast.
    CastAssignedCurse(SpellId, Side),
    /// The stat buff cooldowns cast.
    CastAllStatBuffCooldowns(usize),
}

#[derive(Clone, Debug)]
pub(crate) struct Item {
    /// Shared so evaluation, which can change the fight, never borrows the rotation.
    condition: Option<Rc<Cond>>,
    action: Act,
    /// Whether the condition runs a cost check (Go `APLValueSpellCanCast`), whose bookkeeping
    /// must happen whenever Go asks it.
    asks_cost: bool,
    /// Whether the condition's checks are all `spellCanCast` nodes of its own: asked where
    /// asking each spell leaves no trace, the condition keeps none either.
    asks_spell_costs: bool,
}

impl Item {
    fn new(condition: Option<Rc<Cond>>, action: Act) -> Item {
        let asks_cost = condition.as_deref().is_some_and(Cond::asks_cost);
        let asks_spell_costs = asks_cost
            && condition
                .as_deref()
                .is_some_and(|condition| condition.cost_checks_all(&|_| true));
        Item {
            condition,
            action,
            asks_cost,
            asks_spell_costs,
        }
    }
}

/// Go `APLRotation` fields beyond the priority list: the controlling strict sequence, the
/// sequence flag that lifts major cooldown restrictions, the running channel's interrupt
/// condition and the sequences hooked to the queued spell's action.
#[derive(Clone, Debug, Default)]
pub(crate) struct AplState {
    pub(crate) controlling: Option<usize>,
    pub(crate) in_sequence: bool,
    interrupt_channel_if: Option<usize>,
    allow_channel_recast: bool,
    pub(crate) queue_hooks: Vec<usize>,
    /// Each item's proof that it is not ready, while it holds.
    proofs: Vec<Proof>,
    /// The last look that found nothing, while what it read stays put.
    look: Option<Look>,
    /// While a look walks the list: until when every item it asked keeps its answer.
    walk_until: i64,
    /// While a look walks the list: the energy below which every answer it asked holds as
    /// energy rises.
    walk_energy: f64,
    /// A move waits for the queued spell: the rotation runs again as soon as the spell fires.
    pub(crate) move_held_for_queue: bool,
}

/// A look's proof that no item is ready: it holds while the clock is before `until` and nothing
/// any item read has changed, or only energy has risen and stays below `energy_limit`.
#[derive(Clone, Copy, Debug)]
struct Look {
    until: i64,
    versions: super::Versions,
    execute_phase: i32,
    waiting_for_mana: f64,
    energy_limit: f64,
}

/// A priority item's cached "not ready": it holds while the clock is before `until` and none
/// of the kinds of state in `reads` has changed. Only an answer whose asking left no trace is
/// kept, and only used where asking would again leave none.
#[derive(Clone, Copy, Debug)]
struct Proof {
    until: i64,
    reads: u8,
    versions: super::Versions,
    execute_phase: i32,
    /// A false condition: Go stops asking before the action, so the proof holds even where
    /// asking the action would keep bookkeeping.
    condition_only: bool,
    /// The energy below which the answer holds as energy rises (see [`Look`]).
    energy_limit: f64,
}

impl Proof {
    const NONE: Proof = Proof {
        until: i64::MIN,
        reads: 0,
        versions: super::Versions::ZERO,
        execute_phase: 0,
        condition_only: false,
        energy_limit: 0.0,
    };
}

impl<A: Agent> Fight<A> {
    /// Go `GetAuraByID` on a unit, as the rotation reads it. The rotation compiles before the
    /// targets past the first get their auras, which sit at the same positions as the first's.
    fn rotation_aura(&self, side: Side, id: &ActionId) -> Option<FoundAura<AuraRef>> {
        let source = if side.is_target() { Side::Target } else { side };
        let tracker = &self.trackers[source.index()];
        let found = tracker.find_by_id(id).map(|index| FoundAura {
            aura: AuraRef { side, index },
            max_stacks: tracker.auras[index].max_stacks,
        });
        if found.is_none() {
            self.no_aura_found(side, id);
        }
        found
    }

    /// Go `GetAPLDot`: the spell's area or self-only dot, the caster's `AOEDot`, which every
    /// unit shares, or else `Spell.Dot` of the unit: the spell's own dot or its related dot
    /// spell's, which sits on the targets and not on the player.
    fn rotation_dot(&self, id: &ActionId, unit: Unit) -> Option<SpellId> {
        let spell = self.apl_spell(id)?;
        let holder = match self.spells[spell].dot {
            Some(_) => spell,
            None => self.spells[spell]
                .related_dot_spell
                .filter(|&related| self.spells.get(related).is_some_and(|s| s.dot.is_some()))?,
        };
        let dot = self.spells[holder].dot?;
        (!self.dots[dot].side.is_target() || matches!(unit, Unit::Target(_))).then_some(holder)
    }

    /// Whether a spell is harmful and keeps its own dot on the targets: one the game refuses to
    /// cast at the player, and the gate's `harmful_target_dot`.
    fn harmful_target_dot(&self, spell: SpellId) -> bool {
        let spell = &self.spells[spell];
        !spell.flags.helpful && spell.dot.is_some_and(|dot| self.dots[dot].side.is_target())
    }

    /// Go `GetIcdAuraByID` on a unit, as the rotation reads it.
    fn rotation_icd_aura(&self, unit: Unit, id: &ActionId) -> Option<AuraRef> {
        let side = match unit {
            Unit::Player => Side::Player,
            Unit::Target(position) => Side::target(position),
        };
        let source = if side.is_target() { Side::Target } else { side };
        let found = self.trackers[source.index()]
            .find_icd_by_id(id)
            .map(|index| AuraRef { side, index });
        if found.is_none() {
            self.no_aura_found(side, id);
        }
        found
    }

    /// Go `GetAPLMultidotSpell` and `Spell.CurDot`: the spell whose dot a multidot action or
    /// value reads, its own on a target or its related dot spell's. An area or self-only dot
    /// is the caster's `AOEDot`, which `CurDot` does not return.
    fn multidot_dot(&self, id: &ActionId) -> Option<SpellId> {
        let spell = self.apl_spell(id)?;
        let dot = match self.spells[spell]
            .dot
            .filter(|&dot| self.dots[dot].side.is_target())
        {
            Some(_) => Some(spell),
            None => self.spells[spell]
                .related_dot_spell
                .filter(|&related| self.spells.get(related).is_some_and(|s| s.dot.is_some())),
        };
        if dot.is_none() {
            self.check(Level::Warning, || {
                format!(
                    "Spell {} does not have an associated DoT",
                    action_string(id)
                )
            });
        }
        dot
    }

    /// Go `Spell.RelatedSelfBuff` of a spell the rotation reads it of: the player's aura of the
    /// label the input names for the spell.
    fn self_buff(&self, spell: SpellId) -> Option<AuraRef> {
        let reads = self.rotation_reads.as_ref()?;
        let label = &reads
            .self_buffs
            .iter()
            .find(|buff| buff.spell == self.spells[spell].id)?
            .aura;
        self.trackers[Side::Player.index()]
            .auras
            .iter()
            .position(|aura| &aura.label == label)
            .map(|index| AuraRef {
                side: Side::Player,
                index,
            })
    }

    /// The spells the input names for a `castAllStatBuffCooldowns` of the stats.
    pub(super) fn cast_all_spells(&self, stats: &[i32]) -> Vec<SpellId> {
        let Some(reads) = self.rotation_reads.as_ref() else {
            return Vec::new();
        };
        let Some(entry) = reads
            .cast_cooldowns
            .iter()
            .find(|entry| entry.stats == stats)
        else {
            return Vec::new();
        };
        entry
            .spells
            .iter()
            .filter_map(|id| {
                self.spells
                    .iter()
                    .position(|spell| spell.caster == Side::Player && &spell.id == id)
            })
            .collect()
    }

    /// The spell holding the dot of a spell whose Go `ExpectedTickDamage` the runtime computes:
    /// Rake and Rip by their class, the Priest's dots, Agony and Flame Shock here.
    fn expected_tick_spell(&self, id: &ActionId) -> Option<SpellId> {
        let spell = self.apl_spell(id)?;
        let holder = match self.spells[spell].dot {
            Some(_) => spell,
            None => self.spells[spell].related_dot_spell?,
        };
        self.spells[holder].dot?;
        // Each class sets the tick base these dots read once the rotation is compiled.
        let known = [
            "rake",
            "rip",
            "shadow_word_pain",
            "devouring_plague",
            "mind_flay",
            "starshards",
            "bane_of_agony",
            "flame_shock_dot",
        ];
        self.spells[holder]
            .class_spell
            .as_deref()
            .is_some_and(|name| known.contains(&name))
            .then_some(holder)
    }

    /// Go `Spell.ExpectedTickDamage` (`snapshot` false) and
    /// `ExpectedTickDamageFromCurrentSnapshot` of the spell holding the dot, on a target.
    fn expected_tick_damage(&mut self, spell: SpellId, target: Side, snapshot: bool) -> f64 {
        if let Some(damage) = A::expected_tick_damage(self, spell, target, snapshot) {
            return damage;
        }
        let dot = self.dot_on(self.spells[spell].dot.expect("a dot spell"), target);
        let base = self.dots[dot]
            .tick_base
            .expect("an expected tick has its base");
        match self.spells[spell].class_spell.as_deref() {
            // shocks.go: the tick its rank rolls, or the average crit, per second of its period.
            Some("flame_shock_dot") => {
                if snapshot {
                    let outcome = super::damage::Outcome::tick_hit_rolled(
                        self.dots[dot].tick_can_crit,
                        self.spells[spell].magic_defense,
                    )
                    .unwrap_or(super::damage::Outcome::Tick);
                    let damage = self
                        .snapshot_dot_tick_calc_with(dot, target, outcome)
                        .damage;
                    damage / crate::core::time::seconds(self.dots[dot].tick_period)
                } else {
                    let damage = self.expected_periodic_damage(
                        spell,
                        dot,
                        target,
                        base,
                        super::damage::Outcome::ExpectedMagicCrit,
                    );
                    damage / crate::core::time::seconds(self.calc_tick_period(dot))
                }
            }
            class => {
                // agony.go reads every tick as a hit; the Priest's dots scale by the hit chance.
                let outcome = if class == Some("bane_of_agony") {
                    super::damage::Outcome::ExpectedMagicAlwaysHit
                } else {
                    super::damage::Outcome::ExpectedMagicHit
                };
                if snapshot {
                    self.snapshot_dot_tick_calc_with(dot, target, outcome)
                        .damage
                } else {
                    self.expected_periodic_damage(spell, dot, target, base, outcome)
                }
            }
        }
    }

    /// Go `APLActionChangeTarget.Execute`'s log line, of the player's current target.
    pub(crate) fn log_change_target(&mut self) {
        if self.log.is_some() {
            let line = format!(
                "Changing target to {}",
                self.label_of(self.current_target_side())
            );
            self.player_log(&line);
        }
    }

    /// The player's aura of a label.
    fn player_aura_by_label(&self, label: &str) -> Option<AuraRef> {
        self.trackers[Side::Player.index()]
            .auras
            .iter()
            .position(|aura| aura.label == label)
            .map(|index| AuraRef {
                side: Side::Player,
                index,
            })
    }

    /// Go `GetMatchingItemProcAuras` over the input's item proc buffs: those moving one of the
    /// stats and, with a minimum, whose ICD is longer.
    fn item_procs(&self, procs: &crate::rotation::StatProcs) -> Vec<(AuraRef, bool)> {
        let Some(reads) = self.rotation_reads.as_ref() else {
            return Vec::new();
        };
        reads
            .item_proc_buffs
            .iter()
            .filter(|buff| buff.stats.iter().any(|stat| procs.stats.contains(stat)))
            .filter_map(|buff| {
                let aura = self.player_aura_by_label(&buff.aura)?;
                let icd = self.aura(aura).icd;
                let long_enough =
                    procs.min_icd <= 0 || icd.is_some_and(|(_, duration)| duration > procs.min_icd);
                long_enough.then_some((aura, buff.swapped))
            })
            .collect()
    }

    /// Go `APLValueMultipleCdUsages.GetBool`.
    fn multiple_cd_usages(
        &mut self,
        target: SpellId,
        target_buff: AuraRef,
        base: Option<(SpellId, AuraRef)>,
        offset: &Compiled,
        align_cd_end: bool,
    ) -> bool {
        let total = self.duration;
        let remaining = self.duration - self.now;
        let cast_offset = self.get_duration(offset);
        let cd = self.spells[target].cd.map_or(0, |(_, cd)| cd);
        let duration = self.aura(target_buff).duration;
        let effective = cd + duration;
        // Go `1 + int(float64(totalTime)/float64(targetSpellCD))`; the gate requires a cooldown.
        let max_uses = 1 + (total as f64 / cd as f64) as i64;
        let full_at_end = remaining <= duration;
        let second_within = remaining >= effective;
        let Some((base, base_buff)) = base else {
            return if align_cd_end {
                full_at_end || second_within
            } else {
                true
            };
        };
        let base_ready = self.spells[base]
            .cd
            .is_none_or(|(timer, _)| self.timers[timer] <= self.now);
        let base_state = self.aura(base_buff);
        let (base_active, base_duration) = (base_state.active, base_state.duration);
        let active_and_should_cast = base_active && self.now >= cast_offset;
        let will_lose_uses = base_ready && (max_uses - 1) * cd + duration + cast_offset > total;
        let cast_and_can_again = !base_ready && second_within;
        let active_at_end = cast_offset + base_duration >= total;
        let exceeds = base_duration >= remaining - duration;
        if align_cd_end {
            if max_uses == 1 {
                // Go's `active at end and not exceeding, or not active at end`.
                return full_at_end || (active_and_should_cast && !(active_at_end && exceeds));
            }
            return full_at_end || active_and_should_cast || will_lose_uses || cast_and_can_again;
        }
        active_and_should_cast
            || cast_and_can_again
            || will_lose_uses
            || (!base_ready && self.spell_ready(target))
    }

    /// The player's resource bars, which values of a bar the player lacks give none for.
    fn rotation_bars(&self) -> crate::rotation::Bars {
        crate::rotation::Bars {
            mana: self.has_mana_bar(),
            max_rage: self.rage.as_ref().map(|bar| bar.max),
            max_combo_points: self.energy.as_ref().map(|bar| bar.max_combo_points),
        }
    }

    /// Go `GetAPLSpell`: the first APL-flagged spell with the action ID, otherwise the first
    /// registered one. The potion action names the first combat potion.
    pub(crate) fn apl_spell(&self, id: &ActionId) -> Option<SpellId> {
        let spell = self.find_apl_spell(id);
        if spell.is_none() {
            self.check(Level::Warning, || {
                format!(
                    "{} does not know spell {}",
                    self.config.player_label,
                    action_string(id)
                )
            });
        }
        spell
    }

    /// [`Self::apl_spell`] without its validation message.
    fn find_apl_spell(&self, id: &ActionId) -> Option<SpellId> {
        if id.other_id == "OtherActionPotion" {
            return self.spells.iter().position(|s| s.flags.combat_potion);
        }
        let apl = self.spells.iter().position(|s| &s.id == id && s.flags.apl);
        apl.or_else(|| self.spells.iter().position(|s| &s.id == id))
            // Go `RankAliases`: an older rank a rotation names presses the known one.
            .or_else(|| {
                self.spells.iter().position(|s| {
                    s.flags.apl
                        && s.rank_aliases.iter().any(|alias| {
                            alias.spell_id == id.spell_id
                                && alias.item_id == id.item_id
                                && alias.other_id == id.other_id
                        })
                })
            })
    }

    /// Go `GetAPLCastSpell`: a registered spell without the APL flag defers to an APL-flagged
    /// spell with the same action ignoring the tag, then to `GetAPLSpell`.
    fn apl_cast_spell(&self, id: &ActionId) -> Option<SpellId> {
        if let Some(spell) = self.spells.iter().find(|s| &s.id == id) {
            if !spell.flags.apl {
                let same = |s: &super::Spell<A::Spell>| {
                    s.id.spell_id == id.spell_id
                        && s.id.item_id == id.item_id
                        && s.id.other_id == id.other_id
                };
                if let Some(apl) = self.spells.iter().position(|s| same(s) && s.flags.apl) {
                    return Some(apl);
                }
            }
        }
        self.apl_spell(id)
    }

    /// Go's prepull registration: each castable action at its time, in a stable time order, and
    /// the items holding the interrupt conditions of prepull channels, numbered from `base`.
    pub(crate) fn compile_prepull(
        &self,
        rotation: &Rotation,
        base: usize,
    ) -> (Vec<(i64, PrepullAct)>, Vec<Item>, Vec<PrepullList>) {
        self.with_lookup(|lookup| self.compile_prepull_with(rotation, base, lookup))
    }

    fn compile_prepull_with(
        &self,
        rotation: &Rotation,
        base: usize,
        lookup: &Lookup<AuraRef>,
    ) -> (Vec<(i64, PrepullAct)>, Vec<Item>, Vec<PrepullList>) {
        let mut channels = Vec::new();
        let mut lists = Vec::new();
        let mut prepull: Vec<(i64, PrepullAct)> = rotation
            .prepull
            .iter()
            // Go newAPLAction: a constant false condition drops the action; any other
            // condition is never evaluated before the prepull cast.
            .filter(|prepull| {
                compile_condition(prepull.condition.as_ref(), lookup) != CompiledCondition::Pruned
            })
            .filter_map(|prepull| {
                self.prepull_act(prepull, lookup, base, &mut channels, &mut lists)
            })
            .collect();
        // apl.go: with a swap the prepull leaves on another set, the swap back to Main a
        // nanosecond before the pull (Go's `RegisterPrepullAction(-1, ...)`), registered after
        // the rotation's.
        let last_swap = prepull.iter().rev().find_map(|(_, act)| match act {
            PrepullAct::ItemSwap(to_swapped) => Some(*to_swapped),
            _ => None,
        });
        if last_swap == Some(true) {
            prepull.push((-1, PrepullAct::SwapBackToMain));
        }
        prepull.sort_by_key(|(do_at, _)| *do_at);
        (prepull, channels, lists)
    }

    /// Go `newAPLAction` of a prepull action, without its condition: the action at its time,
    /// or `None` where Go builds none. A channel's interrupt item or a strict sequence joins
    /// `channels`, numbered from `base`, and a list of spells or auras joins `lists`.
    pub(super) fn prepull_act(
        &self,
        prepull: &crate::rotation::Prepull,
        lookup: &Lookup<AuraRef>,
        base: usize,
        channels: &mut Vec<Item>,
        lists: &mut Vec<PrepullList>,
    ) -> Option<(i64, PrepullAct)> {
        let find = |side: Side, id: &ActionId| self.rotation_aura(side, id);
        match &prepull.action {
            // Go newActionCastSpell: a target that names no unit drops the action. The runtime
            // casts a prepull action on the first target, which the gate checks.
            ParsedAction::CastSpell { spell: id, target } => {
                let spell = self.apl_cast_spell(id)?;
                target.resolve(self.targets.len())?;
                Some((prepull.do_at_ns, PrepullAct::Cast(spell)))
            }
            ParsedAction::CastAtPlayer(id) => self
                .apl_cast_spell(id)
                .map(|spell| (prepull.do_at_ns, PrepullAct::CastAtPlayer(spell))),
            // Go `newActionChannelSpell`: a cast without an interrupt condition, otherwise a
            // channel whose condition the rotation reads once it runs.
            ParsedAction::ChannelSpell {
                spell: id,
                target,
                interrupt_if,
                allow_recast,
            } => {
                // Go builds the interrupt condition, then looks the spell up, then the target.
                match crate::rotation::compile_bool_value(interrupt_if.as_ref(), lookup) {
                    None => {
                        let spell = self.apl_cast_spell(id)?;
                        target.resolve(self.targets.len())?;
                        Some((prepull.do_at_ns, PrepullAct::Cast(spell)))
                    }
                    Some(interrupt) => {
                        let spell = self
                            .apl_spell(id)
                            .filter(|&spell| self.spells[spell].flags.channeled)?;
                        target.resolve(self.targets.len())?;
                        channels.push(Item::new(
                            None,
                            Act::Channel {
                                spell,
                                interrupt: Rc::new(interrupt),
                                allow_recast: *allow_recast,
                            },
                        ));
                        let item = base + channels.len() - 1;
                        Some((prepull.do_at_ns, PrepullAct::Channel(spell, item)))
                    }
                }
            }
            // Go `newActionStrictSequence`, as the priority list builds it.
            ParsedAction::StrictSequence(_) => {
                let act = self.compile_action(&prepull.action, lookup, None, &mut Vec::new())?;
                channels.push(Item::new(None, act));
                let item = base + channels.len() - 1;
                Some((prepull.do_at_ns, PrepullAct::StrictSequence(item)))
            }
            ParsedAction::CastAllStatBuffCooldowns(stats) => {
                lists.push(PrepullList::Spells(self.cast_all_spells(stats)));
                Some((
                    prepull.do_at_ns,
                    PrepullAct::CastAllStatBuffCooldowns(lists.len() - 1),
                ))
            }
            // Go `newActionActivateAllStatBuffProcAuras`: the proc buffs of the stats in the
            // set; none without one, or for Swap1 without an item swap.
            ParsedAction::ActivateAllStatBuffProcAuras { stats, swap1 } => {
                if *swap1 && self.item_swap.is_none() {
                    self.check(Level::Warning, || {
                        "No swap set configured in Settings.".into()
                    });
                    return None;
                }
                let auras: Vec<AuraRef> = self
                    .item_procs(&crate::rotation::StatProcs {
                        stats: stats.clone(),
                        min_icd: 0,
                    })
                    .into_iter()
                    .filter(|(_, swapped)| swapped == swap1)
                    .map(|(aura, _)| aura)
                    .collect();
                if auras.is_empty() {
                    self.check(Level::Warning, || {
                        "There are no proc items buffing the specified stat type(s) for the specified item set.".into()
                    });
                    return None;
                }
                self.check(Level::Information, || {
                    let ids: Vec<String> = auras
                        .iter()
                        .filter_map(|&aura| self.aura(aura).action_id.as_ref().map(action_string))
                        .collect();
                    format!(
                        "Activate All Stat Buff Proc Auras ({}) for swap set: {} will activate the following Auras: {}",
                        super::rotation_checks::stat_names(stats),
                        if *swap1 { "Swap1" } else { "Main" },
                        ids.join(", ")
                    )
                });
                lists.push(PrepullList::Auras(auras));
                Some((
                    prepull.do_at_ns,
                    PrepullAct::ActivateAllStatBuffProcAuras(*swap1, lists.len() - 1),
                ))
            }
            // Go `newActionItemSwap`: no action without an item swap.
            ParsedAction::ItemSwap(to_swapped) => {
                if self.item_swap.is_none() && *to_swapped {
                    self.check(Level::Warning, || {
                        "No swap set configured in Settings.".into()
                    });
                }
                self.item_swap
                    .as_ref()
                    .map(|_| (prepull.do_at_ns, PrepullAct::ItemSwap(*to_swapped)))
            }
            // Go GetAPLAura on the player: an unknown aura drops the action.
            ParsedAction::ActivateAura(id) => find(Side::Player, id)
                .map(|found| (prepull.do_at_ns, PrepullAct::ActivateAura(found.aura))),
            ParsedAction::CancelAura(id) => find(Side::Player, id)
                .map(|found| (prepull.do_at_ns, PrepullAct::CancelAura(found.aura))),
            // Go `newActionActivateAuraWithStacks`: no action for an aura that does not
            // stack.
            ParsedAction::ActivateAuraWithStacks { id, stacks } => find(Side::Player, id)
                .filter(|found| {
                    if found.max_stacks == 0 {
                        self.check(Level::Warning, || {
                            format!("{} is not a stackable aura", action_string(id))
                        });
                    }
                    found.max_stacks != 0
                })
                .map(|found| {
                    let stacks = (*stacks).min(found.max_stacks);
                    (
                        prepull.do_at_ns,
                        PrepullAct::ActivateAuraWithStacks(found.aura, stacks),
                    )
                }),
            // Go `newActionChangeTarget`: no action for a unit Go finds nothing for; the gate
            // admits only the first target.
            ParsedAction::ChangeTarget(unit) => unit
                .resolve(self.targets.len())
                .map(|_| (prepull.do_at_ns, PrepullAct::ChangeTarget)),
            // Go `GetAPLICDAura` on the player.
            ParsedAction::TriggerIcd(id) => self
                .rotation_icd_aura(Unit::Player, id)
                .map(|aura| (prepull.do_at_ns, PrepullAct::TriggerIcd(aura))),
            // The parser reads a constant range or duration in a prepull action.
            ParsedAction::Move(Value::Const(range)) => {
                Some((prepull.do_at_ns, PrepullAct::Move(range.float)))
            }
            ParsedAction::PrepullGroupMove(range) => {
                let mut range = range;
                while let Value::Variable { resolved, .. } = range {
                    range = resolved;
                }
                let Value::Const(range) = range else {
                    return None;
                };
                Some((prepull.do_at_ns, PrepullAct::GroupMove(range.float)))
            }
            ParsedAction::MoveDuration(Value::Const(duration)) => Some((
                prepull.do_at_ns,
                PrepullAct::MoveDuration(duration.duration_ns),
            )),
            _ => None,
        }
    }

    /// The names a rotation uses, resolved as Go does on the casting player, for `f`.
    pub(super) fn with_lookup<T>(&self, f: impl FnOnce(&Lookup<AuraRef>) -> T) -> T {
        let aura = |id: &ActionId| self.rotation_aura(Side::Player, id);
        let target_aura =
            |position: usize, id: &ActionId| self.rotation_aura(Side::target(position), id);
        let spell = |id: &ActionId| self.apl_spell(id);
        let dot = |id: &ActionId, unit: Unit| self.rotation_dot(id, unit);
        let dot_base_duration = |id: &ActionId| {
            self.dot_base_durations
                .iter()
                .find(|(spell, _)| spell == id)
                .map(|(_, duration)| *duration)
        };
        let pet_aura_known = |pet: usize, id: &ActionId| self.pet_aura_known(pet, id);
        // A pet the fight simulates, by its position among the player's pets.
        let pet_aura = |pet: usize, id: &ActionId| {
            let side = self.pet_side(self.pet_agent_labels.get(pet)?)?;
            self.rotation_aura(side, id)
        };
        let icd_aura = |unit: Unit, id: &ActionId| self.rotation_icd_aura(unit, id);
        let lookup = Lookup {
            aura: &aura,
            target_aura: &target_aura,
            targets: self.targets.len(),
            spell: &spell,
            dot: &dot,
            dot_base_duration: &dot_base_duration,
            pet_aura_known: &pet_aura_known,
            pet_aura: &pet_aura,
            simulated_pet_with_mana: &|pet: usize| {
                let side = self.pet_side(self.pet_agent_labels.get(pet)?)?;
                let Side::Pet(slot) = side else { return None };
                (self.unit(side).powers.max_mana > 0.0).then_some(slot)
            },
            unsummoned_pet_with_mana: &|pet: usize| {
                self.pet_agent_labels
                    .get(pet)
                    .is_some_and(|label| self.pet_side(label).is_none())
                    && self.pet_agent_mana_bars.get(pet).copied().unwrap_or(false)
            },
            unsummoned_pet_max_mana: &|pet: usize| {
                let label = self.pet_agent_labels.get(pet)?;
                if self.pet_side(label).is_some() {
                    return None;
                }
                self.pet_agent_max_mana.get(pet).copied().flatten()
            },
            item_swap: self.item_swap.is_some(),
            bars: self.rotation_bars(),
            icd_aura: &icd_aura,
            multidot: &|id: &ActionId| self.multidot_dot(id).is_some(),
            assigned_curse: self
                .assigned_curse
                .as_ref()
                .map(|(name, id)| (name.as_str(), id)),
            tanked: self.tanked(),
            reads: self.rotation_reads.as_ref(),
            exact_spell: &|id: &ActionId| self.exact_spell(id),
            self_buff: &|spell: usize| self.self_buff(spell),
            item_procs: &|procs: &crate::rotation::StatProcs| self.item_procs(procs),
            aura_by_label: &|label: &str| self.player_aura_by_label(label),
            expected_tick: &|id: &ActionId| self.expected_tick_spell(id),
        };
        f(&lookup)
    }

    /// Go `newAPLRotation` for the supported subset: the items of the priority list, then the
    /// items of each instance of a group, and where each instance's items are.
    pub(crate) fn compile_rotation(&self, rotation: &Rotation) -> Result<Compilation, String> {
        self.with_lookup(|lookup| self.compile_rotation_with(rotation, lookup))
    }

    fn compile_rotation_with(
        &self,
        rotation: &Rotation,
        lookup: &Lookup<AuraRef>,
    ) -> Result<Compilation, String> {
        // A reference whose condition is a constant false is pruned, and takes no part in
        // binding the groups.
        let live = |index: usize| {
            compile_condition(rotation.priority_list[index].condition.as_ref(), lookup)
                != CompiledCondition::Pruned
        };
        let (rotation, binding) = bind(rotation, &live)?;
        let mut items = Vec::new();
        // Schedules' inner actions, which become items after the groups' items.
        let mut inner = Vec::new();
        for (index, item) in rotation.priority_list.iter().enumerate() {
            let Some(action) = self.compile_action(
                &item.action,
                lookup,
                binding.instance(Reference::Top(index)),
                &mut inner,
            ) else {
                continue;
            };
            let condition = match compile_condition(item.condition.as_ref(), lookup) {
                // A constant false condition prunes the action; its spells already left
                // the major cooldowns in Go's export.
                CompiledCondition::Pruned => continue,
                CompiledCondition::Always => None,
                CompiledCondition::When(condition) => Some(Rc::new(Cond::lower(&condition))),
            };
            items.push(Item::new(condition, action));
        }
        let priority = items.len();
        let mut groups = Vec::new();
        for (instance, group) in binding.instances.iter().enumerate() {
            let start = items.len();
            for (index, item) in group.items.iter().enumerate() {
                let reference = Reference::Nested {
                    instance,
                    item: index,
                };
                let Some(action) = self.compile_action(
                    &item.action,
                    lookup,
                    binding.instance(reference),
                    &mut inner,
                ) else {
                    continue;
                };
                // Go `newAPLActionWithGroupVars` builds the condition and keeps a constant
                // whatever it holds.
                let condition = compile_bool_value(item.condition.as_ref(), lookup)
                    .map(|condition| Rc::new(Cond::lower(&condition)));
                items.push(Item::new(condition, action));
            }
            groups.push(start..items.len());
        }
        // A schedule's inner action is an item of its own after the rest, which no walk of the
        // priority list or a group reaches.
        let offset = items.len();
        items.extend(inner);
        for item in &mut items {
            if let Act::Schedule { inner, .. } = &mut item.action {
                *inner += offset;
            }
        }
        // Go's finalizing walks each action's inner actions before the action itself.
        fn visit(items: &[Item], item: usize, out: &mut Vec<(Rc<str>, usize)>) {
            match &items[item].action {
                Act::Schedule { inner, .. } => visit(items, *inner, out),
                Act::Sequence { name, .. } => out.push((Rc::clone(name), item)),
                _ => {}
            }
        }
        let mut sequences = Vec::new();
        for item in 0..priority {
            visit(&items, item, &mut sequences);
        }
        // Go `APLActionResetSequence.Finalize`: the first sequence of the name.
        for item in &mut items {
            if let Act::ResetSequence { name, sequence } = &mut item.action {
                *sequence = sequences
                    .iter()
                    .find(|(found, _)| found == name)
                    .map(|(_, index)| *index);
            }
        }
        Ok(Compilation {
            items,
            priority,
            groups,
            sequences,
        })
    }

    /// Go's multidot actions: the encounter's targets cap the dot count.
    fn max_dots(&self, max_dots: i32) -> i32 {
        let targets = self.targets.len() as i32;
        if targets < max_dots {
            self.check(Level::Warning, || {
                format!("Encounter only has {targets} targets. Using that for Max Dots instead of {max_dots}")
            });
        }
        max_dots.min(targets)
    }

    /// Go `newActionCastSpell` of a sequence's step: the spell the step casts and the unit it is
    /// cast on, none for an unknown spell or a target that names no unit.
    fn step_spell(&self, step: &Step) -> Option<(SpellId, Side)> {
        let spell = self.apl_cast_spell(&step.spell)?;
        let side = match step.target.resolve(self.targets.len())? {
            Unit::Player => Side::Player,
            Unit::Target(position) => Side::target(position),
        };
        Some((spell, side))
    }

    /// Go's action constructors: the action a parsed one builds, or `None` where Go builds none.
    /// A reference runs the instance it is bound to.
    pub(super) fn compile_action(
        &self,
        action: &ParsedAction,
        lookup: &Lookup<AuraRef>,
        instance: Option<usize>,
        inner: &mut Vec<Item>,
    ) -> Option<Act> {
        // Go `GetAPLMultidotSpell`: a multidot line for a spell without a dot `CurDot`
        // returns is dropped.
        let multidot_dot = |id: &ActionId| self.multidot_dot(id);
        Some(match action {
            // Go newActionCastSpell: an unknown spell, or a target that is no unit, drops the
            // action.
            ParsedAction::CastSpell { spell: id, target } => {
                let unit = target.resolve(self.targets.len());
                match (self.apl_cast_spell(id), unit) {
                    // The game refuses a harmful spell on a friendly unit, so a line casting
                    // one whose dot sits on its target at the player never casts. Go builds
                    // the action and panics on the player's missing dot (Rupture, Subtlety
                    // Rogue sim 2990f120, 25 October 2026); Rust follows the game.
                    (Some(spell), Some(Unit::Player)) if self.harmful_target_dot(spell) => {
                        return None
                    }
                    (Some(spell), Some(Unit::Player)) => Act::Cast(spell, Side::Player),
                    (Some(spell), Some(Unit::Target(position))) => {
                        Act::Cast(spell, Side::target(position))
                    }
                    _ => return None,
                }
            }
            // Go newActionCastFriendlySpell at the player: the same cast on the player.
            ParsedAction::CastAtPlayer(id) => Act::Cast(self.apl_cast_spell(id)?, Side::Player),
            ParsedAction::AutocastOtherCooldowns => Act::Autocast,
            // Go newActionStrictSequence runs every step or none: a step the character
            // lacks drops the whole action (ElliotWood/Forever#625).
            ParsedAction::StrictSequence(steps) => {
                // Go builds every step before it looks for a missing one.
                let casts: Vec<Option<StepCast>> = steps
                    .iter()
                    .map(|step| {
                        if step.change_target {
                            // Go `newActionChangeTarget`: none for a unit it finds nothing for.
                            match step.target.resolve(self.targets.len())? {
                                Unit::Target(position) => {
                                    Some((NO_SPELL, Side::Player, Some((position, NEVER_EXPIRES))))
                                }
                                Unit::Player => None,
                            }
                        } else {
                            self.step_spell(step)
                                .map(|(spell, side)| (spell, side, None))
                        }
                    })
                    .collect();
                let casts = casts.into_iter().collect::<Option<Vec<_>>>()?;
                if casts.is_empty() {
                    return None;
                }
                let mut spells = Vec::with_capacity(casts.len());
                let mut targets = Vec::with_capacity(casts.len());
                let mut changes = Vec::with_capacity(casts.len());
                for (spell, side, change) in casts {
                    spells.push(spell);
                    targets.push(side);
                    changes.push(change);
                }
                // A step whose condition is a constant false is pruned, so Go drops the action.
                let mut conditions = Vec::with_capacity(steps.len());
                for step in steps {
                    conditions.push(match compile_condition(step.condition.as_ref(), lookup) {
                        CompiledCondition::Pruned => return None,
                        CompiledCondition::Always => None,
                        CompiledCondition::When(condition) => {
                            Some(Rc::new(Cond::lower(&condition)))
                        }
                    });
                }
                Act::StrictSequence {
                    spells,
                    targets,
                    changes,
                    conditions,
                    next: 0,
                }
            }
            // Go newActionSequence drops the steps it builds no action for (an unknown cast, a
            // condition that is a constant false), and the action when none remain.
            ParsedAction::Sequence { name, steps } => {
                let mut spells = Vec::with_capacity(steps.len());
                let mut targets = Vec::with_capacity(steps.len());
                let mut conditions = Vec::with_capacity(steps.len());
                for step in steps {
                    let Some((spell, target)) = self.step_spell(step) else {
                        continue;
                    };
                    let condition = match compile_condition(step.condition.as_ref(), lookup) {
                        CompiledCondition::Pruned => continue,
                        CompiledCondition::Always => None,
                        CompiledCondition::When(condition) => {
                            Some(Rc::new(Cond::lower(&condition)))
                        }
                    };
                    spells.push(spell);
                    targets.push(target);
                    conditions.push(condition);
                }
                if spells.is_empty() {
                    return None;
                }
                Act::Sequence {
                    name: Rc::from(name.as_str()),
                    spells,
                    targets,
                    conditions,
                    next: 0,
                }
            }
            // Go `newActionWait` and `newActionWaitUntil`: no action without a value.
            ParsedAction::Wait(duration) => Act::Wait {
                duration: Rc::new(compile_duration_value(duration.as_ref(), lookup)?),
                until: 0,
            },
            ParsedAction::WaitUntil(condition) => {
                Act::WaitUntil(Rc::new(compile_bool_value(condition.as_ref(), lookup)?))
            }
            // Go `newActionSchedule`: the inner action as `newAPLAction` builds it, then the
            // times; no action without either.
            ParsedAction::Schedule {
                timings,
                invalid_timings,
                inner: item,
            } => {
                let condition = match compile_condition(item.condition.as_ref(), lookup) {
                    CompiledCondition::Pruned => return None,
                    CompiledCondition::Always => None,
                    CompiledCondition::When(condition) => Some(Rc::new(Cond::lower(&condition))),
                };
                let action = self.compile_action(&item.action, lookup, None, inner)?;
                for timing in invalid_timings {
                    self.check(Level::Warning, || {
                        format!("Invalid duration value '{timing}'")
                    });
                }
                let timings: Rc<[i64]> = Rc::from(timings.as_ref()?.as_slice());
                inner.push(Item::new(condition, action));
                Act::Schedule {
                    timings,
                    next: 0,
                    inner: inner.len() - 1,
                }
            }
            // Go `newActionStrictMultidot`: as a multidot, its overlap 0ms without a value.
            ParsedAction::StrictMultidot {
                spell,
                max_dots,
                max_overlap,
            } => {
                let dot_spell = multidot_dot(spell)?;
                let cast = self.apl_cast_spell(spell)?;
                let overlap = Rc::new(
                    compile_duration_value(max_overlap.as_ref(), lookup).unwrap_or_else(|| {
                        Compiled::Const(crate::rotation::parse_const("0ms").expect("0ms"))
                    }),
                );
                Act::StrictMultidot {
                    spell: cast,
                    dot_spell,
                    max_dots: self.max_dots(*max_dots),
                    overlap,
                    ready: Vec::new(),
                    next: 0,
                }
            }
            // Go `newActionWarlockAssignedCurseAction`: only a Warlock builds it, and none for a
            // target that names no unit.
            ParsedAction::CastAssignedCurse(target) => {
                let (_, curse) = lookup.assigned_curse?;
                let side = match target.resolve(self.targets.len())? {
                    Unit::Target(position) => Side::target(position),
                    Unit::Player => Side::Target,
                };
                Act::CastAssignedCurse(self.apl_spell(curse)?, side)
            }
            // Go `processMajorCooldowns`: the spells the input names for its stats; none leaves
            // an action that is never ready.
            ParsedAction::CastAllStatBuffCooldowns(stats) => Act::CastAllStatBuffCooldowns {
                spells: self.cast_all_spells(stats).into(),
                ready: Vec::new(),
            },
            ParsedAction::ActivateAllStatBuffProcAuras { .. } => return None,
            // Go `newActionChangeTarget`: no action for a unit Go finds nothing for. A relative
            // unit resolves against the current target this copy of the rotation is for; the
            // gate refuses a change to the player.
            ParsedAction::ChangeTarget(unit) => match unit.resolve(self.targets.len())? {
                Unit::Target(to) => Act::ChangeTarget {
                    last: NEVER_EXPIRES,
                    to,
                },
                Unit::Player => return None,
            },
            // Go `newActionCancelAura`: no action for an aura the player lacks.
            ParsedAction::CancelAura(id) => {
                Act::CancelAura(self.rotation_aura(Side::Player, id)?.aura)
            }
            // Parsed only among the prepull actions.
            ParsedAction::ActivateAuraWithStacks { .. } | ParsedAction::TriggerIcd(_) => {
                return None
            }
            // Go `newActionResetSequence`: no action without a name.
            ParsedAction::ResetSequence(name) => {
                if name.is_empty() {
                    self.check(Level::Warning, || {
                        "Reset Sequence must provide a sequence name".into()
                    });
                    return None;
                }
                Act::ResetSequence {
                    name: Rc::from(name.as_str()),
                    sequence: None,
                }
            }
            // Go newActionChannelSpell: without an interrupt condition it is a cast;
            // otherwise the spell must be a channel. A target that names no unit drops the
            // action.
            ParsedAction::ChannelSpell {
                spell,
                target,
                interrupt_if,
                allow_recast,
            } => match compile_bool_value(interrupt_if.as_ref(), lookup) {
                None => {
                    let spell = self.apl_cast_spell(spell)?;
                    match target.resolve(self.targets.len())? {
                        Unit::Target(position) => Act::Cast(spell, Side::target(position)),
                        Unit::Player => Act::Cast(spell, Side::Player),
                    }
                }
                Some(interrupt) => {
                    let spell = self
                        .apl_spell(spell)
                        .filter(|&spell| self.spells[spell].flags.channeled)?;
                    target.resolve(self.targets.len())?;
                    Act::Channel {
                        spell,
                        interrupt: Rc::new(interrupt),
                        allow_recast: *allow_recast,
                    }
                }
            },
            // Go GetAPLMultidotSpell: an unknown spell or one without a dot drops the
            // action; the encounter's one target caps the dot count.
            ParsedAction::Multidot {
                spell,
                max_dots,
                max_overlap,
            } => {
                let dot_spell = multidot_dot(spell)?;
                let spell = self.apl_spell(spell)?;
                let overlap = compile_duration_value(max_overlap.as_ref(), lookup).map(Rc::new);
                Act::Multidot {
                    spell,
                    dot_spell,
                    max_dots: self.max_dots(*max_dots),
                    overlap,
                }
            }
            // Go `newActionMove` and `newActionMoveDuration` always build the action. A value
            // that gives none panics where Go reads it, which the gate refuses.
            ParsedAction::Move(range) => Act::Move(Rc::new(compile_raw_value(range, lookup)?)),
            // Parsed only as a prepull action, which `prepull_act` builds.
            ParsedAction::PrepullGroupMove(_) => return None,
            ParsedAction::MoveDuration(duration) => {
                Act::MoveDuration(Rc::new(compile_raw_value(duration, lookup)?))
            }
            // Go `newActionGroupReference`: no action without a group name; a reference that
            // binds no group is never ready.
            ParsedAction::GroupReference { name, .. } => {
                if name.is_empty() {
                    return None;
                }
                Act::Group { instance }
            }
            // Go `newActionItemSwap`: no action without an item swap.
            ParsedAction::ItemSwap(to_swapped) => {
                if self.item_swap.is_none() && *to_swapped {
                    self.check(Level::Warning, || {
                        "No swap set configured in Settings.".into()
                    });
                }
                self.item_swap.as_ref()?;
                Act::ItemSwap(*to_swapped)
            }
            // Parsed only among the prepull actions.
            ParsedAction::ActivateAura(_) => return None,
        })
    }

    fn get_bool(&mut self, value: &Compiled) -> bool {
        match value {
            Compiled::Const(constant) => constant.boolean,
            Compiled::GroupUsed(used) => *used,
            Compiled::AuraIsActive(aura) => self.aura(*aura).active,
            // Go `APLValueAuraIsActive` with `includeReactionTime`: `Aura.TimeActive` is zero
            // while inactive and the time since the aura started otherwise.
            Compiled::AuraIsActiveAfterReaction(aura) => {
                let state = self.aura(*aura);
                state.active && self.now - state.start >= self.config.reaction
            }
            // Go `APLValueAuraIsInactive` with `includeReactionTime`: `Aura.TimeInactive` is
            // never-expiring for an aura that has not faded, the time since it faded otherwise.
            Compiled::AuraIsInactiveAfterReaction(aura) => {
                let state = self.aura(*aura);
                let inactive_for = if state.fade_time < 0 {
                    NEVER_EXPIRES
                } else {
                    self.now - state.fade_time
                };
                !state.active && inactive_for >= self.config.reaction
            }
            // Go `APLValueFrontOfTarget`.
            Compiled::FrontOfTarget => self.config.melee.in_front_of_target,
            // Go `ShouldRefreshExclusiveEffects`: any of the aura's effects asking for a refresh.
            Compiled::AuraShouldRefresh { aura, overlap } => {
                let window = self.get_duration(overlap);
                // The exporter reads the first target's aura, which its copies share.
                let first = AuraRef {
                    side: if aura.side.is_target() {
                        Side::Target
                    } else {
                        aura.side
                    },
                    index: aura.index,
                };
                let (_, readings) = self
                    .aura_refresh
                    .iter()
                    .find(|(refreshed, _)| *refreshed == first)
                    .expect("the gate requires a refresh reading");
                let state = self.aura(*aura);
                let remaining = state.remaining(self.now);
                readings
                    .iter()
                    .enumerate()
                    .any(|(position, reading)| match reading {
                        // Alone in its category: refreshed once inactive or within the overlap.
                        RefreshReading::Own => !state.active || remaining <= window,
                        // A permanent aura of another effect holds the category for good.
                        RefreshReading::Never => false,
                        RefreshReading::Live => {
                            self.exclusive_should_refresh(*aura, position, window)
                        }
                    })
            }
            Compiled::DotIsActive(dot) => self.dot_active(*dot),
            // Go `APLValueSpellIsReady`: ready, or ready within the spell queue window.
            Compiled::SpellIsReady(spell) => {
                self.spell_ready(*spell)
                    || self.spell_time_to_ready(*spell) <= MAX_SPELL_QUEUE_WINDOW
            }
            // Go `APLValueGCDIsReady`: ready, or ready within the spell queue window.
            Compiled::GcdIsReady => {
                self.gcd_ready() || self.gcd_time_to_ready() <= MAX_SPELL_QUEUE_WINDOW
            }
            // Go `APLValueActiveItemSwapSet`.
            Compiled::ActiveItemSwapSet(swapped) => self.item_swapped() == *swapped,
            // Go `APLValueIsExecutePhase`: the encounter's execute phase is at or below it.
            Compiled::IsExecutePhase(threshold) => self.execute_phase <= *threshold,
            // Go `APLValueSpellCanCast`: `CanCastOrQueue`, with its cost check's side effects.
            Compiled::SpellCanCast(spell) => self.can_cast_or_queue(*spell),
            // Go `APLValueSpellIsChanneling`: the player's channeled dot is the spell's.
            Compiled::SpellIsChanneling(spell) => self
                .player
                .channeled_dot
                .is_some_and(|dot| self.dots[dot].spell == *spell),
            // Go `Unit.SpellInFlight`.
            Compiled::SpellInFlight(spell) => self.spells[*spell].in_flight > 0,
            // Go `Unit.Moving` of the player.
            Compiled::PlayerIsMoving => self.player.moving,
            // Go `Timer.IsReady` of the aura's ICD, or the aura newer than the reaction time.
            Compiled::AuraIcdIsReady {
                aura,
                include_reaction_time,
            } => {
                let state = self.aura(*aura);
                let (timer, _) = state.icd.expect("an ICD aura");
                self.timers[timer] <= self.now
                    || (*include_reaction_time
                        && state.active
                        && self.now - state.start < self.config.reaction)
            }
            Compiled::Settled(value) => *value,
            // Go `APLValueAllItemStatProcsActive`: a buff that can proc and is down or short of
            // its stacks refuses.
            Compiled::AllItemStatProcsActive(buffs) => buffs.iter().all(|&(aura, swapped)| {
                let state = self.aura(aura);
                !((!state.active || state.stacks < state.max_stacks) && !swapped)
            }),
            Compiled::AnyItemStatProcsActive(buffs) => buffs.iter().any(|&(aura, _)| {
                let state = self.aura(aura);
                state.active && state.stacks == state.max_stacks
            }),
            // Go: one that can proc, is down, and has a ready ICD.
            Compiled::AnyItemStatProcsAvailable(buffs) => buffs.iter().any(|&(aura, swapped)| {
                let state = self.aura(aura);
                !state.active
                    && !swapped
                    && state
                        .icd
                        .is_some_and(|(timer, _)| self.timers[timer] <= self.now)
            }),
            Compiled::AnyStatBuffCooldownsActive(auras) => auras.iter().any(|&aura| {
                let state = self.aura(aura);
                state.active && state.stacks == state.max_stacks
            }),
            // Go: the player's hardcast is of the spell's action and still running.
            Compiled::PlayerSpellCasting(spell) => {
                let hardcast = self.player.hardcast;
                hardcast
                    .spell
                    .is_some_and(|casting| self.spells[casting].id == self.spells[*spell].id)
                    && hardcast.expires > self.now
            }
            Compiled::MultipleCdUsages {
                target,
                target_buff,
                base,
                offset,
                align_cd_end,
            } => {
                let offset = offset.clone();
                self.multiple_cd_usages(*target, *target_buff, *base, &offset, *align_cd_end)
            }
            // Go `APLValueSequenceIsComplete`: no sequence of the name is never complete.
            Compiled::SequenceIsComplete(name) => match self.named_sequence(name) {
                Some(item) => match self.rotation[item].action {
                    Act::Sequence {
                        ref spells, next, ..
                    } => next >= spells.len(),
                    _ => unreachable!("item is a sequence"),
                },
                None => false,
            },
            // Go `APLValueSequenceIsReady`: the sequence's own readiness.
            Compiled::SequenceIsReady(name) => self
                .named_sequence(name)
                .is_some_and(|item| self.sequence_step_ready(item)),
            // Go: every enabled target's dot is active, and every target is enabled.
            Compiled::DotIsActiveOnAllTargets(dots) => dots.iter().all(|&dot| self.dot_active(dot)),
            Compiled::And(values) => values.iter().all(|value| self.get_bool(value)),
            Compiled::Or(values) => values.iter().any(|value| self.get_bool(value)),
            Compiled::Not(value) => !self.get_bool(value),
            Compiled::Compare { op, lhs, rhs } => match lhs.value_type() {
                ValueType::Bool => match op {
                    CompareOp::Eq => self.get_bool(lhs) == self.get_bool(rhs),
                    CompareOp::Ne => self.get_bool(lhs) != self.get_bool(rhs),
                    _ => false,
                },
                ValueType::Int => compare(*op, self.get_int(lhs), self.get_int(rhs)),
                ValueType::Float => compare_float(*op, self.get_float(lhs), self.get_float(rhs)),
                ValueType::Duration => compare(*op, self.get_duration(lhs), self.get_duration(rhs)),
                // Go compares the strings byte by byte.
                ValueType::String => compare(*op, self.get_string(lhs), self.get_string(rhs)),
            },
            Compiled::Coerced { inner, .. } => match inner.value_type() {
                ValueType::Bool => self.get_bool(inner),
                ValueType::Int => self.get_int(inner) != 0,
                ValueType::Float => self.get_float(inner) != 0.0,
                ValueType::Duration => self.get_duration(inner) != 0,
                ValueType::String => false,
            },
            _ => false,
        }
    }

    /// Go `GetString`: a constant's own text, or a value coerced to a string as Go formats it.
    fn get_string(&mut self, value: &Compiled) -> String {
        match value {
            Compiled::Const(constant) => constant.string.clone(),
            Compiled::Coerced { inner, .. } => match inner.value_type() {
                ValueType::Int => self.get_int(inner).to_string(),
                ValueType::Float => crate::rotation::go_float_string(self.get_float(inner)),
                ValueType::Duration => {
                    crate::rotation::go_duration_string(self.get_duration(inner))
                }
                ValueType::String => self.get_string(inner),
                // Go panics coercing a bool to a string; a comparison never asks.
                ValueType::Bool => String::new(),
            },
            _ => String::new(),
        }
    }

    fn get_int(&mut self, value: &Compiled) -> i32 {
        match value {
            Compiled::Const(constant) => constant.int,
            Compiled::AuraNumStacks(aura) => self.aura(*aura).stacks,
            // Go `APLValueAuraNumStacks` with `includeReactionTime`: the stacks of a reaction
            // time ago, which its stack change handler kept. The wrapping difference is Go's
            // for a handler that has not run since the reset.
            Compiled::AuraNumStacksAfterReaction(aura) => {
                let state = self.aura(*aura);
                if self.now.wrapping_sub(state.stack_update) >= self.config.reaction {
                    state.stacks
                } else {
                    state.previous_stacks
                }
            }
            Compiled::CurrentComboPoints => self.energy_bar().combo_points,
            // Go `APLValueSpellChanneledTicks`: `Dot.TickCount` of the player's channel, whatever
            // its spell.
            Compiled::SpellChanneledTicks(_) => self
                .player
                .channeled_dot
                .map_or(0, |dot| self.dots[dot].tick_count()),
            // Go `Spell.GetNumCharges`: no spell of the fork has charges.
            Compiled::SpellNumCharges(_) => 0,
            Compiled::MaxComboPoints(max) => *max,
            Compiled::SettledInt(value) => *value,
            // Go `APLValueMax.GetInt` and `APLValueMin.GetInt`.
            Compiled::Max(values) => {
                let first = self.get_int(&values[0]);
                values[1..]
                    .iter()
                    .fold(first, |result, value| result.max(self.get_int(value)))
            }
            Compiled::Min(values) => {
                let first = self.get_int(&values[0]);
                values[1..]
                    .iter()
                    .fold(first, |result, value| result.min(self.get_int(value)))
            }
            // Go `ActiveTargetCount`: every target is active throughout.
            Compiled::NumberTargets => self.targets.len() as i32,
            // Go `APLValueMath.GetInt`: int32 arithmetic, which wraps.
            Compiled::Math { op, lhs, rhs } => {
                let (lhs, rhs) = (self.get_int(lhs), self.get_int(rhs));
                match op {
                    MathOp::Add => lhs.wrapping_add(rhs),
                    MathOp::Sub => lhs.wrapping_sub(rhs),
                    MathOp::Mul => lhs.wrapping_mul(rhs),
                    MathOp::Div => {
                        assert!(rhs != 0, "integer division by zero");
                        lhs.wrapping_div(rhs)
                    }
                }
            }
            Compiled::Coerced { inner, .. } => match inner.value_type() {
                ValueType::Bool => i32::from(self.get_bool(inner)),
                ValueType::Int => self.get_int(inner),
                ValueType::Float => self.get_float(inner) as i32,
                ValueType::Duration => crate::core::time::seconds(self.get_duration(inner)) as i32,
                ValueType::String => 0,
            },
            _ => 0,
        }
    }

    fn get_float(&mut self, value: &Compiled) -> f64 {
        match value {
            Compiled::Const(constant) => constant.float,
            Compiled::CurrentManaPercent => self.player.mana / self.player.powers.max_mana,
            // Go `Unit.CurrentHealthPercent`.
            Compiled::CurrentHealthPercent => self.player.health / self.player_max_health(),
            // Go `GetRemainingDurationPercent` for a fight timed by duration.
            Compiled::RemainingTimePercent => {
                (self.duration - self.now) as f64 / self.duration as f64
            }
            Compiled::CurrentMana => self.player.mana,
            Compiled::PetCurrentMana(slot) => self.unit(Side::Pet(*slot)).mana,
            Compiled::PetCurrentManaPercent(slot) => {
                let side = Side::Pet(*slot);
                self.unit(side).mana / self.active_pet(side).config.max_mana
            }
            Compiled::CurrentEnergy => self.energy_bar().current,
            Compiled::CurrentRage => self.current_rage(),
            Compiled::MaxEnergy => self.energy_bar().max,
            Compiled::MaxMana => self.player.powers.max_mana,
            // Go `APLValueSpellCurrentCost`: no cost reads zero.
            Compiled::SpellCurrentCost(spell) => self.current_cost(*spell),
            // Go `Spell.CurCPM`: no casts per minute before the pull.
            Compiled::SpellCpm(spell) => {
                if self.now <= 0 {
                    0.0
                } else {
                    let minutes = self.now as f64 / (60 * crate::core::time::NS_PER_SECOND) as f64;
                    f64::from(self.spells[*spell].casts) / minutes
                }
            }
            Compiled::NumberTargets => self.targets.len() as f64,
            // Go `APLValueDotPercentIncrease.GetFloat`: 1 without a snapshot to compare, else the
            // ratio to it, rounded to five places, less 1.
            Compiled::DotPercentIncrease(at) => {
                let side = Side::target(at.target);
                let snapshot = self.expected_tick_damage(at.spell, side, true);
                if snapshot == 0.0 {
                    1.0
                } else {
                    let fresh = self.expected_tick_damage(at.spell, side, false);
                    ((fresh / snapshot) * 100000.0).round() / 100000.0 - 1.0
                }
            }
            // Go `APLValueDotTickRatePercentIncrease.GetFloat`: 1 while the dot is down, else the
            // running period over the one it would tick at now, less 1.
            Compiled::DotTickRatePercentIncrease(at) => {
                let dot = self.dot_of(*at);
                let current = if self.dot_active(*at) {
                    crate::core::time::seconds(self.dots[dot].tick_period)
                } else {
                    0.0
                };
                if current == 0.0 {
                    1.0
                } else {
                    current / crate::core::time::seconds(self.calc_tick_period(dot)) - 1.0
                }
            }
            // Go `Duration.Seconds` of the time so far over the fight's.
            Compiled::CurrentTimePercent => {
                crate::core::time::seconds(self.now) / crate::core::time::seconds(self.duration)
            }
            Compiled::CurrentHealth => self.player.health,
            Compiled::MaxHealth => self.player_max_health(),
            Compiled::MaxRage(max) => *max,
            // Go `manaTickWhileCasting`.
            Compiled::CurrentManaRegen => self.player.mana_tick_casting,
            // Go `DistanceFromTarget`.
            Compiled::UnitDistance => self.config.distance,
            // Go's `max` and `min` builtins, which a NaN wins.
            Compiled::Max(values) => {
                let first = self.get_float(&values[0]);
                values[1..]
                    .iter()
                    .fold(first, |result, value| go_max(result, self.get_float(value)))
            }
            Compiled::Min(values) => {
                let first = self.get_float(&values[0]);
                values[1..]
                    .iter()
                    .fold(first, |result, value| go_min(result, self.get_float(value)))
            }
            // Go `APLValueMath.GetFloat`.
            Compiled::Math { op, lhs, rhs } => match op {
                MathOp::Add => self.get_float(lhs) + self.get_float(rhs),
                MathOp::Sub => self.get_float(lhs) - self.get_float(rhs),
                MathOp::Mul => self.get_float(lhs) * self.get_float(rhs),
                MathOp::Div
                    if lhs.value_type() == ValueType::Duration
                        && rhs.value_type() == ValueType::Duration =>
                {
                    let divisor = crate::core::time::seconds(self.get_duration(rhs));
                    assert!(divisor != 0.0, "Division by zero in duration / duration");
                    crate::core::time::seconds(self.get_duration(lhs)) / divisor
                }
                MathOp::Div => self.get_float(lhs) / self.get_float(rhs),
            },
            Compiled::Coerced { inner, .. } => match inner.value_type() {
                ValueType::Bool => f64::from(u8::from(self.get_bool(inner))),
                ValueType::Int => f64::from(self.get_int(inner)),
                ValueType::Float => self.get_float(inner),
                ValueType::Duration => crate::core::time::seconds(self.get_duration(inner)),
                ValueType::String => 0.0,
            },
            _ => 0.0,
        }
    }

    fn get_duration(&mut self, value: &Compiled) -> i64 {
        match value {
            Compiled::Const(constant) => constant.duration_ns,
            // Go `APLValueAuraRemainingTime`: zero when inactive.
            Compiled::AuraRemainingTime(aura) => self.aura_remaining_time(*aura),
            Compiled::RemainingTime => self.duration - self.now,
            // Go `Spell.TimeToReady`.
            Compiled::SpellTimeToReady(spell) => self.spell_time_to_ready(*spell),
            // Go `Dot.TimeUntilNextTick`: the next tick time is zero while inactive.
            Compiled::DotTimeToNextTick(at) => {
                let next = if self.dot_active(*at) {
                    self.dots[self.dot_of(*at)].tick_next_at
                } else {
                    0
                };
                next - self.now
            }
            Compiled::Math { op, lhs, rhs } => self.math_duration(*op, lhs, rhs),
            // Go `APLValueDotBaseDuration`: what the rotation captured when it was built.
            Compiled::DotBaseDuration(duration) => *duration,
            Compiled::TotemRemainingTime {
                totem,
                include_reaction_time,
            } => {
                let delay = if *include_reaction_time {
                    self.config.reaction
                } else {
                    0
                };
                let expires = A::totem_expiration(self, *totem);
                (expires + delay - self.now).max(0)
            }
            Compiled::CurrentTime => self.now,
            Compiled::TimeToNextEnergyTick => self.time_to_next_energy_tick(),
            // Go `APLValueDotRemainingTime`: zero when inactive.
            Compiled::DotRemainingTime(at) => {
                if self.dot_active(*at) {
                    let aura = self.aura(self.dots[self.dot_of(*at)].aura);
                    aura.expires - self.now
                } else {
                    0
                }
            }
            // Go `Spell.CastTime`: the default cast time with current cast speed, unrounded.
            Compiled::SpellCastTime(spell) => self.class_cast_time(*spell).unwrap_or_else(|| {
                let cast_time = self.spells[*spell].default_cast.cast_time;
                self.apply_cast_speed_for_spell(cast_time, *spell)
            }),
            // Go `Spell.TravelTime`.
            Compiled::SpellTravelTime(spell) => self.travel_time(*spell),
            // Go `Spell.NextChargeIn`: no spell of the fork recharges.
            Compiled::SpellTimeToCharge(_) => 0,
            // Go `APLValueSpellGCDHastedDuration`: the GCD with cast speed, to the millisecond,
            // no shorter than its minimum, unless the spell ignores haste.
            Compiled::SpellGcdHastedDuration(spell) => {
                let state = &self.spells[*spell];
                let cast = state.default_cast;
                if state.ignore_haste {
                    cast.gcd
                } else {
                    let minimum = if cast.gcd_min != 0 {
                        cast.gcd_min
                    } else {
                        crate::core::time::NS_PER_SECOND
                    };
                    let hasted = self.apply_cast_speed_of(self.caster(*spell), cast.gcd);
                    minimum.max(crate::core::time::round(
                        hasted,
                        crate::core::time::NS_PER_MILLISECOND,
                    ))
                }
            }
            // Go `Spell.CD.Duration`: zero for a spell without a cooldown.
            Compiled::SpellFullCooldown(spell) => self.spells[*spell].cd.map_or(0, |(_, cd)| cd),
            Compiled::Max(values) => {
                let first = self.get_duration(&values[0]);
                values[1..]
                    .iter()
                    .fold(first, |result, value| result.max(self.get_duration(value)))
            }
            Compiled::Min(values) => {
                let first = self.get_duration(&values[0]);
                values[1..]
                    .iter()
                    .fold(first, |result, value| result.min(self.get_duration(value)))
            }
            // Go `energyBar.TimeToTargetEnergy`: whole ticks until the energy reaches the target.
            Compiled::EnergyTimeToTarget(target) => {
                let target = self.get_float(target);
                self.time_to_target_energy(target)
            }
            // Go `GCD.TimeToReady`.
            Compiled::GcdTimeToReady => self.gcd_time_to_ready(),
            // Go `APLValueAutoTimeSinceLast`, which wraps past a hand that never swung.
            Compiled::AutoTimeSinceLast(kind) => self.time_since_last_auto(*kind),
            Compiled::InputDelay => self.config.reaction,
            // Go `Timer.TimeToReady` of the aura's ICD.
            Compiled::AuraInternalCooldown(aura) => {
                let (timer, _) = self.aura(*aura).icd.expect("an ICD aura");
                (self.timers[timer] - self.now).max(0)
            }
            // Go `Aura.Duration`.
            Compiled::AuraDuration(aura) => self.aura(*aura).duration,
            Compiled::SettledDuration(duration) => *duration,
            // Go `APLValueItemProcsMinRemainingTime`: never without one up.
            Compiled::ItemProcsMinRemainingTime(buffs) => buffs
                .iter()
                .filter(|(aura, _)| self.aura(*aura).active)
                .map(|(aura, _)| self.aura(*aura).remaining(self.now))
                .fold(NEVER_EXPIRES, i64::min),
            // Go `APLValueItemProcsMaxRemainingICD`: zero without one down that can proc.
            Compiled::ItemProcsMaxRemainingIcd(buffs) => buffs
                .iter()
                .filter(|(aura, swapped)| !swapped && !self.aura(*aura).active)
                .filter_map(|(aura, _)| self.aura(*aura).icd)
                .map(|(timer, _)| (self.timers[timer] - self.now).max(0))
                .fold(0, i64::max),
            Compiled::AnyStatBuffCooldownsMinDuration(auras) => auras
                .iter()
                .filter(|aura| {
                    let state = self.aura(**aura);
                    state.active && state.stacks == state.max_stacks
                })
                .map(|aura| self.aura(*aura).remaining(self.now))
                .fold(NEVER_EXPIRES, i64::min),
            // Go `APLValueSequenceTimeToReady`: never once done or without a sequence, else the
            // time until the next step's spell is ready.
            Compiled::SequenceTimeToReady(name) => match self.named_sequence(name) {
                Some(item) => match self.rotation[item].action {
                    Act::Sequence {
                        ref spells, next, ..
                    } if next < spells.len() => self.spell_time_to_ready(spells[next]),
                    _ => NEVER_EXPIRES,
                },
                None => NEVER_EXPIRES,
            },
            // Go: zero once a target lacks the dot, otherwise the least time left.
            Compiled::DotLowestRemainingTime(dots) => {
                let mut lowest = NEVER_EXPIRES;
                for &dot in dots {
                    if !self.dot_active(dot) {
                        return 0;
                    }
                    let aura = self.aura(self.dots[self.dot_of(dot)].aura);
                    lowest = lowest.min(aura.remaining(self.now));
                }
                lowest
            }
            // Go `Dot.tickPeriod` while active, `CalcTickPeriod` otherwise.
            Compiled::DotTickFrequency(at) => {
                let dot = self.dot_of(*at);
                if self.dot_active(*at) {
                    self.dots[dot].tick_period
                } else {
                    self.calc_tick_period(dot)
                }
            }
            Compiled::ChannelClipDelay => self.config.channel_clip_delay,
            // Go `APLValueAutoTimeToNext`.
            Compiled::AutoTimeToNext(kind) => (self.next_auto_attack_at(*kind) - self.now).max(0),
            // Go `APLValueAutoSwingTime`: the hand's current swing duration.
            Compiled::AutoSwingTime(kind) => match kind {
                // Go reads a hand of the unit's auto attacks.
                crate::rotation::SwingType::MainHand => self.autos.mh.cur_swing_duration.max(0),
                crate::rotation::SwingType::OffHand => self.autos.oh.cur_swing_duration.max(0),
                crate::rotation::SwingType::Ranged => self.autos.ranged.cur_swing_duration.max(0),
                crate::rotation::SwingType::Unknown => 0,
            },
            Compiled::Coerced { inner, .. } => match inner.value_type() {
                ValueType::Bool => {
                    if self.get_bool(inner) {
                        1
                    } else {
                        0
                    }
                }
                ValueType::Int => {
                    crate::rotation::duration_from_seconds(f64::from(self.get_int(inner)))
                }
                ValueType::Float => crate::rotation::duration_from_seconds(self.get_float(inner)),
                ValueType::Duration => self.get_duration(inner),
                ValueType::String => 0,
            },
            _ => 0,
        }
    }

    /// Go `APLValueMath.GetDuration`: int64 arithmetic, which wraps, and float products
    /// truncated toward zero.
    fn math_duration(&mut self, op: MathOp, lhs: &Compiled, rhs: &Compiled) -> i64 {
        fn scale<A: Agent>(fight: &mut Fight<A>, duration: i64, by: &Compiled) -> i64 {
            match by.value_type() {
                ValueType::Int => duration.wrapping_mul(i64::from(fight.get_int(by))),
                ValueType::Float => (duration as f64 * fight.get_float(by)) as i64,
                other => panic!("invalid {other:?} operand for duration multiplication"),
            }
        }
        match op {
            MathOp::Add => self.get_duration(lhs).wrapping_add(self.get_duration(rhs)),
            MathOp::Sub => self.get_duration(lhs).wrapping_sub(self.get_duration(rhs)),
            MathOp::Mul if lhs.value_type() == ValueType::Duration => {
                let duration = self.get_duration(lhs);
                scale(self, duration, rhs)
            }
            MathOp::Mul => {
                let duration = self.get_duration(rhs);
                scale(self, duration, lhs)
            }
            MathOp::Div => match rhs.value_type() {
                ValueType::Int => {
                    let divisor = self.get_int(rhs);
                    assert!(divisor != 0, "Division by zero in duration / int");
                    self.get_duration(lhs).wrapping_div(i64::from(divisor))
                }
                ValueType::Float => {
                    let divisor = self.get_float(rhs);
                    assert!(divisor != 0.0, "Division by zero in duration / float");
                    (self.get_duration(lhs) as f64 / divisor) as i64
                }
                other => panic!("invalid {other:?} divisor for duration division"),
            },
        }
    }

    /// Go `APLValueAuraRemainingTime`: zero when inactive.
    fn aura_remaining_time(&self, aura: AuraRef) -> i64 {
        let state = self.aura(aura);
        match (state.active, state.expires) {
            (false, _) => 0,
            (true, NEVER_EXPIRES) => NEVER_EXPIRES,
            (true, expires) => expires - self.now,
        }
    }

    /// The dot a compiled rotation reads: the spell's, on the target it names. An area or
    /// self-only dot is the same on every target.
    fn dot_of(&self, at: DotAt) -> DotId {
        let dot = self.spells[at.spell].dot.expect("compiled dots have a dot");
        self.dot_on(dot, Side::target(at.target))
    }

    /// Whether the dot of a compiled spell is active on its unit.
    pub(crate) fn dot_active(&self, at: DotAt) -> bool {
        self.aura(self.dots[self.dot_of(at)].aura).active
    }

    /// Go `APLActionCastSpell.IsReady`: castable or queueable, and major cooldowns wait for
    /// the GCD unless reactive or inside a sequence.
    fn cast_ready(&mut self, spell: SpellId) -> bool {
        self.can_cast_or_queue(spell) && {
            let flags = self.spells[spell].flags;
            !flags.mcd || flags.reactive || self.gcd_ready() || self.apl.in_sequence
        }
    }

    /// Go `APLActionStrictSequence.IsReady`. A ready sequence leaves the sequence flag set,
    /// as Go does.
    fn sequence_ready(&mut self, item: usize) -> bool {
        let Act::StrictSequence {
            ref spells,
            ref changes,
            ref conditions,
            ..
        } = self.rotation[item].action
        else {
            unreachable!("item is a strict sequence");
        };
        let (first, change, condition) = (spells[0], changes[0], conditions[0].clone());
        // Go checks the GCD only for a sequence that casts.
        let casts = changes.iter().any(Option::is_none);
        self.apl.in_sequence = true;
        if casts && self.gcd_time_to_ready() > MAX_SPELL_QUEUE_WINDOW
            || !self.step_condition_holds(condition.as_deref())
            || !self.step_ready(first, change)
        {
            self.apl.in_sequence = false;
            return false;
        }
        true
    }

    /// Go `APLAction.IsReady` of a strict sequence's step without its condition: a change of
    /// target runs once a timestep, a cast when it can be cast or queued.
    fn step_ready(&mut self, spell: SpellId, change: Option<(usize, i64)>) -> bool {
        match change {
            Some((_, last)) => last != self.now,
            None => self.cast_ready(spell),
        }
    }

    /// The player's current target, Go `Character.CurrentTarget`.
    pub(crate) fn current_target_side(&self) -> Side {
        Side::target(self.player_target)
    }

    /// Go `APLActionChangeTarget.Execute`: the player's current target moves to the target at
    /// `to`, and the rotation reads its units from the copy compiled for it.
    pub(crate) fn change_target(&mut self, to: usize) {
        self.change_rotation_copy(to);
        self.log_change_target();
    }

    /// Puts the copy of the rotation compiled for the current target at `to` in place, carrying
    /// each item's state over, and forgets the cached answers of the copy that leaves.
    pub(crate) fn change_rotation_copy(&mut self, to: usize) {
        if to == self.player_target {
            return;
        }
        let from = self.player_target;
        let mut next = std::mem::take(&mut self.rotation_copies[to]);
        assert_eq!(
            next.len(),
            self.rotation.len(),
            "the rotation copies differ"
        );
        for (item, old) in next.iter_mut().zip(&self.rotation) {
            item.action.carry_state(&old.action);
        }
        self.rotation_copies[from] = std::mem::replace(&mut self.rotation, next);
        self.player_target = to;
        self.apl.proofs.clear();
        self.apl.proofs.resize(self.rotation.len(), Proof::NONE);
        self.apl.look = None;
    }

    /// Whether asking a cast item would leave no trace, so it may be answered without asking:
    /// no log is kept, its condition runs no cost check (the caller's part), and the cast's
    /// cost check keeps no bookkeeping. A rage, energy or focus cost never does; a mana cost
    /// does not while it is affordable with no out-of-mana wait open, or while it is
    /// unaffordable and no lower than the mana waited for.
    pub(crate) fn asking_is_quiet(&self, spell: SpellId) -> bool {
        let state = &self.spells[spell];
        if self.log.is_some() || state.caster != Side::Player {
            return false;
        }
        let unit = &self.player;
        if let Some(cost) = state.cost {
            if !matches!(
                cost.kind,
                ResourceKind::Energy | ResourceKind::Focus | ResourceKind::Rage
            ) {
                let cost = self.current_cost(spell);
                let waiting = unit.waiting_for_mana;
                let quiet = if unit.mana >= cost {
                    waiting == 0.0
                } else {
                    waiting != 0.0 && cost >= waiting
                };
                if cost > 0.0 && !quiet {
                    return false;
                }
            }
        }
        true
    }

    /// Whether a cast is ruled out by what blocks it whatever its condition says: one queued
    /// spell per timestep, the GCD or a channel beyond the queue window, its own cooldowns
    /// beyond it, or a major cooldown waiting for the GCD.
    fn timers_rule_out(&self, spell: SpellId) -> bool {
        let state = &self.spells[spell];
        let unit = &self.player;
        !self.can_queue_spell()
            || (state.flags.channeled && unit.hardcast.expires > self.now + MAX_SPELL_QUEUE_WINDOW)
            || (state.default_cast.gcd > 0 && (unit.gcd - self.now).max(0) > MAX_SPELL_QUEUE_WINDOW)
            || self.spell_time_to_ready(spell) > MAX_SPELL_QUEUE_WINDOW
            || (state.flags.mcd
                && !state.flags.reactive
                && !self.gcd_ready()
                && !self.apl.in_sequence)
    }

    /// Go `APLAction.IsReady`: the condition, then the action's readiness. Where asking a cast
    /// would leave no trace (see [`Fight::asking_is_quiet`]), one its timers rule out, or whose
    /// proof that it is not ready holds, is answered without asking; debug builds ask it anyway
    /// and check the answers agree.
    fn item_ready(&mut self, item: usize) -> Option<Ready> {
        let asks_cost = self.rotation[item].asks_cost;
        if !A::CACHE_ROTATION {
            if let Act::Cast(spell, _) = self.rotation[item].action {
                if !asks_cost && self.asking_is_quiet(spell) && self.timers_rule_out(spell) {
                    return None;
                }
            }
            return self.item_ready_asked(item);
        }
        // Cost checks that all leave no trace keep no bookkeeping to do.
        let asks_cost = asks_cost && !self.cost_checks_quiet(item);
        if let Act::Cast(spell, target) = self.rotation[item].action {
            if !asks_cost && self.asking_is_quiet(spell) {
                let until = if self.timers_rule_out(spell) {
                    Some((self.timers_lift_at(spell), f64::INFINITY))
                } else {
                    self.proof_until(item, true)
                };
                if let Some((until, energy)) = until {
                    self.note(until, energy);
                    debug_assert!(
                        self.item_ready_asked(item).is_none(),
                        "a cached answer that does not hold at {}",
                        self.now
                    );
                    return None;
                }
                return self.cast_item_ready(item, spell, target);
            }
        }
        if asks_cost {
            let ready = self.item_ready_asked(item);
            if ready.is_none() {
                self.note_until(self.now);
            }
            return ready;
        }
        // A false condition ends Go's asking before the action, whatever the action.
        if let Some((until, energy)) = self.proof_until(item, false) {
            self.note(until, energy);
            debug_assert!(
                self.item_ready_asked(item).is_none(),
                "a cached condition that does not hold at {}",
                self.now
            );
            return None;
        }
        if let Some(condition) = self.rotation[item].condition.clone() {
            if self.condition_refuses(item, &condition) {
                return None;
            }
        }
        let ready = self.item_action_ready(item);
        if ready.is_none() {
            match self.rotation[item].action {
                Act::Autocast => match self.autocast_until() {
                    Some((until, energy)) => self.note(until, energy),
                    None => self.note_until(self.now),
                },
                _ => self.note_until(self.now),
            }
        }
        ready
    }

    /// A sequence step's own condition, as Go's step `IsReady` reads it: the walk's promise
    /// lasts no longer than the condition's answer.
    fn step_condition_holds(&mut self, condition: Option<&Cond>) -> bool {
        let Some(condition) = condition else {
            return true;
        };
        let (holds, until, _) = self.condition_stable(condition);
        self.note_until(until);
        holds
    }

    /// Whether an item's condition is false, keeping a proof of it that holds as long as the
    /// condition's answer does.
    fn condition_refuses(&mut self, item: usize, condition: &Cond) -> bool {
        let (holds, until, reads) = self.condition_stable(condition);
        if holds {
            return false;
        }
        // A condition that reads no energy keeps its answer as energy rises.
        let energy = if !A::CACHE_OVER_ENERGY || until <= self.now {
            self.energy_now()
        } else if reads & reads::RESOURCES == 0 || !condition.reads_energy() {
            f64::INFINITY
        } else {
            self.condition_energy_limit(condition)
        };
        self.prove(item, until, reads, true, energy);
        self.note(until, energy);
        true
    }

    /// The walk's promise shrinks to `until`, and to not holding once energy rises.
    fn note_until(&mut self, until: i64) {
        self.note(until, self.energy_now());
    }

    /// The walk's promise shrinks to `until`, and to holding as energy rises only while it stays
    /// below `energy`.
    fn note(&mut self, until: i64, energy: f64) {
        self.apl.walk_until = self.apl.walk_until.min(until);
        if A::CACHE_OVER_ENERGY {
            self.apl.walk_energy = self.apl.walk_energy.min(energy);
        }
    }

    /// Whether every cost check an item's condition runs is a `spellCanCast` asked where asking
    /// leaves no trace.
    fn cost_checks_quiet(&self, item: usize) -> bool {
        let item = &self.rotation[item];
        item.asks_spell_costs
            && item.condition.as_deref().is_some_and(|condition| {
                condition.cost_checks_all(&|spell| self.asking_is_quiet(spell))
            })
    }

    /// For a spell that cannot cast or queue, asked where asking leaves no trace: until when
    /// the clock alone cannot change that, read as a cast item's proof reads it (its queue,
    /// channel, GCD and cooldowns, then the form, the extra condition or the cost). A swapped
    /// spell, and an energy cost where looks hold as energy rises, give no time.
    pub(crate) fn cast_refused_until(&mut self, spell: SpellId) -> i64 {
        let state = &self.spells[spell];
        let energy = matches!(state.cost.map(|cost| cost.kind), Some(ResourceKind::Energy));
        if state.flags.swapped || (A::CACHE_OVER_ENERGY && energy) {
            return self.now;
        }
        let unit = &self.player;
        let window = MAX_SPELL_QUEUE_WINDOW;
        let mut at = self.now;
        if !self.can_queue_spell() {
            at = at.max(self.now + 1);
        }
        if state.flags.channeled && unit.hardcast.expires > self.now + window {
            at = at.max(unit.hardcast.expires - window);
        }
        if state.default_cast.gcd > 0 && unit.gcd - self.now > window {
            at = at.max(unit.gcd - window);
        }
        let ready = self.spell_ready_at(spell);
        if ready - self.now > window {
            at = at.max(ready - window);
        }
        if at > self.now {
            return at;
        }
        if self.wrong_form(spell) || self.extra_cast_condition(spell) {
            // The form refuses it, or the extra condition lets it go and the cost refuses it.
            return NEVER_EXPIRES;
        }
        self.extra_condition_proof(spell)
            .map_or(self.now, |(until, _)| until)
    }

    /// When a cast's timers stop ruling it out with nothing but the clock moving: once every
    /// part that does has run out.
    fn timers_lift_at(&self, spell: SpellId) -> i64 {
        let state = &self.spells[spell];
        let unit = &self.player;
        let window = MAX_SPELL_QUEUE_WINDOW;
        let mut at = self.now;
        if !self.can_queue_spell() {
            at = at.max(self.now + 1);
        }
        if state.flags.channeled && unit.hardcast.expires > self.now + window {
            at = at.max(unit.hardcast.expires - window);
        }
        if state.default_cast.gcd > 0 && unit.gcd - self.now > window {
            at = at.max(unit.gcd - window);
        }
        let ready = self.spell_ready_at(spell);
        if ready - self.now > window {
            at = at.max(ready - window);
        }
        if state.flags.mcd && !state.flags.reactive && unit.gcd > self.now && !self.apl.in_sequence
        {
            at = at.max(unit.gcd);
        }
        at
    }

    /// Until when an item's proof that it is not ready holds, if it still does. Only a
    /// condition's proof serves where asking the action could keep bookkeeping.
    fn proof_until(&self, item: usize, quiet: bool) -> Option<(i64, f64)> {
        let proof = self.apl.proofs.get(item)?;
        let (now, versions) = (&self.versions, &proof.versions);
        let holds = self.now < proof.until
            && (quiet || proof.condition_only)
            && (proof.reads & reads::TIMERS == 0 || versions.timers == now.timers)
            && (proof.reads & reads::RESOURCES == 0 || versions.resources == now.resources)
            && (proof.reads & reads::AURAS == 0 || versions.auras == now.auras)
            && (proof.reads & reads::CASTS == 0 || versions.casts == now.casts)
            && (proof.reads & reads::SPELLS == 0 || versions.spells == now.spells)
            && (proof.reads & reads::CLASS == 0 || versions.class == now.class)
            && (proof.reads & reads::PHASE == 0 || proof.execute_phase == self.execute_phase);
        holds.then_some((proof.until, proof.energy_limit))
    }

    /// Keeps an item's "not ready" until the clock reaches `until` or something in `reads`
    /// changes, and notes the energy below which it holds as energy rises.
    fn prove(&mut self, item: usize, until: i64, reads: u8, condition_only: bool, energy: f64) {
        if until <= self.now {
            return;
        }
        let proof = Proof {
            until,
            reads,
            versions: self.versions,
            execute_phase: self.execute_phase,
            condition_only,
            energy_limit: energy,
        };
        if let Some(slot) = self.apl.proofs.get_mut(item) {
            *slot = proof;
        }
    }

    /// A cast item whose timers let it go, asked where asking leaves no trace: its condition,
    /// then its cast check. Not ready, it keeps a proof of why where one is known.
    fn cast_item_ready(&mut self, item: usize, spell: SpellId, target: Side) -> Option<Ready> {
        if let Some(condition) = self.rotation[item].condition.clone() {
            if self.condition_refuses(item, &condition) {
                return None;
            }
        }
        if self.cast_ready(spell) {
            return Some(Ready::Cast(spell, target));
        }
        // The timers let it go: the form, the extra condition or the cost refused it.
        let proof = if self.wrong_form(spell) {
            Some((NEVER_EXPIRES, reads::AURAS, f64::INFINITY))
        } else if self.extra_cast_condition(spell) {
            let energy = self.energy_cost_limit(spell);
            Some((NEVER_EXPIRES, reads::RESOURCES | reads::SPELLS, energy))
        } else {
            self.extra_condition_proof(spell)
                .map(|(until, reads)| (until, reads, f64::INFINITY))
        };
        match proof {
            Some((until, reads, energy)) => {
                self.prove(item, until, reads, false, energy);
                self.note(until, energy);
            }
            None => self.note_until(self.now),
        }
        None
    }

    /// [`Fight::item_ready`] asked in full, as Go asks it.
    fn item_ready_asked(&mut self, item: usize) -> Option<Ready> {
        if let Some(condition) = self.rotation[item].condition.clone() {
            if !self.condition(&condition) {
                return None;
            }
        }
        self.item_action_ready(item)
    }

    /// Go `APLAction.IsReady` after the condition: the action's readiness.
    fn item_action_ready(&mut self, item: usize) -> Option<Ready> {
        match self.rotation[item].action {
            Act::Cast(spell, target) => {
                self.cast_ready(spell).then_some(Ready::Cast(spell, target))
            }
            Act::Autocast => self.autocast_ready().map(Ready::Autocast),
            Act::StrictSequence { .. } => {
                self.sequence_ready(item).then_some(Ready::Sequence(item))
            }
            Act::Sequence { .. } => self
                .sequence_step_ready(item)
                .then_some(Ready::SequenceStep(item)),
            // Go `APLActionWait.IsReady`: a duration to wait.
            Act::Wait { ref duration, .. } => {
                let duration = Rc::clone(duration);
                (self.get_duration(&duration) > 0).then_some(Ready::Wait(item))
            }
            // Go `APLActionWaitUntil.IsReady`: the condition does not hold yet.
            Act::WaitUntil(ref condition) => {
                let condition = Rc::clone(condition);
                (!self.get_bool(&condition)).then_some(Ready::WaitUntil(item))
            }
            // Go `APLActionSchedule.IsReady`: a time left that has come, and the inner action.
            Act::Schedule {
                ref timings,
                next,
                inner,
            } => {
                if next >= timings.len() || self.now < timings[next] {
                    return None;
                }
                self.item_ready(inner)
                    .map(|ready| Ready::Scheduled(item, Box::new(ready)))
            }
            Act::StrictMultidot { .. } => self
                .strict_multidot_ready(item)
                .then_some(Ready::StrictMultidot(item)),
            // Go `APLActionCastAllStatBuffCooldowns.IsReady`: the ready casts of those worn, all of
            // them inside a sequence.
            Act::CastAllStatBuffCooldowns { ref spells, .. } => {
                let spells = Rc::clone(spells);
                if spells.is_empty() {
                    return None;
                }
                let ready: Vec<SpellId> = spells
                    .iter()
                    .copied()
                    .filter(|&spell| self.cast_ready(spell))
                    .collect();
                let go = if self.apl.in_sequence {
                    ready.len() == spells.len()
                } else {
                    !ready.is_empty()
                };
                if let Act::CastAllStatBuffCooldowns {
                    ready: ref mut found,
                    ..
                } = self.rotation[item].action
                {
                    *found = ready;
                }
                go.then_some(Ready::CastAllStatBuffCooldowns(item))
            }
            // Go `APLActionCastWarlockAssignedCurse.IsReady`: `CanCast`, which does not queue.
            Act::CastAssignedCurse(spell, side) => self
                .can_cast(spell)
                .then_some(Ready::CastAssignedCurse(spell, side)),
            // Go `APLActionChangeTarget.IsReady`: once a timestep.
            Act::ChangeTarget { last, .. } => {
                (last != self.now).then_some(Ready::ChangeTarget(item))
            }
            // Go `APLActionCancelAura.IsReady`: the aura is active.
            Act::CancelAura(aura) => self.aura(aura).active.then_some(Ready::CancelAura(aura)),
            // Go `APLActionResetSequence.IsReady`: its sequence has started.
            Act::ResetSequence { sequence, .. } => sequence
                .filter(|&sequence| self.sequence_next(sequence) != 0)
                .map(|_| Ready::ResetSequence(item)),
            Act::Channel { spell, .. } => self
                .can_cast_or_queue(spell)
                .then_some(Ready::Channel(item, spell)),
            // Go `APLActionMove.IsReady`: not already moving, a different range or the prepull,
            // and no cast or channel in progress.
            Act::Move(ref range) => {
                let range = Rc::clone(range);
                self.move_ready(&range).then_some(Ready::Move(item))
            }
            Act::MoveDuration(ref duration) => {
                let duration = Rc::clone(duration);
                self.move_duration_ready(&duration)
                    .then_some(Ready::MoveDuration(item))
            }
            // Go APLActionGroupReference.IsReady: any action of the group is ready.
            Act::Group { instance } => self.group_ready(instance).map(|_| Ready::Group(item)),
            // Go APLActionMultidot.IsReady: the overlap, then the target whose dot is down or
            // ends within it and which the spell can be cast or queued on.
            Act::Multidot { spell, .. } => self
                .multidot_ready(item)
                .map(|target| Ready::Cast(spell, target)),
            // Go `APLActionItemSwap.IsReady`: the other set is worn.
            Act::ItemSwap(to_swapped) => self
                .item_swap_ready(to_swapped)
                .then_some(Ready::ItemSwap(to_swapped)),
        }
    }

    /// Go `APLActionStrictMultidot.IsReady`: the overlap, then each target in turn whose dot is
    /// down or ends within it, or, after one that is taken, ends no later than the one before
    /// it once the cast's time passes, and which the spell can be cast or queued on.
    fn strict_multidot_ready(&mut self, item: usize) -> bool {
        let Act::StrictMultidot {
            spell,
            dot_spell,
            max_dots,
            ref overlap,
            ..
        } = self.rotation[item].action
        else {
            unreachable!("item is a strict multidot");
        };
        let overlap = Rc::clone(overlap);
        let overlap = self.get_duration(&overlap);
        let dot = self.spells[dot_spell]
            .dot
            .expect("multidot spells have a dot");
        let remaining = |fight: &Self, target: Side| {
            let aura = fight.aura(fight.dots[fight.dot_on(dot, target)].aura);
            (aura.active, aura.remaining(fight.now))
        };
        let mut ready = Vec::new();
        let mut previous: Option<Side> = None;
        for position in 0..max_dots.max(0) as usize {
            let target = Side::target(position);
            let (active, left) = remaining(self, target);
            if (!active || left < overlap) && self.can_cast_or_queue(spell) {
                ready.push(target);
                previous = Some(target);
            } else if let Some(before) = previous {
                let (before_active, before_left) = remaining(self, before);
                if (!before_active || left - self.effective_cast_time(spell) <= before_left)
                    && self.can_cast_or_queue(spell)
                {
                    ready.push(target);
                    previous = Some(target);
                }
            }
        }
        let any = !ready.is_empty();
        if let Act::StrictMultidot {
            ready: ref mut found,
            ..
        } = self.rotation[item].action
        {
            *found = ready;
        }
        any
    }

    /// Go `Spell.EffectiveCastTime`: the GCD or the cast, whichever is longer, with cast speed
    /// unless the spell ignores haste.
    fn effective_cast_time(&self, spell: SpellId) -> i64 {
        let state = &self.spells[spell];
        let mut cast = state.default_cast;
        if !state.ignore_haste {
            cast.gcd = self.apply_cast_speed_of(self.caster(spell), cast.gcd);
            cast.cast_time = self.apply_cast_speed_for_spell(cast.cast_time, spell);
        }
        cast.effective_time()
    }

    /// Go `APLActionSequence.IsReady`: the next step's readiness inside the sequence, its
    /// condition first.
    fn sequence_step_ready(&mut self, item: usize) -> bool {
        let Act::Sequence {
            ref spells,
            ref conditions,
            next,
            ..
        } = self.rotation[item].action
        else {
            unreachable!("item is a sequence");
        };
        let step = spells.get(next).copied();
        let condition = conditions.get(next).cloned().flatten();
        self.apl.in_sequence = true;
        let ready = step.is_some_and(|spell| {
            self.step_condition_holds(condition.as_deref()) && self.cast_ready(spell)
        });
        self.apl.in_sequence = false;
        ready
    }

    /// The step a sequence runs next.
    fn sequence_next(&self, item: usize) -> usize {
        match self.rotation[item].action {
            Act::Sequence { next, .. } => next,
            _ => unreachable!("item is a sequence"),
        }
    }

    /// The priority list's first sequence of the name, as Go's finalizing finds it.
    fn named_sequence(&self, name: &str) -> Option<usize> {
        self.rotation_sequences
            .iter()
            .find(|(found, _)| &**found == name)
            .map(|(_, item)| *item)
    }

    /// Whether a Wait or WaitUntil controls the rotation: Go's `HasBlockingControllingAction`,
    /// which no event wakes early.
    pub(crate) fn rotation_blocked(&self) -> bool {
        self.apl.controlling.is_some_and(|item| {
            matches!(
                self.rotation[item].action,
                Act::Wait { .. } | Act::WaitUntil(_)
            )
        })
    }

    /// The first action of a group instance that is ready, Go `APLActionGroupReference.IsReady`
    /// and `Execute` both look for.
    fn group_ready(&mut self, instance: Option<usize>) -> Option<Ready> {
        let range = instance.and_then(|instance| self.group_ranges.get(instance).cloned())?;
        range.into_iter().find_map(|item| self.item_ready(item))
    }

    /// Go `APLActionMove.IsReady`: not already moving, a different range or the prepull, and no
    /// cast in progress.
    fn move_ready(&mut self, range: &Compiled) -> bool {
        let ready = !self.player.moving
            && (self.get_float(range) != self.config.distance || self.now < 0)
            && self.player.hardcast.expires < self.now;
        // Forever's rule (README, Rust-only mechanics): a player who has queued a spell stays
        // put until it fires. The reference walks at once, so a shot queued for the end of the
        // global cooldown fires inside its minimum range, or a strike outside melee range.
        if ready && self.move_waits_for_queued_cast && self.queued_cast_pending() {
            self.apl.move_held_for_queue = true;
            return false;
        }
        ready
    }

    /// Whether the player has a queued spell still waiting to fire that is in range where the
    /// player stands: waiting for one out of range would not let it land, and a rotation that
    /// keeps queueing it would never move.
    fn queued_cast_pending(&self) -> bool {
        self.player
            .queued
            .is_some_and(|queued| queued.action.is_some() && self.in_cast_range(queued.spell))
    }

    /// Go `APLActionMoveDuration.IsReady`: not moving, or the move ends this step, for a
    /// duration that is not zero, with no channel and no cast in progress, unless the cast
    /// allows moving.
    fn move_duration_ready(&mut self, duration: &Compiled) -> bool {
        if self.player.moving {
            let ends = self.player.movement.map(|movement| movement.end());
            if ends != Some(self.now) {
                return false;
            }
        }
        if self.get_duration(duration) == 0 {
            return false;
        }
        let hardcast = self.player.hardcast;
        let can_move = hardcast
            .spell
            .is_some_and(|spell| self.spells[spell].flags.can_cast_while_moving);
        (hardcast.expires < self.now || can_move) && self.player.channeled_dot.is_none()
    }

    /// Go `APLActionMultidot.IsReady`: the overlap, then the first target in unit index order
    /// whose dot is down or ends within it and which the spell can be cast or queued on.
    fn multidot_ready(&mut self, item: usize) -> Option<Side> {
        let Act::Multidot {
            spell,
            dot_spell,
            max_dots,
            ref overlap,
        } = self.rotation[item].action
        else {
            unreachable!("item is a multidot");
        };
        let overlap = overlap.clone();
        let overlap = overlap.map_or(0, |value| self.get_duration(&value));
        let dot = self.spells[dot_spell]
            .dot
            .expect("multidot spells have a dot");
        for position in 0..max_dots.max(0) as usize {
            let target = Side::target(position);
            let aura = self.aura(self.dots[self.dot_on(dot, target)].aura);
            let active = aura.active;
            let remaining = if active { aura.expires - self.now } else { 0 };
            if (!active || remaining < overlap) && self.can_cast_or_queue(spell) {
                return Some(target);
            }
        }
        None
    }

    /// Go `APLRotation.getNextAction`. A look that found nothing answers the next one while
    /// nothing any item read has changed and the clock has not reached the time one of them
    /// could change by itself; debug builds walk the list anyway and check it agrees.
    fn next_action(&mut self) -> Option<Ready> {
        if let Some(item) = self.apl.controlling {
            self.apl.look = None;
            return match self.rotation[item].action {
                // Go `APLActionWait.GetNextAction`: once the wait is over the list runs again.
                Act::Wait { until, .. } => {
                    if self.now >= until {
                        self.apl.controlling = None;
                        self.next_action()
                    } else {
                        None
                    }
                }
                // Go `APLActionWaitUntil.GetNextAction`.
                Act::WaitUntil(ref condition) => {
                    let condition = Rc::clone(condition);
                    if self.get_bool(&condition) {
                        self.apl.controlling = None;
                        self.next_action()
                    } else {
                        None
                    }
                }
                _ => self.sequence_next_action(item),
            };
        }
        if !A::CACHE_ROTATION {
            return (0..self.priority_len).find_map(|item| self.item_ready(item));
        }
        if self.look_holds() {
            debug_assert!(
                self.walk_priority_list().is_none(),
                "a cached look that does not hold at {}",
                self.now
            );
            return None;
        }
        let found = self.walk_priority_list();
        self.apl.look = (found.is_none() && self.log.is_none() && self.apl.walk_until > self.now)
            .then_some(Look {
                until: self.apl.walk_until,
                versions: self.versions,
                execute_phase: self.execute_phase,
                waiting_for_mana: self.player.waiting_for_mana,
                energy_limit: self.apl.walk_energy,
            });
        found
    }

    /// The first ready item of the priority list, noting until when the answers it asked hold.
    fn walk_priority_list(&mut self) -> Option<Ready> {
        self.apl.walk_until = NEVER_EXPIRES;
        self.apl.walk_energy = f64::INFINITY;
        (0..self.priority_len).find_map(|item| self.item_ready(item))
    }

    /// Whether the last look that found nothing still answers for this one.
    fn look_holds(&self) -> bool {
        self.apl.look.is_some_and(|look| {
            self.now < look.until
                && self.look_versions_hold(&look)
                && look.execute_phase == self.execute_phase
                && look.waiting_for_mana == self.player.waiting_for_mana
                && self.log.is_none()
                && !self.apl.in_sequence
        })
    }

    /// Whether nothing a look read has changed, or only energy has risen and stays below the
    /// level at which an answer it asked could change.
    fn look_versions_hold(&self, look: &Look) -> bool {
        let (then, now) = (&look.versions, &self.versions);
        if then == now {
            return true;
        }
        if !A::CACHE_OVER_ENERGY {
            return false;
        }
        let gains = now.energy_gains - then.energy_gains;
        let only_energy = super::Versions {
            resources: now.resources,
            energy_gains: now.energy_gains,
            ..*then
        };
        gains > 0
            && now.resources - then.resources == gains
            && only_energy == *now
            && self.energy_now() < look.energy_limit
    }

    /// Go `APLActionStrictSequence.GetNextAction`, of a strict sequence or of the one a strict
    /// multidot runs.
    fn sequence_next_action(&mut self, item: usize) -> Option<Ready> {
        let (step, spell, target, change, condition) = match self.rotation[item].action {
            Act::StrictSequence {
                ref spells,
                ref targets,
                ref changes,
                ref conditions,
                next,
            } => (
                next,
                spells[next],
                targets[next],
                changes[next],
                conditions[next].clone(),
            ),
            Act::StrictMultidot {
                spell,
                ref ready,
                next,
                ..
            } => (next, spell, ready[next], None, None),
            _ => unreachable!("only strict sequences and waits control the rotation"),
        };
        // Go `APLAction.IsReady` of the step: its condition, then its cast.
        if self.step_condition_holds(condition.as_deref()) && self.step_ready(spell, change) {
            // Off the GCD the step advances now; otherwise the cast queues and the step
            // stays until it fires. A change of target off the GCD runs again later.
            if self.gcd_ready() {
                self.advance_sequence(item);
            }
            Some(match change {
                Some(_) => Ready::SequenceChangeTarget(item, step),
                None => Ready::Cast(spell, target),
            })
        } else if !self.can_queue_spell() {
            // A spell was queued this timestep: advance when its action fires.
            if !self.apl.queue_hooks.contains(&item) {
                self.apl.queue_hooks.push(item);
            }
            let fire_at = self.player.queued.expect("a queued spell").fire_at;
            self.set_rotation_timer(fire_at + 1);
            None
        } else if self.gcd_time_to_ready() <= MAX_SPELL_QUEUE_WINDOW {
            // The GCD is ready and the step is not: the sequence is bad, so leave it.
            self.relinquish_sequence(item);
            self.next_action()
        } else {
            None
        }
    }

    /// Go `APLActionStrictSequence.advanceSequence`, or a sequence's step advancing when its
    /// queued cast fires.
    pub(crate) fn advance_sequence(&mut self, item: usize) {
        let done = match self.rotation[item].action {
            Act::Sequence { ref mut next, .. } => {
                *next += 1;
                return;
            }
            Act::StrictSequence {
                ref spells,
                ref mut next,
                ..
            } => {
                *next += 1;
                *next == spells.len()
            }
            Act::StrictMultidot {
                ref ready,
                ref mut next,
                ..
            } => {
                *next += 1;
                *next == ready.len()
            }
            _ => unreachable!("item is a strict sequence"),
        };
        if done {
            self.relinquish_sequence(item);
        }
    }

    /// Go `APLActionStrictSequence.relinquishControl`.
    fn relinquish_sequence(&mut self, item: usize) {
        if let Act::StrictSequence { ref mut next, .. } | Act::StrictMultidot { ref mut next, .. } =
            self.rotation[item].action
        {
            *next = 0;
        }
        self.apl.queue_hooks.retain(|&hooked| hooked != item);
        self.apl.in_sequence = false;
        assert_eq!(
            self.apl.controlling,
            Some(item),
            "wrong APL controlling action"
        );
        self.apl.controlling = None;
    }

    fn execute(&mut self, action: Ready) {
        match action {
            Ready::Cast(spell, target) => self.cast_or_queue(spell, target),
            Ready::Autocast(cooldown) => self.autocast(cooldown),
            Ready::Sequence(item) => {
                self.apl.in_sequence = true;
                assert!(self.apl.controlling.is_none(), "nested controlling actions");
                self.apl.controlling = Some(item);
            }
            // Go APLActionSequence.Execute: a cast advances the step at once; a queued one
            // advances it when its action fires, and the rotation waits until just after.
            Ready::SequenceStep(item) => {
                self.apl.in_sequence = true;
                let Act::Sequence {
                    ref spells,
                    ref targets,
                    next,
                    ..
                } = self.rotation[item].action
                else {
                    unreachable!("item is a sequence");
                };
                let (spell, target) = (spells[next], targets[next]);
                self.cast_or_queue(spell, target);
                if self.can_queue_spell() {
                    self.advance_sequence(item);
                } else {
                    self.apl.queue_hooks.push(item);
                    let fire_at = self.player.queued.expect("a queued spell").fire_at;
                    self.set_rotation_timer(fire_at + 1);
                }
                self.apl.in_sequence = false;
            }
            // Go `APLActionMove.Execute`: the value is read again.
            Ready::Move(item) => {
                let Act::Move(ref range) = self.rotation[item].action else {
                    unreachable!("item is a move");
                };
                let range = Rc::clone(range);
                let range = self.get_float(&range);
                if self.log.is_some() {
                    let line = format!("[DEBUG] Moving to {range:.1} yards");
                    self.player_log(&line);
                }
                self.move_to(Side::Player, range);
            }
            // Go `APLActionGroupReference.Execute`: the first action that is ready, found again.
            Ready::Group(item) => {
                let Act::Group { instance } = self.rotation[item].action else {
                    unreachable!("item is a group reference");
                };
                if let Some(ready) = self.group_ready(instance) {
                    self.execute(ready);
                }
            }
            // Go `APLActionMoveDuration.Execute`.
            Ready::MoveDuration(item) => {
                let Act::MoveDuration(ref duration) = self.rotation[item].action else {
                    unreachable!("item is a move for a duration");
                };
                let duration = Rc::clone(duration);
                let duration = self.get_duration(&duration);
                self.move_duration(Side::Player, duration);
            }
            Ready::ItemSwap(to_swapped) => self.item_swap_action(to_swapped),
            // Go `APLActionWait.Execute`: control, then wait for the duration read again.
            Ready::Wait(item) => {
                assert!(self.apl.controlling.is_none(), "nested controlling actions");
                self.apl.controlling = Some(item);
                let Act::Wait { ref duration, .. } = self.rotation[item].action else {
                    unreachable!("item is a wait");
                };
                let duration = Rc::clone(duration);
                let until = self.now + self.get_duration(&duration);
                if let Act::Wait {
                    until: ref mut at, ..
                } = self.rotation[item].action
                {
                    *at = until;
                }
                self.wait_until(until);
            }
            // Go `APLActionStrictMultidot.Execute`: a strict sequence of the casts on the targets
            // its readiness found takes control.
            Ready::StrictMultidot(item) => {
                if let Act::StrictMultidot { ref mut next, .. } = self.rotation[item].action {
                    *next = 0;
                }
                self.apl.in_sequence = true;
                assert!(self.apl.controlling.is_none(), "nested controlling actions");
                self.apl.controlling = Some(item);
            }
            // Go `APLActionCastAllStatBuffCooldowns.Execute`: the ready ones, every one before
            // the pull.
            Ready::CastAllStatBuffCooldowns(item) => {
                let Act::CastAllStatBuffCooldowns {
                    ref spells,
                    ref ready,
                } = self.rotation[item].action
                else {
                    unreachable!("item casts the stat buff cooldowns");
                };
                let casts: Vec<SpellId> = if self.now < 0 {
                    spells.to_vec()
                } else {
                    ready.clone()
                };
                let target = self.current_target_side();
                for spell in casts {
                    self.cast_or_queue(spell, target);
                }
            }
            // Go `APLActionCastWarlockAssignedCurse.Execute`: `Spell.Cast` on the action's target.
            Ready::CastAssignedCurse(spell, side) => {
                self.cast(spell, side);
            }
            // Go `APLActionChangeTarget.Execute`: the first target stays the current one.
            Ready::ChangeTarget(item) => {
                let Act::ChangeTarget { ref mut last, to } = self.rotation[item].action else {
                    unreachable!("item is a change of target");
                };
                *last = self.now;
                self.change_target(to);
            }
            // Go `APLActionChangeTarget.Execute` as a strict sequence's step.
            Ready::SequenceChangeTarget(item, step) => {
                let Act::StrictSequence {
                    ref mut changes, ..
                } = self.rotation[item].action
                else {
                    unreachable!("item is a strict sequence");
                };
                let change = changes[step].as_mut().expect("the step changes target");
                change.1 = self.now;
                let to = change.0;
                self.change_target(to);
            }
            // Go `APLActionCancelAura.Execute`.
            Ready::CancelAura(aura) => {
                if self.log.is_some() {
                    let id = self.aura(aura).action_id.clone().unwrap_or_default();
                    let line = format!("Cancelling aura {}", super::log::action_string(&id));
                    self.unit_log(aura.side, &line);
                }
                self.deactivate_aura(aura);
            }
            // Go `APLActionWaitUntil.Execute`.
            Ready::WaitUntil(item) => {
                assert!(self.apl.controlling.is_none(), "nested controlling actions");
                self.apl.controlling = Some(item);
            }
            // Go `APLActionSchedule.Execute`: the next time, then the inner action.
            Ready::Scheduled(item, inner) => {
                if let Act::Schedule { ref mut next, .. } = self.rotation[item].action {
                    *next += 1;
                }
                self.execute(*inner);
            }
            // Go `APLActionResetSequence.Execute`.
            Ready::ResetSequence(item) => {
                let Act::ResetSequence {
                    sequence: Some(sequence),
                    ..
                } = self.rotation[item].action
                else {
                    unreachable!("a ready reset has its sequence");
                };
                if let Act::Sequence { ref mut next, .. } = self.rotation[sequence].action {
                    *next = 0;
                }
            }
            Ready::Channel(item, spell) => {
                self.cast_or_queue(spell, self.current_target_side());
                let Act::Channel { allow_recast, .. } = self.rotation[item].action else {
                    unreachable!("item is a channel");
                };
                self.apl.interrupt_channel_if = Some(item);
                self.apl.allow_channel_recast = allow_recast;
            }
        }
    }

    /// Go `APLActionChannelSpell.Execute` in the prepull: the cast, then the rotation reads the
    /// channel's interrupt condition.
    /// Go `APLActionStrictSequence.Execute` before the pull: the sequence takes control, and
    /// the rotation runs its steps once it acts. A second one while one holds control is
    /// refused by the gate.
    pub(crate) fn prepull_sequence(&mut self, item: usize) {
        self.apl.in_sequence = true;
        assert!(self.apl.controlling.is_none(), "nested controlling actions");
        self.apl.controlling = Some(item);
    }

    pub(crate) fn prepull_channel(&mut self, spell: SpellId, item: usize) {
        self.cast_or_queue(spell, self.current_target_side());
        let Act::Channel { allow_recast, .. } = self.rotation[item].action else {
            unreachable!("item is a channel");
        };
        self.apl.interrupt_channel_if = Some(item);
        self.apl.allow_channel_recast = allow_recast;
    }

    /// Go `Dot.ChannelCanBeInterrupted`.
    fn channel_can_be_interrupted(&mut self, dot: DotId) -> bool {
        let state = &self.dots[dot];
        if !state.channeled || state.remaining_ticks == 0 {
            return false;
        }
        let interrupt = match self.apl.interrupt_channel_if {
            Some(item) => match &self.rotation[item].action {
                Act::Channel { interrupt, .. } => Rc::clone(interrupt),
                _ => return false,
            },
            None => return false,
        };
        self.get_bool(&interrupt)
    }

    /// Whether two spells share a class spell mask, as Go `Spell.Matches` with the other's
    /// mask.
    fn same_class_spell(&self, a: SpellId, b: SpellId) -> bool {
        match (&self.spells[a].class_spell, &self.spells[b].class_spell) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }

    /// Go `nextActionWouldRecastChannel`, evaluated with no channel running. The caller
    /// restores the channel.
    fn next_action_would_recast_channel(&mut self, dot: DotId) -> bool {
        let channeled = self.dots[dot].spell;
        self.player.channeled_dot = None;
        for item in 0..self.priority_len {
            if let Some(condition) = self.rotation[item].condition.clone() {
                if !self.condition(&condition) {
                    continue;
                }
            }
            let spell = match self.rotation[item].action {
                Act::Cast(spell, _) | Act::Channel { spell, .. } => spell,
                Act::Autocast | Act::Sequence { .. } => continue,
                // Go: a different action that is fully ready would be cast first.
                Act::Move(ref range) => {
                    let range = Rc::clone(range);
                    if self.move_ready(&range) {
                        return false;
                    }
                    continue;
                }
                Act::MoveDuration(ref duration) => {
                    let duration = Rc::clone(duration);
                    if self.move_duration_ready(&duration) {
                        return false;
                    }
                    continue;
                }
                // Go: a different action that is fully ready would be cast first.
                Act::Group { instance } => {
                    if self.group_ready(instance).is_some() {
                        return false;
                    }
                    continue;
                }
                // Go: a different action that is fully ready would be cast first.
                Act::Multidot { .. } => {
                    if self.multidot_ready(item).is_some() {
                        return false;
                    }
                    continue;
                }
                // Go: a different action that is fully ready would be cast first.
                Act::ItemSwap(to_swapped) => {
                    if self.item_swap_ready(to_swapped) {
                        return false;
                    }
                    continue;
                }
                Act::StrictSequence { .. } => {
                    // A different action that is fully ready would be cast first.
                    if self.sequence_ready(item) {
                        return false;
                    }
                    continue;
                }
                // Go: a different action that is fully ready would be cast first.
                Act::Wait { .. }
                | Act::WaitUntil(_)
                | Act::Schedule { .. }
                | Act::StrictMultidot { .. }
                | Act::CancelAura(_)
                | Act::ChangeTarget { .. }
                | Act::CastAllStatBuffCooldowns { .. }
                | Act::CastAssignedCurse(..)
                | Act::ResetSequence { .. } => {
                    if self.item_action_ready(item).is_some() {
                        return false;
                    }
                    continue;
                }
            };
            // Go uses CanCast, so a spell queued this timestep does not block the recast.
            let can_cast = self.can_cast(spell);
            if spell == channeled || self.same_class_spell(spell, channeled) {
                return can_cast;
            }
            if can_cast {
                return false;
            }
        }
        false
    }

    /// Go `APLRotation.shouldInterruptChannel`.
    pub(crate) fn should_interrupt_channel(&mut self) -> bool {
        let Some(dot) = self.player.channeled_dot else {
            return false;
        };
        if !self.channel_can_be_interrupted(dot) {
            return false;
        }
        let would_recast = self.next_action_would_recast_channel(dot);
        self.player.channeled_dot = Some(dot);
        if would_recast {
            self.apl.allow_channel_recast
        } else {
            true
        }
    }

    /// End a channel the rotation interrupts: no tick runs on expiry, then the rotation
    /// waits out the clip delay when the GCD is ready.
    pub(crate) fn interrupt_channel(&mut self, dot: DotId) {
        let delay = self.config.channel_clip_delay;
        self.dots[dot].tick_next_at = NEVER_EXPIRES;
        let aura = self.dots[dot].aura;
        self.deactivate_aura(aura);
        if self.gcd_ready() {
            self.wait_until(self.now + delay);
        }
    }

    /// Go channel OnExpire: the rotation forgets the channel's interrupt condition.
    pub(crate) fn forget_channel_interrupt(&mut self) {
        self.apl.interrupt_channel_if = None;
        self.apl.allow_channel_recast = false;
    }

    /// Go `APLRotation.DoNextAction`.
    pub(crate) fn do_next_action(&mut self) {
        if self.now < 0 || self.in_rotation {
            return;
        }
        if let Some(dot) = self.player.channeled_dot {
            // All ticks spent but the aura not yet expired: end it now.
            if self.dots[dot].remaining_ticks == 0 {
                let aura = self.dots[dot].aura;
                self.deactivate_aura(aura);
                return;
            }
            // Go also evaluates the interrupt condition when the GCD fires.
            if self.should_interrupt_channel() {
                self.interrupt_channel(dot);
            }
            return;
        }
        if self.player.rotation_timer > self.now {
            return;
        }
        self.in_rotation = true;
        // Go brings the unit's position up to date before the rotation reads it.
        self.update_position(Side::Player, false);
        let mut executed = 0;
        while let Some(action) = self.next_action() {
            assert!(executed <= 1000, "infinite rotation loop");
            self.execute(action);
            executed += 1;
        }
        self.in_rotation = false;
        if executed == 0 && self.log.is_some() {
            self.player_log("No available actions!");
        }
        if self.player.rotation_timer <= self.now {
            // A moving unit does not wait for its GCD. Forever's rule (README, Rust-only
            // mechanics): neither does a rotation with a move, so a walk timed to a swing timer
            // starts when its condition turns true, as moving needs no GCD.
            let mut next = self.now + self.config.reaction;
            if !self.player.moving && !self.rotation_with_moves_looks_through_gcd {
                next = next.max(self.player.gcd);
            }
            self.wait_until(next);
        }
    }

    /// Go `APLRotation.reset` and each action's `Reset`.
    pub(crate) fn rotation_reset(&mut self, side: Side) {
        if side == Side::Player {
            // Go `Character.reset`: the current target is the first again.
            self.change_rotation_copy(0);
            self.in_rotation = false;
            self.apl.controlling = None;
            self.apl.move_held_for_queue = false;
            self.apl.interrupt_channel_if = None;
            self.apl.allow_channel_recast = false;
            self.apl.proofs.clear();
            self.apl.proofs.resize(self.rotation.len(), Proof::NONE);
            self.apl.look = None;
            for item in 0..self.rotation.len() {
                match self.rotation[item].action {
                    Act::StrictSequence {
                        ref mut next,
                        ref mut changes,
                        ..
                    } => {
                        *next = 0;
                        for (_, last) in changes.iter_mut().flatten() {
                            *last = NEVER_EXPIRES;
                        }
                        self.apl.queue_hooks.retain(|&hooked| hooked != item);
                        self.apl.in_sequence = false;
                    }
                    Act::Sequence { ref mut next, .. } => {
                        *next = 0;
                        self.apl.queue_hooks.retain(|&hooked| hooked != item);
                    }
                    Act::Schedule { ref mut next, .. } => *next = 0,
                    Act::ChangeTarget { ref mut last, .. } => *last = NEVER_EXPIRES,
                    Act::StrictMultidot {
                        ref mut next,
                        ref mut ready,
                        ..
                    } => {
                        *next = 0;
                        ready.clear();
                        self.apl.queue_hooks.retain(|&hooked| hooked != item);
                    }
                    _ => {}
                }
            }
        }
    }
}

fn compare<T: PartialOrd>(op: CompareOp, lhs: T, rhs: T) -> bool {
    match op {
        CompareOp::Eq => lhs == rhs,
        CompareOp::Ne => lhs != rhs,
        CompareOp::Lt => lhs < rhs,
        CompareOp::Le => lhs <= rhs,
        CompareOp::Gt => lhs > rhs,
        CompareOp::Ge => lhs >= rhs,
    }
}

fn compare_float(op: CompareOp, lhs: f64, rhs: f64) -> bool {
    compare(op, lhs, rhs)
}

/// Go's `max` builtin on floats: a NaN wins, and positive zero beats negative zero.
fn go_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a > b || (a == b && b.is_sign_negative()) {
        a
    } else {
        b
    }
}

/// Go's `min` builtin on floats: a NaN wins, and negative zero beats positive zero.
fn go_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a < b || (a == b && a.is_sign_negative()) {
        a
    } else {
        b
    }
}
