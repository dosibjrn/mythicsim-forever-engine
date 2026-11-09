//! Item effects Go registers in code that need a shared mechanic: stat auras that stack,
//! Thunderfury's slow in the attack speed category, dots that share an aura, procs of hits taken,
//! Jom Gabbar's stacks and another class's totem. Their Go results
//! and first-fight logs are compared with the rest of the fixture family; these tests keep what
//! each mechanism has to do in a fight, and what the gate refuses.

use forever_engine::{contracts::prepared_v2::PreparedV2, prepared_coverage};
use serde_json::{json, Value};
use std::{fs, path::Path};

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/mage/prepared-v2/{name}.prepared.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn first_fight_log(mut value: Value) -> String {
    value["sim"]["iterations"] = json!(1);
    value["sim"]["debug_first_iteration"] = json!(true);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    let report =
        forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
    report.result["logs"].as_str().unwrap().to_string()
}

/// The refusals of a valid prepared input, as `code: reason`.
fn refusals(value: Value) -> Vec<String> {
    crate::refusal_codes(value)
        .into_iter()
        .map(|(code, reason)| format!("{code}: {reason}"))
        .collect()
}

fn effect_of<'a>(value: &'a mut Value, kind: &str, key: &str, label: &str) -> &'a mut Value {
    value["effects"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|effect| effect["kind"] == kind && effect[key] == label)
        .unwrap_or_else(|| panic!("no {kind} effect for {label}"))
}

/// How many times the target swings in the first fight.
fn target_swings(logs: &str) -> usize {
    logs.lines()
        .filter(|line| line.contains("] [Target 1] Casting {OtherID: 3, Tag: 1}"))
        .count()
}

/// Each stack of Bonereaver's Edge adds 700 armor penetration, and the stat auras number a
/// combination by the stacks, not only by whether the aura is up.
#[test]
fn a_stacking_stat_aura_is_numbered_by_its_stacks() {
    for name in [
        "warrior-bonereavers-edge",
        "protection-paladin-bonereavers-edge",
        "survival-hunter-bonereavers-edge",
        "combat-rogue-3-targets-bonereavers-edge",
    ] {
        let value = fixture(name);
        let auras = value["effects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|effect| effect["kind"] == "stat_auras")
            .unwrap();
        let labels: Vec<&str> = auras["auras"]
            .as_array()
            .unwrap()
            .iter()
            .map(|label| label.as_str().unwrap())
            .collect();
        let stacks: Vec<i64> = auras["stacks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|stacks| stacks.as_i64().unwrap())
            .collect();
        let position = labels
            .iter()
            .position(|label| *label == "Bonereaver's Edge")
            .unwrap();
        assert_eq!(stacks[position], 3, "{name}");
        // The bits before the aura: one for each aura read as active or not.
        let offset: u32 = stacks[..position]
            .iter()
            .map(|stacks| if *stacks > 0 { 2 } else { 1 })
            .sum();
        let total: u32 = stacks
            .iter()
            .map(|stacks| if *stacks > 0 { 2 } else { 1 })
            .sum();
        let combos = auras["combos"].as_array().unwrap();
        assert_eq!(combos.len(), 1 << total, "{name}");
        assert!(auras["changed"]
            .as_array()
            .unwrap()
            .contains(&json!("ArmorPenetration")));
        let penetration = |combo: usize| combos[combo]["ArmorPenetration"].as_f64().unwrap();
        for level in 0..=3usize {
            assert_eq!(
                penetration(level << offset) - penetration(0),
                700.0 * level as f64,
                "{name}"
            );
        }
        assert_eq!(
            prepared_coverage(&serde_json::from_value(value).unwrap()),
            Vec::<String>::new()
        );
    }
}

/// The handler adds a stack after it activates the aura: the first fight climbs to 3 stacks and
/// loses them all when the aura fades. Without the stack the aura never holds any.
#[test]
fn a_weapon_proc_that_stacks_adds_a_stack_after_activating() {
    let value = fixture("warrior-bonereavers-edge");
    let logs = first_fight_log(value.clone());
    for climb in ["0 --> 1", "1 --> 2", "2 --> 3", "3 --> 0"] {
        assert!(
            logs.lines()
                .any(|line| line.contains(&format!("{{SpellID: 21153}} stacks: {climb}"))),
            "{climb}"
        );
    }

    let mut without = value;
    effect_of(
        &mut without,
        "stat_proc",
        "trigger_aura",
        "Bonereaver's Edge Proc",
    )
    .as_object_mut()
    .unwrap()
    .remove("add_stack");
    let logs = first_fight_log(without);
    assert!(!logs.contains("{SpellID: 21153} stacks:"));
}

/// Armor penetration comes off the target's armor in each hit: the same fight with the aura
/// stripped of its penetration takes more armor from every physical hit.
#[test]
fn armor_penetration_is_read_live() {
    let value = fixture("warrior-bonereavers-edge");
    let reduced = |value: &Value| -> f64 {
        let mut value = value.clone();
        value["sim"]["iterations"] = json!(1);
        let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
        forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic)
            .unwrap()
            .result["raidMetrics"]["dps"]["avg"]
            .as_f64()
            .unwrap()
    };
    let mut flat = value.clone();
    for effect in flat["effects"].as_array_mut().unwrap() {
        if effect["kind"] == "stat_auras" {
            for combo in effect["combos"].as_array_mut().unwrap() {
                combo["ArmorPenetration"] = json!(0.0);
            }
        }
    }
    assert!(reduced(&value) > reduced(&flat));
}

/// A stat proc with stacks needs the effect that adds them: the trigger listens to hits.
#[test]
fn bonereavers_edge_needs_its_effect() {
    let mut value = fixture("warrior-bonereavers-edge");
    let index = value["effects"]
        .as_array()
        .unwrap()
        .iter()
        .position(|effect| effect["trigger_aura"] == "Bonereaver's Edge Proc")
        .unwrap();
    value["effects"].as_array_mut().unwrap().remove(index);
    assert!(refusals(value).contains(
        &"aura_listener_unclaimed: player aura \"Bonereaver's Edge Proc\" listens to combat \
          events without an effect"
            .to_string()
    ));
}

/// Thunderfury casts its strike and then its bounce, a batch window after the hit, and a landed
/// strike puts Cyclone on its target.
#[test]
fn thunderfury_strikes_then_bounces() {
    let logs = first_fight_log(fixture("warrior-thunderfury"));
    // The first strike that lands: one that misses puts nothing on the target.
    let all: Vec<&str> = logs.lines().collect();
    let strike = (0..all.len())
        .find(|&index| {
            all[index].contains("Casting {SpellID: 21992, Tag: 1}")
                && all[index..index + 12]
                    .iter()
                    .any(|line| line.contains("{SpellID: 21992, Tag: 1} Hit for"))
        })
        .expect("a strike that lands");
    let lines: Vec<&str> = logs.lines().skip(strike).take(12).collect();
    let bounce = lines
        .iter()
        .position(|line| line.contains("Casting {SpellID: 21992, Tag: 2}"))
        .expect("the bounce");
    let cyclone = lines
        .iter()
        .position(|line| line.contains("[Target 1] Aura gained: {SpellID: 27648}"))
        .expect("Cyclone");
    let resistance = lines
        .iter()
        .position(|line| line.contains("[Target 1] Aura gained: {SpellID: 21992}"))
        .expect("the resistance aura");
    assert!(cyclone < bounce && bounce < resistance, "{lines:#?}");
}

/// Cyclone is the only slow of the attack speed category here, so the target swings slower than
/// it would without it, and a slow of nothing leaves the swings as they were.
#[test]
fn cyclone_slows_the_target_that_swings_at_the_tank() {
    let value = fixture("protection-paladin-thunderfury");
    let mut unslowed = value.clone();
    effect_of(
        &mut unslowed,
        "thunderfury",
        "trigger_aura",
        "Thunderfury Proc",
    )["slow_multiplier"] = json!(1.0);
    let slowed = target_swings(&first_fight_log(value));
    let unslowed = target_swings(&first_fight_log(unslowed));
    assert!(slowed < unslowed, "{slowed} swings against {unslowed}");
}

/// The raid's permanent Thunder Clap bids as much and outlasts Cyclone, so it keeps the
/// category and Cyclone slows nothing more.
#[test]
fn a_permanent_clap_keeps_the_attack_speed_category_from_cyclone() {
    let value = fixture("protection-paladin-thunderfury-thunder-clap-debuff");
    let mut unslowed = value.clone();
    effect_of(
        &mut unslowed,
        "thunderfury",
        "trigger_aura",
        "Thunderfury Proc",
    )["slow_multiplier"] = json!(1.0);
    let logs = first_fight_log(value);
    assert!(logs.contains("[Target 1] Aura gained: {SpellID: 27648}"));
    assert_eq!(
        target_swings(&logs),
        target_swings(&first_fight_log(unslowed))
    );
}

/// The warrior's Thunder Clap and Cyclone bid in one category, and a warrior that casts the clap
/// under Thunderfury is prepared with the clap's effect: both slows are described. The two bid
/// the same, so the clap keeps the category while it outlasts each Cyclone gained beside it (Go
/// `keepsTie`): the target swings as it would without Cyclone's slow until Cyclone is up without
/// the clap or gained when the clap no longer outlasts it. The fight compared is the first, from
/// the fixture's seed, whose first clap lands before Cyclone.
#[test]
fn a_warrior_shares_the_attack_speed_category_with_cyclone() {
    const CLAP: &str = "{SpellID: 11581}";
    const CYCLONE: &str = "{SpellID: 27648}";
    let at = |line: &str| -> f64 { line[1..line.find(']').unwrap()].parse().unwrap() };
    // Thunder Clap (Player) lasts 30 seconds and Cyclone 12, the prepared auras' durations.
    const CLAP_SECONDS: f64 = 30.0;
    const CYCLONE_SECONDS: f64 = 12.0;
    // When the clap first stops holding the category against Cyclone (Cyclone up without the
    // clap, or gained or refreshed when the clap does not outlast it), and how often Cyclone was
    // gained beside the clap before.
    let alone = |logs: &str| -> (f64, usize) {
        let (mut clap, mut cyclone, mut gains, mut clap_ends) = (false, false, 0, 0.0);
        for line in logs.lines() {
            for (aura, up) in [(CLAP, &mut clap), (CYCLONE, &mut cyclone)] {
                if line.contains(&format!("[Target 1] Aura gained: {aura}")) {
                    *up = true;
                } else if line.contains(&format!("[Target 1] Aura faded: {aura}")) {
                    *up = false;
                }
            }
            if line.contains(&format!("[Target 1] Aura gained: {CLAP}"))
                || line.contains(&format!("[Target 1] Aura refreshed: {CLAP}"))
            {
                clap_ends = at(line) + CLAP_SECONDS;
            }
            let cyclone_applied = line.contains(&format!("[Target 1] Aura gained: {CYCLONE}"))
                || line.contains(&format!("[Target 1] Aura refreshed: {CYCLONE}"));
            if cyclone_applied && clap {
                if clap_ends <= at(line) + CYCLONE_SECONDS {
                    return (at(line), gains);
                }
                gains += 1;
            }
            if cyclone && !clap {
                return (at(line), gains);
            }
        }
        (f64::INFINITY, gains)
    };
    let swings_before = |logs: &str, end: f64| -> Vec<String> {
        logs.lines()
            .filter(|line| line.contains("] [Target 1] Casting {OtherID: 3, Tag: 1}"))
            .filter(|line| at(line) < end)
            .map(str::to_string)
            .collect()
    };
    let mut value = fixture("protection-warrior-3-targets-thunderfury");
    let seed = value["sim"]["seed"].as_i64().unwrap();
    let (logs, end) = (seed..seed + 100)
        .find_map(|candidate| {
            value["sim"]["seed"] = json!(candidate);
            let logs = first_fight_log(value.clone());
            let (end, gains) = alone(&logs);
            (gains > 0).then_some((logs, end))
        })
        .expect("a seed whose first Thunder Clap lands before Cyclone");
    let kinds: Vec<&str> = value["effects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|effect| effect["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"thunder_clap") && kinds.contains(&"thunderfury"));
    let memberships = |label: &str| -> Value {
        value["target"]["auras"]
            .as_array()
            .unwrap()
            .iter()
            .find(|aura| aura["label"] == label)
            .unwrap()["exclusive_memberships"]
            .clone()
    };
    assert_eq!(memberships("Cyclone")[0]["category"], "AtkSpdReduction");
    let mut unslowed = value.clone();
    effect_of(
        &mut unslowed,
        "thunderfury",
        "trigger_aura",
        "Thunderfury Proc",
    )["slow_multiplier"] = json!(1.0);
    let swings = swings_before(&logs, end);
    assert!(swings.len() > 10, "{end}: {swings:?}");
    assert_eq!(swings, swings_before(&first_fight_log(unslowed), end));
    assert_eq!(
        memberships("Thunder Clap (Player)")[0]["category"],
        "AtkSpdReduction"
    );
}

/// Thunderfury's trigger and its two target auras need the effect.
#[test]
fn thunderfury_needs_its_effect() {
    let mut value = fixture("warrior-thunderfury");
    let index = value["effects"]
        .as_array()
        .unwrap()
        .iter()
        .position(|effect| effect["kind"] == "thunderfury")
        .unwrap();
    value["effects"].as_array_mut().unwrap().remove(index);
    let reasons = refusals(value);
    assert!(
        reasons.contains(
            &"aura_listener_unclaimed: player aura \"Thunderfury Proc\" listens to combat \
              events without an effect"
                .to_string()
        ),
        "{reasons:?}"
    );
}

/// Two weapons whose spells share a name put their dots on one aura, which ticks both dots.
#[test]
fn dots_that_share_an_aura_tick_together() {
    let logs = first_fight_log(fixture("warrior-plaguefang-stinging-viper"));
    let mut both = 0;
    let mut previous: Option<&str> = None;
    for line in logs.lines().filter(|line| line.contains(" tick ")) {
        let time = line.split(']').next().unwrap();
        if previous == Some(time) {
            both += 1;
        }
        previous = Some(time);
    }
    // At some second the aura held both dots, and both ticked on it.
    assert!(both > 0, "no second held a tick of both dots");
    assert!(logs.contains("{SpellID: 1309315} tick"));
    assert!(logs.contains("{SpellID: 1291663} tick"));
    // Stinging Viper's cast puts its dot on the aura Plaguefang registered, under its action.
    assert!(!logs.contains("Aura gained: {SpellID: 1291663}"));
}

/// The gate follows the sharing for weapon procs only: a dot that is not a weapon proc's
/// still cannot share an aura.
#[test]
fn only_weapon_proc_dots_share_an_aura() {
    let original = fixture("warrior-plaguefang-stinging-viper");
    let prepared: PreparedV2 = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
    let mut value = original;
    let index = value["effects"]
        .as_array()
        .unwrap()
        .iter()
        .position(|effect| effect["trigger_aura"] == "Stinging Viper Proc")
        .unwrap();
    value["effects"][index]["periodic"] = Value::Null;
    let reasons = refusals(value);
    assert!(
        reasons.iter().any(|reason| reason.starts_with(
            "effect_unimplemented: the dots of spell 1309315 and spell 1291663 share the aura"
        )),
        "{reasons:?}"
    );
}

/// The times a spell is cast at in a log.
fn casts_of(logs: &str, spell: i64) -> Vec<&str> {
    let needle = format!("Casting {{SpellID: {spell}}}");
    logs.lines()
        .filter(|line| line.contains(&needle))
        .map(|line| line.split(']').next().unwrap())
        .collect()
}

/// A damage proc of a hit taken is rolled on the target's swings at the wearer, so it is cast at
/// the time of one, and never between them.
#[test]
fn a_struck_damage_proc_is_cast_when_the_target_swings() {
    let logs = first_fight_log(fixture("protection-warrior-3-targets-totem-of-infliction"));
    let times = casts_of(&logs, 16783);
    assert!(!times.is_empty(), "the proc never fired");
    for time in times {
        let swung = logs.lines().any(|line| {
            line.starts_with(time)
                && line.contains("] [Target ")
                && line.contains("Casting {OtherID: 3, Tag: 1}")
        });
        assert!(swung, "{time} is not the time of a swing at the tank");
    }
}

/// The listener of the proc is claimed by its effect: without the effect the gate refuses it.
#[test]
fn a_struck_damage_proc_needs_its_effect() {
    let mut value = fixture("protection-warrior-3-targets-totem-of-infliction");
    let index = value["effects"]
        .as_array()
        .unwrap()
        .iter()
        .position(|effect| effect["kind"] == "spell_data_damage_proc")
        .unwrap();
    value["effects"].as_array_mut().unwrap().remove(index);
    let reasons = refusals(value);
    assert!(
        reasons
            .iter()
            .any(|reason| reason.starts_with("aura_listener_unclaimed: player aura")),
        "{reasons:?}"
    );
}

/// Jom Gabbar gains a stack every two seconds up to ten, which fall off with the aura.
#[test]
fn jom_gabbar_gains_a_stack_every_two_seconds() {
    let logs = first_fight_log(fixture("warrior-jom-gabbar"));
    let stacks: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("{SpellID: 29602} stacks: "))
        .collect();
    assert_eq!(stacks.len(), 11, "{stacks:#?}");
    assert!(stacks[0].contains("stacks: 0 --> 1"));
    assert!(stacks[9].contains("stacks: 9 --> 10"));
    assert!(stacks[10].contains("stacks: 10 --> 0"));
    let seconds: Vec<f64> = stacks
        .iter()
        .map(|line| {
            line.trim_start_matches('[')
                .split(']')
                .next()
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    for pair in seconds[..10].windows(2) {
        assert!((pair[1] - pair[0] - 2.0).abs() < 1e-9, "{seconds:?}");
    }
    assert!(logs.contains("Aura faded: {SpellID: 29602}"));
}

/// A character wears another class's idol, libram or totem as Go does: a mage in Totem of the
/// Storm prepares, passes the gate and runs.
#[test]
fn another_classs_totem_is_worn_and_run() {
    let value = fixture("frost-2-targets-totem-of-the-storm");
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
    forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
}

/// A wearer that throws a Goblin Sapper Charge hears its hit on the thrower as a hit taken: the
/// proc names that spell, and without it the same proc hears nothing in a fight nothing swings at
/// the wearer in.
#[test]
fn a_stat_proc_of_hits_taken_hears_the_sappers_hit() {
    let value = fixture("warrior-painwalker-buckler");
    let proc = value["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["trigger_aura"] == "Painwalker Buckler")
        .unwrap();
    assert_eq!(proc["kind"], "spell_data_stat_proc");
    assert_eq!(proc["callbacks"], json!(["on_spell_hit_taken"]));
    let spells = proc["trigger_spells"].as_array().unwrap();
    assert_eq!(spells.len(), 1, "{proc}");
    let dps = |value: &Value| -> f64 {
        let mut value = value.clone();
        value["sim"]["iterations"] = json!(400);
        let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
        forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic)
            .unwrap()
            .result["raidMetrics"]["dps"]["avg"]
            .as_f64()
            .unwrap()
    };
    // Drawing the proc's chance on that hit moves the random stream every later roll reads.
    let mut deaf = value.clone();
    let proc = effect_of(
        &mut deaf,
        "spell_data_stat_proc",
        "trigger_aura",
        "Painwalker Buckler",
    );
    proc["trigger_spells"] = json!([]);
    assert_ne!(dps(&value), dps(&deaf));
}

/// A request of the fixture family with `edit` applied, prepared in Rust under the reference's
/// rules.
fn prepared_request(case: &str, edit: impl FnOnce(&mut Value)) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/mage/prepared-v2/{case}.request.json"));
    let mut request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    edit(&mut request);
    forever_engine::prepare_json_with(
        &serde_json::to_vec(&request).unwrap(),
        case,
        forever_engine::Rules::GoReference,
    )
    .unwrap_or_else(|err| panic!("{case}: {err}"))
}

/// Equip these items, by equipment slot, on the request's player.
fn wear(request: &mut Value, items: &[(usize, i64)]) {
    let slots = request["raid"]["parties"][0]["players"][0]["equipment"]["items"]
        .as_array_mut()
        .unwrap();
    for &(slot, id) in items {
        slots[slot] = json!({ "id": id });
    }
}

fn combat(mut value: Value, iterations: u64) -> Value {
    value["sim"]["iterations"] = json!(iterations);
    value["sim"]["debug_first_iteration"] = json!(false);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic)
        .unwrap()
        .result
}

/// A unit's resource metric of this spell: (events, gain).
fn resource_of(unit: &Value, spell: i64) -> Option<(i64, f64)> {
    unit["resources"].as_array()?.iter().find_map(|resource| {
        (resource["id"]["spellId"] == spell).then(|| {
            (
                resource["events"].as_i64().unwrap(),
                resource["gain"].as_f64().unwrap(),
            )
        })
    })
}

fn player_of(result: &Value) -> &Value {
    &result["raidMetrics"]["parties"][0]["players"][0]
}

/// A player action's casts and damage over every unit it hit.
fn action_totals(result: &Value, spell: i64) -> (i64, f64) {
    player_of(result)["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|action| action["id"]["spellId"] == spell)
        .flat_map(|action| action["targets"].as_array().unwrap())
        .fold((0, 0.0), |(casts, damage), target| {
            (
                casts + target["casts"].as_i64().unwrap_or(0),
                damage + target["damage"].as_f64().unwrap_or(0.0),
            )
        })
}

fn has_effect(value: &Value, kind: &str, key: &str, label: &str) -> bool {
    value["effects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|effect| effect["kind"] == kind && effect[key] == label)
}

/// Nightslayer Armor's eight pieces heal the rogue for 500 a spell batch window after each
/// Vanish (Go sim/rogue/items.go, Clean Escape 23583), under the bonus's health metrics
/// (23582). Production fallbacks to Go, 25 October 2026.
#[test]
fn nightslayer_heals_a_batch_window_after_each_vanish() {
    let value = prepared_request("production-subtlety-rogue", |request| {
        let set = [16821, 16823, 16820, 16825, 16826, 16827, 16822, 16824];
        let slots = [0, 2, 4, 5, 6, 7, 8, 9];
        wear(request, &slots.into_iter().zip(set).collect::<Vec<_>>());
    });
    assert!(has_effect(
        &value,
        "health_on_cast",
        "trigger_aura",
        "Nightslayer Armor 8P"
    ));
    assert_eq!(refusals(value.clone()), Vec::<String>::new());
    let logs = first_fight_log(value.clone());
    let time = |line: &str| -> f64 { line[1..line.find(']').unwrap()].parse().unwrap() };
    let vanishes: Vec<f64> = logs
        .lines()
        .filter(|line| line.contains("Completed cast {SpellID: 1856}"))
        .map(time)
        .collect();
    let heals: Vec<f64> = logs
        .lines()
        .filter(|line| line.contains("Gained 500.000 health from {SpellID: 23582}"))
        .map(time)
        .collect();
    assert!(!vanishes.is_empty());
    assert_eq!(heals.len(), vanishes.len());
    for (heal, vanish) in heals.iter().zip(&vanishes) {
        assert!((heal - vanish - 0.01).abs() < 1e-9, "{heal} after {vanish}");
    }
    let result = combat(value, 20);
    let (vanish_casts, _) = action_totals(&result, 1856);
    let (events, gain) = resource_of(player_of(&result), 23582).unwrap();
    assert_eq!(events, vanish_casts);
    assert_eq!(gain, 500.0 * events as f64);
}

/// Stormshroud Armor (item_sets_classic.go): two pieces cast Lightning (18980), a nature hit of
/// 15 to 25, a spell batch window after 10% of the landed melee hits; three restore 30 energy
/// (23864) after 4% of them. Production fallbacks to Go, 24 and 25 October 2026.
#[test]
fn stormshroud_casts_lightning_and_restores_energy() {
    let value = prepared_request("production-combat-rogue", |request| {
        wear(request, &[(2, 15058), (4, 15056), (8, 15057), (6, 21278)]);
    });
    assert!(has_effect(
        &value,
        "spell_data_damage_proc",
        "trigger_aura",
        "Stormshroud Armor 2P"
    ));
    assert!(has_effect(
        &value,
        "energize_proc",
        "trigger_aura",
        "Stormshroud Armor 3P"
    ));
    assert_eq!(refusals(value.clone()), Vec::<String>::new());
    let result = combat(value, 20);
    let (_, damage) = action_totals(&result, 18980);
    assert!(damage > 0.0, "Lightning dealt {damage}");
    let (events, gain) = resource_of(player_of(&result), 23864).unwrap();
    assert!(events > 0);
    assert_eq!(gain, 30.0 * events as f64);
}

/// The raid's Judgement of Light heals whoever lands a melee hit on the target, half the time, a
/// spell batch window later (Go buffs/paladin.go `AttachJudgementOfLightHeal`); the health
/// metrics are the target's. A pet that strikes in melee rolls the same chance, and its heal
/// changes nothing, as a pet is always at full health. Production fallbacks to Go, 25 October
/// 2026, and a Demonology Warlock's Succubus, 27 October 2026.
#[test]
fn judgement_of_light_heals_the_player_under_the_targets_metrics() {
    let value = prepared_request("production-combat-rogue", |request| {
        request["raid"]["debuffs"]["judgementOfLight"] = true.into();
    });
    assert!(has_effect(
        &value,
        "judgement_of_light",
        "aura",
        "Judgement of Light (External)"
    ));
    assert_eq!(refusals(value.clone()), Vec::<String>::new());
    let result = combat(value, 20);
    let target = &result["encounterMetrics"]["targets"][0];
    let (events, gain) = resource_of(target, 20346).unwrap();
    assert!(events > 0);
    assert_eq!(gain, 61.0 * events as f64);
    assert_eq!(resource_of(player_of(&result), 20346), None);

    let hunter = prepared_request("beast-mastery-hunter-pet-uptime-half", |request| {
        request["raid"]["debuffs"]["judgementOfLight"] = true.into();
    });
    assert!(!refusals(hunter)
        .iter()
        .any(|reason| reason.contains("Judgement of Light")));
}

/// Deathbone Guardian's three pieces roll on the melee hits the wearer takes. Nothing strikes a
/// player who does not tank, so the bonus is inert there. A tank's is a struck stat proc under
/// Forever's rules and stays refused under Go's.
#[test]
fn deathbone_surge_is_inert_without_a_tank() {
    let deathbone = [(6, 14622), (7, 14620), (9, 14621)];
    let value = prepared_request("production-combat-rogue", |request| {
        wear(request, &deathbone)
    });
    assert!(has_effect(
        &value,
        "inert_listener",
        "aura",
        "Deathbone Guardian 3P"
    ));
    assert_eq!(refusals(value), Vec::<String>::new());

    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/apl-protection-warrior-boss-target.request.json");
    let mut request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    wear(&mut request, &deathbone);
    let refused = forever_engine::prepare_json_with(
        &serde_json::to_vec(&request).unwrap(),
        "deathbone-tank",
        forever_engine::Rules::GoReference,
    );
    match refused {
        Err(forever_engine::PrepareError::Refused(refusal)) => assert_eq!(
            refusal.reason,
            "player aura \"Deathbone Guardian 3P\" reacts to the target's swings"
        ),
        other => panic!("a tank's Deathbone Surge prepared: {other:?}"),
    }
    let forever = forever_engine::prepare_json_with(
        &serde_json::to_vec(&request).unwrap(),
        "deathbone-tank",
        forever_engine::Rules::Forever,
    )
    .unwrap();
    assert!(has_effect(
        &forever,
        "spell_data_stat_proc",
        "aura",
        "Deathbone Surge"
    ));
}

/// The party's Flametongue Totem on a character no class effect describes: while it alone holds
/// its category, the trigger casts the totem's Fire hit (16389) at once off each landed main hand
/// auto, from the main hand's speed. A Windfury Totem outbids it, and the trigger never turns on.
#[test]
fn the_partys_flametongue_totem_hits_off_main_hand_autos() {
    let value = prepared_request("production-combat-rogue", |request| {
        let buffs = &mut request["raid"]["parties"][0]["buffs"];
        buffs.as_object_mut().unwrap().remove("windfuryTotem");
        buffs["flametongueTotem"] = true.into();
    });
    assert!(has_effect(
        &value,
        "spell_data_damage_proc",
        "trigger_aura",
        "Flametongue Totem Trigger"
    ));
    assert_eq!(refusals(value.clone()), Vec::<String>::new());
    let (_, damage) = action_totals(&combat(value, 20), 16389);
    assert!(damage > 0.0, "the totem's hit dealt {damage}");

    let value = prepared_request("production-combat-rogue", |request| {
        request["raid"]["parties"][0]["buffs"]["flametongueTotem"] = true.into();
    });
    assert!(!has_effect(
        &value,
        "spell_data_damage_proc",
        "trigger_aura",
        "Flametongue Totem Trigger"
    ));
    assert_eq!(refusals(value), Vec::<String>::new());
}

/// A Rogue fixture request with these items in their equipment slots, by slot index.
fn rogue_request(items: &[(usize, i64)]) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/classes/rogue/prepare/assassination.request.json");
    let mut request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let slots = request["raid"]["parties"][0]["players"][0]["equipment"]["items"]
        .as_array_mut()
        .unwrap();
    for &(slot, id) in items {
        slots[slot] = json!({ "id": id });
    }
    request["simOptions"]["iterations"] = json!(300);
    request
}

fn prepare_forever(request: &Value) -> Value {
    forever_engine::prepare_json_with(
        request.to_string().as_bytes(),
        "rust",
        forever_engine::Rules::Forever,
    )
    .unwrap()
}

fn dps(prepared: &Value) -> f64 {
    let prepared: PreparedV2 = serde_json::from_value(prepared.clone()).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
    let report =
        forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
    report.result["raidMetrics"]["dps"]["avg"].as_f64().unwrap()
}

/// Badge of the Swarmguard under Forever's rules: its use opens a 30 second window in which
/// landed melee and ranged hits that deal damage stack Insight of the Qiraji (200 armor
/// penetration, 6 at most) at 10 procs a minute, Go's `NewStackingStatBonusCD` with a procs per
/// minute manager over the trigger's mask. Go's exporter has no effect for it, so Rust prepares
/// it itself. On six production rogue requests Rust's gain from the badge matched Go's engine
/// within 0.1 DPS (Combat +43.1, Assassination +29.5 to +37.0, Subtlety +30.5 to +30.9).
#[test]
fn badge_of_the_swarmguard_stacks_on_a_proc_rate() {
    let with = prepare_forever(&rogue_request(&[(12, 21670)]));
    let badge = with["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == "stacking_cd_on_use" && effect["item_id"] == 21670)
        .expect("a stacking effect for the badge");
    assert_eq!(badge["stat_aura"], "Badge of the Swarmguard Proc");
    assert_ne!(badge["window_aura"], badge["stat_aura"]);
    assert_eq!(badge["rng_label"], "Badge of the Swarmguard");
    assert_eq!(badge["require_damage"], true);
    assert_eq!(badge["landed_only"], true);
    let heard = badge["trigger_spells"].as_array().unwrap();
    let chances = badge["chances"].as_array().unwrap();
    assert!(!heard.is_empty() && chances.len() == heard.len(), "{badge}");
    for chance in chances {
        let chance = chance["chance"].as_f64().unwrap();
        // 10 a minute over a weapon of 1.3 to 3.8 seconds.
        assert!(chance > 0.2 && chance < 0.65, "{chance}");
    }
    let without = prepare_forever(&rogue_request(&[]));
    let (with, without) = (dps(&with), dps(&without));
    assert!(
        with > without * 1.01,
        "the badge gained {with} over {without}"
    );
}

/// The reference's rules keep Go's exporter, which has no effect for the badge.
#[test]
fn badge_of_the_swarmguard_is_unrepresented_under_go_rules() {
    let request = rogue_request(&[(12, 21670)]);
    let prepared = forever_engine::prepare_json_with(
        request.to_string().as_bytes(),
        "rust",
        forever_engine::Rules::GoReference,
    );
    match prepared {
        Err(error) => assert!(error.to_string().contains("21670"), "{error}"),
        Ok(value) => assert!(
            !value["effects"]
                .as_array()
                .unwrap()
                .iter()
                .any(|effect| effect["kind"] == "stacking_cd_on_use"),
            "Go's rules prepared the badge"
        ),
    }
}
