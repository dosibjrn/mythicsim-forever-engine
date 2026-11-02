//! The exporter's Rogue part, tools/oracle-v2/rogue.go: the effects whose parameters Go keeps in
//! closures. Each formula mirrors the cited file of sim/rogue at the pinned revision.

use serde_json::{json, Value};

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::dbcenums;
use crate::prepare::env::Environment;
use crate::prepare::export::action_id;
use crate::prepare::sim::Rules;

use super::poisons::{poison_proc_mask, IMBUES};
use super::spell_data::spell_data;
use super::util::{has_dagger, Hand};
use super::{Rogue, CLASS_SPELLS, SLICE_AND_DICE_DURATIONS};

/// The stable names of the class spells a mask names, in the exporter's order:
/// `rogueMaskNames`.
pub(super) fn mask_names(mask: i64) -> Vec<&'static str> {
    CLASS_SPELLS
        .iter()
        .filter(|entry| mask & entry.mask != 0)
        .map(|entry| entry.name)
        .collect()
}

/// An action naming a spell, as the exporter writes it.
pub(super) fn action(id: i32) -> Value {
    action_id(Some(&ActionId {
        spell_id: id,
        ..ActionId::default()
    }))
}

/// `Rogue`'s effects in the exporter's order: `rogueEffects`, then `rogueSpecEffects`, then the
/// notes of `rogueUnrepresented`.
pub(super) fn class_effects(
    rogue: &Rogue,
    env: &Environment,
    unrepresented: &mut Vec<String>,
) -> Vec<Value> {
    let mut effects = rogue_effects(rogue, env);
    effects.extend(rogue.spec_effects(env, unrepresented));
    effects.extend(super::items::nightslayer_8_effect(env));
    // `AdditiveEnergyRegenBonus` is only ever raised by `ApplyAdditiveEnergyRegenBonus`, which
    // nothing calls.
    effects
}

/// tools/oracle-v2/rogue.go `rogueEffects`.
fn rogue_effects(rogue: &Rogue, env: &Environment) -> Vec<Value> {
    let sim = &env.sim;
    let data = spell_data();
    let state = &rogue.state;
    let mut effects = Vec::new();

    // sinister_strike.go: the highest rank's base plus normalized main hand damage.
    if rogue.spells.sinister_strike.is_some() {
        let row = data.sinister_strike.highest();
        effects.push(json!({
            "kind": "sinister_strike", "spell_id": row.id,
            "base_damage": row.damage_effect().average(CHARACTER_LEVEL),
        }));
    }
    // backstab.go: the base, the dagger and behind conditions, and Puncturing Wounds' combo point.
    if rogue.spells.backstab.is_some() {
        let row = data.backstab.highest_for(env.rules);
        effects.push(json!({
            "kind": "backstab", "spell_id": row.id,
            "base_damage": row.damage_effect().average(CHARACTER_LEVEL),
            "main_hand_dagger": has_dagger(sim, env.player, Hand::Main),
            "extra_combo_point_chance": data
                .puncturing_wounds
                .effect_at(2)
                .value_at(rogue.talents.i32("puncturing_wounds"))
                / 100.0,
            "extra_combo_point_action": action(data.puncturing_wounds_triggered.highest().id),
        }));
    }
    // eviscerate.go: the rolled base, the bonus a combo point, and 3% of attack power a point, a
    // Go literal.
    if rogue.spells.eviscerate.is_some() {
        let row = data.eviscerate.highest();
        let damage = row.damage_effect();
        effects.push(json!({
            "kind": "eviscerate", "spell_id": row.id,
            "damage_average": damage.average(CHARACTER_LEVEL),
            "damage_variance": damage.variance,
            "combo_point_damage": f64::from(damage.points_per_resource) + state.deathmantle_bonus.get(),
            "attack_power_per_combo_point": 0.03,
        }));
    }
    // slice_and_dice.go: the duration at each combo point count and the attack speed bonus.
    if let Some(spell) = rogue.spells.slice_and_dice {
        let multiplier = data
            .improved_slice_and_dice
            .multiplier_at(rogue.talents.i32("improved_slice_and_dice"));
        let durations: Vec<i64> = SLICE_AND_DICE_DURATIONS
            .iter()
            .map(|duration| {
                ((*duration + state.slice_and_dice_bonus_duration.get()) as f64 * multiplier) as i64
            })
            .collect();
        let aura = rogue
            .auras
            .slice_and_dice
            .map(|aura| sim.aura(aura).label.clone());
        effects.push(json!({
            "kind": "slice_and_dice", "spell_id": sim.spell(spell).action_id.spell_id,
            "aura": aura.unwrap_or_default(), "durations_ns": durations,
            "melee_speed_multiplier": 1.0 + state.slice_and_dice_bonus_flat.get(),
        }));
    }
    // talents_combat.go registerBladeFlurry: the attack speed it attaches, and the spell of its
    // extra hit on the next target, a Go literal, which needs a second target.
    if let (Some(_), Some(aura)) = (rogue.spells.blade_flurry, rogue.auras.blade_flurry) {
        let row = data.blade_flurry.highest();
        effects.push(json!({
            "kind": "blade_flurry", "spell_id": row.id, "aura": sim.aura(aura).label,
            "hit_spell_id": 22482,
            "attack_speed_multiplier": 1.0
                + row.effect(dbcenums::A_MOD_MELEE_HASTE_3, 0).average(CHARACTER_LEVEL) / 100.0,
        }));
    }
    // talents_combat.go registerAdrenalineRush: the regeneration multiplier and the energy at or
    // below which the major cooldown fires, a Go literal.
    if let (Some(_), Some(aura)) = (rogue.spells.adrenaline_rush, rogue.auras.adrenaline_rush) {
        let row = data.adrenaline_rush.highest();
        effects.push(json!({
            "kind": "adrenaline_rush", "spell_id": row.id, "aura": sim.aura(aura).label,
            "regen_multiplier": 1.0
                + row.effect(dbcenums::A_MOD_POWER_REGEN_PERCENT, 3).average(CHARACTER_LEVEL) / 100.0,
            "energy_threshold": 45.0,
        }));
    }
    // rogue.go ApplyFinisher: Relentless Strikes' 20% a combo point for 25 energy, Go literals,
    // and Ruthlessness's chance.
    effects.push(json!({
        "kind": "rogue_finisher", "relentless_strikes": rogue.talents.bool("relentless_strikes"),
        "relentless_strikes_chance_per_point": 0.2, "relentless_strikes_energy": 25.0,
        "relentless_strikes_action": action(14179),
        "ruthlessness_chance": rogue.ruthlessness_chance,
        "ruthlessness_action": action(14161),
    }));
    // poisons.go: each imbue's weapon proc rolls its chance, raised by Improved Poisons, inside
    // the handler. The attack power shares are server side, from the devs (community #781).
    let bonus = poison_bonus(rogue);
    let hands = |imbue: i32| poison_proc_mask(sim, env.player, imbue).names();
    if let Some(spell) = rogue.spells.instant_poison {
        effects.push(json!({
            "kind": "instant_poison", "trigger_aura": "Instant Poison",
            "spell_id": sim.spell(spell).action_id.spell_id,
            "proc_mask": hands(IMBUES.instant), "proc_chance": 0.2 + bonus,
            "min_damage": 76.0, "max_damage": 100.0, "attack_power_share": 0.005,
        }));
    }
    if let Some(spell) = rogue.spells.deadly_poison {
        let id = &sim.spell(spell).action_id;
        // Deadly Poison V (dot 25349, 23 a tick) is taught by an Ahn'Qiraj 20 book, so under
        // Forever's rules the rogue brews rank IV (dot 11354, 18 a tick); see book_ranks.
        let tick_damage = match env.rules {
            Rules::Forever => 18.0,
            Rules::GoReference => 23.0,
        };
        effects.push(json!({
            "kind": "deadly_poison", "trigger_aura": "Deadly Poison", "spell_id": id.spell_id,
            "tag": id.tag, "proc_mask": hands(IMBUES.deadly), "proc_chance": 0.3 + bonus,
            "tick_damage": tick_damage, "attack_power_share": 0.0045,
        }));
    }
    effects
}

/// The poison proc chance the exporter writes for a poison: `base + bonus`.
pub(super) fn poison_bonus(rogue: &Rogue) -> f64 {
    spell_data()
        .improved_poisons
        .effect(
            dbcenums::A_ADD_FLAT_MODIFIER,
            dbcenums::SPELLMOD_CHANCE_OF_SUCCESS,
        )
        .fraction_at(rogue.talents.i32("improved_poisons"))
}
