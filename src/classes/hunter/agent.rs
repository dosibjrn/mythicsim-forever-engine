//! The Hunter class agent for the fight runtime: class spells and auras by name, and the
//! hooks that dispatch to each spell's module.

use std::rc::Rc;

use crate::{
    contracts::prepared_v2::{Effect, PreparedV2, Spell as ExportedSpell},
    core::{
        fight::{
            action_string, casting_regen::CastingRegen, melee::Hand, Agent, AuraRef, DotId, Fight,
            Outcome, Side, SpellId, SpellResult,
        },
        time::{go_string, NS_PER_MILLISECOND},
    },
};

use super::items::ManaProc;
use super::pet::{
    self as hunter_pet, FuriousHowl, PetAbility, PetAi, PetAuras,
    DUST_CLOUD_YIELDS_TO_SUNDER_AND_EXPOSE, SUNDER_ARMOR_CATEGORY,
};
use super::spells::{
    aspect_of_the_hawk::AspectOfTheHawk,
    explosive_trap::{self, ExplosiveTrap},
    melee::{self, MongooseBite, RaptorStrike},
    rapid_fire::RapidFire,
    serpent_sting,
    serpent_sting::SerpentSting,
    shots,
    summon_hawk::SummonHawk,
    volley::{self, Volley},
};
use super::talents::survival::SurvivalProc;

/// What a Hunter spell does when its effects apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HunterSpell {
    AimedShot,
    RenatakisCharm,
    ArcaneShot,
    SniperShot,
    MultiShot,
    SerpentSting,
    AspectOfTheHawk,
    RapidFire,
    SummonHawk,
    /// A hawk slot's dot spell.
    Hawk,
    /// A pet ability, by its position among the rolled ranges.
    PetAbility(usize),
    Intimidation,
    BestialWrath,
    AspectOfTheBeast,
    RaptorStrikeQueue,
    RaptorStrike,
    RaptorStrikeHit,
    MongooseBite,
    LaceratingStrikes,
    StriderKick,
    WingClip,
    /// traps.go: Freezing Trap's empty `ApplyEffects`.
    FreezingTrap,
    ImmolationTrap,
    ExplosiveTrap,
    Volley,
}

/// Class auras with Rust behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HunterAura {
    AspectOfTheHawk,
    QuickShots,
    RapidFire,
    Intimidation,
    BestialWrath,
    FrenzyTrigger,
    FrenzyEffect,
    DustCloud,
    /// The Wolf's Furious Howl on the pet.
    FuriousHowl,
    AspectOfTheBeast,
    QuickStrikes,
    RaptorStrikeQueued,
    ResourcefulnessTrigger,
    Resourcefulness,
    ExposePrey,
    RapidRecuperationTrigger,
    RapidRecuperation,
    /// A set bonus mana proc's trigger, by its position in the agent's procs.
    ManaProc(usize),
}

/// Hunter state that Go keeps in the `Hunter` struct and its closures.
#[derive(Default)]
pub(crate) struct HunterAgent {
    aimed_shot_bonus: f64,
    /// Arcane Shot's flat damage and ranged attack power share.
    arcane_shot: (f64, f64),
    /// The shots Renataki's Charm of Beasts resets.
    renatakis_shots: Vec<SpellId>,
    /// The set bonus mana procs, in effect order.
    mana_procs: Vec<ManaProc>,
    sniper_shot_bonus: f64,
    serpent_sting: Option<SerpentSting>,
    aspect: Option<Rc<AspectOfTheHawk>>,
    rapid_fire: Option<RapidFire>,
    summon_hawk: Option<Rc<SummonHawk>>,
    /// The pet's damage abilities, by their effect order.
    pet_abilities: Vec<PetAbility>,
    /// Dust Cloud's target aura and the armor it changes.
    pub(crate) dust_cloud: Option<(AuraRef, f64)>,
    pet_ai: Option<Rc<PetAi>>,
    pet_auras: Rc<PetAuras>,
    beast: Option<Rc<AspectOfTheHawk>>,
    raptor_strike: Option<RaptorStrike>,
    mongoose_bite: Option<MongooseBite>,
    wing_clip_damage: f64,
    /// The aura Strider Kick activates after its hit.
    strider_kick_speed: Option<AuraRef>,
    /// The Wolf's Furious Howl.
    pub(crate) furious_howl: Option<FuriousHowl>,
    /// Resourcefulness's trigger and its regeneration.
    resourcefulness: Option<(SurvivalProc, CastingRegen)>,
    /// The spells Resourcefulness's trigger hears, by spellbook position.
    resourcefulness_spells: Rc<Vec<bool>>,
    /// Expose Prey's trigger and whether the target carries Hunter's Mark.
    expose_prey: Option<(SurvivalProc, bool)>,
    /// Rapid Recuperation's trigger and its regeneration.
    rapid_recuperation: Option<(SurvivalProc, CastingRegen)>,
    immolation_trap: Option<melee::ImmolationTrap>,
    explosive_trap: Option<ExplosiveTrap>,
    volley: Option<Volley>,
}

/// Player aura labels claimed by implemented class effects.
fn class_auras(prepared: &PreparedV2) -> Vec<(String, HunterAura)> {
    let mut auras = Vec::new();
    for effect in &prepared.effects {
        match effect {
            Effect::AspectOfTheHawk {
                aura, proc_aura, ..
            } => {
                auras.push((aura.clone(), HunterAura::AspectOfTheHawk));
                if let Some(proc_aura) = proc_aura {
                    auras.push((proc_aura.clone(), HunterAura::QuickShots));
                }
            }
            Effect::RapidFire { aura, .. } => auras.push((aura.clone(), HunterAura::RapidFire)),
            Effect::AspectOfTheBeast {
                aura, proc_aura, ..
            } => {
                auras.push((aura.clone(), HunterAura::AspectOfTheBeast));
                if let Some(proc_aura) = proc_aura {
                    auras.push((proc_aura.clone(), HunterAura::QuickStrikes));
                }
            }
            Effect::RaptorStrike { queue_aura, .. } => {
                auras.push((queue_aura.clone(), HunterAura::RaptorStrikeQueued))
            }
            Effect::Resourcefulness {
                trigger_aura, aura, ..
            } => {
                auras.push((trigger_aura.clone(), HunterAura::ResourcefulnessTrigger));
                auras.push((aura.clone(), HunterAura::Resourcefulness));
            }
            Effect::ExposePrey { trigger_aura, .. } => {
                auras.push((trigger_aura.clone(), HunterAura::ExposePrey))
            }
            Effect::RapidRecuperation {
                trigger_aura, aura, ..
            } => {
                auras.push((trigger_aura.clone(), HunterAura::RapidRecuperationTrigger));
                auras.push((aura.clone(), HunterAura::RapidRecuperation));
            }
            Effect::HunterSetManaProc { trigger_aura, .. } => {
                let index = auras
                    .iter()
                    .filter(|(_, kind)| matches!(kind, HunterAura::ManaProc(_)))
                    .count();
                auras.push((trigger_aura.clone(), HunterAura::ManaProc(index)));
            }
            _ => {}
        }
    }
    auras
}

/// Pet aura labels claimed by implemented class effects.
fn pet_auras(prepared: &PreparedV2) -> Vec<(String, HunterAura)> {
    let mut auras = Vec::new();
    for effect in &prepared.effects {
        match effect {
            Effect::Intimidation { aura, .. } => {
                auras.push((aura.clone(), HunterAura::Intimidation))
            }
            Effect::BestialWrath { aura, .. } => {
                auras.push((aura.clone(), HunterAura::BestialWrath))
            }
            Effect::Frenzy {
                trigger_aura, aura, ..
            } => {
                auras.push((trigger_aura.clone(), HunterAura::FrenzyTrigger));
                auras.push((aura.clone(), HunterAura::FrenzyEffect));
            }
            Effect::HunterPetFuriousHowl { pet_aura, .. } => {
                auras.push((pet_aura.clone(), HunterAura::FuriousHowl))
            }
            _ => {}
        }
    }
    auras
}

/// The pet abilities' spell IDs and what each does, in effect order.
fn pet_abilities(prepared: &PreparedV2) -> Vec<(i32, PetAbility)> {
    prepared
        .effects
        .iter()
        .filter_map(|effect| {
            Some((
                PetAbility::spell_id(effect)?,
                PetAbility::from_effect(effect)?,
            ))
        })
        .collect()
}

/// The class behavior of an exported spell, if Rust implements it.
pub(crate) fn spell_behavior(spell: &ExportedSpell) -> Option<HunterSpell> {
    let id = spell.action_id.clone().unwrap_or_default();
    let class = spell.class_spell.as_deref()?;
    if id.item_id != 0 {
        return None;
    }
    if id.tag != 0 {
        return match (class, id.tag) {
            ("summon_hawk", _) if spell.dot.is_some() => Some(HunterSpell::Hawk),
            ("raptor_strike", 1) => Some(HunterSpell::RaptorStrikeHit),
            ("raptor_strike_queue", 3) => Some(HunterSpell::RaptorStrikeQueue),
            _ => None,
        };
    }
    match class {
        "aimed_shot" => Some(HunterSpell::AimedShot),
        "arcane_shot" => Some(HunterSpell::ArcaneShot),
        "sniper_shot" => Some(HunterSpell::SniperShot),
        "multi_shot" => Some(HunterSpell::MultiShot),
        "serpent_sting" if spell.dot.is_some() => Some(HunterSpell::SerpentSting),
        "aspect_of_the_hawk" => Some(HunterSpell::AspectOfTheHawk),
        "rapid_fire" => Some(HunterSpell::RapidFire),
        "summon_hawk" => Some(HunterSpell::SummonHawk),
        "intimidation" => Some(HunterSpell::Intimidation),
        "bestial_wrath" => Some(HunterSpell::BestialWrath),
        "aspect_of_the_beast" => Some(HunterSpell::AspectOfTheBeast),
        "raptor_strike" => Some(HunterSpell::RaptorStrike),
        "mongoose_bite" => Some(HunterSpell::MongooseBite),
        "lacerating_strikes" if spell.dot.is_some() => Some(HunterSpell::LaceratingStrikes),
        "strider_kick" => Some(HunterSpell::StriderKick),
        "wing_clip" => Some(HunterSpell::WingClip),
        "freezing_trap" => Some(HunterSpell::FreezingTrap),
        "immolation_trap" if spell.dot.is_some() => Some(HunterSpell::ImmolationTrap),
        "explosive_trap" if spell.dot.is_some() => Some(HunterSpell::ExplosiveTrap),
        "volley" if spell.dot.as_ref().is_some_and(|dot| dot.channeled) => {
            Some(HunterSpell::Volley)
        }
        _ => None,
    }
}

impl HunterAgent {
    /// Build a fight for a prepared input that passed the coverage gate.
    pub(crate) fn fight(prepared: &PreparedV2) -> Result<Fight<HunterAgent>, String> {
        let auras = class_auras(prepared);
        let pet_aura_kinds = pet_auras(prepared);
        let abilities = pet_abilities(prepared);
        let renatakis = prepared.effects.iter().find_map(|effect| match effect {
            Effect::RenatakisCharm { item_id, .. } => Some(*item_id),
            _ => None,
        });
        let behavior = |spell: &ExportedSpell| {
            let id = spell.action_id.clone().unwrap_or_default();
            if id.item_id != 0 && Some(id.item_id) == renatakis && id.tag == 0 {
                return Some(HunterSpell::RenatakisCharm);
            }
            // Dust Cloud carries no damage mask.
            let pet_ability = match spell.class_spell.as_deref() {
                Some(class) => class == "pet_damage",
                None => abilities.iter().any(|(spell_id, ability)| {
                    *spell_id == id.spell_id
                        && matches!(
                            ability,
                            PetAbility::DustCloud | PetAbility::FuriousHowl { .. }
                        )
                }),
            };
            if pet_ability && id.tag == 0 && id.item_id == 0 {
                return abilities
                    .iter()
                    .position(|(spell_id, _)| *spell_id == id.spell_id)
                    .map(HunterSpell::PetAbility);
            }
            spell_behavior(spell)
        };
        let target_auras: Vec<(String, HunterAura)> = prepared
            .effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::HunterPetDustCloud { aura, .. } => {
                    Some((aura.clone(), HunterAura::DustCloud))
                }
                _ => None,
            })
            .collect();
        let mut fight = Fight::new(prepared, HunterAgent::default(), behavior, |unit, label| {
            let list = match unit {
                "player" => &auras,
                "pet" => &pet_aura_kinds,
                "target" => &target_auras,
                _ => return None,
            };
            list.iter()
                .find(|(name, _)| name == label)
                .map(|(_, kind)| *kind)
        })?;
        for (spell_id, ability) in &abilities {
            if let PetAbility::Bleed { tick_base, .. } = ability {
                let spell = fight
                    .spells
                    .iter()
                    .position(|spell| spell.caster.is_pet() && spell.id.spell_id == *spell_id)
                    .ok_or_else(|| format!("pet ability {spell_id} is not registered"))?;
                let dot = fight.spells[spell]
                    .dot
                    .ok_or_else(|| format!("pet ability {spell_id} has no dot"))?;
                fight.dots[dot].tick_base = Some(*tick_base);
            }
        }
        fight.agent.pet_abilities = abilities.into_iter().map(|(_, ability)| ability).collect();
        for effect in &prepared.effects {
            if let Effect::HunterPetDustCloud { aura, armor, .. } = effect {
                let index = fight.trackers[Side::Target.index()]
                    .find(aura)
                    .ok_or_else(|| format!("target aura {aura} is not registered"))?;
                let aura = AuraRef {
                    side: Side::Target,
                    index,
                };
                fight.agent.dust_cloud = Some((aura, *armor));
            }
        }
        let pet = prepared.effects.iter().find_map(|effect| match effect {
            Effect::HunterPet { pet, .. } => fight.pet_side(pet),
            _ => None,
        });
        for effect in &prepared.effects {
            if let Effect::HunterPetFuriousHowl {
                pet_aura,
                owner_aura,
                attack_power,
                ..
            } = effect
            {
                let pet = pet.ok_or("the hunter's pet is not simulated")?;
                fight.agent.furious_howl = Some(FuriousHowl {
                    pet,
                    pet_aura: PetAuras::pet_aura(&fight, pet, pet_aura)?,
                    owner_aura: fight.player_aura(owner_aura)?,
                    attack_power: *attack_power,
                });
            }
        }
        let mut pet_auras = PetAuras {
            pet,
            ..PetAuras::default()
        };
        let pet_side = || pet.ok_or("the hunter's pet is not simulated");
        let find_spell = |fight: &Fight<HunterAgent>, id: i32| {
            fight.spells.iter().position(|spell| {
                spell.id.spell_id == id && spell.id.tag == 0 && spell.id.item_id == 0
            })
        };
        for effect in &prepared.effects {
            match effect {
                Effect::HunterPet {
                    rotation,
                    special_ability,
                    focus_dump,
                    extra_ability,
                    wait_ns,
                    melee_range,
                    move_to,
                    uptime,
                    ..
                } => {
                    let mut ai = PetAi::bind(
                        &fight,
                        pet_side()?,
                        rotation,
                        *special_ability,
                        *focus_dump,
                        *extra_ability,
                        *wait_ns,
                        *melee_range,
                        *move_to,
                    )?;
                    ai.uptime = *uptime;
                    fight.agent.pet_ai = Some(Rc::new(ai));
                }
                Effect::Intimidation {
                    aura, crit_bonus, ..
                } => {
                    pet_auras.intimidation =
                        Some((PetAuras::pet_aura(&fight, pet_side()?, aura)?, *crit_bonus));
                }
                Effect::BestialWrath {
                    aura,
                    damage_multiplier,
                    ..
                } => {
                    pet_auras.bestial_wrath = Some((
                        PetAuras::pet_aura(&fight, pet_side()?, aura)?,
                        *damage_multiplier,
                    ));
                }
                Effect::Frenzy {
                    trigger_aura,
                    aura,
                    proc_chance,
                    speed_multiplier,
                    ..
                } => {
                    pet_auras.frenzy = Some((
                        PetAuras::pet_aura(&fight, pet_side()?, trigger_aura)?,
                        PetAuras::pet_aura(&fight, pet_side()?, aura)?,
                        *proc_chance,
                        *speed_multiplier,
                    ));
                }
                Effect::AimedShot { flat_bonus, .. } => fight.agent.aimed_shot_bonus = *flat_bonus,
                Effect::ArcaneShot {
                    base_damage,
                    rap_coefficient,
                    ..
                } => fight.agent.arcane_shot = (*base_damage, *rap_coefficient),
                Effect::HunterSetManaProc { .. } => {
                    let proc = ManaProc::bind(&mut fight, effect)?.expect("a mana proc");
                    fight.agent.mana_procs.push(proc);
                }
                Effect::RenatakisCharm { shots, .. } => {
                    let mut resets = Vec::new();
                    for &shot in shots {
                        resets.push(
                            find_spell(&fight, shot)
                                .ok_or_else(|| format!("shot {shot} is not registered"))?,
                        );
                    }
                    fight.agent.renatakis_shots = resets;
                }
                Effect::SniperShot { flat_bonus, .. } => {
                    fight.agent.sniper_shot_bonus = *flat_bonus
                }
                Effect::SerpentSting {
                    spell_id,
                    tick_base,
                    attack_power_share,
                    tick_outcome,
                } => {
                    let spell = find_spell(&fight, *spell_id)
                        .ok_or_else(|| format!("Serpent Sting {spell_id} is not registered"))?;
                    let sting = SerpentSting::bind(
                        &fight,
                        spell,
                        *tick_base,
                        *attack_power_share,
                        tick_outcome,
                    )?;
                    fight.agent.serpent_sting = Some(sting);
                }
                Effect::AspectOfTheHawk {
                    aura,
                    proc_aura,
                    haste_multiplier,
                    proc_chance,
                    ..
                } => {
                    let quick_shots = match (proc_aura, haste_multiplier, proc_chance) {
                        (Some(label), Some(haste), Some(chance)) => {
                            Some((label.as_str(), *haste, *chance))
                        }
                        (None, None, None) => None,
                        _ => return Err("Deadly Aspects is incomplete".into()),
                    };
                    let bound = AspectOfTheHawk::bind(
                        &fight,
                        &prepared.effects,
                        &prepared.player.spells,
                        aura,
                        quick_shots,
                        false,
                    )?;
                    fight.agent.aspect = Some(Rc::new(bound));
                    // Go aspects.go: each aspect's aura holds a single aura "Aspect" effect, so
                    // casting Aspect of the Beast switches Aspect of the Hawk off and back.
                    fight.enforce_exported_category(Side::Player, "Aspect");
                }
                Effect::AspectOfTheBeast {
                    aura,
                    proc_aura,
                    haste_multiplier,
                    proc_chance,
                    ..
                } => {
                    let quick_strikes = match (proc_aura, haste_multiplier, proc_chance) {
                        (Some(label), Some(haste), Some(chance)) => {
                            Some((label.as_str(), *haste, *chance))
                        }
                        (None, None, None) => None,
                        _ => return Err("Deadly Aspects is incomplete".into()),
                    };
                    let bound = AspectOfTheHawk::bind(
                        &fight,
                        &prepared.effects,
                        &prepared.player.spells,
                        aura,
                        quick_strikes,
                        true,
                    )?;
                    fight.agent.beast = Some(Rc::new(bound));
                    fight.enforce_exported_category(Side::Player, "Aspect");
                }
                Effect::RaptorStrike {
                    spell_id,
                    queue_aura,
                    base_damage,
                    melee_range,
                } => {
                    let bound = RaptorStrike::bind(
                        &fight,
                        *spell_id,
                        queue_aura,
                        *base_damage,
                        *melee_range,
                    )?;
                    fight.agent.raptor_strike = Some(bound);
                }
                Effect::MongooseBite {
                    spell_id,
                    aura,
                    base_damage,
                    lacerating_share,
                    lacerating_tick_outcome,
                } => {
                    find_spell(&fight, *spell_id)
                        .ok_or_else(|| format!("Mongoose Bite {spell_id} is not registered"))?;
                    let lacerating = match (lacerating_share, lacerating_tick_outcome) {
                        (Some(share), Some(outcome)) => Some((
                            *share,
                            serpent_sting::tick_outcome(outcome).ok_or_else(|| {
                                format!("Lacerating Strikes tick outcome {outcome} is unsupported")
                            })?,
                        )),
                        (None, None) => None,
                        _ => return Err("Lacerating Strikes is incomplete".into()),
                    };
                    let bound = MongooseBite::bind(&fight, aura, *base_damage, lacerating)?;
                    fight.agent.mongoose_bite = Some(bound);
                }
                Effect::StriderKick { speed_aura, .. } => {
                    fight.agent.strider_kick_speed = speed_aura
                        .as_deref()
                        .map(|label| fight.player_aura(label))
                        .transpose()?;
                }
                Effect::WingClip { base_damage, .. } => fight.agent.wing_clip_damage = *base_damage,
                Effect::ImmolationTrap {
                    spell_id,
                    tick_base,
                    attack_power_share,
                    tick_can_crit,
                    tick_magic,
                } => {
                    let spell = find_spell(&fight, *spell_id)
                        .ok_or_else(|| format!("Immolation Trap {spell_id} is not registered"))?;
                    let dot = fight.spells[spell]
                        .dot
                        .ok_or("Immolation Trap has no dot")?;
                    fight.dots[dot].tick_base = Some(*tick_base);
                    // traps.go: the share split over the burn's ticks.
                    fight.agent.immolation_trap = Some(melee::ImmolationTrap {
                        tick_base: *tick_base,
                        attack_power_per_tick: attack_power_share
                            / f64::from(fight.dots[dot].base_tick_count),
                    });
                    fight.dots[dot].tick_can_crit = explosive_trap::trap_tick_crit(
                        Outcome::tick_hit_rolled(*tick_can_crit, *tick_magic)?,
                    )?;
                }
                Effect::ExplosiveTrap {
                    spell_id,
                    hit_min,
                    hit_max,
                    attack_power_share,
                    hits,
                    aoe_cap_multiplier,
                    tick_base,
                    tick_can_crit,
                    tick_magic,
                } => {
                    let spell = find_spell(&fight, *spell_id)
                        .ok_or_else(|| format!("Explosive Trap {spell_id} is not registered"))?;
                    let immolation = prepared.effects.iter().find_map(|effect| match effect {
                        Effect::ImmolationTrap { spell_id, .. } => find_spell(&fight, *spell_id),
                        _ => None,
                    });
                    fight.agent.explosive_trap = Some(explosive_trap::bind(
                        &mut fight,
                        spell,
                        *hit_min,
                        *hit_max,
                        *attack_power_share,
                        *hits,
                        *aoe_cap_multiplier,
                        *tick_base,
                        Outcome::tick_hit_rolled(*tick_can_crit, *tick_magic)?,
                        immolation,
                    )?);
                }
                Effect::Volley {
                    spell_id,
                    tick_base,
                    ranged_delay_ns,
                } => {
                    let spell = find_spell(&fight, *spell_id)
                        .ok_or_else(|| format!("Volley {spell_id} is not registered"))?;
                    fight.agent.volley = Some(volley::bind(
                        &mut fight,
                        spell,
                        *tick_base,
                        *ranged_delay_ns,
                    )?);
                }
                Effect::Resourcefulness {
                    trigger_aura,
                    aura,
                    proc_chance,
                    regen,
                    spells,
                } => {
                    let bound = SurvivalProc::bind(&fight, trigger_aura, aura, *proc_chance)?;
                    let count = fight.spells.len();
                    if spells.iter().any(|&spell| spell >= count) {
                        return Err("Resourcefulness names a spell outside the spellbook".into());
                    }
                    let mut hears = vec![false; count];
                    for &spell in spells {
                        hears[spell] = true;
                    }
                    let regen = CastingRegen::bind(&mut fight, bound.aura, *regen)?;
                    fight.agent.resourcefulness = Some((bound, regen));
                    fight.agent.resourcefulness_spells = Rc::new(hears);
                }
                Effect::ExposePrey {
                    trigger_aura,
                    aura,
                    proc_chance,
                    marked,
                } => {
                    let bound = SurvivalProc::bind(&fight, trigger_aura, aura, *proc_chance)?;
                    fight.agent.expose_prey = Some((bound, *marked));
                }
                Effect::RapidRecuperation {
                    trigger_aura,
                    aura,
                    regen,
                } => {
                    let bound = SurvivalProc::bind(&fight, trigger_aura, aura, 1.0)?;
                    let regen = CastingRegen::bind(&mut fight, bound.aura, *regen)?;
                    fight.agent.rapid_recuperation = Some((bound, regen));
                }
                Effect::RapidFire {
                    aura,
                    haste_multiplier,
                    ..
                } => {
                    fight.agent.rapid_fire = Some(RapidFire::bind(&fight, aura, *haste_multiplier)?)
                }
                Effect::SummonHawk {
                    base_damage,
                    attack_power_share,
                    swing_share,
                    always_hits,
                    hawk_spells,
                    ..
                } => {
                    let bound = SummonHawk::bind(
                        &mut fight,
                        *base_damage,
                        *attack_power_share,
                        *swing_share,
                        *always_hits,
                        hawk_spells,
                    )?;
                    fight.agent.summon_hawk = Some(Rc::new(bound));
                }
                _ => {}
            }
        }
        fight.agent.pet_auras = Rc::new(pet_auras);
        Ok(fight)
    }

    fn aspect(fight: &Fight<Self>) -> Rc<AspectOfTheHawk> {
        fight
            .agent
            .aspect
            .clone()
            .expect("Aspect of the Hawk is bound")
    }

    fn beast(fight: &Fight<Self>) -> Rc<AspectOfTheHawk> {
        fight
            .agent
            .beast
            .clone()
            .expect("Aspect of the Beast is bound")
    }

    fn raptor(fight: &Fight<Self>) -> RaptorStrike {
        fight.agent.raptor_strike.expect("Raptor Strike is bound")
    }

    fn mongoose(fight: &Fight<Self>) -> MongooseBite {
        fight
            .agent
            .mongoose_bite
            .clone()
            .expect("Mongoose Bite is bound")
    }

    fn rapid_fire(fight: &Fight<Self>) -> RapidFire {
        fight.agent.rapid_fire.expect("Rapid Fire is bound")
    }

    fn summon_hawk(fight: &Fight<Self>) -> Rc<SummonHawk> {
        fight
            .agent
            .summon_hawk
            .clone()
            .expect("Summon Hawk is bound")
    }
}

impl Agent for HunterAgent {
    type Spell = HunterSpell;
    type Aura = HunterAura;

    fn apply_effects(fight: &mut Fight<Self>, spell: SpellId, target: Side, behavior: HunterSpell) {
        match behavior {
            HunterSpell::AimedShot => {
                let bonus = fight.agent.aimed_shot_bonus;
                shots::apply(fight, spell, target, bonus);
            }
            HunterSpell::SniperShot => {
                let bonus = fight.agent.sniper_shot_bonus;
                shots::apply(fight, spell, target, bonus);
            }
            HunterSpell::MultiShot => shots::multi_shot(fight, spell, target),
            HunterSpell::ArcaneShot => {
                let (base, share) = fight.agent.arcane_shot;
                shots::arcane_shot(fight, spell, target, base, share);
            }
            HunterSpell::RenatakisCharm => {
                // Go Cooldown.Reset on each shot's timer, which the shots sharing it share.
                for index in 0..fight.agent.renatakis_shots.len() {
                    let shot = fight.agent.renatakis_shots[index];
                    if let Some((timer, _)) = fight.spells[shot].cd {
                        fight.timers[timer] = crate::core::time::STARTING_CD_TIME;
                    }
                }
            }
            HunterSpell::SerpentSting => serpent_sting::apply(fight, spell, target),
            HunterSpell::AspectOfTheHawk => {
                let aura = Self::aspect(fight).aura;
                fight.activate_aura(aura);
            }
            HunterSpell::RapidFire => {
                let aura = Self::rapid_fire(fight).aura;
                fight.activate_aura(aura);
            }
            HunterSpell::SummonHawk => Self::summon_hawk(fight).apply(fight, spell, target),
            HunterSpell::Hawk => panic!("a hawk is never cast"),
            HunterSpell::PetAbility(index) => {
                let ability = fight.agent.pet_abilities[index];
                ability.apply(fight, spell, target);
            }
            HunterSpell::Intimidation => {
                let (aura, _) = fight
                    .agent
                    .pet_auras
                    .intimidation
                    .expect("Intimidation is bound");
                fight.activate_aura(aura);
            }
            HunterSpell::BestialWrath => {
                let (aura, _) = fight
                    .agent
                    .pet_auras
                    .bestial_wrath
                    .expect("Bestial Wrath is bound");
                fight.activate_aura(aura);
            }
            HunterSpell::AspectOfTheBeast => {
                let aura = Self::beast(fight).aura;
                fight.activate_aura(aura);
            }
            HunterSpell::RaptorStrikeQueue => {
                let aura = Self::raptor(fight).queue_aura;
                fight.activate_aura(aura);
            }
            HunterSpell::RaptorStrike => Self::raptor(fight).apply(fight, target),
            HunterSpell::RaptorStrikeHit => Self::raptor(fight).apply_hit(fight, spell, target),
            HunterSpell::MongooseBite => {
                let bite = Self::mongoose(fight);
                if let Some((bleed, damage)) = bite.apply(fight, spell, target) {
                    // Go procLaceratingStrikes (patches 87 and 132): what a running bleed still
                    // owes is read first, the bleed is cast, keeping its tick timer, and its
                    // snapshot is written after, the owed damage and the bite's share over the
                    // ticks left.
                    let owed = bite.outstanding_bleed(fight);
                    fight.cast(bleed, target);
                    let base = bite.rolled_bleed(fight, owed, damage);
                    if let Some(bound) = fight.agent.mongoose_bite.as_mut() {
                        bound.bleed = (base, 1.0);
                    }
                }
            }
            HunterSpell::LaceratingStrikes => Self::mongoose(fight).apply_bleed(fight),
            HunterSpell::StriderKick => {
                let speed = fight.agent.strider_kick_speed;
                melee::strider_kick(fight, spell, target, speed);
            }
            HunterSpell::WingClip => {
                let base = fight.agent.wing_clip_damage;
                melee::wing_clip(fight, spell, target, base);
            }
            HunterSpell::FreezingTrap => {}
            HunterSpell::ImmolationTrap => {
                let trap = fight
                    .agent
                    .immolation_trap
                    .expect("Immolation Trap is bound");
                melee::immolation_trap(fight, spell, target, trap);
            }
            HunterSpell::ExplosiveTrap => fight
                .agent
                .explosive_trap
                .expect("Explosive Trap is bound")
                .apply(fight, spell, target),
            HunterSpell::Volley => fight
                .agent
                .volley
                .expect("Volley is bound")
                .apply(fight, spell),
        }
    }

    fn replace_mh_swing(fight: &mut Fight<Self>, swing: SpellId) -> SpellId {
        match fight.agent.raptor_strike {
            Some(raptor) => raptor.replace(fight, swing),
            None => swing,
        }
    }

    fn cast_time(fight: &Fight<Self>, spell: SpellId, behavior: HunterSpell) -> Option<i64> {
        match behavior {
            HunterSpell::AimedShot
            | HunterSpell::SniperShot
            | HunterSpell::MultiShot
            | HunterSpell::ArcaneShot
            | HunterSpell::SerpentSting => shots::cast_time(fight, spell),
            _ => None,
        }
    }

    fn extra_cast_condition(fight: &Fight<Self>, spell: SpellId, behavior: HunterSpell) -> bool {
        match behavior {
            HunterSpell::AspectOfTheHawk => !fight.aura(Self::aspect(fight).aura).active,
            HunterSpell::RapidFire => !fight.aura(Self::rapid_fire(fight).aura).active,
            HunterSpell::AspectOfTheBeast => !fight.aura(Self::beast(fight).aura).active,
            HunterSpell::RaptorStrikeQueue => Self::raptor(fight).can_queue(fight),
            HunterSpell::MongooseBite => fight.aura(Self::mongoose(fight).window).active,
            // Go: each pet ability needs the pet enabled; Swipe also needs three targets.
            HunterSpell::PetAbility(index) => {
                let enabled = fight.active_pet(fight.caster(spell)).enabled;
                match fight.agent.pet_abilities[index] {
                    PetAbility::Swipe {
                        min_targets,
                        cleave,
                    } => enabled && cleave.is_some() && fight.targets.len() >= min_targets,
                    PetAbility::DustCloud => {
                        let (aura, _) = fight.agent.dust_cloud.expect("Dust Cloud is bound");
                        // Go `DustCloudYieldsToSunderAndExpose` (mythicsim patch 110): the pet
                        // holds Dust Cloud while Sunder Armor or Expose Armor is on the target.
                        if DUST_CLOUD_YIELDS_TO_SUNDER_AND_EXPOSE
                            && fight
                                .exclusive_category_any_active(Side::Target, SUNDER_ARMOR_CATEGORY)
                        {
                            return false;
                        }
                        enabled && !fight.aura(aura).active
                    }
                    PetAbility::FuriousHowl { .. } => {
                        let howl = fight.agent.furious_howl.expect("Furious Howl is bound");
                        enabled && !fight.aura(howl.pet_aura).active
                    }
                    _ => enabled,
                }
            }
            _ => true,
        }
    }

    fn should_activate(fight: &Fight<Self>, _spell: SpellId, behavior: HunterSpell) -> bool {
        match behavior {
            HunterSpell::RapidFire => !fight.aura(Self::rapid_fire(fight).aura).active,
            // Go: only worth it with a shot whose cooldown is not ready.
            HunterSpell::RenatakisCharm => fight.agent.renatakis_shots.iter().any(|&shot| {
                fight.spells[shot]
                    .cd
                    .is_some_and(|(timer, _)| fight.timers[timer] > fight.now)
            }),
            _ => true,
        }
    }

    fn on_travel(
        fight: &mut Fight<Self>,
        spell: SpellId,
        result: SpellResult,
        behavior: HunterSpell,
    ) {
        match behavior {
            HunterSpell::SerpentSting => {
                let sting = fight
                    .agent
                    .serpent_sting
                    .clone()
                    .expect("Serpent Sting is bound");
                if let Some(attack_power) = serpent_sting::on_travel(fight, spell, result, &sting) {
                    if let Some(bound) = fight.agent.serpent_sting.as_mut() {
                        bound.snapshotted(result.target, attack_power);
                    }
                }
            }
            _ => fight.deal_damage(spell, result, false),
        }
    }

    fn on_dot_tick(fight: &mut Fight<Self>, dot: DotId, behavior: HunterSpell) {
        match behavior {
            HunterSpell::SerpentSting => {
                let sting = fight
                    .agent
                    .serpent_sting
                    .clone()
                    .expect("Serpent Sting is bound");
                sting.tick(fight, dot);
            }
            HunterSpell::Hawk => SummonHawk::tick(fight, dot),
            HunterSpell::LaceratingStrikes => Self::mongoose(fight).bleed_tick(fight, dot),
            HunterSpell::ImmolationTrap => fight.snapshot_dot_tick(dot),
            HunterSpell::ExplosiveTrap => fight
                .agent
                .explosive_trap
                .expect("Explosive Trap is bound")
                .tick(fight, dot),
            HunterSpell::Volley => fight
                .agent
                .volley
                .expect("Volley is bound")
                .tick(fight, dot),
            HunterSpell::PetAbility(index) => match fight.agent.pet_abilities[index] {
                PetAbility::Bleed { outcome, .. } | PetAbility::ScorpidPoison { outcome, .. } => {
                    hunter_pet::bleed_tick(fight, dot, outcome)
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// Go hunter.go wraps the main hand auto's `ApplyEffects` with a line for a swing that
    /// fired later than an uncontested rotation would have.
    fn before_melee_auto(fight: &mut Fight<Self>, spell: SpellId, hand: Hand) {
        let delay = fight.autos.mh.pending_swing_delay;
        if fight.log.is_none()
            || hand != Hand::Main
            || fight.spells[spell].caster != Side::Player
            || fight.spells[spell].id.tag != 1
            || delay <= NS_PER_MILLISECOND
        {
            return;
        }
        let line = format!(
            "{} delayed by {}, was ready at {}",
            action_string(&fight.spells[spell].id),
            go_string(delay),
            go_string(fight.now - delay)
        );
        fight.player_log(&line);
    }

    fn on_gain(fight: &mut Fight<Self>, aura: AuraRef, kind: HunterAura) {
        match kind {
            HunterAura::AspectOfTheHawk => Self::aspect(fight).on_gain(fight),
            HunterAura::QuickShots => Self::aspect(fight).quick_shots_changed(fight, true),
            HunterAura::RapidFire => Self::rapid_fire(fight).changed(fight, true),
            HunterAura::Intimidation => fight
                .agent
                .pet_auras
                .clone()
                .intimidation_changed(fight, true),
            HunterAura::BestialWrath => fight
                .agent
                .pet_auras
                .clone()
                .bestial_wrath_changed(fight, true),
            HunterAura::FrenzyEffect => fight.agent.pet_auras.clone().frenzy_changed(fight, true),
            HunterAura::DustCloud => {
                let (_, armor) = fight.agent.dust_cloud.expect("Dust Cloud is bound");
                fight.add_target_armor(aura.side, armor);
            }
            HunterAura::FuriousHowl => fight
                .agent
                .furious_howl
                .expect("Furious Howl is bound")
                .pet_changed(fight, true),
            HunterAura::AspectOfTheBeast => Self::beast(fight).on_gain(fight),
            HunterAura::QuickStrikes => Self::beast(fight).quick_shots_changed(fight, true),
            HunterAura::RaptorStrikeQueued => RaptorStrike::queue_changed(fight, true),
            HunterAura::Resourcefulness => {
                let (_, regen) = fight
                    .agent
                    .resourcefulness
                    .expect("Resourcefulness is bound");
                regen.changed(fight, true);
            }
            HunterAura::RapidRecuperation => {
                let (_, regen) = fight
                    .agent
                    .rapid_recuperation
                    .expect("Rapid Recuperation is bound");
                regen.changed(fight, true);
            }
            HunterAura::FrenzyTrigger
            | HunterAura::ResourcefulnessTrigger
            | HunterAura::RapidRecuperationTrigger
            | HunterAura::ManaProc(_)
            | HunterAura::ExposePrey => {}
        }
    }

    fn on_expire(fight: &mut Fight<Self>, aura: AuraRef, kind: HunterAura) {
        match kind {
            HunterAura::AspectOfTheHawk => Self::aspect(fight).on_expire(fight),
            HunterAura::QuickShots => Self::aspect(fight).quick_shots_changed(fight, false),
            HunterAura::RapidFire => Self::rapid_fire(fight).changed(fight, false),
            HunterAura::Intimidation => fight
                .agent
                .pet_auras
                .clone()
                .intimidation_changed(fight, false),
            HunterAura::BestialWrath => fight
                .agent
                .pet_auras
                .clone()
                .bestial_wrath_changed(fight, false),
            HunterAura::FrenzyEffect => fight.agent.pet_auras.clone().frenzy_changed(fight, false),
            HunterAura::DustCloud => {
                let (_, armor) = fight.agent.dust_cloud.expect("Dust Cloud is bound");
                fight.add_target_armor(aura.side, -armor);
            }
            HunterAura::FuriousHowl => fight
                .agent
                .furious_howl
                .expect("Furious Howl is bound")
                .pet_changed(fight, false),
            HunterAura::AspectOfTheBeast => Self::beast(fight).on_expire(fight),
            HunterAura::QuickStrikes => Self::beast(fight).quick_shots_changed(fight, false),
            HunterAura::RaptorStrikeQueued => RaptorStrike::queue_changed(fight, false),
            HunterAura::Resourcefulness => {
                let (_, regen) = fight
                    .agent
                    .resourcefulness
                    .expect("Resourcefulness is bound");
                regen.changed(fight, false);
            }
            HunterAura::RapidRecuperation => {
                let (_, regen) = fight
                    .agent
                    .rapid_recuperation
                    .expect("Rapid Recuperation is bound");
                regen.changed(fight, false);
            }
            HunterAura::FrenzyTrigger
            | HunterAura::ResourcefulnessTrigger
            | HunterAura::RapidRecuperationTrigger
            | HunterAura::ManaProc(_)
            | HunterAura::ExposePrey => {}
        }
    }

    fn on_spell_hit_dealt(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: HunterAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        match kind {
            HunterAura::AspectOfTheHawk => {
                Self::aspect(fight).on_spell_hit_dealt(fight, spell, result)
            }
            HunterAura::AspectOfTheBeast => {
                Self::beast(fight).on_spell_hit_dealt(fight, spell, result)
            }
            HunterAura::ResourcefulnessTrigger => {
                let (proc, _) = fight
                    .agent
                    .resourcefulness
                    .expect("Resourcefulness is bound");
                let hears = Rc::clone(&fight.agent.resourcefulness_spells);
                proc.resourcefulness_hit(fight, spell, result, &hears);
            }
            HunterAura::ExposePrey => {
                let (proc, _) = fight.agent.expose_prey.expect("Expose Prey is bound");
                proc.expose_prey_hit(fight, spell, result);
            }
            HunterAura::ManaProc(index) => {
                let proc = fight.agent.mana_procs[index].clone();
                proc.on_hit(fight, spell, result);
            }
            HunterAura::RapidRecuperationTrigger => {
                let (proc, _) = fight
                    .agent
                    .rapid_recuperation
                    .expect("Rapid Recuperation is bound");
                let sting = matches!(
                    fight.spells[spell].behavior,
                    crate::core::fight::SpellBehavior::Class(HunterSpell::SerpentSting)
                );
                proc.rapid_recuperation_hit(fight, spell, result, sting);
            }
            HunterAura::Intimidation => fight
                .agent
                .pet_auras
                .clone()
                .intimidation_hit(fight, result),
            HunterAura::FrenzyTrigger => fight
                .agent
                .pet_auras
                .clone()
                .frenzy_hit(fight, spell, result),
            _ => {}
        }
    }

    fn on_delayed_proc(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: HunterAura,
        _spell: SpellId,
        result: SpellResult,
    ) {
        match kind {
            HunterAura::FrenzyTrigger => fight.agent.pet_auras.clone().frenzy_proc(fight),
            HunterAura::ResourcefulnessTrigger => {
                let (proc, _) = fight
                    .agent
                    .resourcefulness
                    .expect("Resourcefulness is bound");
                fight.activate_aura(proc.aura);
            }
            HunterAura::ExposePrey => {
                let (proc, marked) = fight.agent.expose_prey.expect("Expose Prey is bound");
                proc.expose_prey_proc(fight, &result, marked);
            }
            HunterAura::ManaProc(index) => {
                let proc = fight.agent.mana_procs[index].clone();
                proc.restore(fight);
            }
            HunterAura::RapidRecuperationTrigger => {
                let (proc, _) = fight
                    .agent
                    .rapid_recuperation
                    .expect("Rapid Recuperation is bound");
                if result.landed() {
                    fight.activate_aura(proc.aura);
                }
            }
            _ => {}
        }
    }

    fn pet_rotation(fight: &mut Fight<Self>, pet: Side) {
        if let Some(ai) = fight.agent.pet_ai.clone().filter(|ai| ai.pet == pet) {
            ai.rotation(fight);
        }
    }
}
