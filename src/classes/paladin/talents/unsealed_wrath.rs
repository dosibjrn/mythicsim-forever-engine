//! Unsealed Wrath (1310735), push 113554's rework of Twist of Light under Forever's rules,
//! from upstream wowsims/forever #772's talents_retribution.go: each landed melee auto stores
//! the active seal in its Echo, one stack a swing up to the Echo's stacks; a swing landed under
//! a different seal spends every other Echo's stacks, each a roll of the stored seal.

use crate::{
    contracts::prepared_v2::SealEcho,
    core::fight::{Fight, SpellId, SpellResult, OUTCOME_LANDED},
};

use super::super::{
    agent::PaladinAgent,
    spells::seals::{self, SealKind},
};
use super::twist_of_light::Echo;

pub(crate) fn bind(fight: &Fight<PaladinAgent>, echoes: &[SealEcho]) -> Result<Vec<Echo>, String> {
    super::twist_of_light::bind(fight, echoes)
}

/// The Echoes forget their seals at reset.
pub(crate) fn reset(fight: &mut Fight<PaladinAgent>) {
    for echo in &mut fight.agent.unsealed_echoes {
        echo.seal = None;
    }
}

/// The trigger, at once on a landed melee auto: with no seal up nothing happens. Every other
/// seal's Echo that is up fades and replays its stored seal once per stack; then the active
/// seal's Echo stores it and gains a stack. A seal that leaves no Echo (the Crusader, Light and
/// Wisdom) spends the others and stores nothing.
pub(crate) fn on_spell_hit_dealt(
    fight: &mut Fight<PaladinAgent>,
    spell: SpellId,
    result: &SpellResult,
) {
    let state = &fight.spells[spell];
    if state.flags.proc || !state.white_hit || result.outcome & OUTCOME_LANDED == 0 {
        return;
    }
    let Some(active) = seals::active_seal(fight) else {
        return;
    };
    let active_name = fight.agent.seals.seals[active].kind.name();
    for index in 0..fight.agent.unsealed_echoes.len() {
        let echo = &fight.agent.unsealed_echoes[index];
        if echo.seal_name == active_name || !fight.aura(echo.aura).active {
            continue;
        }
        let aura = echo.aura;
        let stacks = fight.aura(aura).stacks;
        let stored = fight.agent.unsealed_echoes[index].seal.take();
        fight.deactivate_aura(aura);
        let Some(seal) = stored else {
            continue;
        };
        for _ in 0..stacks {
            replay(fight, seal, result);
        }
    }
    if let Some(index) = fight
        .agent
        .unsealed_echoes
        .iter()
        .position(|echo| echo.seal_name == active_name)
    {
        fight.agent.unsealed_echoes[index].seal = Some(active);
        let aura = fight.agent.unsealed_echoes[index].aura;
        fight.activate_aura(aura);
        fight.add_stack(aura);
    }
}

/// One stored copy: the seal's own effect with its own chance, as Twist of Light's Echo
/// replays it.
fn replay(fight: &mut Fight<PaladinAgent>, seal: usize, result: &SpellResult) {
    match fight.agent.seals.seals[seal].kind {
        SealKind::Command => seals::try_command(fight, seal, result.target),
        SealKind::Righteousness | SealKind::Fury => {
            let proc_spell = fight.agent.seals.seals[seal].proc_spell;
            fight.cast(proc_spell, result.target);
        }
        // Upstream: the Echo of Justice has nothing to replay.
        SealKind::Justice => {}
        SealKind::Crusader | SealKind::Light | SealKind::Wisdom => {
            unreachable!("Seal of the Crusader, Light and Wisdom leave no Echo")
        }
    }
}
