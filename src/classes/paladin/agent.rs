//! The Paladin class agent for the fight runtime: class spells and auras by name, and the
//! hooks that dispatch to each spell's and talent's module.

use std::rc::Rc;

use crate::{
    contracts::prepared_v2::{
        ActionId, ConsecrationRank, Effect, PreparedV2, Spell as ExportedSpell,
    },
    core::fight::{Agent, AuraRef, DotId, Fight, Side, SpellId, SpellResult},
};

use super::{
    spells::{
        crusader::{self, CrusaderJudgement},
        heals::{Heals, HolyLightHaste, Illumination},
        holy::{self, DivineFavor, Vigil},
        judgement, judgement_refresh,
        oath_of_light::OathOfLight,
        seals::{self, Seals},
        strikes,
    },
    talents::{
        protection::{
            self, HolyShield, IronCreed, Reckoning, Redoubt, RighteousFury, ShieldSpecialization,
            SwiftJudgement, TemplarsBulwark,
        },
        retribution::{self, EyeForAnEye, SanctifiedJudgement, Vengeance, Vindication},
        twist_of_light::{self, Echo},
        unsealed_wrath,
    },
};

/// What a Paladin spell does when its effects apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaladinSpell {
    /// A castable seal, by its position among the bound seals.
    Seal(usize),
    SealOfCommandProc,
    /// A Seal of Righteousness rank's damage spell, by its seal's position.
    SealOfRighteousnessProc(usize),
    JudgementOfCommand,
    JudgementOfRighteousness,
    JudgementOfFury,
    /// A Seal of Fury rank's damage spell, by its seal's position.
    SealOfFuryProc(usize),
    Judgement,
    /// A Holy Strike rank, by its position in the effect.
    HolyStrike(usize),
    /// Hammer of the Righteous, whose weapon DPS multiple and most targets the agent holds.
    HammerOfTheRighteous,
    HammerOfWrath,
    /// A Consecration rank, by its position in the effect.
    Consecration(usize),
    /// A Holy Shock rank's damage, by its position in the effect.
    HolyShock(usize),
    DivineFavor,
    /// Oath of Light's cast (push 113554, Forever's rules).
    OathOfLight,
    RighteousFury,
    SwiftJudgement,
    /// Swift Judgement under Forever's rules since push 113554: a second Judgement.
    SecondJudgement,
    TemplarsBulwark,
    /// A Holy Shield rank and its damage, by the rank's position in the effect.
    HolyShield(usize),
    HolyShieldProc(usize),
    Exorcism,
    HolyWrath,
    /// A Light's Vigil rank, by its position in the effect.
    LightsVigil(usize),
    LightsVigilStrike,
    /// A Light's Vigil rank on an ally (push 113554, the healer sim), by its position.
    VigilAlly(usize),
    /// A Judgement of the Crusader rank, by its position in the effect.
    JudgementOfTheCrusader(usize),
    EyeForAnEye,
    /// A heal rank, by its position in the effect.
    Heal(usize),
    /// A Lay on Hands rank, by its position in the effect.
    LayOnHands(usize),
    /// A paladin aura's cast, by its rank's position in the effect.
    AuraCast(usize),
    /// A Seal of Light rank's heal, by its seal's position.
    SealOfLightProc(usize),
    /// A Judgement of Light rank, by its position in the Seal of Light effect.
    JudgementOfLight(usize),
    /// A Seal of Wisdom rank's mana restore, by its seal's position.
    SealOfWisdomProc(usize),
    /// A Judgement of Wisdom rank, by its position in the Seal of Wisdom effect.
    JudgementOfWisdom(usize),
    JudgementOfJustice,
}

/// Class auras with Rust behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaladinAura {
    JudgementRefresh,
    /// A seal's aura, by the seal's position.
    Seal(usize),
    VengeanceTrigger,
    Vengeance,
    VindicationTrigger,
    SanctifiedJudgement,
    SacredArbiter,
    TwistOfLight,
    /// Unsealed Wrath's trigger (push 113554, Forever's rules).
    UnsealedWrath,
    DivineFavor,
    OathOfLight,
    RighteousFury,
    InstrumentOfLaw,
    SwiftJudgement,
    RedoubtTrigger,
    Redoubt,
    ShieldSpecialization,
    ReckoningBlock,
    ReckoningCrit,
    /// Reckoning's charges, which a main hand auto spends.
    ReckoningCharges,
    IronCreedTrigger,
    IronCreed,
    /// A Holy Shield rank's aura, by the rank's position in the effect.
    HolyShield(usize),
    /// A target aura of the "Judgement of the Crusader" category, by its member position.
    CrusaderJudgement(usize),
    /// Infusion of Light's or Holy Alacrity's trigger, by its position among the effects.
    HolyLightHaste(usize),
    /// Infusion of Light's or Holy Alacrity's aura, by the same position.
    HolyLightHasteAura(usize),
    Illumination,
    EyeForAnEye,
}

/// Paladin state that Go keeps in the `Paladin` struct and its closures.
#[derive(Default)]
pub(crate) struct PaladinAgent {
    judgement_refresh: Option<Rc<judgement_refresh::JudgementRefresh>>,
    pub(crate) seals: Seals,
    judgement_wake_delay: i64,
    holy_strike: Vec<f64>,
    /// Hammer of the Righteous: its weapon DPS multiple and most targets.
    hammer_of_the_righteous: Option<(f64, usize)>,
    consecration: Vec<ConsecrationRank>,
    vengeance: Option<Vengeance>,
    vindication: Option<Vindication>,
    sanctified_judgement: Option<SanctifiedJudgement>,
    sacred_arbiter: Rc<Vec<AuraRef>>,
    pub(crate) echoes: Vec<Echo>,
    /// Unsealed Wrath's Echoes (push 113554, Forever's rules).
    pub(crate) unsealed_echoes: Vec<Echo>,
    /// Shieldsworn (push 113554, Forever's rules): a landed Holy Strike triggers Holy Shield and
    /// Shield Specialization.
    pub(crate) shieldsworn: bool,
    holy_shock: Vec<(f64, f64)>,
    divine_favor: Option<Rc<DivineFavor>>,
    /// Oath of Light (push 113554, Forever's rules): read by the heals.
    pub(crate) oath: Option<OathOfLight>,
    /// Consecrated Ground's mark on the target and its Holy damage multiplier.
    pub(crate) consecrated_ground: Option<(AuraRef, f64)>,
    pub(crate) righteous_fury: Option<RighteousFury>,
    swift_judgement: Option<Rc<SwiftJudgement>>,
    redoubt: Option<Rc<Redoubt>>,
    pub(crate) shield_specialization: Option<Rc<ShieldSpecialization>>,
    reckoning: Option<Rc<Reckoning>>,
    iron_creed: Option<Rc<IronCreed>>,
    pub(crate) holy_shield: Vec<Rc<HolyShield>>,
    /// Holy Shield's cast condition: a shield to block with.
    holy_shield_can_block: bool,
    /// Exorcism and Holy Wrath hit only an Undead or Demon target.
    undead_or_demon: bool,
    /// Light's Vigil's ranks.
    pub(crate) vigils: Vec<Vigil>,
    /// Light's Vigil on an ally, rank by rank (push 113554, the healer sim).
    pub(crate) vigil_allies: Vec<holy::VigilAlly>,
    /// Each Judgement of the Crusader rank's target aura, by its position in the effect.
    crusader_auras: Vec<AuraRef>,
    /// The members of the "Judgement of the Crusader" category, by position.
    pub(crate) crusader_judgements: Vec<CrusaderJudgement>,
    /// Infusion of Light's and Holy Alacrity's auras and whether a crit is needed.
    holy_light_haste: Vec<HolyLightHaste>,
    heals: Option<Rc<Heals>>,
    /// The heals as each set reads them, Main then Swap1, when an item swap changes the healing
    /// power they snapshot.
    heals_by_set: Option<[Rc<Heals>; 2]>,
    /// Each Lay on Hands rank's mana restore and mana metrics.
    lay_on_hands: Vec<(f64, usize)>,
    illumination: Option<Illumination>,
    /// The aura each castable paladin aura rank turns on.
    aura_casts: Vec<AuraRef>,
    /// Each Judgement of Light rank's target aura, by its position in the effect.
    light_judgements: Vec<AuraRef>,
    /// Each Judgement of Wisdom rank's target aura, by its position in the effect.
    wisdom_judgements: Vec<AuraRef>,
    justice_judgement: Option<AuraRef>,
    eye_for_an_eye: Option<EyeForAnEye>,
    forbearance: Option<AuraRef>,
    pub(crate) templars_bulwark: Option<TemplarsBulwark>,
}

impl PaladinAgent {
    /// The class behavior of an exported spell, if Rust implements it.
    fn spell(prepared: &PreparedV2, spell: &ExportedSpell) -> Option<PaladinSpell> {
        let effects = &prepared.effects;
        let id = spell.action_id.clone().unwrap_or_default();
        // Holy Shield's damage carries its rank's tag 2.
        if spell.class_spell.as_deref() == Some("holy_shield_proc") {
            return effects.iter().find_map(|effect| match effect {
                Effect::HolyShield { ranks, .. } if id.tag == 2 => ranks
                    .iter()
                    .position(|rank| rank.spell_id == id.spell_id)
                    .map(PaladinSpell::HolyShieldProc),
                _ => None,
            });
        }
        if id.tag != 0 {
            return None;
        }
        // Eye for an Eye's reflection carries no class mask.
        let eye = effects.iter().any(
            |effect| matches!(effect, Effect::EyeForAnEye { spell_id, .. } if *spell_id == id.spell_id),
        );
        if eye && spell.class_spell.is_none() {
            return Some(PaladinSpell::EyeForAnEye);
        }
        let damage = spell.damage_effect.is_some();
        let has = |kind: &str| effects.iter().any(|effect| effect.kind() == kind);
        match spell.class_spell.as_deref()? {
            "seal_of_command" | "seal_of_righteousness" => {
                seals::seal_index(effects, id.spell_id).map(PaladinSpell::Seal)
            }
            "seal_of_command_proc" if has("seal_of_command") => {
                Some(PaladinSpell::SealOfCommandProc)
            }
            "seal_of_righteousness_proc" => effects.iter().find_map(|effect| match effect {
                Effect::SealOfRighteousness { ranks, .. } => ranks
                    .iter()
                    .find(|rank| rank.proc_spell_id == id.spell_id)
                    .and_then(|rank| seals::seal_index(effects, rank.seal_spell_id))
                    .map(PaladinSpell::SealOfRighteousnessProc),
                _ => None,
            }),
            "judgement_of_command" if damage && has("seal_of_command") => {
                Some(PaladinSpell::JudgementOfCommand)
            }
            "judgement_of_righteousness" if damage && has("seal_of_righteousness") => {
                Some(PaladinSpell::JudgementOfRighteousness)
            }
            "judgement" if has("judgement") => Some(PaladinSpell::Judgement),
            "seal_of_fury" => seals::seal_index(effects, id.spell_id).map(PaladinSpell::Seal),
            "seal_of_fury_proc" => effects.iter().find_map(|effect| match effect {
                Effect::SealOfFury { ranks, .. } => ranks
                    .iter()
                    .find(|rank| rank.proc_spell_id == id.spell_id)
                    .and_then(|rank| seals::seal_index(effects, rank.seal_spell_id))
                    .map(PaladinSpell::SealOfFuryProc),
                _ => None,
            }),
            "seal_of_light" => seals::seal_index(effects, id.spell_id).map(PaladinSpell::Seal),
            "seal_of_wisdom" | "seal_of_justice" => {
                seals::seal_index(effects, id.spell_id).map(PaladinSpell::Seal)
            }
            "seal_of_wisdom_proc" => effects.iter().find_map(|effect| match effect {
                Effect::SealOfWisdom { ranks } => ranks
                    .iter()
                    .find(|rank| rank.proc_spell_id == id.spell_id)
                    .and_then(|rank| seals::seal_index(effects, rank.seal_spell_id))
                    .map(PaladinSpell::SealOfWisdomProc),
                _ => None,
            }),
            "judgement_of_wisdom" => effects.iter().find_map(|effect| match effect {
                Effect::SealOfWisdom { ranks } => ranks
                    .iter()
                    .position(|rank| rank.judgement_spell_id == id.spell_id)
                    .map(PaladinSpell::JudgementOfWisdom),
                _ => None,
            }),
            "judgement_of_justice" => effects.iter().find_map(|effect| match effect {
                Effect::SealOfJustice {
                    judgement_spell_id, ..
                } if *judgement_spell_id == id.spell_id => Some(PaladinSpell::JudgementOfJustice),
                _ => None,
            }),
            "seal_of_light_proc" => effects.iter().find_map(|effect| match effect {
                Effect::SealOfLight { ranks, .. } => ranks
                    .iter()
                    .find(|rank| rank.proc_spell_id == id.spell_id)
                    .and_then(|rank| seals::seal_index(effects, rank.seal_spell_id))
                    .map(PaladinSpell::SealOfLightProc),
                _ => None,
            }),
            "judgement_of_light" => effects.iter().find_map(|effect| match effect {
                Effect::SealOfLight { ranks, .. } => ranks
                    .iter()
                    .position(|rank| rank.judgement_spell_id == id.spell_id)
                    .map(PaladinSpell::JudgementOfLight),
                _ => None,
            }),
            "judgement_of_fury" if damage && has("seal_of_fury") => {
                Some(PaladinSpell::JudgementOfFury)
            }
            "hammer_of_the_righteous" => effects.iter().find_map(|effect| match effect {
                Effect::HammerOfTheRighteous { spell_id, .. } if *spell_id == id.spell_id => {
                    Some(PaladinSpell::HammerOfTheRighteous)
                }
                _ => None,
            }),
            "holy_strike" if damage => effects.iter().find_map(|effect| match effect {
                Effect::HolyStrike { ranks } => ranks
                    .iter()
                    .position(|rank| rank.spell_id == id.spell_id)
                    .map(PaladinSpell::HolyStrike),
                _ => None,
            }),
            "hammer_of_wrath" if damage && has("hammer_of_wrath") => {
                Some(PaladinSpell::HammerOfWrath)
            }
            "consecration" if spell.dot.is_some() => {
                effects.iter().find_map(|effect| match effect {
                    Effect::Consecration { ranks, .. } => ranks
                        .iter()
                        .position(|rank| rank.spell_id == id.spell_id)
                        .map(PaladinSpell::Consecration),
                    _ => None,
                })
            }
            "holy_shock" => effects.iter().find_map(|effect| match effect {
                Effect::HolyShock { ranks } => ranks
                    .iter()
                    .position(|rank| rank.spell_id == id.spell_id)
                    .map(PaladinSpell::HolyShock),
                _ => None,
            }),
            "righteous_fury" if has("righteous_fury") => Some(PaladinSpell::RighteousFury),
            "exorcism" if damage && has("exorcism") => Some(PaladinSpell::Exorcism),
            "lights_vigil_strike" if damage && has("lights_vigil") => {
                Some(PaladinSpell::LightsVigilStrike)
            }
            "lights_vigil" => effects.iter().find_map(|effect| match effect {
                Effect::LightsVigil { ranks } => ranks
                    .iter()
                    .position(|rank| rank.spell_id == id.spell_id)
                    .map(PaladinSpell::LightsVigil),
                _ => None,
            }),
            "lights_vigil_ally" => effects.iter().find_map(|effect| match effect {
                Effect::LightsVigilAlly { ranks } => ranks
                    .iter()
                    .position(|rank| rank.spell_id == id.spell_id)
                    .map(PaladinSpell::VigilAlly),
                _ => None,
            }),
            "holy_wrath" if damage && has("holy_wrath") => Some(PaladinSpell::HolyWrath),
            "holy_light" | "flash_of_light" | "holy_shock_heal" => {
                effects.iter().find_map(|effect| match effect {
                    Effect::PaladinHeals { ranks, .. } => ranks
                        .iter()
                        .position(|rank| rank.spell_id == id.spell_id)
                        .map(PaladinSpell::Heal),
                    _ => None,
                })
            }
            "lay_on_hands" => effects.iter().find_map(|effect| match effect {
                Effect::LayOnHands { ranks } => ranks
                    .iter()
                    .position(|rank| rank.spell_id == id.spell_id)
                    .map(PaladinSpell::LayOnHands),
                _ => None,
            }),
            "retribution_aura"
            | "devotion_aura"
            | "concentration_aura"
            | "fire_resistance_aura"
            | "frost_resistance_aura"
            | "shadow_resistance_aura" => effects.iter().find_map(|effect| match effect {
                Effect::PaladinAuraCast { ranks } => ranks
                    .iter()
                    .position(|rank| rank.spell_id == id.spell_id)
                    .map(PaladinSpell::AuraCast),
                _ => None,
            }),
            "seal_of_the_crusader" => {
                seals::seal_index(effects, id.spell_id).map(PaladinSpell::Seal)
            }
            "judgement_of_the_crusader" => effects.iter().find_map(|effect| match effect {
                Effect::SealOfTheCrusader { ranks, .. } => ranks
                    .iter()
                    .position(|rank| rank.judgement_spell_id == id.spell_id)
                    .map(PaladinSpell::JudgementOfTheCrusader),
                _ => None,
            }),
            "swift_judgement" if has("swift_judgement") => Some(PaladinSpell::SwiftJudgement),
            "second_judgement" if has("second_judgement") => Some(PaladinSpell::SecondJudgement),
            "templars_bulwark" if has("templars_bulwark") => Some(PaladinSpell::TemplarsBulwark),
            "holy_shield" => effects.iter().find_map(|effect| match effect {
                Effect::HolyShield { ranks, .. } => ranks
                    .iter()
                    .position(|rank| rank.spell_id == id.spell_id)
                    .map(PaladinSpell::HolyShield),
                _ => None,
            }),
            "divine_favor" => effects.iter().find_map(|effect| match effect {
                Effect::DivineFavor { spell_id, .. } if *spell_id == id.spell_id => {
                    Some(PaladinSpell::DivineFavor)
                }
                _ => None,
            }),
            "oath_of_light" => effects.iter().find_map(|effect| match effect {
                Effect::OathOfLight { spell_id, .. } if *spell_id == id.spell_id => {
                    Some(PaladinSpell::OathOfLight)
                }
                _ => None,
            }),
            _ => None,
        }
    }

    /// The class auras on the target, by label: the members of the "Judgement of the
    /// Crusader" category.
    fn target_auras(prepared: &PreparedV2) -> Vec<(String, PaladinAura)> {
        let crusader = prepared
            .effects
            .iter()
            .any(|effect| matches!(effect, Effect::SealOfTheCrusader { .. }));
        prepared
            .effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::ExclusiveCategory {
                    unit,
                    category,
                    members,
                    ..
                } if crusader && unit == "target" && category == "Judgement of the Crusader" => {
                    Some(members)
                }
                _ => None,
            })
            .flatten()
            .enumerate()
            .map(|(position, member)| {
                (
                    member.aura.clone(),
                    PaladinAura::CrusaderJudgement(position),
                )
            })
            .collect()
    }

    /// The class auras of a prepared input, by unit and label.
    fn auras(prepared: &PreparedV2) -> Vec<(String, PaladinAura)> {
        let mut auras = Vec::new();
        let mut seal = 0;
        for effect in &prepared.effects {
            match effect {
                Effect::JudgementRefresh { trigger_aura, .. } => {
                    auras.push((trigger_aura.clone(), PaladinAura::JudgementRefresh))
                }
                Effect::Vengeance {
                    trigger_aura, aura, ..
                } => {
                    auras.push((trigger_aura.clone(), PaladinAura::VengeanceTrigger));
                    auras.push((aura.clone(), PaladinAura::Vengeance));
                }
                Effect::Vindication { trigger_aura, .. } => {
                    auras.push((trigger_aura.clone(), PaladinAura::VindicationTrigger))
                }
                Effect::SanctifiedJudgement { trigger_aura, .. } => {
                    auras.push((trigger_aura.clone(), PaladinAura::SanctifiedJudgement))
                }
                Effect::SacredArbiter { trigger_aura, .. } => {
                    auras.push((trigger_aura.clone(), PaladinAura::SacredArbiter))
                }
                Effect::TwistOfLight { trigger_aura, .. } => {
                    auras.push((trigger_aura.clone(), PaladinAura::TwistOfLight))
                }
                Effect::UnsealedWrath { trigger_aura, .. } => {
                    auras.push((trigger_aura.clone(), PaladinAura::UnsealedWrath))
                }
                Effect::DivineFavor { aura, .. } => {
                    auras.push((aura.clone(), PaladinAura::DivineFavor))
                }
                Effect::OathOfLight { aura, .. } => {
                    auras.push((aura.clone(), PaladinAura::OathOfLight))
                }
                Effect::RighteousFury {
                    aura,
                    instrument_of_law,
                    ..
                } => {
                    auras.push((aura.clone(), PaladinAura::RighteousFury));
                    if let Some(law) = instrument_of_law {
                        auras.push((law.aura.clone(), PaladinAura::InstrumentOfLaw));
                    }
                }
                Effect::SwiftJudgement { aura, .. } => {
                    auras.push((aura.clone(), PaladinAura::SwiftJudgement))
                }
                Effect::Redoubt {
                    trigger_aura, aura, ..
                } => {
                    auras.push((trigger_aura.clone(), PaladinAura::RedoubtTrigger));
                    auras.push((aura.clone(), PaladinAura::Redoubt));
                }
                Effect::ShieldSpecialization { trigger_aura, .. } => {
                    auras.push((trigger_aura.clone(), PaladinAura::ShieldSpecialization))
                }
                Effect::Reckoning {
                    block_aura,
                    crit_aura,
                    charges_aura,
                    ..
                } => {
                    auras.push((block_aura.clone(), PaladinAura::ReckoningBlock));
                    auras.push((crit_aura.clone(), PaladinAura::ReckoningCrit));
                    auras.push((charges_aura.clone(), PaladinAura::ReckoningCharges));
                }
                Effect::IronCreed {
                    trigger_aura, aura, ..
                } => {
                    auras.push((trigger_aura.clone(), PaladinAura::IronCreedTrigger));
                    auras.push((aura.clone(), PaladinAura::IronCreed));
                }
                Effect::HolyShield { ranks, .. } => {
                    for (position, rank) in ranks.iter().enumerate() {
                        auras.push((rank.aura.clone(), PaladinAura::HolyShield(position)));
                    }
                }
                Effect::EyeForAnEye { trigger_aura, .. } => {
                    auras.push((trigger_aura.clone(), PaladinAura::EyeForAnEye))
                }
                _ => {}
            }
        }
        let hastes = prepared.effects.iter().filter_map(|effect| match effect {
            Effect::HolyLightHaste {
                trigger_aura, aura, ..
            } => Some((trigger_aura, aura)),
            _ => None,
        });
        for (position, (trigger_aura, aura)) in hastes.enumerate() {
            auras.push((trigger_aura.clone(), PaladinAura::HolyLightHaste(position)));
            auras.push((aura.clone(), PaladinAura::HolyLightHasteAura(position)));
        }
        for effect in &prepared.effects {
            if let Effect::Illumination { trigger_aura, .. } = effect {
                auras.push((trigger_aura.clone(), PaladinAura::Illumination));
            }
        }
        // Seal auras in the order seals::bind builds the seals.
        for effect in &prepared.effects {
            if let Effect::SealOfCommand { ranks, .. } = effect {
                for rank in ranks {
                    auras.push((rank.aura.clone(), PaladinAura::Seal(seal)));
                    seal += 1;
                }
            }
        }
        for effect in &prepared.effects {
            if let Effect::SealOfRighteousness { ranks, .. } = effect {
                for rank in ranks {
                    auras.push((rank.aura.clone(), PaladinAura::Seal(seal)));
                    seal += 1;
                }
            }
        }
        for effect in &prepared.effects {
            if let Effect::SealOfTheCrusader { ranks, .. } = effect {
                for rank in ranks {
                    auras.push((rank.aura.clone(), PaladinAura::Seal(seal)));
                    seal += 1;
                }
            }
        }
        for effect in &prepared.effects {
            if let Effect::SealOfFury { ranks, .. } = effect {
                for rank in ranks {
                    auras.push((rank.aura.clone(), PaladinAura::Seal(seal)));
                    seal += 1;
                }
            }
        }
        for effect in &prepared.effects {
            if let Effect::SealOfLight { ranks, .. } = effect {
                for rank in ranks {
                    auras.push((rank.aura.clone(), PaladinAura::Seal(seal)));
                    seal += 1;
                }
            }
        }
        for effect in &prepared.effects {
            if let Effect::SealOfWisdom { ranks } = effect {
                for rank in ranks {
                    auras.push((rank.aura.clone(), PaladinAura::Seal(seal)));
                    seal += 1;
                }
            }
        }
        for effect in &prepared.effects {
            if let Effect::SealOfJustice { aura, .. } = effect {
                auras.push((aura.clone(), PaladinAura::Seal(seal)));
                seal += 1;
            }
        }
        auras
    }

    /// Build a fight for a prepared input that passed the coverage gate.
    pub(crate) fn fight(prepared: &PreparedV2) -> Result<Fight<PaladinAgent>, String> {
        let auras = Self::auras(prepared);
        let target_auras = Self::target_auras(prepared);
        let spell = |exported: &ExportedSpell| PaladinAgent::spell(prepared, exported);
        let mut fight = Fight::new(prepared, PaladinAgent::default(), spell, |unit, label| {
            let auras = match unit {
                "player" => &auras,
                "target" => &target_auras,
                _ => return None,
            };
            auras
                .iter()
                .find(|(name, _)| name == label)
                .map(|(_, kind)| *kind)
        })?;
        let effects = &prepared.effects;
        fight.agent.seals = seals::bind(&mut fight, effects)?;
        fight.agent.undead_or_demon = matches!(
            prepared.target.mob_type.as_deref(),
            Some("MobTypeUndead" | "MobTypeDemon")
        );
        for effect in effects {
            match effect {
                Effect::JudgementRefresh {
                    judgement_auras, ..
                } => {
                    let bound = judgement_refresh::bind(&fight, judgement_auras)?;
                    fight.agent.judgement_refresh = Some(Rc::new(bound));
                }
                Effect::Judgement { wake_delay_ns, .. } => {
                    fight.agent.judgement_wake_delay = *wake_delay_ns;
                }
                Effect::HammerOfTheRighteous {
                    weapon_dps,
                    max_targets,
                    ..
                } => {
                    fight.agent.hammer_of_the_righteous =
                        Some((*weapon_dps, (*max_targets).max(1) as usize));
                }
                Effect::HolyStrike { ranks } => {
                    fight.agent.holy_strike =
                        ranks.iter().map(|rank| rank.weapon_percent).collect();
                }
                Effect::Consecration {
                    ranks,
                    consecrated_ground,
                } => {
                    fight.agent.consecration = ranks.clone();
                    if let Some(ground) = consecrated_ground {
                        let aura = fight.trackers[Side::Target.index()]
                            .find(&ground.aura)
                            .map(|index| AuraRef {
                                side: Side::Target,
                                index,
                            })
                            .ok_or_else(|| {
                                format!("target aura {} is not registered", ground.aura)
                            })?;
                        fight.agent.consecrated_ground = Some((aura, ground.multiplier));
                    }
                }
                Effect::LightsVigil { ranks } => {
                    for rank in ranks {
                        let aura = fight.trackers[Side::Target.index()]
                            .find(&rank.aura)
                            .map(|index| AuraRef {
                                side: Side::Target,
                                index,
                            })
                            .ok_or_else(|| {
                                format!("target aura {} is not registered", rank.aura)
                            })?;
                        let strike = fight
                            .spells
                            .iter()
                            .position(|spell| {
                                spell.id.spell_id == rank.strike_spell_id && spell.id.tag == 0
                            })
                            .ok_or_else(|| {
                                format!("strike {} is not registered", rank.strike_spell_id)
                            })?;
                        let metrics = fight.new_mana_metrics(rank.metrics_action_id.clone());
                        fight.agent.vigils.push(Vigil {
                            aura,
                            strike,
                            refund: rank.refund,
                            metrics,
                            cost: 0.0,
                        });
                    }
                }
                Effect::LightsVigilAlly { ranks } => {
                    for rank in ranks {
                        let heal = fight
                            .spells
                            .iter()
                            .position(|spell| {
                                spell.id.spell_id == rank.heal_spell_id && spell.id.tag == 0
                            })
                            .ok_or_else(|| {
                                format!("vigil heal {} is not registered", rank.heal_spell_id)
                            })?;
                        fight.agent.vigil_allies.push(holy::VigilAlly {
                            aura: fight.player_aura(&rank.aura)?,
                            heal,
                            average: rank.average,
                            variance: rank.variance,
                            players: rank.players,
                        });
                    }
                }
                Effect::HolyShock { ranks } => {
                    fight.agent.holy_shock =
                        ranks.iter().map(|rank| (rank.min, rank.max)).collect();
                }
                Effect::RighteousFury {
                    aura,
                    threat_percent,
                    instrument_of_law,
                    all_threat_multiplier,
                    ..
                } => {
                    let law = instrument_of_law
                        .as_ref()
                        .map(|law| (law.aura.as_str(), law.multiplier));
                    let mut bound =
                        RighteousFury::bind(&mut fight, effects, aura, *threat_percent, law)?;
                    bound.all_threat = all_threat_multiplier.unwrap_or(1.0);
                    fight.agent.righteous_fury = Some(bound);
                }
                Effect::Shieldsworn {} => fight.agent.shieldsworn = true,
                Effect::SwiftJudgement {
                    aura,
                    cost_percent_add,
                    ..
                } => {
                    let bound = SwiftJudgement::bind(&mut fight, aura, *cost_percent_add)?;
                    fight.agent.swift_judgement = Some(Rc::new(bound));
                }
                Effect::TemplarsBulwark {
                    aura, health_share, ..
                } => {
                    let forbearance = fight.player_aura("Forbearance")?;
                    fight.agent.forbearance = Some(forbearance);
                    fight.agent.templars_bulwark = Some(TemplarsBulwark {
                        aura: fight.player_aura(aura)?,
                        forbearance,
                        health_share: *health_share,
                        strength: 0.0,
                    });
                }
                Effect::Redoubt {
                    aura, proc_chance, ..
                } => {
                    let bound = Redoubt::bind(&fight, effects, aura, *proc_chance)?;
                    fight.agent.redoubt = Some(Rc::new(bound));
                }
                Effect::ShieldSpecialization {
                    proc_chance,
                    mana_share,
                    metrics_action_id,
                    no_threat,
                    ..
                } => {
                    let metrics = fight.new_mana_metrics(metrics_action_id.clone());
                    fight.resources[metrics].no_threat = *no_threat;
                    fight.agent.shield_specialization = Some(Rc::new(ShieldSpecialization {
                        chance: *proc_chance,
                        mana_share: *mana_share,
                        metrics,
                    }));
                }
                Effect::Reckoning {
                    block_chance,
                    crit_chance,
                    charges_aura,
                    icd_ns,
                    spender_spells,
                    ..
                } => {
                    let count = fight.spells.len();
                    if spender_spells.iter().any(|&spell| spell >= count) {
                        return Err("Reckoning names a spell outside the spellbook".into());
                    }
                    let mut spends = vec![false; count];
                    for &spell in spender_spells {
                        spends[spell] = true;
                    }
                    // The triggers' shared cooldown: a timer of its own, reset with the others.
                    fight.timers.push(crate::core::time::STARTING_CD_TIME);
                    let timer = fight.timers.len() - 1;
                    fight.agent.reckoning = Some(Rc::new(Reckoning {
                        block_chance: *block_chance,
                        crit_chance: *crit_chance,
                        charges: fight.player_aura(charges_aura)?,
                        icd: (timer, *icd_ns),
                        spends,
                    }));
                }
                Effect::IronCreed { aura, .. } => {
                    let bound = IronCreed::bind(&fight, effects, aura)?;
                    fight.agent.iron_creed = Some(Rc::new(bound));
                }
                Effect::HolyShield { ranks, can_block } => {
                    for rank in ranks {
                        let bound = HolyShield::bind(
                            &fight,
                            effects,
                            &rank.aura,
                            rank.proc_spell,
                            rank.charges,
                            rank.damage,
                        )?;
                        fight.agent.holy_shield.push(Rc::new(bound));
                    }
                    fight.agent.holy_shield_can_block = *can_block;
                }
                Effect::DivineFavor {
                    aura, crit, spells, ..
                } => {
                    let bound = DivineFavor::bind(&mut fight, aura, *crit, spells)?;
                    fight.agent.divine_favor = Some(Rc::new(bound));
                }
                Effect::OathOfLight {
                    aura,
                    holy_light,
                    flash_of_light,
                    holy_shock,
                    boundless_light_crit,
                    holder_share,
                    ..
                } => {
                    let bound = OathOfLight::bind(
                        &mut fight,
                        aura,
                        (*holy_light, *flash_of_light, *holy_shock),
                        *boundless_light_crit,
                        *holder_share,
                    )?;
                    fight.agent.oath = Some(bound);
                }
                Effect::Vengeance {
                    aura,
                    per_stack,
                    spells,
                    ..
                } => {
                    let bound = Vengeance::bind(&mut fight, aura, *per_stack, spells)?;
                    fight.agent.vengeance = Some(bound);
                }
                Effect::Vindication {
                    proc_chance,
                    aura,
                    target_aura,
                    ..
                } => {
                    let target_aura = fight.trackers[Side::Target.index()]
                        .find(target_aura)
                        .map(|index| AuraRef {
                            side: Side::Target,
                            index,
                        })
                        .ok_or_else(|| format!("target aura {target_aura} is not registered"))?;
                    let aura = match aura {
                        Some(aura) => Some(fight.player_aura(aura)?),
                        None => None,
                    };
                    fight.agent.vindication = Some(Vindication {
                        chance: *proc_chance,
                        aura,
                        target_aura,
                    });
                }
                Effect::SanctifiedJudgement {
                    proc_chance,
                    refund,
                    metrics_action_id,
                    ..
                } => {
                    let metrics = fight.new_mana_metrics(metrics_action_id.clone());
                    fight.agent.sanctified_judgement = Some(SanctifiedJudgement {
                        chance: *proc_chance,
                        refund: *refund,
                        metrics,
                    });
                }
                Effect::SacredArbiter {
                    judgement_auras, ..
                } => {
                    let bound = judgement_refresh::bind(&fight, judgement_auras)?;
                    fight.agent.sacred_arbiter = Rc::new(bound.judgements().to_vec());
                }
                Effect::TwistOfLight { echoes, .. } => {
                    fight.agent.echoes = twist_of_light::bind(&fight, echoes)?;
                }
                Effect::UnsealedWrath { echoes, .. } => {
                    fight.agent.unsealed_echoes = unsealed_wrath::bind(&fight, echoes)?;
                }
                Effect::SealOfTheCrusader { ranks, .. } => {
                    for rank in ranks {
                        let aura = target_aura(&fight, &rank.judgement_aura)?;
                        fight.agent.crusader_auras.push(aura);
                    }
                    let members = effects.iter().find_map(|effect| match effect {
                        Effect::ExclusiveCategory {
                            unit,
                            category,
                            members,
                            ..
                        } if unit == "target" && category == "Judgement of the Crusader" => {
                            Some(members)
                        }
                        _ => None,
                    });
                    for member in members.into_iter().flatten() {
                        // A member none of the ranks names is the raid's debuff.
                        let raid = !ranks.iter().any(|rank| rank.judgement_aura == member.aura);
                        target_aura(&fight, &member.aura)?;
                        fight.agent.crusader_judgements.push(CrusaderJudgement {
                            bonus: member.priority,
                            raid,
                        });
                    }
                }
                Effect::HolyLightHaste {
                    aura,
                    on_crit,
                    cast_time_ns,
                    holy_light_spells,
                    ..
                } => {
                    let bound = HolyLightHaste::bind(
                        &mut fight,
                        aura,
                        *on_crit,
                        *cast_time_ns,
                        holy_light_spells,
                    )?;
                    fight.agent.holy_light_haste.push(bound);
                }
                Effect::PaladinHeals {
                    ranks,
                    flash_of_light_bonus,
                    blessing_holy_light,
                    blessing_flash_of_light,
                    player,
                    target,
                } => {
                    let bound = Rc::new(Heals::bind(
                        ranks,
                        *flash_of_light_bonus,
                        (*blessing_holy_light, *blessing_flash_of_light),
                        player,
                        target,
                    )?);
                    // The swap set's weapon moves the healing power the heals read.
                    let worn = fight.item_swap_worn_effects().and_then(|worn| {
                        worn.into_iter().find_map(|effect| match effect {
                            Effect::PaladinHeals {
                                ranks,
                                flash_of_light_bonus,
                                blessing_holy_light,
                                blessing_flash_of_light,
                                player,
                                target,
                            } => Some(Heals::bind(
                                &ranks,
                                flash_of_light_bonus,
                                (blessing_holy_light, blessing_flash_of_light),
                                &player,
                                &target,
                            )),
                            _ => None,
                        })
                    });
                    if let Some(worn) = worn {
                        fight.agent.heals_by_set = Some([bound.clone(), Rc::new(worn?)]);
                    }
                    fight.agent.heals = Some(bound);
                }
                Effect::LayOnHands { ranks } => {
                    for rank in ranks {
                        let metrics = fight.new_mana_metrics(ActionId {
                            spell_id: rank.spell_id,
                            ..ActionId::default()
                        });
                        fight.agent.lay_on_hands.push((rank.mana, metrics));
                    }
                }
                Effect::SealOfLight { ranks, .. } => {
                    for rank in ranks {
                        let aura = target_aura(&fight, &rank.judgement_aura)?;
                        fight.agent.light_judgements.push(aura);
                    }
                }
                Effect::SealOfWisdom { ranks } => {
                    for rank in ranks {
                        let aura = target_aura(&fight, &rank.judgement_aura)?;
                        fight.agent.wisdom_judgements.push(aura);
                    }
                }
                Effect::SealOfJustice { judgement_aura, .. } => {
                    fight.agent.justice_judgement = Some(target_aura(&fight, judgement_aura)?);
                }
                Effect::PaladinAuraCast { ranks } => {
                    for rank in ranks {
                        let aura = fight.player_aura(&rank.aura)?;
                        fight.agent.aura_casts.push(aura);
                    }
                }
                Effect::Illumination {
                    proc_chance,
                    refund,
                    metrics_action_id,
                    ..
                } => {
                    let metrics = fight.new_mana_metrics(metrics_action_id.clone());
                    fight.agent.illumination = Some(Illumination {
                        chance: *proc_chance,
                        refund: *refund,
                        metrics,
                    });
                }
                Effect::EyeForAnEye {
                    spell_id, share, ..
                } => {
                    let spell = fight
                        .spells
                        .iter()
                        .position(|spell| spell.id.spell_id == *spell_id && spell.id.tag == 0)
                        .ok_or_else(|| format!("Eye for an Eye {spell_id} is not registered"))?;
                    fight.agent.eye_for_an_eye = Some(EyeForAnEye {
                        spell,
                        share: *share,
                        reflected: 0.0,
                    });
                }
                _ => {}
            }
        }
        Ok(fight)
    }
}

/// A target aura by label.
fn target_aura(fight: &Fight<PaladinAgent>, label: &str) -> Result<AuraRef, String> {
    fight.trackers[Side::Target.index()]
        .find(label)
        .map(|index| AuraRef {
            side: Side::Target,
            index,
        })
        .ok_or_else(|| format!("target aura {label} is not registered"))
}

impl PaladinAgent {
    /// A bound talent or spell state.
    fn bound<T>(state: &Option<Rc<T>>) -> Rc<T> {
        Rc::clone(state.as_ref().expect("the effect is bound"))
    }

    /// The gain and loss of the tanking auras.
    fn protection_toggle(fight: &mut Fight<Self>, kind: PaladinAura, active: bool) {
        match kind {
            PaladinAura::RighteousFury => {
                let fury = fight
                    .agent
                    .righteous_fury
                    .clone()
                    .expect("Righteous Fury is bound");
                if active {
                    fury.on_gain(fight);
                } else {
                    fury.on_expire(fight);
                }
            }
            PaladinAura::InstrumentOfLaw => {
                let fury = fight
                    .agent
                    .righteous_fury
                    .as_ref()
                    .expect("Righteous Fury is bound");
                let multiplier = fury.law.expect("Instrument of Law is bound").1;
                if active {
                    protection::law_gain(fight, multiplier);
                } else {
                    protection::law_expire(fight, multiplier);
                }
            }
            PaladinAura::SwiftJudgement => {
                let swift = Self::bound(&fight.agent.swift_judgement);
                if active {
                    swift.on_gain(fight);
                } else {
                    swift.on_expire(fight);
                }
            }
            PaladinAura::Redoubt => Self::bound(&fight.agent.redoubt).toggle(fight, active),
            PaladinAura::IronCreed => Self::bound(&fight.agent.iron_creed).toggle(fight, active),
            PaladinAura::HolyShield(rank) => {
                Rc::clone(&fight.agent.holy_shield[rank]).toggle(fight, active)
            }
            _ => {}
        }
    }

    fn divine_favor(fight: &Fight<Self>) -> Rc<DivineFavor> {
        Rc::clone(
            fight
                .agent
                .divine_favor
                .as_ref()
                .expect("Divine Favor is bound"),
        )
    }
}

impl Agent for PaladinAgent {
    type Spell = PaladinSpell;
    type Aura = PaladinAura;

    fn on_item_swap(fight: &mut Fight<Self>, worn: bool) {
        seals::on_item_swap(fight, worn);
        if let Some(by_set) = &fight.agent.heals_by_set {
            fight.agent.heals = Some(by_set[usize::from(worn)].clone());
        }
    }

    fn apply_effects(
        fight: &mut Fight<Self>,
        spell: SpellId,
        target: Side,
        behavior: PaladinSpell,
    ) {
        match behavior {
            PaladinSpell::Seal(seal) => seals::apply(fight, seal),
            PaladinSpell::SealOfCommandProc => seals::command_proc(fight, spell, target),
            PaladinSpell::SealOfRighteousnessProc(seal) => {
                seals::righteousness_proc(fight, spell, target, seal)
            }
            PaladinSpell::JudgementOfCommand => judgement::command(fight, spell, target),
            PaladinSpell::JudgementOfRighteousness => {
                judgement::righteousness(fight, spell, target)
            }
            // seal_of_fury.go: the rolled damage on the same table as Judgement of
            // Righteousness, binary too.
            PaladinSpell::JudgementOfFury => judgement::righteousness(fight, spell, target),
            PaladinSpell::SealOfFuryProc(seal) => seals::fury_proc(fight, spell, target, seal),
            PaladinSpell::Judgement => {
                let delay = fight.agent.judgement_wake_delay;
                judgement::apply(fight, spell, target, delay);
            }
            PaladinSpell::HammerOfTheRighteous => {
                let (weapon_dps, max_targets) = fight
                    .agent
                    .hammer_of_the_righteous
                    .expect("Hammer of the Righteous is bound");
                strikes::hammer_of_the_righteous(fight, spell, target, weapon_dps, max_targets);
            }
            PaladinSpell::HolyStrike(rank) => {
                let weapon_percent = fight.agent.holy_strike[rank];
                strikes::holy_strike(fight, spell, target, weapon_percent);
            }
            PaladinSpell::HammerOfWrath => strikes::hammer_of_wrath(fight, spell, target),
            PaladinSpell::Exorcism => strikes::exorcism(fight, spell, target),
            PaladinSpell::LightsVigil(rank) => holy::lights_vigil(fight, spell, rank),
            PaladinSpell::LightsVigilStrike => holy::vigil_strike(fight, spell, target),
            PaladinSpell::VigilAlly(rank) => holy::swear_vigil(fight, rank),
            PaladinSpell::HolyWrath => {
                let hits = fight.agent.undead_or_demon;
                strikes::holy_wrath(fight, spell, hits);
            }
            PaladinSpell::Consecration(_) => strikes::consecration(fight, spell, target),
            PaladinSpell::HolyShock(rank) => {
                let roll = fight.agent.holy_shock[rank];
                holy::holy_shock(fight, spell, target, roll);
            }
            PaladinSpell::DivineFavor => Self::divine_favor(fight).apply(fight),
            PaladinSpell::OathOfLight => {
                let oath = fight.agent.oath.expect("Oath of Light is bound");
                oath.apply(fight, target);
            }
            PaladinSpell::RighteousFury => {
                let aura = fight
                    .agent
                    .righteous_fury
                    .as_ref()
                    .expect("Righteous Fury is bound")
                    .aura;
                fight.activate_aura(aura);
            }
            PaladinSpell::SwiftJudgement => Self::bound(&fight.agent.swift_judgement).apply(fight),
            // Upstream #772's swift_judgement.go: the active seal's judgement, as Judgement casts.
            PaladinSpell::SecondJudgement => {
                let seal = seals::active_seal(fight).expect("Swift Judgement needs a seal");
                let judgement = fight.agent.seals.seals[seal].judgement;
                fight.cast(judgement, target);
            }
            PaladinSpell::TemplarsBulwark => fight
                .agent
                .templars_bulwark
                .expect("Templar's Bulwark is bound")
                .apply(fight),
            PaladinSpell::HolyShield(rank) => {
                Rc::clone(&fight.agent.holy_shield[rank]).apply(fight)
            }
            PaladinSpell::HolyShieldProc(rank) => {
                Rc::clone(&fight.agent.holy_shield[rank]).proc_damage(fight, spell, target)
            }
            PaladinSpell::JudgementOfTheCrusader(rank) => {
                let aura = fight.agent.crusader_auras[rank];
                crusader::judge(fight, spell, target, aura);
            }
            PaladinSpell::EyeForAnEye => {
                let eye = fight.agent.eye_for_an_eye.expect("Eye for an Eye is bound");
                eye.reflect(fight, spell, target);
            }
            PaladinSpell::Heal(rank) => {
                Self::bound(&fight.agent.heals).cast(fight, spell, target, rank)
            }
            PaladinSpell::LayOnHands(rank) => {
                let rank = fight.agent.lay_on_hands[rank];
                Self::bound(&fight.agent.heals).lay_on_hands(fight, spell, target, rank)
            }
            PaladinSpell::SealOfLightProc(seal) => seals::light_proc(fight, spell, seal),
            // seal_of_light.go: an outcome that always hits, then the rank's debuff.
            PaladinSpell::JudgementOfLight(rank) => {
                let aura = fight.agent.light_judgements[rank];
                crusader::judge(fight, spell, target, aura);
            }
            PaladinSpell::SealOfWisdomProc(seal) => seals::wisdom_proc(fight, seal),
            // seal_of_wisdom.go and seal_of_justice.go: the same as Judgement of Light.
            PaladinSpell::JudgementOfWisdom(rank) => {
                let aura = fight.agent.wisdom_judgements[rank];
                crusader::judge(fight, spell, target, aura);
            }
            PaladinSpell::JudgementOfJustice => {
                let aura = fight.agent.justice_judgement.expect("bound");
                crusader::judge(fight, spell, target, aura);
            }
            // auras.go registerAuraSpell: the cast turns the aura on.
            PaladinSpell::AuraCast(rank) => {
                let aura = fight.agent.aura_casts[rank];
                fight.activate_aura(aura);
            }
        }
    }

    fn extra_cast_condition(fight: &Fight<Self>, _spell: SpellId, behavior: PaladinSpell) -> bool {
        match behavior {
            PaladinSpell::Judgement | PaladinSpell::SecondJudgement => {
                seals::active_seal(fight).is_some()
            }
            PaladinSpell::HammerOfWrath => fight.is_execute_phase_20(),
            PaladinSpell::HolyShield(_) => fight.agent.holy_shield_can_block,
            PaladinSpell::Exorcism => fight.agent.undead_or_demon,
            PaladinSpell::TemplarsBulwark => !fight
                .agent
                .forbearance
                .is_some_and(|aura| fight.aura(aura).active),
            _ => true,
        }
    }

    fn should_activate(fight: &Fight<Self>, _spell: SpellId, behavior: PaladinSpell) -> bool {
        match behavior {
            PaladinSpell::SwiftJudgement => fight
                .agent
                .swift_judgement
                .as_ref()
                .expect("Swift Judgement is bound")
                .should_activate(fight),
            PaladinSpell::SecondJudgement => seals::active_seal(fight).is_some(),
            _ => true,
        }
    }

    fn modify_cast(fight: &mut Fight<Self>, spell: SpellId, behavior: PaladinSpell) {
        match behavior {
            PaladinSpell::HammerOfWrath
            | PaladinSpell::HolyWrath
            | PaladinSpell::Heal(_)
            | PaladinSpell::LightsVigil(_)
            | PaladinSpell::VigilAlly(_) => strikes::hold_melee_for_cast(fight, spell),
            _ => {}
        }
    }

    fn on_dot_tick(fight: &mut Fight<Self>, dot: DotId, behavior: PaladinSpell) {
        if let PaladinSpell::Consecration(rank) = behavior {
            let rank = fight.agent.consecration[rank].clone();
            strikes::consecration_tick(fight, dot, &rank);
        }
    }

    fn reset(fight: &mut Fight<Self>) {
        fight.agent.seals.current = None;
        if let Some(fury) = fight.agent.righteous_fury.as_mut() {
            fury.law_at_reset = true;
        }
        twist_of_light::reset(fight);
        unsealed_wrath::reset(fight);
    }

    fn caster_damage_multiplier(fight: &Fight<Self>, spell: SpellId, target: Side) -> Option<f64> {
        // Consecrated Ground's handler: Holy spells, while the target is marked.
        let (aura, multiplier) = fight.agent.consecrated_ground?;
        fight.aura(fight.aura_on(aura, target)).active.then(|| {
            if fight.spells[spell].school & 2 != 0 {
                multiplier
            } else {
                1.0
            }
        })
    }

    fn on_gain(fight: &mut Fight<Self>, _aura: AuraRef, kind: PaladinAura) {
        Self::protection_toggle(fight, kind, true);
        match kind {
            PaladinAura::Seal(seal) => seals::crusader_toggle(fight, seal, true),
            PaladinAura::HolyLightHasteAura(position) => {
                fight.agent.holy_light_haste[position].toggle(fight, true)
            }
            PaladinAura::CrusaderJudgement(member) => crusader::toggle(fight, member, true),
            _ => {}
        }
        if kind == PaladinAura::DivineFavor {
            Self::divine_favor(fight).on_gain(fight);
        }
        if kind == PaladinAura::OathOfLight {
            let oath = fight.agent.oath.expect("Oath of Light is bound");
            oath.on_gain(fight);
        }
        if kind == PaladinAura::Vengeance {
            fight
                .agent
                .vengeance
                .expect("Vengeance is bound")
                .on_gain(fight);
        }
    }

    fn on_expire(fight: &mut Fight<Self>, _aura: AuraRef, kind: PaladinAura) {
        Self::protection_toggle(fight, kind, false);
        match kind {
            PaladinAura::Seal(seal) => seals::crusader_toggle(fight, seal, false),
            PaladinAura::HolyLightHasteAura(position) => {
                fight.agent.holy_light_haste[position].toggle(fight, false)
            }
            PaladinAura::CrusaderJudgement(member) => crusader::toggle(fight, member, false),
            _ => {}
        }
        if kind == PaladinAura::DivineFavor {
            Self::divine_favor(fight).on_expire(fight);
        }
        if kind == PaladinAura::OathOfLight {
            let oath = fight.agent.oath.expect("Oath of Light is bound");
            oath.on_expire(fight);
        }
        if kind == PaladinAura::Vengeance {
            fight
                .agent
                .vengeance
                .expect("Vengeance is bound")
                .on_expire(fight);
        }
    }

    fn on_stacks_change(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: PaladinAura,
        _old: i32,
        new: i32,
    ) {
        if kind == PaladinAura::Vengeance {
            fight
                .agent
                .vengeance
                .expect("Vengeance is bound")
                .on_stacks_change(fight, new);
        }
    }

    fn on_cast_complete(fight: &mut Fight<Self>, aura: AuraRef, kind: PaladinAura, spell: SpellId) {
        if kind == PaladinAura::SwiftJudgement {
            Self::bound(&fight.agent.swift_judgement).on_cast_complete(fight, spell);
        }
        if kind == PaladinAura::DivineFavor {
            Self::divine_favor(fight).on_cast_complete(fight, spell);
        }
        if kind == PaladinAura::SanctifiedJudgement {
            fight
                .agent
                .sanctified_judgement
                .expect("Sanctified Judgement is bound")
                .on_cast_complete(fight, aura, spell);
        }
        if let PaladinAura::HolyLightHaste(position) = kind {
            if !fight.agent.holy_light_haste[position].on_crit {
                holy::holy_light_haste_trigger(fight, aura, spell, None);
            }
        }
        if let PaladinAura::HolyLightHasteAura(position) = kind {
            fight.agent.holy_light_haste[position].on_cast_complete(fight, spell);
        }
    }

    fn on_spell_hit_dealt(
        fight: &mut Fight<Self>,
        aura: AuraRef,
        kind: PaladinAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        match kind {
            PaladinAura::JudgementRefresh => {
                let refresh = fight
                    .agent
                    .judgement_refresh
                    .clone()
                    .expect("Judgement Refresh is bound");
                refresh.on_spell_hit_dealt(fight, spell, result);
            }
            PaladinAura::Seal(_) => seals::on_spell_hit_dealt(fight, aura, spell, result),
            PaladinAura::VengeanceTrigger => {
                Vengeance::on_spell_hit_dealt(fight, aura, spell, result)
            }
            PaladinAura::VindicationTrigger => fight
                .agent
                .vindication
                .expect("Vindication is bound")
                .on_spell_hit_dealt(fight, aura, spell, result),
            PaladinAura::SacredArbiter => {
                let judgements = Rc::clone(&fight.agent.sacred_arbiter);
                retribution::sacred_arbiter_hit(fight, &judgements, spell, result);
            }
            PaladinAura::TwistOfLight => twist_of_light::on_spell_hit_dealt(fight, spell, result),
            PaladinAura::UnsealedWrath => unsealed_wrath::on_spell_hit_dealt(fight, spell, result),
            PaladinAura::IronCreedTrigger => {
                Self::bound(&fight.agent.iron_creed).trigger(fight, aura, spell, result)
            }
            PaladinAura::ReckoningCharges => {
                Self::bound(&fight.agent.reckoning).on_charges_hit(fight, spell, result)
            }
            PaladinAura::HolyLightHaste(position)
                if fight.agent.holy_light_haste[position].on_crit =>
            {
                holy::holy_light_haste_trigger(fight, aura, spell, Some(result));
            }
            _ => {}
        }
    }

    fn on_enemy_hit_taken(
        fight: &mut Fight<Self>,
        aura: AuraRef,
        kind: PaladinAura,
        result: &SpellResult,
    ) {
        match kind {
            PaladinAura::RedoubtTrigger => {
                Self::bound(&fight.agent.redoubt).trigger(fight, aura, result)
            }
            PaladinAura::Redoubt => Self::bound(&fight.agent.redoubt).block(fight, result),
            PaladinAura::ShieldSpecialization => {
                Self::bound(&fight.agent.shield_specialization).trigger(fight, aura, result)
            }
            PaladinAura::ReckoningBlock => {
                Self::bound(&fight.agent.reckoning).trigger(fight, aura, true, result)
            }
            PaladinAura::ReckoningCrit => {
                Self::bound(&fight.agent.reckoning).trigger(fight, aura, false, result)
            }
            PaladinAura::HolyShield(rank) => {
                Rc::clone(&fight.agent.holy_shield[rank]).block(fight, result)
            }
            PaladinAura::EyeForAnEye => EyeForAnEye::trigger(fight, aura, result),
            _ => {}
        }
    }

    /// The same listeners on a spell that hits the paladin, as the Goblin Sapper Charge's half
    /// does: Redoubt's trigger hears only melee hits, the rest only the outcome.
    fn on_spell_hit_taken(
        fight: &mut Fight<Self>,
        aura: AuraRef,
        kind: PaladinAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        if kind == PaladinAura::RedoubtTrigger && !fight.spells[spell].melee_proc {
            return;
        }
        Self::on_enemy_hit_taken(fight, aura, kind, result);
    }

    /// Go registers the talents' modifiers before the seals': Templar's Bulwark's shield
    /// absorbs first, then each Seal of Fury rank's.
    fn player_damage_taken_modifiers(fight: &mut Fight<Self>, result: &mut SpellResult) {
        TemplarsBulwark::absorb(fight, result);
        seals::fury_absorb(fight, result);
    }

    fn on_class_action(fight: &mut Fight<Self>, tag: u32) {
        if tag >= seals::FURY_DEAL_TAG {
            seals::fury_deal(fight, (tag - seals::FURY_DEAL_TAG) as usize);
        }
    }

    fn on_heal_dealt(
        fight: &mut Fight<Self>,
        aura: AuraRef,
        kind: PaladinAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        match kind {
            PaladinAura::Illumination => fight
                .agent
                .illumination
                .expect("Illumination is bound")
                .on_heal_dealt(fight, aura, spell, result),
            PaladinAura::HolyLightHaste(position)
                if fight.agent.holy_light_haste[position].on_crit =>
            {
                holy::holy_light_haste_trigger(fight, aura, spell, Some(result));
            }
            _ => {}
        }
    }

    fn on_delayed_proc(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: PaladinAura,
        spell: SpellId,
        result: SpellResult,
    ) {
        match kind {
            PaladinAura::Seal(seal) => seals::on_delayed_proc(fight, seal, result.target),
            PaladinAura::VengeanceTrigger => fight
                .agent
                .vengeance
                .expect("Vengeance is bound")
                .on_delayed_proc(fight),
            PaladinAura::VindicationTrigger => fight
                .agent
                .vindication
                .expect("Vindication is bound")
                .on_delayed_proc(fight),
            PaladinAura::ReckoningBlock | PaladinAura::ReckoningCrit => {
                Self::bound(&fight.agent.reckoning).add_charge(fight)
            }
            PaladinAura::ReckoningCharges => Self::bound(&fight.agent.reckoning).spend(fight),
            PaladinAura::IronCreedTrigger => {
                let aura = Self::bound(&fight.agent.iron_creed).aura;
                fight.activate_aura(aura);
            }
            PaladinAura::HolyLightHaste(position) => {
                let aura = fight.agent.holy_light_haste[position].aura;
                fight.activate_aura(aura);
            }
            PaladinAura::Illumination => fight
                .agent
                .illumination
                .expect("Illumination is bound")
                .on_delayed_proc(fight, spell),
            PaladinAura::EyeForAnEye => {
                let mut eye = fight.agent.eye_for_an_eye.expect("Eye for an Eye is bound");
                eye.reflected = (result.damage * eye.share).min(fight.player_max_health() / 2.0);
                fight.agent.eye_for_an_eye = Some(eye);
                fight.cast(eye.spell, result.attacker);
            }
            _ => {}
        }
    }
}
