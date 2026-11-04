//! Prepared v2 execution for Hunter builds. The gate decides which inputs Rust supports;
//! supported inputs run in the shared fight runtime with the Hunter agent.

mod coverage;

pub(crate) use coverage::GATE;

/// Run a prepared v2 input that passed the coverage gate with the Hunter agent.
pub(crate) fn run_prepared(
    prepared: &crate::contracts::prepared_v2::PreparedV2,
    rng_mode: crate::core::rng::RngMode,
) -> Result<crate::core::fight::FightReport, String> {
    let mut fight = crate::classes::hunter::agent::HunterAgent::fight(prepared, rng_mode)?;
    Ok(fight.run())
}
