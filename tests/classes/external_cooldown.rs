//! The external Power Infusion any class receives from priests in the raid
//! (`individualBuffs.powerInfusions`, Go's generated external cooldown): what the gate needs to
//! accept it and how its sources take turns. The Go result and first-fight log goldens of every
//! class that receives it are compared with the other supported cases in
//! `mage/prepared_v2.rs`.

use super::refusal_codes;
use forever_engine::{contracts::prepared_v2::PreparedV2, simulate_prepared};
use serde_json::{json, Value};
use std::{fs, path::Path};

fn fixture(case: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/mage/prepared-v2/{case}.prepared.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn without_effect(mut value: Value, kind: &str) -> Value {
    value["effects"]
        .as_array_mut()
        .unwrap()
        .retain(|effect| effect["kind"] != kind);
    value
}

/// The seconds into the first fight at which the cooldown manager cast Power Infusion on the
/// player.
fn casts(mut value: Value, sources: u64) -> Vec<f64> {
    for effect in value["effects"].as_array_mut().unwrap() {
        if effect["kind"] == "external_cooldown" {
            effect["sources"] = json!(sources);
        }
    }
    value["sim"]["iterations"] = json!(1);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    let report = simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
    report.result["logs"]
        .as_str()
        .unwrap()
        .lines()
        .filter(|line| line.ends_with("Major cooldown used: {SpellID: 10060, Tag: -1}"))
        .map(|line| {
            let end = line.find(']').unwrap();
            line[1..end].parse().unwrap()
        })
        .collect()
}

#[test]
fn the_gate_needs_the_cooldown_and_the_aura_effects() {
    let value = fixture("fire-mage-external-power-infusion");
    assert_eq!(refusal_codes(value.clone()), []);
    // The spell is Go's external cooldown; without its effect it has no behavior.
    assert_eq!(
        refusal_codes(without_effect(value.clone(), "external_cooldown")),
        [(
            "unknown_spell",
            "rotation reaches spell 10060 tag -1 without a known behavior".to_string()
        )]
    );
    // The multipliers are the aura's exclusive effects, which only its effect describes, and an
    // external cooldown whose aura no effect describes would change nothing.
    assert_eq!(
        refusal_codes(without_effect(value, "power_infusion")),
        [
            (
                "aura_listener_unclaimed",
                "player aura \"Power Infusions (External)\" bids in Power Infusion's categories \
                 without an effect"
                    .to_string()
            ),
            (
                "aura_listener_unclaimed",
                "player aura \"Power Infusions (External)\" is cast by an external cooldown \
                 without an effect"
                    .to_string()
            )
        ]
    );
}

#[test]
fn the_sources_take_turns_and_wait_out_their_cooldowns() {
    let value = fixture("fire-mage-external-power-infusion");
    for sources in 1..=3 {
        let times = casts(value.clone(), sources);
        // The cooldown is three minutes, so each source casts once in a two minute fight, each
        // after the aura of the one before has ended.
        assert_eq!(times.len() as u64, sources, "{times:?}");
        for pair in times.windows(2) {
            assert!(pair[1] - pair[0] >= 15.0, "{times:?}");
        }
    }
}

#[test]
fn a_source_returns_when_its_cooldown_ends() {
    let times = casts(fixture("fire-mage-external-power-infusion-long-fight"), 2);
    assert!(times.len() >= 4, "{times:?}");
    for (earlier, later) in times.iter().zip(&times[2..]) {
        assert!(later - earlier >= 180.0, "{times:?}");
    }
}

/// The seconds into the first fight at which the cooldown manager cast the external buff with the
/// spell on the player, after the prepared input has been changed.
fn cast_times(mut value: Value, spell: u32) -> Vec<f64> {
    value["sim"]["iterations"] = json!(1);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    let report = simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
    let suffix = format!("Major cooldown used: {{SpellID: {spell}, Tag: -1}}");
    report.result["logs"]
        .as_str()
        .unwrap()
        .lines()
        .filter(|line| line.ends_with(&suffix))
        .map(|line| {
            let end = line.find(']').unwrap();
            line[1..end].parse().unwrap()
        })
        .collect()
}

/// Sets the activation of the external cooldown.
fn with_activation(mut value: Value, activation: Value) -> Value {
    for effect in value["effects"].as_array_mut().unwrap() {
        if effect["kind"] == "external_cooldown" {
            effect["activation"] = activation.clone();
        }
    }
    value
}

#[test]
fn innervate_waits_for_the_mana_threshold() {
    let value = fixture("shadow-priest-external-innervate");
    // Without mana consumables the priest is innervated a while into the fight, at 1000 mana.
    let times = cast_times(value.clone(), 29166);
    assert!(!times.is_empty() && times[0] > 20.0, "{times:?}");
    // With a threshold above any mana the cooldown manager casts it as soon as it looks.
    let times = cast_times(
        with_activation(
            value.clone(),
            json!({"kind": "mana_at_most", "threshold": 1e9}),
        ),
        29166,
    );
    assert!(times[0] < 5.0, "{times:?}");
    // And with one below any it never does.
    let times = cast_times(
        with_activation(value, json!({"kind": "mana_at_most", "threshold": -1.0})),
        29166,
    );
    assert!(times.is_empty(), "{times:?}");
}

#[test]
fn mana_tide_totem_waits_for_its_initial_delay() {
    // 40 seconds into a long fight, and halfway through a short one.
    let times = cast_times(fixture("fire-mage-external-mana-tide"), 1325257);
    assert!(times[0] >= 40.0 && times[0] < 45.0, "{times:?}");
    let times = cast_times(fixture("fire-mage-external-mana-tide-short-fight"), 1325257);
    assert!(times[0] >= 22.5 && times[0] < 27.0, "{times:?}");
    // The delay is the activation's: earlier for a smaller one.
    let times = cast_times(
        with_activation(
            fixture("fire-mage-external-mana-tide"),
            json!({"kind": "not_before", "time_ns": 5_000_000_000_i64}),
        ),
        1325257,
    );
    assert!(times[0] >= 5.0 && times[0] < 10.0, "{times:?}");
}

#[test]
fn the_sources_of_mana_tide_and_innervate_take_turns() {
    for (case, spell, cooldown, aura) in [
        (
            "destruction-warlock-external-mana-tide-2-sources",
            1325257,
            300.0,
            13.0,
        ),
        ("fire-mage-external-innervate-2-sources", 29166, 360.0, 20.0),
    ] {
        let times = cast_times(fixture(case), spell);
        // The second source waits for the aura of the first to end, and each returns after its
        // cooldown: no cast lands inside the aura of the one before.
        assert!(times.len() >= 2, "{case}: {times:?}");
        assert!(times[1] - times[0] >= aura, "{case}: {times:?}");
        if let Some(third) = times.get(2) {
            assert!(third - times[0] >= cooldown, "{case}: {times:?}");
        }
    }
}

/// An external cooldown whose aura no effect describes is refused, as is one Innervate's
/// regeneration or Mana Tide Totem's stat aura does not describe.
#[test]
fn the_gate_needs_the_aura_effects_of_innervate_and_mana_tide() {
    let value = fixture("fire-mage-external-innervate");
    assert_eq!(refusal_codes(value.clone()), []);
    assert_eq!(
        refusal_codes(without_effect(value, "innervate_regen")),
        [(
            "aura_listener_unclaimed",
            "player aura \"Innervates (External)\" is cast by an external cooldown \
             without an effect"
                .to_string()
        )]
    );
    let value = fixture("fire-mage-external-mana-tide");
    assert_eq!(refusal_codes(value.clone()), []);
    assert_eq!(
        refusal_codes(without_effect(value, "stat_auras")),
        [(
            "aura_listener_unclaimed",
            "player aura \"Mana Tide Totem (External)\" is cast by an external cooldown \
             without an effect"
                .to_string()
        )]
    );
}
