//! The Shaman class agent for the fight runtime: class spells and auras by name, and the
//! hooks that dispatch to each spell's and talent's module.

use std::rc::Rc;

use crate::{
    contracts::prepared_v2::{Effect, PreparedV2, Spell as ExportedSpell},
    core::{
        fight::{
            heals::HealBook, Agent, AuraRef, DotId, Fight, Side, SpellId, SpellResult,
            PRIORITY_AUTO,
        },
        queue::Handle,
    },
};

use super::{
    masks::{is_class, HOLDS_MELEE},
    spells::{
        chain_lightning::{self, ChainLightning},
        earth_shock, fire_nova, flame_shock, lava_burst,
        lightning_bolt::{self, Overload},
        magma_totem,
        rockbiter_threat::RockbiterThreat,
        searing_totem::{self, SearingTotem},
        stormstrike::{self, Stormstrike},
        totems::{self, Expirations, ManaTide, StrengthOfEarth},
        water_shield::WaterShield,
        weapon_imbues, weapon_sync,
        windfury_totem::{self, WindfuryTotem},
        windfury_weapon::{self, WindfuryWeapon},
    },
    talents::{
        elemental_devastation::{self, ElementalDevastation},
        elemental_focus::{self, ElementalFocus},
        elemental_mastery,
        flurry::{self, Flurry},
        improved_stormstrike::{self, ImprovedStormstrike},
        maelstrom_weapon::{self, MaelstromWeapon},
        rage_of_the_farseer::{self, RageOfTheFarseer},
    },
};
use crate::rotation::Totem;

/// What a Shaman spell does when its effects apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShamanSpell {
    LightningBolt,
    LightningBoltOverload,
    ChainLightning,
    ChainLightningOverload,
    FlameShock,
    FlameShockDot,
    LavaBurst,
    FireNova,
    SearingTotem,
    SearingTotemAttack,
    EarthShock,
    StrengthOfEarthTotem,
    StormstrikeCast,
    FrostShock,
    MagmaTotem,
    LightningShield,
    WaterShield,
    GraceOfAirTotem,
    ManaSpringTotem,
    /// The Restoration shaman's own Mana Tide Totem (Forever's rules).
    ManaTideTotem,
    WindfuryTotem,
    FlametongueTotem,
    FlametongueTotemAttack,
    /// The Flametongue Weapon hit of one hand, by its position in the effect.
    FlametongueHit(usize),
    FrostbrandHit,
    StormstrikeMainHand,
    StormstrikeOffHand,
    /// Windfury Weapon's special hit of the main hand or the off hand.
    WindfuryAttack {
        main_hand: bool,
    },
    RageOfTheFarseer,
    ElementalMastery,
    /// Rockbiter Weapon Proc (patch 181).
    RockbiterThreat,
    /// A heal of MythicSim's healer sims (`core::fight::heals`).
    Heal,
}

/// Class auras with Rust behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShamanAura {
    ElementalFocus,
    Clearcasting,
    ElementalDevastationTrigger,
    ElementalDevastation,
    ImprovedStormstrikeTrigger,
    ImprovedStormstrike,
    MaelstromWeaponTrigger,
    MaelstromWeapon,
    /// Stormstrike's debuff on the target.
    Stormstrike,
    FlurryTrigger,
    FlametongueTotem,
    FlametongueTotemTrigger,
    WindfuryImbue,
    /// The shaman's own Windfury Totem's aura, its dummy aura, its trigger and its charges.
    WindfuryTotem,
    WindfuryTotemDummy,
    WindfuryTotemTrigger,
    WindfuryTotemProc,
    /// One hand's Flametongue Weapon trigger, by its position in the effect.
    FlametongueTrigger(usize),
    FrostbrandTrigger,
    Flurry,
    ElementalMastery,
    RageOfTheFarseer,
    RockbiterWeapon,
    /// The listener that casts Rockbiter Weapon Proc (patch 181).
    RockbiterThreat,
    /// Water Shield's globes, which the shaman's critical spells spend.
    WaterShield,
}

/// Shaman state that Go keeps in the `Shaman` struct and its closures.
#[derive(Default)]
pub(crate) struct ShamanAgent {
    /// A Restoration shaman's heals.
    heals: Option<Rc<HealBook>>,
    /// The overloads of each Lightning Bolt and Chain Lightning rank, by spellbook position:
    /// Lightning Bolt has one, and Chain Lightning one for each target it can hit, in hit order.
    overloads: Vec<Vec<SpellId>>,
    lightning_bolt: Option<Overload>,
    chain_lightning: Option<ChainLightning>,
    /// Flame Shock's periodic half.
    flame_shock_dot: Option<SpellId>,
    lava_burst_bonus: f64,
    fire_nova_damage: f64,
    searing_totem: Option<SearingTotem>,
    elemental_focus: Option<ElementalFocus>,
    focus_trigger: elemental_focus::Trigger,
    totems: Expirations,
    searing_duration: i64,
    strength_of_earth: Option<StrengthOfEarth>,
    stormstrike: Option<Stormstrike>,
    devastation: Option<Rc<ElementalDevastation>>,
    improved_stormstrike: Option<ImprovedStormstrike>,
    maelstrom: Option<MaelstromWeapon>,
    maelstrom_chances: maelstrom_weapon::Chances,
    flurry: Option<Flurry>,
    /// Flurry's charge cooldown, a Go timer: when it is ready.
    flurry_icd: i64,
    /// Melee auto attacks, Go `ProcMaskMeleeWhiteHit`, by spellbook position.
    white: Vec<bool>,
    farseer: Option<RageOfTheFarseer>,
    elemental_mastery: Option<elemental_mastery::ElementalMastery>,
    /// Stormstrike's main hand strike, when the character has a main hand weapon.
    stormstrike_main_hand: Option<SpellId>,
    /// Go `HasMHWeapon() || HasOHWeapon()`, Stormstrike's cast condition.
    stormstrike_weapon: bool,
    /// Each Flametongue Weapon hand: its hit spell, whether it deals damage, its base, and the
    /// spells that trigger it by spellbook position.
    flametongue: Vec<(SpellId, bool, f64, Vec<bool>)>,
    /// Frostbrand Weapon's hit spell and base, and its trigger's chance by spellbook position.
    frostbrand: Option<(SpellId, f64)>,
    frostbrand_chances: Vec<Option<f64>>,
    /// Magma Totem's pulse base and lifetime.
    magma: Option<(f64, i64)>,
    /// Lightning Shield's aura and charges.
    lightning_shield: Option<(AuraRef, i32)>,
    water_shield: Option<WaterShield>,
    /// When Water Shield's next globe can go.
    water_shield_ready: i64,
    grace_of_air: Option<StrengthOfEarth>,
    mana_spring: Option<StrengthOfEarth>,
    /// The Restoration shaman's Mana Tide Totem (Forever's rules): its aura and the Healing
    /// Stream Totem spells it shares the water slot with.
    mana_tide: Option<ManaTide>,
    flametongue_totem: Option<totems::FlametongueTotem>,
    windfury_weapon: Option<WindfuryWeapon>,
    windfury_totem: Option<WindfuryTotem>,
    /// The Windfury Totem's pending periodic tick, which its expiry cancels.
    windfury_totem_tick: Option<Handle>,
    /// Enhancement's weapon sync: whether it delays or syncs the off hand, and Flurry's charge
    /// cooldown it waits out.
    weapon_sync: Option<(weapon_sync::SyncType, i64)>,
    /// The last air totem aura cast, which a new one replaces.
    air_totem: Option<AuraRef>,
    /// Rockbiter Weapon's gain and loss lines.
    rockbiter_logs: Option<(String, String)>,
    /// Rockbiter Weapon's stat aura bit, when an item swap switches it.
    rockbiter_stat_bit: Option<u32>,
    /// Spirit Weapons' threat on a Rockbiter aura that follows the weapon in the hand.
    rockbiter_threat_multiplier: Option<f64>,
    /// Rockbiter Weapon Proc and the hands that carry Rockbiter now (patch 181).
    rockbiter_threat: Option<RockbiterThreat>,
    /// Whether each spell's proc mask holds the main hand's or the off hand's melee bits.
    melee_main_hand: Vec<bool>,
    melee_off_hand: Vec<bool>,
    /// With an item swap, what each set's weapon gives the imbues and totems, Main then Swap1.
    weapon_values: Option<[WeaponValues; 2]>,
}

/// The values Flametongue Weapon, Flametongue Totem and Maelstrom Weapon read from the weapon in
/// the hand: each Flametongue hand's base damage, the totem's attack damage and Maelstrom's
/// chances by spell from its proc manager.
#[derive(Clone, Debug, Default, PartialEq)]
struct WeaponValues {
    flametongue: Vec<f64>,
    totem_damage: Option<f64>,
    maelstrom: maelstrom_weapon::Chances,
    /// The hands that carry Rockbiter and the main hand's swing speed (patch 181).
    rockbiter: Option<(bool, bool, f64)>,
    windfury: Option<WindfuryValues>,
    /// Frostbrand Weapon's chances by spell, from the proc manager of the weapon in the hand.
    frostbrand: Option<Vec<Option<f64>>>,
}

/// Windfury Weapon's heard spells, chances and whether it blocks the party totem.
type WindfuryValues = (Vec<usize>, Vec<(usize, f64)>, bool);

impl WeaponValues {
    fn of(effects: &[Effect], spells: usize) -> WeaponValues {
        let mut values = WeaponValues {
            maelstrom: vec![None; spells],
            ..WeaponValues::default()
        };
        for effect in effects {
            match effect {
                Effect::FlametongueWeapon { hands } => {
                    values.flametongue = hands.iter().map(|hand| hand.base_damage).collect();
                }
                Effect::FlametongueTotem { attack_damage, .. } => {
                    values.totem_damage = Some(*attack_damage);
                }
                Effect::MaelstromWeapon { chances, .. } => {
                    for chance in chances {
                        if let Some(slot) = values.maelstrom.get_mut(chance.spell) {
                            *slot = Some(chance.chance);
                        }
                    }
                }
                Effect::RockbiterThreat {
                    main_hand,
                    off_hand,
                    main_hand_swing_speed,
                    ..
                } => {
                    values.rockbiter = Some((*main_hand, *off_hand, *main_hand_swing_speed));
                }
                Effect::FrostbrandWeapon { chances, .. } => {
                    let mut by_spell = vec![None; spells];
                    for chance in chances {
                        if let Some(slot) = by_spell.get_mut(chance.spell) {
                            *slot = Some(chance.chance);
                        }
                    }
                    values.frostbrand = Some(by_spell);
                }
                Effect::WindfuryWeapon {
                    trigger_spells,
                    chances,
                    blocks_windfury_totem,
                    ..
                } => {
                    values.windfury = Some((
                        trigger_spells.clone(),
                        chances
                            .iter()
                            .map(|chance| (chance.spell, chance.chance))
                            .collect(),
                        *blocks_windfury_totem,
                    ));
                }
                _ => {}
            }
        }
        values
    }
}

/// Whether a prepared input carries the effect of a kind.
fn has_effect(prepared: &PreparedV2, kind: &str) -> bool {
    prepared.effects.iter().any(|effect| effect.kind() == kind)
}

impl ShamanAgent {
    /// A damage or heal result of the shaman's, which may spend a Water Shield globe.
    fn water_shield_result(fight: &mut Fight<Self>, spell: SpellId, result: &SpellResult) {
        let state = fight
            .agent
            .water_shield
            .clone()
            .expect("Water Shield is bound");
        let ready = fight.agent.water_shield_ready;
        if let Some(next) = state.on_result(fight, spell, result, ready) {
            fight.agent.water_shield_ready = next;
        }
    }

    /// The class behavior of an exported spell, if Rust implements it.
    fn spell(prepared: &PreparedV2, spell: &ExportedSpell) -> Option<ShamanSpell> {
        let has = |kind| has_effect(prepared, kind);
        let damage = spell.damage_effect.is_some();
        let id = spell.action_id.clone().unwrap_or_default();
        // Class spells without a class mask, which Go casts by action ID.
        let farseer = prepared.effects.iter().any(|effect| {
            matches!(effect, Effect::RageOfTheFarseer { spell_id, .. }
                if *spell_id == id.spell_id && id.tag == 0)
        });
        if spell.class_spell.is_none() && farseer {
            return Some(ShamanSpell::RageOfTheFarseer);
        }
        let position = prepared
            .player
            .spells
            .iter()
            .position(|exported| std::ptr::eq(exported, spell));
        for effect in &prepared.effects {
            match effect {
                // Go registers Rockbiter Weapon Proc without a class mask.
                Effect::RockbiterThreat { spell_id, .. }
                    if spell.class_spell.is_none() && *spell_id == id.spell_id && id.tag == 0 =>
                {
                    return Some(ShamanSpell::RockbiterThreat);
                }
                Effect::FlametongueTotem { attack_spell, .. }
                    if Some(*attack_spell) == position =>
                {
                    return Some(ShamanSpell::FlametongueTotemAttack);
                }
                // Go registers Lightning Shield's cast without a class mask.
                Effect::LightningShield { spell_id, .. }
                    if spell.class_spell.is_none() && *spell_id == id.spell_id && id.tag == 0 =>
                {
                    return Some(ShamanSpell::LightningShield);
                }
                Effect::WaterShield { spell_id, .. }
                    if spell.class_spell.is_none() && *spell_id == id.spell_id && id.tag == 0 =>
                {
                    return Some(ShamanSpell::WaterShield);
                }
                Effect::FlametongueWeapon { hands } => {
                    if let Some(hand) = hands.iter().position(|hand| Some(hand.spell) == position) {
                        return Some(ShamanSpell::FlametongueHit(hand));
                    }
                }
                Effect::FrostbrandWeapon { spell_id, .. }
                    if *spell_id == id.spell_id && id.tag == 0 =>
                {
                    return Some(ShamanSpell::FrostbrandHit);
                }
                Effect::WindfuryWeapon {
                    main_hand_attack,
                    off_hand_attack,
                    ..
                } if Some(*main_hand_attack) == position || Some(*off_hand_attack) == position => {
                    return Some(ShamanSpell::WindfuryAttack {
                        main_hand: Some(*main_hand_attack) == position,
                    });
                }
                _ => {}
            }
        }
        let searing_attack = prepared.effects.iter().any(|effect| {
            matches!(effect, Effect::SearingTotem { attack_spell_id, .. }
                if *attack_spell_id == id.spell_id && id.tag == 0)
        });
        match spell.class_spell.as_deref()? {
            "heal" => Some(ShamanSpell::Heal),
            "lightning_bolt" if damage && has("lightning_bolt") => Some(ShamanSpell::LightningBolt),
            "lightning_bolt_overload" if has("lightning_bolt") => {
                Some(ShamanSpell::LightningBoltOverload)
            }
            "chain_lightning" if damage && has("chain_lightning") => {
                Some(ShamanSpell::ChainLightning)
            }
            "chain_lightning_overload" if has("chain_lightning") => {
                Some(ShamanSpell::ChainLightningOverload)
            }
            "flame_shock_direct" if damage && has("flame_shock") => Some(ShamanSpell::FlameShock),
            "flame_shock_dot" if spell.dot.is_some() && has("flame_shock") => {
                Some(ShamanSpell::FlameShockDot)
            }
            "lava_burst" if damage && has("lava_burst") => Some(ShamanSpell::LavaBurst),
            "elemental_mastery" if has("elemental_mastery") => Some(ShamanSpell::ElementalMastery),
            "fire_nova" if has("fire_nova") => Some(ShamanSpell::FireNova),
            "searing_totem" if spell.dot.is_some() && has("searing_totem") => {
                Some(ShamanSpell::SearingTotem)
            }
            "searing_totem" if searing_attack => Some(ShamanSpell::SearingTotemAttack),
            "earth_shock" if damage && has("earth_shock") => Some(ShamanSpell::EarthShock),
            "frost_shock" if damage && has("frost_shock") => Some(ShamanSpell::FrostShock),
            "magma_totem" if spell.dot.is_some() && has("magma_totem") => {
                Some(ShamanSpell::MagmaTotem)
            }
            "flametongue_totem"
                if prepared.effects.iter().any(|effect| {
                    matches!(effect, Effect::FlametongueTotem { spell_id, .. }
                        if *spell_id == id.spell_id && id.tag == 0)
                }) =>
            {
                Some(ShamanSpell::FlametongueTotem)
            }
            "stormstrike_cast" if has("stormstrike") => Some(ShamanSpell::StormstrikeCast),
            "stormstrike_damage" if has("stormstrike") && id.tag == 1 => {
                Some(ShamanSpell::StormstrikeMainHand)
            }
            "stormstrike_damage" if has("stormstrike") && id.tag == 2 => {
                Some(ShamanSpell::StormstrikeOffHand)
            }
            "basic_totem"
                if prepared.effects.iter().any(|effect| {
                    matches!(effect, Effect::StrengthOfEarthTotem { spell_id, .. }
                        if *spell_id == id.spell_id && id.tag == 0)
                }) =>
            {
                Some(ShamanSpell::StrengthOfEarthTotem)
            }
            "basic_totem"
                if prepared.effects.iter().any(|effect| {
                    matches!(effect, Effect::GraceOfAirTotem { spell_id, .. }
                        if *spell_id == id.spell_id && id.tag == 0)
                }) =>
            {
                Some(ShamanSpell::GraceOfAirTotem)
            }
            "basic_totem"
                if prepared.effects.iter().any(|effect| {
                    matches!(effect, Effect::ManaSpringTotem { spell_id, .. }
                        if *spell_id == id.spell_id && id.tag == 0)
                }) =>
            {
                Some(ShamanSpell::ManaSpringTotem)
            }
            "basic_totem"
                if prepared.effects.iter().any(|effect| {
                    matches!(effect, Effect::ManaTideTotem { spell_id, .. }
                        if *spell_id == id.spell_id && id.tag == 0)
                }) =>
            {
                Some(ShamanSpell::ManaTideTotem)
            }
            "basic_totem"
                if prepared.effects.iter().any(|effect| {
                    matches!(effect, Effect::WindfuryTotemSelf { spell_id, .. }
                        if *spell_id == id.spell_id && id.tag == 0)
                }) =>
            {
                Some(ShamanSpell::WindfuryTotem)
            }
            _ => None,
        }
    }

    /// Build a fight for a prepared input that passed the coverage gate.
    pub(crate) fn fight(prepared: &PreparedV2) -> Result<Fight<ShamanAgent>, String> {
        let auras: Vec<(String, ShamanAura)> = prepared
            .effects
            .iter()
            .flat_map(|effect| match effect {
                Effect::ElementalFocus {
                    trigger_aura, aura, ..
                } => vec![
                    (trigger_aura.clone(), ShamanAura::ElementalFocus),
                    (aura.clone(), ShamanAura::Clearcasting),
                ],
                Effect::ElementalDevastation {
                    trigger_aura, aura, ..
                } => vec![
                    (
                        trigger_aura.clone(),
                        ShamanAura::ElementalDevastationTrigger,
                    ),
                    (aura.clone(), ShamanAura::ElementalDevastation),
                ],
                Effect::ImprovedStormstrike {
                    trigger_aura, aura, ..
                } => vec![
                    (trigger_aura.clone(), ShamanAura::ImprovedStormstrikeTrigger),
                    (aura.clone(), ShamanAura::ImprovedStormstrike),
                ],
                Effect::MaelstromWeapon {
                    trigger_aura, aura, ..
                } => vec![
                    (trigger_aura.clone(), ShamanAura::MaelstromWeaponTrigger),
                    (aura.clone(), ShamanAura::MaelstromWeapon),
                ],
                Effect::Flurry {
                    trigger_aura, aura, ..
                } => vec![
                    (trigger_aura.clone(), ShamanAura::FlurryTrigger),
                    (aura.clone(), ShamanAura::Flurry),
                ],
                Effect::RageOfTheFarseer { aura, .. } => {
                    vec![(aura.clone(), ShamanAura::RageOfTheFarseer)]
                }
                Effect::ElementalMastery { aura, .. } => {
                    vec![(aura.clone(), ShamanAura::ElementalMastery)]
                }
                Effect::RockbiterWeapon { aura, .. } => {
                    vec![(aura.clone(), ShamanAura::RockbiterWeapon)]
                }
                Effect::RockbiterThreat { aura, .. } => {
                    vec![(aura.clone(), ShamanAura::RockbiterThreat)]
                }
                Effect::WaterShield { aura, .. } => {
                    vec![(aura.clone(), ShamanAura::WaterShield)]
                }
                Effect::FlametongueWeapon { hands } => hands
                    .iter()
                    .enumerate()
                    .map(|(hand, state)| {
                        (
                            state.trigger_aura.clone(),
                            ShamanAura::FlametongueTrigger(hand),
                        )
                    })
                    .collect(),
                Effect::FrostbrandWeapon { trigger_aura, .. } => {
                    vec![(trigger_aura.clone(), ShamanAura::FrostbrandTrigger)]
                }
                Effect::FlametongueTotem {
                    aura, trigger_aura, ..
                } => vec![
                    (aura.clone(), ShamanAura::FlametongueTotem),
                    (trigger_aura.clone(), ShamanAura::FlametongueTotemTrigger),
                ],
                Effect::WindfuryWeapon { trigger_aura, .. } => {
                    vec![(trigger_aura.clone(), ShamanAura::WindfuryImbue)]
                }
                Effect::WindfuryTotemSelf {
                    totem_aura,
                    dummy_aura,
                    trigger_aura,
                    proc_aura,
                    ..
                } => vec![
                    (totem_aura.clone(), ShamanAura::WindfuryTotem),
                    (dummy_aura.clone(), ShamanAura::WindfuryTotemDummy),
                    (trigger_aura.clone(), ShamanAura::WindfuryTotemTrigger),
                    (proc_aura.clone(), ShamanAura::WindfuryTotemProc),
                ],
                _ => Vec::new(),
            })
            .collect();
        let target_auras: Vec<(String, ShamanAura)> = prepared
            .effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Stormstrike { aura, .. } => Some((aura.clone(), ShamanAura::Stormstrike)),
                _ => None,
            })
            .collect();
        let spell = |exported: &ExportedSpell| ShamanAgent::spell(prepared, exported);
        let mut fight = Fight::new(prepared, ShamanAgent::default(), spell, |unit, label| {
            let auras = match unit {
                "player" => &auras,
                _ => &target_auras,
            };
            auras
                .iter()
                .find(|(name, _)| name == label)
                .map(|(_, kind)| *kind)
        })?;
        let find = |fight: &Fight<ShamanAgent>, spell_id: i32, tag: i32| {
            fight
                .spells
                .iter()
                .position(|spell| spell.id.spell_id == spell_id && spell.id.tag == tag)
        };
        fight.agent.overloads = vec![Vec::new(); fight.spells.len()];
        fight.agent.maelstrom_chances = vec![None; fight.spells.len()];
        fight.agent.frostbrand_chances = vec![None; fight.spells.len()];
        fight.agent.white = prepared
            .player
            .spells
            .iter()
            .map(|spell| {
                spell
                    .proc_mask
                    .iter()
                    .any(|mask| mask == "ProcMaskMeleeMHAuto" || mask == "ProcMaskMeleeOHAuto")
            })
            .collect();
        let holds = |spell: &ExportedSpell, names: [&str; 2]| {
            spell
                .proc_mask
                .iter()
                .any(|mask| names.contains(&mask.as_str()))
        };
        fight.agent.melee_main_hand = prepared
            .player
            .spells
            .iter()
            .map(|spell| holds(spell, ["ProcMaskMeleeMHAuto", "ProcMaskMeleeMHSpecial"]))
            .collect();
        fight.agent.melee_off_hand = prepared
            .player
            .spells
            .iter()
            .map(|spell| holds(spell, ["ProcMaskMeleeOHAuto", "ProcMaskMeleeOHSpecial"]))
            .collect();
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
                    fight.agent.heals = Some(Rc::new(book));
                }
                Effect::LightningBolt {
                    overload_chance,
                    overload_tag,
                    rng_label,
                }
                | Effect::ChainLightning {
                    overload_chance,
                    overload_tag,
                    rng_label,
                    ..
                } => {
                    let (cast, overload) = match effect {
                        Effect::LightningBolt { .. } => (
                            ShamanSpell::LightningBolt,
                            ShamanSpell::LightningBoltOverload,
                        ),
                        _ => (
                            ShamanSpell::ChainLightning,
                            ShamanSpell::ChainLightningOverload,
                        ),
                    };
                    Self::bind_overloads(&mut fight, cast, overload, *overload_tag)?;
                    match effect {
                        Effect::ChainLightning {
                            bounce_reduction,
                            bounce_bonus,
                            ..
                        } => {
                            fight.agent.chain_lightning = Some(ChainLightning {
                                overload_chance: *overload_chance,
                                label: rng_label.clone(),
                                bounce_reduction: *bounce_reduction,
                                bounce_bonus: *bounce_bonus,
                            });
                        }
                        _ => {
                            fight.agent.lightning_bolt = Some(Overload {
                                chance: *overload_chance,
                                label: rng_label.clone(),
                            });
                        }
                    }
                }
                Effect::FlameShock {
                    spell_id,
                    tick_base,
                    tick_can_crit,
                } => {
                    let dot_spell = fight.spells.iter().position(|spell| {
                        spell.id.spell_id == *spell_id
                            && matches!(
                                spell.behavior,
                                crate::core::fight::SpellBehavior::Class(
                                    ShamanSpell::FlameShockDot
                                )
                            )
                    });
                    if let Some(dot_spell) = dot_spell {
                        let dot = fight.spells[dot_spell]
                            .dot
                            .expect("the periodic half has a dot");
                        fight.dots[dot].tick_base = Some(*tick_base);
                        fight.dots[dot].tick_can_crit = *tick_can_crit;
                        fight.agent.flame_shock_dot = Some(dot_spell);
                    }
                }
                Effect::LavaBurst {
                    flame_shock_bonus, ..
                } => fight.agent.lava_burst_bonus = *flame_shock_bonus,
                Effect::FireNova { base_damage, .. } => fight.agent.fire_nova_damage = *base_damage,
                Effect::SearingTotem {
                    attack_spell_id,
                    attack_damage,
                    magma_totem_aura,
                    flametongue_totem_aura,
                    duration_ns,
                    ..
                } => {
                    fight.agent.searing_duration = *duration_ns;
                    if let Some(attack) = find(&fight, *attack_spell_id, 0) {
                        let magma_totem = fight.player_aura(magma_totem_aura).ok();
                        let flametongue_totem = fight.player_aura(flametongue_totem_aura).ok();
                        fight.agent.searing_totem = Some(SearingTotem {
                            attack,
                            attack_damage: *attack_damage,
                            magma_totem,
                            flametongue_totem,
                        });
                    }
                }
                Effect::ElementalFocus {
                    trigger_aura,
                    aura,
                    proc_chance,
                    cost_percent_add,
                    max_stacks,
                } => {
                    let bound = elemental_focus::bind(
                        &mut fight,
                        trigger_aura,
                        aura,
                        *proc_chance,
                        *cost_percent_add,
                        *max_stacks,
                    )?;
                    fight.agent.elemental_focus = Some(bound);
                }
                Effect::StrengthOfEarthTotem {
                    aura, duration_ns, ..
                } => {
                    fight.agent.strength_of_earth = Some(StrengthOfEarth {
                        aura: fight.player_aura(aura)?,
                        duration: *duration_ns,
                    });
                }
                Effect::Stormstrike {
                    spell_id,
                    aura,
                    damage_multiplier,
                    has_main_hand,
                    has_off_hand,
                } => {
                    fight.agent.stormstrike_weapon = *has_main_hand || *has_off_hand;
                    fight.agent.stormstrike =
                        Some(stormstrike::bind(&fight, aura, *damage_multiplier)?);
                    fight.agent.stormstrike_main_hand =
                        has_main_hand.then(|| find(&fight, *spell_id, 1)).flatten();
                }
                Effect::Flurry {
                    trigger_aura,
                    aura,
                    melee_speed_multiplier,
                    charge_icd_ns,
                    can_proc_from_procs,
                    ..
                } => {
                    fight.agent.flurry = Some(flurry::bind(
                        &fight,
                        trigger_aura,
                        aura,
                        *melee_speed_multiplier,
                        *charge_icd_ns,
                        *can_proc_from_procs,
                    )?);
                }
                Effect::ElementalMastery {
                    aura,
                    cast_time_delta_ns,
                    chain_lightning_cooldown_ns,
                    ..
                } => {
                    fight.agent.elemental_mastery = Some(elemental_mastery::bind(
                        &mut fight,
                        aura,
                        *cast_time_delta_ns,
                        *chain_lightning_cooldown_ns,
                    )?);
                }
                Effect::RageOfTheFarseer {
                    aura,
                    melee_speed_multiplier,
                    ..
                } => {
                    fight.agent.farseer = Some(rage_of_the_farseer::bind(
                        &fight,
                        aura,
                        *melee_speed_multiplier,
                    )?);
                }
                Effect::RockbiterWeapon {
                    aura,
                    gain_log,
                    expire_log,
                    threat_multiplier,
                } => {
                    fight.agent.rockbiter_logs = Some((gain_log.clone(), expire_log.clone()));
                    fight.agent.rockbiter_threat_multiplier = *threat_multiplier;
                    // With an item swap that switches it, a stat aura whose bit carries its attack
                    // power.
                    fight.agent.rockbiter_stat_bit =
                        Fight::<ShamanAgent>::stat_aura_bit(&prepared.effects, aura);
                }
                Effect::RockbiterThreat {
                    spell_id,
                    main_hand,
                    off_hand,
                    threat_per_swing_second,
                    main_hand_swing_speed,
                    ..
                } => {
                    let spell = find(&fight, *spell_id, 0).ok_or_else(|| {
                        format!("Rockbiter Weapon Proc {spell_id} is not registered")
                    })?;
                    fight.agent.rockbiter_threat = Some(RockbiterThreat {
                        spell,
                        main_hand: *main_hand,
                        off_hand: *off_hand,
                        threat_per_swing_second: *threat_per_swing_second,
                        main_hand_swing_speed: *main_hand_swing_speed,
                    });
                }
                Effect::FlametongueWeapon { hands } => {
                    for hand in hands {
                        let mut triggers = vec![false; fight.spells.len()];
                        for &spell in &hand.trigger_spells {
                            *triggers.get_mut(spell).ok_or_else(|| {
                                format!("Flametongue Weapon names spell {spell}")
                            })? = true;
                        }
                        if hand.spell >= fight.spells.len() {
                            return Err(format!("Flametongue Weapon names spell {}", hand.spell));
                        }
                        fight.agent.flametongue.push((
                            hand.spell,
                            hand.deals_damage,
                            hand.base_damage,
                            triggers,
                        ));
                    }
                }
                Effect::MagmaTotem {
                    pulse_damage,
                    duration_ns,
                    ..
                } => fight.agent.magma = Some((*pulse_damage, *duration_ns)),
                Effect::LightningShield { aura, charges, .. } => {
                    fight.agent.lightning_shield = Some((fight.player_aura(aura)?, *charges));
                }
                Effect::WaterShield {
                    aura,
                    globes,
                    icd_ns,
                    mana_percent,
                    mana_spell_id,
                    trigger_spells,
                    ..
                } => {
                    let state = WaterShield::bind(
                        &mut fight,
                        aura,
                        *globes,
                        *icd_ns,
                        *mana_percent,
                        *mana_spell_id,
                        trigger_spells,
                    )?;
                    fight.agent.water_shield = Some(state);
                }
                Effect::GraceOfAirTotem {
                    aura, duration_ns, ..
                } => {
                    fight.agent.grace_of_air = Some(StrengthOfEarth {
                        aura: fight.player_aura(aura)?,
                        duration: *duration_ns,
                    });
                }
                Effect::WindfuryWeapon {
                    trigger_aura,
                    trigger_spells,
                    chances,
                    main_hand_spells,
                    main_hand_attack,
                    off_hand_attack,
                    attack_power,
                    blocks_windfury_totem,
                } => {
                    fight.agent.windfury_weapon = Some(windfury_weapon::bind(
                        &fight,
                        trigger_aura,
                        (*main_hand_attack, *off_hand_attack),
                        *attack_power,
                        trigger_spells,
                        main_hand_spells,
                        chances,
                        *blocks_windfury_totem,
                    )?);
                }
                Effect::WindfuryTotemSelf { .. } => {
                    fight.agent.windfury_totem = Some(windfury_totem::bind(&fight, effect)?);
                }
                Effect::WeaponSync {
                    sync,
                    flurry_icd_ns,
                } => {
                    fight.agent.weapon_sync =
                        Some((weapon_sync::SyncType::parse(sync)?, *flurry_icd_ns));
                }
                Effect::ManaTideTotem {
                    aura,
                    duration_ns,
                    healing_stream_totems,
                    ..
                } => {
                    let streams = fight
                        .spells
                        .iter()
                        .enumerate()
                        .filter(|(_, spell)| {
                            spell.id.tag == 0 && healing_stream_totems.contains(&spell.id.spell_id)
                        })
                        .map(|(position, _)| position)
                        .collect();
                    fight.agent.mana_tide = Some(ManaTide {
                        totem: StrengthOfEarth {
                            aura: fight.player_aura(aura)?,
                            duration: *duration_ns,
                        },
                        streams,
                    });
                }
                Effect::ManaSpringTotem {
                    aura, duration_ns, ..
                } => {
                    fight.agent.mana_spring = Some(StrengthOfEarth {
                        aura: fight.player_aura(aura)?,
                        duration: *duration_ns,
                    });
                }
                Effect::FlametongueTotem {
                    aura,
                    trigger_aura,
                    attack_spell,
                    attack_deals_damage,
                    attack_damage,
                    trigger_spells,
                    disabled_by_weapon,
                    duration_ns,
                    ..
                } => {
                    let mut triggers = vec![false; fight.spells.len()];
                    for &spell in trigger_spells {
                        *triggers
                            .get_mut(spell)
                            .ok_or_else(|| format!("Flametongue Totem names spell {spell}"))? =
                            true;
                    }
                    fight.agent.flametongue_totem = Some(totems::FlametongueTotem {
                        aura: fight.player_aura(aura)?,
                        trigger: fight.player_aura(trigger_aura)?,
                        attack: *attack_spell,
                        attack_deals_damage: *attack_deals_damage,
                        attack_damage: *attack_damage,
                        triggers,
                        enabled: !*disabled_by_weapon,
                        duration: *duration_ns,
                    });
                }
                Effect::FrostbrandWeapon {
                    spell_id,
                    base_damage,
                    chances,
                    ..
                } => {
                    let spell = find(&fight, *spell_id, 0).ok_or_else(|| {
                        format!("Frostbrand Weapon spell {spell_id} is not registered")
                    })?;
                    fight.agent.frostbrand = Some((spell, *base_damage));
                    for chance in chances {
                        *fight
                            .agent
                            .frostbrand_chances
                            .get_mut(chance.spell)
                            .ok_or_else(|| {
                                format!("Frostbrand Weapon names spell {}", chance.spell)
                            })? = Some(chance.chance);
                    }
                }
                Effect::ElementalDevastation {
                    trigger_aura,
                    aura,
                    melee_crit,
                    can_proc_from_procs,
                    excluded_class_spells,
                } => {
                    let melee = (0..fight.spells.len())
                        .filter(|&spell| {
                            let state = &fight.spells[spell];
                            state.melee_proc && !state.flags.no_spell_mods
                        })
                        .collect();
                    fight.agent.devastation = Some(Rc::new(elemental_devastation::bind(
                        &mut fight,
                        trigger_aura,
                        aura,
                        *melee_crit,
                        melee,
                        *can_proc_from_procs,
                        excluded_class_spells,
                    )?));
                }
                Effect::ImprovedStormstrike {
                    trigger_aura,
                    aura,
                    proc_chance,
                    spirit_regen_rate_casting,
                    ..
                } => {
                    fight.agent.improved_stormstrike = Some(improved_stormstrike::bind(
                        &mut fight,
                        trigger_aura,
                        aura,
                        *proc_chance,
                        *spirit_regen_rate_casting,
                    )?);
                }
                Effect::MaelstromWeapon {
                    trigger_aura,
                    aura,
                    per_stack,
                    chances,
                    ..
                } => {
                    for chance in chances {
                        let slot = fight
                            .agent
                            .maelstrom_chances
                            .get_mut(chance.spell)
                            .ok_or_else(|| {
                                format!("Maelstrom Weapon names spell {}", chance.spell)
                            })?;
                        *slot = Some(chance.chance);
                    }
                    let bolts = fight.spells_with_class(&["lightning_bolt"]);
                    fight.agent.maelstrom = Some(maelstrom_weapon::bind(
                        &mut fight,
                        trigger_aura,
                        aura,
                        *per_stack,
                        bolts,
                    )?);
                }
                _ => {}
            }
        }
        if let Some(worn) = fight.item_swap_worn_effects() {
            let spells = fight.spells.len();
            fight.agent.weapon_values = Some([
                WeaponValues::of(&prepared.effects, spells),
                WeaponValues::of(&worn, spells),
            ]);
        }
        Ok(fight)
    }

    /// Pair each rank with its overload, which rolls a client damage row of its own.
    fn bind_overloads(
        fight: &mut Fight<ShamanAgent>,
        cast: ShamanSpell,
        overload: ShamanSpell,
        tag: i32,
    ) -> Result<(), String> {
        let behavior =
            |fight: &Fight<ShamanAgent>, spell: SpellId| match fight.spells[spell].behavior {
                crate::core::fight::SpellBehavior::Class(kind) => Some(kind),
                _ => None,
            };
        for spell in 0..fight.spells.len() {
            if behavior(fight, spell) != Some(cast) {
                continue;
            }
            let id = fight.spells[spell].id.spell_id;
            // Go registers an overload for each target the spell can hit, all under one id.
            let copies: Vec<SpellId> = (0..fight.spells.len())
                .filter(|&copy| {
                    fight.spells[copy].id.spell_id == id
                        && fight.spells[copy].id.tag == tag
                        && behavior(fight, copy) == Some(overload)
                })
                .collect();
            if copies.is_empty() {
                return Err(format!("spell {id} has no overload"));
            }
            fight.agent.overloads[spell] = copies;
        }
        Ok(())
    }

    /// Go `cancelFireTotems`: Magma Totem's dot, Searing Totem's dot, then Flametongue Totem's
    /// aura. Totem of Wrath is never set.
    fn cancel_fire_totems(fight: &mut Fight<Self>) {
        let magma = fight.spells.iter().position(|spell| {
            matches!(
                spell.behavior,
                crate::core::fight::SpellBehavior::Class(ShamanSpell::MagmaTotem)
            )
        });
        if let Some(dot) = magma.and_then(|spell| fight.spells[spell].dot) {
            let aura = fight.dots[dot].aura;
            fight.deactivate_aura(aura);
        }
        let searing = fight.spells.iter().position(|spell| {
            matches!(
                spell.behavior,
                crate::core::fight::SpellBehavior::Class(ShamanSpell::SearingTotem)
            )
        });
        if let Some(dot) = searing.and_then(|spell| fight.spells[spell].dot) {
            let aura = fight.dots[dot].aura;
            fight.deactivate_aura(aura);
        }
        if let Some(aura) = fight
            .agent
            .flametongue_totem
            .as_ref()
            .map(|state| state.aura)
        {
            fight.deactivate_aura(aura);
        }
    }

    fn focus(fight: &Fight<Self>) -> ElementalFocus {
        fight
            .agent
            .elemental_focus
            .expect("Elemental Focus is bound")
    }

    fn searing_totem(fight: &Fight<Self>) -> SearingTotem {
        fight
            .agent
            .searing_totem
            .clone()
            .expect("Searing Totem is bound")
    }

    fn windfury_totem(fight: &Fight<Self>) -> WindfuryTotem {
        fight
            .agent
            .windfury_totem
            .clone()
            .expect("Windfury Totem is bound")
    }
}

impl Agent for ShamanAgent {
    type Spell = ShamanSpell;
    type Aura = ShamanAura;

    /// weapon_imbues.go and buffs/flametongue_totem.go read the speed of the weapon in the hand,
    /// and Maelstrom Weapon's proc manager is rebuilt for it.
    fn on_item_swap(fight: &mut Fight<Self>, worn: bool) {
        let Some(values) = fight.agent.weapon_values.clone() else {
            return;
        };
        let set = &values[usize::from(worn)];
        for (hand, damage) in fight.agent.flametongue.iter_mut().zip(&set.flametongue) {
            hand.2 = *damage;
        }
        if let (Some(totem), Some(damage)) =
            (fight.agent.flametongue_totem.as_mut(), set.totem_damage)
        {
            totem.attack_damage = damage;
        }
        fight.agent.maelstrom_chances = set.maelstrom.clone();
        if let Some(chances) = &set.frostbrand {
            fight.agent.frostbrand_chances = chances.clone();
        }
        if let (Some(state), Some((main_hand, off_hand, speed))) =
            (fight.agent.rockbiter_threat.as_mut(), set.rockbiter)
        {
            state.main_hand = main_hand;
            state.off_hand = off_hand;
            state.main_hand_swing_speed = speed;
        }
        // RegisterWindfuryImbue rebuilds its proc manager for the weapon in the hand, and a
        // main hand imbue outbids the party totem.
        if let (Some(state), Some((triggers, chances, blocks))) =
            (fight.agent.windfury_weapon.as_mut(), &set.windfury)
        {
            let chances: Vec<crate::contracts::prepared_v2::SpellChance> = chances
                .iter()
                .map(
                    |&(spell, chance)| crate::contracts::prepared_v2::SpellChance { spell, chance },
                )
                .collect();
            state
                .follow_weapon(triggers, &chances, *blocks)
                .expect("the swap set's Windfury Weapon names known spells");
            // The swap's callbacks put the imbue's trigger up before the values follow the new
            // weapon: a trigger already up that now blocks the totem takes it down, as its gain
            // does in Go once the main hand carries the imbue.
            let state = state.clone();
            if fight.aura(state.trigger).active {
                state.on_trigger_gain(fight);
            }
        }
    }

    fn apply_effects(fight: &mut Fight<Self>, spell: SpellId, target: Side, behavior: ShamanSpell) {
        match behavior {
            ShamanSpell::Heal => {
                let book = fight.agent.heals.clone().expect("the heals are bound");
                let rank = book
                    .rank_of(spell)
                    .expect("a heal is one of the bound ranks");
                // A Healing Stream Totem takes the water slot back from a Mana Tide Totem.
                if let Some(tide) = fight.agent.mana_tide.clone() {
                    if tide.streams.contains(&spell) {
                        fight.deactivate_aura(tide.totem.aura);
                    }
                }
                book.cast(fight, rank);
            }
            ShamanSpell::LightningBolt | ShamanSpell::LightningBoltOverload => {
                lightning_bolt::apply(fight, spell, target)
            }
            ShamanSpell::ChainLightning | ShamanSpell::ChainLightningOverload => {
                let state = fight
                    .agent
                    .chain_lightning
                    .clone()
                    .expect("Chain Lightning is bound");
                let overloads = (behavior == ShamanSpell::ChainLightning)
                    .then(|| fight.agent.overloads[spell].clone());
                chain_lightning::apply(fight, spell, target, &state, overloads.as_deref());
            }
            ShamanSpell::FlameShock => {
                let dot_spell = fight.agent.flame_shock_dot.expect("Flame Shock is bound");
                flame_shock::apply(fight, spell, target, dot_spell);
            }
            ShamanSpell::FlameShockDot => flame_shock::apply_dot(fight, spell, target),
            ShamanSpell::LavaBurst => {
                let dot = fight
                    .agent
                    .flame_shock_dot
                    .and_then(|dot_spell| fight.spells[dot_spell].dot);
                let bonus = fight.agent.lava_burst_bonus;
                lava_burst::apply(fight, spell, target, dot, bonus);
            }
            ShamanSpell::FireNova => {
                let base = fight.agent.fire_nova_damage;
                fire_nova::apply(fight, spell, base);
            }
            ShamanSpell::SearingTotem => {
                let state = Self::searing_totem(fight);
                searing_totem::apply(fight, spell, &state);
                let expires = fight.now + fight.agent.searing_duration;
                fight.agent.totems.set(Totem::Fire, expires);
            }
            ShamanSpell::EarthShock | ShamanSpell::FrostShock => {
                earth_shock::apply(fight, spell, target)
            }
            ShamanSpell::MagmaTotem => {
                Self::cancel_fire_totems(fight);
                let dot = fight.spells[spell].dot.expect("Magma Totem has a dot");
                fight.apply_dot(dot);
                let (_, duration) = fight.agent.magma.expect("Magma Totem is bound");
                let expires = fight.now + duration;
                fight.agent.totems.set(Totem::Fire, expires);
            }
            ShamanSpell::FlametongueTotem => {
                Self::cancel_fire_totems(fight);
                let state = fight
                    .agent
                    .flametongue_totem
                    .clone()
                    .expect("Flametongue Totem is bound");
                let expires = fight.now + state.duration;
                fight.agent.totems.set(Totem::Fire, expires);
                fight.activate_aura(state.aura);
            }
            ShamanSpell::FlametongueTotemAttack => {
                let state = fight
                    .agent
                    .flametongue_totem
                    .clone()
                    .expect("Flametongue Totem is bound");
                if state.attack_deals_damage {
                    weapon_imbues::hit(fight, spell, target, state.attack_damage);
                }
            }
            ShamanSpell::LightningShield => {
                let (aura, charges) = fight
                    .agent
                    .lightning_shield
                    .expect("Lightning Shield is bound");
                // Go deactivateShields on its own shield. Client 70532 dropped "Only one
                // Elemental Shield can be active" from Water Shield, so the two stand together.
                fight.deactivate_aura(aura);
                fight.activate_aura(aura);
                fight.set_stacks(aura, charges);
            }
            ShamanSpell::WaterShield => {
                let state = fight
                    .agent
                    .water_shield
                    .clone()
                    .expect("Water Shield is bound");
                state.cast(fight);
            }
            ShamanSpell::GraceOfAirTotem => {
                let totem = fight
                    .agent
                    .grace_of_air
                    .expect("Grace of Air Totem is bound");
                if let Some(previous) = fight.agent.air_totem {
                    fight.deactivate_aura(previous);
                }
                fight.agent.air_totem = Some(totem.aura);
                let expires = totems::strength_of_earth(fight, totem);
                fight.agent.totems.set(Totem::Air, expires);
            }
            ShamanSpell::WindfuryTotem => {
                let totem = Self::windfury_totem(fight);
                if let Some(previous) = fight.agent.air_totem {
                    fight.deactivate_aura(previous);
                }
                fight
                    .agent
                    .totems
                    .set(Totem::Air, fight.now + totem.duration);
                fight.agent.air_totem = Some(totem.totem);
                fight.activate_aura(totem.totem);
            }
            ShamanSpell::ManaSpringTotem => {
                // The only water totem in scope is this one, so the previous aura is its own.
                let totem = fight.agent.mana_spring.expect("Mana Spring Totem is bound");
                let expires = totems::strength_of_earth(fight, totem);
                fight.agent.totems.set(Totem::Water, expires);
            }
            ShamanSpell::ManaTideTotem => {
                // The water slot's Healing Stream Totem ends, and its heals over time with it.
                let tide = fight
                    .agent
                    .mana_tide
                    .clone()
                    .expect("Mana Tide Totem is bound");
                let book = fight.agent.heals.clone().expect("the heals are bound");
                for heal in &book.heals {
                    if let (true, Some(dot)) = (tide.streams.contains(&heal.spell), heal.dot) {
                        let aura = fight.dots[dot].aura;
                        fight.deactivate_aura(aura);
                    }
                }
                let expires = totems::strength_of_earth(fight, tide.totem);
                fight.agent.totems.set(Totem::Water, expires);
            }
            ShamanSpell::FlametongueHit(hand) => {
                let (_, deals_damage, base, _) = fight.agent.flametongue[hand];
                if deals_damage {
                    weapon_imbues::hit(fight, spell, target, base);
                }
            }
            ShamanSpell::FrostbrandHit => {
                let (_, base) = fight.agent.frostbrand.expect("Frostbrand Weapon is bound");
                weapon_imbues::hit(fight, spell, target, base);
            }
            ShamanSpell::StormstrikeCast => {
                let state = fight.agent.stormstrike.expect("Stormstrike is bound");
                let main_hand = fight.agent.stormstrike_main_hand;
                state.cast(fight, spell, target, main_hand);
            }
            ShamanSpell::StormstrikeMainHand => stormstrike::strike(fight, spell, target, true),
            ShamanSpell::StormstrikeOffHand => stormstrike::strike(fight, spell, target, false),
            ShamanSpell::WindfuryAttack { main_hand } => {
                let state = fight
                    .agent
                    .windfury_weapon
                    .clone()
                    .expect("Windfury Weapon is bound");
                state.strike(fight, spell, target, main_hand);
            }
            ShamanSpell::ElementalMastery => {
                let aura = fight
                    .agent
                    .elemental_mastery
                    .expect("Elemental Mastery is bound")
                    .aura;
                fight.activate_aura(aura);
            }
            ShamanSpell::RageOfTheFarseer => {
                let aura = fight
                    .agent
                    .farseer
                    .expect("Rage of the Farseer is bound")
                    .aura;
                fight.activate_aura(aura);
            }
            ShamanSpell::StrengthOfEarthTotem => {
                let totem = fight
                    .agent
                    .strength_of_earth
                    .expect("Strength of Earth Totem is bound");
                let expires = totems::strength_of_earth(fight, totem);
                fight.agent.totems.set(Totem::Earth, expires);
            }
            ShamanSpell::SearingTotemAttack => {
                let base = Self::searing_totem(fight).attack_damage;
                searing_totem::attack(fight, spell, target, base);
            }
            ShamanSpell::RockbiterThreat => {
                let state = fight
                    .agent
                    .rockbiter_threat
                    .expect("Rockbiter Weapon Proc is bound");
                state.apply(fight, spell, target);
            }
        }
    }

    fn reset(fight: &mut Fight<Self>) {
        fight.agent.totems = Expirations::default();
        fight.agent.flurry_icd = crate::core::time::STARTING_CD_TIME;
        fight.agent.water_shield_ready = crate::core::time::STARTING_CD_TIME;
    }

    /// Stormstrike needs a weapon.
    fn extra_cast_condition(fight: &Fight<Self>, _spell: SpellId, behavior: ShamanSpell) -> bool {
        match behavior {
            ShamanSpell::StormstrikeCast => fight.agent.stormstrike_weapon,
            _ => true,
        }
    }

    /// Go `ApplySyncType`'s replacement: it may move the off hand swing, then returns the swing.
    fn replace_mh_swing(fight: &mut Fight<Self>, swing: SpellId) -> SpellId {
        if let Some((sync, icd)) = fight.agent.weapon_sync {
            weapon_sync::apply(fight, sync, icd);
        }
        swing
    }

    /// Go `AttachPeriodicAction`'s tick on the Windfury Totem's aura: its effect, then the next.
    fn on_class_action(fight: &mut Fight<Self>, _tag: u32) {
        let totem = Self::windfury_totem(fight);
        totem.tick(fight);
        let next = fight.now + totem.period;
        fight.agent.windfury_totem_tick = Some(fight.schedule_class_action(next, PRIORITY_AUTO, 0));
    }

    /// The Windfury Totem's expiry cancels its pending tick, even one already popped.
    fn class_action_valid(fight: &Fight<Self>, _tag: u32, handle: Handle) -> bool {
        fight.agent.windfury_totem_tick == Some(handle)
    }

    fn totem_expiration(fight: &Fight<Self>, totem: Totem) -> i64 {
        fight.agent.totems.get(totem)
    }

    /// Go `holdMeleeForCast`: a hardcast holds the melee swing until it completes.
    fn modify_cast(fight: &mut Fight<Self>, spell: SpellId, _behavior: ShamanSpell) {
        if !is_class(fight.spells[spell].class_spell.as_deref(), HOLDS_MELEE) {
            return;
        }
        let cast_time = fight.spells[spell].cur_cast.cast_time;
        let cast_time = fight.apply_cast_speed_for_spell(cast_time, spell);
        if cast_time > 0 {
            fight.hold_melee_for_cast(fight.now + cast_time);
        }
    }

    fn caster_damage_multiplier(fight: &Fight<Self>, spell: SpellId, target: Side) -> Option<f64> {
        fight
            .agent
            .stormstrike
            .and_then(|state| state.caster_multiplier(fight, spell, target))
    }

    fn on_spell_hit_dealt(
        fight: &mut Fight<Self>,
        aura: AuraRef,
        kind: ShamanAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        match kind {
            ShamanAura::ElementalDevastationTrigger => {
                let state = fight
                    .agent
                    .devastation
                    .clone()
                    .expect("Elemental Devastation is bound");
                state.on_spell_hit_dealt(fight, spell, result);
            }
            ShamanAura::FlurryTrigger => {
                let state = fight.agent.flurry.expect("Flurry is bound");
                state.on_spell_hit_dealt(fight, spell, result);
            }
            ShamanAura::WindfuryImbue => {
                let state = fight
                    .agent
                    .windfury_weapon
                    .clone()
                    .expect("Windfury Weapon is bound");
                state.on_trigger_hit(fight, spell, result);
            }
            ShamanAura::WindfuryTotemTrigger => {
                Self::windfury_totem(fight).on_trigger_hit(fight, spell, result);
            }
            ShamanAura::WindfuryTotemProc => {
                Self::windfury_totem(fight).on_spend_hit(fight, spell, result);
            }
            ShamanAura::FlametongueTotemTrigger => {
                let state = fight
                    .agent
                    .flametongue_totem
                    .clone()
                    .expect("Flametongue Totem is bound");
                if state.triggers[spell] && result.landed() {
                    fight.cast(state.attack, result.target);
                }
            }
            ShamanAura::FlametongueTrigger(hand) => {
                let (hit, _, _, ref triggers) = fight.agent.flametongue[hand];
                if triggers[spell] && result.landed() {
                    fight.cast(hit, result.target);
                }
            }
            ShamanAura::FrostbrandTrigger => {
                let (hit, _) = fight.agent.frostbrand.expect("Frostbrand Weapon is bound");
                let chance = fight.agent.frostbrand_chances[spell];
                weapon_imbues::frostbrand_trigger(fight, aura, hit, result, chance);
            }
            ShamanAura::MaelstromWeaponTrigger => {
                let state = fight.agent.maelstrom.expect("Maelstrom Weapon is bound");
                let chance = fight.agent.maelstrom_chances[spell];
                state.on_spell_hit_dealt(fight, spell, result, chance);
            }
            ShamanAura::RockbiterThreat => {
                let state = fight
                    .agent
                    .rockbiter_threat
                    .expect("Rockbiter Weapon Proc is bound");
                let main_hand = fight.agent.melee_main_hand[spell];
                let off_hand = fight.agent.melee_off_hand[spell];
                state.on_spell_hit_dealt(fight, main_hand, off_hand, result);
            }
            ShamanAura::WaterShield => Self::water_shield_result(fight, spell, result),
            _ => {}
        }
    }

    fn on_heal_dealt(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: ShamanAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        if kind == ShamanAura::WaterShield {
            Self::water_shield_result(fight, spell, result);
        }
    }

    fn on_spell_hit_taken(
        fight: &mut Fight<Self>,
        aura: AuraRef,
        kind: ShamanAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        if kind == ShamanAura::Stormstrike {
            let state = fight.agent.stormstrike.expect("Stormstrike is bound");
            state.on_spell_hit_taken(fight, aura, spell, result);
        }
    }

    fn on_delayed_proc(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: ShamanAura,
        spell: SpellId,
        result: SpellResult,
    ) {
        match kind {
            ShamanAura::FlurryTrigger => {
                let state = fight.agent.flurry.expect("Flurry is bound");
                let white = fight.agent.white[spell];
                let mut icd = fight.agent.flurry_icd;
                state.handle(fight, &result, white, &mut icd);
                fight.agent.flurry_icd = icd;
            }
            ShamanAura::ElementalDevastationTrigger => fight
                .agent
                .devastation
                .clone()
                .expect("Elemental Devastation is bound")
                .grant(fight),
            ShamanAura::ImprovedStormstrikeTrigger => fight
                .agent
                .improved_stormstrike
                .expect("Improved Stormstrike is bound")
                .grant(fight),
            ShamanAura::MaelstromWeaponTrigger => fight
                .agent
                .maelstrom
                .expect("Maelstrom Weapon is bound")
                .grant(fight),
            _ => {}
        }
    }

    fn on_stacks_change(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: ShamanAura,
        _old: i32,
        new: i32,
    ) {
        if kind == ShamanAura::MaelstromWeapon {
            let state = fight.agent.maelstrom.expect("Maelstrom Weapon is bound");
            state.on_stacks_change(fight, new);
        }
    }

    fn on_travel(
        fight: &mut Fight<Self>,
        spell: SpellId,
        result: SpellResult,
        behavior: ShamanSpell,
    ) {
        let overload = match behavior {
            ShamanSpell::LightningBolt => {
                let roll = fight
                    .agent
                    .lightning_bolt
                    .clone()
                    .expect("Lightning Bolt is bound");
                let copy = fight.agent.overloads[spell][0];
                Some((copy, roll))
            }
            _ => None,
        };
        lightning_bolt::arrive(
            fight,
            spell,
            result,
            overload.as_ref().map(|(copy, roll)| (*copy, roll)),
        );
    }

    fn on_dot_tick(fight: &mut Fight<Self>, dot: DotId, behavior: ShamanSpell) {
        match behavior {
            ShamanSpell::Heal => {
                let book = fight.agent.heals.clone().expect("the heals are bound");
                book.tick(fight, dot);
            }
            // Go CalcPeriodicAoeDamage and DealBatchedPeriodicDamage.
            ShamanSpell::MagmaTotem => {
                let (base, _) = fight.agent.magma.expect("Magma Totem is bound");
                magma_totem::tick(fight, dot, base);
            }
            ShamanSpell::FlameShockDot => fight.snapshot_dot_tick(dot),
            ShamanSpell::SearingTotem => {
                let state = Self::searing_totem(fight);
                let side = fight.dots[dot].side;
                searing_totem::tick(fight, side, &state);
            }
            _ => {}
        }
    }

    /// Go buffs/flametongue_totem.go `JoinFlametongueTotem`: the totem's effect turns the trigger
    /// on while it holds the Flametongue Totem category and off when it loses it, to a Windfury
    /// Totem (patch 98, community #677) or a main hand Flametongue Weapon, or fades.
    fn on_tracked_category_change(fight: &mut Fight<Self>, category: usize) {
        let Some(totem) = fight.agent.flametongue_totem.clone() else {
            return;
        };
        let (name, holder) = fight.tracked_category_holder(category);
        if name != "FlametongueTotem" {
            return;
        }
        let holds = totem.enabled && holder == Some(totem.aura);
        let active = fight.aura(totem.trigger).active;
        if holds && !active {
            fight.activate_aura(totem.trigger);
        } else if !holds && active {
            fight.deactivate_aura(totem.trigger);
        }
    }

    fn on_exclusive_gain(fight: &mut Fight<Self>, _aura: AuraRef, kind: ShamanAura) {
        if kind == ShamanAura::WindfuryTotemDummy {
            Self::windfury_totem(fight).on_dummy_gain(fight);
        }
        if kind == ShamanAura::WindfuryImbue {
            let state = fight
                .agent
                .windfury_weapon
                .clone()
                .expect("Windfury Weapon is bound");
            state.on_trigger_gain(fight);
        }
    }

    fn on_gain(fight: &mut Fight<Self>, _aura: AuraRef, kind: ShamanAura) {
        match kind {
            ShamanAura::Clearcasting => Self::focus(fight).on_gain(fight),
            ShamanAura::ElementalDevastation => fight
                .agent
                .devastation
                .clone()
                .expect("Elemental Devastation is bound")
                .on_gain(fight),
            ShamanAura::ImprovedStormstrike => fight
                .agent
                .improved_stormstrike
                .expect("Improved Stormstrike is bound")
                .on_gain(fight),
            ShamanAura::Flurry => fight.agent.flurry.expect("Flurry is bound").on_gain(fight),
            // The totem's effect in the Flametongue Totem category turns the trigger on as it
            // takes the category (`on_tracked_category_change`).
            ShamanAura::FlametongueTotem => {}
            ShamanAura::WindfuryTotem => {
                // Go AttachPeriodicAction with TickImmediately: the first tick is pending now.
                let now = fight.now;
                fight.agent.windfury_totem_tick =
                    Some(fight.schedule_class_action(now, PRIORITY_AUTO, 0));
            }
            ShamanAura::WindfuryTotemProc => Self::windfury_totem(fight).on_proc_gain(fight),
            ShamanAura::RageOfTheFarseer => fight
                .agent
                .farseer
                .expect("Rage of the Farseer is bound")
                .on_gain(fight),
            ShamanAura::ElementalMastery => fight
                .agent
                .elemental_mastery
                .expect("Elemental Mastery is bound")
                .on_gain(fight),
            ShamanAura::RockbiterWeapon => {
                if let (Some((line, _)), true) = (&fight.agent.rockbiter_logs, fight.log.is_some())
                {
                    let line = line.clone();
                    fight.player_log(&line);
                }
                if let Some(bit) = fight.agent.rockbiter_stat_bit {
                    fight.set_stat_mask(fight.stat_mask | bit);
                }
                if let Some(threat) = fight.agent.rockbiter_threat_multiplier {
                    fight.player.threat_multiplier *= threat;
                }
            }
            _ => {}
        }
    }

    fn on_expire(fight: &mut Fight<Self>, _aura: AuraRef, kind: ShamanAura) {
        match kind {
            ShamanAura::Clearcasting => Self::focus(fight).on_expire(fight),
            ShamanAura::ElementalDevastation => fight
                .agent
                .devastation
                .clone()
                .expect("Elemental Devastation is bound")
                .on_expire(fight),
            ShamanAura::ImprovedStormstrike => fight
                .agent
                .improved_stormstrike
                .expect("Improved Stormstrike is bound")
                .on_expire(fight),
            ShamanAura::MaelstromWeapon => fight
                .agent
                .maelstrom
                .expect("Maelstrom Weapon is bound")
                .on_expire(fight),
            ShamanAura::Flurry => fight
                .agent
                .flurry
                .expect("Flurry is bound")
                .on_expire(fight),
            ShamanAura::WindfuryTotem => {
                Self::windfury_totem(fight).on_totem_expire(fight);
                if let Some(tick) = fight.agent.windfury_totem_tick.take() {
                    fight.queue.cancel(tick);
                }
            }
            ShamanAura::WindfuryTotemDummy => Self::windfury_totem(fight).on_dummy_expire(fight),
            ShamanAura::WindfuryTotemProc => Self::windfury_totem(fight).on_proc_expire(fight),
            ShamanAura::WindfuryImbue => {
                let state = fight
                    .agent
                    .windfury_weapon
                    .clone()
                    .expect("Windfury Weapon is bound");
                state.on_trigger_expire(fight);
            }
            ShamanAura::FlametongueTotem => {
                let trigger = fight
                    .agent
                    .flametongue_totem
                    .as_ref()
                    .expect("Flametongue Totem is bound")
                    .trigger;
                fight.deactivate_aura(trigger);
            }
            ShamanAura::RageOfTheFarseer => fight
                .agent
                .farseer
                .expect("Rage of the Farseer is bound")
                .on_expire(fight),
            ShamanAura::ElementalMastery => fight
                .agent
                .elemental_mastery
                .expect("Elemental Mastery is bound")
                .on_expire(fight),
            ShamanAura::RockbiterWeapon => {
                if let (Some((_, line)), true) = (&fight.agent.rockbiter_logs, fight.log.is_some())
                {
                    let line = line.clone();
                    fight.player_log(&line);
                }
                if let Some(bit) = fight.agent.rockbiter_stat_bit {
                    fight.set_stat_mask(fight.stat_mask & !bit);
                }
                if let Some(threat) = fight.agent.rockbiter_threat_multiplier {
                    fight.player.threat_multiplier /= threat;
                }
            }
            _ => {}
        }
    }

    fn on_cast_complete(fight: &mut Fight<Self>, _aura: AuraRef, kind: ShamanAura, spell: SpellId) {
        match kind {
            ShamanAura::ElementalFocus => {
                let focus = Self::focus(fight);
                if let Some(trigger) = focus.roll(fight, spell) {
                    fight.agent.focus_trigger = Some(trigger);
                    focus.grant(fight);
                }
            }
            ShamanAura::Clearcasting => {
                let last = fight.agent.focus_trigger;
                Self::focus(fight).consume(fight, spell, last);
            }
            ShamanAura::ImprovedStormstrikeTrigger => {
                if fight.spells[spell].class_spell.as_deref() == Some("stormstrike_cast") {
                    fight
                        .agent
                        .improved_stormstrike
                        .expect("Improved Stormstrike is bound")
                        .on_cast_complete(fight, spell);
                }
            }
            ShamanAura::MaelstromWeapon => {
                let bolt = fight.spells[spell].class_spell.as_deref() == Some("lightning_bolt");
                fight
                    .agent
                    .maelstrom
                    .expect("Maelstrom Weapon is bound")
                    .on_cast_complete(fight, bolt);
            }
            _ => {}
        }
    }
}
