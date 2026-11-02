//! The Warrior's part of tools/oracle-v2/warrior.go: the effects Go keeps in closures, each
//! formula mirroring the cited Go file at the pinned revision.

use serde_json::{json, Value};

use crate::prepare::buffs::generated;
use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::common_effects::{exclusive_category_effect, SPELL_BATCH_WINDOW};
use crate::prepare::dbcenums;
use crate::prepare::env::Environment;
use crate::prepare::rotation::rotation_spells;
use crate::prepare::sim::{AuraId, Rules, Sim, UnitId, NEVER_EXPIRES, SECOND};
use crate::prepare::spell::{DefenseType, ProcMask};
use crate::prepare::spelldata::Spell as Row;
use crate::prepare::stats::Stat;

use super::helpers::{action_id_string, last_stand_share, spell_action};
use super::shouts::SHOUT_EXPIRATION_THRESHOLD;
use super::spell_data::spell_data;
use super::spells::SUNDER_THREAT_PER_ATTACK_POWER;
use super::stances::STANCE_EFFECT_CATEGORY;
use super::Warrior;

/// The stance names Rust reads, in sim/warrior/stances.go's order.
fn stance_name(stance: &str) -> &'static str {
    match stance {
        "WarriorStanceBattle" => "battle",
        "WarriorStanceDefensive" => "defensive",
        "WarriorStanceBerserker" => "berserker",
        _ => "none",
    }
}

/// Whether the rotation names Shield Block, which the exporter's `warriorEffects` records as
/// `warriorRotationShieldBlock`.
pub(super) fn rotation_names_shield_block(sim: &Sim, unit: UnitId) -> bool {
    rotation_spells(sim, unit).contains(&i64::from(spell_data().shield_block.highest().id))
}

/// The exporter's `damageRoll`: a client damage row's roll.
fn damage_roll(row: &Row) -> Value {
    let effect = row.damage_effect();
    let average = effect.average(CHARACTER_LEVEL);
    json!({"average": average, "min": average * (1.0 - effect.variance / 2.0),
        "max": average * (1.0 + effect.variance / 2.0), "rolls": effect.variance != 0.0,
        "variance": effect.variance})
}

/// The exporter's `blockedForGood`: whether an aura can never activate, as each of its exclusive
/// categories is a single aura category held by another aura that never expires.
fn blocked_for_good(sim: &Sim, aura: AuraId) -> bool {
    let effects = &sim.aura(aura).exclusive_effects;
    for effect in effects {
        let category = sim.effects[effect.0].category;
        let active = sim.categories[category.0].active_effect;
        let blocked = match active {
            None => false,
            Some(active) => {
                active != *effect
                    && sim.aura(sim.effects[active.0].aura).duration == NEVER_EXPIRES
                    && sim.categories[category.0].single_aura
            }
        };
        if !blocked {
            return false;
        }
    }
    !effects.is_empty()
}

/// The exporter's `targetArmorWithStacks`: the target's armor with the aura active at the given
/// stacks, from a separate reset simulation.
fn target_armor_with_stacks(env: &Environment, label: &str, stacks: i32) -> f64 {
    let mut fresh = env.fresh();
    let target = fresh.encounter.targets[0];
    if stacks > 0 {
        let aura = fresh
            .sim
            .get_aura(target, label)
            .expect("the target has the aura in every reset");
        fresh.sim.activate(aura);
        // A stronger aura in the exclusive armor category keeps it out.
        if !fresh.sim.aura(aura).active {
            return f64::NAN;
        }
        fresh.sim.set_stacks(aura, stacks);
    }
    let unit = fresh.sim.unit(target);
    unit.pseudo_stats.armor_multiplier * unit.stats[Stat::Armor]
}

/// The exporter's `stackBid`: what one stack of a target aura bids in its exclusive category,
/// read from a separate reset simulation.
fn stack_bid(env: &Environment, label: &str, category: &str) -> f64 {
    let mut fresh = env.fresh();
    let target = fresh.encounter.targets[0];
    let aura = fresh
        .sim
        .get_aura(target, label)
        .expect("the target has the aura in every reset");
    fresh.sim.activate(aura);
    if !fresh.sim.aura(aura).active {
        return 0.0;
    }
    fresh.sim.set_stacks(aura, 1);
    for effect in &fresh.sim.aura(aura).exclusive_effects {
        let effect = &fresh.sim.effects[effect.0];
        if fresh.sim.categories[effect.category.0].name == category {
            return effect.priority;
        }
    }
    0.0
}

/// The exporter's `healModifiers`: spell_result.go calcHealingInternal's multipliers for a heal
/// of the character on itself, at reset.
fn heal_modifiers(env: &Environment, notes: &mut Vec<String>) -> Value {
    let sim = &env.sim;
    let unit = env.player;
    let pseudo = &sim.unit(unit).pseudo_stats;
    let _ = notes;
    json!({"healing_dealt_multiplier": pseudo.healing_dealt_multiplier,
        "periodic_healing_dealt_multiplier": pseudo.periodic_healing_dealt_multiplier,
        "healing_taken_multiplier": pseudo.healing_taken_multiplier,
        "table_healing_dealt_multiplier": env.attack_table(unit, unit).healing_dealt_multiplier,
        "healing_power": sim.stat(unit, Stat::HealingPower) + pseudo.bonus_healing_taken})
}

impl Warrior {
    /// The spell a ladder's top rank registered, if the warrior has it.
    fn spell_of(sim: &Sim, unit: UnitId, row: &Row) -> Option<crate::prepare::sim::SpellId> {
        sim.get_spell(unit, &spell_action(row.id))
    }

    /// Go `warriorEffects`.
    pub(super) fn all_effects(&self, env: &Environment, notes: &mut Vec<String>) -> Vec<Value> {
        let sim = &env.sim;
        let unit = env.player;
        let data = spell_data();
        let level = CHARACTER_LEVEL;
        let talent = |name: &str| self.talent(name);
        let mut effects: Vec<Value> = vec![crate::prepare::rage::rage_bar_effect(sim, unit, 1.0)];

        // stances.go: the stance the warrior starts in and each stance's cast and aura.
        let max_retained_rage = data.tactical_mastery.value_at(1)
            + data
                .improved_tactical_mastery
                .value_at(talent("improved_tactical_mastery"));
        let mut stances = Vec::new();
        for (row, stance) in [
            (data.battle_stance.highest(), "battle"),
            (data.defensive_stance.highest(), "defensive"),
            (data.berserker_stance.highest(), "berserker"),
        ] {
            let spell = sim.spell(Self::spell_of(sim, unit, row).expect("stances register"));
            let aura = sim.aura(spell.related_self_buff.expect("a stance has its aura"));
            stances.push(
                json!({"spell_id": spell.action_id.spell_id, "stance": stance,
                "aura": aura.label}),
            );
        }
        effects.push(json!({"kind": "warrior_stances",
            "default_stance": stance_name(&self.inputs.default_stance), "stances": stances,
            "max_retained_rage": max_retained_rage}));

        if self.has_talent("bloodthirst") {
            let row = data.bloodthirst.highest();
            effects.push(json!({"kind": "bloodthirst", "spell_id": row.id,
                "attack_power_share": row.effects[1].percent(),
                "base_damage": row.damage_effect().average(level)}));
        }
        // whirlwind.go: a warrior with an off hand weapon strikes with it too.
        let whirlwind = data.whirlwind.highest();
        effects.push(json!({"kind": "whirlwind", "spell_id": whirlwind.id,
            "off_hand": sim.oh_weapon(unit).is_some(),
            "max_targets": i32::from(whirlwind.max_targets)}));
        // execute.go: the dummy effect's base and ten times its chain amplitude per extra rage.
        let execute = data.execute.highest();
        effects.push(json!({"kind": "execute", "spell_id": execute.id,
            "base_damage": execute.effect_n(1).average(level),
            "damage_per_rage": f64::from(execute.effect_n(1).chain_amp) * 10.0}));
        // hamstring.go
        let hamstring = data.hamstring.highest();
        effects.push(json!({"kind": "hamstring", "spell_id": hamstring.id,
            "base_damage": hamstring.damage_effect().average(level)}));
        // pummel.go: the same strike in Berserker Stance. Go's exporter has no effect for it, so
        // only Forever's rules describe it.
        if sim.rules == Rules::Forever {
            let pummel = data.pummel.by_id(6554);
            effects.push(json!({"kind": "pummel", "spell_id": pummel.id,
                "base_damage": pummel.damage_effect().average(level)}));
        }
        // shield_bash.go, mocking_blow.go, disarm.go, victory_rush.go and intercept.go, which
        // Go's exporter has no effects for either.
        if sim.rules == Rules::Forever {
            let shield_bash = data.shield_bash.highest();
            effects.push(json!({"kind": "shield_bash", "spell_id": shield_bash.id,
                "base_damage": shield_bash.damage_effect().average(level),
                "can_block": sim.unit(unit).pseudo_stats.can_block}));
            let mocking_blow = data.mocking_blow.highest();
            effects.push(json!({"kind": "mocking_blow", "spell_id": mocking_blow.id,
                "base_damage": mocking_blow.damage_effect().average(level)}));
            let disarm = data.disarm.highest();
            effects.push(json!({"kind": "warrior_disarm", "spell_id": disarm.id,
                "aura": format!("Disarm-{}", sim.unit(unit).label)}));
            let victory_rush = data.victory_rush.highest();
            effects.push(json!({"kind": "victory_rush", "spell_id": victory_rush.id,
                "aura": "Victorious"}));
            // The damage sits on the stun the charge triggers, not on Intercept itself.
            let intercept = data.intercept.highest();
            if let Some(spell) = Self::spell_of(sim, unit, intercept) {
                effects.push(
                    json!({"kind": "warrior_intercept", "spell_id": intercept.id,
                    "aura": "Intercept", "speed_multiplier": 3.0, "overshoot": 3.5,
                    "min_range": sim.spell(spell).min_range,
                    "damage": intercept.effect_n(2).trigger().damage_effect().average(level)}),
                );
            }
        }
        // bloodrage.go: instant and periodic rage scaled by Improved Bloodrage, and a share of
        // base health.
        let bloodrage = data.bloodrage.highest();
        let over_time = data.bloodrage_triggered.highest();
        let improved_bloodrage = data
            .improved_bloodrage
            .multiplier_at(talent("improved_bloodrage"));
        effects.push(json!({"kind": "bloodrage", "spell_id": bloodrage.id,
            "instant_rage": data.bloodrage.effect_at(1).tenths_at(1) * improved_bloodrage,
            "rage_per_tick": over_time.periodic_effect().tenths() * improved_bloodrage,
            "ticks": (over_time.duration() / over_time.periodic_effect().period()) as i32,
            "period_ns": over_time.periodic_effect().period(),
            "health_cost": sim.character(unit).base_stats[Stat::Health]
                * f64::from(bloodrage.power(dbcenums::POWER_HEALTH).cost_pct) / 100.0,
            "rage_threshold": 70.0}));
        // berserker_rage.go: Improved Berserker Rage's rage.
        let berserker_rage = data.berserker_rage.highest();
        effects.push(
            json!({"kind": "berserker_rage", "spell_id": berserker_rage.id,
            "aura": "Berserker Rage",
            "rage_gain": data.improved_berserker_rage.effect_at(1)
                .tenths_at(talent("improved_berserker_rage"))}),
        );
        // charge.go: the cast spends no rage, gives rage, triples the warrior's movement speed
        // through its dash aura and runs to 3.5 yards inside the spell's minimum range.
        let charge_row = data.charge.by_id(11578);
        if let Some(charge) = Self::spell_of(sim, unit, charge_row) {
            let charge = sim.spell(charge);
            effects.push(
                json!({"kind": "warrior_charge", "spell_id": charge.action_id.spell_id,
                "aura": "Charge",
                "rage": charge_row.energize_effect().tenths()
                    + data.improved_charge.tenths_at(talent("improved_charge")),
                "vanguard": self.has_talent("vanguard"), "speed_multiplier": 3.0,
                "overshoot": 3.5, "min_range": charge.min_range,
                "no_threat": charge_row.no_threat()}),
            );
        }
        if self.has_talent("death_wish") {
            let row = data.death_wish.highest();
            effects.push(
                json!({"kind": "death_wish", "spell_id": row.id, "aura": "Death Wish",
                "physical_multiplier":
                    1.0 + row.effect(dbcenums::A_MOD_DAMAGE_PERCENT_DONE, 1).percent(),
                "wait_ns": crate::prepare::spell::GCD_DEFAULT}),
            );
        }
        // items.go Battlegear of Might 5 piece.
        if let Some(aura) = sim.get_aura(unit, "Battlegear of Might 5P") {
            effects.push(json!({"kind": "battlegear_of_might_rage",
                "trigger_aura": sim.aura(aura).label,
                "rng_label": "Battlegear of Might - 5PC", "proc_chance": 0.2, "rage": 1.0,
                "metrics_action_id": json!({"spell_id": 29478})}));
        }
        // talents_arms.go registerSweepingStrikes.
        if let Some(aura) = sim.get_aura(unit, "Sweeping Strikes") {
            let row = data.sweeping_strikes.highest();
            effects.push(json!({"kind": "sweeping_strikes", "spell_id": 12723,
                "aura": sim.aura(aura).label, "charges": i32::from(row.proc_charges)}));
        }
        // Auras that multiply the warrior's damage taken while up.
        let mut damage_taken = Vec::new();
        for (label, row) in [
            ("Death Wish", data.death_wish.highest()),
            ("Recklessness", data.recklessness.highest()),
        ] {
            if let Some(aura) = sim.get_aura(unit, label) {
                if !sim.aura(aura).active {
                    damage_taken.push(json!({"aura": sim.aura(aura).label,
                        "multiplier": 1.0
                            + row.effect(dbcenums::A_MOD_DAMAGE_PERCENT_TAKEN, 127).percent()}));
                }
            }
        }
        effects.push(json!({"kind": "player_damage_taken", "auras": damage_taken}));
        // recklessness.go
        effects.push(
            json!({"kind": "recklessness", "spell_id": data.recklessness.highest().id,
            "aura": "Recklessness"}),
        );
        // sunder_armor.go: the warrior's own stacks, refused while another aura holds the
        // armor category.
        let target = env.encounter.targets[0];
        let sunder_row = data.sunder_armor.highest();
        let sunder_label = sim
            .get_spell(unit, &spell_action(sunder_row.id))
            .and_then(|spell| {
                sim.spell(spell)
                    .related_aura_arrays
                    .values()
                    .next()
                    .and_then(|auras| {
                        auras
                            .get(sim.unit(target).unit_index as usize)
                            .copied()
                            .flatten()
                    })
            });
        if let Some(sunder) = sunder_label {
            let blocked = blocked_for_good(sim, sunder);
            // Patch 185: each cast sets the flat threat to the client's value plus a share of the
            // spell's melee attack power.
            effects.push(json!({"kind": "sunder_armor", "spell_id": sunder_row.id,
                "aura": sim.aura(sunder).label, "blocked": blocked,
                "flat_threat": sunder_row.find_effect(dbcenums::E_THREAT, 0, 0).average(level),
                "threat_per_attack_power": SUNDER_THREAT_PER_ATTACK_POWER}));
            // The armor category the stacks bid in, with the target's armor at each stack
            // count, read from separate reset simulations.
            if !blocked && sim.aura(sunder).exclusive_effects.len() == 1 {
                let category = sim.effects[sim.aura(sunder).exclusive_effects[0].0].category;
                let name = sim.categories[category.0].name.clone();
                if let Some(mut category) =
                    exclusive_category_effect(sim, target, "target", &name, notes)
                {
                    let label = sim.aura(sunder).label.clone();
                    let armor: Vec<f64> = (0..=sim.aura(sunder).max_stacks)
                        .map(|stacks| target_armor_with_stacks(env, &label, stacks))
                        .collect();
                    let members = category["members"].as_array_mut().expect("members");
                    for member in members {
                        let member_label = member["aura"].as_str().unwrap_or("").to_string();
                        if let Some(aura) = sim.get_aura(target, &member_label) {
                            if sim.aura(aura).max_stacks > 0 {
                                member["per_stack"] = json!(stack_bid(env, &member_label, &name));
                            }
                        }
                    }
                    category["armor_by_stacks"] = json!(armor);
                    effects.push(category);
                }
            }
        }
        if talent("deep_wounds") > 0 {
            let bleed = data.deep_wounds_triggered.by_id(412609);
            effects.push(json!({"kind": "deep_wounds", "spell_id": bleed.id,
                "trigger_aura": "Deep Wounds - Trigger",
                "share": data.deep_wounds.fraction_at(talent("deep_wounds")),
                "tick_can_crit": bleed.periodic_can_crit(),
                "tick_magic": bleed.defense_type_core() == DefenseType::Magic}));
        }
        // talents_fury.go registerUnbridledWrath: white hits, not melee specials
        if talent("unbridled_wrath") > 0 {
            let trigger = data.unbridled_wrath_triggered.highest();
            effects.push(
                json!({"kind": "unbridled_wrath", "trigger_aura": "Unbridled Wrath",
                "spell_id": trigger.id,
                "proc_chance": data.unbridled_wrath.fraction_at(talent("unbridled_wrath")),
                "rage": trigger.energize_effect().tenths(),
                "delay_ns": SPELL_BATCH_WINDOW}),
            );
        }
        if talent("flurry") > 0 {
            let buff = data.flurry_triggered.highest();
            let mut flurry = json!({"kind": "warrior_flurry", "trigger_aura": "Flurry - Trigger",
                "aura": "Flurry",
                "melee_speed_multiplier": data.flurry.multiplier_at(talent("flurry")),
                "charges": i32::from(buff.proc_charges)});
            // Forever's logs: misses, dodges and parries spend a charge too (flurry.rs).
            if sim.rules == Rules::Forever {
                flurry["every_swing_spends"] = json!(true);
            }
            effects.push(flurry);
        }
        if self.has_talent("anger_management") {
            let row = data.anger_management.highest();
            effects.push(json!({"kind": "anger_management", "spell_id": row.id,
                "rage": row.effects[1].base_points,
                "period_ns": (row.effects[2].base_points as i64) * SECOND}));
        }
        // stances.go and battle_shout.go: the stances and the shouts are single aura
        // categories.
        for name in [STANCE_EFFECT_CATEGORY, "BattleShout"] {
            if let Some(effect) = exclusive_category_effect(sim, unit, "player", name, notes) {
                effects.push(effect);
            }
        }
        // stances.go: each stance's passive multiplies threat, and Defensive and Berserker
        // Stance damage taken, Defensive damage dealt too, from client data.
        let mut defiance = Vec::new();
        if sim.get_aura(unit, "Shield Wall").is_some() {
            defiance.push(json!({"aura": "Shield Wall", "stat": "damage_taken",
                "multiplier": 1.0 + data.shield_wall.highest()
                    .effect(dbcenums::A_MOD_DAMAGE_PERCENT_TAKEN, 127).percent()}));
        }
        if talent("defiance") > 0 && sim.unit(unit).pseudo_stats.can_block {
            defiance.push(json!({"aura": "Defensive Stance", "stat": "threat",
                "multiplier": data.defiance.effect(dbcenums::A_MOD_THREAT, 127)
                    .multiplier_at(talent("defiance"))}));
        }
        let mut pseudo_stat_auras = vec![
            json!({"aura": "Battle Stance", "stat": "threat",
                "multiplier": data.battle_stance_passive.effect(dbcenums::A_MOD_THREAT, 127)
                    .multiplier_at(1)}),
            json!({"aura": "Defensive Stance", "stat": "threat",
                "multiplier": data.defensive_stance_passive.effect(dbcenums::A_MOD_THREAT, 127)
                    .multiplier_at(1)}),
            json!({"aura": "Defensive Stance", "stat": "damage_taken",
                "multiplier": data.defensive_stance_passive
                    .effect(dbcenums::A_MOD_DAMAGE_PERCENT_TAKEN, 127).multiplier_at(1)}),
            json!({"aura": "Defensive Stance", "stat": "damage_dealt",
                "multiplier": data.defensive_stance_passive
                    .effect(dbcenums::A_MOD_DAMAGE_PERCENT_DONE, 127).multiplier_at(1)}),
        ];
        pseudo_stat_auras.extend(defiance);
        pseudo_stat_auras.push(json!({"aura": "Berserker Stance", "stat": "threat",
            "multiplier": data.berserker_stance_passive.effect(dbcenums::A_MOD_THREAT, 127)
                .multiplier_at(1)}));
        pseudo_stat_auras.push(json!({"aura": "Berserker Stance", "stat": "damage_taken",
            "multiplier": data.berserker_stance_passive
                .effect(dbcenums::A_MOD_DAMAGE_PERCENT_TAKEN, 127).multiplier_at(1)}));
        effects.push(json!({"kind": "pseudo_stat_auras", "auras": pseudo_stat_auras}));
        // battle_shout.go: the warrior's own shout, which outbids the party's at an equal value.
        let battle_shout = data.battle_shout.highest_for(env.rules);
        if let Some(own) = sim.get_spell(unit, &spell_action(battle_shout.id)) {
            let mut value = generated::BATTLE_SHOUT.value(0);
            if self.inputs.use_battle_shout && sim.state_flag(self.has_bs_t2) {
                value += super::shouts::BATTLE_SHOUT_T2_BONUS;
            }
            for aura in sim.auras_with_tag(unit, "BattleShout") {
                let aura = sim.aura(aura);
                if aura.action_id.as_ref().is_some_and(|id| id.tag == 0) {
                    effects.push(json!({"kind": "battle_shout",
                        "spell_id": sim.spell(own).action_id.spell_id, "aura": aura.label,
                        "value": value,
                        "refresh_threshold_ns": SHOUT_EXPIRATION_THRESHOLD}));
                }
            }
        }
        // rend.go: the client tick base and a share of attack power a tick, a Go literal.
        let rend = data.rend.highest();
        effects.push(json!({"kind": "rend", "spell_id": rend.id,
            "tick_base": rend.periodic_effect().average(level), "attack_power_per_tick": 0.02,
            "tick_can_crit": rend.periodic_can_crit(),
            "tick_magic": rend.defense_type_core() == DefenseType::Magic}));
        // overpower.go: the client base on normalized main hand damage.
        let overpower = data.overpower.by_id(11585);
        effects.push(json!({"kind": "overpower", "spell_id": overpower.id,
            "base_damage": overpower.damage_effect().average(level)}));
        if self.has_talent("mortal_strike") {
            let row = data.mortal_strike.highest();
            effects.push(json!({"kind": "mortal_strike", "spell_id": row.id,
                "base_damage": row.damage_effect().average(level)}));
        }
        if self.has_talent("spearing_strike") {
            let row = data.spearing_strike.highest();
            let mob_type = sim.unit(target).mob_type.as_str();
            let mob_multiplier = if mob_type == "MobTypeGiant" || mob_type == "MobTypeDragonkin" {
                1.0 + row.effects[2].base_points
            } else {
                1.0
            };
            effects.push(json!({"kind": "spearing_strike", "spell_id": row.id,
                "weapon_share": row.effects[1].percent(), "mob_multiplier": mob_multiplier}));
        }
        // slam.go: the client base on main hand weapon damage.
        let slam = data.slam.highest();
        effects.push(json!({"kind": "slam", "spell_id": slam.id,
            "base_damage": slam.damage_effect().average(level),
            "stops_swings": talent("improved_slam") == 0}));
        // demoralizing_shout.go: a magic hit roll on each target in unit index order, each
        // landed one activating that target's debuff.
        if let Some(shout) = Self::spell_of(sim, unit, data.demoralizing_shout.highest()) {
            let position = sim
                .unit(unit)
                .spellbook
                .iter()
                .position(|candidate| *candidate == shout);
            let aura = sim
                .spell(shout)
                .related_aura_arrays
                .values()
                .last()
                .and_then(|auras| {
                    auras
                        .get(sim.unit(env.encounter.targets[0]).unit_index as usize)
                        .copied()
                        .flatten()
                });
            if let (Some(position), Some(aura)) = (position, aura) {
                effects.push(json!({"kind": "demoralizing_shout", "spell": position,
                    "aura": sim.aura(aura).label}));
                // buffs.DemoralizingShoutAura and the raid's Demoralizing Roar and Shout
                // debuffs share one single aura category, each bidding its attack power cut.
                if let Some(category) = exclusive_category_effect(
                    sim,
                    env.encounter.targets[0],
                    "target",
                    generated::DEMORALIZING_SHOUT.category,
                    notes,
                ) {
                    effects.push(category);
                }
            }
        }
        // challenging_shout.go: an always hit outcome with no damage on every active target in
        // unit index order. The fork has no taunt, so the shout changes no target's aim or swing.
        if let Some(shout) = Self::spell_of(sim, unit, data.challenging_shout.highest()) {
            if let Some(position) = sim
                .unit(unit)
                .spellbook
                .iter()
                .position(|candidate| *candidate == shout)
            {
                effects.push(json!({"kind": "challenging_shout", "spell": position}));
            }
        }
        // taunt.go and intimidating_shout.go: an always hit outcome with no damage on the
        // target, with the rage, global cooldown and cooldown the shared cast applies (Taunt
        // needs Defensive Stance). The fork has no taunt and no fear, so neither changes a
        // target's aim, swing or threat, and the shout does not reach the other targets.
        // talents_protection.go Concussion Blow: a special hit outcome with no damage, refunded
        // on a miss; the fork's stun changes no target's swing. Go's exporter has no effect for
        // it, so only Forever's rules describe it.
        for (kind, rank) in [
            ("taunt", data.taunt.highest()),
            ("intimidating_shout", data.intimidating_shout.highest()),
            ("concussion_blow", data.concussion_blow.highest()),
        ] {
            if kind == "concussion_blow" && sim.rules != Rules::Forever {
                continue;
            }
            if let Some(spell) = Self::spell_of(sim, unit, rank) {
                if let Some(position) = sim
                    .unit(unit)
                    .spellbook
                    .iter()
                    .position(|candidate| *candidate == spell)
                {
                    effects.push(json!({"kind": kind, "spell": position}));
                }
            }
        }
        if talent("bloodthrill") > 0 {
            effects.push(
                json!({"kind": "bloodthrill", "trigger_aura": "Bloodthrill - Trigger",
                "proc_chance": data.bloodthrill.fraction_at(talent("bloodthrill")),
                // Client 70425: the proc opens the 5 sec Overpower window (1282733).
                "window_ns": data.bloodthrill_triggered.by_id(1282733).duration(),
                "delay_ns": SPELL_BATCH_WINDOW}),
            );
        }
        if talent("weaponmaster") > 0 && sim.get_aura(unit, "Weaponmaster (Sword)").is_some() {
            let swords = weapon_types_mask(sim, unit, &["WeaponTypeSword"]);
            let mut hands = Vec::new();
            if swords.matches(ProcMask::MELEE_MH) {
                hands.push("main");
            }
            if swords.matches(ProcMask::MELEE_OH) {
                hands.push("off");
            }
            effects.push(json!({"kind": "weaponmaster_sword",
                "trigger_aura": "Weaponmaster (Sword)",
                "proc_chance": data.weaponmaster.effect_at(3).fraction_at(talent("weaponmaster")),
                "extra_attack_tag": 1290261, "sword_hands": hands,
                // 16 October 2026 beta notes: Shield Bash and Shield Slam no longer trigger it.
                "excluded_class_spells": ["shield_bash", "shield_slam"]}));
        }
        // heroic_strike_cleave.go: each strike's queue spell and aura, the queue delay and the
        // base damage the strike adds to a main hand weapon swing.
        let mut strikes = Vec::new();
        for (ladder, cleave) in [(&data.heroic_strike, false), (&data.cleave, true)] {
            let row = ladder.highest_for(env.rules);
            let Some(spell) = Self::spell_of(sim, unit, row) else {
                continue;
            };
            strikes.push(json!({"spell_id": sim.spell(spell).action_id.spell_id,
                "queue_aura": format!("HS/Cleave Queue Aura-{}",
                    action_id_string(&sim.spell(spell).action_id)),
                "base_damage": row.damage_effect().average(level), "cleave": cleave}));
        }
        effects.push(json!({"kind": "heroic_strike_queue",
            "queue_delay_ns": i64::from(self.inputs.queue_delay) * crate::prepare::sim::MILLISECOND,
            "strikes": strikes,
            // Patch 184: a Cleave that lands on several targets splits its flat threat.
            "cleave_threat_split": true}));
        // overpower.go: a dodge opens the Overpower window.
        effects.push(
            json!({"kind": "overpower_window", "trigger_aura": "Overpower - Trigger",
            "aura": "Overpower Aura"}),
        );
        effects.extend(self.tank_effects(env, notes));
        effects
    }

    /// Go `warriorTankEffects`: the effects that act on hits the warrior takes and the tank's
    /// own spells. A player tanking the target is refused before it comes to this, so a hit
    /// taken needs the Goblin Sapper Charge's self hit, which the export refuses too.
    fn tank_effects(&self, env: &Environment, notes: &mut Vec<String>) -> Vec<Value> {
        let sim = &env.sim;
        let unit = env.player;
        let data = spell_data();
        let level = CHARACTER_LEVEL;
        let target = env.encounter.targets[0];
        let tanking = sim.unit(target).current_target == Some(unit);
        let takes_damage = crate::prepare::export::player_takes_damage(env);
        let mut effects = Vec::new();
        let inert = |label: &str, effects: &mut Vec<Value>| {
            if sim.get_aura(unit, label).is_some() {
                effects.push(
                    json!({"kind": "inert_listener", "unit": "player", "aura": label,
                    "reason": "acts only on hits the player takes"}),
                );
            }
        };
        // revenge.go. Under Forever's rules a warrior off the target's threat still has the
        // strike, which no hit taken ever opens.
        if !tanking && sim.rules != Rules::Forever {
            inert("Revenge - Trigger", &mut effects);
        } else if sim.get_aura(unit, "Revenge - Trigger").is_some() {
            let row = data.revenge.highest_for(env.rules);
            effects.push(json!({"kind": "revenge", "spell_id": row.id,
                "trigger_aura": "Revenge - Trigger", "aura": "Revenge",
                "damage": damage_roll(row), "attack_power_share": 0.25}));
        }
        // talents_protection.go registerShieldSlam
        if self.has_talent("shield_slam") {
            let row = data.shield_slam.highest();
            effects.push(json!({"kind": "shield_slam", "spell_id": row.id,
                "damage": damage_roll(row),
                "can_block": sim.unit(unit).pseudo_stats.can_block}));
        }
        // thunder_clap.go: the row's average plus a share of attack power; a landed clap slows
        // the target by its bid in the attack speed category, which holds only the clap, or the
        // clap and Thunderfury's Cyclone, whose gains and expiries the runtime follows with the
        // category's active effect.
        let clap_row = data.thunder_clap.highest();
        if let Some(clap) = Self::spell_of(sim, unit, clap_row) {
            let slow = clap_row.effects[1].base_value();
            let bonus = sim.state(self.thunder_clap_effect_bonus);
            let aura = sim
                .spell(clap)
                .related_aura_arrays
                .values()
                .last()
                .and_then(|auras| {
                    auras
                        .get(sim.unit(target).unit_index as usize)
                        .copied()
                        .flatten()
                });
            // The category's other members, Cyclone aside: (bid, permanent and up at the reset).
            let members = aura.and_then(|aura| {
                (sim.aura(aura).exclusive_effects.len() == 1).then(|| {
                    let category = sim.effects[sim.aura(aura).exclusive_effects[0].0].category;
                    let effects = &sim.categories[category.0].effects;
                    let cyclone = effects
                        .iter()
                        .any(|member| sim.aura(sim.effects[member.0].aura).label == "Cyclone");
                    let others: Vec<(f64, bool)> = effects
                        .iter()
                        .map(|member| &sim.effects[member.0])
                        .filter(|member| {
                            member.aura != aura && sim.aura(member.aura).label != "Cyclone"
                        })
                        .map(|member| {
                            let other = sim.aura(member.aura);
                            (
                                member.priority,
                                other.active && other.duration == NEVER_EXPIRES,
                            )
                        })
                        .collect();
                    (cyclone, others)
                })
            });
            let multiplier = 1.0 - (slow * (1.0 + bonus)) / 100.0;
            let bid = 1.0 - 1.0 / multiplier;
            // A permanent slow of the same category up from the pull, as a raid's Thunder Clap
            // debuff: on an equal or lower bid the clap takes the category while it is up (Go
            // `keepsTie`, the same spell), and the slow back once it fades is the permanent
            // one's. The clap so slows only by what it bids over it: none on an equal bid.
            let relative = match &members {
                Some((_, others)) if others.is_empty() => Some(bid),
                Some((false, others)) if others.iter().all(|(_, permanent)| *permanent) => {
                    let held = others.iter().map(|(bid, _)| *bid).fold(0.0, f64::max);
                    Some(1.0 - (1.0 - bid.max(held)) / (1.0 - held))
                }
                _ => None,
            };
            match (aura, relative) {
                (Some(aura), Some(bid)) => {
                    effects.push(json!({"kind": "thunder_clap", "spell_id": clap_row.id,
                        "base_damage": clap_row.damage_effect().average(level),
                        "attack_power_share": 0.0255,
                        "max_targets": i32::from(clap_row.max_targets),
                        "aura": sim.aura(aura).label,
                        "bid": bid}));
                }
                _ => notes.push("Thunder Clap's slow shares its category".to_string()),
            }
        }
        // talents_protection.go registerLastStand
        if let Some(aura) = sim.get_aura(unit, "Last Stand") {
            let buff = data.last_stand_triggered.highest();
            effects.push(json!({"kind": "last_stand", "spell_id": data.last_stand.highest().id,
                "aura": sim.aura(aura).label,
                "health_share": last_stand_share(buff),
                "metrics_action_id": serde_json::to_value(sim.aura(aura).action_id.clone().unwrap_or_default())
                    .expect("an action id serializes")}));
        }
        // shield_wall.go: a survival cooldown a tank autocasts below 40% health.
        if let Some(aura) = sim.get_aura(unit, "Shield Wall") {
            effects.push(
                json!({"kind": "shield_wall", "spell_id": data.shield_wall.highest().id,
                "aura": sim.aura(aura).label, "autocast": !self.dps_spec, "health_percent": 0.4,
                "can_block": sim.unit(unit).pseudo_stats.can_block}),
            );
        }
        // shield_block.go: in Defensive Stance with a shield, an aura of block chance whose
        // stacks the player's blocks spend. Only the rotation casts it.
        if let Some(aura) = sim.get_aura(unit, "Shield Block") {
            if rotation_names_shield_block(sim, unit) {
                effects.push(
                    json!({"kind": "shield_block", "spell_id": data.shield_block.highest().id,
                    "aura": sim.aura(aura).label,
                    "can_block": sim.unit(unit).pseudo_stats.can_block}),
                );
            }
        }
        // retaliation.go
        if let Some(aura) = sim.get_aura(unit, "Retaliation") {
            let row = data.retaliation.highest();
            let hit = data.retaliation_triggered.highest();
            effects.push(json!({"kind": "retaliation", "spell_id": row.id,
                "aura": sim.aura(aura).label, "hit_spell_id": hit.id,
                "charges": i32::from(row.proc_charges),
                "hit_base_damage": hit.damage_effect().average(level)}));
        }
        // talents_protection.go registerRageOnAvoid
        let mut avoid = Vec::new();
        if self.talent("shield_specialization") > 0 {
            let energize = data.shield_specialization_triggered.highest();
            avoid.push(
                json!({"aura": "Shield Specialization", "spell_id": energize.id,
                "rage": energize.energize_effect().tenths(), "outcomes": ["block"],
                "needs_block": false,
                "chance": data.shield_specialization.effect_at(2)
                    .fraction_at(self.talent("shield_specialization"))}),
            );
        }
        if self.talent("master_of_defense") > 0 {
            let energize = data.master_of_defense_triggered.highest();
            avoid.push(json!({"aura": "Master of Defense", "spell_id": energize.id,
                "rage": energize.energize_effect().tenths(), "outcomes": ["dodge", "parry"],
                "needs_block": true,
                "chance": data.master_of_defense.fraction_at(self.talent("master_of_defense"))}));
        }
        if !avoid.is_empty() {
            effects.push(json!({"kind": "rage_on_avoid",
                "can_block": sim.unit(unit).pseudo_stats.can_block, "triggers": avoid}));
        }
        // talents_fury.go registerEnrage
        if self.talent("enrage") > 0 {
            if let Some(enrage) = sim.get_aura(unit, "Enrage") {
                if takes_damage {
                    effects.push(json!({"kind": "warrior_enrage",
                        "trigger_aura": "Enrage - Trigger", "aura": sim.aura(enrage).label,
                        "proc_chance": f64::from(data.enrage.rank(self.talent("enrage")).proc_chance) / 100.0,
                        "physical_damage_done": data.enrage.fraction_at(self.talent("enrage"))}));
                    if let Some(effect) =
                        exclusive_category_effect(sim, unit, "player", "Enrage", notes)
                    {
                        effects.push(effect);
                    }
                } else {
                    inert("Enrage - Trigger", &mut effects);
                }
            }
        }
        // talents_fury.go registerBloodCraze
        if self.talent("blood_craze") > 0 {
            if let Some(hot) = sim.get_spell(unit, &spell_action(16488)) {
                effects.push(json!({"kind": "blood_craze",
                    "spell_id": sim.spell(hot).action_id.spell_id,
                    "heal": heal_modifiers(env, notes),
                    "damage_taken_aura": "Blood Craze - Damage Taken",
                    "health_fraction": data.blood_craze.effect_at(1)
                        .fraction_at(self.talent("blood_craze")),
                    "hit_threshold": data.blood_craze.effect_at(2)
                        .fraction_at(self.talent("blood_craze"))}));
            }
        }
        // talents_protection.go registerImprovedShieldBash
        if let Some(aura) = sim.get_aura(unit, "Improved Shield Bash") {
            effects.push(json!({"kind": "inert_listener", "unit": "player",
                "aura": sim.aura(aura).label,
                "reason": "acts only on Shield Bash hits, and the gate rejects a rotation that reaches Shield Bash"}));
        }
        // talents_arms.go registerImprovedHamstring
        if let Some(aura) = sim.get_aura(unit, "Improved Hamstring - Trigger") {
            let row = data.hamstring.highest();
            let root_id = data.improved_hamstring_triggered.highest().id;
            let root = if Self::spell_of(sim, unit, row).is_some() {
                sim.unit(target).auras.iter().copied().rfind(|other| {
                    sim.aura(*other)
                        .action_id
                        .as_ref()
                        .is_some_and(|id| id.spell_id == root_id)
                })
            } else {
                None
            };
            match root {
                Some(root) => effects.push(json!({"kind": "improved_hamstring",
                    "trigger_aura": sim.aura(aura).label, "aura": sim.aura(root).label,
                    "proc_chance": data.improved_hamstring
                        .fraction_at(self.talent("improved_hamstring")),
                    "delay_ns": SPELL_BATCH_WINDOW})),
                None => notes.push("Improved Hamstring has no root aura".to_string()),
            }
        }
        effects
    }
}

/// Go `GetProcMaskForTypes` for the weapon types of the equipped hands.
pub(super) fn weapon_types_mask(sim: &Sim, unit: UnitId, types: &[&str]) -> ProcMask {
    use crate::prepare::items::slot;
    let mut mask = ProcMask::UNKNOWN;
    for (slot, hand) in [
        (slot::RANGED, ProcMask::RANGED),
        (slot::MAIN_HAND, ProcMask::MELEE_MH),
        (slot::OFF_HAND, ProcMask::MELEE_OH),
    ] {
        let item = &sim.character(unit).equipment[slot];
        if !item.is_empty() && types.contains(&item.weapon_type.as_str()) {
            mask = mask | hand;
        }
    }
    mask
}
