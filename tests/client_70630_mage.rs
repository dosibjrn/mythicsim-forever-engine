//! Client 1.60.1.70630's Mage changes that only Forever's rules prepare: Winter's Chill. The
//! client's other Mage rows of that build (Ice Barrier and Frigid Power, Frost Warding's absorb,
//! Frost Blast's 0.271 coefficient, Arcane Wrath's cooldown, Missile Barrage) were already in the
//! imported data through hotfix push 112753, so they are not new here.

use forever_engine::{
    contracts::prepared_v2::PreparedV2, prepare_json_with, simulate_prepared, Rules,
};
use serde_json::{json, Value};
use std::{fs, path::Path};

const FROSTBOLT: i64 = 25304;
/// The Frostbolt a Forever mage casts for 25304: rank X, as rank XI is an Ahn'Qiraj 20 book.
const FROSTBOLT_KNOWN: i64 = 10181;
const CONE_OF_COLD: i64 = 10161;
const ICE_LANCE: i64 = 1240047;
const WINTERS_CHILL: i64 = 12579;
const FINGERS_OF_FROST: i64 = 400669;
const FIREBALL: i64 = 25306;

fn request(name: &str) -> Value {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/mage/prepared-v2"
    ))
    .join(format!("{name}.request.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn cast(spell: i64, condition: Option<Value>) -> Value {
    let mut action = json!({"castSpell": {"spellId": {"spellId": spell}}});
    if let Some(condition) = condition {
        action["castSpell"]["condition"] = condition.clone();
        return json!({"action": {"castSpell": {"spellId": {"spellId": spell}}, "condition": condition}});
    }
    json!({ "action": action })
}

fn stacks_at_least(spell: i64, stacks: u32) -> Value {
    json!({"cmp": {"lhs": {"auraNumStacks": {"auraId": {"spellId": spell}}}, "op": "OpGe",
        "rhs": {"const": {"val": stacks.to_string()}}}})
}

fn aura_active(spell: i64) -> Value {
    json!({"auraIsActive": {"auraId": {"spellId": spell}}})
}

/// The first iteration's log lines of the improved Cone of Cold fixture (a Frost Mage with Winter's
/// Chill and Fingers of Frost) on `rotation`, a fight of `seconds`.
fn log(rules: Rules, rotation: Vec<Value>, seconds: f64) -> (Value, Vec<String>) {
    let mut request = request("frost-cone-of-cold-improved");
    request["simOptions"]["debugFirstIteration"] = true.into();
    request["simOptions"]["iterations"] = 1.into();
    request["encounter"]["duration"] = seconds.into();
    request["encounter"]["durationVariation"] = 0.into();
    let player = &mut request["raid"]["parties"][0]["players"][0];
    if rules == Rules::Forever {
        // The fixture's talents in client 1.60.1.70707's order: Arcane Focus's points on
        // Improved Channeling, which takes its square.
        player["talentsString"] = json!("050205103-3-053500000101331251");
    }
    player["rotation"] = json!({"type": "TypeAPL", "priorityList": rotation});
    // Cone of Cold is a point blank spell.
    player["distanceFromTarget"] = 5.into();
    let prepared = prepare_json_with(request.to_string().as_bytes(), "rust", rules).unwrap();
    let typed: PreparedV2 = serde_json::from_value(prepared.clone()).unwrap();
    let report = simulate_prepared(&typed).unwrap();
    let logs = report.result["logs"].as_str().unwrap().to_string();
    (prepared, logs.lines().map(str::to_string).collect())
}

fn time(line: &str) -> f64 {
    line[1..line.find(']').unwrap()].parse().unwrap()
}

fn winters_chill_effect(prepared: &Value) -> Value {
    prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == "winters_chill")
        .expect("winters_chill")
        .clone()
}

/// The base damage of the first hit of `spell` (the log's DEBUG line before it) and the spell
/// power it was worked out with.
fn first_base_damage(lines: &[String], spell: i64) -> (f64, f64) {
    let tag = format!("{{SpellID: {spell}}} [DEBUG]");
    let line = lines
        .iter()
        .find(|line| line.contains(&tag))
        .unwrap_or_else(|| panic!("no hit of {spell}"));
    let number = |key: &str| -> f64 {
        let from = line.find(key).unwrap() + key.len();
        let rest = &line[from..];
        rest[..rest.find(',').unwrap()].trim().parse().unwrap()
    };
    (number("BaseDamage:"), number("SP:"))
}

/// The stack counts Winter's Chill went through, with the time of each change.
fn stack_changes(lines: &[String]) -> Vec<(f64, i32, i32)> {
    let tag = format!("{{SpellID: {WINTERS_CHILL}}} stacks: ");
    lines
        .iter()
        .filter_map(|line| {
            let rest = &line[line.find(&tag)? + tag.len()..];
            let (from, to) = rest.split_once(" --> ")?;
            Some((
                time(line),
                from.trim().parse().ok()?,
                to.trim().parse().ok()?,
            ))
        })
        .collect()
}

fn cone_rotation() -> Vec<Value> {
    vec![
        cast(CONE_OF_COLD, Some(stacks_at_least(WINTERS_CHILL, 3))),
        cast(FROSTBOLT, None),
    ]
}

#[test]
fn forever_prepares_the_next_cone_of_cold_rule_and_the_reference_does_not() {
    let (forever, _) = log(Rules::Forever, cone_rotation(), 30.0);
    let effect = winters_chill_effect(&forever);
    assert_eq!(effect["cone_of_cold_consumes"], true);
    let share = effect["spell_power_per_stack"].as_f64().unwrap();
    assert!((share - 0.07).abs() < 1e-12, "{share}");
    // The 5% per stack of 70425 (12579 effect 1, 70630 base 5) is 20% since hotfix push 113531
    // (SpellEffect 688253).
    assert!((effect["damage_per_stack"].as_f64().unwrap() - 0.2).abs() < 1e-12);

    let (go, _) = log(Rules::GoReference, cone_rotation(), 30.0);
    let effect = winters_chill_effect(&go);
    assert!(effect.get("cone_of_cold_consumes").is_none());
    assert!(effect.get("spell_power_per_stack").is_none());
}

#[test]
fn a_cone_of_cold_uses_the_stacks_up_and_adds_seven_percent_of_spell_damage_each() {
    // Only Frostbolts land before the first Cone of Cold, so both rules stack alike up to it.
    let (_, forever) = log(Rules::Forever, cone_rotation(), 120.0);
    let (_, go) = log(Rules::GoReference, cone_rotation(), 120.0);

    let cone = |lines: &[String]| {
        lines
            .iter()
            .find(|line| line.contains(&format!("Casting {{SpellID: {CONE_OF_COLD}}}")))
            .map(|line| time(line))
            .expect("a Cone of Cold is cast")
    };
    let (forever_cone, go_cone) = (cone(&forever), cone(&go));
    assert_eq!(forever_cone, go_cone);

    let stacks = stack_changes(&forever)
        .into_iter()
        .rfind(|(at, ..)| *at < forever_cone)
        .map(|(_, _, to)| to)
        .unwrap();
    assert!(stacks >= 3, "{stacks}");

    // Winter's Chill is gone as the cast completes under Forever, still up under the reference.
    let faded = |lines: &[String]| {
        lines.iter().any(|line| {
            time(line) == forever_cone
                && line.contains(&format!("{{SpellID: {WINTERS_CHILL}}}"))
                && line.contains("faded")
        })
    };
    assert!(
        faded(&forever),
        "Forever keeps Winter's Chill after the Cone of Cold"
    );
    assert!(!faded(&go), "the reference should keep the stacks");

    // The same roll on the same spell damage, so the base damage differs by the extra share.
    let (forever_base, spell_power) = first_base_damage(&forever, CONE_OF_COLD);
    let (go_base, go_spell_power) = first_base_damage(&go, CONE_OF_COLD);
    assert_eq!(spell_power, go_spell_power);
    let expected = 0.07 * f64::from(stacks) * spell_power;
    assert!(
        (forever_base - go_base - expected).abs() < 0.2,
        "{forever_base} vs {go_base} with {stacks} stacks of {spell_power} spell power"
    );
}

#[test]
fn the_stacks_begin_again_after_the_cone_of_cold() {
    let (_, lines) = log(Rules::Forever, cone_rotation(), 120.0);
    let changes = stack_changes(&lines);
    // After the drop to 0 the next stack is the first again.
    let drop = changes.iter().position(|(_, _, to)| *to == 0);
    if let Some(drop) = drop {
        let next = changes[drop + 1..].iter().find(|(_, _, to)| *to > 0);
        assert!(
            next.is_none_or(|(_, from, to)| *from == 0 && *to == 1),
            "{next:?}"
        );
    }
    // A Cone of Cold can only be cast with 3 stacks, so each one fades the aura.
    let casts = lines
        .iter()
        .filter(|line| line.contains(&format!("Casting {{SpellID: {CONE_OF_COLD}}}")))
        .count();
    let fades = lines
        .iter()
        .filter(|line| {
            line.contains(&format!("{{SpellID: {WINTERS_CHILL}}}")) && line.contains("faded")
        })
        .count();
    assert!(casts >= 2, "{casts}");
    assert!(fades >= casts, "{fades} fades for {casts} Cones of Cold");
}

#[test]
fn only_frostbolt_frost_blast_and_an_empowered_ice_lance_apply_winters_chill() {
    // Ice Lance whenever Fingers of Frost is up, Frostbolt below five stacks, Fireball (which
    // applies none) otherwise, so the stacks fade and build again; never a Cone of Cold. Since
    // hotfix push 113531 Ice Lance has an 8 sec cooldown, and Frostbolt alone would hold the
    // stacks at five for the whole fight.
    let rotation = vec![
        cast(ICE_LANCE, Some(aura_active(FINGERS_OF_FROST))),
        cast(
            FROSTBOLT,
            Some(json!({"not": {"val": stacks_at_least(WINTERS_CHILL, 5)}})),
        ),
        cast(FIREBALL, None),
    ];
    let (_, lines) = log(Rules::Forever, rotation, 600.0);
    let gains: Vec<f64> = stack_changes(&lines)
        .into_iter()
        .filter(|(_, from, to)| to > from)
        .map(|(at, ..)| at)
        .collect();
    assert!(gains.len() >= 8, "{} stacks gained", gains.len());

    let at = |at: f64, event: &str, spell: i64| {
        lines.iter().any(|line| {
            time(line) == at
                && line.contains(&format!("{{SpellID: {spell}}}"))
                && line.contains(event)
        })
    };
    let mut lance = 0;
    for gain in &gains {
        let bolt = at(*gain, "Hit for", FROSTBOLT_KNOWN) || at(*gain, "Crit for", FROSTBOLT_KNOWN);
        let empowered_lance = at(*gain, "Completed cast", ICE_LANCE);
        assert!(
            bolt || empowered_lance,
            "a stack at {gain} that no applying spell explains"
        );
        lance += usize::from(empowered_lance && !bolt);
    }
    assert!(
        lance > 0,
        "an empowered Ice Lance never applied Winter's Chill"
    );
}

#[test]
fn the_reference_stacks_from_any_landed_frost_damage() {
    // Ice Lance without Fingers of Frost is no empowered lance: Go counts its landing, Forever does not.
    let rotation = vec![cast(ICE_LANCE, None)];
    let (_, forever) = log(Rules::Forever, rotation.clone(), 600.0);
    let (_, go) = log(Rules::GoReference, rotation, 600.0);
    assert!(stack_changes(&forever).is_empty());
    assert!(!stack_changes(&go).is_empty());
}
