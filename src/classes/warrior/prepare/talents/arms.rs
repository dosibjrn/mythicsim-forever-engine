//! Go sim/warrior/talents_arms.go.

use std::rc::Rc;

use crate::prepare::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use crate::prepare::character::cooldown_type;
use crate::prepare::dbcenums;
use crate::prepare::env::Environment;
use crate::prepare::sim::{AuraConfig, Sim, UnitId, NEVER_EXPIRES};
use crate::prepare::spell::{
    school, DefenseType, ProcMask, SpellConfig, SpellFlag, SpellFlag as F,
};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};

use super::super::helpers::*;
use super::super::masks;
use super::super::spell_data::spell_data;
use super::super::spells::add_cooldown;
use super::super::Warrior;
use super::millis;

impl Warrior {
    /// Go `registerArmsTalents`.
    pub(in super::super) fn register_arms_talents(&self, sim: &mut Sim, unit: UnitId) {
        // Tier 1
        self.register_improved_heroic_strike(sim, unit);
        self.register_deflection(sim, unit);
        self.register_improved_rend(sim, unit);

        // Tier 2
        // Improved Charge: charge.go. Improved Tactical Mastery: stances.go.
        self.register_improved_overpower(sim, unit);

        // Tier 3
        self.register_anger_management(sim, unit);
        self.register_deep_wounds(sim, unit);

        // Tier 4
        self.register_spearing_strike(sim, unit);
        self.register_two_handed_weapon_specialization(sim, unit);
        self.register_impale(sim, unit);

        // Tier 5
        self.register_bloodthrill(sim, unit);
        self.register_sweeping_strikes(sim, unit);
        self.register_weaponmaster(sim, unit);

        // Tier 6
        self.register_improved_slam(sim, unit);
        self.register_improved_hamstring(sim, unit);

        // Tier 7
        self.register_mortal_strike(sim, unit);
    }

    fn register_improved_heroic_strike(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("improved_heroic_strike");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::HEROIC_STRIKE,
                kind: SpellModType::PowerCostFlat,
                int_value: spell_data().improved_heroic_strike.tenths_at(rank) as i32,
                ..SpellModConfig::default()
            },
        );
    }

    fn register_deflection(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("deflection");
        if rank == 0 {
            return;
        }
        sim.unit_mut(unit).pseudo_stats.base_parry_chance +=
            spell_data().deflection.fraction_at(rank);
    }

    fn register_improved_rend(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("improved_rend");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::REND,
                kind: SpellModType::DamageDoneFlat,
                float_value: spell_data().improved_rend.fraction_at(rank),
                ..SpellModConfig::default()
            },
        );
    }

    fn register_improved_overpower(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("improved_overpower");
        if rank == 0 {
            return;
        }
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Improved Overpower".to_string(),
                action_id: Some(with_tag(&spell_action(12963), rank)),
                ..AuraConfig::default()
            },
        );
        sim.make_permanent(aura);
        sim.attach_spell_mod(
            aura,
            SpellModConfig {
                class_mask: masks::OVERPOWER,
                kind: SpellModType::BonusCritPercent,
                float_value: spell_data().improved_overpower.value_at(rank),
                ..SpellModConfig::default()
            },
        );
    }

    fn register_anger_management(&self, sim: &mut Sim, unit: UnitId) {
        if !self.has_talent("anger_management") {
            return;
        }
        // The rage a period gives only runs in a fight.
        sim.register_reset_effect(unit, Rc::new(|_: &mut Sim| {}));
    }

    fn register_deep_wounds(&self, sim: &mut Sim, unit: UnitId) {
        if self.talent("deep_wounds") == 0 {
            return;
        }
        let bleed = spell_data().deep_wounds_triggered.by_id(412609);
        let tick = bleed.effect_n(1);
        let tick_length = tick.period();
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(bleed.id),
                spell_school: school::PHYSICAL,
                proc_mask: ProcMask::EMPTY,
                class_spell_mask: masks::DEEP_WOUNDS,
                flags: F::NO_ON_CAST_COMPLETE
                    | F::IGNORE_RESISTS
                    | F::PROC
                    | F::IGNORE_ATTACKER_MODIFIERS,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                dot: crate::prepare::spell::DotConfig {
                    aura: AuraConfig {
                        label: "DeepWounds".to_string(),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: (bleed.duration() / tick_length) as i32,
                    tick_length,
                    ..Default::default()
                },
                ..SpellConfig::default()
            },
        );
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Deep Wounds - Trigger".to_string(),
                trigger_immediately: true,
                proc_mask_exclude: ProcMask::EMPTY,
                outcome: HitOutcome::CRIT,
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                ..ProcTrigger::default()
            },
        );
    }

    fn register_two_handed_weapon_specialization(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("two_handed_weapon_specialization");
        if rank == 0 {
            return;
        }
        // The effect is all physical damage, auto attacks included, so no mask narrows it.
        let weapon_mod = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                school: school::PHYSICAL,
                kind: SpellModType::DamageDonePct,
                float_value: crate::prepare::hotfix_113554::talent(sim.rules, 12163, 3)
                    .effect(dbcenums::A_MOD_DAMAGE_PERCENT_DONE, 1)
                    .fraction_at(rank),
                ..SpellModConfig::default()
            },
        );
        if self.main_hand_is_two_hand(sim, unit) {
            sim.activate_spell_mod(weapon_mod);
        }
        sim.register_item_swap_callback(
            unit,
            &[
                crate::prepare::items::slot::MAIN_HAND,
                crate::prepare::items::slot::OFF_HAND,
            ],
            crate::prepare::item_swap::SwapCallback::HandMod {
                id: weapon_mod,
                hand_type: "HandTypeTwoHand".to_string(),
            },
        );
    }

    /// Go `GetMainHandType() == HandTypeTwoHand`.
    fn main_hand_is_two_hand(&self, sim: &Sim, unit: UnitId) -> bool {
        sim.mh_weapon(unit)
            .is_some_and(|item| item.hand_type == "HandTypeTwoHand")
    }

    fn register_impale(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("impale");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::DAMAGE_SPELLS,
                kind: SpellModType::CritMultiplierFlat,
                float_value: spell_data().impale.fraction_at(rank),
                ..SpellModConfig::default()
            },
        );
    }

    fn register_mortal_strike(&self, sim: &mut Sim, unit: UnitId) {
        if !self.has_talent("mortal_strike") {
            return;
        }
        let rank = spell_data().mortal_strike.highest();
        let cd = new_cooldown(sim, unit, cooldown_of(rank));
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                flags: F::APL | F::MELEE_METRICS,
                class_spell_mask: masks::MORTAL_STRIKE,
                max_range: MELEE_RANGE,
                cost: rage_cost_with_refund(rank),
                cast: cast_config(rank.gcd(), true, cd),
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        );
    }

    fn register_spearing_strike(&self, sim: &mut Sim, unit: UnitId) {
        if !self.has_talent("spearing_strike") {
            return;
        }
        let rank = spell_data().spearing_strike.highest();
        let cd = new_cooldown(sim, unit, cooldown_of(rank));
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                flags: F::APL | F::MELEE_METRICS,
                class_spell_mask: masks::SPEARING_STRIKE,
                max_range: f64::from(rank.max_range),
                cost: rage_cost_with_refund(rank),
                cast: cast_config(rank.gcd(), true, cd),
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                has_extra_cast_condition: true,
                ..SpellConfig::default()
            },
        );
    }

    fn register_bloodthrill(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("bloodthrill");
        if rank == 0 {
            return;
        }
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Bloodthrill - Trigger".to_string(),
                action_id: spell_action(1289682),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                proc_mask: ProcMask::MELEE_MH,
                outcome: HitOutcome::LANDED,
                proc_chance: super::super::trees_20261104::bloodthrill(sim.rules).fraction_at(rank),
                ..ProcTrigger::default()
            },
        );
    }

    fn register_weaponmaster(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("weaponmaster");
        if rank == 0 {
            return;
        }
        let data = spell_data();
        let action = spell_action(1290261);

        let weapon_type = |sim: &Sim, slot: usize| -> String {
            sim.character(unit).equipment[slot].weapon_type.clone()
        };
        let in_hand = |sim: &Sim, slot: usize, types: &[&str]| {
            let item = &sim.character(unit).equipment[slot];
            !item.is_empty() && types.contains(&weapon_type(sim, slot).as_str())
        };
        use crate::prepare::items::slot;
        let types_mask = |sim: &Sim, types: &[&str]| -> ProcMask {
            let mut mask = ProcMask::UNKNOWN;
            if in_hand(sim, slot::RANGED, types) {
                mask = mask | ProcMask::RANGED;
            }
            if in_hand(sim, slot::MAIN_HAND, types) {
                mask = mask | ProcMask::MELEE_MH;
            }
            if in_hand(sim, slot::OFF_HAND, types) {
                mask = mask | ProcMask::MELEE_OH;
            }
            mask
        };
        let crit_mask = types_mask(sim, &["WeaponTypeAxe", "WeaponTypePolearm"]);
        let armor_ignore_on =
            types_mask(sim, &["WeaponTypeMace", "WeaponTypeStaff"]).matches(ProcMask::MELEE_MH);

        // The crit goes with the weapon: an axe in one hand crits more with that hand only.
        let crit = data.weaponmaster.effect_at(1).value_at(rank);
        let mut crit_mods = Vec::new();
        for hand in [ProcMask::MELEE_MH, ProcMask::MELEE_OH] {
            let modifier = sim.add_dynamic_mod(
                unit,
                SpellModConfig {
                    kind: SpellModType::BonusCritPercent,
                    proc_mask: hand,
                    float_value: crit,
                    ..SpellModConfig::default()
                },
            );
            crit_mods.push((hand, modifier));
        }
        for (hand, modifier) in &crit_mods {
            if crit_mask.matches(*hand) {
                sim.activate_spell_mod(*modifier);
            } else {
                sim.deactivate_spell_mod(*modifier);
            }
        }

        let armor_ignore_aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Weaponmaster (Mace/Staff)".to_string(),
                action_id: Some(with_tag(&action, 2)),
                duration: NEVER_EXPIRES,
                on_gain: Some(noop()),
                on_expire: Some(noop()),
                ..AuraConfig::default()
            },
        );
        if armor_ignore_on {
            sim.make_permanent(armor_ignore_aura);
            // The permanent aura's gain adds the armor ignore to every attack table the warrior
            // owns. Rust keeps the tables in the environment, where nothing else changes a
            // warrior's armor ignore between finalizing and the reset, so it is added once the
            // tables exist.
            let armor_ignore = data.weaponmaster.effect_at(2).fraction_at(rank);
            self.post_finalize
                .borrow_mut()
                .push(std::rc::Rc::new(move |env: &mut Environment| {
                    let attacker = env.sim.unit(unit).unit_index as usize;
                    for table in &mut env.attack_tables[attacker] {
                        table.armor_ignore_factor += armor_ignore;
                    }
                }));
        }

        // readWeapons, setCritMods and the armor ignore aura follow the weapons at a swap.
        sim.register_item_swap_callback(
            unit,
            &[slot::MAIN_HAND, slot::OFF_HAND],
            crate::prepare::item_swap::SwapCallback::Weaponmaster {
                armor_aura: armor_ignore_aura,
                armor_ignore: data.weaponmaster.effect_at(2).fraction_at(rank),
                mh_crit: crit_mods[0].1,
                oh_crit: crit_mods[1].1,
            },
        );

        let sword_aura = sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Weaponmaster (Sword)".to_string(),
                action_id: with_tag(&action, 3),
                metrics_action_id: with_tag(&action, 3),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                proc_mask: ProcMask::MELEE,
                outcome: HitOutcome::LANDED,
                proc_chance: data.weaponmaster.effect_at(3).fraction_at(rank),
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );
        // The extra attack is the main hand swing's config under the talent's tag, registered
        // when the aura initializes.
        sim.apply_on_init(
            sword_aura,
            Rc::new(move |sim: &mut Sim, _| {
                let Some(mut config) = sim.unit(unit).auto_attacks.mh_config.clone() else {
                    return;
                };
                config.action_id = with_tag(&config.action_id, 1290261);
                sim.get_or_register_spell(unit, config);
            }),
        );
    }

    fn register_improved_hamstring(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("improved_hamstring");
        if rank == 0 {
            return;
        }
        let root = spell_data().improved_hamstring_triggered.highest();
        let label = format!("Improved Hamstring-{}", sim.unit(unit).label);
        new_enemy_aura_array(sim, |sim, target| {
            sim.get_or_register_aura(
                target,
                AuraConfig {
                    label: label.clone(),
                    action_id: Some(spell_action(root.id)),
                    duration: root.duration(),
                    ..AuraConfig::default()
                },
            )
        });
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Improved Hamstring - Trigger".to_string(),
                action_id: spell_action(12289),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                class_spell_mask: masks::HAMSTRING,
                outcome: HitOutcome::LANDED,
                proc_chance: spell_data().improved_hamstring.fraction_at(rank),
                ..ProcTrigger::default()
            },
        );
    }

    fn register_improved_slam(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talent("improved_slam");
        if rank == 0 {
            return;
        }
        let ladder = &crate::prepare::hotfix_113554::talent(sim.rules, 12862, 2);
        for (kind, op) in [
            (SpellModType::CastTimeFlat, dbcenums::SPELLMOD_CASTING_TIME),
            (
                SpellModType::GlobalCooldownFlat,
                dbcenums::SPELLMOD_GLOBAL_COOLDOWN,
            ),
            (SpellModType::CooldownFlat, dbcenums::SPELLMOD_COOLDOWN),
        ] {
            sim.add_static_mod(
                unit,
                SpellModConfig {
                    class_mask: masks::SLAM,
                    kind,
                    time_value: millis(
                        ladder
                            .effect(dbcenums::A_ADD_FLAT_MODIFIER, op)
                            .value_at(rank),
                    ),
                    ..SpellModConfig::default()
                },
            );
        }
    }

    fn register_sweeping_strikes(&self, sim: &mut Sim, unit: UnitId) {
        if !self.has_talent("sweeping_strikes") {
            return;
        }
        let rank = spell_data().sweeping_strikes.highest();
        let action = spell_action(12723);

        sim.register_spell(
            unit,
            SpellConfig {
                action_id: action.clone(),
                class_spell_mask: masks::SWEEPING_STRIKES_HIT,
                spell_school: school::PHYSICAL,
                proc_mask: ProcMask::MELEE_SPECIAL,
                flags: SpellFlag::IGNORE_MODIFIERS
                    | F::MELEE_METRICS
                    | F::PASSIVE_SPELL
                    | F::NO_ON_CAST_COMPLETE,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        );
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: with_tag(&action, 1),
                class_spell_mask: masks::SWEEPING_STRIKES_NORMALIZED_HIT,
                spell_school: school::PHYSICAL,
                defense_type: DefenseType::Melee,
                proc_mask: ProcMask::MELEE_SPECIAL,
                flags: F::MELEE_METRICS | F::PASSIVE_SPELL | F::NO_ON_CAST_COMPLETE,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        );
        let aura = sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Sweeping Strikes".to_string(),
                action_id: action.clone(),
                metrics_action_id: action.clone(),
                duration: rank.duration(),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                proc_mask: ProcMask::MELEE,
                outcome: HitOutcome::LANDED,
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );
        sim.aura_mut(aura).max_stacks = i32::from(rank.proc_charges);

        let cd = new_cooldown(sim, unit, cooldown_of(rank));
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id: action,
                class_spell_mask: masks::SWEEPING_STRIKES,
                spell_school: school::PHYSICAL,
                cost: rage_cost(rank),
                cast: crate::prepare::spell::CastConfig {
                    cd,
                    ..Default::default()
                },
                has_extra_cast_condition: true,
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
        add_cooldown(sim, unit, spell, cooldown_type::DPS);
    }
}
