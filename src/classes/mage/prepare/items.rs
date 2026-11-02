//! Go sim/mage/items.go: the Mage's item sets and the one item effect it registers.
//!
//! The shared item registry asks this module whether it knows a set bonus or an item: Go's
//! `core.NewItemSet` and `core.NewItemEffect` calls in `init`.

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use crate::prepare::character::{cooldown_type, MajorCooldown};
use crate::prepare::env::Environment;
use crate::prepare::item_sets::ItemSet;
use crate::prepare::sim::{AuraConfig, AuraId, Cooldown, Sim, UnitId, SECOND};
use crate::prepare::spell::{school, CastConfig, ProcMask, SpellConfig, SpellFlag};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::stats::{Stat, Stats};

use super::masks;

/// Aldor Regalia (648), Tirisfal Regalia (649) and Tempest Regalia (671): Go's
/// `core.NewItemSet` calls.
pub(crate) static ITEM_SETS: &[ItemSet] = &[
    ItemSet {
        id: 648,
        name: "Aldor Regalia",
        alternative_name: "",
        bonuses: &[(4, aldor_regalia_4)],
        required_profession: "",
    },
    ItemSet {
        id: 649,
        name: "Tirisfal Regalia",
        alternative_name: "",
        bonuses: &[(2, tirisfal_regalia_2), (4, tirisfal_regalia_4)],
        required_profession: "",
    },
    ItemSet {
        id: 671,
        name: "Tempest Regalia",
        alternative_name: "",
        bonuses: &[(2, tempest_regalia_2), (4, tempest_regalia_4)],
        required_profession: "",
    },
];

fn aldor_regalia_4(env: &mut Environment, aura: AuraId) {
    let sim = &mut env.sim;
    for (class_mask, seconds) in [
        (masks::PRESENCE_OF_MIND, -24),
        (masks::BLAST_WAVE, -4),
        (masks::ICE_BLOCK, -40),
    ] {
        sim.attach_spell_mod(
            aura,
            SpellModConfig {
                kind: SpellModType::CastTimeFlat,
                time_value: SECOND * seconds,
                class_mask,
                ..SpellModConfig::default()
            },
        );
    }
}

fn tirisfal_regalia_2(env: &mut Environment, aura: AuraId) {
    let sim = &mut env.sim;
    sim.attach_spell_mod(
        aura,
        SpellModConfig {
            kind: SpellModType::DamageDoneFlat,
            float_value: 0.20,
            class_mask: masks::ARCANE_BLAST,
            ..SpellModConfig::default()
        },
    );
    sim.attach_spell_mod(
        aura,
        SpellModConfig {
            kind: SpellModType::PowerCostPctAdd,
            float_value: 0.20,
            class_mask: masks::ARCANE_BLAST,
            ..SpellModConfig::default()
        },
    );
}

fn tirisfal_regalia_4(env: &mut Environment, aura: AuraId) {
    let unit = env.player;
    let sim = &mut env.sim;
    // Go's temporary stats aura, which the proc below activates in a fight.
    let _madness = sim.new_temporary_stats_aura(
        unit,
        "Arcane Madness",
        &ActionId {
            spell_id: 37444,
            ..ActionId::default()
        },
        Stats::from_pairs(&[(Stat::SpellDamage, 70.0)]),
        SECOND * 6,
    );
    sim.attach_proc_trigger(
        aura,
        &ProcTrigger {
            name: "Tirisfal 4PC".to_string(),
            callback: CallbackMask::ON_SPELL_HIT_DEALT,
            proc_mask: ProcMask::SPELL_DAMAGE,
            outcome: HitOutcome::CRIT,
            ..ProcTrigger::default()
        },
    );
}

fn tempest_regalia_2(env: &mut Environment, aura: AuraId) {
    let sim = &mut env.sim;
    sim.attach_spell_mod(
        aura,
        SpellModConfig {
            kind: SpellModType::DotNumberOfTicksFlat,
            int_value: 1,
            class_mask: masks::EVOCATION,
            ..SpellModConfig::default()
        },
    );
}

fn tempest_regalia_4(env: &mut Environment, aura: AuraId) {
    let sim = &mut env.sim;
    sim.attach_spell_mod(
        aura,
        SpellModConfig {
            kind: SpellModType::DamageDoneFlat,
            float_value: 0.05,
            class_mask: masks::FIREBALL | masks::FROSTBOLT | masks::ARCANE_MISSILES_TICK,
            ..SpellModConfig::default()
        },
    );
}

/// The Go `core.NewItemEffect(19959, ...)`: Hazza'rah's Charm of Magic. Use: increases the
/// critical hit chance of your Arcane spells by 5%, and the critical hit damage by 50% for
/// 20 sec (24544). The client's class mask names Arcane Explosion and Arcane Missiles only.
pub(crate) fn hazzarahs_charm(sim: &mut Sim, unit: UnitId) {
    let duration = SECOND * 20;

    let aura = sim.register_aura(
        unit,
        AuraConfig {
            label: "Arcane Potency".to_string(),
            action_id: Some(ActionId {
                spell_id: 24544,
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
            float_value: 5.0,
            class_mask: masks::ARCANE_EXPLOSION | masks::ARCANE_MISSILES,
            ..SpellModConfig::default()
        },
    );
    sim.attach_spell_mod(
        aura,
        SpellModConfig {
            kind: SpellModType::CritMultiplierFlat,
            float_value: 0.5,
            class_mask: masks::ARCANE_EXPLOSION | masks::ARCANE_MISSILES,
            ..SpellModConfig::default()
        },
    );

    let timer = sim.new_timer(unit);
    let shared = sim.get_offensive_trinket_cd(unit);
    let spell = sim.register_spell(
        unit,
        SpellConfig {
            action_id: ActionId {
                item_id: 19959,
                ..ActionId::default()
            },
            spell_school: school::ARCANE,
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
    use crate::prepare::sim::SECOND;
    use crate::prepare::spell::school;

    const REQUEST: &str =
        include_str!("../../../../tests/classes/mage/prepare/arcane.request.json");

    fn prepared_with_charm() -> Environment {
        let mut value: serde_json::Value = serde_json::from_str(REQUEST).expect("a request");
        let mut items = vec![serde_json::json!({}); 12];
        items.push(serde_json::json!({"id": 19959}));
        value["raid"]["parties"][0]["players"][0]["equipment"] =
            serde_json::json!({ "items": items });
        // The request's talents in client 1.60.1.70707's order, which Forever's rules read:
        // Arcane Focus's five points dropped.
        value["raid"]["parties"][0]["players"][0]["talentsString"] =
            serde_json::json!("050205103100301531-305000012-003");
        let bytes = serde_json::to_vec(&value).expect("json");
        let request = Request::from_json(&bytes).expect("parsed");
        Environment::new(request.message(), crate::classes::prepare_agent).expect("prepared")
    }

    /// Hazza'rah's Charm of Magic (items.go): an Arcane Potency aura with its two spell mods, and
    /// a use spell on a 3 minute cooldown that shares the 20 second offensive trinket timer.
    #[test]
    fn hazzarahs_charm_registers_its_aura_and_use_spell() {
        let env = prepared_with_charm();
        let sim = &env.sim;
        let aura = sim
            .get_aura(env.player, "Arcane Potency")
            .map(|id| sim.aura(id))
            .expect("the charm's aura");
        assert_eq!(aura.duration, 20 * SECOND);
        assert_eq!(aura.callback_names(), ["on_gain", "on_expire"]);
        let spell = sim
            .unit(env.player)
            .spellbook
            .iter()
            .map(|id| sim.spell(*id))
            .find(|spell| spell.action_id.item_id == 19959)
            .expect("the charm's use spell");
        assert_eq!(spell.cd.duration, 180 * SECOND);
        assert_eq!(spell.shared_cd.duration, 20 * SECOND);
        assert_eq!(spell.spell_school, school::ARCANE);
        let cooldowns = &sim.character(env.player).initial_major_cooldowns;
        assert!(cooldowns.iter().any(|mcd| {
            sim.spell(mcd.spell).action_id.item_id == 19959
                && mcd.cooldown_type == cooldown_type::DPS
        }));
    }
}
