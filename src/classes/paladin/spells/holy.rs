//! Holy Shock, Light's Vigil and Divine Favor, from Go sim/paladin/holy_shock.go,
//! lights_vigil.go and divine_favor.go, and the triggers that speed Holy Light.

use crate::core::fight::{
    AuraRef, Fight, ModId, ModKind, Side, SpellId, SpellResult, OUTCOME_CRIT,
};

use super::super::agent::PaladinAgent;

/// One Light's Vigil rank: its aura on the target, its strike, the refund share and the cost
/// of the last cast.
#[derive(Clone, Debug)]
pub(crate) struct Vigil {
    pub(crate) aura: AuraRef,
    pub(crate) strike: SpellId,
    pub(crate) refund: f64,
    pub(crate) metrics: usize,
    pub(crate) cost: f64,
}

/// Light's Vigil's cast: one vigil per paladin, so a new one replaces any that is up.
pub(crate) fn lights_vigil(fight: &mut Fight<PaladinAgent>, spell: SpellId, rank: usize) {
    fight.agent.vigils[rank].cost = fight.spells[spell].cur_cast.cost;
    for other in 0..fight.agent.vigils.len() {
        let aura = fight.agent.vigils[other].aura;
        if fight.aura(aura).active {
            fight.deactivate_aura(aura);
        }
    }
    let aura = fight.agent.vigils[rank].aura;
    fight.activate_aura(aura);
}

/// Light's Vigil on an ally (push 113554, the Holy Paladin healer sim): one rank's vigil aura on
/// the player, who stands for the heal target, and the party heal it triggers.
#[derive(Clone, Copy, Debug)]
pub(crate) struct VigilAlly {
    pub(crate) aura: AuraRef,
    /// The party heal's spell.
    pub(crate) heal: SpellId,
    pub(crate) average: f64,
    pub(crate) variance: f64,
    pub(crate) players: u32,
}

/// The cast: one vigil per paladin, so a new one replaces any that is up.
pub(crate) fn swear_vigil(fight: &mut Fight<PaladinAgent>, rank: usize) {
    for other in 0..fight.agent.vigil_allies.len() {
        let aura = fight.agent.vigil_allies[other].aura;
        if fight.aura(aura).active {
            fight.deactivate_aura(aura);
        }
    }
    let aura = fight.agent.vigil_allies[rank].aura;
    fight.activate_aura(aura);
}

/// A Holy Shock heal on the player: a vigil on them is spent, the Holy Shock triggers no
/// cooldown and heals the player and the rest of their party, each with a roll and a crit of its
/// own. `healing` and `bonus_healing_taken` are the player's, as the Holy Shock heal's.
pub(crate) fn vigil_party_heal(
    fight: &mut Fight<PaladinAgent>,
    holy_shock: SpellId,
    healing: crate::core::fight::healing::Healing,
    bonus_healing_taken: f64,
) {
    let Some(vigil) = fight
        .agent
        .vigil_allies
        .iter()
        .copied()
        .find(|vigil| fight.aura(vigil.aura).active)
    else {
        return;
    };
    fight.deactivate_aura(vigil.aura);
    if let Some((timer, _)) = fight.spells[holy_shock].cd {
        fight.timers[timer] = crate::core::time::STARTING_CD_TIME;
    }
    for _ in 0..vigil.players {
        let base = fight.effect_roll(vigil.average, vigil.variance);
        fight.calc_and_deal_healing(vigil.heal, Side::Player, base, healing, bonus_healing_taken);
    }
}

/// Light's Vigil's strike: the rolled damage, a magic hit and crit.
pub(crate) fn vigil_strike(fight: &mut Fight<PaladinAgent>, spell: SpellId, target: Side) {
    let base = fight.roll_damage_effect(spell);
    let result = fight.calc_damage(spell, target, base);
    fight.deal_damage(spell, result, false);
}

/// The trigger of Infusion of Light (talents_holy.go), on a crit from Holy Shock or Flash of
/// Light, or of the Libram of Holy Alacrity (item_librams.go), on a Holy Shock cast: its aura
/// a batch window later. The aura only speeds a Holy Light cast, which no rotation in scope
/// reaches. `result` is the hit for Infusion of Light and none for a cast.
pub(crate) fn holy_light_haste_trigger(
    fight: &mut Fight<PaladinAgent>,
    trigger: AuraRef,
    spell: SpellId,
    result: Option<&SpellResult>,
) {
    let state = &fight.spells[spell];
    let names: &[&str] = if result.is_some() {
        &["holy_shock", "holy_shock_heal", "flash_of_light"]
    } else {
        &["holy_shock", "holy_shock_heal"]
    };
    let named = state
        .class_spell
        .as_deref()
        .is_some_and(|name| names.contains(&name));
    if state.flags.proc || !named {
        return;
    }
    let result = match result {
        Some(result) if result.outcome & OUTCOME_CRIT == 0 => return,
        Some(result) => *result,
        None => SpellResult {
            armor_multiplier: 0.0,
            target: Side::Target,
            attacker: fight.spells[spell].caster,
            outcome: 0,
            damage: 0.0,
            threat: 0.0,
        },
    };
    fight.schedule_delayed_proc(trigger, spell, result);
}

/// Holy Shock's damage: the rank's roll, a magic hit and crit, unless the target holds a
/// vigil, which the cast consumes instead: the strike, the refund, and no cooldown.
pub(crate) fn holy_shock(
    fight: &mut Fight<PaladinAgent>,
    spell: SpellId,
    target: Side,
    (min, max): (f64, f64),
) {
    let vigil = (0..fight.agent.vigils.len())
        .find(|&rank| fight.aura(fight.agent.vigils[rank].aura).active);
    if let Some(rank) = vigil {
        let vigil = fight.agent.vigils[rank].clone();
        fight.deactivate_aura(vigil.aura);
        fight.cast(vigil.strike, target);
        fight.add_mana(vigil.cost * vigil.refund, vigil.metrics);
        if let Some((timer, _)) = fight.spells[spell].cd {
            fight.timers[timer] = crate::core::time::STARTING_CD_TIME;
        }
        return;
    }
    let base = fight.go_roll(min, max);
    let result = fight.calc_damage(spell, target, base);
    fight.deal_damage(spell, result, false);
}

/// Divine Favor: its aura carries a crit mod on the spells it names and fades when one of
/// them is cast.
#[derive(Clone, Debug)]
pub(crate) struct DivineFavor {
    pub(crate) aura: AuraRef,
    pub(crate) crit_mod: ModId,
    /// The spells whose casts spend the aura.
    pub(crate) spells: Vec<bool>,
}

impl DivineFavor {
    pub(crate) fn bind(
        fight: &mut Fight<PaladinAgent>,
        aura: &str,
        crit: f64,
        names: &[String],
    ) -> Result<Self, String> {
        let aura = fight.player_aura(aura)?;
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let affected = fight.spells_with_class(&names);
        // The trigger matches the class mask alone, mods or not.
        let spells = fight
            .spells
            .iter()
            .map(|spell| {
                spell
                    .class_spell
                    .as_deref()
                    .is_some_and(|class| names.contains(&class))
            })
            .collect();
        let crit_mod = fight.register_mod(ModKind::BonusCritPercent, crit, 0, affected);
        Ok(DivineFavor {
            aura,
            crit_mod,
            spells,
        })
    }

    /// The spell activates the aura.
    pub(crate) fn apply(&self, fight: &mut Fight<PaladinAgent>) {
        fight.activate_aura(self.aura);
    }

    pub(crate) fn on_gain(&self, fight: &mut Fight<PaladinAgent>) {
        fight.activate_mod(self.crit_mod);
    }

    pub(crate) fn on_expire(&self, fight: &mut Fight<PaladinAgent>) {
        fight.deactivate_mod(self.crit_mod);
    }

    /// The aura's trigger: a cast of a named spell, procs included, spends it at once.
    pub(crate) fn on_cast_complete(&self, fight: &mut Fight<PaladinAgent>, spell: SpellId) {
        if self.spells[spell] {
            fight.deactivate_aura(self.aura);
        }
    }
}
