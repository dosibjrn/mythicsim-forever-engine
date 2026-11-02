//! The Warrior class agent for the fight runtime: class spells and auras by name, and the
//! hooks that dispatch to each spell's and talent's module.

use crate::{
    contracts::prepared_v2::{ActionId, DamageRoll, Effect, PreparedV2, Spell as ExportedSpell},
    core::fight::{
        movement::MovementKind, Agent, AuraRef, DotId, Fight, ModKind, Side, SpellId, SpellResult,
    },
};

use super::{
    items::MightRage,
    spells::{
        battle_shout::{self, BattleShout},
        berserker_rage::{self, BerserkerRage},
        bloodrage::{self, Bloodrage},
        bloodthirst::{self, Bloodthirst},
        challenging_shout,
        charge::{self, Charge},
        concussion_blow,
        death_wish::{self, DeathWish},
        demoralizing_shout, disarm,
        execute::{self, Execute},
        hamstring,
        heroic_strike::{self, Queue, Strike},
        intercept::{self, Intercept},
        intimidating_shout, mortal_strike, overpower,
        rend::{self, Rend},
        retaliation::{self, Retaliation},
        revenge::{self, Revenge},
        shield_block::ShieldBlock,
        shield_slam,
        shield_wall::ShieldWall,
        slam,
        spearing_strike::{self, SpearingStrike},
        stances::{self, Stance, StanceCast, StanceLock},
        sunder_armor::{self, SunderArmor},
        sweeping_strikes::{self, SweepingStrikes},
        taunt,
        thunder_clap::{self, ThunderClap},
        whirlwind::{self, Whirlwind},
    },
    talents::{
        anger_management::{self, AngerManagement},
        blood_craze::{self, BloodCraze},
        bloodthrill::{self, Bloodthrill},
        deep_wounds::{self, DeepWounds},
        enrage::{self, Enrage},
        flurry::{self, Flurry},
        improved_hamstring::{self, ImprovedHamstring},
        last_stand::LastStand,
        rage_on_avoid::{self, AvoidRage},
        unbridled_wrath::{self, UnbridledWrath},
        weaponmaster::{self, WeaponmasterSword},
    },
};

/// What a Warrior spell does when its effects apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WarriorSpell {
    Bloodthirst,
    Whirlwind,
    WhirlwindOffHand,
    Execute,
    Hamstring,
    Pummel,
    ShieldBash,
    MockingBlow,
    Disarm,
    /// victory_rush.go: castable only while Victorious is up.
    VictoryRush,
    /// intercept.go: the cast at range that runs to the target.
    Intercept,
    Bloodrage,
    BerserkerRage,
    /// charge.go: the cast before the pull that runs to the target.
    Charge,
    DeathWish,
    Recklessness,
    /// The warrior's own Sunder Armor, castable only while no other aura holds its category.
    SunderArmor,
    /// A stance cast or a spell a stance gates, which the gate keeps from ever casting.
    StanceLocked(StanceLock),
    /// shield_wall.go: the survival cooldown and its aura.
    ShieldWall,
    /// talents_protection.go: Last Stand's survival cooldown.
    LastStand,
    /// shield_block.go: the cast and its aura.
    ShieldBlock,
    DeepWounds,
    /// Heroic Strike or Cleave, by strike index.
    Strike(usize),
    /// A strike's queue cast, by strike index.
    QueueStrike(usize),
    /// A stance cast, by its position among the stances.
    Stance(usize),
    BattleShout,
    Rend,
    Overpower,
    MortalStrike,
    SpearingStrike,
    Slam,
    Revenge,
    ShieldSlam,
    ThunderClap,
    /// demoralizing_shout.go: the shout and its debuff on each target it lands on.
    DemoralizingShout,
    /// challenging_shout.go: an always hit with no damage on each target.
    ChallengingShout,
    /// taunt.go: an always hit with no damage on the target, in Defensive Stance.
    Taunt,
    /// intimidating_shout.go: an always hit with no damage on the target.
    IntimidatingShout,
    /// talents_protection.go: a special hit with no damage on the target, refunded on a miss.
    ConcussionBlow,
    Retaliation,
    RetaliationHit,
    SweepingStrikes,
    /// Sweeping Strikes' copy of a hit and its normalized main hand attack.
    SweepingStrikesHit,
    SweepingStrikesNormalizedHit,
    /// Blood Craze's hot, which only its trigger applies.
    BloodCraze,
}

/// Class auras with Rust behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WarriorAura {
    DeepWoundsTrigger,
    UnbridledWrath,
    FlurryTrigger,
    Flurry,
    OverpowerTrigger,
    DeathWish,
    BerserkerRage,
    /// charge.go's dash aura, which triples the movement speed.
    Dash,
    /// intercept.go's dash aura, which also lands the stun's damage at its end.
    InterceptDash,
    /// A strike's queue aura, by strike index.
    Queue(usize),
    BloodthrillTrigger,
    WeaponmasterSword,
    RevengeTrigger,
    /// Thunder Clap's debuff on the target.
    ThunderClap,
    Retaliation,
    /// A rage on avoid trigger, by its position.
    AvoidRage(usize),
    EnrageTrigger,
    Enrage,
    BloodCrazeDamageTaken,
    LastStand,
    /// shield_block.go: block chance whose stacks blocks spend.
    ShieldBlock,
    ImprovedHamstringTrigger,
    /// Battlegear of Might's 5 piece bonus.
    MightRage,
    /// Sweeping Strikes' charges, whose trigger copies hits to the next target.
    SweepingStrikes,
}

/// The class periodic tag of a strike's queue delay, plus the strike index.
const QUEUE_TAG: u32 = 16;

/// Warrior state that Go keeps in the `Warrior` struct and its closures.
#[derive(Default)]
pub(crate) struct WarriorAgent {
    stance: Stance,
    bloodthirst: Option<Bloodthirst>,
    whirlwind: Option<Whirlwind>,
    execute: Option<Execute>,
    hamstring: f64,
    pummel: f64,
    shield_bash: f64,
    mocking_blow: f64,
    disarm: Option<AuraRef>,
    victorious: Option<AuraRef>,
    intercept: Option<Intercept>,
    improved_hamstring: Option<ImprovedHamstring>,
    bloodrage: Option<Bloodrage>,
    berserker_rage: Option<BerserkerRage>,
    charge: Option<Charge>,
    death_wish: Option<DeathWish>,
    recklessness: Option<AuraRef>,
    sunder_blocked: bool,
    /// The warrior's own Sunder Armor and the target's armor category it bids in.
    sunder: Option<SunderArmor>,
    deep_wounds: Option<DeepWounds>,
    unbridled_wrath: Option<UnbridledWrath>,
    flurry: Option<Flurry>,
    anger_management: Option<AngerManagement>,
    overpower_window: Option<AuraRef>,
    queue: Queue,
    /// Whether each spell's proc mask holds `ProcMaskEmpty`.
    empty_mask: Vec<bool>,
    /// Whether each spell's proc mask holds a main hand bit.
    main_hand_mask: Vec<bool>,
    /// Go sim/warrior/talents_arms.go `BloodthrillWhiteSwingsOnly` (patch 176): the spells that can
    /// proc Bloodthrill, main hand white swings other than Retaliation's, Heroic Strike and Cleave.
    bloodthrill_swing: Vec<bool>,
    /// Whether each spell's proc mask holds an off hand bit.
    off_hand_mask: Vec<bool>,
    /// Go `struck` in `registerDeepWounds`: the crit that casts the bleed came from the off hand
    /// weapon of a dual wielder, for the one cast the trigger makes.
    deep_wounds_off_hand: bool,
    /// Whether each spell's proc mask holds a bit of a hand that wields a sword.
    sword_mask: Vec<bool>,
    /// With an item swap, Weaponmaster's sword mask for each set, Main then Swap1: the hands
    /// holding a sword, which its proc's condition reads.
    sword_masks: Option<[Vec<bool>; 2]>,
    /// With an item swap of the off hand, whether each set holds a shield, Main then Swap1:
    /// Shield Slam, Shield Block, Shield Wall and the rage on a block read it.
    shields: Option<[bool; 2]>,
    /// Go `WarriorInputs.DefaultStance`, which every reset restores.
    default_stance: Stance,
    stances: Vec<StanceCast>,
    max_retained_rage: f64,
    battle_shout: Option<BattleShout>,
    rend: Option<Rend>,
    overpower: f64,
    mortal_strike: f64,
    spearing_strike: Option<SpearingStrike>,
    slam: f64,
    /// Without Improved Slam the cast stops the swings.
    slam_stops_swings: bool,
    bloodthrill: Option<Bloodthrill>,
    weaponmaster: Option<WeaponmasterSword>,
    revenge: Option<Revenge>,
    shield_slam: Option<DamageRoll>,
    can_block: bool,
    thunder_clap: Option<ThunderClap>,
    /// Demoralizing Shout's debuff on the first target.
    demoralizing_shout: Option<AuraRef>,
    retaliation: Option<Retaliation>,
    sweeping_strikes: Option<SweepingStrikes>,
    /// Go `copyDamage`, which the Sweeping Strikes hit spell deals on its next cast.
    sweeping_copy: f64,
    might_rage: Option<MightRage>,
    shield_wall: Option<ShieldWall>,
    shield_block: Option<ShieldBlock>,
    pub(crate) last_stand: Option<LastStand>,
    avoid_rage: Vec<AvoidRage>,
    enrage: Option<Enrage>,
    blood_craze: Option<BloodCraze>,
}

/// Whether a prepared input carries the effect of a kind.
fn has_effect(prepared: &PreparedV2, kind: &str) -> bool {
    prepared.effects.iter().any(|effect| effect.kind() == kind)
}

fn id_of(spell: &ExportedSpell) -> ActionId {
    spell.action_id.clone().unwrap_or_default()
}

impl WarriorAgent {
    /// The class behavior of an exported spell, if Rust implements it.
    fn spell(prepared: &PreparedV2, spell: &ExportedSpell) -> Option<WarriorSpell> {
        let has = |kind| has_effect(prepared, kind);
        let id = id_of(spell);
        for effect in &prepared.effects {
            match effect {
                Effect::Bloodrage { spell_id, .. } if *spell_id == id.spell_id && id.tag == 0 => {
                    return Some(WarriorSpell::Bloodrage)
                }
                Effect::BloodCraze { spell_id, .. }
                    if *spell_id == id.spell_id && spell.dot.is_some() =>
                {
                    return Some(WarriorSpell::BloodCraze)
                }
                Effect::HeroicStrikeQueue { strikes, .. } => {
                    if let Some(index) = strikes.iter().position(|s| s.spell_id == id.spell_id) {
                        match id.tag {
                            0 => return Some(WarriorSpell::Strike(index)),
                            1 => return Some(WarriorSpell::QueueStrike(index)),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        let stance = |name: &str| {
            prepared.effects.iter().find_map(|effect| match effect {
                Effect::WarriorStances { stances, .. } => stances
                    .iter()
                    .position(|s| s.spell_id == id.spell_id && s.stance == name)
                    .map(WarriorSpell::Stance),
                _ => None,
            })
        };
        match spell.class_spell.as_deref()? {
            "bloodthirst" if has("bloodthirst") => Some(WarriorSpell::Bloodthirst),
            "whirlwind" if has("whirlwind") => Some(WarriorSpell::Whirlwind),
            "whirlwind_off_hand" if has("whirlwind") => Some(WarriorSpell::WhirlwindOffHand),
            "execute" if has("execute") => Some(WarriorSpell::Execute),
            "hamstring" if has("hamstring") => Some(WarriorSpell::Hamstring),
            "pummel" if has("pummel") => Some(WarriorSpell::Pummel),
            "shield_bash" if has("shield_bash") => Some(WarriorSpell::ShieldBash),
            "mocking_blow" if has("mocking_blow") => Some(WarriorSpell::MockingBlow),
            "disarm" if has("warrior_disarm") => Some(WarriorSpell::Disarm),
            "victory_rush" if has("victory_rush") => Some(WarriorSpell::VictoryRush),
            "intercept" if has("warrior_intercept") => Some(WarriorSpell::Intercept),
            "berserker_rage" if has("berserker_rage") => Some(WarriorSpell::BerserkerRage),
            "charge" if has("warrior_charge") => Some(WarriorSpell::Charge),
            "death_wish" if has("death_wish") => Some(WarriorSpell::DeathWish),
            "recklessness" if has("recklessness") => Some(WarriorSpell::Recklessness),
            "sunder_armor" if has("sunder_armor") => Some(WarriorSpell::SunderArmor),
            "deep_wounds" if spell.dot.is_some() && has("deep_wounds") => {
                Some(WarriorSpell::DeepWounds)
            }
            "battle_stance" => stance("battle"),
            "defensive_stance" => stance("defensive"),
            "berserker_stance" => stance("berserker"),
            "battle_shout" if has("battle_shout") => Some(WarriorSpell::BattleShout),
            "rend" if spell.dot.is_some() && has("rend") => Some(WarriorSpell::Rend),
            "overpower" if has("overpower") => Some(WarriorSpell::Overpower),
            "mortal_strike" if has("mortal_strike") => Some(WarriorSpell::MortalStrike),
            "spearing_strike" if has("spearing_strike") => Some(WarriorSpell::SpearingStrike),
            "slam" if has("slam") => Some(WarriorSpell::Slam),
            "revenge" if has("revenge") => Some(WarriorSpell::Revenge),
            "shield_slam" if has("shield_slam") => Some(WarriorSpell::ShieldSlam),
            "thunder_clap" if has("thunder_clap") => Some(WarriorSpell::ThunderClap),
            "demoralizing_shout" if has("demoralizing_shout") => {
                Some(WarriorSpell::DemoralizingShout)
            }
            "challenging_shout" if has("challenging_shout") => Some(WarriorSpell::ChallengingShout),
            "taunt" if has("taunt") => Some(WarriorSpell::Taunt),
            "intimidating_shout" if has("intimidating_shout") => {
                Some(WarriorSpell::IntimidatingShout)
            }
            "concussion_blow" if has("concussion_blow") => Some(WarriorSpell::ConcussionBlow),
            "retaliation" if has("retaliation") => Some(WarriorSpell::Retaliation),
            "retaliation_hit" if has("retaliation") => Some(WarriorSpell::RetaliationHit),
            "sweeping_strikes" if has("sweeping_strikes") => Some(WarriorSpell::SweepingStrikes),
            "sweeping_strikes_hit" if has("sweeping_strikes") => {
                Some(WarriorSpell::SweepingStrikesHit)
            }
            "sweeping_strikes_normalized_hit" if has("sweeping_strikes") => {
                Some(WarriorSpell::SweepingStrikesNormalizedHit)
            }
            "retaliation" if has("warrior_stances") => {
                Some(WarriorSpell::StanceLocked(StanceLock::Battle))
            }
            "shield_wall" if has("shield_wall") => Some(WarriorSpell::ShieldWall),
            "last_stand" if has("last_stand") => Some(WarriorSpell::LastStand),
            "shield_block" if has("shield_block") => Some(WarriorSpell::ShieldBlock),
            "shield_wall" if has("warrior_stances") => {
                Some(WarriorSpell::StanceLocked(StanceLock::Defensive))
            }
            _ => None,
        }
    }

    /// Aura labels claimed by implemented class effects, as (unit, label, kind).
    fn auras(prepared: &PreparedV2) -> Vec<(&'static str, String, WarriorAura)> {
        let mut auras: Vec<(&'static str, String, WarriorAura)> = Self::player_auras(prepared)
            .into_iter()
            .map(|(label, kind)| ("player", label, kind))
            .collect();
        for effect in &prepared.effects {
            if let Effect::ThunderClap { aura, .. } = effect {
                auras.push(("target", aura.clone(), WarriorAura::ThunderClap));
            }
        }
        auras
    }

    /// Player aura labels claimed by implemented class effects.
    fn player_auras(prepared: &PreparedV2) -> Vec<(String, WarriorAura)> {
        let mut auras = Vec::new();
        for effect in &prepared.effects {
            match effect {
                Effect::Revenge { trigger_aura, .. } => {
                    auras.push((trigger_aura.clone(), WarriorAura::RevengeTrigger))
                }
                Effect::Retaliation { aura, .. } => {
                    auras.push((aura.clone(), WarriorAura::Retaliation))
                }
                Effect::BattlegearOfMightRage { trigger_aura, .. } => {
                    auras.push((trigger_aura.clone(), WarriorAura::MightRage))
                }
                Effect::ImprovedHamstring { trigger_aura, .. } => {
                    auras.push((trigger_aura.clone(), WarriorAura::ImprovedHamstringTrigger))
                }
                Effect::RageOnAvoid { triggers, .. } => {
                    for (index, trigger) in triggers.iter().enumerate() {
                        auras.push((trigger.aura.clone(), WarriorAura::AvoidRage(index)));
                    }
                }
                Effect::WarriorEnrage {
                    trigger_aura, aura, ..
                } => {
                    auras.push((trigger_aura.clone(), WarriorAura::EnrageTrigger));
                    auras.push((aura.clone(), WarriorAura::Enrage));
                }
                Effect::BloodCraze {
                    damage_taken_aura, ..
                } => auras.push((
                    damage_taken_aura.clone(),
                    WarriorAura::BloodCrazeDamageTaken,
                )),
                Effect::DeepWounds { trigger_aura, .. } => {
                    auras.push((trigger_aura.clone(), WarriorAura::DeepWoundsTrigger))
                }
                Effect::UnbridledWrath { trigger_aura, .. } => {
                    auras.push((trigger_aura.clone(), WarriorAura::UnbridledWrath))
                }
                Effect::WarriorFlurry {
                    trigger_aura, aura, ..
                } => {
                    auras.push((trigger_aura.clone(), WarriorAura::FlurryTrigger));
                    auras.push((aura.clone(), WarriorAura::Flurry));
                }
                Effect::Bloodthrill { trigger_aura, .. } => {
                    auras.push((trigger_aura.clone(), WarriorAura::BloodthrillTrigger))
                }
                Effect::WeaponmasterSword { trigger_aura, .. } => {
                    auras.push((trigger_aura.clone(), WarriorAura::WeaponmasterSword))
                }
                Effect::OverpowerWindow { trigger_aura, .. } => {
                    auras.push((trigger_aura.clone(), WarriorAura::OverpowerTrigger))
                }
                Effect::DeathWish { aura, .. } => {
                    auras.push((aura.clone(), WarriorAura::DeathWish))
                }
                Effect::SweepingStrikes { aura, .. } => {
                    auras.push((aura.clone(), WarriorAura::SweepingStrikes))
                }
                Effect::BerserkerRage { aura, .. } => {
                    auras.push((aura.clone(), WarriorAura::BerserkerRage))
                }
                Effect::WarriorCharge { aura, .. } => auras.push((aura.clone(), WarriorAura::Dash)),
                Effect::WarriorIntercept { aura, .. } => {
                    auras.push((aura.clone(), WarriorAura::InterceptDash))
                }
                Effect::LastStand { aura, .. } => {
                    auras.push((aura.clone(), WarriorAura::LastStand))
                }
                Effect::ShieldBlock { aura, .. } => {
                    auras.push((aura.clone(), WarriorAura::ShieldBlock))
                }
                Effect::HeroicStrikeQueue { strikes, .. } => {
                    for (index, strike) in strikes.iter().enumerate() {
                        auras.push((strike.queue_aura.clone(), WarriorAura::Queue(index)));
                    }
                }
                _ => {}
            }
        }
        auras
    }

    /// Build a fight for a prepared input that passed the coverage gate.
    pub(crate) fn fight(prepared: &PreparedV2) -> Result<Fight<WarriorAgent>, String> {
        let auras = Self::auras(prepared);
        let spell = |exported: &ExportedSpell| WarriorAgent::spell(prepared, exported);
        let mut fight = Fight::new(prepared, WarriorAgent::default(), spell, |unit, label| {
            auras
                .iter()
                .find(|(side, name, _)| *side == unit && name == label)
                .map(|(_, _, kind)| *kind)
        })?;
        fight.agent.main_hand_mask = prepared
            .player
            .spells
            .iter()
            .map(|spell| {
                spell
                    .proc_mask
                    .iter()
                    .any(|mask| mask == "ProcMaskMeleeMHAuto" || mask == "ProcMaskMeleeMHSpecial")
            })
            .collect();
        fight.agent.bloodthrill_swing = prepared
            .player
            .spells
            .iter()
            .map(|spell| {
                let class = spell.class_spell.as_deref();
                let white = spell
                    .proc_mask
                    .iter()
                    .any(|mask| mask == "ProcMaskMeleeMHAuto")
                    && !matches!(class, Some("retaliation" | "retaliation_hit"));
                white || matches!(class, Some("heroic_strike" | "cleave"))
            })
            .collect();
        fight.agent.off_hand_mask = prepared
            .player
            .spells
            .iter()
            .map(|spell| {
                spell
                    .proc_mask
                    .iter()
                    .any(|mask| mask == "ProcMaskMeleeOHAuto" || mask == "ProcMaskMeleeOHSpecial")
            })
            .collect();
        fight.agent.empty_mask = prepared
            .player
            .spells
            .iter()
            .map(|spell| spell.proc_mask.iter().any(|mask| mask == "ProcMaskEmpty"))
            .collect();
        let find = |fight: &Fight<WarriorAgent>, spell_id: i32, tag: i32| {
            fight
                .spells
                .iter()
                .position(|spell| spell.id.spell_id == spell_id && spell.id.tag == tag)
                .ok_or_else(|| format!("spell {spell_id} tag {tag} is not registered"))
        };
        let rage_metrics = |fight: &mut Fight<WarriorAgent>, spell_id: i32| {
            fight.new_rage_metrics(ActionId {
                spell_id,
                ..ActionId::default()
            })
        };
        for effect in &prepared.effects {
            match effect {
                Effect::WarriorStances {
                    default_stance,
                    stances,
                    max_retained_rage,
                } => {
                    fight.agent.default_stance = Stance::from_name(default_stance);
                    fight.agent.stance = fight.agent.default_stance;
                    fight.agent.max_retained_rage = *max_retained_rage;
                    for entry in stances {
                        let aura = fight.player_aura(&entry.aura)?;
                        let metrics = rage_metrics(&mut fight, entry.spell_id);
                        fight.agent.stances.push(StanceCast {
                            stance: Stance::from_name(&entry.stance),
                            aura,
                            metrics,
                        });
                    }
                }
                Effect::BattleShout {
                    aura,
                    value,
                    refresh_threshold_ns,
                    ..
                } => {
                    let aura = fight.player_aura(aura)?;
                    let category = fight
                        .exclusive
                        .iter()
                        .position(|category| category.members.iter().any(|m| m.aura == aura))
                        .ok_or("Battle Shout has no exclusive category".to_string())?;
                    fight.agent.battle_shout = Some(BattleShout {
                        aura,
                        category,
                        value: *value,
                        refresh_threshold: *refresh_threshold_ns,
                    });
                }
                Effect::Rend {
                    spell_id,
                    tick_base,
                    attack_power_per_tick,
                    tick_can_crit,
                    ..
                } => {
                    let spell = find(&fight, *spell_id, 0)?;
                    let dot = fight.spells[spell]
                        .dot
                        .ok_or("Rend has no dot".to_string())?;
                    fight.agent.rend = Some(Rend {
                        dot,
                        tick_base: *tick_base,
                        attack_power_per_tick: *attack_power_per_tick,
                        tick_can_crit: *tick_can_crit,
                    });
                }
                Effect::Overpower { base_damage, .. } => fight.agent.overpower = *base_damage,
                Effect::MortalStrike { base_damage, .. } => {
                    fight.agent.mortal_strike = *base_damage
                }
                Effect::SpearingStrike {
                    weapon_share,
                    mob_multiplier,
                    ..
                } => {
                    fight.agent.spearing_strike = Some(SpearingStrike {
                        weapon_share: *weapon_share,
                        mob_multiplier: *mob_multiplier,
                    })
                }
                Effect::Slam {
                    base_damage,
                    stops_swings,
                    ..
                } => {
                    fight.agent.slam = *base_damage;
                    fight.agent.slam_stops_swings = *stops_swings;
                }
                Effect::Revenge {
                    aura,
                    damage,
                    attack_power_share,
                    ..
                } => {
                    fight.agent.revenge = Some(Revenge {
                        aura: fight.player_aura(aura)?,
                        damage: *damage,
                        attack_power_share: *attack_power_share,
                    })
                }
                Effect::ShieldSlam {
                    damage, can_block, ..
                } => {
                    fight.agent.shield_slam = Some(*damage);
                    fight.agent.can_block = *can_block;
                }
                Effect::ImprovedHamstring {
                    trigger_aura,
                    aura,
                    proc_chance,
                    delay_ns,
                } => {
                    let index = fight.trackers[Side::Target.index()]
                        .find(aura)
                        .ok_or_else(|| format!("target aura {aura} is not registered"))?;
                    fight.agent.improved_hamstring = Some(ImprovedHamstring {
                        trigger: fight.player_aura(trigger_aura)?,
                        root: AuraRef {
                            side: Side::Target,
                            index,
                        },
                        proc_chance: *proc_chance,
                        delay: *delay_ns,
                    });
                }
                Effect::ThunderClap {
                    base_damage,
                    attack_power_share,
                    max_targets,
                    aura,
                    bid,
                    ..
                } => {
                    let index = fight.trackers[Side::Target.index()]
                        .find(aura)
                        .ok_or_else(|| format!("target aura {aura} is not registered"))?;
                    let aura = AuraRef {
                        side: Side::Target,
                        index,
                    };
                    fight.agent.thunder_clap = Some(ThunderClap {
                        aura,
                        base_damage: *base_damage,
                        attack_power_share: *attack_power_share,
                        max_targets: *max_targets as usize,
                    });
                    fight.register_enemy_slow(aura, thunder_clap::slow(*bid));
                }
                Effect::DemoralizingShout { aura, .. } => {
                    let index = fight.trackers[Side::Target.index()]
                        .find(aura)
                        .ok_or_else(|| format!("target aura {aura} is not registered"))?;
                    fight.agent.demoralizing_shout = Some(AuraRef {
                        side: Side::Target,
                        index,
                    });
                }
                Effect::LastStand {
                    aura,
                    health_share,
                    metrics_action_id,
                    ..
                } => {
                    let bound = LastStand::bind(
                        &mut fight,
                        &prepared.effects,
                        aura,
                        *health_share,
                        metrics_action_id.clone(),
                    )?;
                    fight.agent.last_stand = Some(bound);
                }
                Effect::ShieldWall {
                    aura,
                    autocast,
                    health_percent,
                    can_block,
                    ..
                } => {
                    fight.agent.shield_wall = Some(ShieldWall {
                        aura: fight.player_aura(aura)?,
                        autocast: *autocast,
                        health_percent: *health_percent,
                        can_block: *can_block,
                    })
                }
                Effect::ShieldBlock {
                    aura, can_block, ..
                } => {
                    let bound = ShieldBlock::bind(&fight, &prepared.effects, aura, *can_block)?;
                    fight.agent.shield_block = Some(bound);
                }
                Effect::Retaliation {
                    aura,
                    hit_spell_id,
                    charges,
                    hit_base_damage,
                    ..
                } => {
                    fight.agent.retaliation = Some(Retaliation {
                        aura: fight.player_aura(aura)?,
                        hit: find(&fight, *hit_spell_id, 0)?,
                        charges: *charges,
                        hit_base_damage: *hit_base_damage,
                    })
                }
                Effect::BattlegearOfMightRage {
                    rng_label,
                    proc_chance,
                    rage,
                    metrics_action_id,
                    ..
                } => {
                    let metrics = fight.new_rage_metrics(metrics_action_id.clone());
                    fight.agent.might_rage = Some(MightRage {
                        rng_label: rng_label.clone(),
                        proc_chance: *proc_chance,
                        rage: *rage,
                        metrics,
                    })
                }
                Effect::SweepingStrikes { aura, charges, .. } => {
                    let class_spell = |fight: &Fight<WarriorAgent>, name: &str| {
                        fight
                            .spells
                            .iter()
                            .position(|spell| spell.class_spell.as_deref() == Some(name))
                            .ok_or_else(|| format!("Sweeping Strikes has no {name} spell"))
                    };
                    fight.agent.sweeping_strikes = Some(SweepingStrikes {
                        aura: fight.player_aura(aura)?,
                        charges: *charges,
                        hit: class_spell(&fight, "sweeping_strikes_hit")?,
                        normalized: class_spell(&fight, "sweeping_strikes_normalized_hit")?,
                    })
                }
                Effect::RageOnAvoid {
                    can_block,
                    triggers,
                } => {
                    for trigger in triggers {
                        let metrics = rage_metrics(&mut fight, trigger.spell_id);
                        fight.agent.avoid_rage.push(AvoidRage {
                            aura: fight.player_aura(&trigger.aura)?,
                            outcomes: rage_on_avoid::outcome_bits(&trigger.outcomes),
                            needs_block: trigger.needs_block,
                            allowed: !trigger.needs_block || *can_block,
                            chance: trigger.chance,
                            rage: trigger.rage,
                            metrics,
                        });
                    }
                }
                Effect::WarriorEnrage {
                    trigger_aura,
                    aura,
                    proc_chance,
                    physical_damage_done,
                } => {
                    // Go AttachSpellMod with the physical school: every physical spell of the
                    // warrior without SpellFlagNoSpellMods.
                    let physical = (0..fight.spells.len())
                        .filter(|&spell| {
                            let state = &fight.spells[spell];
                            state.caster == Side::Player
                                && state.school & 1 != 0
                                && !state.flags.no_spell_mods
                        })
                        .collect();
                    let damage_mod = fight.register_mod(
                        ModKind::DamageDonePercent,
                        *physical_damage_done,
                        0,
                        physical,
                    );
                    fight.agent.enrage = Some(Enrage {
                        trigger: fight.player_aura(trigger_aura)?,
                        aura: fight.player_aura(aura)?,
                        proc_chance: *proc_chance,
                        damage_mod,
                    });
                }
                Effect::BloodCraze {
                    spell_id,
                    heal,
                    health_fraction,
                    hit_threshold,
                    ..
                } => {
                    let spell = find(&fight, *spell_id, 0)?;
                    let dot = fight.spells[spell]
                        .dot
                        .ok_or("Blood Craze has no hot".to_string())?;
                    fight.agent.blood_craze = Some(BloodCraze {
                        dot,
                        heal: *heal,
                        health_fraction: *health_fraction,
                        hit_threshold: *hit_threshold,
                    });
                }
                Effect::WeaponmasterSword {
                    trigger_aura,
                    proc_chance,
                    extra_attack_tag,
                    sword_hands,
                    excluded_class_spells,
                } => {
                    let extra_attack = fight
                        .spells
                        .iter()
                        .position(|spell| {
                            spell.id.other_id == "OtherActionAttack"
                                && spell.id.tag == *extra_attack_tag
                        })
                        .ok_or("Weaponmaster has no extra attack".to_string())?;
                    fight.spells[extra_attack].behavior =
                        crate::core::fight::SpellBehavior::MeleeAuto(
                            crate::core::fight::melee::Hand::Main,
                        );
                    let mask_of = |sword_hands: &[String]| -> Vec<bool> {
                        let hand_bits = |mask: &str| {
                            (sword_hands.iter().any(|h| h == "main")
                                && matches!(mask, "ProcMaskMeleeMHAuto" | "ProcMaskMeleeMHSpecial"))
                                || (sword_hands.iter().any(|h| h == "off")
                                    && matches!(
                                        mask,
                                        "ProcMaskMeleeOHAuto" | "ProcMaskMeleeOHSpecial"
                                    ))
                        };
                        // The trigger's ExtraCondition leaves the excluded class spells out.
                        prepared
                            .player
                            .spells
                            .iter()
                            .map(|spell| {
                                spell.proc_mask.iter().any(|mask| hand_bits(mask))
                                    && !spell
                                        .class_spell
                                        .as_ref()
                                        .is_some_and(|class| excluded_class_spells.contains(class))
                            })
                            .collect()
                    };
                    fight.agent.sword_mask = mask_of(sword_hands);
                    // talents_arms.go readWeapons: an item swap rereads the hands.
                    let worn = fight.item_swap_worn_effects().and_then(|effects| {
                        effects.into_iter().find_map(|effect| match effect {
                            Effect::WeaponmasterSword { sword_hands, .. } => Some(sword_hands),
                            _ => None,
                        })
                    });
                    if let Some(worn) = worn {
                        fight.agent.sword_masks =
                            Some([fight.agent.sword_mask.clone(), mask_of(&worn)]);
                    }
                    fight.agent.weaponmaster = Some(WeaponmasterSword {
                        trigger: fight.player_aura(trigger_aura)?,
                        proc_chance: *proc_chance,
                        extra_attack,
                    });
                }
                Effect::Bloodthirst {
                    attack_power_share,
                    base_damage,
                    ..
                } => {
                    fight.agent.bloodthirst = Some(Bloodthirst {
                        attack_power_share: *attack_power_share,
                        base_damage: *base_damage,
                    })
                }
                Effect::Whirlwind {
                    spell_id,
                    off_hand,
                    max_targets,
                } => {
                    let off_hand = if *off_hand && prepared.melee.dual_wielding {
                        Some(find(&fight, *spell_id, 2)?)
                    } else {
                        None
                    };
                    fight.agent.whirlwind = Some(Whirlwind {
                        off_hand,
                        max_targets: *max_targets as usize,
                    });
                }
                Effect::Execute {
                    base_damage,
                    damage_per_rage,
                    ..
                } => {
                    fight.agent.execute = Some(Execute {
                        base_damage: *base_damage,
                        damage_per_rage: *damage_per_rage,
                    })
                }
                Effect::Hamstring { base_damage, .. } => fight.agent.hamstring = *base_damage,
                Effect::Pummel { base_damage, .. } => fight.agent.pummel = *base_damage,
                Effect::ShieldBash {
                    base_damage,
                    can_block,
                    ..
                } => {
                    fight.agent.shield_bash = *base_damage;
                    fight.agent.can_block = *can_block;
                }
                Effect::MockingBlow { base_damage, .. } => fight.agent.mocking_blow = *base_damage,
                Effect::WarriorDisarm { aura, .. } => {
                    let index = fight.trackers[Side::Target.index()]
                        .find(aura)
                        .ok_or_else(|| format!("target aura {aura} is not registered"))?;
                    fight.agent.disarm = Some(AuraRef {
                        side: Side::Target,
                        index,
                    });
                }
                Effect::VictoryRush { aura, .. } => {
                    fight.agent.victorious = Some(fight.player_aura(aura)?);
                }
                Effect::WarriorIntercept {
                    aura,
                    speed_multiplier,
                    overshoot,
                    min_range,
                    damage,
                    ..
                } => {
                    fight.agent.intercept = Some(Intercept {
                        aura: fight.player_aura(aura)?,
                        speed_multiplier: *speed_multiplier,
                        overshoot: *overshoot,
                        min_range: *min_range,
                        damage: *damage,
                        cast: None,
                    });
                }
                Effect::Bloodrage {
                    spell_id,
                    instant_rage,
                    rage_per_tick,
                    ticks,
                    period_ns,
                    health_cost,
                    rage_threshold,
                } => {
                    let metrics = rage_metrics(&mut fight, *spell_id);
                    fight.agent.bloodrage = Some(Bloodrage {
                        instant_rage: *instant_rage,
                        rage_per_tick: *rage_per_tick,
                        ticks: *ticks,
                        period: *period_ns,
                        health_cost: *health_cost,
                        rage_threshold: *rage_threshold,
                        metrics,
                    });
                }
                Effect::BerserkerRage {
                    spell_id,
                    aura,
                    rage_gain,
                } => {
                    let metrics = rage_metrics(&mut fight, *spell_id);
                    fight.agent.berserker_rage = Some(BerserkerRage {
                        aura: fight.player_aura(aura)?,
                        rage_gain: *rage_gain,
                        metrics,
                    });
                }
                Effect::WarriorCharge {
                    spell_id,
                    aura,
                    rage,
                    vanguard,
                    speed_multiplier,
                    overshoot,
                    min_range,
                    no_threat,
                } => {
                    let metrics = rage_metrics(&mut fight, *spell_id);
                    fight.resources[metrics].no_threat = *no_threat;
                    fight.agent.charge = Some(Charge {
                        aura: fight.player_aura(aura)?,
                        rage: *rage,
                        vanguard: *vanguard,
                        speed_multiplier: *speed_multiplier,
                        overshoot: *overshoot,
                        min_range: *min_range,
                        metrics,
                    });
                }
                Effect::DeathWish {
                    aura,
                    physical_multiplier,
                    wait_ns,
                    ..
                } => {
                    fight.agent.death_wish = Some(DeathWish {
                        aura: fight.player_aura(aura)?,
                        physical_multiplier: *physical_multiplier,
                        wait: *wait_ns,
                    })
                }
                Effect::Recklessness { aura, .. } => {
                    fight.agent.recklessness = Some(fight.player_aura(aura)?)
                }
                Effect::SunderArmor {
                    blocked,
                    aura,
                    flat_threat,
                    threat_per_attack_power,
                    ..
                } => {
                    fight.agent.sunder_blocked = *blocked;
                    let index = fight.trackers[Side::Target.index()].find(aura);
                    let category = fight.armor_category.as_ref().map(|(category, _)| *category);
                    if let (Some(index), Some(category), false) = (index, category, *blocked) {
                        let aura = AuraRef {
                            side: Side::Target,
                            index,
                        };
                        let member = fight.exclusive[category]
                            .members
                            .iter()
                            .any(|member| member.aura == aura);
                        if member {
                            fight.agent.sunder = Some(SunderArmor {
                                aura,
                                category,
                                flat_threat: *flat_threat,
                                threat_per_attack_power: *threat_per_attack_power,
                            });
                        }
                    }
                }
                Effect::DeepWounds {
                    spell_id,
                    share,
                    tick_can_crit,
                    ..
                } => {
                    let spell = find(&fight, *spell_id, 0)?;
                    let dot = fight.spells[spell]
                        .dot
                        .ok_or("Deep Wounds has no dot".to_string())?;
                    fight.agent.deep_wounds = Some(DeepWounds {
                        spell,
                        dot,
                        share: *share,
                        tick_can_crit: *tick_can_crit,
                    });
                }
                Effect::UnbridledWrath {
                    trigger_aura,
                    spell_id,
                    proc_chance,
                    rage,
                    ..
                } => {
                    let metrics = rage_metrics(&mut fight, *spell_id);
                    fight.agent.unbridled_wrath = Some(UnbridledWrath {
                        trigger: fight.player_aura(trigger_aura)?,
                        proc_chance: *proc_chance,
                        rage: *rage,
                        metrics,
                    });
                }
                Effect::WarriorFlurry {
                    aura,
                    melee_speed_multiplier,
                    charges,
                    every_swing_spends,
                    ..
                } => {
                    fight.agent.flurry = Some(Flurry {
                        aura: fight.player_aura(aura)?,
                        melee_speed_multiplier: *melee_speed_multiplier,
                        charges: *charges,
                        every_swing_spends: *every_swing_spends,
                    })
                }
                Effect::AngerManagement {
                    spell_id,
                    rage,
                    period_ns,
                } => {
                    let metrics = rage_metrics(&mut fight, *spell_id);
                    fight.agent.anger_management = Some(AngerManagement {
                        rage: *rage,
                        period: *period_ns,
                        metrics,
                    });
                }
                Effect::OverpowerWindow { aura, .. } => {
                    fight.agent.overpower_window = Some(fight.player_aura(aura)?)
                }
                Effect::HeroicStrikeQueue {
                    queue_delay_ns,
                    strikes,
                    cleave_threat_split,
                } => {
                    let mut queue = Queue::default();
                    for strike in strikes {
                        queue.strikes.push(Strike {
                            spell: find(&fight, strike.spell_id, 0)?,
                            queue_aura: fight.player_aura(&strike.queue_aura)?,
                            base_damage: strike.base_damage,
                            cleave: strike.cleave,
                            threat_split: *cleave_threat_split,
                        });
                        queue.queued.push(false);
                    }
                    // Go queuedRealismICD: its own timer.
                    fight.timers.push(crate::core::time::STARTING_CD_TIME);
                    queue.realism = Some((fight.timers.len() - 1, *queue_delay_ns));
                    fight.agent.queue = queue;
                }
                _ => {}
            }
        }
        for effect in &prepared.effects {
            if let Effect::Bloodthrill {
                trigger_aura,
                proc_chance,
                window_ns,
                ..
            } = effect
            {
                let rend = fight
                    .agent
                    .rend
                    .ok_or("Bloodthrill needs Rend".to_string())?;
                let window = fight
                    .agent
                    .overpower_window
                    .ok_or("Bloodthrill needs the Overpower window".to_string())?;
                fight.agent.bloodthrill = Some(Bloodthrill {
                    trigger: fight.player_aura(trigger_aura)?,
                    proc_chance: *proc_chance,
                    window_duration: *window_ns,
                    overpower_window: window,
                    rend_dot: rend.dot,
                });
            }
        }
        // An off hand swap puts a shield on or takes it off: what each set says.
        let can_block = |effects: &[Effect]| {
            effects.iter().find_map(|effect| match effect {
                Effect::ShieldWall { can_block, .. }
                | Effect::ShieldBlock { can_block, .. }
                | Effect::ShieldSlam { can_block, .. }
                | Effect::ShieldBash { can_block, .. }
                | Effect::RageOnAvoid { can_block, .. } => Some(*can_block),
                _ => None,
            })
        };
        if let (Some(main), Some(swapped)) = (
            can_block(&prepared.effects),
            fight
                .item_swap_worn_effects()
                .and_then(|effects| can_block(&effects)),
        ) {
            if main != swapped {
                fight.agent.shields = Some([main, swapped]);
            }
        }
        Ok(fight)
    }
}

impl WarriorAgent {
    /// heroic_strike_cleave.go `consumeQueuedSwing`: drop the queue the strike fired from.
    fn consume_queue(fight: &mut Fight<Self>) {
        if let Some(current) = fight.agent.queue.current {
            let aura = fight.agent.queue.strikes[current].queue_aura;
            fight.deactivate_aura(aura);
        }
    }

    /// Go `registerCleave`'s `ApplyEffects` from `results = results[:0]` on: each hit's result
    /// object takes its place in the slice every cast of Cleave shares, and the deal loop reads
    /// the slice's element when its turn comes, so a cast that starts during a deal replaces
    /// the results this one has yet to deal.
    fn deal_cleave(fight: &mut Fight<Self>, spell: SpellId, results: Vec<SpellResult>) {
        for (place, result) in results.iter().enumerate() {
            let queue = &mut fight.agent.queue;
            let id = queue.pool.new_result_of(result);
            match queue.cleave_slice.get_mut(place) {
                Some(slot) => *slot = id,
                None => queue.cleave_slice.push(id),
            }
        }
        for place in 0..results.len() {
            let queue = &mut fight.agent.queue;
            let id = queue.cleave_slice[place];
            let result = queue.pool.result(id);
            queue.dealing.push(id);
            fight.deal_damage(spell, result, false);
            let queue = &mut fight.agent.queue;
            queue.dealing.pop();
            queue.pool.dispose(id);
        }
    }

    /// The warrior's listeners of hits it takes. Go's proc triggers skip a spell with the proc
    /// flag; Retaliation, a plain listener, hears only melee hits.
    fn hit_taken(
        fight: &mut Fight<Self>,
        kind: WarriorAura,
        result: &SpellResult,
        proc: bool,
        melee: bool,
    ) {
        match kind {
            WarriorAura::Retaliation if melee => {
                let params = fight.agent.retaliation.expect("Retaliation is bound");
                retaliation::on_hit_taken(fight, params, result);
            }
            _ if proc => {}
            WarriorAura::RevengeTrigger => {
                let params = fight.agent.revenge.expect("Revenge is bound");
                revenge::on_hit_taken(fight, params, result);
            }
            WarriorAura::AvoidRage(index) => {
                let params = fight.agent.avoid_rage[index];
                rage_on_avoid::on_hit_taken(fight, params, result);
            }
            WarriorAura::ShieldBlock => {
                let params = fight.agent.shield_block.expect("Shield Block is bound");
                params.on_hit_taken(fight, result);
            }
            WarriorAura::EnrageTrigger => {
                let params = fight.agent.enrage.expect("Enrage is bound");
                enrage::on_hit_taken(fight, params, result);
            }
            WarriorAura::BloodCrazeDamageTaken => {
                let params = fight.agent.blood_craze.expect("Blood Craze is bound");
                blood_craze::on_hit_taken(fight, params, result);
            }
            _ => {}
        }
    }
}

impl Agent for WarriorAgent {
    type Spell = WarriorSpell;
    type Aura = WarriorAura;

    fn on_item_swap(fight: &mut Fight<Self>, worn: bool) {
        if let Some(masks) = &fight.agent.sword_masks {
            fight.agent.sword_mask = masks[usize::from(worn)].clone();
        }
        if let Some(shields) = fight.agent.shields {
            let can_block = shields[usize::from(worn)];
            let agent = &mut fight.agent;
            agent.can_block = can_block;
            if let Some(shield_wall) = agent.shield_wall.as_mut() {
                shield_wall.can_block = can_block;
            }
            if let Some(shield_block) = agent.shield_block.as_mut() {
                shield_block.can_block = can_block;
            }
            for trigger in &mut agent.avoid_rage {
                trigger.allowed = !trigger.needs_block || can_block;
            }
            // The shield casts' conditions read it.
            fight.changed(crate::core::fight::reads::CLASS);
        }
    }
    const CACHE_ROTATION: bool = true;

    fn apply_effects(
        fight: &mut Fight<Self>,
        spell: SpellId,
        target: Side,
        behavior: WarriorSpell,
    ) {
        match behavior {
            WarriorSpell::Bloodthirst => {
                let params = fight.agent.bloodthirst.expect("Bloodthirst is bound");
                bloodthirst::apply(fight, spell, target, params);
            }
            WarriorSpell::Whirlwind => {
                let params = fight.agent.whirlwind.expect("Whirlwind is bound");
                let sweeping = fight.agent.sweeping_strikes;
                whirlwind::apply(fight, spell, target, params, sweeping);
            }
            WarriorSpell::WhirlwindOffHand => {
                let params = fight.agent.whirlwind.expect("Whirlwind is bound");
                whirlwind::apply_off_hand(fight, spell, target, params);
            }
            WarriorSpell::Execute => {
                let params = fight.agent.execute.expect("Execute is bound");
                execute::apply(fight, spell, target, params);
            }
            WarriorSpell::Hamstring => {
                let base = fight.agent.hamstring;
                hamstring::apply(fight, spell, target, base);
            }
            // pummel.go: Hamstring's strike, with Pummel's base.
            WarriorSpell::Pummel => {
                let base = fight.agent.pummel;
                hamstring::apply(fight, spell, target, base);
            }
            // shield_bash.go and mocking_blow.go: Pummel's strike.
            WarriorSpell::ShieldBash => {
                let base = fight.agent.shield_bash;
                hamstring::apply(fight, spell, target, base);
            }
            WarriorSpell::MockingBlow => {
                let base = fight.agent.mocking_blow;
                hamstring::apply(fight, spell, target, base);
            }
            WarriorSpell::Disarm => {
                let aura = fight.agent.disarm.expect("Disarm is bound");
                disarm::apply(fight, spell, target, aura);
            }
            WarriorSpell::VictoryRush => {
                unreachable!("Victorious never comes up, so Victory Rush is never cast")
            }
            WarriorSpell::Intercept => {
                let mut params = fight.agent.intercept.expect("Intercept is bound");
                intercept::apply(fight, spell, target, &mut params);
                fight.agent.intercept = Some(params);
                fight.activate_aura(params.aura);
                intercept::run(fight, params);
            }
            WarriorSpell::Bloodrage => {
                let params = fight.agent.bloodrage.expect("Bloodrage is bound");
                bloodrage::apply(fight, params);
            }
            WarriorSpell::BerserkerRage => {
                let params = fight.agent.berserker_rage.expect("Berserker Rage is bound");
                berserker_rage::apply(fight, params);
            }
            WarriorSpell::DeathWish => {
                let params = fight.agent.death_wish.expect("Death Wish is bound");
                death_wish::apply(fight, params);
            }
            WarriorSpell::Recklessness => {
                let aura = fight.agent.recklessness.expect("Recklessness is bound");
                fight.activate_aura(aura);
            }
            WarriorSpell::DeepWounds => {
                let params = fight.agent.deep_wounds.expect("Deep Wounds is bound");
                let off_hand = std::mem::take(&mut fight.agent.deep_wounds_off_hand);
                deep_wounds::apply(fight, spell, target, params, off_hand);
            }
            WarriorSpell::Strike(index) => {
                // Go QueuedSwingConsumedBeforeDamage: the queue is dropped before the strike's
                // damage, so an extra attack its hit procs at once is a white swing.
                let params = fight.agent.queue.strikes[index];
                if params.cleave {
                    let results = heroic_strike::cleave_results(fight, spell, target, params);
                    Self::consume_queue(fight);
                    Self::deal_cleave(fight, spell, results);
                } else {
                    let result = heroic_strike::strike(fight, spell, target, params);
                    Self::consume_queue(fight);
                    fight.deal_damage(spell, result, false);
                }
            }
            WarriorSpell::QueueStrike(index) => {
                let mut queue = std::mem::take(&mut fight.agent.queue);
                let armed = heroic_strike::queue(fight, &mut queue, index);
                let delay = queue.realism.map_or(0, |(_, delay)| delay);
                fight.agent.queue = queue;
                if armed.is_some() {
                    let tag = QUEUE_TAG + index as u32;
                    fight.start_class_periodic(tag, delay, 1, crate::core::fight::PRIORITY_GCD);
                }
            }
            WarriorSpell::Stance(index) => {
                let cast = fight.agent.stances[index];
                let retained = fight.agent.max_retained_rage;
                fight.agent.stance = stances::change(fight, cast, retained);
                fight.versions.class += 1;
            }
            WarriorSpell::BattleShout => {
                let params = fight.agent.battle_shout.expect("Battle Shout is bound");
                battle_shout::apply(fight, spell, target, params);
            }
            WarriorSpell::Charge => {
                let params = fight.agent.charge.expect("Charge is bound");
                charge::apply(fight, spell, params);
            }
            WarriorSpell::Rend => {
                let params = fight.agent.rend.expect("Rend is bound");
                rend::apply(fight, spell, target, params);
            }
            WarriorSpell::Overpower => {
                let base = fight.agent.overpower;
                let window = fight.agent.overpower_window.expect("the window is bound");
                overpower::apply(fight, spell, target, base, window);
            }
            WarriorSpell::MortalStrike => {
                let base = fight.agent.mortal_strike;
                mortal_strike::apply(fight, spell, target, base);
            }
            WarriorSpell::SpearingStrike => {
                let params = fight
                    .agent
                    .spearing_strike
                    .expect("Spearing Strike is bound");
                spearing_strike::apply(fight, spell, target, params);
            }
            WarriorSpell::Slam => {
                let base = fight.agent.slam;
                slam::apply(fight, spell, target, base);
            }
            WarriorSpell::Revenge => {
                let params = fight.agent.revenge.expect("Revenge is bound");
                revenge::apply(fight, spell, target, params);
            }
            WarriorSpell::ShieldSlam => {
                let damage = fight.agent.shield_slam.expect("Shield Slam is bound");
                shield_slam::apply(fight, spell, target, damage);
            }
            WarriorSpell::ThunderClap => {
                let params = fight.agent.thunder_clap.expect("Thunder Clap is bound");
                let sweeping = fight.agent.sweeping_strikes;
                thunder_clap::apply(fight, spell, target, params, sweeping);
            }
            WarriorSpell::DemoralizingShout => {
                let aura = fight
                    .agent
                    .demoralizing_shout
                    .expect("Demoralizing Shout is bound");
                demoralizing_shout::apply(fight, spell, aura);
            }
            WarriorSpell::ChallengingShout => challenging_shout::apply(fight, spell),
            WarriorSpell::Taunt => taunt::apply(fight, spell, target),
            WarriorSpell::IntimidatingShout => intimidating_shout::apply(fight, spell, target),
            WarriorSpell::ConcussionBlow => concussion_blow::apply(fight, spell, target),
            WarriorSpell::Retaliation => {
                let params = fight.agent.retaliation.expect("Retaliation is bound");
                retaliation::apply(fight, params);
            }
            WarriorSpell::SweepingStrikes => {
                let params = fight
                    .agent
                    .sweeping_strikes
                    .expect("Sweeping Strikes is bound");
                sweeping_strikes::apply(fight, params);
            }
            WarriorSpell::SweepingStrikesHit => {
                let damage = fight.agent.sweeping_copy;
                sweeping_strikes::apply_hit(fight, spell, target, damage);
            }
            WarriorSpell::SweepingStrikesNormalizedHit => {
                sweeping_strikes::apply_normalized(fight, spell, target);
            }
            WarriorSpell::RetaliationHit => {
                let params = fight.agent.retaliation.expect("Retaliation is bound");
                retaliation::strike(fight, spell, target, params);
            }
            WarriorSpell::SunderArmor => {
                let params = fight.agent.sunder.expect("Sunder Armor is bound");
                sunder_armor::apply(fight, spell, target, params);
            }
            WarriorSpell::ShieldWall => {
                let aura = fight.agent.shield_wall.expect("Shield Wall is bound").aura;
                fight.activate_aura(aura);
            }
            WarriorSpell::LastStand => {
                let aura = fight.agent.last_stand.expect("Last Stand is bound").aura;
                fight.activate_aura(aura);
            }
            WarriorSpell::ShieldBlock => {
                let params = fight.agent.shield_block.expect("Shield Block is bound");
                params.apply(fight);
            }
            WarriorSpell::StanceLocked(_) | WarriorSpell::BloodCraze => {
                panic!("the gate keeps {behavior:?} from being cast")
            }
        }
    }

    fn extra_cast_condition(fight: &Fight<Self>, _spell: SpellId, behavior: WarriorSpell) -> bool {
        let stance = fight.agent.stance;
        match behavior {
            // whirlwind.go, recklessness.go and berserker_rage.go: Berserker Stance.
            WarriorSpell::Whirlwind
            | WarriorSpell::Recklessness
            | WarriorSpell::BerserkerRage
            | WarriorSpell::Pummel
            | WarriorSpell::Intercept => stance == Stance::Berserker,
            // shield_bash.go: a shield, in Defensive or Battle Stance.
            WarriorSpell::ShieldBash => {
                fight.agent.can_block && matches!(stance, Stance::Defensive | Stance::Battle)
            }
            // mocking_blow.go: Battle Stance.
            WarriorSpell::MockingBlow => stance == Stance::Battle,
            // disarm.go: Defensive Stance.
            WarriorSpell::Disarm => stance == Stance::Defensive,
            // victory_rush.go: Victorious up.
            WarriorSpell::VictoryRush => fight
                .agent
                .victorious
                .is_some_and(|victorious| fight.aura(victorious).active),
            // execute.go: Berserker or Battle Stance, in the execute phase.
            WarriorSpell::Execute => {
                matches!(stance, Stance::Berserker | Stance::Battle) && fight.is_execute_phase_20()
            }
            // hamstring.go: Battle or Berserker Stance.
            WarriorSpell::Hamstring => matches!(stance, Stance::Berserker | Stance::Battle),
            // sunder_armor.go CanApplySunderAura: the warrior's own stacks, or an empty category.
            WarriorSpell::SunderArmor => {
                !fight.agent.sunder_blocked
                    && fight
                        .agent
                        .sunder
                        .is_some_and(|sunder| sunder_armor::condition(fight, sunder))
            }
            WarriorSpell::StanceLocked(lock) => lock.allows(stance),
            // shield_wall.go: Defensive Stance and a shield.
            WarriorSpell::ShieldWall => {
                stance == Stance::Defensive
                    && fight
                        .agent
                        .shield_wall
                        .is_some_and(|shield_wall| shield_wall.can_block)
            }
            // shield_block.go: a shield and Defensive Stance.
            WarriorSpell::ShieldBlock => {
                fight
                    .agent
                    .shield_block
                    .is_some_and(|shield_block| shield_block.can_block)
                    && stance == Stance::Defensive
            }
            // stances.go: a stance cast needs another stance.
            WarriorSpell::Stance(index) => fight.agent.stances[index].stance != stance,
            WarriorSpell::BattleShout => {
                battle_shout::condition(fight, fight.agent.battle_shout.expect("bound"))
            }
            WarriorSpell::Charge => {
                charge::condition(fight, fight.agent.charge.expect("bound"), stance)
            }
            // rend.go: Battle or Defensive Stance.
            WarriorSpell::Rend => matches!(stance, Stance::Battle | Stance::Defensive),
            // overpower.go: Battle Stance with the window open.
            WarriorSpell::Overpower => {
                stance == Stance::Battle
                    && fight
                        .agent
                        .overpower_window
                        .is_some_and(|window| fight.aura(window).active)
            }
            // talents_arms.go: Spearing Strike requires Battle Stance.
            WarriorSpell::SpearingStrike => stance == Stance::Battle,
            // revenge.go: Defensive Stance with Revenge up.
            WarriorSpell::Revenge => {
                stance == Stance::Defensive
                    && fight
                        .agent
                        .revenge
                        .is_some_and(|revenge| fight.aura(revenge.aura).active)
            }
            // talents_protection.go: Shield Slam needs a shield.
            WarriorSpell::ShieldSlam => fight.agent.can_block,
            // thunder_clap.go: Battle or Defensive Stance.
            WarriorSpell::ThunderClap => matches!(stance, Stance::Battle | Stance::Defensive),
            // taunt.go: Defensive Stance.
            WarriorSpell::Taunt => stance == Stance::Defensive,
            // retaliation.go: Battle Stance.
            WarriorSpell::Retaliation => stance == Stance::Battle,
            // talents_arms.go: Sweeping Strikes requires Battle Stance.
            WarriorSpell::SweepingStrikes => stance == Stance::Battle,
            WarriorSpell::QueueStrike(index) => {
                heroic_strike::queue_condition(fight, &fight.agent.queue, index)
            }
            _ => true,
        }
    }

    fn extra_condition_proof(
        fight: &Fight<Self>,
        _spell: SpellId,
        behavior: WarriorSpell,
    ) -> Option<(i64, u8)> {
        use crate::core::{fight::reads, time::NEVER_EXPIRES};
        Some(match behavior {
            // The stance, and what never changes in a fight.
            WarriorSpell::Whirlwind
            | WarriorSpell::Recklessness
            | WarriorSpell::BerserkerRage
            | WarriorSpell::Hamstring
            | WarriorSpell::Pummel
            | WarriorSpell::StanceLocked(_)
            | WarriorSpell::ShieldWall
            | WarriorSpell::ShieldBlock
            | WarriorSpell::Stance(_)
            | WarriorSpell::Charge
            | WarriorSpell::Rend
            | WarriorSpell::SpearingStrike
            | WarriorSpell::ShieldSlam
            | WarriorSpell::ThunderClap
            | WarriorSpell::Taunt
            | WarriorSpell::Retaliation
            | WarriorSpell::SweepingStrikes => (NEVER_EXPIRES, reads::CLASS),
            WarriorSpell::Execute => (NEVER_EXPIRES, reads::CLASS | reads::PHASE),
            // The warrior's stacks, or the category's active member.
            WarriorSpell::SunderArmor => (NEVER_EXPIRES, reads::AURAS),
            WarriorSpell::Overpower | WarriorSpell::Revenge => {
                (NEVER_EXPIRES, reads::CLASS | reads::AURAS)
            }
            // The category's member; the warrior's own shout passes once its remaining time
            // falls to the threshold.
            WarriorSpell::BattleShout => {
                let params = fight.agent.battle_shout.expect("Battle Shout is bound");
                let until = match fight.exclusive_active(params.category) {
                    Some((aura, _)) if aura == params.aura => {
                        let expires = fight.aura(aura).expires;
                        if expires == NEVER_EXPIRES {
                            NEVER_EXPIRES
                        } else {
                            expires - params.refresh_threshold
                        }
                    }
                    _ => NEVER_EXPIRES,
                };
                (until, reads::AURAS)
            }
            // The queue, the rage and its cost, and the hardcast and realism delay running out.
            WarriorSpell::QueueStrike(_) => {
                let mut until = NEVER_EXPIRES;
                let hardcast = fight.player.hardcast.expires;
                if hardcast > fight.now {
                    until = until.min(hardcast);
                }
                if let Some((timer, _)) = fight.agent.queue.realism {
                    let ready = fight.timers[timer];
                    if ready > fight.now {
                        until = until.min(ready);
                    }
                }
                (
                    until,
                    reads::CLASS | reads::RESOURCES | reads::SPELLS | reads::CASTS | reads::TIMERS,
                )
            }
            _ => return None,
        })
    }

    fn modify_cast(fight: &mut Fight<Self>, spell: SpellId, behavior: WarriorSpell) {
        if behavior == WarriorSpell::Slam {
            let stops_swings = fight.agent.slam_stops_swings;
            slam::modify_cast(fight, spell, stops_swings);
        }
    }

    fn should_activate(fight: &Fight<Self>, _spell: SpellId, behavior: WarriorSpell) -> bool {
        match behavior {
            WarriorSpell::Bloodrage => {
                bloodrage::should_activate(fight, fight.agent.bloodrage.expect("bound"))
            }
            WarriorSpell::BerserkerRage => {
                berserker_rage::should_activate(fight, fight.agent.berserker_rage.expect("bound"))
            }
            // shield_wall.go: a tank below the health share; never a DPS warrior.
            WarriorSpell::ShieldWall => {
                let shield_wall = fight.agent.shield_wall.expect("Shield Wall is bound");
                shield_wall.autocast
                    && fight.player.health / fight.player_max_health() < shield_wall.health_percent
            }
            // retaliation.go and shield_wall.go: manual use only for a DPS warrior.
            WarriorSpell::StanceLocked(StanceLock::Battle | StanceLock::Defensive) => false,
            WarriorSpell::Retaliation => false,
            // talents_arms.go registerSweepingStrikes: it strikes "an additional nearby opponent",
            // so with one target it would spend 30 rage for nothing.
            WarriorSpell::SweepingStrikes => fight.targets.len() >= 2,
            _ => true,
        }
    }

    /// Every rule above reads only rage, health, the targets or the class's settings, which
    /// change only with what a cached look reads, so a refusal holds while the look does.
    fn should_activate_until(
        _fight: &Fight<Self>,
        _spell: SpellId,
        _behavior: WarriorSpell,
    ) -> Option<i64> {
        Some(crate::core::time::NEVER_EXPIRES)
    }

    fn replace_mh_swing(fight: &mut Fight<Self>, swing: SpellId) -> SpellId {
        // heroic_strike_cleave.go TryHSOrCleave.
        let Some(current) = fight.agent.queue.current else {
            return swing;
        };
        let strike = fight.agent.queue.strikes[current];
        if !fight.aura(strike.queue_aura).active {
            return swing;
        }
        if !fight.can_cast(strike.spell) {
            fight.deactivate_aura(strike.queue_aura);
            return swing;
        }
        strike.spell
    }

    fn dealing_result(fight: &Fight<Self>, spell: SpellId) -> Option<SpellResult> {
        fight.agent.queue.dealing_result(spell)
    }

    fn clone_result(
        fight: &mut Fight<Self>,
        spell: SpellId,
        result: &SpellResult,
    ) -> Option<(SpellResult, usize)> {
        fight.agent.queue.clone_result(spell, result)
    }

    fn dispose_clone(fight: &mut Fight<Self>, token: usize) {
        fight.agent.queue.dispose_clone(token);
    }

    fn on_periodic(fight: &mut Fight<Self>, tag: u32) {
        match tag {
            anger_management::PERIODIC_TAG => {
                let params = fight
                    .agent
                    .anger_management
                    .expect("Anger Management is bound");
                anger_management::tick(fight, params);
            }
            bloodrage::PERIODIC_TAG => {
                let params = fight.agent.bloodrage.expect("Bloodrage is bound");
                bloodrage::tick(fight, params);
            }
            enrage::PERIODIC_TAG => {
                let params = fight.agent.enrage.expect("Enrage is bound");
                enrage::enrage(fight, params);
            }
            blood_craze::PERIODIC_TAG => {
                let params = fight.agent.blood_craze.expect("Blood Craze is bound");
                blood_craze::apply(fight, params);
            }
            improved_hamstring::PERIODIC_TAG => {
                let params = fight
                    .agent
                    .improved_hamstring
                    .expect("Improved Hamstring is bound");
                improved_hamstring::root(fight, params);
            }
            tag if tag >= QUEUE_TAG => {
                let index = (tag - QUEUE_TAG) as usize;
                let aura = fight.agent.queue.strikes[index].queue_aura;
                fight.activate_aura(aura);
                fight.agent.queue.queued[index] = false;
                fight.versions.class += 1;
            }
            _ => {}
        }
    }

    fn on_dot_tick(fight: &mut Fight<Self>, dot: DotId, behavior: WarriorSpell) {
        match behavior {
            WarriorSpell::DeepWounds => {
                let params = fight.agent.deep_wounds.expect("Deep Wounds is bound");
                deep_wounds::tick(fight, dot, params.tick_can_crit);
            }
            WarriorSpell::Rend => {
                let params = fight.agent.rend.expect("Rend is bound");
                rend::tick(fight, dot, params);
            }
            WarriorSpell::BloodCraze => {
                let params = fight.agent.blood_craze.expect("Blood Craze is bound");
                blood_craze::tick(fight, dot, params);
            }
            _ => {}
        }
    }

    fn on_movement(fight: &mut Fight<Self>, side: Side, kind: MovementKind) {
        if let (Side::Player, Some(params)) = (side, fight.agent.charge) {
            charge::on_movement(fight, params, kind);
        }
        if let (Side::Player, Some(params)) = (side, fight.agent.intercept) {
            intercept::on_movement(fight, params, kind);
        }
    }

    fn reset(fight: &mut Fight<Self>) {
        // Go Warrior.Reset: the default stance.
        fight.agent.stance = fight.agent.default_stance;
        if let Some(params) = fight.agent.anger_management {
            anger_management::reset(fight, params);
        }
        let queue = &mut fight.agent.queue;
        queue.current = None;
        queue.queued.iter_mut().for_each(|queued| *queued = false);
        fight.versions.class += 1;
    }

    fn on_gain(fight: &mut Fight<Self>, aura: AuraRef, kind: WarriorAura) {
        match kind {
            WarriorAura::LastStand => {
                let last_stand = fight.agent.last_stand.expect("Last Stand is bound");
                fight.agent.last_stand = Some(last_stand.on_gain(fight));
            }
            WarriorAura::ShieldBlock => fight
                .agent
                .shield_block
                .expect("Shield Block is bound")
                .toggle(fight, true),
            WarriorAura::Flurry => flurry::on_gain(fight, fight.agent.flurry.expect("bound")),
            WarriorAura::DeathWish => {
                death_wish::on_gain(fight, fight.agent.death_wish.expect("bound"))
            }
            WarriorAura::BerserkerRage => berserker_rage::on_gain(fight),
            WarriorAura::Dash => charge::on_gain(fight, fight.agent.charge.expect("bound")),
            WarriorAura::InterceptDash => {
                intercept::on_gain(fight, fight.agent.intercept.expect("bound"))
            }
            WarriorAura::Enrage => {
                let params = fight.agent.enrage.expect("Enrage is bound");
                fight.activate_mod(params.damage_mod);
            }
            WarriorAura::Queue(index) => {
                if let Some(current) = fight.agent.queue.current {
                    let current = fight.agent.queue.strikes[current].queue_aura;
                    fight.deactivate_aura(current);
                }
                let _ = aura;
                fight.agent.queue.current = Some(index);
                fight.versions.class += 1;
            }
            _ => {}
        }
    }

    fn on_expire(fight: &mut Fight<Self>, _aura: AuraRef, kind: WarriorAura) {
        match kind {
            WarriorAura::LastStand => fight
                .agent
                .last_stand
                .expect("Last Stand is bound")
                .on_expire(fight),
            WarriorAura::ShieldBlock => fight
                .agent
                .shield_block
                .expect("Shield Block is bound")
                .toggle(fight, false),
            WarriorAura::Flurry => flurry::on_expire(fight, fight.agent.flurry.expect("bound")),
            WarriorAura::DeathWish => {
                death_wish::on_expire(fight, fight.agent.death_wish.expect("bound"))
            }
            WarriorAura::BerserkerRage => berserker_rage::on_expire(fight),
            WarriorAura::Dash => charge::on_expire(fight, fight.agent.charge.expect("bound")),
            WarriorAura::InterceptDash => {
                intercept::on_expire(fight, fight.agent.intercept.expect("bound"))
            }
            WarriorAura::Queue(_) => {
                fight.agent.queue.current = None;
                fight.versions.class += 1;
            }
            WarriorAura::Enrage => {
                let params = fight.agent.enrage.expect("Enrage is bound");
                fight.deactivate_mod(params.damage_mod);
            }
            _ => {}
        }
    }

    fn on_enemy_hit_taken(
        fight: &mut Fight<Self>,
        aura: AuraRef,
        kind: WarriorAura,
        result: &SpellResult,
    ) {
        if kind == WarriorAura::MightRage {
            // The target's swing has no spell of the player's; the delayed handler reads none.
            let params = fight.agent.might_rage.clone().expect("bound");
            params.on_hit_taken(fight, aura, 0, result);
            return;
        }
        // The target's swing is a melee auto attack without the proc flag.
        Self::hit_taken(fight, kind, result, false, true);
    }

    /// A player spell that hits the player, as the Goblin Sapper Charge's half on the player:
    /// the warrior's listeners of hits taken hear it as they hear the target's swings.
    fn on_spell_hit_taken(
        fight: &mut Fight<Self>,
        aura: AuraRef,
        kind: WarriorAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        if aura.side != Side::Player {
            return;
        }
        let state = &fight.spells[spell];
        let (proc, melee) = (state.flags.proc, state.melee_proc);
        if kind == WarriorAura::MightRage {
            // Go's proc trigger skips a spell flagged a proc.
            if !proc {
                let params = fight.agent.might_rage.clone().expect("bound");
                params.on_hit_taken(fight, aura, spell, result);
            }
            return;
        }
        Self::hit_taken(fight, kind, result, proc, melee);
    }

    fn on_spell_hit_dealt(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: WarriorAura,
        spell: SpellId,
        result: &SpellResult,
    ) {
        match kind {
            WarriorAura::DeepWoundsTrigger => {
                let params = fight.agent.deep_wounds.expect("Deep Wounds is bound");
                let empty = fight.agent.empty_mask[spell];
                fight.agent.deep_wounds_off_hand =
                    fight.agent.off_hand_mask[spell] && fight.autos.dual_wielding;
                deep_wounds::on_hit(fight, params, spell, empty, result);
                fight.agent.deep_wounds_off_hand = false;
            }
            WarriorAura::UnbridledWrath => {
                let params = fight.agent.unbridled_wrath.expect("bound");
                unbridled_wrath::on_hit(fight, params, spell, result);
            }
            WarriorAura::FlurryTrigger => {
                let params = fight.agent.flurry.expect("Flurry is bound");
                flurry::on_hit(fight, params, spell, result);
            }
            WarriorAura::BloodthrillTrigger => {
                let params = fight.agent.bloodthrill.expect("Bloodthrill is bound");
                // Go: the trigger's ProcMaskMeleeMH, narrowed to white swings, Heroic Strike and Cleave
                // (patch 176, from upstream's combat logs).
                let main_hand =
                    fight.agent.main_hand_mask[spell] && fight.agent.bloodthrill_swing[spell];
                bloodthrill::on_hit(fight, params, spell, main_hand, result);
            }
            WarriorAura::WeaponmasterSword => {
                let params = fight.agent.weaponmaster.expect("Weaponmaster is bound");
                let sword = fight.agent.sword_mask[spell];
                weaponmaster::on_hit(fight, params, spell, sword, result);
            }
            WarriorAura::SweepingStrikes => {
                let params = fight.agent.sweeping_strikes.expect("bound");
                if let Some(copy) = sweeping_strikes::on_hit(fight, params, spell, result) {
                    fight.agent.sweeping_copy = copy;
                    sweeping_strikes::copy(fight, params, result.target);
                }
            }
            WarriorAura::OverpowerTrigger => {
                let window = fight.agent.overpower_window.expect("bound");
                overpower::on_hit(fight, window, spell, result);
            }
            WarriorAura::ImprovedHamstringTrigger
                if fight.spells[spell].class_spell.as_deref() == Some("hamstring") =>
            {
                let params = fight.agent.improved_hamstring.expect("bound");
                improved_hamstring::on_hamstring(fight, params, result);
            }
            _ => {}
        }
    }

    fn on_delayed_proc(
        fight: &mut Fight<Self>,
        _aura: AuraRef,
        kind: WarriorAura,
        _spell: SpellId,
        _result: SpellResult,
    ) {
        match kind {
            WarriorAura::UnbridledWrath => {
                let params = fight.agent.unbridled_wrath.expect("bound");
                unbridled_wrath::grant(fight, params);
            }
            WarriorAura::BloodthrillTrigger => {
                let params = fight.agent.bloodthrill.expect("bound");
                bloodthrill::open(fight, params);
            }
            WarriorAura::MightRage => {
                let params = fight.agent.might_rage.clone().expect("bound");
                params.grant(fight);
            }
            _ => {}
        }
    }
}
