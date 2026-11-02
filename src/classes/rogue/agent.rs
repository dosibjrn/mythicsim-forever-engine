//! The Rogue class agent for the fight runtime: class spells and auras by name, and the hooks
//! that dispatch to each spell's and talent's module.
//!
//! Go's `BreakStealth` opens every Rogue strike and cooldown but Slice and Dice, Cold Blood,
//! Premeditation and Preparation.

use std::rc::Rc;

use crate::{
    contracts::prepared_v2::{Effect, PreparedV2, Spell as ExportedSpell},
    core::fight::{Agent, AuraRef, DotId, Fight, Side, SpellId, SpellResult},
};

use super::{
    spells::{
        ambush::Ambush,
        backstab::Backstab,
        eviscerate::Eviscerate,
        expose_armor::ExposeArmor,
        finisher::{self, Finisher},
        kidney_shot::KidneyShot,
        mutilate::Mutilate,
        poisons::{self, PoisonProc},
        rupture::{self, Rupture},
        sinister_strike,
        slice_and_dice::SliceAndDice,
        stealth::Stealth,
        subtlety_strikes::{self, Garrote},
    },
    talents::{
        adrenaline_rush::AdrenalineRush,
        blade_flurry::BladeFlurry,
        cold_blood::{self, ColdBlood},
        procs::{self, Handler, Proc},
        quietus::{self, Quietus},
        riposte::Riposte,
        subtlety::{self, Premeditation, Preparation},
        thousand_cuts::{self, ThousandCuts},
        venom::{self, Venom},
    },
};

/// What a Rogue spell does when its effects apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RogueSpell {
    SinisterStrike,
    /// Blade Flurry's extra hit on the next target.
    BladeFlurryHit,
    Backstab,
    Eviscerate,
    SliceAndDice,
    BladeFlurry,
    AdrenalineRush,
    InstantPoison,
    DeadlyPoison,
    Ambush,
    Rupture,
    Mutilate,
    MutilateHit,
    ColdBlood,
    Premeditation,
    Preparation,
    Stealth,
    Vanish,
    WoundPoison,
    Venom,
    GhostlyStrike,
    Hemorrhage,
    Garrote,
    KidneyShot,
    ExposeArmor,
    Riposte,
}

/// Class auras with Rust behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RogueAura {
    InstantPoisonTrigger,
    DeadlyPoisonTrigger,
    SliceAndDice,
    BladeFlurry,
    AdrenalineRush,
    ColdBlood,
    ThousandCuts,
    WoundPoisonTrigger,
    Venom,
    Quietus,
    KidneyShotStun,
    ExposeArmor,
    RiposteTrigger,
    /// A talent proc trigger, by its index in the agent's triggers.
    Proc(usize),
}

/// Rogue state that Go keeps in the `Rogue` struct and its closures.
#[derive(Default)]
pub(crate) struct RogueAgent {
    sinister_strike: f64,
    backstab: Option<Backstab>,
    eviscerate: Option<Eviscerate>,
    slice_and_dice: Option<SliceAndDice>,
    blade_flurry: Option<BladeFlurry>,
    /// The damage Blade Flurry's extra hit deals, Go's `curDmg`.
    blade_flurry_damage: f64,
    adrenaline_rush: Option<AdrenalineRush>,
    finisher: Option<Finisher>,
    instant_poison: Option<Rc<PoisonProc>>,
    /// Instant Poison's least and most damage, and its share of attack power.
    instant_poison_damage: (f64, f64, f64),
    deadly_poison: Option<Rc<PoisonProc>>,
    /// Deadly Poison's tick a stack, and its share of attack power a stack.
    deadly_poison_tick: (f64, f64),
    stealth: Option<Stealth>,
    ambush: Option<Ambush>,
    rupture: Option<Rc<Rupture>>,
    /// What each target's Rupture stored, by target position.
    rupture_snapshot: [rupture::Snapshot; crate::core::fight::MAX_TARGETS],
    mutilate: Option<Mutilate>,
    cold_blood: Option<Rc<ColdBlood>>,
    premeditation: Option<Premeditation>,
    preparation: Option<Rc<Preparation>>,
    thousand_cuts: Option<Rc<ThousandCuts>>,
    procs: Vec<Rc<Proc>>,
    wound_poison: Option<Rc<PoisonProc>>,
    wound_debuff: Option<AuraRef>,
    venom: Option<Rc<Venom>>,
    /// Go `additivePoisonBonusChance`, which Venom raises while it is up.
    poison_bonus: f64,
    ghostly_dodge: Option<AuraRef>,
    hemorrhage_debuff: Option<AuraRef>,
    garrote: Option<Garrote>,
    /// What each target's Garrote stored, by target position.
    garrote_snapshot: [rupture::Snapshot; crate::core::fight::MAX_TARGETS],
    quietus: Option<Quietus>,
    kidney_shot: Option<KidneyShot>,
    expose_armor: Option<ExposeArmor>,
    /// The Kidney Shot stun's pause of the target's swing: the swing time before it and when
    /// it ends, kept in Go's aura closure.
    kidney_shot_pause: (i64, i64),
    riposte: Option<Riposte>,
}

/// Whether a prepared input carries the effect of a kind.
fn has_effect(prepared: &PreparedV2, kind: &str) -> bool {
    prepared.effects.iter().any(|effect| effect.kind() == kind)
}

/// Whether a weapon proc on the named hands hears a spell: no Suppress Weapon Procs flag, and
/// a proc mask that names one of the hands.
fn weapon_proc_hears(spell: &ExportedSpell, hands: &[String]) -> bool {
    !spell.has_flag("SpellFlagSuppressWeaponProcs")
        && spell.proc_mask.iter().any(|mask| hands.contains(mask))
}

impl RogueAgent {
    /// The class behavior of an exported spell, if Rust implements it.
    fn spell(prepared: &PreparedV2, spell: &ExportedSpell) -> Option<RogueSpell> {
        let has = |kind| has_effect(prepared, kind);
        let id = spell.action_id.clone().unwrap_or_default();
        let flurry_hit = prepared.effects.iter().any(|effect| {
            matches!(effect, Effect::BladeFlurry { hit_spell_id, .. }
                if id.spell_id == *hit_spell_id && id.tag == 0 && id.item_id == 0)
        });
        if flurry_hit {
            return Some(RogueSpell::BladeFlurryHit);
        }
        let energy = spell
            .cost
            .as_ref()
            .is_some_and(|cost| cost.resource == "energy");
        match spell.class_spell.as_deref()? {
            "sinister_strike" if energy && has("sinister_strike") => {
                Some(RogueSpell::SinisterStrike)
            }
            "backstab" if energy && has("backstab") => Some(RogueSpell::Backstab),
            "eviscerate" if energy && has("eviscerate") => Some(RogueSpell::Eviscerate),
            "slice_and_dice" if energy && has("slice_and_dice") => Some(RogueSpell::SliceAndDice),
            "blade_flurry" if has("blade_flurry") => Some(RogueSpell::BladeFlurry),
            "adrenaline_rush" if has("adrenaline_rush") => Some(RogueSpell::AdrenalineRush),
            "instant_poison" if has("instant_poison") => Some(RogueSpell::InstantPoison),
            "deadly_poison" if spell.dot.is_some() && has("deadly_poison") => {
                Some(RogueSpell::DeadlyPoison)
            }
            "ambush" if energy && has("ambush") => Some(RogueSpell::Ambush),
            "rupture" if energy && spell.dot.is_some() && has("rupture") => {
                Some(RogueSpell::Rupture)
            }
            "mutilate" if energy && has("mutilate") => Some(RogueSpell::Mutilate),
            "mutilate_hit" if has("mutilate") => Some(RogueSpell::MutilateHit),
            "cold_blood" if has("cold_blood") => Some(RogueSpell::ColdBlood),
            "premeditation" if has("premeditation") => Some(RogueSpell::Premeditation),
            "preparation" if has("preparation") => Some(RogueSpell::Preparation),
            "stealth" if has("stealth") => Some(RogueSpell::Stealth),
            "vanish" if has("stealth") => Some(RogueSpell::Vanish),
            "wound_poison" if has("wound_poison") => Some(RogueSpell::WoundPoison),
            "venom" if energy && has("venom") => Some(RogueSpell::Venom),
            "ghostly_strike" if energy && has("ghostly_strike") => Some(RogueSpell::GhostlyStrike),
            "hemorrhage" if energy && has("hemorrhage") => Some(RogueSpell::Hemorrhage),
            "kidney_shot" if energy && has("kidney_shot") => Some(RogueSpell::KidneyShot),
            "expose_armor" if energy && has("expose_armor") => Some(RogueSpell::ExposeArmor),
            "riposte" if energy && has("riposte") => Some(RogueSpell::Riposte),
            "garrote" if energy && spell.dot.is_some() && has("garrote") => {
                Some(RogueSpell::Garrote)
            }
            _ => None,
        }
    }

    /// Build a fight for a prepared input that passed the coverage gate.
    pub(crate) fn fight(prepared: &PreparedV2) -> Result<Fight<RogueAgent>, String> {
        let auras: Vec<(String, RogueAura)> = prepared
            .effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::InstantPoison { trigger_aura, .. } => {
                    Some((trigger_aura.clone(), RogueAura::InstantPoisonTrigger))
                }
                Effect::DeadlyPoison { trigger_aura, .. } => {
                    Some((trigger_aura.clone(), RogueAura::DeadlyPoisonTrigger))
                }
                Effect::SliceAndDice { aura, .. } => Some((aura.clone(), RogueAura::SliceAndDice)),
                Effect::BladeFlurry { aura, .. } => Some((aura.clone(), RogueAura::BladeFlurry)),
                Effect::AdrenalineRush { aura, .. } => {
                    Some((aura.clone(), RogueAura::AdrenalineRush))
                }
                Effect::ColdBlood { aura, .. } => Some((aura.clone(), RogueAura::ColdBlood)),
                Effect::ThousandCuts { aura, .. } => Some((aura.clone(), RogueAura::ThousandCuts)),
                Effect::WoundPoison { trigger_aura, .. } => {
                    Some((trigger_aura.clone(), RogueAura::WoundPoisonTrigger))
                }
                Effect::Venom { aura, .. } => Some((aura.clone(), RogueAura::Venom)),
                Effect::Quietus { aura, .. } => Some((aura.clone(), RogueAura::Quietus)),
                Effect::KidneyShot { stun_aura, .. } if !stun_aura.is_empty() => {
                    Some((stun_aura.clone(), RogueAura::KidneyShotStun))
                }
                Effect::ExposeArmor { aura, .. } => Some((aura.clone(), RogueAura::ExposeArmor)),
                Effect::Riposte { trigger_aura, .. } => {
                    Some((trigger_aura.clone(), RogueAura::RiposteTrigger))
                }
                _ => None,
            })
            .chain(
                prepared
                    .effects
                    .iter()
                    .filter(|effect| matches!(effect, Effect::RogueProc { .. }))
                    .enumerate()
                    .filter_map(|(index, effect)| match effect {
                        Effect::RogueProc { trigger_aura, .. } => {
                            Some((trigger_aura.clone(), RogueAura::Proc(index)))
                        }
                        _ => None,
                    }),
            )
            .collect();
        let spell = |exported: &ExportedSpell| RogueAgent::spell(prepared, exported);
        let mut fight = Fight::new(prepared, RogueAgent::default(), spell, |unit, label| {
            auras
                .iter()
                .find(|(name, kind)| {
                    // Kidney Shot's stun and Expose Armor are the class auras on the target.
                    let side = if matches!(kind, RogueAura::KidneyShotStun | RogueAura::ExposeArmor)
                    {
                        "target"
                    } else {
                        "player"
                    };
                    name == label && unit == side
                })
                .map(|(_, kind)| *kind)
        })?;
        let class_spell = |fight: &Fight<RogueAgent>, kind: RogueSpell| {
            fight.spells.iter().position(|spell| {
                matches!(spell.behavior, crate::core::fight::SpellBehavior::Class(k) if k == kind)
            })
        };
        let poison = |fight: &Fight<RogueAgent>,
                      kind: RogueSpell,
                      label: &str,
                      hands: &[String],
                      chance: f64|
         -> Result<PoisonProc, String> {
            let spell = class_spell(fight, kind).ok_or_else(|| format!("{label} has no spell"))?;
            Ok(PoisonProc {
                label: label.to_string(),
                spell,
                chance,
                eligible: prepared
                    .player
                    .spells
                    .iter()
                    .map(|exported| weapon_proc_hears(exported, hands))
                    .collect(),
            })
        };
        for effect in &prepared.effects {
            match effect {
                Effect::SinisterStrike { base_damage, .. } => {
                    fight.agent.sinister_strike = *base_damage;
                }
                Effect::Backstab {
                    base_damage,
                    main_hand_dagger,
                    extra_combo_point_chance,
                    extra_combo_point_action,
                    ..
                } => {
                    let extra_combo_point_metrics = fight.new_resource_metrics(
                        extra_combo_point_action.clone(),
                        crate::core::fight::ResourceKind::ComboPoints,
                    );
                    fight.agent.backstab = Some(Backstab {
                        base_damage: *base_damage,
                        main_hand_dagger: *main_hand_dagger,
                        extra_combo_point_chance: *extra_combo_point_chance,
                        extra_combo_point_metrics,
                    });
                }
                Effect::Eviscerate {
                    damage_average,
                    damage_variance,
                    combo_point_damage,
                    attack_power_per_combo_point,
                    ..
                } => {
                    fight.agent.eviscerate = Some(Eviscerate {
                        damage_average: *damage_average,
                        damage_variance: *damage_variance,
                        combo_point_damage: *combo_point_damage,
                        attack_power_per_combo_point: *attack_power_per_combo_point,
                    });
                }
                Effect::SliceAndDice {
                    aura,
                    durations_ns,
                    melee_speed_multiplier,
                    ..
                } => {
                    fight.agent.slice_and_dice = Some(SliceAndDice {
                        aura: fight.player_aura(aura)?,
                        durations: durations_ns.clone(),
                        melee_speed_multiplier: *melee_speed_multiplier,
                    });
                }
                Effect::BladeFlurry {
                    aura,
                    attack_speed_multiplier,
                    hit_spell_id,
                    ..
                } => {
                    let hit = fight
                        .spells
                        .iter()
                        .position(|spell| spell.id.spell_id == *hit_spell_id && spell.id.tag == 0)
                        .ok_or_else(|| {
                            format!("Blade Flurry's hit {hit_spell_id} is not registered")
                        })?;
                    fight.agent.blade_flurry = Some(BladeFlurry {
                        aura: fight.player_aura(aura)?,
                        attack_speed_multiplier: *attack_speed_multiplier,
                        hit,
                    });
                }
                Effect::AdrenalineRush {
                    aura,
                    regen_multiplier,
                    energy_threshold,
                    ..
                } => {
                    fight.agent.adrenaline_rush = Some(AdrenalineRush {
                        aura: fight.player_aura(aura)?,
                        regen_multiplier: *regen_multiplier,
                        energy_threshold: *energy_threshold,
                    });
                }
                Effect::RogueFinisher {
                    relentless_strikes,
                    relentless_strikes_chance_per_point,
                    relentless_strikes_energy,
                    relentless_strikes_action,
                    ruthlessness_chance,
                    ruthlessness_action,
                } => {
                    let bound = finisher::bind(
                        &mut fight,
                        *relentless_strikes,
                        *relentless_strikes_chance_per_point,
                        *relentless_strikes_energy,
                        relentless_strikes_action,
                        *ruthlessness_chance,
                        ruthlessness_action,
                    );
                    fight.agent.finisher = Some(bound);
                }
                Effect::InstantPoison {
                    trigger_aura,
                    proc_mask,
                    proc_chance,
                    min_damage,
                    max_damage,
                    attack_power_share,
                    ..
                } => {
                    let bound = poison(
                        &fight,
                        RogueSpell::InstantPoison,
                        trigger_aura,
                        proc_mask,
                        *proc_chance,
                    )?;
                    fight.agent.instant_poison = Some(Rc::new(bound));
                    fight.agent.instant_poison_damage =
                        (*min_damage, *max_damage, *attack_power_share);
                }
                Effect::DeadlyPoison {
                    trigger_aura,
                    proc_mask,
                    proc_chance,
                    tick_damage,
                    attack_power_share,
                    ..
                } => {
                    let bound = poison(
                        &fight,
                        RogueSpell::DeadlyPoison,
                        trigger_aura,
                        proc_mask,
                        *proc_chance,
                    )?;
                    let dot = fight.spells[bound.spell]
                        .dot
                        .ok_or("Deadly Poison has no dot")?;
                    // The dot is 25349, which carries Periodic Can Crit in the client.
                    fight.dots[dot].tick_can_crit = true;
                    fight.agent.deadly_poison = Some(Rc::new(bound));
                    fight.agent.deadly_poison_tick = (*tick_damage, *attack_power_share);
                }
                Effect::Stealth { aura, .. } => {
                    fight.agent.stealth = Some(Stealth {
                        aura: fight.player_aura(aura)?,
                    });
                }
                Effect::Ambush {
                    base_damage,
                    main_hand_dagger,
                    cutthroat_aura,
                    ..
                } => {
                    let cutthroat = if cutthroat_aura.is_empty() {
                        None
                    } else {
                        Some(fight.player_aura(cutthroat_aura)?)
                    };
                    fight.agent.ambush = Some(Ambush {
                        base_damage: *base_damage,
                        main_hand_dagger: *main_hand_dagger,
                        cutthroat,
                    });
                }
                Effect::Rupture {
                    tick_damage,
                    damage_per_combo_point,
                    base_tick_count,
                    attack_power_shares,
                    tick_can_crit,
                    magic,
                    hemorrhage_aura,
                    hemorrhage_multiplier,
                    ..
                } => {
                    // spelldata TickOutcome for a dot whose hit was rolled on the special table.
                    let tick_outcome = match (tick_can_crit, magic) {
                        (true, true) => crate::core::fight::Outcome::TickMagicHitAndCrit,
                        (true, false) => crate::core::fight::Outcome::TickPhysicalCrit,
                        (false, true) => {
                            return Err("Rupture ticks that roll a magic hit are unsupported".into())
                        }
                        (false, false) => crate::core::fight::Outcome::Tick,
                    };
                    let hemorrhage = if hemorrhage_aura.is_empty() {
                        None
                    } else {
                        let index = fight.trackers[Side::Target.index()]
                            .find(hemorrhage_aura)
                            .ok_or_else(|| {
                                format!("target aura {hemorrhage_aura} is not registered")
                            })?;
                        Some(AuraRef {
                            side: Side::Target,
                            index,
                        })
                    };
                    fight.agent.rupture = Some(Rc::new(Rupture {
                        tick_damage: *tick_damage,
                        damage_per_combo_point: *damage_per_combo_point,
                        base_tick_count: *base_tick_count,
                        attack_power_shares: attack_power_shares.clone(),
                        tick_outcome,
                        hemorrhage,
                        hemorrhage_multiplier: *hemorrhage_multiplier,
                    }));
                }
                Effect::Mutilate {
                    flat_damage,
                    poison_bonus,
                    combo_points,
                    daggers,
                    ..
                } => {
                    let hand = |tag: i32| {
                        fight.spells.iter().position(|spell| {
                            spell.class_spell.as_deref() == Some("mutilate_hit")
                                && spell.id.tag == tag
                        })
                    };
                    let (main_hand, off_hand) = (
                        hand(1).ok_or("Mutilate has no main hand hit")?,
                        hand(2).ok_or("Mutilate has no off hand hit")?,
                    );
                    fight.agent.mutilate = Some(Mutilate {
                        flat_damage: *flat_damage,
                        poison_bonus: *poison_bonus,
                        combo_points: *combo_points,
                        daggers: *daggers,
                        main_hand,
                        off_hand,
                    });
                }
                Effect::ColdBlood {
                    aura,
                    crit_bonus,
                    class_spells,
                    spend_class_spells,
                    ..
                } => {
                    let bound = cold_blood::bind(
                        &mut fight,
                        aura,
                        *crit_bonus,
                        class_spells,
                        spend_class_spells,
                    )?;
                    fight.agent.cold_blood = Some(Rc::new(bound));
                }
                Effect::Premeditation { combo_points, .. } => {
                    let spell = class_spell(&fight, RogueSpell::Premeditation)
                        .ok_or("Premeditation has no spell")?;
                    fight.agent.premeditation = Some(subtlety::bind_premeditation(
                        &mut fight,
                        spell,
                        *combo_points,
                    ));
                }
                Effect::Preparation { reset_spells, .. } => {
                    if let Some(&spell) = reset_spells
                        .iter()
                        .find(|&&spell| spell >= fight.spells.len())
                    {
                        return Err(format!("Preparation resets unknown spell {spell}"));
                    }
                    let reset = reset_spells.clone();
                    let vanish = class_spell(&fight, RogueSpell::Vanish);
                    fight.agent.preparation = Some(Rc::new(Preparation { reset, vanish }));
                }
                Effect::ThousandCuts {
                    aura,
                    cost_per_stack,
                    class_spells,
                } => {
                    let bound =
                        thousand_cuts::bind(&mut fight, aura, *cost_per_stack, class_spells)?;
                    fight.agent.thousand_cuts = Some(Rc::new(bound));
                }
                Effect::WoundPoison {
                    trigger_aura,
                    proc_mask,
                    proc_chance,
                    debuff_aura,
                    ..
                } => {
                    let bound = poison(
                        &fight,
                        RogueSpell::WoundPoison,
                        trigger_aura,
                        proc_mask,
                        *proc_chance,
                    )?;
                    fight.agent.wound_poison = Some(Rc::new(bound));
                    fight.agent.wound_debuff = Some(Self::target_aura(&fight, debuff_aura)?);
                }
                Effect::Venom {
                    aura,
                    durations_ns,
                    damage_bonus,
                    chance_bonus,
                    class_spells,
                    ..
                } => {
                    let bound = venom::bind(
                        &mut fight,
                        aura,
                        durations_ns,
                        *damage_bonus,
                        *chance_bonus,
                        class_spells,
                    )?;
                    fight.agent.venom = Some(Rc::new(bound));
                }
                Effect::Quietus {
                    aura,
                    execute_phase,
                    damage_bonus,
                    class_spells,
                } => {
                    let bound = quietus::bind(
                        &mut fight,
                        aura,
                        *execute_phase,
                        *damage_bonus,
                        class_spells,
                    )?;
                    fight.agent.quietus = Some(bound);
                }
                Effect::KidneyShot {
                    target_stun_immune,
                    stun_aura,
                    base_duration_ns,
                    duration_per_combo_point_ns,
                    stun_duration_multiplier,
                    damage_taken_multiplier,
                    ..
                } => {
                    let stun = if *target_stun_immune || stun_aura.is_empty() {
                        None
                    } else {
                        Some(Self::target_aura(&fight, stun_aura)?)
                    };
                    fight.agent.kidney_shot = Some(KidneyShot {
                        stun,
                        base_duration: *base_duration_ns,
                        duration_per_combo_point: *duration_per_combo_point_ns,
                        stun_duration_multiplier: *stun_duration_multiplier,
                        damage_taken_multiplier: *damage_taken_multiplier,
                    });
                }
                Effect::ExposeArmor {
                    aura,
                    armor_per_combo_point,
                    points_back,
                    points_back_action,
                    blocking_priority,
                    ..
                } => {
                    let points_back_metrics = fight.new_resource_metrics(
                        points_back_action.clone(),
                        crate::core::fight::ResourceKind::ComboPoints,
                    );
                    fight.agent.expose_armor = Some(ExposeArmor {
                        aura: Self::target_aura(&fight, aura)?,
                        armor_per_combo_point: *armor_per_combo_point,
                        points_back: *points_back,
                        points_back_metrics,
                        blocking_priority: *blocking_priority,
                    });
                }
                Effect::Riposte { ready_aura, .. } => {
                    fight.agent.riposte = Some(Riposte {
                        ready: fight.player_aura(ready_aura)?,
                    });
                }
                Effect::GhostlyStrike { dodge_aura, .. } => {
                    fight.agent.ghostly_dodge = if dodge_aura.is_empty() {
                        None
                    } else {
                        Some(fight.player_aura(dodge_aura)?)
                    };
                }
                Effect::Hemorrhage { debuff_aura, .. } => {
                    fight.agent.hemorrhage_debuff = Some(Self::target_aura(&fight, debuff_aura)?);
                }
                Effect::Garrote {
                    tick_damage,
                    attack_power_share,
                    tick_can_crit,
                    magic,
                    dirty_deeds,
                    ..
                } => {
                    fight.agent.garrote = Some(Garrote {
                        tick_damage: *tick_damage,
                        attack_power_share: *attack_power_share,
                        tick_outcome: rupture::tick_outcome(*tick_can_crit, *magic)?,
                        dirty_deeds: *dirty_deeds,
                    });
                }
                Effect::RogueProc {
                    handler,
                    proc_chance,
                    spells,
                    outcome,
                    periodic,
                    delay_ns,
                    action,
                    aura,
                    ..
                } => {
                    let handler = match (handler.as_str(), action, aura) {
                        ("combo_point", Some(action), _) => {
                            Handler::ComboPoint(fight.new_resource_metrics(
                                action.clone(),
                                crate::core::fight::ResourceKind::ComboPoints,
                            ))
                        }
                        ("activate", _, Some(aura)) => Handler::Activate(fight.player_aura(aura)?),
                        ("stack", _, Some(aura)) => Handler::Stack(fight.player_aura(aura)?),
                        ("extra_attack", _, _) => Handler::ExtraAttack,
                        (other, _, _) => return Err(format!("unknown Rogue proc handler {other}")),
                    };
                    let mut heard = vec![false; fight.spells.len()];
                    for &spell in spells {
                        if let Some(slot) = heard.get_mut(spell) {
                            *slot = true;
                        }
                    }
                    fight.agent.procs.push(Rc::new(Proc {
                        handler,
                        chance: *proc_chance,
                        outcome: procs::outcome_mask(outcome)?,
                        periodic: *periodic,
                        delay: *delay_ns,
                        spells: heard,
                    }));
                }
                _ => {}
            }
        }
        Ok(fight)
    }

    /// Go `BreakStealth`.
    fn break_stealth(fight: &mut Fight<Self>) {
        if let Some(stealth) = fight.agent.stealth {
            stealth.break_stealth(fight);
        }
    }

    /// Go `isPoisoned`: the rogue's Deadly Poison or Wound Poison is on the target.
    fn poisoned(fight: &Fight<Self>, target: Side) -> bool {
        let deadly = fight
            .agent
            .deadly_poison
            .as_ref()
            .and_then(|poison| fight.spells[poison.spell].dot)
            .is_some_and(|dot| {
                let dot = fight.dot_on(dot, target);
                fight.aura(fight.dots[dot].aura).active
            });
        deadly
            || fight
                .agent
                .wound_debuff
                .is_some_and(|debuff| fight.aura(fight.aura_on(debuff, target)).active)
    }

    /// A target aura by label.
    fn target_aura(fight: &Fight<Self>, label: &str) -> Result<AuraRef, String> {
        let index = fight.trackers[Side::Target.index()]
            .find(label)
            .ok_or_else(|| format!("target aura {label} is not registered"))?;
        Ok(AuraRef {
            side: Side::Target,
            index,
        })
    }

    fn finisher(fight: &Fight<Self>) -> Finisher {
        fight.agent.finisher.expect("the finisher is bound")
    }
}

impl Agent for RogueAgent {
    type Spell = RogueSpell;
    type Aura = RogueAura;
    const CACHE_ROTATION: bool = true;
    const CACHE_OVER_ENERGY: bool = true;

    fn apply_effects(fight: &mut Fight<Self>, spell: SpellId, target: Side, behavior: RogueSpell) {
        if matches!(
            behavior,
            RogueSpell::SinisterStrike
                | RogueSpell::Backstab
                | RogueSpell::Eviscerate
                | RogueSpell::BladeFlurry
                | RogueSpell::AdrenalineRush
                | RogueSpell::Ambush
                | RogueSpell::Rupture
                | RogueSpell::Mutilate
                | RogueSpell::Venom
                | RogueSpell::GhostlyStrike
                | RogueSpell::Hemorrhage
                | RogueSpell::Garrote
                | RogueSpell::KidneyShot
                | RogueSpell::ExposeArmor
        ) {
            Self::break_stealth(fight);
        }
        match behavior {
            RogueSpell::SinisterStrike => {
                let base = fight.agent.sinister_strike;
                sinister_strike::apply(fight, spell, target, base);
            }
            RogueSpell::Backstab => {
                let backstab = fight.agent.backstab.expect("Backstab is bound");
                backstab.apply(fight, spell, target);
            }
            RogueSpell::Eviscerate => {
                let eviscerate = fight.agent.eviscerate.expect("Eviscerate is bound");
                let finisher = Self::finisher(fight);
                eviscerate.apply(fight, spell, target, &finisher);
            }
            RogueSpell::SliceAndDice => {
                let slice = fight
                    .agent
                    .slice_and_dice
                    .clone()
                    .expect("Slice and Dice is bound");
                let finisher = Self::finisher(fight);
                slice.apply(fight, spell, &finisher);
            }
            RogueSpell::BladeFlurry => {
                let flurry = fight.agent.blade_flurry.expect("Blade Flurry is bound");
                flurry.apply(fight);
            }
            RogueSpell::BladeFlurryHit => {
                let damage = fight.agent.blade_flurry_damage;
                BladeFlurry::apply_hit(fight, spell, target, damage);
            }
            RogueSpell::AdrenalineRush => {
                let rush = fight
                    .agent
                    .adrenaline_rush
                    .expect("Adrenaline Rush is bound");
                rush.apply(fight);
            }
            RogueSpell::InstantPoison => {
                let damage = fight.agent.instant_poison_damage;
                poisons::instant_poison(fight, spell, target, damage);
            }
            RogueSpell::DeadlyPoison => {
                let tick = fight.agent.deadly_poison_tick;
                poisons::deadly_poison(fight, spell, target, tick);
            }
            RogueSpell::Ambush => {
                let ambush = fight.agent.ambush.expect("Ambush is bound");
                ambush.apply(fight, spell, target);
            }
            RogueSpell::Rupture => {
                let rupture = fight.agent.rupture.clone().expect("Rupture is bound");
                let finisher = Self::finisher(fight);
                if let Some(snapshot) = rupture.apply(fight, spell, target, &finisher) {
                    let position = target.target_position().unwrap_or(0);
                    fight.agent.rupture_snapshot[position] = snapshot;
                }
            }
            RogueSpell::Mutilate => {
                let mutilate = fight.agent.mutilate.expect("Mutilate is bound");
                mutilate.apply(fight, spell, target);
            }
            RogueSpell::MutilateHit => {
                let mutilate = fight.agent.mutilate.expect("Mutilate is bound");
                let poisoned = Self::poisoned(fight, target);
                mutilate.hit(fight, spell, target, poisoned);
            }
            RogueSpell::WoundPoison => {
                let debuff = fight.agent.wound_debuff.expect("Wound Poison is bound");
                poisons::wound_poison(fight, spell, target, debuff);
            }
            RogueSpell::Venom => {
                let venom = fight.agent.venom.clone().expect("Venom is bound");
                let finisher = Self::finisher(fight);
                venom.apply(fight, spell, &finisher);
            }
            RogueSpell::GhostlyStrike => {
                let dodge = fight.agent.ghostly_dodge;
                subtlety_strikes::ghostly_strike(fight, spell, target, dodge);
            }
            RogueSpell::Hemorrhage => {
                let debuff = fight.agent.hemorrhage_debuff.expect("Hemorrhage is bound");
                subtlety_strikes::hemorrhage(fight, spell, target, debuff);
            }
            RogueSpell::Garrote => {
                let garrote = fight.agent.garrote.expect("Garrote is bound");
                if let Some(snapshot) = garrote.apply(fight, spell, target) {
                    let position = target.target_position().unwrap_or(0);
                    fight.agent.garrote_snapshot[position] = snapshot;
                }
            }
            RogueSpell::KidneyShot => {
                let kidney_shot = fight.agent.kidney_shot.expect("Kidney Shot is bound");
                let finisher = Self::finisher(fight);
                kidney_shot.apply(fight, spell, target, &finisher);
            }
            RogueSpell::Riposte => {
                let riposte = fight.agent.riposte.expect("Riposte is bound");
                riposte.apply(fight, spell, target);
            }
            RogueSpell::ExposeArmor => {
                let expose_armor = fight.agent.expose_armor.expect("Expose Armor is bound");
                let finisher = Self::finisher(fight);
                expose_armor.apply(fight, spell, target, &finisher);
            }
            RogueSpell::ColdBlood => {
                let cold_blood = fight.agent.cold_blood.clone().expect("Cold Blood is bound");
                cold_blood.apply(fight);
            }
            RogueSpell::Premeditation => {
                let premeditation = fight.agent.premeditation.expect("Premeditation is bound");
                premeditation.apply(fight);
            }
            RogueSpell::Preparation => {
                let preparation = fight
                    .agent
                    .preparation
                    .clone()
                    .expect("Preparation is bound");
                preparation.apply(fight);
            }
            RogueSpell::Stealth => fight.agent.stealth.expect("Stealth is bound").apply(fight),
            RogueSpell::Vanish => fight.agent.stealth.expect("Stealth is bound").vanish(fight),
        }
    }

    fn extra_cast_condition(fight: &Fight<Self>, _spell: SpellId, behavior: RogueSpell) -> bool {
        match behavior {
            RogueSpell::Backstab => fight
                .agent
                .backstab
                .is_some_and(|backstab| backstab.can_cast(fight)),
            RogueSpell::Eviscerate
            | RogueSpell::SliceAndDice
            | RogueSpell::Rupture
            | RogueSpell::Venom
            | RogueSpell::KidneyShot => fight.energy_bar().combo_points > 0,
            RogueSpell::ExposeArmor => fight
                .agent
                .expose_armor
                .is_some_and(|expose_armor| expose_armor.can_cast(fight)),
            RogueSpell::Riposte => fight
                .agent
                .riposte
                .is_some_and(|riposte| riposte.can_cast(fight)),
            RogueSpell::Garrote => fight
                .agent
                .garrote
                .is_some_and(|garrote| garrote.can_cast(fight, fight.agent.stealth)),
            RogueSpell::Ambush => fight
                .agent
                .ambush
                .is_some_and(|ambush| ambush.can_cast(fight, fight.agent.stealth)),
            RogueSpell::Mutilate => fight
                .agent
                .mutilate
                .is_some_and(|mutilate| mutilate.daggers),
            RogueSpell::Premeditation => fight
                .agent
                .stealth
                .is_some_and(|stealth| stealth.active(fight)),
            RogueSpell::Stealth => Stealth::can_cast(fight),
            _ => true,
        }
    }

    fn extra_condition_proof(
        _fight: &Fight<Self>,
        _spell: SpellId,
        behavior: RogueSpell,
    ) -> Option<(i64, u8)> {
        use crate::core::{fight::reads, time::NEVER_EXPIRES};
        Some(match behavior {
            // The weapons, the position, or the clock past the prepull.
            RogueSpell::Backstab | RogueSpell::Mutilate | RogueSpell::Stealth => (NEVER_EXPIRES, 0),
            RogueSpell::Eviscerate
            | RogueSpell::SliceAndDice
            | RogueSpell::Rupture
            | RogueSpell::Venom
            | RogueSpell::KidneyShot => (NEVER_EXPIRES, reads::RESOURCES),
            // The combo points, and the category's active member.
            RogueSpell::ExposeArmor => (NEVER_EXPIRES, reads::RESOURCES | reads::AURAS),
            // Stealth, Cutthroat or the Riposte window.
            RogueSpell::Riposte
            | RogueSpell::Garrote
            | RogueSpell::Ambush
            | RogueSpell::Premeditation => (NEVER_EXPIRES, reads::AURAS),
            _ => return None,
        })
    }

    fn modify_cast(fight: &mut Fight<Self>, spell: SpellId, behavior: RogueSpell) {
        if matches!(
            behavior,
            RogueSpell::Eviscerate
                | RogueSpell::SliceAndDice
                | RogueSpell::Rupture
                | RogueSpell::Venom
                | RogueSpell::KidneyShot
                | RogueSpell::ExposeArmor
        ) {
            let points = fight.energy_bar().combo_points as usize;
            fight.set_metrics_split(spell, points);
        }
    }

    fn should_activate(fight: &Fight<Self>, _spell: SpellId, behavior: RogueSpell) -> bool {
        match behavior {
            RogueSpell::AdrenalineRush => fight
                .agent
                .adrenaline_rush
                .is_some_and(|rush| rush.should_activate(fight)),
            RogueSpell::Preparation => fight
                .agent
                .preparation
                .as_ref()
                .is_some_and(|preparation| preparation.should_activate(fight)),
            _ => true,
        }
    }

    fn on_dot_tick(fight: &mut Fight<Self>, dot: DotId, behavior: RogueSpell) {
        match behavior {
            RogueSpell::DeadlyPoison => fight.snapshot_dot_tick(dot),
            RogueSpell::Rupture => {
                let rupture = fight.agent.rupture.clone().expect("Rupture is bound");
                let position = fight.dots[dot].side.target_position().unwrap_or(0);
                let snapshot = fight.agent.rupture_snapshot[position];
                rupture.tick(fight, dot, snapshot);
            }
            RogueSpell::Garrote => {
                let garrote = fight.agent.garrote.expect("Garrote is bound");
                let position = fight.dots[dot].side.target_position().unwrap_or(0);
                let snapshot = fight.agent.garrote_snapshot[position];
                rupture::bleed_tick(fight, dot, snapshot, garrote.tick_outcome);
            }
            _ => {}
        }
    }

    fn on_gain(fight: &mut Fight<Self>, _aura: AuraRef, kind: RogueAura) {
        match kind {
            RogueAura::SliceAndDice => {
                let slice = fight.agent.slice_and_dice.clone().expect("bound");
                slice.on_gain(fight);
            }
            RogueAura::BladeFlurry => fight.agent.blade_flurry.expect("bound").on_gain(fight),
            RogueAura::AdrenalineRush => fight.agent.adrenaline_rush.expect("bound").on_gain(fight),
            RogueAura::Venom => {
                let venom = fight.agent.venom.clone().expect("bound");
                fight.agent.poison_bonus += venom.chance_bonus;
                venom.on_gain(fight);
            }
            RogueAura::Quietus => fight.agent.quietus.expect("bound").on_gain(fight),
            RogueAura::ExposeArmor => fight.agent.expose_armor.expect("bound").on_gain(fight),
            RogueAura::KidneyShotStun => {
                let kidney_shot = fight.agent.kidney_shot.expect("bound");
                fight.agent.kidney_shot_pause = kidney_shot.pause_target(fight);
                kidney_shot.on_stun_change(fight, true);
            }
            RogueAura::ColdBlood => fight
                .agent
                .cold_blood
                .clone()
                .expect("bound")
                .on_gain(fight),
            RogueAura::ThousandCuts => fight
                .agent
                .thousand_cuts
                .clone()
                .expect("bound")
                .on_gain(fight),
            _ => {}
        }
    }

    fn on_execute_phase(fight: &mut Fight<Self>, phase: i32) {
        if let Some(quietus) = fight.agent.quietus {
            quietus.on_execute_phase(fight, phase);
        }
    }

    fn on_stacks_change(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: RogueAura,
        _old: i32,
        new: i32,
    ) {
        if kind == RogueAura::ThousandCuts {
            let cuts = fight.agent.thousand_cuts.clone().expect("bound");
            cuts.on_stacks_change(fight, new);
        }
    }

    fn on_apply_effects(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: RogueAura,
        spell: SpellId,
        _target: Side,
    ) {
        if kind == RogueAura::ThousandCuts {
            let cuts = fight.agent.thousand_cuts.clone().expect("bound");
            cuts.on_apply_effects(fight, spell);
        }
    }

    fn on_enemy_hit_taken(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: RogueAura,
        result: &SpellResult,
    ) {
        if kind == RogueAura::RiposteTrigger {
            let riposte = fight.agent.riposte.expect("bound");
            riposte.on_hit_taken(fight, result);
        }
    }

    fn on_periodic_damage_dealt(
        fight: &mut Fight<Self>,
        aura: AuraRef,
        kind: RogueAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        if let RogueAura::Proc(index) = kind {
            let trigger = fight.agent.procs[index].clone();
            if trigger.periodic {
                trigger.callback(fight, aura, spell, result);
            }
        }
    }

    fn on_delayed_proc(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: RogueAura,
        _spell: SpellId,
        _result: SpellResult,
    ) {
        if let RogueAura::Proc(index) = kind {
            let trigger = fight.agent.procs[index].clone();
            trigger.handle(fight);
        }
    }

    fn on_expire(fight: &mut Fight<Self>, _aura: AuraRef, kind: RogueAura) {
        match kind {
            RogueAura::SliceAndDice => {
                let slice = fight.agent.slice_and_dice.clone().expect("bound");
                slice.on_expire(fight);
            }
            RogueAura::BladeFlurry => fight.agent.blade_flurry.expect("bound").on_expire(fight),
            RogueAura::AdrenalineRush => {
                fight.agent.adrenaline_rush.expect("bound").on_expire(fight)
            }
            RogueAura::ColdBlood => fight
                .agent
                .cold_blood
                .clone()
                .expect("bound")
                .on_expire(fight),
            RogueAura::Venom => {
                let venom = fight.agent.venom.clone().expect("bound");
                fight.agent.poison_bonus -= venom.chance_bonus;
                venom.on_expire(fight);
            }
            RogueAura::Quietus => fight.agent.quietus.expect("bound").on_expire(fight),
            RogueAura::KidneyShotStun => {
                let kidney_shot = fight.agent.kidney_shot.expect("bound");
                let pause = fight.agent.kidney_shot_pause;
                kidney_shot.resume_target(fight, pause);
                kidney_shot.on_stun_change(fight, false);
            }
            RogueAura::ThousandCuts => fight
                .agent
                .thousand_cuts
                .clone()
                .expect("bound")
                .on_expire(fight),
            _ => {}
        }
    }

    fn caster_damage_multiplier(fight: &Fight<Self>, spell: SpellId, target: Side) -> Option<f64> {
        fight
            .agent
            .rupture
            .as_ref()
            .and_then(|rupture| rupture.caster_multiplier(fight, spell, target))
    }

    fn on_spell_hit_dealt(
        fight: &mut Fight<Self>,
        aura: AuraRef,
        kind: RogueAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        let poison = match kind {
            RogueAura::InstantPoisonTrigger => fight.agent.instant_poison.clone(),
            RogueAura::DeadlyPoisonTrigger => fight.agent.deadly_poison.clone(),
            RogueAura::WoundPoisonTrigger => fight.agent.wound_poison.clone(),
            RogueAura::ColdBlood => {
                let cold_blood = fight.agent.cold_blood.clone().expect("bound");
                cold_blood.on_spell_hit_dealt(fight, spell);
                // Both Mutilate hands take the crit, then the off-hand strike, always second,
                // spends the charge (forever-bugs #224, community #785).
                if fight
                    .agent
                    .mutilate
                    .as_ref()
                    .is_some_and(|mutilate| spell == mutilate.off_hand)
                {
                    fight.deactivate_aura(cold_blood.aura);
                }
                None
            }
            RogueAura::Proc(index) => {
                let trigger = fight.agent.procs[index].clone();
                if !trigger.periodic {
                    trigger.callback(fight, aura, spell, result);
                }
                None
            }
            RogueAura::BladeFlurry => {
                let flurry = fight.agent.blade_flurry.expect("Blade Flurry is bound");
                if let Some(damage) = flurry.on_spell_hit_dealt(fight, spell, result) {
                    fight.agent.blade_flurry_damage = damage;
                    flurry.strike(fight, result.target);
                }
                None
            }
            _ => None,
        };
        if let Some(poison) = poison {
            let bonus = fight.agent.poison_bonus;
            poison.on_spell_hit_dealt(fight, spell, result, bonus);
        }
    }
}
