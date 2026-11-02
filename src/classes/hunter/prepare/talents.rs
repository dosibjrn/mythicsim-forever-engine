//! Go sim/hunter/talents_beast_mastery.go, talents_marksmanship.go and talents_survival.go: the
//! talents a hunter's `ApplyTalents` registers. A talent the sim leaves unmodelled registers
//! nothing, as in Go.

use std::rc::Rc;

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::character::{cooldown_type, MajorCooldown};
use crate::prepare::dbcenums;
use crate::prepare::env::{Environment, FinalizeEffect};
use crate::prepare::sim::SpellId;
use crate::prepare::sim::{AuraConfig, AuraId, Cooldown, EventCallbacks, Rules, Sim, MILLISECOND};
use crate::prepare::spell::{Cast, CastConfig, ProcMask, SpellConfig, SpellFlag};
use crate::prepare::spell_mod::{ModId, SpellModConfig, SpellModType};
use crate::prepare::stats::{Stat, Stats};

use super::spell_data::spell_data;
use super::spells::longest_cooldown as longest_cooldown_of;
use super::{masks, Hunter};

fn class_mod(kind: SpellModType, class_mask: i64, float_value: f64) -> SpellModConfig {
    SpellModConfig {
        kind,
        class_mask,
        float_value,
        ..SpellModConfig::default()
    }
}

fn proc_mod(kind: SpellModType, proc_mask: ProcMask, float_value: f64) -> SpellModConfig {
    SpellModConfig {
        kind,
        proc_mask,
        float_value,
        ..SpellModConfig::default()
    }
}

/// `(duration, per-rank value) * time.Millisecond`: Go's `time.Millisecond * time.Duration(v)`.
fn millis(value: f64) -> i64 {
    (value as i64) * MILLISECOND
}

impl Hunter {
    /// Go `registerBeastMasteryTalents`.
    pub(super) fn register_beast_mastery_talents(&mut self, sim: &mut Sim) {
        // Tier 1
        // Deadly Aspects: aspects.go
        self.register_endurance_training(sim);

        // Tier 2
        self.register_focused_fire(sim);
        // Improved Aspect of the Monkey, Pathfinding and Improved Revive Pet register nothing.

        // Tier 3
        // Bestial Swiftness registers nothing.
        self.register_unleashed_fury(sim);

        // Tier 4
        // Improved Mend Pet registers nothing.
        self.register_ferocity(sim);
        // Summon Hawk: summon_hawk.go

        // Tier 5
        // Spirit Bond registers nothing.
        self.register_intimidation(sim);
        self.register_bestial_discipline(sim);

        // Tier 6
        self.register_frenzy(sim);

        // Tier 7
        self.register_bestial_wrath(sim);
    }

    fn register_endurance_training(&mut self, sim: &mut Sim) {
        let Some(pet) = self.pet else { return };
        if self.t("endurance_training") == 0 {
            return;
        }
        // Forever drops the hunter's own health bonus: the spell carries only the pet modifier,
        // +3% a rank.
        let multiplier = spell_data()
            .endurance_training
            .effect(
                dbcenums::A_ADD_FLAT_MODIFIER,
                dbcenums::SPELLMOD_ALL_EFFECTS,
            )
            .multiplier_at(self.t("endurance_training"));
        sim.unit_mut(pet)
            .sdm
            .multiply_stat(Stat::Health, multiplier);
    }

    fn register_focused_fire(&mut self, sim: &mut Sim) {
        let Some(pet) = self.pet else { return };
        if self.t("focused_fire") == 0 {
            return;
        }
        // The single dummy effect is the 1% per rank damage bonus, for "you and your pet".
        let multiplier = spell_data()
            .focused_fire
            .effect_at(1)
            .multiplier_at(self.t("focused_fire"));
        sim.unit_mut(self.unit).pseudo_stats.damage_dealt_multiplier *= multiplier;
        sim.unit_mut(pet).pseudo_stats.damage_dealt_multiplier *= multiplier;
    }

    /// 19616's mask names the pet passive and Summon Hawk, so the hawks take it too.
    fn register_unleashed_fury(&mut self, sim: &mut Sim) {
        let rank = self.t("unleashed_fury");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            self.unit,
            class_mod(
                SpellModType::DamageDonePct,
                masks::SUMMON_HAWK,
                spell_data().unleashed_fury.fraction_at(rank),
            ),
        );
        if let Some(pet) = self.pet {
            sim.unit_mut(pet).pseudo_stats.damage_dealt_multiplier *=
                spell_data().unleashed_fury.multiplier_at(rank);
        }
    }

    /// 19598's mask names the pet passive and Summon Hawk, so the hawks take it too.
    fn register_ferocity(&mut self, sim: &mut Sim) {
        let rank = self.t("ferocity");
        if rank == 0 {
            return;
        }
        let crit = spell_data().ferocity.value_at(rank);
        sim.add_static_mod(
            self.unit,
            class_mod(SpellModType::BonusCritPercent, masks::SUMMON_HAWK, crit),
        );
        if let Some(pet) = self.pet {
            let mut stats = Stats::default();
            stats[Stat::PhysicalCritPercent] = crit;
            stats[Stat::SpellCritPercent] = crit;
            sim.add_stats(pet, &stats);
        }
    }

    fn register_bestial_discipline(&mut self, sim: &mut Sim) {
        let rank = self.t("bestial_discipline");
        if rank == 0 {
            return;
        }
        // The pet's focus regen is handled where the focus bar is enabled. This half is the
        // hunter's own mana regen while casting.
        sim.unit_mut(self.unit)
            .pseudo_stats
            .spirit_regen_rate_casting += spell_data()
            .bestial_discipline
            .effect(dbcenums::A_MOD_MANA_REGEN_INTERRUPT, 0)
            .fraction_at(rank);
    }

    fn register_frenzy(&mut self, sim: &mut Sim) {
        let Some(pet) = self.pet else { return };
        let rank = self.t("frenzy");
        if rank == 0 {
            return;
        }
        let frenzy_rank = spell_data().frenzy_triggered.highest();
        let speed_multiplier = 1.3;
        sim.register_aura(
            pet,
            AuraConfig {
                label: "Frenzy Effect".to_string(),
                action_id: Some(ActionId::spell(frenzy_rank.id)),
                duration: frenzy_rank.duration(),
                on_gain: Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
                    let unit = sim.aura(aura).unit;
                    sim.multiply_attack_speed(unit, speed_multiplier);
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
                    let unit = sim.aura(aura).unit;
                    sim.multiply_attack_speed(unit, 1.0 / speed_multiplier);
                })),
                ..AuraConfig::default()
            },
        );
        sim.make_proc_trigger_aura(
            pet,
            &ProcTrigger {
                name: "Frenzy".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                outcome: HitOutcome::CRIT,
                proc_chance: spell_data().frenzy.fraction_at(rank),
                ..ProcTrigger::default()
            },
        );
    }

    /// Bosses are immune to the stun; the pet's next attack still gets the crit bonus. Not an
    /// auto-cast cooldown: Go registers no major cooldown, so it is only cast if a rotation names it.
    fn register_intimidation(&mut self, sim: &mut Sim) {
        let Some(pet) = self.pet else { return };
        if !self.flag("intimidation") {
            return;
        }
        let unit = self.unit;
        let rank = spell_data().intimidation.rank(1);
        let action_id = ActionId::spell(rank.id);
        let bonus_crit = 100.0;
        sim.register_aura(
            pet,
            AuraConfig {
                label: "Intimidation".to_string(),
                action_id: Some(action_id.clone()),
                duration: rank.duration(),
                on_gain: Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
                    let unit = sim.aura(aura).unit;
                    sim.add_stat_dynamic(unit, Stat::PhysicalCritPercent, bonus_crit);
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
                    let unit = sim.aura(aura).unit;
                    sim.add_stat_dynamic(unit, Stat::PhysicalCritPercent, -bonus_crit);
                })),
                events: EventCallbacks {
                    on_spell_hit_dealt: true,
                    ..EventCallbacks::default()
                },
                ..AuraConfig::default()
            },
        );
        let timer = sim.new_timer(unit);
        let mana = rank.mana_cost();
        sim.register_spell(
            unit,
            SpellConfig {
                action_id,
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::APL,
                cost: crate::prepare::spell::CostOptions {
                    mana_base_cost_percent: mana.base_cost_percent,
                    mana_flat_cost: mana.flat_cost,
                    ..Default::default()
                },
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: rank.gcd(),
                        ..Cast::default()
                    },
                    ignore_haste: true,
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown_of(rank),
                    },
                    ..CastConfig::default()
                },
                ..SpellConfig::default()
            },
        );
    }

    fn register_bestial_wrath(&mut self, sim: &mut Sim) {
        let Some(pet) = self.pet else { return };
        if !self.flag("bestial_wrath") {
            return;
        }
        let unit = self.unit;
        let rank = spell_data().bestial_wrath.highest();
        let action_id = ActionId::spell(rank.id);
        let damage_multiplier = 1.5;
        let aura = sim.register_aura(
            pet,
            AuraConfig {
                label: "Bestial Wrath".to_string(),
                action_id: Some(action_id.clone()),
                duration: rank.duration(),
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.unit_mut(pet).pseudo_stats.damage_dealt_multiplier *= damage_multiplier;
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.unit_mut(pet).pseudo_stats.damage_dealt_multiplier /= damage_multiplier;
                })),
                ..AuraConfig::default()
            },
        );
        if let Some(state) = self.pet_state.as_mut() {
            state.bestial_wrath_aura = Some(aura);
        }
        let timer = sim.new_timer(unit);
        let mana = rank.mana_cost();
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id,
                class_spell_mask: masks::BESTIAL_WRATH,
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::APL,
                cost: crate::prepare::spell::CostOptions {
                    mana_base_cost_percent: mana.base_cost_percent,
                    mana_flat_cost: mana.flat_cost,
                    ..Default::default()
                },
                cast: CastConfig {
                    default_cast: Cast {
                        non_empty: true,
                        ..Cast::default()
                    },
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown_of(rank),
                    },
                    ..CastConfig::default()
                },
                ..SpellConfig::default()
            },
        );
        sim.add_major_cooldown(
            unit,
            MajorCooldown {
                spell,
                priority: 0,
                cooldown_type: cooldown_type::DPS,
                allow_spell_queueing: false,
                timings: Vec::new(),
            },
        );
    }

    /// Go `registerMarksmanshipTalents`.
    pub(super) fn register_marksmanship_talents(&mut self, sim: &mut Sim) {
        // Tier 1
        self.register_hawk_eye(sim);
        // Improved Concussive Shot registers nothing.
        self.register_lethal_attacks(sim);

        // Tier 2
        self.register_improved_stings(sim);
        self.register_efficiency(sim);
        self.register_careful_aim(sim);

        // Tier 3
        // Rapid Killing: rapid_fire.go
        self.register_improved_arcane_shot(sim);
        self.register_lone_wolf(sim);

        // Tier 4
        // Trueshot Aura is a party buff.
        self.register_mortal_shots(sim);

        // Tier 5
        self.register_rapid_recuperation(sim);
        self.register_barrage(sim);
        // Scatter Shot registers nothing.

        // Tier 6
        self.register_ranged_weapon_specialization(sim);

        // Tier 7
        // Sniper Shot: sniper_shot.go
    }

    fn register_efficiency(&mut self, sim: &mut Sim) {
        let rank = self.t("efficiency");
        if rank == 0 {
            return;
        }
        // The tooltip reads "Shots, Stings and melee abilities", but 19416's class mask leaves
        // out Sniper Shot, and Strider Kick until client 1.60.1.70707 added bit 2097152 of the
        // third mask word (Flanking Strike and Strider Kick; SpellEffect 696992). The Go
        // reference keeps the old mask.
        let value = spell_data()
            .efficiency
            .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_COST)
            .fraction_at(rank);
        let melee = match sim.rules {
            Rules::Forever => masks::MELEE,
            Rules::GoReference => masks::MELEE & !masks::STRIDER_KICK,
        };
        sim.add_static_mod(
            self.unit,
            class_mod(
                SpellModType::PowerCostPctAdd,
                masks::SHOTS_AND_STINGS | melee,
                value,
            ),
        );
    }

    fn register_improved_arcane_shot(&mut self, sim: &mut Sim) {
        let rank = self.t("improved_arcane_shot");
        if rank == 0 {
            return;
        }
        let value = spell_data()
            .improved_arcane_shot
            .effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_COOLDOWN)
            .value_at(rank);
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                kind: SpellModType::CooldownFlat,
                class_mask: masks::ARCANE_SHOT,
                time_value: millis(value),
                ..SpellModConfig::default()
            },
        );
    }

    fn register_improved_stings(&mut self, sim: &mut Sim) {
        let rank = self.t("improved_stings");
        if rank == 0 {
            return;
        }
        // The client's rank curve gives 6/13/20, not a linear 6/12/18.
        let value = spell_data()
            .improved_stings
            .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_DOT)
            .fraction_at(rank);
        sim.add_static_mod(
            self.unit,
            class_mod(SpellModType::DotDamageDonePct, masks::SERPENT_STING, value),
        );
    }

    fn register_mortal_shots(&mut self, sim: &mut Sim) {
        let rank = self.t("mortal_shots");
        if rank == 0 {
            return;
        }
        // Client 19485's class mask: Auto Shot, Aimed, Arcane and Multi-Shot, Serpent Sting and
        // Volley, and since 70450 Sniper Shot (Go `MortalShotsIncludesSniperShot`, MythicSim patch
        // 172).
        let bonus = spell_data().mortal_shots.fraction_at(rank);
        sim.add_static_mod(
            self.unit,
            proc_mod(
                SpellModType::CritMultiplierFlat,
                ProcMask::RANGED_AUTO,
                bonus,
            ),
        );
        sim.add_static_mod(
            self.unit,
            class_mod(
                SpellModType::CritMultiplierFlat,
                masks::AIMED_SHOT
                    | masks::ARCANE_SHOT
                    | masks::MULTI_SHOT
                    | masks::SERPENT_STING
                    | masks::VOLLEY
                    | masks::SNIPER_SHOT,
                bonus,
            ),
        );
    }

    fn register_barrage(&mut self, sim: &mut Sim) {
        let rank = self.t("barrage");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            self.unit,
            class_mod(
                SpellModType::DamageDoneFlat,
                masks::MULTI_SHOT | masks::AIMED_SHOT | masks::VOLLEY,
                spell_data().barrage.effect_at(1).fraction_at(rank),
            ),
        );
    }

    fn register_ranged_weapon_specialization(&mut self, sim: &mut Sim) {
        let rank = self.t("ranged_weapon_specialization");
        if rank == 0 {
            return;
        }
        // Serpent Sting is a sting, not a ranged weapon attack, and is left out. It carries the
        // ranged special proc mask, so the shots are named instead: Auto Shot has no class mask.
        let value = spell_data().ranged_weapon_specialization.fraction_at(rank);
        sim.add_static_mod(
            self.unit,
            proc_mod(SpellModType::DamageDonePct, ProcMask::RANGED_AUTO, value),
        );
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                kind: SpellModType::DamageDonePct,
                class_mask: masks::ALL & !masks::SERPENT_STING,
                proc_mask: ProcMask::RANGED_SPECIAL,
                float_value: value,
                ..SpellModConfig::default()
            },
        );
    }

    /// 1223984 states 20% of Intellect a rank (Misc 3) twice, once per attack power aura.
    fn register_careful_aim(&mut self, sim: &mut Sim) {
        let rank = self.t("careful_aim");
        if rank == 0 {
            return;
        }
        let ap_per_int = spell_data().careful_aim.effect_at(1).fraction_at(rank);
        let sdm = &mut sim.unit_mut(self.unit).sdm;
        sdm.add_stat_dependency(Stat::Intellect, Stat::AttackPower, ap_per_int);
        sdm.add_stat_dependency(Stat::Intellect, Stat::RangedAttackPower, ap_per_int);
    }

    fn register_hawk_eye(&mut self, sim: &mut Sim) {
        let rank = self.t("hawk_eye");
        if rank == 0 {
            return;
        }
        let bonus_range = spell_data()
            .hawk_eye
            .effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_RANGE)
            .value_at(rank);
        sim.unit_mut(self.unit).auto_attacks.ranged.max_range += bonus_range;
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                kind: SpellModType::Custom,
                proc_mask: ProcMask::RANGED,
                apply_custom: Some(Rc::new(move |sim: &mut Sim, _: ModId, spell: SpellId| {
                    if sim.spell(spell).max_range > 0.0 {
                        sim.spell_mut(spell).max_range += bonus_range;
                    }
                })),
                remove_custom: Some(Rc::new(move |sim: &mut Sim, _: ModId, spell: SpellId| {
                    if sim.spell(spell).max_range > 0.0 {
                        sim.spell_mut(spell).max_range -= bonus_range;
                    }
                })),
                ..SpellModConfig::default()
            },
        );
    }

    fn register_lethal_attacks(&mut self, sim: &mut Sim) {
        let rank = self.t("lethal_attacks");
        if rank == 0 {
            return;
        }
        let crit = spell_data().lethal_attacks.value_at(rank);
        sim.add_stat(self.unit, Stat::PhysicalCritPercent, crit);
        sim.add_stat(self.unit, Stat::SpellCritPercent, crit);
    }

    /// 415370: 20% damage done to every school while no pet is out.
    fn register_lone_wolf(&mut self, sim: &mut Sim) {
        if !self.flag("lone_wolf") || self.pet.is_some() {
            return;
        }
        sim.unit_mut(self.unit).pseudo_stats.damage_dealt_multiplier *= spell_data()
            .lone_wolf
            .effect(dbcenums::A_MOD_DAMAGE_PERCENT_DONE, 127)
            .multiplier_at(1);
    }

    /// Only the Serpent Sting half is modelled: nothing dies mid fight to hand out the kill half.
    fn register_rapid_recuperation(&mut self, sim: &mut Sim) {
        let rank = self.t("rapid_recuperation");
        if rank == 0 {
            return;
        }
        let unit = self.unit;
        let buff = spell_data().rapid_recuperation_triggered.highest();
        let regen = spell_data()
            .rapid_recuperation
            .effect_at(1)
            .fraction_at(rank);
        sim.register_aura(
            unit,
            AuraConfig {
                label: "Rapid Recuperation".to_string(),
                action_id: Some(ActionId::spell(buff.id)),
                duration: buff.duration(),
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.unit_mut(unit).pseudo_stats.spirit_regen_rate_casting += regen;
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.unit_mut(unit).pseudo_stats.spirit_regen_rate_casting -= regen;
                })),
                ..AuraConfig::default()
            },
        );
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Rapid Recuperation Trigger".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                class_spell_mask: masks::SERPENT_STING,
                ..ProcTrigger::default()
            },
        );
    }

    /// Go `registerSurvivalTalents`.
    pub(super) fn register_survival_talents(&mut self, sim: &mut Sim) {
        // Tier 1
        self.register_improved_tracking();
        self.register_deflection(sim);

        // Tier 2
        // Entrapment registers nothing.
        self.register_savage_strikes(sim);
        self.register_survivalist(sim);
        // Improved Wing Clip registers nothing.

        // Tier 3
        self.register_clever_traps(sim);
        self.register_surefooted(sim);
        // Deterrence registers nothing.

        // Tier 4
        self.register_survival_tactics(sim);
        self.register_predators_edge(sim);
        // Counterattack registers nothing.

        // Tier 5
        self.register_resourcefulness(sim);
        self.register_expose_prey(sim);
        self.register_survivalists_discipline(sim);
        // Strider Kick: strider_kick.go

        // Tier 6
        self.register_lightning_reflexes(sim);

        // Tier 7
        // Lacerating Strikes: lacerating_strikes.go
    }

    fn register_savage_strikes(&mut self, sim: &mut Sim) {
        let rank = self.t("savage_strikes");
        if rank == 0 {
            return;
        }
        // 19159's mask also names the Lacerating Strikes bleed, whose ticks crit.
        sim.add_static_mod(
            self.unit,
            class_mod(
                SpellModType::BonusCritPercent,
                masks::MELEE | masks::LACERATING_STRIKES,
                spell_data().savage_strikes.value_at(rank),
            ),
        );
    }

    fn register_survivalist(&mut self, sim: &mut Sim) {
        let rank = self.t("survivalist");
        if rank == 0 {
            return;
        }
        sim.unit_mut(self.unit)
            .sdm
            .multiply_stat(Stat::Health, spell_data().survivalist.multiplier_at(rank));
    }

    /// 19290's melee hit effect carries the 1/2/3% curve; spell hit reads the melee curve too.
    fn register_surefooted(&mut self, sim: &mut Sim) {
        let rank = self.t("surefooted");
        if rank == 0 {
            return;
        }
        let hit = spell_data()
            .surefooted
            .effect(dbcenums::A_MOD_HIT_CHANCE, 0)
            .value_at(rank);
        sim.add_stat(self.unit, Stat::PhysicalHitPercent, hit);
        sim.add_stat(self.unit, Stat::SpellHitPercent, hit);
    }

    fn register_resourcefulness(&mut self, sim: &mut Sim) {
        let rank = self.t("resourcefulness");
        if rank == 0 {
            return;
        }
        let unit = self.unit;
        // 440529's cost mask leaves out Strider Kick.
        sim.add_static_mod(
            unit,
            class_mod(
                SpellModType::PowerCostPctAdd,
                // 440529's cost mask leaves out Strider Kick; the 16 October 2026 beta notes put
                // it in ("Resourcefulness now reduces the mana cost of Strider Kick").
                masks::TRAPS | masks::MELEE,
                spell_data()
                    .resourcefulness
                    .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_COST)
                    .fraction_at(rank),
            ),
        );
        // The buff is 50% mana regen while casting for 30 sec at both ranks; the points buy the
        // proc chance, 50/100%.
        let buff = spell_data().resourcefulness_triggered.highest();
        let regen = buff
            .effect(dbcenums::A_MOD_MANA_REGEN_INTERRUPT, 0)
            .average(CHARACTER_LEVEL)
            / 100.0;
        sim.register_aura(
            unit,
            AuraConfig {
                label: "Resourcefulness".to_string(),
                action_id: Some(ActionId::spell(buff.id)),
                duration: buff.duration(),
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.unit_mut(unit).pseudo_stats.spirit_regen_rate_casting += regen;
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.unit_mut(unit).pseudo_stats.spirit_regen_rate_casting -= regen;
                })),
                ..AuraConfig::default()
            },
        );
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Resourcefulness Trigger".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                // 16 October 2026 beta notes: only crits of melee and ranged abilities, not traps
                // or auto attacks.
                proc_mask: ProcMask::MELEE_SPECIAL | ProcMask::RANGED_SPECIAL,
                outcome: HitOutcome::CRIT,
                proc_chance: 0.5 * f64::from(rank),
                ..ProcTrigger::default()
            },
        );
    }

    /// The client curve reads 3% a rank, where our sim had 2%.
    fn register_lightning_reflexes(&mut self, sim: &mut Sim) {
        let rank = self.t("lightning_reflexes");
        if rank == 0 {
            return;
        }
        sim.unit_mut(self.unit).sdm.multiply_stat(
            Stat::Agility,
            spell_data().lightning_reflexes.multiplier_at(rank),
        );
    }

    /// Everything a raid encounter can be is trackable apart from Mechanical. Damage only.
    fn register_improved_tracking(&mut self) {
        let rank = self.t("improved_tracking");
        if rank == 0 {
            return;
        }
        let multiplier = spell_data().improved_tracking.multiplier_at(rank);
        let effect: FinalizeEffect = Rc::new(move |env: &mut Environment| {
            let hunter = env.player;
            // Unknown counts: the site's default target is a boss of no set type, and the
            // hunter tracks whatever it is (community #783).
            for target in env.encounter.targets.clone() {
                let trackable = matches!(
                    env.sim.unit(target).mob_type.as_str(),
                    "MobTypeUnknown"
                        | "MobTypeBeast"
                        | "MobTypeDemon"
                        | "MobTypeDragonkin"
                        | "MobTypeElemental"
                        | "MobTypeGiant"
                        | "MobTypeHumanoid"
                        | "MobTypeUndead"
                );
                if trackable {
                    env.attack_table_mut(hunter, target).damage_dealt_multiplier *= multiplier;
                }
            }
        });
        self.post_finalize_effects.push(effect);
    }

    fn register_deflection(&mut self, sim: &mut Sim) {
        let rank = self.t("deflection");
        if rank == 0 {
            return;
        }
        sim.unit_mut(self.unit).pseudo_stats.base_parry_chance +=
            spell_data().deflection.fraction_at(rank);
    }

    fn register_clever_traps(&mut self, sim: &mut Sim) {
        let rank = self.t("clever_traps");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            self.unit,
            class_mod(
                SpellModType::DamageDonePct,
                masks::TRAPS,
                spell_data()
                    .clever_traps
                    .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_ALL_EFFECTS)
                    .fraction_at(rank),
            ),
        );
    }

    fn register_survival_tactics(&mut self, sim: &mut Sim) {
        let rank = self.t("survival_tactics");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            self.unit,
            class_mod(
                SpellModType::BonusHitPercent,
                masks::TRAPS,
                spell_data().survival_tactics.value_at(rank),
            ),
        );
    }

    fn register_predators_edge(&mut self, sim: &mut Sim) {
        let rank = self.t("predators_edge");
        if rank == 0 {
            return;
        }
        // 1310627's crit damage mask is the melee abilities and the Lacerating Strikes bleed:
        // no auto attacks, no hawks.
        sim.add_static_mod(
            self.unit,
            class_mod(
                SpellModType::CritMultiplierFlat,
                masks::MELEE | masks::LACERATING_STRIKES,
                spell_data()
                    .predators_edge
                    .effect(
                        dbcenums::A_ADD_PCT_MODIFIER,
                        dbcenums::SPELLMOD_CRIT_DAMAGE_BONUS,
                    )
                    .fraction_at(rank),
            ),
        );
        sim.add_static_mod(
            self.unit,
            proc_mod(
                SpellModType::DamageDonePct,
                ProcMask::MELEE_OH,
                spell_data()
                    .predators_edge
                    .effect(dbcenums::A_MOD_OFFHAND_DAMAGE_PCT, 0)
                    .fraction_at(rank),
            ),
        );
    }

    fn register_survivalists_discipline(&mut self, sim: &mut Sim) {
        let rank = self.t("survivalists_discipline");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            self.unit,
            class_mod(
                SpellModType::CooldownMultiplier,
                masks::TRAPS,
                spell_data()
                    .survivalists_discipline
                    .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_COOLDOWN)
                    .multiplier_at(rank),
            ),
        );
    }

    /// Expose Prey opens the Mongoose Bite window off a landed melee or ranged attack on a
    /// target with Hunter's Mark.
    fn register_expose_prey(&mut self, sim: &mut Sim) {
        let rank = self.t("expose_prey");
        if rank == 0 {
            return;
        }
        sim.make_proc_trigger_aura(
            self.unit,
            &ProcTrigger {
                name: "Expose Prey".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                proc_mask: ProcMask::MELEE_OR_RANGED,
                proc_chance: spell_data().expose_prey.fraction_at(rank),
                ..ProcTrigger::default()
            },
        );
    }

    /// Go `addPvpGloves`: `RegisterPvPGloveMod` for the glove items that add 5% to Multi-Shot.
    pub(super) fn add_pvp_gloves(&mut self, sim: &mut Sim) {
        const PVP_GLOVE_ITEM_IDS: [i32; 4] = [23279, 22862, 16463, 16571];
        let unit = self.unit;
        let id = sim.add_dynamic_mod(
            unit,
            class_mod(SpellModType::DamageDoneFlat, masks::MULTI_SHOT, 0.05),
        );
        let hands = sim.character(unit).equipment[crate::prepare::items::slot::HANDS].id;
        if PVP_GLOVE_ITEM_IDS.contains(&hands) {
            sim.activate_spell_mod(id);
        } else {
            sim.deactivate_spell_mod(id);
        }
    }
}
