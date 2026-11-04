//! The Mage class agent for the fight runtime: class spells and auras by name, and the
//! hooks that dispatch to each spell's and talent's module.

use std::rc::Rc;

use crate::{
    contracts::prepared_v2::{Effect, PreparedV2, Spell as ExportedSpell},
    core::fight::{Agent, AuraRef, DotId, Fight, Side, SpellId, SpellResult},
};

use super::{
    spells::{
        arcane_blast, arcane_missiles, arcane_power, arcane_wrath, area_hit, blizzard, cold_snap,
        combustion, evocation, fire_blast, flamestrike, ice_barrier, ice_lance, mana_gems,
        presence_of_mind, scorch,
    },
    talents::{
        arcane_concentration, fingers_of_frost, fire_power, heating_up, ignite, master_of_elements,
        missile_barrage, winters_chill,
    },
};

/// What a Mage spell does when its effects apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MageSpell {
    Frostbolt,
    FrostBlast,
    ArcaneBlast,
    /// Arcane Wrath (client 70425).
    ArcaneWrath,
    ArcanePower,
    PresenceOfMind,
    FireBlast,
    Fireball,
    FrostfireBolt,
    Pyroblast,
    Combustion,
    Ignite,
    Scorch,
    IceLance,
    ArcaneMissiles,
    ArcaneMissile,
    ColdSnap,
    /// Ice Barrier (the fork's patch 187).
    IceBarrier,
    Evocation,
    /// A mana gem by its index in Go's order, smallest first.
    ManaGem(usize),
    /// Arcane Explosion, Cone of Cold, Frost Nova or Blast Wave.
    AreaHit,
    Flamestrike,
    /// Blizzard's channel.
    Blizzard,
    /// The spell each Blizzard period casts.
    BlizzardTick,
    /// Improved Blizzard's chill.
    ImprovedBlizzard,
}

/// Class auras with Rust behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MageAura {
    WintersChill,
    WintersChillTrigger,
    FingersOfFrost,
    FingersOfFrostTrigger,
    Clearcasting,
    ArcaneConcentrationTrigger,
    MissileBarrage,
    MissileBarrageTrigger,
    EvocationRegen,
    ArcaneCharges,
    ArcanePower,
    PresenceOfMind,
    IgniteTrigger,
    FireVulnerability,
    MasterOfElementsTrigger,
    HeatingUp,
    HeatingUpTrigger,
    /// Heating Up's trigger on every Fire Blast cast.
    HeatingUpFireBlast,
    Combustion,
    /// Ice Barrier's shield.
    IceBarrier,
    /// Frigid Power, which every Ice Barrier cast applies.
    FrigidPower,
}

/// Mage state that Go keeps in the `Mage` struct and its closures.
#[derive(Default)]
pub(crate) struct MageAgent {
    winters_chill: Option<Rc<winters_chill::WintersChill>>,
    fingers_of_frost: Option<fingers_of_frost::FingersOfFrost>,
    arcane_concentration: Option<Rc<arcane_concentration::ArcaneConcentration>>,
    missile_barrage: Option<Rc<missile_barrage::MissileBarrage>>,
    arcane_charges: Option<Rc<arcane_blast::ArcaneCharges>>,
    arcane_power: Option<Rc<arcane_power::ArcanePower>>,
    ice_barrier: Option<Rc<ice_barrier::IceBarrier>>,
    presence_of_mind: Option<Rc<presence_of_mind::PresenceOfMind>>,
    improved_scorch: Option<Rc<scorch::ImprovedScorch>>,
    master_of_elements: Option<Rc<master_of_elements::MasterOfElements>>,
    heating_up: Option<Rc<heating_up::HeatingUp>>,

    combustion: Option<Rc<combustion::Combustion>>,
    ignite: Option<Rc<ignite::Ignite>>,
    ice_lance_frozen_multiplier: f64,
    /// Arcane Missiles channel spell to the missile spell of the same rank.
    missiles: Vec<(SpellId, SpellId)>,
    /// Arcane Wrath's coefficient and share of maximum mana.
    arcane_wrath: (f64, f64),
    fire_power: Option<Rc<fire_power::FirePower>>,
    gems: mana_gems::ManaGems,
    evocation_regen: Option<(AuraRef, f64)>,
    blizzard: Option<blizzard::Blizzard>,
    /// Each Flamestrike rank's spell and tick amount.
    flamestrike_ticks: Vec<(SpellId, f64)>,
}

/// Player aura labels claimed by implemented class effects.
fn class_auras(prepared: &PreparedV2) -> Vec<(String, MageAura)> {
    let mut auras = Vec::new();
    for effect in &prepared.effects {
        let (aura, trigger, kinds) = match effect {
            Effect::WintersChill {
                aura, trigger_aura, ..
            } => (
                aura,
                trigger_aura,
                (MageAura::WintersChill, MageAura::WintersChillTrigger),
            ),
            Effect::FingersOfFrost {
                aura, trigger_aura, ..
            } => (
                aura,
                trigger_aura,
                (MageAura::FingersOfFrost, MageAura::FingersOfFrostTrigger),
            ),
            Effect::ArcaneConcentration {
                aura, trigger_aura, ..
            } => (
                aura,
                trigger_aura,
                (MageAura::Clearcasting, MageAura::ArcaneConcentrationTrigger),
            ),
            Effect::MissileBarrage {
                aura, trigger_aura, ..
            } => (
                aura,
                trigger_aura,
                (MageAura::MissileBarrage, MageAura::MissileBarrageTrigger),
            ),
            Effect::Evocation { regen_aura, .. } => {
                auras.push((regen_aura.clone(), MageAura::EvocationRegen));
                continue;
            }
            Effect::ArcaneBlast { aura, .. } => {
                auras.push((aura.clone(), MageAura::ArcaneCharges));
                continue;
            }
            Effect::ArcanePower { aura, .. } => {
                auras.push((aura.clone(), MageAura::ArcanePower));
                continue;
            }
            Effect::IceBarrier {
                aura,
                frigid_power_aura,
                ..
            } => {
                auras.push((aura.clone(), MageAura::IceBarrier));
                auras.push((frigid_power_aura.clone(), MageAura::FrigidPower));
                continue;
            }
            Effect::Ignite { trigger_aura, .. } => {
                auras.push((trigger_aura.clone(), MageAura::IgniteTrigger));
                continue;
            }
            Effect::MasterOfElements { trigger_aura, .. } => {
                auras.push((trigger_aura.clone(), MageAura::MasterOfElementsTrigger));
                continue;
            }
            Effect::Combustion { aura, .. } => {
                auras.push((aura.clone(), MageAura::Combustion));
                continue;
            }
            Effect::HeatingUp {
                aura,
                trigger_aura,
                fire_blast_trigger_aura,
                ..
            } => {
                auras.push((
                    fire_blast_trigger_aura.clone(),
                    MageAura::HeatingUpFireBlast,
                ));
                (
                    aura,
                    trigger_aura,
                    (MageAura::HeatingUp, MageAura::HeatingUpTrigger),
                )
            }
            Effect::Scorch {
                improved_scorch: Some(improved),
            } => {
                auras.push((improved.aura.clone(), MageAura::FireVulnerability));
                continue;
            }
            Effect::PresenceOfMind { aura, .. } => {
                auras.push((aura.clone(), MageAura::PresenceOfMind));
                continue;
            }
            _ => continue,
        };
        auras.push((aura.clone(), kinds.0));
        auras.push((trigger.clone(), kinds.1));
    }
    auras
}

impl MageAgent {
    /// The class behavior of an exported spell, if Rust implements it.
    pub(crate) fn spell(spell: &ExportedSpell, gems: &[i32]) -> Option<MageSpell> {
        match spell.class_spell.as_deref()? {
            "cold_snap" => Some(MageSpell::ColdSnap),
            "ice_barrier" => Some(MageSpell::IceBarrier),
            "arcane_power" => Some(MageSpell::ArcanePower),
            "combustion" => Some(MageSpell::Combustion),
            "ignite" if spell.dot.is_some() => Some(MageSpell::Ignite),
            "fire_blast" if spell.damage_effect.is_some() => Some(MageSpell::FireBlast),
            "fireball" if spell.damage_effect.is_some() && spell.dot.is_some() => {
                Some(MageSpell::Fireball)
            }
            "frostfire_bolt" if spell.damage_effect.is_some() && spell.dot.is_some() => {
                Some(MageSpell::FrostfireBolt)
            }
            "pyroblast" if spell.damage_effect.is_some() && spell.dot.is_some() => {
                Some(MageSpell::Pyroblast)
            }
            "scorch" if spell.damage_effect.is_some() => Some(MageSpell::Scorch),
            "presence_of_mind" => Some(MageSpell::PresenceOfMind),
            "evocation" if spell.dot.is_some() => Some(MageSpell::Evocation),
            "mana_gem" => {
                let item = spell.action_id.as_ref()?.item_id;
                gems.iter()
                    .position(|gem| *gem == item)
                    .map(MageSpell::ManaGem)
            }
            "frostbolt" if spell.damage_effect.is_some() => Some(MageSpell::Frostbolt),
            "frost_blast" if spell.damage_effect.is_some() => Some(MageSpell::FrostBlast),
            "arcane_blast" if spell.damage_effect.is_some() => Some(MageSpell::ArcaneBlast),
            "arcane_wrath" if spell.damage_effect.is_some() => Some(MageSpell::ArcaneWrath),
            "ice_lance" if spell.damage_effect.is_some() => Some(MageSpell::IceLance),
            "arcane_missiles_cast" if spell.dot.is_some() => Some(MageSpell::ArcaneMissiles),
            "arcane_missiles_tick" if spell.damage_effect.is_some() => {
                Some(MageSpell::ArcaneMissile)
            }
            "arcane_explosion" | "cone_of_cold" | "frost_nova" | "blast_wave"
                if spell.damage_effect.is_some() =>
            {
                Some(MageSpell::AreaHit)
            }
            "flamestrike" if spell.damage_effect.is_some() && spell.dot.is_some() => {
                Some(MageSpell::Flamestrike)
            }
            "blizzard" if spell.dot.is_some() => Some(MageSpell::Blizzard),
            "blizzard" => Some(MageSpell::BlizzardTick),
            "improved_blizzard" => Some(MageSpell::ImprovedBlizzard),
            _ => None,
        }
    }

    /// Build a fight for a prepared input that passed the coverage gate.
    pub(crate) fn fight(prepared: &PreparedV2, rng_mode: crate::core::rng::RngMode) -> Result<Fight<MageAgent>, String> {
        let auras = class_auras(prepared);
        let gems: Vec<i32> = prepared
            .effects
            .iter()
            .find_map(|effect| match effect {
                Effect::ManaGems { gems, .. } => Some(gems.iter().map(|gem| gem.item_id).collect()),
                _ => None,
            })
            .unwrap_or_default();
        let spell = |exported: &ExportedSpell| MageAgent::spell(exported, &gems);
        let mut fight = Fight::new(prepared, rng_mode, MageAgent::default(), spell, |unit, label| {
            (unit == "player")
                .then(|| {
                    auras
                        .iter()
                        .find(|(name, _)| name == label)
                        .map(|(_, kind)| *kind)
                })
                .flatten()
        })?;
        for effect in &prepared.effects {
            match effect {
                Effect::WintersChill {
                    aura,
                    trigger_aura,
                    proc_chance,
                    damage_per_stack,
                    spell_power_per_stack,
                    cone_of_cold_consumes,
                    ..
                } => {
                    let bound = winters_chill::bind(
                        &mut fight,
                        aura,
                        trigger_aura,
                        *proc_chance,
                        *damage_per_stack,
                        (*spell_power_per_stack, *cone_of_cold_consumes),
                    )?;
                    fight.agent.winters_chill = Some(Rc::new(bound));
                }
                Effect::FingersOfFrost {
                    aura,
                    trigger_aura,
                    proc_chance,
                    shatter_crit,
                    ..
                } => {
                    let bound = fingers_of_frost::bind(
                        &mut fight,
                        aura,
                        trigger_aura,
                        *proc_chance,
                        *shatter_crit,
                    )?;
                    fight.agent.fingers_of_frost = Some(bound);
                }
                Effect::ArcaneConcentration {
                    aura,
                    trigger_aura,
                    proc_chance,
                    ..
                } => {
                    let bound =
                        arcane_concentration::bind(&fight, aura, trigger_aura, *proc_chance)?;
                    fight.agent.arcane_concentration = Some(Rc::new(bound));
                }
                Effect::MissileBarrage {
                    aura,
                    arcane_blast_chance,
                    missile_chance,
                    bolt_chance,
                    rng_label,
                    cost_percent_add,
                    tick_length_delta_ns,
                    ..
                } => {
                    let bound = missile_barrage::bind(
                        &mut fight,
                        aura,
                        (*arcane_blast_chance, *missile_chance, *bolt_chance),
                        rng_label,
                        *cost_percent_add,
                        *tick_length_delta_ns,
                    )?;
                    fight.agent.missile_barrage = Some(Rc::new(bound));
                }
                Effect::IceLance {
                    frozen_multiplier, ..
                } => fight.agent.ice_lance_frozen_multiplier = *frozen_multiplier,
                Effect::ArcaneBlast {
                    aura,
                    intellect_per_stack,
                    done_intellect,
                    cost_per_stack,
                    ..
                } => {
                    let place = Fight::<Self>::stat_aura_place(&prepared.effects, aura)
                        .ok_or("the Arcane Blast stacks are not a stat aura")?;
                    let bound = arcane_blast::bind(
                        &mut fight,
                        aura,
                        (*intellect_per_stack, *cost_per_stack, *done_intellect),
                        place,
                    )?;
                    fight.agent.arcane_charges = Some(Rc::new(bound));
                }
                Effect::ArcaneWrath {
                    coefficient,
                    mana_share,
                    ..
                } => fight.agent.arcane_wrath = (*coefficient, *mana_share),
                Effect::FirePower {
                    per_effect,
                    max_effects,
                } => {
                    let bound = fire_power::bind(&fight, *per_effect, *max_effects);
                    fight.agent.fire_power = Some(Rc::new(bound));
                }
                Effect::MasterOfElements {
                    trigger_aura,
                    refund,
                    metrics_action_id,
                } => {
                    let bound = master_of_elements::bind(
                        &mut fight,
                        trigger_aura,
                        *refund,
                        metrics_action_id,
                    )?;
                    fight.agent.master_of_elements = Some(Rc::new(bound));
                }
                Effect::Ignite {
                    spell_id,
                    share,
                    num_ticks,
                    can_proc_from_procs,
                    ..
                } => {
                    let bound =
                        ignite::bind(&fight, *spell_id, *share, *num_ticks, *can_proc_from_procs)?;
                    fight.agent.ignite = Some(Rc::new(bound));
                }
                Effect::Combustion {
                    spell_id,
                    aura,
                    crit_per_stack,
                    max_crits,
                } => {
                    let spell = fight
                        .spells
                        .iter()
                        .position(|spell| spell.id.spell_id == *spell_id && spell.id.tag == 0)
                        .ok_or_else(|| format!("Combustion spell {spell_id} is not registered"))?;
                    let bound =
                        combustion::bind(&mut fight, spell, aura, *crit_per_stack, *max_crits)?;
                    fight.agent.combustion = Some(Rc::new(bound));
                }
                Effect::Pyroblast {
                    spell_id,
                    tick_base,
                    tick_can_crit,
                } => {
                    let dot = fight
                        .spells
                        .iter()
                        .find(|spell| spell.id.spell_id == *spell_id && spell.id.tag == 0)
                        .and_then(|spell| spell.dot);
                    if let Some(dot) = dot {
                        fight.dots[dot].tick_base = Some(*tick_base);
                        fight.dots[dot].tick_can_crit = *tick_can_crit;
                    }
                }
                Effect::HeatingUp {
                    aura,
                    cast_time_per_stack,
                    ..
                } => {
                    let bound = heating_up::bind(&mut fight, aura, *cast_time_per_stack)?;
                    fight.agent.heating_up = Some(Rc::new(bound));
                }
                Effect::Fireball { ranks } | Effect::FrostfireBolt { ranks } => {
                    for rank in ranks {
                        let dot = fight
                            .spells
                            .iter()
                            .find(|spell| spell.id.spell_id == rank.spell_id && spell.id.tag == 0)
                            .and_then(|spell| spell.dot);
                        if let Some(dot) = dot {
                            fight.dots[dot].tick_base = Some(rank.tick_base);
                            fight.dots[dot].tick_can_crit = rank.tick_can_crit;
                        }
                    }
                }
                Effect::Scorch {
                    improved_scorch: Some(improved),
                } => {
                    let bound = scorch::bind(
                        &mut fight,
                        &improved.aura,
                        improved.proc_chance,
                        improved.damage_per_stack,
                    )?;
                    fight.agent.improved_scorch = Some(Rc::new(bound));
                }
                Effect::ArcanePower {
                    aura,
                    damage,
                    cost_percent_add,
                    ..
                } => {
                    let bound = arcane_power::bind(&mut fight, aura, *damage, *cost_percent_add)?;
                    fight.agent.arcane_power = Some(Rc::new(bound));
                }
                Effect::IceBarrier {
                    aura,
                    frigid_power_aura,
                    absorb_base,
                    warding,
                    hit,
                    dot,
                    frigid_hit,
                    frigid_dot,
                    ..
                } => {
                    let bound = ice_barrier::bind(
                        &mut fight,
                        aura,
                        frigid_power_aura,
                        *absorb_base,
                        *warding,
                        (*hit, *dot),
                        (*frigid_hit, *frigid_dot),
                    )?;
                    fight.agent.ice_barrier = Some(Rc::new(bound));
                }
                Effect::PresenceOfMind {
                    spell_id,
                    aura,
                    cast_time_percent,
                } => {
                    let spell = fight
                        .spells
                        .iter()
                        .position(|spell| spell.id.spell_id == *spell_id && spell.id.tag == 0)
                        .ok_or_else(|| {
                            format!("Presence of Mind spell {spell_id} is not registered")
                        })?;
                    let bound =
                        presence_of_mind::bind(&mut fight, spell, aura, *cast_time_percent)?;
                    fight.agent.presence_of_mind = Some(Rc::new(bound));
                }
                Effect::ManaGems {
                    gems,
                    regen_window_seconds,
                } => {
                    let mana = gems.iter().map(|gem| gem.mana).collect();
                    fight.agent.gems = mana_gems::ManaGems::new(mana, *regen_window_seconds);
                }
                Effect::Evocation {
                    regen_aura,
                    regen_multiplier,
                    ..
                } => {
                    let aura = fight.player_aura(regen_aura)?;
                    fight.agent.evocation_regen = Some((aura, *regen_multiplier));
                }
                Effect::Flamestrike { ranks } => {
                    for rank in ranks {
                        if let Some(spell) = fight.spells.iter().position(|spell| {
                            spell.id.spell_id == rank.spell_id && spell.id.tag == 0
                        }) {
                            fight.agent.flamestrike_ticks.push((spell, rank.tick_base));
                        }
                    }
                }
                Effect::Blizzard {
                    spell_id,
                    tick_spell_id,
                    tick_base,
                    improved_blizzard_spell_id,
                } => {
                    let bound = blizzard::bind(
                        &fight,
                        *spell_id,
                        *tick_spell_id,
                        *tick_base,
                        *improved_blizzard_spell_id,
                    )?;
                    fight.agent.blizzard = Some(bound);
                }
                Effect::ArcaneMissiles { ranks } => {
                    for rank in ranks {
                        let find = |id: i32| {
                            fight
                                .spells
                                .iter()
                                .position(|spell| spell.id.spell_id == id && spell.id.tag == 0)
                        };
                        if let (Some(channel), Some(missile)) =
                            (find(rank.channel_spell_id), find(rank.tick_spell_id))
                        {
                            fight.agent.missiles.push((channel, missile));
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(fight)
    }

    /// Go `procFingersOfFrost` after a chilling bolt's hit roll: the charges at launch.
    fn launch_fingers(fight: &mut Fight<Self>, spell: SpellId, result: &SpellResult) {
        if fight.agent.fingers_of_frost.is_none() {
            return;
        }
        if Self::with_fingers(fight, |state, fight| state.launch(fight, spell, result)) {
            let aura = fight.agent.fingers_of_frost.as_ref().expect("bound").aura;
            fight.activate_aura(aura);
            let max = fight.aura(aura).max_stacks;
            fight.set_stacks(aura, max);
        }
    }

    /// Run a Fingers of Frost hook with its state taken out of the agent.
    fn with_fingers<T>(
        fight: &mut Fight<Self>,
        hook: impl FnOnce(&mut fingers_of_frost::FingersOfFrost, &mut Fight<Self>) -> T,
    ) -> T {
        let mut state = fight
            .agent
            .fingers_of_frost
            .take()
            .expect("Fingers of Frost is bound");
        let value = hook(&mut state, fight);
        fight.agent.fingers_of_frost = Some(state);
        value
    }

    fn winters_chill(fight: &Fight<Self>) -> Rc<winters_chill::WintersChill> {
        fight
            .agent
            .winters_chill
            .clone()
            .expect("Winter's Chill is bound")
    }

    fn arcane_concentration(fight: &Fight<Self>) -> Rc<arcane_concentration::ArcaneConcentration> {
        fight
            .agent
            .arcane_concentration
            .clone()
            .expect("Arcane Concentration is bound")
    }

    fn arcane_charges(fight: &Fight<Self>) -> Rc<arcane_blast::ArcaneCharges> {
        fight
            .agent
            .arcane_charges
            .clone()
            .expect("Arcane Blast is bound")
    }

    fn arcane_power(fight: &Fight<Self>) -> Rc<arcane_power::ArcanePower> {
        fight
            .agent
            .arcane_power
            .clone()
            .expect("Arcane Power is bound")
    }

    fn ice_barrier(fight: &Fight<Self>) -> Rc<ice_barrier::IceBarrier> {
        fight
            .agent
            .ice_barrier
            .clone()
            .expect("Ice Barrier is bound")
    }

    fn ignite(fight: &Fight<Self>) -> Rc<ignite::Ignite> {
        fight.agent.ignite.clone().expect("Ignite is bound")
    }

    fn combustion(fight: &Fight<Self>) -> Rc<combustion::Combustion> {
        fight.agent.combustion.clone().expect("Combustion is bound")
    }

    fn heating_up(fight: &Fight<Self>) -> Rc<heating_up::HeatingUp> {
        fight.agent.heating_up.clone().expect("Heating Up is bound")
    }

    fn improved_scorch(fight: &Fight<Self>) -> Rc<scorch::ImprovedScorch> {
        fight
            .agent
            .improved_scorch
            .clone()
            .expect("Improved Scorch is bound")
    }

    fn presence_of_mind(fight: &Fight<Self>) -> Rc<presence_of_mind::PresenceOfMind> {
        fight
            .agent
            .presence_of_mind
            .clone()
            .expect("Presence of Mind is bound")
    }

    fn missile_barrage(fight: &Fight<Self>) -> Rc<missile_barrage::MissileBarrage> {
        fight
            .agent
            .missile_barrage
            .clone()
            .expect("Missile Barrage is bound")
    }
}

/// The class action of a Fingers of Frost charge spent on a spell's landing (patch 139).
const FINGERS_OF_FROST_LANDING: u32 = 0;

impl Agent for MageAgent {
    /// Fire Power's handler on every attack table (client 70425).
    fn caster_damage_multiplier(fight: &Fight<Self>, spell: SpellId, target: Side) -> Option<f64> {
        let fire_power = fight.agent.fire_power.as_ref()?;
        if fight.spells[spell].caster != Side::Player {
            return None;
        }
        let fire_vulnerability = fight
            .agent
            .improved_scorch
            .as_ref()
            .map(|scorch| scorch.aura);
        Some(fire_power.multiplier(fight, spell, target, fire_vulnerability))
    }

    fn on_class_action(fight: &mut Fight<Self>, tag: u32) {
        if tag == FINGERS_OF_FROST_LANDING {
            let aura = fight.agent.fingers_of_frost.as_ref().expect("bound").aura;
            if fight.aura(aura).active {
                fight.remove_stack(aura);
            }
        }
    }

    type Spell = MageSpell;
    type Aura = MageAura;

    fn apply_effects(fight: &mut Fight<Self>, spell: SpellId, target: Side, behavior: MageSpell) {
        match behavior {
            // Go frost_blast.go's ApplyEffects is Frostbolt's: a chill that rolls Fingers of Frost at
            // launch, dealt on arrival.
            MageSpell::Frostbolt | MageSpell::FrostBlast => {
                let base = fight.roll_damage_effect(spell);
                let result = fight.calc_damage(spell, target, base);
                Self::launch_fingers(fight, spell, &result);
                fight.deal_damage_after_travel(spell, result);
            }
            MageSpell::ArcaneBlast => {
                let charges = Self::arcane_charges(fight);
                arcane_blast::apply(fight, spell, target, &charges);
            }
            MageSpell::ArcanePower => {
                let aura = Self::arcane_power(fight).aura;
                fight.activate_aura(aura);
            }
            MageSpell::FireBlast => fire_blast::apply(fight, spell, target),
            MageSpell::Combustion => {
                let state = Self::combustion(fight);
                combustion::apply(fight, &state);
            }
            MageSpell::Ignite => Self::ignite(fight).apply(fight, target),
            MageSpell::Fireball | MageSpell::FrostfireBolt | MageSpell::Pyroblast => {
                let base = fight.roll_damage_effect(spell);
                let result = fight.calc_damage(spell, target, base);
                if matches!(behavior, MageSpell::FrostfireBolt) {
                    Self::launch_fingers(fight, spell, &result);
                }
                let dot = fight.spells[spell].dot.expect("the bolt has a dot");
                let dot = fight.dot_on(dot, target);
                fight.deal_damage_after_travel_then_dot(spell, result, dot);
            }
            MageSpell::Scorch => {
                let improved = fight.agent.improved_scorch.clone();
                scorch::apply(fight, spell, target, improved.as_deref());
            }
            MageSpell::PresenceOfMind => {
                let aura = Self::presence_of_mind(fight).aura;
                fight.activate_aura(aura);
            }
            MageSpell::IceLance => {
                // Go IsTargetFrozen: Fingers of Frost is active.
                let frozen = fight
                    .agent
                    .fingers_of_frost
                    .as_ref()
                    .is_some_and(|fingers| fingers.frozen(fight));
                let multiplier = frozen.then_some(fight.agent.ice_lance_frozen_multiplier);
                // Client 70630: an empowered lance applies Winter's Chill.
                let chill = frozen.then(|| fight.agent.winters_chill.clone()).flatten();
                ice_lance::apply(fight, spell, target, multiplier, chill.as_deref());
            }
            MageSpell::ArcaneMissiles => arcane_missiles::apply_channel(fight, spell),
            MageSpell::ArcaneMissile => arcane_missiles::apply_missile(fight, spell, target),
            MageSpell::ArcaneWrath => {
                let wrath = fight.agent.arcane_wrath;
                arcane_wrath::apply(fight, spell, target, wrath);
            }
            MageSpell::ColdSnap => cold_snap::apply(fight),
            MageSpell::IceBarrier => Self::ice_barrier(fight).apply(fight),
            MageSpell::Evocation => evocation::apply(fight, spell),
            MageSpell::ManaGem(gem) => {
                let mana = fight.agent.gems.mana[gem];
                mana_gems::apply(fight, spell, mana);
                fight.agent.gems.used[gem] = true;
            }
            MageSpell::AreaHit => {
                // Client 70630: Winter's Chill's stacks add to the next Cone of Cold's damage.
                let chill = fight.agent.winters_chill.clone();
                area_hit::apply(fight, spell, chill.as_deref());
            }
            MageSpell::Flamestrike => flamestrike::apply(fight, spell),
            MageSpell::Blizzard => blizzard::apply_channel(fight, spell),
            MageSpell::BlizzardTick => {
                let state = fight.agent.blizzard.expect("Blizzard is bound");
                state.apply_tick(fight, spell);
            }
            MageSpell::ImprovedBlizzard => blizzard::apply_chill(fight, spell, target),
        }
    }

    fn extra_cast_condition(fight: &Fight<Self>, _spell: SpellId, behavior: MageSpell) -> bool {
        match behavior {
            MageSpell::ManaGem(gem) => fight.agent.gems.available(gem),
            MageSpell::PresenceOfMind => fight.gcd_ready(),
            MageSpell::Combustion => Self::combustion(fight).can_cast(fight),
            _ => true,
        }
    }

    fn should_activate(fight: &Fight<Self>, _spell: SpellId, behavior: MageSpell) -> bool {
        match behavior {
            MageSpell::ManaGem(gem) => fight.agent.gems.should_activate(fight, gem),
            // Go leaves Evocation to the rotation.
            MageSpell::Evocation => false,
            _ => true,
        }
    }

    fn reset(fight: &mut Fight<Self>) {
        fight.agent.gems.reset();
    }

    fn on_dot_gain(fight: &mut Fight<Self>, _dot: DotId, behavior: MageSpell) {
        if behavior == MageSpell::Evocation {
            let (aura, _) = fight.agent.evocation_regen.expect("Evocation is bound");
            fight.activate_aura(aura);
        }
    }

    fn on_dot_expire(fight: &mut Fight<Self>, _dot: DotId, behavior: MageSpell) {
        if behavior == MageSpell::Evocation {
            let (aura, _) = fight.agent.evocation_regen.expect("Evocation is bound");
            fight.deactivate_aura(aura);
        }
    }

    fn on_dot_tick(fight: &mut Fight<Self>, dot: DotId, behavior: MageSpell) {
        if matches!(
            behavior,
            MageSpell::Fireball | MageSpell::FrostfireBolt | MageSpell::Pyroblast
        ) {
            fight.snapshot_dot_tick(dot);
            return;
        }
        if behavior == MageSpell::Ignite {
            Self::ignite(fight).tick(fight, dot);
            return;
        }
        if behavior == MageSpell::Flamestrike {
            let spell = fight.dots[dot].spell;
            let (_, tick_base) = *fight
                .agent
                .flamestrike_ticks
                .iter()
                .find(|(rank, _)| *rank == spell)
                .expect("every Flamestrike rank has a tick amount");
            flamestrike::tick(fight, dot, tick_base);
            return;
        }
        if behavior == MageSpell::Blizzard {
            let state = fight.agent.blizzard.expect("Blizzard is bound");
            let side = fight.dots[dot].side;
            state.on_channel_tick(fight, side);
            return;
        }
        if behavior == MageSpell::ArcaneMissiles {
            let channel = fight.dots[dot].spell;
            let side = fight.dots[dot].side;
            let missile = fight
                .agent
                .missiles
                .iter()
                .find(|(spell, _)| *spell == channel)
                .map(|(_, missile)| *missile)
                .expect("every channel rank has a missile");
            fight.cast(missile, side);
        }
    }

    fn on_gain(fight: &mut Fight<Self>, aura: AuraRef, kind: MageAura) {
        match kind {
            MageAura::IceBarrier | MageAura::FrigidPower => {
                Self::ice_barrier(fight).on_gain(fight, aura)
            }
            MageAura::WintersChill => Self::winters_chill(fight).on_gain(fight),
            MageAura::FingersOfFrost => {
                Self::with_fingers(fight, |state, fight| state.on_gain(fight))
            }
            MageAura::Clearcasting => Self::arcane_concentration(fight).on_gain(fight),
            MageAura::MissileBarrage => Self::missile_barrage(fight).on_gain(fight),
            MageAura::ArcaneCharges => Self::arcane_charges(fight).on_gain(fight),
            MageAura::ArcanePower => Self::arcane_power(fight).on_gain(fight),
            MageAura::FireVulnerability => Self::improved_scorch(fight).on_gain(fight),
            MageAura::HeatingUp => Self::heating_up(fight).on_gain(fight),
            MageAura::Combustion => Self::combustion(fight).on_gain(fight),
            MageAura::PresenceOfMind => Self::presence_of_mind(fight).on_gain(fight),
            MageAura::EvocationRegen => {
                let (_, multiplier) = fight.agent.evocation_regen.expect("Evocation is bound");
                evocation::regen_gain(fight, multiplier);
            }
            _ => {}
        }
    }

    fn on_expire(fight: &mut Fight<Self>, aura: AuraRef, kind: MageAura) {
        match kind {
            MageAura::IceBarrier | MageAura::FrigidPower => {
                Self::ice_barrier(fight).on_expire(fight, aura)
            }
            MageAura::WintersChill => Self::winters_chill(fight).on_expire(fight),
            MageAura::FingersOfFrost => {
                Self::with_fingers(fight, |state, fight| state.on_expire(fight))
            }
            MageAura::Clearcasting => Self::arcane_concentration(fight).on_expire(fight),
            MageAura::MissileBarrage => Self::missile_barrage(fight).on_expire(fight),
            MageAura::ArcaneCharges => Self::arcane_charges(fight).on_expire(fight),
            MageAura::ArcanePower => Self::arcane_power(fight).on_expire(fight),
            MageAura::FireVulnerability => Self::improved_scorch(fight).on_expire(fight),
            MageAura::HeatingUp => Self::heating_up(fight).on_expire(fight),
            MageAura::Combustion => Self::combustion(fight).on_expire(fight),
            MageAura::PresenceOfMind => Self::presence_of_mind(fight).on_expire(fight),
            MageAura::EvocationRegen => {
                let (_, multiplier) = fight.agent.evocation_regen.expect("Evocation is bound");
                evocation::regen_expire(fight, multiplier);
            }
            _ => {}
        }
    }

    fn on_stacks_change(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: MageAura,
        _old: i32,
        new: i32,
    ) {
        match kind {
            MageAura::WintersChill => Self::winters_chill(fight).on_stacks_change(fight, new),
            MageAura::ArcaneCharges => Self::arcane_charges(fight).on_stacks_change(fight, new),
            MageAura::FireVulnerability => {
                Self::improved_scorch(fight).on_stacks_change(fight, new)
            }
            MageAura::HeatingUp => Self::heating_up(fight).on_stacks_change(fight, new),
            MageAura::Combustion => Self::combustion(fight).on_stacks_change(fight, new),
            _ => {}
        }
    }

    fn on_spell_hit_dealt(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: MageAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        match kind {
            MageAura::WintersChillTrigger => {
                Self::winters_chill(fight).on_spell_hit_dealt(fight, spell, result)
            }
            MageAura::ArcaneConcentrationTrigger => {
                Self::arcane_concentration(fight).on_spell_hit_dealt(fight, spell, result)
            }
            MageAura::MissileBarrageTrigger => {
                Self::missile_barrage(fight).trigger(fight, spell, result)
            }
            MageAura::IgniteTrigger => Self::ignite(fight).on_spell_hit_dealt(fight, spell, result),
            MageAura::HeatingUpTrigger => {
                Self::heating_up(fight).on_spell_hit_dealt(fight, spell, result)
            }
            MageAura::Combustion => {
                Self::combustion(fight).on_spell_hit_dealt(fight, spell, result)
            }
            MageAura::MasterOfElementsTrigger => {
                let state = fight
                    .agent
                    .master_of_elements
                    .clone()
                    .expect("Master of Elements is bound");
                state.on_spell_hit_dealt(fight, spell, result)
            }
            MageAura::FingersOfFrostTrigger
                if Self::with_fingers(fight, |state, fight| {
                    state.should_proc(fight, spell, result)
                }) =>
            {
                let aura = fight.agent.fingers_of_frost.as_ref().expect("bound").aura;
                fight.activate_aura(aura);
                let max = fight.aura(aura).max_stacks;
                fight.set_stacks(aura, max);
            }
            _ => {}
        }
    }

    fn on_cast_complete(fight: &mut Fight<Self>, aura: AuraRef, kind: MageAura, spell: SpellId) {
        match kind {
            MageAura::FingersOfFrost
                if Self::with_fingers(fight, |state, fight| {
                    state.on_cast_complete(fight, spell)
                }) =>
            {
                // Go OnCastComplete runs after the damage roll, so the consuming cast keeps the bonus.
                // A spell that travels spends the charge when it lands (patch 139), so an instant
                // cast while it flies still sees the last one; the landing's own damage and procs
                // at that instant come after.
                // Ice Lance spends its charge at cast even though it travels (patch 144).
                let travel = fight.travel_time(spell);
                if travel <= 0 || fight.spells[spell].class_spell.as_deref() == Some("ice_lance") {
                    fight.remove_stack(aura);
                } else {
                    fight.schedule_class_action(
                        fight.now + travel,
                        crate::core::fight::PRIORITY_AUTO,
                        FINGERS_OF_FROST_LANDING,
                    );
                }
            }
            MageAura::Clearcasting => {
                Self::arcane_concentration(fight).on_cast_complete(fight, spell)
            }
            MageAura::MissileBarrage => Self::missile_barrage(fight).on_cast_complete(fight, spell),
            MageAura::WintersChill => Self::winters_chill(fight).on_cast_complete(fight, spell),
            MageAura::HeatingUp => Self::heating_up(fight).on_cast_complete(fight, spell),
            MageAura::HeatingUpFireBlast => {
                Self::heating_up(fight).on_fire_blast_cast(fight, spell)
            }
            MageAura::PresenceOfMind => {
                Self::presence_of_mind(fight).on_cast_complete(fight, spell)
            }
            _ => {}
        }
    }
}
