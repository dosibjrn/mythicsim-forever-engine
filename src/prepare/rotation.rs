//! Go sim/core/apl.go `newAPLRotation`, for what building the rotation changes in the
//! prepared state: the prepull actions it registers and the major cooldowns it removes
//! because it casts them itself.
//!
//! Go builds every action and value of the rotation. Most of that only reads the simulation;
//! preparation follows the part that changes it. An action whose spell the player does not
//! know is dropped, as Go's constructors return nil for it; a condition Go folds to a constant
//! is refused, since only Go's constant folding can say whether it prunes the action.

use crate::contracts::prepared_v2::ActionId;
use crate::contracts::request::{Message, Value};

use super::agent::proto_to_action_id;
use super::env::Environment;
use super::sim::{Sim, SpellId, UnitId};
use super::spell::SpellFlag;
use super::Refusal;

/// The spell ids the rotation of the request names anywhere, which the exporter's
/// `rotationSpellIDs` collects: every `spellId` in the rotation's protojson.
pub(crate) fn rotation_spells(sim: &Sim, unit: UnitId) -> Vec<i64> {
    fn walk(node: &serde_json::Value, out: &mut Vec<i64>) {
        match node {
            serde_json::Value::Object(map) => {
                for (key, child) in map {
                    if key == "spellId" {
                        if let Some(id) = child.as_f64() {
                            out.push(id as i64);
                        }
                    }
                    walk(child, out);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|child| walk(child, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    if let Some(rotation) = sim.character(unit).player.message("rotation") {
        walk(&rotation.to_protojson(), &mut out);
    }
    out
}

/// Go `GetAPLSpell`.
fn apl_spell(env: &Environment, id: &ActionId) -> Option<SpellId> {
    let sim = &env.sim;
    let book = &sim.unit(env.player).spellbook;
    if id.other_id == "OtherActionPotion" && id.spell_id == 0 && id.item_id == 0 {
        return book
            .iter()
            .copied()
            .find(|s| sim.spell(*s).flags.matches(SpellFlag::COMBAT_POTION));
    }
    book.iter()
        .copied()
        .find(|s| &sim.spell(*s).action_id == id && sim.spell(*s).flags.matches(SpellFlag::APL))
        .or_else(|| sim.get_spell(env.player, id))
        // Go `RankAliases`: an older rank a rotation names presses the known one.
        .or_else(|| {
            book.iter().copied().find(|s| {
                sim.spell(*s).flags.matches(SpellFlag::APL)
                    && sim
                        .spell(*s)
                        .rank_aliases
                        .iter()
                        .any(|alias| same_action(alias, id))
            })
        })
}

/// Go `ActionID.SameAction`: the same action whatever the tag.
fn same_action(a: &ActionId, b: &ActionId) -> bool {
    a.spell_id == b.spell_id && a.item_id == b.item_id && a.other_id == b.other_id
}

/// Go `GetAPLCastSpell`.
fn apl_cast_spell(env: &Environment, id: &ActionId) -> Option<SpellId> {
    let sim = &env.sim;
    if let Some(spell) = sim.get_spell(env.player, id) {
        if !sim.spell(spell).flags.matches(SpellFlag::APL) {
            if let Some(queued) = sim.unit(env.player).spellbook.iter().copied().find(|s| {
                let other = &sim.spell(*s).action_id;
                other.same_action_ignore_tag(id) && sim.spell(*s).flags.matches(SpellFlag::APL)
            }) {
                return Some(queued);
            }
        }
    }
    apl_spell(env, id)
}

/// Whether an action's target reference resolves: Go `GetTargetUnit(...).Get() != nil`. No
/// reference, or one of an unknown type, is the current target.
fn target_resolves(env: &Environment, target: Option<&Message>) -> Result<bool, Refusal> {
    let Some(target) = target else {
        return Ok(true);
    };
    if target.enum_name("type") == "Unknown" {
        return Ok(true);
    }
    match source_unit(env, Some(target)) {
        Ok(unit) => Ok(unit.is_some()),
        Err(()) => Err(Refusal::new(
            "rotation",
            format!(
                "a rotation target of type {} is not prepared yet",
                target.enum_name("type")
            ),
        )),
    }
}

/// Go `spell.CurDot() != nil`: the spell's dot on its caster's current target, or its related
/// dot spell's.
fn has_cur_dot(env: &Environment, spell: SpellId) -> Result<bool, Refusal> {
    let s = env.sim.spell(spell);
    if s.dots.is_empty() {
        return match s.related_dot_spell {
            Some(related) => has_cur_dot(env, related),
            None => Ok(false),
        };
    }
    let Some(target) = env.sim.unit(s.unit).current_target else {
        return Err(Refusal::new(
            "rotation",
            "a dot read without a current target".to_string(),
        ));
    };
    let index = env.sim.unit(target).unit_index as usize;
    Ok(s.dots.get(index).copied().flatten().is_some())
}

/// One built action: Go's `APLAction` reduced to what construction needs.
struct Built {
    /// Spells a cast action presses, including those of inner actions.
    casts: Vec<SpellId>,
    /// The spell of a cast action itself: what Go's `removeFromMajorCooldowns` reads of a
    /// pruned action's impl.
    direct: Option<SpellId>,
}

impl Built {
    fn new(casts: Vec<SpellId>) -> Self {
        Built {
            casts,
            direct: None,
        }
    }
}

struct Builder<'a> {
    env: &'a Environment,
    /// Go `rot.valueVariables`: each variable's name and value.
    variables: Vec<(String, Message)>,
    /// Go `rot.prunedActions`: impls of actions a constant false condition removed.
    pruned_casts: Vec<SpellId>,
    /// The values Go constructs: those of every action whose impl it builds, and their
    /// conditions. An action Go drops for its impl never builds its condition.
    built_values: Vec<Message>,
}

/// A value's type as far as Go's constructors check it: only a boolean matters, to Go
/// `newValueCompare`'s check that booleans are only compared for equality.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Ty {
    Bool,
    Other,
    Unknown,
}

/// What Go's constructor returns for a value: nil, a value read during the fight, or an
/// `APLValueConst` with its type and `boolVal`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Fold {
    Nil,
    Dyn(Ty),
    Const(Ty, bool),
}

impl Fold {
    const FALSE: Fold = Fold::Const(Ty::Bool, false);
    const TRUE: Fold = Fold::Const(Ty::Bool, true);

    fn ty(self) -> Option<Ty> {
        match self {
            Fold::Nil => None,
            Fold::Dyn(ty) | Fold::Const(ty, _) => Some(ty),
        }
    }
}

/// Every result Go's constructor may give for a value. Preparation does not follow every
/// value's constructor, so a value it does not follow may be nil or read during the fight;
/// the set keeps every possibility, and a decision that depends on which one is refused.
type Folds = std::collections::BTreeSet<Fold>;

fn one(fold: Fold) -> Folds {
    Folds::from([fold])
}

/// Go `coerceTo(value, ValueTypeBool)`: a constant keeps its `boolVal` with the new type.
fn coerce_bool(folds: &Folds) -> Folds {
    folds
        .iter()
        .map(|fold| match *fold {
            Fold::Nil => Fold::Nil,
            Fold::Dyn(_) => Fold::Dyn(Ty::Bool),
            Fold::Const(_, b) => Fold::Const(Ty::Bool, b),
        })
        .collect()
}

/// Go `newValueConst`: `boolVal` is whether the text is empty, unless it says true or false.
fn const_fold(text: &str) -> Fold {
    if text.eq_ignore_ascii_case("true") {
        Fold::TRUE
    } else if text.eq_ignore_ascii_case("false") {
        Fold::FALSE
    } else {
        Fold::Const(Ty::Other, !text.is_empty())
    }
}

/// The non-nil values of a list Go filters, as the possible outcomes of Go `newValueAnd`,
/// `newValueOr`, `newValueMin` and `newValueMax`: nil for none, the value itself for one, and
/// for more, whether any of them is the constant `short` (false for And, true for Or).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Gathered {
    Empty,
    One(Fold),
    Many { short: bool },
}

fn gather(children: &[Folds], short: Option<Fold>) -> std::collections::BTreeSet<Gathered> {
    let mut states = std::collections::BTreeSet::from([Gathered::Empty]);
    for child in children {
        let mut next = std::collections::BTreeSet::new();
        for state in &states {
            for fold in child {
                let is_short = Some(*fold) == short;
                next.insert(match (*state, *fold) {
                    (state, Fold::Nil) => state,
                    (Gathered::Empty, fold) => Gathered::One(fold),
                    (Gathered::One(first), _) => Gathered::Many {
                        short: Some(first) == short || is_short,
                    },
                    (Gathered::Many { short: any }, _) => Gathered::Many {
                        short: any || is_short,
                    },
                });
            }
        }
        states = next;
    }
    states
}

/// Go `newValueAnd` (short constant false) and `newValueOr` (short constant true).
fn fold_logic(children: &[Folds], short: Fold) -> Folds {
    let children: Vec<Folds> = children.iter().map(coerce_bool).collect();
    gather(&children, Some(short))
        .into_iter()
        .map(|state| match state {
            Gathered::Empty => Fold::Nil,
            Gathered::One(fold) => fold,
            Gathered::Many { short: true } => short,
            Gathered::Many { short: false } => Fold::Dyn(Ty::Bool),
        })
        .collect()
}

/// Go `NextActiveTargetUnit` and `PreviousActiveTargetUnit` of the player's current target:
/// every target is active, and the targets go round.
fn neighbour_target(env: &Environment, next: bool) -> Option<UnitId> {
    let targets = &env.encounter.targets;
    let current = env.sim.unit(env.player).current_target?;
    let index = targets.iter().position(|target| *target == current)?;
    let count = targets.len();
    Some(
        targets[if next {
            (index + 1) % count
        } else {
            (index + count - 1) % count
        }],
    )
}

/// The unit a value's source unit reference names while the rotation is built: `None` for a
/// reference Go resolves to no unit, an error for one preparation does not follow.
fn source_unit(env: &Environment, reference: Option<&Message>) -> Result<Option<UnitId>, ()> {
    let kind = reference.map_or_else(|| "Unknown".to_string(), |r| r.enum_name("type"));
    match kind.as_str() {
        "Unknown" | "Self" => Ok(Some(env.player)),
        // The raid holds one player, at index 0.
        "Player" => Ok((reference.map_or(0, |r| r.i32("index")) == 0).then_some(env.player)),
        // Go `Environment.GetUnit` names no unit for the sets of all players and all targets.
        "AllPlayers" | "AllTargets" => Ok(None),
        "CurrentTarget" => Ok(env.sim.unit(env.player).current_target),
        // Go `NextActiveTarget` and `PreviousActiveTarget` of the current target: every target
        // is enabled while the rotation is built, as preparation refuses one disabled at start.
        "NextTarget" | "PreviousTarget" => {
            let targets = &env.encounter.targets;
            let Some(current) = env.sim.unit(env.player).current_target else {
                return Err(());
            };
            let Some(index) = targets.iter().position(|t| *t == current) else {
                return Err(());
            };
            let next = if kind == "NextTarget" {
                (index + 1) % targets.len()
            } else {
                (index + targets.len() - 1) % targets.len()
            };
            Ok(Some(targets[next]))
        }
        "Target" => {
            let index = reference.map_or(0, |r| r.i32("index"));
            Ok(usize::try_from(index)
                .ok()
                .and_then(|i| env.encounter.targets.get(i).copied()))
        }
        // Go `Environment.GetUnit`: a pet of the owner the reference names, by its index.
        "Pet" => {
            let index = reference.map_or(0, |r| r.i32("index"));
            let owner = reference
                .and_then(|r| r.message("owner"))
                .map(|owner| (owner.enum_name("type"), owner.i32("index")));
            let owner_is_player = match owner {
                Some((kind, _)) if kind == "Self" => true,
                Some((kind, index)) if kind == "Player" => index == 0,
                _ => false,
            };
            Ok(owner_is_player
                .then(|| {
                    usize::try_from(index)
                        .ok()
                        .and_then(|i| env.sim.unit(env.player).pets.get(i).copied())
                })
                .flatten())
        }
        _ => Err(()),
    }
}

/// Go `GetAPLAura(GetSourceUnit(...), id).Get()`: whether the aura exists, or `None` when
/// preparation does not follow the reference.
fn aura_known(env: &Environment, config: &Message) -> Option<bool> {
    let unit = source_unit(env, config.message("source_unit")).ok()?;
    let id = proto_to_action_id(config.message("aura_id")?);
    Some(unit.is_some_and(|unit| {
        env.sim
            .unit(unit)
            .auras
            .iter()
            .any(|aura| env.sim.aura(*aura).action_id.as_ref() == Some(&id))
    }))
}

/// Go `GetAPLAura(...).Get().MaxStacks`: `Some(None)` for an aura the unit lacks, `None` when
/// preparation does not follow the reference.
fn aura_stacks(env: &Environment, config: &Message) -> Option<Option<i32>> {
    let unit = source_unit(env, config.message("source_unit")).ok()?;
    let id = proto_to_action_id(config.message("aura_id")?);
    Some(unit.and_then(|unit| {
        env.sim
            .unit(unit)
            .auras
            .iter()
            .find(|aura| env.sim.aura(**aura).action_id.as_ref() == Some(&id))
            .map(|aura| env.sim.aura(*aura).max_stacks)
    }))
}

/// Go `spell.Dot(target)`, following the related dot spell.
fn spell_dot(env: &Environment, spell: SpellId, target: UnitId) -> bool {
    let s = env.sim.spell(spell);
    if s.dots.is_empty() {
        return s
            .related_dot_spell
            .is_some_and(|related| spell_dot(env, related, target));
    }
    let index = env.sim.unit(target).unit_index as usize;
    s.dots.get(index).copied().flatten().is_some()
}

/// Go `NewDotReference(GetTargetUnit(...), id).Get() != nil`, or `None` when preparation does
/// not follow the reference.
fn dot_exists(env: &Environment, config: &Message) -> Option<bool> {
    let Some(id) = config.message("spell_id") else {
        return Some(false);
    };
    let reference = config.message("target_unit");
    let kind = reference.map_or_else(|| "Unknown".to_string(), |r| r.enum_name("type"));
    let target = match kind.as_str() {
        "Unknown" | "CurrentTarget" => env.sim.unit(env.player).current_target,
        "NextTarget" => neighbour_target(env, true),
        "PreviousTarget" => neighbour_target(env, false),
        "Target" => {
            let index = reference.map_or(0, |r| r.i32("index"));
            usize::try_from(index)
                .ok()
                .and_then(|i| env.encounter.targets.get(i).copied())
        }
        // Go `GetUnit` names no unit for the sets, so the dot reference is empty.
        "AllPlayers" | "AllTargets" => None,
        _ => return None,
    };
    let Some(target) = target else {
        return Some(false);
    };
    let Some(spell) = apl_spell(env, &proto_to_action_id(id)) else {
        return Some(false);
    };
    Some(env.sim.spell(spell).aoe_dot.is_some() || spell_dot(env, spell, target))
}

/// Go's value constructors, as far as constant folding goes: `newAPLValue` for a condition.
fn fold(env: &Environment, value: &Message) -> Folds {
    let unknown = || Folds::from([Fold::Nil, Fold::Dyn(Ty::Unknown)]);
    let Some((kind, Value::Message(config))) = value.oneof("value") else {
        return one(Fold::Nil);
    };
    // An aura value: nil without an aura id, `missing` for an aura the unit lacks.
    let aura_value = |missing: Fold, present: Fold| -> Folds {
        if config.message("aura_id").is_none() {
            return one(Fold::Nil);
        }
        match aura_known(env, config) {
            Some(true) => one(present),
            Some(false) => one(missing),
            None => Folds::from([missing, present]),
        }
    };
    match kind {
        "const" => one(const_fold(config.str("val"))),
        "and" => fold_logic(
            &config
                .messages("vals")
                .into_iter()
                .map(|v| fold(env, v))
                .collect::<Vec<_>>(),
            Fold::FALSE,
        ),
        "or" => fold_logic(
            &config
                .messages("vals")
                .into_iter()
                .map(|v| fold(env, v))
                .collect::<Vec<_>>(),
            Fold::TRUE,
        ),
        "not" => {
            let inner = config
                .message("val")
                .map_or_else(|| one(Fold::Nil), |v| fold(env, v));
            coerce_bool(&inner)
                .into_iter()
                .map(|fold| match fold {
                    Fold::Const(_, b) => Fold::Const(Ty::Bool, !b),
                    other => other,
                })
                .collect()
        }
        "min" | "max" => {
            let children: Vec<Folds> = config
                .messages("vals")
                .into_iter()
                .map(|v| fold(env, v))
                .collect();
            // coerceAllToSameType may change a type; a lone value is returned as it is.
            gather(&children, None)
                .into_iter()
                .map(|state| match state {
                    Gathered::Empty => Fold::Nil,
                    Gathered::One(Fold::Const(_, b)) => Fold::Const(Ty::Unknown, b),
                    Gathered::One(_) | Gathered::Many { .. } => Fold::Dyn(Ty::Unknown),
                })
                .collect()
        }
        "cmp" | "math" => {
            let side = |name: &str| {
                config
                    .message(name)
                    .map_or_else(|| one(Fold::Nil), |v| fold(env, v))
            };
            let (lhs, rhs) = (side("lhs"), side("rhs"));
            let mut out = Folds::new();
            for l in &lhs {
                for r in &rhs {
                    if *l == Fold::Nil || *r == Fold::Nil {
                        out.insert(Fold::Nil);
                        continue;
                    }
                    let equality =
                        kind == "cmp" && matches!(config.enum_name("op").as_str(), "OpEq" | "OpNe");
                    let checked = [l.ty(), r.ty()].into_iter().all(|ty| ty == Some(Ty::Other));
                    // Go nils comparisons of booleans other than for equality and math on
                    // types it rejects; only two other-typed operands of a comparison are
                    // certain to give a value.
                    if !(kind == "cmp" && (equality || checked)) {
                        out.insert(Fold::Nil);
                    }
                    out.insert(Fold::Dyn(if kind == "cmp" {
                        Ty::Bool
                    } else {
                        Ty::Unknown
                    }));
                }
            }
            out
        }
        "aura_is_known" => match aura_known(env, config) {
            Some(known) => one(Fold::Const(Ty::Bool, known)),
            None => Folds::from([Fold::FALSE, Fold::TRUE]),
        },
        // Go `newValueSpellCanCast`, `IsReady`, `TimeToReady` and `CastTime`: nil when
        // `GetAPLSpell` finds no spell.
        "spell_can_cast" | "spell_is_ready" | "spell_time_to_ready" | "spell_cast_time" => {
            let ty = if matches!(kind, "spell_can_cast" | "spell_is_ready") {
                Ty::Bool
            } else {
                Ty::Other
            };
            match config.message("spell_id") {
                Some(id) if apl_spell(env, &proto_to_action_id(id)).is_some() => one(Fold::Dyn(ty)),
                _ => one(Fold::Nil),
            }
        }
        "spell_is_known" => match config.message("spell_id") {
            Some(id) => one(Fold::Const(
                Ty::Bool,
                apl_spell(env, &proto_to_action_id(id)).is_some(),
            )),
            None => unknown(),
        },
        // Go `newValueActiveItemSwapSet`: nil for an unknown set or without an item swap.
        "active_item_swap_set" => {
            let unknown = config.enum_name("swap_set") == "Unknown";
            if unknown || env.sim.item_swap(env.player).is_none() {
                one(Fold::Nil)
            } else {
                one(Fold::Dyn(Ty::Bool))
            }
        }
        "aura_is_active" => aura_value(Fold::FALSE, Fold::Dyn(Ty::Bool)),
        "aura_is_inactive" => aura_value(Fold::TRUE, Fold::Dyn(Ty::Bool)),
        // Go's constant "0ms": its boolVal is true, as the text is not empty.
        "aura_remaining_time" => aura_value(Fold::Const(Ty::Other, true), Fold::Dyn(Ty::Other)),
        // Go's constant "0" for an aura the unit lacks, and nil for one without stacks.
        "aura_num_stacks" => {
            if config.message("aura_id").is_none() {
                return one(Fold::Nil);
            }
            match aura_stacks(env, config) {
                Some(None) => one(Fold::Const(Ty::Other, true)),
                Some(Some(0)) => one(Fold::Nil),
                Some(Some(_)) => one(Fold::Dyn(Ty::Other)),
                None => Folds::from([
                    Fold::Const(Ty::Other, true),
                    Fold::Nil,
                    Fold::Dyn(Ty::Other),
                ]),
            }
        }
        // Values Go always builds.
        "remaining_time"
        | "remaining_time_percent"
        | "current_time"
        | "current_time_percent"
        | "number_targets" => one(Fold::Dyn(Ty::Other)),
        // Go `newValueAutoSwingTime`, `newValueAutoTimeToNext` and `newValueAutoTimeSinceLast`
        // build a duration whatever the swing type.
        "auto_swing_time" | "auto_time_to_next" | "auto_time_since_last" => {
            one(Fold::Dyn(Ty::Other))
        }
        // Go `newValueIsExecutePhase`: nil for an unknown threshold.
        "is_execute_phase" => {
            if config.enum_name("threshold") == "Unknown" {
                one(Fold::Nil)
            } else {
                one(Fold::Dyn(Ty::Bool))
            }
        }
        // Nil for a unit without mana.
        "current_mana_percent" => match source_unit(env, config.message("source_unit")) {
            Ok(Some(unit)) if env.sim.unit(unit).mana_bar.enabled => one(Fold::Dyn(Ty::Other)),
            Ok(_) => one(Fold::Nil),
            Err(()) => unknown(),
        },
        // Go `NewDotReference`: nil when the spell has no dot on the target.
        "dot_is_active" | "dot_remaining_time" | "dot_time_to_next_tick" => {
            let ty = if kind == "dot_is_active" {
                Ty::Bool
            } else {
                Ty::Other
            };
            match dot_exists(env, config) {
                Some(true) => one(Fold::Dyn(ty)),
                Some(false) => one(Fold::Nil),
                None => Folds::from([Fold::Nil, Fold::Dyn(ty)]),
            }
        }
        _ => unknown(),
    }
}

impl Builder<'_> {
    /// Go `newValueVariableRef` builds the value the variable names where it is read, and that
    /// value can read variables too.
    fn expand_variable_references(&self, built: &mut Vec<Message>) {
        let mut next = 0;
        while next < built.len() && built.len() < 4096 {
            let value = built[next].clone();
            next += 1;
            let Some(("variable_ref", Value::Message(reference))) = value.oneof("value") else {
                continue;
            };
            let name = reference.str("name");
            let Some((_, resolved)) = self.variables.iter().find(|(variable, _)| variable == name)
            else {
                continue;
            };
            let mut inner = Vec::new();
            values(resolved, &mut inner);
            built.extend(inner.into_iter().cloned());
        }
    }

    /// Go `newAPLAction`: `None` for an action Go's constructor returns nil for or prunes.
    fn action(&mut self, config: Option<&Message>) -> Result<Option<Built>, Refusal> {
        let Some(config) = config else {
            return Ok(None);
        };
        let Some(built) = self.action_impl(config)? else {
            return Ok(None);
        };
        let mut found = Vec::new();
        values(config, &mut found);
        let mut messages: Vec<Message> = found.into_iter().cloned().collect();
        self.expand_variable_references(&mut messages);
        self.built_values.extend(messages);
        // Go prunes an action whose condition folds to a constant false, and keeps its impl
        // so the spell it casts still leaves the major cooldowns.
        if let Some(condition) = config.message("condition") {
            let folded = coerce_bool(&fold(self.env, condition));
            let pruned = folded.iter().filter(|f| **f == Fold::FALSE).count();
            if pruned == folded.len() {
                self.pruned_casts.extend(built.direct);
                return Ok(None);
            }
            if pruned > 0 {
                return Err(Refusal::new(
                    "rotation",
                    "a rotation condition Go may fold to a constant is not prepared yet"
                        .to_string(),
                ));
            }
        }
        Ok(Some(built))
    }

    fn action_impl(&mut self, config: &Message) -> Result<Option<Built>, Refusal> {
        // Go asks the agent first: a class builds its own actions.
        if let Some(built) =
            self.env
                .agent
                .custom_apl_action(&self.env.sim, self.env.player, config)
        {
            return Ok(built.then(|| Built::new(Vec::new())));
        }
        let Some((kind, Value::Message(action))) = config.oneof("action") else {
            return Err(Refusal::new(
                "rotation",
                "a rotation action without a kind".to_string(),
            ));
        };
        match kind {
            "cast_spell" | "cast_friendly_spell" => {
                let Some(id) = action.message("spell_id") else {
                    return Ok(None);
                };
                let id = proto_to_action_id(id);
                let spell = if kind == "cast_spell" {
                    apl_cast_spell(self.env, &id)
                } else {
                    apl_spell(self.env, &id)
                };
                let Some(spell) = spell else {
                    return Ok(None);
                };
                if !target_resolves(self.env, action.message("target"))? {
                    return Ok(None);
                }
                Ok(Some(Built {
                    casts: vec![spell],
                    direct: Some(spell),
                }))
            }
            // Go `newActionChannelSpell`: without an interrupt condition it is a cast action,
            // which removes its spell from the major cooldowns; with one it is a channel action,
            // which does not.
            "channel_spell" => {
                let Some(id) = action.message("spell_id") else {
                    return Ok(None);
                };
                let id = proto_to_action_id(id);
                let casts = action.message("interrupt_if").is_none();
                let spell = if casts {
                    apl_cast_spell(self.env, &id)
                } else {
                    apl_spell(self.env, &id)
                };
                let Some(spell) = spell else {
                    return Ok(None);
                };
                if !casts
                    && !self
                        .env
                        .sim
                        .spell(spell)
                        .flags
                        .matches(SpellFlag::CHANNELED)
                {
                    return Ok(None);
                }
                if !target_resolves(self.env, action.message("target"))? {
                    return Ok(None);
                }
                Ok(Some(if casts {
                    Built {
                        casts: vec![spell],
                        direct: Some(spell),
                    }
                } else {
                    Built::new(Vec::new())
                }))
            }
            // Go `newActionSequence` drops the steps that are nil and is nil without any;
            // `newActionStrictSequence` is nil when any step is, and prunes the others.
            "sequence" | "strict_sequence" => {
                let mut built = Vec::new();
                let mut missing = false;
                for inner in action.messages("actions") {
                    match self.action(Some(inner))? {
                        Some(step) => built.push(step),
                        None => missing = true,
                    }
                }
                if kind == "strict_sequence" && missing {
                    self.pruned_casts
                        .extend(built.iter().filter_map(|step| step.direct));
                    return Ok(None);
                }
                if built.is_empty() {
                    return Ok(None);
                }
                Ok(Some(Built::new(
                    built.into_iter().flat_map(|step| step.casts).collect(),
                )))
            }
            // Go `newActionMultidot`: dropped unless the spell has a dot on the current
            // target; it removes no major cooldown.
            // Go `newActionActivateAura`, `newActionCancelAura`, `newActionActivateAuraWithStacks`
            // and `newActionTriggerICD`: nil for an aura the player lacks; they change nothing
            // while the rotation is built.
            "activate_aura" | "cancel_aura" | "activate_aura_with_stacks" | "trigger_icd" => {
                let Some(id) = action.message("aura_id") else {
                    return Ok(None);
                };
                let id = proto_to_action_id(id);
                let sim = &self.env.sim;
                let found = sim.unit(self.env.player).auras.iter().find(|aura| {
                    let a = sim.aura(**aura);
                    if kind == "trigger_icd" {
                        (a.action_id.as_ref() == Some(&id)
                            || a.action_id_for_proc.as_ref() == Some(&id))
                            && a.icd.is_some()
                    } else {
                        a.action_id.as_ref() == Some(&id)
                    }
                });
                let Some(aura) = found else {
                    return Ok(None);
                };
                if kind == "activate_aura_with_stacks" && sim.aura(*aura).max_stacks == 0 {
                    return Ok(None);
                }
                Ok(Some(Built::new(Vec::new())))
            }
            // Go `newActionChangeTarget`: nil when the new target resolves to no unit.
            "change_target" => match source_unit(self.env, action.message("new_target")) {
                Ok(Some(_)) => Ok(Some(Built::new(Vec::new()))),
                Ok(None) => Ok(None),
                Err(()) => Err(Refusal::new(
                    "rotation",
                    "a change of target to this unit is not prepared yet".to_string(),
                )),
            },
            // Go `newActionStrictMultidot` as `newActionMultidot`: its casts on each target are
            // its own, which no walk of the rotation's actions reaches.
            "multidot" | "strict_multidot" => {
                let Some(id) = action.message("spell_id") else {
                    return Ok(None);
                };
                let Some(spell) = apl_spell(self.env, &proto_to_action_id(id)) else {
                    return Ok(None);
                };
                if !has_cur_dot(self.env, spell)? {
                    return Ok(None);
                }
                Ok(Some(Built::new(Vec::new())))
            }
            // Go builds these without changing the simulation; a move reads its range only
            // when it runs.
            "autocast_other_cooldowns" | "wait" | "wait_until" | "move" | "move_duration" => {
                Ok(Some(Built::new(Vec::new())))
            }
            // Go `newActionItemSwap`: no action for an unknown set or without an item swap.
            "item_swap" => {
                let unknown = action.enum_name("swap_set") == "Unknown";
                if unknown || self.env.sim.item_swap(self.env.player).is_none() {
                    return Ok(None);
                }
                Ok(Some(Built::new(Vec::new())))
            }
            // Go `newActionSchedule`: the inner action as `newAPLAction` builds it, whose casts
            // leave the major cooldowns, then the times; one that fails to parse drops both.
            "schedule" => {
                let Some(inner) = self.action(action.message("inner_action"))? else {
                    return Ok(None);
                };
                let parses = action
                    .str("schedule")
                    .split(',')
                    .all(|timing| crate::rotation::parse_go_duration(timing.trim()).is_some());
                if !parses {
                    return Ok(None);
                }
                Ok(Some(Built::new(inner.casts)))
            }
            // Go `newActionCastAllStatBuffCooldowns` always builds the action; the cooldowns it
            // casts leave the major cooldowns once the rotation is finalized.
            "cast_all_stat_buff_cooldowns" => Ok(Some(Built::new(Vec::new()))),
            // Go `newActionActivateAllStatBuffProcAuras`: none without an item proc buff of the
            // stats in the set, or for Swap1 without an item swap.
            "activate_all_stat_buff_proc_auras" => {
                let swap1 = action.enum_name("swap_set") == "Swap1";
                if swap1 && self.env.sim.item_swap(self.env.player).is_none() {
                    return Ok(None);
                }
                let stats: Vec<i32> = ["stat_type1", "stat_type2", "stat_type3"]
                    .iter()
                    .map(|field| action.i32(field))
                    .filter(|stat| *stat >= 0)
                    .collect();
                let any = self
                    .env
                    .sim
                    .character(self.env.player)
                    .item_proc_buffs
                    .iter()
                    .any(|buff| {
                        buff.is_swapped == swap1
                            && buff
                                .buffed_stat_types
                                .iter()
                                .any(|stat| stats.contains(&(*stat as i32)))
                    });
                Ok(any.then(|| Built::new(Vec::new())))
            }
            // Go `newActionResetSequence`: no action without a sequence name.
            "reset_sequence" => {
                if action.str("sequence_name").is_empty() {
                    return Ok(None);
                }
                Ok(Some(Built::new(Vec::new())))
            }
            // Go `newActionGroupReference`: no action without a group name. The group's own
            // actions are built with the groups.
            "group_reference" => {
                if action.str("group_name").is_empty() {
                    return Ok(None);
                }
                Ok(Some(Built::new(Vec::new())))
            }
            other => Err(Refusal::new(
                "rotation",
                format!("rotation action {other} is not prepared yet"),
            )),
        }
    }
}

/// Every `proto.APLValue` inside an action, depth first, leaving out its inner actions, which
/// Go builds, or drops, on their own.
fn values<'a>(message: &'a Message, out: &mut Vec<&'a Message>) {
    if message.type_name() == "proto.APLValue" {
        out.push(message);
    }
    let walk = |inner: &'a Message, out: &mut Vec<&'a Message>| {
        if inner.type_name() != "proto.APLAction" {
            values(inner, out);
        }
    };
    for name in message.set_fields() {
        match message.get(name) {
            Some(Value::Message(inner)) => walk(inner, out),
            Some(Value::List(items)) => {
                for item in items {
                    if let Value::Message(inner) = item {
                        walk(inner, out);
                    }
                }
            }
            _ => {}
        }
    }
}

/// apl_values_aura.go `newValueAuraNumStacks`: a stack change and a reset callback on the aura
/// the value reads. Go reads an aura the unit lacks as the constant 0 and registers nothing,
/// and drops the value of an aura that does not stack.
fn observe_stacks(env: &mut Environment, unit: UnitId, id: &ActionId) {
    let aura = env
        .sim
        .unit(unit)
        .auras
        .iter()
        .copied()
        .find(|aura| env.sim.aura(*aura).action_id.as_ref() == Some(id));
    if let Some(aura) = aura.filter(|aura| env.sim.aura(*aura).max_stacks > 0) {
        env.sim.apply_on_stacks_change(
            aura,
            std::rc::Rc::new(|_: &mut super::sim::Sim, _, _, _| {}),
        );
        env.sim
            .apply_on_reset(aura, std::rc::Rc::new(|_: &mut super::sim::Sim, _| {}));
    }
}

/// What building the rotation's values registers: apl_values_aura.go `newValueAuraNumStacks`
/// adds a stack change and a reset callback to the aura it reads, and apl_values_dot.go's base
/// value reads register an aura, which preparation refuses for now.
fn register_value_observers(env: &mut Environment, built: &[Message]) -> Result<(), Refusal> {
    for value in built {
        let Some((kind, Value::Message(config))) = value.oneof("value") else {
            continue;
        };
        match kind {
            "aura_num_stacks" => {
                let Some(id) = config.message("aura_id") else {
                    continue;
                };
                let id = proto_to_action_id(id);
                let unit = match source_unit(env, config.message("source_unit")) {
                    Ok(Some(unit)) => unit,
                    // Go reads a unit it cannot resolve as the constant 0 and registers nothing.
                    Ok(None) => continue,
                    Err(()) => {
                        let kind = config
                            .message("source_unit")
                            .map(|u| u.enum_name("type"))
                            .unwrap_or_default();
                        return Err(Refusal::new(
                            "rotation",
                            format!("an aura stack read on a {kind} unit is not prepared yet"),
                        ));
                    }
                };
                observe_stacks(env, unit, &id);
            }
            "dot_percent_increase"
            | "dot_crit_percent_increase"
            | "dot_tick_rate_percent_increase"
                if config.bool("use_base_value") =>
            {
                return Err(Refusal::new(
                    "rotation",
                    "a dot increase read against its base value is not prepared yet".to_string(),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Every `dotBaseDuration` value anywhere in the rotation, as the exporter finds them.
fn dot_base_duration_values<'a>(message: &'a Message, out: &mut Vec<&'a Message>) {
    if message.type_name() == "proto.APLValueDotBaseDuration" {
        out.push(message);
        return;
    }
    for name in message.set_fields() {
        match message.get(name) {
            Some(Value::Message(inner)) => dot_base_duration_values(inner, out),
            Some(Value::List(items)) => {
                for item in items {
                    if let Value::Message(inner) = item {
                        dot_base_duration_values(inner, out);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Go `APLValueDotBaseDuration`'s `baseDuration`, which `newValueDotBaseDuration` reads from
/// `GetAPLDot` on the first target as the rotation is built: the spell's area dot, else its
/// dot on the target. A spell with no such dot gives no value and is left out.
fn rotation_dot_base_durations(
    env: &Environment,
    rotation: &Message,
) -> Vec<crate::contracts::prepared_v2::RotationDotBaseDuration> {
    let mut values = Vec::new();
    dot_base_duration_values(rotation, &mut values);
    let mut durations: Vec<crate::contracts::prepared_v2::RotationDotBaseDuration> = Vec::new();
    for value in values {
        let Some(id) = value.message("spell_id") else {
            continue;
        };
        let id = proto_to_action_id(id);
        if durations.iter().any(|known| known.spell == id) {
            continue;
        }
        let Some(spell) = apl_spell(env, &id) else {
            continue;
        };
        let dot = env.sim.spell(spell).aoe_dot.or_else(|| {
            let target = *env.encounter.targets.first()?;
            spell_dot_of(env, spell, target)
        });
        let Some(dot) = dot else {
            continue;
        };
        let dot = &env.sim.dots[dot.0];
        // Go `Dot.BaseDuration`.
        let base_duration = (f64::from(dot.base_tick_count)
            * dot.base_tick_length as f64
            * dot.base_duration_multiplier) as i64
            + dot.base_duration_flat;
        durations.push(crate::contracts::prepared_v2::RotationDotBaseDuration {
            spell: id,
            base_duration_ns: base_duration,
        });
    }
    durations.sort_by(|a, b| a.spell.cmp(&b.spell));
    durations
}

/// Go `spell.Dot(target)`, following the related dot spell.
fn spell_dot_of(env: &Environment, spell: SpellId, target: UnitId) -> Option<super::sim::DotId> {
    let s = env.sim.spell(spell);
    if s.dots.is_empty() {
        return spell_dot_of(env, s.related_dot_spell?, target);
    }
    let index = env.sim.unit(target).unit_index as usize;
    s.dots.get(index).copied().flatten()
}

/// The unit a parsed reference names as the rotation is built, `None` where Go finds none.
fn unit_of(env: &Environment, reference: crate::rotation::UnitRef) -> Option<UnitId> {
    use crate::rotation::{Unit as Resolved, UnitRef};
    let targets = &env.encounter.targets;
    match reference.resolve(targets.len())? {
        Resolved::Player => Some(env.player),
        Resolved::Target(position) => match reference {
            UnitRef::CurrentTarget => env.sim.unit(env.player).current_target,
            UnitRef::NextTarget => neighbour_target(env, true),
            UnitRef::PreviousTarget => neighbour_target(env, false),
            _ => targets.get(position).copied(),
        },
    }
}

/// The spells a group's action removes from the major cooldowns because it casts them, and the
/// ones of a strict sequence Go drops for a missing step, which it still removes.
fn group_casts(
    env: &Environment,
    action: &crate::rotation::Action,
    casts: &mut Vec<SpellId>,
    pruned: &mut Vec<SpellId>,
) {
    use crate::rotation::Action;
    let targets = env.encounter.targets.len();
    match action {
        Action::CastSpell { spell, target } if target.resolve(targets).is_some() => {
            casts.extend(apl_cast_spell(env, spell));
        }
        Action::CastAtPlayer(spell) => casts.extend(apl_cast_spell(env, spell)),
        Action::ChannelSpell {
            spell,
            target,
            interrupt_if: None,
            ..
        } if target.resolve(targets).is_some() => casts.extend(apl_cast_spell(env, spell)),
        Action::Schedule { inner, .. } => group_casts(env, &inner.action, casts, pruned),
        Action::Sequence { steps, .. } => casts.extend(steps.iter().filter_map(|step| {
            step.target
                .resolve(targets)
                .and_then(|_| apl_cast_spell(env, &step.spell))
        })),
        // A change of target casts nothing; Go builds it for a unit it finds.
        Action::StrictSequence(steps) => {
            let found: Vec<Option<Option<SpellId>>> = steps
                .iter()
                .map(|step| {
                    step.target.resolve(targets).and_then(|_| {
                        if step.change_target {
                            Some(None)
                        } else {
                            apl_cast_spell(env, &step.spell).map(Some)
                        }
                    })
                })
                .collect();
            if found.iter().all(Option::is_some) {
                casts.extend(found.into_iter().flatten().flatten());
            } else {
                pruned.extend(found.into_iter().flatten().flatten());
            }
        }
        _ => {}
    }
}

/// What building the groups and the variables they read adds, from the rotation parsed and its
/// references bound: the casts removed from the major cooldowns, and the stack reads' callbacks
/// on the auras. Go builds every group, referenced or not, and a variable's value where it is
/// read, in a group's condition before the group's own value replaces it.
fn build_groups(
    env: &mut Environment,
    rotation: &Message,
    live: &[bool],
) -> Result<(Vec<SpellId>, Vec<SpellId>), Refusal> {
    let refuse = |reason: String| Refusal::new("rotation", reason);
    let parsed = crate::rotation::parse(&rotation.to_protojson()).map_err(|reasons| {
        refuse(format!(
            "rotation groups and variables with {} are not prepared yet",
            reasons.join("; ")
        ))
    })?;
    let (_, binding) =
        crate::rotation::bind(&parsed, &|index| live.get(index).copied() == Some(true))
            .map_err(refuse)?;
    let mut casts = Vec::new();
    let mut pruned = Vec::new();
    let mut reads = Vec::new();
    // Each group as written, whose conditions Go builds with the rotation's own variables, and
    // each instance after its reference filled it.
    let written = parsed.groups.iter().flat_map(|group| group.items.iter());
    let bound = binding
        .instances
        .iter()
        .flat_map(|instance| instance.items.iter());
    for item in written.chain(bound) {
        let mut visit = |value: &crate::rotation::Value| {
            use crate::rotation::{AuraRead, Value as V};
            match value {
                V::AuraNumStacks(id) => reads.push((crate::rotation::UnitRef::Player, id.clone())),
                V::TargetAura {
                    read: AuraRead::NumStacks | AuraRead::NumStacksAfterReaction,
                    unit,
                    id,
                } => reads.push((*unit, id.clone())),
                _ => {}
            }
        };
        if let Some(condition) = &item.condition {
            condition.visit(&mut visit);
        }
        for value in item.action.values() {
            value.visit(&mut visit);
        }
    }
    for instance in &binding.instances {
        for item in &instance.items {
            group_casts(env, &item.action, &mut casts, &mut pruned);
        }
    }
    for (reference, id) in reads {
        if let Some(unit) = unit_of(env, reference) {
            observe_stacks(env, unit, &id);
        }
    }
    Ok((casts, pruned))
}

/// Go `newAPLRotation` at finalization.
pub(crate) fn build_rotation(
    env: &mut Environment,
    rotation: Option<&Message>,
) -> Result<(), Refusal> {
    let Some(rotation) = rotation else {
        return Ok(());
    };
    // Go reads the lists whatever the rotation's type. The UI sends `TypeAuto` for a preset it
    // has not edited, with the preset's list in full, so it runs exactly as `TypeAPL`.
    if !matches!(rotation.enum_name("type").as_str(), "TypeAPL" | "TypeAuto") {
        return Err(Refusal::new(
            "rotation",
            "only APL rotations are prepared".to_string(),
        ));
    }
    let mut prepull = 0;
    let mut casts = Vec::new();
    let mut live = Vec::new();
    let mut pruned;
    let built_values;
    {
        let mut builder = Builder {
            env,
            variables: rotation
                .messages("value_variables")
                .into_iter()
                .filter_map(|variable| {
                    Some((
                        variable.str("name").to_string(),
                        variable.message("value")?.clone(),
                    ))
                })
                .collect(),
            pruned_casts: Vec::new(),
            built_values: Vec::new(),
        };
        // apl.go: the set the last prepull item swap Go builds goes to.
        let mut last_swap: Option<String> = None;
        for item in rotation.messages("prepull_actions") {
            if item.bool("hide") {
                continue;
            }
            let swap = item
                .message("action")
                .and_then(|action| action.message("item_swap"))
                .map(|swap| swap.enum_name("swap_set"));
            let Some(at) = item.message("do_at_value") else {
                continue;
            };
            let Some(Value::Message(constant)) = at.get("const") else {
                return Err(Refusal::new(
                    "rotation",
                    "a prepull time that is not a constant".to_string(),
                ));
            };
            let do_at = crate::rotation::parse_const(constant.str("val"))
                .map_err(|err| Refusal::new("rotation", err))?
                .duration_ns;
            if do_at > 0 {
                continue;
            }
            if builder.action(item.message("action"))?.is_some() {
                prepull += 1;
                if let Some(set) = swap {
                    last_swap = Some(set);
                }
            }
        }
        // apl.go: Go swaps back to Main a nanosecond before the pull when the prepull's last
        // item swap goes to another set.
        if last_swap.is_some_and(|set| set != "Main") {
            prepull += 1;
        }
        for item in rotation.messages("priority_list") {
            if item.bool("hide") {
                continue;
            }
            let built = builder.action(item.message("action"))?;
            live.push(built.is_some());
            if let Some(built) = built {
                casts.extend(built.casts);
            }
        }
        pruned = builder.pruned_casts;
        built_values = builder.built_values;
    }
    register_value_observers(env, &built_values)?;
    if !rotation.messages("groups").is_empty() || !rotation.messages("value_variables").is_empty() {
        let (group_casts, group_pruned) = build_groups(env, rotation, &live)?;
        casts.extend(group_casts);
        pruned.extend(group_pruned);
    }
    env.rotation_dot_base_durations = rotation_dot_base_durations(env, rotation);
    env.rotation_reads = rotation_reads(env, rotation);
    env.prepull_actions += prepull;
    let player = env.player;
    for spell in casts.into_iter().chain(pruned) {
        let action = env.sim.spell(spell).action_id.clone();
        env.sim.remove_initial_major_cooldown(player, &action);
    }
    cast_all_stat_buff_cooldowns(env, rotation);
    Ok(())
}

/// Every value message of the rotation, depth first.
fn all_values<'a>(message: &'a Message, out: &mut Vec<&'a Message>) {
    if message.type_name() == "proto.APLValue" {
        out.push(message);
    }
    for name in message.set_fields() {
        match message.get(name) {
            Some(Value::Message(inner)) => all_values(inner, out),
            Some(Value::List(items)) => {
                for item in items {
                    if let Value::Message(inner) = item {
                        all_values(inner, out);
                    }
                }
            }
            _ => {}
        }
    }
}

/// apl_values_boss.go `GetTargetUnit` of a boss value: a target for no reference, the current,
/// next, previous or an indexed one; none for the player or a set, which read no target.
fn boss_value_target(env: &Environment, reference: Option<&Message>) -> Option<UnitId> {
    let targets = &env.encounter.targets;
    let first = *targets.first()?;
    let Some(reference) = reference else {
        return Some(first);
    };
    match reference.enum_name("type").as_str() {
        "Unknown" | "CurrentTarget" | "NextTarget" | "PreviousTarget" => Some(first),
        "Target" => usize::try_from(reference.i32("index"))
            .ok()
            .and_then(|index| targets.get(index).copied()),
        _ => None,
    }
}

/// The exporter's `rotationReads`: what the rotation's values read that the rest of the export
/// leaves out, or none when no value reads any of it.
fn rotation_reads(
    env: &Environment,
    rotation: &Message,
) -> Option<crate::contracts::prepared_v2::RotationReads> {
    use crate::contracts::prepared_v2::{
        RotationCooldownBuffs, RotationItemProcBuff, RotationReads, RotationSelfBuff,
        RotationTargetSpell,
    };
    let mut values = Vec::new();
    all_values(rotation, &mut values);
    let mut reads = RotationReads::default();
    let mut used = false;
    let mut item_procs = false;
    let mut cooldown_keys: Vec<Vec<i32>> = Vec::new();
    let stat_key = |config: &Message| -> Vec<i32> {
        ["stat_type1", "stat_type2", "stat_type3"]
            .iter()
            .map(|field| config.i32(field))
            .filter(|stat| *stat >= 0)
            .collect()
    };
    // The prepull's activateAllStatBuffProcAuras reads the item proc buffs too, and
    // castAllStatBuffCooldowns names its stats; both are actions.
    let mut actions = Vec::new();
    all_actions(rotation, &mut actions);
    for action in &actions {
        match action.oneof("action") {
            Some(("activate_all_stat_buff_proc_auras", _)) => {
                item_procs = true;
                used = true;
            }
            Some(("cast_all_stat_buff_cooldowns", _)) => used = true,
            _ => {}
        }
    }
    let consumables = &env.sim.character(env.player).consumables;
    for value in values {
        let Some((kind, Value::Message(config))) = value.oneof("value") else {
            continue;
        };
        match kind {
            "all_item_stat_procs_active"
            | "any_item_stat_procs_active"
            | "any_item_stat_procs_available"
            | "item_procs_min_remaining_time"
            | "item_procs_max_remaining_icd"
            | "num_equipped_stat_proc_items" => {
                item_procs = true;
                used = true;
            }
            "num_stat_buff_cooldowns"
            | "any_stat_buff_cooldowns_active"
            | "any_stat_buff_cooldowns_min_duration" => {
                let key = stat_key(config);
                if !cooldown_keys.contains(&key) {
                    cooldown_keys.push(key);
                }
                used = true;
            }
            "selected_potion" => {
                reads.potion_id = Some(consumables.i32("pot_id"));
                used = true;
            }
            "selected_conjured" => {
                reads.conjured_id = Some(consumables.i32("conjured_id"));
                used = true;
            }
            "boss_spell_is_casting" | "boss_spell_time_to_ready" => {
                let (Some(id), Some(target)) = (
                    config.message("spell_id"),
                    boss_value_target(env, config.message("target_unit")),
                ) else {
                    continue;
                };
                let id = proto_to_action_id(id);
                let Some(spell) = env.sim.get_spell(target, &id) else {
                    continue;
                };
                if reads.target_spells.iter().any(|known| known.spell == id) {
                    continue;
                }
                let spell = env.sim.spell(spell);
                reads.target_spells.push(RotationTargetSpell {
                    spell: id,
                    has_cooldown: spell.cd.timer.is_some() || spell.shared_cd.timer.is_some(),
                });
                used = true;
            }
            "multiple_cd_usages" => {
                for field in ["base_spell_id", "target_spell_id"] {
                    let Some(id) = config.message(field) else {
                        continue;
                    };
                    let Some(spell) = apl_spell(env, &proto_to_action_id(id)) else {
                        continue;
                    };
                    let state = env.sim.spell(spell);
                    let Some(aura) = state.related_self_buff else {
                        continue;
                    };
                    if reads
                        .self_buffs
                        .iter()
                        .any(|known| known.spell == state.action_id)
                    {
                        continue;
                    }
                    reads.self_buffs.push(RotationSelfBuff {
                        spell: state.action_id.clone(),
                        aura: env.sim.aura(aura).label.clone(),
                        has_cooldown: state.cd.timer.is_some(),
                    });
                    used = true;
                }
            }
            _ => {}
        }
    }
    if !used {
        return None;
    }
    if item_procs {
        for buff in &env.sim.character(env.player).item_proc_buffs {
            reads.item_proc_buffs.push(RotationItemProcBuff {
                aura: env.sim.aura(buff.aura).label.clone(),
                stats: buff
                    .buffed_stat_types
                    .iter()
                    .map(|stat| *stat as i32)
                    .collect(),
                swapped: buff.is_swapped,
            });
        }
    }
    cooldown_keys.sort();
    for key in cooldown_keys {
        let (spells, auras) = matching_stat_buff_cooldowns(env, &key);
        reads.cooldown_buffs.push(RotationCooldownBuffs {
            stats: key,
            spells: spells
                .iter()
                .map(|spell| env.sim.spell(*spell).action_id.clone())
                .collect(),
            auras: auras
                .iter()
                .map(|aura| env.sim.aura(*aura).label.clone())
                .collect(),
        });
    }
    let key = |id: &ActionId| (id.spell_id, id.item_id, id.other_id.clone(), id.tag);
    reads.target_spells.sort_by_key(|entry| key(&entry.spell));
    reads.self_buffs.sort_by_key(|entry| key(&entry.spell));
    Some(reads)
}

/// Every action message of the rotation, depth first.
fn all_actions<'a>(message: &'a Message, out: &mut Vec<&'a Message>) {
    if message.type_name() == "proto.APLAction" {
        out.push(message);
    }
    for name in message.set_fields() {
        match message.get(name) {
            Some(Value::Message(inner)) => all_actions(inner, out),
            Some(Value::List(items)) => {
                for item in items {
                    if let Value::Message(inner) = item {
                        all_actions(inner, out);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Go `majorCooldownManager.GetMatchingStatBuffSpells` and `GetMatchingStatBuffCooldownAuras`:
/// the instant major cooldowns whose buff aura moves one of the stats, as spells and auras.
fn matching_stat_buff_cooldowns(
    env: &Environment,
    stats: &[i32],
) -> (Vec<SpellId>, Vec<super::sim::AuraId>) {
    let character = env.sim.character(env.player);
    let mut spells = Vec::new();
    let mut auras = Vec::new();
    for mcd in &character.initial_major_cooldowns {
        let Some((_, buff)) = character
            .cooldown_buff_auras
            .iter()
            .find(|(spell, _)| *spell == mcd.spell)
        else {
            continue;
        };
        let matches = buff
            .buffed_stat_types
            .iter()
            .any(|stat| stats.contains(&(*stat as i32)));
        if matches && env.sim.spell(mcd.spell).default_cast.effective_time() == 0 {
            spells.push(mcd.spell);
            auras.push(buff.aura);
        }
    }
    (spells, auras)
}

/// Go `APLActionCastAllStatBuffCooldowns.processMajorCooldowns`, after the rotation took the
/// cooldowns it casts: each action's spells, by its stats, which then leave the major cooldowns.
fn cast_all_stat_buff_cooldowns(env: &mut Environment, rotation: &Message) {
    let mut actions = Vec::new();
    all_actions(rotation, &mut actions);
    let mut keys: Vec<Vec<i32>> = Vec::new();
    for action in actions {
        let Some(("cast_all_stat_buff_cooldowns", Value::Message(config))) = action.oneof("action")
        else {
            continue;
        };
        let key: Vec<i32> = ["stat_type1", "stat_type2", "stat_type3"]
            .iter()
            .map(|field| config.i32(field))
            .filter(|stat| *stat >= 0)
            .collect();
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    if keys.is_empty() {
        return;
    }
    keys.sort();
    let mut casts = Vec::new();
    let mut removed = Vec::new();
    for key in keys {
        let (spells, _) = matching_stat_buff_cooldowns(env, &key);
        removed.extend(spells.iter().copied());
        casts.push(crate::contracts::prepared_v2::RotationCooldownBuffs {
            stats: key,
            spells: spells
                .iter()
                .map(|spell| env.sim.spell(*spell).action_id.clone())
                .collect(),
            auras: Vec::new(),
        });
    }
    let player = env.player;
    for spell in removed {
        let action = env.sim.spell(spell).action_id.clone();
        env.sim.remove_initial_major_cooldown(player, &action);
    }
    if let Some(reads) = env.rotation_reads.as_mut() {
        reads.cast_cooldowns = casts;
    }
}
