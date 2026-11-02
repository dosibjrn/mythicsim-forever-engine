//! Seals, from Go sim/paladin/seals.go, seal_of_command.go, seal_of_righteousness.go,
//! seal_of_fury.go and seal_of_the_crusader.go: one seal up at a time, its aura's proc on
//! landed white hits, and the damage a batch window after the hit. Seal of Fury's hit also
//! grants an absorb shield while the paladin carries a shield. Seal of the Crusader has no
//! proc: its aura changes attack power, melee speed and the main hand auto's damage while it
//! holds. Seal of Light (seal_of_light.go) rolls its proc manager at the hit and heals the
//! paladin a batch window later. Seal of Wisdom (seal_of_wisdom.go) restores mana a batch window
//! after every landed melee hit that is no proc. Seal of Justice (seal_of_justice.go) does
//! nothing: raid bosses are immune to its stun.

use crate::{
    contracts::prepared_v2::{ActionId, Effect},
    core::{
        fight::{
            healing::Healing, melee::PhysicalOutcome, Action, AuraRef, Fight, ModId, ModKind,
            Outcome, Side, SpellId, SpellResult, OUTCOME_LANDED, PRIORITY_LOW,
        },
        time::STARTING_CD_TIME,
    },
};

use super::super::agent::PaladinAgent;

/// Which seal a rank belongs to, for its proc and its Twist of Light Echo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SealKind {
    Command,
    Righteousness,
    Crusader,
    Fury,
    Light,
    Wisdom,
    Justice,
}

impl SealKind {
    /// The class spell name the exporter writes for the seal.
    pub(crate) fn name(self) -> &'static str {
        match self {
            SealKind::Command => "seal_of_command",
            SealKind::Righteousness => "seal_of_righteousness",
            SealKind::Crusader => "seal_of_the_crusader",
            SealKind::Fury => "seal_of_fury",
            SealKind::Light => "seal_of_light",
            SealKind::Wisdom => "seal_of_wisdom",
            SealKind::Justice => "seal_of_justice",
        }
    }
}

/// One rank of one seal: Go `sealConfig`.
#[derive(Clone, Debug)]
pub(crate) struct Seal {
    pub(crate) kind: SealKind,
    /// The castable seal spell.
    pub(crate) spell: SpellId,
    pub(crate) aura: AuraRef,
    pub(crate) judgement: SpellId,
    /// The damage spell the seal fires.
    pub(crate) proc_spell: SpellId,
    /// Seal of Righteousness: the per-hit value per hundred of swing speed; Seal of Fury: the
    /// proc's damage; Seal of Light: the proc's heal.
    pub(crate) per_hit_value: f64,
    /// Seal of Command: the rank's own proc cooldown.
    pub(crate) icd_timer: usize,
    /// Seal of the Crusader: what its aura changes.
    pub(crate) crusader: Option<Crusader>,
    /// Seal of Fury: its absorb shield.
    pub(crate) fury: Option<FuryShield>,
}

/// A Seal of Fury rank's absorb shield: its aura, the share of the Holy damage it absorbs and
/// Go `DamageAbsorptionAura.ShieldStrength`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FuryShield {
    pub(crate) aura: AuraRef,
    pub(crate) share: f64,
    pub(crate) strength: f64,
}

/// Improved Seal of Fury: the mana a spent shield returns.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ImprovedFury {
    pub(crate) mana: f64,
    pub(crate) per_level: f64,
    pub(crate) max_levels: f64,
    pub(crate) levels: f64,
    pub(crate) metrics: usize,
}

/// The class action tag of a Seal of Fury hit waiting its batch window, plus its slot.
pub(crate) const FURY_DEAL_TAG: u32 = 0x100;

/// What a Seal of the Crusader rank's aura changes while it holds: its attack power through
/// the stat aura combinations when the rotation names the rank, the melee speed, and the
/// damage done mod on the main hand auto.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Crusader {
    pub(crate) stat_bit: Option<u32>,
    pub(crate) melee_speed: f64,
    pub(crate) auto_mod: ModId,
}

/// Seal of Command's proc parameters, shared by every rank.
#[derive(Clone, Debug)]
pub(crate) struct CommandProc {
    pub(crate) weapon_percent: f64,
    pub(crate) coefficient: f64,
    pub(crate) target_bonus_coefficient: f64,
    pub(crate) chance: f64,
    pub(crate) label: String,
    pub(crate) icd: i64,
}

/// Every bound seal rank and the seal the paladin is under.
#[derive(Clone, Debug, Default)]
pub(crate) struct Seals {
    pub(crate) seals: Vec<Seal>,
    /// Go `Paladin.currentSeal`.
    pub(crate) current: Option<usize>,
    pub(crate) command: Option<CommandProc>,
    pub(crate) hand_multiplier: f64,
    pub(crate) swing_speed: f64,
    pub(crate) deal_delay: i64,
    /// With an item swap, the weapon values of each set, Main then Swap1: Seal of
    /// Righteousness's hand multiplier, swing speed and spell power shares, and Seal of
    /// Command's chance, which its proc manager rebuilds for the weapon in the hand.
    pub(crate) by_set: Option<[SealWeapon; 2]>,
    /// Seal of Fury's shield needs one equipped.
    pub(crate) fury_can_block: bool,
    pub(crate) fury_improved: Option<ImprovedFury>,
    /// Seal of Fury hits waiting their batch window: the spell, the result and the seal.
    pub(crate) fury_pending: Vec<Option<(SpellId, SpellResult, usize)>>,
    /// Seal of Light's proc manager and its heal's multipliers.
    pub(crate) light: Option<LightProc>,
    /// Seal of Wisdom's ranks, by seal, with their restore's mana metrics.
    pub(crate) wisdom: Vec<(usize, usize)>,
}

/// Seal of Light's 15 PPM manager, each spell's chance, and the heal's multipliers.
#[derive(Clone, Debug)]
pub(crate) struct LightProc {
    pub(crate) chances: Vec<Option<f64>>,
    pub(crate) healing: Healing,
}

/// The seals' values that follow the main hand weapon.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SealWeapon {
    pub(crate) hand_multiplier: f64,
    pub(crate) swing_speed: f64,
    /// Each Seal of Righteousness rank's spell power share, when the swap moves it.
    pub(crate) hit_coefficients: Vec<f64>,
    pub(crate) command_chance: f64,
}

impl SealWeapon {
    fn of(effects: &[Effect]) -> SealWeapon {
        let mut weapon = SealWeapon::default();
        for effect in effects {
            match effect {
                Effect::SealOfRighteousness {
                    ranks,
                    hand_multiplier,
                    swing_speed,
                    ..
                } => {
                    weapon.hand_multiplier = *hand_multiplier;
                    weapon.swing_speed = *swing_speed;
                    weapon.hit_coefficients = ranks
                        .iter()
                        .filter_map(|rank| rank.hit_coefficient)
                        .collect();
                }
                Effect::SealOfCommand { proc_chance, .. } => weapon.command_chance = *proc_chance,
                _ => {}
            }
        }
        weapon
    }
}

/// Go seal_of_righteousness.go and seal_of_command.go read the weapon in the hand: once a swap
/// put a set's weapon in it, the seals take that set's values. Seal of Righteousness's hit takes
/// the hand's spell power share too: the seal's aura holds only the per-hit value, and each hit
/// is computed from the weapon in the hand as it lands.
pub(crate) fn on_item_swap(fight: &mut Fight<PaladinAgent>, worn: bool) {
    let Some(by_set) = fight.agent.seals.by_set.as_ref() else {
        return;
    };
    let weapon = by_set[usize::from(worn)].clone();
    let seals = &mut fight.agent.seals;
    seals.hand_multiplier = weapon.hand_multiplier;
    seals.swing_speed = weapon.swing_speed;
    if let Some(command) = seals.command.as_mut() {
        command.chance = weapon.command_chance;
    }
    let procs: Vec<SpellId> = seals
        .seals
        .iter()
        .filter(|seal| seal.kind == SealKind::Righteousness)
        .map(|seal| seal.proc_spell)
        .collect();
    for (spell, coefficient) in procs.into_iter().zip(weapon.hit_coefficients) {
        fight.spells[spell].bonus_coefficient = coefficient;
    }
}

/// The position of a seal rank among every bound seal, in the order [`bind`] builds them:
/// Seal of Command's ranks, then Seal of Righteousness's, Seal of the Crusader's, Seal of
/// Fury's, Seal of Light's, Seal of Wisdom's and Seal of Justice's.
pub(crate) fn seal_index(effects: &[Effect], seal_spell_id: i32) -> Option<usize> {
    let mut index = 0;
    for effect in effects {
        if let Effect::SealOfCommand { ranks, .. } = effect {
            for rank in ranks {
                if rank.seal_spell_id == seal_spell_id {
                    return Some(index);
                }
                index += 1;
            }
        }
    }
    for effect in effects {
        if let Effect::SealOfRighteousness { ranks, .. } = effect {
            for rank in ranks {
                if rank.seal_spell_id == seal_spell_id {
                    return Some(index);
                }
                index += 1;
            }
        }
    }
    for effect in effects {
        if let Effect::SealOfTheCrusader { ranks, .. } = effect {
            for rank in ranks {
                if rank.seal_spell_id == seal_spell_id {
                    return Some(index);
                }
                index += 1;
            }
        }
    }
    for effect in effects {
        if let Effect::SealOfFury { ranks, .. } = effect {
            for rank in ranks {
                if rank.seal_spell_id == seal_spell_id {
                    return Some(index);
                }
                index += 1;
            }
        }
    }
    for effect in effects {
        if let Effect::SealOfLight { ranks, .. } = effect {
            for rank in ranks {
                if rank.seal_spell_id == seal_spell_id {
                    return Some(index);
                }
                index += 1;
            }
        }
    }
    for effect in effects {
        if let Effect::SealOfWisdom { ranks } = effect {
            for rank in ranks {
                if rank.seal_spell_id == seal_spell_id {
                    return Some(index);
                }
                index += 1;
            }
        }
    }
    for effect in effects {
        if let Effect::SealOfJustice {
            seal_spell_id: justice,
            ..
        } = effect
        {
            if *justice == seal_spell_id {
                return Some(index);
            }
            index += 1;
        }
    }
    None
}

/// Resolve every seal rank's spells, auras and cooldowns.
pub(crate) fn bind(fight: &mut Fight<PaladinAgent>, effects: &[Effect]) -> Result<Seals, String> {
    let find = |fight: &Fight<PaladinAgent>, id: i32, occurrence: usize| {
        fight
            .spells
            .iter()
            .enumerate()
            .filter(|(_, spell)| spell.id.spell_id == id && spell.id.tag == 0)
            .nth(occurrence)
            .map(|(index, _)| index)
            .ok_or_else(|| format!("seal spell {id} is not registered"))
    };
    let mut seals = Seals::default();
    if let Some(worn) = fight.item_swap_worn_effects() {
        seals.by_set = Some([SealWeapon::of(effects), SealWeapon::of(&worn)]);
    }
    for effect in effects {
        if let Effect::SealOfCommand {
            ranks,
            proc_spell_id,
            weapon_percent,
            coefficient,
            target_bonus_coefficient,
            proc_chance,
            rng_label,
            icd_ns,
            deal_delay_ns,
        } = effect
        {
            for (position, rank) in ranks.iter().enumerate() {
                // Each rank registers its own proc spell and cooldown, in rank order.
                let icd_timer = fight.timers.len();
                fight.timers.push(STARTING_CD_TIME);
                seals.seals.push(Seal {
                    kind: SealKind::Command,
                    spell: find(fight, rank.seal_spell_id, 0)?,
                    aura: fight.player_aura(&rank.aura)?,
                    judgement: find(fight, rank.judgement_spell_id, 0)?,
                    proc_spell: find(fight, *proc_spell_id, position)?,
                    per_hit_value: 0.0,
                    icd_timer,
                    crusader: None,
                    fury: None,
                });
            }
            seals.command = Some(CommandProc {
                weapon_percent: *weapon_percent,
                coefficient: *coefficient,
                target_bonus_coefficient: *target_bonus_coefficient,
                chance: *proc_chance,
                label: rng_label.clone(),
                icd: *icd_ns,
            });
            seals.deal_delay = *deal_delay_ns;
        }
    }
    for effect in effects {
        if let Effect::SealOfRighteousness {
            ranks,
            hand_multiplier,
            swing_speed,
            deal_delay_ns,
        } = effect
        {
            for rank in ranks {
                seals.seals.push(Seal {
                    kind: SealKind::Righteousness,
                    spell: find(fight, rank.seal_spell_id, 0)?,
                    aura: fight.player_aura(&rank.aura)?,
                    judgement: find(fight, rank.judgement_spell_id, 0)?,
                    proc_spell: find(fight, rank.proc_spell_id, 0)?,
                    per_hit_value: rank.per_hit_value,
                    icd_timer: 0,
                    crusader: None,
                    fury: None,
                });
            }
            seals.hand_multiplier = *hand_multiplier;
            seals.swing_speed = *swing_speed;
            seals.deal_delay = *deal_delay_ns;
        }
    }
    for effect in effects {
        if let Effect::SealOfTheCrusader { ranks, auto_spells } = effect {
            for rank in ranks {
                let aura = fight.player_aura(&rank.aura)?;
                let stat_bit = Fight::<PaladinAgent>::stat_aura_bit(effects, &rank.aura);
                let auto_mod = fight.register_mod(
                    ModKind::DamageDonePercent,
                    rank.auto_damage_percent,
                    0,
                    auto_spells.clone(),
                );
                let judgement = find(fight, rank.judgement_spell_id, 0)?;
                seals.seals.push(Seal {
                    kind: SealKind::Crusader,
                    spell: find(fight, rank.seal_spell_id, 0)?,
                    aura,
                    judgement,
                    // The seal fires nothing; its judgement stands in.
                    proc_spell: judgement,
                    per_hit_value: 0.0,
                    icd_timer: 0,
                    crusader: Some(Crusader {
                        stat_bit,
                        melee_speed: rank.melee_speed,
                        auto_mod,
                    }),
                    fury: None,
                });
            }
        }
    }
    for effect in effects {
        if let Effect::SealOfFury {
            ranks,
            can_block,
            deal_delay_ns,
            improved,
        } = effect
        {
            for rank in ranks {
                seals.seals.push(Seal {
                    kind: SealKind::Fury,
                    spell: find(fight, rank.seal_spell_id, 0)?,
                    aura: fight.player_aura(&rank.aura)?,
                    judgement: find(fight, rank.judgement_spell_id, 0)?,
                    proc_spell: find(fight, rank.proc_spell_id, 0)?,
                    per_hit_value: rank.proc_damage,
                    icd_timer: 0,
                    crusader: None,
                    fury: Some(FuryShield {
                        aura: fight.player_aura(&rank.shield_aura)?,
                        share: rank.shield_share,
                        strength: 0.0,
                    }),
                });
            }
            seals.fury_can_block = *can_block;
            seals.deal_delay = *deal_delay_ns;
            if let Some(improved) = improved {
                seals.fury_improved = Some(ImprovedFury {
                    mana: improved.mana,
                    per_level: improved.per_level,
                    max_levels: improved.max_levels,
                    levels: improved.levels,
                    metrics: {
                        let metrics = fight.new_mana_metrics(improved.metrics_action_id.clone());
                        fight.resources[metrics].no_threat = improved.no_threat;
                        metrics
                    },
                });
            }
        }
    }
    for effect in effects {
        if let Effect::SealOfLight {
            ranks,
            chances,
            healing_dealt_multiplier,
            healing_taken_multiplier,
            table_healing_dealt_multiplier,
        } = effect
        {
            for rank in ranks {
                seals.seals.push(Seal {
                    kind: SealKind::Light,
                    spell: find(fight, rank.seal_spell_id, 0)?,
                    aura: fight.player_aura(&rank.aura)?,
                    judgement: find(fight, rank.judgement_spell_id, 0)?,
                    proc_spell: find(fight, rank.proc_spell_id, 0)?,
                    per_hit_value: rank.heal,
                    icd_timer: 0,
                    crusader: None,
                    fury: None,
                });
            }
            let mut by_spell = vec![None; fight.spells.len()];
            for chance in chances {
                if let Some(slot) = by_spell.get_mut(chance.spell) {
                    *slot = Some(chance.chance);
                }
            }
            seals.light = Some(LightProc {
                chances: by_spell,
                healing: Healing {
                    dealt_multiplier: *healing_dealt_multiplier,
                    taken_multiplier: *healing_taken_multiplier,
                    table_multiplier: *table_healing_dealt_multiplier,
                    healing_power: 0.0,
                },
            });
        }
    }
    for effect in effects {
        if let Effect::SealOfWisdom { ranks } = effect {
            for rank in ranks {
                let metrics = fight.new_mana_metrics(ActionId {
                    spell_id: rank.proc_spell_id,
                    ..ActionId::default()
                });
                seals.wisdom.push((seals.seals.len(), metrics));
                seals.seals.push(Seal {
                    kind: SealKind::Wisdom,
                    spell: find(fight, rank.seal_spell_id, 0)?,
                    aura: fight.player_aura(&rank.aura)?,
                    judgement: find(fight, rank.judgement_spell_id, 0)?,
                    proc_spell: find(fight, rank.proc_spell_id, 0)?,
                    per_hit_value: rank.mana,
                    icd_timer: 0,
                    crusader: None,
                    fury: None,
                });
            }
        }
    }
    for effect in effects {
        if let Effect::SealOfJustice {
            seal_spell_id,
            aura,
            judgement_spell_id,
            ..
        } = effect
        {
            let judgement = find(fight, *judgement_spell_id, 0)?;
            seals.seals.push(Seal {
                kind: SealKind::Justice,
                spell: find(fight, *seal_spell_id, 0)?,
                aura: fight.player_aura(aura)?,
                judgement,
                // The seal fires nothing; its judgement stands in.
                proc_spell: judgement,
                per_hit_value: 0.0,
                icd_timer: 0,
                crusader: None,
                fury: None,
            });
        }
    }
    Ok(seals)
}

/// Go `Paladin.activeSeal`: the current seal while its aura is up.
pub(crate) fn active_seal(fight: &Fight<PaladinAgent>) -> Option<usize> {
    let current = fight.agent.seals.current?;
    fight
        .aura(fight.agent.seals.seals[current].aura)
        .active
        .then_some(current)
}

/// Go `Paladin.applySeal`: the replaced seal leaves its Echo, then the seal category drops
/// whatever seal was up as the new one activates; recasting the same seal refreshes it.
pub(crate) fn apply(fight: &mut Fight<PaladinAgent>, seal: usize) {
    if let Some(previous) = fight.agent.seals.current {
        let previous_aura = fight.agent.seals.seals[previous].aura;
        if previous != seal && fight.aura(previous_aura).active {
            let kind = fight.agent.seals.seals[previous].kind;
            super::super::talents::twist_of_light::leave_echo(fight, kind, previous);
        }
    }
    fight.agent.seals.current = Some(seal);
    let aura = fight.agent.seals.seals[seal].aura;
    // Go ExclusiveEffect.Activate: an equal bid from a fresh seal takes the category.
    if !fight.aura(aura).active {
        for other in 0..fight.agent.seals.seals.len() {
            let other_aura = fight.agent.seals.seals[other].aura;
            if other_aura != aura && fight.aura(other_aura).active {
                fight.deactivate_aura(other_aura);
            }
        }
    }
    fight.activate_aura(aura);
}

/// Seal of the Crusader's gain and expiry, in Go's attachment order: the attack power, the
/// melee speed, then the auto damage mod.
pub(crate) fn crusader_toggle(fight: &mut Fight<PaladinAgent>, seal: usize, active: bool) {
    let Some(crusader) = fight.agent.seals.seals[seal].crusader else {
        return;
    };
    if let Some(bit) = crusader.stat_bit {
        fight.set_stat_aura(bit, active);
    }
    if active {
        fight.multiply_melee_speed(crusader.melee_speed);
        fight.activate_mod(crusader.auto_mod);
    } else {
        fight.multiply_melee_speed(1.0 / crusader.melee_speed);
        fight.deactivate_mod(crusader.auto_mod);
    }
}

/// The seal aura's proc trigger: landed white hits that are not procs, handled a batch
/// window later. Seal of the Crusader has none; Seal of Light hears every melee hit that is no
/// proc and rolls its manager first.
pub(crate) fn on_spell_hit_dealt(
    fight: &mut Fight<PaladinAgent>,
    aura: AuraRef,
    spell: SpellId,
    result: &SpellResult,
) {
    let kind = fight
        .agent
        .seals
        .seals
        .iter()
        .find(|seal| seal.aura == aura)
        .map(|seal| seal.kind);
    match kind {
        Some(SealKind::Crusader | SealKind::Justice) => return,
        Some(SealKind::Light) => return light_trigger(fight, aura, spell, result),
        Some(SealKind::Wisdom) => {
            // Go's trigger hears every landed melee hit that is no proc.
            let state = &fight.spells[spell];
            if !state.flags.proc && state.melee_proc && result.outcome & OUTCOME_LANDED != 0 {
                fight.schedule_delayed_proc(aura, spell, *result);
            }
            return;
        }
        _ => {}
    }
    let state = &fight.spells[spell];
    if state.flags.proc || !state.white_hit || result.outcome & OUTCOME_LANDED == 0 {
        return;
    }
    fight.schedule_delayed_proc(aura, spell, *result);
}

/// The delayed handler of a seal's trigger: Seal of Command rolls its proc, Seal of
/// Righteousness always fires.
pub(crate) fn on_delayed_proc(fight: &mut Fight<PaladinAgent>, seal: usize, target: Side) {
    match fight.agent.seals.seals[seal].kind {
        SealKind::Command => try_command(fight, seal, target),
        SealKind::Righteousness | SealKind::Fury => {
            let spell = fight.agent.seals.seals[seal].proc_spell;
            fight.cast(spell, target);
        }
        // Go `procSpell.Cast(sim, &paladin.Unit)`.
        SealKind::Light | SealKind::Wisdom => {
            let spell = fight.agent.seals.seals[seal].proc_spell;
            fight.cast(spell, Side::Player);
        }
        SealKind::Crusader | SealKind::Justice => {
            unreachable!("Seal of the Crusader and Seal of Justice have no proc")
        }
    }
}

/// Seal of Light's trigger, Go `AttachProcTriggerCallback` with the manager: a landed hit of
/// a spell the manager hears (melee, no proc), its chance rolled under the seal's label, then
/// the handler a batch window later.
fn light_trigger(
    fight: &mut Fight<PaladinAgent>,
    aura: AuraRef,
    spell: SpellId,
    result: &SpellResult,
) {
    if result.outcome & OUTCOME_LANDED == 0 {
        return;
    }
    let Some(chance) = fight
        .agent
        .seals
        .light
        .as_ref()
        .and_then(|light| light.chances.get(spell).copied().flatten())
    else {
        return;
    };
    let label = fight.aura(aura).label.clone();
    if !fight.proc(chance, &label) {
        return;
    }
    fight.schedule_delayed_proc(aura, spell, *result);
}

/// Seal of Light's heal, Go `CalcAndDealHealing(heal, OutcomeAlwaysHit)` on the paladin.
pub(crate) fn light_proc(fight: &mut Fight<PaladinAgent>, spell: SpellId, seal: usize) {
    let heal = fight.agent.seals.seals[seal].per_hit_value;
    let mut healing = fight
        .agent
        .seals
        .light
        .as_ref()
        .expect("Seal of Light is bound")
        .healing;
    healing.healing_power = fight.player.powers.healing_power;
    fight.calc_and_deal_self_healing(spell, heal, healing);
}

/// Seal of Wisdom's restore, Go `AddMana(mana, manaMetrics)` on the paladin.
pub(crate) fn wisdom_proc(fight: &mut Fight<PaladinAgent>, seal: usize) {
    let (_, metrics) = *fight
        .agent
        .seals
        .wisdom
        .iter()
        .find(|(rank, _)| *rank == seal)
        .expect("Seal of Wisdom is bound");
    let mana = fight.agent.seals.seals[seal].per_hit_value;
    fight.add_mana(mana, metrics);
}

/// Seal of Command's `tryProc`: the rank's cooldown, then the proc manager's roll.
pub(crate) fn try_command(fight: &mut Fight<PaladinAgent>, seal: usize, target: Side) {
    let command = fight
        .agent
        .seals
        .command
        .clone()
        .expect("Seal of Command is bound");
    let timer = fight.agent.seals.seals[seal].icd_timer;
    if fight.timers[timer] > fight.now || !fight.proc(command.chance, &command.label) {
        return;
    }
    fight.timers[timer] = fight.now + command.icd;
    let spell = fight.agent.seals.seals[seal].proc_spell;
    fight.cast(spell, target);
}

/// Go `dealAfterBatch`: the computed hit lands a batch window later, at low priority.
fn deal_after_batch(fight: &mut Fight<PaladinAgent>, spell: SpellId, result: SpellResult) {
    let at = fight.now + fight.agent.seals.deal_delay;
    fight.schedule(at, PRIORITY_LOW, Action::DelayedDamage { spell, result });
}

/// Seal of Command's proc: a share of weapon damage and spell power, plus the target's extra
/// Holy damage taken at the full coefficient, on the melee table.
pub(crate) fn command_proc(fight: &mut Fight<PaladinAgent>, spell: SpellId, target: Side) {
    let command = fight
        .agent
        .seals
        .command
        .clone()
        .expect("Seal of Command is bound");
    let attack_power = fight.melee_attack_power();
    let weapon = fight.mh_weapon_damage(attack_power);
    // Go's arm64 build fuses each spell power share into its add.
    let mut base = command
        .coefficient
        .mul_add(fight.spell_power(spell), weapon)
        * command.weapon_percent;
    let holy = crate::core::fight::school_index(2);
    let target_bonus = fight.target_unit(target).state.school_bonus_spell_damage[holy];
    base = command.target_bonus_coefficient.mul_add(target_bonus, base);
    let result = fight.calc_damage_with(
        spell,
        target,
        base,
        Outcome::Table(PhysicalOutcome::MeleeSpecialHitAndCrit { count: true }),
    );
    deal_after_batch(fight, spell, result);
}

/// Seal of Righteousness's proc: the per-hit value scaled by the weapon's speed and hand.
pub(crate) fn righteousness_proc(
    fight: &mut Fight<PaladinAgent>,
    spell: SpellId,
    target: Side,
    seal: usize,
) {
    let seals = &fight.agent.seals;
    let value = seals.seals[seal].per_hit_value;
    let base = value / 100.0 * seals.hand_multiplier * seals.swing_speed;
    let result = fight.calc_damage_with(
        spell,
        target,
        base,
        Outcome::Table(PhysicalOutcome::MeleeSpecialCritOnly { count: true }),
    );
    deal_after_batch(fight, spell, result);
}

/// Seal of Fury's proc: the rank's flat Holy damage on the special table, crits only, dealt a
/// batch window later.
pub(crate) fn fury_proc(
    fight: &mut Fight<PaladinAgent>,
    spell: SpellId,
    target: Side,
    seal: usize,
) {
    let base = fight.agent.seals.seals[seal].per_hit_value;
    let result = fight.calc_damage_with(
        spell,
        target,
        base,
        Outcome::Table(PhysicalOutcome::MeleeSpecialCritOnly { count: true }),
    );
    let pending = &mut fight.agent.seals.fury_pending;
    let slot = match pending.iter().position(Option::is_none) {
        Some(slot) => slot,
        None => {
            pending.push(None);
            pending.len() - 1
        }
    };
    pending[slot] = Some((spell, result, seal));
    let at = fight.now + fight.agent.seals.deal_delay;
    fight.schedule_class_action(at, PRIORITY_LOW, FURY_DEAL_TAG + slot as u32);
}

/// The delayed hit: its damage, then with a shield equipped and damage dealt the rank's
/// absorb shield of its share, which replaces the strength of one already up.
pub(crate) fn fury_deal(fight: &mut Fight<PaladinAgent>, slot: usize) {
    let (spell, result, seal) = fight.agent.seals.fury_pending[slot]
        .take()
        .expect("a pending Seal of Fury hit");
    fight.deal_damage(spell, result, false);
    if !fight.agent.seals.fury_can_block || result.damage <= 0.0 {
        return;
    }
    let mut shield = fight.agent.seals.seals[seal]
        .fury
        .expect("a Seal of Fury rank");
    // Go DamageAbsorptionAura.Activate: the aura, then the fresh strength and its stacks.
    fight.activate_aura(shield.aura);
    shield.strength = result.damage * shield.share;
    let stacks = (shield.strength as i32).max(1);
    fight.aura_mut(shield.aura).max_stacks = stacks;
    fight.agent.seals.seals[seal].fury = Some(shield);
    fight.set_stacks(shield.aura, stacks);
}

/// Seal of Fury's shields as damage taken modifiers, in rank order: each that holds absorbs
/// what it can of a hit that deals damage, Improved Seal of Fury returns mana when one is
/// spent, and a spent shield fades.
pub(crate) fn fury_absorb(fight: &mut Fight<PaladinAgent>, result: &mut SpellResult) {
    for seal in 0..fight.agent.seals.seals.len() {
        let Some(mut shield) = fight.agent.seals.seals[seal].fury else {
            continue;
        };
        if !fight.aura(shield.aura).active || result.damage <= 0.0 {
            continue;
        }
        let absorbed = shield.strength.min(result.damage);
        result.damage -= absorbed;
        shield.strength -= absorbed;
        fight.agent.seals.seals[seal].fury = Some(shield);
        if fight.log.is_some() {
            let line = format!(
                "{} absorbed {absorbed:.1} damage, new shield strength: {:.1}",
                fight.aura(shield.aura).label,
                shield.strength
            );
            fight.player_log(&line);
        }
        if let Some(improved) = fight.agent.seals.fury_improved {
            if shield.strength <= 0.0 {
                let levels = improved.levels.max(0.0).min(improved.max_levels);
                // Go's arm64 build fuses the level share into its add.
                let mana = improved.mana * improved.per_level.mul_add(levels, 1.0);
                fight.add_mana(mana, improved.metrics);
            }
        }
        if shield.strength <= 0.0 {
            fight.deactivate_aura(shield.aura);
            continue;
        }
        fight.set_stacks(shield.aura, shield.strength as i32);
    }
}
