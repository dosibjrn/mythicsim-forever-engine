//! Go sim/druid's Feral Bear spells (barkskin.go, demoralizing_roar.go, enrage.go,
//! frenzied_regeneration.go, lacerate.go, primal_bite.go, maul.go, swipe.go).

use std::rc::Rc;

use crate::classes::druid::forms::{ANY, BEAR};
use crate::contracts::prepared_v2::ActionId;
use crate::prepare::aura_helpers::PseudoStatField;
use crate::prepare::buffs::generated::DEMORALIZING_ROAR;
use crate::prepare::character::constants::{CHARACTER_LEVEL, MAX_MELEE_RANGE};
use crate::prepare::character::{cooldown_type, MajorCooldown};
use crate::prepare::dbcenums::A_MOD_DAMAGE_PERCENT_TAKEN;
use crate::prepare::hotfix_113554;
use crate::prepare::major_cooldown::COOLDOWN_PRIORITY_DEFAULT;
use crate::prepare::sim::{AuraConfig, Cooldown, Duration, Rules, Sim, MILLISECOND};
use crate::prepare::spell::{
    school, Cast, CastConfig, CostOptions, DefenseType, DotConfig, ProcMask, SpellConfig, SpellFlag,
};
use crate::prepare::spelldata::{Ladder, Spell};
use crate::prepare::stats::{SchoolIndex, Stat};

use super::baseline::{aura_array_to_map, new_enemy_aura_array};
use super::{masks, Druid};

const DEMORALIZING_ROAR_RANKS: [i32; 5] = [99, 1735, 9490, 9747, 9898];
const LACERATE_RANKS: [i32; 3] = [414644, 1235826, 1235827];
const PRIMAL_BITE_RANKS: [i32; 4] = [407995, 1238069, 1238070, 1238073];
const MAUL_RANKS: [i32; 7] = [6807, 6808, 6809, 8972, 9745, 9880, 9881];
const SWIPE_RANKS: [i32; 5] = [779, 780, 769, 9754, 9908];

/// The Primal Bite rank a level 60 druid casts: rank 4 (1238073) in the frozen data, rank 3
/// (1238070) under Forever's rules, where hotfix push 113531 removes rank 4 and gives rank 3 its
/// level and its 77 flat damage.
pub(super) fn primal_bite_rank(rules: Rules) -> &'static Spell {
    match rules {
        Rules::Forever => hotfix_113554::row(rules, 1238070),
        Rules::GoReference => Ladder::ranked(&PRIMAL_BITE_RANKS).highest(),
    }
}

/// Go `LacerateMaxStacks`.
pub(crate) const LACERATE_MAX_STACKS: i32 = 5;
/// maul.go's realism cooldown.
pub(crate) const MAUL_REALISM: Duration = 50 * MILLISECOND;

fn rage_cost(rank: &Spell, refund: f64) -> CostOptions {
    CostOptions {
        rage_cost: rank.cost() as i32,
        rage_refund: refund,
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

impl Druid {
    /// Go `RegisterFeralTankSpells`.
    pub(super) fn register_feral_tank_spells(&mut self, sim: &mut Sim) {
        self.register_bear_form_spell(sim);
        self.register_barkskin(sim);
        self.register_demoralizing_roar_spell(sim);
        // Forever drops Faerie Fire (Feral); the Balance version is the only one.
        self.register_faerie_fire_spell(sim);
        self.register_enrage_spell(sim);
        self.register_frenzied_regeneration_spell(sim);
        self.register_lacerate_spell(sim);
        self.register_primal_bite_spell(sim);
        self.register_maul_spell(sim);
        self.register_swipe_bear_spell(sim);
    }

    /// Go `registerBarkskin`: 20% less Physical damage taken.
    fn register_barkskin(&mut self, sim: &mut Sim) {
        let unit = self.unit;
        let rank = Ladder::ranked(&[22812]).highest();
        let action = ActionId::spell(rank.id);
        let multiplier = 1.0 + rank.effect(A_MOD_DAMAGE_PERCENT_TAKEN, 1).base_value() / 100.0;

        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Barkskin".to_string(),
                action_id: Some(action.clone()),
                duration: rank.duration(),
                ..AuraConfig::default()
            },
        );
        sim.attach_multiplicative_pseudo_stat_buff(
            aura,
            PseudoStatField::SchoolDamageTakenMultiplier(SchoolIndex::Physical),
            multiplier,
        );

        let timer = sim.new_timer(unit);
        let spell = self.register_spell(
            sim,
            ANY,
            SpellConfig {
                action_id: action,
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                flags: SpellFlag::APL,
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: rank.cooldown().max(rank.category_cooldown()),
                    },
                    default_cast: Cast {
                        gcd: rank.gcd(),
                        ..Cast::default()
                    },
                    ..CastConfig::default()
                },
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
        self.barkskin = Some(spell);
        sim.add_major_cooldown(
            unit,
            MajorCooldown {
                spell,
                priority: COOLDOWN_PRIORITY_DEFAULT,
                cooldown_type: cooldown_type::SURVIVAL,
                allow_spell_queueing: false,
                timings: Vec::new(),
            },
        );
    }

    /// Go `registerDemoralizingRoarSpell`.
    fn register_demoralizing_roar_spell(&mut self, sim: &mut Sim) {
        // registerDemoralizingRoarAura: Forever has no Feral Aggression node, so there are no
        // talent points to pass.
        let auras = new_enemy_aura_array(sim, |sim, target| {
            DEMORALIZING_ROAR.aura(sim, target, true, 0, 0.0)
        });
        self.demoralizing_roar_auras = auras.clone();

        let rank = Ladder::ranked(&DEMORALIZING_ROAR_RANKS).highest();
        let spell = self.register_spell(
            sim,
            BEAR,
            SpellConfig {
                action_id: ActionId::spell(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::EMPTY,
                class_spell_mask: masks::DEMORALIZING_ROAR,
                flags: SpellFlag::APL,
                cost: rage_cost(rank, 0.0),
                cast: gcd_cast(rank),
                threat_multiplier: 1.0,
                // Two threat a level, the sim's long-standing value; the client states none.
                flat_threat_bonus: 2.0 * f64::from(CHARACTER_LEVEL),
                related_aura_arrays: aura_array_to_map(sim, &auras),
                ..SpellConfig::default()
            },
        );
        self.demoralizing_roar = Some(spell);
    }

    /// Go `registerEnrageSpell`: 2 Rage a second for 10 sec and an instant 10 Rage up front.
    fn register_enrage_spell(&mut self, sim: &mut Sim) {
        let unit = self.unit;
        let rank = Ladder::ranked(&[5229]).highest();
        let action = ActionId::spell(rank.id);

        const ARMOR_MULTIPLIER: f64 = 1.0 - 0.27;
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Enrage".to_string(),
                action_id: Some(action.clone()),
                duration: rank.duration(),
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.apply_dynamic_equip_scaling(unit, Stat::Armor, ARMOR_MULTIPLIER);
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.apply_dynamic_equip_scaling(unit, Stat::Armor, 1.0 / ARMOR_MULTIPLIER);
                })),
                ..AuraConfig::default()
            },
        );
        self.st.enrage_aura.set(Some(aura));

        let timer = sim.new_timer(unit);
        let spell = self.register_spell(
            sim,
            BEAR,
            SpellConfig {
                action_id: action,
                class_spell_mask: masks::ENRAGE,
                flags: SpellFlag::APL,
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: rank.cooldown().max(rank.category_cooldown()),
                    },
                    ignore_haste: true,
                    ..CastConfig::default()
                },
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
        self.enrage = Some(spell);
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

    /// Go `registerFrenziedRegenerationSpell`.
    fn register_frenzied_regeneration_spell(&mut self, sim: &mut Sim) {
        let unit = self.unit;
        let rank = Ladder::ranked(&[22842]).highest();
        let action = ActionId::spell(rank.id);

        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Frenzied Regeneration".to_string(),
                action_id: Some(action.clone()),
                duration: rank.duration(),
                ..AuraConfig::default()
            },
        );
        self.st.frenzied_regeneration_aura.set(Some(aura));

        let timer = sim.new_timer(unit);
        let spell = self.register_spell(
            sim,
            BEAR,
            SpellConfig {
                action_id: action,
                spell_school: rank.spell_school(),
                proc_mask: ProcMask::EMPTY,
                class_spell_mask: masks::FRENZIED_REGENERATION,
                flags: SpellFlag::APL | SpellFlag::HELPFUL,
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: rank.gcd(),
                        ..Cast::default()
                    },
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: rank.cooldown().max(rank.category_cooldown()),
                    },
                    ignore_haste: true,
                    ..CastConfig::default()
                },
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
        self.frenzied_regeneration = Some(spell);
        sim.add_major_cooldown(
            unit,
            MajorCooldown {
                spell,
                priority: COOLDOWN_PRIORITY_DEFAULT,
                cooldown_type: cooldown_type::SURVIVAL,
                allow_spell_queueing: false,
                timings: Vec::new(),
            },
        );
    }

    /// Go `registerLacerateSpell`. Hotfix push 113531 makes Lacerate a talent (1328617), so under
    /// Forever's rules a druid without it has no Lacerate. Its two new low ranks leave the level
    /// 60 rank 1235827 as it was.
    fn register_lacerate_spell(&mut self, sim: &mut Sim) {
        if sim.rules == Rules::Forever && !self.tal.lacerate {
            return;
        }
        let rank = Ladder::ranked(&LACERATE_RANKS).highest();
        let tick = rank.periodic_effect();
        let spell = self.register_spell(
            sim,
            BEAR,
            SpellConfig {
                action_id: ActionId::spell(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                class_spell_mask: masks::LACERATE,
                flags: SpellFlag::MELEE_METRICS | SpellFlag::APL,
                rank: rank.rank_number(),
                cost: rage_cost(rank, rank.miss_refund()),
                cast: gcd_cast(rank),
                damage_multiplier: 1.0,
                threat_multiplier: 3.33,
                max_range: MAX_MELEE_RANGE,
                dot: DotConfig {
                    aura: AuraConfig {
                        label: "Lacerate".to_string(),
                        max_stacks: LACERATE_MAX_STACKS,
                        duration: rank.duration(),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: (rank.duration() / tick.period()) as i32,
                    tick_length: tick.period(),
                    ..DotConfig::default()
                },
                ..SpellConfig::default()
            },
        );
        self.lacerate = Some(spell);
    }

    /// Go `registerPrimalBiteSpell`.
    fn register_primal_bite_spell(&mut self, sim: &mut Sim) {
        if !self.tal.primal_bite {
            return;
        }
        let unit = self.unit;
        let rank = primal_bite_rank(sim.rules);
        let timer = sim.new_timer(unit);
        let mut cast = gcd_cast(rank);
        cast.cd = Cooldown {
            timer: Some(timer),
            duration: rank.cooldown().max(rank.category_cooldown()),
        };
        let spell = self.register_spell(
            sim,
            BEAR,
            SpellConfig {
                action_id: ActionId::spell(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                class_spell_mask: masks::PRIMAL_BITE,
                flags: SpellFlag::MELEE_METRICS | SpellFlag::APL,
                rank: rank.rank_number(),
                cost: rage_cost(rank, rank.miss_refund()),
                cast,
                damage_multiplier: 1.0,
                threat_multiplier: 1.5,
                max_range: MAX_MELEE_RANGE,
                ..SpellConfig::default()
            },
        );
        self.primal_bite = Some(spell);
    }

    /// Go `registerMaulSpell`: the strike that fires on the next swing and the queue spell and
    /// aura a rotation presses.
    fn register_maul_spell(&mut self, sim: &mut Sim) {
        let unit = self.unit;
        let rank = Ladder::ranked(&MAUL_RANKS).highest();
        let strike = self.register_spell(
            sim,
            BEAR,
            SpellConfig {
                action_id: ActionId::spell(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                class_spell_mask: masks::MAUL,
                flags: SpellFlag::MELEE_METRICS,
                rank: rank.rank_number(),
                cost: rage_cost(rank, rank.miss_refund()),
                cast: CastConfig {
                    default_cast: Cast {
                        non_empty: true,
                        ..Cast::default()
                    },
                    ..CastConfig::default()
                },
                damage_multiplier: 1.0,
                threat_multiplier: 1.75,
                max_range: MAX_MELEE_RANGE,
                ..SpellConfig::default()
            },
        );
        // The strike is held in a local variable in Go, not a Druid field: the exporter lists no
        // form for it.
        self.form_masks.retain(|(spell, _)| *spell != strike);
        self.maul_strike = Some(strike);

        // makeMaulQueueSpellAndAura.
        let queue_action = super::with_tag(rank.id, 1);
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Maul Queue Aura".to_string(),
                action_id: Some(queue_action.clone()),
                duration: crate::prepare::sim::NEVER_EXPIRES,
                on_reset: Some(Rc::new(|_: &mut Sim, _| {})),
                on_gain: Some(Rc::new(|_: &mut Sim, _| {})),
                on_expire: Some(Rc::new(|_: &mut Sim, _| {})),
                ..AuraConfig::default()
            },
        );
        self.st.maul_queue_aura.set(Some(aura));
        // druid.maulRealismICD: a cooldown with a timer of its own.
        sim.new_timer(unit);

        let queue = self.register_spell(
            sim,
            BEAR,
            SpellConfig {
                action_id: queue_action,
                spell_school: school::PHYSICAL,
                defense_type: DefenseType::Melee,
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                flags: SpellFlag::MELEE_METRICS | SpellFlag::APL | SpellFlag::NO_METRICS,
                cast: CastConfig {
                    default_cast: Cast {
                        non_empty: true,
                        ..Cast::default()
                    },
                    ..CastConfig::default()
                },
                ..SpellConfig::default()
            },
        );
        self.maul = Some(queue);
    }

    /// Go `registerSwipeBearSpell`.
    fn register_swipe_bear_spell(&mut self, sim: &mut Sim) {
        let rank = Ladder::ranked(&SWIPE_RANKS).highest();
        let spell = self.register_spell(
            sim,
            BEAR,
            SpellConfig {
                action_id: ActionId::spell(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                class_spell_mask: masks::SWIPE,
                flags: SpellFlag::MELEE_METRICS | SpellFlag::APL,
                cost: rage_cost(rank, rank.miss_refund()),
                cast: gcd_cast(rank),
                damage_multiplier: 1.0,
                // Season of Discovery's "Modifies Threat +101%", which the client does not carry.
                threat_multiplier: 2.0,
                max_range: MAX_MELEE_RANGE,
                ..SpellConfig::default()
            },
        );
        self.swipe = Some(spell);
    }
}
