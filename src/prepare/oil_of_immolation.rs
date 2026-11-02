//! Oil of Immolation, a MythicSim Rust-only consumable beyond the Go reference pin (client
//! 1.60.1.70707): the request's `oilOfImmolation` flag has the character use item 8956 half a
//! second before the pull, so that its Fire Shield is up from the pull, and again each time it
//! ends.
//!
//! The pinned game data (`data/spells.jsonl`) holds neither spell row, so the values below are
//! the client's, restated as literals the way Go restates the Goblin Sapper's:
//!
//! - Item 8956 (required level 31) uses spell 11350 "Fire Shield": a 15 s aura (duration index 8)
//!   with a periodic trigger spell effect every 3 s, which casts spell 11351 on the user. It has
//!   no cooldown of its own and no global cooldown, and its category (SpellCategoryID 79, 3 s)
//!   is not the potion category, so it shares nothing with the potions.
//! - Spell 11351 "Fire Shield" is a Fire school school damage effect of 50 base points (a flat
//!   50, no spell power coefficient) on every enemy within its radius of the caster.
//!
//! The two durations below are not client rows but the players' report (Bae, a Classic player on
//! the project's Discord): using the oil while a swing timer is running stalls the auto attack
//! swing for 0.5 s, so players use it 0.5 s before the pull, where no swing is running yet.

use serde_json::{json, Value};

use crate::contracts::prepared_v2::ActionId;

use super::env::Environment;
use super::export_items::spell_position;
use super::sim::{AuraConfig, Duration, SECOND};
use super::spell::{school, DefenseType, ProcMask, SpellConfig, SpellFlag};

/// Item 8956 "Oil of Immolation".
pub(crate) const ITEM: i32 = 8956;
/// Its use spell 11350, the aura's action.
pub(crate) const USE_SPELL: i32 = 11350;
/// The trigger spell 11351, the damage each period deals.
pub(crate) const TICK_SPELL: i32 = 11351;
/// The aura's label.
pub(crate) const AURA: &str = "Oil of Immolation";
/// The aura's duration: spell 11350's duration index 8.
const DURATION: Duration = 15 * SECOND;
/// The aura's period: the periodic trigger effect's amplitude.
const PERIOD: Duration = 3 * SECOND;
/// The tick's flat Fire damage: spell 11351's base points.
const DAMAGE: f64 = 50.0;
/// How far a use pushes back each running melee swing: "0.5 sec swingtime stunt when using it
/// during swings".
const SWING_DELAY: Duration = SECOND / 2;
/// How long before the pull the first use happens, so that it costs no swing time: the same half
/// second.
const PREPOT: Duration = SECOND / 2;
/// The tick's radius in yards around the user: the radius of spell 11351's area effect.
const RADIUS: f64 = 5.0;

/// `oilOfImmolation`: the aura the oil's use applies and the spell its period casts.
pub(crate) fn register_oil_of_immolation(env: &mut Environment) {
    let unit = env.player;
    env.sim.register_spell(
        unit,
        SpellConfig {
            action_id: ActionId::spell(TICK_SPELL),
            spell_school: school::FIRE,
            defense_type: DefenseType::Magic,
            // A triggered spell: it raises no proc of its own.
            proc_mask: ProcMask::EMPTY,
            flags: SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::PASSIVE_SPELL,
            damage_multiplier: 1.0,
            threat_multiplier: 1.0,
            ..SpellConfig::default()
        },
    );
    env.sim.register_aura(
        unit,
        AuraConfig {
            label: AURA.to_string(),
            action_id: Some(ActionId::spell(USE_SPELL)),
            duration: DURATION,
            ..AuraConfig::default()
        },
    );
}

/// The `oil_of_immolation` effect when the character uses the oil: the aura, the tick spell by
/// spellbook position and the literals above.
pub(crate) fn oil_of_immolation_effect(env: &Environment) -> Option<Value> {
    env.sim.get_aura(env.player, AURA)?;
    let spell = spell_position(env, &ActionId::spell(TICK_SPELL))?;
    Some(json!({
        "kind": "oil_of_immolation", "item_id": ITEM, "aura": AURA, "spell": spell,
        "period_ns": PERIOD, "ticks": DURATION / PERIOD, "radius": RADIUS, "damage": DAMAGE,
        "swing_delay_ns": SWING_DELAY, "prepot_ns": PREPOT,
    }))
}
