//! Twist of Light, from Go sim/paladin/talents_retribution.go: replacing a seal leaves an
//! Echo of it, and the next landed white hit replays the replaced seal and consumes it.

use crate::{
    contracts::prepared_v2::SealEcho,
    core::fight::{AuraRef, Fight, SpellId, SpellResult, OUTCOME_LANDED},
};

use super::super::{
    agent::PaladinAgent,
    spells::seals::{self, SealKind},
};

/// One Echo: its aura on the paladin and the seal rank whose effect it holds.
#[derive(Clone, Debug)]
pub(crate) struct Echo {
    pub(crate) aura: AuraRef,
    /// The seal the Echo stands for, by class spell name.
    pub(crate) seal_name: String,
    /// Go `sealEcho.seal`.
    pub(crate) seal: Option<usize>,
}

pub(crate) fn bind(fight: &Fight<PaladinAgent>, echoes: &[SealEcho]) -> Result<Vec<Echo>, String> {
    echoes
        .iter()
        .map(|echo| {
            Ok(Echo {
                aura: fight.player_aura(&echo.aura)?,
                seal_name: echo.seal.clone(),
                seal: None,
            })
        })
        .collect()
}

/// Go `applySeal`'s Echo: the replaced seal's Echo remembers it and activates.
pub(crate) fn leave_echo(fight: &mut Fight<PaladinAgent>, kind: SealKind, seal: usize) {
    let Some(echo) = fight
        .agent
        .echoes
        .iter()
        .position(|echo| echo.seal_name == kind.name())
    else {
        return;
    };
    fight.agent.echoes[echo].seal = Some(seal);
    let aura = fight.agent.echoes[echo].aura;
    fight.activate_aura(aura);
}

/// Go `Paladin.Reset`: the Echoes forget their seals.
pub(crate) fn reset(fight: &mut Fight<PaladinAgent>) {
    for echo in &mut fight.agent.echoes {
        echo.seal = None;
    }
}

/// The permanent trigger: on a landed white hit, every Echo that is up replays its seal, in
/// the fixed order, and fades.
pub(crate) fn on_spell_hit_dealt(
    fight: &mut Fight<PaladinAgent>,
    spell: SpellId,
    result: &SpellResult,
) {
    let state = &fight.spells[spell];
    if state.flags.proc || !state.white_hit || result.outcome & OUTCOME_LANDED == 0 {
        return;
    }
    for index in 0..fight.agent.echoes.len() {
        let aura = fight.agent.echoes[index].aura;
        if !fight.aura(aura).active {
            continue;
        }
        if let Some(seal) = fight.agent.echoes[index].seal {
            match fight.agent.seals.seals[seal].kind {
                SealKind::Command => seals::try_command(fight, seal, result.target),
                SealKind::Righteousness | SealKind::Fury => {
                    let proc_spell = fight.agent.seals.seals[seal].proc_spell;
                    fight.cast(proc_spell, result.target);
                }
                // Go: the Echo of Justice has nothing to replay.
                SealKind::Justice => {}
                SealKind::Crusader | SealKind::Light | SealKind::Wisdom => {
                    unreachable!("Seal of the Crusader, Light and Wisdom leave no Echo")
                }
            }
        }
        fight.agent.echoes[index].seal = None;
        fight.deactivate_aura(aura);
    }
}
