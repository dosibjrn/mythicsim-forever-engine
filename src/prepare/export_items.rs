//! The exporter's descriptions of item and enchant effects: tools/oracle-v2 `gear_procs.go`,
//! `item_procs.go`, `energy_procs.go`, `damage_on_use.go`, `melee_procs.go` and the item parts of
//! `main.go` (`meleeItemListeners`, `hitTakenItemListeners`, `survivalOnUse`, `speedOnUse`,
//! `simpleStatActive`, `onlyOnUseEffect`).
//!
//! Each function reads the registered auras and spells the way the Go exporter reads them, and
//! restates the formulas the runtime executes. A shape it cannot describe is noted in
//! `unrepresented`, which refuses the request.

use serde_json::{json, Value};

use crate::contracts::prepared_v2::ActionId;
use crate::contracts::request::Message;
use crate::data::spells::Spell;

use super::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use super::common_effects::{action_id_string, active_stats, flat_string, SPELL_BATCH_WINDOW};
use super::consumable_effects::proc_trigger_spells;
use super::dbcenums;
use super::env::Environment;
use super::items::{self, effect_stats};
use super::procs::DynamicProcManager;
use super::resolve_proc::{item_proc_chance, proc_trigger};
use super::shared_auras::in_area;
use super::sim::{AuraId, SpellId, UnitId, SECOND};
use super::spell::{school, DefenseType, ProcMask, SpellFlag};
use super::spelldata::{effect::AURA_ON_ENEMY, effect::AURA_ON_PET, find, must_find};
use super::stats::{Stat, Stats};

// ---------------------------------------------------------------------------------------------
// Helpers shared by the descriptions.
// ---------------------------------------------------------------------------------------------

/// druid.go `callbackNames`: the names of the callbacks a trigger listens to, in Go's order.
pub(crate) fn callback_names(callback: CallbackMask) -> Vec<&'static str> {
    [
        (CallbackMask::ON_SPELL_HIT_DEALT, "on_spell_hit_dealt"),
        (CallbackMask::ON_SPELL_HIT_TAKEN, "on_spell_hit_taken"),
        (
            CallbackMask::ON_PERIODIC_DAMAGE_DEALT,
            "on_periodic_damage_dealt",
        ),
        (CallbackMask::ON_HEAL_DEALT, "on_heal_dealt"),
        (
            CallbackMask::ON_PERIODIC_HEAL_DEALT,
            "on_periodic_heal_dealt",
        ),
        (CallbackMask::ON_CAST_COMPLETE, "on_cast_complete"),
        (CallbackMask::ON_APPLY_EFFECTS, "on_apply_effects"),
        (
            CallbackMask::ON_PERIODIC_DAMAGE_TAKEN,
            "on_periodic_damage_taken",
        ),
    ]
    .into_iter()
    .filter(|(mask, _)| callback.matches(*mask))
    .map(|(_, name)| name)
    .collect()
}

/// Go `HitOutcome.String` of one bit.
fn outcome_name(flag: HitOutcome) -> &'static str {
    if flag.matches(HitOutcome::MISS) {
        "Miss"
    } else if flag.matches(HitOutcome::DODGE) {
        "Dodge"
    } else if flag.matches(HitOutcome::PARRY) {
        "Parry"
    } else if flag.matches(HitOutcome::BLOCK) && flag.matches(HitOutcome::CRIT) {
        "BlockedCrit"
    } else if flag.matches(HitOutcome::BLOCK) {
        "Block"
    } else if flag.matches(HitOutcome::GLANCE) {
        "Glance"
    } else if flag.matches(HitOutcome::CRIT) {
        "Crit"
    } else if flag.matches(HitOutcome::HIT) {
        "Hit"
    } else if flag.matches(HitOutcome::CRUSH) {
        "Crush"
    } else {
        "Empty"
    }
}

/// druid.go `outcomeNames`: the name of each bit of the outcome.
pub(crate) fn outcome_names(outcome: HitOutcome) -> Vec<&'static str> {
    (0..32)
        .map(|bit| HitOutcome(1u16.checked_shl(bit).unwrap_or(0)))
        .filter(|flag| flag.0 != 0 && outcome.0 & flag.0 != 0)
        .map(outcome_name)
        .collect()
}

/// melee_procs.go `dpmChances`: the chance a dynamic proc manager rolls for each spell it hears,
/// taking the first mask entry the spell's proc mask matches.
pub(crate) fn dpm_chances(
    env: &Environment,
    dpm: &DynamicProcManager,
    eligible: impl Fn(&super::spell::Spell) -> bool,
) -> Vec<Value> {
    let mut chances = Vec::new();
    for (position, spell) in env.sim.unit(env.player).spellbook.iter().enumerate() {
        let spell = env.sim.spell(*spell);
        if !eligible(spell) {
            continue;
        }
        for (mask, chance) in dpm.proc_masks.iter().zip(&dpm.proc_chances) {
            if mask.matches(spell.proc_mask) {
                chances.push(json!({"spell": position, "chance": chance}));
                break;
            }
        }
    }
    chances
}

/// The position of the last spellbook entry with this action: the exporter's loops keep
/// overwriting the position, and `-1` where there is none.
pub(crate) fn spell_position(env: &Environment, action: &ActionId) -> Option<usize> {
    let mut found = None;
    for (position, spell) in env.sim.unit(env.player).spellbook.iter().enumerate() {
        if &env.sim.spell(*spell).action_id == action {
            found = Some(position);
        }
    }
    found
}

/// The spell at a spellbook position.
fn spell_at(env: &Environment, position: usize) -> SpellId {
    env.sim.unit(env.player).spellbook[position]
}

fn trigger_for(env: &Environment, row: &'static Spell, with_item_chance: bool) -> ProcTrigger {
    let opts = if with_item_chance {
        vec![item_proc_chance(row)]
    } else {
        Vec::new()
    };
    proc_trigger(&env.sim, Some(env.player), row, &opts)
}

/// An unset chance reads as certain: `AttachProcTriggerCallback`.
fn chance_or_certain(listener: &ProcTrigger) -> f64 {
    if listener.proc_chance == 0.0 {
        1.0
    } else {
        listener.proc_chance
    }
}

pub(crate) fn aura_named(env: &Environment, label: &str) -> Option<AuraId> {
    env.sim.get_aura(env.player, label)
}

/// A proc mask is a melee hit taken or dealt.
const STRUCK_PROC_MASK: ProcMask = ProcMask::MELEE_OR_RANGED;

/// The Goblin Sapper Charge's hit on its thrower, when the wearer throws one: the one spell that
/// damages the player.
fn sapper_self_hit(env: &Environment) -> Option<SpellId> {
    env.sim
        .unit(env.player)
        .spellbook
        .iter()
        .find(|spell| {
            let id = &env.sim.spell(**spell).action_id;
            id.item_id == super::common_effects::GOBLIN_SAPPER_ITEM && id.tag == 1
        })
        .copied()
}

/// melee_procs.go `hearsHitsTaken`: whether a listener is shaped to hear the melee hits the wearer
/// takes, which the target's swings are: its mask names melee and ranged hits and may name spell
/// damage.
fn hears_hits_taken(listener: &ProcTrigger) -> bool {
    let mask = STRUCK_PROC_MASK.0 | ProcMask::SPELL_DAMAGE.0;
    callback_names(listener.callback) == ["on_spell_hit_taken"]
        && listener.proc_mask.0 & !mask == 0
        && listener.proc_mask.matches(ProcMask::MELEE_MH_AUTO)
        && !listener.can_proc_from_procs
        && !listener.is_weapon_proc
        && listener.class_spell_mask == 0
        && listener.spell_flags == SpellFlag::NONE
        && listener.proc_mask_exclude == ProcMask::UNKNOWN
}

/// melee_procs.go `hearsTheSappersHit`: the spellbook position of the Goblin Sapper Charge's hit
/// on its thrower when a listener of hits taken hears it, its mask and flags matching the spell.
fn hears_the_sappers_hit(env: &Environment, listener: &ProcTrigger) -> Option<usize> {
    let spell = sapper_self_hit(env)?;
    if callback_names(listener.callback) != ["on_spell_hit_taken"] {
        return None;
    }
    proc_trigger_spells(env, listener)
        .into_iter()
        .find(|position| env.sim.unit(env.player).spellbook[*position] == spell)
}

/// melee_procs.go `hearsTheTargetsSwings`: whether a listener hears the melee hits the wearer
/// takes, which the target's swings are, and no others. Its mask may name spell damage only when
/// the wearer throws no Goblin Sapper Charge, whose hit on the thrower is the one spell that
/// damages the player.
fn hears_the_targets_swings(env: &Environment, listener: &ProcTrigger) -> bool {
    if sapper_self_hit(env).is_some() && listener.proc_mask.matches(ProcMask::SPELL_DAMAGE) {
        return false;
    }
    hears_hits_taken(listener)
}

/// Go `Encounter.ActiveTargetCount` with every target active.
fn active_target_count(env: &Environment) -> usize {
    env.encounter.targets.len()
}

/// Go `Encounter.AOECapMultiplier` with every target active.
fn aoe_cap_multiplier(env: &Environment) -> f64 {
    (20.0 / env.encounter.targets.len() as f64).min(1.0)
}

/// Go `spell.Dot(target)`: the dot of the spell or of its related dot spell on the unit.
fn spell_dot(env: &Environment, spell: SpellId, target: UnitId) -> Option<super::sim::DotId> {
    let s = env.sim.spell(spell);
    if s.dots.is_empty() {
        return s
            .related_dot_spell
            .and_then(|related| spell_dot(env, related, target));
    }
    let index = env.sim.unit(target).unit_index as usize;
    s.dots.get(index).copied().flatten()
}

fn has_mana_bar(env: &Environment) -> bool {
    env.sim.unit(env.player).mana_bar.enabled
}

// ---------------------------------------------------------------------------------------------
// main.go: the item uses.
// ---------------------------------------------------------------------------------------------

/// Items whose use registers temporary stats with `RegisterTemporaryStatsOnUseCD`, by the Go
/// literal stats each passes (common/classic/items_weapons.go): Headmaster's Charge.
fn temporary_stat_items(item: i32) -> Option<Stats> {
    match item {
        13937 => Some(Stats::from_pairs(&[(Stat::Intellect, 20.0)])),
        _ => None,
    }
}

/// main.go `onlyOnUseEffect`: the item's one use effect, or none.
pub(crate) fn only_on_use_effect(item: i32) -> Option<Message> {
    if item == 0 {
        return None;
    }
    let db_item = items::database_item(item)?;
    let mut on_use: Option<Message> = None;
    for effect in db_item.item_effects {
        if !effect.has("on_use") {
            continue;
        }
        if on_use.is_some() {
            return None;
        }
        on_use = Some(effect);
    }
    on_use
}

fn related_self_buff(env: &Environment, spell: SpellId) -> Option<AuraId> {
    env.sim.spell(spell).related_self_buff
}

/// main.go `speedOnUse`: shared.NewSpellDataSpeedOnUse: the use activates the buff row's aura,
/// its self buff, which multiplies melee, ranged and cast speed by the row's haste percents while
/// up. Nil for any other item use.
fn speed_on_use(env: &Environment, spell: SpellId) -> Option<Value> {
    let item = env.sim.spell(spell).action_id.item_id;
    let on_use = only_on_use_effect(item)?;
    let aura = related_self_buff(env, spell)?;
    let row = find(on_use.i32("buff_id"));
    if row.speed_effects().is_empty()
        || env.sim.aura(aura).label != super::resolve_aura::aura_config(row, &[]).label
    {
        return None;
    }
    let pseudo = row.speed_pseudo_stats();
    let multiplier = |stat: usize| 1.0 + pseudo.get(stat).copied().unwrap_or(0.0) / 100.0;
    use super::spelldata::speed::pseudo_stat;
    Some(json!({
        "kind": "speed_on_use", "item_id": item, "aura": env.sim.aura(aura).label,
        "melee_multiplier": multiplier(pseudo_stat::MELEE_HASTE_PERCENT),
        "ranged_multiplier": multiplier(pseudo_stat::RANGED_HASTE_PERCENT),
        "cast_multiplier": multiplier(pseudo_stat::SPELL_HASTE_PERCENT),
    }))
}

/// main.go `simpleStatActive`: shared.NewSimpleStatActive: an on-use item whose one use effect's
/// buff, its self buff, carries the effect's scaling stats times the buff row's area bonus. Nil
/// for any other item use.
pub(crate) fn simple_stat_active(env: &Environment, spell: SpellId) -> Option<Stats> {
    let item = env.sim.spell(spell).action_id.item_id;
    let aura = related_self_buff(env, spell)?;
    if temporary_stat_items(item).is_some() || speed_on_use(env, spell).is_some() {
        return None;
    }
    let on_use = only_on_use_effect(item)?;
    if on_use.str("buff_name") != env.sim.aura(aura).label {
        return None;
    }
    items::scaling_option(&on_use)?;
    let (amount, _) = find(on_use.i32("buff_id")).area_bonus(|area| in_area(env, area));
    Some(effect_stats(&on_use).multiply(amount))
}

/// main.go `survivalOnUse`: shared.NewSpellDataAbsorbOnUse and NewSpellDataHealOnUse: an item
/// use whose row shields the wearer against the schools its absorb effect masks, for the amount
/// the effect rolls, or heals it directly, a share of maximum health or a rolled amount.
fn survival_on_use(env: &Environment, spell: SpellId, item: i32) -> Option<Value> {
    let effect = only_on_use_effect(item)?;
    let row = find(effect.i32("buff_id"));
    if row.is_nil() {
        return None;
    }
    let player = env.player;
    let absorb = row.absorb_effect();
    if !absorb.is_nil() {
        if let Some(aura) = related_self_buff(env, spell) {
            return Some(json!({
                "kind": "absorb_on_use", "item_id": item, "aura": env.sim.aura(aura).label,
                "schools": absorb.misc, "average": absorb.average(env.sim.unit(player).level),
                "variance": absorb.variance,
            }));
        }
    }
    let heal = row.proc_heal_effect();
    if heal.is_nil()
        || heal.aura == dbcenums::A_PERIODIC_HEAL
        || related_self_buff(env, spell).is_some()
        || env.sim.spell(spell).bonus_coefficient != 0.0
    {
        return None;
    }
    let pseudo = &env.sim.unit(player).pseudo_stats;
    let mut exported = json!({
        "kind": "heal_on_use", "item_id": item, "can_crit": !row.cannot_crit(),
        "healing_dealt_multiplier": pseudo.healing_dealt_multiplier,
        "healing_taken_multiplier": pseudo.healing_taken_multiplier,
        "table_healing_dealt_multiplier": env.attack_table(player, player).healing_dealt_multiplier,
        "bonus_healing_taken": pseudo.bonus_healing_taken,
    });
    if heal.effect_type == dbcenums::E_HEAL_PCT {
        exported["max_health_share"] = json!(heal.percent());
    } else {
        exported["average"] = json!(heal.average(env.sim.unit(player).level));
        exported["variance"] = json!(heal.variance);
    }
    Some(exported)
}

/// The item cases of the item loop: temporary stats, speed, damage, survival and energize on
/// use, Burst of Knowledge and Second Wind, in Go's order. Class item uses follow through the
/// agent. `None` for an item none claims; an empty list for one claimed that exports nothing.
pub(crate) fn use_item_effect(
    env: &mut Environment,
    spell: SpellId,
    item: i32,
    unrepresented: &mut Vec<String>,
) -> Option<Vec<Value>> {
    // RegisterTemporaryStatsOnUseCD on an item: Go literal stats, or shared.NewSimpleStatActive's
    // on-use buff from the item's database effect.
    if temporary_stat_items(item).is_some() || simple_stat_active(env, spell).is_some() {
        let Some(aura) = related_self_buff(env, spell) else {
            unrepresented.push(format!("item {item} has no temporary stats aura"));
            return Some(Vec::new());
        };
        let bonus = temporary_stat_items(item).or_else(|| simple_stat_active(env, spell))?;
        let label = env.sim.aura(aura).label.clone();
        let aura_id = env.sim.aura(aura).action_id.clone().unwrap_or_default();
        return Some(vec![json!({
            "kind": "temporary_stats", "spell_id": 0, "item_id": item, "aura": label,
            "active_stats": active_stats(env, &label),
            "gain_log": format!("Gained {} from {}.", flat_string(&bonus), action_id_string(&aura_id)),
            "expire_log": format!("Lost {} from fading {}.", flat_string(&bonus), action_id_string(&aura_id)),
        })]);
    }
    if let Some(effect) = speed_on_use(env, spell) {
        return Some(vec![effect]);
    }
    if DAMAGE_ON_USE_ITEMS.contains(&item) {
        let effect = damage_on_use_effect(env, spell, unrepresented);
        return Some(if effect.is_null() {
            Vec::new()
        } else {
            vec![effect]
        });
    }
    // common/classic/items_trinkets.go Burst of Knowledge.
    if item == 11832 {
        return Some(
            match spell_cost_aura_on_use_effect(env, spell, "Burst of Knowledge") {
                Some(effect) => vec![effect],
                None => {
                    unrepresented.push("Burst of Knowledge has no aura".to_string());
                    Vec::new()
                }
            },
        );
    }
    if let Some(effect) = survival_on_use(env, spell, item) {
        return Some(vec![effect]);
    }
    // common/classic/items_trinkets.go Second Wind: Go literals, 63 mana a second for 10
    // seconds on its own metrics, and the automatic use waits for 630 mana missing.
    if item == 11819 && has_mana_bar(env) {
        return Some(vec![json!({
            "kind": "second_wind", "item_id": item, "mana": 63.0, "ticks": 10,
            "period_ns": SECOND, "metrics_spell_id": 15604, "min_deficit": 630.0,
        })]);
    }
    // A healer's on-use spell mods under Forever's rules.
    if let Some(effect) = super::healer_gear::use_item_effect(env, spell, item, unrepresented) {
        return Some(if effect.is_null() {
            Vec::new()
        } else {
            vec![effect]
        });
    }
    // A use effect a class file registers itself.
    if let Some(effect) = env.agent.class_item_use_effect(env, spell, item) {
        return Some(vec![effect]);
    }
    // Forever's rules: a heal over time on use (Orb of Soran'ruk) has no runtime behavior. Go
    // files it as a survival cooldown, which the automatic use never casts without timings or a
    // defensive health threshold; the gate refuses a rotation or a threshold that reaches it.
    if env.sim.rules == super::sim::Rules::Forever && periodic_heal_on_use(item) {
        return Some(Vec::new());
    }
    Some(energize_on_use_effects(env, spell, item, unrepresented))
}

/// Whether the item's one use effect is a client heal over time on the wearer.
fn periodic_heal_on_use(item: i32) -> bool {
    only_on_use_effect(item).is_some_and(|effect| {
        let heal = find(effect.i32("buff_id")).proc_heal_effect();
        !heal.is_nil() && heal.aura == dbcenums::A_PERIODIC_HEAL
    })
}

/// The default case of the item loop: shared.NewSpellDataEnergizeOnUse: an item use spell that
/// restores mana, at once or as a self hot of the row's ticks; the manager waits until the whole
/// gain fits. It has no self buff; an item whose use activates one registered another effect.
fn energize_on_use_effects(
    env: &Environment,
    spell: SpellId,
    item: i32,
    unrepresented: &mut Vec<String>,
) -> Vec<Value> {
    let mut effects = Vec::new();
    let mut found = false;
    if let Some(db_item) = items::database_item(item) {
        if related_self_buff(env, spell).is_none() {
            let level = env.sim.unit(env.player).level;
            for item_effect in &db_item.item_effects {
                if !item_effect.has("on_use") {
                    continue;
                }
                let row = find(item_effect.i32("buff_id"));
                let effect = row.proc_energize_effect();
                // Go `energizedBarOf`: mana, rage or energy, on a character with that bar.
                let unit = env.sim.unit(env.player);
                let power = match effect.misc {
                    _ if effect.is_nil() => continue,
                    dbcenums::POWER_MANA => None,
                    dbcenums::POWER_RAGE if unit.rage_bar.enabled => Some("rage"),
                    dbcenums::POWER_ENERGY if unit.energy_bar.enabled => Some("energy"),
                    _ => continue,
                };
                // Rage is stated in tenths.
                let scale = if dbcenums::power_in_tenths(effect.misc) {
                    0.1
                } else {
                    1.0
                };
                let periodic = effect.aura == dbcenums::A_PERIODIC_ENERGIZE;
                let mut ticks = 1.0;
                if periodic {
                    if env.sim.spell(spell).aoe_dot.is_none() {
                        continue;
                    }
                    ticks = f64::from(
                        super::resolve_aura::dot_config(row, effect, &[]).number_of_ticks,
                    );
                }
                found = true;
                let mut energize = json!({
                    "kind": "energize_on_use", "item_id": item, "spell_id": row.id,
                    "average": effect.average(level) * scale, "variance": effect.variance,
                    "whole": effect.max(level) * ticks * scale,
                });
                if periodic {
                    energize["periodic"] = json!(true);
                }
                if let Some(power) = power {
                    energize["power"] = json!(power);
                }
                effects.push(energize);
            }
        }
    }
    if !found {
        unrepresented.push(format!("major cooldown item {item} has no exported effect"));
    }
    effects
}

/// common/classic/items_trinkets.go Burst of Knowledge (11832): its use activates an aura whose
/// spell mod lowers the mana cost of spells by a flat amount. The changes are read from a
/// separate reset simulation with the aura active, by spellbook position.
fn spell_cost_aura_on_use_effect(env: &Environment, spell: SpellId, label: &str) -> Option<Value> {
    aura_named(env, label)?;
    let mut fresh = env.fresh();
    let player = fresh.player;
    let book = fresh.sim.unit(player).spellbook.clone();
    let flat =
        |sim: &super::sim::Sim, id: SpellId| sim.spell(id).cost.as_ref().map(|c| c.flat_modifier);
    let before: Vec<Option<i32>> = book.iter().map(|id| flat(&fresh.sim, *id)).collect();
    let aura = fresh.sim.get_aura(player, label)?;
    fresh.sim.activate(aura);
    let mut changes = Vec::new();
    for (i, id) in book.iter().enumerate() {
        if let Some(now) = flat(&fresh.sim, *id) {
            let was = before[i].unwrap_or(0);
            if now != was {
                changes.push(json!({"spell": i, "flat": now - was}));
            }
        }
    }
    Some(json!({
        "kind": "spell_cost_aura_on_use", "item_id": env.sim.spell(spell).action_id.item_id,
        "aura": label, "cost_changes": changes,
    }))
}

// ---------------------------------------------------------------------------------------------
// damage_on_use.go.
// ---------------------------------------------------------------------------------------------

/// common/forever/stat_bonus_cds_auto_gen.go: the on-use items shared.NewSpellDataDamageOnUse
/// registers.
const DAMAGE_ON_USE_ITEMS: [i32; 6] = [8348, 11905, 13171, 21891, 219345, 274759];

/// The outcome shared_utils.go `damageOutcome` picks for the defense type, by the runtime's name.
fn on_use_outcome(defense: DefenseType, cannot_crit: bool) -> &'static str {
    match (defense, cannot_crit) {
        (DefenseType::Melee, true) => "melee_special_hit",
        (DefenseType::Melee, false) => "melee_special_hit_and_crit",
        (DefenseType::Magic, true) => "magic_hit",
        (DefenseType::Magic, false) => "magic_hit_and_crit",
        _ => "",
    }
}

/// damage_on_use.go `damageOnUseEffect`: shared_utils.go `spellDataOnUseDamageSpell` on the one
/// target: the row's direct hit, rolled once and scaled as `calcMultiTargetDamage` scales it for
/// one target, then the damage over time it carries. A row it cannot describe is unrepresented
/// and returns null.
fn damage_on_use_effect(
    env: &Environment,
    spell: SpellId,
    unrepresented: &mut Vec<String>,
) -> Value {
    let s = env.sim.spell(spell);
    let item = s.action_id.item_id;
    if !DAMAGE_ON_USE_ITEMS.contains(&item) || s.action_id.spell_id != 0 {
        return Value::Null;
    }
    let position = env
        .sim
        .unit(env.player)
        .spellbook
        .iter()
        .rposition(|registered| *registered == spell);
    let on_use = only_on_use_effect(item);
    let (Some(on_use), Some(position)) = (on_use, position) else {
        unrepresented.push(format!(
            "damage on-use item {item} has no single on-use spell"
        ));
        return Value::Null;
    };
    let row = must_find(on_use.i32("buff_id"));
    let direct = row.damage_effect();
    let periodic = row.periodic_damage_effect();
    let defense = s.defense_type;
    let physical = s.spell_school & school::PHYSICAL != 0;
    let outcome = on_use_outcome(defense, row.cannot_crit());
    let target = env.encounter.targets[0];
    let dot = spell_dot(env, spell, target);
    let dot_coefficient = dot.map_or(0.0, |dot| env.sim.dots[dot.0].bonus_coefficient);
    let level = env.sim.unit(env.player).level;

    if outcome.is_empty()
        || !row.debuff_effects().is_empty()
        || (direct.is_nil() && periodic.is_nil())
    {
        unrepresented.push(format!(
            "damage on-use item {item}'s row is not a hit or a damage over time"
        ));
        return Value::Null;
    }
    if physical && (s.bonus_coefficient != 0.0 || (dot.is_some() && dot_coefficient != 0.0)) {
        unrepresented.push(format!(
            "damage on-use item {item} scales a physical hit with spell power"
        ));
        return Value::Null;
    }
    if !periodic.is_nil() && row.periodic_can_crit() && defense != DefenseType::Magic {
        unrepresented.push(format!(
            "damage on-use item {item}'s ticks roll a physical crit"
        ));
        return Value::Null;
    }
    if !periodic.is_nil() && dot.is_none() {
        unrepresented.push(format!("damage on-use item {item} has no dot"));
        return Value::Null;
    }
    let several = !direct.is_nil()
        && (direct.hits_an_area() || direct.chain_targets > 1)
        && active_target_count(env) > 1;
    if several && !periodic.is_nil() {
        unrepresented.push(format!(
            "damage on-use item {item} hits several targets and leaves a damage over time"
        ));
        return Value::Null;
    }
    let mut effect = json!({"kind": "damage_on_use", "item_id": item, "spell": position});
    if !direct.is_nil() {
        let mut scale = 1.0;
        if direct.hits_an_area() && row.max_targets == 0 && !row.splits_damage {
            scale = aoe_cap_multiplier(env);
        }
        let mut shape = json!({
            "average": direct.average(level), "variance": direct.variance,
            "scale": scale, "outcome": outcome,
        });
        // Past one target the spell is the proc spell of `proc_damage_shape`: an area calculated
        // on every target or the row's cap of them, or a chain, every hit before any is dealt.
        if several && direct.hits_an_area() {
            shape["area"] = json!({
                "max_targets": i32::from(row.max_targets), "splits": row.splits_damage,
                "aoe_cap_multiplier": aoe_cap_multiplier(env),
            });
        } else if several {
            shape["chain"] = json!({
                "targets": i32::from(direct.chain_targets),
                "amp": f64::from(direct.chain_amp),
            });
        }
        effect["direct"] = shape;
    }
    if !periodic.is_nil() {
        let mut ticks = json!({
            "tick_base": periodic.average(level), "tick_can_crit": row.periodic_can_crit(),
        });
        if direct.is_nil() {
            ticks["application_outcome"] = json!(on_use_outcome(defense, true));
        }
        effect["periodic"] = ticks;
    }
    effect
}

// ---------------------------------------------------------------------------------------------
// item_procs.go.
// ---------------------------------------------------------------------------------------------

/// Procs common/forever/stat_bonus_procs_auto_gen.go and enchants_auto_gen.go register with
/// shared.NewSpellDataProc: the trigger's name, its trigger and buff rows, and the item or
/// enchant whose effect entry states the buff's stats; or with shared.NewSpellDataAuraProc, whose
/// buff is the row's own parsed aura on the wearer.
struct SpellDataStatProc {
    label: &'static str,
    trigger: i32,
    buff: i32,
    item: i32,
    enchant: i32,
    parsed: bool,
}

const SPELL_DATA_STAT_PROCS: [SpellDataStatProc; 9] = [
    SpellDataStatProc {
        label: "Draconic Infused Emblem",
        trigger: 1318931,
        buff: 1318930,
        item: 22268,
        enchant: 0,
        parsed: false,
    },
    SpellDataStatProc {
        // Client 70425's generator names it after the stat it grants.
        label: "The Green Tower - NatureResistance - ",
        trigger: 18097,
        buff: 17154,
        item: 1204,
        enchant: 0,
        parsed: false,
    },
    SpellDataStatProc {
        label: "Wall of the Dead",
        trigger: 19409,
        buff: 18828,
        item: 1979,
        enchant: 0,
        parsed: false,
    },
    SpellDataStatProc {
        label: "Darkmoon Card: Blue Dragon",
        trigger: 23688,
        buff: 23684,
        item: 19288,
        enchant: 0,
        parsed: false,
    },
    SpellDataStatProc {
        label: "Wrath of Cenarius",
        trigger: 25906,
        buff: 25907,
        item: 21190,
        enchant: 0,
        parsed: false,
    },
    SpellDataStatProc {
        label: "Painwalker Buckler",
        trigger: 1293701,
        buff: 1293700,
        item: 274290,
        enchant: 0,
        parsed: false,
    },
    SpellDataStatProc {
        label: "Enchant Weapon - Grand Sorcerer",
        trigger: 1231163,
        buff: 1231162,
        item: 0,
        enchant: 7942,
        parsed: false,
    },
    SpellDataStatProc {
        label: "Enchant 2H Weapon - Grand Arcanist",
        trigger: 1231152,
        buff: 1231138,
        item: 0,
        enchant: 7941,
        parsed: false,
    },
    SpellDataStatProc {
        label: "Enchant Weapon - Insight",
        trigger: 1248758,
        buff: 1299796,
        item: 0,
        enchant: 8216,
        parsed: true,
    },
];

/// shared_utils.go applySpellDataProc names the buff it builds after the trigger.
fn spell_data_stat_proc_aura(label: &str) -> String {
    format!("{label} Proc")
}

/// The stat auras of the spell data procs the character wears, which `statAurasEffect` combines.
/// A proc of the hits the wearer takes joins them only when the wearer tanks the target, since
/// nothing else hits it.
pub(crate) fn spell_data_stat_proc_auras(env: &Environment) -> Vec<String> {
    SPELL_DATA_STAT_PROCS
        .iter()
        .filter(|proc| aura_named(env, proc.label).is_some())
        .filter(|proc| {
            let trigger = must_find(proc.trigger);
            let listener = trigger_for(env, trigger, true);
            listener.callback != CallbackMask::ON_SPELL_HIT_TAKEN
                || tanks_the_target(env)
                || hears_the_sappers_hit(env, &listener).is_some()
        })
        .map(|proc| spell_data_stat_proc_aura(proc.label))
        .collect()
}

/// main.go `tanksTheTarget`: whether the target swings at the wearer.
fn tanks_the_target(env: &Environment) -> bool {
    let target = env.encounter.targets[0];
    env.sim.unit(target).current_target == Some(env.player)
}

/// item_procs.go `spellDataStatProcEffects`: the listener its trigger row decodes to rolls the
/// stated chance under the trigger's name, behind the trigger aura's cooldown, and a spell batch
/// window later activates a temporary stats aura of the effect entry's stats. Only a proc without
/// a rate, stacks or charges, hearing landed or any hits, casts and heals, is described.
pub(crate) fn spell_data_stat_proc_effects(
    env: &Environment,
    unrepresented: &mut Vec<String>,
) -> Vec<Value> {
    let mut effects = Vec::new();
    for proc in &SPELL_DATA_STAT_PROCS {
        let Some(trigger_aura) = aura_named(env, proc.label) else {
            continue;
        };
        let proc_aura = aura_named(env, &spell_data_stat_proc_aura(proc.label));
        let trigger = must_find(proc.trigger);
        let buff = must_find(proc.buff);
        let entries: Vec<Message> = if proc.item != 0 {
            items::database_item(proc.item)
                .map(|item| item.item_effects)
                .unwrap_or_default()
        } else {
            items::enchant(proc.enchant)
                .map(|enchant| enchant.enchant_effects)
                .unwrap_or_default()
        };
        let mut entry: Option<Message> = entries
            .into_iter()
            .rfind(|candidate| candidate.i32("buff_id") == proc.buff);
        if proc.parsed {
            // The parsed aura needs no effect entry; one on a pet or an enemy is not described.
            entry = Some(Message::empty("proto.ItemEffect"));
            if !super::spelldata::effect::effects_on(buff, AURA_ON_PET).is_empty()
                || !super::spelldata::effect::effects_on(buff, AURA_ON_ENEMY).is_empty()
            {
                entry = None;
            }
        }
        let listener = trigger_for(env, trigger, true);
        let names = callback_names(listener.callback);
        // A listener of hits taken hears the target's swings at a tank, and the Goblin Sapper
        // Charge's hit on its thrower when its mask and flags match that spell.
        let struck = hears_hits_taken(&listener);
        let sapper_hit = hears_the_sappers_hit(env, &listener);
        let heard = (!names.is_empty()
            && names.iter().all(|name| {
                matches!(
                    *name,
                    "on_spell_hit_dealt" | "on_heal_dealt" | "on_cast_complete"
                )
            }))
            || struck
            || sapper_hit.is_some();
        if struck && !tanks_the_target(env) && sapper_hit.is_none() {
            effects.push(json!({
                "kind": "inert_listener", "unit": "player",
                "aura": env.sim.aura(trigger_aura).label,
                "reason": "hears only melee hits the player takes",
            }));
            continue;
        }
        let unsupported = match (&entry, proc_aura) {
            (Some(entry), Some(_)) => {
                entry.has("stacking_aura")
                    || env.sim.aura(trigger_aura).dpm.is_some()
                    || trigger.rppm != 0.0
                    || entry.message("proc").is_some_and(|p| p.f64("ppm") != 0.0)
                    || buff.max_stack.max(trigger.max_stack) > 0
                    || buff.proc_charges > 0
                    || !heard
                    || listener.is_weapon_proc
                    || (listener.outcome != HitOutcome::EMPTY
                        && listener.outcome != HitOutcome::LANDED)
            }
            _ => true,
        };
        if unsupported {
            unrepresented.push(format!(
                "{} is not a chance stat proc on hits, heals or casts",
                proc.label
            ));
            continue;
        }
        let (Some(entry), Some(proc_aura)) = (entry, proc_aura) else {
            continue;
        };
        let mut effect = json!({
            "kind": "spell_data_stat_proc", "trigger_aura": env.sim.aura(trigger_aura).label,
            "aura": env.sim.aura(proc_aura).label,
            "trigger_spells": match sapper_hit {
                Some(position) => vec![position],
                None if struck => Vec::new(),
                None => proc_trigger_spells(env, &listener),
            },
            "callbacks": names,
            "landed_only": listener.outcome == HitOutcome::LANDED,
            "require_damage": listener.require_damage_dealt,
            "proc_chance": chance_or_certain(&listener),
        });
        if struck {
            effect["struck"] = json!(true);
        }
        if !proc.parsed {
            let bonus = effect_stats(&entry);
            let id = env
                .sim
                .aura(proc_aura)
                .action_id
                .clone()
                .unwrap_or_default();
            effect["gain_log"] = json!(format!(
                "Gained {} from {}.",
                flat_string(&bonus),
                action_id_string(&id)
            ));
            effect["expire_log"] = json!(format!(
                "Lost {} from fading {}.",
                flat_string(&bonus),
                action_id_string(&id)
            ));
        }
        effects.push(effect);
    }
    effects
}

// ---------------------------------------------------------------------------------------------
// main.go: the lion horn and the damage shield, classic items through the shared constructors.
// ---------------------------------------------------------------------------------------------

/// classic items_store_gaps.go The Lion Horn of Stormwind.
pub(crate) const LION_HORN: &str = "The Lion Horn of Stormwind";

/// main.go `lionHornProcAura`: a tank's proc aura joins the stat auras, whose combinations carry
/// the target's rolls.
pub(crate) fn lion_horn_proc_aura(env: &Environment) -> Vec<String> {
    let target = env.encounter.targets[0];
    if aura_named(env, LION_HORN).is_none()
        || env.sim.unit(target).current_target != Some(env.player)
    {
        return Vec::new();
    }
    vec![format!("{LION_HORN} Proc")]
}

/// main.go `lionHornProc`: The Lion Horn of Stormwind, through shared.NewProcStatBonusEffect: a
/// listener on landed melee hits the wearer takes that dealt damage, at the item effect's chance
/// and cooldown, that a spell batch window later activates the effect's temporary stats aura.
fn lion_horn_proc(env: &Environment, trigger: AuraId) -> Option<Value> {
    let proc_aura = aura_named(env, &format!("{LION_HORN} Proc"))?;
    let entry = items::database_item(14557)?
        .item_effects
        .into_iter()
        .rfind(|candidate| candidate.has("proc"))?;
    let proc = entry.message("proc")?;
    let trigger_aura = env.sim.aura(trigger);
    let icd_ms = proc.i32("icd_ms");
    if entry.has("stacking_aura")
        || entry.i32("max_cumulative_stacks") > 0
        || trigger_aura.dpm.is_some()
        || proc.f64("ppm") != 0.0
        || (icd_ms != 0) != trigger_aura.icd.is_some()
        || trigger_aura
            .icd
            .is_some_and(|icd| icd.duration != i64::from(icd_ms) * super::sim::MILLISECOND)
    {
        return None;
    }
    let chance = if proc.f64("proc_chance") == 0.0 {
        1.0
    } else {
        proc.f64("proc_chance")
    };
    let bonus = effect_stats(&entry);
    let id = env
        .sim
        .aura(proc_aura)
        .action_id
        .clone()
        .unwrap_or_default();
    Some(json!({
        "kind": "spell_data_stat_proc", "trigger_aura": trigger_aura.label,
        "aura": env.sim.aura(proc_aura).label, "trigger_spells": Vec::<usize>::new(),
        "callbacks": ["on_spell_hit_taken"], "struck": true, "landed_only": true,
        "require_damage": true, "proc_chance": chance,
        "gain_log": format!("Gained {} from {}.", flat_string(&bonus), action_id_string(&id)),
        "expire_log": format!("Lost {} from fading {}.", flat_string(&bonus), action_id_string(&id)),
    }))
}

/// main.go `damageShieldProc`: classic items_armor.go newDamageShieldEffect, through
/// shared.NewProcDamageEffect: a listener on landed melee hits the wearer takes, at no chance or
/// cooldown, that casts a fixed magic hit of the school on the attacker at once, which cannot
/// crit. Only that shape is described.
fn damage_shield_proc(
    env: &Environment,
    aura: AuraId,
    spell_id: i32,
    damage: f64,
) -> Option<Value> {
    let position = spell_position(env, &ActionId::spell(spell_id));
    let a = env.sim.aura(aura);
    let position = position?;
    if a.dpm.is_some()
        || a.icd.is_some()
        || env.sim.spell(spell_at(env, position)).defense_type != DefenseType::Magic
        || a.events.on_spell_hit_dealt
        || !a.events.on_spell_hit_taken
        || a.events.on_periodic_damage_dealt
    {
        return None;
    }
    Some(json!({
        "kind": "spell_data_damage_proc", "trigger_aura": a.label, "trigger_spells": Vec::<usize>::new(),
        "struck": true, "landed_only": true, "require_damage": false, "proc_chance": 1.0,
        "spell": position, "average": 0.0, "variance": 0.0, "roll": [damage, damage],
        "can_crit": false,
    }))
}

/// main.go `spellDataAbsorbProc`: shared.NewSpellDataAbsorbProc: a listener resolved from the
/// trigger row that casts the absorb row's spell on the wearer at once, whose aura shields it
/// against the schools the absorb effect masks for the amount the effect rolls. Only a listener
/// on the melee hits the player takes, at a static chance, is described.
fn spell_data_absorb_proc(
    env: &Environment,
    aura: AuraId,
    trigger_id: i32,
    absorb_id: i32,
) -> Option<Value> {
    let trigger = must_find(trigger_id);
    let row = must_find(absorb_id);
    let listener = trigger_for(env, trigger, true);
    let absorb = row.absorb_effect();
    let position = spell_position(env, &ActionId::spell(row.id))?;
    let spell = env.sim.spell(spell_at(env, position));
    let a = env.sim.aura(aura);
    if absorb.is_nil()
        || spell.related_self_buff.is_none()
        || listener.dpm.is_some()
        || trigger.rppm != 0.0
        || listener.can_proc_from_procs
        || listener.is_weapon_proc
        || listener.class_spell_mask != 0
        || listener.spell_flags != SpellFlag::NONE
        || listener.proc_mask_exclude != ProcMask::UNKNOWN
        || (listener.icd != 0) != a.icd.is_some()
        || a.icd.is_some_and(|icd| icd.duration != listener.icd)
    {
        return None;
    }
    let level = env.sim.unit(env.player).level;
    let shield = env.sim.aura(spell.related_self_buff?).label.clone();
    Some(json!({
        "kind": "spell_data_absorb_proc", "trigger_aura": a.label,
        "outcome": outcome_names(listener.outcome),
        "require_damage": listener.require_damage_dealt, "proc_chance": chance_or_certain(&listener),
        "spell": position, "aura": shield, "schools": absorb.misc,
        "average": absorb.average(level), "variance": absorb.variance,
    }))
}

/// main.go `meleeItemListeners`: the item procs of `SPELL_DATA_DAMAGE_PROCS` whose listener,
/// decoded by spelldata's `ProcTrigger` from the trigger row, hears only melee hits it deals. The
/// listener never acts unless a spell with a melee special mask exists.
pub(crate) fn melee_item_listeners(env: &Environment) -> Vec<Value> {
    let book = &env.sim.unit(env.player).spellbook;
    if book.iter().any(|spell| {
        env.sim
            .spell(*spell)
            .proc_mask
            .matches(ProcMask::MELEE_SPECIAL)
    }) {
        return Vec::new();
    }
    let mut effects = Vec::new();
    for (label, trigger, _) in SPELL_DATA_DAMAGE_PROCS {
        if aura_named(env, label).is_none() {
            continue;
        }
        let listener = trigger_for(env, find(trigger), true);
        if listener.callback == CallbackMask::ON_SPELL_HIT_TAKEN
            || listener.proc_mask == ProcMask::UNKNOWN
            || listener.proc_mask.0 & !ProcMask::MELEE.0 != 0
        {
            continue;
        }
        effects.push(json!({
            "kind": "inert_listener", "unit": "player", "aura": label,
            "reason": "acts only on melee hits",
        }));
    }
    effects
}

/// main.go `hitTakenItemProcs`: item procs whose listener hears only the melee hits the player
/// takes: Go literal triggers in classic items (Essence of the Pure Flame's damage shield, The
/// Lion Horn of Stormwind), and Uther's Strength and the chest absorption enchants, whose trigger
/// rows spelldata decodes.
///
/// Beyond the exporter's list, Deathbone Guardian's three piece Deathbone Surge
/// (item_sets_classic.go `setStatProc`): a 15% chance on the melee hits the wearer takes, so
/// without a tank it never rolls. A tank's is a struck stat proc under Forever's rules and stays
/// refused as a reaction to the target's swings under Go's.
const HIT_TAKEN_ITEM_PROCS: [(&str, i32, i32); 11] = [
    ("Essence of the Pure Flame", 0, 0),
    ("Force Reactive Disk", 0, 0),
    ("Naglering", 0, 0),
    ("Drillborer Disk", 0, 0),
    ("Razor Gauntlets", 0, 0),
    (LION_HORN, 0, 0),
    ("Uther's Strength", 8397, 10368),
    ("Enchant Chest - Minor Absorption", 7445, 7423),
    ("Enchant Chest - Lesser Absorption", 7446, 7447),
    ("Enchant Chest - Absorption", 1249072, 1249073),
    ("Deathbone Guardian 3P", 0, 0),
];

/// main.go `damageShieldItems`: the damage shields of common/classic/items_trinkets.go and
/// items_armor.go (`newDamageShieldEffect`): the spell each casts on its attacker and the amount,
/// which neither crits nor partially resists.
const DAMAGE_SHIELD_ITEMS: [(&str, i32, f64); 4] = [
    ("Essence of the Pure Flame", 23266, 13.0),
    ("Naglering", 15438, 3.0),
    ("Drillborer Disk", 15438, 3.0),
    ("Razor Gauntlets", 1302193, 7.0),
];

/// main.go `forceReactiveDiskProc`: classic items_armor.go Force Reactive Disk: a listener on the
/// melee hits the wearer takes that it blocks, behind a one second cooldown, whose handler a spell
/// batch window later casts a nature spell on the wearer that deals 25 to every target on the
/// magic table with a crit. Only that shape is described, or `None`.
fn force_reactive_disk_proc(env: &Environment, aura: AuraId) -> Option<Value> {
    let position = spell_position(env, &ActionId::item(18168))?;
    let a = env.sim.aura(aura);
    if a.dpm.is_some()
        || a.icd.is_none_or(|icd| icd.duration != SECOND)
        || !a.events.on_spell_hit_taken
        || a.events.on_spell_hit_dealt
        || env.sim.spell(spell_at(env, position)).defense_type != DefenseType::Magic
    {
        return None;
    }
    Some(json!({
        "kind": "force_reactive_disk", "trigger_aura": a.label, "spell": position, "damage": 25.0,
    }))
}

/// main.go `hitTakenItemListeners`.
pub(crate) fn hit_taken_item_listeners(
    env: &Environment,
    tanking: bool,
    unrepresented: &mut Vec<String>,
) -> Vec<Value> {
    const LIFECYCLE: [&str; 7] = [
        "on_init",
        "on_reset",
        "on_done_iteration",
        "on_gain",
        "on_expire",
        "on_stacks_change",
        "on_encounter_start",
    ];
    let mut effects = Vec::new();
    for (label, trigger_id, absorb_id) in HIT_TAKEN_ITEM_PROCS {
        let Some(aura) = aura_named(env, label) else {
            continue;
        };
        let mut heard = env.sim.aura(aura).callback_names().iter().all(|callback| {
            LIFECYCLE.contains(&callback.as_str()) || callback == "on_spell_hit_taken"
        });
        if trigger_id != 0 {
            let listener = trigger_for(env, find(trigger_id), false);
            let names = callback_names(listener.callback);
            heard = heard
                && names.len() == 1
                && names[0] == "on_spell_hit_taken"
                && listener.proc_mask != ProcMask::UNKNOWN
                && listener.proc_mask.0 & !ProcMask::MELEE.0 == 0;
        }
        if let (true, Some(&(_, spell, damage))) = (
            heard && tanking,
            DAMAGE_SHIELD_ITEMS
                .iter()
                .find(|(item, _, _)| *item == label),
        ) {
            match damage_shield_proc(env, aura, spell, damage) {
                Some(effect) => effects.push(effect),
                None => unrepresented.push(format!("{label}'s proc is not a damage shield")),
            }
            continue;
        }
        if heard && tanking && label == "Force Reactive Disk" {
            match force_reactive_disk_proc(env, aura) {
                Some(effect) => effects.push(effect),
                None => unrepresented.push(format!("{label}'s proc is not a block shield")),
            }
            continue;
        }
        if heard && tanking && label == LION_HORN {
            match lion_horn_proc(env, aura) {
                Some(effect) => effects.push(effect),
                None => unrepresented.push(format!("{label}'s proc is not a chance stat proc")),
            }
            continue;
        }
        // Go setStatProc's Deathbone Surge: each melee swing the wearer takes, landed or not, has
        // a 15% chance, once a minute (the set aura's cooldown), to give 70 spell power for 10
        // sec, a spell batch window later. Forever's rules describe it; Go's exporter refuses it.
        if heard
            && tanking
            && label == "Deathbone Guardian 3P"
            && env.sim.rules == super::sim::Rules::Forever
        {
            match aura_named(env, "Deathbone Surge") {
                Some(proc_aura) if env.sim.aura(aura).icd.is_some() => {
                    let bonus =
                        Stats::from_pairs(&[(Stat::SpellDamage, 70.0), (Stat::HealingPower, 70.0)]);
                    let id = ActionId::spell(1299746);
                    effects.push(json!({
                        "kind": "spell_data_stat_proc", "trigger_aura": label,
                        "aura": env.sim.aura(proc_aura).label,
                        "trigger_spells": Vec::<usize>::new(),
                        "callbacks": ["on_spell_hit_taken"], "struck": true, "landed_only": false,
                        "require_damage": false, "proc_chance": 0.15,
                        "gain_log": format!("Gained {} from {}.", flat_string(&bonus), action_id_string(&id)),
                        "expire_log": format!("Lost {} from fading {}.", flat_string(&bonus), action_id_string(&id)),
                    }));
                }
                _ => unrepresented.push(format!("{label}'s proc is not a chance stat proc")),
            }
            continue;
        }
        if heard && tanking && absorb_id != 0 {
            match spell_data_absorb_proc(env, aura, trigger_id, absorb_id) {
                Some(effect) => effects.push(effect),
                None => unrepresented.push(format!(
                    "{label}'s proc is not an absorb shield on the wearer"
                )),
            }
            continue;
        }
        if heard {
            effects.push(json!({
                "kind": "inert_listener", "unit": "player", "aura": label,
                "reason": "hears only melee hits the player takes",
            }));
        }
    }
    if let (Some(aura), true) = (aura_named(env, "Freezing Band"), tanking) {
        match freezing_band_proc(env, aura) {
            Some(effect) => effects.push(effect),
            None => unrepresented
                .push("Freezing Band's proc is not a rolled magic hit on the attacker".to_string()),
        }
    }
    effects
}

/// main.go `freezingBandProc`: classic items_store_gaps.go Freezing Band, through
/// shared.NewProcDamageEffect: a listener on the landed melee hits the wearer takes that dealt
/// damage, at 1%, that casts a magic hit of Frost rolled between 50 and 50 on the attacker at
/// once, which can crit. Only that shape is described.
fn freezing_band_proc(env: &Environment, aura: AuraId) -> Option<Value> {
    let position = spell_position(env, &ActionId::spell(18798))?;
    let a = env.sim.aura(aura);
    if a.dpm.is_some()
        || a.icd.is_some()
        || env.sim.spell(spell_at(env, position)).defense_type != DefenseType::Magic
        || a.events.on_spell_hit_dealt
        || !a.events.on_spell_hit_taken
        || a.events.on_periodic_damage_dealt
    {
        return None;
    }
    Some(json!({
        "kind": "spell_data_damage_proc", "trigger_aura": a.label, "trigger_spells": Vec::<usize>::new(),
        "struck": true, "landed_only": true, "require_damage": true, "proc_chance": 0.01,
        "spell": position, "average": 0.0, "variance": 0.0, "roll": [50.0, 50.0],
        "can_crit": true,
    }))
}

// ---------------------------------------------------------------------------------------------
// melee_procs.go.
// ---------------------------------------------------------------------------------------------

/// A shape `meleeProcEffects` cannot describe, in Go's words.
fn not_single_target_magic_hit(label: &str) -> String {
    not_single_target_hit(label, "magic")
}

/// [`not_single_target_magic_hit`] for a hit of the named kind.
fn not_single_target_hit(label: &str, hit: &str) -> String {
    format!("{label}'s proc is not a single target {hit} hit")
}

/// melee_procs.go `spellDataDamageProcs`: item procs common/shared/shared_utils.go
/// applySpellDataDamageProc builds from client rows, which
/// common/forever/stat_bonus_procs_auto_gen.go registers with shared.NewSpellDataDamageProc: the
/// item's trigger aura, its trigger row and the damage row the proc casts.
const SPELL_DATA_DAMAGE_PROCS: [(&str, i32, i32); 27] = [
    ("Totem of Infliction", 7617, 16783),
    ("Skullflame Shield -  - ", 18815, 18817),
    ("Hurricane", 1316319, 29502),
    ("Red Whelp Gloves", 9233, 9057),
    ("Vile Protector", 7619, 1293421),
    ("Thermaplugg's Central Core", 1292880, 1292879),
    ("Swine Fists", 1293783, 1293782),
    ("Girdle of Reprisal", 7617, 16783),
    ("Fiery Plate Gauntlets", 7721, 7714),
    ("Storm Gauntlets", 16615, 16614),
    ("Orb of Fire", 16982, 13441),
    ("Blazefury Medallion", 7711, 7712),
    ("Force Imbued Gauntlets", 1302248, 1302247),
    ("Grand Marshal's Aegis - ", 13959, 16782),
    ("High Warlord's Shield Wall - ", 13959, 16782),
    ("High Warlord's Shield Wall -  - ", 1216968, 16782),
    ("Grand Marshal's Aegis -  - ", 1216968, 16782),
    ("Premier High Warlord's Shield Wall", 13959, 16782),
    ("Premier Grand Marshal's Aegis", 13959, 16782),
    ("Searing Dagger", 1291568, 1291570),
    ("Cursed Murloc Eye", 1292674, 1292675),
    ("Thorncursed Grips", 1293331, 1293333),
    ("Coldflame Saber", 1300128, 1300130),
    ("Satchel of Copper Bombs", 1318034, 1318031),
    ("Satchel of Bronze Bombs", 1318062, 1318061),
    ("Satchel of Iron Bombs", 1318069, 1318068),
    ("Satchel of Dark Iron Bombs", 1318123, 1318121),
];

/// melee_procs.go `spellDataDamageProcEffects`: a listener resolved from the trigger row that
/// casts the damage spell at once on the unit hit, or, for a hit taken, on the attacker. The hit
/// is described as `proc_damage_shape` does: dealt where it lands, or struck by a melee or ranged
/// hit.
fn spell_data_damage_proc_effects(
    env: &Environment,
    unrepresented: &mut Vec<String>,
) -> Vec<Value> {
    let mut effects = Vec::new();
    let melee = env.sim.unit(env.player).spellbook.iter().any(|spell| {
        env.sim
            .spell(*spell)
            .proc_mask
            .matches(ProcMask::MELEE_SPECIAL)
    });
    for (label, trigger_id, damage_id) in SPELL_DATA_DAMAGE_PROCS {
        if aura_named(env, label).is_none() {
            continue;
        }
        let trigger = must_find(trigger_id);
        let damage = must_find(damage_id);
        let listener = trigger_for(env, trigger, true);
        let struck = listener.callback == CallbackMask::ON_SPELL_HIT_TAKEN;
        // Without a melee special, meleeItemListeners describes a listener of melee hits dealt as
        // inert.
        if !struck
            && !melee
            && listener.proc_mask != ProcMask::UNKNOWN
            && listener.proc_mask.0 & !ProcMask::MELEE.0 == 0
        {
            continue;
        }
        let position = spell_position(env, &ActionId::spell(damage.id));
        let names = callback_names(listener.callback);
        let outcome_ok =
            listener.outcome == HitOutcome::EMPTY || listener.outcome == HitOutcome::LANDED;
        let struck_ok = !struck || hears_the_targets_swings(env, &listener);
        let shape = position.and_then(|position| proc_damage_shape(env, damage, position).ok());
        let (Some(_), Some(shape)) = (position, shape) else {
            unrepresented.push(not_single_target_magic_hit(label));
            continue;
        };
        if trigger.rppm != 0.0
            || listener.dpm.is_some()
            || names.len() != 1
            || (names[0] != "on_spell_hit_dealt" && names[0] != "on_cast_complete" && !struck)
            || !outcome_ok
            || !struck_ok
        {
            unrepresented.push(not_single_target_magic_hit(label));
            continue;
        }
        // A cast carries no result: AttachProcTriggerCallback checks no outcome and no damage for it.
        let casts = names[0] == "on_cast_complete";
        // A struck proc hears the attacker's hits, which the Rust runtime tells by their masks.
        let trigger_spells = if struck {
            Vec::new()
        } else {
            proc_trigger_spells(env, &listener)
        };
        let mut exported = json!({
            "kind": "spell_data_damage_proc", "trigger_aura": label, "trigger_spells": trigger_spells,
            "landed_only": listener.outcome == HitOutcome::LANDED && !casts,
            "require_damage": listener.require_damage_dealt && !casts,
            "proc_chance": chance_or_certain(&listener),
        });
        for (key, value) in shape {
            exported[key] = value;
        }
        if casts {
            exported["casts"] = json!(true);
        }
        if struck {
            exported["struck"] = json!(true);
        }
        effects.push(exported);
    }
    effects
}

/// item_sets_classic.go `setMeleeDamageProc` bonuses: (set bonus aura, trigger name, trigger
/// row, hit spell, Go literal range). Stormshroud Armor's Lightning (2 pieces, client 18979 and
/// 18980: 10% on a melee hit for 20 nature with a variance of 0.5) and Volcanic Armor's Firebolt
/// (3 pieces, 9233 and 9057, Go's range doubled from Era).
const SET_MELEE_DAMAGE_PROCS: [(&str, &str, i32, i32, f64, f64); 2] = [
    (
        "Stormshroud Armor 2P",
        "Lightning",
        18979,
        18980,
        15.0,
        25.0,
    ),
    (
        "Volcanic Armor 3P",
        "Firebolt Trigger (Volcanic Armor)",
        9233,
        9057,
        30.0,
        50.0,
    ),
];

/// `setMeleeDamageProc`: a proc trigger on the landed melee hits the wearer deals, none flagged
/// a proc, at the trigger row's stated chance rolled under the trigger's name; a spell batch
/// window later the handler casts a magic hit of the Go literal range, which can crit, on the
/// unit the trigger's hit landed on.
fn set_melee_damage_proc_effects(env: &Environment, unrepresented: &mut Vec<String>) -> Vec<Value> {
    let mut effects = Vec::new();
    for (label, name, trigger_id, spell_id, min, max) in SET_MELEE_DAMAGE_PROCS {
        let Some(aura) = aura_named(env, label) else {
            continue;
        };
        let a = env.sim.aura(aura);
        let position = spell_position(env, &ActionId::spell(spell_id));
        let only_hits_dealt = a.callback_names().iter().all(|callback| {
            callback == "on_spell_hit_dealt"
                || matches!(
                    callback.as_str(),
                    "on_init" | "on_reset" | "on_done_iteration" | "on_gain" | "on_expire"
                )
        });
        let Some(position) =
            position.filter(|_| a.dpm.is_none() && a.icd.is_none() && only_hits_dealt)
        else {
            unrepresented.push(not_single_target_magic_hit(label));
            continue;
        };
        let listener = ProcTrigger {
            callback: CallbackMask::ON_SPELL_HIT_DEALT,
            outcome: HitOutcome::LANDED,
            proc_mask: ProcMask::MELEE,
            ..ProcTrigger::default()
        };
        effects.push(json!({
            "kind": "spell_data_damage_proc", "trigger_aura": label,
            "trigger_spells": proc_trigger_spells(env, &listener),
            "landed_only": true, "require_damage": false,
            "proc_chance": super::resolve_proc::stated_chance(must_find(trigger_id)),
            "spell": position, "average": 0.0, "variance": 0.0, "roll": [min, max],
            "can_crit": true, "delay_ns": SPELL_BATCH_WINDOW, "rng_label": name,
        }));
    }
    effects
}

/// buffs/flametongue_totem.go `driveFlametongueTotem` for a character the party's totem reaches
/// and no class effect describes: the totem's permanent aura is the only member of its exclusive
/// category, so `FlametongueTotemTrigger` is on all fight. The trigger hears the landed main hand
/// autos (its extra condition keeps those of the main hand specials it adds, and Windfury
/// Weapon's strike, which only a Shaman casts) and at once casts `FlametongueTotemAttack`: a
/// magic Fire hit of the main hand's base damage, which can crit and takes no spell power. A
/// category another aura shares, as a Windfury Totem's or a Shaman's own totems, can switch the
/// trigger during the fight and stays unclaimed.
fn party_flametongue_totem_effect(
    env: &Environment,
    unrepresented: &mut Vec<String>,
) -> Option<Value> {
    use super::buffs::flametongue::{
        flametongue_totem_base_damage, FLAMETONGUE_TOTEM_TRIGGER_LABEL,
    };
    let player = env.player;
    let trigger_aura = aura_named(env, FLAMETONGUE_TOTEM_TRIGGER_LABEL)?;
    let sim = &env.sim;
    let category = sim
        .unit(player)
        .categories
        .iter()
        .map(|id| &sim.categories[id.0])
        .find(|category| category.name == super::buffs::generated::FLAMETONGUE_TOTEM.category)?;
    let only_party_totem = category.effects.len() == 1
        && sim.aura(sim.effects[category.effects[0].0].aura).label == "Flametongue Totem";
    if !only_party_totem || !sim.aura(trigger_aura).active {
        return None;
    }
    let weapon = &sim.character(player).equipment[super::items::slot::MAIN_HAND];
    let attack = spell_position(env, &ActionId::spell(16389));
    let mut trigger = proc_trigger(sim, Some(player), must_find(15036), &[]);
    trigger.proc_mask = ProcMask(trigger.proc_mask.0 & ProcMask::MELEE_MH.0);
    let names = callback_names(trigger.callback);
    let certain = trigger.proc_chance == 0.0 || trigger.proc_chance == 1.0;
    let (Some(attack), true, true, true) = (
        attack,
        weapon.swing_speed != 0.0,
        certain && trigger.dpm.is_none() && trigger.icd == 0,
        names == ["on_spell_hit_dealt"]
            && (trigger.outcome == HitOutcome::EMPTY || trigger.outcome == HitOutcome::LANDED),
    ) else {
        unrepresented.push(format!(
            "{FLAMETONGUE_TOTEM_TRIGGER_LABEL} is not a certain hit on landed main hand autos"
        ));
        return None;
    };
    // The trigger's mask with the main hand specials, which its extra condition narrows to the
    // autos for anyone but a Shaman's Windfury Weapon.
    let book = &sim.unit(player).spellbook;
    let trigger_spells: Vec<usize> = proc_trigger_spells(env, &trigger)
        .into_iter()
        .filter(|&position| {
            sim.spell(book[position])
                .proc_mask
                .matches(ProcMask::MELEE_MH_AUTO)
        })
        .collect();
    Some(json!({
        "kind": "spell_data_damage_proc", "trigger_aura": FLAMETONGUE_TOTEM_TRIGGER_LABEL,
        "trigger_spells": trigger_spells, "landed_only": trigger.outcome == HitOutcome::LANDED,
        "require_damage": trigger.require_damage_dealt, "proc_chance": 1.0, "spell": attack,
        "average": flametongue_totem_base_damage(weapon.swing_speed), "variance": 0.0,
        "can_crit": true,
    }))
}

/// melee_procs.go `spellDataHealProcs`: enchants common/shared/shared_utils.go
/// NewSpellDataHealProc builds from client rows: Recovery.
const SPELL_DATA_HEAL_PROCS: [(&str, i32, i32); 2] = [
    ("Enchant Weapon - Recovery", 1248761, 1248759),
    ("Truesilver Breastplate", 9778, 9777),
];

/// melee_procs.go `spellDataHealProcEffects`: a listener resolved from the trigger row that casts
/// the heal row on the wearer at once. Only a direct heal, a share of maximum health or a rolled
/// amount, is described.
fn spell_data_heal_proc_effects(env: &Environment, unrepresented: &mut Vec<String>) -> Vec<Value> {
    let mut effects = Vec::new();
    for (label, trigger_id, heal_id) in SPELL_DATA_HEAL_PROCS {
        let Some(aura) = aura_named(env, label) else {
            continue;
        };
        let trigger = must_find(trigger_id);
        let heal = must_find(heal_id);
        let listener = trigger_for(env, trigger, true);
        let position = spell_position(env, &ActionId::spell(heal.id));
        let effect = heal.proc_heal_effect();
        let names = callback_names(listener.callback);
        let struck = hears_the_targets_swings(env, &listener);
        let a = env.sim.aura(aura);
        let icd = a.icd.is_some_and(|icd| icd.duration != listener.icd);
        let bonus_coefficient =
            position.map_or(0.0, |p| env.sim.spell(spell_at(env, p)).bonus_coefficient);
        let Some(position) = position else {
            unrepresented.push(format!("{label}'s proc is not a direct heal on the wearer"));
            continue;
        };
        if effect.is_nil()
            || effect.aura == dbcenums::A_PERIODIC_HEAL
            || (effect.effect_type != dbcenums::E_HEAL_PCT
                && effect.effect_type != dbcenums::E_HEAL)
            || listener.dpm.is_some()
            || trigger.rppm != 0.0
            || names.len() != 1
            || (names[0] != "on_spell_hit_dealt" && !struck)
            || icd
            || (listener.icd != 0) != a.icd.is_some()
            || bonus_coefficient != 0.0
        {
            unrepresented.push(format!("{label}'s proc is not a direct heal on the wearer"));
            continue;
        }
        let player = env.player;
        let pseudo = &env.sim.unit(player).pseudo_stats;
        let level = env.sim.unit(player).level;
        let mut exported = json!({
            "kind": "spell_data_heal_proc", "trigger_aura": label,
            "trigger_spells": proc_trigger_spells(env, &listener),
            "outcome": outcome_names(listener.outcome),
            "require_damage": listener.require_damage_dealt,
            "proc_chance": chance_or_certain(&listener), "spell": position,
            "can_crit": !heal.cannot_crit(),
            "healing_dealt_multiplier": pseudo.healing_dealt_multiplier,
            "healing_taken_multiplier": pseudo.healing_taken_multiplier,
            "table_healing_dealt_multiplier": env.attack_table(player, player).healing_dealt_multiplier,
            "bonus_healing_taken": pseudo.bonus_healing_taken,
        });
        if struck {
            exported["struck"] = json!(true);
            exported["trigger_spells"] = json!(Vec::<usize>::new());
        }
        if effect.effect_type == dbcenums::E_HEAL_PCT {
            exported["max_health_share"] = json!(effect.percent());
        } else {
            exported["average"] = json!(effect.average(level));
            exported["variance"] = json!(effect.variance);
        }
        effects.push(exported);
    }
    effects
}

/// melee_procs.go `weaponEnchantDamageProcs`: common/forever/enchants.go Fiery Weapon (803) and
/// Lifestealing (1898), and Fiery Blaze (36) from enchants_auto_gen.go: a weapon proc on landed
/// hits, at a rate of the enchanted hand, that casts the row's damage spell at once on the unit
/// hit. Only a magic hit on one target is described.
const WEAPON_ENCHANT_DAMAGE_PROCS: [(&str, i32); 3] = [
    ("Enchant Weapon - Fiery Weapon", 13897),
    ("Enchant Weapon - Lifestealing", 20004),
    ("Enchant: Fiery Blaze", 6297),
];

/// melee_procs.go `singleTargetMultiHit`: a chain or a capped or split area hit casts one roll on
/// it, and an uncapped area hit one roll times the encounter's AoE cap multiplier, which is
/// exactly one for a single target.
fn single_target_multi_hit(env: &Environment, effect: &crate::data::spells::Effect) -> bool {
    if !effect.hits_an_area() && effect.chain_targets <= 1 {
        return true;
    }
    active_target_count(env) == 1 && aoe_cap_multiplier(env) == 1.0
}

fn weapon_enchant_damage_proc_effects(
    env: &Environment,
    unrepresented: &mut Vec<String>,
) -> Vec<Value> {
    let mut effects = Vec::new();
    for (label, damage_id) in WEAPON_ENCHANT_DAMAGE_PROCS {
        let Some(aura) = aura_named(env, label) else {
            continue;
        };
        let damage = must_find(damage_id);
        let position = spell_position(env, &ActionId::spell(damage.id));
        let effect = damage.damage_effect();
        let a = env.sim.aura(aura);
        let (Some(position), Some(dpm)) = (position, a.dpm.as_ref()) else {
            unrepresented.push(not_single_target_magic_hit(label));
            continue;
        };
        let magic = !effect.is_nil()
            && damage.periodic_damage_effect().is_nil()
            && !damage.applies_an_aura_to_an_enemy()
            && env.sim.spell(spell_at(env, position)).defense_type == DefenseType::Magic;
        let single = magic && damage.speed == 0.0 && single_target_multi_hit(env, effect);
        // Past one target an area or a chain, or a hit that flies, is the proc spell of
        // `proc_damage_shape`.
        let shape = if magic && !single {
            proc_damage_shape(env, damage, position).ok()
        } else {
            None
        };
        if a.icd.is_some() || (!single && shape.is_none()) {
            unrepresented.push(not_single_target_magic_hit(label));
            continue;
        }
        let chances = dpm_chances(env, dpm, |spell| {
            !spell.flags.matches(SpellFlag::SUPPRESS_WEAPON_PROCS)
        });
        let triggers: Vec<Value> = chances
            .iter()
            .map(|chance| chance["spell"].clone())
            .collect();
        let mut exported = json!({
            "kind": "spell_data_damage_proc", "trigger_aura": a.label, "trigger_spells": triggers,
            "landed_only": true, "require_damage": false, "proc_chance": 1.0, "chances": chances,
        });
        match shape {
            Some(shape) => {
                for (key, value) in shape {
                    exported[key] = value;
                }
            }
            None => {
                exported["spell"] = json!(position);
                exported["average"] = json!(effect.average(env.sim.unit(env.player).level));
                exported["variance"] = json!(effect.variance);
                exported["can_crit"] = json!(!damage.cannot_crit());
            }
        }
        effects.push(exported);
    }
    effects
}

/// An item common/shared/shared_utils.go NewProcDamageEffect builds by hand, as
/// melee_procs.go `procDamageItems` lists it.
struct ProcDamageItem {
    label: &'static str,
    spell_id: i32,
    min: f64,
    max: f64,
    defense: DefenseType,
    require_damage: bool,
    mask: ProcMask,
    /// The runtime's name for the table the hit rolls, where the damage type alone does not say.
    outcome: &'static str,
}

/// melee_procs.go `procDamageItems`: a listener on landed hits at a legacy proc manager's rate
/// that casts a hit of a Go literal range at once on the unit hit. Heart of Wyrmthalak is a
/// magic hit on melee and ranged hits. Iceblade Hacker and Warblade of Caer Darrow are Frost hits
/// of the melee defense type on the landed melee hits that dealt damage, at a fixed chance of 1 on
/// the hand holding the weapon. Their client rows carry the always-hit attribute (the fork's
/// patch 112), so they roll the crit and never miss, dodge or parry
/// (`OutcomeMeleeCritOnly`); a magic hit rolls the magic table.
const PROC_DAMAGE_ITEMS: [ProcDamageItem; 4] = [
    ProcDamageItem {
        label: "Heart of Wyrmthalak",
        spell_id: 27655,
        min: 112.0,
        max: 168.0,
        defense: DefenseType::Magic,
        require_damage: false,
        mask: ProcMask::MELEE_OR_RANGED,
        outcome: "",
    },
    ProcDamageItem {
        label: "Iceblade Hacker",
        spell_id: 1298414,
        min: 40.70000076293945,
        max: 40.70000076293945,
        defense: DefenseType::Melee,
        require_damage: true,
        mask: ProcMask::MELEE,
        outcome: "melee_special_crit_only",
    },
    ProcDamageItem {
        label: "Warblade of Caer Darrow",
        spell_id: 1298499,
        min: 27.719999313354492,
        max: 27.719999313354492,
        defense: DefenseType::Melee,
        require_damage: true,
        mask: ProcMask::MELEE,
        outcome: "melee_special_crit_only",
    },
    ProcDamageItem {
        label: "Darkmoon Card: Maelstrom",
        spell_id: 23687,
        min: 200.0,
        max: 300.0,
        defense: DefenseType::Magic,
        require_damage: false,
        mask: ProcMask::MELEE,
        outcome: "",
    },
];

fn proc_damage_item_effects(env: &Environment, unrepresented: &mut Vec<String>) -> Vec<Value> {
    let mut effects = Vec::new();
    for item in PROC_DAMAGE_ITEMS {
        let (label, mask) = (item.label, item.mask);
        let Some(aura) = aura_named(env, label) else {
            continue;
        };
        let hit = if item.defense == DefenseType::Melee {
            "melee"
        } else {
            "magic"
        };
        let position = spell_position(env, &ActionId::spell(item.spell_id));
        let a = env.sim.aura(aura);
        let (Some(position), Some(dpm)) = (position, a.dpm.as_ref()) else {
            unrepresented.push(not_single_target_hit(label, hit));
            continue;
        };
        if a.icd.is_some()
            || env.sim.spell(spell_at(env, position)).defense_type != item.defense
            || !a.events.on_spell_hit_dealt
            || a.events.on_spell_hit_taken
            || a.events.on_periodic_damage_dealt
        {
            unrepresented.push(not_single_target_hit(label, hit));
            continue;
        }
        let listener = ProcTrigger {
            proc_mask: mask,
            outcome: HitOutcome::LANDED,
            ..ProcTrigger::default()
        };
        let mut effect = json!({
            "kind": "spell_data_damage_proc", "trigger_aura": label,
            "trigger_spells": proc_trigger_spells(env, &listener),
            "landed_only": true, "require_damage": item.require_damage, "proc_chance": 1.0,
            "spell": position, "average": 0.0, "variance": 0.0, "roll": [item.min, item.max],
            "can_crit": true,
            "chances": dpm_chances(env, dpm, |spell| {
                spell.proc_mask.matches(mask) && !spell.flags.matches(SpellFlag::PROC)
            }),
        });
        if !item.outcome.is_empty() {
            effect["outcome"] = json!(item.outcome);
        }
        effects.push(effect);
    }
    effects
}

/// weapon_procs.go `weaponDamageProcs`: common/forever/items_weapons.go weapons whose chance on
/// hit casts a spell of the client's rows, registered through itemhelpers.CreateWeaponProcSpell
/// over shared.SpellDataProcDamageSpell. The trigger aura is the weapon's name and " Proc".
const WEAPON_DAMAGE_PROCS: [(&str, i32); 17] = [
    ("Barbaric Crossbow", 1291551),
    ("Plaguefang", 1309315),
    ("Wolfsbane", 1282503),
    ("Venomstrike", 29653),
    ("Stinging Viper", 1291663),
    ("Alcor's Sunrazor", 18833),
    ("Bloodfist", 16433),
    ("Bonechill Hammer", 18276),
    ("Darrowspike", 18276),
    ("Coldrage Dagger", 1293790),
    ("Glacial Blade", 18398),
    ("Flame Wrath", 16559),
    ("Masterwork Stormhammer", 16921),
    ("Electrified Dagger", 23592),
    ("Shadowstrike", 21170),
    ("The Cruel Hand of Timmy", 17505),
    ("Skullforge Reaver", 17484),
];

/// weapon_procs.go `procHitOutcome`: shared_utils.go `damageOutcome` by the runtime's name for a
/// hit of the spell's defense type. A magic hit has no name: the effect's `can_crit` picks
/// between the two magic appliers, as it does for every spell data damage proc.
fn proc_hit_outcome(defense: DefenseType, cannot_crit: bool) -> Option<&'static str> {
    match (defense, cannot_crit) {
        (DefenseType::Melee, true) => Some("melee_special_hit"),
        (DefenseType::Melee, false) => Some("melee_special_hit_and_crit"),
        (DefenseType::Ranged, true) => Some("ranged_hit"),
        (DefenseType::Ranged, false) => Some("ranged_hit_and_crit"),
        _ => None,
    }
}

/// weapon_procs.go `procTickOutcome`: spelldata `Spell.TickOutcome` by the runtime's name.
fn proc_tick_outcome(row: &Spell) -> &'static str {
    let magic = row.defense_type_core() == DefenseType::Magic;
    match (row.periodic_can_crit(), magic) {
        (true, true) => "tick_magic_hit_and_crit",
        (true, false) => "tick_physical_crit",
        (false, true) => "tick_magic_hit",
        (false, false) => "tick",
    }
}

/// weapon_procs.go `procDamageShape`: the fields of a spell_data_damage_proc effect that the
/// damage row and the spell registered for it decide, whoever listens: the spell, the row's
/// direct hit and the table it rolls, the targets of an area or chain, and the damage over time
/// the row carries. The reason the runtime cannot run the row, phrased for "the item's proc ...",
/// is returned instead where there is one.
fn proc_damage_shape(
    env: &Environment,
    damage: &'static Spell,
    position: usize,
) -> Result<serde_json::Map<String, Value>, &'static str> {
    proc_damage_shape_with(env, damage, position, true)
}

/// [`proc_damage_shape`], refusing ticks that roll a magic hit without a crit only where
/// `refuse_magic_ticks` (the reference's rows).
fn proc_damage_shape_with(
    env: &Environment,
    damage: &'static Spell,
    position: usize,
    refuse_magic_ticks: bool,
) -> Result<serde_json::Map<String, Value>, &'static str> {
    let level = env.sim.unit(env.player).level;
    let (direct, periodic) = (damage.damage_effect(), damage.periodic_damage_effect());
    let defense = env.sim.spell(spell_at(env, position)).defense_type;
    let outcome = proc_hit_outcome(defense, damage.cannot_crit());
    if direct.is_nil() && periodic.is_nil() {
        return Err("deals no damage");
    } else if damage.debuffs_the_target() {
        return Err("debuffs the target");
    } else if !periodic.is_nil() && (direct.hits_an_area() || direct.chain_targets > 1) {
        return Err("spreads and leaves a damage over time");
    } else if !periodic.is_nil() && direct.is_nil() && damage.speed != 0.0 {
        return Err("missile carries only a damage over time");
    } else if refuse_magic_ticks
        && !periodic.is_nil()
        && proc_tick_outcome(damage) == "tick_magic_hit"
    {
        return Err("ticks roll a magic hit without a crit");
    } else if !matches!(
        defense,
        DefenseType::Magic | DefenseType::Melee | DefenseType::Ranged
    ) {
        return Err("rolls no known hit table");
    }
    let mut shape = serde_json::Map::new();
    shape.insert("spell".to_string(), json!(position));
    shape.insert("average".to_string(), json!(0.0));
    shape.insert("variance".to_string(), json!(0.0));
    shape.insert("can_crit".to_string(), json!(!damage.cannot_crit()));
    if !direct.is_nil() {
        shape.insert("average".to_string(), json!(direct.average(level)));
        shape.insert("variance".to_string(), json!(direct.variance));
        if let Some(outcome) = outcome {
            shape.insert("outcome".to_string(), json!(outcome));
        }
        if direct.hits_an_area() {
            shape.insert(
                "area".to_string(),
                json!({
                    "max_targets": i32::from(damage.max_targets), "splits": damage.splits_damage,
                    "aoe_cap_multiplier": aoe_cap_multiplier(env),
                }),
            );
        } else if direct.chain_targets > 1 {
            shape.insert(
                "chain".to_string(),
                json!({
                    "targets": i32::from(direct.chain_targets),
                    "amp": f64::from(direct.chain_amp),
                }),
            );
        }
    }
    if !periodic.is_nil() {
        shape.insert(
            "periodic".to_string(),
            json!({
                "tick_base": periodic.average(level), "tick_outcome": proc_tick_outcome(damage),
                "with_direct": !direct.is_nil(),
            }),
        );
    }
    Ok(shape)
}

/// Under Forever's rules, the damage over time alone of a chance on hit `CHANCE_ON_HIT_ITEMS`
/// registers (forever_items.rs), where the reference describes none: a magic one rolls the magic
/// hit as it goes on and its ticks deal their amount, as Classic dots do, where the reference's
/// row reads a magic hit on every tick; one with a missile goes on as the hit lands (its flight is
/// not modelled); and one around the wielder, as Seeping Willow's, goes on every target. `None`
/// for any other row, which [`proc_damage_shape`] describes.
fn chance_on_hit_dot_shape(
    env: &Environment,
    damage: &'static Spell,
    position: usize,
) -> Option<serde_json::Map<String, Value>> {
    let level = env.sim.unit(env.player).level;
    let (direct, periodic) = (damage.damage_effect(), damage.periodic_damage_effect());
    if periodic.is_nil() || damage.debuffs_the_target() {
        return None;
    }
    if !direct.is_nil() {
        // With a hit, the dot goes on where the hit landed: only a magic tick's shape changes.
        if direct.hits_an_area() || direct.chain_targets > 1 {
            return None;
        }
        let mut shape = proc_damage_shape_with(env, damage, position, false).ok()?;
        shape["periodic"]["tick_outcome"] = json!(match proc_tick_outcome(damage) {
            "tick_magic_hit" => "tick",
            other => other,
        });
        return Some(shape);
    }
    let defense = env.sim.spell(spell_at(env, position)).defense_type;
    let magic = defense == DefenseType::Magic;
    let tick = match proc_tick_outcome(damage) {
        "tick_magic_hit" => "tick",
        other => other,
    };
    let mut periodic_shape = json!({
        "tick_base": periodic.average(level), "tick_outcome": tick, "with_direct": false,
    });
    if magic {
        periodic_shape["application"] = json!("magic_hit");
    }
    let mut shape = serde_json::Map::new();
    shape.insert("spell".to_string(), json!(position));
    shape.insert("average".to_string(), json!(0.0));
    shape.insert("variance".to_string(), json!(0.0));
    shape.insert("can_crit".to_string(), json!(!damage.cannot_crit()));
    shape.insert("periodic".to_string(), periodic_shape);
    if periodic.hits_an_area() {
        shape.insert(
            "area".to_string(),
            json!({
                "max_targets": i32::from(damage.max_targets), "splits": false,
                "aoe_cap_multiplier": aoe_cap_multiplier(env),
            }),
        );
    }
    Some(shape)
}

/// The worn items, then with an item swap the swap set's, once each, as item_effects.rs
/// registers them.
fn chance_on_hit_items(env: &Environment) -> Vec<i32> {
    let mut items: Vec<i32> = env
        .sim
        .character(env.player)
        .equipment
        .iter()
        .map(|item| item.id)
        .collect();
    if let Some(swap) = env.sim.item_swap(env.player) {
        items.extend(swap.un_equipped.iter().map(|item| item.id));
    }
    let mut seen = Vec::new();
    for item in items {
        if item != 0 && !seen.contains(&item) {
            seen.push(item);
        }
    }
    seen
}

/// Under Forever's rules, the buffs of the chance on hit items whose `stat_proc` effect
/// [`chance_on_hit_item_effects`] describes and whose gain changes the wearer's stats, each read
/// from a reset simulation of its own: Destiny's Strength, Quel'Serrar's Defense, Armor and
/// Attack Power. The `stat_proc` effect only activates the aura, so its stats reach the runtime as
/// a stat aura, and a tank's swing rolls follow its Defense and Armor.
pub(crate) fn chance_on_hit_stat_auras(env: &Environment) -> Vec<String> {
    use super::shared_items::ProcKind;
    let mut labels = Vec::new();
    if env.sim.rules != super::sim::Rules::Forever {
        return labels;
    }
    for item in chance_on_hit_items(env) {
        for (kind, cfg) in super::forever_items::chance_on_hit_registrations(item) {
            if !matches!(kind, ProcKind::Proc | ProcKind::Aura) {
                continue;
            }
            let label = spell_data_stat_proc_aura(cfg.name);
            let described = aura_named(env, cfg.name)
                .map(|trigger| env.sim.aura(trigger))
                .is_some_and(|trigger| trigger.dpm.is_some() && trigger.icd.is_none())
                && aura_named(env, &label).is_some_and(|aura| env.sim.aura(aura).max_stacks == 0);
            if !described || labels.contains(&label) {
                continue;
            }
            let mut fresh = env.fresh();
            let player = fresh.player;
            let aura = fresh.sim.get_aura(player, &label).expect("the aura exists");
            let before = fresh.sim.unit(player).stats;
            fresh.sim.activate(aura);
            if fresh.sim.unit(player).stats != before {
                labels.push(label);
            }
        }
    }
    labels
}

/// The client's creature types (`CreatureType`), by the bit a creature type mask gives each,
/// as the request names them.
const CREATURE_TYPES: [(i32, &str); 9] = [
    (1, "MobTypeBeast"),
    (2, "MobTypeDragonkin"),
    (3, "MobTypeDemon"),
    (4, "MobTypeElemental"),
    (5, "MobTypeGiant"),
    (6, "MobTypeUndead"),
    (7, "MobTypeHumanoid"),
    (8, "MobTypeCritter"),
    (9, "MobTypeMechanical"),
];

/// A chance on hit buff's melee attack power against creature types
/// (`A_MOD_MELEE_ATTACK_POWER_VERSUS`, its mask in the misc value), which the parsed aura leaves
/// out: Quel'Serrar's Sanctuary grants 36 against Dragonkin (22850, mask 2). It holds when every
/// target is of a type the mask names, as a fight's targets are of one type.
fn melee_power_versus_targets(env: &Environment, row: &Spell) -> Option<f64> {
    let named = |mask: i32, target: UnitId| {
        let mob_type = env.sim.unit(target).mob_type.as_str();
        CREATURE_TYPES
            .iter()
            .any(|(id, name)| mask & (1 << (id - 1)) != 0 && *name == mob_type)
    };
    let power: f64 = row
        .effects
        .iter()
        .filter(|effect| {
            effect.aura == dbcenums::A_MOD_MELEE_ATTACK_POWER_VERSUS
                && env
                    .encounter
                    .targets
                    .iter()
                    .all(|&target| named(effect.misc, target))
        })
        .map(|effect| effect.average(env.sim.unit(env.player).level))
        .sum();
    (power != 0.0).then_some(power)
}

/// Under Forever's rules: the chance on hit items `CHANCE_ON_HIT_ITEMS` registers
/// (forever_items.rs), which the reference leaves unregistered. Each weapon or item proc rolls its
/// manager's chance on the landed hits it hears, at the rate MythicSim assumes, and at once on the
/// hit casts its damage spell, activates its buff (and its attack speed, where the buff changes
/// it) or puts its armor debuff on the unit hit.
fn chance_on_hit_item_effects(env: &Environment, unrepresented: &mut Vec<String>) -> Vec<Value> {
    use super::shared_items::ProcKind;
    let mut effects = Vec::new();
    if env.sim.rules != super::sim::Rules::Forever {
        return effects;
    }
    for item in chance_on_hit_items(env) {
        for (kind, cfg) in super::forever_items::chance_on_hit_registrations(item) {
            let Some(trigger) = aura_named(env, cfg.name) else {
                continue;
            };
            let trigger = env.sim.aura(trigger);
            let (Some(dpm), None) = (trigger.dpm.as_ref(), trigger.icd) else {
                unrepresented.push(format!("{}'s chance on hit has no proc manager", cfg.name));
                continue;
            };
            let chances = dpm_chances(env, dpm, |spell| {
                !spell.flags.matches(SpellFlag::SUPPRESS_WEAPON_PROCS)
                    && !spell.flags.matches(SpellFlag::PROC)
            });
            let (_, row) = cfg.rows();
            match kind {
                ProcKind::Damage => {
                    let Some(position) = spell_position(env, &ActionId::spell(row.id)) else {
                        unrepresented.push(not_single_target_magic_hit(cfg.name));
                        continue;
                    };
                    let shape = match chance_on_hit_dot_shape(env, row, position)
                        .map_or_else(|| proc_damage_shape(env, row, position), Ok)
                    {
                        Ok(shape) => shape,
                        Err(refusal) => {
                            unrepresented.push(format!("{}'s proc {refusal}", cfg.name));
                            continue;
                        }
                    };
                    let triggers: Vec<Value> = chances
                        .iter()
                        .map(|chance| chance["spell"].clone())
                        .collect();
                    let mut effect = json!({
                        "kind": "spell_data_damage_proc", "trigger_aura": trigger.label,
                        "trigger_spells": triggers, "landed_only": true, "require_damage": false,
                        "proc_chance": 1.0, "chances": chances,
                    });
                    for (key, value) in shape {
                        effect[key] = value;
                    }
                    effects.push(effect);
                }
                ProcKind::Proc | ProcKind::Aura => {
                    let Some(aura) = aura_named(env, &spell_data_stat_proc_aura(cfg.name)) else {
                        unrepresented.push(format!("{}'s chance on hit has no buff", cfg.name));
                        continue;
                    };
                    let aura = env.sim.aura(aura);
                    if aura.max_stacks > 0 {
                        unrepresented.push(format!("{}'s buff stacks", cfg.name));
                        continue;
                    }
                    effects.push(json!({
                        "kind": "stat_proc", "trigger_aura": trigger.label,
                        "rng_label": trigger.label, "aura": aura.label, "chances": chances,
                    }));
                    if let Some(power) = melee_power_versus_targets(env, row) {
                        effects.push(json!({
                            "kind": "mob_type_bonus", "aura": aura.label, "attack_power": 0.0,
                            "ranged_attack_power": 0.0, "spell_damage": 0.0,
                            "melee_attack_power": power,
                        }));
                    }
                    let melee: f64 = row
                        .effects
                        .iter()
                        .filter(|effect| effect.aura == dbcenums::A_MOD_MELEE_HASTE_3)
                        .map(|effect| 1.0 + effect.base_value() / 100.0)
                        .product();
                    if kind == ProcKind::Aura && melee != 1.0 {
                        effects.push(json!({
                            "kind": "speed_aura", "aura": aura.label, "melee_multiplier": melee,
                            "ranged_multiplier": 1.0, "cast_multiplier": 1.0,
                        }));
                    }
                }
                ProcKind::Debuff => {
                    let label = format!("{} {}", row.name, row.id);
                    let mut scratch = env.fresh();
                    let target = scratch.encounter.targets[0];
                    let Some(debuff) = scratch.sim.get_aura(target, &label) else {
                        unrepresented.push(format!("{} has no {label} aura", cfg.name));
                        continue;
                    };
                    let mut armor = vec![0.0];
                    let base = scratch.sim.stat(target, Stat::Armor);
                    scratch.sim.activate(debuff);
                    for stacks in 1..=scratch.sim.aura(debuff).max_stacks {
                        scratch.sim.set_stacks(debuff, stacks);
                        armor.push(scratch.sim.stat(target, Stat::Armor) - base);
                    }
                    effects.push(json!({
                        "kind": "armor_debuff_proc", "trigger_aura": trigger.label,
                        "rng_label": trigger.label, "chances": chances, "aura": label,
                        "armor_by_stacks": armor, "immediate": true,
                    }));
                }
                _ => unrepresented.push(format!("{}'s chance on hit is not described", cfg.name)),
            }
        }
    }
    effects
}

/// weapon_procs.go `weaponDamageProcEffects`: the spell shared_utils.go
/// `spellDataProcDamageSpell` builds, cast at once by the handler on the unit hit: the row's direct
/// hit rolled once, or for a chain once a target on the amount the previous jump keeps, on the
/// table the spell's defense type names, then the damage over time the row carries on the targets
/// it landed on. An area hit is calculated on every target, or on the row's cap of them from the
/// target out, and a split one shares one roll. A hit with a missile speed is dealt after its
/// travel. A row that also puts a debuff on the target is not described.
fn weapon_damage_proc_effects(env: &Environment, unrepresented: &mut Vec<String>) -> Vec<Value> {
    let mut effects = Vec::new();
    for (label, damage_id) in WEAPON_DAMAGE_PROCS {
        let Some(aura) = aura_named(env, &format!("{label} Proc")) else {
            continue;
        };
        let damage = must_find(damage_id);
        let position = spell_position(env, &ActionId::spell(damage.id));
        let a = env.sim.aura(aura);
        let (Some(position), Some(dpm), false) = (position, a.dpm.as_ref(), a.icd.is_some()) else {
            unrepresented.push(format!("{label}'s proc has no proc manager or spell"));
            continue;
        };
        let shape = match proc_damage_shape(env, damage, position) {
            Ok(shape) => shape,
            Err(refusal) => {
                unrepresented.push(format!("{label}'s proc {refusal}"));
                continue;
            }
        };
        let chances = dpm_chances(env, dpm, |spell| {
            !spell.flags.matches(SpellFlag::SUPPRESS_WEAPON_PROCS)
        });
        let triggers: Vec<Value> = chances
            .iter()
            .map(|chance| chance["spell"].clone())
            .collect();
        let mut effect = json!({
            "kind": "spell_data_damage_proc", "trigger_aura": a.label, "trigger_spells": triggers,
            "landed_only": true, "require_damage": false, "proc_chance": 1.0, "chances": chances,
        });
        for (key, value) in shape {
            effect[key] = value;
        }
        effects.push(effect);
    }
    effects
}

/// weapon_procs.go `frostOilEffects`: common/forever/frost_oil.go (the fork's patch 126), Frost
/// Oil, a temporary enchantment on a melee weapon whose client row casts a Frostbolt on a landed
/// hit of that weapon at a flat chance, at once on the unit hit. One listener for each oiled
/// hand, over the spell `shared.SpellDataProcDamageSpell` builds from the row. Shadow Oil (a
/// MythicSim Rust-only addition beyond the Go reference pin, client 70707 enchant 25) is its twin,
/// casting its Shadow Bolt the same way.
fn proc_oil_effects(env: &Environment, unrepresented: &mut Vec<String>) -> Vec<Value> {
    let mut effects = Vec::new();
    for oil in &super::proc_oil::PROC_OILS {
        for mask in [ProcMask::MELEE_MH, ProcMask::MELEE_OH] {
            let label = oil.aura_name(mask);
            let label = label.as_str();
            let Some(aura) = aura_named(env, label) else {
                continue;
            };
            let damage = must_find(oil.bolt);
            let position = spell_position(env, &ActionId::spell(damage.id));
            let a = env.sim.aura(aura);
            let Some(position) = position.filter(|_| a.dpm.is_none() && a.icd.is_none()) else {
                unrepresented.push(format!("{label} is not a flat chance on hit for a spell"));
                continue;
            };
            let shape = match proc_damage_shape(env, damage, position) {
                Ok(shape) => shape,
                Err(refusal) => {
                    unrepresented.push(format!("{label}'s proc {refusal}"));
                    continue;
                }
            };
            let listener = ProcTrigger {
                proc_mask: mask,
                is_weapon_proc: true,
                outcome: HitOutcome::LANDED,
                ..ProcTrigger::default()
            };
            let mut effect = json!({
                "kind": "spell_data_damage_proc", "trigger_aura": a.label,
                "trigger_spells": proc_trigger_spells(env, &listener),
                "landed_only": true, "require_damage": false,
                "proc_chance": f64::from(damage.proc_chance) / 100.0,
            });
            for (key, value) in shape {
                effect[key] = value;
            }
            effects.push(effect);
        }
    }
    effects
}

/// weapon_procs.go `flurryAxeEffects`: common/forever/items_weapons.go Flurry Axe: a weapon proc
/// on landed hits, at the weapon's proc manager, that casts a spell whose effect is one extra main
/// hand attack at once (`AutoAttacks.ExtraMHAttack`). The spell deals nothing of its own.
fn flurry_axe_effects(env: &Environment, unrepresented: &mut Vec<String>) -> Vec<Value> {
    let Some(aura) = aura_named(env, "Flurry Axe Proc") else {
        return Vec::new();
    };
    let position = spell_position(env, &ActionId::spell(18797));
    let a = env.sim.aura(aura);
    let (Some(position), Some(dpm), false) = (position, a.dpm.as_ref(), a.icd.is_some()) else {
        unrepresented.push("Flurry Axe's proc has no proc manager or spell".to_string());
        return Vec::new();
    };
    let chances = dpm_chances(env, dpm, |spell| {
        !spell.flags.matches(SpellFlag::SUPPRESS_WEAPON_PROCS)
    });
    vec![json!({
        "kind": "extra_attack_proc", "trigger_aura": a.label, "proc_chance": 1.0, "attacks": 1,
        "chances": chances, "spell": position,
    })]
}

/// weapon_procs.go `weaponAuraProcAuras`: common/forever/items_weapons.go Sword of Zeal and
/// Argent Avenger, weapons whose chance on hit activates an aura of the row on the wearer,
/// registered through itemhelpers.CreateWeaponProcAura. The aura is the row's effects parsed
/// (`parse_effects`), the trigger the weapon's name and " Proc". Bonereaver's Edge is the same
/// through itemhelpers.CreateWeaponProcTrigger, whose handler activates the stacking aura
/// `MakeStackingAura` builds and adds a stack.
pub(crate) const WEAPON_AURA_PROC_AURAS: [&str; 3] =
    ["Sword of Zeal", "Argent Avenger", "Bonereaver's Edge"];

/// weapon_procs.go `weaponAuraProcStacks`: the weapon procs above whose handler also adds a stack
/// of the aura.
const WEAPON_AURA_PROC_STACKS: [&str; 1] = ["Bonereaver's Edge"];

/// weapon_procs.go `weaponAuraProcEffects`: a weapon proc on landed hits at the weapon's proc
/// manager whose handler, a spell batch window later, activates the aura and, for a weapon that
/// stacks it, adds a stack. The aura logs nothing but the generic lines, and its stat changes are
/// read as stat auras, by stack where the stats follow them.
fn weapon_aura_proc_effects(env: &Environment, unrepresented: &mut Vec<String>) -> Vec<Value> {
    let mut effects = Vec::new();
    for name in WEAPON_AURA_PROC_AURAS {
        let Some(trigger) = aura_named(env, &format!("{name} Proc")) else {
            continue;
        };
        let trigger = env.sim.aura(trigger);
        let aura = aura_named(env, name).map(|aura| env.sim.aura(aura));
        let stacks = WEAPON_AURA_PROC_STACKS.contains(&name);
        let (Some(dpm), None, Some(aura)) = (trigger.dpm.as_ref(), trigger.icd, aura) else {
            unrepresented.push(format!("{name}'s proc is not a chance on hit for its aura"));
            continue;
        };
        if (aura.max_stacks > 0) != stacks {
            unrepresented.push(format!("{name}'s proc is not a chance on hit for its aura"));
            continue;
        }
        let mut effect = json!({
            "kind": "stat_proc", "trigger_aura": trigger.label, "rng_label": trigger.label,
            "aura": aura.label,
            "chances": dpm_chances(env, dpm, |spell| {
                !spell.flags.matches(SpellFlag::SUPPRESS_WEAPON_PROCS)
            }),
        });
        if stacks {
            effect["add_stack"] = json!(true);
        }
        effects.push(effect);
    }
    effects
}

/// weapon_procs.go `lobotomizerEffects`: common/forever/items_weapons.go The Lobotomizer: a weapon
/// proc on landed hits, at the weapon's proc manager, that casts Brain Damage (1290950), a
/// physical spell of the melee defense type that rolls 200 to 300 on the magic hit table with a
/// crit, at once on the unit hit.
fn lobotomizer_effects(env: &Environment, unrepresented: &mut Vec<String>) -> Vec<Value> {
    let Some(aura) = aura_named(env, "The Lobotomizer Proc") else {
        return Vec::new();
    };
    let position = spell_position(env, &ActionId::spell(1290950));
    let a = env.sim.aura(aura);
    let (Some(position), Some(dpm), false) = (position, a.dpm.as_ref(), a.icd.is_some()) else {
        unrepresented.push("The Lobotomizer's proc has no proc manager or spell".to_string());
        return Vec::new();
    };
    let chances = dpm_chances(env, dpm, |spell| {
        !spell.flags.matches(SpellFlag::SUPPRESS_WEAPON_PROCS)
    });
    let triggers: Vec<Value> = chances
        .iter()
        .map(|chance| chance["spell"].clone())
        .collect();
    vec![json!({
        "kind": "spell_data_damage_proc", "trigger_aura": a.label, "trigger_spells": triggers,
        "landed_only": true, "require_damage": false, "proc_chance": 1.0, "chances": chances,
        "spell": position, "average": 0.0, "variance": 0.0, "roll": [200.0, 300.0],
        "can_crit": true,
    })]
}

/// weapon_procs.go `ebonHiltEffects`: common/classic/items_weapons.go Ebon Hilt of Marduk: a weapon
/// proc on landed hits, at the weapon's proc manager, that casts Corruption (18656): a magic hit
/// roll without damage whose landing starts a damage over time of 28 every 3 seconds for 3 ticks
/// (`Dot.Snapshot` of a flat amount, ticking `OutcomeTick` on current stats, which a spell without
/// a spell power share deals as it would any periodic amount). The item also holds a permanent
/// aura that lowers the wearer's threat by 1%, which the player's threat multiplier already
/// carries.
fn ebon_hilt_effects(env: &Environment, unrepresented: &mut Vec<String>) -> Vec<Value> {
    let Some(aura) = aura_named(env, "Ebon Hilt of Marduk Proc") else {
        return Vec::new();
    };
    let position = spell_position(env, &ActionId::spell(18656));
    let a = env.sim.aura(aura);
    let (Some(position), Some(dpm), false) = (position, a.dpm.as_ref(), a.icd.is_some()) else {
        unrepresented.push("Ebon Hilt of Marduk's proc has no proc manager or spell".to_string());
        return Vec::new();
    };
    if env.sim.spell(spell_at(env, position)).defense_type != DefenseType::Magic {
        unrepresented.push("Ebon Hilt of Marduk's proc has no proc manager or spell".to_string());
        return Vec::new();
    }
    let chances = dpm_chances(env, dpm, |spell| {
        !spell.flags.matches(SpellFlag::SUPPRESS_WEAPON_PROCS)
    });
    let triggers: Vec<Value> = chances
        .iter()
        .map(|chance| chance["spell"].clone())
        .collect();
    vec![json!({
        "kind": "spell_data_damage_proc", "trigger_aura": a.label, "trigger_spells": triggers,
        "landed_only": true, "require_damage": false, "proc_chance": 1.0, "chances": chances,
        "spell": position, "average": 0.0, "variance": 0.0, "can_crit": false,
        "periodic": {
            "tick_base": 28.0, "tick_outcome": "tick", "with_direct": false,
            "application": "magic_hit",
        },
    })]
}

/// melee_procs.go `setStatProcs`: set bonuses common/forever/item_sets_classic.go `setStatProc`
/// builds: a proc trigger on the set bonus aura, rolling its proc manager on the hits it hears,
/// that activates a temporary stats aura a batch window later.
fn set_stat_proc_effects(env: &Environment, unrepresented: &mut Vec<String>) -> Vec<Value> {
    let mut effects = Vec::new();
    // (set aura, trigger name, aura, aura spell, stats, proc mask)
    let procs: [(&str, &str, &str, i32, Stats, ProcMask); 1] = [(
        "Lightforge Armor 5P",
        "Item - Crusader's Wrath Proc - Lightforge Armor",
        "Crusader's Wrath",
        27499,
        Stats::from_pairs(&[(Stat::SpellDamage, 65.0), (Stat::HealingPower, 65.0)]),
        ProcMask::MELEE_WHITE_HIT,
    )];
    for (set_aura, name, aura_label, aura_id, stats, mask) in procs {
        let Some(set) = aura_named(env, set_aura) else {
            continue;
        };
        let dpm = env.sim.aura(set).dpm.as_ref();
        let (Some(dpm), true) = (dpm, aura_named(env, aura_label).is_some()) else {
            unrepresented.push(format!("{set_aura} has no proc manager"));
            continue;
        };
        let id = ActionId::spell(aura_id);
        effects.push(json!({
            "kind": "stat_proc", "trigger_aura": set_aura, "rng_label": name, "aura": aura_label,
            "chances": dpm_chances(env, dpm, |spell| {
                spell.proc_mask.matches(mask) && !spell.flags.matches(SpellFlag::PROC)
            }),
            "gain_log": format!("Gained {} from {}.", flat_string(&stats), action_id_string(&id)),
            "expire_log": format!("Lost {} from fading {}.", flat_string(&stats), action_id_string(&id)),
        }));
    }
    effects
}

/// melee_procs.go `meleeProcEffects`: the weapon, item and set procs that act on melee hits, in
/// Go's order. The classic items' effects (Crusader, Dragon's Call, Sulfuras) and the raid
/// buffs' (the Windfury totem) belong to the ports of those packages and sit between the extra
/// attack procs and the chili.
pub(crate) fn melee_proc_effects(env: &Environment, unrepresented: &mut Vec<String>) -> Vec<Value> {
    let mut effects = spell_data_damage_proc_effects(env, unrepresented);
    effects.extend(set_melee_damage_proc_effects(env, unrepresented));
    effects.extend(spell_data_heal_proc_effects(env, unrepresented));
    effects.extend(weapon_enchant_damage_proc_effects(env, unrepresented));
    effects.extend(proc_damage_item_effects(env, unrepresented));
    effects.extend(weapon_damage_proc_effects(env, unrepresented));
    effects.extend(proc_oil_effects(env, unrepresented));
    effects.extend(lobotomizer_effects(env, unrepresented));
    effects.extend(ebon_hilt_effects(env, unrepresented));
    effects.extend(set_stat_proc_effects(env, unrepresented));
    super::classic_export::crusader_effect(env, unrepresented, &mut effects);
    // common/classic/items_weapons.go Ironfoe (11684) and common/forever/items_trinkets.go Hand
    // of Justice (11815): proc triggers on landed melee hits, Go literal chances, with the aura's
    // cooldown, whose handlers grant two and one extra main hand attacks at once.
    for (label, chance, attacks) in [
        ("Fury of Forgewright", 0.06, 2),
        ("Hand of Justice", 0.01, 1),
    ] {
        if let Some(aura) = aura_named(env, label) {
            effects.push(json!({
                "kind": "extra_attack_proc", "trigger_aura": env.sim.aura(aura).label,
                "proc_chance": chance, "attacks": attacks,
            }));
        }
    }
    effects.extend(flurry_axe_effects(env, unrepresented));
    effects.extend(weapon_aura_proc_effects(env, unrepresented));
    effects.extend(chance_on_hit_item_effects(env, unrepresented));
    super::classic_export::thunderfury_effect(env, unrepresented, &mut effects);
    super::classic_export::dragons_call_effect(env, unrepresented, &mut effects);
    super::classic_export::sulfuras_effect(env, unrepresented, &mut effects);
    if let Some(chili) = super::consumable_effects::dragonbreath_chili_effect(env) {
        effects.push(chili);
    }
    effects.extend(super::buffs::windfury_totem_effect(env, unrepresented));
    effects.extend(party_flametongue_totem_effect(env, unrepresented));
    effects
}

/// The effects `prepare` appends after the common effects and the inert pets: the melee, gear,
/// spell data and energy procs, in Go's order. The stat auras follow in `export`.
pub(crate) fn item_proc_effects(env: &Environment, unrepresented: &mut Vec<String>) -> Vec<Value> {
    let mut effects = melee_proc_effects(env, unrepresented);
    effects.extend(super::oil_of_immolation::oil_of_immolation_effect(env));
    effects.extend(gear_proc_effects(env, unrepresented));
    effects.extend(spell_data_stat_proc_effects(env, unrepresented));
    effects.extend(energy_proc_effects(env, unrepresented));
    effects.extend(super::healer_gear::export_effects(env, unrepresented));
    effects
}

// ---------------------------------------------------------------------------------------------
// gear_procs.go and energy_procs.go.
// ---------------------------------------------------------------------------------------------

/// gear_procs.go `gearProcEffects`: shared gear procs of common/forever: Battlegear of Valor's
/// Warrior's Resolve and the Puncture Armor weapon procs.
pub(crate) fn gear_proc_effects(env: &Environment, unrepresented: &mut Vec<String>) -> Vec<Value> {
    let mut effects = Vec::new();
    // Under Forever's rules: Mark of the Champion's bonus against Undead and Demons, when every
    // target is of one of those types (Go reads each target's own type). Go's melee attack power
    // reads the bonus's ranged attack power, which the item sets to the same value.
    let undead_or_demon = env.encounter.targets.iter().all(|&target| {
        matches!(
            env.sim.unit(target).mob_type.as_str(),
            "MobTypeUndead" | "MobTypeDemon"
        )
    });
    for (label, attack_power, spell_damage) in [
        ("Mark of the Champion (Physical)", 150.0, 0.0),
        ("Mark of the Champion (Spell)", 0.0, 85.0),
    ] {
        if env.sim.rules != super::sim::Rules::Forever || !undead_or_demon {
            break;
        }
        if aura_named(env, label).is_some() {
            effects.push(json!({
                "kind": "mob_type_bonus", "aura": label, "attack_power": attack_power,
                "ranged_attack_power": attack_power, "spell_damage": spell_damage,
            }));
        }
    }
    // Under Forever's rules (Go's exporter refuses the swap): with an item swap, the item and
    // enchant speed auras the swap gains and expires, as core/enchant_speed.go attaches them.
    let swaps = env.sim.item_swap(env.player).is_some();
    if swaps && env.sim.rules == super::sim::Rules::Forever {
        for &(aura, [melee, ranged, cast]) in &env.sim.speed_auras {
            if env.sim.aura(aura).unit != env.player {
                continue;
            }
            effects.push(json!({
                "kind": "speed_aura", "aura": env.sim.aura(aura).label,
                "melee_multiplier": melee, "ranged_multiplier": ranged, "cast_multiplier": cast,
            }));
        }
    }
    // item_sets_classic.go Battlegear of Valor 5 piece, Warrior's Resolve: landed melee hits roll
    // a legacy one proc a minute manager under the trigger's name; a batch window later the
    // handler heals Roll(88, 133) and, with a rage bar, gives 10 rage, both Go literals.
    if let Some(set) = aura_named(env, "Battlegear of Valor 5P") {
        match env.sim.aura(set).dpm.as_ref() {
            None => unrepresented.push("Battlegear of Valor 5P has no proc manager".to_string()),
            Some(dpm) => {
                let rage = if env.sim.unit(env.player).rage_bar.enabled {
                    10.0
                } else {
                    0.0
                };
                effects.push(json!({
                    "kind": "health_rage_proc", "trigger_aura": env.sim.aura(set).label,
                    "rng_label": "Warrior's Resolve",
                    "chances": dpm_chances(env, dpm, |spell| {
                        spell.proc_mask.matches(ProcMask::MELEE) && !spell.flags.matches(SpellFlag::PROC)
                    }),
                    "heal_min": 88.0, "heal_max": 133.0, "rage": rage,
                    "metrics_action_id": json!({"spell_id": 450589}),
                }));
            }
        }
    }
    // classic items_weapons.go Runeblade of Baron Rivendare, under Forever's rules (Go's exporter
    // does not run the regeneration): Unholy Aura gives 60 health every 5 seconds while it holds.
    if let Some(aura) =
        aura_named(env, "Unholy Aura").filter(|_| env.sim.rules == super::sim::Rules::Forever)
    {
        effects.push(json!({
            "kind": "health_regen_aura", "aura": env.sim.aura(aura).label,
            "period_ns": 5 * super::sim::SECOND, "health": 60.0,
            "metrics_action_id": json!({"spell_id": 17625}),
        }));
    }
    // warrior items.go Battlegear of Heroism 4 piece, under Forever's rules (Go's exporter leaves
    // its listener unclaimed): Warrior's Resolve, as Battlegear of Valor's.
    if let Some(set) = aura_named(env, "Battlegear of Heroism 4P")
        .filter(|_| env.sim.rules == super::sim::Rules::Forever)
    {
        match env.sim.aura(set).dpm.as_ref() {
            None => unrepresented.push("Battlegear of Heroism 4P has no proc manager".to_string()),
            Some(dpm) => {
                let rage = if env.sim.unit(env.player).rage_bar.enabled {
                    10.0
                } else {
                    0.0
                };
                effects.push(json!({
                    "kind": "health_rage_proc", "trigger_aura": env.sim.aura(set).label,
                    "rng_label": "Warrior's Resolve",
                    "chances": dpm_chances(env, dpm, |spell| {
                        spell.proc_mask.matches(ProcMask::MELEE) && !spell.flags.matches(SpellFlag::PROC)
                    }),
                    "heal_min": 88.0, "heal_max": 133.0, "rage": rage,
                    "metrics_action_id": json!({"spell_id": 450589}),
                }));
            }
        }
    }
    // rogue items.go Darkmantle Armor 4 piece, Rogue Armor Energize, under Forever's rules (Go's
    // exporter leaves the listener unclaimed): landed white hits roll a legacy one proc a minute
    // manager under the trigger's name; a batch window later a player with an energy bar gains
    // 20 energy.
    if let Some(set) = aura_named(env, "Darkmantle Armor 4P")
        .filter(|_| env.sim.rules == super::sim::Rules::Forever)
    {
        match env.sim.aura(set).dpm.as_ref() {
            None => unrepresented.push("Darkmantle Armor 4P has no proc manager".to_string()),
            Some(dpm) => effects.push(json!({
                "kind": "energy_proc", "trigger_aura": env.sim.aura(set).label,
                "rng_label": "Rogue Armor Energize",
                "chances": dpm_chances(env, dpm, |spell| !spell.flags.matches(SpellFlag::PROC)),
                "energy": 20.0, "metrics_action_id": json!({"spell_id": 27787}),
            })),
        }
    }
    // classic items_trinkets.go Darkmoon Card: Heroism: landed melee hits roll a legacy two procs a
    // minute manager under the trigger's name; a batch window later the handler heals
    // Roll(120, 180), a Go literal, and gives no rage.
    if let Some(aura) = aura_named(env, "Darkmoon Card: Heroism") {
        let a = env.sim.aura(aura);
        match (a.dpm.as_ref(), a.icd) {
            (Some(dpm), None) => effects.push(json!({
                "kind": "health_rage_proc", "trigger_aura": a.label, "rng_label": a.label,
                "chances": dpm_chances(env, dpm, |spell| {
                    spell.proc_mask.matches(ProcMask::MELEE) && !spell.flags.matches(SpellFlag::PROC)
                }),
                "heal_min": 120.0, "heal_max": 180.0, "rage": 0.0,
                "metrics_action_id": json!({"spell_id": 23682}),
            })),
            _ => unrepresented.push("Darkmoon Card: Heroism has no proc manager".to_string()),
        }
    }
    // items_weapons.go Bashguuder and Rivenspike: a weapon proc at two procs a minute of the
    // weapon's speed on landed hits; a batch window later the handler activates the target's
    // Puncture Armor and adds a stack, and each stack change moves the target's armor through
    // AddStatDynamic. The armor at each stack count is read from a separate reset simulation.
    // stat_bonus_procs_auto_gen.go Annihilator is the same through
    // shared.NewSpellDataDebuffProc: the trigger carries the item's name, the row's Armor Shatter
    // takes 165 armor a stack, and the handler applies it at once to the unit hit.
    for (label, debuff_label, immediate) in [
        ("Bashguuder Proc", "Puncture Armor", false),
        ("Rivenspike Proc", "Puncture Armor", false),
        ("Annihilator", "Armor Shatter 16928", true),
    ] {
        let Some(trigger) = aura_named(env, label) else {
            continue;
        };
        let Some(dpm) = env.sim.aura(trigger).dpm.clone() else {
            unrepresented.push(format!(
                "{} has no proc manager",
                env.sim.aura(trigger).label
            ));
            continue;
        };
        let mut scratch = env.fresh();
        let target = scratch.encounter.targets[0];
        let Some(debuff) = scratch.sim.get_aura(target, debuff_label) else {
            unrepresented.push(format!("{label} has no {debuff_label} aura"));
            continue;
        };
        let mut armor = vec![0.0];
        let base = scratch.sim.stat(target, Stat::Armor);
        scratch.sim.activate(debuff);
        for stacks in 1..=scratch.sim.aura(debuff).max_stacks {
            scratch.sim.set_stacks(debuff, stacks);
            armor.push(scratch.sim.stat(target, Stat::Armor) - base);
        }
        let mut effect = json!({
            "kind": "armor_debuff_proc", "trigger_aura": label, "rng_label": label,
            "chances": dpm_chances(env, &dpm, |spell| {
                !spell.flags.matches(SpellFlag::SUPPRESS_WEAPON_PROCS)
            }),
            "aura": scratch.sim.aura(debuff).label, "armor_by_stacks": armor,
        });
        if immediate {
            effect["immediate"] = json!(true);
        }
        effects.push(effect);
    }
    effects
}

/// energy_procs.go `energyProcEffects`: common/forever/item_sets_classic.go Shadowcraft Armor
/// (5): a set bonus proc trigger on landed white hits at one proc a minute of each hand's speed,
/// whose handler waits a spell batch window and restores 20 energy to a character with an energy
/// bar.
pub(crate) fn energy_proc_effects(
    env: &Environment,
    unrepresented: &mut Vec<String>,
) -> Vec<Value> {
    let mut effects = Vec::new();
    if let Some(aura) = aura_named(env, "Shadowcraft Armor 5P") {
        let Some(dpm) = env.sim.aura(aura).dpm.as_ref() else {
            unrepresented.push("Shadowcraft Armor's energize has no proc manager".to_string());
            return effects;
        };
        effects.push(json!({
            "kind": "energize_proc", "trigger_aura": env.sim.aura(aura).label,
            "rng_label": "Rogue Armor Energize",
            "chances": dpm_chances(env, dpm, |spell| {
                !spell.flags.matches(SpellFlag::PROC) && spell.proc_mask.matches(ProcMask::MELEE_WHITE_HIT)
            }),
            "energy": 20.0, "metrics_action_id": json!({"spell_id": 27787}),
            "delay_ns": SPELL_BATCH_WINDOW,
        }));
    }
    // item_sets_classic.go Stormshroud Armor (3), Revitalize: a set bonus proc trigger on landed
    // melee hits, none flagged a proc, at the client's 4% (23863) rolled under its name, whose
    // handler a spell batch window later restores 30 energy (23864). Go attaches it only to a
    // character with an energy bar.
    if let Some(aura) = aura_named(env, "Stormshroud Armor 3P") {
        let listener = ProcTrigger {
            callback: CallbackMask::ON_SPELL_HIT_DEALT,
            outcome: HitOutcome::LANDED,
            proc_mask: ProcMask::MELEE,
            ..ProcTrigger::default()
        };
        let chance = super::resolve_proc::stated_chance(must_find(23863));
        let chances: Vec<Value> = proc_trigger_spells(env, &listener)
            .into_iter()
            .map(|spell| json!({"spell": spell, "chance": chance}))
            .collect();
        let a = env.sim.aura(aura);
        if a.dpm.is_some() || a.icd.is_some() {
            unrepresented.push("Stormshroud Armor's energize is not a chance proc".to_string());
        } else {
            effects.push(json!({
                "kind": "energize_proc", "trigger_aura": a.label, "rng_label": "Revitalize",
                "chances": chances, "energy": 30.0,
                "metrics_action_id": json!({"spell_id": 23864}), "delay_ns": SPELL_BATCH_WINDOW,
            }));
        }
    }
    effects.extend(beaststalker_mana_proc(env));
    effects
}

/// item_sets_classic.go Beaststalker Armor (5), under Forever's rules on a class other than a
/// Hunter: the set (Dungeon Set 1) is not class locked in Forever (classMask 0), and its 5% chance
/// on landed melee and ranged autoattacks to restore 200 mana (450577) works for any wearer with a
/// mana bar. The Hunter domain exports it as `hunter_set_mana_proc`; Go attaches its handler
/// only there, which left the trigger without an effect on anyone else.
fn beaststalker_mana_proc(env: &Environment) -> Option<Value> {
    if env.sim.rules != super::sim::Rules::Forever
        || env.sim.character(env.player).class == "ClassHunter"
    {
        return None;
    }
    let aura = aura_named(env, "Beaststalker Armor 5P")?;
    let listener = ProcTrigger {
        callback: CallbackMask::ON_SPELL_HIT_DEALT,
        outcome: HitOutcome::LANDED,
        proc_mask: ProcMask::WHITE_HIT,
        ..ProcTrigger::default()
    };
    let chances: Vec<Value> = proc_trigger_spells(env, &listener)
        .into_iter()
        .map(|spell| json!({"spell": spell, "chance": 0.05}))
        .collect();
    let label = &env.sim.aura(aura).label;
    Some(json!({
        "kind": "mana_proc", "trigger_aura": label, "rng_label": label,
        "chances": chances, "mana": 200.0,
        "metrics_action_id": json!({"spell_id": 450577}), "delay_ns": SPELL_BATCH_WINDOW,
    }))
}
