//! Production requests the Rust engine used to refuse, which then ran in the Go engine
//! (fixtures/forever-gaps): each prepares under Forever's rules, passes the gate and runs, and
//! the effect it lacked acts.

use forever_engine::{
    check_prepared, contracts::prepared_v2::PreparedV2, prepare_json_with, simulate_prepared, Rules,
};
use serde_json::{json, Value};
use std::{fs, path::Path};

#[path = "support/paladin_113554.rs"]
mod paladin_113554;

/// The paladin requests recorded with push 113554's talent tree.
const RECORDED_IN_113554: &[&str] = &[
    "retribution-paladin-swap-dark-iron-sunderer",
    "protection-paladin-quel-serrar",
];

fn request(name: &str) -> Value {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/forever-gaps"
    ))
    .join(format!("{name}.request.json"));
    let mut request = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    // A paladin's talents read in push 113554's tree, which Forever's rules prepare. A request
    // recorded since that push is in its tree already.
    if !RECORDED_IN_113554.contains(&name) {
        paladin_113554::forever_request(&mut request);
    }
    request
}

/// The fixtures' Druid talents in hotfix push 113531's order, which Forever's rules read: the
/// recorded strings are refused there (tests/hotfix_113554.rs).
fn forever_talents(name: &str) -> Option<&'static str> {
    match name {
        "feral-bear-druid-thorns" => Some("-5500033212310212551-055"),
        "feral-druid-swap-heart-of-wyrmthalak" => Some("050022-55000032023012125001-055"),
        _ => None,
    }
}

fn prepared(name: &str, rules: Rules) -> Value {
    let mut request = request(name);
    if let (Rules::Forever, Some(talents)) = (rules, forever_talents(name)) {
        request["raid"]["parties"][0]["players"][0]["talentsString"] = Value::from(talents);
    }
    prepare_json_with(request.to_string().as_bytes(), "rust", rules).unwrap()
}

fn kinds(prepared: &Value) -> Vec<&str> {
    prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|effect| effect["kind"].as_str().unwrap())
        .collect()
}

/// The request under Forever's rules passes the gate; the player's metrics of one run.
fn run(name: &str) -> (Value, Value) {
    let value = prepared(name, Rules::Forever);
    let prepared: PreparedV2 = serde_json::from_value(value.clone()).unwrap();
    check_prepared(&prepared).unwrap_or_else(|error| panic!("{name}: {error:?}"));
    let result = simulate_prepared(&prepared).unwrap().result;
    let player = result["raidMetrics"]["parties"][0]["players"][0].clone();
    assert!(player["dps"]["avg"].as_f64().unwrap() > 0.0, "{name}");
    (value, player)
}

/// Casts, damage and healing of the player's action with this spell and tag.
fn action(player: &Value, spell_id: i64, tag: i64) -> (f64, f64, f64) {
    let mut totals = (0.0, 0.0, 0.0);
    for action in player["actions"].as_array().unwrap() {
        let id = &action["id"];
        if id["spellId"].as_i64() != Some(spell_id) || id["tag"].as_i64().unwrap_or(0) != tag {
            continue;
        }
        for target in action["targets"].as_array().unwrap() {
            totals.0 += target["casts"].as_f64().unwrap_or(0.0);
            totals.1 += target["damage"].as_f64().unwrap_or(0.0);
            totals.2 += target["healing"].as_f64().unwrap_or(0.0);
        }
    }
    totals
}

/// A seal twisting tank: Seal of Light (20349) heals the paladin off its 15 PPM manager
/// (20340), and its judgement puts up Judgement of Light (20346). Go's exporter describes
/// neither.
#[test]
fn seal_of_light_heals_a_tank_and_judges_light() {
    let name = "protection-paladin-seal-of-light";
    // The recorded talents, which the reference reads in Go's order.
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/forever-gaps/{name}.request.json"));
    let recorded = fs::read(path).unwrap();
    let reference = prepare_json_with(&recorded, "rust", Rules::GoReference).unwrap();
    assert!(!kinds(&reference).contains(&"seal_of_light"));
    let (value, player) = run(name);
    let kinds = kinds(&value);
    assert!(kinds.contains(&"seal_of_light"), "{kinds:?}");
    assert!(kinds.contains(&"judgement_of_light"), "{kinds:?}");
    assert!(action(&player, 20349, 0).0 > 0.0, "Seal of Light casts");
    assert!(action(&player, 20340, 0).2 > 0.0, "Seal of Light heals");
    assert!(
        action(&player, 20346, 0).0 > 0.0,
        "Judgement of Light casts"
    );
}

/// A tank's own Retribution Aura rank 2 (10298): the cast turns the aura on, and the shield
/// (tag 2) strikes back at the target's landed swings.
#[test]
fn a_tanks_own_retribution_aura_strikes_back() {
    let (value, player) = run("protection-paladin-retribution-aura");
    assert!(kinds(&value).contains(&"paladin_aura_cast"));
    assert_eq!(action(&player, 10298, 0).0, 50.0, "one cast per iteration");
    assert!(action(&player, 10298, 2).1 > 0.0, "the shield's damage");
}

/// Thunder Clap beside a permanent slow in its exclusive category runs, its bid raised over
/// the slow it shares the category with.
#[test]
fn thunder_clap_beside_a_permanent_slow_runs() {
    let (_, player) = run("protection-warrior-thunder-clap");
    assert!(action(&player, 11581, 0).0 > 0.0, "Thunder Clap casts");
}

/// Manual Crowd Pummeler openers that move Windfury Weapon to the swap weapon, against the
/// party's Windfury Totem.
#[test]
fn a_windfury_weapon_swap_runs() {
    let (value, _) = run("enhancement-shaman-windfury-swap");
    assert!(kinds(&value).contains(&"windfury_weapon"));
}

/// Retribution swaps from the Manual Crowd Pummeler onto weapons with procs of their own.
#[test]
fn retribution_swaps_onto_weapons_with_procs_run() {
    run("retribution-paladin-swap-blade-of-hanna");
    let (_, player) = run("retribution-paladin-swap-sulfuras");
    assert!(action(&player, 21162, 0).1 > 0.0, "Sulfuras's Fireball");
}

/// The gain of the player's resource row with this spell and type.
fn resource_gain(player: &Value, spell_id: i64, kind: &str) -> f64 {
    player["resources"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["id"]["spellId"].as_i64() == Some(spell_id) && row["type"] == kind)
        .map(|row| row["gain"].as_f64().unwrap_or(0.0))
        .sum()
}

/// Vampiric Embrace (15286): the cast on its cooldown puts the debuff up, and a batch window
/// after each Shadow hit or tick on the target the priest gains health. Go's exporter leaves
/// the cast unknown.
#[test]
fn vampiric_embrace_heals_the_priest() {
    let name = "shadow-priest-vampiric-embrace";
    assert!(!kinds(&prepared(name, Rules::GoReference)).contains(&"vampiric_embrace"));
    let (value, player) = run(name);
    assert!(kinds(&value).contains(&"vampiric_embrace"));
    assert!(action(&player, 15286, 0).0 > 50.0, "Vampiric Embrace casts");
    assert!(resource_gain(&player, 15286, "ResourceTypeHealth") > 0.0);
}

/// A strict sequence among the prepull actions takes control of the rotation, whose first
/// step waits on a time condition: each step casts once a fight.
#[test]
fn a_prepull_strict_sequence_runs_its_steps_once() {
    let (_, player) = run("shadow-priest-prepull-sequence");
    for spell in [20554, 14751, 1309636] {
        assert_eq!(action(&player, spell, 0).0, 50.0, "spell {spell}");
    }
}

/// Sequence steps with conditions of their own, as strict sequences have.
#[test]
fn sequence_steps_carry_conditions() {
    run("marksmanship-hunter-sequence-conditions");
}

/// Fourteen stat auras, more than the 4,096 combinations preparation used to read.
#[test]
fn fourteen_stat_auras_prepare() {
    let (value, _) = run("enhancement-shaman-many-stat-auras");
    assert!(kinds(&value).contains(&"stat_auras"));
}

/// A condition on a spell value that Go builds only for a known spell, which preparation used
/// to leave to the Go exporter.
#[test]
fn spell_values_fold_as_go_builds_them() {
    // The rotation's conditions read Light's Vigil on the enemy, which push 113554 makes friendly
    // only; Forever's rules do not model its party heal yet and refuse the rotation that reaches
    // it, so the fold is checked under the reference's rules.
    let name = "retribution-paladin-spell-value-fold";
    // Without the Manual Crowd Pummeler opener, an item swap the reference does not prepare.
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/forever-gaps/{name}.request.json"));
    let mut request_without_swap: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let player = &mut request_without_swap["raid"]["parties"][0]["players"][0];
    player.as_object_mut().unwrap().remove("itemSwap");
    player["rotation"]["priorityList"]
        .as_array_mut()
        .unwrap()
        .drain(..2);
    let value = prepare_json_with(
        request_without_swap.to_string().as_bytes(),
        "rust",
        Rules::GoReference,
    )
    .unwrap();
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    check_prepared(&prepared).unwrap_or_else(|error| panic!("{name}: {error:?}"));
    let result = simulate_prepared(&prepared).unwrap().result;
    assert!(
        result["raidMetrics"]["parties"][0]["players"][0]["dps"]["avg"]
            .as_f64()
            .unwrap()
            > 0.0
    );

    match prepare_json_with(request(name).to_string().as_bytes(), "rust", Rules::Forever) {
        Err(forever_engine::PrepareError::Refused(refusal)) => assert!(
            refusal
                .reason
                .contains("Light's Vigil on an ally (push 113554) is unsupported"),
            "{}",
            refusal.reason
        ),
        other => panic!("{:?}", other.map(|_| ())),
    }
}

/// A demon's mana percent and aura time read in the warlock's rotation, and an aura time
/// without an aura, which Go builds no value for.
#[test]
fn pet_and_missing_aura_values_run() {
    run("demonology-warlock-pet-values");
    run("demonology-warlock-pet-aura-time");
    run("combat-rogue-aura-value-without-aura");
}

/// Stormcaller's Garb (3): Lightning Bolt, Chain Lightning, Overload and Shock hits give
/// Stormcaller's Wrath (26121).
#[test]
fn stormcallers_garb_procs_its_wrath() {
    let (value, player) = run("enhancement-shaman-stormcallers-garb");
    assert!(value["effects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|effect| effect["kind"] == "spell_data_stat_proc"
            && effect["aura"] == "Stormcaller's Wrath"));
    let uptime = player["auras"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|aura| aura["id"]["spellId"].as_i64() == Some(26121))
        .map(|aura| aura["uptimeSecondsAvg"].as_f64().unwrap_or(0.0))
        .sum::<f64>();
    assert!(uptime > 0.0, "Stormcaller's Wrath uptime");
}

/// The Bear's Swipe (1264502) with three targets up cleaves them.
#[test]
fn a_bears_swipe_cleaves_three_targets() {
    let (_, player) = run("hunter-bear-swipe-three-targets");
    let pet = &player["pets"][0];
    assert!(action(pet, 1264502, 0).1 > 0.0, "Swipe damage");
}

/// Item swaps the second worker's sims carried: Frostbrand Weapon on the swap weapon, whose
/// chances follow the weapon in the hand; Heart of Wyrmthalak's proc manager, which a swap
/// rebuilds; and a swap weapon with Fiery Weapon, whose proc holds while a hand carries it.
#[test]
fn swaps_with_weapon_procs_run() {
    run("enhancement-shaman-frostbrand-swap");
    run("feral-druid-swap-heart-of-wyrmthalak");
    let (_, player) = run("enhancement-shaman-swap-fiery-weapon");
    assert!(action(&player, 13897, 0).1 > 0.0, "Fiery Weapon damage");
}

/// The raid's Judgement of Light on each of three targets, each with metrics of its own.
#[test]
fn judgement_of_light_runs_on_several_targets() {
    run("protection-paladin-judgement-of-light-three-targets");
}

/// The mana percent of a demon the fight never summons reads its full bar, as Go's reset leaves
/// it.
#[test]
fn an_unsummoned_pets_mana_percent_is_full() {
    run("demonology-warlock-unsummoned-pet-mana");
}

/// A request edited in place: its JSON text with each replacement made, which must change it.
fn edited(name: &str, replacements: &[(&str, &str)]) -> Value {
    let mut text = request(name).to_string();
    for (from, to) in replacements {
        assert!(text.contains(from), "{name} has no {from}");
        text = text.replace(from, to);
    }
    serde_json::from_str(&text).unwrap()
}

/// Prepares an edited request under Forever's rules and checks that it passes the gate and runs.
fn run_request(value: &Value) -> Value {
    let prepared = prepare_json_with(value.to_string().as_bytes(), "rust", Rules::Forever).unwrap();
    let prepared: PreparedV2 = serde_json::from_value(prepared).unwrap();
    check_prepared(&prepared).unwrap_or_else(|error| panic!("{error:?}"));
    let result = simulate_prepared(&prepared).unwrap().result;
    result["raidMetrics"]["parties"][0]["players"][0].clone()
}

/// The raw mana of a demon the fight never summons is its full bar, as Go's reset leaves it:
/// the Imp's 662 is below 700 and above 600, so Life Tap (11689) casts far more often under the
/// first condition.
#[test]
fn an_unsummoned_pets_mana_is_its_full_bar() {
    let pet_mana = |threshold: &str| {
        edited(
            "demonology-warlock-unsummoned-pet-mana",
            &[
                (
                    r#""currentManaPercent":{"sourceUnit":{"owner":{"type":"Self"},"type":"Pet"}}"#,
                    r#""currentMana":{"sourceUnit":{"owner":{"type":"Self"},"type":"Pet"}}"#,
                ),
                (r#""val":"20%""#, threshold),
            ],
        )
    };
    let below = run_request(&pet_mana(r#""val":"700""#));
    let above = run_request(&pet_mana(r#""val":"600""#));
    assert!(action(&below, 11689, 0).0 > 10.0 * action(&above, 11689, 0).0);
}

/// A prepull strict sequence whose step reads an aura: the step conditions are read with the
/// priority list's, and each step casts once a fight.
#[test]
fn a_prepull_sequence_step_may_read_an_aura() {
    let value = edited(
        "shadow-priest-prepull-sequence",
        &[(
            r#""condition":{"cmp":{"lhs":{"currentTime":{}},"op":"OpGt","rhs":{"const":{"val":"0s"}}}}"#,
            r#""condition":{"not":{"val":{"auraIsActive":{"auraId":{"spellId":15473}}}}}"#,
        )],
    );
    let player = run_request(&value);
    for spell in [20554, 14751, 1309636] {
        assert_eq!(action(&player, spell, 0).0, 50.0, "spell {spell}");
    }
}

/// Casts of the player's action with this item.
fn item_casts(player: &Value, item_id: i64) -> f64 {
    player["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|action| action["id"]["itemId"].as_i64() == Some(item_id))
        .flat_map(|action| action["targets"].as_array().unwrap())
        .map(|target| target["casts"].as_f64().unwrap_or(0.0))
        .sum()
}

/// The exported effects of a kind.
fn effects<'a>(prepared: &'a Value, kind: &str) -> Vec<&'a Value> {
    prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|effect| effect["kind"] == kind)
        .collect()
}

/// Darkmantle Armor (4): the rogue's landed melee hits roll for 20 energy (27787).
#[test]
fn darkmantle_armor_gains_energy() {
    let (value, player) = run("combat-rogue-darkmantle-armor");
    assert!(kinds(&value).contains(&"energy_proc"));
    assert!(resource_gain(&player, 27787, "ResourceTypeEnergy") > 0.0);
}

/// Riposte (14251) runs without a tank under Forever's rules, beside Darkmantle Armor.
#[test]
fn riposte_runs_without_a_tank() {
    let (value, _) = run("combat-rogue-darkmantle-riposte");
    assert!(kinds(&value).contains(&"riposte"));
}

/// Pummel (6554) in a Warrior's rotation casts.
#[test]
fn pummel_casts() {
    let (value, player) = run("dps-warrior-pummel");
    assert!(kinds(&value).contains(&"pummel"));
    assert!(action(&player, 6554, 0).0 > 0.0, "Pummel casts");
}

/// Battlegear of Heroism (4) restores health and rage on a melee hit.
#[test]
fn battlegear_of_heroism_procs() {
    let (value, _) = run("dps-warrior-battlegear-of-heroism");
    assert!(kinds(&value).contains(&"health_rage_proc"));
}

/// The Elements (5) puts up The Furious Storm off the shaman's spells.
#[test]
fn the_furious_storm_procs_for_any_spec() {
    let (value, _) = run("elemental-shaman-the-elements");
    assert!(effects(&value, "spell_data_stat_proc")
        .iter()
        .any(|effect| effect["aura"] == "The Furious Storm"));
}

/// A bonus against demons is inert against a target of another type.
#[test]
fn a_mob_type_bonus_against_another_type_is_inert() {
    run("demonology-warlock-mob-type-bonus");
}

/// Battle Squawk in a fight that outlasts it: its melee speed comes off when it expires.
#[test]
fn battle_squawk_expires() {
    let (value, _) = run("combat-rogue-battle-squawk");
    assert!(kinds(&value).contains(&"battle_squawk"));
}

/// The Warlock's assigned curse at all targets.
#[test]
fn the_assigned_curse_runs() {
    run("demonology-warlock-assigned-curse");
}

/// Swaps onto Thunderfury, onto gear that changes the crit the player takes, and onto the
/// Runeblade of Baron Rivendare, whose Unholy Aura (17625) restores health.
#[test]
fn retribution_swaps_onto_thunderfury_and_runeblade_run() {
    let (value, _) = run("retribution-paladin-swap-thunderfury");
    assert!(kinds(&value).contains(&"thunderfury"));
    run("retribution-paladin-swap-crit-taken");
    let (value, player) = run("retribution-paladin-swap-runeblade");
    assert!(kinds(&value).contains(&"health_regen_aura"));
    assert!(resource_gain(&player, 17625, "ResourceTypeHealth") > 0.0);
}

/// The Gorilla's Thunderstomp (1264455) strikes every target of a five-target fight.
#[test]
fn thunderstomp_strikes_several_targets() {
    let (value, _) = run("hunter-gorilla-thunderstomp");
    assert!(effects(&value, "hunter_pet_strike")
        .iter()
        .any(|effect| effect["spell_id"] == 1264455 && effect["cleave_targets"] == 4));
}

/// A prepull reference to a group whose one action moves to a variable's range.
#[test]
fn a_prepull_group_reference_moves() {
    run("hunter-prepull-group-move");
}

/// Fire Hardened Hauberk (6972) gains rage on use, once a fight, and Orb of Soran'ruk's heal
/// over time stays unused as in Go.
#[test]
fn on_use_rage_and_an_unused_heal_over_time_run() {
    let (value, player) = run("protection-warrior-fire-hardened-hauberk");
    assert!(effects(&value, "energize_on_use")
        .iter()
        .any(|effect| effect["power"] == "rage"));
    assert_eq!(item_casts(&player, 6972), 50.0);
    let (_, player) = run("demonology-warlock-orb-of-soranruk");
    assert_eq!(item_casts(&player, 6898), 0.0);
}

/// Sequence steps cast Curse of the Elements (1311680) at the next target.
#[test]
fn sequence_steps_cast_at_another_target() {
    let (_, player) = run("demonology-warlock-sequence-other-targets");
    let on_second = player["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|action| action["id"]["spellId"] == 1311680)
        .filter_map(|action| action["targets"].get(1))
        .map(|target| target["casts"].as_f64().unwrap_or(0.0))
        .sum::<f64>();
    assert!(on_second > 0.0);
}

/// The casts of the player's action with this spell on each target.
fn casts_by_target(player: &Value, spell_id: i64) -> Vec<f64> {
    let mut casts = Vec::new();
    for action in player["actions"].as_array().unwrap() {
        if action["id"]["spellId"] != spell_id {
            continue;
        }
        for (position, target) in action["targets"].as_array().unwrap().iter().enumerate() {
            if casts.len() <= position {
                casts.resize(position + 1, 0.0);
            }
            casts[position] += target["casts"].as_f64().unwrap_or(0.0);
        }
    }
    casts
}

/// A Balance Druid moves to the next target once Moonfire (9835) and Insect Swarm (24977) are up,
/// so both dots land on all five targets.
#[test]
fn a_change_of_target_in_the_priority_list_moves_the_dots() {
    let (_, player) = run("balance-druid-change-target");
    for spell in [9835, 24977] {
        let casts = casts_by_target(&player, spell);
        assert!(
            casts.len() >= 5 && casts[..5].iter().all(|&count| count > 0.0),
            "spell {spell}: {casts:?}"
        );
    }
}

/// A strict sequence changes target between its Curses of the Elements (1311680), then stays on
/// the last target it moved to, where Hellfire (11684) is cast.
#[test]
fn a_change_of_target_in_a_strict_sequence_moves_the_casts() {
    let (_, player) = run("demonology-warlock-sequence-change-target");
    let curses = casts_by_target(&player, 1311680);
    assert!(curses.len() >= 5 && curses[4] > 0.0, "{curses:?}");
    let hellfire = casts_by_target(&player, 11684);
    assert!(hellfire.len() >= 5 && hellfire[4] > 0.0, "{hellfire:?}");
}

/// Vampiric Embrace against three targets, cast on the next target with the priest's damage
/// there: the debuff lands on that target and heals the priest from the damage it takes.
#[test]
fn vampiric_embrace_on_another_target_heals_the_priest() {
    let (_, player) = run("shadow-priest-vampiric-embrace-three-targets");
    let casts = casts_by_target(&player, 15286);
    assert!(casts.len() >= 2 && casts[1] > 0.0, "{casts:?}");
    assert!(resource_gain(&player, 15286, "ResourceTypeHealth") > 0.0);
}

/// The raid's Judgement of Light beside a Succubus: the demon's melee hits roll the heal's chance
/// as the player's do, and the demon, always at full health, gains nothing from it.
#[test]
fn judgement_of_light_runs_beside_a_melee_demon() {
    let (value, _) = run("demonology-warlock-succubus-judgement-of-light");
    assert!(kinds(&value).contains(&"judgement_of_light"));
    assert!(
        value["pets"]
            .as_array()
            .is_some_and(|pets| !pets.is_empty()),
        "the Succubus is simulated"
    );
}

/// A Manual Crowd Pummeler opener that swaps onto Dark Edge of Insanity with Iron Counterweight
/// (enchant 34): the enchant's speed aura multiplies melee speed by 1.03 while the swap wears it.
#[test]
fn a_swap_onto_a_speed_enchant_runs() {
    let (value, _) = run("enhancement-shaman-swap-speed-enchant");
    let speed = effects(&value, "speed_aura");
    assert!(
        speed.iter().any(
            |effect| effect["aura"] == "Enchant 34 Speed (ItemSlotMainHand)"
                && effect["melee_multiplier"] == 1.03
        ),
        "{speed:?}"
    );
}

/// A tanking Bear's Manual Crowd Pummeler opener swaps onto Headspike (+7 Defense): the target's
/// rolls under the swap's stat aura carry its Defense and the crit the bear takes.
#[test]
fn a_tanks_swap_that_changes_defense_runs() {
    let (value, _) = run("feral-bear-druid-swap-defense");
    assert!(value["enemy"]["rolls"].as_array().unwrap().len() > 1);
}

/// Mark of the Champion against an Undead target: its 150 attack power against Undead and
/// Demons is the player's while the item is worn.
#[test]
fn mark_of_the_champion_against_undead_runs() {
    let (value, _) = run("dps-warrior-mark-of-the-champion-undead");
    let bonus = effects(&value, "mob_type_bonus");
    assert!(
        bonus
            .iter()
            .any(|effect| effect["aura"] == "Mark of the Champion (Physical)"
                && effect["ranged_attack_power"] == 150.0),
        "{bonus:?}"
    );
}

/// A Rogue moves to the next target once Rupture (11275) and Deadly Poison are up on the one it
/// is on: Rupture lands on all four targets, each with its own dot.
#[test]
fn a_rogue_keeps_rupture_on_several_targets() {
    let (_, player) = run("assassination-rogue-change-target-rupture");
    let ruptures = casts_by_target(&player, 11275);
    assert!(
        ruptures.len() >= 4 && ruptures[..4].iter().all(|&casts| casts > 0.0),
        "{ruptures:?}"
    );
    run("combat-rogue-change-target");
}

/// Chastise (1277335) in a Shadow Priest's rotation: Go casts it only at a Humanoid, which this
/// target is not, so it never casts.
#[test]
fn chastise_casts_only_at_a_humanoid() {
    let (_, player) = run("shadow-priest-chastise");
    assert_eq!(action(&player, 1277335, 0).0, 0.0);
}

/// Skysight (1259686), the Windshaper Skyborne racial, in a Hunter's rotation: the cast puts up
/// Elemental Blessing, which multiplies the movement speed while it lasts.
#[test]
fn skysight_casts_and_puts_up_its_blessing() {
    let (value, player) = run("hunter-skysight");
    assert!(effects(&value, "skysight")
        .iter()
        .any(|effect| effect["aura"] == "Elemental Blessing"));
    assert!(action(&player, 1259686, 0).0 > 0.0, "Skysight casts");
}

/// A Protection Paladin casts its own Devotion Aura (10293) while it is down: the cast turns the
/// aura on, once a fight, and its armor reaches the target's swings through the rolls.
#[test]
fn a_tanks_own_devotion_aura_casts() {
    let (value, player) = run("protection-paladin-devotion-aura");
    assert!(effects(&value, "paladin_aura_cast")
        .iter()
        .any(|effect| effect["ranks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rank| rank["spell_id"] == 10293)));
    assert_eq!(action(&player, 10293, 0).0, 50.0);
}

#[test]
fn hammer_of_the_righteous_cleaves() {
    let (value, player) = run("protection-paladin-hammer-of-the-righteous");
    let effect = &effects(&value, "hammer_of_the_righteous")[0];
    assert_eq!(effect["spell_id"], 407632);
    assert_eq!(effect["weapon_dps"], 3.0);
    assert_eq!(effect["max_targets"], 4);
    let (casts, damage, _) = action(&player, 407632, 0);
    // 980 since push 113554's tree (978 before): the fixture's talents read in it. 961 since the
    // raid's Battle Shout VII, Strength of Earth V and Grace of Air III are the ranks below the
    // Ahn'Qiraj 20 books, which Forever's rules leave out.
    assert_eq!(casts, 961.0);
    assert!(damage > 0.0);
}

#[test]
fn seal_of_wisdom_restores_mana_beside_the_raids_judgement() {
    let (value, player) = run("retribution-paladin-seal-of-wisdom");
    assert!(!effects(&value, "seal_of_wisdom").is_empty());
    assert!(resource_gain(&player, 20168, "ResourceTypeMana") > 0.0);
    // Go's JowManaMetrics: both judgements restore under the action of the first to restore.
    let own = resource_gain(&player, 20186, "ResourceTypeMana");
    let raid = resource_gain(&player, 20355, "ResourceTypeMana");
    assert!(own + raid > 0.0);
    assert!(own == 0.0 || raid == 0.0, "own {own} raid {raid}");
}

#[test]
fn seal_of_justice_judges_justice() {
    let (value, player) = run("retribution-paladin-seal-of-justice");
    assert!(!effects(&value, "seal_of_justice").is_empty());
    // 3298 since push 113554's tree (3983 before): the fixture's talents read in it.
    assert_eq!(action(&player, 20164, 0).0, 3298.0);
    assert_eq!(action(&player, 20184, 0).0, 754.0);
}

/// Shield Bash, Mocking Blow, Disarm, Intercept, Revenge off the target's threat and Freezing
/// Trap, each first in its rotation: the casts each makes in 50 fights.
#[test]
fn warrior_strikes_and_freezing_trap_run() {
    for (name, spell_id, casts) in [
        ("protection-warrior-shield-bash", 1672, 531.0),
        // 77 since the book ranks (#183) and two-hand crit rage (#184) changed the rage a fight has.
        ("arms-warrior-mocking-blow", 20560, 77.0),
        ("protection-warrior-disarm", 676, 129.0),
        ("warrior-intercept", 20617, 50.0),
        ("arms-warrior-revenge", 25288, 0.0),
        // Hotfix push 113554 puts Freezing Trap on Explosive Trap's cooldown (category 411).
        ("hunter-freezing-trap", 1499, 213.0),
    ] {
        let (_, player) = run(name);
        assert_eq!(action(&player, spell_id, 0).0, casts, "{name}");
    }
}

/// Thorns (9910): the cast turns on the druid's own shield, which strikes the target a spell
/// batch window after each swing a tank takes, for its fixed amount.
#[test]
fn a_druids_own_thorns_strikes_back() {
    let (value, player) = run("feral-bear-druid-thorns");
    assert_eq!(
        effects(&value, "druid_thorns")[0]["aura"],
        "Thorns (Player)"
    );
    let shield = effects(&value, "spell_data_damage_proc")
        .into_iter()
        .find(|effect| effect["trigger_aura"] == "Thorns (Player)")
        .expect("the tank's own Thorns strikes back");
    // core.NewDamageShield's fixed amount, which draws nothing.
    assert!(shield["roll"].is_null());
    // 991 with the raid's Battle Shout VII, Strength of Earth V and Grace of Air III at the
    // ranks below their Ahn'Qiraj 20 books (993 before).
    assert_eq!(action(&player, 9910, 0).0, 991.0);
    assert!(action(&player, 9910, 2).1 > 0.0);
    // Off the target's threat nothing hits the druid, so the shield never strikes.
    let (_, player) = run("balance-druid-thorns");
    // 1631 with the rotation's Starfire VII (25298, an Ahn'Qiraj 20 book) read as Starfire VI
    // (1634 before).
    assert_eq!(action(&player, 9910, 0).0, 1631.0);
    assert_eq!(action(&player, 9910, 2).1, 0.0);
}

/// A Shockadin's Manual Crowd Pummeler opener onto Blade of Hanna. Push 113554 removes One-Handed
/// Weapon Specialization, which the swap used to carry, so the fixture's talents read in its tree
/// carry no mod. With Two-Handed Weapon Specialization 3/3 the swap carries its damage and, since
/// push 113554, its -20% threat (effect 1367246): on under both two-handers.
#[test]
fn a_swap_carries_the_weapon_specializations() {
    let name = "shockadin-paladin-mcp-swap-one-hand-specialization";
    let spell_mods = |value: &Value| -> Vec<Value> {
        effects(value, "item_swap")[0]["callbacks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|callback| callback["kind"] == "spell_mod")
            .cloned()
            .collect()
    };
    let (forever, _) = run(name);
    assert!(spell_mods(&forever).is_empty());

    let mut request = request(name);
    // The fixture's talents in push 113554's tree, with Two-Handed Weapon Specialization 3/3.
    request["raid"]["parties"][0]["players"][0]["talentsString"] =
        Value::from("20123210002001-553220020100005-500000000003");
    let value = prepare_json_with(request.to_string().as_bytes(), "rust", Rules::Forever).unwrap();
    let prepared: PreparedV2 = serde_json::from_value(value.clone()).unwrap();
    check_prepared(&prepared).unwrap_or_else(|error| panic!("{name}: {error:?}"));
    let mods = spell_mods(&value);
    let kinds: Vec<(&str, f64)> = mods
        .iter()
        .map(|callback| {
            (
                callback["mod"]["kind"].as_str().unwrap(),
                callback["mod"]["value"].as_f64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        [("damage_done_pct", 0.06), ("threat_multiplier_pct", -0.2)],
        "{mods:?}"
    );
    for callback in &mods {
        assert_eq!(
            (&callback["main"], &callback["swapped"]),
            (&Value::Bool(true), &Value::Bool(true))
        );
    }
}

/// A Manual Crowd Pummeler opener that swaps back to Ironfoe (11684): Fury of Forgewright is off
/// while the Pummeler is in the hand and on once Ironfoe is.
#[test]
fn a_swap_back_to_ironfoe_runs() {
    let (value, _) = run("retribution-paladin-swap-ironfoe");
    let swap = effects(&value, "item_swap");
    let fury: Vec<&Value> = swap[0]["callbacks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|callback| callback["aura"] == "Fury of Forgewright")
        .collect();
    assert_eq!(fury.len(), 1, "{fury:?}");
    assert_eq!(
        (&fury[0]["main"], &fury[0]["swapped"]),
        (&json!("off"), &json!("on"))
    );
}

/// The uptime of the player's aura with this spell id, summed over its tags.
fn aura_uptime(player: &Value, spell_id: i64) -> f64 {
    player["auras"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|aura| aura["id"]["spellId"].as_i64() == Some(spell_id))
        .map(|aura| aura["uptimeSecondsAvg"].as_f64().unwrap_or(0.0))
        .sum()
}

/// Runs a prepared input through the gate.
fn run_prepared(value: &Value) -> Value {
    let prepared: PreparedV2 = serde_json::from_value(value.clone()).unwrap();
    check_prepared(&prepared).unwrap_or_else(|error| panic!("{error:?}"));
    simulate_prepared(&prepared).unwrap().result
}

/// Beaststalker Armor (5) on an Enhancement Shaman (production sim 672e9c16, 2 November 2026):
/// Forever does not lock Dungeon Set 1 to its class, and the five piece bonus's 5% chance on
/// landed melee and ranged autoattacks restores 200 mana (450577) to any wearer with a mana bar.
/// The Hunter domain exports it for a Hunter; the gate refused it on anyone else.
#[test]
fn beaststalker_armor_restores_a_shamans_mana() {
    let (value, player) = run("enhancement-shaman-beaststalker-armor");
    let procs = effects(&value, "mana_proc");
    assert_eq!(procs.len(), 1, "{procs:?}");
    assert_eq!(procs[0]["trigger_aura"], "Beaststalker Armor 5P");
    assert_eq!(
        (&procs[0]["mana"], &procs[0]["delay_ns"]),
        (&json!(200.0), &json!(10_000_000))
    );
    assert!(effects(&value, "hunter_set_mana_proc").is_empty());
    let gain = resource_gain(&player, 450577, "ResourceTypeMana");
    assert!(gain > 0.0, "{gain}");
    // The Go reference has no such effect, so its rules still refuse it.
    let reference: PreparedV2 = serde_json::from_value(prepared(
        "enhancement-shaman-beaststalker-armor",
        Rules::GoReference,
    ))
    .unwrap();
    assert!(check_prepared(&reference).is_err());
}

/// A Manual Crowd Pummeler opener that swaps onto the shaman's own weapon beside Badge of the
/// Swarmguard (production sims 472a5d95, 599bddd8 and 2a8fbade, 1 November 2026): the badge's
/// proc manager rolls a rate for the main hand's speed, which the swap rebuilds. With the
/// Pummeler's chances zeroed, the badge stacks in a five minute fight only through its second
/// use, after the swap, at the swapped weapon's chances.
#[test]
fn a_swap_rebuilds_the_swarmguards_chances() {
    let (value, _) = run("enhancement-shaman-swap-swarmguard");
    let badge = effects(&value, "stacking_cd_on_use")
        .into_iter()
        .position(|effect| effect["item_id"] == 21670)
        .unwrap();
    let mut value = edited(
        "enhancement-shaman-swap-swarmguard",
        &[(r#""duration":60"#, r#""duration":300"#)],
    );
    value = prepare_json_with(value.to_string().as_bytes(), "rust", Rules::Forever).unwrap();
    let index = value["effects"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .filter(|(_, effect)| effect["kind"] == "stacking_cd_on_use")
        .nth(badge)
        .unwrap()
        .0;
    let swapped = effects(&value, "item_swap")[0]["swapped"].clone();
    assert!(swapped
        .as_array()
        .unwrap()
        .iter()
        .any(|change| change["path"]
            .as_str()
            .unwrap()
            .starts_with(&format!("/effects/{index}/chances"))));
    let player = |result: &Value| result["raidMetrics"]["parties"][0]["players"][0].clone();
    let followed = player(&run_prepared(&value));
    for chance in value["effects"][index]["chances"].as_array_mut().unwrap() {
        chance["chance"] = json!(0.0);
    }
    let after_swap = player(&run_prepared(&value));
    let stacks = |player: &Value| aura_uptime(player, 26481);
    // The badge still stacks with the Pummeler's chances zeroed, which needs the swapped
    // weapon's chances. Zeroing the first use's chances moves the uptime by less than the noise
    // between the two runs, so the two uptimes are not compared.
    assert!(stacks(&followed) > 0.0, "{}", stacks(&followed));
    assert!(stacks(&after_swap) > 0.0);
}

/// A Retribution Paladin's Manual Crowd Pummeler opener that swaps onto Dark Iron Sunderer
/// (production sims 00ee2f2b and c468fd40, 1 November 2026): the Sunderer's Cleave Armor has no
/// chances while the Pummeler is in the hand, and the swap's proc manager gives it its own.
#[test]
fn a_swap_onto_dark_iron_sunderer_cleaves_armor() {
    let (value, _) = run("retribution-paladin-swap-dark-iron-sunderer");
    let procs = effects(&value, "armor_debuff_proc");
    let sunderer = procs
        .iter()
        .find(|effect| effect["trigger_aura"] == "Dark Iron Sunderer")
        .unwrap();
    assert_eq!(sunderer["chances"], json!([]));
    let result = run_prepared(&value);
    let target = &result["encounterMetrics"]["targets"][0];
    let cleaved: f64 = target["auras"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|aura| aura["id"]["spellId"] == 15280)
        .map(|aura| aura["uptimeSecondsAvg"].as_f64().unwrap_or(0.0))
        .sum();
    assert!(cleaved > 0.0, "{target}");
}

/// A Combat Rogue's strict sequence that casts Vanish, Adrenaline Rush and Blade Flurry at the
/// player (production sims 519a3701 and 15fb0511, 1 November 2026): they act on the rogue alone,
/// so the casts run as at the target. The recorded sequence opens with Stealth, which never casts
/// in combat, so the test's own sequence leaves it out.
#[test]
fn a_rogues_own_cooldowns_cast_at_the_player() {
    run("combat-rogue-self-casts-at-player");
    let mut value = request("combat-rogue-self-casts-at-player");
    let at_player = |spell: i64| json!({"castSpell": {"spellId": {"spellId": spell}, "target": {"type": "Player"}}});
    value["raid"]["parties"][0]["players"][0]["rotation"]["priorityList"][0] = json!({
        "action": {"strictSequence": {"actions": [at_player(1856), at_player(13750), at_player(13877)]}}
    });
    let player = run_request(&value);
    for spell in [1856, 13750, 13877] {
        assert!(action(&player, spell, 0).0 >= 50.0, "spell {spell}");
    }
}

/// The stat aura combination with only `label` up, and the one with no aura up.
fn alone_and_base<'a>(prepared: &'a Value, label: &str) -> (usize, &'a Value, &'a Value) {
    let stat_auras = effects(prepared, "stat_auras");
    assert_eq!(stat_auras.len(), 1);
    let auras = stat_auras[0]["auras"].as_array().unwrap();
    let bit = auras
        .iter()
        .position(|aura| aura == label)
        .unwrap_or_else(|| panic!("{label} is no stat aura: {auras:?}"));
    let combos = &stat_auras[0]["combos"];
    (bit, &combos[1 << bit], &combos[0])
}

/// Quel'Serrar on a Protection Paladin tank (production sims 0f6efd63, 338079ed, eadd22f1 and
/// 1524d32a, 1 November 2026): its Sanctuary (22850) grants 13 Defense, 300 Armor and 30 Attack
/// Power, and its Defense and Armor change the target's swings, which the gate refused. The buff
/// is now a stat aura, so the swing rolls are read with it up: 13 Defense moves the miss, dodge,
/// parry and block chances up and the crit chance down by 0.52% each, and the Armor lowers the
/// armor multiplier.
#[test]
fn quel_serrars_sanctuary_changes_the_targets_swings() {
    let (value, player) = run("protection-paladin-quel-serrar");
    let (bit, alone, base) = alone_and_base(&value, "Quel'Serrar Proc");
    let power = |combo: &Value| combo["AttackPower"].as_f64().unwrap();
    assert_eq!(power(alone) - power(base), 30.0);
    let rolls = value["enemy"]["rolls"].as_array().unwrap();
    let (up, down) = (&rolls[1 << bit], &rolls[0]);
    for (step, sign) in [
        ("miss_chance", 1.0),
        ("dodge_chance", 1.0),
        ("parry_chance", 1.0),
        ("block_chance", 1.0),
        ("crit_chance", -1.0),
    ] {
        let moved = up[step].as_f64().unwrap() - down[step].as_f64().unwrap();
        assert!((moved - sign * 0.0052).abs() < 1e-9, "{step} moved {moved}");
    }
    assert!(up["armor_multiplier"].as_f64().unwrap() < down["armor_multiplier"].as_f64().unwrap());
    assert!(aura_uptime(&player, 22850) > 0.0);
}

/// Destiny (647) on a Warrior who does not tank: the chance on hit's buff (17152, Strength 200)
/// was activated but read as no stat aura, so its Strength never reached the runtime. It is now
/// one, as Quel'Serrar's is, without the Defense, which only the target's swings at a tank read.
#[test]
fn destinys_strength_reaches_the_runtime() {
    let value = edited(
        "dps-warrior-mark-of-the-champion-undead",
        &[(r#""id":235480"#, r#""id":647"#)],
    );
    let prepared = prepare_json_with(value.to_string().as_bytes(), "rust", Rules::Forever).unwrap();
    let (_, alone, base) = alone_and_base(&prepared, "Destiny Proc");
    assert!(
        alone["AttackPower"].as_f64().unwrap() > base["AttackPower"].as_f64().unwrap(),
        "{alone} {base}"
    );
    let player = run_prepared(&prepared)["raidMetrics"]["parties"][0]["players"][0].clone();
    assert!(aura_uptime(&player, 17152) > 0.0);
}

/// The Black Book on a Demonology Warlock with the Imp out (production sim f0aad096, 1 November
/// 2026): its use had no exported effect. Under Forever's rules its Blessing (23720) doubles the
/// demon's damage done in every school for 30 sec, on a 5 minute cooldown (client 70707), so a
/// two minute fight casts it once and the Imp's Firebolt deals more than without the book.
#[test]
fn the_black_book_doubles_the_demons_damage() {
    let (value, player) = run("demonology-warlock-black-book");
    let book = effects(&value, "black_book");
    assert_eq!(book.len(), 1, "{book:?}");
    assert_eq!(book[0]["aura"], "Blessing of the Black Book");
    assert_eq!(book[0]["damage_multiplier"], json!(2.0));
    let uses: f64 = player["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|action| action["id"]["itemId"] == 19337)
        .flat_map(|action| action["targets"].as_array().unwrap())
        .map(|target| target["casts"].as_f64().unwrap_or(0.0))
        .sum();
    assert_eq!(uses, 50.0, "one use in each of the 50 iterations");
    let without = run_request(&edited(
        "demonology-warlock-black-book",
        &[(r#"{"id":19337}"#, "{}")],
    ));
    let firebolt = |player: &Value| {
        player["pets"][0]["actions"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|action| action["targets"].as_array().unwrap())
            .map(|target| target["damage"].as_f64().unwrap_or(0.0))
            .sum::<f64>()
    };
    assert!(
        firebolt(&player) > 1.15 * firebolt(&without),
        "{} {}",
        firebolt(&player),
        firebolt(&without)
    );
    // The reference's Classic Black Book has no such effect, so its rules still refuse it.
    let reference = request("demonology-warlock-black-book").to_string();
    let refused = prepare_json_with(reference.as_bytes(), "rust", Rules::GoReference).unwrap_err();
    assert!(
        format!("{refused:?}").contains("major cooldown item 19337 has no exported effect"),
        "{refused:?}"
    );
}

/// Quel'Serrar on a Warrior who does not tank and throws Goblin Sapper Charges, against
/// Dragonkin: the Sanctuary's 13 Defense lowers the crit the sapper's hit on its thrower takes by
/// 0.52%, which the stat aura combinations carry, and its 36 melee attack power against Dragonkin
/// (22850, aura 102 of mask 2) holds while it is up. Against Undead the bonus is not exported.
#[test]
fn quel_serrar_off_a_tank_against_dragonkin() {
    let quel_serrar = (r#""id":235480"#, r#""id":18348"#);
    let dragonkin = (
        r#""mobType":"MobTypeUndead""#,
        r#""mobType":"MobTypeDragonkin""#,
    );
    let value = edited(
        "dps-warrior-mark-of-the-champion-undead",
        &[quel_serrar, dragonkin],
    );
    let prepared = prepare_json_with(value.to_string().as_bytes(), "rust", Rules::Forever).unwrap();
    assert_eq!(effects(&prepared, "goblin_sapper").len(), 1);
    let (_, alone, base) = alone_and_base(&prepared, "Quel'Serrar Proc");
    let taken = |combo: &Value| combo["PseudoReducedCritTakenPercent"].as_f64().unwrap();
    assert!(
        (taken(alone) - taken(base) - 0.0052).abs() < 1e-12,
        "{alone} {base}"
    );
    let bonuses = effects(&prepared, "mob_type_bonus");
    let bonus = bonuses
        .iter()
        .find(|effect| effect["aura"] == "Quel'Serrar Proc")
        .unwrap_or_else(|| panic!("{bonuses:?}"));
    assert_eq!(bonus["melee_attack_power"], json!(36.0));
    assert_eq!(bonus["ranged_attack_power"], json!(0.0));
    run_prepared(&prepared);
    let undead = edited("dps-warrior-mark-of-the-champion-undead", &[quel_serrar]);
    let undead = prepare_json_with(undead.to_string().as_bytes(), "rust", Rules::Forever).unwrap();
    assert!(effects(&undead, "mob_type_bonus")
        .iter()
        .all(|effect| effect["aura"] != "Quel'Serrar Proc"));
}

/// An Orc Enhancement Shaman with Jom Gabbar (production sim ff415e0d, 1 November 2026): the
/// totems, the swap, the imbues and the potion take 10 stat aura bits, Blood Fury one and Jom
/// Gabbar's stacks four, 15 in all, past the reference's 14. Forever's rules combine up to 16.
#[test]
fn fifteen_stat_aura_bits_run() {
    let (value, _) = run("enhancement-shaman-fifteen-stat-aura-bits");
    let stat_auras = effects(&value, "stat_auras");
    let auras = stat_auras[0]["auras"].as_array().unwrap();
    for label in ["Blood Fury", "Jom Gabbar"] {
        assert!(auras.contains(&json!(label)), "{auras:?}");
    }
    assert_eq!(stat_auras[0]["combos"].as_array().unwrap().len(), 1 << 15);
    let reference = request("enhancement-shaman-fifteen-stat-aura-bits").to_string();
    let refused = prepare_json_with(reference.as_bytes(), "rust", Rules::GoReference).unwrap_err();
    assert!(
        format!("{refused:?}").contains("exceed the combination limit"),
        "{refused:?}"
    );
}
