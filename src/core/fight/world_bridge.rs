//! Synchronous source-Rust fight ↔ actual gsim world boundary.
//! Activated only by FOREVER_GSIM_WORLD_BRIDGE=1. All damage comes from the
//! unchanged native Fight::deal_damage; the world applies it before native
//! scheduler resumes. This is a bounded multi-fight diagnostic, NOT P7 acceptance.
use std::{cell::RefCell, io::{self, BufRead, Write}};
use serde::Serialize;

// Native 2D geometry: parity-tested against the existing gsim scenario.
// External JSON bridge remains authoritative until full event/runtime parity.
#[allow(dead_code)] // Geometry is staged: no in-process backend may claim parity yet.
mod spatial;
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
    // `finish` closes the encounter before Fight::cleanup() tears down auras/pets.
    // Teardown callbacks must not enqueue events for the next stochastic iteration.
    iteration_finished: bool,
}
thread_local! {
    static BRIDGE: RefCell<Option<Bridge>> = const { RefCell::new(None) };
    static NATIVE_SPATIAL: RefCell<Option<spatial::NativeSpatialWorld>> =
        const { RefCell::new(None) };
}

pub(crate) fn enabled() -> bool {
    std::env::var("FOREVER_GSIM_WORLD_BRIDGE").ok().as_deref() == Some("1")
        || NATIVE_SPATIAL.with(|state| state.borrow().is_some())
}

/// Start one retained native actor with an in-process 2D mob ledger. Every
/// applied event originates from Fight::deal_damage; no gsim stdin is needed.
pub(crate) fn start_inprocess(
    source_hash: &str,
    target_count: usize,
    target_health: f64,
    engagement_range_yards: f64,
) -> Result<(), &'static str> {
    // A native route execution must never silently replace a previous
    // in-flight source/ACK chain or an external gsim world peer. In particular,
    // overwriting either thread-local state would let the next evaluator
    // attribute retained actor/world observations to the wrong source.
    if std::env::var("FOREVER_GSIM_WORLD_BRIDGE").ok().as_deref() == Some("1") {
        return Err("NATIVE_SPATIAL_EXTERNAL_GSIM_BRIDGE_CONFLICT");
    }
    if BRIDGE.with(|slot| slot.borrow().is_some())
        || NATIVE_SPATIAL.with(|slot| slot.borrow().is_some())
    {
        return Err("NATIVE_SPATIAL_BRIDGE_ALREADY_ACTIVE");
    }
    // Validate the existing native source-hash contract *before* installing
    // either thread-local. Otherwise start() panics after NATIVE_SPATIAL is
    // installed and leaves a half-started world behind.
    if source_hash.len() != 64 {
        return Err("RUST_WORLD_INVALID_SOURCE");
    }
    let world = spatial::NativeSpatialWorld::new(
        target_count, target_health, engagement_range_yards,
    )?;
    NATIVE_SPATIAL.with(|slot| *slot.borrow_mut() = Some(world));
    start(source_hash);
    Ok(())
}

pub(crate) fn stop_inprocess() {
    // A rejected in-process start never owns the external gsim BRIDGE.
    // In particular, its cleanup must not discard a paid external peer's
    // source hash, event journal, or lethal ACK lineage.
    if !inprocess_enabled() {
        return;
    }
    NATIVE_SPATIAL.with(|slot| *slot.borrow_mut() = None);
    BRIDGE.with(|slot| *slot.borrow_mut() = None);
}

#[cfg(test)]
mod inprocess_bridge_lifetime_tests {
    use super::*;

    #[test]
    fn nested_native_spatial_start_refuses_without_overwriting_paid_source() {
        let original = "a".repeat(64);
        let competing = "b".repeat(64);
        start_inprocess(&original, 2, 80.0, 5.0).unwrap();
        begin_iteration(0);
        let before = inprocess_position_receipt().unwrap();

        // Before this guard, start_inprocess silently replaced both source
        // identity and the target ledger even though the first run was live.
        assert_eq!(
            start_inprocess(&competing, 3, 140.0, 10.0),
            Err("NATIVE_SPATIAL_BRIDGE_ALREADY_ACTIVE"),
        );
        BRIDGE.with(|slot| {
            let state = slot.borrow();
            let active = state.as_ref().unwrap();
            assert_eq!(active.source_hash, original);
            assert_eq!(active.iteration, 0);
        });
        assert_eq!(inprocess_position_receipt(), Some(before));
        stop_inprocess();

        // Explicit teardown permits a new, independent source-native request.
        start_inprocess(&competing, 3, 140.0, 10.0).unwrap();
        BRIDGE.with(|slot| {
            assert_eq!(slot.borrow().as_ref().unwrap().source_hash, competing);
        });
        stop_inprocess();
    }

    #[test]
    fn rejected_native_start_cleanup_preserves_external_paid_ack_session() {
        let valid_source = "c".repeat(64);
        // Invalid source identifiers were previously rejected by start()
        // only *after* installing the spatial ledger.
        assert_eq!(
            start_inprocess("short", 2, 80.0, 5.0),
            Err("RUST_WORLD_INVALID_SOURCE"),
        );
        assert!(!inprocess_enabled());
        assert!(BRIDGE.with(|slot| slot.borrow().is_none()));

        // An external gsim peer already has paid ACK ancestry. A rejected
        // attempt to start native-inprocess must not erase it on teardown.
        BRIDGE.with(|slot| *slot.borrow_mut() = Some(Bridge {
            iteration: 4,
            sequence: 7,
            next_tick_ns: 900,
            last_tick_ns: 800,
            last_health: Some(0.0),
            previous_ack_id: "a".repeat(64),
            source_hash: valid_source.clone(),
            event_ordinal: 13,
            pending: Vec::new(),
            iteration_finished: false,
        }));
        assert_eq!(
            start_inprocess(&"d".repeat(64), 2, 80.0, 5.0),
            Err("NATIVE_SPATIAL_BRIDGE_ALREADY_ACTIVE"),
        );
        stop_inprocess(); // Must not tear down the external bridge.
        BRIDGE.with(|slot| {
            let state = slot.borrow();
            let external = state.as_ref().expect("external ACK peer survived");
            assert_eq!(external.source_hash, valid_source);
            assert_eq!(external.iteration, 4);
            assert_eq!(external.sequence, 7);
            assert_eq!(external.last_tick_ns, 800);
            assert_eq!(external.previous_ack_id, "a".repeat(64));
        });
        assert!(!inprocess_enabled());
        BRIDGE.with(|slot| *slot.borrow_mut() = None);

        // A failed source check must not poison the next valid in-process run.
        start_inprocess(&"e".repeat(64), 2, 80.0, 5.0).unwrap();
        assert!(inprocess_enabled());
        stop_inprocess();
        assert!(BRIDGE.with(|slot| slot.borrow().is_none()));
    }
}

/// Physical walking time since the previous acknowledged target; this must
/// be charged on the SAME native clock before combat resumes.
pub(crate) fn inprocess_enabled() -> bool {
    NATIVE_SPATIAL.with(|state| state.borrow().is_some())
}

pub(crate) fn inprocess_travel_ns() -> i64 {
    NATIVE_SPATIAL.with(|state| state.borrow().as_ref().map_or(0, |world|
        world.last_walk_ns()
    ))
}

/// Native 100ms boundary observations during paid walking (not estimated steps).
pub(crate) fn inprocess_moving_samples() -> u32 {
    NATIVE_SPATIAL.with(|state| state.borrow().as_ref().map_or(0, |world|
        world.moving_samples()
    ))
}

/// A geometry receipt from the actual in-process route at the current paid
/// native target. Never reconstruct route identity from the pull ordinal.
pub(crate) fn inprocess_position_receipt() -> Option<Value> {
    NATIVE_SPATIAL.with(|slot| {
        slot.borrow().as_ref().map(|world| {
            let target = world.target_position();
            let actor = world.player_position();
            json!({
                "target_source_index": world.target_index(),
                "target_xy_yards": [target.x, target.y],
                "actor_xy_yards": [actor.x, actor.y],
                "distance_yards": world.distance_yards(),
            })
        })
    })
}

pub(crate) fn start(source_hash: &str) {
    assert!(enabled() && source_hash.len() == 64, "RUST_WORLD_INVALID_SOURCE");
    BRIDGE.with(|state| *state.borrow_mut() = Some(Bridge {
        iteration: 0, sequence: 0, next_tick_ns: 0, last_tick_ns: -1,
        last_health: None, previous_ack_id: String::new(),
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
        if bridge.sequence == 0 || bridge.previous_ack_id.len() != 64
            || bridge.last_health != Some(0.0) {
            return Err("RUST_WORLD_NEXT_TARGET_NOT_NATIVE_LETHAL");
        }
        // The source-native lethal ACK authorizes this precise spatial
        // transition. Refuse a route advance before any native lethal.
        NATIVE_SPATIAL.with(|slot| {
            if let Some(world) = slot.borrow_mut().as_mut() {
                world.next_target(at_ns)?;
            }
            Ok::<(), &'static str>(())
        })?;
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
            last_tick_ns: 100, last_health: Some(10.0),
            previous_ack_id: "a".repeat(64), source_hash: "b".repeat(64),
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

/// Identifies the real native effect and its caster without positional metadata swaps.
/// The world bridge only transports events; it never invents combat damage.
pub(crate) struct DamageSource {
    pub(crate) action: String,
    /// Source-native target position: Target=0, Extra(k)=k+1.
    pub(crate) target_index: u32,
    pub(crate) target_label: String,
    pub(crate) caster_type: String,
    pub(crate) caster_index: u32,
    pub(crate) caster_label: String,
}

pub(crate) fn record_damage(
    time_ns: i64,
    damage: f64,
    crit: bool,
    landed: bool,
    periodic: bool,
    source: DamageSource,
) {
    if !enabled() {
        return;
    }
    assert!(damage.is_finite() && damage > 0.0 && landed, "RUST_WORLD_DAMAGE_INVALID");
    BRIDGE.with(|state| {
        let mut state = state.borrow_mut();
        let bridge = state.as_mut().expect("RUST_WORLD_NOT_STARTED");
        append_damage_event(bridge, time_ns, damage, crit, landed, periodic, source);
    });
}

fn append_damage_event(
    bridge: &mut Bridge,
    time_ns: i64,
    damage: f64,
    crit: bool,
    landed: bool,
    periodic: bool,
    source: DamageSource,
) {
    let DamageSource {
        action,
        target_index,
        target_label,
        caster_type,
        caster_index,
        caster_label,
    } = source;
    assert!(!bridge.iteration_finished, "RUST_WORLD_DAMAGE_AFTER_FINISH");
    assert!(!has_lethal_ack(bridge), "RUST_WORLD_DAMAGE_AFTER_LETHAL_ACK");
    assert!(target_index < super::MAX_TARGETS as u32,
        "RUST_WORLD_DAMAGE_TARGET_OUT_OF_RANGE");
    bridge.event_ordinal += 1;
    bridge.pending.push(DamageEvent {
        event_id: format!("native-effect-{}", bridge.event_ordinal),
        native_time_ns: time_ns,
        damage,
        source_action_id: action,
        rank: 0,
        landed,
        crit,
        periodic,
        native_target_index: target_index,
        native_target_unit_index: target_index,
        native_target_label: target_label,
        native_caster_type: caster_type,
        native_caster_unit_index: caster_index,
        native_caster_label: caster_label,
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
    // In-process ACK: consume the exact same native damage event journal,
    // maintaining a real per-mob health ledger, without JSON/stdin round trips.
    // External gsim ACK handling below remains byte-for-byte unchanged.
    let native = NATIVE_SPATIAL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(world) = slot.as_mut() else { return None };
        // Real native clock advances 2D position; Fight sees moving=true
        // until the paid walking interval completes.
        let moving = world.advance_to(clock_ns);
        let distance = world.distance_yards();
        for event in &events {
            // The current in-process evaluator has one live mob health bar.
            // It must not turn an Extra(k) DoT into damage against that mob;
            // multi-alive targets belong to a future expanded spatial ledger.
            assert_eq!(event.native_target_index, 0,
                "NATIVE_SPATIAL_UNSUPPORTED_EXTRA_TARGET_DAMAGE");
            world.apply_native_damage(event.damage)
                .expect("NATIVE_SPATIAL_SOURCE_DAMAGE_INVALID");
        }
        Some((distance, world.target_health(), moving))
    });
    if let Some((distance, health, moving)) = native {
        assert!(distance.is_finite() && (0.0..=100.0).contains(&distance),
            "NATIVE_SPATIAL_DISTANCE_INVALID");
        bridge.last_health = Some(health);
        // Native source SHA is an internal ACK token here, not a fabricated
        // gsim world SHA. The monotonically increasing sequence and event
        // ledger are both retained for the next real target.
        bridge.previous_ack_id = bridge.source_hash.clone();
        bridge.last_tick_ns = clock_ns;
        bridge.sequence += 1;
        bridge.next_tick_ns = i64::MAX;
        BRIDGE.with(|state| *state.borrow_mut() = Some(bridge));
        return (distance, true, moving);
    }
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
    assert_damage_acknowledgement(&event_ids, accepted);
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

fn has_lethal_ack(bridge: &Bridge) -> bool {
    bridge.sequence > 0
        && bridge.last_health == Some(0.0)
        && bridge.previous_ack_id.len() == 64
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
    if NATIVE_SPATIAL.with(|state| state.borrow().is_some()) {
        return;
    }
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
                last_health: Some(0.0),
                previous_ack_id: "a".repeat(64),
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
fn test_damage_source(action: &str) -> DamageSource {
    DamageSource {
        action: action.to_owned(),
        target_index: 0,
        target_label: "target".to_owned(),
        caster_type: "player".to_owned(),
        caster_index: 0,
        caster_label: "Druid".to_owned(),
    }
}

#[cfg(test)]
mod source_world_target_identity_tests {
    use super::{append_damage_event, test_damage_source, Bridge};
    #[test]
    fn world_damage_receipt_uses_true_native_target_index() {
        let mut bridge = Bridge {
            iteration: 0, sequence: 1, next_tick_ns: 0, last_tick_ns: -1,
            last_health: Some(100.0), previous_ack_id: "a".repeat(64),
            source_hash: "b".repeat(64), event_ordinal: 0,
            pending: Vec::new(), iteration_finished: false,
        };
        let first = test_damage_source("first DoT");
        let mut second = test_damage_source("second DoT");
        second.target_index = 1;
        second.target_label = "second mob".into();
        append_damage_event(&mut bridge, 1_000_000, 5.0, false, true, true, first);
        append_damage_event(&mut bridge, 2_000_000, 7.0, false, true, true, second);
        assert_eq!(bridge.pending[0].native_target_index, 0);
        assert_eq!(bridge.pending[1].native_target_index, 1);
        assert_eq!(bridge.pending[1].native_target_unit_index, 1);
        assert_eq!(bridge.pending[1].native_target_label, "second mob");
    }
}

#[cfg(test)]
mod disabled_bridge_tests {
    use super::{enabled, record_damage, test_damage_source};

    #[test]
    fn disabled_bridge_ignores_damage_before_validation() {
        if enabled() {
            return;
        }
        record_damage(0, f64::NAN, false, false, false, test_damage_source("disabled"));
    }
}

#[cfg(test)]
mod iteration_teardown_tests {
    use super::{
        append_damage_event, assert_damage_acknowledgement, begin_iteration,
        mark_iteration_finished, test_damage_source, Bridge, BRIDGE,
    };

    fn bridge(iteration_finished: bool) -> Bridge {
        Bridge {
            iteration: 0,
            sequence: 1,
            next_tick_ns: 10,
            last_tick_ns: 5,
            last_health: Some(100.0),
            previous_ack_id: "a".repeat(64),
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
                10,
                5.0,
                false,
                true,
                false,
                test_damage_source("cleanup-proc"),
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
                20,
                7.0,
                false,
                true,
                false,
                test_damage_source("combat-proc"),
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
                10,
                5.0,
                false,
                true,
                false,
                test_damage_source("late-proc"),
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
                10,
                5.0,
                false,
                true,
                false,
                test_damage_source("late-proc"),
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

#[cfg(test)]
mod inprocess_spatial_ack_tests {
    use super::*;

    #[test]
    #[should_panic(expected = "NATIVE_SPATIAL_UNSUPPORTED_EXTRA_TARGET_DAMAGE")]
    fn inprocess_world_refuses_noncurrent_source_target_damage() {
        let source = "f".repeat(64);
        start_inprocess(&source, 8, 40.0, 5.0).unwrap();
        begin_iteration(0);
        exchange(0, 100_000_000_000, 0.0);
        let mut extra = test_damage_source("old mob DoT");
        extra.target_index = 1;
        record_damage(1_000_000, 10.0, false, true, true, extra);
        exchange(1_000_000, 100_000_000_000, 10.0);
    }

    #[test]
    fn paid_spatial_transition_requires_actual_native_lethal_damage() {
        let source = "f".repeat(64);
        start_inprocess(&source, 8, 40.0, 5.0).unwrap();
        begin_iteration(0);
        assert_eq!(exchange(0, 100_000_000_000, 0.0).0, 5.0);
        assert_eq!(
            begin_next_target(1_000_000),
            Err("RUST_WORLD_NEXT_TARGET_NOT_NATIVE_LETHAL")
        );

        record_damage(1_000_000, 15.0, false, true, false, DamageSource {
            action: "source-spell".into(), target_index: 0, target_label: "Mob".into(),
            caster_type: "player".into(), caster_index: 0,
            caster_label: "Warlock".into(),
        });
        exchange(1_000_000, 100_000_000_000, 15.0);
        assert!(!acknowledged_lethal());

        record_damage(2_000_000, 30.0, false, true, false, DamageSource {
            action: "source-pet".into(), target_index: 0, target_label: "Mob".into(),
            caster_type: "pet".into(), caster_index: 1,
            caster_label: "Imp".into(),
        });
        exchange(2_000_000, 100_000_000_000, 45.0);
        assert!(acknowledged_lethal());
        begin_next_target(3_000_000).unwrap();
        // World path identity is a source mob id, not the pull ordinal.
        let position = inprocess_position_receipt().unwrap();
        assert_eq!(position["target_source_index"], 1);
        assert_eq!(position["target_xy_yards"], json!([20.0, 0.0]));
        assert_eq!(position["actor_xy_yards"], json!([5.0, 0.0]));
        assert_eq!(position["distance_yards"], 15.0);
        // From x=5 to mob x=20, stop five yards short: pay ten yards
        // on the actual native clock, without an instant position jump.
        let travel = inprocess_travel_ns();
        assert_eq!(travel, (10.0 / 7.0_f64 * 1e9).ceil() as i64);
        assert!(!acknowledged_lethal());
        let (distance, _, moving) = exchange(3_000_000, 100_000_000_000, 45.0);
        assert_eq!(distance, 15.0);
        assert!(moving);
        let (distance, _, moving) =
            exchange(3_000_000 + travel / 2, 100_000_000_000, 45.0);
        assert!(moving && distance > 5.0 && distance < 15.0);
        let (distance, _, moving) =
            exchange(3_000_000 + travel, 100_000_000_000, 45.0);
        assert!(!moving);
        assert!((distance - 5.0).abs() < 1e-8);
        assert!(inprocess_moving_samples() >= 2);
        assert_eq!(inprocess_position_receipt().unwrap()["actor_xy_yards"],
            json!([15.0, 0.0]));
        stop_inprocess();
    }
}
