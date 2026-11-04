use forever_engine::{
    check_prepared, contracts::prepared_v2::PreparedV2, prepared_coverage, PreparedError,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{fs, path::Path};

#[derive(Deserialize)]
struct Manifest {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    prepared: String,
    expected_coverage: Coverage,
}

#[derive(Deserialize)]
struct Coverage {
    supported: bool,
    reasons: Vec<String>,
}

fn family() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/mage/prepared-v2")
}

fn manifest() -> Manifest {
    serde_json::from_slice(&fs::read(family().join("manifest.json")).unwrap()).unwrap()
}

fn reference_json() -> Value {
    serde_json::from_slice(&fs::read(family().join("frost-reference.prepared.json")).unwrap())
        .unwrap()
}

type Mutation = (&'static str, fn(&mut Value));

fn parse(value: Value) -> Result<PreparedV2, serde_json::Error> {
    serde_json::from_value(value)
}

fn reasons(value: Value) -> Vec<String> {
    match check_prepared(&parse(value).unwrap()) {
        Err(PreparedError::Unsupported(reasons)) => reasons,
        other => panic!("expected unsupported, got {other:?}"),
    }
}

#[test]
fn accepted_fixtures_report_their_expected_coverage() {
    for case in manifest().cases {
        let bytes = fs::read(family().join(&case.prepared)).unwrap();
        let prepared: PreparedV2 = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(prepared.scenario_id, case.id);
        let result = check_prepared(&prepared);
        let reasons = match result {
            Ok(()) => Vec::new(),
            Err(PreparedError::Unsupported(reasons)) => reasons,
            Err(err) => panic!("{} invalid: {err}", case.id),
        };
        assert_eq!(
            reasons.is_empty(),
            case.expected_coverage.supported,
            "{}",
            case.id
        );
        assert_eq!(reasons, case.expected_coverage.reasons, "{}", case.id);
    }
}

#[test]
fn real_reference_needs_only_named_mechanics() {
    // The real request is fully representable: no exporter gaps, unclaimed listeners or
    // rotation operators remain. Only unimplemented effects block it.
    let prepared = parse(reference_json()).unwrap();
    for reason in prepared_coverage(&prepared) {
        assert!(
            reason.starts_with("effect ") && reason.ends_with(" is not implemented"),
            "{reason}"
        );
    }
}

#[test]
fn unknown_fields_and_effect_kinds_fail_deserialization() {
    let mut top = reference_json();
    top["new_field"] = json!(1);
    assert!(parse(top)
        .unwrap_err()
        .to_string()
        .contains("unknown field `new_field`"));

    let mut spell = reference_json();
    spell["player"]["spells"][0]["new_modifier"] = json!(1.5);
    assert!(parse(spell)
        .unwrap_err()
        .to_string()
        .contains("unknown field `new_modifier`"));

    let mut effect = reference_json();
    effect["effects"]
        .as_array_mut()
        .unwrap()
        .push(json!({"kind": "mana_shield"}));
    assert!(parse(effect)
        .unwrap_err()
        .to_string()
        .contains("unknown variant `mana_shield`"));

    let mut parameter = reference_json();
    for effect in parameter["effects"].as_array_mut().unwrap() {
        if effect["kind"] == "winters_chill" {
            effect["extra_stack_rule"] = json!(true);
        }
    }
    assert!(parse(parameter).is_err());
}

#[test]
fn identity_and_bounds_violations_are_invalid_not_unsupported() {
    let cases: [Mutation; 6] = [
        ("revision", |v| {
            v["reference"]["engine_revision"] = json!("0".repeat(40))
        }),
        ("client", |v| {
            v["reference"]["client_build"] = json!("1.60.0.1")
        }),
        ("schema", |v| v["contract"] = json!("forever-result")),
        ("seed", |v| v["sim"]["seed"] = json!(0)),
        ("regen", |v| {
            v["player"]["mana"]["spirit_regen_per_second"] = json!(40.0)
        }),
        ("cast speed", |v| {
            v["player"]["stats"]["SpellHasteRating"] = json!(10.0)
        }),
    ];
    for (name, mutate) in cases {
        let mut value = reference_json();
        mutate(&mut value);
        let result = check_prepared(&parse(value).unwrap());
        assert!(
            matches!(result, Err(PreparedError::Invalid(_))),
            "{name}: {result:?}"
        );
    }
}

#[test]
fn exporter_gaps_block_simulation() {
    let mut value = reference_json();
    value["unrepresented"] = json!(["target auto attacks are unsupported"]);
    assert!(reasons(value)
        .contains(&"unrepresented by the exporter: target auto attacks are unsupported".into()));
}

#[test]
fn active_listeners_without_an_effect_are_reported() {
    let mut value = reference_json();
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["aura"] != "Parry Haste");
    assert!(reasons(value).contains(
        &"target aura \"Parry Haste\" listens to combat events without an effect".into()
    ));
}

/// The weapon enchant damage procs and the listeners of melee hits taken from the app's item
/// catalog each need their effect; the procs carry their proc manager's chance per spell.
#[test]
fn catalog_item_procs_need_their_effects() {
    let path = family().join("combat-rogue-item-procs.prepared.json");
    let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert!(check_prepared(&parse(value.clone()).unwrap()).is_ok());
    let weapon_procs: Vec<&Value> = value["effects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|effect| effect["kind"] == "spell_data_damage_proc" && effect["chances"].is_array())
        .collect();
    assert_eq!(weapon_procs.len(), 2);
    for (aura, kind) in [
        ("Enchant Weapon - Fiery Weapon", "spell_data_damage_proc"),
        ("Enchant Weapon - Lifestealing", "spell_data_damage_proc"),
        ("Orb of Fire", "spell_data_damage_proc"),
        ("The Lion Horn of Stormwind", "inert_listener"),
        ("Enchant Chest - Absorption", "inert_listener"),
    ] {
        let mut value = value.clone();
        value["effects"].as_array_mut().unwrap().retain(|effect| {
            !(effect["kind"] == kind && effect["trigger_aura"] == aura || effect["aura"] == aura)
        });
        assert!(
            reasons(value).contains(&format!(
                "player aura \"{aura}\" listens to combat events without an effect"
            )),
            "{aura}"
        );
    }
}

/// The spell data stat procs and the mana on-use items each need their effect.
#[test]
fn spell_procs_and_mana_items_need_their_effects() {
    let load = |case: &str| -> Value {
        let path = family().join(format!("{case}.prepared.json"));
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    };
    for case in [
        "arcane-mage-spell-stat-procs",
        "fire-mage-mana-on-use-trinkets",
        "elemental-shaman-insight-earthen-sigil",
    ] {
        assert!(
            check_prepared(&parse(load(case)).unwrap()).is_ok(),
            "{case}"
        );
    }
    for (case, trigger) in [
        (
            "arcane-mage-spell-stat-procs",
            "Enchant Weapon - Grand Sorcerer",
        ),
        ("arcane-mage-spell-stat-procs", "Draconic Infused Emblem"),
        (
            "elemental-shaman-insight-earthen-sigil",
            "Enchant Weapon - Insight",
        ),
    ] {
        let mut value = load(case);
        value["effects"]
            .as_array_mut()
            .unwrap()
            .retain(|effect| effect["trigger_aura"] != trigger);
        assert!(
            reasons(value).contains(&format!(
                "player aura \"{trigger}\" listens to combat events without an effect"
            )),
            "{trigger}"
        );
    }
    for (case, item) in [
        ("fire-mage-mana-on-use-trinkets", 18371),
        ("fire-mage-mana-on-use-trinkets", 11832),
        ("elemental-shaman-insight-earthen-sigil", 20525),
    ] {
        let mut value = load(case);
        value["effects"]
            .as_array_mut()
            .unwrap()
            .retain(|effect| effect["item_id"] != item);
        assert!(
            reasons(value).contains(&format!(
                "rotation reaches item {item} without a known behavior"
            )),
            "{item}"
        );
    }
}

#[test]
fn unsupported_rotation_operators_are_reported() {
    let mut value = reference_json();
    value["player"]["rotation"]["priorityList"][3]["action"]["condition"] =
        json!({"notAValue": {"spellId": {"spellId": 12579}}});
    assert!(reasons(value).contains(&"rotation item 4: value notAValue is unsupported".into()));

    let mut prepull = reference_json();
    prepull["player"]["rotation"]["prepullActions"] = json!([{"action": {"castSpell": {"spellId": {"spellId": 25304}}}, "doAtValue": {"const": {"val": "-1s"}}}]);
    // The exported count says Go registered none, so this rotation is not the one Go ran.
    assert!(reasons(prepull).contains(
        &"Go registered 0 prepull actions and the rotation 1; prepull actions outside the rotation are unsupported"
            .into()
    ));
}

#[test]
fn rotation_spells_without_behavior_are_reported() {
    // Arcane Explosion runs only with the effect that describes it.
    let mut value = reference_json();
    value["player"]["rotation"]["priorityList"][5]["action"]["castSpell"]["spellId"] =
        json!({"spellId": 10202});
    assert!(check_prepared(&parse(value.clone()).unwrap()).is_ok());
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "arcane_explosion");
    assert!(
        reasons(value).contains(&"rotation reaches spell 10202 without a known behavior".into())
    );
}

/// Blood Fury sets the stats Go computes while it is active. The runtime changes only
/// the stats it reads during a fight, so a change to any other stat is unsupported.
#[test]
fn temporary_stat_changes_must_be_to_dynamic_stats() {
    let path = family().join("frost-orc.prepared.json");
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert!(check_prepared(&parse(value.clone()).unwrap()).is_ok());
    let blood_fury = value["effects"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|effect| effect["kind"] == "blood_fury")
        .unwrap();
    blood_fury["active_stats"]["SpellHasteRating"] = json!(10.0);
    assert!(reasons(value).contains(
        &"Blood Fury changes SpellHasteRating, which the runtime holds fixed".to_string()
    ));
}

/// Ignite acts on the crits of any reachable Fire spell, so a rotation that adds
/// Fireball to an Ignite build stays supported.
#[test]
fn ignite_builds_may_cast_fire_spells() {
    let path = family().join("arcane-ignite.prepared.json");
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let fireball = value["player"]["spells"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|spell| spell["class_spell"] == "fireball")
        .unwrap()["action_id"]
        .clone();
    let id = fireball["spell_id"].as_i64().unwrap();
    value["player"]["rotation"]["priorityList"]
        .as_array_mut()
        .unwrap()
        .insert(
            0,
            json!({"action": {"castSpell": {"spellId": {"spellId": id}}}}),
        );
    assert!(check_prepared(&parse(value).unwrap()).is_ok());
}

/// Since the fork's patch 88 Ignite ignores hits on a unit that is not an enemy, so the Goblin
/// Sapper Charge's crit on the Mage no longer panics in Go, and an Ignite build that throws the
/// charge is supported; its accepted Go golden checks the result. See UPSTREAM.md.
#[test]
fn ignite_builds_with_a_goblin_sapper_are_supported() {
    let path = family().join("fire-mage-goblin-sapper.prepared.json");
    let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert!(check_prepared(&parse(value).unwrap()).is_ok());
}

/// Community fix ElliotWood/Forever#622 (252f57aa8), in the reference since 20b551c6b.
/// Without Fingers of Frost the Ice Lance condition reads the missing aura as inactive, so
/// Ice Lance is never cast; before the fix Go dropped the condition and cast it on every
/// global cooldown. The build is supported, and its Go golden casts no Ice Lance.
#[test]
fn community_fix_622_unknown_aura_conditions_read_as_inactive() {
    let bytes = fs::read(family().join("frost-no-fingers.prepared.json")).unwrap();
    let prepared: PreparedV2 = serde_json::from_slice(&bytes).unwrap();
    assert!(!prepared.player.talents.contains_key("fingers_of_frost"));
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
}

/// Without Missile Barrage the Arcane Missiles rule names a missing aura behind an
/// `auraIsKnown` guard, which prunes the rule, so the build is supported; its Go golden
/// casts no Arcane Missiles.
#[test]
fn community_fix_622_guarded_conditions_are_supported() {
    let bytes = fs::read(family().join("reference-no-missile-barrage.prepared.json")).unwrap();
    let prepared: PreparedV2 = serde_json::from_slice(&bytes).unwrap();
    assert!(!prepared.player.talents.contains_key("missile_barrage"));
    assert_eq!(prepared_coverage(&prepared), Vec::<String>::new());
}

/// The comparable view of a Go `RaidSimResult`, matching tools/prepared_v2.py
/// `comparable`: every field but timing and the log, identified lists keyed by ID with an
/// occurrence number for repeats, empty containers dropped.
fn comparable(result: &Value) -> Value {
    fn key(id: &Value) -> String {
        let map: std::collections::BTreeMap<String, Value> =
            serde_json::from_value(id.clone()).unwrap();
        serde_json::to_string(&map)
            .unwrap()
            .replace(':', ": ")
            .replace(',', ", ")
    }
    fn view(value: &Value) -> Value {
        match value {
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), view(v)))
                    .filter(|(_, v)| !is_empty(v))
                    .collect(),
            ),
            Value::Array(items)
                if !items.is_empty() && items.iter().all(|item| item.get("id").is_some()) =>
            {
                let mut keyed = serde_json::Map::new();
                let mut seen = std::collections::BTreeMap::<String, usize>::new();
                for item in items {
                    let base = key(&item["id"]);
                    let count = seen.entry(base.clone()).or_default();
                    *count += 1;
                    let name = if *count == 1 {
                        base
                    } else {
                        format!("{base} #{count}")
                    };
                    let mut rest = item.as_object().unwrap().clone();
                    rest.remove("id");
                    keyed.insert(name, view(&Value::Object(rest)));
                }
                Value::Object(keyed)
            }
            Value::Array(items) => Value::Array(items.iter().map(view).collect()),
            other => other.clone(),
        }
    }
    fn is_empty(value: &Value) -> bool {
        matches!(value, Value::Object(map) if map.is_empty())
            || matches!(value, Value::Array(items) if items.is_empty())
    }
    let mut result = result.as_object().unwrap().clone();
    result.remove("elapsedNs");
    result.remove("logs");
    view(&Value::Object(result))
}

/// The mean beside a standard deviation, matching tools/prepared_v2.py `stdev_mean`.
/// Both engines take sqrt(sumSq/n - mean^2), which cancels when the samples are nearly
/// equal, and Go may fuse the subtraction, so deviations compare as variances.
fn stdev_mean(
    key: &str,
    go: &serde_json::Map<String, Value>,
    rust: &serde_json::Map<String, Value>,
) -> Option<f64> {
    let mean_key = match key.strip_suffix("Stdev") {
        Some(stem) => format!("{stem}Avg"),
        None if key == "stdev" => "avg".into(),
        None => return None,
    };
    let numbers = [go, rust]
        .iter()
        .all(|side| side.get(&mean_key).is_some_and(Value::is_number));
    numbers.then(|| go[&mean_key].as_f64().unwrap())
}

/// A reported deviation as a number: protojson omits a zero, and writes "NaN" when cancellation
/// leaves sqrt(sumSq/n - mean^2) a negative residue, whose variance is zero.
fn deviation(value: Option<&Value>) -> Option<f64> {
    match value {
        None => Some(0.0),
        Some(Value::String(text)) if text == "NaN" => Some(0.0),
        Some(value) => value.as_f64(),
    }
}

/// Integers exactly; floats within 1e-9 relative, the FMA and summation-order allowance
/// documented in docs/prepared-v2.md.
fn differences(go: &Value, rust: &Value, path: &str, out: &mut Vec<String>) {
    match (go, rust) {
        (Value::Object(a), Value::Object(b)) => {
            let keys: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
            for key in keys {
                let (x, y) = (a.get(key), b.get(key));
                let variances = stdev_mean(key, a, b).zip(deviation(x).zip(deviation(y)));
                match (x, y) {
                    _ if variances.is_some() => {
                        let (mean, (x, y)) = variances.unwrap();
                        if (x * x - y * y).abs() > 1e-9 * (mean * mean).max(1.0) {
                            out.push(format!("{path}/{key}: Go {x}, Rust {y}"));
                        }
                    }
                    (Some(x), Some(y)) => differences(x, y, &format!("{path}/{key}"), out),
                    _ => out.push(format!("{path}/{key}: present on one side only")),
                }
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (index, (x, y)) in a.iter().zip(b).enumerate() {
                differences(x, y, &format!("{path}/{index}"), out);
            }
        }
        (Value::Number(a), Value::Number(b)) => {
            let (x, y) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            let integral = a.is_i64() || a.is_u64();
            let tolerance = if integral && (b.is_i64() || b.is_u64()) {
                0.0
            } else {
                1e-9 * x.abs().max(y.abs()).max(1.0)
            };
            if (x - y).abs() > tolerance {
                out.push(format!("{path}: Go {x}, Rust {y}"));
            }
        }
        (a, b) if a == b => {}
        (a, b) => out.push(format!("{path}: Go {a}, Rust {b}")),
    }
}

#[derive(Deserialize)]
struct Golden {
    id: String,
    prepared: String,
    go_result: Option<GoldenFile>,
    go_log: Option<GoldenFile>,
}

#[derive(Deserialize)]
struct GoldenFile {
    file: String,
}

#[derive(Deserialize)]
struct GoldenManifest {
    cases: Vec<Golden>,
}

/// Runs one case and compares it with its Go golden: the result's fields and, when the case
/// keeps one, the first-fight log. An `Err` names the case and says what differs.
fn check_golden(case: &Golden, golden: &GoldenFile) -> Result<(), String> {
    let read = |name: &str| fs::read(family().join(name)).map_err(|err| format!("{name}: {err}"));
    let prepared: PreparedV2 = serde_json::from_slice(&read(&case.prepared)?)
        .map_err(|err| format!("{}: {err}", case.prepared))?;
    let report = forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic)
        .map_err(|err| format!("{}: simulation failed: {err}", case.id))?;
    let expected: Value = serde_json::from_slice(&read(&golden.file)?)
        .map_err(|err| format!("{}: {err}", golden.file))?;
    let mut found = Vec::new();
    differences(&expected, &comparable(&report.result), "", &mut found);
    if !found.is_empty() {
        return Err(format!("{}:\n{}", case.id, found.join("\n")));
    }
    if let Some(log) = &case.go_log {
        let expected =
            String::from_utf8(read(&log.file)?).map_err(|err| format!("{}: {err}", log.file))?;
        let actual = report.result["logs"]
            .as_str()
            .ok_or_else(|| format!("{}: the result has no log", case.id))?;
        for (line, (go, rust)) in expected.lines().zip(actual.lines()).enumerate() {
            if go != rust {
                return Err(format!(
                    "{} log line {}:\n  Go:   {go}\n  Rust: {rust}",
                    case.id,
                    line + 1
                ));
            }
        }
        let (go, rust) = (expected.lines().count(), actual.lines().count());
        if go != rust {
            return Err(format!(
                "{} log length: Go {go} lines, Rust {rust}",
                case.id
            ));
        }
    }
    Ok(())
}

/// The zero-based shard and the shard count `GOLDEN_SHARD=i/n` names, or the one shard of
/// every case when it is unset. A malformed value fails the test rather than checking nothing.
fn golden_shard() -> (usize, usize) {
    let Ok(value) = std::env::var("GOLDEN_SHARD") else {
        return (0, 1);
    };
    let parsed = value
        .split_once('/')
        .and_then(|(i, n)| Some((i.parse::<usize>().ok()?, n.parse::<usize>().ok()?)))
        .filter(|&(i, n)| 1 <= i && i <= n);
    let (i, n) =
        parsed.unwrap_or_else(|| panic!("GOLDEN_SHARD={value:?} is not i/n with 1 <= i <= n"));
    (i - 1, n)
}

/// Every accepted case with a golden is simulated and compared, spread over the machine's
/// cores, or over `RUST_TEST_THREADS` workers when it is set. A case's outcome depends only on its own input, so the order the threads take them
/// in changes nothing; failures are reported together, in manifest order, each by case id.
/// `GOLDEN_SHARD=i/n` (1 <= i <= n) keeps only the cases whose manifest index is i - 1 modulo
/// n, so n CI jobs together check every case; unset, the test checks them all.
#[test]
fn supported_cases_match_pinned_go_goldens() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let manifest: GoldenManifest =
        serde_json::from_slice(&fs::read(family().join("manifest.json")).unwrap()).unwrap();
    let cases: Vec<(&Golden, &GoldenFile)> = manifest
        .cases
        .iter()
        .filter_map(|case| case.go_result.as_ref().map(|golden| (case, golden)))
        .collect();
    assert!(cases.len() >= 5);
    let (shard, shards) = golden_shard();
    let cases: Vec<(&Golden, &GoldenFile)> = cases
        .into_iter()
        .enumerate()
        .filter(|(index, _)| index % shards == shard)
        .map(|(_, case)| case)
        .collect();
    assert!(
        !cases.is_empty(),
        "shard {}/{shards} has no case",
        shard + 1
    );
    let workers = std::env::var("RUST_TEST_THREADS")
        .ok()
        .and_then(|count| count.parse::<usize>().ok())
        .filter(|&count| count > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |count| count.get()));
    let next = AtomicUsize::new(0);
    let outcomes: Vec<std::sync::Mutex<Option<Result<(), String>>>> =
        cases.iter().map(|_| Default::default()).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers.min(cases.len()) {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(&(case, golden)) = cases.get(index) else {
                    break;
                };
                // A panic in the engine names its case instead of ending the whole run.
                let outcome = std::panic::catch_unwind(|| check_golden(case, golden))
                    .unwrap_or_else(|panic| {
                        let message = panic
                            .downcast_ref::<String>()
                            .map(String::as_str)
                            .or_else(|| panic.downcast_ref::<&str>().copied())
                            .unwrap_or("no message");
                        Err(format!("{}: panicked: {message}", case.id))
                    });
                *outcomes[index].lock().unwrap() = Some(outcome);
            });
        }
    });
    let failures: Vec<String> = outcomes
        .into_iter()
        .map(|outcome| outcome.into_inner().unwrap().expect("every case ran"))
        .filter_map(Result::err)
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} cases differ from their Go goldens:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

/// A damage on-use item runs only on outcome appliers the runtime knows: Linken's Boomerang's
/// melee table, renamed to one it does not, leaves its cooldown without a behavior.
#[test]
fn damage_on_use_needs_a_known_outcome() {
    let path = family().join("arcane-mage-linkens-boomerang.prepared.json");
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert!(check_prepared(&parse(value.clone()).unwrap()).is_ok());
    for effect in value["effects"].as_array_mut().unwrap() {
        if effect["kind"] == "damage_on_use" {
            assert_eq!(effect["direct"]["outcome"], "melee_special_hit_and_crit");
            effect["direct"]["outcome"] = json!("ranged_hit_and_crit");
        }
    }
    assert!(reasons(value).contains(&"rotation reaches item 11905 without a known behavior".into()));
}

/// Each refusal carries a stable code a worker counts fallbacks by: the shared gate's, the
/// exporter's and the rotation parser's, and the class and several target limits of the
/// refused fixtures.
#[test]
fn refusals_carry_stable_codes() {
    let codes = |value: Value| -> Vec<&'static str> {
        forever_engine::prepared_refusals(&parse(value).unwrap())
            .into_iter()
            .map(|refusal| refusal.code)
            .collect()
    };
    let mut unrepresented = reference_json();
    unrepresented["unrepresented"] = json!(["target auto attacks are unsupported"]);
    assert_eq!(codes(unrepresented), ["exporter_unrepresented"]);

    let mut rotation = reference_json();
    rotation["player"]["rotation"]["priorityList"][3]["action"]["condition"] =
        json!({"notAValue": {"spellId": {"spellId": 12579}}});
    assert_eq!(codes(rotation), ["rotation_unsupported"]);

    let mut listener = reference_json();
    listener["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["aura"] != "Parry Haste");
    assert_eq!(codes(listener), ["aura_listener_unclaimed"]);

    let mut spell = reference_json();
    spell["player"]["rotation"]["priorityList"][5]["action"]["castSpell"]["spellId"] =
        json!({"spellId": 10202});
    spell["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != "arcane_explosion");
    assert_eq!(codes(spell), ["unknown_spell"]);

    let path = family().join("production-marksmanship-hunter.prepared.json");
    let mut hunter: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    for effect in hunter["effects"].as_array_mut().unwrap() {
        if effect["kind"] == "serpent_sting" {
            effect["tick_outcome"] = json!("magic_hit");
        }
    }
    assert_eq!(codes(hunter), ["class_limit"]);

    // A tank's fight against several targets is refused for a class whose hit-taken
    // behavior has not been checked against every copy of the boss swinging at it.
    let path = family().join("combat-riposte-tank.prepared.json");
    let mut unchecked: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    unchecked["encounter"]["target_count"] = json!(3);
    assert_eq!(codes(unchecked), ["several_targets_unsupported"]);
}

/// Every code is listed once, and the contract documentation names each.
#[test]
fn refusal_codes_are_unique_and_documented() {
    let docs =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/prepared-v2.md"))
            .unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for (code, _) in forever_engine::REFUSAL_CODES {
        assert!(seen.insert(code), "{code} is listed twice");
        assert!(
            docs.contains(&format!("`{code}`")),
            "docs/prepared-v2.md lacks {code}"
        );
    }
}

/// Every `allValues` list in a comparable result, by path.
fn all_values(value: &Value, path: &str, out: &mut Vec<(String, Vec<f64>)>) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                if key == "allValues" {
                    let values = item.as_array().unwrap().iter();
                    out.push((path.into(), values.map(|v| v.as_f64().unwrap()).collect()));
                } else {
                    all_values(item, &format!("{path}/{key}"), out);
                }
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                all_values(item, &format!("{path}/{index}"), out);
            }
        }
        _ => {}
    }
}

/// Under Go `saveAllValues` every distribution reports each iteration's value in iteration
/// order, and stat weights subtract them pairwise, so they must match Go bit for bit, not
/// within the golden tolerance. The cases cover a caster with mana, a pet with labeled random
/// streams, a tank's threat and damage taken, and a Warlock's demon with labeled streams.
#[test]
fn saved_iteration_values_match_go_exactly() {
    let cases = [
        "all-values-frost-mage",
        "all-values-beast-mastery-hunter-labeled",
        "all-values-protection-warrior",
        "all-values-affliction-warlock-labeled",
    ];
    for case in cases {
        let read = |name: String| fs::read(family().join(name)).unwrap();
        let prepared: PreparedV2 =
            serde_json::from_slice(&read(format!("{case}.prepared.json"))).unwrap();
        assert!(prepared.sim.save_all_values, "{case}");
        let iterations = prepared.sim.iterations as usize;
        let report = forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
        let go: Value = serde_json::from_slice(&read(format!("{case}.go-result.json"))).unwrap();
        let (mut expected, mut actual) = (Vec::new(), Vec::new());
        all_values(&go, "", &mut expected);
        all_values(&comparable(&report.result), "", &mut actual);
        let paths = |lists: &[(String, Vec<f64>)]| -> Vec<String> {
            lists.iter().map(|(path, _)| path.clone()).collect()
        };
        assert_eq!(paths(&expected), paths(&actual), "{case}: distributions");
        for path in [
            "/raidMetrics/dps",
            "/raidMetrics/parties/0/players/0/threat",
        ] {
            assert!(
                paths(&expected).iter().any(|p| p == path),
                "{case}: Go lacks {path}"
            );
        }
        for ((path, go), (_, rust)) in expected.iter().zip(&actual) {
            assert_eq!(go.len(), iterations, "{case}{path}");
            let differs = go
                .iter()
                .zip(rust)
                .position(|(a, b)| a.to_bits() != b.to_bits());
            if let Some(index) = differs {
                panic!(
                    "{case}{path} iteration {index}: Go {}, Rust {}",
                    go[index], rust[index]
                );
            }
            assert_eq!(go.len(), rust.len(), "{case}{path}");
        }
    }
}

/// Without `saveAllValues` no distribution reports per-iteration values, as in Go.
#[test]
fn iteration_values_are_reported_only_on_request() {
    let path = family().join("all-values-frost-mage.prepared.json");
    let mut prepared: PreparedV2 = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    prepared.sim.save_all_values = false;
    let report = forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
    let mut found = Vec::new();
    all_values(&report.result, "", &mut found);
    assert!(found.is_empty(), "{found:?}");
}
