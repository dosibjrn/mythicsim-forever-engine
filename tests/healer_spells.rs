//! The healer spells MythicSim's healer sims cast past each class's heals (README, Rust-only
//! mechanics): Penance's heal for a Discipline priest, Tranquility and Nature's Swiftness for a
//! Restoration druid, and Nature's Swiftness and Water Shield for a Restoration shaman. Each test
//! puts one of `fixtures/healers`' requests on a rotation that casts the spell, then checks the
//! export, that the request passes the gate, and what the spell did in the fights.

use forever_engine::{check_prepared, prepare_json_with, simulate_prepared, Rules};
use serde_json::{json, Value};
use std::{fs, path::Path};

/// A healer fixture's request on these talents, casting `spells` (client spell IDs) in order,
/// each on the player but the self buffs.
fn request(name: &str, talents: &str, spells: &[(i64, Option<Value>)]) -> Value {
    let path = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/healers"))
        .join(format!("{name}.request.json"));
    let mut request: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let player = &mut request["raid"]["parties"][0]["players"][0];
    player["talentsString"] = json!(talents);
    let list: Vec<Value> = spells
        .iter()
        .map(|(id, condition)| {
            let mut cast = json!({"spellId": {"spellId": id}});
            if !SELF_BUFFS.contains(id) {
                cast["target"] = json!({"type": "Self"});
            }
            let mut action = json!({"castSpell": cast});
            if let Some(condition) = condition {
                action["condition"] = condition.clone();
            }
            json!({ "action": action })
        })
        .collect();
    player["rotation"] = json!({"type": "TypeAPL", "priorityList": list});
    request["simOptions"]["iterations"] = json!(20);
    request
}

/// Nature's Swiftness (Druid, Shaman) and Water Shield: cast with no target.
const SELF_BUFFS: &[i64] = &[16188, 17116, 408510];

const DISCIPLINE: &str = "005205031332011531-03505003";
const RESTORATION_DRUID: &str = "05002001--5050035153113051";
const RESTORATION_SHAMAN: &str = "-055-5531500015513151";

fn active(spell: i64) -> Option<Value> {
    Some(json!({"auraIsActive": {"auraId": {"spellId": spell}}}))
}

fn inactive(spell: i64) -> Option<Value> {
    Some(json!({"not": {"val": {"auraIsActive": {"auraId": {"spellId": spell}}}}}))
}

fn prepared(request: &Value) -> Value {
    prepare_json_with(request.to_string().as_bytes(), "rust", Rules::Forever).unwrap()
}

fn effects<'a>(prepared: &'a Value, kind: &str) -> Vec<&'a Value> {
    prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|effect| effect["kind"] == kind)
        .collect()
}

fn heal(prepared: &Value, spell_id: i64) -> &Value {
    effects(prepared, "heals")[0]["heals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|heal| heal["spell_id"] == spell_id)
        .unwrap_or_else(|| panic!("no heal for spell {spell_id}"))
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

/// The first fight's log line after the first cast of `spell`.
fn cast_after(result: &Value, spell: i64) -> String {
    let log = result["logs"].as_str().unwrap();
    let marker = format!("Completed cast {{SpellID: {spell}}}");
    let (_, rest) = log
        .split_once(&marker)
        .unwrap_or_else(|| panic!("the first fight never casts {spell}"));
    rest.lines()
        .find(|line| line.contains("Casting"))
        .unwrap()
        .to_string()
}

/// A Discipline priest's Penance (1316995) heals: a bolt at once and one a second for two
/// more, each of the bolt row's healing, and no damage spell under it.
#[test]
fn penance_heals_for_a_discipline_priest() {
    let request = request("holy-priest", DISCIPLINE, &[(1316995, None), (10917, None)]);
    let prepared = prepared(&request);
    let penance = heal(&prepared, 1316995);
    assert!(penance["direct"]["average"].as_f64().unwrap() > 0.0);
    assert_eq!(penance["direct"]["average"], penance["tick"]);
    assert!(effects(&prepared, "penance").is_empty());
    let result = simulate(&prepared);
    assert!(sum(&result, 1316995, "casts") > 0.0);
    let ticks = sum(&result, 1316995, "ticks") + sum(&result, 1316995, "critTicks");
    let hits = sum(&result, 1316995, "hits") + sum(&result, 1316995, "crits");
    // Two ticks a cast, less those of a channel the fight's end cuts.
    assert!(
        ticks <= 2.0 * hits && ticks >= 2.0 * (hits - 20.0),
        "{ticks} ticks, {hits} casts"
    );
    assert!(sum(&result, 1316995, "healing") > 0.0);
}

/// Tranquility (9863) heals the party of five each 2 sec over its 10 sec channel, which a
/// 5 minute cooldown follows.
#[test]
fn tranquility_heals_the_party_over_its_channel() {
    let request = request(
        "restoration-druid",
        RESTORATION_DRUID,
        &[(9863, None), (25297, None)],
    );
    let prepared = prepared(&request);
    let tranquility = heal(&prepared, 9863);
    assert_eq!(tranquility["targets"], 5);
    assert_eq!(tranquility["tick"], 285.0);
    assert!(tranquility.get("direct").is_none());
    let result = simulate(&prepared);
    // One channel a fight: 3 minutes hold one cooldown.
    assert_eq!(sum(&result, 9863, "casts"), 20.0);
    let ticks = sum(&result, 9863, "ticks") + sum(&result, 9863, "critTicks");
    assert_eq!(ticks, 20.0 * 5.0 * 5.0, "five ticks on five players");
    assert!(sum(&result, 9863, "healing") > 0.0);
}

/// Nature's Swiftness makes the next heal with a cast time instant, and that cast spends it:
/// the Druid's (17116) on Healing Touch, the Shaman's (16188) on Healing Wave.
#[test]
fn natures_swiftness_makes_the_next_heal_instant() {
    for (name, talents, swiftness, heal) in [
        // Healing Touch X (9889) and Healing Wave IX (10396): the ranks below the Ahn'Qiraj 20
        // books (25297, 25357) that a Forever druid and shaman cast.
        ("restoration-druid", RESTORATION_DRUID, 17116, 9889),
        ("restoration-shaman", RESTORATION_SHAMAN, 16188, 10396),
    ] {
        let request = request(
            name,
            talents,
            &[(heal, active(swiftness)), (swiftness, None), (heal, None)],
        );
        let prepared = prepared(&request);
        let aura = effects(&prepared, "instant_cast_aura");
        assert_eq!(aura.len(), 1, "{name}");
        assert_eq!(aura[0]["spell_id"], swiftness);
        let result = simulate(&prepared);
        let next = cast_after(&result, swiftness);
        assert!(
            next.contains(&format!("{{SpellID: {heal}}}")) && next.contains("Cast Time = 0s"),
            "{name}: {next}"
        );
        // Spent: the cast after it takes its full time again.
        let after = cast_after(&result, heal);
        assert!(!after.contains("Cast Time = 0s"), "{name}: {after}");
    }
}

/// Water Shield (408510): three globes, each spent by a direct heal that crits, at most once
/// per 3.5 sec, for 2% of the maximum mana.
#[test]
fn water_shield_globes_restore_mana_on_crit_heals() {
    let request = request(
        "restoration-shaman",
        RESTORATION_SHAMAN,
        &[(408510, inactive(408510)), (10623, None)],
    );
    let prepared = prepared(&request);
    // The class effect alone: a healer export carries no second globes effect that could bind
    // beside it and double the mana.
    assert!(effects(&prepared, "mana_globes").is_empty());
    let globes = effects(&prepared, "water_shield");
    assert_eq!(globes.len(), 1);
    assert_eq!(globes[0]["mana_percent"], 2.0);
    assert_eq!(globes[0]["globes"], 3);
    assert_eq!(globes[0]["icd_ns"], 3_500_000_000_i64);
    let result = simulate(&prepared);
    let mana = result["raidMetrics"]["parties"][0]["players"][0]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|resource| resource["id"]["spellId"] == 408511)
        .expect("the globes' mana metrics");
    let (events, gain) = (
        mana["events"].as_f64().unwrap(),
        mana["gain"].as_f64().unwrap(),
    );
    assert!(events > 0.0);
    // At most one globe each 3.5 sec of the 3 minute fights.
    assert!(events <= 20.0 * (180.0 / 3.5 + 1.0), "{events} globes");
    assert!(sum(&result, 408510, "casts") >= events / 3.0);
    assert!(gain / events > 100.0, "{} mana a globe", gain / events);
}

/// The Holy build's talents (16/35/0) and the Discipline build's: Litany of Light 2 of 2 in the
/// first, Renewed Hope 5 of 5 and Divine Aegis 3 of 3 in the second.
const HOLY: &str = "00520101133-33505003030121531";
const POWER_WORD_SHIELD: i64 = 10901;
const FLASH_HEAL: i64 = 10917;
// Greater Heal IV and Renew IX, the ranks a Forever priest casts: Greater Heal V (25314) and
// Renew X (25315) are Ahn'Qiraj 20 book ranks, which Forever's rules leave out.
const GREATER_HEAL: i64 = 10965;
const RENEW: i64 = 10929;
const PENANCE: i64 = 1316995;
const DIVINE_AEGIS_ABSORB: i64 = 431624;
const LITANY_OF_LIGHT: i64 = 1317006;
const DESPERATE_PRAYER: i64 = 19243;

/// The Discipline build with talents spent elsewhere: Soul Warding, Renewed Hope and Divine
/// Aegis are the 14th, 16th and 17th digits of the Discipline tree.
fn discipline_without(soul_warding: bool, renewed_hope: bool, divine_aegis: bool) -> String {
    let mut tree: Vec<char> = DISCIPLINE.chars().collect();
    for (digit, drop) in [(13, soul_warding), (15, renewed_hope), (16, divine_aegis)] {
        if drop {
            tree[digit] = '0';
        }
    }
    tree.into_iter().collect()
}

/// Divine Aegis (431622) at 3 ranks shields 15% of each critical heal that is not a heal over
/// time. Its first effect states the percent (5, 10, 15 over the ranks); its absorb aura 431624
/// is the spell the shielding counts on. The share is taken from the heal before the overheal
/// share comes off, and the absorb is full.
#[test]
fn divine_aegis_shields_a_share_of_a_critical_heal() {
    let built = request("holy-priest", DISCIPLINE, &[(GREATER_HEAL, None)]);
    let prepared = prepared(&built);
    let absorb = &heal(&prepared, GREATER_HEAL)["crit_absorb"];
    assert_eq!(absorb["share"], 0.15);
    assert_eq!(absorb["spell_id"], DIVINE_AEGIS_ABSORB);
    assert!(absorb.get("ticks").is_none());
    // Penance's bolts are heals of their own; a heal over time and a shield are not.
    assert_eq!(heal(&prepared, PENANCE)["crit_absorb"]["ticks"], true);
    assert!(heal(&prepared, RENEW).get("crit_absorb").is_none());
    assert!(heal(&prepared, POWER_WORD_SHIELD)
        .get("crit_absorb")
        .is_none());
    let result = simulate(&prepared);
    let crit_healing = sum(&result, GREATER_HEAL, "critHealing");
    assert!(crit_healing > 0.0);
    // Greater Heal's overhealing share is the pooled one for a heal on one player.
    let overheal = 0.0537;
    let expected = 0.15 * crit_healing / (1.0 - overheal);
    let shielded = sum(&result, DIVINE_AEGIS_ABSORB, "shielding");
    assert!(
        (shielded - expected).abs() < expected * 1e-6,
        "{shielded} shielded, {expected} from the crits"
    );
    // One absorb a crit, no more.
    assert_eq!(
        sum(&result, DIVINE_AEGIS_ABSORB, "hits"),
        sum(&result, GREATER_HEAL, "crits")
    );
}

/// A critical tick of a heal over time shields nothing, and a Discipline priest without the
/// talent has no absorb spell at all.
#[test]
fn divine_aegis_leaves_heals_over_time_alone() {
    let built = request("holy-priest", DISCIPLINE, &[(RENEW, None)]);
    let result = simulate(&prepared(&built));
    assert!(sum(&result, RENEW, "critTicks") > 0.0);
    assert_eq!(sum(&result, DIVINE_AEGIS_ABSORB, "shielding"), 0.0);
    let built = request(
        "holy-priest",
        &discipline_without(false, false, true),
        &[(GREATER_HEAL, None)],
    );
    let prepared = prepared(&built);
    assert!(heal(&prepared, GREATER_HEAL).get("crit_absorb").is_none());
    let result = simulate(&prepared);
    assert_eq!(sum(&result, DIVINE_AEGIS_ABSORB, "shielding"), 0.0);
}

/// Each of Penance's bolts, the one at the cast and the two ticks, is a heal that can crit and
/// shield.
#[test]
fn divine_aegis_shields_penance_bolts() {
    let built = request("holy-priest", DISCIPLINE, &[(PENANCE, None)]);
    let result = simulate(&prepared(&built));
    let crits = sum(&result, PENANCE, "crits") + sum(&result, PENANCE, "critTicks");
    assert!(crits > 0.0);
    assert_eq!(sum(&result, DIVINE_AEGIS_ABSORB, "hits"), crits);
    let gross = sum(&result, PENANCE, "critHealing") / (1.0 - 0.0537);
    let shielded = sum(&result, DIVINE_AEGIS_ABSORB, "shielding");
    assert!((shielded - 0.15 * gross).abs() < gross * 1e-6);
}

/// Renewed Hope (425280) at 5 ranks: 10% crit on a Flash Heal, Heal, Greater Heal or Penance cast
/// on a member with Weakened Soul (6788, 15 sec), which the heal cuts by 5 sec. A Discipline
/// priest that shields and heals crits more often than the same priest without the talent, and
/// one that never shields does not. Soul Warding is left out so the shield has its 4 sec
/// cooldown and the rotation reaches the heal.
#[test]
fn renewed_hope_raises_the_crit_of_heals_on_shielded_members() {
    let rate = |talents: &str, spells: &[(i64, Option<Value>)]| -> f64 {
        let mut built = request("holy-priest", talents, spells);
        built["simOptions"]["iterations"] = json!(100);
        let result = simulate(&prepared(&built));
        let crits = sum(&result, FLASH_HEAL, "crits");
        crits / (crits + sum(&result, FLASH_HEAL, "hits"))
    };
    let shielding: &[(i64, Option<Value>)] = &[(POWER_WORD_SHIELD, None), (FLASH_HEAL, None)];
    let with = rate(&discipline_without(true, false, false), shielding);
    let without = rate(&discipline_without(true, true, false), shielding);
    assert!(with > without + 0.03, "{with} with, {without} without");
    let bare: &[(i64, Option<Value>)] = &[(FLASH_HEAL, None)];
    let with = rate(&discipline_without(true, false, false), bare);
    let without = rate(&discipline_without(true, true, false), bare);
    assert!(
        (with - without).abs() < 0.02,
        "{with} with, {without} without"
    );
}

/// The export states Renewed Hope's row: the crit and the cut over the ranks (2 to 10 points, 1
/// to 5 sec), Weakened Soul's duration, and the heals and shields it reads.
#[test]
fn renewed_hope_exports_its_row() {
    let built = request("holy-priest", DISCIPLINE, &[(FLASH_HEAL, None)]);
    let prepared = prepared(&built);
    let talents = effects(&prepared, "priest_heal_talents");
    assert_eq!(talents.len(), 1);
    let hope = &talents[0]["renewed_hope"];
    assert_eq!(hope["crit_percent"], 10.0);
    assert_eq!(hope["cut_ns"], 5_000_000_000_i64);
    assert_eq!(hope["duration_ns"], 15_000_000_000_i64);
    let heals = hope["heal_spell_ids"].as_array().unwrap();
    assert!(heals.contains(&json!(FLASH_HEAL)) && heals.contains(&json!(PENANCE)));
    assert!(!heals.contains(&json!(RENEW)));
    assert!(hope["shield_spell_ids"]
        .as_array()
        .unwrap()
        .contains(&json!(POWER_WORD_SHIELD)));
    assert!(talents[0]["litany_of_light"].is_null());
}

/// Litany of Light (1317006) at 2 ranks returns 10% of a heal's base mana cost when the heal
/// before it was a different spell. A priest that casts one heal over and over gets it once, on
/// the first; one that alternates Renew and Flash Heal gets it on every change.
#[test]
fn litany_of_light_returns_mana_when_the_heal_changes() {
    let litany = |result: &Value| -> (f64, f64) {
        let mana = result["raidMetrics"]["parties"][0]["players"][0]["resources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|resource| resource["id"]["spellId"] == LITANY_OF_LIGHT)
            .map_or((0.0, 0.0), |mana| {
                (
                    mana["events"].as_f64().unwrap(),
                    mana["gain"].as_f64().unwrap(),
                )
            });
        mana
    };
    let prepared_same = prepared(&request("holy-priest", HOLY, &[(GREATER_HEAL, None)]));
    let talents = effects(&prepared_same, "priest_heal_talents");
    assert_eq!(talents[0]["litany_of_light"]["share"], 0.1);
    assert!(talents[0]["renewed_hope"].is_null());
    let (events, _) = litany(&simulate(&prepared_same));
    assert_eq!(events, 20.0, "once a fight");
    let alternating = request(
        "holy-priest",
        HOLY,
        &[(RENEW, inactive(RENEW)), (FLASH_HEAL, None)],
    );
    let result = simulate(&prepared(&alternating));
    let (events, gain) = litany(&result);
    let renews = sum(&result, RENEW, "casts");
    // A change into Renew and a change out of it, less those the fight's end cuts, and the
    // first cast of each fight.
    assert!(
        events >= 2.0 * renews - 20.0 && events <= 2.0 * renews + 20.0,
        "{events} returns, {renews} Renews"
    );
    // 10% of a base cost of a few hundred mana.
    let each = gain / events;
    assert!(each > 5.0 && each < 100.0, "{each} mana a return");
}

/// Desperate Prayer (19243) is a self heal at no cost on a ten minute cooldown: the rotation
/// that names it casts it once in a fight, and a rotation that does not never does.
#[test]
fn desperate_prayer_heals_once_on_its_cooldown() {
    let built = request(
        "holy-priest",
        DISCIPLINE,
        &[(DESPERATE_PRAYER, None), (FLASH_HEAL, None)],
    );
    let with_prayer = prepared(&built);
    let prayer = heal(&with_prayer, DESPERATE_PRAYER);
    assert!(prayer["direct"]["average"].as_f64().unwrap() > 1000.0);
    assert_eq!(prayer["targets"], 1);
    let result = simulate(&with_prayer);
    assert_eq!(sum(&result, DESPERATE_PRAYER, "casts"), 20.0);
    assert!(sum(&result, DESPERATE_PRAYER, "healing") > 0.0);
    let built = request("holy-priest", DISCIPLINE, &[(FLASH_HEAL, None)]);
    let result = simulate(&prepared(&built));
    assert_eq!(sum(&result, DESPERATE_PRAYER, "casts"), 0.0);
}

/// Go's rules have none of this: no absorb spell, no talent effect, no Desperate Prayer.
#[test]
fn the_reference_rules_leave_the_healer_priest_alone() {
    let built = request("holy-priest", DISCIPLINE, &[(GREATER_HEAL, None)]);
    let value =
        prepare_json_with(built.to_string().as_bytes(), "rust", Rules::GoReference).unwrap();
    let text = value.to_string();
    assert!(!text.contains("crit_absorb"));
    assert!(!text.contains("priest_heal_talents"));
    assert!(effects(&value, "priest_heal_talents").is_empty());
    for spell in [DIVINE_AEGIS_ABSORB, DESPERATE_PRAYER] {
        assert!(!text.contains(&format!("\"spell_id\":{spell}")), "{spell}");
    }
}
