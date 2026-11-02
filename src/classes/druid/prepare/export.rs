//! The exporter's Druid part: tools/oracle-v2/druid.go, druid_feral.go, druid_bear.go and
//! druid_items.go. The client damage rows of the Druid's spells and the effects whose
//! parameters Go keeps in closures.

use serde_json::{json, Value};

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::attack::Weapon;
use crate::prepare::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use crate::prepare::buffs::exclusive_refresh;
use crate::prepare::buffs::generated::{DEMORALIZING_ROAR, FAERIE_FIRE};
use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::common_effects::exclusive_category_effect;
use crate::prepare::dbcenums::{
    A_ADD_PCT_MODIFIER, A_MOD_CASTING_SPEED_NOT_STACK, A_MOD_DECREASE_SPEED, A_NONE,
    A_PERIODIC_ENERGIZE, SPELLMOD_CHANCE_OF_SUCCESS, SPELLMOD_COST, SPELLMOD_GLOBAL_COOLDOWN,
    SPELLMOD_PROC_COOLDOWN,
};
use crate::prepare::env::Environment;
use crate::prepare::export::action_id;
use crate::prepare::rage::rage_bar_effect;
use crate::prepare::resolve_proc::{chance, proc_trigger};
use crate::prepare::sim::{Sim, SpellId, UnitId, SECOND};
use crate::prepare::spell::{DefenseType, SpellFlag, GCD_DEFAULT};
use crate::prepare::spelldata::{Ladder, Spell as Row};

use super::forms::{
    bear_weapon, cat_weapon, form_names, spell_position, weapon_from_main_hand,
    ANIMAL_SPIRIT_REGEN_SUPPRESSION, BEAR_FORM_THREAT_MULTIPLIER, CAT_FORM_THREAT_MULTIPLIER,
};
use super::{masks, Druid};

/// buffs/drivers.go `innervateSpiritRegenMultiplier` and `innervateRegenTag`.
const INNERVATE_SPIRIT_REGEN_MULTIPLIER: f64 = 5.0;
const INNERVATE_REGEN_TAG: i32 = -2;

/// The ranks of the ladders the exporter and the registration share.
const STARFIRE_RANKS: [i32; 7] = [2912, 8949, 8950, 8951, 9875, 9876, 25298];
const WRATH_RANKS: [i32; 8] = [5176, 5177, 5178, 5179, 5180, 6780, 8905, 9912];
const MOONFIRE_RANKS: [i32; 10] = [8921, 8924, 8925, 8926, 8927, 8928, 8929, 9833, 9834, 9835];
const INSECT_SWARM_RANKS: [i32; 5] = [5570, 24974, 24975, 24976, 24977];
const HURRICANE_RANKS: [i32; 3] = [16914, 17401, 17402];
const HURRICANE_TRIGGERED: [i32; 3] = [1278965, 1278968, 1278759];
const CAT_FORM_RANK: [i32; 1] = [768];
const PROWL_RANKS: [i32; 3] = [5215, 6783, 9913];
const RAVAGE_RANKS: [i32; 4] = [6785, 6787, 9866, 9867];
const SHRED_RANKS: [i32; 5] = [5221, 6800, 8992, 9829, 9830];
const CLAW_RANKS: [i32; 5] = [1082, 3029, 5201, 9849, 9850];
const RIP_RANKS: [i32; 6] = [1079, 9492, 9493, 9752, 9894, 9896];
const RAKE_RANKS: [i32; 4] = [1822, 1823, 1824, 9904];
const BITE_RANKS: [i32; 5] = [22568, 22827, 22828, 22829, 31018];
const BEAR_ENRAGE_RANK: [i32; 1] = [5229];
const MAUL_RANKS: [i32; 7] = [6807, 6808, 6809, 8972, 9745, 9880, 9881];
const LACERATE_RANKS: [i32; 3] = [414644, 1235826, 1235827];
const SWIPE_RANKS: [i32; 5] = [779, 780, 769, 9754, 9908];

/// Go `HitOutcome.String` of one flag, which `outcomeNames` lists.
fn outcome_names(outcome: HitOutcome) -> Vec<&'static str> {
    [
        (HitOutcome::MISS, "Miss"),
        (HitOutcome::HIT, "Hit"),
        (HitOutcome::DODGE, "Dodge"),
        (HitOutcome::GLANCE, "Glance"),
        (HitOutcome::PARRY, "Parry"),
        (HitOutcome::BLOCK, "Block"),
        (HitOutcome::CRIT, "Crit"),
        (HitOutcome::CRUSH, "Crush"),
        (HitOutcome::PARTIAL_1_4, "Empty"),
        (HitOutcome::PARTIAL_2_4, "Empty"),
        (HitOutcome::PARTIAL_3_4, "Empty"),
    ]
    .into_iter()
    .filter(|(flag, _)| outcome.matches(*flag))
    .map(|(_, name)| name)
    .collect()
}

/// tools/oracle-v2/druid.go `callbackNames`.
fn callback_names(callback: CallbackMask) -> Vec<&'static str> {
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
    .filter(|(flag, _)| callback.matches(*flag))
    .map(|(_, name)| name)
    .collect()
}

/// The spellbook positions of spells whose class mask is in a set: `spellsMatching`.
fn spells_matching(sim: &Sim, unit: UnitId, mask: i64) -> Vec<usize> {
    sim.unit(unit)
        .spellbook
        .iter()
        .enumerate()
        .filter(|(_, spell)| sim.spell(**spell).matches(mask))
        .map(|(i, _)| i)
        .collect()
}

/// The spellbook positions of spells a proc trigger listens to: `procTriggerSpells`.
fn proc_trigger_spells(sim: &Sim, unit: UnitId, trigger: &ProcTrigger) -> Vec<usize> {
    sim.unit(unit)
        .spellbook
        .iter()
        .enumerate()
        .filter(|(_, spell)| trigger.matches_spell(sim.spell(**spell)))
        .map(|(i, _)| i)
        .collect()
}

/// tools/oracle-v2/druid.go `periodicRank`.
fn periodic_rank(row: &Row) -> Value {
    json!({
        "spell_id": row.id,
        "tick_base": row.periodic_effect().average(CHARACTER_LEVEL),
        "tick_can_crit": row.periodic_can_crit() && row.defense_type_core() == DefenseType::Magic,
    })
}

/// main.go `exportWeapon`.
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

/// The spells a form-breaking consumable registers: conjured items and explosives. A potion no
/// longer drops the form (fork patch 111).
fn form_breaking_spells(sim: &Sim, unit: UnitId) -> Vec<usize> {
    let flags = SpellFlag::CONJURED | SpellFlag::EXPLOSIVE;
    sim.unit(unit)
        .spellbook
        .iter()
        .enumerate()
        .filter(|(_, spell)| sim.spell(**spell).flags.matches(flags))
        .map(|(i, _)| i)
        .collect()
}

/// Go `nanos` of a duration.
fn nanos(d: i64) -> i64 {
    d
}

impl Druid {
    /// The damage rows `druidDamageRows` rolls by spell ID.
    pub(super) fn druid_damage_effect(&self, sim: &Sim, spell: SpellId) -> Option<Value> {
        let action = &sim.spell(spell).action_id;
        if action.spell_id == 0 || action.tag != 0 {
            return None;
        }
        let id = action.spell_id;
        let highest = |ids: &[i32]| Ladder::ranked(ids).highest().id;
        let wanted = STARFIRE_RANKS.contains(&id)
            || WRATH_RANKS.contains(&id)
            || id == highest(&BITE_RANKS)
            || id == highest(&MAUL_RANKS)
            || id == super::bear::primal_bite_rank(sim.rules).id
            || id == highest(&MOONFIRE_RANKS);
        if !wanted {
            return None;
        }
        let row = crate::prepare::hotfix_113554::row(sim.rules, id);
        let effect = row.damage_effect();
        if effect.is_nil() {
            return None;
        }
        Some(json!({"average": effect.average(CHARACTER_LEVEL), "variance": effect.variance}))
    }

    /// Go `druidEffects`.
    pub(super) fn druid_effects(&self, env: &Environment, notes: &mut Vec<String>) -> Vec<Value> {
        let sim = &env.sim;
        let unit = env.player;
        let mut effects: Vec<Value> = Vec::new();

        // MythicSim's healer sims (Rust only): the Restoration druid's heals.
        if !self.heals.is_empty() {
            effects.push(crate::prepare::heals::heals_effect(env, &self.heals));
            effects.extend(self.bloom_effect());
        }

        // thorns.go: the cast turns the druid's own Thorns on. Forever's rules describe it; Go's
        // exporter leaves the cast unknown.
        if sim.rules == crate::prepare::sim::Rules::Forever {
            let rotation = crate::prepare::rotation::rotation_spells(sim, unit);
            let thorns = sim.unit(unit).spellbook.iter().copied().find(|&spell| {
                let state = sim.spell(spell);
                state.class_spell_mask == masks::THORNS
                    && rotation.contains(&i64::from(state.action_id.spell_id))
            });
            if let Some(aura) = thorns.and_then(|spell| sim.spell(spell).related_self_buff) {
                effects.push(json!({
                    "kind": "druid_thorns",
                    "spell_id": sim.spell(thorns.expect("found")).action_id.spell_id,
                    "aura": sim.aura(aura).label,
                }));
            }
        }

        // druid.go RegisterSpell: the forms each spell may be cast in. forms.go: the form the
        // druid starts each fight in.
        let mut forms = Vec::new();
        for (i, spell) in sim.unit(unit).spellbook.iter().enumerate() {
            if let Some((_, form)) = self.form_masks.iter().find(|(id, _)| id == spell) {
                forms.push(json!({"spell": i, "forms": form_names(*form)}));
            }
        }
        effects.push(json!({
            "kind": "druid_forms", "starting_form": form_names(self.st.starting_form), "spells": forms,
        }));
        if let Some(aura) = sim.get_aura(unit, "Moonkin Form") {
            effects.push(json!({
                "kind": "moonkin_form", "spell_id": 24858, "aura": sim.aura(aura).label,
            }));
        }
        // starfire.go and wrath.go: a direct hit, Wrath's after travel.
        effects.push(json!({"kind": "starfire"}));
        effects.push(json!({"kind": "wrath"}));
        // moonfire.go: the hit applies the tagged dot spell's dot when it lands.
        if self.moonfire.is_some() {
            let moonfire = Ladder::ranked(&MOONFIRE_RANKS).highest();
            effects.push(json!({"kind": "moonfire", "rank": periodic_rank(moonfire)}));
        }
        let target = env.encounter.targets[0];
        if self.tal.insect_swarm {
            let rank = Ladder::ranked(&INSECT_SWARM_RANKS).highest();
            let insect_swarm = self
                .insect_swarm
                .expect("Insect Swarm is registered with its talent");
            let dot = sim.spell(insect_swarm).dots[sim.unit(target).unit_index as usize]
                .expect("Insect Swarm has a dot on the target");
            let dot_label = sim.aura(sim.dots[dot.0].aura).label.clone();
            let mut debuff = String::new();
            for aura in &sim.unit(target).auras {
                let aura = sim.aura(*aura);
                if aura.action_id == Some(ActionId::spell(rank.id)) && aura.label != dot_label {
                    debuff = aura.label.clone();
                }
            }
            effects.push(json!({
                "kind": "insect_swarm", "rank": periodic_rank(rank), "debuff_aura": debuff,
            }));
        }
        if self.hurricane.is_some() {
            let tick = Ladder::ranked(&HURRICANE_TRIGGERED).highest();
            effects.push(json!({
                "kind": "hurricane",
                "spell_id": Ladder::ranked(&HURRICANE_RANKS).highest().id,
                "tick_spell_id": tick.id,
                "tick_base": tick.damage_effect().average(CHARACTER_LEVEL),
            }));
        }
        // innervate.go and buffs/drivers.go AttachInnervateRegen: Go literals.
        if let Some(aura) = sim.get_aura(unit, "Innervates (Player)") {
            let aura = sim.aura(aura);
            let mut regen = aura.action_id.clone().unwrap_or_default();
            regen.tag = INNERVATE_REGEN_TAG;
            effects.push(json!({
                "kind": "innervate", "spell_id": Ladder::ranked(&[29166]).highest().id,
                "aura": aura.label, "spirit_regen_multiplier": INNERVATE_SPIRIT_REGEN_MULTIPLIER,
                "regen_metrics_action_id": action_id(Some(&regen)),
            }));
        }
        // omen_of_clarity.go: the trigger's spells, outcome and cooldown come from the client
        // row as spelldata.ProcTrigger resolves them; the chance is the cast time share of two
        // a minute.
        let clearcasting = Ladder::ranked(&[16870]).highest();
        let omen = Ladder::ranked(&[16864]).highest();
        let moonkin = Ladder::ranked(&[24858]).highest();
        let trigger = proc_trigger(sim, Some(unit), omen, &[chance(1.0)]);
        effects.push(json!({
            "kind": "omen_of_clarity", "trigger_aura": trigger.name, "aura": "Clearcasting",
            "callbacks": callback_names(trigger.callback), "outcome": outcome_names(trigger.outcome),
            "require_damage_dealt": trigger.require_damage_dealt,
            "trigger_spells": proc_trigger_spells(sim, unit, &trigger),
            "trigger_immediately": trigger.trigger_immediately, "proc_chance": trigger.proc_chance,
            "icd_ns": nanos(omen.icd()), "ppm": 2.0, "gcd_ns": nanos(GCD_DEFAULT),
            "moonkin_chance_multiplier":
                1.0 + moonkin.effect(A_ADD_PCT_MODIFIER, SPELLMOD_CHANCE_OF_SUCCESS).percent(),
            "moonkin_cooldown_multiplier":
                1.0 + moonkin.effect(A_ADD_PCT_MODIFIER, SPELLMOD_PROC_COOLDOWN).percent(),
            "cost_spells": spells_matching(sim, unit, masks::CLEARCASTING_SPELLS),
            "cost_percent_add": clearcasting.effect(A_ADD_PCT_MODIFIER, SPELLMOD_COST).percent(),
            // 16 October 2026 beta notes: Faerie Fire no longer triggers it in Cat or Bear Form.
            "form_excluded_spells":
                spells_matching(sim, unit, masks::FAERIE_FIRE | masks::FAERIE_FIRE_FERAL),
        }));
        if self.tal.natures_grace {
            // talents_balance.go applyNaturesGrace
            let triggered = Ladder::ranked(&[16886]).highest();
            let gcd_reduction = (GCD_DEFAULT as f64
                * triggered
                    .effect(A_ADD_PCT_MODIFIER, SPELLMOD_GLOBAL_COOLDOWN)
                    .percent()) as i64;
            let gcd_mask = masks::ENTANGLING_ROOTS
                | masks::FAERIE_FIRE
                | masks::HURRICANE
                | masks::INSECT_SWARM
                | masks::MOONFIRE
                | masks::STARFIRE
                | masks::THORNS
                | masks::WRATH
                | masks::HEALING_TOUCH
                | masks::REGROWTH
                | masks::REJUVENATION
                | masks::TRANQUILITY
                | masks::MARK_OF_THE_WILD;
            effects.push(json!({
                "kind": "natures_grace", "trigger_aura": "Nature's Grace Trigger",
                "aura": "Nature's Grace",
                "haste_multiplier":
                    1.0 + triggered.effect(A_MOD_CASTING_SPEED_NOT_STACK, 0).base_value() / 100.0,
                "gcd_reduction_ns": nanos(gcd_reduction),
                "gcd_spells": spells_matching(sim, unit, gcd_mask),
                "trigger_spells": proc_trigger_spells(sim, unit, &ProcTrigger {
                    class_spell_mask: masks::DAMAGING_SPELLS,
                    ..ProcTrigger::default()
                }),
            }));
        }
        if self.tal.eclipse > 0 {
            // talents_balance.go applyEclipse: a Go literal of two charges a Wrath.
            let ladder = Ladder::talent(408248, 3);
            effects.push(json!({
                "kind": "eclipse", "trigger_aura": "Eclipse Trigger", "aura": "Eclipse",
                "cast_time_reduction_ns":
                    nanos(crate::prepare::sim::MILLISECOND
                        * (ladder.effect_at(2).value_at(self.tal.eclipse) as i64)),
                "charges_per_wrath": 2,
                "duration_ns": nanos(Ladder::ranked(&[408255]).highest().duration()),
            }));
        }
        effects.extend(self.druid_feral_effects(env, notes));
        effects.extend(self.druid_bear_effects(env, notes));
        effects.extend(self.druid_item_effects(sim, unit));
        effects
    }

    /// The threat multiplier a form multiplies when the agent's reset enters it: the initial
    /// value with every permanent aura the reset activates, with the form's own factor taken
    /// back out (`formStartThreat`).
    fn form_start_threat(&self, sim: &Sim, unit: UnitId, label: &str, factor: f64) -> f64 {
        let mut threat = sim.unit(unit).pseudo_stats.threat_multiplier;
        if sim
            .get_aura(unit, label)
            .is_some_and(|aura| sim.aura(aura).active)
        {
            threat /= factor;
        }
        threat
    }

    /// Go `druidFeralEffects`.
    fn druid_feral_effects(&self, env: &Environment, notes: &mut Vec<String>) -> Vec<Value> {
        let sim = &env.sim;
        let unit = env.player;
        let mut effects = Vec::new();
        let target = env.encounter.targets[0];
        let initial = &sim.unit(unit).initial_pseudo_stats;

        if let (Some(_), Some(cat_aura)) = (self.cat_form, self.cat_form_aura) {
            // forms.go RegisterCatFormAura and registerCatFormSpell, with Furor's carry over.
            let furor = if self.tal.furor > 0 {
                Ladder::talent(17056, 5)
                    .effect_at(2)
                    .value_at(self.tal.furor)
            } else {
                0.0
            };
            let main_hand = weapon_from_main_hand(sim, unit);
            let cat = cat_weapon(sim, unit);
            // movement.go NewPassiveMovementSpeedEffect: the form's speed applies when its
            // effect holds the category, which the player_movement effect describes with the
            // aura.
            effects.push(json!({
                "kind": "cat_form", "spell_id": Ladder::ranked(&CAT_FORM_RANK).highest().id,
                "aura": sim.aura(cat_aura).label,
                "initial_threat_multiplier":
                    self.form_start_threat(sim, unit, "Cat Form", CAT_FORM_THREAT_MULTIPLIER),
                "threat_multiplier": CAT_FORM_THREAT_MULTIPLIER,
                "initial_spirit_regen_multiplier": initial.spirit_regen_multiplier,
                "spirit_regen_multiplier": ANIMAL_SPIRIT_REGEN_SUPPRESSION,
                "initial_movement_speed_multiplier": initial.movement_speed_multiplier,
                "movement_speed_bonus": 0.25,
                "furor_max": furor,
                "cost_spells": spells_matching(sim, unit, masks::FAERIE_FIRE),
                "gcd_spells": spells_matching(sim, unit, masks::FAERIE_FIRE),
                "gcd_delta_ns": nanos(-500 * crate::prepare::sim::MILLISECOND),
                "form_breaking_spells": form_breaking_spells(sim, unit),
                "main_hand": export_weapon(&main_hand),
                "cat_weapon": export_weapon(&cat),
            }));

            if let Some(prowl) = self.prowl_aura {
                // prowl.go
                let rank = Ladder::ranked(&PROWL_RANKS).highest();
                effects.push(json!({
                    "kind": "prowl", "spell_id": rank.id, "aura": sim.aura(prowl).label,
                    "movement_speed_multiplier":
                        1.0 + rank.effect(A_MOD_DECREASE_SPEED, 0).base_value() / 100.0,
                }));
            }

            // ravage.go, shred.go and claw.go: the rank's flat damage plus main hand weapon
            // damage, a combo point when it lands and a refund when it does not.
            let mut builders = Vec::new();
            for (kind, spell, ranks) in [
                ("ravage", self.ravage, &RAVAGE_RANKS[..]),
                ("shred", self.shred, &SHRED_RANKS[..]),
                ("claw", self.claw, &CLAW_RANKS[..]),
            ] {
                let Some(spell) = spell else { continue };
                builders.push(json!({
                    "kind": kind, "spell": spell_position(sim, unit, spell),
                    "flat_damage":
                        Ladder::ranked(ranks).highest().damage_effect().average(CHARACTER_LEVEL),
                }));
            }
            effects.push(json!({
                "kind": "cat_builders", "builders": builders, "cannot_shred": self.cannot_shred_target,
            }));
        }
        if let Some(rip) = self.rip {
            // rip.go: the tick base and points per combo point; the attack power share is a Go
            // literal.
            let rank = Ladder::ranked(&RIP_RANKS).highest();
            // spell_result.go TargetDamageMultiplier: a bleed tick also takes the target's
            // periodic physical multiplier.
            if sim
                .unit(target)
                .pseudo_stats
                .periodic_physical_damage_taken_multiplier
                != 1.0
            {
                notes.push("the target takes a periodic physical damage multiplier".to_string());
            }
            let tick = rank.periodic_effect();
            effects.push(json!({
                "kind": "rip", "spell": spell_position(sim, unit, rip),
                "tick_base": tick.average(CHARACTER_LEVEL),
                "tick_per_combo_point": f64::from(tick.points_per_resource),
                "attack_power_share_per_combo_point": 0.01,
                "attack_power_share_max_points": 4.0,
                "tick_can_crit": rank.periodic_can_crit(),
                "tick_magic": rank.defense_type_core() == DefenseType::Magic,
                "expected_combo_points": 5.0,
                "short_name": "Rip",
            }));
        }
        if let Some(rake) = self.rake {
            // rake.go: a flat hit and a flat bleed tick, neither scaling with attack power.
            let rank = Ladder::ranked(&RAKE_RANKS).highest();
            effects.push(json!({
                "kind": "rake", "spell": spell_position(sim, unit, rake),
                "flat_damage": rank.damage_effect().average(CHARACTER_LEVEL),
                "tick_base": rank.periodic_effect().average(CHARACTER_LEVEL),
                // rake.go: 5% of attack power on the hit and on every tick, a Go literal fitted
                // to beta logs (community #769).
                "attack_power_share": 0.05,
                "tick_can_crit": rank.periodic_can_crit(),
                "tick_magic": rank.defense_type_core() == DefenseType::Magic,
                "short_name": "Rake",
            }));
        }
        if let Some(bite) = self.ferocious_bite {
            // ferocious_bite.go: client values, and 3% attack power a point, a Go literal.
            let ladder = Ladder::ranked(&BITE_RANKS);
            let rank = ladder.highest();
            let mut effect = json!({
                "kind": "ferocious_bite", "spell": spell_position(sim, unit, bite),
                "damage_per_energy": ladder.effect_at(2).fraction_at(rank.rank_number()),
                "damage_per_combo_point": f64::from(rank.damage_effect().points_per_resource),
                "attack_power_per_combo_point": 0.03,
            });
            // Predatory Discipline (1328613, hotfix push 113531): "Ferocious Bite no longer
            // converts extra Energy into damage, but still deals extra damage based on excess
            // Energy". The talent's dummy (20) names no value in the tooltip and is unread.
            if self.tal.predatory_discipline {
                effect["keeps_energy"] = json!(true);
            }
            effects.push(effect);
        }
        if let Some(shifting_power) = self.shifting_power {
            // shifting_power.go
            effects.push(json!({
                "kind": "shifting_power", "spell": spell_position(sim, unit, shifting_power),
                "energy": Ladder::ranked(&[1322605]).highest().energize_effect().average(CHARACTER_LEVEL)
                    + sim_wolfshead_energy(sim, self),
            }));
        }
        if let (Some(faerie_fire), false) = (self.faerie_fire, self.faerie_fire_auras.is_empty()) {
            // faerie_fire.go and buffs FaerieFireAura
            let aura = self.faerie_fire_auras[sim.unit(target).unit_index as usize]
                .expect("Faerie Fire has an aura on the target");
            effects.push(json!({
                "kind": "faerie_fire", "spell": spell_position(sim, unit, faerie_fire),
                "aura": sim.aura(aura).label, "armor_reduction": FAERIE_FIRE.value(0),
                "refresh": exclusive_refresh(sim, aura),
            }));
        }
        if let Some(berserk) = self.berserk_aura {
            // talents_feral_combat.go applyBerserk: +100% crit, a Go literal.
            effects.push(json!({
                "kind": "berserk", "spell_id": 417141, "aura": sim.aura(berserk).label,
                "crit_percent": 100.0,
                "crit_spells": spells_matching(sim, unit,
                    masks::CLAW | masks::SHRED | masks::RAKE | masks::RAVAGE),
            }));
        }
        if self.tal.blood_frenzy > 0 {
            // talents_feral_combat.go applyBloodFrenzy
            let ladder = Ladder::talent(16958, 2);
            let trigger = ProcTrigger {
                name: "Blood Frenzy (Cat)".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                class_spell_mask: masks::BUILDER,
                outcome: HitOutcome::CRIT,
                proc_chance: ladder.effect_at(1).fraction_at(self.tal.blood_frenzy),
                ..ProcTrigger::default()
            };
            // The bear half: Rage on any melee crit in Bear Form, the client's energize in its
            // own units.
            let bear = ProcTrigger {
                proc_mask: crate::prepare::spell::ProcMask::MELEE,
                ..ProcTrigger::default()
            };
            let triggered = Ladder::ranked(&[16959]).highest();
            effects.push(json!({
                "kind": "blood_frenzy", "trigger_aura": trigger.name,
                "bear_trigger_aura": "Blood Frenzy (Bear)",
                "proc_chance": trigger.proc_chance,
                "trigger_spells": proc_trigger_spells(sim, unit, &trigger),
                "outcome": outcome_names(trigger.outcome),
                "trigger_immediately": trigger.trigger_immediately,
                "metrics_action_id": action_id(Some(&ActionId::spell(triggered.id))),
                "bear_trigger_spells": proc_trigger_spells(sim, unit, &bear),
                "bear_rage": triggered.effect_n(1).base_value() / 10.0,
            }));
        }
        if self.tal.rend_and_tear > 0 {
            // talents_feral_combat.go applyRendAndTear
            let bleeds: Vec<i64> = [self.rip, self.rake, self.lacerate]
                .into_iter()
                .flatten()
                .map(|spell| spell_position(sim, unit, spell))
                .collect();
            let special: Vec<usize> = sim
                .unit(unit)
                .spellbook
                .iter()
                .enumerate()
                .filter(|(_, spell)| {
                    sim.spell(**spell)
                        .proc_mask
                        .matches(crate::prepare::spell::ProcMask::MELEE_SPECIAL)
                })
                .map(|(i, _)| i)
                .collect();
            effects.push(json!({
                "kind": "rend_and_tear",
                "multiplier": Ladder::talent(1223246, 5).effect_at(1).multiplier_at(self.tal.rend_and_tear),
                "spells": special, "bleed_spells": bleeds,
            }));
        }
        // feralcat.go EnableRageBar: the cat's rage bar, whose listener acts only while it is
        // the current power bar, which only Bear Form makes it.
        if sim.get_aura(unit, "RageBar").is_some() && self.bear_form.is_none() {
            effects.push(rage_bar_effect(sim, unit, 1.0));
        }
        effects
    }

    /// Go `druidBearEffects`.
    fn druid_bear_effects(&self, env: &Environment, notes: &mut Vec<String>) -> Vec<Value> {
        let sim = &env.sim;
        let unit = env.player;
        let mut effects = Vec::new();
        if let (Some(bear_form), Some(bear_aura)) = (self.bear_form, self.bear_form_aura) {
            // rage.go EnableRageBar: feralbear.go passes BaseRageMultiplier 1.
            effects.push(rage_bar_effect(sim, unit, 1.0));
            let initial = &sim.unit(unit).initial_pseudo_stats;
            let main_hand = weapon_from_main_hand(sim, unit);
            let paw = bear_weapon(sim, unit);
            effects.push(json!({
                "kind": "bear_form", "spell_id": 9634, "aura": sim.aura(bear_aura).label,
                "spell": spell_position(sim, unit, bear_form),
                "health_bonus": self.bear_form_health_bonus(env, notes),
                "initial_threat_multiplier":
                    self.form_start_threat(sim, unit, "Bear Form", BEAR_FORM_THREAT_MULTIPLIER),
                "threat_multiplier": BEAR_FORM_THREAT_MULTIPLIER,
                "initial_spirit_regen_multiplier": initial.spirit_regen_multiplier,
                "spirit_regen_multiplier": ANIMAL_SPIRIT_REGEN_SUPPRESSION,
                "furor_proc_chance": self.furor_proc_chance,
                "cost_spells": spells_matching(sim, unit, masks::FAERIE_FIRE),
                "form_breaking_spells": form_breaking_spells(sim, unit),
                "main_hand": export_weapon(&main_hand), "bear_weapon": export_weapon(&paw),
            }));

            if let (Some(_), Some(enrage_aura)) = (self.enrage, self.st.enrage_aura.get()) {
                // enrage.go
                let rank = Ladder::ranked(&BEAR_ENRAGE_RANK).highest();
                effects.push(json!({
                    "kind": "enrage", "spell_id": rank.id, "aura": sim.aura(enrage_aura).label,
                    "instant_rage": rank.effect(A_NONE, 1).tenths()
                        + self.intensity_enrage_rage_bonus
                        + sim.state(self.st.wolfshead_enrage_rage),
                    "rage_per_tick": rank.effect(A_PERIODIC_ENERGIZE, 1).base_value() / 10.0,
                    "ticks": rank.duration() / SECOND,
                    "period_ns": nanos(SECOND),
                }));
            }
            if let Some(roar) = self.demoralizing_roar {
                if !self.demoralizing_roar_auras.is_empty() {
                    // demoralizing_roar.go
                    let target = env.encounter.targets[0];
                    let aura = self.demoralizing_roar_auras[sim.unit(target).unit_index as usize]
                        .expect("Demoralizing Roar has an aura on the target");
                    effects.push(json!({
                        "kind": "demoralizing_roar", "spell": spell_position(sim, unit, roar),
                        "aura": sim.aura(aura).label,
                    }));
                    // buffs.DemoralizingRoarAura and the raid's Demoralizing Roar and Shout
                    // debuffs share one single aura category, each bidding its attack power cut.
                    if let Some(category) = exclusive_category_effect(
                        sim,
                        target,
                        "target",
                        DEMORALIZING_ROAR.category,
                        notes,
                    ) {
                        effects.push(category);
                    }
                }
            }
            if let Some(maul) = self.maul {
                // maul.go: the queue spell, its aura and realism cooldown, and the strike.
                let rank = Ladder::ranked(&MAUL_RANKS).highest();
                let strike = sim
                    .unit(unit)
                    .spellbook
                    .iter()
                    .position(|spell| sim.spell(*spell).action_id == ActionId::spell(rank.id));
                let queue_aura = sim.get_aura(unit, "Maul Queue Aura");
                match (strike, queue_aura) {
                    (Some(strike), Some(queue_aura)) => effects.push(json!({
                        "kind": "maul", "spell": strike,
                        "queue_spell": spell_position(sim, unit, maul),
                        "queue_aura": sim.aura(queue_aura).label,
                        "realism_ns": nanos(super::bear::MAUL_REALISM),
                        "flat_damage": rank.damage_effect().average(CHARACTER_LEVEL),
                    })),
                    _ => notes.push("Maul is incomplete".to_string()),
                }
            }
            if let Some(lacerate) = self.lacerate {
                // lacerate.go: a flat tick a stack and a weapon share a stack.
                let ladder = Ladder::ranked(&LACERATE_RANKS);
                let rank = ladder.highest();
                effects.push(json!({
                    "kind": "lacerate", "spell": spell_position(sim, unit, lacerate),
                    "tick_base": rank.periodic_effect().average(CHARACTER_LEVEL),
                    "weapon_share_per_stack": ladder.effect_at(2).fraction_at(rank.rank_number()),
                    "max_stacks": super::bear::LACERATE_MAX_STACKS,
                    "tick_can_crit": rank.periodic_can_crit(),
                    "tick_magic": rank.defense_type_core() == DefenseType::Magic,
                }));
            }
            if let Some(primal_bite) = self.primal_bite {
                // primal_bite.go: Berserk lifts its cooldown, a Go literal.
                effects.push(json!({
                    "kind": "primal_bite", "spell": spell_position(sim, unit, primal_bite),
                    "flat_damage": super::bear::primal_bite_rank(sim.rules)
                        .damage_effect().average(CHARACTER_LEVEL),
                }));
            }
            if let Some(swipe) = self.swipe {
                // swipe.go: up to three targets, each a flat hit and a share of the attack power.
                effects.push(json!({
                    "kind": "swipe", "spell": spell_position(sim, unit, swipe),
                    "flat_damage": Ladder::ranked(&SWIPE_RANKS).highest()
                        .damage_effect().average(CHARACTER_LEVEL),
                    // swipeAttackPowerCoeff, a Go literal the client rows do not carry, fitted
                    // to beta logs (community #787).
                    "attack_power_coefficient": 0.05,
                }));
            }
            if self.barkskin.is_some() {
                // barkskin.go: the aura's physical damage taken cut is a stat aura.
                effects.push(json!({
                    "kind": "barkskin", "spell_id": Ladder::ranked(&[22812]).highest().id,
                    "aura": "Barkskin",
                }));
            }
            if let (Some(frenzied), Some(aura)) = (
                self.frenzied_regeneration,
                self.st.frenzied_regeneration_aura.get(),
            ) {
                // frenzied_regeneration.go
                let rank = Ladder::ranked(&[22842]).highest();
                effects.push(json!({
                    "kind": "frenzied_regeneration",
                    "spell": spell_position(sim, unit, frenzied),
                    "aura": sim.aura(aura).label, "ticks": rank.duration() / SECOND,
                    "period_ns": nanos(SECOND), "max_rage_per_tick": 10.0,
                    "health_share_per_rage": 0.01,
                    "healing_taken_multiplier": sim.unit(unit).pseudo_stats.healing_taken_multiplier,
                }));
            }
        }
        // talents_feral_combat.go applyNaturalReaction: its listener needs Bear Form, so a
        // cat's is inert in effect, which the runtime's form check gives.
        if self.tal.natural_reaction > 0 {
            let triggered = Ladder::ranked(&[417053]).highest();
            let trigger = ProcTrigger {
                name: "Natural Reaction".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_TAKEN,
                outcome: HitOutcome::DODGE,
                proc_chance: Ladder::talent(417051, 5)
                    .effect_at(2)
                    .fraction_at(self.tal.natural_reaction),
                ..ProcTrigger::default()
            };
            effects.push(json!({
                "kind": "natural_reaction", "trigger_aura": trigger.name,
                "proc_chance": trigger.proc_chance, "outcome": outcome_names(trigger.outcome),
                "trigger_immediately": trigger.trigger_immediately,
                "rage": triggered.effect_n(1).base_value() / 10.0,
                "metrics_action_id": action_id(Some(&ActionId::spell(triggered.id))),
            }));
        }
        effects
    }

    /// forms.go Bear Form's health on a shift, read from separate reset simulations: the health
    /// its stat bonus, a flat 1240 health and the form shift stats without any, adds to the
    /// maximum. Both shifts are checked against Go.
    fn bear_form_health_bonus(&self, env: &Environment, notes: &mut Vec<String>) -> f64 {
        let unit = env.player;
        let sim = &env.sim;
        let bear = self.bear_form_aura.expect("a bear form has its aura");
        if !sim.aura(bear).active {
            notes.push("Bear Form is not up after the reset".to_string());
            return 0.0;
        }
        // AddStatsDynamic recomputes the stats from those before dependencies, so a health
        // multiplier such as Tauren Endurance also scales the bonus.
        let in_form = sim.unit(unit).stats[crate::prepare::stats::Stat::Health];
        let mut without = sim.unit(unit).stats_without_deps;
        without[crate::prepare::stats::Stat::Health] -= 1240.0;
        let bonus = in_form
            - sim
                .unit(unit)
                .sdm
                .apply_stat_dependencies(without)
                .floor_game_stats()[crate::prepare::stats::Stat::Health];
        // Leaving the form must not raise the maximum.
        let mut fresh = env.fresh();
        let fresh_bear = fresh
            .sim
            .get_aura(fresh.player, "Bear Form")
            .expect("the fresh reset has the aura");
        fresh.sim.current_time = SECOND;
        fresh.sim.deactivate(fresh_bear);
        let out_of_form = fresh.sim.unit(fresh.player).stats[crate::prepare::stats::Stat::Health];
        if out_of_form > in_form - bonus {
            notes.push("leaving Bear Form raises maximum health".to_string());
        }
        bonus
    }

    /// tools/oracle-v2/druid_items.go `druidItemEffects`.
    fn druid_item_effects(&self, sim: &Sim, unit: UnitId) -> Vec<Value> {
        let mut effects = Vec::new();
        if let Some(aura) = sim.get_aura(unit, "Feralheart Raiment 4P") {
            let mana = ProcTrigger {
                proc_mask: crate::prepare::spell::ProcMask::SPELL_DAMAGE
                    | crate::prepare::spell::ProcMask::SPELL_HEALING,
                ..ProcTrigger::default()
            };
            let energy = ProcTrigger {
                proc_mask: crate::prepare::spell::ProcMask::MELEE_WHITE_HIT,
                ..ProcTrigger::default()
            };
            effects.push(json!({
                "kind": "natures_bounty", "aura": sim.aura(aura).label, "proc_chance": 0.02,
                "mana_label": "Nature's Bounty (Mana)", "mana": 200.0,
                "mana_spells": proc_trigger_spells(sim, unit, &mana),
                "energy_label": "Nature's Bounty (Energy)", "energy": 20.0,
                "energy_spells": proc_trigger_spells(sim, unit, &energy),
                "rage_label": "Nature's Bounty (Rage)", "rage": 10.0,
                "metrics_action_id": action_id(Some(&ActionId::spell(450608))),
            }));
        }
        // Wildheart Raiment's Nature's Bounty hears only melee hits the player takes.
        if let Some(aura) = sim.get_aura(unit, "Wildheart Raiment 5P") {
            effects.push(json!({
                "kind": "inert_listener", "unit": "player", "aura": sim.aura(aura).label,
                "reason": "acts only on melee hits the player takes",
            }));
        }
        if let Some(aura) = sim.get_aura(unit, "Symbols of Unending Life 3P") {
            let trigger = ProcTrigger {
                class_spell_mask: masks::FEROCIOUS_BITE | masks::RIP,
                ..ProcTrigger::default()
            };
            effects.push(json!({
                "kind": "unending_life_refund", "aura": sim.aura(aura).label, "energy": 30.0,
                "spells": proc_trigger_spells(sim, unit, &trigger),
                "label": "Symbols of Unending Life Finisher Bonus",
                "metrics_action_id": action_id(Some(&ActionId::spell(26107))),
            }));
        }
        effects
    }
}

/// The Wolfshead Helm's extra Shifting Power energy as the druid state holds it.
fn sim_wolfshead_energy(sim: &Sim, druid: &Druid) -> f64 {
    sim.state(druid.st.wolfshead_shifting_power_energy)
}
