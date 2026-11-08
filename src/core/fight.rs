//! A class-independent fight runtime that mirrors the pinned Go `sim/core`.
//!
//! One player fights one to five identical targets, each with its own auras, dots, armor,
//! stats and metrics. The runtime owns time, the pending-action queue, random
//! streams, units, auras, spells, casting, damage, channels, cooldowns, the rotation and
//! metrics. Class behavior plugs in through [`Agent`], so the runtime never names a
//! class. Event order, random draw order and floating-point operation order follow Go,
//! because the shared random stream makes every later draw depend on them.
//!
//! The runtime is built from a validated prepared v2 input. Static modifiers are already
//! applied there and are never applied again here.

pub(crate) mod world_bridge;
mod absorb;
mod aura;
mod blade_of_hanna;
pub(crate) mod burst;
mod cast;
pub(crate) mod casting_regen;
mod cleave;
mod damage;
pub(crate) mod damage_taken;
mod dot;
mod enemy;
pub(crate) mod energy;
pub(crate) mod exclusive;
mod external_cooldown;
mod focus;
mod gear_procs;
mod innervate;
pub(crate) mod item_swap;
mod on_use_damage;
pub(crate) mod proc_damage;
mod revelation;
pub(crate) use on_use_damage::known as on_use_damage_known;
mod heal_proc;
pub(crate) mod healer_gear;
pub(crate) mod healing;
pub(crate) mod heals;
pub(crate) mod health_regen;
mod leveling_recovery;
mod log;
pub(crate) mod melee;
pub(crate) mod metrics;
pub(crate) mod movement;
mod oil_of_immolation;
pub(crate) mod pet;
mod power_infusion;
mod racial;
pub(crate) mod rage;
mod rotation;
pub(crate) mod rotation_checks;
mod spell_mod;
mod sulfuras;
mod thunderfury;
mod whelp;

use std::collections::{BTreeMap, HashMap};
use leveling_recovery::{split_world_regeneration_receipt, WorldRecoveryReceipt, WorldRecoveryStrategy};

pub(crate) use aura::{AuraBehavior, AuraRef, Tracker};
pub(crate) use damage::{
    Outcome, SpellResult, OUTCOME_BLOCK, OUTCOME_CRIT, OUTCOME_DODGE, OUTCOME_LANDED, OUTCOME_MISS,
    OUTCOME_PARRY,
};
pub(crate) use dot::Dot;
pub(crate) use enemy::EnemySlow;
pub(crate) use log::action_string;
pub(crate) use metrics::{ActionTotals, FightReport};
pub(crate) use spell_mod::{ModId, ModKind};

use crate::{
    contracts::prepared_v2::{
        ActionId, Effect, EnergizedPower, ExclusiveMembership, PreparedV2, Schools,
    },
    core::{
        queue::{Handle, PendingQueue},
        rng::{RngMode, SimRng},
        time::{NEVER_EXPIRES, STARTING_CD_TIME},
    },
    mechanics::mana::RegenInputs,
};

pub(crate) type SpellId = usize;
pub(crate) type DotId = usize;
pub(crate) type TimerId = usize;

/// The units of a supported fight: the targets, the player and the pets Rust simulates.
/// `Target` is the first target, the player's current target, and `Extra(k)` the identical
/// copy at position `k + 1` of a fight against several; their Go unit indexes are their
/// positions. The player's Go unit index follows the targets', and a pet's is its own.
/// `Pet(i)` is the pet at position `i` of the simulated pets, in unit index order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Side {
    Target,
    Player,
    Pet(u8),
    Extra(u8),
}

/// The most targets a fight can have, as the application offers.
pub(crate) const MAX_TARGETS: usize = 5;

/// The units a spell can hit, by [`Side::index`]: every target and the player.
pub(crate) const DEFENDERS: usize = MAX_TARGETS + 1;

impl Side {
    /// The unit's position in per-unit lists: the first target, the player, the other
    /// targets in the room [`MAX_TARGETS`] leaves, then each pet. A unit a spell can hit
    /// comes before [`DEFENDERS`].
    pub(crate) fn index(self) -> usize {
        match self {
            Side::Target => 0,
            Side::Player => 1,
            Side::Extra(extra) => 2 + extra as usize,
            Side::Pet(pet) => DEFENDERS + pet as usize,
        }
    }

    /// The target at a position of the encounter's targets, counting from zero.
    pub(crate) fn target(position: usize) -> Side {
        match position {
            0 => Side::Target,
            position => Side::Extra(position as u8 - 1),
        }
    }

    /// The position among the encounter's targets of a unit that is one.
    pub(crate) fn target_position(self) -> Option<usize> {
        match self {
            Side::Target => Some(0),
            Side::Extra(extra) => Some(1 + extra as usize),
            _ => None,
        }
    }

    /// Whether the unit is one of the simulated pets.
    pub(crate) fn is_pet(self) -> bool {
        matches!(self, Side::Pet(_))
    }

    /// Whether the unit is one of the encounter's targets.
    pub(crate) fn is_target(self) -> bool {
        matches!(self, Side::Target | Side::Extra(_))
    }
}

/// Class behavior hooks. Each hook receives the whole fight; the class reads and writes
/// its own state through `fight.agent`.
pub(crate) trait Agent: Sized {
    /// What a class spell does when its effects apply.
    type Spell: Copy + std::fmt::Debug;
    /// Which class aura a callback belongs to.
    type Aura: Copy + std::fmt::Debug;

    /// Go `Spell.ApplyEffects`.
    fn apply_effects(fight: &mut Fight<Self>, spell: SpellId, target: Side, behavior: Self::Spell);
    /// Whether the class registers health metrics under the spell's action before the spell
    /// itself, as Go classes do for a spell that heals its caster; they then come first in
    /// the caster's resource metrics.
    fn health_metrics_before_cost(_behavior: Self::Spell) -> bool {
        false
    }
    /// Go `ExtraCastCondition`.
    fn extra_cast_condition(_fight: &Fight<Self>, _spell: SpellId, _behavior: Self::Spell) -> bool {
        true
    }
    /// Go `ExtraCastCondition` where a class wrapper may log a failure, as the druid's form
    /// check does. Every cast and cast check runs this one.
    fn extra_cast_condition_logged(
        fight: &mut Fight<Self>,
        spell: SpellId,
        behavior: Self::Spell,
    ) -> bool {
        Self::extra_cast_condition(fight, spell, behavior)
    }
    /// Whether the rotation keeps cached answers for this class: a proof per item that it is not
    /// ready, and a look's proof that none is. Only a class whose empty looks can be proven
    /// and mostly follow nothing changing gains from them: one that says how long its failed
    /// extra cast conditions stay failed ([`Agent::extra_condition_proof`]). A class whose
    /// rotation wakes on changes that do not change its answer only pays for them. Any other
    /// asks its rotation as before and pays nothing.
    const CACHE_ROTATION: bool = false;
    /// Whether a cached look also holds as energy rises, while it stays below what any item
    /// waits for: for an energy user, whose rotation wakes on every energy tick. Any other class
    /// keeps no energy bookkeeping.
    const CACHE_OVER_ENERGY: bool = false;
    /// For a class spell's extra cast condition that just failed: until when the clock alone
    /// cannot make it pass, and the kinds of state (see [`reads`]) whose change could. `None`
    /// where the class does not say, and the rotation asks the spell again on every look. A
    /// condition whose answer can change as energy rises has none, so a proof holds over energy
    /// gains whatever it reads.
    fn extra_condition_proof(
        _fight: &Fight<Self>,
        _spell: SpellId,
        _behavior: Self::Spell,
    ) -> Option<(i64, u8)> {
        None
    }
    /// Go `Agent.Reset`, which `Character.reset` runs after the unit and its cooldown manager.
    fn agent_reset(_fight: &mut Fight<Self>) {}
    /// A class wrapper Go puts around any spell's `ApplyEffects`, run after them.
    fn after_apply_effects(_fight: &mut Fight<Self>, _spell: SpellId) {}
    /// Go `AddActivationCondition` on any major cooldown, checked before its own condition.
    fn cooldown_activation_condition(_fight: &Fight<Self>, _spell: SpellId) -> bool {
        true
    }
    /// The exclusive effects of a class aura, which Go activates before it logs the gain.
    fn on_exclusive_gain(_fight: &mut Fight<Self>, _aura: AuraRef, _kind: Self::Aura) {}
    /// Go `ExclusiveEffect` `OnGain` and `OnExpire` in a category the runtime only tracks: the
    /// category's active effect changed, as it does while a member's aura activates or fades.
    fn on_tracked_category_change(_fight: &mut Fight<Self>, _category: usize) {}
    /// Go `RegisterExecutePhaseCallback`'s callbacks, after each execute phase change.
    fn on_execute_phase(_fight: &mut Fight<Self>, _phase: i32) {}
    /// Go `CastConfig.ModifyCast`, run first in a full cast. It may not change the cost.
    fn modify_cast(_fight: &mut Fight<Self>, _spell: SpellId, _behavior: Self::Spell) {}
    /// Go `Spell.CastTime` for a class spell whose `CastConfig.CastTime` replaces the default,
    /// which the cast's `ModifyCast` also applies; `None` keeps Go's default.
    fn cast_time(_fight: &Fight<Self>, _spell: SpellId, _behavior: Self::Spell) -> Option<i64> {
        None
    }
    /// A class wrapper that runs before a melee auto attack's `ApplyEffects`, as Go classes
    /// wrap `MHConfig().ApplyEffects`.
    fn before_melee_auto(_fight: &mut Fight<Self>, _spell: SpellId, _hand: melee::Hand) {}
    /// Go `MajorCooldown.ShouldActivate` for class cooldowns.
    fn should_activate(_fight: &Fight<Self>, _spell: SpellId, _behavior: Self::Spell) -> bool {
        true
    }
    /// For a class cooldown whose [`Agent::should_activate`] just refused it: until when the
    /// clock alone cannot make it accept while nothing a cached look reads changes. `None`
    /// where the class does not say, and the autocast promises nothing past this look. A rule
    /// whose answer can change as energy rises has none.
    fn should_activate_until(
        _fight: &Fight<Self>,
        _spell: SpellId,
        _behavior: Self::Spell,
    ) -> Option<i64> {
        None
    }
    /// The `WaitTravelTime` callback of a class spell scheduled with
    /// [`Fight::class_after_travel`]: by default the result is dealt on arrival.
    fn on_travel(
        fight: &mut Fight<Self>,
        spell: SpellId,
        result: SpellResult,
        _behavior: Self::Spell,
    ) {
        fight.deal_damage(spell, result, false);
    }
    /// Go `DamageDoneByCasterExtraMultiplier` on the target's attack table: the active
    /// handler's multiplier for a spell, if any handler is active.
    fn caster_damage_multiplier(
        _fight: &Fight<Self>,
        _spell: SpellId,
        _target: Side,
    ) -> Option<f64> {
        None
    }
    /// A class spell's Go `ExpectedTickDamage` on a target, from the dot's snapshot or for a
    /// tick cast now, where the class computes it itself.
    fn expected_tick_damage(
        _fight: &mut Fight<Self>,
        _spell: SpellId,
        _target: Side,
        _snapshot: bool,
    ) -> Option<f64> {
        None
    }
    /// When a class's totem of the slot expires, for Go's `totemRemainingTime`. The gate
    /// admits the value only for a class that implements this.
    fn totem_expiration(_fight: &Fight<Self>, _totem: crate::rotation::Totem) -> i64 {
        unreachable!("totemRemainingTime needs a class with totems")
    }
    /// Go `ReplaceMHSwing`: the spell a main hand swing casts instead of the auto attack.
    fn replace_mh_swing(_fight: &mut Fight<Self>, swing: SpellId) -> SpellId {
        swing
    }
    /// A tick of a periodic action a class started with [`Fight::start_class_periodic`].
    fn on_periodic(_fight: &mut Fight<Self>, _tag: u32) {}
    /// A pending action a class scheduled with [`Fight::schedule_class_action`].
    fn on_class_action(_fight: &mut Fight<Self>, _tag: u32) {}
    /// Whether a class action popped from the queue still stands once time has advanced: Go
    /// checks `cancelled` again after the advance, which can expire the aura that owns it.
    fn class_action_valid(_fight: &Fight<Self>, _tag: u32, _handle: Handle) -> bool {
        true
    }
    /// A dot or channel tick of a class spell.
    /// Go's item swap callbacks of the class for the main hand, once the swap put the set's
    /// weapon in the hand: `worn` is true for the swap set. The class takes what the swapped
    /// state's effects say for the weapon it now holds.
    fn on_item_swap(_fight: &mut Fight<Self>, _worn: bool) {}
    fn on_dot_tick(_fight: &mut Fight<Self>, _dot: DotId, _behavior: Self::Spell) {}
    /// The class part of a dot aura's OnGain, which Go runs before the dot's own.
    fn on_dot_gain(_fight: &mut Fight<Self>, _dot: DotId, _behavior: Self::Spell) {}
    /// The class part of a dot aura's OnExpire, which Go runs before the dot's own.
    fn on_dot_expire(_fight: &mut Fight<Self>, _dot: DotId, _behavior: Self::Spell) {}
    /// Go `RegisterMovementCallback`: a unit's movement starts, updates its position or ends.
    fn on_movement(_fight: &mut Fight<Self>, _side: Side, _kind: movement::MovementKind) {}
    /// Go reset effects registered by the class, run before auras reset.
    fn reset(_fight: &mut Fight<Self>) {}
    fn on_gain(_fight: &mut Fight<Self>, _aura: AuraRef, _kind: Self::Aura) {}
    fn on_expire(_fight: &mut Fight<Self>, _aura: AuraRef, _kind: Self::Aura) {}
    fn on_stacks_change(
        _fight: &mut Fight<Self>,
        _aura: AuraRef,
        _kind: Self::Aura,
        _old: i32,
        _new: i32,
    ) {
    }
    /// Go `OnApplyEffects`, before the spell's effects apply.
    fn on_apply_effects(
        _fight: &mut Fight<Self>,
        _aura: AuraRef,
        _kind: Self::Aura,
        _spell: SpellId,
        _target: Side,
    ) {
    }
    fn on_cast_complete(
        _fight: &mut Fight<Self>,
        _aura: AuraRef,
        _kind: Self::Aura,
        _spell: SpellId,
    ) {
    }
    fn on_spell_hit_dealt(
        _fight: &mut Fight<Self>,
        _aura: AuraRef,
        _kind: Self::Aura,
        _spell: SpellId,
        _result: &SpellResult,
    ) {
    }
    /// Go `DynamicDamageTakenModifiers` of the player, after the outcome of the target's
    /// swing, such as an absorb shield.
    fn player_damage_taken_modifiers(_fight: &mut Fight<Self>, _result: &mut SpellResult) {}
    /// Go `OnHealDealt` of a class aura on the caster.
    fn on_heal_dealt(
        _fight: &mut Fight<Self>,
        _aura: AuraRef,
        _kind: Self::Aura,
        _spell: SpellId,
        _result: &SpellResult,
    ) {
    }
    /// Go `OnSpellHitTaken` of a class aura on the player, for the target's swing at it.
    fn on_enemy_hit_taken(
        _fight: &mut Fight<Self>,
        _aura: AuraRef,
        _kind: Self::Aura,
        _result: &SpellResult,
    ) {
    }
    /// Go `OnSpellHitTaken` of a class aura on the target, for the player's hit.
    fn on_spell_hit_taken(
        _fight: &mut Fight<Self>,
        _aura: AuraRef,
        _kind: Self::Aura,
        _spell: SpellId,
        _result: &SpellResult,
    ) {
    }
    /// Go `OnPeriodicDamageTaken` of a class aura on the target, for the player's tick.
    fn on_periodic_damage_taken(
        _fight: &mut Fight<Self>,
        _aura: AuraRef,
        _kind: Self::Aura,
        _spell: SpellId,
        _result: &SpellResult,
    ) {
    }
    /// Go `OnPeriodicDamageDealt` for an aura of the caster.
    fn on_periodic_damage_dealt(
        _fight: &mut Fight<Self>,
        _aura: AuraRef,
        _kind: Self::Aura,
        _spell: SpellId,
        _result: &SpellResult,
    ) {
    }
    /// Go `ExecuteCustomRotation` of a simulated pet the class runs, which its rotation runs
    /// once per timestep. Dragon's Call's whelp runs its own and never reaches this.
    fn pet_rotation(_fight: &mut Fight<Self>, _pet: Side) {}
    /// The result of the hit of a spell that a class is dealing, as the listeners hear it. Go
    /// hands every listener the one result object, which an earlier listener's clone of it can
    /// reset; a class that models its pool of results gives the object's current state.
    fn dealing_result(_fight: &Fight<Self>, _spell: SpellId) -> Option<SpellResult> {
        None
    }
    /// Go `Spell.CloneResult`, which a proc trigger that waits a batch window takes of the hit
    /// it heard: the clone the handler will hear and a token for [`Agent::dispose_clone`], for
    /// a class that models its pool of results.
    fn clone_result(
        _fight: &mut Fight<Self>,
        _spell: SpellId,
        _result: &SpellResult,
    ) -> Option<(SpellResult, usize)> {
        None
    }
    /// Go `DisposeResult` of a clone once its handler has run.
    fn dispose_clone(_fight: &mut Fight<Self>, _token: usize) {}
    /// A proc handler that Go delays by the spell batch window.
    fn on_delayed_proc(
        _fight: &mut Fight<Self>,
        _aura: AuraRef,
        _kind: Self::Aura,
        _spell: SpellId,
        _result: SpellResult,
    ) {
    }
}

/// Go `ProcMaskDirect`, by the bit names the exporter writes.
pub(crate) const DIRECT_PROC_MASKS: &[&str] = &[
    "ProcMaskMeleeMHAuto",
    "ProcMaskMeleeOHAuto",
    "ProcMaskMeleeMHSpecial",
    "ProcMaskMeleeOHSpecial",
    "ProcMaskRangedAuto",
    "ProcMaskRangedSpecial",
    "ProcMaskSpellDamage",
];

/// What a non-class spell does. Class spells use [`SpellBehavior::Class`].
#[derive(Clone, Debug)]
pub(crate) enum SpellBehavior<S> {
    /// Registered but not castable by this runtime; reaching it is a coverage failure.
    None,
    Class(S),
    /// Go consumes.go potion: one mana gain with a random spread, always rolled.
    PotionMana {
        label: String,
        min: f64,
        spread: f64,
        stone_multiplier: f64,
        regen_window: f64,
    },
    /// Go consumes.go potion with a stat aura or a rage gain: the aura activates, then each
    /// instant gain rolls under the potion's name.
    PotionResource {
        label: String,
        /// Each gain's resource, minimum and spread.
        gains: Vec<(ResourceKind, f64, f64)>,
        stone_multiplier: f64,
        /// The temporary stat aura, by player aura index, resolved once auras register.
        aura: Option<usize>,
        /// The potion's resource metrics, by resource, registered lazily.
        metrics: Vec<(ResourceKind, usize)>,
    },
    /// Go consumes.go `makePotionActivationSpellInternal` for a Forever potion with a spell of
    /// its own: the cast casts that spell at the current target.
    CastsSpell(SpellId),
    /// Go consumes_forever_potions.go Dragonfire Potion's spell: on every target in unit index
    /// order, a magic hit and crit of `direct`, whose landing applies the dot, dealt before the
    /// next target's; each tick deals `tick`.
    DragonfirePotion {
        direct: f64,
        tick: f64,
    },
    /// Go consumes_forever_potions.go Caustic Smog Potion's spell: on every target in unit index
    /// order, a magic hit roll whose landing applies the dot; each tick rolls `tick` within the
    /// variance under the label.
    CausticSmogPotion {
        tick: f64,
        variance: f64,
        label: String,
    },
    /// Go consumes.go conjured item: rolled only when the spread exceeds one.
    ConjuredMana {
        label: String,
        min: f64,
        spread: f64,
        selected: bool,
        regen_window: f64,
    },
    /// Go consumes.go conjured item restoring energy, such as Thistle Tea: the cast fires once
    /// all but `spill` of the gain fits in the bar.
    ConjuredEnergy {
        label: String,
        min: f64,
        spread: f64,
        selected: bool,
        spill: f64,
        metrics: usize,
    },
    /// Go consumes.go Goblin Sapper Charge: a Fire hit on the target and one on the player.
    GoblinSapper,
    /// Go movement.go's Movement spell: the aura and its stacks.
    Move,
    /// Go consumes.go newBasicExplosiveSpellConfig without the self hit.
    BasicExplosive {
        min: f64,
        max: f64,
        aoe_cap_multiplier: f64,
    },
    /// Go spell_data_energize.go: an item use that rolls a client energize effect.
    EnergizeOnUse {
        average: f64,
        variance: f64,
        whole: f64,
        /// Each tick of the spell's self hot gains a roll, rather than the cast.
        periodic: bool,
        bar: EnergizedBar,
    },
    /// Go racials.go Touch of the Grave's drain: shadow damage from the caster's maximum
    /// health, healing the caster for the damage dealt.
    TouchOfTheGraveDrain {
        health_fraction: f64,
        metrics: usize,
    },
    /// Go sim/warrior/items.go Diamond Flask: the cast starts its self hot, whose last tick
    /// activates the player aura at this index.
    DiamondFlask(usize),
    /// Go racials.go Eureka!'s cast, which activates its aura.
    Eureka,
    /// A racial whose `ApplyEffects` only activates its player aura.
    ActivateAura(usize),
    /// Go core/buffs.go `registerExternalConsecutiveCDApproximation`'s cast, bound by
    /// [`Fight::bind_external_cooldowns`].
    ExternalCooldown,
    /// Go attack.go's main or off hand auto attack.
    MeleeAuto(melee::Hand),
    /// Go core/consumes.go Dragonbreath Chili's proc: on every target in unit index order, a
    /// magic hit on its own rolled base damage, dealt before the next target's.
    AreaRollDamage {
        min: f64,
        max: f64,
    },
    /// A magic hit on a rolled base damage, as Acid Spit's, or one that cannot crit, as a
    /// damage shield's.
    RollDamage {
        min: f64,
        max: f64,
        can_crit: bool,
    },
    /// A hit on a rolled base damage that takes a physical attack table, as an item proc of
    /// the melee defense type does, Iceblade Hacker's Frost hit: the school keeps its partial
    /// resist roll and the table rolls the hit and the crit.
    RollTableDamage {
        min: f64,
        max: f64,
        outcome: melee::PhysicalOutcome,
    },
    /// Sulfuras's Fireball: a magic hit rolled between two bounds whose landing applies its burn.
    SulfurasFireball {
        min: f64,
        max: f64,
    },
    /// A fixed amount on a physical attack table, which rolls no damage: Blade of Hanna's
    /// Perfected Form.
    FixedTableDamage {
        damage: f64,
        outcome: melee::PhysicalOutcome,
    },
    /// A fixed hit that always lands, as Sulfuras's Immolation casts.
    FixedHit(f64),
    /// Force Reactive Disk's spell: a hit of a literal on the magic table with a crit against
    /// every target, each dealt as it is calculated.
    AreaFixedDamage(f64),
    /// Thunderfury's strike: a hit of a literal on the magic table with a crit, whose landing
    /// puts the slow on its target.
    ThunderfuryStrike(f64),
    /// Thunderfury's bounce: a hit of nothing on the magic table to several targets, each it
    /// lands on taking the resistance aura.
    ThunderfuryBounce,
    /// A magic hit on a client damage effect's roll, which draws only with a variance, as an
    /// item proc built from client rows casts.
    EffectRoll {
        average: f64,
        variance: f64,
        can_crit: bool,
    },
    /// A weapon or item proc's spell of a client row: a hit on the row's roll on the table of
    /// its defense type, a chain, a damage over time, a missile's travel.
    ProcDamage(proc_damage::ProcDamage),
    /// A weapon proc's spell that grants extra main hand attacks, as Flurry Axe's.
    ExtraAttack(i32),
    /// A damage on-use item's spell.
    OnUseDamage(on_use_damage::OnUseDamage),
    /// A heal proc's spell: a heal on the wearer through `CalcAndDealHealing`.
    SelfHeal(heal_proc::SelfHeal),
    /// An item use's absorb shield, by its position in `Fight::item_absorbs`.
    AbsorbOnUse(usize),
    /// Go items_trinkets.go Second Wind: mana each period for a number of ticks, used once the
    /// deficit reaches the minimum.
    PeriodicMana {
        amount: f64,
        ticks: i32,
        period: i64,
        metrics: usize,
        min_deficit: f64,
    },
}

/// Go spell flags used by the runtime, parsed from exported names.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Flags {
    pub(crate) binary: bool,
    pub(crate) channeled: bool,
    pub(crate) helpful: bool,
    pub(crate) no_on_cast_complete: bool,
    pub(crate) no_metrics: bool,
    pub(crate) no_logs: bool,
    pub(crate) apl: bool,
    pub(crate) mcd: bool,
    pub(crate) reactive: bool,
    pub(crate) no_on_damage_dealt: bool,
    pub(crate) ignore_resists: bool,
    pub(crate) ignore_target_modifiers: bool,
    pub(crate) ignore_attacker_modifiers: bool,
    pub(crate) passive: bool,
    pub(crate) swapped: bool,
    pub(crate) can_cast_while_moving: bool,
    pub(crate) proc: bool,
    pub(crate) melee_metrics: bool,
    pub(crate) no_spell_mods: bool,
    /// Go `SpellFlagCombatPotion`, which the rotation's potion action names.
    pub(crate) combat_potion: bool,
    pub(crate) cannot_be_dodged: bool,
    /// Go `SpellFlagPushback`: damage taken during the hardcast pushes the cast back.
    pub(crate) pushback: bool,
}

impl Flags {
    fn parse(names: &[String]) -> Self {
        let mut flags = Flags::default();
        for name in names {
            match name.as_str() {
                "SpellFlagBinary" => flags.binary = true,
                "SpellFlagChanneled" => flags.channeled = true,
                "SpellFlagHelpful" => flags.helpful = true,
                "SpellFlagNoOnCastComplete" => flags.no_on_cast_complete = true,
                "SpellFlagNoMetrics" => flags.no_metrics = true,
                "SpellFlagNoLogs" => flags.no_logs = true,
                "SpellFlagAPL" => flags.apl = true,
                "SpellFlagMCD" => flags.mcd = true,
                "SpellFlagReactive" => flags.reactive = true,
                "SpellFlagNoOnDamageDealt" => flags.no_on_damage_dealt = true,
                "SpellFlagIgnoreResists" => flags.ignore_resists = true,
                "SpellFlagIgnoreTargetModifiers" => flags.ignore_target_modifiers = true,
                "SpellFlagIgnoreAttackerModifiers" => flags.ignore_attacker_modifiers = true,
                "SpellFlagPassiveSpell" => flags.passive = true,
                "SpellFlagSwapped" => flags.swapped = true,
                "SpellFlagCanCastWhileMoving" => flags.can_cast_while_moving = true,
                "SpellFlagProc" => flags.proc = true,
                "SpellFlagMeleeMetrics" => flags.melee_metrics = true,
                "SpellFlagNoSpellMods" => flags.no_spell_mods = true,
                "SpellFlagCombatPotion" => flags.combat_potion = true,
                "SpellFlagCannotBeDodged" => flags.cannot_be_dodged = true,
                "SpellFlagPushback" => flags.pushback = true,
                _ => {}
            }
        }
        flags
    }
}

/// Go `Cast`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Cast {
    pub(crate) cost: f64,
    pub(crate) gcd: i64,
    pub(crate) gcd_min: i64,
    pub(crate) cast_time: i64,
    pub(crate) non_empty: bool,
}

impl Cast {
    /// Go `Cast.GCDTime`.
    pub(crate) fn gcd_time(&self) -> i64 {
        let mut gcd = self.gcd.max(0);
        if self.gcd > 0 {
            gcd = if self.gcd_min != 0 {
                self.gcd_min.max(gcd)
            } else {
                crate::core::time::NS_PER_SECOND.max(gcd)
            };
        }
        gcd
    }

    /// Go `Cast.EffectiveTime`.
    pub(crate) fn effective_time(&self) -> i64 {
        self.gcd_time().max(self.cast_time)
    }
}

/// Go `SpellCost`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cost {
    /// The resource the cost spends.
    pub(crate) kind: ResourceKind,
    /// Go `EnergyCost.Refund`.
    pub(crate) refund: f64,
    pub(crate) base: i32,
    pub(crate) flat_modifier: i32,
    pub(crate) percent_modifier: f64,
    pub(crate) additive_percent_modifier: f64,
}

/// Go `DamageRange` (patch 92): the count, total, smallest and largest of one kind of landed
/// damage event.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct DamageRange {
    pub(crate) count: i32,
    pub(crate) total: f64,
    pub(crate) min: f64,
    pub(crate) max: f64,
}

impl DamageRange {
    /// Go `DamageRange.add`.
    pub(crate) fn add(&mut self, damage: f64) {
        if self.count == 0 || damage < self.min {
            self.min = damage;
        }
        if self.count == 0 || damage > self.max {
            self.max = damage;
        }
        self.count += 1;
        self.total += damage;
    }

    /// Go `DamageRange.merge`.
    pub(crate) fn merge(&mut self, other: &DamageRange) {
        if other.count == 0 {
            return;
        }
        if self.count == 0 || other.min < self.min {
            self.min = other.min;
        }
        if self.count == 0 || other.max > self.max {
            self.max = other.max;
        }
        self.count += other.count;
        self.total += other.total;
    }
}

/// Go `SpellMetrics` for one target during one iteration.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SpellMetrics {
    pub(crate) casts: i32,
    pub(crate) misses: i32,
    pub(crate) dodges: i32,
    pub(crate) parries: i32,
    pub(crate) glances: i32,
    pub(crate) blocks: i32,
    pub(crate) blocked_crits: i32,
    pub(crate) crushes: i32,
    pub(crate) hits: i32,
    pub(crate) resisted_hits: i32,
    pub(crate) crits: i32,
    pub(crate) resisted_crits: i32,
    pub(crate) ticks: i32,
    pub(crate) resisted_ticks: i32,
    pub(crate) crit_ticks: i32,
    pub(crate) resisted_crit_ticks: i32,
    pub(crate) total_damage: f64,
    pub(crate) total_resisted_damage: f64,
    pub(crate) total_crit_damage: f64,
    pub(crate) total_resisted_crit_damage: f64,
    pub(crate) total_tick_damage: f64,
    pub(crate) total_resisted_tick_damage: f64,
    pub(crate) total_crit_tick_damage: f64,
    pub(crate) total_resisted_crit_tick_damage: f64,
    pub(crate) total_glance_damage: f64,
    pub(crate) total_block_damage: f64,
    pub(crate) total_blocked_crit_damage: f64,
    pub(crate) total_crush_damage: f64,
    pub(crate) total_threat: f64,
    pub(crate) total_cast_time: i64,
    pub(crate) total_healing: f64,
    pub(crate) total_crit_healing: f64,
    /// The heals' overhealing share (see [`Spell::overheal`]), which healing leaves out.
    pub(crate) total_overhealing: f64,
    /// Go `TotalShielding`: the absorb shields the spell put on the unit.
    pub(crate) total_shielding: f64,
    /// The spread of each kind of landed damage event (patch 92).
    pub(crate) hit_range: DamageRange,
    pub(crate) crit_range: DamageRange,
    pub(crate) tick_range: DamageRange,
    pub(crate) crit_tick_range: DamageRange,
}

/// The weapon an attack is made with, whose skill moves its table (Go weapon_skill.go).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SkillHand {
    MainHand,
    OffHand,
    Ranged,
}

impl SkillHand {
    /// Go `AttackTable.weaponSkillBonus`: `ProcMaskMeleeMH`, then `ProcMaskMeleeOH`, then
    /// `ProcMaskRanged`.
    fn of(proc_mask: &[String]) -> Option<SkillHand> {
        let any = |names: [&str; 2]| proc_mask.iter().any(|mask| names.contains(&mask.as_str()));
        if any(["ProcMaskMeleeMHAuto", "ProcMaskMeleeMHSpecial"]) {
            Some(SkillHand::MainHand)
        } else if any(["ProcMaskMeleeOHAuto", "ProcMaskMeleeOHSpecial"]) {
            Some(SkillHand::OffHand)
        } else if any(["ProcMaskRangedAuto", "ProcMaskRangedSpecial"]) {
            Some(SkillHand::Ranged)
        } else {
            None
        }
    }
}

pub(crate) struct Spell<S> {
    pub(crate) id: ActionId,
    /// The unit that casts the spell: the player or its pet.
    pub(crate) caster: Side,
    pub(crate) flags: Flags,
    pub(crate) behavior: SpellBehavior<S>,
    pub(crate) school: u8,
    pub(crate) school_index: usize,
    /// Go `SpellSchoolFrostfire`, which reads the better of its two schools' bonuses.
    pub(crate) frostfire: bool,
    pub(crate) magic_defense: bool,
    pub(crate) direct_proc: bool,
    /// Go `ProcMaskSpellDamage`.
    pub(crate) proc_spell_damage: bool,
    /// Go `ProcMaskMelee`.
    pub(crate) melee_proc: bool,
    /// Go `ProcMaskMeleeOrRanged`.
    pub(crate) melee_or_ranged_proc: bool,
    /// Go `ProcMaskRanged`: `Spell.IsRanged`.
    pub(crate) ranged_proc: bool,
    /// Go `ProcMaskMeleeOH`: `Spell.IsOH`.
    pub(crate) off_hand_proc: bool,
    /// Go `AttackTable.weaponSkillBonus`: the hand whose weapon skill the attack takes, by its
    /// proc mask (melee main hand first, then off hand, then ranged).
    pub(crate) skill_hand: Option<SkillHand>,
    /// Go `ProcMaskMeleeWhiteHit`.
    pub(crate) white_hit: bool,
    /// Go `ProcMaskMeleeSpecial`.
    pub(crate) melee_special: bool,
    /// Go `CastRequirement`, as the player auras whose form it refuses.
    pub(crate) requirement_auras: Vec<AuraRef>,
    pub(crate) class_spell: Option<String>,
    /// Go `RankAliases`: older rank IDs a rotation may still name.
    pub(crate) rank_aliases: Vec<ActionId>,
    pub(crate) class_spell_mask: bool,
    pub(crate) missile_speed: f64,
    pub(crate) cast_kind: crate::contracts::prepared_v2::CastKind,
    pub(crate) has_extra_cast_condition: bool,
    /// Go's range condition, part of `ExtraCastCondition` when either bound is set.
    pub(crate) min_range: f64,
    pub(crate) max_range: f64,
    pub(crate) ignore_haste: bool,
    pub(crate) cost: Option<Cost>,
    pub(crate) default_cast: Cast,
    pub(crate) cur_cast: Cast,
    pub(crate) cd: Option<(TimerId, i64)>,
    pub(crate) shared_cd: Option<(TimerId, i64)>,
    pub(crate) cast_time_multiplier: f64,
    pub(crate) cd_multiplier: f64,
    pub(crate) bonus_hit_percent: f64,
    pub(crate) bonus_crit_percent: f64,
    pub(crate) bonus_spell_damage: f64,
    pub(crate) bonus_expertise_percent: f64,
    pub(crate) damage_multiplier: f64,
    pub(crate) damage_multiplier_additive: f64,
    pub(crate) direct_damage_multiplier_additive: f64,
    pub(crate) crit_multiplier_pct: f64,
    pub(crate) crit_multiplier_additive: f64,
    pub(crate) bonus_base_damage: f64,
    pub(crate) bonus_coefficient: f64,
    pub(crate) threat_multiplier: f64,
    pub(crate) flat_threat_bonus: f64,
    /// Go `Spell.PushbackResist`, which the pushback chance loses.
    pub(crate) pushback_resist: f64,
    pub(crate) damage_effect: Option<(f64, f64)>,
    pub(crate) dot: Option<DotId>,
    /// Go `RelatedDotSpell`, whose dot `Spell.Dot` resolves to when this spell has none.
    pub(crate) related_dot_spell: Option<SpellId>,
    /// Index into the resource metrics for this spell's mana cost.
    pub(crate) mana_metrics: Option<usize>,
    /// Indexes into the resource metrics for this spell's energy cost and combo points.
    pub(crate) energy_metrics: Option<(usize, usize)>,
    /// The health metrics a class registers before the spell, per
    /// [`Agent::health_metrics_before_cost`].
    pub(crate) health_metrics: Option<usize>,
    /// Go `Spell.HealthMetrics` for the player, registered at the spell's first heal.
    pub(crate) self_health_metrics: Option<usize>,
    /// The share of each of the spell's heals that counts as overhealing, under Forever's
    /// rules in MythicSim's healer sims; zero counts every heal in full, as Go does.
    pub(crate) overheal: f64,
    /// Go `Spell.ResourceMetrics`: the rage metrics a white hit without a cost registers on
    /// its first landed hit.
    pub(crate) rage_metrics: Option<usize>,
    /// Go rage.go's switch on the exact proc mask: the hand of a white hit that gives rage.
    pub(crate) white_hand: Option<melee::Hand>,
    /// The spell's metrics on each unit it can hit, by [`Side::index`], kept apart so the
    /// spell stays small.
    pub(crate) metrics: [SpellMetrics; DEFENDERS],
    /// Index into the fight's action metrics, absent for `SpellFlagNoMetrics`; the current
    /// split's for a spell with metric splits.
    pub(crate) action: Option<usize>,
    /// Go `splitSpellMetrics`, by split; `metrics` holds the current split's while it is set.
    pub(crate) split_metrics: Vec<[SpellMetrics; DEFENDERS]>,
    /// The action metrics of each split.
    pub(crate) split_actions: Vec<usize>,
    /// The current split, which Go keeps across iterations.
    pub(crate) split: usize,
    /// Go `Spell.casts`: the casts whose effects applied this iteration, prepull casts included.
    pub(crate) casts: u32,
    /// Go `Unit.SpellsInFlight` of the spell: its travel callbacks still pending.
    pub(crate) in_flight: u32,
}

/// Go `Hardcast`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Hardcast {
    pub(crate) expires: i64,
    pub(crate) spell: Option<SpellId>,
    pub(crate) target: Side,
    /// The cast time the hardcast started with.
    pub(crate) cast_time: i64,
    /// Whether the cast carries `SpellFlagPushback`.
    pub(crate) pushback: bool,
}

impl Hardcast {
    /// No hardcast, expiring at `expires`.
    pub(crate) const fn idle(expires: i64) -> Self {
        Self {
            expires,
            spell: None,
            target: Side::Target,
            cast_time: 0,
            pushback: false,
        }
    }
}

/// Go `QueuedSpell`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct QueuedSpell {
    pub(crate) spell: SpellId,
    pub(crate) target: Side,
    pub(crate) action: Option<Handle>,
    pub(crate) initiated_at: i64,
    /// Go `queueAction.NextActionAt`, kept after the action runs.
    pub(crate) fire_at: i64,
}

/// The stats a temporary stat change can set, as Go `Unit.stats` entries.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Powers {
    pub(crate) spell_damage: f64,
    pub(crate) attack_power: f64,
    pub(crate) ranged_attack_power: f64,
    pub(crate) spell_crit_percent: f64,
    /// Nothing in scope reads physical crit yet; it follows the stat for completeness.
    pub(crate) physical_crit_percent: f64,
    /// Go `MaxMana`: the Mana stat, which a stat aura can change.
    pub(crate) max_mana: f64,
    /// Go `stats.MP5`, which mana regeneration reads.
    pub(crate) mp5: f64,
    /// Go `stats.HealingPower`, which a heal's bonus coefficient reads.
    pub(crate) healing_power: f64,
    /// Go `SpiritManaRegenPerSecond`, from Intellect and Spirit, which stat auras can change.
    pub(crate) spirit_regen_per_second: f64,
    /// Go `stats.PhysicalDamage`, which `Spell.BonusDamage` adds to a physical spell and a stat
    /// aura, as Sword of Zeal's, can change.
    pub(crate) physical_damage: f64,
    /// Go `stats.ArmorPenetration`, which `GetArmorDamageModifier` takes off the target's armor
    /// and a stacking aura, as Bonereaver's Edge's, can change.
    pub(crate) armor_penetration: f64,
    /// Go `stats.SpellPiercing`, which a partial resist takes off the target's resistance and a
    /// temporary stat change, as Eye of Moam's, can change.
    pub(crate) spell_piercing: f64,
    /// Go `stats.PhysicalHitPercent` and `stats.SpellHitPercent`, which an item swap's weapon
    /// can change.
    pub(crate) physical_hit_percent: f64,
    pub(crate) spell_hit_percent: f64,
}

/// Mutable player state, reset to the prepared values each iteration.
pub(crate) struct Player {
    /// Go `Unit.stats` for the stats auras can change during a fight.
    pub(crate) powers: Powers,
    pub(crate) mana: f64,
    /// Go `healthBar.currentHealth`; the player takes no damage in scope.
    pub(crate) health: f64,
    pub(crate) spell_cost_percent_modifier: i32,
    /// Go `PseudoStats.ThreatMultiplier`, which auras, forms and stances can multiply.
    pub(crate) threat_multiplier: f64,
    /// Go `PseudoStats.SchoolDamageDealtMultiplier`, which auras can multiply.
    pub(crate) school_damage_dealt_multiplier: [f64; 8],
    /// Go `PseudoStats.DisableDWMissPenalty`, which a queued Raptor Strike sets.
    pub(crate) disable_dw_miss_penalty: bool,
    /// Go `PseudoStats.DamageTakenMultiplier`, which auras can multiply.
    pub(crate) damage_taken_multiplier: f64,
    /// Go `PseudoStats.SchoolDamageTakenMultiplier`, which racial survival auras multiply.
    pub(crate) school_damage_taken_multiplier: [f64; 8],
    /// Go `PseudoStats.DamageDealtMultiplier`, which Defensive Stance multiplies.
    pub(crate) damage_dealt_multiplier: f64,
    /// Go `PseudoStats.CastSpeedMultiplier`.
    pub(crate) cast_speed_multiplier: f64,
    /// Go `PseudoStats.AttackSpeedMultiplier` and `MeleeSpeedMultiplier`.
    pub(crate) attack_speed_multiplier: f64,
    pub(crate) melee_speed_multiplier: f64,
    /// Go `PseudoStats.RangedSpeedMultiplier`.
    pub(crate) ranged_speed_multiplier: f64,
    pub(crate) spirit_regen_rate_casting: f64,
    pub(crate) spirit_regen_multiplier: f64,
    pub(crate) force_full_spirit_regen: bool,
    pub(crate) mana_regen_multiplier: f64,
    pub(crate) five_second_rule_refresh: i64,
    pub(crate) mana_tick_casting: f64,
    pub(crate) mana_tick_not_casting: f64,
    pub(crate) waiting_for_mana: f64,
    pub(crate) waiting_for_mana_start: i64,
    /// Go `spiritRegenAttribution`: the regeneration source whose bonus mana ticks credit to
    /// its own metrics.
    pub(crate) spirit_attribution: Option<SpiritAttribution>,
    /// The active casting regeneration auras a casting mana tick credits, under Forever's rule.
    pub(crate) casting_regen_sources: Vec<casting_regen::CastingRegenSource>,
    pub(crate) gcd: i64,
    pub(crate) rotation_timer: i64,
    pub(crate) hardcast: Hardcast,
    pub(crate) hardcast_action: Option<Handle>,
    /// Go `HardcastAvoidanceAura`'s state: a tank's hardcast drops its avoidance until the cast
    /// completes.
    pub(crate) reduced_avoidance: bool,
    pub(crate) rotation_action: Option<Handle>,
    pub(crate) queued: Option<QueuedSpell>,
    pub(crate) channeled_dot: Option<DotId>,
    pub(crate) mana_spent: f64,
    pub(crate) mana_gained: f64,
    pub(crate) oom_time: i64,
    pub(crate) went_oom: bool,
    pub(crate) first_oom: i64,
    /// Go `Unit.Moving` and its pending movement.
    pub(crate) moving: bool,
    pub(crate) movement: Option<movement::Movement>,
    /// Go `Unit.movementEndedAt`: when the unit last stopped moving, `-NEVER_EXPIRES` while it
    /// has not moved this iteration.
    pub(crate) movement_ended_at: i64,
}

impl Player {
    /// A unit's state before its first reset, from its configuration.
    pub(crate) fn initial(config: &Config) -> Self {
        Player {
            powers: config.powers,
            mana: config.max_mana,
            health: config.max_health,
            spell_cost_percent_modifier: config.initial.spell_cost_percent_modifier,
            threat_multiplier: config.threat_multiplier,
            school_damage_dealt_multiplier: config.school_damage_dealt_multiplier,
            disable_dw_miss_penalty: config.melee.disable_dw_miss_penalty,
            damage_taken_multiplier: config.damage_taken_multiplier,
            school_damage_taken_multiplier: config.school_damage_taken_multiplier,
            cast_speed_multiplier: config.initial.cast_speed_multiplier,
            attack_speed_multiplier: config.melee.attack_speed_multiplier,
            melee_speed_multiplier: config.melee.melee_speed_multiplier,
            ranged_speed_multiplier: config.ranged_speed_multiplier,
            spirit_regen_rate_casting: config.initial.spirit_regen_rate_casting,
            spirit_regen_multiplier: config.initial.spirit_regen_multiplier,
            force_full_spirit_regen: config.initial.force_full_spirit_regen,
            damage_dealt_multiplier: config.damage_dealt_multiplier,
            mana_regen_multiplier: 1.0,
            five_second_rule_refresh: 0,
            mana_tick_casting: 0.0,
            mana_tick_not_casting: 0.0,
            waiting_for_mana: 0.0,
            waiting_for_mana_start: 0,
            spirit_attribution: None,
            casting_regen_sources: Vec::new(),
            gcd: STARTING_CD_TIME,
            rotation_timer: STARTING_CD_TIME,
            hardcast: Hardcast::idle(STARTING_CD_TIME),
            hardcast_action: None,
            reduced_avoidance: false,
            rotation_action: None,
            queued: None,
            channeled_dot: None,
            mana_spent: 0.0,
            mana_gained: 0.0,
            oom_time: 0,
            went_oom: false,
            first_oom: 0,
            moving: false,
            movement_ended_at: -NEVER_EXPIRES,
            movement: None,
        }
    }
}

/// Go common/classic/enchants.go Crusader.
#[derive(Clone, Debug)]
pub(crate) struct Crusader {
    pub(crate) chances: Vec<Option<f64>>,
    pub(crate) mh_aura: AuraRef,
    pub(crate) oh_aura: AuraRef,
    pub(crate) heal_min: f64,
    pub(crate) heal_max: f64,
    pub(crate) heal_metrics: usize,
}

/// Go buffs/drivers.go `driveWindfuryTotem`.
#[derive(Clone, Debug)]
pub(crate) struct Windfury {
    pub(crate) totem: AuraRef,
    pub(crate) period: i64,
    pub(crate) trigger: AuraRef,
    pub(crate) trigger_spells: Vec<bool>,
    pub(crate) trigger_chance: f64,
    pub(crate) proc_aura: AuraRef,
    pub(crate) spend_spells: Vec<bool>,
    /// The extra main hand attack, absent when no spell can trigger the totem.
    pub(crate) extra: Option<SpellId>,
    /// Whether a higher Windfury Totem category effect, as a main hand Windfury Weapon's,
    /// holds the category, so the totem's effect cannot turn the trigger on.
    pub(crate) blocked: bool,
    /// Go `ProcTrigger.RequireDamageDealt` of the trigger and of the charge spender.
    pub(crate) trigger_require_damage: bool,
    pub(crate) spend_require_damage: bool,
}

/// The trigger aura of an effect `Fight::damage_procs` holds: a spell data damage proc, or a
/// weapon's extra attack proc, which casts its spell at once on the unit hit the same way.
fn damage_proc_trigger(effect: &Effect) -> Option<&String> {
    match effect {
        Effect::SpellDataDamageProc { trigger_aura, .. } => Some(trigger_aura),
        Effect::ExtraAttackProc {
            trigger_aura,
            chances: Some(_),
            spell: Some(_),
            ..
        } => Some(trigger_aura),
        _ => None,
    }
}

/// Go common/shared/shared_utils.go `applySpellDataDamageProc`: an item proc that casts a
/// single target magic hit at once on the unit hit.
#[derive(Clone, Debug)]
pub(crate) struct DamageProc {
    pub(crate) trigger_spells: Vec<bool>,
    /// A "when struck" proc, on melee and ranged hits the player takes.
    pub(crate) struck: bool,
    /// A proc on the spells the player casts, which carry no result.
    pub(crate) casts: bool,
    pub(crate) landed_only: bool,
    pub(crate) require_damage: bool,
    pub(crate) chance: f64,
    /// A weapon proc's manager: each spell's chance, rolled with Go `Proc`.
    pub(crate) chances: Option<Vec<Option<f64>>>,
    pub(crate) spell: SpellId,
    /// The wait before the handler casts the hit, or none to cast it at once.
    pub(crate) delay: Option<i64>,
    /// The label of the chance roll where it is not the trigger aura's.
    pub(crate) rng_label: Option<String>,
}

/// A proc whose manager rolls on landed hits and whose handler a spell batch window later
/// activates a stat aura: a set bonus's or a weapon's.
#[derive(Clone, Debug)]
pub(crate) struct StatProc {
    /// Each spell's chance.
    pub(crate) chances: Vec<Option<f64>>,
    /// The label of the roll.
    pub(crate) rng_label: String,
    pub(crate) aura: AuraRef,
    /// The handler also adds a stack of the aura.
    pub(crate) add_stack: bool,
}

/// Go common/shared/shared_utils.go `applySpellDataProc`: a stat proc on the spells, heals and
/// casts its listener hears.
#[derive(Clone, Debug)]
pub(crate) struct SpellStatProc {
    pub(crate) trigger_spells: Vec<bool>,
    pub(crate) hits: bool,
    pub(crate) heals: bool,
    pub(crate) casts: bool,
    /// A proc of the hits the player takes, which for the spells it names is the Goblin Sapper
    /// Charge's hit on its thrower.
    pub(crate) taken: bool,
    /// A "when struck" proc, on the target's melee swings.
    pub(crate) struck: bool,
    pub(crate) landed_only: bool,
    pub(crate) require_damage: bool,
    pub(crate) chance: f64,
    pub(crate) aura: AuraRef,
}

/// A proc on the player's casts whose handler restores health a delay later: Nightslayer
/// Armor's eight piece bonus on Vanish.
#[derive(Clone, Debug)]
pub(crate) struct HealthOnCast {
    /// The spells it hears, by spellbook position.
    pub(crate) spells: Vec<bool>,
    pub(crate) health: f64,
    pub(crate) metrics: usize,
    pub(crate) delay: i64,
}

/// Go buffs/paladin.go `AttachJudgementOfLightHeal` on the judged target.
#[derive(Clone, Debug)]
pub(crate) struct JudgementOfLight {
    pub(crate) rng_label: String,
    pub(crate) chance: f64,
    pub(crate) heal: f64,
    /// The target's health metrics.
    pub(crate) metrics: usize,
    /// Each target copy's own health metrics, as Go registers them on each target's aura.
    pub(crate) copy_metrics: Vec<usize>,
    pub(crate) delay: i64,
}

/// Go core/consumes.go `registerDragonbreathChili`.
#[derive(Clone, Debug)]
pub(crate) struct DragonbreathChili {
    pub(crate) spells: Vec<bool>,
    pub(crate) proc_chance: f64,
    pub(crate) spell: SpellId,
    pub(crate) delay: i64,
}

/// Go buffs/drivers.go `driveSunderArmor`.
#[derive(Clone, Debug)]
struct SunderRamp {
    aura: AuraRef,
    period: i64,
    ticks: i32,
    armor_by_stacks: Vec<f64>,
    /// A stronger permanent member of the aura's exclusive category blocks every activation.
    blocked: bool,
}

/// The raid's Expose Armor under Forever's rules (`ExternalExposeArmor`): the target opens at
/// `armor_before` and the aura lands `delay` into the fight.
#[derive(Clone, Debug)]
struct ExternalExpose {
    aura: AuraRef,
    delay: i64,
    armor_before: f64,
}

/// Go aura_helpers.go `ApplyFixedUptimeAura` for a player aura.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FixedUptime {
    aura: AuraRef,
    uptime: f64,
    chance_per_tick: f64,
    tick_length: i64,
    start_time: i64,
    /// The aura's own duration, which the first roll replaces for its activation only.
    duration: i64,
    /// buffs.go ApplyFixedShoutAura: the player's own aura whose gain brings this one back.
    chained_by: Option<AuraRef>,
}

/// Go `spiritRegenAttribution`: the spirit regeneration state before the source applied.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SpiritAttribution {
    pub(crate) metrics: usize,
    pub(crate) multiplier: f64,
    pub(crate) force_full: bool,
}

/// Static configuration copied from the prepared input.
pub(crate) struct Config {
    pub(crate) iterations: u32,
    pub(crate) seed: i64,
    pub(crate) debug_first_iteration: bool,
    pub(crate) debug: bool,
    /// Go `saveAllValues`: each distribution keeps every iteration's value.
    pub(crate) save_all_values: bool,
    pub(crate) base_duration: i64,
    pub(crate) duration_variation: i64,
    /// Go `ExecuteProportion_90`, `_45`, `_35`, `_25` and `_20`.
    pub(crate) execute_proportions: [f64; 5],
    /// Go `PseudoStats.DamageTakenMultiplier` for the player after the reset.
    pub(crate) damage_taken_multiplier: f64,
    /// Go `PseudoStats.SchoolDamageTakenMultiplier` for the player after the reset.
    pub(crate) school_damage_taken_multiplier: [f64; 8],
    pub(crate) player_label: String,
    pub(crate) player_name: String,
    pub(crate) target_label: String,
    /// The player's Go unit index, which counts every target.
    pub(crate) player_index: i32,
    pub(crate) player_level: i32,
    pub(crate) target_level: i32,
    /// Whether the target's level above the caster's adds partial resists (the Go reference) or
    /// not (Forever).
    pub(crate) level_partial_resist: bool,
    pub(crate) reaction: i64,
    pub(crate) channel_clip_delay: i64,
    pub(crate) distance: f64,
    pub(crate) cast_speed: f64,
    pub(crate) spell_haste_rating: f64,
    pub(crate) max_mana: f64,
    pub(crate) max_health: f64,
    pub(crate) teardown_max_mana: f64,
    pub(crate) spirit_regen_per_second: f64,
    /// The prepared stats; auras change the player's copy.
    pub(crate) powers: Powers,
    pub(crate) school_damage: [f64; 8],
    pub(crate) initial: InitialPseudo,
    pub(crate) school_bonus_hit_chance: [f64; 8],
    pub(crate) damage_dealt_multiplier: f64,
    pub(crate) school_damage_dealt_multiplier: [f64; 8],
    pub(crate) crit_damage_multiplier: f64,
    pub(crate) threat_multiplier: f64,
    pub(crate) table: crate::contracts::prepared_v2::AttackTable,
    pub(crate) target_resistance: [f64; 8],
    pub(crate) target_damage_taken_multiplier: f64,
    pub(crate) target_school_damage_taken_multiplier: [f64; 8],
    pub(crate) target_school_bonus_spell_damage: [f64; 8],
    pub(crate) target_bonus_spell_damage_taken: f64,
    pub(crate) target_reduced_crit_taken_percent: f64,
    pub(crate) dot_damage_multiplier_additive: f64,
    pub(crate) target_auto_swing_melee: bool,
    pub(crate) melee: crate::contracts::prepared_v2::Melee,
    pub(crate) melee_haste_rating: f64,
    pub(crate) expertise_percent: f64,
    /// A pet's mana regeneration per second while casting and not, as Go computes it.
    pub(crate) fixed_regen: Option<(f64, f64)>,
    /// Go `RangedHitPercent` and `RangedCritPercent`, which ranged attacks add.
    pub(crate) ranged_hit_percent: f64,
    pub(crate) ranged_crit_percent: f64,
    /// The ranged speed pseudo stat after the reset, and the defender's ranged attack power
    /// bonus, Hunter's Mark.
    pub(crate) ranged_speed_multiplier: f64,
    pub(crate) defender_bonus_ranged_attack_power: f64,
}

/// The pseudo stats Go restores at each reset, after permanent auras applied.
#[derive(Clone, Copy, Debug)]
pub(crate) struct InitialPseudo {
    pub(crate) spell_cost_percent_modifier: i32,
    pub(crate) cast_speed_multiplier: f64,
    pub(crate) spirit_regen_rate_casting: f64,
    pub(crate) spirit_regen_multiplier: f64,
    pub(crate) force_full_spirit_regen: bool,
}

/// The raw MP5 of the player's stats: Go's `AddStatsDynamic` adds each bonus to it when its aura
/// gains and takes the bonus away when it expires, in the order the auras change.
#[derive(Clone, Debug)]
struct Mp5Path {
    value: f64,
    /// The MP5 each stat aura adds, by bit.
    bonuses: Vec<f64>,
}

impl Mp5Path {
    /// The raw MP5 after the auras of the changed bits gained or expired, as the new mask says.
    fn change(&mut self, changed: u32, mask: u32) -> f64 {
        for (bit, bonus) in self.bonuses.iter().enumerate() {
            if changed & (1 << bit) != 0 {
                if mask & (1 << bit) != 0 {
                    self.value += bonus;
                } else {
                    self.value -= bonus;
                }
            }
        }
        self.value
    }
}

/// One major cooldown, in Go's initial order.
#[derive(Clone, Debug)]
pub(crate) struct MajorCooldown {
    pub(crate) spell: SpellId,
    pub(crate) priority: i32,
    pub(crate) explosive: bool,
    /// Go `CooldownTypeSurvival`.
    pub(crate) survival: bool,
    pub(crate) timings: Vec<i64>,
    pub(crate) uses: usize,
}

/// Go spell_data_energize.go `energizedBar`: the bar an item use's gain fills, with the item's
/// own metrics for rage and energy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum EnergizedBar {
    Mana,
    Rage(usize),
    Energy(usize),
}

/// Go `proto.ResourceType` of a resource metric.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResourceKind {
    Mana,
    Health,
    #[allow(dead_code)] // Shared with the Warrior domain in progress.
    Rage,
    Energy,
    ComboPoints,
    Focus,
}

impl ResourceKind {
    /// The Go proto enum name, as Go's result JSON writes it.
    pub(crate) fn proto_name(self) -> &'static str {
        match self {
            ResourceKind::Mana => "ResourceTypeMana",
            ResourceKind::Health => "ResourceTypeHealth",
            ResourceKind::Rage => "ResourceTypeRage",
            ResourceKind::Energy => "ResourceTypeEnergy",
            ResourceKind::ComboPoints => "ResourceTypeComboPoints",
            ResourceKind::Focus => "ResourceTypeFocus",
        }
    }
}

/// Go `ResourceMetrics`.
#[derive(Clone, Debug)]
pub(crate) struct ResourceMetrics {
    pub(crate) id: ActionId,
    /// The unit whose resource it measures.
    pub(crate) unit: Side,
    pub(crate) kind: ResourceKind,
    pub(crate) events: i32,
    pub(crate) gain: f64,
    pub(crate) actual_gain: f64,
    pub(crate) previous_events: i32,
    pub(crate) previous_actual_gain: f64,
    pub(crate) is_mana_regen: bool,
    /// Go `ResourceMetrics.NoThreat`: a gain from a spell the client flags No Threat adds no
    /// threat at the end of the iteration.
    pub(crate) no_threat: bool,
}

/// A target's stats and pseudo stats that auras can change during a fight, reset to the
/// prepared values each iteration.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TargetState {
    /// Go resistance stats by school index; armor at the physical index.
    pub(crate) resistance: [f64; 8],
    /// Go `PseudoStats.SchoolDamageTakenMultiplier`.
    pub(crate) school_damage_taken_multiplier: [f64; 8],
    /// Go `PseudoStats.SchoolBonusSpellDamage`: the spell damage each school's spells gain
    /// against the target.
    pub(crate) school_bonus_spell_damage: [f64; 8],
}

/// One target's mutable state. Every target starts each fight from the same prepared values,
/// since a fight against several holds identical copies.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TargetUnit {
    pub(crate) state: TargetState,
    /// Go `Unit.Armor()`, which the Sunder Armor ramp lowers.
    pub(crate) armor: f64,
    /// Armor changes from auras other than the Sunder Armor ramp, which sets the rest.
    pub(crate) armor_delta: f64,
}

/// Go `Unit.AddDynamicDamageTakenModifier` on the target for a modifier that multiplies the
/// player's damage of the schools in `school_mask` while `aura` is active. Go applies every
/// modifier after the outcome, in registration order.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DamageTakenModifier {
    /// The unit whose spells it multiplies.
    pub(crate) source: Side,
    pub(crate) school_mask: u8,
    pub(crate) aura: AuraRef,
    pub(crate) multiplier: f64,
    /// When set, the multiplier is one plus this for each of the aura's stacks.
    pub(crate) per_stack: Option<f64>,
}

/// Go `Unit.AddDynamicDamageTakenModifier` on the target for a modifier that multiplies the
/// player's damage of the spells in `spells` while any aura in `auras` is active, as a class
/// asking whether one of its dots burns the target does.
#[derive(Clone, Debug)]
pub(crate) struct SpellDamageTakenModifier {
    pub(crate) spells: Vec<bool>,
    pub(crate) auras: Vec<AuraRef>,
    pub(crate) multiplier: f64,
}

/// A scheduled action. Each variant mirrors one Go pending action.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Action {
    EncounterStart,
    ManaTick,
    Rotation(Side),
    Hardcast(Side),
    QueuedCast(Side),
    Travel {
        spell: SpellId,
        result: SpellResult,
        /// A dot the spell applies when it lands.
        dot: Option<DotId>,
    },
    DotTick(DotId),
    /// Go `WaitTravelTime` around a loop of `DealDamage`: the results of one cast that reached
    /// several targets, by their place in `Fight::travel_batches`, dealt in order on arrival.
    TravelBatch(usize),
    /// A class spell's travel callback: [`Agent::on_travel`].
    ClassTravel {
        spell: SpellId,
        result: SpellResult,
    },
    DelayedProc {
        aura: AuraRef,
        spell: SpellId,
        result: SpellResult,
        /// The class's handle on the result's clone, which it disposes of once the proc has
        /// run: [`Agent::clone_result`].
        token: Option<usize>,
    },
    /// The "Pushback trigger" handler, delayed by the spell batch window, with the player's
    /// pushback chance.
    Pushback {
        chance: f64,
    },
    /// A spell data damage proc's handler for the target's swing, which no spell of the
    /// player's carries: the proc's hit on the attacker, a delay after the swing.
    StruckDamageProc {
        proc: usize,
        target: Side,
    },
    /// A rotation prepull action: Go `APLActionCastSpell.Execute`.
    Prepull(SpellId),
    /// A rotation prepull action: Go `APLActionCastFriendlySpell.Execute` at the player.
    PrepullAtPlayer(SpellId),
    /// A rotation prepull action: Go `APLActionChannelSpell.Execute`, with the item holding its
    /// interrupt condition.
    PrepullChannel(SpellId, usize),
    /// A rotation prepull action: Go `APLActionStrictSequence.Execute`, with the item holding
    /// the sequence.
    PrepullSequence(usize),
    /// A rotation prepull action: Go `APLActionItemSwap.Execute`.
    PrepullItemSwap(bool),
    /// apl.go's swap back to Main before the pull.
    PrepullSwapBack,
    /// A rotation prepull action: Go `APLActionChangeTarget.Execute` to the first target.
    PrepullChangeTarget,
    /// A rotation prepull action that runs a list: every stat buff cooldown it casts, or every
    /// proc buff it activates for its set (Swap1 when true).
    PrepullList(bool, usize),
    /// A rotation prepull action: Go `APLActionActivateAura.Execute`.
    PrepullAura(AuraRef),
    /// A rotation prepull action: Go `APLActionCancelAura.Execute`.
    PrepullCancelAura(AuraRef),
    /// A rotation prepull action: Go `APLActionActivateAuraWithStacks.Execute`.
    PrepullAuraWithStacks(AuraRef, i32),
    /// A rotation prepull action: Go `APLActionTriggerICD.Execute`.
    PrepullTriggerIcd(AuraRef),
    /// A rotation prepull action: Go `APLActionMove.Execute`.
    PrepullMove(f64),
    /// A rotation prepull action: Go `APLActionMoveDuration.Execute`.
    PrepullMoveDuration(i64),
    /// A rotation prepull action: Go `APLActionGroupReference.Execute` of a group of moves.
    PrepullGroupMove(f64),
    /// A tick of the raid's Sunder Armor ramp on a target, by its position, with the ticks
    /// done so far.
    SunderTick(u8, i32),
    /// The raid's Expose Armor landing on a target, by its position (Forever's rules).
    ExternalExpose(u8),
    /// Go trackChanceOfDeath's pending action: mark the player dead if health is still gone.
    DeathCheck,
    /// A fixed uptime aura's periodic roll, or its first roll.
    FixedUptime {
        index: usize,
        first: bool,
    },
    /// The party Windfury Totem's periodic refresh.
    WindfuryRefresh,
    /// A tick of a regenerating aura's periodic action, by its position and the generation of
    /// the gain that started it.
    HealthRegenTick {
        regen: usize,
        generation: u32,
    },
    /// The character uses the Oil of Immolation.
    OilUse,
    /// A period of the Oil of Immolation's shield, numbered from 1.
    OilTick(i32),
    /// A tick of an aura's periodic stack adder, with the ticks left including this one.
    StackTick {
        aura: AuraRef,
        period: i64,
        ticks_left: i32,
    },
    /// The end of a unit's movement: Go `MovementAction.OnAction`.
    MovementEnd(Side),
    /// Go `scheduleMeleeWeaveWakeup`'s action: wake a weaver's rotation when its out-of-range
    /// main hand swing comes ready.
    MeleeWeaveWakeup(Side),
    /// buffs.go ApplyFixedShoutAura's chain behind the player's own shout: the comeback a
    /// reaction time after the own aura runs out, then its one periodic tick.
    FixedShoutChain {
        index: usize,
        periodic: bool,
    },
    /// A computed result dealt later: Go `NewDelayedAction` with `DealDamage`.
    DelayedDamage {
        spell: SpellId,
        result: SpellResult,
    },
    /// Go `Unit.ReactToEvent(sim, false, false)` from a pending action.
    React,
    /// A tick of a class periodic action: Go `StartPeriodicAction` without a tick on start.
    ClassPeriodic(Periodic),
    /// A tick of an item's periodic mana gain, Go `StartPeriodicAction` without a tick on start.
    ItemManaTick {
        amount: f64,
        metrics: usize,
        periodic: Periodic,
    },
    /// Go `Pet.statInheritanceAction`: the pet takes its owner's pending stat changes at its
    /// heartbeat.
    PetInheritance(Side),
    /// Go `Pet.timeoutAction`: a summoned pet's time is up.
    PetTimeout(Side),
    /// A class's own pending action, which it may cancel: [`Agent::on_class_action`].
    ClassPending(u32),
}

/// Go `PeriodicActionOptions` for a class periodic action, carried by its pending action.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Periodic {
    /// What the class does on each tick, passed to [`Agent::on_periodic`].
    pub(crate) tag: u32,
    pub(crate) period: i64,
    /// Zero ticks forever, as in Go.
    pub(crate) num_ticks: i32,
    pub(crate) done: i32,
    pub(crate) priority: i32,
}

/// A player pseudo stat an aura multiplies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PseudoStat {
    DamageTaken,
    Threat,
    DamageDealt,
}

/// Go `ActionPriority`.
pub(crate) const PRIORITY_LOW: i32 = -1;
pub(crate) const PRIORITY_GCD: i32 = 0;
pub(crate) const PRIORITY_REGEN: i32 = 1;
pub(crate) const PRIORITY_AUTO: i32 = 2;
pub(crate) const PRIORITY_DOT: i32 = 3;
pub(crate) const PRIORITY_PREPULL: i32 = 10;
/// Go `SpellBatchWindow`.
/// Go `SpellPushbackDuration`.
pub(crate) const SPELL_PUSHBACK_DURATION: i64 = 500 * crate::core::time::NS_PER_MILLISECOND;
pub(crate) const SPELL_BATCH_WINDOW: i64 = 10 * crate::core::time::NS_PER_MILLISECOND;

/// Counters of the changes to what the rotation's cached answers read: each change bumps its
/// kind's counter (see `rotation::reads`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Versions {
    /// The spells' cooldown timers and the GCD.
    pub(crate) timers: u64,
    /// Mana, rage, energy, combo points and health, and their maximums.
    pub(crate) resources: u64,
    /// Any aura's activity, stacks and expiry.
    pub(crate) auras: u64,
    /// The spells' costs.
    pub(crate) spells: u64,
    /// The hardcast, the queued spell, movement, distance and the channel.
    pub(crate) casts: u64,
    /// Class state a class's extra cast conditions read (a stance, a queued strike).
    pub(crate) class: u64,
    /// The energy gains among the resource changes, so a look that waits only for energy can
    /// tell that nothing else moved.
    pub(crate) energy_gains: u64,
}

impl Versions {
    pub(crate) const ZERO: Versions = Versions {
        timers: 0,
        resources: 0,
        auras: 0,
        spells: 0,
        casts: 0,
        class: 0,
        energy_gains: 0,
    };
}

/// What a rotation answer can depend on besides the clock, as bits.
pub(crate) mod reads {
    /// Cooldown timers and the GCD.
    pub(crate) const TIMERS: u8 = 1;
    /// Mana, rage, energy, focus, combo points and health, and their maximums.
    pub(crate) const RESOURCES: u8 = 2;
    /// Any aura's state, dots and exclusive categories included.
    pub(crate) const AURAS: u8 = 4;
    /// The hardcast, the queued spell, movement, distance and the channel.
    pub(crate) const CASTS: u8 = 8;
    /// The execute phase.
    pub(crate) const PHASE: u8 = 16;
    /// The spells' costs.
    pub(crate) const SPELLS: u8 = 32;
    /// Class state a class's extra cast conditions read.
    pub(crate) const CLASS: u8 = 64;
    pub(crate) const ALL: u8 = TIMERS | RESOURCES | AURAS | CASTS | PHASE | SPELLS | CLASS;
}

pub(crate) struct Fight<A: Agent> {
    pub(crate) agent: A,
    /// Changes to what the rotation's cached answers read.
    pub(crate) versions: Versions,
    pub(crate) config: Config,
    pub(crate) now: i64,
    pub(crate) duration: i64,
    end_of_combat: i64,
    pull_started_at: i64,
    /// Go `executePhase` and `nextExecuteDuration` for a fight timed by duration.
    pub(crate) execute_phase: i32,
    next_execute: i64,
    pub(crate) rng: SimRng,
    pub(crate) queue: PendingQueue<Action>,
    min_tracker_time: i64,
    /// Go `sim.minTaskTime`.
    min_task_time: i64,
    /// Go `energyBar`, for a player that has one.
    pub(crate) energy: Option<energy::EnergyBar>,
    /// The player as the defender of its own spells, when a spell can hit the player.
    pub(crate) self_target: Option<damage_taken::SelfTarget>,
    /// The Goblin Sapper Charge, when the character has it.
    pub(crate) goblin_sapper: Option<damage_taken::GoblinSapper>,
    /// Go `UnitMetrics.Died` for the player.
    pub(crate) death: damage_taken::Death,
    /// Each target's swing at the player, by position, when the player tanks them: Go points
    /// every copy of the boss at the tank. Empty otherwise.
    pub(crate) enemies: Vec<enemy::EnemyAttack>,
    /// An untanked target's swing speed and melee haste, for the timer its parry haste reads.
    pub(crate) untanked_swing: Option<(f64, f64)>,
    /// Auras Go keeps up through `ApplyFixedUptimeAura`.
    pub(crate) fixed_uptime: Vec<FixedUptime>,
    /// Item procs that restore energy.
    pub(crate) energize_procs: Vec<energy::EnergizeProc>,
    pub(crate) health_on_cast: Vec<HealthOnCast>,
    pub(crate) judgements_of_light: Vec<JudgementOfLight>,
    /// Go `Unit.JowManaMetrics` of the player, which takes the action of the first Judgement
    /// of Wisdom that restores its mana: whether one has.
    pub(crate) jow_metrics_named: bool,
    /// Listeners on the player's casts that restore mana: [`Effect::ManaOnCast`].
    pub(crate) mana_on_cast: Vec<energy::ManaOnCast>,
    /// A healer's stacking on-use trinkets: [`Effect::StackingCdOnUse`].
    pub(crate) stacking_cds: Vec<healer_gear::StackingCd>,
    /// A healer's on-use shields on heals: [`Effect::ShieldOnHeal`].
    pub(crate) shields_on_heal: Vec<healer_gear::ShieldOnHeal>,
    /// A healer's instant cast auras: [`Effect::InstantCastAura`].
    pub(crate) instant_casts: Vec<healer_gear::InstantCast>,
    /// Go `rageBar`, for a player that has one.
    pub(crate) rage: Option<rage::RageBar>,
    /// Single aura exclusive categories the runtime enforces.
    pub(crate) exclusive: Vec<exclusive::Category>,
    /// The exclusive categories the runtime only tracks for their uptime metrics: Go runs
    /// their transitions, but no member blocks another's activation.
    pub(crate) exclusive_tracking: Vec<exclusive::Category>,
    /// Each aura's exclusive effects in Go's order, in the enforced or tracked categories.
    pub(crate) aura_effects: HashMap<AuraRef, Vec<exclusive::EffectRef>>,
    /// Each unit's exported exclusive memberships, by aura, for the first run to resolve.
    pub(crate) exported_memberships: Vec<(AuraRef, Vec<ExclusiveMembership>)>,
    /// The target's major armor category and its armor at each stack count of the active
    /// member.
    pub(crate) armor_category: Option<(usize, Vec<f64>)>,
    /// The copies of the major armor category on the targets past the first.
    pub(crate) extra_armor_categories: Vec<(usize, Side)>,
    /// Player auras that multiply the player's damage taken, by aura index.
    pub(crate) damage_taken_auras: Vec<(usize, f64, PseudoStat)>,
    /// Set while the reset activates permanent auras, whose pseudo stats the prepared values
    /// already hold.
    pub(crate) resetting_auras: bool,
    /// The last hit on the player's resistance multiplier and the damage after it, as Go
    /// `SpellResult` carries them for rage from damage taken.
    pub(crate) player_hit_resistance: (f64, f64),
    pub(crate) player: Player,
    /// Go `Unit.CastSpeed`. Go's unit reset restores the pseudo stats but not this value,
    /// so a speed change undone at the end of a fight carries into the next one.
    pub(crate) cast_speed: f64,
    /// Each unit's auras, by [`Side::index`].
    pub(crate) trackers: Vec<Tracker<A::Aura>>,
    /// Each target's mutable stats and armor, by position.
    pub(crate) targets: Vec<TargetUnit>,
    /// Whether the targets past the first hold their copies of the first's auras, dots and
    /// exclusive categories, which the first run makes once the class finished the fight.
    extra_targets_built: bool,
    /// Each dot's copies on the targets past the first, in target order; a dot on the caster
    /// has none.
    pub(crate) dot_copies: Vec<Vec<DotId>>,
    /// By the first dot of an aura, the later dots that share it: Go registers an aura once for
    /// each label.
    pub(crate) dot_siblings: Vec<Vec<DotId>>,
    /// The results of casts in flight to several targets, for [`Action::TravelBatch`], and the
    /// places free for reuse.
    pub(crate) travel_batches: Vec<Vec<(SpellId, SpellResult)>>,
    free_travel_batches: Vec<usize>,
    /// The target's dynamic damage taken modifiers.
    pub(crate) damage_taken_modifiers: Vec<DamageTakenModifier>,
    /// Go `doneIteration` is deactivating the auras: an iteration is ending.
    pub(crate) done_iteration: bool,
    /// The aura the tracker is deactivating because its duration ran out, for a class that tells
    /// an aura fading by time from one cut short (a refresh, a consume, the iteration's end).
    pub(crate) expiring_by_duration: Option<AuraRef>,
    /// Spell-conditioned modifiers, applied after `damage_taken_modifiers`; no class registers
    /// both kinds, so Go's single registration order is kept.
    pub(crate) spell_damage_taken_modifiers: Vec<SpellDamageTakenModifier>,
    /// Go `healthBar.DamageTakenHealthMetrics` for the player.
    damage_taken_health: usize,
    pub(crate) spells: Vec<Spell<A::Spell>>,
    pub(crate) dots: Vec<Dot>,
    pub(crate) mods: Vec<spell_mod::SpellMod>,
    /// Crit modifiers a dot's tick does not take, as a priest's Inner Focus (community #793):
    /// while active, a tick of a spell it names rolls crit with its chance taken back.
    pub(crate) tick_crit_takebacks: Vec<(spell_mod::ModId, Vec<bool>, f64)>,
    pub(crate) timers: Vec<i64>,
    pub(crate) major_cooldowns: Vec<MajorCooldown>,
    pub(crate) cooldown_order: Vec<usize>,
    cooldown_min_ready: i64,
    pub(crate) rotation: Vec<rotation::Item>,
    /// The player's current target, by position: Go `Character.CurrentTarget`, which a change
    /// of target moves.
    pub(crate) player_target: usize,
    /// For a rotation that changes target, the copy of the rotation compiled for each current
    /// target by position, the one in place left empty; none otherwise.
    pub(crate) rotation_copies: Vec<Vec<rotation::Item>>,
    /// How many of the items are the priority list's; the rest are the group instances'.
    pub(crate) priority_len: usize,
    /// Where each group instance's items are.
    pub(crate) group_ranges: Vec<std::ops::Range<usize>>,
    /// A Warlock's assigned curse: the curse option and the curse spell's action.
    pub(crate) assigned_curse: Option<(String, ActionId)>,
    /// What the rotation's values read that the rest of the input leaves out.
    pub(crate) rotation_reads: Option<crate::contracts::prepared_v2::RotationReads>,
    /// The priority list's sequences by name, which reset sequence actions and sequence values
    /// find.
    pub(crate) rotation_sequences: Vec<(std::rc::Rc<str>, usize)>,
    /// Prepull casts by time, in Go's stable time order.
    prepull: Vec<(i64, rotation::PrepullAct)>,
    /// The spells and auras of the prepull actions that run several, by position.
    prepull_lists: Vec<rotation::PrepullList>,
    /// The dot base durations the rotation's `dotBaseDuration` values captured, by spell.
    dot_base_durations: Vec<(ActionId, i64)>,
    in_rotation: bool,
    /// Go `APLRotation` state for sequences and channels.
    pub(crate) apl: rotation::AplState,
    pub(crate) resources: Vec<ResourceMetrics>,
    pub(crate) actions: Vec<ActionTotals>,
    /// Each target's registered actions, by position. An untanked target never acts, so their
    /// metrics stay zero; a tanked one's swing fills its own.
    pub(crate) target_actions: Vec<Vec<ActionTotals>>,
    /// The player's weapon attacks.
    pub(crate) autos: melee::AutoAttacks,
    sunder: Option<SunderRamp>,
    external_expose: Option<ExternalExpose>,
    /// Gnome's Eureka!, when the character has it.
    pub(crate) eureka: Option<racial::Eureka>,
    /// Registered pets that nothing summons.
    pub(crate) pets: Vec<pet::InertPet>,
    /// The pet enabled at each reset, which Rust simulates.
    /// The simulated pets, in unit index order: `Side::Pet(i)` is the one at `i`.
    pub(crate) active_pets: Vec<pet::ActivePet>,
    /// Go `sim.trackers` after the target's and the player's: the enabled pets' trackers, in
    /// the order Go added them; a disabled pet's tracker leaves by swap removal.
    pub(crate) pet_trackers: Vec<u8>,
    /// Go `Environment.heartbeatOffset`, from which pet stat inheritance heartbeats count.
    pub(crate) heartbeat_offset: i64,
    /// The auras of each of the player's pets, in Go registration order, for the rotation's
    /// pet source units.
    pub(crate) pet_agent_auras: Vec<Vec<ActionId>>,
    /// The labels of those pets, in the same order.
    pub(crate) pet_agent_labels: Vec<String>,
    /// Whether each of those pets has a mana bar, in the same order.
    pub(crate) pet_agent_mana_bars: Vec<bool>,
    /// The full mana an inert one of those pets states, in the same order.
    pub(crate) pet_agent_max_mana: Vec<Option<f64>>,
    /// Go `APLRotation.curValidations` while [`Self::presentation`] builds the rotation's
    /// lines again to collect Go's validation messages; `None` the rest of the time.
    pub(crate) checks: std::cell::RefCell<Option<Vec<rotation_checks::Check>>>,
    /// The rotation as the application shows it, with Go's validation messages of each
    /// line, built with the rotation.
    pub(crate) presentation: Option<rotation_checks::Presentation>,
    /// The logged fight's queued strikes that went out on an extra attack, for the
    /// application's timeline: when, the strike and the extra attack it replaced.
    pub(crate) extra_attack_strikes: Vec<serde_json::Value>,
    mana_regen_casting: usize,
    mana_regen_not_casting: usize,
    mana_gain_spell: Option<SpellId>,
    pub(crate) log: Option<Vec<String>>,
    /// Lines auras log when gained, by index.
    pub(crate) aura_logs: Vec<String>,
    /// The Crusader enchant, when a weapon carries it.
    pub(crate) crusader: Option<Crusader>,
    /// Dragon's Call and the Emerald Dragon Whelp it summons.
    pub(crate) whelp: Option<whelp::Whelp>,
    pub(crate) sulfuras: Option<sulfuras::Sulfuras>,
    pub(crate) blade_of_hanna: Option<blade_of_hanna::BladeOfHanna>,
    /// The slows of the attack speed category, by the position of their aura on the target.
    pub(crate) enemy_slows: Vec<(usize, enemy::EnemySlow)>,
    /// Thunderfury's proc, when worn.
    pub(crate) thunderfury: Option<thunderfury::Thunderfury>,
    /// The party Windfury Totem.
    pub(crate) windfury: Option<Windfury>,
    /// The generated buffs other players cast on the player on cooldown.
    pub(crate) external_cooldowns: Vec<external_cooldown::ExternalCooldown>,
    /// The auras other than the druid's own that carry Innervate's regeneration.
    innervate_regens: Vec<innervate::InnervateRegen>,
    /// Forever's Read Ley Line, which an Innervate blocks and replaces.
    ley_line_innervate: Option<innervate::LeyLineInnervate>,
    /// The Innervate regeneration auras up on the player, the druid's own included.
    innervates_up: u32,
    /// What Power Infusion multiplies, when the player has a copy of its aura.
    power_infusion: Option<power_infusion::PowerInfusion>,
    /// The player's healing dealt multiplier relative to its exported value, which Power
    /// Infusion moves while a copy of the aura holds the category.
    pub(crate) healing_dealt_factor: f64,
    /// A crit chance, as a fraction, the next heal rolls on top of its spell's: set by a class
    /// for the one heal it names (Renewed Hope) and put back to zero after it.
    pub(crate) heal_crit_bonus: f64,
    /// Dragonbreath Chili, when the character ate it.
    pub(crate) chili: Option<DragonbreathChili>,
    /// Enchant Weapon - Revelation, when a weapon carries it.
    pub(crate) revelation: Option<revelation::Revelation>,
    /// Go `AttackTable.MobTypeBonusStats`' spell damage, which only the Potion of Elemental
    /// Purging gives: its aura and the bonus the player's spells take while it holds, when the
    /// targets are of its mob type.
    pub(crate) mob_type_spell_damage: Option<(AuraRef, f64)>,
    /// Go `AttackTable.MobTypeBonusStats` from Mark of the Champion: each aura and its ranged
    /// attack power (which Go's melee attack power reads too) and spell damage bonus.
    pub(crate) mob_type_bonuses: Vec<(AuraRef, f64, f64)>,
    /// Under Forever's rules, the auras whose melee attack power holds against the targets'
    /// type alone, and that attack power.
    pub(crate) mob_type_melee_bonuses: Vec<(AuraRef, f64)>,
    /// Item damage procs built from client rows, by their aura's position.
    pub(crate) damage_procs: Vec<DamageProc>,
    /// Enchant heal procs, by their position among the heal proc effects.
    pub(crate) heal_procs: Vec<heal_proc::HealProc>,
    /// Absorb shields: the item uses' by their position among the absorb effects, then the
    /// absorb procs'.
    pub(crate) item_absorbs: Vec<absorb::ItemAbsorb>,
    /// Absorb procs on the melee hits the player takes, by their position among the effects.
    pub(crate) absorb_procs: Vec<absorb::AbsorbProc>,
    /// Set bonus stat procs: each spell's chance, the roll's label and the aura activated.
    pub(crate) stat_procs: Vec<StatProc>,
    /// Gear procs that heal and give rage, by their aura's position.
    pub(crate) health_rage_procs: Vec<gear_procs::HealthRageProc>,
    pub(crate) energy_procs: Vec<gear_procs::EnergyProc>,
    pub(crate) health_regens: Vec<health_regen::HealthRegen>,
    /// The Oil of Immolation, when the character uses it.
    pub(crate) oil: Option<oil_of_immolation::OilOfImmolation>,
    /// Gear procs that stack an armor debuff on the target, by their aura's position.
    pub(crate) armor_debuff_procs: Vec<gear_procs::ArmorDebuffProc>,
    /// Spell data stat procs, by their position among the effects of their kind.
    pub(crate) spell_stat_procs: Vec<SpellStatProc>,
    /// The spell mods each spell cost aura applies while active.
    pub(crate) aura_mods: Vec<Vec<super::fight::spell_mod::ModId>>,
    /// The player's stats for each combination of active stat auras, by bit mask.
    pub(crate) stat_combos: Vec<Powers>,
    /// The player's maximum health for each combination, when one changes it.
    pub(crate) stat_health: Vec<Option<f64>>,
    /// The player's Spirit for each combination, when one changes it.
    pub(crate) stat_spirit: Vec<Option<f64>>,
    /// The player's Intellect for each combination, when one changes it: Go refreshes the
    /// cached mana regeneration rates whenever a dynamic change moves it.
    stat_intellect: Vec<Option<f64>>,
    /// The crit the player takes by combination, when one changes it: the Goblin Sapper Charge's
    /// hit on its thrower reads it.
    stat_crit_taken: Vec<Option<f64>>,
    /// The player's school spell damage stats for each combination, when one changes them.
    pub(crate) stat_school_damage: Vec<Option<[f64; 8]>>,
    /// The player's resistances for each combination that a combination changes, by Go
    /// school index.
    pub(crate) stat_resistance: Vec<Vec<(usize, f64)>>,
    /// The raw MP5 of the stats, when an aura adds a bonus that is not a whole number.
    mp5_path: Option<Mp5Path>,
    /// Go `ItemSwap`, when the character swaps items.
    pub(crate) item_swap: Option<item_swap::ItemSwapState>,
    /// The active stat auras.
    pub(crate) stat_mask: u32,
    /// The labels of the stat auras, bit by bit.
    pub(crate) stat_aura_labels: Vec<String>,
    pub(crate) totals: metrics::Totals,
    pub(crate) encounter_damage_taken: f64,
    /// Go `isInPrepull`, which holds through the reset.
    pub(crate) in_prepull: bool,
    /// Go `StartDistanceFromTarget` of the player, which every reset restores.
    start_distance: f64,
    /// Go `PseudoStats.MovementSpeedMultiplier` of the player, and its value after the reset.
    pub(crate) move_multiplier: f64,
    initial_move_multiplier: f64,
    /// The multiplier the exporter read after the reset, which the permanent auras' effects must
    /// leave it at.
    reset_move_multiplier: f64,
    /// The movement speed categories the player holds auras of, by name, and once built the
    /// enforced categories of them whose active effect multiplies the speed.
    movement_category_names: Vec<&'static str>,
    pub(crate) movement_categories: Vec<usize>,
    /// The player's health when a fight starts, where it differs from the maximum.
    health_at_reset: Option<f64>,
    /// Go `HpPercentForDefensives`, below which survival major cooldowns fire.
    pub(crate) hp_percent_for_defensives: f64,
    /// Forever's rule for casting regeneration auras (see [`casting_regen`]).
    pub(crate) casting_regen_at_once: bool,
    /// Forever's rule that a rotation's move waits for a queued spell to fire.
    pub(crate) move_waits_for_queued_cast: bool,
    /// Forever's rule that a melee swing leaves the Auto Shot timer alone.
    pub(crate) melee_keeps_auto_shot_timer: bool,
    /// Forever's rule that a move whose end time has come reads as arrived.
    pub(crate) move_arrives_at_destination: bool,
    /// Forever's rule that a rotation with a Move keeps looking through the global cooldown:
    /// set only while the rule is on and the rotation holds a Move.
    pub(crate) rotation_with_moves_looks_through_gcd: bool,
    /// Forever's rule that the off hand's half swing wait counts from the melee attack's start.
    pub(crate) off_hand_waits_from_attack_start: bool,
    /// Forever's rule that autocasting passes over a cooldown the GCD holds back.
    pub(crate) autocast_skips_unready_cooldowns: bool,
    /// Forever's rule that a damage bonus applied after the calculation raises threat too.
    pub(crate) threat_follows_damage_bonus: bool,
    /// A healer sim's burst healing window under Forever's rules (see [`burst`]).
    pub(crate) burst: Option<burst::BurstWindow>,
    /// How each exclusive effect of an aura a rotation asks about reads.
    pub(crate) aura_refresh: Vec<(AuraRef, Vec<exclusive::RefreshReading>)>,
    /// Single aura categories that a rotation's `auraShouldRefresh` reads, or a class asks for,
    /// and no class describes, as (unit, name): the runtime enforces them from the exported
    /// memberships.
    derived_categories: Vec<(Side, String)>,
}

/// Go movement.go's exclusive categories of movement speed effects: `NewPassiveMovementSpeedEffect`
/// and `NewActiveMovementSpeedEffect` bid their bonus in single aura categories, and the active
/// effect multiplies the unit's speed.
const MOVEMENT_CATEGORIES: [&str; 2] = ["PassiveMovementSpeed", "ActiveMovementSpeed"];

/// Errors that make a prepared input impossible to run despite passing coverage.
pub(crate) type BuildError = String;

/// A proc manager's chances by spellbook position: each spell it names, none for the rest.
pub(crate) fn chances_by_spell(
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

/// The damage proc a spell data damage proc or a weapon's extra attack proc runs, with its
/// trigger spells and its proc manager's chances by spell, or none for another effect.
pub(crate) fn damage_proc_of(effect: &Effect, spells: usize) -> Option<DamageProc> {
    let mut delay = None;
    let mut rng_label = None;
    let (trigger_spells, struck, casts, landed_only, require_damage, proc_chance, spell, chances) =
        match effect {
            Effect::SpellDataDamageProc {
                trigger_spells,
                struck,
                casts,
                landed_only,
                require_damage,
                proc_chance,
                spell,
                chances,
                delay_ns,
                rng_label: label,
                ..
            } => {
                delay = *delay_ns;
                rng_label = label.clone();
                (
                    trigger_spells.clone(),
                    *struck,
                    *casts,
                    *landed_only,
                    *require_damage,
                    *proc_chance,
                    *spell,
                    chances,
                )
            }
            // A weapon proc that casts the spell granting the extra attacks: it hears the
            // landed hits its proc manager has a chance for.
            Effect::ExtraAttackProc {
                chances: chances @ Some(entries),
                spell: Some(spell),
                ..
            } => (
                entries.iter().map(|entry| entry.spell).collect(),
                false,
                false,
                true,
                false,
                1.0,
                *spell,
                chances,
            ),
            _ => return None,
        };
    let mut mask = vec![false; spells];
    for &trigger in &trigger_spells {
        if let Some(slot) = mask.get_mut(trigger) {
            *slot = true;
        }
    }
    let chances = chances
        .as_ref()
        .map(|chances| chances_by_spell(chances, spells));
    Some(DamageProc {
        trigger_spells: mask,
        struck,
        casts,
        landed_only,
        require_damage,
        chance: proc_chance,
        chances,
        spell,
        delay,
        rng_label,
    })
}

/// Whether a stat aura label is the item swap's own, whose bit carries the swap's equipment stats
/// and which the swap sets itself.
fn item_swap_stats_label(effects: &[Effect], label: &str) -> bool {
    effects.iter().any(
        |effect| matches!(effect, Effect::ItemSwap { swapped_aura, .. } if swapped_aura == label),
    )
}

/// The gain and expiry lines of a stat aura an item swap switches, when it logs one of each.
fn item_swap_aura_logs(effects: &[Effect], label: &str) -> Option<(String, String)> {
    effects.iter().find_map(|effect| match effect {
        Effect::ItemSwap { stat_aura_logs, .. } => stat_aura_logs
            .iter()
            .find(|logs| logs.aura == label)
            .and_then(
                |logs| match (logs.gain_logs.as_slice(), logs.expire_logs.as_slice()) {
                    ([gain], [expire]) => Some((gain.clone(), expire.clone())),
                    _ => None,
                },
            ),
        _ => None,
    })
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

/// Go `SpellSchoolFrostfire`: Fire and Frost.
pub(crate) const SCHOOL_FROSTFIRE: u8 = 4 | 16;
/// Go `stats.SchoolIndexFire` and `SchoolIndexFrost`.
const SCHOOL_INDEX_FIRE: usize = 3;
/// The school spell damage stats by Go school index.
const SCHOOL_DAMAGE_STATS: [(usize, &str); 6] = [
    (2, "ArcaneDamage"),
    (3, "FireDamage"),
    (4, "FrostDamage"),
    (5, "HolyDamage"),
    (6, "NatureDamage"),
    (7, "ShadowDamage"),
];

/// The Go school index of a school spell damage stat, such as 7 for `ShadowDamage`.
pub(crate) fn school_damage_index(stat: &str) -> Option<usize> {
    SCHOOL_DAMAGE_STATS
        .iter()
        .find(|(_, name)| *name == stat)
        .map(|(index, _)| *index)
}
/// The resistance stats by Go school index.
const RESISTANCE_STATS: [(usize, &str); 5] = [
    (2, "ArcaneResistance"),
    (3, "FireResistance"),
    (4, "FrostResistance"),
    (6, "NatureResistance"),
    (7, "ShadowResistance"),
];
pub(crate) const SCHOOL_INDEX_FROST: usize = 4;

/// Go `SpellSchool` to `SchoolIndex`, in Go's switch order.
pub(crate) fn school_index(school: u8) -> usize {
    match school {
        s if s & 1 != 0 => 1,
        s if s & 64 != 0 => 2,
        s if s & 4 != 0 => 3,
        s if s & 16 != 0 => 4,
        s if s & 2 != 0 => 5,
        s if s & 8 != 0 => 6,
        s if s & 32 != 0 => 7,
        _ => 0,
    }
}

fn stat(stats: &BTreeMap<String, f64>, name: &str) -> Result<f64, BuildError> {
    stats
        .get(name)
        .copied()
        .ok_or_else(|| format!("prepared stats lack {name}"))
}

/// The exported fields one acting unit's configuration reads: the player's, or its pet's.
pub(crate) struct UnitSource<'a> {
    pub(crate) label: &'a str,
    pub(crate) name: &'a str,
    pub(crate) level: i32,
    pub(crate) reaction_ns: i64,
    pub(crate) channel_clip_delay_ns: i64,
    pub(crate) distance: f64,
    pub(crate) cast_speed: f64,
    pub(crate) stats: &'a BTreeMap<String, f64>,
    pub(crate) pseudo: &'a crate::contracts::prepared_v2::PseudoStats,
    pub(crate) table: &'a crate::contracts::prepared_v2::AttackTable,
    pub(crate) melee: &'a crate::contracts::prepared_v2::Melee,
    pub(crate) max_mana: f64,
    pub(crate) teardown_max: f64,
    pub(crate) spirit_regen_per_second: f64,
    pub(crate) fixed_regen: Option<(f64, f64)>,
}

/// One acting unit's configuration, with the fight's own settings and the target's.
pub(crate) fn unit_config(prepared: &PreparedV2, unit: &UnitSource) -> Result<Config, BuildError> {
    let target = &prepared.target;
    let pseudo = unit.pseudo;
    let target_pseudo = &target.pseudo_stats;
    let resistance = |name: &str| stat(&target.stats, name);
    Ok(Config {
        iterations: prepared.sim.iterations,
        seed: prepared.sim.seed,
        debug_first_iteration: prepared.sim.debug_first_iteration,
        debug: prepared.sim.debug,
        save_all_values: prepared.sim.save_all_values,
        base_duration: prepared.encounter.duration_ns,
        duration_variation: prepared.encounter.duration_variation_ns,
        damage_taken_multiplier: pseudo.damage_taken_multiplier,
        school_damage_taken_multiplier: schools(&pseudo.school_damage_taken_multiplier),
        execute_proportions: [
            prepared.encounter.execute_proportion_90,
            prepared.encounter.execute_proportion_45,
            prepared.encounter.execute_proportion_35,
            prepared.encounter.execute_proportion_25,
            prepared.encounter.execute_proportion_20,
        ],
        player_label: unit.label.to_string(),
        player_name: unit.name.to_string(),
        target_label: target.label.clone(),
        player_index: prepared.player.index,
        player_level: unit.level,
        target_level: target.level,
        level_partial_resist: !target.no_level_partial_resist,
        reaction: unit.reaction_ns,
        channel_clip_delay: unit.channel_clip_delay_ns,
        distance: unit.distance,
        cast_speed: unit.cast_speed,
        spell_haste_rating: stat(unit.stats, "SpellHasteRating")?,
        max_mana: unit.max_mana,
        max_health: stat(unit.stats, "Health")?,
        teardown_max_mana: unit.teardown_max,
        spirit_regen_per_second: unit.spirit_regen_per_second,
        powers: Powers {
            spell_crit_percent: stat(unit.stats, "SpellCritPercent")?,
            physical_crit_percent: stat(unit.stats, "PhysicalCritPercent")?,
            spell_damage: stat(unit.stats, "SpellDamage")?,
            attack_power: stat(unit.stats, "AttackPower")?,
            ranged_attack_power: stat(unit.stats, "RangedAttackPower")?,
            mp5: stat(unit.stats, "MP5")?,
            max_mana: unit.max_mana,
            healing_power: unit.stats.get("HealingPower").copied().unwrap_or(0.0),
            spirit_regen_per_second: unit.spirit_regen_per_second,
            physical_damage: stat(unit.stats, "PhysicalDamage")?,
            armor_penetration: stat(unit.stats, "ArmorPenetration")?,
            spell_piercing: stat(unit.stats, "SpellPiercing")?,
            physical_hit_percent: stat(unit.stats, "PhysicalHitPercent")?,
            spell_hit_percent: stat(unit.stats, "SpellHitPercent")?,
        },
        school_damage: [
            0.0,
            0.0,
            stat(unit.stats, "ArcaneDamage")?,
            stat(unit.stats, "FireDamage")?,
            stat(unit.stats, "FrostDamage")?,
            stat(unit.stats, "HolyDamage")?,
            stat(unit.stats, "NatureDamage")?,
            stat(unit.stats, "ShadowDamage")?,
        ],
        initial: InitialPseudo {
            spell_cost_percent_modifier: pseudo.spell_cost_percent_modifier,
            cast_speed_multiplier: pseudo.cast_speed_multiplier,
            spirit_regen_rate_casting: pseudo.spirit_regen_rate_casting,
            spirit_regen_multiplier: pseudo.spirit_regen_multiplier,
            force_full_spirit_regen: pseudo.force_full_spirit_regen,
        },
        school_bonus_hit_chance: schools(&pseudo.school_bonus_hit_chance),
        damage_dealt_multiplier: pseudo.damage_dealt_multiplier,
        school_damage_dealt_multiplier: schools(&pseudo.school_damage_dealt_multiplier),
        crit_damage_multiplier: pseudo.crit_damage_multiplier,
        threat_multiplier: pseudo.threat_multiplier,
        table: unit.table.clone(),
        target_resistance: [
            0.0,
            stat(&target.stats, "Armor")?,
            resistance("ArcaneResistance")?,
            resistance("FireResistance")?,
            resistance("FrostResistance")?,
            0.0,
            resistance("NatureResistance")?,
            resistance("ShadowResistance")?,
        ],
        target_damage_taken_multiplier: target_pseudo.damage_taken_multiplier,
        target_school_damage_taken_multiplier: schools(
            &target_pseudo.school_damage_taken_multiplier,
        ),
        target_school_bonus_spell_damage: schools(&target_pseudo.school_bonus_spell_damage),
        target_bonus_spell_damage_taken: target_pseudo.bonus_spell_damage_taken,
        target_reduced_crit_taken_percent: target_pseudo.reduced_crit_taken_percent,
        dot_damage_multiplier_additive: pseudo.dot_damage_multiplier_additive,
        target_auto_swing_melee: target.auto_swing_melee,
        melee: unit.melee.clone(),
        melee_haste_rating: stat(unit.stats, "MeleeHasteRating")?,
        expertise_percent: stat(unit.stats, "ExpertisePercent")?,
        fixed_regen: unit.fixed_regen,
        ranged_hit_percent: stat(unit.stats, "RangedHitPercent")?,
        ranged_crit_percent: stat(unit.stats, "RangedCritPercent")?,
        ranged_speed_multiplier: unit
            .melee
            .ranged_state
            .as_ref()
            .map_or(1.0, |ranged| ranged.ranged_speed_multiplier),
        defender_bonus_ranged_attack_power: unit
            .melee
            .ranged_state
            .as_ref()
            .map_or(0.0, |ranged| ranged.defender_bonus_ranged_attack_power),
    })
}

impl<A: Agent> Fight<A> {
    /// Build a fight from a prepared input that passed [`crate::check_prepared`].
    /// `class_spell` and `class_aura` map exported entries to class behavior.
    pub(crate) fn new(
        prepared: &PreparedV2,
        rng_mode: RngMode,
        agent: A,
        class_spell: impl Fn(&crate::contracts::prepared_v2::Spell) -> Option<A::Spell>,
        class_aura: impl Fn(&str, &str) -> Option<A::Aura>,
    ) -> Result<Self, BuildError> {
        let player = &prepared.player;
        let target = &prepared.target;
        let config = unit_config(
            prepared,
            &UnitSource {
                label: &player.label,
                name: &player.name,
                level: player.level,
                reaction_ns: player.reaction_ns,
                channel_clip_delay_ns: player.channel_clip_delay_ns,
                distance: player.distance_yards,
                cast_speed: player.cast_speed,
                stats: &player.stats,
                pseudo: &player.pseudo_stats,
                table: &player.attack_table,
                melee: &prepared.melee,
                max_mana: player.mana.max,
                teardown_max: player.mana.teardown_max,
                spirit_regen_per_second: player.mana.spirit_regen_per_second,
                fixed_regen: None,
            },
        )?;

        let mut timer_names: Vec<String> = Vec::new();
        let mut timer = |name: &str| -> TimerId {
            match timer_names.iter().position(|existing| existing == name) {
                Some(index) => index,
                None => {
                    timer_names.push(name.to_string());
                    timer_names.len() - 1
                }
            }
        };

        let mut resources = Vec::new();
        let mut resource = |unit: Side, id: ActionId, regen: bool, kind: ResourceKind| {
            resources.push(ResourceMetrics {
                id,
                unit,
                kind,
                events: 0,
                gain: 0.0,
                actual_gain: 0.0,
                previous_events: 0,
                previous_actual_gain: 0.0,
                is_mana_regen: regen,
                no_threat: false,
            });
            resources.len() - 1
        };
        let regen_id = |tag| ActionId {
            other_id: "OtherActionManaRegen".into(),
            tag,
            ..ActionId::default()
        };
        let mana_regen_casting = resource(Side::Player, regen_id(1), false, ResourceKind::Mana);
        let mana_regen_not_casting = resource(Side::Player, regen_id(2), false, ResourceKind::Mana);
        let pet_regen: Vec<(usize, usize)> = (0..prepared.pets.len())
            .map(|pet| {
                let side = Side::Pet(pet as u8);
                (
                    resource(side, regen_id(1), false, ResourceKind::Mana),
                    resource(side, regen_id(2), false, ResourceKind::Mana),
                )
            })
            .collect();
        // Go EnableRageBar, in the agent constructor before the health bar: damage taken,
        // then refund and encounter start metrics.
        let other = |name: &str| ActionId {
            other_id: name.into(),
            ..ActionId::default()
        };
        let mut rage = prepared.effects.iter().find_map(|effect| match effect {
            Effect::RageBar {
                max_rage,
                starting_rage,
                main_hand_rage,
                off_hand_rage,
                main_hand_crit_rage,
                off_hand_crit_rage,
                threat_per_rage,
                threat_multiplied,
                crit_multiplies_swing,
                ..
            } => Some(rage::RageBar {
                max: max_rage.max(100.0),
                starting: starting_rage.clamp(0.0, max_rage.max(100.0)),
                current: 0.0,
                main_hand_rage: *main_hand_rage,
                off_hand_rage: *off_hand_rage,
                main_hand_crit_rage: *main_hand_crit_rage,
                off_hand_crit_rage: *off_hand_crit_rage,
                threat_per_rage: *threat_per_rage,
                threat_multiplied: *threat_multiplied,
                crit_multiplies_swing: *crit_multiplies_swing,
                gain_threat: Vec::new(),
                damage_taken_metrics: 0,
                refund_metrics: 0,
                damage_taken_multiplier: 1.0,
                gain_spell: None,
                in_use: true,
            }),
            _ => None,
        });
        if let Some(bar) = rage.as_mut() {
            bar.damage_taken_metrics = resource(
                Side::Player,
                other("OtherActionDamageTaken"),
                false,
                ResourceKind::Rage,
            );
            bar.refund_metrics = resource(
                Side::Player,
                other("OtherActionRefund"),
                false,
                ResourceKind::Rage,
            );
            resource(
                Side::Player,
                other("OtherActionEncounterStart"),
                false,
                ResourceKind::Rage,
            );
        }
        let damage_taken_health = resource(
            Side::Player,
            ActionId {
                other_id: "OtherActionDamageTaken".into(),
                ..ActionId::default()
            },
            false,
            ResourceKind::Health,
        );

        let effects = &prepared.effects;
        let mut spells = Vec::new();
        let mut dots = Vec::new();
        let mut mana_gain_spell = None;
        let mut pet_mana_gain_spell: Vec<Option<SpellId>> = vec![None; prepared.pets.len()];
        // Spells that activate a player aura, resolved once the auras are registered.
        let mut activations: Vec<(SpellId, &str)> = Vec::new();
        // Potions with a temporary stat aura, resolved once the auras are registered.
        let mut potion_auras: Vec<(SpellId, &str)> = Vec::new();
        // Diamond Flasks and the Strength aura their last tick activates.
        let mut flask_auras: Vec<(SpellId, &str)> = Vec::new();
        // The player's spellbook, then each simulated pet's.
        let unit_spells = player
            .spells
            .iter()
            .map(|spell| (Side::Player, spell))
            .chain(
                prepared
                    .pets
                    .iter()
                    .enumerate()
                    .flat_map(|(pet, exported)| {
                        exported
                            .spells
                            .iter()
                            .map(move |spell| (Side::Pet(pet as u8), spell))
                    }),
            );
        for (caster, exported) in unit_spells {
            let id = exported.action_id.clone().unwrap_or_default();
            if id.other_id == "OtherActionManaGain" {
                match caster {
                    Side::Pet(pet) => pet_mana_gain_spell[pet as usize] = Some(spells.len()),
                    _ => mana_gain_spell = Some(spells.len()),
                }
            }
            let item = id.item_id;
            let behavior = if let Some(class) = class_spell(exported) {
                SpellBehavior::Class(class)
            } else if id.other_id == "OtherActionAttack"
                && (id.tag == 1
                    || id.tag == 2
                    || effects.iter().any(|effect| {
                        matches!(effect, Effect::WindfuryTotem { extra_attack_spell: Some(extra_attack_spell), .. }
                            | Effect::WindfuryTotemSelf { extra_spell: extra_attack_spell, .. }
                            if *extra_attack_spell == spells.len())
                    }))
            {
                SpellBehavior::MeleeAuto(if id.tag == 2 {
                    melee::Hand::Off
                } else {
                    melee::Hand::Main
                })
            } else if !caster.is_target() && id.other_id == "OtherActionMove" {
                SpellBehavior::Move
            } else if let Some((min, max)) = caster
                .is_pet()
                .then(|| {
                    effects.iter().find_map(|effect| match effect {
                        Effect::EmeraldDragonWhelp {
                            acid_spit_spell_id,
                            acid_spit_min,
                            acid_spit_max,
                            ..
                        } if id.spell_id == *acid_spit_spell_id && id.tag == 0 => {
                            Some((*acid_spit_min, *acid_spit_max))
                        }
                        _ => None,
                    })
                })
                .flatten()
            {
                // Acid Spit rolls its base damage as Dragonbreath Chili's proc does on a target.
                SpellBehavior::RollDamage {
                    min,
                    max,
                    can_crit: true,
                }
            } else if caster.is_pet() {
                // A pet has no items or racials.
                SpellBehavior::None
            } else if id.other_id == "OtherActionShoot" && prepared.melee.auto_swing_ranged {
                SpellBehavior::MeleeAuto(melee::Hand::Ranged)
            } else {
                effects
                    .iter()
                    .find_map(|effect| match effect {
                        Effect::PotionMana {
                            item_id,
                            rng_label,
                            gains,
                            stone_multiplier,
                            regen_window_seconds,
                        } if *item_id == item && gains.len() == 1 => {
                            Some(SpellBehavior::PotionMana {
                                label: rng_label.clone(),
                                min: gains[0].min,
                                spread: gains[0].spread,
                                stone_multiplier: *stone_multiplier,
                                regen_window: *regen_window_seconds,
                            })
                        }
                        Effect::PotionResource {
                            item_id,
                            rng_label,
                            gains,
                            stone_multiplier,
                            aura,
                            ..
                        } if *item_id == item => {
                            if let Some(aura) = aura {
                                potion_auras.push((spells.len(), aura));
                            }
                            let kind = |name: &str| match name {
                                "ResourceTypeRage" => ResourceKind::Rage,
                                _ => ResourceKind::Mana,
                            };
                            Some(SpellBehavior::PotionResource {
                                label: rng_label.clone(),
                                gains: gains
                                    .iter()
                                    .map(|gain| (kind(&gain.resource), gain.min, gain.spread))
                                    .collect(),
                                stone_multiplier: *stone_multiplier,
                                aura: None,
                                metrics: Vec::new(),
                            })
                        }
                        Effect::DragonfirePotion { item_id, spell, .. }
                        | Effect::CausticSmogPotion { item_id, spell, .. }
                            if *item_id == item =>
                        {
                            Some(SpellBehavior::CastsSpell(*spell))
                        }
                        Effect::ElementalPurgingPotion { item_id, aura, .. } if *item_id == item => {
                            activations.push((spells.len(), aura));
                            Some(SpellBehavior::None)
                        }
                        Effect::ConjuredMana {
                            item_id,
                            rng_label,
                            gains,
                            selected,
                            regen_window_seconds,
                        } if *item_id == item && gains.len() == 1 => {
                            Some(SpellBehavior::ConjuredMana {
                                label: rng_label.clone(),
                                min: gains[0].min,
                                spread: gains[0].spread,
                                selected: *selected,
                                regen_window: *regen_window_seconds,
                            })
                        }
                        Effect::ConjuredEnergy {
                            item_id,
                            rng_label,
                            gains,
                            selected,
                            spill,
                        } if *item_id == item && gains.len() == 1 => {
                            Some(SpellBehavior::ConjuredEnergy {
                                label: rng_label.clone(),
                                min: gains[0].min,
                                spread: gains[0].spread,
                                selected: *selected,
                                spill: *spill,
                                metrics: resource(
                                    Side::Player,
                                    id.clone(),
                                    false,
                                    ResourceKind::Energy,
                                ),
                            })
                        }
                        Effect::GoblinSapper { item_id, .. } if *item_id == item && id.tag == 0 => {
                            Some(SpellBehavior::GoblinSapper)
                        }
                        Effect::BasicExplosive {
                            item_id,
                            min_damage,
                            max_damage,
                            aoe_cap_multiplier,
                        } if *item_id == item && id.tag == 0 => {
                            Some(SpellBehavior::BasicExplosive {
                                min: *min_damage,
                                max: *max_damage,
                                aoe_cap_multiplier: *aoe_cap_multiplier,
                            })
                        }
                        Effect::HealOnUse { item_id, .. } if *item_id == item && id.tag == 0 => {
                            heal_proc::heal_on_use(effects, item).map(SpellBehavior::SelfHeal)
                        }
                        Effect::AbsorbOnUse { item_id, .. } if *item_id == item && id.tag == 0 => {
                            effects
                                .iter()
                                .filter(|effect| matches!(effect, Effect::AbsorbOnUse { .. }))
                                .position(|effect| matches!(effect, Effect::AbsorbOnUse { item_id, .. } if *item_id == item))
                                .map(SpellBehavior::AbsorbOnUse)
                        }
                        Effect::SecondWind {
                            item_id,
                            mana,
                            ticks,
                            period_ns,
                            metrics_spell_id,
                            min_deficit,
                        } if *item_id == item && id.tag == 0 => Some(SpellBehavior::PeriodicMana {
                            amount: *mana,
                            ticks: *ticks,
                            period: *period_ns,
                            metrics: resource(
                                Side::Player,
                                ActionId {
                                    spell_id: *metrics_spell_id,
                                    ..ActionId::default()
                                },
                                false,
                                ResourceKind::Mana,
                            ),
                            min_deficit: *min_deficit,
                        }),
                        Effect::EnergizeOnUse {
                            item_id,
                            average,
                            variance,
                            whole,
                            periodic,
                            power,
                            ..
                        } if *item_id == item => {
                            // Go `energizedBarOf`: the item's own metrics on a rage or energy
                            // bar, registered with the cooldown.
                            let id = ActionId {
                                item_id: item,
                                ..ActionId::default()
                            };
                            let bar = match power {
                                None => EnergizedBar::Mana,
                                Some(EnergizedPower::Rage) => EnergizedBar::Rage(resource(
                                    Side::Player,
                                    id,
                                    false,
                                    ResourceKind::Rage,
                                )),
                                Some(EnergizedPower::Energy) => EnergizedBar::Energy(resource(
                                    Side::Player,
                                    id,
                                    false,
                                    ResourceKind::Energy,
                                )),
                            };
                            Some(SpellBehavior::EnergizeOnUse {
                                average: *average,
                                variance: *variance,
                                whole: *whole,
                                periodic: *periodic,
                                bar,
                            })
                        }
                        Effect::SpellCostAuraOnUse { item_id, aura, .. }
                        | Effect::SpellModAuraOnUse { item_id, aura, .. }
                        | Effect::ShieldOnHeal { item_id, aura, .. }
                        | Effect::StackingCdOnUse {
                            item_id,
                            window_aura: aura,
                            ..
                        } if *item_id == item && id.tag == 0 =>
                        {
                            activations.push((spells.len(), aura));
                            Some(SpellBehavior::None)
                        }
                        Effect::Eureka { spell_id, .. }
                            if id.spell_id == *spell_id && id.tag == 0 =>
                        {
                            Some(SpellBehavior::Eureka)
                        }
                        Effect::ExternalCooldown {
                            spell_id,
                            spell_tag,
                            ..
                        } if id.spell_id == *spell_id
                            && id.tag == *spell_tag
                            && id.item_id == 0 =>
                        {
                            Some(SpellBehavior::ExternalCooldown)
                        }
                        Effect::Berserking { spell_id, aura, .. }
                        | Effect::BloodFury { spell_id, aura, .. }
                        | Effect::ShatterCurse { spell_id, aura, .. }
                        | Effect::Stoneform { spell_id, aura, .. }
                        | Effect::ReadLeyLine { spell_id, aura, .. }
                        | Effect::Skysight { spell_id, aura, .. }
                            if id.spell_id == *spell_id && id.tag == 0 =>
                        {
                            activations.push((spells.len(), aura));
                            Some(SpellBehavior::None)
                        }
                        Effect::StackingOnUse { spell_id, aura, .. }
                        | Effect::InstantCastAura { spell_id, aura, .. }
                            if id.spell_id == *spell_id && id.item_id == 0 && id.tag == 0 =>
                        {
                            activations.push((spells.len(), aura));
                            Some(SpellBehavior::None)
                        }
                        Effect::TemporaryStats {
                            spell_id,
                            item_id,
                            aura,
                            ..
                        } if id.spell_id == *spell_id && id.item_id == *item_id && id.tag == 0 => {
                            activations.push((spells.len(), aura));
                            Some(SpellBehavior::None)
                        }
                        Effect::SpeedOnUse { item_id, aura, .. }
                            if id.item_id == *item_id && id.tag == 0 =>
                        {
                            activations.push((spells.len(), aura));
                            Some(SpellBehavior::None)
                        }
                        Effect::DiamondFlask { item_id, aura, .. }
                            if id.item_id == *item_id && id.tag == 0 =>
                        {
                            flask_auras.push((spells.len(), aura));
                            Some(SpellBehavior::DiamondFlask(0))
                        }
                        Effect::DragonbreathChili {
                            spell_id,
                            roll_min,
                            roll_max,
                            ..
                        } if id.spell_id == *spell_id && id.tag == 0 => {
                            Some(SpellBehavior::AreaRollDamage {
                                min: *roll_min,
                                max: *roll_max,
                            })
                        }
                        Effect::DamageOnUse {
                            spell,
                            direct,
                            periodic,
                            ..
                        } if *spell == spells.len() => {
                            // The gate refuses an outcome name the runtime does not know.
                            on_use_damage::OnUseDamage::new(direct, periodic)
                                .ok()
                                .map(SpellBehavior::OnUseDamage)
                        }
                        Effect::DragonfirePotion {
                            spell,
                            direct_damage,
                            tick_damage,
                            ..
                        } if *spell == spells.len() => Some(SpellBehavior::DragonfirePotion {
                            direct: *direct_damage,
                            tick: *tick_damage,
                        }),
                        Effect::CausticSmogPotion {
                            spell,
                            tick_damage,
                            tick_variance,
                            rng_label,
                            ..
                        } if *spell == spells.len() => Some(SpellBehavior::CausticSmogPotion {
                            tick: *tick_damage,
                            variance: *tick_variance,
                            label: rng_label.clone(),
                        }),
                        // The spell of a weapon's extra attack proc: it grants the attacks.
                        Effect::ExtraAttackProc {
                            spell: Some(spell),
                            attacks,
                            ..
                        } if *spell == spells.len() => Some(SpellBehavior::ExtraAttack(*attacks)),
                        Effect::SpellDataHealProc { spell, .. } if *spell == spells.len() => {
                            heal_proc::self_heal(effects, spells.len()).map(SpellBehavior::SelfHeal)
                        }
                        Effect::SpellDataAbsorbProc { spell, .. } if *spell == spells.len() => {
                            absorb::proc_shield(effects, spells.len()).map(SpellBehavior::AbsorbOnUse)
                        }
                        // A weapon proc of a client row that rolls a table of the spell's defense
                        // type, a chain or a damage over time; the gate refuses a name the runtime
                        // does not know.
                        Effect::SpellDataDamageProc {
                            spell,
                            average,
                            variance,
                            can_crit,
                            roll: None,
                            outcome,
                            chain,
                            area,
                            periodic,
                            ..
                        } if *spell == spells.len()
                            && (outcome.is_some()
                                || chain.is_some()
                                || area.is_some()
                                || periodic.is_some()
                                || exported.missile_speed > 0.0) =>
                        {
                            proc_damage::ProcDamage::new(
                                *average,
                                *variance,
                                *can_crit,
                                outcome.as_deref(),
                                chain.as_ref(),
                                area.as_ref(),
                                periodic.as_ref(),
                            )
                            .ok()
                            .map(SpellBehavior::ProcDamage)
                        }
                        // A hit of the melee defense type rolls its Go literal range on the melee
                        // special table (shared_utils.go `damageOutcome`); the gate refuses a
                        // name the runtime does not know.
                        Effect::SpellDataDamageProc {
                            spell,
                            roll: Some([min, max]),
                            outcome: Some(outcome),
                            ..
                        } if *spell == spells.len() => melee::PhysicalOutcome::of_melee_proc(outcome)
                            .map(|outcome| SpellBehavior::RollTableDamage {
                                min: *min,
                                max: *max,
                                outcome,
                            }),
                        Effect::SpellDataDamageProc {
                            spell,
                            roll: Some([min, max]),
                            can_crit,
                            outcome: None,
                            ..
                        } if *spell == spells.len() => Some(SpellBehavior::RollDamage {
                            min: *min,
                            max: *max,
                            can_crit: *can_crit,
                        }),
                        Effect::SpellDataDamageProc {
                            spell,
                            average,
                            variance,
                            can_crit,
                            roll: None,
                            ..
                        } if *spell == spells.len() => Some(SpellBehavior::EffectRoll {
                            average: *average,
                            variance: *variance,
                            can_crit: *can_crit,
                        }),
                        Effect::TouchOfTheGrave {
                            drain_spell_id,
                            health_fraction,
                            ..
                        } if id.spell_id == *drain_spell_id && id.tag == 0 => {
                            Some(SpellBehavior::TouchOfTheGraveDrain {
                                health_fraction: *health_fraction,
                                metrics: resource(
                                    Side::Player,
                                    id.clone(),
                                    false,
                                    ResourceKind::Health,
                                ),
                            })
                        }
                        Effect::SulfurasHandOfRagnaros {
                            fireball_spell,
                            roll_min,
                            roll_max,
                            ..
                        } if *fireball_spell == spells.len() => {
                            Some(SpellBehavior::SulfurasFireball {
                                min: *roll_min,
                                max: *roll_max,
                            })
                        }
                        Effect::SulfurasHandOfRagnaros {
                            immolation_spell,
                            immolation_damage,
                            ..
                        } if *immolation_spell == spells.len() => {
                            Some(SpellBehavior::FixedHit(*immolation_damage))
                        }
                        Effect::BladeOfHanna {
                            strike_spell,
                            damage,
                            ..
                        } if *strike_spell == spells.len() => {
                            melee::PhysicalOutcome::of_melee_proc("melee_special_hit_and_crit").map(
                                |outcome| SpellBehavior::FixedTableDamage {
                                    damage: *damage,
                                    outcome,
                                },
                            )
                        }
                        Effect::ForceReactiveDisk { spell, damage, .. } if *spell == spells.len() => {
                            Some(SpellBehavior::AreaFixedDamage(*damage))
                        }
                        // The shield's tick is the same hit: a literal on the magic table with
                        // a crit against every target, each dealt as it is calculated.
                        Effect::OilOfImmolation { spell, damage, .. } if *spell == spells.len() => {
                            Some(SpellBehavior::AreaFixedDamage(*damage))
                        }
                        Effect::Thunderfury {
                            strike_spell,
                            strike_damage,
                            ..
                        } if *strike_spell == spells.len() => {
                            Some(SpellBehavior::ThunderfuryStrike(*strike_damage))
                        }
                        Effect::Thunderfury { bounce_spell, .. }
                            if *bounce_spell == spells.len() =>
                        {
                            Some(SpellBehavior::ThunderfuryBounce)
                        }
                        _ => None,
                    })
                    .unwrap_or(SpellBehavior::None)
            };
            let cost = exported.cost.as_ref().map(|cost| Cost {
                kind: match cost.resource.as_str() {
                    "energy" => ResourceKind::Energy,
                    "rage" => ResourceKind::Rage,
                    "focus" => ResourceKind::Focus,
                    _ => ResourceKind::Mana,
                },
                refund: cost.refund,
                base: cost.base_cost,
                flat_modifier: cost.flat_modifier,
                percent_modifier: cost.percent_modifier,
                additive_percent_modifier: cost.additive_percent_modifier,
            });
            let health_metrics = match &behavior {
                SpellBehavior::Class(class) if A::health_metrics_before_cost(*class) => {
                    Some(resource(caster, id.clone(), false, ResourceKind::Health))
                }
                _ => None,
            };
            // Go newEnergyCost registers the energy metrics, then the combo point metrics.
            let (mana_metrics, energy_metrics) = match cost.map(|cost| cost.kind) {
                Some(ResourceKind::Energy) => {
                    let energy = resource(caster, id.clone(), false, ResourceKind::Energy);
                    let combo = resource(caster, id.clone(), false, ResourceKind::ComboPoints);
                    (None, Some((energy, combo)))
                }
                Some(kind) => (Some(resource(caster, id.clone(), false, kind)), None),
                None => (None, None),
            };
            let spell_id = spells.len();
            let dot = exported.dot.as_ref().map(|exported_dot| {
                dots.push(Dot::new(spell_id, caster, exported_dot));
                dots.len() - 1
            });
            let cast = &exported.default_cast;
            spells.push(Spell {
                flags: Flags::parse(&exported.flags),
                behavior,
                school: exported.school,
                school_index: school_index(exported.school),
                frostfire: exported.school == SCHOOL_FROSTFIRE,
                magic_defense: exported.defense_type == "DefenseTypeMagic",
                direct_proc: exported
                    .proc_mask
                    .iter()
                    .any(|mask| DIRECT_PROC_MASKS.contains(&mask.as_str())),
                proc_spell_damage: exported
                    .proc_mask
                    .iter()
                    .any(|mask| mask == "ProcMaskSpellDamage"),
                white_hit: exported
                    .proc_mask
                    .iter()
                    .any(|mask| mask == "ProcMaskMeleeMHAuto" || mask == "ProcMaskMeleeOHAuto"),
                melee_special: exported.proc_mask.iter().any(|mask| {
                    mask == "ProcMaskMeleeMHSpecial" || mask == "ProcMaskMeleeOHSpecial"
                }),
                requirement_auras: Vec::new(),
                off_hand_proc: exported
                    .proc_mask
                    .iter()
                    .any(|mask| mask == "ProcMaskMeleeOHAuto" || mask == "ProcMaskMeleeOHSpecial"),
                ranged_proc: exported
                    .proc_mask
                    .iter()
                    .any(|mask| mask == "ProcMaskRangedAuto" || mask == "ProcMaskRangedSpecial"),
                skill_hand: SkillHand::of(&exported.proc_mask),
                melee_or_ranged_proc: exported.proc_mask.iter().any(|mask| {
                    matches!(
                        mask.as_str(),
                        "ProcMaskMeleeMHAuto"
                            | "ProcMaskMeleeOHAuto"
                            | "ProcMaskMeleeMHSpecial"
                            | "ProcMaskMeleeOHSpecial"
                            | "ProcMaskRangedAuto"
                            | "ProcMaskRangedSpecial"
                    )
                }),
                melee_proc: exported.proc_mask.iter().any(|mask| {
                    matches!(
                        mask.as_str(),
                        "ProcMaskMeleeMHAuto"
                            | "ProcMaskMeleeOHAuto"
                            | "ProcMaskMeleeMHSpecial"
                            | "ProcMaskMeleeOHSpecial"
                    )
                }),
                class_spell: exported.class_spell.clone(),
                rank_aliases: exported.rank_aliases.clone(),
                class_spell_mask: exported.class_spell.is_some(),
                missile_speed: exported.missile_speed,
                cast_kind: exported.cast_kind,
                has_extra_cast_condition: exported.has_extra_cast_condition,
                min_range: exported.min_range,
                max_range: exported.max_range,
                ignore_haste: exported.ignore_haste,
                cost,
                default_cast: Cast {
                    cost: cast.cost,
                    gcd: cast.gcd_ns,
                    gcd_min: cast.gcd_min_ns,
                    cast_time: cast.cast_time_ns,
                    non_empty: cast.non_empty,
                },
                cur_cast: Cast::default(),
                cd: exported
                    .cd
                    .as_ref()
                    .map(|cd| (timer(&cd.timer), cd.duration_ns)),
                shared_cd: exported
                    .shared_cd
                    .as_ref()
                    .map(|cd| (timer(&cd.timer), cd.duration_ns)),
                cast_time_multiplier: exported.cast_time_multiplier,
                cd_multiplier: exported.cd_multiplier,
                bonus_hit_percent: exported.bonus_hit_percent,
                bonus_crit_percent: exported.bonus_crit_percent,
                bonus_spell_damage: exported.bonus_spell_damage,
                bonus_expertise_percent: exported.bonus_expertise_percent,
                damage_multiplier: exported.damage_multiplier,
                damage_multiplier_additive: exported.damage_multiplier_additive,
                direct_damage_multiplier_additive: exported.direct_damage_multiplier_additive,
                crit_multiplier_pct: exported.crit_multiplier_pct,
                crit_multiplier_additive: exported.crit_multiplier_additive,
                bonus_base_damage: exported.bonus_base_damage,
                bonus_coefficient: exported.bonus_coefficient,
                threat_multiplier: exported.threat_multiplier,
                flat_threat_bonus: exported.flat_threat_bonus,
                pushback_resist: exported.pushback_resist,
                damage_effect: exported.damage_effect.map(|e| (e.average, e.variance)),
                dot,
                related_dot_spell: exported.related_dot_spell,
                mana_metrics,
                energy_metrics,
                health_metrics,
                self_health_metrics: None,
                overheal: match caster {
                    Side::Player => player
                        .overheal
                        .iter()
                        .find(|overheal| overheal.spell_id == id.spell_id)
                        .map_or(0.0, |overheal| overheal.fraction),
                    _ => 0.0,
                },
                rage_metrics: None,
                white_hand: match exported.proc_mask.as_slice() {
                    [mask] if mask == "ProcMaskMeleeMHAuto" => Some(melee::Hand::Main),
                    [mask] if mask == "ProcMaskMeleeOHAuto" => Some(melee::Hand::Off),
                    _ => None,
                },
                metrics: [SpellMetrics::default(); DEFENDERS],
                action: None,
                caster,
                split_metrics: Vec::new(),
                split_actions: Vec::new(),
                split: 0,
                casts: 0,
                in_flight: 0,
                id,
            });
        }

        // Go keys action metrics by action ID in spellbook order, for each unit, one tagged
        // ID per metric split.
        let mut actions: Vec<ActionTotals> = Vec::new();
        let mut pet_actions: Vec<Vec<ActionTotals>> = vec![Vec::new(); prepared.pets.len()];
        let exported_spells = player
            .spells
            .iter()
            .chain(prepared.pets.iter().flat_map(|pet| pet.spells.iter()));
        for (spell, exported) in spells.iter_mut().zip(exported_spells) {
            if spell.flags.no_metrics {
                continue;
            }
            let actions = match spell.caster {
                Side::Pet(pet) => &mut pet_actions[pet as usize],
                _ => &mut actions,
            };
            let ids: Vec<ActionId> = if exported.metric_splits > 1 {
                (0..exported.metric_splits)
                    .map(|tag| ActionId {
                        tag: tag as i32,
                        ..spell.id.clone()
                    })
                    .collect()
            } else {
                vec![spell.id.clone()]
            };
            let mut indexes = Vec::new();
            for id in ids {
                let index = match actions.iter().position(|action| action.id == id) {
                    Some(index) => index,
                    None => {
                        actions.push(ActionTotals {
                            id,
                            melee: spell.flags.melee_metrics,
                            passive: spell.flags.passive,
                            school: spell.school,
                            targets: metrics::defender_reports(),
                        });
                        actions.len() - 1
                    }
                };
                indexes.push(index);
            }
            spell.action = Some(indexes[0]);
            if indexes.len() > 1 {
                spell.split_metrics = vec![[SpellMetrics::default(); DEFENDERS]; indexes.len()];
                spell.split_actions = indexes;
            }
        }

        let mut aura_logs: Vec<String> = Vec::new();
        // Go AddStatsDynamic: the player's stats for each combination of active stat auras.
        let (stat_aura_labels, stat_combos, stat_places) = effects
            .iter()
            .find_map(|effect| match effect {
                Effect::StatAuras {
                    auras,
                    combos,
                    stacks,
                    ..
                } => Some((
                    auras.clone(),
                    combos.clone(),
                    aura::stat_aura_places(stacks, auras.len()),
                )),
                _ => None,
            })
            .unwrap_or_default();
        let stat_stacks: Vec<i32> = effects
            .iter()
            .find_map(|effect| match effect {
                Effect::StatAuras { stacks, .. } => Some(stacks.clone()),
                _ => None,
            })
            .unwrap_or_default();
        // Go adds each bonus to the raw stats and takes it away again, so an MP5 bonus that is not
        // a whole number leaves the raw MP5 a few ulps off its start once the aura expires. The
        // runtime follows that raw value when the export gives such a bonus.
        let mp5_path = effects.iter().find_map(|effect| match effect {
            Effect::StatAuras { raw_mp5, .. } if !raw_mp5.is_empty() => {
                let mp5 = |mask: usize| stat_combos[mask].get("MP5").copied().unwrap_or_default();
                let bonuses = stat_aura_labels
                    .iter()
                    .enumerate()
                    .map(|(bit, label)| {
                        raw_mp5
                            .get(label)
                            .copied()
                            .unwrap_or_else(|| mp5(1 << bit) - mp5(0))
                    })
                    .collect();
                Some(Mp5Path {
                    value: config.powers.mp5,
                    bonuses,
                })
            }
            _ => None,
        });
        // The exporter writes the Health stat only when a combination changes it.
        let stat_health: Vec<Option<f64>> = stat_combos
            .iter()
            .map(|combo| combo.get("Health").copied())
            .collect();
        let stat_spirit: Vec<Option<f64>> = stat_combos
            .iter()
            .map(|combo| combo.get("Spirit").copied())
            .collect();
        let stat_intellect: Vec<Option<f64>> = stat_combos
            .iter()
            .map(|combo| combo.get("Intellect").copied())
            .collect();
        // Under Forever's rules the crit the player takes, when a combination changes it.
        let stat_crit_taken: Vec<Option<f64>> = stat_combos
            .iter()
            .map(|combo| combo.get("PseudoReducedCritTakenPercent").copied())
            .collect();
        // And each school spell damage and resistance stat only when a combination changes it;
        // the others keep their reset values.
        let by_school = |names: &[(usize, &str)], base: [f64; 8]| -> Vec<Option<[f64; 8]>> {
            stat_combos
                .iter()
                .map(|combo| {
                    names
                        .iter()
                        .any(|(_, name)| combo.contains_key(*name))
                        .then(|| {
                            let mut values = base;
                            for (index, name) in names {
                                if let Some(value) = combo.get(*name) {
                                    values[*index] = *value;
                                }
                            }
                            values
                        })
                })
                .collect()
        };
        let stat_school_damage = by_school(&SCHOOL_DAMAGE_STATS, config.school_damage);
        let stat_resistance: Vec<Vec<(usize, f64)>> = stat_combos
            .iter()
            .map(|combo| {
                RESISTANCE_STATS
                    .iter()
                    .filter_map(|(index, name)| combo.get(*name).map(|value| (*index, *value)))
                    .collect()
            })
            .collect();
        let stat_combos: Vec<Powers> = stat_combos
            .iter()
            .map(|combo| {
                let read = |name: &str| {
                    combo
                        .get(name)
                        .copied()
                        .ok_or_else(|| format!("stat combination lacks {name}"))
                };
                Ok(Powers {
                    spell_damage: read("SpellDamage")?,
                    attack_power: read("AttackPower")?,
                    ranged_attack_power: read("RangedAttackPower")?,
                    spell_crit_percent: read("SpellCritPercent")?,
                    physical_crit_percent: read("PhysicalCritPercent")?,
                    // The exporter writes the Mana stat only when a combination changes it.
                    max_mana: combo.get("Mana").copied().unwrap_or(config.max_mana),
                    mp5: read("MP5")?,
                    // And the HealingPower stat only when a combination changes it.
                    healing_power: combo
                        .get("HealingPower")
                        .copied()
                        .unwrap_or(config.powers.healing_power),
                    // Written only when a combination changes it.
                    spirit_regen_per_second: combo
                        .get("SpiritManaRegenPerSecond")
                        .copied()
                        .unwrap_or(config.spirit_regen_per_second),
                    // Written only when a combination changes it.
                    physical_damage: combo
                        .get("PhysicalDamage")
                        .copied()
                        .unwrap_or(config.powers.physical_damage),
                    // Written only when a combination changes it.
                    armor_penetration: combo
                        .get("ArmorPenetration")
                        .copied()
                        .unwrap_or(config.powers.armor_penetration),
                    spell_piercing: combo
                        .get("SpellPiercing")
                        .copied()
                        .unwrap_or(config.powers.spell_piercing),
                    // Written only when a combination changes them.
                    physical_hit_percent: combo
                        .get("PhysicalHitPercent")
                        .copied()
                        .unwrap_or(config.powers.physical_hit_percent),
                    spell_hit_percent: combo
                        .get("SpellHitPercent")
                        .copied()
                        .unwrap_or(config.powers.spell_hit_percent),
                })
            })
            .collect::<Result<_, BuildError>>()?;
        // The targets past the first copy the first's trackers at the first run.
        let mut trackers: Vec<Tracker<A::Aura>> = (0..DEFENDERS + prepared.pets.len())
            .map(|_| Tracker::default())
            .collect();
        let mut exported_memberships = Vec::new();
        let unit_auras = [(Side::Target, &target.auras), (Side::Player, &player.auras)]
            .into_iter()
            .chain(
                prepared
                    .pets
                    .iter()
                    .enumerate()
                    .map(|(pet, exported)| (Side::Pet(pet as u8), &exported.auras)),
            );
        // Go `Unit.JowManaMetrics`: every Judgement of Wisdom on the target restores the
        // attacker's mana under one metric.
        let mut jow_metrics: Option<usize> = None;
        for (side, auras) in unit_auras {
            let unit = match side {
                Side::Player => "player",
                Side::Target => "target",
                Side::Pet(_) => "pet",
                Side::Extra(_) => {
                    unreachable!("the other targets copy the first's at the first run")
                }
            };
            for exported in auras {
                let behavior = if let Some(dot) = dots
                    .iter()
                    .position(|dot: &Dot| dot.aura_label == exported.label && dot.side == side)
                {
                    AuraBehavior::Dot(dot)
                } else if let Some(kind) = class_aura(unit, &exported.label) {
                    AuraBehavior::Class(kind)
                } else if !side.is_target()
                    && exported
                        .action_id
                        .as_ref()
                        .is_some_and(|id| id.other_id == "OtherActionMove")
                {
                    AuraBehavior::Movement
                } else if let Some(jow) = effects.iter().find_map(|effect| match effect {
                    Effect::JudgementOfWisdom {
                        aura,
                        proc_chance,
                        mana,
                        metrics_action_id,
                        delay_ns,
                        ..
                    } if side == Side::Target && *aura == exported.label => Some((
                        *proc_chance,
                        *mana,
                        metrics_action_id.clone(),
                        *delay_ns,
                    )),
                    _ => None,
                }) {
                    let metrics = *jow_metrics.get_or_insert_with(|| {
                        resource(Side::Player, jow.2.clone(), false, ResourceKind::Mana)
                    });
                    AuraBehavior::JudgementOfWisdom {
                        chance: jow.0,
                        mana: jow.1,
                        id: jow.2.clone(),
                        metrics,
                        delay: jow.3,
                    }
                } else if let Some((chance, delay, drain)) =
                    effects.iter().find_map(|effect| match effect {
                        Effect::TouchOfTheGrave {
                            trigger_aura,
                            drain_spell_id,
                            proc_chance,
                            delay_ns,
                            ..
                        } if side == Side::Player && *trigger_aura == exported.label => Some((
                            *proc_chance,
                            *delay_ns,
                            *drain_spell_id,
                        )),
                        _ => None,
                    })
                {
                    let drain = spells
                        .iter()
                        .position(|spell: &Spell<A::Spell>| {
                            spell.id.spell_id == drain && spell.id.tag == 0
                        })
                        .ok_or_else(|| format!("Touch of the Grave drain {drain} is not registered"))?;
                    AuraBehavior::TouchOfTheGrave {
                        chance,
                        delay,
                        drain,
                    }
                } else if side == Side::Player
                    && effects
                        .iter()
                        .any(|effect| matches!(effect, Effect::Eureka { aura, .. } if *aura == exported.label))
                {
                    AuraBehavior::Eureka
                } else if let Some((attack, cast)) = effects.iter().find_map(|effect| match effect {
                    Effect::Berserking {
                        aura,
                        attack_speed_multiplier,
                        cast_speed_multiplier,
                        ..
                    } if side == Side::Player && *aura == exported.label => {
                        Some((*attack_speed_multiplier, *cast_speed_multiplier))
                    }
                    _ => None,
                }) {
                    AuraBehavior::MultiplyAttackAndCastSpeed { attack, cast }
                } else if let Some((melee, ranged, cast)) =
                    effects.iter().find_map(|effect| match effect {
                        Effect::SpeedOnUse {
                            aura,
                            melee_multiplier,
                            ranged_multiplier,
                            cast_multiplier,
                            ..
                        } if side == Side::Player && *aura == exported.label => {
                            Some((*melee_multiplier, *ranged_multiplier, *cast_multiplier))
                        }
                        _ => None,
                    })
                {
                    AuraBehavior::MultiplySpeeds {
                        melee,
                        ranged,
                        cast,
                    }
                } else if let Some((melee, ranged, cast)) =
                    effects.iter().find_map(|effect| match effect {
                        Effect::SpeedAura {
                            aura,
                            melee_multiplier,
                            ranged_multiplier,
                            cast_multiplier,
                        } if side == Side::Player && *aura == exported.label => Some((
                            *melee_multiplier,
                            *ranged_multiplier,
                            *cast_multiplier,
                        )),
                        _ => None,
                    })
                {
                    AuraBehavior::SwapSpeed {
                        melee,
                        ranged,
                        cast,
                    }
                } else if let Some(per_stack) = effects.iter().find_map(|effect| match effect {
                    Effect::BattleSquawk { aura, per_stack }
                        if side == Side::Player && *aura == exported.label =>
                    {
                        Some(*per_stack)
                    }
                    _ => None,
                }) {
                    AuraBehavior::BattleSquawk { per_stack }
                } else if let Some((multiplier, schools)) =
                    effects.iter().find_map(|effect| match effect {
                        Effect::ShatterCurse {
                            aura,
                            school_damage_taken_multiplier,
                            schools,
                            ..
                        }
                        | Effect::Stoneform {
                            aura,
                            school_damage_taken_multiplier,
                            schools,
                            ..
                        } if side == Side::Player && *aura == exported.label => {
                            Some((*school_damage_taken_multiplier, schools))
                        }
                        _ => None,
                    })
                {
                    let names = ["none", "physical", "arcane", "fire", "frost", "holy", "nature", "shadow"];
                    let mut mask = [false; 8];
                    for school in schools {
                        let index = names
                            .iter()
                            .position(|name| name == school)
                            .ok_or_else(|| format!("unknown school {school}"))?;
                        mask[index] = true;
                    }
                    AuraBehavior::MultiplySelfDamageTaken {
                        multiplier,
                        schools: mask,
                    }
                } else if let Some(multiplier) = effects.iter().find_map(|effect| match effect {
                    Effect::ReadLeyLine {
                        aura,
                        regen_multiplier,
                        ..
                    } if side == Side::Player && *aura == exported.label => Some(*regen_multiplier),
                    _ => None,
                }) {
                    AuraBehavior::MultiplyManaRegenSpeed(multiplier)
                } else if let Some(speed) = effects.iter().find_map(|effect| match effect {
                    Effect::Skysight {
                        aura,
                        speed_multiplier,
                        ..
                    } if side == Side::Player && *aura == exported.label => Some(*speed_multiplier),
                    _ => None,
                }) {
                    AuraBehavior::MultiplyMovementSpeed(speed)
                } else if let Some(index) = (side == Side::Player)
                    .then(|| {
                        effects
                            .iter()
                            .filter(|effect| matches!(effect, Effect::InnervateRegen { .. }))
                            .position(|effect| {
                                matches!(effect, Effect::InnervateRegen { aura, .. } if *aura == exported.label)
                            })
                    })
                    .flatten()
                {
                    AuraBehavior::InnervateRegen(index)
                } else if let Some(kind) = effects.iter().find_map(|effect| match effect {
                    Effect::WindfuryTotem {
                        totem_aura,
                        trigger_aura,
                        proc_aura,
                        ..
                    } if side == Side::Player => {
                        if *totem_aura == exported.label {
                            Some(AuraBehavior::WindfuryTotem)
                        } else if *trigger_aura == exported.label {
                            Some(AuraBehavior::WindfuryTrigger)
                        } else if *proc_aura == exported.label {
                            let bit = stat_aura_labels.iter().position(|label| label == proc_aura)?;
                            Some(AuraBehavior::WindfuryProc {
                                bit: 1 << stat_places[bit].0,
                            })
                        } else {
                            None
                        }
                    }
                    _ => None,
                }) {
                    kind
                } else if let Some(bit) = stat_aura_labels.iter().position(|label| {
                    side == Side::Player
                        && *label == exported.label
                        && !item_swap_stats_label(effects, label)
                }) {
                    // The lines Go's NewTemporaryStatsAura logs; multiplier and generated buffs
                    // log none.
                    let logs = effects.iter().find_map(|effect| match effect {
                        Effect::TemporaryStats {
                            aura,
                            gain_log,
                            expire_log,
                            ..
                        }
                        | Effect::DiamondFlask {
                            aura,
                            gain_log,
                            expire_log,
                            ..
                        } if *aura == exported.label => Some((gain_log, expire_log)),
                        Effect::PotionResource {
                            aura: Some(aura),
                            gain_log: Some(gain_log),
                            expire_log: Some(expire_log),
                            ..
                        } if *aura == exported.label => Some((gain_log, expire_log)),
                        Effect::Crusader {
                            mh_aura,
                            mh_gain_log,
                            mh_expire_log,
                            ..
                        } if *mh_aura == exported.label => Some((mh_gain_log, mh_expire_log)),
                        Effect::Crusader {
                            oh_aura,
                            oh_gain_log,
                            oh_expire_log,
                            ..
                        } if *oh_aura == exported.label => Some((oh_gain_log, oh_expire_log)),
                        Effect::StatProc {
                            aura,
                            gain_log: Some(gain_log),
                            expire_log: Some(expire_log),
                            ..
                        }
                        | Effect::SpellDataStatProc {
                            aura,
                            gain_log: Some(gain_log),
                            expire_log: Some(expire_log),
                            ..
                        } if *aura == exported.label => Some((gain_log, expire_log)),
                        _ => None,
                    });
                    // A stat aura an item swap switches logs what the exporter read it log.
                    let swap_logs = logs.is_none().then(|| item_swap_aura_logs(effects, &exported.label)).flatten();
                    let logs = logs.or(swap_logs.as_ref().map(|(gain, expire)| (gain, expire)));
                    let mut logged = |line: &String| {
                        aura_logs.push(line.clone());
                        aura_logs.len() - 1
                    };
                    let (gain_log, expire_log) = match logs {
                        Some((gain, expire)) => (Some(logged(gain)), Some(logged(expire))),
                        None => (None, None),
                    };
                    let (offset, width) = stat_places[bit];
                    if stat_stacks.get(bit).copied().unwrap_or(0) > 0 {
                        // Jom Gabbar's aura adds its own stacks on a timer.
                        let periodic = effects.iter().find_map(|effect| match effect {
                            Effect::StackingOnUse {
                                aura,
                                period_ns,
                                ticks,
                                ..
                            } if *aura == exported.label => Some((*period_ns, *ticks)),
                            _ => None,
                        });
                        AuraBehavior::StackingStats {
                            offset,
                            width,
                            periodic,
                        }
                    } else {
                        AuraBehavior::TemporaryStats {
                            bit: 1 << offset,
                            gain_log,
                            expire_log,
                        }
                    }
                } else if side == Side::Player
                    && effects.iter().any(|effect| {
                        matches!(effect, Effect::ChanceOfDeath { aura } if *aura == exported.label)
                    })
                {
                    AuraBehavior::ChanceOfDeath
                } else if let Some(chance) = (side == Side::Player)
                    .then(|| {
                        effects.iter().find_map(|effect| match effect {
                            Effect::PushbackTrigger { aura, chance } if *aura == exported.label => {
                                Some(*chance)
                            }
                            _ => None,
                        })
                    })
                    .flatten()
                {
                    AuraBehavior::PushbackTrigger { chance }
                } else if effects.iter().any(|effect| {
                    matches!(effect, Effect::ParryHaste { unit: u, aura, .. } if u == unit && *aura == exported.label)
                }) {
                    AuraBehavior::ParryHaste
                } else if let Some(index) = (side == Side::Player)
                    .then(|| {
                        effects
                            .iter()
                            .filter(|effect| {
                                matches!(effect, Effect::EnergizeProc { .. } | Effect::ManaProc { .. })
                            })
                            .position(|effect| {
                                matches!(effect, Effect::EnergizeProc { trigger_aura, .. } | Effect::ManaProc { trigger_aura, .. } if *trigger_aura == exported.label)
                            })
                    })
                    .flatten()
                {
                    AuraBehavior::EnergizeProc(index)
                } else if let Some(index) = (side == Side::Player)
                    .then(|| {
                        effects
                            .iter()
                            .filter(|effect| matches!(effect, Effect::HealthOnCast { .. }))
                            .position(|effect| {
                                matches!(effect, Effect::HealthOnCast { trigger_aura, .. } if *trigger_aura == exported.label)
                            })
                    })
                    .flatten()
                {
                    AuraBehavior::HealthOnCast(index)
                } else if let Some(index) = (side == Side::Target)
                    .then(|| {
                        effects
                            .iter()
                            .filter(|effect| matches!(effect, Effect::JudgementOfLight { .. }))
                            .position(|effect| {
                                matches!(effect, Effect::JudgementOfLight { aura, .. } if *aura == exported.label)
                            })
                    })
                    .flatten()
                {
                    AuraBehavior::JudgementOfLight(index)
                } else if let Some(index) = (side == Side::Player)
                    .then(|| {
                        effects
                            .iter()
                            .filter(|effect| matches!(effect, Effect::ManaOnCast { .. }))
                            .position(|effect| {
                                matches!(effect, Effect::ManaOnCast { trigger_aura, .. } if *trigger_aura == exported.label)
                            })
                    })
                    .flatten()
                {
                    AuraBehavior::ManaOnCast(index)
                } else if let Some(index) = (side == Side::Player)
                    .then(|| {
                        effects
                            .iter()
                            .filter(|effect| matches!(effect, Effect::StackingCdOnUse { .. }))
                            .position(|effect| {
                                matches!(effect, Effect::StackingCdOnUse { window_aura, .. } if *window_aura == exported.label)
                            })
                    })
                    .flatten()
                {
                    AuraBehavior::StackingCdWindow(index)
                } else if let Some(index) = (side == Side::Player)
                    .then(|| {
                        effects
                            .iter()
                            .filter(|effect| matches!(effect, Effect::ShieldOnHeal { .. }))
                            .position(|effect| {
                                matches!(effect, Effect::ShieldOnHeal { aura, .. } if *aura == exported.label)
                            })
                    })
                    .flatten()
                {
                    AuraBehavior::ShieldOnHeal(index)
                } else if side == Side::Player
                    && effects.iter().any(|effect| {
                        matches!(effect, Effect::Crusader { trigger_aura, .. } if *trigger_aura == exported.label)
                    })
                {
                    AuraBehavior::Crusader
                } else if side == Side::Player
                    && effects.iter().any(|effect| {
                        matches!(effect, Effect::Revelation { trigger_aura, ticks_trigger_aura, .. } if *trigger_aura == exported.label || *ticks_trigger_aura == exported.label)
                    })
                {
                    AuraBehavior::RevelationTrigger
                } else if side == Side::Player
                    && effects.iter().any(|effect| {
                        matches!(effect, Effect::Revelation { aura, .. } if *aura == exported.label)
                    })
                {
                    AuraBehavior::RevelationBuff
                } else if side == Side::Player
                    && effects.iter().any(|effect| {
                        matches!(effect, Effect::DragonbreathChili { trigger_aura, .. } if *trigger_aura == exported.label)
                    })
                {
                    AuraBehavior::DragonbreathChili
                } else if side == Side::Player
                    && effects.iter().any(|effect| {
                        matches!(effect, Effect::EmeraldDragonWhelp { trigger_aura, .. } if *trigger_aura == exported.label)
                    })
                {
                    AuraBehavior::EmeraldDragonWhelp
                } else if side == Side::Player
                    && effects.iter().any(|effect| {
                        matches!(effect, Effect::SulfurasHandOfRagnaros { trigger_aura, .. } if *trigger_aura == exported.label)
                    })
                {
                    AuraBehavior::SulfurasProc
                } else if side == Side::Player
                    && effects.iter().any(|effect| {
                        matches!(effect, Effect::BladeOfHanna { trigger_aura, .. } if *trigger_aura == exported.label)
                    })
                {
                    AuraBehavior::BladeOfHanna
                } else if side == Side::Player
                    && effects.iter().any(|effect| {
                        matches!(effect, Effect::SulfurasHandOfRagnaros { immolation_aura, .. } if *immolation_aura == exported.label)
                    })
                {
                    AuraBehavior::SulfurasImmolation
                } else if let Some(spell) = (side == Side::Player)
                    .then(|| {
                        effects.iter().find_map(|effect| match effect {
                            Effect::ForceReactiveDisk {
                                trigger_aura,
                                spell,
                                ..
                            } if *trigger_aura == exported.label => Some(*spell),
                            _ => None,
                        })
                    })
                    .flatten()
                {
                    AuraBehavior::ForceReactiveDisk(spell)
                } else if side == Side::Player
                    && effects.iter().any(|effect| {
                        matches!(effect, Effect::Thunderfury { trigger_aura, .. } if *trigger_aura == exported.label)
                    })
                {
                    AuraBehavior::Thunderfury
                } else if let Some(delta) = side
                    .is_target()
                    .then(|| {
                        effects.iter().find_map(|effect| match effect {
                            Effect::Thunderfury {
                                resistance_aura,
                                nature_resistance,
                                ..
                            } if *resistance_aura == exported.label => Some(*nature_resistance),
                            _ => None,
                        })
                    })
                    .flatten()
                {
                    AuraBehavior::ThunderfuryResistance(delta)
                } else if side == Side::Player
                    && effects.iter().any(|effect| {
                        matches!(effect, Effect::RageBar { aura, .. } if *aura == exported.label)
                    })
                {
                    AuraBehavior::RageBar
                } else if let Some((chance, attacks)) =
                    effects.iter().find_map(|effect| match effect {
                        Effect::ExtraAttackProc {
                            trigger_aura,
                            proc_chance,
                            attacks,
                            chances: None,
                            ..
                        } if side == Side::Player && *trigger_aura == exported.label => {
                            Some((*proc_chance, *attacks))
                        }
                        _ => None,
                    })
                {
                    AuraBehavior::ExtraAttackProc { chance, attacks }
                } else if let Some(proc) = (side == Side::Player)
                    .then(|| {
                        effects
                            .iter()
                            .filter(|effect| damage_proc_trigger(effect).is_some())
                            .position(|effect| damage_proc_trigger(effect) == Some(&exported.label))
                    })
                    .flatten()
                {
                    AuraBehavior::SpellDataDamageProc(proc)
                } else if let Some(proc) = (side == Side::Player)
                    .then(|| {
                        effects
                            .iter()
                            .filter(|effect| matches!(effect, Effect::SpellDataHealProc { .. }))
                            .position(|effect| {
                                matches!(effect, Effect::SpellDataHealProc { trigger_aura, .. } if *trigger_aura == exported.label)
                            })
                    })
                    .flatten()
                {
                    AuraBehavior::HealProc(proc)
                } else if let Some(proc) = (side == Side::Player)
                    .then(|| {
                        effects
                            .iter()
                            .filter(|effect| matches!(effect, Effect::SpellDataAbsorbProc { .. }))
                            .position(|effect| {
                                matches!(effect, Effect::SpellDataAbsorbProc { trigger_aura, .. } if *trigger_aura == exported.label)
                            })
                    })
                    .flatten()
                {
                    AuraBehavior::AbsorbProc(proc)
                } else if let Some(proc) = (side == Side::Player)
                    .then(|| {
                        effects
                            .iter()
                            .filter(|effect| matches!(effect, Effect::StatProc { .. }))
                            .position(|effect| {
                                matches!(effect, Effect::StatProc { trigger_aura, .. } if *trigger_aura == exported.label)
                            })
                    })
                    .flatten()
                {
                    AuraBehavior::StatProc(proc)
                } else if let Some(behavior) =
                    gear_procs::behavior(effects, side, &exported.label)
                {
                    behavior
                } else if let Some(proc) = (side == Side::Player)
                    .then(|| {
                        effects
                            .iter()
                            .filter(|effect| matches!(effect, Effect::SpellDataStatProc { .. }))
                            .position(|effect| {
                                matches!(effect, Effect::SpellDataStatProc { trigger_aura, .. } if *trigger_aura == exported.label)
                            })
                    })
                    .flatten()
                {
                    AuraBehavior::SpellDataStatProc(proc)
                } else if effects.iter().any(|effect| {
                    matches!(effect, Effect::InertListener { unit: u, aura, .. } if u == unit && *aura == exported.label)
                }) {
                    AuraBehavior::Inert
                } else {
                    AuraBehavior::Static
                };
                let icd = exported
                    .icd
                    .as_ref()
                    .map(|icd| (timer(&icd.timer), icd.duration_ns));
                trackers[side.index()].register(exported, behavior, icd);
                if !exported.exclusive_memberships.is_empty() {
                    let index = trackers[side.index()]
                        .find(&exported.label)
                        .ok_or_else(|| format!("aura {} is not registered", exported.label))?;
                    exported_memberships.push((
                        AuraRef { side, index },
                        exported.exclusive_memberships.clone(),
                    ));
                }
            }
        }
        for (spell, label) in potion_auras {
            let index = trackers[Side::Player.index()]
                .find(label)
                .ok_or_else(|| format!("aura {label} is not registered"))?;
            if let SpellBehavior::PotionResource { aura, .. } = &mut spells[spell].behavior {
                *aura = Some(index);
            }
        }
        for (spell, label) in flask_auras {
            let index = trackers[Side::Player.index()]
                .find(label)
                .ok_or_else(|| format!("aura {label} is not registered"))?;
            spells[spell].behavior = SpellBehavior::DiamondFlask(index);
        }
        for (spell, label) in activations {
            let aura = trackers[Side::Player.index()]
                .find(label)
                .ok_or_else(|| format!("aura {label} is not registered"))?;
            spells[spell].behavior = SpellBehavior::ActivateAura(aura);
        }
        for dot in &mut dots {
            dot.aura = trackers[dot.side.index()]
                .find(&dot.aura_label)
                .map(|index| AuraRef {
                    side: dot.side,
                    index,
                })
                .ok_or_else(|| format!("dot aura {} is not registered", dot.aura_label))?;
        }

        // Go registers a dot's aura by label, so two spells whose dots share a label put them on
        // one aura, which runs the callbacks of every dot.
        let mut dot_siblings: Vec<Vec<DotId>> = vec![Vec::new(); dots.len()];
        for first in 0..dots.len() {
            if dots.iter().position(|dot| dot.aura == dots[first].aura) == Some(first) {
                dot_siblings[first] = (first + 1..dots.len())
                    .filter(|&later| dots[later].aura == dots[first].aura)
                    .collect();
            }
        }

        let mut major_cooldowns = Vec::new();
        for cooldown in &player.major_cooldowns {
            let spell = cooldown
                .spell
                .filter(|&index| index < spells.len())
                .or_else(|| {
                    spells
                        .iter()
                        .position(|spell| spell.id == cooldown.action_id)
                })
                .ok_or_else(|| format!("major cooldown {} has no spell", cooldown.action_id))?;
            major_cooldowns.push(MajorCooldown {
                spell,
                priority: cooldown.priority,
                explosive: cooldown.kind.iter().any(|kind| kind == "explosive"),
                survival: cooldown.kind.iter().any(|kind| kind == "survival"),
                timings: cooldown.timings_ns.clone(),
                uses: 0,
            });
        }

        let parsed =
            crate::rotation::parse(&player.rotation).map_err(|reasons| reasons.join("; "))?;
        let mut fight = Fight {
            agent,
            now: 0,
            duration: config.base_duration,
            end_of_combat: config.base_duration,
            pull_started_at: 0,
            execute_phase: 0,
            next_execute: NEVER_EXPIRES,
            rng: SimRng::new(rng_mode, prepared.sim.labeled_rng, prepared.sim.seed as u64),
            queue: PendingQueue::default(),
            min_tracker_time: NEVER_EXPIRES,
            min_task_time: NEVER_EXPIRES,
            energy: None,
            rage,
            exclusive: Vec::new(),
            exclusive_tracking: Vec::new(),
            aura_effects: HashMap::new(),
            exported_memberships,
            armor_category: None,
            extra_armor_categories: Vec::new(),
            player_hit_resistance: (0.0, 1.0),
            damage_taken_auras: Vec::new(),
            resetting_auras: false,
            self_target: None,
            goblin_sapper: None,
            enemies: Vec::new(),
            untanked_swing: None,
            death: damage_taken::Death::default(),
            fixed_uptime: Vec::new(),
            energize_procs: Vec::new(),
            health_on_cast: Vec::new(),
            judgements_of_light: Vec::new(),
            jow_metrics_named: false,
            mana_on_cast: Vec::new(),
            stacking_cds: Vec::new(),
            shields_on_heal: Vec::new(),
            instant_casts: Vec::new(),
            player: Player::initial(&config),
            cast_speed: config.cast_speed,
            targets: vec![
                TargetUnit {
                    state: TargetState {
                        resistance: config.target_resistance,
                        school_damage_taken_multiplier: config
                            .target_school_damage_taken_multiplier,
                        school_bonus_spell_damage: config.target_school_bonus_spell_damage,
                    },
                    armor: prepared.melee.defender_armor,
                    armor_delta: 0.0,
                };
                prepared.encounter.target_count.max(1) as usize
            ],
            extra_targets_built: false,
            dot_copies: Vec::new(),
            dot_siblings,
            travel_batches: Vec::new(),
            free_travel_batches: Vec::new(),
            damage_taken_modifiers: Vec::new(),
            done_iteration: false,
            expiring_by_duration: None,
            spell_damage_taken_modifiers: Vec::new(),
            damage_taken_health,
            config,
            trackers,
            spells,
            dots,
            mods: Vec::new(),
            tick_crit_takebacks: Vec::new(),
            timers: vec![STARTING_CD_TIME; timer_names.len()],
            versions: Versions::default(),
            cooldown_order: (0..major_cooldowns.len()).collect(),
            major_cooldowns,
            cooldown_min_ready: NEVER_EXPIRES,
            rotation: Vec::new(),
            priority_len: 0,
            group_ranges: Vec::new(),
            player_target: 0,
            rotation_copies: Vec::new(),
            rotation_sequences: Vec::new(),
            assigned_curse: crate::engine::coverage::assigned_curse(prepared),
            rotation_reads: prepared.player.rotation_reads.clone(),
            prepull: Vec::new(),
            prepull_lists: Vec::new(),
            dot_base_durations: Vec::new(),
            in_rotation: false,
            apl: rotation::AplState::default(),
            resources,
            actions,
            target_actions: (0..prepared.encounter.target_count.max(1))
                .map(|_| {
                    target
                        .metrics_actions
                        .iter()
                        .map(|action| ActionTotals {
                            id: action.action_id.clone(),
                            melee: action.melee_metrics,
                            passive: false,
                            school: action.school,
                            targets: metrics::defender_reports(),
                        })
                        .collect()
                })
                .collect(),
            mana_regen_casting,
            mana_regen_not_casting,
            mana_gain_spell,
            log: None,
            autos: melee::AutoAttacks::default(),
            sunder: None,
            external_expose: None,
            aura_logs,
            stat_combos,
            stat_health,
            stat_spirit,
            stat_intellect,
            stat_crit_taken,
            stat_school_damage,
            stat_resistance,
            stat_mask: 0,
            item_swap: None,
            mp5_path,
            stat_aura_labels: stat_aura_labels.clone(),
            crusader: None,
            whelp: None,
            sulfuras: None,
            blade_of_hanna: None,
            thunderfury: None,
            enemy_slows: Vec::new(),
            windfury: None,
            external_cooldowns: Vec::new(),
            innervate_regens: Vec::new(),
            ley_line_innervate: None,
            innervates_up: 0,
            power_infusion: None,
            healing_dealt_factor: 1.0,
            heal_crit_bonus: 0.0,
            chili: None,
            revelation: None,
            mob_type_spell_damage: None,
            mob_type_bonuses: Vec::new(),
            mob_type_melee_bonuses: Vec::new(),
            damage_procs: Vec::new(),
            spell_stat_procs: Vec::new(),
            aura_mods: Vec::new(),
            heal_procs: Vec::new(),
            item_absorbs: Vec::new(),
            absorb_procs: Vec::new(),
            stat_procs: Vec::new(),
            health_rage_procs: Vec::new(),
            energy_procs: Vec::new(),
            health_regens: Vec::new(),
            oil: None,
            armor_debuff_procs: Vec::new(),
            eureka: None,
            pets: pet::inert_pets(effects)?,
            active_pets: Vec::new(),
            pet_trackers: Vec::new(),
            pet_agent_auras: pet::pet_agent_auras(prepared),
            pet_agent_labels: pet::pet_agent_labels(prepared),
            pet_agent_mana_bars: pet::pet_agent_mana_bars(prepared),
            pet_agent_max_mana: pet::pet_agent_max_mana(prepared),
            checks: std::cell::RefCell::new(None),
            presentation: None,
            extra_attack_strikes: Vec::new(),
            heartbeat_offset: 0,
            totals: metrics::Totals {
                target_dtps: vec![
                    metrics::Distribution::default();
                    prepared.encounter.target_count.max(1) as usize
                ],
                target_dps: vec![
                    metrics::Distribution::default();
                    prepared.encounter.target_count.max(1) as usize
                ],
                target_threat: vec![
                    metrics::Distribution::default();
                    prepared.encounter.target_count.max(1) as usize
                ],
                ..metrics::Totals::default()
            },
            encounter_damage_taken: 0.0,
            in_prepull: false,
            start_distance: prepared.player.distance_yards,
            move_multiplier: 1.0,
            initial_move_multiplier: 1.0,
            reset_move_multiplier: 1.0,
            movement_category_names: Vec::new(),
            movement_categories: Vec::new(),
            health_at_reset: player.health_at_reset,
            hp_percent_for_defensives: player.hp_percent_for_defensives,
            casting_regen_at_once: player.casting_regen_at_once,
            move_waits_for_queued_cast: player.move_waits_for_queued_cast,
            melee_keeps_auto_shot_timer: player.melee_keeps_auto_shot_timer,
            move_arrives_at_destination: player.move_arrives_at_destination,
            rotation_with_moves_looks_through_gcd: player.rotation_with_moves_looks_through_gcd,
            off_hand_waits_from_attack_start: player.off_hand_waits_from_attack_start,
            autocast_skips_unready_cooldowns: player.autocast_skips_unready_cooldowns,
            threat_follows_damage_bonus: player.threat_follows_damage_bonus,
            burst: player.burst_window.map(burst::BurstWindow::new),
            aura_refresh: Vec::new(),
            derived_categories: Vec::new(),
        };
        for (((exported, regen), mana_gain_spell), actions) in prepared
            .pets
            .iter()
            .zip(pet_regen)
            .zip(pet_mana_gain_spell)
            .zip(pet_actions)
        {
            fight.active_pets.push(pet::ActivePet::new(
                prepared,
                exported,
                regen,
                mana_gain_spell,
                actions,
            )?);
        }
        if let Some(energy) = &player.energy {
            fight.enable_energy_bar(energy);
        }
        for effect in effects {
            let (rng_label, chances, resource, amount, metrics_action_id, delay_ns) = match effect {
                Effect::EnergizeProc {
                    rng_label,
                    chances,
                    energy,
                    metrics_action_id,
                    delay_ns,
                    ..
                } => (
                    rng_label,
                    chances,
                    ResourceKind::Energy,
                    energy,
                    metrics_action_id,
                    delay_ns,
                ),
                Effect::ManaProc {
                    rng_label,
                    chances,
                    mana,
                    metrics_action_id,
                    delay_ns,
                    ..
                } => (
                    rng_label,
                    chances,
                    ResourceKind::Mana,
                    mana,
                    metrics_action_id,
                    delay_ns,
                ),
                _ => continue,
            };
            let mut by_spell = vec![None; fight.spells.len()];
            for entry in chances {
                if let Some(slot) = by_spell.get_mut(entry.spell) {
                    *slot = Some(entry.chance);
                }
            }
            let metrics = fight.new_resource_metrics(metrics_action_id.clone(), resource);
            fight.energize_procs.push(energy::EnergizeProc {
                label: rng_label.clone(),
                chances: by_spell,
                resource,
                energy: *amount,
                metrics,
                delay: *delay_ns,
            });
        }
        for effect in effects {
            match effect {
                Effect::HealthOnCast {
                    trigger_spells,
                    health,
                    metrics_action_id,
                    delay_ns,
                    ..
                } => {
                    let mut spells = vec![false; fight.spells.len()];
                    for &spell in trigger_spells {
                        if let Some(slot) = spells.get_mut(spell) {
                            *slot = true;
                        }
                    }
                    let metrics =
                        fight.new_resource_metrics(metrics_action_id.clone(), ResourceKind::Health);
                    fight.health_on_cast.push(HealthOnCast {
                        spells,
                        health: *health,
                        metrics,
                        delay: *delay_ns,
                    });
                }
                Effect::JudgementOfLight {
                    proc_chance,
                    rng_label,
                    heal,
                    metrics_action_id,
                    delay_ns,
                    ..
                } => {
                    // Go registers the heal's metrics on the judged target.
                    let metrics = fight.new_resource_metrics_of(
                        Side::Target,
                        metrics_action_id.clone(),
                        ResourceKind::Health,
                    );
                    fight.judgements_of_light.push(JudgementOfLight {
                        rng_label: rng_label.clone(),
                        chance: *proc_chance,
                        heal: *heal,
                        metrics,
                        copy_metrics: Vec::new(),
                        delay: *delay_ns,
                    });
                }
                Effect::ManaOnCast {
                    rng_label,
                    proc_chance,
                    spells,
                    metrics_action_id,
                    crit_only,
                    ..
                } => {
                    let mut mana = vec![None; fight.spells.len()];
                    for entry in spells {
                        let slot = mana.get_mut(entry.spell).ok_or_else(|| {
                            format!(
                                "mana on cast of spell {} outside the spellbook",
                                entry.spell
                            )
                        })?;
                        *slot = Some(entry.mana);
                    }
                    let metrics =
                        fight.new_resource_metrics(metrics_action_id.clone(), ResourceKind::Mana);
                    fight.mana_on_cast.push(energy::ManaOnCast {
                        label: rng_label.clone(),
                        chance: *proc_chance,
                        mana,
                        metrics,
                        crit_only: *crit_only,
                    });
                }
                _ => {}
            }
        }
        for effect in effects {
            match effect {
                Effect::StackingCdOnUse {
                    stat_aura,
                    window_aura,
                    decay,
                    trigger_spells,
                    callbacks,
                    landed_only,
                    proc_chance,
                    chances: manager,
                    require_damage,
                    rng_label,
                    ..
                } => {
                    let mut chances = vec![None; fight.spells.len()];
                    for &spell in trigger_spells {
                        let slot = chances.get_mut(spell).ok_or_else(|| {
                            format!("stacking trinket spell {spell} outside the spellbook")
                        })?;
                        *slot = Some(*proc_chance);
                    }
                    // A rate's manager: each heard spell rolls its own chance, and a spell its
                    // masks miss never stacks.
                    if let Some(manager) = manager {
                        let heard = chances.clone();
                        chances = vec![None; fight.spells.len()];
                        for entry in manager {
                            if heard.get(entry.spell).copied().flatten().is_none() {
                                return Err(format!(
                                    "stacking trinket chance for spell {} it does not hear",
                                    entry.spell
                                ));
                            }
                            chances[entry.spell] = Some(entry.chance);
                        }
                    }
                    let heard = |name: &str| callbacks.iter().any(|callback| callback == name);
                    if callbacks.iter().any(|callback| {
                        !["on_spell_hit_dealt", "on_heal_dealt", "on_cast_complete"]
                            .contains(&callback.as_str())
                    }) {
                        return Err(format!("a stacking trinket listens to {callbacks:?}"));
                    }
                    let stat = fight.player_aura(stat_aura)?;
                    if fight.aura(stat).max_stacks == 0 {
                        return Err(format!("stacking trinket aura {stat_aura} has no stacks"));
                    }
                    fight.stacking_cds.push(healer_gear::StackingCd {
                        stat_aura: stat,
                        decay: *decay,
                        chances,
                        hits: heard("on_spell_hit_dealt"),
                        heals: heard("on_heal_dealt"),
                        casts: heard("on_cast_complete"),
                        landed_only: *landed_only,
                        require_damage: *require_damage,
                        rng_label: rng_label.clone().unwrap_or_else(|| window_aura.clone()),
                    });
                }
                Effect::ShieldOnHeal {
                    item_id,
                    aura,
                    share,
                    chance,
                    heal,
                    spell_id,
                } => {
                    let spell = fight
                        .spells
                        .iter()
                        .position(|state| {
                            state.id.tag == 0
                                && if *item_id != 0 {
                                    state.id.item_id == *item_id
                                } else {
                                    state.id.spell_id == *spell_id && state.id.item_id == 0
                                }
                        })
                        .ok_or_else(|| {
                            format!("shield item {item_id} or spell {spell_id} has no spell")
                        })?;
                    fight.shields_on_heal.push(healer_gear::ShieldOnHeal {
                        share: *share,
                        spell,
                        chance: *chance,
                        heal: *heal,
                        label: aura.clone(),
                    });
                }
                _ => {}
            }
        }
        for effect in effects {
            if let Effect::FixedUptimeAura {
                aura,
                uptime,
                tick_length_ns,
                start_time_ns,
                chained_by,
            } = effect
            {
                let chained_by = match chained_by {
                    Some(label) => Some(fight.player_aura(label)?),
                    None => None,
                };
                let aura = fight.player_aura(aura)?;
                let duration = fight.aura(aura).duration;
                let ticks_per_aura = duration as f64 / *tick_length_ns as f64;
                fight.fixed_uptime.push(FixedUptime {
                    aura,
                    uptime: *uptime,
                    chance_per_tick: if *uptime == 1.0 {
                        1.0
                    } else {
                        1.0 - (1.0 - uptime).powf(1.0 / ticks_per_aura)
                    },
                    tick_length: *tick_length_ns,
                    start_time: *start_time_ns,
                    duration,
                    chained_by,
                });
            }
        }
        for effect in effects {
            if let Effect::GoblinSapper {
                item_id,
                self_tag,
                min_damage,
                max_damage,
                aoe_cap_multiplier,
                self_attack_table,
            } = effect
            {
                let self_spell = fight
                    .spells
                    .iter()
                    .position(|spell| spell.id.item_id == *item_id && spell.id.tag == *self_tag)
                    .ok_or_else(|| {
                        format!("Goblin Sapper Charge {item_id} has no self damage spell")
                    })?;
                fight.self_target =
                    Some(damage_taken::SelfTarget::new(prepared, self_attack_table)?);
                fight.goblin_sapper = Some(damage_taken::GoblinSapper {
                    min_damage: *min_damage,
                    max_damage: *max_damage,
                    aoe_cap_multiplier: *aoe_cap_multiplier,
                    self_spell,
                });
            }
        }
        if prepared.enemy.is_none() {
            fight.untanked_swing = effects.iter().find_map(|effect| match effect {
                Effect::ParryHaste {
                    unit,
                    swing_speed: Some(speed),
                    melee_haste_multiplier: Some(haste),
                    ..
                } if unit == "target" => Some((*speed, *haste)),
                _ => None,
            });
        }
        if let Some(values) = &prepared.enemy {
            let action = fight.target_actions[0]
                .iter()
                .position(|action| action.id == values.action_id)
                .ok_or("the target's swing has no target action")?;
            if values.rolls.len() != fight.stat_combos.len().max(1) {
                return Err(format!(
                    "the target's swing has {} rolls for {} stat aura combinations",
                    values.rolls.len(),
                    fight.stat_combos.len()
                ));
            }
            let attack_power_auras: Vec<(AuraRef, f64, f64)> = values
                .attack_power_auras
                .iter()
                .map(|entry| {
                    fight.trackers[Side::Target.index()]
                        .find(&entry.aura)
                        .map(|index| {
                            (
                                AuraRef {
                                    side: Side::Target,
                                    index,
                                },
                                entry.attack_power,
                                entry.log_attack_power,
                            )
                        })
                        .ok_or_else(|| format!("target aura {} is not registered", entry.aura))
                })
                .collect::<Result<_, BuildError>>()?;
            // Every copy of the boss swings the same swing at the tank, each with its own
            // metrics and melee speed.
            let shared = std::rc::Rc::new(values.clone());
            fight.enemies = (0..prepared.encounter.target_count.max(1))
                .map(|_| enemy::EnemyAttack {
                    values: std::rc::Rc::clone(&shared),
                    action,
                    metrics: SpellMetrics::default(),
                    melee_speed_multiplier: values.melee_speed_multiplier.unwrap_or(1.0),
                    attack_speed_multiplier: values.attack_speed_multiplier.unwrap_or(1.0),
                    attack_power_auras: attack_power_auras.clone(),
                })
                .collect();
        }
        for effect in effects {
            if let Effect::PlayerDamageTaken { auras } = effect {
                for entry in auras {
                    let aura = fight.player_aura(&entry.aura)?;
                    fight.damage_taken_auras.push((
                        aura.index,
                        entry.multiplier,
                        PseudoStat::DamageTaken,
                    ));
                }
            }
            if let Effect::PlayerMovement {
                speed_multiplier,
                initial_speed_multiplier,
                ..
            } = effect
            {
                // Every reset restores the multiplier before the permanent auras change it, and
                // the passive and active speed categories apply their effects as they take hold.
                fight.initial_move_multiplier =
                    initial_speed_multiplier.unwrap_or(*speed_multiplier);
                fight.reset_move_multiplier = *speed_multiplier;
                fight.move_multiplier = fight.initial_move_multiplier;
                for name in MOVEMENT_CATEGORIES {
                    let held = fight
                        .exported_memberships
                        .iter()
                        .any(|(aura, memberships)| {
                            aura.side == Side::Player
                                && memberships.iter().any(|membership| {
                                    membership.category == name && membership.single_aura
                                })
                        });
                    if held {
                        fight
                            .derived_categories
                            .push((Side::Player, name.to_string()));
                        fight.movement_category_names.push(name);
                    }
                }
            }
            if let Effect::PseudoStatAuras { auras } = effect {
                for entry in auras {
                    let aura = fight.player_aura(&entry.aura)?;
                    let stat = match entry.stat.as_str() {
                        "damage_taken" => PseudoStat::DamageTaken,
                        "threat" => PseudoStat::Threat,
                        "damage_dealt" => PseudoStat::DamageDealt,
                        other => return Err(format!("pseudo stat {other} is not supported")),
                    };
                    fight
                        .damage_taken_auras
                        .push((aura.index, entry.multiplier, stat));
                }
            }
        }
        for effect in effects {
            if let Effect::ExclusiveCategory {
                unit,
                category,
                members,
                armor_by_stacks,
            } = effect
            {
                let side = if unit == "target" {
                    Side::Target
                } else {
                    Side::Player
                };
                let mut entries = Vec::new();
                for member in members {
                    let index = fight.trackers[side.index()]
                        .find(&member.aura)
                        .ok_or_else(|| format!("aura {} is not registered", member.aura))?;
                    entries.push(exclusive::Member::new(
                        AuraRef { side, index },
                        member.priority,
                        member.spell_id,
                        member.per_stack,
                    ));
                }
                let mut enforced = exclusive::Category::new(entries);
                enforced.name = category.clone();
                fight.exclusive.push(enforced);
                if !armor_by_stacks.is_empty() {
                    fight.armor_category =
                        Some((fight.exclusive.len() - 1, armor_by_stacks.clone()));
                }
            }
        }
        for effect in effects {
            if let Effect::AuraShouldRefresh { unit, aura, modes } = effect {
                let side = if unit == "target" {
                    Side::Target
                } else {
                    Side::Player
                };
                let index = fight.trackers[side.index()]
                    .find(aura)
                    .ok_or_else(|| format!("{unit} aura {aura} is not registered"))?;
                let exported = if side == Side::Target {
                    &prepared.target.auras
                } else {
                    &prepared.player.auras
                };
                let memberships = exported
                    .iter()
                    .find(|candidate| candidate.label == *aura)
                    .map(|candidate| candidate.exclusive_memberships.as_slice())
                    .unwrap_or_default();
                let aura_ref = AuraRef { side, index };
                let mut readings = Vec::new();
                for (position, mode) in modes.iter().enumerate() {
                    readings.push(match mode.as_str() {
                        "own" => exclusive::RefreshReading::Own,
                        "never" => exclusive::RefreshReading::Never,
                        _ => {
                            let membership = memberships
                                .get(position)
                                .ok_or("an exclusive effect of the aura is not exported")?;
                            // A single aura category no class describes is enforced from the
                            // exported memberships; any other is only tracked, which is how
                            // Go reads a category that refuses nothing.
                            let described = fight
                                .enforced_category(side, &membership.category)
                                .is_some();
                            let derived = fight
                                .derived_categories
                                .contains(&(side, membership.category.clone()));
                            if membership.single_aura && !described && !derived {
                                fight
                                    .derived_categories
                                    .push((side, membership.category.clone()));
                            }
                            exclusive::RefreshReading::Live
                        }
                    });
                }
                fight.aura_refresh.push((aura_ref, readings));
            }
        }
        if let Some(bar) = fight.rage.as_mut() {
            bar.gain_spell = fight
                .spells
                .iter()
                .position(|spell| spell.id.other_id == "OtherActionRageGain");
        }
        fight.dot_base_durations = prepared
            .player
            .rotation_dot_base_durations
            .iter()
            .map(|entry| (entry.spell.clone(), entry.base_duration_ns))
            .collect();
        fight.bind_item_swap(prepared)?;
        let compilation = fight.compile_rotation(&parsed)?;
        // The rotation is fixed for the fight, so whether it moves is settled once here.
        fight.rotation_with_moves_looks_through_gcd &= compilation.has_move();
        fight.rotation = compilation.items;
        fight.priority_len = compilation.priority;
        fight.group_ranges = compilation.groups;
        fight.rotation_sequences = compilation.sequences;
        let (prepull, channels, lists) = fight.compile_prepull(&parsed, fight.rotation.len());
        fight.rotation.extend(channels);
        fight.prepull = prepull;
        fight.prepull_lists = lists;
        // A rotation that changes target: a copy for each current target, whose current, next
        // and previous targets resolve against it, swapped in as the target changes.
        if fight.targets.len() > 1 && crate::engine::coverage::rotation_changes_target(&parsed) {
            fight.rotation_copies.push(Vec::new());
            for position in 1..fight.targets.len() {
                let mut items = crate::rotation::with_current_target(position, || {
                    let compilation = fight.compile_rotation(&parsed)?;
                    if compilation.priority != fight.priority_len
                        || compilation.groups != fight.group_ranges
                    {
                        return Err(format!(
                            "the rotation compiled for target {} differs in shape",
                            position + 1
                        ));
                    }
                    let mut items = compilation.items;
                    let base = items.len();
                    let (_, channels, _) = fight.compile_prepull(&parsed, base);
                    items.extend(channels);
                    Ok::<_, String>(items)
                })?;
                if items.len() != fight.rotation.len() {
                    return Err(format!(
                        "the rotation compiled for target {} differs in shape",
                        position + 1
                    ));
                }
                items.shrink_to_fit();
                fight.rotation_copies.push(items);
            }
        }
        fight.presentation = Some(fight.presentation(&parsed, &player.rotation));
        for effect in effects {
            if let Effect::ExternalExposeArmor {
                aura,
                delay_ns,
                armor_before,
            } = effect
            {
                let index = fight.trackers[Side::Target.index()]
                    .find(aura)
                    .ok_or_else(|| format!("target aura {aura} is not registered"))?;
                let aura = AuraRef {
                    side: Side::Target,
                    index,
                };
                // The reset leaves it down; it lands at its delay.
                fight.set_aura_permanent(aura, false);
                fight.external_expose = Some(ExternalExpose {
                    aura,
                    delay: *delay_ns,
                    armor_before: *armor_before,
                });
            }
        }
        for effect in effects {
            if let Effect::SunderArmorRamp {
                aura,
                period_ns,
                ticks,
                armor_by_stacks,
                blocked,
            } = effect
            {
                let index = fight.trackers[Side::Target.index()]
                    .find(aura)
                    .ok_or_else(|| format!("target aura {aura} is not registered"))?;
                fight.sunder = Some(SunderRamp {
                    aura: AuraRef {
                        side: Side::Target,
                        index,
                    },
                    period: *period_ns,
                    ticks: *ticks,
                    armor_by_stacks: armor_by_stacks.clone(),
                    blocked: *blocked,
                });
            }
        }
        for effect in effects {
            match effect {
                Effect::Crusader {
                    mh_aura,
                    oh_aura,
                    chances,
                    heal_min,
                    heal_max,
                    heal_metrics_action_id,
                    ..
                } => {
                    let by_spell = chances_by_spell(chances, fight.spells.len());
                    let heal_metrics = fight
                        .new_resource_metrics(heal_metrics_action_id.clone(), ResourceKind::Health);
                    fight.crusader = Some(Crusader {
                        chances: by_spell,
                        mh_aura: fight.player_aura(mh_aura)?,
                        oh_aura: fight.player_aura(oh_aura)?,
                        heal_min: *heal_min,
                        heal_max: *heal_max,
                        heal_metrics,
                    });
                }
                Effect::DragonbreathChili {
                    spell_id,
                    proc_chance,
                    trigger_spells,
                    delay_ns,
                    ..
                } => {
                    let mut spells = vec![false; fight.spells.len()];
                    for &spell in trigger_spells {
                        if let Some(slot) = spells.get_mut(spell) {
                            *slot = true;
                        }
                    }
                    let spell = fight
                        .spells
                        .iter()
                        .position(|spell| spell.id.spell_id == *spell_id && spell.id.tag == 0)
                        .ok_or_else(|| {
                            format!("Dragonbreath Chili's spell {spell_id} is not registered")
                        })?;
                    fight.chili = Some(DragonbreathChili {
                        spells,
                        proc_chance: *proc_chance,
                        spell,
                        delay: *delay_ns,
                    });
                }
                Effect::ElementalPurgingPotion {
                    aura,
                    spell_damage,
                    mob_type,
                    ..
                } if target.mob_type.as_deref() == Some(mob_type.as_str()) => {
                    fight.mob_type_spell_damage = Some((fight.player_aura(aura)?, *spell_damage));
                }
                Effect::MobTypeBonus {
                    aura,
                    ranged_attack_power,
                    spell_damage,
                    melee_attack_power,
                    ..
                } => {
                    let aura = fight.player_aura(aura)?;
                    if *ranged_attack_power != 0.0 || *spell_damage != 0.0 {
                        fight
                            .mob_type_bonuses
                            .push((aura, *ranged_attack_power, *spell_damage));
                    }
                    if *melee_attack_power != 0.0 {
                        fight
                            .mob_type_melee_bonuses
                            .push((aura, *melee_attack_power));
                    }
                }
                Effect::EmeraldDragonWhelp { .. } => fight.bind_whelp(effect)?,
                Effect::Revelation { .. } => fight.bind_revelation(effect)?,
                Effect::SulfurasHandOfRagnaros { .. } => fight.bind_sulfuras(effect)?,
                Effect::BladeOfHanna { .. } => fight.bind_blade_of_hanna(effect)?,
                Effect::Thunderfury { .. } => fight.bind_thunderfury(effect)?,
                _ => {}
            }
        }
        for effect in effects {
            if let Effect::WindfuryTotem {
                totem_aura,
                period_ns,
                trigger_aura,
                trigger_spells,
                trigger_proc_chance,
                proc_aura,
                spend_spells,
                extra_attack_spell,
                trigger_require_damage,
                spend_require_damage,
                ..
            } = effect
            {
                let spell_count = fight.spells.len();
                let mask = |spells: &[usize]| {
                    let mut mask = vec![false; spell_count];
                    for &spell in spells {
                        if let Some(slot) = mask.get_mut(spell) {
                            *slot = true;
                        }
                    }
                    mask
                };
                // Go activates the trigger through the totem's exclusive effect, which runs before
                // the totem logs its gain, after the air totem slot has displaced a party Grace of
                // Air. Go's OnReset activates the totem every reset, even when a main hand
                // Windfury Weapon then outbids the effect and takes both down, which leaves them
                // inactive in the exported reset state.
                let trigger = fight.player_aura(trigger_aura)?;
                let totem = fight.player_aura(totem_aura)?;
                let tracker = &mut fight.trackers[Side::Player.index()];
                tracker.set_permanent(totem.index);
                tracker.clear_permanent(trigger.index);
                // Without melee autos there is no extra attack, and no spell can trigger the totem.
                if extra_attack_spell.is_none() && !trigger_spells.is_empty() {
                    return Err("Windfury Totem has trigger spells but no extra attack".into());
                }
                fight.windfury = Some(Windfury {
                    totem: fight.player_aura(totem_aura)?,
                    period: *period_ns,
                    trigger,
                    trigger_spells: mask(trigger_spells),
                    trigger_chance: *trigger_proc_chance,
                    proc_aura: fight.player_aura(proc_aura)?,
                    spend_spells: mask(spend_spells),
                    extra: *extra_attack_spell,
                    blocked: false,
                    trigger_require_damage: *trigger_require_damage,
                    spend_require_damage: *spend_require_damage,
                });
            }
        }
        fight.bind_heal_procs(effects)?;
        fight.bind_power_infusion(effects)?;
        fight.bind_innervate_regens(effects);
        fight.bind_read_ley_line(effects)?;
        fight.bind_external_cooldowns(effects, &player.auras)?;
        for effect in effects {
            if let Effect::AbsorbOnUse {
                aura,
                schools,
                average,
                variance,
                ..
            } = effect
            {
                let aura = fight.player_aura(aura)?;
                fight
                    .item_absorbs
                    .push(absorb::ItemAbsorb::new(aura, *schools, *average, *variance));
            }
        }
        fight.bind_absorb_procs(effects)?;
        for effect in effects {
            if let Some(proc) = damage_proc_of(effect, fight.spells.len()) {
                fight.damage_procs.push(proc);
            }
        }
        for effect in effects {
            if let Effect::SpellDataStatProc {
                aura,
                trigger_spells,
                callbacks,
                struck,
                landed_only,
                require_damage,
                proc_chance,
                ..
            } = effect
            {
                let mut mask = vec![false; fight.spells.len()];
                for &trigger in trigger_spells {
                    if let Some(slot) = mask.get_mut(trigger) {
                        *slot = true;
                    }
                }
                let heard = |name: &str| callbacks.iter().any(|callback| callback == name);
                let known = ["on_spell_hit_dealt", "on_heal_dealt", "on_cast_complete"];
                // A proc of hits taken hears the target's swings (`struck`) or, by the spells it
                // names, the Goblin Sapper Charge's hit on its thrower.
                let taken = callbacks == &["on_spell_hit_taken"];
                let valid = if *struck || taken {
                    taken
                } else {
                    callbacks
                        .iter()
                        .all(|callback| known.contains(&callback.as_str()))
                };
                if !valid {
                    return Err(format!("a stat proc listens to {callbacks:?}"));
                }
                let aura = fight.player_aura(aura)?;
                fight.spell_stat_procs.push(SpellStatProc {
                    trigger_spells: mask,
                    hits: heard("on_spell_hit_dealt"),
                    heals: heard("on_heal_dealt"),
                    casts: heard("on_cast_complete"),
                    taken,
                    struck: *struck,
                    landed_only: *landed_only,
                    require_damage: *require_damage,
                    chance: *proc_chance,
                    aura,
                });
            }
            // Go spell_mod.go: one dynamic mod per amount, over the spells it changes.
            if let Effect::SpellCostAuraOnUse {
                aura, cost_changes, ..
            } = effect
            {
                let aura = fight.player_aura(aura)?;
                let mut amounts: Vec<i32> = cost_changes.iter().map(|change| change.flat).collect();
                amounts.sort_unstable();
                amounts.dedup();
                let mods = amounts
                    .into_iter()
                    .map(|amount| {
                        let affected = cost_changes
                            .iter()
                            .filter(|change| change.flat == amount)
                            .map(|change| change.spell)
                            .collect();
                        fight.register_mod(
                            spell_mod::ModKind::PowerCostFlat,
                            f64::from(amount),
                            0,
                            affected,
                        )
                    })
                    .collect();
                fight.aura_mods.push(mods);
                fight.aura_mut(aura).behavior = AuraBehavior::SpellMods(fight.aura_mods.len() - 1);
            }
            // A healer's instant cast aura: its mods while up, and the casts that spend it.
            if let Effect::InstantCastAura { aura, changes, .. } = effect {
                let aura = fight.player_aura(aura)?;
                let mut mods = Vec::new();
                let mut spends = vec![false; fight.spells.len()];
                for change in changes {
                    let kind = spell_mod::ModKind::named(&change.kind).ok_or_else(|| {
                        format!("spell mod {:?} on an instant cast aura", change.kind)
                    })?;
                    let slot = spends.get_mut(change.spell).ok_or_else(|| {
                        format!(
                            "instant cast of spell {} outside the spellbook",
                            change.spell
                        )
                    })?;
                    *slot = true;
                    let (float, time) = if kind.is_time() {
                        (0.0, change.value as i64)
                    } else {
                        (change.value, 0)
                    };
                    mods.push(fight.register_mod(kind, float, time, vec![change.spell]));
                }
                fight
                    .instant_casts
                    .push(healer_gear::InstantCast { mods, spends });
                fight.aura_mut(aura).behavior =
                    AuraBehavior::InstantCast(fight.instant_casts.len() - 1);
            }
            // A healer's on-use aura of spell mods: one dynamic mod per kind and amount, over the
            // spells it changes.
            if let Effect::SpellModAuraOnUse { aura, changes, .. } = effect {
                let aura = fight.player_aura(aura)?;
                let mut groups: Vec<(spell_mod::ModKind, f64, Vec<SpellId>)> = Vec::new();
                for change in changes {
                    let kind = spell_mod::ModKind::named(&change.kind)
                        .ok_or_else(|| format!("spell mod {:?} on use", change.kind))?;
                    if change.spell >= fight.spells.len() {
                        return Err(format!(
                            "spell mod on spell {} outside the spellbook",
                            change.spell
                        ));
                    }
                    match groups
                        .iter_mut()
                        .find(|(k, value, _)| *k == kind && *value == change.value)
                    {
                        Some((_, _, spells)) => spells.push(change.spell),
                        None => groups.push((kind, change.value, vec![change.spell])),
                    }
                }
                let mods = groups
                    .into_iter()
                    .map(|(kind, value, spells)| {
                        let (float, time) = if kind.is_time() {
                            (0.0, value as i64)
                        } else {
                            (value, 0)
                        };
                        fight.register_mod(kind, float, time, spells)
                    })
                    .collect();
                fight.aura_mods.push(mods);
                fight.aura_mut(aura).behavior = AuraBehavior::SpellMods(fight.aura_mods.len() - 1);
            }
        }
        for effect in effects {
            if let Effect::StatProc {
                rng_label,
                aura,
                chances,
                add_stack,
                ..
            } = effect
            {
                let mut by_spell = vec![None; fight.spells.len()];
                for entry in chances {
                    if let Some(slot) = by_spell.get_mut(entry.spell) {
                        *slot = Some(entry.chance);
                    }
                }
                let aura = fight.player_aura(aura)?;
                fight.stat_procs.push(StatProc {
                    chances: by_spell,
                    rng_label: rng_label.clone(),
                    aura,
                    add_stack: *add_stack,
                });
            }
        }
        fight.bind_gear_procs(effects)?;
        fight.bind_health_regens(effects)?;
        fight.bind_oil_of_immolation(effects)?;
        fight.bind_on_use_damage(effects);
        fight.autos.melee = prepared.melee.auto_swing_melee;
        fight.autos.dual_wielding = prepared.melee.dual_wielding;
        fight.autos.mh.weapon = prepared.melee.main_hand.clone();
        fight.autos.oh.weapon = prepared.melee.off_hand.clone();
        let auto_spell = |fight: &Self, caster: Side, tag: i32| {
            fight.spells.iter().position(|spell| {
                spell.caster == caster
                    && spell.id.other_id == "OtherActionAttack"
                    && spell.id.tag == tag
            })
        };
        fight.autos.mh.spell = auto_spell(&fight, Side::Player, 1);
        fight.autos.oh.spell = auto_spell(&fight, Side::Player, 2);
        for (index, exported) in prepared.pets.iter().enumerate() {
            let side = Side::Pet(index as u8);
            let (mh, oh) = (auto_spell(&fight, side, 1), auto_spell(&fight, side, 2));
            let pet = &mut fight.active_pets[index];
            pet.autos.melee = exported.melee.auto_swing_melee;
            pet.autos.dual_wielding = exported.melee.dual_wielding;
            pet.autos.mh.weapon = exported.melee.main_hand.clone();
            pet.autos.oh.weapon = exported.melee.off_hand.clone();
            pet.autos.mh.spell = mh;
            pet.autos.oh.spell = oh;
        }
        fight.autos.ranged_auto = prepared.melee.auto_swing_ranged;
        fight.autos.ranged.weapon = prepared.melee.ranged.clone();
        fight.autos.ranged.spell = fight
            .spells
            .iter()
            .position(|spell| spell.id.other_id == "OtherActionShoot");
        for effect in effects {
            if let Effect::Eureka {
                aura,
                cost_percent,
                damage_percent,
                tick_cancel_percent,
                cost_spells,
                damage_spells,
                tick_cancel_spells,
                ..
            } = effect
            {
                fight.eureka = Some(fight.bind_eureka(
                    aura,
                    [*cost_percent, *damage_percent, *tick_cancel_percent],
                    [cost_spells, damage_spells, tick_cancel_spells],
                )?);
            }
        }
        for (spell, exported) in player.spells.iter().enumerate() {
            for label in exported.requirement_auras.iter().flatten() {
                let aura = fight.player_aura(label)?;
                fight.spells[spell].requirement_auras.push(aura);
            }
        }
        fight.bind_item_swap_variants(effects);
        Ok(fight)
    }

    /// Counts a change to what the rotation's cached answers read (see [`reads`]), for a class
    /// that keeps them; for any other it compiles to nothing.
    #[inline(always)]
    pub(crate) fn changed(&mut self, kind: u8) {
        if A::CACHE_ROTATION {
            let versions = &mut self.versions;
            match kind {
                reads::TIMERS => versions.timers += 1,
                reads::RESOURCES => versions.resources += 1,
                reads::AURAS => versions.auras += 1,
                reads::CASTS => versions.casts += 1,
                reads::SPELLS => versions.spells += 1,
                reads::CLASS => versions.class += 1,
                _ => unreachable!("one kind of change at a time"),
            }
        }
    }

    /// Counts an energy gain, which [`Fight::changed`] has already counted as a resource change.
    #[inline(always)]
    pub(crate) fn gained_energy(&mut self) {
        if A::CACHE_OVER_ENERGY {
            self.versions.energy_gains += 1;
        }
    }

    /// Go `Unit.AddStatsDynamic` for the stat auras: the stats of the new combination, and
    /// current mana held to a lower maximum.
    pub(crate) fn set_stat_mask(&mut self, mask: u32) {
        let changed = self.stat_mask ^ mask;
        self.stat_mask = mask;
        self.changed(reads::RESOURCES);
        let (mp5, spirit_regen) = (
            self.player.powers.mp5,
            self.player.powers.spirit_regen_per_second,
        );
        // A dynamic pet takes the owner's change at its heartbeat.
        self.set_player_powers(self.stat_combos[mask as usize]);
        if let Some(path) = self.mp5_path.as_mut() {
            self.player.powers.mp5 = path.change(changed, mask);
        }
        if let (Some(Some(crit_taken)), Some(target)) = (
            self.stat_crit_taken.get(mask as usize),
            self.self_target.as_mut(),
        ) {
            target.reduced_crit_taken_percent = *crit_taken;
        }
        if self.has_mana_bar() && self.player.mana > self.player.powers.max_mana {
            self.player.mana = self.player.powers.max_mana;
        }
        // Go processDynamicBonus runs UpdateManaRegenRates when MP5, Intellect or Spirit move.
        // Intellect plays no part in Forever's regeneration, but the refresh it causes picks up
        // a casting rate an aura changed without one, as Improved Stormstrike's does.
        let intellect = |mask: u32| self.stat_intellect.get(mask as usize).copied().flatten();
        if self.player.powers.mp5 != mp5
            || self.player.powers.spirit_regen_per_second != spirit_regen
            || intellect(mask) != intellect(mask ^ changed)
        {
            self.update_mana_regen_rates();
        }
    }

    /// Enforces a single aura exclusive category of the unit from the exported memberships, as
    /// Go does for every one: a class whose auras share one, as a Hunter's aspects do, asks for it
    /// so an aura that takes the category's place switches the active one off.
    pub(crate) fn enforce_exported_category(&mut self, side: Side, name: &str) {
        let key = (side, name.to_string());
        if !self.derived_categories.contains(&key) {
            self.derived_categories.push(key);
        }
    }

    /// Go `HasManaBar`: a class without mana, such as a Rogue, exports no maximum mana.
    pub(crate) fn has_mana_bar(&self) -> bool {
        self.config.max_mana > 0.0
    }

    /// The mana regeneration inputs for Go `ManaRegenPerSecondWhileCasting`.
    pub(crate) fn regen_inputs(&self) -> RegenInputs {
        RegenInputs {
            mp5: self.player.powers.mp5,
            spirit_regen_per_second: self.player.powers.spirit_regen_per_second,
            spirit_regen_rate_casting: self.player.spirit_regen_rate_casting,
            force_full_spirit_regen: self.player.force_full_spirit_regen,
            spirit_regen_multiplier: self.player.spirit_regen_multiplier,
            mana_regen_multiplier: self.player.mana_regen_multiplier,
        }
    }

    /// Go `MultiplyManaRegenSpeed`.
    pub(crate) fn multiply_mana_regen_speed(&mut self, multiplier: f64) {
        self.player.mana_regen_multiplier *= multiplier;
        self.update_mana_regen_rates();
    }

    /// Go `UpdateManaRegenRates`.
    pub(crate) fn update_mana_regen_rates(&mut self) {
        let inputs = self.regen_inputs();
        self.player.mana_tick_casting =
            crate::mechanics::mana::regen_per_second_casting(inputs) * 2.0;
        self.player.mana_tick_not_casting =
            crate::mechanics::mana::regen_per_second_not_casting(inputs) * 2.0;
    }

    /// The state of an acting unit: the player, or its pet.
    pub(crate) fn unit(&self, side: Side) -> &Player {
        match side {
            Side::Pet(_) => &self.active_pet(side).state,
            _ => &self.player,
        }
    }

    /// [`Self::unit`], mutable.
    pub(crate) fn unit_mut(&mut self, side: Side) -> &mut Player {
        match side {
            Side::Pet(_) => &mut self.active_pet_mut(side).state,
            _ => &mut self.player,
        }
    }

    /// The configuration of an acting unit. The fight's and the target's settings are the
    /// same in every unit's copy.
    pub(crate) fn unit_config(&self, side: Side) -> &Config {
        match side {
            Side::Pet(_) => &self.active_pet(side).config,
            _ => &self.config,
        }
    }

    /// The configuration of a unit whose distance changes as it moves: a pet, or the player.
    pub(crate) fn unit_config_mut(&mut self, side: Side) -> &mut Config {
        match side {
            Side::Pet(_) => &mut self.active_pet_mut(side).config,
            Side::Player => &mut self.config,
            _ => panic!("a target does not move"),
        }
    }

    /// Go `Unit.CastSpeed` of an acting unit.
    pub(crate) fn unit_cast_speed(&self, side: Side) -> f64 {
        match side {
            Side::Pet(_) => self.active_pet(side).cast_speed,
            _ => self.cast_speed,
        }
    }

    /// The unit that casts a spell.
    pub(crate) fn caster(&self, spell: SpellId) -> Side {
        self.spells[spell].caster
    }

    /// Go `Spell.Dot` for the target: the spell's own dot, else its related spell's.
    pub(crate) fn spell_dot(&self, spell: SpellId) -> Option<DotId> {
        let state = &self.spells[spell];
        state.dot.or_else(|| {
            state
                .related_dot_spell
                .and_then(|related| self.spells.get(related)?.dot)
        })
    }

    /// Go `AttachMultiplicativePseudoStatBuff` on a school's damage dealt multiplier: the
    /// gain multiplies.
    pub(crate) fn multiply_school_damage_dealt(&mut self, school_index: usize, factor: f64) {
        self.player.school_damage_dealt_multiplier[school_index] *= factor;
    }

    /// The expiry of [`Self::multiply_school_damage_dealt`], which divides.
    pub(crate) fn divide_school_damage_dealt(&mut self, school_index: usize, divisor: f64) {
        self.player.school_damage_dealt_multiplier[school_index] /= divisor;
    }

    /// Go `PseudoStats.SchoolBonusSpellDamage` changed on a target, as a debuff's gain and
    /// expiry change the spell damage a school's spells gain against it.
    pub(crate) fn add_target_school_bonus_spell_damage(
        &mut self,
        target: Side,
        school_index: usize,
        delta: f64,
    ) {
        self.target_unit_mut(target).state.school_bonus_spell_damage[school_index] += delta;
    }

    /// Go `AddStatsDynamic` on a target's resistance.
    pub(crate) fn add_target_resistance(&mut self, target: Side, school_index: usize, delta: f64) {
        self.target_unit_mut(target).state.resistance[school_index] += delta;
    }

    /// A parsed aura's multiplier on a target's school damage taken: Go multiplies by the
    /// factor on gain and by its reciprocal on expiry.
    pub(crate) fn multiply_target_school_damage_taken(
        &mut self,
        target: Side,
        school_index: usize,
        factor: f64,
    ) {
        self.target_unit_mut(target)
            .state
            .school_damage_taken_multiplier[school_index] *= factor;
    }

    /// Go `AddStatsDynamic` on a target's armor, for an armor debuff the class applies.
    pub(crate) fn add_target_armor(&mut self, target: Side, delta: f64) {
        let unit = self.target_unit_mut(target);
        unit.armor += delta;
        unit.armor_delta += delta;
    }

    /// How many units a spell can hit in this fight, the slots [`Side::index`] gives them:
    /// every target and the player.
    pub(crate) fn defender_count(&self) -> usize {
        self.targets.len() + 1
    }

    /// Every target of the encounter, in unit index order.
    pub(crate) fn target_sides(&self) -> impl Iterator<Item = Side> {
        (0..self.targets.len()).map(Side::target)
    }

    /// Go `NextActiveTargetUnit`: the target after this one in unit index order, wrapping to
    /// the first.
    pub(crate) fn next_target(&self, target: Side) -> Side {
        let position = target.target_position().expect("the unit is a target");
        Side::target((position + 1) % self.targets.len())
    }

    /// A target's mutable state. A spell on the player, such as its own dot, reads the first
    /// target's, as the runtime always has.
    pub(crate) fn target_unit(&self, target: Side) -> &TargetUnit {
        &self.targets[target.target_position().unwrap_or(0)]
    }

    fn target_unit_mut(&mut self, target: Side) -> &mut TargetUnit {
        &mut self.targets[target.target_position().unwrap_or(0)]
    }

    /// The same aura on another target: every target holds a copy of the first's auras at the
    /// same positions. An aura on any other unit is its own.
    pub(crate) fn aura_on(&self, aura: AuraRef, target: Side) -> AuraRef {
        if aura.side.is_target() && target.is_target() {
            AuraRef {
                side: target,
                index: aura.index,
            }
        } else {
            aura
        }
    }

    /// A dot's copy on a target: the dot itself on the first target or on its caster.
    pub(crate) fn dot_on(&self, dot: DotId, target: Side) -> DotId {
        match target {
            Side::Extra(extra) => self
                .dot_copies
                .get(dot)
                .and_then(|copies| copies.get(extra as usize))
                .copied()
                .unwrap_or(dot),
            _ => dot,
        }
    }

    /// Give each target past the first its copies of the first's auras, dots and exclusive
    /// categories. Go registers a target's auras and dots on every target alike, which the
    /// exporter checks, so each copy sits at the first's position; a dot aura behaves as its
    /// target's copy of the dot.
    fn build_extra_targets(&mut self) {
        self.extra_targets_built = true;
        if self.targets.len() < 2 {
            return;
        }
        self.dot_copies = vec![Vec::new(); self.dots.len()];
        for extra in 0..self.targets.len() - 1 {
            let side = Side::Extra(extra as u8);
            for dot in 0..self.dot_copies.len() {
                if self.dots[dot].side != Side::Target {
                    continue;
                }
                let mut copy = self.dots[dot].clone();
                copy.side = side;
                copy.aura = AuraRef {
                    side,
                    index: copy.aura.index,
                };
                copy.tick_action = None;
                self.dots.push(copy);
                self.dot_copies[dot].push(self.dots.len() - 1);
            }
            self.dot_siblings.resize(self.dots.len(), Vec::new());
            for dot in 0..self.dot_copies.len() {
                let siblings: Vec<DotId> = self.dot_siblings[dot]
                    .iter()
                    .filter_map(|&sibling| self.dot_copies[sibling].get(extra).copied())
                    .collect();
                if let Some(&copy) = self.dot_copies[dot].get(extra) {
                    self.dot_siblings[copy] = siblings;
                }
            }
            let auras = self.trackers[Side::Target.index()].copy_auras(|behavior| match behavior {
                AuraBehavior::Dot(dot) => AuraBehavior::Dot(self.dot_copies[*dot][extra]),
                other => other.clone(),
            });
            self.trackers[side.index()].auras = auras;
            // Go `AttachJudgementOfLightHeal` registers the heal's metrics on each target.
            for index in 0..self.judgements_of_light.len() {
                let id = self.resources[self.judgements_of_light[index].metrics]
                    .id
                    .clone();
                let metrics = self.new_resource_metrics_of(side, id, ResourceKind::Health);
                self.judgements_of_light[index].copy_metrics.push(metrics);
            }
            let main_armor = self.armor_category.as_ref().map(|(category, _)| *category);
            let on_first: Vec<usize> = (0..self.exclusive.len())
                .filter(|&category| {
                    self.exclusive[category]
                        .members
                        .iter()
                        .all(|member| member.aura.side == Side::Target)
                })
                .collect();
            for category in on_first {
                let copy = self.exclusive[category].on_unit(side);
                self.exclusive.push(copy);
                if Some(category) == main_armor {
                    self.extra_armor_categories
                        .push((self.exclusive.len() - 1, side));
                }
            }
        }
    }

    /// Go `GetStat(stats.Spirit)` of the player for the active stat auras, or `reset`, the
    /// prepared value, when no combination changes it.
    pub(crate) fn player_spirit(&self, reset: f64) -> f64 {
        self.stat_spirit
            .get(self.stat_mask as usize)
            .copied()
            .flatten()
            .unwrap_or(reset)
    }

    /// Go's school spell damage stats of the player, for the active stat auras.
    pub(crate) fn player_school_damage(&self) -> &[f64; 8] {
        self.stat_school_damage
            .get(self.stat_mask as usize)
            .and_then(Option::as_ref)
            .unwrap_or(&self.config.school_damage)
    }

    /// The player's resistances for the active stat auras, over the reset values in `reset`.
    pub(crate) fn player_resistance(&self, reset: &[f64; 8]) -> [f64; 8] {
        let mut values = *reset;
        if let Some(changed) = self.stat_resistance.get(self.stat_mask as usize) {
            for &(index, value) in changed {
                values[index] = value;
            }
        }
        values
    }

    /// Go `healthBar.MaxHealth` of the player: the Health stat of the active stat auras.
    pub(crate) fn player_max_health(&self) -> f64 {
        self.stat_health
            .get(self.stat_mask as usize)
            .copied()
            .flatten()
            .unwrap_or(self.config.max_health)
    }

    /// Go `healthBar.RemoveHealth` on the player.
    pub(crate) fn remove_health(&mut self, amount: f64) {
        assert!(amount >= 0.0, "negative health removal");
        let max_health = self.player_max_health();
        let old = self.player.health;
        let new = (old - amount).max(0.0);
        let resource = &mut self.resources[self.damage_taken_health];
        resource.events += 1;
        resource.gain += -amount;
        resource.actual_gain += new - old;
        if self.log.is_some() {
            let line = format!(
                "Spent {amount:.3} health from {} ({old:.3} --> {new:.3}) of {max_health:.0} total.",
                log::action_string(&self.resources[self.damage_taken_health].id),
            );
            self.player_log(&line);
        }
        self.player.health = new;
        self.changed(reads::RESOURCES);
    }

    /// Go `RandomFloat(label)`.
    pub(crate) fn random(&mut self, label: &str) -> f64 {
        self.rng.next_f64(label)
    }

    /// Go `Proc(p, label)` labeled with an aura's label, without copying the label.
    pub(crate) fn proc_for_aura(&mut self, chance: f64, aura: AuraRef) -> bool {
        let label = &self.trackers[aura.side.index()].auras[aura.index].label;
        self.rng.proc(chance, label)
    }

    /// `random` labeled with an aura's label, as Go proc triggers do.
    pub(crate) fn random_for_aura(&mut self, aura: AuraRef) -> f64 {
        let label = &self.trackers[aura.side.index()].auras[aura.index].label;
        self.rng.next_f64(label)
    }

    /// Go `Proc(p, label)`: no draw at the extremes.
    pub(crate) fn proc(&mut self, chance: f64, label: &str) -> bool {
        self.rng.proc(chance, label)
    }

    pub(crate) fn schedule(&mut self, time: i64, priority: i32, action: Action) -> Handle {
        self.queue.push(time, priority, action)
    }

    /// Go `rescheduleTracker`.
    pub(crate) fn reschedule_tracker(&mut self, time: i64) {
        self.min_tracker_time = self.min_tracker_time.min(time);
    }

    /// Run every iteration and return the aggregate report.
    pub(crate) fn run(&mut self) -> FightReport {
        if !self.extra_targets_built {
            self.build_extra_targets();
            self.build_exclusive_tracking();
            self.apply_enemy_slow_priorities();
        }
        let started = crate::core::stopwatch::Stopwatch::start();
        if world_bridge::enabled() {
            self.config.iterations = world_bridge::requested_iterations(self.config.iterations);
        }
        let iterations = self.config.iterations;
        let mut total_duration = 0i64;
        let mut first_duration = 0i64;
        let mut logs = String::new();
        for iteration in 0..iterations {
            let seed = self.config.seed + i64::from(iteration);
            self.rng.reseed(seed as u64);
            // Go keeps one buffer: every iteration with debug, otherwise only the first.
            let logged = self.config.debug || (iteration == 0 && self.config.debug_first_iteration);
            self.log = logged.then(Vec::new);
            if world_bridge::enabled() { world_bridge::begin_iteration(iteration); }
            self.run_once();
            if let Some(lines) = self.log.take() {
                logs.extend(lines);
            }
            if iteration == 0 {
                first_duration = self.duration;
            }
            total_duration += self.duration;
        }
        let elapsed_ns = started.elapsed_ns();
        self.report(first_duration, total_duration, logs, elapsed_ns)
    }

    /// Go `runOnce`: reset, prepull, the event loop and cleanup.
    fn prepare_first_native_pull(&mut self) {
        self.reset();
        // Go PrePull: time starts at the first prepull action; later ones run first at a tie.
        let count = self.prepull.len() as i32;
        // Forever's Oil of Immolation rule: the oil is used before the pull, even with no prepull.
        let start = match (self.prepull.first(), self.oil_of_immolation_start()) {
            (Some(&(first, _)), Some(oil)) => Some(first.min(oil)),
            (Some(&(first, _)), None) => Some(first),
            (None, oil) => oil,
        };
        if let Some(start) = start {
            self.now = start;
        }
        for index in 0..self.prepull.len() {
            let (at, act) = self.prepull[index];
            let action = match act {
                rotation::PrepullAct::Cast(spell) => Action::Prepull(spell),
                rotation::PrepullAct::CastAtPlayer(spell) => Action::PrepullAtPlayer(spell),
                rotation::PrepullAct::Channel(spell, item) => Action::PrepullChannel(spell, item),
                rotation::PrepullAct::StrictSequence(item) => Action::PrepullSequence(item),
                rotation::PrepullAct::ItemSwap(to_swapped) => Action::PrepullItemSwap(to_swapped),
                rotation::PrepullAct::SwapBackToMain => Action::PrepullSwapBack,
                rotation::PrepullAct::ChangeTarget => Action::PrepullChangeTarget,
                rotation::PrepullAct::CastAllStatBuffCooldowns(list) => {
                    Action::PrepullList(false, list)
                }
                rotation::PrepullAct::ActivateAllStatBuffProcAuras(swap1, list) => {
                    Action::PrepullList(swap1, list)
                }
                rotation::PrepullAct::ActivateAura(aura) => Action::PrepullAura(aura),
                rotation::PrepullAct::CancelAura(aura) => Action::PrepullCancelAura(aura),
                rotation::PrepullAct::ActivateAuraWithStacks(aura, stacks) => {
                    Action::PrepullAuraWithStacks(aura, stacks)
                }
                rotation::PrepullAct::TriggerIcd(aura) => Action::PrepullTriggerIcd(aura),
                rotation::PrepullAct::Move(range) => Action::PrepullMove(range),
                rotation::PrepullAct::GroupMove(range) => Action::PrepullGroupMove(range),
                rotation::PrepullAct::MoveDuration(duration) => {
                    Action::PrepullMoveDuration(duration)
                }
            };
            self.schedule(at, PRIORITY_PREPULL + count - index as i32, action);
        }
        self.schedule(0, PRIORITY_PREPULL + count + 1, Action::EncounterStart);
    }

    fn run_once(&mut self) {
        self.prepare_first_native_pull();
        while self.step() {}
        // A real applied gsim lethal ACK ends this pull even if the saved
        // encounter's duration has not elapsed. Preserve the actual world
        // clock in the native result rather than charging an unplayed tail.
        if world_bridge::enabled() && world_bridge::acknowledged_lethal() {
            self.duration = self.now.max(1);
        }
        // Cleanup can emit final source-native damage (for example from
        // expiring effects or pet teardown). Keep the world bridge open until
        // those events have been included in the terminal exchange and ACK.
        self.cleanup();
        if world_bridge::enabled() {
            world_bridge::finish(self.duration, self.encounter_damage_taken);
        }
    }


    /// Multiple distinct gsim world deaths on one retained source-Rust actor.
    /// Native paid rest is charged between ACKed kills; this alone does not certify P4–P6.
    /// Source-native, clock-paid recovery duration is a candidate choice,
    /// not a fabricated health/mana refill. Preserve one actor and live ACKs.
    pub(crate) fn native_two_world_targets_with_recovery(
        &mut self,
        target_count: usize,
        recovery_seconds: u32,
    ) -> Result<serde_json::Value, &'static str> {
        self.native_two_world_targets_with_strategy(
            target_count, WorldRecoveryStrategy::Fixed(recovery_seconds))
    }

    /// Full ordinary vendor food/water when real carried resource thresholds
    /// require it. This preserves the old Decisions recovery policy without
    /// running a second Python recovery simulator.
    pub(crate) fn native_two_world_targets_with_decisions_recovery(
        &mut self, target_count: usize,
        health_fraction: f64, mana_fraction: f64,
    ) -> Result<serde_json::Value, &'static str> {
        self.native_two_world_targets_with_strategy(target_count,
            WorldRecoveryStrategy::Decisions { health_fraction, mana_fraction })
    }

    fn native_two_world_targets_with_strategy(
        &mut self,
        target_count: usize,
        strategy: WorldRecoveryStrategy,
    ) -> Result<serde_json::Value, &'static str> {
        if let WorldRecoveryStrategy::Fixed(recovery_seconds) = strategy {
            if !(1..=120).contains(&recovery_seconds) {
                return Err("RUST_WORLD_RECOVERY_SECONDS_INVALID");
            }
        }
        if !world_bridge::enabled() {
            return Err("RUST_MULTI_WORLD_TARGETS_REQUIRES_GSIM");
        }
        if !(2..=100).contains(&target_count) {
            return Err("RUST_MULTI_WORLD_TARGET_COUNT_INVALID");
        }
        if !self.extra_targets_built {
            self.build_extra_targets();
            self.build_exclusive_tracking();
            self.apply_enemy_slow_priorities();
        }
        self.rng.reseed(self.config.seed as u64);
        self.log = None;
        world_bridge::begin_iteration(0);
        self.prepare_first_native_pull();
        let food_water_metrics = if matches!(strategy, WorldRecoveryStrategy::Decisions { .. }) {
            Some(self.register_world_food_water_metrics())
        } else {
            None
        };
        let mut windows = Vec::with_capacity(target_count);
        let mut previous_damage = 0.0;
        let mut previous_health_damage = 0.0_f64;
        let mut previous_health_damage_events = 0;
        let mut previous_native_health = self.player.health;
        let mut previous_mana_regen_gain = 0.0_f64;
        let mut previous_mana_regen_events = 0_i32;
        // These values come from the actual retained Rust actor/spell metrics,
        // not a second cooldown / resource simulation or estimated rotation.
        let mut previous_mana_spent = self.player.mana_spent;
        let mut previous_mana_gained = self.player.mana_gained;
        let mut previous_spell_casts: Vec<u32> =
            self.spells.iter().map(|spell| spell.casts).collect();

        for index in 0..target_count {
            let mut native_recovery_ticks = 0;
            let mut native_recovery_elapsed_ns = 0;
            let mut decisions_receipt = WorldRecoveryReceipt::default();
            if index != 0 {
                // Retain the Rust actor, resources and all native queue state.
                // Start next 2D target's range feed before paid travel/rest.
                let killed_at = self.now;
                let world_started_at = killed_at
                    .checked_add(1_000_000)
                    .ok_or("RUST_WORLD_TARGET_CLOCK_OVERFLOW")?;
                world_bridge::begin_next_target(world_started_at)?;
                let next_at = match strategy {
                    WorldRecoveryStrategy::Fixed(seconds) => {
                        let until = killed_at
                            .checked_add(i64::from(seconds) * crate::core::time::NS_PER_SECOND)
                            .ok_or("RUST_WORLD_TARGET_CLOCK_OVERFLOW")?;
                        native_recovery_ticks = self.recover_native_world_actor_until(until)?;
                        native_recovery_elapsed_ns = until - killed_at;
                        until
                    }
                    WorldRecoveryStrategy::Decisions {
                        health_fraction, mana_fraction
                    } => {
                        let (food, water) = food_water_metrics.expect("Decisions source metrics");
                        decisions_receipt = self.recover_native_world_actor_decisions(
                            health_fraction, mana_fraction, food, water)?;
                        native_recovery_ticks = decisions_receipt.passive_ticks;
                        native_recovery_elapsed_ns = decisions_receipt.elapsed_ns;
                        self.now
                    }
                };
                let next_end = next_at
                    .checked_add(self.config.base_duration)
                    .ok_or("RUST_WORLD_TARGET_DURATION_OVERFLOW")?;
                self.begin_next_target_with_bridge(next_at, next_end, false)?;
            }
            // Snapshot only AFTER paid travel/food/water. Subtracting the last
            // kill's cumulative metrics would incorrectly call resting regen
            // "in-combat" regen and pollute 3-mob optimizer features.
            let combat_started_at_ns = self.now;
            let combat_start_health = self.player.health;
            let combat_start_mana = self.player.mana;
            let combat_start_health_damage = -self.resources[self.damage_taken_health].gain;
            let combat_start_mana_regen_gain =
                self.resources[self.mana_regen_casting].actual_gain
                + self.resources[self.mana_regen_not_casting].actual_gain;
            let combat_start_mana_regen_events =
                self.resources[self.mana_regen_casting].events
                + self.resources[self.mana_regen_not_casting].events;
            let combat_start_mana_spent = self.player.mana_spent;
            let combat_start_mana_gained = self.player.mana_gained;
            while self.step() {}
            if !world_bridge::acknowledged_lethal() {
                return Err("RUST_MULTI_WORLD_TARGET_NOT_NATIVE_LETHAL");
            }
            let total_damage = self.encounter_damage_taken;
            // Only native Rust owns player damage and its resource metrics.
            // Health may regenerate between attacks; remaining HP is not damage taken.
            let health_damage = -self.resources[self.damage_taken_health].gain;
            let health_damage_events = self.resources[self.damage_taken_health].events;
            if health_damage < previous_health_damage
                || health_damage_events < previous_health_damage_events
            {
                return Err("RUST_WORLD_HEALTH_DAMAGE_RECEIPT_NONMONOTONIC");
            }
            // Existing source-native 2-second mana ticks own these gains.
            // These events include in-combat ticks, NOT paid between-pull rest.
            let mana_regen_gain = self.resources[self.mana_regen_casting].actual_gain
                + self.resources[self.mana_regen_not_casting].actual_gain;
            let mana_regen_events = self.resources[self.mana_regen_casting].events
                + self.resources[self.mana_regen_not_casting].events;
            let regen = split_world_regeneration_receipt(
                previous_mana_regen_gain, combat_start_mana_regen_gain,
                mana_regen_gain, previous_mana_regen_events,
                combat_start_mana_regen_events, mana_regen_events,
            )?;
            let spent = self.player.mana_spent - previous_mana_spent;
            let gained = self.player.mana_gained - previous_mana_gained;
            let combat_mana_spent = self.player.mana_spent - combat_start_mana_spent;
            let combat_mana_gained = self.player.mana_gained - combat_start_mana_gained;
            if !spent.is_finite() || !gained.is_finite()
                || !combat_mana_spent.is_finite() || !combat_mana_gained.is_finite()
                || spent < -1e-8 || gained < -1e-8
                || combat_mana_spent < -1e-8 || combat_mana_gained < -1e-8
            {
                return Err("RUST_WORLD_MANA_CAST_RECEIPT_NONMONOTONIC");
            }
            // Actual casts since the previous world death. Long cooldown
            // *availability* alone must never be reported as cooldown *usage*.
            let health_gain = self.player.health - previous_native_health
                + (health_damage - previous_health_damage);
            let combat_health_gain = self.player.health - combat_start_health
                + (health_damage - combat_start_health_damage);
            if !health_gain.is_finite() || health_gain < -1e-6
                || !combat_health_gain.is_finite() || combat_health_gain < -1e-6
            {
                return Err("RUST_WORLD_HEALTH_GAIN_BUDGET_INVALID");
            }
            let cooldown_states: Vec<_> = self.spells.iter()
                .filter_map(|spell| {
                    let cd_ns = spell.cd.map_or(0, |(_, duration)| duration);
                    let shared_ns = spell.shared_cd.map_or(0, |(_, duration)| duration);
                    let duration_ns = cd_ns.max(shared_ns);
                    if duration_ns < 30 * crate::core::time::NS_PER_SECOND {
                        return None;
                    }
                    let ready_at_ns = spell.cd.map_or(0, |(timer,_)| self.timers[timer])
                        .max(spell.shared_cd.map_or(0, |(timer,_)| self.timers[timer]));
                    Some(serde_json::json!({
                        "spell_id": spell.id.spell_id,
                        "item_id": spell.id.item_id,
                        "duration_ns": duration_ns,
                        "ready_in_ns": (ready_at_ns - self.now).max(0),
                    }))
                }).collect();
            let cast_receipts: Vec<_> = self.spells.iter().enumerate()
                .filter_map(|(position, spell)| {
                    let count = spell.casts.saturating_sub(previous_spell_casts[position]);
                    if count == 0 {
                        return None;
                    }
                    let cooldown_ns = spell.cd.map_or(0, |(_, duration)| duration)
                        .max(spell.shared_cd.map_or(0, |(_, duration)| duration));
                    Some(serde_json::json!({
                        "spell_id": spell.id.spell_id,
                        "item_id": spell.id.item_id,
                        "casts": count,
                        "cooldown_ns": cooldown_ns,
                        "is_long_cooldown": cooldown_ns >= 30 * crate::core::time::NS_PER_SECOND,
                    }))
                }).collect();
            windows.push(serde_json::json!({
                "native_spell_cast_receipts": cast_receipts,
                // Historical cumulative-since-last-kill fields include recovery.
                "native_mana_spent": spent.max(0.0),
                "native_mana_gained": gained.max(0.0),
                "native_health_actual_gain": health_gain.max(0.0),
                // True fight-only features, excluding travel and food/water.
                "native_combat_start_ns": combat_started_at_ns,
                "native_combat_duration_ns": self.now - combat_started_at_ns,
                "native_combat_start_actor_health": combat_start_health,
                "native_combat_start_actor_mana": combat_start_mana,
                "native_combat_mana_spent": combat_mana_spent.max(0.0),
                "native_combat_mana_gained": combat_mana_gained.max(0.0),
                "native_combat_health_actual_gain": combat_health_gain.max(0.0),
                "native_long_cooldown_states": cooldown_states,
                "index": index,
                "native_kill_ns": self.now,
                "native_damage": total_damage - previous_damage,
                "actor_mana": self.player.mana,
                "actor_health": self.player.health,
                "native_health_damage_taken": health_damage - previous_health_damage,
                "native_health_damage_events": health_damage_events - previous_health_damage_events,
                "native_recovery_mana_regen_actual_gain": regen.recovery_gain,
                "native_recovery_mana_regen_events": regen.recovery_events,
                "native_combat_mana_regen_actual_gain": regen.combat_gain,
                "native_combat_mana_regen_events": regen.combat_events,
                "native_passive_recovery_ns": native_recovery_elapsed_ns,
                "native_passive_recovery_ticks": native_recovery_ticks,
                "food_seconds": decisions_receipt.food_ns as f64 / 1e9,
                "water_seconds": decisions_receipt.water_ns as f64 / 1e9,
                "food_health_gain": decisions_receipt.food_health_gain,
                "water_mana_gain": decisions_receipt.water_mana_gain,
                "decisions_full_recovery": decisions_receipt.full_recovery,
            }));
            previous_damage = total_damage;
            previous_health_damage = health_damage;
            previous_health_damage_events = health_damage_events;
            previous_native_health = self.player.health;
            previous_mana_regen_gain = mana_regen_gain;
            previous_mana_regen_events = mana_regen_events;
            previous_mana_spent = self.player.mana_spent;
            previous_mana_gained = self.player.mana_gained;
            for (previous, spell) in previous_spell_casts.iter_mut().zip(&self.spells) {
                *previous = spell.casts;
            }
        }
        // Count only native scheduler regeneration actually paid on the
        // retained actor between ACKed world kills. Time alone, an assumed
        // refill or a completed movement schedule is not regeneration proof.
        let native_passive_recovery_paid = windows.iter().skip(1).all(|window| {
            window["native_passive_recovery_ns"]
                .as_i64().is_some_and(|ns| ns > 0)
                && window["native_passive_recovery_ticks"]
                    .as_u64().is_some_and(|ticks| ticks > 0)
        });
        let report = serde_json::json!({
            "scope": "MULTI_GSIM_2D_KILLS_SAME_NATIVE_ACTOR_NOT_P4_P6",
            "continuous_native_actor": true, "actual_2d_ack": true,
            "paid_recovery": native_passive_recovery_paid,
            "world_targets_killed": target_count,
            "recovery_mode": match strategy {
                WorldRecoveryStrategy::Fixed(_) => "fixed_passive",
                WorldRecoveryStrategy::Decisions { .. } => "decisions_food_water",
            },
            "recovery_seconds_per_gap": match strategy {
                WorldRecoveryStrategy::Fixed(seconds) => Some(seconds),
                WorldRecoveryStrategy::Decisions { .. } => None,
            },
            "thresholds": match strategy {
                WorldRecoveryStrategy::Decisions { health_fraction, mana_fraction } =>
                    Some(serde_json::json!({
                        "health_fraction": health_fraction,
                        "mana_fraction": mana_fraction,
                    })),
                WorldRecoveryStrategy::Fixed(_) => None,
            },
            "windows": windows,
        });
        self.cleanup();
        Ok(report)
    }

    /// Callable two-window source-Rust continuation smoke, NOT a 100-mob/2D gate.
    /// Both windows use real native scheduler actions and the same player/queue.
    /// This diagnostic is duration-bounded; no mob death or paid downtime is claimed.
    pub(crate) fn native_two_pull_windows(&mut self)
        -> Result<serde_json::Value, &'static str>
    {
        if world_bridge::enabled() {
            return Err("RUST_TWO_WINDOW_2D_CONTROLLER_NOT_CONNECTED");
        }
        if !self.extra_targets_built {
            self.build_extra_targets();
            self.build_exclusive_tracking();
            self.apply_enemy_slow_priorities();
        }
        self.rng.reseed(self.config.seed as u64);
        self.log = None;
        self.prepare_first_native_pull();
        while self.step() {}
        let first_end_ns = self.end_of_combat;
        let first = serde_json::json!({
            "index": 0, "end_ns": first_end_ns,
            "actor_mana": self.player.mana, "actor_health": self.player.health,
        });
        let second_end_ns = first_end_ns.checked_add(self.config.base_duration)
            .ok_or("RUST_TWO_WINDOW_DURATION_OVERFLOW")?;
        self.begin_next_target(first_end_ns, second_end_ns)?;
        while self.step() {}
        let second = serde_json::json!({
            "index": 1, "end_ns": self.end_of_combat,
            "actor_mana": self.player.mana, "actor_health": self.player.health,
        });
        let report = serde_json::json!({
            "scope": "NATIVE_TWO_DURATION_WINDOWS_DIAGNOSTIC_NOT_GATE",
            "selected_native_combat": true,
            "actual_2d_ack": false, "paid_recovery": false,
            "continuous_actor": true, "windows": [first, second],
        });
        self.cleanup();
        Ok(report)
    }

    /// Advance a retained native actor through a paid, target-free rest interval.
    /// Consume the existing native scheduled mana/health regeneration actions,
    /// not an invented mana refill. Keep combat events and their handles queued
    /// for the following pull; never execute attacks on a defeated world target.
    fn recover_native_world_actor_until(&mut self, until_ns: i64)
        -> Result<usize, &'static str>
    {
        if until_ns < self.now {
            return Err("RUST_WORLD_RECOVERY_CLOCK_REWOUND");
        }
        let mut ticks = 0;
        // Follow the live 2D clock through travel. A single unobserved time jump
        // would skip the actual SpatialPlayerMoveStep observations and lose
        // native moving/cast-legality receipts even though gsim paid the route.
        // No native attacks are allowed while the actor recovers.
        let mut boundary = self.now;
        while boundary < until_ns {
            boundary = boundary
                .saturating_add(100_000_000)
                .min(until_ns);
            while let Some((at, handle, action)) = self.queue.pop_next_matching_through(
                boundary,
                |action| matches!(action, Action::ManaTick | Action::HealthRegenTick { .. }),
            ) {
                if at > self.now {
                    self.advance_to(at);
                }
                if self.handle_still_valid(handle, &action) {
                    self.run_action(handle, action);
                    ticks += 1;
                }
            }
            self.advance_to(boundary);
            if world_bridge::enabled() {
                let (distance, front, moving) =
                    world_bridge::exchange(boundary, self.duration, self.encounter_damage_taken);
                if distance != self.config.distance {
                    self.config.distance = distance;
                    self.changed(reads::CASTS);
                }
                if self.config.melee.in_front_of_target != front {
                    self.config.melee.in_front_of_target = front;
                    self.changed(reads::CASTS);
                }
                if self.player.moving != moving {
                    self.player.moving = moving;
                    self.changed(reads::CASTS);
                }
            }
        }
        Ok(ticks)
    }

    /// Begin a second distinct world target on the SAME native actor.
    /// No iteration reset, resource reset, cooldown reset, aura reset or queue clear.
    /// A native-only window still starts at the preceding exact window end.
    /// Native-only duration windows continue at the old exact end timestamp.
    /// A live world may kill its target earlier, but only the separate
    /// world_bridge::begin_next_target() validates the actual lethal ACK.
    fn accepts_next_target_clock(&self, at_ns: i64, next_end_ns: i64, live_world: bool) -> bool {
        at_ns >= self.now
            && at_ns >= 0
            && next_end_ns > at_ns
            && (if live_world {
                // The paid passive recovery helper already advanced the
                // retained actor to its next pull's timestamp.
                at_ns <= self.end_of_combat || (at_ns == self.now)
            } else {
                at_ns == self.end_of_combat
            })
    }

    pub(crate) fn begin_next_target(
        &mut self,
        at_ns: i64,
        next_end_ns: i64,
    ) -> Result<(), &'static str> {
        self.begin_next_target_with_bridge(at_ns, next_end_ns, true)
    }

    fn begin_next_target_with_bridge(
        &mut self,
        at_ns: i64,
        next_end_ns: i64,
        start_world_target: bool,
    ) -> Result<(), &'static str> {
        if !self.accepts_next_target_clock(at_ns, next_end_ns, world_bridge::enabled()) {
            return Err("RUST_NEXT_TARGET_CLOCK_INVALID");
        }
        if world_bridge::enabled() && start_world_target {
            world_bridge::begin_next_target(at_ns)?;
        }
        self.advance_to(at_ns);
        // Target auras/armor are new; player, pets, timers and queued
        // native actions remain exactly those of the preceding encounter.
        // A defeated target can still have active debuffs. End ONLY its aura
        // lifetime before reset; never call global cleanup on the actor.
        self.aura_done_iteration(Side::Target);
        self.retire_defeated_world_enemy_attack();
        self.reset_unit(Side::Target);
        if self.config.target_auto_swing_melee {
            // Enemy reset computes an offset from a fight starting at t=0.
            // A retained world's fresh enemy must swing relative to THIS
            // pull, not immediately at a stale timestamp from the first.
            let attack = self.autos.enemy_attack(Side::Target);
            attack.swing_at = at_ns.checked_add(attack.swing_at)
                .ok_or("RUST_NEXT_TARGET_ENEMY_SWING_CLOCK_OVERFLOW")?;
            attack.natural_ready_at = at_ns.checked_add(attack.natural_ready_at)
                .ok_or("RUST_NEXT_TARGET_ENEMY_SWING_CLOCK_OVERFLOW")?;
            attack.previous_swing = at_ns.checked_add(attack.previous_swing)
                .ok_or("RUST_NEXT_TARGET_ENEMY_SWING_CLOCK_OVERFLOW")?;
        }
        self.pull_started_at = at_ns;
        self.duration = next_end_ns;
        self.end_of_combat = next_end_ns;
        self.execute_phase = 0;
        self.next_execute_phase();
        self.schedule(at_ns, PRIORITY_PREPULL + 1, Action::EncounterStart);
        Ok(())
    }

    /// Go `Simulation.reset` and `Environment.reset`.
    fn reset(&mut self) {
        self.in_prepull = true;
        self.duration = self.config.base_duration;
        if self.config.duration_variation != 0 {
            let variation = self.config.duration_variation * 2;
            let roll = self.random("sim duration");
            self.duration += (roll * variation as f64) as i64 - self.config.duration_variation;
        }
        self.queue.clear();
        if let Some(burst) = &mut self.burst {
            burst.reset();
        }
        // Casts still in flight at the end of the last fight never land.
        self.free_travel_batches = (0..self.travel_batches.len()).collect();
        for batch in &mut self.travel_batches {
            batch.clear();
        }
        self.pull_started_at = 0;
        self.end_of_combat = self.duration;
        self.execute_phase = 0;
        self.next_execute_phase();
        self.now = 0;
        self.min_tracker_time = NEVER_EXPIRES;
        self.min_task_time = NEVER_EXPIRES;
        // Go Environment.reset always rolls the pet heartbeat offset, even without pets.
        let roll = self.random("Pet Stat Inheritance");
        let prepull_start = self.prepull.first().map_or(0, |&(at, _)| at);
        self.heartbeat_offset = prepull_start - pet::PET_UPDATE_INTERVAL
            + crate::rotation::duration_from_seconds(pet::PET_UPDATE_INTERVAL_SECONDS * roll);
        self.encounter_damage_taken = 0.0;
        // Go resets every target before the raid, in unit index order.
        for side in self.target_sides() {
            self.reset_unit(side);
        }
        self.reset_unit(Side::Player);
        self.reset_cooldown_manager();
        A::agent_reset(self);
        // Go Character.reset swaps back to the Main set after the agent's reset.
        self.item_swap_reset();
        // The auras the reset and the agent activated have run their movement speed effects;
        // the exporter read where they leave the multiplier.
        self.move_multiplier = self.reset_move_multiplier;
        // Go Character.reset resets the pets after the owner's agent.
        self.reset_pets();
        // Go initManaTickAction, after the environment reset: two seconds after the prepull
        // starts, when the player or any of its registered pets has a mana bar.
        let pet_mana_bar = self.active_pets.iter().any(|pet| pet.config.max_mana > 0.0)
            || self.pets.iter().any(|pet| pet.tto.is_some());
        if self.has_mana_bar() || pet_mana_bar {
            let prepull_start = self.prepull.first().map_or(0, |&(at, _)| at);
            self.schedule(
                prepull_start + 2 * crate::core::time::NS_PER_SECOND,
                PRIORITY_REGEN,
                Action::ManaTick,
            );
        }
        self.in_prepull = false;
    }

    /// Go `Unit.reset` for a target, or followed by `Character.reset` for the player.
    fn reset_unit(&mut self, side: Side) {
        if let Some(position) = side.target_position() {
            // Go restores the initial pseudo stats; auras that changed stats undid them as
            // they faded at the end of the last fight.
            self.targets[position].state = TargetState {
                resistance: self.config.target_resistance,
                school_damage_taken_multiplier: self.config.target_school_damage_taken_multiplier,
                school_bonus_spell_damage: self.config.target_school_bonus_spell_damage,
            };
        }
        if side == Side::Player {
            self.timers.fill(STARTING_CD_TIME);
            // Go Unit.reset: the player starts at its distance and speed, standing still.
            self.config.distance = self.start_distance;
            self.move_multiplier = self.initial_move_multiplier;
            self.player.moving = false;
            self.player.movement = None;
            self.player.movement_ended_at = -NEVER_EXPIRES;
            // Go UnitMetrics.reset.
            self.death.died = false;
            let player = &mut self.player;
            player.gcd = STARTING_CD_TIME;
            player.rotation_timer = STARTING_CD_TIME;
            player.hardcast = Hardcast::idle(STARTING_CD_TIME);
            player.hardcast_action = None;
            player.reduced_avoidance = false;
            player.rotation_action = None;
            player.channeled_dot = None;
            player.queued = None;
            player.mana_spent = 0.0;
            player.mana_gained = 0.0;
            player.oom_time = 0;
            player.went_oom = false;
            player.first_oom = 0;
            for resource in &mut self.resources {
                resource.previous_events = resource.events;
                resource.previous_actual_gain = resource.actual_gain;
            }
            let initial = self.config.initial;
            player.spell_cost_percent_modifier = initial.spell_cost_percent_modifier;
            player.threat_multiplier = self.config.threat_multiplier;
            player.school_damage_dealt_multiplier = self.config.school_damage_dealt_multiplier;
            self.healing_dealt_factor = 1.0;
            player.disable_dw_miss_penalty = self.config.melee.disable_dw_miss_penalty;
            player.damage_taken_multiplier = self.config.damage_taken_multiplier;
            player.school_damage_taken_multiplier = self.config.school_damage_taken_multiplier;
            player.damage_dealt_multiplier = self.config.damage_dealt_multiplier;
            player.cast_speed_multiplier = initial.cast_speed_multiplier;
            player.attack_speed_multiplier = self.config.melee.attack_speed_multiplier;
            player.melee_speed_multiplier = self.config.melee.melee_speed_multiplier;
            player.ranged_speed_multiplier = self.config.ranged_speed_multiplier;
            player.powers = self.config.powers;
            self.stat_mask = 0;
            if let Some(path) = self.mp5_path.as_mut() {
                path.value = self.config.powers.mp5;
            }
            player.spirit_regen_rate_casting = initial.spirit_regen_rate_casting;
            player.spirit_regen_multiplier = initial.spirit_regen_multiplier;
            player.force_full_spirit_regen = initial.force_full_spirit_regen;
            player.five_second_rule_refresh = 0;
            player.spirit_attribution = None;
            player.casting_regen_sources.clear();
            self.reset_auto_attacks();
            // An aura an item swap switches that changed the armor ignore factor undid it as it
            // faded at the end of the last fight; the reset's activation is in the prepared value.
            if let Some(swap) = &self.item_swap {
                self.config.melee.armor_ignore_factor = swap.armor_ignore_reset;
            }
            // Go runs reset effects first in the aura tracker's reset.
            self.reset_mods();
            A::reset(self);
            // Go ApplyFixedUptimeAura's reset effect: the periodic roll, then the first one.
            for index in 0..self.fixed_uptime.len() {
                let fixed = self.fixed_uptime[index];
                self.schedule(
                    self.now + fixed.tick_length,
                    PRIORITY_GCD,
                    Action::FixedUptime {
                        index,
                        first: false,
                    },
                );
                self.schedule(
                    self.now + fixed.start_time,
                    PRIORITY_GCD,
                    Action::FixedUptime { index, first: true },
                );
            }
        }
        self.reset_auras(side);
        // Go driveWindfuryTotem's OnReset: the totem refreshes every period from the reset.
        if side == Side::Player {
            self.oil_of_immolation_reset();
            if let Some(windfury) = &self.windfury {
                let period = windfury.period;
                self.schedule(period, PRIORITY_AUTO, Action::WindfuryRefresh);
            }
        }
        // Go ScheduledAura's OnReset: the ramp's first tick at the pull, at dot priority.
        if let Some(position) = side.target_position() {
            let unit = &mut self.targets[position];
            unit.armor = self.config.melee.defender_armor;
            unit.armor_delta = 0.0;
            if self.sunder.is_some() {
                self.schedule(self.now, PRIORITY_DOT, Action::SunderTick(position as u8, 0));
            }
            if let Some(expose) = &self.external_expose {
                let delay = expose.delay;
                self.targets[position].armor = expose.armor_before;
                self.schedule(self.now + delay, PRIORITY_DOT, Action::ExternalExpose(position as u8));
            }
        }
        if side == Side::Player {
            // Only the slots of the fight's targets and the player ever hold metrics.
            let units = self.defender_count();
            for spell in &mut self.spells {
                // Go `Spell.reset` and the reset's clear of `SpellsInFlight`.
                spell.casts = 0;
                spell.in_flight = 0;
                spell.metrics[..units].fill(SpellMetrics::default());
                for split in &mut spell.split_metrics {
                    split[..units].fill(SpellMetrics::default());
                }
            }
            self.player.mana = self.config.max_mana;
            self.player.health = self.health_at_reset.unwrap_or(self.config.max_health);
            self.player.mana_regen_multiplier = 1.0;
            self.player.waiting_for_mana = 0.0;
            self.player.waiting_for_mana_start = 0;
            self.update_mana_regen_rates();
            // Go energyBar.reset, after the mana and health bars.
            let prepull_start = self.prepull.first().map_or(0, |&(at, _)| at);
            self.reset_energy(prepull_start);
            self.reset_rage();
        }
        // Go AutoAttacks.reset: an enemy with a melee swing rolls its opening offset, in unit
        // index order. A tank has every copy swing at it; otherwise they only keep the timer
        // their parry haste reads.
        if side.is_target() && self.config.target_auto_swing_melee {
            let roll = self.random("Enemy Swing Offset");
            self.reset_enemy_attack(side, roll);
        }
        self.rotation_reset(side);
        // Go addTracker: each target's tracker first, then the player's.
        let tracker_min = self.trackers[side.index()].min_expires;
        self.reschedule_tracker(tracker_min);
    }

    /// Go `nextExecutePhase` for a fight that ends by duration.
    fn next_execute_phase(&mut self) {
        self.next_execute = NEVER_EXPIRES;
        let [p90, p45, p35, p25, p20] = self.config.execute_proportions;
        let (phase, proportion) = match self.execute_phase {
            0 => (100, p90),
            100 => (90, p45),
            90 => (45, p35),
            45 => (35, p25),
            35 => (25, p20),
            25 => {
                self.execute_phase = 20;
                return;
            }
            phase => panic!("executePhase = {phase} invalid"),
        };
        self.execute_phase = phase;
        self.next_execute = self.pull_started_at +
            ((1.0 - proportion) * (self.end_of_combat - self.pull_started_at) as f64) as i64;
    }

    /// Go `IsExecutePhase20`.
    pub(crate) fn is_execute_phase_20(&self) -> bool {
        self.execute_phase <= 20
    }

    /// Go `IsExecutePhase35`.
    pub(crate) fn is_execute_phase_35(&self) -> bool {
        self.execute_phase <= 35
    }

    /// Go `Simulation.Step`. Returns false when the fight is over.
    fn step(&mut self) -> bool {
        // A committed source-native lethal world ACK must stop scheduling
        // attacks on the now-dead target. The same actor may later be
        // continued explicitly using begin_next_target, without a reset.
        if world_bridge::enabled() && world_bridge::acknowledged_lethal() {
            return false;
        }
        if world_bridge::enabled() {
            let next_native = self.queue.peek_time().unwrap_or(NEVER_EXPIRES)
                .min(self.autos.min_time).min(self.min_task_time)
                .min(self.end_of_combat);
            if let Some(boundary) = world_bridge::due(next_native, self.end_of_combat) {
                if boundary > self.now { self.advance_to(boundary); }
                let (distance, front, moving) = world_bridge::exchange(
                    boundary, self.duration, self.encounter_damage_taken);
                if distance != self.config.distance {
                    self.config.distance = distance;
                    self.changed(reads::CASTS);
                }
                // Facing can invalidate cached APL FrontOfTarget conditions
                // even when the distance is unchanged. Mirror range's
                // production invalidation at the native scheduler boundary.
                if self.config.melee.in_front_of_target != front {
                    self.config.melee.in_front_of_target = front;
                    self.changed(reads::CASTS);
                }
                // The source-Rust spell cast rules already test player.moving;
                // use the real gsim route status to enforce them, including
                // between SpatialPlayerMoveStep events, without teleporting.
                if self.player.moving != moving {
                    self.player.moving = moving;
                    self.changed(reads::CASTS);
                }
                return true;
            }
        }
        // Go runs due weapon swings before the next pending action, ties included.
        let next = self.queue.peek_time().unwrap_or(NEVER_EXPIRES);
        if self.due_weapon_attack(next) && self.autos.min_time <= self.min_task_time {
            if self.autos.min_time > self.end_of_combat {
                return false;
            }
            self.advance_weapon_attacks();
            return true;
        }
        // Then due tasks, the energy ticks.
        if self.due_task(next) {
            if self.min_task_time > self.end_of_combat {
                return false;
            }
            self.advance_tasks();
            return true;
        }
        // Preserve scheduled post-pull events for a continuing native session.
        // The existing one-shot iteration still resets the queue on its next run.
        if next > self.end_of_combat {
            return false;
        }
        let Some((time, handle, action)) = self.queue.pop() else {
            return false;
        };
        if time > self.end_of_combat {
            return false;
        }
        // Go marks an action consumed when it is popped, before the advance: an aura the
        // advance expires that changes the owner's stats schedules the next heartbeat.
        if let Action::PetInheritance(side) = action {
            self.pet_inheritance_popped(side);
        }
        if time > self.now {
            self.advance_to(time);
        }
        // An expiring aura can cancel the action that is about to run.
        if !self.handle_still_valid(handle, &action) {
            return true;
        }
        self.run_action(handle, action);
        true
    }

    /// Go `Simulation.advance`: expire auras whose time has come.
    pub(crate) fn advance_to(&mut self, time: i64) {
        self.now = time;
        // Go loops so equal proportions pass several phases in one advance, calling the execute
        // phase callbacks after each.
        while self.now >= self.next_execute {
            self.next_execute_phase();
            let phase = self.execute_phase;
            A::on_execute_phase(self, phase);
        }
        if self.now >= self.min_tracker_time {
            self.min_tracker_time = NEVER_EXPIRES;
            // Go adds the pet's tracker when the pet is enabled, after the player's, and
            // removes it when the pet is disabled.
            // Go adds every target's tracker in unit index order, then the player's.
            for position in 0..self.targets.len() {
                let next = self.try_advance_tracker(Side::target(position));
                self.min_tracker_time = self.min_tracker_time.min(next);
            }
            let next = self.try_advance_tracker(Side::Player);
            self.min_tracker_time = self.min_tracker_time.min(next);
            let mut position = 0;
            while position < self.pet_trackers.len() {
                let next = self.try_advance_tracker(Side::Pet(self.pet_trackers[position]));
                self.min_tracker_time = self.min_tracker_time.min(next);
                position += 1;
            }
        }
    }

    fn run_action(&mut self, handle: Handle, action: Action) {
        match action {
            Action::EncounterStart => self.encounter_start(),
            Action::ManaTick => {
                // Go ticks every player with a mana bar, then every enabled pet with one.
                if self.has_mana_bar() {
                    self.mana_tick();
                }
                for index in 0..self.active_pets.len() {
                    let side = Side::Pet(index as u8);
                    let pet = self.active_pet(side);
                    if pet.enabled && pet.config.max_mana > 0.0 {
                        self.pet_mana_tick(side);
                    }
                }
                let next = self.now + 2 * crate::core::time::NS_PER_SECOND;
                self.schedule(next, PRIORITY_REGEN, Action::ManaTick);
            }
            Action::Rotation(side) => {
                if self.unit(side).rotation_action == Some(handle) {
                    self.unit_mut(side).rotation_action = None;
                }
                self.complete_due_hardcast_of(side);
                self.do_next_action_of(side);
            }
            Action::Hardcast(side) => {
                if self.unit(side).hardcast_action == Some(handle) {
                    self.unit_mut(side).hardcast_action = None;
                }
                self.complete_due_hardcast_of(side);
            }
            Action::QueuedCast(side @ Side::Pet(_)) => {
                if let Some(queued) = self.unit_mut(side).queued.as_mut() {
                    if queued.action == Some(handle) {
                        queued.action = None;
                    }
                    let (spell, target) = (queued.spell, queued.target);
                    self.cast(spell, target);
                }
            }
            Action::QueuedCast(_) => {
                if let Some(queued) = self.player.queued.as_mut() {
                    if queued.action == Some(handle) {
                        queued.action = None;
                    }
                    let (spell, target) = (queued.spell, queued.target);
                    // A strict sequence's hook unhooks itself, casts, then advances it.
                    let hooks = std::mem::take(&mut self.apl.queue_hooks);
                    let cast = self.cast(spell, target);
                    for item in hooks {
                        self.advance_sequence(item);
                    }
                    // A move the rotation held for this spell goes now, not at the end of the
                    // global cooldown the spell started (Forever's rule, `move_ready`). A
                    // rotation with a move goes on from a queued instant that went off, as it
                    // does after one it casts itself (Forever's rule, README, Rust-only
                    // mechanics); one that failed would only queue it again for this instant.
                    if std::mem::take(&mut self.apl.move_held_for_queue)
                        || (cast
                            && self.rotation_with_moves_looks_through_gcd
                            && self.player.hardcast.expires <= self.now)
                    {
                        self.set_rotation_timer(self.now);
                    }
                }
            }
            Action::Travel { spell, result, dot } => {
                self.spells[spell].in_flight -= 1;
                self.deal_damage(spell, result, false);
                if let (Some(dot), true) = (dot, result.landed()) {
                    self.apply_dot(dot);
                }
            }
            Action::DotTick(dot) => self.periodic_tick(dot, handle),
            Action::TravelBatch(batch) => {
                let results = std::mem::take(&mut self.travel_batches[batch]);
                if let Some(&(spell, _)) = results.first() {
                    self.spells[spell].in_flight -= 1;
                }
                for &(spell, result) in &results {
                    self.deal_damage(spell, result, false);
                }
                self.travel_batches[batch] = results;
                self.travel_batches[batch].clear();
                self.free_travel_batches.push(batch);
            }
            Action::ClassTravel { spell, result } => {
                self.spells[spell].in_flight -= 1;
                if let SpellBehavior::Class(behavior) = self.spells[spell].behavior {
                    A::on_travel(self, spell, result, behavior);
                }
            }
            Action::DelayedProc {
                aura,
                spell,
                result,
                token,
            } => {
                self.delayed_proc(aura, spell, result);
                if let Some(token) = token {
                    A::dispose_clone(self, token);
                }
            }
            Action::Pushback { chance } => self.pushback_handler(chance),
            Action::StruckDamageProc { proc, target } => {
                let spell = self.damage_procs[proc].spell;
                self.cast(spell, target);
            }
            Action::Prepull(spell) => self.cast_or_queue(spell, self.current_target_side()),
            Action::PrepullAtPlayer(spell) => self.cast_or_queue(spell, Side::Player),
            Action::PrepullChannel(spell, item) => self.prepull_channel(spell, item),
            Action::PrepullSequence(item) => self.prepull_sequence(item),
            Action::PrepullItemSwap(to_swapped) => self.item_swap_action(to_swapped),
            // Go's `SwapItems` to Main, which logs nothing of its own.
            Action::PrepullSwapBack => self.swap_items(false, false),
            Action::PrepullChangeTarget => self.log_change_target(),
            Action::PrepullList(swap1, list) => match self.prepull_lists[list].clone() {
                rotation::PrepullList::Spells(spells) => {
                    for spell in spells {
                        self.cast_or_queue(spell, self.current_target_side());
                    }
                }
                // Go: nothing while the other set is worn, else each aura as an activate aura
                // action activates it.
                rotation::PrepullList::Auras(auras) => {
                    if self.item_swap.is_some() && self.item_swapped() != swap1 {
                        return;
                    }
                    for aura in auras {
                        self.activate_aura_action(aura);
                    }
                }
            },
            Action::PrepullAura(aura) => self.activate_aura_action(aura),
            // Go `APLActionCancelAura.Execute`, which a prepull runs whether or not it is ready.
            Action::PrepullCancelAura(aura) => {
                if self.log.is_some() {
                    let id = self.aura(aura).action_id.clone().unwrap_or_default();
                    let line = format!("Cancelling aura {}", log::action_string(&id));
                    self.unit_log(aura.side, &line);
                }
                self.deactivate_aura(aura);
            }
            Action::PrepullAuraWithStacks(aura, stacks) => {
                self.activate_aura_with_stacks_action(aura, stacks)
            }
            // Go `APLActionTriggerICD.Execute`.
            Action::PrepullTriggerIcd(aura) => {
                if self.log.is_some() {
                    let id = self.aura(aura).action_id.clone().unwrap_or_default();
                    let line = format!("Triggering ICD {}", log::action_string(&id));
                    self.unit_log(aura.side, &line);
                }
                let (timer, duration) = self.aura(aura).icd.expect("an ICD aura");
                self.timers[timer] = self.now + duration;
                self.changed(reads::TIMERS);
            }
            Action::PrepullMove(range) => {
                // Go `APLActionMove.Execute` logs the range even when the unit is already there.
                if self.log.is_some() {
                    let line = format!("[DEBUG] Moving to {range:.1} yards");
                    self.player_log(&line);
                }
                self.move_to(Side::Player, range);
            }
            Action::PrepullMoveDuration(duration) => self.move_duration(Side::Player, duration),
            // Go `APLActionMove.IsReady` before the pull: not moving and no cast in progress.
            Action::PrepullGroupMove(range) => {
                if !self.player.moving && self.player.hardcast.expires < self.now {
                    if self.log.is_some() {
                        let line = format!("[DEBUG] Moving to {range:.1} yards");
                        self.player_log(&line);
                    }
                    self.move_to(Side::Player, range);
                }
            }
            Action::SunderTick(target, done) => self.sunder_tick(target, done),
            Action::ExternalExpose(target) => self.external_expose_lands(target),
            Action::DeathCheck => self.death_check(),
            Action::FixedUptime { index, first } => self.fixed_uptime_roll(index, first),
            Action::MovementEnd(side) => {
                if self
                    .unit(side)
                    .movement
                    .is_some_and(|movement| movement.action == handle)
                {
                    self.finalize_movement(side);
                }
            }
            Action::MeleeWeaveWakeup(side) => {
                if !self.main_hand_in_range(side) {
                    self.react_to_event(side);
                }
            }
            Action::FixedShoutChain { index, periodic } => {
                let fixed = self.fixed_uptime[index];
                self.activate_aura(fixed.aura);
                if !periodic {
                    self.schedule(
                        self.now + fixed.duration + 1,
                        PRIORITY_GCD,
                        Action::FixedShoutChain {
                            index,
                            periodic: true,
                        },
                    );
                }
            }
            Action::StackTick {
                aura,
                period,
                ticks_left,
            } => self.stack_tick(aura, period, ticks_left),
            Action::HealthRegenTick { regen, generation } => {
                self.health_regen_tick(regen, generation)
            }
            Action::OilUse => self.oil_use(),
            Action::OilTick(tick) => self.oil_tick(tick),
            Action::WindfuryRefresh => {
                let windfury = self.windfury.as_ref().expect("Windfury Totem is bound");
                let (totem, period) = (windfury.totem, windfury.period);
                self.activate_aura(totem);
                self.schedule(self.now + period, PRIORITY_AUTO, Action::WindfuryRefresh);
            }
            Action::PetInheritance(side) => self.pet_inheritance(side),
            Action::PetTimeout(side) => {
                let pet = self.active_pet_mut(side);
                if pet.timeout == Some(handle) {
                    pet.timeout = None;
                    self.disable_pet(side);
                }
            }
            Action::ClassPending(tag) => A::on_class_action(self, tag),
            Action::ClassPeriodic(periodic) => {
                A::on_periodic(self, periodic.tag);
                let done = periodic.done + 1;
                if periodic.num_ticks == 0 || done < periodic.num_ticks {
                    let next = Periodic { done, ..periodic };
                    self.schedule(
                        self.now + periodic.period,
                        periodic.priority,
                        Action::ClassPeriodic(next),
                    );
                }
            }
            Action::ItemManaTick {
                amount,
                metrics,
                periodic,
            } => {
                self.add_mana(amount, metrics);
                let done = periodic.done + 1;
                if done < periodic.num_ticks {
                    let next = Periodic { done, ..periodic };
                    self.schedule(
                        self.now + periodic.period,
                        periodic.priority,
                        Action::ItemManaTick {
                            amount,
                            metrics,
                            periodic: next,
                        },
                    );
                }
            }
            Action::DelayedDamage { spell, result } => self.deal_damage(spell, result, false),
            Action::React => self.react_to_event_now(),
        }
    }

    /// Go ApplyFixedUptimeAura's actions. The periodic roll activates the aura, adding a stack
    /// when it stacks, and comes back a period later. The first roll activates it once, for a
    /// random share of its duration, so the collapsed chance keeps the uptime.
    /// The chain's OnGain on the player's own aura: come back a reaction time after it ends.
    pub(crate) fn fixed_shout_chain_gain(&mut self, aura: AuraRef) {
        for index in 0..self.fixed_uptime.len() {
            if self.fixed_uptime[index].chained_by == Some(aura) {
                let at = self.aura(aura).expires + self.config.reaction;
                self.schedule(
                    at,
                    PRIORITY_GCD,
                    Action::FixedShoutChain {
                        index,
                        periodic: false,
                    },
                );
            }
        }
    }

    fn fixed_uptime_roll(&mut self, index: usize, first: bool) {
        let fixed = self.fixed_uptime[index];
        if first {
            if self.proc(fixed.uptime, "FixedAura") {
                let span = (fixed.duration - fixed.tick_length) as f64;
                let random = fixed.tick_length + (span * self.random("FixedAuraDur")) as i64;
                self.aura_mut(fixed.aura).duration = random;
                self.activate_aura(fixed.aura);
                self.aura_mut(fixed.aura).duration = fixed.duration;
            }
            return;
        }
        if self.proc(fixed.chance_per_tick, "FixedAura") {
            self.activate_aura(fixed.aura);
            if self.aura(fixed.aura).max_stacks > 0 {
                self.add_stack(fixed.aura);
            }
        }
        self.schedule(
            self.now + fixed.tick_length,
            PRIORITY_GCD,
            Action::FixedUptime {
                index,
                first: false,
            },
        );
    }

    /// Go `AddPendingAction` for a class: the action runs at `at`, unless cancelled through the
    /// returned handle.
    pub(crate) fn schedule_class_action(&mut self, at: i64, priority: i32, tag: u32) -> Handle {
        self.schedule(at, priority, Action::ClassPending(tag))
    }

    /// Go `StartPeriodicAction` for a class: the first tick a period from now.
    pub(crate) fn start_class_periodic(
        &mut self,
        tag: u32,
        period: i64,
        num_ticks: i32,
        priority: i32,
    ) {
        let periodic = Periodic {
            tag,
            period,
            num_ticks,
            done: 0,
            priority,
        };
        self.schedule(self.now + period, priority, Action::ClassPeriodic(periodic));
    }

    /// The raid's Expose Armor lands on a target under Forever's rules: the aura goes up and the
    /// target takes its prepared armor, which counts it.
    fn external_expose_lands(&mut self, target: u8) {
        let expose = self.external_expose.clone().expect("the effect is bound");
        let side = Side::target(target as usize);
        let aura = self.aura_on(expose.aura, side);
        self.activate_aura(aura);
        let armor = self.config.melee.defender_armor;
        let unit = &mut self.targets[target as usize];
        unit.armor = armor + unit.armor_delta;
    }

    /// Go driveSunderArmor's periodic action on a target: activate, add a stack, and come back
    /// a period later until every tick has run.
    fn sunder_tick(&mut self, target: u8, done: i32) {
        let ramp = self.sunder.clone().expect("the ramp is bound");
        let side = Side::target(target as usize);
        let aura = self.aura_on(ramp.aura, side);
        if ramp.blocked {
            // Go Aura.Activate counts the proc before the exclusive effect blocks it.
            self.aura_mut(aura).procs += 1;
        } else {
            self.activate_aura(aura);
        }
        if self.aura(aura).active {
            self.add_stack(aura);
        }
        // With the armor category bound, its active member sets the armor.
        if self.armor_category.is_none() {
            let stacks = self.aura(aura).stacks.max(0) as usize;
            let unit = &mut self.targets[target as usize];
            unit.armor =
                ramp.armor_by_stacks[stacks.min(ramp.armor_by_stacks.len() - 1)] + unit.armor_delta;
        }
        if done + 1 < ramp.ticks {
            self.schedule(
                self.now + ramp.period,
                PRIORITY_DOT,
                Action::SunderTick(target, done + 1),
            );
        }
    }

    /// Handles of reusable actions are cleared when consumed so Go's `consumed` checks hold.
    fn handle_still_valid(&mut self, handle: Handle, action: &Action) -> bool {
        match action {
            Action::DotTick(dot) => self.dots[*dot].tick_action == Some(handle),
            Action::ClassPending(tag) => A::class_action_valid(self, *tag, handle),
            _ => true,
        }
    }

    /// Go encounter start: agents and auras, then each unit starts the pull.
    fn encounter_start(&mut self) {
        // No supported unit has encounter start callbacks. Go starts the encounter for every
        // enabled unit, which randomizes the swings, then each starts the pull: its swings,
        // and for the player its rotation at max(0, GCD ready).
        self.randomize_melee_timing(Side::Player);
        let enabled_pets: Vec<Side> = self
            .pet_sides()
            .into_iter()
            .filter(|&side| self.active_pet(side).enabled)
            .collect();
        for &side in &enabled_pets {
            self.randomize_melee_timing(side);
        }
        // Go AllUnits lists the targets first, so their swings join the weapon attacks first,
        // in unit index order.
        self.start_enemy_attacks();
        self.start_auto_attacks(Side::Player);
        let ready = self.player.gcd.max(0);
        self.set_gcd_timer(ready);
        for side in enabled_pets {
            self.start_auto_attacks(side);
        }
    }

    /// Go `Simulation.Cleanup` and `doneIteration`.
    fn cleanup(&mut self) {
        self.now = self.duration;
        self.queue.clear();
        self.player.hardcast = Hardcast::idle(0);
        // Go Character.doneIteration swaps back to the Main set before anything else.
        self.item_swap_reset();
        // Go Character.doneIteration finishes the pets first.
        self.pets_done_iteration();
        self.player_done_iteration();
        self.rage_done_iteration();
        self.aura_done_iteration(Side::Player);
        for spell in 0..self.spells.len() {
            if self.spells[spell].caster == Side::Player {
                self.spell_done_iteration(spell);
            }
        }
        self.enemy_done_iteration();
        let damage = self.totals.iteration_damage;
        // Go finishes each target in unit index order.
        for side in self.target_sides() {
            self.aura_done_iteration(side);
        }
        self.exclusive_done_iteration();
        self.unit_done_iteration(damage);
    }
}

#[cfg(test)]
mod tests {
    use super::{Mp5Path, Side, DEFENDERS, MAX_TARGETS};

    #[test]
    fn native_persistent_early_world_kill_clock_boundary() {
        let prepared: crate::contracts::prepared_v2::PreparedV2 = serde_json::from_str(
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"),
                "/fixtures/mage/prepared-v2/balance-druid-revelation.prepared.json"))
        ).unwrap();
        let mut fight = crate::classes::druid::agent::DruidAgent::fight(&prepared, crate::core::rng::RngMode::Stochastic).unwrap();
        fight.prepare_first_native_pull();
        let now = fight.now;
        let cap = fight.end_of_combat;
        assert!(cap > now + 2_000_000);
        let early = now.max(0) + 1_000_000;
        // A real early gsim world kill can be the next pull timestamp,
        // but the unchanged native-only duration-window cannot.
        assert!(fight.accepts_next_target_clock(early, early + 2_000_000, true));
        assert!(!fight.accepts_next_target_clock(early, early + 2_000_000, false));
        assert!(fight.accepts_next_target_clock(cap, cap + 2_000_000, false));
        assert!(!fight.accepts_next_target_clock(cap + 1, cap + 2_000_000, true));
        assert!(!fight.accepts_next_target_clock(now - 1, cap + 1, true));
        assert!(!fight.accepts_next_target_clock(early, early, true));
        // This test does not fake a lethal source event or certify a hybrid mob.
    }

    #[test]
    fn native_world_recovery_pays_scheduled_mana_without_resetting_actor() {
        let prepared: crate::contracts::prepared_v2::PreparedV2 =
            serde_json::from_str(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/fixtures/mage/prepared-v2/balance-druid-revelation.prepared.json"
            )))
            .unwrap();
        let mut fight = crate::classes::druid::agent::DruidAgent::fight(
            &prepared,
            crate::core::rng::RngMode::Stochastic,
        )
        .unwrap();
        fight.prepare_first_native_pull();
        while fight.step() {}
        let before = fight.now;
        fight.player.mana = 0.0;
        let end = before + 12 * crate::core::time::NS_PER_SECOND;
        let ticks = fight.recover_native_world_actor_until(end).unwrap();
        assert!(ticks > 0, "scheduled native mana ticks were not replayed");
        assert!(fight.player.mana > 0.0, "native mana regeneration did not apply");
        assert_eq!(fight.now, end);
        assert!(fight.accepts_next_target_clock(end, end + 1_000_000_000, true));
    }

    #[test]
    fn native_persistent_next_target_resets_only_enemy_swing_on_monotonic_clock() {
        let prepared: crate::contracts::prepared_v2::PreparedV2 =
            serde_json::from_str(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/fixtures/mage/prepared-v2/feral-bear-druid-defensives.prepared.json"
            )))
            .unwrap();
        let mut fight = crate::classes::druid::agent::DruidAgent::fight(
            &prepared,
            crate::core::rng::RngMode::Stochastic,
        )
        .unwrap();
        fight.prepare_first_native_pull();
        while fight.step() {}
        let at = fight.end_of_combat;
        fight.begin_next_target(at, at + 5 * crate::core::time::NS_PER_SECOND)
            .unwrap();
        assert!(fight.config.target_auto_swing_melee);
        assert!(!fight.autos.enemy.enabled, "enemy cannot attack before next pull starts");
        assert!(fight.autos.enemy.swing_at >= at, "new enemy swing was backdated");
        assert!(fight.autos.enemy.swing_at < at + fight.autos.enemy.cur_swing_duration);
    }

    #[test]
    fn native_persistent_next_target_retains_actor_and_native_queue() {
        use super::{Action, PRIORITY_PREPULL, PRIORITY_REGEN};
        let prepared: crate::contracts::prepared_v2::PreparedV2 = serde_json::from_str(
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"),
                "/fixtures/mage/prepared-v2/balance-druid-revelation.prepared.json"))
        ).unwrap();
        let mut fight = crate::classes::druid::agent::DruidAgent::fight(&prepared, crate::core::rng::RngMode::Stochastic).unwrap();
        if !fight.extra_targets_built {
            fight.build_extra_targets();
            fight.build_exclusive_tracking();
            fight.apply_enemy_slow_priorities();
        }
        eprintln!("NATIVE_NEXT_TARGET_STAGE=initial_reset");
        fight.reset();
        fight.schedule(0, PRIORITY_PREPULL + 1, Action::EncounterStart);
        let mut steps = 0;
        while fight.step() {
            steps += 1;
            assert!(steps < 100_000, "first native pull did not terminate");
        }
        assert!(steps > 1, "native combat did not execute");
        eprintln!("NATIVE_NEXT_TARGET_STAGE=first_pull_done");
        let first_end = fight.end_of_combat;
        let kept_mana = fight.config.max_mana * 0.37;
        let kept_health = fight.config.max_health * 0.61;
        fight.player.mana = kept_mana;
        fight.player.health = kept_health;
        let kept_timer = fight.timers.first().copied();
        let future = fight.queue.push(first_end + 5_000_000, PRIORITY_REGEN, Action::ManaTick);
        let old_now = fight.now;
        assert_eq!(fight.begin_next_target(first_end - 1, first_end + 2_000_000_000),
            Err("RUST_NEXT_TARGET_CLOCK_INVALID"));
        assert_eq!(fight.now, old_now);
        eprintln!("NATIVE_NEXT_TARGET_STAGE=transition");
        fight.begin_next_target(first_end, first_end + 2_000_000_000).unwrap();
        assert_eq!(fight.player.mana, kept_mana);
        assert_eq!(fight.player.health, kept_health);
        assert_eq!(fight.timers.first().copied(), kept_timer);
        assert!(fight.queue.cancel(future), "queued native future action was lost");
        assert!(fight.next_execute >= first_end);
        let mut second_steps = 0;
        while fight.step() {
            second_steps += 1;
            assert!(second_steps < 100_000, "second native pull did not terminate");
        }
        assert!(second_steps > 0, "second target did not run native combat");
        assert!(fight.now >= first_end, "native clock reset on second target");
    }

    #[test]
    fn native_persistent_next_target_offsets_target_armor_events() {
        use super::{AuraRef, ExternalExpose, SunderRamp};
        let prepared: crate::contracts::prepared_v2::PreparedV2 = serde_json::from_str(
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"),
                "/fixtures/mage/prepared-v2/balance-druid-revelation.prepared.json"))
        ).unwrap();
        // Both source-backed armor ramps previously scheduled events at t=0
        // after the first pull, so the next native scheduler could rewind.
        for with_sunder in [true, false] {
            let mut fight = crate::classes::druid::agent::DruidAgent::fight(&prepared, crate::core::rng::RngMode::Stochastic).unwrap();
            if !fight.extra_targets_built {
                fight.build_extra_targets();
                fight.build_exclusive_tracking();
                fight.apply_enemy_slow_priorities();
            }
            fight.prepare_first_native_pull();
            let mut steps = 0;
            while fight.step() {
                steps += 1;
                assert!(steps < 100_000, "first native pull did not terminate");
            }
            let at = fight.end_of_combat;
            assert!(at > 1_000_000_000);
            let aura = AuraRef { side: Side::Target, index: 0 };
            if with_sunder {
                fight.sunder = Some(SunderRamp {
                    aura, period: 2_000_000_000, ticks: 1,
                    armor_by_stacks: vec![0.0], blocked: false,
                });
            } else {
                fight.external_expose = Some(ExternalExpose {
                    aura, delay: 1_000_000_000, armor_before: 0.0,
                });
            }
            fight.begin_next_target(at, at + 3_000_000_000).unwrap();
            assert!(fight.queue.peek_time().unwrap() >= at,
                "second target queued source armor event before its own spawn");
        }
    }

    /// Go adds a bonus to the raw stats and takes it away again: 77 plus Mana Tide Totem's
    /// 483.33 and minus it again is a few ulps short of 77, and the path keeps that.
    #[test]
    fn a_fractional_mp5_bonus_leaves_the_raw_mp5_off_its_start() {
        let bonus = 290.0 * 5000.0 / 3000.0;
        let mut path = Mp5Path {
            value: 77.0,
            bonuses: vec![0.0, bonus],
        };
        assert_eq!(path.change(0b10, 0b10), 77.0 + bonus);
        let after = path.change(0b10, 0b00);
        assert_eq!(after, 76.99999999999994);
        assert_ne!(after, 77.0);
        // A whole number bonus comes back exactly, and a bit that did not change is left alone.
        let mut path = Mp5Path {
            value: 77.0,
            bonuses: vec![31.0, bonus],
        };
        path.change(0b01, 0b01);
        assert_eq!(path.change(0b01, 0b00), 77.0);
        assert_eq!(path.change(0b00, 0b00), 77.0);
    }

    /// Every target and the player come before the pets in per-unit lists, so a spell's
    /// metrics hold one entry for each unit it can hit.
    #[test]
    fn defenders_come_before_pets() {
        let defenders: Vec<usize> = (0..MAX_TARGETS)
            .map(Side::target)
            .chain(std::iter::once(Side::Player))
            .map(Side::index)
            .collect();
        let mut sorted = defenders.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, (0..DEFENDERS).collect::<Vec<_>>());
        assert_eq!(Side::Pet(0).index(), DEFENDERS);
    }

    #[test]
    fn target_positions_round_trip() {
        for position in 0..MAX_TARGETS {
            let side = Side::target(position);
            assert!(side.is_target());
            assert_eq!(side.target_position(), Some(position));
        }
        assert_eq!(Side::target(0), Side::Target);
        assert_eq!(Side::Player.target_position(), None);
        assert_eq!(Side::Pet(0).target_position(), None);
    }
}
