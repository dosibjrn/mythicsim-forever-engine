//! Go sim/warlock/items.go: the Warlock's item sets and the item effects it registers.
//!
//! The shared item registry asks this module whether it knows a set bonus or an item: Go's
//! `core.NewItemSet` and `core.NewItemEffect` calls in `init`.

use std::rc::Rc;

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use crate::prepare::character::{cooldown_type, MajorCooldown};
use crate::prepare::env::Environment;
use crate::prepare::item_sets::ItemSet;
use crate::prepare::sim::{AuraConfig, AuraId, Cooldown, Rules, Sim, UnitId, MILLISECOND, SECOND};
use crate::prepare::spell::{school, CastConfig, DefenseType, SpellConfig, SpellFlag};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::spelldata::must_find;
use crate::prepare::stats::{Stat, Stats};

use super::masks;

/// Oblivion Raiment (644), Voidheart Raiment (645), Corruptor Raiment (646) and Malefic Raiment
/// (670): Go's `core.NewItemSet` calls.
pub(crate) static ITEM_SETS: &[ItemSet] = &[
    ItemSet {
        id: 644,
        name: "Oblivion Raiment",
        alternative_name: "",
        bonuses: &[(2, oblivion_raiment_2)],
        required_profession: "",
    },
    ItemSet {
        id: 645,
        name: "Voidheart Raiment",
        alternative_name: "",
        bonuses: &[(2, voidheart_raiment_2), (4, voidheart_raiment_4)],
        required_profession: "",
    },
    ItemSet {
        id: 646,
        name: "Corruptor Raiment",
        alternative_name: "",
        bonuses: &[(2, corruptor_raiment_2), (4, corruptor_raiment_4)],
        required_profession: "",
    },
    ItemSet {
        id: 670,
        name: "Malefic Raiment",
        alternative_name: "",
        bonuses: &[(2, malefic_raiment_2), (4, malefic_raiment_4)],
        required_profession: "",
    },
];

/// Dungeon Set 3, 2pc: grants your pet 45 mana per 5 sec (37375). Go returns for any other
/// class.
fn oblivion_raiment_2(env: &mut Environment, aura: AuraId) {
    if env.sim.character(env.player).class != "ClassWarlock" {
        return;
    }
    let pets = env.sim.unit(env.player).pets.clone();
    let on_gain_pets = pets.clone();
    env.sim.apply_on_gain(
        aura,
        Rc::new(move |sim: &mut Sim, _| {
            for pet in &on_gain_pets {
                sim.add_stat_dynamic(*pet, Stat::MP5, 45.0);
            }
        }),
    );
    env.sim.apply_on_expire(
        aura,
        Rc::new(move |sim: &mut Sim, _| {
            for pet in &pets {
                sim.add_stat_dynamic(*pet, Stat::MP5, -45.0);
            }
        }),
    );
}

/// T4, 2pc: your shadow damage spells have a chance to grant you 135 bonus shadow damage for 15
/// sec (37379), and your fire damage spells the same in fire (39437).
fn voidheart_raiment_2(env: &mut Environment, aura: AuraId) {
    let unit = env.player;
    let sim = &mut env.sim;
    sim.new_temporary_stats_aura(
        unit,
        "Flameshadow",
        &ActionId {
            spell_id: 37379,
            ..ActionId::default()
        },
        Stats::from_pairs(&[(Stat::ShadowDamage, 135.0)]),
        SECOND * 15,
    );
    sim.new_temporary_stats_aura(
        unit,
        "Shadowflame Hellfire and RoF",
        &ActionId {
            spell_id: 39437,
            ..ActionId::default()
        },
        Stats::from_pairs(&[(Stat::FireDamage, 135.0)]),
        SECOND * 15,
    );

    sim.attach_proc_trigger(
        aura,
        &ProcTrigger {
            name: "Voidheart Raiment 2pc".to_string(),
            proc_chance: 0.05,
            callback: CallbackMask::ON_SPELL_HIT_DEALT,
            require_damage_dealt: true,
            ..ProcTrigger::default()
        },
    );
}

/// T4, 4pc: increases the duration of your Corruption and Immolate abilities by 3 sec (37380).
fn voidheart_raiment_4(env: &mut Environment, aura: AuraId) {
    let sim = &mut env.sim;
    sim.attach_spell_mod(
        aura,
        SpellModConfig {
            kind: SpellModType::DotNumberOfTicksFlat,
            int_value: 1,
            class_mask: masks::CORRUPTION | masks::IMMOLATE_DOT,
            ..SpellModConfig::default()
        },
    );
    sim.expose_to_apl(aura, 37380);
}

/// T5, 2pc: causes your pet to be healed for 15% of the damage you deal (37381).
fn corruptor_raiment_2(env: &mut Environment, aura: AuraId) {
    env.sim.attach_proc_trigger(
        aura,
        &ProcTrigger {
            name: "Corruptor Raiment 2pc - Pet Healing".to_string(),
            // 37381 carries the bit.
            can_proc_from_procs: true,
            action_id: ActionId {
                spell_id: 37381,
                ..ActionId::default()
            },
            outcome: HitOutcome::LANDED,
            callback: CallbackMask::ON_SPELL_HIT_DEALT,
            ..ProcTrigger::default()
        },
    );
}

/// T5, 4pc: your Shadowbolt spell hits increase the damage of Corruption by 10% and your
/// Incinerate spell hits increase the damage of Immolate by 10% (37384).
fn corruptor_raiment_4(env: &mut Environment, aura: AuraId) {
    let unit = env.player;
    let sim = &mut env.sim;
    sim.on_spell_registered(
        unit,
        Rc::new(move |sim: &mut Sim, spell| {
            if !sim
                .spell(spell)
                .matches(masks::CORRUPTION | masks::IMMOLATE_DOT)
            {
                return;
            }
            for target in super::spells::target_units(sim) {
                let index = sim.unit(target).unit_index as usize;
                let dot = sim.spell(spell).dots.get(index).copied().flatten();
                if let Some(dot) = dot {
                    let dot_aura = sim.dots[dot.0].aura;
                    sim.apply_on_gain(dot_aura, Rc::new(|_: &mut Sim, _| {}));
                }
            }
        }),
    );

    sim.attach_proc_trigger(
        aura,
        &ProcTrigger {
            name: "Corruptor Raiment 4pc - Improved Corruption and Immolate".to_string(),
            action_id: ActionId {
                spell_id: 37384,
                ..ActionId::default()
            },
            class_spell_mask: masks::SHADOW_BOLT | masks::INCINERATE,
            outcome: HitOutcome::LANDED,
            callback: CallbackMask::ON_SPELL_HIT_DEALT,
            ..ProcTrigger::default()
        },
    );
    sim.expose_to_apl(aura, 37384);
}

/// T6, 2pc: each time one of your Corruption or Immolate spells deals periodic damage, you heal
/// 70 health (38394).
fn malefic_raiment_2(env: &mut Environment, aura: AuraId) {
    env.sim.attach_proc_trigger(
        aura,
        &ProcTrigger {
            name: "Malefic Raiment 2pc - Dot Heals".to_string(),
            action_id: ActionId {
                spell_id: 38394,
                ..ActionId::default()
            },
            class_spell_mask: masks::CORRUPTION | masks::IMMOLATE_DOT,
            callback: CallbackMask::ON_PERIODIC_DAMAGE_DEALT,
            ..ProcTrigger::default()
        },
    );
}

/// T6, 4pc: increases damage done by shadowbolt and incinerate by 6% (38393).
fn malefic_raiment_4(env: &mut Environment, aura: AuraId) {
    let sim = &mut env.sim;
    sim.attach_spell_mod(
        aura,
        SpellModConfig {
            kind: SpellModType::DamageDoneFlat,
            float_value: 0.06,
            class_mask: masks::SHADOW_BOLT | masks::INCINERATE,
            ..SpellModConfig::default()
        },
    );
    sim.expose_to_apl(aura, 38393);
}

/// The demon aura of The Black Book under Forever's rules, the client row's name.
pub(crate) const BLACK_BOOK_DEMON_AURA: &str = "Blessing of the Black Book";

/// Blessing of the Black Book's client row (client 70707).
pub(crate) const BLACK_BOOK_BUFF: i32 = 23720;

/// The Go `core.NewItemEffect(19337, ...)`: The Black Book.
pub(crate) fn black_book(sim: &mut Sim, unit: UnitId) {
    if sim.rules == Rules::Forever {
        black_book_forever(sim, unit);
        return;
    }
    let duration = SECOND * 30;

    // Go `warlock.Pets`: every pet of the warlock.
    for pet in sim.unit(unit).pets.clone() {
        sim.new_temporary_stats_aura(
            pet,
            "Blessing of The Black Book",
            &ActionId {
                spell_id: 23720,
                ..ActionId::default()
            },
            Stats::from_pairs(&[
                (Stat::SpellDamage, 200.0),
                (Stat::AttackPower, 325.0),
                (Stat::Armor, 1600.0),
            ]),
            duration,
        );
    }

    let aura = sim.register_aura(
        unit,
        AuraConfig {
            label: "Blessing of The Black Book".to_string(),
            action_id: Some(ActionId {
                item_id: 19337,
                ..ActionId::default()
            }),
            duration,
            on_gain: Some(Rc::new(|_: &mut Sim, _| {})),
            ..AuraConfig::default()
        },
    );

    let timer = sim.new_timer(unit);
    let shared = sim.get_offensive_trinket_cd(unit);
    let spell = sim.register_spell(
        unit,
        SpellConfig {
            action_id: ActionId {
                item_id: 19337,
                ..ActionId::default()
            },
            defense_type: DefenseType::Magic,
            flags: SpellFlag::NO_ON_CAST_COMPLETE,
            cast: CastConfig {
                cd: Cooldown {
                    timer: Some(timer),
                    duration: SECOND * 180,
                },
                shared_cd: Cooldown {
                    timer: Some(shared),
                    duration,
                },
                ..CastConfig::default()
            },
            has_extra_cast_condition: true,
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
    sim.character_mut(unit).cooldown_buff_auras.push((
        spell,
        crate::prepare::aura_helpers::StatBuffAura::new(
            aura,
            vec![Stat::SpellDamage, Stat::AttackPower, Stat::Armor],
        ),
    ));
}

/// The Black Book under Forever's rules, as the client states it and the reference's current
/// items.go registers it: Blessing of the Black Book (23720) lands on the demon that is out, every
/// effect of its row on the pet (implicit target 5): 100% more damage done in every school
/// (aura 79, misc 127) and 100% more armor (aura 101) for 30 sec. The use waits for a demon out,
/// on the item's own 5 minute cooldown and its category's 30 seconds (1141), which the
/// offensive trinket timer stands for. The demon's aura of each pet is plain here: the runtime
/// multiplies the damage (`black_book`), and nothing swings at the demon, so its armor is read
/// by nothing.
fn black_book_forever(sim: &mut Sim, unit: UnitId) {
    let row = must_find(BLACK_BOOK_BUFF);
    let duration = row.duration();
    for pet in sim.unit(unit).pets.clone() {
        sim.register_aura(
            pet,
            AuraConfig {
                label: BLACK_BOOK_DEMON_AURA.to_string(),
                action_id: Some(ActionId {
                    spell_id: BLACK_BOOK_BUFF,
                    ..ActionId::default()
                }),
                duration,
                ..AuraConfig::default()
            },
        );
    }
    let on_use = crate::prepare::items::database_item(19337)
        .and_then(|item| item.item_effects.into_iter().next())
        .and_then(|effect| effect.message("on_use").cloned())
        .expect("The Black Book has a use");
    let timer = sim.new_timer(unit);
    let shared = sim.get_offensive_trinket_cd(unit);
    let spell = sim.register_spell(
        unit,
        SpellConfig {
            action_id: ActionId {
                item_id: 19337,
                ..ActionId::default()
            },
            defense_type: DefenseType::Magic,
            flags: SpellFlag::NO_ON_CAST_COMPLETE,
            cast: CastConfig {
                cd: Cooldown {
                    timer: Some(timer),
                    duration: i64::from(on_use.i32("cooldown_ms")) * MILLISECOND,
                },
                shared_cd: Cooldown {
                    timer: Some(shared),
                    duration: i64::from(on_use.i32("category_cooldown_ms")) * MILLISECOND,
                },
                ..CastConfig::default()
            },
            has_extra_cast_condition: true,
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

/// The Go `core.NewItemEffect(19957, ...)`: Hazza'rah's Charm of Destruction. Use: increases the
/// critical hit chance of your Destruction spells by 10% for 20 sec (24543). The client's class
/// mask leaves out Incinerate, which Classic's Destruction flag took in.
pub(crate) fn hazzarahs_charm_of_destruction(sim: &mut Sim, unit: UnitId) {
    let duration = SECOND * 20;

    let aura = sim.register_aura(
        unit,
        AuraConfig {
            label: "Massive Destruction".to_string(),
            action_id: Some(ActionId {
                spell_id: 24543,
                ..ActionId::default()
            }),
            duration,
            ..AuraConfig::default()
        },
    );
    sim.attach_spell_mod(
        aura,
        SpellModConfig {
            kind: SpellModType::BonusCritPercent,
            float_value: 10.0,
            class_mask: masks::DESTRUCTION_SPELLS & !masks::INCINERATE,
            ..SpellModConfig::default()
        },
    );

    let timer = sim.new_timer(unit);
    let shared = sim.get_offensive_trinket_cd(unit);
    let spell = sim.register_spell(
        unit,
        SpellConfig {
            action_id: ActionId {
                item_id: 19957,
                ..ActionId::default()
            },
            spell_school: school::FIRE,
            flags: SpellFlag::NO_ON_CAST_COMPLETE,
            cast: CastConfig {
                cd: Cooldown {
                    timer: Some(timer),
                    duration: SECOND * 180,
                },
                shared_cd: Cooldown {
                    timer: Some(shared),
                    duration,
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

#[cfg(test)]
mod tests {
    use crate::contracts::request::Request;
    use crate::prepare::character::cooldown_type;
    use crate::prepare::env::Environment;
    use crate::prepare::sim::{Rules, SECOND};
    use crate::prepare::spell::school;

    const REQUEST: &str =
        include_str!("../../../../tests/classes/warlock/prepare/affliction-succubus.request.json");

    fn prepared_with_trinkets() -> Environment {
        prepared_with_trinkets_under(Rules::Forever)
    }

    fn prepared_with_trinkets_under(rules: Rules) -> Environment {
        let mut value: serde_json::Value = serde_json::from_str(REQUEST).expect("a request");
        let mut items = vec![serde_json::json!({}); 12];
        items.push(serde_json::json!({"id": 19337}));
        items.push(serde_json::json!({"id": 19957}));
        value["raid"]["parties"][0]["players"][0]["equipment"] =
            serde_json::json!({ "items": items });
        let bytes = serde_json::to_vec(&value).expect("json");
        let request = Request::from_json(&bytes).expect("parsed");
        Environment::with_rules(request.message(), crate::classes::prepare_agent, rules)
            .expect("prepared")
    }

    /// The Black Book (items.go at the reference pin): a Blessing aura on the warlock, one on
    /// each demon, and a use spell on a 3 minute cooldown that shares the 30 second offensive
    /// trinket timer.
    #[test]
    fn the_black_book_registers_its_auras_and_use_spell() {
        let env = prepared_with_trinkets_under(Rules::GoReference);
        let sim = &env.sim;
        let aura = sim
            .get_aura(env.player, "Blessing of The Black Book")
            .map(|id| sim.aura(id))
            .expect("the warlock's aura");
        assert_eq!(aura.duration, 30 * SECOND);
        assert_eq!(aura.callback_names(), ["on_gain"]);
        for pet in &sim.unit(env.player).pets {
            assert!(sim.get_aura(*pet, "Blessing of The Black Book").is_some());
        }
        let spell = sim
            .unit(env.player)
            .spellbook
            .iter()
            .map(|id| sim.spell(*id))
            .find(|spell| spell.action_id.item_id == 19337)
            .expect("the use spell");
        assert_eq!(spell.cd.duration, 180 * SECOND);
        assert_eq!(spell.shared_cd.duration, 30 * SECOND);
        assert!(spell.has_extra_cast_condition);
    }

    /// The Black Book under Forever's rules (client 70707): the client row's Blessing on each
    /// demon and none on the warlock, and a use on the item's 5 minute cooldown that shares the
    /// 30 second offensive trinket timer and waits for a demon out.
    #[test]
    fn the_forever_black_book_blesses_only_the_demon() {
        let env = prepared_with_trinkets();
        let sim = &env.sim;
        assert!(sim
            .get_aura(env.player, "Blessing of The Black Book")
            .is_none());
        let pets = &sim.unit(env.player).pets;
        assert!(!pets.is_empty());
        for pet in pets {
            let aura = sim
                .get_aura(*pet, super::BLACK_BOOK_DEMON_AURA)
                .map(|id| sim.aura(id))
                .expect("the demon's aura");
            assert_eq!(aura.duration, 30 * SECOND);
        }
        let spell = sim
            .unit(env.player)
            .spellbook
            .iter()
            .map(|id| sim.spell(*id))
            .find(|spell| spell.action_id.item_id == 19337)
            .expect("the use spell");
        assert_eq!(spell.cd.duration, 300 * SECOND);
        assert_eq!(spell.shared_cd.duration, 30 * SECOND);
        assert!(spell.has_extra_cast_condition);
    }

    /// Hazza'rah's Charm of Destruction (items.go): Massive Destruction with its crit mod and a
    /// use spell on the burst trinket timer, a DPS cooldown.
    #[test]
    fn the_destruction_charm_registers_its_aura_and_use_spell() {
        let env = prepared_with_trinkets();
        let sim = &env.sim;
        let aura = sim
            .get_aura(env.player, "Massive Destruction")
            .map(|id| sim.aura(id))
            .expect("the charm's aura");
        assert_eq!(aura.duration, 20 * SECOND);
        let spell = sim
            .unit(env.player)
            .spellbook
            .iter()
            .map(|id| sim.spell(*id))
            .find(|spell| spell.action_id.item_id == 19957)
            .expect("the use spell");
        assert_eq!(spell.spell_school, school::FIRE);
        assert_eq!(spell.cd.duration, 180 * SECOND);
        assert_eq!(spell.shared_cd.duration, 20 * SECOND);
        let cooldowns = &sim.character(env.player).initial_major_cooldowns;
        assert!(cooldowns.iter().any(|mcd| {
            sim.spell(mcd.spell).action_id.item_id == 19957
                && mcd.cooldown_type == cooldown_type::DPS
        }));
    }
}
