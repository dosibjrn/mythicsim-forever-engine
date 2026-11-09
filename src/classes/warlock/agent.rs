//! The Warlock class agent for the fight runtime: class spells and auras by name, and the
//! hooks that dispatch to each spell's and talent's module.

use std::rc::Rc;

use crate::{
    contracts::prepared_v2::{Effect, PreparedV2, Spell as ExportedSpell},
    core::fight::{
        healing::Healing, school_damage_index, Agent, AuraRef, DotId, Fight, Side, SpellBehavior,
        SpellId, SpellResult, MAX_TARGETS, PRIORITY_REGEN,
    },
};

use super::{
    pets::{self, DemonAi},
    spells::{
        bane_of_agony::{self, BaneOfAgony},
        bane_of_doom, bind_snapshot_dot,
        conflagrate::Conflagrate,
        corruption,
        curse_of_recklessness::{self, CurseOfRecklessness},
        curse_of_the_elements::{self, CurseOfTheElements},
        drain_life::DrainLife,
        find_spell,
        hellfire::{self, Hellfire},
        immolate, incinerate,
        life_tap::{self, LifeTap},
        rain_of_fire::{self, RainOfFire},
        searing_pain, shadow_bolt, shadowburn,
        siphon_life::SiphonLife,
        soul_fire,
    },
    talents::{
        bane_of_havoc::{self, BaneOfHavoc},
        decimation::{self, Decimation},
        demonic_brand::{self, DemonicBrand},
        improved_shadow_bolt::{self, ImprovedShadowBolt},
        nightfall::{self, Nightfall},
        shadow_and_flame::{self, ShadowAndFlame},
        wrack::{self, Wrack},
    },
};

/// What a Warlock spell does when its effects apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WarlockSpell {
    ShadowBolt,
    Immolate,
    /// Immolate's related dot spell, which only ticks.
    ImmolateDot,
    Corruption,
    BaneOfAgony,
    CurseOfTheElements,
    CurseOfRecklessness,
    LifeTap,
    Conflagrate,
    Shadowburn,
    SearingPain,
    SoulFire,
    AmplifyCurse,
    SiphonLife,
    BaneOfDoom,
    DrainLife,
    Incinerate,
    Wrack,
    /// Hellfire's channel.
    Hellfire,
    /// Rain of Fire's channel.
    RainOfFire,
    /// The spell each Rain of Fire period casts.
    RainOfFireTick,
    BaneOfHavoc,
    DeathCoil,
    /// The Succubus's Lash of Pain.
    LashOfPain,
    /// The demon's Demonic Brand hit.
    DemonicBrand,
    /// The Imp's Firebolt.
    Firebolt,
    /// The Black Book's use under Forever's rules.
    BlackBook,
}

/// Class auras with Rust behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WarlockAura {
    CurseOfTheElements,
    CurseOfRecklessness,
    ImprovedShadowBoltTrigger,
    ShadowAndFlameTrigger,
    ShadowAndFlame,
    NightfallTrigger,
    ShadowTrance,
    DecimationTrigger,
    Decimation,
    DemonicBrandTrigger,
    /// The demon's aura that spends the brand's charges.
    DemonicBrandConsumer,
    /// The Voidwalker's sacrifice.
    FelEnergy,
    /// Bane of Havoc on a target.
    HavocBane,
    /// The warlock's permanent listener that copies damage onto the baned target.
    HavocCopy,
    /// Blessing of the Black Book on the demon.
    BlackBook,
}

/// [`Agent::on_periodic`] tags of Warlock periodic actions.
const FEL_ENERGY_TICK: u32 = 1;

/// Warlock state that Go keeps in the `Warlock` struct and its closures.
#[derive(Default)]
pub(crate) struct WarlockAgent {
    /// Immolate's dot, which Immolate applies and Conflagrate consumes.
    immolate_dot: Option<DotId>,
    corruption_dot: Option<DotId>,
    pub(crate) bane_of_agony: Option<BaneOfAgony>,
    /// Go agony.go `wholeTickAmp`: the Amplify Curse factor each target's Bane of Agony snapshot
    /// spent, indexed by the target's dot (patch 177).
    pub(crate) agony_whole_tick_amp: Vec<f64>,
    siphon_life: Option<SiphonLife>,
    bane_of_doom_dot: Option<DotId>,
    drain_life: Option<DrainLife>,
    wrack: Option<Wrack>,
    pub(crate) hellfire: Option<Hellfire>,
    rain_of_fire: Option<RainOfFire>,
    /// Death Coil's base, its healing spell and the warlock's healing modifiers.
    death_coil: Option<(f64, SpellId, Healing)>,
    pub(crate) bane_of_havoc: Option<BaneOfHavoc>,
    /// Go's `havocTarget`: the target that holds Bane of Havoc.
    pub(crate) havoc_target: Option<Side>,
    /// Incinerate's bonus on a target burning with Immolate.
    incinerate_bonus: f64,
    /// Go `currentActiveBane`: the bane each target holds, by position.
    pub(crate) bane_slot: [Option<AuraRef>; MAX_TARGETS],
    curse_of_the_elements: Option<Rc<CurseOfTheElements>>,
    curse_of_recklessness: Option<CurseOfRecklessness>,
    /// Go `currentActiveCurse`: the curse each target holds, by position.
    pub(crate) curse_slot: [Option<AuraRef>; MAX_TARGETS],
    life_tap: Option<LifeTap>,
    conflagrate: Option<Rc<Conflagrate>>,
    improved_shadow_bolt: Option<Rc<ImprovedShadowBolt>>,
    shadow_and_flame: Option<Rc<ShadowAndFlame>>,
    amplify_curse: Option<AuraRef>,
    nightfall: Option<Rc<Nightfall>>,
    demon: Option<Rc<DemonAi>>,
    lash_of_pain_base: f64,
    /// Firebolt's damage roll bounds.
    firebolt: (f64, f64),
    /// The firebolt row's average and variance, which Go `Effect.Roll` reads.
    firebolt_roll: Option<(f64, f64)>,
    decimation: Option<Rc<Decimation>>,
    demonic_brand: Option<Rc<DemonicBrand>>,
    /// The Black Book's aura on the summoned demon and its damage done multiplier.
    black_book: Option<(AuraRef, f64)>,
    /// Fel Energy's period, share of maximum mana and mana metrics.
    fel_energy: Option<(i64, f64, usize)>,
}

/// Aura labels claimed by implemented class effects, as (unit, label, kind).
fn class_auras(prepared: &PreparedV2) -> Vec<(&'static str, String, WarlockAura)> {
    let mut auras = Vec::new();
    for effect in &prepared.effects {
        match effect {
            Effect::CurseOfTheElements { aura, .. } => {
                auras.push(("target", aura.clone(), WarlockAura::CurseOfTheElements))
            }
            Effect::CurseOfRecklessness { aura, .. } => {
                auras.push(("target", aura.clone(), WarlockAura::CurseOfRecklessness))
            }
            Effect::ImprovedShadowBolt { trigger_aura, .. } => auras.push((
                "player",
                trigger_aura.clone(),
                WarlockAura::ImprovedShadowBoltTrigger,
            )),
            Effect::ShadowAndFlame {
                trigger_aura,
                shadow_aura,
                fire_aura,
                ..
            } => {
                auras.push((
                    "player",
                    trigger_aura.clone(),
                    WarlockAura::ShadowAndFlameTrigger,
                ));
                auras.push(("player", shadow_aura.clone(), WarlockAura::ShadowAndFlame));
                auras.push(("player", fire_aura.clone(), WarlockAura::ShadowAndFlame));
            }
            Effect::Nightfall {
                trigger_aura, aura, ..
            } => {
                auras.push((
                    "player",
                    trigger_aura.clone(),
                    WarlockAura::NightfallTrigger,
                ));
                auras.push(("player", aura.clone(), WarlockAura::ShadowTrance));
            }
            Effect::FelEnergy { aura, .. } => {
                auras.push(("player", aura.clone(), WarlockAura::FelEnergy))
            }
            Effect::BlackBook { aura, .. } => {
                auras.push(("pet", aura.clone(), WarlockAura::BlackBook))
            }
            Effect::BaneOfHavoc {
                aura, copy_aura, ..
            } => {
                auras.push(("target", aura.clone(), WarlockAura::HavocBane));
                auras.push(("player", copy_aura.clone(), WarlockAura::HavocCopy));
            }
            Effect::Decimation {
                trigger_aura, aura, ..
            } => {
                auras.push((
                    "player",
                    trigger_aura.clone(),
                    WarlockAura::DecimationTrigger,
                ));
                auras.push(("player", aura.clone(), WarlockAura::Decimation));
            }
            Effect::DemonicBrand {
                trigger_aura,
                consumer_aura,
                ..
            } => {
                auras.push((
                    "player",
                    trigger_aura.clone(),
                    WarlockAura::DemonicBrandTrigger,
                ));
                if let Some(consumer) = consumer_aura {
                    auras.push(("pet", consumer.clone(), WarlockAura::DemonicBrandConsumer));
                }
            }
            _ => {}
        }
    }
    auras
}

impl WarlockAgent {
    /// The class behavior of an exported spell, if Rust implements it.
    pub(crate) fn spell(spell: &ExportedSpell) -> Option<WarlockSpell> {
        let id = spell.action_id.clone().unwrap_or_default();
        if id.item_id == 19337 && id.tag == 0 {
            return Some(WarlockSpell::BlackBook);
        }
        let damage = spell.damage_effect.is_some();
        let dot = spell.dot.is_some();
        match spell.class_spell.as_deref()? {
            "shadow_bolt" if damage => Some(WarlockSpell::ShadowBolt),
            "immolate" if damage && spell.related_dot_spell.is_some() => {
                Some(WarlockSpell::Immolate)
            }
            "immolate_dot" if dot => Some(WarlockSpell::ImmolateDot),
            "corruption" if dot => Some(WarlockSpell::Corruption),
            "bane_of_agony" if dot => Some(WarlockSpell::BaneOfAgony),
            "curse_of_the_elements" => Some(WarlockSpell::CurseOfTheElements),
            "curse_of_recklessness" => Some(WarlockSpell::CurseOfRecklessness),
            "life_tap" => Some(WarlockSpell::LifeTap),
            "conflagrate" if damage => Some(WarlockSpell::Conflagrate),
            "shadowburn" if damage => Some(WarlockSpell::Shadowburn),
            "searing_pain" if damage => Some(WarlockSpell::SearingPain),
            "soul_fire" if damage => Some(WarlockSpell::SoulFire),
            "amplify_curse" => Some(WarlockSpell::AmplifyCurse),
            "siphon_life" if dot => Some(WarlockSpell::SiphonLife),
            "bane_of_doom" if dot => Some(WarlockSpell::BaneOfDoom),
            "drain_life" if dot => Some(WarlockSpell::DrainLife),
            "incinerate" if damage => Some(WarlockSpell::Incinerate),
            "wrack" if dot => Some(WarlockSpell::Wrack),
            "hellfire" if dot => Some(WarlockSpell::Hellfire),
            "rain_of_fire" if dot => Some(WarlockSpell::RainOfFire),
            "rain_of_fire" => Some(WarlockSpell::RainOfFireTick),
            "death_coil" if !dot => Some(WarlockSpell::DeathCoil),
            "bane_of_havoc" if !damage && !dot => Some(WarlockSpell::BaneOfHavoc),
            "succubus_lash_of_pain" => Some(WarlockSpell::LashOfPain),
            "demonic_brand" => Some(WarlockSpell::DemonicBrand),
            "imp_firebolt" => Some(WarlockSpell::Firebolt),
            _ => None,
        }
    }

    /// Build a fight for a prepared input that passed the coverage gate.
    pub(crate) fn fight(
        prepared: &PreparedV2,
        rng_mode: crate::core::rng::RngMode,
    ) -> Result<Fight<WarlockAgent>, String> {
        let auras = class_auras(prepared);
        let mut fight = Fight::new(
            prepared,
            rng_mode,
            WarlockAgent::default(),
            WarlockAgent::spell,
            |unit, label| {
                auras
                    .iter()
                    .find(|(side, name, _)| *side == unit && name == label)
                    .map(|(_, _, kind)| *kind)
            },
        )?;
        let spirit = prepared
            .player
            .stats
            .get("Spirit")
            .copied()
            .ok_or("prepared stats lack Spirit")?;
        // The simulated pet that is the summoned demon.
        let demon_side = prepared.effects.iter().find_map(|effect| match effect {
            Effect::WarlockPet { pet, .. } => fight.pet_side(pet),
            _ => None,
        });
        for effect in &prepared.effects {
            match effect {
                Effect::Immolate {
                    spell_id,
                    tick_base,
                    tick_can_crit,
                } => {
                    let dot = bind_snapshot_dot(&mut fight, *spell_id, *tick_base, *tick_can_crit)?;
                    fight.agent.immolate_dot = Some(dot);
                }
                Effect::Corruption {
                    spell_id,
                    tick_base,
                    tick_can_crit,
                } => {
                    let dot = bind_snapshot_dot(&mut fight, *spell_id, *tick_base, *tick_can_crit)?;
                    fight.agent.corruption_dot = Some(dot);
                }
                Effect::SiphonLife {
                    spell_id,
                    tick_base,
                    tick_can_crit,
                    self_healing_multiplier,
                } => {
                    let dot = bind_snapshot_dot(&mut fight, *spell_id, *tick_base, *tick_can_crit)?;
                    let metrics = fight.spells[find_spell(&fight, *spell_id)?]
                        .health_metrics
                        .ok_or("Siphon Life has no health metrics")?;
                    fight.agent.siphon_life =
                        Some(SiphonLife::new(dot, *self_healing_multiplier, metrics));
                }
                Effect::DrainLife {
                    spell_id,
                    tick_base,
                    tick_can_crit,
                    soul_siphon,
                    self_healing_multiplier,
                } => {
                    let dot = bind_snapshot_dot(&mut fight, *spell_id, *tick_base, *tick_can_crit)?;
                    let metrics = fight.spells[find_spell(&fight, *spell_id)?]
                        .health_metrics
                        .ok_or("Drain Life has no health metrics")?;
                    fight.agent.drain_life = Some(DrainLife::new(
                        dot,
                        *soul_siphon,
                        *self_healing_multiplier,
                        metrics,
                    ));
                }
                Effect::Wrack {
                    spell_id,
                    tick_base,
                    tick_can_crit,
                    soul_siphon,
                    dot_bonus,
                    dot_spells,
                } => {
                    let dot = bind_snapshot_dot(&mut fight, *spell_id, *tick_base, *tick_can_crit)?;
                    let bound = wrack::bind(&mut fight, dot, *soul_siphon, dot_spells, *dot_bonus);
                    fight.agent.wrack = Some(bound);
                }
                Effect::Hellfire {
                    spell_id,
                    tick_base,
                    tick_can_crit,
                } => {
                    find_spell(&fight, *spell_id)?;
                    fight.agent.hellfire = Some(hellfire::bind(*tick_base, *tick_can_crit));
                }
                Effect::RainOfFire {
                    spell_id,
                    tick_spell_id,
                    tick_base,
                } => {
                    let bound = rain_of_fire::bind(&fight, *spell_id, *tick_spell_id, *tick_base)?;
                    fight.agent.rain_of_fire = Some(bound);
                }
                Effect::DeathCoil {
                    base_damage,
                    healing_dealt_multiplier,
                    healing_taken_multiplier,
                    table_healing_dealt_multiplier,
                    healing_power,
                } => {
                    let damage_spell = fight.spells.iter().position(|spell| {
                        matches!(
                            spell.behavior,
                            SpellBehavior::Class(WarlockSpell::DeathCoil)
                        )
                    });
                    if let Some(damage_spell) = damage_spell {
                        let id = fight.spells[damage_spell].id.spell_id;
                        let heal = fight
                            .spells
                            .iter()
                            .position(|spell| spell.id.spell_id == id && spell.id.tag == 1)
                            .ok_or("Death Coil has no healing spell")?;
                        fight.agent.death_coil = Some((
                            *base_damage,
                            heal,
                            Healing {
                                dealt_multiplier: *healing_dealt_multiplier,
                                taken_multiplier: *healing_taken_multiplier,
                                table_multiplier: *table_healing_dealt_multiplier,
                                healing_power: *healing_power,
                            },
                        ));
                    }
                }
                Effect::BaneOfHavoc {
                    spell_id,
                    aura,
                    share,
                    ..
                } => {
                    let bound = bane_of_havoc::bind(&fight, *spell_id, aura, *share)?;
                    fight.agent.bane_of_havoc = Some(bound);
                }
                Effect::Incinerate { immolate_bonus } => {
                    fight.agent.incinerate_bonus = *immolate_bonus;
                }
                Effect::BaneOfDoom {
                    spell_id,
                    tick_base,
                    tick_can_crit,
                } => {
                    let dot = bind_snapshot_dot(&mut fight, *spell_id, *tick_base, *tick_can_crit)?;
                    fight.agent.bane_of_doom_dot = Some(dot);
                }
                Effect::BaneOfAgony {
                    spell_id,
                    tick_base,
                    tick_can_crit,
                    ramp_share,
                    ramp_every_ticks,
                    ..
                } => {
                    if *ramp_every_ticks <= 0 {
                        return Err("Bane of Agony ramps every nonpositive tick count".into());
                    }
                    let dot = bind_snapshot_dot(&mut fight, *spell_id, *tick_base, *tick_can_crit)?;
                    fight.agent.bane_of_agony = Some(BaneOfAgony::new(
                        dot,
                        *tick_base,
                        *ramp_share,
                        *ramp_every_ticks,
                    ));
                }
                Effect::CurseOfRecklessness {
                    aura, armor_delta, ..
                } => {
                    let bound = curse_of_recklessness::bind(&fight, aura, *armor_delta)?;
                    fight.agent.curse_of_recklessness = Some(bound);
                }
                Effect::CurseOfTheElements {
                    aura,
                    resistance_delta,
                    school_damage_taken_multiplier,
                    blocked,
                    ..
                } => {
                    let bound = curse_of_the_elements::bind(
                        &fight,
                        aura,
                        resistance_delta,
                        school_damage_taken_multiplier,
                        *blocked,
                    )?;
                    fight.agent.curse_of_the_elements = Some(Rc::new(bound));
                }
                Effect::LifeTap {
                    spell_id,
                    base_amount,
                    mana_multiplier,
                    pet_mana_share,
                    no_threat,
                } => {
                    let bound = life_tap::bind(
                        &mut fight,
                        *spell_id,
                        *base_amount,
                        *mana_multiplier,
                        spirit,
                        *pet_mana_share,
                        demon_side,
                        *no_threat,
                    );
                    fight.agent.life_tap = Some(bound);
                }
                Effect::ImprovedShadowBolt {
                    aura,
                    multiplier,
                    trigger_spells,
                    ..
                } => {
                    let bound =
                        improved_shadow_bolt::bind(&mut fight, aura, *multiplier, trigger_spells)?;
                    fight.agent.improved_shadow_bolt = Some(Rc::new(bound));
                }
                Effect::ShadowAndFlame {
                    shadow_aura,
                    fire_aura,
                    multiplier,
                    trigger_spells,
                    shadow_spells,
                    ..
                } => {
                    let bound = shadow_and_flame::bind(
                        &fight,
                        shadow_aura,
                        fire_aura,
                        *multiplier,
                        trigger_spells,
                        shadow_spells,
                    )?;
                    fight.agent.shadow_and_flame = Some(Rc::new(bound));
                }
                Effect::AmplifyCurse { aura, .. } => {
                    fight.agent.amplify_curse = Some(fight.player_aura(aura)?);
                }
                Effect::Nightfall {
                    aura,
                    proc_chance,
                    rng_label,
                    trigger_spells,
                    consume_spells,
                    modded_spells,
                    cast_time_percent,
                    ..
                } => {
                    let bound = nightfall::bind(
                        &mut fight,
                        aura,
                        *proc_chance,
                        rng_label,
                        trigger_spells,
                        consume_spells,
                        modded_spells,
                        *cast_time_percent,
                    )?;
                    fight.agent.nightfall = Some(Rc::new(bound));
                }
                Effect::WarlockPet {
                    min_mana,
                    autocast_spells,
                    wait_ns,
                    ..
                } => {
                    let demon = demon_side.ok_or("the demon is not simulated")?;
                    let bound = pets::bind(&fight, demon, autocast_spells, *min_mana, *wait_ns)?;
                    fight.agent.demon = Some(Rc::new(bound));
                }
                Effect::LashOfPain { base_damage } => fight.agent.lash_of_pain_base = *base_damage,
                // Without a simulated demon the use never casts: it waits for a demon out.
                Effect::BlackBook {
                    aura,
                    damage_multiplier,
                    ..
                } => {
                    if let Some(demon) = demon_side {
                        let index = fight.trackers[demon.index()]
                            .find(aura)
                            .ok_or_else(|| format!("demon aura {aura} is not registered"))?;
                        let aura = AuraRef { side: demon, index };
                        fight.agent.black_book = Some((aura, *damage_multiplier));
                    }
                }
                Effect::Firebolt {
                    min_damage,
                    max_damage,
                    average,
                    variance,
                } => {
                    fight.agent.firebolt = (*min_damage, *max_damage);
                    fight.agent.firebolt_roll = average.zip(*variance);
                }
                Effect::FelEnergy {
                    spell_id,
                    mana_fraction,
                    period_ns,
                    ..
                } => {
                    let metrics = fight.new_mana_metrics(crate::contracts::prepared_v2::ActionId {
                        spell_id: *spell_id,
                        ..Default::default()
                    });
                    fight.agent.fel_energy = Some((*period_ns, *mana_fraction, metrics));
                }
                Effect::Decimation {
                    aura,
                    execute_phase,
                    trigger_spells,
                    damage_spells,
                    damage_done_flat,
                    cast_spells,
                    cast_time_percent,
                    ..
                } => {
                    let bound = decimation::bind(
                        &mut fight,
                        aura,
                        *execute_phase,
                        trigger_spells,
                        damage_spells,
                        *damage_done_flat,
                        cast_spells,
                        *cast_time_percent,
                    )?;
                    fight.agent.decimation = Some(Rc::new(bound));
                }
                Effect::DemonicBrand {
                    target_aura,
                    charges,
                    trigger_spells,
                    pet,
                    marker_aura,
                    brand_spell,
                    min_damage,
                    max_damage,
                    spell_power_coefficient,
                    school_power_stat,
                    ..
                } => {
                    let demon = match (pet, marker_aura, brand_spell, school_power_stat) {
                        (None, ..) => None,
                        (Some(_), Some(marker_aura), Some(brand_spell), Some(stat)) => {
                            Some(demonic_brand::DemonConfig {
                                marker_aura,
                                brand_spell: *brand_spell,
                                min_damage: *min_damage,
                                max_damage: *max_damage,
                                coefficient: *spell_power_coefficient,
                                school: school_damage_index(stat).ok_or_else(|| {
                                    format!("{stat} is not a school spell damage stat")
                                })?,
                            })
                        }
                        _ => return Err("Demonic Brand's demon half is incomplete".into()),
                    };
                    let bound = demonic_brand::bind(
                        &fight,
                        target_aura,
                        *charges,
                        trigger_spells,
                        demon,
                        demon_side,
                    )?;
                    fight.agent.demonic_brand = Some(Rc::new(bound));
                }
                _ => {}
            }
        }
        // Bane of Agony spends Amplify Curse, which is bound above.
        for effect in &prepared.effects {
            if let Effect::BaneOfAgony {
                amplify: Some(factor),
                ..
            } = effect
            {
                if let (Some(agony), Some(aura)) = (
                    fight.agent.bane_of_agony.as_mut(),
                    fight.agent.amplify_curse,
                ) {
                    agony.amplify = Some((aura, *factor));
                }
            }
        }
        // Conflagrate reads Immolate's dot, which is bound above.
        for effect in &prepared.effects {
            if let Effect::Conflagrate {
                spell_id,
                keep_immolate_chance,
                rng_label,
            } = effect
            {
                find_spell(&fight, *spell_id)?;
                let immolate = fight
                    .agent
                    .immolate_dot
                    .ok_or("Conflagrate needs Immolate's dot")?;
                fight.agent.conflagrate = Some(Rc::new(Conflagrate::new(
                    immolate,
                    *keep_immolate_chance,
                    rng_label,
                )));
            }
        }
        Ok(fight)
    }
}

impl Agent for WarlockAgent {
    type Spell = WarlockSpell;
    type Aura = WarlockAura;

    fn apply_effects(
        fight: &mut Fight<Self>,
        spell: SpellId,
        target: Side,
        behavior: WarlockSpell,
    ) {
        match behavior {
            // items.go: the use lands the blessing on the demon out.
            WarlockSpell::BlackBook => {
                let (aura, _) = fight.agent.black_book.expect("The Black Book is bound");
                fight.activate_aura(aura);
            }
            WarlockSpell::ShadowBolt => shadow_bolt::apply(fight, spell, target),
            WarlockSpell::SoulFire => soul_fire::apply(fight, spell, target),
            WarlockSpell::Shadowburn => shadowburn::apply(fight, spell, target),
            WarlockSpell::SearingPain => searing_pain::apply(fight, spell, target),
            WarlockSpell::Immolate => {
                let dot = fight.agent.immolate_dot.expect("Immolate is bound");
                immolate::apply(fight, spell, target, dot);
            }
            WarlockSpell::Corruption => {
                let dot = fight.agent.corruption_dot.expect("Corruption is bound");
                corruption::apply(fight, spell, target, dot);
            }
            WarlockSpell::BaneOfAgony => bane_of_agony::apply(fight, spell, target),
            WarlockSpell::SiphonLife => {
                let dot = fight.agent.siphon_life.expect("Siphon Life is bound").dot;
                corruption::apply(fight, spell, target, dot);
            }
            WarlockSpell::BaneOfDoom => {
                let dot = fight.agent.bane_of_doom_dot.expect("Bane of Doom is bound");
                bane_of_doom::apply(fight, spell, target, dot);
            }
            WarlockSpell::DrainLife => {
                let dot = fight.agent.drain_life.expect("Drain Life is bound").dot;
                corruption::apply(fight, spell, target, dot);
            }
            WarlockSpell::Wrack => {
                let dot = fight.agent.wrack.expect("Wrack is bound").dot;
                corruption::apply(fight, spell, target, dot);
            }
            WarlockSpell::Hellfire => hellfire::apply_channel(fight, spell),
            WarlockSpell::RainOfFire => rain_of_fire::apply_channel(fight, spell),
            WarlockSpell::RainOfFireTick => {
                let rain = fight.agent.rain_of_fire.expect("Rain of Fire is bound");
                rain.apply_tick(fight, spell);
            }
            WarlockSpell::DeathCoil => {
                // The hit rolls at the cast and lands after travel.
                let (base, _, _) = fight.agent.death_coil.expect("Death Coil is bound");
                let result = fight.calc_damage(spell, target, base);
                fight.class_after_travel(spell, result);
            }
            WarlockSpell::BaneOfHavoc => bane_of_havoc::apply(fight, spell, target),
            WarlockSpell::Incinerate => {
                let (immolate, bonus) = (fight.agent.immolate_dot, fight.agent.incinerate_bonus);
                incinerate::apply(fight, spell, target, immolate, bonus);
            }
            WarlockSpell::CurseOfTheElements => {
                let curse = fight
                    .agent
                    .curse_of_the_elements
                    .clone()
                    .expect("Curse of the Elements is bound");
                curse.apply(fight, spell, target);
            }
            WarlockSpell::CurseOfRecklessness => {
                let curse = fight
                    .agent
                    .curse_of_recklessness
                    .expect("Curse of Recklessness is bound");
                curse.apply(fight, spell, target);
            }
            WarlockSpell::LifeTap => {
                let tap = fight.agent.life_tap.expect("Life Tap is bound");
                tap.apply(fight);
            }
            WarlockSpell::Conflagrate => {
                let conflagrate = fight
                    .agent
                    .conflagrate
                    .clone()
                    .expect("Conflagrate is bound");
                conflagrate.apply(fight, spell, target);
            }
            WarlockSpell::AmplifyCurse => {
                let aura = fight.agent.amplify_curse.expect("Amplify Curse is bound");
                fight.activate_aura(aura);
            }
            WarlockSpell::LashOfPain => {
                let base = fight.agent.lash_of_pain_base;
                pets::lash_of_pain(fight, spell, target, base);
            }
            WarlockSpell::Firebolt => {
                let bounds = fight.agent.firebolt;
                let roll = fight.agent.firebolt_roll;
                pets::firebolt(fight, spell, target, bounds, roll);
            }
            WarlockSpell::DemonicBrand => {
                let brand = fight
                    .agent
                    .demonic_brand
                    .clone()
                    .expect("Demonic Brand is bound");
                brand.brand_hit(fight, spell, target);
            }
            WarlockSpell::ImmolateDot => panic!("Immolate's dot spell is never cast"),
        }
    }

    fn on_periodic(fight: &mut Fight<Self>, tag: u32) {
        if tag == FEL_ENERGY_TICK {
            let (_, fraction, metrics) = fight.agent.fel_energy.expect("bound");
            let max_mana = fight.player.powers.max_mana;
            fight.add_mana(max_mana * fraction, metrics);
        }
    }

    fn health_metrics_before_cost(behavior: WarlockSpell) -> bool {
        // siphon_life.go and drain_life.go register their health metrics before the spell.
        matches!(behavior, WarlockSpell::SiphonLife | WarlockSpell::DrainLife)
    }

    fn on_travel(
        fight: &mut Fight<Self>,
        spell: SpellId,
        result: SpellResult,
        behavior: WarlockSpell,
    ) {
        fight.deal_damage(spell, result, false);
        if behavior == WarlockSpell::DeathCoil && result.landed() {
            let (_, heal, healing) = fight.agent.death_coil.expect("Death Coil is bound");
            fight.calc_and_deal_self_healing(heal, result.damage, healing);
        }
    }

    fn pet_rotation(fight: &mut Fight<Self>, pet: Side) {
        let demon = fight.agent.demon.clone().expect("the demon's AI is bound");
        if demon.pet == pet {
            demon.rotation(fight);
        }
    }

    fn on_periodic_damage_dealt(
        fight: &mut Fight<Self>,
        aura: AuraRef,
        kind: WarlockAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        match kind {
            WarlockAura::NightfallTrigger => {
                let talent = fight.agent.nightfall.clone().expect("bound");
                talent.on_periodic_damage_dealt(fight, aura, spell, result);
            }
            WarlockAura::HavocCopy => bane_of_havoc::copy_damage(fight, result),
            _ => {}
        }
    }

    fn on_delayed_proc(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: WarlockAura,
        spell: SpellId,
        _result: SpellResult,
    ) {
        match kind {
            WarlockAura::NightfallTrigger => {
                let talent = fight.agent.nightfall.clone().expect("bound");
                talent.on_trigger(fight);
            }
            WarlockAura::ShadowTrance => {
                let talent = fight.agent.nightfall.clone().expect("bound");
                talent.on_consume(fight, spell);
            }
            _ => {}
        }
    }

    fn on_cast_complete(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: WarlockAura,
        spell: SpellId,
    ) {
        if kind == WarlockAura::ShadowTrance {
            let talent = fight.agent.nightfall.clone().expect("bound");
            talent.on_cast_complete(fight, spell);
        }
    }

    fn extra_cast_condition(fight: &Fight<Self>, _spell: SpellId, behavior: WarlockSpell) -> bool {
        match behavior {
            // items.go: a demon out and enabled.
            WarlockSpell::BlackBook => fight
                .agent
                .black_book
                .is_some_and(|(aura, _)| fight.active_pet(aura.side).enabled),
            WarlockSpell::Conflagrate => fight
                .agent
                .conflagrate
                .as_ref()
                .expect("Conflagrate is bound")
                .can_cast(fight),
            _ => true,
        }
    }

    fn on_dot_tick(fight: &mut Fight<Self>, dot: DotId, behavior: WarlockSpell) {
        match behavior {
            WarlockSpell::BaneOfAgony => bane_of_agony::tick(fight, dot),
            WarlockSpell::ImmolateDot | WarlockSpell::Corruption | WarlockSpell::BaneOfDoom => {
                fight.snapshot_dot_tick(dot)
            }
            WarlockSpell::SiphonLife => {
                let siphon = fight.agent.siphon_life.expect("Siphon Life is bound");
                siphon.tick(fight, dot);
            }
            WarlockSpell::DrainLife => {
                let drain = fight.agent.drain_life.expect("Drain Life is bound");
                drain.tick(fight, dot);
            }
            WarlockSpell::Wrack => {
                let wrack = fight.agent.wrack.expect("Wrack is bound");
                wrack.tick(fight, dot);
            }
            WarlockSpell::Hellfire => hellfire::tick(fight, dot),
            WarlockSpell::RainOfFire => {
                let rain = fight.agent.rain_of_fire.expect("Rain of Fire is bound");
                let side = fight.dots[dot].side;
                rain.on_channel_tick(fight, side);
            }
            _ => {}
        }
    }

    fn on_gain(fight: &mut Fight<Self>, aura: AuraRef, kind: WarlockAura) {
        match kind {
            WarlockAura::CurseOfTheElements => {
                let curse = fight.agent.curse_of_the_elements.clone().expect("bound");
                curse.on_gain(fight, aura.side);
            }
            WarlockAura::CurseOfRecklessness => {
                let curse = fight.agent.curse_of_recklessness.expect("bound");
                curse.on_gain(fight, aura.side);
            }
            WarlockAura::ShadowAndFlame => {
                let talent = fight.agent.shadow_and_flame.clone().expect("bound");
                talent.on_gain(fight, aura);
            }
            WarlockAura::ShadowTrance => {
                let talent = fight.agent.nightfall.clone().expect("bound");
                talent.on_gain(fight);
            }
            WarlockAura::Decimation => {
                let talent = fight.agent.decimation.clone().expect("bound");
                talent.on_gain(fight);
            }
            WarlockAura::HavocBane => bane_of_havoc::on_gain(fight, aura),
            WarlockAura::BlackBook => {
                let (_, multiplier) = fight.agent.black_book.expect("The Black Book is bound");
                fight.unit_mut(aura.side).damage_dealt_multiplier *= multiplier;
            }
            WarlockAura::FelEnergy => {
                // Go StartPeriodicAction at the regeneration priority, first tick a period on.
                let (period, _, _) = fight.agent.fel_energy.expect("bound");
                fight.start_class_periodic(FEL_ENERGY_TICK, period, 0, PRIORITY_REGEN);
            }
            _ => {}
        }
    }

    fn on_expire(fight: &mut Fight<Self>, aura: AuraRef, kind: WarlockAura) {
        match kind {
            WarlockAura::CurseOfTheElements => {
                let curse = fight.agent.curse_of_the_elements.clone().expect("bound");
                curse.on_expire(fight, aura.side);
            }
            WarlockAura::CurseOfRecklessness => {
                let curse = fight.agent.curse_of_recklessness.expect("bound");
                curse.on_expire(fight, aura.side);
            }
            WarlockAura::ShadowAndFlame => {
                let talent = fight.agent.shadow_and_flame.clone().expect("bound");
                talent.on_expire(fight, aura);
            }
            WarlockAura::ShadowTrance => {
                let talent = fight.agent.nightfall.clone().expect("bound");
                talent.on_expire(fight);
            }
            WarlockAura::Decimation => {
                let talent = fight.agent.decimation.clone().expect("bound");
                talent.on_expire(fight);
            }
            WarlockAura::HavocBane => bane_of_havoc::on_expire(fight, aura),
            WarlockAura::BlackBook => {
                let (_, multiplier) = fight.agent.black_book.expect("The Black Book is bound");
                fight.unit_mut(aura.side).damage_dealt_multiplier /= multiplier;
            }
            _ => {}
        }
    }

    fn on_spell_hit_dealt(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: WarlockAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        match kind {
            WarlockAura::ImprovedShadowBoltTrigger => {
                let talent = fight.agent.improved_shadow_bolt.clone().expect("bound");
                talent.on_spell_hit_dealt(fight, spell, result);
            }
            WarlockAura::ShadowAndFlameTrigger => {
                let talent = fight.agent.shadow_and_flame.clone().expect("bound");
                talent.on_spell_hit_dealt(fight, spell, result);
            }
            WarlockAura::DecimationTrigger => {
                let talent = fight.agent.decimation.clone().expect("bound");
                talent.on_spell_hit_dealt(fight, spell, result);
            }
            WarlockAura::DemonicBrandTrigger => {
                let talent = fight.agent.demonic_brand.clone().expect("bound");
                talent.on_spell_hit_dealt(fight, spell, result);
            }
            WarlockAura::DemonicBrandConsumer => {
                let talent = fight.agent.demonic_brand.clone().expect("bound");
                talent.on_demon_hit(fight, spell, result);
            }
            WarlockAura::HavocCopy => bane_of_havoc::copy_damage(fight, result),
            _ => {}
        }
    }
}
