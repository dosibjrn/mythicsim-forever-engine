//! The exporter's Rogue part, continued: tools/oracle-v2/rogue_specs.go, the Assassination and
//! Subtlety spells and talents. Each formula mirrors the cited file of sim/rogue at the pinned
//! revision.

use serde_json::{json, Value};

use crate::prepare::aura_helpers::ProcTrigger;
use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::common_effects::SPELL_BATCH_WINDOW;
use crate::prepare::consumable_effects::proc_trigger_spells;
use crate::prepare::dbcenums;
use crate::prepare::env::Environment;
use crate::prepare::sim::{Rules, SECOND};
use crate::prepare::spell::{DefenseType, ProcMask};

use super::export::{action, mask_names, poison_bonus};
use super::poisons::{poison_proc_mask, IMBUES};
use super::rework;
use super::spell_data::spell_data;
use super::spells::BUILDER;
use super::spells::SHANK_ENERGY_GAIN;
use super::util::{has_dagger, mutilate_weapon_share, proc_mask_for_types, spell_action, Hand};
use super::{masks, Rogue, SLICE_AND_DICE_DURATIONS};

/// A talent proc trigger's export: the spellbook positions it hears, by `matchesSpell`, and the
/// outcome it needs. Go's handler runs a spell batch window after the roll.
fn rogue_proc(
    env: &Environment,
    label: &str,
    handler: &str,
    chance: f64,
    trigger: &ProcTrigger,
    outcome: &str,
    periodic: bool,
) -> Value {
    json!({
        "kind": "rogue_proc", "trigger_aura": label, "handler": handler, "proc_chance": chance,
        "spells": proc_trigger_spells(env, trigger), "outcome": outcome, "periodic": periodic,
        "delay_ns": SPELL_BATCH_WINDOW,
    })
}

impl Rogue {
    /// tools/oracle-v2/rogue_specs.go `rogueSpecEffects`.
    pub(super) fn spec_effects(
        &self,
        env: &Environment,
        unrepresented: &mut Vec<String>,
    ) -> Vec<Value> {
        let sim = &env.sim;
        let player = env.player;
        let data = spell_data();
        let target = env.encounter.targets[0];
        let target_index = sim.unit(target).unit_index as usize;
        let talents = &self.talents;
        let level = CHARACTER_LEVEL;
        let target_aura_label = |auras: &[Option<crate::prepare::sim::AuraId>]| {
            auras
                .get(target_index)
                .copied()
                .flatten()
                .map(|aura| sim.aura(aura).label.clone())
        };
        let mut effects = Vec::new();

        // stealth.go and vanish.go: Stealth before the pull, and Vanish, which stops the swings.
        if let (Some(stealth), Some(vanish)) = (self.auras.stealth, self.spells.vanish) {
            effects.push(json!({
                "kind": "stealth", "aura": sim.aura(stealth).label,
                "spell_id": sim.aura(stealth).action_id.as_ref().map_or(0, |id| id.spell_id),
                "vanish_spell_id": sim.spell(vanish).action_id.spell_id,
            }));
        }
        // ambush.go: from Stealth, or under Cutthroat, with a main hand dagger.
        if self.spells.ambush.is_some() {
            let row = data.ambush.highest();
            // Cutthroat's aura, or under Forever's rules Ghostly Strike's: either lets the next
            // Ambush out of Stealth.
            let cutthroat = self
                .auras
                .cutthroat
                .or(self.auras.ghostly_ready)
                .map(|aura| sim.aura(aura).label.clone())
                .unwrap_or_default();
            effects.push(json!({
                "kind": "ambush", "spell_id": row.id,
                "base_damage": row.damage_effect().average(level),
                "main_hand_dagger": has_dagger(sim, player, Hand::Main),
                "cutthroat_aura": cutthroat,
            }));
        }
        // rupture.go: the tick and its step a combo point, the attack power share a point, a Go
        // literal, and the multiplier Hemorrhage's debuff gives the rogue's Rupture ticks while
        // it is up, read from the damage taken from caster effect of the client row
        // (talents_subtlety.go). Under Forever's rules it gives Garrote's ticks the same.
        if self.spells.rupture.is_some() {
            let row = data.rupture.highest();
            let tick = row.periodic_effect();
            let hemorrhage = target_aura_label(&self.auras.hemorrhage).unwrap_or_default();
            let mut rupture = json!({
                "kind": "rupture", "spell_id": row.id, "tick_damage": tick.average(level),
                "damage_per_combo_point": f64::from(tick.points_per_resource),
                "base_tick_count":
                    ((row.duration() + self.blood_splatter_rupture(env.rules)) / tick.period()) as i32,
                "attack_power_shares": [0.0, 0.01, 0.02, 0.03, 0.03, 0.03],
                "tick_can_crit": row.periodic_can_crit(),
                "magic": row.defense_type_core() == DefenseType::Magic,
                "hemorrhage_aura": hemorrhage,
                "hemorrhage_multiplier": 1.0
                    + data.hemorrhage.highest()
                        .effect(dbcenums::A_MOD_SPELL_DAMAGE_FROM_CASTER, 0)
                        .percent(),
            });
            // The notes of 2 November 2026 extend Hemorrhage's debuff to the rogue's Garrote.
            if env.rules == Rules::Forever {
                rupture["hemorrhage_garrote"] = json!(true);
            }
            effects.push(rupture);
        }
        // talents_assassination.go registerMutilate: two combo points, then the off hand and
        // main hand hits, each harder while a lingering poison is on the target.
        if self.spells.mutilate.is_some() {
            let row = data.mutilate.highest();
            effects.push(json!({
                "kind": "mutilate", "spell_id": row.id, "flat_damage": rework::mutilate_flat_damage(env.rules, row.id),
                "poison_bonus": row.effect_n(4).average(level) / 100.0,
                "combo_points": row.energize_effect().average(level) as i32,
                "daggers": has_dagger(sim, player, Hand::Main) && has_dagger(sim, player, Hand::Off),
                "weapon_share": mutilate_weapon_share(env.rules),
            }));
        }
        // talents_assassination.go registerColdBlood: a crit bonus on the masked spells until
        // one of them hits, except Mutilate's hand strikes, which take the crit; the off-hand
        // strike then spends it (community #785), which the runtime reads off the mutilate
        // effect's hands.
        if self.spells.cold_blood.is_some() {
            let row = data.cold_blood.highest();
            effects.push(json!({
                "kind": "cold_blood", "spell_id": row.id, "aura": "Cold Blood",
                "crit_bonus": row
                    .effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_CRITICAL_CHANCE)
                    .average(level),
                "class_spells": mask_names(masks::COLD_BLOODED),
                "spend_class_spells": mask_names(masks::COLD_BLOODED & !masks::MUTILATE_HIT),
            }));
        }
        // talents_subtlety.go registerPremeditation: combo points from Stealth.
        if self.spells.premeditation.is_some() {
            let row = data.premeditation.highest();
            effects.push(json!({
                "kind": "premeditation", "spell_id": row.id,
                "combo_points": row.energize_effect().average(level) as i32,
            }));
        }
        // talents_subtlety.go registerPreparation: it finishes the cooldown of every other rogue
        // spell that has one, in spellbook order, and it fires as a major cooldown once Vanish is
        // cooling down.
        if let Some(preparation) = self.spells.preparation {
            let reset: Vec<usize> = sim
                .unit(player)
                .spellbook
                .iter()
                .enumerate()
                .filter(|(_, spell)| {
                    **spell != preparation
                        && sim.spell(**spell).class_spell_mask & masks::ALL != 0
                        && sim.spell(**spell).cd.timer.is_some()
                })
                .map(|(i, _)| i)
                .collect();
            effects.push(json!({
                "kind": "preparation", "spell_id": sim.spell(preparation).action_id.spell_id,
                "reset_spells": reset,
            }));
        }
        // talents_assassination.go registerSealFate: a crit from a builder adds a combo point.
        let rank = talents.i32("seal_fate");
        if rank > 0 {
            let trigger = ProcTrigger {
                spell_flags: BUILDER,
                ..ProcTrigger::default()
            };
            let mut effect = rogue_proc(
                env,
                "Seal Fate Trigger",
                "combo_point",
                rework::talent_ladder(env.rules, 14186, 3, 5).fraction_at(rank),
                &trigger,
                "crit",
                false,
            );
            effect["action"] = action(14195);
            effects.push(effect);
        }
        // talents_subtlety.go registerInitiative: a landed Garrote or Ambush adds a combo point.
        let rank = talents.i32("initiative");
        if rank > 0 {
            let trigger = ProcTrigger {
                class_spell_mask: masks::GARROTE | masks::AMBUSH,
                ..ProcTrigger::default()
            };
            let mut effect = rogue_proc(
                env,
                "Initiative Trigger",
                "combo_point",
                data.initiative.fraction_at(rank),
                &trigger,
                "landed",
                false,
            );
            effect["action"] = action(data.initiative_triggered.highest().id);
            effects.push(effect);
        }
        // talents_subtlety.go registerCutthroat: a landed Backstab lets Ambush out of Stealth.
        if let Some(cutthroat) = self.auras.cutthroat {
            let trigger = ProcTrigger {
                class_spell_mask: masks::BACKSTAB,
                ..ProcTrigger::default()
            };
            let mut effect = rogue_proc(
                env,
                "Cutthroat Trigger",
                "activate",
                data.cutthroat.fraction_at(talents.i32("cutthroat")),
                &trigger,
                "landed",
                false,
            );
            effect["aura"] = json!(sim.aura(cutthroat).label);
            effects.push(effect);
        }
        // talents_combat.go registerHackAndSlash: on axes and swords, a landed hit from that hand
        // grants an extra main hand attack at once.
        let mask = proc_mask_for_types(sim, player, &["WeaponTypeAxe", "WeaponTypeSword"]);
        let points = talents.i32("hack_and_slash");
        if points > 0 && mask != ProcMask::UNKNOWN {
            let trigger = ProcTrigger {
                proc_mask: mask,
                ..ProcTrigger::default()
            };
            let mut effect = rogue_proc(
                env,
                "Hack and Slash",
                "extra_attack",
                data.hack_and_slash.effect_at(3).value_at(points) / 100.0,
                &trigger,
                "landed",
                false,
            );
            effect["delay_ns"] = json!(0_i64);
            effects.push(effect);
        }
        // poisons.go registerWoundPoisonSpell: a magic hit roll that stacks the healing debuff.
        if let Some(spell) = self.spells.wound_poison {
            let mask = poison_proc_mask(sim, player, IMBUES.wound);
            effects.push(json!({
                "kind": "wound_poison", "trigger_aura": "Wound Poison",
                "spell_id": sim.spell(spell).action_id.spell_id,
                "proc_mask": mask.names(), "proc_chance": 0.3 + poison_bonus(self),
                "debuff_aura": target_aura_label(&self.auras.wound_poison_debuff)
                    .unwrap_or_default(),
            }));
        }
        // talents_assassination.go registerVenom: a finisher whose aura, on the Slice and Dice
        // ladder without Improved Slice and Dice, raises poison damage and the poison chance.
        // Under Forever's rules there is no finisher: Shank restarts the 6 sec aura (30% poison
        // damage, 20% poison chance), so every duration is the same.
        if let Some(aura) = self.auras.venom {
            let row = data.venom.highest();
            let forever = env.rules == Rules::Forever;
            let durations: Vec<i64> = if forever {
                vec![rework::VENOM_DURATION; SLICE_AND_DICE_DURATIONS.len()]
            } else {
                SLICE_AND_DICE_DURATIONS.to_vec()
            };
            let (damage_bonus, chance_bonus) = if forever {
                (rework::VENOM_DAMAGE_BONUS, rework::VENOM_CHANCE_BONUS)
            } else {
                (
                    row.effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_DAMAGE)
                        .average(level)
                        / 100.0,
                    row.effect(
                        dbcenums::A_ADD_FLAT_MODIFIER,
                        dbcenums::SPELLMOD_CHANCE_OF_SUCCESS,
                    )
                    .average(level)
                        / 100.0,
                )
            };
            effects.push(json!({
                "kind": "venom", "spell_id": row.id, "aura": sim.aura(aura).label,
                "durations_ns": durations,
                "damage_bonus": damage_bonus, "chance_bonus": chance_bonus,
                "class_spells": mask_names(masks::POISONS),
            }));
        }
        // talents_subtlety.go registerGhostlyStrike: main hand weapon damage, its weapon share in
        // the spell's multiplier, and a dodge buff, a stat aura when the target tanks the player.
        if let Some(spell) = self.spells.ghostly_strike {
            let dodge = sim
                .get_aura(player, "Ghostly Strike Buff")
                .map(|aura| sim.aura(aura).label.clone())
                .unwrap_or_default();
            let mut ghostly = json!({
                "kind": "ghostly_strike", "spell_id": sim.spell(spell).action_id.spell_id,
                "dodge_aura": dodge,
            });
            // Forever's rules (push 113483): the row states normalized weapon damage, and the
            // buff of 1329326 lets the next Ambush or Garrote out of Stealth.
            if env.rules == Rules::Forever {
                ghostly["normalized"] = json!(true);
                if let Some(ready) = self.auras.ghostly_ready {
                    ghostly["ready_aura"] = json!(sim.aura(ready).label);
                }
            }
            effects.push(ghostly);
        }
        // talents_subtlety.go registerHemorrhage: normalized main hand damage and its debuff.
        if let Some(spell) = self.spells.hemorrhage {
            effects.push(json!({
                "kind": "hemorrhage", "spell_id": sim.spell(spell).action_id.spell_id,
                "debuff_aura": target_aura_label(&self.auras.hemorrhage).unwrap_or_default(),
            }));
        }
        // garrote.go: from Stealth, behind the target unless Dirty Deeds, a bleed of the tick and
        // 3% of attack power, a Go literal, read again at each tick.
        if self.spells.garrote.is_some() {
            let row = data.garrote.highest();
            let mut garrote = json!({
                "kind": "garrote", "spell_id": row.id,
                "tick_damage": row.periodic_effect().average(level),
                "attack_power_share": 0.03, "tick_can_crit": row.periodic_can_crit(),
                "magic": row.defense_type_core() == DefenseType::Magic,
                "dirty_deeds": talents.i32("dirty_deeds") > 0,
            });
            // Ghostly Strike's buff (Forever's rules) lets Garrote out of Stealth.
            if let Some(ready) = self.auras.ghostly_ready {
                garrote["ghostly_aura"] = json!(sim.aura(ready).label);
            }
            effects.push(garrote);
        }
        // kidney_shot.go: a finisher whose stun lasts a second and a second a combo point, scaled
        // by the target's stun duration multiplier, and raises the rogue's damage on the target by
        // Improved Kidney Shot. A stun immune target ignores it. A stun also pauses the target's
        // swings, which only matter when it tanks the player.
        if let Some(spell) = sim.get_spell(player, &spell_action(8643)) {
            let spell_id = sim.spell(spell).action_id.clone();
            let stun = sim
                .unit(target)
                .auras
                .iter()
                .map(|aura| sim.aura(*aura))
                .filter(|aura| aura.action_id.as_ref() == Some(&spell_id) && aura.tag == "Stun")
                .map(|aura| aura.label.clone())
                .next_back()
                .unwrap_or_default();
            let pseudo = &sim.unit(target).pseudo_stats;
            effects.push(json!({
                "kind": "kidney_shot", "spell_id": spell_id.spell_id,
                "target_stun_immune": pseudo.stun_immune, "stun_aura": stun,
                "base_duration_ns": data.kidney_shot.highest().duration(),
                "duration_per_combo_point_ns": SECOND,
                "stun_duration_multiplier": pseudo.stun_duration_multiplier,
                "damage_taken_multiplier": 1.0
                    + data.improved_kidney_shot.value_at(talents.i32("improved_kidney_shot")) / 100.0,
                "target_tanks_player": env.tanking(),
            }));
        }
        // The notes of 2 November 2026 (Forever's rules): Shank, an off hand strike for 50% weapon
        // damage that generates Energy (Flash of Steel adds to it), which Boot Knife makes add
        // two Combo Points and Venom makes restart its aura; and Poised, which takes 10 Energy
        // (Composure adds 5 or 10) off the first Energy ability of the fight.
        if let Some(spell) = self.spells.shank {
            let mut shank = json!({
                "kind": "shank", "spell_id": sim.spell(spell).action_id.spell_id,
                "energy_gain":
                    SHANK_ENERGY_GAIN + rework::flash_of_steel_energy(self.extra_points("flash_of_steel")),
            });
            let boot_knife = rework::boot_knife_chance(self.extra_points("boot_knife"));
            if boot_knife > 0.0 {
                shank["boot_knife_chance"] = json!(boot_knife);
                shank["boot_knife_combo_points"] = json!(rework::BOOT_KNIFE_COMBO_POINTS);
                shank["boot_knife_action"] = action(rework::BOOT_KNIFE_TRIGGERED_SPELL_ID);
            }
            effects.push(shank);
        }
        if let (Some(_), Some(aura)) = (self.spells.death_mark, self.auras.death_mark) {
            // Coup de Grace raises the stored share and the cap of Death Mark as it does
            // Eviscerate's damage (14162's second effect).
            let coup = 1.0
                + rework::talent_ladder(env.rules, 14162, 3, 3)
                    .fraction_at(talents.i32("improved_eviscerate"));
            effects.push(json!({
                "kind": "death_mark", "spell_id": rework::DEATH_MARK_SPELL_ID,
                "aura": sim.aura(aura).label,
                "stored_per_point": rework::DEATH_MARK_STORED_PER_POINT,
                "cap_base": rework::DEATH_MARK_CAP_BASE,
                "cap_per_point": rework::DEATH_MARK_CAP_PER_POINT,
                "attack_power_per_point": rework::DEATH_MARK_ATTACK_POWER_PER_POINT,
                "coup_de_grace": coup,
                "doubled_class_spells": mask_names(masks::GARROTE | masks::RUPTURE),
            }));
        }
        if let Some(aura) = self.auras.poised {
            effects.push(json!({
                "kind": "poised", "aura": sim.aura(aura).label,
                "cost_reduction": self.poised_cost_reduction(),
                "class_spells": mask_names(masks::ENERGY_ABILITIES),
            }));
        }
        effects.extend(self.expose_armor_effects(env, unrepresented));
        // talents_subtlety.go registerQuietus: an execute phase callback that raises Sinister
        // Strike, Ghostly Strike and Hemorrhage once the target reaches 35%.
        if let Some(aura) = sim.get_aura(player, "Quietus") {
            effects.push(json!({
                "kind": "quietus", "aura": sim.aura(aura).label, "execute_phase": 35,
                "damage_bonus": data.quietus.effect_at(1).fraction_at(talents.i32("quietus")),
                "class_spells": mask_names(masks::QUIETUS),
            }));
        }
        // talents_combat.go registerRiposte: its trigger hears parries the player makes, and
        // nothing attacks the player unless a target tanks it. A parry readies Riposte, a main
        // hand weapon strike that spends the ready aura. Forever's rules describe it without a
        // tank too, where the trigger never fires and Riposte is never ready, so a rotation
        // that names it never casts it, as in Go.
        if let Some(aura) = sim.get_aura(player, "Riposte Trigger") {
            let label = sim.aura(aura).label.clone();
            if env.tanking() || sim.rules == crate::prepare::sim::Rules::Forever {
                let ready = sim
                    .get_aura(player, "Riposte Ready")
                    .map(|aura| sim.aura(aura).label.clone())
                    .unwrap_or_default();
                effects.push(json!({
                    "kind": "riposte", "spell_id": data.riposte.highest().id,
                    "trigger_aura": label, "ready_aura": ready,
                }));
            } else {
                effects.push(json!({
                    "kind": "inert_listener", "unit": "player", "aura": label,
                    "reason": "acts only on attacks the player parries, and nothing attacks the player",
                }));
            }
        }
        // talents_subtlety.go registerThousandCuts: Rupture ticks stack a discount on the next
        // Backstab or Hemorrhage, which spends it.
        if let Some(aura) = self.auras.thousand_cuts {
            let buff = data.thousand_cuts_triggered.highest();
            let trigger = ProcTrigger {
                class_spell_mask: masks::RUPTURE,
                ..ProcTrigger::default()
            };
            let mut effect = rogue_proc(
                env,
                "Thousand Cuts Trigger",
                "stack",
                1.0,
                &trigger,
                "any",
                true,
            );
            effect["aura"] = json!(sim.aura(aura).label);
            effects.push(effect);
            effects.push(json!({
                "kind": "thousand_cuts", "aura": sim.aura(aura).label,
                "cost_per_stack": buff
                    .effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_COST)
                    .average(level) as i32,
                "class_spells": mask_names(masks::BACKSTAB | masks::HEMORRHAGE),
            }));
        }
        effects
    }
}
