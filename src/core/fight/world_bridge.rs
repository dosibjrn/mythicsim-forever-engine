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

struct Bridge {
    iteration: u32,
    sequence: u64,
    next_tick_ns: i64,
    last_tick_ns: i64,
    last_health: Option<f64>,
    previous_ack_id: String,
    source_hash: String,
    event_ordinal: u64,
    pending: Vec<DamageEvent>,
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
        last_health: None, previous_ack_id: String::new(),
        source_hash: source_hash.to_owned(), event_ordinal: 0,
        pending: Vec::new(),
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
        assert!(bridge.pending.is_empty(), "RUST_WORLD_UNFLUSHED_EVENTS");
        bridge.iteration = iteration;
        bridge.sequence = 0;
        bridge.next_tick_ns = 0;
        bridge.last_tick_ns = -1;
        bridge.last_health = None;
        bridge.previous_ack_id.clear();
        bridge.event_ordinal = 0;
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
        if bridge.sequence == 0 || bridge.previous_ack_id.len() != 64
            || bridge.last_health != Some(0.0) {
            return Err("RUST_WORLD_NEXT_TARGET_NOT_NATIVE_LETHAL");
        }
        bridge.iteration = bridge.iteration.checked_add(1)
            .ok_or("RUST_WORLD_TARGET_INDEX_OVERFLOW")?;
        bridge.last_health = None; // new world target, not a healed old one
        bridge.next_tick_ns = at_ns;
        // Deliberately preserve sequence, previous_ack_id, last_tick_ns,
        // event_ordinal and source identity across targets.
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
            last_tick_ns: 100, last_health: Some(25.0),
            previous_ack_id: "a".repeat(64), source_hash: "b".repeat(64),
            event_ordinal: 9, pending: Vec::new(),
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
            last_tick_ns: 100, last_health: Some(10.0),
            previous_ack_id: "a".repeat(64), source_hash: "b".repeat(64),
            event_ordinal: 1, pending: Vec::new(),
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

pub(crate) fn record_damage(time_ns: i64, damage: f64, crit: bool, landed: bool,
    periodic: bool, action: String, target_label: String,
    caster_type: &str, caster_index: u32, caster_label: String) {
    if !enabled() { return; }
    assert!(damage.is_finite() && damage > 0.0 && landed, "RUST_WORLD_DAMAGE_INVALID");
    BRIDGE.with(|state| {
        let mut state = state.borrow_mut();
        let bridge = state.as_mut().expect("RUST_WORLD_NOT_STARTED");
        bridge.event_ordinal += 1;
        bridge.pending.push(DamageEvent {
            event_id: format!("native-effect-{}", bridge.event_ordinal),
            native_time_ns: time_ns, damage, source_action_id: action,
            rank: 0, landed, crit, periodic,
            native_target_index: 0, native_target_unit_index: 0,
            native_target_label: target_label,
            native_caster_type: caster_type.to_owned(), native_caster_unit_index: caster_index,
            native_caster_label: caster_label,
        });
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
    }));
    let tick = receive();
    assert!(tick["type"] == "gsim_world_range_tick"
        && tick["sequence"] == bridge.sequence
        && tick["iteration_index"] == bridge.iteration
        && tick["native_clock_ns"] == clock_ns
        && tick["world_source"] == "gsim.CombatRuntime"
        && tick["previous_world_ack_id"] == bridge.previous_ack_id,
        "RUST_WORLD_TICK_IDENTITY_OR_ACK_CHAIN_INVALID");
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
    assert!(before_health > 0.0 && before_health <= max_health,
        "RUST_WORLD_HEALTH_INVALID");
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
    let accepted = ack["applied_event_ids"].as_array()
        .expect("RUST_WORLD_DAMAGE_ACK_EVENT_IDS_MISSING");
    assert!(accepted.len() == event_ids.len()
        && accepted.iter().zip(event_ids.iter()).all(|(a,b)| a.as_str() == Some(b)),
        "RUST_WORLD_DAMAGE_ACK_EVENTS_MISMATCH");
    let ack_before = number(&ack, "world_target_health_before");
    let ack_after = number(&ack, "world_target_health_after");
    let expected_damage: f64 = events.iter().map(|e| e.damage).sum();
    assert!((ack_before - before_health).abs() < 1e-6
        && (ack_after - (before_health - expected_damage).max(0.0)).abs() < 1e-6,
        "RUST_WORLD_DAMAGE_ACK_HEALTH_MISMATCH");
    bridge.previous_ack_id = string(&ack, "world_after_sha256").to_owned();
    assert!(bridge.previous_ack_id.len() == 64, "RUST_WORLD_DAMAGE_ACK_HASH_INVALID");
    bridge.last_health = Some(ack_after);
    bridge.last_tick_ns = clock_ns;
    bridge.sequence += 1;
    bridge.next_tick_ns = next_world_boundary_ns;
    BRIDGE.with(|state| *state.borrow_mut() = Some(bridge));
    (distance, front, moving)
}

pub(crate) fn acknowledged_lethal() -> bool {
    BRIDGE.with(|state| {
        let state = state.borrow();
        let bridge = state.as_ref().expect("RUST_WORLD_NOT_STARTED");
        bridge.sequence > 0 && bridge.last_health == Some(0.0)
            && bridge.previous_ack_id.len() == 64 && bridge.pending.is_empty()
    })
}

pub(crate) fn finish(duration_ns: i64, cumulative_damage: f64) {
    // The terminal world flush is at the next representable gsim millisecond.
    // This is an explicit diagnostic boundary, never a claim of P7 parity.
    let last = BRIDGE.with(|state| state.borrow().as_ref()
        .expect("RUST_WORLD_NOT_STARTED").last_tick_ns);
    if acknowledged_lethal() {
        // gsim has already applied the last native event and killed the mob.
        // Another range tick would address a dead world target and violate
        // the causal ACK sequence. No fabricated terminal world exchange.
        assert!(duration_ns == last, "RUST_WORLD_LETHAL_DURATION_NOT_ACK_CLOCK");
    } else {
        let clock = ((duration_ns + 999_999) / 1_000_000) * 1_000_000;
        let clock = clock.max(last + 1_000_000);
        let _ = exchange(clock, duration_ns, cumulative_damage);
    }
    BRIDGE.with(|state| {
        let state = state.borrow();
        let bridge = state.as_ref().expect("RUST_WORLD_NOT_STARTED");
        assert!(bridge.pending.is_empty(), "RUST_WORLD_FINISH_UNFLUSHED");
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
mod persistent_next_target_tests {
    use super::{begin_next_target, Bridge, BRIDGE};

    #[test]
    fn world_persistent_next_target_preserves_ack_and_monotonic_clock() {
        BRIDGE.with(|state| *state.borrow_mut() = Some(Bridge {
            iteration: 0, sequence: 4, next_tick_ns: 130_000_000,
            last_tick_ns: 120_000_000, last_health: Some(0.0),
            previous_ack_id: "a".repeat(64),
            source_hash: "a".repeat(64), event_ordinal: 3, pending: Vec::new(),
        }));
        assert_eq!(begin_next_target(120_000_000),
            Err("RUST_WORLD_NEXT_TARGET_CLOCK_REWOUND"));
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
