//! tools/oracle-v2/hunter.go: the effects whose parameters Go keeps in closures, in the
//! exporter's order, and the behavior its effects cannot describe.

use serde_json::{json, Value};

use crate::contracts::prepared_v2::ActionId;
use crate::contracts::request::{Message, Value as RequestValue};
use crate::prepare::aura_helpers::{CallbackMask, ProcTrigger};
use crate::prepare::character::constants::{CHARACTER_LEVEL, MAX_MELEE_RANGE};
use crate::prepare::dbcenums;
use crate::prepare::export::action_id;
use crate::prepare::parse_effects::{dry_run, ParseOptions};
use crate::prepare::sim::{Sim, SpellId, UnitId, UnitType, MILLISECOND, NEVER_EXPIRES};
use crate::prepare::spell::{school, DefenseType, ProcMask};
use crate::prepare::spelldata::{spell::TickOutcomeKind, Ladder, Spell as Row};

use super::spell_data::spell_data;
use super::spells::{
    ARCANE_SHOT_RAP_COEFFICIENT, EXPLOSIVE_TRAP_RANGE, SERPENT_STING_RAP_PER_TICK,
    SUMMON_HAWK_RAP_SHARE, SUMMON_HAWK_SWING_SHARE, TRAP_ATTACK_POWER_SHARE, VOLLEY_TICK_DAMAGE,
};
use super::{masks, Hunter};

/// Go `core.SpellBatchWindow`.
const SPELL_BATCH_WINDOW: i64 = 10 * MILLISECOND;

/// Go `hunterTickOutcome`: a crit only where the row states Periodic Can Crit, on the crit of the
/// row's defense type.
fn tick_outcome(row: &Row) -> &'static str {
    match row.tick_outcome_kind() {
        TickOutcomeKind::MagicCrit => "magic_crit",
        TickOutcomeKind::PhysicalCrit => "physical_crit",
        TickOutcomeKind::MagicHit => "magic_hit",
        TickOutcomeKind::Plain => "plain",
    }
}

/// Go `hunterSpellPosition`.
fn spell_position(sim: &Sim, unit: UnitId, spell: SpellId) -> usize {
    sim.unit(unit)
        .spellbook
        .iter()
        .position(|candidate| *candidate == spell)
        .expect("a spell in the spellbook")
}

fn spell_id_of(sim: &Sim, spell: SpellId) -> i32 {
    sim.spell(spell).action_id.spell_id
}

fn active_target_count(sim: &Sim) -> usize {
    sim.units
        .iter()
        .filter(|unit| unit.unit_type == UnitType::Enemy)
        .count()
}

/// Go `hunterPetRowStrike`: the effect's roll as a hit's range, no draw without a variance.
fn pet_row_strike(id: i32, row: &Row, outcome: &str) -> Value {
    let effect = row.damage_effect();
    let average = effect.average(CHARACTER_LEVEL);
    json!({
        "kind": "hunter_pet_strike", "spell_id": id, "outcome": outcome,
        "draws": effect.variance != 0.0,
        "min_damage": average * (1.0 - effect.variance / 2.0),
        "max_damage": average * (1.0 + effect.variance / 2.0),
        "average": average, "variance": effect.variance,
    })
}

/// Go `hunterPetRow`: the row among the ladders' highest ranks with this spell ID.
fn pet_row(ladders: &[&Ladder], id: i32) -> Option<&'static Row> {
    ladders
        .iter()
        .map(|ladder| ladder.highest())
        .find(|row| row.id == id)
}

impl Hunter {
    /// Go `hunterEffects`.
    pub(super) fn export_effects(&self, sim: &Sim, unit: UnitId) -> Vec<Value> {
        let data = spell_data();
        let mut effects: Vec<Value> = Vec::new();

        // aimed_shot.go and sniper_shot.go: a normalized ranged weapon shot plus the rank's flat
        // bonus, rolled on the ranged hit table and dealt after travel.
        if let Some(spell) = self.aimed_shot {
            effects.push(json!({
                "kind": "aimed_shot", "spell_id": spell_id_of(sim, spell),
                "flat_bonus": data.aimed_shot.highest().damage_effect().average(CHARACTER_LEVEL),
            }));
        }
        // arcane_shot.go: the rank's flat damage plus a share of ranged attack power.
        if let Some(spell) = self.arcane_shot {
            effects.push(json!({
                "kind": "arcane_shot", "spell_id": spell_id_of(sim, spell),
                "base_damage": data.arcane_shot.highest().damage_effect().average(CHARACTER_LEVEL),
                "rap_coefficient": ARCANE_SHOT_RAP_COEFFICIENT,
            }));
        }
        if let Some(spell) = self.sniper_shot {
            effects.push(json!({
                "kind": "sniper_shot", "spell_id": spell_id_of(sim, spell),
                "flat_bonus": data.sniper_shot.highest().damage_effect().average(CHARACTER_LEVEL),
            }));
        }
        // multi_shot.go: one normalized shot per target, at most three.
        if let Some(spell) = self.multi_shot {
            effects.push(json!({"kind": "multi_shot", "spell_id": spell_id_of(sim, spell)}));
        }
        // serpent_sting.go: a ranged hit roll, then after travel a dot whose ticks add a share
        // of ranged attack power.
        if let Some(spell) = self.serpent_sting {
            let rank = data.serpent_sting.highest_for(sim.rules);
            effects.push(json!({
                "kind": "serpent_sting", "spell_id": spell_id_of(sim, spell),
                "tick_base": rank.periodic_effect().average(CHARACTER_LEVEL),
                "attack_power_share": SERPENT_STING_RAP_PER_TICK,
                "tick_outcome": tick_outcome(rank),
            }));
        }
        // aspects.go: Aspect of the Hawk's ranged attack power is a stat aura; Deadly Aspects
        // procs Quick Shots on ranged autos.
        if let (Some(aura), Some(spell)) = (self.aspect_of_the_hawk_aura, self.aspect_of_the_hawk) {
            let mut effect = json!({
                "kind": "aspect_of_the_hawk", "spell_id": spell_id_of(sim, spell),
                "aura": sim.aura(aura).label,
            });
            let deadly_aspects = self.t("deadly_aspects");
            if deadly_aspects > 0 {
                let quick_shots = data.aspect_of_the_hawk_triggered.highest();
                let proc = sim.get_aura(unit, "Quick Shots").expect("Quick Shots");
                effect["proc_aura"] = json!(sim.aura(proc).label);
                effect["haste_multiplier"] = json!(
                    1.0 + quick_shots
                        .effect(dbcenums::A_MOD_RANGED_HASTE, 0)
                        .average(CHARACTER_LEVEL)
                        / 100.0
                );
                effect["proc_chance"] =
                    json!(data.deadly_aspects.effect_at(1).fraction_at(deadly_aspects));
            }
            effects.push(effect);
        }
        // rapid_fire.go: ranged and melee attack speed.
        if let (Some(aura), Some(spell)) = (self.rapid_fire_aura, self.rapid_fire) {
            let rank = data.rapid_fire.highest();
            effects.push(json!({
                "kind": "rapid_fire", "spell_id": spell_id_of(sim, spell),
                "aura": sim.aura(aura).label,
                "haste_multiplier": 1.0
                    + rank.effect(dbcenums::A_MOD_RANGED_HASTE, 0).average(CHARACTER_LEVEL) / 100.0,
            }));
        }
        // summon_hawk.go: a dive bomb on its rank's base plus a share of ranged attack power,
        // then a hawk dot in a free slot or the one with the least time left. The hawk swings
        // once on arrival and then every tick for a share of the rank's base.
        if self.summon_hawk.is_some() {
            let rank = data.summon_hawk.highest();
            let hawks: Vec<usize> = sim
                .unit(unit)
                .spellbook
                .iter()
                .enumerate()
                .filter(|(_, spell)| {
                    let action = &sim.spell(**spell).action_id;
                    action.spell_id == rank.id && action.tag > 0
                })
                .map(|(i, _)| i)
                .collect();
            // A wrong count is named by `export_notes`.
            effects.push(json!({
                "kind": "summon_hawk", "spell_id": rank.id,
                "base_damage": rank.damage_effect().average(CHARACTER_LEVEL),
                "attack_power_share": SUMMON_HAWK_RAP_SHARE, "swing_share": SUMMON_HAWK_SWING_SHARE,
                "always_hits": rank.always_hits(), "hawk_spells": hawks,
                "hawk_duration_ns": data.summon_hawk_triggered.by_id(1293248).duration(),
            }));
        }
        if self.pet.is_some() {
            effects.extend(self.pet_effects(sim, unit));
        }
        effects.extend(self.melee_effects(sim, unit));
        // talents_marksmanship.go registerRapidRecuperation: Serpent Sting's hit, a spell batch
        // window later and if it landed, grants casting regeneration.
        if let Some(trigger) = sim.get_aura(unit, "Rapid Recuperation Trigger") {
            let aura = sim
                .get_aura(unit, "Rapid Recuperation")
                .expect("Rapid Recuperation");
            effects.push(json!({
                "kind": "rapid_recuperation", "trigger_aura": sim.aura(trigger).label,
                "aura": sim.aura(aura).label,
                "regen": data.rapid_recuperation.effect_at(1).fraction_at(self.t("rapid_recuperation")),
            }));
        }
        effects.extend(self.set_mana_procs(sim, unit));
        // talents_survival.go Defensive State: the trigger hears only attacks the player dodges.
        if sim.get_aura(unit, "Defensive State - Trigger").is_some() {
            effects.push(json!({
                "kind": "inert_listener", "unit": "player", "aura": "Defensive State - Trigger",
                "reason": "acts only on attacks the player dodges, and nothing attacks the player",
            }));
        }
        effects
    }

    /// The effect of one pet damage ability (Go `hunterPetAbility`), or `None` when Rust has none.
    fn pet_ability(&self, sim: &Sim, pet: UnitId, spell: SpellId) -> Option<Value> {
        let data = spell_data();
        let s = sim.spell(spell);
        let id = s.action_id.spell_id;
        if s.action_id.tag != 0 {
            return None;
        }
        // newScorpidPoison: a melee special hit roll, then a dot whose stack Apply resets, so
        // each landed cast is one stack of the Go literal tick.
        if id == 24587 {
            return Some(json!({
                "kind": "hunter_pet_scorpid_poison", "spell_id": id, "tick_base": 5.0,
                "tick_can_crit": data.scorpid_poison_triggered.rank(4).periodic_can_crit(),
                "tick_magic": s.defense_type == DefenseType::Magic,
            }));
        }
        // pet_abilities.go: each literal ability's rolled range and hit table.
        let literal = match id {
            17261 => Some((81.0, 99.0, "melee_special")),
            3009 => Some((43.0, 59.0, "melee_special")),
            25012 => Some((86.0, 98.0, "magic")),
            _ => None,
        };
        if let Some((min, max, outcome)) = literal {
            return Some(json!({
                "kind": "hunter_pet_strike", "spell_id": id, "outcome": outcome, "draws": true,
                "min_damage": min, "max_damage": max,
            }));
        }
        let strike_rows = [
            &data.demoralizing_screech_triggered,
            &data.pinch_triggered,
            &data.dismember_triggered,
            &data.mine_triggered,
        ];
        if let Some(row) = pet_row(&strike_rows, id) {
            if s.spell_school != school::PHYSICAL {
                return None;
            }
            return Some(pet_row_strike(id, row, "melee_special"));
        }
        // newThunderstomp: a cleave of magic hits, so one hit on a single target. Forever's rules
        // describe the cleave on more: the row's targets at most, each a roll of its own.
        let thunderstomp = data.thunderstomp_triggered.highest();
        if thunderstomp.id == id {
            let mut strike = pet_row_strike(id, thunderstomp, "magic");
            let targets = active_target_count(sim);
            if targets > 1 {
                if sim.rules != crate::prepare::sim::Rules::Forever {
                    return None;
                }
                let cap = i32::from(thunderstomp.max_targets);
                strike["cleave_targets"] = json!(if cap > 0 { cap } else { targets as i32 });
            }
            return Some(strike);
        }
        // newSwipe: its cast condition needs three active targets, and it cleaves them, each a
        // roll of the row on the melee special table. Forever's rules describe the cleave; Go's
        // exporter refuses it.
        let swipe = data.swipe_triggered.highest();
        if swipe.id == id {
            let mut effect = json!({"kind": "hunter_pet_swipe", "spell_id": id, "min_targets": 3});
            if active_target_count(sim) >= 3 {
                if sim.rules != crate::prepare::sim::Rules::Forever {
                    return None;
                }
                let damage = swipe.damage_effect();
                effect["cleave"] = json!({
                    "max_targets": i32::from(damage.chain_targets),
                    "average": damage.average(CHARACTER_LEVEL), "variance": damage.variance,
                });
            }
            return Some(effect);
        }
        let bleed_rows = [
            &data.savage_rend_triggered,
            &data.tendon_rip_triggered,
            &data.web_triggered,
        ];
        let target = sim.unit(pet).current_target;
        let has_dot = target.is_some_and(|target| {
            let index = sim.unit(target).unit_index as usize;
            s.dots.get(index).copied().flatten().is_some()
        });
        if let Some(row) = pet_row(&bleed_rows, id) {
            if has_dot {
                let hit = if s.defense_type == DefenseType::Ranged {
                    "ranged"
                } else {
                    "melee_special"
                };
                // spelldata TickOutcomeHitRolled: a crit roll the row allows, else a plain tick.
                let mut tick = "plain";
                if row.periodic_can_crit() {
                    tick = "physical_crit";
                    if s.defense_type == DefenseType::Magic {
                        tick = "magic_crit";
                    }
                }
                return Some(json!({
                    "kind": "hunter_pet_bleed", "spell_id": id, "hit": hit, "tick_outcome": tick,
                    "tick_base": row.periodic_effect().average(CHARACTER_LEVEL),
                }));
            }
        }
        None
    }

    /// Go `hunterPetEffects`: pet.go `ExecuteCustomRotation` and pet_abilities.go, and the pet
    /// auras of Intimidation, Bestial Wrath and Frenzy.
    fn pet_effects(&self, sim: &Sim, unit: UnitId) -> Vec<Value> {
        let data = spell_data();
        let pet = self.pet.expect("a pet");
        let state = self.pet_state.as_ref().expect("a pet");
        let mut effects = Vec::new();
        let rotation = match self.options.enum_name("pet_type").as_str() {
            "Cat" => "cat",
            "Scorpid" => "scorpid",
            _ => "default",
        };
        for spell in sim.unit(pet).spellbook.clone() {
            if sim.spell(spell).class_spell_mask != masks::PET_DAMAGE {
                continue;
            }
            // Unsupported abilities are named by `export_notes`.
            if let Some(effect) = self.pet_ability(sim, pet, spell) {
                effects.push(effect);
            }
        }
        // newDustCloud: no damage mask; a landed melee special hit puts the target's armor down
        // while its aura holds, and the pet casts it again once it falls off.
        {
            let row = data.dust_cloud_triggered.highest();
            let cast = sim
                .unit(pet)
                .spellbook
                .iter()
                .any(|spell| sim.spell(*spell).action_id == ActionId::spell(row.id));
            if cast {
                let parsed = dry_run(
                    row,
                    ParseOptions {
                        ignore_stacks: true,
                        ..ParseOptions::default()
                    },
                );
                let target = sim.unit(pet).current_target;
                let aura = target.and_then(|target| sim.get_aura(target, &row.name));
                if let (Some(aura), true) = (
                    aura,
                    parsed.skipped.is_empty()
                        && parsed.applied.len() == 1
                        && parsed.applied[0].kind == "stat Armor",
                ) {
                    effects.push(json!({
                        "kind": "hunter_pet_dust_cloud", "spell_id": row.id,
                        "aura": sim.aura(aura).label, "armor": parsed.applied[0].value,
                    }));
                }
            }
        }
        // newFuriousHowl (the fork's patch 119): no damage mask. The aura it gives the pet and the
        // hunter carries the row's melee attack power, which the pet takes as a dynamic stat; the
        // hunter's is a stat aura the effect leaves to `stat_auras`. The hunter takes it while
        // within the effect's radius of the target.
        {
            let row = data.furious_howl_triggered.highest();
            let cast = sim
                .unit(pet)
                .spellbook
                .iter()
                .any(|spell| sim.spell(*spell).action_id == ActionId::spell(row.id));
            if cast {
                let parsed = dry_run(row, ParseOptions::default());
                let pet_aura = sim.get_aura(pet, &row.name);
                let owner_aura = sim.get_aura(unit, &row.name);
                if let (Some(pet_aura), Some(owner_aura), true) = (
                    pet_aura,
                    owner_aura,
                    parsed.skipped.is_empty()
                        && parsed.applied.len() == 1
                        && parsed.applied[0].kind == "stat AttackPower",
                ) {
                    let mut radius = 15.0;
                    let effect = row.effect(dbcenums::A_MOD_ATTACK_POWER, 0);
                    if effect.radius_max > 0.0 {
                        radius = f64::from(effect.radius_max);
                    }
                    effects.push(json!({
                        "kind": "hunter_pet_furious_howl", "spell_id": row.id,
                        "pet_aura": sim.aura(pet_aura).label,
                        "owner_aura": sim.aura(owner_aura).label, "radius": radius,
                        "attack_power": parsed.applied[0].value,
                    }));
                }
            }
        }
        // The ability slots, as positions in the pet's spellbook, or -1 for an empty slot.
        let slot = |spell: Option<SpellId>| {
            spell.map_or(-1, |spell| spell_position(sim, pet, spell) as i64)
        };
        effects.push(json!({
            "kind": "hunter_pet", "pet": sim.unit(pet).label, "rotation": rotation,
            "special_ability": slot(state.special_ability),
            "focus_dump": slot(state.focus_dump), "extra_ability": slot(state.extra_ability),
            "wait_ns": 500 * MILLISECOND, "melee_range": MAX_MELEE_RANGE,
            "move_to": MAX_MELEE_RANGE - 1.0,
            // pet.go Reset: the share of the fight before ExecuteCustomRotation disables the pet.
            "uptime": self.options.f64("pet_uptime").clamp(0.0, 1.0),
        }));
        if let Some(aura) = sim.get_aura(pet, "Intimidation") {
            for spell in &sim.unit(unit).spellbook {
                if Some(&sim.spell(*spell).action_id) == sim.aura(aura).action_id.as_ref() {
                    effects.push(json!({
                        "kind": "intimidation", "spell_id": sim.spell(*spell).action_id.spell_id,
                        "aura": sim.aura(aura).label, "crit_bonus": 100.0,
                    }));
                }
            }
        }
        if let Some(aura) = state.bestial_wrath_aura {
            effects.push(json!({
                "kind": "bestial_wrath",
                "spell_id": sim.aura(aura).action_id.as_ref().map_or(0, |id| id.spell_id),
                "aura": sim.aura(aura).label, "damage_multiplier": 1.5,
            }));
        }
        if let Some(trigger) = sim.get_aura(pet, "Frenzy") {
            let frenzy = sim.get_aura(pet, "Frenzy Effect").expect("Frenzy Effect");
            effects.push(json!({
                "kind": "frenzy", "trigger_aura": sim.aura(trigger).label,
                "aura": sim.aura(frenzy).label,
                "proc_chance": data.frenzy.fraction_at(self.t("frenzy")),
                "speed_multiplier": 1.3, "delay_ns": SPELL_BATCH_WINDOW,
            }));
        }
        effects
    }

    /// Go `hunterMeleeEffects`: Aspect of the Beast, Raptor Strike, Mongoose Bite with
    /// Lacerating Strikes, Strider Kick, Wing Clip, the traps, Resourcefulness and Expose Prey.
    fn melee_effects(&self, sim: &Sim, unit: UnitId) -> Vec<Value> {
        let data = spell_data();
        let mut effects = Vec::new();
        // aspects.go: Aspect of the Beast's attack power is a stat aura; Deadly Aspects procs
        // Quick Strikes on landed melee white hits.
        if let (Some(aura), Some(spell)) = (self.aspect_of_the_beast_aura, self.aspect_of_the_beast)
        {
            let mut effect = json!({
                "kind": "aspect_of_the_beast", "spell_id": spell_id_of(sim, spell),
                "aura": sim.aura(aura).label,
            });
            let deadly_aspects = self.t("deadly_aspects");
            if deadly_aspects > 0 {
                let rank = data.aspect_of_the_beast_triggered.highest();
                let proc = sim.get_aura(unit, "Quick Strikes").expect("Quick Strikes");
                effect["proc_aura"] = json!(sim.aura(proc).label);
                effect["haste_multiplier"] = json!(
                    1.0 + rank
                        .effect(dbcenums::A_MOD_MELEE_HASTE_3, 0)
                        .average(CHARACTER_LEVEL)
                        / 100.0
                );
                effect["proc_chance"] =
                    json!(data.deadly_aspects.effect_at(2).fraction_at(deadly_aspects));
            }
            effects.push(effect);
        }
        // raptor_strike.go: the queue spell's aura makes the next main hand swing cast Raptor
        // Strike, whose hit adds the rank's flat damage to a main hand weapon swing.
        if let (Some(spell), Some(_)) = (self.raptor_strike, self.raptor_strike_hit) {
            let queue = sim
                .get_aura(unit, "Raptor Strike Queued")
                .expect("Raptor Strike Queued");
            effects.push(json!({
                "kind": "raptor_strike", "spell_id": spell_id_of(sim, spell),
                "queue_aura": sim.aura(queue).label,
                "base_damage": data.raptor_strike.highest().damage_effect().average(CHARACTER_LEVEL),
                "melee_range": MAX_MELEE_RANGE,
            }));
        }
        // mongoose_bite.go and lacerating_strikes.go.
        if let Some(spell) = self.mongoose_bite {
            let defensive_state = self.defensive_state.expect("Defensive State");
            let mut effect = json!({
                "kind": "mongoose_bite", "spell_id": spell_id_of(sim, spell),
                "aura": sim.aura(defensive_state).label,
                "base_damage": data.mongoose_bite.highest().damage_effect().average(CHARACTER_LEVEL),
            });
            if self.lacerating_strikes.is_some() {
                effect["lacerating_share"] = json!(0.4);
                effect["lacerating_tick_outcome"] =
                    json!(tick_outcome(data.lacerating_strikes_triggered.highest()));
            }
            effects.push(effect);
        }
        if let Some(spell) = self.strider_kick {
            // strider_kick.go: the kick activates an aura of the client row's speed effect after
            // its hit.
            let mut effect = json!({"kind": "strider_kick", "spell_id": spell_id_of(sim, spell)});
            if let Some(speed) = sim.get_aura(unit, "Strider Kick") {
                effect["speed_aura"] = json!(sim.aura(speed).label);
            }
            effects.push(effect);
        }
        if let Some(spell) = self.wing_clip {
            effects.push(json!({
                "kind": "wing_clip", "spell_id": spell_id_of(sim, spell),
                "base_damage": data.wing_clip.highest().effect_n(2).average(CHARACTER_LEVEL),
            }));
        }
        // traps.go: Freezing Trap deals nothing. Go's exporter has no effect for it, so only
        // Forever's rules describe it.
        if let Some(spell) = self
            .freezing_trap
            .filter(|_| sim.rules == crate::prepare::sim::Rules::Forever)
        {
            effects.push(json!({"kind": "freezing_trap", "spell_id": spell_id_of(sim, spell)}));
        }
        // traps.go: a magic hit roll without a hit count, dealt, then the dot when it landed.
        if let Some(spell) = self.immolation_trap {
            let rank = data.immolation_trap.highest();
            let effect = data.immolation_trap_triggered.rank(rank.rank_number());
            effects.push(json!({
                "kind": "immolation_trap", "spell_id": spell_id_of(sim, spell),
                "tick_base": effect.periodic_effect().average(CHARACTER_LEVEL),
                // traps.go trapAttackPowerShare (patch 147), split over the burn's ticks.
                "attack_power_share": TRAP_ATTACK_POWER_SHARE,
                // spelldata TickOutcomeHitRolled: a crit where the effect row states Periodic Can
                // Crit.
                "tick_can_crit": effect.periodic_can_crit(),
                "tick_magic": sim.spell(spell).defense_type == DefenseType::Magic,
            }));
        }
        // traps.go registerExplosiveTrapSpell: one hit for each active target, then the area dot
        // on the hunter.
        if let Some(spell) = self.explosive_trap {
            let rank = data.explosive_trap.highest();
            let effect = data.explosive_trap_effect.rank(rank.rank_number());
            let hit_range = match rank.id {
                13813 => EXPLOSIVE_TRAP_RANGE[1],
                14316 => EXPLOSIVE_TRAP_RANGE[2],
                14317 => EXPLOSIVE_TRAP_RANGE[3],
                _ => [0.0, 0.0],
            };
            effects.push(json!({
                "kind": "explosive_trap", "spell_id": spell_id_of(sim, spell),
                "hit_min": hit_range[0], "hit_max": hit_range[1],
                // traps.go explosiveTrapHitDamage: trapAttackPowerShare (patch 147) on each roll.
                "attack_power_share": TRAP_ATTACK_POWER_SHARE,
                "hits": active_target_count(sim),
                "aoe_cap_multiplier": aoe_cap_multiplier(active_target_count(sim)),
                "tick_base": effect.effect(dbcenums::A_PERIODIC_DAMAGE, 0).average(CHARACTER_LEVEL),
                "tick_can_crit": effect.periodic_can_crit(),
                "tick_magic": sim.spell(spell).defense_type == DefenseType::Magic,
            }));
        }
        // volley.go: the channel holds the ranged swing for the rank's duration, then the area
        // dot on the hunter snapshots the Go literal tick of the rank.
        if let Some(spell) = self.volley {
            let rank = data.volley.highest();
            effects.push(json!({
                "kind": "volley", "spell_id": spell_id_of(sim, spell),
                "tick_base": VOLLEY_TICK_DAMAGE[rank.rank_number() as usize],
                "ranged_delay_ns": rank.duration(),
            }));
        }
        // talents_survival.go: Resourcefulness's crit trigger and its casting regeneration, and
        // Expose Prey's landed hits on a marked target opening the Mongoose Bite window.
        if let Some(trigger) = sim.get_aura(unit, "Resourcefulness Trigger") {
            let buff = data.resourcefulness_triggered.highest();
            let aura = sim
                .get_aura(unit, "Resourcefulness")
                .expect("Resourcefulness");
            let special = ProcTrigger {
                proc_mask: ProcMask::MELEE_SPECIAL | ProcMask::RANGED_SPECIAL,
                ..ProcTrigger::default()
            };
            let special_spells: Vec<usize> = sim
                .unit(unit)
                .spellbook
                .iter()
                .enumerate()
                .filter(|(_, spell)| special.matches_spell(sim.spell(**spell)))
                .map(|(position, _)| position)
                .collect();
            effects.push(json!({
                "kind": "resourcefulness", "trigger_aura": sim.aura(trigger).label,
                "aura": sim.aura(aura).label,
                "proc_chance": 0.5 * f64::from(self.t("resourcefulness")),
                "regen": buff.effect(dbcenums::A_MOD_MANA_REGEN_INTERRUPT, 0)
                    .average(CHARACTER_LEVEL) / 100.0,
                // 16 October 2026 beta notes: only melee and ranged ability crits.
                "spells": special_spells,
            }));
        }
        if let Some(trigger) = sim.get_aura(unit, "Expose Prey") {
            let target = sim.unit(unit).current_target.expect("a current target");
            let marked = sim
                .auras_with_tag(target, "HuntersMark")
                .into_iter()
                .any(|aura| sim.aura(aura).active);
            let defensive_state = self.defensive_state.expect("Defensive State");
            effects.push(json!({
                "kind": "expose_prey", "trigger_aura": sim.aura(trigger).label,
                "aura": sim.aura(defensive_state).label,
                "proc_chance": data.expose_prey.fraction_at(self.t("expose_prey")),
                "marked": marked,
            }));
        }
        effects
    }

    /// Go `hunterSetManaProcs`: set bonus proc triggers that restore mana. Each handler waits a
    /// spell batch window and restores mana to a character with a mana bar.
    fn set_mana_procs(&self, sim: &Sim, unit: UnitId) -> Vec<Value> {
        let procs: [(&str, ProcMask, f64, &str, f64, i32); 3] = [
            (
                "Beaststalker Armor 5P",
                ProcMask::WHITE_HIT,
                0.05,
                "landed",
                200.0,
                450577,
            ),
            (
                "Beastmaster Armor 4P",
                ProcMask::WHITE_HIT,
                0.05,
                "landed",
                200.0,
                450577,
            ),
            (
                "Cryptstalker Armor 6P",
                ProcMask::RANGED,
                1.0,
                "crit",
                50.0,
                28753,
            ),
        ];
        let mut effects = Vec::new();
        for (label, proc_mask, chance, outcome, mana, metrics) in procs {
            if sim.get_aura(unit, label).is_none() {
                continue;
            }
            let trigger = ProcTrigger {
                proc_mask,
                proc_chance: chance,
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                ..ProcTrigger::default()
            };
            let spells: Vec<usize> = sim
                .unit(unit)
                .spellbook
                .iter()
                .enumerate()
                .filter(|(_, spell)| trigger.matches_spell(sim.spell(**spell)))
                .map(|(i, _)| i)
                .collect();
            effects.push(json!({
                "kind": "hunter_set_mana_proc", "trigger_aura": label,
                "spells": spells, "outcome": outcome,
                "proc_chance": chance, "mana": mana,
                "metrics_action_id": action_id(Some(&ActionId::spell(metrics))),
                "delay_ns": SPELL_BATCH_WINDOW,
            }));
        }
        effects
    }

    /// What the effects name as unrepresented while they run (`classNotes`), and
    /// `hunterUnrepresented`.
    pub(super) fn export_notes(&self, sim: &Sim, unit: UnitId) -> Vec<String> {
        let data = spell_data();
        let mut notes = Vec::new();
        if self.summon_hawk.is_some() {
            let rank = data.summon_hawk.highest();
            let hawks = sim
                .unit(unit)
                .spellbook
                .iter()
                .filter(|spell| {
                    let action = &sim.spell(**spell).action_id;
                    action.spell_id == rank.id && action.tag > 0
                })
                .count();
            if hawks != rank.effect_n(3).base_points as usize {
                notes.push(format!("Summon Hawk registered {hawks} hawks"));
            }
        }
        if let Some(pet) = self.pet {
            for spell in sim.unit(pet).spellbook.clone() {
                if sim.spell(spell).class_spell_mask != masks::PET_DAMAGE {
                    continue;
                }
                if self.pet_ability(sim, pet, spell).is_none() {
                    notes.push(format!(
                        "pet ability {} is unsupported",
                        crate::prepare::common_effects::action_id_string(
                            &sim.spell(spell).action_id
                        )
                    ));
                }
            }
            // newDustCloud's effects.
            let row = data.dust_cloud_triggered.highest();
            let cast = sim
                .unit(pet)
                .spellbook
                .iter()
                .any(|spell| sim.spell(*spell).action_id == ActionId::spell(row.id));
            if cast {
                let parsed = dry_run(
                    row,
                    ParseOptions {
                        ignore_stacks: true,
                        ..ParseOptions::default()
                    },
                );
                let target = sim.unit(pet).current_target;
                let aura = target.and_then(|target| sim.get_aura(target, &row.name));
                if aura.is_none()
                    || !parsed.skipped.is_empty()
                    || parsed.applied.len() != 1
                    || parsed.applied[0].kind != "stat Armor"
                {
                    notes.push("Dust Cloud's effects are unsupported".to_string());
                }
            }
            // newFuriousHowl's effects.
            let row = data.furious_howl_triggered.highest();
            let cast = sim
                .unit(pet)
                .spellbook
                .iter()
                .any(|spell| sim.spell(*spell).action_id == ActionId::spell(row.id));
            if cast {
                let parsed = dry_run(row, ParseOptions::default());
                if sim.get_aura(pet, &row.name).is_none()
                    || sim.get_aura(unit, &row.name).is_none()
                    || !parsed.skipped.is_empty()
                    || parsed.applied.len() != 1
                    || parsed.applied[0].kind != "stat AttackPower"
                {
                    notes.push("Furious Howl's effects are unsupported".to_string());
                }
            }
        }
        if sim.get_aura(unit, "Expose Prey").is_some() {
            if let Some(target) = sim.unit(unit).current_target {
                for aura in sim.auras_with_tag(target, "HuntersMark") {
                    if sim.aura(aura).duration != NEVER_EXPIRES {
                        notes.push("Expose Prey reads a Hunter's Mark that can change".to_string());
                    }
                }
            }
        }
        // summon_hawk.go deals periodic physical damage, which this multiplier would scale.
        if self.summon_hawk.is_some() {
            if let Some(target) = sim.unit(unit).current_target {
                if sim
                    .unit(target)
                    .pseudo_stats
                    .periodic_physical_damage_taken_multiplier
                    != 1.0
                {
                    notes.push(
                        "periodic physical damage taken multipliers are unsupported".to_string(),
                    );
                }
            }
        }
        notes
    }

    /// Go `hunterSwingReplacementKeepsSwing`: raptor_strike.go `TryRaptorStrike` returns the swing
    /// it is given unless a Raptor Strike is queued, and only the queue spell, which only a
    /// rotation casts, queues one. A rotation that never names the queue spell keeps every swing.
    pub(super) fn swing_replacement_keeps_swing_impl(&self) -> bool {
        let Some(raptor_strike) = self.raptor_strike_id else {
            return true;
        };
        let queue = ActionId {
            spell_id: raptor_strike,
            tag: 3,
            ..ActionId::default()
        };
        let Some(rotation) = &self.rotation else {
            return true;
        };
        !names_action(rotation, &queue)
    }
}

/// Go `Encounter.AOECapMultiplier`: the damage an area spell keeps per target above the cap.
fn aoe_cap_multiplier(targets: usize) -> f64 {
    (20.0 / targets as f64).min(1.0)
}

/// Whether a message names an action ID anywhere inside it: the exporter's reflective visit.
fn names_action(message: &Message, wanted: &ActionId) -> bool {
    if message.type_name() == "proto.ActionID" {
        return crate::prepare::agent::proto_to_action_id(message) == *wanted;
    }
    for name in message.set_fields() {
        let found = match message.get(name) {
            Some(RequestValue::Message(inner)) => names_action(inner, wanted),
            Some(RequestValue::List(items)) => items.iter().any(|item| match item {
                RequestValue::Message(inner) => names_action(inner, wanted),
                _ => false,
            }),
            Some(RequestValue::Map(entries)) => entries.iter().any(|(_, item)| match item {
                RequestValue::Message(inner) => names_action(inner, wanted),
                _ => false,
            }),
            _ => false,
        };
        if found {
            return true;
        }
    }
    false
}
