//! Rogue preparation: Go sim/rogue's construction and initialization, and the exporter's Rogue
//! description (tools/oracle-v2/rogue.go and rogue_specs.go).

mod armor;
mod export;
mod export_specs;
pub(crate) mod items;
pub(crate) mod masks;
mod poisons;
pub(crate) mod rework;
mod spell_data;
mod spells;
mod talents;
mod util;

use std::cell::Cell;
use std::rc::Rc;

use crate::contracts::request::Message;
use crate::prepare::agent::{fill_talents, ClassSpellName, PrepAgent};
use crate::prepare::attack::AutoAttackOptions;
use crate::prepare::character::constants::DODGE_RATING_PER_DODGE_PERCENT;
use crate::prepare::energy::EnergyBarOptions;
use crate::prepare::env::{Environment, FinalizeEffect};
use crate::prepare::racials::EurekaSpells;
use crate::prepare::sim::{AuraId, Duration, Rules, Sim, SpellId, UnitId, UnitType, SECOND};
use crate::prepare::stats::Stat;
use crate::prepare::talent_layout::{self, new_talent, talent, Extras, LayoutTalent};
use crate::prepare::Refusal;

use spell_data::spell_data;

/// Go sim/rogue `TalentTreeSizes`.
const TALENT_TREE_SIZES: [usize; 3] = [17, 17, 19];

/// The Rogue trees of the hotfix push 113483, which only Forever's rules read: each tree's
/// talents in the client's order (row, then column), with upstream's proto field names
/// (wowsims/forever #840), from the TraitNodes of the store and Wowhead's talent calculator.
/// Murder, Quietus, Cutthroat, Thousand Cuts and Improved Distract are gone. Blood Splatter
/// (1329256), Composure (1329265), Flash of Steel (1329284), Boot Knife (1329286), Death Mark
/// (1316709) and Quick Getaway (1310728, the id Quietus had) are new, so the pinned proto has no
/// field for them and they are read into the extras. Relentless Strikes has five ranks, where
/// the proto's field is a bool, so it is read into the extras too. Improved Eviscerate is
/// renamed Coup de Grace and Improved Sinister Strike Efficient Strikes (same proto fields).
const FOREVER_ASSASSINATION: &[LayoutTalent] = &[
    talent("improved_gouge", "Improved Gouge", 3),
    talent("remorseless_attacks", "Remorseless Attacks", 2),
    talent("malice", "Malice", 5),
    talent("ruthlessness", "Ruthlessness", 3),
    talent("improved_expose_armor", "Improved Expose Armor", 2),
    talent("improved_slice_and_dice", "Improved Slice and Dice", 3),
    talent("vigor", "Vigor", 2),
    new_talent("blood_splatter", "Blood Splatter", 1),
    talent("lethality", "Lethality", 2),
    talent("vile_poisons", "Vile Poisons", 5),
    talent("cold_blood", "Cold Blood", 1),
    talent("improved_poisons", "Improved Poisons", 5),
    new_talent("relentless_strikes", "Relentless Strikes", 5),
    talent("venom", "Venom", 1),
    talent("improved_kidney_shot", "Improved Kidney Shot", 2),
    talent("seal_fate", "Seal Fate", 3),
    talent("mutilate", "Mutilate", 1),
];

const FOREVER_COMBAT: &[LayoutTalent] = &[
    talent("improved_eviscerate", "Coup de Grace", 3),
    talent("improved_sinister_strike", "Efficient Strikes", 2),
    talent("lightning_reflexes", "Lightning Reflexes", 5),
    talent("puncturing_wounds", "Puncturing Wounds", 3),
    talent("deflection", "Deflection", 3),
    talent("precision", "Precision", 3),
    talent("improved_sprint", "Improved Sprint", 2),
    talent("endurance", "Endurance", 2),
    talent("riposte", "Riposte", 1),
    new_talent("composure", "Composure", 2),
    talent("improved_kick", "Improved Kick", 2),
    talent("flawless_execution", "Flawless Execution", 1),
    talent("weapon_expertise", "Weapon Expertise", 2),
    talent("aggression", "Aggression", 3),
    talent("dual_wield_specialization", "Dual Wield Specialization", 2),
    talent("blade_flurry", "Blade Flurry", 1),
    talent("hack_and_slash", "Hack and Slash", 5),
    new_talent("flash_of_steel", "Flash of Steel", 5),
    talent("adrenaline_rush", "Adrenaline Rush", 1),
];

const FOREVER_SUBTLETY: &[LayoutTalent] = &[
    talent("setup", "Setup", 3),
    talent("master_of_deception", "Master of Deception", 3),
    talent("opportunity", "Opportunity", 5),
    talent("dirty_deeds", "Dirty Deeds", 2),
    talent("camouflage", "Camouflage", 3),
    talent("improved_ambush", "Improved Ambush", 3),
    talent("dirty_tricks", "Dirty Tricks", 2),
    talent("initiative", "Initiative", 3),
    talent("premeditation", "Premeditation", 1),
    talent("serrated_blades", "Serrated Blades", 3),
    talent("elusiveness", "Elusiveness", 2),
    talent("ghostly_strike", "Ghostly Strike", 1),
    new_talent("quick_getaway", "Quick Getaway", 2),
    talent("heightened_senses", "Heightened Senses", 2),
    talent("preparation", "Preparation", 1),
    talent("hemorrhage", "Hemorrhage", 1),
    new_talent("boot_knife", "Boot Knife", 5),
    new_talent("death_mark", "Death Mark", 1),
];

pub(crate) const FOREVER_TREES: [&[LayoutTalent]; 3] =
    [FOREVER_ASSASSINATION, FOREVER_COMBAT, FOREVER_SUBTLETY];

/// tools/oracle-v2/rogue.go `rogueClassSpells`.
static CLASS_SPELLS: &[ClassSpellName] = &[
    ClassSpellName {
        mask: masks::AMBUSH,
        name: "ambush",
    },
    ClassSpellName {
        mask: masks::BACKSTAB,
        name: "backstab",
    },
    ClassSpellName {
        mask: masks::EVISCERATE,
        name: "eviscerate",
    },
    ClassSpellName {
        mask: masks::EXPOSE_ARMOR,
        name: "expose_armor",
    },
    ClassSpellName {
        mask: masks::FEINT,
        name: "feint",
    },
    ClassSpellName {
        mask: masks::GARROTE,
        name: "garrote",
    },
    ClassSpellName {
        mask: masks::GOUGE,
        name: "gouge",
    },
    ClassSpellName {
        mask: masks::RUPTURE,
        name: "rupture",
    },
    ClassSpellName {
        mask: masks::SINISTER_STRIKE,
        name: "sinister_strike",
    },
    ClassSpellName {
        mask: masks::SLICE_AND_DICE,
        name: "slice_and_dice",
    },
    ClassSpellName {
        mask: masks::STEALTH,
        name: "stealth",
    },
    ClassSpellName {
        mask: masks::VANISH,
        name: "vanish",
    },
    ClassSpellName {
        mask: masks::HEMORRHAGE,
        name: "hemorrhage",
    },
    ClassSpellName {
        mask: masks::PREMEDITATION,
        name: "premeditation",
    },
    ClassSpellName {
        mask: masks::PREPARATION,
        name: "preparation",
    },
    ClassSpellName {
        mask: masks::SHADOWSTEP,
        name: "shadowstep",
    },
    ClassSpellName {
        mask: masks::ADRENALINE_RUSH,
        name: "adrenaline_rush",
    },
    ClassSpellName {
        mask: masks::BLADE_FLURRY,
        name: "blade_flurry",
    },
    ClassSpellName {
        mask: masks::COLD_BLOOD,
        name: "cold_blood",
    },
    ClassSpellName {
        mask: masks::MUTILATE,
        name: "mutilate",
    },
    ClassSpellName {
        mask: masks::MUTILATE_HIT,
        name: "mutilate_hit",
    },
    ClassSpellName {
        mask: masks::GHOSTLY_STRIKE,
        name: "ghostly_strike",
    },
    ClassSpellName {
        mask: masks::INSTANT_POISON,
        name: "instant_poison",
    },
    ClassSpellName {
        mask: masks::WOUND_POISON,
        name: "wound_poison",
    },
    ClassSpellName {
        mask: masks::DEADLY_POISON,
        name: "deadly_poison",
    },
    ClassSpellName {
        mask: masks::VENOM,
        name: "venom",
    },
    ClassSpellName {
        mask: masks::RIPOSTE,
        name: "riposte",
    },
    ClassSpellName {
        mask: masks::KIDNEY_SHOT,
        name: "kidney_shot",
    },
    ClassSpellName {
        mask: masks::SHANK,
        name: "shank",
    },
    ClassSpellName {
        mask: masks::DEATH_MARK,
        name: "death_mark",
    },
];

/// Go `sliceAndDiceDurations`: Slice and Dice and Venom share this ladder, by combo points.
pub(super) const SLICE_AND_DICE_DURATIONS: [Duration; 6] = [
    0,
    9 * SECOND,
    12 * SECOND,
    15 * SECOND,
    18 * SECOND,
    21 * SECOND,
];

/// The Rogue fields Go's closures and item sets change while a character is built and reset:
/// the closures only see the simulation, so the agent shares them with each one it builds.
#[derive(Default)]
pub(super) struct RogueState {
    /// Go `SliceAndDiceBonusFlat`.
    pub slice_and_dice_bonus_flat: Cell<f64>,
    /// Go `SliceAndDiceBonusDuration`.
    pub slice_and_dice_bonus_duration: Cell<Duration>,
    /// Go `DeathmantleBonus`.
    pub deathmantle_bonus: Cell<f64>,
    /// Go `HasPvpEnergy`.
    pub has_pvp_energy: Cell<bool>,
    /// Go `additivePoisonBonusChance`.
    pub additive_poison_bonus_chance: Cell<f64>,
}

/// The spells Go keeps on the Rogue, which the exporter and the registrations read.
#[derive(Clone, Default)]
pub(super) struct RogueSpells {
    pub ambush: Option<SpellId>,
    pub backstab: Option<SpellId>,
    pub deadly_poison: Option<SpellId>,
    pub wound_poison: Option<SpellId>,
    pub instant_poison: Option<SpellId>,
    pub eviscerate: Option<SpellId>,
    pub expose_armor: Option<SpellId>,
    pub garrote: Option<SpellId>,
    pub rupture: Option<SpellId>,
    pub sinister_strike: Option<SpellId>,
    pub slice_and_dice: Option<SpellId>,
    pub vanish: Option<SpellId>,
    pub blade_flurry: Option<SpellId>,
    pub adrenaline_rush: Option<SpellId>,
    pub cold_blood: Option<SpellId>,
    pub ghostly_strike: Option<SpellId>,
    pub hemorrhage: Option<SpellId>,
    pub mutilate: Option<SpellId>,
    pub preparation: Option<SpellId>,
    pub premeditation: Option<SpellId>,
    pub shank: Option<SpellId>,
    pub death_mark: Option<SpellId>,
    pub venom: Option<SpellId>,
}

/// The auras Go keeps on the Rogue. An aura array is by unit index, with the enemies' auras.
#[derive(Clone, Default)]
pub(super) struct RogueAuras {
    pub adrenaline_rush: Option<AuraId>,
    pub blade_flurry: Option<AuraId>,
    pub cutthroat: Option<AuraId>,
    pub death_mark: Option<AuraId>,
    /// Ghostly Strike's buff, Forever's rules: the next Ambush or Garrote needs no Stealth.
    pub ghostly_ready: Option<AuraId>,
    pub expose_armor: Vec<Option<AuraId>>,
    pub hemorrhage: Vec<Option<AuraId>>,
    pub poised: Option<AuraId>,
    pub slice_and_dice: Option<AuraId>,
    pub stealth: Option<AuraId>,
    pub thousand_cuts: Option<AuraId>,
    pub venom: Option<AuraId>,
    pub wound_poison_debuff: Vec<Option<AuraId>>,
}

/// Go `Rogue`: the talents, the registered spells and auras, and what the exporter reads.
#[derive(Clone)]
pub(crate) struct Rogue {
    talents: Message,
    /// The talents Forever's layout has that the proto has no field for, and Relentless Strikes'
    /// ranks: points by field.
    extras: Extras,
    state: Rc<RogueState>,
    spells: RogueSpells,
    auras: RogueAuras,
    /// Go `ruthlessnessChance`.
    ruthlessness_chance: f64,
    /// The `RegisterPostFinalizeEffect` calls of the talents.
    post_finalize: Vec<FinalizeEffect>,
}

/// Go `NewRogue`.
pub(crate) fn new_rogue(
    sim: &mut Sim,
    unit: UnitId,
    player: &Message,
) -> Result<Box<dyn PrepAgent>, Refusal> {
    // Go reads options.GetRogue().Options.ClassOptions without a check.
    player
        .message("rogue")
        .and_then(|rogue| rogue.message("options"))
        .ok_or_else(|| Refusal::new("class_option", "a rogue without its options".to_string()))?;
    let talents_string = player.str("talents_string");
    let (talents, extras) = match sim.rules {
        Rules::Forever => talent_layout::fill(
            "proto.RogueTalents",
            "rogue",
            talents_string,
            &FOREVER_TREES,
        ),
        Rules::GoReference => fill_talents("proto.RogueTalents", talents_string, TALENT_TREE_SIZES)
            .map(|talents| (talents, Extras::new())),
    }
    .map_err(|err| Refusal::new("talents", err))?;
    let state = Rc::new(RogueState::default());

    // Passive rogue threat reduction: https://wotlk.wowhead.com/spell=21184/rogue-passive-dnd
    let pseudo = &mut sim.unit_mut(unit).pseudo_stats;
    pseudo.threat_multiplier *= 0.71;
    pseudo.can_parry = true;

    let mut max_energy = 100.0;
    // Forever expands Vigor from 1 rank to 2; the client states 5 then 10 extra energy.
    max_energy += spell_data().vigor.value_at(talents.i32("vigor"));
    if state.has_pvp_energy.get() {
        max_energy += 10.0;
    }

    sim.enable_energy_bar(
        unit,
        EnergyBarOptions {
            max_combo_points: 5,
            max_energy,
            has_no_regen: false,
        },
    );

    let options = AutoAttackOptions {
        main_hand: util::weapon_from_main_hand(sim, unit),
        off_hand: util::weapon_from_off_hand(sim, unit),
        auto_swing_melee: true,
        ..AutoAttackOptions::default()
    };
    sim.enable_auto_attacks(unit, options);

    poisons::apply_poisons(sim, unit);

    let crit_per_agi = sim.crit_per_agi_max_level(unit);
    let sdm = &mut sim.unit_mut(unit).sdm;
    sdm.add_stat_dependency(Stat::Strength, Stat::AttackPower, 1.0);
    sdm.add_stat_dependency(Stat::Agility, Stat::AttackPower, 1.0);
    sdm.add_stat_dependency(Stat::Agility, Stat::PhysicalCritPercent, crit_per_agi);
    // Classic's rogue dodges at twice its crit rate per agility (14.5 agility a dodge at 60).
    sdm.add_stat_dependency(
        Stat::Agility,
        Stat::DodgeRating,
        2.0 * crit_per_agi * DODGE_RATING_PER_DODGE_PERCENT,
    );

    Ok(Box::new(Rogue {
        talents,
        extras,
        state,
        spells: RogueSpells::default(),
        auras: RogueAuras::default(),
        ruthlessness_chance: 0.0,
        post_finalize: Vec::new(),
    }))
}

impl Rogue {
    /// The points in a talent Forever's layout reads into the extras (see
    /// [`FOREVER_ASSASSINATION`]); 0 under the reference, which never sets them.
    pub(super) fn extra_points(&self, field: &str) -> i32 {
        self.extras.get(field).copied().unwrap_or(0)
    }
}

impl PrepAgent for Rogue {
    fn apply_talents(&mut self, sim: &mut Sim, unit: UnitId) {
        self.register_assassination_talents(sim, unit);
        self.register_combat_talents(sim, unit);
        self.register_subtlety_talents(sim, unit);
    }

    /// Go `Rogue.Initialize`.
    fn initialize(&mut self, sim: &mut Sim, unit: UnitId) {
        self.register_ambush_spell(sim, unit);
        self.register_backstab_spell(sim, unit);
        self.register_eviscerate(sim, unit);
        self.register_expose_armor_spell(sim, unit);
        self.register_kidney_shot(sim, unit);
        self.register_garrote(sim, unit);
        self.register_deadly_poison_spell(sim, unit);
        self.register_instant_poison_spell(sim, unit);
        self.register_wound_poison_spell(sim, unit);
        self.register_rupture(sim, unit);
        self.register_sinister_strike_spell(sim, unit);
        self.register_slice_and_dice(sim, unit);
        self.register_vanish_spell(sim, unit);
        self.register_stealth_aura(sim, unit);
        // The notes of 2 November 2026 add Shank and Poised to Forever's Rogue.
        self.register_shank_spell(sim, unit);
        self.register_poised_aura(sim, unit);

        // Forever states a flat SpellAuraOptions.ProcChance of 100 on the talent spell and puts
        // the real per-rank chance on the effect, so the row's ProcChance would read 100% at
        // every rank.
        self.ruthlessness_chance = spell_data()
            .ruthlessness
            .fraction_at(self.talents.i32("ruthlessness"));
    }

    /// Go `Rogue.Reset`: the major cooldowns are disabled for the fight, which the initial
    /// list the export reads is not, and the energy regeneration multiplier follows the
    /// additive bonus, which nothing raises.
    fn reset(&mut self, sim: &mut Sim, unit: UnitId) {
        sim.multiply_energy_regen_speed(unit, 1.0);
    }

    fn take_post_finalize_effects(&mut self) -> Vec<FinalizeEffect> {
        std::mem::take(&mut self.post_finalize)
    }

    fn apply_item_effect(&mut self, sim: &mut Sim, unit: UnitId, item: i32) -> bool {
        items::apply_item_effect(sim, unit, item)
    }

    fn talents(&self) -> &Message {
        &self.talents
    }

    fn class_spells(&self) -> &'static [ClassSpellName] {
        CLASS_SPELLS
    }

    /// The Rogue's effects read the environment: `export_effects`.
    fn effects(&self, _sim: &Sim, _unit: UnitId) -> Vec<serde_json::Value> {
        Vec::new()
    }

    fn export_effects(
        &self,
        env: &Environment,
        unrepresented: &mut Vec<String>,
    ) -> Vec<serde_json::Value> {
        export::class_effects(self, env, unrepresented)
    }

    /// tools/oracle-v2/rogue.go `rogueStatAuras`: Ghostly Strike's buff adds dodge rating through
    /// `AttachStatBuff`, which only the swings of a target that tanks the player read.
    fn stat_auras(&self, sim: &Sim, unit: UnitId) -> Vec<String> {
        let Some(target) = sim
            .all_units()
            .into_iter()
            .find(|target| sim.unit(*target).unit_type == UnitType::Enemy)
        else {
            return Vec::new();
        };
        let target = sim.unit(target);
        if target.current_target != Some(unit) && target.secondary_target != Some(unit) {
            return Vec::new();
        }
        sim.get_aura(unit, "Ghostly Strike Buff")
            .map(|aura| vec![sim.aura(aura).label.clone()])
            .unwrap_or_default()
    }

    /// The item set bonuses change the Rogue's state, as Go's hand them the agent.
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    /// Go `Rogue.EurekaSpells`: a Gnome rogue's Eureka! names the same direct abilities for cost
    /// and damage.
    fn eureka_spells(&self) -> Option<EurekaSpells> {
        let abilities = masks::AMBUSH
            | masks::BACKSTAB
            | masks::EVISCERATE
            | masks::GHOSTLY_STRIKE
            | masks::GOUGE
            | masks::HEMORRHAGE
            | masks::MUTILATE
            | masks::MUTILATE_HIT
            | masks::SINISTER_STRIKE
            | masks::RIPOSTE;
        Some(EurekaSpells {
            cost: abilities,
            damage: abilities,
            tick: 0,
        })
    }
}
