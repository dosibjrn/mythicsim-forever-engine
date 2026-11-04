//! Warrior tests: the prepared v2 gate on the production Fury Warrior request. Its Go result
//! and first-fight log are compared with the rest of the fixture family.

mod prepare;

use forever_engine::{
    check_prepared, contracts::prepared_v2::PreparedV2, prepared_coverage, PreparedError,
};
use serde_json::{json, Value};
use std::{fs, path::Path};

fn fury_json() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/production-warrior.prepared.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn reasons(value: Value) -> Vec<String> {
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    match check_prepared(&prepared) {
        Err(PreparedError::Unsupported(reasons)) => reasons,
        other => panic!("expected unsupported, got {other:?}"),
    }
}

fn effect_mut<'a>(value: &'a mut Value, kind: &str) -> &'a mut Value {
    value["effects"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|effect| effect["kind"] == kind)
        .unwrap()
}

#[test]
fn production_fury_request_is_supported() {
    let prepared: PreparedV2 = serde_json::from_value(fury_json()).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
}

/// A stance change runs only with the stance category and the stance passives exported.
#[test]
fn a_stance_change_needs_the_stance_passives() {
    let mut value = fury_json();
    let last = value["player"]["rotation"]["priorityList"]
        .as_array()
        .unwrap()
        .len()
        - 1;
    let item = &mut value["player"]["rotation"]["priorityList"][last]["action"];
    assert_eq!(item["castSpell"]["spellId"]["spellId"], 2458);
    item["castSpell"]["spellId"]["spellId"] = json!(2457);
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "pseudo_stat_auras");
    assert!(reasons(value.clone()).contains(&"rotation reaches spell 2457, a stance change".into()));
    assert!(crate::refusal_codes(value).contains(&(
        "class_limit",
        "rotation reaches spell 2457, a stance change".into()
    )));
}

/// The raid's Expose Armor holds the armor category for good, so the warrior's own Sunder
/// Armor can never be cast. Without it the stacks would change the target's armor.
#[test]
fn sunder_armor_needs_a_blocked_category() {
    let mut value = fury_json();
    effect_mut(&mut value, "sunder_armor")["blocked"] = json!(false);
    assert_eq!(
        reasons(value),
        ["rotation reaches spell 11597, which stacks the warrior's own Sunder Armor"]
    );
}

/// Retaliation cast by hand runs only with its effect exported.
#[test]
fn retaliation_in_the_rotation_needs_its_effect() {
    let mut value = fury_json();
    value["player"]["rotation"]["priorityList"]
        .as_array_mut()
        .unwrap()
        .insert(
            0,
            json!({"action": {"castSpell": {"spellId": {"spellId": 20230}}}}),
        );
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "retaliation");
    assert!(reasons(value).contains(&"rotation casts spell 20230, which has no behavior".into()));
}

/// The warrior's own Sunder Armor and Slam without Improved Slam run against Go.
#[test]
fn own_sunder_armor_and_a_swing_stopping_slam_are_supported() {
    for case in [
        "arms-warrior-own-sunder",
        "protection-warrior-own-sunder",
        "arms-warrior-no-improved-slam",
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("fixtures/mage/prepared-v2/{case}.prepared.json"));
        let prepared: PreparedV2 = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new(), "{case}");
    }
}

/// Without the target's armor category the warrior's own stacks could not set its armor.
#[test]
fn own_sunder_armor_needs_the_armor_category() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/protection-warrior-own-sunder.prepared.json");
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| !(effect["kind"] == "exclusive_category" && effect["unit"] == "target"));
    assert!(reasons(value).contains(
        &"rotation reaches spell 11597, which stacks the warrior's own Sunder Armor".into()
    ));
}

/// Go gives `currentRage` no value without a rage bar, which drops the term.
#[test]
fn rage_without_a_rage_bar_is_unsupported() {
    let mut value = fury_json();
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "rage_bar");
    let reasons = reasons(value);
    assert!(reasons.contains(&"rotation item 15 reads rage, which the player lacks".into()));
    assert!(reasons
        .contains(&"player aura \"RageBar\" listens to combat events without an effect".into()));
}

fn protection_json() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/production-protection-warrior.prepared.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn production_tank_requests_are_supported() {
    for case in [
        "production-protection-warrior",
        "production-fury-protection-warrior",
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("fixtures/mage/prepared-v2/{case}.prepared.json"));
        let prepared: PreparedV2 = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new(), "{case}");
    }
}

/// Revenge's trigger hears the target's swings; without its effect nothing runs it.
/// Last Stand below the defensive health threshold, with its maximum health on the stat aura
/// combinations, and a Greater Stoneshield Potion beside it.
#[test]
fn last_stand_below_a_health_threshold_is_supported() {
    for case in [
        "protection-warrior-last-stand",
        "fury-protection-warrior-last-stand",
        "protection-warrior-sapper-last-stand",
        "protection-warrior-greater-stoneshield",
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("fixtures/mage/prepared-v2/{case}.prepared.json"));
        let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        let prepared: PreparedV2 = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new(), "{case}");
        // Last Stand runs only through its effect.
        let mut value = value;
        value["effects"]
            .as_array_mut()
            .unwrap()
            .retain(|effect| effect["kind"] != "last_stand");
        assert!(
            reasons(value).iter().any(|reason| reason.contains("12975")),
            "{case}"
        );
    }
}

/// Racial survival cooldowns and Shield Wall below the defensive health threshold, on
/// Protection Warriors without Last Stand.
#[test]
fn survival_cooldowns_below_a_health_threshold_are_supported() {
    for case in [
        "protection-warrior-dwarf-stoneform",
        "protection-warrior-orc-shatter-curse",
        "protection-warrior-sapper-orc-shatter-curse",
        "protection-warrior-shield-wall",
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("fixtures/mage/prepared-v2/{case}.prepared.json"));
        let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        let prepared: PreparedV2 = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new(), "{case}");
        // Shield Wall runs only through its effect.
        let mut value = value;
        value["effects"]
            .as_array_mut()
            .unwrap()
            .retain(|effect| effect["kind"] != "shield_wall");
        assert!(
            reasons(value).iter().any(|reason| reason.contains("871")),
            "{case}"
        );
    }
}

/// Shield Block (2565), which the production Protection rotations cast: the cast runs only
/// through its effect, which also hears the target's swings for the blocks that spend it.
#[test]
fn shield_block_requests_are_supported() {
    for case in [
        "production-protection-warrior-shield-block",
        "production-fury-protection-warrior-shield-block",
        "production-protection-warrior-shield-block-3-targets",
    ] {
        let value = warrior_fixture(case);
        let prepared: PreparedV2 = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new(), "{case}");
        let mut value = value;
        value["effects"]
            .as_array_mut()
            .unwrap()
            .retain(|effect| effect["kind"] != "shield_block");
        let reasons = reasons(value);
        assert!(
            reasons.contains(&"rotation reaches spell 2565 without a known behavior".into()),
            "{case}: {reasons:?}"
        );
        assert!(
            reasons.contains(&"player aura \"Shield Block\" reacts to the target's swings".into()),
            "{case}: {reasons:?}"
        );
    }
}

/// The cast sets Shield Block to its two charges, and only a blocked swing at the same moment
/// spends one. An aura that fades, as at the end of the fight, drops its last charge after it.
#[test]
fn shield_block_spends_a_charge_on_each_block() {
    let logs = first_fight_log(warrior_fixture(
        "production-protection-warrior-shield-block",
    ));
    let lines: Vec<&str> = logs.lines().collect();
    assert!(logs.contains("{SpellID: 2565} stacks: 0 --> 2"));
    let mut spends = 0;
    for (index, line) in lines.iter().enumerate() {
        if !line.contains("{SpellID: 2565} stacks: 2 --> 1")
            && !line.contains("{SpellID: 2565} stacks: 1 --> 0")
            || (index > 0 && lines[index - 1].ends_with("Aura faded: {SpellID: 2565}"))
        {
            continue;
        }
        spends += 1;
        let time = &line[..line.find(']').unwrap() + 1];
        assert!(
            lines[..index]
                .iter()
                .rev()
                .take_while(|earlier| earlier.starts_with(time))
                .any(|earlier| earlier.contains("} Block for ")),
            "{line}: no block spent it"
        );
    }
    assert!(spends > 1, "no charge was spent");
}

/// shield_block.go's cast condition needs a shield to block with.
#[test]
fn shield_block_needs_a_shield() {
    let mut value = warrior_fixture("production-protection-warrior-shield-block");
    assert!(first_fight_log(value.clone()).contains("Casting {SpellID: 2565}"));
    effect_mut(&mut value, "shield_block")["can_block"] = json!(false);
    assert!(!first_fight_log(value).contains("Casting {SpellID: 2565}"));
}

#[test]
fn revenge_trigger_needs_its_effect() {
    let mut value = protection_json();
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "revenge");
    assert!(reasons(value)
        .contains(&"player aura \"Revenge - Trigger\" reacts to the target's swings".into()));
}

/// The runtime reads the player's damage taken live, so an aura the exporter finds changing
/// it must be one an effect multiplies it with.
#[test]
fn an_untracked_damage_taken_aura_is_unsupported() {
    let mut value = protection_json();
    value["enemy"]["damage_taken_auras"]
        .as_array_mut()
        .unwrap()
        .push(json!("player:Enrage"));
    assert_eq!(
        reasons(value),
        ["player:Enrage changes the player's damage taken, which nothing multiplies"]
    );
}

fn warrior_fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/mage/prepared-v2/{name}.prepared.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// Enrage and Blood Craze act on the Goblin Sapper Charge's hit on the player with nothing
/// tanking it, and Improved Hamstring's trigger needs its effect.
#[test]
fn hit_taken_listeners_act_on_the_sappers_hit() {
    for name in [
        "warrior-enrage-sapper",
        "warrior-blood-craze-sapper",
        "arms-warrior-improved-hamstring",
    ] {
        let prepared: PreparedV2 = serde_json::from_value(warrior_fixture(name)).unwrap();
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new(), "{name}");
    }
    let mut value = warrior_fixture("arms-warrior-improved-hamstring");
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "improved_hamstring");
    assert!(reasons(value).contains(
        &"player aura \"Improved Hamstring - Trigger\" listens to combat events without an effect"
            .into()
    ));
}

/// Sweeping Strikes' hit spell shares the cast's action ID and comes first in the spellbook,
/// so the major cooldown names its own spell by position, as Go holds the spell itself.
#[test]
fn sweeping_strikes_cooldown_names_its_own_spell() {
    let value = warrior_fixture("arms-warrior-sweeping-strikes");
    let spells = value["player"]["spells"].as_array().unwrap();
    let first = spells
        .iter()
        .find(|spell| spell["action_id"]["spell_id"] == 12723)
        .unwrap();
    assert_eq!(first["class_spell"], "sweeping_strikes_hit");
    let cooldown = value["player"]["major_cooldowns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|cooldown| cooldown["action_id"]["spell_id"] == 12723)
        .unwrap();
    let own = cooldown["spell"].as_u64().unwrap() as usize;
    assert_eq!(spells[own]["class_spell"], "sweeping_strikes");

    let mut value = value.clone();
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "sweeping_strikes");
    assert!(
        reasons(value).contains(&"rotation reaches spell 12723 without a known behavior".into())
    );
}

/// Battlegear of Might's rage and the Premier High Warlord's Shield Wall's strike back act on
/// the target's swings only with their effects.
#[test]
fn gear_listeners_of_the_target_swings_need_their_effects() {
    for (name, kind, label) in [
        (
            "protection-warrior-battlegear-of-might",
            "battlegear_of_might_rage",
            "Battlegear of Might 5P",
        ),
        (
            "protection-warrior-premier-shield-wall",
            "spell_data_damage_proc",
            "Premier High Warlord's Shield Wall",
        ),
    ] {
        let mut value = warrior_fixture(name);
        let prepared: PreparedV2 = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new(), "{name}");
        value["effects"]
            .as_array_mut()
            .unwrap()
            .retain(|effect| effect["kind"] != kind);
        assert!(
            reasons(value).contains(&format!(
                "player aura {label:?} reacts to the target's swings"
            )),
            "{name}"
        );
    }
}

fn first_fight_log(mut value: Value) -> String {
    value["sim"]["iterations"] = json!(1);
    value["sim"]["debug_first_iteration"] = json!(true);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    let report =
        forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
    report.result["logs"].as_str().unwrap().to_string()
}

fn lines_with<'a>(logs: &'a str, target: u32, spell: &str) -> Vec<&'a str> {
    logs.lines()
        .filter(|line| line.contains(&format!("[Target {target}] {{SpellID: {spell}")))
        .collect()
}

/// Whirlwind's cap on the row is four targets, so a fifth is never struck, and each of the
/// four takes both hands' strikes.
#[test]
fn whirlwind_strikes_four_of_five_targets() {
    let logs = first_fight_log(warrior_fixture("warrior-whirlwind-5-targets"));
    for target in 1..=4 {
        let hits = lines_with(&logs, target, "1680");
        for (tag, hand) in [("Tag: 1", "main"), ("Tag: 2", "off")] {
            assert!(
                hits.iter().any(|line| line.contains(tag)),
                "Target {target}: no {hand} hand strike"
            );
        }
    }
    assert!(lines_with(&logs, 5, "1680").is_empty());
}

/// Cleave strikes two targets, the cast target and the next, and calculates both hits before
/// it deals either: the damage lines of one cast come after both debug lines.
#[test]
fn cleave_strikes_two_targets_from_the_cast_target() {
    let logs = first_fight_log(warrior_fixture("production-warrior-2-targets"));
    let lines: Vec<&str> = logs.lines().collect();
    let cast = lines
        .iter()
        .position(|line| line.contains("Casting {SpellID: 20569}"))
        .unwrap();
    let rest = &lines[cast..];
    let position = |needle: &str, deal: bool| {
        rest.iter()
            .position(|line| line.contains(needle) && line.contains("[DEBUG]") != deal)
            .unwrap()
    };
    let first_debug = position("[Target 1] {SpellID: 20569} ", false);
    let second_debug = position("[Target 2] {SpellID: 20569} ", false);
    let first_deal = position("[Target 1] {SpellID: 20569} ", true);
    let second_deal = position("[Target 2] {SpellID: 20569} ", true);
    assert!(first_debug < second_debug);
    assert!(second_debug < first_deal && first_deal < second_deal);
}

/// Thunder Clap's debuff lands on each target the clap lands on, up to the row's four.
#[test]
fn thunder_clap_debuffs_each_target_it_lands_on() {
    let logs = first_fight_log(warrior_fixture("arms-warrior-thunder-clap-5-targets"));
    for target in 1..=4 {
        let gained = format!("[Target {target}] Aura gained: {{SpellID: 11581}}");
        assert!(logs.lines().any(|line| line.contains(&gained)), "{gained}");
    }
    assert!(!logs
        .lines()
        .any(|line| line.contains("[Target 5] Aura gained: {SpellID: 11581}")));
}

/// A landed melee hit while two targets are active casts a copy of its damage on the next
/// target, and each copy spends a charge.
#[test]
fn sweeping_strikes_copies_a_hit_to_the_next_target() {
    let logs = first_fight_log(warrior_fixture("production-arms-warrior-2-targets"));
    assert!(lines_with(&logs, 2, "12723}").len() > 3, "{logs}");
    assert!(logs.contains("{SpellID: 12723} stacks: 5 --> 4"));
}

/// Sweeping Strikes strikes "an additional nearby opponent", so with one target Go does not
/// let Thunder Clap spend a charge on a normalized attack against the target it already hit
/// (#667), though a rotation may still cast the aura. The clap still lands.
#[test]
fn sweeping_strikes_does_nothing_against_one_target() {
    let logs = first_fight_log(warrior_fixture("arms-warrior-thunder-clap-1-target"));
    assert!(lines_with(&logs, 1, "12723, Tag: 1}").is_empty(), "{logs}");
    assert!(logs.contains("[Target 1] Aura gained: {SpellID: 11581}"));
}

/// A multidot line puts Rend, and the Deep Wounds a crit of its tick starts, on each target
/// whose dot is down.
#[test]
fn a_multidot_line_puts_rend_on_every_target() {
    let logs = first_fight_log(warrior_fixture("arms-warrior-multidot-rend-3-targets"));
    for target in 1..=3 {
        assert!(
            !lines_with(&logs, target, "11574} tick").is_empty(),
            "Target {target}: no Rend tick"
        );
    }
}

/// Every copy of the boss swings at a tank in Go, each on its own timer, and so does each in
/// Rust: every target casts its auto attack at the player and the player's listeners hear all
/// of them. Each copy's own Thunder Clap slow moves its own timer.
#[test]
fn every_copy_of_the_boss_swings_at_the_tank() {
    let logs = first_fight_log(warrior_fixture("production-protection-warrior-3-targets"));
    for target in 1..=3 {
        let swing = format!("[Target {target}] Casting {{OtherID: 3, Tag: 1}}");
        assert!(
            logs.matches(&swing).count() > 10,
            "Target {target}: it does not swing"
        );
        let hit = format!("[Target {target}] [protection-warrior (#1)] {{OtherID: 3, Tag: 1}}");
        assert!(logs.contains(&hit), "Target {target}: it never lands a hit");
    }
    // The player's Revenge answers a dodge, parry or block from any of them.
    assert!(logs.contains("Casting {SpellID: 25288}"));
}

/// Retaliation's strike goes back at the copy whose melee hit spent the charge.
#[test]
fn retaliation_strikes_the_copy_that_hit() {
    let logs = first_fight_log(warrior_fixture("protection-warrior-own-sunder-3-targets"));
    for target in 1..=3 {
        assert!(
            !lines_with(&logs, target, "20240").is_empty(),
            "Target {target}: no Retaliation strike"
        );
    }
}

/// The times a target's swings at the player start, in seconds.
fn swing_times(logs: &str, target: u32) -> Vec<f64> {
    logs.lines()
        .filter(|line| line.contains(&format!("[Target {target}] Casting {{OtherID: 3, Tag: 1}}")))
        .map(|line| {
            line[1..line.find(']').unwrap()]
                .parse()
                .expect("a time stamp")
        })
        .collect()
}

/// Thunder Clap slows each target it lands on, up to the row's four: that copy's swings come
/// a fifth further apart while the debuff holds, and the fifth target keeps its pace, as Go
/// moves each copy's melee speed on its own.
#[test]
fn thunder_clap_slows_only_the_copies_it_lands_on() {
    let logs = first_fight_log(warrior_fixture("production-protection-warrior-5-targets"));
    // A parry shortens one interval, so look at the longest the target ever waits.
    let longest = |target: u32| {
        let times = swing_times(&logs, target);
        times
            .windows(2)
            .map(|pair| ((pair[1] - pair[0]) * 100.0).round() / 100.0)
            .fold(0.0, f64::max)
    };
    for target in 1..=4 {
        assert!(
            logs.contains(&format!(
                "[Target {target}] Aura gained: {{SpellID: 11581}}"
            )),
            "Target {target}: not clapped"
        );
        assert_eq!(longest(target), 2.4, "Target {target}: not slowed");
    }
    assert!(!logs.contains("[Target 5] Aura gained: {SpellID: 11581}"));
    assert_eq!(longest(5), 2.0, "Target 5: slowed");
}

/// Challenging Shout is an always hit for no damage on every target in unit index order, one
/// roll-free outcome each, cast once for its cooldown. The fork's shout does not taunt, so a
/// tank's threat from it is none and the copies of the boss keep swinging as before.
#[test]
fn challenging_shout_hits_every_target_for_nothing() {
    for case in [
        "protection-warrior-challenging-shout-3-targets",
        "arms-warrior-challenging-shout-3-targets",
    ] {
        let logs = first_fight_log(warrior_fixture(case));
        assert_eq!(
            logs.matches("Casting {SpellID: 1161}").count(),
            1,
            "{case}: the shout is cast once for its cooldown"
        );
        let mut previous = 0;
        for target in 1..=3 {
            let hits = lines_with(&logs, target, "1161");
            assert_eq!(hits.len(), 1, "{case}: Target {target}");
            assert!(
                hits[0].ends_with("Hit for 0.000 damage (SpellSchool: 1). (Threat: 0.000)"),
                "{case}: {}",
                hits[0]
            );
            let at = logs
                .find(&format!("[Target {target}] {{SpellID: 1161}}"))
                .unwrap();
            assert!(at > previous, "{case}: Target {target} out of order");
            previous = at;
        }
        assert!(lines_with(&logs, 4, "1161").is_empty());
    }
}

/// Against one target the shout hits it alone, and a tank's copies of the boss go on swinging
/// at it: the shout changes nobody's aim.
#[test]
fn challenging_shout_leaves_a_tanks_targets_swinging() {
    let logs = first_fight_log(warrior_fixture("protection-warrior-challenging-shout"));
    let shout = logs.find("[Target 1] {SpellID: 1161}").unwrap();
    assert_eq!(lines_with(&logs, 1, "1161").len(), 1);
    assert!(
        logs[shout..].contains("[Target 1] [protection-warrior (#1)] {OtherID: 3, Tag: 1}"),
        "no swing at the tank after the shout"
    );
}

/// The shout runs only with its effect.
#[test]
fn challenging_shout_needs_its_effect() {
    let mut value = warrior_fixture("arms-warrior-challenging-shout");
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "challenging_shout");
    assert!(reasons(value)
        .contains(&"rotation reaches spell 1161 without a known behavior".to_string()));
}

/// Demoralizing Shout rolls a magic hit on every target in unit index order and debuffs each
/// it lands on, and each copy of the boss swings at the tank with the attack power its own
/// debuff leaves it, as Go reads it from the unit that swings.
#[test]
fn demoralizing_shout_debuffs_every_target_and_cuts_each_swing() {
    let logs = first_fight_log(warrior_fixture(
        "protection-warrior-demoralizing-shout-5-targets",
    ));
    for target in 1..=5 {
        let cast = lines_with(&logs, target, "11556");
        assert!(!cast.is_empty(), "Target {target}: no shout");
        let gained = logs
            .find(&format!(
                "[Target {target}] Aura gained: {{SpellID: 11556}}"
            ))
            .unwrap_or_else(|| panic!("Target {target}: no debuff"));
        let swing = format!(
            "[Target {target}] [protection-warrior (#1)] {{OtherID: 3, Tag: 1}} [DEBUG] MAP: 600.0"
        );
        assert!(
            logs[gained..].contains(&swing),
            "Target {target}: no swing under the debuff"
        );
        assert!(
            !logs[..gained].contains(&swing),
            "Target {target}: a swing under a debuff it did not hold yet"
        );
    }
}

/// A raid's permanent debuff in the Demoralizing category bids as much as the shout and
/// outlasts it, so the category holds the player's shout off: it is cast and rolled, and its
/// aura is never gained.
#[test]
fn a_permanent_debuff_holds_the_shout_off() {
    for case in [
        "protection-warrior-demoralizing-shout-over-roar-debuff",
        "protection-warrior-demoralizing-shout-over-shout-debuff",
    ] {
        let logs = first_fight_log(warrior_fixture(case));
        assert!(
            !lines_with(&logs, 1, "11556").is_empty(),
            "{case}: no shout"
        );
        assert!(
            !logs.contains("Aura gained: {SpellID: 11556}"),
            "{case}: the shout's aura was gained"
        );
    }
}

/// The shout runs only with its effect.
#[test]
fn demoralizing_shout_needs_its_effect() {
    let mut value = warrior_fixture("fury-warrior-demoralizing-shout");
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "demoralizing_shout");
    assert!(reasons(value)
        .contains(&"rotation reaches spell 11556 without a known behavior".to_string()));
}

/// The casts of a queued strike between each arming of its queue aura and its fade, and the
/// rage they spend, from a first-fight log.
fn strikes_per_queue(logs: &str, strike: &str) -> Vec<(usize, f64)> {
    let gained = format!("Aura gained: {{SpellID: {strike}, Tag: 1}}");
    let faded = format!("Aura faded: {{SpellID: {strike}, Tag: 1}}");
    let cast = format!("Casting {{SpellID: {strike}}} ");
    let spent = format!(" rage from {{SpellID: {strike}}} ");
    let mut queues = Vec::new();
    let mut open: Option<(usize, f64)> = None;
    for line in logs.lines() {
        if line.contains(&gained) {
            open = Some((0, 0.0));
        } else if line.contains(&faded) {
            queues.extend(open.take());
        } else if let Some((casts, rage)) = open.as_mut() {
            if line.contains(&cast) {
                *casts += 1;
            } else if line.contains(&spent) {
                let amount = line
                    .split("Spent ")
                    .nth(1)
                    .unwrap()
                    .split(' ')
                    .next()
                    .unwrap();
                *rage += amount.parse::<f64>().unwrap();
            }
        }
    }
    queues
}

/// A queued Heroic Strike or Cleave replaces one main hand swing and is gone. An extra attack
/// its own hit procs at once (Windfury Totem here, Sword Specialization alike) is a white swing,
/// so each queue is spent by one strike and one rage cost (Ambiwar, MythicSim Discord #bugs,
/// 21 October 2026). Each fixture's first fight, before the fix, had a queue spent twice.
#[test]
fn a_queued_strike_is_spent_once_and_its_extra_attack_is_a_white_swing() {
    for (name, strike, cost) in [
        ("fury-warrior-battlegear-of-might-sapper", "25286", 12.0),
        ("warrior-heroic-strike", "25286", 13.0),
        ("production-fury-protection-warrior", "25286", 15.0),
        ("production-arms-warrior-2-targets", "20569", 20.0),
        ("production-warrior-5-targets", "20569", 17.0),
    ] {
        let logs = first_fight_log(warrior_fixture(name));
        let queues = strikes_per_queue(&logs, strike);
        assert!(queues.iter().any(|&(casts, _)| casts == 1), "{name}");
        for (casts, rage) in queues {
            assert!(casts <= 1, "{name}: one queue cast {strike} {casts} times");
            assert!(
                rage <= cost,
                "{name}: one queue spent {rage} rage on {strike}"
            );
        }
    }
    // The strike's hit still procs Windfury Totem (at 43.43 in this fight, where the old order
    // cast Heroic Strike twice): the extra attack is the plain swing of Windfury's own row.
    let logs = first_fight_log(warrior_fixture("fury-warrior-battlegear-of-might-sapper"));
    let lines: Vec<&str> = logs.lines().collect();
    let white_after_strike = lines.windows(8).any(|window| {
        window[0].contains("[Target 1] {SpellID: 25286} ")
            && window[0].contains(" for ")
            && window[1].contains("Aura gained: {SpellID: 10610, Tag: -1}")
            && window[2..]
                .iter()
                .any(|line| line.contains("Casting {OtherID: 3, Tag: 25584}"))
    });
    assert!(white_after_strike);
}

/// The lines of a first-fight log at one timestamp.
fn lines_at<'a>(logs: &'a str, time: &str) -> Vec<&'a str> {
    let prefix = format!("[{time}] ");
    logs.lines()
        .filter(|line| line.starts_with(&prefix))
        .collect()
}

fn has_line(logs: &str, time: &str, needle: &str) -> bool {
    lines_at(logs, time)
        .iter()
        .any(|line| line.contains(needle))
}

/// A prepull move runs unchecked, the Movement aura's stacks count the yards, and Charge
/// triples the speed until the run ends, which also ends the dash aura, all as Go logs it. The
/// swings start once the run has put the warrior in range, before the pull.
#[test]
fn a_prepull_charge_runs_to_the_target() {
    let logs = first_fight_log(warrior_fixture("production-warrior-prepull-charge"));
    assert!(has_line(&logs, "-6.00", "[DEBUG] Moving to 25.0 yards"));
    assert!(has_line(&logs, "-6.00", "{OtherID: 20} stacks: 0 --> 1"));
    // 25 yards at 7 yards a second take 3.571 seconds.
    assert!(has_line(&logs, "-2.43", "{OtherID: 20} stacks: 1 --> 25"));
    assert!(has_line(&logs, "-2.43", "Aura faded: {OtherID: 20}"));
    assert!(has_line(&logs, "-1.00", "Casting {SpellID: 11578}"));
    assert!(has_line(
        &logs,
        "-1.00",
        "[DEBUG] Movement speed changed from 7.00 (0.00%) to 21.00 (200.00%)"
    ));
    assert!(has_line(
        &logs,
        "-1.00",
        "Gained 18.000 rage from {SpellID: 11578}"
    ));
    // The run to 4.5 yards takes 0.976 seconds at the dash speed, and ends the aura.
    assert!(has_line(&logs, "-0.02", "{OtherID: 20} stacks: 4 --> 0"));
    assert!(has_line(&logs, "-0.02", "Aura faded: {SpellID: 11578}"));
    assert!(has_line(
        &logs,
        "-0.02",
        "[DEBUG] Movement speed changed from 21.00 (200.00%) to 7.00 (0.00%)"
    ));
    assert!(has_line(&logs, "0.00", "Casting {OtherID: 3, Tag: 1}"));
}

/// Fork patch 192: a spell's range check reads where a moving unit is. The warrior walks out to 25
/// yards from -6 s and casts its prepull Charge at -3 s, 21 yards out, inside Charge's 8 to 25
/// yards: it charges in and swings. Before the patch the range check read the 0 yards the walk
/// started from, the Charge failed silently and the warrior never swung.
#[test]
fn charge_cast_while_moving_checks_the_current_range() {
    let logs = first_fight_log(warrior_fixture(
        "production-warrior-prepull-charge-while-moving",
    ));
    assert!(logs.contains("[DEBUG] Moving to 25.0 yards"));
    assert!(
        has_line(&logs, "-3.00", "Casting {SpellID: 11578}"),
        "{logs}"
    );
    assert!(has_line(
        &logs,
        "-3.00",
        "[DEBUG] Movement speed changed from 7.00 (0.00%) to 21.00 (200.00%)"
    ));
    assert!(logs.contains("Casting {OtherID: 3"), "{logs}");
}

/// A second move starts from the position the first has reached and replaces it, without
/// casting the Movement spell again.
#[test]
fn a_second_prepull_move_replaces_the_first() {
    let logs = first_fight_log(warrior_fixture("production-warrior-prepull-move-cancelled"));
    assert_eq!(logs.matches("Casting {OtherID: 20}").count(), 1);
    assert_eq!(logs.matches("[DEBUG] Moving to").count(), 2);
}

/// A warrior still moving at the pull cannot start a cast with a cast time, and its swings wait
/// for a range it never reaches.
#[test]
fn a_warrior_moving_at_the_pull_cannot_slam() {
    let logs = first_fight_log(warrior_fixture(
        "production-arms-warrior-prepull-move-at-the-pull",
    ));
    assert!(logs.contains("[DEBUG] Moving to 40.0 yards"));
    assert!(!logs.contains("Casting {SpellID: 11605"));
    assert!(!logs.contains("Casting {OtherID: 3"));
}

/// Go drops a prepull action after the pull, a hidden one and one whose condition is constant
/// false, so none of the moves runs.
#[test]
fn prepull_moves_go_drops_never_run() {
    let logs = first_fight_log(warrior_fixture("production-warrior-prepull-move-dropped"));
    assert!(!logs.contains("Moving to"));
    assert!(!logs.contains("Casting {OtherID: 20}"));
}

/// Charge needs the movement speed and its own effect.
#[test]
fn charge_needs_its_effect_and_the_movement_speed() {
    let mut value = warrior_fixture("production-warrior-prepull-charge");
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "player_movement");
    assert_eq!(
        reasons(value),
        ["a prepull move has no exported movement speed"]
    );
    let mut value = warrior_fixture("production-warrior-prepull-charge");
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "warrior_charge");
    assert_eq!(
        reasons(value),
        ["rotation reaches spell 11578 without a known behavior"]
    );
}

/// Runeblade of Baron Rivendare's Unholy Aura gives 60 health every five seconds, which the
/// runtime does not run: a request with it is refused, with a move or without.
#[test]
fn the_unholy_auras_regeneration_is_refused() {
    let unholy = warrior_fixture("production-arms-warrior-prepull-move-with-unholy-aura");
    assert!(reasons(unholy).contains(
        &"player aura \"Unholy Aura\" regenerates health every five seconds, which the runtime does not run"
            .into()
    ));
}

/// The Diamond Flask is a five second channel cast before the pull: its self hot ticks each
/// second and the last tick, at the pull, activates a Strength aura for a minute.
#[test]
fn the_diamond_flask_channels_then_grants_strength() {
    let logs = first_fight_log(warrior_fixture("production-warrior-diamond-flask"));
    assert!(has_line(&logs, "-5.00", "Casting {ItemID: 20130}"));
    assert!(has_line(&logs, "-5.00", "Aura gained: {ItemID: 20130}"));
    assert!(has_line(&logs, "0.00", "Aura faded: {ItemID: 20130}"));
    assert!(has_line(&logs, "0.00", "Aura gained: {SpellID: 1318070}"));
    assert!(has_line(
        &logs,
        "0.00",
        "Gained {\"Strength\": 20.000,} from {SpellID: 1318070}."
    ));
    assert!(has_line(&logs, "60.00", "Aura faded: {SpellID: 1318070}"));
}

/// The flask's major cooldown never activates on its own, so without a cast in the rotation or
/// the prepull the autocast leaves it alone.
#[test]
fn the_diamond_flask_is_never_autocast() {
    let mut value = warrior_fixture("production-warrior-diamond-flask");
    value["player"]["rotation"]["prepullActions"] = json!([]);
    value["player"]["prepull_actions"] = json!(0);
    let logs = first_fight_log(value);
    assert!(!logs.contains("ItemID: 20130"));
    assert!(!logs.contains("1318070"));
}

/// A rotation that casts the flask itself takes it from the major cooldowns, so the exporter
/// describes it from the spellbook; the cast begins when the rotation reaches it.
#[test]
fn a_flask_the_rotation_casts_is_described() {
    let logs = first_fight_log(warrior_fixture(
        "production-warrior-diamond-flask-in-rotation",
    ));
    let cast = logs
        .lines()
        .find(|line| line.contains("Casting {ItemID: 20130}"))
        .unwrap();
    let time: f64 = cast[1..cast.find(']').unwrap()].parse().unwrap();
    assert!(time >= 10.0, "{cast}");
    assert!(logs.contains("Aura gained: {SpellID: 1318070}"));
}

/// Without its effect the flask has no behavior, whichever way the rotation reaches it.
#[test]
fn the_diamond_flask_needs_its_effect() {
    let mut value = warrior_fixture("production-warrior-diamond-flask");
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "diamond_flask");
    assert!(reasons(value).contains(&"rotation reaches item 20130 without a known behavior".into()));
}

/// The Frost hit of Iceblade Hacker and Warblade of Caer Darrow (shared.NewProcDamageEffect with
/// the melee defense type): the effect hears the melee hits of the hand holding the weapon, at a
/// fixed chance of 1, and its spell is of the melee defense type.
#[test]
fn a_melee_weapon_damage_proc_hears_the_hits_of_its_own_hand() {
    let main = &["ProcMaskMeleeMHAuto", "ProcMaskMeleeMHSpecial"][..];
    let off = &["ProcMaskMeleeOHAuto", "ProcMaskMeleeOHSpecial"][..];
    let both = &[
        "ProcMaskMeleeMHAuto",
        "ProcMaskMeleeMHSpecial",
        "ProcMaskMeleeOHAuto",
        "ProcMaskMeleeOHSpecial",
    ][..];
    for (name, aura, hands) in [
        ("warrior-iceblade-hacker", "Iceblade Hacker", main),
        (
            "arms-warrior-warblade-of-caer-darrow",
            "Warblade of Caer Darrow",
            main,
        ),
        (
            "warrior-warblade-of-caer-darrow-off-hand",
            "Warblade of Caer Darrow",
            off,
        ),
        (
            "warrior-warblade-of-caer-darrow-both-hands",
            "Warblade of Caer Darrow",
            both,
        ),
    ] {
        let value = warrior_fixture(name);
        let prepared: PreparedV2 = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new(), "{name}");
        let effect = value["effects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|effect| effect["trigger_aura"] == aura)
            .unwrap();
        // The client rows carry the always-hit attribute: the crit roll, and no miss.
        assert_eq!(effect["outcome"], "melee_special_crit_only", "{name}");
        let spells = value["player"]["spells"].as_array().unwrap();
        let masks_of = |position: &Value| -> Vec<&str> {
            spells[position.as_u64().unwrap() as usize]["proc_mask"]
                .as_array()
                .unwrap()
                .iter()
                .map(|mask| mask.as_str().unwrap())
                .collect()
        };
        assert_eq!(
            spells[effect["spell"].as_u64().unwrap() as usize]["defense_type"],
            "DefenseTypeMelee",
            "{name}"
        );
        // Every spell the manager rolls for carries a mask of a wielding hand, as a Whirlwind
        // that swings both hands does; the other hand's spells are not rolled for.
        let chances = effect["chances"].as_array().unwrap();
        assert!(!chances.is_empty(), "{name}");
        for chance in chances {
            assert_eq!(chance["chance"], 1.0, "{name}");
            let masks = masks_of(&chance["spell"]);
            assert!(
                masks.iter().any(|mask| hands.contains(mask)),
                "{name}: {masks:?}"
            );
        }
        // Each hand holding the weapon has a spell the manager rolls for.
        for hand in hands {
            assert!(
                chances
                    .iter()
                    .any(|chance| masks_of(&chance["spell"]).contains(hand)),
                "{name}: {hand}"
            );
        }
        // A spell the trigger hears that the manager does not roll for swings only the other hand.
        let rolled: Vec<u64> = chances
            .iter()
            .map(|chance| chance["spell"].as_u64().unwrap())
            .collect();
        for heard in effect["trigger_spells"].as_array().unwrap() {
            if !rolled.contains(&heard.as_u64().unwrap()) {
                let masks = masks_of(heard);
                assert!(
                    !masks.iter().any(|mask| hands.contains(mask)),
                    "{name}: {masks:?}"
                );
            }
        }
    }
}

/// Without its effect the aura listens to hits with nothing that handles them.
#[test]
fn a_melee_weapon_damage_proc_needs_its_effect() {
    let mut value = warrior_fixture("warrior-iceblade-hacker");
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["trigger_aura"] != "Iceblade Hacker");
    assert!(reasons(value).contains(
        &"player aura \"Iceblade Hacker\" listens to combat events without an effect".into()
    ));
}

/// The runtime rolls the melee special table, with or without the crit, for a literal range of
/// a melee spell, and nothing else.
#[test]
fn a_melee_weapon_damage_proc_names_a_table_the_runtime_rolls() {
    let refused = |value: Value| {
        crate::refusal_codes(value)
            .into_iter()
            .filter(|(code, _)| *code == "proc_unsupported")
            .map(|(_, reason)| reason)
            .collect::<Vec<_>>()
    };
    let hit = |outcome: &str| {
        format!(
            "Iceblade Hacker's hit rolls the {outcome} table, which the runtime rolls only for a \
             melee spell's literal range"
        )
    };
    let position = |value: &Value| -> usize {
        value["effects"]
            .as_array()
            .unwrap()
            .iter()
            .position(|effect| effect["trigger_aura"] == "Iceblade Hacker")
            .unwrap()
    };
    let original = warrior_fixture("warrior-iceblade-hacker");
    assert!(refused(original.clone()).is_empty());

    // A table the runtime does not know.
    let mut value = original.clone();
    let index = position(&value);
    value["effects"][index]["outcome"] = json!("melee_special_block_and_crit");
    assert_eq!(refused(value), [hit("melee_special_block_and_crit")]);

    // A table that disagrees with the spell's crit: Go picks the no crit table only for a spell
    // the client bars from critting.
    let mut value = original.clone();
    let index = position(&value);
    value["effects"][index]["outcome"] = json!("melee_special_hit");
    assert_eq!(refused(value.clone()), [hit("melee_special_hit")]);
    value["effects"][index]["can_crit"] = json!(false);
    assert!(refused(value).is_empty());
    let mut value = original.clone();
    value["effects"][index]["can_crit"] = json!(false);
    assert_eq!(refused(value), [hit("melee_special_crit_only")]);

    // The table the spell would roll without the always-hit attribute is a table the runtime
    // rolls too.
    let mut value = original.clone();
    value["effects"][index]["outcome"] = json!("melee_special_hit_and_crit");
    assert!(refused(value).is_empty());

    // Without its literal range the hit rolls the client effect, which here deals nothing.
    let mut value = original.clone();
    let index = position(&value);
    value["effects"][index]
        .as_object_mut()
        .unwrap()
        .remove("roll");
    assert_eq!(
        refused(value),
        ["Iceblade Hacker's hit is not rolled: a client effect that deals nothing"]
    );

    // A spell of another defense type would take the wrong critical strike multiplier.
    let mut value = original;
    let index = position(&value);
    let spell = value["effects"][index]["spell"].as_u64().unwrap() as usize;
    value["player"]["spells"][spell]["defense_type"] = json!("DefenseTypeMagic");
    assert_eq!(refused(value), [hit("melee_special_crit_only")]);
}

/// The hit takes the melee table's crit roll and nothing else: its client row always hits, so
/// it never misses or dodges as a melee special would, it crits for twice its base damage, and a
/// Frost hit still takes the partial resist roll.
#[test]
fn a_melee_weapon_damage_proc_rolls_the_crit_only_table_in_a_fight() {
    let logs = first_fight_log(warrior_fixture("warrior-iceblade-hacker"));
    let lines = lines_with(&logs, 1, "1298414");
    for refused in ["} Dodge (", "} Miss (", "} Parry ("] {
        assert!(
            !lines.iter().any(|line| line.contains(refused)),
            "{refused}"
        );
    }
    for expected in [
        "} Crit for 81.400 damage",
        "} Hit for 40.700 damage",
        "} Crit (25% Resist) for 61.050 damage",
    ] {
        assert!(
            lines.iter().any(|line| line.contains(expected)),
            "{expected}"
        );
    }
}

/// Taunt and Intimidating Shout are an always hit for no damage on the target they are cast at
/// alone, one roll-free outcome each cast, with no threat: the fork has no taunt and no fear.
#[test]
fn taunt_and_intimidating_shout_hit_the_first_target_for_nothing() {
    for (case, spell, casts) in [
        ("protection-warrior-taunt-3-targets", "355", None),
        ("arms-warrior-taunt-3-targets", "355", None),
        (
            "protection-warrior-intimidating-shout-3-targets",
            "5246",
            Some(2),
        ),
        ("arms-warrior-intimidating-shout-3-targets", "5246", Some(2)),
    ] {
        let logs = first_fight_log(warrior_fixture(case));
        let cast = logs
            .matches(&format!("Casting {{SpellID: {spell}}}"))
            .count();
        let hits = lines_with(&logs, 1, spell);
        // A queued Taunt shows as a line of its own, so only the hits are compared with the casts.
        let hits: Vec<_> = hits.iter().filter(|line| line.contains(" Hit ")).collect();
        assert_eq!(hits.len(), cast, "{case}: a hit for each cast");
        assert!(cast >= 2, "{case}: cast {cast} times");
        if let Some(expected) = casts {
            assert_eq!(cast, expected, "{case}");
        }
        for line in hits {
            assert!(
                line.ends_with("Hit for 0.000 damage (SpellSchool: 1). (Threat: 0.000)"),
                "{case}: {line}"
            );
        }
        for target in 2..=3 {
            assert!(lines_with(&logs, target, spell).is_empty(), "{case}");
        }
    }
}

/// The shout goes on its own cooldown, three minutes in the pinned data, and spends rage; Taunt
/// spends none and is ready again after eight seconds.
#[test]
fn taunt_and_intimidating_shout_keep_their_cooldowns() {
    let seconds = |logs: &str, spell: &str| -> Vec<f64> {
        logs.lines()
            .filter(|line| line.contains(&format!("Casting {{SpellID: {spell}}}")))
            .map(|line| line[1..line.find(']').unwrap()].parse().unwrap())
            .collect()
    };
    let logs = first_fight_log(warrior_fixture("protection-warrior-intimidating-shout"));
    let shout = seconds(&logs, "5246");
    assert_eq!(shout.len(), 2, "{shout:?}");
    assert!(shout[1] - shout[0] >= 180.0, "{shout:?}");
    assert!(logs.contains("Spent 23.000 rage from {SpellID: 5246}"));
    let logs = first_fight_log(warrior_fixture("protection-warrior-taunt"));
    let taunt = seconds(&logs, "355");
    for pair in taunt.windows(2) {
        assert!(pair[1] - pair[0] >= 8.0, "{taunt:?}");
    }
    assert!(!logs.contains("rage from {SpellID: 355}"));
}

/// Taunt needs Defensive Stance: the Arms warrior's Battle Stance never casts it, and it casts
/// it once its rotation has changed stance.
#[test]
fn taunt_needs_defensive_stance() {
    let casts = |value: Value| {
        first_fight_log(value)
            .matches("Casting {SpellID: 355}")
            .count()
    };
    assert!(casts(warrior_fixture("arms-warrior-taunt")) > 0);
    let mut value = warrior_fixture("arms-warrior-intimidating-shout");
    let rotation = serde_json::to_string(&value["player"]["rotation"])
        .unwrap()
        .replace("\"spellId\":5246", "\"spellId\":355");
    value["player"]["rotation"] = serde_json::from_str(&rotation).unwrap();
    assert_eq!(casts(value), 0);
}

/// Each spell runs only with its effect.
#[test]
fn taunt_and_intimidating_shout_need_their_effects() {
    for (case, kind, spell) in [
        ("arms-warrior-taunt", "taunt", "355"),
        (
            "arms-warrior-intimidating-shout",
            "intimidating_shout",
            "5246",
        ),
    ] {
        let mut value = warrior_fixture(case);
        value["effects"]
            .as_array_mut()
            .unwrap()
            .retain(|effect| effect["kind"] != kind);
        assert!(
            reasons(value).contains(&format!(
                "rotation reaches spell {spell} without a known behavior"
            )),
            "{case}"
        );
    }
}

/// The Vindicator's Battlegear 3 piece bonus takes 15 seconds off the shout's cooldown, a spell
/// mod of the common cast.
#[test]
fn intimidating_shouts_cooldown_follows_the_vindicators_set_bonus() {
    let logs = first_fight_log(warrior_fixture(
        "protection-warrior-intimidating-shout-vindicators",
    ));
    let casts: Vec<f64> = logs
        .lines()
        .filter(|line| line.contains("Casting {SpellID: 5246}"))
        .map(|line| line[1..line.find(']').unwrap()].parse().unwrap())
        .collect();
    assert_eq!(casts.len(), 3, "{casts:?}");
    for pair in casts.windows(2) {
        assert!(
            pair[1] - pair[0] >= 165.0 && pair[1] - pair[0] < 170.0,
            "{casts:?}"
        );
    }
}

/// The rage bar of a prepared warrior.
fn rage_bar(value: &Value) -> &Value {
    value["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == "rage_bar")
        .unwrap()
}

/// Fork patches 123 and 124, the reference's rule: a critical auto attack pays the swing's rage
/// and, on top, the rage of the swing before a two-hander's factor times the crit rage multiplier,
/// now 2, less one. A two-hander's critical swing is not worth twice its swing, a one-hander's is.
#[test]
fn a_critical_auto_attack_pays_the_swing_and_the_one_handers_rage() {
    // The Arms Warrior's two-hander: 15.75 rage a swing (3.46 x 1.3006 x 3.5), whose crit adds
    // the 3.46 x 3.5 = 12.11 of a one-hander at the same speed.
    let arms = warrior_fixture("production-arms-warrior");
    let bar = rage_bar(&arms);
    assert_eq!(bar["main_hand_rage"], 15.75);
    assert_eq!(bar["main_hand_crit_rage"], 12.11);
    assert!(bar.get("crit_multiplier").is_none());
    // The Fury Warrior's one-handers: the crit adds the swing's rage again, and the off hand's
    // is half of the hit factor.
    let fury = warrior_fixture("production-warrior");
    let bar = rage_bar(&fury);
    assert_eq!(bar["main_hand_crit_rage"], bar["main_hand_rage"]);
    assert_eq!(bar["off_hand_crit_rage"], bar["off_hand_rage"]);

    // In a fight a main hand auto gives the swing's rage, and a critical one the sum.
    let logs = first_fight_log(arms);
    let lines: Vec<&str> = logs.lines().collect();
    let (mut hits, mut crits) = (0, 0);
    for (index, line) in lines.iter().enumerate() {
        let crit = line.contains("{OtherID: 3, Tag: 1} Crit for");
        if !crit && !line.contains("{OtherID: 3, Tag: 1} Hit for") {
            continue;
        }
        let Some(gain) = lines[index + 1..]
            .iter()
            .take(2)
            .find(|line| line.contains("rage from {OtherID: 3, Tag: 1}"))
        else {
            continue;
        };
        let expected = if crit {
            "Gained 27.860 rage"
        } else {
            "Gained 15.750 rage"
        };
        assert!(gain.contains(expected), "{line}\n{gain}");
        if crit {
            crits += 1;
        } else {
            hits += 1;
        }
    }
    assert!(hits > 0 && crits > 0, "{hits} hits and {crits} crits");
}

/// Forever's rules (wowsims/forever 7c28cfa73): a critical white hit gives twice its swing's whole
/// rage, a two-hander's factor included. Forever's combat logs of 9 October 2026 hold 21 two-hand
/// crits at exactly 2x (16.2 to 32.4 rage over 13 crits, 14.8 to 29.7 over 8). Forever preparation
/// writes `crit_multiplies_swing` on the rage bar and the reference's does not, so the Arms
/// Warrior's two-hander crits for 31.5 rage, not 27.86. A one-hander's crit already gave twice its
/// swing, so the Fury Warrior's fight is the same with the flag or without.
#[test]
fn a_critical_auto_attack_doubles_the_whole_swing_under_forever_rules() {
    let request = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/mage/prepared-v2/production-arms-warrior.request.json"),
    )
    .unwrap();
    let forever =
        forever_engine::prepare_json(&crate::warrior_trees_20261104::forever_bytes(&request), "x")
            .unwrap();
    assert_eq!(rage_bar(&forever)["crit_multiplies_swing"], json!(true));
    let go = forever_engine::prepare_json_with(&request, "x", forever_engine::Rules::GoReference)
        .unwrap();
    assert!(rage_bar(&go).get("crit_multiplies_swing").is_none());

    let mut arms = warrior_fixture("production-arms-warrior");
    rage_bar_mut(&mut arms)["crit_multiplies_swing"] = json!(true);
    let logs = first_fight_log(arms);
    let lines: Vec<&str> = logs.lines().collect();
    let mut crits = 0;
    for (index, line) in lines.iter().enumerate() {
        if !line.contains("{OtherID: 3, Tag: 1} Crit for") {
            continue;
        }
        let Some(gain) = lines[index + 1..]
            .iter()
            .take(2)
            .find(|line| line.contains("rage from {OtherID: 3, Tag: 1}"))
        else {
            continue;
        };
        assert!(gain.contains("Gained 31.500 rage"), "{line}\n{gain}");
        crits += 1;
    }
    assert!(crits > 0, "no main hand crits");

    let fury = warrior_fixture("production-warrior");
    let mut flagged = fury.clone();
    rage_bar_mut(&mut flagged)["crit_multiplies_swing"] = json!(true);
    assert_eq!(first_fight_log(fury), first_fight_log(flagged));
}

/// The rage bar of a prepared warrior, to change.
fn rage_bar_mut(value: &mut Value) -> &mut Value {
    value["effects"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|effect| effect["kind"] == "rage_bar")
        .unwrap()
}

/// Fork patch 116: a dual wielder's off hand crit sizes Deep Wounds off the off hand weapon, as
/// the main hand's sizes it off the main hand. The first crit of the Fury Warrior's fight is an
/// off hand auto, which starts the bleed at 1.29 seconds, and the bleed's first tick at 4.29
/// carries what the off hand's crit and a main hand crit after it gave it (44.5 in Go, where
/// sizing both off the main hand made it 47.0).
#[test]
fn an_off_hand_crit_sizes_deep_wounds_off_the_off_hand_weapon() {
    let logs = first_fight_log(warrior_fixture("production-warrior"));
    let crit = logs
        .lines()
        .find(|line| line.contains("Crit for") && line.contains("{OtherID: 3"))
        .expect("a crit");
    assert!(
        crit.starts_with("[1.29]") && crit.contains("Tag: 2}"),
        "{crit}"
    );
    let tick = logs
        .lines()
        .find(|line| line.contains("{SpellID: 412609} [DEBUG]"))
        .expect("a tick");
    assert!(
        tick.starts_with("[4.29]") && tick.contains("BaseDamage:44.5,"),
        "{tick}"
    );
}

/// Fork patches 114 and 118: Windfury Totem's proc comes off the main hand only, as its tooltip
/// says, so the off hand's autos and specials, which the party aura's mask names, do not roll
/// it. Without an off hand weapon an off hand special is a shield strike, which the server
/// swings as a main hand attack, so it still rolls.
#[test]
fn windfury_totem_hears_the_main_hand_only() {
    let heard = |value: &Value| -> Vec<(Value, Vec<String>)> {
        let effect = value["effects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|effect| effect["kind"] == "windfury_totem")
            .unwrap();
        effect["trigger_spells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|position| {
                let spell = &value["player"]["spells"][position.as_u64().unwrap() as usize];
                let masks = spell["proc_mask"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|mask| mask.as_str().unwrap().to_string())
                    .collect();
                (spell["action_id"].clone(), masks)
            })
            .collect()
    };
    // Dual wielding: nothing that swings only the off hand, and not Whirlwind's off hand strike
    // or the shield strike's off hand mask, which the off hand weapon contradicts.
    let fury = heard(&warrior_fixture("production-warrior"));
    assert!(!fury.is_empty());
    for (id, masks) in &fury {
        assert!(
            masks.iter().any(|mask| mask.starts_with("ProcMaskMeleeMH")),
            "{id}: {masks:?}"
        );
    }
    // Shield Bash (1672) and the off hand Whirlwind strike (1680, tag 1 is the main hand's).
    assert!(!fury.iter().any(|(id, _)| id["spell_id"] == 1672));
    assert!(!fury
        .iter()
        .any(|(id, _)| id["spell_id"] == 1680 && id["tag"] == 2));
    assert!(!fury.iter().any(|(id, _)| id["tag"] == 2));
    // With no off hand weapon the shield strike rolls it.
    let arms = heard(&warrior_fixture("production-arms-warrior"));
    assert!(arms
        .iter()
        .any(|(id, masks)| id["spell_id"] == 1672
            && masks == &["ProcMaskMeleeOHSpecial".to_string()]));
}

/// Fork patch 125: a boss's 1% hit suppression can be switched off. The exporter reads it from
/// the attack table, and the runtime takes it off every physical hit chance, so a Warrior who
/// is 8% hit capped misses nothing against the boss.
#[test]
fn switching_off_the_hit_suppression_lowers_the_misses() {
    let off = warrior_fixture("warrior-no-hit-suppression");
    let on = warrior_fixture("production-warrior");
    assert_eq!(off["melee"]["hit_suppression"], 0.0);
    assert_eq!(on["melee"]["hit_suppression"], 0.01);
    // The same input with the suppression back on rolls different hit tables.
    let mut suppressed = off.clone();
    suppressed["melee"]["hit_suppression"] = json!(0.01);
    assert_ne!(first_fight_log(off), first_fight_log(suppressed));
}

/// The damage figure of a debug damage line.
fn debug_damage(line: &str) -> f64 {
    let value = line.split(" for ").nth(1).unwrap();
    value[..value.find(' ').unwrap()].parse().unwrap()
}

/// The threat figure of a debug damage line.
fn debug_threat(line: &str) -> f64 {
    let value = line.split("(Threat: ").nth(1).unwrap();
    value[..value.find(')').unwrap()].parse().unwrap()
}

fn threat_multiplier(value: &Value) -> f64 {
    value["player"]["pseudo_stats"]["threat_multiplier"]
        .as_f64()
        .unwrap()
}

fn landed_lines<'a>(logs: &'a str, target: u32, spell: &str) -> Vec<&'a str> {
    lines_with(logs, target, spell)
        .into_iter()
        .filter(|line| line.contains(" damage (SpellSchool"))
        .collect()
}

/// Patch 183 (wowsims/forever 913a5bcf69): Revenge is 2.25 times its damage and no flat bonus.
/// Patch 184: Heroic Strike adds 2.54 a spell level (152.4 at Rank 9), none with attack power.
#[test]
fn revenge_and_heroic_strike_threat_follow_the_captures() {
    let value = warrior_fixture("production-protection-warrior");
    let multiplier = threat_multiplier(&value);
    let logs = first_fight_log(value);
    let revenge = landed_lines(&logs, 1, "25288}");
    assert!(revenge.len() > 5, "{} Revenge hits", revenge.len());
    for line in revenge {
        let want = 2.25 * debug_damage(line) * multiplier;
        assert!(
            (debug_threat(line) - want).abs() < 2e-3,
            "{line} is not {want} threat"
        );
    }
    let strikes = landed_lines(&logs, 1, "25286}");
    assert!(strikes.len() > 5, "{} Heroic Strikes", strikes.len());
    for line in strikes {
        let want = (debug_damage(line) + 152.4) * multiplier;
        assert!(
            (debug_threat(line) - want).abs() < 2e-3,
            "{line} is not {want} threat"
        );
    }
}

/// Patch 184: Cleave adds 1.53 a spell level (91.8 at Rank 5) on one target.
#[test]
fn cleave_threat_adds_a_flat_share_on_one_target() {
    let value = warrior_fixture("warrior-cleave");
    let multiplier = threat_multiplier(&value);
    let logs = first_fight_log(value);
    let cleaves = landed_lines(&logs, 1, "20569}");
    assert!(!cleaves.is_empty());
    for line in cleaves {
        let want = (debug_damage(line) + 91.8) * multiplier;
        assert!(
            (debug_threat(line) - want).abs() < 2e-3,
            "{line} is not {want} threat"
        );
    }
}

/// Patch 184: a Cleave that lands on two targets splits its flat threat between them, and with
/// the split off each target takes the whole.
#[test]
fn cleave_splits_its_flat_threat_between_the_targets_it_lands_on() {
    let flat_over_hits = |split: bool, flat: f64| {
        let mut value = warrior_fixture("arms-warrior-3-targets-flame-wrath");
        effect_mut(&mut value, "heroic_strike_queue")["cleave_threat_split"] = json!(split);
        let logs = first_fight_log(value);
        // The first cast's two hits, on the first two targets, at one time.
        let first = landed_lines(&logs, 1, "20569}")[0];
        let second = landed_lines(&logs, 2, "20569}")[0];
        let stamp = |line: &str| line[..line.find(']').unwrap()].to_string();
        assert_eq!(stamp(first), stamp(second));
        // Each threat over its damage plus its flat share is the same threat multiplier.
        let scale = |line: &str| debug_threat(line) / (debug_damage(line) + flat);
        (scale(first), scale(second))
    };
    let (a, b) = flat_over_hits(true, 91.8 / 2.0);
    assert!((a - b).abs() < 1e-5, "split: {a} and {b}");
    let (a, b) = flat_over_hits(false, 91.8);
    assert!((a - b).abs() < 1e-5, "unsplit: {a} and {b}");
    // The shares do not agree with the other split.
    let (a, b) = flat_over_hits(true, 91.8);
    assert!((a - b).abs() > 1e-4, "{a} and {b}");
}

/// Patch 185: Sunder Armor's flat threat is the client's 206 plus 0.05 of the spell's melee
/// attack power at the cast.
#[test]
fn sunder_armor_threat_adds_a_share_of_the_attack_power() {
    let run = |share: f64| {
        let mut value = warrior_fixture("protection-warrior-own-sunder");
        effect_mut(&mut value, "sunder_armor")["threat_per_attack_power"] = json!(share);
        let logs = first_fight_log(value);
        let casts = landed_lines(&logs, 1, "11597}");
        assert!(!casts.is_empty());
        debug_threat(casts[0])
    };
    let multiplier = threat_multiplier(&warrior_fixture("protection-warrior-own-sunder"));
    let flat = run(0.0);
    assert!((flat - 206.0 * multiplier).abs() < 2e-3, "{flat}");
    let with_attack_power = run(0.05);
    let attack_power = (with_attack_power / multiplier - 206.0) / 0.05;
    assert!(
        (100.0..10_000.0).contains(&attack_power),
        "{with_attack_power} threat is {attack_power} attack power"
    );
}

/// The rage gain spell's threat on each target, of a fight with the threat multiplier applied
/// or not.
fn rage_gain_threat(name: &str, multiplied: bool) -> Vec<f64> {
    let mut value = warrior_fixture(name);
    value["sim"]["iterations"] = json!(20);
    effect_mut(&mut value, "rage_bar")["threat_multiplied"] = json!(multiplied);
    let targets = value["encounter"]["target_count"].as_u64().unwrap_or(1) as usize;
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    let report =
        forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
    let player = &report.result["raidMetrics"]["parties"][0]["players"][0];
    let action = player["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["id"]["otherId"] == "OtherActionRageGain")
        .expect("a tank gains rage");
    action["targets"]
        .as_array()
        .unwrap()
        .iter()
        .take(targets)
        .map(|target| target["threat"].as_f64().unwrap_or(0.0))
        .collect()
}

/// Patch 182 (wowsims/forever 913a5bcf69): rage gained is 5 threat times the warrior's threat
/// multiplier (Defensive Stance and Defiance) at the gain, split between the targets in combat.
#[test]
fn rage_gain_threat_takes_the_threat_multiplier_and_splits_between_the_targets() {
    let on = rage_gain_threat("production-protection-warrior", true)[0];
    let off = rage_gain_threat("production-protection-warrior", false)[0];
    let ratio = on / off;
    assert!(
        (1.29..1.5).contains(&ratio),
        "one target: x{ratio} of the unmultiplied"
    );

    let on = rage_gain_threat("production-protection-warrior-3-targets", true);
    let off = rage_gain_threat("production-protection-warrior-3-targets", false);
    assert_eq!((on.len(), off.len()), (3, 3));
    for target in 0..3 {
        assert!((on[target] - on[0]).abs() < 1e-6 && (off[target] - off[0]).abs() < 1e-6);
    }
    let ratio = on[0] / off[0];
    assert!(
        (0.43..0.5).contains(&ratio),
        "three targets: x{ratio} of the unsplit"
    );
}

/// Flurry (12319): with the buff up, every white swing spends a charge whatever its outcome,
/// and a Heroic Strike or Cleave spends one only when it misses (Forever logs on build
/// 1.60.1.70450, see src/classes/warrior/talents/flurry.rs). A swing's listeners can run after
/// a nested extra attack's, so the charges are counted per moment of the log.
#[test]
fn flurry_spends_a_charge_on_every_white_swing_and_a_missed_queued_strike() {
    let mut value = warrior_fixture("production-warrior");
    effect_mut(&mut value, "warrior_flurry")["every_swing_spends"] = json!(true);
    let logs = first_fight_log(value);
    let result = |line: &str| line.contains("] [Target ") && line.contains("(SpellSchool: ");
    // Either hand's swing, or an extra attack (Windfury's tag), which swings the same way.
    let white = |line: &str| result(line) && line.contains("{OtherID: 3, Tag: ");
    let queued_miss = |line: &str| {
        result(line)
            && (line.contains("{SpellID: 25286} Miss (")
                || line.contains("{SpellID: 20569} Miss ("))
    };
    let unlanded = |line: &str| {
        line.contains("} Miss (") || line.contains("} Dodge (") || line.contains("} Parry (")
    };
    let mut moments: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in logs.lines() {
        let time = &line[..line.find(']').unwrap() + 1];
        match moments.last_mut() {
            Some((at, lines)) if *at == time => lines.push(line),
            _ => moments.push((time, vec![line])),
        }
    }
    let (mut spends, mut unlanded_spends, mut faded) = (0, 0, false);
    for (time, lines) in &moments {
        let mut spent = 0;
        for line in lines {
            if line.ends_with("Aura faded: {SpellID: 12966}") {
                faded = true;
            }
            let Some(stacks) = line.split("{SpellID: 12966} stacks: ").nth(1) else {
                continue;
            };
            let (from, to) = stacks.split_once(" --> ").unwrap();
            let (from, to): (i32, i32) = (from.parse().unwrap(), to.parse().unwrap());
            if to == from - 1 && !faded {
                spent += 1;
            }
            faded = false;
        }
        let swings: Vec<&&str> = lines.iter().filter(|line| white(line)).collect();
        let misses = lines.iter().filter(|line| queued_miss(line)).count();
        assert!(
            spent <= swings.len() + misses,
            "{time}: {spent} charges spent by {} swings and {misses} missed strikes",
            swings.len()
        );
        if spent > 0 && misses == 0 && swings.iter().all(|line| unlanded(line)) {
            unlanded_spends += 1;
        }
        spends += spent;
    }
    assert!(spends > 10, "{spends} charges spent");
    assert!(
        unlanded_spends > 0,
        "no missed, dodged or parried swing spent a charge"
    );
}

/// A queued Heroic Strike that goes out on a Weaponmaster or Windfury Totem extra attack is
/// listed, under `--rotation-stats`, with the extra attack it replaced, so the application's
/// timeline can tell it from a strike that took a regular swing (MythicSim Discord #warrior,
/// 30 October 2026). Each listed strike is a cast at that time in the first fight's log, and
/// the result without the adapter's additions does not carry the list.
#[test]
fn a_strike_on_an_extra_attack_names_the_extra_attack() {
    let mut sources = std::collections::BTreeSet::new();
    for seed in 1..=12 {
        let mut value = warrior_fixture("arms-warrior-warblade-of-caer-darrow");
        value["sim"]["iterations"] = json!(1);
        value["sim"]["debug_first_iteration"] = json!(true);
        value["sim"]["seed"] = json!(seed);
        // Queue Heroic Strike whenever it can be paid for, as a custom rotation can, rather
        // than only above 100 rage and in execute.
        let rotation = value["player"]["rotation"]["priorityList"]
            .as_array_mut()
            .unwrap();
        let strike = rotation
            .iter_mut()
            .find(|line| line["action"]["castSpell"]["spellId"]["spellId"] == 25286)
            .unwrap();
        strike["action"]
            .as_object_mut()
            .unwrap()
            .remove("condition");
        let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
        let mut report =
            forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic)
                .unwrap();
        let player = "/raidMetrics/parties/0/players/0";
        assert!(report
            .result
            .pointer(&format!("{player}/extraAttackStrikes"))
            .is_none());
        assert!(report.add_rotation_stats());
        let logs = report.result["logs"].as_str().unwrap().to_string();
        let strikes = report
            .result
            .pointer(&format!("{player}/extraAttackStrikes"))
            .unwrap();
        for strike in strikes.as_array().unwrap() {
            assert_eq!(strike["strike"], "{SpellID: 25286}", "{strike}");
            let extra = strike["extraAttack"].as_str().unwrap();
            assert!(
                extra == "{OtherID: 3, Tag: 1290261}" || extra == "{OtherID: 3, Tag: 25584}",
                "{strike}"
            );
            let time = format!("{:.2}", strike["time"].as_f64().unwrap());
            assert!(
                has_line(&logs, &time, "Casting {SpellID: 25286}"),
                "seed {seed}: no Heroic Strike cast at {time}"
            );
            sources.insert(extra.to_string());
        }
    }
    // Both extra attacks hand a queued strike over within these fights.
    assert_eq!(sources.len(), 2, "{sources:?}");
}
