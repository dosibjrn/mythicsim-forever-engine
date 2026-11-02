//! MythicSim's healer sims for the Priest, the Restoration Druid and the Restoration Shaman,
//! which Rust alone runs: the Go engine has none of their heals, so there is no Go golden (the
//! Holy Paladin has Go goldens, in the fixture family). Each request in `fixtures/healers`
//! prepares in Rust, passes the gate and heals.
//!
//! Under Forever's rules a healer sim counts only the share of each heal real healers do not
//! overheal (`prepare::overheal`), so each heal line logs what heals and the overhealing beside
//! it; their sum is the heal the client rows give. A heal over time's tick rolls the healing crit
//! there too, as its row states Periodic Can Crit.
//!
//! Two kinds of check:
//!
//! - The first fight's log lines below pin one heal of each kind to the amount its client rows
//!   give, worked out by hand in the comments: the rank's amount at level 60, its coefficient on
//!   the healing power the line states, and the talents' client modifiers at their ranks.
//! - `<name>.rust-golden.json` keeps the whole fight's healing by spell, as Rust made it, so a
//!   change that moves it shows. It is not evidence that the numbers are right; the log lines
//!   are. Regenerate it with `UPDATE_HEALER_GOLDENS=1 cargo test --test healers` after a change
//!   that should move it, and say why in the commit.

use forever_engine::{check_prepared, prepare, prepare_json_with, simulate_prepared, Rules};
use serde_json::{json, Map, Value};
use std::{fs, path::Path};

fn fixtures() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/healers"))
}

/// A healer request as Forever's rules read it: the Restoration druid's talents in hotfix push
/// 113531's order (Reflection and Subtlety swap, Bloom takes Gift of the Earthmother's square).
fn forever_request(name: &str) -> Vec<u8> {
    let request = fs::read(fixtures().join(format!("{name}.request.json"))).unwrap();
    if name != "restoration-druid" {
        return request;
    }
    let mut value: Value = serde_json::from_slice(&request).unwrap();
    value["raid"]["parties"][0]["players"][0]["talentsString"] =
        json!("05002001--5053005153113051");
    serde_json::to_vec(&value).unwrap()
}

/// The fight's result for a healer request: prepared in Rust, gated and simulated.
fn run(name: &str) -> Value {
    let request = forever_request(name);
    let prepared = prepare(&request, "rust").unwrap_or_else(|err| panic!("{name}: {err:?}"));
    check_prepared(&prepared).unwrap_or_else(|err| panic!("{name}: {err:?}"));
    simulate_prepared(&prepared)
        .unwrap_or_else(|err| panic!("{name}: {err:?}"))
        .result
}

/// [`run`] under the Go reference's rules, where no heal follows the party's damage.
fn run_reference(name: &str) -> Value {
    let request = fs::read(fixtures().join(format!("{name}.request.json"))).unwrap();
    let value = prepare_json_with(&request, "rust", Rules::GoReference)
        .unwrap_or_else(|err| panic!("{name}: {err:?}"));
    assert!(
        !value.to_string().contains("\"party\""),
        "{name}: the reference has no party damage"
    );
    let prepared = serde_json::from_value(value).unwrap();
    check_prepared(&prepared).unwrap_or_else(|err| panic!("{name}: {err:?}"));
    simulate_prepared(&prepared)
        .unwrap_or_else(|err| panic!("{name}: {err:?}"))
        .result
}

/// A heal's players reached a cast: its hits and crits over its casts.
fn players_a_cast(result: &Value, spell: i64) -> f64 {
    let action = player(result)["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["id"]["spellId"] == spell)
        .unwrap();
    let sum = |key: &str| -> f64 {
        action["targets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|target| target[key].as_f64().unwrap_or(0.0))
            .sum()
    };
    (sum("hits") + sum("crits")) / sum("casts")
}

fn player(result: &Value) -> &Value {
    &result["raidMetrics"]["parties"][0]["players"][0]
}

/// The player's healing per second and, by action, what each did in every fight together.
fn summary(result: &Value) -> Value {
    let player = player(result);
    let mut actions = Map::new();
    for action in player["actions"].as_array().unwrap() {
        let sum = |key: &str| -> f64 {
            action["targets"]
                .as_array()
                .unwrap()
                .iter()
                .map(|target| target[key].as_f64().unwrap_or(0.0))
                .sum()
        };
        let healing = sum("healing") + sum("shielding");
        if healing == 0.0 {
            continue;
        }
        actions.insert(
            action["id"].to_string(),
            json!({
                "casts": sum("casts"), "hits": sum("hits"), "crits": sum("crits"),
                "ticks": sum("ticks"), "crit_ticks": sum("critTicks"),
                "healing": sum("healing"), "shielding": sum("shielding"),
                "overhealing": sum("overhealing"),
            }),
        );
    }
    json!({"hps": player["hps"]["avg"], "seconds_oom": player["secondsOomAvg"], "actions": actions})
}

/// The summary equals its golden, or the golden is rewritten when asked.
fn golden(name: &str, result: &Value) {
    let path = fixtures().join(format!("{name}.rust-golden.json"));
    let actual = summary(result);
    if std::env::var_os("UPDATE_HEALER_GOLDENS").is_some() {
        fs::write(&path, serde_json::to_string_pretty(&actual).unwrap() + "\n").unwrap();
        return;
    }
    let expected: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        actual, expected,
        "{name}: the healing moved from its golden"
    );
}

/// The first fight's log has every one of these lines.
fn logs(name: &str, result: &Value, lines: &[&str]) {
    let log = result["logs"].as_str().unwrap();
    for line in lines {
        assert!(
            log.contains(line),
            "{name}: the first fight never logs {line:?}"
        );
    }
}

/// The 38/13 Holy priest: Spiritual Healing 3 (10% direct and periodic), Improved Renew 3
/// (15%), Twin Disciplines 5 (5% periodic on Renew), Improved Power Word: Shield 3 (20%), on
/// 268.2 healing power.
#[test]
fn holy_priest_heals_as_its_rows_state() {
    let result = run("holy-priest");
    logs(
        "holy-priest",
        &result,
        &[
            // Prayer of Mending rank 1 (401859): 172 + 0.429 x 268.2 = 287.06, then +10%, on
            // each charge that heals.
            "{SpellID: 401859} [DEBUG] HealingPower: 268.2, BaseHealing:287.1, AfterCasterMods:315.8",
            // Power Word: Shield rank 10 (10901): 928 + 0.1 x 268.2 = 954.82, then +20%.
            "{SpellID: 10901} Shield for 1145.790.",
            // Renew rank 9 (10929; rank 10, 25315, is an Ahn'Qiraj 20 book rank that Forever's
            // rules leave out): 134 + 0.2 x 268.2 = 187.64 a tick, then 1.10 x 1.15 x 1.05,
            // 249.246, of which Renew's 16.06% is overhealing.
            "{SpellID: 10929} tick Hit for 209.217 healing, 40.029 overhealing.",
        ],
    );
    // Under Forever's rules a charge heals only when the player holding it is hit
    // (`prepare::party_damage`): the tank about once a second, the others about once every five
    // seconds. Recast every ten seconds or so, a cast heals about three charges before the next
    // replaces it; the same process drawn a million times in Python gives 2.97 at ten seconds
    // and 3.03 at 10.3.
    let charges = players_a_cast(&result, 401859);
    assert!(
        (2.85..=3.2).contains(&charges),
        "{charges} Prayer of Mending charges a cast"
    );
    // The Go reference's rules heal all five charges at once.
    let reference = run_reference("holy-priest");
    assert_eq!(players_a_cast(&reference, 401859), 5.0);
    golden("holy-priest", &result);
}

/// The 46-point Restoration druid: Gift of Nature 5 (10% direct and periodic), Improved
/// Rejuvenation 3 (15%), Genesis 5 (10% periodic since hotfix push 113531, which also gives Wild
/// Growth a cast), Nature's Splendor, on 302 healing power.
#[test]
fn restoration_druid_heals_as_its_rows_state() {
    let result = run("restoration-druid");
    logs(
        "restoration-druid",
        &result,
        &[
            // Wild Growth rank 3 (1238215): 97 + 0.033 x 302 = 106.97 a tick, then 1.10 x 1.10
            // (Gift of Nature, and Genesis 5/5 at 2% a rank, whose first effect names Wild
            // Growth), on each of five players, 129.429, of which a group heal's 24.58% is
            // overhealing.
            "{SpellID: 1238215} tick Hit for 97.615 healing, 31.814 overhealing.",
            // Rejuvenation rank 10 (9841; rank 11, 25299, is an Ahn'Qiraj 20 book rank that
            // Forever's rules leave out): 161 + 0.2 x 302 = 221.4 a tick, then 1.10 x 1.15 x 1.10
            // (Genesis 5/5 at 2% a rank since hotfix push 113531), 308.078, of which
            // Rejuvenation's 17.76% is overhealing.
            "{SpellID: 9841} tick Hit for 253.363 healing, 54.715 overhealing.",
            // Swiftmend consumes that Rejuvenation: its five ticks with Nature's Splendor, 1540.4,
            // then Gift of Nature's 10% and Genesis's 10% (its second effect names Swiftmend) on
            // Swiftmend's own direct heal.
            "{SpellID: 18562} [DEBUG] HealingPower: 302.0, BaseHealing:1540.4, AfterCasterMods:1848.5",
            // Bloom, when the Regrowth cast at 9 s and the Rejuvenation cast at 21.5 s end by
            // their duration (27 s and 15 s with Nature's Splendor): 255 Spirit, of which a
            // heal at once's 5.37% is overhealing. The Swiftmends consumed the rest.
            "[36.00] [restoration-druid (#1)] [restoration-druid (#1)] {SpellID: 1328621} Hit for 241.306 healing, 13.694 overhealing.",
            "[36.50] [restoration-druid (#1)] [restoration-druid (#1)] {SpellID: 1328621} Hit for 241.306 healing, 13.694 overhealing.",
        ],
    );
    golden("restoration-druid", &result);
}

/// Bloom (1328621, hotfix push 113531): a Rejuvenation or Regrowth that ends by its duration
/// heals the druid's Spirit at once, a flat heal that nothing modifies and nothing crits, as its
/// own action. The client data states no more (two dummy effects, 100 and 50, and no heal spell).
#[test]
fn bloom_heals_the_druids_spirit_when_a_heal_over_time_ends() {
    // The talent alone: Bloom is the only difference between these two strings.
    let mut without = request_value("restoration-druid");
    without["raid"]["parties"][0]["players"][0]["talentsString"] =
        json!("05002001--5053005053113051");
    let without = run_value(&without);
    let with = run("restoration-druid");
    assert_eq!(action_total(&without, 1328621, "casts"), 0.0);
    assert!(!without["logs"].as_str().unwrap().contains("1328621"));
    let spirit = {
        let prepared = prepare(&forever_request("restoration-druid"), "rust").unwrap();
        let prepared = serde_json::to_value(&prepared).unwrap();
        prepared["player"]["stats"]["Spirit"].as_f64().unwrap()
    };
    assert_eq!(spirit, 255.0);
    // Every Bloom is a hit for the whole Spirit less the heal at once's overhealing share, and
    // nothing else of the druid's heals moves.
    let blooms = action_total(&with, 1328621, "hits");
    assert!(blooms > 0.0);
    assert_eq!(action_total(&with, 1328621, "crits"), 0.0);
    let healing = action_total(&with, 1328621, "healing");
    let overhealing = action_total(&with, 1328621, "overhealing");
    assert!((healing + overhealing - blooms * spirit).abs() < 1e-6);
    assert!((overhealing / (healing + overhealing) - 0.0537).abs() < 1e-9);
    for spell in [9889, 6778, 9841, 9858, 1238215, 18562] {
        assert_eq!(
            action_total(&with, spell, "healing"),
            action_total(&without, spell, "healing"),
            "{spell}"
        );
    }
    let hps = |result: &Value| player(result)["hps"]["avg"].as_f64().unwrap();
    assert!(hps(&with) > hps(&without));

    // The first fight's Rejuvenation and Regrowth casts that Swiftmend consumed bloom nothing, so
    // the log holds fewer Blooms than casts of the two.
    let log = with["logs"].as_str().unwrap();
    let casts = |spell: i64| {
        log.lines()
            .filter(|line| line.contains(&format!("Completed cast {{SpellID: {spell}}}")))
            .count()
    };
    let in_log = log.matches("{SpellID: 1328621} Hit").count();
    assert!(
        in_log > 0 && in_log < casts(9841) + casts(9858),
        "{in_log} Blooms"
    );
}

/// A Rejuvenation cast again while it runs goes on another player and the one running goes on
/// healing its own, so each cast blooms once when it ends: the one it leaves blooms as the new
/// one goes up, and one cut short by the fight's end blooms nothing.
#[test]
fn bloom_follows_every_heal_over_time_to_its_end() {
    let cast = |condition: Option<Value>| {
        let mut action = json!({"action": {"castSpell": {
            "spellId": {"spellId": 9841}, "target": {"type": "Self"}}}});
        if let Some(condition) = condition {
            action["action"]["condition"] = condition;
        }
        action
    };
    let rotation = |items: Vec<Value>| {
        let mut request = request_value("restoration-druid");
        rotation_of(&mut request)["priorityList"] = Value::Array(items);
        run_value(&request)
    };
    let counts = |result: &Value| {
        let log = result["logs"].as_str().unwrap();
        let casts = log.matches("Completed cast {SpellID: 9841}").count();
        (casts, log.matches("{SpellID: 1328621} Hit").count())
    };
    // Recast every global cooldown: every cast but the first leaves one running.
    let (casts, blooms) = counts(&rotation(vec![cast(None)]));
    assert!(casts > 20, "{casts} casts");
    assert!(
        (casts - 1..=casts).contains(&blooms),
        "{blooms} Blooms for {casts} casts"
    );
    // Recast only when it has ended: each ends by its duration, Rejuvenation's 12 seconds and
    // Nature's Splendor's 3, and all bloom but the one the fight cuts short.
    let after_ending = json!({"not": {"val": {"auraIsActive": {"auraId": {"spellId": 9841}}}}});
    let result = rotation(vec![cast(Some(after_ending))]);
    let (casts, blooms) = counts(&result);
    assert!(casts > 5, "{casts} casts");
    assert!(
        (casts - 1..=casts).contains(&blooms),
        "{blooms} Blooms for {casts} casts"
    );
    // A Bloom is 15.00 seconds after a cast that completed then.
    let log = result["logs"].as_str().unwrap();
    let time = |line: &str| -> f64 { line[1..line.find(']').unwrap()].parse().unwrap() };
    let completed: Vec<f64> = log
        .lines()
        .filter(|line| line.contains("Completed cast {SpellID: 9841}"))
        .map(time)
        .collect();
    for line in log
        .lines()
        .filter(|line| line.contains("{SpellID: 1328621} Hit"))
    {
        let at = time(line);
        assert!(
            completed.iter().any(|cast| (at - cast - 15.0).abs() < 1e-9),
            "a Bloom at {at} s follows no cast by 15 s"
        );
    }
}

/// The 40-point Restoration shaman: Purification 5 (10% direct and periodic), Healing Way 3
/// (25% on Healing Wave), Restorative Totems 5 (25% on Healing Stream's ticks).
#[test]
fn restoration_shaman_heals_as_its_rows_state() {
    let result = run("restoration-shaman");
    let log = result["logs"].as_str().unwrap();
    // Chain Heal rank 3 (10623) lands on up to three players, each with its own roll of 473 to
    // 539 plus 0.714 of healing power, the second at half and the third at a quarter. A cast's
    // first heal is the largest, so a heal larger than the one before starts the next cast.
    // The first player heals 25% more with a Riptide up (the Riptide holder, see
    // `Shaman::riptide_chain_heal_bonus`): the fixture's Riptide is up for nearly every cast, and
    // the casts it is not up for read the roll without it.
    let field = |line: &str, name: &str| -> f64 {
        let at = line.find(name).unwrap() + name.len();
        line[at..].split(',').next().unwrap().parse().unwrap()
    };
    let mut casts: Vec<Vec<(f64, f64)>> = Vec::new();
    for line in log
        .lines()
        .filter(|line| line.contains("{SpellID: 10623} [DEBUG]"))
    {
        let heal = (field(line, "HealingPower: "), field(line, "BaseHealing:"));
        match casts.last_mut() {
            Some(cast) if cast.last().is_some_and(|last| heal.1 < last.1) => cast.push(heal),
            _ => casts.push(vec![heal]),
        }
    }
    assert!(!casts.is_empty());
    for cast in &casts {
        assert!(cast.len() <= 3, "{} Chain Heal jumps", cast.len());
        for (jump, &(healing_power, base)) in cast.iter().enumerate() {
            let roll = base / 0.5f64.powi(jump as i32) - 0.714 * healing_power;
            let boosted = jump == 0 && !(472.5..=539.5).contains(&roll);
            let roll = if boosted {
                // The first player's whole heal, roll and healing power, is 25% larger.
                base / 1.25 - 0.714 * healing_power
            } else {
                roll
            };
            assert!((472.5..=539.5).contains(&roll), "jump {jump}: roll {roll}");
        }
    }
    // Under Forever's rules it jumps only to players hit in the last four seconds
    // (`prepare::party_damage`): each of the other four with 1 - e^(-0.2 x 4) = 55%, so a cast
    // reaches 2.72 players on average. The Go reference's rules reach all three.
    let players = players_a_cast(&result, 10623);
    assert!(
        (2.65..=2.79).contains(&players),
        "{players} players a Chain Heal"
    );
    let reference = run_reference("restoration-shaman");
    assert_eq!(players_a_cast(&reference, 10623), 3.0);
    // Riptide recast every 6 seconds keeps its 15 second heal over time on each player: about
    // five ticks a cast, not the two or three a cut short one would give.
    let riptide = player(&result)["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["id"]["spellId"] == 1239243)
        .unwrap();
    let sum = |key: &str| -> f64 {
        riptide["targets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t[key].as_f64().unwrap_or(0.0))
            .sum()
    };
    let ticks = sum("ticks") + sum("critTicks");
    assert!(
        ticks > 4.5 * sum("casts"),
        "{ticks} ticks for {} casts",
        sum("casts")
    );
    golden("restoration-shaman", &result);
}

/// Under Forever's rules a healer sim names the overhealing share of each of its heals, and only
/// what is left counts: the Holy Paladin's Flash of Light, Holy Light and Holy Shock heals lose
/// their measured shares, so its healing per second falls by each heal's share of its healing.
/// The Go reference's rules, which the Holy Paladin's Go goldens use, and a damage spec name none.
#[test]
fn forever_healer_sims_count_healing_less_overhealing() {
    let family = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/mage/prepared-v2"
    ));
    let request = |name: &str| fs::read(family.join(format!("{name}.request.json"))).unwrap();
    // Without racials: the 26 October racials are Forever's own, and this compares the
    // overhealing split alone.
    let mut holy: Value = serde_json::from_slice(&request("holy-paladin-healing")).unwrap();
    holy["raid"]["parties"][0]["players"][0]["disableRacials"] = true.into();
    // Talents that mean the same in the reference's tree and push 113554's, which Forever's
    // rules read, so both draw the same rolls: Divine Strength 5, Healing Light 3, Infusion of
    // Light 2, Illumination 5, Divine Favor and Holy Shock; Precision 3, Guardian's Favor 2,
    // Toughness 5 and Anticipation 3.
    holy["raid"]["parties"][0]["players"][0]["talentsString"] = "50300000025101-50323".into();
    // The reference keeps the Ahn'Qiraj 20 book rank the rotation names (Holy Light IX, 25292)
    // where Forever's rules cast the rank below (Holy Light VIII, 10329), so the reference casts
    // that rank here too: both draw the same rolls.
    let reference_request = serde_json::to_string(&holy)
        .unwrap()
        .replace("25292", "10329");
    let reference =
        prepare_json_with(reference_request.as_bytes(), "rust", Rules::GoReference).unwrap();
    holy["raid"]["parties"][0]["players"][0]["talentsString"] =
        "05300015002001-003200005003".into();
    let holy = serde_json::to_vec(&holy).unwrap();
    let forever = prepare_json_with(&holy, "rust", Rules::Forever).unwrap();
    assert_eq!(forever["player"]["talents"], reference["player"]["talents"]);
    let share = |spell: i64| {
        forever["player"]["overheal"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["spell_id"] == spell)
            .map(|entry| entry["fraction"].as_f64().unwrap())
    };
    // Flash of Light rank 6, Holy Light rank 8 (rank 9 is an Ahn'Qiraj 20 book rank) and the Go
    // reference's Holy Shock heal.
    assert_eq!(share(19943), Some(0.0699));
    assert_eq!(share(10329), Some(0.1024));
    assert_eq!(share(25903), Some(0.0658));
    // Holy Shock's damage is no heal.
    assert_eq!(share(1311606), None);
    assert!(reference["player"].get("overheal").is_none());
    let shadow = request("shadow-priest-holy-nova");
    let shadow = prepare_json_with(&shadow, "rust", Rules::Forever).unwrap();
    assert!(shadow["player"].get("overheal").is_none());

    // The heals draw the same rolls either way, so each heal's healing and overhealing add up
    // to what the reference heals, split at its share.
    let sim = |prepared: &Value| {
        let prepared = serde_json::from_value(prepared.clone()).unwrap();
        simulate_prepared(&prepared).unwrap().result
    };
    let (forever, reference) = (sim(&forever), sim(&reference));
    let totals = |result: &Value, spell: i64, key: &str| -> f64 {
        player(result)["actions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|action| action["id"]["spellId"] == spell)
            .flat_map(|action| action["targets"].as_array().unwrap())
            .map(|target| target[key].as_f64().unwrap_or(0.0))
            .sum()
    };
    for (spell, fraction) in [(19943, 0.0699), (10329, 0.1024), (25903, 0.0658)] {
        let whole = totals(&reference, spell, "healing");
        let healing = totals(&forever, spell, "healing");
        let overhealing = totals(&forever, spell, "overhealing");
        assert!(whole > 0.0, "{spell} heals");
        assert!(
            ((healing + overhealing) / whole - 1.0).abs() < 1e-9,
            "{spell}"
        );
        assert!((overhealing / whole - fraction).abs() < 1e-9, "{spell}");
    }
    let hps = |result: &Value| player(result)["hps"]["avg"].as_f64().unwrap();
    assert!(hps(&forever) < hps(&reference));
}

/// Heal over time ticks crit in Forever: foreverlogs.gg's level 30 beta logs flag Renew,
/// Rejuvenation, Regrowth and Healing Stream Totem ticks as crits at 1.5 times the tick, mid-way
/// between normal ticks on the same player, at about the caster's direct heal crit share
/// (`validation/2026-10-25-heal-tick-crits.json`), and their client rows state Periodic Can Crit.
/// Under Forever's rules each tick rolls the healing crit on spell crit; the Go reference's
/// ticks never crit.
#[test]
fn forever_heal_ticks_crit_as_their_rows_state() {
    let request = fs::read(fixtures().join("restoration-druid.request.json")).unwrap();
    let forever = prepare_json_with(
        &forever_request("restoration-druid"),
        "rust",
        Rules::Forever,
    )
    .unwrap();
    let reference = prepare_json_with(&request, "rust", Rules::GoReference).unwrap();
    let flagged = |prepared: &Value, spell: i64| -> bool {
        prepared["effects"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|effect| effect["kind"] == "heals")
            .flat_map(|effect| effect["heals"].as_array().unwrap())
            .find(|heal| heal["spell_id"] == spell)
            .map(|heal| heal["tick_can_crit"] == true)
            .unwrap()
    };
    // Rejuvenation rank 10 (rank 11 is an Ahn'Qiraj 20 book rank), Regrowth rank 9 and Wild
    // Growth rank 3; Healing Touch (rank 10) has no tick.
    for spell in [9841, 9858, 1238215] {
        assert!(flagged(&forever, spell), "{spell} ticks crit under Forever");
        assert!(
            !flagged(&reference, spell),
            "{spell} ticks never crit in Go"
        );
    }
    assert!(!flagged(&forever, 9889));

    let sim = |prepared: &Value| {
        let prepared = serde_json::from_value(prepared.clone()).unwrap();
        simulate_prepared(&prepared).unwrap().result
    };
    let (forever, reference) = (sim(&forever), sim(&reference));
    let totals = |result: &Value, spell: i64, key: &str| -> f64 {
        player(result)["actions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|action| action["id"]["spellId"] == spell)
            .flat_map(|action| action["targets"].as_array().unwrap())
            .map(|target| target[key].as_f64().unwrap_or(0.0))
            .sum()
    };
    // The druid's spell crit, 15.86% and 25.86% while Elune's Light is up: Rejuvenation takes
    // no crit bonus of its own.
    let crits = totals(&forever, 9841, "critTicks");
    let share = crits / (crits + totals(&forever, 9841, "ticks"));
    assert!(
        (0.15..0.2586).contains(&share),
        "Rejuvenation crits {share}"
    );
    for spell in [9841, 9858, 1238215] {
        assert_eq!(totals(&reference, spell, "critTicks"), 0.0, "{spell}");
        assert!(totals(&forever, spell, "critTicks") > 0.0, "{spell}");
    }
    // A crit tick heals 1.5 times the 308.078 a tick of Rejuvenation rank 10 heals (above),
    // 462.117, of which Rejuvenation's 17.76% is overhealing.
    logs(
        "restoration-druid",
        &forever,
        &["{SpellID: 9841} tick Crit for 380.045 healing, 82.072 overhealing."],
    );
}

/// A healer sim under Forever's rules reports its burst healing beside its healing per second:
/// the most it healed in any ten seconds of each fight, over ten seconds (`prepare::burst_hps`).
/// It is at least the fight's whole healing per second, and at most all of a fight's healing in
/// ten seconds. The Go reference's rules report none.
#[test]
fn forever_healer_sims_report_burst_healing() {
    for name in ["holy-priest", "restoration-druid", "restoration-shaman"] {
        let result = run(name);
        let player = player(&result);
        let hps = player["hps"]["avg"].as_f64().unwrap();
        let burst = player["burstHps"]["avg"].as_f64().unwrap();
        let fight = result["avgIterationDuration"].as_f64().unwrap_or(120.0);
        println!("{name}: {hps:.1} HPS, burst {burst:.1}, fight {fight:.1} s");
        assert!(burst > hps, "{name}: burst {burst} at most {hps}");
        assert!(
            burst < hps * fight / 10.0,
            "{name}: burst {burst} above the fight's healing"
        );
        assert!(
            run_reference(name)["raidMetrics"]["parties"][0]["players"][0]
                .get("burstHps")
                .is_none(),
            "{name}: the reference reports burst"
        );
    }
}

/// The first fight's burst equals the best ten seconds of the healing its log lines show: every
/// heal, tick and shield on the healer, less overhealing. The log's times are rounded to the
/// hundredth of a second, so the check brackets it between windows a hundredth shorter and longer.
#[test]
fn the_first_fights_burst_is_its_best_ten_seconds_of_logged_healing() {
    let mut request: Value =
        serde_json::from_slice(&fs::read(fixtures().join("holy-priest.request.json")).unwrap())
            .unwrap();
    request["simOptions"]["saveAllValues"] = json!(true);
    let prepared = prepare(request.to_string().as_bytes(), "rust").unwrap();
    check_prepared(&prepared).unwrap();
    let result = simulate_prepared(&prepared).unwrap().result;
    let first = player(&result)["burstHps"]["allValues"][0]
        .as_f64()
        .unwrap();
    let mut heals: Vec<(f64, f64)> = Vec::new();
    for line in result["logs"].as_str().unwrap().lines() {
        let labels: Vec<&str> = line.splitn(4, "] [").collect();
        if labels.len() < 3 {
            continue;
        }
        let (unit, target) = (labels[1], labels[2].split(']').next().unwrap());
        if unit != target {
            continue;
        }
        let amount = if let Some(at) = line.find(" for ").filter(|_| line.contains(" healing")) {
            line[at + 5..].split(' ').next().unwrap()
        } else if let Some(at) = line.find(" Shield for ") {
            line[at + 12..].trim_end_matches('.')
        } else {
            continue;
        };
        let time: f64 = line[1..line.find(']').unwrap()].parse().unwrap();
        heals.push((time, amount.parse().unwrap()));
    }
    assert!(heals.len() > 50, "{} heals logged", heals.len());
    let best = |window: f64| -> f64 {
        heals
            .iter()
            .map(|&(end, _)| {
                heals
                    .iter()
                    .filter(|&&(at, _)| at > end - window && at <= end)
                    .map(|&(_, amount)| amount)
                    .sum::<f64>()
            })
            .fold(0.0, f64::max)
            / 10.0
    };
    let (low, high) = (best(9.99), best(10.01));
    assert!(
        low - 0.01 <= first && first <= high + 0.01,
        "burst {first}, logged {low} to {high}"
    );
}

/// Inner Focus (14751) makes the healer's next heal free and then ends: its client row has one
/// proc charge, and its cost effect's class mask names every heal. A heal spends it; a
/// healer's Inner Focus that no heal spent made every later heal free until the fight ended.
#[test]
fn a_heal_spends_inner_focus() {
    let mut request: Value =
        serde_json::from_slice(&fs::read(fixtures().join("holy-priest.request.json")).unwrap())
            .unwrap();
    request["simOptions"]["iterations"] = json!(1);
    let prepared = prepare(request.to_string().as_bytes(), "rust").unwrap();
    check_prepared(&prepared).unwrap();
    let result = simulate_prepared(&prepared).unwrap().result;
    let lines: Vec<&str> = result["logs"].as_str().unwrap().lines().collect();
    let gains: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains("Aura gained: {SpellID: 14751}"))
        .map(|(i, _)| i)
        .collect();
    assert!(
        gains.len() >= 2,
        "{} Inner Focus casts in a 3 minute fight",
        gains.len()
    );
    for &gained in &gains {
        let faded = lines[gained..]
            .iter()
            .position(|line| line.contains("Aura faded: {SpellID: 14751}"))
            .map(|offset| gained + offset)
            .expect("Inner Focus fades");
        let time = |line: &str| -> f64 { line[1..line.find(']').unwrap()].parse().unwrap() };
        // Whatever spent it, it ends at once rather than at the fight's end.
        assert!(
            time(lines[faded]) - time(lines[gained]) < 3.0,
            "Inner Focus up from {} to {}",
            lines[gained],
            lines[faded]
        );
    }
    // After the last one, heals cost mana again.
    let last = *gains.last().unwrap();
    assert!(
        lines[last..]
            .iter()
            .any(|line| line.contains("Spent") && line.contains("mana from")),
        "no heal paid for mana after the last Inner Focus"
    );
}

/// A request's result, prepared in Rust under Forever's rules, gated and simulated.
fn run_value(request: &Value) -> Value {
    let request = serde_json::to_vec(request).unwrap();
    let prepared = prepare(&request, "rust").unwrap();
    check_prepared(&prepared).unwrap();
    simulate_prepared(&prepared).unwrap().result
}

/// A healer fixture's request as JSON, for a test to change.
fn request_value(name: &str) -> Value {
    serde_json::from_slice(&forever_request(name)).unwrap()
}

fn rotation_of(request: &mut Value) -> &mut Value {
    &mut request["raid"]["parties"][0]["players"][0]["rotation"]
}

/// One action's total of a result key, over its targets.
fn action_total(result: &Value, spell: i64, key: &str) -> f64 {
    player(result)["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|action| action["id"]["spellId"] == spell)
        .flat_map(|action| action["targets"].as_array().unwrap())
        .map(|target| target[key].as_f64().unwrap_or(0.0))
        .sum()
}

/// The `BaseHealing` of each of a spell's heals in the first fight's log, in order.
fn base_healing(result: &Value, spell: i64) -> Vec<f64> {
    let marker = format!("{{SpellID: {spell}}} [DEBUG]");
    result["logs"]
        .as_str()
        .unwrap()
        .lines()
        .filter(|line| line.contains(&marker))
        .map(|line| {
            let at = line.find("BaseHealing:").unwrap() + "BaseHealing:".len();
            line[at..].split(',').next().unwrap().parse().unwrap()
        })
        .collect()
}

/// The 31/18 Holy Paladin on push 113554's tree with Oath of Light (1328548) and Boundless Light
/// 2/2, on 305 healing power, sworn on the heal target before the pull and again when under five
/// seconds are left. The Oath adds its flat bonus to the heal's base amount, before healing power
/// (the Greater Blessing of Light's 400 and 115 are the same numbers): Holy Light 400, Flash of
/// Light 115 and Holy Shock 229, the rows' 6.67, 1.92 and 3.83 a level at level 60. While the
/// Oath is up a heal reaches its holder, the tank, with the tank's share of the party's hits
/// (`party_damage::tank_share`, 5 in 9), and only those heals carry the bonus: the rest are a
/// plain roll.
#[test]
fn holy_paladin_oath_of_light_adds_its_bonus_to_the_three_heals() {
    let result = run("holy-paladin");
    logs(
        "holy-paladin",
        &result,
        &[
            // Sworn three seconds before the pull for 8% of the paladin's base mana, 120.
            "Casting {SpellID: 1328548} (Cost = 120.000, Cast Time = 0s, GCD = 1.5s",
            "Aura gained: {SpellID: 1328548}",
            // Holy Shock rank 4 (25903) heals 320 and has no roll: 320 + 229 + 0.429 x 305, then
            // Healing Light's 12%.
            "{SpellID: 25903} [DEBUG] HealingPower: 305.0, BaseHealing:679.8, AfterCasterMods:761.4",
        ],
    );
    // Without the Oath's cast the same heal is 320 + 0.429 x 305.
    let mut bare = request_value("holy-paladin");
    let rotation = rotation_of(&mut bare);
    rotation["prepullActions"] = json!([]);
    rotation["priorityList"].as_array_mut().unwrap().remove(1);
    let bare = run_value(&bare);
    logs(
        "holy-paladin (no Oath)",
        &bare,
        &["{SpellID: 25903} [DEBUG] HealingPower: 305.0, BaseHealing:450.8, AfterCasterMods:504.9"],
    );
    assert!(!bare["logs"].as_str().unwrap().contains("1328548"));

    // Every roll of Holy Light rank 8 (rank 9 is an Ahn'Qiraj 20 book rank) and Flash of Light
    // rank 6 lands in its row's range plus
    // the Oath's bonus and the coefficient on healing power.
    let prepared = prepare(&forever_request("holy-paladin"), "rust").unwrap();
    let prepared = serde_json::to_value(&prepared).unwrap();
    let heals = prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == "paladin_heals")
        .unwrap();
    for (spell, coefficient, bonus) in [(10329, 0.714, 400.0), (19943, 0.429, 115.0)] {
        let rank = heals["ranks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|rank| rank["spell_id"] == spell)
            .unwrap();
        let (min, max) = (rank["min"].as_f64().unwrap(), rank["max"].as_f64().unwrap());
        let swore = base_healing(&result, spell);
        let bare = base_healing(&bare, spell);
        assert!(!swore.is_empty() && !bare.is_empty(), "{spell}");
        let in_range = |base: f64, plus: f64| {
            (min - 0.1..=max + 0.1).contains(&(base - coefficient * 305.0 - plus))
        };
        // Without the Oath's cast every heal is a plain roll.
        for &base in &bare {
            assert!(in_range(base, 0.0), "{spell} without the Oath: {base}");
        }
        // With it, a heal is the tank's (the bonus on its roll) or another player's (a plain
        // roll), the tank's about 5 in 9 of them.
        let (mut tank, mut others) = (0, 0);
        for &base in &swore {
            if in_range(base, bonus) {
                tank += 1;
            } else {
                assert!(in_range(base, 0.0), "{spell} with the Oath: {base}");
                others += 1;
            }
        }
        assert!(
            tank > 0 && others > 0,
            "{spell}: {tank} tank heals, {others} others"
        );
        let share = f64::from(tank) / f64::from(tank + others);
        assert!(
            (0.3..=0.8).contains(&share),
            "{spell}: the tank's share {share}"
        );
    }
}

/// Boundless Light's Holy Shock heal crit holds only on an ally without the Oath: while the
/// paladin's Oath is up, the Holy Shock heals that reach the tank (5 in 9 of them) lose its 30
/// points of crit and the rest keep it, so the heal crits about 17 points less often. The Oath's
/// bonuses on the tank's heals outweigh that lost crit and its mana, so healing per second rises.
#[test]
fn oath_of_light_takes_boundless_lights_crit_off_the_holy_shock_heal() {
    let sworn = run("holy-paladin");
    let mut bare = request_value("holy-paladin");
    let rotation = rotation_of(&mut bare);
    rotation["prepullActions"] = json!([]);
    rotation["priorityList"].as_array_mut().unwrap().remove(1);
    let bare = run_value(&bare);
    let crit_share = |result: &Value| {
        let crits = action_total(result, 25903, "crits");
        crits / (crits + action_total(result, 25903, "hits"))
    };
    let (with, without) = (crit_share(&sworn), crit_share(&bare));
    assert!(
        (0.12..=0.22).contains(&(without - with)),
        "Holy Shock crits {with} with the Oath, {without} without"
    );
    let hps = |result: &Value| player(result)["hps"]["avg"].as_f64().unwrap();
    assert!(
        hps(&sworn) > hps(&bare) * 1.1,
        "{} {}",
        hps(&sworn),
        hps(&bare)
    );
    // The Oath is no heal: one cast every five minutes at most, and it heals nothing itself.
    assert_eq!(action_total(&sworn, 1328548, "healing"), 0.0);
    assert!(action_total(&sworn, 1328548, "casts") >= 300.0);
    golden("holy-paladin", &sworn);
}

/// An Oath cast on any unit but the player swears it on an ally the sim does not model: it costs
/// the mana and replaces the Oath on the player, whose heals then carry no bonus.
#[test]
fn an_oath_sworn_on_another_unit_bonuses_nothing() {
    let mut request = request_value("holy-paladin");
    let rotation = rotation_of(&mut request);
    // The prepull swears it at the first target, an ally elsewhere, and nothing swears it on
    // the player.
    rotation["prepullActions"][0]["action"] = json!({
        "castSpell": {"spellId": {"spellId": 1328548}}
    });
    rotation["priorityList"].as_array_mut().unwrap().remove(1);
    let elsewhere = run_value(&request);
    assert!(elsewhere["logs"]
        .as_str()
        .unwrap()
        .contains("Completed cast {SpellID: 1328548}"));
    assert!(!elsewhere["logs"]
        .as_str()
        .unwrap()
        .contains("Aura gained: {SpellID: 1328548}"));
    assert_eq!(base_healing(&elsewhere, 25903)[0], 450.8);
}
