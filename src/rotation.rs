//! A strict subset of Go's APL rotation language.
//!
//! Parsing accepts the protojson rotation carried by prepared v2 and records every
//! operator outside the implemented subset as a named reason. Constant parsing and type
//! coercion follow sim/core/apl_values_operators.go at the pinned revision, including
//! Go `time.ParseDuration` for duration constants.

use serde_json::{Map, Value as Json};

use crate::contracts::prepared_v2::ActionId;

mod groups;

pub use groups::{bind, Binding, Reference};

/// Go `APLValueType`, in coercion priority order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ValueType {
    Int,
    Float,
    Duration,
    String,
    Bool,
}

/// Go `APLValueConst`: every representation is kept because a coerced constant reads
/// the field of its new type, not a conversion of its original value.
#[derive(Clone, Debug, PartialEq)]
pub struct Const {
    pub value_type: ValueType,
    pub int: i32,
    pub float: f64,
    pub duration_ns: i64,
    pub string: String,
    pub boolean: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Go `ShamanTotems_TotemType`, the totem slot a Shaman value reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Totem {
    Earth,
    Air,
    Fire,
    Water,
}

/// Go `UnitReference` as far as a rotation reads it. The player is the one player of the raid,
/// which `Self` and `Player` index 0 both name. Go's current target never changes without a
/// change target action, which is unsupported, so it is always the first target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitRef {
    Player,
    CurrentTarget,
    PreviousTarget,
    NextTarget,
    /// `Target` with its index into the encounter's targets.
    Target(usize),
    /// A reference Go's `Environment.GetUnit` resolves to no unit: `AllPlayers` and
    /// `AllTargets` name sets, which only multi-unit actions read.
    Nobody,
}

/// A unit a [`UnitRef`] resolved to: the player, or a target by its position among the
/// encounter's targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Player,
    Target(usize),
}

thread_local! {
    /// The player's current target a rotation is compiled for, by position: the first target
    /// unless [`with_current_target`] compiles a copy for another.
    static CURRENT_TARGET: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Runs `f` with the current target that unit references resolve against at `position`, as Go
/// `UnitReference.Get` reads the player's `CurrentTarget` where it is asked.
pub fn with_current_target<T>(position: usize, f: impl FnOnce() -> T) -> T {
    let old = CURRENT_TARGET.with(|current| current.replace(position));
    let result = f();
    CURRENT_TARGET.with(|current| current.set(old));
    result
}

impl UnitRef {
    /// Go `Environment.GetUnit` in a fight of `targets` targets, all enabled: the unit, or
    /// `None` where Go finds no unit. The current target is the first unless a copy is compiled
    /// for another ([`with_current_target`]); the previous and next targets wrap around it.
    pub fn resolve(self, targets: usize) -> Option<Unit> {
        let targets = targets.max(1);
        let current = CURRENT_TARGET.with(std::cell::Cell::get) % targets;
        match self {
            UnitRef::Player => Some(Unit::Player),
            UnitRef::CurrentTarget => Some(Unit::Target(current)),
            UnitRef::PreviousTarget => Some(Unit::Target((current + targets - 1) % targets)),
            UnitRef::NextTarget => Some(Unit::Target((current + 1) % targets)),
            UnitRef::Target(index) => (index < targets).then_some(Unit::Target(index)),
            UnitRef::Nobody => None,
        }
    }
}

/// The dot a dot value reads: Go `NewDotReference` of a spell on a unit. A dot value without
/// a spell has no value in Go.
#[derive(Clone, Debug, PartialEq)]
pub struct DotRef {
    pub spell: Option<ActionId>,
    pub target: UnitRef,
}

/// What an aura value reads of an aura on a unit other than the player, or with
/// `includeReactionTime`, of any unit's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuraRead {
    IsKnown,
    IsActive,
    NumStacks,
    RemainingTime,
    /// `auraIsActive` that holds once the aura has been up for the player's reaction time.
    IsActiveAfterReaction,
    /// `auraIsInactive` that holds once the aura has been down for the player's reaction time.
    IsInactiveAfterReaction,
    /// `auraNumStacks` as it stood a reaction time ago.
    NumStacksAfterReaction,
}

/// A dot as a compiled rotation reads it: the spell holding it, by its position in the
/// player's spellbook, and the target whose copy of it counts, by position among the targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DotAt {
    pub spell: usize,
    pub target: usize,
}

/// Go `APLValueMath_MathOperator`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathOp {
    Add,
    Sub,
    Mul,
    Div,
}

impl MathOp {
    /// Go `APLValueMath.Type`, from the operand types after `newValueMath` coerced them.
    pub fn result_type(self, lhs: ValueType, rhs: ValueType) -> ValueType {
        match self {
            MathOp::Add | MathOp::Sub => lhs,
            MathOp::Mul if lhs == ValueType::Duration || rhs == ValueType::Duration => {
                ValueType::Duration
            }
            MathOp::Div if lhs == ValueType::Duration && rhs == ValueType::Duration => {
                ValueType::Float
            }
            _ if lhs == ValueType::Float || rhs == ValueType::Float => ValueType::Float,
            _ => lhs,
        }
    }

    /// The getter types Go's `APLValueMath` reads its operands with, when it is read with
    /// the getter of its own type.
    fn operand_getters(self, lhs: ValueType, rhs: ValueType) -> (ValueType, ValueType) {
        match (self, self.result_type(lhs, rhs)) {
            (MathOp::Add | MathOp::Sub, result) => (result, result),
            // A duration product or quotient reads each operand with its own getter.
            (MathOp::Mul | MathOp::Div, ValueType::Duration) => (lhs, rhs),
            (MathOp::Div, ValueType::Float)
                if lhs == ValueType::Duration && rhs == ValueType::Duration =>
            {
                (lhs, rhs)
            }
            (_, result) => (result, result),
        }
    }
}

/// Go `highestOrderTypeList`: the highest type of the values, placeholders left out until a
/// reference fills them.
fn highest_type(values: &[Value]) -> ValueType {
    values
        .iter()
        .filter(|value| !matches!(value, Value::Placeholder(_)))
        .map(Value::value_type)
        .max()
        .unwrap_or(ValueType::Int)
}

/// Operand types after `newValueMath`, which coerces addition and subtraction operands to
/// the higher of their two types.
fn math_operand_types(op: MathOp, lhs: ValueType, rhs: ValueType) -> (ValueType, ValueType) {
    match op {
        MathOp::Add | MathOp::Sub => (lhs.max(rhs), lhs.max(rhs)),
        MathOp::Mul | MathOp::Div => (lhs, rhs),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Const(Const),
    Compare {
        op: CompareOp,
        lhs: Box<Value>,
        rhs: Box<Value>,
    },
    And(Vec<Value>),
    Or(Vec<Value>),
    Not(Box<Value>),
    Math {
        op: MathOp,
        lhs: Box<Value>,
        rhs: Box<Value>,
    },
    /// Go sim/shaman/apl_values.go `APLValueTotemRemainingTime`; no totem type gives no value.
    TotemRemainingTime {
        totem: Option<Totem>,
        include_reaction_time: bool,
    },
    CurrentManaPercent,
    /// Go `APLValueCurrentHealthPercent` of the player.
    CurrentHealthPercent,
    /// Exact last ACKed 2D current-target health percent, available only in
    /// the selected native/world bridge. No source-independent mob HP model.
    WorldCurrentTargetHealthPercent,
    RemainingTimePercent,
    CurrentMana,
    /// `currentMana` with one of the player's pets, by its position among them, as its source
    /// unit.
    PetCurrentMana(usize),
    /// `currentManaPercent` with one of the player's pets as its source unit.
    PetCurrentManaPercent(usize),
    CurrentEnergy,
    MaxEnergy,
    CurrentComboPoints,
    TimeToNextEnergyTick,
    /// Go `APLValueCurrentRage`.
    CurrentRage,
    /// Go `APLValueIsExecutePhase`, by its percent threshold.
    IsExecutePhase(i32),
    RemainingTime,
    CurrentTime,
    NumberTargets,
    /// Go `APLValueActiveItemSwapSet`: whether the set it names (Swap1 when true) is worn.
    ActiveItemSwapSet(bool),
    AuraIsKnown(ActionId),
    /// `auraIsKnown` with one of the player's pets, by its position among them, as its
    /// source unit.
    PetAuraIsKnown {
        pet: usize,
        id: ActionId,
    },
    /// `auraIsActive` with one of the player's pets, by its position among them, as its source
    /// unit: the pet's aura while it is simulated, else an aura that is never up.
    PetAuraIsActive {
        pet: usize,
        id: ActionId,
    },
    /// `auraRemainingTime` with one of the player's pets as its source unit.
    PetAuraRemainingTime {
        pet: usize,
        id: ActionId,
    },
    AuraIsActive(ActionId),
    AuraNumStacks(ActionId),
    AuraRemainingTime(ActionId),
    /// An aura value with a unit other than the player as its source unit, or with
    /// `includeReactionTime`, which Go's values of an aura on any unit read.
    TargetAura {
        read: AuraRead,
        unit: UnitRef,
        id: ActionId,
    },
    DotIsActive(DotRef),
    DotRemainingTime(DotRef),
    SpellIsKnown(ActionId),
    SpellIsReady(ActionId),
    SpellCastTime(ActionId),
    SpellTimeToReady(ActionId),
    DotTimeToNextTick(DotRef),
    /// Go `APLValueDotBaseDuration`: the dot's base duration on the first target, read when the
    /// rotation is built.
    DotBaseDuration(Option<ActionId>),
    GcdIsReady,
    /// Go `APLValueAuraShouldRefresh`: the aura, the unit it is on (the current target by
    /// default) and the overlap a refresh allows.
    AuraShouldRefresh {
        id: ActionId,
        unit: UnitRef,
        max_overlap: Box<Value>,
    },
    /// Go `APLValueFrontOfTarget`.
    FrontOfTarget,
    /// Go `APLValueVariableRef`: a named variable and the value it resolved to as the rotation
    /// was built, which a group's variable of the same name replaces in the group's conditions.
    Variable {
        name: String,
        resolved: Box<Value>,
    },
    /// Go `APLValueVariablePlaceholder`: a value the reference to a group fills in. The type it
    /// reads as is that of the value that fills it.
    Placeholder(String),
    /// Go `APLValueActionGroupUsed`: whether a reference binds a group of the name, which
    /// binding the groups settles.
    ActionGroupUsed {
        name: String,
        used: bool,
    },
    /// Go `APLValueMaxMana`.
    MaxMana,
    /// Go `APLValueSpellCanCast`: `CanCastOrQueue` on the current target.
    SpellCanCast(ActionId),
    /// Go `APLValueAutoTimeToNext`.
    AutoTimeToNext(AutoAttackType),
    /// Go `APLValueAutoSwingTime`: a hand's current swing duration.
    AutoSwingTime(SwingType),
    /// Go `APLValueSpellCurrentCost`.
    SpellCurrentCost(ActionId),
    /// Go `APLValueSpellTravelTime`: `Spell.TravelTime`.
    SpellTravelTime(ActionId),
    /// Go `APLValueSpellCPM`: the casts this iteration per minute of the fight so far.
    SpellCpm(ActionId),
    /// Go `APLValueSpellIsChanneling`: the player's channel is the spell's.
    SpellIsChanneling(ActionId),
    /// Go `APLValueSpellChanneledTicks`: the ticks of whatever the player channels.
    SpellChanneledTicks(ActionId),
    /// Go `APLValueSpellNumCharges`. No spell of the fork has charges.
    SpellNumCharges(ActionId),
    /// Go `APLValueSpellTimeToCharge`. No spell of the fork recharges.
    SpellTimeToCharge(ActionId),
    /// Go `APLValueSpellGCDHastedDuration`.
    SpellGcdHastedDuration(ActionId),
    /// Go `APLValueSpellFullCooldown`: `Spell.CD.Duration`, with its flat modifiers.
    SpellFullCooldown(ActionId),
    /// Go `APLValueSpellInFlight`: a travel of the spell is still pending.
    SpellInFlight(ActionId),
    /// Go `APLValueMax`: the largest of the values, all coerced to the highest of their types.
    Max(Vec<Value>),
    /// Go `APLValueMin`: the smallest of them.
    Min(Vec<Value>),
    /// Go `APLValueCurrentTimePercent`: the time so far over the fight's duration.
    CurrentTimePercent,
    /// Go `APLValueCurrentHealth` of the player.
    CurrentHealth,
    /// Go `APLValueMaxHealth`.
    MaxHealth,
    /// Go `APLValueMaxRage`: no value without a rage bar.
    MaxRage,
    /// Go `APLValueMaxComboPoints`: no value without an energy bar.
    MaxComboPoints,
    /// Go `APLValueCurrentManaRegen`: the mana tick while casting; no value without mana.
    CurrentManaRegen,
    /// Go `APLValueEnergyTimeToTarget`: when the energy ticks reach the value, read as a float.
    EnergyTimeToTarget(Box<Value>),
    /// Go `APLValueUnitIsMoving` of the unit; only the player moves.
    UnitIsMoving(UnitRef),
    /// Go `APLValueUnitDistance`: the player's distance from the target, whatever unit it names.
    UnitDistance,
    /// Go `APLValueGCDTimeToReady`.
    GcdTimeToReady,
    /// Go `APLValueAutoTimeSinceLast`: the time since the auto attack last swung.
    AutoTimeSinceLast(AutoAttackType),
    /// Go `APLValueInputDelay`: the player's reaction time.
    InputDelay,
    /// Go `APLValueChannelClipDelay`.
    ChannelClipDelay,
    /// Go `APLValueAuraInternalCooldown`: the time until the ICD of the first aura of the unit
    /// with the action, or triggered by it, that has one, is ready.
    AuraInternalCooldown {
        unit: UnitRef,
        id: ActionId,
    },
    /// Go `APLValueAuraICDIsReady`; with `includeReactionTime` also while the aura has been
    /// up for less than the reaction time.
    AuraIcdIsReady {
        unit: UnitRef,
        id: ActionId,
        include_reaction_time: bool,
    },
    /// Go `APLValueAuraDuration`: the aura's duration.
    AuraDuration {
        unit: UnitRef,
        id: ActionId,
    },
    /// Go `APLValueDotIsActiveOnAllTargets` of the spell's dot.
    DotIsActiveOnAllTargets(ActionId),
    /// Go `APLValueDotLowestRemainingTime`: zero once a target lacks the dot.
    DotLowestRemainingTime(ActionId),
    /// Go `APLValueDotTickFrequency`: the dot's tick period, or the one it would tick at now
    /// while it is down.
    DotTickFrequency(DotRef),
    /// Go `APLValueDotPercentIncrease`: how much more a tick cast now would deal than the
    /// dot's snapshot, through the spell's `ExpectedTickDamage`.
    DotPercentIncrease(DotRef),
    /// Go `APLValueDotTickRatePercentIncrease`: how much longer the running tick period is
    /// than one cast now.
    DotTickRatePercentIncrease(DotRef),
    /// Go `APLValueBossCurrentTarget`: whether the unit's current target is the player, which
    /// a target's is while the player tanks it.
    BossCurrentTarget(UnitRef),
    /// Go `APLValueAllItemStatProcsActive`: every proc buff that can proc is up at full stacks.
    AllItemStatProcsActive(StatProcs),
    /// Go `APLValueAnyItemStatProcsActive`: one is up at full stacks.
    AnyItemStatProcsActive(StatProcs),
    /// Go `APLValueAnyItemStatProcsAvailable`: one that can proc is down with its ICD ready.
    AnyItemStatProcsAvailable(StatProcs),
    /// Go `APLValueItemProcsMinRemainingTime`: the least time left of those up.
    ItemProcsMinRemainingTime(StatProcs),
    /// Go `APLValueItemProcsMaxRemainingICD`: the longest ICD left of those that can proc and are down.
    ItemProcsMaxRemainingIcd(StatProcs),
    /// Go `APLValueNumEquippedStatProcItems`: how many can proc.
    NumEquippedStatProcItems(StatProcs),
    /// Go `APLValueNumStatBuffCooldowns`: how many of the matching cooldowns are worn.
    NumStatBuffCooldowns(Vec<i32>),
    /// Go `APLValueAnyStatBuffCooldownsActive`: a matching cooldown's buff is up at full stacks.
    AnyStatBuffCooldownsActive(Vec<i32>),
    /// Go `APLValueAnyStatBuffCooldownsMinDuration`: the least time left of those up at full stacks.
    AnyStatBuffCooldownsMinDuration(Vec<i32>),
    /// Go `APLValueSelectedPotion`: whether the consumables select the potion.
    SelectedPotion(i32),
    /// Go `APLValueSelectedConjured`: whether the consumables select the conjured item.
    SelectedConjured(i32),
    /// Go `APLValueBossSpellIsCasting`: the unit hardcasts its spell of the action.
    BossSpellIsCasting {
        spell: ActionId,
        unit: UnitRef,
    },
    /// Go `APLValueBossSpellTimeToReady`: the time until the unit's spell is ready.
    BossSpellTimeToReady {
        spell: ActionId,
        unit: UnitRef,
    },
    /// Go `APLValueMultipleCdUsages`: whether casting the target spell now keeps the uses its
    /// cooldown allows, lined up with the base spell's buff when the base spell has one.
    MultipleCdUsages {
        base: Option<ActionId>,
        target: ActionId,
        offset: Option<Box<Value>>,
        align_cd_end: bool,
    },
    /// Go sim/warlock/apl_values.go `APLValueWarlockIsAssignedCurse`: whether the curse type
    /// (Agony when absent) is the Warlock's assigned curse.
    WarlockIsAssignedCurse(String),
    /// Go `APLValueWarlockAssignedCurseIsActive`: the assigned curse's aura on the unit has
    /// been up for the reaction time.
    WarlockAssignedCurseIsActive(UnitRef),
    /// Go `APLValueSequenceIsComplete` of the sequence of the name.
    SequenceIsComplete(String),
    /// Go `APLValueSequenceIsReady`: the sequence's next step is ready.
    SequenceIsReady(String),
    /// Go `APLValueSequenceTimeToReady`: the time until its next step's spell is ready.
    SequenceTimeToReady(String),
    /// A value the pinned Go engine builds nothing for, so it drops out of its parent as a
    /// value without one does: Go's switch has no case for it, or its constructor is in a file
    /// the build leaves out.
    Unbuilt(&'static str),
}

/// What an item stat proc value matches: Go `IntTupleToStatsList` of its stat types, as Go's
/// stat indices, and the ICD its buffs must exceed (`minIcdSeconds`), none at zero or less.
#[derive(Clone, Debug, PartialEq)]
pub struct StatProcs {
    pub stats: Vec<i32>,
    pub min_icd: i64,
}

/// Go `APLValueAutoSwingTime_SwingType`: which swing a value reads. Unknown reads as none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwingType {
    Unknown,
    MainHand,
    OffHand,
    Ranged,
}

/// Go `APLValueAutoAttackType`: which auto attack a value reads. Unknown reads as any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoAttackType {
    Any,
    Melee,
    MainHand,
    OffHand,
    Ranged,
}

impl Value {
    /// Visit this value and every nested value, parents first.
    pub fn visit(&self, f: &mut impl FnMut(&Value)) {
        f(self);
        match self {
            Value::Compare { lhs, rhs, .. } | Value::Math { lhs, rhs, .. } => {
                lhs.visit(f);
                rhs.visit(f);
            }
            Value::And(values) | Value::Or(values) => {
                values.iter().for_each(|value| value.visit(f))
            }
            Value::Not(value) => value.visit(f),
            Value::AuraShouldRefresh { max_overlap, .. } => max_overlap.visit(f),
            Value::Max(values) | Value::Min(values) => {
                values.iter().for_each(|value| value.visit(f))
            }
            Value::EnergyTimeToTarget(value) => value.visit(f),
            Value::MultipleCdUsages {
                offset: Some(offset),
                ..
            } => offset.visit(f),
            Value::Variable { resolved, .. } => resolved.visit(f),
            _ => {}
        }
    }

    /// This value with `numberTargets` read as the fight's target count, a constant that
    /// folds.
    pub fn with_targets(&self, count: usize) -> Value {
        let map = |value: &Value| Box::new(value.with_targets(count));
        match self {
            Value::NumberTargets => {
                Value::Const(parse_const(&count.to_string()).expect("int constant"))
            }
            Value::Compare { op, lhs, rhs } => Value::Compare {
                op: *op,
                lhs: map(lhs),
                rhs: map(rhs),
            },
            Value::Math { op, lhs, rhs } => Value::Math {
                op: *op,
                lhs: map(lhs),
                rhs: map(rhs),
            },
            Value::And(values) => Value::And(
                values
                    .iter()
                    .map(|value| value.with_targets(count))
                    .collect(),
            ),
            Value::Or(values) => Value::Or(
                values
                    .iter()
                    .map(|value| value.with_targets(count))
                    .collect(),
            ),
            Value::Not(value) => Value::Not(map(value)),
            Value::Max(values) => Value::Max(
                values
                    .iter()
                    .map(|value| value.with_targets(count))
                    .collect(),
            ),
            Value::Min(values) => Value::Min(
                values
                    .iter()
                    .map(|value| value.with_targets(count))
                    .collect(),
            ),
            Value::EnergyTimeToTarget(value) => Value::EnergyTimeToTarget(map(value)),
            Value::Variable { name, resolved } => Value::Variable {
                name: name.clone(),
                resolved: map(resolved),
            },
            other => other.clone(),
        }
    }

    /// The Go type before coercion.
    pub fn value_type(&self) -> ValueType {
        match self {
            Value::Const(constant) => constant.value_type,
            Value::Compare { .. }
            | Value::And(_)
            | Value::Or(_)
            | Value::Not(_)
            | Value::AuraIsKnown(_)
            | Value::PetAuraIsKnown { .. }
            | Value::PetAuraIsActive { .. }
            | Value::AuraIsActive(_)
            | Value::AuraShouldRefresh { .. }
            | Value::ActionGroupUsed { .. }
            | Value::FrontOfTarget
            | Value::DotIsActive(_)
            | Value::SpellIsKnown(_)
            | Value::SpellIsReady(_)
            | Value::IsExecutePhase(_)
            | Value::SpellCanCast(_)
            | Value::ActiveItemSwapSet(_)
            | Value::SpellIsChanneling(_)
            | Value::SpellInFlight(_)
            | Value::GcdIsReady => ValueType::Bool,
            Value::TargetAura {
                read:
                    AuraRead::IsKnown
                    | AuraRead::IsActive
                    | AuraRead::IsActiveAfterReaction
                    | AuraRead::IsInactiveAfterReaction,
                ..
            } => ValueType::Bool,
            Value::TargetAura {
                read: AuraRead::NumStacks | AuraRead::NumStacksAfterReaction,
                ..
            } => ValueType::Int,
            Value::TargetAura {
                read: AuraRead::RemainingTime,
                ..
            } => ValueType::Duration,
            Value::AuraNumStacks(_)
            | Value::NumberTargets
            | Value::SpellChanneledTicks(_)
            | Value::SpellNumCharges(_)
            | Value::CurrentComboPoints => ValueType::Int,
            Value::AuraRemainingTime(_)
            | Value::PetAuraRemainingTime { .. }
            | Value::DotRemainingTime(_)
            | Value::SpellCastTime(_)
            | Value::SpellTimeToReady(_)
            | Value::DotTimeToNextTick(_)
            | Value::AutoTimeToNext(_)
            | Value::AutoSwingTime(_)
            | Value::DotBaseDuration(_)
            | Value::RemainingTime
            | Value::TotemRemainingTime { .. }
            | Value::CurrentTime
            | Value::SpellTravelTime(_)
            | Value::SpellTimeToCharge(_)
            | Value::SpellGcdHastedDuration(_)
            | Value::SpellFullCooldown(_)
            | Value::TimeToNextEnergyTick => ValueType::Duration,
            Value::CurrentManaPercent
            | Value::PetCurrentManaPercent(_)
            | Value::CurrentHealthPercent
            | Value::WorldCurrentTargetHealthPercent
            | Value::CurrentMana
            | Value::PetCurrentMana(_)
            | Value::RemainingTimePercent
            | Value::CurrentEnergy
            | Value::CurrentRage
            | Value::MaxEnergy
            | Value::SpellCurrentCost(_)
            | Value::SpellCpm(_)
            | Value::MaxMana => ValueType::Float,
            Value::Math { op, lhs, rhs } => {
                let (lhs, rhs) = math_operand_types(*op, lhs.value_type(), rhs.value_type());
                op.result_type(lhs, rhs)
            }
            Value::Variable { resolved, .. } => resolved.value_type(),
            Value::Max(values) | Value::Min(values) => highest_type(values),
            Value::UnitIsMoving(_)
            | Value::AuraIcdIsReady { .. }
            | Value::SequenceIsComplete(_)
            | Value::SequenceIsReady(_)
            | Value::WarlockIsAssignedCurse(_)
            | Value::WarlockAssignedCurseIsActive(_)
            | Value::BossCurrentTarget(_)
            | Value::SelectedPotion(_)
            | Value::SelectedConjured(_)
            | Value::AllItemStatProcsActive(_)
            | Value::AnyItemStatProcsActive(_)
            | Value::AnyItemStatProcsAvailable(_)
            | Value::AnyStatBuffCooldownsActive(_)
            | Value::BossSpellIsCasting { .. }
            | Value::MultipleCdUsages { .. }
            | Value::DotIsActiveOnAllTargets(_) => ValueType::Bool,
            Value::SequenceTimeToReady(_) | Value::BossSpellTimeToReady { .. } => {
                ValueType::Duration
            }
            Value::ItemProcsMinRemainingTime(_) => ValueType::Duration,
            Value::ItemProcsMaxRemainingIcd(_) => ValueType::Duration,
            Value::AnyStatBuffCooldownsMinDuration(_) => ValueType::Duration,
            Value::NumEquippedStatProcItems(_) => ValueType::Int,
            Value::NumStatBuffCooldowns(_) => ValueType::Int,
            Value::AuraInternalCooldown { .. }
            | Value::AuraDuration { .. }
            | Value::DotLowestRemainingTime(_)
            | Value::DotTickFrequency(_) => ValueType::Duration,
            Value::DotPercentIncrease(_) | Value::DotTickRatePercentIncrease(_) => ValueType::Float,
            Value::MaxComboPoints => ValueType::Int,
            Value::CurrentTimePercent
            | Value::CurrentHealth
            | Value::MaxHealth
            | Value::MaxRage
            | Value::CurrentManaRegen
            | Value::UnitDistance => ValueType::Float,
            Value::EnergyTimeToTarget(_)
            | Value::GcdTimeToReady
            | Value::AutoTimeSinceLast(_)
            | Value::InputDelay
            | Value::ChannelClipDelay => ValueType::Duration,
            // Go never reads a value it did not build.
            Value::Unbuilt(_) => ValueType::Int,
            // Unknown until a group's reference fills it; the checks that read the type of an
            // operand run again once it has.
            Value::Placeholder(_) => ValueType::Int,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Go `APLActionCastSpell` on the unit its target names, the current target by default.
    CastSpell {
        spell: ActionId,
        target: UnitRef,
    },
    /// Go `APLActionCastFriendlySpell` at the player, the one player in scope.
    CastAtPlayer(ActionId),
    AutocastOtherCooldowns,
    /// Go `APLActionStrictSequence`: casts that run in order once the first is ready.
    StrictSequence(Vec<Step>),
    /// Go `APLActionSequence`: casts that run one step at a time, each when it is ready, and
    /// never again once done, until a reset sequence action of its name starts it over.
    Sequence {
        name: String,
        steps: Vec<Step>,
    },
    /// Go `APLActionWait`: the rotation waits for the duration value; none when Go builds no
    /// value.
    Wait(Option<Value>),
    /// Go `APLActionWaitUntil`: the rotation waits until the condition holds; none when Go
    /// builds no value.
    WaitUntil(Option<Value>),
    /// Go `APLActionSchedule`: the inner action, ready at each of the times in turn. No times
    /// when one fails to parse, which builds no action; `invalid_timings` are those, as Go's
    /// validation message quotes them.
    Schedule {
        timings: Option<Vec<i64>>,
        invalid_timings: Vec<String>,
        inner: Box<Item>,
    },
    /// Go `APLActionResetSequence`: starts the sequence of the name over.
    ResetSequence(String),
    /// Go `APLActionStrictMultidot`: the spell on each of up to `max_dots` targets whose dot
    /// is down or runs out within `max_overlap`, or before the previous one's, in one strict
    /// sequence.
    StrictMultidot {
        spell: ActionId,
        max_dots: i32,
        max_overlap: Option<Value>,
    },
    /// Go `APLActionCancelAura` of one of the player's auras.
    CancelAura(ActionId),
    /// Go `APLActionChangeTarget` to the unit, the player when it names none.
    ChangeTarget(UnitRef),
    /// Go sim/warlock/apl_values.go `APLActionCastWarlockAssignedCurse` on the unit.
    CastAssignedCurse(UnitRef),
    /// Go `APLActionCastAllStatBuffCooldowns`: the instant cooldowns whose buff moves one of
    /// the stats, which leave the major cooldowns.
    CastAllStatBuffCooldowns(Vec<i32>),
    /// Go `APLActionActivateAllStatBuffProcAuras` of the stats and the set (Swap1 when true),
    /// parsed only as a prepull action.
    ActivateAllStatBuffProcAuras {
        stats: Vec<i32>,
        swap1: bool,
    },
    /// Go `APLActionActivateAuraWithStacks` of one of the player's auras, parsed only as a
    /// prepull action.
    ActivateAuraWithStacks {
        id: ActionId,
        stacks: i32,
    },
    /// Go `APLActionTriggerICD` of the player's aura with the ICD, parsed only as a prepull
    /// action.
    TriggerIcd(ActionId),
    /// Go `APLActionChannelSpell`: a channel the rotation may interrupt.
    ChannelSpell {
        spell: ActionId,
        /// The unit the channel is cast on, the current target by default. Go builds no action
        /// for a target that names no unit.
        target: UnitRef,
        interrupt_if: Option<Value>,
        allow_recast: bool,
    },
    /// Go `APLActionActivateAura` on one of the player's auras, parsed only as a prepull action.
    ActivateAura(ActionId),
    /// Go `APLActionMove`: the range from the target to move to, which Go reads as the float
    /// value of the value it names.
    Move(Value),
    /// Go `APLActionMoveDuration`: a move that lasts as long as the duration value it names.
    MoveDuration(Value),
    /// Go `APLActionGroupReference`: the actions of a group, the first that is ready runs. The
    /// variables fill the placeholders of the group's conditions. An empty name builds no action.
    GroupReference {
        name: String,
        variables: Vec<(String, Value)>,
    },
    /// Go `APLActionGroupReference` before the pull, of a group only it references whose
    /// actions are moves without conditions: its Execute runs the first that is ready, and a
    /// move before the pull is ready while the player neither moves nor casts. Parsed only as
    /// a prepull action, with the range Go reads.
    PrepullGroupMove(Value),
    /// Go `APLActionItemSwap`: swap to the item swap's set (`true`) or back to the request's
    /// equipment, ready only while the other set is worn.
    ItemSwap(bool),
    /// Go `APLActionMultidot`: the spell on the first of up to `max_dots` targets whose dot
    /// is down or runs out within `max_overlap`.
    Multidot {
        spell: ActionId,
        max_dots: i32,
        max_overlap: Option<Value>,
    },
}

/// A step of a sequence: a cast of the spell on the unit Go casts it on, or, in a strict
/// sequence, Go `APLActionChangeTarget` to the unit. Go builds no action for a unit it finds
/// nothing for.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// The spell a cast step casts; none for a change of target.
    pub spell: ActionId,
    pub target: UnitRef,
    /// A sequence step's own condition, which Go checks as it asks whether the step is ready.
    pub condition: Option<Value>,
    /// The step changes the player's current target to `target` and casts nothing.
    pub change_target: bool,
}

impl Action {
    /// The values an action reads in its own fields, which a group's variables do not reach.
    pub fn values(&self) -> Vec<&Value> {
        match self {
            Action::ChannelSpell { interrupt_if, .. } => interrupt_if.iter().collect(),
            Action::Multidot { max_overlap, .. } | Action::StrictMultidot { max_overlap, .. } => {
                max_overlap.iter().collect()
            }
            Action::Move(value) | Action::MoveDuration(value) | Action::PrepullGroupMove(value) => {
                vec![value]
            }
            Action::Wait(value) | Action::WaitUntil(value) => value.iter().collect(),
            Action::StrictSequence(steps) | Action::Sequence { steps, .. } => steps
                .iter()
                .filter_map(|step| step.condition.as_ref())
                .collect(),
            // Go builds the inner action without the group's variables.
            Action::Schedule { inner, .. } => {
                let mut values: Vec<&Value> = inner.condition.iter().collect();
                values.extend(inner.action.values());
                values
            }
            Action::GroupReference { variables, .. } => {
                variables.iter().map(|(_, value)| value).collect()
            }
            _ => Vec::new(),
        }
    }

    /// The spells of the action that Go builds, in a fight of `targets` targets: a step whose
    /// target names no unit builds no action, and takes the whole of a strict sequence with it.
    pub fn reachable_spells(&self, targets: usize) -> Vec<&ActionId> {
        match self {
            Action::Sequence { steps, .. } => steps
                .iter()
                .filter(|step| !step.change_target && step.target.resolve(targets).is_some())
                .map(|step| &step.spell)
                .collect(),
            Action::Schedule { inner, .. } => inner.action.reachable_spells(targets),
            Action::StrictSequence(steps)
                if steps
                    .iter()
                    .any(|step| step.target.resolve(targets).is_none()) =>
            {
                Vec::new()
            }
            other => other.spells(),
        }
    }

    /// The action a schedule runs, through any schedules around it; any other action itself.
    pub fn scheduled(&self) -> &Action {
        match self {
            Action::Schedule { inner, .. } => inner.action.scheduled(),
            other => other,
        }
    }

    /// The spells the action names, in order, as Go `GetAllActions` visits casts.
    pub fn spells(&self) -> Vec<&ActionId> {
        match self {
            Action::CastSpell { spell: id, .. } | Action::CastAtPlayer(id) => vec![id],
            Action::AutocastOtherCooldowns => Vec::new(),
            Action::StrictSequence(steps) | Action::Sequence { steps, .. } => steps
                .iter()
                .filter(|step| !step.change_target)
                .map(|step| &step.spell)
                .collect(),
            Action::Schedule { inner, .. } => inner.action.spells(),
            Action::ChannelSpell { spell, .. }
            | Action::Multidot { spell, .. }
            | Action::StrictMultidot { spell, .. } => vec![spell],
            Action::ActivateAura(_)
            | Action::Move(_)
            | Action::MoveDuration(_)
            | Action::PrepullGroupMove(_)
            | Action::ItemSwap(_)
            | Action::Wait(_)
            | Action::WaitUntil(_)
            | Action::ResetSequence(_)
            | Action::CancelAura(_)
            | Action::ChangeTarget(_)
            | Action::CastAssignedCurse(_)
            | Action::CastAllStatBuffCooldowns(_)
            | Action::ActivateAllStatBuffProcAuras { .. }
            | Action::ActivateAuraWithStacks { .. }
            | Action::TriggerIcd(_)
            | Action::GroupReference { .. } => Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// One-based position in the request's priority list, hidden items included.
    pub position: usize,
    pub condition: Option<Value>,
    pub action: Action,
}

/// Go `APLPrepullAction`: an action at a fixed time before the pull.
#[derive(Clone, Debug, PartialEq)]
pub struct Prepull {
    /// One-based position in the request's prepull list, hidden actions included.
    pub position: usize,
    /// Nanoseconds relative to the pull, never positive.
    pub do_at_ns: i64,
    pub action: Action,
    /// Go compiles a prepull action's condition only to prune it: a constant false drops the
    /// action, and anything else never stops it from running.
    pub condition: Option<Value>,
}

/// Go `APLGroup`: a named list of actions that a reference runs, with variables that stand for
/// the rotation's own of the same name inside its conditions.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub name: String,
    pub variables: Vec<(String, Value)>,
    /// The group's actions. Their positions continue the priority list's.
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rotation {
    pub priority_list: Vec<Item>,
    pub prepull: Vec<Prepull>,
    pub groups: Vec<Group>,
}

thread_local! {
    /// Go `APLRotation.valueVariables` while the rotation is parsed: each variable's name and
    /// its value as written.
    static VARIABLES: std::cell::RefCell<Vec<(String, Json)>> = const { std::cell::RefCell::new(Vec::new()) };
    /// The variables being resolved, which a variable that names itself would never finish.
    static RESOLVING: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Set while compilation reads a name as Go reads it without `GetAPLAura` or
    /// `GetAPLSpell`, which give no validation message of a missing aura or spell.
    static UNREPORTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` as a lookup Go makes without a validation message.
fn unreported<T>(f: impl FnOnce() -> T) -> T {
    let outer = UNREPORTED.with(|flag| flag.replace(true));
    let out = f();
    UNREPORTED.with(|flag| flag.set(outer));
    out
}

/// Whether the lookups now are ones Go makes without a validation message.
pub(crate) fn lookups_unreported() -> bool {
    UNREPORTED.with(std::cell::Cell::get)
}

/// Go `valueVariables`: the list of a rotation or a group, each a name and a value.
fn parse_variables(list: Option<&Json>) -> Result<Vec<(String, Json)>, Vec<String>> {
    let Some(list) = list else {
        return Ok(Vec::new());
    };
    let items = list
        .as_array()
        .ok_or_else(|| vec!["variables must be a list".to_string()])?;
    let mut variables = Vec::new();
    for item in items {
        let object = item
            .as_object()
            .ok_or_else(|| vec!["a variable must be an object".to_string()])?;
        if let Some(key) = object.keys().find(|key| *key != "name" && *key != "value") {
            return Err(vec![format!("variable field {key} is unsupported")]);
        }
        let name = object
            .get("name")
            .and_then(Json::as_str)
            .unwrap_or_default();
        let value = object.get("value").cloned().unwrap_or(Json::Null);
        variables.push((name.to_string(), value));
    }
    Ok(variables)
}

/// A variable's value parsed as `newAPLValue` builds it, which can give none.
fn parse_variable_value(value: &Json) -> Result<Value, Vec<String>> {
    if value.is_null() {
        return Err(vec!["a variable without a value is unsupported".to_string()]);
    }
    parse_value(value)
}

/// Parse a protojson `APLRotation`. Returns every unsupported construct, not only the first.
pub fn parse(rotation: &Json) -> Result<Rotation, Vec<String>> {
    let mut reasons = Vec::new();
    let mut parsed = Rotation::default();
    let Some(object) = rotation.as_object() else {
        return Err(vec!["rotation must be an object".into()]);
    };
    for (key, value) in object {
        match key.as_str() {
            // Go reads the lists whatever the type; the UI sends `TypeAuto` with a preset's list.
            "type" if value == "TypeAPL" || value == "TypeAuto" => {}
            "type" => reasons.push(format!("rotation type {value} is unsupported")),
            "priorityList" | "prepullActions" | "groups" | "valueVariables" => {}
            other => reasons.push(format!("rotation field {other} is unsupported")),
        }
    }
    if object.get("type").is_none() {
        reasons.push("rotation type is missing".into());
    }
    // Go parses the value variables first; a reference builds the value it names when it is
    // read.
    match parse_variables(object.get("valueVariables")) {
        Ok(variables) => VARIABLES.with(|scope| *scope.borrow_mut() = variables),
        Err(variable_reasons) => reasons.extend(variable_reasons),
    }
    RESOLVING.with(|stack| stack.borrow_mut().clear());
    for (index, item) in object
        .get("priorityList")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        match parse_item(item, index + 1) {
            Ok(Some(item)) => parsed.priority_list.push(item),
            Ok(None) => {}
            Err(mut item_reasons) => {
                for reason in &mut item_reasons {
                    *reason = format!("rotation item {}: {reason}", index + 1);
                }
                reasons.extend(item_reasons);
            }
        }
    }
    for (index, item) in object
        .get("prepullActions")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        match parse_prepull(item, index + 1) {
            Ok(Some(prepull)) => parsed.prepull.push(prepull),
            Ok(None) => {}
            Err(reason) => reasons.push(format!("prepull action {}: {reason}", index + 1)),
        }
    }
    let mut next_position = object
        .get("priorityList")
        .and_then(Json::as_array)
        .map_or(0, Vec::len)
        + 1;
    for (index, group) in object
        .get("groups")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        match parse_group(group, &mut next_position) {
            Ok(group) => parsed.groups.push(group),
            Err(group_reasons) => reasons.extend(
                group_reasons
                    .into_iter()
                    .map(|reason| format!("group {}: {reason}", index + 1)),
            ),
        }
    }
    VARIABLES.with(|scope| scope.borrow_mut().clear());
    reasons.extend(group_sequence_reads(&parsed));
    reasons.extend(prepull_group_moves(&mut parsed));
    if reasons.is_empty() {
        Ok(parsed)
    } else {
        Err(reasons)
    }
}

/// Go `APLActionGroupReference` before the pull: Go finalizes the prepull actions first, so the
/// reference binds the first instance of its group no priority list reference holds. Rust runs
/// a group only the reference names whose actions are moves without conditions, read with the
/// rotation's variables: the first move is the one Execute finds ready, or none is.
fn prepull_group_moves(rotation: &mut Rotation) -> Vec<String> {
    fn references(item: &Item, name: &str) -> bool {
        match &item.action {
            Action::GroupReference { name: other, .. } => other == name,
            Action::Schedule { inner, .. } => references(inner, name),
            _ => false,
        }
    }
    let mut reasons = Vec::new();
    let names: Vec<String> = rotation
        .prepull
        .iter()
        .filter_map(|prepull| match &prepull.action {
            Action::GroupReference { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    for prepull in &mut rotation.prepull {
        let Action::GroupReference { name, variables } = &prepull.action else {
            continue;
        };
        let position = prepull.position;
        let mut refuse = |reason: &str| {
            reasons.push(format!("prepull action {position}: {reason}"));
        };
        let groups: Vec<&Group> = rotation
            .groups
            .iter()
            .filter(|group| &group.name == name)
            .collect();
        let shared = names.iter().filter(|other| *other == name).count() > 1
            || rotation
                .priority_list
                .iter()
                .chain(rotation.groups.iter().flat_map(|group| group.items.iter()))
                .any(|item| references(item, name));
        let [group] = groups.as_slice() else {
            refuse("a prepull reference to a group not named once is unsupported");
            continue;
        };
        if shared || !variables.is_empty() || !group.variables.is_empty() {
            refuse(
                "a prepull reference to a group with variables or other references is unsupported",
            );
            continue;
        }
        // A condition that is a constant reads its boolean: an action whose condition is false
        // is never ready.
        let mut ranges = Vec::new();
        for item in &group.items {
            let ready = match item.condition.as_ref().map(constant) {
                None => true,
                Some(Some(condition)) => condition.boolean,
                Some(None) => {
                    ranges.clear();
                    break;
                }
            };
            match &item.action {
                Action::Move(range) if constant(range).is_some() => {
                    if ready {
                        ranges.push(range.clone());
                    }
                }
                _ => {
                    ranges.clear();
                    break;
                }
            }
        }
        let Some(range) = ranges.into_iter().next() else {
            refuse("a prepull reference to a group of actions other than moves to a constant is unsupported");
            continue;
        };
        prepull.action = Action::PrepullGroupMove(range);
    }
    reasons
}

/// The constant a value is, directly or as the value of a variable.
fn constant(value: &Value) -> Option<&Const> {
    match value {
        Value::Const(constant) => Some(constant),
        Value::Variable { resolved, .. } => constant(resolved),
        _ => None,
    }
}

/// Go's reset sequence actions and sequence values find a sequence by name in the priority list's
/// actions, then in the groups' configurations, which no reference runs. Rust finds those of the
/// priority list; a name a group's sequence holds is refused.
fn group_sequence_reads(rotation: &Rotation) -> Vec<String> {
    fn sequence_names(item: &Item, out: &mut Vec<String>) {
        match &item.action {
            Action::Sequence { name, .. } => out.push(name.clone()),
            Action::Schedule { inner, .. } => sequence_names(inner, out),
            _ => {}
        }
    }
    let mut in_groups = Vec::new();
    for item in rotation.groups.iter().flat_map(|group| group.items.iter()) {
        sequence_names(item, &mut in_groups);
    }
    let mut read = Vec::new();
    let mut note = |value: &Value| match value {
        Value::SequenceIsComplete(name)
        | Value::SequenceIsReady(name)
        | Value::SequenceTimeToReady(name) => read.push(name.clone()),
        _ => {}
    };
    fn walk(item: &Item, note: &mut impl FnMut(&Value), read: &mut Vec<String>) {
        if let Some(condition) = &item.condition {
            condition.visit(note);
        }
        for value in item.action.values() {
            value.visit(note);
        }
        match &item.action {
            Action::ResetSequence(name) => read.push(name.clone()),
            Action::Schedule { inner, .. } => walk(inner, note, read),
            _ => {}
        }
    }
    let mut names = Vec::new();
    for item in rotation
        .priority_list
        .iter()
        .chain(rotation.groups.iter().flat_map(|group| group.items.iter()))
    {
        walk(item, &mut note, &mut names);
    }
    names.extend(read);
    let mut reasons = Vec::new();
    for name in names {
        if !name.is_empty() && in_groups.contains(&name) {
            let reason = format!("a sequence {name:?} of a group read by name is unsupported");
            if !reasons.contains(&reason) {
                reasons.push(reason);
            }
        }
    }
    reasons
}

/// Go `newAPLRotation`'s group parsing: the group's variables and its actions, which are numbered
/// from `next_position`, continuing the priority list.
fn parse_group(group: &Json, next_position: &mut usize) -> Result<Group, Vec<String>> {
    let object = group
        .as_object()
        .ok_or_else(|| vec!["a group must be an object".to_string()])?;
    if let Some(key) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "name" | "actions" | "variables"))
    {
        return Err(vec![format!("group field {key} is unsupported")]);
    }
    let name = object
        .get("name")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string();
    let mut reasons = Vec::new();
    let mut variables = Vec::new();
    match parse_variables(object.get("variables")) {
        Ok(list) => {
            for (variable, value) in list {
                match parse_variable_value(&value) {
                    Ok(value) => variables.push((variable, value)),
                    Err(variable_reasons) => reasons.extend(variable_reasons),
                }
            }
        }
        Err(variable_reasons) => reasons.extend(variable_reasons),
    }
    let mut items = Vec::new();
    for item in object
        .get("actions")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
    {
        let position = *next_position;
        *next_position += 1;
        match parse_item(item, position) {
            Ok(Some(item)) => items.push(item),
            Ok(None) => {}
            Err(item_reasons) => reasons.extend(
                item_reasons
                    .into_iter()
                    .map(|reason| format!("item {position}: {reason}")),
            ),
        }
    }
    if reasons.is_empty() {
        Ok(Group {
            name,
            variables,
            items,
        })
    } else {
        Err(reasons)
    }
}

/// Go `newAPLRotation`'s prepull parsing: a hidden action or one after the pull is skipped.
fn parse_prepull(item: &Json, position: usize) -> Result<Option<Prepull>, String> {
    let object = item.as_object().ok_or("prepull action must be an object")?;
    for key in object.keys() {
        if !matches!(key.as_str(), "action" | "doAtValue" | "hide") {
            return Err(format!("field {key} is unsupported"));
        }
    }
    if object.get("hide").and_then(Json::as_bool) == Some(true) {
        return Ok(None);
    }
    let do_at = match object.get("doAtValue").map(parse_value) {
        Some(Ok(Value::Const(constant))) => constant,
        Some(Ok(_)) => return Err("a do-at time other than a constant is unsupported".into()),
        Some(Err(reasons)) => return Err(reasons.join("; ")),
        None => return Err("no do-at time".into()),
    };
    // Go `GetDuration` on the constant, as its type converts it.
    let do_at_ns = match do_at.value_type {
        ValueType::Duration | ValueType::Int | ValueType::Float => do_at.duration_ns,
        _ => return Err("a do-at time that is not a duration is unsupported".into()),
    };
    if do_at_ns > 0 {
        return Ok(None);
    }
    let action = object
        .get("action")
        .and_then(Json::as_object)
        .ok_or("no action")?;
    // Go registers no prepull action for a cast that names no spell.
    if names_no_spell(action) {
        return Ok(None);
    }
    let condition = match action.get("condition") {
        Some(condition) => Some(parse_value(condition).map_err(|reasons| reasons.join("; "))?),
        None => None,
    };
    let action = match single(action, &["uuid", "condition"])? {
        ("castSpell", config) => {
            let step = parse_step(config)?;
            Action::CastSpell {
                spell: step.spell,
                target: step.target,
            }
        }
        ("activateAura", config) => {
            let object = config.as_object().ok_or("activateAura must be an object")?;
            if let Some(key) = object.keys().find(|key| *key != "auraId") {
                return Err(format!("activateAura field {key} is unsupported"));
            }
            Action::ActivateAura(parse_action_id(
                object.get("auraId").ok_or("activateAura has no auraId")?,
            )?)
        }
        ("move", config) => parse_move(config)?,
        ("moveDuration", config) => parse_move_duration(config)?,
        ("castFriendlySpell", config) => parse_cast_friendly_spell(config)?,
        ("changeTarget", config) => parse_change_target(config)?,
        ("castAllStatBuffCooldowns", config) => parse_cast_all_stat_buff_cooldowns(config)?,
        ("activateAllStatBuffProcAuras", config) => {
            let object = config
                .as_object()
                .ok_or("activateAllStatBuffProcAuras must be an object")?;
            if let Some(key) = object.keys().find(|key| {
                !matches!(
                    key.as_str(),
                    "statType1" | "statType2" | "statType3" | "swapSet"
                )
            }) {
                return Err(format!(
                    "activateAllStatBuffProcAuras field {key} is unsupported"
                ));
            }
            let swap1 = match object.get("swapSet").and_then(Json::as_str) {
                None | Some("Main") => false,
                Some("Swap1") => true,
                Some(other) => {
                    return Err(format!(
                        "activateAllStatBuffProcAuras swap set {other} is unsupported"
                    ))
                }
            };
            Action::ActivateAllStatBuffProcAuras {
                stats: parse_stat_types(config)?,
                swap1,
            }
        }
        ("channelSpell", config) => parse_channel_spell(config)?,
        ("strictSequence", config) => parse_strict_sequence(config)?,
        ("itemSwap", config) => Action::ItemSwap(parse_swap_set("itemSwap", config)?),
        ("cancelAura", config) => Action::CancelAura(parse_aura_action("cancelAura", config)?),
        ("triggerIcd", config) => Action::TriggerIcd(parse_aura_action("triggerIcd", config)?),
        // Resolved into the group's move once the groups are parsed.
        ("groupReference", config) => parse_group_reference(config)?,
        ("activateAuraWithStacks", config) => {
            let object = config
                .as_object()
                .ok_or("activateAuraWithStacks must be an object")?;
            if let Some(key) = object
                .keys()
                .find(|key| !matches!(key.as_str(), "auraId" | "numStacks"))
            {
                return Err(format!("activateAuraWithStacks field {key} is unsupported"));
            }
            Action::ActivateAuraWithStacks {
                id: parse_action_id(
                    object
                        .get("auraId")
                        .ok_or("activateAuraWithStacks has no auraId")?,
                )?,
                stacks: object
                    .get("numStacks")
                    .map(json_i32)
                    .transpose()?
                    .unwrap_or(0),
            }
        }
        (name, _) => return Err(format!("action {name} is unsupported")),
    };
    // A prepull action runs at a fixed time before the pull, where only a constant is read.
    if let Action::Move(value) | Action::MoveDuration(value) = &action {
        if !matches!(value, Value::Const(_)) {
            return Err("a prepull move other than to a constant is unsupported".into());
        }
    }
    Ok(Some(Prepull {
        position,
        do_at_ns,
        action,
        condition,
    }))
}

/// Go `newActionMove`: the range from the target to move to, which Go reads as the float value
/// of the value it names. A value without a float getter panics where Go reads it, and so does
/// a missing one, which Rust cannot reproduce.
fn parse_move(config: &Json) -> Result<Action, String> {
    let object = config.as_object().ok_or("move must be an object")?;
    if let Some(key) = object.keys().find(|key| *key != "rangeFromTarget") {
        return Err(format!("move field {key} is unsupported"));
    }
    let range = object.get("rangeFromTarget").ok_or("move has no range")?;
    let value = parse_value(range).map_err(|reasons| reasons.join("; "))?;
    if !reads_as(&value, ValueType::Float) {
        return Err("a move range Go cannot read as a float is unsupported".into());
    }
    Ok(Action::Move(value))
}

/// Go `newActionMoveDuration`: the duration of the move, read with the duration getter.
fn parse_move_duration(config: &Json) -> Result<Action, String> {
    let object = config.as_object().ok_or("moveDuration must be an object")?;
    if let Some(key) = object.keys().find(|key| *key != "duration") {
        return Err(format!("moveDuration field {key} is unsupported"));
    }
    let duration = object
        .get("duration")
        .ok_or("moveDuration has no duration")?;
    let value = parse_value(duration).map_err(|reasons| reasons.join("; "))?;
    if !reads_as(&value, ValueType::Duration) {
        return Err("a move duration Go cannot read as a duration is unsupported".into());
    }
    Ok(Action::MoveDuration(value))
}

/// Whether Go's value answers the getter of a type: a constant answers every getter, and any
/// other value only its own type's.
fn reads_as(value: &Value, getter: ValueType) -> bool {
    matches!(value, Value::Const(_)) || value.value_type() == getter
}

fn is_empty(value: &Json) -> bool {
    match value {
        Json::Array(items) => items.is_empty(),
        Json::Object(fields) => fields.is_empty(),
        Json::Null => true,
        _ => false,
    }
}

fn single<'a>(
    object: &'a Map<String, Json>,
    ignored: &[&str],
) -> Result<(&'a str, &'a Json), String> {
    let mut fields = object
        .iter()
        .filter(|(key, _)| !ignored.contains(&key.as_str()));
    match (fields.next(), fields.next()) {
        (Some((key, value)), None) => Ok((key.as_str(), value)),
        _ => Err("expected exactly one operator".into()),
    }
}

fn parse_item(item: &Json, position: usize) -> Result<Option<Item>, Vec<String>> {
    let object = item
        .as_object()
        .ok_or_else(|| vec!["item must be an object".to_string()])?;
    for key in object.keys() {
        // The notes are comments for the reader.
        if key != "action" && key != "hide" && key != "notes" {
            return Err(vec![format!("item field {key} is unsupported")]);
        }
    }
    if object.get("hide").and_then(Json::as_bool) == Some(true) {
        return Ok(None);
    }
    let action = object
        .get("action")
        .and_then(Json::as_object)
        .ok_or_else(|| vec!["item has no action".to_string()])?;
    if names_no_spell(action) {
        return Ok(None);
    }
    parse_action_object(action, position).map(Some)
}

/// Whether an action is a cast that names no spell. Go `GetAPLSpell` returns no spell for a
/// missing `spellId`, so `newActionCastSpell`, `newActionCastFriendlySpell` and
/// `newActionChannelSpell` build no action and the rotation skips the line, condition and all,
/// as it skips a hidden one (Discipline Priest sim e4f6ebb0, 25 October 2026: a
/// castFriendlySpell line saved before its spell was picked).
fn names_no_spell(action: &Map<String, Json>) -> bool {
    match single(action, &["uuid", "condition"]) {
        Ok(("castSpell" | "castFriendlySpell" | "channelSpell", config)) => config
            .as_object()
            .is_some_and(|config| config.get("spellId").is_none_or(Json::is_null)),
        _ => false,
    }
}

/// A protojson `APLAction`: its condition and what it does.
fn parse_action_object(action: &Map<String, Json>, position: usize) -> Result<Item, Vec<String>> {
    let mut reasons = Vec::new();
    let condition = match action.get("condition") {
        Some(condition) => match parse_value(condition) {
            Ok(value) => Some(value),
            Err(mut value_reasons) => {
                reasons.append(&mut value_reasons);
                None
            }
        },
        None => None,
    };
    let parsed = match single(action, &["condition", "uuid"]) {
        Ok(("castSpell", config)) => parse_cast_spell_action(config),
        Ok(("castFriendlySpell", config)) => parse_cast_friendly_spell(config),
        Ok(("autocastOtherCooldowns", config)) if is_empty(config) => {
            Ok(Action::AutocastOtherCooldowns)
        }
        Ok(("strictSequence", config)) => parse_strict_sequence(config),
        Ok(("sequence", config)) => parse_sequence(config),
        Ok(("channelSpell", config)) => parse_channel_spell(config),
        Ok(("multidot", config)) => parse_multidot(config),
        Ok(("strictMultidot", config)) => match parse_multidot(config) {
            Ok(Action::Multidot {
                spell,
                max_dots,
                max_overlap,
            }) => Ok(Action::StrictMultidot {
                spell,
                max_dots,
                max_overlap,
            }),
            other => other,
        },
        Ok(("cancelAura", config)) => {
            parse_aura_action("cancelAura", config).map(Action::CancelAura)
        }
        Ok(("castWarlockAssignedCurse", config)) => parse_assigned_curse(config),
        Ok(("changeTarget", config)) => parse_change_target(config),
        Ok(("castAllStatBuffCooldowns", config)) => parse_cast_all_stat_buff_cooldowns(config),
        Ok(("move", config)) => parse_move(config),
        Ok(("moveDuration", config)) => parse_move_duration(config),
        Ok(("groupReference", config)) => parse_group_reference(config),
        Ok(("itemSwap", config)) => parse_swap_set("itemSwap", config).map(Action::ItemSwap),
        Ok(("wait", config)) => parse_value_field("wait", config, "duration").map(Action::Wait),
        Ok(("waitUntil", config)) => {
            parse_value_field("waitUntil", config, "condition").map(Action::WaitUntil)
        }
        Ok(("schedule", config)) => parse_schedule(config, position),
        Ok(("resetSequence", config)) => parse_sequence_name("resetSequence", config)
            .map(|name| Action::ResetSequence(name.unwrap_or_default())),
        Ok((name, _)) => Err(format!("action {name} is unsupported")),
        Err(err) => Err(err),
    };
    match parsed {
        Ok(action) if reasons.is_empty() => Ok(Item {
            position,
            condition,
            action,
        }),
        Ok(_) => Err(reasons),
        Err(reason) => {
            reasons.push(reason);
            Err(reasons)
        }
    }
}

/// An action's one value field, `None` when it is absent, which Go builds no value for.
fn parse_value_field(name: &str, config: &Json, field: &str) -> Result<Option<Value>, String> {
    let object = config
        .as_object()
        .ok_or_else(|| format!("{name} must be an object"))?;
    if let Some(key) = object.keys().find(|key| key.as_str() != field) {
        return Err(format!("{name} field {key} is unsupported"));
    }
    object
        .get(field)
        .map(|value| parse_value(value).map_err(|reasons| reasons.join("; ")))
        .transpose()
}

/// Go `newActionSchedule`: the inner action, then the comma separated times, each Go
/// `time.ParseDuration` of its trimmed text. One that fails to parse builds no action.
fn parse_schedule(config: &Json, position: usize) -> Result<Action, String> {
    let object = config.as_object().ok_or("schedule must be an object")?;
    if let Some(key) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "schedule" | "innerAction"))
    {
        return Err(format!("schedule field {key} is unsupported"));
    }
    let inner = object
        .get("innerAction")
        .and_then(Json::as_object)
        .ok_or("a schedule without an inner action is unsupported")?;
    let inner = parse_action_object(inner, position).map_err(|reasons| reasons.join("; "))?;
    if matches!(inner.action, Action::GroupReference { .. }) {
        return Err("a schedule of a group reference is unsupported".into());
    }
    let text = object
        .get("schedule")
        .and_then(Json::as_str)
        .unwrap_or_default();
    let timings = text
        .split(',')
        .map(|timing| parse_go_duration(timing.trim()))
        .collect::<Option<Vec<i64>>>();
    let invalid_timings = text
        .split(',')
        .map(str::trim)
        .filter(|timing| parse_go_duration(timing).is_none())
        .map(str::to_string)
        .collect();
    Ok(Action::Schedule {
        timings,
        invalid_timings,
        inner: Box::new(inner),
    })
}

/// The sequence a reset sequence action or a sequence value names, `None` when it names none.
fn parse_sequence_name(name: &str, config: &Json) -> Result<Option<String>, String> {
    let object = config
        .as_object()
        .ok_or_else(|| format!("{name} must be an object"))?;
    if let Some(key) = object.keys().find(|key| key.as_str() != "sequenceName") {
        return Err(format!("{name} field {key} is unsupported"));
    }
    Ok(object
        .get("sequenceName")
        .and_then(Json::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_string))
}

/// The swap set an item swap action or value names: `true` for Swap1, `false` for Main. Go builds
/// nothing for an unknown set.
fn parse_swap_set(name: &str, config: &Json) -> Result<bool, String> {
    let object = config
        .as_object()
        .ok_or_else(|| format!("{name} must be an object"))?;
    if let Some(key) = object.keys().find(|key| key.as_str() != "swapSet") {
        return Err(format!("{name} field {key} is unsupported"));
    }
    match object.get("swapSet").and_then(Json::as_str) {
        Some("Main") => Ok(false),
        Some("Swap1") => Ok(true),
        other => Err(format!("{name} swap set {other:?} is unsupported")),
    }
}

/// Go `newActionGroupReference`: the group's name and the variables that fill its placeholders.
fn parse_group_reference(config: &Json) -> Result<Action, String> {
    let object = config
        .as_object()
        .ok_or("groupReference must be an object")?;
    if let Some(key) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "groupName" | "variables"))
    {
        return Err(format!("groupReference field {key} is unsupported"));
    }
    let name = object
        .get("groupName")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string();
    let mut variables = Vec::new();
    for (variable, value) in parse_variables(object.get("variables")).map_err(|r| r.join("; "))? {
        let value = parse_variable_value(&value).map_err(|reasons| reasons.join("; "))?;
        variables.push((variable, value));
    }
    Ok(Action::GroupReference { name, variables })
}

/// Go `newActionStrictSequence` for sequences of unconditional casts.
fn parse_strict_sequence(config: &Json) -> Result<Action, String> {
    let object = config
        .as_object()
        .ok_or("strictSequence must be an object")?;
    for key in object.keys() {
        if key != "actions" {
            return Err(format!("strictSequence field {key} is unsupported"));
        }
    }
    let mut spells = Vec::new();
    for action in object
        .get("actions")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
    {
        let action = action
            .as_object()
            .ok_or("strictSequence action must be an object")?;
        match single(action, &["uuid", "condition"])? {
            ("castSpell", cast) => {
                let mut step = parse_step(cast)?;
                // Go builds each step as an action of its own, condition included.
                if let Some(condition) = action.get("condition") {
                    step.condition =
                        Some(parse_value(condition).map_err(|reasons| reasons.join("; "))?);
                }
                spells.push(step)
            }
            // Go builds any action as a step; a change of target is the one Rust runs.
            ("changeTarget", config) => {
                let Action::ChangeTarget(target) = parse_change_target(config)? else {
                    unreachable!("a change of target parses as one");
                };
                let condition = match action.get("condition") {
                    Some(condition) => {
                        Some(parse_value(condition).map_err(|reasons| reasons.join("; "))?)
                    }
                    None => None,
                };
                spells.push(Step {
                    spell: ActionId::default(),
                    target,
                    condition,
                    change_target: true,
                })
            }
            (name, _) => return Err(format!("strictSequence action {name} is unsupported")),
        }
    }
    Ok(Action::StrictSequence(spells))
}

/// Go `newActionSequence` for sequences of casts, each with its own condition, with the name
/// reset sequence actions and sequence values find it by.
fn parse_sequence(config: &Json) -> Result<Action, String> {
    let object = config.as_object().ok_or("sequence must be an object")?;
    for key in object.keys() {
        if key != "actions" && key != "name" {
            return Err(format!("sequence field {key} is unsupported"));
        }
    }
    let mut spells = Vec::new();
    for action in object
        .get("actions")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
    {
        let action = action
            .as_object()
            .ok_or("sequence action must be an object")?;
        match single(action, &["uuid", "condition"])? {
            ("castSpell", cast) => {
                let mut step = parse_step(cast)?;
                // Go builds each step as an action of its own, condition included.
                if let Some(condition) = action.get("condition") {
                    step.condition =
                        Some(parse_value(condition).map_err(|reasons| reasons.join("; "))?);
                }
                spells.push(step)
            }
            (name, _) => return Err(format!("sequence action {name} is unsupported")),
        }
    }
    let name = object
        .get("name")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(Action::Sequence {
        name,
        steps: spells,
    })
}

/// Go `newActionChannelSpell`: the channel, the unit it is cast on and its interrupt condition.
fn parse_channel_spell(config: &Json) -> Result<Action, String> {
    let object = config.as_object().ok_or("channelSpell must be an object")?;
    let mut interrupt_if = None;
    let mut allow_recast = false;
    let target = parse_unit_ref(object.get("target"), UnitRef::CurrentTarget)
        .map_err(|()| unsupported_unit("channelSpell", "target", object.get("target")))?;
    for (key, value) in object {
        match key.as_str() {
            "spellId" | "target" => {}
            "interruptIf" => {
                interrupt_if = Some(parse_value(value).map_err(|reasons| reasons.join("; "))?)
            }
            "allowRecast" => {
                allow_recast = value.as_bool().ok_or("allowRecast must be a boolean")?
            }
            other => return Err(format!("channelSpell field {other} is unsupported")),
        }
    }
    let spell = parse_action_id(object.get("spellId").ok_or("channelSpell has no spellId")?)?;
    Ok(Action::ChannelSpell {
        spell,
        target,
        interrupt_if,
        allow_recast,
    })
}

/// Go `newActionCastFriendlySpell`: no target means the current target, as for `castSpell`;
/// the first player of the raid, or the unit itself, is the player.
fn parse_cast_friendly_spell(config: &Json) -> Result<Action, String> {
    let object = config
        .as_object()
        .ok_or("castFriendlySpell must be an object")?;
    for key in object.keys() {
        if key != "spellId" && key != "target" {
            return Err(format!("castFriendlySpell field {key} is unsupported"));
        }
    }
    let spell = parse_action_id(
        object
            .get("spellId")
            .ok_or("castFriendlySpell has no spellId")?,
    )?;
    let target = parse_unit_ref(object.get("target"), UnitRef::CurrentTarget)
        .map_err(|()| unsupported_unit("castFriendlySpell", "target", object.get("target")))?;
    Ok(match target {
        UnitRef::Player => Action::CastAtPlayer(spell),
        target => Action::CastSpell { spell, target },
    })
}

/// Go `newActionChangeTarget`: the unit `GetSourceUnit` finds, the player itself without one.
fn parse_change_target(config: &Json) -> Result<Action, String> {
    let object = config.as_object().ok_or("changeTarget must be an object")?;
    if let Some(key) = object.keys().find(|key| key.as_str() != "newTarget") {
        return Err(format!("changeTarget field {key} is unsupported"));
    }
    let unit = parse_unit_ref(object.get("newTarget"), UnitRef::Player)
        .map_err(|()| unsupported_unit("changeTarget", "newTarget", object.get("newTarget")))?;
    Ok(Action::ChangeTarget(unit))
}

/// Go `newActionCastAllStatBuffCooldowns`: the stats it matches.
fn parse_cast_all_stat_buff_cooldowns(config: &Json) -> Result<Action, String> {
    let object = config
        .as_object()
        .ok_or("castAllStatBuffCooldowns must be an object")?;
    if let Some(key) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "statType1" | "statType2" | "statType3"))
    {
        return Err(format!(
            "castAllStatBuffCooldowns field {key} is unsupported"
        ));
    }
    Ok(Action::CastAllStatBuffCooldowns(parse_stat_types(config)?))
}

/// Go `newActionWarlockAssignedCurseAction`: the unit the curse is cast on, which the runtime
/// casts on the first target or on no unit.
fn parse_assigned_curse(config: &Json) -> Result<Action, String> {
    let object = config
        .as_object()
        .ok_or("castWarlockAssignedCurse must be an object")?;
    if let Some(key) = object.keys().find(|key| key.as_str() != "target") {
        return Err(format!(
            "castWarlockAssignedCurse field {key} is unsupported"
        ));
    }
    match parse_unit_ref(object.get("target"), UnitRef::CurrentTarget) {
        Ok(unit @ (UnitRef::CurrentTarget | UnitRef::Target(0) | UnitRef::Nobody)) => {
            Ok(Action::CastAssignedCurse(unit))
        }
        _ => Err(unsupported_unit(
            "castWarlockAssignedCurse",
            "target",
            object.get("target"),
        )),
    }
}

/// An action on one of the player's auras: its aura. Go builds no action without one, which Rust
/// refuses rather than reproduce.
fn parse_aura_action(name: &str, config: &Json) -> Result<ActionId, String> {
    let object = config
        .as_object()
        .ok_or_else(|| format!("{name} must be an object"))?;
    if let Some(key) = object.keys().find(|key| key.as_str() != "auraId") {
        return Err(format!("{name} field {key} is unsupported"));
    }
    parse_action_id(
        object
            .get("auraId")
            .ok_or_else(|| format!("{name} has no auraId"))?,
    )
}

/// Go `newActionMultidot`: its spell, dot count and overlap.
fn parse_multidot(config: &Json) -> Result<Action, String> {
    let object = config.as_object().ok_or("multidot must be an object")?;
    let mut spell = None;
    let mut max_dots = 0;
    let mut max_overlap = None;
    for (key, field) in object {
        match key.as_str() {
            "spellId" => spell = Some(parse_action_id(field)?),
            "maxDots" => max_dots = json_i32(field)?,
            "maxOverlap" => {
                max_overlap = Some(parse_value(field).map_err(|reasons| reasons.join("; "))?)
            }
            other => return Err(format!("multidot field {other} is unsupported")),
        }
    }
    Ok(Action::Multidot {
        spell: spell.ok_or("multidot has no spellId")?,
        max_dots,
        max_overlap,
    })
}

/// Go `newActionCastSpell` in a priority list: the spell and the unit it is cast on.
fn parse_cast_spell_action(config: &Json) -> Result<Action, String> {
    let step = parse_step(config)?;
    Ok(Action::CastSpell {
        spell: step.spell,
        target: step.target,
    })
}

/// A `castSpell` configuration's spell and the unit it is cast on, the current target by default.
fn parse_step(config: &Json) -> Result<Step, String> {
    let object = config.as_object().ok_or("castSpell must be an object")?;
    let target = parse_unit_ref(object.get("target"), UnitRef::CurrentTarget)
        .map_err(|()| unsupported_unit("castSpell", "target", object.get("target")))?;
    for key in object.keys() {
        if key != "spellId" && key != "target" {
            return Err(format!("castSpell field {key} is unsupported"));
        }
    }
    let spell = parse_action_id(object.get("spellId").ok_or("castSpell has no spellId")?)?;
    Ok(Step {
        spell,
        target,
        condition: None,
        change_target: false,
    })
}

/// Go `GetTargetUnit` and `GetSourceUnit`: the unit a `UnitReference` names, or `default`
/// when it is absent or its type is unknown. Go's `Environment.GetUnit` reads only the fields
/// its type uses: `Self`, `CurrentTarget`, `PreviousTarget` and `NextTarget` ignore an index
/// and an owner; a player is the raid's index, a target the encounter's; the sets of all
/// players and all targets, and a player past the one in the raid, name no unit, which a cast
/// drops and a dot or aura read treats as one nobody has. `Err` for a reference Rust does not
/// resolve: a pet, a negative target index (Go reads past its list) and fields no reference has.
fn parse_unit_ref(reference: Option<&Json>, default: UnitRef) -> Result<UnitRef, ()> {
    let Some(reference) = reference else {
        return Ok(default);
    };
    let object = reference.as_object().ok_or(())?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "type" | "index" | "owner"))
    {
        return Err(());
    }
    let kind = match object.get("type") {
        None => "Unknown",
        Some(kind) => kind.as_str().ok_or(())?,
    };
    let index = match object.get("index") {
        None => 0,
        Some(index) => i32::try_from(index.as_i64().ok_or(())?).map_err(|_| ())?,
    };
    match kind {
        "Unknown" => Ok(default),
        "Self" => Ok(UnitRef::Player),
        // The raid holds one player, at index 0.
        "Player" if index == 0 => Ok(UnitRef::Player),
        "Player" | "AllPlayers" | "AllTargets" => Ok(UnitRef::Nobody),
        "CurrentTarget" => Ok(UnitRef::CurrentTarget),
        "PreviousTarget" => Ok(UnitRef::PreviousTarget),
        "NextTarget" => Ok(UnitRef::NextTarget),
        "Target" => Ok(UnitRef::Target(usize::try_from(index).map_err(|_| ())?)),
        _ => Err(()),
    }
}

/// [`parse_unit_ref`] for a value that reads a dot or an aura on a unit.
fn parse_value_unit_ref(reference: Option<&Json>, default: UnitRef) -> Result<UnitRef, ()> {
    parse_unit_ref(reference, default)
}

/// Go `GetSourceUnit` on a pet of the player: its position among the player's pets, the first
/// when the reference names none. `None` for any other reference.
fn pet_position(source: &Json) -> Option<usize> {
    let source = source.as_object()?;
    let owner_is_self = source
        .get("owner")
        .and_then(Json::as_object)
        .is_some_and(|owner| {
            owner.len() == 1 && owner.get("type").and_then(Json::as_str) == Some("Self")
        });
    let known_keys = source
        .keys()
        .all(|key| ["type", "index", "owner"].contains(&key.as_str()));
    if source.get("type").and_then(Json::as_str) != Some("Pet") || !owner_is_self || !known_keys {
        return None;
    }
    match source.get("index") {
        None => Some(0),
        Some(index) => index.as_u64().and_then(|index| usize::try_from(index).ok()),
    }
}

/// The reason a unit reference field is refused.
fn unsupported_unit(name: &str, field: &str, reference: Option<&Json>) -> String {
    format!(
        "{name} {field} {} is unsupported",
        reference.cloned().unwrap_or_default()
    )
}

/// Parse a protojson `ActionID`.
pub fn parse_action_id(value: &Json) -> Result<ActionId, String> {
    let object = value.as_object().ok_or("action id must be an object")?;
    let mut id = ActionId::default();
    for (key, field) in object {
        match key.as_str() {
            "spellId" => id.spell_id = json_i32(field)?,
            "itemId" => id.item_id = json_i32(field)?,
            "tag" => id.tag = json_i32(field)?,
            // Go `ProtoToActionID` ignores the rank.
            "rank" => {
                json_i32(field)?;
            }
            "otherId" => {
                id.other_id = field
                    .as_str()
                    .ok_or("otherId must be a string")?
                    .to_string()
            }
            other => return Err(format!("action id field {other} is unsupported")),
        }
    }
    let kinds = [id.spell_id != 0, id.item_id != 0, !id.other_id.is_empty()];
    if kinds.iter().filter(|set| **set).count() != 1 {
        return Err("action id must name exactly one spell, item or other action".into());
    }
    Ok(id)
}

fn json_i32(value: &Json) -> Result<i32, String> {
    value
        .as_i64()
        .and_then(|number| i32::try_from(number).ok())
        .ok_or_else(|| format!("invalid integer {value}"))
}

fn parse_value(value: &Json) -> Result<Value, Vec<String>> {
    let object = value
        .as_object()
        .ok_or_else(|| vec!["value must be an object".to_string()])?;
    let (name, config) = single(object, &["uuid"]).map_err(|err| vec![err])?;
    let fields = config.as_object();
    let only = |allowed: &[&str]| -> Result<(), Vec<String>> {
        match fields {
            Some(fields) => match fields.keys().find(|key| !allowed.contains(&key.as_str())) {
                Some(key) => Err(vec![format!("{name} field {key} is unsupported")]),
                None => Ok(()),
            },
            None => Err(vec![format!("{name} must be an object")]),
        }
    };
    match name {
        "const" => {
            only(&["val"])?;
            let text = config.get("val").and_then(Json::as_str).unwrap_or_default();
            parse_const(text).map(Value::Const).map_err(|err| vec![err])
        }
        "cmp" => {
            only(&["op", "lhs", "rhs"])?;
            let op = match config.get("op").and_then(Json::as_str) {
                Some("OpEq") => CompareOp::Eq,
                Some("OpNe") => CompareOp::Ne,
                Some("OpLt") => CompareOp::Lt,
                Some("OpLe") => CompareOp::Le,
                Some("OpGt") => CompareOp::Gt,
                Some("OpGe") => CompareOp::Ge,
                other => {
                    return Err(vec![format!(
                        "comparison operator {other:?} is unsupported"
                    )])
                }
            };
            let lhs = config
                .get("lhs")
                .ok_or_else(|| vec!["cmp has no lhs".to_string()]);
            let rhs = config
                .get("rhs")
                .ok_or_else(|| vec!["cmp has no rhs".to_string()]);
            let (lhs, rhs) = (lhs.and_then(parse_value), rhs.and_then(parse_value));
            match (lhs, rhs) {
                (Ok(lhs), Ok(rhs)) => Ok(Value::Compare {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                }),
                (lhs, rhs) => Err(lhs.err().into_iter().chain(rhs.err()).flatten().collect()),
            }
        }
        "math" => {
            only(&["op", "lhs", "rhs"])?;
            let op = match config.get("op").and_then(Json::as_str) {
                Some("OpAdd") => MathOp::Add,
                Some("OpSub") => MathOp::Sub,
                Some("OpMul") => MathOp::Mul,
                Some("OpDiv") => MathOp::Div,
                other => return Err(vec![format!("math operator {other:?} is unsupported")]),
            };
            let lhs = config
                .get("lhs")
                .ok_or_else(|| vec!["math has no lhs".to_string()]);
            let rhs = config
                .get("rhs")
                .ok_or_else(|| vec!["math has no rhs".to_string()]);
            let (lhs, rhs) = match (lhs.and_then(parse_value), rhs.and_then(parse_value)) {
                (Ok(lhs), Ok(rhs)) => (lhs, rhs),
                (lhs, rhs) => {
                    return Err(lhs.err().into_iter().chain(rhs.err()).flatten().collect())
                }
            };
            // Go panics when it reads an operand with a getter its type lacks. Constants and
            // the coerced operands of a sum or difference answer every getter.
            let (lhs_type, rhs_type) = math_operand_types(op, lhs.value_type(), rhs.value_type());
            let (lhs_getter, rhs_getter) = op.operand_getters(lhs_type, rhs_type);
            let answers = |value: &Value, getter: ValueType| {
                matches!(value, Value::Const(_))
                    || matches!(op, MathOp::Add | MathOp::Sub)
                    || value.value_type() == getter
            };
            if !answers(&lhs, lhs_getter) || !answers(&rhs, rhs_getter) {
                return Err(vec![
                    "math that reads an operand as another type is unsupported".into(),
                ]);
            }
            Ok(Value::Math {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            })
        }
        "max" | "min" => {
            only(&["vals"])?;
            let mut values = Vec::new();
            let mut reasons = Vec::new();
            for value in config
                .get("vals")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
            {
                match parse_value(value) {
                    Ok(value) => values.push(value),
                    Err(mut value_reasons) => reasons.append(&mut value_reasons),
                }
            }
            if !reasons.is_empty() {
                return Err(reasons);
            }
            // Go has no boolean or string getter for them and panics where it reads one. One
            // value stands for the operator (Go `newValueMin` returns it), so it is read as itself.
            if values.len() > 1
                && matches!(highest_type(&values), ValueType::Bool | ValueType::String)
            {
                return Err(vec![format!(
                    "{name} of booleans or strings is unsupported"
                )]);
            }
            Ok(if name == "max" {
                Value::Max(values)
            } else {
                Value::Min(values)
            })
        }
        "currentTimePercent" => {
            only(&[])?;
            Ok(Value::CurrentTimePercent)
        }
        "maxHealth" => {
            only(&[])?;
            Ok(Value::MaxHealth)
        }
        "maxRage" => {
            only(&[])?;
            Ok(Value::MaxRage)
        }
        "maxComboPoints" => {
            only(&[])?;
            Ok(Value::MaxComboPoints)
        }
        "currentManaRegen" => {
            only(&[])?;
            Ok(Value::CurrentManaRegen)
        }
        "gcdTimeToReady" => {
            only(&[])?;
            Ok(Value::GcdTimeToReady)
        }
        "inputDelay" => {
            only(&[])?;
            Ok(Value::InputDelay)
        }
        "channelClipDelay" => {
            only(&[])?;
            Ok(Value::ChannelClipDelay)
        }
        // Go reads the rotation's own unit whatever unit the value names.
        "unitDistance" => {
            only(&["sourceUnit"])?;
            Ok(Value::UnitDistance)
        }
        "unitIsMoving" | "currentHealth" => {
            only(&["sourceUnit"])?;
            // Go `GetSourceUnit`: no unit reference means the player itself.
            let unit =
                parse_value_unit_ref(config.get("sourceUnit"), UnitRef::Player).map_err(|()| {
                    vec![unsupported_unit(
                        name,
                        "sourceUnit",
                        config.get("sourceUnit"),
                    )]
                })?;
            match (name, unit) {
                ("unitIsMoving", unit) => Ok(Value::UnitIsMoving(unit)),
                (_, UnitRef::Player) => Ok(Value::CurrentHealth),
                // A unit Go finds nothing for gives no value.
                (_, UnitRef::Nobody) => Ok(Value::Unbuilt("currentHealth")),
                _ => Err(vec!["currentHealth of a target is unsupported".to_string()]),
            }
        }
        "energyTimeToTarget" => {
            only(&["targetEnergy"])?;
            match config.get("targetEnergy") {
                Some(value) => Ok(Value::EnergyTimeToTarget(Box::new(parse_value(value)?))),
                // Go builds no target energy, so no value.
                None => Ok(Value::Unbuilt("energyTimeToTarget")),
            }
        }
        "autoTimeSinceLast" => {
            only(&["autoType"])?;
            Ok(Value::AutoTimeSinceLast(parse_auto_attack_type(
                name, config,
            )?))
        }
        // Go's switch has no case for these, or the Balance druid's constructor sits in a file
        // the build leaves out.
        "currentGenericResource" => {
            only(&[])?;
            Ok(Value::Unbuilt("currentGenericResource"))
        }
        "currentSolarEnergy" => {
            only(&[])?;
            Ok(Value::Unbuilt("currentSolarEnergy"))
        }
        "currentLunarEnergy" => {
            only(&[])?;
            Ok(Value::Unbuilt("currentLunarEnergy"))
        }
        "druidCurrentEclipsePhase" => {
            only(&["eclipsePhase"])?;
            Ok(Value::Unbuilt("druidCurrentEclipsePhase"))
        }
        "protectionPaladinDamageTakenLastGlobal" => {
            only(&[])?;
            Ok(Value::Unbuilt("protectionPaladinDamageTakenLastGlobal"))
        }
        "dotCritPercentIncrease" => {
            only(&["spellId", "targetUnit", "useBaseValue"])?;
            Ok(Value::Unbuilt("dotCritPercentIncrease"))
        }
        "and" | "or" => {
            only(&["vals"])?;
            let mut values = Vec::new();
            let mut reasons = Vec::new();
            for value in config
                .get("vals")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
            {
                match parse_value(value) {
                    Ok(value) => values.push(value),
                    Err(mut value_reasons) => reasons.append(&mut value_reasons),
                }
            }
            if !reasons.is_empty() {
                Err(reasons)
            } else if name == "and" {
                Ok(Value::And(values))
            } else {
                Ok(Value::Or(values))
            }
        }
        "currentManaPercent" => {
            only(&["sourceUnit"])?;
            // Go `GetSourceUnit`, as for `currentMana`.
            match config.get("sourceUnit") {
                None => Ok(Value::CurrentManaPercent),
                Some(source) => match pet_position(source) {
                    Some(pet) => Ok(Value::PetCurrentManaPercent(pet)),
                    None if parse_value_unit_ref(Some(source), UnitRef::Player)
                        == Ok(UnitRef::Player) =>
                    {
                        Ok(Value::CurrentManaPercent)
                    }
                    None => Err(vec![unsupported_unit(name, "sourceUnit", Some(source))]),
                },
            }
        }
        // No source unit means player HP (existing behavior). A selected
        // world target is a different unit; its percentage must be read from
        // the live world ACK, never treated as the player's health.
        "currentHealthPercent" => {
            only(&["sourceUnit"])?;
            match config.get("sourceUnit") {
                None => Ok(Value::CurrentHealthPercent),
                Some(source) => match parse_value_unit_ref(Some(source), UnitRef::Player) {
                    Ok(UnitRef::Player) => Ok(Value::CurrentHealthPercent),
                    Ok(UnitRef::CurrentTarget) => Ok(Value::WorldCurrentTargetHealthPercent),
                    _ => Err(vec![unsupported_unit(name, "sourceUnit", Some(source))]),
                },
            }
        }
        "remainingTime" => {
            only(&[])?;
            Ok(Value::RemainingTime)
        }
        "remainingTimePercent" => {
            only(&[])?;
            Ok(Value::RemainingTimePercent)
        }
        "currentMana" => {
            only(&["sourceUnit"])?;
            // Go `GetSourceUnit`: the player, or one of its pets by its index (the first
            // without one).
            match config.get("sourceUnit") {
                None => Ok(Value::CurrentMana),
                Some(source) => match pet_position(source) {
                    Some(pet) => Ok(Value::PetCurrentMana(pet)),
                    None if parse_value_unit_ref(Some(source), UnitRef::Player)
                        == Ok(UnitRef::Player) =>
                    {
                        Ok(Value::CurrentMana)
                    }
                    None => Err(vec![unsupported_unit(name, "sourceUnit", Some(source))]),
                },
            }
        }
        "currentTime" => {
            only(&[])?;
            Ok(Value::CurrentTime)
        }
        "currentEnergy" => {
            only(&[])?;
            Ok(Value::CurrentEnergy)
        }
        "maxEnergy" => {
            only(&[])?;
            Ok(Value::MaxEnergy)
        }
        "currentRage" => {
            only(&[])?;
            Ok(Value::CurrentRage)
        }
        "isExecutePhase" => {
            only(&["threshold"])?;
            match config.get("threshold").and_then(Json::as_str) {
                Some("E20") => Ok(Value::IsExecutePhase(20)),
                Some("E25") => Ok(Value::IsExecutePhase(25)),
                Some("E35") => Ok(Value::IsExecutePhase(35)),
                Some("E45") => Ok(Value::IsExecutePhase(45)),
                Some("E90") => Ok(Value::IsExecutePhase(90)),
                other => Err(vec![format!(
                    "isExecutePhase threshold {other:?} is unsupported"
                )]),
            }
        }
        "currentComboPoints" => {
            only(&[])?;
            Ok(Value::CurrentComboPoints)
        }
        "timeToNextEnergyTick" => {
            only(&[])?;
            Ok(Value::TimeToNextEnergyTick)
        }
        "numberTargets" => {
            only(&[])?;
            Ok(Value::NumberTargets)
        }
        "activeItemSwapSet" => parse_swap_set("activeItemSwapSet", config)
            .map(Value::ActiveItemSwapSet)
            .map_err(|reason| vec![reason]),
        "totemRemainingTime" => {
            only(&["totemType", "includeReactionTime"])?;
            let totem = match config.get("totemType").and_then(Json::as_str) {
                None | Some("TypeUnknownTotem") => None,
                Some("Earth") => Some(Totem::Earth),
                Some("Air") => Some(Totem::Air),
                Some("Fire") => Some(Totem::Fire),
                Some("Water") => Some(Totem::Water),
                Some(other) => return Err(vec![format!("totem type {other} is unsupported")]),
            };
            let include_reaction_time = match config.get("includeReactionTime") {
                None => false,
                Some(value) => value
                    .as_bool()
                    .ok_or_else(|| vec!["includeReactionTime must be a boolean".to_string()])?,
            };
            Ok(Value::TotemRemainingTime {
                totem,
                include_reaction_time,
            })
        }
        "gcdIsReady" => {
            only(&[])?;
            Ok(Value::GcdIsReady)
        }
        "autoTimeToNext" => {
            only(&["autoType"])?;
            Ok(Value::AutoTimeToNext(parse_auto_attack_type(name, config)?))
        }
        "autoSwingTime" => {
            only(&["autoType"])?;
            // protojson writes the enum by name and omits the zero value, Unknown.
            let kind = match config.get("autoType").map(|kind| kind.as_str()) {
                None | Some(Some("Unknown")) => SwingType::Unknown,
                Some(Some("MainHand")) => SwingType::MainHand,
                Some(Some("OffHand")) => SwingType::OffHand,
                Some(Some("Ranged")) => SwingType::Ranged,
                Some(other) => {
                    return Err(vec![format!(
                        "autoSwingTime autoType {other:?} is unsupported"
                    )])
                }
            };
            Ok(Value::AutoSwingTime(kind))
        }
        "spellCurrentCost" => {
            only(&["spellId"])?;
            let id = config
                .get("spellId")
                .ok_or_else(|| vec!["spellCurrentCost has no spellId".to_string()])
                .and_then(|id| parse_action_id(id).map_err(|err| vec![err]))?;
            Ok(Value::SpellCurrentCost(id))
        }
        "dotIsActive" | "dotRemainingTime" | "dotTimeToNextTick" => {
            // Go `GetTargetUnit`: no unit reference means the current target. A dot without a
            // spell has no value, which drops the term.
            only(&["spellId", "targetUnit"])?;
            let spell = match config.get("spellId") {
                Some(id) => Some(parse_action_id(id).map_err(|err| vec![err])?),
                None => None,
            };
            let target = parse_value_unit_ref(config.get("targetUnit"), UnitRef::CurrentTarget)
                .map_err(|()| {
                    vec![unsupported_unit(
                        name,
                        "targetUnit",
                        config.get("targetUnit"),
                    )]
                })?;
            let dot = DotRef { spell, target };
            Ok(match name {
                "dotIsActive" => Value::DotIsActive(dot),
                "dotRemainingTime" => Value::DotRemainingTime(dot),
                _ => Value::DotTimeToNextTick(dot),
            })
        }
        "dotPercentIncrease" | "dotTickRatePercentIncrease" => {
            only(&["spellId", "targetUnit", "useBaseValue"])?;
            // Go registers an aura for the base value, which the preparation refuses.
            if config.get("useBaseValue").and_then(Json::as_bool) == Some(true) {
                return Err(vec![format!(
                    "{name} against its base value is unsupported"
                )]);
            }
            let spell = match config.get("spellId") {
                Some(id) => Some(parse_action_id(id).map_err(|err| vec![err])?),
                None => None,
            };
            let target = parse_value_unit_ref(config.get("targetUnit"), UnitRef::CurrentTarget)
                .map_err(|()| {
                    vec![unsupported_unit(
                        name,
                        "targetUnit",
                        config.get("targetUnit"),
                    )]
                })?;
            let dot = DotRef { spell, target };
            Ok(if name == "dotPercentIncrease" {
                Value::DotPercentIncrease(dot)
            } else {
                Value::DotTickRatePercentIncrease(dot)
            })
        }
        "dotTickFrequency" => {
            only(&["spellId", "targetUnit"])?;
            let spell = match config.get("spellId") {
                Some(id) => Some(parse_action_id(id).map_err(|err| vec![err])?),
                None => None,
            };
            let target = parse_value_unit_ref(config.get("targetUnit"), UnitRef::CurrentTarget)
                .map_err(|()| {
                    vec![unsupported_unit(
                        name,
                        "targetUnit",
                        config.get("targetUnit"),
                    )]
                })?;
            Ok(Value::DotTickFrequency(DotRef { spell, target }))
        }
        "allItemStatProcsActive"
        | "anyItemStatProcsActive"
        | "anyItemStatProcsAvailable"
        | "itemProcsMinRemainingTime"
        | "itemProcsMaxRemainingIcd"
        | "numEquippedStatProcItems"
        | "numStatBuffCooldowns"
        | "anyStatBuffCooldownsActive"
        | "anyStatBuffCooldownsMinDuration" => {
            only(&["statType1", "statType2", "statType3", "minIcdSeconds"])?;
            let stats = parse_stat_types(config).map_err(|err| vec![err])?;
            let cooldowns = matches!(
                name,
                "numStatBuffCooldowns"
                    | "anyStatBuffCooldownsActive"
                    | "anyStatBuffCooldownsMinDuration"
            );
            if cooldowns && config.get("minIcdSeconds").is_some() {
                return Err(vec![format!("{name} field minIcdSeconds is unsupported")]);
            }
            let min_icd = match config.get("minIcdSeconds") {
                None => 0,
                Some(seconds) => duration_from_seconds(
                    seconds
                        .as_f64()
                        .ok_or_else(|| vec!["minIcdSeconds must be a number".to_string()])?,
                ),
            };
            let procs = StatProcs {
                stats: stats.clone(),
                min_icd,
            };
            Ok(match name {
                "allItemStatProcsActive" => Value::AllItemStatProcsActive(procs),
                "anyItemStatProcsActive" => Value::AnyItemStatProcsActive(procs),
                "anyItemStatProcsAvailable" => Value::AnyItemStatProcsAvailable(procs),
                "itemProcsMinRemainingTime" => Value::ItemProcsMinRemainingTime(procs),
                "itemProcsMaxRemainingIcd" => Value::ItemProcsMaxRemainingIcd(procs),
                "numEquippedStatProcItems" => Value::NumEquippedStatProcItems(procs),
                "numStatBuffCooldowns" => Value::NumStatBuffCooldowns(stats),
                "anyStatBuffCooldownsActive" => Value::AnyStatBuffCooldownsActive(stats),
                "anyStatBuffCooldownsMinDuration" => Value::AnyStatBuffCooldownsMinDuration(stats),
                _ => unreachable!("a stat proc value"),
            })
        }
        "selectedPotion" | "selectedConjured" => {
            let field = if name == "selectedPotion" {
                "potionId"
            } else {
                "conjuredId"
            };
            only(&[field])?;
            // Go gives no value without an item.
            let item = match config.get(field) {
                Some(id) => parse_action_id(id).map_err(|err| vec![err])?.item_id,
                None => 0,
            };
            Ok(match (name, item) {
                (_, 0) => Value::Unbuilt("selectedPotion"),
                ("selectedPotion", item) => Value::SelectedPotion(item),
                (_, item) => Value::SelectedConjured(item),
            })
        }
        "bossSpellIsCasting" | "bossSpellTimeToReady" => {
            only(&["spellId", "targetUnit"])?;
            let Some(spell) = config.get("spellId") else {
                return Ok(Value::Unbuilt("bossSpellIsCasting"));
            };
            let spell = parse_action_id(spell).map_err(|err| vec![err])?;
            // Go `GetTargetUnit`: no unit reference means the current target.
            let unit = parse_value_unit_ref(config.get("targetUnit"), UnitRef::CurrentTarget)
                .map_err(|()| {
                    vec![unsupported_unit(
                        name,
                        "targetUnit",
                        config.get("targetUnit"),
                    )]
                })?;
            Ok(if name == "bossSpellIsCasting" {
                Value::BossSpellIsCasting { spell, unit }
            } else {
                Value::BossSpellTimeToReady { spell, unit }
            })
        }
        "multipleCdUsages" => {
            only(&["baseSpellId", "targetSpellId", "offset", "alignCdEnd"])?;
            let base = match config.get("baseSpellId") {
                Some(id) => Some(parse_action_id(id).map_err(|err| vec![err])?),
                None => None,
            };
            let Some(target) = config.get("targetSpellId") else {
                return Ok(Value::Unbuilt("multipleCdUsages"));
            };
            let target = parse_action_id(target).map_err(|err| vec![err])?;
            let offset = match config.get("offset") {
                Some(value) => Some(Box::new(parse_value(value)?)),
                None => None,
            };
            let align_cd_end = match config.get("alignCdEnd") {
                None => false,
                Some(value) => value
                    .as_bool()
                    .ok_or_else(|| vec!["alignCdEnd must be a boolean".to_string()])?,
            };
            Ok(Value::MultipleCdUsages {
                base,
                target,
                offset,
                align_cd_end,
            })
        }
        "bossCurrentTarget" => {
            only(&["targetUnit"])?;
            // Go `GetSourceUnit`: no unit reference means the player itself.
            let unit =
                parse_value_unit_ref(config.get("targetUnit"), UnitRef::Player).map_err(|()| {
                    vec![unsupported_unit(
                        name,
                        "targetUnit",
                        config.get("targetUnit"),
                    )]
                })?;
            Ok(Value::BossCurrentTarget(unit))
        }
        "warlockIsAssignedCurse" => {
            only(&["curseType"])?;
            // protojson omits the zero value, Agony.
            Ok(Value::WarlockIsAssignedCurse(
                config
                    .get("curseType")
                    .and_then(Json::as_str)
                    .unwrap_or("Agony")
                    .to_string(),
            ))
        }
        "warlockAssignedCurseIsActive" => {
            only(&["targetUnit"])?;
            let unit = parse_value_unit_ref(config.get("targetUnit"), UnitRef::CurrentTarget)
                .map_err(|()| {
                    vec![unsupported_unit(
                        name,
                        "targetUnit",
                        config.get("targetUnit"),
                    )]
                })?;
            Ok(Value::WarlockAssignedCurseIsActive(unit))
        }
        "sequenceIsComplete" | "sequenceIsReady" | "sequenceTimeToReady" => {
            // Go gives no value without a sequence name.
            let Some(sequence) = parse_sequence_name(name, config).map_err(|err| vec![err])? else {
                return Ok(Value::Unbuilt(match name {
                    "sequenceIsComplete" => "sequenceIsComplete",
                    "sequenceIsReady" => "sequenceIsReady",
                    _ => "sequenceTimeToReady",
                }));
            };
            Ok(match name {
                "sequenceIsComplete" => Value::SequenceIsComplete(sequence),
                "sequenceIsReady" => Value::SequenceIsReady(sequence),
                _ => Value::SequenceTimeToReady(sequence),
            })
        }
        "dotIsActiveOnAllTargets" | "dotLowestRemainingTime" => {
            only(&["spellId"])?;
            // Go `GetAPLMultidotSpell`: no value without a spell.
            let Some(id) = config.get("spellId") else {
                return Ok(Value::Unbuilt(if name == "dotIsActiveOnAllTargets" {
                    "dotIsActiveOnAllTargets"
                } else {
                    "dotLowestRemainingTime"
                }));
            };
            let id = parse_action_id(id).map_err(|err| vec![err])?;
            Ok(if name == "dotIsActiveOnAllTargets" {
                Value::DotIsActiveOnAllTargets(id)
            } else {
                Value::DotLowestRemainingTime(id)
            })
        }
        "auraInternalCooldown" | "auraIcdIsReady" | "auraDuration" => {
            only(&["auraId", "sourceUnit", "includeReactionTime"])?;
            if name != "auraIcdIsReady" && config.get("includeReactionTime").is_some() {
                return Err(vec![format!(
                    "{name} field includeReactionTime is unsupported"
                )]);
            }
            // Go gives no value without an aura.
            let Some(id) = config.get("auraId") else {
                return Ok(Value::Unbuilt(match name {
                    "auraInternalCooldown" => "auraInternalCooldown",
                    "auraIcdIsReady" => "auraIcdIsReady",
                    _ => "auraDuration",
                }));
            };
            let id = parse_action_id(id).map_err(|err| vec![err])?;
            // Go `GetSourceUnit`: no unit reference means the player itself.
            let unit =
                parse_value_unit_ref(config.get("sourceUnit"), UnitRef::Player).map_err(|()| {
                    vec![unsupported_unit(
                        name,
                        "sourceUnit",
                        config.get("sourceUnit"),
                    )]
                })?;
            let include_reaction_time = match config.get("includeReactionTime") {
                None => false,
                Some(value) => value
                    .as_bool()
                    .ok_or_else(|| vec!["includeReactionTime must be a boolean".to_string()])?,
            };
            Ok(match name {
                "auraInternalCooldown" => Value::AuraInternalCooldown { unit, id },
                "auraIcdIsReady" => Value::AuraIcdIsReady {
                    unit,
                    id,
                    include_reaction_time,
                },
                _ => Value::AuraDuration { unit, id },
            })
        }
        "dotBaseDuration" => {
            only(&["spellId"])?;
            // Go `GetAPLDot`: a dot value without a spell has no value.
            let id = match config.get("spellId") {
                Some(id) => Some(parse_action_id(id).map_err(|err| vec![err])?),
                None => None,
            };
            Ok(Value::DotBaseDuration(id))
        }
        "spellIsKnown"
        | "spellIsReady"
        | "spellCastTime"
        | "spellTimeToReady"
        | "spellCanCast"
        | "spellTravelTime"
        | "spellCpm"
        | "spellIsChanneling"
        | "spellChanneledTicks"
        | "spellNumCharges"
        | "spellTimeToCharge"
        | "spellGcdHastedDuration"
        | "spellFullCooldown"
        | "spellInFlight" => {
            only(&["spellId"])?;
            // Go `GetAPLSpell(nil)` finds no spell: spellIsKnown is a constant false, and the
            // values Go checked give no value. The rest stay refused.
            let Some(id) = config.get("spellId") else {
                return match name {
                    "spellIsKnown" => Ok(Value::Const(parse_const("false").expect("a bool"))),
                    "spellCpm" | "spellGcdHastedDuration" => {
                        Err(vec![format!("{name} has no spellId")])
                    }
                    "spellIsReady" => Ok(Value::Unbuilt("spellIsReady")),
                    "spellInFlight" => Ok(Value::Unbuilt("spellInFlight")),
                    "spellCastTime" => Ok(Value::Unbuilt("spellCastTime")),
                    "spellTimeToReady" => Ok(Value::Unbuilt("spellTimeToReady")),
                    "spellCanCast" => Ok(Value::Unbuilt("spellCanCast")),
                    "spellTravelTime" => Ok(Value::Unbuilt("spellTravelTime")),
                    "spellIsChanneling" => Ok(Value::Unbuilt("spellIsChanneling")),
                    "spellChanneledTicks" => Ok(Value::Unbuilt("spellChanneledTicks")),
                    "spellNumCharges" => Ok(Value::Unbuilt("spellNumCharges")),
                    "spellTimeToCharge" => Ok(Value::Unbuilt("spellTimeToCharge")),
                    _ => Ok(Value::Unbuilt("spellFullCooldown")),
                };
            };
            let id = parse_action_id(id).map_err(|err| vec![err])?;
            Ok(match name {
                "spellIsKnown" => Value::SpellIsKnown(id),
                "spellIsReady" => Value::SpellIsReady(id),
                "spellTimeToReady" => Value::SpellTimeToReady(id),
                "spellCanCast" => Value::SpellCanCast(id),
                "spellCastTime" => Value::SpellCastTime(id),
                "spellTravelTime" => Value::SpellTravelTime(id),
                "spellCpm" => Value::SpellCpm(id),
                "spellIsChanneling" => Value::SpellIsChanneling(id),
                "spellChanneledTicks" => Value::SpellChanneledTicks(id),
                "spellNumCharges" => Value::SpellNumCharges(id),
                "spellTimeToCharge" => Value::SpellTimeToCharge(id),
                "spellGcdHastedDuration" => Value::SpellGcdHastedDuration(id),
                "spellFullCooldown" => Value::SpellFullCooldown(id),
                _ => Value::SpellInFlight(id),
            })
        }
        "not" => {
            only(&["val"])?;
            let value = config
                .get("val")
                .ok_or_else(|| vec!["not has no val".to_string()])?;
            Ok(Value::Not(Box::new(parse_value(value)?)))
        }
        "auraIsKnown" | "auraIsActive" | "auraIsInactive" | "auraRemainingTime"
            if config
                .get("sourceUnit")
                .and_then(|unit| unit.get("type"))
                .is_some_and(|kind| kind == "Pet") =>
        {
            // Go GetSourceUnit: a pet of the player by its index.
            only(&["auraId", "sourceUnit"])?;
            let id = config
                .get("auraId")
                .ok_or_else(|| vec![format!("{name} has no auraId")])
                .and_then(|id| parse_action_id(id).map_err(|err| vec![err]))?;
            let unsupported = || {
                vec![format!(
                    "{name} sourceUnit {} is unsupported",
                    config.get("sourceUnit").cloned().unwrap_or_default()
                )]
            };
            let source = config
                .get("sourceUnit")
                .and_then(Json::as_object)
                .ok_or_else(unsupported)?;
            let kind = source.get("type").and_then(Json::as_str);
            let owner_is_self =
                source
                    .get("owner")
                    .and_then(Json::as_object)
                    .is_some_and(|owner| {
                        owner.len() == 1 && owner.get("type").and_then(Json::as_str) == Some("Self")
                    });
            let known_keys = source
                .keys()
                .all(|key| ["type", "index", "owner"].contains(&key.as_str()));
            match kind {
                Some("Pet") if owner_is_self && known_keys => {
                    let pet = match source.get("index") {
                        None => 0,
                        Some(index) => index
                            .as_u64()
                            .and_then(|index| usize::try_from(index).ok())
                            .ok_or_else(unsupported)?,
                    };
                    Ok(match name {
                        "auraIsKnown" => Value::PetAuraIsKnown { pet, id },
                        "auraIsActive" => Value::PetAuraIsActive { pet, id },
                        "auraRemainingTime" => Value::PetAuraRemainingTime { pet, id },
                        // Go `newValueAuraIsInactive`: `!aura.IsActive()`, a constant true for an
                        // aura the pet lacks.
                        _ => Value::Not(Box::new(Value::PetAuraIsActive { pet, id })),
                    })
                }
                _ => Err(unsupported()),
            }
        }
        "auraShouldRefresh" => {
            only(&["auraId", "maxOverlap", "sourceUnit"])?;
            let id = config
                .get("auraId")
                .ok_or_else(|| vec![format!("{name} has no auraId")])
                .and_then(|id| parse_action_id(id).map_err(|err| vec![err]))?;
            // Go `GetTargetUnit`: no unit reference means the current target.
            let unit =
                parse_unit_ref(config.get("sourceUnit"), UnitRef::CurrentTarget).map_err(|()| {
                    vec![unsupported_unit(
                        name,
                        "sourceUnit",
                        config.get("sourceUnit"),
                    )]
                })?;
            let max_overlap = match config.get("maxOverlap") {
                Some(value) => parse_value(value)?,
                // Go defaults a missing overlap to a constant 0ms.
                None => Value::Const(parse_const("0ms").expect("duration constant")),
            };
            Ok(Value::AuraShouldRefresh {
                id,
                unit,
                max_overlap: Box::new(max_overlap),
            })
        }
        "frontOfTarget" => {
            only(&[])?;
            Ok(Value::FrontOfTarget)
        }
        // Go `newValueVariableRef`: the first variable of the name, whose value is built where
        // it is read. A name no variable has gives no value, and a variable that names itself
        // never finishes building in Go.
        "variableRef" => {
            only(&["name"])?;
            let name = config
                .get("name")
                .and_then(Json::as_str)
                .unwrap_or_default();
            let found = VARIABLES.with(|scope| {
                scope
                    .borrow()
                    .iter()
                    .find(|(variable, _)| variable == name)
                    .map(|(_, value)| value.clone())
            });
            let Some(value) = found else {
                return Err(vec![format!("value variable {name:?} is not found")]);
            };
            if RESOLVING.with(|stack| stack.borrow().iter().any(|open| open == name)) {
                return Err(vec![format!("value variable {name:?} refers to itself")]);
            }
            RESOLVING.with(|stack| stack.borrow_mut().push(name.to_string()));
            let resolved = parse_variable_value(&value);
            RESOLVING.with(|stack| stack.borrow_mut().pop());
            Ok(Value::Variable {
                name: name.to_string(),
                resolved: Box::new(resolved?),
            })
        }
        // Go `newValueVariablePlaceholder`: no value without a name.
        "variablePlaceholder" => {
            only(&["name"])?;
            match config.get("name").and_then(Json::as_str) {
                Some(name) if !name.is_empty() => Ok(Value::Placeholder(name.to_string())),
                _ => Err(vec![
                    "a variable placeholder without a name is unsupported".into()
                ]),
            }
        }
        "actionGroupUsed" => {
            only(&["name"])?;
            Ok(Value::ActionGroupUsed {
                name: config
                    .get("name")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_string(),
                used: false,
            })
        }
        "maxMana" => {
            only(&[])?;
            Ok(Value::MaxMana)
        }
        "auraIsKnown" | "auraIsActive" | "auraIsInactive" | "auraNumStacks"
        | "auraRemainingTime" => {
            // Go `GetSourceUnit`: no unit reference means the player itself. Only the activity
            // and stacks of an aura have `includeReactionTime`; Go's request reader rejects the
            // field on the others.
            only(&["auraId", "sourceUnit", "includeReactionTime"])?;
            // Go `newValueAuraIsActive` and `newValueAuraNumStacks` give no value without an aura.
            let Some(id) = config.get("auraId") else {
                return match name {
                    "auraIsActive" => Ok(Value::Unbuilt("auraIsActive")),
                    "auraNumStacks" => Ok(Value::Unbuilt("auraNumStacks")),
                    // Go `newValueAuraRemainingTime`: no value without an aura id.
                    "auraRemainingTime" => Ok(Value::Unbuilt("auraRemainingTime")),
                    _ => Err(vec![format!("{name} has no auraId")]),
                };
            };
            let id = parse_action_id(id).map_err(|err| vec![err])?;
            let unit =
                parse_value_unit_ref(config.get("sourceUnit"), UnitRef::Player).map_err(|()| {
                    vec![unsupported_unit(
                        name,
                        "sourceUnit",
                        config.get("sourceUnit"),
                    )]
                })?;
            let after_reaction = match config.get("includeReactionTime") {
                None => false,
                Some(value) => value
                    .as_bool()
                    .ok_or_else(|| vec!["includeReactionTime must be a boolean".to_string()])?,
            };
            if after_reaction {
                let read = match name {
                    "auraIsActive" => AuraRead::IsActiveAfterReaction,
                    "auraIsInactive" => AuraRead::IsInactiveAfterReaction,
                    "auraNumStacks" => AuraRead::NumStacksAfterReaction,
                    _ => {
                        return Err(vec![format!(
                            "{name} field includeReactionTime is unsupported"
                        )])
                    }
                };
                return Ok(Value::TargetAura { read, unit, id });
            }
            let read = match name {
                "auraIsKnown" => AuraRead::IsKnown,
                "auraIsActive" | "auraIsInactive" => AuraRead::IsActive,
                "auraNumStacks" => AuraRead::NumStacks,
                _ => AuraRead::RemainingTime,
            };
            let value = match (unit, read) {
                (UnitRef::Player, AuraRead::IsKnown) => Value::AuraIsKnown(id),
                (UnitRef::Player, AuraRead::IsActive) => Value::AuraIsActive(id),
                (UnitRef::Player, AuraRead::NumStacks) => Value::AuraNumStacks(id),
                (UnitRef::Player, AuraRead::RemainingTime) => Value::AuraRemainingTime(id),
                (unit, read) => Value::TargetAura { read, unit, id },
            };
            // Go `newValueAuraIsInactive` reads `!aura.IsActive()` and, for an aura the unit
            // lacks, is a constant true, which is what the negation of the missing aura's
            // constant false folds to.
            Ok(if name == "auraIsInactive" {
                Value::Not(Box::new(value))
            } else {
                value
            })
        }
        other => Err(vec![format!("value {other} is unsupported")]),
    }
}

/// Go `stats.IntTupleToStatsList` of `statType1` to `statType3`: the indices from zero up, a
/// missing field reading Go's zero, Strength.
fn parse_stat_types(config: &Json) -> Result<Vec<i32>, String> {
    let mut stats = Vec::new();
    for field in ["statType1", "statType2", "statType3"] {
        let stat = match config.get(field) {
            None => 0,
            Some(value) => json_i32(value)?,
        };
        if stat >= 0 {
            stats.push(stat);
        }
    }
    Ok(stats)
}

/// An `APLValueAutoAttackType` field. protojson writes the enum by name and omits the zero
/// value, UnknownAuto, which Go's switches read as any auto attack.
fn parse_auto_attack_type(name: &str, config: &Json) -> Result<AutoAttackType, Vec<String>> {
    match config.get("autoType").map(|kind| kind.as_str()) {
        None | Some(Some("UnknownAuto" | "AnyAuto")) => Ok(AutoAttackType::Any),
        Some(Some("MeleeAuto")) => Ok(AutoAttackType::Melee),
        Some(Some("MainHandAuto")) => Ok(AutoAttackType::MainHand),
        Some(Some("OffHandAuto")) => Ok(AutoAttackType::OffHand),
        Some(Some("RangedAuto")) => Ok(AutoAttackType::Ranged),
        Some(other) => Err(vec![format!("{name} autoType {other:?} is unsupported")]),
    }
}

/// Go `DurationFromSeconds`.
pub fn duration_from_seconds(seconds: f64) -> i64 {
    (1_000_000_000_f64 * seconds) as i64
}

/// Go `newValueConst`.
pub fn parse_const(text: &str) -> Result<Const, String> {
    let mut constant = Const {
        value_type: ValueType::String,
        int: 0,
        float: 0.0,
        duration_ns: 0,
        string: text.to_string(),
        boolean: !text.is_empty(),
    };
    if text.eq_ignore_ascii_case("true") {
        constant.boolean = true;
        constant.value_type = ValueType::Bool;
        return Ok(constant);
    }
    if text.eq_ignore_ascii_case("false") {
        constant.boolean = false;
        constant.value_type = ValueType::Bool;
        return Ok(constant);
    }
    if let Some(duration) = parse_go_duration(text) {
        constant.duration_ns = duration;
        constant.value_type = ValueType::Duration;
        return Ok(constant);
    }
    if let Some(int) = parse_go_atoi(text) {
        constant.int = int;
        constant.float = f64::from(int);
        constant.duration_ns = duration_from_seconds(constant.float);
        constant.value_type = ValueType::Int;
        return Ok(constant);
    }
    if text.len() > 1 && text.ends_with('%') {
        if let Some(float) = parse_go_float(&text[..text.len() - 1]) {
            constant.float = float / 100.0;
            constant.duration_ns = duration_from_seconds(float / 100.0);
            constant.value_type = ValueType::Float;
            return Ok(constant);
        }
    }
    if let Some(float) = parse_go_float(text) {
        constant.float = float;
        constant.duration_ns = duration_from_seconds(float);
        constant.value_type = ValueType::Float;
        return Ok(constant);
    }
    // Anything else stays a string, as "1,5s" with a comma does: Go compares it as text with
    // the value on the other side formatted ([`go_duration_string`], [`go_float_string`]).
    Ok(constant)
}

/// Go `time.Duration.String`, which a duration coerced to a string reads: "1h2m3.5s", "1.5s",
/// "500ms", "1.234µs", "2ns", "0s".
pub fn go_duration_string(ns: i64) -> String {
    let negative = ns < 0;
    let mut u = ns.unsigned_abs();
    // Go `fmtFrac`: the fraction of v / 10^prec without trailing zeros, nor a point when none.
    let frac = |v: u64, prec: u32| -> (String, u64) {
        let mut digits = String::new();
        let mut v = v;
        let mut print = false;
        for _ in 0..prec {
            let digit = v % 10;
            print = print || digit != 0;
            if print {
                digits.insert(0, char::from(b'0' + digit as u8));
            }
            v /= 10;
        }
        if print {
            digits.insert(0, '.');
        }
        (digits, v)
    };
    let mut out = if u < 1_000_000_000 {
        let (unit, prec) = match u {
            0 => return "0s".to_string(),
            1..=999 => ("ns", 0),
            1_000..=999_999 => ("\u{b5}s", 3),
            _ => ("ms", 6),
        };
        let (fraction, whole) = frac(u, prec);
        format!("{whole}{fraction}{unit}")
    } else {
        let (fraction, seconds) = frac(u, 9);
        u = seconds;
        let mut text = format!("{}{fraction}s", u % 60);
        u /= 60;
        if u > 0 {
            text = format!("{}m{text}", u % 60);
            u /= 60;
            if u > 0 {
                text = format!("{u}h{text}");
            }
        }
        text
    };
    if negative {
        out.insert(0, '-');
    }
    out
}

/// Go `fmt.Sprintf("%.3f", v)`, which a float coerced to a string reads.
pub fn go_float_string(value: f64) -> String {
    format!("{value:.3}")
}

/// Go `strconv.Atoi` for the int32 values the APL stores.
fn parse_go_atoi(text: &str) -> Option<i32> {
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Go keeps int32(Atoi(...)); values outside int32 would wrap there, so reject them.
    text.parse::<i64>().ok().and_then(|v| i32::try_from(v).ok())
}

/// Go `strconv.ParseFloat` for plain decimal and exponent forms. Special values such as
/// "inf", "nan" and hexadecimal floats are rejected rather than reinterpreted.
fn parse_go_float(text: &str) -> Option<f64> {
    let body = text.strip_prefix(['+', '-']).unwrap_or(text);
    if body.is_empty()
        || !body
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
        || !body
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_digit() || b == b'.')
    {
        return None;
    }
    text.parse::<f64>().ok().filter(|value| value.is_finite())
}

/// Go `time.ParseDuration`, ported from the standard library's integer algorithm.
pub fn parse_go_duration(text: &str) -> Option<i64> {
    let mut s = text.as_bytes();
    let mut negative = false;
    if let Some((&sign, rest)) = s.split_first() {
        if sign == b'-' || sign == b'+' {
            negative = sign == b'-';
            s = rest;
        }
    }
    if s == b"0" {
        return Some(0);
    }
    if s.is_empty() {
        return None;
    }
    let mut total: u64 = 0;
    while !s.is_empty() {
        if !(s[0] == b'.' || s[0].is_ascii_digit()) {
            return None;
        }
        let (mut value, rest, int_digits) = leading_int(s)?;
        s = rest;
        let mut fraction = 0u64;
        let mut scale = 1f64;
        let mut fraction_digits = false;
        if let Some((&b'.', rest)) = s.split_first() {
            s = rest;
            let (f, rest, digits, sc) = leading_fraction(s);
            fraction = f;
            scale = sc;
            fraction_digits = digits;
            s = rest;
        }
        if !int_digits && !fraction_digits {
            return None;
        }
        let unit_length = s
            .iter()
            .position(|b| *b == b'.' || b.is_ascii_digit())
            .unwrap_or(s.len());
        if unit_length == 0 {
            return None;
        }
        let unit: u64 = match &s[..unit_length] {
            b"ns" => 1,
            // "us", then U+00B5 and U+03BC micro signs followed by "s".
            b"us" | [0xc2, 0xb5, b's'] | [0xce, 0xbc, b's'] => 1_000,
            b"ms" => 1_000_000,
            b"s" => 1_000_000_000,
            b"m" => 60_000_000_000,
            b"h" => 3_600_000_000_000,
            _ => return None,
        };
        s = &s[unit_length..];
        if value > (1u64 << 63) / unit {
            return None;
        }
        value *= unit;
        if fraction > 0 {
            value = value.checked_add((fraction as f64 * (unit as f64 / scale)) as u64)?;
            if value > 1u64 << 63 {
                return None;
            }
        }
        total = total.checked_add(value)?;
        if total > 1u64 << 63 {
            return None;
        }
    }
    if negative {
        return Some((total as i64).wrapping_neg());
    }
    if total > (1u64 << 63) - 1 {
        return None;
    }
    Some(total as i64)
}

fn leading_int(s: &[u8]) -> Option<(u64, &[u8], bool)> {
    let mut value: u64 = 0;
    let mut index = 0;
    while index < s.len() && s[index].is_ascii_digit() {
        if value > (1u64 << 63) / 10 {
            return None;
        }
        value = value * 10 + u64::from(s[index] - b'0');
        if value > 1u64 << 63 {
            return None;
        }
        index += 1;
    }
    Some((value, &s[index..], index > 0))
}

fn leading_fraction(s: &[u8]) -> (u64, &[u8], bool, f64) {
    let mut value: u64 = 0;
    let mut scale = 1f64;
    let mut overflow = false;
    let mut index = 0;
    while index < s.len() && s[index].is_ascii_digit() {
        if !overflow {
            if value > (1u64 << 63) / 10 {
                overflow = true;
            } else {
                let next = value * 10 + u64::from(s[index] - b'0');
                if next > 1u64 << 63 {
                    overflow = true;
                } else {
                    value = next;
                    scale *= 10.0;
                }
            }
        }
        index += 1;
    }
    (value, &s[index..], index > 0, scale)
}

/// A compiled condition: Go's constant folding and type coercion applied. `R` names an
/// aura: a runtime handle, or the action ID when only the compiled shape matters.
#[derive(Clone, Debug, PartialEq)]
pub enum Compiled<R> {
    Const(Const),
    Compare {
        op: CompareOp,
        lhs: Box<Compiled<R>>,
        rhs: Box<Compiled<R>>,
    },
    And(Vec<Compiled<R>>),
    Or(Vec<Compiled<R>>),
    Not(Box<Compiled<R>>),
    /// Go `APLValueMath`.
    Math {
        op: MathOp,
        lhs: Box<Compiled<R>>,
        rhs: Box<Compiled<R>>,
    },
    TotemRemainingTime {
        totem: Totem,
        include_reaction_time: bool,
    },
    CurrentManaPercent,
    /// Go `APLValueCurrentHealthPercent` of the player.
    CurrentHealthPercent,
    /// Live gsim ACKed current-target HP for native/world optimized actions.
    WorldCurrentTargetHealthPercent,
    RemainingTimePercent,
    CurrentMana,
    /// The mana of a pet the fight simulates, by its place among the simulated pets.
    PetCurrentMana(u8),
    /// The same pet's mana as a share of its maximum.
    PetCurrentManaPercent(u8),
    CurrentEnergy,
    MaxEnergy,
    MaxMana,
    FrontOfTarget,
    /// Go `APLValueAuraShouldRefresh` with the overlap coerced to a duration.
    AuraShouldRefresh {
        aura: R,
        overlap: Box<Compiled<R>>,
    },
    CurrentComboPoints,
    TimeToNextEnergyTick,
    CurrentRage,
    IsExecutePhase(i32),
    RemainingTime,
    CurrentTime,
    NumberTargets,
    AuraIsActive(R),
    /// Go `APLValueAuraIsActive` with `includeReactionTime`.
    AuraIsActiveAfterReaction(R),
    /// Go `APLValueAuraNumStacks` with `includeReactionTime`.
    AuraNumStacksAfterReaction(R),
    /// Go `APLValueActionGroupUsed`, settled when the groups were bound.
    GroupUsed(bool),
    /// Go `APLValueAuraIsInactive` with `includeReactionTime`.
    AuraIsInactiveAfterReaction(R),
    AuraNumStacks(R),
    AuraRemainingTime(R),
    /// A dot by the spell holding it and the target it is read on.
    DotIsActive(DotAt),
    DotRemainingTime(DotAt),
    SpellIsReady(usize),
    SpellCastTime(usize),
    SpellTimeToReady(usize),
    DotTimeToNextTick(DotAt),
    /// Go `APLValueDotBaseDuration` with the base duration it captured.
    DotBaseDuration(i64),
    /// Go `APLValueActiveItemSwapSet`: whether the set is worn, Swap1 when true.
    ActiveItemSwapSet(bool),
    GcdIsReady,
    SpellCanCast(usize),
    AutoTimeToNext(AutoAttackType),
    AutoSwingTime(SwingType),
    SpellCurrentCost(usize),
    SpellTravelTime(usize),
    SpellCpm(usize),
    SpellIsChanneling(usize),
    SpellChanneledTicks(usize),
    SpellNumCharges(usize),
    SpellTimeToCharge(usize),
    SpellGcdHastedDuration(usize),
    SpellFullCooldown(usize),
    SpellInFlight(usize),
    /// Go `APLValueMax` of values coerced to one type.
    Max(Vec<Compiled<R>>),
    /// Go `APLValueMin` of values coerced to one type.
    Min(Vec<Compiled<R>>),
    CurrentTimePercent,
    CurrentHealth,
    MaxHealth,
    /// Go `APLValueMaxRage` with the maximum it read when built.
    MaxRage(f64),
    /// Go `APLValueMaxComboPoints` with the maximum it read when built.
    MaxComboPoints(i32),
    CurrentManaRegen,
    /// Go `APLValueEnergyTimeToTarget` with the target coerced to a float.
    EnergyTimeToTarget(Box<Compiled<R>>),
    /// Go `APLValueUnitIsMoving` of the player.
    PlayerIsMoving,
    UnitDistance,
    GcdTimeToReady,
    AutoTimeSinceLast(AutoAttackType),
    InputDelay,
    ChannelClipDelay,
    /// Go `APLValueAuraInternalCooldown` of the aura holding the ICD.
    AuraInternalCooldown(R),
    /// Go `APLValueAuraICDIsReady` of the aura holding the ICD.
    AuraIcdIsReady {
        aura: R,
        include_reaction_time: bool,
    },
    /// Go `APLValueAuraDuration`.
    AuraDuration(R),
    /// Go `APLValueDotIsActiveOnAllTargets`: the dot on each target.
    DotIsActiveOnAllTargets(Vec<DotAt>),
    /// Go `APLValueDotLowestRemainingTime`: the dot on each target.
    DotLowestRemainingTime(Vec<DotAt>),
    DotTickFrequency(DotAt),
    /// Go `APLValueDotPercentIncrease` of the spell holding the dot, on a target.
    DotPercentIncrease(DotAt),
    /// Go `APLValueDotTickRatePercentIncrease`.
    DotTickRatePercentIncrease(DotAt),
    /// A boolean whose state never changes during a fight, which Go reads each time without
    /// folding: `APLValueWarlockIsAssignedCurse`, `APLValueBossCurrentTarget` and the selected
    /// consumable values, and a target spell's hardcast, which no target without an AI starts.
    Settled(bool),
    /// A duration that never changes during a fight, which Go reads without folding: the time
    /// until a target's spell without a cooldown is ready.
    SettledDuration(i64),
    /// An int that never changes during a fight, which Go reads without folding: the matching
    /// stat buff cooldowns that are worn, and the matching proc buffs that can proc.
    SettledInt(i32),
    /// The item stat proc values over the matching proc buffs, each with whether it is
    /// swapped out.
    AllItemStatProcsActive(Vec<(R, bool)>),
    AnyItemStatProcsActive(Vec<(R, bool)>),
    AnyItemStatProcsAvailable(Vec<(R, bool)>),
    ItemProcsMinRemainingTime(Vec<(R, bool)>),
    ItemProcsMaxRemainingIcd(Vec<(R, bool)>),
    /// The stat buff cooldown values over the matching cooldowns' buffs.
    AnyStatBuffCooldownsActive(Vec<R>),
    AnyStatBuffCooldownsMinDuration(Vec<R>),
    /// Go `APLValueBossSpellIsCasting` of one of the player's own spells.
    PlayerSpellCasting(usize),
    /// Go `APLValueMultipleCdUsages`: the target spell and its buff, the base spell and its buff
    /// when it has one, the offset as a duration and whether to align with the fight's end.
    MultipleCdUsages {
        target: usize,
        target_buff: R,
        base: Option<(usize, R)>,
        offset: Box<Compiled<R>>,
        align_cd_end: bool,
    },
    /// Go `APLValueSequenceIsComplete` of the sequence of the name.
    SequenceIsComplete(String),
    SequenceIsReady(String),
    SequenceTimeToReady(String),
    /// Go `APLValueCoerced`.
    Coerced {
        to: ValueType,
        inner: Box<Compiled<R>>,
    },
}

impl<R> Compiled<R> {
    pub fn value_type(&self) -> ValueType {
        match self {
            Compiled::Const(constant) => constant.value_type,
            Compiled::Compare { .. }
            | Compiled::And(_)
            | Compiled::Or(_)
            | Compiled::Not(_)
            | Compiled::AuraIsActive(_)
            | Compiled::AuraIsActiveAfterReaction(_)
            | Compiled::AuraIsInactiveAfterReaction(_)
            | Compiled::DotIsActive(_)
            | Compiled::SpellIsReady(_)
            | Compiled::IsExecutePhase(_)
            | Compiled::SpellCanCast(_)
            | Compiled::GroupUsed(_)
            | Compiled::ActiveItemSwapSet(_)
            | Compiled::SpellIsChanneling(_)
            | Compiled::SpellInFlight(_)
            | Compiled::GcdIsReady => ValueType::Bool,
            Compiled::AuraNumStacks(_)
            | Compiled::AuraNumStacksAfterReaction(_)
            | Compiled::NumberTargets
            | Compiled::SpellChanneledTicks(_)
            | Compiled::SpellNumCharges(_)
            | Compiled::CurrentComboPoints => ValueType::Int,
            Compiled::AuraRemainingTime(_)
            | Compiled::AutoTimeToNext(_)
            | Compiled::AutoSwingTime(_)
            | Compiled::DotRemainingTime(_)
            | Compiled::SpellCastTime(_)
            | Compiled::SpellTimeToReady(_)
            | Compiled::DotTimeToNextTick(_)
            | Compiled::DotBaseDuration(_)
            | Compiled::RemainingTime
            | Compiled::TotemRemainingTime { .. }
            | Compiled::CurrentTime
            | Compiled::SpellTravelTime(_)
            | Compiled::SpellTimeToCharge(_)
            | Compiled::SpellGcdHastedDuration(_)
            | Compiled::SpellFullCooldown(_)
            | Compiled::TimeToNextEnergyTick => ValueType::Duration,
            Compiled::CurrentManaPercent
            | Compiled::PetCurrentManaPercent(_)
            | Compiled::CurrentHealthPercent
            | Compiled::WorldCurrentTargetHealthPercent
            | Compiled::CurrentMana
            | Compiled::PetCurrentMana(_)
            | Compiled::RemainingTimePercent
            | Compiled::CurrentEnergy
            | Compiled::CurrentRage
            | Compiled::MaxEnergy
            | Compiled::SpellCurrentCost(_)
            | Compiled::SpellCpm(_)
            | Compiled::MaxMana => ValueType::Float,
            Compiled::FrontOfTarget | Compiled::AuraShouldRefresh { .. } => ValueType::Bool,
            Compiled::Math { op, lhs, rhs } => op.result_type(lhs.value_type(), rhs.value_type()),
            // Go reads the first value's type, which coercion made every value's.
            Compiled::Max(values) | Compiled::Min(values) => values[0].value_type(),
            Compiled::PlayerIsMoving
            | Compiled::AuraIcdIsReady { .. }
            | Compiled::SequenceIsComplete(_)
            | Compiled::SequenceIsReady(_)
            | Compiled::Settled(_)
            | Compiled::PlayerSpellCasting(_)
            | Compiled::MultipleCdUsages { .. }
            | Compiled::AllItemStatProcsActive(_)
            | Compiled::AnyItemStatProcsActive(_)
            | Compiled::AnyItemStatProcsAvailable(_)
            | Compiled::AnyStatBuffCooldownsActive(_)
            | Compiled::DotIsActiveOnAllTargets(_) => ValueType::Bool,
            Compiled::SequenceTimeToReady(_)
            | Compiled::SettledDuration(_)
            | Compiled::ItemProcsMinRemainingTime(_)
            | Compiled::ItemProcsMaxRemainingIcd(_)
            | Compiled::AnyStatBuffCooldownsMinDuration(_) => ValueType::Duration,
            Compiled::SettledInt(_) => ValueType::Int,
            Compiled::DotPercentIncrease(_) | Compiled::DotTickRatePercentIncrease(_) => {
                ValueType::Float
            }
            Compiled::AuraInternalCooldown(_)
            | Compiled::AuraDuration(_)
            | Compiled::DotLowestRemainingTime(_)
            | Compiled::DotTickFrequency(_) => ValueType::Duration,
            Compiled::MaxComboPoints(_) => ValueType::Int,
            Compiled::CurrentTimePercent
            | Compiled::CurrentHealth
            | Compiled::MaxHealth
            | Compiled::MaxRage(_)
            | Compiled::CurrentManaRegen
            | Compiled::UnitDistance => ValueType::Float,
            Compiled::EnergyTimeToTarget(_)
            | Compiled::GcdTimeToReady
            | Compiled::AutoTimeSinceLast(_)
            | Compiled::InputDelay
            | Compiled::ChannelClipDelay => ValueType::Duration,
            Compiled::Coerced { to, .. } => *to,
        }
    }

    fn const_bool(&self) -> Option<bool> {
        match self {
            Compiled::Const(constant) if constant.value_type == ValueType::Bool => {
                Some(constant.boolean)
            }
            _ => None,
        }
    }

    /// Go `coerceTo`: a constant changes type in place; anything else is wrapped.
    fn coerce(self, to: ValueType) -> Self {
        if self.value_type() == to {
            self
        } else if let Compiled::Const(mut constant) = self {
            constant.value_type = to;
            Compiled::Const(constant)
        } else {
            Compiled::Coerced {
                to,
                inner: Box::new(self),
            }
        }
    }
}

impl<R: Clone> Compiled<R> {
    /// What the value means, with comparisons of constants evaluated and And and Or
    /// simplified. Go keeps such comparisons, so this is only for comparing compilations.
    fn folded(&self) -> Compiled<R> {
        match self {
            Compiled::Compare { op, lhs, rhs } => {
                let (lhs, rhs) = (lhs.folded(), rhs.folded());
                match (&lhs, &rhs) {
                    (Compiled::Const(a), Compiled::Const(b)) if a.value_type == b.value_type => {
                        let result = match a.value_type {
                            ValueType::Int => Some(op.apply(a.int, b.int)),
                            ValueType::Float => Some(op.apply(a.float, b.float)),
                            ValueType::Duration => Some(op.apply(a.duration_ns, b.duration_ns)),
                            ValueType::Bool => match op {
                                CompareOp::Eq => Some(a.boolean == b.boolean),
                                CompareOp::Ne => Some(a.boolean != b.boolean),
                                _ => None,
                            },
                            ValueType::String => None,
                        };
                        match result {
                            Some(value) => bool_const(value),
                            None => Compiled::Compare {
                                op: *op,
                                lhs: Box::new(lhs),
                                rhs: Box::new(rhs),
                            },
                        }
                    }
                    _ => Compiled::Compare {
                        op: *op,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                }
            }
            Compiled::And(values) => simplify(values, true, Compiled::And),
            Compiled::Not(value) => {
                let value = value.folded();
                match value.const_bool() {
                    Some(constant) => bool_const(!constant),
                    None => Compiled::Not(Box::new(value)),
                }
            }
            Compiled::Or(values) => simplify(values, false, Compiled::Or),
            Compiled::Coerced { to, inner } => Compiled::Coerced {
                to: *to,
                inner: Box::new(inner.folded()),
            },
            // Go never folds arithmetic; its operands still fold.
            Compiled::Math { op, lhs, rhs } => Compiled::Math {
                op: *op,
                lhs: Box::new(lhs.folded()),
                rhs: Box::new(rhs.folded()),
            },
            other => other.clone(),
        }
    }
}

/// Fold And (`identity` true) or Or (`identity` false): identity terms drop out, the
/// other constant decides, and one remaining term stands for the operator.
fn simplify<R: Clone>(
    values: &[Compiled<R>],
    identity: bool,
    build: fn(Vec<Compiled<R>>) -> Compiled<R>,
) -> Compiled<R> {
    let mut kept = Vec::new();
    for value in values.iter().map(Compiled::folded) {
        match value.const_bool() {
            Some(constant) if constant == identity => {}
            Some(_) => return bool_const(!identity),
            None => kept.push(value),
        }
    }
    match kept.len() {
        0 => bool_const(identity),
        1 => kept.pop().expect("one value"),
        _ => build(kept),
    }
}

impl CompareOp {
    fn apply<T: PartialOrd>(self, lhs: T, rhs: T) -> bool {
        match self {
            CompareOp::Eq => lhs == rhs,
            CompareOp::Ne => lhs != rhs,
            CompareOp::Lt => lhs < rhs,
            CompareOp::Le => lhs <= rhs,
            CompareOp::Gt => lhs > rhs,
            CompareOp::Ge => lhs >= rhs,
        }
    }
}

/// An aura a rotation names, as Go `GetAPLAura` finds it on the casting player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoundAura<R> {
    pub aura: R,
    pub max_stacks: i32,
}

/// How compilation resolves the names a rotation uses, as Go does on the casting player.
pub struct Lookup<'a, R> {
    /// Go `GetAuraByID`: the aura, or `None` when the character lacks it.
    pub aura: &'a dyn Fn(&ActionId) -> Option<FoundAura<R>>,
    /// Go `GetAuraByID` on a target, by its position among the encounter's targets.
    pub target_aura: &'a dyn Fn(usize, &ActionId) -> Option<FoundAura<R>>,
    /// How many targets the encounter has, which resolves a unit reference.
    pub targets: usize,
    /// Go `GetAPLSpell`: the spellbook position of the spell, or `None` when unknown.
    pub spell: &'a dyn Fn(&ActionId) -> Option<usize>,
    /// Go `GetAPLDot`: the spellbook position of the spell when it has a dot on the unit.
    pub dot: &'a dyn Fn(&ActionId, Unit) -> Option<usize>,
    /// Go `GetAPLDot` on the first target and `Dot.BaseDuration`, as the rotation was built: the
    /// base duration in nanoseconds, or `None` when the spell has no such dot.
    pub dot_base_duration: &'a dyn Fn(&ActionId) -> Option<i64>,
    /// Go `GetAuraByID` on the player's pet at a position among its pets: whether it has the
    /// aura, false when there is no such pet.
    pub pet_aura_known: &'a dyn Fn(usize, &ActionId) -> bool,
    /// Go `GetAuraByID` on the player's pet at a position among its pets, while the pet is
    /// simulated: its aura, `None` for a pet that is never summoned or lacks the aura.
    pub pet_aura: &'a dyn Fn(usize, &ActionId) -> Option<FoundAura<R>>,
    /// The player's pet at a position among its pets, while the fight simulates it and it has a
    /// mana bar: its place among the simulated pets.
    pub simulated_pet_with_mana: &'a dyn Fn(usize) -> Option<u8>,
    /// Whether the player's pet at a position is one the fight never summons that has a mana
    /// bar: Go resets its mana to full and nothing spends it.
    pub unsummoned_pet_with_mana: &'a dyn Fn(usize) -> bool,
    /// That pet's full mana where the input states it.
    pub unsummoned_pet_max_mana: &'a dyn Fn(usize) -> Option<f64>,
    /// Go `ItemSwap.IsEnabled`: whether the character swaps items.
    pub item_swap: bool,
    /// The player's resource bars as the rotation is built, which values of a bar the player
    /// lacks give none for.
    pub bars: Bars,
    /// Go `GetIcdAuraByID` on a unit: the first aura with the action, or triggered by it, that
    /// has an ICD.
    pub icd_aura: &'a dyn Fn(Unit, &ActionId) -> Option<R>,
    /// Go `GetAPLMultidotSpell`: whether the spell has a dot `Spell.CurDot` returns.
    pub multidot: &'a dyn Fn(&ActionId) -> bool,
    /// A Warlock's assigned curse: its option name and the action of the curse spell. Only a
    /// Warlock builds the curse values.
    pub assigned_curse: Option<(&'a str, &'a ActionId)>,
    /// Whether the player tanks the targets, whose current target it then is.
    pub tanked: bool,
    /// What the rotation's values read that the rest of the input leaves out.
    pub reads: Option<&'a crate::contracts::prepared_v2::RotationReads>,
    /// Go `Unit.GetSpell` on the player: the spell of exactly the action.
    pub exact_spell: &'a dyn Fn(&ActionId) -> Option<usize>,
    /// Go `Spell.RelatedSelfBuff` of the spell at a spellbook position, as `reads` names it.
    pub self_buff: &'a dyn Fn(usize) -> Option<R>,
    /// Go `GetMatchingItemProcAuras`: the item proc buffs moving one of the stats whose ICD
    /// exceeds the minimum when there is one, each with whether it is swapped out.
    pub item_procs: &'a dyn Fn(&StatProcs) -> Vec<(R, bool)>,
    /// The player's aura of a label.
    pub aura_by_label: &'a dyn Fn(&str) -> Option<R>,
    /// Go `GetAPLSpell` of a spell with an `ExpectedTickDamage`, as the spellbook position of
    /// the spell holding its dot.
    pub expected_tick: &'a dyn Fn(&ActionId) -> Option<usize>,
}

/// The player's resource bars: Go `HasManaBar`, the rage bar's `MaximumRage` and the energy
/// bar's `MaxComboPoints`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bars {
    pub mana: bool,
    pub max_rage: Option<f64>,
    pub max_combo_points: Option<i32>,
}

/// What Go `newAPLAction` makes of an action's condition.
#[derive(Clone, Debug, PartialEq)]
pub enum CompiledCondition<R> {
    /// A constant false condition removes the action.
    Pruned,
    /// No condition, a constant true, or a condition that compiled to no value.
    Always,
    When(Compiled<R>),
}

impl<R: Clone + PartialEq> CompiledCondition<R> {
    /// Whether two compilations act the same, though Go may keep constant comparisons.
    #[cfg(test)]
    pub fn same_meaning(&self, other: &Self) -> bool {
        self.folded() == other.folded()
    }

    /// Whether the condition can never hold, though Go may keep evaluating it.
    pub fn never_holds(&self) -> bool {
        self.folded() == CompiledCondition::Pruned
    }

    fn folded(&self) -> Self {
        match self {
            CompiledCondition::When(value) => {
                let value = value.folded();
                match value.const_bool() {
                    Some(true) => CompiledCondition::Always,
                    Some(false) => CompiledCondition::Pruned,
                    None => CompiledCondition::When(value),
                }
            }
            other => other.clone(),
        }
    }
}

fn bool_const<R>(value: bool) -> Compiled<R> {
    Compiled::Const(parse_const(if value { "true" } else { "false" }).expect("bool constant"))
}

/// Go `newValueAnd` and `newValueOr`: terms without a value drop out, one term stands for
/// the operator, and a constant that decides the result replaces it.
fn fold<R>(
    values: &[Value],
    lookup: &Lookup<R>,
    deciding: bool,
    build: fn(Vec<Compiled<R>>) -> Compiled<R>,
) -> Option<Compiled<R>> {
    let mut compiled: Vec<_> = values
        .iter()
        .filter_map(|value| compile_value(value, lookup))
        .map(|value| value.coerce(ValueType::Bool))
        .collect();
    match compiled.len() {
        0 => None,
        1 => compiled.pop(),
        _ => Some(
            match compiled
                .iter()
                .position(|v| v.const_bool() == Some(deciding))
            {
                Some(index) => compiled.swap_remove(index),
                None => build(compiled),
            },
        ),
    }
}

/// Go `NewDotReference`: the dot of a spell on the unit a dot value names. No spell, no unit
/// or no dot on the unit gives no value.
fn dot_at<R>(dot: &DotRef, lookup: &Lookup<R>) -> Option<DotAt> {
    let unit = dot.target.resolve(lookup.targets)?;
    let spell = (lookup.dot)(dot.spell.as_ref()?, unit)?;
    let target = match unit {
        Unit::Target(position) => position,
        Unit::Player => 0,
    };
    Some(DotAt { spell, target })
}

/// Go `newAPLValue` for the supported subset. A spell or dot the character lacks gives no
/// value, so the term drops out of its parent. An aura the character lacks reads as inactive,
/// with no stacks and no time left (ElliotWood/Forever#622).
fn compile_value<R>(value: &Value, lookup: &Lookup<R>) -> Option<Compiled<R>> {
    let aura = lookup.aura;
    Some(match value {
        Value::Const(constant) => Compiled::Const(constant.clone()),
        Value::CurrentManaPercent => Compiled::CurrentManaPercent,
        Value::CurrentHealthPercent => Compiled::CurrentHealthPercent,
        Value::WorldCurrentTargetHealthPercent => Compiled::WorldCurrentTargetHealthPercent,
        Value::RemainingTimePercent => Compiled::RemainingTimePercent,
        Value::CurrentMana => Compiled::CurrentMana,
        // Go `newValueCurrentMana` on a pet: no value for a pet without a mana bar. A pet the
        // fight never summons is refused by the gate.
        // A pet the fight never summons keeps the full mana Go's reset gives it.
        Value::PetCurrentMana(pet) => match (lookup.simulated_pet_with_mana)(*pet) {
            Some(slot) => Compiled::PetCurrentMana(slot),
            None => Compiled::Const(
                parse_const(&format!("{}", (lookup.unsummoned_pet_max_mana)(*pet)?))
                    .expect("a float constant"),
            ),
        },
        // Go `newValueCurrentManaPercent` on a pet: no value without a mana bar, and a full bar
        // for a pet the fight never summons, whose mana the reset fills and nothing spends.
        Value::PetCurrentManaPercent(pet) => match (lookup.simulated_pet_with_mana)(*pet) {
            Some(slot) => Compiled::PetCurrentManaPercent(slot),
            None if (lookup.unsummoned_pet_with_mana)(*pet) => {
                Compiled::Const(parse_const("100%").expect("percent constant"))
            }
            None => return None,
        },
        // Go `newValueAuraRemainingTime` on a pet: a constant zero for an aura the pet lacks or
        // for a pet never summoned, whose auras never go up.
        Value::PetAuraRemainingTime { pet, id } => match (lookup.pet_aura)(*pet, id) {
            Some(found) => Compiled::AuraRemainingTime(found.aura),
            None => {
                (lookup.pet_aura_known)(*pet, id);
                Compiled::Const(parse_const("0ms").expect("duration constant"))
            }
        },
        // Go `newValueCurrentEnergy` and its siblings: no value (and a warning) for a player
        // without an energy bar, as a Balance Druid's rotation reading energy has.
        Value::CurrentEnergy => {
            lookup.bars.max_combo_points?;
            Compiled::CurrentEnergy
        }
        Value::CurrentRage => Compiled::CurrentRage,
        Value::IsExecutePhase(threshold) => Compiled::IsExecutePhase(*threshold),
        Value::MaxEnergy => {
            lookup.bars.max_combo_points?;
            Compiled::MaxEnergy
        }
        Value::MaxMana => Compiled::MaxMana,
        Value::FrontOfTarget => Compiled::FrontOfTarget,
        // Go `newValueAuraShouldRefresh`: no value without the aura.
        Value::AuraShouldRefresh {
            id,
            unit,
            max_overlap,
        } => {
            let found = match unit.resolve(lookup.targets)? {
                Unit::Target(position) => (lookup.target_aura)(position, id),
                Unit::Player => aura(id),
            }?;
            // Go gives a missing overlap a constant 0ms.
            let overlap = compile_value(max_overlap, lookup)
                .map(|value| value.coerce(ValueType::Duration))
                .unwrap_or_else(|| Compiled::Const(parse_const("0ms").expect("duration constant")));
            Compiled::AuraShouldRefresh {
                aura: found.aura,
                overlap: Box::new(overlap),
            }
        }
        Value::CurrentComboPoints => {
            lookup.bars.max_combo_points?;
            Compiled::CurrentComboPoints
        }
        Value::TimeToNextEnergyTick => {
            lookup.bars.max_combo_points?;
            Compiled::TimeToNextEnergyTick
        }
        Value::RemainingTime => Compiled::RemainingTime,
        Value::CurrentTime => Compiled::CurrentTime,
        Value::NumberTargets => Compiled::NumberTargets,
        Value::TotemRemainingTime {
            totem,
            include_reaction_time,
        } => Compiled::TotemRemainingTime {
            totem: (*totem)?,
            include_reaction_time: *include_reaction_time,
        },
        Value::DotIsActive(dot) => Compiled::DotIsActive(dot_at(dot, lookup)?),
        Value::DotRemainingTime(dot) => Compiled::DotRemainingTime(dot_at(dot, lookup)?),
        // Go `newValueSpellIsKnown` is a constant.
        Value::SpellIsKnown(id) => bool_const((lookup.spell)(id).is_some()),
        Value::SpellIsReady(id) => Compiled::SpellIsReady((lookup.spell)(id)?),
        Value::SpellCastTime(id) => Compiled::SpellCastTime((lookup.spell)(id)?),
        Value::SpellTimeToReady(id) => Compiled::SpellTimeToReady((lookup.spell)(id)?),
        Value::SpellCanCast(id) => Compiled::SpellCanCast((lookup.spell)(id)?),
        Value::AutoTimeToNext(auto) => Compiled::AutoTimeToNext(*auto),
        Value::AutoSwingTime(kind) => Compiled::AutoSwingTime(*kind),
        // Go GetAPLSpell: an unknown spell gives no value.
        Value::SpellCurrentCost(id) => Compiled::SpellCurrentCost((lookup.spell)(id)?),
        Value::SpellTravelTime(id) => Compiled::SpellTravelTime((lookup.spell)(id)?),
        Value::SpellCpm(id) => Compiled::SpellCpm((lookup.spell)(id)?),
        Value::SpellIsChanneling(id) => Compiled::SpellIsChanneling((lookup.spell)(id)?),
        Value::SpellChanneledTicks(id) => Compiled::SpellChanneledTicks((lookup.spell)(id)?),
        Value::SpellNumCharges(id) => Compiled::SpellNumCharges((lookup.spell)(id)?),
        Value::SpellTimeToCharge(id) => Compiled::SpellTimeToCharge((lookup.spell)(id)?),
        Value::SpellGcdHastedDuration(id) => Compiled::SpellGcdHastedDuration((lookup.spell)(id)?),
        Value::SpellFullCooldown(id) => Compiled::SpellFullCooldown((lookup.spell)(id)?),
        Value::SpellInFlight(id) => Compiled::SpellInFlight((lookup.spell)(id)?),
        Value::DotTimeToNextTick(dot) => Compiled::DotTimeToNextTick(dot_at(dot, lookup)?),
        // Go `newValueMax` and `newValueMin`: values without a value drop out, the rest are
        // coerced to their highest type, and one value stands for the operator.
        Value::Max(values) | Value::Min(values) => {
            let compiled: Vec<_> = values
                .iter()
                .filter_map(|value| compile_value(value, lookup))
                .collect();
            let to = compiled.iter().map(Compiled::value_type).max()?;
            let mut compiled: Vec<_> = compiled.into_iter().map(|value| value.coerce(to)).collect();
            if compiled.len() == 1 {
                return compiled.pop();
            }
            if matches!(value, Value::Max(_)) {
                Compiled::Max(compiled)
            } else {
                Compiled::Min(compiled)
            }
        }
        Value::CurrentTimePercent => Compiled::CurrentTimePercent,
        Value::CurrentHealth => Compiled::CurrentHealth,
        Value::MaxHealth => Compiled::MaxHealth,
        Value::MaxRage => Compiled::MaxRage(lookup.bars.max_rage?),
        Value::MaxComboPoints => Compiled::MaxComboPoints(lookup.bars.max_combo_points?),
        Value::CurrentManaRegen if lookup.bars.mana => Compiled::CurrentManaRegen,
        Value::CurrentManaRegen => return None,
        // Go `newValueEnergyTimeToTarget`: no value without an energy bar or a target.
        Value::EnergyTimeToTarget(target) => {
            lookup.bars.max_combo_points?;
            let target = compile_value(target, lookup)?.coerce(ValueType::Float);
            Compiled::EnergyTimeToTarget(Box::new(target))
        }
        // Only the player moves; a unit Go finds nothing for gives no value.
        Value::UnitIsMoving(unit) => match unit.resolve(lookup.targets)? {
            Unit::Player => Compiled::PlayerIsMoving,
            Unit::Target(_) => bool_const(false),
        },
        Value::UnitDistance => Compiled::UnitDistance,
        Value::GcdTimeToReady => Compiled::GcdTimeToReady,
        Value::AutoTimeSinceLast(kind) => Compiled::AutoTimeSinceLast(*kind),
        Value::InputDelay => Compiled::InputDelay,
        Value::ChannelClipDelay => Compiled::ChannelClipDelay,
        Value::Unbuilt(_) => return None,
        // Go `GetAPLICDAura`: no value for a unit Go finds nothing for or without the aura.
        Value::AuraInternalCooldown { unit, id } => {
            Compiled::AuraInternalCooldown((lookup.icd_aura)(unit.resolve(lookup.targets)?, id)?)
        }
        Value::AuraIcdIsReady {
            unit,
            id,
            include_reaction_time,
        } => Compiled::AuraIcdIsReady {
            aura: (lookup.icd_aura)(unit.resolve(lookup.targets)?, id)?,
            include_reaction_time: *include_reaction_time,
        },
        // Go `GetAPLAura`: no value without the aura.
        Value::AuraDuration { unit, id } => {
            let found = match unit.resolve(lookup.targets)? {
                Unit::Target(position) => (lookup.target_aura)(position, id),
                Unit::Player => aura(id),
            }?;
            Compiled::AuraDuration(found.aura)
        }
        // Go reads `GetAPLDot` on each target, keeping those it finds; none gives no value.
        Value::DotIsActiveOnAllTargets(id) | Value::DotLowestRemainingTime(id) => {
            if !(lookup.multidot)(id) {
                return None;
            }
            let dots: Vec<DotAt> = (0..lookup.targets)
                .filter_map(|position| {
                    let spell = (lookup.dot)(id, Unit::Target(position))?;
                    Some(DotAt {
                        spell,
                        target: position,
                    })
                })
                .collect();
            if dots.is_empty() {
                return None;
            }
            if matches!(value, Value::DotIsActiveOnAllTargets(_)) {
                Compiled::DotIsActiveOnAllTargets(dots)
            } else {
                Compiled::DotLowestRemainingTime(dots)
            }
        }
        Value::DotTickFrequency(dot) => Compiled::DotTickFrequency(dot_at(dot, lookup)?),
        // Go `newDotIncreaseValue`: no value for a spell without an `ExpectedTickDamage`; a
        // target that names no unit, or the player, has no dot to read.
        Value::DotPercentIncrease(dot) | Value::DotTickRatePercentIncrease(dot) => {
            let spell = (lookup.expected_tick)(dot.spell.as_ref()?)?;
            let Unit::Target(target) = dot.target.resolve(lookup.targets)? else {
                return None;
            };
            let at = DotAt { spell, target };
            if matches!(value, Value::DotPercentIncrease(_)) {
                Compiled::DotPercentIncrease(at)
            } else {
                Compiled::DotTickRatePercentIncrease(at)
            }
        }
        // Go `newItemStatProcValue`: no value without a matching buff, except the count.
        Value::AllItemStatProcsActive(procs)
        | Value::AnyItemStatProcsActive(procs)
        | Value::AnyItemStatProcsAvailable(procs)
        | Value::ItemProcsMinRemainingTime(procs)
        | Value::ItemProcsMaxRemainingIcd(procs)
        | Value::NumEquippedStatProcItems(procs) => {
            let found = (lookup.item_procs)(procs);
            if let Value::NumEquippedStatProcItems(_) = value {
                // Go `CanProc`: no custom condition of the fork's, so only a swapped out buff
                // cannot proc.
                return Some(Compiled::SettledInt(
                    found.iter().filter(|(_, swapped)| !swapped).count() as i32,
                ));
            }
            if found.is_empty() {
                return None;
            }
            match value {
                Value::AllItemStatProcsActive(_) => Compiled::AllItemStatProcsActive(found),
                Value::AnyItemStatProcsActive(_) => Compiled::AnyItemStatProcsActive(found),
                Value::AnyItemStatProcsAvailable(_) => Compiled::AnyItemStatProcsAvailable(found),
                Value::ItemProcsMinRemainingTime(_) => Compiled::ItemProcsMinRemainingTime(found),
                _ => Compiled::ItemProcsMaxRemainingIcd(found),
            }
        }
        // Go `GetMatchingStatBuffSpells` as the rotation is built: the count of those worn,
        // which no item swap changes, is a value even with none.
        Value::NumStatBuffCooldowns(stats) => {
            let count = lookup.reads.map_or(0, |reads| {
                reads
                    .cooldown_buffs
                    .iter()
                    .find(|entry| &entry.stats == stats)
                    .map_or(0, |entry| entry.spells.len())
            });
            Compiled::SettledInt(count as i32)
        }
        // Go `GetMatchingStatBuffCooldownAuras`: no value without a matching cooldown.
        Value::AnyStatBuffCooldownsActive(stats)
        | Value::AnyStatBuffCooldownsMinDuration(stats) => {
            let entry = lookup
                .reads?
                .cooldown_buffs
                .iter()
                .find(|entry| &entry.stats == stats)?;
            if entry.auras.is_empty() {
                return None;
            }
            let auras = entry
                .auras
                .iter()
                .map(|label| (lookup.aura_by_label)(label))
                .collect::<Option<Vec<R>>>()?;
            if matches!(value, Value::AnyStatBuffCooldownsActive(_)) {
                Compiled::AnyStatBuffCooldownsActive(auras)
            } else {
                Compiled::AnyStatBuffCooldownsMinDuration(auras)
            }
        }
        // Go `newValueSelectedPotion` and `newValueSelectedConjured`: the consumables' choice.
        Value::SelectedPotion(item) => Compiled::Settled(lookup.reads?.potion_id? == *item),
        Value::SelectedConjured(item) => Compiled::Settled(lookup.reads?.conjured_id? == *item),
        // Go `GetTargetSpell`: the unit's spell of the action, none when it knows none. A target
        // without an AI never hardcasts, and its spells without a cooldown are always ready.
        Value::BossSpellIsCasting { spell, unit } | Value::BossSpellTimeToReady { spell, unit } => {
            let casting = matches!(value, Value::BossSpellIsCasting { .. });
            match unit.resolve(lookup.targets)? {
                Unit::Player => {
                    let spell = (lookup.exact_spell)(spell)?;
                    if casting {
                        Compiled::PlayerSpellCasting(spell)
                    } else {
                        Compiled::SpellTimeToReady(spell)
                    }
                }
                Unit::Target(_) => {
                    lookup
                        .reads?
                        .target_spells
                        .iter()
                        .find(|known| &known.spell == spell)?;
                    if casting {
                        Compiled::Settled(false)
                    } else {
                        Compiled::SettledDuration(0)
                    }
                }
            }
        }
        // Go `newValueMultipleCdUsages`: no value without a target spell with a related self
        // buff; a base spell without one leaves only the offset.
        Value::MultipleCdUsages {
            base,
            target,
            offset,
            align_cd_end,
        } => {
            let target = (lookup.spell)(target)?;
            let target_buff = (lookup.self_buff)(target)?;
            let base = base
                .as_ref()
                .and_then(|id| (lookup.spell)(id))
                .and_then(|spell| (lookup.self_buff)(spell).map(|buff| (spell, buff)));
            let offset = offset
                .as_deref()
                .and_then(|offset| compile_value(offset, lookup))
                .map(|offset| offset.coerce(ValueType::Duration))
                .unwrap_or_else(|| Compiled::Const(parse_const("0ms").expect("duration constant")));
            Compiled::MultipleCdUsages {
                target,
                target_buff,
                base,
                offset: Box::new(offset),
                align_cd_end: *align_cd_end,
            }
        }
        // Go: the player's own current target is a target, and a target's is the player while
        // the player tanks it.
        Value::BossCurrentTarget(unit) => match unit.resolve(lookup.targets)? {
            Unit::Player => Compiled::Settled(false),
            Unit::Target(_) => Compiled::Settled(lookup.tanked),
        },
        Value::WarlockIsAssignedCurse(curse) => {
            Compiled::Settled(lookup.assigned_curse?.0 == curse.as_str())
        }
        // Go reads the curse's aura of tag 1, else of tag 0, on the unit; Rust gives no value
        // where Go would read an aura the unit lacks.
        Value::WarlockAssignedCurseIsActive(unit) => {
            let (_, curse) = lookup.assigned_curse?;
            let tagged = |tag: i32| ActionId {
                tag,
                ..curse.clone()
            };
            let find = |id: &ActionId| match unit.resolve(lookup.targets) {
                Some(Unit::Target(position)) => (lookup.target_aura)(position, id),
                Some(Unit::Player) => aura(id),
                None => None,
            };
            // Go reads the aura with `GetAuraByID`, which reports nothing of a missing one.
            let found = unreported(|| find(&tagged(1)).or_else(|| find(&tagged(0))))?;
            Compiled::AuraIsActiveAfterReaction(found.aura)
        }
        // Go finds the sequence when the rotation is finalized; one of no action reads as done
        // nothing and never ready.
        Value::SequenceIsComplete(name) => Compiled::SequenceIsComplete(name.clone()),
        Value::SequenceIsReady(name) => Compiled::SequenceIsReady(name.clone()),
        Value::SequenceTimeToReady(name) => Compiled::SequenceTimeToReady(name.clone()),
        Value::DotBaseDuration(id) => {
            Compiled::DotBaseDuration((lookup.dot_base_duration)(id.as_ref()?)?)
        }
        Value::GcdIsReady => Compiled::GcdIsReady,
        // Go `newValueActiveItemSwapSet`: no value without an item swap.
        Value::ActiveItemSwapSet(swapped) if lookup.item_swap => {
            Compiled::ActiveItemSwapSet(*swapped)
        }
        Value::ActiveItemSwapSet(_) => return None,
        // Go `APLValueVariableRef`: the value the variable resolved to. A variable whose value
        // gives none has an unknown type, which the parser does not reach.
        Value::Variable { resolved, .. } => compile_value(resolved, lookup)?,
        // A placeholder no reference filled is refused before the rotation compiles.
        Value::Placeholder(_) => return None,
        Value::ActionGroupUsed { used, .. } => Compiled::GroupUsed(*used),
        Value::AuraIsKnown(id) => bool_const(aura(id).is_some()),
        Value::PetAuraIsKnown { pet, id } => bool_const((lookup.pet_aura_known)(*pet, id)),
        // Go `newValueAuraIsActive` on a pet: a constant false for an aura it lacks (with Go's
        // warning) and for the aura of a pet never summoned, whose auras never go up.
        Value::PetAuraIsActive { pet, id } => match (lookup.pet_aura)(*pet, id) {
            Some(found) => Compiled::AuraIsActive(found.aura),
            None => {
                (lookup.pet_aura_known)(*pet, id);
                bool_const(false)
            }
        },
        Value::AuraIsActive(id) => match aura(id) {
            Some(found) => Compiled::AuraIsActive(found.aura),
            None => bool_const(false),
        },
        // Go `GetAPLAura` on a unit: one that resolves to no unit, or lacks the aura, reads as
        // an aura it does not have. Go's `newValueAuraIsActive` and `newValueAuraIsInactive`
        // make a constant of such an aura, whatever the reaction time.
        Value::TargetAura { read, unit, id } => {
            let found = match unit.resolve(lookup.targets) {
                Some(Unit::Target(position)) => (lookup.target_aura)(position, id),
                Some(Unit::Player) => aura(id),
                None => None,
            };
            match (read, found) {
                (AuraRead::IsKnown, found) => bool_const(found.is_some()),
                (AuraRead::IsActive, Some(found)) => Compiled::AuraIsActive(found.aura),
                (AuraRead::IsActive, None) => bool_const(false),
                (AuraRead::IsActiveAfterReaction, Some(found)) => {
                    Compiled::AuraIsActiveAfterReaction(found.aura)
                }
                (AuraRead::IsActiveAfterReaction, None) => bool_const(false),
                (AuraRead::IsInactiveAfterReaction, Some(found)) => {
                    Compiled::AuraIsInactiveAfterReaction(found.aura)
                }
                (AuraRead::IsInactiveAfterReaction, None) => bool_const(true),
                // Go `newValueAuraNumStacks` warns that the aura does not stack and drops the
                // value, with or without the reaction time.
                (AuraRead::NumStacksAfterReaction, Some(found)) if found.max_stacks == 0 => {
                    return None
                }
                (AuraRead::NumStacksAfterReaction, Some(found)) => {
                    Compiled::AuraNumStacksAfterReaction(found.aura)
                }
                (AuraRead::NumStacksAfterReaction, None) => {
                    Compiled::Const(parse_const("0").expect("int constant"))
                }
                // Go warns that the aura does not stack and drops the value.
                (AuraRead::NumStacks, Some(found)) if found.max_stacks == 0 => return None,
                (AuraRead::NumStacks, Some(found)) => Compiled::AuraNumStacks(found.aura),
                (AuraRead::NumStacks, None) => {
                    Compiled::Const(parse_const("0").expect("int constant"))
                }
                (AuraRead::RemainingTime, Some(found)) => Compiled::AuraRemainingTime(found.aura),
                (AuraRead::RemainingTime, None) => {
                    Compiled::Const(parse_const("0ms").expect("duration constant"))
                }
            }
        }
        Value::AuraNumStacks(id) => match aura(id) {
            // Go warns that the aura does not stack and drops the value.
            Some(found) if found.max_stacks == 0 => return None,
            Some(found) => Compiled::AuraNumStacks(found.aura),
            None => Compiled::Const(parse_const("0").expect("int constant")),
        },
        Value::Compare { op, lhs, rhs } => {
            // Go builds both operands before it looks at either, so each reports what it lacks.
            let (lhs, rhs) = (compile_value(lhs, lookup), compile_value(rhs, lookup));
            let (lhs, rhs) = (lhs?, rhs?);
            let to = lhs.value_type().max(rhs.value_type());
            // Go `newValueCompare` warns that booleans only compare for equality and gives
            // no value, so the comparison drops out of its parent.
            if to == ValueType::Bool && !matches!(op, CompareOp::Eq | CompareOp::Ne) {
                return None;
            }
            Compiled::Compare {
                op: *op,
                lhs: Box::new(lhs.coerce(to)),
                rhs: Box::new(rhs.coerce(to)),
            }
        }
        Value::Math { op, lhs, rhs } => {
            let (lhs, rhs) = (compile_value(lhs, lookup), compile_value(rhs, lookup));
            let (lhs, rhs) = (lhs?, rhs?);
            let (lhs, rhs) = match op {
                MathOp::Add | MathOp::Sub => {
                    let to = lhs.value_type().max(rhs.value_type());
                    (lhs.coerce(to), rhs.coerce(to))
                }
                MathOp::Mul | MathOp::Div => (lhs, rhs),
            };
            let (lhs_type, rhs_type) = (lhs.value_type(), rhs.value_type());
            let numeric = |t: ValueType| matches!(t, ValueType::Int | ValueType::Float);
            // Go newValueMath warns and gives no value for these.
            if matches!(lhs_type, ValueType::Bool | ValueType::String)
                || matches!(rhs_type, ValueType::Bool | ValueType::String)
                || (*op == MathOp::Mul
                    && lhs_type == ValueType::Duration
                    && rhs_type == ValueType::Duration)
                || (*op == MathOp::Div && numeric(lhs_type) && rhs_type == ValueType::Duration)
            {
                return None;
            }
            Compiled::Math {
                op: *op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            }
        }
        Value::AuraRemainingTime(id) => match aura(id) {
            Some(found) => Compiled::AuraRemainingTime(found.aura),
            None => Compiled::Const(parse_const("0ms").expect("duration constant")),
        },
        // Go `newValueNot` folds a constant operand.
        Value::Not(value) => {
            let value = compile_value(value, lookup)?.coerce(ValueType::Bool);
            match value.const_bool() {
                Some(constant) => bool_const(!constant),
                None => Compiled::Not(Box::new(value)),
            }
        }
        Value::And(values) => return fold(values, lookup, false, Compiled::And),
        Value::Or(values) => return fold(values, lookup, true, Compiled::Or),
    })
}

/// Go `newAPLValue` without a coercion, as `newActionMove` and `newActionMoveDuration` read
/// their values: `None` when the value gives none.
pub fn compile_raw_value<R>(value: &Value, lookup: &Lookup<R>) -> Option<Compiled<R>> {
    compile_value(value, lookup)
}

/// Go `coerceTo(newAPLValue(value), Bool)`, as `newActionChannelSpell` compiles its
/// interrupt condition: `None` when the value has none.
pub fn compile_bool_value<R>(value: Option<&Value>, lookup: &Lookup<R>) -> Option<Compiled<R>> {
    value
        .and_then(|value| compile_value(value, lookup))
        .map(|value| value.coerce(ValueType::Bool))
}

/// Go `coerceTo(newAPLValue(value), Duration)`, as `newActionMultidot` compiles its overlap:
/// `None` when the value has none.
pub fn compile_duration_value<R>(value: Option<&Value>, lookup: &Lookup<R>) -> Option<Compiled<R>> {
    value
        .and_then(|value| compile_value(value, lookup))
        .map(|value| value.coerce(ValueType::Duration))
}

/// Go `newAPLAction`'s condition handling for one action.
pub fn compile_condition<R>(condition: Option<&Value>, lookup: &Lookup<R>) -> CompiledCondition<R> {
    let compiled = condition
        .and_then(|value| compile_value(value, lookup))
        .map(|value| value.coerce(ValueType::Bool));
    match compiled {
        None => CompiledCondition::Always,
        Some(value) => match value.const_bool() {
            Some(false) => CompiledCondition::Pruned,
            Some(true) => CompiledCondition::Always,
            None => CompiledCondition::When(value),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_spell(_: &ActionId) -> Option<usize> {
        None
    }

    fn no_dot(_: &ActionId, _: Unit) -> Option<usize> {
        None
    }

    fn only_auras<'a, R>(
        aura: &'a dyn Fn(&ActionId) -> Option<FoundAura<R>>,
        target_aura: &'a dyn Fn(usize, &ActionId) -> Option<FoundAura<R>>,
    ) -> Lookup<'a, R> {
        Lookup {
            aura,
            target_aura,
            targets: 1,
            spell: &no_spell,
            dot: &no_dot,
            dot_base_duration: &|_| None,
            pet_aura_known: &|_, _| false,
            pet_aura: &|_, _| None,
            simulated_pet_with_mana: &|_| None,
            unsummoned_pet_with_mana: &|_| false,
            unsummoned_pet_max_mana: &|_| None,
            item_swap: false,
            bars: Bars::default(),
            icd_aura: &|_, _| None,
            multidot: &|_| false,
            assigned_curse: None,
            tanked: false,
            reads: None,
            exact_spell: &|_| None,
            self_buff: &|_| None,
            item_procs: &|_| Vec::new(),
            aura_by_label: &|_| None,
            expected_tick: &|_| None,
        }
    }

    /// Go `GetSourceUnit` on a pet of the player reads that pet's auras, a constant.
    #[test]
    fn aura_is_known_reads_a_pet_of_the_player() {
        let pet = serde_json::json!({"auraIsKnown": {
            "auraId": {"spellId": 1293696},
            "sourceUnit": {"type": "Pet", "index": 1, "owner": {"type": "Self"}}
        }});
        let value = parse_value(&pet).unwrap();
        let id = ActionId {
            spell_id: 1293696,
            ..ActionId::default()
        };
        assert_eq!(
            value,
            Value::PetAuraIsKnown {
                pet: 1,
                id: id.clone()
            }
        );
        let none = |_: &ActionId| -> Option<FoundAura<ActionId>> { None };
        let known = |pet: usize, aura: &ActionId| pet == 1 && *aura == id;
        let lookup = Lookup {
            pet_aura_known: &known,
            pet_aura: &|_, _| None,
            simulated_pet_with_mana: &|_| None,
            unsummoned_pet_with_mana: &|_| false,
            unsummoned_pet_max_mana: &|_| None,
            item_swap: false,
            bars: Bars::default(),
            icd_aura: &|_, _| None,
            multidot: &|_| false,
            assigned_curse: None,
            tanked: false,
            reads: None,
            exact_spell: &|_| None,
            self_buff: &|_| None,
            item_procs: &|_| Vec::new(),
            aura_by_label: &|_| None,
            expected_tick: &|_| None,
            ..only_auras(&none, &|_, _| None)
        };
        assert_eq!(
            compile_condition(Some(&value), &lookup),
            CompiledCondition::Always
        );
        let other = serde_json::json!({"auraIsKnown": {
            "auraId": {"spellId": 1293696},
            "sourceUnit": {"type": "Pet", "index": 0, "owner": {"type": "CurrentTarget"}}
        }});
        assert!(parse_value(&other).is_err());
    }

    /// Go `GetSourceUnit` on a pet of the player: `auraIsActive` reads the aura of a pet the fight
    /// simulates, as a Demonology rotation asks whether its Succubus carries Demonic Brand, and
    /// is never true for a pet that is never summoned or lacks the aura; `auraIsInactive` is its
    /// negation.
    #[test]
    fn aura_is_active_reads_a_simulated_pet_of_the_player() {
        let id = ActionId {
            spell_id: 1293696,
            ..ActionId::default()
        };
        let read = |name: &str| {
            parse_value(&serde_json::json!({name: {
                "auraId": {"spellId": 1293696},
                "sourceUnit": {"type": "Pet", "index": 2, "owner": {"type": "Self"}}
            }}))
            .unwrap()
        };
        let active = read("auraIsActive");
        assert_eq!(
            active,
            Value::PetAuraIsActive {
                pet: 2,
                id: id.clone()
            }
        );
        assert_eq!(read("auraIsInactive"), Value::Not(Box::new(active.clone())));
        let none = |_: &ActionId| -> Option<FoundAura<ActionId>> { None };
        let simulated = |pet: usize, aura: &ActionId| {
            (pet == 2 && *aura == id).then(|| FoundAura {
                aura: aura.clone(),
                max_stacks: 0,
            })
        };
        let lookup = Lookup {
            pet_aura: &simulated,
            ..only_auras(&none, &|_, _| None)
        };
        assert_eq!(
            compile_condition(Some(&active), &lookup),
            CompiledCondition::When(Compiled::AuraIsActive(id.clone()))
        );
        let inert = only_auras(&none, &|_, _| None);
        assert!(compile_condition(Some(&active), &inert).never_holds());
    }

    /// Go `time.Duration.String`, which a duration compared with a string constant reads.
    #[test]
    fn durations_format_as_go_prints_them() {
        for (ns, text) in [
            (0, "0s"),
            (2, "2ns"),
            (1_234, "1.234\u{b5}s"),
            (500_000_000, "500ms"),
            (1_500_000_000, "1.5s"),
            (60_000_000_000, "1m0s"),
            (3_723_500_000_000, "1h2m3.5s"),
            (-1_500_000_000, "-1.5s"),
        ] {
            assert_eq!(go_duration_string(ns), text);
        }
        assert_eq!(go_float_string(0.5), "0.500");
    }

    /// Go keeps a constant it cannot read as a number, as "1,5s" with a comma, as a string, and
    /// compares the other side with it as text.
    #[test]
    fn a_comma_decimal_is_a_string_constant() {
        let constant = parse_const("1,5s").unwrap();
        assert_eq!(constant.value_type, ValueType::String);
        assert_eq!(constant.string, "1,5s");
        assert!(constant.boolean);
        assert_eq!(
            (constant.int, constant.float, constant.duration_ns),
            (0, 0.0, 0)
        );
        // Text order: "1.2s" sorts after "1,5s", "0s" before it.
        assert!(go_duration_string(1_200_000_000).as_str() > "1,5s");
        assert!(go_duration_string(0).as_str() < "1,5s");
    }

    /// Go `GetAPLSpell(nil)` and an aura value without an aura: spellIsKnown is a constant false,
    /// spellIsReady and auraNumStacks give no value.
    #[test]
    fn values_without_their_spell_or_aura() {
        let parse = |json: serde_json::Value| parse_value(&json).unwrap();
        assert_eq!(
            parse(serde_json::json!({"spellIsReady": {}})),
            Value::Unbuilt("spellIsReady")
        );
        assert_eq!(
            parse(serde_json::json!({"auraNumStacks": {}})),
            Value::Unbuilt("auraNumStacks")
        );
        assert!(matches!(
            parse(serde_json::json!({"spellIsKnown": {}})),
            Value::Const(Const {
                value_type: ValueType::Bool,
                boolean: false,
                ..
            })
        ));
    }

    /// Go `newValueCurrentMana` on a pet of the player, the first without an index: the mana of
    /// a pet the fight simulates (a Demonology rotation's Dark Pact line), no value otherwise.
    /// Energy values give no value for a player without an energy bar.
    #[test]
    fn pet_mana_and_missing_energy() {
        let mana = parse_value(&serde_json::json!({"currentMana": {
            "sourceUnit": {"type": "Pet", "owner": {"type": "Self"}}
        }}))
        .unwrap();
        assert_eq!(mana, Value::PetCurrentMana(0));
        let none = |_: &ActionId| -> Option<FoundAura<ActionId>> { None };
        let summoned = Lookup {
            simulated_pet_with_mana: &|pet| (pet == 0).then_some(1),
            unsummoned_pet_with_mana: &|_| false,
            unsummoned_pet_max_mana: &|_| None,
            ..only_auras(&none, &|_, _| None)
        };
        assert_eq!(
            compile_value(&mana, &summoned),
            Some(Compiled::PetCurrentMana(1))
        );
        assert_eq!(compile_value(&mana, &only_auras(&none, &|_, _| None)), None);
        let energy = parse_value(&serde_json::json!({"currentEnergy": {}})).unwrap();
        assert_eq!(
            compile_value(&energy, &only_auras(&none, &|_, _| None)),
            None
        );
    }

    /// Go `newValueMin` returns a single value as itself, so a `min` of one boolean is that
    /// boolean; two booleans still reach Go's `Min`, which panics reading them.
    #[test]
    fn a_min_of_one_boolean_is_the_boolean() {
        let one = serde_json::json!({"min": {"vals": [{"const": {"val": "true"}}]}});
        assert!(matches!(parse_value(&one).unwrap(), Value::Min(values) if values.len() == 1));
        let two = serde_json::json!({"min": {"vals": [
            {"const": {"val": "true"}}, {"const": {"val": "false"}}
        ]}});
        assert!(parse_value(&two).is_err());
    }

    /// Go builds each strict sequence step as an action of its own, so a step may carry a
    /// condition, which its readiness checks.
    #[test]
    fn a_strict_sequence_step_keeps_its_condition() {
        let rotation = serde_json::json!({"type": "TypeAPL", "priorityList": [{"action": {
            "strictSequence": {"actions": [
                {"castSpell": {"spellId": {"spellId": 1311595}},
                 "condition": {"cmp": {"lhs": {"currentManaPercent": {}}, "op": "OpGt",
                                        "rhs": {"const": {"val": "40%"}}}}},
                {"castSpell": {"spellId": {"spellId": 20930}}}
            ]}
        }}]});
        let parsed = parse(&rotation).unwrap();
        let Action::StrictSequence(steps) = &parsed.priority_list[0].action else {
            panic!("a strict sequence");
        };
        assert!(steps[0].condition.is_some());
        assert!(steps[1].condition.is_none());
        assert_eq!(parsed.priority_list[0].action.values().len(), 1);
    }

    /// Go `APLValueAutoSwingTime` and `APLValueSpellCurrentCost`, read by the upstream Rogue
    /// and Feral presets.
    #[test]
    fn swing_time_and_current_cost_parse() {
        let swing = serde_json::json!({"autoSwingTime": {"autoType": "MainHand"}});
        assert_eq!(
            parse_value(&swing),
            Ok(Value::AutoSwingTime(SwingType::MainHand))
        );
        assert_eq!(
            Value::AutoSwingTime(SwingType::MainHand).value_type(),
            ValueType::Duration
        );
        let unknown = serde_json::json!({"autoSwingTime": {}});
        assert_eq!(
            parse_value(&unknown),
            Ok(Value::AutoSwingTime(SwingType::Unknown))
        );
        let cost = serde_json::json!({"spellCurrentCost": {"spellId": {"spellId": 9830}}});
        let id = ActionId {
            spell_id: 9830,
            ..ActionId::default()
        };
        assert_eq!(parse_value(&cost), Ok(Value::SpellCurrentCost(id.clone())));
        assert_eq!(Value::SpellCurrentCost(id).value_type(), ValueType::Float);
    }

    /// Go `APLActionMultidot` in the priority list and `APLActionActivateAura` among the
    /// prepull actions, as the upstream Balance presets use them.
    #[test]
    fn multidot_and_prepull_aura_activation_parse() {
        let rotation = serde_json::json!({
            "type": "TypeAPL",
            "prepullActions": [{"action": {"activateAura": {"auraId": {"spellId": 24858}}},
                                "doAtValue": {"const": {"val": "-10s"}}}],
            "priorityList": [{"action": {"multidot": {"spellId": {"spellId": 9835}, "maxDots": 3,
                                                      "maxOverlap": {"const": {"val": "0ms"}}}}}]
        });
        let parsed = parse(&rotation).unwrap();
        let id = |spell_id| ActionId {
            spell_id,
            ..ActionId::default()
        };
        assert_eq!(parsed.prepull[0].action, Action::ActivateAura(id(24858)));
        let Action::Multidot {
            spell, max_dots, ..
        } = &parsed.priority_list[0].action
        else {
            panic!("expected a multidot");
        };
        assert_eq!((spell, *max_dots), (&id(9835), 3));
        assert_eq!(parsed.priority_list[0].action.spells(), vec![&id(9835)]);
        let item = serde_json::json!({"action": {"activateAura": {"auraId": {"spellId": 24858}}}});
        assert!(parse_item(&item, 1).is_err());
    }

    /// Go `APLActionMove` reads its range as the float of the value: a constant holds one, and
    /// a duration constant holds none, so it moves to zero yards. A value with no float getter
    /// panics in Go and is unsupported.
    #[test]
    fn a_move_parses_its_range_value() {
        let prepull = |range: serde_json::Value| {
            serde_json::json!({
                "type": "TypeAPL",
                "prepullActions": [{"action": {"move": {"rangeFromTarget": range}},
                                    "doAtValue": {"const": {"val": "-5s"}}}]
            })
        };
        for (range, yards) in [("25", 25.0), ("7.5", 7.5), ("30s", 0.0), ("true", 0.0)] {
            let parsed = parse(&prepull(serde_json::json!({"const": {"val": range}}))).unwrap();
            let Action::Move(Value::Const(constant)) = &parsed.prepull[0].action else {
                panic!("a constant range");
            };
            assert_eq!(constant.float, yards, "{range}");
            assert_eq!(parsed.prepull[0].do_at_ns, -5_000_000_000);
            assert!(parsed.prepull[0].action.spells().is_empty());
        }
        // A prepull move reads a constant only, and a value with no float getter is no range.
        let variable = prepull(serde_json::json!({"currentRage": {}}));
        assert_eq!(
            parse(&variable).unwrap_err(),
            ["prepull action 1: a prepull move other than to a constant is unsupported"]
        );
        let stacks = prepull(serde_json::json!({"auraNumStacks": {"auraId": {"spellId": 7}}}));
        assert_eq!(
            parse(&stacks).unwrap_err(),
            ["prepull action 1: a move range Go cannot read as a float is unsupported"]
        );
        let item = serde_json::json!({"action": {"moveDuration": {
            "duration": {"const": {"val": "2s"}}}}, "doAtValue": {"const": {"val": "-3s"}}});
        let parsed = parse(&serde_json::json!({"type": "TypeAPL", "prepullActions": [item]}));
        assert!(matches!(
            parsed.unwrap().prepull[0].action,
            Action::MoveDuration(Value::Const(_))
        ));
        // The priority list holds moves too, which run once ready.
        let item =
            serde_json::json!({"action": {"move": {"rangeFromTarget": {"const": {"val": "5"}}}}});
        let parsed = parse_item(&item, 1).unwrap().unwrap();
        assert!(matches!(parsed.action, Action::Move(Value::Const(_))));
        let variable = serde_json::json!({"action": {"move": {
            "rangeFromTarget": {"remainingTime": {}}}}});
        assert_eq!(
            parse_item(&variable, 1).unwrap_err(),
            ["a move range Go cannot read as a float is unsupported"]
        );
        // A move for a duration reads a duration.
        let item = |duration: serde_json::Value| serde_json::json!({"action": {"moveDuration": {"duration": duration}}});
        let parsed = parse_item(&item(serde_json::json!({"const": {"val": "2s"}})), 1);
        assert!(matches!(
            parsed.unwrap().unwrap().action,
            Action::MoveDuration(Value::Const(_))
        ));
        let parsed = parse_item(&item(serde_json::json!({"remainingTime": {}})), 1);
        assert!(matches!(
            parsed.unwrap().unwrap().action,
            Action::MoveDuration(Value::RemainingTime)
        ));
        assert!(parse_item(&item(serde_json::json!({"currentRage": {}})), 1).is_err());
    }

    #[test]
    fn duration_parser_matches_go_examples() {
        assert_eq!(parse_go_duration("25s"), Some(25_000_000_000));
        assert_eq!(parse_go_duration("1.5s"), Some(1_500_000_000));
        assert_eq!(parse_go_duration("1m30s"), Some(90_000_000_000));
        assert_eq!(parse_go_duration("100ms"), Some(100_000_000));
        assert_eq!(parse_go_duration("-2h"), Some(-7_200_000_000_000));
        assert_eq!(parse_go_duration("0"), Some(0));
        assert_eq!(parse_go_duration(".5s"), Some(500_000_000));
        assert_eq!(parse_go_duration("25"), None);
        assert_eq!(parse_go_duration("80%"), None);
        assert_eq!(parse_go_duration("s"), None);
    }

    #[test]
    fn constants_keep_every_go_representation() {
        let percent = parse_const("80%").unwrap();
        assert_eq!(percent.value_type, ValueType::Float);
        assert_eq!(percent.float, 0.8);
        assert_eq!(percent.duration_ns, 800_000_000);
        let seconds = parse_const("25s").unwrap();
        assert_eq!(seconds.value_type, ValueType::Duration);
        assert_eq!((seconds.duration_ns, seconds.float), (25_000_000_000, 0.0));
        let int = parse_const("25").unwrap();
        assert_eq!(int.value_type, ValueType::Int);
        assert_eq!(
            (int.int, int.float, int.duration_ns),
            (25, 25.0, 25_000_000_000)
        );
        assert_eq!(parse_const("TRUE").unwrap().value_type, ValueType::Bool);
        assert_eq!(parse_const("hello").unwrap().value_type, ValueType::String);
    }

    #[test]
    fn aura_is_active_reads_the_player_or_the_current_target() {
        let item = |source: serde_json::Value| {
            serde_json::json!({"action": {"castSpell": {"spellId": {"spellId": 1}},
                "condition": {"auraIsActive": {"auraId": {"spellId": 2}, "sourceUnit": source}}}})
        };
        let rotation = serde_json::json!({"type": "TypeAPL", "priorityList": [
            item(serde_json::json!({"type": "Self"})),
            item(serde_json::json!({"type": "CurrentTarget"})),
        ]});
        let parsed = parse(&rotation).unwrap();
        let id = ActionId {
            spell_id: 2,
            ..ActionId::default()
        };
        assert_eq!(
            parsed.priority_list[0].condition,
            Some(Value::AuraIsActive(id.clone()))
        );
        assert_eq!(
            parsed.priority_list[1].condition,
            Some(Value::TargetAura {
                read: AuraRead::IsActive,
                unit: UnitRef::CurrentTarget,
                id
            })
        );
        // A player past the raid's one is no unit, and a reference to a unit Go ignores the
        // index of is that unit.
        for (source, unit) in [
            (
                serde_json::json!({"type": "Player", "index": 1}),
                UnitRef::Nobody,
            ),
            (
                serde_json::json!({"type": "CurrentTarget", "index": 1}),
                UnitRef::CurrentTarget,
            ),
        ] {
            let rotation =
                serde_json::json!({"type": "TypeAPL", "priorityList": [item(source.clone())]});
            assert!(matches!(
                parse(&rotation).unwrap().priority_list[0].condition,
                Some(Value::TargetAura { unit: found, .. }) if found == unit
            ));
        }
        // A pet of the player reads that pet (`aura_is_active_reads_a_simulated_pet_of_the_player`);
        // a pet of another unit is unsupported.
        let source = serde_json::json!({"type": "Pet", "owner": {"type": "CurrentTarget"}});
        let rotation =
            serde_json::json!({"type": "TypeAPL", "priorityList": [item(source.clone())]});
        assert_eq!(
            parse(&rotation).unwrap_err(),
            [format!(
                "rotation item 1: auraIsActive sourceUnit {source} is unsupported"
            )]
        );
    }

    #[test]
    fn math_types_follow_go() {
        let math = |op: &str, lhs: serde_json::Value, rhs: serde_json::Value| {
            parse_value(&serde_json::json!({"math": {"op": op, "lhs": lhs, "rhs": rhs}}))
        };
        let remaining = || serde_json::json!({"remainingTime": {}});
        let mana = || serde_json::json!({"currentMana": {}});
        let constant = |val: &str| serde_json::json!({"const": {"val": val}});
        let typed = |value: Result<Value, Vec<String>>| value.unwrap().value_type();
        // A duration times an integer stays a duration; a sum takes the higher type.
        assert_eq!(
            typed(math("OpMul", remaining(), constant("100"))),
            ValueType::Duration
        );
        assert_eq!(
            typed(math("OpAdd", constant("1"), mana())),
            ValueType::Float
        );
        assert_eq!(
            typed(math("OpDiv", remaining(), remaining())),
            ValueType::Float
        );
        assert_eq!(
            typed(math("OpDiv", remaining(), constant("2"))),
            ValueType::Duration
        );
        assert_eq!(
            typed(math("OpMul", constant("2"), constant("3"))),
            ValueType::Int
        );
        // A duration over a float is a float, which Go reads from the duration and panics.
        assert!(math("OpDiv", remaining(), constant("2.5")).is_err());
        assert!(math("OpMul", serde_json::json!({"numberTargets": {}}), mana()).is_err());
        assert!(math("OpMod", mana(), mana()).is_err());
    }

    #[test]
    fn unsupported_operators_are_named() {
        let rotation = serde_json::json!({
            "type": "TypeAPL",
            "priorityList": [
                {"action": {"castSpell": {"spellId": {"spellId": 25304}},
                            "condition": {"notAValue": {"spellId": {"spellId": 1}}}}},
                {"action": {"customRotation": {}}}
            ]
        });
        let reasons = parse(&rotation).unwrap_err();
        assert_eq!(
            reasons,
            [
                "rotation item 1: value notAValue is unsupported",
                "rotation item 2: action customRotation is unsupported"
            ]
        );
    }

    /// The condition over spell 44404, an aura the test character lacks.
    fn condition(json: serde_json::Value) -> CompiledCondition<()> {
        let rotation = parse(&serde_json::json!({
            "type": "TypeAPL",
            "priorityList": [{"action": {
                "castSpell": {"spellId": {"spellId": 25345}},
                "condition": json,
            }}],
        }))
        .unwrap();
        let condition = rotation.priority_list[0].condition.as_ref();
        let lacks = |_: &ActionId| None::<FoundAura<()>>;
        compile_condition(condition, &only_auras(&lacks, &|_, _| None))
    }

    #[test]
    fn missing_auras_read_as_inactive_as_community_622() {
        let active = serde_json::json!({"auraIsActive": {"auraId": {"spellId": 44404}}});
        let known = serde_json::json!({"auraIsKnown": {"auraId": {"spellId": 44404}}});
        let low_mana = serde_json::json!({"cmp": {
            "op": "OpLt", "lhs": {"currentManaPercent": {}}, "rhs": {"const": {"val": "20%"}},
        }});
        // Unguarded, the action never fires; the pinned reference before #622 dropped the
        // term and fired it on every pass.
        assert_eq!(condition(active.clone()), CompiledCondition::Pruned);
        // An auraIsKnown guard prunes the action too.
        assert_eq!(
            condition(serde_json::json!({"and": {"vals": [known, active.clone()]}})),
            CompiledCondition::Pruned
        );
        // The constant false decides an And.
        assert_eq!(
            condition(serde_json::json!({"and": {"vals": [low_mana, active.clone()]}})),
            CompiledCondition::Pruned
        );
        // Go does not fold comparisons, so a live constant comparison remains.
        assert!(matches!(
            condition(serde_json::json!({"cmp": {
                "op": "OpEq", "lhs": active, "rhs": {"const": {"val": "false"}},
            }})),
            CompiledCondition::When(Compiled::Compare { .. })
        ));
    }

    #[test]
    fn or_and_stack_counts_fold_as_go_does() {
        let compile = |json: serde_json::Value, max_stacks: i32| {
            let rotation = parse(&serde_json::json!({
                "type": "TypeAPL",
                "priorityList": [{"action": {
                    "castSpell": {"spellId": {"spellId": 25304}},
                    "condition": json,
                }}],
            }))
            .unwrap();
            let condition = rotation.priority_list[0].condition.clone();
            // The character has 400573 and lacks 44404.
            let find = move |id: &ActionId| {
                (id.spell_id == 400573).then_some(FoundAura {
                    aura: (),
                    max_stacks,
                })
            };
            compile_condition(condition.as_ref(), &only_auras(&find, &|_, id| find(id)))
        };
        let stacks = |id: i32| {
            serde_json::json!({"cmp": {"op": "OpGe",
                "lhs": {"auraNumStacks": {"auraId": {"spellId": id}}}, "rhs": {"const": {"val": "3"}}}})
        };
        let known = serde_json::json!({"auraIsKnown": {"auraId": {"spellId": 400573}}});
        // A known aura's stacks compare as integers.
        assert!(matches!(
            compile(stacks(400573), 4),
            CompiledCondition::When(Compiled::Compare { .. })
        ));
        // Go drops the stacks of an aura without MaxStacks.
        assert_eq!(compile(stacks(400573), 0), CompiledCondition::Always);
        // A missing aura compares a constant 0, which never reaches 3.
        let missing = compile(stacks(44404), 4);
        assert!(matches!(
            missing,
            CompiledCondition::When(Compiled::Compare { .. })
        ));
        assert!(missing.never_holds());
        // A constant true decides an Or.
        assert_eq!(
            compile(
                serde_json::json!({"or": {"vals": [stacks(400573), known]}}),
                4,
            ),
            CompiledCondition::Always
        );
        // Without the stacking aura, the constant 0 >= 3 beside the mana term means the mana
        // term alone.
        let low_mana = serde_json::json!({"cmp": {
            "op": "OpLt", "lhs": {"currentManaPercent": {}}, "rhs": {"const": {"val": "40%"}},
        }});
        let with_missing = compile(
            serde_json::json!({"or": {"vals": [stacks(44404), low_mana.clone()]}}),
            4,
        );
        assert_ne!(with_missing, compile(low_mana.clone(), 4));
        assert!(with_missing.same_meaning(&compile(low_mana, 4)));
    }

    #[test]
    fn not_and_remaining_time_fold_as_go_does() {
        let compile = |json: serde_json::Value| {
            let rotation = parse(&serde_json::json!({
                "type": "TypeAPL",
                "priorityList": [{"action": {
                    "castSpell": {"spellId": {"spellId": 10207}},
                    "condition": json,
                }}],
            }))
            .unwrap();
            let condition = rotation.priority_list[0].condition.clone();
            // The character has 22959 and lacks 400625.
            let find = |id: &ActionId| {
                (id.spell_id == 22959).then_some(FoundAura {
                    aura: (),
                    max_stacks: 5,
                })
            };
            compile_condition(condition.as_ref(), &only_auras(&find, &|_, id| find(id)))
        };
        let known = |id: i32| serde_json::json!({"auraIsKnown": {"auraId": {"spellId": id}}});
        // Go folds Not of a constant.
        assert_eq!(
            compile(serde_json::json!({"not": {"val": known(22959)}})),
            CompiledCondition::Pruned
        );
        assert_eq!(
            compile(serde_json::json!({"not": {"val": known(400625)}})),
            CompiledCondition::Always
        );
        // Remaining time compares as a duration; a missing aura has none left.
        let remaining = |id: i32| {
            serde_json::json!({"cmp": {"op": "OpLe",
                "lhs": {"auraRemainingTime": {"auraId": {"spellId": id}}}, "rhs": {"const": {"val": "4s"}}}})
        };
        assert!(matches!(
            compile(remaining(22959)),
            CompiledCondition::When(Compiled::Compare { .. })
        ));
        // 0 <= 4s always holds.
        assert!(compile(remaining(400625)).same_meaning(&CompiledCondition::Always));
    }

    #[test]
    fn sequences_and_channels_parse_with_their_values() {
        let rotation = parse(&serde_json::json!({
            "type": "TypeAPL",
            "priorityList": [
                {"action": {"strictSequence": {"actions": [
                    {"castSpell": {"spellId": {"spellId": 14751}}},
                    {"castSpell": {"spellId": {"spellId": 10947, "rank": 9}}},
                ]}}},
                {"action": {"channelSpell": {
                    "spellId": {"spellId": 18807},
                    "interruptIf": {"and": {"vals": [
                        {"cmp": {"op": "OpLe",
                            "lhs": {"spellTimeToReady": {"spellId": {"spellId": 10947}}},
                            "rhs": {"const": {"val": "0s"}}}},
                        {"cmp": {"op": "OpLe",
                            "lhs": {"dotTimeToNextTick": {"spellId": {"spellId": 18807}}},
                            "rhs": {"const": {"val": "0.05s"}}}},
                        {"gcdIsReady": {}},
                    ]}},
                    "allowRecast": true,
                }}},
            ],
        }))
        .unwrap();
        assert_eq!(
            rotation.priority_list[0].action,
            Action::StrictSequence(vec![
                Step {
                    spell: ActionId::spell(14751),
                    target: UnitRef::CurrentTarget,
                    condition: None,
                    change_target: false,
                },
                Step {
                    spell: ActionId::spell(10947),
                    target: UnitRef::CurrentTarget,
                    condition: None,
                    change_target: false,
                }
            ])
        );
        let Action::ChannelSpell {
            spell,
            target: UnitRef::CurrentTarget,
            interrupt_if: Some(Value::And(terms)),
            allow_recast: true,
        } = &rotation.priority_list[1].action
        else {
            panic!("expected an interruptible channel");
        };
        assert_eq!(*spell, ActionId::spell(18807));
        assert_eq!(terms[2], Value::GcdIsReady);
        let types: Vec<ValueType> = terms
            .iter()
            .map(|term| match term {
                Value::Compare { lhs, .. } => lhs.value_type(),
                other => other.value_type(),
            })
            .collect();
        assert_eq!(
            types,
            [ValueType::Duration, ValueType::Duration, ValueType::Bool]
        );
    }

    #[test]
    fn a_named_sequence_and_a_target_aura_remaining_time_parse() {
        let rotation = parse(&serde_json::json!({
            "type": "TypeAPL",
            "priorityList": [
                {"action": {
                    "condition": {"cmp": {"op": "OpLe",
                        "lhs": {"auraRemainingTime": {"auraId": {"spellId": 11581},
                            "sourceUnit": {"type": "CurrentTarget"}}},
                        "rhs": {"const": {"val": "1.5s"}}}},
                    "sequence": {"name": "Stance into Reck", "actions": [
                        {"castSpell": {"spellId": {"spellId": 2458}}},
                        {"castSpell": {"spellId": {"spellId": 1719}}},
                    ]},
                }},
            ],
        }))
        .unwrap();
        let item = &rotation.priority_list[0];
        assert_eq!(
            item.action,
            Action::Sequence {
                name: "Stance into Reck".to_string(),
                steps: vec![
                    Step {
                        spell: ActionId::spell(2458),
                        target: UnitRef::CurrentTarget,
                        condition: None,
                        change_target: false,
                    },
                    Step {
                        spell: ActionId::spell(1719),
                        target: UnitRef::CurrentTarget,
                        condition: None,
                        change_target: false,
                    }
                ]
            }
        );
        let Some(Value::Compare { lhs, .. }) = &item.condition else {
            panic!("expected a comparison");
        };
        assert_eq!(
            **lhs,
            Value::TargetAura {
                read: AuraRead::RemainingTime,
                unit: UnitRef::CurrentTarget,
                id: ActionId::spell(11581)
            }
        );
    }

    /// Go `newActionCastFriendlySpell`: the first player or the unit itself is the player, no
    /// target is the current target, and any other unit is not modeled.
    #[test]
    fn a_friendly_cast_names_the_player_or_the_current_target() {
        let cast = |target: Option<serde_json::Value>| {
            let mut config = serde_json::json!({"spellId": {"spellId": 25292}});
            if let Some(target) = target {
                config["target"] = target;
            }
            parse(&serde_json::json!({
                "type": "TypeAPL",
                "priorityList": [{"action": {
                    "condition": {"cmp": {"op": "OpLt", "lhs": {"currentHealthPercent": {}},
                        "rhs": {"const": {"val": "60%"}}}},
                    "castFriendlySpell": config,
                }}],
            }))
            .map(|rotation| rotation.priority_list[0].action.clone())
        };
        let spell = ActionId::spell(25292);
        assert_eq!(
            cast(Some(serde_json::json!({"type": "Player", "index": 0}))),
            Ok(Action::CastAtPlayer(spell.clone()))
        );
        assert_eq!(
            cast(Some(serde_json::json!({"type": "Self"}))),
            Ok(Action::CastAtPlayer(spell.clone()))
        );
        assert_eq!(
            cast(None),
            Ok(Action::CastSpell {
                spell,
                target: UnitRef::CurrentTarget
            })
        );
        // A player past the raid's one, and the sets of players and targets, name no unit.
        for unit in [
            serde_json::json!({"type": "Player", "index": 1}),
            serde_json::json!({"type": "AllPlayers"}),
            serde_json::json!({"type": "AllTargets"}),
        ] {
            assert_eq!(
                cast(Some(unit.clone())),
                Ok(Action::CastSpell {
                    spell: ActionId::spell(25292),
                    target: UnitRef::Nobody
                }),
                "{unit}"
            );
        }
        // A target of the encounter is cast at like any other.
        assert_eq!(
            cast(Some(serde_json::json!({"type": "Target", "index": 1}))),
            Ok(Action::CastSpell {
                spell: ActionId::spell(25292),
                target: UnitRef::Target(1)
            })
        );
    }

    /// Go `GetAPLSpell` finds no spell for a missing `spellId`, so a cast, friendly cast or
    /// channel naming none builds no action: the line is skipped whatever its condition reads,
    /// and a prepull one registers nothing (Discipline Priest sim e4f6ebb0, 25 October 2026).
    #[test]
    fn a_cast_naming_no_spell_builds_no_action() {
        let rotation = parse(&serde_json::json!({
            "type": "TypeAPL",
            "prepullActions": [{"action": {"castSpell": {}}, "doAtValue": {"const": {"val": "-1s"}}}],
            "priorityList": [
                {"action": {"castFriendlySpell": {"target": {"type": "Self"}},
                    "condition": {"spellIsReady": {"spellId": {"spellId": 10060}}}}},
                {"action": {"castSpell": {"spellId": {"spellId": 10934}}}},
                {"action": {"channelSpell": {"interruptIf": {"const": {"val": "true"}}}}},
                {"action": {"castSpell": {"spellId": null, "target": {"type": "Self"}}}},
            ],
        }))
        .expect("the rotation parses");
        assert!(rotation.prepull.is_empty());
        assert_eq!(rotation.priority_list.len(), 1);
        assert_eq!(rotation.priority_list[0].position, 2);
        assert_eq!(
            rotation.priority_list[0].action,
            Action::CastSpell {
                spell: ActionId::spell(10934),
                target: UnitRef::CurrentTarget
            }
        );
    }

    /// A three target fight whose targets carry aura 2 and whose dot spells sit on the
    /// targets: spell 7 holds a target dot, spell 8 an area dot. The player has aura 3.
    fn target_lookup<T>(run: impl FnOnce(&Lookup<usize>) -> T) -> T {
        let player = |id: &ActionId| {
            (id.spell_id == 3).then_some(FoundAura {
                aura: 100,
                max_stacks: 0,
            })
        };
        let target = |position: usize, id: &ActionId| {
            (id.spell_id == 2).then_some(FoundAura {
                aura: position,
                max_stacks: 5,
            })
        };
        let dot = |id: &ActionId, unit: Unit| match (id.spell_id, unit) {
            (7, Unit::Target(_)) | (8, _) => Some(id.spell_id as usize),
            _ => None,
        };
        run(&Lookup {
            aura: &player,
            target_aura: &target,
            targets: 3,
            spell: &no_spell,
            dot: &dot,
            dot_base_duration: &|_| None,
            pet_aura_known: &|_, _| false,
            pet_aura: &|_, _| None,
            simulated_pet_with_mana: &|_| None,
            unsummoned_pet_with_mana: &|_| false,
            unsummoned_pet_max_mana: &|_| None,
            item_swap: false,
            bars: Bars::default(),
            icd_aura: &|_, _| None,
            multidot: &|_| false,
            assigned_curse: None,
            tanked: false,
            reads: None,
            exact_spell: &|_| None,
            self_buff: &|_| None,
            item_procs: &|_| Vec::new(),
            aura_by_label: &|_| None,
            expected_tick: &|_| None,
        })
    }

    fn compiled(json: serde_json::Value) -> Option<CompiledCondition<usize>> {
        let value = parse_value(&json).ok()?;
        Some(target_lookup(|lookup| {
            compile_condition(Some(&value), lookup)
        }))
    }

    #[test]
    fn unit_references_resolve_as_go_does() {
        let parse = |json: serde_json::Value| parse_unit_ref(Some(&json), UnitRef::CurrentTarget);
        assert_eq!(
            parse(serde_json::json!({"type": "Self"})),
            Ok(UnitRef::Player)
        );
        assert_eq!(
            parse(serde_json::json!({"type": "Player", "index": 0})),
            Ok(UnitRef::Player)
        );
        assert_eq!(
            parse(serde_json::json!({"type": "Target"})),
            Ok(UnitRef::Target(0))
        );
        assert_eq!(
            parse(serde_json::json!({"type": "Target", "index": 2})),
            Ok(UnitRef::Target(2))
        );
        // No type, or an unknown one, is the default.
        assert_eq!(parse(serde_json::json!({})), Ok(UnitRef::CurrentTarget));
        assert_eq!(
            parse(serde_json::json!({"type": "Unknown", "index": 3})),
            Ok(UnitRef::CurrentTarget)
        );
        // Go finds no unit for the sets, or a player past the raid's one.
        for nobody in [
            serde_json::json!({"type": "AllTargets"}),
            serde_json::json!({"type": "AllPlayers"}),
            serde_json::json!({"type": "Player", "index": 1}),
            serde_json::json!({"type": "Player", "index": -2}),
            serde_json::json!({"type": "AllTargets", "owner": {"type": "Self"}}),
        ] {
            assert_eq!(parse(nobody.clone()), Ok(UnitRef::Nobody), "{nobody}");
        }
        // Go reads only the fields a type uses, so the others change nothing.
        assert_eq!(
            parse(serde_json::json!({"type": "Self", "index": 1})),
            Ok(UnitRef::Player)
        );
        assert_eq!(
            parse(serde_json::json!({"type": "NextTarget", "index": 3, "owner": {"type": "Self"}})),
            Ok(UnitRef::NextTarget)
        );
        assert_eq!(
            parse(serde_json::json!({"type": "Target", "owner": {"type": "Self"}})),
            Ok(UnitRef::Target(0))
        );
        // A pet, a negative target index (Go reads before its list) and an unknown field stay
        // unsupported.
        for refused in [
            serde_json::json!({"type": "Pet", "owner": {"type": "Self"}}),
            serde_json::json!({"type": "Target", "index": -1}),
            serde_json::json!({"type": "Self", "name": "x"}),
        ] {
            assert_eq!(parse(refused.clone()), Err(()), "{refused}");
        }
        // The current target is the first, the next wraps to the first, and the previous
        // wraps to the last; an index past the targets is no unit.
        assert_eq!(UnitRef::CurrentTarget.resolve(3), Some(Unit::Target(0)));
        assert_eq!(UnitRef::NextTarget.resolve(3), Some(Unit::Target(1)));
        assert_eq!(UnitRef::NextTarget.resolve(1), Some(Unit::Target(0)));
        assert_eq!(UnitRef::PreviousTarget.resolve(3), Some(Unit::Target(2)));
        assert_eq!(UnitRef::PreviousTarget.resolve(1), Some(Unit::Target(0)));
        assert_eq!(UnitRef::Target(2).resolve(3), Some(Unit::Target(2)));
        assert_eq!(UnitRef::Target(3).resolve(3), None);
        assert_eq!(UnitRef::Player.resolve(3), Some(Unit::Player));
    }

    #[test]
    fn a_cast_names_its_target() {
        let cast = |target: serde_json::Value| {
            parse(
                &serde_json::json!({"type": "TypeAPL", "priorityList": [{"action": {
                "castSpell": {"spellId": {"spellId": 25311}, "target": target}}}]}),
            )
            .map(|rotation| rotation.priority_list[0].action.clone())
        };
        assert_eq!(
            cast(serde_json::json!({"type": "Target", "index": 1})),
            Ok(Action::CastSpell {
                spell: ActionId::spell(25311),
                target: UnitRef::Target(1)
            })
        );
        // A set names no unit, so Go builds no action.
        for set in ["AllTargets", "AllPlayers"] {
            assert_eq!(
                cast(serde_json::json!({"type": set})),
                Ok(Action::CastSpell {
                    spell: ActionId::spell(25311),
                    target: UnitRef::Nobody
                })
            );
        }
        // A sequence step or a prepull action names its unit like any cast.
        let step = |target: serde_json::Value| {
            parse(
                &serde_json::json!({"type": "TypeAPL", "priorityList": [{"action": {
                "sequence": {"actions": [{"castSpell": {
                    "spellId": {"spellId": 1}, "target": target}}]}}}]}),
            )
            .map(|rotation| rotation.priority_list[0].action.clone())
        };
        for (target, unit) in [
            (
                serde_json::json!({"type": "CurrentTarget"}),
                UnitRef::CurrentTarget,
            ),
            (
                serde_json::json!({"type": "Target", "index": 1}),
                UnitRef::Target(1),
            ),
            (serde_json::json!({"type": "AllTargets"}), UnitRef::Nobody),
        ] {
            assert_eq!(
                step(target),
                Ok(Action::Sequence {
                    name: String::new(),
                    steps: vec![Step {
                        spell: ActionId::spell(1),
                        target: unit,
                        condition: None,
                        change_target: false,
                    }]
                })
            );
        }
        let prepull = parse(&serde_json::json!({"type": "TypeAPL", "prepullActions": [{
            "action": {"castSpell": {"spellId": {"spellId": 1},
                "target": {"type": "Player", "index": 2}}},
            "doAtValue": {"const": {"val": "-1s"}}}]}))
        .unwrap();
        assert_eq!(
            prepull.prepull[0].action,
            Action::CastSpell {
                spell: ActionId::spell(1),
                target: UnitRef::Nobody
            }
        );
    }

    /// Go gives a dot value with no spell, or with a unit that has no dot, no value, which
    /// drops the term from its parent.
    #[test]
    fn dots_name_a_unit_and_drop_without_a_spell() {
        let dot = |kind: &str, spell: i32, unit: serde_json::Value| serde_json::json!({kind: {"spellId": {"spellId": spell}, "targetUnit": unit}});
        let on = |kind: &str, unit: serde_json::Value| compiled(dot(kind, 7, unit)).unwrap();
        let at = |spell: usize, target: usize| {
            CompiledCondition::When(Compiled::DotIsActive(DotAt { spell, target }))
        };
        assert_eq!(
            on(
                "dotIsActive",
                serde_json::json!({"type": "Target", "index": 2})
            ),
            at(7, 2)
        );
        assert_eq!(
            on("dotIsActive", serde_json::json!({"type": "NextTarget"})),
            at(7, 1)
        );
        // No unit is the current target; a target dot has none on the player or past the
        // targets, while an area dot is the caster's whatever unit names it.
        let current = serde_json::json!({"dotIsActive": {"spellId": {"spellId": 7}}});
        assert_eq!(compiled(current), Some(at(7, 0)));
        for unit in [
            serde_json::json!({"type": "Self"}),
            serde_json::json!({"type": "Target", "index": 3}),
        ] {
            assert_eq!(on("dotIsActive", unit), CompiledCondition::Always);
        }
        assert_eq!(
            compiled(dot("dotIsActive", 8, serde_json::json!({"type": "Self"}))),
            Some(at(8, 0))
        );
        // No spell, or no dot: the term drops, and the one that remains stands alone.
        let present = dot(
            "dotIsActive",
            7,
            serde_json::json!({"type": "Target", "index": 1}),
        );
        for (name, missing) in [
            ("no spell", serde_json::json!({"dotIsActive": {}})),
            (
                "no dot",
                dot(
                    "dotIsActive",
                    9,
                    serde_json::json!({"type": "CurrentTarget"}),
                ),
            ),
        ] {
            assert_eq!(
                compiled(missing.clone()),
                Some(CompiledCondition::Always),
                "{name}"
            );
            assert_eq!(
                compiled(serde_json::json!({"and": {"vals": [missing.clone(), present.clone()]}})),
                compiled(present.clone()),
                "{name}"
            );
            assert_eq!(
                compiled(serde_json::json!({"not": {"val": missing.clone()}})),
                Some(CompiledCondition::Always),
                "{name}"
            );
            let remaining = serde_json::json!({"dotRemainingTime": missing["dotIsActive"].clone()});
            assert_eq!(
                compiled(serde_json::json!({"cmp": {"op": "OpLt", "lhs": remaining,
                    "rhs": {"const": {"val": "3s"}}}})),
                Some(CompiledCondition::Always),
                "{name}"
            );
        }
        // The sets of all targets and all players name no unit: the dot reference is empty.
        assert!(parse_value(&dot(
            "dotIsActive",
            7,
            serde_json::json!({"type": "AllTargets"})
        ))
        .is_ok());
        assert_eq!(
            compiled(dot(
                "dotIsActive",
                7,
                serde_json::json!({"type": "AllTargets"})
            )),
            Some(CompiledCondition::Always)
        );
    }

    /// Go `newValueCompare` gives booleans only equality, and drops any other comparison.
    #[test]
    fn ordered_comparisons_of_booleans_drop_out() {
        let known = serde_json::json!({"spellIsKnown": {"spellId": {"spellId": 7}}});
        let active = serde_json::json!({"auraIsActive": {"auraId": {"spellId": 2},
            "sourceUnit": {"type": "Target", "index": 1}}});
        let present = serde_json::json!({"dotIsActive": {"spellId": {"spellId": 7}}});
        for op in ["OpLt", "OpLe", "OpGt", "OpGe"] {
            let ordered =
                serde_json::json!({"cmp": {"op": op, "lhs": active.clone(), "rhs": known.clone()}});
            assert_eq!(
                compiled(ordered.clone()),
                Some(CompiledCondition::Always),
                "{op}"
            );
            // Against a constant too.
            let constant = serde_json::json!({"cmp": {"op": op, "lhs": active.clone(),
                "rhs": {"const": {"val": "true"}}}});
            assert_eq!(compiled(constant), Some(CompiledCondition::Always), "{op}");
            let and = serde_json::json!({"and": {"vals": [ordered.clone(), present.clone()]}});
            assert_eq!(compiled(and), compiled(present.clone()), "{op}");
            let or = serde_json::json!({"or": {"vals": [present.clone(), ordered.clone()]}});
            assert_eq!(compiled(or), compiled(present.clone()), "{op}");
            let not = serde_json::json!({"not": {"val": ordered}});
            assert_eq!(compiled(not), Some(CompiledCondition::Always), "{op}");
        }
        // Equality still compares.
        let equal = serde_json::json!({"cmp": {"op": "OpEq", "lhs": active, "rhs": present}});
        assert!(matches!(compiled(equal), Some(CompiledCondition::When(_))));
    }

    /// Go `newValueAuraIsInactive`: an aura the unit lacks is a constant true, and any other
    /// is the negation of its activity.
    #[test]
    fn aura_is_inactive_negates_the_aura() {
        let inactive = |id: i32, source: Option<serde_json::Value>| {
            let mut config = serde_json::json!({"auraId": {"spellId": id}});
            if let Some(source) = source {
                config["sourceUnit"] = source;
            }
            compiled(serde_json::json!({"auraIsInactive": config})).unwrap()
        };
        let on = |aura: usize| {
            CompiledCondition::When(Compiled::Not(Box::new(Compiled::AuraIsActive(aura))))
        };
        assert_eq!(inactive(5, None), CompiledCondition::Always);
        assert_eq!(inactive(3, None), on(100));
        assert_eq!(
            inactive(2, Some(serde_json::json!({"type": "Target", "index": 2}))),
            on(2)
        );
        assert_eq!(
            inactive(2, Some(serde_json::json!({"type": "NextTarget"}))),
            on(1)
        );
        assert_eq!(
            inactive(2, Some(serde_json::json!({"type": "PreviousTarget"}))),
            on(2)
        );
        // A target past the fight's, or the player when it lacks the aura, has no such aura.
        assert_eq!(
            inactive(2, Some(serde_json::json!({"type": "Target", "index": 3}))),
            CompiledCondition::Always
        );
        assert_eq!(
            inactive(2, Some(serde_json::json!({"type": "Self"}))),
            CompiledCondition::Always
        );
    }

    /// Go `APLValueAuraIsActive` and `APLValueAuraIsInactive` with `includeReactionTime`: the
    /// aura must have been up or down for the player's reaction time, and an aura the player
    /// lacks is still a constant.
    #[test]
    fn aura_activity_can_include_the_reaction_time() {
        let read = |kind: &str, id: i32, field: serde_json::Value, source: serde_json::Value| {
            let mut config = serde_json::json!({"auraId": {"spellId": id}});
            if !field.is_null() {
                config["includeReactionTime"] = field;
            }
            if !source.is_null() {
                config["sourceUnit"] = source;
            }
            compiled(serde_json::json!({kind: config}))
        };
        let none = serde_json::Value::Null;
        let yes = serde_json::json!(true);
        assert_eq!(
            read("auraIsActive", 3, yes.clone(), none.clone()),
            Some(CompiledCondition::When(
                Compiled::AuraIsActiveAfterReaction(100)
            ))
        );
        // Not a negation of the active read: the inactive time starts when the aura fades.
        assert_eq!(
            read("auraIsInactive", 3, yes.clone(), none.clone()),
            Some(CompiledCondition::When(
                Compiled::AuraIsInactiveAfterReaction(100)
            ))
        );
        assert_eq!(
            read("auraIsActive", 5, yes.clone(), none.clone()),
            Some(CompiledCondition::Pruned)
        );
        assert_eq!(
            read("auraIsInactive", 5, yes.clone(), none.clone()),
            Some(CompiledCondition::Always)
        );
        // False is the plain read.
        assert_eq!(
            read("auraIsInactive", 3, serde_json::json!(false), none.clone()),
            read("auraIsInactive", 3, none.clone(), none.clone())
        );
        // Any unit's aura has the reaction time: a target's, one past the fight's targets (an
        // aura nobody has) and a set of units, which is no unit at all.
        let target = serde_json::json!({"type": "Target", "index": 1});
        assert_eq!(
            read("auraIsActive", 2, yes.clone(), target.clone()),
            Some(CompiledCondition::When(
                Compiled::AuraIsActiveAfterReaction(1)
            ))
        );
        assert_eq!(
            read("auraIsInactive", 2, yes.clone(), target.clone()),
            Some(CompiledCondition::When(
                Compiled::AuraIsInactiveAfterReaction(1)
            ))
        );
        let past = serde_json::json!({"type": "Target", "index": 3});
        let all = serde_json::json!({"type": "AllTargets"});
        for source in [past, all] {
            assert_eq!(
                read("auraIsActive", 2, yes.clone(), source.clone()),
                Some(CompiledCondition::Pruned)
            );
            assert_eq!(
                read("auraIsInactive", 2, yes.clone(), source),
                Some(CompiledCondition::Always)
            );
        }
        // The stacks of a stacking aura read a reaction time ago; a missing aura has none, and
        // an aura that does not stack has no value.
        let stacks = |id: i32, source: serde_json::Value| {
            let mut read = serde_json::json!({"auraId": {"spellId": id},
                "includeReactionTime": true});
            if !source.is_null() {
                read["sourceUnit"] = source;
            }
            compiled(serde_json::json!({"cmp": {"op": "OpGe",
                "lhs": {"auraNumStacks": read}, "rhs": {"const": {"val": "2"}}}}))
        };
        let Some(CompiledCondition::When(Compiled::Compare { lhs, .. })) =
            stacks(2, target.clone())
        else {
            panic!("a comparison of the stacks");
        };
        assert_eq!(*lhs, Compiled::AuraNumStacksAfterReaction(1));
        let Some(CompiledCondition::When(Compiled::Compare { lhs, .. })) =
            stacks(7, target.clone())
        else {
            panic!("a comparison of the constant 0");
        };
        assert!(matches!(*lhs, Compiled::Const(_)));
        // Aura 3 does not stack: the read has no value and neither has the comparison.
        assert_eq!(stacks(3, none.clone()), Some(CompiledCondition::Always));
        // The reads that have no reaction time are not Go requests.
        for kind in ["auraRemainingTime", "auraIsKnown"] {
            let parsed = parse_value(&serde_json::json!({kind: {
                "auraId": {"spellId": 2}, "includeReactionTime": true}}));
            assert!(parsed.is_err(), "{kind}");
        }
        assert!(parse_value(&serde_json::json!({"auraIsActive": {
            "auraId": {"spellId": 2}, "includeReactionTime": "yes"}}))
        .is_err());
    }

    /// Go `GetUnit` names no unit for the sets `AllPlayers` and `AllTargets`, so a dot or an
    /// aura read on them is a reference that does not exist: the dot value has no value and
    /// drops out of its parent, and the aura reads as one the unit lacks.
    #[test]
    fn sets_of_units_name_no_unit_in_dot_and_aura_values() {
        let all = serde_json::json!({"type": "AllTargets"});
        let dot = |kind: &str| serde_json::json!({kind: {"spellId": {"spellId": 7}, "targetUnit": all.clone()}});
        for kind in ["dotIsActive", "dotRemainingTime", "dotTimeToNextTick"] {
            let parsed = parse_value(&dot(kind)).unwrap();
            assert!(matches!(
                parsed,
                Value::DotIsActive(DotRef {
                    target: UnitRef::Nobody,
                    ..
                }) | Value::DotRemainingTime(DotRef {
                    target: UnitRef::Nobody,
                    ..
                }) | Value::DotTimeToNextTick(DotRef {
                    target: UnitRef::Nobody,
                    ..
                })
            ));
        }
        // A dot term that drops out leaves the rest of the condition.
        let rest = serde_json::json!({"cmp": {"op": "OpGe", "lhs": {"remainingTime": {}},
            "rhs": {"const": {"val": "6"}}}});
        let with = |term: serde_json::Value| {
            compiled(serde_json::json!({"and": {"vals": [term, rest.clone()]}}))
        };
        assert_eq!(
            with(serde_json::json!({"not": {"val": dot("dotIsActive")}})),
            compiled(rest.clone())
        );
        assert_eq!(with(dot("dotIsActive")), compiled(rest.clone()));
        // A dot on a unit that does exist still counts.
        assert_ne!(
            with(serde_json::json!({"dotIsActive": {"spellId": {"spellId": 8}}})),
            compiled(rest.clone())
        );
        // The aura of a set is an aura nobody has.
        let aura = |kind: &str| {
            compiled(serde_json::json!({kind: {
                "auraId": {"spellId": 2}, "sourceUnit": {"type": "AllPlayers"}}}))
        };
        assert_eq!(aura("auraIsKnown"), Some(CompiledCondition::Pruned));
        assert_eq!(aura("auraIsActive"), Some(CompiledCondition::Pruned));
        assert_eq!(aura("auraIsInactive"), Some(CompiledCondition::Always));
        // Go reads no other field of a set, and a field no reference has still refuses.
        assert!(parse_value(&serde_json::json!({"dotIsActive": {
            "spellId": {"spellId": 7},
            "targetUnit": {"type": "AllTargets", "owner": {"type": "Self"}}}}))
        .is_ok());
        assert!(parse_value(&serde_json::json!({"dotIsActive": {
            "spellId": {"spellId": 7},
            "targetUnit": {"type": "AllTargets", "name": "x"}}}))
        .is_err());
    }

    /// Go `newValueDotBaseDuration` captures the dot's base duration when the rotation is
    /// built, so the value is a duration the compiled rotation holds; a spell without that dot
    /// has no value.
    #[test]
    fn dot_base_duration_is_captured_when_the_rotation_is_built() {
        let parsed = parse_value(&serde_json::json!({"dotBaseDuration": {
            "spellId": {"spellId": 7}}}))
        .unwrap();
        assert_eq!(parsed.value_type(), ValueType::Duration);
        let player = |_: &ActionId| None::<FoundAura<()>>;
        let target = |_: usize, _: &ActionId| None::<FoundAura<()>>;
        let lookup = Lookup {
            aura: &player,
            target_aura: &target,
            targets: 1,
            spell: &no_spell,
            dot: &no_dot,
            dot_base_duration: &|id| (id.spell_id == 7).then_some(18_000_000_000),
            pet_aura_known: &|_, _| false,
            pet_aura: &|_, _| None,
            simulated_pet_with_mana: &|_| None,
            unsummoned_pet_with_mana: &|_| false,
            unsummoned_pet_max_mana: &|_| None,
            item_swap: false,
            bars: Bars::default(),
            icd_aura: &|_, _| None,
            multidot: &|_| false,
            assigned_curse: None,
            tanked: false,
            reads: None,
            exact_spell: &|_| None,
            self_buff: &|_| None,
            item_procs: &|_| Vec::new(),
            aura_by_label: &|_| None,
            expected_tick: &|_| None,
        };
        let compare = |spell: i32| {
            let value = parse_value(&serde_json::json!({"cmp": {"op": "OpGt",
                "lhs": {"remainingTime": {}},
                "rhs": {"dotBaseDuration": {"spellId": {"spellId": spell}}}}}))
            .unwrap();
            compile_condition(Some(&value), &lookup)
        };
        assert_eq!(
            compare(7),
            CompiledCondition::When(Compiled::Compare {
                op: CompareOp::Gt,
                lhs: Box::new(Compiled::RemainingTime),
                rhs: Box::new(Compiled::DotBaseDuration(18_000_000_000)),
            })
        );
        // The comparison has no value without the dot, and a condition with none always holds.
        assert_eq!(compare(8), CompiledCondition::Always);
        let no_spell_id = parse_value(&serde_json::json!({"dotBaseDuration": {}})).unwrap();
        assert!(compile_value(&no_spell_id, &lookup).is_none());
    }

    #[test]
    fn auras_on_a_target_read_that_targets_aura() {
        let read = |kind: &str, unit: serde_json::Value| {
            let rhs = if kind == "auraRemainingTime" {
                "2s"
            } else {
                "1"
            };
            compiled(serde_json::json!({"cmp": {"op": "OpGe",
                "lhs": {kind: {"auraId": {"spellId": 2}, "sourceUnit": unit}},
                "rhs": {"const": {"val": rhs}}}}))
            .unwrap()
        };
        let CompiledCondition::When(Compiled::Compare { lhs, .. }) = read(
            "auraNumStacks",
            serde_json::json!({"type": "Target", "index": 1}),
        ) else {
            panic!("expected a comparison");
        };
        assert_eq!(*lhs, Compiled::AuraNumStacks(1));
        let CompiledCondition::When(Compiled::Compare { lhs, .. }) = read(
            "auraRemainingTime",
            serde_json::json!({"type": "PreviousTarget"}),
        ) else {
            panic!("expected a comparison");
        };
        assert_eq!(*lhs, Compiled::AuraRemainingTime(2));
        // The aura of a target that does not exist has no stacks: Go's constant "0".
        let CompiledCondition::When(Compiled::Compare { lhs, .. }) = read(
            "auraNumStacks",
            serde_json::json!({"type": "Target", "index": 9}),
        ) else {
            panic!("expected a comparison");
        };
        assert_eq!(*lhs, Compiled::Const(parse_const("0").unwrap()));
    }
}
