//! The class side of preparation: Go's `Agent` interface and the oracle exporter's per-class
//! `classExport`, in one trait each class implements under `src/classes/<class>/prepare`.

use crate::contracts::prepared_v2::ActionId;
use crate::contracts::request::Message;

use super::sim::{Sim, UnitId};

/// Go `ProtoToActionID`.
pub(crate) fn proto_to_action_id(id: &Message) -> ActionId {
    let mut out = ActionId {
        tag: id.i32("tag"),
        ..ActionId::default()
    };
    if let Some((field, _)) = id.oneof("raw_id") {
        match field {
            "spell_id" => out.spell_id = id.i32("spell_id"),
            "item_id" => out.item_id = id.i32("item_id"),
            "other_id" => {
                let number = id.enum_number("other_id");
                if number != 0 {
                    out.other_id = id.enum_name("other_id");
                }
            }
            _ => {}
        }
    }
    out
}

/// A class mask bit's stable name, as the exporter's `classSpellName`.
pub(crate) struct ClassSpellName {
    pub mask: i64,
    pub name: &'static str,
}

/// A copy of a class agent, so an [`Environment`](super::env::Environment) can be copied with it.
pub(crate) trait CloneAgent {
    fn clone_agent(&self) -> Box<dyn PrepAgent>;
}

impl<T: PrepAgent + Clone + 'static> CloneAgent for T {
    fn clone_agent(&self) -> Box<dyn PrepAgent> {
        Box::new(self.clone())
    }
}

impl Clone for Box<dyn PrepAgent> {
    fn clone(&self) -> Self {
        self.clone_agent()
    }
}

/// One class: construction and initialization as Go's agent runs them, and the exporter's
/// description of what Go keeps in closures.
pub(crate) trait PrepAgent: CloneAgent {
    /// Go `Agent.AddRaidBuffs`: the buffs the class brings to the raid.
    fn add_raid_buffs(&self, _raid_buffs: &mut Message) {}
    /// Go `Agent.AddPartyBuffs`.
    fn add_party_buffs(&self, _sim: &Sim, _party_buffs: &mut Message) {}
    /// What the agent's constructor does to the raid's debuffs the request carries (Go's
    /// `NewWarlock` clears the curse the warlock casts itself), before they are applied.
    fn adjust_raid_debuffs(&self, _debuffs: &mut Message) {}
    /// Go `Agent.ApplyTalents`.
    fn apply_talents(&mut self, _sim: &mut Sim, _unit: UnitId) {}
    /// Go `Agent.Initialize`.
    fn initialize(&mut self, _sim: &mut Sim, _unit: UnitId) {}
    /// A class's item swap callback that rebuilds what swings for the weapon now in the main
    /// hand: a feral druid's paw (druid.go Initialize).
    fn item_swap_main_hand(&mut self, _sim: &mut Sim, _unit: UnitId) {}
    /// Go `Agent.Reset`.
    fn reset(&mut self, _sim: &mut Sim, _unit: UnitId) {}
    /// A `core.NewItemEffect` the class package registers for an item: applies it and answers
    /// true, or answers false for an item the class registers no effect for. Called after the
    /// shared effects (`items_registry`), in Go's equipment order.
    fn apply_item_effect(&mut self, _sim: &mut Sim, _unit: UnitId, _item: i32) -> bool {
        false
    }
    /// A `core.NewEnchantEffect` the class package registers for an enchant.
    fn apply_enchant_effect(&mut self, _sim: &mut Sim, _unit: UnitId, _enchant: i32) -> bool {
        false
    }

    /// The talents proto the class filled from the talent string.
    fn talents(&self) -> &Message;
    /// Stable names for the class's spell mask bits.
    fn class_spells(&self) -> &'static [ClassSpellName];
    /// The exporter's class effects, in its order.
    fn effects(&self, _sim: &Sim, _unit: UnitId) -> Vec<serde_json::Value> {
        Vec::new()
    }
    /// Stable names for class spells Go registers without a class mask, by action.
    fn unmasked_spell(&self, _id: &ActionId) -> Option<&'static str> {
        None
    }
    /// Whether the class's main hand swing replacement always keeps the swing it is given.
    fn swing_replacement_keeps_swing(&self) -> bool {
        false
    }
    /// Go `EurekaAgent.EurekaSpells`: the spell masks the Gnome racial Eureka! names for the
    /// class. A class that does not implement it leaves the racial on every class spell.
    fn eureka_spells(&self) -> Option<super::racials::EurekaSpells> {
        None
    }
    /// Go `Agent.NewAPLAction`, asked first for every rotation action: `Some(true)` when the
    /// class builds the action itself, `Some(false)` when it builds it as nil, `None` to leave
    /// it to the core actions.
    fn custom_apl_action(&self, _sim: &Sim, _unit: UnitId, _action: &Message) -> Option<bool> {
        None
    }
    /// Whether a spell is one of the class's mana gems, which the class describes itself:
    /// the item loop skips `spell.Matches(mage.MageSpellManaGem)`.
    fn is_mana_gem(&self, _sim: &Sim, _spell: super::sim::SpellId) -> bool {
        false
    }
    /// The effects `RegisterPostFinalizeEffect` calls registered while the class applied its
    /// talents, taken once for the environment to run after finalization.
    fn take_post_finalize_effects(&mut self) -> Vec<super::env::FinalizeEffect> {
        Vec::new()
    }
    /// Go `PetAgent.Initialize`: the class half of a pet's initialization, run after the
    /// player's.
    fn initialize_pet(&mut self, _sim: &mut Sim, _pet: UnitId) {}
    /// Go `PetAgent.Reset`: run when the pet resets, before the pet is enabled.
    fn reset_pet(&mut self, _sim: &mut Sim, _pet: UnitId) {}
    /// The exporter's `resetOnlyPetClasses`: the class's pets are enabled only at reset, never
    /// summoned, dismissed or expired during a fight.
    fn reset_only_pets(&self) -> bool {
        false
    }
    /// The exporter's `classSummonedPets`: whether a class effect summons the pet.
    fn summoned_pet(&self, _sim: &Sim, _pet: UnitId) -> bool {
        false
    }
    /// The exporter's `statCallbackPets`: whether the pet's enable and disable callbacks only
    /// add stats.
    fn stat_callback_pet(&self, _sim: &Sim, _pet: UnitId) -> bool {
        false
    }
    /// The exporter's `classExport.inertPet`: why a registered pet never acts in this build, or
    /// `None` when it may.
    fn inert_pet(&self, _sim: &Sim, _pet: UnitId) -> Option<&'static str> {
        None
    }
    /// The exporter's `classExport.statAuras`: the labels of the class auras whose gain and
    /// expiry change stats through `AddStatsDynamic`.
    fn stat_auras(&self, _sim: &Sim, _unit: UnitId) -> Vec<String> {
        Vec::new()
    }
    /// The exporter's `classExport.damageTakenModifiers`: how many of the target's dynamic
    /// damage taken modifiers the class effects describe.
    fn damage_taken_modifiers(&self) -> usize {
        0
    }
    /// Go `classExport.unrepresented`: class behavior the effects cannot describe, one reason
    /// each.
    fn unrepresented(&self, _sim: &Sim, _unit: UnitId) -> Vec<String> {
        Vec::new()
    }
    /// The client damage roll of a spell, `{average, variance}`, for the spells the class names.
    fn damage_effect(&self, _sim: &Sim, _spell: super::sim::SpellId) -> Option<serde_json::Value> {
        None
    }
    /// Go `classItemUseEffects`: the exported effect of an item use the class package registers
    /// itself, keyed by the item. It is the item loop's last case, after the shared ones.
    fn class_item_use_effect(
        &self,
        _env: &super::env::Environment,
        _spell: super::sim::SpellId,
        _item: i32,
    ) -> Option<serde_json::Value> {
        None
    }
    /// Go `buffs.SetFlametongueAttackTraits`: what the class's own Flametongue Attack carries,
    /// which the party's Flametongue Totem hit takes. Any other class has none.
    fn flametongue_attack_traits(&self) -> super::buffs::flametongue::FlametongueAttackTraits {
        super::buffs::flametongue::FlametongueAttackTraits::default()
    }
    /// The agent as `Any`, for a class's item set bonus that sets state on its own agent (Go
    /// hands a set bonus the agent). A class that has no such bonus answers `None`.
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        None
    }
    /// The exporter's class effects when they need more than the simulation, such as the
    /// attack tables and the target, and the notes a class effect adds to the unrepresented
    /// list (`classNotes`). The default is `effects`.
    fn export_effects(
        &self,
        env: &super::env::Environment,
        _notes: &mut Vec<String>,
    ) -> Vec<serde_json::Value> {
        self.effects(&env.sim, env.player)
    }
}

/// Go `FillTalentsProto`: each digit sets the field numbered by its position, counting each
/// tree from the sum of the earlier trees' sizes.
pub(crate) fn fill_talents(
    type_name: &str,
    talents: &str,
    tree_sizes: [usize; 3],
) -> Result<Message, String> {
    use serde_json::{Map, Value};
    let mut object = Map::new();
    let mut offset = 0;
    for (tree, digits) in talents.split('-').enumerate() {
        for (i, digit) in digits.chars().enumerate() {
            // Go: strconv.Atoi, whose error is ignored, reads a non-digit as 0.
            let points = digit.to_digit(10).map_or(0, i64::from);
            let number = (offset + i + 1) as u32;
            let (name, is_bool) = crate::contracts::request::field_by_number(type_name, number)
                .ok_or_else(|| {
                    format!(
                        "Couldn't find proto field for talent #{number}, full string: {talents}"
                    )
                })?;
            let value = if is_bool {
                Value::Bool(points == 1)
            } else {
                Value::from(points)
            };
            object.insert(name, value);
        }
        offset += tree_sizes.get(tree).copied().unwrap_or(0);
    }
    Message::from_json_text(type_name, &Value::Object(object).to_string())
}

/// [`fill_talents`] for a string in a client's own talent order rather than Go's field numbering:
/// `field` names the field at a tree's position (both counted from 0). A client build that moves
/// or removes talents changes that order while the proto's numbers stay Go's.
pub(crate) fn fill_talents_by_layout(
    type_name: &str,
    talents: &str,
    field: impl Fn(usize, usize) -> Option<&'static str>,
) -> Result<Message, String> {
    use serde_json::{Map, Value};
    let mut object = Map::new();
    for (tree, digits) in talents.split('-').enumerate() {
        for (i, digit) in digits.chars().enumerate() {
            // As Go reads a non-digit: 0.
            let points = digit.to_digit(10).map_or(0, i64::from);
            let field = field(tree, i).ok_or_else(|| {
                format!(
                    "no talent at tree {} position {}, full string: {talents}",
                    tree + 1,
                    i + 1
                )
            })?;
            let (name, is_bool) = crate::contracts::request::field_by_name(type_name, field)
                .ok_or_else(|| format!("{type_name} has no field {field}"))?;
            let value = if is_bool {
                Value::Bool(points == 1)
            } else {
                Value::from(points)
            };
            object.insert(name, value);
        }
    }
    Message::from_json_text(type_name, &Value::Object(object).to_string())
}

/// [`fill_talents_by_layout`] for a layout that also names talents the pinned proto does not
/// have, such as those a client hotfix adds after the reference: a field the proto lacks is
/// returned with its points beside the message instead of being refused.
pub(crate) fn fill_talents_by_layout_with_extras(
    type_name: &str,
    talents: &str,
    field: impl Fn(usize, usize) -> Option<&'static str>,
) -> Result<(Message, std::collections::BTreeMap<&'static str, i64>), String> {
    use serde_json::{Map, Value};
    let mut object = Map::new();
    let mut extras = std::collections::BTreeMap::new();
    for (tree, digits) in talents.split('-').enumerate() {
        for (i, digit) in digits.chars().enumerate() {
            // As Go reads a non-digit: 0.
            let points = digit.to_digit(10).map_or(0, i64::from);
            let field = field(tree, i).ok_or_else(|| {
                format!(
                    "no talent at tree {} position {}, full string: {talents}",
                    tree + 1,
                    i + 1
                )
            })?;
            match crate::contracts::request::field_by_name(type_name, field) {
                Some((name, is_bool)) => {
                    let value = if is_bool {
                        Value::Bool(points == 1)
                    } else {
                        Value::from(points)
                    };
                    object.insert(name, value);
                }
                None => {
                    extras.insert(field, points);
                }
            }
        }
    }
    let message = Message::from_json_text(type_name, &Value::Object(object).to_string())?;
    Ok((message, extras))
}
