//! The exporter's Shaman part, tools/oracle-v2/shaman.go: the client damage rows of the
//! Shaman's spells (`shamanDamageRows`, `shamanTaggedDamageRows`), the effects whose parameters
//! Go keeps in closures (`shamanEffects`) and the notes it cannot describe
//! (`shamanUnrepresented`).

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::{json, Value};

use crate::prepare::aura_helpers::ProcTrigger;
use crate::prepare::buffs::flametongue::{
    flametongue_totem_base_damage, FLAMETONGUE_TOTEM_TRIGGER_LABEL,
    WINDFURY_WEAPON_MAIN_HAND_STRIKE,
};
use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::common_effects::{action_id_string, flat_string};
use crate::prepare::consumable_effects::proc_trigger_spells;
use crate::prepare::dbcenums;
use crate::prepare::env::Environment;
use crate::prepare::export_items::{dpm_chances, outcome_names};
use crate::prepare::resolve_proc::{chance, proc_trigger};
use crate::prepare::sim::{Duration, Sim, SpellId, MILLISECOND, SECOND};
use crate::prepare::spell::{DefenseType, ProcMask, SpellFlag};
use crate::prepare::spelldata::{must_find, Ladder, Spell as Row};
use crate::prepare::stats::{Stat, Stats};

use super::spell_data::spell_data;
use super::spells::{chain_lightning_overload_ranks, lightning_bolt_overload_ranks};
use super::{Shaman, CAST_TAG_LIGHTNING_OVERLOAD};

fn nanos(d: Duration) -> i64 {
    d
}

/// `shamanDamageRows`: the rows by spell ID, those whose `ApplyEffects` roll a client damage
/// effect: every Lightning Bolt and Chain Lightning rank, Lava Burst's, Flame Shock's, Earth
/// Shock's and Frost Shock's highest rank.
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
        each(&mut rows, &data.lightning_bolt);
        each(&mut rows, &data.chain_lightning);
        highest(&mut rows, &data.lava_burst);
        highest(&mut rows, &data.flame_shock);
        highest(&mut rows, &data.earth_shock);
        highest(&mut rows, &data.frost_shock);
        rows
    })
}

/// `shamanTaggedDamageRows`: Lightning Overload's bolts, one client row per rank of the spell
/// they follow, by the parent's spell ID.
fn tagged_damage_rows() -> &'static HashMap<i32, &'static Row> {
    static ROWS: OnceLock<HashMap<i32, &'static Row>> = OnceLock::new();
    ROWS.get_or_init(|| {
        let data = spell_data();
        let mut rows: HashMap<i32, &'static Row> = HashMap::new();
        data.lightning_bolt.each(|rank, row| {
            rows.insert(row.id, lightning_bolt_overload_ranks().rank(rank));
        });
        data.chain_lightning.each(|rank, row| {
            rows.insert(row.id, chain_lightning_overload_ranks().rank(rank));
        });
        rows
    })
}

/// Go `attachDamageEffects` for one spell: the roll of the client row, `{average, variance}`.
pub(super) fn damage_effect(sim: &Sim, spell: SpellId) -> Option<Value> {
    let action = &sim.spell(spell).action_id;
    if action.spell_id == 0 {
        return None;
    }
    let row = if action.tag != 0 {
        if action.tag != CAST_TAG_LIGHTNING_OVERLOAD {
            return None;
        }
        tagged_damage_rows().get(&action.spell_id)?
    } else {
        damage_rows().get(&action.spell_id)?
    };
    let effect = row.damage_effect();
    if effect.is_nil() {
        return None;
    }
    Some(json!({"average": effect.average(CHARACTER_LEVEL), "variance": effect.variance}))
}

/// The positions of the spellbook entries with this exact action.
fn spell_positions(
    env: &Environment,
    action: &crate::contracts::prepared_v2::ActionId,
) -> Vec<usize> {
    env.sim
        .unit(env.player)
        .spellbook
        .iter()
        .enumerate()
        .filter(|(_, spell)| &env.sim.spell(**spell).action_id == action)
        .map(|(position, _)| position)
        .collect()
}

fn spell_id_action(id: i32) -> crate::contracts::prepared_v2::ActionId {
    crate::contracts::prepared_v2::ActionId::spell(id)
}

impl Shaman {
    /// Go `shamanEffects`, with `playerEffects` after them.
    pub(super) fn shaman_effects(&self, env: &Environment, notes: &mut Vec<String>) -> Vec<Value> {
        let data = spell_data();
        let sim = &env.sim;
        let player = env.player;
        let talent = |name: &str| self.talent(name);
        let has = |name: &str| self.has_talent(name);
        let mut effects: Vec<Value> = Vec::new();
        // MythicSim's healer sims (Rust only): the Restoration shaman's heals.
        if !self.heals.is_empty() {
            let mut heals = crate::prepare::heals::heals_effect(env, &self.heals);
            self.riptide_chain_heal_bonus(env, &mut heals);
            effects.push(heals);
        }

        // lightning_bolt.go: the overload rolls on landing, before the bolt deals its damage.
        effects.push(json!({
            "kind": "lightning_bolt", "overload_chance": self.overload_chance(),
            "overload_tag": CAST_TAG_LIGHTNING_OVERLOAD,
            "rng_label": "Lightning Bolt Elemental Overload",
        }));
        // chain_lightning.go: a third of the overload chance per hit, and a Go literal bounce
        // reduction.
        effects.push(json!({
            "kind": "chain_lightning", "overload_chance": self.overload_chance(),
            "overload_tag": CAST_TAG_LIGHTNING_OVERLOAD,
            "rng_label": "Chain Lightning Elemental Overload", "bounce_reduction": 0.7,
            "bounce_bonus": sim.state(self.chain_lightning_bounce_bonus),
        }));
        // shocks.go registerFlameShockSpell: the dot snapshots its tick and rolls the family
        // table's outcome.
        let flame_shock = data.flame_shock.highest();
        let tick = flame_shock.periodic_effect();
        effects.push(json!({
            "kind": "flame_shock", "spell_id": flame_shock.id,
            "tick_base": tick.average(CHARACTER_LEVEL),
            "tick_can_crit": flame_shock.periodic_can_crit()
                && flame_shock.defense_type_core() == DefenseType::Magic,
        }));
        if has("lava_burst") {
            // lava_burst.go: the bonus against a target burning with Flame Shock.
            let row = data.lava_burst.highest();
            effects.push(json!({
                "kind": "lava_burst", "spell_id": row.id,
                "flame_shock_bonus": 1.0 + row.effect_n(2).percent(),
            }));
        }
        // fire_totems.go registerFireNovaSpell: one hit on each target from the nova's damage
        // row average.
        effects.push(json!({
            "kind": "fire_nova", "spell_id": data.fire_nova.highest().id,
            "base_damage": data.fire_nova_triggered.by_id(408428).damage_effect().average(CHARACTER_LEVEL),
        }));
        // fire_totems.go registerSearingTotemSpell: a target dot whose ticks cast the attack.
        let searing = data.searing_totem.highest();
        let searing_attack = data.searing_totem_triggered.highest();
        effects.push(json!({
            "kind": "searing_totem", "spell_id": searing.id, "attack_spell_id": searing_attack.id,
            "attack_damage": searing_attack.damage_effect().average(CHARACTER_LEVEL),
            "magma_totem_aura": "Magma Totem", "flametongue_totem_aura": "Flametongue Totem (Self)",
            "duration_ns": nanos(searing.duration()),
        }));
        // shocks.go registerEarthShockSpell and registerFrostShockSpell.
        effects.push(json!({"kind": "earth_shock", "spell_id": data.earth_shock.highest().id}));
        effects.push(json!({"kind": "frost_shock", "spell_id": data.frost_shock.highest().id}));
        effects.extend(self.imbue_effects(env, notes));
        effects.extend(self.totem_effects(env, notes));
        // totems.go registerStrengthOfEarthTotemSpell: the earth totem's aura; its Strength
        // reaches the fight through the class's stat auras.
        if let Some(aura) = sim.get_aura(player, "Strength Of Earth Totem (Self)") {
            let row = data.strength_of_earth_totem.highest_for(env.rules);
            effects.push(json!({
                "kind": "strength_of_earth_totem", "spell_id": row.id,
                "aura": sim.aura(aura).label, "duration_ns": nanos(row.duration()),
            }));
        }
        if has("stormstrike") {
            // stormstrike.go: the target debuff raises this shaman's lightning damage.
            let row = data.stormstrike.highest();
            effects.push(json!({
                "kind": "stormstrike", "spell_id": row.id,
                "aura": format!("Stormstrike-{}", sim.unit(player).label),
                "damage_multiplier": 1.0 + row.effect(dbcenums::A_MOD_SPELL_DAMAGE_FROM_CASTER, 0).percent(),
                "has_main_hand": sim.mh_weapon(player).is_some(),
                "has_off_hand": sim.oh_weapon(player).is_some(),
            }));
        }
        if talent("elemental_devastation") > 0 {
            // talents_elemental.go applyElementalDevastation
            effects.push(json!({
                "kind": "elemental_devastation", "trigger_aura": "Elemental Devastation Trigger",
                "aura": "Elemental Devastation",
                "melee_crit": data.elemental_devastation.effect(dbcenums::A_DUMMY, 0)
                    .value_at(talent("elemental_devastation")),
                // Forever's rank lacks the bit: a crit from a spell flagged Proc, as an
                // overload, does not count.
                "can_proc_from_procs": data.elemental_devastation.highest().can_proc_from_procs(),
                // Totems and what they cast never trigger it, Fire Nova included (community
                // #789).
                "excluded_class_spells": ["fire_nova"],
            }));
        }
        if talent("flurry") > 0 {
            // talents_enhancement.go applyFlurry: a Go literal 500 ms charge cooldown.
            effects.push(json!({
                "kind": "flurry", "trigger_aura": "Flurry Trigger", "aura": "Flurry",
                "melee_speed_multiplier": data.flurry.multiplier_at(talent("flurry")),
                "charge_icd_ns": nanos(500 * MILLISECOND),
                "max_stacks": i32::from(data.flurry_triggered.highest().proc_charges),
                // Forever's rank 16256 lacks the bit: a hit from a spell flagged Proc does not
                // trigger it.
                "can_proc_from_procs": data.flurry.highest().can_proc_from_procs(),
            }));
        }
        if has("stormstrike") && talent("improved_stormstrike") > 0 {
            // talents_enhancement.go applyImprovedStormstrike
            effects.push(json!({
                "kind": "improved_stormstrike", "trigger_aura": "Improved Stormstrike Trigger",
                "aura": "Improved Stormstrike", "reset_aura": "Improved Stormstrike Reset",
                "proc_chance": data.improved_stormstrike.effect_at(1)
                    .fraction_at(talent("improved_stormstrike")),
                "spirit_regen_rate_casting": data.improved_stormstrike_triggered.highest()
                    .effect(dbcenums::A_MOD_MANA_REGEN_INTERRUPT, 0).percent(),
            }));
        }
        // talents_enhancement.go applyMaelstromWeapon: 10 PPM, a Go literal, rolled per hand.
        if let Some(trigger) = sim.get_aura(player, "Maelstrom Weapon Trigger") {
            let aura = sim.aura(trigger);
            if talent("maelstrom_weapon") > 0 {
                if let Some(dpm) = aura.dpm.clone() {
                    effects.push(json!({
                        "kind": "maelstrom_weapon", "trigger_aura": aura.label,
                        "aura": "Maelstrom Weapon",
                        "per_stack": data.maelstrom_weapon.effect_at(1)
                            .fraction_at(talent("maelstrom_weapon")),
                        "max_stacks": 5,
                        "chances": dpm_chances(env, &dpm, |spell| {
                            spell.proc_mask.matches(ProcMask::MELEE)
                                && !spell.flags.matches(SpellFlag::PROC)
                        }),
                    }));
                }
            }
        }
        // weapon_imbues.go RegisterRockbiterImbue: a permanent temporary stats aura, already in
        // the prepared stats, that logs its gain and loss.
        if let Some(aura) = sim.get_aura(player, "Rockbiter Weapon") {
            let aura = sim.aura(aura);
            let bonus = Stats::from_pairs(&[(
                Stat::AttackPower,
                data.rockbiter_weapon_triggered
                    .highest()
                    .effect(dbcenums::A_MOD_ATTACK_POWER, 0)
                    .average(CHARACTER_LEVEL)
                    * (1.0
                        + data
                            .elemental_weapons
                            .effect_at(1)
                            .fraction_at(talent("elemental_weapons"))),
            )]);
            let id = aura.action_id.clone().unwrap_or_default();
            let mut effect = json!({
                "kind": "rockbiter_weapon", "aura": aura.label,
                "gain_log": format!("Gained {} from {}.", flat_string(&bonus), action_id_string(&id)),
                "expire_log": format!("Lost {} from fading {}.", flat_string(&bonus), action_id_string(&id)),
            });
            // Spirit Weapons' threat, which the aura multiplies in as it is gained and divides out
            // as it fades when it follows the weapon in the hand. Go's exporter does not say.
            let threat = sim.state(self.rockbiter_swap_threat);
            if threat != 0.0 && sim.rules == crate::prepare::sim::Rules::Forever {
                effect["threat_multiplier"] = json!(threat);
            }
            effects.push(effect);
        }
        // weapon_imbues.go registerRockbiterThreat (patch 181): the hands whose weapons carry Rockbiter
        // and the main hand's swing speed, read from the weapons worn now.
        if let Some(aura) = sim.get_aura(player, "Rockbiter Weapon Threat") {
            let equipment = &sim.character(player).equipment;
            let main_hand = &equipment[crate::prepare::items::slot::MAIN_HAND];
            let off_hand = &equipment[crate::prepare::items::slot::OFF_HAND];
            effects.push(json!({
                "kind": "rockbiter_threat", "aura": sim.aura(aura).label,
                "spell_id": super::imbues::ROCKBITER_THREAT_SPELL_ID,
                "main_hand": main_hand.temp_enchant == super::imbues::ROCKBITER_ENCHANT_ID,
                "off_hand": off_hand.temp_enchant == super::imbues::ROCKBITER_ENCHANT_ID,
                "threat_per_swing_second": 72.0,
                "main_hand_swing_speed": main_hand.swing_speed,
            }));
        }
        if has("rage_of_the_farseer") {
            // talents_enhancement.go applyRageOfTheFarseer
            let row = data.rage_of_the_farseer.highest();
            effects.push(json!({
                "kind": "rage_of_the_farseer", "spell_id": row.id, "aura": "Rage of the Farseer",
                "melee_speed_multiplier": 1.0
                    + row.effect(dbcenums::A_MOD_MELEE_RANGED_HASTE_2, 0).percent(),
            }));
        }
        if self.has_talent("elemental_mastery") {
            // talents_elemental.go applyElementalMastery: a major cooldown whose aura speeds
            // Lightning Bolt and Lava Burst and cancels Chain Lightning's cooldown.
            let row = data.elemental_mastery.highest();
            let chain = data.chain_lightning.highest();
            effects.push(json!({
                "kind": "elemental_mastery", "spell_id": row.id, "aura": "Elemental Mastery",
                "cast_time_delta_ns": nanos(super::talents::millis(
                    row.effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_CASTING_TIME)
                        .base_points,
                )),
                "chain_lightning_cooldown_ns": nanos(chain.cooldown().max(chain.category_cooldown())),
            }));
        }
        if has("elemental_focus") {
            // talents_elemental.go applyElementalFocus
            let clearcasting = data.elemental_focus_triggered.highest();
            effects.push(json!({
                "kind": "elemental_focus", "trigger_aura": "Elemental Focus", "aura": "Clearcasting",
                "proc_chance": f64::from(data.elemental_focus.rank(1).proc_chance) / 100.0,
                "cost_percent_add": clearcasting
                    .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_COST).percent(),
                "max_stacks": i32::from(clearcasting.proc_charges),
            }));
        }
        // enhancement.go ApplySyncType: the main hand swing replacement that moves the off hand
        // swing, with Flurry's charge cooldown a Go literal.
        if self.enhancement_options {
            let sync = match self.sync_type.as_str() {
                "Auto" => {
                    let mh = sim.character(player).equipment
                        [crate::prepare::items::slot::MAIN_HAND]
                        .swing_speed;
                    let oh = sim.character(player).equipment[crate::prepare::items::slot::OFF_HAND]
                        .swing_speed;
                    Some(if mh != oh { "none" } else { "auto" })
                }
                "SyncMainhandOffhandSwings" => Some("sync"),
                "DelayOffhandSwings" => Some("delay"),
                _ => None,
            };
            if let Some(sync) = sync {
                effects.push(json!({
                    "kind": "weapon_sync", "sync": sync,
                    "flurry_icd_ns": nanos(500 * MILLISECOND),
                }));
            }
        }
        effects
    }

    /// Go `shamanTotemEffects`: the totems and the shield the shaman may cast.
    fn totem_effects(&self, env: &Environment, notes: &mut Vec<String>) -> Vec<Value> {
        let data = spell_data();
        let sim = &env.sim;
        let player = env.player;
        let mut effects: Vec<Value> = Vec::new();
        // registerMagmaTotemSpell: an area dot on the shaman whose pulses roll hit and crit on
        // each target from the pulse's damage row average.
        let magma = data.magma_totem.highest();
        effects.push(json!({
            "kind": "magma_totem", "spell_id": magma.id,
            "pulse_damage": data.magma_totem_triggered.by_id(10581).damage_effect().average(CHARACTER_LEVEL),
            // Trial by Fire (client 70425) lengthens it.
            "duration_ns": nanos(magma.duration() + super::spells::magma_totem_bonus(self.talent("trial_by_fire"))),
        }));
        // registerLightningShieldSpell: the cast puts up every charge. Its trigger hears only
        // the shield self proc, which needs a proc rate the exporter rejects, so the orb never
        // fires.
        if let Some(aura) = sim.get_aura(player, "Lightning Shield") {
            let row = data.lightning_shield.highest();
            effects.push(json!({
                "kind": "lightning_shield", "spell_id": row.id, "aura": sim.aura(aura).label,
                "charges": i32::from(row.proc_charges),
            }));
        }
        // registerWaterShieldSpell (talent): the cast puts up every globe; the shaman's own
        // critical spells spend one for mana (Forever's rules only: Go never casts it). The hit
        // taken trigger never fires in scope.
        if let Some(aura) = sim
            .get_aura(player, "Water Shield")
            .filter(|_| sim.rules == crate::prepare::sim::Rules::Forever)
        {
            use super::shields::*;
            effects.push(json!({
                "kind": "water_shield", "spell_id": WATER_SHIELD_SPELL_ID,
                "aura": sim.aura(aura).label, "globes": WATER_SHIELD_GLOBES,
                "icd_ns": nanos(WATER_SHIELD_ICD), "mana_percent": WATER_SHIELD_MANA_PERCENT,
                "mana_spell_id": WATER_SHIELD_MANA_SPELL_ID,
                "trigger_spells": proc_trigger_spells(env, &water_shield_crit_trigger()),
            }));
        }
        // item_sets.go Stormcaller's Garb (3), under Forever's rules (Go's exporter refuses its
        // listener): a landed Lightning Bolt, Chain Lightning, Overload or Shock rolls 20% and a
        // batch window later gives 50 Nature damage for 8 sec.
        if sim.rules == crate::prepare::sim::Rules::Forever {
            let set = sim.get_aura(player, "Stormcaller's Garb 3P");
            let wrath = sim.get_aura(player, "Stormcaller's Wrath");
            if let (Some(set), Some(wrath)) = (set, wrath) {
                let trigger = ProcTrigger {
                    name: "Stormcaller Spelldamage Bonus".to_string(),
                    callback: crate::prepare::aura_helpers::CallbackMask::ON_SPELL_HIT_DEALT,
                    outcome: crate::prepare::aura_helpers::HitOutcome::LANDED,
                    class_spell_mask: super::masks::LIGHTNING_BOLT
                        | super::masks::CHAIN_LIGHTNING
                        | super::masks::OVERLOAD
                        | super::masks::SHOCK,
                    can_proc_from_procs: true,
                    proc_chance: 0.20,
                    ..ProcTrigger::default()
                };
                let bonus = Stats::from_pairs(&[(Stat::NatureDamage, 50.0)]);
                let id = crate::contracts::prepared_v2::ActionId::spell(26121);
                effects.push(json!({
                    "kind": "spell_data_stat_proc", "trigger_aura": sim.aura(set).label,
                    "aura": sim.aura(wrath).label,
                    "trigger_spells": proc_trigger_spells(env, &trigger),
                    "callbacks": ["on_spell_hit_dealt"], "landed_only": true,
                    "require_damage": false, "proc_chance": trigger.proc_chance,
                    "gain_log": format!("Gained {} from {}.", flat_string(&bonus), action_id_string(&id)),
                    "expire_log": format!("Lost {} from fading {}.", flat_string(&bonus), action_id_string(&id)),
                }));
            }
        }
        // Since the 16 October 2026 beta notes no air totem category stands between the
        // shaman's own totems and the party's: only the party's Windfury Totem still contests
        // the shaman's own, through the Windfury Totem category.
        let party_windfury = sim.get_aura(player, "Windfury Totem").is_some();
        // registerGraceOfAirTotemSpell: the air totem's aura, whose Agility is a class stat
        // aura.
        if let Some(aura) = sim.get_aura(player, "Grace Of Air Totem (Self)") {
            let row = data.grace_of_air_totem.highest_for(env.rules);
            effects.push(json!({
                "kind": "grace_of_air_totem", "spell_id": row.id, "aura": sim.aura(aura).label,
                "duration_ns": nanos(row.duration()),
            }));
        }
        // registerWindfuryTotemSpell: the totem's aura refreshes two auras every 5 seconds, Go
        // literals; the second's exclusive effect turns the trigger on, which grants charges of
        // attack power and an extra main hand attack. The party's Windfury Totem or a main hand
        // Windfury Weapon would contest it.
        if let Some(totem) = sim.get_aura(player, "Windfury Totem (Self)") {
            let trigger = sim.get_aura(player, "Windfury Totem Trigger (Self)");
            let proc_aura = sim.get_aura(player, "Windfury Totem Proc (Self)");
            let extra = sim.unit(player).spellbook.iter().rposition(|spell| {
                let id = &sim.spell(*spell).action_id;
                id.other_id == "OtherActionAttack"
                    && id.tag == 10610
                    && id.spell_id == 0
                    && id.item_id == 0
            });
            let grant = proc_trigger(
                sim,
                Some(player),
                data.windfury_totem_triggered.by_id(10612),
                &[],
            );
            let spend = proc_trigger(
                sim,
                Some(player),
                data.windfury_totem_triggered.by_id(10610),
                &[chance(1.0)],
            );
            let landed = crate::prepare::aura_helpers::HitOutcome::LANDED;
            let hit_dealt = crate::prepare::aura_helpers::CallbackMask::ON_SPELL_HIT_DEALT;
            match (trigger, proc_aura, extra) {
                (Some(trigger), Some(proc_aura), Some(extra))
                    if grant.dpm.is_none()
                        && grant.outcome == landed
                        && spend.outcome == landed
                        && grant.callback == hit_dealt
                        && spend.callback == hit_dealt
                        && spend.icd == 0
                        && spend.dpm.is_none()
                        && grant.require_damage_dealt
                        && spend.require_damage_dealt =>
                {
                    let buff = data.windfury_totem_triggered.by_id(10610);
                    let value = buff.effect_n(1).average(CHARACTER_LEVEL);
                    let bonus = Stats::from_pairs(&[(Stat::AttackPower, value)]);
                    let proc_id = sim.aura(proc_aura).action_id.clone().unwrap_or_default();
                    let white = ProcTrigger {
                        proc_mask: ProcMask::MELEE_WHITE_HIT,
                        can_proc_from_procs: true,
                        ..ProcTrigger::default()
                    };
                    let imbue_on_main_hand = sim.get_aura(player, "Windfury Imbue").is_some()
                        && sim.character(player).equipment[crate::prepare::items::slot::MAIN_HAND]
                            .temp_enchant
                            == 283;
                    effects.push(json!({
                        "kind": "windfury_totem_self", "spell_id": data.windfury_totem.highest().id,
                        "totem_aura": sim.aura(totem).label,
                        "duration_ns": nanos(data.windfury_totem.highest().duration()),
                        "period_ns": nanos(5 * SECOND),
                        "tracking_aura": "Windfury Party Weapon Buff Tracking Aura",
                        "dummy_aura": "Windfury Dummy Aura (self)",
                        "trigger_aura": sim.aura(trigger).label,
                        "trigger_spells": crate::prepare::buffs::windfury_trigger_spells(env, &grant),
                        "trigger_proc_chance": grant.proc_chance,
                        "proc_aura": sim.aura(proc_aura).label,
                        "spend_spells": proc_trigger_spells(env, &spend),
                        "extra_spell": extra,
                        "white_spells": proc_trigger_spells(env, &white),
                        "proc_gain_log": format!("Gained {} from {}.", flat_string(&bonus), action_id_string(&proc_id)),
                        "proc_expire_log": format!("Lost {} from fading {}.", flat_string(&bonus), action_id_string(&proc_id)),
                        "contested": party_windfury || imbue_on_main_hand,
                    }));
                }
                _ => notes.push(
                    "Windfury Totem (Self) is not the shape the runtime implements".to_string(),
                ),
            }
        }
        // registerManaSpringTotemSpell: the water totem's aura, whose MP5 is a class stat aura.
        if let Some(aura) = sim.get_aura(player, "Mana Spring Totem (Self)") {
            let row = data.mana_spring_totem.highest();
            effects.push(json!({
                "kind": "mana_spring_totem", "spell_id": row.id, "aura": sim.aura(aura).label,
                "duration_ns": nanos(row.duration()),
            }));
        }
        // Mana Tide Totem (Forever's rules, Restoration): the aura's MP5, the water totem slot.
        if let Some(aura) = sim.get_aura(player, super::totems::MANA_TIDE_TOTEM_AURA) {
            let row = data.mana_tide_totem.highest();
            effects.push(json!({
                "kind": "mana_tide_totem", "spell_id": row.id, "aura": sim.aura(aura).label,
                "duration_ns": nanos(row.duration()),
                "healing_stream_totems": super::heals::healing_stream_totems(),
            }));
        }
        // registerFlametongueTotemSpell and buffs/flametongue_totem.go: the totem's aura turns
        // on the trigger, which casts the hit off landed main hand autos, unless a main hand
        // Flametongue Weapon or the party's totem holds the benefit.
        if let Some(aura) = self.flametongue_totem_aura {
            let mut trigger = proc_trigger(sim, Some(player), must_find(15036), &[]);
            trigger.proc_mask = ProcMask(trigger.proc_mask.0 & ProcMask::MELEE_MH.0);
            // Windfury Weapon's main-hand strikes proc it as the main hand's autos do (MythicSim
            // patch 141): the trigger hears main hand specials, kept to the autos and 439440.
            let mut trigger_spells = proc_trigger_spells(env, &trigger);
            if trigger.proc_mask.matches(ProcMask::MELEE_MH_AUTO) {
                trigger.proc_mask = ProcMask(trigger.proc_mask.0 | ProcMask::MELEE_MH_SPECIAL.0);
                let book = &sim.unit(player).spellbook;
                trigger_spells = proc_trigger_spells(env, &trigger)
                    .into_iter()
                    .filter(|&position| {
                        let spell = sim.spell(book[position]);
                        spell.proc_mask.matches(ProcMask::MELEE_MH_AUTO)
                            || spell.action_id == spell_id_action(WINDFURY_WEAPON_MAIN_HAND_STRIKE)
                    })
                    .collect();
            }
            let attack = spell_positions(env, &spell_id_action(16389))
                .last()
                .copied();
            let weapon = &sim.character(player).equipment[crate::prepare::items::slot::MAIN_HAND];
            let rank = data.flametongue_totem.highest();
            effects.push(json!({
                "kind": "flametongue_totem", "spell_id": rank.id, "aura": sim.aura(aura).label,
                "trigger_aura": FLAMETONGUE_TOTEM_TRIGGER_LABEL,
                "attack_spell": attack.map_or(-1, |position| position as i64),
                "attack_deals_damage": weapon.swing_speed != 0.0,
                "attack_damage": flametongue_totem_base_damage(weapon.swing_speed),
                "trigger_spells": trigger_spells,
                "trigger_outcome": outcome_names(trigger.outcome),
                "disabled_by_weapon": sim.get_aura(player, "Flametongue Imbue ItemSlotMainHand").is_some(),
                "duration_ns": nanos(rank.duration()),
                "party_totem": sim.get_aura(player, FLAMETONGUE_PARTY_TOTEM_LABEL).is_some(),
            }));
        }
        effects
    }

    /// Go `shamanImbueEffects`: Flametongue and Frostbrand Weapon, weapon procs that cast an
    /// imbue hit.
    fn imbue_effects(&self, env: &Environment, notes: &mut Vec<String>) -> Vec<Value> {
        let data = spell_data();
        let sim = &env.sim;
        let player = env.player;
        let mut effects: Vec<Value> = Vec::new();
        // RegisterFlametongueImbue: one trigger and one hit spell for each imbued weapon, main
        // hand first, registered in that order, the hit from the weapon's speed held to 1.3 to
        // 4.0.
        let imbue_hit = data.flametongue_weapon_triggered.by_id(16344);
        let hits = spell_positions(env, &spell_id_action(imbue_hit.id));
        let equipment = &sim.character(player).equipment;
        let mut flametongue: Vec<Value> = Vec::new();
        for (label, weapon, mask) in [
            (
                "Flametongue Imbue ItemSlotMainHand",
                &equipment[crate::prepare::items::slot::MAIN_HAND],
                ProcMask::MELEE_MH,
            ),
            (
                "Flametongue Imbue ItemSlotOffHand",
                &equipment[crate::prepare::items::slot::OFF_HAND],
                ProcMask::MELEE_OH,
            ),
        ] {
            if sim.get_aura(player, label).is_none() {
                continue;
            }
            let Some(hit) = hits.get(flametongue.len()) else {
                notes.push(format!("{label} has no hit spell"));
                continue;
            };
            let speed = weapon.swing_speed.clamp(1.3, 4.0);
            flametongue.push(json!({
                "trigger_aura": label, "spell": hit, "deals_damage": weapon.swing_speed != 0.0,
                "base_damage": speed * imbue_hit.effect_n(1).average(CHARACTER_LEVEL) / 100.0,
                "trigger_spells": proc_trigger_spells(env, &ProcTrigger {
                    proc_mask: mask,
                    is_weapon_proc: true,
                    ..ProcTrigger::default()
                }),
            }));
        }
        if !flametongue.is_empty() {
            effects.push(json!({"kind": "flametongue_weapon", "hands": flametongue}));
        }
        // RegisterWindfuryImbue and newWindfuryAttackSpell: a weapon proc with its own cooldown
        // that strikes twice, as two special hits of the hand that procced it (439440 main
        // hand, 439441 off hand), each a weapon hit with the rank's attack power added.
        if let Some(trigger_id) = sim.get_aura(player, "Windfury Imbue") {
            let trigger = sim.aura(trigger_id);
            if let Some(dpm) = trigger.dpm.clone() {
                let mh_attack = spell_positions(env, &spell_id_action(439440))
                    .last()
                    .copied();
                let oh_attack = spell_positions(env, &spell_id_action(439441))
                    .last()
                    .copied();
                let mut mask = ProcMask::UNKNOWN;
                if equipment[crate::prepare::items::slot::MAIN_HAND].temp_enchant == 283 {
                    mask = ProcMask(mask.0 | ProcMask::MELEE_MH.0);
                }
                if equipment[crate::prepare::items::slot::OFF_HAND].temp_enchant == 283 {
                    mask = ProcMask(mask.0 | ProcMask::MELEE_OH.0);
                }
                match (mh_attack, oh_attack, trigger.icd) {
                    (Some(mh_attack), Some(oh_attack), Some(_)) => {
                        effects.push(json!({
                            "kind": "windfury_weapon", "trigger_aura": trigger.label,
                            "trigger_spells": proc_trigger_spells(env, &ProcTrigger {
                                proc_mask: mask,
                                is_weapon_proc: true,
                                ..ProcTrigger::default()
                            }),
                            "chances": dpm_chances(env, &dpm, |spell| {
                                !spell.flags.matches(SpellFlag::SUPPRESS_WEAPON_PROCS)
                            }),
                            "main_hand_spells": proc_trigger_spells(env, &ProcTrigger {
                                proc_mask: ProcMask::MELEE_MH,
                                is_weapon_proc: true,
                                ..ProcTrigger::default()
                            }),
                            "main_hand_attack": mh_attack,
                            "off_hand_attack": oh_attack,
                            "attack_power": self.windfury_ap_bonus
                                * (1.0 + data.elemental_weapons.effect_at(3)
                                    .fraction_at(self.talent("elemental_weapons"))),
                            // A main hand imbue outbids the party Windfury Totem's category
                            // effect.
                            "blocks_windfury_totem": mask.matches(ProcMask::MELEE_MH),
                        }));
                    }
                    _ => notes.push("Windfury Weapon is incomplete".to_string()),
                }
            }
        }
        // RegisterFrostbrandImbue: 8 procs a minute, a Go literal, from the hands it imbues.
        if let Some(trigger_id) = sim.get_aura(player, "Frostbrand Imbue") {
            let trigger = sim.aura(trigger_id);
            if let Some(dpm) = trigger.dpm.clone() {
                let row = data.frostbrand_weapon_triggered.highest();
                effects.push(json!({
                    "kind": "frostbrand_weapon", "trigger_aura": trigger.label, "spell_id": row.id,
                    "base_damage": row.damage_effect().average(CHARACTER_LEVEL),
                    "chances": dpm_chances(env, &dpm, |spell| {
                        !spell.flags.matches(SpellFlag::SUPPRESS_WEAPON_PROCS)
                    }),
                }));
            }
        }
        effects
    }

    /// Go `shamanUnrepresented`: shaman behavior the exporter cannot describe.
    pub(super) fn shaman_unrepresented(&self) -> Vec<String> {
        let mut unrepresented = Vec::new();
        // shields.go startShieldProcPeriodicAction: a periodic self hit at encounter start.
        if self.self_buffs.shield_procrate > 0.0 {
            unrepresented.push("shaman shield proc rate is unsupported".to_string());
        }
        // shocks.go periodicTickOutcome: a physical crit roll on Flame Shock ticks.
        let row = spell_data().flame_shock.highest();
        if row.periodic_can_crit() && row.defense_type_core() != DefenseType::Magic {
            unrepresented.push("Flame Shock ticks roll a physical crit".to_string());
        }
        unrepresented
    }
}

/// The label of the party's Flametongue Totem (buffs/flametongue_totem.go).
const FLAMETONGUE_PARTY_TOTEM_LABEL: &str = "Flametongue Totem";
