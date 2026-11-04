//! Class and spec integration tests.

mod dark_inspiration;
mod druid;
mod external_cooldown;
mod global_cooldown;
mod hunter;
mod item_effects;
mod item_swap;
mod mage;
mod mana_totems;
mod oil_of_immolation;
mod paladin;
#[path = "../support/paladin_113554.rs"]
mod paladin_113554;
mod priest;
mod proc_oil;
mod rogue;
#[path = "../support/rogue_113483.rs"]
mod rogue_113483;
mod rotation_targets;
mod rotation_values;
mod shaman;
#[path = "../support/shaman_layout.rs"]
mod shaman_layout;
mod warlock;
mod warrior;
#[path = "../support/warrior_trees_20261104.rs"]
mod warrior_trees_20261104;
mod weapon_procs;

/// Each refusal of a valid prepared input: its stable code and its reason.
fn refusal_codes(value: serde_json::Value) -> Vec<(&'static str, String)> {
    let prepared: forever_engine::contracts::prepared_v2::PreparedV2 =
        serde_json::from_value(value).unwrap();
    forever_engine::prepared_refusals(&prepared)
        .into_iter()
        .map(|refusal| (refusal.code, refusal.reason))
        .collect()
}

/// SHA-256 for the preparation goldens, the same code the request digest uses.
#[path = "../../src/contracts/request/sha256.rs"]
mod sha256;
