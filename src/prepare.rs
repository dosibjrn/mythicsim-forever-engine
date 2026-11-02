//! Rust preparation: the application's request built into a reset simulation and described
//! as prepared v2, as the pinned Go engine and tools/oracle-v2 do.
//!
//! The modules mirror the Go files they port, under sim/core. A request feature preparation
//! does not cover yet is refused with a stable code, so the caller can prepare it in Go; a
//! request it prepares must give exactly the prepared state the Go exporter writes.

pub(crate) mod agent;
pub(crate) mod attack;
pub(crate) mod aura_helpers;
pub(crate) mod battle_squawk;
pub(crate) mod buffs;
pub(crate) mod burst_hps;
pub(crate) mod character;
pub(crate) mod classic_enchants;
pub(crate) mod classic_export;
pub(crate) mod classic_items;
pub(crate) mod classic_weapons;
pub(crate) mod classic_whelp;
pub(crate) mod common_effects;
pub(crate) mod consumable_effects;
pub(crate) mod consumes;
pub(crate) mod damage_taken;
pub(crate) mod dbcenums;
pub(crate) mod debuffs;
pub(crate) mod enchant_speed;
pub(crate) mod enemy;
pub(crate) mod energy;
pub(crate) mod env;
pub(crate) mod export;
pub(crate) mod export_items;
pub(crate) mod external_cooldowns;
pub(crate) mod forever_item_sets;
pub(crate) mod forever_items;
pub(crate) mod forever_items_generated;
pub(crate) mod forever_set_spells_generated;
pub(crate) mod healer_gear;
pub(crate) mod heals;
pub(crate) mod hotfix_113554;
pub(crate) mod incapacitate;
pub(crate) mod item_aura;
pub(crate) mod item_effects;
pub(crate) mod item_proc;
pub(crate) mod item_sets;
pub(crate) mod item_swap;
#[cfg(test)]
pub(crate) mod item_test_support;
pub(crate) mod itemhelpers;
pub(crate) mod items;
pub(crate) mod items_registry;
pub(crate) mod major_cooldown;
pub(crate) mod oil_of_immolation;
pub(crate) mod overheal;
pub(crate) mod parse_effects;
pub(crate) mod party_damage;
pub(crate) mod periodic_action;
pub(crate) mod pet;
pub(crate) mod power_infusion;
pub(crate) mod presets;
pub(crate) mod proc_oil;
pub(crate) mod proc_type_mask;
pub(crate) mod procs;
pub(crate) mod professions;
pub(crate) mod racials;
pub(crate) mod rage;
pub(crate) mod resolve_aura;
pub(crate) mod resolve_proc;
pub(crate) mod resolve_spell;
pub(crate) mod revelation;
pub(crate) mod rotation;
pub(crate) mod shared_auras;
pub(crate) mod shared_items;
pub(crate) mod shared_on_use;
pub(crate) mod shared_procs;
pub(crate) mod sim;
pub(crate) mod spell;
pub(crate) mod spell_mod;
pub(crate) mod spelldata;
pub(crate) mod stat_auras;
pub(crate) mod stats;
pub(crate) mod talent_layout;
pub(crate) mod target;
pub(crate) mod weapon_skill;

use crate::contracts::prepared_v2::PreparedV2;
use crate::contracts::request::Request;

/// Why preparation refuses a request: a stable code and the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub reason: String,
}

impl Refusal {
    pub(crate) fn new(code: &'static str, reason: String) -> Refusal {
        Refusal { code, reason }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.reason)
    }
}

/// The outcome of preparing a request in Rust.
#[derive(Debug)]
pub enum PrepareError {
    /// The request is not valid protojson for the reference's schema.
    Invalid(String),
    /// Rust preparation does not cover the request; Go can prepare it.
    Refused(Refusal),
    /// Preparation failed inside Rust, a defect; Go can prepare the request.
    Fault(String),
}

impl std::fmt::Display for PrepareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrepareError::Invalid(reason) => write!(f, "invalid request: {reason}"),
            PrepareError::Refused(refusal) => write!(f, "preparation refused: {refusal}"),
            PrepareError::Fault(reason) => write!(f, "preparation failed: {reason}"),
        }
    }
}

pub use sim::Rules;

/// Prepares a request as JSON, the document tools/oracle-v2 writes, with Forever's rules.
pub fn prepare_json(request: &[u8], scenario: &str) -> Result<serde_json::Value, PrepareError> {
    prepare_json_with(request, scenario, Rules::Forever)
}

/// Prepares a request as JSON with the given rules: [`Rules::GoReference`] gives exactly what the
/// pinned exporter writes, for the comparisons with Go.
pub fn prepare_json_with(
    request: &[u8],
    scenario: &str,
    rules: Rules,
) -> Result<serde_json::Value, PrepareError> {
    let bytes = request;
    let mut request = Request::from_json(bytes).map_err(PrepareError::Invalid)?;
    let options = request.message().message("sim_options");
    if options.is_none_or(|options| options.i32("iterations") <= 0) {
        return Err(PrepareError::Invalid("missing iterations".to_string()));
    }
    let digest = request.sha256();
    if rules == Rules::Forever {
        if let Some(known) = known_ranks(bytes)? {
            request = known;
        }
    }
    let mut env =
        env::Environment::with_rules(request.message(), crate::classes::prepare_agent, rules)
            .map_err(PrepareError::Refused)?;
    export::export(&mut env, &digest, scenario).map_err(PrepareError::Refused)
}

/// The request with every Ahn'Qiraj 20 book rank it names read as the rank below, which is what
/// a player knows under Forever's rules (`spelldata::book_ranks`), or None when it names none.
/// The digest stays the one of the request as sent.
fn known_ranks(bytes: &[u8]) -> Result<Option<Request>, PrepareError> {
    let mut value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|err| PrepareError::Invalid(err.to_string()))?;
    if !spelldata::book_ranks::rewrite_request(&mut value) {
        return Ok(None);
    }
    let bytes = serde_json::to_vec(&value).map_err(|err| PrepareError::Fault(err.to_string()))?;
    Request::from_json(&bytes)
        .map(Some)
        .map_err(PrepareError::Invalid)
}

/// Prepares a request into the prepared v2 contract.
pub fn prepare(request: &[u8], scenario: &str) -> Result<PreparedV2, PrepareError> {
    let value = prepare_json(request, scenario)?;
    serde_json::from_value(value)
        .map_err(|err| PrepareError::Invalid(format!("prepared output: {err}")))
}
