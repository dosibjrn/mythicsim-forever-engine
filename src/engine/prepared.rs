//! Prepared v2 entry point: identity and bound validation, then the spec coverage gate.
//!
//! Invalid input is a contract violation. Unsupported input is valid but outside the
//! implemented capability set; each reason carries a stable code a worker can record as
//! its fallback reason.

use crate::{
    classes,
    contracts::prepared_v2::{PreparedV2, CONTRACT, SCHEMA_VERSION},
    mechanics::{
        haste::cast_speed,
        mana::{regen_per_second_casting, regen_per_second_not_casting, RegenInputs},
    },
    rotation, SOURCE_REVISION,
};

pub use super::coverage::{Refusal, REFUSAL_CODES};

/// The client build whose data the pinned reference prepares, from upstream/sources.json.
pub const CLIENT_BUILD: &str = env!("FOREVER_CLIENT_BUILD");

const SECOND: i64 = 1_000_000_000;

#[derive(Debug, PartialEq)]
pub enum PreparedError {
    /// The input violates the contract or its identity does not match this engine.
    Invalid(String),
    /// The input is valid but uses mechanics this engine does not implement.
    Unsupported(Vec<String>),
}

impl std::fmt::Display for PreparedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PreparedError::Invalid(reason) => write!(f, "prepared input rejected: {reason}"),
            PreparedError::Unsupported(reasons) => {
                write!(f, "prepared input unsupported:")?;
                for reason in reasons {
                    write!(f, "\n  {reason}")?;
                }
                Ok(())
            }
        }
    }
}

/// Contract identity and wire limits. These never depend on implemented mechanics.
pub fn validate(prepared: &PreparedV2) -> Result<(), String> {
    if prepared.schema_version != SCHEMA_VERSION || prepared.contract != CONTRACT {
        return Err("unsupported schema version or contract".into());
    }
    if prepared.reference.engine_revision != SOURCE_REVISION {
        return Err(format!(
            "reference revision {} differs from {SOURCE_REVISION}",
            prepared.reference.engine_revision
        ));
    }
    if prepared.reference.client_build != CLIENT_BUILD {
        return Err(format!(
            "client build {} differs from {CLIENT_BUILD}",
            prepared.reference.client_build
        ));
    }
    if prepared.scenario_id.is_empty() || prepared.scenario_id.len() > 200 {
        return Err("scenario_id must contain 1 to 200 bytes".into());
    }
    if prepared.request_sha256.len() != 64
        || !prepared
            .request_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("request_sha256 must be 64 lowercase hexadecimal digits".into());
    }
    // Side::Pet(i) is the pet at position i, in Go unit index order, which fits in a u8.
    if prepared.pets.len() > usize::from(u8::MAX)
        || prepared
            .pets
            .windows(2)
            .any(|pair| pair[0].index >= pair[1].index)
    {
        return Err("pets must be fewer than 256, in increasing unit index order".into());
    }
    let sim = &prepared.sim;
    let encounter = &prepared.encounter;
    if !(1..=1_000_000).contains(&sim.iterations)
        || sim.seed <= 0
        || sim.seed > i64::MAX - i64::from(sim.iterations)
    {
        return Err("iterations must be 1 to 1000000 with a positive seed".into());
    }
    if !(SECOND..=600 * SECOND).contains(&encounter.duration_ns)
        || !(0..=120 * SECOND).contains(&encounter.duration_variation_ns)
        || encounter.duration_variation_ns >= encounter.duration_ns
    {
        return Err(
            "duration must be 1 to 600 seconds with a smaller variation of at most 120 seconds"
                .into(),
        );
    }
    let player = &prepared.player;
    if !(10_000_000..=SECOND).contains(&player.reaction_ns)
        || !(0..=SECOND).contains(&player.channel_clip_delay_ns)
        || !(0.0..=100.0).contains(&player.distance_yards)
    {
        return Err("reaction time, channel clip delay or distance is outside limits".into());
    }
    for (name, value) in [
        ("cast_speed", player.cast_speed),
        ("max_mana", player.mana.max),
        ("base_mana", player.mana.base),
        (
            "spirit_regen_per_second",
            player.mana.spirit_regen_per_second,
        ),
    ] {
        if !value.is_finite() || value < 0.0 {
            return Err(format!("{name} must be finite and nonnegative"));
        }
    }
    if player.cast_speed == 0.0 {
        return Err("cast_speed must be positive".into());
    }
    // A class without a mana bar, such as a Rogue, exports no mana at all and no mana costs.
    let manaless = player.mana.max == 0.0;
    if manaless {
        let mana_cost = player.spells.iter().any(|spell| {
            spell
                .cost
                .as_ref()
                .is_some_and(|cost| cost.resource == "mana")
        });
        if mana_cost
            || player.mana.base != 0.0
            || player.mana.teardown_max != 0.0
            || player.mana.regen_per_second_casting != 0.0
            || player.mana.regen_per_second_not_casting != 0.0
        {
            return Err("a player without mana must have no mana costs or regeneration".into());
        }
    } else if !(player.mana.teardown_max > 0.0 && player.mana.teardown_max <= player.mana.max) {
        return Err("mana teardown_max must be positive and at most max".into());
    }
    // Rust recomputes Go's starting regeneration from the exported components. A mismatch
    // means the components or the formula drifted, so the fight cannot be trusted.
    let pseudo = &player.pseudo_stats;
    let inputs = RegenInputs {
        mp5: *player.stats.get("MP5").ok_or("player stats lack MP5")?,
        spirit_regen_per_second: player.mana.spirit_regen_per_second,
        spirit_regen_rate_casting: pseudo.spirit_regen_rate_casting,
        force_full_spirit_regen: pseudo.force_full_spirit_regen,
        spirit_regen_multiplier: pseudo.spirit_regen_multiplier,
        mana_regen_multiplier: 1.0,
    };
    for (name, rust, go) in if manaless {
        Vec::new()
    } else {
        vec![
            (
                "casting",
                regen_per_second_casting(inputs),
                player.mana.regen_per_second_casting,
            ),
            (
                "not casting",
                regen_per_second_not_casting(inputs),
                player.mana.regen_per_second_not_casting,
            ),
        ]
    } {
        // Go may fuse multiply-add on some architectures; allow only that rounding.
        if (rust - go).abs() > 1e-12 * go.abs().max(1.0) {
            return Err(format!(
                "mana regeneration while {name} is {rust}, Go prepared {go}"
            ));
        }
    }
    // Rust recomputes cast speed when an aura multiplies it, so the formula must agree with
    // the cast speed Go prepared.
    let haste_rating = *player
        .stats
        .get("SpellHasteRating")
        .ok_or("player stats lack SpellHasteRating")?;
    let speed = cast_speed(pseudo.cast_speed_multiplier, haste_rating);
    if (speed - player.cast_speed).abs() > 1e-12 * player.cast_speed {
        return Err(format!(
            "cast speed is {speed}, Go prepared {}",
            player.cast_speed
        ));
    }
    for spell in &player.spells {
        let cast = &spell.default_cast;
        if cast.gcd_ns < 0 || cast.cast_time_ns < 0 || cast.gcd_min_ns < 0 {
            return Err(format!(
                "spell {:?} has a negative cast timing",
                spell.action_id
            ));
        }
        if !spell.missile_speed.is_finite() || spell.missile_speed < 0.0 {
            return Err(format!(
                "spell {:?} has an invalid missile speed",
                spell.action_id
            ));
        }
    }
    Ok(())
}

/// Every reason the engine cannot simulate a valid input, each with its stable code. Empty
/// means supported.
pub fn refusals(prepared: &PreparedV2) -> Vec<Refusal> {
    let mut refusals: Vec<Refusal> = prepared
        .unrepresented
        .iter()
        .map(|reason| {
            Refusal::new(
                "exporter_unrepresented",
                format!("unrepresented by the exporter: {reason}"),
            )
        })
        .collect();
    let rotation = match rotation::parse(&prepared.player.rotation) {
        Ok(rotation) => Some(rotation),
        Err(rotation_reasons) => {
            refusals.extend(
                rotation_reasons
                    .into_iter()
                    .map(|reason| Refusal::new("rotation_unsupported", reason)),
            );
            None
        }
    };
    refusals.extend(super::coverage::prepared_coverage(
        prepared,
        rotation.as_ref(),
    ));
    refusals
}

/// Every reason the engine cannot simulate a valid input. Empty means supported.
pub fn coverage(prepared: &PreparedV2) -> Vec<String> {
    refusals(prepared)
        .into_iter()
        .map(|refusal| refusal.reason)
        .collect()
}

/// Validate and gate a prepared v2 input.
pub fn check(prepared: &PreparedV2) -> Result<(), PreparedError> {
    validate(prepared).map_err(PreparedError::Invalid)?;
    let reasons = coverage(prepared);
    if reasons.is_empty() {
        Ok(())
    } else {
        Err(PreparedError::Unsupported(reasons))
    }
}

/// Validate the route identity and its consecutive level progression. This is scenario
/// continuity only: spell/talent legality and combat effects remain owned by prepared-v2/Rust.
fn validate_route_segments(segments: &[PreparedV2]) -> Result<(), PreparedError> {
    if !(2..=60).contains(&segments.len()) {
        return Err(PreparedError::Invalid(
            "RUST_LEVEL_ROUTE_REQUIRES_2_TO_60_CONSECUTIVE_LEVELS".to_owned(),
        ));
    }
    let first = &segments[0].player;
    for (index, segment) in segments.iter().enumerate() {
        let player = &segment.player;
        if player.class != first.class
            || player.race != first.race
            || player.name != first.name
            || player.index != first.index
        {
            return Err(PreparedError::Invalid(format!(
                "RUST_LEVEL_ROUTE_ACTOR_IDENTITY_CHANGED_AT_SEGMENT_{index}"
            )));
        }
        let expected_level = first.level.checked_add(index as i32).ok_or_else(|| {
            PreparedError::Invalid("RUST_LEVEL_ROUTE_LEVEL_OVERFLOW".to_owned())
        })?;
        if player.level != expected_level {
            return Err(PreparedError::Invalid(format!(
                "RUST_LEVEL_ROUTE_LEVEL_SEQUENCE_INVALID_AT_SEGMENT_{index}: expected {expected_level}, got {}",
                player.level
            )));
        }
    }
    Ok(())
}

/// Which engine made a result, so results of different engines or builds are never compared by
/// mistake.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct EngineIdentity {
    /// The engine's name, as release/manifest.json states it.
    pub engine: String,
    /// The crate version, equal to the release manifest's package version.
    pub package_version: String,
    /// The pinned Go reference revision whose behavior the engine reproduces.
    pub go_reference_revision: String,
    /// The client build of the reference's game data.
    pub client_build: String,
    /// The schema version of release/manifest.json, which declares the engine's capabilities.
    pub release_manifest_schema_version: u32,
}

impl EngineIdentity {
    /// This build's identity.
    pub fn current() -> Self {
        EngineIdentity {
            engine: "forever-engine".into(),
            package_version: env!("CARGO_PKG_VERSION").into(),
            go_reference_revision: SOURCE_REVISION.into(),
            client_build: CLIENT_BUILD.into(),
            release_manifest_schema_version: env!("FOREVER_RELEASE_MANIFEST_SCHEMA_VERSION")
                .parse()
                .expect("the build script writes a number"),
        }
    }
}

/// The engine identity and result of a prepared v2 simulation. `result` follows Go's
/// `RaidSimResult` JSON for the fields Rust implements.
#[derive(Debug, serde::Serialize)]
pub struct PreparedReport {
    pub engine: String,
    pub schema_version: u32,
    pub source_revision: String,
    pub identity: EngineIdentity,
    pub scenario_id: String,
    pub request_sha256: String,
    pub elapsed_ns: u64,
    pub result: serde_json::Value,
    /// The rotation as MythicSim's Go CLI adapter presents it, kept out of the result unless
    /// [`PreparedReport::add_rotation_stats`] adds it.
    #[serde(skip)]
    presentation: Option<crate::core::fight::rotation_checks::Presentation>,
    /// The logged fight's queued strikes that went out on an extra attack, kept out of the
    /// result unless [`PreparedReport::add_rotation_stats`] adds them.
    #[serde(skip)]
    extra_attack_strikes: Vec<serde_json::Value>,
}

impl PreparedReport {
    /// Add `effectiveRotation`, `rotationStats` and `rotationPresentationVersion` to the
    /// player's metrics, as MythicSim's Go CLI adapter does for the application: the rotation
    /// that can run and Go's validation messages of each of its lines. Go's plain CLI, whose
    /// results the goldens are, writes none of them. False when the result has no player.
    pub fn add_rotation_stats(&mut self) -> bool {
        let (Some(presentation), Some(serde_json::Value::Object(player))) = (
            self.presentation.as_ref(),
            self.result.pointer_mut("/raidMetrics/parties/0/players/0"),
        ) else {
            return false;
        };
        presentation.add_to(player);
        // Each queued strike, such as Heroic Strike, that went out on a Windfury Totem or
        // Weaponmaster extra attack in the logged fight, so the timeline can say so.
        player.insert(
            "extraAttackStrikes".into(),
            serde_json::Value::Array(self.extra_attack_strikes.clone()),
        );
        true
    }
}

/// Validate, gate and simulate a prepared v2 input.
pub fn simulate(prepared: &PreparedV2, rng_mode: crate::core::rng::RngMode) -> Result<PreparedReport, PreparedError> {
    check(prepared)?;
    run(prepared, rng_mode)
}


/// One selected source-native actor and monotonic gsim ACKs; diagnostic only.
pub fn native_two_world_targets(prepared: &PreparedV2, target_count: usize, rng_mode: crate::core::rng::RngMode)
    -> Result<serde_json::Value, PreparedError>
{
    check(prepared)?;
    if !crate::core::fight::world_bridge::enabled() {
        return Err(PreparedError::Invalid(
            "RUST_MULTI_WORLD_TARGETS_REQUIRES_GSIM".to_owned()));
    }
    if !(2..=100).contains(&target_count) {
        return Err(PreparedError::Invalid(
            "RUST_MULTI_WORLD_TARGET_COUNT_INVALID".to_owned()));
    }
    crate::core::fight::world_bridge::start(&prepared.request_sha256);
    crate::classes::native_two_world_targets(prepared, target_count, rng_mode)
        .map_err(PreparedError::Invalid)
}

/// Execute consecutive prepared level segments as one native 2D world route. Each segment
/// is independently prepared-v2 checked; the world bridge is started once and its lethal ACK
/// chain, actor resources, pet resources, RNG stream and monotonic clock continue across levels.
pub fn native_level_route(
    segments: &[PreparedV2],
    target_count_per_level: usize,
    rng_mode: crate::core::rng::RngMode,
) -> Result<serde_json::Value, PreparedError> {
    if !(2..=100).contains(&target_count_per_level) {
        return Err(PreparedError::Invalid(
            "RUST_MULTI_WORLD_TARGET_COUNT_INVALID".to_owned(),
        ));
    }
    validate_route_segments(segments)?;
    for segment in segments {
        check(segment)?;
    }
    if !crate::core::fight::world_bridge::enabled() {
        return Err(PreparedError::Invalid(
            "RUST_MULTI_WORLD_TARGETS_REQUIRES_GSIM".to_owned(),
        ));
    }
    // The first prepared request anchors the single bridge session. Per-level request hashes
    // are all retained in the route report and are checked before any world work begins.
    crate::core::fight::world_bridge::start(&segments[0].request_sha256);
    crate::classes::native_level_route(segments, target_count_per_level, rng_mode)
        .map_err(PreparedError::Invalid)
}

/// A real native two-window same-actor smoke; no world ACK, mob kills or paid recovery.
pub fn native_two_pull_windows(prepared: &PreparedV2, rng_mode: crate::core::rng::RngMode)
    -> Result<serde_json::Value, PreparedError>
{
    check(prepared)?;
    crate::classes::native_two_pull_windows(prepared, rng_mode).map_err(PreparedError::Invalid)
}

/// What gating a prepared v2 input and simulating it when supported came to.
#[derive(Debug)]
pub enum Gated {
    /// The gate supports the input and this is its simulation.
    Simulated(Box<PreparedReport>),
    /// The gate refuses the input, with each reason and its stable code. Nothing ran.
    Refused(Vec<Refusal>),
}

/// Validate an input, gate it once and simulate it when the gate supports it, so a caller
/// that wants both needs one pass over the input instead of a [`refusals`] call and then a
/// [`simulate`] call that gates it again. A refusal is a result here, not an error; invalid
/// input is still [`PreparedError::Invalid`].
pub fn simulate_gated(prepared: &PreparedV2, rng_mode: crate::core::rng::RngMode) -> Result<Gated, PreparedError> {
    validate(prepared).map_err(PreparedError::Invalid)?;
    let refusals = refusals(prepared);
    if !refusals.is_empty() {
        return Ok(Gated::Refused(refusals));
    }
    run(prepared, rng_mode).map(|report| Gated::Simulated(Box::new(report)))
}

/// Simulate an input that is valid and that the gate supports.
fn run(prepared: &PreparedV2, rng_mode: crate::core::rng::RngMode) -> Result<PreparedReport, PreparedError> {
    if crate::core::fight::world_bridge::enabled() {
        crate::core::fight::world_bridge::start(&prepared.request_sha256);
    }
    let report = classes::run_prepared(prepared, rng_mode).map_err(PreparedError::Invalid)?;
    let elapsed_ns = report.elapsed_ns;
    let presentation = report.presentation.clone();
    let extra_attack_strikes = report.extra_attack_strikes.clone();
    Ok(PreparedReport {
        engine: format!("forever-rust-{}", env!("CARGO_PKG_VERSION")),
        schema_version: SCHEMA_VERSION,
        source_revision: SOURCE_REVISION.into(),
        identity: EngineIdentity::current(),
        scenario_id: prepared.scenario_id.clone(),
        request_sha256: prepared.request_sha256.clone(),
        elapsed_ns,
        result: serde_json::to_value(&report)
            .map_err(|err| PreparedError::Invalid(err.to_string()))?,
        presentation,
        extra_attack_strikes,
    })
}


#[cfg(test)]
mod native_level_route_tests {
    use super::*;

    fn fixture() -> PreparedV2 {
        serde_json::from_str(include_str!(
            "../../fixtures/mage/prepared-v2/balance-druid-revelation.prepared.json"
        )).expect("prepared-v2 fixture")
    }

    #[test]
    fn native_route_requires_same_actor_and_consecutive_levels() {
        let mut first = fixture();
        let mut second = fixture();
        first.player.level = 59;
        second.player.level = 60;
        assert!(validate_route_segments(&[first, second]).is_ok());
    }

    #[test]
    fn native_route_rejects_level_gaps_and_actor_changes() {
        let mut first = fixture();
        let mut second = fixture();
        first.player.level = 59;
        second.player.level = 61;
        assert!(matches!(
            validate_route_segments(&[first, second]),
            Err(PreparedError::Invalid(_))
        ));

        let mut first = fixture();
        let mut second = fixture();
        first.player.level = 59;
        second.player.level = 60;
        second.player.class = "ClassMage".to_owned();
        assert!(matches!(
            validate_route_segments(&[first, second]),
            Err(PreparedError::Invalid(_))
        ));
    }
}
