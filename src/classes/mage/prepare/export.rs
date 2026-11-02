//! The exporter's Mage part, tools/oracle-v2/mage.go: the client damage rows of the Mage's
//! spells (`mageDamageRows`) and the effects whose parameters Go keeps in closures
//! (`mageEffects`).

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::{json, Value};

use crate::contracts::prepared_v2::ActionId;
use crate::contracts::request::Message;
use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::dbcenums;
use crate::prepare::export::action_id;
use crate::prepare::hotfix_113554;
use crate::prepare::sim::{Duration, Rules, Sim, SpellId, UnitId, MILLISECOND, SECOND};
use crate::prepare::spell::DefenseType;
use crate::prepare::spelldata::{Ladder, Spell as Row};

use super::spell_data::spell_data;

/// Go `IceLanceFrozenMultiplier`: the 16 October 2026 beta notes cut it to +200% (was +300%),
/// so a frozen hit deals 3 times an unfrozen one.
const ICE_LANCE_FROZEN_MULTIPLIER: f64 = 3.0;

/// `mageDamageRows`: the rows by spell ID.
fn damage_rows() -> &'static HashMap<i32, &'static Row> {
    static ROWS: OnceLock<HashMap<i32, &'static Row>> = OnceLock::new();
    ROWS.get_or_init(|| {
        let data = spell_data();
        let mut rows: HashMap<i32, &'static Row> = HashMap::new();
        let each = |rows: &mut HashMap<i32, &'static Row>, ladder: &Ladder| {
            ladder.each(|_, row| {
                rows.insert(row.id, row);
            });
        };
        let highest = |rows: &mut HashMap<i32, &'static Row>, ladder: &Ladder| {
            let row = ladder.highest();
            if !row.is_nil() {
                rows.insert(row.id, row);
            }
        };
        each(&mut rows, &data.frostbolt);
        each(&mut rows, &data.arcane_missiles_triggered);
        highest(&mut rows, &data.ice_lance);
        highest(&mut rows, &data.arcane_blast);
        highest(&mut rows, &data.arcane_wrath);
        highest(&mut rows, &data.fire_blast);
        each(&mut rows, &data.scorch);
        each(&mut rows, &data.fireball);
        each(&mut rows, &data.frostfire_bolt);
        highest(&mut rows, &data.pyroblast);
        for ladder in [
            &data.arcane_explosion,
            &data.blast_wave,
            &data.cone_of_cold,
            &data.frost_nova,
        ] {
            highest(&mut rows, ladder);
        }
        each(&mut rows, &data.flamestrike_ranks);
        rows
    })
}

/// Go `attachDamageEffects` for one spell: the roll of the client row, `{average, variance}`.
pub(super) fn damage_effect(sim: &Sim, spell: SpellId) -> Option<Value> {
    let action = &sim.spell(spell).action_id;
    if action.spell_id == 0 || action.tag != 0 {
        return None;
    }
    let row = match damage_rows().get(&action.spell_id) {
        Some(row) => *row,
        // Frost Blast (patch 153) rolls Frostbolt's way, but Go's exporter lists no row for it,
        // so only Forever's rules describe it.
        None if sim.rules == Rules::Forever => frost_blast_row(action.spell_id)?,
        None => return None,
    };
    let effect = row.damage_effect();
    if effect.is_nil() {
        // Go's exporter stops on a row without a damage effect; there is nothing to describe.
        return None;
    }
    Some(json!({"average": effect.average(CHARACTER_LEVEL), "variance": effect.variance}))
}

/// A Frost Blast rank's client row.
fn frost_blast_row(id: i32) -> Option<&'static Row> {
    let mut found = None;
    spell_data().frost_blast.each(|_, row| {
        if row.id == id {
            found = Some(row);
        }
    });
    found
}

fn nanos(d: Duration) -> i64 {
    d
}

/// A periodic effect that can crit on the row's own crit table: Go's `tick_can_crit`.
fn tick_can_crit(row: &Row) -> bool {
    row.periodic_can_crit() && row.defense_type_core() == DefenseType::Magic
}

/// `[{spell_id, tick_base, tick_can_crit}]` over a ladder's ranks, as Fireball's shape.
fn dot_ranks(ladder: &Ladder) -> Vec<Value> {
    let mut ranks = Vec::new();
    ladder.each(|_, row| {
        ranks.push(json!({
            "spell_id": row.id,
            "tick_base": row.periodic_effect().average(CHARACTER_LEVEL),
            "tick_can_crit": tick_can_crit(row),
        }));
    });
    ranks
}

/// Go `mageEffects`.
pub(super) fn effects(talents: &Message, sim: &Sim, unit: UnitId) -> Vec<Value> {
    let data = spell_data();
    let mut effects: Vec<Value> = Vec::new();

    // talents_arcane.go registerArcaneConcentration
    let arcane_concentration = talents.i32("arcane_concentration");
    if arcane_concentration > 0 {
        effects.push(json!({
            "kind": "arcane_concentration", "talent_rank": arcane_concentration,
            "proc_chance": data.arcane_concentration.effect_at(1).fraction_at(arcane_concentration),
            "icd_ns": nanos(data.arcane_concentration.highest().icd()),
            "trigger_aura": "Arcane Concentration",
            "aura": "Clearcasting",
            "aura_duration_ns": nanos(data.arcane_concentration_triggered.highest().duration()),
        }));
    }
    // talents_arcane.go registerMissileBarrage: the client rows; Arcane Wrath, Frost Blast and the
    // bolts proc at half of Arcane Blast's chance, and the missiles no longer proc it (patch 175,
    // the 19 October in-game tooltip; client 70425 gave them a quarter).
    let missile_barrage = talents.i32("missile_barrage");
    if missile_barrage > 0 {
        let buff = data.missile_barrage_triggered.highest();
        let arcane_blast_chance = data
            .missile_barrage
            .effect(dbcenums::A_PROC_TRIGGER_SPELL, 0)
            .value_at(missile_barrage)
            / 100.0;
        effects.push(json!({
            "kind": "missile_barrage", "trigger_aura": "Missile Barrage Trigger",
            "aura": "Missile Barrage",
            "arcane_blast_chance": arcane_blast_chance,
            "missile_chance": 0.0,
            "bolt_chance": arcane_blast_chance / 2.0,
            "rng_label": "Missile Barrage",
            "cost_percent_add": buff
                .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_COST)
                .average(CHARACTER_LEVEL)
                / 100.0,
            "tick_length_delta_ns": nanos(
                (buff
                    .effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_ACTIVATION_TIME)
                    .average(CHARACTER_LEVEL) as i64)
                    .wrapping_mul(MILLISECOND)
            ),
        }));
    }
    // talents_frost.go registerFingersOfFrost
    let fingers = talents.i32("fingers_of_frost");
    if fingers > 0 {
        let shatter = talents.i32("shatter");
        effects.push(json!({
            "kind": "fingers_of_frost", "talent_rank": fingers, "shatter_rank": shatter,
            "proc_chance": data.fingers_of_frost.effect_at(2).fraction_at(fingers),
            "max_stacks": data.fingers_of_frost.effect_at(1).value_at(fingers) as i32,
            "shatter_crit": data.shatter.value_at(shatter),
            "duration_ns": nanos(data.fingers_of_frost_triggered.highest().duration()),
            "trigger_aura": "Fingers of Frost Trigger", "aura": "Fingers of Frost",
        }));
    }
    // talents_frost.go registerWinterChill
    let winters_chill = talents.i32("winters_chill");
    if winters_chill > 0 {
        let trigger = hotfix_113554::row(sim.rules, data.winters_chill_triggered.highest().id);
        let mut effect = json!({
            "kind": "winters_chill", "talent_rank": winters_chill,
            "proc_chance": data.winters_chill.effect_at(2).fraction_at(winters_chill),
            "max_stacks": data.winters_chill.effect_at(1).value_at(winters_chill) as i32,
            "damage_per_stack": trigger.effect_n(1).percent(),
            "duration_ns": nanos(trigger.duration()),
            "trigger_aura": "Winters Chill Talent", "aura": "Winter's Chill",
        });
        // Client 1.60.1.70630 (SpellEffect 11180 effect 3 and 12579 effect 2, +7 each; 11180
        // SpellAuraOptions ProcCharges 1; 12579 ProcTypeMask 131072): the stacks are "your next
        // Cone of Cold", which they make deal 7% more of the mage's spell damage each, and only
        // Frostbolt, Frost Blast and an empowered Ice Lance apply them. The reference keeps
        // the 70425 rule: any landed Frost damage, stacks that stay.
        if sim.rules == Rules::Forever {
            effect["spell_power_per_stack"] =
                json!(data.winters_chill.effect_at(3).fraction_at(winters_chill));
            effect["cone_of_cold_consumes"] = json!(true);
        }
        effects.push(effect);
    }
    // ice_lance.go
    if talents.bool("ice_lance") {
        // Hotfix push 113531 raises the Frozen bonus to 300% (the rank's second effect) under
        // Forever's rules, so a frozen hit deals 4 times an unfrozen one.
        let id = data.ice_lance.highest().id;
        let frozen_multiplier = match hotfix_113554::hotfixed(id) {
            Some(row) if sim.rules == Rules::Forever => 1.0 + row.effect_n(2).percent(),
            _ => ICE_LANCE_FROZEN_MULTIPLIER,
        };
        effects.push(json!({
            "kind": "ice_lance", "spell_id": id,
            "frozen_multiplier": frozen_multiplier,
        }));
    }
    // cold_snap.go
    if talents.bool("cold_snap") {
        effects.push(json!({"kind": "cold_snap", "spell_id": data.cold_snap.highest().id}));
    }
    // ice_barrier.go (the fork's patch 187): the shield's absorb, read when it is cast, and the
    // Frost damage both auras attach in Piercing Ice's old shape.
    if talents.bool("ice_barrier") {
        let rank = hotfix_113554::row(sim.rules, data.ice_barrier.highest().id);
        let frigid = hotfix_113554::row(sim.rules, data.ice_barrier_triggered.by_id(1326891).id);
        let pct = |row: &Row, misc: i32| {
            row.effect(dbcenums::A_ADD_PCT_MODIFIER, misc)
                .average(CHARACTER_LEVEL)
                / 100.0
        };
        effects.push(json!({
            "kind": "ice_barrier", "spell_id": rank.id, "aura": "Ice Barrier",
            "frigid_power_aura": frigid.name,
            "absorb_base": rank.effect_n(1).average(CHARACTER_LEVEL),
            "warding": 1.0 + data.frost_warding.effect_at(3).fraction_at(talents.i32("frost_warding")),
            "hit": pct(rank, dbcenums::SPELLMOD_DAMAGE), "dot": pct(rank, dbcenums::SPELLMOD_DOT),
            "frigid_hit": pct(frigid, dbcenums::SPELLMOD_DAMAGE),
            "frigid_dot": pct(frigid, dbcenums::SPELLMOD_DOT),
        }));
    }
    // evocation.go
    let evocation = data.evocation.highest();
    effects.push(json!({
        "kind": "evocation", "spell_id": evocation.id, "regen_aura": "Evocation Regen",
        "channel_aura": "Evocation",
        "regen_multiplier": evocation
            .effect(dbcenums::A_MOD_POWER_REGEN_PERCENT, 0)
            .average(CHARACTER_LEVEL)
            / 100.0,
    }));
    // arcane_missiles.go: channel rank N fires tick rank N.
    let mut missiles = Vec::new();
    data.arcane_missiles.each(|rank, row| {
        missiles.push(json!({
            "channel_spell_id": row.id,
            "tick_spell_id": data.arcane_missiles_triggered.rank(rank).id,
        }));
    });
    effects.push(json!({
        "kind": "arcane_missiles", "ranks": missiles,
    }));
    // frostbolt.go
    effects.push(json!({"kind": "frostbolt"}));
    // arcane_blast.go and arcane_charge.go: the stacks' spell damage from Intellect is a stat aura.
    if talents.bool("arcane_blast") {
        let buff = hotfix_113554::row(sim.rules, data.arcane_blast_triggered.highest().id);
        effects.push(json!({
            "kind": "arcane_blast", "spell_id": data.arcane_blast.highest().id,
            "aura": "Arcane Blast",
            "intellect_per_stack": buff.effect_n(1).percent(),
            "cost_per_stack": buff
                .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_COST)
                .average(CHARACTER_LEVEL)
                / 100.0,
        }));
    }
    // arcane_wrath.go: the hit adds a share of maximum mana as spell damage at its coefficient.
    if talents.bool("arcane_wrath") {
        let rank = data.arcane_wrath.highest();
        effects.push(json!({
            "kind": "arcane_wrath", "spell_id": rank.id,
            "coefficient": rank.damage_effect().coeff(),
            "mana_share": rank.effect_n(2).base_points / 100.0,
        }));
    }
    // talents_fire.go registerFirePower: a damage done by caster handler on every attack table.
    let fire_power = talents.i32("fire_power");
    if fire_power > 0 {
        effects.push(json!({
            "kind": "fire_power", "max_effects": 4,
            "per_effect": data.fire_power.effect_at(1).fraction_at(fire_power),
        }));
    }
    // fire_blast.go
    effects.push(json!({"kind": "fire_blast"}));
    // fireball.go: every rank; the hit lands after travel, then its dot snapshots and ticks.
    effects.push(json!({"kind": "fireball", "ranks": dot_ranks(&data.fireball)}));
    // frostfire_bolt.go: Fireball's shape with a Frostfire school, every rank.
    effects.push(json!({"kind": "frostfire_bolt", "ranks": dot_ranks(&data.frostfire_bolt)}));
    // scorch.go: every rank; Improved Scorch stacks Fire Vulnerability on the mage itself.
    let mut scorch = json!({"kind": "scorch"});
    let improved_scorch = talents.i32("improved_scorch");
    if improved_scorch > 0 {
        scorch["improved_scorch"] = json!({
            "aura": "Fire Vulnerability",
            "proc_chance": data.improved_scorch.fraction_at(improved_scorch),
            "damage_per_stack": data.fire_vulnerability.highest().effect_n(1).average(CHARACTER_LEVEL) / 100.0,
        });
    }
    effects.push(scorch);
    // pyroblast.go: Fireball's shape, highest rank only
    if talents.bool("pyroblast") {
        let row = data.pyroblast.highest();
        effects.push(json!({
            "kind": "pyroblast", "spell_id": row.id,
            "tick_base": row.periodic_effect().average(CHARACTER_LEVEL),
            "tick_can_crit": tick_can_crit(row),
        }));
    }
    // combustion.go
    if talents.bool("combustion") {
        let buff = data.combustion_triggered.highest();
        effects.push(json!({
            "kind": "combustion", "spell_id": data.combustion.highest().id, "aura": "Combustion",
            "crit_per_stack": buff
                .effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_CRITICAL_CHANCE)
                .average(CHARACTER_LEVEL),
            "max_crits": i32::from(data.combustion.highest().proc_charges),
        }));
    }
    // talents_fire.go registerHotStreak
    if talents.bool("heating_up") {
        let buff = data.heating_up_triggered.highest();
        effects.push(json!({
            "kind": "heating_up", "aura": "Heating Up", "trigger_aura": "Heating Up Trigger",
            "fire_blast_trigger_aura": "Hot Streak Fire Blast Trigger",
            "cast_time_per_stack": buff
                .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_CASTING_TIME)
                .percent(),
        }));
    }
    // talents_fire.go registerMasterOfElements
    let master_of_elements = talents.i32("master_of_elements");
    if master_of_elements > 0 {
        effects.push(json!({
            "kind": "master_of_elements", "trigger_aura": "Master of Elements",
            "refund": data.master_of_elements.fraction_at(master_of_elements),
            "metrics_action_id": action_id(Some(&ActionId {
                spell_id: data.master_of_elements.highest().id,
                ..ActionId::default()
            })),
        }));
    }
    // talents_fire.go registerIgnite: fire spell crits feed a dot
    let ignite = talents.i32("ignite");
    if ignite > 0 {
        effects.push(json!({
            "kind": "ignite", "trigger_aura": "Ignite Talent",
            "spell_id": data.ignite_triggered.highest().id,
            "share": data.ignite.fraction_at(ignite),
            "num_ticks": (data.ignite_triggered.highest().duration() / (2 * SECOND)) as i32,
            // Forever's talent row lacks the bit, so a spell flagged Proc cannot trigger it.
            "can_proc_from_procs": data.ignite.highest().can_proc_from_procs(),
        }));
    }
    // arcane_power.go
    if talents.bool("arcane_power") {
        let rank = data.arcane_power.highest();
        effects.push(json!({
            "kind": "arcane_power", "spell_id": rank.id, "aura": "Arcane Power",
            "damage": rank
                .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_DAMAGE)
                .average(CHARACTER_LEVEL)
                / 100.0,
            "cost_percent_add": rank
                .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_COST)
                .average(CHARACTER_LEVEL)
                / 100.0,
        }));
    }
    // presence_of_mind.go
    if talents.bool("presence_of_mind") {
        let rank = data.presence_of_mind.highest();
        effects.push(json!({
            "kind": "presence_of_mind", "spell_id": rank.id, "aura": "Presence of Mind",
            "cast_time_percent": rank
                .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_CASTING_TIME)
                .average(CHARACTER_LEVEL)
                / 100.0,
        }));
    }
    // arcane_explosion.go, cone_of_cold.go and frost_nova.go: a rolled hit on each target.
    effects.push(json!({"kind": "arcane_explosion"}));
    effects.push(json!({"kind": "cone_of_cold"}));
    effects.push(json!({"kind": "frost_nova"}));
    // blast_wave.go
    if talents.bool("blast_wave") {
        effects.push(json!({"kind": "blast_wave"}));
    }
    // flamestrike.go: a rolled hit, then an area dot whose ticks deal the triggered spell's
    // amount of the same rank.
    let mut flamestrikes = Vec::new();
    data.flamestrike_ranks.each(|_, row| {
        flamestrikes.push(json!({
            "spell_id": row.id,
            "tick_base": data
                .flamestrike_triggered
                .rank(row.rank_number())
                .damage_effect()
                .average(CHARACTER_LEVEL),
        }));
    });
    effects.push(json!({"kind": "flamestrike", "ranks": flamestrikes}));
    // blizzard.go: the channel casts the triggered tick of its rank every period; with
    // Improved Blizzard each landed tick casts the chill.
    let blizzard = data.blizzard.highest();
    let tick = data.blizzard_triggered.rank(blizzard.rank_number());
    let mut blizzard_effect = json!({
        "kind": "blizzard", "spell_id": blizzard.id, "tick_spell_id": tick.id,
        "tick_base": tick.damage_effect().average(CHARACTER_LEVEL),
    });
    if talents.i32("improved_blizzard") > 0 {
        blizzard_effect["improved_blizzard_spell_id"] =
            json!(data.improved_blizzard_triggered.highest().id);
    }
    effects.push(blizzard_effect);
    // mana_gems.go: smaller gems wait for larger ones; all share the conjured cooldown.
    let gems: Vec<Value> = [
        (5514, data.conjure_mana_agate_triggered.highest()),
        (5513, data.conjure_mana_jade_triggered.highest()),
        (8007, data.conjure_mana_citrine_triggered.highest()),
        (8008, data.conjure_mana_ruby_triggered.highest()),
    ]
    .into_iter()
    .map(|(item, row)| {
        json!({"item_id": item, "mana": row.energize_effect().average(CHARACTER_LEVEL)})
    })
    .collect();
    effects.push(json!({"kind": "mana_gems", "gems": gems, "regen_window_seconds": 2.0}));
    // armors.go: the regen part is already in the prepared pseudo stats.
    if sim.get_aura(unit, "Mage Armor").is_some() {
        effects.push(json!({"kind": "mage_armor", "aura": "Mage Armor"}));
    }
    effects
}
