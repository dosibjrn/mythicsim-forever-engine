//! Go sim/core/racials_test.go, disable_racials_test.go and the weapon specialization checks of
//! racial_test.go, on a warrior built through the same construction Go's tests use.

use super::*;
use crate::contracts::request::{Message, Request};
use crate::prepare::agent::{ClassSpellName, PrepAgent};
use crate::prepare::sim::{school_array_index, RageBar, SpellId};

#[derive(Clone)]
struct FakeAgent {
    talents: Message,
}

impl PrepAgent for FakeAgent {
    fn talents(&self) -> &Message {
        &self.talents
    }
    fn class_spells(&self) -> &'static [ClassSpellName] {
        &[]
    }
}

/// A warrior with a rage bar and two spells for Eureka! to act on: an ability the class tags
/// and a spell it does not.
fn factory(sim: &mut Sim, unit: UnitId, _player: &Message) -> Result<Box<dyn PrepAgent>, Refusal> {
    sim.unit_mut(unit).rage_bar = RageBar {
        enabled: true,
        max_rage: 100.0,
        ..RageBar::default()
    };
    for (id, flags, class_spell_mask) in [(1, SpellFlag::NONE, 1), (2, SpellFlag::APL, 0)] {
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: ActionId::spell(id),
                class_spell_mask,
                proc_mask: ProcMask::MELEE_MH_SPECIAL,
                spell_school: school::PHYSICAL,
                flags,
                damage_multiplier: 1.0,
                ..SpellConfig::default()
            },
        );
    }
    Ok(Box::new(FakeAgent {
        talents: Message::empty("proto.WarriorTalents"),
    }))
}

fn environment(race: &str, player: &str, mob_type: &str) -> Environment {
    environment_with(Rules::Forever, race, player, mob_type)
}

fn environment_with(rules: Rules, race: &str, player: &str, mob_type: &str) -> Environment {
    let request = format!(
        r#"{{"simOptions":{{"iterations":1,"randomSeed":"100"}},
        "raid":{{"parties":[{{"players":[{{"name":"Warrior","class":"ClassWarrior",
          "race":"{race}","buffs":{{}},"consumables":{{}}{player}}}]}}]}},
        "encounter":{{"duration":180,"targets":[{{"level":63,"mobType":"{mob_type}"}}]}}}}"#
    );
    let request = Request::from_json(request.as_bytes()).expect("a valid request");
    match Environment::with_rules(request.message(), factory, rules) {
        Ok(env) => env,
        Err(refusal) => panic!("{refusal}"),
    }
}

fn warrior(race: &str, player: &str) -> Environment {
    environment(race, player, "MobTypeElemental")
}

/// The warrior as the Go reference prepares it, which keeps the 16 October racials.
fn go_warrior(race: &str, player: &str) -> Environment {
    environment_with(Rules::GoReference, race, player, "MobTypeElemental")
}

/// The equipment of one main hand weapon: Worn Mace (36), Worn Shortsword (25) and Worn Axe (37)
/// have no effects. Go equips by the item's own slot, so a second main hand weapon replaces the
/// first.
fn weapon(id: i32) -> String {
    let empty = "{},".repeat(14);
    format!(r#","equipment":{{"items":[{empty}{{"id":{id}}}]}}"#)
}

fn stat(env: &Environment, stat: Stat) -> f64 {
    env.sim.stat(env.player, stat)
}

fn aura(env: &Environment, label: &str) -> Option<AuraId> {
    env.sim.get_aura(env.player, label)
}

fn spell(env: &Environment, id: i32) -> Option<SpellId> {
    env.sim.get_spell(env.player, &ActionId::spell(id))
}

fn physical_taken(env: &Environment) -> f64 {
    env.sim
        .unit(env.player)
        .pseudo_stats
        .school_damage_taken_multiplier[school_array_index(SchoolIndex::Physical)]
}

#[test]
fn disabling_racials_drops_them_but_keeps_the_base_stats() {
    let with = warrior("RaceOrc", "");
    let without = warrior("RaceOrc", r#","disableRacials":true"#);
    assert!(aura(&with, "Blood Fury").is_some());
    assert!(aura(&without, "Blood Fury").is_none());
    assert_eq!(
        with.sim.character(with.player).base_stats,
        without.sim.character(without.player).base_stats
    );

    // The human's spirit racial is a multiplier on top of the base stats, so it goes too.
    let human = warrior("RaceHuman", "");
    let human_without = warrior("RaceHuman", r#","disableRacials":true"#);
    assert!(stat(&human, Stat::Spirit) > stat(&human_without, Stat::Spirit));
    assert_eq!(
        stat(&human_without, Stat::Spirit),
        human_without.sim.character(human_without.player).base_stats[Stat::Spirit]
    );
}

#[test]
fn blood_fury_is_percent_based_and_reports_its_temporary_stats() {
    let mut env = warrior("RaceOrc", "");
    let unit = env.player;
    let blood_fury = aura(&env, "Blood Fury").expect("Blood Fury");
    let heard: Rc<std::cell::RefCell<Vec<Stats>>> = Rc::default();
    let listener_heard = Rc::clone(&heard);
    env.sim
        .unit_mut(unit)
        .on_temporary_stats_changes
        .push(Rc::new(move |_: &mut Sim, aura: AuraId, change: &Stats| {
            assert_eq!(aura, blood_fury);
            listener_heard.borrow_mut().push(*change);
        }));

    let before = env.sim.stats(unit);
    env.sim.activate(blood_fury);
    let during = env.sim.stats(unit);
    env.sim.deactivate(blood_fury);
    let after = env.sim.stats(unit);

    let heard = heard.borrow();
    assert_eq!(heard.len(), 2);
    assert_eq!(heard[0], during.subtract(&before));
    assert_eq!(heard[1], after.subtract(&during));
    assert_eq!(after, before);
    let attack_power = before[Stat::AttackPower];
    assert!(attack_power > 0.0);
    assert!((during[Stat::AttackPower] - attack_power * 1.1).abs() <= 1.0);
    assert!((heard[0][Stat::AttackPower] - attack_power * 0.1).abs() <= 1.0);
    assert_eq!(heard[1][Stat::AttackPower], -heard[0][Stat::AttackPower]);

    let spell_id = spell(&env, 20572).expect("Blood Fury's spell");
    assert_eq!(env.sim.spell(spell_id).related_self_buff, Some(blood_fury));
    let icd = env.sim.aura(blood_fury).icd.expect("an ICD");
    assert_eq!(icd, env.sim.spell(spell_id).cd);
    // The 26 October notes' 27 October additions: two minutes (was three).
    assert_eq!(icd.duration, 2 * MINUTE);
    // 16 October beta notes: three minutes (was two), which the Go reference keeps.
    let go = go_warrior("RaceOrc", "");
    let go_blood_fury = aura(&go, "Blood Fury").expect("Blood Fury");
    assert_eq!(
        go.sim.aura(go_blood_fury).icd.expect("an ICD").duration,
        3 * MINUTE
    );
}

#[test]
fn berserking_is_a_flat_ten_percent_with_no_cost() {
    let mut env = warrior("RaceTroll", "");
    let unit = env.player;
    let before = env.sim.unit(unit).pseudo_stats.attack_speed_multiplier;
    let berserking = aura(&env, "Berserking").expect("Berserking");
    env.sim.activate(berserking);
    assert_eq!(
        env.sim.unit(unit).pseudo_stats.attack_speed_multiplier,
        before * 1.1
    );
    env.sim.deactivate(berserking);
    let spell_id = spell(&env, 20554).expect("Berserking's spell");
    assert!(env.sim.spell(spell_id).cost.is_none());
    let mcds = &env.sim.character(unit).initial_major_cooldowns;
    assert_eq!(mcds.len(), 1);
    assert_eq!(mcds[0].cooldown_type, cooldown_type::DPS);
    // The 26 October notes' 27 October additions: 15 seconds (was 10), which the Go reference
    // keeps.
    assert_eq!(env.sim.aura(berserking).duration, 15 * SECOND);
    let go = go_warrior("RaceTroll", "");
    assert_eq!(
        go.sim
            .aura(aura(&go, "Berserking").expect("Berserking"))
            .duration,
        10 * SECOND
    );
}

#[test]
fn elunes_light_grants_ten_crit_on_use() {
    let mut env = warrior("RaceNightElf", "");
    let before = stat(&env, Stat::PhysicalCritPercent);
    let elunes_light = aura(&env, "Elune's Light").expect("Elune's Light");
    env.sim.activate(elunes_light);
    assert_eq!(stat(&env, Stat::PhysicalCritPercent), before + 10.0);
    env.sim.deactivate(elunes_light);
    assert_eq!(stat(&env, Stat::PhysicalCritPercent), before);
    let unit = env.player;
    let spell_id = spell(&env, 1259799).expect("Elune's Light's spell");
    assert!(env.sim.spell(spell_id).flags.matches(SpellFlag::MCD));
    assert_eq!(
        env.sim.character(unit).initial_major_cooldowns[0].cooldown_type,
        cooldown_type::DPS
    );
    assert_eq!(
        env.sim.aura(elunes_light).icd.map(|icd| icd.duration),
        Some(3 * MINUTE)
    );
}

#[test]
fn stoneform_takes_ten_percent_of_physical_damage_while_it_lasts() {
    let mut env = warrior("RaceDwarf", "");
    let unit = env.player;
    assert_eq!(physical_taken(&env), 1.0);
    let stoneform = aura(&env, "Stoneform").expect("Stoneform");
    env.sim.activate(stoneform);
    assert_eq!(physical_taken(&env), 0.9);
    env.sim.deactivate(stoneform);
    assert_eq!(physical_taken(&env), 1.0 * 0.9 / 0.9);
    let mcds = &env.sim.character(unit).initial_major_cooldowns;
    assert_eq!(mcds[0].cooldown_type, cooldown_type::SURVIVAL);
    // A spell on the 1.5 sec GCD is not reactive.
    let spell_id = spell(&env, 20594).expect("Stoneform's spell");
    assert!(!env.sim.spell(spell_id).flags.matches(SpellFlag::REACTIVE));
}

#[test]
fn shatter_curse_is_a_survival_cooldown_on_the_six_magic_schools() {
    let mut env = warrior("RaceOrc", "");
    let unit = env.player;
    let shatter_curse = aura(&env, "Shatter Curse").expect("Shatter Curse");
    env.sim.activate(shatter_curse);
    let taken = env
        .sim
        .unit(unit)
        .pseudo_stats
        .school_damage_taken_multiplier;
    for index in [
        SchoolIndex::Arcane,
        SchoolIndex::Fire,
        SchoolIndex::Frost,
        SchoolIndex::Holy,
        SchoolIndex::Nature,
        SchoolIndex::Shadow,
    ] {
        // 16 October beta notes: 5% less magic damage taken (was 15%).
        assert_eq!(taken[school_array_index(index)], 0.95);
    }
    assert_eq!(taken[school_array_index(SchoolIndex::Physical)], 1.0);
    let survival = env
        .sim
        .character(unit)
        .initial_major_cooldowns
        .iter()
        .filter(|mcd| mcd.cooldown_type == cooldown_type::SURVIVAL)
        .count();
    assert_eq!(survival, 1);
}

#[test]
fn tauren_endurance_is_health_only_and_dwarf_artisans_precision_adds_hit() {
    // 26 October notes: Endurance keeps only its 5% health; Earth Fury is removed and Artisan's
    // Precision adds 1% hit with spells and attacks.
    let human = warrior("RaceHuman", "");
    let crit_damage =
        |env: &Environment| env.sim.unit(env.player).pseudo_stats.crit_damage_multiplier;
    for race in ["RaceTauren", "RaceDwarf"] {
        let env = warrior(race, "");
        assert_eq!(crit_damage(&env), crit_damage(&human));
        let hit = if race == "RaceDwarf" { 1.0 } else { 0.0 };
        for stat_kind in [Stat::PhysicalHitPercent, Stat::SpellHitPercent] {
            assert!((stat(&env, stat_kind) - stat(&human, stat_kind) - hit).abs() < 1e-9);
        }
    }
    let tauren = warrior("RaceTauren", "");
    let tauren_without = warrior("RaceTauren", r#","disableRacials":true"#);
    assert!(
        (stat(&tauren, Stat::Health) / stat(&tauren_without, Stat::Health) - 1.05).abs() < 1e-9
    );
    assert!(aura(&warrior("RaceDwarf", &weapon(36)), "Earth Fury").is_none());
}

#[test]
fn go_reference_keeps_the_16_october_crit_damage_racials() {
    // 16 October beta notes: Endurance grants 2% increased critical damage (was 1% hit) and
    // Earth Fury replaces Mace Specialization with the same.
    let human = go_warrior("RaceHuman", "");
    for race in ["RaceTauren", "RaceDwarf"] {
        let env = go_warrior(race, "");
        assert_eq!(
            stat(&env, Stat::PhysicalHitPercent),
            stat(&human, Stat::PhysicalHitPercent)
        );
        assert_eq!(
            env.sim.unit(env.player).pseudo_stats.crit_damage_multiplier,
            human
                .sim
                .unit(human.player)
                .pseudo_stats
                .crit_damage_multiplier
                * 1.02
        );
    }
    // Earth Fury is the multiplier alone, with no aura.
    let dwarf = go_warrior("RaceDwarf", &weapon(36));
    assert!(aura(&dwarf, "Earth Fury").is_none());
}

#[test]
fn touch_of_the_grave_uses_the_melee_variant() {
    let env = warrior("RaceUndead", "");
    let id = aura(&env, "Touch of the Grave").expect("Touch of the Grave");
    let aura = env.sim.aura(id);
    assert_eq!(
        aura.action_id_for_proc.as_ref().map(|id| id.spell_id),
        Some(1260189)
    );
    assert!(aura.events.on_spell_hit_dealt);
    // The 26 October notes' 27 October additions: 15 seconds (was 25).
    assert_eq!(aura.icd.map(|icd| icd.duration), Some(15 * SECOND));
    // 16 October beta notes: 25 seconds (was 1 sec), which the Go reference keeps.
    let go = go_warrior("RaceUndead", "");
    let go_touch = go.sim.aura(
        go.sim
            .get_aura(go.player, "Touch of the Grave")
            .expect("Touch of the Grave"),
    );
    assert_eq!(go_touch.icd.map(|icd| icd.duration), Some(25 * SECOND));
    assert!(aura.icd.and_then(|icd| icd.timer).is_some());
    assert_eq!(aura.duration, crate::core::time::NEVER_EXPIRES);
    assert!(aura.active);
    assert!(spell(&env, 1260198).is_some());
}

#[test]
fn skyborne_racials_speed_up_swings_and_hit_elementals_harder() {
    let human = warrior("RaceHuman", "");
    let human_pseudo = human.sim.unit(human.player).pseudo_stats.clone();
    for race in ["RaceHighOrderSkyborne", "RaceWindshaperSkyborne"] {
        let env = warrior(race, "");
        let unit = env.player;
        let pseudo = &env.sim.unit(unit).pseudo_stats;
        // 26 October notes: Wind Blessed's haste is 2% (was 3%).
        assert_eq!(
            pseudo.attack_speed_multiplier,
            human_pseudo.attack_speed_multiplier * 1.02
        );
        assert_eq!(
            pseudo.cast_speed_multiplier,
            human_pseudo.cast_speed_multiplier * 1.02
        );
        // 16 October beta notes, which the Go reference keeps: 3% (was 1%).
        let go = go_warrior(race, "");
        assert_eq!(
            go.sim.unit(go.player).pseudo_stats.attack_speed_multiplier,
            human_pseudo.attack_speed_multiplier * 1.03
        );
        let table = env.attack_table(unit, env.encounter.targets[0]);
        assert_eq!(table.damage_dealt_multiplier, 1.05);
        assert_eq!(table.crit_multiplier, 1.0);

        let read_ley_line = spell(&env, 1259705);
        let skysight = spell(&env, 1259686);
        assert_eq!(read_ley_line.is_some(), race == "RaceHighOrderSkyborne");
        assert_eq!(skysight.is_some(), race == "RaceWindshaperSkyborne");
        // Client 1.60.1.70058 SpellMisc: Read Ley Line casts in 2 sec, Skysight in 0.5 sec.
        if let Some(id) = read_ley_line {
            assert_eq!(env.sim.spell(id).default_cast.cast_time, 2 * SECOND);
        }
        if let Some(id) = skysight {
            assert_eq!(env.sim.spell(id).default_cast.cast_time, 500 * MILLISECOND);
        }
    }
    // The table of a target that is not an elemental is untouched.
    let troll = environment("RaceHighOrderSkyborne", "", "MobTypeHumanoid");
    let table = troll.attack_table(troll.player, troll.encounter.targets[0]);
    assert_eq!(table.damage_dealt_multiplier, 1.0);
}

#[test]
fn beast_slaying_applies_only_to_beasts() {
    let troll = environment("RaceTroll", "", "MobTypeBeast");
    let table = troll.attack_table(troll.player, troll.encounter.targets[0]);
    assert_eq!(table.damage_dealt_multiplier, 1.05);
    let elemental = warrior("RaceTroll", "");
    let table = elemental.attack_table(elemental.player, elemental.encounter.targets[0]);
    assert_eq!(table.damage_dealt_multiplier, 1.0);
}

#[test]
fn energized_and_elemental_blessing_change_regen_and_movement() {
    // 26 October notes: Skysight is 15% movement speed for 15 seconds (was 10% for 30), and
    // Read Leyline 50% more regeneration for 15 seconds (was 100%).
    for (rules, speed, blessing_duration, regen) in [
        (Rules::Forever, 1.15, 15 * SECOND, 1.5),
        (Rules::GoReference, 1.1, 30 * SECOND, 2.0),
    ] {
        let mut env = environment_with(rules, "RaceWindshaperSkyborne", "", "MobTypeElemental");
        let unit = env.player;
        let blessing = aura(&env, "Elemental Blessing").expect("Elemental Blessing");
        assert_eq!(env.sim.aura(blessing).duration, blessing_duration);
        let before = env.sim.unit(unit).pseudo_stats.movement_speed_multiplier;
        env.sim.activate(blessing);
        assert_eq!(
            env.sim.unit(unit).pseudo_stats.movement_speed_multiplier,
            before * speed
        );
        env.sim.deactivate(blessing);
        assert!((env.sim.unit(unit).pseudo_stats.movement_speed_multiplier - before).abs() < 1e-12);

        let mut env = environment_with(rules, "RaceHighOrderSkyborne", "", "MobTypeElemental");
        let energized = aura(&env, "Energized").expect("Energized");
        assert_eq!(env.sim.aura(energized).duration, 15 * SECOND);
        // A warrior has no mana bar, so the aura changes nothing; a mana bar gains the regen.
        env.sim.activate(energized);
        assert_eq!(env.sim.unit(env.player).mana_bar.mana_regen_multiplier, 0.0);
        env.sim.deactivate(energized);
        let unit = env.player;
        env.sim.unit_mut(unit).mana_bar.enabled = true;
        env.sim.unit_mut(unit).mana_bar.mana_regen_multiplier = 1.0;
        env.sim.activate(energized);
        assert_eq!(env.sim.unit(unit).mana_bar.mana_regen_multiplier, regen);
        env.sim.deactivate(energized);
        assert!((env.sim.unit(unit).mana_bar.mana_regen_multiplier - 1.0).abs() < 1e-12);
    }
}

#[test]
fn expansive_mind_and_eureka_for_a_gnome_warrior() {
    let mut env = warrior("RaceGnome", "");
    let unit = env.player;
    assert_eq!(env.sim.unit(unit).rage_bar.max_rage, 105.0);

    // 16 October beta notes: an 8 second buff with no charges.
    let eureka = aura(&env, "Eureka!").expect("Eureka!");
    assert_eq!(env.sim.aura(eureka).duration, 8 * SECOND);
    env.sim.activate(eureka);
    assert!(env.sim.aura(eureka).active);
    assert_eq!(env.sim.aura(eureka).stacks, 0);

    let ability = spell(&env, 1).expect("the class ability");
    let item_spell = spell(&env, 2).expect("the item spell");
    assert_eq!(env.sim.spell(item_spell).damage_multiplier, 1.0);
    assert!((env.sim.spell(ability).damage_multiplier - 1.1).abs() < 1e-4);

    env.sim.deactivate(eureka);
    assert!(!env.sim.aura(eureka).active);
    assert!((env.sim.spell(ability).damage_multiplier - 1.0).abs() < 1e-4);
    assert!(!env.sim.aura(eureka).events.on_cast_complete);
}

#[test]
fn ruthless_needs_no_weapon_and_savage_is_gone() {
    // 26 October notes: Human Ruthless is 1% crit (was 2%) with any weapon or none, and Orc
    // Savage is removed.
    let crit = |env: &Environment| stat(env, Stat::PhysicalCritPercent);
    let spell_crit = |env: &Environment| stat(env, Stat::SpellCritPercent);
    for (race, label, percent) in [("RaceHuman", "Ruthless", 1.0), ("RaceOrc", "Savage", 0.0)] {
        for gear in [String::new(), weapon(25), weapon(36), weapon(37)] {
            let env = warrior(race, &gear);
            let without = warrior(race, &format!(r#"{gear},"disableRacials":true"#));
            assert!((crit(&env) - crit(&without) - percent).abs() < 1e-9);
            assert!((spell_crit(&env) - spell_crit(&without) - percent).abs() < 1e-9);
            assert!(aura(&env, label).is_none());
            let withheld = warrior(
                race,
                &format!(r#"{gear},"disableWeaponSpecialization":true"#),
            );
            assert_eq!(crit(&withheld), crit(&env));
        }
    }
    assert!(aura(&warrior("RaceDwarf", &weapon(36)), "Mace Specialization").is_none());
}

#[test]
fn human_spirit_is_ten_percent() {
    // 26 October notes: "Human Spirit now increases Spirit by 10% (was 5%)."
    for (rules, multiplier) in [(Rules::Forever, 1.1), (Rules::GoReference, 1.05)] {
        let with = environment_with(rules, "RaceHuman", "", "MobTypeElemental");
        let without = environment_with(
            rules,
            "RaceHuman",
            r#","disableRacials":true"#,
            "MobTypeElemental",
        );
        let (a, b) = (stat(&with, Stat::Spirit), stat(&without, Stat::Spirit));
        // Stat multipliers round the stat down: 45 base Spirit is 49 at 10%, 47 at 5%.
        assert_eq!(a, (b * multiplier).floor());
    }
}

#[test]
fn go_reference_keeps_the_16_october_crit_racials() {
    // 16 October beta notes: Human Ruthless (2%) and Orc Savage (1%) replace the sword and axe
    // specializations and raise both crits with any weapon or none.
    let crit = |env: &Environment| stat(env, Stat::PhysicalCritPercent);
    for (race, percent) in [("RaceHuman", 2.0), ("RaceOrc", 1.0)] {
        let env = go_warrior(race, &weapon(25));
        let without = go_warrior(race, &format!(r#"{},"disableRacials":true"#, weapon(25)));
        assert!((crit(&env) - crit(&without) - percent).abs() < 1e-9);
    }
}

/// A warrior that registers a survival on-use cooldown with no GCD, as an item trinket does.
fn survival_factory(
    sim: &mut Sim,
    unit: UnitId,
    player: &Message,
) -> Result<Box<dyn PrepAgent>, Refusal> {
    let timer = sim.new_timer(unit);
    let mut stats = Stats::default();
    stats[Stat::Armor] = 100.0;
    let aura = sim.register_temporary_stats_on_use_cd(
        unit,
        "Shield Wall Trinket",
        stats,
        10 * SECOND,
        SpellConfig {
            action_id: ActionId::item(1234),
            cast: CastConfig {
                cd: Cooldown {
                    timer: Some(timer),
                    duration: 5 * MINUTE,
                },
                ..CastConfig::default()
            },
            ..SpellConfig::default()
        },
    );
    assert_eq!(aura.infer_cd_type(), cooldown_type::SURVIVAL);
    factory(sim, unit, player)
}

#[test]
fn a_survival_on_use_cooldown_without_a_gcd_is_reactive() {
    let request = r#"{"simOptions":{"iterations":1,"randomSeed":"100"},
        "raid":{"parties":[{"players":[{"name":"Tank","class":"ClassWarrior",
          "race":"RaceHuman","buffs":{},"consumables":{}}]}]},
        "encounter":{"duration":180,"targets":[{"level":63}]}}"#;
    let request = Request::from_json(request.as_bytes()).expect("a valid request");
    let env = match Environment::new(request.message(), survival_factory) {
        Ok(env) => env,
        Err(refusal) => panic!("{refusal}"),
    };
    let unit = env.player;
    let id = env
        .sim
        .get_spell(unit, &ActionId::item(1234))
        .expect("the on-use spell");
    let spell = env.sim.spell(id);
    assert!(spell.flags.matches(SpellFlag::MCD));
    assert!(spell.flags.matches(SpellFlag::APL));
    assert!(spell.flags.matches(SpellFlag::REACTIVE));
    assert!(spell.flags.matches(SpellFlag::NO_ON_CAST_COMPLETE));
    let aura = env
        .sim
        .get_aura(unit, "Shield Wall Trinket")
        .expect("the aura");
    assert_eq!(spell.related_self_buff, Some(aura));
    assert_eq!(env.sim.aura(aura).icd, Some(spell.cd));
    let mcd = env
        .sim
        .character(unit)
        .initial_major_cooldowns
        .iter()
        .find(|mcd| mcd.spell == id)
        .expect("the major cooldown");
    assert_eq!(mcd.cooldown_type, cooldown_type::SURVIVAL);
    assert!(mcd.timings.is_empty());
}
