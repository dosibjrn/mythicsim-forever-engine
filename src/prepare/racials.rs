//! Go sim/core/racials.go.

use std::rc::Rc;

use crate::contracts::prepared_v2::ActionId;

use super::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger, PseudoStatField, StatMultiplier};
use super::character::{cooldown_type, MajorCooldown};
use super::env::Environment;
use super::sim::{AuraConfig, AuraId, Cooldown, Rules, Sim, UnitId, MILLISECOND, SECOND};
use super::spell::{
    school, Cast, CastConfig, DefenseType, ProcMask, Resource, SpellConfig, SpellFlag, GCD_DEFAULT,
};
use super::spell_mod::{SpellModConfig, SpellModType};
use super::stats::{SchoolIndex, Stat, Stats};
use super::Refusal;

const MINUTE: i64 = 60 * SECOND;

/// Go `EurekaSpells`: names, as the class's own spell masks, the abilities the client lists on
/// the three effects of its Eureka! rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EurekaSpells {
    /// Effect 0, SPELLMOD_COST -10%.
    pub cost: i64,
    /// Effect 1, SPELLMOD_DAMAGE +10%: the spells whose direct hits gain the bonus.
    pub damage: i64,
    /// Effect 2, SPELLMOD_DOT +10%: the spells whose dot ticks gain the bonus.
    pub tick: i64,
}

/// Go `applyRaceEffects`. A race with no racial effects registers nothing, as in Go.
pub(crate) fn apply_race_effects(env: &mut Environment) -> Result<(), Refusal> {
    let unit = env.player;
    let race = env.sim.character(unit).race.clone();
    // The 26 October 2026 development notes' racial pass is Forever's own; the Go reference
    // keeps the 16 October racials.
    let oct26 = env.sim.rules == Rules::Forever;
    match race.as_str() {
        "RaceDwarf" => {
            if oct26 {
                // 26 October notes: "Earth Fury (2% increased critical strike damage and healing)
                // removed. Added: Artisan's Precision - Increases hit chance with spells and
                // attacks by 1%."
                add_hit(env, 1.0);
            } else {
                // Earth Fury replaced Mace Specialization: 2% increased critical damage and
                // healing.
                env.sim.unit_mut(unit).pseudo_stats.crit_damage_multiplier *= 1.02;
            }
            apply_creature_type_slaying(env, "MobTypeBeast");

            let action_id = ActionId::spell(20594);
            let sim = &mut env.sim;
            let stone_form = sim.register_aura(
                unit,
                AuraConfig {
                    label: "Stoneform".to_string(),
                    action_id: Some(action_id.clone()),
                    duration: 8 * SECOND,
                    ..AuraConfig::default()
                },
            );
            sim.attach_multiplicative_pseudo_stat_buff(
                stone_form,
                PseudoStatField::SchoolDamageTakenMultiplier(SchoolIndex::Physical),
                0.9,
            );
            let timer = sim.new_timer(unit);
            let spell = sim.register_spell(
                unit,
                SpellConfig {
                    action_id,
                    flags: SpellFlag::NO_ON_CAST_COMPLETE,
                    cast: CastConfig {
                        default_cast: Cast {
                            gcd: GCD_DEFAULT,
                            ..Cast::default()
                        },
                        cd: Cooldown {
                            timer: Some(timer),
                            duration: 3 * MINUTE,
                        },
                        ..CastConfig::default()
                    },
                    related_self_buff: Some(stone_form),
                    ..SpellConfig::default()
                },
            );
            sim.add_major_cooldown(unit, major_cooldown(spell, cooldown_type::SURVIVAL));
        }
        "RaceGnome" => {
            apply_expansive_mind(&mut env.sim, unit);
            apply_eureka(env);
        }
        "RaceHuman" => {
            if oct26 {
                // 26 October notes: "Ruthless now increases critical chance by 1% (was 2%).
                // Human Spirit now increases Spirit by 10% (was 5%)."
                env.sim.unit_mut(unit).sdm.multiply_stat(Stat::Spirit, 1.1);
                add_crit(env, 1.0);
            } else {
                env.sim.unit_mut(unit).sdm.multiply_stat(Stat::Spirit, 1.05);
                // Ruthless replaced Sword Specialization: 2% critical strike chance, any weapon.
                add_crit(env, 2.0);
            }
        }
        "RaceNightElf" => {
            let sim = &mut env.sim;
            sim.unit_mut(unit).pseudo_stats.base_dodge_chance += 0.01;
            sim.unit_mut(unit).pseudo_stats.movement_speed_multiplier *= 1.02;
            let timer = sim.new_timer(unit);
            sim.register_temporary_stats_on_use_cd(
                unit,
                "Elune's Light",
                Stats::from_pairs(&[
                    (Stat::PhysicalCritPercent, 10.0),
                    (Stat::SpellCritPercent, 10.0),
                ]),
                15 * SECOND,
                SpellConfig {
                    action_id: ActionId::spell(1259799),
                    cast: CastConfig {
                        cd: Cooldown {
                            timer: Some(timer),
                            duration: 3 * MINUTE,
                        },
                        ..CastConfig::default()
                    },
                    ..SpellConfig::default()
                },
            );
        }
        "RaceOrc" => {
            if oct26 {
                // 26 October notes: "Savage removed (1% increased critical strike chance).
                // Command re-added, increasing Warlock and Hunter pet damage by 5%."
                apply_command(&mut env.sim, unit);
            } else {
                // Savage replaced Axe Specialization: 1% critical strike chance, any weapon.
                add_crit(env, 1.0);
            }

            let sim = &mut env.sim;
            let blood_fury_id = ActionId::spell(20572);
            let blood_fury_aura = sim.new_temporary_stat_multiplier_aura(
                unit,
                AuraConfig {
                    label: "Blood Fury".to_string(),
                    action_id: Some(blood_fury_id.clone()),
                    duration: 15 * SECOND,
                    ..AuraConfig::default()
                },
                &[
                    StatMultiplier {
                        stat: Stat::AttackPower,
                        multiplier: 1.1,
                    },
                    StatMultiplier {
                        stat: Stat::RangedAttackPower,
                        multiplier: 1.1,
                    },
                    StatMultiplier {
                        stat: Stat::SpellDamage,
                        multiplier: 1.1,
                    },
                    StatMultiplier {
                        stat: Stat::HealingPower,
                        multiplier: 1.1,
                    },
                ],
            );
            // 16 October beta notes: "Blood Fury: Cooldown increased to 3 minutes (was 2 minutes)."
            // The 26 October notes' 27 October additions take it back: "Blood Fury's cooldown
            // reduced to 2 minutes (was 3 minutes)."
            let blood_fury_cd = Cooldown {
                timer: Some(sim.new_timer(unit)),
                duration: if oct26 { 2 * MINUTE } else { 3 * MINUTE },
            };
            sim.aura_mut(blood_fury_aura.aura).icd = Some(blood_fury_cd);
            let blood_fury = sim.register_spell(
                unit,
                SpellConfig {
                    action_id: blood_fury_id,
                    flags: SpellFlag::NO_ON_CAST_COMPLETE,
                    cast: CastConfig {
                        cd: blood_fury_cd,
                        ..CastConfig::default()
                    },
                    related_self_buff: Some(blood_fury_aura.aura),
                    ..SpellConfig::default()
                },
            );
            sim.add_major_cooldown(unit, major_cooldown(blood_fury, cooldown_type::DPS));
            sim.character_mut(unit)
                .cooldown_buff_auras
                .push((blood_fury, blood_fury_aura.clone()));

            let shatter_curse_id = ActionId::spell(1299026);
            let shatter_curse_aura = sim.register_aura(
                unit,
                AuraConfig {
                    label: "Shatter Curse".to_string(),
                    action_id: Some(shatter_curse_id.clone()),
                    duration: 8 * SECOND,
                    ..AuraConfig::default()
                },
            );
            for school in [
                SchoolIndex::Arcane,
                SchoolIndex::Fire,
                SchoolIndex::Frost,
                SchoolIndex::Holy,
                SchoolIndex::Nature,
                SchoolIndex::Shadow,
            ] {
                // 16 October beta notes: "Magic damage taken reduced by 5% (was 15%)."
                sim.attach_multiplicative_pseudo_stat_buff(
                    shatter_curse_aura,
                    PseudoStatField::SchoolDamageTakenMultiplier(school),
                    0.95,
                );
            }
            let timer = sim.new_timer(unit);
            let shatter_curse = sim.register_spell(
                unit,
                SpellConfig {
                    action_id: shatter_curse_id,
                    flags: SpellFlag::NO_ON_CAST_COMPLETE,
                    cast: CastConfig {
                        default_cast: Cast {
                            gcd: GCD_DEFAULT,
                            ..Cast::default()
                        },
                        cd: Cooldown {
                            timer: Some(timer),
                            duration: 3 * MINUTE,
                        },
                        ..CastConfig::default()
                    },
                    related_self_buff: Some(shatter_curse_aura),
                    ..SpellConfig::default()
                },
            );
            sim.add_major_cooldown(unit, major_cooldown(shatter_curse, cooldown_type::SURVIVAL));
        }
        "RaceTauren" => {
            let sim = &mut env.sim;
            sim.unit_mut(unit).sdm.multiply_stat(Stat::Health, 1.05);
            // 26 October notes: "Endurance no longer provides any modifiers beyond 5% increased
            // Health (no hit chance or critical damage modifiers)."
            if !oct26 {
                // 16 October beta notes: "Endurance: Now grants 2% increased critical damage and
                // healing (was 1% hit chance)."
                sim.unit_mut(unit).pseudo_stats.crit_damage_multiplier *= 1.02;
            }
        }
        "RaceTroll" => {
            apply_creature_type_slaying(env, "MobTypeBeast");

            let sim = &mut env.sim;
            let action_id = ActionId::spell(20554);
            // 27 October additions to the 26 October notes: "Berserking duration increased to 15
            // seconds (was 10 seconds)."
            let berserking_aura = sim.register_aura(
                unit,
                AuraConfig {
                    label: "Berserking".to_string(),
                    action_id: Some(action_id.clone()),
                    duration: if oct26 { 15 * SECOND } else { 10 * SECOND },
                    ..AuraConfig::default()
                },
            );
            sim.attach_multiply_attack_speed(berserking_aura, 1.1);
            sim.attach_multiply_cast_speed(berserking_aura, 1.1);
            let timer = sim.new_timer(unit);
            let berserking = sim.register_spell(
                unit,
                SpellConfig {
                    action_id,
                    flags: SpellFlag::NO_ON_CAST_COMPLETE,
                    cast: CastConfig {
                        cd: Cooldown {
                            timer: Some(timer),
                            duration: 3 * MINUTE,
                        },
                        ..CastConfig::default()
                    },
                    related_self_buff: Some(berserking_aura),
                    ..SpellConfig::default()
                },
            );
            sim.add_major_cooldown(unit, major_cooldown(berserking, cooldown_type::DPS));
        }
        "RaceUndead" => apply_touch_of_the_grave(&mut env.sim, unit),
        "RaceHighOrderSkyborne" => {
            apply_skyborne_shared_racials(env);

            let sim = &mut env.sim;
            // Client 1.60.1.70532: Energized (1270842) lasts 30 seconds; 70245 had 15, which the
            // Go reference keeps. The 26 October notes bring Forever back to 15 seconds: "Read
            // Leyline now increases Mana and Health regeneration by 50% (was 100%), with a
            // 15-second duration." Attuned's 100% needs a Ley Line before the pull, so it is not
            // assumed.
            let energized_duration = 15 * SECOND;
            let regen = read_ley_line_regen_multiplier(sim.rules);
            let energized = sim.register_aura(
                unit,
                AuraConfig {
                    label: "Energized".to_string(),
                    action_id: Some(ActionId::spell(1270842)),
                    duration: energized_duration,
                    on_gain: Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
                        let unit = sim.aura(aura).unit;
                        if sim.has_mana_bar(unit) {
                            sim.multiply_mana_regen_speed(unit, regen);
                        }
                    })),
                    on_expire: Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
                        let unit = sim.aura(aura).unit;
                        if sim.has_mana_bar(unit) {
                            sim.multiply_mana_regen_speed(unit, 1.0 / regen);
                        }
                    })),
                    ..AuraConfig::default()
                },
            );
            // Client 1.60.1.70058: Read Ley Line has a 2 sec cast (SpellMisc CastingTimeIndex).
            let timer = sim.new_timer(unit);
            sim.register_spell(
                unit,
                SpellConfig {
                    action_id: ActionId::spell(1259705),
                    flags: SpellFlag::APL | SpellFlag::NO_ON_CAST_COMPLETE,
                    cast: CastConfig {
                        default_cast: Cast {
                            gcd: GCD_DEFAULT,
                            cast_time: 2 * SECOND,
                            ..Cast::default()
                        },
                        cd: Cooldown {
                            timer: Some(timer),
                            duration: 2 * MINUTE,
                        },
                        ..CastConfig::default()
                    },
                    related_self_buff: Some(energized),
                    ..SpellConfig::default()
                },
            );
        }
        "RaceWindshaperSkyborne" => {
            apply_skyborne_shared_racials(env);

            let sim = &mut env.sim;
            // 26 October notes: "Skysight is now a 15% increased movement speed (was 10%), with a
            // 15-second duration." Charged's 30% needs an elemental convergence before the pull,
            // so it is not assumed.
            let (speed, duration) = skysight_speed_and_duration(sim.rules);
            let elemental_blessing = sim.register_aura(
                unit,
                AuraConfig {
                    label: "Elemental Blessing".to_string(),
                    action_id: Some(ActionId::spell(1259688)),
                    duration,
                    on_gain: Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
                        let unit = sim.aura(aura).unit;
                        sim.multiply_movement_speed(unit, speed);
                    })),
                    on_expire: Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
                        let unit = sim.aura(aura).unit;
                        sim.multiply_movement_speed(unit, 1.0 / speed);
                    })),
                    ..AuraConfig::default()
                },
            );
            // Client 1.60.1.70058: Skysight has a 0.5 sec cast (SpellMisc CastingTimeIndex).
            let timer = sim.new_timer(unit);
            sim.register_spell(
                unit,
                SpellConfig {
                    action_id: ActionId::spell(1259686),
                    flags: SpellFlag::APL | SpellFlag::NO_ON_CAST_COMPLETE,
                    cast: CastConfig {
                        default_cast: Cast {
                            gcd: GCD_DEFAULT,
                            cast_time: 500 * MILLISECOND,
                            ..Cast::default()
                        },
                        cd: Cooldown {
                            timer: Some(timer),
                            duration: 2 * MINUTE,
                        },
                        ..CastConfig::default()
                    },
                    related_self_buff: Some(elemental_blessing),
                    ..SpellConfig::default()
                },
            );
        }
        _ => {}
    }
    Ok(())
}

fn major_cooldown(spell: super::sim::SpellId, cooldown_type: u32) -> MajorCooldown {
    MajorCooldown {
        spell,
        priority: 0,
        cooldown_type,
        allow_spell_queueing: false,
        timings: Vec::new(),
    }
}

/// Go `applySkyborneSharedRacials`.
fn apply_skyborne_shared_racials(env: &mut Environment) {
    apply_creature_type_slaying(env, "MobTypeElemental");
    // 16 October beta notes: "Wind Blessed: Haste bonus increased to 3% (was 1%)." 26 October
    // notes: "Wind-Blessed Haste bonus reduced to 2% (was 3%)."
    let haste = if env.sim.rules == Rules::Forever {
        1.02
    } else {
        1.03
    };
    let pseudo = &mut env.sim.unit_mut(env.player).pseudo_stats;
    pseudo.attack_speed_multiplier *= haste;
    pseudo.cast_speed_multiplier *= haste;
}

/// Energized's mana regeneration: 50% more under the 26 October notes, double in the Go
/// reference.
/// Skysight's Elemental Blessing: its movement speed multiplier and duration, the 26 October
/// notes' under Forever's rules.
pub(crate) fn skysight_speed_and_duration(rules: Rules) -> (f64, i64) {
    match rules {
        Rules::Forever => (1.15, 15 * SECOND),
        Rules::GoReference => (1.1, 30 * SECOND),
    }
}

pub(crate) fn read_ley_line_regen_multiplier(rules: Rules) -> f64 {
    match rules {
        Rules::Forever => 1.5,
        Rules::GoReference => 2.0,
    }
}

/// Command, which the 26 October notes re-add: "increasing Warlock and Hunter pet damage by 5%".
/// The pets exist by now, as in Go, where the class registers them before its effects.
fn apply_command(sim: &mut Sim, unit: UnitId) {
    if !matches!(
        sim.character(unit).class.as_str(),
        "ClassHunter" | "ClassWarlock"
    ) {
        return;
    }
    for pet in sim.unit(unit).pets.clone() {
        sim.unit_mut(pet).pseudo_stats.damage_dealt_multiplier *= 1.05;
    }
}

/// Go `applyCreatureTypeSlaying`: Beast Slaying (troll 20557), Big Game Hunter (dwarf 1259721)
/// and Elemental Insight (Skyborne 1259707) are one 5% damage done versus the creature type.
/// Go changes the character's attack tables once they exist, after finalization.
fn apply_creature_type_slaying(env: &mut Environment, mob_type: &'static str) {
    let player = env.player;
    env.post_finalize
        .push(Rc::new(move |env: &mut Environment| {
            let attacker = env.sim.unit(player).unit_index as usize;
            for defender in env.sim.all_units() {
                if env.sim.unit(defender).mob_type == mob_type {
                    let index = env.sim.unit(defender).unit_index as usize;
                    env.attack_tables[attacker][index].damage_dealt_multiplier *= 1.05;
                }
            }
        }));
}

/// Dwarf Artisan's Precision, as Tauren Endurance's hit was before the 16 October notes:
/// `AddStats` of the hit, physical and spell alike.
fn add_hit(env: &mut Environment, hit_percent: f64) {
    let unit = env.player;
    env.sim
        .add_stat(unit, Stat::PhysicalHitPercent, hit_percent);
    env.sim.add_stat(unit, Stat::SpellHitPercent, hit_percent);
}

/// Ruthless and Savage: `AddStats` of the crit, physical and spell alike.
fn add_crit(env: &mut Environment, crit_percent: f64) {
    let unit = env.player;
    env.sim
        .add_stat(unit, Stat::PhysicalCritPercent, crit_percent);
    env.sim.add_stat(unit, Stat::SpellCritPercent, crit_percent);
}

/// Go `applyExpansiveMind`.
fn apply_expansive_mind(sim: &mut Sim, unit: UnitId) {
    match sim.character(unit).class.as_str() {
        "ClassWarrior" => sim.unit_mut(unit).rage_bar.max_rage *= 1.05,
        "ClassRogue" => sim.unit_mut(unit).energy_bar.max_energy *= 1.05,
        "ClassPriest" | "ClassMage" | "ClassWarlock" => {
            sim.unit_mut(unit).sdm.multiply_stat(Stat::Mana, 1.05);
        }
        _ => {}
    }
}

/// Go `applyEureka`: Eureka! raises the damage of the abilities the client's rows name and cuts
/// their cost. The 16 October 2026 beta notes make it "an 8 second buff with no charges. Same
/// effect on damage and ability costs."
fn apply_eureka(env: &mut Environment) {
    let unit = env.player;
    let class = env.sim.character(unit).class.clone();
    let mut proc_mask = ProcMask::SPECIAL;
    let (spell_id, resource_type) = match class.as_str() {
        "ClassRogue" => (1259812, Resource::Energy),
        "ClassWarrior" => (1259813, Resource::Rage),
        "ClassMage" => (1259817, Resource::Mana),
        "ClassWarlock" => (1259821, Resource::Mana),
        "ClassPriest" => {
            proc_mask = proc_mask | ProcMask::SPELL_HEALING;
            (1259823, Resource::Mana)
        }
        _ => return,
    };
    // Build 70009 states -10% cost (effect 0, SPELLMOD_COST) on all five class variants and
    // 70170 keeps it.
    const COST_REDUCTION: f64 = 0.1;
    let action_id = ActionId::spell(spell_id);
    const ANY_CLASS_SPELL: i64 = i64::MAX;
    let spells = env.agent.eureka_spells().unwrap_or(EurekaSpells {
        cost: ANY_CLASS_SPELL,
        damage: ANY_CLASS_SPELL,
        tick: 0,
    });
    let sim = &mut env.sim;

    let cost_mod = sim.add_dynamic_mod(
        unit,
        SpellModConfig {
            kind: SpellModType::PowerCostPct,
            class_mask: spells.cost,
            proc_mask,
            resource_type: Some(resource_type),
            float_value: -COST_REDUCTION,
            ..SpellModConfig::default()
        },
    );
    // A spell's damage multiplier covers its hits and its ticks alike, so the +10% goes on
    // every spell of either list, and the ticks of a spell that is only on the direct list take
    // it back.
    let damage_mod = sim.add_dynamic_mod(
        unit,
        SpellModConfig {
            kind: SpellModType::DamageDonePct,
            class_mask: spells.damage | spells.tick,
            proc_mask,
            float_value: 0.1,
            ..SpellModConfig::default()
        },
    );
    let direct_only = spells.damage & !spells.tick;
    let tick_cancel_mod = (direct_only != 0).then(|| {
        sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::DotDamageDonePct,
                class_mask: direct_only,
                proc_mask,
                float_value: 1.0 / 1.1 - 1.0,
                ..SpellModConfig::default()
            },
        )
    });

    let aura = sim.register_aura(
        unit,
        AuraConfig {
            label: "Eureka!".to_string(),
            action_id: Some(action_id.clone()),
            duration: 8 * SECOND,
            on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                sim.activate_spell_mod(cost_mod);
                sim.activate_spell_mod(damage_mod);
                if let Some(tick_cancel_mod) = tick_cancel_mod {
                    sim.activate_spell_mod(tick_cancel_mod);
                }
            })),
            on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                sim.deactivate_spell_mod(cost_mod);
                sim.deactivate_spell_mod(damage_mod);
                if let Some(tick_cancel_mod) = tick_cancel_mod {
                    sim.deactivate_spell_mod(tick_cancel_mod);
                }
            })),
            ..AuraConfig::default()
        },
    );

    let timer = sim.new_timer(unit);
    let spell = sim.register_spell(
        unit,
        SpellConfig {
            action_id,
            flags: SpellFlag::NO_ON_CAST_COMPLETE,
            cast: CastConfig {
                cd: Cooldown {
                    timer: Some(timer),
                    duration: 2 * MINUTE,
                },
                ..CastConfig::default()
            },
            related_self_buff: Some(aura),
            ..SpellConfig::default()
        },
    );
    sim.add_major_cooldown(unit, major_cooldown(spell, cooldown_type::DPS));
}

/// Go `applyTouchOfTheGrave`: a drain of 5% of the caster's maximum health. The 16 October 2026
/// beta notes: "a 30% chance to activate for casters and a 20% chance to activate for
/// Paladins/Rogues/Warriors, as well as a 25 second internal cooldown" (was 10% and 5% with a
/// 1 sec cooldown), and it "no longer scales with Shadow increases on the target or the caster".
fn apply_touch_of_the_grave(sim: &mut Sim, unit: UnitId) {
    let (aura_id, proc_chance) = match sim.character(unit).class.as_str() {
        "ClassWarrior" | "ClassPaladin" | "ClassRogue" => (1260189, 0.2),
        _ => (1260201, 0.3),
    };
    // 27 October additions to the 26 October notes: "Touch of the Grave internal cooldown
    // reduced to 15 seconds (was 25 seconds)."
    let icd = if sim.rules == Rules::Forever {
        15 * SECOND
    } else {
        25 * SECOND
    };
    // The drain heals through health metrics Go creates here; they are not prepared state.
    sim.register_spell(
        unit,
        SpellConfig {
            action_id: ActionId::spell(1260198),
            spell_school: school::SHADOW,
            defense_type: DefenseType::Magic,
            proc_mask: ProcMask::EMPTY,
            flags: SpellFlag::PROC
                | SpellFlag::PASSIVE_SPELL
                | SpellFlag::IGNORE_MODIFIERS
                | SpellFlag::NO_SPELL_MODS,
            damage_multiplier: 1.0,
            threat_multiplier: 1.0,
            ..SpellConfig::default()
        },
    );
    sim.make_proc_trigger_aura(
        unit,
        &ProcTrigger {
            name: "Touch of the Grave".to_string(),
            action_id: ActionId::spell(aura_id),
            callback: CallbackMask::ON_SPELL_HIT_DEALT,
            proc_mask: ProcMask::MELEE | ProcMask::RANGED | ProcMask::SPELL_DAMAGE,
            outcome: HitOutcome::LANDED,
            proc_chance,
            icd,
            ..ProcTrigger::default()
        },
    );
}

/// tools/oracle-v2 `eurekaEffect`: the class names its spells by masks, which the exporter
/// resolves, with spell_mod.go `shouldApply`'s rules, into spell positions.
pub(crate) fn eureka_effect(env: &Environment) -> Option<serde_json::Value> {
    use super::spell::{ProcMask, Resource, SpellFlag};
    let player = env.player;
    let aura = env.sim.aura(env.sim.get_aura(player, "Eureka!")?);
    let masks = env.agent.eureka_spells().unwrap_or(EurekaSpells {
        cost: i64::MAX,
        damage: i64::MAX,
        tick: 0,
    });
    let class = env.sim.character(player).class.as_str();
    let mut proc_mask = ProcMask::SPECIAL;
    if class == "ClassPriest" {
        proc_mask = ProcMask(proc_mask.0 | ProcMask::SPELL_HEALING.0);
    }
    let direct_only = masks.damage & !masks.tick;
    let modded = |spell: &super::spell::Spell, mask: i64| {
        mask != 0
            && !spell.flags.matches(SpellFlag::NO_SPELL_MODS)
            && spell.matches(mask)
            && proc_mask.matches(spell.proc_mask)
    };
    // The cost modifier names the class's resource: rage for a warrior, energy for a rogue and
    // mana otherwise, as applyEureka's ResourceType does.
    let pays_class_resource = |spell: &super::spell::Spell| match spell.cost.as_ref() {
        Some(cost) => match cost.resource {
            Resource::Rage => class == "ClassWarrior",
            Resource::Energy => class == "ClassRogue",
            Resource::Mana => class != "ClassWarrior" && class != "ClassRogue",
            Resource::Focus => false,
        },
        None => false,
    };
    let (mut cost, mut damage, mut ticks) = (vec![], vec![], vec![]);
    for (i, id) in env.sim.unit(player).spellbook.iter().enumerate() {
        let spell = env.sim.spell(*id);
        if pays_class_resource(spell) && modded(spell, masks.cost) {
            cost.push(i);
        }
        if modded(spell, masks.damage | masks.tick) {
            damage.push(i);
        }
        if modded(spell, direct_only) {
            ticks.push(i);
        }
    }
    Some(serde_json::json!({
        "kind": "eureka", "spell_id": aura.action_id.as_ref().map_or(0, |id| id.spell_id),
        "aura": aura.label, "cost_percent": -0.1, "damage_percent": 0.1,
        // Go folds the constant 1/1.1 - 1 exactly: -1/11, rounded once.
        "tick_cancel_percent": -1.0 / 11.0,
        "cost_spells": cost, "damage_spells": damage, "tick_cancel_spells": ticks,
    }))
}

#[cfg(test)]
mod tests;
