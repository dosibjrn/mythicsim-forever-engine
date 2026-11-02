//! Go `auraTracker` and `Aura` lifecycle.
//!
//! Callback lists are Go slices maintained by swap removal. Go iterates them with a
//! range loop over the slice header taken before the loop, so a removal or activation
//! during a callback changes which entries the loop still visits. [`CallbackList`]
//! keeps the backing array and logical length separately to reproduce that exactly.

use crate::{
    contracts::prepared_v2::{ActionId, Aura as ExportedAura},
    core::time::NEVER_EXPIRES,
};

use super::{
    log::action_string, metrics::Aggregator, Agent, DotId, Fight, Side, SpellId, SpellResult,
    TimerId,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct AuraRef {
    pub(crate) side: Side,
    pub(crate) index: usize,
}

/// The place of each stat aura in the stat mask, as the `stat_auras` effect numbers its
/// combinations: the first bit and the number of bits. An aura read as active or not takes one
/// bit; one whose stats follow its stacks takes the bits that count to its maximum stacks.
pub(crate) fn stat_aura_places(stacks: &[i32], auras: usize) -> Vec<(u32, u32)> {
    let mut offset = 0;
    (0..auras)
        .map(|index| {
            let width = match stacks.get(index).copied().unwrap_or(0) {
                most if most > 0 => u32::BITS - (most as u32).leading_zeros(),
                _ => 1,
            };
            offset += width;
            (offset - width, width)
        })
        .collect()
}

#[derive(Clone, Debug)]
pub(crate) enum AuraBehavior<K> {
    /// Stat or regeneration effects that the prepared values already include.
    Static,
    /// A listener that never acts in the supported scope.
    Inert,
    /// Go health.go `trackChanceOfDeath`'s listener on hits the player takes.
    ChanceOfDeath,
    /// Go attack.go `applyParryHaste`: a parry pulls the unit's next main hand swing in.
    ParryHaste,
    /// Go character.go's "Pushback trigger" on a tanking player, with its pushback chance.
    PushbackTrigger {
        chance: f64,
    },
    /// An item proc that restores energy: [`super::energy::EnergizeProc`], by index.
    EnergizeProc(usize),
    /// A proc on the player's casts that restores health a delay later: by its position in
    /// `Fight::health_on_cast`.
    HealthOnCast(usize),
    /// Go buffs/paladin.go `AttachJudgementOfLightHeal`, on the target: by its position in
    /// `Fight::judgements_of_light`.
    JudgementOfLight(usize),
    /// Go buffs/paladin.go `AttachJudgementOfWisdomMana`.
    JudgementOfWisdom {
        chance: f64,
        mana: f64,
        /// The judgement's action, which names the attacker's metrics at its first restore.
        id: ActionId,
        /// The player's metrics, shared by every Judgement of Wisdom.
        metrics: usize,
        delay: i64,
    },
    /// Go racials.go `applyTouchOfTheGrave`: a proc trigger that casts the drain.
    TouchOfTheGrave {
        chance: f64,
        delay: i64,
        drain: SpellId,
    },
    /// Go racials.go `applyEureka`'s aura.
    Eureka,
    /// Go `AttachMultiplyAttackSpeed` followed by `AttachMultiplyCastSpeed`, as Berserking.
    MultiplyAttackAndCastSpeed {
        attack: f64,
        cast: f64,
    },
    /// Battle Squawk: the melee speed each stack gives, taken back as the stacks fall to none.
    BattleSquawk {
        per_stack: f64,
    },
    /// Go `AttachHastePseudoStats`: melee, ranged and cast speed multipliers, each attached only
    /// when it changes speed, in that order.
    MultiplySpeeds {
        melee: f64,
        ranged: f64,
        cast: f64,
    },
    /// Go `MultiplyMovementSpeed` of the player while up, as Skysight's Elemental Blessing.
    MultiplyMovementSpeed(f64),
    /// An item or enchant speed aura an item swap gains and expires: as `MultiplySpeeds`, but
    /// the reset's gain is already in the exported speeds.
    SwapSpeed {
        melee: f64,
        ranged: f64,
        cast: f64,
    },
    /// Go `AttachMultiplicativePseudoStatBuff` on the player's damage taken of each school, as
    /// Orc Shatter Curse attaches it.
    MultiplySelfDamageTaken {
        multiplier: f64,
        schools: [bool; 8],
    },
    /// Go `MultiplyManaRegenSpeed` on gain and its reciprocal on expire, as racials.go
    /// Energized does with 2 and 0.5.
    MultiplyManaRegenSpeed(f64),
    /// Go `AttachInnervateRegen` on an aura that is not the druid's own, by its position in
    /// `Fight::innervate_regens`.
    InnervateRegen(usize),
    /// Go `NewTemporaryStatMultiplierAura`: the stats while active. Go recomputes every stat
    /// from the same inputs on each change, so expiry restores the prepared values exactly.
    TemporaryStats {
        /// The aura's bit in `Fight::stat_mask`.
        bit: u32,
        /// Lines in `Fight::aura_logs` logged before the stats change on gain and expiry.
        gain_log: Option<usize>,
        expire_log: Option<usize>,
    },
    /// Go `core.MakeStackingAura`: each stack adds the same stats, so the stat combination
    /// follows the aura's stacks, in the `width` bits of `Fight::stat_mask` from `offset`.
    StackingStats {
        offset: u32,
        width: u32,
        /// A stack added at once on gain and every period after it, for the ticks in all:
        /// Go `StartPeriodicAction` with `TickImmediately`.
        periodic: Option<(i64, i32)>,
    },
    /// An aura that adds to the attack tables' armor ignore factor while up, as Weaponmaster's
    /// mace aura, which the prepared value already holds at the reset.
    ArmorIgnoreFactor(f64),
    /// The Crusader enchant's trigger.
    Crusader,
    /// Dragon's Call's trigger, which summons the Emerald Dragon Whelp.
    EmeraldDragonWhelp,
    /// Force Reactive Disk's trigger, with the spell it casts.
    ForceReactiveDisk(SpellId),
    /// Thunderfury's weapon proc trigger.
    Thunderfury,
    /// Thunderfury's resistance aura on a target: its nature resistance changes by the amount
    /// while it is up.
    ThunderfuryResistance(f64),
    /// Sulfuras, Hand of Ragnaros's weapon proc trigger and its Immolation.
    SulfurasProc,
    SulfurasImmolation,
    /// Blade of Hanna's trigger.
    BladeOfHanna,
    /// The party Windfury Totem's totem aura, whose exclusive effect holds the trigger.
    WindfuryTotem,
    /// The party Windfury Totem's trigger.
    WindfuryTrigger,
    /// The party Windfury Totem's charges of attack power, which landed autos spend.
    WindfuryProc {
        bit: u32,
    },
    /// Dragonbreath Chili's trigger.
    DragonbreathChili,
    /// Revelation's hit and heal trigger, or its tick trigger.
    RevelationTrigger,
    /// Revelation's class buff.
    RevelationBuff,
    /// An item proc trigger on landed melee hits that grants extra main hand attacks.
    ExtraAttackProc {
        chance: f64,
        attacks: i32,
    },
    /// Go rage.go's "RageBar" aura: landed white hits give rage.
    RageBar,
    /// An item damage proc's trigger, by its position in `Fight::damage_procs`.
    SpellDataDamageProc(usize),
    /// An enchant heal proc's trigger, by its position in `Fight::heal_procs`.
    HealProc(usize),
    /// An absorb proc's trigger on the melee hits the player takes, by its position in
    /// `Fight::absorb_procs`.
    AbsorbProc(usize),
    /// A set bonus stat proc's trigger, by its position in `Fight::stat_procs`.
    StatProc(usize),
    /// A gear proc's trigger that heals and gives rage, by its position in
    /// `Fight::health_rage_procs`.
    HealthRageProc(usize),
    /// An energy proc of the gear procs, by its position.
    EnergyProc(usize),
    /// A gear proc's trigger that stacks an armor debuff on the target, by its position in
    /// `Fight::armor_debuff_procs`.
    ArmorDebuffTrigger(usize),
    /// That armor debuff on the target.
    ArmorDebuff(usize),
    /// A spell data stat proc's trigger, by its position in `Fight::spell_stat_procs`.
    SpellDataStatProc(usize),
    /// An aura whose spell mods, at this position in `Fight::aura_mods`, apply while active.
    SpellMods(usize),
    /// A listener on the player's casts that restores mana, by its position in
    /// `Fight::mana_on_cast`.
    ManaOnCast(usize),
    /// A healer's stacking on-use trinket's window, by its position in `Fight::stacking_cds`.
    StackingCdWindow(usize),
    /// A healer's on-use shield on heals, by its position in `Fight::shields_on_heal`.
    ShieldOnHeal(usize),
    /// A healer's instant cast aura, by its position in `Fight::instant_casts`.
    InstantCast(usize),
    /// The aura of a dot or channel.
    Dot(DotId),
    /// Go movement.go's Movement aura, which marks its unit moving.
    Movement,
    Class(K),
}

/// Go callback slice, by list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum List {
    Active = 0,
    ApplyEffects,
    CastComplete,
    SpellHitDealt,
    SpellHitTaken,
    PeriodicDamageDealt,
    PeriodicDamageTaken,
    EncounterStart,
    HealDealt,
}

const LISTS: usize = 9;

/// A Go slice's header as a range loop captures it: which array it reads and how far.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Snapshot {
    generation: usize,
    pub(crate) len: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CallbackList {
    /// The slice's array, as long as its capacity.
    backing: Vec<usize>,
    len: usize,
    /// How many times an append has outgrown the array. A range loop that began before one
    /// keeps reading the array it began with, which later swaps no longer change.
    generation: usize,
    /// The arrays an append outgrew, by generation.
    retired: Vec<Vec<usize>>,
}

impl CallbackList {
    fn clear(&mut self) {
        self.len = 0;
    }

    /// Go `append` of one element: a full array is replaced by one of twice the capacity (one,
    /// to begin with) holding a copy of the elements.
    fn push(&mut self, aura: usize) -> usize {
        if self.len == self.backing.len() {
            let capacity = (self.backing.len() * 2).max(1);
            let mut grown = vec![0; capacity];
            grown[..self.len].copy_from_slice(&self.backing[..self.len]);
            self.retired
                .push(std::mem::replace(&mut self.backing, grown));
            self.generation += 1;
        }
        self.backing[self.len] = aura;
        self.len += 1;
        self.len - 1
    }

    /// Go `removeBySwappingToBack`. Returns the aura moved into `index`, if any.
    fn swap_remove(&mut self, index: usize) -> Option<usize> {
        self.backing[index] = self.backing[self.len - 1];
        self.len -= 1;
        (index < self.len).then(|| self.backing[index])
    }

    /// The slice header a Go range loop would capture now.
    pub(crate) fn snapshot(&self) -> Snapshot {
        Snapshot {
            generation: self.generation,
            len: self.len,
        }
    }

    /// What a Go range loop reads at `index`, including stale entries past the length.
    pub(crate) fn read(&self, snapshot: Snapshot, index: usize) -> usize {
        if snapshot.generation == self.generation {
            self.backing[index]
        } else {
            self.retired[snapshot.generation][index]
        }
    }

    pub(crate) fn live(&self) -> &[usize] {
        &self.backing[..self.len]
    }
}

pub(crate) struct Aura<K> {
    pub(crate) label: String,
    /// Go `Aura.Tag`, which `HasActiveAuraWithTag` matches.
    pub(crate) tag: Option<String>,
    pub(crate) action_id: Option<ActionId>,
    /// Go `ActionIDForProc`: the proc a trigger aura stands for.
    pub(crate) proc_action_id: Option<ActionId>,
    /// Go `AuraMetrics.ID`, fixed at registration; a metric split retags `action_id` alone.
    pub(crate) metrics_id: Option<ActionId>,
    pub(crate) duration: i64,
    pub(crate) max_stacks: i32,
    pub(crate) behavior: AuraBehavior<K>,
    lists: [bool; LISTS],
    permanent: bool,
    /// The stacks an aura active after Go's reset holds, which its OnReset set as it activated.
    reset_stacks: i32,
    /// The later member of its exclusive category that displaces it during the reset.
    displaced_by: Option<String>,
    /// Blocked at the reset by an earlier member of its exclusive category.
    blocked_at_reset: bool,
    pub(crate) icd: Option<(TimerId, i64)>,
    pub(crate) active: bool,
    pub(crate) stacks: i32,
    /// When the stacks last changed and what they were before the change a reaction time
    /// older than this one: what `auraNumStacks` with `includeReactionTime` reads. Go keeps
    /// them in each such value's stack change handler, which every one of an aura's values
    /// runs alike, so the aura holds them.
    pub(crate) stack_update: i64,
    pub(crate) previous_stacks: i32,
    pub(crate) start: i64,
    pub(crate) expires: i64,
    pub(crate) fade_time: i64,
    positions: [Option<usize>; LISTS],
    pub(crate) uptime: i64,
    pub(crate) procs: i32,
    pub(crate) aggregate: Aggregator,
    pub(crate) procs_sum: i64,
}

impl<K> Aura<K> {
    pub(crate) fn remaining(&self, now: i64) -> i64 {
        if !self.active {
            0
        } else if self.expires == NEVER_EXPIRES {
            NEVER_EXPIRES
        } else {
            self.expires - now
        }
    }
}

pub(crate) struct Tracker<K> {
    pub(crate) auras: Vec<Aura<K>>,
    pub(crate) lists: [CallbackList; LISTS],
    pub(crate) min_expires: i64,
}

impl<K> Default for Tracker<K> {
    fn default() -> Self {
        Self {
            auras: Vec::new(),
            lists: Default::default(),
            min_expires: NEVER_EXPIRES,
        }
    }
}

impl<K> Tracker<K> {
    pub(crate) fn register(
        &mut self,
        exported: &ExportedAura,
        behavior: AuraBehavior<K>,
        icd: Option<(TimerId, i64)>,
    ) {
        let mut lists = [false; LISTS];
        for callback in &exported.callbacks {
            let list = match callback.as_str() {
                "on_apply_effects" => List::ApplyEffects,
                "on_cast_complete" => List::CastComplete,
                "on_spell_hit_dealt" => List::SpellHitDealt,
                "on_spell_hit_taken" => List::SpellHitTaken,
                "on_periodic_damage_dealt" => List::PeriodicDamageDealt,
                "on_periodic_damage_taken" => List::PeriodicDamageTaken,
                "on_encounter_start" => List::EncounterStart,
                "on_heal_dealt" => List::HealDealt,
                _ => continue,
            };
            lists[list as usize] = true;
        }
        self.auras.push(Aura {
            label: exported.label.clone(),
            tag: exported.tag.clone(),
            action_id: exported.action_id.clone(),
            proc_action_id: exported.action_id_for_proc.clone(),
            metrics_id: exported
                .action_id
                .clone()
                .filter(|_| !exported.metrics_hidden),
            duration: exported.duration_ns,
            max_stacks: exported.max_stacks,
            behavior,
            lists,
            // An aura active right after Go's reset was activated by its OnReset, as was one
            // a later member of its exclusive category displaced.
            permanent: exported.active || exported.displaced_by.is_some(),
            reset_stacks: if exported.active { exported.stacks } else { 0 },
            displaced_by: exported.displaced_by.clone(),
            blocked_at_reset: exported.blocked_at_reset,
            icd,
            active: false,
            stacks: 0,
            stack_update: NEVER_EXPIRES,
            previous_stacks: 0,
            start: 0,
            expires: 0,
            fade_time: -NEVER_EXPIRES,
            positions: [None; LISTS],
            uptime: 0,
            procs: 0,
            aggregate: Aggregator::default(),
            procs_sum: 0,
        });
    }

    /// Copies of the auras for another target, fresh as registered, with each behavior mapped
    /// to the copy's.
    pub(crate) fn copy_auras(
        &self,
        behavior: impl Fn(&AuraBehavior<K>) -> AuraBehavior<K>,
    ) -> Vec<Aura<K>> {
        self.auras
            .iter()
            .map(|aura| Aura {
                label: aura.label.clone(),
                tag: aura.tag.clone(),
                action_id: aura.action_id.clone(),
                proc_action_id: aura.proc_action_id.clone(),
                metrics_id: aura.metrics_id.clone(),
                duration: aura.duration,
                max_stacks: aura.max_stacks,
                behavior: behavior(&aura.behavior),
                lists: aura.lists,
                permanent: aura.permanent,
                reset_stacks: aura.reset_stacks,
                displaced_by: aura.displaced_by.clone(),
                blocked_at_reset: aura.blocked_at_reset,
                icd: aura.icd,
                active: false,
                stacks: 0,
                stack_update: NEVER_EXPIRES,
                previous_stacks: 0,
                start: 0,
                expires: 0,
                fade_time: -NEVER_EXPIRES,
                positions: [None; LISTS],
                uptime: 0,
                procs: 0,
                aggregate: Aggregator::default(),
                procs_sum: 0,
            })
            .collect()
    }

    /// Activate the aura at every reset, as a Go `OnReset` that activates it does.
    pub(crate) fn set_permanent(&mut self, index: usize) {
        self.auras[index].permanent = true;
    }

    /// Leave the aura to whatever activates it at reset, as an effect another aura's activation
    /// turns on, though the exported reset state shows it active.
    pub(crate) fn clear_permanent(&mut self, index: usize) {
        self.auras[index].permanent = false;
    }

    pub(crate) fn find(&self, label: &str) -> Option<usize> {
        self.auras.iter().position(|aura| aura.label == label)
    }

    /// Go `GetAuraByID`: the first aura with the same action, tag included.
    pub(crate) fn find_by_id(&self, id: &ActionId) -> Option<usize> {
        self.auras
            .iter()
            .position(|aura| aura.action_id.as_ref() == Some(id))
    }

    /// Go `GetIcdAuraByID`: the first aura with the action, or triggered by it, that has an
    /// ICD.
    pub(crate) fn find_icd_by_id(&self, id: &ActionId) -> Option<usize> {
        self.auras.iter().position(|aura| {
            aura.icd.is_some()
                && (aura.action_id.as_ref() == Some(id) || aura.proc_action_id.as_ref() == Some(id))
        })
    }

    fn add_to(&mut self, list: List, aura: usize) {
        let position = self.lists[list as usize].push(aura);
        self.auras[aura].positions[list as usize] = Some(position);
    }

    fn remove_from(&mut self, list: List, aura: usize) {
        if let Some(position) = self.auras[aura].positions[list as usize].take() {
            if let Some(moved) = self.lists[list as usize].swap_remove(position) {
                self.auras[moved].positions[list as usize] = Some(position);
            }
        }
    }
}

impl<A: Agent> Fight<A> {
    pub(crate) fn aura(&self, aura: AuraRef) -> &Aura<A::Aura> {
        &self.trackers[aura.side.index()].auras[aura.index]
    }

    /// A registered player aura by label.
    pub(crate) fn player_aura(&self, label: &str) -> Result<AuraRef, String> {
        self.trackers[Side::Player.index()]
            .find(label)
            .map(|index| AuraRef {
                side: Side::Player,
                index,
            })
            .ok_or_else(|| format!("player aura {label} is not registered"))
    }

    /// Go `HasActiveAuraWithTag` on the player.
    pub(crate) fn player_has_active_aura_with_tag(&self, tag: &str) -> bool {
        self.trackers[Side::Player.index()]
            .auras
            .iter()
            .any(|aura| aura.active && aura.tag.as_deref() == Some(tag))
    }

    pub(crate) fn aura_mut(&mut self, aura: AuraRef) -> &mut Aura<A::Aura> {
        &mut self.trackers[aura.side.index()].auras[aura.index]
    }

    /// Whether the aura reset activates the aura. An aura Go's agent reset activates later,
    /// as a druid's starting form, is active after the reset without being permanent.
    pub(crate) fn set_aura_permanent(&mut self, aura: AuraRef, permanent: bool) {
        self.aura_mut(aura).permanent = permanent;
    }

    fn unit_label(&self, side: Side) -> String {
        self.label_of(side)
    }

    /// Go `Aura.Refresh`.
    pub(crate) fn refresh_aura(&mut self, aura: AuraRef) {
        self.changed(super::reads::AURAS);
        let now = self.now;
        let tracker = &mut self.trackers[aura.side.index()];
        let state = &mut tracker.auras[aura.index];
        if state.duration == NEVER_EXPIRES {
            state.expires = NEVER_EXPIRES;
        } else {
            state.expires = now + state.duration;
            if state.expires < tracker.min_expires {
                tracker.min_expires = state.expires;
                let expires = state.expires;
                self.reschedule_tracker(expires);
            }
        }
    }

    /// Go `Aura.Activate`.
    pub(crate) fn activate_aura(&mut self, aura: AuraRef) {
        self.changed(super::reads::AURAS);
        self.aura_mut(aura).procs += 1;
        if self.aura(aura).active {
            if let Some(id) = self
                .aura(aura)
                .action_id
                .clone()
                .filter(|_| self.log.is_some())
            {
                let line = format!("Aura refreshed: {}", action_string(&id));
                self.unit_log(aura.side, &line);
            }
            self.refresh_aura(aura);
            return;
        }
        assert!(self.aura(aura).duration != 0, "aura with zero duration");
        // Go activates exclusive effects first: a stronger member of an exclusive category
        // blocks the activation, otherwise the effects' gains run.
        if !self.activate_exclusive(aura) {
            return;
        }
        self.track_exclusive_activate(aura);
        match self.aura(aura).behavior {
            AuraBehavior::Class(kind) => A::on_exclusive_gain(self, aura, kind),
            // The party Windfury Totem's effect turns its trigger on, after the air totem slot
            // has displaced any weaker air totem, unless a stronger effect holds its category.
            AuraBehavior::WindfuryTotem => {
                let windfury = self.windfury.as_ref().expect("Windfury Totem is bound");
                let (trigger, blocked) = (windfury.trigger, windfury.blocked);
                if !blocked && !self.aura(trigger).active {
                    self.activate_aura(trigger);
                }
            }
            _ => {}
        }
        {
            let now = self.now;
            let state = self.aura_mut(aura);
            state.active = true;
            state.start = now;
        }
        self.refresh_aura(aura);
        let tracker = &mut self.trackers[aura.side.index()];
        if tracker.auras[aura.index].duration != NEVER_EXPIRES {
            tracker.add_to(List::Active, aura.index);
        }
        for list in [
            List::ApplyEffects,
            List::CastComplete,
            List::SpellHitDealt,
            List::SpellHitTaken,
            List::PeriodicDamageDealt,
            List::PeriodicDamageTaken,
            List::EncounterStart,
            List::HealDealt,
        ] {
            if tracker.auras[aura.index].lists[list as usize] {
                tracker.add_to(list, aura.index);
            }
        }
        if let Some(id) = self
            .aura(aura)
            .action_id
            .clone()
            .filter(|_| self.log.is_some())
        {
            let line = format!("Aura gained: {}", action_string(&id));
            self.unit_log(aura.side, &line);
        }
        if !self.health_regens.is_empty() {
            self.health_regen_gained(aura);
        }
        self.on_gain(aura);
    }

    /// Go `Aura.Deactivate`.
    pub(crate) fn deactivate_aura(&mut self, aura: AuraRef) {
        self.changed(super::reads::AURAS);
        if !self.aura(aura).active {
            return;
        }
        // Go closes the exclusive effects' uptime before it clears the expiry.
        self.close_exclusive_uptime(aura);
        let now = self.now;
        let has_id = {
            let state = self.aura_mut(aura);
            state.active = false;
            state.action_id.is_some()
        };
        if has_id {
            let state = self.aura_mut(aura);
            let start = state.start.max(0);
            if now > state.expires {
                state.uptime += (state.expires - start).max(0);
            } else {
                state.uptime += (now - start).max(0);
            }
            if self.log.is_some() {
                let expires = self.aura(aura).expires;
                let id = self.aura(aura).action_id.clone().unwrap_or_default();
                let line = format!("Aura faded: {}", action_string(&id));
                let label = self.unit_label(aura.side);
                self.log_at(now.min(expires), &label, &line);
            }
        }
        {
            let state = self.aura_mut(aura);
            state.expires = 0;
            state.fade_time = now;
        }
        let tracker = &mut self.trackers[aura.side.index()];
        for list in [
            List::Active,
            List::ApplyEffects,
            List::CastComplete,
            List::SpellHitDealt,
            List::SpellHitTaken,
            List::PeriodicDamageDealt,
            List::PeriodicDamageTaken,
            List::EncounterStart,
            List::HealDealt,
        ] {
            tracker.remove_from(list, aura.index);
        }
        if self.aura(aura).stacks != 0 {
            self.set_stacks(aura, 0);
        }
        self.deactivate_exclusive(aura);
        self.track_exclusive_deactivate(aura);
        if let Side::Pet(_) = aura.side {
            self.pet_aura_expired(aura);
        }
        if !self.health_regens.is_empty() {
            self.health_regen_expired(aura);
        }
        self.on_expire(aura);
    }

    /// A tick of a periodic stack adder: one more stack, and the next tick a period away while
    /// ticks are left.
    pub(crate) fn stack_tick(&mut self, aura: AuraRef, period: i64, ticks_left: i32) {
        self.add_stack(aura);
        if ticks_left > 1 {
            self.schedule(
                self.now + period,
                super::PRIORITY_AUTO,
                super::Action::StackTick {
                    aura,
                    period,
                    ticks_left: ticks_left - 1,
                },
            );
        }
    }

    /// Go `Aura.SetStacks`.
    pub(crate) fn set_stacks(&mut self, aura: AuraRef, stacks: i32) {
        self.changed(super::reads::AURAS);
        let state = self.aura(aura);
        assert!(state.active || stacks == 0, "stacks on an inactive aura");
        assert!(state.max_stacks != 0, "stacks on an aura without MaxStacks");
        let old = state.stacks;
        let new = stacks.min(state.max_stacks).max(0);
        if old == new {
            return;
        }
        if let Some(id) = state.action_id.clone().filter(|_| self.log.is_some()) {
            let line = format!("{} stacks: {old} --> {new}", action_string(&id));
            self.unit_log(aura.side, &line);
        }
        {
            // Go `APLValueAuraNumStacks`'s stack change handler: the stacks before a change a
            // reaction time after the last one are the ones a reaction-time read falls back to.
            let (now, reaction) = (self.now, self.config.reaction);
            let state = self.aura_mut(aura);
            if now.wrapping_sub(state.stack_update) >= reaction {
                state.previous_stacks = old;
            }
            state.stack_update = now;
            state.stacks = new;
        }
        match self.aura(aura).behavior {
            AuraBehavior::Class(kind) => A::on_stacks_change(self, aura, kind, old, new),
            AuraBehavior::ArmorDebuff(proc) => {
                self.armor_debuff_stacks_changed(proc, aura.side, old, new)
            }
            // Go `MakeStackingAura`'s OnStacksChange adds the stats of the stacks gained.
            AuraBehavior::StackingStats { offset, width, .. } => {
                let field = ((1u32 << width) - 1) << offset;
                self.set_stat_mask((self.stat_mask & !field) | ((new as u32) << offset));
            }
            // The reset's stacks are in the exported melee speed; their fall to none takes it
            // back.
            AuraBehavior::BattleSquawk { per_stack } if new == 0 => {
                self.multiply_melee_speed(per_stack.powi(-old));
            }
            _ => {}
        }
        self.exclusive_stacks_changed(aura, new);
        if self.aura(aura).stacks == 0 {
            self.deactivate_aura(aura);
        }
    }

    pub(crate) fn add_stack(&mut self, aura: AuraRef) {
        let stacks = self.aura(aura).stacks + 1;
        self.set_stacks(aura, stacks);
    }

    pub(crate) fn remove_stack(&mut self, aura: AuraRef) {
        let stacks = (self.aura(aura).stacks - 1).max(0);
        self.set_stacks(aura, stacks);
    }

    /// Go `AddStatsDynamic` for a stat aura a class aura owns: the player's stats become the
    /// combination with the aura's bit set or cleared.
    pub(crate) fn set_stat_aura(&mut self, bit: u32, active: bool) {
        let mask = if active {
            self.stat_mask | bit
        } else {
            self.stat_mask & !bit
        };
        self.set_stat_mask(mask);
    }

    /// A stacking stat aura's place in the active stat mask, by label: its offset and width.
    pub(crate) fn stat_aura_place(
        effects: &[crate::contracts::prepared_v2::Effect],
        label: &str,
    ) -> Option<(u32, u32)> {
        effects.iter().find_map(|effect| match effect {
            crate::contracts::prepared_v2::Effect::StatAuras { auras, stacks, .. } => {
                let position = auras.iter().position(|aura| aura == label)?;
                Some(stat_aura_places(stacks, auras.len())[position])
            }
            _ => None,
        })
    }

    /// A stat aura's bit in the active stat mask, by label: the first bit of its place.
    pub(crate) fn stat_aura_bit(
        effects: &[crate::contracts::prepared_v2::Effect],
        label: &str,
    ) -> Option<u32> {
        effects.iter().find_map(|effect| match effect {
            crate::contracts::prepared_v2::Effect::StatAuras { auras, stacks, .. } => {
                let position = auras.iter().position(|aura| aura == label)?;
                Some(1 << stat_aura_places(stacks, auras.len())[position].0)
            }
            _ => None,
        })
    }

    fn on_gain(&mut self, aura: AuraRef) {
        self.multiply_damage_taken_for(aura, false);
        if aura.side == Side::Player && !self.fixed_uptime.is_empty() {
            self.fixed_shout_chain_gain(aura);
        }
        match self.aura(aura).behavior {
            AuraBehavior::Dot(dot) => self.dot_on_gain(dot),
            AuraBehavior::RevelationBuff => self.revelation_buff_changed(true),
            AuraBehavior::Movement => self.movement_changed(aura.side, true),
            AuraBehavior::MultiplyMovementSpeed(speed) => self.multiply_movement_speed(speed),
            AuraBehavior::Eureka => self.eureka_gain(),
            AuraBehavior::MultiplyAttackAndCastSpeed { attack, cast } => {
                self.multiply_attack_speed(attack);
                self.multiply_cast_speed(cast);
            }
            AuraBehavior::MultiplySpeeds {
                melee,
                ranged,
                cast,
            } => self.multiply_speeds(melee, ranged, cast, false),
            AuraBehavior::SwapSpeed {
                melee,
                ranged,
                cast,
            } => {
                if !self.resetting_auras {
                    self.multiply_speeds(melee, ranged, cast, false);
                }
            }
            AuraBehavior::MultiplySelfDamageTaken {
                multiplier,
                schools,
            } => self.multiply_self_damage_taken(multiplier, schools, false),
            AuraBehavior::MultiplyManaRegenSpeed(multiplier) => {
                self.multiply_mana_regen_speed(multiplier)
            }
            AuraBehavior::InnervateRegen(index) => self.innervate_regen_gained(index),
            AuraBehavior::WindfuryProc { bit } => self.set_stat_mask(self.stat_mask | bit),
            AuraBehavior::StackingStats {
                periodic: Some((period, ticks)),
                ..
            } => {
                // Go `NewPeriodicAction`: at time zero, which may be the reset, the first tick
                // waits in the queue; otherwise it runs inside the gain.
                if self.now == 0 {
                    self.schedule(
                        0,
                        super::PRIORITY_AUTO,
                        super::Action::StackTick {
                            aura,
                            period,
                            ticks_left: ticks,
                        },
                    );
                } else {
                    self.stack_tick(aura, period, ticks);
                }
            }
            AuraBehavior::ThunderfuryResistance(delta) => {
                self.thunderfury_resistance(aura.side, delta)
            }
            AuraBehavior::SpellMods(index) => {
                for position in 0..self.aura_mods[index].len() {
                    self.activate_mod(self.aura_mods[index][position]);
                }
            }
            AuraBehavior::StackingCdWindow(index) => self.stacking_cd_gained(index),
            AuraBehavior::InstantCast(index) => self.instant_cast_gained(index),
            AuraBehavior::ArmorIgnoreFactor(delta) => {
                if !self.resetting_auras {
                    self.config.melee.armor_ignore_factor += delta;
                }
            }
            AuraBehavior::TemporaryStats { bit, gain_log, .. } => {
                if let (Some(line), true) = (gain_log, self.log.is_some()) {
                    let line = self.aura_logs[line].clone();
                    self.player_log(&line);
                }
                self.set_stat_mask(self.stat_mask | bit);
            }
            AuraBehavior::Class(kind) => A::on_gain(self, aura, kind),
            _ => {}
        }
    }

    fn on_expire(&mut self, aura: AuraRef) {
        self.multiply_damage_taken_for(aura, true);
        match self.aura(aura).behavior {
            AuraBehavior::Dot(dot) => {
                self.dot_on_expire(dot);
                self.revelation_dot_expired(dot);
            }
            AuraBehavior::RevelationBuff => self.revelation_buff_changed(false),
            AuraBehavior::Movement => self.movement_changed(aura.side, false),
            AuraBehavior::MultiplyMovementSpeed(speed) => self.multiply_movement_speed(1.0 / speed),
            AuraBehavior::Eureka => self.eureka_expire(),
            AuraBehavior::MultiplyAttackAndCastSpeed { attack, cast } => {
                self.multiply_attack_speed(1.0 / attack);
                self.multiply_cast_speed(1.0 / cast);
            }
            AuraBehavior::MultiplySpeeds {
                melee,
                ranged,
                cast,
            }
            | AuraBehavior::SwapSpeed {
                melee,
                ranged,
                cast,
            } => self.multiply_speeds(melee, ranged, cast, true),
            AuraBehavior::MultiplySelfDamageTaken {
                multiplier,
                schools,
            } => self.multiply_self_damage_taken(multiplier, schools, true),
            AuraBehavior::WindfuryProc { bit } => self.set_stat_mask(self.stat_mask & !bit),
            AuraBehavior::ThunderfuryResistance(delta) => {
                self.thunderfury_resistance(aura.side, -delta)
            }
            AuraBehavior::ArmorIgnoreFactor(delta) => {
                self.config.melee.armor_ignore_factor -= delta;
            }
            AuraBehavior::SpellMods(index) => {
                for position in 0..self.aura_mods[index].len() {
                    self.deactivate_mod(self.aura_mods[index][position]);
                }
            }
            AuraBehavior::StackingCdWindow(index) => self.stacking_cd_expired(index),
            AuraBehavior::InstantCast(index) => self.instant_cast_expired(index),
            AuraBehavior::WindfuryTotem => {
                let trigger = self
                    .windfury
                    .as_ref()
                    .expect("Windfury Totem is bound")
                    .trigger;
                self.deactivate_aura(trigger);
            }
            AuraBehavior::TemporaryStats {
                bit, expire_log, ..
            } => {
                if let (Some(line), true) = (expire_log, self.log.is_some()) {
                    let line = self.aura_logs[line].clone();
                    self.player_log(&line);
                }
                self.set_stat_mask(self.stat_mask & !bit);
            }
            AuraBehavior::MultiplyManaRegenSpeed(multiplier) => {
                self.multiply_mana_regen_speed(1.0 / multiplier)
            }
            AuraBehavior::InnervateRegen(index) => self.innervate_regen_expired(index),
            AuraBehavior::Class(kind) => A::on_expire(self, aura, kind),
            _ => {}
        }
    }

    /// Go `AttachProcTriggerCallback` for an extra attack item: landed melee hits other than
    /// procs, the cooldown, the chance roll, then the extra attacks at once.
    fn extra_attack_proc(
        &mut self,
        aura: AuraRef,
        spell: SpellId,
        result: &SpellResult,
        chance: f64,
        attacks: i32,
    ) {
        let state = &self.spells[spell];
        if state.flags.proc || !state.melee_proc || result.outcome & super::OUTCOME_LANDED == 0 {
            return;
        }
        let icd = self.aura(aura).icd;
        if let Some((timer, _)) = icd {
            if self.timers[timer] > self.now {
                return;
            }
        }
        if chance != 1.0 && self.random_for_aura(aura) > chance {
            return;
        }
        if let Some((timer, duration)) = icd {
            self.timers[timer] = self.now + duration;
        }
        self.extra_mh_attacks(attacks);
    }

    /// Go `AttachMultiplicativePseudoStatBuff` on the player's damage taken multiplier. Its
    /// callbacks join the aura's others; nothing reads the multiplier between them.
    fn multiply_damage_taken_for(&mut self, aura: AuraRef, expire: bool) {
        if aura.side != Side::Player {
            return;
        }
        if self.resetting_auras {
            return;
        }
        for position in 0..self.damage_taken_auras.len() {
            let (index, multiplier, stat) = self.damage_taken_auras[position];
            if index == aura.index {
                let value = match stat {
                    super::PseudoStat::DamageTaken => &mut self.player.damage_taken_multiplier,
                    super::PseudoStat::Threat => &mut self.player.threat_multiplier,
                    super::PseudoStat::DamageDealt => &mut self.player.damage_dealt_multiplier,
                };
                if expire {
                    *value /= multiplier;
                } else {
                    *value *= multiplier;
                }
            }
        }
    }

    /// Go `auraTracker.reset`: reset effects ran before this; then each aura resets in
    /// registration order and permanent auras activate.
    pub(crate) fn reset_auras(&mut self, side: Side) {
        let tracker = &mut self.trackers[side.index()];
        for list in &mut tracker.lists {
            list.clear();
        }
        tracker.min_expires = NEVER_EXPIRES;
        // Few units have an aura another displaces at reset; the rest skip the search.
        let displacing = tracker.auras.iter().any(|aura| aura.displaced_by.is_some());
        for index in 0..self.trackers[side.index()].auras.len() {
            let state = &mut self.trackers[side.index()].auras[index];
            assert!(
                !state.active && state.stacks == 0,
                "aura active during reset"
            );
            state.uptime = 0;
            state.procs = 0;
            state.fade_time = -NEVER_EXPIRES;
            // Go's `OnReset` of a stack read: the stacks are zero, and no change has been seen.
            state.stack_update = NEVER_EXPIRES;
            state.previous_stacks = 0;
            if state.permanent {
                // Go ExclusiveEffect.Activate: a stronger later member of the category
                // deactivates the earlier one before it activates.
                if displacing {
                    let label = state.label.clone();
                    for other in 0..index {
                        let displaced = &self.trackers[side.index()].auras[other];
                        if displaced.active && displaced.displaced_by.as_deref() == Some(&label) {
                            self.displace_aura(AuraRef { side, index: other });
                        }
                    }
                }
                self.resetting_auras = true;
                self.activate_aura(AuraRef { side, index });
                self.resetting_auras = false;
                let stacks = self.trackers[side.index()].auras[index].reset_stacks;
                if stacks > 0 {
                    self.set_stacks(AuraRef { side, index }, stacks);
                }
            } else if state.blocked_at_reset {
                // Go Aura.Activate counts the proc before the exclusive effect blocks it.
                state.procs += 1;
            }
        }
    }

    /// Go `ExclusiveEffect.Deactivate` of a displaced aura's effect: its OnExpire, which for
    /// the party Windfury Totem turns the trigger off before taking the totem down.
    fn displace_aura(&mut self, aura: AuraRef) {
        if let AuraBehavior::WindfuryTotem = self.aura(aura).behavior {
            let trigger = self
                .windfury
                .as_ref()
                .expect("Windfury Totem is bound")
                .trigger;
            self.deactivate_aura(trigger);
        }
        self.deactivate_aura(aura);
    }

    /// Go `auraTracker.tryAdvance` and `advance`.
    pub(crate) fn try_advance_tracker(&mut self, side: Side) -> i64 {
        if self.now < self.trackers[side.index()].min_expires {
            return self.trackers[side.index()].min_expires;
        }
        loop {
            let tracker = &self.trackers[side.index()];
            let mut min = NEVER_EXPIRES;
            let mut expired = None;
            for &index in tracker.lists[List::Active as usize].live() {
                let expires = tracker.auras[index].expires;
                if expires <= self.now {
                    expired = Some(index);
                    break;
                }
                min = min.min(expires);
            }
            // Deactivation edits the list, so scan again from the start, as Go does.
            match expired {
                Some(index) => {
                    let aura = AuraRef { side, index };
                    self.expiring_by_duration = Some(aura);
                    self.deactivate_aura(aura);
                    self.expiring_by_duration = None;
                }
                None => {
                    self.trackers[side.index()].min_expires = min;
                    return min;
                }
            }
        }
    }

    /// Go `AttachHastePseudoStats`'s gain or expiry: each speed it attached, multiplied by its
    /// multiplier or its reciprocal.
    fn multiply_speeds(&mut self, melee: f64, ranged: f64, cast: f64, expire: bool) {
        let factor = |multiplier: f64| if expire { 1.0 / multiplier } else { multiplier };
        if melee != 1.0 {
            self.multiply_melee_speed(factor(melee));
        }
        if ranged != 1.0 {
            self.multiply_ranged_speed(factor(ranged));
        }
        if cast != 1.0 {
            self.multiply_cast_speed(factor(cast));
        }
    }

    /// Go `APLActionActivateAura.Execute`: an aura whose cooldown is not ready only logs so;
    /// otherwise it activates and its cooldown starts.
    /// Go `APLActionActivateAuraWithStacks.Execute`: not while its ICD runs, then the aura
    /// with its stacks.
    pub(crate) fn activate_aura_with_stacks_action(&mut self, aura: AuraRef, stacks: i32) {
        let id = self.aura(aura).action_id.clone().unwrap_or_default();
        if let Some((timer, _)) = self.aura(aura).icd {
            if self.timers[timer] > self.now {
                if self.log.is_some() {
                    let line = format!(
                        "Could not activate aura {} ({stacks} stacks) because it's not ready",
                        super::log::action_string(&id)
                    );
                    self.unit_log(aura.side, &line);
                }
                return;
            }
        }
        if self.log.is_some() {
            let line = format!(
                "Activating aura {} ({stacks} stacks)",
                super::log::action_string(&id)
            );
            self.unit_log(aura.side, &line);
        }
        self.activate_aura(aura);
        self.set_stacks(aura, stacks);
    }

    pub(crate) fn activate_aura_action(&mut self, aura: AuraRef) {
        let id = self.aura(aura).action_id.clone().unwrap_or_default();
        let icd = self.aura(aura).icd;
        if let Some((timer, _)) = icd {
            if self.timers[timer] > self.now {
                if self.log.is_some() {
                    let line = format!(
                        "Could not activate aura {} because it's not ready",
                        super::log::action_string(&id)
                    );
                    self.unit_log(aura.side, &line);
                }
                return;
            }
        }
        if self.log.is_some() {
            let line = format!("Activating aura {}", super::log::action_string(&id));
            self.unit_log(aura.side, &line);
        }
        self.activate_aura(aura);
        if let Some((timer, duration)) = icd {
            self.timers[timer] = self.now + duration;
            self.changed(super::reads::TIMERS);
        }
    }

    /// Whether `aura` is fading now because its duration ran out, as against a deactivation
    /// from outside: a dot recast, a consume, the iteration's end. Only true inside the aura's
    /// own expiry callbacks.
    pub(crate) fn fading_by_duration(&self, aura: AuraRef) -> bool {
        self.expiring_by_duration == Some(aura)
    }

    /// Go `auraTracker.expireAll`: deactivate the active auras, first listed first.
    pub(crate) fn expire_all_auras(&mut self, side: Side) {
        loop {
            let tracker = &self.trackers[side.index()];
            let Some(&index) = tracker.lists[List::Active as usize].live().first() else {
                break;
            };
            self.deactivate_aura(AuraRef { side, index });
        }
        self.trackers[side.index()].min_expires = NEVER_EXPIRES;
    }

    /// Go `auraTracker.doneIteration`: deactivate every aura in registration order, then
    /// fold this iteration's uptime and procs into the aggregates.
    pub(crate) fn aura_done_iteration(&mut self, side: Side) {
        self.done_iteration = true;
        'restart: loop {
            for index in 0..self.trackers[side.index()].auras.len() {
                if self.trackers[side.index()].auras[index].active {
                    self.deactivate_aura(AuraRef { side, index });
                    continue 'restart;
                }
            }
            break;
        }
        self.done_iteration = false;
        for state in &mut self.trackers[side.index()].auras {
            state.start = 0;
            state.expires = 0;
            state
                .aggregate
                .add(crate::core::time::seconds(state.uptime));
            state.procs_sum += i64::from(state.procs);
        }
    }

    /// Go `auraTracker.OnCastComplete` on the caster. No active check, as in Go.
    pub(crate) fn on_cast_complete(&mut self, spell: SpellId) {
        let list = List::CastComplete as usize;
        let side = self.spells[spell].caster;
        let snapshot = self.trackers[side.index()].lists[list].snapshot();
        for position in 0..snapshot.len {
            let index = self.trackers[side.index()].lists[list].read(snapshot, position);
            let aura = AuraRef { side, index };
            match self.aura(aura).behavior {
                AuraBehavior::Class(kind) => A::on_cast_complete(self, aura, kind, spell),
                AuraBehavior::RevelationBuff => self.revelation_cast_complete(spell),
                // Go `AttachProcTriggerCallback`'s OnCastComplete: the spells it hears, no chance
                // to roll, then the handler a delay later. A cast carries no result.
                AuraBehavior::HealthOnCast(index) if self.health_on_cast[index].spells[spell] => {
                    let result = SpellResult {
                        armor_multiplier: 0.0,
                        target: Side::Target,
                        attacker: Side::Player,
                        outcome: 0,
                        damage: 0.0,
                        threat: 0.0,
                    };
                    let delay = self.health_on_cast[index].delay;
                    self.schedule_delayed_proc_at(self.now + delay, aura, spell, result);
                }
                AuraBehavior::SpellDataStatProc(proc) if self.spell_stat_procs[proc].casts => {
                    self.spell_stat_proc_callback(aura, proc, Some(spell), None)
                }
                AuraBehavior::ManaOnCast(index) => self.mana_on_cast_callback(aura, index, spell),
                AuraBehavior::InstantCast(index) => self.instant_cast_spent(aura, index, spell),
                AuraBehavior::HealProc(proc) if self.heal_procs[proc].casts => {
                    let result = SpellResult {
                        armor_multiplier: 0.0,
                        target: Side::Player,
                        attacker: Side::Player,
                        outcome: 0,
                        damage: 0.0,
                        threat: 0.0,
                    };
                    self.heal_proc_callback(aura, proc, Some(spell), &result)
                }
                AuraBehavior::StackingCdWindow(index) => self.stacking_cd_callback(
                    aura,
                    index,
                    spell,
                    super::healer_gear::StackEvent::Cast,
                    None,
                ),
                AuraBehavior::SpellDataDamageProc(proc) if self.damage_procs[proc].casts => {
                    // A cast carries no result: the proc answers the current target.
                    let result = SpellResult {
                        armor_multiplier: 0.0,
                        target: self.current_target_side(),
                        attacker: Side::Player,
                        outcome: 0,
                        damage: 0.0,
                        threat: 0.0,
                    };
                    self.damage_proc_callback(aura, proc, Some(spell), &result)
                }
                _ => {}
            }
        }
    }

    /// Go `auraTracker.OnApplyEffects` on the caster. No active check, as in Go.
    pub(crate) fn on_apply_effects(&mut self, spell: SpellId, target: Side) {
        let list = List::ApplyEffects as usize;
        let snapshot = self.trackers[Side::Player.index()].lists[list].snapshot();
        for position in 0..snapshot.len {
            let index = self.trackers[Side::Player.index()].lists[list].read(snapshot, position);
            let aura = AuraRef {
                side: Side::Player,
                index,
            };
            if let AuraBehavior::Class(kind) = self.aura(aura).behavior {
                A::on_apply_effects(self, aura, kind, spell, target);
            }
        }
    }

    /// Go `auraTracker.OnPeriodicDamageDealt` on the caster, which skips no inactive aura, then
    /// `OnPeriodicDamageTaken` on the target for its class auras (Vampiric Embrace).
    pub(crate) fn on_periodic_damage(&mut self, spell: SpellId, result: &SpellResult) {
        let side = self.spells[spell].caster;
        let list = List::PeriodicDamageDealt as usize;
        let snapshot = self.trackers[side.index()].lists[list].snapshot();
        for position in 0..snapshot.len {
            let index = self.trackers[side.index()].lists[list].read(snapshot, position);
            let aura = AuraRef { side, index };
            match self.aura(aura).behavior {
                AuraBehavior::Class(kind) => {
                    A::on_periodic_damage_dealt(self, aura, kind, spell, result)
                }
                AuraBehavior::RevelationTrigger => self.revelation_callback(aura, spell, result),
                _ => {}
            }
        }
        // Go `OnPeriodicDamageTaken` on the target, for the class auras that listen to it.
        let side = result.target;
        let list = List::PeriodicDamageTaken as usize;
        let snapshot = self.trackers[side.index()].lists[list].snapshot();
        for position in 0..snapshot.len {
            let index = self.trackers[side.index()].lists[list].read(snapshot, position);
            let aura = AuraRef { side, index };
            if !self.aura(aura).active {
                continue;
            }
            if let AuraBehavior::Class(kind) = self.aura(aura).behavior {
                A::on_periodic_damage_taken(self, aura, kind, spell, result);
            }
        }
    }

    /// Go `auraTracker.OnHealDealt` on the caster, for the class auras that listen to heals.
    pub(crate) fn on_heal_dealt(&mut self, spell: SpellId, result: &SpellResult) {
        let side = self.spells[spell].caster;
        let list = List::HealDealt as usize;
        let snapshot = self.trackers[side.index()].lists[list].snapshot();
        for position in 0..snapshot.len {
            let index = self.trackers[side.index()].lists[list].read(snapshot, position);
            let aura = AuraRef { side, index };
            if !self.aura(aura).active {
                continue;
            }
            match self.aura(aura).behavior {
                AuraBehavior::Class(kind) => A::on_heal_dealt(self, aura, kind, spell, result),
                AuraBehavior::RevelationTrigger => self.revelation_callback(aura, spell, result),
                AuraBehavior::SpellDataStatProc(proc) if self.spell_stat_procs[proc].heals => {
                    self.spell_stat_proc_callback(aura, proc, Some(spell), Some(result))
                }
                AuraBehavior::StackingCdWindow(index) => self.stacking_cd_callback(
                    aura,
                    index,
                    spell,
                    super::healer_gear::StackEvent::Heal,
                    Some(result),
                ),
                AuraBehavior::ShieldOnHeal(index) => self.shield_on_heal(index, spell, result),
                AuraBehavior::ManaOnCast(index) => {
                    self.mana_on_crit_callback(aura, index, spell, result)
                }
                _ => {}
            }
        }
    }

    /// Go `auraTracker.OnSpellHitDealt` on the caster and `OnSpellHitTaken` on the target.
    pub(crate) fn on_spell_hit(&mut self, spell: SpellId, result: &SpellResult) {
        let caster = self.spells[spell].caster;
        for (side, list) in [
            (caster, List::SpellHitDealt),
            (result.target, List::SpellHitTaken),
        ] {
            // Listeners of the caster's hits, as opposed to the hits its target takes.
            let dealt = list == List::SpellHitDealt;
            let list = list as usize;
            let snapshot = self.trackers[side.index()].lists[list].snapshot();
            for position in 0..snapshot.len {
                let index = self.trackers[side.index()].lists[list].read(snapshot, position);
                let aura = AuraRef { side, index };
                if !self.aura(aura).active {
                    continue;
                }
                // Every listener hears the one result object, as it is when its turn comes.
                let heard = A::dealing_result(self, spell).unwrap_or(*result);
                let result = &heard;
                match self.aura(aura).behavior.clone() {
                    AuraBehavior::Class(kind) if dealt => {
                        A::on_spell_hit_dealt(self, aura, kind, spell, result)
                    }
                    AuraBehavior::Class(kind) => {
                        A::on_spell_hit_taken(self, aura, kind, spell, result)
                    }
                    AuraBehavior::RageBar if dealt => self.rage_bar_hit_dealt(spell, result),
                    AuraBehavior::ExtraAttackProc { chance, attacks } if dealt => {
                        self.extra_attack_proc(aura, spell, result, chance, attacks)
                    }
                    AuraBehavior::RageBar if side == Side::Player => {
                        self.rage_bar_hit_taken(result)
                    }
                    AuraBehavior::JudgementOfWisdom { chance, delay, .. } => {
                        self.judgement_of_wisdom_callback(aura, spell, result, chance, delay)
                    }
                    AuraBehavior::JudgementOfLight(index) if !dealt => {
                        self.judgement_of_light_callback(aura, index, spell, result)
                    }
                    AuraBehavior::TouchOfTheGrave { chance, delay, .. } if dealt => {
                        self.touch_of_the_grave_callback(aura, spell, result, chance, delay)
                    }
                    AuraBehavior::WindfuryTrigger if dealt => {
                        self.windfury_trigger(aura, spell, result)
                    }
                    AuraBehavior::WindfuryProc { .. } if dealt => {
                        let windfury = self.windfury.as_ref().expect("Windfury Totem is bound");
                        // The charges' own trigger: a landed auto spends one, at once.
                        if windfury.spend_spells[spell]
                            && result.outcome & super::OUTCOME_LANDED != 0
                            && !(windfury.spend_require_damage && result.damage == 0.0)
                        {
                            self.remove_stack(aura);
                        }
                    }
                    AuraBehavior::Crusader if dealt => self.crusader_callback(aura, spell, result),
                    AuraBehavior::EmeraldDragonWhelp if dealt => {
                        self.whelp_callback(aura, spell, result)
                    }
                    AuraBehavior::BladeOfHanna if dealt => {
                        self.blade_of_hanna_callback(spell, result)
                    }
                    AuraBehavior::SulfurasProc if dealt => {
                        self.sulfuras_callback(aura, spell, result)
                    }
                    AuraBehavior::Thunderfury if dealt => {
                        self.thunderfury_callback(aura, spell, result)
                    }

                    AuraBehavior::DragonbreathChili if dealt => {
                        self.chili_callback(aura, spell, result)
                    }
                    AuraBehavior::RevelationTrigger if dealt => {
                        self.revelation_callback(aura, spell, result)
                    }
                    AuraBehavior::SpellDataDamageProc(proc)
                        if dealt != self.damage_procs[proc].struck
                            && !self.damage_procs[proc].casts =>
                    {
                        self.damage_proc_callback(aura, proc, Some(spell), result)
                    }
                    AuraBehavior::HealProc(proc) if dealt && !self.heal_procs[proc].casts => {
                        self.heal_proc_callback(aura, proc, Some(spell), result)
                    }
                    AuraBehavior::HealthRageProc(proc) if dealt => {
                        self.health_rage_proc_callback(aura, proc, spell, result)
                    }
                    AuraBehavior::EnergyProc(proc) if dealt => {
                        self.energy_proc_callback(aura, proc, spell, result)
                    }
                    AuraBehavior::ArmorDebuffTrigger(proc) if dealt => {
                        self.armor_debuff_proc_callback(aura, proc, spell, result)
                    }
                    AuraBehavior::SpellDataStatProc(proc)
                        if dealt && self.spell_stat_procs[proc].hits =>
                    {
                        self.spell_stat_proc_callback(aura, proc, Some(spell), Some(result))
                    }
                    // The Goblin Sapper Charge's hit on its thrower, heard as a hit taken.
                    AuraBehavior::SpellDataStatProc(proc)
                        if !dealt && self.spell_stat_procs[proc].taken =>
                    {
                        self.spell_stat_proc_callback(aura, proc, Some(spell), Some(result))
                    }
                    AuraBehavior::StatProc(proc) if dealt => {
                        // Go AttachProcTriggerCallback: landed hits the manager hears, its roll
                        // under the trigger's name, then the handler a batch window later.
                        if result.outcome & super::OUTCOME_LANDED == 0 {
                            continue;
                        }
                        let Some(chance) = self.stat_procs[proc].chances[spell] else {
                            continue;
                        };
                        // Go `Proc(chance, label)`, reading the label in place.
                        if self.rng.proc(chance, &self.stat_procs[proc].rng_label) {
                            self.schedule_delayed_proc(aura, spell, *result);
                        }
                    }
                    AuraBehavior::ChanceOfDeath if !dealt && side == Side::Player => {
                        self.chance_of_death_hit_taken(result)
                    }
                    AuraBehavior::ParryHaste if !dealt => self.parry_haste(side, result),
                    AuraBehavior::EnergizeProc(index) if dealt => {
                        self.energize_proc_callback(aura, index, spell, result)
                    }
                    AuraBehavior::StackingCdWindow(index) if dealt => self.stacking_cd_callback(
                        aura,
                        index,
                        spell,
                        super::healer_gear::StackEvent::Hit,
                        Some(result),
                    ),
                    _ => {}
                }
            }
        }
    }

    /// Go `AttachProcTriggerCallback` with `ProcMaskDirect` and no outcome filter, delayed by
    /// the spell batch window.
    fn judgement_of_wisdom_callback(
        &mut self,
        aura: AuraRef,
        spell: SpellId,
        result: &SpellResult,
        chance: f64,
        delay: i64,
    ) {
        let spell_state = &self.spells[spell];
        if spell_state.flags.proc || !spell_state.direct_proc {
            return;
        }
        if chance != 1.0 && self.random_for_aura(aura) > chance {
            return;
        }
        self.schedule_delayed_proc_at(self.now + delay, aura, spell, *result);
    }

    /// Go `AttachJudgementOfLightHeal`'s trigger on the judged target: a landed melee hit that
    /// is no proc runs the handler a spell batch window later, with no chance of its own.
    fn judgement_of_light_callback(
        &mut self,
        aura: AuraRef,
        index: usize,
        spell: SpellId,
        result: &SpellResult,
    ) {
        let state = &self.spells[spell];
        if state.flags.proc || !state.melee_proc || result.outcome & super::OUTCOME_LANDED == 0 {
            return;
        }
        let delay = self.judgements_of_light[index].delay;
        self.schedule_delayed_proc_at(self.now + delay, aura, spell, *result);
    }

    /// The handler: Go `Proc(judgementProcChance, "Judgement of Light - Heal")`, then the
    /// attacker gains the heal under the target's health metrics. A pet's hit rolls the same
    /// chance; its heal changes nothing, as no target strikes a pet and nothing else takes its
    /// health, so a pet is always at full health.
    fn judgement_of_light_heal(&mut self, index: usize, judged: Side, spell: SpellId) {
        let state = &self.judgements_of_light[index];
        let metrics = match judged {
            Side::Extra(extra) => state.copy_metrics[extra as usize],
            _ => state.metrics,
        };
        let (chance, heal) = (state.chance, state.heal);
        if !self
            .rng
            .proc(chance, &self.judgements_of_light[index].rng_label)
        {
            return;
        }
        if self.spells[spell].caster == Side::Player {
            self.gain_health(heal, metrics);
        }
    }

    /// Go `AttachProcTriggerCallback`'s delayed handler: run it a spell batch window from now.
    pub(crate) fn schedule_delayed_proc(
        &mut self,
        aura: AuraRef,
        spell: SpellId,
        result: SpellResult,
    ) {
        self.schedule_delayed_proc_at(self.now + super::SPELL_BATCH_WINDOW, aura, spell, result);
    }

    /// The same at a given time. Go clones the result the handler will hear, which a class
    /// that pools its results answers for with [`Agent::clone_result`].
    pub(crate) fn schedule_delayed_proc_at(
        &mut self,
        at: i64,
        aura: AuraRef,
        spell: SpellId,
        result: SpellResult,
    ) {
        let (result, token) = match A::clone_result(self, spell, &result) {
            Some((clone, token)) => (clone, Some(token)),
            None => (result, None),
        };
        self.schedule(
            at,
            super::PRIORITY_DOT,
            super::Action::DelayedProc {
                aura,
                spell,
                result,
                token,
            },
        );
    }

    /// Go `AttachProcTriggerCallback` for the Windfury Totem trigger: landed hits, the
    /// cooldown, the chance roll; then charges, one fewer when an auto granted them, and an
    /// extra main hand attack at once.
    fn windfury_trigger(&mut self, aura: AuraRef, spell: SpellId, result: &SpellResult) {
        let windfury = self.windfury.as_ref().expect("Windfury Totem is bound");
        if !windfury.trigger_spells[spell] || result.outcome & super::OUTCOME_LANDED == 0 {
            return;
        }
        if windfury.trigger_require_damage && result.damage == 0.0 {
            return;
        }
        let (trigger_chance, proc_aura, extra) =
            (windfury.trigger_chance, windfury.proc_aura, windfury.extra);
        let icd = self.aura(aura).icd;
        if let Some((timer, _)) = icd {
            if self.timers[timer] > self.now {
                return;
            }
        }
        if trigger_chance != 1.0 && self.random_for_aura(aura) > trigger_chance {
            return;
        }
        if let Some((timer, duration)) = icd {
            self.timers[timer] = self.now + duration;
        }
        self.activate_aura(proc_aura);
        let mut charges = self.aura(proc_aura).max_stacks;
        if self.spells[spell].white_hit {
            charges -= 1;
        }
        self.set_stacks(proc_aura, charges);
        // Go MaybeReplaceMHSwing: a queued Heroic Strike replaces the extra swing too.
        let extra = extra.expect("a Windfury Totem a spell triggers has an extra attack");
        let mut attack = extra;
        if self.config.melee.replace_main_hand_swing {
            attack = A::replace_mh_swing(self, extra);
            self.note_extra_attack_strike(extra, attack);
        }
        self.cast(attack, result.target);
    }

    /// Go `AttachProcTriggerCallback` for Crusader: a weapon proc on landed hits that rolls the
    /// hand's chance, then waits a spell batch window.
    fn crusader_callback(&mut self, aura: AuraRef, spell: SpellId, result: &SpellResult) {
        if result.outcome & super::OUTCOME_LANDED == 0 {
            return;
        }
        let Some(chance) = self
            .crusader
            .as_ref()
            .and_then(|crusader| crusader.chances[spell])
        else {
            return;
        };
        if !self.proc_for_aura(chance, aura) {
            return;
        }
        self.schedule_delayed_proc(aura, spell, *result);
    }

    /// Go `AttachProcTriggerCallback` for an item damage proc: its spells and outcome, a hit
    /// that dealt damage, its cooldown and chance; then the damage spell at once, on the unit
    /// hit unless that is the wearer.
    pub(crate) fn damage_proc_callback(
        &mut self,
        aura: AuraRef,
        proc: usize,
        spell: Option<SpellId>,
        result: &SpellResult,
    ) {
        let state = &self.damage_procs[proc];
        // A struck proc's trigger mask: melee and ranged hits, none flagged a proc. The
        // target's swing, which has no spell of the player's, is such a hit.
        let heard = match spell {
            None => state.struck,
            Some(spell) if state.struck => {
                self.spells[spell].melee_or_ranged_proc && !self.spells[spell].flags.proc
            }
            Some(spell) => state.trigger_spells[spell],
        };
        if !heard
            || (!state.casts
                && ((state.landed_only && result.outcome & super::OUTCOME_LANDED == 0)
                    || (state.require_damage && result.damage == 0.0)))
        {
            return;
        }
        let (chance, damage_spell) = (state.chance, state.spell);
        // The target's swing is no spell of the player's; no proc manager in scope hears it.
        let manager = state
            .chances
            .as_ref()
            .map(|chances| spell.and_then(|spell| chances[spell]));
        let icd = self.aura(aura).icd;
        if let Some((timer, _)) = icd {
            if self.timers[timer] > self.now {
                return;
            }
        }
        if chance != 1.0 {
            let roll = match &self.damage_procs[proc].rng_label {
                Some(label) => {
                    let label = label.clone();
                    self.rng.next_f64(&label)
                }
                None => self.random_for_aura(aura),
            };
            if roll > chance {
                return;
            }
        }
        // Go `DynamicProcManager.Proc` under the trigger's name, after the static chance; a spell
        // its masks miss never procs.
        if let Some(manager) = manager {
            match manager {
                Some(chance) if self.proc_for_aura(chance, aura) => {}
                _ => return,
            }
        }
        if let Some((timer, duration)) = icd {
            self.timers[timer] = self.now + duration;
        }
        // A Go literal trigger without TriggerImmediately: the handler casts the hit a delay
        // later, on the unit the trigger's hit landed on, or for the target's swing on its
        // attacker.
        if let Some(delay) = self.damage_procs[proc].delay {
            match spell {
                Some(spell) => {
                    self.schedule_delayed_proc_at(self.now + delay, aura, spell, *result)
                }
                None => {
                    let target = self.damage_proc_target(None, result);
                    self.schedule(
                        self.now + delay,
                        super::PRIORITY_DOT,
                        super::Action::StruckDamageProc { proc, target },
                    );
                }
            }
            return;
        }
        let target = self.damage_proc_target(spell, result);
        self.cast(damage_spell, target);
    }

    /// Go procDamageTarget: a proc of a hit taken answers its attacker, which for the swing of a
    /// copy of the boss is that copy.
    fn damage_proc_target(&self, spell: Option<SpellId>, result: &SpellResult) -> Side {
        match (result.target == Side::Player, spell) {
            (true, None) => result.attacker,
            (true, Some(_)) => self.current_target_side(),
            (false, _) => result.target,
        }
    }

    /// Go `AttachProcTriggerCallback` for a spell data stat proc: the spells it hears, and for a
    /// hit or heal its outcome and damage; its cooldown and chance; then the handler a spell batch
    /// window later. A cast carries no result.
    pub(crate) fn spell_stat_proc_callback(
        &mut self,
        aura: AuraRef,
        proc: usize,
        spell: Option<SpellId>,
        result: Option<&SpellResult>,
    ) {
        let state = &self.spell_stat_procs[proc];
        // A struck proc hears the target's swing, which is no spell of the player's.
        let heard = match spell {
            Some(spell) => state.trigger_spells[spell],
            None => state.struck,
        };
        if !heard {
            return;
        }
        if let Some(result) = result {
            if (state.landed_only && result.outcome & super::OUTCOME_LANDED == 0)
                || (state.require_damage && result.damage == 0.0)
            {
                return;
            }
        }
        let chance = state.chance;
        let icd = self.aura(aura).icd;
        if let Some((timer, _)) = icd {
            if self.timers[timer] > self.now {
                return;
            }
        }
        if chance != 1.0 && self.random_for_aura(aura) > chance {
            return;
        }
        if let Some((timer, duration)) = icd {
            self.timers[timer] = self.now + duration;
        }
        let result = result.copied().unwrap_or(SpellResult {
            armor_multiplier: 0.0,
            target: Side::Target,
            attacker: Side::Player,
            outcome: 0,
            damage: 0.0,
            threat: 0.0,
        });
        // The handler a batch window later activates the aura and reads no spell.
        self.schedule_delayed_proc(aura, spell.unwrap_or(0), result);
    }

    /// Go `AttachProcTriggerCallback` for Dragonbreath Chili: landed melee hits, its cooldown,
    /// then a chance roll, and the cast a spell batch window later.
    fn chili_callback(&mut self, aura: AuraRef, spell: SpellId, result: &SpellResult) {
        let Some(chili) = self.chili.as_ref() else {
            return;
        };
        if !chili.spells[spell] || result.outcome & super::OUTCOME_LANDED == 0 {
            return;
        }
        let (chance, delay) = (chili.proc_chance, chili.delay);
        let icd = self.aura(aura).icd;
        if let Some((timer, _)) = icd {
            if self.timers[timer] > self.now {
                return;
            }
        }
        if chance != 1.0 && self.random_for_aura(aura) > chance {
            return;
        }
        if let Some((timer, duration)) = icd {
            self.timers[timer] = self.now + duration;
        }
        self.schedule_delayed_proc_at(self.now + delay, aura, spell, *result);
    }

    /// The delayed half of a proc trigger.
    pub(crate) fn delayed_proc(&mut self, aura: AuraRef, spell: SpellId, result: SpellResult) {
        match self.aura(aura).behavior.clone() {
            AuraBehavior::JudgementOfWisdom {
                mana, metrics, id, ..
            } => {
                // Go: melee and ranged hits always pay; spells must land.
                if !self.spells[spell].melee_or_ranged_proc
                    && result.outcome & super::OUTCOME_LANDED == 0
                {
                    return;
                }
                // Go gives the mana to the attacker, each unit with its own metrics, which a
                // pet registers at its first proc.
                // Go: a unit without a mana bar gains nothing.
                let caster = self.spells[spell].caster;
                if self.unit_config(caster).max_mana <= 0.0 {
                    return;
                }
                let metrics = match caster {
                    Side::Pet(_) => match self.active_pet(caster).jow_metrics {
                        Some(existing) => existing,
                        None => {
                            let created = self.new_mana_metrics_of(caster, id);
                            self.active_pet_mut(caster).jow_metrics = Some(created);
                            created
                        }
                    },
                    _ => {
                        if !self.jow_metrics_named {
                            self.resources[metrics].id = id;
                            self.jow_metrics_named = true;
                        }
                        metrics
                    }
                };
                self.add_mana(mana, metrics);
            }
            AuraBehavior::TouchOfTheGrave { drain, .. } => {
                self.cast(drain, result.target);
            }
            AuraBehavior::RevelationTrigger => self.revelation_handler(),
            AuraBehavior::Crusader => {
                let crusader = self.crusader.as_ref().expect("Crusader is bound");
                // Go Ternary(spell.IsOH(), ohAura, mhAura).
                let hand = if self.spells[spell].off_hand_proc {
                    crusader.oh_aura
                } else {
                    crusader.mh_aura
                };
                let (heal_min, heal_max, heal_metrics) =
                    (crusader.heal_min, crusader.heal_max, crusader.heal_metrics);
                self.activate_aura(hand);
                let heal = self.go_roll(heal_min, heal_max);
                self.gain_health(heal, heal_metrics);
            }
            AuraBehavior::EmeraldDragonWhelp => self.whelp_summon(),
            AuraBehavior::SulfurasImmolation => self.sulfuras_immolation_hit(result.attacker),
            AuraBehavior::Thunderfury => self.thunderfury_handler(result.target),
            AuraBehavior::ForceReactiveDisk(disk) => {
                self.cast(disk, Side::Player);
            }
            AuraBehavior::StatProc(proc) => {
                let (aura, add_stack) =
                    (self.stat_procs[proc].aura, self.stat_procs[proc].add_stack);
                self.activate_aura(aura);
                if add_stack {
                    self.add_stack(aura);
                }
            }
            AuraBehavior::HealthRageProc(proc) => self.health_rage_proc_handler(proc),
            AuraBehavior::EnergyProc(proc) => self.energy_proc_handler(proc),
            AuraBehavior::ArmorDebuffTrigger(proc) => {
                self.armor_debuff_proc_handler(proc, result.target)
            }
            AuraBehavior::SpellDataStatProc(proc) => {
                let aura = self.spell_stat_procs[proc].aura;
                self.activate_aura(aura);
            }
            AuraBehavior::DragonbreathChili => {
                let spell = self
                    .chili
                    .as_ref()
                    .expect("Dragonbreath Chili is bound")
                    .spell;
                self.cast(spell, result.target);
            }
            AuraBehavior::EnergizeProc(index) => self.energize_proc_handler(index),
            AuraBehavior::SpellDataDamageProc(proc) => {
                let damage_spell = self.damage_procs[proc].spell;
                let target = self.damage_proc_target(Some(spell), &result);
                self.cast(damage_spell, target);
            }
            AuraBehavior::HealthOnCast(index) => {
                let (health, metrics) = (
                    self.health_on_cast[index].health,
                    self.health_on_cast[index].metrics,
                );
                self.gain_health(health, metrics);
            }
            AuraBehavior::JudgementOfLight(index) => {
                self.judgement_of_light_heal(index, aura.side, spell)
            }
            AuraBehavior::ManaOnCast(index) => self.mana_on_cast_handler(index, spell),
            AuraBehavior::StackingCdWindow(index) => self.stacking_cd_handler(index),
            AuraBehavior::Class(kind) => A::on_delayed_proc(self, aura, kind, spell, result),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Go's append doubles a full array and copies it, so a range loop that began on the old
    /// array keeps reading the entries it held, while swaps made afterwards change the new one.
    #[test]
    fn a_loop_that_began_before_an_append_outgrew_the_array_keeps_the_old_one() {
        let mut list = CallbackList::default();
        for aura in 0..4 {
            list.push(aura);
        }
        let before = list.snapshot();
        // The array holds four and is full: this append replaces it.
        list.push(4);
        // Removing the aura at 1 moves the last into its place in the new array only.
        assert_eq!(list.swap_remove(1), Some(4));
        assert_eq!(list.read(before, 1), 1);
        assert_eq!(list.read(list.snapshot(), 1), 4);
        // A loop that begins after the append reads the new array.
        let after = list.snapshot();
        list.push(5);
        assert_eq!(list.read(after, 2), 2);
    }

    /// An append that finds room writes into the array in place, so loops see it.
    #[test]
    fn an_append_with_room_writes_in_place() {
        let mut list = CallbackList::default();
        for aura in 0..3 {
            list.push(aura);
        }
        // Three entries in an array of four.
        let before = list.snapshot();
        assert_eq!(list.swap_remove(0), Some(2));
        assert_eq!(list.read(before, 0), 2);
        list.clear();
        assert_eq!(list.push(9), 0);
        assert_eq!(list.read(list.snapshot(), 0), 9);
    }
}
