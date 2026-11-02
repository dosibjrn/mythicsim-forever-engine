//! Go sim/druid's Balance spells (starfire.go, wrath.go, moonfire.go, insect_swarm.go,
//! hurricane.go) and talents_balance.go.

use std::rc::Rc;

use crate::classes::druid::forms::{HUMANOID, MOONKIN};
use crate::contracts::prepared_v2::ActionId;
use crate::prepare::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger, PseudoStatField};
use crate::prepare::buffs::generated::INSECT_SWARM;
use crate::prepare::dbcenums::{
    A_ADD_FLAT_MODIFIER, A_ADD_PCT_MODIFIER, A_MOD_CASTING_SPEED_NOT_STACK, A_MOD_CRIT_PCT,
    A_MOD_DAMAGE_PERCENT_DONE, A_MOD_HIT_CHANCE, A_MOD_SPELL_HIT_CHANCE, A_PERIODIC_DUMMY,
    SPELLMOD_CASTING_TIME, SPELLMOD_COST, SPELLMOD_CRITICAL_CHANCE, SPELLMOD_CRIT_DAMAGE_BONUS,
    SPELLMOD_DAMAGE, SPELLMOD_DOT, SPELLMOD_GLOBAL_COOLDOWN, SPELLMOD_RANGE,
};
use crate::prepare::sim::{AuraConfig, EventCallbacks, Rules, Sim, MILLISECOND};
use crate::prepare::spell::{
    Cast, CastConfig, CostOptions, DotConfig, ProcMask, SpellConfig, SpellFlag,
};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::spelldata::{Ladder, Spell};
use crate::prepare::stats::{SchoolIndex, Stat};

use super::baseline::{aura_array_to_map, new_enemy_aura_array};
use super::{masks, Druid, Spec};

const STARFIRE_RANKS: [i32; 7] = [2912, 8949, 8950, 8951, 9875, 9876, 25298];
const WRATH_RANKS: [i32; 8] = [5176, 5177, 5178, 5179, 5180, 6780, 8905, 9912];
const MOONFIRE_RANKS: [i32; 10] = [8921, 8924, 8925, 8926, 8927, 8928, 8929, 9833, 9834, 9835];
const INSECT_SWARM_RANKS: [i32; 5] = [5570, 24974, 24975, 24976, 24977];
const HURRICANE_RANKS: [i32; 3] = [16914, 17401, 17402];
const HURRICANE_TRIGGERED: [i32; 3] = [1278965, 1278968, 1278759];

fn ms(value: f64) -> i64 {
    // time.Millisecond * time.Duration(value)
    MILLISECOND * (value as i64)
}

fn flat_mana(rank: &Spell) -> CostOptions {
    CostOptions {
        mana_flat_cost: rank.cost() as i32,
        ..CostOptions::default()
    }
}

impl Druid {
    /// Go `RegisterBalanceSpells`.
    pub(super) fn register_balance_spells(&mut self, sim: &mut Sim) {
        for rank in STARFIRE_RANKS {
            self.register_starfire_spell(sim, Ladder::ranked(&[rank]).highest());
        }
        self.register_moonfire_spell(sim);
        for rank in WRATH_RANKS {
            self.register_wrath_spell(sim, rank);
        }
        self.register_hurricane_spell(sim);
        self.register_faerie_fire_spell(sim);
    }

    /// Go `registerStarfireSpell`: every rank is registered.
    fn register_starfire_spell(&mut self, sim: &mut Sim, rank: &Spell) {
        let spell = self.register_spell(
            sim,
            HUMANOID | MOONKIN,
            SpellConfig {
                action_id: ActionId::spell(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::SPELL_DAMAGE,
                class_spell_mask: masks::STARFIRE,
                flags: SpellFlag::APL,
                rank: rank.rank_number(),
                max_range: f64::from(rank.max_range),
                cost: flat_mana(rank),
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: rank.gcd(),
                        cast_time: rank.cast_time(),
                        ..Cast::default()
                    },
                    ..CastConfig::default()
                },
                bonus_coefficient: rank.damage_effect().coeff(),
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        );
        self.starfire.push(spell);
    }

    /// Go `registerWrathSpell`: every rank is registered; `druid.Wrath` stays the highest.
    fn register_wrath_spell(&mut self, sim: &mut Sim, id: i32) {
        let rank = Ladder::ranked(&[id]).highest();
        let spell = self.register_spell(
            sim,
            HUMANOID | MOONKIN,
            SpellConfig {
                action_id: ActionId::spell(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::SPELL_DAMAGE,
                class_spell_mask: masks::WRATH,
                flags: SpellFlag::APL,
                missile_speed: f64::from(rank.speed),
                rank: rank.rank_number(),
                cost: flat_mana(rank),
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: rank.gcd(),
                        cast_time: rank.cast_time(),
                        ..Cast::default()
                    },
                    ..CastConfig::default()
                },
                bonus_coefficient: rank.damage_effect().coeff(),
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                max_range: f64::from(rank.max_range),
                ..SpellConfig::default()
            },
        );
        if id == *WRATH_RANKS.last().expect("ranks") {
            self.wrath = Some(spell);
        }
    }

    /// Go `registerMoonfireSpell`.
    pub(super) fn register_moonfire_spell(&mut self, sim: &mut Sim) {
        let rank = Ladder::ranked(&MOONFIRE_RANKS).highest();
        let tick = rank.periodic_effect();
        let impact = self.register_spell(
            sim,
            HUMANOID | MOONKIN,
            SpellConfig {
                action_id: ActionId::spell(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::SPELL_DAMAGE,
                class_spell_mask: masks::MOONFIRE_INITIAL,
                flags: SpellFlag::APL,
                rank: rank.rank_number(),
                cost: flat_mana(rank),
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: rank.gcd(),
                        ..Cast::default()
                    },
                    ..CastConfig::default()
                },
                bonus_coefficient: rank.damage_effect().coeff(),
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                max_range: f64::from(rank.max_range),
                ..SpellConfig::default()
            },
        );
        self.moonfire = Some(impact);

        let dot = sim.register_spell(
            self.unit,
            SpellConfig {
                action_id: super::with_tag(rank.id, 1),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::SPELL_DAMAGE,
                class_spell_mask: masks::MOONFIRE_DOT,
                flags: SpellFlag::PASSIVE_SPELL,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                dot: DotConfig {
                    aura: AuraConfig {
                        label: "Moonfire".to_string(),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: (rank.duration() / tick.period()) as i32,
                    tick_length: tick.period(),
                    affected_by_cast_speed: false,
                    bonus_coefficient: tick.coeff(),
                    ..DotConfig::default()
                },
                ..SpellConfig::default()
            },
        );
        sim.spell_mut(impact).related_dot_spell = Some(dot);
    }

    /// Go `registerInsectSwarmSpell`.
    fn register_insect_swarm_spell(&mut self, sim: &mut Sim) {
        let rank = Ladder::ranked(&INSECT_SWARM_RANKS).highest();
        let tick = rank.periodic_effect();
        let auras = new_enemy_aura_array(sim, |sim, target| {
            INSECT_SWARM.aura(sim, target, true, 0, 0.0)
        });
        let spell = self.register_spell(
            sim,
            HUMANOID | MOONKIN,
            SpellConfig {
                action_id: ActionId::spell(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::SPELL_DAMAGE,
                class_spell_mask: masks::INSECT_SWARM,
                flags: SpellFlag::APL | SpellFlag::BINARY,
                rank: rank.rank_number(),
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                max_range: f64::from(rank.max_range),
                cost: flat_mana(rank),
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: rank.gcd(),
                        ..Cast::default()
                    },
                    ..CastConfig::default()
                },
                dot: DotConfig {
                    aura: AuraConfig {
                        label: "Insect Swarm".to_string(),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: (rank.duration() / tick.period()) as i32,
                    tick_length: tick.period(),
                    affected_by_cast_speed: false,
                    bonus_coefficient: tick.coeff(),
                    ..DotConfig::default()
                },
                related_aura_arrays: aura_array_to_map(sim, &auras),
                ..SpellConfig::default()
            },
        );
        self.insect_swarm = Some(spell);
    }

    /// Go `registerHurricaneSpell`.
    fn register_hurricane_spell(&mut self, sim: &mut Sim) {
        let rank = Ladder::ranked(&HURRICANE_RANKS).highest();
        // The tick length is on Hurricane's own periodic dummy; the damage is the spell
        // HurricaneTriggered casts each tick.
        let tick_length = rank.effect(A_PERIODIC_DUMMY, 0).period();
        let tick_spell = Ladder::ranked(&HURRICANE_TRIGGERED).highest();
        let tick = tick_spell.damage_effect();

        let hurricane = self.register_spell(
            sim,
            HUMANOID | MOONKIN,
            SpellConfig {
                action_id: ActionId::spell(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::SPELL_DAMAGE,
                flags: SpellFlag::CHANNELED | SpellFlag::APL,
                class_spell_mask: masks::HURRICANE,
                max_range: f64::from(rank.max_range),
                rank: rank.rank_number(),
                cost: flat_mana(rank),
                // Forever states no cooldown on Hurricane, so the spell has none.
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: rank.gcd(),
                        ..Cast::default()
                    },
                    ..CastConfig::default()
                },
                dot: DotConfig {
                    is_aoe: true,
                    aura: AuraConfig {
                        label: "Hurricane (Aura)".to_string(),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: (rank.duration() / tick_length) as i32,
                    tick_length,
                    affected_by_cast_speed: true,
                    ..DotConfig::default()
                },
                ..SpellConfig::default()
            },
        );
        self.hurricane = Some(hurricane);

        let tick_proc = sim.register_spell(
            self.unit,
            SpellConfig {
                action_id: ActionId::spell(tick_spell.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::SPELL_DAMAGE,
                class_spell_mask: masks::HURRICANE,
                // The tick is its own client row that the channel triggers, a proc rather
                // than a cast.
                flags: SpellFlag::PROC,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                bonus_coefficient: tick.coeff(),
                ..SpellConfig::default()
            },
        );
        sim.spell_mut(hurricane).related_dot_spell = Some(tick_proc);
    }

    /// Go `registerBalanceTalents`.
    pub(super) fn register_balance_talents(&mut self, sim: &mut Sim) {
        // Tier 1
        self.apply_improved_wrath(sim);
        self.apply_genesis(sim);

        // Tier 2
        self.apply_moonglow(sim);
        self.apply_improved_moonfire(sim);
        self.apply_natures_majesty(sim);
        self.apply_natures_reach(sim);

        // Tier 3: Improved Entangling Roots is not modelled.
        self.apply_natures_splendor(sim);

        // Tier 4
        if self.tal.insect_swarm {
            self.register_insect_swarm_spell(sim);
        }
        self.apply_vengeance(sim);
        self.apply_improved_starfire(sim);

        // Tier 5: Overgrowth is not modelled.
        self.apply_natures_grace(sim);
        self.apply_eclipse(sim);

        // Tier 6
        self.apply_moonfury(sim);
    }

    fn apply_moonfury(&mut self, sim: &mut Sim) {
        if self.tal.moonfury == 0 {
            return;
        }
        // Forever states Moonfury as +2% damage a rank to the Arcane and Nature schools (mask
        // 72) rather than as a spell modifier.
        let multiplier = Ladder::talent(16896, 5)
            .effect(A_MOD_DAMAGE_PERCENT_DONE, 72)
            .multiplier_at(self.tal.moonfury);
        let pseudo = &mut sim.unit_mut(self.unit).pseudo_stats;
        *PseudoStatField::SchoolDamageDealtMultiplier(SchoolIndex::Arcane).get_mut(pseudo) *=
            multiplier;
        *PseudoStatField::SchoolDamageDealtMultiplier(SchoolIndex::Nature).get_mut(pseudo) *=
            multiplier;
    }

    fn apply_moonglow(&mut self, sim: &mut Sim) {
        if self.tal.moonglow == 0 {
            return;
        }
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                class_mask: masks::MOONFIRE
                    | masks::STARFIRE
                    | masks::WRATH
                    | masks::INSECT_SWARM
                    | masks::HURRICANE,
                kind: SpellModType::PowerCostPctAdd,
                float_value: Ladder::talent(16845, 3)
                    .effect(A_ADD_PCT_MODIFIER, SPELLMOD_COST)
                    .fraction_at(self.tal.moonglow),
                ..SpellModConfig::default()
            },
        );
    }

    /// Go `applyNaturesGrace`: Forever replaces the cast time reduction with a short haste buff.
    fn apply_natures_grace(&mut self, sim: &mut Sim) {
        if !self.tal.natures_grace {
            return;
        }
        let unit = self.unit;
        let triggered = Ladder::ranked(&[16886]).highest();
        let haste_multiplier = 1.0
            + triggered
                .effect(A_MOD_CASTING_SPEED_NOT_STACK, 0)
                .base_value()
                / 100.0;

        // Effect 1 also cuts the global cooldown by 10%, on top of the haste. Every spell in
        // its mask has the default 1.5 sec GCD, so the percentage is taken off that.
        let gcd_reduction = (crate::prepare::spell::GCD_DEFAULT as f64
            * triggered
                .effect(A_ADD_PCT_MODIFIER, SPELLMOD_GLOBAL_COOLDOWN)
                .percent()) as i64;

        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Nature's Grace".to_string(),
                action_id: Some(ActionId::spell(triggered.id)),
                duration: triggered.duration(),
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.multiply_cast_speed(unit, haste_multiplier)
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.multiply_cast_speed(unit, 1.0 / haste_multiplier)
                })),
                ..AuraConfig::default()
            },
        );
        sim.attach_spell_mod(
            aura,
            SpellModConfig {
                kind: SpellModType::GlobalCooldownFlat,
                class_mask: masks::ENTANGLING_ROOTS
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
                    | masks::MARK_OF_THE_WILD,
                time_value: gcd_reduction,
                ..SpellModConfig::default()
            },
        );

        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Nature's Grace Trigger".to_string(),
                action_id: ActionId::spell(Ladder::ranked(&[16880]).highest().id),
                metrics_action_id: ActionId::default(),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                class_spell_mask: masks::DAMAGING_SPELLS,
                outcome: HitOutcome::CRIT,
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );
    }

    fn apply_vengeance(&mut self, sim: &mut Sim) {
        if self.tal.vengeance == 0 {
            return;
        }
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                class_mask: masks::DAMAGING_SPELLS,
                kind: SpellModType::CritMultiplierFlat,
                float_value: Ladder::talent(16909, 5)
                    .effect(A_ADD_PCT_MODIFIER, SPELLMOD_CRIT_DAMAGE_BONUS)
                    .fraction_at(self.tal.vengeance),
                ..SpellModConfig::default()
            },
        );
    }

    /// Nature's Reach: 16819 adds 2/4% hit, and 10/20% to the range of every spell its class mask
    /// reaches (Wrath, Starfire, Moonfire, Insect Swarm, Hurricane, Faerie Fire, Entangling Roots,
    /// Thorns). The sim's range mod is flat yards, so the share is taken of each spell's own range as
    /// it registers.
    fn apply_natures_reach(&mut self, sim: &mut Sim) {
        if self.tal.natures_reach == 0 {
            return;
        }
        let ladder = Ladder::talent(16819, 2);
        let share = ladder
            .effect(A_ADD_PCT_MODIFIER, SPELLMOD_RANGE)
            .fraction_at(self.tal.natures_reach);
        let reached = masks::WRATH
            | masks::STARFIRE
            | masks::MOONFIRE_INITIAL
            | masks::MOONFIRE_DOT
            | masks::INSECT_SWARM
            | masks::HURRICANE
            | masks::FAERIE_FIRE
            | masks::ENTANGLING_ROOTS
            | masks::THORNS;
        sim.on_spell_registered(
            self.unit,
            Rc::new(move |sim: &mut Sim, spell| {
                if sim.spell(spell).matches(reached) {
                    sim.spell_mut(spell).max_range *= 1.0 + share;
                }
            }),
        );
        let hit = ladder
            .effect(A_MOD_HIT_CHANCE, 0)
            .value_at(self.tal.natures_reach);
        sim.add_stat(self.unit, Stat::PhysicalHitPercent, hit);
        let spell_hit = ladder
            .effect(A_MOD_SPELL_HIT_CHANCE, 0)
            .value_at(self.tal.natures_reach);
        sim.add_stat(self.unit, Stat::SpellHitPercent, spell_hit);
    }

    fn apply_improved_moonfire(&mut self, sim: &mut Sim) {
        if self.tal.improved_moonfire == 0 {
            return;
        }
        let ladder = Ladder::talent(16821, 2);
        // 5% a point damage increase to Moonfire and its DoT.
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                class_mask: masks::MOONFIRE,
                kind: SpellModType::DamageDoneFlat,
                float_value: ladder
                    .effect(A_ADD_PCT_MODIFIER, SPELLMOD_DAMAGE)
                    .fraction_at(self.tal.improved_moonfire),
                ..SpellModConfig::default()
            },
        );
        // 5% a point chance to crit with Moonfire.
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                class_mask: masks::MOONFIRE,
                kind: SpellModType::BonusCritPercent,
                float_value: ladder
                    .effect(A_ADD_FLAT_MODIFIER, SPELLMOD_CRITICAL_CHANCE)
                    .value_at(self.tal.improved_moonfire),
                ..SpellModConfig::default()
            },
        );
    }

    /// Improved Wrath, new in Forever: 0.1 sec off the cast and 10% off the cost a rank.
    fn apply_improved_wrath(&mut self, sim: &mut Sim) {
        if self.tal.improved_wrath == 0 {
            return;
        }
        let ladder = Ladder::talent(16814, 5);
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                class_mask: masks::WRATH,
                kind: SpellModType::CastTimeFlat,
                time_value: ms(ladder
                    .effect(A_ADD_FLAT_MODIFIER, SPELLMOD_CASTING_TIME)
                    .value_at(self.tal.improved_wrath)),
                ..SpellModConfig::default()
            },
        );
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                class_mask: masks::WRATH,
                kind: SpellModType::PowerCostPctAdd,
                float_value: ladder
                    .effect(A_ADD_PCT_MODIFIER, SPELLMOD_COST)
                    .fraction_at(self.tal.improved_wrath),
                ..SpellModConfig::default()
            },
        );
    }

    /// Genesis, new in Forever: +1% periodic damage and healing a rank, +2% under Forever's rules
    /// since hotfix push 113531.
    fn apply_genesis(&mut self, sim: &mut Sim) {
        if self.tal.genesis == 0 {
            return;
        }
        // Client 1223081's mask includes the bleeds (Rake, Rip, Lacerate) and not Lifebloom.
        // Its first effect also names Wild Growth, and its second, a direct healing mod, names
        // Swiftmend, which MythicSim's healer sim casts: the Restoration druid's alone.
        let healer = self.spec == Spec::Restoration && sim.rules == Rules::Forever;
        let wild_growth = if healer { masks::WILD_GROWTH } else { 0 };
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                class_mask: masks::DOT
                    | masks::RAKE
                    | masks::RIP
                    | masks::LACERATE
                    | masks::REJUVENATION
                    | masks::REGROWTH
                    | wild_growth,
                kind: SpellModType::DotDamageDonePct,
                float_value: super::talents::genesis(sim.rules)
                    .effect(A_ADD_PCT_MODIFIER, SPELLMOD_DOT)
                    .fraction_at(self.tal.genesis),
                ..SpellModConfig::default()
            },
        );
        if healer {
            sim.add_static_mod(
                self.unit,
                SpellModConfig {
                    class_mask: masks::SWIFTMEND,
                    kind: SpellModType::DamageDoneFlat,
                    float_value: super::talents::genesis(sim.rules)
                        .effect(A_ADD_PCT_MODIFIER, SPELLMOD_DAMAGE)
                        .fraction_at(self.tal.genesis),
                    ..SpellModConfig::default()
                },
            );
        }
    }

    /// Nature's Majesty, new in Forever: +2% critical strike chance a rank.
    fn apply_natures_majesty(&mut self, sim: &mut Sim) {
        if self.tal.natures_majesty == 0 {
            return;
        }
        let crit = Ladder::talent(1223082, 2)
            .effect(A_MOD_CRIT_PCT, 0)
            .value_at(self.tal.natures_majesty);
        sim.add_stat(self.unit, Stat::SpellCritPercent, crit);
        sim.add_stat(self.unit, Stat::PhysicalCritPercent, crit);
    }

    /// Moonfire ticks every 3 sec and Insect Swarm every 2 sec, so the added duration is one
    /// extra tick on each.
    fn apply_natures_splendor(&mut self, sim: &mut Sim) {
        if !self.tal.natures_splendor {
            return;
        }
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                class_mask: masks::DOT,
                kind: SpellModType::DotNumberOfTicksFlat,
                int_value: 1,
                ..SpellModConfig::default()
            },
        );
    }

    /// Improved Starfire, new in Forever: 0.1 sec off the cast a rank.
    fn apply_improved_starfire(&mut self, sim: &mut Sim) {
        if self.tal.improved_starfire == 0 {
            return;
        }
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                class_mask: masks::STARFIRE,
                kind: SpellModType::CastTimeFlat,
                time_value: ms(Ladder::talent(16850, 5)
                    .effect(A_ADD_FLAT_MODIFIER, SPELLMOD_CASTING_TIME)
                    .value_at(self.tal.improved_starfire)),
                ..SpellModConfig::default()
            },
        );
    }

    /// Eclipse, new in Forever: every Wrath banks charges that each shorten one Starfire cast.
    fn apply_eclipse(&mut self, sim: &mut Sim) {
        if self.tal.eclipse == 0 {
            return;
        }
        let unit = self.unit;
        let triggered = Ladder::ranked(&[408255]).highest();
        let cast_time_reduction = ms(Ladder::talent(408248, 3)
            .effect_at(2)
            .value_at(self.tal.eclipse));

        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Eclipse".to_string(),
                action_id: Some(ActionId::spell(triggered.id)),
                duration: triggered.duration(),
                max_stacks: 4,
                ..AuraConfig::default()
            },
        );
        sim.attach_spell_mod(
            aura,
            SpellModConfig {
                class_mask: masks::STARFIRE,
                kind: SpellModType::CastTimeFlat,
                time_value: cast_time_reduction,
                ..SpellModConfig::default()
            },
        );
        self.eclipse_aura = Some(aura);

        let trigger = sim.register_aura(
            unit,
            AuraConfig {
                label: "Eclipse Trigger".to_string(),
                events: EventCallbacks {
                    on_cast_complete: true,
                    ..EventCallbacks::default()
                },
                ..AuraConfig::default()
            },
        );
        sim.make_permanent(trigger);
    }
}
