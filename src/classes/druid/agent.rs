//! The Druid class agent for the fight runtime: class spells and auras by name, and the
//! hooks that dispatch to each spell's and talent's module.

use std::rc::Rc;

use crate::{
    contracts::prepared_v2::{Effect, PreparedV2, Spell as ExportedSpell, Weapon},
    core::fight::{heals::HealBook, Agent, AuraRef, DotId, Fight, Side, SpellId, SpellResult},
};

use super::{
    forms::{self, Forms},
    items::{self, NaturesBounty, UnendingLifeRefund},
    spells::{
        bear::{self, Barkskin, Enrage, FrenziedRegeneration, Lacerate, Maul},
        bear_form::{self, BearForm},
        bloom::Bloom,
        cat_builders::{Builder, CatBuilders},
        cat_form::{self, CatForm},
        faerie_fire::FaerieFire,
        ferocious_bite::FerociousBite,
        hurricane, innervate, insect_swarm, moonfire,
        per_dot::PerDot,
        prowl::{self, Prowl},
        rake::{self, Rake},
        rip::{self, Rip},
        shifting_power::{self, ShiftingPower},
        starfire, swiftmend, wrath,
    },
    talents::{
        berserk::{self, Berserk},
        blood_frenzy::{self, BloodFrenzy},
        eclipse,
        natural_reaction::{self, NaturalReaction},
        natures_grace, omen_of_clarity, rend_and_tear,
    },
};

/// What a Druid spell does when its effects apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DruidSpell {
    MoonkinForm,
    Starfire,
    Wrath,
    Moonfire,
    MoonfireDot,
    InsectSwarm,
    Innervate,
    CatForm,
    BearForm,
    Prowl,
    Builder(Builder),
    Rake,
    Rip,
    FerociousBite,
    ShiftingPower,
    FaerieFire,
    Berserk,
    Enrage,
    DemoralizingRoar,
    Maul,
    MaulQueue,
    Lacerate,
    PrimalBite,
    Swipe,
    /// Hurricane's channel.
    Hurricane,
    /// The spell each Hurricane period casts.
    HurricaneTick,
    Barkskin,
    /// thorns.go: the cast turns the druid's own Thorns on.
    Thorns,
    FrenziedRegeneration,
    /// A survival cooldown Go never casts without a health threshold: only its cast checks run.
    Survival,
    /// A heal of MythicSim's healer sims (`core::fight::heals`).
    Heal,
    /// Swiftmend, which consumes a Rejuvenation or Regrowth on the player.
    Swiftmend,
}

/// Class auras with Rust behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DruidAura {
    Clearcasting,
    OmenOfClarity,
    NaturesGrace,
    NaturesGraceTrigger,
    Eclipse,
    EclipseTrigger,
    Innervate,
    CatForm,
    Prowl,
    Berserk,
    BloodFrenzy,
    /// Blood Frenzy's bear half, which acts only in Bear Form.
    BloodFrenzyBear,
    /// The target's Faerie Fire.
    FaerieFire,
    BearForm,
    Enrage,
    NaturalReaction,
    Barkskin,
    NaturesBounty,
    UnendingLifeRefund,
}

/// The weapons of one item swap set: in caster form and the paw of each form.
#[derive(Clone, Debug, Default)]
struct FormWeapons {
    cat: Option<(Weapon, Weapon)>,
    bear: Option<(Weapon, Weapon)>,
}

impl FormWeapons {
    fn of(effects: &[Effect]) -> FormWeapons {
        let mut weapons = FormWeapons::default();
        for effect in effects {
            match effect {
                Effect::CatForm {
                    main_hand,
                    cat_weapon,
                    ..
                } => weapons.cat = Some((main_hand.clone(), cat_weapon.clone())),
                Effect::BearForm {
                    main_hand,
                    bear_weapon,
                    ..
                } => weapons.bear = Some((main_hand.clone(), bear_weapon.clone())),
                _ => {}
            }
        }
        weapons
    }
}

/// Druid state that Go keeps in the `Druid` struct and its closures.
pub(crate) struct DruidAgent {
    /// The Restoration druid's heals, and the ranks Swiftmend consumes, Rejuvenation first.
    heals: Option<Rc<HealBook>>,
    swiftmend_hots: Vec<usize>,
    /// Bloom (hotfix push 113531, Forever's rules): the heal a heal over time's end makes.
    bloom: Option<Rc<Bloom>>,
    /// With an item swap, each set's weapon in caster form and paw, Main then Swap1.
    form_weapons: Option<[FormWeapons; 2]>,
    moonkin_form: Option<AuraRef>,
    moonfire_dot: Option<SpellId>,
    insect_swarm_debuff: Option<AuraRef>,
    innervate: Option<Rc<innervate::Innervate>>,
    omen: Option<Rc<omen_of_clarity::OmenOfClarity>>,
    natures_grace: Option<Rc<natures_grace::NaturesGrace>>,
    eclipse: Option<Rc<eclipse::Eclipse>>,
    forms: Option<Rc<Forms>>,
    /// Go `Druid.form`.
    pub(crate) form: u8,
    /// Go `lastCatFormEnergy` and `lastCatFormExitAt`, which no reset clears.
    pub(crate) last_cat_form_energy: f64,
    pub(crate) last_cat_form_exit: i64,
    pub(crate) cat_form: Option<Rc<CatForm>>,
    pub(crate) prowl: Option<Rc<Prowl>>,
    builders: Option<Rc<CatBuilders>>,
    rip: Option<Rc<Rip>>,
    pub(crate) rip_snapshot: PerDot<rip::Snapshot>,
    rake: Option<Rc<Rake>>,
    /// Rake's stored tick and attack power.
    pub(crate) rake_snapshot: PerDot<rake::Snapshot>,
    ferocious_bite: Option<FerociousBite>,
    shifting_power: Option<ShiftingPower>,
    faerie_fire: Option<FaerieFire>,
    pub(crate) berserk: Option<Berserk>,
    blood_frenzy: Option<Rc<BloodFrenzy>>,
    pub(crate) bear_form: Option<Rc<BearForm>>,
    pub(crate) enrage: Option<Enrage>,
    demoralizing_roar: Option<crate::core::fight::AuraRef>,
    pub(crate) maul: Option<Maul>,
    /// Go `isMaulQueued`.
    pub(crate) maul_queued: bool,
    lacerate: Option<Lacerate>,
    /// The bleed's stored tick.
    pub(crate) lacerate_snapshot: PerDot<f64>,
    primal_bite: Option<f64>,
    /// Swipe's flat damage and attack power share.
    swipe: Option<(f64, f64)>,
    hurricane: Option<hurricane::Hurricane>,
    natural_reaction: Option<NaturalReaction>,
    pub(crate) frenzied_regeneration: Option<AuraRef>,
    barkskin: Option<Barkskin>,
    /// The druid's own Thorns aura.
    thorns: Option<AuraRef>,
    frenzied_regeneration_spell: Option<FrenziedRegeneration>,
    natures_bounty: Option<Rc<NaturesBounty>>,
    unending_life: Option<Rc<UnendingLifeRefund>>,
}

impl Default for DruidAgent {
    fn default() -> Self {
        DruidAgent {
            heals: None,
            swiftmend_hots: Vec::new(),
            bloom: None,
            moonkin_form: None,
            moonfire_dot: None,
            insect_swarm_debuff: None,
            innervate: None,
            omen: None,
            natures_grace: None,
            eclipse: None,
            forms: None,
            form: forms::HUMANOID,
            last_cat_form_energy: 0.0,
            last_cat_form_exit: 0,
            cat_form: None,
            prowl: None,
            builders: None,
            rip: None,
            rip_snapshot: PerDot::default(),
            rake: None,
            rake_snapshot: PerDot::default(),
            ferocious_bite: None,
            shifting_power: None,
            faerie_fire: None,
            berserk: None,
            blood_frenzy: None,
            bear_form: None,
            form_weapons: None,
            enrage: None,
            demoralizing_roar: None,
            maul: None,
            maul_queued: false,
            lacerate: None,
            lacerate_snapshot: PerDot::default(),
            primal_bite: None,
            swipe: None,
            hurricane: None,
            natural_reaction: None,
            frenzied_regeneration: None,
            barkskin: None,
            thorns: None,
            frenzied_regeneration_spell: None,
            natures_bounty: None,
            unending_life: None,
        }
    }
}

/// Player aura labels claimed by implemented class effects.
fn class_auras(prepared: &PreparedV2) -> Vec<(String, DruidAura)> {
    let mut auras = Vec::new();
    for effect in &prepared.effects {
        match effect {
            Effect::OmenOfClarity {
                aura, trigger_aura, ..
            } => {
                auras.push((aura.clone(), DruidAura::Clearcasting));
                auras.push((trigger_aura.clone(), DruidAura::OmenOfClarity));
            }
            Effect::NaturesGrace {
                aura, trigger_aura, ..
            } => {
                auras.push((aura.clone(), DruidAura::NaturesGrace));
                auras.push((trigger_aura.clone(), DruidAura::NaturesGraceTrigger));
            }
            Effect::Eclipse {
                aura, trigger_aura, ..
            } => {
                auras.push((aura.clone(), DruidAura::Eclipse));
                auras.push((trigger_aura.clone(), DruidAura::EclipseTrigger));
            }
            Effect::Innervate { aura, .. } => auras.push((aura.clone(), DruidAura::Innervate)),
            Effect::CatForm { aura, .. } => auras.push((aura.clone(), DruidAura::CatForm)),
            Effect::Prowl { aura, .. } => auras.push((aura.clone(), DruidAura::Prowl)),
            Effect::Berserk { aura, .. } => auras.push((aura.clone(), DruidAura::Berserk)),
            Effect::BloodFrenzy {
                trigger_aura,
                bear_trigger_aura,
                ..
            } => {
                auras.push((trigger_aura.clone(), DruidAura::BloodFrenzy));
                auras.push((bear_trigger_aura.clone(), DruidAura::BloodFrenzyBear));
            }
            Effect::BearForm { aura, .. } => auras.push((aura.clone(), DruidAura::BearForm)),
            Effect::Enrage { aura, .. } => auras.push((aura.clone(), DruidAura::Enrage)),
            Effect::Barkskin { aura, .. } => auras.push((aura.clone(), DruidAura::Barkskin)),
            Effect::NaturesBounty { aura, .. } => {
                auras.push((aura.clone(), DruidAura::NaturesBounty))
            }
            Effect::UnendingLifeRefund { aura, .. } => {
                auras.push((aura.clone(), DruidAura::UnendingLifeRefund))
            }
            Effect::NaturalReaction { trigger_aura, .. } => {
                auras.push((trigger_aura.clone(), DruidAura::NaturalReaction))
            }
            _ => {}
        }
    }
    auras
}

/// The spell position an effect names, by kind.
fn effect_spell(prepared: &PreparedV2, position: usize) -> Option<DruidSpell> {
    prepared.effects.iter().find_map(|effect| match effect {
        Effect::Rip { spell, .. } if *spell == position => Some(DruidSpell::Rip),
        Effect::Rake { spell, .. } if *spell == position => Some(DruidSpell::Rake),
        Effect::FerociousBite { spell, .. } if *spell == position => {
            Some(DruidSpell::FerociousBite)
        }
        Effect::ShiftingPower { spell, .. } if *spell == position => {
            Some(DruidSpell::ShiftingPower)
        }
        Effect::FaerieFire { spell, .. } if *spell == position => Some(DruidSpell::FaerieFire),
        Effect::DemoralizingRoar { spell, .. } if *spell == position => {
            Some(DruidSpell::DemoralizingRoar)
        }
        Effect::Maul { spell, .. } if *spell == position => Some(DruidSpell::Maul),
        Effect::Maul { queue_spell, .. } if *queue_spell == position => Some(DruidSpell::MaulQueue),
        Effect::Lacerate { spell, .. } if *spell == position => Some(DruidSpell::Lacerate),
        Effect::PrimalBite { spell, .. } if *spell == position => Some(DruidSpell::PrimalBite),
        Effect::Swipe { spell, .. } if *spell == position => Some(DruidSpell::Swipe),
        Effect::FrenziedRegeneration { spell, .. } if *spell == position => {
            Some(DruidSpell::FrenziedRegeneration)
        }
        Effect::CatBuilders { builders, .. } => builders
            .iter()
            .find(|builder| builder.spell == position)
            .and_then(|builder| Builder::parse(&builder.kind))
            .map(DruidSpell::Builder),
        _ => None,
    })
}

impl DruidAgent {
    /// The class behavior of an exported spell, if Rust implements it.
    pub(crate) fn spell(
        prepared: &PreparedV2,
        position: usize,
        spell: &ExportedSpell,
    ) -> Option<DruidSpell> {
        let id = spell.action_id.clone().unwrap_or_default();
        // Spells Go registers without a class mask, named by their effects.
        for effect in &prepared.effects {
            match effect {
                Effect::Prowl { spell_id, .. } if *spell_id == id.spell_id && id.tag == 0 => {
                    return Some(DruidSpell::Prowl)
                }
                Effect::Berserk { spell_id, .. } if *spell_id == id.spell_id && id.tag == 0 => {
                    return Some(DruidSpell::Berserk)
                }
                Effect::Enrage { spell_id, .. } if *spell_id == id.spell_id && id.tag == 0 => {
                    return Some(DruidSpell::Enrage)
                }
                Effect::Barkskin { spell_id, .. } if *spell_id == id.spell_id && id.tag == 0 => {
                    return Some(DruidSpell::Barkskin)
                }
                Effect::DruidThorns { spell_id, .. } if *spell_id == id.spell_id && id.tag == 0 => {
                    return Some(DruidSpell::Thorns)
                }
                _ => {}
            }
        }
        // The other spells an effect names by position.
        if let Some(behavior) = effect_spell(prepared, position) {
            return Some(behavior);
        }
        // A druid survival cooldown without timings, whose cast checks still run.
        let survival = prepared.player.major_cooldowns.iter().any(|cooldown| {
            cooldown.action_id == id
                && cooldown.kind.iter().any(|kind| kind == "survival")
                && cooldown.timings_ns.is_empty()
        });
        let druid_spell = prepared.effects.iter().any(|effect| {
            matches!(effect, Effect::DruidForms { spells, .. }
                if spells.iter().any(|entry| entry.spell == position))
        });
        if survival && druid_spell {
            return Some(DruidSpell::Survival);
        }
        match spell.class_spell.as_deref()? {
            "heal" => Some(DruidSpell::Heal),
            "swiftmend" => Some(DruidSpell::Swiftmend),
            "moonkin_form" => Some(DruidSpell::MoonkinForm),
            "starfire" if spell.damage_effect.is_some() => Some(DruidSpell::Starfire),
            "wrath" if spell.damage_effect.is_some() => Some(DruidSpell::Wrath),
            "moonfire" if spell.damage_effect.is_some() => Some(DruidSpell::Moonfire),
            "moonfire_dot" if spell.dot.is_some() => Some(DruidSpell::MoonfireDot),
            "insect_swarm" if spell.dot.is_some() => Some(DruidSpell::InsectSwarm),
            "hurricane" if spell.dot.is_some() => Some(DruidSpell::Hurricane),
            "hurricane" => Some(DruidSpell::HurricaneTick),
            "innervate" => Some(DruidSpell::Innervate),
            "cat_form" => prepared
                .effects
                .iter()
                .any(|effect| matches!(effect, Effect::CatForm { .. }))
                .then_some(DruidSpell::CatForm),
            "bear_form" => prepared
                .effects
                .iter()
                .any(
                    |effect| matches!(effect, Effect::BearForm { spell, .. } if *spell == position),
                )
                .then_some(DruidSpell::BearForm),
            "rip" | "ferocious_bite" | "shifting_power" | "faerie_fire" | "shred" | "claw"
            | "ravage" => effect_spell(prepared, position),
            _ => None,
        }
    }

    /// Build a fight for a prepared input that passed the coverage gate.
    pub(crate) fn fight(prepared: &PreparedV2) -> Result<Fight<DruidAgent>, String> {
        let auras = class_auras(prepared);
        let target_auras: Vec<(String, DruidAura)> = prepared
            .effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::FaerieFire { aura, .. } => Some((aura.clone(), DruidAura::FaerieFire)),
                _ => None,
            })
            .collect();
        let positions: Vec<*const ExportedSpell> = prepared
            .player
            .spells
            .iter()
            .map(|spell| spell as *const ExportedSpell)
            .collect();
        let mut fight = Fight::new(
            prepared,
            DruidAgent::default(),
            |exported| {
                let position = positions
                    .iter()
                    .position(|&spell| std::ptr::eq(spell, exported))?;
                DruidAgent::spell(prepared, position, exported)
            },
            |unit, label| match unit {
                "player" => auras
                    .iter()
                    .find(|(name, _)| name == label)
                    .map(|(_, kind)| *kind),
                _ => target_auras
                    .iter()
                    .find(|(name, _)| name == label)
                    .map(|(_, kind)| *kind),
            },
        )?;
        let find_spell = |fight: &Fight<DruidAgent>, id: i32, tag: i32| {
            fight.spells.iter().position(|spell| {
                spell.id.spell_id == id && spell.id.tag == tag && spell.id.item_id == 0
            })
        };
        let spell_count = fight.spells.len();
        // Go AddStatsDynamic: a stat aura's bit in the stat combinations.
        let stat_bit = |label: &str| {
            prepared.effects.iter().find_map(|effect| match effect {
                Effect::StatAuras { auras, .. } => auras
                    .iter()
                    .position(|aura| aura == label)
                    .map(|bit| 1u32 << bit),
                _ => None,
            })
        };
        for effect in &prepared.effects {
            match effect {
                Effect::Heals {
                    heals,
                    player,
                    bonus_healing_taken,
                    party,
                } => {
                    let book = HealBook::bind(
                        &fight,
                        heals,
                        *player,
                        *bonus_healing_taken,
                        party.as_ref(),
                    )?;
                    let rank_of = |id: &i32| heals.iter().position(|heal| heal.spell_id == *id);
                    fight.agent.swiftmend_hots = swiftmend::CONSUMES
                        .iter()
                        .flat_map(|ids| ids.iter().rev().filter_map(rank_of))
                        .collect();
                    fight.agent.heals = Some(Rc::new(book));
                }
                Effect::Bloom { spell_id, hots } => {
                    let spirit = prepared
                        .player
                        .stats
                        .get("Spirit")
                        .copied()
                        .ok_or("prepared stats lack Spirit")?;
                    let bound = Bloom::bind(&fight, *spell_id, spirit, hots)?;
                    fight.agent.bloom = Some(Rc::new(bound));
                }
                Effect::DruidForms {
                    starting_form,
                    spells,
                } => {
                    let bound = Forms::new(spell_count, starting_form, spells);
                    fight.agent.form = bound.starting;
                    fight.agent.forms = Some(Rc::new(bound));
                }
                Effect::MoonkinForm { aura, .. } => {
                    fight.agent.moonkin_form = Some(fight.player_aura(aura)?);
                }
                Effect::Moonfire { rank } => {
                    let dot_spell = find_spell(&fight, rank.spell_id, 1)
                        .ok_or("Moonfire's dot spell is not registered")?;
                    let dot = fight.spells[dot_spell]
                        .dot
                        .ok_or("Moonfire's dot spell has no dot")?;
                    fight.dots[dot].tick_base = Some(rank.tick_base);
                    fight.dots[dot].tick_can_crit = rank.tick_can_crit;
                    fight.agent.moonfire_dot = Some(dot_spell);
                }
                Effect::InsectSwarm { rank, debuff_aura } => {
                    let spell = find_spell(&fight, rank.spell_id, 0)
                        .ok_or("Insect Swarm is not registered")?;
                    let dot = fight.spells[spell].dot.ok_or("Insect Swarm has no dot")?;
                    fight.dots[dot].tick_base = Some(rank.tick_base);
                    fight.dots[dot].tick_can_crit = rank.tick_can_crit;
                    fight.agent.insect_swarm_debuff = fight.trackers[Side::Target.index()]
                        .find(debuff_aura)
                        .map(|index| AuraRef {
                            side: Side::Target,
                            index,
                        });
                }
                Effect::Innervate {
                    aura,
                    spirit_regen_multiplier,
                    regen_metrics_action_id,
                    ..
                } => {
                    let bound = innervate::bind(
                        &mut fight,
                        aura,
                        *spirit_regen_multiplier,
                        regen_metrics_action_id,
                    )?;
                    fight.agent.innervate = Some(Rc::new(bound));
                }
                Effect::OmenOfClarity {
                    trigger_aura,
                    aura,
                    trigger_immediately,
                    proc_chance,
                    trigger_spells,
                    icd_ns,
                    ppm,
                    gcd_ns,
                    moonkin_chance_multiplier,
                    moonkin_cooldown_multiplier,
                    cost_spells,
                    cost_percent_add,
                    form_excluded_spells,
                    ..
                } => {
                    let moonkin = prepared.effects.iter().find_map(|effect| match effect {
                        Effect::MoonkinForm { aura, .. } => Some(aura.as_str()),
                        _ => None,
                    });
                    let spell_masked = prepared
                        .player
                        .spells
                        .iter()
                        .map(|spell| {
                            spell.proc_mask.iter().any(|mask| {
                                mask == "ProcMaskSpellDamage" || mask == "ProcMaskSpellHealing"
                            })
                        })
                        .collect();
                    let bound = omen_of_clarity::bind(
                        &mut fight,
                        omen_of_clarity::Params {
                            aura,
                            trigger: trigger_aura,
                            trigger_spells,
                            form_excluded_spells,
                            spell_masked,
                            cost_spells,
                            icd: *icd_ns,
                            ppm: *ppm,
                            gcd: *gcd_ns,
                            moonkin,
                            moonkin_chance: *moonkin_chance_multiplier,
                            moonkin_cooldown: *moonkin_cooldown_multiplier,
                            trigger_immediately: *trigger_immediately,
                            proc_chance: *proc_chance,
                            cost_percent_add: *cost_percent_add,
                        },
                    )?;
                    fight.agent.omen = Some(Rc::new(bound));
                }
                Effect::NaturesGrace {
                    aura,
                    haste_multiplier,
                    gcd_reduction_ns,
                    gcd_spells,
                    trigger_spells,
                    ..
                } => {
                    let bound = natures_grace::bind(
                        &mut fight,
                        aura,
                        *haste_multiplier,
                        *gcd_reduction_ns,
                        gcd_spells,
                        trigger_spells,
                    )?;
                    fight.agent.natures_grace = Some(Rc::new(bound));
                }
                Effect::Eclipse {
                    aura,
                    cast_time_reduction_ns,
                    charges_per_wrath,
                    ..
                } => {
                    let bound = eclipse::bind(
                        &mut fight,
                        aura,
                        *cast_time_reduction_ns,
                        *charges_per_wrath,
                    )?;
                    fight.agent.eclipse = Some(Rc::new(bound));
                }
                Effect::CatForm {
                    spell_id,
                    aura,
                    initial_threat_multiplier,
                    threat_multiplier,
                    initial_spirit_regen_multiplier,
                    spirit_regen_multiplier,
                    furor_max,
                    cost_spells,
                    gcd_spells,
                    gcd_delta_ns,
                    form_breaking_spells,
                    main_hand,
                    cat_weapon,
                    ..
                } => {
                    let spell = find_spell(&fight, *spell_id, 0)
                        .ok_or("Cat Form's spell is not registered")?;
                    let stat_bit = stat_bit(aura).ok_or("Cat Form is not a stat aura")?;
                    let bound = cat_form::bind(
                        &mut fight,
                        cat_form::Params {
                            spell,
                            spell_id: *spell_id,
                            aura,
                            stat_bit,
                            initial_threat_multiplier: *initial_threat_multiplier,
                            threat_multiplier: *threat_multiplier,
                            initial_spirit_regen_multiplier: *initial_spirit_regen_multiplier,
                            spirit_regen_multiplier: *spirit_regen_multiplier,
                            furor_max: *furor_max,
                            cost_spells,
                            gcd_spells,
                            gcd_delta: *gcd_delta_ns,
                            form_breaking_spells,
                            main_hand,
                            cat_weapon,
                        },
                    )?;
                    fight.agent.cat_form = Some(Rc::new(bound));
                }
                Effect::Prowl {
                    spell_id,
                    aura,
                    movement_speed_multiplier,
                } => {
                    let spell =
                        find_spell(&fight, *spell_id, 0).ok_or("Prowl is not registered")?;
                    let bound = prowl::bind(&mut fight, spell, aura, *movement_speed_multiplier)?;
                    fight.agent.prowl = Some(Rc::new(bound));
                }
                Effect::CatBuilders {
                    builders,
                    cannot_shred,
                } => {
                    let mut flat_damage = vec![None; spell_count];
                    for builder in builders {
                        if let Some(slot) = flat_damage.get_mut(builder.spell) {
                            *slot = Some(builder.flat_damage);
                        }
                    }
                    fight.agent.builders = Some(Rc::new(CatBuilders {
                        flat_damage,
                        cannot_shred: *cannot_shred,
                    }));
                }
                Effect::Rip {
                    tick_base,
                    tick_per_combo_point,
                    attack_power_share_per_combo_point,
                    attack_power_share_max_points,
                    tick_can_crit,
                    expected_combo_points,
                    short_name,
                    ..
                } => {
                    fight.agent.rip = Some(Rc::new(Rip {
                        tick_base: *tick_base,
                        tick_per_combo_point: *tick_per_combo_point,
                        share_per_combo_point: *attack_power_share_per_combo_point,
                        share_max_points: *attack_power_share_max_points,
                        tick_can_crit: *tick_can_crit,
                        expected_combo_points: *expected_combo_points,
                        short_name: short_name.clone(),
                    }));
                }
                Effect::Rake {
                    flat_damage,
                    tick_base,
                    attack_power_share,
                    tick_can_crit,
                    short_name,
                    ..
                } => {
                    fight.agent.rake = Some(Rc::new(Rake {
                        flat_damage: *flat_damage,
                        tick_base: *tick_base,
                        attack_power_share: *attack_power_share,
                        tick_can_crit: *tick_can_crit,
                        short_name: short_name.clone(),
                    }));
                }
                Effect::FerociousBite {
                    damage_per_energy,
                    damage_per_combo_point,
                    attack_power_per_combo_point,
                    keeps_energy,
                    ..
                } => {
                    fight.agent.ferocious_bite = Some(FerociousBite {
                        damage_per_energy: *damage_per_energy,
                        damage_per_combo_point: *damage_per_combo_point,
                        attack_power_per_combo_point: *attack_power_per_combo_point,
                        keeps_energy: *keeps_energy,
                    });
                }
                Effect::ShiftingPower { spell, energy } => {
                    let bound = shifting_power::bind(&mut fight, *spell, *energy);
                    fight.agent.shifting_power = Some(bound);
                }
                Effect::FaerieFire {
                    aura,
                    armor_reduction,
                    refresh,
                    ..
                } => {
                    let index = fight.trackers[Side::Target.index()]
                        .find(aura)
                        .ok_or_else(|| format!("target aura {aura} is not registered"))?;
                    fight.agent.faerie_fire = Some(FaerieFire {
                        aura: AuraRef {
                            side: Side::Target,
                            index,
                        },
                        armor: (refresh.as_slice() == ["own"]).then_some(*armor_reduction),
                    });
                }
                Effect::Berserk {
                    aura,
                    crit_percent,
                    crit_spells,
                    ..
                } => {
                    let bound = berserk::bind(&mut fight, aura, *crit_percent, crit_spells)?;
                    fight.agent.berserk = Some(bound);
                }
                Effect::BloodFrenzy {
                    trigger_aura,
                    bear_trigger_aura,
                    proc_chance,
                    trigger_spells,
                    trigger_immediately,
                    metrics_action_id,
                    bear_trigger_spells,
                    bear_rage,
                    ..
                } => {
                    let bound = blood_frenzy::bind(
                        &mut fight,
                        blood_frenzy::Params {
                            trigger: trigger_aura,
                            bear_trigger: bear_trigger_aura,
                            proc_chance: *proc_chance,
                            trigger_spells,
                            trigger_immediately: *trigger_immediately,
                            metrics_action_id,
                            bear_trigger_spells,
                            bear_rage: *bear_rage,
                        },
                    )?;
                    fight.agent.blood_frenzy = Some(Rc::new(bound));
                }
                Effect::BearForm {
                    spell_id,
                    aura,
                    spell,
                    health_bonus,
                    initial_threat_multiplier,
                    threat_multiplier,
                    initial_spirit_regen_multiplier,
                    spirit_regen_multiplier,
                    furor_proc_chance,
                    cost_spells,
                    form_breaking_spells,
                    main_hand,
                    bear_weapon,
                } => {
                    let stat_bit = stat_bit(aura).ok_or("Bear Form is not a stat aura")?;
                    let bound = bear_form::bind(
                        &mut fight,
                        bear_form::Params {
                            spell: *spell,
                            spell_id: *spell_id,
                            aura,
                            stat_bit,
                            health_bonus: *health_bonus,
                            initial_threat_multiplier: *initial_threat_multiplier,
                            threat_multiplier: *threat_multiplier,
                            initial_spirit_regen_multiplier: *initial_spirit_regen_multiplier,
                            spirit_regen_multiplier: *spirit_regen_multiplier,
                            furor_proc_chance: *furor_proc_chance,
                            cost_spells,
                            form_breaking_spells,
                            main_hand,
                            bear_weapon,
                        },
                    )?;
                    fight.agent.bear_form = Some(Rc::new(bound));
                    fight.agent.frenzied_regeneration =
                        fight.player_aura("Frenzied Regeneration").ok();
                }
                Effect::Enrage {
                    spell_id,
                    aura,
                    instant_rage,
                    rage_per_tick,
                    ticks,
                    period_ns,
                } => {
                    let aura_ref = fight.player_aura(aura)?;
                    let stat_bit = stat_bit(aura).ok_or("Enrage is not a stat aura")?;
                    let metrics = fight.new_rage_metrics(crate::contracts::prepared_v2::ActionId {
                        spell_id: *spell_id,
                        ..Default::default()
                    });
                    fight.agent.enrage = Some(Enrage {
                        aura: aura_ref,
                        stat_bit,
                        instant_rage: *instant_rage,
                        rage_per_tick: *rage_per_tick,
                        ticks: *ticks,
                        period: *period_ns,
                        metrics,
                    });
                }
                Effect::DemoralizingRoar { aura, .. } => {
                    let index = fight.trackers[Side::Target.index()]
                        .find(aura)
                        .ok_or_else(|| format!("target aura {aura} is not registered"))?;
                    fight.agent.demoralizing_roar = Some(AuraRef {
                        side: Side::Target,
                        index,
                    });
                }
                Effect::Maul {
                    spell,
                    queue_aura,
                    realism_ns,
                    flat_damage,
                    ..
                } => {
                    // Go maulRealismICD: its own timer.
                    fight.timers.push(crate::core::time::STARTING_CD_TIME);
                    let timer = fight.timers.len() - 1;
                    fight.agent.maul = Some(Maul {
                        spell: *spell,
                        queue_aura: fight.player_aura(queue_aura)?,
                        realism: (timer, *realism_ns),
                        flat_damage: *flat_damage,
                    });
                }
                Effect::Lacerate {
                    tick_base,
                    weapon_share_per_stack,
                    max_stacks,
                    tick_can_crit,
                    ..
                } => {
                    fight.agent.lacerate = Some(Lacerate {
                        tick_base: *tick_base,
                        weapon_share_per_stack: *weapon_share_per_stack,
                        max_stacks: *max_stacks,
                        tick_can_crit: *tick_can_crit,
                    });
                }
                Effect::Swipe {
                    flat_damage,
                    attack_power_coefficient,
                    ..
                } => {
                    fight.agent.swipe = Some((*flat_damage, *attack_power_coefficient));
                }
                Effect::Hurricane {
                    spell_id,
                    tick_spell_id,
                    tick_base,
                } => {
                    let bound = hurricane::bind(&fight, *spell_id, *tick_spell_id, *tick_base)?;
                    fight.agent.hurricane = Some(bound);
                }
                Effect::PrimalBite { flat_damage, .. } => {
                    fight.agent.primal_bite = Some(*flat_damage);
                }
                Effect::NaturesBounty {
                    aura,
                    proc_chance,
                    mana_label,
                    mana,
                    mana_spells,
                    energy_label,
                    energy,
                    energy_spells,
                    rage_label,
                    rage,
                    metrics_action_id,
                } => {
                    let bound = items::bind_bounty(
                        &mut fight,
                        items::BountyParams {
                            aura,
                            proc_chance: *proc_chance,
                            labels: [mana_label, energy_label, rage_label],
                            amounts: [*mana, *energy, *rage],
                            mana_spells,
                            energy_spells,
                            metrics_action_id,
                        },
                    )?;
                    fight.agent.natures_bounty = Some(Rc::new(bound));
                }
                Effect::UnendingLifeRefund {
                    aura,
                    energy,
                    spells,
                    metrics_action_id,
                    ..
                } => {
                    let bound = items::bind_unending_life(
                        &mut fight,
                        aura,
                        *energy,
                        spells,
                        metrics_action_id,
                    )?;
                    fight.agent.unending_life = Some(Rc::new(bound));
                }
                Effect::DruidThorns { aura, .. } => {
                    fight.agent.thorns = Some(fight.player_aura(aura)?);
                }
                Effect::Barkskin { aura, .. } => {
                    let stat_bit = stat_bit(aura).ok_or("Barkskin is not a stat aura")?;
                    fight.agent.barkskin = Some(Barkskin {
                        aura: fight.player_aura(aura)?,
                        stat_bit,
                    });
                }
                Effect::FrenziedRegeneration {
                    spell,
                    aura,
                    ticks,
                    period_ns,
                    max_rage_per_tick,
                    health_share_per_rage,
                    healing_taken_multiplier,
                } => {
                    // Go registers the Rage metrics, then the health metrics.
                    let id = fight.spells[*spell].id.clone();
                    let rage_metrics = fight.new_rage_metrics(id.clone());
                    let health_metrics = fight.new_health_metrics(id);
                    fight.agent.frenzied_regeneration_spell = Some(FrenziedRegeneration {
                        aura: fight.player_aura(aura)?,
                        ticks: *ticks,
                        period: *period_ns,
                        max_rage_per_tick: *max_rage_per_tick,
                        health_share_per_rage: *health_share_per_rage,
                        healing_taken_multiplier: *healing_taken_multiplier,
                        rage_metrics,
                        health_metrics,
                    });
                }
                Effect::NaturalReaction {
                    trigger_aura,
                    proc_chance,
                    trigger_immediately,
                    rage,
                    metrics_action_id,
                    ..
                } => {
                    let bound = natural_reaction::bind(
                        &mut fight,
                        trigger_aura,
                        *proc_chance,
                        *trigger_immediately,
                        *rage,
                        metrics_action_id,
                    )?;
                    fight.agent.natural_reaction = Some(bound);
                }
                Effect::RendAndTear {
                    multiplier,
                    spells,
                    bleed_spells,
                } => rend_and_tear::bind(&mut fight, *multiplier, spells, bleed_spells)?,
                _ => {}
            }
        }
        if let Some(worn) = fight.item_swap_worn_effects() {
            fight.agent.form_weapons =
                Some([FormWeapons::of(&prepared.effects), FormWeapons::of(&worn)]);
        }
        Ok(fight)
    }

    fn innervate(fight: &Fight<Self>) -> Rc<innervate::Innervate> {
        fight.agent.innervate.clone().expect("Innervate is bound")
    }

    /// Omen of Clarity's trigger: since the 16 October 2026 beta notes Faerie Fire cannot set it
    /// off in Cat or Bear Form, which costs no resources there (the sim's Bear Form is Dire Bear
    /// Form too).
    fn omen_hit(fight: &mut Fight<Self>, spell: SpellId, result: &SpellResult) {
        let omen = Self::omen(fight);
        if omen.form_excluded[spell] && fight.agent.form & (forms::CAT | forms::BEAR) != 0 {
            return;
        }
        omen.on_spell_hit_dealt(fight, spell, result);
    }

    fn omen(fight: &Fight<Self>) -> Rc<omen_of_clarity::OmenOfClarity> {
        fight.agent.omen.clone().expect("Omen of Clarity is bound")
    }

    fn natures_grace(fight: &Fight<Self>) -> Rc<natures_grace::NaturesGrace> {
        fight
            .agent
            .natures_grace
            .clone()
            .expect("Nature's Grace is bound")
    }

    fn eclipse(fight: &Fight<Self>) -> Rc<eclipse::Eclipse> {
        fight.agent.eclipse.clone().expect("Eclipse is bound")
    }

    fn cat_form(fight: &Fight<Self>) -> Rc<CatForm> {
        fight.agent.cat_form.clone().expect("Cat Form is bound")
    }

    fn prowl(fight: &Fight<Self>) -> Rc<Prowl> {
        fight.agent.prowl.clone().expect("Prowl is bound")
    }

    fn builders(fight: &Fight<Self>) -> Rc<CatBuilders> {
        fight
            .agent
            .builders
            .clone()
            .expect("the cat builders are bound")
    }

    fn rip(fight: &Fight<Self>) -> Rc<Rip> {
        fight.agent.rip.clone().expect("Rip is bound")
    }

    fn blood_frenzy(fight: &Fight<Self>) -> Rc<BloodFrenzy> {
        fight
            .agent
            .blood_frenzy
            .clone()
            .expect("Blood Frenzy is bound")
    }
}

impl Agent for DruidAgent {
    type Spell = DruidSpell;
    type Aura = DruidAura;

    /// Go druid.go Initialize's swap callback: in Cat or Bear Form the paw is rebuilt from the
    /// weapon now in the hand; the forms read it from there when they next shift.
    fn on_item_swap(fight: &mut Fight<Self>, worn: bool) {
        let Some(weapons) = fight.agent.form_weapons.clone() else {
            return;
        };
        let set = &weapons[usize::from(worn)];
        if let (Some(cat), Some((main_hand, paw))) = (fight.agent.cat_form.as_mut(), &set.cat) {
            let cat = Rc::make_mut(cat);
            cat.main_hand = main_hand.clone();
            cat.cat_weapon = paw.clone();
        }
        if let (Some(bear), Some((main_hand, paw))) = (fight.agent.bear_form.as_mut(), &set.bear) {
            let bear = Rc::make_mut(bear);
            bear.main_hand = main_hand.clone();
            bear.bear_weapon = paw.clone();
        }
        let paw = match fight.agent.form {
            forms::CAT => set.cat.as_ref(),
            forms::BEAR => set.bear.as_ref(),
            _ => None,
        };
        if let Some((_, paw)) = paw {
            fight.set_main_hand(paw.clone());
        }
    }

    /// Go rake.go and rip.go `ExpectedTickDamage`.
    fn expected_tick_damage(
        fight: &mut Fight<Self>,
        spell: SpellId,
        target: Side,
        snapshot: bool,
    ) -> Option<f64> {
        match fight.spells[spell].behavior {
            crate::core::fight::SpellBehavior::Class(DruidSpell::Rake) => {
                let rake = fight.agent.rake.clone()?;
                Some(rake.expected_tick_damage(fight, spell, target, snapshot))
            }
            crate::core::fight::SpellBehavior::Class(DruidSpell::Rip) => {
                let rip = fight.agent.rip.clone()?;
                Some(rip.expected_tick_damage(fight, spell, target, snapshot))
            }
            _ => None,
        }
    }

    fn apply_effects(fight: &mut Fight<Self>, spell: SpellId, target: Side, behavior: DruidSpell) {
        match behavior {
            DruidSpell::Heal => {
                let book = fight.agent.heals.clone().expect("the heals are bound");
                let rank = book
                    .rank_of(spell)
                    .expect("a heal is one of the bound ranks");
                // A heal over time cast again leaves the one running to end on its player.
                if let Some(bloom) = fight.agent.bloom.clone() {
                    bloom.on_recast(fight, &book, rank);
                }
                book.cast(fight, rank);
            }
            DruidSpell::Swiftmend => {
                let book = fight.agent.heals.clone().expect("the heals are bound");
                let ranks = fight.agent.swiftmend_hots.clone();
                swiftmend::cast(fight, &book, &ranks, spell);
            }
            DruidSpell::MoonkinForm => {
                let aura = fight.agent.moonkin_form.expect("Moonkin Form is bound");
                fight.activate_aura(aura);
            }
            DruidSpell::Starfire => starfire::apply(fight, spell, target),
            DruidSpell::Wrath => wrath::apply(fight, spell, target),
            DruidSpell::Moonfire => {
                let dot_spell = fight.agent.moonfire_dot.expect("Moonfire is bound");
                moonfire::apply(fight, spell, target, dot_spell);
            }
            DruidSpell::MoonfireDot => moonfire::apply_dot(fight, spell, target),
            DruidSpell::InsectSwarm => insect_swarm::apply(fight, spell, target),
            DruidSpell::Innervate => {
                let aura = Self::innervate(fight).aura;
                fight.activate_aura(aura);
            }
            DruidSpell::CatForm => Self::cat_form(fight).apply(fight),
            DruidSpell::BearForm => {
                let bear = fight.agent.bear_form.clone().expect("Bear Form is bound");
                bear.apply(fight);
            }
            DruidSpell::Prowl => Self::prowl(fight).apply(fight),
            DruidSpell::Builder(_) => Self::builders(fight).apply(fight, spell, target),
            DruidSpell::Rip => Self::rip(fight).apply(fight, spell, target),
            DruidSpell::Rake => {
                let rake = fight.agent.rake.clone().expect("Rake is bound");
                rake.apply(fight, spell, target);
            }
            DruidSpell::FerociousBite => {
                let bite = fight.agent.ferocious_bite.expect("Ferocious Bite is bound");
                bite.apply(fight, spell, target);
            }
            DruidSpell::ShiftingPower => {
                let power = fight.agent.shifting_power.expect("Shifting Power is bound");
                power.apply(fight);
            }
            DruidSpell::FaerieFire => {
                let faerie_fire = fight.agent.faerie_fire.expect("Faerie Fire is bound");
                faerie_fire.apply(fight, spell, target);
            }
            DruidSpell::Berserk => {
                let aura = fight.agent.berserk.expect("Berserk is bound").aura;
                fight.activate_aura(aura);
            }
            DruidSpell::Enrage => fight.agent.enrage.expect("Enrage is bound").apply(fight),
            DruidSpell::DemoralizingRoar => {
                let aura = fight
                    .agent
                    .demoralizing_roar
                    .expect("Demoralizing Roar is bound");
                bear::demoralizing_roar(fight, spell, aura);
            }
            DruidSpell::Maul => {
                let maul = fight.agent.maul.expect("Maul is bound");
                maul.strike(fight, spell, target);
            }
            DruidSpell::MaulQueue => fight.agent.maul.expect("Maul is bound").queue(fight),
            DruidSpell::Lacerate => {
                let lacerate = fight.agent.lacerate.expect("Lacerate is bound");
                lacerate.apply(fight, spell, target);
            }
            DruidSpell::PrimalBite => {
                let flat = fight.agent.primal_bite.expect("Primal Bite is bound");
                bear::primal_bite(fight, spell, target, flat);
            }
            DruidSpell::Swipe => {
                let (flat, coefficient) = fight.agent.swipe.expect("Swipe is bound");
                bear::swipe(fight, spell, flat, coefficient);
            }
            DruidSpell::Hurricane => hurricane::apply_channel(fight, spell),
            DruidSpell::HurricaneTick => {
                let state = fight.agent.hurricane.expect("Hurricane is bound");
                state.apply_tick(fight, spell);
            }
            DruidSpell::Barkskin => fight
                .agent
                .barkskin
                .expect("Barkskin is bound")
                .apply(fight),
            DruidSpell::Thorns => {
                let aura = fight.agent.thorns.expect("Thorns is bound");
                fight.activate_aura(aura);
            }
            DruidSpell::FrenziedRegeneration => fight
                .agent
                .frenzied_regeneration_spell
                .expect("Frenzied Regeneration is bound")
                .apply(fight),
            DruidSpell::Survival => panic!("the runtime never casts a survival cooldown"),
        }
    }

    fn extra_cast_condition(fight: &Fight<Self>, _spell: SpellId, behavior: DruidSpell) -> bool {
        match behavior {
            DruidSpell::Swiftmend => fight
                .agent
                .heals
                .as_ref()
                .is_some_and(|book| book.hot_up(fight, &fight.agent.swiftmend_hots).is_some()),
            DruidSpell::Innervate => Self::innervate(fight).can_cast(fight),
            DruidSpell::Prowl => Self::prowl(fight).can_cast(fight),
            DruidSpell::Builder(builder) => Self::builders(fight).can_cast(fight, builder),
            DruidSpell::Rip => Self::rip(fight).can_cast(fight),
            DruidSpell::ShiftingPower => ShiftingPower::can_cast(fight),
            DruidSpell::FerociousBite => fight
                .agent
                .ferocious_bite
                .is_some_and(|bite| bite.can_cast(fight)),
            DruidSpell::MaulQueue => fight.agent.maul.is_some_and(|maul| maul.can_queue(fight)),
            _ => true,
        }
    }

    /// Go `RegisterSpell`'s form check, which logs before the spell's own condition.
    fn extra_cast_condition_logged(
        fight: &mut Fight<Self>,
        spell: SpellId,
        behavior: DruidSpell,
    ) -> bool {
        let form = fight.agent.form;
        if let Some(forms) = fight.agent.forms.clone() {
            if forms.wrong_form(form, spell) {
                if fight.log.is_some() {
                    let id = crate::core::fight::action_string(&fight.spells[spell].id);
                    fight.sim_log(&format!("Failed cast to spell {id}, wrong form"));
                }
                return false;
            }
        }
        Self::extra_cast_condition(fight, spell, behavior)
    }

    /// Go `RegisterSpell`'s `ModifyCast`: a humanoid spell cast outside its forms clears the
    /// form.
    fn modify_cast(fight: &mut Fight<Self>, spell: SpellId, _behavior: DruidSpell) {
        let form = fight.agent.form;
        let clears = fight
            .agent
            .forms
            .as_ref()
            .is_some_and(|forms| forms.clears_form(form, spell));
        if clears {
            match (fight.agent.cat_form.clone(), fight.agent.bear_form.clone()) {
                (_, Some(bear)) if form & forms::BEAR != 0 => bear.clear_form(fight),
                (Some(cat), _) => cat.clear_form(fight),
                _ => fight.agent.form = forms::HUMANOID,
            }
        }
        // Go `Druid.RegisterSpell`: a hard cast restarts the swing timer as it completes
        // (mythicsim patch 105), as for a caster-form druid swinging a staff. A druid that does
        // not auto attack, or a cast made instant, is untouched.
        let cast_time = fight.spells[spell].cur_cast.cast_time;
        let cast_time = fight.apply_cast_speed_for_spell(cast_time, spell);
        if cast_time > 0 {
            fight.hold_melee_for_cast(fight.now + cast_time);
        }
    }

    fn should_activate(_fight: &Fight<Self>, _spell: SpellId, behavior: DruidSpell) -> bool {
        // Go leaves Innervate to the rotation.
        behavior != DruidSpell::Innervate
    }

    fn cooldown_activation_condition(fight: &Fight<Self>, spell: SpellId) -> bool {
        fight
            .agent
            .cat_form
            .as_ref()
            .is_none_or(|cat| cat.activation_allowed(fight, spell))
            && fight
                .agent
                .bear_form
                .as_ref()
                .is_none_or(|bear| bear.activation_allowed(fight, spell))
    }

    fn after_apply_effects(fight: &mut Fight<Self>, spell: SpellId) {
        if let Some(cat) = fight.agent.cat_form.clone() {
            cat.after_apply_effects(fight, spell);
        }
        if let Some(bear) = fight.agent.bear_form.clone() {
            bear.after_apply_effects(fight, spell);
        }
    }

    /// Go restores the initial pseudo stats, which the exported ones include the form in.
    fn reset(fight: &mut Fight<Self>) {
        if let Some(cat) = fight.agent.cat_form.clone() {
            fight.player.threat_multiplier = cat.initial_threat_multiplier;
            fight.player.spirit_regen_multiplier = cat.initial_spirit_regen_multiplier;
            fight.player.powers = fight.stat_combos[0];
        }
        if let Some(bear) = fight.agent.bear_form.clone() {
            fight.player.threat_multiplier = bear.initial_threat_multiplier;
            fight.player.spirit_regen_multiplier = bear.initial_spirit_regen_multiplier;
            fight.player.powers = fight.stat_combos[0];
        }
        // Maul's queue aura OnReset.
        fight.agent.maul_queued = false;
    }

    /// Go `Druid.Reset`, and the feral cat's: back to the starting form, then out of it and
    /// into Cat Form.
    fn agent_reset(fight: &mut Fight<Self>) {
        if let Some(forms) = fight.agent.forms.clone() {
            fight.agent.form = forms.starting;
        }
        if let Some(cat) = fight.agent.cat_form.clone() {
            cat.clear_form(fight);
            fight.activate_aura(cat.aura);
        } else if let Some(bear) = fight.agent.bear_form.clone() {
            bear.clear_form(fight);
            fight.activate_aura(bear.aura);
        }
    }

    fn replace_mh_swing(fight: &mut Fight<Self>, swing: SpellId) -> SpellId {
        match fight.agent.maul {
            Some(maul) => maul.replace_swing(fight, swing),
            None => swing,
        }
    }

    fn on_periodic(fight: &mut Fight<Self>, tag: u32) {
        match tag {
            bear::ENRAGE_TAG => fight.agent.enrage.expect("Enrage is bound").tick(fight),
            bear::MAUL_QUEUE_TAG => fight.agent.maul.expect("Maul is bound").arm(fight),
            bear::FRENZIED_REGENERATION_TAG => fight
                .agent
                .frenzied_regeneration_spell
                .expect("Frenzied Regeneration is bound")
                .tick(fight),
            _ => {}
        }
    }

    fn on_enemy_hit_taken(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: DruidAura,
        result: &SpellResult,
    ) {
        match kind {
            DruidAura::NaturalReaction => {
                let natural = fight
                    .agent
                    .natural_reaction
                    .expect("Natural Reaction is bound");
                natural.on_hit_taken(fight, result);
            }
            DruidAura::NaturesBounty => {
                let bounty = fight.agent.natures_bounty.clone().expect("bound");
                bounty.on_enemy_hit_taken(fight, result);
            }
            _ => {}
        }
    }

    fn on_dot_gain(fight: &mut Fight<Self>, dot: DotId, behavior: DruidSpell) {
        if behavior == DruidSpell::InsectSwarm {
            let side = fight.dots[dot].side;
            insect_swarm::on_dot_gain(fight, fight.agent.insect_swarm_debuff, side);
        }
    }

    fn on_dot_expire(fight: &mut Fight<Self>, dot: DotId, behavior: DruidSpell) {
        if behavior == DruidSpell::Heal {
            if let Some(bloom) = fight.agent.bloom.clone() {
                bloom.on_dot_expire(fight, dot);
            }
        }
        if behavior == DruidSpell::InsectSwarm {
            let side = fight.dots[dot].side;
            insect_swarm::on_dot_expire(fight, fight.agent.insect_swarm_debuff, side);
        }
    }

    fn on_dot_tick(fight: &mut Fight<Self>, dot: DotId, behavior: DruidSpell) {
        match behavior {
            DruidSpell::Heal => {
                let book = fight.agent.heals.clone().expect("the heals are bound");
                book.tick(fight, dot);
            }
            DruidSpell::MoonfireDot | DruidSpell::InsectSwarm => fight.snapshot_dot_tick(dot),
            DruidSpell::Hurricane => {
                let state = fight.agent.hurricane.expect("Hurricane is bound");
                let side = fight.dots[dot].side;
                state.on_channel_tick(fight, side);
            }
            DruidSpell::Rip => Self::rip(fight).tick(fight, dot),
            DruidSpell::Rake => {
                let rake = fight.agent.rake.clone().expect("Rake is bound");
                rake.tick(fight, dot);
            }
            DruidSpell::Lacerate => fight
                .agent
                .lacerate
                .expect("Lacerate is bound")
                .tick(fight, dot),
            _ => {}
        }
    }

    fn on_exclusive_gain(fight: &mut Fight<Self>, _aura: AuraRef, kind: DruidAura) {
        if kind == DruidAura::FaerieFire {
            let faerie_fire = fight.agent.faerie_fire.expect("Faerie Fire is bound");
            faerie_fire.on_exclusive_gain(fight);
        }
    }

    fn on_gain(fight: &mut Fight<Self>, _aura: AuraRef, kind: DruidAura) {
        match kind {
            DruidAura::Clearcasting => Self::omen(fight).on_gain(fight),
            DruidAura::NaturesGrace => Self::natures_grace(fight).on_gain(fight),
            DruidAura::Eclipse => Self::eclipse(fight).on_gain(fight),
            DruidAura::Innervate => Self::innervate(fight).on_gain(fight),
            DruidAura::CatForm => Self::cat_form(fight).on_gain(fight),
            DruidAura::Prowl => Self::prowl(fight).on_gain(fight),
            DruidAura::Berserk => fight.agent.berserk.expect("bound").on_gain(fight),
            DruidAura::BearForm => fight.agent.bear_form.clone().expect("bound").on_gain(fight),
            DruidAura::Enrage => fight.agent.enrage.expect("bound").on_gain(fight),
            DruidAura::Barkskin => fight.agent.barkskin.expect("bound").on_gain(fight),
            _ => {}
        }
    }

    fn on_expire(fight: &mut Fight<Self>, _aura: AuraRef, kind: DruidAura) {
        match kind {
            DruidAura::Clearcasting => Self::omen(fight).on_expire(fight),
            DruidAura::NaturesGrace => Self::natures_grace(fight).on_expire(fight),
            DruidAura::Eclipse => Self::eclipse(fight).on_expire(fight),
            DruidAura::Innervate => Self::innervate(fight).on_expire(fight),
            DruidAura::CatForm => Self::cat_form(fight).on_expire(fight),
            DruidAura::Prowl => Self::prowl(fight).on_expire(fight),
            DruidAura::Berserk => fight.agent.berserk.expect("bound").on_expire(fight),
            DruidAura::FaerieFire => {
                let faerie_fire = fight.agent.faerie_fire.expect("Faerie Fire is bound");
                faerie_fire.on_expire(fight);
            }
            DruidAura::BearForm => fight
                .agent
                .bear_form
                .clone()
                .expect("bound")
                .on_expire(fight),
            DruidAura::Enrage => fight.agent.enrage.expect("bound").on_expire(fight),
            DruidAura::Barkskin => fight.agent.barkskin.expect("bound").on_expire(fight),
            _ => {}
        }
    }

    /// Go Omen of Clarity's trigger also hears the druid's heals, as an item's heal is.
    fn on_heal_dealt(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: DruidAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        if kind == DruidAura::OmenOfClarity {
            Self::omen_hit(fight, spell, result);
        }
    }

    fn on_spell_hit_dealt(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: DruidAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        match kind {
            DruidAura::OmenOfClarity => Self::omen_hit(fight, spell, result),
            DruidAura::NaturesGraceTrigger => {
                Self::natures_grace(fight).on_spell_hit_dealt(fight, spell, result)
            }
            DruidAura::Prowl => Self::prowl(fight).on_spell_hit_dealt(fight),
            DruidAura::BloodFrenzy => {
                Self::blood_frenzy(fight).on_spell_hit_dealt(fight, spell, result)
            }
            DruidAura::BloodFrenzyBear => {
                Self::blood_frenzy(fight).on_bear_hit_dealt(fight, spell, result)
            }
            DruidAura::NaturesBounty => {
                let bounty = fight.agent.natures_bounty.clone().expect("bound");
                bounty.on_spell_hit_dealt(fight, spell, result);
            }
            DruidAura::UnendingLifeRefund => {
                let refund = fight.agent.unending_life.clone().expect("bound");
                refund.on_spell_hit_dealt(fight, spell, result);
            }
            _ => {}
        }
    }

    fn on_delayed_proc(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: DruidAura,
        _spell: SpellId,
        _result: SpellResult,
    ) {
        match kind {
            DruidAura::OmenOfClarity => Self::omen(fight).on_delayed_proc(fight),
            DruidAura::BloodFrenzy => Self::blood_frenzy(fight).on_delayed_proc(fight),
            DruidAura::BloodFrenzyBear => Self::blood_frenzy(fight).on_bear_delayed_proc(fight),
            DruidAura::NaturesBounty => {
                let bounty = fight.agent.natures_bounty.clone().expect("bound");
                bounty.on_delayed_proc(fight, _spell);
            }
            DruidAura::UnendingLifeRefund => {
                let refund = fight.agent.unending_life.clone().expect("bound");
                refund.on_delayed_proc(fight);
            }
            DruidAura::NaturalReaction => fight
                .agent
                .natural_reaction
                .expect("Natural Reaction is bound")
                .on_delayed_proc(fight),
            _ => {}
        }
    }

    fn on_cast_complete(fight: &mut Fight<Self>, _aura: AuraRef, kind: DruidAura, spell: SpellId) {
        match kind {
            DruidAura::Clearcasting => Self::omen(fight).on_cast_complete(fight, spell),
            DruidAura::EclipseTrigger => Self::eclipse(fight).on_cast_complete(fight, spell),
            DruidAura::NaturesBounty => {
                let bounty = fight.agent.natures_bounty.clone().expect("bound");
                bounty.on_cast_complete(fight, spell);
            }
            _ => {}
        }
    }
}
