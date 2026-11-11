use forever_engine::{
    contracts::prepared_v2::PreparedV2, gear_options, get_options, native_item_options,
    native_player_context, native_rolling_world_apl_prepared, native_talent_options,
    native_two_pull_windows, native_two_world_targets_with_decisions_recovery,
    native_two_world_targets_with_recovery, prepare_json_with_actor_level, prepared_refusals,
    simulate, simulate_prepared, simulate_prepared_gated, validate_prepared, Gated,
    NativeWarlockWorldSession, OptionsClass, PrepareError, PreparedError, Refusal, Request,
    RngMode, Rules, SOURCE_REVISION,
};
use serde::Deserialize;
use std::{
    env, fs,
    hint::black_box,
    io::Write,
    path::{Path, PathBuf},
    process,
};

/// Reads only the schema version so each contract keeps its own strict parser.
#[derive(Deserialize)]
struct SchemaPeek {
    schema_version: Option<u32>,
}

fn prepared_v2(input: &[u8]) -> Result<Option<PreparedV2>, String> {
    let peek: SchemaPeek =
        serde_json::from_slice(input).map_err(|err| format!("request rejected: {err}"))?;
    if peek.schema_version != Some(2) {
        return Ok(None);
    }
    serde_json::from_slice(input)
        .map(Some)
        .map_err(|err| format!("prepared input rejected: {err}"))
}

/// The process id that keeps concurrent writers' temporary files apart. WebAssembly has no
/// process ids (std panics asking for one), and a host runs one instance per output there.
#[cfg(not(target_family = "wasm"))]
fn process_id() -> u32 {
    process::id()
}

#[cfg(target_family = "wasm")]
fn process_id() -> u32 {
    0
}

/// Writes `contents` to `path` so a reader sees the whole file or none of it.
///
/// The bytes go to a hidden sibling file, which is flushed to disk and then renamed over the
/// destination. A run that is killed or timed out part way, by a signal, a worker's timeout or
/// a full disk, leaves at most that sibling behind, never a truncated report. The sibling's
/// name starts with a dot and ends with `.tmp`, so it is never mistaken for a result.
fn write_atomically(path: &str, contents: &str) -> Result<(), String> {
    let destination = Path::new(path);
    let name = destination
        .file_name()
        .ok_or_else(|| format!("{path} is not a file path"))?;
    let mut sibling = std::ffi::OsString::from(".");
    sibling.push(name);
    sibling.push(format!(".{}.tmp", process_id()));
    let temporary: PathBuf = destination.with_file_name(sibling);
    let written = fs::File::create(&temporary)
        .and_then(|mut file| {
            file.write_all(contents.as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&temporary, destination));
    if let Err(err) = written {
        // Best effort: the error being reported is the write's, not the cleanup's.
        let _ = fs::remove_file(&temporary);
        return Err(err.to_string());
    }
    Ok(())
}

/// What ended a run without its output, and the exit status that tells a caller which.
///
/// Status 1 is any error. With `sim --gate`, the coverage gate's refusal and an input that
/// fails validation have a status of their own, so a worker that runs the gate and the
/// simulation as one process can tell a fallback from a fault without reading messages.
struct Failure {
    status: i32,
    message: Option<String>,
}

/// The exit status of `sim --gate` when the gate refuses a valid input: its report is on
/// standard output, as `check` prints it, and nothing was simulated.
const EXIT_REFUSED: i32 = 3;
/// The exit status of `sim --gate` when the prepared input fails validation.
const EXIT_REJECTED: i32 = 4;
/// The exit status of `prepare` and `sim --request` when Rust preparation does not cover the
/// request: the refusal is on standard output and Go can prepare the request instead.
const EXIT_NOT_PREPARED: i32 = 5;

/// Prepares a request in Rust, or ends with `EXIT_NOT_PREPARED` and the refusal on standard
/// output. A request Rust cannot read is refused too: Go reads protojson itself and decides.
///
/// A panic inside preparation is reported the same way, with the code `prepare_fault`, so a
/// defect in Rust preparation sends the request to Go rather than failing it.
fn prepare_request(
    request: &[u8],
    scenario: &str,
    rules: Rules,
    actor_level: Option<i32>,
) -> Result<serde_json::Value, Failure> {
    let prepared = std::panic::catch_unwind(|| {
        prepare_json_with_actor_level(request, scenario, rules, actor_level)
    })
    .unwrap_or_else(|panic| {
        let reason = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|text| text.to_string()))
            .unwrap_or_else(|| "preparation panicked".to_string());
        Err(PrepareError::Fault(reason))
    });
    prepared.map_err(|err| {
        let (code, reason) = match &err {
            PrepareError::Refused(refusal) => (refusal.code.to_string(), refusal.reason.clone()),
            PrepareError::Invalid(reason) => ("request".to_string(), reason.clone()),
            PrepareError::Fault(reason) => ("prepare_fault".to_string(), reason.clone()),
        };
        println!(
            "{}",
            serde_json::json!({"prepared": false, "refusal": {"code": code, "reason": reason}})
        );
        Failure {
            status: EXIT_NOT_PREPARED,
            message: None,
        }
    })
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Failure {
            status: 1,
            message: Some(message),
        }
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

/// The gate's verdict as `check` prints it.
fn coverage_report(prepared: &PreparedV2, refusals: &[Refusal]) -> Result<String, String> {
    let reasons: Vec<&str> = refusals
        .iter()
        .map(|refusal| refusal.reason.as_str())
        .collect();
    let report = serde_json::json!({
        "scenario_id": prepared.scenario_id,
        "supported": refusals.is_empty(),
        "reasons": reasons,
        "refusals": refusals,
    });
    serde_json::to_string_pretty(&report).map_err(|err| err.to_string())
}

#[derive(Default)]
struct NativeApiSession {
    // Rust owns the prepared protobuf-derived actor, not a JSON file handle.
    prepared: Option<PreparedV2>,
    // Last completed and charged native world measurement, NOT a resumable
    // Fight. Invalidate it before a new candidate starts or prepare is called.
    last_paid_world: Option<serde_json::Value>,
    // Unlike PreparedV2, this owns an actual retained source-native Fight.
    // It remains level-locked: no earned XP/training implementation yet.
    retained_world: Option<NativeWarlockWorldSession>,
}

/// A refused native candidate must carry a machine-readable reason, while
/// retaining the original string error field for existing API consumers.
#[derive(Debug)]
struct NativeApiError {
    code: String,
    reason: String,
}

impl NativeApiError {
    fn new(code: &str, reason: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            reason: reason.into(),
        }
    }
}

impl From<String> for NativeApiError {
    fn from(reason: String) -> Self {
        Self::new("invalid_api_request", reason)
    }
}

impl From<&str> for NativeApiError {
    fn from(reason: &str) -> Self {
        Self::new("invalid_api_request", reason)
    }
}

impl From<PrepareError> for NativeApiError {
    fn from(error: PrepareError) -> Self {
        match error {
            PrepareError::Invalid(reason) => Self::new("invalid_prepare_request", reason),
            PrepareError::Refused(refusal) => Self::new(refusal.code, refusal.reason),
            PrepareError::Fault(reason) => Self::new("native_prepare_fault", reason),
        }
    }
}

/// Preserve the requested sampled actor level. A malformed value must not be
/// interpreted as absent and silently prepare a default L60 character.
fn native_api_actor_level(request: &serde_json::Value) -> Result<Option<i32>, String> {
    let Some(value) = request.get("actor_level") else {
        return Ok(None);
    };
    value
        .as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .filter(|level| (1..=60).contains(level))
        .map(Some)
        .ok_or_else(|| "actor_level must be an integer in 1..=60".to_owned())
}

/// Between-pull rest is a candidate policy, not the terminal full-resource
/// requirement. Absent policy preserves the source's historical 100%/100%
/// between-kill Decisions behavior for old optimizer inputs.
fn native_api_between_pull_thresholds(
    request: &serde_json::Value,
) -> Result<(f64, f64), NativeApiError> {
    let Some(policy) = request.get("between_pulls") else {
        return Ok((1.0, 1.0));
    };
    let object = policy.as_object().ok_or_else(|| {
        NativeApiError::new(
            "invalid_between_pull_policy",
            "between_pulls must contain health_fraction and mana_fraction",
        )
    })?;
    if object.len() != 2
        || !object.contains_key("health_fraction")
        || !object.contains_key("mana_fraction")
    {
        return Err(NativeApiError::new(
            "invalid_between_pull_policy",
            "between_pulls requires exactly health_fraction and mana_fraction",
        ));
    }
    let health = object["health_fraction"].as_f64();
    let mana = object["mana_fraction"].as_f64();
    match (health, mana) {
        (Some(h), Some(m))
            if h.is_finite()
                && m.is_finite()
                && (0.0..=1.0).contains(&h)
                && (0.0..=1.0).contains(&m) =>
        {
            Ok((h, m))
        }
        _ => Err(NativeApiError::new(
            "invalid_between_pull_policy",
            "between_pulls fractions must be finite numbers in 0..=1",
        )),
    }
}

/// The only currently executable source rolling policy: ACKed first-target
/// HP <= 50%, no more than two simultaneously alive native/2D enemies.
/// Refuse unknown inputs before spending combat time; an ignored policy must
/// never be returned as an applied native measurement.
fn native_api_world_apl(
    request: &serde_json::Value,
) -> Result<Option<serde_json::Value>, NativeApiError> {
    let Some(policy) = request.get("world_apl") else {
        return Ok(None);
    };
    let supported = serde_json::json!({
        "pull_mode": "rolling",
        "switch_trigger": {
            "type": "current_target_hp_fraction_at_most",
            "fraction": 0.5
        },
        "max_live_targets": 2
    });
    if policy != &supported {
        return Err(NativeApiError::new(
            "unsupported_world_apl_policy",
            "world_apl must be exact rolling 50%-HP, max_live_targets=2",
        ));
    }
    Ok(Some(policy.clone()))
}

/// Prepare directly from the exact source request. No raw-JSON/protobuf SHA
/// comparison: the native PreparedV2 carries the engine's canonical identity.
fn native_api_prepare(
    request: &serde_json::Value,
) -> Result<(serde_json::Value, PreparedV2), NativeApiError> {
    let source = request.get("request").ok_or("prepare needs request")?;
    let bytes = serde_json::to_vec(source).map_err(|e| e.to_string())?;
    let actor_level = native_api_actor_level(request)?;
    let source_prepared = std::panic::catch_unwind(|| {
        prepare_json_with_actor_level(&bytes, "rust", Rules::Forever, actor_level)
    })
    .map_err(|_| NativeApiError::new("native_prepare_fault", "native prepare panicked"))?
    .map_err(NativeApiError::from)?;
    let value = serde_json::to_value(source_prepared).map_err(|e| e.to_string())?;
    let prepared = serde_json::from_value::<PreparedV2>(value.clone())
        .map_err(|e| format!("native prepared-v2 rejected: {e}"))?;
    Ok((value, prepared))
}

/// Native world/ACK exchange is an untrusted bidirectional pipe. A malformed
/// peer frame triggers fail-closed assertions inside the existing causal
/// bridge; it must refuse this candidate without killing the persistent API
/// process (and the remaining sampled candidate search).
fn native_api_world_operation<T, E: std::fmt::Display>(
    operation: impl FnOnce() -> Result<T, E>,
) -> Result<T, NativeApiError> {
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)).map_err(|panic| {
            let reason = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|text| text.to_string()))
                .unwrap_or_else(|| "native world/ACK protocol panicked".to_owned());
            NativeApiError::new("native_world_protocol_panic", reason)
        })?;
    result.map_err(|error| NativeApiError::new("native_world_refused", error.to_string()))
}

/// Extract only observations made by the original paid native world/ACK
/// execution. A terminal receipt is a measurement, not a transferable player,
/// cooldown, XP, inventory or world object.
fn native_paid_world_snapshot(
    prepared: &PreparedV2,
    report: &serde_json::Value,
    retained: bool,
) -> serde_json::Value {
    let final_kill_clock_ns = report["windows"]
        .as_array()
        .and_then(|windows| windows.last())
        .and_then(|window| window.get("native_kill_ns"))
        .cloned();
    serde_json::json!({
        "kind": if retained { "retained_native_world_segment" } else { "completed_independent_native_world" },
        "source_sha256": prepared.request_sha256,
        "actor_level": prepared.player.level,
        "world_targets_killed": report.get("world_targets_killed"),
        "final_kill_clock_ns": final_kill_clock_ns,
        "terminal_recovery": report.get("terminal_recovery"),
        "post_combat_actor_retained": retained,
        "xp_earned": null,
        "level_transition_supported": false
    })
}

impl NativeApiSession {
    fn dispatch(
        &mut self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, NativeApiError> {
        match request.get("op").and_then(serde_json::Value::as_str) {
            // Native source preparation is a snapshot, not a retained Fight.
            // Expose this capability boundary so a level-range coordinator can
            // distinguish a paid sampled fight from true continuous progression.
            Some("state") => {
                let prepared = self
                    .prepared
                    .as_ref()
                    .ok_or("state needs a preceding prepare")?;
                Ok(serde_json::json!({
                    "kind": "source_prepared_actor",
                    "actor_level": prepared.player.level,
                    "actor_class": prepared.player.class,
                    "source_sha256": prepared.request_sha256,
                    "retained_combat_runtime": self.retained_world.is_some(),
                    "retained_xp": false,
                    "level_transition_supported": false,
                    "last_paid_world": self.last_paid_world.as_ref()
                }))
            }
            // The current API retains PreparedV2 only. A request to cross a
            // level boundary must never silently prepare a new actor and
            // describe it as the old actor gaining XP or training.
            Some("advance") => Err(NativeApiError::new(
                "native_level_transition_unsupported",
                "native API may retain same-level combat but has no earned XP, training or cross-level actor transition",
            )),
            Some("options") => {
                let class = request
                    .get("class")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("options needs class")?;
                let level = request
                    .get("level")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|n| u8::try_from(n).ok())
                    .filter(|n| (1..=60).contains(n))
                    .ok_or("options needs level 1..60")?;
                serde_json::to_value(get_options(level, class.parse::<OptionsClass>()?)?)
                    .map_err(|e| NativeApiError::from(e.to_string()))
            }
            Some("prepare") => {
                // No measured world from the previous candidate can be attached
                // to a newly prepared actor, even when its preparation refuses.
                self.last_paid_world = None;
                self.retained_world = None;
                // On refusal no earlier actor may accidentally be evaluated.
                self.prepared = None;
                let (value, prepared) = native_api_prepare(request)?;
                self.prepared = Some(prepared);
                Ok(value)
            }
            Some("evaluate") => {
                // An opt-in retained native Warlock world spans multiple paid
                // evaluate messages without resetting its Fight, queue or clock.
                // All historical sampled calls still create independent fights.
                let continued = match request.get("continue_from_previous") {
                    None | Some(serde_json::Value::Bool(false)) => false,
                    Some(serde_json::Value::Bool(true)) => true,
                    _ => return Err(NativeApiError::new(
                        "invalid_api_request", "continue_from_previous must be a boolean")),
                };
                let retain = match request.get("retain_native_actor") {
                    None | Some(serde_json::Value::Bool(false)) => false,
                    Some(serde_json::Value::Bool(true)) => true,
                    _ => return Err(NativeApiError::new(
                        "invalid_api_request", "retain_native_actor must be a boolean")),
                };
                // Validate candidate recovery policy before replacing any retained
                // native state or spending a world tick on this request.
                let (between_health, between_mana) =
                    native_api_between_pull_thresholds(request)?;
                let world_apl = native_api_world_apl(request)?;
                if world_apl.is_some() && continued {
                    return Err(NativeApiError::new(
                        "native_rolling_continuation_unsupported",
                        "a rolling two-live Fight must begin with both world identities unbound",
                    ));
                }
                if continued && (!retain || self.retained_world.is_none()) {
                    return Err(NativeApiError::new(
                        "native_combat_continuation_unsupported",
                        "continue_from_previous requires a preceding paid retained native Fight and retain_native_actor true",
                    ));
                }
                if continued && request.get("request").is_some() {
                    return Err(NativeApiError::new(
                        "native_combat_continuation_source_mismatch",
                        "a retained Fight cannot be replaced with a newly prepared actor",
                    ));
                }
                if !continued {
                    self.retained_world = None;
                }
                // A new world attempt supersedes the previous paid segment.
                // A native refusal must not leave a stale measured checkpoint
                // that a route coordinator might mistake for this candidate.
                self.last_paid_world = None;
                // Evaluate either the actor retained by a preceding prepare in
                // this session, or prepare a new source inline in this call.
                if request.get("request").is_some() {
                    self.prepared = None;
                    self.retained_world = None;
                    let (_, prepared) = native_api_prepare(request)?;
                    self.prepared = Some(prepared);
                }
                let prepared = self
                    .prepared
                    .as_ref()
                    .ok_or("evaluate needs a preceding prepare or inline request")?;
                // A persistent session may retain a source-prepared L40 actor
                // while the next route sample requests L50. Never silently
                // charge an L40 world as evidence for a named L50 candidate.
                // For inline requests, this also confirms the actual native
                // prepared actor matches the explicit level after preparation.
                if let Some(requested_level) = native_api_actor_level(request)? {
                    if prepared.player.level != requested_level {
                        return Err(NativeApiError::new(
                            "native_actor_level_mismatch",
                            format!(
                                "evaluate requested actor level {requested_level} but native prepared actor is level {}",
                                prepared.player.level
                            ),
                        ));
                    }
                }
                let targets = request
                    .get("targets")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|n| usize::try_from(n).ok())
                    .filter(|n| (2..=100).contains(n))
                    .ok_or("evaluate needs targets 2..100")?;
                if world_apl.is_some() && targets != 2 {
                    return Err(NativeApiError::new(
                        "native_rolling_requires_two_targets",
                        "rolling World-APL currently supports exactly two native world targets",
                    ));
                }
                let rng = request
                    .get("rng_mode")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("evaluate needs rng_mode")?;
                let mode = RngMode::parse_cli(rng)?;
                if request
                    .get("recover_at_end")
                    .and_then(serde_json::Value::as_bool)
                    != Some(true)
                {
                    return Err("evaluate requires recover_at_end true".into());
                }
                // Compile the requested World-APL into the genuine source-native
                // Warlock rotation before Fight creation. Do not overwrite the
                // original source-prepared actor shared by later candidates.
                let rolling_prepared = world_apl.as_ref().map(|_| {
                    native_rolling_world_apl_prepared(prepared).map_err(|reason| {
                        NativeApiError::new("native_world_apl_refused", reason)
                    })
                }).transpose()?;
                let paid_prepared = rolling_prepared.as_ref().unwrap_or(prepared);
                // This is the existing real two-dimensional gsim tick/ACK
                // boundary and native Decisions terminal dual-resource rest.
                // With FOREVER_GSIM_WORLD_BRIDGE=1 it writes world events to
                // stdout and consumes causal ACKs on stdin during this call.
                // Stage a successful retained Fight until the World-APL
                // receipt is verified. A rejected rolling policy must not
                // leave an unACKed/partially paid Fight available to continue.
                let mut verified_retained_world = None;
                let report = if retain {
                    if prepared.player.class != "ClassWarlock" {
                        return Err(NativeApiError::new(
                            "native_retained_class_unsupported",
                            "retained native Fight currently supports Warlock only",
                        ));
                    }
                    let mut live = if continued {
                        self.retained_world.take().ok_or_else(|| NativeApiError::new(
                            "native_combat_continuation_unsupported", "no retained Fight",
                        ))?
                    } else {
                        native_api_world_operation(|| NativeWarlockWorldSession::new(paid_prepared, mode))?
                    };
                    if live.rng_mode() != mode {
                        self.retained_world = Some(live);
                        return Err(NativeApiError::new(
                            "native_continuation_rng_mismatch",
                            "cannot reseed or change RNG mode of an already retained native Fight",
                        ));
                    }
                    // A failed native segment drops the potentially partial
                    // Fight rather than allowing an unACKed continuation.
                    let result = native_api_world_operation(|| {
                        live.evaluate_segment_with_recovery(
                            targets, continued, between_health, between_mana,
                        )
                    });
                    if result.is_ok() {
                        verified_retained_world = Some(live);
                    }
                    result?
                } else {
                    native_api_world_operation(|| {
                        native_two_world_targets_with_decisions_recovery(
                            paid_prepared, targets, mode, between_health, between_mana,
                        )
                    })?
                };
                let mut report = report;
                if let Some(policy) = world_apl {
                    // An API policy request is not an application receipt.
                    // Only native source-ACKed A→B switching and both distinct
                    // lethal world ACKs in a single Fight can produce the echo
                    // that Forever's paid rolling evaluator requires.
                    if report["world_overlap"]["both_world_targets_lethal_ack"] != true
                        || report["world_overlap"]["source_apl_selected_b"] != true
                    {
                        return Err(NativeApiError::new(
                            "native_rolling_world_apl_not_applied",
                            "requested rolling World-APL did not produce both live-target lethal ACKs",
                        ));
                    }
                    report["applied_world_apl"] = policy;
                }
                // A retained Fight is usable only after every requested
                // source-applied policy has been checked against native ACKs.
                self.retained_world = verified_retained_world;
                self.last_paid_world = Some(native_paid_world_snapshot(prepared, &report, retain));
                Ok(serde_json::json!({
                    "engine": "forever-engine",
                    "source_sha256": prepared.request_sha256,
                    "result": report,
                    "continuity": {
                        "kind": if retain { "retained_native_world_segment" } else { "independent_source_prepared_fight" },
                        "post_combat_actor_retained": retain,
                        "xp_earned": null,
                        "level_transition_supported": false
                    },
                    "checkpoint_certified": false
                }))
            }
            _ => Err("unsupported API op (expected options, prepare, state, evaluate or advance)".into()),
        }
    }
}

/// Persistent NDJSON Rust API; gsim's world ACKs use the same stdin/stdout
/// stream during evaluate and are consumed by the existing native bridge.
fn run_native_api() -> Result<(), Failure> {
    let mut session = NativeApiSession::default();
    loop {
        let mut line = String::new();
        if std::io::stdin()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?
            == 0
        {
            return Ok(());
        }
        let response = match serde_json::from_str::<serde_json::Value>(&line)
            .map_err(|e| {
                NativeApiError::new("invalid_api_request", format!("invalid API request: {e}"))
            })
            .and_then(|request| session.dispatch(&request))
        {
            Ok(result) => serde_json::json!({"ok": true, "result": result}),
            Err(error) => serde_json::json!({
                "ok": false,
                "error": error.reason.clone(),
                "refusal": {"code": error.code, "reason": error.reason}
            }),
        };
        println!("{}", response);
        std::io::stdout().flush().map_err(|e| e.to_string())?;
    }
}

#[cfg(test)]
mod native_optimizer_api_tests {
    use super::{NativeApiError, NativeApiSession, PrepareError};

    #[test]
    fn world_apl_api_accepts_only_exact_source_executable_rolling_policy() {
        use super::native_api_world_apl;
        let policy = serde_json::json!({
            "pull_mode": "rolling",
            "switch_trigger": {
                "type": "current_target_hp_fraction_at_most", "fraction": 0.5
            },
            "max_live_targets": 2
        });
        assert_eq!(native_api_world_apl(&serde_json::json!({})).unwrap(), None);
        assert_eq!(
            native_api_world_apl(&serde_json::json!({
                "world_apl": policy.clone()
            }))
            .unwrap(),
            Some(policy.clone())
        );
        for invalid in [
            serde_json::Value::Null,
            serde_json::json!({"pull_mode": "serial"}),
            serde_json::json!({"pull_mode": "rolling",
                "switch_trigger": {"type": "current_target_hp_fraction_at_most",
                    "fraction": 0.6}, "max_live_targets": 2}),
            serde_json::json!({"pull_mode": "rolling",
                "switch_trigger": {"type": "current_target_hp_fraction_at_most",
                    "fraction": 0.5}, "max_live_targets": 3}),
        ] {
            let refusal = native_api_world_apl(&serde_json::json!({
                "world_apl": invalid
            }))
            .unwrap_err();
            assert_eq!(refusal.code, "unsupported_world_apl_policy");
        }
        let mut session = NativeApiSession::default();
        let refusal = session
            .dispatch(&serde_json::json!({
                "op": "evaluate", "world_apl": {"pull_mode": "rolling"},
                "targets": 2, "rng_mode": "balanced", "recover_at_end": true
            }))
            .unwrap_err();
        assert_eq!(refusal.code, "unsupported_world_apl_policy");
    }

    #[test]
    fn native_prepare_refusal_keeps_source_code_and_reason() {
        let error = NativeApiError::from(PrepareError::Refused(forever_engine::PrepareRefusal {
            code: "source_item_refused",
            reason: "unsupported candidate item".to_owned(),
        }));
        assert_eq!(error.code, "source_item_refused");
        assert_eq!(error.reason, "unsupported candidate item");
        let error = NativeApiError::from(PrepareError::Invalid("missing iterations".to_owned()));
        assert_eq!(error.code, "invalid_prepare_request");
        let error = NativeApiError::from(PrepareError::Fault("internal failure".to_owned()));
        assert_eq!(error.code, "native_prepare_fault");
    }

    #[test]
    fn sampled_actor_level_rejects_malformed_values_without_l60_fallback() {
        use super::native_api_actor_level;

        for invalid in [
            serde_json::json!(null),
            serde_json::json!("40"),
            serde_json::json!(40.5),
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::json!(61),
            serde_json::json!(4_294_967_296_i64),
        ] {
            let request = serde_json::json!({"actor_level": invalid});
            assert_eq!(
                native_api_actor_level(&request).unwrap_err(),
                "actor_level must be an integer in 1..=60"
            );
        }
        assert_eq!(
            native_api_actor_level(&serde_json::json!({"actor_level": 40})).unwrap(),
            Some(40)
        );
        assert_eq!(
            native_api_actor_level(&serde_json::json!({})).unwrap(),
            None
        );
    }

    #[test]
    fn malformed_world_ack_refuses_one_candidate_without_ending_the_session() {
        use super::native_api_world_operation;

        let refused = native_api_world_operation(|| -> Result<(), &str> {
            panic!("RUST_WORLD_TICK_IDENTITY_OR_ACK_CHAIN_INVALID");
        })
        .unwrap_err();
        assert_eq!(refused.code, "native_world_protocol_panic");
        assert!(refused
            .reason
            .contains("RUST_WORLD_TICK_IDENTITY_OR_ACK_CHAIN_INVALID"));

        let refused = native_api_world_operation(|| -> Result<(), &str> {
            Err("RUST_MULTI_WORLD_TARGET_NOT_NATIVE_LETHAL")
        })
        .unwrap_err();
        assert_eq!(refused.code, "native_world_refused");
        assert_eq!(refused.reason, "RUST_MULTI_WORLD_TARGET_NOT_NATIVE_LETHAL");

        assert_eq!(native_api_world_operation(|| Ok::<_, &str>(7)).unwrap(), 7);
        let mut session = NativeApiSession::default();
        assert_eq!(
            session
                .dispatch(&serde_json::json!({"op":"options","class":"warlock","level":40}))
                .unwrap()["level"],
            40
        );
    }

    #[test]
    fn reused_prepared_actor_cannot_be_scored_as_another_level() {
        // A real v2 prepared source actor, without any gsim process or paid
        // world simulation: the refusal must precede the world bridge.
        let prepared: super::PreparedV2 = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fixtures/mage/prepared-v2/balance-druid-revelation.prepared.json"
        )))
        .unwrap();
        assert_eq!(prepared.player.level, 60);
        let mut session = NativeApiSession {
            prepared: Some(prepared),
            last_paid_world: None,
            retained_world: None,
        };
        let refusal = session
            .dispatch(&serde_json::json!({
                "op": "evaluate",
                "actor_level": 40,
                "targets": 2,
                "rng_mode": "balanced",
                "recover_at_end": true
            }))
            .unwrap_err();
        assert_eq!(refusal.code, "native_actor_level_mismatch");
        assert!(refusal.reason.contains("requested actor level 40"));
        assert!(refusal.reason.contains("prepared actor is level 60"));
        // A refused comparison must not mutate the source-prepared actor.
        assert_eq!(session.prepared.as_ref().unwrap().player.level, 60);
    }

    #[test]
    fn native_session_exposes_prepared_state_but_refuses_unearned_level_continuity() {
        let prepared: super::PreparedV2 = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fixtures/mage/prepared-v2/balance-druid-revelation.prepared.json"
        )))
        .unwrap();
        let sha = prepared.request_sha256.clone();
        let mut session = NativeApiSession {
            prepared: Some(prepared),
            last_paid_world: None,
            retained_world: None,
        };
        let state = session
            .dispatch(&serde_json::json!({"op": "state"}))
            .unwrap();
        assert_eq!(state["kind"], "source_prepared_actor");
        assert_eq!(state["actor_level"], 60);
        assert_eq!(state["source_sha256"], sha);
        assert_eq!(state["retained_combat_runtime"], false);
        assert_eq!(state["retained_xp"], false);
        assert_eq!(state["level_transition_supported"], false);
        assert!(state["last_paid_world"].is_null());

        // The native engine has no XP/level-up source transition: never
        // manufacture a successful 60->61 advancement or run a new fight.
        let refusal = session
            .dispatch(&serde_json::json!({
                "op": "advance", "from_level": 60, "to_level": 61
            }))
            .unwrap_err();
        assert_eq!(refusal.code, "native_level_transition_unsupported");
        assert!(refusal.reason.contains("earned XP"));

        let continuation = session
            .dispatch(&serde_json::json!({
                "op": "evaluate", "continue_from_previous": true,
                "actor_level": 60, "targets": 2,
                "rng_mode": "balanced", "recover_at_end": true
            }))
            .unwrap_err();
        assert_eq!(continuation.code, "native_combat_continuation_unsupported");
        assert!(continuation
            .reason
            .contains("requires a preceding paid retained native Fight"));

        let invalid_flag = session
            .dispatch(&serde_json::json!({
                "op": "evaluate", "continue_from_previous": "true"
            }))
            .unwrap_err();
        assert_eq!(invalid_flag.code, "invalid_api_request");
        assert_eq!(session.prepared.as_ref().unwrap().player.level, 60);
        assert_eq!(session.prepared.as_ref().unwrap().request_sha256, sha);
    }

    #[test]
    fn retained_world_rejects_non_warlock_before_any_native_payment() {
        let prepared: super::PreparedV2 = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fixtures/mage/prepared-v2/balance-druid-revelation.prepared.json"
        )))
        .unwrap();
        let mut session = NativeApiSession {
            prepared: Some(prepared),
            last_paid_world: None,
            retained_world: None,
        };
        let error = session
            .dispatch(&serde_json::json!({
                "op": "evaluate", "retain_native_actor": true,
                "actor_level": 60, "targets": 2,
                "rng_mode": "balanced", "recover_at_end": true,
            }))
            .unwrap_err();
        assert_eq!(error.code, "native_retained_class_unsupported");
        assert_eq!(
            session
                .dispatch(&serde_json::json!({"op": "state"}))
                .unwrap()["retained_combat_runtime"],
            false
        );
        let invalid = session
            .dispatch(&serde_json::json!({
                "op": "evaluate", "retain_native_actor": "true",
            }))
            .unwrap_err();
        assert_eq!(invalid.code, "invalid_api_request");
    }

    #[test]
    fn paid_world_snapshot_keeps_actual_terminal_receipt_without_claiming_continuity() {
        let prepared: super::PreparedV2 = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fixtures/mage/prepared-v2/balance-druid-revelation.prepared.json"
        )))
        .unwrap();
        let report = serde_json::json!({
            "world_targets_killed": 2,
            "windows": [{"native_kill_ns": 11}, {"native_kill_ns": 43}],
            "terminal_recovery": {
                "paid": true, "native_clock_accounted": true,
                "end_clock_ns": 99,
                "health_after": 1350.0, "mana_after": 2000.0,
                "max_health": 1350.0, "max_mana": 2000.0
            }
        });
        let snapshot = super::native_paid_world_snapshot(&prepared, &report, false);
        let mut session = NativeApiSession {
            prepared: Some(prepared),
            last_paid_world: Some(snapshot),
            retained_world: None,
        };
        let state = session
            .dispatch(&serde_json::json!({"op": "state"}))
            .unwrap();
        let paid = &state["last_paid_world"];
        assert_eq!(paid["kind"], "completed_independent_native_world");
        assert_eq!(paid["source_sha256"], state["source_sha256"]);
        assert_eq!(paid["actor_level"], state["actor_level"]);
        assert_eq!(paid["world_targets_killed"], 2);
        assert_eq!(paid["final_kill_clock_ns"], 43);
        assert_eq!(paid["terminal_recovery"], report["terminal_recovery"]);
        assert_eq!(paid["post_combat_actor_retained"], false);
        assert!(paid["xp_earned"].is_null());
        assert_eq!(paid["level_transition_supported"], false);

        // Unsupported continuation must not promote the saved observation to
        // a resumed Fight or mutate the source-prepared actor.
        assert_eq!(
            session
                .dispatch(&serde_json::json!({
                    "op": "advance", "from_level": 60, "to_level": 61
                }))
                .unwrap_err()
                .code,
            "native_level_transition_unsupported"
        );
        assert_eq!(
            session
                .dispatch(&serde_json::json!({"op": "state"}))
                .unwrap()["last_paid_world"],
            *paid
        );
    }

    #[test]
    fn source_prepared_l20_l40_warlock_effects_bind_to_registered_ranks() {
        // Real Rust preparation and native Fight construction, no gsim peer or
        // paid world claim. This regresses the L20 live refusal caused by
        // exporting Immolate 11668 from L60 while registering the L20 rank.
        let mut source: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/classes/warlock/prepare/affliction-no-summon.request.json"
        )))
        .unwrap();
        let actor = &mut source["raid"]["parties"][0]["players"][0];
        actor["race"] = serde_json::json!("RaceTroll");
        actor["talentsString"] = serde_json::json!("0-0-0");
        actor["rotation"] = serde_json::json!({
            "type": "TypeAPL", "priorityList": []
        });
        actor["warlock"]["options"]["classOptions"] = serde_json::json!({"summon": "NoSummon"});
        source["simOptions"]["iterations"] = serde_json::json!(1);
        let source_bytes = serde_json::to_vec(&source).unwrap();
        for level in [20, 40] {
            let value = forever_engine::prepare_json_with_actor_level(
                &source_bytes,
                "rust",
                forever_engine::Rules::Forever,
                Some(level),
            )
            .unwrap_or_else(|err| panic!("Warlock L{level} source prepare refused: {err}"));
            let registered: std::collections::HashSet<i64> = value["player"]["spells"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|spell| spell["action_id"]["spell_id"].as_i64())
                .collect();
            for effect in value["effects"].as_array().unwrap() {
                let kind = effect["kind"].as_str().unwrap_or_default();
                if matches!(
                    kind,
                    "immolate"
                        | "corruption"
                        | "bane_of_doom"
                        | "drain_life"
                        | "hellfire"
                        | "rain_of_fire"
                        | "siphon_life"
                        | "bane_of_agony"
                        | "life_tap"
                ) {
                    let spell_id = effect["spell_id"].as_i64().unwrap();
                    assert!(
                        registered.contains(&spell_id),
                        "Warlock L{level} effect {kind} uses unregistered spell {spell_id}"
                    );
                }
            }
            let prepared: forever_engine::contracts::prepared_v2::PreparedV2 =
                serde_json::from_value(value).unwrap();
            forever_engine::NativeWarlockWorldSession::new(
                &prepared,
                forever_engine::RngMode::Balanced,
            )
            .unwrap_or_else(|err| panic!("Warlock L{level} native world binding refused: {err}"));
        }
    }

    #[test]
    fn between_pull_rest_policy_is_native_configurable_without_free_terminal_reset() {
        use super::native_api_between_pull_thresholds;
        assert_eq!(
            native_api_between_pull_thresholds(&serde_json::json!({})).unwrap(),
            (1.0, 1.0)
        );
        assert_eq!(
            native_api_between_pull_thresholds(&serde_json::json!({
                "between_pulls": {"health_fraction": 0.3, "mana_fraction": 0.3}
            }))
            .unwrap(),
            (0.3, 0.3)
        );
        assert_eq!(
            native_api_between_pull_thresholds(&serde_json::json!({
                "between_pulls": {"health_fraction": 0.0, "mana_fraction": 1.0}
            }))
            .unwrap(),
            (0.0, 1.0)
        );
        for invalid in [
            serde_json::json!({"between_pulls": null}),
            serde_json::json!({"between_pulls": [0.3, 0.3]}),
            serde_json::json!({"between_pulls": {"health_fraction": 0.3}}),
            serde_json::json!({"between_pulls": {
                "health_fraction": "0.3", "mana_fraction": 0.3
            }}),
            serde_json::json!({"between_pulls": {
                "health_fraction": -0.1, "mana_fraction": 0.3
            }}),
            serde_json::json!({"between_pulls": {
                "health_fraction": 0.3, "mana_fraction": 1.1
            }}),
            serde_json::json!({"between_pulls": {
                "health_fraction": 0.3, "mana_fraction": 0.3, "unsupported": true
            }}),
        ] {
            assert_eq!(
                native_api_between_pull_thresholds(&invalid)
                    .unwrap_err()
                    .code,
                "invalid_between_pull_policy"
            );
            // A bad candidate never enters Rust's paid gsim world execution.
            let mut session = NativeApiSession::default();
            let refusal = session
                .dispatch(&serde_json::json!({
                    "op": "evaluate", "targets": 2, "rng_mode": "balanced",
                    "recover_at_end": true, "between_pulls": invalid["between_pulls"]
                }))
                .unwrap_err();
            assert_eq!(refusal.code, "invalid_between_pull_policy");
        }
    }

    #[test]
    fn options_from_native_and_eval_without_prepared_actor_refuses() {
        let mut session = NativeApiSession::default();
        let options = session
            .dispatch(&serde_json::json!({
                "op": "options", "class": "warlock", "level": 40
            }))
            .unwrap();
        assert_eq!(options["level"], 40);
        assert_eq!(options["class"], "warlock");
        assert!(session
            .dispatch(&serde_json::json!({
                "op": "evaluate", "targets": 2,
                "rng_mode": "balanced", "recover_at_end": true
            }))
            .unwrap_err()
            .reason
            .contains("preceding prepare"));
    }
}

fn run() -> Result<(), Failure> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args == ["api"] {
        return run_native_api();
    }
    if args == ["version"] {
        println!(
            "forever-rust-prototype-{} source-{SOURCE_REVISION}",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    if args.is_empty() || args == ["--help"] {
        println!("forever-engine sim --infile REQUEST.json|PREPARED_V2.json [--outfile RESULT.json] [--trace] [--gate] [--rotation-stats]\nforever-engine sim --request RAID_SIM_REQUEST.json [--scenario ID] [--outfile RESULT.json] [--gate] [--rotation-stats] [--go-rules]\nforever-engine prepare --request RAID_SIM_REQUEST.json [--actor-level 20] [--scenario ID] [--outfile PREPARED_V2.json] [--go-rules]\nforever-engine bench --infile REQUEST.json|PREPARED_V2.json [--outfile RESULT.json] [--warmups 3] [--samples 7]\nforever-engine check --infile PREPARED_V2.json\nforever-engine gear-options --item-types ItemTypeHead,ItemTypeHands,...\nforever-engine item-options --class priest|mage|shaman|paladin --level N --spec unrestricted --slot SLOT\nforever-engine player-context --class paladin --level 20 [--race human|dwarf]\nforever-engine options --class CLASS --level LEVEL\nforever-engine talent-options --class CLASS --level LEVEL\nforever-engine version");
        return Ok(());
    }

    if args[0] == "options" || args[0] == "talent-options" {
        let command = args[0].as_str();
        if args.len() != 5 || args[1] != "--class" || args[3] != "--level" {
            return Err(format!("{command} requires --class CLASS --level LEVEL").into());
        }
        if command == "talent-options" {
            let level = args[4]
                .parse::<i32>()
                .map_err(|_| "talent-options level must be an integer 1..=60")?;
            println!(
                "{}",
                serde_json::to_string_pretty(&native_talent_options(&args[2], level))
                    .map_err(|err| err.to_string())?
            );
        } else {
            let class: OptionsClass = args[2].parse()?;
            let level = args[4]
                .parse::<u8>()
                .map_err(|_| "options level must be an integer 1..=60")?;
            let options = get_options(level, class)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&options).map_err(|err| err.to_string())?
            );
        }
        return Ok(());
    }

    if args[0] == "gear-options" {
        if args.len() != 3 || args[1] != "--item-types" {
            return Err("gear-options requires --item-types ItemTypeHead,ItemTypeHands,...".into());
        }
        let item_types: Vec<String> = args[2]
            .split(',')
            .map(str::trim)
            .map(str::to_string)
            .collect();
        if item_types.is_empty() || item_types.iter().any(String::is_empty) {
            return Err("gear-options requires at least one nonempty item type".into());
        }
        let mut unique = std::collections::BTreeSet::new();
        if item_types.iter().any(|item_type| !unique.insert(item_type)) {
            return Err("gear-options item types must be unique".into());
        }
        let output = serde_json::to_string_pretty(&gear_options(&item_types))
            .map_err(|err| err.to_string())?;
        println!("{output}");
        return Ok(());
    }
    if args[0] == "player-context" {
        let mut class: Option<String> = None;
        let mut race: Option<String> = None;
        let mut level: Option<i32> = None;
        let mut index = 1;
        while index < args.len() {
            let flag = args[index].as_str();
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| format!("{flag} requires a value"))?
                .clone();
            index += 1;
            match flag {
                "--class" => {
                    if class.replace(value).is_some() {
                        return Err("duplicate --class".into());
                    }
                }
                "--race" => {
                    if race.replace(value).is_some() {
                        return Err("duplicate --race".into());
                    }
                }
                "--level" => {
                    let parsed = value.parse::<i32>().map_err(|_| "invalid --level")?;
                    if level.replace(parsed).is_some() {
                        return Err("duplicate --level".into());
                    }
                }
                _ => return Err(format!("unsupported player-context argument {flag}").into()),
            }
        }
        let class = class.ok_or("player-context requires --class")?;
        let level = level.ok_or("player-context requires --level")?;
        let response = native_player_context(&class, race.as_deref().unwrap_or(""), level);
        let output = serde_json::to_string_pretty(&response).map_err(|err| err.to_string())?;
        println!("{output}");
        return Ok(());
    }
    if args[0] == "item-options" {
        let mut class: Option<String> = None;
        let mut level: Option<i32> = None;
        let mut spec: Option<String> = None;
        let mut slot: Option<String> = None;
        let mut index = 1;
        while index < args.len() {
            let flag = args[index].as_str();
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| format!("{flag} requires a value"))?
                .clone();
            index += 1;
            match flag {
                "--class" => {
                    if class.replace(value).is_some() {
                        return Err("duplicate --class".into());
                    }
                }
                "--level" => {
                    let parsed = value.parse::<i32>().map_err(|_| "invalid --level")?;
                    if level.replace(parsed).is_some() {
                        return Err("duplicate --level".into());
                    }
                }
                "--spec" => {
                    if spec.replace(value).is_some() {
                        return Err("duplicate --spec".into());
                    }
                }
                "--slot" => {
                    if slot.replace(value).is_some() {
                        return Err("duplicate --slot".into());
                    }
                }
                _ => return Err(format!("unsupported item-options argument {flag}").into()),
            }
        }
        let class = class.ok_or("item-options requires --class")?;
        let level = level.ok_or("item-options requires --level")?;
        let spec = spec.ok_or("item-options requires --spec")?;
        let slot = slot.ok_or("item-options requires --slot")?;
        let response = native_item_options(&class, level, &spec, &slot);
        let output = serde_json::to_string_pretty(&response).map_err(|err| err.to_string())?;
        println!("{output}");
        return Ok(());
    }

    if args[0] == "native-world-pulls" {
        if !(args.len() == 7 || args.len() == 9 || args.len() == 13)
            || args[1] != "--infile"
            || args[3] != "--targets"
            || args[5] != "--rng-mode"
        {
            return Err("native-world-pulls requires --infile PREPARED_V2.json --targets 2..100 --rng-mode MODE [--recovery-seconds 1..120 | --recovery-policy decisions [--health-threshold 0..1 --mana-threshold 0..1]]".into());
        }
        let decisions = if args.len() == 9 || args.len() == 13 {
            match args[7].as_str() {
                "--recovery-seconds" if args.len() == 9 => false,
                "--recovery-policy" if args[8] == "decisions" => true,
                _ => return Err("invalid native-world-pulls recovery policy".into()),
            }
        } else {
            false
        };
        let recovery_seconds = if !decisions {
            if args.len() == 9 {
                args[8]
                    .parse::<u32>()
                    .map_err(|_| "invalid recovery seconds")?
            } else {
                12
            }
        } else {
            0
        };
        if !decisions && !(1..=120).contains(&recovery_seconds) {
            return Err("native-world-pulls requires --recovery-seconds 1..120".into());
        }
        let (health_threshold, mana_threshold) = if decisions && args.len() == 13 {
            if args[9] != "--health-threshold" || args[11] != "--mana-threshold" {
                return Err("expected --health-threshold F --mana-threshold F".into());
            }
            let hp = args[10]
                .parse::<f64>()
                .map_err(|_| "invalid health threshold")?;
            let mana = args[12]
                .parse::<f64>()
                .map_err(|_| "invalid mana threshold")?;
            (hp, mana)
        } else {
            (0.5, 0.1)
        };
        if decisions
            && !(health_threshold.is_finite()
                && mana_threshold.is_finite()
                && (0.0..=1.0).contains(&health_threshold)
                && (0.0..=1.0).contains(&mana_threshold))
        {
            return Err("native-world-pulls Decisions threshold must be 0..1".into());
        }
        if args.len() == 13 && !decisions {
            return Err("extra arguments require --recovery-policy decisions".into());
        }
        let target_count = args[4]
            .parse::<usize>()
            .map_err(|_| "native-world-pulls target count must be an integer")?;
        if !(2..=100).contains(&target_count) {
            return Err("native-world-pulls requires --targets 2..100".into());
        }
        // '-' accepts one prepared-v2 JSON line on stdin, followed by the
        // existing bidirectional live 2D world tick/ACK protocol.
        let input = if args[2] == "-" {
            let mut line = String::new();
            std::io::stdin()
                .read_line(&mut line)
                .map_err(|e| e.to_string())?;
            if line.is_empty() {
                return Err("native-world-pulls stdin needs prepared-v2 JSON line".into());
            }
            line.into_bytes()
        } else {
            fs::read(&args[2]).map_err(|err| err.to_string())?
        };
        let prepared =
            prepared_v2(&input)?.ok_or("native-world-pulls requires a prepared v2 input")?;
        let mode = RngMode::parse_cli(&args[6])?;
        let report = if decisions {
            native_two_world_targets_with_decisions_recovery(
                &prepared,
                target_count,
                mode,
                health_threshold,
                mana_threshold,
            )
        } else {
            native_two_world_targets_with_recovery(&prepared, target_count, mode, recovery_seconds)
        }
        .map_err(|err| err.to_string())?;
        println!(
            "{}",
            serde_json::json!({
                "engine": "forever-engine",
                "source_sha256": prepared.request_sha256,
                "result": report, "checkpoint_certified": false,
            })
        );
        return Ok(());
    }
    if args[0] == "native-pulls" {
        if args.len() != 7
            || args[1] != "--infile"
            || args[3] != "--targets"
            || args[4] != "2"
            || args[5] != "--rng-mode"
        {
            return Err("native-pulls requires --infile PREPARED_V2.json --targets 2".into());
        }
        let input = fs::read(&args[2]).map_err(|err| err.to_string())?;
        let prepared = prepared_v2(&input)?.ok_or("native-pulls requires a prepared v2 input")?;
        let mode = RngMode::parse_cli(&args[6])?;
        let windows = native_two_pull_windows(&prepared, mode).map_err(|err| err.to_string())?;
        println!(
            "{}",
            serde_json::json!({
                "engine": "forever-engine",
                "source_sha256": prepared.request_sha256,
                "scenario_id": prepared.scenario_id,
                "result": windows,
                "checkpoint_certified": false,
            })
        );
        return Ok(());
    }
    if args.len() == 3 && args[0] == "check" && args[1] == "--infile" {
        let input = fs::read(&args[2]).map_err(|err| err.to_string())?;
        let prepared = prepared_v2(&input)?.ok_or("check requires a prepared v2 input")?;
        // An invalid input is an error, never a refusal.
        validate_prepared(&prepared).map_err(|err| PreparedError::Invalid(err).to_string())?;
        let refusals = prepared_refusals(&prepared);
        println!("{}", coverage_report(&prepared, &refusals)?);
        return Ok(());
    }
    if args[0] != "sim" && args[0] != "bench" && args[0] != "prepare" {
        return Err("expected sim, prepare, bench, check or version".into());
    }
    let mut rng_mode: Option<RngMode> = None;
    let mut infile = None;
    let mut request_file = None;
    let mut scenario = None;
    let mut outfile = None;
    let mut trace = false;
    let mut gate = false;
    // Add the rotation the application shows and Go's validation messages of its lines, as
    // MythicSim's Go CLI adapter does.
    let mut rotation_stats = false;
    // Prepare with the Go reference's rules where Rust departs from it, as the exporter does.
    let mut go_rules = false;
    let mut actor_level: Option<i32> = None;
    let mut warmups = 3u32;
    let mut samples = 7u32;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--infile" | "--outfile" | "--request" | "--scenario" => {
                let flag = &args[index];
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| format!("{flag} requires a value"))?;
                let target = match flag.as_str() {
                    "--infile" => &mut infile,
                    "--request" => &mut request_file,
                    "--scenario" => &mut scenario,
                    _ => &mut outfile,
                };
                if target.replace(value.clone()).is_some() {
                    return Err(format!("duplicate {flag}").into());
                }
            }
            "--actor-level" => {
                index += 1;
                let value = args.get(index).ok_or("--actor-level requires a value")?;
                let level: i32 = value
                    .parse()
                    .map_err(|_| "--actor-level must be an integer")?;
                if actor_level.replace(level).is_some() {
                    return Err("duplicate --actor-level".into());
                }
            }
            "--rng-mode" => {
                index += 1;
                let value = args.get(index).ok_or("--rng-mode requires a value")?;
                if rng_mode.replace(RngMode::parse_cli(value)?).is_some() {
                    return Err("duplicate --rng-mode".into());
                }
            }
            "--trace" if !trace => trace = true,
            "--gate" if args[0] == "sim" && !gate => gate = true,
            "--rotation-stats" if args[0] == "sim" && !rotation_stats => rotation_stats = true,
            "--go-rules" if args[0] != "bench" && !go_rules => go_rules = true,
            "--warmups" | "--samples" if args[0] == "bench" => {
                let flag = &args[index];
                index += 1;
                let value: u32 = args
                    .get(index)
                    .ok_or_else(|| format!("{flag} requires a count"))?
                    .parse()
                    .map_err(|_| format!("invalid {flag}"))?;
                if flag == "--warmups" {
                    warmups = value;
                } else {
                    samples = value;
                }
            }
            unknown => return Err(format!("unsupported argument {unknown}").into()),
        }
        index += 1;
    }
    let rules = if go_rules {
        Rules::GoReference
    } else {
        Rules::Forever
    };
    if go_rules && request_file.is_none() {
        return Err("--go-rules requires --request".into());
    }
    if actor_level.is_some() && request_file.is_none() {
        return Err("--actor-level requires --request".into());
    }
    if args[0] == "prepare" {
        let request = request_file.ok_or("prepare requires --request")?;
        if infile.is_some() || trace || gate {
            return Err(
                "prepare takes --request, --actor-level, --scenario, --outfile and --go-rules"
                    .into(),
            );
        }
        let input = fs::read(request).map_err(|err| err.to_string())?;
        let prepared = prepare_request(
            &input,
            scenario.as_deref().unwrap_or("rust"),
            rules,
            actor_level,
        )?;
        let output = serde_json::to_string_pretty(&prepared).map_err(|err| err.to_string())?;
        if let Some(path) = outfile {
            write_atomically(&path, &(output + "\n"))?;
        } else {
            println!("{output}");
        }
        return Ok(());
    }
    if scenario.is_some() && request_file.is_none() {
        return Err("--scenario requires --request".into());
    }
    let rng_mode =
        rng_mode.ok_or("sim and bench require explicit --rng-mode balanced|stochastic")?;
    let prepared_from_request = match request_file {
        Some(request) => {
            if infile.is_some() || args[0] != "sim" {
                return Err("--request replaces --infile and is for sim and prepare".into());
            }
            let input = fs::read(request).map_err(|err| err.to_string())?;
            let prepared = prepare_request(
                &input,
                scenario.as_deref().unwrap_or("rust"),
                rules,
                actor_level,
            )?;
            Some(serde_json::to_vec(&prepared).map_err(|err| err.to_string())?)
        }
        None => None,
    };
    let input = match prepared_from_request {
        Some(prepared) => prepared,
        None => fs::read(infile.ok_or("--infile is required")?).map_err(|err| err.to_string())?,
    };
    if let Some(prepared) = prepared_v2(&input)? {
        if trace {
            return Err(
                "prepared v2 inputs do not support --trace; set debugFirstIteration for logs"
                    .into(),
            );
        }
        let mut report = if gate {
            validate_prepared(&prepared).map_err(|err| Failure {
                status: EXIT_REJECTED,
                message: Some(PreparedError::Invalid(err).to_string()),
            })?;
            match simulate_prepared_gated(&prepared, rng_mode).map_err(|err| err.to_string())? {
                Gated::Simulated(report) => *report,
                Gated::Refused(refusals) => {
                    println!("{}", coverage_report(&prepared, &refusals)?);
                    return Err(Failure {
                        status: EXIT_REFUSED,
                        message: None,
                    });
                }
            }
        } else {
            simulate_prepared(&prepared, rng_mode).map_err(|err| err.to_string())?
        };
        if rotation_stats {
            report.add_rotation_stats();
        }
        let output = if args[0] == "bench" {
            if warmups > 20 || !(1..=100).contains(&samples) {
                return Err("bench requires 0 to 20 warmups and 1 to 100 samples".into());
            }
            for _ in 1..warmups {
                black_box(
                    simulate_prepared(black_box(&prepared), rng_mode)
                        .map_err(|err| err.to_string())?,
                );
            }
            let mut timings = Vec::with_capacity(samples as usize);
            for _ in 0..samples {
                let sample = black_box(simulate_prepared(black_box(&prepared), rng_mode))
                    .map_err(|err| err.to_string())?;
                if sample.result != report.result {
                    return Err("bench samples produced different results".into());
                }
                timings.push(sample.elapsed_ns);
            }
            serde_json::to_string_pretty(&serde_json::json!({
                "warmups": warmups.max(1), "samples": samples,
                "elapsed_ns_samples": timings, "report": report,
            }))
        } else {
            serde_json::to_string_pretty(&report)
        }
        .map_err(|err| err.to_string())?;
        if let Some(path) = outfile {
            write_atomically(&path, &(output + "\n"))?;
        } else {
            println!("{output}");
        }
        return Ok(());
    }
    if gate || rotation_stats {
        return Err("--gate and --rotation-stats require a prepared v2 input".into());
    }
    let request: Request =
        serde_json::from_slice(&input).map_err(|err| format!("request rejected: {err}"))?;
    let output = if args[0] == "bench" {
        if trace || warmups > 20 || !(1..=100).contains(&samples) {
            return Err("bench requires no trace, 0 to 20 warmups and 1 to 100 samples".into());
        }
        for _ in 0..warmups {
            black_box(simulate(black_box(&request), false)?);
        }
        let mut timings = Vec::with_capacity(samples as usize);
        let mut reports = Vec::with_capacity(samples as usize);
        for _ in 0..samples {
            let report = black_box(simulate(black_box(&request), false)?);
            timings.push(report.elapsed_ns);
            reports.push(report);
        }
        serde_json::to_string_pretty(&serde_json::json!({
            "warmups": warmups, "samples": samples,
            "elapsed_ns_samples": timings, "reports": reports,
        }))
    } else {
        serde_json::to_string_pretty(&simulate(&request, trace)?)
    }
    .map_err(|err| err.to_string())?;
    if let Some(path) = outfile {
        write_atomically(&path, &(output + "\n"))?;
    } else {
        println!("{output}");
    }
    Ok(())
}

fn main() {
    if let Err(failure) = run() {
        if let Some(message) = failure.message {
            eprintln!("{message}");
        }
        process::exit(failure.status);
    }
}

#[cfg(test)]
mod tests {
    use super::write_atomically;
    use std::fs;

    fn scratch(label: &str) -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("forever-main-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn names(directory: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_written_file_is_complete_and_alone() {
        let directory = scratch("whole");
        let path = directory.join("result.json");
        write_atomically(path.to_str().unwrap(), "first\n").unwrap();
        write_atomically(path.to_str().unwrap(), "second\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second\n");
        assert_eq!(names(&directory), ["result.json"]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_failed_write_leaves_the_destination_and_no_temporary_file() {
        let directory = scratch("failed");
        // A directory cannot be replaced by a file, so the final rename fails after the
        // temporary file was written.
        let path = directory.join("result.json");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("kept"), "kept").unwrap();
        assert!(write_atomically(path.to_str().unwrap(), "new\n").is_err());
        assert_eq!(names(&directory), ["result.json"]);
        assert_eq!(fs::read_to_string(path.join("kept")).unwrap(), "kept");
        fs::remove_dir_all(directory).unwrap();
    }
}
