//! Synchronous source-Rust fight ↔ actual gsim world boundary.
//! Activated only by FOREVER_GSIM_WORLD_BRIDGE=1. All damage comes from the
//! unchanged native Fight::deal_damage; the world applies it before native
//! scheduler resumes. This is a bounded multi-fight diagnostic, NOT P7 acceptance.
use std::{cell::RefCell, io::{self, BufRead, Write}};
use serde::Serialize;
use serde_json::{json, Value};

const TICK_NS: i64 = 100_000_000;

#[derive(Clone, Serialize)]
struct DamageEvent {
    event_id: String,
    native_time_ns: i64,
    damage: f64,
    source_action_id: String,
    rank: u32,
    landed: bool,
    crit: bool,
    periodic: bool,
    native_target_index: u32,
    native_target_unit_index: u32,
    native_target_label: String,
    native_caster_type: String,
    native_caster_unit_index: u32,
    native_caster_label: String,
}

/// Source-ACK-backed early World-APL selection, never a fake world kill.
#[derive(Clone, Serialize)]
struct SourceAplWorldSwitch {
    from_native_target_index: u32,
    to_native_target_index: u32,
    prior_world_target_id: String,
    prior_world_target_health: f64,
    prior_world_target_max_health: f64,
    prior_world_ack_id: String,
    source_ack_sequence: u64,
    switch_native_clock_ns: i64,
    health_fraction_at_most: f64,
}

struct Bridge {
    iteration: u32,
    sequence: u64,
    next_tick_ns: i64,
    last_tick_ns: i64,
    last_health: Option<f64>,
    /// Maximum HP accompanying the same committed gsim target ACK.
    last_max_health: Option<f64>,
    /// Sticky gsim target IDs and HP, learned only from validated world ACKs.
    native_world_target_vitals: std::collections::BTreeMap<u32, (String, f64, f64)>,
    previous_ack_id: String,
    /// Populated only by source-ACK-validated APL changeTarget, never serial pulls.
    source_apl_world_switch: Option<SourceAplWorldSwitch>,
    /// True only after gsim committed two different simultaneously living native slots.
    observed_two_live_targets: bool,
    /// ACK frames with a positive native periodic old-A tick and actual old-A HP loss.
    old_a_post_switch_dot_acks: u64,
    source_hash: String,
    event_ordinal: u64,
    pending: Vec<DamageEvent>,
    // `finish` closes the encounter before Fight::cleanup() tears down auras/pets.
    // Teardown callbacks must not enqueue events for the next stochastic iteration.
    iteration_finished: bool,
}
thread_local! {
    static BRIDGE: RefCell<Option<Bridge>> = const { RefCell::new(None) };
}

pub(crate) fn enabled() -> bool {
    std::env::var("FOREVER_GSIM_WORLD_BRIDGE").ok().as_deref() == Some("1")
}

pub(crate) fn start(source_hash: &str) {
    assert!(enabled() && source_hash.len() == 64, "RUST_WORLD_INVALID_SOURCE");
    BRIDGE.with(|state| *state.borrow_mut() = Some(Bridge {
        iteration: 0, sequence: 0, next_tick_ns: 0, last_tick_ns: -1,
        last_health: None, last_max_health: None, native_world_target_vitals: Default::default(), previous_ack_id: String::new(), source_apl_world_switch: None, observed_two_live_targets: false, old_a_post_switch_dot_acks: 0,
        source_hash: source_hash.to_owned(), event_ordinal: 0,
        pending: Vec::new(), iteration_finished: false,
    }));
}

/// Preserve original source request and seed; only first N source fights
/// run during a cheap diagnostic, never silently award 10k-fight acceptance.
pub(crate) fn requested_iterations(source_count: u32) -> u32 {
    let count = std::env::var("FOREVER_GSIM_WORLD_ITERATIONS")
        .ok().map(|s| s.parse::<u32>().expect("RUST_WORLD_ITERATIONS_NOT_INTEGER"))
        .unwrap_or(1);
    assert!(count > 0 && count <= source_count, "RUST_WORLD_ITERATIONS_OUT_OF_SOURCE_RANGE");
    count
}

pub(crate) fn begin_iteration(iteration: u32) {
    BRIDGE.with(|state| {
        let mut state = state.borrow_mut();
        let bridge = state.as_mut().expect("RUST_WORLD_NOT_STARTED");
        assert!(iteration == 0 || iteration == bridge.iteration + 1,
            "RUST_WORLD_ITERATION_NOT_SEQUENTIAL");
        assert!(iteration == 0 || bridge.iteration_finished,
            "RUST_WORLD_PREVIOUS_ITERATION_NOT_FINISHED");
        assert!(bridge.pending.is_empty(), "RUST_WORLD_UNFLUSHED_EVENTS");
        bridge.iteration = iteration;
        bridge.sequence = 0;
        bridge.next_tick_ns = 0;
        bridge.last_tick_ns = -1;
        bridge.last_health = None;
        bridge.last_max_health = None;
        bridge.native_world_target_vitals.clear();
        bridge.source_apl_world_switch = None;
        bridge.observed_two_live_targets = false;
        bridge.old_a_post_switch_dot_acks = 0;
        bridge.previous_ack_id.clear();
        bridge.event_ordinal = 0;
        bridge.iteration_finished = false;
    });
}

/// Switch the acknowledged world target without restarting the native clock
/// or ACK ancestry. Only a future same-actor session may call this; legacy
/// independent iterations intentionally continue to use begin_iteration().
pub(crate) fn begin_next_target(at_ns: i64) -> Result<(), &'static str> {
    BRIDGE.with(|state| {
        let mut state = state.borrow_mut();
        let bridge = state.as_mut().ok_or("RUST_WORLD_NOT_STARTED")?;
        if !bridge.pending.is_empty() {
            return Err("RUST_WORLD_UNACKNOWLEDGED_DAMAGE");
        }
        if at_ns <= bridge.last_tick_ns || at_ns < 0 {
            return Err("RUST_WORLD_NEXT_TARGET_CLOCK_REWOUND");
        }
        // Expiring a duration does not kill a distinct gsim world mob.
        // Only the previous source-native effects' committed HP/death/hash ACK
        // can authorize this retained Rust actor to acquire its next target.
        if !has_lethal_ack(bridge) {
            return Err("RUST_WORLD_NEXT_TARGET_NOT_NATIVE_LETHAL");
        }
        bridge.iteration = bridge.iteration.checked_add(1)
            .ok_or("RUST_WORLD_TARGET_INDEX_OVERFLOW")?;
        bridge.last_health = None; // new world target, not a healed old one
        bridge.last_max_health = None;
        bridge.native_world_target_vitals.clear(); // serial slot reused for a different world ID
        bridge.source_apl_world_switch = None; // the next serial mob is not an APL switch
        bridge.observed_two_live_targets = false;
        bridge.old_a_post_switch_dot_acks = 0;
        bridge.next_tick_ns = at_ns;
        // Deliberately preserve sequence, previous_ack_id, last_tick_ns,
        // event_ordinal and source identity across targets.
        Ok(())
    })
}

/// Source-ACK-authorized acquisition of B while A is still alive.
///
/// Unlike serial begin_next_target, this does not retire or clear A's world
/// identity, native periodic effects, ACK ancestry, or the shared clock. It
/// only advances the world-boundary selection to slot 1. The calling Fight
/// must separately switch its *native* APL target to Side::Extra(0); this
/// bridge does not synthesize an APL cast, a second mob or a world kill.
/// A's current HP must come from a committed native-index-0 2D ACK.
// Only source APL changeTarget can authorize this transition in production.
pub(crate) fn begin_overlapping_target(
    at_ns: i64,
    health_fraction_at_most: f64,
) -> Result<(), &'static str> {
    BRIDGE.with(|state| {
        let mut state = state.borrow_mut();
        let bridge = state.as_mut().ok_or("RUST_WORLD_NOT_STARTED")?;
        if !health_fraction_at_most.is_finite()
            || !(0.0..=1.0).contains(&health_fraction_at_most)
        {
            return Err("RUST_WORLD_OVERLAP_THRESHOLD_INVALID");
        }
        if !bridge.pending.is_empty() {
            return Err("RUST_WORLD_UNACKNOWLEDGED_DAMAGE");
        }
        if at_ns < bridge.last_tick_ns || at_ns < 0 {
            return Err("RUST_WORLD_NEXT_TARGET_CLOCK_REWOUND");
        }
        if bridge.iteration != 0 || bridge.native_world_target_vitals.contains_key(&1) {
            return Err("RUST_WORLD_OVERLAP_TWO_LIVE_TARGET_LIMIT");
        }
        if bridge.sequence == 0 || bridge.previous_ack_id.len() != 64 {
            return Err("RUST_WORLD_OVERLAP_SOURCE_ACK_MISSING");
        }
        let Some((world_id, health, maximum)) =
            bridge.native_world_target_vitals.get(&0)
        else {
            return Err("RUST_WORLD_OVERLAP_FIRST_TARGET_ID_MISSING");
        };
        if world_id.is_empty() || !health.is_finite() || !maximum.is_finite()
            || *maximum <= 0.0 || *health <= 0.0 || *health > *maximum
            || bridge.last_health != Some(*health)
        {
            return Err("RUST_WORLD_OVERLAP_FIRST_TARGET_NOT_LIVE_ACKED");
        }
        if *health / *maximum > health_fraction_at_most {
            return Err("RUST_WORLD_OVERLAP_THRESHOLD_NOT_REACHED");
        }
        // Preserve A's authentic switch-authorizing ACK even as future
        // B/old-A periodic effects advance the shared world hash chain.
        bridge.source_apl_world_switch = Some(SourceAplWorldSwitch {
            from_native_target_index: 0,
            to_native_target_index: 1,
            prior_world_target_id: world_id.clone(),
            prior_world_target_health: *health,
            prior_world_target_max_health: *maximum,
            prior_world_ack_id: bridge.previous_ack_id.clone(),
            source_ack_sequence: bridge.sequence,
            switch_native_clock_ns: at_ns,
            health_fraction_at_most,
        });
        bridge.iteration = 1;
        bridge.last_health = None; // Selected B's HP awaits its OWN world ACK.
        bridge.last_max_health = None;
        // Keep index 0 and its last ACKed HP; old-A DoT events still target 0.
        // Never reset sequence, previous_ack_id, pending native effects or
        // event_ordinal. B must bind a distinct world ID on its next damage.
        // An APL changeTarget at the just-ACKed clock is valid; the next
        // gsim tick must still be strictly later than the committed ACK.
        bridge.next_tick_ns = if at_ns == bridge.last_tick_ns {
            at_ns.saturating_add(1_000_000)
        } else {
            at_ns
        };
        Ok(())
    })
}

#[cfg(test)]
mod persistent_next_target_world_lethal_ack_tests {
    use super::*;

    #[test]
    fn persistent_next_target_requires_real_lethal_world_ack() {
        // This is the live Rust bridge's real state machine, not a replacement
        // combat evaluator. Only exchange() can set these ACK-derived fields
        // in production.
        BRIDGE.with(|slot| *slot.borrow_mut() = Some(Bridge {
            iteration: 0, sequence: 1, next_tick_ns: 200,
            last_tick_ns: 100, last_health: Some(25.0), last_max_health: None, native_world_target_vitals: Default::default(),
            previous_ack_id: "a".repeat(64), source_apl_world_switch: None, observed_two_live_targets: false, old_a_post_switch_dot_acks: 0, source_hash: "b".repeat(64),
            event_ordinal: 9, pending: Vec::new(), iteration_finished: false,
        }));
        assert_eq!(begin_next_target(200),
            Err("RUST_WORLD_NEXT_TARGET_NOT_NATIVE_LETHAL"));
        BRIDGE.with(|slot| {
            let mut state = slot.borrow_mut();
            let state = state.as_mut().unwrap();
            state.last_health = Some(0.0);
            state.previous_ack_id.clear();
        });
        assert_eq!(begin_next_target(200),
            Err("RUST_WORLD_NEXT_TARGET_NOT_NATIVE_LETHAL"));
        BRIDGE.with(|slot| slot.borrow_mut().as_mut().unwrap()
            .previous_ack_id = "a".repeat(64));
        assert_eq!(begin_next_target(200), Ok(()));
        BRIDGE.with(|slot| {
            let state = slot.borrow();
            let state = state.as_ref().unwrap();
            assert_eq!(state.iteration, 1);
            assert_eq!(state.sequence, 1);
            assert_eq!(state.event_ordinal, 9);
            assert_eq!(state.previous_ack_id, "a".repeat(64));
            assert_eq!(state.last_tick_ns, 100);
            assert_eq!(state.next_tick_ns, 200);
            assert_eq!(state.last_health, None);
        });
        // The new target cannot be skipped without its own lethal ACK.
        assert_eq!(begin_next_target(300),
            Err("RUST_WORLD_NEXT_TARGET_NOT_NATIVE_LETHAL"));
    }
}

#[cfg(test)]
mod world_lethal_terminal_tests {
    use super::*;

    #[test]
    fn world_lethal_terminal_requires_committed_native_world_ack() {
        BRIDGE.with(|state| *state.borrow_mut() = Some(Bridge {
            iteration: 0, sequence: 1, next_tick_ns: 200,
            last_tick_ns: 100, last_health: Some(10.0), last_max_health: None, native_world_target_vitals: Default::default(),
            previous_ack_id: "a".repeat(64), source_apl_world_switch: None, observed_two_live_targets: false, old_a_post_switch_dot_acks: 0, source_hash: "b".repeat(64),
            event_ordinal: 1, pending: Vec::new(), iteration_finished: false,
        }));
        assert!(!acknowledged_lethal());
        BRIDGE.with(|state| state.borrow_mut().as_mut().unwrap().last_health = Some(0.0));
        assert!(acknowledged_lethal());
        BRIDGE.with(|state| state.borrow_mut().as_mut().unwrap().previous_ack_id.clear());
        assert!(!acknowledged_lethal());
        BRIDGE.with(|state| *state.borrow_mut() = None);
    }
}

pub(crate) fn due(next_native_ns: i64, end_ns: i64) -> Option<i64> {
    BRIDGE.with(|state| {
        let state = state.borrow();
        let bridge = state.as_ref().expect("RUST_WORLD_NOT_STARTED");
        // gsim is the sole authority for independent world deadlines.
        // When the real world has no queued events or moving actors, poll
        // immediately BEFORE the next native scheduler action instead of
        // manufacturing twelve hundred 100ms handshakes per static fight.
        // The millisecond floor never advances world beyond that action.
        let boundary = if bridge.next_tick_ns == i64::MAX {
            if next_native_ns < 0 || next_native_ns > end_ns {
                return None;
            }
            (next_native_ns / 1_000_000) * 1_000_000
        } else {
            bridge.next_tick_ns
        };
        (boundary >= 0 && boundary > bridge.last_tick_ns
            && boundary <= next_native_ns && boundary <= end_ns)
            .then_some(boundary)
    })
}

/// Owned native event facts passed as one argument across the combat/world boundary.
/// The bridge owns ACK identity and serializes source damage unchanged.
pub(crate) struct DamageInput {
    pub(crate) time_ns: i64,
    pub(crate) damage: f64,
    pub(crate) crit: bool,
    pub(crate) landed: bool,
    pub(crate) periodic: bool,
    /// The actual source-native target: Target=0, Extra(0)=1, etc.
    pub(crate) target_index: u32,
    pub(crate) action: String,
    pub(crate) target_label: String,
    pub(crate) caster_type: &'static str,
    pub(crate) caster_index: u32,
    pub(crate) caster_label: String,
}

pub(crate) fn record_damage(event: DamageInput) {
    if !enabled() {
        return;
    }
    assert!(event.damage.is_finite() && event.damage > 0.0 && event.landed,
        "RUST_WORLD_DAMAGE_INVALID");
    BRIDGE.with(|state| {
        let mut state = state.borrow_mut();
        let bridge = state.as_mut().expect("RUST_WORLD_NOT_STARTED");
        append_damage_event(bridge, event);
    });
}

fn append_damage_event(bridge: &mut Bridge, event: DamageInput) {
    assert!(!bridge.iteration_finished, "RUST_WORLD_DAMAGE_AFTER_FINISH");
    assert!(!has_lethal_ack(bridge), "RUST_WORLD_DAMAGE_AFTER_LETHAL_ACK");
    assert!(event.target_index < super::MAX_TARGETS as u32,
        "RUST_WORLD_DAMAGE_TARGET_OUT_OF_RANGE");
    bridge.event_ordinal += 1;
    bridge.pending.push(DamageEvent {
        event_id: format!("native-effect-{}", bridge.event_ordinal),
        native_time_ns: event.time_ns,
        damage: event.damage,
        source_action_id: event.action,
        rank: 0,
        landed: event.landed,
        crit: event.crit,
        periodic: event.periodic,
        native_target_index: event.target_index,
        native_target_unit_index: event.target_index,
        native_target_label: event.target_label,
        native_caster_type: event.caster_type.to_owned(),
        native_caster_unit_index: event.caster_index,
        native_caster_label: event.caster_label,
    });
}

fn send(value: &Value) {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    writeln!(output, "{}", value).expect("RUST_WORLD_STDOUT_FAILED");
    output.flush().expect("RUST_WORLD_FLUSH_FAILED");
}
fn receive() -> Value {
    let stdin = io::stdin();
    let mut line = String::new();
    assert!(stdin.lock().read_line(&mut line).expect("RUST_WORLD_STDIN_FAILED") > 0,
        "RUST_WORLD_PEER_CLOSED_BEFORE_ACK");
    serde_json::from_str(&line).expect("RUST_WORLD_INVALID_JSON")
}
fn number(value: &Value, key: &str) -> f64 {
    let n = value[key].as_f64().unwrap_or_else(|| panic!("RUST_WORLD_MISSING_{key}"));
    assert!(n.is_finite(), "RUST_WORLD_NONFINITE");
    n
}
fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_else(|| panic!("RUST_WORLD_MISSING_{key}"))
}

/// After an ACK-backed changeTarget from A to B, gsim must actually have
/// selected a distinct world mob before native effects can be sent. The source
/// switch record is retained unchanged over every tick of this joint Fight.
fn validate_overlap_selected_world_tick(bridge: &Bridge, tick: &Value) {
    if let Some(switch) = bridge.source_apl_world_switch.as_ref() {
        let selected = tick["selected_target_id"].as_str();
        assert!(bridge.iteration == switch.to_native_target_index
            && selected.is_some_and(|id| !id.is_empty()
                && id != switch.prior_world_target_id),
            "RUST_WORLD_OVERLAP_SELECTED_WORLD_ID_INVALID");
    }
}

/// Record concrete overlap evidence only after the entire gsim source-damage
/// acknowledgement and per-world-ID HP continuity have been validated.
/// Source switch intent or two later deaths are insufficient by themselves.
fn record_overlap_ack_evidence(bridge: &mut Bridge, events: &[DamageEvent], ack: &Value) {
    let Some(source_switch) = bridge.source_apl_world_switch.as_ref() else {
        return;
    };
    // The ledger has already applied this ACK. If the very first real
    // native B hit kills B, its committed *after* HP is zero even though
    // source/gsim observed A and B alive together immediately before it.
    // Count the per-world-ID pre-damage vitals from this validated ACK, not
    // merely both post-damage ledger values. This is not inferred from a
    // policy echo, a prepared mob, or unACKed native damage.
    let rows = ack["native_target_health_acks"].as_array();
    let a_before = rows.and_then(|rows| rows.iter().find(|row|
        row["native_target_index"].as_u64() == Some(0)))
        .map(|row| number(row, "world_target_health_before") > 0.0)
        .unwrap_or_else(|| bridge.native_world_target_vitals.get(&0)
            .is_some_and(|(id, hp, _)| id == &source_switch.prior_world_target_id && *hp > 0.0));
    let b_before = rows.is_some_and(|rows| rows.iter().any(|row|
        row["native_target_index"].as_u64() == Some(1)
            && number(row, "world_target_health_before") > 0.0));
    let b_after = bridge.native_world_target_vitals.get(&1)
        .is_some_and(|(_, hp, _)| *hp > 0.0);
    bridge.observed_two_live_targets |= a_before && (b_before || b_after);
    let old_a_periodic = events.iter().any(|event| {
        event.native_target_index == source_switch.from_native_target_index
            && event.native_time_ns >= source_switch.switch_native_clock_ns
            && event.periodic && event.landed && event.damage > 0.0
    });
    let old_a_hp_lost = ack["native_target_health_acks"].as_array()
        .is_some_and(|rows| rows.iter().any(|row| {
            row["native_target_index"].as_u64()
                == Some(u64::from(source_switch.from_native_target_index))
                && number(row, "world_target_health_before")
                    > number(row, "world_target_health_after")
        }));
    if old_a_periodic && old_a_hp_lost {
        bridge.old_a_post_switch_dot_acks += 1;
    }
}

/// A matching iteration_index on the outer ACK is not enough: the selected
/// damage-ACK's *native slot* must be B, with the multi-target ledger retained.
/// Otherwise A's scalar HP could be misrepresented as B's selected HP.
fn validate_overlap_selected_world_ack(bridge: &Bridge, ack: &Value) {
    if let Some(switch) = bridge.source_apl_world_switch.as_ref() {
        assert!(ack["selected_native_target_index"].as_u64()
                == Some(u64::from(switch.to_native_target_index))
            && ack["native_target_health_acks"].is_array(),
            "RUST_WORLD_OVERLAP_SELECTED_B_ACK_INVALID");
    }
}

/// One retained Rust process. The next native scheduler action cannot execute
/// before the gsim CombatRuntime applied every real source damage event.
pub(crate) fn exchange(clock_ns: i64, duration_ns: i64, cumulative_damage: f64) -> (f64, bool, bool) {
    let mut bridge = BRIDGE.with(|state| state.borrow_mut().take()
        .expect("RUST_WORLD_NOT_STARTED"));
    assert!(clock_ns > bridge.last_tick_ns && clock_ns >= 0,
        "RUST_WORLD_CLOCK_NOT_MONOTONIC");
    let events = std::mem::take(&mut bridge.pending);
    let event_ids: Vec<String> = events.iter().map(|event| event.event_id.clone()).collect();
    let pid = std::process::id();
    send(&json!({
        "type": "native_rust_world_boundary_request",
        "iteration_index": bridge.iteration,
        "sequence": bridge.sequence, "native_clock_ns": clock_ns,
        "native_encounter_end_ns": duration_ns,
        "native_process_id": pid, "native_request_sha256": bridge.source_hash,
        // The 2D peer can authorize a genuinely overlapping B only from this
        // source APL + world-ACK receipt, never merely iteration_index=1.
        "source_apl_world_switch": bridge.source_apl_world_switch.as_ref(),
    }));
    let tick = receive();
    assert!(tick["type"] == "gsim_world_range_tick"
        && tick["sequence"] == bridge.sequence
        && tick["iteration_index"] == bridge.iteration
        && tick["native_clock_ns"] == clock_ns
        && tick["world_source"] == "gsim.CombatRuntime"
        && tick["previous_world_ack_id"] == bridge.previous_ack_id,
        "RUST_WORLD_TICK_IDENTITY_OR_ACK_CHAIN_INVALID");
    validate_overlap_selected_world_tick(&bridge, &tick);
    let distance = number(&tick, "distance_yards");
    assert!((0.0..=100.0).contains(&distance), "RUST_WORLD_DISTANCE_INVALID");
    let front = tick["in_front_of_target"].as_bool()
        .expect("RUST_WORLD_FACING_NOT_PROVIDED");
    // World movement is a live gsim fact, not a native game-rule override.
    // Existing stationary 2D probes omit movement; retain their false
    // default. When a live route explicitly supplies movement, reject any
    // malformed flag rather than silently permitting a noninstant cast.
    let moving = tick.get("gsim_player_moving").map(|value|
        value.as_bool().expect("RUST_WORLD_PLAYER_MOVEMENT_INVALID")
    ).unwrap_or(false);
    let before_health = number(&tick, "world_target_health_before");
    let max_health = number(&tick, "world_target_max_health");
    validate_selected_world_health(&bridge, before_health, max_health);
    if let Some(last) = bridge.last_health {
        assert!((last - before_health).abs() < 1e-6, "RUST_WORLD_HEALTH_ACK_NOT_CONTINUOUS");
    }
    let before_hash = string(&tick, "world_sha256").to_owned();
    let next_world_boundary_ns = match tick.get("next_world_boundary_ns") {
        Some(Value::Null) => i64::MAX,
        Some(value) => value.as_i64().expect("RUST_WORLD_DEADLINE_NOT_INTEGER"),
        None => panic!("RUST_WORLD_DEADLINE_MISSING"),
    };
    assert!(next_world_boundary_ns == i64::MAX
        || (next_world_boundary_ns > clock_ns
            && next_world_boundary_ns <= clock_ns.saturating_add(TICK_NS)),
        "RUST_WORLD_DEADLINE_INVALID");
    send(&json!({
        "type": "native_rust_world_receipt",
        "iteration_index": bridge.iteration,
        "sequence": bridge.sequence, "native_process_id": pid,
        "native_request_sha256": bridge.source_hash,
        "observed_input_world_sha256": before_hash,
        "native_damage_events": events,
        "native_range_receipt": {
            "sequence": bridge.sequence, "native_clock_ns": clock_ns,
            "player_index": 0, "applied_yards": distance,
            "native_encounter_damage": cumulative_damage,
            "facing_applied": true, "in_front_of_target": front,
            "gsim_player_moving": moving
        },
        "checkpoint_certified": false
    }));
    let ack = receive();
    assert!(ack["type"] == "gsim_native_damage_applied_ack"
        && ack["sequence"] == bridge.sequence
        && ack["iteration_index"] == bridge.iteration
        && ack["native_process_id"] == pid
        && ack["world_before_sha256"] == before_hash,
        "RUST_WORLD_DAMAGE_ACK_IDENTITY_INVALID");
    validate_overlap_selected_world_ack(&bridge, &ack);
    let accepted = ack["applied_event_ids"].as_array()
        .expect("RUST_WORLD_DAMAGE_ACK_EVENT_IDS_MISSING");
    assert_damage_acknowledgement(&event_ids, accepted);
    let ack_before = number(&ack, "world_target_health_before");
    let ack_after = number(&ack, "world_target_health_after");
    // A scalar HP ACK only describes the selected world mob. Never certify
    // an old target's DoT by subtracting its damage from a different mob.
    // Concurrent targets need separate gsim-produced, hash-bound HP ACKs.
    validate_native_target_damage_acks(&events, &ack, before_health, ack_before, ack_after);
    validate_world_target_continuity(&mut bridge.native_world_target_vitals, &ack);
    record_overlap_ack_evidence(&mut bridge, &events, &ack);
    bridge.previous_ack_id = string(&ack, "world_after_sha256").to_owned();
    assert!(bridge.previous_ack_id.len() == 64, "RUST_WORLD_DAMAGE_ACK_HASH_INVALID");
    bridge.last_health = Some(ack_after);
    bridge.last_max_health = Some(max_health);
    bridge.last_tick_ns = clock_ns;
    bridge.sequence += 1;
    bridge.next_tick_ns = next_world_boundary_ns;
    BRIDGE.with(|state| *state.borrow_mut() = Some(bridge));
    (distance, front, moving)
}

/// The selected B may already be dead while an older source-ACKed A DoT
/// remains active. Accept a zero-HP selected world only when its *own* native
/// index and maximum HP were previously committed dead by gsim. A never-ACKed
/// zero-HP selected mob is not a real engagement or an invented lethal kill.
fn validate_selected_world_health(bridge: &Bridge, health: f64, maximum: f64) {
    assert!(maximum.is_finite() && maximum > 0.0 && health.is_finite()
        && health >= 0.0 && health <= maximum,
        "RUST_WORLD_HEALTH_INVALID");
    if health == 0.0 {
        assert!(bridge.native_world_target_vitals.get(&bridge.iteration)
            .is_some_and(|(_, previous, max)| *previous == 0.0
                && (max - maximum).abs() < 1e-6),
            "RUST_WORLD_SELECTED_DEAD_TARGET_UNACKNOWLEDGED");
    }
}

/// A single-target ACK cannot certify damage to two distinct living enemies.
/// For overlapping damage require the 2D peer's per-native-index, per-world-ID
/// post-application vitals, under the same committed world_after_sha256.
/// These are observations from gsim, not native projections of enemy HP.
fn validate_native_target_damage_acks(
    events: &[DamageEvent], ack: &Value, selected_before: f64,
    ack_before: f64, ack_after: f64,
) {
    assert!((ack_before - selected_before).abs() < 1e-6,
        "RUST_WORLD_DAMAGE_ACK_HEALTH_MISMATCH");
    let mut damage_by_target = std::collections::BTreeMap::<u32, f64>::new();
    for event in events {
        *damage_by_target.entry(event.native_target_index).or_default() += event.damage;
    }
    // A serial scalar receipt is valid only while the selected native
    // target is zero. After an early switch to target 1, a tick containing
    // ONLY target 0's old DoT must still use the per-world-ID ACK ledger.
    // Otherwise it could subtract old-target damage from the new mob's HP.
    let selected_index = ack.get("selected_native_target_index")
        .map(|value| value.as_u64()
            .expect("RUST_WORLD_MULTI_TARGET_SELECTED_INDEX_INVALID"));
    if selected_index.unwrap_or(0) == 0
        && ack.get("native_target_health_acks").is_none()
        && damage_by_target.keys().all(|&index| index == 0)
    {
        let damage = damage_by_target.get(&0).copied().unwrap_or(0.0);
        assert!((ack_after - (selected_before - damage).max(0.0)).abs() < 1e-6,
            "RUST_WORLD_DAMAGE_ACK_HEALTH_MISMATCH");
        return;
    }
    let selected_index = selected_index
        .expect("RUST_WORLD_MULTI_TARGET_SELECTED_INDEX_MISSING");
    assert!(selected_index < super::MAX_TARGETS as u64,
        "RUST_WORLD_MULTI_TARGET_SELECTED_INDEX_INVALID");
    let rows = ack["native_target_health_acks"].as_array()
        .expect("RUST_WORLD_MULTI_TARGET_ACK_MISSING");
    let mut seen = std::collections::BTreeSet::new();
    let mut ids = std::collections::BTreeSet::new();
    let mut selected_after_verified = false;
    for row in rows {
        let index = row["native_target_index"].as_u64()
            .expect("RUST_WORLD_MULTI_TARGET_ACK_INDEX_MISSING");
        assert!(index < super::MAX_TARGETS as u64
            && seen.insert(index), "RUST_WORLD_MULTI_TARGET_ACK_DUPLICATE_OR_INVALID");
        let id = row["world_target_id"].as_str()
            .expect("RUST_WORLD_MULTI_TARGET_ACK_ID_MISSING");
        assert!(!id.is_empty() && ids.insert(id),
            "RUST_WORLD_MULTI_TARGET_ACK_WORLD_ID_NOT_DISTINCT");
        let health_before = number(row, "world_target_health_before");
        let health_after = number(row, "world_target_health_after");
        let max_health = number(row, "world_target_max_health");
        // A retained old-target DoT may tick after gsim already ACKed that
        // target dead. A zero-HP no-op is legal ONLY for that previously
        // bound dead identity (enforced by validate_world_target_continuity).
        assert!(max_health > 0.0 && health_before >= 0.0
            && health_before <= max_health && health_after >= 0.0
            && health_after <= max_health,
            "RUST_WORLD_MULTI_TARGET_ACK_HEALTH_INVALID");
        let damage = damage_by_target.get(&(index as u32))
            .expect("RUST_WORLD_MULTI_TARGET_ACK_UNSOLICITED");
        assert!((health_after - (health_before - damage).max(0.0)).abs() < 1e-6,
            "RUST_WORLD_MULTI_TARGET_ACK_DAMAGE_MISMATCH");
        if index == selected_index {
            assert!((health_before - selected_before).abs() < 1e-6
                && (health_after - ack_after).abs() < 1e-6,
                "RUST_WORLD_MULTI_TARGET_SELECTED_ACK_MISMATCH");
            selected_after_verified = true;
        }
    }
    // If the selected target did not take damage, the scalar ACK still
    // describes its unchanged HP; never invent an extra receipt.
    if !selected_after_verified {
        assert!(!damage_by_target.contains_key(&(selected_index as u32))
            && (ack_after - selected_before).abs() < 1e-6,
            "RUST_WORLD_MULTI_TARGET_SELECTED_ACK_MISSING");
    }
    assert_eq!(seen.len(), damage_by_target.len(),
        "RUST_WORLD_MULTI_TARGET_ACK_INCOMPLETE");
}

/// Bind each native target slot to one real gsim world mob across subsequent
/// committed ACKs. A different world ID, resurrected HP or changed max HP is
/// not a continued DoT target, even if an isolated damage sum would balance.
fn validate_world_target_continuity(
    known: &mut std::collections::BTreeMap<u32, (String, f64, f64)>,
    ack: &Value,
) {
    let Some(rows) = ack.get("native_target_health_acks") else {
        assert!(known.is_empty(), "RUST_WORLD_MULTI_TARGET_LEDGER_DISAPPEARED");
        return; // Existing serial target-0 wire contract has no world ID.
    };
    let rows = rows.as_array().expect("RUST_WORLD_MULTI_TARGET_ACK_MISSING");
    for row in rows {
        let index = row["native_target_index"].as_u64()
            .expect("RUST_WORLD_MULTI_TARGET_ACK_INDEX_MISSING") as u32;
        let id = string(row, "world_target_id");
        let before = number(row, "world_target_health_before");
        let after = number(row, "world_target_health_after");
        let maximum = number(row, "world_target_max_health");
        if before == 0.0 {
            // Do not let an unbound world target start life already dead.
            // Zero is allowed only for an ACKed old mob whose DoTs have
            // continued ticking while the second mob remains engaged.
            assert!(known.get(&index).is_some_and(|(old_id, old_health, old_max)|
                old_id == id && *old_health == 0.0
                    && (*old_max - maximum).abs() < 1e-6),
                "RUST_WORLD_MULTI_TARGET_ZERO_HP_NOT_PREVIOUSLY_ACKED");
        }
        if let Some((old_id, old_health, old_max)) = known.get(&index) {
            assert!(old_id == id, "RUST_WORLD_MULTI_TARGET_WORLD_ID_CHANGED");
            assert!((old_health - before).abs() < 1e-6
                && (old_max - maximum).abs() < 1e-6,
                "RUST_WORLD_MULTI_TARGET_HP_NOT_CONTINUOUS");
        }
        assert!(known.iter().all(|(&other, (old_id, _, _))|
                other == index || old_id != id),
            "RUST_WORLD_MULTI_TARGET_WORLD_ID_REASSIGNED");
        known.insert(index, (id.to_owned(), after, maximum));
    }
}

fn has_lethal_ack(bridge: &Bridge) -> bool {
    if bridge.sequence == 0 || bridge.last_health != Some(0.0)
        || bridge.previous_ack_id.len() != 64 {
        return false;
    }
    // In an overlapping world the currently selected mob can die before the
    // previous mob's DoTs finish. Never finish the native Fight or permit slot
    // reuse from B's scalar lethal ACK while A is still alive. Both distinct
    // source-ACKed world identities must have committed zero HP. The original
    // serial scalar receipt remains valid when there is no multi-target ledger.
    if bridge.native_world_target_vitals.contains_key(&1) {
        return bridge.native_world_target_vitals.len() == 2
            && bridge.native_world_target_vitals.get(&0)
                .is_some_and(|(_, health, _)| *health == 0.0)
            && bridge.native_world_target_vitals.get(&1)
                .is_some_and(|(_, health, _)| *health == 0.0);
    }
    true
}

/// Source-native current target HP from a completed 2D world ACK, never a
/// forecast using damage, DPS or a neighbouring target's health.
pub(crate) fn current_target_health_percent() -> Option<f64> {
    BRIDGE.with(|state| {
        let state = state.borrow();
        let bridge = state.as_ref()?;
        if bridge.sequence == 0 || bridge.previous_ack_id.len() != 64 {
            return None;
        }
        let (health, max_health) = bridge.last_health.zip(bridge.last_max_health)?;
        if !health.is_finite() || !max_health.is_finite()
            || max_health <= 0.0 || health < 0.0 || health > max_health {
            return None;
        }
        Some(health / max_health)
    })
}

pub(crate) fn lethal_ack_received() -> bool {
    BRIDGE.with(|state| {
        let state = state.borrow();
        let bridge = state.as_ref().expect("RUST_WORLD_NOT_STARTED");
        has_lethal_ack(bridge)
    })
}

pub(crate) fn acknowledged_lethal() -> bool {
    BRIDGE.with(|state| {
        let state = state.borrow();
        let bridge = state.as_ref().expect("RUST_WORLD_NOT_STARTED");
        has_lethal_ack(bridge) && bridge.pending.is_empty()
    })
}

/// Evidence from one retained native Fight's already-committed gsim ACKs.
/// A missing proof remains false/zero; it must not be inferred from a policy echo.
pub(crate) fn overlap_ack_evidence() -> (bool, u64) {
    BRIDGE.with(|state| {
        let state = state.borrow();
        let bridge = state.as_ref().expect("RUST_WORLD_NOT_STARTED");
        (bridge.observed_two_live_targets, bridge.old_a_post_switch_dot_acks)
    })
}

/// An APL-selected overlapping pair has two distinct, genuinely bound world
/// identities and two lethal gsim ACKs. A serial pull does not have both
/// native slots in the same world ledger.
pub(crate) fn acknowledged_overlapping_deaths() -> Option<(String, String)> {
    BRIDGE.with(|state| {
        let state = state.borrow();
        let bridge = state.as_ref()?;
        // Two zero-HP ledger rows alone do not prove a source-selected rolling
        // pull. The same retained Fight must have the ACK-backed A→B APL
        // transition, still bound to the first real world identity. In
        // particular a serial/foreign multi-target receipt is not overlap.
        let source_switch = bridge.source_apl_world_switch.as_ref()?;
        if bridge.iteration != 1
            || source_switch.from_native_target_index != 0
            || source_switch.to_native_target_index != 1
            || bridge.iteration != source_switch.to_native_target_index
            || !has_lethal_ack(bridge)
            || !bridge.pending.is_empty()
        {
            return None;
        }
        let (a_id, a_hp, _) = bridge.native_world_target_vitals.get(&0)?;
        if a_id != &source_switch.prior_world_target_id {
            return None;
        }
        let (b_id, b_hp, _) = bridge.native_world_target_vitals.get(&1)?;
        if *a_hp != 0.0 || *b_hp != 0.0 || a_id == b_id || a_id.is_empty() || b_id.is_empty() {
            return None;
        }
        Some((a_id.clone(), b_id.clone()))
    })
}

fn assert_damage_acknowledgement(event_ids: &[String], accepted: &[Value]) {
    assert!(accepted.len() == event_ids.len()
        && accepted.iter().zip(event_ids.iter()).all(|(a, b)| a.as_str() == Some(b)),
        "RUST_WORLD_DAMAGE_ACK_EVENTS_MISMATCH");
}

fn mark_iteration_finished() {
    BRIDGE.with(|state| {
        let mut state = state.borrow_mut();
        let bridge = state.as_mut().expect("RUST_WORLD_NOT_STARTED");
        assert!(bridge.pending.is_empty(), "RUST_WORLD_FINISH_UNFLUSHED");
        bridge.iteration_finished = true;
    });
}

pub(crate) fn finish(duration_ns: i64, cumulative_damage: f64) {
    // The terminal world flush is at the next representable gsim millisecond.
    // This is an explicit diagnostic boundary, never a claim of P7 parity.
    let (last, lethal_ack, pending_empty) = BRIDGE.with(|state| {
        let state = state.borrow();
        let bridge = state.as_ref().expect("RUST_WORLD_NOT_STARTED");
        (
            bridge.last_tick_ns,
            has_lethal_ack(bridge),
            bridge.pending.is_empty(),
        )
    });
    if lethal_ack {
        // gsim has already applied the last native event and killed the mob.
        // A cleanup event after that ACK is a real lifecycle violation; never
        // drop it or try to apply more damage to an already-dead target.
        assert!(pending_empty, "RUST_WORLD_LETHAL_HAS_PENDING_EVENTS");
        assert!(
            duration_ns == last,
            "RUST_WORLD_LETHAL_DURATION_NOT_ACK_CLOCK"
        );
    } else {
        let clock = ((duration_ns + 999_999) / 1_000_000) * 1_000_000;
        let clock = clock.max(last + 1_000_000);
        let _ = exchange(clock, duration_ns, cumulative_damage);
    }
    mark_iteration_finished();
    BRIDGE.with(|state| {
        let state = state.borrow();
        let bridge = state.as_ref().expect("RUST_WORLD_NOT_STARTED");
        send(&json!({
            "type": "native_rust_world_iteration_done",
            "iteration_index": bridge.iteration,
            "native_process_id": std::process::id(),
            "native_request_sha256": bridge.source_hash,
            "native_duration_ns": duration_ns,
            "native_total_damage": cumulative_damage,
        }));
    });
}

#[cfg(test)]
mod world_overlap_acquisition_tests {
    use super::{begin_overlapping_target, begin_next_target, Bridge, BRIDGE};
    use std::collections::BTreeMap;

    fn acknowledged_first_target(health: f64) {
        let mut vitals = BTreeMap::new();
        vitals.insert(0, ("real-world-A".to_owned(), health, 100.0));
        BRIDGE.with(|slot| *slot.borrow_mut() = Some(Bridge {
            iteration: 0, sequence: 3, next_tick_ns: 200, last_tick_ns: 100,
            last_health: Some(health), last_max_health: Some(100.0),
            native_world_target_vitals: vitals,
            previous_ack_id: "a".repeat(64), source_apl_world_switch: None, observed_two_live_targets: false, old_a_post_switch_dot_acks: 0, source_hash: "b".repeat(64),
            event_ordinal: 9, pending: Vec::new(), iteration_finished: false,
        }));
    }

    #[test]
    fn post_switch_old_a_dot_requires_real_hp_loss_and_concurrent_live_ack() {
        acknowledged_first_target(45.0);
        begin_overlapping_target(200, 0.5).unwrap();
        BRIDGE.with(|slot| {
            let mut state = slot.borrow_mut();
            let bridge = state.as_mut().unwrap();
            let mut dot = super::DamageEvent {
                event_id: "native-dot".into(),
                native_time_ns: 201,
                damage: 15.0,
                source_action_id: "native-dot".into(),
                rank: 1, landed: true, crit: false, periodic: true,
                native_target_index: 0, native_target_unit_index: 0,
                native_target_label: "A".into(),
                native_caster_type: "player".into(), native_caster_unit_index: 0,
                native_caster_label: "warlock".into(),
            };
            let real_old_a_ack = serde_json::json!({
                "native_target_health_acks": [{
                    "native_target_index": 0,
                    "world_target_health_before": 45.0,
                    "world_target_health_after": 30.0
                }]
            });
            // A landed native DoT alone is not two engaged living targets.
            super::record_overlap_ack_evidence(bridge, &[dot.clone()], &real_old_a_ack);
            assert!(!bridge.observed_two_live_targets);
            assert_eq!(bridge.old_a_post_switch_dot_acks, 1);
            // Only actual source/2D-bound B permits the concurrent-alive proof.
            bridge.native_world_target_vitals.insert(
                1, ("real-world-B".into(), 90.0, 100.0));
            dot.periodic = false;
            super::record_overlap_ack_evidence(bridge, &[dot.clone()], &real_old_a_ack);
            assert!(bridge.observed_two_live_targets);
            assert_eq!(bridge.old_a_post_switch_dot_acks, 1);
            dot.periodic = true;
            super::record_overlap_ack_evidence(bridge, &[dot], &serde_json::json!({
                "native_target_health_acks": [{
                    "native_target_index": 0,
                    "world_target_health_before": 30.0,
                    "world_target_health_after": 30.0
                }]
            }));
            assert_eq!(bridge.old_a_post_switch_dot_acks, 1);
        });
        BRIDGE.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    fn first_native_b_hit_killing_b_still_proves_two_live_targets_before_damage() {
        acknowledged_first_target(45.0);
        begin_overlapping_target(200, 0.5).unwrap();
        BRIDGE.with(|slot| {
            let mut state = slot.borrow_mut();
            let bridge = state.as_mut().unwrap();
            // These post-application vitals have already passed per-ID world
            // continuity validation. The first B hit was lethal, so B's
            // committed after-health is zero; A remained alive throughout.
            bridge.native_world_target_vitals.insert(
                1, ("real-world-B".into(), 0.0, 90.0));
            let first_b_ack = serde_json::json!({
                "native_target_health_acks": [{
                    "native_target_index": 1,
                    "world_target_health_before": 90.0,
                    "world_target_health_after": 0.0
                }]
            });
            super::record_overlap_ack_evidence(bridge, &[], &first_b_ack);
            assert!(bridge.observed_two_live_targets);
            // A later ACK after A died cannot manufacture the same proof
            // from the old switch-time A health snapshot.
            bridge.observed_two_live_targets = false;
            bridge.native_world_target_vitals.get_mut(&0).unwrap().1 = 0.0;
            super::record_overlap_ack_evidence(bridge, &[], &first_b_ack);
            assert!(!bridge.observed_two_live_targets);
        });
        BRIDGE.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    fn source_ack_at_half_hp_acquires_b_without_erasing_a_or_doing_a_kill() {
        acknowledged_first_target(45.0);
        // Serial transitions still cannot advance after a merely nonlethal ACK.
        assert_eq!(begin_next_target(200), Err("RUST_WORLD_NEXT_TARGET_NOT_NATIVE_LETHAL"));
        assert_eq!(begin_overlapping_target(200, 0.5), Ok(()));
        BRIDGE.with(|slot| {
            let borrowed = slot.borrow();
            let bridge = borrowed.as_ref().unwrap();
            assert_eq!(bridge.iteration, 1);
            assert_eq!(bridge.sequence, 3);
            assert_eq!(bridge.previous_ack_id, "a".repeat(64));
            assert_eq!(bridge.event_ordinal, 9);
            assert_eq!(bridge.last_tick_ns, 100);
            assert_eq!(bridge.next_tick_ns, 200);
            assert_eq!(bridge.last_health, None);
            assert_eq!(bridge.native_world_target_vitals.get(&0).unwrap().1, 45.0);
            assert!(!bridge.native_world_target_vitals.contains_key(&1));
            let receipt = serde_json::to_value(
                bridge.source_apl_world_switch.as_ref().unwrap()).unwrap();
            assert_eq!(receipt["from_native_target_index"], 0);
            assert_eq!(receipt["to_native_target_index"], 1);
            assert_eq!(receipt["prior_world_target_id"], "real-world-A");
            assert_eq!(receipt["prior_world_target_health"], 45.0);
            assert_eq!(receipt["prior_world_target_max_health"], 100.0);
            assert_eq!(receipt["prior_world_ack_id"], "a".repeat(64));
            assert_eq!(receipt["source_ack_sequence"], 3);
            assert_eq!(receipt["switch_native_clock_ns"], 200);
            assert_eq!(receipt["health_fraction_at_most"], 0.5);
        });
        // The same encounter may never acquire an unbounded third world mob.
        assert_eq!(begin_overlapping_target(300, 0.5),
            Err("RUST_WORLD_OVERLAP_TWO_LIVE_TARGET_LIMIT"));
        BRIDGE.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    fn selected_b_world_and_native_ack_must_match_source_switch() {
        acknowledged_first_target(45.0);
        begin_overlapping_target(200, 0.5).unwrap();
        BRIDGE.with(|slot| {
            let state = slot.borrow();
            let bridge = state.as_ref().unwrap();
            super::validate_overlap_selected_world_tick(
                bridge, &serde_json::json!({"selected_target_id": "real-world-B"}));
            super::validate_overlap_selected_world_ack(
                bridge, &serde_json::json!({
                    "selected_native_target_index": 1,
                    "native_target_health_acks": []
                }));
        });
        BRIDGE.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    #[should_panic(expected = "RUST_WORLD_OVERLAP_SELECTED_WORLD_ID_INVALID")]
    fn source_switched_b_cannot_keep_selecting_old_world_a() {
        acknowledged_first_target(45.0);
        begin_overlapping_target(200, 0.5).unwrap();
        BRIDGE.with(|slot| {
            let state = slot.borrow();
            super::validate_overlap_selected_world_tick(
                state.as_ref().unwrap(),
                &serde_json::json!({"selected_target_id": "real-world-A"}));
        });
    }

    #[test]
    #[should_panic(expected = "RUST_WORLD_OVERLAP_SELECTED_B_ACK_INVALID")]
    fn source_switched_b_cannot_accept_native_a_as_selected_ack() {
        acknowledged_first_target(45.0);
        begin_overlapping_target(200, 0.5).unwrap();
        BRIDGE.with(|slot| {
            let state = slot.borrow();
            super::validate_overlap_selected_world_ack(
                state.as_ref().unwrap(),
                &serde_json::json!({
                    "selected_native_target_index": 0,
                    "native_target_health_acks": []
                }));
        });
    }

    #[test]
    #[should_panic(expected = "RUST_WORLD_OVERLAP_SELECTED_B_ACK_INVALID")]
    fn source_switched_b_cannot_accept_scalar_only_ack() {
        acknowledged_first_target(45.0);
        begin_overlapping_target(200, 0.5).unwrap();
        BRIDGE.with(|slot| {
            let state = slot.borrow();
            super::validate_overlap_selected_world_ack(
                state.as_ref().unwrap(),
                &serde_json::json!({"selected_native_target_index": 1}));
        });
    }

    #[test]
    fn source_ack_threshold_and_distinct_first_world_identity_are_mandatory() {
        acknowledged_first_target(60.0);
        assert_eq!(begin_overlapping_target(200, 0.5),
            Err("RUST_WORLD_OVERLAP_THRESHOLD_NOT_REACHED"));
        assert_eq!(begin_overlapping_target(200, f64::NAN),
            Err("RUST_WORLD_OVERLAP_THRESHOLD_INVALID"));
        BRIDGE.with(|slot| {
            slot.borrow_mut().as_mut().unwrap().native_world_target_vitals.clear();
        });
        assert_eq!(begin_overlapping_target(200, 1.0),
            Err("RUST_WORLD_OVERLAP_FIRST_TARGET_ID_MISSING"));
        BRIDGE.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    fn apl_switch_at_exact_ack_clock_preserves_two_distinct_world_identities() {
        acknowledged_first_target(45.0);
        assert_eq!(begin_overlapping_target(100, 0.5), Ok(()));
        BRIDGE.with(|slot| {
            let mut state = slot.borrow_mut();
            let bridge = state.as_mut().unwrap();
            assert_eq!(bridge.iteration, 1);
            assert!(bridge.next_tick_ns > bridge.last_tick_ns);
            assert_eq!(bridge.native_world_target_vitals.get(&0).unwrap().0, "real-world-A");
            bridge.native_world_target_vitals.insert(
                1, ("real-world-B".to_owned(), 0.0, 100.0));
            bridge.last_health = Some(0.0);
        });
        assert_eq!(super::acknowledged_overlapping_deaths(), None);
        BRIDGE.with(|slot| {
            slot.borrow_mut().as_mut().unwrap()
                .native_world_target_vitals.get_mut(&0).unwrap().1 = 0.0;
        });
        assert_eq!(super::acknowledged_overlapping_deaths(),
            Some(("real-world-A".to_owned(), "real-world-B".to_owned())));
        BRIDGE.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    fn old_a_dot_can_continue_after_selected_b_is_acknowledged_dead() {
        acknowledged_first_target(30.0);
        assert_eq!(begin_overlapping_target(200, 0.5), Ok(()));
        BRIDGE.with(|slot| {
            let mut state = slot.borrow_mut();
            let bridge = state.as_mut().unwrap();
            // First B world tick/ACK has bound the distinct selected identity
            // and committed lethal HP, but old A remains alive at 30 HP.
            bridge.native_world_target_vitals.insert(
                1, ("real-world-B".to_owned(), 0.0, 200.0));
            bridge.last_health = Some(0.0);
            super::validate_selected_world_health(bridge, 0.0, 200.0);
            // A zero-health receipt cannot be used for another world index.
            bridge.iteration = 2;
        });
        BRIDGE.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    #[should_panic(expected = "RUST_WORLD_SELECTED_DEAD_TARGET_UNACKNOWLEDGED")]
    fn a_new_world_target_cannot_arrive_predead() {
        acknowledged_first_target(30.0);
        begin_overlapping_target(200, 0.5).unwrap();
        BRIDGE.with(|slot| {
            let state = slot.borrow();
            super::validate_selected_world_health(state.as_ref().unwrap(), 0.0, 200.0);
        });
    }

    #[test]
    fn dead_or_unacknowledged_a_is_not_an_early_pull() {
        acknowledged_first_target(0.0);
        assert_eq!(begin_overlapping_target(200, 0.5),
            Err("RUST_WORLD_OVERLAP_FIRST_TARGET_NOT_LIVE_ACKED"));
        BRIDGE.with(|slot| {
            slot.borrow_mut().as_mut().unwrap().last_health = Some(20.0);
        });
        assert_eq!(begin_overlapping_target(200, 0.5),
            Err("RUST_WORLD_OVERLAP_FIRST_TARGET_NOT_LIVE_ACKED"));
        BRIDGE.with(|slot| {
            slot.borrow_mut().as_mut().unwrap().previous_ack_id.clear();
        });
        assert_eq!(begin_overlapping_target(200, 0.5),
            Err("RUST_WORLD_OVERLAP_SOURCE_ACK_MISSING"));
        BRIDGE.with(|slot| *slot.borrow_mut() = None);
    }
}

#[cfg(test)]
mod world_overlap_terminal_tests {
    use super::{acknowledged_lethal, begin_next_target, Bridge, BRIDGE};

    fn set_overlap(old_health: f64, selected_health: f64) {
        let mut vitals = std::collections::BTreeMap::new();
        vitals.insert(0, ("world-A".to_owned(), old_health, 100.0));
        vitals.insert(1, ("world-B".to_owned(), selected_health, 100.0));
        BRIDGE.with(|slot| *slot.borrow_mut() = Some(Bridge {
            iteration: 0,
            sequence: 2,
            next_tick_ns: 200,
            last_tick_ns: 100,
            last_health: Some(selected_health),
            last_max_health: Some(100.0),
            native_world_target_vitals: vitals,
            previous_ack_id: "a".repeat(64), source_apl_world_switch: None, observed_two_live_targets: false, old_a_post_switch_dot_acks: 0,
            source_hash: "b".repeat(64),
            event_ordinal: 2,
            pending: Vec::new(),
            iteration_finished: false,
        }));
    }

    #[test]
    fn overlapping_target_b_lethal_cannot_finish_while_old_dot_target_a_lives() {
        set_overlap(15.0, 0.0);
        assert!(!acknowledged_lethal());
        assert_eq!(
            begin_next_target(200),
            Err("RUST_WORLD_NEXT_TARGET_NOT_NATIVE_LETHAL")
        );
        BRIDGE.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    fn two_independent_world_lethal_acks_allow_terminal_completion() {
        set_overlap(0.0, 0.0);
        assert!(acknowledged_lethal());
        begin_next_target(200).unwrap();
        BRIDGE.with(|slot| {
            let state = slot.borrow();
            let bridge = state.as_ref().unwrap();
            assert!(bridge.native_world_target_vitals.is_empty());
            assert_eq!(bridge.last_health, None);
        });
        BRIDGE.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    fn two_dead_world_ids_without_source_apl_switch_are_not_rolling_evidence() {
        set_overlap(0.0, 0.0);
        BRIDGE.with(|slot| {
            // A two-slot ACK ledger by itself can occur without a
            // source-selected A→B pull. Both deaths remain valid facts,
            // but cannot be labeled as rolling World-APL.
            slot.borrow_mut().as_mut().unwrap().iteration = 1;
        });
        assert!(acknowledged_lethal());
        assert_eq!(super::acknowledged_overlapping_deaths(), None);
        BRIDGE.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    fn selected_target_b_lethal_requires_prior_target_identity_receipt() {
        set_overlap(0.0, 0.0);
        BRIDGE.with(|slot| {
            slot.borrow_mut().as_mut().unwrap()
                .native_world_target_vitals.remove(&0);
        });
        assert!(!acknowledged_lethal());
        BRIDGE.with(|slot| *slot.borrow_mut() = None);
    }
}

#[cfg(test)]
mod persistent_next_target_tests {
    use super::{begin_next_target, Bridge, BRIDGE};

    #[test]
    fn world_persistent_next_target_preserves_ack_and_monotonic_clock() {
        BRIDGE.with(|state| {
            *state.borrow_mut() = Some(Bridge {
                iteration: 0,
                sequence: 4,
                next_tick_ns: 130_000_000,
                last_tick_ns: 120_000_000,
                last_health: Some(0.0), last_max_health: None, native_world_target_vitals: Default::default(),
                previous_ack_id: "a".repeat(64), source_apl_world_switch: None, observed_two_live_targets: false, old_a_post_switch_dot_acks: 0,
                source_hash: "a".repeat(64),
                event_ordinal: 3,
                pending: Vec::new(),
                iteration_finished: false,
            });
        });
        assert_eq!(
            begin_next_target(120_000_000),
            Err("RUST_WORLD_NEXT_TARGET_CLOCK_REWOUND")
        );
        begin_next_target(121_000_000).unwrap();
        BRIDGE.with(|state| {
            let state = state.borrow();
            let bridge = state.as_ref().unwrap();
            assert_eq!(bridge.iteration, 1);
            assert_eq!(bridge.sequence, 4);
            assert_eq!(bridge.last_tick_ns, 120_000_000);
            assert_eq!(bridge.next_tick_ns, 121_000_000);
            assert_eq!(bridge.previous_ack_id, "a".repeat(64));
            assert_eq!(bridge.event_ordinal, 3);
            assert_eq!(bridge.last_health, None);
        });
        BRIDGE.with(|state| *state.borrow_mut() = None);
    }
}

#[cfg(test)]
mod rolling_target_identity_tests {
    use super::{append_damage_event, Bridge, DamageInput};
    use crate::core::fight::{Side, MAX_TARGETS};

    fn empty_world() -> Bridge {
        Bridge {
            iteration: 0,
            sequence: 1,
            next_tick_ns: 0,
            last_tick_ns: -1,
            last_health: Some(100.0), last_max_health: None, native_world_target_vitals: Default::default(),
            previous_ack_id: "a".repeat(64), source_apl_world_switch: None, observed_two_live_targets: false, old_a_post_switch_dot_acks: 0,
            source_hash: "b".repeat(64),
            event_ordinal: 0,
            pending: Vec::new(),
            iteration_finished: false,
        }
    }

    fn native_dot(target_index: u32, action: &str) -> DamageInput {
        DamageInput {
            time_ns: 1_000_000,
            damage: 10.0,
            crit: false,
            landed: true,
            periodic: true,
            target_index,
            action: action.to_owned(),
            target_label: format!("target-{target_index}"),
            caster_type: "player",
            caster_index: 0,
            caster_label: "Warlock".to_owned(),
        }
    }

    #[test]
    fn overlapping_old_and_new_target_dots_keep_distinct_world_indices() {
        let mut state = empty_world();
        let first = Side::Target.target_position().unwrap() as u32;
        let second = Side::Extra(0).target_position().unwrap() as u32;
        assert_eq!((first, second), (0, 1));
        append_damage_event(&mut state, native_dot(first, "Corruption first"));
        append_damage_event(&mut state, native_dot(second, "Corruption second"));
        assert_eq!(state.pending.len(), 2);
        assert_eq!(state.pending[0].native_target_index, 0);
        assert_eq!(state.pending[0].native_target_unit_index, 0);
        assert_eq!(state.pending[1].native_target_index, 1);
        assert_eq!(state.pending[1].native_target_unit_index, 1);
        assert_ne!(state.pending[0].event_id, state.pending[1].event_id);
        let wire = serde_json::to_value(&state.pending).unwrap();
        assert_eq!(wire[0]["native_target_index"].as_u64(), Some(0));
        assert_eq!(wire[1]["native_target_index"].as_u64(), Some(1));
        assert_eq!(wire[1]["periodic"].as_bool(), Some(true));
    }

    #[test]
    #[should_panic(expected = "RUST_WORLD_DAMAGE_TARGET_OUT_OF_RANGE")]
    fn unprepared_target_index_is_not_silently_attributed_to_first_mob() {
        append_damage_event(&mut empty_world(), native_dot(MAX_TARGETS as u32, "invalid"));
    }
}

#[cfg(test)]
mod native_multi_target_ack_ledger_tests {
    use super::{validate_native_target_damage_acks, DamageEvent};
    use serde_json::json;

    fn dot(index: u32, damage: f64) -> DamageEvent {
        DamageEvent {
            event_id: format!("native-effect-{}", index + 1),
            native_time_ns: 100_000_000, damage,
            source_action_id: "Corruption".into(), rank: 1, landed: true,
            crit: false, periodic: true, native_target_index: index,
            native_target_unit_index: index,
            native_target_label: format!("mob-{index}"),
            native_caster_type: "player".into(), native_caster_unit_index: 0,
            native_caster_label: "Warlock".into(),
        }
    }

    #[test]
    fn separate_old_dot_and_new_target_damage_need_distinct_world_lethal_acks() {
        let ack = json!({
            "selected_native_target_index": 1,
            "native_target_health_acks": [
                {"native_target_index": 0, "world_target_id": "old-mob",
                 "world_target_health_before": 15.0, "world_target_health_after": 0.0,
                 "world_target_max_health": 100.0},
                {"native_target_index": 1, "world_target_id": "new-mob",
                 "world_target_health_before": 100.0, "world_target_health_after": 70.0,
                 "world_target_max_health": 100.0}
            ]
        });
        validate_native_target_damage_acks(
            &[dot(0, 15.0), dot(1, 30.0)], &ack, 100.0, 100.0, 70.0);
    }

    #[test]
    fn only_old_mob_dot_can_kill_old_mob_after_new_target_is_selected() {
        // The selected new mob stays at 100 HP. Only the old mob's real
        // gsim-owned DoT damage is lethal; no native forecast or fake kill.
        let ack = json!({
            "selected_native_target_index": 1,
            "native_target_health_acks": [
                {"native_target_index": 0, "world_target_id": "old-mob",
                 "world_target_health_before": 15.0, "world_target_health_after": 0.0,
                 "world_target_max_health": 100.0}
            ]
        });
        validate_native_target_damage_acks(
            &[dot(0, 15.0)], &ack, 100.0, 100.0, 100.0);
    }

    #[test]
    fn old_target_dot_after_its_acknowledged_death_is_an_id_bound_noop() {
        // A native periodic effect is still scheduled for target A while B
        // is alive. Gsim has previously ACKed A's death; the next source
        // effect must not be laundered into B or revive the original mob.
        let mut known = std::collections::BTreeMap::new();
        let first = json!({"native_target_health_acks": [
            {"native_target_index": 0, "world_target_id": "old-mob",
             "world_target_health_before": 15.0, "world_target_health_after": 0.0,
             "world_target_max_health": 100.0}
        ]});
        super::validate_world_target_continuity(&mut known, &first);
        let ack = json!({
            "selected_native_target_index": 1,
            "native_target_health_acks": [
                {"native_target_index": 0, "world_target_id": "old-mob",
                 "world_target_health_before": 0.0, "world_target_health_after": 0.0,
                 "world_target_max_health": 100.0}
            ]
        });
        validate_native_target_damage_acks(
            &[dot(0, 10.0)], &ack, 100.0, 100.0, 100.0);
        super::validate_world_target_continuity(&mut known, &ack);
        assert_eq!(known.get(&0).unwrap().1, 0.0);
        assert_eq!(known.get(&0).unwrap().0, "old-mob");
        assert!(!known.contains_key(&1));
    }

    #[test]
    #[should_panic(expected = "RUST_WORLD_MULTI_TARGET_ZERO_HP_NOT_PREVIOUSLY_ACKED")]
    fn previously_unbound_dead_target_cannot_forge_a_lethal_receipt() {
        let ack = json!({
            "selected_native_target_index": 1,
            "native_target_health_acks": [
                {"native_target_index": 0, "world_target_id": "unbound-mob",
                 "world_target_health_before": 0.0, "world_target_health_after": 0.0,
                 "world_target_max_health": 100.0}
            ]
        });
        validate_native_target_damage_acks(
            &[dot(0, 10.0)], &ack, 100.0, 100.0, 100.0);
        super::validate_world_target_continuity(
            &mut std::collections::BTreeMap::new(), &ack);
    }

    #[test]
    #[should_panic(expected = "RUST_WORLD_MULTI_TARGET_ACK_MISSING")]
    fn only_old_mob_dot_must_not_use_new_mobs_scalar_hp_ack() {
        // Previously this exact event set used the serial fast path and
        // falsely accepted a 15-HP loss on B as old A's DoT receipt.
        validate_native_target_damage_acks(
            &[dot(0, 15.0)],
            &json!({"selected_native_target_index": 1}),
            100.0, 100.0, 85.0);
    }

    #[test]
    #[should_panic(expected = "RUST_WORLD_MULTI_TARGET_ACK_MISSING")]
    fn selected_new_mob_hit_cannot_use_scalar_without_world_id() {
        validate_native_target_damage_acks(
            &[dot(1, 30.0)],
            &json!({"selected_native_target_index": 1}),
            100.0, 100.0, 70.0);
    }

    #[test]
    #[should_panic(expected = "RUST_WORLD_MULTI_TARGET_ACK_MISSING")]
    fn scalar_world_ack_cannot_launder_old_mob_dot_into_current_target() {
        validate_native_target_damage_acks(
            &[dot(0, 15.0), dot(1, 30.0)],
            &json!({"selected_native_target_index": 1}),
            100.0, 100.0, 55.0);
    }

    #[test]
    #[should_panic(expected = "RUST_WORLD_MULTI_TARGET_ACK_WORLD_ID_NOT_DISTINCT")]
    fn two_native_indices_cannot_both_claim_one_gsim_mob() {
        let ack = json!({
            "selected_native_target_index": 1,
            "native_target_health_acks": [
                {"native_target_index": 0, "world_target_id": "same",
                 "world_target_health_before": 15.0, "world_target_health_after": 0.0,
                 "world_target_max_health": 100.0},
                {"native_target_index": 1, "world_target_id": "same",
                 "world_target_health_before": 100.0, "world_target_health_after": 70.0,
                 "world_target_max_health": 100.0}
            ]
        });
        validate_native_target_damage_acks(
            &[dot(0, 15.0), dot(1, 30.0)], &ack, 100.0, 100.0, 70.0);
    }
}

#[cfg(test)]
mod persistent_multi_world_identity_tests {
    use super::validate_world_target_continuity;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn row(index: u32, id: &str, before: f64, after: f64) -> serde_json::Value {
        json!({"native_target_index": index, "world_target_id": id,
               "world_target_health_before": before,
               "world_target_health_after": after, "world_target_max_health": 100.0})
    }

    #[test]
    fn old_dead_mob_and_new_live_mob_keep_distinct_stable_world_vitals() {
        let mut known = BTreeMap::new();
        validate_world_target_continuity(&mut known, &json!({
            "native_target_health_acks": [
                row(0, "old-world-id", 15.0, 0.0),
                row(1, "new-world-id", 100.0, 70.0)]
        }));
        validate_world_target_continuity(&mut known, &json!({
            "native_target_health_acks": [row(1, "new-world-id", 70.0, 50.0)]
        }));
        assert_eq!(known.get(&0).unwrap().1, 0.0);
        assert_eq!(known.get(&1).unwrap().1, 50.0);
        assert_ne!(known.get(&0).unwrap().0, known.get(&1).unwrap().0);
    }

    #[test]
    #[should_panic(expected = "RUST_WORLD_MULTI_TARGET_WORLD_ID_CHANGED")]
    fn one_native_index_cannot_rebind_to_another_world_mob() {
        let mut known = BTreeMap::new();
        validate_world_target_continuity(&mut known,
            &json!({"native_target_health_acks": [row(0, "mob-a", 100.0, 50.0)]}));
        validate_world_target_continuity(&mut known,
            &json!({"native_target_health_acks": [row(0, "mob-b", 50.0, 25.0)]}));
    }

    #[test]
    #[should_panic(expected = "RUST_WORLD_MULTI_TARGET_WORLD_ID_REASSIGNED")]
    fn a_world_mob_cannot_move_between_native_indices() {
        let mut known = BTreeMap::new();
        validate_world_target_continuity(&mut known,
            &json!({"native_target_health_acks": [row(0, "mob-a", 100.0, 50.0)]}));
        validate_world_target_continuity(&mut known,
            &json!({"native_target_health_acks": [row(1, "mob-a", 100.0, 25.0)]}));
    }

    #[test]
    #[should_panic(expected = "RUST_WORLD_MULTI_TARGET_HP_NOT_CONTINUOUS")]
    fn unchanged_world_id_cannot_reset_old_dot_target_hp() {
        let mut known = BTreeMap::new();
        validate_world_target_continuity(&mut known,
            &json!({"native_target_health_acks": [row(0, "mob-a", 100.0, 50.0)]}));
        validate_world_target_continuity(&mut known,
            &json!({"native_target_health_acks": [row(0, "mob-a", 100.0, 25.0)]}));
    }

    #[test]
    #[should_panic(expected = "RUST_WORLD_MULTI_TARGET_LEDGER_DISAPPEARED")]
    fn after_multi_target_world_id_is_learned_scalar_ack_is_not_enough() {
        let mut known = BTreeMap::new();
        validate_world_target_continuity(&mut known,
            &json!({"native_target_health_acks": [row(0, "mob-a", 100.0, 50.0)]}));
        validate_world_target_continuity(&mut known, &json!({}));
    }
}

#[cfg(test)]
mod acked_world_target_hp_tests {
    use super::{current_target_health_percent, begin_next_target, Bridge, BRIDGE};

    #[test]
    fn target_health_requires_matching_committed_world_ack_and_resets_on_new_target() {
        BRIDGE.with(|slot| *slot.borrow_mut() = Some(Bridge {
            iteration: 0, sequence: 2, next_tick_ns: 200,
            last_tick_ns: 100, last_health: Some(50.0),
            last_max_health: Some(100.0), native_world_target_vitals: Default::default(), previous_ack_id: "a".repeat(64), source_apl_world_switch: None, observed_two_live_targets: false, old_a_post_switch_dot_acks: 0,
            source_hash: "b".repeat(64), event_ordinal: 0,
            pending: Vec::new(), iteration_finished: false,
        }));
        assert_eq!(current_target_health_percent(), Some(0.5));
        BRIDGE.with(|slot| slot.borrow_mut().as_mut().unwrap().previous_ack_id.clear());
        assert_eq!(current_target_health_percent(), None);
        BRIDGE.with(|slot| {
            let mut slot = slot.borrow_mut();
            let world = slot.as_mut().unwrap();
            world.previous_ack_id = "a".repeat(64);
            world.last_health = Some(0.0);
        });
        assert_eq!(current_target_health_percent(), Some(0.0));
        assert_eq!(begin_next_target(200), Ok(()));
        assert_eq!(current_target_health_percent(), None);
        BRIDGE.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    fn invalid_or_uncommitted_world_health_is_not_an_apl_threshold() {
        BRIDGE.with(|slot| *slot.borrow_mut() = Some(Bridge {
            iteration: 0, sequence: 0, next_tick_ns: 0,
            last_tick_ns: -1, last_health: Some(50.0),
            last_max_health: Some(100.0), native_world_target_vitals: Default::default(), previous_ack_id: "a".repeat(64), source_apl_world_switch: None, observed_two_live_targets: false, old_a_post_switch_dot_acks: 0,
            source_hash: "b".repeat(64), event_ordinal: 0,
            pending: Vec::new(), iteration_finished: false,
        }));
        assert_eq!(current_target_health_percent(), None);
        BRIDGE.with(|slot| {
            let mut slot = slot.borrow_mut();
            let world = slot.as_mut().unwrap();
            world.sequence = 1;
            world.last_health = Some(101.0);
        });
        assert_eq!(current_target_health_percent(), None);
        BRIDGE.with(|slot| {
            let mut slot = slot.borrow_mut();
            let world = slot.as_mut().unwrap();
            world.last_health = Some(f64::NAN);
        });
        assert_eq!(current_target_health_percent(), None);
        BRIDGE.with(|slot| *slot.borrow_mut() = None);
    }
}

#[cfg(test)]
mod disabled_bridge_tests {
    use super::{enabled, record_damage, DamageInput};

    #[test]
    fn disabled_bridge_ignores_damage_before_validation() {
        if enabled() {
            return;
        }
        record_damage(DamageInput {
            time_ns: 0, damage: f64::NAN, crit: false, landed: false,
            periodic: false, target_index: 0, action: "disabled".to_owned(),
            target_label: "target".to_owned(), caster_type: "player",
            caster_index: 0, caster_label: "Druid".to_owned(),
        });
    }
}

#[cfg(test)]
mod iteration_teardown_tests {
    use super::{
        append_damage_event, assert_damage_acknowledgement, begin_iteration,
        mark_iteration_finished, Bridge, DamageInput, BRIDGE,
    };

    fn bridge(iteration_finished: bool) -> Bridge {
        Bridge {
            iteration: 0,
            sequence: 1,
            next_tick_ns: 10,
            last_tick_ns: 5,
            last_health: Some(100.0), last_max_health: None, native_world_target_vitals: Default::default(),
            previous_ack_id: "a".repeat(64), source_apl_world_switch: None, observed_two_live_targets: false, old_a_post_switch_dot_acks: 0,
            source_hash: "b".repeat(64),
            event_ordinal: 0,
            pending: Vec::new(),
            iteration_finished,
        }
    }

    #[test]
    fn cleanup_damage_is_acknowledged_before_next_iteration() {
        BRIDGE.with(|state| *state.borrow_mut() = Some(bridge(false)));

        // cleanup() can emit final native damage. It must remain pending until
        // the terminal exchange sends it and verifies the matching world ACK.
        BRIDGE.with(|state| {
            let mut state = state.borrow_mut();
            let bridge = state.as_mut().unwrap();
            append_damage_event(
                bridge,
                DamageInput {
                    time_ns: 10, damage: 5.0, crit: false, landed: true,
                    periodic: false, target_index: 0, action: "cleanup-proc".to_owned(),
                    target_label: "target".to_owned(), caster_type: "player",
                    caster_index: 0, caster_label: "Druid".to_owned(),
                },
            );
            assert_eq!(bridge.pending.len(), 1);
            assert_eq!(bridge.event_ordinal, 1);
        });

        // This is the same take-and-ACK sequence used by exchange().
        let events = BRIDGE.with(|state| {
            let mut state = state.borrow_mut();
            std::mem::take(&mut state.as_mut().unwrap().pending)
        });
        let event_ids = events
            .iter()
            .map(|event| event.event_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(event_ids, vec!["native-effect-1"]);
        let accepted = event_ids
            .iter()
            .cloned()
            .map(serde_json::Value::String)
            .collect::<Vec<_>>();
        assert_damage_acknowledgement(&event_ids, &accepted);

        // Only after the matching ACK may finish close the iteration and the
        // next stochastic iteration begin.
        mark_iteration_finished();
        begin_iteration(1);
        BRIDGE.with(|state| {
            let state = state.borrow();
            let bridge = state.as_ref().unwrap();
            assert!(!bridge.iteration_finished);
            assert!(bridge.pending.is_empty());
        });

        // The next iteration accepts its own combat damage normally.
        BRIDGE.with(|state| {
            let mut state = state.borrow_mut();
            let bridge = state.as_mut().unwrap();
            append_damage_event(
                bridge,
                DamageInput {
                    time_ns: 20, damage: 7.0, crit: false, landed: true,
                    periodic: false, target_index: 0, action: "combat-proc".to_owned(),
                    target_label: "target".to_owned(), caster_type: "player",
                    caster_index: 0, caster_label: "Druid".to_owned(),
                },
            );
            assert_eq!(bridge.pending.len(), 1);
            assert_eq!(bridge.pending[0].source_action_id, "combat-proc");
        });
        BRIDGE.with(|state| *state.borrow_mut() = None);
    }

    #[test]
    #[should_panic(expected = "RUST_WORLD_DAMAGE_ACK_EVENTS_MISMATCH")]
    fn terminal_flush_rejects_an_unacknowledged_cleanup_event() {
        assert_damage_acknowledgement(&["native-effect-1".to_owned()], &[]);
    }

    #[test]
    #[should_panic(expected = "RUST_WORLD_DAMAGE_AFTER_LETHAL_ACK")]
    fn cleanup_damage_after_lethal_ack_is_rejected() {
        let mut state = bridge(false);
        state.last_health = Some(0.0);
        state.previous_ack_id = "a".repeat(64);
        append_damage_event(
            &mut state,
                DamageInput {
                    time_ns: 10, damage: 5.0, crit: false, landed: true,
                    periodic: false, target_index: 0, action: "late-proc".to_owned(),
                    target_label: "target".to_owned(), caster_type: "player",
                    caster_index: 0, caster_label: "Druid".to_owned(),
                },
            );
    }

    #[test]
    #[should_panic(expected = "RUST_WORLD_DAMAGE_AFTER_FINISH")]
    fn damage_after_finished_iteration_is_rejected_not_dropped() {
        BRIDGE.with(|state| *state.borrow_mut() = Some(bridge(true)));
        BRIDGE.with(|state| {
            let mut state = state.borrow_mut();
            append_damage_event(
                state.as_mut().unwrap(),
                DamageInput {
                    time_ns: 10, damage: 5.0, crit: false, landed: true,
                    periodic: false, target_index: 0, action: "late-proc".to_owned(),
                    target_label: "target".to_owned(), caster_type: "player",
                    caster_index: 0, caster_label: "Druid".to_owned(),
                },
            );
        });
    }

    #[test]
    #[should_panic(expected = "RUST_WORLD_PREVIOUS_ITERATION_NOT_FINISHED")]
    fn next_iteration_requires_a_finished_previous_iteration() {
        BRIDGE.with(|state| *state.borrow_mut() = Some(bridge(false)));
        begin_iteration(1);
    }
}
