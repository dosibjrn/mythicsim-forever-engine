//! Item swapping (sim/core/item_swaps.go): MythicSim's Manual Crowd Pummeler opener, which
//! swaps the main hand back to the player's own weapon once the Pummeler's haste fades, and the
//! targeted swaps of the fixture family (dual wield, a shield, a swap while casting or on the
//! global cooldown, a swap weapon that carries weapon skill). Their Go results and first-fight
//! logs are compared with the rest of the family; these tests keep what the swap does in a fight
//! and what the gate refuses.

use forever_engine::{contracts::prepared_v2::PreparedV2, prepared_coverage};
use serde_json::{json, Value};
use std::{fs, path::Path};

const SUPPORTED: [&str; 14] = [
    "mcp-shockadin-paladin",
    "mcp-retribution-paladin",
    "mcp-arms-warrior",
    "mcp-enhancement-shaman",
    "mcp-elehance-shaman-2-targets",
    "mcp-elehance-shaman-3-targets",
    "mcp-feral-druid",
    "mcp-feral-bear-druid",
    "item-swap-weapon-skill",
    "item-swap-dual-wield",
    "item-swap-shield",
    "item-swap-mid-cast",
    "item-swap-mid-gcd",
    "item-swap-orc-windfury-totem",
];

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/mage/prepared-v2/{name}.prepared.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn first_fight_log(mut value: Value) -> String {
    value["sim"]["iterations"] = json!(1);
    value["sim"]["debug_first_iteration"] = json!(true);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    let report = forever_engine::simulate_prepared(&prepared).unwrap();
    report.result["logs"].as_str().unwrap().to_string()
}

/// The refusals of a valid prepared input, as `code: reason`.
fn refusals(value: Value) -> Vec<String> {
    crate::refusal_codes(value)
        .into_iter()
        .map(|(code, reason)| format!("{code}: {reason}"))
        .collect()
}

fn item_swap(value: &mut Value) -> &mut Value {
    value["effects"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|effect| effect["kind"] == "item_swap")
        .expect("an item swap effect")
}

#[test]
fn the_pummeler_openers_and_the_targeted_swaps_are_supported() {
    for case in SUPPORTED {
        let prepared: PreparedV2 = serde_json::from_value(fixture(case)).unwrap();
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new(), "{case}");
    }
}

/// The opener uses the Pummeler at the pull and swaps to the own weapon on the first evaluation
/// after its haste fades: the swap set's aura, the stats change toward Swap1 and a global
/// cooldown of at least 1.5 seconds. The iteration's end swaps back.
#[test]
fn the_opener_swaps_once_the_pummelers_haste_fades() {
    let logs = first_fight_log(fixture("mcp-arms-warrior"));
    let lines: Vec<&str> = logs.lines().collect();
    let at = |text: &str| {
        lines
            .iter()
            .position(|line| line.contains(text))
            .unwrap_or_else(|| panic!("no {text}"))
    };
    let faded = at("Aura faded: {SpellID: 13494}");
    let swap = at("Item Swap to set Swap1");
    assert!(faded < swap);
    let stats = lines[swap..]
        .iter()
        .position(|line| line.contains("Item Swap - Stats Change"))
        .unwrap();
    assert!(lines[swap + stats..]
        .iter()
        .take(3)
        .any(|line| line.contains("Extending GCD for 1.5s due to rotation / CDs.")));
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains("Item Swap - Stats Change"))
            .count(),
        2,
        "the swap and the iteration's end"
    );
}

/// A swap set weapon with skill moves its hand's table once it is worn (patch 135).
#[test]
fn the_swap_weapons_skill_moves_the_main_hand_table() {
    let mut value = fixture("item-swap-weapon-skill");
    let swapped = item_swap(&mut value)["swapped"].as_array().unwrap().clone();
    let skill = swapped
        .iter()
        .find(|change| change["path"] == "/melee/main_hand_skill")
        .expect("the main hand table changes");
    assert_eq!(skill["value"]["skill"].as_f64(), Some(8.0));
    assert!(value["melee"].get("main_hand_skill").is_none());
}

/// Spirit Weapons turns Rockbiter's threat into a bonus while Rockbiter is up; a swap that puts
/// Rockbiter on or off changes the threat multiplier, which the runtime does not follow.
#[test]
fn a_toggled_rockbiter_with_spirit_weapons_is_refused() {
    assert!(refusals(fixture("item-swap-spirit-weapons")).contains(
        &"item_swap_unsupported: an item swap changes /player/pseudo_stats/threat_multiplier, \
          which the runtime does not follow"
            .to_string()
    ));
}

#[test]
fn an_item_swap_of_an_unchecked_class_is_refused() {
    let mut value = fixture("mcp-arms-warrior");
    value["player"]["class"] = json!("ClassRogue");
    assert!(refusals(value).contains(
        &"item_swap_unsupported: an item swap of a ClassRogue is unsupported".to_string()
    ));
}

#[test]
fn a_swapped_value_no_runtime_part_follows_is_refused() {
    let mut value = fixture("mcp-arms-warrior");
    item_swap(&mut value)["swapped"]
        .as_array_mut()
        .unwrap()
        .push(json!({"path": "/player/pseudo_stats/threat_multiplier", "value": 1.3}));
    assert!(refusals(value).contains(
        &"item_swap_unsupported: an item swap changes /player/pseudo_stats/threat_multiplier, \
          which the runtime does not follow"
            .to_string()
    ));
}

/// A spell's crit with the swap set worn is its crit at the reset with the weapon
/// specializations' modifiers the swap switches; any other value is refused.
#[test]
fn a_specialization_crit_must_follow_its_modifiers() {
    let mut value = fixture("item-swap-dual-wield");
    let change = item_swap(&mut value)["swapped"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|change| {
            change["path"]
                .as_str()
                .unwrap()
                .ends_with("/bonus_crit_percent")
        })
        .expect("a crit change");
    let path = change["path"].as_str().unwrap().to_string();
    change["value"] = json!(change["value"].as_f64().unwrap() + 1.0);
    assert!(refusals(value).contains(&format!(
        "item_swap_unsupported: an item swap changes {path}, which the runtime does not follow"
    )));
}

/// Beside a Warrior's off hand, a swap only of the main hand is followed: another slot in the
/// swap set is refused.
#[test]
fn a_swap_of_another_slot_is_refused() {
    let mut value = fixture("mcp-retribution-paladin");
    item_swap(&mut value)["slots"] = json!(["ItemSlotMainHand", "ItemSlotOffHand"]);
    assert!(refusals(value)
        .iter()
        .any(|refusal| refusal.starts_with("item_swap_unsupported: an item swap of slots")));
}

/// The Warrior off hand swap family: requests the Go exporter refuses, with the pinned Go
/// engine's DPS for every iteration.
fn warrior_request(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/warrior/item-swap/{name}.request.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn warrior_prepared(request: &Value, name: &str) -> PreparedV2 {
    let bytes = serde_json::to_vec(request).unwrap();
    let prepared =
        forever_engine::prepare_json_with(&bytes, name, forever_engine::Rules::GoReference)
            .unwrap_or_else(|err| panic!("{name}: {err}"));
    serde_json::from_value(prepared).unwrap()
}

const WARRIOR_SWAPS: [&str; 3] = [
    "arms-two-hander-to-shield",
    "arms-slow-two-hander-for-mortal-strike",
    "fury-protection-shield-block-off",
];

/// A two-hander for a one-hander and shield, and back, gives Go's DPS in every iteration: the
/// main hand's attack, Two-Handed Weapon Specialization, the shield's stats, and the shield
/// casts and Shield Block falling without the shield.
#[test]
fn warrior_swaps_match_go_every_iteration() {
    for name in WARRIOR_SWAPS {
        let prepared = warrior_prepared(&warrior_request(name), name);
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new(), "{name}");
        let report = forever_engine::simulate_prepared(&prepared).unwrap();
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("fixtures/warrior/item-swap/{name}.go-dps.json"));
        let go: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        let values = |value: &Value| -> Vec<u64> {
            value
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap().to_bits())
                .collect()
        };
        assert_eq!(
            values(&report.result["raidMetrics"]["dps"]["allValues"]),
            values(&go["allValues"]),
            "{name}"
        );
    }
}

/// The swap set holds both hands, and a callback registered for both runs once.
#[test]
fn an_off_hand_swap_exports_both_hands_and_each_callback_once() {
    let name = "arms-two-hander-to-shield";
    let prepared = serde_json::to_value(warrior_prepared(&warrior_request(name), name)).unwrap();
    let effect = prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == "item_swap")
        .unwrap();
    assert_eq!(
        effect["slots"],
        json!(["ItemSlotMainHand", "ItemSlotOffHand"])
    );
    let callbacks: Vec<String> = effect["callbacks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|callback| {
            let mut callback = callback.clone();
            callback.as_object_mut().unwrap().remove("slot");
            callback.to_string()
        })
        .collect();
    for (index, callback) in callbacks.iter().enumerate() {
        assert!(
            !callbacks[..index].contains(callback),
            "exported twice: {callback}"
        );
    }
}

/// Mortal Strike lands between the swap to the slow two-hander and the swap back, which waits
/// for its cooldown.
#[test]
fn mortal_strike_hits_with_the_swapped_two_hander() {
    let name = "arms-slow-two-hander-for-mortal-strike";
    let mut request = warrior_request(name);
    request["simOptions"]["iterations"] = json!(1);
    request["simOptions"]["debugFirstIteration"] = json!(true);
    let prepared = warrior_prepared(&request, name);
    let logs = forever_engine::simulate_prepared(&prepared).unwrap().result["logs"]
        .as_str()
        .unwrap()
        .to_string();
    let lines: Vec<&str> = logs.lines().collect();
    let to_swap = lines
        .iter()
        .position(|line| line.contains("Item Swap to set Swap1"))
        .expect("a swap to the slow two-hander");
    let back = to_swap
        + lines[to_swap..]
            .iter()
            .position(|line| line.contains("Item Swap to set Main"))
            .expect("a swap back");
    assert!(lines[to_swap..back]
        .iter()
        .any(|line| line.contains("{SpellID: 21553}") && line.contains("Casting")));
}

/// The target's swings read the player's block from tables the reset's shield built, so an
/// off hand swap of a tanking Warrior is refused.
#[test]
fn an_off_hand_swap_while_tanking_is_refused() {
    let name = "fury-protection-shield-block-off";
    let mut request = warrior_request(name);
    request["raid"]["tanks"] = json!([{"index": 0, "type": "Player"}]);
    let prepared = serde_json::to_value(warrior_prepared(&request, name)).unwrap();
    assert!(refusals(prepared).contains(
        &"item_swap_unsupported: an off hand swap while the target swings at the player is \
          unsupported"
            .to_string()
    ));
}

/// A swap set that dual wields is not followed.
#[test]
fn an_off_hand_weapon_swap_is_refused() {
    let name = "arms-two-hander-to-shield";
    let mut request = warrior_request(name);
    request["raid"]["parties"][0]["players"][0]["itemSwap"]["items"][15] = json!({"id": 12940});
    let bytes = serde_json::to_vec(&request).unwrap();
    let err = forever_engine::prepare_json_with(&bytes, name, forever_engine::Rules::GoReference)
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("an item swap of an off hand weapon is unsupported"),
        "{err}"
    );
}

/// A fixture request prepared in Rust under Forever's rules.
fn prepared_forever(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/mage/prepared-v2/{name}.request.json"));
    let mut request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    request["simOptions"]["iterations"] = json!(20);
    // A paladin's talents read in push 113554's tree, which Forever's rules prepare.
    crate::paladin_113554::forever_request(&mut request);
    forever_engine::prepare_json_with(
        request.to_string().as_bytes(),
        name,
        forever_engine::Rules::Forever,
    )
    .unwrap()
}

/// Under Forever's rules the Rockbiter aura that follows the weapon in the hand says Spirit
/// Weapons' threat, which the runtime multiplies in and out as the swap puts Rockbiter on or off
/// (Go's 1.3 / 0.7, about 1.86), so the swap runs. Go's exporter does not say it, so the
/// exported fixture above stays refused.
#[test]
fn a_toggled_rockbiter_with_spirit_weapons_follows_under_forever_rules() {
    let value = prepared_forever("item-swap-spirit-weapons");
    let rockbiter = value["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == "rockbiter_weapon")
        .expect("Rockbiter Weapon");
    let threat = rockbiter["threat_multiplier"].as_f64().unwrap();
    assert!((1.85..1.87).contains(&threat), "{threat}");
    let prepared: PreparedV2 = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
    first_fight_log(value);
}

/// Seal of Light's proc manager is Go's NewLegacyPPMManager, which an item swap rebuilds; with
/// it audited, a Retribution Paladin's Manual Crowd Pummeler opener prepares in Rust.
#[test]
fn a_retribution_swap_with_seal_of_light_prepares_in_rust() {
    let value = prepared_forever("mcp-retribution-paladin");
    let prepared: PreparedV2 = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
    first_fight_log(value);
}

/// A Shockadin request (fixtures/paladin/item-swap) prepared in Rust under Forever's rules.
fn paladin_swap_forever(request: &Value) -> Value {
    forever_engine::prepare_json_with(
        request.to_string().as_bytes(),
        "rust",
        forever_engine::Rules::Forever,
    )
    .unwrap()
}

/// Seal of Righteousness rank 8's (25713) non-critical hits in a fight log, by time in seconds.
fn righteousness_hits(log: &str) -> Vec<(f64, String)> {
    log.lines()
        .filter(|line| line.contains("{SpellID: 25713} Hit for") && !line.contains("Resist"))
        .map(|line| {
            let time = line[1..line.find(']').unwrap()].parse().unwrap();
            let amount = line.split("Hit for ").nth(1).unwrap();
            (time, amount.split(' ').next().unwrap().to_string())
        })
        .collect()
}

/// The seal's rank holds only the per-hit value of a hundredth of a second of swing (the dummy
/// aura of 20293, 1786 plus its per-level gain); the hit (25713) is computed as it lands, from the
/// weapon in the hand: its speed, 0.85 or 1.2 for its hand, and its spell power share (client
/// 70707's $<1h> or $<2h>). So once a Shockadin's Manual Crowd Pummeler opener swaps back to a
/// one-hander, the seal hits as it does with the one-hander worn from the pull, not with the
/// Pummeler's two-hand share (0.12 a point of spell power against 0.10).
#[test]
fn seal_of_righteousness_hits_with_the_swapped_one_hander() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/paladin/item-swap/shockadin-mcp-to-one-hander.request.json");
    let request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let value = paladin_swap_forever(&request);
    let prepared: PreparedV2 = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
    let effects = value["effects"].as_array().unwrap();
    let seal = effects
        .iter()
        .position(|effect| effect["kind"] == "seal_of_righteousness")
        .unwrap();
    // Rank 8's judgement dummy carries 0.2 as a 32-bit float.
    let dummy = f64::from(0.2f32);
    assert_eq!(effects[seal]["hand_multiplier"].as_f64(), Some(1.2));
    assert_eq!(
        effects[seal]["ranks"][7]["hit_coefficient"].as_f64(),
        Some(dummy * 1.2 / 2.0)
    );
    let swapped: Vec<(String, f64)> = effects
        .iter()
        .find(|effect| effect["kind"] == "item_swap")
        .unwrap()["swapped"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|change| {
            let path = change["path"].as_str().unwrap();
            let field = path.strip_prefix(&format!("/effects/{seal}/"))?;
            Some((field.to_string(), change["value"].as_f64().unwrap()))
        })
        .collect();
    for change in [
        ("hand_multiplier", 0.85),
        ("swing_speed", 2.5),
        ("ranks/7/hit_coefficient", dummy / 2.0),
    ] {
        assert!(
            swapped.contains(&(change.0.to_string(), change.1)),
            "{change:?} in {swapped:?}"
        );
    }

    // The same character with Brutality Blade from the pull and no opener.
    let mut worn = request.clone();
    let player = &mut worn["raid"]["parties"][0]["players"][0];
    let rotation = player["rotation"]["priorityList"].as_array_mut().unwrap();
    rotation.drain(..2);
    player["equipment"]["items"][12] = json!({"id": 18832, "enchant": 1900});
    player["enableItemSwap"] = json!(false);
    player.as_object_mut().unwrap().remove("itemSwap");

    let opener = righteousness_hits(&first_fight_log(value));
    let swap = opener
        .iter()
        .position(|(time, _)| *time > 30.0)
        .expect("hits after the swap");
    let one_hander: std::collections::BTreeSet<String> =
        righteousness_hits(&first_fight_log(paladin_swap_forever(&worn)))
            .into_iter()
            .map(|(_, amount)| amount)
            .collect();
    for (time, amount) in &opener[..swap] {
        assert!(
            !one_hander.contains(amount),
            "{time}: {amount} before the swap"
        );
    }
    for (time, amount) in &opener[swap..] {
        assert!(
            one_hander.contains(amount),
            "{time}: {amount} not {one_hander:?}"
        );
    }
}
