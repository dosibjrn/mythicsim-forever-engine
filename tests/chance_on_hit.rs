//! Forever's chance on hit items (README, Rust-only mechanics): the item effects the Go reference
//! leaves unregistered for want of a stated rate or a constructor, which Forever's rules register
//! from `CHANCE_ON_HIT_ITEMS` at 1.8% per second of the hitting weapon's speed. Each test wears
//! the item on one of fixtures/forever-gaps' requests, checks what the export carries, and that
//! the request passes the gate and the effect acts. None of it reaches the Go reference's rules.

use forever_engine::{
    check_prepared, contracts::prepared_v2::PreparedV2, prepare_json_with, simulate_prepared, Rules,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::Path};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn request(name: &str) -> Value {
    let path = Path::new(ROOT).join(format!("fixtures/forever-gaps/{name}.request.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// The client item rows, by ID.
fn item_rows() -> BTreeMap<i64, Value> {
    fs::read_to_string(Path::new(ROOT).join("data/items.jsonl"))
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let row: Value = serde_json::from_str(line).unwrap();
            (row["id"].as_i64().unwrap(), row)
        })
        .collect()
}

/// The items `CHANCE_ON_HIT_ITEMS` registers, with their kind, read from the generated table.
fn registered() -> Vec<(String, i64)> {
    let text =
        fs::read_to_string(Path::new(ROOT).join("src/prepare/forever_items_generated.rs")).unwrap();
    let table = &text[text.find("CHANCE_ON_HIT_ITEMS").unwrap()..];
    let mut items = Vec::new();
    for line in table
        .lines()
        .filter(|line| line.contains("Registration::Proc"))
    {
        let kind = line
            .split("ProcKind::")
            .nth(1)
            .unwrap()
            .split(',')
            .next()
            .unwrap();
        for part in line.split("item_id: ").skip(1) {
            let id = part.split(',').next().unwrap().trim().parse().unwrap();
            items.push((kind.to_string(), id));
        }
    }
    items
}

/// A fixture request wearing the item where it goes: a melee weapon in the main hand of a
/// Fury Warrior (a two-hander alone), an off hand weapon in a Combat Rogue's off hand, a ranged
/// weapon on a Hunter, anything else in the Warrior's place for it.
fn wearing(item: i64, rows: &BTreeMap<i64, Value>) -> Value {
    let row = &rows[&item];
    let kind = row["type"].as_str().unwrap();
    let hand = row["hand_type"].as_str().unwrap_or("");
    let (mut request, slot) = match (kind, hand) {
        ("ItemTypeRanged", _) => (request("hunter-skysight"), 15),
        ("ItemTypeWeapon", "HandTypeOffHand") => (request("combat-rogue-battle-squawk"), 15),
        ("ItemTypeTrinket", _) => (request("combat-rogue-battle-squawk"), 12),
        ("ItemTypeBack", _) => (request("dps-warrior-pummel"), 3),
        ("ItemTypeHands", _) => (request("dps-warrior-pummel"), 6),
        _ => (request("dps-warrior-pummel"), 12),
    };
    let items = request["raid"]["parties"][0]["players"][0]["equipment"]["items"]
        .as_array_mut()
        .unwrap();
    items[slot] = json!({ "id": item });
    if hand == "HandTypeTwoHand" && slot == 12 {
        items[13] = json!({});
    }
    request["simOptions"]["iterations"] = json!(3);
    request
}

fn prepared(request: &Value, rules: Rules) -> Result<Value, String> {
    prepare_json_with(request.to_string().as_bytes(), "rust", rules).map_err(|e| e.to_string())
}

/// Every registered item prepares under Forever's rules with its effect described, and the
/// request passes the gate and runs.
#[test]
fn every_chance_on_hit_item_prepares_and_runs() {
    let rows = item_rows();
    let mut failures = Vec::new();
    for (kind, item) in registered() {
        let request = wearing(item, &rows);
        let value = match prepared(&request, Rules::Forever) {
            Ok(value) => value,
            Err(error) => {
                failures.push(format!("{item} {kind}: {error}"));
                continue;
            }
        };
        let prepared: PreparedV2 = serde_json::from_value(value.clone()).unwrap();
        if let Err(error) = check_prepared(&prepared) {
            failures.push(format!("{item} {kind}: gate {error:?}"));
            continue;
        }
        if let Err(error) = simulate_prepared(&prepared) {
            failures.push(format!("{item} {kind}: run {error:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The request with `item` worn, `iterations` of it, and `targets` copies of its target.
fn fight(item: i64, iterations: i64, targets: usize) -> Value {
    let mut request = wearing(item, &item_rows());
    request["simOptions"]["iterations"] = json!(iterations);
    let target = request["encounter"]["targets"][0].clone();
    request["encounter"]["targets"] = json!(vec![target; targets]);
    request
}

/// The request prepared under Forever's rules, through the gate, and its player's metrics.
fn run(request: &Value) -> (Value, Value) {
    let value = prepared(request, Rules::Forever).unwrap();
    let prepared: PreparedV2 = serde_json::from_value(value.clone()).unwrap();
    check_prepared(&prepared).unwrap();
    let result = simulate_prepared(&prepared).unwrap().result;
    (
        value,
        result["raidMetrics"]["parties"][0]["players"][0].clone(),
    )
}

fn effects<'a>(prepared: &'a Value, kind: &str, trigger: &str) -> Vec<&'a Value> {
    prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|effect| effect["kind"] == kind && effect["trigger_aura"] == trigger)
        .collect()
}

/// The chances an effect rolls, by spell position.
fn chances(effect: &Value) -> Vec<f64> {
    effect["chances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|chance| chance["chance"].as_f64().unwrap())
        .collect()
}

/// Outcomes (hits, crits, misses and ticks) and damage of the player's action with this spell,
/// and the targets it damaged, over every iteration.
fn action(player: &Value, spell_id: i64) -> (f64, f64, usize) {
    let mut totals = (0.0, 0.0, 0);
    for action in player["actions"].as_array().unwrap() {
        if action["id"]["spellId"].as_i64() != Some(spell_id) {
            continue;
        }
        for target in action["targets"].as_array().unwrap() {
            for outcome in ["hits", "crits", "misses", "ticks", "critTicks"] {
                totals.0 += target[outcome].as_f64().unwrap_or(0.0);
            }
            let damage = target["damage"].as_f64().unwrap_or(0.0);
            totals.1 += damage;
            if damage > 0.0 {
                totals.2 += 1;
            }
        }
    }
    totals
}

fn aura_uptime(player: &Value, spell_id: i64) -> f64 {
    player["auras"]
        .as_array()
        .unwrap()
        .iter()
        .find(|aura| aura["id"]["spellId"] == spell_id)
        .and_then(|aura| aura["uptimeSecondsAvg"].as_f64())
        .unwrap_or(0.0)
}

/// The Go reference's rules leave every one of them unregistered, as Go does.
#[test]
fn the_reference_rules_register_none_of_them() {
    for item in [17112, 12969, 17068, 11607, 12590] {
        let value = prepared(&fight(item, 3, 1), Rules::GoReference).unwrap();
        let name = item_rows()[&item]["name"].as_str().unwrap().to_string();
        let mentions = value["effects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|effect| effect.to_string().contains(&format!("\"{name}")));
        assert!(!mentions, "{name} is described under the reference's rules");
    }
}

/// Empyrean Demolisher (17112, a 2.8 sec main hand): its Haste (21165) gives 20% attack speed
/// for 10 sec, rolled at 1.8% a second of the weapon's speed on the main hand's hits, 5.04% a
/// hit. At about 1.08 procs a minute of swinging, 10 sec each, it is up about a sixth of the
/// fight; Go never procs it.
#[test]
fn empyrean_demolisher_hastes_at_the_assumed_rate() {
    let (prepared, player) = run(&fight(17112, 400, 1));
    let proc = effects(&prepared, "stat_proc", "Empyrean Demolisher");
    assert_eq!(proc.len(), 1);
    assert_eq!(proc[0]["aura"], "Empyrean Demolisher Proc");
    for chance in chances(proc[0]) {
        assert!((chance - 2.8 * 1.8 / 100.0).abs() < 1e-9, "chance {chance}");
    }
    let speed = prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| {
            effect["kind"] == "speed_aura" && effect["aura"] == "Empyrean Demolisher Proc"
        })
        .unwrap();
    assert_eq!(speed["melee_multiplier"], 1.2);
    // The main hand's white swings alone, at its base speed, roll 1.08 procs a minute; its
    // special attacks, Windfury's attacks, Flurry and the proc's own haste add hits, so a Fury
    // Warrior procs it about twice a minute and holds it up a third of the fight.
    let aura = player["auras"]
        .as_array()
        .unwrap()
        .iter()
        .find(|aura| aura["id"]["spellId"] == 21165)
        .unwrap();
    let procs_a_minute = aura["procsAvg"].as_f64().unwrap() / 2.0;
    assert!(
        (1.08..=3.5).contains(&procs_a_minute),
        "{procs_a_minute} procs a minute"
    );
    let uptime = aura_uptime(&player, 21165) / 120.0;
    let unrefreshed = procs_a_minute * 10.0 / 60.0;
    assert!(
        uptime <= unrefreshed && uptime >= 0.7 * unrefreshed,
        "Haste up {uptime} of the fight at {procs_a_minute} procs a minute"
    );
}

/// Seeping Willow (12969): 51 Nature every 2 sec for 10 sec on every enemy within 8 yards. The
/// proc rolls the magic hit on each target as its dot goes on, and the dot ticks five times.
#[test]
fn seeping_willow_ticks_on_every_target() {
    let request = fight(12969, 300, 1);
    let (prepared, single) = run(&request);
    let proc = effects(&prepared, "spell_data_damage_proc", "Seeping Willow");
    assert_eq!(proc.len(), 1);
    assert_eq!(proc[0]["periodic"]["tick_base"], 51.0);
    assert_ne!(proc[0]["periodic"]["tick_outcome"], "tick_magic_hit");
    assert_eq!(proc[0]["periodic"]["application"], "magic_hit");
    assert!(proc[0]["area"].is_object());
    for chance in chances(proc[0]) {
        assert!((chance - 3.6 * 1.8 / 100.0).abs() < 1e-9, "chance {chance}");
    }
    let (_, one, targets) = action(&single, 17196);
    assert_eq!(targets, 1);
    let (_, three, targets) = action(&run(&fight(12969, 300, 3)).1, 17196);
    assert_eq!(targets, 3);
    // Every proc reaches all three, so they take about three times the damage of one, a little
    // more since the Warrior's Whirlwind and Cleave hits on the other two roll the proc too.
    assert!(
        (2.7..=4.5).contains(&(three / one)),
        "{three} on three targets, {one} on one"
    );
}

/// Deathbringer (17068): a Shadow Bolt (1305398) of 140 Shadow damage at once on the hit, at
/// 1.8% a second of the weapon's 2.9 sec speed.
#[test]
fn deathbringer_casts_its_shadow_bolt() {
    let (prepared, player) = run(&fight(17068, 300, 1));
    let proc = effects(&prepared, "spell_data_damage_proc", "Deathbringer");
    assert_eq!(proc.len(), 1);
    assert!(proc[0]["periodic"].is_null());
    let speed = item_rows()[&17068]["weapon_speed"].as_f64().unwrap();
    for chance in chances(proc[0]) {
        assert!(
            (chance - speed * 1.8 / 100.0).abs() < 1e-9,
            "chance {chance}"
        );
    }
    let (bolts, damage, _) = action(&player, 1305398);
    assert!(bolts / 300.0 > 1.0, "{} bolts a fight", bolts / 300.0);
    assert!(
        (100.0..=250.0).contains(&(damage / bolts)),
        "{} a bolt",
        damage / bolts
    );
}

/// Dark Iron Sunderer (11607): its chance on hit takes 300 armor off the target for 20 sec, as
/// one stack of the row's aura.
#[test]
fn dark_iron_sunderer_takes_the_targets_armor() {
    let (prepared, _) = run(&fight(11607, 50, 1));
    let proc = effects(&prepared, "armor_debuff_proc", "Dark Iron Sunderer");
    assert_eq!(proc.len(), 1);
    assert_eq!(proc[0]["armor_by_stacks"], json!([0.0, -300.0]));
    assert_eq!(proc[0]["immediate"], true);
}

/// Felstriker (12590): every attack hits and crits for 3 sec, read from its row as 100% hit and
/// crit on a buff the stat auras carry.
#[test]
fn felstriker_buffs_hit_and_crit() {
    let (prepared, player) = run(&fight(12590, 200, 1));
    let proc = effects(&prepared, "stat_proc", "Felstriker");
    assert_eq!(proc.len(), 1);
    assert!(aura_uptime(&player, 16551) > 0.0);
}
