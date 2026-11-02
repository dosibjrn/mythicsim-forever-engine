//! Go sim/druid's Feral Cat spells (claw.go, shred.go, ravage.go, rake.go, rip.go,
//! ferocious_bite.go, prowl.go, shifting_power.go) and talents_feral_combat.go.

use std::rc::Rc;

use crate::classes::druid::forms::{ANY, BEAR, CAT};
use crate::contracts::prepared_v2::ActionId;
use crate::prepare::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use crate::prepare::character::constants::{CHARACTER_LEVEL, MAX_MELEE_RANGE};
use crate::prepare::character::{cooldown_type, MajorCooldown};
use crate::prepare::dbcenums::{
    A_ADD_FLAT_MODIFIER, A_ADD_PCT_MODIFIER, A_MOD_DECREASE_SPEED, A_MOD_DODGE_PERCENT,
    SPELLMOD_COOLDOWN, SPELLMOD_CRIT_DAMAGE_BONUS, SPELLMOD_DAMAGE,
};
use crate::prepare::hotfix_113554;
use crate::prepare::major_cooldown::COOLDOWN_PRIORITY_DEFAULT;
use crate::prepare::sim::{
    AuraConfig, Cooldown, EventCallbacks, Sim, MILLISECOND, NEVER_EXPIRES, SECOND,
};
use crate::prepare::spell::{
    school, Cast, CastConfig, CostOptions, DotConfig, ProcMask, SpellConfig, SpellFlag,
};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::spelldata::{Ladder, Spell};
use crate::prepare::stats::{Stat, Stats};

use super::{masks, Druid};

const CLAW_RANKS: [i32; 5] = [1082, 3029, 5201, 9849, 9850];
const SHRED_RANKS: [i32; 5] = [5221, 6800, 8992, 9829, 9830];
const RAVAGE_RANKS: [i32; 4] = [6785, 6787, 9866, 9867];
const RAKE_RANKS: [i32; 4] = [1822, 1823, 1824, 9904];
const RIP_RANKS: [i32; 6] = [1079, 9492, 9493, 9752, 9894, 9896];
const BITE_RANKS: [i32; 5] = [22568, 22827, 22828, 22829, 31018];
const PROWL_RANKS: [i32; 3] = [5215, 6783, 9913];

fn energy_cost(rank: &Spell) -> CostOptions {
    CostOptions {
        energy_cost: rank.cost() as i32,
        energy_refund: rank.miss_refund(),
        ..CostOptions::default()
    }
}

fn gcd_cast(rank: &Spell) -> CastConfig {
    CastConfig {
        default_cast: Cast {
            gcd: rank.gcd(),
            ..Cast::default()
        },
        ignore_haste: true,
        ..CastConfig::default()
    }
}

/// The base of a melee special's `SpellConfig`: school, defense, proc mask, flags and rank.
fn melee_special(rank: &Spell, mask: i64, extra_flags: SpellFlag) -> SpellConfig {
    SpellConfig {
        action_id: ActionId::spell(rank.id),
        spell_school: rank.spell_school(),
        defense_type: rank.defense_type_core(),
        proc_mask: ProcMask::MELEE_MH_SPECIAL,
        class_spell_mask: mask,
        flags: SpellFlag::MELEE_METRICS | SpellFlag::APL | extra_flags,
        rank: rank.rank_number(),
        cost: energy_cost(rank),
        cast: gcd_cast(rank),
        threat_multiplier: 1.0,
        max_range: MAX_MELEE_RANGE,
        ..SpellConfig::default()
    }
}

impl Druid {
    /// Go `RegisterFeralCatSpells`.
    pub(super) fn register_feral_cat_spells(&mut self, sim: &mut Sim) {
        self.register_cat_form_spell(sim);

        // Forever has no Cat-form Mangle.
        self.register_rake_spell(sim);
        self.register_rip_spell(sim);
        self.register_ferocious_bite_spell(sim);
        // Forever drops Faerie Fire (Feral); the Balance version is the only one.
        self.register_faerie_fire_spell(sim);
        self.register_shred_spell(sim);
        self.register_claw_spell(sim);
        self.register_prowl_spell(sim);
        self.register_ravage_spell(sim);
        self.register_shifting_power_spell(sim);
        // A cat that leaves its form to refresh Moonfire.
        self.register_moonfire_spell(sim);
    }

    fn register_claw_spell(&mut self, sim: &mut Sim) {
        let ladder = Ladder::ranked(&CLAW_RANKS);
        let rank = ladder.highest();
        let weapon_multiplier = ladder.effect_at(2).fraction_at(rank.rank_number());
        let mut config = melee_special(rank, masks::CLAW, SpellFlag::NONE);
        config.damage_multiplier = weapon_multiplier;
        self.claw = Some(self.register_spell(sim, CAT, config));
    }

    fn register_shred_spell(&mut self, sim: &mut Sim) {
        let ladder = Ladder::ranked(&SHRED_RANKS);
        let rank = ladder.highest();
        let weapon_multiplier = ladder.effect_at(2).fraction_at(rank.rank_number());
        let mut config = melee_special(rank, masks::SHRED, SpellFlag::NONE);
        config.damage_multiplier = weapon_multiplier;
        self.shred = Some(self.register_spell(sim, CAT, config));
    }

    fn register_ravage_spell(&mut self, sim: &mut Sim) {
        let ladder = Ladder::ranked(&RAVAGE_RANKS);
        let rank = ladder.highest();
        let weapon_multiplier = ladder.effect_at(2).fraction_at(rank.rank_number());
        let mut config = melee_special(rank, masks::RAVAGE, SpellFlag::CANNOT_BE_DODGED);
        config.damage_multiplier = weapon_multiplier;
        self.ravage = Some(self.register_spell(sim, CAT, config));
    }

    fn register_rake_spell(&mut self, sim: &mut Sim) {
        let rank = Ladder::ranked(&RAKE_RANKS).highest();
        let tick = rank.periodic_effect();
        let mut config = melee_special(rank, masks::RAKE, SpellFlag::IGNORE_RESISTS);
        config.damage_multiplier = 1.0;
        config.dot = DotConfig {
            aura: AuraConfig {
                label: "Rake".to_string(),
                duration: rank.duration(),
                ..AuraConfig::default()
            },
            number_of_ticks: (rank.duration() / tick.period()) as i32,
            tick_length: tick.period(),
            ..DotConfig::default()
        };
        self.rake = Some(self.register_spell(sim, CAT, config));
    }

    fn register_rip_spell(&mut self, sim: &mut Sim) {
        let rank = Ladder::ranked(&RIP_RANKS).highest();
        let tick = rank.periodic_effect();
        let mut config = melee_special(rank, masks::RIP, SpellFlag::NONE);
        config.damage_multiplier = 1.0;
        config.dot = DotConfig {
            aura: AuraConfig {
                label: "Rip".to_string(),
                ..AuraConfig::default()
            },
            number_of_ticks: (rank.duration() / tick.period()) as i32,
            tick_length: tick.period(),
            ..DotConfig::default()
        };
        self.rip = Some(self.register_spell(sim, CAT, config));
    }

    fn register_ferocious_bite_spell(&mut self, sim: &mut Sim) {
        let rank = Ladder::ranked(&BITE_RANKS).highest();
        let mut config = melee_special(rank, masks::FEROCIOUS_BITE, SpellFlag::NONE);
        config.damage_multiplier = 1.0;
        self.ferocious_bite = Some(self.register_spell(sim, CAT, config));
    }

    fn register_prowl_spell(&mut self, sim: &mut Sim) {
        let unit = self.unit;
        let rank = Ladder::ranked(&PROWL_RANKS).highest();
        let action = ActionId::spell(rank.id);
        let movement_speed_multiplier =
            1.0 + rank.effect(A_MOD_DECREASE_SPEED, 0).base_value() / 100.0;

        let icd = Cooldown {
            timer: Some(sim.new_timer(unit)),
            duration: rank.cooldown().max(rank.category_cooldown()),
        };

        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Prowl".to_string(),
                action_id: Some(action.clone()),
                duration: NEVER_EXPIRES,
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.multiply_movement_speed(unit, movement_speed_multiplier);
                    // No white swing goes out from Prowl: the swing replacement keeps the swing.
                    sim.unit_mut(unit).auto_attacks.replace_mh_swing = true;
                })),
                events: EventCallbacks {
                    on_spell_hit_dealt: true,
                    ..EventCallbacks::default()
                },
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.multiply_movement_speed(unit, 1.0 / movement_speed_multiplier);
                    sim.unit_mut(unit).auto_attacks.replace_mh_swing = false;
                })),
                ..AuraConfig::default()
            },
        );
        self.prowl_aura = Some(aura);

        if let Some(cat_aura) = self.cat_form_aura {
            sim.apply_on_expire(
                cat_aura,
                Rc::new(move |sim: &mut Sim, _| {
                    if sim.aura(aura).active {
                        sim.deactivate(aura);
                    }
                }),
            );
        }

        let spell = self.register_spell(
            sim,
            ANY,
            SpellConfig {
                action_id: action,
                spell_school: rank.spell_school(),
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::APL,
                cast: CastConfig {
                    cd: icd,
                    ..CastConfig::default()
                },
                ..SpellConfig::default()
            },
        );
        self.prowl = Some(spell);
    }

    /// Go `registerShiftingPowerSpell`.
    fn register_shifting_power_spell(&mut self, sim: &mut Sim) {
        if !self.tal.shifting_power {
            return;
        }
        let unit = self.unit;
        // Hotfix push 113531 halves the cooldown to 8 sec under Forever's rules.
        let rank = hotfix_113554::row(sim.rules, 1322605);
        let mana = rank.mana_cost();
        let timer = sim.new_timer(unit);
        let spell = self.register_spell(
            sim,
            CAT,
            SpellConfig {
                action_id: ActionId::spell(rank.id),
                class_spell_mask: masks::SHIFTING_POWER,
                flags: SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::APL,
                cost: CostOptions {
                    mana_base_cost_percent: mana.base_cost_percent,
                    mana_flat_cost: mana.flat_cost,
                    ..CostOptions::default()
                },
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: rank.gcd(),
                        ..Cast::default()
                    },
                    ignore_haste: true,
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: rank.cooldown().max(rank.category_cooldown()),
                    },
                    ..CastConfig::default()
                },
                ..SpellConfig::default()
            },
        );
        self.shifting_power = Some(spell);
    }

    /// Go `registerFeralCombatTalents`.
    pub(super) fn register_feral_combat_talents(&mut self, sim: &mut Sim) {
        // Tier 1
        self.apply_ferocity(sim);
        self.apply_heart_of_the_wild(sim);

        // Tier 2
        self.apply_feral_swiftness(sim);
        self.apply_feral_instincts(sim);
        // Brutal Impact is not modelled.
        self.apply_thick_hide(sim);

        // Tier 3
        self.apply_shredding_attacks(sim);
        self.apply_savage_fury(sim);
        // Feral Charge is not modelled; Sharpened Claws is applied in forms.

        // Tier 4: Shifting Power and Primal Bite are spells; Predatory Strikes is in forms.
        self.apply_blood_frenzy(sim);

        // Tier 5
        self.apply_improved_shifting_power(sim);
        self.apply_natural_instinct(sim);

        // Tier 6
        self.apply_natural_reaction(sim);
        self.apply_rend_and_tear(sim);

        // Tier 7
        self.apply_berserk(sim);
    }

    /// Forever swaps the armor multiplier for flat base Armor from level and defense skill, and
    /// the form's own armor multiplier applies on top of it.
    fn apply_thick_hide(&mut self, sim: &mut Sim) {
        if self.tal.thick_hide == 0 {
            return;
        }
        let unit = self.unit;
        let ladder = Ladder::talent(16929, 3);
        let per_defense = ladder.effect_at(1).fraction_at(self.tal.thick_hide);
        let per_level = ladder.effect_at(2).value_at(self.tal.thick_hide);

        // Defense skill is the rating the gear carries divided by the rating a point costs.
        let defense_skill = sim.equip_stats(unit)[Stat::DefenseRating]
            / crate::prepare::character::constants::DEFENSE_RATING_PER_DEFENSE_LEVEL;
        // talents_feral_combat.go 60 is one fused multiply-add: the level product is rounded
        // and the defense product is added exactly.
        let mut armor = per_defense.mul_add(defense_skill, per_level * f64::from(CHARACTER_LEVEL));
        if self.st.starting_form & BEAR != 0 {
            armor *= super::forms::BASE_BEAR_ARMOR_MULTI;
        }
        sim.add_stat(unit, Stat::Armor, armor);
    }

    /// Natural Instinct: extra critical strike damage on every ability the druid uses.
    fn apply_natural_instinct(&mut self, sim: &mut Sim) {
        if self.tal.natural_instinct == 0 {
            return;
        }
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                class_mask: masks::ALL,
                school: school::PHYSICAL,
                kind: SpellModType::CritMultiplierFlat,
                float_value: Ladder::talent(1223242, 2)
                    .effect(A_ADD_PCT_MODIFIER, SPELLMOD_CRIT_DAMAGE_BONUS)
                    .fraction_at(self.tal.natural_instinct),
                ..SpellModConfig::default()
            },
        );
    }

    /// +2% Intellect a rank in every form. The Cat and Bear form bonuses are applied
    /// dynamically in the form auras.
    fn apply_heart_of_the_wild(&mut self, sim: &mut Sim) {
        if self.tal.heart_of_the_wild == 0 {
            return;
        }
        let multiplier = Ladder::talent(17003, 5)
            .effect(crate::prepare::dbcenums::A_MOD_TOTAL_STAT_PERCENTAGE, 0)
            .multiplier_at(self.tal.heart_of_the_wild);
        sim.unit_mut(self.unit)
            .sdm
            .multiply_stat(Stat::Intellect, multiplier);
    }

    fn apply_feral_swiftness(&mut self, sim: &mut Sim) {
        if self.tal.feral_swiftness == 0 {
            return;
        }
        let mut bonus = Stats::default();
        bonus[Stat::DodgeRating] =
            crate::prepare::character::constants::DODGE_RATING_PER_DODGE_PERCENT
                * Ladder::talent(17002, 2)
                    .effect(A_MOD_DODGE_PERCENT, 0)
                    .value_at(self.tal.feral_swiftness);
        if let Some(aura) = self.cat_form_aura {
            sim.attach_stats_buff(aura, bonus);
        }
        if let Some(aura) = self.bear_form_aura {
            sim.attach_stats_buff(aura, bonus);
        }
    }

    /// Ferocity: one less Energy or Rage a rank on the feral builders.
    fn apply_ferocity(&mut self, sim: &mut Sim) {
        if self.tal.ferocity == 0 {
            return;
        }
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                class_mask: masks::CLAW
                    | masks::RAKE
                    | masks::PRIMAL_BITE
                    | masks::MAUL
                    | masks::SWIPE,
                kind: SpellModType::PowerCostFlat,
                int_value: -self.tal.ferocity,
                ..SpellModConfig::default()
            },
        );
    }

    /// Forever's Savage Fury names Shred, which Classic's did not.
    fn apply_savage_fury(&mut self, sim: &mut Sim) {
        if self.tal.savage_fury == 0 {
            return;
        }
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                class_mask: masks::CLAW | masks::RAKE | masks::SHRED | masks::MAUL | masks::SWIPE,
                kind: SpellModType::DamageDoneFlat,
                float_value: Ladder::talent(16998, 2)
                    .effect(A_ADD_PCT_MODIFIER, SPELLMOD_DAMAGE)
                    .fraction_at(self.tal.savage_fury),
                ..SpellModConfig::default()
            },
        );
    }

    /// Shredding Attacks: 6 less Energy a rank on Shred, and one less Rage a rank on Lacerate.
    fn apply_shredding_attacks(&mut self, sim: &mut Sim) {
        if self.tal.shredding_attacks == 0 {
            return;
        }
        let ladder = Ladder::talent(16966, 3);
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                class_mask: masks::SHRED,
                kind: SpellModType::PowerCostFlat,
                int_value: ladder.effect_at(1).value_at(self.tal.shredding_attacks) as i32,
                ..SpellModConfig::default()
            },
        );
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                class_mask: masks::LACERATE,
                kind: SpellModType::PowerCostFlat,
                int_value: (ladder.effect_at(2).value_at(self.tal.shredding_attacks) as i32) / 10,
                ..SpellModConfig::default()
            },
        );
    }

    /// Blood Frenzy (Primal Fury until build 70009): a combo point on a Cat builder crit and
    /// Rage on a Bear crit.
    fn apply_blood_frenzy(&mut self, sim: &mut Sim) {
        if self.tal.blood_frenzy == 0 {
            return;
        }
        let unit = self.unit;
        let proc_chance = Ladder::talent(16958, 2)
            .effect_at(1)
            .fraction_at(self.tal.blood_frenzy);
        let triggered = Ladder::ranked(&[16959]).highest();
        let action = ActionId::spell(triggered.id);

        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Blood Frenzy (Cat)".to_string(),
                action_id: action.clone(),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                class_spell_mask: masks::BUILDER,
                outcome: HitOutcome::CRIT,
                proc_chance,
                ..ProcTrigger::default()
            },
        );
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Blood Frenzy (Bear)".to_string(),
                action_id: action,
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                proc_mask: ProcMask::MELEE,
                outcome: HitOutcome::CRIT,
                proc_chance,
                ..ProcTrigger::default()
            },
        );
    }

    /// Forever repurposes Feral Instinct: Swipe hits for 10% more a rank.
    fn apply_feral_instincts(&mut self, sim: &mut Sim) {
        if self.tal.feral_instinct == 0 {
            return;
        }
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                class_mask: masks::SWIPE,
                kind: SpellModType::DamageDoneFlat,
                float_value: Ladder::talent(16947, 3)
                    .effect(A_ADD_PCT_MODIFIER, SPELLMOD_DAMAGE)
                    .fraction_at(self.tal.feral_instinct),
                ..SpellModConfig::default()
            },
        );
    }

    /// Natural Reaction, new in Forever: dodge chance, and a chance at Rage on every dodge.
    fn apply_natural_reaction(&mut self, sim: &mut Sim) {
        if self.tal.natural_reaction == 0 {
            return;
        }
        let unit = self.unit;
        let ladder = Ladder::talent(417051, 5);
        // unit.go 289: `unit.stats[stat] += amount` fuses the multiply into the add.
        let rate = crate::prepare::character::constants::DODGE_RATING_PER_DODGE_PERCENT;
        let value = ladder
            .effect(A_MOD_DODGE_PERCENT, 0)
            .value_at(self.tal.natural_reaction);
        let current = sim.unit(unit).stats[Stat::DodgeRating];
        sim.unit_mut(unit).stats[Stat::DodgeRating] = rate.mul_add(value, current);

        let proc_chance = ladder.effect_at(2).fraction_at(self.tal.natural_reaction);
        let triggered = Ladder::ranked(&[417053]).highest();
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Natural Reaction".to_string(),
                action_id: ActionId::spell(triggered.id),
                callback: CallbackMask::ON_SPELL_HIT_TAKEN,
                outcome: HitOutcome::DODGE,
                proc_chance,
                ..ProcTrigger::default()
            },
        );
    }

    /// Rend and Tear, new in Forever: +2% a rank to melee abilities against a bleeding target.
    fn apply_rend_and_tear(&mut self, sim: &mut Sim) {
        if self.tal.rend_and_tear == 0 {
            return;
        }
        for target in sim.all_units() {
            if sim.unit(target).unit_type == crate::prepare::sim::UnitType::Enemy {
                sim.add_dynamic_damage_taken_modifier(target);
            }
        }
    }

    /// Improved Shifting Power: the cooldown of Shifting Power.
    fn apply_improved_shifting_power(&mut self, sim: &mut Sim) {
        if self.tal.improved_shifting_power == 0 {
            return;
        }
        let value = Ladder::talent(1322670, 2)
            .effect(A_ADD_FLAT_MODIFIER, SPELLMOD_COOLDOWN)
            .value_at(self.tal.improved_shifting_power);
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                class_mask: masks::SHIFTING_POWER,
                kind: SpellModType::CooldownFlat,
                time_value: MILLISECOND * (value as i64),
                ..SpellModConfig::default()
            },
        );
    }

    /// Berserk, new in Forever (client 417141): 3 minute cooldown, and for 15 seconds +100%
    /// critical strike chance on the Combo Point builders.
    fn apply_berserk(&mut self, sim: &mut Sim) {
        if !self.tal.berserk {
            return;
        }
        let unit = self.unit;
        let action = ActionId::spell(417141);
        let crit_mod = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::BonusCritPercent,
                class_mask: masks::CLAW | masks::SHRED | masks::RAKE | masks::RAVAGE,
                float_value: 100.0,
                ..SpellModConfig::default()
            },
        );
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Berserk".to_string(),
                action_id: Some(action.clone()),
                duration: 15 * SECOND,
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.activate_spell_mod(crit_mod)
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.deactivate_spell_mod(crit_mod)
                })),
                ..AuraConfig::default()
            },
        );
        self.berserk_aura = Some(aura);

        let timer = sim.new_timer(unit);
        let spell = self.register_spell(
            sim,
            CAT | BEAR,
            SpellConfig {
                action_id: action,
                flags: SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::APL,
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: 180 * SECOND,
                    },
                    ..CastConfig::default()
                },
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
        self.berserk = Some(spell);
        sim.add_major_cooldown(
            unit,
            MajorCooldown {
                spell,
                priority: COOLDOWN_PRIORITY_DEFAULT,
                cooldown_type: cooldown_type::DPS,
                allow_spell_queueing: false,
                timings: Vec::new(),
            },
        );
    }
}
