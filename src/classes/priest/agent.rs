//! The Priest class agent for the fight runtime: class spells and auras by name, and the
//! hooks that dispatch to each spell's and talent's module.

use std::rc::Rc;

use crate::{
    contracts::prepared_v2::{Effect, PreparedV2, Spell as ExportedSpell},
    core::fight::{
        heals::HealBook, Agent, AuraRef, DotId, Fight, Outcome, Side, SpellId, SpellResult,
        OUTCOME_LANDED,
    },
};

use super::{
    spells::{
        dark_sacrifice, devouring_plague, direct, holy, periodic, prayer_of_mending, shadowfiend,
        shadowform,
    },
    talents::{
        inner_focus, litany_of_light, power_in_light, power_infusion, renewed_hope, searing_light,
        shadow_weaving,
    },
};

/// What a Priest spell does when its effects apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PriestSpell {
    MindBlast,
    /// Chastise (Forever): a rolled Holy hit, cast only at a Humanoid.
    Chastise,
    ShadowWordDeath,
    ShadowWordPain,
    DevouringPlague,
    MindFlay,
    Starshards,
    HolyNova,
    HolyNovaHeal,
    PowerInfusion,
    Shadowfiend,
    Shadowform,
    InnerFocus,
    DarkSacrifice,
    Smite,
    HolyFire,
    Penance,
    /// A heal of MythicSim's healer sims (`core::fight::heals`).
    Heal,
    VampiricEmbrace,
}

/// Class auras with Rust behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PriestAura {
    Shadowform,
    DarkInspirationMindBlast,
    DarkInspirationMindFlay,
    InnerFocus,
    ShadowWeavingTrigger,
    SearingLight,
    SearingLightTrigger,
    ShadowfiendManaRestore,
    /// Vampiric Embrace's debuff on the target.
    VampiricEmbrace,
}

/// Priest state that Go keeps in the `Priest` struct and its closures.
#[derive(Default)]
pub(crate) struct PriestAgent {
    /// Whether the target is a Humanoid, the only creature Chastise can be cast at.
    humanoid_target: bool,
    early_demise_crit: f64,
    /// Improved Mind Flay's step a tick (client 70425).
    mind_flay_ramp: f64,
    /// Whether the step compounds tick over tick (Forever's tooltip) or adds (the Go reference).
    mind_flay_ramp_compounds: bool,
    /// Dark Inspiration's Spirit aura (client 70425).
    dark_inspiration: Option<AuraRef>,
    /// Devouring Plague ranks by spell, with their health metrics.
    devouring_plague: Vec<(SpellId, usize)>,
    shadowform: Option<Rc<shadowform::Shadowform>>,
    inner_focus: Option<Rc<inner_focus::InnerFocus>>,
    shadow_weaving: Option<Rc<shadow_weaving::ShadowWeaving>>,
    dark_sacrifice: Option<Rc<dark_sacrifice::DarkSacrifice>>,
    searing_light: Option<Rc<searing_light::SearingLight>>,
    /// Holy Nova's heal spell and base by damage spell, and the healing modifiers.
    holy_nova: Option<Rc<HolyNova>>,
    /// The healer's heals.
    heals: Option<Rc<HealBook>>,
    /// Prayer of Mending on the party, under Forever's rules.
    pub(crate) mending: Option<prayer_of_mending::MendingRun>,
    /// Renewed Hope, and who on the party has Weakened Soul.
    renewed_hope: Option<Rc<renewed_hope::RenewedHope>>,
    pub(crate) souls: renewed_hope::Souls,
    /// Litany of Light, and the spell the last heal cast was, as one of its families.
    litany_of_light: Option<Rc<litany_of_light::LitanyOfLight>>,
    pub(crate) last_heal: Option<usize>,
    power_infusion: Option<Rc<power_infusion::PowerInfusion>>,
    shadowfiend: Option<Rc<shadowfiend::Shadowfiend>>,
    /// Vampiric Embrace's debuff, its heal share and health metrics.
    vampiric_embrace: Option<(AuraRef, f64, usize)>,
}

/// Holy Nova's ranks resolved to spells.
#[derive(Clone, Debug)]
pub(crate) struct HolyNova {
    heals: Vec<(SpellId, SpellId, f64)>,
    healing: crate::core::fight::healing::Healing,
}

/// Aura labels claimed by implemented class effects, as (unit, label, kind).
fn class_auras(prepared: &PreparedV2) -> Vec<(&'static str, String, PriestAura)> {
    let mut auras: Vec<(&'static str, String, PriestAura)> = player_auras(prepared)
        .into_iter()
        .map(|(label, kind)| ("player", label, kind))
        .collect();
    for effect in &prepared.effects {
        if let Effect::VampiricEmbrace { aura, .. } = effect {
            auras.push(("target", aura.clone(), PriestAura::VampiricEmbrace));
        }
        if let Effect::Shadowfiend {
            mana_restore_aura, ..
        } = effect
        {
            auras.push((
                "pet",
                mana_restore_aura.clone(),
                PriestAura::ShadowfiendManaRestore,
            ));
        }
    }
    auras
}

/// Player aura labels claimed by implemented class effects.
fn player_auras(prepared: &PreparedV2) -> Vec<(String, PriestAura)> {
    let mut auras = Vec::new();
    for effect in &prepared.effects {
        match effect {
            Effect::Shadowform { aura, .. } => auras.push((aura.clone(), PriestAura::Shadowform)),
            Effect::InnerFocus { aura, .. } => auras.push((aura.clone(), PriestAura::InnerFocus)),
            Effect::DarkInspiration {
                mind_blast_trigger,
                mind_flay_trigger,
                ..
            } => {
                auras.push((
                    mind_blast_trigger.clone(),
                    PriestAura::DarkInspirationMindBlast,
                ));
                auras.push((
                    mind_flay_trigger.clone(),
                    PriestAura::DarkInspirationMindFlay,
                ));
            }
            Effect::ShadowWeaving { trigger_aura, .. } => {
                auras.push((trigger_aura.clone(), PriestAura::ShadowWeavingTrigger));
            }
            Effect::SearingLight {
                aura, trigger_aura, ..
            } => {
                auras.push((aura.clone(), PriestAura::SearingLight));
                auras.push((trigger_aura.clone(), PriestAura::SearingLightTrigger));
            }
            _ => {}
        }
    }
    auras
}

/// The spell IDs of class spells Go registers without a class mask.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct UnmaskedSpells {
    pub(crate) inner_focus: Option<i32>,
    pub(crate) dark_sacrifice: Option<i32>,
    /// Holy Nova's triggered heals, which carry no class mask.
    pub(crate) holy_nova_heals: [i32; 8],
}

impl UnmaskedSpells {
    pub(crate) fn of(effects: &[Effect]) -> Self {
        let mut ids = UnmaskedSpells::default();
        for effect in effects {
            match effect {
                Effect::InnerFocus { spell_id, .. } => ids.inner_focus = Some(*spell_id),
                Effect::DarkSacrifice { spell_id, .. } => ids.dark_sacrifice = Some(*spell_id),
                Effect::HolyNova { ranks, .. } => {
                    for (slot, rank) in ids.holy_nova_heals.iter_mut().zip(ranks) {
                        *slot = rank.heal_spell_id;
                    }
                }
                _ => {}
            }
        }
        ids
    }
}

impl PriestAgent {
    /// The class behavior of an exported spell, if Rust implements it.
    pub(crate) fn spell(
        spell: &ExportedSpell,
        unmasked: UnmaskedSpells,
        vampiric_embrace: bool,
    ) -> Option<PriestSpell> {
        let id = spell.action_id.clone().unwrap_or_default();
        let Some(class) = spell.class_spell.as_deref() else {
            let own = |known: Option<i32>| known == Some(id.spell_id) && id.tag == 0;
            let heal =
                id.spell_id != 0 && id.tag == 0 && unmasked.holy_nova_heals.contains(&id.spell_id);
            return if heal {
                Some(PriestSpell::HolyNovaHeal)
            } else if own(unmasked.inner_focus) {
                Some(PriestSpell::InnerFocus)
            } else if own(unmasked.dark_sacrifice) && spell.dot.is_some() {
                Some(PriestSpell::DarkSacrifice)
            } else {
                None
            };
        };
        match class {
            "mind_blast" if spell.damage_effect.is_some() => Some(PriestSpell::MindBlast),
            "shadow_word_death" if spell.damage_effect.is_some() => {
                Some(PriestSpell::ShadowWordDeath)
            }
            "shadow_word_pain" if spell.dot.is_some() => Some(PriestSpell::ShadowWordPain),
            "devouring_plague" if spell.dot.is_some() => Some(PriestSpell::DevouringPlague),
            "mind_flay" if spell.dot.as_ref().is_some_and(|dot| dot.channeled) => {
                Some(PriestSpell::MindFlay)
            }
            "starshards" if spell.dot.as_ref().is_some_and(|dot| dot.channeled) => {
                Some(PriestSpell::Starshards)
            }
            "shadowform" => Some(PriestSpell::Shadowform),
            "shadowfiend" => Some(PriestSpell::Shadowfiend),
            "power_infusion" => Some(PriestSpell::PowerInfusion),
            "smite" if spell.damage_effect.is_some() => Some(PriestSpell::Smite),
            "chastise" if spell.damage_effect.is_some() => Some(PriestSpell::Chastise),
            "holy_nova" if spell.damage_effect.is_some() => Some(PriestSpell::HolyNova),
            "holy_fire" if spell.damage_effect.is_some() && spell.dot.is_some() => {
                Some(PriestSpell::HolyFire)
            }
            "penance" if spell.dot.as_ref().is_some_and(|dot| dot.channeled) => {
                Some(PriestSpell::Penance)
            }
            "heal" => Some(PriestSpell::Heal),
            "vampiric_embrace" if vampiric_embrace => Some(PriestSpell::VampiricEmbrace),
            _ => None,
        }
    }

    /// Build a fight for a prepared input that passed the coverage gate.
    pub(crate) fn fight(prepared: &PreparedV2) -> Result<Fight<PriestAgent>, String> {
        let auras = class_auras(prepared);
        let unmasked = UnmaskedSpells::of(&prepared.effects);
        let vampiric_embrace = prepared
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::VampiricEmbrace { .. }));
        let mut fight = Fight::new(
            prepared,
            PriestAgent {
                humanoid_target: prepared.target.mob_type.as_deref() == Some("MobTypeHumanoid"),
                ..PriestAgent::default()
            },
            |spell| PriestAgent::spell(spell, unmasked, vampiric_embrace),
            |unit, label| {
                auras
                    .iter()
                    .find(|(owner, name, _)| *owner == unit && name == label)
                    .map(|(_, _, kind)| *kind)
            },
        )?;
        let find_spell = |fight: &Fight<PriestAgent>, id: i32| {
            fight.spells.iter().position(|spell| {
                spell.id.spell_id == id && spell.id.tag == 0 && spell.id.item_id == 0
            })
        };
        let set_ticks =
            |fight: &mut Fight<PriestAgent>,
             ranks: &[crate::contracts::prepared_v2::FireballRank]| {
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
            };
        for effect in &prepared.effects {
            match effect {
                Effect::VampiricEmbrace {
                    aura,
                    heal_fraction,
                    metrics_action_id,
                    ..
                } => {
                    let aura = fight.trackers[Side::Target.index()]
                        .find(aura)
                        .map(|index| AuraRef {
                            side: Side::Target,
                            index,
                        })
                        .ok_or("Vampiric Embrace's debuff is not registered")?;
                    let metrics = fight.new_health_metrics(metrics_action_id.clone());
                    fight.agent.vampiric_embrace = Some((aura, *heal_fraction, metrics));
                }
                Effect::HolyNova {
                    ranks,
                    healing_dealt_multiplier,
                    healing_taken_multiplier,
                    table_healing_dealt_multiplier,
                    healing_power,
                } => {
                    let mut heals = Vec::new();
                    for rank in ranks {
                        let spell = find_spell(&fight, rank.spell_id)
                            .ok_or("a Holy Nova rank is not registered")?;
                        let heal = find_spell(&fight, rank.heal_spell_id)
                            .ok_or("a Holy Nova heal is not registered")?;
                        heals.push((spell, heal, rank.heal_base));
                    }
                    fight.agent.holy_nova = Some(Rc::new(HolyNova {
                        heals,
                        healing: crate::core::fight::healing::Healing {
                            dealt_multiplier: *healing_dealt_multiplier,
                            taken_multiplier: *healing_taken_multiplier,
                            table_multiplier: *table_healing_dealt_multiplier,
                            healing_power: *healing_power,
                        },
                    }));
                }
                Effect::Shadowfiend {
                    aura,
                    duration_ns,
                    pet,
                    attack_power_coefficient,
                    attack_power_without_deps,
                    attack_power_dependency_terms,
                    stats,
                    fight_end_stats,
                    mana_restore_aura,
                    mana_restore_fraction,
                    mana_restore_action_id,
                    ..
                } => {
                    let bound = shadowfiend::bind(
                        &mut fight,
                        aura,
                        *duration_ns,
                        pet,
                        mana_restore_aura,
                        *mana_restore_fraction,
                        *mana_restore_action_id,
                        *attack_power_coefficient,
                        *attack_power_without_deps,
                        attack_power_dependency_terms,
                        stats,
                        fight_end_stats,
                    )?;
                    fight.agent.shadowfiend = Some(Rc::new(bound));
                }
                // The priest's own copy of the aura; the external caster's belongs to the
                // external cooldown.
                Effect::PowerInfusion { aura, .. }
                    if !prepared.effects.iter().any(|other| {
                        matches!(other, Effect::ExternalCooldown { aura: external, .. } if external == aura)
                    }) =>
                {
                    let bound = power_infusion::bind(&mut fight, aura)?;
                    fight.agent.power_infusion = Some(Rc::new(bound));
                }
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
                    fight.agent.heals = Some(Rc::new(book));
                }
                Effect::PriestHealTalents {
                    renewed_hope,
                    litany_of_light,
                } => {
                    if let Some(spec) = renewed_hope {
                        let (bound, souls) = renewed_hope::bind(&mut fight, spec)?;
                        fight.agent.renewed_hope = Some(Rc::new(bound));
                        fight.agent.souls = souls;
                    }
                    if let Some(spec) = litany_of_light {
                        let bound = litany_of_light::bind(&mut fight, spec)?;
                        fight.agent.litany_of_light = Some(Rc::new(bound));
                    }
                }
                Effect::ShadowWordDeath { early_demise_crit } => {
                    fight.agent.early_demise_crit = *early_demise_crit;
                }
                Effect::ShadowWordPain { ranks }
                | Effect::MindFlay { ranks }
                | Effect::Starshards { ranks }
                | Effect::HolyFire { ranks }
                | Effect::Penance { ranks } => {
                    set_ticks(&mut fight, ranks);
                }
                Effect::PowerInLight {
                    multiplier,
                    spells,
                    dot_spells,
                } => power_in_light::bind(&mut fight, *multiplier, spells, dot_spells)?,
                Effect::SearingLight {
                    trigger_aura,
                    aura,
                    trigger_immediately,
                    proc_chance,
                    trigger_spells,
                    cost_percent_add,
                    cost_spells,
                    cancel_spells,
                    ..
                } => {
                    let bound = searing_light::bind(
                        &mut fight,
                        aura,
                        trigger_aura,
                        *proc_chance,
                        *trigger_immediately,
                        trigger_spells,
                        *cost_percent_add,
                        cost_spells,
                        cancel_spells,
                    )?;
                    fight.agent.searing_light = Some(Rc::new(bound));
                }
                Effect::DevouringPlague {
                    ranks,
                    heal_metrics_tag,
                } => {
                    set_ticks(&mut fight, ranks);
                    // Go registers each rank's health metrics with the spell.
                    for rank in ranks {
                        if let Some(spell) = find_spell(&fight, rank.spell_id) {
                            let mut id = fight.spells[spell].id.clone();
                            id.tag = *heal_metrics_tag;
                            let metrics = fight.new_health_metrics(id);
                            fight.agent.devouring_plague.push((spell, metrics));
                        }
                    }
                }
                Effect::Shadowform {
                    aura,
                    damage_percent,
                    cost_percent,
                    crit_multiplier,
                    school_spells,
                    crit_spells,
                    ..
                } => {
                    let bound = shadowform::bind(
                        &mut fight,
                        aura,
                        *damage_percent,
                        *cost_percent,
                        *crit_multiplier,
                        school_spells,
                        crit_spells,
                    )?;
                    fight.agent.shadowform = Some(Rc::new(bound));
                }
                Effect::ImprovedMindFlay {
                    ramp_per_tick,
                    compounding,
                } => {
                    fight.agent.mind_flay_ramp = *ramp_per_tick;
                    fight.agent.mind_flay_ramp_compounds = *compounding;
                }
                Effect::DarkInspiration { aura, .. } => {
                    fight.agent.dark_inspiration = Some(fight.player_aura(aura)?);
                }
                Effect::InnerFocus {
                    spell_id,
                    aura,
                    cost_percent,
                    crit_percent,
                    crit_spells,
                    spender_spells,
                } => {
                    let spell = find_spell(&fight, *spell_id)
                        .ok_or_else(|| format!("Inner Focus {spell_id} is not registered"))?;
                    let bound = inner_focus::bind(
                        &mut fight,
                        spell,
                        aura,
                        *cost_percent,
                        *crit_percent,
                        crit_spells,
                        spender_spells,
                    )?;
                    fight.agent.inner_focus = Some(Rc::new(bound));
                }
                Effect::ShadowWeaving {
                    trigger_aura,
                    aura,
                    trigger_immediately,
                    proc_chance,
                    trigger_spells,
                    damage_per_stack,
                    ..
                } => {
                    let bound = shadow_weaving::bind(
                        &mut fight,
                        aura,
                        trigger_aura,
                        *proc_chance,
                        *trigger_immediately,
                        trigger_spells,
                        *damage_per_stack,
                    )?;
                    fight.agent.shadow_weaving = Some(Rc::new(bound));
                }
                Effect::DarkSacrifice {
                    spell_id,
                    tick_base,
                    spirit_divisor,
                    metrics_action_id,
                    no_threat,
                    ..
                } => {
                    let spell = find_spell(&fight, *spell_id)
                        .ok_or_else(|| format!("Dark Sacrifice {spell_id} is not registered"))?;
                    let spirit = prepared
                        .player
                        .stats
                        .get("Spirit")
                        .copied()
                        .ok_or("prepared stats lack Spirit")?;
                    let bound = dark_sacrifice::bind(
                        &mut fight,
                        spell,
                        *tick_base,
                        spirit,
                        *spirit_divisor,
                        metrics_action_id,
                        *no_threat,
                    )?;
                    fight.agent.dark_sacrifice = Some(Rc::new(bound));
                }
                _ => {}
            }
        }
        Ok(fight)
    }

    fn shadowform(fight: &Fight<Self>) -> Rc<shadowform::Shadowform> {
        fight.agent.shadowform.clone().expect("Shadowform is bound")
    }

    fn inner_focus(fight: &Fight<Self>) -> Rc<inner_focus::InnerFocus> {
        fight
            .agent
            .inner_focus
            .clone()
            .expect("Inner Focus is bound")
    }

    fn shadow_weaving(fight: &Fight<Self>) -> Rc<shadow_weaving::ShadowWeaving> {
        fight
            .agent
            .shadow_weaving
            .clone()
            .expect("Shadow Weaving is bound")
    }

    fn searing_light(fight: &Fight<Self>) -> Rc<searing_light::SearingLight> {
        fight
            .agent
            .searing_light
            .clone()
            .expect("Searing Light is bound")
    }

    fn dark_sacrifice(fight: &Fight<Self>) -> Rc<dark_sacrifice::DarkSacrifice> {
        fight
            .agent
            .dark_sacrifice
            .clone()
            .expect("Dark Sacrifice is bound")
    }
}

impl PriestAgent {
    /// Vampiric Embrace's proc trigger on the target: a hit or tick of the priest's own Shadow
    /// spell that deals damage, handled a spell batch window later.
    fn vampiric_embrace_trigger(
        fight: &mut Fight<Self>,
        aura: AuraRef,
        spell: SpellId,
        result: &SpellResult,
    ) {
        let state = &fight.spells[spell];
        if state.caster != Side::Player || state.school & SHADOW == 0 || result.damage == 0.0 {
            return;
        }
        fight.schedule_delayed_proc(aura, spell, *result);
    }
}

/// Go `SpellSchoolShadow`.
const SHADOW: u8 = 1 << 5;

impl Agent for PriestAgent {
    type Spell = PriestSpell;
    type Aura = PriestAura;

    /// chastise.go `ExtraCastCondition`: the target is a Humanoid.
    fn extra_cast_condition(fight: &Fight<Self>, _spell: SpellId, behavior: PriestSpell) -> bool {
        behavior != PriestSpell::Chastise || fight.agent.humanoid_target
    }

    fn reset(fight: &mut Fight<Self>) {
        fight.agent.mending = None;
        fight.agent.souls.clear();
        fight.agent.last_heal = None;
    }

    fn on_class_action(fight: &mut Fight<Self>, tag: u32) {
        if tag == prayer_of_mending::MENDING_TAG {
            let book = fight.agent.heals.clone().expect("the heals are bound");
            prayer_of_mending::hop(fight, &book);
        }
    }

    fn apply_effects(fight: &mut Fight<Self>, spell: SpellId, target: Side, behavior: PriestSpell) {
        match behavior {
            // talents_shadow.go: the outcome always lands, the debuff goes up, then the outcome is
            // dealt.
            PriestSpell::VampiricEmbrace => {
                let (aura, _, _) = fight
                    .agent
                    .vampiric_embrace
                    .expect("Vampiric Embrace is bound");
                let result = fight.calc_outcome(spell, target, Outcome::AlwaysHit);
                if result.outcome & OUTCOME_LANDED != 0 {
                    // Go `NewEnemyAuraArray`: the debuff on the cast's own target.
                    let aura = fight.aura_on(aura, target);
                    fight.activate_aura(aura);
                }
                fight.deal_damage(spell, result, false);
            }
            PriestSpell::Heal => {
                let book = fight.agent.heals.clone().expect("the heals are bound");
                let rank = book
                    .rank_of(spell)
                    .expect("a heal is one of the bound ranks");
                if let Some(litany) = fight.agent.litany_of_light.clone() {
                    litany.on_cast(fight, spell);
                }
                let crit = match fight.agent.renewed_hope.clone() {
                    Some(hope) => hope.on_cast(fight, spell),
                    None => 0.0,
                };
                match book.mending(rank) {
                    Some((charges, duration)) => {
                        prayer_of_mending::cast(fight, &book, rank, charges, duration);
                    }
                    None => book.cast_with_crit(fight, rank, 1.0, crit),
                }
            }
            // chastise.go: `CalcAndDealDamage` of the row's roll on `OutcomeMagicHitAndCrit`,
            // as Smite.
            PriestSpell::MindBlast | PriestSpell::Smite | PriestSpell::Chastise => {
                direct::apply(fight, spell, target)
            }
            // talents_holy.go: Go `CalcAndDealAoeDamageWithVariance`, which rolls, resolves and
            // deals the hit on each target in unit index order before the heal is cast on the
            // priest.
            PriestSpell::HolyNova => {
                let sides: Vec<Side> = fight.target_sides().collect();
                for side in sides {
                    direct::apply(fight, spell, side);
                }
                let nova = fight.agent.holy_nova.clone().expect("Holy Nova is bound");
                let &(_, heal, _) = nova
                    .heals
                    .iter()
                    .find(|(rank, _, _)| *rank == spell)
                    .expect("the rank has a heal");
                fight.cast(heal, Side::Player);
            }
            PriestSpell::PowerInfusion => {
                let bound = fight
                    .agent
                    .power_infusion
                    .clone()
                    .expect("Power Infusion is bound");
                bound.apply(fight);
            }
            PriestSpell::Shadowfiend => {
                let bound = fight
                    .agent
                    .shadowfiend
                    .clone()
                    .expect("Shadowfiend is bound");
                shadowfiend::summon(fight, &bound);
            }
            PriestSpell::HolyNovaHeal => {
                let nova = fight.agent.holy_nova.clone().expect("Holy Nova is bound");
                let &(_, _, base) = nova
                    .heals
                    .iter()
                    .find(|(_, heal, _)| *heal == spell)
                    .expect("the heal has a rank");
                // Go reads the live healing power, which a stat aura such as an on-use
                // trinket's changes, plus the bonus healing taken the export folds into the
                // reset value. The healing dealt multiplier Power Infusion moves is the
                // fight's.
                let mut healing = nova.healing;
                let bonus_healing_taken = healing.healing_power - fight.config.powers.healing_power;
                healing.healing_power =
                    fight.unit(Side::Player).powers.healing_power + bonus_healing_taken;
                fight.calc_and_deal_self_healing_crit(spell, base, healing);
            }
            PriestSpell::HolyFire => holy::apply_holy_fire(fight, spell, target),
            PriestSpell::Penance => holy::apply_penance(fight, spell, target),
            PriestSpell::ShadowWordDeath => {
                let crit = fight.agent.early_demise_crit;
                direct::apply_shadow_word_death(fight, spell, target, crit);
            }
            PriestSpell::ShadowWordPain
            | PriestSpell::DevouringPlague
            | PriestSpell::MindFlay
            | PriestSpell::Starshards => periodic::apply(fight, spell, target),
            PriestSpell::Shadowform => {
                let aura = Self::shadowform(fight).aura;
                fight.activate_aura(aura);
            }
            PriestSpell::InnerFocus => {
                let aura = Self::inner_focus(fight).aura;
                fight.activate_aura(aura);
            }
            PriestSpell::DarkSacrifice => Self::dark_sacrifice(fight).apply(fight),
        }
    }

    fn should_activate(fight: &Fight<Self>, _spell: SpellId, behavior: PriestSpell) -> bool {
        match behavior {
            PriestSpell::DarkSacrifice => Self::dark_sacrifice(fight).should_activate(fight),
            _ => true,
        }
    }

    fn on_dot_tick(fight: &mut Fight<Self>, dot: DotId, behavior: PriestSpell) {
        match behavior {
            PriestSpell::Heal => {
                let book = fight.agent.heals.clone().expect("the heals are bound");
                let spell = fight.dots[dot].spell;
                if let Some(hope) = fight.agent.renewed_hope.clone() {
                    fight.heal_crit_bonus = hope.on_tick(fight, spell);
                }
                book.tick(fight, dot);
                fight.heal_crit_bonus = 0.0;
            }
            PriestSpell::MindFlay if fight.agent.mind_flay_ramp != 0.0 => {
                // Improved Mind Flay: each tick deals the step more than the one before.
                let mut result = fight.snapshot_dot_tick_calc(dot);
                let ticks = fight.dots[dot].tick_count() - 1;
                let ramp = fight.agent.mind_flay_ramp;
                let factor = if fight.agent.mind_flay_ramp_compounds {
                    (1.0 + ramp).powi(ticks)
                } else {
                    ramp.mul_add(f64::from(ticks), 1.0)
                };
                let spell = fight.dots[dot].spell;
                fight.scale_damage(spell, &mut result, factor);
                fight.deal_damage(spell, result, true);
            }
            PriestSpell::ShadowWordPain
            | PriestSpell::MindFlay
            | PriestSpell::Starshards
            | PriestSpell::HolyFire
            | PriestSpell::Penance => {
                fight.snapshot_dot_tick(dot);
            }
            PriestSpell::DevouringPlague => {
                let spell = fight.dots[dot].spell;
                let metrics = fight
                    .agent
                    .devouring_plague
                    .iter()
                    .find(|(rank, _)| *rank == spell)
                    .map(|(_, metrics)| *metrics)
                    .expect("every Devouring Plague rank has health metrics");
                devouring_plague::tick(fight, dot, metrics);
            }
            PriestSpell::DarkSacrifice => Self::dark_sacrifice(fight).tick(fight),
            _ => {}
        }
    }

    fn on_gain(fight: &mut Fight<Self>, _aura: AuraRef, kind: PriestAura) {
        match kind {
            PriestAura::Shadowform => Self::shadowform(fight).on_gain(fight),
            PriestAura::InnerFocus => Self::inner_focus(fight).on_gain(fight),
            PriestAura::SearingLight => Self::searing_light(fight).on_gain(fight),
            PriestAura::ShadowWeavingTrigger
            | PriestAura::SearingLightTrigger
            | PriestAura::ShadowfiendManaRestore
            | PriestAura::DarkInspirationMindBlast
            | PriestAura::DarkInspirationMindFlay
            | PriestAura::VampiricEmbrace => {}
        }
    }

    fn on_expire(fight: &mut Fight<Self>, _aura: AuraRef, kind: PriestAura) {
        match kind {
            PriestAura::Shadowform => Self::shadowform(fight).on_expire(fight),
            PriestAura::InnerFocus => Self::inner_focus(fight).on_expire(fight),
            PriestAura::SearingLight => Self::searing_light(fight).on_expire(fight),
            PriestAura::ShadowWeavingTrigger
            | PriestAura::SearingLightTrigger
            | PriestAura::ShadowfiendManaRestore
            | PriestAura::DarkInspirationMindBlast
            | PriestAura::DarkInspirationMindFlay
            | PriestAura::VampiricEmbrace => {}
        }
    }

    fn on_stacks_change(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: PriestAura,
        _old: i32,
        new: i32,
    ) {
        let _ = (fight, kind, new);
    }

    fn on_cast_complete(fight: &mut Fight<Self>, aura: AuraRef, kind: PriestAura, spell: SpellId) {
        match kind {
            // Go's proc trigger runs its handler a spell batch window later.
            PriestAura::DarkInspirationMindBlast
                if fight.spells[spell].class_spell.as_deref() == Some("mind_blast") =>
            {
                let result = SpellResult {
                    armor_multiplier: 0.0,
                    target: Side::Target,
                    attacker: Side::Player,
                    outcome: 0,
                    damage: 0.0,
                    threat: 0.0,
                };
                fight.schedule_delayed_proc(aura, spell, result);
            }
            PriestAura::InnerFocus => Self::inner_focus(fight).on_cast_complete(fight, spell),
            PriestAura::SearingLight => Self::searing_light(fight).on_cast_complete(fight, spell),
            _ => {}
        }
    }

    fn on_spell_hit_dealt(
        fight: &mut Fight<Self>,
        aura: AuraRef,
        kind: PriestAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        match kind {
            PriestAura::ShadowWeavingTrigger => {
                Self::shadow_weaving(fight).on_spell_hit_dealt(fight, spell, result);
            }
            // Go AttachProcTriggerCallback: the handler waits a spell batch window.
            PriestAura::ShadowfiendManaRestore => {
                fight.schedule_delayed_proc(aura, spell, *result);
            }
            _ => {}
        }
    }

    fn on_spell_hit_taken(
        fight: &mut Fight<Self>,
        aura: AuraRef,
        kind: PriestAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        if kind == PriestAura::VampiricEmbrace {
            Self::vampiric_embrace_trigger(fight, aura, spell, result);
        }
    }

    fn on_periodic_damage_taken(
        fight: &mut Fight<Self>,
        aura: AuraRef,
        kind: PriestAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        if kind == PriestAura::VampiricEmbrace {
            Self::vampiric_embrace_trigger(fight, aura, spell, result);
        }
    }

    fn on_periodic_damage_dealt(
        fight: &mut Fight<Self>,
        aura: AuraRef,
        kind: PriestAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        match kind {
            PriestAura::SearingLightTrigger => {
                Self::searing_light(fight).on_periodic_damage_dealt(fight, spell, result);
            }
            // Go's proc trigger runs its handler a spell batch window later.
            PriestAura::DarkInspirationMindFlay
                if fight.spells[spell].class_spell.as_deref() == Some("mind_flay") =>
            {
                fight.schedule_delayed_proc(aura, spell, *result);
            }
            _ => {}
        }
    }

    fn on_delayed_proc(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: PriestAura,
        spell: SpellId,
        result: SpellResult,
    ) {
        match kind {
            PriestAura::DarkInspirationMindBlast => {
                let aura = fight.agent.dark_inspiration.expect("bound");
                fight.activate_aura(aura);
            }
            // A full channel: the last tick saw no ticks left.
            PriestAura::DarkInspirationMindFlay => {
                let full = fight.spells[spell].dot.is_some_and(|dot| {
                    let dot = fight.dot_on(dot, result.target);
                    fight.dots[dot].remaining_ticks == 0
                });
                if full {
                    let aura = fight.agent.dark_inspiration.expect("bound");
                    fight.activate_aura(aura);
                }
            }
            PriestAura::ShadowfiendManaRestore => {
                let bound = fight
                    .agent
                    .shadowfiend
                    .clone()
                    .expect("Shadowfiend is bound");
                shadowfiend::restore(fight, &bound, &result);
            }
            PriestAura::ShadowWeavingTrigger => {
                Self::shadow_weaving(fight).handler(fight, result.target)
            }
            PriestAura::SearingLightTrigger => {
                let aura = Self::searing_light(fight).aura;
                fight.activate_aura(aura);
            }
            // The party's heal, of which the player is the one simulated.
            PriestAura::VampiricEmbrace => {
                let (_, share, metrics) = fight
                    .agent
                    .vampiric_embrace
                    .expect("Vampiric Embrace is bound");
                fight.gain_health(result.damage * share, metrics);
            }
            _ => {}
        }
    }
}
