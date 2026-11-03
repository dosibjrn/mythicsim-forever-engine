//! Strict prepared v2 wire types: a reset Go simulation described as data.
//!
//! The pinned Go engine prepares the character, gear, buffs and talents, resets one
//! simulation and exports the resulting state with `tools/oracle-v2`. Static modifiers
//! are already applied to stats and spells. Behavior Rust must execute is named in
//! `effects` with the parameters Go keeps in closures. See docs/prepared-v2.md.
//!
//! Every struct rejects unknown fields and every effect kind is an explicit variant, so
//! a new Go effect or field fails deserialization instead of disappearing.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The `schema_version` that identifies this contract.
pub const SCHEMA_VERSION: u32 = 2;
/// The `contract` name that distinguishes prepared inputs from other v2 documents.
pub const CONTRACT: &str = "forever-prepared";
/// Go's `NeverExpires` duration, used for permanent auras.
pub const NEVER_EXPIRES_NS: i64 = i64::MAX;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedV2 {
    pub schema_version: u32,
    pub contract: String,
    pub reference: Reference,
    /// SHA-256 of the deterministic protobuf encoding of the original request.
    pub request_sha256: String,
    pub scenario_id: String,
    pub sim: SimOptions,
    pub encounter: Encounter,
    pub target: Target,
    pub player: Player,
    pub melee: Melee,
    /// The pet enabled at each reset, which Rust simulates; at most one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pets: Vec<Pet>,
    /// The target's swings at the player when the player tanks it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enemy: Option<Enemy>,
    pub effects: Vec<Effect>,
    /// Request features the exporter could not describe. Must be empty to simulate.
    pub unrepresented: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub engine_revision: String,
    pub client_build: String,
    pub exporter: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SimOptions {
    pub iterations: u32,
    pub seed: i64,
    /// Go `useLabeledRands`: one random stream per label instead of one shared stream.
    pub labeled_rng: bool,
    pub debug_first_iteration: bool,
    /// Go `debug`: log every iteration into one buffer.
    pub debug: bool,
    /// Go `saveAllValues`: report every iteration's value in each distribution's `allValues`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub save_all_values: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Encounter {
    pub duration_ns: i64,
    pub duration_variation_ns: i64,
    pub execute_proportion_20: f64,
    pub execute_proportion_25: f64,
    pub execute_proportion_35: f64,
    pub execute_proportion_45: f64,
    pub execute_proportion_90: f64,
    /// How many targets the fight has when it has several, zero for the usual one. Every
    /// target past the first is an idle copy of `target`: it resets, activates its permanent
    /// auras and reports metrics, but nothing in the supported fight acts on it. Its Go unit
    /// index follows the first's, and the player's index counts it.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub target_count: u32,
}

/// Per-school values in Go's `SchoolIndex` order.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Schools {
    pub none: f64,
    pub physical: f64,
    pub arcane: f64,
    pub fire: f64,
    pub frost: f64,
    pub holy: f64,
    pub nature: f64,
    pub shadow: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PseudoStats {
    pub spell_cost_percent_modifier: i32,
    pub cast_speed_multiplier: f64,
    pub spirit_regen_rate_casting: f64,
    pub force_full_spirit_regen: bool,
    pub spirit_regen_multiplier: f64,
    pub threat_multiplier: f64,
    pub damage_dealt_multiplier: f64,
    pub school_damage_dealt_multiplier: Schools,
    pub dot_damage_multiplier_additive: f64,
    pub crit_damage_multiplier: f64,
    pub damage_taken_multiplier: f64,
    pub school_damage_taken_multiplier: Schools,
    pub school_bonus_spell_damage: Schools,
    pub school_bonus_hit_chance: Schools,
    pub bonus_spell_damage_taken: f64,
    pub bonus_spell_crit_percent_taken: f64,
    pub reduced_crit_taken_percent: f64,
    pub incapacitated: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(deny_unknown_fields)]
pub struct ActionId {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub spell_id: i32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub item_id: i32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub other_id: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tag: i32,
}

fn one() -> f64 {
    1.0
}

fn is_one(value: &f64) -> bool {
    *value == 1.0
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero(value: &i32) -> bool {
    *value == 0
}

fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

fn is_zero_usize(value: &usize) -> bool {
    *value == 0
}

fn is_zero_f64(value: &f64) -> bool {
    *value == 0.0
}

impl ActionId {
    pub fn spell(spell_id: i32) -> Self {
        Self {
            spell_id,
            ..Self::default()
        }
    }

    pub fn item(item_id: i32) -> Self {
        Self {
            item_id,
            ..Self::default()
        }
    }

    /// Go `SameActionIgnoreTag`.
    pub fn same_action_ignore_tag(&self, other: &ActionId) -> bool {
        self.spell_id == other.spell_id
            && self.item_id == other.item_id
            && self.other_id == other.other_id
    }
}

impl std::fmt::Display for ActionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.spell_id != 0 {
            write!(f, "spell {}", self.spell_id)?;
        } else if self.item_id != 0 {
            write!(f, "item {}", self.item_id)?;
        } else {
            write!(f, "{}", self.other_id)?;
        }
        if self.tag != 0 {
            write!(f, " tag {}", self.tag)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Cooldown {
    /// Shared Go timer identity: spells naming the same timer share one cooldown.
    pub timer: String,
    pub duration_ns: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Aura {
    pub label: String,
    pub tag: Option<String>,
    pub action_id: Option<ActionId>,
    pub action_id_for_proc: Option<ActionId>,
    pub duration_ns: i64,
    pub max_stacks: i32,
    /// Active after the reset, which for an `on_reset` aura means permanent.
    pub active: bool,
    pub stacks: i32,
    pub callbacks: Vec<String>,
    pub icd: Option<Cooldown>,
    pub exclusive_effects: u32,
    /// A permanent aura its reset activated and the named later member of its exclusive
    /// category displaced during the same reset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub displaced_by: Option<String>,
    /// A permanent aura an earlier member of its exclusive category blocked at the reset,
    /// which still counted a proc.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub blocked_at_reset: bool,
    /// An action ID set after registration: the aura logs it, but Go lists no metrics for it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub metrics_hidden: bool,
    /// Each of the aura's exclusive effects, in the aura's order. Go's aura metrics report each
    /// effect's uptime under its category.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclusive_memberships: Vec<ExclusiveMembership>,
}

/// An inert pet's permanent aura and the categories of its exclusive effects, in the aura's
/// order. Go's aura metrics report each effect's uptime under its category.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InertPetExclusiveEffects {
    pub aura: ActionId,
    pub categories: Vec<String>,
}

/// One exclusive effect of an aura: its category, whether the category holds a single aura,
/// the effect's bid after the reset and its position among the category's effects, which
/// settles a tie for the highest bid.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExclusiveMembership {
    pub category: String,
    pub single_aura: bool,
    pub priority: f64,
    pub position: u32,
}

/// Callbacks that react to combat events rather than an aura's own lifetime.
pub const EVENT_CALLBACKS: &[&str] = &[
    "on_apply_effects",
    "on_cast_complete",
    "on_spell_hit_dealt",
    "on_spell_hit_taken",
    "on_periodic_damage_dealt",
    "on_periodic_damage_taken",
    "on_heal_dealt",
    "on_heal_taken",
    "on_periodic_heal_dealt",
    "on_periodic_heal_taken",
    "on_encounter_start",
];

impl Aura {
    pub fn has_event_callbacks(&self) -> bool {
        self.callbacks
            .iter()
            .any(|callback| EVENT_CALLBACKS.contains(&callback.as_str()))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub index: i32,
    pub label: String,
    pub level: i32,
    pub mob_type: Option<String>,
    pub stats: BTreeMap<String, f64>,
    pub pseudo_stats: PseudoStats,
    pub auras: Vec<Aura>,
    /// Go rolls an opening swing offset for an enemy with a melee swing at every reset,
    /// even when the enemy never swings.
    pub auto_swing_melee: bool,
    pub auto_swing_ranged: bool,
    /// Registered actions the target reports with zero metrics, since it never acts.
    pub metrics_actions: Vec<MetricsAction>,
    /// Forever's rule: being above the caster's level gives the target no partial resists, so
    /// only resistance past the caster's Spell Piercing does. False is the Go reference, whose
    /// target partially resists non-binary spells and dot ticks by 2% a level on average.
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_level_partial_resist: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Mana {
    pub max: f64,
    pub base: f64,
    pub spirit_regen_per_second: f64,
    /// Go's computed rates at the start of the fight, for preparation checks.
    pub regen_per_second_casting: f64,
    pub regen_per_second_not_casting: f64,
    /// The lowest maximum mana while Go deactivates every aura at the end of a fight.
    /// Each Mana change clamps current mana, and time to OOM reads it afterwards.
    pub teardown_max: f64,
}

/// A Seal of Command rank: the castable seal, its aura and its judgement.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SealOfCommandRank {
    pub seal_spell_id: i32,
    pub aura: String,
    pub judgement_spell_id: i32,
}

/// A Seal of Righteousness rank: the castable seal, its aura, its judgement, the damage spell
/// it fires on a hit and the per-hit value per hundred of swing speed.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SealOfRighteousnessRank {
    pub seal_spell_id: i32,
    pub aura: String,
    pub judgement_spell_id: i32,
    pub proc_spell_id: i32,
    pub per_hit_value: f64,
    /// The damage spell's spell power share with the weapon in the hand, given when an item swap
    /// moves it between a one-hander and a two-hander: each hit reads the hand as it lands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_coefficient: Option<f64>,
}

/// What an item swap callback leaves its aura in under one set: activated if inactive (`on`),
/// activated with an active aura refreshed (`activate`), deactivated (`off`) or untouched
/// (`keep`).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SwapAuraState {
    On,
    Activate,
    Off,
    Keep,
}

/// A dynamic spell modifier an item swap callback switches: Go `SpellMod` with its kind, value and
/// the spells it reaches, and whether it is active after the reset.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SwapSpellMod {
    pub kind: String,
    pub value: f64,
    pub spells: Vec<usize>,
    pub active: bool,
}

/// An item swap callback of a swapped slot (sim/core/item_swaps.go `RegisterItemSwapCallback`),
/// as the state its target takes under the request's set (`main`) and the swap set (`swapped`).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SwapCallback {
    /// An aura that follows the weapon: a racial weapon specialization, an item or enchant proc
    /// trigger (`registerProcInternal`) or an enchant's buff (`RegisterEnchantBuffWithSlots`).
    Aura {
        slot: String,
        aura: String,
        main: SwapAuraState,
        swapped: SwapAuraState,
    },
    /// An aura that follows a weapon imbue the set's main hand carries (`main`, `swapped`), as
    /// Flametongue's trigger: off when the weapon now in the hand lacks it, activated when it
    /// carries it and the weapon it replaced did not, untouched otherwise.
    AuraOnChange {
        slot: String,
        aura: String,
        main: bool,
        swapped: bool,
    },
    /// An item's use (`RegisterActive`): unusable while the item is not worn, and on cooldown for
    /// at least 30 seconds once it is put on during the fight; its aura falls off at any swap.
    ItemUse {
        slot: String,
        spell: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        aura: Option<String>,
        main: bool,
        swapped: bool,
    },
    /// A spell modifier active only with a weapon of a hand type, as a weapon specialization's.
    SpellMod {
        slot: String,
        #[serde(rename = "mod")]
        modifier: SwapSpellMod,
        main: bool,
        swapped: bool,
    },
}

/// One value of the prepared state that differs once the swap set is worn: a JSON pointer into the
/// prepared document and the value there.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SwapChange {
    pub path: String,
    pub value: serde_json::Value,
}

/// What an aura an item swap switches does as it is gained and fades: Weaponmaster's mace
/// aura adds `armor_ignore_factor` to every attack table's armor ignore factor.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SwapAuraEffect {
    pub aura: String,
    pub armor_ignore_factor: f64,
}

/// The lines a stat aura an item swap switches logs beyond its own as it is gained and fades.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SwapAuraLogs {
    pub aura: String,
    pub gain_logs: Vec<String>,
    pub expire_logs: Vec<String>,
}

/// A Seal of Fury rank: its seal, judgement and damage spell, the damage spell's flat Holy
/// damage, and the absorb shield aura with the share of the damage it absorbs.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SealOfFuryRank {
    pub seal_spell_id: i32,
    pub aura: String,
    pub judgement_spell_id: i32,
    pub proc_spell_id: i32,
    pub proc_damage: f64,
    pub shield_aura: String,
    pub shield_share: f64,
}

/// Improved Seal of Fury: the mana a spent shield returns, raised per level the target is
/// above the paladin up to a cap, and its metrics.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImprovedSealOfFury {
    pub mana: f64,
    pub per_level: f64,
    pub max_levels: f64,
    pub levels: f64,
    pub metrics_action_id: ActionId,
    /// Its mana makes no threat (client 70374).
    pub no_threat: bool,
}

/// The Bear's Swipe cleave: the targets it hits and each hit's roll.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PetCleave {
    pub max_targets: i32,
    pub average: f64,
    pub variance: f64,
}

/// A Seal of Light rank: its seal, the self heal its proc casts and the heal's amount, and its
/// judgement with the judgement's target aura.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SealOfLightRank {
    pub seal_spell_id: i32,
    pub aura: String,
    pub proc_spell_id: i32,
    pub heal: f64,
    pub judgement_spell_id: i32,
    pub judgement_aura: String,
}

/// A Seal of Wisdom rank: its seal, the mana restore its proc casts and the restore's amount,
/// and its judgement with the judgement's target aura.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SealOfWisdomRank {
    pub seal_spell_id: i32,
    pub aura: String,
    pub proc_spell_id: i32,
    pub mana: f64,
    pub judgement_spell_id: i32,
    pub judgement_aura: String,
}

/// A Seal of the Crusader rank: its seal, judgement, the judgement's target aura, the melee
/// speed multiplier and the main hand auto damage it takes back.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SealOfTheCrusaderRank {
    pub seal_spell_id: i32,
    pub aura: String,
    pub judgement_spell_id: i32,
    pub judgement_aura: String,
    pub melee_speed: f64,
    pub auto_damage_percent: f64,
}

/// A Lay on Hands rank: its spell and the mana it restores to a healed unit with a mana bar.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LayOnHandsRank {
    pub spell_id: i32,
    pub mana: f64,
}

/// A Paladin heal rank: its spell, which heal it is (`holy_light`, `flash_of_light` or
/// `holy_shock_heal`) and the bounds of its roll, equal when it draws nothing.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PaladinHealRank {
    pub spell_id: i32,
    pub heal: String,
    pub min: f64,
    pub max: f64,
    /// A rolled rank's client average and variance, which Go `Effect.Roll` reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub average: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variance: Option<f64>,
}

/// The paladin's healing modifiers on one unit at reset, its bonus healing taken and whether
/// it carries Blessing of Light for the whole fight.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PaladinHealUnit {
    pub healing_dealt_multiplier: f64,
    pub periodic_healing_dealt_multiplier: f64,
    pub healing_taken_multiplier: f64,
    pub table_healing_dealt_multiplier: f64,
    pub healing_power: f64,
    pub bonus_healing_taken: f64,
    pub blessing_of_light: bool,
}

/// A Holy Strike rank's percent of the normalized weapon swing plus the flat roll.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HolyStrikeRank {
    pub spell_id: i32,
    pub weapon_percent: f64,
}

/// A Consecration rank: the tick every target takes and the bonus the first targets take.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConsecrationRank {
    pub spell_id: i32,
    pub tick: f64,
    pub bonus: f64,
    pub bonus_coefficient: f64,
    pub bonus_targets: i32,
}

/// Consecrated Ground: the target aura Consecration's ticks mark and its Holy damage
/// multiplier while it holds.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConsecratedGround {
    pub aura: String,
    pub multiplier: f64,
}

/// A Holy Shield rank: the castable spell, the damage spell each block casts, the block
/// chance aura, its charges and the damage.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HolyShieldRank {
    pub spell_id: i32,
    pub proc_spell: usize,
    pub aura: String,
    pub charges: i32,
    pub damage: f64,
}

/// A Light's Vigil rank: the cast, its strike, the vigil aura on the target, the share of the
/// cast's cost the strike refunds and the refund's metrics.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LightsVigilRank {
    pub spell_id: i32,
    pub strike_spell_id: i32,
    pub aura: String,
    pub refund: f64,
    pub metrics_action_id: ActionId,
}

/// A Light's Vigil rank in the Holy Paladin healer sim under Forever's rules (push 113554): the
/// cast, its vigil aura on the player, and the party heal the next Holy Shock heal on the player
/// triggers: the heal spell, its client roll at the caster level and the players it lands on.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LightsVigilAllyRank {
    pub spell_id: i32,
    pub aura: String,
    pub heal_spell_id: i32,
    pub average: f64,
    pub variance: f64,
    pub players: u32,
}

/// A Holy Shock rank's damage roll.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HolyShockRank {
    pub spell_id: i32,
    pub min: f64,
    pub max: f64,
}

/// One Twist of Light Echo: its aura and the seal whose effect it replays.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SealEcho {
    pub aura: String,
    pub seal: String,
}

/// Go `energyBar` for a player that has one, and its combo points.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Energy {
    pub max_energy: f64,
    pub max_combo_points: i32,
    pub tick_duration_ns: i64,
    pub energy_per_tick: f64,
}

/// A Fireball rank's dot: the base amount its ticks snapshot and whether they can crit.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FireballRank {
    pub spell_id: i32,
    pub tick_base: f64,
    pub tick_can_crit: bool,
}

/// A Flamestrike rank's area dot: the amount each tick deals before spell power.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FlamestrikeRank {
    pub spell_id: i32,
    pub tick_base: f64,
}

/// A damage on-use item's direct hit: the row's roll, the scale the area rule leaves for one
/// target, and the outcome applier by name. Against several targets the hit is an `area` or a
/// `chain`, as a damage proc's is, and the scale is not used.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OnUseDirect {
    pub average: f64,
    pub variance: f64,
    pub scale: f64,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<ProcArea>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<ProcChain>,
}

/// A damage on-use item's damage over time: each tick's amount, whether a tick can crit, and the
/// application's outcome applier when the row deals no direct hit.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OnUsePeriodic {
    pub tick_base: f64,
    pub tick_can_crit: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_outcome: Option<String>,
}

/// Improved Scorch's stacking Fire Vulnerability buff on the mage.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImprovedScorch {
    pub aura: String,
    pub proc_chance: f64,
    pub damage_per_stack: f64,
}

/// An action a unit's metrics list, from Go `Spell.doneIteration`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetricsAction {
    pub action_id: ActionId,
    pub melee_metrics: bool,
    pub school: u8,
    /// Go `SpellFlagPassiveSpell`, which metrics report.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub passive: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttackTable {
    pub base_spell_miss_chance: f64,
    pub spell_crit_suppression: f64,
    pub bonus_spell_crit_percent: f64,
    pub crit_multiplier: f64,
    pub damage_dealt_multiplier: f64,
    pub damage_taken_multiplier: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Cost {
    pub resource: String,
    pub base_cost: i32,
    pub flat_modifier: i32,
    pub percent_modifier: f64,
    pub additive_percent_modifier: f64,
    /// Go `EnergyCost.Refund` or `RageCost.Refund`: the share of the cost a missed strike
    /// gives back.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub refund: f64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Cast {
    pub cost: f64,
    pub gcd_ns: i64,
    pub gcd_min_ns: i64,
    pub cast_time_ns: i64,
    pub non_empty: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CastKind {
    /// Go `makeCastFunc`: cost, GCD, hardcast and haste handling.
    Full,
    /// Go `makeCastFuncSimple`: cooldown checks without cost, GCD or hardcast.
    Simple,
    /// Go `makeCastFuncAutosOrProcs`: effects only.
    AutosOrProcs,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Dot {
    /// "target" for a per-target dot, "self" for a self-only or AoE aura.
    pub unit: String,
    pub aura_label: String,
    pub base_tick_count: i32,
    pub base_tick_length_ns: i64,
    pub bonus_coefficient: f64,
    pub periodic_damage_multiplier: f64,
    pub base_duration_multiplier: f64,
    pub base_duration_flat_ns: i64,
    pub affected_by_cast_speed: bool,
    pub affected_by_real_haste: bool,
    pub haste_reduces_duration: bool,
    pub channeled: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DamageEffect {
    /// Client amount at the caster level, before variance.
    pub average: f64,
    /// Client variance: the roll is uniform over average x (1 +- variance / 2).
    pub variance: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Spell {
    pub action_id: Option<ActionId>,
    pub rank: i32,
    /// Go's client school bit mask.
    pub school: u8,
    pub defense_type: String,
    pub proc_mask: Vec<String>,
    pub flags: Vec<String>,
    /// Stable name for the Go class spell mask, absent for non-class spells.
    pub class_spell: Option<String>,
    pub missile_speed: f64,
    pub cost: Option<Cost>,
    pub default_cast: Cast,
    pub cast_kind: CastKind,
    pub ignore_haste: bool,
    pub has_extra_cast_condition: bool,
    pub has_cast_requirement: bool,
    /// The player auras whose form the cast requirement refuses: Go `castRequirementFailure`
    /// fails while any is active. Absent when the requirement is not represented.
    #[serde(default)]
    pub requirement_auras: Option<Vec<String>>,
    pub min_range: f64,
    pub max_range: f64,
    pub max_charges: i32,
    pub cd: Option<Cooldown>,
    pub shared_cd: Option<Cooldown>,
    pub bonus_hit_percent: f64,
    pub bonus_crit_percent: f64,
    pub bonus_spell_damage: f64,
    pub bonus_expertise_percent: f64,
    pub cast_time_multiplier: f64,
    pub cd_multiplier: f64,
    pub damage_multiplier: f64,
    pub damage_multiplier_additive: f64,
    pub direct_damage_multiplier_additive: f64,
    pub crit_multiplier_pct: f64,
    pub crit_multiplier_additive: f64,
    pub bonus_base_damage: f64,
    pub bonus_coefficient: f64,
    pub threat_multiplier: f64,
    pub flat_threat_bonus: f64,
    pub pushback_resist: f64,
    pub dot: Option<Dot>,
    pub damage_effect: Option<DamageEffect>,
    /// The spellbook position of the spell whose dot Go `Spell.Dot` resolves to when this
    /// spell has none of its own.
    #[serde(default)]
    pub related_dot_spell: Option<usize>,
    /// Go `MetricSplits`: the spell reports one tagged metric entry per split.
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub metric_splits: usize,
    /// Go `RankAliases`: older rank IDs a rotation may still name, which `GetAPLSpell` falls
    /// back to (Lava Burst rank 4, client 70425).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rank_aliases: Vec<ActionId>,
}

impl Spell {
    /// Go `slices.ContainsFunc(RankAliases, id.SameAction)`.
    pub fn has_rank_alias(&self, id: &ActionId) -> bool {
        self.rank_aliases.iter().any(|alias| {
            alias.spell_id == id.spell_id
                && alias.item_id == id.item_id
                && alias.other_id == id.other_id
        })
    }

    pub fn has_flag(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MajorCooldown {
    pub action_id: ActionId,
    pub priority: i32,
    #[serde(rename = "type")]
    pub kind: Vec<String>,
    pub timings_ns: Vec<i64>,
    /// The cooldown's spell by position, present only when an earlier spell shares its
    /// action ID, as Sweeping Strikes' hit precedes its cast. Go holds the spell itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spell: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Player {
    pub index: i32,
    pub label: String,
    pub level: i32,
    pub mob_type: Option<String>,
    pub stats: BTreeMap<String, f64>,
    pub pseudo_stats: PseudoStats,
    pub auras: Vec<Aura>,
    pub name: String,
    pub class: String,
    pub race: String,
    pub professions: Vec<String>,
    pub talents_string: String,
    pub talents: BTreeMap<String, i32>,
    pub class_options: serde_json::Value,
    pub reaction_ns: i64,
    pub channel_clip_delay_ns: i64,
    pub distance_yards: f64,
    /// Go `Unit.CastSpeed`, the factor applied to hasted durations.
    pub cast_speed: f64,
    pub mana: Mana,
    /// Go `energyBar`, absent for a player without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy: Option<Energy>,
    pub attack_table: AttackTable,
    pub spells: Vec<Spell>,
    /// Go's initial major cooldown order after the rotation claimed its spells.
    pub major_cooldowns: Vec<MajorCooldown>,
    /// The request's APL in protojson form. Interpreted by the rotation module.
    pub rotation: serde_json::Value,
    /// The health a fight starts with, where it differs from the maximum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_at_reset: Option<f64>,
    /// Mana carried into a prepared route stage; absent means a full bar.
    /// Values above the new maximum are clamped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mana_at_reset: Option<f64>,
    /// Go `HpPercentForDefensives`: survival major cooldowns wait for the health share to
    /// fall to it, and never fire on their own at zero.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub hp_percent_for_defensives: f64,
    /// Forever's rule: an aura that lets more spirit regeneration continue while casting
    /// (Resourcefulness, Rapid Recuperation, Improved Stormstrike) refreshes the mana tick as it
    /// changes, and its mana lands in metrics of its own. False is the Go reference, whose cached
    /// tick ignores the change until something else refreshes it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub casting_regen_at_once: bool,
    /// Forever's rule: a rotation's move waits for a spell the player has queued to fire, so a
    /// shot or strike queued at the end of the global cooldown is cast where it was pressed. False
    /// is the Go reference, whose move starts at once and can carry the player out of the queued
    /// spell's range before it fires.
    #[serde(default, skip_serializing_if = "is_false")]
    pub move_waits_for_queued_cast: bool,
    /// Forever's rule: a melee swing leaves the Auto Shot timer where it was, so a shot due at a
    /// time still fires then after a swing lands before it. False is the Go reference, under
    /// which every melee swing restarts the timer as if a shot had just fired.
    #[serde(default, skip_serializing_if = "is_false")]
    pub melee_keeps_auto_shot_timer: bool,
    /// Forever's rule: a move whose end time has come reads as arrived, so a rotation woken at
    /// that instant, before the move's end action runs, sees the destination. False is the Go
    /// reference, which reads the millisecond-rounded walk a hair short of it and can leave a
    /// Hunter who stepped in out of melee range for the rest of the fight.
    #[serde(default, skip_serializing_if = "is_false")]
    pub move_arrives_at_destination: bool,
    /// Forever's rule: a rotation with a Move anywhere in it keeps looking through the global
    /// cooldown, so a walk starts when its condition turns true, as moving needs no global
    /// cooldown, and goes on from a queued instant as it goes off. False is the Go reference,
    /// whose idle rotation sleeps until the global cooldown ends, and a walk timed to a swing
    /// timer or waiting on a queued instant starts late.
    #[serde(default, skip_serializing_if = "is_false")]
    pub rotation_with_moves_looks_through_gcd: bool,
    /// Forever's rule: a dual wielder who also shoots has a melee attack that an Auto Shot turns
    /// off and a walk into melee range turns back on as it starts, and the off hand's half swing
    /// wait counts from then. An attack still on when the player steps back in adds no new wait.
    /// False is the Go reference, which adds the half swing on every arrival in melee range.
    #[serde(default, skip_serializing_if = "is_false")]
    pub off_hand_waits_from_attack_start: bool,
    /// Forever's rule: Autocast Other Cooldowns passes over a ready cooldown that may not fire
    /// at this point of the GCD (an explosive while the GCD is ready or too little of it is
    /// left for its cast, any other while the GCD runs) and looks at the next one. False is the
    /// Go reference, which stops at the first ready cooldown and fires nothing when it fails.
    #[serde(default, skip_serializing_if = "is_false")]
    pub autocast_skips_unready_cooldowns: bool,
    /// Forever's rule: a damage bonus applied after the damage is calculated (Improved Mind
    /// Flay's ramp, Ice Lance on a frozen target, Soul Siphon, Amplify Curse, Incinerate on an
    /// Immolated target) raises the hit's threat with its damage. False is the Go reference,
    /// which kept the threat of the damage before the bonus.
    #[serde(default, skip_serializing_if = "is_false")]
    pub threat_follows_damage_bonus: bool,
    /// MythicSim's healer sims under Forever's rules: each heal's share that real healers
    /// overheal, measured on foreverlogs.gg (`prepare::overheal`). Only the rest counts as
    /// healing; the share counts as overhealing. Empty is the Go reference, which counts every
    /// heal in full.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overheal: Vec<Overheal>,
    /// MythicSim's healer sims under Forever's rules: the length of the window burst healing
    /// per second is measured over, in nanoseconds (`prepare::burst_hps`). None reports no burst,
    /// as Go does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub burst_window: Option<i64>,
    /// Every prepull action Go registered: the rotation's, and any a class or item adds.
    pub prepull_actions: usize,
    /// The dot base durations the rotation's `dotBaseDuration` values captured when Go built
    /// the rotation, by spell.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rotation_dot_base_durations: Vec<RotationDotBaseDuration>,
    /// What the rotation's values read that the rest of the input leaves out, for a rotation
    /// that holds such a value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation_reads: Option<RotationReads>,
}

/// What `APLValueDotBaseDuration` captured for a spell: the dot's `BaseDuration` on the first
/// target when Go built the rotation, before the reset activated any aura's spell mods.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RotationDotBaseDuration {
    pub spell: ActionId,
    pub base_duration_ns: i64,
}

/// What the rotation's values read that the rest of the input leaves out.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RotationReads {
    /// Go apl_values_consumables.go: the potion and the conjured item the consumables select.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub potion_id: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conjured_id: Option<i32>,
    /// Go apl_values_boss.go: the spells the first target knows that a boss value names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_spells: Vec<RotationTargetSpell>,
    /// Go `APLValueMultipleCdUsages`: the related self buff of each spell it names that has one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub self_buffs: Vec<RotationSelfBuff>,
    /// Go `Character.ItemProcBuffs`, which the item stat proc values filter by stat and ICD.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub item_proc_buffs: Vec<RotationItemProcBuff>,
    /// The major cooldowns whose buff aura moves one of the stats a stat buff cooldown value
    /// names, as the value finds them while the rotation is built, by the stats.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cooldown_buffs: Vec<RotationCooldownBuffs>,
    /// The spells each `castAllStatBuffCooldowns` casts, by its stats.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cast_cooldowns: Vec<RotationCooldownBuffs>,
}

/// A `StatBuffAura` of `Character.ItemProcBuffs`: the aura's label, the stats it buffs as Go's
/// stat indices, and whether it is swapped out.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RotationItemProcBuff {
    pub aura: String,
    pub stats: Vec<i32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub swapped: bool,
}

/// The major cooldowns matching stats: their spells and their buff auras' labels.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RotationCooldownBuffs {
    pub stats: Vec<i32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spells: Vec<ActionId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auras: Vec<String>,
}

/// A spell the first target knows, which its identical copies share.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RotationTargetSpell {
    pub spell: ActionId,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_cooldown: bool,
}

/// A spell's `RelatedSelfBuff`, by the aura's label, and whether the spell has a cooldown.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RotationSelfBuff {
    pub spell: ActionId,
    pub aura: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_cooldown: bool,
}

/// A pet Go enables at each reset, as core/pet.go builds it.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Pet {
    pub index: i32,
    pub label: String,
    pub level: i32,
    pub mob_type: Option<String>,
    pub stats: BTreeMap<String, f64>,
    pub pseudo_stats: PseudoStats,
    pub auras: Vec<Aura>,
    pub name: String,
    pub reaction_ns: i64,
    pub distance_yards: f64,
    pub cast_speed: f64,
    pub mana: PetMana,
    pub attack_table: AttackTable,
    pub melee: Melee,
    pub spells: Vec<Spell>,
    pub metrics_actions: Vec<MetricsAction>,
    /// The lines Go's Enable logs after its stat change: the pet's stats and inheritance.
    pub summon_log: Vec<String>,
    /// The stats line Go's Disable logs once the inheritance is gone.
    pub dismiss_log: String,
    /// The stats line Go's Disable logs while the pet's permanent auras are still up, at the
    /// reset or during a fight, where it differs from the fight's end: a party aura on the pet
    /// adds its stats (Go patch 131). Only for a pet without `dismiss_stats`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_dismiss_log: Option<String>,
    /// Go `isDynamic`: the pet follows its owner's stat changes.
    pub dynamic_stats: bool,
    /// A focus bar, for a pet without a mana bar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<PetFocus>,
    /// A guardian an effect summons during a fight with a timeout.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub summoned: bool,
    /// Go `GetMovementSpeed` of a pet that starts out of melee range, zero otherwise.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub movement_speed: f64,
    /// A dynamic pet's linear inheritance of its owner's stat changes, one owner stat for
    /// each pet stat.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inheritance: Vec<StatInheritance>,
    /// What Go recomputes a dynamic pet's stats from: its stats before dependencies, and its
    /// enabled stat dependencies in their sorted order.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stats_without_deps: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stat_dependencies: Vec<StatDependency>,
    /// A dynamic pet enabled at reset: Go `inheritedStats`, which Disable takes away, and the
    /// stats of its dismissal line in Go's stat order, with the stats its inheritance can
    /// change listed even at zero.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inherited_stats: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dismiss_stats: Vec<DismissStat>,
    /// What each permanent aura's expiry adds to the stats before dependencies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aura_stats: Vec<PetAuraStats>,
    /// What the pet's `OnPetDisable` adds to the stats before dependencies, after Disable takes
    /// the inheritance away and before its auras expire, for a pet whose enable and disable
    /// callbacks only move stats.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub disable_stats: BTreeMap<String, f64>,
}

/// The change a pet aura's expiry makes to the pet's stats before dependencies, Go
/// aura_helpers.go `AttachStatsBuff`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PetAuraStats {
    pub aura: String,
    pub stats: BTreeMap<String, f64>,
}

/// One stat of a pet's dismissal line, as Go stats.go `FlatString` names it.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DismissStat {
    pub stat: String,
    pub value: f64,
}

/// Go focus.go `focusBar` of a pet.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PetFocus {
    pub max: f64,
    pub regen_per_tick: f64,
    pub tick_duration_ns: i64,
}

/// Go `stats.StatDependency`: `dst` gains `amount` times `src`, in whole `step`s where
/// nonzero, or is multiplied by `amount` when `src` is `dst`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StatDependency {
    pub src: String,
    pub dst: String,
    pub amount: f64,
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub step: f64,
}

/// One term of a pet's stat inheritance: the pet stat gains `coefficient` times the change
/// of the owner stat.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StatInheritance {
    pub owner: String,
    pub pet: String,
    pub coefficient: f64,
}

/// A pet's mana bar and its regeneration, which Go computes from the pet's own stats.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PetMana {
    pub max: f64,
    pub regen_per_second_casting: f64,
    pub regen_per_second_not_casting: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManaGain {
    pub min: f64,
    pub spread: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MissileRank {
    pub channel_spell_id: i32,
    pub tick_spell_id: i32,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManaGem {
    pub item_id: i32,
    pub mana: f64,
}

/// A Go `Weapon`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Weapon {
    pub base_damage_min: f64,
    pub base_damage_max: f64,
    pub attack_power_per_dps: f64,
    pub swing_speed: f64,
    pub normalized_swing_speed: f64,
    pub school: u8,
    pub min_range: f64,
    pub max_range: f64,
}

/// The player's weapon attacks and the physical attack table against the target, with the
/// defender's static chances resolved.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Melee {
    pub auto_swing_melee: bool,
    pub auto_swing_ranged: bool,
    pub dual_wielding: bool,
    pub main_hand: Weapon,
    pub off_hand: Weapon,
    pub ranged: Weapon,
    pub base_miss_chance: f64,
    pub base_glance_chance: f64,
    pub glance_multiplier: f64,
    pub glance_spread: f64,
    pub hit_suppression: f64,
    pub melee_crit_suppression: f64,
    pub ignore_armor: bool,
    pub armor_ignore_factor: f64,
    pub in_front_of_target: bool,
    pub attack_speed_multiplier: f64,
    pub melee_speed_multiplier: f64,
    pub dodge_reduction: f64,
    pub disable_dw_miss_penalty: bool,
    pub defender_dodge: f64,
    pub defender_parry: f64,
    pub defender_block: f64,
    pub defender_armor: f64,
    pub defender_block_reduction: f64,
    pub defender_bonus_attack_power: f64,
    pub defender_bonus_physical_damage_taken: f64,
    pub defender_reduced_physical_hit_taken: f64,
    /// A class replace function on the main hand, which the exporter admits only when it
    /// returns the swing unchanged: Go still reacts to the event before each main hand swing.
    #[serde(default)]
    pub replace_main_hand_swing: bool,
    /// The ranged auto attack's static inputs, present only with ranged auto attacks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranged_state: Option<RangedState>,
    /// Go weapon_skill.go: the table a main hand attack reads where the weapon's skill above
    /// base moves it, against an enemy at or above the attacker's level. Absent otherwise, and
    /// the attack reads the table above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_hand_skill: Option<WeaponSkillTable>,
    /// [`Self::main_hand_skill`] for off hand attacks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off_hand_skill: Option<WeaponSkillTable>,
    /// [`Self::main_hand_skill`] for ranged attacks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranged_skill: Option<WeaponSkillTable>,
}

/// Go weapon_skill.go: what an attack with one hand reads in place of [`Melee`]'s fields of the
/// same names, resolved from the gap between the target's defense and the hand's weapon skill.
/// Glancing chance, parry and crit suppression stay on base skill.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WeaponSkillTable {
    /// The hand's skill above base, from its weapon type's line.
    pub skill: f64,
    /// Go `AttackTable.MissChance`, in place of `base_miss_chance`.
    pub miss_chance: f64,
    /// Go `AttackTable.HitSuppressionFor`.
    pub hit_suppression: f64,
    /// The defender's dodge with Go `AttackTable.DodgeChance` in place of the table's base.
    pub defender_dodge: f64,
    /// Go `AttackTable.Glance`.
    pub glance_multiplier: f64,
    pub glance_spread: f64,
}

/// Go attack.go's ranged auto attack inputs: the ranged speed pseudo stat and the defender's
/// ranged attack power bonus.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RangedState {
    pub ranged_speed_multiplier: f64,
    pub defender_bonus_ranged_attack_power: f64,
}

/// The target's main hand swings at the player when the player tanks it: Go attack.go's enemy
/// `ApplyEffects`, `CalcDamage` and `outcomeEnemyMeleeWhite`, with every value resolved as Go
/// computes it at reset.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Enemy {
    pub action_id: ActionId,
    pub school: u8,
    pub swing_speed: f64,
    pub melee_haste_multiplier: f64,
    pub base_damage_min: f64,
    pub damage_spread: f64,
    pub attack_power: f64,
    pub attack_power_coefficient: f64,
    pub bonus_damage: f64,
    pub attacker_multiplier: f64,
    /// The steps that read the player's defenses, by stat aura combination as the stat_auras
    /// effect numbers them; one entry without stat auras.
    pub rolls: Vec<EnemyRolls>,
    pub threat_multiplier: f64,
    pub flat_threat_bonus: f64,
    pub unit_threat_multiplier: f64,
    pub log_attack_power: f64,
    pub log_ranged_attack_power: f64,
    pub log_spell_power: f64,
    /// Auras inactive at reset whose activation changes a value above, as "player:label" or
    /// "target:label".
    pub changing_auras: Vec<String>,
    /// The rolls while a hardcast holds the tank's reduced avoidance aura, by stat aura
    /// combination; empty when the player does not tank.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reduced_avoidance_rolls: Vec<EnemyRolls>,
    /// Go `TotalMeleeHasteMultiplier`'s factors on the target, which a slow multiplies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attack_speed_multiplier: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub melee_speed_multiplier: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub melee_haste_rating_multiplier: Option<f64>,
    /// Player auras inactive at reset whose activation changes only the player's damage
    /// taken multiplier, which the runtime reads live, as "player:label".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub damage_taken_auras: Vec<String>,
    /// Target auras inactive at reset whose activation changes only the target's melee speed
    /// multiplier, as "target:label".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speed_auras: Vec<String>,
    /// Player auras whose activation changes only the swing's school damage taken multiplier
    /// and so its target multiplier.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub school_damage_taken_auras: Vec<String>,
    /// Target auras that change only the target's attack power, with the swing's attack power
    /// and the debug line's MAP while each is active alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attack_power_auras: Vec<EnemyAttackPowerAura>,
}

/// A target aura that changes only the target's attack power.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnemyAttackPowerAura {
    pub aura: String,
    pub attack_power: f64,
    pub log_attack_power: f64,
}

/// The steps of the target's swing that read the player's defenses.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnemyRolls {
    pub armor_multiplier: f64,
    pub bonus_damage_taken: f64,
    pub target_multiplier: f64,
    /// What each step of the table adds to the running chance, zero for a skipped step.
    pub miss_chance: f64,
    pub dodge_chance: f64,
    pub parry_chance: f64,
    pub block_chance: f64,
    pub crit_chance: f64,
    pub crush_chance: f64,
    pub block_reduction: f64,
    /// The block reduction's factors, the block value and its multiplier, whose product Go
    /// fuses into the blocked damage's subtraction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_value_multiplier: Option<f64>,
    /// The target multiplier's factors besides the player's damage taken multiplier: the
    /// physical school's and the attack table's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub school_damage_taken_multiplier: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_damage_taken_multiplier: Option<f64>,
}

/// Go `calcHealingInternal`'s multipliers for a heal at reset: the caster's healing dealt
/// and periodic healing dealt, the target's healing taken, the attack table's healing dealt,
/// and the healing power the debug line shows.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HealModifiers {
    pub healing_dealt_multiplier: f64,
    pub periodic_healing_dealt_multiplier: f64,
    pub healing_taken_multiplier: f64,
    pub table_healing_dealt_multiplier: f64,
    pub healing_power: f64,
}

/// One heal's overhealing share in MythicSim's healer sims: what real healers overheal of it.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Overheal {
    pub spell_id: i32,
    /// The share of each heal that is overhealing, from zero to below one.
    pub fraction: f64,
}

/// One heal rank a healer casts on the player, in MythicSim's healer sims (Rust only, no Go
/// counterpart): every heal lands on a player who needs it and counts in full. A rank can heal
/// at once, over time, both (Regrowth) or shield; the spell carries its cost, cast, crit bonus,
/// multipliers and the direct or shield's healing power coefficient, and its dot the ticks, tick
/// period and each tick's coefficient.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HealSpell {
    pub spell_id: i32,
    /// The direct heal's client roll at the caster level, none for a heal over time or a shield.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct: Option<DamageEffect>,
    /// How many players the direct heal lands on, each with its own roll and crit: one, a party
    /// heal's party, or a chain heal's jumps.
    pub targets: u32,
    /// Each further player's share of the heal before it: a chain heal's falloff, one for a
    /// party heal.
    pub jump_multiplier: f64,
    /// A heal over time's client amount per tick at the caster level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick: Option<f64>,
    /// Forever's rule: each tick of the heal over time rolls the healing crit, as its row
    /// states Periodic Can Crit. False is the Go reference, whose heal ticks never crit.
    #[serde(default, skip_serializing_if = "is_false")]
    pub tick_can_crit: bool,
    /// A shield's client absorb at the caster level, counted as shielding when it goes up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absorb: Option<f64>,
    /// Under Forever's rules, a heal whose reach depends on who the party's damage hits
    /// ([`PartyDamage`]); none heals `targets` players at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smart: Option<SmartHeal>,
    /// A multiplier on every player past the first, on top of the falloff: The Ten Storms'
    /// bonus to Chain Heal's jumps. One is none.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub past_first_multiplier: f64,
    /// A healer's gear under Forever's rules: the heal over time rank, by client spell ID, that
    /// goes up with this heal at no cost (Vestments of Transcendence's Renew on Greater Heal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub then_hot: Option<i32>,
    /// A healer's gear under Forever's rules: the absorb a crit of the direct heal puts on its
    /// player, counted as this heal's shielding (Vestments of Faith's Armor of Faith).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crit_shield: Option<f64>,
    /// Forever's rules: a share of each critical heal that becomes an absorb on its player
    /// (Divine Aegis), counted as another spell's shielding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crit_absorb: Option<CritAbsorb>,
    /// A healer's gear under Forever's rules: the direct heal's multiplier while any of these
    /// heal over time ranks is up (Idol of Rejuvenation on Regrowth).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hot_bonus: Option<HotBonus>,
    /// A healer's gear under Forever's rules: what each of the healer's heals over time up
    /// takes off this spell's cooldown when it is cast, in nanoseconds (Idol of Synthesis on
    /// Swiftmend).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooldown_per_hot_ns: Option<i64>,
}

/// A heal's critical hits shielding their player, as Divine Aegis does.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CritAbsorb {
    /// The share of the critical heal the absorb holds.
    pub share: f64,
    /// The client spell ID of the absorb aura, whose spell the shielding counts on.
    pub spell_id: i32,
    /// Whether the ticks of a channelled heal (Penance) shield as well. A tick of a heal over
    /// time never does.
    #[serde(default, skip_serializing_if = "is_false")]
    pub ticks: bool,
}

/// A direct heal's bonus while a heal over time of the healer is up.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HotBonus {
    /// The heal over time ranks, by client spell ID.
    pub spell_ids: Vec<i32>,
    pub multiplier: f64,
    /// Whether the bonus is the first player's alone, as Riptide's is on Chain Heal ("directly
    /// on that target"), where the others jump on at their falloff of the unboosted heal.
    #[serde(default, skip_serializing_if = "is_false")]
    pub first_target_only: bool,
}

/// Renewed Hope (425280): the priest's Power Word: Shield leaves Weakened Soul on a member of
/// the party, and a listed heal on a member who has it gains crit chance and cuts the debuff.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RenewedHope {
    /// The heal ranks that gain the crit and cut the debuff, by client spell ID.
    pub heal_spell_ids: Vec<i32>,
    /// The shield ranks that leave Weakened Soul, by client spell ID.
    pub shield_spell_ids: Vec<i32>,
    /// The crit chance a heal gains, in percentage points.
    pub crit_percent: f64,
    /// What a heal takes off the debuff's remaining time, in nanoseconds.
    pub cut_ns: i64,
    /// Weakened Soul's duration, in nanoseconds.
    pub duration_ns: i64,
    /// Hits a second on each member of the party, the tank first: who needs the next heal.
    pub hit_rates: Vec<f64>,
}

/// Litany of Light (1317006): casting a heal after a different heal restores a share of its
/// base mana cost.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LitanyOfLight {
    /// The share of the spell's base cost that returns.
    pub share: f64,
    /// The heal ranks, grouped by the spell they are a rank of: the next heal differs when its
    /// group does.
    pub families: Vec<Vec<i32>>,
    /// The metrics the mana returned counts on.
    pub metrics_action_id: ActionId,
}

/// A heal that reaches players as the party's damage hits them, in MythicSim's healer sims under
/// Forever's rules.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SmartHeal {
    /// Prayer of Mending: the buff waits on a player until that player is hit, heals, and moves
    /// to another member of the party with a charge less and its duration again, until the
    /// charges or the duration run out. A new cast replaces the one running.
    Mending {
        charges: u32,
        /// How long the buff waits on each player, in nanoseconds.
        duration: i64,
    },
    /// Chain Heal: the first player, then a jump to each other member hit lately, up to
    /// `targets` players.
    Chain,
}

/// The damage a healer's party takes, in MythicSim's healer sims under Forever's rules: how
/// often each of the five members is hit, for the heals that follow it ([`SmartHeal`]). An
/// assumption, not a measurement: Forever's level 30 logs hold neither Prayer of Mending nor
/// Chain Heal (`prepare::party_damage`).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PartyDamage {
    /// Hits a second on each member, the tank first.
    pub hit_rates: Vec<f64>,
    /// How long after a hit a member still needs healing, in nanoseconds.
    pub injured_window: i64,
}

/// A client damage row's roll: spelldata `Effect.Roll` draws between the bounds unless the
/// row has no variance, when it gives the average.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DamageRoll {
    pub average: f64,
    pub min: f64,
    pub max: f64,
    pub rolls: bool,
    /// The row's variance, which `Effect.Roll` reads with the average.
    #[serde(default)]
    pub variance: f64,
}

/// One trigger of rage on an avoided hit taken: its aura, energize spell, rage, the outcomes
/// it hears, whether it needs a shield, and its chance.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AvoidTrigger {
    pub aura: String,
    pub spell_id: i32,
    pub rage: f64,
    pub outcomes: Vec<String>,
    pub needs_block: bool,
    pub chance: f64,
}

/// A flat change to a spell's cost, by spellbook position.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CostChange {
    pub spell: usize,
    pub flat: i32,
}

/// A spell mod's change to one spell, by spellbook position: `kind` names the runtime's mod
/// (`cast_time_percent`, `cast_time_flat`, `power_cost_percent_add`, `power_cost_flat`,
/// `bonus_crit_percent`, `damage_done_flat` or `global_cooldown_flat`); a time is in
/// nanoseconds.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SpellModChange {
    pub spell: usize,
    pub kind: String,
    pub value: f64,
}

/// A spell a mana listener hears, by spellbook position, with the mana it restores.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SpellMana {
    pub spell: usize,
    pub mana: f64,
}

/// shared_utils.go `calcMultiTargetDamage` for a chain: the targets it reaches and the share of
/// the damage each jump keeps, the row's `ChainAmp` as the float32 the client states.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcChain {
    pub targets: i32,
    pub amp: f64,
}

/// shared_utils.go `calcMultiTargetDamage` for an area: the row's cap on the targets it hits (zero
/// for all of them), whether the roll is split evenly among them and the encounter's AoE cap
/// multiplier an uncapped area takes.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcArea {
    pub max_targets: i32,
    pub splits: bool,
    pub aoe_cap_multiplier: f64,
}

/// shared_utils.go `spellDataProcDamageSpell` for a row with a periodic damage effect: the tick's
/// amount and the outcome applier each tick rolls (spelldata `Spell.TickOutcome`) by name:
/// `tick_magic_hit_and_crit`, `tick_physical_crit` or `tick`. Where the row also deals a direct
/// hit the dot lands with it; alone it goes on unrolled, or on the hit roll `application` names.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcPeriodic {
    pub tick_base: f64,
    pub tick_outcome: String,
    pub with_direct: bool,
    /// The outcome applier a dot without a direct hit rolls when it is applied, by name:
    /// `magic_hit`, as Ebon Hilt of Marduk's Corruption does. Absent, the dot goes on unrolled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
}

/// One castable paladin aura rank: the cast's spell and the aura it turns on.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PaladinAuraRank {
    pub spell_id: i32,
    pub aura: String,
}

/// A spell a dynamic proc manager hears, by spellbook position, with the chance it rolls.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SpellChance {
    pub spell: usize,
    pub chance: f64,
}

/// One hand's Flametongue Weapon: its trigger, its hit spell by spellbook position, and the
/// spells whose landed hits cast it.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FlametongueHand {
    pub trigger_aura: String,
    pub spell: usize,
    /// Whether the hand's weapon has a speed; an empty hand's hit deals nothing.
    pub deals_damage: bool,
    pub base_damage: f64,
    pub trigger_spells: Vec<usize>,
}

/// A spell druid.RegisterSpell registered, by spellbook position, with the forms it may be
/// cast in.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DruidFormSpell {
    pub spell: usize,
    pub forms: Vec<String>,
}

/// A member of an exclusive category: its aura, Go `ExclusiveEffect.Priority` and its spell.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExclusiveMember {
    pub aura: String,
    pub priority: f64,
    pub spell_id: i32,
    /// A stacking member's bid for each stack, which every stack change sets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_stack: Option<f64>,
}

/// An aura, the pseudo stat it multiplies, `damage_taken` or `threat`, and the multiplier.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PseudoStatAura {
    pub aura: String,
    pub stat: String,
    pub multiplier: f64,
}

/// A stat by Go's name and its value.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NamedStat {
    pub stat: String,
    pub value: f64,
}

/// One Holy Nova rank: the damage spell, its triggered heal and the heal's base.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HolyNovaRank {
    pub spell_id: i32,
    pub heal_spell_id: i32,
    pub heal_base: f64,
}

/// One cat builder: which builder, its spell position and the rank's flat damage.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CatBuilder {
    pub kind: String,
    pub spell: usize,
    pub flat_damage: f64,
}

/// An aura and the multiplier it attaches.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuraMultiplier {
    pub aura: String,
    pub multiplier: f64,
}

/// A potion's instant resource gain, as Go `resourceGainConfig`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceGain {
    pub resource: String,
    pub min: f64,
    pub spread: f64,
}

/// A warrior stance's cast and aura.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WarriorStance {
    pub spell_id: i32,
    pub stance: String,
    pub aura: String,
}

/// Heroic Strike or Cleave: the strike, its queue aura and its base damage.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedStrike {
    pub spell_id: i32,
    pub queue_aura: String,
    pub base_damage: f64,
    pub cleave: bool,
}

/// Go `GeneratedExternalCD.ShouldActivate`: when the major cooldown manager casts an external
/// cooldown, in addition to its spell being castable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExternalActivation {
    /// No condition, Go's default `ShouldActivate`.
    #[default]
    Always,
    /// Go `driveInnervates`: the player's current mana is at most the threshold, read once the
    /// environment finalized.
    ManaAtMost { threshold: f64 },
    /// Go `driveManaTideTotems`: the simulation has reached the time, read from the base
    /// duration once the environment finalized.
    NotBefore { time_ns: i64 },
}

impl ExternalActivation {
    fn is_always(&self) -> bool {
        matches!(self, ExternalActivation::Always)
    }
}

/// The bar other than mana an item use's gain fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnergizedPower {
    Rage,
    Energy,
}

/// Behavior Rust must execute, with the parameters Go keeps in closures. Each variant
/// names the Go source that defines it in docs/prepared-v2.md.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Effect {
    Frostbolt {},
    /// The Gnome racial Eureka!: a major cooldown whose 8 second buff lowers the cost and raises
    /// the damage of the spells the class names, resolved into spell positions.
    Eureka {
        spell_id: i32,
        aura: String,
        cost_percent: f64,
        damage_percent: f64,
        tick_cancel_percent: f64,
        cost_spells: Vec<usize>,
        damage_spells: Vec<usize>,
        tick_cancel_spells: Vec<usize>,
    },
    /// The Undead racial Touch of the Grave: landed hits can drain the target.
    /// items_70425.go Blade of Hanna: a landed main hand swing among `trigger_spells` stacks
    /// `stacks_aura`; at its most the stacks go, `strike_spell` deals `damage` to the target on the
    /// melee special table, and `buff_aura`, a stat aura, comes up.
    BladeOfHanna {
        trigger_aura: String,
        stacks_aura: String,
        buff_aura: String,
        damage: f64,
        strike_spell: usize,
        trigger_spells: Vec<usize>,
    },
    TouchOfTheGrave {
        trigger_aura: String,
        drain_spell_id: i32,
        proc_chance: f64,
        proc_mask: Vec<String>,
        health_fraction: f64,
        delay_ns: i64,
    },
    /// The Troll racial Berserking: a major cooldown whose aura multiplies attack speed and
    /// then cast speed.
    Berserking {
        spell_id: i32,
        aura: String,
        attack_speed_multiplier: f64,
        cast_speed_multiplier: f64,
    },
    /// Go `NewGeneratedExternalCD`: other players cast a buff on the player on cooldown,
    /// `sources` of them taking turns (Go `registerExternalConsecutiveCDApproximation`). The
    /// cast is a simple spell with no cost, metrics or log whose own timer is the one it exports;
    /// each source also has a timer that the cast sets to `cooldown_ns`. The cast needs the next
    /// source's timer ready and no active aura with `aura_tag`, then activates `aura`. The major
    /// cooldown manager casts it when `activation` allows.
    ExternalCooldown {
        spell_id: i32,
        spell_tag: i32,
        aura: String,
        aura_tag: String,
        sources: i32,
        cooldown_ns: i64,
        duration_ns: i64,
        #[serde(default, skip_serializing_if = "ExternalActivation::is_always")]
        activation: ExternalActivation,
    },
    /// Go buffs/drivers.go `AttachInnervateRegen` on an aura other than the druid's own: while
    /// the aura is up the player has full spirit regeneration at `spirit_regen_multiplier` times
    /// the rate, and the bonus mana is credited to regeneration metrics of their own.
    InnervateRegen {
        aura: String,
        spirit_regen_multiplier: f64,
        regen_metrics_action_id: ActionId,
    },
    /// The Orc racial Blood Fury: a major cooldown whose aura multiplies stats through Go's
    /// dynamic stat dependencies. `active_stats` holds every stat the aura changes, at the
    /// value Go computes while it is active.
    BloodFury {
        spell_id: i32,
        aura: String,
        active_stats: BTreeMap<String, f64>,
    },
    /// An on-use cooldown whose aura adds flat stats: Go `RegisterTemporaryStatsOnUseCD`, as
    /// the Night Elf racial Elune's Light. `active_stats` holds every stat the aura changes,
    /// at the value Go computes while it is active; `gain_log` and `expire_log` are the lines
    /// its gain and expiry log.
    TemporaryStats {
        spell_id: i32,
        /// The item whose use casts it, for an item's on-use cooldown.
        #[serde(default)]
        item_id: i32,
        aura: String,
        active_stats: BTreeMap<String, f64>,
        gain_log: String,
        expire_log: String,
    },
    /// An item use that activates its aura, which multiplies melee, ranged and cast speed while
    /// up: Go shared.NewSpellDataSpeedOnUse.
    SpeedOnUse {
        item_id: i32,
        aura: String,
        melee_multiplier: f64,
        ranged_multiplier: f64,
        cast_multiplier: f64,
    },
    /// The Diamond Flask (sim/warrior/items.go): a channel that is a self hot of five ticks, whose
    /// last tick activates a Strength aura. `active_stats` holds every stat the aura changes at
    /// the value Go computes while it is active; `gain_log` and `expire_log` are the lines its
    /// gain and expiry log.
    DiamondFlask {
        item_id: i32,
        aura: String,
        active_stats: BTreeMap<String, f64>,
        gain_log: String,
        expire_log: String,
    },
    /// movement.go: a player that moves, or whose movement speed an aura changes.
    /// `speed_multiplier` is the player's `PseudoStats.MovementSpeedMultiplier` after the reset,
    /// and `initial_speed_multiplier` what every reset restores before the permanent auras
    /// change it (the value after the reset where absent). `speed_auras` names every aura whose
    /// gain or fade changes it in the passive and active movement speed categories, which the
    /// runtime enforces and follows, and Elemental Blessing, which multiplies it directly.
    PlayerMovement {
        speed_multiplier: f64,
        #[serde(default)]
        initial_speed_multiplier: Option<f64>,
        speed_auras: Vec<String>,
    },
    /// sim/warrior/charge.go: the prepull cast that gives rage, triples the warrior's movement
    /// speed while its aura is up and moves it `overshoot` yards inside the spell's minimum range.
    WarriorCharge {
        spell_id: i32,
        aura: String,
        rage: f64,
        vanguard: bool,
        speed_multiplier: f64,
        overshoot: f64,
        min_range: f64,
        /// The client flags the cast No Threat: the rage it gives adds no threat.
        no_threat: bool,
    },
    /// The forms the druid starts in and each druid spell may be cast in.
    DruidForms {
        starting_form: Vec<String>,
        spells: Vec<DruidFormSpell>,
    },
    /// Moonkin Form's cast, which activates its aura.
    MoonkinForm {
        spell_id: i32,
        aura: String,
    },
    /// Every Starfire rank's direct hit.
    Starfire {},
    /// Every Wrath rank's hit after travel.
    Wrath {},
    /// Moonfire's hit, which casts its snapshotting dot spell when it lands.
    Moonfire {
        rank: FireballRank,
    },
    /// Insect Swarm's binary hit roll, then its snapshotting dot and the target debuff.
    InsectSwarm {
        rank: FireballRank,
        debuff_aura: String,
    },
    /// Source-native Forever Entangling Roots: timed periodic combat occurs in the Druid agent.
    EntanglingRoots {},
    /// Innervate's cast and aura: full spirit regeneration at a multiple, attributed to its own
    /// regeneration metrics.
    Innervate {
        spell_id: i32,
        aura: String,
        spirit_regen_multiplier: f64,
        regen_metrics_action_id: ActionId,
    },
    /// Omen of Clarity: landed spells can grant Clearcasting at a chance from their cast time,
    /// doubled with a halved cooldown in Moonkin Form.
    OmenOfClarity {
        trigger_aura: String,
        aura: String,
        callbacks: Vec<String>,
        outcome: Vec<String>,
        require_damage_dealt: bool,
        trigger_immediately: bool,
        proc_chance: f64,
        trigger_spells: Vec<usize>,
        icd_ns: i64,
        ppm: f64,
        gcd_ns: i64,
        moonkin_chance_multiplier: f64,
        moonkin_cooldown_multiplier: f64,
        cost_spells: Vec<usize>,
        cost_percent_add: f64,
        /// The spells that never trigger it in Cat or Bear Form: Faerie Fire since the 16 October
        /// 2026 beta notes.
        form_excluded_spells: Vec<usize>,
    },
    /// Nature's Grace: a damaging spell crit grants cast speed and a shorter GCD.
    NaturesGrace {
        trigger_aura: String,
        aura: String,
        haste_multiplier: f64,
        gcd_reduction_ns: i64,
        gcd_spells: Vec<usize>,
        trigger_spells: Vec<usize>,
    },
    /// Eclipse: each Wrath banks charges that shorten Starfire's cast.
    Eclipse {
        trigger_aura: String,
        aura: String,
        cast_time_reduction_ns: i64,
        charges_per_wrath: i32,
        duration_ns: i64,
    },
    /// Cat Form: its cast with Furor's energy carry over, and its aura's pseudo stat, weapon,
    /// movement speed and Faerie Fire changes. The initial values are the unit's before any
    /// aura applied; the exported pseudo stats include the form.
    CatForm {
        spell_id: i32,
        aura: String,
        initial_threat_multiplier: f64,
        threat_multiplier: f64,
        initial_spirit_regen_multiplier: f64,
        spirit_regen_multiplier: f64,
        initial_movement_speed_multiplier: f64,
        movement_speed_bonus: f64,
        furor_max: f64,
        cost_spells: Vec<usize>,
        gcd_spells: Vec<usize>,
        gcd_delta_ns: i64,
        form_breaking_spells: Vec<usize>,
        main_hand: Weapon,
        cat_weapon: Weapon,
    },
    /// Prowl: a prepull cast whose aura slows movement and lets the rotation act before each
    /// main hand swing until a hit ends it.
    Prowl {
        spell_id: i32,
        aura: String,
        movement_speed_multiplier: f64,
    },
    /// The cat's builders: the rank's flat damage plus main hand weapon damage, a combo point
    /// when it lands and a refund when it does not.
    CatBuilders {
        builders: Vec<CatBuilder>,
        cannot_shred: bool,
    },
    /// Rip: a physical bleed on combo points whose attack power share is read at each tick.
    Rip {
        spell: usize,
        tick_base: f64,
        tick_per_combo_point: f64,
        attack_power_share_per_combo_point: f64,
        attack_power_share_max_points: f64,
        tick_can_crit: bool,
        tick_magic: bool,
        expected_combo_points: f64,
        short_name: String,
    },
    /// Rake: a flat hit and a bleed of a flat tick, each plus a share of attack power. The
    /// bleed reads the share from the attack power each tick has, as Rip's does.
    Rake {
        spell: usize,
        flat_damage: f64,
        tick_base: f64,
        /// The share of attack power on the hit and on every tick, a Go literal.
        attack_power_share: f64,
        tick_can_crit: bool,
        tick_magic: bool,
        short_name: String,
    },
    /// Ferocious Bite: rolled damage per combo point and per point of excess energy.
    FerociousBite {
        spell: usize,
        damage_per_energy: f64,
        damage_per_combo_point: f64,
        attack_power_per_combo_point: f64,
        /// Predatory Discipline (1328613, hotfix push 113531): a landed bite keeps the energy
        /// left after its cost and still deals the damage it is worth.
        #[serde(default, skip_serializing_if = "is_false")]
        keeps_energy: bool,
    },
    /// Shifting Power: mana into energy.
    ShiftingPower {
        spell: usize,
        energy: f64,
    },
    /// Faerie Fire's hit and its target aura, with how the aura's exclusive effects read in
    /// this fight.
    FaerieFire {
        spell: usize,
        aura: String,
        armor_reduction: f64,
        refresh: Vec<String>,
    },
    /// Berserk: a cooldown whose aura raises the builders' critical strike chance.
    Berserk {
        spell_id: i32,
        aura: String,
        crit_percent: f64,
        crit_spells: Vec<usize>,
    },
    /// Blood Frenzy: a cat builder crit grants a combo point; the bear half needs Bear Form.
    BloodFrenzy {
        trigger_aura: String,
        bear_trigger_aura: String,
        proc_chance: f64,
        trigger_spells: Vec<usize>,
        outcome: Vec<String>,
        trigger_immediately: bool,
        metrics_action_id: ActionId,
        /// The bear half's melee spells and Rage.
        #[serde(default)]
        bear_trigger_spells: Vec<usize>,
        #[serde(default)]
        bear_rage: f64,
    },
    /// Bear Form, which a bear enters at each reset and keeps for the fight.
    BearForm {
        spell_id: i32,
        aura: String,
        /// The cast's position in the spellbook.
        spell: usize,
        /// The health the form's stat bonus adds to the maximum.
        health_bonus: f64,
        initial_threat_multiplier: f64,
        threat_multiplier: f64,
        initial_spirit_regen_multiplier: f64,
        spirit_regen_multiplier: f64,
        furor_proc_chance: f64,
        cost_spells: Vec<usize>,
        form_breaking_spells: Vec<usize>,
        main_hand: Weapon,
        bear_weapon: Weapon,
    },
    /// Enrage: instant Rage, Rage each period while its aura is up, and the aura's armor cut
    /// through the stat auras.
    Enrage {
        spell_id: i32,
        aura: String,
        instant_rage: f64,
        rage_per_tick: f64,
        ticks: i32,
        period_ns: i64,
    },
    /// Demoralizing Roar: a magic hit roll that activates the target debuff.
    DemoralizingRoar {
        spell: usize,
        aura: String,
    },
    /// Maul: a queue cast that arms its aura after a realism delay, and the strike the next
    /// main hand swing casts in its place.
    Maul {
        spell: usize,
        queue_spell: usize,
        queue_aura: String,
        realism_ns: i64,
        flat_damage: f64,
    },
    /// Lacerate: a weapon share a stack, and a stacking bleed of a flat tick a stack.
    Lacerate {
        spell: usize,
        tick_base: f64,
        weapon_share_per_stack: f64,
        max_stacks: i32,
        tick_can_crit: bool,
        tick_magic: bool,
    },
    /// Primal Bite: flat damage plus main hand weapon damage; Berserk lifts its cooldown.
    PrimalBite {
        spell: usize,
        flat_damage: f64,
    },
    /// Swipe: a flat hit plus a share of the attack power on each of the first three targets.
    Swipe {
        spell: usize,
        flat_damage: f64,
        attack_power_coefficient: f64,
    },
    /// Hurricane's channel: each period casts the triggered tick spell, a hit-checked fixed
    /// amount on every target.
    Hurricane {
        spell_id: i32,
        tick_spell_id: i32,
        tick_base: f64,
    },
    /// Barkskin: a cooldown whose aura's physical damage taken cut is a stat aura; cast in a
    /// fight it restarts the main hand swing.
    Barkskin {
        spell_id: i32,
        aura: String,
    },
    /// Thorns (Forever's rules): the cast turns on the druid's own shield, which strikes back
    /// through the shared damage shield effects.
    DruidThorns {
        spell_id: i32,
        aura: String,
    },
    /// Frenzied Regeneration: each period while its aura is up, up to a cap of Rage becomes a
    /// share of maximum health a point.
    FrenziedRegeneration {
        spell: usize,
        aura: String,
        ticks: i32,
        period_ns: i64,
        max_rage_per_tick: f64,
        health_share_per_rage: f64,
        healing_taken_multiplier: f64,
    },
    /// Feralheart Raiment's four piece Nature's Bounty: a chance at mana on a spell's cast, energy
    /// on a landed white hit, or Rage when a melee attack strikes the druid.
    NaturesBounty {
        aura: String,
        proc_chance: f64,
        mana_label: String,
        mana: f64,
        mana_spells: Vec<usize>,
        energy_label: String,
        energy: f64,
        energy_spells: Vec<usize>,
        rage_label: String,
        rage: f64,
        metrics_action_id: ActionId,
    },
    /// Symbols of Unending Life's three piece bonus: energy when a finisher does not land.
    UnendingLifeRefund {
        aura: String,
        energy: f64,
        spells: Vec<usize>,
        label: String,
        metrics_action_id: ActionId,
    },
    /// Natural Reaction: Rage when the druid dodges in Bear Form.
    NaturalReaction {
        trigger_aura: String,
        proc_chance: f64,
        outcome: Vec<String>,
        trigger_immediately: bool,
        rage: f64,
        metrics_action_id: ActionId,
    },
    /// Rend and Tear: the target takes more from the druid's special attacks while it bleeds.
    RendAndTear {
        multiplier: f64,
        spells: Vec<usize>,
        bleed_spells: Vec<usize>,
    },
    /// Go exclusive_effect.go ShouldRefreshExclusiveEffects for an aura a rotation asks
    /// about: how each of its exclusive effects reads in this fight.
    AuraShouldRefresh {
        unit: String,
        aura: String,
        modes: Vec<String>,
    },
    /// Auras whose gain and expiry change stats through Go's AddStatsDynamic, and the player's
    /// stats Rust reads for every combination of them: entry i has aura j active when bit j
    /// of i is set. `changed` names every stat any combination changes. An aura whose stats
    /// follow its stacks (Go `core.MakeStackingAura`) takes the bits that count its stacks, so
    /// `stacks` lists its maximum stacks, or 0 for an aura read as active or not, and aura j's
    /// bits start where the bits of the auras before it end; a count past the maximum reads as
    /// the maximum. It is absent when no aura stacks. `raw_mp5` holds the exact MP5 bonus of
    /// each aura whose bonus is not added and taken away exactly: Go adds a bonus to the raw
    /// stats and takes it away again, which leaves a fraction a few ulps off, so the runtime
    /// does the same instead of reading the combination.
    StatAuras {
        auras: Vec<String>,
        combos: Vec<BTreeMap<String, f64>>,
        changed: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        stacks: Vec<i32>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        raw_mp5: BTreeMap<String, f64>,
    },
    /// The Crusader weapon enchant: a weapon proc at a per-spell chance that activates the
    /// hand's Holy Strength and heals.
    Crusader {
        trigger_aura: String,
        mh_aura: String,
        oh_aura: String,
        chances: Vec<SpellChance>,
        heal_min: f64,
        heal_max: f64,
        heal_metrics_action_id: ActionId,
        mh_gain_log: String,
        mh_expire_log: String,
        oh_gain_log: String,
        oh_expire_log: String,
    },
    /// A set bonus proc (common/forever setStatProc): each spell's chance from the trigger's
    /// proc manager on landed hits, keyed by the trigger's name, and the temporary stats aura
    /// it activates a spell batch window later, with its log lines.
    StatProc {
        trigger_aura: String,
        rng_label: String,
        aura: String,
        chances: Vec<SpellChance>,
        /// Absent for a parsed aura, as a weapon proc's: it logs neither.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gain_log: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expire_log: Option<String>,
        /// The handler also adds a stack of the aura after it activates it, as Bonereaver's Edge
        /// does.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        add_stack: bool,
    },
    /// Paladin talents_holy.go Illumination: a heal crit's chance to return a share of the
    /// heal's base cost, a batch window later.
    Illumination {
        trigger_aura: String,
        proc_chance: f64,
        refund: f64,
        metrics_action_id: ActionId,
    },
    /// Dragonbreath Chili: a chance on landed melee hits to cast a rolled Fire hit, after a
    /// spell batch window.
    DragonbreathChili {
        trigger_aura: String,
        spell_id: i32,
        proc_chance: f64,
        trigger_spells: Vec<usize>,
        roll_min: f64,
        roll_max: f64,
        delay_ns: i64,
    },
    /// The party Windfury Totem: the totem aura refreshed every period holds a trigger that can
    /// grant charges of attack power and cast an extra main hand attack; landed autos spend the
    /// charges.
    WindfuryTotem {
        totem_aura: String,
        period_ns: i64,
        trigger_aura: String,
        trigger_spells: Vec<usize>,
        trigger_outcome: Vec<String>,
        trigger_proc_chance: f64,
        proc_aura: String,
        spend_spells: Vec<usize>,
        spend_outcome: Vec<String>,
        /// The extra main hand attack; absent without melee autos, when no spell can trigger
        /// the totem.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extra_attack_spell: Option<usize>,
        /// Go `ProcTrigger.RequireDamageDealt` of each trigger: a hit that deals no damage,
        /// such as a landed Mutilate's own roll, does not reach it.
        trigger_require_damage: bool,
        spend_require_damage: bool,
    },
    /// The raid's Sunder Armor, ramped one stack a period from the pull; target armor at
    /// each stack count, as Go computes it.
    SunderArmorRamp {
        aura: String,
        period_ns: i64,
        ticks: i32,
        armor_by_stacks: Vec<f64>,
        /// A stronger permanent member of the aura's exclusive category, such as the raid's
        /// Expose Armor, blocks every activation, which Go still counts as a proc.
        #[serde(default)]
        blocked: bool,
    },
    /// The raid's Expose Armor under Forever's rules: the target opens the fight at
    /// `armor_before`, its armor without Expose Armor, and the aura lands at full `delay_ns`
    /// into the fight, taking the target to its prepared armor. The raid's Sunder Armor ramps
    /// a stack every default GCD from the pull (`SunderArmorRamp`); landing at 3 s gives the
    /// same armor-seconds over the opening, so a player gets the same damage from either.
    ExternalExposeArmor {
        aura: String,
        delay_ns: i64,
        armor_before: f64,
    },
    /// Paladin judgement.go: a landed melee strike refreshes the active judgement debuffs.
    JudgementRefresh {
        trigger_aura: String,
        proc_mask: Vec<String>,
        judgement_auras: Vec<String>,
    },
    /// Paladin judgement.go: Judgement casts the active seal's judgement, then wakes the
    /// rotation a delay after its cooldown ends.
    Judgement {
        spell_id: i32,
        wake_delay_ns: i64,
    },
    /// Paladin seal_of_command.go: every rank's seal and judgement, and the proc its seal and
    /// Echo roll on landed white hits.
    SealOfCommand {
        ranks: Vec<SealOfCommandRank>,
        proc_spell_id: i32,
        weapon_percent: f64,
        coefficient: f64,
        /// The share of the target's extra Holy damage taken a proc adds: the coefficient times
        /// Improved Seals (MythicSim patch 169).
        target_bonus_coefficient: f64,
        proc_chance: f64,
        rng_label: String,
        icd_ns: i64,
        deal_delay_ns: i64,
    },
    /// Paladin seal_of_righteousness.go: every rank's seal, judgement and per-hit proc.
    /// sim/core/item_swaps.go: the swap between the request's equipment (the Main set) and the
    /// item swap's set (Swap1) of the slots the swap takes: the set auras, the swap's equipment
    /// stats change line each way, the callbacks of the swapped slots in Go's order, the values of
    /// the prepared state that differ with the swap set worn, and the log lines of the stat auras
    /// the callbacks switch.
    ItemSwap {
        slots: Vec<String>,
        main_aura: String,
        swapped_aura: String,
        to_swapped_log: String,
        to_main_log: String,
        callbacks: Vec<SwapCallback>,
        swapped: Vec<SwapChange>,
        stat_aura_logs: Vec<SwapAuraLogs>,
        /// Auras the unit's reset activates, as a proc trigger's `OnReset` does, that the swap's
        /// reset then switches off for the request's set.
        reset_activates: Vec<String>,
        /// What switched auras do beyond their stats as they are gained and fade.
        aura_effects: Vec<SwapAuraEffect>,
    },
    SealOfRighteousness {
        ranks: Vec<SealOfRighteousnessRank>,
        hand_multiplier: f64,
        swing_speed: f64,
        deal_delay_ns: i64,
    },
    /// Paladin seal_of_fury.go: every rank's seal, judgement and per-hit Holy damage, dealt a
    /// batch window after its hit, and with a shield the absorb aura of a share of it; Improved
    /// Seal of Fury's mana when a shield is spent.
    SealOfFury {
        ranks: Vec<SealOfFuryRank>,
        can_block: bool,
        deal_delay_ns: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        improved: Option<ImprovedSealOfFury>,
    },
    /// Paladin seal_of_light.go: every rank's seal, self heal and judgement. A landed melee hit
    /// that is no proc rolls the 15 PPM manager's chance for the spell under the seal's label,
    /// and a batch window later the heal lands on the paladin through the healing multipliers.
    /// The judgement's debuff heals through a `judgement_of_light` effect.
    SealOfLight {
        ranks: Vec<SealOfLightRank>,
        chances: Vec<SpellChance>,
        healing_dealt_multiplier: f64,
        healing_taken_multiplier: f64,
        table_healing_dealt_multiplier: f64,
    },
    /// Paladin seal_of_wisdom.go: every rank's seal, mana restore and judgement. A landed melee
    /// hit that is no proc runs the handler a batch window later, which casts the restore on the
    /// paladin. The judgement's debuff returns mana through a `judgement_of_wisdom` effect.
    SealOfWisdom {
        ranks: Vec<SealOfWisdomRank>,
    },
    /// Paladin seal_of_justice.go: the seal, whose stun raid bosses are immune to, and the
    /// judgement, whose debuff does nothing in the sim.
    SealOfJustice {
        seal_spell_id: i32,
        aura: String,
        judgement_spell_id: i32,
        judgement_aura: String,
    },
    /// Paladin seal_of_the_crusader.go: every rank's seal and judgement, and the spells the
    /// seal's main hand auto damage mod applies to. The judgement auras' Holy damage is their
    /// bid in the "Judgement of the Crusader" exclusive category.
    SealOfTheCrusader {
        ranks: Vec<SealOfTheCrusaderRank>,
        auto_spells: Vec<usize>,
    },
    /// Paladin talents_holy.go Infusion of Light and the Libram of Holy Alacrity
    /// (item_librams.go): a Holy Shock crit, or any Holy Shock cast, activates an aura that
    /// only speeds a Holy Light cast. `on_crit` is true for Infusion of Light.
    HolyLightHaste {
        trigger_aura: String,
        aura: String,
        on_crit: bool,
        /// The cast time the aura takes off each Holy Light rank, and their spells; a Holy
        /// Light cast spends the aura.
        cast_time_ns: i64,
        holy_light_spells: Vec<usize>,
    },
    /// Paladin holy_light.go, flash_of_light.go and holy_shock.go: every heal rank's roll,
    /// the Libram of Light's Flash of Light bonus, Blessing of Light's bonuses, and the
    /// healing modifiers on the paladin and on the target, which a plain cast heals.
    /// A castable paladin aura (Forever's rules): each rank's cast turns its aura on, which
    /// takes the paladin aura categories from the last one.
    PaladinAuraCast {
        ranks: Vec<PaladinAuraRank>,
    },
    PaladinHeals {
        ranks: Vec<PaladinHealRank>,
        flash_of_light_bonus: f64,
        blessing_holy_light: f64,
        blessing_flash_of_light: f64,
        player: PaladinHealUnit,
        target: PaladinHealUnit,
    },
    /// Druid Bloom (hotfix push 113531, Forever's rules, healer sims): a heal over time of
    /// `hots` that ends by its duration heals the player for its Spirit, as the spell `spell_id`.
    Bloom {
        spell_id: i32,
        hots: Vec<i32>,
    },
    /// MythicSim's healer sims (Rust only): every heal rank a class registers for the player,
    /// and the player's healing modifiers at reset with its bonus healing taken.
    Heals {
        heals: Vec<HealSpell>,
        player: HealModifiers,
        bonus_healing_taken: f64,
        /// Forever's rules: the party's damage, when a heal follows it ([`SmartHeal`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        party: Option<PartyDamage>,
    },
    /// The healer Priest's talents that act on the party rather than on one heal (Rust only,
    /// Forever's rules): what the heals of this priest cannot state by themselves.
    PriestHealTalents {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        renewed_hope: Option<RenewedHope>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        litany_of_light: Option<LitanyOfLight>,
    },
    /// Paladin lay_on_hands.go: each rank the rotation names drains the paladin's mana,
    /// restores mana to the paladin when it heals the paladin, and heals for the paladin's
    /// maximum health with the healing crit roll.
    LayOnHands {
        ranks: Vec<LayOnHandsRank>,
    },
    /// Paladin talents_retribution.go Eye for an Eye: a crit taken, a batch window later,
    /// deals a share of its damage back as Holy damage, at most half of maximum health.
    EyeForAnEye {
        trigger_aura: String,
        spell_id: i32,
        share: f64,
    },
    /// Paladin talents_retribution.go Pursuit of Justice: the permanent aura's passive
    /// movement speed, which only logs in scope.
    PursuitOfJustice {
        aura: String,
        initial_multiplier: f64,
        bonus: f64,
    },
    /// Paladin holy_strike.go: every rank's weapon percent; the flat roll is on the spell.
    HolyStrike {
        ranks: Vec<HolyStrikeRank>,
    },
    /// Paladin hammer_of_wrath.go: the damage rolls are on the spells.
    HammerOfWrath {},
    /// Paladin hammer_of_the_righteous.go (Forever's rules): Holy damage of `weapon_dps` times
    /// the main hand's damage a second, on the melee special table, at the target and the next
    /// ones up to `max_targets`, each calculated and dealt in turn.
    HammerOfTheRighteous {
        spell_id: i32,
        weapon_dps: f64,
        max_targets: i32,
    },
    /// Paladin exorcism.go: the rolls are on the spells; it hits only an Undead or Demon.
    Exorcism {},
    /// Paladin holy_wrath.go: the rolls are on the spells; a cast that pauses the swing and
    /// hits only an Undead or Demon, after travel.
    HolyWrath {},
    /// Paladin consecration.go: every rank's tick and the bonus the first targets take, and
    /// Consecrated Ground's mark when talented.
    Consecration {
        ranks: Vec<ConsecrationRank>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        consecrated_ground: Option<ConsecratedGround>,
    },
    /// Paladin righteous_fury.go: the aura's Holy threat mod, and Instrument of Law's threat
    /// reduction that holds only while the aura is down.
    RighteousFury {
        spell_id: i32,
        aura: String,
        threat_percent: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instrument_of_law: Option<AuraMultiplier>,
        /// Push 113554 under Forever's rules: the multiplier on all the paladin's threat while
        /// the aura holds (effect 1367626, +33%).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        all_threat_multiplier: Option<f64>,
    },
    /// Paladin swift_judgement.go: a major cooldown that finishes Judgement's cooldown and
    /// makes the next Judgement free.
    SwiftJudgement {
        spell_id: i32,
        aura: String,
        cost_percent_add: f64,
    },
    /// Paladin templars_bulwark.go: a survival cooldown Go never fires at no health threshold.
    TemplarsBulwark {
        spell_id: i32,
        /// The absorb shield's aura and the share of maximum health it absorbs.
        aura: String,
        health_share: f64,
    },
    /// Paladin talents_protection.go Redoubt: landed melee hits taken can raise block chance
    /// for a few blocks.
    Redoubt {
        trigger_aura: String,
        aura: String,
        proc_chance: f64,
    },
    /// Paladin talents_protection.go Shield Specialization: blocks restore mana.
    ShieldSpecialization {
        trigger_aura: String,
        proc_chance: f64,
        mana_share: f64,
        metrics_action_id: ActionId,
        /// Its mana makes no threat (client 70374).
        no_threat: bool,
    },
    /// Paladin talents_protection.go Reckoning: blocks and crits taken can grant an extra
    /// attack.
    Reckoning {
        block_aura: String,
        crit_aura: String,
        block_chance: f64,
        crit_chance: f64,
        /// The charges each trigger adds behind the shared cooldown, which the next main hand
        /// auto spends on one free main hand swing each (16 October 2026 beta notes).
        charges_aura: String,
        icd_ns: i64,
        /// The spells that spend the charges: every main hand auto.
        spender_spells: Vec<usize>,
    },
    /// Paladin talents_protection.go Iron Creed: a landed Holy Strike under Righteous Fury
    /// lowers damage taken.
    IronCreed {
        trigger_aura: String,
        aura: String,
    },
    /// Paladin holy_shield.go: each rank the rotation names, with its block chance aura,
    /// whose charges blocks spend to deal Holy damage.
    HolyShield {
        ranks: Vec<HolyShieldRank>,
        /// The cast's condition: a shield to block with.
        can_block: bool,
    },
    /// Paladin lights_vigil.go: each rank's vigil on the target, the strike Holy Shock fires
    /// on a vigiled target instead of its damage, and the refund of the vigil's cost.
    LightsVigil {
        ranks: Vec<LightsVigilRank>,
    },
    /// Light's Vigil on an ally (push 113554, the Holy Paladin healer sim under Forever's rules):
    /// the cast puts a vigil on the player for 30 sec, one at a time, and the next Holy Shock
    /// heal on the player spends it, triggers no cooldown and heals the player's whole party.
    LightsVigilAlly {
        ranks: Vec<LightsVigilAllyRank>,
    },
    /// Paladin holy_shock.go: every rank's damage roll.
    HolyShock {
        ranks: Vec<HolyShockRank>,
    },
    /// Paladin divine_favor.go: a major cooldown whose aura raises the crit of the spells it
    /// names until one of them is cast.
    DivineFavor {
        spell_id: i32,
        aura: String,
        crit: f64,
        spells: Vec<String>,
    },
    /// Paladin oath_of_light.go (hotfix push 113554, Forever's rules): the cast swears the Oath
    /// on the healed ally for the aura's 5 minutes, and its Holy Light, Flash of Light and Holy
    /// Shock heals add the flat bonus to their base amount while it holds. Boundless Light's crit
    /// on the Holy Shock heal holds only without it.
    OathOfLight {
        spell_id: i32,
        aura: String,
        holy_light: f64,
        flash_of_light: f64,
        holy_shock: f64,
        boundless_light_crit: f64,
        /// The share of the party's heals that reach the Oath's holder, among the players the
        /// healer's heals land on: the tank's share of the party's hits. A heal that does not
        /// reach the holder gets none of the flat bonus, and a Holy Shock heal on it has
        /// Boundless Light's crit. One is the Oath on every heal.
        #[serde(default = "one")]
        holder_share: f64,
    },
    /// An item proc common/shared/shared_utils.go builds from client rows: a listener that
    /// casts a single target magic hit at once on the unit hit. A weapon proc rolls its proc
    /// manager's chance for each spell in `chances` in place of `proc_chance`.
    SpellDataDamageProc {
        trigger_aura: String,
        trigger_spells: Vec<usize>,
        /// A "when struck" proc: it hears the melee and ranged hits the player takes, which
        /// carry no spellbook position, and answers the attacker.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        struck: bool,
        /// The listener hears the spells the player casts, not the hits they deal: it checks no
        /// outcome and no damage, since a cast carries no result, and answers the current
        /// target.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        casts: bool,
        landed_only: bool,
        require_damage: bool,
        proc_chance: f64,
        spell: usize,
        average: f64,
        variance: f64,
        can_crit: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chances: Option<Vec<SpellChance>>,
        /// A Go literal range the hit rolls, as `NewProcDamageEffect` items state, in place of
        /// the client effect's average and variance.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        roll: Option<[f64; 2]>,
        /// The outcome applier of a `roll` hit by name, as shared_utils.go `damageOutcome` picks
        /// it from the spell's defense type: `melee_special_hit_and_crit` for a hit of the melee
        /// defense type, as Iceblade Hacker's, or `melee_special_hit` when it cannot crit.
        /// Absent, the hit rolls the magic hit table, with a crit when `can_crit`.
        ///
        /// A weapon proc built from a client row (`CreateWeaponProcSpell` over
        /// `SpellDataProcDamageSpell`) names it without a `roll` as well: the row's effect
        /// rolled on the melee or ranged table of the spell's defense type.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<String>,
        /// A chain the row's effect makes: the hit rolls once for each target it reaches, from
        /// the unit hit on, keeping `amp` of the damage at each jump.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chain: Option<ProcChain>,
        /// An area the row's effect hits instead: every target, or the row's cap of them from
        /// the unit hit on.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        area: Option<ProcArea>,
        /// The damage over time the row carries, on the targets the proc landed on.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        periodic: Option<ProcPeriodic>,
        /// A Go literal proc trigger whose handler waits this long before it casts the hit, as
        /// item_sets_classic.go `setMeleeDamageProc` does a spell batch window later on the
        /// unit the trigger hit. Absent, the hit is cast at once.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delay_ns: Option<i64>,
        /// The label the trigger's chance rolls under, where it is not the trigger aura's: the
        /// trigger's own name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rng_label: Option<String>,
    },
    /// common/shared/shared_utils.go NewSpellDataAbsorbOnUse: the item use's aura shields the
    /// wearer for the absorb effect's roll against the schools its bits name.
    AbsorbOnUse {
        item_id: i32,
        aura: String,
        schools: u8,
        average: f64,
        variance: f64,
    },
    /// common/shared/shared_utils.go NewSpellDataAbsorbProc: a listener on the melee hits the
    /// player takes, resolved from its trigger row, that casts the absorb row's spell, by
    /// spellbook position, on the wearer at once; the spell's aura shields the wearer for the
    /// absorb effect's roll against the schools its bits name.
    SpellDataAbsorbProc {
        trigger_aura: String,
        /// Go `HitOutcome` names the listener hears; empty hears every outcome.
        outcome: Vec<String>,
        require_damage: bool,
        proc_chance: f64,
        spell: usize,
        aura: String,
        schools: u8,
        average: f64,
        variance: f64,
    },
    /// common/shared/shared_utils.go NewSpellDataHealOnUse: the item use heals the wearer
    /// directly, a share of maximum health or a rolled amount.
    HealOnUse {
        item_id: i32,
        can_crit: bool,
        healing_dealt_multiplier: f64,
        healing_taken_multiplier: f64,
        table_healing_dealt_multiplier: f64,
        bonus_healing_taken: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_health_share: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        average: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        variance: Option<f64>,
    },
    /// common/classic/items_trinkets.go Second Wind: the use restores mana each period for a
    /// number of ticks on its own metrics; the automatic use waits for the deficit.
    SecondWind {
        item_id: i32,
        mana: f64,
        ticks: i32,
        period_ns: i64,
        metrics_spell_id: i32,
        min_deficit: f64,
    },
    /// An enchant proc common/shared/shared_utils.go builds from client rows: a listener that
    /// casts a direct heal on the wearer at once, a share of maximum health or a rolled amount.
    SpellDataHealProc {
        trigger_aura: String,
        trigger_spells: Vec<usize>,
        /// A "when struck" proc: it hears the melee hits the player takes, which carry no
        /// spellbook position.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        struck: bool,
        /// Go `HitOutcome` names the listener hears; empty hears every outcome.
        outcome: Vec<String>,
        require_damage: bool,
        proc_chance: f64,
        spell: usize,
        can_crit: bool,
        healing_dealt_multiplier: f64,
        healing_taken_multiplier: f64,
        table_healing_dealt_multiplier: f64,
        bonus_healing_taken: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_health_share: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        average: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        variance: Option<f64>,
        /// The trigger hears the casts of `trigger_spells` instead of their hits: a healer's
        /// gear under Forever's rules (Rotmender's Raiment's Rotmending).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        casts: bool,
    },
    /// common/shared/shared_utils.go NewSpellDataDamageOnUse: an on-use item's spell, by
    /// spellbook position, that deals its row's hit or damage over time on the one target.
    DamageOnUse {
        item_id: i32,
        spell: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        direct: Option<OnUseDirect>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        periodic: Option<OnUsePeriodic>,
    },
    /// common/forever item_sets_classic.go Battlegear of Valor's Warrior's Resolve: landed
    /// melee hits roll each spell's chance from the set's proc manager under the label; a
    /// batch window later the handler heals a roll of the range and gives the rage.
    HealthRageProc {
        trigger_aura: String,
        rng_label: String,
        chances: Vec<SpellChance>,
        heal_min: f64,
        heal_max: f64,
        rage: f64,
        metrics_action_id: ActionId,
    },
    /// classic items_weapons.go Runeblade of Baron Rivendare's Unholy Aura (Forever's rules): its
    /// gain starts a periodic action that gives the wearer `health` every `period_ns`, which its
    /// expiry cancels.
    HealthRegenAura {
        aura: String,
        period_ns: i64,
        health: f64,
        metrics_action_id: ActionId,
    },
    /// Oil of Immolation (MythicSim Rust only, client 70707 item 8956): the character uses the oil
    /// `prepot_ns` before the pull and again as its `aura` ends, and each of the aura's `ticks`
    /// periods casts the tick `spell` (a Fire hit of `damage` on every target) if the player
    /// stands within `radius` yards. A use while the player's melee swings are running pushes
    /// each pending swing back by `swing_delay_ns`; the use before the pull finds none running.
    /// A prepared value without the two durations (an older one) uses the oil at the pull and
    /// delays nothing.
    OilOfImmolation {
        item_id: i32,
        aura: String,
        spell: usize,
        period_ns: i64,
        ticks: i32,
        radius: f64,
        damage: f64,
        #[serde(default)]
        swing_delay_ns: i64,
        #[serde(default)]
        prepot_ns: i64,
    },
    /// core/battle_squawk.go (Forever's rules): the party's squawks, up from the reset in the
    /// exported melee state, take their melee speed back as their stacks fall to none at the
    /// aura's expiry, `per_stack` each.
    BattleSquawk {
        aura: String,
        per_stack: f64,
    },
    /// items_trinkets.go Mark of the Champion (Forever's rules, targets of its mob types): while
    /// the aura holds, the player's attack tables add the bonus. Go's melee attack power reads
    /// `ranged_attack_power`, its ranged attack power the same, and a magic spell's bonus damage
    /// `spell_damage`.
    MobTypeBonus {
        aura: String,
        attack_power: f64,
        ranged_attack_power: f64,
        spell_damage: f64,
        /// Under Forever's rules, melee attack power against the targets' type alone
        /// (`A_MOD_MELEE_ATTACK_POWER_VERSUS`), as Quel'Serrar's Sanctuary grants against
        /// Dragonkin; ranged attacks do not read it.
        #[serde(default, skip_serializing_if = "is_zero_f64")]
        melee_attack_power: f64,
    },
    /// core/enchant_speed.go `registerSpeedAura` (Forever's rules, with an item swap): an item
    /// or enchant's speed aura multiplies the melee, ranged and cast speed while up. The one
    /// worn from the start is in the exported speeds; the swap gains and expires them.
    SpeedAura {
        aura: String,
        melee_multiplier: f64,
        ranged_multiplier: f64,
        cast_multiplier: f64,
    },
    /// Rogue items.go Darkmantle Armor (4), Rogue Armor Energize (Forever's rules): landed hits
    /// roll each spell's chance from the set's proc manager under the label; a batch window
    /// later the handler gives `energy` to a player with an energy bar.
    EnergyProc {
        trigger_aura: String,
        rng_label: String,
        chances: Vec<SpellChance>,
        energy: f64,
        metrics_action_id: ActionId,
    },
    /// common/forever items_weapons.go Bashguuder and Rivenspike: a weapon proc whose handler,
    /// a batch window later, stacks the target's Puncture Armor; each stack count sets the
    /// target's armor change, as Go measures it.
    ArmorDebuffProc {
        trigger_aura: String,
        rng_label: String,
        chances: Vec<SpellChance>,
        aura: String,
        armor_by_stacks: Vec<f64>,
        /// The handler runs at once on the hit, as Annihilator's does, rather than a batch
        /// window later.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        immediate: bool,
    },
    /// Paladin talents_retribution.go Vengeance: crits stack a Holy and Physical damage mod.
    Vengeance {
        trigger_aura: String,
        aura: String,
        per_stack: f64,
        spells: Vec<usize>,
    },
    /// Paladin talents_retribution.go Vindication: landed melee hits activate the target's
    /// aura and the paladin's attack power aura.
    Vindication {
        trigger_aura: String,
        proc_chance: f64,
        /// The paladin's attack power aura, which push 113554 removes under Forever's rules.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        aura: Option<String>,
        target_aura: String,
    },
    /// Paladin talents_retribution.go Sanctified Judgement: Judgement returns part of the
    /// active seal's cost.
    SanctifiedJudgement {
        trigger_aura: String,
        proc_chance: f64,
        refund: f64,
        metrics_action_id: ActionId,
    },
    /// Paladin talents_retribution.go Sacred Arbiter: a landed Holy Strike refreshes every
    /// judgement aura on the target.
    SacredArbiter {
        trigger_aura: String,
        judgement_auras: Vec<String>,
    },
    /// Paladin talents_retribution.go Twist of Light: a replaced seal leaves an Echo that the
    /// next landed white hit consumes.
    TwistOfLight {
        trigger_aura: String,
        echoes: Vec<SealEcho>,
    },
    /// Paladin Unsealed Wrath (push 113554, Forever's rules): landed melee autos store the
    /// active seal in its Echo, up to the Echo's stacks; one landed with a different seal spends
    /// every other Echo's stacks, each a roll of the stored seal.
    UnsealedWrath {
        trigger_aura: String,
        echoes: Vec<SealEcho>,
    },
    /// Paladin Swift Judgement (push 113554, Forever's rules): a second Judgement on its own
    /// cooldown, which unleashes the active seal's judgement, free and off the global cooldown.
    SecondJudgement {
        spell_id: i32,
    },
    /// Paladin Shieldsworn (push 113554, Forever's rules): a landed Holy Strike triggers the
    /// active Holy Shield's damage without a charge and Shield Specialization's mana chance
    /// without its cooldown.
    Shieldsworn {},
    /// Every Mind Blast rank's direct hit.
    MindBlast {},
    /// Every Shadow Word: Death rank's direct hit; Early Demise adds crit in the 20% execute
    /// phase.
    ShadowWordDeath {
        early_demise_crit: f64,
    },
    /// Every Shadow Word: Pain rank: a hit roll without a hit count, then a snapshotting dot
    /// whose ticks roll only a crit.
    ShadowWordPain {
        ranks: Vec<FireballRank>,
    },
    /// Every Devouring Plague rank: Shadow Word: Pain's shape, each tick healing the priest for
    /// its damage under the rank's action ID with this tag.
    DevouringPlague {
        ranks: Vec<FireballRank>,
        heal_metrics_tag: i32,
    },
    /// Every Mind Flay rank: a binary hit roll, then a channel.
    MindFlay {
        ranks: Vec<FireballRank>,
    },
    /// Improved Mind Flay (client 70425): each Mind Flay tick deals `ramp_per_tick` more than the
    /// one before. Compounding (Rust only) multiplies each tick by it over the tick before, as the
    /// tooltip reads; without it the step is added per tick, as the Go reference has it.
    ImprovedMindFlay {
        ramp_per_tick: f64,
        #[serde(default, skip_serializing_if = "is_false")]
        compounding: bool,
    },
    /// Dark Inspiration (client 70425): Mind Blast's cast and a full Mind Flay channel's last tick
    /// activate `aura`, a stat aura.
    DarkInspiration {
        aura: String,
        mind_blast_trigger: String,
        mind_flay_trigger: String,
    },
    /// Holy Nova: each rank's rolled hit, then its triggered heal on the priest, with the
    /// healing modifiers Go reads.
    HolyNova {
        ranks: Vec<HolyNovaRank>,
        healing_dealt_multiplier: f64,
        healing_taken_multiplier: f64,
        table_healing_dealt_multiplier: f64,
        healing_power: f64,
    },
    /// The Shadowfiend's summon: the pet for the timeline aura's duration, its attack power
    /// inherited from the priest's spell and shadow damage at each summon, and its mana restore
    /// aura, active while it is out.
    Shadowfiend {
        spell_id: i32,
        aura: String,
        duration_ns: i64,
        pet: String,
        attack_power_coefficient: f64,
        attack_power_without_deps: f64,
        attack_power_dependency_terms: Vec<f64>,
        stats: Vec<NamedStat>,
        /// The same stats once the fight's end has expired the pet's permanent auras, where a
        /// party aura on the pet changes them (Go patch 131); its attack power is unchanged.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fight_end_stats: Vec<NamedStat>,
        mana_restore_aura: String,
        mana_restore_fraction: f64,
        mana_restore_action_id: i32,
    },
    /// A copy of the Power Infusion aura, the priest's own or the external caster's (one effect
    /// each): its multipliers apply to the damage of the named school indexes and to healing
    /// dealt while the copy holds the aura's exclusive categories.
    PowerInfusion {
        spell_id: i32,
        aura: String,
        damage_multiplier: f64,
        schools: Vec<usize>,
        healing_multiplier: f64,
    },
    /// Starshards, the Night Elf priest's racial: a hit roll, then a snapshotting channel.
    Starshards {
        ranks: Vec<FireballRank>,
    },
    /// Shadowform's cast and aura: Shadow damage and cost modifiers on `school_spells`, a crit
    /// damage bonus on `crit_spells`, and helpful Holy casts in `cancel_spells` end it.
    Shadowform {
        spell_id: i32,
        aura: String,
        damage_percent: f64,
        cost_percent: f64,
        crit_multiplier: f64,
        school_spells: Vec<usize>,
        crit_spells: Vec<usize>,
    },
    /// Inner Focus: the next priest spell is free and gains crit; the cooldown restarts when
    /// the aura ends.
    InnerFocus {
        spell_id: i32,
        aura: String,
        cost_percent: i32,
        crit_percent: f64,
        crit_spells: Vec<usize>,
        spender_spells: Vec<usize>,
    },
    /// Priest talents_shadow.go applyVampiricEmbrace (Forever's rules): the cast always lands
    /// and puts up `aura` on the target; a batch window after each of the priest's Shadow hits
    /// or ticks on it that deals damage, the priest gains `heal_fraction` of the damage as
    /// health under the metrics of `metrics_action_id`.
    VampiricEmbrace {
        spell_id: i32,
        aura: String,
        heal_fraction: f64,
        metrics_action_id: ActionId,
    },
    /// Shadow Weaving: landed Shadow spells stack a Shadow damage bonus.
    ShadowWeaving {
        trigger_aura: String,
        aura: String,
        callbacks: Vec<String>,
        outcome: Vec<String>,
        trigger_immediately: bool,
        proc_chance: f64,
        trigger_spells: Vec<usize>,
        /// Each stack of the target's debuff `aura` multiplies this priest's Shadow damage on
        /// that target (community #777).
        damage_per_stack: f64,
    },
    /// Dark Sacrifice: a self-only periodic mana gain of the client base plus Spirit over a
    /// divisor, a major cooldown used once the whole gain fits.
    DarkSacrifice {
        spell_id: i32,
        aura: String,
        tick_base: f64,
        spirit_divisor: f64,
        metrics_action_id: ActionId,
        /// The client flags the cast No Threat: the mana it gives adds no threat.
        no_threat: bool,
    },
    /// Every Smite rank's cast on its own client row.
    Smite {},
    /// Every Holy Fire rank: the hit rolls, a landed hit applies a snapshotting dot, then the
    /// hit is dealt.
    HolyFire {
        ranks: Vec<FireballRank>,
    },
    /// Penance: a hit roll without a hit count, then a channel that ticks on application and
    /// each second after.
    /// Penance: every rank, each with its own bolt.
    Penance {
        ranks: Vec<FireballRank>,
    },
    /// Power in Light: the target's dynamic damage taken modifier multiplies `spells` while any
    /// dot in `dot_spells` burns it: Holy Fire, and since the 16 October 2026 beta notes Shadow
    /// Word: Pain.
    PowerInLight {
        multiplier: f64,
        spells: Vec<usize>,
        dot_spells: Vec<usize>,
    },
    /// Searing Light: Holy Fire ticks may grant a free Holy Nova.
    SearingLight {
        trigger_aura: String,
        aura: String,
        callbacks: Vec<String>,
        outcome: Vec<String>,
        trigger_immediately: bool,
        proc_chance: f64,
        trigger_spells: Vec<usize>,
        cost_percent_add: f64,
        cost_spells: Vec<usize>,
        cancel_spells: Vec<usize>,
    },
    /// A registered pet nothing summons: Go resets and dismisses it each fight, logging its
    /// stats, and reports its zero metrics.
    InertPet {
        name: String,
        label: String,
        unit_index: i32,
        metrics_actions: Vec<MetricsAction>,
        auras: Vec<ActionId>,
        /// The auras with an action that every reset activates for the fight, in
        /// registration order.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        permanent_auras: Vec<ActionId>,
        /// The permanent auras with exclusive effects, each effect alone in its category on
        /// the pet, so that it holds the category while the aura is up: a party aura that
        /// reaches the pet, such as Mana Spring Totem (Go patch 131).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        exclusive_effects: Vec<InertPetExclusiveEffects>,
        dismissed_log: String,
        reason: String,
        /// Whether each reset logs its dismissal, as when its agent's Reset disables it.
        #[serde(default = "yes", skip_serializing_if = "is_true")]
        dismissed_at_reset: bool,
        /// Whether it has a mana bar, which gives it Go's time to out of mana.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        mana_bar: bool,
        /// Under Forever's rules, the maximum mana of a pet with a mana bar, which Go's reset
        /// fills and nothing spends: what a rotation's `currentMana` of the pet reads.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_mana: Option<f64>,
    },
    /// The Orc racial Shatter Curse: a survival cooldown whose aura multiplies the player's
    /// damage taken of the named schools. Go autocasts it only below a nonzero defensive
    /// health threshold; configured timings still cast it.
    ShatterCurse {
        spell_id: i32,
        aura: String,
        school_damage_taken_multiplier: f64,
        schools: Vec<String>,
    },
    /// Warrior talents_protection.go Last Stand: a survival cooldown whose aura raises maximum
    /// health by its share of the maximum health at activation, through `stat_auras`, gaining
    /// that much health on its metrics, and on expiry takes it back, leaving at least 1.
    LastStand {
        spell_id: i32,
        aura: String,
        health_share: f64,
        metrics_action_id: ActionId,
    },
    /// Warrior shield_wall.go: a survival cooldown whose aura multiplies damage taken, through
    /// `pseudo_stat_auras`; a tank autocasts it below the health share, in Defensive Stance with
    /// a shield.
    ShieldWall {
        spell_id: i32,
        aura: String,
        autocast: bool,
        health_percent: f64,
        can_block: bool,
    },
    /// Warrior shield_block.go: a cast in Defensive Stance with a shield whose aura adds block
    /// chance, through `stat_auras`, at its maximum stacks; each block the player makes spends a
    /// stack.
    ShieldBlock {
        spell_id: i32,
        aura: String,
        can_block: bool,
    },
    /// The Dwarf racial Stoneform: a survival cooldown whose aura multiplies the player's
    /// damage taken of the named schools, the physical one, which the target's swings read.
    /// Go autocasts it only below a nonzero defensive health threshold; configured timings
    /// still cast it.
    Stoneform {
        spell_id: i32,
        aura: String,
        school_damage_taken_multiplier: f64,
        schools: Vec<String>,
    },
    /// The Windshaper Skyborne racial Skysight: a cast that only a rotation action uses, whose
    /// aura Elemental Blessing multiplies the movement speed while up.
    Skysight {
        spell_id: i32,
        aura: String,
        speed_multiplier: f64,
    },
    /// The High Order Skyborne racial Read Ley Line: a cast that only a rotation action
    /// uses, whose aura Energized multiplies mana regeneration.
    ReadLeyLine {
        spell_id: i32,
        aura: String,
        regen_multiplier: f64,
        /// Forever's 26 October notes: Read Ley Line cannot be cast while an Innervate is up,
        /// and an Innervate that lands removes Energized.
        #[serde(default, skip_serializing_if = "is_false")]
        innervate_replaces: bool,
    },
    /// Master of Elements: Fire and Frost crits refund part of the base cost.
    MasterOfElements {
        trigger_aura: String,
        refund: f64,
        metrics_action_id: ActionId,
    },
    /// Pyroblast, Fireball's shape at its highest rank.
    Pyroblast {
        spell_id: i32,
        tick_base: f64,
        tick_can_crit: bool,
    },
    /// Heating Up: crits stack a Pyroblast cast time cut.
    HeatingUp {
        aura: String,
        trigger_aura: String,
        /// The trigger that adds a stack on every Fire Blast cast (community #808).
        fire_blast_trigger_aura: String,
        cast_time_per_stack: f64,
    },
    /// Combustion's major cooldown and stacking Fire crit buff.
    Combustion {
        spell_id: i32,
        aura: String,
        crit_per_stack: f64,
        max_crits: i32,
    },
    /// Fire Blast's instant hit.
    FireBlast {},
    /// Arcane Explosion's rolled hit on each target, cast at the one target.
    ArcaneExplosion {},
    /// Cone of Cold's rolled binary hit on each target.
    ConeOfCold {},
    /// Frost Nova's rolled binary hit on each target.
    FrostNova {},
    /// Blast Wave's rolled binary hit on each target, when talented.
    BlastWave {},
    /// Every Flamestrike rank: a rolled hit, then an area dot of hit-checked ticks.
    Flamestrike {
        ranks: Vec<FlamestrikeRank>,
    },
    /// Blizzard's channel: each period casts the triggered tick spell, a hit-checked fixed
    /// amount; with Improved Blizzard every landed tick casts the chill spell.
    Blizzard {
        spell_id: i32,
        tick_spell_id: i32,
        tick_base: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        improved_blizzard_spell_id: Option<i32>,
    },
    /// Every Fireball rank: a hit after travel, then a dot that snapshots.
    Fireball {
        ranks: Vec<FireballRank>,
    },
    /// Every Frostfire Bolt rank: Fireball's shape with the Frostfire school.
    FrostfireBolt {
        ranks: Vec<FireballRank>,
    },
    /// Every Scorch rank, with Improved Scorch when talented.
    Scorch {
        #[serde(default)]
        improved_scorch: Option<ImprovedScorch>,
    },
    /// Ignite: crits of fire spells feed a fire dot.
    Ignite {
        trigger_aura: String,
        spell_id: i32,
        share: f64,
        num_ticks: i32,
        /// Whether a spell flagged Proc can trigger it.
        can_proc_from_procs: bool,
    },
    /// Arcane Power's major cooldown and aura.
    ArcanePower {
        spell_id: i32,
        aura: String,
        damage: f64,
        cost_percent_add: f64,
    },
    /// Presence of Mind's major cooldown and aura.
    PresenceOfMind {
        spell_id: i32,
        aura: String,
        cast_time_percent: f64,
    },
    /// Arcane Blast and its stacking buff.
    /// Arcane Blast and its stacks (client 70425): each stack's spell damage from Intellect is a
    /// stacking stat aura; the stacks raise Arcane Blast's own cost.
    ArcaneBlast {
        spell_id: i32,
        aura: String,
        /// The share of Intellect each stack gives as spell damage, for the log line.
        intellect_per_stack: f64,
        /// The Intellect left once an iteration's end has deactivated the auras registered
        /// before the stacks', which their last log line reads.
        done_intellect: f64,
        cost_per_stack: f64,
    },
    /// Arcane Wrath (client 70425): the hit adds `mana_share` of maximum mana as spell damage at
    /// its `coefficient`.
    ArcaneWrath {
        spell_id: i32,
        coefficient: f64,
        mana_share: f64,
    },
    /// Fire Power (client 70425): Fire spells, Ignite aside, deal `per_effect` more per Fire
    /// effect of the mage on the target, up to `max_effects`.
    FirePower {
        per_effect: f64,
        max_effects: i32,
    },
    IceLance {
        spell_id: i32,
        frozen_multiplier: f64,
    },
    ArcaneMissiles {
        ranks: Vec<MissileRank>,
    },
    ColdSnap {
        spell_id: i32,
    },
    /// Ice Barrier (the fork's patch 187, client hotfix push 112753): the shield, which absorbs
    /// `warding x (absorb_base + 0.1 x Frost spell power)` as cast and raises Frost damage while it
    /// holds, and Frigid Power, which raises it again for its full duration.
    IceBarrier {
        spell_id: i32,
        aura: String,
        frigid_power_aura: String,
        absorb_base: f64,
        warding: f64,
        /// The shield's SPELLMOD_DAMAGE and SPELLMOD_DOT shares.
        hit: f64,
        dot: f64,
        /// Frigid Power's.
        frigid_hit: f64,
        frigid_dot: f64,
    },
    Evocation {
        spell_id: i32,
        regen_aura: String,
        channel_aura: String,
        regen_multiplier: f64,
    },
    ManaGems {
        gems: Vec<ManaGem>,
        regen_window_seconds: f64,
    },
    MageArmor {
        aura: String,
    },
    ArcaneConcentration {
        talent_rank: i32,
        proc_chance: f64,
        icd_ns: i64,
        trigger_aura: String,
        aura: String,
        aura_duration_ns: i64,
    },
    MissileBarrage {
        trigger_aura: String,
        aura: String,
        arcane_blast_chance: f64,
        /// An Arcane Missiles missile's chance (client 70425).
        missile_chance: f64,
        bolt_chance: f64,
        rng_label: String,
        cost_percent_add: f64,
        tick_length_delta_ns: i64,
    },
    FingersOfFrost {
        talent_rank: i32,
        shatter_rank: i32,
        proc_chance: f64,
        max_stacks: i32,
        shatter_crit: f64,
        duration_ns: i64,
        trigger_aura: String,
        aura: String,
    },
    WintersChill {
        talent_rank: i32,
        proc_chance: f64,
        max_stacks: i32,
        /// Each stack's Cone of Cold damage (client 70425).
        damage_per_stack: f64,
        /// Client 1.60.1.70630: each stack also adds this share of the mage's spell damage to
        /// the next Cone of Cold (11180 effect 3, 7%).
        #[serde(default, skip_serializing_if = "is_zero_f64")]
        spell_power_per_stack: f64,
        /// Client 1.60.1.70630: only Frostbolt, Frost Blast and a Fingers of Frost empowered Ice
        /// Lance add a stack, and the next Cone of Cold uses them all up.
        #[serde(default, skip_serializing_if = "is_false")]
        cone_of_cold_consumes: bool,
        duration_ns: i64,
        trigger_aura: String,
        aura: String,
    },
    JudgementOfWisdom {
        aura: String,
        proc_chance: f64,
        proc_mask: Vec<String>,
        mana: f64,
        metrics_action_id: ActionId,
        delay_ns: i64,
    },
    /// Go buffs/paladin.go `AttachJudgementOfLightHeal`: a landed melee hit on the judged target
    /// that is no proc runs the handler a spell batch window later, which rolls `proc_chance`
    /// under `rng_label` and heals the attacker for `heal`. The health metrics belong to the
    /// target, as Go registers them on the aura's unit.
    JudgementOfLight {
        aura: String,
        proc_chance: f64,
        rng_label: String,
        heal: f64,
        metrics_action_id: ActionId,
        delay_ns: i64,
    },
    PotionMana {
        item_id: i32,
        rng_label: String,
        gains: Vec<ManaGain>,
        stone_multiplier: f64,
        regen_window_seconds: f64,
    },
    ConjuredMana {
        item_id: i32,
        rng_label: String,
        gains: Vec<ManaGain>,
        selected: bool,
        regen_window_seconds: f64,
    },
    /// `whole` is the gain the manager waits to fit; a `periodic` gain rolls `average` on each
    /// tick of the use spell's self hot.
    EnergizeOnUse {
        item_id: i32,
        spell_id: i32,
        average: f64,
        variance: f64,
        whole: f64,
        #[serde(default, skip_serializing_if = "is_false")]
        periodic: bool,
        /// The bar the gain fills; none for mana.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        power: Option<EnergizedPower>,
    },
    /// A stat proc common/shared/shared_utils.go applySpellDataProc builds from client rows: a
    /// listener on the spells it hears through `callbacks`, at a chance behind the trigger
    /// aura's cooldown, that activates a stat aura a spell batch window later. A temporary
    /// stats aura logs its gain and expiry; the parsed aura of spell_data_aura.go
    /// registerSpellDataAuraProc logs neither.
    SpellDataStatProc {
        trigger_aura: String,
        aura: String,
        trigger_spells: Vec<usize>,
        callbacks: Vec<String>,
        /// A "when struck" proc, as The Lion Horn of Stormwind's: it hears the target's melee
        /// swings on a tank, its one callback `on_spell_hit_taken`.
        #[serde(default, skip_serializing_if = "is_false")]
        struck: bool,
        landed_only: bool,
        require_damage: bool,
        proc_chance: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gain_log: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expire_log: Option<String>,
    },
    /// An item use that activates an aura whose spell mods add flat amounts to spell costs, as
    /// common/classic/items_trinkets.go Burst of Knowledge.
    SpellCostAuraOnUse {
        item_id: i32,
        aura: String,
        cost_changes: Vec<CostChange>,
    },
    /// MythicSim's healer sims under Forever's rules: an item use that activates an aura of
    /// spell mods its client row states, as Hazza'rah's Charm of Healing's cast time and cost.
    /// Each change is one spell's, by spellbook position, as a reset simulation with the aura
    /// active reads it.
    SpellModAuraOnUse {
        item_id: i32,
        aura: String,
        changes: Vec<SpellModChange>,
    },
    /// MythicSim's healer sims under Forever's rules: a listener on the player's casts that
    /// restores mana a spell batch window later, as Sudden Insight, Idol of Longevity or The
    /// Earthfury's Mana Surge. Each spell it hears, by spellbook position, with the mana it
    /// restores.
    ManaOnCast {
        trigger_aura: String,
        rng_label: String,
        proc_chance: f64,
        spells: Vec<SpellMana>,
        metrics_action_id: ActionId,
        /// Only a heal that crits restores the mana, as Dreamwalker Raiment's Healing Touch.
        #[serde(default, skip_serializing_if = "is_false")]
        crit_only: bool,
    },
    /// MythicSim's healer sims under Forever's rules: an item use whose window aura moves the
    /// stacks of a stacking stats aura while it is up, as Talisman of Ascendance (a stack on each
    /// hit or heal) or Zandalarian Hero Charm (all its stacks at once, one fewer each cast). The
    /// window going up brings the stats aura up, at its most stacks when they `decay`, and its
    /// end takes the stats aura down. Each heard event of a `trigger_spells` spell rolls
    /// `proc_chance`, and a spell batch window later the stacks move. A trigger on a proc rate,
    /// as Badge of the Swarmguard's, rolls its manager's chance for each spell in `chances` in
    /// place of `proc_chance`, under the trigger's `rng_label`, and with `require_damage` a hit
    /// that dealt no damage is not heard.
    StackingCdOnUse {
        item_id: i32,
        stat_aura: String,
        window_aura: String,
        decay: bool,
        trigger_spells: Vec<usize>,
        callbacks: Vec<String>,
        landed_only: bool,
        proc_chance: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chances: Option<Vec<SpellChance>>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        require_damage: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rng_label: Option<String>,
    },
    /// MythicSim's healer sims under Forever's rules: an item use whose aura, while up, shields
    /// the player each direct heal lands on for `share` of the healing it did, as Scarab
    /// Brooch's Persistent Shield. The shields count on the item's spell.
    ShieldOnHeal {
        item_id: i32,
        aura: String,
        share: f64,
        /// Each heal's chance; one when unset.
        #[serde(default = "one", skip_serializing_if = "is_one")]
        chance: f64,
        /// Heals the healer instead of shielding (Garments of the Oracle's 10% of a heal on
        /// another player).
        #[serde(default, skip_serializing_if = "is_false")]
        heal: bool,
        /// The spell, by client ID, the amounts count on when `item_id` is 0 (a set bonus).
        #[serde(default, skip_serializing_if = "is_zero")]
        spell_id: i32,
    },
    /// MythicSim's healer sims under Forever's rules: a spell whose aura makes the next cast of
    /// the spells it changes instant and is spent by that cast, as Nature's Swiftness. Each
    /// change is one spell's mod while the aura is up, by spellbook position.
    InstantCastAura {
        spell_id: i32,
        aura: String,
        changes: Vec<SpellModChange>,
    },
    InertListener {
        unit: String,
        aura: String,
        reason: String,
    },
    /// Every Shadow Bolt rank: a hit after travel.
    ShadowBolt {},
    /// Immolate's hit and the snapshot dot on its related spell.
    Immolate {
        spell_id: i32,
        tick_base: f64,
        tick_can_crit: bool,
    },
    /// Corruption's snapshot dot.
    Corruption {
        spell_id: i32,
        tick_base: f64,
        tick_can_crit: bool,
    },
    /// Siphon Life's snapshot dot, which heals the warlock for each tick's damage.
    SiphonLife {
        spell_id: i32,
        tick_base: f64,
        tick_can_crit: bool,
        self_healing_multiplier: f64,
    },
    /// Drain Life's channeled snapshot dot: each tick is scaled by Soul Siphon and heals the
    /// warlock.
    DrainLife {
        spell_id: i32,
        tick_base: f64,
        tick_can_crit: bool,
        soul_siphon: f64,
        self_healing_multiplier: f64,
    },
    /// Wrack: a channel scaled by Soul Siphon, and while it runs a bonus on the ticks of
    /// `dot_spells`.
    Wrack {
        spell_id: i32,
        tick_base: f64,
        tick_can_crit: bool,
        soul_siphon: f64,
        dot_bonus: f64,
        dot_spells: Vec<usize>,
    },
    /// Rain of Fire: the channel is an area dot on the warlock whose every tick casts the
    /// triggered tick spell, a fixed amount rolled to hit and to crit on each target. The cast
    /// itself rolls a hit on every target and deals no damage.
    RainOfFire {
        spell_id: i32,
        tick_spell_id: i32,
        tick_base: f64,
    },
    /// Hellfire: the channel is an area dot on the warlock whose every tick rolls a fixed
    /// amount on each target, then burns the warlock for it. `tick_can_crit` is false when the
    /// client row says the area hit cannot crit.
    Hellfire {
        spell_id: i32,
        tick_base: f64,
        tick_can_crit: bool,
    },
    /// Bane of Havoc: the cast takes the bane slot with the target aura; the copy listener
    /// copies `share` of the warlock's damage to other targets onto the baned one, through the
    /// spell of the same ID with tag 1.
    BaneOfHavoc {
        spell_id: i32,
        aura: String,
        copy_aura: String,
        share: f64,
    },
    /// Death Coil: a fixed base, landing after travel, whose damage heals the warlock through
    /// its tagged healing spell with the warlock's healing modifiers.
    DeathCoil {
        base_damage: f64,
        healing_dealt_multiplier: f64,
        healing_taken_multiplier: f64,
        table_healing_dealt_multiplier: f64,
        healing_power: f64,
    },
    /// Curse of Recklessness: the debuff on the target and the net changes Go measures when it
    /// activates, through its per-stat exclusive category beside the raid's permanent debuffs.
    CurseOfRecklessness {
        spell_id: i32,
        aura: String,
        armor_delta: f64,
        #[serde(default, skip_serializing_if = "is_zero_f64")]
        attack_power_delta: f64,
    },
    /// Incinerate's damage bonus on a target burning with Immolate.
    Incinerate {
        immolate_bonus: f64,
    },
    /// Bane of Doom's one-tick snapshot dot on the bane slot.
    BaneOfDoom {
        spell_id: i32,
        tick_base: f64,
        tick_can_crit: bool,
    },
    /// Bane of Agony's ramping snapshot dot: the snapshot pays `ramp_share` of the tick and
    /// every `ramp_every_ticks` ticks adds that share back.
    BaneOfAgony {
        spell_id: i32,
        tick_base: f64,
        tick_can_crit: bool,
        ramp_share: f64,
        ramp_every_ticks: i32,
        #[serde(default)]
        amplify: Option<f64>,
    },
    /// Amplify Curse's major cooldown and aura, spent by Bane of Agony.
    AmplifyCurse {
        spell_id: i32,
        aura: String,
    },
    /// Curse of the Elements' debuff on the target: flat resistance changes and school
    /// damage taken multipliers while it is active.
    CurseOfTheElements {
        spell_id: i32,
        aura: String,
        resistance_delta: BTreeMap<String, f64>,
        school_damage_taken_multiplier: BTreeMap<String, f64>,
        /// A stronger or equal permanent member of the curse's exclusive category, such as the
        /// raid's own Curse of the Elements, blocks every activation, which Go still counts as a
        /// proc.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        blocked: bool,
    },
    /// Life Tap: (base + Spirit) times the multiplier, as health spent and mana gained.
    LifeTap {
        spell_id: i32,
        base_amount: f64,
        mana_multiplier: f64,
        /// Demonic Energies: the share of the restore the summoned demon gains.
        #[serde(default, skip_serializing_if = "is_zero_f64")]
        pet_mana_share: f64,
        /// The client flags the cast No Threat: neither the mana nor the demon's share adds
        /// threat.
        no_threat: bool,
    },
    /// Conflagrate's hit, which consumes Immolate unless Shadow and Flame spares it.
    Conflagrate {
        spell_id: i32,
        keep_immolate_chance: f64,
        rng_label: String,
    },
    /// Shadowburn's instant binary hit.
    Shadowburn {},
    /// Nightfall: periodic damage of its spells may grant Shadow Trance, which makes Shadow
    /// Bolt instant until an instant Shadow Bolt completes.
    Nightfall {
        trigger_aura: String,
        aura: String,
        aura_spell_id: i32,
        proc_chance: f64,
        rng_label: String,
        /// Spellbook positions of the spells whose periodic damage rolls the chance.
        trigger_spells: Vec<usize>,
        /// Spellbook positions of the spells whose instant cast consumes Shadow Trance.
        consume_spells: Vec<usize>,
        /// Spellbook positions of the spells Shadow Trance's cast time modifier changes.
        modded_spells: Vec<usize>,
        cast_time_percent: f64,
    },
    /// Fel Energy, the Voidwalker's sacrifice: its permanent aura restores a share of maximum
    /// mana every period.
    FelEnergy {
        aura: String,
        spell_id: i32,
        mana_fraction: f64,
        period_ns: i64,
    },
    /// The Imp's Firebolt: a damage roll between Go's literal bounds at level 60.
    Firebolt {
        min_damage: f64,
        max_damage: f64,
        /// The row's average and variance, which Go `Effect.Roll` reads.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        average: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        variance: Option<f64>,
    },
    /// Decimation: a landed hit of its spells inside the execute phase grants an aura whose
    /// modifiers raise their damage and cut Soul Fire's cast time.
    Decimation {
        trigger_aura: String,
        aura: String,
        /// Go `IsExecutePhase<N>`'s threshold.
        execute_phase: i32,
        /// Spellbook positions of the spells whose landed hits trigger it.
        trigger_spells: Vec<usize>,
        /// Spellbook positions and value of the aura's damage done modifier.
        damage_spells: Vec<usize>,
        damage_done_flat: f64,
        /// Spellbook positions and value of the aura's cast time modifier.
        cast_spells: Vec<usize>,
        cast_time_percent: f64,
    },
    /// Demonic Brand: a landed Searing Pain brands its target with charges, and each landed
    /// direct hit of the summoned demon spends one for an extra hit. Without a summoned demon
    /// the trigger does nothing.
    DemonicBrand {
        trigger_aura: String,
        /// The brand on the target.
        target_aura: String,
        charges: i32,
        trigger_spells: Vec<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pet: Option<String>,
        /// The demon's copy of the brand's stacks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        marker_aura: Option<String>,
        /// The demon's permanent aura that spends charges.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        consumer_aura: Option<String>,
        /// The extra hit's position in the demon's spellbook.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        brand_spell: Option<usize>,
        #[serde(default, skip_serializing_if = "is_zero_f64")]
        min_damage: f64,
        #[serde(default, skip_serializing_if = "is_zero_f64")]
        max_damage: f64,
        /// The share of the warlock's spell power and school power the hit adds.
        #[serde(default, skip_serializing_if = "is_zero_f64")]
        spell_power_coefficient: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        school_power_stat: Option<String>,
    },
    /// The Black Book under Forever's rules: its use, which waits for a demon out, activates
    /// the demon's aura, which multiplies the demon's damage done in every school while up.
    BlackBook {
        item_id: i32,
        aura: String,
        damage_multiplier: f64,
    },
    /// The summoned demon's AI: the first ability it can cast while its mana stays above
    /// `min_mana`, otherwise a wait.
    WarlockPet {
        pet: String,
        min_mana: f64,
        /// Positions in the pet's spellbook.
        autocast_spells: Vec<usize>,
        wait_ns: i64,
    },
    /// The Succubus's Lash of Pain: a fixed base, a Go literal.
    LashOfPain {
        base_damage: f64,
    },
    /// Searing Pain's hit.
    SearingPain {},
    /// Soul Fire's hit after travel.
    SoulFire {},
    /// Improved Shadow Bolt: Shadow Bolt crits leave a target debuff that multiplies the
    /// warlock's shadow damage after the outcome.
    ImprovedShadowBolt {
        trigger_aura: String,
        aura: String,
        spell_id: i32,
        multiplier: f64,
        /// Spellbook positions of the spells the trigger listens to.
        trigger_spells: Vec<usize>,
    },
    /// Shadow and Flame: Conflagrate and Shadowburn hits multiply the warlock's shadow or
    /// fire damage dealt for a while.
    ShadowAndFlame {
        trigger_aura: String,
        shadow_aura: String,
        fire_aura: String,
        shadow_spell_id: i32,
        fire_spell_id: i32,
        multiplier: f64,
        /// Spellbook positions of the spells the trigger listens to.
        trigger_spells: Vec<usize>,
        /// The trigger spells that raise shadow damage; the others raise fire damage.
        shadow_spells: Vec<usize>,
    },
    /// Every Lightning Bolt rank: an overload may roll when the bolt lands.
    LightningBolt {
        overload_chance: f64,
        overload_tag: i32,
        rng_label: String,
    },
    /// Every Chain Lightning rank: a third of the overload chance per hit and a bounce
    /// reduction on later targets.
    ChainLightning {
        overload_chance: f64,
        overload_tag: i32,
        rng_label: String,
        bounce_reduction: f64,
        bounce_bonus: f64,
    },
    /// Flame Shock's hit and the dot it applies when it lands.
    FlameShock {
        spell_id: i32,
        tick_base: f64,
        tick_can_crit: bool,
    },
    /// Lava Burst, stronger against a target burning with Flame Shock.
    LavaBurst {
        spell_id: i32,
        flame_shock_bonus: f64,
    },
    /// Fire Nova: one hit on each target from a fixed base.
    FireNova {
        spell_id: i32,
        base_damage: f64,
    },
    /// Searing Totem: a target dot whose ticks cast the totem's attack.
    SearingTotem {
        spell_id: i32,
        attack_spell_id: i32,
        attack_damage: f64,
        magma_totem_aura: String,
        flametongue_totem_aura: String,
        /// The fire totem's lifetime, for `totemRemainingTime`.
        duration_ns: i64,
    },
    /// Earth Shock: a binary hit from the highest rank's damage roll.
    EarthShock {
        spell_id: i32,
    },
    /// Strength of Earth Totem: the earth totem's aura, whose Strength is a class stat aura.
    StrengthOfEarthTotem {
        spell_id: i32,
        aura: String,
        duration_ns: i64,
    },
    /// Stormstrike: a melee strike whose target debuff raises this shaman's lightning damage
    /// until its charges are spent.
    Stormstrike {
        spell_id: i32,
        aura: String,
        damage_multiplier: f64,
        /// Go `HasMHWeapon` and `HasOHWeapon`: its cast condition, and whether it strikes.
        has_main_hand: bool,
        has_off_hand: bool,
    },
    /// Elemental Devastation: spell crits raise melee crit for a while.
    ElementalDevastation {
        trigger_aura: String,
        aura: String,
        melee_crit: f64,
        /// Whether a crit from a spell flagged Proc can trigger it.
        can_proc_from_procs: bool,
        /// The class spells whose crits never trigger it (community #789).
        excluded_class_spells: Vec<String>,
    },
    /// Flurry: melee crits grant charges of melee speed that white hits spend.
    Flurry {
        trigger_aura: String,
        aura: String,
        melee_speed_multiplier: f64,
        charge_icd_ns: i64,
        max_stacks: i32,
        /// Whether a hit from a spell flagged Proc can trigger it.
        can_proc_from_procs: bool,
    },
    /// Improved Stormstrike: Stormstrike may raise casting spirit regeneration; its cooldown
    /// reset hears only hits the player takes.
    ImprovedStormstrike {
        trigger_aura: String,
        aura: String,
        reset_aura: String,
        proc_chance: f64,
        spirit_regen_rate_casting: f64,
    },
    /// Maelstrom Weapon: landed melee hits stack a Lightning Bolt cast time and cost cut.
    MaelstromWeapon {
        trigger_aura: String,
        aura: String,
        per_stack: f64,
        max_stacks: i32,
        /// The proc manager's chance for each spell the trigger hears.
        chances: Vec<SpellChance>,
    },
    /// Magma Totem: an area dot on the shaman whose pulses roll hit and crit on each target.
    MagmaTotem {
        spell_id: i32,
        pulse_damage: f64,
        duration_ns: i64,
    },
    /// Lightning Shield: the cast puts up every charge; without a shield proc rate the orbs
    /// never fire.
    LightningShield {
        spell_id: i32,
        aura: String,
        charges: i32,
    },
    /// Water Shield (client 1.60.1.70532): the cast puts up every globe; a critical hit of a
    /// trigger spell, damage or heal, spends one and restores a share of maximum mana, one globe
    /// per internal cooldown.
    WaterShield {
        spell_id: i32,
        aura: String,
        globes: i32,
        icd_ns: i64,
        mana_percent: f64,
        /// The globe spell whose mana row the gains land in (408511).
        mana_spell_id: i32,
        /// Spellbook positions whose critical hits spend a globe.
        trigger_spells: Vec<usize>,
    },
    /// Grace of Air Totem: the air totem's aura, whose Agility is a class stat aura.
    GraceOfAirTotem {
        spell_id: i32,
        aura: String,
        duration_ns: i64,
    },
    /// The shaman's own Windfury Totem: the totem's aura refreshes a tracking aura and a dummy
    /// aura every period, the dummy's exclusive effect turns the trigger on, and the trigger
    /// grants the proc aura's charges and an extra main hand attack. Spellbook positions name
    /// the spells; `contested` says the party's Windfury Totem or a main hand imbue would contest
    /// it.
    WindfuryTotemSelf {
        spell_id: i32,
        totem_aura: String,
        duration_ns: i64,
        period_ns: i64,
        tracking_aura: String,
        dummy_aura: String,
        trigger_aura: String,
        trigger_spells: Vec<usize>,
        trigger_proc_chance: f64,
        proc_aura: String,
        spend_spells: Vec<usize>,
        extra_spell: usize,
        white_spells: Vec<usize>,
        proc_gain_log: String,
        proc_expire_log: String,
        contested: bool,
    },
    /// Mana Spring Totem: the water totem's aura, whose MP5 is a class stat aura.
    ManaSpringTotem {
        spell_id: i32,
        aura: String,
        duration_ns: i64,
    },
    /// Mana Tide Totem (MythicSim's Restoration shaman sim, Forever's rules): the water totem's
    /// aura, whose MP5 is a class stat aura. It takes the water totem slot of the Healing Stream
    /// Totems `healing_stream_totems` names: its cast ends their heals over time, and a cast of
    /// one of them ends it.
    ManaTideTotem {
        spell_id: i32,
        aura: String,
        duration_ns: i64,
        healing_stream_totems: Vec<i32>,
    },
    /// Flametongue Totem: the totem's aura turns on a trigger that casts a fire hit off landed
    /// main hand autos, unless a main hand Flametongue Weapon holds the benefit.
    FlametongueTotem {
        spell_id: i32,
        aura: String,
        trigger_aura: String,
        attack_spell: usize,
        attack_deals_damage: bool,
        attack_damage: f64,
        trigger_spells: Vec<usize>,
        trigger_outcome: Vec<String>,
        disabled_by_weapon: bool,
        duration_ns: i64,
        /// Whether the party's Flametongue Totem shares the benefit.
        party_totem: bool,
    },
    /// Windfury Weapon: a weapon proc with its own cooldown that strikes twice with the hand
    /// that procced it, as two special weapon hits with the rank's attack power added.
    WindfuryWeapon {
        trigger_aura: String,
        trigger_spells: Vec<usize>,
        chances: Vec<SpellChance>,
        /// Spells of the main hand, whose procs strike with the main hand.
        main_hand_spells: Vec<usize>,
        /// The main hand and off hand strike spells.
        main_hand_attack: usize,
        off_hand_attack: usize,
        /// The attack power each strike adds to the shaman's own.
        attack_power: f64,
        /// Whether a main hand imbue holds the party Windfury Totem's category.
        blocks_windfury_totem: bool,
    },
    /// Enhancement's weapon sync: the main hand swing replacement that moves the off hand swing
    /// before returning the swing. `sync` is "none", "auto" (weapons of equal speed, which
    /// delay), "sync" or "delay".
    WeaponSync {
        sync: String,
        flurry_icd_ns: i64,
    },
    /// Frost Shock: Earth Shock's shape on the Frost school.
    FrostShock {
        spell_id: i32,
    },
    /// Flametongue Weapon: each imbued hand's landed hits cast that hand's imbue hit.
    FlametongueWeapon {
        hands: Vec<FlametongueHand>,
    },
    /// Frostbrand Weapon: a proc manager's chance on landed weapon hits to cast a Frost hit.
    FrostbrandWeapon {
        trigger_aura: String,
        spell_id: i32,
        base_damage: f64,
        chances: Vec<SpellChance>,
    },
    /// Rockbiter Weapon: a permanent attack power aura, already in the prepared stats, that
    /// logs its gain and loss.
    RockbiterWeapon {
        aura: String,
        gain_log: String,
        expire_log: String,
        /// Spirit Weapons' threat multiplier, which the aura multiplies into the player's as it
        /// is gained and divides out as it fades, when it follows the weapon in the hand.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        threat_multiplier: Option<f64>,
    },
    /// Rockbiter Weapon Proc (patch 181): a landed melee hit of a hand whose weapon carries
    /// Rockbiter casts the spell, whose flat threat is `threat_per_swing_second` times the main
    /// hand weapon's swing speed. The hands and the speed are those of the weapons worn.
    RockbiterThreat {
        aura: String,
        spell_id: i32,
        main_hand: bool,
        off_hand: bool,
        threat_per_swing_second: f64,
        main_hand_swing_speed: f64,
    },
    /// Rage of the Farseer: a major cooldown whose aura multiplies melee speed.
    RageOfTheFarseer {
        spell_id: i32,
        aura: String,
        melee_speed_multiplier: f64,
    },
    /// Elemental Mastery (client 70425): a major cooldown whose aura makes Lightning Bolt and
    /// Lava Burst cast faster and cancels Chain Lightning's cooldown.
    ElementalMastery {
        spell_id: i32,
        aura: String,
        cast_time_delta_ns: i64,
        chain_lightning_cooldown_ns: i64,
    },
    /// Elemental Focus: a completed elemental cast may grant Clearcasting.
    ElementalFocus {
        trigger_aura: String,
        aura: String,
        proc_chance: f64,
        cost_percent_add: f64,
        max_stacks: i32,
    },
    /// Go `AttachMultiplicativePseudoStatBuff` on the player's damage taken or threat
    /// multiplier, for any player aura: the gain multiplies, the expiry divides, and an aura up
    /// from the reset is already in the prepared values.
    PseudoStatAuras {
        auras: Vec<PseudoStatAura>,
    },
    /// exclusive_effect.go: a single aura category on a unit, with each member aura's bid and
    /// spell, in Go's registration order.
    ExclusiveCategory {
        unit: String,
        category: String,
        members: Vec<ExclusiveMember>,
        /// For the target's major armor category: the target's armor at each stack count of
        /// the active member, the stacking members being the same debuff. A member that does
        /// not stack takes its bid off the armor with no stacks.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        armor_by_stacks: Vec<f64>,
    },
    /// core/rage.go: a rage bar, with the rage each landed white hit gives.
    RageBar {
        aura: String,
        max_rage: f64,
        starting_rage: f64,
        main_hand_rage: f64,
        off_hand_rage: f64,
        /// The rage a critical hit adds to its swing's rage: the swing's rage before a
        /// two-hander's factor times `CritRageMultiplier` less one.
        main_hand_crit_rage: f64,
        off_hand_crit_rage: f64,
        threat_per_rage: f64,
        /// Patch 182: the threat of a gain is `threat_per_rage` times the unit's threat multiplier at
        /// the gain, split between the targets in combat. Off, `threat_per_rage` on each target.
        threat_multiplied: bool,
        /// Forever's rules: a critical white hit gives `CritRageMultiplier` times its swing's whole
        /// rage, a two-hander's factor included, in place of the crit rage values. Without it, as
        /// in Go, a two-hander's crit adds the rage of the swing before that factor.
        #[serde(default, skip_serializing_if = "is_false")]
        crit_multiplies_swing: bool,
    },
    /// Go `AttachMultiplicativePseudoStatBuff` on the player's damage taken multiplier, for
    /// auras that are not up from the reset: the gain multiplies, the expiry divides.
    PlayerDamageTaken {
        auras: Vec<AuraMultiplier>,
    },
    /// An item proc trigger whose handler grants extra main hand attacks at once: Ironfoe's
    /// Fury of Forgewright and the Hand of Justice.
    ExtraAttackProc {
        trigger_aura: String,
        proc_chance: f64,
        attacks: i32,
        /// A weapon proc, as Flurry Axe's: each spell its proc manager hears with the chance it
        /// rolls for the landed hit, in place of `proc_chance`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chances: Option<Vec<SpellChance>>,
        /// The spell the weapon proc casts, whose effect grants the attacks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spell: Option<usize>,
    },
    /// A potion that restores rage or mana and may carry a temporary stat aura.
    PotionResource {
        item_id: i32,
        rng_label: String,
        gains: Vec<ResourceGain>,
        stone_multiplier: f64,
        #[serde(default)]
        aura: Option<String>,
        #[serde(default)]
        gain_log: Option<String>,
        #[serde(default)]
        expire_log: Option<String>,
    },
    /// common/forever/revelation.go Enchant Weapon - Revelation: the hit and heal trigger hears
    /// `hit_spells` and the tick trigger `tick_spells`; on a landed one that does not crit and
    /// deals something each rolls `chance_at_no_crit` times one less the player's spell crit
    /// under its own label, and the batch window later activates the buff. The buff adds the crit
    /// to `affected_spells`, the next cast of one of `consuming_spells` spends it and the end of
    /// a `channel_spells` dot ends it.
    Revelation {
        trigger_aura: String,
        ticks_trigger_aura: String,
        aura: String,
        hit_spells: Vec<usize>,
        tick_spells: Vec<usize>,
        affected_spells: Vec<usize>,
        consuming_spells: Vec<usize>,
        channel_spells: Vec<usize>,
        bonus_crit_percent: f64,
        chance_at_no_crit: f64,
        delay_ns: i64,
    },
    /// consumes_forever_potions.go Dragonfire Potion: the cast casts `spell`, a magic hit and
    /// crit of the literal on every target, and on each it lands on the spell's dot, which ticks
    /// its literal.
    DragonfirePotion {
        item_id: i32,
        spell: usize,
        direct_damage: f64,
        tick_damage: f64,
    },
    /// consumes_forever_potions.go Caustic Smog Potion: the cast casts `spell`, a magic hit roll
    /// on every target, and on each it lands on the spell's dot, whose ticks roll the literal
    /// within the variance under the label.
    CausticSmogPotion {
        item_id: i32,
        spell: usize,
        tick_damage: f64,
        tick_variance: f64,
        rng_label: String,
    },
    /// consumes_forever_potions.go Potion of Elemental Purging: the cast activates the aura, which
    /// adds the spell damage against targets of the mob type while it holds.
    ElementalPurgingPotion {
        item_id: i32,
        aura: String,
        spell_damage: f64,
        mob_type: String,
    },
    /// Warrior stances.go: the starting stance and each stance's cast and aura.
    WarriorStances {
        default_stance: String,
        stances: Vec<WarriorStance>,
        max_retained_rage: f64,
    },
    /// Bloodthirst: attack power share plus a client base, on the special hit table.
    Bloodthirst {
        spell_id: i32,
        attack_power_share: f64,
        base_damage: f64,
    },
    /// Whirlwind: a normalized main hand strike, and the off hand's with an off hand weapon,
    /// each hitting up to `max_targets` targets from the cast target on.
    Whirlwind {
        spell_id: i32,
        off_hand: bool,
        max_targets: i32,
    },
    /// Execute: a base plus damage for each extra rage it spends.
    Execute {
        spell_id: i32,
        base_damage: f64,
        damage_per_rage: f64,
    },
    /// Hamstring: a fixed base on the special hit table.
    Hamstring {
        spell_id: i32,
        base_damage: f64,
    },
    /// Pummel (Forever's rules): a fixed client base on the special hit table, refunded on a
    /// miss, in Berserker Stance.
    Pummel {
        spell_id: i32,
        base_damage: f64,
    },
    /// Shield Bash (Forever's rules): a fixed client base on the special hit table, refunded on
    /// a miss, with a shield in Battle or Defensive Stance.
    ShieldBash {
        spell_id: i32,
        base_damage: f64,
        can_block: bool,
    },
    /// Mocking Blow (Forever's rules): a fixed client base on the special hit table, refunded on
    /// a miss, in Battle Stance.
    MockingBlow {
        spell_id: i32,
        base_damage: f64,
    },
    /// Disarm (Forever's rules): a special hit roll without damage that puts up the target's
    /// aura, refunded on a miss, in Defensive Stance. The aura only tracks its uptime.
    WarriorDisarm {
        spell_id: i32,
        aura: String,
    },
    /// Victory Rush (Forever's rules): castable while Victorious is up, which nothing in the
    /// sim brings up, since no target dies.
    VictoryRush {
        spell_id: i32,
        aura: String,
    },
    /// Intercept (Forever's rules): a cast at range in Berserker Stance that puts up its dash
    /// aura for the spell's cooldown and runs the warrior to `overshoot` yards inside the
    /// minimum range. The aura multiplies the movement speed, ends with the movement, and at its
    /// end the stun's damage lands on the target with an outcome that always hits.
    WarriorIntercept {
        spell_id: i32,
        aura: String,
        speed_multiplier: f64,
        overshoot: f64,
        min_range: f64,
        damage: f64,
    },
    /// Bloodrage: instant and periodic rage for a share of base health.
    Bloodrage {
        spell_id: i32,
        instant_rage: f64,
        rage_per_tick: f64,
        ticks: i32,
        period_ns: i64,
        health_cost: f64,
        rage_threshold: f64,
    },
    /// Berserker Rage: rage from Improved Berserker Rage and an aura.
    BerserkerRage {
        spell_id: i32,
        aura: String,
        rage_gain: f64,
    },
    /// Death Wish: physical damage dealt multiplied while its aura lasts.
    DeathWish {
        spell_id: i32,
        aura: String,
        physical_multiplier: f64,
        wait_ns: i64,
    },
    /// Recklessness: an aura whose crit is a temporary stat change.
    Recklessness {
        spell_id: i32,
        aura: String,
    },
    /// The warrior's own Sunder Armor; blocked when another aura holds the armor category
    /// for good.
    SunderArmor {
        spell_id: i32,
        aura: String,
        blocked: bool,
        /// The client's flat threat, which patch 185 sets the spell's to at each cast with
        /// `threat_per_attack_power` of the spell's melee attack power added.
        flat_threat: f64,
        /// 0 keeps the spell's registered flat threat.
        threat_per_attack_power: f64,
    },
    /// Deep Wounds: a physical crit casts a bleed that carries what it still owed.
    DeepWounds {
        spell_id: i32,
        trigger_aura: String,
        share: f64,
        tick_can_crit: bool,
        tick_magic: bool,
    },
    /// Unbridled Wrath: landed white hits may grant rage a spell batch window later.
    UnbridledWrath {
        trigger_aura: String,
        spell_id: i32,
        proc_chance: f64,
        rage: f64,
        delay_ns: i64,
    },
    /// The Warrior's Flurry: a melee crit grants melee speed for a few white swings.
    WarriorFlurry {
        trigger_aura: String,
        aura: String,
        melee_speed_multiplier: f64,
        charges: i32,
        /// Forever's rules: every white swing spends a charge whatever its outcome, and a Heroic
        /// Strike or Cleave spends one only when it misses. Without it, as in Go, a landed white
        /// hit spends one, Heroic Strike and Cleave included.
        #[serde(default, skip_serializing_if = "is_false")]
        every_swing_spends: bool,
    },
    /// Anger Management: rage every period from the reset.
    AngerManagement {
        spell_id: i32,
        rage: f64,
        period_ns: i64,
    },
    /// Heroic Strike and Cleave: queued onto the next main hand swing.
    HeroicStrikeQueue {
        queue_delay_ns: i64,
        strikes: Vec<QueuedStrike>,
        /// Patch 184: a Cleave that lands on several targets splits its flat threat between them.
        cleave_threat_split: bool,
    },
    /// The warrior's own Battle Shout: its aura and the value it bids for the shout category.
    BattleShout {
        spell_id: i32,
        aura: String,
        value: f64,
        refresh_threshold_ns: i64,
    },
    /// Rend: a bleed whose ticks add a share of attack power at each tick.
    Rend {
        spell_id: i32,
        tick_base: f64,
        attack_power_per_tick: f64,
        tick_can_crit: bool,
        tick_magic: bool,
    },
    /// Overpower: a base on normalized main hand damage that cannot be dodged, parried or
    /// blocked, in its window.
    Overpower {
        spell_id: i32,
        base_damage: f64,
    },
    /// Mortal Strike: a base on normalized main hand damage.
    MortalStrike {
        spell_id: i32,
        base_damage: f64,
    },
    /// Spearing Strike: a share of normalized main hand damage, raised against giants and
    /// dragonkin.
    SpearingStrike {
        spell_id: i32,
        weapon_share: f64,
        mob_multiplier: f64,
    },
    /// Slam: a base on main hand weapon damage after its cast.
    Slam {
        spell_id: i32,
        base_damage: f64,
        stops_swings: bool,
    },
    /// Demoralizing Shout: a magic hit roll on every target, each landed one activating that
    /// target's debuff.
    DemoralizingShout {
        spell: usize,
        aura: String,
    },
    /// Challenging Shout: an always hit outcome with no damage on every active target, as Go's
    /// fork casts it, with no taunt.
    ChallengingShout {
        spell: usize,
    },
    /// Taunt: an always hit outcome with no damage on the target, in Defensive Stance. The fork's
    /// taunt changes no target's aim, swing or threat.
    Taunt {
        spell: usize,
    },
    /// Intimidating Shout: an always hit outcome with no damage on the target, as Go's fork casts
    /// it, with no fear and no effect on the other targets.
    IntimidatingShout {
        spell: usize,
    },
    /// Concussion Blow: a special hit outcome with no damage on the target, refunded on a miss.
    /// Its stun has no effect in scope.
    ConcussionBlow {
        spell: usize,
    },
    /// Bloodthrill: main hand hits on a bleeding target may open the Overpower window longer.
    Bloodthrill {
        trigger_aura: String,
        proc_chance: f64,
        window_ns: i64,
        delay_ns: i64,
    },
    /// Weaponmaster with a sword: melee hits of a sword hand may grant an extra main hand
    /// attack.
    WeaponmasterSword {
        trigger_aura: String,
        proc_chance: f64,
        extra_attack_tag: i32,
        sword_hands: Vec<String>,
        /// The class spells that never trigger it: Shield Bash and Shield Slam since the 16
        /// October 2026 beta notes.
        excluded_class_spells: Vec<String>,
    },
    /// Overpower: a dodge opens its window.
    OverpowerWindow {
        trigger_aura: String,
        aura: String,
    },
    /// Revenge: a block, dodge or parry taken opens it; the strike rolls the client row plus
    /// a share of attack power.
    Revenge {
        spell_id: i32,
        trigger_aura: String,
        aura: String,
        damage: DamageRoll,
        attack_power_share: f64,
    },
    /// Shield Slam: the client roll plus the block value.
    ShieldSlam {
        spell_id: i32,
        damage: DamageRoll,
        can_block: bool,
    },
    /// Thunder Clap: the row's average plus a share of attack power on the magic table; a
    /// landed clap slows the target by its bid.
    ThunderClap {
        spell_id: i32,
        base_damage: f64,
        attack_power_share: f64,
        max_targets: i32,
        aura: String,
        bid: f64,
    },
    /// Warrior items.go Battlegear of Might's 5 piece bonus: landed hits taken that dealt
    /// damage roll the chance under the label, then give rage a batch window later.
    BattlegearOfMightRage {
        trigger_aura: String,
        rng_label: String,
        proc_chance: f64,
        rage: f64,
        metrics_action_id: ActionId,
    },
    /// Sweeping Strikes: in Battle Stance, an aura with the row's charges, whose copies of
    /// hits need a second target and so never happen in scope.
    SweepingStrikes {
        spell_id: i32,
        aura: String,
        charges: i32,
    },
    /// Retaliation: charges that strike back at landed melee hits taken.
    Retaliation {
        spell_id: i32,
        aura: String,
        hit_spell_id: i32,
        charges: i32,
        hit_base_damage: f64,
    },
    /// Shield Specialization and Master of Defense: rage at a chance on avoided hits taken.
    RageOnAvoid {
        can_block: bool,
        triggers: Vec<AvoidTrigger>,
    },
    /// The warrior's Enrage: a landed hit taken that dealt damage enrages at a chance.
    WarriorEnrage {
        trigger_aura: String,
        aura: String,
        proc_chance: f64,
        physical_damage_done: f64,
    },
    /// Improved Hamstring: a landed Hamstring activates the root on the target at a chance,
    /// after a delay.
    ImprovedHamstring {
        trigger_aura: String,
        aura: String,
        proc_chance: f64,
        delay_ns: i64,
    },
    /// Blood Craze: a hot of maximum health after a crit or a large hit taken.
    BloodCraze {
        spell_id: i32,
        heal: HealModifiers,
        damage_taken_aura: String,
        health_fraction: f64,
        hit_threshold: f64,
    },
    /// Aimed Shot: a normalized ranged weapon shot plus the rank's flat bonus, dealt after
    /// travel.
    AimedShot {
        spell_id: i32,
        flat_bonus: f64,
    },
    /// Arcane Shot: the rank's flat damage plus a share of ranged attack power, with its spell
    /// power coefficient, on the ranged hit and crit table after travel.
    ArcaneShot {
        spell_id: i32,
        base_damage: f64,
        rap_coefficient: f64,
    },
    /// Renataki's Charm of Beasts: its use resets the named shots' cooldowns, and its major
    /// cooldown waits for one of them to be cooling.
    RenatakisCharm {
        item_id: i32,
        shots: Vec<i32>,
    },
    /// A Hunter set bonus proc that restores mana: the listed spells' hits of an outcome,
    /// `landed` or `crit`, roll the chance, and a delay later the mana is restored.
    HunterSetManaProc {
        trigger_aura: String,
        spells: Vec<usize>,
        outcome: String,
        proc_chance: f64,
        mana: f64,
        metrics_action_id: ActionId,
        delay_ns: i64,
    },
    /// Sniper Shot: Aimed Shot's shape with its own flat bonus.
    SniperShot {
        spell_id: i32,
        flat_bonus: f64,
    },
    /// Multi-Shot: one normalized ranged weapon shot on the one target in scope.
    MultiShot {
        spell_id: i32,
    },
    /// Serpent Sting: a ranged hit roll, then after travel a dot whose ticks add a share of
    /// ranged attack power.
    SerpentSting {
        spell_id: i32,
        tick_base: f64,
        attack_power_share: f64,
        tick_outcome: String,
    },
    /// Aspect of the Hawk: activates its aura, whose ranged attack power is a stat aura;
    /// Deadly Aspects procs a ranged haste aura on ranged autos.
    AspectOfTheHawk {
        spell_id: i32,
        aura: String,
        #[serde(default)]
        proc_aura: Option<String>,
        #[serde(default)]
        haste_multiplier: Option<f64>,
        #[serde(default)]
        proc_chance: Option<f64>,
    },
    /// Rapid Fire: an aura that multiplies ranged and melee attack speed.
    RapidFire {
        spell_id: i32,
        aura: String,
        haste_multiplier: f64,
    },
    /// Summon Hawk: a dive bomb on a base plus a share of ranged attack power, then a hawk dot
    /// in a free slot or the one with the least time left, which swings on arrival and on every
    /// tick for `swing_share` of the base.
    SummonHawk {
        spell_id: i32,
        base_damage: f64,
        attack_power_share: f64,
        swing_share: f64,
        always_hits: bool,
        hawk_spells: Vec<usize>,
        hawk_duration_ns: i64,
    },
    /// Go consumes.go conjured item that restores energy, such as Thistle Tea.
    ConjuredEnergy {
        item_id: i32,
        rng_label: String,
        gains: Vec<ManaGain>,
        selected: bool,
        /// How much of the gain may overflow the bar when the cast fires, a Go literal.
        spill: f64,
    },
    SinisterStrike {
        spell_id: i32,
        base_damage: f64,
    },
    Backstab {
        spell_id: i32,
        base_damage: f64,
        main_hand_dagger: bool,
        extra_combo_point_chance: f64,
        extra_combo_point_action: ActionId,
    },
    Eviscerate {
        spell_id: i32,
        damage_average: f64,
        damage_variance: f64,
        combo_point_damage: f64,
        attack_power_per_combo_point: f64,
    },
    SliceAndDice {
        spell_id: i32,
        aura: String,
        durations_ns: Vec<i64>,
        melee_speed_multiplier: f64,
    },
    BladeFlurry {
        spell_id: i32,
        aura: String,
        attack_speed_multiplier: f64,
        /// The extra hit Blade Flurry casts on the next target.
        hit_spell_id: i32,
    },
    AdrenalineRush {
        spell_id: i32,
        aura: String,
        regen_multiplier: f64,
        energy_threshold: f64,
    },
    RogueFinisher {
        relentless_strikes: bool,
        relentless_strikes_chance_per_point: f64,
        relentless_strikes_energy: f64,
        relentless_strikes_action: ActionId,
        ruthlessness_chance: f64,
        ruthlessness_action: ActionId,
    },
    InstantPoison {
        trigger_aura: String,
        spell_id: i32,
        proc_mask: Vec<String>,
        proc_chance: f64,
        min_damage: f64,
        max_damage: f64,
        /// The share of attack power each hit adds (community #781).
        attack_power_share: f64,
    },
    DeadlyPoison {
        trigger_aura: String,
        spell_id: i32,
        tag: i32,
        proc_mask: Vec<String>,
        proc_chance: f64,
        tick_damage: f64,
        /// The share of attack power each stack adds to a tick, read at the tick (community
        /// #781).
        attack_power_share: f64,
    },
    /// Go sim/rogue/stealth.go and vanish.go: Stealth before the pull, and Vanish.
    Stealth {
        aura: String,
        spell_id: i32,
        vanish_spell_id: i32,
    },
    Ambush {
        spell_id: i32,
        base_damage: f64,
        main_hand_dagger: bool,
        cutthroat_aura: String,
    },
    Rupture {
        spell_id: i32,
        tick_damage: f64,
        damage_per_combo_point: f64,
        base_tick_count: i32,
        attack_power_shares: Vec<f64>,
        tick_can_crit: bool,
        magic: bool,
        hemorrhage_aura: String,
        hemorrhage_multiplier: f64,
    },
    Mutilate {
        spell_id: i32,
        flat_damage: f64,
        poison_bonus: f64,
        combo_points: i32,
        daggers: bool,
        weapon_share: f64,
    },
    ColdBlood {
        spell_id: i32,
        aura: String,
        crit_bonus: f64,
        class_spells: Vec<String>,
        /// The class spells whose hit spends the aura.
        spend_class_spells: Vec<String>,
    },
    Premeditation {
        spell_id: i32,
        combo_points: i32,
    },
    Preparation {
        spell_id: i32,
        /// Every other Rogue spell with a cooldown, by spellbook position.
        reset_spells: Vec<usize>,
    },
    /// A Rogue talent proc trigger: the spells it hears, the outcome and chance, and what its
    /// handler does a spell batch window later.
    RogueProc {
        trigger_aura: String,
        handler: String,
        proc_chance: f64,
        spells: Vec<usize>,
        outcome: String,
        periodic: bool,
        delay_ns: i64,
        #[serde(default)]
        action: Option<ActionId>,
        #[serde(default)]
        aura: Option<String>,
    },
    ThousandCuts {
        aura: String,
        cost_per_stack: i32,
        class_spells: Vec<String>,
    },
    WoundPoison {
        trigger_aura: String,
        spell_id: i32,
        proc_mask: Vec<String>,
        proc_chance: f64,
        debuff_aura: String,
    },
    Venom {
        spell_id: i32,
        aura: String,
        durations_ns: Vec<i64>,
        damage_bonus: f64,
        chance_bonus: f64,
        class_spells: Vec<String>,
    },
    GhostlyStrike {
        spell_id: i32,
        dodge_aura: String,
    },
    Hemorrhage {
        spell_id: i32,
        debuff_aura: String,
    },
    /// Go sim/rogue/kidney_shot.go: a finisher whose stun pauses the swings of a target that
    /// tanks the player. A stun immune target gets only the finisher.
    KidneyShot {
        spell_id: i32,
        target_stun_immune: bool,
        stun_aura: String,
        base_duration_ns: i64,
        duration_per_combo_point_ns: i64,
        stun_duration_multiplier: f64,
        damage_taken_multiplier: f64,
        target_tanks_player: bool,
    },
    /// Go sim/rogue/talents_combat.go `registerRiposte` against a target that tanks the
    /// player: a parry readies a main hand weapon strike.
    Riposte {
        spell_id: i32,
        trigger_aura: String,
        ready_aura: String,
    },
    /// Go sim/rogue/expose_armor.go: a finisher whose debuff bids its armor a combo point in
    /// the target's major armor category, with Improved Expose Armor's combo points back on a
    /// five point spend. A permanent member that holds the category from the reset blocks it
    /// for good with its bid.
    ExposeArmor {
        spell_id: i32,
        aura: String,
        armor_per_combo_point: f64,
        points_back: i32,
        points_back_action: ActionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocking_priority: Option<f64>,
    },
    Quietus {
        aura: String,
        execute_phase: i32,
        damage_bonus: f64,
        class_spells: Vec<String>,
    },
    Garrote {
        spell_id: i32,
        tick_damage: f64,
        attack_power_share: f64,
        tick_can_crit: bool,
        magic: bool,
        dirty_deeds: bool,
    },
    /// Go consumes.go Goblin Sapper Charge: a rolled Fire hit on the target and a second roll
    /// on the player, which rolls on the player's attack table against itself.
    GoblinSapper {
        item_id: i32,
        self_tag: i32,
        min_damage: f64,
        max_damage: f64,
        aoe_cap_multiplier: f64,
        self_attack_table: AttackTable,
    },
    /// Aspect of the Beast: Aspect of the Hawk's shape for melee, its attack power a stat aura
    /// and Deadly Aspects' Quick Strikes a melee haste aura on landed melee white hits.
    AspectOfTheBeast {
        spell_id: i32,
        aura: String,
        #[serde(default)]
        proc_aura: Option<String>,
        #[serde(default)]
        haste_multiplier: Option<f64>,
        #[serde(default)]
        proc_chance: Option<f64>,
    },
    /// Raptor Strike: the queue spell's aura makes the next main hand swing cast it, whose hit
    /// adds the rank's flat damage to a main hand weapon swing.
    RaptorStrike {
        spell_id: i32,
        queue_aura: String,
        base_damage: f64,
        melee_range: f64,
    },
    /// Mongoose Bite: cast in the Defensive State window, the rank's flat damage plus a
    /// normalized main hand swing; Lacerating Strikes bleeds a share of a landed bite.
    MongooseBite {
        spell_id: i32,
        aura: String,
        base_damage: f64,
        #[serde(default)]
        lacerating_share: Option<f64>,
        #[serde(default)]
        lacerating_tick_outcome: Option<String>,
    },
    /// Strider Kick: a normalized main hand swing, then the aura that speeds the hunter up for
    /// its duration (an active movement speed effect, which `player_movement` lists).
    StriderKick {
        spell_id: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        speed_aura: Option<String>,
    },
    /// Wing Clip: its damage effect on the weapon special table.
    WingClip {
        spell_id: i32,
        base_damage: f64,
    },
    /// Freezing Trap (Forever's rules): a cast without an effect, which spends its mana, global
    /// cooldown and the trap category's cooldown.
    FreezingTrap {
        spell_id: i32,
    },
    /// Immolation Trap: a magic hit roll without a hit count, then a fire dot.
    ImmolationTrap {
        spell_id: i32,
        tick_base: f64,
        /// The share of the higher of ranged and melee attack power the whole burn takes, a Go
        /// literal (patch 147), split over its ticks.
        attack_power_share: f64,
        /// Whether the effect row lets a tick crit, and whether the trap is a magic spell.
        tick_can_crit: bool,
        tick_magic: bool,
    },
    /// Explosive Trap: `hits` magic hits from the cast target on, each rolled between the
    /// bounds and scaled by the AoE cap, then the area dot on the hunter, which ticks its
    /// snapshot of `tick_base` on every target Immolation Trap is not burning.
    ExplosiveTrap {
        spell_id: i32,
        hit_min: f64,
        hit_max: f64,
        /// The share of the higher of ranged and melee attack power each hit adds to its roll,
        /// a Go literal (patch 147); the burning ground takes none.
        attack_power_share: f64,
        hits: i32,
        aoe_cap_multiplier: f64,
        tick_base: f64,
        /// Whether the effect row lets a tick crit, and whether the trap is a magic spell.
        tick_can_crit: bool,
        tick_magic: bool,
    },
    /// Volley: the channel holds the ranged swing for `ranged_delay_ns`, then the area dot on
    /// the hunter ticks its snapshot of `tick_base` on every target.
    Volley {
        spell_id: i32,
        tick_base: f64,
        ranged_delay_ns: i64,
    },
    /// Resourcefulness: crits proc casting mana regeneration a spell batch window later.
    Resourcefulness {
        trigger_aura: String,
        aura: String,
        proc_chance: f64,
        regen: f64,
        /// The spells whose crits it hears: melee and ranged abilities (16 October 2026 notes).
        spells: Vec<usize>,
    },
    /// Rapid Recuperation: Serpent Sting's landed hit grants casting regeneration.
    RapidRecuperation {
        trigger_aura: String,
        aura: String,
        regen: f64,
    },
    /// Expose Prey: melee and ranged hits on a marked target open the Defensive State window.
    ExposePrey {
        trigger_aura: String,
        aura: String,
        proc_chance: f64,
        marked: bool,
    },
    /// Go hunter pet.go ExecuteCustomRotation: the family's ability slots, as positions in the
    /// pet's spellbook or -1, its rotation, the wait when nothing is cast and the move into melee
    /// range.
    HunterPet {
        pet: String,
        rotation: String,
        special_ability: i64,
        focus_dump: i64,
        extra_ability: i64,
        wait_ns: i64,
        melee_range: f64,
        move_to: f64,
        /// The share of the fight the pet stays out: once less remains, its rotation
        /// disables it.
        uptime: f64,
    },
    /// A hunter pet ability that deals one hit: its range, whether Go draws a roll (a client
    /// row without a variance does not), and its table, `melee_special` or `magic`.
    HunterPetStrike {
        spell_id: i32,
        min_damage: f64,
        max_damage: f64,
        draws: bool,
        outcome: String,
        /// A client row's average and variance, which Go `Effect.Roll` reads; absent for a
        /// Go literal range, which `Simulation.Roll` draws.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        average: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        variance: Option<f64>,
        /// Forever's rules: a cleave that hits the target and the next ones up to this many,
        /// each a roll of its own, as Go `CalcAndDealCleaveDamageWithVariance` does.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cleave_targets: Option<i32>,
    },
    /// A hunter pet ability whose landed hit applies a dot: the hit's table, `melee_special` or
    /// `ranged`, the dot's tick base and its tick outcome.
    HunterPetBleed {
        spell_id: i32,
        hit: String,
        tick_base: f64,
        tick_outcome: String,
    },
    /// The Bear's Swipe, whose cast condition needs this many active targets. With them up
    /// (Forever's rules) it cleaves `max_targets` of them, each a roll of `average` and
    /// `variance` on the melee special table.
    HunterPetSwipe {
        spell_id: i32,
        min_targets: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cleave: Option<PetCleave>,
    },
    /// The Scorpid's Scorpid Poison: a melee special hit roll, then one stack of a dot whose
    /// tick is the Go literal base on the multiplier at the cast.
    HunterPetScorpidPoison {
        spell_id: i32,
        tick_base: f64,
        /// Whether the poison's row lets a tick crit, and whether the spell is magic.
        tick_can_crit: bool,
        tick_magic: bool,
    },
    /// The Tallstrider's Dust Cloud: a melee special hit roll, then the target aura that
    /// changes its armor by this amount while it holds.
    HunterPetDustCloud {
        spell_id: i32,
        aura: String,
        armor: f64,
    },
    /// The Wolf's Furious Howl: the pet's aura adds this melee attack power to the pet while it
    /// holds, and the owner takes the owner's aura (a stat aura of `stat_auras`) when the cast
    /// finds the owner within the radius of the target. The pet casts it while its aura is down.
    HunterPetFuriousHowl {
        spell_id: i32,
        pet_aura: String,
        owner_aura: String,
        radius: f64,
        attack_power: f64,
    },
    /// Intimidation: the pet's aura, the physical crit it adds, a Go literal, and the landed hit
    /// that ends it.
    Intimidation {
        spell_id: i32,
        aura: String,
        crit_bonus: f64,
    },
    /// Bestial Wrath: the pet's aura and its damage dealt multiplier, a Go literal.
    BestialWrath {
        spell_id: i32,
        aura: String,
        damage_multiplier: f64,
    },
    /// Frenzy: the pet's crits proc its attack speed aura a spell batch window later.
    Frenzy {
        trigger_aura: String,
        aura: String,
        proc_chance: f64,
        speed_multiplier: f64,
        delay_ns: i64,
    },
    /// Sulfuras, Hand of Ragnaros: a weapon proc that casts its Fireball at once, a magic hit
    /// rolled between two bounds whose landing applies a burn of a fixed base, Go literals;
    /// and Immolation, a fixed Fire hit on every melee attacker that lands a hit on the wearer.
    SulfurasHandOfRagnaros {
        trigger_aura: String,
        chances: Vec<SpellChance>,
        fireball_spell: usize,
        roll_min: f64,
        roll_max: f64,
        dot_base: f64,
        immolation_aura: String,
        immolation_spell: usize,
        immolation_damage: f64,
    },
    /// Jom Gabbar (classic items_trinkets.go): the item's spell activates `aura`, a stacking stat
    /// aura among the stat auras. Gaining it adds a stack at once, and another every `period_ns`
    /// until `ticks` have been added in all (`StartPeriodicAction` with `TickImmediately`).
    StackingOnUse {
        spell_id: i32,
        aura: String,
        period_ns: i64,
        ticks: i32,
    },
    /// Force Reactive Disk (classic items_armor.go): a listener on the melee hits the wearer takes
    /// that the wearer blocks, behind the aura's cooldown, whose handler a spell batch window later
    /// casts `spell` for `damage` on every target, each rolling the magic table with a crit.
    ForceReactiveDisk {
        trigger_aura: String,
        spell: usize,
        damage: f64,
    },
    /// Thunderfury, Blessed Blade of the Windseeker: a weapon proc on landed hits whose handler
    /// a spell batch window later casts two spells on the unit hit. The strike is a hit of a Go
    /// literal on the magic table with a crit, whose landing puts the slow on the target: Cyclone,
    /// an exclusive effect in the attack speed category Thunder Clap shares, that multiplies the
    /// target's attack speed by one over `slow_multiplier`. The bounce deals nothing on the magic
    /// table to up to `bounce_targets` targets from the unit hit, and each it lands on takes the
    /// resistance aura, which adds `nature_resistance` to the target's nature resistance.
    Thunderfury {
        trigger_aura: String,
        chances: Vec<SpellChance>,
        strike_spell: usize,
        bounce_spell: usize,
        strike_damage: f64,
        bounce_targets: i32,
        slow_aura: String,
        slow_multiplier: f64,
        resistance_aura: String,
        nature_resistance: f64,
    },
    /// Dragon's Call: a weapon proc whose handler summons the Emerald Dragon Whelp, a guardian
    /// whose rotation spits Acid Spit half the time.
    EmeraldDragonWhelp {
        trigger_aura: String,
        pet: String,
        chances: Vec<SpellChance>,
        delay_ns: i64,
        duration_ns: i64,
        acid_spit_spell_id: i32,
        acid_spit_min: f64,
        acid_spit_max: f64,
        spit_chance: f64,
    },
    /// Go consumes.go newBasicExplosiveSpellConfig without the self hit: a rolled hit on every
    /// target scaled by the AoE cap, dealt after travel when the explosive flies.
    BasicExplosive {
        item_id: i32,
        min_damage: f64,
        max_damage: f64,
        aoe_cap_multiplier: f64,
    },
    /// Go health.go trackChanceOfDeath once a spell can hit the player.
    ChanceOfDeath {
        aura: String,
    },
    /// Go attack.go applyParryHaste once the target swings at the player: a parry pulls the
    /// parrying unit's next main hand swing in.
    ParryHaste {
        unit: String,
        aura: String,
        /// For an untanked target, which never swings: the swing timer its reset rolls, which
        /// a parry still pulls in.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        swing_speed: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        melee_haste_multiplier: Option<f64>,
    },
    /// Go character.go's "Pushback trigger" on a tanking player: a landed hit that deals damage
    /// during a hardcast with the pushback flag pushes the cast back a spell batch window later.
    /// `chance` is the player's `PseudoStats.PushbackChance`, which each spell's
    /// `pushback_resist` reduces.
    PushbackTrigger {
        aura: String,
        chance: f64,
    },
    /// A proc trigger on the player's casts of the spells it hears, by spellbook position, whose
    /// handler a delay later restores a fixed amount of health through Go `GainHealth`, as
    /// Nightslayer Armor's eight piece bonus does on Vanish.
    HealthOnCast {
        trigger_aura: String,
        trigger_spells: Vec<usize>,
        health: f64,
        metrics_action_id: ActionId,
        delay_ns: i64,
    },
    /// An item proc trigger that restores energy a spell batch window after a landed hit, such
    /// as Shadowcraft Armor's: the chance each spell rolls, by spellbook position.
    EnergizeProc {
        trigger_aura: String,
        rng_label: String,
        chances: Vec<SpellChance>,
        energy: f64,
        metrics_action_id: ActionId,
        delay_ns: i64,
    },
    /// The same for mana, under Forever's rules: a set bonus proc trigger the class's own
    /// domain does not export, whose handler a spell batch window after a landed hit restores
    /// mana to a character with a mana bar, as Beaststalker Armor's five piece bonus on a class
    /// other than a Hunter.
    ManaProc {
        trigger_aura: String,
        rng_label: String,
        chances: Vec<SpellChance>,
        mana: f64,
        metrics_action_id: ActionId,
        delay_ns: i64,
    },
    /// Go aura_helpers.go ApplyFixedUptimeAura: a periodic roll that activates the aura and a
    /// first roll with a random duration.
    FixedUptimeAura {
        aura: String,
        uptime: f64,
        tick_length_ns: i64,
        start_time_ns: i64,
        /// buffs.go ApplyFixedShoutAura: the player's own aura whose gain brings this one back
        /// a reaction time after it runs out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chained_by: Option<String>,
    },
}

impl Effect {
    /// The stable kind name used in capability reports.
    pub fn kind(&self) -> &'static str {
        match self {
            Effect::Frostbolt {} => "frostbolt",
            Effect::ArcaneBlast { .. } => "arcane_blast",
            Effect::ArcaneWrath { .. } => "arcane_wrath",
            Effect::FirePower { .. } => "fire_power",
            Effect::ArcanePower { .. } => "arcane_power",
            Effect::Ignite { .. } => "ignite",
            Effect::FireBlast {} => "fire_blast",
            Effect::ArcaneExplosion {} => "arcane_explosion",
            Effect::ConeOfCold {} => "cone_of_cold",
            Effect::FrostNova {} => "frost_nova",
            Effect::BlastWave {} => "blast_wave",
            Effect::Flamestrike { .. } => "flamestrike",
            Effect::Blizzard { .. } => "blizzard",
            Effect::Combustion { .. } => "combustion",
            Effect::Pyroblast { .. } => "pyroblast",
            Effect::HeatingUp { .. } => "heating_up",
            Effect::MasterOfElements { .. } => "master_of_elements",
            Effect::Fireball { .. } => "fireball",
            Effect::FrostfireBolt { .. } => "frostfire_bolt",
            Effect::Scorch { .. } => "scorch",
            Effect::TouchOfTheGrave { .. } => "touch_of_the_grave",
            Effect::BladeOfHanna { .. } => "blade_of_hanna",
            Effect::Eureka { .. } => "eureka",
            Effect::Berserking { .. } => "berserking",
            Effect::BloodFury { .. } => "blood_fury",
            Effect::ExternalCooldown { .. } => "external_cooldown",
            Effect::InnervateRegen { .. } => "innervate_regen",
            Effect::TemporaryStats { .. } => "temporary_stats",
            Effect::SpeedOnUse { .. } => "speed_on_use",
            Effect::DiamondFlask { .. } => "diamond_flask",
            Effect::PlayerMovement { .. } => "player_movement",
            Effect::WarriorCharge { .. } => "warrior_charge",
            Effect::DruidForms { .. } => "druid_forms",
            Effect::MoonkinForm { .. } => "moonkin_form",
            Effect::Starfire {} => "starfire",
            Effect::Wrath {} => "wrath",
            Effect::Moonfire { .. } => "moonfire",
            Effect::InsectSwarm { .. } => "insect_swarm",
            Effect::EntanglingRoots {} => "entangling_roots",
            Effect::Innervate { .. } => "innervate",
            Effect::OmenOfClarity { .. } => "omen_of_clarity",
            Effect::NaturesGrace { .. } => "natures_grace",
            Effect::Eclipse { .. } => "eclipse",
            Effect::MindBlast {} => "mind_blast",
            Effect::ShadowWordDeath { .. } => "shadow_word_death",
            Effect::ShadowWordPain { .. } => "shadow_word_pain",
            Effect::DevouringPlague { .. } => "devouring_plague",
            Effect::MindFlay { .. } => "mind_flay",
            Effect::ImprovedMindFlay { .. } => "improved_mind_flay",
            Effect::DarkInspiration { .. } => "dark_inspiration",
            Effect::Starshards { .. } => "starshards",
            Effect::HolyNova { .. } => "holy_nova",
            Effect::Shadowfiend { .. } => "shadowfiend",
            Effect::PowerInfusion { .. } => "power_infusion",
            Effect::Shadowform { .. } => "shadowform",
            Effect::InnerFocus { .. } => "inner_focus",
            Effect::ShadowWeaving { .. } => "shadow_weaving",
            Effect::VampiricEmbrace { .. } => "vampiric_embrace",
            Effect::DarkSacrifice { .. } => "dark_sacrifice",
            Effect::InertPet { .. } => "inert_pet",
            Effect::Smite {} => "smite",
            Effect::HolyFire { .. } => "holy_fire",
            Effect::Penance { .. } => "penance",
            Effect::PowerInLight { .. } => "power_in_light",
            Effect::SearingLight { .. } => "searing_light",
            Effect::JudgementRefresh { .. } => "judgement_refresh",
            Effect::Judgement { .. } => "judgement",
            Effect::SealOfCommand { .. } => "seal_of_command",
            Effect::SealOfRighteousness { .. } => "seal_of_righteousness",
            Effect::ItemSwap { .. } => "item_swap",
            Effect::SealOfTheCrusader { .. } => "seal_of_the_crusader",
            Effect::SealOfLight { .. } => "seal_of_light",
            Effect::SealOfWisdom { .. } => "seal_of_wisdom",
            Effect::SealOfJustice { .. } => "seal_of_justice",
            Effect::SealOfFury { .. } => "seal_of_fury",
            Effect::HolyLightHaste { .. } => "holy_light_haste",
            Effect::PaladinHeals { .. } => "paladin_heals",
            Effect::PaladinAuraCast { .. } => "paladin_aura_cast",
            Effect::Heals { .. } => "heals",
            Effect::PriestHealTalents { .. } => "priest_heal_talents",
            Effect::Bloom { .. } => "bloom",
            Effect::LayOnHands { .. } => "lay_on_hands",
            Effect::EyeForAnEye { .. } => "eye_for_an_eye",
            Effect::PursuitOfJustice { .. } => "pursuit_of_justice",
            Effect::HolyStrike { .. } => "holy_strike",
            Effect::HammerOfWrath { .. } => "hammer_of_wrath",
            Effect::HammerOfTheRighteous { .. } => "hammer_of_the_righteous",
            Effect::Exorcism { .. } => "exorcism",
            Effect::HolyWrath { .. } => "holy_wrath",
            Effect::Consecration { .. } => "consecration",
            Effect::Vengeance { .. } => "vengeance",
            Effect::Vindication { .. } => "vindication",
            Effect::SanctifiedJudgement { .. } => "sanctified_judgement",
            Effect::SacredArbiter { .. } => "sacred_arbiter",
            Effect::TwistOfLight { .. } => "twist_of_light",
            Effect::UnsealedWrath { .. } => "unsealed_wrath",
            Effect::SecondJudgement { .. } => "second_judgement",
            Effect::Shieldsworn { .. } => "shieldsworn",
            Effect::HolyShock { .. } => "holy_shock",
            Effect::LightsVigil { .. } => "lights_vigil",
            Effect::LightsVigilAlly { .. } => "lights_vigil_ally",
            Effect::StatProc { .. } => "stat_proc",
            Effect::Illumination { .. } => "illumination",
            Effect::RighteousFury { .. } => "righteous_fury",
            Effect::SwiftJudgement { .. } => "swift_judgement",
            Effect::TemplarsBulwark { .. } => "templars_bulwark",
            Effect::Redoubt { .. } => "redoubt",
            Effect::ShieldSpecialization { .. } => "shield_specialization",
            Effect::Reckoning { .. } => "reckoning",
            Effect::IronCreed { .. } => "iron_creed",
            Effect::HolyShield { .. } => "holy_shield",
            Effect::DivineFavor { .. } => "divine_favor",
            Effect::OathOfLight { .. } => "oath_of_light",
            Effect::SpellDataDamageProc { .. } => "spell_data_damage_proc",
            Effect::SpellDataHealProc { .. } => "spell_data_heal_proc",
            Effect::SecondWind { .. } => "second_wind",
            Effect::AbsorbOnUse { .. } => "absorb_on_use",
            Effect::SpellDataAbsorbProc { .. } => "spell_data_absorb_proc",
            Effect::HealOnUse { .. } => "heal_on_use",
            Effect::HealthRageProc { .. } => "health_rage_proc",
            Effect::EnergyProc { .. } => "energy_proc",
            Effect::BattleSquawk { .. } => "battle_squawk",
            Effect::SpeedAura { .. } => "speed_aura",
            Effect::MobTypeBonus { .. } => "mob_type_bonus",
            Effect::HealthRegenAura { .. } => "health_regen_aura",
            Effect::OilOfImmolation { .. } => "oil_of_immolation",
            Effect::DamageOnUse { .. } => "damage_on_use",
            Effect::ArmorDebuffProc { .. } => "armor_debuff_proc",
            Effect::SunderArmorRamp { .. } => "sunder_armor_ramp",
            Effect::ExternalExposeArmor { .. } => "external_expose_armor",
            Effect::StatAuras { .. } => "stat_auras",
            Effect::CatForm { .. } => "cat_form",
            Effect::Prowl { .. } => "prowl",
            Effect::CatBuilders { .. } => "cat_builders",
            Effect::Rip { .. } => "rip",
            Effect::Rake { .. } => "rake",
            Effect::FerociousBite { .. } => "ferocious_bite",
            Effect::ShiftingPower { .. } => "shifting_power",
            Effect::FaerieFire { .. } => "faerie_fire",
            Effect::Berserk { .. } => "berserk",
            Effect::BloodFrenzy { .. } => "blood_frenzy",
            Effect::RendAndTear { .. } => "rend_and_tear",
            Effect::BearForm { .. } => "bear_form",
            Effect::Enrage { .. } => "enrage",
            Effect::DemoralizingRoar { .. } => "demoralizing_roar",
            Effect::Maul { .. } => "maul",
            Effect::Lacerate { .. } => "lacerate",
            Effect::PrimalBite { .. } => "primal_bite",
            Effect::Swipe { .. } => "swipe",
            Effect::Hurricane { .. } => "hurricane",
            Effect::DruidThorns { .. } => "druid_thorns",
            Effect::NaturalReaction { .. } => "natural_reaction",
            Effect::Barkskin { .. } => "barkskin",
            Effect::NaturesBounty { .. } => "natures_bounty",
            Effect::UnendingLifeRefund { .. } => "unending_life_refund",
            Effect::FrenziedRegeneration { .. } => "frenzied_regeneration",
            Effect::AuraShouldRefresh { .. } => "aura_should_refresh",
            Effect::WindfuryTotem { .. } => "windfury_totem",
            Effect::Crusader { .. } => "crusader",
            Effect::DragonbreathChili { .. } => "dragonbreath_chili",
            Effect::ShatterCurse { .. } => "shatter_curse",
            Effect::Stoneform { .. } => "stoneform",
            Effect::ShieldWall { .. } => "shield_wall",
            Effect::ShieldBlock { .. } => "shield_block",
            Effect::LastStand { .. } => "last_stand",
            Effect::ReadLeyLine { .. } => "read_ley_line",
            Effect::Skysight { .. } => "skysight",
            Effect::PresenceOfMind { .. } => "presence_of_mind",
            Effect::IceLance { .. } => "ice_lance",
            Effect::ArcaneMissiles { .. } => "arcane_missiles",
            Effect::ColdSnap { .. } => "cold_snap",
            Effect::IceBarrier { .. } => "ice_barrier",
            Effect::Evocation { .. } => "evocation",
            Effect::ManaGems { .. } => "mana_gems",
            Effect::MageArmor { .. } => "mage_armor",
            Effect::ArcaneConcentration { .. } => "arcane_concentration",
            Effect::MissileBarrage { .. } => "missile_barrage",
            Effect::FingersOfFrost { .. } => "fingers_of_frost",
            Effect::WintersChill { .. } => "winters_chill",
            Effect::JudgementOfWisdom { .. } => "judgement_of_wisdom",
            Effect::JudgementOfLight { .. } => "judgement_of_light",
            Effect::PotionMana { .. } => "potion_mana",
            Effect::ConjuredMana { .. } => "conjured_mana",
            Effect::EnergizeOnUse { .. } => "energize_on_use",
            Effect::SpellDataStatProc { .. } => "spell_data_stat_proc",
            Effect::SpellCostAuraOnUse { .. } => "spell_cost_aura_on_use",
            Effect::SpellModAuraOnUse { .. } => "spell_mod_aura_on_use",
            Effect::ManaOnCast { .. } => "mana_on_cast",
            Effect::StackingCdOnUse { .. } => "stacking_cd_on_use",
            Effect::ShieldOnHeal { .. } => "shield_on_heal",
            Effect::InstantCastAura { .. } => "instant_cast_aura",
            Effect::InertListener { .. } => "inert_listener",
            Effect::ShadowBolt {} => "shadow_bolt",
            Effect::Immolate { .. } => "immolate",
            Effect::Corruption { .. } => "corruption",
            Effect::BaneOfAgony { .. } => "bane_of_agony",
            Effect::AmplifyCurse { .. } => "amplify_curse",
            Effect::CurseOfTheElements { .. } => "curse_of_the_elements",
            Effect::LifeTap { .. } => "life_tap",
            Effect::Conflagrate { .. } => "conflagrate",
            Effect::Shadowburn {} => "shadowburn",
            Effect::Nightfall { .. } => "nightfall",
            Effect::Decimation { .. } => "decimation",
            Effect::SiphonLife { .. } => "siphon_life",
            Effect::BaneOfDoom { .. } => "bane_of_doom",
            Effect::DrainLife { .. } => "drain_life",
            Effect::Incinerate { .. } => "incinerate",
            Effect::DeathCoil { .. } => "death_coil",
            Effect::CurseOfRecklessness { .. } => "curse_of_recklessness",
            Effect::Wrack { .. } => "wrack",
            Effect::RainOfFire { .. } => "rain_of_fire",
            Effect::Hellfire { .. } => "hellfire",
            Effect::BaneOfHavoc { .. } => "bane_of_havoc",
            Effect::Firebolt { .. } => "firebolt",
            Effect::FelEnergy { .. } => "fel_energy",
            Effect::DemonicBrand { .. } => "demonic_brand",
            Effect::WarlockPet { .. } => "warlock_pet",
            Effect::LashOfPain { .. } => "lash_of_pain",
            Effect::SearingPain {} => "searing_pain",
            Effect::SoulFire {} => "soul_fire",
            Effect::ImprovedShadowBolt { .. } => "improved_shadow_bolt",
            Effect::ShadowAndFlame { .. } => "shadow_and_flame",
            Effect::LightningBolt { .. } => "lightning_bolt",
            Effect::ChainLightning { .. } => "chain_lightning",
            Effect::FlameShock { .. } => "flame_shock",
            Effect::LavaBurst { .. } => "lava_burst",
            Effect::FireNova { .. } => "fire_nova",
            Effect::SearingTotem { .. } => "searing_totem",
            Effect::ElementalFocus { .. } => "elemental_focus",
            Effect::EarthShock { .. } => "earth_shock",
            Effect::StrengthOfEarthTotem { .. } => "strength_of_earth_totem",
            Effect::Stormstrike { .. } => "stormstrike",
            Effect::ElementalDevastation { .. } => "elemental_devastation",
            Effect::Flurry { .. } => "flurry",
            Effect::ImprovedStormstrike { .. } => "improved_stormstrike",
            Effect::MaelstromWeapon { .. } => "maelstrom_weapon",
            Effect::RageOfTheFarseer { .. } => "rage_of_the_farseer",
            Effect::ElementalMastery { .. } => "elemental_mastery",
            Effect::RockbiterWeapon { .. } => "rockbiter_weapon",
            Effect::RockbiterThreat { .. } => "rockbiter_threat",
            Effect::FrostShock { .. } => "frost_shock",
            Effect::WeaponSync { .. } => "weapon_sync",
            Effect::WindfuryWeapon { .. } => "windfury_weapon",
            Effect::MagmaTotem { .. } => "magma_totem",
            Effect::LightningShield { .. } => "lightning_shield",
            Effect::WaterShield { .. } => "water_shield",
            Effect::GraceOfAirTotem { .. } => "grace_of_air_totem",
            Effect::ManaSpringTotem { .. } => "mana_spring_totem",
            Effect::ManaTideTotem { .. } => "mana_tide_totem",
            Effect::WindfuryTotemSelf { .. } => "windfury_totem_self",
            Effect::FlametongueTotem { .. } => "flametongue_totem",
            Effect::FlametongueWeapon { .. } => "flametongue_weapon",
            Effect::FrostbrandWeapon { .. } => "frostbrand_weapon",
            Effect::RageBar { .. } => "rage_bar",
            Effect::ExclusiveCategory { .. } => "exclusive_category",
            Effect::PseudoStatAuras { .. } => "pseudo_stat_auras",
            Effect::ExtraAttackProc { .. } => "extra_attack_proc",
            Effect::PlayerDamageTaken { .. } => "player_damage_taken",
            Effect::PotionResource { .. } => "potion_resource",
            Effect::Revelation { .. } => "revelation",
            Effect::DragonfirePotion { .. } => "dragonfire_potion",
            Effect::CausticSmogPotion { .. } => "caustic_smog_potion",
            Effect::ElementalPurgingPotion { .. } => "elemental_purging_potion",
            Effect::WarriorStances { .. } => "warrior_stances",
            Effect::Bloodthirst { .. } => "bloodthirst",
            Effect::Whirlwind { .. } => "whirlwind",
            Effect::Execute { .. } => "execute",
            Effect::Hamstring { .. } => "hamstring",
            Effect::Bloodrage { .. } => "bloodrage",
            Effect::BerserkerRage { .. } => "berserker_rage",
            Effect::DeathWish { .. } => "death_wish",
            Effect::Recklessness { .. } => "recklessness",
            Effect::SunderArmor { .. } => "sunder_armor",
            Effect::DeepWounds { .. } => "deep_wounds",
            Effect::UnbridledWrath { .. } => "unbridled_wrath",
            Effect::WarriorFlurry { .. } => "warrior_flurry",
            Effect::AngerManagement { .. } => "anger_management",
            Effect::HeroicStrikeQueue { .. } => "heroic_strike_queue",
            Effect::OverpowerWindow { .. } => "overpower_window",
            Effect::BattleShout { .. } => "battle_shout",
            Effect::Rend { .. } => "rend",
            Effect::Overpower { .. } => "overpower",
            Effect::MortalStrike { .. } => "mortal_strike",
            Effect::SpearingStrike { .. } => "spearing_strike",
            Effect::Slam { .. } => "slam",
            Effect::DemoralizingShout { .. } => "demoralizing_shout",
            Effect::ChallengingShout { .. } => "challenging_shout",
            Effect::Taunt { .. } => "taunt",
            Effect::IntimidatingShout { .. } => "intimidating_shout",
            Effect::ConcussionBlow { .. } => "concussion_blow",
            Effect::Pummel { .. } => "pummel",
            Effect::ShieldBash { .. } => "shield_bash",
            Effect::MockingBlow { .. } => "mocking_blow",
            Effect::WarriorDisarm { .. } => "warrior_disarm",
            Effect::VictoryRush { .. } => "victory_rush",
            Effect::WarriorIntercept { .. } => "warrior_intercept",
            Effect::Bloodthrill { .. } => "bloodthrill",
            Effect::WeaponmasterSword { .. } => "weaponmaster_sword",
            Effect::Revenge { .. } => "revenge",
            Effect::ShieldSlam { .. } => "shield_slam",
            Effect::ThunderClap { .. } => "thunder_clap",
            Effect::Retaliation { .. } => "retaliation",
            Effect::SweepingStrikes { .. } => "sweeping_strikes",
            Effect::BattlegearOfMightRage { .. } => "battlegear_of_might_rage",
            Effect::RageOnAvoid { .. } => "rage_on_avoid",
            Effect::WarriorEnrage { .. } => "warrior_enrage",
            Effect::BloodCraze { .. } => "blood_craze",
            Effect::ImprovedHamstring { .. } => "improved_hamstring",
            Effect::AimedShot { .. } => "aimed_shot",
            Effect::ArcaneShot { .. } => "arcane_shot",
            Effect::RenatakisCharm { .. } => "renatakis_charm",
            Effect::BlackBook { .. } => "black_book",
            Effect::HunterSetManaProc { .. } => "hunter_set_mana_proc",
            Effect::SniperShot { .. } => "sniper_shot",
            Effect::MultiShot { .. } => "multi_shot",
            Effect::SerpentSting { .. } => "serpent_sting",
            Effect::AspectOfTheHawk { .. } => "aspect_of_the_hawk",
            Effect::RapidFire { .. } => "rapid_fire",
            Effect::SummonHawk { .. } => "summon_hawk",
            Effect::ConjuredEnergy { .. } => "conjured_energy",
            Effect::SinisterStrike { .. } => "sinister_strike",
            Effect::Backstab { .. } => "backstab",
            Effect::Eviscerate { .. } => "eviscerate",
            Effect::SliceAndDice { .. } => "slice_and_dice",
            Effect::BladeFlurry { .. } => "blade_flurry",
            Effect::AdrenalineRush { .. } => "adrenaline_rush",
            Effect::RogueFinisher { .. } => "rogue_finisher",
            Effect::InstantPoison { .. } => "instant_poison",
            Effect::DeadlyPoison { .. } => "deadly_poison",
            Effect::GoblinSapper { .. } => "goblin_sapper",
            Effect::Stealth { .. } => "stealth",
            Effect::Ambush { .. } => "ambush",
            Effect::Rupture { .. } => "rupture",
            Effect::Mutilate { .. } => "mutilate",
            Effect::ColdBlood { .. } => "cold_blood",
            Effect::Premeditation { .. } => "premeditation",
            Effect::Preparation { .. } => "preparation",
            Effect::RogueProc { .. } => "rogue_proc",
            Effect::ThousandCuts { .. } => "thousand_cuts",
            Effect::WoundPoison { .. } => "wound_poison",
            Effect::Venom { .. } => "venom",
            Effect::GhostlyStrike { .. } => "ghostly_strike",
            Effect::Hemorrhage { .. } => "hemorrhage",
            Effect::Garrote { .. } => "garrote",
            Effect::Quietus { .. } => "quietus",
            Effect::KidneyShot { .. } => "kidney_shot",
            Effect::ExposeArmor { .. } => "expose_armor",
            Effect::Riposte { .. } => "riposte",
            Effect::BasicExplosive { .. } => "basic_explosive",
            Effect::EmeraldDragonWhelp { .. } => "emerald_dragon_whelp",
            Effect::Thunderfury { .. } => "thunderfury",
            Effect::ForceReactiveDisk { .. } => "force_reactive_disk",
            Effect::StackingOnUse { .. } => "stacking_on_use",
            Effect::SulfurasHandOfRagnaros { .. } => "sulfuras_hand_of_ragnaros",
            Effect::HunterPet { .. } => "hunter_pet",
            Effect::AspectOfTheBeast { .. } => "aspect_of_the_beast",
            Effect::RaptorStrike { .. } => "raptor_strike",
            Effect::MongooseBite { .. } => "mongoose_bite",
            Effect::StriderKick { .. } => "strider_kick",
            Effect::WingClip { .. } => "wing_clip",
            Effect::ImmolationTrap { .. } => "immolation_trap",
            Effect::FreezingTrap { .. } => "freezing_trap",
            Effect::ExplosiveTrap { .. } => "explosive_trap",
            Effect::Volley { .. } => "volley",
            Effect::Resourcefulness { .. } => "resourcefulness",
            Effect::RapidRecuperation { .. } => "rapid_recuperation",
            Effect::ExposePrey { .. } => "expose_prey",
            Effect::HunterPetStrike { .. } => "hunter_pet_strike",
            Effect::HunterPetBleed { .. } => "hunter_pet_bleed",
            Effect::HunterPetSwipe { .. } => "hunter_pet_swipe",
            Effect::HunterPetScorpidPoison { .. } => "hunter_pet_scorpid_poison",
            Effect::HunterPetDustCloud { .. } => "hunter_pet_dust_cloud",
            Effect::HunterPetFuriousHowl { .. } => "hunter_pet_furious_howl",
            Effect::Intimidation { .. } => "intimidation",
            Effect::BestialWrath { .. } => "bestial_wrath",
            Effect::Frenzy { .. } => "frenzy",
            Effect::ChanceOfDeath { .. } => "chance_of_death",
            Effect::ParryHaste { .. } => "parry_haste",
            Effect::PushbackTrigger { .. } => "pushback_trigger",
            Effect::FixedUptimeAura { .. } => "fixed_uptime_aura",
            Effect::HealthOnCast { .. } => "health_on_cast",
            Effect::EnergizeProc { .. } => "energize_proc",
            Effect::ManaProc { .. } => "mana_proc",
        }
    }
}
