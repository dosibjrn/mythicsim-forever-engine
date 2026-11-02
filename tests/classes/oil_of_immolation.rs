//! Oil of Immolation, a MythicSim Rust-only consumable beyond the Go reference pin (client 70707,
//! item 8956, use spell 11350 "Fire Shield", whose 3 second period casts spell 11351 for a flat
//! 50 Fire damage on the enemies within 5 yards). No Go golden exists, so these tests pin the
//! client rows' numbers. The character uses the oil half a second before the pull, where no swing
//! is running, and each use in combat stalls the running melee swings for half a second (a
//! player's report, not a client row).

use forever_engine::{contracts::prepared_v2::PreparedV2, prepare_json};
use serde_json::{json, Value};
use std::{fs, path::Path};

const USE_SPELL: i64 = 11350;
const TICK_SPELL: i64 = 11351;
const FIGHT_SECONDS: u64 = 120;
const ITERATIONS: u64 = 20;
/// Mighty Rage Potion, which a Combat Rogue can drink: a potion with its own two minute cooldown.
const POTION: i64 = 13442;

/// A Combat Rogue request (melee, at the target) for a fixed `FIGHT_SECONDS` fight of `targets`
/// identical targets, changed by `edit` at the player.
fn request(targets: usize, edit: impl FnOnce(&mut Value)) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/classes/rogue/prepare/combat-wound-poison.request.json");
    let mut request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    request["encounter"]["duration"] = json!(FIGHT_SECONDS);
    request["encounter"]["durationVariation"] = json!(0);
    let target = request["encounter"]["targets"][0].clone();
    request["encounter"]["targets"] = json!(vec![target; targets]);
    request["simOptions"]["iterations"] = json!(ITERATIONS);
    request["simOptions"]["debugFirstIteration"] = json!(false);
    edit(&mut request["raid"]["parties"][0]["players"][0]);
    request
}

fn prepare(request: &Value) -> Value {
    prepare_json(&serde_json::to_vec(request).unwrap(), "oil-of-immolation").unwrap()
}

fn simulate(request: &Value) -> (Value, Value) {
    let prepared = prepare(request);
    let typed: PreparedV2 = serde_json::from_value(prepared.clone()).unwrap();
    let report = forever_engine::simulate_prepared(&typed).unwrap();
    (prepared, report.result)
}

fn player(result: &Value) -> &Value {
    &result["raidMetrics"]["parties"][0]["players"][0]
}

fn action(result: &Value, spell: i64) -> Option<&Value> {
    player(result)["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["id"] == json!({ "spellId": spell }))
}

/// The ticks a target took over the run, landed or not.
fn ticks(action: &Value, target: usize) -> f64 {
    let metrics = &action["targets"][target];
    ["hits", "crits", "misses"]
        .iter()
        .map(|key| metrics[key].as_f64().unwrap_or(0.0))
        .sum()
}

fn oil_effect(prepared: &Value) -> Option<&Value> {
    prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == "oil_of_immolation")
}

fn oil_on(player: &mut Value) {
    player["consumables"]["oilOfImmolation"] = json!(true);
}

/// The shield's seconds up in an average fight.
fn uptime(result: &Value) -> f64 {
    player(result)["auras"]
        .as_array()
        .unwrap()
        .iter()
        .find(|aura| aura["id"] == json!({ "spellId": USE_SPELL }))
        .expect("the shield aura reports")["uptimeSecondsAvg"]
        .as_f64()
        .unwrap()
}

/// The item's literals are the client rows': a 15 second shield with a 3 second period is five
/// ticks of 50 Fire damage, on a spell that raises no proc of its own.
#[test]
fn the_prepared_effect_states_the_client_rows() {
    let prepared = prepare(&request(1, oil_on));
    let effect = oil_effect(&prepared).expect("the oil is described");
    assert_eq!(effect["item_id"], 8956);
    assert_eq!(effect["period_ns"], 3_000_000_000_i64);
    assert_eq!(effect["ticks"], 5);
    assert_eq!(effect["radius"], 5.0);
    assert_eq!(effect["damage"], 50.0);
    assert_eq!(effect["swing_delay_ns"], 500_000_000_i64);
    assert_eq!(effect["prepot_ns"], 500_000_000_i64);
    let aura = prepared["player"]["auras"]
        .as_array()
        .unwrap()
        .iter()
        .find(|aura| aura["label"] == effect["aura"])
        .expect("the shield aura");
    assert_eq!(aura["action_id"]["spell_id"], USE_SPELL);
    assert_eq!(aura["duration_ns"], 15_000_000_000_i64);
    let tick = &prepared["player"]["spells"][effect["spell"].as_u64().unwrap() as usize];
    assert_eq!(tick["action_id"]["spell_id"], TICK_SPELL);
    assert_eq!(tick["school"], 4, "Fire");
    assert_eq!(tick["defense_type"], "DefenseTypeMagic");
    assert_eq!(tick["bonus_coefficient"], 0.0, "no spell power");
    assert_eq!(tick["proc_mask"], json!(["ProcMaskEmpty"]));
    assert_eq!(tick["cast_kind"], "autos_or_procs", "no GCD, no cooldown");
    assert!(tick["cost"].is_null());
}

/// A melee character keeps the shield up for the whole fight and takes a tick every 3 seconds,
/// each a flat 50, or 75 when it crits.
#[test]
fn a_melee_character_has_the_shield_up_and_a_tick_every_3_seconds() {
    let (_, result) = simulate(&request(1, oil_on));
    assert_eq!(uptime(&result), FIGHT_SECONDS as f64);
    let tick = action(&result, TICK_SPELL).expect("the tick spell is cast");
    assert_eq!(ticks(tick, 0), (ITERATIONS * FIGHT_SECONDS / 3) as f64);
    let target = &tick["targets"][0];
    assert_eq!(target["hitRange"]["min"], 50.0);
    assert_eq!(target["hitRange"]["max"], 50.0);
    assert_eq!(target["critRange"]["min"], target["critRange"]["max"]);
    let hits = target["hits"].as_f64().unwrap();
    let crits = target["crits"].as_f64().unwrap();
    assert!(hits > 0.0 && crits > 0.0, "{hits} hits, {crits} crits");
    // A hit of 50, a crit of the spell's multiplier, nothing else: no spell power, no variance.
    assert_eq!(
        target["damage"].as_f64().unwrap(),
        hits * 50.0 + target["critDamage"].as_f64().unwrap()
    );
}

/// The ticks reach every target in range, each on its own roll.
#[test]
fn three_targets_in_range_take_three_times_the_ticks() {
    let (_, result) = simulate(&request(3, oil_on));
    let tick = action(&result, TICK_SPELL).expect("the tick spell is cast");
    let each = (ITERATIONS * FIGHT_SECONDS / 3) as f64;
    for target in 0..3 {
        assert_eq!(ticks(tick, target), each, "target {target}");
        assert_eq!(tick["targets"][target]["hitRange"]["min"], 50.0);
        assert_eq!(tick["targets"][target]["hitRange"]["max"], 50.0);
    }
    let damage = |target: usize| tick["targets"][target]["damage"].as_f64().unwrap();
    assert!(
        damage(0) != damage(1) || damage(1) != damage(2),
        "the targets roll their own hits and crits"
    );
}

/// The fight models one distance between the player and its targets: beyond the radius the
/// shield stays up and no tick lands, as for a ranged character at range; at the radius it does.
#[test]
fn a_character_beyond_the_radius_takes_no_ticks() {
    for (distance, in_range) in [(0.0, true), (5.0, true), (5.1, false), (12.0, false)] {
        let (_, result) = simulate(&request(3, |player| {
            oil_on(player);
            player["distanceFromTarget"] = json!(distance);
        }));
        assert_eq!(uptime(&result), FIGHT_SECONDS as f64, "{distance} yards");
        let ticks_dealt: f64 = action(&result, TICK_SPELL)
            .map(|tick| (0..3).map(|target| ticks(tick, target)).sum())
            .unwrap_or(0.0);
        let expected = if in_range {
            (3 * ITERATIONS * FIGHT_SECONDS / 3) as f64
        } else {
            0.0
        };
        assert_eq!(ticks_dealt, expected, "{distance} yards");
    }
}

/// Without the field, or with it false, nothing is added: the prepared value is the one the
/// request without the field prepares, and no result mentions the oil.
#[test]
fn without_the_field_nothing_changes() {
    let without = request(1, |_| {});
    let explicit = request(1, |player| {
        player["consumables"]["oilOfImmolation"] = json!(false);
    });
    assert_eq!(prepare(&without), prepare(&explicit));
    let (prepared, result) = simulate(&without);
    assert!(oil_effect(&prepared).is_none());
    assert!(action(&result, TICK_SPELL).is_none());
    assert!(player(&result)["auras"]
        .as_array()
        .unwrap()
        .iter()
        .all(|aura| aura["id"] != json!({ "spellId": USE_SPELL })));
    let (_, with) = simulate(&request(1, oil_on));
    assert!(action(&with, TICK_SPELL).is_some());
}

/// The oil's category is not the potion category and its use takes no global cooldown: a warrior
/// that drinks a potion (with a prepull and the Goblin Sapper beside it) drinks it as often, and
/// the potion's spell is prepared as it was.
#[test]
fn the_oil_leaves_the_potion_alone() {
    let load = |oil: bool| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/mage/prepared-v2/arms-warrior-challenging-shout.request.json");
        let mut request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        request["encounter"]["duration"] = json!(300);
        request["encounter"]["durationVariation"] = json!(0);
        request["simOptions"]["iterations"] = json!(ITERATIONS);
        request["simOptions"]["debugFirstIteration"] = json!(false);
        let consumables = &mut request["raid"]["parties"][0]["players"][0]["consumables"];
        assert_eq!(consumables["potId"], POTION);
        if oil {
            consumables["oilOfImmolation"] = json!(true);
        }
        request
    };
    let casts = |result: &Value| -> f64 {
        let action = player(result)["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|action| action["id"] == json!({ "itemId": POTION }))
            .expect("the potion is drunk");
        action["targets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|target| target["casts"].as_f64().unwrap_or(0.0))
            .sum()
    };
    let (without_prepared, without) = simulate(&load(false));
    let (with_prepared, with) = simulate(&load(true));
    assert!(casts(&without) > 0.0);
    assert_eq!(casts(&with), casts(&without));
    // The oil adds its own spell and aura and moves no other: the warrior's other spells are
    // the same, in the same order.
    let spells = |prepared: &Value| -> Vec<Value> {
        prepared["player"]["spells"].as_array().unwrap().clone()
    };
    let (before, mut after) = (spells(&without_prepared), spells(&with_prepared));
    let added = after
        .iter()
        .position(|spell| spell["action_id"]["spell_id"] == TICK_SPELL)
        .expect("the tick spell");
    after.remove(added);
    assert_eq!(after, before);
    assert!(action(&with, TICK_SPELL).is_some());
}

/// The first fight's log of a 40 second Combat Rogue fight (a dual wielder, in melee).
fn fight_log(oil: bool) -> String {
    let mut request = request(1, |player| {
        player["consumables"]["oilOfImmolation"] = json!(oil);
    });
    request["encounter"]["duration"] = json!(40);
    request["simOptions"]["iterations"] = json!(1);
    request["simOptions"]["debugFirstIteration"] = json!(true);
    let (_, result) = simulate(&request);
    result["logs"].as_str().unwrap().to_string()
}

/// The seconds of each log line that holds `text`.
fn times(logs: &str, text: &str) -> Vec<f64> {
    logs.lines()
        .filter(|line| line.contains(text))
        .map(|line| line[1..line.find(']').unwrap()].parse().unwrap())
        .collect()
}

/// The seconds of each swing of a hand: spell 3 of the rogue's auto attacks, tag 1 the main
/// hand and tag 2 the off hand.
fn swings(logs: &str, tag: i32) -> Vec<f64> {
    times(logs, &format!("Casting {{OtherID: 3, Tag: {tag}}}"))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.011
}

/// The first use is half a second before the pull and the ticks keep the client's timing, 3 s
/// apart from the use: the first lands 2.5 s into the fight and the fifth, as the shield ends,
/// at 14.5 s, where the second shield goes up at once.
#[test]
fn the_first_use_is_before_the_pull_and_the_ticks_follow_it() {
    let logs = fight_log(true);
    let gains = times(&logs, &format!("Aura gained: {{SpellID: {USE_SPELL}}}"));
    assert_eq!(&gains[..3], &[-0.5, 14.5, 29.5], "{gains:?}");
    let ticks = times(&logs, &format!("Casting {{SpellID: {TICK_SPELL}}}"));
    let expected: Vec<f64> = (0..13).map(|tick| 2.5 + 3.0 * tick as f64).collect();
    assert_eq!(ticks, expected);
}

/// The use before the pull finds no swing running and costs none: the first swings of both
/// hands land when they do without the oil, and so does every swing up to 9.5 s. (The ticks'
/// own rolls move the chance events after that, so later swings are not comparable.)
#[test]
fn the_use_before_the_pull_costs_no_swing_time() {
    let (with, without) = (fight_log(true), fight_log(false));
    for tag in [1, 2] {
        let before = |logs: &str| -> Vec<f64> {
            swings(logs, tag)
                .into_iter()
                .filter(|&at| at < 9.5)
                .collect()
        };
        let (with, without) = (before(&with), before(&without));
        assert!(without.len() >= 4, "tag {tag}: {without:?}");
        assert_eq!(with, without, "tag {tag}");
    }
    assert!(close(swings(&with, 1)[0], swings(&without, 1)[0]));
    assert!(close(swings(&with, 2)[0], swings(&without, 2)[0]));
}

/// A use in combat pushes the swing that was running back by half a second, in each hand: the
/// off hand's swing due at 14.98 s without the oil lands at 15.48 s, and the main hand, which
/// swung just before the renewal at 14.5 s, swings half a second later than its usual gap.
#[test]
fn a_use_in_combat_delays_the_next_swing_by_half_a_second() {
    let (with, without) = (fight_log(true), fight_log(false));
    // The off hand is untouched by anything else in this fight until the renewal, so its first
    // swing after it is the one it would have made, half a second on.
    let oh_after = |logs: &str| swings(logs, 2).into_iter().find(|&at| at > 14.5).unwrap();
    assert!(close(oh_after(&without), 14.98), "{}", oh_after(&without));
    assert!(close(oh_after(&with), oh_after(&without) + 0.5));
    // The main hand: the gap across the renewal is its usual gap and the half second.
    let mh = swings(&with, 1);
    let across = mh.iter().position(|&at| at > 14.5).unwrap();
    let usual = mh[across - 1] - mh[across - 2];
    assert!(mh[across - 1] < 14.5 && usual > 1.0);
    assert!(
        close(mh[across] - mh[across - 1], usual + 0.5),
        "{usual} then {}",
        mh[across] - mh[across - 1]
    );
    // The second renewal, at 29.5 s, stalls the off hand again.
    let oh = swings(&with, 2);
    let across = oh.iter().position(|&at| at > 29.5).unwrap();
    assert!(close(oh[across] - oh[across - 1], 2.3 + 0.5), "{oh:?}");
}

/// A ranged character has no melee swing running to stall, and the ranged auto is not stalled
/// whatever the character does: a Marksmanship Hunter at range shoots at the same times with
/// and without the oil.
#[test]
fn the_ranged_auto_shot_is_not_delayed() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/classes/hunter/prepare/marksmanship-no-pet.request.json");
    let shots = |oil: bool| {
        let mut request: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        request["encounter"]["duration"] = json!(40);
        request["encounter"]["durationVariation"] = json!(0);
        request["simOptions"]["iterations"] = json!(1);
        request["simOptions"]["debugFirstIteration"] = json!(true);
        let player = &mut request["raid"]["parties"][0]["players"][0];
        player["consumables"]["oilOfImmolation"] = json!(oil);
        let (_, result) = simulate(&request);
        times(result["logs"].as_str().unwrap(), "Casting {OtherID: 4}")
    };
    let (with, without) = (shots(true), shots(false));
    assert!(without.len() > 10);
    assert_eq!(with, without);
}
