use forever_engine::{
    contracts::prepared_v2::PreparedV2, gear_options, get_options, native_item_options,
    native_player_context, native_talent_options, native_two_pull_windows,
    native_two_world_targets_with_decisions_recovery, native_two_world_targets_with_recovery,
    prepare_json_with_actor_level, prepared_refusals, simulate, simulate_prepared,
    simulate_prepared_gated, validate_prepared, Gated, OptionsClass, PrepareError, PreparedError,
    Refusal, Request, RngMode, Rules, SOURCE_REVISION,
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
}

/// Prepare directly from the exact source request. No raw-JSON/protobuf SHA
/// comparison: the native PreparedV2 carries the engine's canonical identity.
fn native_api_prepare(
    request: &serde_json::Value,
) -> Result<(serde_json::Value, PreparedV2), String> {
    let source = request.get("request").ok_or("prepare needs request")?;
    let bytes = serde_json::to_vec(source).map_err(|e| e.to_string())?;
    let actor_level = request
        .get("actor_level")
        .and_then(serde_json::Value::as_i64)
        .and_then(|n| i32::try_from(n).ok());
    let source_prepared = std::panic::catch_unwind(|| {
        prepare_json_with_actor_level(&bytes, "rust", Rules::Forever, actor_level)
    })
    .map_err(|_| "native prepare panicked".to_owned())?
    .map_err(|e| e.to_string())?;
    let value = serde_json::to_value(source_prepared).map_err(|e| e.to_string())?;
    let prepared = serde_json::from_value::<PreparedV2>(value.clone())
        .map_err(|e| format!("native prepared-v2 rejected: {e}"))?;
    Ok((value, prepared))
}

impl NativeApiSession {
    fn dispatch(&mut self, request: &serde_json::Value) -> Result<serde_json::Value, String> {
        match request.get("op").and_then(serde_json::Value::as_str) {
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
                    .map_err(|e| e.to_string())
            }
            Some("prepare") => {
                // On refusal no earlier actor may accidentally be evaluated.
                self.prepared = None;
                let (value, prepared) = native_api_prepare(request)?;
                self.prepared = Some(prepared);
                Ok(value)
            }
            Some("evaluate") => {
                // Evaluate either the actor retained by a preceding prepare in
                // this session, or prepare a new source inline in this call.
                if request.get("request").is_some() {
                    self.prepared = None;
                    let (_, prepared) = native_api_prepare(request)?;
                    self.prepared = Some(prepared);
                }
                let prepared = self
                    .prepared
                    .as_ref()
                    .ok_or("evaluate needs a preceding prepare or inline request")?;
                let targets = request
                    .get("targets")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|n| usize::try_from(n).ok())
                    .filter(|n| (2..=100).contains(n))
                    .ok_or("evaluate needs targets 2..100")?;
                let rng = request
                    .get("rng_mode")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("evaluate needs rng_mode")?;
                let mode = RngMode::parse_cli(rng)?;
                if request.get("recover_at_end").and_then(serde_json::Value::as_bool)
                    != Some(true)
                {
                    return Err("evaluate requires recover_at_end true".to_owned());
                }
                // This is the existing real two-dimensional gsim tick/ACK
                // boundary and native Decisions terminal dual-resource rest.
                // With FOREVER_GSIM_WORLD_BRIDGE=1 it writes world events to
                // stdout and consumes causal ACKs on stdin during this call.
                let report = native_two_world_targets_with_decisions_recovery(
                    prepared, targets, mode, 1.0, 1.0,
                )
                .map_err(|e| e.to_string())?;
                Ok(serde_json::json!({
                    "engine": "forever-engine",
                    "source_sha256": prepared.request_sha256,
                    "result": report,
                    "checkpoint_certified": false
                }))
            }
            _ => Err("unsupported API op (expected options, prepare or evaluate)".to_owned()),
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
            .map_err(|e| format!("invalid API request: {e}"))
            .and_then(|request| session.dispatch(&request))
        {
            Ok(result) => serde_json::json!({"ok": true, "result": result}),
            Err(error) => serde_json::json!({"ok": false, "error": error}),
        };
        println!("{}", response);
        std::io::stdout().flush().map_err(|e| e.to_string())?;
    }
}

#[cfg(test)]
mod native_optimizer_api_tests {
    use super::NativeApiSession;

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
