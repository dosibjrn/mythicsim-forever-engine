//! Go sim/core/buffs/meta_test.go and the driver behaviour preparation can observe.

use super::dark_inspiration::{bid_own_dark_inspiration, DARK_INSPIRATION};
use super::drivers::attach_innervate_regen;
use super::generated::{BATTLE_SHOUT, GRACE_OF_AIR_TOTEM, INNERVATES, SUNDER_ARMOR, THORNS};
use super::paladin::{
    judgement_of_the_crusader_aura, judgement_of_the_crusader_max_rank,
    judgement_of_wisdom_max_rank, rank_name, retribution_aura_max_rank, JudgementRank,
    PaladinAuraRank,
};
use super::support::{
    add_generated_flat_bonus, apply_fixed_shout_aura, join_shared_category, new_damage_shield,
    DamageShield,
};
use super::*;
use crate::prepare::debuffs::scheduled_aura;
use crate::prepare::periodic_action::PeriodicActionOptions;
use crate::prepare::sim::{school_array_index, EnvState, Unit, UnitType, SECOND};
use crate::prepare::spell::school;
use crate::prepare::stats::{SchoolIndex, Stat};

fn sim_with_unit() -> (Sim, UnitId) {
    let mut sim = Sim::new();
    let mut unit = Unit::new(UnitType::Player, "player".to_string());
    unit.reaction_time = 1;
    let id = sim.add_unit(unit);
    (sim, id)
}

fn finalize(sim: &mut Sim, unit: UnitId) {
    let u = sim.unit_mut(unit);
    u.sdm.finalize_stat_deps();
    u.stats_without_deps = u.stats;
    u.stats = u
        .sdm
        .apply_stat_dependencies(u.stats_without_deps)
        .floor_game_stats();
    sim.state = EnvState::Finalized;
}

#[test]
fn a_rank_names_the_aura_only_when_a_paladin_cast_it() {
    assert_eq!(rank_name("Devotion Aura", 0), "Devotion Aura");
    assert_eq!(rank_name("Devotion Aura", 3), "Devotion Aura Rank 3");
    let ranked = Meta {
        rank: 2,
        ..BATTLE_SHOUT
    };
    assert_eq!(ranked.label_for(true), "Battle Shout Rank 2 (Player)");
    assert_eq!(BATTLE_SHOUT.label_for(false), "Battle Shout (External)");
}

#[test]
fn the_max_ranks_state_the_numbers_of_their_rows() {
    let retribution = retribution_aura_max_rank();
    assert_eq!((retribution.spell_id, retribution.rank), (10301, 0));
    assert!(retribution.value > 0.0);
    let crusader = judgement_of_the_crusader_max_rank();
    assert_eq!((crusader.spell_id, crusader.rank), (20303, 0));
    assert!(crusader.value > 0.0);
    // The judgement is the rank-0 debuff; its mana comes from the paired spell 20353.
    let wisdom = judgement_of_wisdom_max_rank();
    assert_eq!(wisdom.spell_id, 20355);
    assert!(wisdom.value > 0.0);
}

#[test]
fn a_damage_shield_registers_its_proc_spell_aura_trigger_and_bid() {
    let (mut sim, unit) = sim_with_unit();
    let aura = new_damage_shield(
        &mut sim,
        unit,
        DamageShield {
            label: "Thorns (External)".to_string(),
            action_id: ActionId {
                spell_id: 9910,
                tag: -1,
                ..ActionId::default()
            },
            duration: 0,
            category: "Thorns",
            single_aura: true,
            school: school::NATURE,
            damage: 18.0,
            bonus_coefficient: 0.0,
        },
    );
    // The shield's hit is a passive spell under the aura's action with the tag moved by two.
    let spell = sim.get_spell(
        unit,
        &ActionId {
            spell_id: 9910,
            tag: 1,
            ..ActionId::default()
        },
    );
    assert!(spell.is_some());
    let shield = sim.aura(aura);
    assert_eq!(shield.duration, NEVER_EXPIRES);
    assert_eq!(shield.build_phase, BuildPhase::BUFFS);
    assert!(shield.events.on_spell_hit_taken);
    let effect = shield.exclusive_effects[0];
    assert_eq!(sim.effects[effect.0].priority, 18.0);
    assert!(sim.categories[sim.effects[effect.0].category.0].single_aura);
}

#[test]
fn a_generated_damage_shield_reads_its_damage_off_the_row() {
    let (mut sim, unit) = sim_with_unit();
    let aura = THORNS.aura(&mut sim, unit, true, 0, 0.0);
    let shield = sim.aura(aura);
    assert_eq!(shield.label, "Thorns (Player)");
    assert_eq!(shield.tag, "");
    let effect = shield.exclusive_effects[0];
    assert_eq!(sim.effects[effect.0].priority, THORNS.value(0));
    assert_eq!(
        sim.categories[sim.effects[effect.0].category.0].name,
        "Thorns"
    );
}

#[test]
fn only_the_players_copy_joins_the_shared_category() {
    let (mut sim, unit) = sim_with_unit();
    let own = sim.register_aura(unit, aura("own", 0));
    let external = sim.register_aura(unit, aura("external", -1));
    join_shared_category(&mut sim, own, "PaladinAura", true);
    join_shared_category(&mut sim, external, "PaladinAura", false);
    assert_eq!(sim.aura(own).exclusive_effects.len(), 1);
    assert!(sim.aura(external).exclusive_effects.is_empty());
}

fn aura(label: &str, tag: i32) -> AuraConfig {
    AuraConfig {
        label: label.to_string(),
        action_id: Some(ActionId {
            spell_id: 1,
            tag,
            ..ActionId::default()
        }),
        duration: 10 * SECOND,
        ..AuraConfig::default()
    }
}

#[test]
fn a_flat_bonus_raises_both_the_amount_and_the_bid_once() {
    let (mut sim, unit) = sim_with_unit();
    let shout = sim.register_aura(
        unit,
        AuraConfig {
            tag: "BattleShout".to_string(),
            ..aura("Battle Shout (External)", -1)
        },
    );
    sim.new_exclusive_effect(shout, "BattleShout", true, 139.0, None, None);
    add_generated_flat_bonus(&mut sim, shout, Stat::AttackPower, 139.0, 30.0);
    let effect = sim.aura(shout).exclusive_effects[0];
    assert_eq!(sim.effects[effect.0].priority, 169.0);
    // A second wearer of the set asks for the same total and gets it once.
    add_generated_flat_bonus(&mut sim, shout, Stat::AttackPower, 139.0, 30.0);
    finalize(&mut sim, unit);
    sim.activate(shout);
    assert_eq!(sim.stat(unit, Stat::AttackPower), 30.0);
    sim.deactivate(shout);
    assert_eq!(sim.stat(unit, Stat::AttackPower), 0.0);
}

#[test]
#[should_panic(expected = "neither the 139 it is worth")]
fn a_flat_bonus_on_another_buffs_base_is_refused() {
    let (mut sim, unit) = sim_with_unit();
    let shout = sim.register_aura(
        unit,
        AuraConfig {
            tag: "BattleShout".to_string(),
            ..aura("Battle Shout (External)", -1)
        },
    );
    sim.new_exclusive_effect(shout, "BattleShout", true, 100.0, None, None);
    add_generated_flat_bonus(&mut sim, shout, Stat::AttackPower, 139.0, 30.0);
}

#[test]
#[should_panic(expected = "bids for its whole category")]
fn a_flat_bonus_needs_the_buff_to_bid_in_its_category() {
    let (mut sim, unit) = sim_with_unit();
    let shout = sim.register_aura(unit, aura("Battle Shout (External)", -1));
    add_generated_flat_bonus(&mut sim, shout, Stat::AttackPower, 139.0, 30.0);
}

#[test]
fn a_fixed_shout_chains_behind_the_players_own_when_the_aura_initializes() {
    let (mut sim, unit) = sim_with_unit();
    let own = sim.register_aura(
        unit,
        AuraConfig {
            tag: "BattleShout".to_string(),
            ..aura("Battle Shout (Player)", 0)
        },
    );
    let external = sim.register_aura(
        unit,
        AuraConfig {
            tag: "BattleShout".to_string(),
            ..aura("Battle Shout (External)", -1)
        },
    );
    apply_fixed_shout_aura(&mut sim, unit, external, "BattleShout");
    assert!(sim.aura(own).on_gain.is_none());
    sim.init_aura(external);
    assert!(sim.aura(own).on_gain.is_some());
    assert!(sim.aura(external).on_init.is_some());
}

#[test]
fn a_scheduled_aura_replaces_its_reset_and_stays_down() {
    let (mut sim, unit) = sim_with_unit();
    let debuff = sim.register_aura(unit, aura("Sunder Armor (External)", -1));
    sim.make_permanent(debuff);
    scheduled_aura(
        &mut sim,
        debuff,
        PeriodicActionOptions {
            period: 1500 * MILLISECOND,
            num_ticks: 5,
            tick_immediately: true,
        },
    );
    sim.reset_aura(debuff);
    assert!(!sim.aura(debuff).active);
    assert_eq!(sim.aura(debuff).duration, NEVER_EXPIRES);
}

#[test]
fn the_grace_of_air_party_copy_is_the_external_buff_of_its_row() {
    let (mut sim, unit) = sim_with_unit();
    let aura = GRACE_OF_AIR_TOTEM.aura(&mut sim, unit, false, 0, 0.0);
    let aura = sim.aura(aura);
    assert_eq!(aura.label, "Grace of Air Totem (External)");
    assert_eq!(aura.tag, "GraceOfAirTotem");
    assert_eq!(aura.build_phase, BuildPhase::BUFFS);
}

#[test]
fn innervate_forces_full_spirit_regen_while_it_is_up() {
    let (mut sim, unit) = sim_with_unit();
    let aura = INNERVATES.aura(&mut sim, unit, false, 0, 0.0);
    attach_innervate_regen(&mut sim, unit, aura);
    finalize(&mut sim, unit);
    let before = sim.unit(unit).pseudo_stats.spirit_regen_multiplier;
    sim.activate(aura);
    let pseudo = &sim.unit(unit).pseudo_stats;
    assert!(pseudo.force_full_spirit_regen);
    assert_eq!(pseudo.spirit_regen_multiplier, before * 5.0);
    sim.deactivate(aura);
    let pseudo = &sim.unit(unit).pseudo_stats;
    assert!(!pseudo.force_full_spirit_regen);
    assert_eq!(pseudo.spirit_regen_multiplier, before);
}

#[test]
fn judgement_of_the_crusader_adds_holy_damage_while_it_holds_the_category() {
    let (mut sim, target) = sim_with_unit();
    let rank = judgement_of_the_crusader_max_rank();
    let aura = judgement_of_the_crusader_aura(&mut sim, target, &rank);
    assert_eq!(sim.aura(aura).label, "Judgement of the Crusader");
    assert_eq!(sim.aura(aura).tag, "JudgementAura");
    finalize(&mut sim, target);
    let holy = school_array_index(SchoolIndex::Holy);
    sim.activate(aura);
    assert_eq!(
        sim.unit(target).pseudo_stats.school_bonus_spell_damage[holy],
        rank.value
    );
    sim.deactivate(aura);
    assert_eq!(
        sim.unit(target).pseudo_stats.school_bonus_spell_damage[holy],
        0.0
    );
    // The same rank asked for twice is the same aura.
    let again = judgement_of_the_crusader_aura(&mut sim, target, &rank);
    assert_eq!(again, aura);
}

#[test]
fn a_stronger_judgement_of_the_crusader_displaces_a_weaker_one() {
    let (mut sim, target) = sim_with_unit();
    let top = judgement_of_the_crusader_max_rank();
    let weak = JudgementRank {
        rank: 1,
        value: top.value / 2.0,
        ..top
    };
    let weak_aura = judgement_of_the_crusader_aura(&mut sim, target, &weak);
    let top_aura = judgement_of_the_crusader_aura(&mut sim, target, &top);
    assert_eq!(
        sim.aura(weak_aura).label,
        "Judgement of the Crusader Rank 1"
    );
    finalize(&mut sim, target);
    let holy = school_array_index(SchoolIndex::Holy);
    sim.activate(weak_aura);
    sim.activate(top_aura);
    // A category that holds one aura at a time deactivates the copy it outbids.
    assert!(!sim.aura(weak_aura).active);
    assert!(sim.aura(top_aura).active);
    assert_eq!(
        sim.unit(target).pseudo_stats.school_bonus_spell_damage[holy],
        top.value
    );
}

#[test]
fn sunder_armor_is_a_stacking_single_aura_debuff() {
    let (mut sim, target) = sim_with_unit();
    let aura = SUNDER_ARMOR.aura(&mut sim, target, false, 0, 0.0);
    let sunder = sim.aura(aura);
    assert_eq!(sunder.label, "Sunder Armor (External)");
    assert_eq!(sunder.tag, "MajorArmorReduction");
    assert_eq!(sunder.max_stacks, 5);
    assert_eq!(sunder.build_phase, BuildPhase::NONE);
}

#[test]
fn a_retribution_aura_shield_scales_with_the_spell_power_of_its_holder() {
    let (mut sim, unit) = sim_with_unit();
    let rank = PaladinAuraRank {
        spell_id: 10301,
        rank: 0,
        value: 26.0,
    };
    let aura = super::paladin::retribution_aura_buff(&mut sim, unit, false, &rank);
    let effect = sim.aura(aura).exclusive_effects[0];
    // The rank's own damage is the priority, and the coefficient is the holder's to apply.
    assert_eq!(sim.effects[effect.0].priority, 26.0);
    assert_eq!(sim.aura(aura).label, "Retribution Aura (External)");
    // Asking again finds the aura rather than registering a second shield.
    let again = super::paladin::retribution_aura_buff(&mut sim, unit, false, &rank);
    assert_eq!(again, aura);
}

/// Go reads every field of the four buff messages (the generated tables, the drivers and
/// `applyDebuffs`), and the class agents write some of them; a field Rust does not read would be
/// silently dropped, so a field the pinned proto gains must be handled or refused here first.
#[test]
fn every_buff_and_debuff_field_is_read_by_a_table_or_a_driver() {
    let sources = [
        include_str!("generated.rs"),
        include_str!("drivers.rs"),
        include_str!("../debuffs.rs"),
    ]
    .join("\n");
    for message in [
        "proto.RaidBuffs",
        "proto.PartyBuffs",
        "proto.IndividualBuffs",
        "proto.Debuffs",
    ] {
        let mut number = 1;
        while let Some((name, _)) = crate::contracts::request::field_by_number(message, number) {
            // Retired: Retribution Aura reads its holder's spell power, so the row stays only to
            // keep the party fields after it numbered.
            let retired = name == "retribution_aura_spell_power";
            assert!(
                retired || sources.contains(&format!("\"{name}\"")),
                "{message}.{name} is read by no buff table or driver"
            );
            number += 1;
        }
        assert!(number > 1, "{message} has fields");
    }
}

/// wowsims/forever meta_test.go "DarkInspiration": each rank is its share of the row's 15%
/// Spirit, rounded half away from zero, and an untaken talent reads the whole row. A priest's own
/// copy, priced by the talent's ladder, is worth the same at each rank.
#[test]
fn dark_inspiration_is_its_ranks_share_of_the_rows_spirit() {
    for (rank, percent) in [
        (0, 15.0),
        (1, 3.0),
        (2, 6.0),
        (3, 9.0),
        (4, 12.0),
        (5, 15.0),
    ] {
        let want = 1.0 + percent / 100.0;
        assert!(
            (DARK_INSPIRATION.value(rank) - want).abs() < 1e-9,
            "rank {rank}: {} for {want}",
            DARK_INSPIRATION.value(rank)
        );
    }
    let talent = Ladder::talent(1326159, 5);
    for rank in 1..=5 {
        assert!((talent.multiplier_at(rank) - DARK_INSPIRATION.value(rank)).abs() < 1e-9);
    }
}

/// wowsims/forever buffs_generated_test.go TestGeneratedDarkInspirationTakesTheTalentsRank: the
/// party's copy at each rank raises 1000 Spirit to floor(1000 * its value) for the whole fight.
#[test]
fn the_partys_dark_inspiration_takes_the_talents_rank() {
    for (rank, want) in [
        (1, 1030.0),
        (2, 1060.0),
        (3, 1090.0),
        (4, 1120.0),
        (5, 1150.0),
    ] {
        let (mut sim, unit) = sim_with_unit();
        sim.unit_mut(unit).stats[Stat::Spirit] = 1000.0;
        // `permanent`, as `apply_party_dark_inspiration` applies it.
        let aura = DARK_INSPIRATION.aura(&mut sim, unit, false, rank, 0.0);
        let aura = sim.make_permanent(aura);
        assert_eq!(sim.aura(aura).label, "Dark Inspiration (External)");
        assert_eq!(sim.aura(aura).duration, NEVER_EXPIRES);
        finalize(&mut sim, unit);
        sim.activate(aura);
        assert_eq!(sim.stat(unit, Stat::Spirit), want, "rank {rank}");
        assert_eq!(want, (1000.0 * DARK_INSPIRATION.value(rank)).floor());
    }
}

/// wowsims/forever TestGeneratedDarkInspirationAndAPriestsCopyDoNotStack: the party's copy and a
/// priest's own bid in one Spirit category, so the priest has the stronger amount alone. The
/// party's copy stays up throughout: a stronger own copy holds the Spirit while it is up and the
/// party's holds it again once it ends, and an equal or weaker one never takes it.
#[test]
fn the_partys_dark_inspiration_and_a_priests_own_do_not_stack() {
    for (name, party_rank, own_rank) in [
        ("stronger priest", 3, 5),
        ("equal bids", 5, 5),
        ("weaker priest", 5, 3),
    ] {
        let (mut sim, unit) = sim_with_unit();
        sim.unit_mut(unit).stats[Stat::Spirit] = 1000.0;
        let party = DARK_INSPIRATION.aura(&mut sim, unit, false, party_rank, 0.0);
        // The priest's aura is the buff row's own spell, so only yielding keeps an equal bid off.
        let own = sim.register_aura(
            unit,
            AuraConfig {
                action_id: Some(ActionId::spell(1326168)),
                ..aura("Dark Inspiration", 0)
            },
        );
        let talent = Ladder::talent(1326159, 5);
        bid_own_dark_inspiration(&mut sim, unit, own, talent.multiplier_at(own_rank));
        finalize(&mut sim, unit);
        sim.activate(party);
        let spirit_at = |rank: i32| (1000.0 * DARK_INSPIRATION.value(rank)).floor();
        let holder = |sim: &Sim| {
            let effect = sim.aura(party).exclusive_effects[0];
            sim.category_active_aura(sim.effects[effect.0].category)
        };
        assert_eq!(
            sim.aura(party).exclusive_effects.len(),
            1,
            "{name}: Spirit's bid alone"
        );
        assert_eq!(
            sim.stat(unit, Stat::Spirit),
            spirit_at(party_rank),
            "{name} alone"
        );
        assert_eq!(holder(&sim), Some(party), "{name} alone");

        sim.activate(own);
        let stronger = if own_rank > party_rank { own } else { party };
        assert_eq!(
            sim.stat(unit, Stat::Spirit),
            spirit_at(own_rank.max(party_rank)),
            "{name} under the priest's copy"
        );
        assert_eq!(
            holder(&sim),
            Some(stronger),
            "{name} under the priest's copy"
        );
        assert!(sim.aura(party).active, "{name}: the party's copy stays up");

        sim.deactivate(own);
        assert_eq!(
            sim.stat(unit, Stat::Spirit),
            spirit_at(party_rank),
            "{name} after"
        );
        assert_eq!(holder(&sim), Some(party), "{name} after");
    }
}
