//! The Rust side of tools/oracle-v2/main.go `prepare`: the prepared v2 description of a reset
//! simulation, written as the Go exporter writes it.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::contracts::prepared_v2::{ActionId, CONTRACT, SCHEMA_VERSION};
use crate::contracts::request::Message;

use super::attack::Weapon;
use super::env::Environment;
use super::sim::{AuraId, Cooldown, Rules, Sim, TimerId, UnitId};
use super::spell::SpellFlag;
use super::stats::{PseudoStats, Stat, Stats, SCHOOL_LEN};
use super::weapon_skill::{self as skill, TableUnits};
use super::Refusal;

const SCHOOL_NAMES: [&str; SCHOOL_LEN] = [
    "none", "physical", "arcane", "fire", "frost", "holy", "nature", "shadow",
];

/// Shared Go timers named in first-seen order: `timerNames`.
#[derive(Default)]
pub(crate) struct TimerNames {
    names: BTreeMap<TimerId, String>,
}

impl TimerNames {
    pub(crate) fn name(&mut self, timer: TimerId) -> String {
        let next = self.names.len();
        self.names
            .entry(timer)
            .or_insert_with(|| format!("timer-{next}"))
            .clone()
    }

    pub(crate) fn cooldown(&mut self, cd: &Cooldown) -> Value {
        match cd.timer {
            None => Value::Null,
            Some(timer) => json!({"timer": self.name(timer), "duration_ns": cd.duration}),
        }
    }
}

/// Go `actionID`: nil for an empty action.
pub(crate) fn action_id(id: Option<&ActionId>) -> Value {
    match id {
        None => Value::Null,
        Some(id) if id.spell_id == 0 && id.item_id == 0 && id.other_id.is_empty() => {
            // Go IsEmptyAction ignores the tag.
            Value::Null
        }
        Some(id) => serde_json::to_value(id).expect("an action ID serializes"),
    }
}

/// Go `spellActionID`: a tag alone still names an action.
fn spell_action_id(id: &ActionId) -> Value {
    let empty = id.spell_id == 0 && id.item_id == 0 && id.other_id.is_empty();
    if empty && id.tag != 0 {
        return json!({"tag": id.tag});
    }
    action_id(Some(id))
}

fn school_values(values: &[f64; SCHOOL_LEN]) -> Value {
    Value::Object(
        SCHOOL_NAMES
            .iter()
            .zip(values)
            .map(|(name, value)| (name.to_string(), json!(value)))
            .collect(),
    )
}

pub(crate) fn stat_values(stats: &Stats) -> Value {
    Value::Object(
        Stat::ALL
            .iter()
            .map(|stat| (stat.name().to_string(), json!(stats[*stat])))
            .collect(),
    )
}

pub(crate) fn export_pseudo(p: &PseudoStats) -> Value {
    json!({
        "spell_cost_percent_modifier": p.spell_cost_percent_modifier,
        "cast_speed_multiplier": p.cast_speed_multiplier,
        "spirit_regen_rate_casting": p.spirit_regen_rate_casting,
        "force_full_spirit_regen": p.force_full_spirit_regen,
        "spirit_regen_multiplier": p.spirit_regen_multiplier,
        "threat_multiplier": p.threat_multiplier,
        "damage_dealt_multiplier": p.damage_dealt_multiplier,
        "school_damage_dealt_multiplier": school_values(&p.school_damage_dealt_multiplier),
        "dot_damage_multiplier_additive": p.dot_damage_multiplier_additive,
        "crit_damage_multiplier": p.crit_damage_multiplier,
        "damage_taken_multiplier": p.damage_taken_multiplier,
        "school_damage_taken_multiplier": school_values(&p.school_damage_taken_multiplier),
        "school_bonus_spell_damage": school_values(&p.school_bonus_spell_damage),
        "school_bonus_hit_chance": school_values(&p.school_bonus_hit_chance),
        "bonus_spell_damage_taken": p.bonus_spell_damage_taken,
        "bonus_spell_crit_percent_taken": p.bonus_spell_crit_percent_taken,
        "reduced_crit_taken_percent": p.reduced_crit_taken_percent,
        "incapacitated": p.incapacitated,
    })
}

/// Go `exportAuras`.
pub(crate) fn export_auras(sim: &Sim, unit: UnitId, timers: &mut TimerNames) -> Value {
    let auras = &sim.unit(unit).auras;
    let position: BTreeMap<AuraId, usize> =
        auras.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let mut out = Vec::new();
    for (i, id) in auras.iter().enumerate() {
        let aura = sim.aura(*id);
        let mut exported = Map::new();
        exported.insert("label".into(), json!(aura.label));
        if !aura.tag.is_empty() {
            exported.insert("tag".into(), json!(aura.tag));
        }
        exported.insert("action_id".into(), action_id(aura.action_id.as_ref()));
        if aura.action_id_for_proc.is_some() {
            let proc = action_id(aura.action_id_for_proc.as_ref());
            if !proc.is_null() {
                exported.insert("action_id_for_proc".into(), proc);
            }
        }
        exported.insert("duration_ns".into(), json!(aura.duration));
        exported.insert("max_stacks".into(), json!(aura.max_stacks));
        exported.insert("active".into(), json!(aura.active));
        exported.insert("stacks".into(), json!(aura.stacks));
        exported.insert("callbacks".into(), json!(aura.callback_names()));
        exported.insert(
            "icd".into(),
            aura.icd
                .as_ref()
                .map_or(Value::Null, |icd| timers.cooldown(icd)),
        );
        exported.insert(
            "exclusive_effects".into(),
            json!(aura.exclusive_effects.len()),
        );
        // aura.go Activate: a reset that activated the aura counted a proc.
        if aura.on_reset.is_some() && !aura.active && aura.procs > 0 {
            for effect in &aura.exclusive_effects {
                let category = sim.effects[effect.0].category;
                let Some(holder) = sim.category_active_aura(category) else {
                    continue;
                };
                if holder == *id || !sim.aura(holder).active || sim.aura(holder).unit != aura.unit {
                    continue;
                }
                if position[&holder] > i {
                    exported.insert("displaced_by".into(), json!(sim.aura(holder).label));
                } else {
                    exported.insert("blocked_at_reset".into(), json!(true));
                }
            }
        }
        let empty = |id: &Option<ActionId>| {
            id.as_ref()
                .is_none_or(|id| id.spell_id == 0 && id.item_id == 0 && id.other_id.is_empty())
        };
        if empty(&aura.metrics_id) && !empty(&aura.action_id) {
            exported.insert("metrics_hidden".into(), json!(true));
        }
        let memberships: Vec<Value> = aura
            .exclusive_effects
            .iter()
            .map(|effect| {
                let ee = &sim.effects[effect.0];
                let category = &sim.categories[ee.category.0];
                let position = category
                    .effects
                    .iter()
                    .position(|e| e == effect)
                    .map_or(-1, |p| p as i64);
                json!({"category": category.name, "single_aura": category.single_aura,
                       "priority": ee.priority, "position": position})
            })
            .collect();
        if !memberships.is_empty() {
            exported.insert("exclusive_memberships".into(), Value::Array(memberships));
        }
        out.push(Value::Object(exported));
    }
    Value::Array(out)
}

/// Go `exportSpell`.
pub(crate) fn export_spell_of(
    env: &Environment,
    spell_id: super::sim::SpellId,
    timers: &mut TimerNames,
    unrepresented: &mut Vec<String>,
) -> Value {
    let sim = &env.sim;
    let spell = sim.spell(spell_id);
    let cost = match &spell.cost {
        None => Value::Null,
        Some(cost) => {
            if cost.refund > 0.0 && cost.refund_to_own_metrics {
                unrepresented.push(format!(
                    "spell {} refunds {} to its own metrics",
                    spell.action_id,
                    cost.resource.name()
                ));
            }
            let mut out = json!({
                "resource": cost.resource.name(),
                "base_cost": cost.base_cost,
                "flat_modifier": cost.flat_modifier,
                "percent_modifier": cost.percent_modifier,
                "additive_percent_modifier": cost.additive_percent_modifier,
            });
            if cost.refund != 0.0 {
                out["refund"] = json!(cost.refund);
            }
            out
        }
    };
    let dot = if let Some(dot) = spell.aoe_dot {
        describe_dot(sim, dot, "self")
    } else if !spell.dots.is_empty() {
        let target = env.encounter.targets[0];
        match spell.dots[sim.unit(target).unit_index as usize] {
            Some(dot) => describe_dot(sim, dot, "target"),
            None => {
                unrepresented.push(format!(
                    "spell {} has a dot on another unit",
                    spell.action_id
                ));
                Value::Null
            }
        }
    } else {
        Value::Null
    };
    let class_spell =
        class_spell_name(env, spell.class_spell_mask, &spell.action_id, unrepresented);
    let mut out = Map::new();
    out.insert("action_id".into(), spell_action_id(&spell.action_id));
    out.insert("rank".into(), json!(spell.rank));
    out.insert("school".into(), json!(spell.spell_school));
    out.insert("defense_type".into(), json!(spell.defense_type.name()));
    out.insert("proc_mask".into(), json!(spell.proc_mask.names()));
    out.insert("flags".into(), json!(spell.flags.names()));
    if !class_spell.is_empty() {
        out.insert("class_spell".into(), json!(class_spell));
    }
    out.insert("missile_speed".into(), json!(spell.missile_speed));
    out.insert("cost".into(), cost);
    let cast = &spell.default_cast;
    out.insert(
        "default_cast".into(),
        json!({"cost": cast.cost, "gcd_ns": cast.gcd, "gcd_min_ns": cast.gcd_min,
               "cast_time_ns": cast.cast_time, "non_empty": cast.non_empty}),
    );
    out.insert(
        "cast_kind".into(),
        // Go reads the final state: a range wraps the extra cast condition after the cast
        // function is chosen, which the exporter's own test then sees.
        json!(if !spell.default_cast.is_empty() {
            "full"
        } else if !spell.has_extra_cast_condition
            && spell.cd.timer.is_none()
            && spell.shared_cd.timer.is_none()
            && !spell.has_cast_requirement
        {
            "autos_or_procs"
        } else {
            "simple"
        }),
    );
    out.insert("ignore_haste".into(), json!(spell.ignore_haste));
    out.insert(
        "has_extra_cast_condition".into(),
        json!(spell.has_extra_cast_condition),
    );
    out.insert(
        "has_cast_requirement".into(),
        json!(spell.has_cast_requirement),
    );
    if let Some(auras) = requirement_auras(sim, spell) {
        out.insert("requirement_auras".into(), json!(auras));
    }
    out.insert("min_range".into(), json!(spell.min_range));
    out.insert("max_range".into(), json!(spell.max_range));
    out.insert("max_charges".into(), json!(spell.max_charges));
    out.insert("cd".into(), timers.cooldown(&spell.cd));
    out.insert("shared_cd".into(), timers.cooldown(&spell.shared_cd));
    out.insert("bonus_hit_percent".into(), json!(spell.bonus_hit_percent));
    out.insert("bonus_crit_percent".into(), json!(spell.bonus_crit_percent));
    out.insert("bonus_spell_damage".into(), json!(spell.bonus_spell_damage));
    out.insert(
        "bonus_expertise_percent".into(),
        json!(spell.bonus_expertise_percent),
    );
    out.insert(
        "cast_time_multiplier".into(),
        json!(spell.cast_time_multiplier),
    );
    out.insert("cd_multiplier".into(), json!(spell.cd_multiplier));
    out.insert("damage_multiplier".into(), json!(spell.damage_multiplier));
    out.insert(
        "damage_multiplier_additive".into(),
        json!(spell.damage_multiplier_additive),
    );
    out.insert(
        "direct_damage_multiplier_additive".into(),
        json!(spell.direct_damage_multiplier_additive),
    );
    out.insert(
        "crit_multiplier_pct".into(),
        json!(spell.crit_multiplier_pct),
    );
    out.insert(
        "crit_multiplier_additive".into(),
        json!(spell.crit_multiplier_additive),
    );
    out.insert("bonus_base_damage".into(), json!(spell.bonus_base_damage));
    out.insert("bonus_coefficient".into(), json!(spell.bonus_coefficient));
    out.insert("threat_multiplier".into(), json!(spell.threat_multiplier));
    out.insert("flat_threat_bonus".into(), json!(spell.flat_threat_bonus));
    out.insert("pushback_resist".into(), json!(spell.pushback_resist));
    out.insert("dot".into(), dot);
    out.insert("damage_effect".into(), Value::Null);
    if spell.metric_splits > 1 {
        out.insert("metric_splits".into(), json!(spell.metric_splits));
    }
    if !spell.rank_aliases.is_empty() {
        let aliases: Vec<Value> = spell.rank_aliases.iter().map(spell_action_id).collect();
        out.insert("rank_aliases".into(), Value::Array(aliases));
    }
    Value::Object(out)
}

/// Priest Shadowform's row, the only aura that puts the unit in a form at the pin.
const SHADOWFORM: i32 = 15473;

/// Go `requirementAuras`, shapeshift.go `castRequirementFailure` as the player auras whose form
/// the requirement refuses. The unit's form is 0 except while a Shadow priest's Shadowform is up
/// (the client's form 28); nothing else at the pin sets a form or `AutoUnshift`. A requirement
/// on caster auras, or one that refuses no form, is not represented.
fn requirement_auras(sim: &Sim, spell: &super::spell::Spell) -> Option<Vec<String>> {
    let r = spell.cast_requirement;
    if !spell.has_cast_requirement
        || r.caster_aura != 0
        || r.exclude_caster_aura != 0
        || !r.allows_form(0)
    {
        return None;
    }
    let mut auras = Vec::new();
    if let Some(aura) = sim.get_aura(spell.unit, "Shadowform") {
        let aura = sim.aura(aura);
        if aura
            .action_id
            .as_ref()
            .is_some_and(|id| id.spell_id == SHADOWFORM)
            && !r.allows_form(super::spelldata::must_find(SHADOWFORM).shapeshift_form())
        {
            auras.push(aura.label.clone());
        }
    }
    Some(auras)
}

fn describe_dot(sim: &Sim, dot: super::sim::DotId, unit: &str) -> Value {
    let d = &sim.dots[dot.0];
    json!({
        "unit": unit,
        "aura_label": sim.aura(d.aura).label,
        "base_tick_count": d.base_tick_count,
        "base_tick_length_ns": d.base_tick_length,
        "bonus_coefficient": d.bonus_coefficient,
        "periodic_damage_multiplier": d.periodic_damage_multiplier,
        "base_duration_multiplier": d.base_duration_multiplier,
        "base_duration_flat_ns": d.base_duration_flat,
        "affected_by_cast_speed": d.affected_by_cast_speed,
        "affected_by_real_haste": d.affected_by_real_haste,
        "haste_reduces_duration": d.haste_reduces_duration,
        "channeled": d.is_channeled,
    })
}

/// Go `classSpell`.
fn class_spell_name(
    env: &Environment,
    mask: i64,
    id: &ActionId,
    unrepresented: &mut Vec<String>,
) -> String {
    if mask == 0 {
        return env.agent.unmasked_spell(id).unwrap_or_default().to_string();
    }
    for entry in env.agent.class_spells() {
        if entry.mask == mask {
            return entry.name.to_string();
        }
    }
    unrepresented.push(format!("spell {id} has unnamed class mask {mask}"));
    String::new()
}

fn export_weapon(weapon: &Weapon) -> Value {
    json!({
        "base_damage_min": weapon.base_damage_min,
        "base_damage_max": weapon.base_damage_max,
        "attack_power_per_dps": weapon.attack_power_per_dps,
        "swing_speed": weapon.swing_speed,
        "normalized_swing_speed": weapon.normalized_swing_speed,
        "school": weapon.spell_school,
        "min_range": weapon.min_range,
        "max_range": weapon.max_range,
    })
}

/// Go `exportMelee` of the player.
fn export_melee(env: &Environment, unrepresented: &mut Vec<String>) -> Value {
    export_melee_of(
        env,
        env.player,
        env.agent.swing_replacement_keeps_swing(),
        unrepresented,
    )
}

/// Go `exportMelee`: the auto attacks of a player or a pet.
pub(crate) fn export_melee_of(
    env: &Environment,
    player: UnitId,
    keeps_swing: bool,
    unrepresented: &mut Vec<String>,
) -> Value {
    let sim = &env.sim;
    let target = env.encounter.targets[0];
    let table = env.attack_table(player, target);
    let aa = &sim.unit(player).auto_attacks;
    let distance = sim.unit(player).distance_from_target;
    let in_range = |w: &Weapon| {
        (w.min_range == 0.0 || w.min_range < distance)
            && (w.max_range == 0.0 || w.max_range >= distance)
    };
    let replaced = aa.auto_swing_melee && aa.replace_mh_swing;
    let class = sim
        .unit(player)
        .character
        .as_deref()
        .map_or("", |character| character.class.as_str());
    let describes = class == "ClassWarrior"
        || class == "ClassHunter"
        || (class == "ClassDruid" && sim.get_aura(player, "Maul Queue Aura").is_some());
    if replaced && in_range(&aa.mh) && !keeps_swing && !describes {
        unrepresented.push("main hand swings can be replaced".to_string());
    }
    let pseudo = &sim.unit(player).pseudo_stats;
    let defender = &sim.unit(target).pseudo_stats;
    // unit.go TotalRealRangedHasteMultiplier, which a dot hasted by real haste reads, has
    // RangedHasteMultiplier in it; Rust takes it as 1, as nothing at the pin changes it.
    if pseudo.ranged_haste_multiplier != 1.0 {
        unrepresented.push("a ranged haste multiplier is unsupported".to_string());
    }
    let t = sim.unit(target);
    let mut out = json!({
        "auto_swing_melee": aa.auto_swing_melee,
        "auto_swing_ranged": aa.auto_swing_ranged,
        "dual_wielding": aa.is_dual_wielding,
        "main_hand": export_weapon(&aa.mh),
        "off_hand": export_weapon(&aa.oh),
        "ranged": export_weapon(&aa.ranged),
        "base_miss_chance": table.base_miss_chance,
        "base_glance_chance": table.base_glance_chance,
        "glance_multiplier": table.glance_multiplier,
        "glance_spread": table.glance_spread,
        "hit_suppression": table.hit_suppression,
        "melee_crit_suppression": table.melee_crit_suppression,
        "ignore_armor": table.ignore_armor,
        "armor_ignore_factor": table.armor_ignore_factor,
        "in_front_of_target": pseudo.in_front_of_target,
        "attack_speed_multiplier": pseudo.attack_speed_multiplier,
        "melee_speed_multiplier": pseudo.melee_speed_multiplier,
        "dodge_reduction": pseudo.dodge_reduction,
        "disable_dw_miss_penalty": pseudo.disable_dw_miss_penalty,
        "defender_dodge": defender.base_dodge_chance + table.base_dodge_chance + t.stats[Stat::DodgePercent] / 100.0,
        "defender_parry": defender.base_parry_chance + table.base_parry_chance + t.stats[Stat::ParryPercent] / 100.0,
        "defender_block": defender.base_block_chance + table.base_block_chance + t.stats[Stat::BlockPercent] / 100.0,
        "defender_armor": defender.armor_multiplier * t.stats[Stat::Armor],
        "defender_block_reduction": t.stats[Stat::BlockValue] * defender.block_value_multiplier,
        "defender_bonus_attack_power": defender.bonus_attack_power,
        "defender_bonus_physical_damage_taken": defender.bonus_physical_damage_taken,
        "defender_reduced_physical_hit_taken": defender.reduced_physical_hit_taken_chance,
    });
    if replaced {
        out["replace_main_hand_swing"] = json!(true);
    }
    // Go weapon_skill.go: the table an attack with a hand reads where its weapon's skill moves it.
    let units = TableUnits {
        attacker_level: sim.unit(player).level,
        defender_type: t.unit_type,
        defender_level: t.level,
    };
    for (key, hand) in [
        ("main_hand_skill", skill::MAIN_HAND),
        ("off_hand_skill", skill::OFF_HAND),
        ("ranged_skill", skill::RANGED),
    ] {
        let bonus = sim.unit(player).weapon_skill[hand];
        if !skill::moves_table(units, bonus) {
            continue;
        }
        let (glance_multiplier, glance_spread) = skill::glance(table, units, bonus);
        out[key] = json!({
            "skill": bonus,
            "miss_chance": skill::miss_chance(table, units, bonus),
            "hit_suppression": skill::hit_suppression(table, units, bonus),
            "defender_dodge": defender.base_dodge_chance
                + skill::dodge_chance(table, units, bonus)
                + t.stats[Stat::DodgePercent] / 100.0,
            "glance_multiplier": glance_multiplier,
            "glance_spread": glance_spread,
        });
    }
    if aa.auto_swing_ranged {
        out["ranged_state"] = json!({
            "ranged_speed_multiplier": pseudo.ranged_speed_multiplier,
            "defender_bonus_ranged_attack_power": defender.bonus_ranged_attack_power,
        });
    }
    out
}

/// Go `metricsActions`.
pub(crate) fn metrics_actions(env: &Environment, unit: UnitId) -> Value {
    let sim = &env.sim;
    let mut seen: Vec<ActionId> = Vec::new();
    let mut out = Vec::new();
    for spell_id in &sim.unit(unit).spellbook {
        let spell = sim.spell(*spell_id);
        if spell.flags.matches(SpellFlag::NO_METRICS) {
            continue;
        }
        let ids: Vec<ActionId> = if spell.metric_splits > 1 {
            (0..spell.metric_splits)
                .map(|i| ActionId {
                    tag: i as i32,
                    ..spell.action_id.clone()
                })
                .collect()
        } else {
            vec![spell.action_id.clone()]
        };
        for id in ids {
            if seen.contains(&id) {
                continue;
            }
            seen.push(id.clone());
            let mut action = json!({
                "action_id": action_id(Some(&id)),
                "melee_metrics": spell.flags.matches(SpellFlag::MELEE_METRICS),
                "school": spell.spell_school,
            });
            if spell.flags.matches(SpellFlag::PASSIVE_SPELL) {
                action["passive"] = json!(true);
            }
            out.push(action);
        }
    }
    Value::Array(out)
}

/// Go `teardownMaxMana`: deactivate every aura in order and keep the lowest maximum mana.
fn teardown_max_mana(env: &mut Environment, unit: UnitId, unrepresented: &mut Vec<String>) -> f64 {
    let mut lowest = env.sim.max_mana(unit);
    loop {
        let active = env
            .sim
            .unit(unit)
            .auras
            .iter()
            .copied()
            .find(|id| env.sim.aura(*id).active);
        let Some(active) = active else {
            return lowest;
        };
        let before = env.sim.max_mana(unit);
        env.sim.deactivate(active);
        if env.sim.max_mana(unit) > before {
            unrepresented.push(format!(
                "aura {} raises maximum mana when it fades",
                env.sim.aura(active).label
            ));
        }
        lowest = lowest.min(env.sim.max_mana(unit));
    }
}

/// The spec's class options: `specClassOptions`.
fn class_options(player: &Message) -> Value {
    let Some((_, spec)) = player.oneof("spec") else {
        return json!({});
    };
    let crate::contracts::request::Value::Message(spec) = spec else {
        return json!({});
    };
    let Some(options) = spec.message("options") else {
        return json!({});
    };
    match options.message("class_options") {
        Some(class_options) => class_options.to_protojson(),
        None => json!({}),
    }
}

/// The talents a class's talents message sets: `talents`.
fn talent_values(talents: &Message) -> Value {
    let mut out = Map::new();
    for name in talents.set_fields() {
        let value = match talents.get(name) {
            Some(crate::contracts::request::Value::Bool(true)) => 1,
            Some(crate::contracts::request::Value::Int(value)) if *value != 0 => *value,
            _ => continue,
        };
        out.insert(name.to_string(), json!(value));
    }
    Value::Object(out)
}

/// Go damage_taken.go `playerTakesDamage`: the Goblin Sapper Charge's hit on the player, or a
/// target swinging at it.
pub(crate) fn player_takes_damage(env: &Environment) -> bool {
    let sapper_self = ActionId {
        item_id: super::common_effects::GOBLIN_SAPPER_ITEM,
        tag: 1,
        ..ActionId::default()
    };
    env.sim.get_spell(env.player, &sapper_self).is_some()
        || env.sim.unit(env.encounter.targets[0]).current_target == Some(env.player)
}

/// The effects `prepare` appends last: Eureka, the tank's pushback trigger, the listeners that
/// never act in scope, Chance of Death and the item listeners.
fn tail_effects(
    env: &mut Environment,
    unrepresented: &mut Vec<String>,
    earlier: &[Value],
) -> Result<Vec<Value>, Refusal> {
    let mut effects = Vec::new();
    // tools/oracle-v2/targets.go targetCopyNotes: Go keeps an internal cooldown per copy.
    if env.encounter.targets.len() > 1 {
        let first = env.encounter.targets[0];
        if let Some(aura) = env
            .sim
            .unit(first)
            .auras
            .iter()
            .find(|aura| env.sim.aura(**aura).icd.is_some())
        {
            return Err(Refusal::new(
                "targets",
                format!(
                    "target aura {} has an internal cooldown",
                    env.sim.aura(*aura).label
                ),
            ));
        }
    }
    effects.extend(super::racials::eureka_effect(env));
    let tanking = env.tanking();
    if tanking {
        if let Some(aura) = env.sim.get_aura(env.player, "Pushback trigger") {
            let _ = aura;
            let chance = env.sim.unit(env.player).pseudo_stats.pushback_chance;
            effects.push(
                json!({"kind": "pushback_trigger", "aura": "Pushback trigger", "chance": chance}),
            );
        }
    }
    let target = env.encounter.targets[0];
    let player = env.player;
    let takes_damage = player_takes_damage(env);
    let inert: [(super::sim::UnitId, &str, &str, &str); 4] = [
        (
            player,
            "player",
            "Chance of Death",
            "acts only when the player takes damage",
        ),
        (
            target,
            "target",
            "Parry Haste",
            "acts only on parried attacks",
        ),
        (
            player,
            "player",
            "Parry Haste",
            "acts only on attacks the player parries, and nothing attacks the player",
        ),
        (
            player,
            "player",
            "Freezing Band",
            "acts only on melee hits the player takes",
        ),
    ];
    for (unit, side, label, reason) in inert {
        if label == "Chance of Death" && takes_damage {
            continue;
        }
        // A tank's Freezing Band is a proc of the hits it takes (hit_taken_item_listeners).
        if label == "Freezing Band" && tanking {
            continue;
        }
        let has = env.sim.get_aura(unit, label).is_some();
        if label == "Parry Haste" && tanking {
            if has {
                effects.push(json!({"kind": "parry_haste", "unit": side, "aura": label}));
            }
            continue;
        }
        if label == "Parry Haste"
            && unit == target
            && env.sim.unit(target).auto_attacks.auto_swing_melee
        {
            if has {
                effects.push(json!({"kind": "parry_haste", "unit": side, "aura": label,
                    "swing_speed": env.sim.unit(target).auto_attacks.mh.swing_speed,
                    "melee_haste_multiplier": env.sim.total_melee_haste_multiplier(target)}));
            }
            continue;
        }
        if has {
            effects.push(
                json!({"kind": "inert_listener", "unit": side, "aura": label, "reason": reason}),
            );
        }
    }
    if takes_damage && env.sim.get_aura(player, "Chance of Death").is_some() {
        effects.push(json!({"kind": "chance_of_death", "aura": "Chance of Death"}));
    }
    effects.extend(party_damage_shields(env, tanking));
    effects.extend(super::export_items::melee_item_listeners(env));
    effects.extend(super::export_items::hit_taken_item_listeners(
        env,
        tanking,
        unrepresented,
    ));
    // A class's inert listener of hits the player takes acts once anything hits the player: the
    // target's swings when it tanks the player, or the Goblin Sapper Charge's self hit. Only the
    // listeners vetted for those hits stay inert: without a tank, the listeners of parried,
    // dodged or blocked attacks, which the sapper's magic hit never is, the procs on melee hits
    // taken, which its spell hit is not, and a form's rage bar no spell enters.
    if takes_damage {
        const SAPPER_ONLY: [&str; 7] = [
            "Parry Haste",
            "Riposte Trigger",
            "Defensive State - Trigger",
            "Revenge - Trigger",
            "RageBar",
            "Freezing Band",
            "Wildheart Raiment 5P",
        ];
        for effect in earlier.iter().chain(effects.iter()) {
            let label = effect["aura"].as_str().unwrap_or_default();
            if effect["kind"] != "inert_listener"
                || effect["unit"] != "player"
                || (!tanking
                    && (SAPPER_ONLY.contains(&label)
                        || effect["reason"] == "hears only melee hits the player takes"
                        || effect["reason"] == DAMAGE_SHIELD_REASON))
            {
                continue;
            }
            let hears_hits_taken = env
                .sim
                .get_aura(player, label)
                .is_some_and(|aura| env.sim.aura(aura).events.on_spell_hit_taken);
            if hears_hits_taken {
                unrepresented.push(if tanking {
                    format!("player aura {label:?} reacts to the target's swings")
                } else {
                    format!(
                        "player aura {label:?} reacts to the Goblin Sapper Charge's hit on the player"
                    )
                });
            }
        }
    }
    Ok(effects)
}

/// The refusals of `prepare` that concern a tank assignment: Go's notes on a tanked target
/// without a melee swing or with a secondary target, and on copies of the target that do not
/// swing alike. The exporter also notes any tank list but the player alone; Go reads the list
/// only through each target's tank indexes, which the environment resolved to the player, no
/// tank, or a refusal for any other unit, so preparation follows the simulation instead.
fn tank_notes(env: &Environment, tanking: bool, unrepresented: &mut Vec<String>) {
    let first = env.encounter.targets[0];
    let unit = env.sim.unit(first);
    if tanking && !unit.auto_attacks.auto_swing_melee {
        unrepresented.push("a tanked target without a melee swing is unsupported".into());
    }
    if unit.secondary_target.is_some() {
        unrepresented.push("a target with a secondary target is unsupported".into());
    }
    // tools/oracle-v2/targets.go swingShape: Rust runs every copy's swing from the first
    // target's values, so each copy must swing at the same unit with the same values.
    if env.encounter.targets.len() > 1 {
        let shape = |target| {
            let unit = env.sim.unit(target);
            let values = (unit.current_target == Some(env.player)
                && unit.auto_attacks.auto_swing_melee
                && unit.auto_attacks.mh.swing_speed > 0.0)
                .then(|| super::enemy::enemy_values(env, target).map(|v| super::enemy::encode(&v)));
            (
                unit.current_target == Some(env.player),
                unit.current_target.is_none(),
                unit.secondary_target.is_some(),
                values.map(|values| values.ok()),
            )
        };
        let expected = shape(first);
        for (i, target) in env.encounter.targets.iter().enumerate().skip(1) {
            if shape(*target) != expected {
                unrepresented.push(format!(
                    "target {} swings unlike the first target: only identical copies are supported",
                    i + 1
                ));
            }
        }
    }
}

/// Builds the prepared v2 document for a reset environment.
pub(crate) fn export(
    env: &mut Environment,
    digest: &str,
    scenario: &str,
) -> Result<Value, Refusal> {
    let (mut prepared, unrepresented) = export_state(env, digest, scenario)?;
    if !unrepresented.is_empty() {
        // The exporter would describe the request as unrepresented, which the gate refuses:
        // Go prepares it and the gate sends it to Go.
        return Err(Refusal::new("unrepresented", unrepresented.join("; ")));
    }
    // tools/oracle-v2/item_swap.go itemSwapEffect, after everything else.
    if let Some(effect) = super::item_swap::item_swap_effect(env, digest, scenario, &prepared)? {
        prepared["effects"]
            .as_array_mut()
            .expect("the effects are a list")
            .push(effect);
    }
    Ok(prepared)
}

/// Go `prepareSet`: the prepared state of the environment as it stands, and what the exporter
/// cannot describe of it, sorted.
pub(crate) fn export_state(
    env: &mut Environment,
    digest: &str,
    scenario: &str,
) -> Result<(Value, Vec<String>), Refusal> {
    let mut unrepresented: Vec<String> = Vec::new();
    let mut timers = TimerNames::default();
    let request = env.request.clone();
    let options = request
        .message("sim_options")
        .cloned()
        .unwrap_or_else(|| Message::empty("proto.SimOptions"));
    let player_message = env.sim.character(env.player).player.clone();
    let target = env.encounter.targets[0];
    let player = env.player;
    // Go `prepare`: stats against a mob type are the fight's, which prepared v2 does not carry.
    // Go reads only the bonus against the target's own type; under Forever's rules a bonus
    // against another type is inert.
    let target_type = env.sim.unit(target).mob_type.clone();
    let forever = env.sim.rules == super::sim::Rules::Forever;
    for (mob_type, bonus) in &env.attack_table(player, target).mob_type_bonus_stats {
        let inert = forever && *mob_type != target_type;
        if !bonus.is_zero() && !inert {
            unrepresented.push(format!(
                "mob type bonus stats for {mob_type} are unsupported"
            ));
        }
    }

    let spells: Vec<super::sim::SpellId> = env.sim.unit(player).spellbook.clone();
    let mut exported_spells: Vec<Value> = spells
        .iter()
        .map(|spell| export_spell_of(env, *spell, &mut timers, &mut unrepresented))
        .collect();
    for (i, spell) in spells.iter().enumerate() {
        if let Some(effect) = env.agent.damage_effect(&env.sim, *spell) {
            exported_spells[i]["damage_effect"] = effect;
        }
        if let Some(related) = env.sim.spell(*spell).related_dot_spell {
            if let Some(position) = spells.iter().position(|s| *s == related) {
                exported_spells[i]["related_dot_spell"] = json!(position);
            }
        }
    }

    let mut mcds = Vec::new();
    for mcd in &env.sim.character(player).initial_major_cooldowns {
        let action = env.sim.spell(mcd.spell).action_id.clone();
        let mut types = Vec::new();
        for (bit, name) in [
            (1u32, "mana"),
            (2, "dps"),
            (4, "explosive"),
            (8, "survival"),
        ] {
            if mcd.cooldown_type & bit != 0 {
                types.push(name);
            }
        }
        let mut out = json!({"action_id": action_id(Some(&action)), "priority": mcd.priority,
                             "type": types, "timings_ns": mcd.timings});
        if let Some(first) = spells
            .iter()
            .find(|s| env.sim.spell(**s).action_id == action)
        {
            if *first != mcd.spell {
                if let Some(position) = spells.iter().position(|s| *s == mcd.spell) {
                    out["spell"] = json!(position);
                }
            }
        }
        mcds.push(out);
    }

    let rotation = player_message
        .message("rotation")
        .map_or_else(|| json!({}), |rotation| rotation.to_protojson());
    let talents = talent_values(env.agent.talents());
    let mut effects: Vec<Value> = env.agent.export_effects(env, &mut unrepresented);
    unrepresented.extend(env.agent.unrepresented(&env.sim, player));
    effects.extend(super::common_effects::common_effects(
        env,
        &mut unrepresented,
    ));
    effects.extend(super::pet::inert_pet_effects(env, &mut unrepresented));
    // Go then appends the melee, gear, spell data and energy proc effects, and the stat auras
    // effect, in that order.
    effects.extend(super::export_items::item_proc_effects(
        env,
        &mut unrepresented,
    ));
    // A tank's swing is read under every stat aura combination too, from the same simulations.
    let tanking = env.tanking();
    let layout = super::stat_auras::stat_layout(env);
    let stat_auras = layout.labels.clone();
    let mut combos = None;
    if tanking && !stat_auras.is_empty() {
        combos = Some(super::enemy::EnemyCombos::new(env, &layout)?);
    }
    match combos.as_mut() {
        Some(combos) => effects.extend(super::stat_auras::stat_auras_effect_reading(
            env,
            &layout,
            Some(&mut |mask, fresh, exact| combos.read(mask, fresh, exact)),
        )?),
        None => effects.extend(super::stat_auras::stat_auras_effect_reading(
            env, &layout, None,
        )?),
    }
    let tail = tail_effects(env, &mut unrepresented, &effects)?;
    effects.extend(tail);

    let professions: Vec<String> = env
        .sim
        .character(player)
        .professions
        .iter()
        .filter(|p| p.as_str() != "ProfessionUnknown")
        .cloned()
        .collect();
    let table = env.attack_table(player, target).clone();
    let character = env.sim.character(player);
    let unit = env.sim.unit(player);
    // mana.go 174 and 201: Go computes the spirit regen once, fused, and fuses the not-casting sum.
    let spirit_regen = unit.stats[Stat::Spirit].mul_add(
        character.spirit_regen_per_spirit,
        character.spirit_regen_base,
    );
    let mp5 = unit.stats[Stat::MP5] / 5.0;
    let pseudo = &unit.pseudo_stats;
    let casting = {
        let mut regen = mp5;
        let mut spirit = 0.0;
        if pseudo.spirit_regen_rate_casting != 0.0 || pseudo.force_full_spirit_regen {
            spirit = spirit_regen * pseudo.spirit_regen_multiplier;
            if !pseudo.force_full_spirit_regen {
                spirit *= pseudo.spirit_regen_rate_casting;
            }
        }
        regen += spirit;
        regen * unit.mana_bar.mana_regen_multiplier
    };
    let not_casting = spirit_regen.mul_add(pseudo.spirit_regen_multiplier, mp5)
        * unit.mana_bar.mana_regen_multiplier;
    let mana = json!({
        "max": unit.stats[Stat::Mana],
        "base": unit.mana_bar.base_mana,
        "spirit_regen_per_second": spirit_regen,
        "regen_per_second_casting": casting,
        "regen_per_second_not_casting": not_casting,
        "teardown_max": 0.0,
    });

    let target_count = env.encounter.targets.len();
    let target_unit = env.sim.unit(target);
    let mut prepared = json!({
        "schema_version": SCHEMA_VERSION,
        "contract": CONTRACT,
        "reference": {
            "engine_revision": crate::SOURCE_REVISION,
            "client_build": crate::CLIENT_BUILD,
            "exporter": "tools/oracle-v2",
        },
        "request_sha256": digest,
        "scenario_id": scenario,
        "sim": {
            "iterations": options.i32("iterations"),
            "seed": options.int("random_seed"),
            "labeled_rng": options.bool("use_labeled_rands") || options.bool("is_test"),
            "debug_first_iteration": options.bool("debug_first_iteration"),
            "debug": options.bool("debug"),
        },
        "encounter": {
            "duration_ns": env.encounter.duration,
            "duration_variation_ns": env.encounter.duration_variation,
            "execute_proportion_20": env.encounter.execute_proportion_20,
            "execute_proportion_25": env.encounter.execute_proportion_25,
            "execute_proportion_35": env.encounter.execute_proportion_35,
            "execute_proportion_45": env.encounter.execute_proportion_45,
            "execute_proportion_90": env.encounter.execute_proportion_90,
        },
        "target": {
            "index": target_unit.unit_index,
            "label": target_unit.label,
            "level": target_unit.level,
            "mob_type": target_unit.mob_type,
            "stats": stat_values(&target_unit.stats),
            "pseudo_stats": export_pseudo(&target_unit.pseudo_stats),
            "auras": export_auras(&env.sim, target, &mut timers),
            "auto_swing_melee": target_unit.auto_attacks.auto_swing_melee,
            "auto_swing_ranged": target_unit.auto_attacks.auto_swing_ranged,
            "metrics_actions": metrics_actions(env, target),
        },
        "player": {
            "index": unit.unit_index,
            "label": unit.label,
            "level": unit.level,
            "stats": stat_values(&unit.stats),
            "pseudo_stats": export_pseudo(&unit.pseudo_stats),
            "auras": export_auras(&env.sim, player, &mut timers),
            "name": character.name,
            "class": character.class,
            "race": character.race,
            "professions": professions,
            "talents_string": player_message.str("talents_string"),
            "talents": talents,
            "class_options": class_options(&player_message),
            "reaction_ns": unit.reaction_time,
            "channel_clip_delay_ns": unit.channel_clip_delay,
            "distance_yards": unit.distance_from_target,
            "cast_speed": unit.cast_speed,
            "mana": mana,
            "attack_table": {
                "base_spell_miss_chance": table.base_spell_miss_chance,
                "spell_crit_suppression": table.spell_crit_suppression,
                "bonus_spell_crit_percent": table.bonus_spell_crit_percent,
                "crit_multiplier": table.crit_multiplier,
                "damage_dealt_multiplier": table.damage_dealt_multiplier,
                "damage_taken_multiplier": table.damage_taken_multiplier,
            },
            "spells": exported_spells,
            "major_cooldowns": mcds,
            "rotation": rotation,
            "prepull_actions": env.prepull_actions,
        },
        "effects": effects,
        "unrepresented": [],
    });
    if target_count > 1 {
        prepared["encounter"]["target_count"] = json!(target_count);
    }
    if options.bool("save_all_values") {
        prepared["sim"]["save_all_values"] = json!(true);
    }
    let hp = player_message
        .message("cooldowns")
        .map_or(0.0, |cooldowns| cooldowns.f64("hp_percent_for_defensives"));
    if hp != 0.0 {
        prepared["player"]["hp_percent_for_defensives"] = json!(hp);
    }
    if !env.rotation_dot_base_durations.is_empty() {
        prepared["player"]["rotation_dot_base_durations"] = json!(env.rotation_dot_base_durations);
    }
    // Forever's beta logs show no partial resists from the target's level (README, Rust-only
    // mechanics); the reference keeps them.
    if env.rules == Rules::Forever {
        prepared["target"]["no_level_partial_resist"] = json!(true);
        // The client's mana regeneration interrupt aura acts at once (README, Rust-only
        // mechanics); the reference's cached tick waits for another refresh.
        prepared["player"]["casting_regen_at_once"] = json!(true);
        // A player does not walk off before the spell they queued fires (README, Rust-only
        // mechanics); the reference's move starts at once.
        prepared["player"]["move_waits_for_queued_cast"] = json!(true);
        // A melee swing does not restart the Auto Shot timer (README, Rust-only mechanics); the
        // reference restarts it on every swing.
        prepared["player"]["melee_keeps_auto_shot_timer"] = json!(true);
        // A move whose end time has come has arrived (README, Rust-only mechanics); the
        // reference can read the walk a hair short of the destination at that instant.
        prepared["player"]["move_arrives_at_destination"] = json!(true);
        // A rotation with a move keeps looking through the global cooldown (README, Rust-only
        // mechanics); the reference's idle rotation sleeps until the global cooldown ends.
        prepared["player"]["rotation_with_moves_looks_through_gcd"] = json!(true);
        // The off hand's half swing wait counts from the melee attack's start, which a walk in
        // after an Auto Shot begins (README, Rust-only mechanics); the reference counts it from
        // every arrival in melee range.
        prepared["player"]["off_hand_waits_from_attack_start"] = json!(true);
        // Autocasting looks past a cooldown the GCD holds back (README, Rust-only mechanics);
        // the reference stops at the first ready one.
        prepared["player"]["autocast_skips_unready_cooldowns"] = json!(true);
        // A damage bonus applied after the calculation is damage the target takes, so threat
        // (README, Rust-only mechanics); the reference kept the threat of the damage before it.
        prepared["player"]["threat_follows_damage_bonus"] = json!(true);
        // A healer sim counts only the share of each heal real healers do not overheal
        // (README, Rust-only mechanics); the reference counts every heal in full.
        let overheal = super::overheal::export(&player_message, &prepared["player"]["spells"]);
        if !overheal.is_empty() {
            prepared["player"]["overheal"] = Value::Array(overheal);
        }
        // A healer sim reports its burst healing beside its healing per second (README,
        // Rust-only mechanics); the reference reports none.
        if super::overheal::is_healer(&player_message) {
            prepared["player"]["burst_window"] = super::burst_hps::export();
        }
    }
    if let Some(reads) = &env.rotation_reads {
        prepared["player"]["rotation_reads"] = json!(reads);
    }
    prepared["melee"] = export_melee(env, &mut unrepresented);
    let pets = super::pet::export_pets(env, &mut timers, &mut unrepresented);
    if !pets.is_empty() {
        prepared["pets"] = Value::Array(pets);
    }
    // main.go `healthAtReset`: the health a reset leaves where it differs from the maximum,
    // which a form the agent enters after the health reset raises.
    {
        // Read from a reset simulation of its own, which a swapped one is not.
        let fresh = env.fresh();
        let unit = fresh.sim.unit(fresh.player);
        if unit.health_bar && unit.current_health != unit.stats[Stat::Health] {
            prepared["player"]["health_at_reset"] = json!(unit.current_health);
        }
    }
    // Go `exportEnergy`: the energy bar, when the class has one.
    let energy = &env.sim.unit(player).energy_bar;
    if energy.enabled {
        if energy.has_no_regen {
            unrepresented.push("an energy bar without regeneration is unsupported".to_string());
        }
        prepared["player"]["energy"] = json!({
            "max_energy": energy.max_energy,
            "max_combo_points": energy.max_combo_points,
            "tick_duration_ns": energy.tick_duration,
            "energy_per_tick": energy.energy_per_tick,
        });
    }
    tank_notes(env, tanking, &mut unrepresented);
    // core/battle_squawk.go (the fork's patch 127): the party's engineers' chickens raise the
    // melee speed at the reset, which the exported melee state holds, for the aura's four
    // minutes. Its expiry and its pass to a pet that follows the owner's melee speed are not run.
    if let Some(squawk) = env.sim.get_aura(player, "Battle Squawk") {
        if env.sim.aura(squawk).expires < env.encounter.duration + env.encounter.duration_variation
        {
            // Forever's rules run its expiry: the stacks fall to none and take their melee speed
            // back, 5% each, as Go's OnStacksChange does.
            if env.sim.rules == super::sim::Rules::Forever {
                if let Some(list) = prepared["effects"].as_array_mut() {
                    list.push(json!({
                        "kind": "battle_squawk", "aura": env.sim.aura(squawk).label,
                        "per_stack": 1.05,
                    }));
                }
            } else {
                unrepresented
                    .push("a fight that outlasts Battle Squawk is unsupported".to_string());
            }
        }
        let pet_follows = env.sim.unit(player).pets.iter().any(|pet| {
            env.sim
                .unit(*pet)
                .pet
                .as_ref()
                .is_some_and(|data| data.dynamic_melee_speed_enabled)
        });
        if pet_follows {
            unrepresented.push(
                "Battle Squawk's melee speed passes to a pet, which is unsupported".to_string(),
            );
        }
    }
    if tanking {
        // The auras whose damage taken multiplier an effect multiplies, which the runtime
        // tracks live.
        let mut tracked = Vec::new();
        for effect in &effects {
            let Some(auras) = effect["auras"].as_array() else {
                continue;
            };
            for entry in auras.iter().filter(|entry| entry.is_object()) {
                let tracks = effect["kind"] == "player_damage_taken"
                    || (effect["kind"] == "pseudo_stat_auras" && entry["stat"] == "damage_taken");
                if let (true, Some(label)) = (tracks, entry["aura"].as_str()) {
                    tracked.push(label.to_string());
                }
            }
        }
        let enemy =
            super::enemy::export_enemy(env, &stat_auras, combos, &tracked, &mut unrepresented)?;
        prepared["enemy"] = serde_json::to_value(enemy).expect("the swing serializes");
    }
    let teardown = teardown_max_mana(env, player, &mut unrepresented);
    prepared["player"]["mana"]["teardown_max"] = json!(teardown);
    if !env.sim.unit(player).mana_bar.enabled {
        prepared["player"]["mana"] = json!({"max": 0.0, "base": 0.0, "spirit_regen_per_second": 0.0,
            "regen_per_second_casting": 0.0, "regen_per_second_not_casting": 0.0, "teardown_max": 0.0});
    }
    unrepresented.sort();
    prepared["unrepresented"] = json!(unrepresented);
    Ok((prepared, unrepresented))
}

/// The reason an inert party damage shield names.
const DAMAGE_SHIELD_REASON: &str = "a damage shield: hears only physical hits the player takes";

/// A tank's party damage shields: Go `core.NewDamageShield`'s trigger, on each landed hit the
/// player takes from a physical attack, casts the shield's spell (its action with the tag plus
/// two) on the attacker a spell batch window later (the trigger is not immediate), a fixed magic
/// hit of the shield's damage that cannot crit. The damage is the shield's bid in its category;
/// the shield spell adds the wearer's spell power by its coefficient. Under Forever's rules a
/// paladin's own Retribution Aura rank or a druid's own Thorns the rotation casts is one more
/// shield.
fn tank_damage_shields(env: &Environment) -> Vec<Value> {
    let mut effects = Vec::new();
    let mut auras: Vec<_> = ["Retribution Aura (External)", "Thorns (External)"]
        .into_iter()
        .filter_map(|label| env.sim.get_aura(env.player, label))
        .collect();
    if env.sim.rules == crate::prepare::sim::Rules::Forever {
        let casts = crate::prepare::rotation::rotation_spells(&env.sim, env.player);
        auras.extend(
            env.sim
                .unit(env.player)
                .auras
                .iter()
                .copied()
                .filter(|&aura| {
                    let state = env.sim.aura(aura);
                    (state.label.starts_with("Retribution Aura Rank ")
                        || state.label == "Thorns (Player)")
                        && state.action_id.as_ref().is_some_and(|id| {
                            id.tag == 0 && casts.contains(&i64::from(id.spell_id))
                        })
                }),
        );
    }
    for aura in auras {
        let state = env.sim.aura(aura);
        let label = state.label.as_str();
        let Some(id) = state.action_id.clone() else {
            continue;
        };
        let spell = ActionId {
            tag: id.tag + 2,
            ..id
        };
        let Some(position) = super::export_items::spell_position(env, &spell) else {
            continue;
        };
        let Some(damage) = state
            .exclusive_effects
            .first()
            .map(|effect| env.sim.effects[effect.0].priority)
        else {
            continue;
        };
        let mut effect = json!({
            "kind": "spell_data_damage_proc", "trigger_aura": label,
            "trigger_spells": Vec::<usize>::new(), "struck": true, "landed_only": true,
            "require_damage": false, "proc_chance": 1.0, "spell": position, "average": 0.0,
            "variance": 0.0, "roll": [damage, damage], "can_crit": false,
            "delay_ns": super::common_effects::SPELL_BATCH_WINDOW,
        });
        // core.NewDamageShield deals the shield's damage as it is, with no roll. Go's exporter
        // writes it as a roll of equal bounds, which draws; Forever's rules describe the fixed
        // amount, which draws nothing.
        if env.sim.rules == crate::prepare::sim::Rules::Forever {
            effect["average"] = json!(damage);
            effect["roll"] = Value::Null;
        }
        effects.push(effect);
    }
    effects
}

/// The party's damage shields on the player: Go buffs/meta.go `newDamageShield` over
/// `core.NewDamageShield`, a listener on the landed hits the player takes that answers a
/// physical one. Without a tank nothing physical hits the player (the Goblin Sapper Charge's
/// self hit is fire), so a shield that hears nothing else never acts and is inert. A tank's
/// shield stays unclaimed: the gate refuses it as a reaction to the target's swings.
fn party_damage_shields(env: &Environment, tanking: bool) -> Vec<Value> {
    const LIFECYCLE: [&str; 6] = [
        "on_init",
        "on_reset",
        "on_done_iteration",
        "on_gain",
        "on_expire",
        "on_stacks_change",
    ];
    if tanking {
        return tank_damage_shields(env);
    }
    ["Retribution Aura (External)", "Thorns (External)"]
        .into_iter()
        .filter(|label| {
            env.sim.get_aura(env.player, label).is_some_and(|aura| {
                env.sim.aura(aura).callback_names().iter().all(|callback| {
                    LIFECYCLE.contains(&callback.as_str()) || callback == "on_spell_hit_taken"
                })
            })
        })
        .map(|label| {
            json!({"kind": "inert_listener", "unit": "player", "aura": label,
                "reason": DAMAGE_SHIELD_REASON})
        })
        .collect()
}
