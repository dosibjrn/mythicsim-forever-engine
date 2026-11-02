//! The exporter's `paladinTankEffects` and `paladinHealEffect` (tools/oracle-v2/paladin.go): the
//! talents and spells of a paladin tanking the target, and the heal rolls.

use serde_json::{json, Value};

use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::dbcenums;
use crate::prepare::env::Environment;
use crate::prepare::export::action_id;
use crate::prepare::rotation::rotation_spells;
use crate::prepare::sim::{Rules, Sim, UnitId, NEVER_EXPIRES};
use crate::prepare::spelldata::Spell as Row;

use super::export::{heal_modifiers, GREATER_BLESSING_OF_LIGHT_CATEGORY};
use super::hotfix_113554 as hotfix;
use super::spell_data::spell_data;
use super::talent_spells::HOLY_SHOCK_RANKS;
use super::util::{spell_action, tagged_action};
use super::Paladin;

impl Paladin {
    /// Go `paladinTankEffects`: the talents and spells of a paladin tanking the target.
    pub(super) fn tank_effects(&self, env: &Environment, label: &str) -> Vec<Value> {
        let sim = &env.sim;
        let unit = env.player;
        let talents = &self.talents;
        let data = spell_data();
        let mut effects: Vec<Value> = Vec::new();
        let rotation_spells = rotation_spells(sim, unit);

        // righteous_fury.go: a Holy threat mod while the aura holds; Instrument of Law's
        // threat reduction holds only while it is down.
        let fury = data.righteous_fury.highest();
        let mut righteous_fury = json!({
            "kind": "righteous_fury", "spell_id": fury.id, "aura": "Righteous Fury",
            "threat_percent": super::RIGHTEOUS_FURY_HOLY_THREAT,
        });
        let law = talents.i32("instrument_of_law");
        if law > 0 {
            righteous_fury["instrument_of_law"] = json!({
                "aura": format!("Instrument of Law{label}"),
                "multiplier": 1.0 - data.instrument_of_law
                    .effect(dbcenums::A_MOD_THREAT, 127)
                    .fraction_at(law),
            });
        }
        if sim.rules == Rules::Forever {
            // Push 113554's effect 1367626: +33% to all threat while the aura holds.
            righteous_fury["all_threat_multiplier"] =
                json!(1.0 + hotfix::RIGHTEOUS_FURY_ALL_THREAT);
        }
        effects.push(righteous_fury);
        if talents.bool("swift_judgement") && sim.rules == Rules::Forever {
            // Push 113554: a second Judgement on its own cooldown.
            effects.push(json!({
                "kind": "second_judgement", "spell_id": data.swift_judgement.highest().id,
            }));
        } else if talents.bool("swift_judgement") {
            // swift_judgement.go
            let rank = data.swift_judgement.highest();
            effects.push(json!({
                "kind": "swift_judgement", "spell_id": rank.id,
                "aura": format!("Swift Judgement{label}"),
                "cost_percent_add": rank
                    .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_COST)
                    .percent(),
            }));
        }
        if talents.bool("templars_bulwark") {
            // templars_bulwark.go: the survival cooldown's absorb shield and Forbearance
            let rank = data.templars_bulwark.highest();
            effects.push(json!({
                "kind": "templars_bulwark", "spell_id": rank.id,
                "aura": format!("Templar's Bulwark{label}"),
                "health_share": rank.effect(dbcenums::A_SCHOOL_ABSORB, 127).percent(),
            }));
        }
        let redoubt = talents.i32("redoubt");
        if redoubt > 0 {
            // talents_protection.go applyRedoubt: the chance is a Go literal
            effects.push(json!({
                "kind": "redoubt", "trigger_aura": format!("Redoubt - Trigger{label}"),
                "aura": format!("Redoubt{label}"), "proc_chance": 0.02 * f64::from(redoubt),
            }));
        }
        let shield_specialization = talents.i32("shield_specialization");
        if shield_specialization > 0 {
            // talents_protection.go applyShieldSpecialization
            let share = data.shield_specialization_triggered.highest();
            effects.push(json!({
                "kind": "shield_specialization",
                "trigger_aura": format!("Shield Specialization{label}"),
                "proc_chance": Self::shield_specialization_chance(sim, shield_specialization),
                // The 16 October 2026 beta notes cut it from 6% to 4%; the row still says 6.
                "mana_share": 0.04,
                "metrics_action_id": action_id(Some(&spell_action(share.id))),
                // No Threat since client 70374 (beta notes 8 October).
                "no_threat": share.no_threat(),
            }));
        }
        let reckoning = talents.i32("reckoning");
        if reckoning > 0 {
            // talents_protection.go applyReckoning
            let block = data.reckoning.fraction_at(reckoning);
            // Since the 16 October 2026 beta notes each trigger banks a charge, up to 4, behind the
            // triggers' shared cooldown, and the next main hand auto, landed or not, spends them all
            // on one free main hand swing each that leaves the swing timer alone.
            let spenders: Vec<usize> = sim
                .unit(unit)
                .spellbook
                .iter()
                .enumerate()
                .filter(|(_, &spell)| {
                    sim.spell(spell)
                        .proc_mask
                        .matches(crate::prepare::spell::ProcMask::MELEE_MH_AUTO)
                })
                .map(|(i, _)| i)
                .collect();
            effects.push(json!({
                "kind": "reckoning", "block_aura": format!("Reckoning - Block{label}"),
                "crit_aura": format!("Reckoning - Crit{label}"),
                "block_chance": block, "crit_chance": block * 2.5,
                "charges_aura": format!("Reckoning{label}"),
                "icd_ns": data.reckoning.highest().icd(),
                "spender_spells": spenders,
            }));
        }
        if talents.i32("iron_creed") > 0 {
            // talents_protection.go applyIronCreed
            effects.push(json!({
                "kind": "iron_creed", "trigger_aura": format!("Iron Creed - Trigger{label}"),
                "aura": format!("Iron Creed{label}"),
            }));
        }
        // talents_protection.go: Improved Righteous Fury and Iron Creed multiply the paladin's
        // damage taken while their auras hold, which the runtime tracks live.
        let mut damage_taken: Vec<Value> = Vec::new();
        let improved_righteous_fury = talents.i32("improved_righteous_fury");
        if improved_righteous_fury > 0 {
            damage_taken.push(json!({
                "aura": "Righteous Fury", "stat": "damage_taken",
                "multiplier": Self::improved_righteous_fury_multiplier(sim, improved_righteous_fury),
            }));
        }
        let iron_creed = talents.i32("iron_creed");
        if iron_creed > 0 {
            damage_taken.push(json!({
                "aura": format!("Iron Creed{label}"), "stat": "damage_taken",
                "multiplier": Self::iron_creed_multiplier(sim, iron_creed),
            }));
        }
        if !damage_taken.is_empty() {
            effects.push(json!({"kind": "pseudo_stat_auras", "auras": damage_taken}));
        }
        let illumination = talents.i32("illumination");
        if illumination > 0 {
            // talents_holy.go applyIllumination: heal crits, the chance and the share of the
            // base cost
            effects.push(json!({
                "kind": "illumination", "trigger_aura": format!("Illumination{label}"),
                "proc_chance": data.illumination.effect_at(1).fraction_at(illumination),
                "refund": data.illumination.effect_at(3).fraction_at(illumination),
                "metrics_action_id": action_id(Some(&spell_action(data.illumination.highest().id))),
            }));
        }
        if talents.bool("holy_shield") {
            // holy_shield.go, the ranks the rotation names
            let mut ranks: Vec<Value> = Vec::new();
            data.holy_shield.each(|_, rank| {
                if !rotation_spells.contains(&i64::from(rank.id)) {
                    return;
                }
                let proc_action = tagged_action(rank.id, 2);
                let mut proc_spell: i64 = -1;
                for (i, spell) in sim.unit(unit).spellbook.iter().enumerate() {
                    if sim.spell(*spell).action_id == proc_action {
                        proc_spell = i as i64;
                    }
                }
                ranks.push(json!({
                    "spell_id": rank.id, "proc_spell": proc_spell,
                    "aura": format!("Holy Shield{label} Rank {}", rank.rank_number()),
                    "charges": i32::from(rank.proc_charges),
                    "damage": Self::holy_shield_damage(sim, rank),
                }));
            });
            effects.push(json!({
                "kind": "holy_shield", "ranks": ranks,
                // The cast's ExtraCastCondition: a shield to block with, static in scope.
                "can_block": sim.unit(unit).pseudo_stats.can_block,
            }));
        }
        if self.new_talent("shieldsworn") > 0 {
            // Push 113554's Shieldsworn: a landed Holy Strike triggers Holy Shield and Shield
            // Specialization (upstream #772's holy_shield.go and talents_protection.go).
            effects.push(json!({"kind": "shieldsworn"}));
        }
        effects
    }

    /// A Holy Shield rank's damage: push 113554's under Forever's rules, where each rank has
    /// the damage of the rank below (hotfix_113554.rs), the row's in the reference.
    pub(super) fn holy_shield_damage(sim: &Sim, rank: &Row) -> f64 {
        if sim.rules == Rules::Forever {
            if let Some(&(_, damage, _)) =
                hotfix::HOLY_SHIELD.iter().find(|(id, _, _)| *id == rank.id)
            {
                return damage;
            }
        }
        rank.effect(dbcenums::A_PROC_TRIGGER_DAMAGE, 0)
            .average(CHARACTER_LEVEL)
    }

    /// Go `paladinHealEffect`: every heal rank's roll, the Libram of Light's Flash of Light
    /// bonus, Blessing of Light's bonuses on a unit that carries it, and the healing modifiers
    /// of the paladin on itself and on the target, which a plain cast heals.
    pub(super) fn heal_effect(&self, env: &Environment, notes: &mut Vec<String>) -> Value {
        let sim = &env.sim;
        let data = spell_data();
        let mut ranks: Vec<Value> = Vec::new();
        let mut roll = |name: &str, rank: &Row| {
            let heal = rank.heal_effect();
            let average = heal.average(CHARACTER_LEVEL);
            ranks.push(json!({
                "spell_id": rank.id, "heal": name,
                "min": average * (1.0 - heal.variance / 2.0),
                "max": average * (1.0 + heal.variance / 2.0),
                "average": average, "variance": heal.variance,
            }));
        };
        data.holy_light.each(|_, rank| roll("holy_light", rank));
        data.flash_of_light
            .each(|_, rank| roll("flash_of_light", rank));
        for rank in &HOLY_SHOCK_RANKS {
            ranks.push(json!({
                "spell_id": rank.heal_id, "heal": "holy_shock_heal",
                "min": rank.heal, "max": rank.heal,
            }));
        }
        let blessing = data.greater_blessing_of_light.highest();
        let mut unit = |u: UnitId| {
            let mut modifiers = heal_modifiers(env, u);
            modifiers.insert(
                "bonus_healing_taken".into(),
                json!(sim.unit(u).pseudo_stats.bonus_healing_taken),
            );
            modifiers.insert(
                "blessing_of_light".into(),
                json!(blessings(sim, u).iter().any(|aura| sim.aura(*aura).active)),
            );
            // A blessing the fight could gain or lose would change the bonus mid-fight.
            for aura in blessings(sim, u) {
                let aura = sim.aura(aura);
                if aura.active != (aura.duration == NEVER_EXPIRES) {
                    notes.push(
                        "a Blessing of Light that can change during the fight is unsupported"
                            .to_string(),
                    );
                }
            }
            Value::Object(modifiers)
        };
        let player = unit(env.player);
        let target = unit(env.encounter.targets[0]);
        json!({
            "kind": "paladin_heals", "ranks": ranks,
            "flash_of_light_bonus": self.flash_of_light_bonus_healing,
            "blessing_holy_light": blessing.effect_n(1).average(CHARACTER_LEVEL),
            "blessing_flash_of_light": blessing.effect_n(2).average(CHARACTER_LEVEL),
            "player": player, "target": target,
        })
    }
}

/// The auras on a unit carrying the Greater Blessing of Light category tag.
fn blessings(sim: &Sim, unit: UnitId) -> Vec<crate::prepare::sim::AuraId> {
    sim.auras_with_tag(unit, GREATER_BLESSING_OF_LIGHT_CATEGORY)
}
