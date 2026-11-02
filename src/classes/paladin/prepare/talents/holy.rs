//! Go `sim/paladin/talents_holy.go`.

use std::rc::Rc;

use crate::prepare::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use crate::prepare::sim::{AuraConfig, Rules, Sim, UnitId};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::stats::Stat;

use super::super::hotfix_113554 as hotfix;
use super::super::masks;
use super::super::spell_data::spell_data;
use super::super::util::{new_enemy_aura_array, spell_action};
use super::super::Paladin;
use super::millis;

impl Paladin {
    /// Go `registerHolyTalents`.
    pub(in super::super) fn register_holy_talents(&mut self, sim: &mut Sim, unit: UnitId) {
        // Tier 1
        self.apply_divine_strength(sim, unit);
        self.apply_divine_intellect(sim, unit);

        // Tier 2
        self.apply_healing_light(sim, unit);
        self.apply_spiritual_focus(sim, unit);
        self.apply_improved_seals(sim, unit);
        // Unyielding Faith shortens Fear and Disorient effects, which the sim never suffers.

        // Tier 3
        // Voice of Truth grants immunity to Silence and Interrupt effects, which the sim never
        // suffers.
        self.apply_reverence(sim, unit);
        self.apply_purifying_power(sim, unit);

        // Tier 4
        self.apply_infusion_of_light(sim, unit);
        self.apply_illumination(sim, unit);
        // Divine Favor registered in registerTalentSpells

        // Tier 5
        self.apply_divine_precision(sim, unit);
        // Holy Shock registered in registerTalentSpells
        self.apply_consecrated_ground(sim, unit);

        // Push 113554, under Forever's rules. Oath of Light is a spell the sim does not register,
        // so no ally carries it and Boundless Light holds on every Holy Shock heal.
        self.apply_boundless_light(sim, unit);

        // Tier 6
        self.apply_holy_power(sim, unit);

        // Tier 7
        // Light's Vigil registered in registerTalentSpells
    }

    /// Divine Strength: increases your Strength by 2/4/6/8/10%.
    fn apply_divine_strength(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("divine_strength");
        if points == 0 {
            return;
        }
        sim.unit_mut(unit).sdm.multiply_stat(
            Stat::Strength,
            spell_data().divine_strength.multiplier_at(points),
        );
    }

    /// Divine Intellect: increases your total Intellect by 2/4/6/8/10%.
    fn apply_divine_intellect(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("divine_intellect");
        if points == 0 {
            return;
        }
        sim.unit_mut(unit).sdm.multiply_stat(
            Stat::Intellect,
            spell_data().divine_intellect.multiplier_at(points),
        );
    }

    /// Healing Light: increases the amount healed by your Holy Light, Flash of Light, and
    /// Holy Shock spells by 4/8/12%.
    fn apply_healing_light(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("healing_light");
        if points == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::HEALING_SPELLS,
                kind: SpellModType::DamageDoneFlat,
                float_value: spell_data().healing_light.fraction_at(points),
                ..SpellModConfig::default()
            },
        );
    }

    /// Spiritual Focus: gives your Flash of Light, Holy Light, and Light's Vigil spells a
    /// chance to not lose casting time when you take damage.
    fn apply_spiritual_focus(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("spiritual_focus");
        if points == 0 {
            return;
        }
        // 20205 names these by class mask; Holy Wrath, Exorcism and the rest are pushed back
        // as usual.
        // Five ranks of 14% since push 113554 under Forever's rules (was 35/70).
        let resist = match sim.rules {
            Rules::Forever => hotfix::at(&hotfix::SPIRITUAL_FOCUS, points) / 100.0,
            Rules::GoReference => spell_data().spiritual_focus.fraction_at(points),
        };
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::FLASH_OF_LIGHT | masks::HOLY_LIGHT | masks::LIGHTS_VIGIL,
                kind: SpellModType::Custom,
                apply_custom: Some(Rc::new(move |sim: &mut Sim, _, spell| {
                    sim.spell_mut(spell).pushback_resist += resist;
                })),
                remove_custom: Some(Rc::new(move |sim: &mut Sim, _, spell| {
                    sim.spell_mut(spell).pushback_resist -= resist;
                })),
                ..SpellModConfig::default()
            },
        );
    }

    /// Improved Seals: increases the damage done by your Seals and Judgements by 5/10/15%.
    fn apply_improved_seals(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("improved_seals");
        if points == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::SEAL_OF_RIGHTEOUSNESS_PROC
                    | masks::SEAL_OF_FURY_PROC
                    | masks::JUDGEMENT_OF_RIGHTEOUSNESS
                    | masks::JUDGEMENT_OF_COMMAND
                    | masks::JUDGEMENT_OF_FURY,
                kind: SpellModType::DamageDoneFlat,
                float_value: Self::improved_seals_bonus(sim, points),
                ..SpellModConfig::default()
            },
        );
    }

    /// Improved Seals' bonus at its points: five ranks of 3% since push 113554 under Forever's
    /// rules, three of 5% in the reference.
    pub(in super::super) fn improved_seals_bonus(sim: &Sim, points: i32) -> f64 {
        match sim.rules {
            Rules::Forever => hotfix::at(&hotfix::IMPROVED_SEALS, points) / 100.0,
            Rules::GoReference => spell_data().improved_seals.fraction_at(points),
        }
    }

    /// Reverence: allows 10/20/30% of your Mana regeneration to continue while casting.
    fn apply_reverence(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("reverence");
        if points == 0 {
            return;
        }
        // Two ranks of 15% since push 113554 under Forever's rules (was 10/20/30).
        sim.unit_mut(unit).pseudo_stats.spirit_regen_rate_casting += match sim.rules {
            Rules::Forever => hotfix::at(&hotfix::REVERENCE, points) / 100.0,
            Rules::GoReference => spell_data().reverence.fraction_at(points),
        };
    }

    /// Purifying Power: reduces the cooldown of your Exorcism and Holy Wrath spells by 17/33%.
    fn apply_purifying_power(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("purifying_power");
        if points == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::EXORCISM | masks::HOLY_WRATH,
                kind: SpellModType::CooldownMultiplier,
                float_value: spell_data()
                    .purifying_power
                    .effect_at(2)
                    .multiplier_at(points),
                ..SpellModConfig::default()
            },
        );
    }

    /// Infusion of Light: your Holy Shock and Flash of Light critical hits reduce the cast
    /// time of your next Holy Light cast within 15 sec.
    fn apply_infusion_of_light(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("infusion_of_light");
        if points == 0 {
            return;
        }
        let rank = spell_data().infusion_of_light_triggered.highest();
        let label = sim.unit(unit).label.clone();

        let infusion = sim.register_aura(
            unit,
            AuraConfig {
                label: format!("Infusion of Light{label}"),
                action_id: Some(spell_action(rank.id)),
                duration: rank.duration(),
                ..AuraConfig::default()
            },
        );
        sim.attach_spell_mod(
            infusion,
            SpellModConfig {
                class_mask: masks::HOLY_LIGHT,
                kind: SpellModType::CastTimeFlat,
                time_value: millis(spell_data().infusion_of_light.value_at(points)),
                ..SpellModConfig::default()
            },
        );
        sim.attach_proc_trigger(
            infusion,
            &ProcTrigger {
                callback: CallbackMask::ON_CAST_COMPLETE,
                class_spell_mask: masks::HOLY_LIGHT,
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );

        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: format!("Infusion of Light - Trigger{label}"),
                callback: CallbackMask::ON_SPELL_HIT_DEALT | CallbackMask::ON_HEAL_DEALT,
                class_spell_mask: masks::HOLY_SHOCK
                    | masks::HOLY_SHOCK_HEAL
                    | masks::FLASH_OF_LIGHT,
                outcome: HitOutcome::CRIT,
                ..ProcTrigger::default()
            },
        );
    }

    /// Illumination: after getting a critical effect from your heal spells you have a chance
    /// to gain Mana equal to a share of the base cost of the spell.
    fn apply_illumination(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("illumination");
        if points == 0 {
            return;
        }
        let label = sim.unit(unit).label.clone();
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: format!("Illumination{label}"),
                callback: CallbackMask::ON_HEAL_DEALT,
                class_spell_mask: masks::HEALING_SPELLS,
                outcome: HitOutcome::CRIT,
                proc_chance: spell_data().illumination.effect_at(1).fraction_at(points),
                ..ProcTrigger::default()
            },
        );
    }

    /// Divine Precision: improves your chance to hit with Holy spells by 6/12/18%.
    fn apply_divine_precision(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("divine_precision");
        if points == 0 {
            return;
        }
        // 1310904 is a miss chance mod on a class mask, not school hit: the Holy Shield proc
        // is left out and Holy Strike, a melee attack, is in.
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::DIVINE_PRECISION,
                kind: SpellModType::BonusHitPercent,
                // Two ranks of 4% since push 113554 under Forever's rules (was 6/12/18).
                float_value: match sim.rules {
                    Rules::Forever => hotfix::at(&hotfix::DIVINE_PRECISION, points),
                    Rules::GoReference => spell_data().divine_precision.value_at(points),
                },
                ..SpellModConfig::default()
            },
        );
    }

    /// Consecrated Ground: gives your Holy spells increased damage against the first 4
    /// enemies that enter your Consecration.
    fn apply_consecrated_ground(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("consecrated_ground");
        if points == 0 {
            return;
        }
        let rank = spell_data().consecrated_ground.highest();
        let label = sim.unit(unit).label.clone();

        let auras = new_enemy_aura_array(sim, |sim, target| {
            let aura = sim.get_or_register_aura(
                target,
                AuraConfig {
                    label: format!("Consecrated Ground{label}"),
                    action_id: Some(spell_action(rank.id)),
                    duration: spell_data()
                        .consecrated_ground_triggered
                        .highest()
                        .duration(),
                    ..AuraConfig::default()
                },
            );
            sim.attach_ddbc(aura, 0, 1, unit)
        });
        self.consecrated_ground_auras = Some(auras);
    }

    /// Holy Power: increases the critical strike chance of your Holy Shock and Holy Strike
    /// abilities, and of your other Holy damage and healing spells.
    fn apply_holy_power(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.talents.i32("holy_power");
        if points == 0 {
            return;
        }
        if sim.rules == Rules::Forever {
            // Push 113554 turns it into A_MOD_CRITICAL_HEALING_AMOUNT on the Holy school: the
            // heal crit's bonus grows by 10% a rank, so 5/5 makes a 150% heal crit 225%. Every
            // paladin heal is Holy. Upstream #772 puts it on the heals as a crit multiplier.
            sim.add_static_mod(
                unit,
                SpellModConfig {
                    // Holy Power's mask names Light's Vigil, whose party heal is the third.
                    class_mask: masks::HEALING_SPELLS | masks::LIGHTS_VIGIL_HEAL,
                    kind: SpellModType::CritMultiplierPct,
                    float_value: hotfix::at(&hotfix::HOLY_POWER, points) / 100.0,
                    ..SpellModConfig::default()
                },
            );
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::CONSECRATION
                    | masks::EXORCISM
                    | masks::HAMMER_OF_WRATH
                    | masks::HOLY_WRATH
                    | masks::HOLY_LIGHT
                    | masks::FLASH_OF_LIGHT
                    | masks::LAY_ON_HANDS
                    | masks::LIGHTS_VIGIL
                    | masks::LIGHTS_VIGIL_STRIKE
                    | masks::SEAL_OF_RIGHTEOUSNESS_PROC
                    | masks::SEAL_OF_COMMAND_PROC
                    | masks::SEAL_OF_FURY_PROC
                    | masks::JUDGEMENT_OF_COMMAND
                    | masks::JUDGEMENT_OF_FURY
                    | masks::JUDGEMENT_OF_RIGHTEOUSNESS,
                kind: SpellModType::BonusCritPercent,
                float_value: spell_data().holy_power.effect_at(1).value_at(points),
                ..SpellModConfig::default()
            },
        );
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::HOLY_SHOCK | masks::HOLY_SHOCK_HEAL | masks::HOLY_STRIKE,
                kind: SpellModType::BonusCritPercent,
                float_value: spell_data().holy_power.effect_at(2).value_at(points),
                ..SpellModConfig::default()
            },
        );
    }

    /// Boundless Light (1328550, push 113554): increases the critical strike chance of your Holy
    /// Shock heal by 15/30% on an ally without your Oath of Light.
    fn apply_boundless_light(&mut self, sim: &mut Sim, unit: UnitId) {
        let points = self.new_talent("boundless_light");
        if points == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::HOLY_SHOCK_HEAL,
                kind: SpellModType::BonusCritPercent,
                float_value: hotfix::at(&hotfix::BOUNDLESS_LIGHT, points),
                ..SpellModConfig::default()
            },
        );
    }
}
