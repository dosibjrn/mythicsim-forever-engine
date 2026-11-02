//! The healer talents past each class's heals, which Forever's rules model from the client rows
//! and tooltips (README, Rust-only mechanics): Riptide's bonus to Chain Heal, Genesis on Wild
//! Growth and Swiftmend, and the Holy Paladin's Oath of Light share. Each test puts a reference build's
//! talents on one of the healer fixtures, checks what the export carries and what the spell did
//! in the fights, and that the Go reference's rules do not read it.

use forever_engine::{check_prepared, prepare_json_with, simulate_prepared, Rules};
use serde_json::{json, Value};
use std::{fs, path::Path};

/// The Restoration shaman reference build's talents (Riptide, Water Shield, Healing Way).
const RESTORATION_SHAMAN: &str = "-055-5531500015513151";
/// The Restoration druid reference build's, in hotfix push 113531's order: Genesis 5.
const RESTORATION_DRUID: &str = "05302001--5053025153113051";
/// The same without Genesis.
const RESTORATION_DRUID_NO_GENESIS: &str = "00302001--5053025153113051";

const CHAIN_HEAL: i64 = 10623;
const RIPTIDE: i64 = 1239243;
// Holy Light VIII, the rank a Forever paladin casts: Holy Light IX (25292) is an Ahn'Qiraj 20
// book rank, which Forever's rules leave out.
const HOLY_LIGHT: i64 = 10329;
const WILD_GROWTH: i64 = 1238215;
const SWIFTMEND: i64 = 18562;

fn fixture(path: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// A healer fixture's request on these talents, with `spells` cast in order, each on the player.
fn request(name: &str, talents: &str, spells: &[i64]) -> Value {
    let mut request = fixture(&format!("fixtures/healers/{name}.request.json"));
    let player = &mut request["raid"]["parties"][0]["players"][0];
    player["talentsString"] = json!(talents);
    if !spells.is_empty() {
        let list: Vec<Value> = spells
            .iter()
            .map(|id| {
                json!({"action": {"castSpell": {
                    "spellId": {"spellId": id}, "target": {"type": "Self"}
                }}})
            })
            .collect();
        player["rotation"] = json!({"type": "TypeAPL", "priorityList": list});
    }
    request["simOptions"]["iterations"] = json!(40);
    request
}

fn prepared(request: &Value, rules: Rules) -> Value {
    prepare_json_with(request.to_string().as_bytes(), "rust", rules).unwrap()
}

fn effect<'a>(prepared: &'a Value, kind: &str) -> &'a Value {
    prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == kind)
        .unwrap_or_else(|| panic!("no {kind} effect"))
}

fn spell(prepared: &Value, spell_id: i64) -> &Value {
    prepared["player"]["spells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|spell| {
            spell["action_id"]["spell_id"] == spell_id && spell["action_id"]["tag"].is_null()
        })
        .unwrap_or_else(|| panic!("no spell {spell_id}"))
}

/// The prepared request passes the gate; the fights' result.
fn simulate(prepared: &Value) -> Value {
    let prepared = serde_json::from_value(prepared.clone()).unwrap();
    check_prepared(&prepared).unwrap();
    simulate_prepared(&prepared).unwrap().result
}

/// What a spell did in every fight together, summed over its targets.
fn sum(result: &Value, spell: i64, key: &str) -> f64 {
    result["raidMetrics"]["parties"][0]["players"][0]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|action| action["id"]["spellId"] == spell)
        .flat_map(|action| action["targets"].as_array().unwrap().iter())
        .map(|target| target[key].as_f64().unwrap_or(0.0))
        .sum()
}

/// What a spell healed a cast, over every fight.
fn healing_a_cast(prepared: &Value, spell: i64) -> f64 {
    let result = simulate(prepared);
    sum(&result, spell, "healing") / sum(&result, spell, "casts")
}

/// Riptide (1239243): a Chain Heal cast directly on its target heals 25% more (the talent's
/// tooltip, the Riptide rows' third effect). The first player of a Chain Heal is read as the
/// Riptide holder while one is up; its jumps keep their falloff of the unboosted heal.
#[test]
fn riptide_raises_the_first_chain_heal_player() {
    let with_riptide = prepared(
        &request(
            "restoration-shaman",
            RESTORATION_SHAMAN,
            &[RIPTIDE, CHAIN_HEAL],
        ),
        Rules::Forever,
    );
    let without = prepared(
        &request("restoration-shaman", RESTORATION_SHAMAN, &[CHAIN_HEAL]),
        Rules::Forever,
    );
    let heals = effect(&with_riptide, "heals")["heals"].as_array().unwrap();
    let chain = heals
        .iter()
        .find(|heal| heal["spell_id"] == CHAIN_HEAL)
        .unwrap();
    assert_eq!(chain["hot_bonus"]["multiplier"], 1.25);
    assert_eq!(chain["hot_bonus"]["first_target_only"], true);
    assert!(chain["hot_bonus"]["spell_ids"]
        .as_array()
        .unwrap()
        .contains(&json!(RIPTIDE)));
    // Every Chain Heal rank takes it and no other heal does; without the talent no Chain Heal
    // does.
    for heal in heals {
        let chain_rank = [1064, 10622, CHAIN_HEAL].contains(&heal["spell_id"].as_i64().unwrap());
        assert_eq!(heal.get("hot_bonus").is_some(), chain_rank, "{heal}");
    }
    let no_talent = prepared(
        &request("restoration-shaman", "-055-5531500015513150", &[CHAIN_HEAL]),
        Rules::Forever,
    );
    assert!(effect(&no_talent, "heals")["heals"]
        .as_array()
        .unwrap()
        .iter()
        .all(|heal| heal.get("hot_bonus").is_none()));
    // The reference's rules, with no party and no Chain Heal reach, state none.
    let reference = prepared(
        &request(
            "restoration-shaman",
            RESTORATION_SHAMAN,
            &[RIPTIDE, CHAIN_HEAL],
        ),
        Rules::GoReference,
    );
    assert!(effect(&reference, "heals")["heals"]
        .as_array()
        .unwrap()
        .iter()
        .all(|heal| heal.get("hot_bonus").is_none()));

    // The first player is a share of a cast's players (1, then half and a quarter of it when
    // the party's damage reaches them): 25% on it is about 15% on the cast.
    let ratio = healing_a_cast(&with_riptide, CHAIN_HEAL) / healing_a_cast(&without, CHAIN_HEAL);
    assert!((1.10..1.20).contains(&ratio), "Chain Heal ratio {ratio}");
}

/// Genesis (1223081): 2% a rank since push 113531, on the periodic healing of Rejuvenation,
/// Regrowth and Wild Growth (its first effect) and on Swiftmend (its second). The reference
/// reads it on Rejuvenation and Regrowth alone.
#[test]
fn genesis_reaches_wild_growth_and_swiftmend() {
    let spells = [WILD_GROWTH, SWIFTMEND];
    let with = prepared(
        &request("restoration-druid", RESTORATION_DRUID, &spells),
        Rules::Forever,
    );
    let without = prepared(
        &request("restoration-druid", RESTORATION_DRUID_NO_GENESIS, &spells),
        Rules::Forever,
    );
    let growth = |prepared: &Value| {
        spell(prepared, WILD_GROWTH)["dot"]["periodic_damage_multiplier"]
            .as_f64()
            .unwrap()
    };
    let swiftmend = |prepared: &Value| {
        spell(prepared, SWIFTMEND)["damage_multiplier_additive"]
            .as_f64()
            .unwrap()
    };
    // Five ranks of 2% a rank, on top of Gift of Nature's 10%.
    assert!((growth(&with) / growth(&without) - 1.1).abs() < 1e-9);
    assert!((swiftmend(&with) - swiftmend(&without) - 0.1).abs() < 1e-9);
    // Regrowth and Rejuvenation keep reading it.
    let rejuvenation = |prepared: &Value| {
        spell(prepared, 25299)["dot"]["periodic_damage_multiplier"]
            .as_f64()
            .unwrap()
    };
    assert!((rejuvenation(&with) / rejuvenation(&without) - 1.1).abs() < 1e-9);
    // Wild Growth heals more with it in the fights.
    assert!(healing_a_cast(&with, WILD_GROWTH) > healing_a_cast(&without, WILD_GROWTH));

    // Under the reference's rules the druid's reference string is read in its own order, and
    // Wild Growth and Swiftmend take no Genesis.
    let reference = prepared(
        &request("restoration-druid", "05002001--5050035153113051", &spells),
        Rules::GoReference,
    );
    let reference_without = prepared(
        &request("restoration-druid", "00002001--5050035153113051", &spells),
        Rules::GoReference,
    );
    assert_eq!(growth(&reference), growth(&reference_without));
    assert_eq!(swiftmend(&reference), swiftmend(&reference_without));
}

/// Oath of Light (1328548, push 113554), once cast on the heal target (the tank): while the aura
/// is up a Holy Light, Flash of Light or Holy Shock heal reaches the tank with the tank's share of
/// the party's hits, 5 in 9 of them, and only those carry the flat bonus; the other heals are
/// plain, and a Holy Shock heal on them has Boundless Light's crit back. The share is derived from
/// the party damage's hit rates. The Oath's cast, mana and bonuses are `tests/healers.rs`'s.
#[test]
fn oath_of_light_bonuses_the_tanks_share_of_heals() {
    let request = fixture("fixtures/healers/holy-paladin.request.json");
    let prepared = prepared(&request, Rules::Forever);
    let oath = effect(&prepared, "oath_of_light");
    // The tank is hit 1.0 a second and the other four 0.2 each.
    assert!((oath["holder_share"].as_f64().unwrap() - 1.0 / 1.8).abs() < 1e-12);
    assert_eq!(oath["holy_light"], 400.0);
    assert_eq!(oath["flash_of_light"], 115.0);
    assert_eq!(oath["holy_shock"], 229.0);
    assert_eq!(oath["boundless_light_crit"], 30.0);

    // A share of every Holy Light carries the 400: more a cast than without the Oath's cast,
    // and less than the full 400 on every heal would give.
    let swore = healing_a_cast(&prepared, HOLY_LIGHT);
    let mut bare_request = request.clone();
    let rotation = &mut bare_request["raid"]["parties"][0]["players"][0]["rotation"];
    rotation["prepullActions"] = json!([]);
    rotation["priorityList"]
        .as_array_mut()
        .unwrap()
        .retain(|line| line["action"]["castSpell"]["spellId"]["spellId"] != 1328548);
    let bare = healing_a_cast(&self::prepared(&bare_request, Rules::Forever), HOLY_LIGHT);
    let gain = swore / bare;
    assert!((1.08..1.18).contains(&gain), "Holy Light gain {gain}");
}

const MANA_TIDE_TOTEM: i64 = 1325256;
const HEALING_STREAM_TOTEM: i64 = 10463;
const LIGHTS_VIGIL_RANK_3: i64 = 1311595;
const LIGHTS_VIGIL_HEAL_RANK_3: i64 = 1311596;
const HOLY_SHOCK_HEAL_RANK_4: i64 = 25903;

/// A condition on an aura being up or down.
fn aura_is(spell: i64, up: bool) -> Value {
    let active = json!({"auraIsActive": {"auraId": {"spellId": spell}}});
    if up {
        active
    } else {
        json!({"not": {"val": active}})
    }
}

/// `request` with each cast given its own condition.
fn request_if(name: &str, talents: &str, casts: &[(i64, Option<Value>)]) -> Value {
    let mut request = request(name, talents, &[]);
    let list: Vec<Value> = casts
        .iter()
        .map(|(id, condition)| {
            let mut line = json!({"action": {"castSpell": {"spellId": {"spellId": id}}}});
            // A totem is cast with no target.
            if *id != MANA_TIDE_TOTEM {
                line["action"]["castSpell"]["target"] = json!({"type": "Self"});
            }
            if let Some(condition) = condition {
                line["action"]["condition"] = condition.clone();
            }
            line
        })
        .collect();
    request["raid"]["parties"][0]["players"][0]["rotation"] =
        json!({"type": "TypeAPL", "priorityList": list});
    request
}

/// The mana the regeneration row (mana per 5, spirit) gave in every fight together.
fn regeneration(result: &Value) -> f64 {
    result["raidMetrics"]["parties"][0]["players"][0]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|resource| resource["id"]["otherId"] == "OtherActionManaRegen")
        .map(|resource| resource["gain"].as_f64().unwrap())
        .sum()
}

/// The times, in seconds, a spell's aura is gained and faded in the first fight's log.
fn aura_times(result: &Value, spell: i64) -> (f64, f64) {
    let log = result["logs"].as_str().unwrap();
    let at = |what: &str| -> f64 {
        let marker = format!("{what}: {{SpellID: {spell}}}");
        let line = log
            .lines()
            .find(|line| line.contains(&marker))
            .unwrap_or_else(|| panic!("the first fight never logs {marker}"));
        line[1..line.find(']').unwrap()].parse().unwrap()
    };
    (at("Aura gained"), at("Aura faded"))
}

/// Mana Tide Totem (1325256, rank 4, the Restoration shaman's own cast under Forever's rules):
/// 60 mana, 13 sec, a 5 minute cooldown and the rank's Mana Tide row, 290 mana every 3 sec, which
/// the sim takes as MP5 as Mana Spring Totem's ticks are: 483.3 MP5 for 13 sec, 1,257 mana.
/// It takes the water totem slot: its cast ends a Healing Stream Totem, and a Healing Stream
/// Totem cast ends it.
#[test]
fn mana_tide_totem_restores_mana_and_takes_the_water_slot() {
    let with_totem = request_if(
        "restoration-shaman",
        RESTORATION_SHAMAN,
        &[(MANA_TIDE_TOTEM, None), (CHAIN_HEAL, None)],
    );
    let prepared_totem = prepared(&with_totem, Rules::Forever);
    let totem = effect(&prepared_totem, "mana_tide_totem");
    assert_eq!(totem["duration_ns"], 13_000_000_000_i64);
    assert!(totem["healing_stream_totems"]
        .as_array()
        .unwrap()
        .contains(&json!(HEALING_STREAM_TOTEM)));
    let cast = spell(&prepared_totem, MANA_TIDE_TOTEM);
    assert_eq!(cast["cost"]["base_cost"], 60);
    assert_eq!(cast["cd"]["duration_ns"], 300_000_000_000_i64);

    // Only a Restoration shaman who took the talent, under Forever's rules, has it.
    let no_talent = prepared(
        &request_if(
            "restoration-shaman",
            "-055-5531500015503151",
            &[(CHAIN_HEAL, None)],
        ),
        Rules::Forever,
    );
    assert!(no_talent["effects"]
        .as_array()
        .unwrap()
        .iter()
        .all(|effect| effect["kind"] != "mana_tide_totem"));
    let reference = prepared(&with_totem, Rules::GoReference);
    assert!(reference["effects"]
        .as_array()
        .unwrap()
        .iter()
        .all(|effect| effect["kind"] != "mana_tide_totem"));

    // One cast a fight (a 5 minute cooldown), the fight's mana regeneration up to 1,257 more: cast
    // at the pull, with the mana full, part of the first ticks has nowhere to go.
    let control = request_if(
        "restoration-shaman",
        RESTORATION_SHAMAN,
        &[(CHAIN_HEAL, None)],
    );
    let (tide, control) = (
        simulate(&prepared_totem),
        simulate(&prepared(&control, Rules::Forever)),
    );
    assert_eq!(sum(&tide, MANA_TIDE_TOTEM, "casts"), 40.0);
    let gained = (regeneration(&tide) - regeneration(&control)) / 40.0;
    assert!((900.0..1350.0).contains(&gained), "{gained} mana a fight");

    // A Healing Stream Totem recast the moment it is down ends the tide at once; a rotation that
    // lets it stand keeps it the 13 sec.
    let recast = request_if(
        "restoration-shaman",
        RESTORATION_SHAMAN,
        &[
            (MANA_TIDE_TOTEM, None),
            (
                HEALING_STREAM_TOTEM,
                Some(aura_is(HEALING_STREAM_TOTEM, false)),
            ),
        ],
    );
    let (gained, faded) = aura_times(
        &simulate(&prepared(&recast, Rules::Forever)),
        MANA_TIDE_TOTEM,
    );
    assert!(faded - gained < 3.0, "{} sec", faded - gained);
    let stands = request_if(
        "restoration-shaman",
        RESTORATION_SHAMAN,
        &[
            (MANA_TIDE_TOTEM, None),
            (
                HEALING_STREAM_TOTEM,
                Some(json!({"and": {"vals": [
                    aura_is(HEALING_STREAM_TOTEM, false), aura_is(MANA_TIDE_TOTEM, false)
                ]}})),
            ),
            (CHAIN_HEAL, None),
        ],
    );
    let (gained, faded) = aura_times(
        &simulate(&prepared(&stands, Rules::Forever)),
        MANA_TIDE_TOTEM,
    );
    assert!(
        (faded - gained - 13.0).abs() < 0.01,
        "{} sec",
        faded - gained
    );
    // And its cast ended the Healing Stream Totem's heals over time.
    let result = simulate(&prepared(&stands, Rules::Forever));
    assert!(sum(&result, HEALING_STREAM_TOTEM, "casts") >= 40.0);
}

/// Light's Vigil (1311595, rank 3, push 113554) in the Holy Paladin healer sim: the cast (1,340
/// mana, 1.5 sec) puts a vigil on the player for 30 sec; the next Holy Shock heal on them is the
/// vigil's, triggers no cooldown and heals the player and the four others of the party, each with
/// a roll and crit of its own (704 and 0.143 of healing power at level 60).
#[test]
fn lights_vigil_heals_the_party_on_the_next_holy_shock() {
    let mut vigil = fixture("fixtures/healers/holy-paladin.request.json");
    let rotation = &mut vigil["raid"]["parties"][0]["players"][0]["rotation"];
    let list = rotation["priorityList"].as_array_mut().unwrap();
    let at = list
        .iter()
        .position(|line| {
            line["action"]["castSpell"]["spellId"]["spellId"] == HOLY_SHOCK_HEAL_RANK_4
        })
        .unwrap();
    list.insert(
        at,
        json!({"action": {"castSpell": {
            "spellId": {"spellId": LIGHTS_VIGIL_RANK_3}, "target": {"type": "Self"}
        }, "condition": aura_is(1311597, false)}}),
    );
    vigil["simOptions"]["iterations"] = json!(40);
    let prepared_vigil = prepared(&vigil, Rules::Forever);
    let ranks = effect(&prepared_vigil, "lights_vigil_ally")["ranks"]
        .as_array()
        .unwrap();
    assert_eq!(ranks.len(), 3);
    let rank = ranks
        .iter()
        .find(|rank| rank["spell_id"] == LIGHTS_VIGIL_RANK_3)
        .unwrap();
    assert_eq!(rank["heal_spell_id"], LIGHTS_VIGIL_HEAL_RANK_3);
    assert_eq!(rank["players"], 5);
    assert!((rank["average"].as_f64().unwrap() - 704.0).abs() < 1.0);
    assert_eq!(
        spell(&prepared_vigil, LIGHTS_VIGIL_RANK_3)["cost"]["base_cost"],
        1340
    );

    let result = simulate(&prepared_vigil);
    let casts = sum(&result, LIGHTS_VIGIL_RANK_3, "casts");
    assert!(casts > 0.0);
    // Five heals a vigil spent, and a vigil is spent by the next Holy Shock after the cast.
    let heals = sum(&result, LIGHTS_VIGIL_HEAL_RANK_3, "hits")
        + sum(&result, LIGHTS_VIGIL_HEAL_RANK_3, "crits");
    assert!(
        heals > 0.0 && heals <= 5.0 * casts,
        "{heals} heals, {casts} casts"
    );
    assert_eq!(heals % 5.0, 0.0);
    assert!(sum(&result, LIGHTS_VIGIL_HEAL_RANK_3, "healing") > 0.0);

    // The Holy Shock a vigil spends has no cooldown: with only the vigil and Holy Shock to cast,
    // Holy Shocks follow each other by the vigil's 6 sec cooldown, not Holy Shock's 10.
    let gap = |casts: &[(i64, Option<Value>)]| {
        let mut request = vigil.clone();
        let player = &mut request["raid"]["parties"][0]["players"][0];
        player["rotation"]["priorityList"] = json!(casts
            .iter()
            .map(|(id, condition)| {
                let mut line = json!({"action": {"castSpell": {
                    "spellId": {"spellId": id}, "target": {"type": "Self"}
                }}});
                if let Some(condition) = condition {
                    line["action"]["condition"] = condition.clone();
                }
                line
            })
            .collect::<Vec<_>>());
        let result = simulate(&prepared(&request, Rules::Forever));
        let times: Vec<f64> = result["logs"]
            .as_str()
            .unwrap()
            .lines()
            .filter(|line| line.contains("Completed cast {SpellID: 25903}"))
            .map(|line| line[1..line.find(']').unwrap()].parse().unwrap())
            .collect();
        times[1] - times[0]
    };
    let with_vigil = gap(&[
        (LIGHTS_VIGIL_RANK_3, Some(aura_is(1311597, false))),
        (HOLY_SHOCK_HEAL_RANK_4, None),
    ]);
    let without = gap(&[(HOLY_SHOCK_HEAL_RANK_4, None)]);
    assert!(
        without >= 10.0 && with_vigil < 9.0,
        "{with_vigil} and {without} sec"
    );
}
