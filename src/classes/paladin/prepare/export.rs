//! The exporter's Paladin part, tools/oracle-v2/paladin.go: the client damage rows of the
//! Paladin's spells (`paladinDamageRows`) and the effects whose parameters Go keeps in
//! closures (`paladinEffects`).

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::{json, Value};

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::dbcenums;
use crate::prepare::env::Environment;
use crate::prepare::export::action_id;
use crate::prepare::rotation::rotation_spells;
use crate::prepare::sim::{Sim, SpellId, UnitId, MILLISECOND, SECOND};
use crate::prepare::spell::{school, ProcMask, SpellFlag};
use crate::prepare::spelldata::{must_find, Ladder, Spell as Row};
use crate::prepare::stats::Stat;

use super::masks;
use super::spell_data::spell_data;
use super::talent_spells::{HOLY_SHOCK_RANKS, LIGHTS_VIGIL_TRIGGERED};
use super::util::spell_action;
use super::Paladin;

/// Go `core.SpellBatchWindow`.
/// The players Light's Vigil's party heal reaches: the target and its party of five, the party
/// the healer sims model (`prepare::party_damage`).
const VIGIL_PARTY: u32 = 5;

const SPELL_BATCH_WINDOW: i64 = 10 * MILLISECOND;

/// Go `sanctifiedJudgementLogScale`: the game returns 10/9 of the stated share of the seal's cost.
const SANCTIFIED_JUDGEMENT_LOG_SCALE: f64 = 10.0 / 9.0;

/// Go `buffs.JudgementAuraTag`.
const JUDGEMENT_AURA_TAG: &str = "JudgementAura";

/// Go `buffs.GreaterBlessingOfLightCategory`.
pub(super) const GREATER_BLESSING_OF_LIGHT_CATEGORY: &str = "BlessingOfLight";

/// seal_of_righteousness.go `sealOfRighteousnessProcIDs`: the damage spell each rank fires.
const SEAL_OF_RIGHTEOUSNESS_PROC_IDS: [(i32, i32); 8] = [
    (1, 25741),
    (2, 25740),
    (3, 25739),
    (4, 25738),
    (5, 25737),
    (6, 25736),
    (7, 25735),
    (8, 25713),
];

/// The client rows paladinDamageRows lists, by spell ID: every Holy Strike rank's flat part,
/// every Hammer of Wrath, Exorcism, Holy Wrath and Judgement of Righteousness rank, each Seal of
/// Fury rank's judgement, each Light's Vigil enemy strike and, for every Judgement of Command
/// rank, the triggered row its first effect names.
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
        each(&mut rows, &data.holy_strike);
        each(&mut rows, &data.hammer_of_wrath);
        each(&mut rows, &data.exorcism);
        each(&mut rows, &data.holy_wrath);
        each(&mut rows, &data.judgement_of_righteousness);
        // seal_of_fury.go: each rank's judgement, the triggered row its third effect names.
        data.seal_of_fury.each(|_, rank| {
            let row = data
                .seal_of_fury_triggered
                .by_id(rank.effect_n(3).base_value() as i32);
            rows.insert(row.id, row);
        });
        // lights_vigil.go: each rank's enemy strike.
        for (_, _, _, strike) in LIGHTS_VIGIL_TRIGGERED {
            let row = must_find(strike);
            rows.insert(row.id, row);
        }
        data.judgement_of_command.each(|_, row| {
            rows.insert(
                row.id,
                data.seal_of_command_triggered
                    .by_id(row.effect_n(1).base_value() as i32),
            );
        });
        rows
    })
}

/// Go `attachDamageEffects` for one spell: the roll of the client row, `{average, variance}`.
pub(super) fn damage_effect(sim: &Sim, spell: SpellId) -> Option<Value> {
    let action = &sim.spell(spell).action_id;
    if action.spell_id == 0 || action.tag != 0 {
        return None;
    }
    let row = damage_rows().get(&action.spell_id)?;
    let effect = row.damage_effect();
    if effect.is_nil() {
        return None;
    }
    Some(json!({"average": effect.average(CHARACTER_LEVEL), "variance": effect.variance}))
}

/// The positions of the spells in the spellbook that satisfy a predicate.
fn spell_positions(sim: &Sim, unit: UnitId, mut keep: impl FnMut(SpellId) -> bool) -> Vec<usize> {
    sim.unit(unit)
        .spellbook
        .iter()
        .enumerate()
        .filter_map(|(i, spell)| keep(*spell).then_some(i))
        .collect()
}

/// Go `spellsMatching`.
fn spells_matching(sim: &Sim, unit: UnitId, mask: i64) -> Vec<usize> {
    spell_positions(sim, unit, |spell| sim.spell(spell).matches(mask))
}

/// Go `exclusiveCategoryEffect`: the members of a single aura exclusive category on a unit.
fn exclusive_category_effect(
    sim: &Sim,
    unit: UnitId,
    side: &str,
    name: &str,
    notes: &mut Vec<String>,
) -> Option<Value> {
    for category in &sim.unit(unit).categories {
        let category = &sim.categories[category.0];
        if category.name != name {
            continue;
        }
        if !category.single_aura {
            notes.push(format!("exclusive category {name} holds several auras"));
            return None;
        }
        let members: Vec<Value> = category
            .effects
            .iter()
            .map(|effect| {
                let effect = &sim.effects[effect.0];
                let aura = sim.aura(effect.aura);
                json!({"aura": aura.label, "priority": effect.priority,
                       "spell_id": aura.action_id.as_ref().map_or(0, |id| id.spell_id)})
            })
            .collect();
        return Some(
            json!({"kind": "exclusive_category", "unit": side, "category": name, "members": members}),
        );
    }
    None
}

/// Go `healModifiers`.
pub(super) fn heal_modifiers(env: &Environment, target: UnitId) -> serde_json::Map<String, Value> {
    let sim = &env.sim;
    let pseudo = &sim.unit(env.player).pseudo_stats;
    let mut modifiers = serde_json::Map::new();
    modifiers.insert(
        "healing_dealt_multiplier".into(),
        json!(pseudo.healing_dealt_multiplier),
    );
    modifiers.insert(
        "periodic_healing_dealt_multiplier".into(),
        json!(pseudo.periodic_healing_dealt_multiplier),
    );
    modifiers.insert(
        "healing_taken_multiplier".into(),
        json!(sim.unit(target).pseudo_stats.healing_taken_multiplier),
    );
    modifiers.insert(
        "table_healing_dealt_multiplier".into(),
        json!(
            env.attack_table(env.player, target)
                .healing_dealt_multiplier
        ),
    );
    modifiers.insert(
        "healing_power".into(),
        json!(
            sim.stat(env.player, Stat::HealingPower)
                + sim.unit(target).pseudo_stats.bonus_healing_taken
        ),
    );
    modifiers
}

impl Paladin {
    /// The Seal of Righteousness proc id of a rank.
    fn righteousness_proc_id(rank: i32) -> i32 {
        SEAL_OF_RIGHTEOUSNESS_PROC_IDS
            .iter()
            .find(|(n, _)| *n == rank)
            .map_or(0, |(_, id)| *id)
    }

    /// Go `sealLabel` for the aura a seal rank puts up.
    fn seal_aura_label(name: &str, label: &str, rank: &Row) -> String {
        super::seals::seal_label(name, label, rank)
    }

    /// Go `paladinEffects`, with the notes `paladinUnrepresented` and the effects add.
    pub(super) fn export_effects_with_notes(
        &self,
        env: &Environment,
        notes: &mut Vec<String>,
    ) -> Vec<Value> {
        let sim = &env.sim;
        let unit = env.player;
        let talents = &self.talents;
        let label = sim.unit(unit).label.clone();
        let target = env.encounter.targets[0];
        let target_index = sim.unit(target).unit_index as usize;
        let data = spell_data();
        let mut effects: Vec<Value> = Vec::new();
        let rotation_spells = rotation_spells(sim, unit);

        // seal_of_command.go doubles its judgement on a stunned target.
        if sim.unit(target).pseudo_stats.stunned {
            notes.push("Judgement of Command against a stunned target is unsupported".to_string());
        }

        // judgement.go: every landed melee strike refreshes the active judgement debuffs.
        let judgements: Vec<String> = self
            .judgement_auras
            .iter()
            .filter_map(|auras| auras.get(target_index).copied().flatten())
            .map(|aura| sim.aura(aura).label.clone())
            .collect();
        effects.push(json!({
            "kind": "judgement_refresh", "trigger_aura": format!("Judgement Refresh{label}"),
            "proc_mask": ProcMask::MELEE.names(), "judgement_auras": judgements,
        }));
        // judgement.go: the active seal's judgement, then a rotation wake a batch window after
        // the cooldown ends.
        let judgement = self
            .judgement
            .map_or(0, |spell| sim.spell(spell).action_id.spell_id);
        effects.push(json!({
            "kind": "judgement", "spell_id": judgement, "wake_delay_ns": SPELL_BATCH_WINDOW,
        }));
        // seals.go dealAfterBatch: seal procs deal their damage a batch window after the hit.
        let deal_delay = SPELL_BATCH_WINDOW;
        if talents.bool("seal_of_command") {
            // seal_of_command.go
            let proc_rank = data.seal_of_command_triggered.rank(1);
            let proc_effect = proc_rank.effect_n(1);
            let mut ranks = Vec::new();
            data.seal_of_command.each(|_, rank| {
                ranks.push(json!({
                    "seal_spell_id": rank.id,
                    "aura": Self::seal_aura_label("Seal of Command", &label, rank),
                    "judgement_spell_id": data.judgement_of_command.rank(rank.rank_number()).id,
                }));
            });
            // NewLegacyPPMManager(7, ProcMaskMeleeWhiteHit), rolled for the main hand auto: 7
            // procs a minute is a Go literal; the chance comes from Go's own proc manager.
            let dpm = sim.new_ppm_manager(unit, 7.0, ProcMask::MELEE_WHITE_HIT);
            effects.push(json!({
                "kind": "seal_of_command", "ranks": ranks, "proc_spell_id": proc_rank.id,
                "weapon_percent": proc_effect.percent()
                    * (1.0 + Self::improved_seals_bonus(sim, talents.i32("improved_seals"))),
                "coefficient": proc_effect.coeff(),
                // Go `SealOfCommandBonusTakesImprovedSeals` (MythicSim patch 169).
                "target_bonus_coefficient": proc_effect.coeff()
                    * (1.0 + Self::improved_seals_bonus(sim, talents.i32("improved_seals"))),
                "proc_chance": dpm.chance(ProcMask::MELEE_MH_AUTO),
                "rng_label": "Seal of Command", "icd_ns": SECOND, "deal_delay_ns": deal_delay,
            }));
        }
        {
            // seal_of_righteousness.go
            let mh = &sim.character(unit).equipment[crate::prepare::items::slot::MAIN_HAND];
            let hand_multiplier = if mh.hand_type == "HandTypeTwoHand" {
                1.2
            } else {
                0.85
            };
            // The seal's aura holds only the per-hit value (a dummy aura); each hit is computed as
            // it lands from the weapon in the hand, its spell power share too ($<1h> or $<2h> in
            // the tooltip), which the registered spell holds for the weapon worn at the pull.
            // Only a swap between a one-hander and a two-hander moves it, so only then does each
            // rank carry the share of this weapon.
            let two_hand = |item: &crate::prepare::items::Item| item.hand_type == "HandTypeTwoHand";
            let share_follows_swap = sim.item_swap(unit).is_some_and(|swap| {
                let main_hand = crate::prepare::items::slot::MAIN_HAND;
                two_hand(&swap.set(false)[main_hand]) != two_hand(&swap.set(true)[main_hand])
            });
            let mut ranks = Vec::new();
            data.seal_of_righteousness.each(|_, rank| {
                let judge_rank = data
                    .judgement_of_righteousness
                    .by_id(rank.effect_n(2).base_value() as i32);
                let mut row = json!({
                    "seal_spell_id": rank.id,
                    "aura": Self::seal_aura_label("Seal of Righteousness", &label, rank),
                    "judgement_spell_id": judge_rank.id,
                    "proc_spell_id": Self::righteousness_proc_id(rank.rank_number()),
                    "per_hit_value": judge_rank.effect_n(2).average(CHARACTER_LEVEL),
                });
                if share_follows_swap {
                    row["hit_coefficient"] = json!(super::seals::righteousness_hit_coefficient(
                        judge_rank.effect_n(2).coeff(),
                        two_hand(mh)
                    ));
                }
                ranks.push(row);
            });
            effects.push(json!({
                "kind": "seal_of_righteousness", "ranks": ranks, "hand_multiplier": hand_multiplier,
                "swing_speed": mh.swing_speed, "deal_delay_ns": deal_delay,
            }));
        }
        {
            // seal_of_fury.go: the per-hit Holy damage and its absorb shield, and each rank's
            // judgement.
            let mut ranks = Vec::new();
            data.seal_of_fury.each(|_, rank| {
                let judge_rank = data
                    .seal_of_fury_triggered
                    .by_id(rank.effect_n(3).base_value() as i32);
                let proc_rank = rank.refs()[0];
                ranks.push(json!({
                    "seal_spell_id": rank.id,
                    "aura": Self::seal_aura_label("Seal of Fury", &label, rank),
                    "judgement_spell_id": judge_rank.id, "proc_spell_id": proc_rank.id,
                    "proc_damage": proc_rank.damage_effect().average(CHARACTER_LEVEL),
                    "shield_aura": format!("Seal of Fury Shield{label} Rank {}", rank.rank_number()),
                    "shield_share": rank.effect_n(2).percent(),
                }));
            });
            let mut fury = json!({"kind": "seal_of_fury", "ranks": ranks,
                "can_block": sim.unit(unit).pseudo_stats.can_block,
                "deal_delay_ns": deal_delay});
            // talents_protection.go applyImprovedSealOfFury: mana when a shield is spent, more
            // per level the target is above the paladin.
            if talents.bool("improved_seal_of_fury") {
                let rank = data.improved_seal_of_fury.highest();
                fury["improved"] = json!({
                    "mana": rank.effect_n(1).average(CHARACTER_LEVEL),
                    "per_level": rank.effect_n(2).percent(),
                    "max_levels": rank.effect_n(3).average(CHARACTER_LEVEL),
                    "levels": f64::from(sim.unit(target).level - sim.unit(unit).level),
                    "metrics_action_id": action_id(Some(&spell_action(rank.id))),
                    // 1314104 is No Threat since client 70374 (beta notes 8 October); the spell is
                    // not in the store, so Go sets the flag.
                    "no_threat": true,
                });
            }
            effects.push(fury);
        }
        {
            // seal_of_the_crusader.go
            let mut ranks = Vec::new();
            data.seal_of_the_crusader.each(|_, rank| {
                let judge_rank = data
                    .seal_of_the_crusader_triggered
                    .by_id(rank.effect_n(3).base_value() as i32);
                let speed = 1.0 + rank.effect(dbcenums::A_MOD_ATTACKSPEED, 0).percent();
                ranks.push(json!({
                    "seal_spell_id": rank.id,
                    "aura": Self::seal_aura_label("Seal of the Crusader", &label, rank),
                    "judgement_spell_id": judge_rank.id,
                    "judgement_aura": format!("Judgement of the Crusader Rank {}", rank.rank_number()),
                    "melee_speed": speed, "auto_damage_percent": 1.0 / speed - 1.0,
                }));
            });
            // The seal's spell mod: every spell with the main hand auto's proc mask that takes
            // mods.
            let autos = spell_positions(sim, unit, |spell| {
                let spell = sim.spell(spell);
                !spell.flags.matches(SpellFlag::NO_SPELL_MODS)
                    && spell.proc_mask.matches(ProcMask::MELEE_MH_AUTO)
            });
            effects.push(
                json!({"kind": "seal_of_the_crusader", "ranks": ranks, "auto_spells": autos}),
            );
            // buffs/paladin.go JudgementOfTheCrusaderAura: every rank and the raid's debuff
            // share one single aura category, each bidding the Holy damage it adds.
            if let Some(category) =
                exclusive_category_effect(sim, target, "target", "Judgement of the Crusader", notes)
            {
                effects.push(category);
            }
        }
        // seal_of_light.go, under Forever's rules where the rotation reaches a rank: the seal's
        // self heal on its 15 PPM manager, and each rank's Judgement of Light on the target.
        let mut light_reached = false;
        data.seal_of_light.each(|_, rank| {
            light_reached |= rotation_spells.contains(&i64::from(rank.id));
        });
        if sim.rules == crate::prepare::sim::Rules::Forever && light_reached {
            self.export_seal_of_light(env, &mut effects, notes);
        }
        // seal_of_wisdom.go and seal_of_justice.go, under Forever's rules where the rotation
        // reaches a rank.
        let mut wisdom_reached = false;
        data.seal_of_wisdom.each(|_, rank| {
            wisdom_reached |= rotation_spells.contains(&i64::from(rank.id));
        });
        if sim.rules == crate::prepare::sim::Rules::Forever && wisdom_reached {
            self.export_seal_of_wisdom(env, &mut effects);
        }
        let justice = data.seal_of_justice.highest();
        if sim.rules == crate::prepare::sim::Rules::Forever
            && rotation_spells.contains(&i64::from(justice.id))
        {
            let judgement = data
                .seal_of_justice_triggered
                .by_id(justice.effect_n(2).base_value() as i32);
            effects.push(json!({
                "kind": "seal_of_justice", "seal_spell_id": justice.id,
                "aura": Self::seal_aura_label("Seal of Justice", &label, justice),
                "judgement_spell_id": judgement.id, "judgement_aura": "Judgement of Justice",
            }));
        }
        // holy_strike.go: the weapon percent of the normalized swing plus the flat roll.
        let mut holy_strikes = Vec::new();
        data.holy_strike.each(|_, rank| {
            holy_strikes
                .push(json!({"spell_id": rank.id, "weapon_percent": rank.effect_n(2).percent()}));
        });
        effects.push(json!({"kind": "holy_strike", "ranks": holy_strikes}));
        // hammer_of_the_righteous.go, under Forever's rules (Go's exporter leaves the cast
        // unknown): the weapon DPS multiple is the row's effect 3, the targets its chain.
        if sim.rules == crate::prepare::sim::Rules::Forever {
            let rank = data.hammer_of_the_righteous.highest();
            if sim.get_spell(unit, &ActionId::spell(rank.id)).is_some() {
                effects.push(
                    json!({"kind": "hammer_of_the_righteous", "spell_id": rank.id,
                    "weapon_dps": rank.effect_n(3).base_points,
                    "max_targets": rank.effect_n(1).chain_targets}),
                );
            }
        }
        // hammer_of_wrath.go: the damage rolls are on the spells.
        effects.push(json!({"kind": "hammer_of_wrath"}));
        // exorcism.go and holy_wrath.go: the rolls are on the spells; both hit only an Undead
        // or Demon target, and Holy Wrath's cast pauses the swing.
        effects.push(json!({"kind": "exorcism"}));
        effects.push(json!({"kind": "holy_wrath"}));
        // consecration.go: the tick everyone takes, and the bonus the first targets take with
        // its own coefficient.
        let mut consecrations = Vec::new();
        data.consecration.each(|n, rank| {
            let tick_spell = data.consecration_triggered.rank(n);
            let tick = tick_spell.effect_n(1);
            let bonus = tick_spell.effect_n(2);
            let dummy = rank.effect(dbcenums::A_PERIODIC_DUMMY, 0);
            consecrations.push(json!({
                "spell_id": rank.id, "tick": tick.average(CHARACTER_LEVEL),
                "bonus": bonus.average(CHARACTER_LEVEL),
                // consecration.go consecrationTickCoefficient: the 16 October 2026 beta notes cut
                // it to 0.042; the rows still carry 0.095.
                "bonus_coefficient": 0.042,
                "bonus_targets": dummy.average(CHARACTER_LEVEL) as i32,
            }));
        });
        let mut consecration = json!({"kind": "consecration", "ranks": consecrations});
        let consecrated_ground = talents.i32("consecrated_ground");
        if consecrated_ground > 0 {
            // talents_holy.go applyConsecratedGround
            consecration["consecrated_ground"] = json!({
                "aura": format!("Consecrated Ground{label}"),
                "multiplier": data.consecrated_ground.multiplier_at(consecrated_ground),
            });
        }
        effects.push(consecration);
        if talents.bool("holy_shock") {
            // holy_shock.go: the hand-written rank table.
            let ranks: Vec<Value> = HOLY_SHOCK_RANKS
                .iter()
                .map(|rank| {
                    json!({"spell_id": rank.spell_id, "min": rank.damage[0], "max": rank.damage[1]})
                })
                .collect();
            effects.push(json!({"kind": "holy_shock", "ranks": ranks}));
        }
        let vigil_reached = {
            let mut reached = false;
            data.lights_vigil.each(|_, rank| {
                reached |= rotation_spells.contains(&i64::from(rank.id));
            });
            reached
        };
        if sim.rules == crate::prepare::sim::Rules::Forever {
            // Push 113554 makes the vigil friendly only: the next Holy Shock heal on the ally
            // heals its party, and the enemy strike and refund are gone. The Holy Paladin
            // healer sim models it; any other build's rotation that reaches it is refused.
            if talents.bool("lights_vigil") && self.healer {
                let mut ranks = Vec::new();
                data.lights_vigil.each(|_, rank| {
                    let (_, _, heal, _) = LIGHTS_VIGIL_TRIGGERED
                        .iter()
                        .copied()
                        .find(|(n, _, _, _)| *n == rank.rank_number())
                        .unwrap_or((0, 0, 0, 0));
                    let heal_rank = data.lights_vigil_triggered.by_id(heal);
                    let roll = heal_rank.heal_effect();
                    ranks.push(json!({
                        "spell_id": rank.id,
                        "aura": format!("Light's Vigil{label} Rank {}", rank.rank_number()),
                        "heal_spell_id": heal_rank.id,
                        "average": roll.average(CHARACTER_LEVEL), "variance": roll.variance,
                        "players": VIGIL_PARTY,
                    }));
                });
                effects.push(json!({"kind": "lights_vigil_ally", "ranks": ranks}));
            } else if talents.bool("lights_vigil") && vigil_reached {
                notes.push("Light's Vigil on an ally (push 113554) is unsupported".to_string());
            }
        } else if talents.bool("lights_vigil") {
            // lights_vigil.go: the vigil, its strike and its refund per rank.
            let mut ranks = Vec::new();
            data.lights_vigil.each(|_, rank| {
                let strike = LIGHTS_VIGIL_TRIGGERED
                    .iter()
                    .find(|(n, _, _, _)| *n == rank.rank_number())
                    .map_or(0, |(_, _, _, strike)| *strike);
                ranks.push(json!({
                    "spell_id": rank.id, "strike_spell_id": strike,
                    "aura": format!("Light's Vigil{label} Rank {}", rank.rank_number()),
                    "refund": rank.effect_n(2).percent(),
                    "metrics_action_id": action_id(Some(&ActionId { spell_id: rank.id, tag: 1, ..ActionId::default() })),
                }));
            });
            effects.push(json!({"kind": "lights_vigil", "ranks": ranks}));
        }
        if talents.bool("divine_favor") {
            // divine_favor.go
            let rank = data.divine_favor.highest();
            effects.push(json!({
                "kind": "divine_favor", "spell_id": rank.id, "aura": format!("Divine Favor{label}"),
                "crit": rank
                    .effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_CRITICAL_CHANCE)
                    .average(CHARACTER_LEVEL),
                "spells": ["holy_light", "flash_of_light", "holy_shock_heal", "holy_shock"],
            }));
        }
        effects.extend(self.oath_of_light_effect(&label));
        let vengeance = talents.i32("vengeance");
        if vengeance > 0 {
            // talents_retribution.go applyVengeance: the damage mod's School and ProcMask, as
            // core shouldApply matches them.
            let spells = spell_positions(sim, unit, |spell| {
                let spell = sim.spell(spell);
                !(spell.flags.matches(SpellFlag::NO_SPELL_MODS)
                    || spell.spell_school & (school::HOLY | school::PHYSICAL) == 0
                    || spell.proc_mask.0 & !ProcMask::SPELL_HEALING.0 == 0)
            });
            effects.push(json!({
                "kind": "vengeance", "trigger_aura": format!("Vengeance - Trigger{label}"),
                "aura": format!("Vengeance{label}"),
                "per_stack": data.vengeance.fraction_at(vengeance), "spells": spells,
            }));
        }
        if talents.i32("vindication") > 0 {
            // talents_retribution.go applyVindication: the chance is a Go literal. Push 113554
            // takes the paladin's aura away under Forever's rules.
            let mut vindication = json!({
                "kind": "vindication", "trigger_aura": format!("Vindication - Trigger{label}"),
                "proc_chance": 1.0, "target_aura": format!("Vindication{label}"),
            });
            if sim.rules == crate::prepare::sim::Rules::GoReference {
                vindication["aura"] = json!(format!("Vindication{label}"));
            }
            effects.push(vindication);
        }
        let sanctified = talents.i32("sanctified_judgement");
        if sanctified > 0 {
            // talents_retribution.go applySanctifiedJudgement: the refund is the row's share times
            // sanctifiedJudgementLogScale, an unexported Go constant of 10/9 fitted to beta logs.
            effects.push(json!({
                "kind": "sanctified_judgement", "trigger_aura": format!("Sanctified Judgement{label}"),
                "proc_chance": data.sanctified_judgement.effect_at(1).fraction_at(sanctified),
                "refund": data.sanctified_judgement.effect_at(2).fraction_at(sanctified) * SANCTIFIED_JUDGEMENT_LOG_SCALE,
                "metrics_action_id": action_id(Some(&spell_action(data.sanctified_judgement.highest().id))),
            }));
        }
        if talents.bool("sacred_arbiter") {
            // talents_retribution.go applySacredArbiter
            let tagged: Vec<String> = sim
                .auras_with_tag(target, JUDGEMENT_AURA_TAG)
                .into_iter()
                .map(|aura| sim.aura(aura).label.clone())
                .collect();
            effects.push(json!({
                "kind": "sacred_arbiter", "trigger_aura": format!("Sacred Arbiter{label}"),
                "judgement_auras": tagged,
            }));
        }
        effects.extend(self.tank_effects(env, &label));
        if talents.bool("twist_of_light") {
            // talents_retribution.go applyTwistOfLight, in its fixed order
            let echoes: Vec<Value> = [
                (1311703, "seal_of_command"),
                (1311701, "seal_of_fury"),
                (1311704, "seal_of_righteousness"),
                (1311705, "seal_of_justice"),
            ]
            .iter()
            .map(|(id, seal)| json!({"aura": format!("Echo ({id}){label}"), "seal": seal}))
            .collect();
            effects.push(json!({
                "kind": "twist_of_light", "trigger_aura": format!("Twist of Light{label}"),
                "echoes": echoes,
            }));
        }
        if self.new_talent("unsealed_wrath") > 0 {
            // Push 113554's Unsealed Wrath, under Forever's rules: the Echoes in upstream #772's
            // order.
            let echoes: Vec<Value> = [
                (1311703, "seal_of_command"),
                (1311701, "seal_of_fury"),
                (1311704, "seal_of_righteousness"),
                (1311705, "seal_of_justice"),
            ]
            .iter()
            .map(|(id, seal)| json!({"aura": format!("Echo ({id}){label}"), "seal": seal}))
            .collect();
            effects.push(json!({
                "kind": "unsealed_wrath", "trigger_aura": format!("Unsealed Wrath{label}"),
                "echoes": echoes,
            }));
        }
        // talents_holy.go applyInfusionOfLight and item_librams.go Libram of Holy Alacrity: an
        // aura that speeds only Holy Light, on a Holy Shock crit or any Holy Shock cast.
        let holy_lights = spells_matching(sim, unit, masks::HOLY_LIGHT);
        let infusion = talents.i32("infusion_of_light");
        if infusion > 0 {
            effects.push(json!({
                "kind": "holy_light_haste", "trigger_aura": format!("Infusion of Light - Trigger{label}"),
                "aura": format!("Infusion of Light{label}"), "on_crit": true,
                "holy_light_spells": holy_lights,
                "cast_time_ns": (data.infusion_of_light.value_at(infusion) as i64) * MILLISECOND,
            }));
        }
        if sim
            .get_aura(unit, &format!("Libram of Holy Alacrity{label}"))
            .is_some()
        {
            effects.push(json!({
                "kind": "holy_light_haste", "trigger_aura": format!("Libram of Holy Alacrity{label}"),
                "aura": format!("Holy Alacrity{label}"), "on_crit": false,
                "holy_light_spells": holy_lights,
                "cast_time_ns": -200 * MILLISECOND,
            }));
        }
        effects.push(self.heal_effect(env, notes));
        // lay_on_hands.go: each rank the rotation names drains the paladin's mana, restores
        // mana to a healed unit with a mana bar and heals it for the paladin's maximum health.
        let mut lay_on_hands = Vec::new();
        data.lay_on_hands.each(|_, rank| {
            if rotation_spells.contains(&i64::from(rank.id)) {
                lay_on_hands.push(json!({"spell_id": rank.id,
                    "mana": rank.energize_effect().average(CHARACTER_LEVEL)}));
            }
        });
        if !lay_on_hands.is_empty() {
            effects.push(json!({"kind": "lay_on_hands", "ranks": lay_on_hands}));
            if sim.has_mana_bar(target) {
                notes.push("Lay on Hands restoring the target's mana is unsupported".to_string());
            }
        }
        // auras.go registerAuraSpell: an aura rank's cast turns its aura on, for every paladin
        // aura Go registers. Forever's rules describe it; Go's exporter leaves the cast unknown.
        if sim.rules == crate::prepare::sim::Rules::Forever {
            let mut ranks = Vec::new();
            for ladder in [
                &data.retribution_aura,
                &data.devotion_aura,
                &data.concentration_aura,
                &data.fire_resistance_aura,
                &data.frost_resistance_aura,
                &data.shadow_resistance_aura,
            ] {
                ladder.each(|_, rank| {
                    if !rotation_spells.contains(&i64::from(rank.id)) {
                        return;
                    }
                    let aura = sim
                        .get_spell(unit, &ActionId::spell(rank.id))
                        .and_then(|spell| sim.spell(spell).related_self_buff);
                    if let Some(aura) = aura {
                        ranks.push(json!({"spell_id": rank.id, "aura": sim.aura(aura).label}));
                    }
                });
            }
            if !ranks.is_empty() {
                effects.push(json!({"kind": "paladin_aura_cast", "ranks": ranks}));
            }
        }
        let eye = talents.i32("eye_for_an_eye");
        if eye > 0 {
            // talents_retribution.go applyEyeForAnEye
            effects.push(json!({
                "kind": "eye_for_an_eye", "trigger_aura": format!("Eye for an Eye{label}"),
                "spell_id": data.eye_for_an_eye.highest().id, "share": data.eye_for_an_eye.fraction_at(eye),
            }));
        }
        let pursuit = talents.i32("pursuit_of_justice");
        if pursuit > 0 {
            // talents_retribution.go applyPursuitOfJustice
            let bonus = data
                .pursuit_of_justice
                .effect(dbcenums::A_MOD_INCREASE_SPEED, 0)
                .fraction_at(pursuit);
            if let Some(aura) = sim.get_aura(unit, "Pursuit of Justice") {
                // movement.go NewPassiveMovementSpeedEffect: the bonus holds while the aura's
                // effect holds the category, which the player_movement effect describes with
                // the aura.
                effects.push(json!({
                    "kind": "pursuit_of_justice", "aura": sim.aura(aura).label, "bonus": bonus,
                    "initial_multiplier":
                        sim.unit(unit).pseudo_stats.movement_speed_multiplier / (1.0 + bonus),
                }));
            }
        }
        effects
    }

    /// Go `registerSealOfLight`'s seal, self heal and judgement for each rank.
    fn export_seal_of_light(
        &self,
        env: &Environment,
        effects: &mut Vec<Value>,
        notes: &mut Vec<String>,
    ) {
        let sim = &env.sim;
        let unit = env.player;
        let label = sim.unit(unit).label.clone();
        let data = spell_data();
        let pseudo = &sim.unit(unit).pseudo_stats;
        let mut ranks = Vec::new();
        let mut chances = None;
        let mut judgements = Vec::new();
        data.seal_of_light.each(|_, rank| {
            let aura = Self::seal_aura_label("Seal of Light", &label, rank);
            let judgement_id = rank.effect_n(2).base_value() as i32;
            let judgement_heal = data
                .seal_of_light_triggered
                .by_id(super::seals::paired(
                    &super::seals::JUDGEMENT_OF_LIGHT_HEAL_IDS,
                    rank.rank_number(),
                ))
                .heal_effect()
                .average(CHARACTER_LEVEL);
            let judgement_aura = format!("Judgement of Light Rank {}", rank.rank_number());
            let proc_rank = rank.refs()[0];
            if chances.is_none() {
                let dpm = sim
                    .get_aura(unit, &aura)
                    .and_then(|aura| sim.aura(aura).dpm.clone());
                chances = dpm.map(|dpm| {
                    crate::prepare::export_items::dpm_chances(env, &dpm, |spell| {
                        spell.proc_mask.matches(ProcMask::MELEE)
                            && !spell.flags.matches(SpellFlag::PROC)
                    })
                });
            }
            ranks.push(json!({
                "seal_spell_id": rank.id, "aura": aura, "proc_spell_id": proc_rank.id,
                "heal": proc_rank.heal_effect().average(CHARACTER_LEVEL),
                "judgement_spell_id": judgement_id, "judgement_aura": judgement_aura,
            }));
            // buffs/paladin.go AttachJudgementOfLightHeal, under health metrics of the target.
            judgements.push(json!({
                "kind": "judgement_of_light", "aura": judgement_aura,
                "proc_chance": crate::prepare::buffs::paladin::JUDGEMENT_PROC_CHANCE,
                "rng_label": "Judgement of Light - Heal", "heal": judgement_heal,
                "metrics_action_id": action_id(Some(&ActionId::spell(judgement_id))),
                "delay_ns": crate::prepare::common_effects::SPELL_BATCH_WINDOW,
            }));
        });
        let Some(chances) = chances else {
            notes.push("Seal of Light has no proc manager".to_string());
            return;
        };
        effects.push(json!({
            "kind": "seal_of_light", "ranks": ranks, "chances": chances,
            "healing_dealt_multiplier": pseudo.healing_dealt_multiplier,
            "healing_taken_multiplier": pseudo.healing_taken_multiplier,
            "table_healing_dealt_multiplier": env.attack_table(unit, unit).healing_dealt_multiplier,
        }));
        effects.extend(judgements);
    }

    /// Go `registerSealOfWisdom`'s seal, mana restore and judgement for each rank.
    fn export_seal_of_wisdom(&self, env: &Environment, effects: &mut Vec<Value>) {
        let sim = &env.sim;
        let unit = env.player;
        let label = sim.unit(unit).label.clone();
        let data = spell_data();
        let mut ranks = Vec::new();
        let mut judgements = Vec::new();
        data.seal_of_wisdom.each(|_, rank| {
            let judgement_id = rank.effect_n(2).base_value() as i32;
            let judgement_mana = data
                .seal_of_wisdom_triggered
                .by_id(super::seals::paired(
                    &super::seals::JUDGEMENT_OF_WISDOM_MANA_IDS,
                    rank.rank_number(),
                ))
                .energize_effect()
                .average(CHARACTER_LEVEL);
            let judgement_aura = format!("Judgement of Wisdom Rank {}", rank.rank_number());
            let proc_rank = rank.refs()[0];
            ranks.push(json!({
                "seal_spell_id": rank.id,
                "aura": Self::seal_aura_label("Seal of Wisdom", &label, rank),
                "proc_spell_id": proc_rank.id,
                "mana": proc_rank.energize_effect().average(CHARACTER_LEVEL),
                "judgement_spell_id": judgement_id, "judgement_aura": judgement_aura,
            }));
            // buffs/paladin.go AttachJudgementOfWisdomMana.
            judgements.push(json!({
                "kind": "judgement_of_wisdom", "aura": judgement_aura,
                "proc_chance": crate::prepare::buffs::paladin::JUDGEMENT_PROC_CHANCE,
                "proc_mask": ProcMask::DIRECT.names(), "mana": judgement_mana,
                "metrics_action_id": action_id(Some(&ActionId::spell(judgement_id))),
                "delay_ns": crate::prepare::common_effects::SPELL_BATCH_WINDOW,
            }));
        });
        effects.push(json!({"kind": "seal_of_wisdom", "ranks": ranks}));
        effects.extend(judgements);
    }

    /// Go `paladinStatAuras`: the paladin auras whose gain or loss changes the stats the
    /// runtime reads or the target's swings at a tanking paladin.
    pub(super) fn export_stat_auras(&self, sim: &Sim, unit: UnitId) -> Vec<String> {
        let talents = &self.talents;
        let label = sim.unit(unit).label.clone();
        let rotation_spells = rotation_spells(sim, unit);
        let data = spell_data();
        let mut labels = Vec::new();
        // Push 113554 takes the paladin's Vindication aura away under Forever's rules.
        if talents.i32("vindication") > 0 && sim.rules == crate::prepare::sim::Rules::GoReference {
            labels.push(format!("Vindication{label}"));
        }
        // seal_of_the_crusader.go: the attack power of each rank the rotation names.
        data.seal_of_the_crusader.each(|_, rank| {
            if rotation_spells.contains(&i64::from(rank.id)) {
                labels.push(Self::seal_aura_label("Seal of the Crusader", &label, rank));
            }
        });
        if sim.get_aura(unit, "Reduced avoidance").is_none() {
            return labels;
        }
        // auras.go registerAuraSpell, under Forever's rules: a Devotion Aura rank the rotation
        // casts adds its armor, which the target's swings read.
        if sim.rules == crate::prepare::sim::Rules::Forever {
            data.devotion_aura.each(|_, rank| {
                if !rotation_spells.contains(&i64::from(rank.id)) {
                    return;
                }
                let aura = sim
                    .get_spell(unit, &ActionId::spell(rank.id))
                    .and_then(|spell| sim.spell(spell).related_self_buff);
                if let Some(aura) = aura {
                    labels.push(sim.aura(aura).label.clone());
                }
            });
        }
        if talents.i32("redoubt") > 0 {
            labels.push(format!("Redoubt{label}"));
        }
        if talents.bool("holy_shield") {
            data.holy_shield.each(|_, rank| {
                if rotation_spells.contains(&i64::from(rank.id)) {
                    labels.push(format!("Holy Shield{label} Rank {}", rank.rank_number()));
                }
            });
        }
        labels
    }
}
