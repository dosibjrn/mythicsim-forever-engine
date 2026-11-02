//! Priest prepared v2 regressions. The Go goldens of every accepted Priest case, including
//! the production Shadow Priest, are compared with the rest of the fixture family in
//! `mage/prepared_v2.rs`; these tests cover what the gate rejects and what the runtime
//! reports for the inert and the summoned Shadowfiend.

mod prepare;

use forever_engine::{
    check_prepared, contracts::prepared_v2::PreparedV2, simulate_prepared, PreparedError,
};
use serde_json::{json, Value};
use std::{fs, path::Path};

fn production() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/production-shadow-priest.prepared.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn parse(value: Value) -> PreparedV2 {
    serde_json::from_value(value).unwrap()
}

fn reasons(value: Value) -> Vec<String> {
    match check_prepared(&parse(value)) {
        Err(PreparedError::Unsupported(reasons)) => reasons,
        other => panic!("expected unsupported, got {other:?}"),
    }
}

/// The rotation item holding an action, by its operator.
fn item<'a>(value: &'a mut Value, action: &str) -> &'a mut Value {
    value["player"]["rotation"]["priorityList"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["action"].get(action).is_some())
        .unwrap()
}

#[test]
fn production_shadow_priest_is_supported() {
    assert!(check_prepared(&parse(production())).is_ok());
}

#[test]
fn inert_shadowfiend_is_dismissed_logged_and_reported() {
    let mut value = production();
    value["sim"]["iterations"] = json!(3);
    let report = simulate_prepared(&parse(value)).unwrap();
    let player = &report.result["raidMetrics"]["parties"][0]["players"][0];
    let pet = &player["pets"][0];
    assert_eq!(pet["name"], "Shadowfiend");
    assert_eq!(pet["unitIndex"], 2);
    assert_eq!(pet["auras"][0]["aggregatorData"]["n"], 3);
    // Every action lists the target, the player and the pet.
    for action in player["actions"].as_array().unwrap() {
        assert_eq!(action["targets"].as_array().unwrap().len(), 3);
    }
    // The party's Mana Spring Totem reaches the summoned pet (Go patch 131): its aura is up from
    // the reset to the fight's end, and its exclusive effect holds its category as long.
    let mana_spring = pet["auras"]
        .as_array()
        .unwrap()
        .iter()
        .find(|aura| aura["id"]["spellId"] == 10494)
        .unwrap();
    let effect = &mana_spring["effects"][0];
    assert_eq!(effect["category"], "ManaSpringTotemMP5Add");
    let (aura_uptime, effect_uptime) = (
        mana_spring["uptimeSecondsAvg"].as_f64().unwrap(),
        effect["uptimeSecondsAvg"].as_f64().unwrap(),
    );
    assert!(
        aura_uptime > 0.0 && (aura_uptime - effect_uptime).abs() < 1e-9,
        "{mana_spring}"
    );
    let logs = report.result["logs"].as_str().unwrap();
    let pet_lines: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("- Shadowfiend]"))
        .collect();
    assert_eq!(pet_lines.len(), 5, "{pet_lines:?}");
    assert!(pet_lines[0].ends_with("Aura gained: {SpellID: 10494, Tag: -1}"));
    assert!(pet_lines[1].ends_with("Pet dismissed"));
    assert!(pet_lines[3].ends_with("Aura faded: {SpellID: 10494, Tag: -1}"));
    assert!(pet_lines[4].ends_with("No pet summoned"));
}

#[test]
fn a_pet_that_may_act_is_rejected() {
    let mut value = production();
    let effects = value["effects"].as_array_mut().unwrap();
    effects.retain(|effect| effect["kind"] != "inert_pet");
    value["unrepresented"] = json!(["pets are unsupported"]);
    assert_eq!(
        reasons(value),
        ["unrepresented by the exporter: pets are unsupported"]
    );
}

/// An interrupt condition naming an aura the character lacks reads it as inactive
/// (community fix #622), so the channel compiles as Go's does.
#[test]
fn interrupt_conditions_read_missing_auras_as_inactive() {
    let mut value = production();
    let channel = &mut item(&mut value, "channelSpell")["action"]["channelSpell"];
    channel["interruptIf"] = json!({"and": {"vals": [
        channel["interruptIf"].clone(),
        {"auraIsActive": {"auraId": {"spellId": 44404}}},
    ]}});
    assert!(check_prepared(&parse(value)).is_ok());
}

#[test]
fn unsupported_sequence_steps_and_channel_targets_are_named() {
    let mut value = production();
    let sequence = &mut item(&mut value, "strictSequence")["action"]["strictSequence"];
    sequence["actions"][0] = json!({"wait": {"duration": {"const": {"val": "1s"}}}});
    assert!(reasons(value)
        .contains(&"rotation item 6: strictSequence action wait is unsupported".into()));

    // A channel is cast on the current target; a target that names no unit drops it.
    let mut value = production();
    let channel = &mut item(&mut value, "channelSpell")["action"]["channelSpell"];
    channel["target"] = json!({"type": "Self"});
    let refused = reasons(value);
    assert!(
        refused.iter().any(
            |reason| reason.starts_with("rotation item 9: channelSpell of spell")
                && reason.ends_with("on a unit other than the current target is unsupported")
        ),
        "{refused:?}"
    );
    let mut value = production();
    let channel = &mut item(&mut value, "channelSpell")["action"]["channelSpell"];
    channel["target"] = json!({"type": "AllTargets"});
    assert!(check_prepared(&parse(value)).is_ok());
}

#[test]
fn shadow_weaving_outside_spell_hits_is_rejected() {
    let mut value = production();
    let weaving = value["effects"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|effect| effect["kind"] == "shadow_weaving")
        .unwrap();
    weaving["callbacks"] = json!(["on_periodic_damage_dealt"]);
    assert!(reasons(value.clone())
        .contains(&"Shadow Weaving listens to [\"on_periodic_damage_dealt\"]".to_string()));
    assert!(crate::refusal_codes(value).contains(&(
        "class_limit",
        "Shadow Weaving listens to [\"on_periodic_damage_dealt\"]".into()
    )));
}

fn smite() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/mage/prepared-v2/production-smite-priest.prepared.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn effect<'a>(value: &'a mut Value, kind: &str) -> &'a mut Value {
    value["effects"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|effect| effect["kind"] == kind)
        .unwrap()
}

#[test]
fn production_smite_priest_is_supported() {
    assert!(check_prepared(&parse(smite())).is_ok());
}

#[test]
fn searing_light_outside_periodic_damage_is_rejected() {
    let mut value = smite();
    let searing = effect(&mut value, "searing_light");
    searing["callbacks"] = json!(["on_spell_hit_dealt"]);
    searing["outcome"] = json!(["Crit"]);
    let reasons = reasons(value);
    assert!(reasons.contains(&"Searing Light listens to [\"on_spell_hit_dealt\"]".to_string()));
    assert!(reasons.contains(&"Searing Light procs on [\"Crit\"]".to_string()));
}

#[test]
fn power_in_light_needs_a_burning_dot() {
    // With no dot named, the modifier never applies and the build deals less.
    let mut value = smite();
    value["sim"]["iterations"] = json!(20);
    value["sim"]["debug_first_iteration"] = json!(false);
    let mut unlit = value.clone();
    effect(&mut unlit, "power_in_light")["dot_spells"] = json!([]);
    let dps = |value: Value| {
        simulate_prepared(&parse(value)).unwrap().result["raidMetrics"]["dps"]["avg"]
            .as_f64()
            .unwrap()
    };
    assert!(dps(value) > dps(unlit));
}

fn accepted(case: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/mage/prepared-v2/{case}.prepared.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn holy_nova_reads_the_live_healing_power() {
    // Talisman of Ephemeral Power raises healing power for a while, and the heal follows it.
    let mut value = accepted("smite-priest-holy-nova-ephemeral-power");
    value["sim"]["iterations"] = json!(1);
    let report = simulate_prepared(&parse(value)).unwrap();
    let logs = report.result["logs"].as_str().unwrap();
    let mut powers: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("{SpellID: 27805} [DEBUG] HealingPower: "))
        .map(|line| line.split("HealingPower: ").nth(1).unwrap())
        .map(|rest| rest.split(',').next().unwrap())
        .collect();
    powers.dedup();
    assert!(powers.len() > 1, "{powers:?}");
}

/// Holy Nova rolls, resolves and deals a hit on every target in unit index order, then casts
/// its heal once on the priest, as Go `CalcAndDealAoeDamageWithVariance` does.
#[test]
fn holy_nova_hits_every_target_then_heals_once() {
    for case in [
        "shadow-priest-holy-nova-5-targets",
        "smite-priest-holy-nova-5-targets",
    ] {
        let mut value = accepted(case);
        value["sim"]["iterations"] = json!(1);
        let report = simulate_prepared(&parse(value)).unwrap();
        let logs = report.result["logs"].as_str().unwrap();
        let casts = logs.matches("Completed cast {SpellID: 27801}").count();
        assert!(casts > 1, "{case}: {casts} casts");
        assert_eq!(
            logs.matches("Completed cast {SpellID: 27805}").count(),
            casts
        );
        for target in 1..=5 {
            // One outcome line for each cast, whether it hit, crit or missed.
            let prefix = format!("[Target {target}] {{SpellID: 27801}} ");
            let outcomes = logs
                .lines()
                .filter(|line| line.contains(&prefix) && !line.contains("[DEBUG]"))
                .count();
            assert_eq!(outcomes, casts, "{case}: Target {target}");
        }
        // The hits come in unit index order, ahead of the heal.
        let first = logs.find("Completed cast {SpellID: 27801}").unwrap();
        let after = &logs[first..];
        let order: Vec<usize> = (1..=5)
            .map(|target| {
                after
                    .find(&format!("[Target {target}] {{SpellID: 27801}} [DEBUG]"))
                    .unwrap()
            })
            .collect();
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
        assert!(order[4] < after.find("Completed cast {SpellID: 27805}").unwrap());
    }
}

#[test]
fn shadowfiend_inherits_attack_power_and_restores_mana() {
    let mut value = accepted("shadow-priest-shadowfiend");
    value["sim"]["iterations"] = json!(1);
    let report = simulate_prepared(&parse(value)).unwrap();
    let logs = report.result["logs"].as_str().unwrap();
    assert!(logs
        .lines()
        .any(|line| line.contains("- Shadowfiend]") && line.ends_with("Pet summoned")));
    assert!(logs
        .lines()
        .any(|line| line.contains("Pet inherited stats: {\"AttackPower\": ")));
    assert!(logs
        .lines()
        .any(|line| line.contains("mana from {SpellID: 401988}")));
}

#[test]
fn a_shadowfiend_effect_for_another_pet_is_rejected() {
    let mut value = accepted("shadow-priest-shadowfiend");
    effect(&mut value, "shadowfiend")["pet"] = json!("another pet");
    let reasons = reasons(value);
    assert!(
        reasons
            .iter()
            .any(|reason| reason.starts_with("pet ") && reason.ends_with("has no behavior")),
        "{reasons:?}"
    );
}

fn shadow_against(targets: u32) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "fixtures/mage/prepared-v2/production-shadow-priest-{targets}-targets.prepared.json"
    ));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// The application's multidot puts Shadow Word: Pain on every target, each with its own dot,
/// while Mind Flay stays on the first.
#[test]
fn multidot_keeps_shadow_word_pain_on_every_target() {
    let mut value = shadow_against(5);
    value["sim"]["iterations"] = json!(1);
    value["sim"]["debug_first_iteration"] = json!(true);
    let report = simulate_prepared(&parse(value)).unwrap();
    let logs = report.result["logs"].as_str().unwrap();
    for target in 1..=5 {
        let ticks = logs
            .lines()
            .filter(|line| line.contains(&format!("[Target {target}] {{SpellID: 10894}} tick")))
            .count();
        assert!(ticks > 0, "Target {target}: no Shadow Word: Pain ticks");
    }
    assert!(!logs
        .lines()
        .any(|line| line.contains("[Target 2] {SpellID: 18807}")));
}

/// The exporter writes a target count only for two to five targets.
#[test]
fn target_counts_outside_the_contract_are_refused() {
    for count in [1, 6] {
        let mut value = production();
        value["encounter"]["target_count"] = json!(count);
        assert!(reasons(value).contains(&format!(
            "target count {count}: a fight has one target, or from 2 to 5 copies of it"
        )));
    }
}

/// Go pet.go: the Shadowfiend's dismissal logs its stats. At its timeout its permanent auras are
/// still up, so the party's Mana Spring Totem (patch 131) shows in the line; at the fight's end
/// Go's doneIteration has expired them first, so it does not.
#[test]
fn the_shadowfiend_out_at_the_fights_end_logs_its_stats_without_its_party_auras() {
    let mut value = accepted("shadow-priest-shadowfiend");
    value["sim"]["iterations"] = json!(1);
    value["sim"]["debug_first_iteration"] = json!(true);
    // The second summon, five minutes after the first, is still out when the fight ends.
    value["encounter"]["duration_ns"] = json!(305_000_000_000_i64);
    value["encounter"]["duration_variation_ns"] = json!(0);
    let report = simulate_prepared(&parse(value)).unwrap();
    let logs = report.result["logs"].as_str().unwrap();
    let lines: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("- Shadowfiend]"))
        .collect();
    let dismissals: Vec<&str> = lines
        .windows(2)
        .filter(|pair| pair[0].ends_with("Pet dismissed"))
        .map(|pair| pair[1])
        .collect();
    // At the reset, at the first summon's timeout and at the fight's end.
    assert_eq!(dismissals.len(), 3, "{lines:?}");
    assert!(
        dismissals[0].contains("\"MP5\": 30.000"),
        "{}",
        dismissals[0]
    );
    assert!(
        dismissals[1].contains("\"MP5\": 30.000"),
        "{}",
        dismissals[1]
    );
    assert!(!dismissals[2].contains("MP5"), "{}", dismissals[2]);
    let fade = lines
        .iter()
        .rposition(|line| line.ends_with("Aura faded: {SpellID: 10494, Tag: -1}"))
        .unwrap();
    assert!(lines[fade + 1].ends_with("Pet dismissed"), "{lines:?}");
}

/// Mind Flay's target metrics on the production Shadow Priest request, prepared with `rules`.
fn production_mind_flay(rules: forever_engine::Rules) -> (Value, Value) {
    let request = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/mage/prepared-v2/production-shadow-priest.request.json"),
    )
    .unwrap();
    // The production priest is Undead, whose Touch of the Grave differs between the rules since the
    // 27 October racial additions, which would part the rolls; a Night Elf's racials are the same
    // under both.
    let mut request: Value = serde_json::from_slice(&request).unwrap();
    request["raid"]["parties"][0]["players"][0]["race"] = json!("RaceNightElf");
    let request = serde_json::to_vec(&request).unwrap();
    let mut prepared = forever_engine::prepare_json_with(&request, "x", rules).unwrap();
    prepared["sim"]["iterations"] = json!(20);
    let effect = prepared["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == "improved_mind_flay")
        .unwrap()
        .clone();
    let report = simulate_prepared(&parse(prepared)).unwrap();
    let player = &report.result["raidMetrics"]["parties"][0]["players"][0];
    let mind_flay = player["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["id"]["spellId"] == 18807)
        .unwrap();
    (effect, mind_flay["targets"][0].clone())
}

/// Improved Mind Flay compounds, as its Forever tooltip reads: "Each instance of damage of Mind
/// Flay deals 20% more damage than its previous instance of damage", so at two points a channel's
/// ticks deal 1, 1.2 and 1.44 times its tick. Rust only: the reference adds the step per tick (1,
/// 1.2 and 1.4). The rolls are the same under both rules, so the strongest tick, a third tick,
/// is 1.44 / 1.4 of the reference's and the first ticks are unchanged.
#[test]
fn improved_mind_flay_compounds_tick_over_tick() {
    let (forever, forever_flay) = production_mind_flay(forever_engine::Rules::Forever);
    let (go, go_flay) = production_mind_flay(forever_engine::Rules::GoReference);
    assert_eq!(forever["ramp_per_tick"], 0.2);
    assert_eq!(forever["compounding"], true);
    assert_eq!(go["ramp_per_tick"], 0.2);
    assert!(go.get("compounding").is_none(), "{go}");
    for field in ["casts", "ticks", "critTicks", "misses"] {
        assert_eq!(forever_flay[field], go_flay[field], "{field}");
    }
    let range = |flay: &Value, range: &str, end: &str| flay[range][end].as_f64().unwrap();
    for name in ["tickRange", "critTickRange"] {
        let ratio = range(&forever_flay, name, "max") / range(&go_flay, name, "max");
        assert!((ratio - 1.44 / 1.4).abs() < 1e-12, "{name}: {ratio}");
        assert_eq!(
            range(&forever_flay, name, "min"),
            range(&go_flay, name, "min")
        );
    }
}

/// Shadow Word: Pain's resisted ticks and Mind Blast's resisted hits in a report, with the
/// player's damage per second.
fn shadow_resists(value: Value) -> (u64, u64, u64, f64) {
    let report = simulate_prepared(&parse(value)).unwrap();
    let player = &report.result["raidMetrics"]["parties"][0]["players"][0];
    let target = |spell: u64| {
        &player["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|action| action["id"]["spellId"] == spell)
            .unwrap()["targets"][0]
    };
    let count = |value: &Value, key: &str| value[key].as_u64().unwrap_or(0);
    let pain = target(10894);
    let blast = target(10947);
    (
        count(pain, "resistedTicks") + count(pain, "resistedCritTicks"),
        count(pain, "ticks") + count(pain, "critTicks"),
        count(blast, "resistedHits") + count(blast, "resistedCrits"),
        player["dps"]["avg"].as_f64().unwrap(),
    )
}

/// Forever's beta logs (foreverlogs.gg, October 2026) show no partial resists from a target's
/// level: about 1,000 Shadow Word: Pain, Corruption, Serpent Sting and other dot ticks and about
/// 100 direct Wrath, Shadow Bolt, Mind Blast and Arcane Shot hits on Razorfen Kraul and Scarlet
/// Monastery bosses, which miss level 30 casters' Shadow Bolts a third of the time, landed whole.
/// Rust only: the reference partially resists them by 2% a level on average.
#[test]
fn a_targets_level_adds_no_partial_resists_in_forever() {
    let mut reference = production();
    reference["sim"]["iterations"] = json!(100);
    assert!(reference["target"].get("no_level_partial_resist").is_none());
    let mut forever = reference.clone();
    forever["target"]["no_level_partial_resist"] = json!(true);

    let (go_ticks, ticks, go_hits, go_dps) = shadow_resists(reference);
    assert!(
        go_ticks > ticks / 20,
        "{go_ticks} of {ticks} ticks resisted"
    );
    assert!(go_hits > 0);
    let (forever_ticks, _, forever_hits, forever_dps) = shadow_resists(forever.clone());
    assert_eq!((forever_ticks, forever_hits), (0, 0));
    assert!(
        forever_dps > go_dps * 1.02,
        "{forever_dps} against {go_dps}"
    );

    // Resistance past the caster's Spell Piercing still partially resists.
    forever["target"]["stats"]["ShadowResistance"] = json!(150);
    let (resisted, ticks, _, _) = shadow_resists(forever);
    assert!(
        resisted > ticks / 20,
        "{resisted} of {ticks} ticks resisted"
    );
}

/// Forever preparation marks the target; the reference's (`Rules::GoReference`) does not.
#[test]
fn forever_preparation_drops_the_level_partial_resist() {
    let request = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/mage/prepared-v2/production-shadow-priest.request.json"),
    )
    .unwrap();
    let forever = forever_engine::prepare_json(&request, "x").unwrap();
    assert_eq!(forever["target"]["no_level_partial_resist"], json!(true));
    let go = forever_engine::prepare_json_with(&request, "x", forever_engine::Rules::GoReference)
        .unwrap();
    assert!(go["target"].get("no_level_partial_resist").is_none());
}
