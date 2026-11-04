//! Shaman tests: the prepared v2 gate on the production Elemental Shaman request. Its Go
//! result and first-fight log are compared with the rest of the fixture family.

use forever_engine::{
    check_prepared, contracts::prepared_v2::PreparedV2, prepared_coverage, PreparedError,
};
use serde_json::{json, Value};
use std::{fs, path::Path};

mod prepare;

fn elemental_json() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/production-elemental-shaman.prepared.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn reasons(value: Value) -> Vec<String> {
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    match check_prepared(&prepared) {
        Err(PreparedError::Unsupported(reasons)) => reasons,
        other => panic!("expected unsupported, got {other:?}"),
    }
}

#[test]
fn production_elemental_request_is_supported() {
    let prepared: PreparedV2 = serde_json::from_value(elemental_json()).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
}

/// The rotation's Magma Totem and its Chain Lightning on several targets wait for two
/// targets, which never happens with the one target in scope, so Magma Totem would need no
/// behavior. Without that condition it does.
#[test]
fn magma_totem_is_reachable_only_without_its_target_count_condition() {
    let mut value = elemental_json();
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "magma_totem");
    let prepared: PreparedV2 = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
    let item = &mut value["player"]["rotation"]["priorityList"][1]["action"];
    assert_eq!(item["castSpell"]["spellId"]["spellId"], 10587);
    item.as_object_mut().unwrap().remove("condition");
    assert_eq!(
        reasons(value),
        ["rotation reaches spell 10587 without a known behavior"]
    );
}

#[test]
fn elemental_focus_listener_needs_its_effect() {
    let mut value = elemental_json();
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "elemental_focus");
    assert!(reasons(value).contains(
        &"player aura \"Elemental Focus\" listens to combat events without an effect".into()
    ));
}

/// Go panics when a product with a float reads a non-constant integer as a float.
#[test]
fn math_reading_an_operand_as_another_type_is_unsupported() {
    let mut value = elemental_json();
    value["player"]["rotation"]["priorityList"][6]["action"]["condition"] = json!({"cmp": {
        "op": "OpGe",
        "lhs": {"currentMana": {}},
        "rhs": {"math": {"op": "OpMul", "lhs": {"numberTargets": {}}, "rhs": {"currentManaPercent": {}}}},
    }});
    assert!(reasons(value).contains(
        &"rotation item 7: math that reads an operand as another type is unsupported".into()
    ));
}

fn enhancement_json() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/enhancement-shaman-no-battle-shout.prepared.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn production_enhancement_request_is_supported() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/production-enhancement-shaman.prepared.json");
    let prepared: PreparedV2 = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
}

#[test]
fn enhancement_request_without_battle_shout_is_supported() {
    let prepared: PreparedV2 = serde_json::from_value(enhancement_json()).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
}

/// A basic totem needs the effect that gives it a behavior. Since the 16 October 2026 beta notes
/// no air totem category joins the shaman's totems to the party's: a Grace of Air cast beside
/// the party's Windfury Totem is supported, and the shaman's own Windfury Totem there still
/// contests the party's.
#[test]
fn other_basic_totems_are_rejected() {
    let mut value = enhancement_json();
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "windfury_totem_self");
    let item = &mut value["player"]["rotation"]["priorityList"][0]["action"];
    assert_eq!(item["castSpell"]["spellId"]["spellId"], 25361);
    item["castSpell"]["spellId"]["spellId"] = json!(10614);
    assert!(reasons(value.clone())
        .contains(&"rotation reaches spell 10614, a totem without a known behavior".into()));
    assert!(crate::refusal_codes(value).contains(&(
        "class_limit",
        "rotation reaches spell 10614, a totem without a known behavior".into()
    )));

    // Grace of Air beside the party's Windfury Totem contests nothing.
    let mut value = enhancement_json();
    value["player"]["rotation"]["priorityList"][0]["action"]["castSpell"]["spellId"]["spellId"] =
        json!(25359);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    assert!(prepared_coverage(&prepared)
        .iter()
        .all(|reason| !reason.contains("Grace of Air Totem")));

    // The shaman's own Windfury Totem contests the party's.
    let mut value = enhancement_json();
    value["player"]["rotation"]["priorityList"][0]["action"]["castSpell"]["spellId"]["spellId"] =
        json!(10614);
    assert!(reasons(value).contains(
        &"rotation reaches spell 10614, a Windfury Totem that contests the party's or Windfury Weapon"
            .into()
    ));
}

/// Go reads totem slots only on a Shaman.
#[test]
fn totem_remaining_time_needs_a_shaman() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/frost-reference.prepared.json");
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    value["player"]["rotation"]["priorityList"][0]["action"]["condition"] = json!({"cmp": {
        "op": "OpLe",
        "lhs": {"totemRemainingTime": {"totemType": "Fire"}},
        "rhs": {"const": {"val": "0s"}},
    }});
    assert!(reasons(value).contains(&"rotation item 1: totemRemainingTime needs a Shaman".into()));
}

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

fn lines_with<'a>(logs: &'a str, target: u32, spell: &str, what: &str) -> Vec<&'a str> {
    logs.lines()
        .filter(|line| {
            line.contains(&format!("[Target {target}] {{SpellID: {spell}}}")) && line.contains(what)
        })
        .collect()
}

/// The number after `key` in a debug damage line.
fn debug_value(line: &str, key: &str) -> f64 {
    let value = line.split(key).nth(1).unwrap();
    value[..value.find(',').unwrap()].parse().unwrap()
}

/// Chain Lightning hits three targets from the cast target on, and no more, each hit weaker
/// than the one before it.
#[test]
fn chain_lightning_hits_three_targets() {
    let logs = first_fight_log(fixture("production-elemental-shaman-5-targets"));
    for target in 1..=3 {
        assert!(
            !lines_with(&logs, target, "10605", " for ").is_empty(),
            "Target {target}"
        );
    }
    for target in 4..=5 {
        assert!(lines_with(&logs, target, "10605", " for ").is_empty());
    }
    // The bounce reduction shows in the attacker modifiers of the first cast.
    let hits: Vec<f64> = logs
        .lines()
        .filter(|line| line.contains("{SpellID: 10605} [DEBUG]"))
        .take(3)
        .map(|line| debug_value(line, "AfterAttackerMods:"))
        .collect();
    assert!(
        hits.len() == 3 && hits[0] > hits[1] && hits[1] > hits[2],
        "{hits:?}"
    );
}

/// Magma Totem's pulses and Fire Nova reach every target.
#[test]
fn magma_totem_and_fire_nova_reach_every_target() {
    let logs = first_fight_log(fixture("production-elemental-shaman-5-targets"));
    for target in 1..=5 {
        assert!(
            !lines_with(&logs, target, "10587", " tick ").is_empty(),
            "Target {target}: no pulse"
        );
        assert!(
            !lines_with(&logs, target, "408345", " ").is_empty(),
            "Target {target}: no nova"
        );
    }
}

/// A multidot line puts Flame Shock's dot on a target whose dot is down besides the first, and
/// that dot ticks there. The rotation's other lines leave the third target for the shaman's
/// mana to decide, as in Go.
#[test]
fn a_multidot_line_puts_flame_shock_on_another_target() {
    let logs = first_fight_log(fixture("elemental-shaman-multidot-flame-shock-3-targets"));
    for target in 1..=2 {
        let tick = format!("[Target {target}] {{SpellID: 29228, Tag: 1}} tick");
        assert!(logs.lines().any(|line| line.contains(&tick)), "{tick}");
    }
}

/// Stormstrike's debuff sits on the one target it was cast on, so only that target takes the
/// bonus on the shaman's Chain Lightning, whichever hit of the cast it is.
#[test]
fn the_stormstrike_debuff_boosts_only_its_target() {
    let logs = first_fight_log(fixture("enhancement-shaman-chain-magma-4-targets"));
    let ratio =
        |line: &str| debug_value(line, "AfterTargetMods:") / debug_value(line, "AfterResistances:");
    let lines: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("{SpellID: 10605} [DEBUG]"))
        .collect();
    let mut boosted_first = false;
    for pair in lines.windows(2) {
        if pair[0].contains("[Target 1]") && pair[1].contains("[Target 2]") {
            assert!(ratio(pair[1]) < 1.2, "{}", pair[1]);
            boosted_first |= ratio(pair[0]) > 1.2;
        }
    }
    assert!(boosted_first, "{logs}");
}

/// A completed hard cast restarts the melee swing timer in Forever (Go `HoldMeleeForCast`, beta
/// logs 2701 and 2712): no swing lands inside the cast and the first swing after it comes a full
/// weapon speed later (melee haste shortens it), even when one came due during the cast. Before
/// the rule that swing landed as the cast completed.
#[test]
fn a_completed_chain_lightning_restarts_the_swing() {
    let value = fixture("enhancement-shaman-chain-magma-4-targets");
    let speed = value["melee"]["main_hand"]["swing_speed"].as_f64().unwrap();
    let logs = first_fight_log(value);
    let stamp = |line: &str| -> f64 { line[1..line.find(']').unwrap()].parse().unwrap() };
    let shaman: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("[enhancement-shaman (#1)]"))
        .collect();
    let swings: Vec<f64> = shaman
        .iter()
        .filter(|line| line.contains("Completed cast {OtherID: 3, Tag: 1}"))
        .map(|line| stamp(line))
        .collect();
    let mut casts = Vec::new();
    let mut started = None;
    for line in &shaman {
        if line.contains("Casting {SpellID: 10605}") && !line.contains("Cast Time = 0s") {
            started = Some(stamp(line));
        } else if line.contains("Completed cast {SpellID: 10605}") {
            casts.extend(started.take().map(|start| (start, stamp(line))));
        }
    }
    assert!(casts.len() >= 3 && !swings.is_empty(), "{logs}");
    for (start, end) in casts {
        for &swing in &swings {
            assert!(
                !(swing > start + 0.011 && swing < end - 0.011),
                "a swing lands at {swing} inside the cast from {start} to {end}"
            );
        }
        if let Some(&next) = swings.iter().find(|&&swing| swing > end + 0.001) {
            assert!(
                next - end >= speed / 2.0,
                "a swing at {next} follows the cast that ended at {end}"
            );
        }
    }
}

/// The threat figure of a debug damage line.
fn debug_threat(line: &str) -> f64 {
    let value = line.split("(Threat: ").nth(1).unwrap();
    value[..value.find(')').unwrap()].parse().unwrap()
}

fn rockbiter_threat_effect(value: &mut Value) -> &mut Value {
    value["effects"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|effect| effect["kind"] == "rockbiter_threat")
        .expect("a Rockbiter fixture carries the effect")
}

/// Patch 181 (wowsims/forever f128c3d268): each landed melee hit of a hand whose weapon carries
/// Rockbiter casts Rockbiter Weapon Proc (20871), 72 threat for every second of the main hand's
/// swing, multiplied by the shaman's threat multiplier like any other threat.
#[test]
fn rockbiter_adds_72_threat_a_second_of_the_main_hand_swing_to_each_hit() {
    let value = fixture("enhancement-shaman-no-battle-shout");
    let multiplier = value["player"]["pseudo_stats"]["threat_multiplier"]
        .as_f64()
        .unwrap();
    let logs = first_fight_log(value);
    let casts = lines_with(&logs, 1, "20871", "(Threat: ");
    assert!(casts.len() > 10, "{} Rockbiter threat casts", casts.len());
    for line in casts {
        assert!(line.contains(" for 0.000 damage"), "{line}");
        let want = 72.0 * 3.8 * multiplier;
        assert!(
            (debug_threat(line) - want).abs() < 1e-3,
            "{line} is not {want} threat"
        );
    }
}

/// The hand that struck must carry Rockbiter, and the flat threat follows the main hand's speed.
#[test]
fn rockbiter_threat_follows_the_hands_that_carry_it_and_the_main_hand_speed() {
    let mut value = fixture("enhancement-shaman-no-battle-shout");
    rockbiter_threat_effect(&mut value)["main_hand"] = json!(false);
    let logs = first_fight_log(value.clone());
    assert!(lines_with(&logs, 1, "20871", "").is_empty());

    // A faster main hand weapon lowers every cast.
    let mut value = fixture("enhancement-shaman-no-battle-shout");
    let multiplier = value["player"]["pseudo_stats"]["threat_multiplier"]
        .as_f64()
        .unwrap();
    rockbiter_threat_effect(&mut value)["main_hand_swing_speed"] = json!(1.5);
    let logs = first_fight_log(value);
    let casts = lines_with(&logs, 1, "20871", "(Threat: ");
    assert!(!casts.is_empty());
    for line in casts {
        assert!((debug_threat(line) - 72.0 * 1.5 * multiplier).abs() < 1e-3);
    }
}

/// The listener on every landed hit needs the effect that casts the threat spell.
#[test]
fn rockbiter_threat_listener_needs_its_effect() {
    let mut value = fixture("enhancement-shaman-no-battle-shout");
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "rockbiter_threat");
    assert!(reasons(value).contains(
        &"player aura \"Rockbiter Weapon Threat\" listens to combat events without an effect"
            .into()
    ));
}

/// Improved Stormstrike's mana gained while casting and the damage per second, over a short run.
fn stormstrike_regen(mut value: Value, forever: bool) -> (Option<f64>, f64) {
    value["sim"]["iterations"] = json!(200);
    if forever {
        value["player"]["casting_regen_at_once"] = json!(true);
    }
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    let report =
        forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
    let player = &report.result["raidMetrics"]["parties"][0]["players"][0];
    let gain = player["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == json!({ "spellId": 1238931 }))
        .map(|row| row["actualGain"].as_f64().unwrap());
    (gain, player["dps"]["avg"].as_f64().unwrap())
}

/// The reference raises Improved Stormstrike's casting regeneration without refreshing the
/// cached mana tick, so the buff acts only through a later refresh. In the client the mana
/// regeneration interrupt aura acts at once: under Forever's rule the mana it gives an
/// Enhancement Shaman, short of mana in the production fight, lands in its own row.
#[test]
fn improved_stormstrike_mana_acts_at_once_in_forever() {
    let enhancement = fixture("production-enhancement-shaman");
    let (go_gain, go_dps) = stormstrike_regen(enhancement.clone(), false);
    assert_eq!(go_gain, None);
    let (gain, dps) = stormstrike_regen(enhancement, true);
    assert!(gain.expect("an Improved Stormstrike row") > 0.0);
    assert!(dps > go_dps, "{dps} against {go_dps}");
}

/// Water Shield (client 1.60.1.70532): an Elemental Shaman who takes the talent and keeps the
/// shield up gets 2% of maximum mana from a critical spell, at most one globe every 3.5 seconds,
/// in the globe spell's own row (408511).
#[test]
fn water_shield_crits_restore_mana() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/production-elemental-shaman.request.json");
    let mut request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let player = &mut request["raid"]["parties"][0]["players"][0];
    // A point of Lightning Overload moves to Water Shield (Restoration's ninth talent).
    player["talentsString"] = json!("5502331020021251-05-053050001");
    player["rotation"]["priorityList"]
        .as_array_mut()
        .unwrap()
        .insert(
            0,
            json!({"action": {
                "castSpell": {"spellId": {"spellId": 408510}},
                "condition": {"not": {"val": {"auraIsActive": {"auraId": {"spellId": 408510}}}}}
            }}),
        );
    request["simOptions"]["iterations"] = json!(200);
    let prepared =
        forever_engine::prepare_json(&serde_json::to_vec(&request).unwrap(), "x").unwrap();
    let effect = prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == "water_shield")
        .expect("a Water Shield effect");
    assert_eq!(effect["globes"], json!(3));
    assert_eq!(effect["icd_ns"], json!(3_500_000_000_i64));
    assert!(!effect["trigger_spells"].as_array().unwrap().is_empty());
    let prepared: PreparedV2 = serde_json::from_value(prepared).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
    let report =
        forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
    let player = &report.result["raidMetrics"]["parties"][0]["players"][0];
    let row = player["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == json!({ "spellId": 408511 }))
        .expect("a Water Shield mana row");
    let events = row["events"].as_f64().unwrap();
    let gain = row["gain"].as_f64().unwrap();
    assert!(events > 0.0 && gain > 0.0, "{row}");
    // No more than one globe per 3.5 seconds of each iteration (the rows are totals).
    let duration = report.result["avgIterationDuration"].as_f64().unwrap();
    let per_fight = events / 200.0;
    assert!(
        per_fight <= (duration / 3.5).ceil(),
        "{per_fight} globes in {duration} s"
    );
}
