//! The same input and seed give the same result in every deployment condition.
//!
//! The engine runs a request on one thread: iterations execute in order, each reseeds the
//! random stream from the request's seed plus its index, and nothing reads the environment,
//! a worker count or the clock apart from the `elapsed_ns` timing fields. A deployment varies
//! what surrounds that: how many workers run requests side by side, in what order they pick
//! them up, how many processes run at once, and what each process's environment holds. These
//! tests run the same inputs under each of those conditions and require the output, apart
//! from the timing fields, to match byte for byte.
//!
//! Each fresh process also seeds Rust's hash maps differently, so any result that depended on
//! hash map iteration order would differ between the processes compared here.

use forever_engine::{contracts::prepared_v2::PreparedV2, simulate, simulate_prepared, Request};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};

/// One input: a prepared v2 fixture across several classes, pets and fight shapes, or a
/// prepared v1 request, which runs through its own entry point and records a trace.
struct Job {
    name: &'static str,
    path: PathBuf,
    prepared: bool,
}

fn jobs() -> Vec<Job> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let prepared = [
        "frostbolt-shared-boundary",
        "tank-frost-mage",
        "feral-bear-druid-survives",
        "feral-bear-druid-boomerang-pushback",
        "fury-protection-warrior-winter-night",
        "protection-paladin-dragons-call-hardcast",
        "survival-melee-hunter-cat-heartbeat",
        "marksmanship-hunter-hands-of-power",
        "affliction-warlock-lash-of-pain-rounding",
    ];
    let v1 = ["static-frost-60", "meditation-300-oom"];
    let mut jobs: Vec<Job> = prepared
        .into_iter()
        .map(|name| Job {
            name,
            path: root
                .join("mage/prepared-v2")
                .join(format!("{name}.prepared.json")),
            prepared: true,
        })
        .collect();
    jobs.extend(v1.into_iter().map(|name| Job {
        name,
        path: root.join(format!("{name}.rust.json")),
        prepared: false,
    }));
    jobs
}

/// The report as the command prints it, without the lines that record wall time. Timing is
/// the one field allowed to differ between runs of the same input.
fn without_timing(report: &str) -> String {
    report
        .lines()
        .filter(|line| {
            let key = line.trim_start();
            !key.starts_with("\"elapsed_ns\":") && !key.starts_with("\"elapsedNs\":")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Runs a job in this process the way the command does and prints its report.
fn run_in_process(job: &Job) -> String {
    let input = fs::read(&job.path).unwrap();
    let text = if job.prepared {
        let prepared: PreparedV2 = serde_json::from_slice(&input).unwrap();
        serde_json::to_string_pretty(&simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap())
    } else {
        let request: Request = serde_json::from_slice(&input).unwrap();
        serde_json::to_string_pretty(&simulate(&request, true).unwrap())
    };
    without_timing(&text.unwrap())
}

/// Every job once, sequentially, in listed order. The others are compared with this.
fn reference(jobs: &[Job]) -> BTreeMap<&'static str, String> {
    jobs.iter()
        .map(|job| (job.name, run_in_process(job)))
        .collect()
}

/// A deterministic shuffle, so a failing order can be repeated. A small linear congruential
/// generator avoids a dependency for a test helper.
fn shuffled(count: usize, mut state: u64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..count).collect();
    for index in (1..count).rev() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        order.swap(index, (state >> 33) as usize % (index + 1));
    }
    order
}

/// The orders a deployment might hand jobs out in: as listed, reversed and shuffled.
fn orders(count: usize) -> Vec<Vec<usize>> {
    vec![
        (0..count).collect(),
        (0..count).rev().collect(),
        shuffled(count, 1),
        shuffled(count, 2),
    ]
}

fn assert_matches(
    context: &str,
    expected: &BTreeMap<&'static str, String>,
    actual: &[(&'static str, String)],
) {
    assert_eq!(actual.len(), expected.len(), "{context}: jobs run");
    for (name, report) in actual {
        assert!(
            report == &expected[name],
            "{context}: {name} differs from the sequential run"
        );
    }
}

#[test]
fn results_do_not_depend_on_worker_count_or_order() {
    let jobs = jobs();
    let expected = reference(&jobs);
    for workers in [1, 2, 3, 8] {
        for (position, order) in orders(jobs.len()).into_iter().enumerate() {
            // Workers take the next job off a shared list, as a pool of request workers does.
            let next = AtomicUsize::new(0);
            let done = Mutex::new(Vec::new());
            std::thread::scope(|scope| {
                for _ in 0..workers {
                    scope.spawn(|| loop {
                        let slot = next.fetch_add(1, Ordering::SeqCst);
                        let Some(&index) = order.get(slot) else {
                            break;
                        };
                        let report = run_in_process(&jobs[index]);
                        done.lock().unwrap().push((jobs[index].name, report));
                    });
                }
            });
            assert_matches(
                &format!("{workers} workers, order {position}"),
                &expected,
                &done.into_inner().unwrap(),
            );
        }
    }
}

#[test]
fn every_worker_may_run_the_same_input_at_once() {
    let jobs = jobs();
    let job = &jobs[3];
    let expected = run_in_process(job);
    let done = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| done.lock().unwrap().push(run_in_process(job)));
        }
    });
    let done = done.into_inner().unwrap();
    assert_eq!(done.len(), 8);
    assert!(done.iter().all(|report| report == &expected));
}

#[test]
fn a_run_repeated_in_one_process_repeats_its_result() {
    let jobs = jobs();
    for job in &jobs {
        assert!(
            run_in_process(job) == run_in_process(job),
            "{} changed between two runs in one process",
            job.name
        );
    }
}

/// The environment variables that tune worker pools and runtimes elsewhere. The engine reads
/// none of them, and the test holds it to that.
const POOL_SETTINGS: [&str; 5] = [
    "RAYON_NUM_THREADS",
    "RUST_TEST_THREADS",
    "GOMAXPROCS",
    "OMP_NUM_THREADS",
    "TOKIO_WORKER_THREADS",
];

fn scratch(label: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "forever-determinism-{label}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    directory
}

#[test]
fn parallel_processes_agree_with_each_other_and_with_a_library_run() {
    let jobs = jobs();
    let expected = reference(&jobs);
    let directory = scratch("processes");
    for (round, order) in orders(jobs.len()).into_iter().enumerate() {
        // The whole round starts together: more processes than most machines have cores.
        let started: Vec<(usize, PathBuf, std::process::Child)> = order
            .iter()
            .chain(order.iter().rev())
            .enumerate()
            .map(|(launch, &index)| {
                let job = &jobs[index];
                let outfile = directory.join(format!("{round}-{launch}-{}.json", job.name));
                let mut command = Command::new(env!("CARGO_BIN_EXE_forever-engine"));
                command
                    .args(["sim", "--infile"])
                    .arg(&job.path)
                    .arg("--outfile")
                    .arg(&outfile);
                if !job.prepared {
                    command.arg("--trace");
                }
                // Each process gets different worker settings.
                for (setting, name) in POOL_SETTINGS.iter().enumerate() {
                    command.env(name, ((launch + setting) % 7 + 1).to_string());
                }
                (index, outfile, command.spawn().unwrap())
            })
            .collect();
        for (index, outfile, mut child) in started {
            assert!(child.wait().unwrap().success(), "{}", jobs[index].name);
            let report = without_timing(fs::read_to_string(&outfile).unwrap().trim_end());
            assert!(
                report == expected[jobs[index].name],
                "round {round}: {} from a parallel process differs from the library run",
                jobs[index].name
            );
        }
    }
    fs::remove_dir_all(directory).unwrap();
}

/// The comparisons above hold only if the output depends on the input, so a different seed
/// must give a different result, and the timing lines are the only ones that differ between
/// two runs of the same input.
#[test]
fn the_seed_changes_the_result_and_timing_is_the_only_noise() {
    let jobs = jobs();
    let job = &jobs[3];
    let mut prepared: serde_json::Value =
        serde_json::from_slice(&fs::read(&job.path).unwrap()).unwrap();
    let original = run_in_process(job);
    let seed = prepared["sim"]["seed"].as_i64().unwrap();
    prepared["sim"]["seed"] = (seed + 1).into();
    let reseeded: PreparedV2 = serde_json::from_value(prepared).unwrap();
    let other = without_timing(
        &serde_json::to_string_pretty(&simulate_prepared(&reseeded, forever_engine::RngMode::Stochastic).unwrap()).unwrap(),
    );
    assert!(original != other, "another seed gave the same report");

    let input: PreparedV2 = serde_json::from_slice(&fs::read(&job.path).unwrap()).unwrap();
    let printed = |report| serde_json::to_string_pretty(&report).unwrap();
    let (first, second) = (
        printed(simulate_prepared(&input, forever_engine::RngMode::Stochastic).unwrap()),
        printed(simulate_prepared(&input, forever_engine::RngMode::Stochastic).unwrap()),
    );
    let noisy = |report: &str| report.lines().map(str::to_owned).collect::<Vec<_>>();
    let differing = noisy(&first)
        .into_iter()
        .zip(noisy(&second))
        .filter(|(a, b)| a != b)
        .all(|(a, _)| a.contains("elapsed_ns") || a.contains("elapsedNs"));
    assert!(differing, "two runs differ outside the timing fields");
}
