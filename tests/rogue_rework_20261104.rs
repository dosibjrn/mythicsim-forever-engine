//! The Rogue rework of the hotfix pushes 113351 and 113483 (client 1.60.1.70755, development notes
//! of 2 November 2026): the three trees in the client's own order, Shank's real ids, Composure,
//! Flash of Steel, Boot Knife, Venom on Shank, Blood Splatter, and the changed rank counts. Only
//! Forever's rules prepare them; the Go reference keeps the frozen data and its goldens.

use forever_engine::RngMode;
use forever_engine::{
    check_prepared, contracts::prepared_v2::PreparedV2, prepare_json_with, simulate_prepared,
    PrepareError, Rules,
};
use serde_json::{json, Value};
use std::{fs, path::Path};

/// Shank at level 44 (a level 60 rogue's), at level 24, and the provisional id #200 gave it.
const SHANK: i64 = 1_329_549;
const SHANK_RANK_ONE: i64 = 1_329_283;
const SHANK_PROVISIONAL: i64 = 1_399_001;
/// The aura Poised grants, Venom's buff, Boot Knife's combo points and Ghostly Strike's buff.
const POISED: i64 = 1_328_826;
const VENOM_BUFF: i64 = 1_310_703;
const BOOT_KNIFE: i64 = 1_329_285;
const GHOSTLY_READY: i64 = 1_329_326;
/// Death Mark at level 60.
const DEATH_MARK: i64 = 1_325_758;

/// Positions in each tree of the client's order (row, then column).
mod at {
    pub const ASSASSINATION_BLOOD_SPLATTER: usize = 7;
    pub const ASSASSINATION_LETHALITY: usize = 8;
    pub const ASSASSINATION_RELENTLESS_STRIKES: usize = 12;
    pub const ASSASSINATION_VENOM: usize = 13;
    pub const ASSASSINATION_SEAL_FATE: usize = 15;
    pub const ASSASSINATION_MUTILATE: usize = 16;
    pub const COMBAT_COMPOSURE: usize = 9;
    pub const COMBAT_DUAL_WIELD: usize = 14;
    pub const COMBAT_FLASH_OF_STEEL: usize = 17;
    pub const SUBTLETY_OPPORTUNITY: usize = 2;
    pub const SUBTLETY_GHOSTLY_STRIKE: usize = 11;
    pub const SUBTLETY_HEMORRHAGE: usize = 15;
    pub const SUBTLETY_BOOT_KNIFE: usize = 16;
    pub const SUBTLETY_DEATH_MARK: usize = 17;
}

const TREE_SIZES: [usize; 3] = [17, 19, 18];

/// A talent string in the new layout: `(tree, position, rank)` for each point spent.
fn talents(points: &[(usize, usize, u8)]) -> String {
    let mut trees: Vec<Vec<u8>> = TREE_SIZES.iter().map(|size| vec![0; *size]).collect();
    for (tree, position, rank) in points {
        trees[*tree][*position] = *rank;
    }
    trees
        .iter()
        .map(|tree| {
            let digits: String = tree.iter().map(|rank| rank.to_string()).collect();
            digits.trim_end_matches('0').to_string()
        })
        .collect::<Vec<_>>()
        .join("-")
}

fn fixture(name: &str) -> Value {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/mage/prepared-v2"
    ))
    .join(format!("{name}.request.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn player_mut(request: &mut Value) -> &mut Value {
    &mut request["raid"]["parties"][0]["players"][0]
}

fn with_talents(mut request: Value, talents: &str) -> Value {
    player_mut(&mut request)["talentsString"] = Value::from(talents);
    request
}

fn try_prepared(request: &Value, rules: Rules) -> Result<Value, PrepareError> {
    prepare_json_with(request.to_string().as_bytes(), "rust", rules)
}

fn prepared(request: &Value, rules: Rules) -> Value {
    try_prepared(request, rules).unwrap()
}

/// The reason a request is refused with; anything else (a panic included, which would be a
/// fault) fails the test.
fn refusal(request: &Value, rules: Rules) -> String {
    match try_prepared(request, rules) {
        Err(PrepareError::Refused(refusal)) => refusal.to_string(),
        Err(other) => panic!("not a refusal: {other}"),
        Ok(_) => panic!("prepared, not refused"),
    }
}

fn effects<'a>(prepared: &'a Value, kind: &str) -> Vec<&'a Value> {
    prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|effect| effect["kind"] == kind)
        .collect()
}

fn spells<'a>(prepared: &'a Value, class_spell: &str) -> Vec<&'a Value> {
    prepared["player"]["spells"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|spell| spell["class_spell"] == class_spell)
        .collect()
}

fn spell<'a>(prepared: &'a Value, class_spell: &str) -> &'a Value {
    let found = spells(prepared, class_spell);
    assert_eq!(found.len(), 1, "{class_spell}");
    found[0]
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn number(value: &Value) -> f64 {
    value.as_f64().unwrap()
}

/// The first fight's log of a request.
fn first_fight_log(request: &Value, rules: Rules) -> String {
    let mut value = prepared(request, rules);
    value["sim"]["iterations"] = json!(1);
    value["sim"]["debug_first_iteration"] = json!(true);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    check_prepared(&prepared).unwrap();
    let report = simulate_prepared(&prepared, RngMode::Stochastic).unwrap();
    report.result["logs"].as_str().unwrap().to_string()
}

fn time_of(line: &str) -> f64 {
    line[1..line.find(']').unwrap()].parse().unwrap()
}

/// Lines of a log that hold every one of the needles.
fn lines_with<'a>(log: &'a str, needles: &[&str]) -> Vec<&'a str> {
    log.lines()
        .filter(|line| needles.iter().all(|needle| line.contains(needle)))
        .collect()
}

/// Shank off the cooldown whenever Energy is below the threshold, ahead of the last cast line
/// (the builder), naming it by `id`.
fn with_shank(mut request: Value, id: i64, threshold: u32) -> Value {
    let list = player_mut(&mut request)["rotation"]["priorityList"]
        .as_array_mut()
        .unwrap();
    let builder = list
        .iter()
        .rposition(|item| item["action"].get("castSpell").is_some())
        .unwrap();
    list.insert(
        builder,
        json!({"action": {
            "castSpell": {"spellId": {"spellId": id}},
            "condition": {"cmp": {
                "lhs": {"currentEnergy": {}},
                "op": "OpLt",
                "rhs": {"const": {"val": threshold.to_string()}},
            }},
        }}),
    );
    request
}

/// A Stealth before the pull and a Garrote opener, the way the application's presets open.
fn with_stealth_opener(mut request: Value) -> Value {
    let rotation = &mut player_mut(&mut request)["rotation"];
    rotation["prepullActions"] = json!([{
        "action": {"castSpell": {"spellId": {"spellId": 1784}}},
        "doAtValue": {"const": {"val": "-1s"}},
    }]);
    let garrote = json!({"action": {
        "castSpell": {"spellId": {"spellId": 11290}},
        "condition": {"auraIsActive": {"auraId": {"spellId": 1784}}},
    }});
    rotation["priorityList"]
        .as_array_mut()
        .unwrap()
        .insert(0, garrote);
    request
}

/// Replaces a request's rotation with the given cast lines, each `(spell id, condition)`.
fn with_rotation(mut request: Value, lines: &[(i64, Option<Value>)]) -> Value {
    let list: Vec<Value> = lines
        .iter()
        .map(|(id, condition)| {
            let mut line = json!({"action": {"castSpell": {"spellId": {"spellId": id}}}});
            if let Some(condition) = condition {
                line["action"]["condition"] = condition.clone();
            }
            line
        })
        .collect();
    let rotation = &mut player_mut(&mut request)["rotation"];
    rotation["prepullActions"] = json!([]);
    rotation["priorityList"] = Value::Array(list);
    request
}

fn combat(points: &[(usize, usize, u8)]) -> Value {
    with_talents(fixture("production-combat-rogue"), &talents(points))
}

fn assassination(points: &[(usize, usize, u8)]) -> Value {
    with_talents(fixture("production-assassination-rogue"), &talents(points))
}

fn subtlety(points: &[(usize, usize, u8)]) -> Value {
    with_talents(fixture("production-subtlety-rogue"), &talents(points))
}

/// The three trees have the client's talent counts, and a string of exactly that many talents
/// reads.
#[test]
fn trees_have_the_clients_talent_counts() {
    let longest = TREE_SIZES.map(|size| "0".repeat(size));
    let request = with_talents(fixture("production-combat-rogue"), &longest.join("-"));
    assert!(try_prepared(&request, Rules::Forever).is_ok());
    for tree in 0..3 {
        let mut trees = longest.clone();
        trees[tree].push('0');
        let request = with_talents(request.clone(), &trees.join("-"));
        let reason = refusal(&request, Rules::Forever);
        assert!(
            reason.contains(&format!(
                "tree {} has {} talents",
                tree + 1,
                TREE_SIZES[tree]
            )),
            "{reason}"
        );
    }
}

/// A string written for the old trees that does not fit refuses with the talent and its ranks,
/// instead of panicking; the Go reference still reads it as it always has.
#[test]
fn old_layout_strings_that_do_not_fit_refuse_cleanly() {
    // Combat: Lethality has two ranks now, and the old string gives it five.
    let old = fixture("production-combat-rogue");
    let reason = refusal(&old, Rules::Forever);
    assert!(
        reason.contains("Lethality has 2 ranks, the string gives 5"),
        "{reason}"
    );
    assert!(try_prepared(&old, Rules::GoReference).is_ok());
    // Subtlety: Setup has three ranks and the old Camouflage gave five.
    let old = fixture("production-subtlety-rogue");
    let reason = refusal(&old, Rules::Forever);
    assert!(reason.contains("tree 3 has 18 talents"), "{reason}");
    assert!(try_prepared(&old, Rules::GoReference).is_ok());
    // None of the production strings panics under either rules.
    for name in [
        "production-combat-rogue",
        "production-assassination-rogue",
        "production-subtlety-rogue",
        "production-combat-rogue-3-targets",
        "production-subtlety-rogue-5-targets",
    ] {
        for rules in [Rules::Forever, Rules::GoReference] {
            let _ = try_prepared(&fixture(name), rules);
        }
    }
    // A string with a fourth tree, or a non-digit, never panics either.
    for text in ["0-0-0-0", "x-y-z", "5-5-5", ""] {
        let _ = try_prepared(&with_talents(old.clone(), text), Rules::Forever);
    }
}

/// Ranks and values of the talents whose counts changed, read through the new layout.
#[test]
fn changed_rank_counts_read_the_new_values() {
    // Lethality 10/20% crit damage bonus (Backstab's additive crit multiplier).
    for (rank, bonus) in [(1u8, 0.10), (2, 0.20)] {
        let request = assassination(&[(0, at::ASSASSINATION_LETHALITY, rank)]);
        let prepared = prepared(&request, Rules::Forever);
        let crit = number(&spell(&prepared, "backstab")["crit_multiplier_additive"]);
        assert!(close(crit, bonus), "Lethality {rank}: {crit}");
    }
    // Seal Fate 33/66/100%.
    for (rank, chance) in [(1u8, 0.33), (2, 0.66), (3, 1.0)] {
        let request = assassination(&[(0, at::ASSASSINATION_SEAL_FATE, rank)]);
        let prepared = prepared(&request, Rules::Forever);
        let proc = effects(&prepared, "rogue_proc")
            .into_iter()
            .find(|effect| effect["trigger_aura"] == "Seal Fate Trigger")
            .unwrap();
        assert!(
            close(number(&proc["proc_chance"]), chance),
            "Seal Fate {rank}"
        );
    }
    // Opportunity 4/8/12/16/20% on Backstab.
    for rank in 1..=5u8 {
        let request = subtlety(&[(2, at::SUBTLETY_OPPORTUNITY, rank)]);
        let prepared = prepared(&request, Rules::Forever);
        let additive = number(&spell(&prepared, "backstab")["damage_multiplier_additive"]);
        assert!(
            close(additive, 1.0 + 0.04 * f64::from(rank)),
            "Opportunity {rank}"
        );
    }
    // Dual Wield Specialization 25/50% on the off hand, which Shank strikes with.
    for (rank, bonus) in [(1u8, 0.25), (2, 0.5)] {
        let request = combat(&[(1, at::COMBAT_DUAL_WIELD, rank)]);
        let prepared = prepared(&request, Rules::Forever);
        let additive = number(&spell(&prepared, "shank")["damage_multiplier_additive"]);
        assert!(
            close(additive, 1.0 + bonus),
            "Dual Wield Specialization {rank}"
        );
    }
    // Relentless Strikes: 4% a Combo Point a rank for 25 Energy, a bool of 20% under the
    // reference.
    for rank in 1..=5u8 {
        let request = assassination(&[(0, at::ASSASSINATION_RELENTLESS_STRIKES, rank)]);
        let prepared = prepared(&request, Rules::Forever);
        let finisher = effects(&prepared, "rogue_finisher")[0];
        assert_eq!(finisher["relentless_strikes"], true);
        assert!(close(
            number(&finisher["relentless_strikes_chance_per_point"]),
            0.04 * f64::from(rank)
        ));
        assert_eq!(finisher["relentless_strikes_energy"], 25.0);
    }
    let none = prepared(&assassination(&[]), Rules::Forever);
    assert_eq!(
        effects(&none, "rogue_finisher")[0]["relentless_strikes"],
        false
    );
    let old = prepared(
        &fixture("production-assassination-rogue"),
        Rules::GoReference,
    );
    let finisher = effects(&old, "rogue_finisher")[0];
    assert_eq!(finisher["relentless_strikes"], true);
    assert!(close(
        number(&finisher["relentless_strikes_chance_per_point"]),
        0.2
    ));
}

/// Mutilate's flat damage by rank, 44, 63 and 88 (67 at the top under the reference), beside the
/// 90% weapon share the notes give.
#[test]
fn mutilate_adds_eighty_eight_at_the_top_rank() {
    let request = assassination(&[(0, at::ASSASSINATION_MUTILATE, 1)]);
    let forever = prepared(&request, Rules::Forever);
    let mutilate = effects(&forever, "mutilate")[0];
    assert_eq!(mutilate["flat_damage"], 88.0);
    assert!(close(number(&mutilate["weapon_share"]), 0.9));
    let reference = prepared(
        &fixture("production-assassination-rogue"),
        Rules::GoReference,
    );
    assert_eq!(effects(&reference, "mutilate")[0]["flat_damage"], 67.0);
}

/// Composure adds 5 or 10 to Poised's 10, Flash of Steel adds Energy to Shank and takes time off
/// its cooldown, and Boot Knife gives it a chance for two Combo Points.
#[test]
fn shank_and_poised_read_their_talents() {
    let plain = prepared(&combat(&[]), Rules::Forever);
    assert_eq!(effects(&plain, "poised")[0]["cost_reduction"], 10);
    assert_eq!(effects(&plain, "shank")[0]["energy_gain"], 25.0);
    assert_eq!(
        spell(&plain, "shank")["cd"]["duration_ns"],
        10_000_000_000i64
    );
    assert!(effects(&plain, "shank")[0]
        .get("boot_knife_chance")
        .is_none());
    for (rank, reduction) in [(1u8, 15), (2, 20)] {
        let prepared = prepared(&combat(&[(1, at::COMBAT_COMPOSURE, rank)]), Rules::Forever);
        assert_eq!(effects(&prepared, "poised")[0]["cost_reduction"], reduction);
    }
    for rank in 1..=5u8 {
        let prepared = prepared(
            &combat(&[(1, at::COMBAT_FLASH_OF_STEEL, rank)]),
            Rules::Forever,
        );
        let shank = effects(&prepared, "shank")[0];
        assert!(close(
            number(&shank["energy_gain"]),
            25.0 + 3.0 * f64::from(rank)
        ));
        // 0.4 sec a rank off 10 sec.
        let cooldown = spell(&prepared, "shank")["cd"]["duration_ns"]
            .as_i64()
            .unwrap();
        assert_eq!(cooldown, 10_000_000_000 - 400_000_000 * i64::from(rank));
    }
    for rank in 1..=5u8 {
        let prepared = prepared(
            &subtlety(&[(2, at::SUBTLETY_BOOT_KNIFE, rank)]),
            Rules::Forever,
        );
        let shank = effects(&prepared, "shank")[0];
        assert!(close(
            number(&shank["boot_knife_chance"]),
            0.2 * f64::from(rank)
        ));
        assert_eq!(shank["boot_knife_combo_points"], 2);
        assert_eq!(shank["boot_knife_action"]["spell_id"], BOOT_KNIFE);
    }
}

/// Shank's real ids: rank 2 is the spell, and rank 1 and the provisional id of #200 press it.
#[test]
fn shank_has_the_clients_ids_and_two_aliases() {
    let prepared = prepared(&combat(&[]), Rules::Forever);
    let shank = spell(&prepared, "shank");
    assert_eq!(shank["action_id"]["spell_id"], SHANK);
    let aliases: Vec<i64> = shank["rank_aliases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|alias| alias["spell_id"].as_i64().unwrap())
        .collect();
    assert_eq!(aliases, [SHANK_RANK_ONE, SHANK_PROVISIONAL]);
    // All three ids press it in a fight.
    for id in [SHANK, SHANK_RANK_ONE, SHANK_PROVISIONAL] {
        let request = with_shank(combat(&[]), id, 60);
        let log = first_fight_log(&request, Rules::Forever);
        assert!(
            !lines_with(&log, &[&format!("Casting {{SpellID: {SHANK}}}")]).is_empty(),
            "a rotation naming {id} casts Shank"
        );
    }
}

/// Venom is no finisher under Forever's rules: the aura (30% poison damage, 20% poison chance,
/// 6 sec) is Shank's. The reference keeps the finisher.
#[test]
fn venom_is_shanks_aura() {
    let request = assassination(&[(0, at::ASSASSINATION_VENOM, 1)]);
    let forever = prepared(&request, Rules::Forever);
    assert!(spells(&forever, "venom").is_empty());
    let venom = effects(&forever, "venom")[0];
    assert!(close(number(&venom["damage_bonus"]), 0.30));
    assert!(close(number(&venom["chance_bonus"]), 0.20));
    let durations = venom["durations_ns"].as_array().unwrap();
    assert!(durations.iter().all(|d| d == 6_000_000_000i64));
    // The reference's finisher is untouched: its old string (Venom is the 17th talent).
    let old = prepared(&request_with_old_venom(), Rules::GoReference);
    assert_eq!(spells(&old, "venom").len(), 1);
    assert_eq!(
        effects(&old, "venom")[0]["durations_ns"][5],
        21_000_000_000i64
    );
}

/// The production Assassination request with Venom taken in the old tree, where it is the 17th
/// talent: the fixture's string gives 16.
fn request_with_old_venom() -> Value {
    with_talents(
        fixture("production-assassination-rogue"),
        "00530310551101051-30230320201401",
    )
}

/// Blood Splatter lengthens Garrote by 9 sec (three ticks) and Rupture by 6 sec (three ticks).
#[test]
fn blood_splatter_adds_ticks() {
    let without = prepared(&assassination(&[]), Rules::Forever);
    let with = prepared(
        &assassination(&[(0, at::ASSASSINATION_BLOOD_SPLATTER, 1)]),
        Rules::Forever,
    );
    let ticks = |prepared: &Value| {
        spell(prepared, "garrote")["dot"]["base_tick_count"]
            .as_i64()
            .unwrap()
    };
    assert_eq!(ticks(&with), ticks(&without) + 3);
    let rupture = |prepared: &Value| {
        effects(prepared, "rupture")[0]["base_tick_count"]
            .as_i64()
            .unwrap()
    };
    assert_eq!(rupture(&with), rupture(&without) + 3);
    // Not under the reference, whatever the string.
    let reference = prepared(
        &fixture("production-assassination-rogue"),
        Rules::GoReference,
    );
    assert_eq!(
        spell(&reference, "garrote")["dot"]["base_tick_count"]
            .as_i64()
            .unwrap(),
        ticks(&without)
    );
}

/// The removed talents register nothing: no Quietus, Cutthroat or Thousand Cuts effects under
/// Forever's rules, while the reference's old string still has them.
#[test]
fn removed_talents_are_gone_under_forever_rules() {
    let all = subtlety(&[
        (2, at::SUBTLETY_OPPORTUNITY, 5),
        (2, at::SUBTLETY_GHOSTLY_STRIKE, 1),
        (2, at::SUBTLETY_BOOT_KNIFE, 5),
        (2, 17, 1),
    ]);
    let forever = prepared(&all, Rules::Forever);
    for kind in ["quietus", "thousand_cuts"] {
        assert!(effects(&forever, kind).is_empty(), "{kind}");
    }
}

/// Death Mark: the spell of rank 4, pressed by the lower ranks, for 25 Energy on a 10 sec
/// cooldown, with the tooltip's numbers; not under the reference, and not without the talent.
#[test]
fn death_mark_is_prepared_from_its_talent() {
    let request = subtlety(&[(2, at::SUBTLETY_DEATH_MARK, 1)]);
    let forever = prepared(&request, Rules::Forever);
    let mark = spell(&forever, "death_mark");
    assert_eq!(mark["action_id"]["spell_id"], DEATH_MARK);
    assert_eq!(mark["cost"]["base_cost"], 25);
    assert_eq!(mark["cd"]["duration_ns"], 10_000_000_000i64);
    assert_eq!(mark["max_range"], 20.0);
    let aliases: Vec<i64> = mark["rank_aliases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|alias| alias["spell_id"].as_i64().unwrap())
        .collect();
    assert_eq!(aliases, [1_316_709, 1_325_756, 1_325_757]);
    let effect = effects(&forever, "death_mark")[0];
    assert!(close(number(&effect["stored_per_point"]), 0.12));
    assert_eq!(effect["cap_base"], 156.0);
    assert_eq!(effect["cap_per_point"], 245.0);
    assert!(close(number(&effect["attack_power_per_point"]), 0.04));
    assert_eq!(effect["coup_de_grace"], 1.0);
    let classes: Vec<&str> = effect["doubled_class_spells"]
        .as_array()
        .unwrap()
        .iter()
        .map(|class| class.as_str().unwrap())
        .collect();
    assert_eq!(classes, ["garrote", "rupture"]);
    // Coup de Grace (the first Combat talent) multiplies the share and the cap.
    let request = with_talents(
        request,
        &talents(&[(1, 0, 3), (2, at::SUBTLETY_DEATH_MARK, 1)]),
    );
    let forever = prepared(&request, Rules::Forever);
    assert!(close(
        number(&effects(&forever, "death_mark")[0]["coup_de_grace"]),
        1.2
    ));
    let reference = prepared(&fixture("production-subtlety-rogue"), Rules::GoReference);
    assert!(spells(&reference, "death_mark").is_empty());
    assert!(spells(&prepared(&subtlety(&[]), Rules::Forever), "death_mark").is_empty());
}

/// A Death Mark at five Combo Points marks the target for 6 sec and then deals a share of what the
/// rogue dealt it, which is part of the rogue's damage and never more than the cap.
#[test]
fn death_mark_releases_stored_damage_after_six_seconds() {
    let combo_points = |n: u32| {
        Some(json!({"cmp": {
            "lhs": {"currentComboPoints": {}},
            "op": "OpGe",
            "rhs": {"const": {"val": n.to_string()}},
        }}))
    };
    let request = with_rotation(
        subtlety(&[
            (2, at::SUBTLETY_DEATH_MARK, 1),
            (2, at::SUBTLETY_HEMORRHAGE, 1),
        ]),
        &[
            (DEATH_MARK, combo_points(5)),
            (11275, combo_points(5)),
            (16511, None),
        ],
    );
    let log = first_fight_log(&request, Rules::Forever);
    // A finisher's casts are tagged with the Combo Points spent.
    let casts = lines_with(&log, &[&format!("Casting {{SpellID: {DEATH_MARK}")]);
    let faded = lines_with(&log, &[&format!("Aura faded: {{SpellID: {DEATH_MARK}}}")]);
    assert!(casts.len() >= 3, "{} casts", casts.len());
    for (cast, fade) in casts.iter().zip(&faded) {
        let held = time_of(fade) - time_of(cast);
        assert!((held - 6.0).abs() < 0.01, "marked for {held} sec");
    }
    let value = {
        let mut value = prepared(&request, Rules::Forever);
        value["sim"]["iterations"] = json!(1);
        value
    };
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    check_prepared(&prepared).unwrap();
    let result = simulate_prepared(&prepared, RngMode::Stochastic).unwrap().result;
    let player = &result["raidMetrics"]["parties"][0]["players"][0];
    let damage_of = |spell_id: Option<i64>| -> f64 {
        player["actions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|action| spell_id.is_none() || action["id"]["spellId"].as_i64() == spell_id)
            .flat_map(|action| action["targets"].as_array().unwrap())
            .map(|target| target["damage"].as_f64().unwrap_or(0.0))
            .sum()
    };
    let release = damage_of(Some(DEATH_MARK));
    assert!(release > 0.0, "no release damage");
    assert!(
        release < damage_of(None) * 0.5,
        "{release} of {}",
        damage_of(None)
    );
}

/// Ghostly Strike's damage is normalized, and its buff lets the next Ambush or Garrote out of
/// Stealth (the aura Cutthroat gave Ambush).
#[test]
fn ghostly_strike_is_normalized_and_readies_ambush_and_garrote() {
    let request = subtlety(&[(2, at::SUBTLETY_GHOSTLY_STRIKE, 1)]);
    let forever = prepared(&request, Rules::Forever);
    let ghostly = effects(&forever, "ghostly_strike")[0];
    assert_eq!(ghostly["normalized"], true);
    assert_eq!(ghostly["ready_aura"], "Ghostly Strike Ready");
    assert_eq!(
        effects(&forever, "ambush")[0]["cutthroat_aura"],
        "Ghostly Strike Ready"
    );
    assert_eq!(
        effects(&forever, "garrote")[0]["ghostly_aura"],
        "Ghostly Strike Ready"
    );
    // The old tree's ninth talent is Ghostly Strike.
    let old = with_talents(fixture("production-subtlety-rogue"), "--000000001");
    let old = prepared(&old, Rules::GoReference);
    let ghostly = effects(&old, "ghostly_strike")[0];
    assert!(ghostly.get("normalized").is_none());
    assert!(ghostly.get("ready_aura").is_none());
    assert!(effects(&old, "garrote")[0].get("ghostly_aura").is_none());
}

/// Shank every 10 sec less Flash of Steel's 2 sec, for 5 Energy and 25 plus its 15.
#[test]
fn shank_cadence_and_energy_with_flash_of_steel() {
    for (rank, cooldown, gain) in [(0u8, 10.0, 25.0), (5, 8.0, 40.0)] {
        let points: Vec<(usize, usize, u8)> = if rank == 0 {
            Vec::new()
        } else {
            vec![(1, at::COMBAT_FLASH_OF_STEEL, rank)]
        };
        let request = with_shank(combat(&points), SHANK, 60);
        let log = first_fight_log(&request, Rules::Forever);
        let casts = lines_with(&log, &[&format!("Casting {{SpellID: {SHANK}}}")]);
        assert!(casts.len() >= 4, "{} casts at rank {rank}", casts.len());
        assert!(casts
            .iter()
            .all(|line| line.contains("Cost = 5.000") || line.contains("Cost = 0.000")));
        for pair in casts.windows(2) {
            let gap = time_of(pair[1]) - time_of(pair[0]);
            assert!(gap >= cooldown - 1e-9, "rank {rank}: {gap} sec apart");
        }
        // A rotation that casts it off the cooldown keeps to that spacing within a swing or so.
        let gains = lines_with(
            &log,
            &[&format!(
                "Gained {gain:.3} energy from {{SpellID: {SHANK}}}"
            )],
        );
        assert!(gains.len() >= 4, "rank {rank}: {} gains", gains.len());
    }
}

/// Boot Knife at 100% adds two Combo Points on every Shank, with its own metrics; none without.
#[test]
fn boot_knife_adds_two_combo_points() {
    let with = with_shank(subtlety(&[(2, at::SUBTLETY_BOOT_KNIFE, 5)]), SHANK, 60);
    let log = first_fight_log(&with, Rules::Forever);
    let shanks = lines_with(&log, &[&format!("Casting {{SpellID: {SHANK}}}")]).len();
    let points = lines_with(
        &log,
        &[&format!(
            "Gained 2 combo points from {{SpellID: {BOOT_KNIFE}}}"
        )],
    )
    .len();
    assert!(shanks >= 4);
    assert_eq!(
        points, shanks,
        "two Combo Points on each of {shanks} Shanks"
    );
    let without = with_shank(subtlety(&[]), SHANK, 60);
    let log = first_fight_log(&without, Rules::Forever);
    assert!(lines_with(&log, &[&BOOT_KNIFE.to_string()]).is_empty());
}

/// Venom's aura goes up with each Shank and falls six seconds later, and the poisons hit
/// harder while it holds (a Shank every 10 sec is up 60% of the time at best).
#[test]
fn venom_follows_shank_for_six_seconds() {
    // The Combat production rotation (Backstab as the builder) with the Venom talent.
    let venom = with_talents(
        fixture("production-combat-rogue"),
        &talents(&[(0, at::ASSASSINATION_VENOM, 1)]),
    );
    let request = with_shank(venom, SHANK, 60);
    let log = first_fight_log(&request, Rules::Forever);
    let gained = lines_with(&log, &[&format!("Aura gained: {{SpellID: {VENOM_BUFF}}}")]);
    let faded = lines_with(&log, &[&format!("Aura faded: {{SpellID: {VENOM_BUFF}}}")]);
    assert!(gained.len() >= 4, "{} gains", gained.len());
    // Each holds six seconds, but the last one the fight cuts short.
    for (gain, fade) in gained.iter().zip(&faded).take(gained.len() - 1) {
        let held = time_of(fade) - time_of(gain);
        assert!((held - 6.0).abs() < 1e-6, "Venom held {held} sec");
    }
    // Every Shank restarts it.
    let shanks = lines_with(&log, &[&format!("Casting {{SpellID: {SHANK}}}")]).len();
    assert!(gained.len() + 1 >= shanks && gained.len() <= shanks);
    // No Venom without the talent.
    let plain = with_shank(combat(&[]), SHANK, 60);
    let log = first_fight_log(&plain, Rules::Forever);
    assert!(lines_with(&log, &[&VENOM_BUFF.to_string()]).is_empty());
}

/// Poised takes 10 off the opener, and Composure's 5 or 10 more: Garrote from Stealth costs 40,
/// 35 or 30 (50 under the reference).
#[test]
fn poised_with_composure_discounts_the_opener() {
    for (rank, cost) in [(0u8, "40.000"), (1, "35.000"), (2, "30.000")] {
        let points: Vec<(usize, usize, u8)> = if rank == 0 {
            Vec::new()
        } else {
            vec![(1, at::COMBAT_COMPOSURE, rank)]
        };
        let request = with_stealth_opener(combat(&points));
        let log = first_fight_log(&request, Rules::Forever);
        let garrote = log
            .lines()
            .find(|line| line.contains("Casting {SpellID: 11290}"))
            .unwrap();
        assert!(
            garrote.contains(&format!("Cost = {cost}")),
            "rank {rank}: {garrote}"
        );
        assert!(log.contains(&format!("Aura gained: {{SpellID: {POISED}}}")));
        assert!(log.contains(&format!("Aura faded: {{SpellID: {POISED}}}")));
    }
    let request = with_stealth_opener(fixture("production-combat-rogue"));
    let log = first_fight_log(&request, Rules::GoReference);
    let garrote = log
        .lines()
        .find(|line| line.contains("Casting {SpellID: 11290}"))
        .unwrap();
    assert!(garrote.contains("Cost = 50.000"));
}

/// Ghostly Strike's buff lets Ambush out of Stealth once: after the Ghostly Strike, the rotation's
/// Ambush casts without Stealth, and only once per Ghostly Strike.
#[test]
fn ghostly_strike_readies_one_ambush_without_stealth() {
    // No Stealth: Ambush first, then Ghostly Strike, then Hemorrhage for the Energy.
    let request = with_rotation(
        subtlety(&[
            (2, at::SUBTLETY_GHOSTLY_STRIKE, 1),
            (2, at::SUBTLETY_HEMORRHAGE, 1),
        ]),
        &[(11269, None), (14278, None), (16511, None)],
    );
    let log = first_fight_log(&request, Rules::Forever);
    let ghostly = lines_with(&log, &["Casting {SpellID: 14278}"]);
    let ambush = lines_with(&log, &["Casting {SpellID: 11269}"]);
    assert!(!ghostly.is_empty());
    assert!(!ambush.is_empty(), "Ambush casts after a Ghostly Strike");
    assert!(
        ambush.len() <= ghostly.len(),
        "{} Ambush for {} Ghostly Strikes",
        ambush.len(),
        ghostly.len()
    );
    assert!(time_of(ambush[0]) >= time_of(ghostly[0]));
    assert!(log.contains(&format!("Aura gained: {{SpellID: {GHOSTLY_READY}}}")));
    assert!(log.contains(&format!("Aura faded: {{SpellID: {GHOSTLY_READY}}}")));
}
