//! Hunter preparation: Go sim/hunter's construction and initialization, and the exporter's
//! Hunter description (tools/oracle-v2/hunter.go).

mod aspects;
mod effects;
pub(crate) mod items;
mod pet;
mod spell_data;
mod spells;
mod talents;
mod weapons;

use crate::contracts::request::Message;
use crate::prepare::agent::{fill_talents, ClassSpellName, PrepAgent};
use crate::prepare::attack::AutoAttackOptions;
use crate::prepare::env::{Environment, FinalizeEffect};
use crate::prepare::sim::{AuraId, Rules, Sim, SpellId, UnitId};
use crate::prepare::stats::Stat;
use crate::prepare::talent_layout::{self, talent, LayoutTalent};
use crate::prepare::Refusal;

pub(crate) use pet::HunterPetState;

/// Go `HunterBaseMaxRange`.
pub(crate) const HUNTER_BASE_MAX_RANGE: f64 = 35.0;
/// Go `ThoridalTheStarsFuryItemID`.
pub(crate) const THORIDAL_THE_STARS_FURY: i32 = 34334;

/// Go `TalentTreeSizes`.
const TALENT_TREE_SIZES: [usize; 3] = [16, 16, 18];

/// Hotfix push 113554's Hunter tree, which only Forever's rules read: each tree's talents in the
/// client's order (row, then column). Pathfinding (19559) and Improved Revive Pet (24443) swap
/// squares in Beast Mastery, so Improved Revive Pet is the fifth talent and Pathfinding, now in
/// the third row, the sixth. Marksmanship and Survival keep the Go reference's order.
const FOREVER_BEAST_MASTERY: &[LayoutTalent] = &[
    talent("deadly_aspects", "Deadly Aspects", 5),
    talent("endurance_training", "Endurance Training", 5),
    talent("focused_fire", "Focused Fire", 2),
    talent(
        "improved_aspect_of_the_monkey",
        "Improved Aspect of the Monkey",
        3,
    ),
    talent("improved_revive_pet", "Improved Revive Pet", 2),
    talent("pathfinding", "Pathfinding", 2),
    talent("bestial_swiftness", "Bestial Swiftness", 1),
    talent("unleashed_fury", "Unleashed Fury", 5),
    talent("improved_mend_pet", "Improved Mend Pet", 2),
    talent("ferocity", "Ferocity", 5),
    talent("summon_hawk", "Summon Hawk", 1),
    talent("spirit_bond", "Spirit Bond", 2),
    talent("intimidation", "Intimidation", 1),
    talent("bestial_discipline", "Bestial Discipline", 2),
    talent("frenzy", "Frenzy", 5),
    talent("bestial_wrath", "Bestial Wrath", 1),
];

const FOREVER_MARKSMANSHIP: &[LayoutTalent] = &[
    talent("hawk_eye", "Hawk Eye", 3),
    talent("improved_concussive_shot", "Improved Concussive Shot", 5),
    talent("lethal_attacks", "Lethal Attacks", 5),
    talent("improved_stings", "Improved Stings", 3),
    talent("efficiency", "Efficiency", 5),
    talent("careful_aim", "Careful Aim", 5),
    talent("rapid_killing", "Rapid Killing", 2),
    talent("improved_arcane_shot", "Improved Arcane Shot", 5),
    talent("lone_wolf", "Lone Wolf", 1),
    talent("trueshot_aura", "Trueshot Aura", 1),
    talent("mortal_shots", "Mortal Shots", 5),
    talent("rapid_recuperation", "Rapid Recuperation", 2),
    talent("barrage", "Barrage", 3),
    talent("scatter_shot", "Scatter Shot", 1),
    talent(
        "ranged_weapon_specialization",
        "Ranged Weapon Specialization",
        5,
    ),
    talent("sniper_shot", "Sniper Shot", 1),
];

const FOREVER_SURVIVAL: &[LayoutTalent] = &[
    talent("improved_tracking", "Improved Tracking", 5),
    talent("deflection", "Deflection", 5),
    talent("entrapment", "Entrapment", 5),
    talent("savage_strikes", "Savage Strikes", 2),
    talent("survivalist", "Survivalist", 5),
    talent("improved_wing_clip", "Improved Wing Clip", 3),
    talent("clever_traps", "Clever Traps", 2),
    talent("surefooted", "Surefooted", 3),
    talent("deterrence", "Deterrence", 1),
    talent("survival_tactics", "Survival Tactics", 2),
    talent("predators_edge", "Predator's Edge", 5),
    talent("counterattack", "Counterattack", 1),
    talent("resourcefulness", "Resourcefulness", 2),
    talent("expose_prey", "Expose Prey", 2),
    talent("survivalists_discipline", "Survivalist's Discipline", 2),
    talent("strider_kick", "Strider Kick", 1),
    talent("lightning_reflexes", "Lightning Reflexes", 5),
    talent("lacerating_strikes", "Lacerating Strikes", 1),
];

pub(crate) const FOREVER_TREES: [&[LayoutTalent]; 3] = [
    FOREVER_BEAST_MASTERY,
    FOREVER_MARKSMANSHIP,
    FOREVER_SURVIVAL,
];

/// Go's Hunter class mask bits, from hunter.go: `HunterSpellFlagsNone` is 0 and the bit of each
/// name after it is `1 << iota`, counting from `SpellMaskSpellRanged` at 1.
pub(crate) mod masks {
    pub const SPELL_RANGED: i64 = 1 << 1;
    pub const AUTO_SHOT: i64 = 1 << 2;
    pub const AIMED_SHOT: i64 = 1 << 3;
    pub const ARCANE_SHOT: i64 = 1 << 4;
    pub const ASPECT_OF_THE_BEAST: i64 = 1 << 5;
    pub const ASPECT_OF_THE_HAWK: i64 = 1 << 6;
    pub const ASPECT_OF_THE_VIPER: i64 = 1 << 7;
    pub const BESTIAL_WRATH: i64 = 1 << 8;
    pub const MULTI_SHOT: i64 = 1 << 9;
    pub const RAPID_FIRE: i64 = 1 << 10;
    pub const RAPTOR_STRIKE: i64 = 1 << 11;
    pub const RAPTOR_STRIKE_QUEUE: i64 = 1 << 12;
    pub const READINESS: i64 = 1 << 13;
    pub const SCORPID_STING: i64 = 1 << 14;
    pub const SERPENT_STING: i64 = 1 << 15;
    pub const STEADY_SHOT: i64 = 1 << 16;
    pub const VOLLEY: i64 = 1 << 17;
    pub const PET_DAMAGE: i64 = 1 << 18;
    pub const EXPLOSIVE_TRAP: i64 = 1 << 19;
    pub const FREEZING_TRAP: i64 = 1 << 20;
    pub const IMMOLATION_TRAP: i64 = 1 << 21;
    pub const LACERATING_STRIKES: i64 = 1 << 22;
    pub const MONGOOSE_BITE: i64 = 1 << 23;
    pub const SNIPER_SHOT: i64 = 1 << 24;
    pub const STRIDER_KICK: i64 = 1 << 25;
    pub const SUMMON_HAWK: i64 = 1 << 26;
    pub const WING_CLIP: i64 = 1 << 27;

    pub const ALL: i64 = AIMED_SHOT
        | ARCANE_SHOT
        | BESTIAL_WRATH
        | MULTI_SHOT
        | RAPID_FIRE
        | RAPTOR_STRIKE
        | SERPENT_STING
        | SNIPER_SHOT
        | SUMMON_HAWK
        | VOLLEY
        | MONGOOSE_BITE
        | STRIDER_KICK
        | WING_CLIP
        | EXPLOSIVE_TRAP
        | FREEZING_TRAP
        | IMMOLATION_TRAP;
    /// The shots and stings in Efficiency's (19416) class mask; Sniper Shot is not in it.
    pub const SHOTS_AND_STINGS: i64 =
        AIMED_SHOT | ARCANE_SHOT | MULTI_SHOT | SERPENT_STING | VOLLEY | SUMMON_HAWK;
    pub const MELEE: i64 = RAPTOR_STRIKE | MONGOOSE_BITE | STRIDER_KICK | WING_CLIP;
    pub const TRAPS: i64 = EXPLOSIVE_TRAP | FREEZING_TRAP | IMMOLATION_TRAP;
}

/// tools/oracle-v2/hunter.go `hunterClassSpells`.
static CLASS_SPELLS: &[ClassSpellName] = &[
    ClassSpellName {
        mask: masks::SPELL_RANGED,
        name: "spell_ranged",
    },
    ClassSpellName {
        mask: masks::AUTO_SHOT,
        name: "auto_shot",
    },
    ClassSpellName {
        mask: masks::AIMED_SHOT,
        name: "aimed_shot",
    },
    ClassSpellName {
        mask: masks::ARCANE_SHOT,
        name: "arcane_shot",
    },
    ClassSpellName {
        mask: masks::ASPECT_OF_THE_BEAST,
        name: "aspect_of_the_beast",
    },
    ClassSpellName {
        mask: masks::ASPECT_OF_THE_HAWK,
        name: "aspect_of_the_hawk",
    },
    ClassSpellName {
        mask: masks::ASPECT_OF_THE_VIPER,
        name: "aspect_of_the_viper",
    },
    ClassSpellName {
        mask: masks::BESTIAL_WRATH,
        name: "bestial_wrath",
    },
    ClassSpellName {
        mask: masks::MULTI_SHOT,
        name: "multi_shot",
    },
    ClassSpellName {
        mask: masks::RAPID_FIRE,
        name: "rapid_fire",
    },
    ClassSpellName {
        mask: masks::RAPTOR_STRIKE,
        name: "raptor_strike",
    },
    ClassSpellName {
        mask: masks::RAPTOR_STRIKE_QUEUE,
        name: "raptor_strike_queue",
    },
    ClassSpellName {
        mask: masks::READINESS,
        name: "readiness",
    },
    ClassSpellName {
        mask: masks::SCORPID_STING,
        name: "scorpid_sting",
    },
    ClassSpellName {
        mask: masks::SERPENT_STING,
        name: "serpent_sting",
    },
    ClassSpellName {
        mask: masks::STEADY_SHOT,
        name: "steady_shot",
    },
    ClassSpellName {
        mask: masks::VOLLEY,
        name: "volley",
    },
    ClassSpellName {
        mask: masks::PET_DAMAGE,
        name: "pet_damage",
    },
    ClassSpellName {
        mask: masks::EXPLOSIVE_TRAP,
        name: "explosive_trap",
    },
    ClassSpellName {
        mask: masks::FREEZING_TRAP,
        name: "freezing_trap",
    },
    ClassSpellName {
        mask: masks::IMMOLATION_TRAP,
        name: "immolation_trap",
    },
    ClassSpellName {
        mask: masks::LACERATING_STRIKES,
        name: "lacerating_strikes",
    },
    ClassSpellName {
        mask: masks::MONGOOSE_BITE,
        name: "mongoose_bite",
    },
    ClassSpellName {
        mask: masks::SNIPER_SHOT,
        name: "sniper_shot",
    },
    ClassSpellName {
        mask: masks::STRIDER_KICK,
        name: "strider_kick",
    },
    ClassSpellName {
        mask: masks::SUMMON_HAWK,
        name: "summon_hawk",
    },
    ClassSpellName {
        mask: masks::WING_CLIP,
        name: "wing_clip",
    },
];

/// Go `Hunter`: the spells and auras its construction and initialization register.
#[derive(Clone)]
pub(crate) struct Hunter {
    pub unit: UnitId,
    pub talents: Message,
    pub options: Message,
    pub ammo_dps: f64,
    pub ammo_damage_bonus: f64,
    pub pet: Option<UnitId>,
    pub pet_state: Option<HunterPetState>,
    pub aimed_shot: Option<SpellId>,
    pub arcane_shot: Option<SpellId>,
    pub aspect_of_the_beast: Option<SpellId>,
    pub aspect_of_the_hawk: Option<SpellId>,
    pub explosive_trap: Option<SpellId>,
    pub freezing_trap: Option<SpellId>,
    pub immolation_trap: Option<SpellId>,
    pub lacerating_strikes: Option<SpellId>,
    pub mongoose_bite: Option<SpellId>,
    pub multi_shot: Option<SpellId>,
    pub rapid_fire: Option<SpellId>,
    pub raptor_strike: Option<SpellId>,
    pub raptor_strike_hit: Option<SpellId>,
    pub serpent_sting: Option<SpellId>,
    pub sniper_shot: Option<SpellId>,
    pub strider_kick: Option<SpellId>,
    pub summon_hawk: Option<SpellId>,
    pub volley: Option<SpellId>,
    pub wing_clip: Option<SpellId>,
    pub aspect_of_the_beast_aura: Option<AuraId>,
    pub aspect_of_the_hawk_aura: Option<AuraId>,
    pub rapid_fire_aura: Option<AuraId>,
    pub talon_of_alar_aura: Option<AuraId>,
    pub quiver_bonus_aura: Option<AuraId>,
    /// Mongoose Bite is only castable in the window a dodge opens.
    pub defensive_state: Option<AuraId>,
    /// The player's rotation, which decides whether swings are replaced.
    pub rotation: Option<Message>,
    /// Go `Raptor Strike`'s spell ID, once registered.
    pub raptor_strike_id: Option<i32>,
    /// What `RegisterPostFinalizeEffect` calls registered, for the environment to run.
    pub post_finalize_effects: Vec<FinalizeEffect>,
}

impl Hunter {
    pub(crate) fn t(&self, name: &str) -> i32 {
        self.talents.i32(name)
    }

    pub(crate) fn flag(&self, name: &str) -> bool {
        self.talents.bool(name)
    }
}

/// `options.GetHunter().Options.ClassOptions`.
fn class_options(player: &Message) -> Result<Message, Refusal> {
    let missing = || Refusal::new("request", "a hunter without hunter options".to_string());
    let Some((_, crate::contracts::request::Value::Message(spec))) = player.oneof("spec") else {
        return Err(missing());
    };
    let options = spec.message("options").ok_or_else(missing)?;
    Ok(options
        .message("class_options")
        .cloned()
        .unwrap_or_else(|| Message::empty("proto.HunterOptions")))
}

/// Go `NewHunter`.
pub(crate) fn new_hunter(
    sim: &mut Sim,
    unit: UnitId,
    player: &Message,
) -> Result<Box<dyn PrepAgent>, Refusal> {
    let talents_string = player.str("talents_string");
    let talents = match sim.rules {
        Rules::Forever => talent_layout::fill(
            "proto.HunterTalents",
            "hunter",
            talents_string,
            &FOREVER_TREES,
        )
        .map(|(talents, _)| talents),
        Rules::GoReference => {
            fill_talents("proto.HunterTalents", talents_string, TALENT_TREE_SIZES)
        }
    }
    .map_err(|err| Refusal::new("talents", err))?;
    let options = class_options(player)?;
    let mut hunter = Hunter {
        unit,
        talents,
        options,
        ammo_dps: 0.0,
        ammo_damage_bonus: 0.0,
        pet: None,
        pet_state: None,
        aimed_shot: None,
        arcane_shot: None,
        aspect_of_the_beast: None,
        aspect_of_the_hawk: None,
        explosive_trap: None,
        freezing_trap: None,
        immolation_trap: None,
        lacerating_strikes: None,
        mongoose_bite: None,
        multi_shot: None,
        rapid_fire: None,
        raptor_strike: None,
        raptor_strike_hit: None,
        serpent_sting: None,
        sniper_shot: None,
        strider_kick: None,
        summon_hawk: None,
        volley: None,
        wing_clip: None,
        aspect_of_the_beast_aura: None,
        aspect_of_the_hawk_aura: None,
        rapid_fire_aura: None,
        talon_of_alar_aura: None,
        quiver_bonus_aura: None,
        defensive_state: None,
        rotation: player.message("rotation").cloned(),
        raptor_strike_id: None,
        post_finalize_effects: Vec::new(),
    };
    sim.unit_mut(unit).pseudo_stats.can_parry = true;
    sim.enable_mana_bar(unit);

    let ranged_item_id = sim.ranged_weapon(unit).map(|item| item.id);
    hunter.apply_ammo_dps();
    hunter.apply_quiver_bonus(sim, ranged_item_id);

    let mut ranged_weapon = weapons::weapon_from_ranged(sim, unit);
    if ranged_item_id != Some(THORIDAL_THE_STARS_FURY) {
        hunter.ammo_damage_bonus = hunter.ammo_dps * ranged_weapon.swing_speed;
        ranged_weapon.base_damage_min += hunter.ammo_damage_bonus;
        ranged_weapon.base_damage_max += hunter.ammo_damage_bonus;
    }

    let main_hand = weapons::weapon_from_main_hand(sim, unit);
    let off_hand = weapons::weapon_from_off_hand(sim, unit);
    sim.enable_auto_attacks(
        unit,
        AutoAttackOptions {
            main_hand,
            off_hand,
            ranged: ranged_weapon,
            auto_swing_melee: true,
            auto_swing_ranged: true,
            replace_mh_swing: true,
            ..AutoAttackOptions::default()
        },
    );
    // The main hand config's ApplyEffects only adds a log line.
    if let Some(config) = sim.unit_mut(unit).auto_attacks.ranged_config.as_mut() {
        config.max_range = HUNTER_BASE_MAX_RANGE;
    }

    hunter.add_stat_dependencies(sim);

    pet::new_hunter_pet(sim, &mut hunter);

    Ok(Box::new(hunter))
}

impl Hunter {
    /// Go `applyAmmoDPS`: Forever is a Classic-era realm, so these are the Classic ammo values.
    fn apply_ammo_dps(&mut self) {
        self.ammo_dps = match self.options.enum_name("ammo").as_str() {
            "RazorArrow" | "SolidShot" => 7.5,
            "JaggedArrow" | "AccurateSlugs" => 13.0,
            "MithrilGyroShot" => 15.0,
            "IceThreadedArrow" | "IceThreadedBullet" => 16.5,
            "ThoriumHeadedArrow" | "ThoriumShells" => 17.5,
            "RockshardPellets" => 18.0,
            "Doomshot" => 20.0,
            "MiniatureCannonBalls" => 20.5,
            // Forever's Engineering ammo, which the reference does not have: "Adds 24.5 damage
            // per second" on both tooltips (README, Rust-only mechanics).
            "SwiftfeatherArrow" | "SwiftstrikeShot" => 24.5,
            _ => self.ammo_dps,
        };
    }

    /// Go `AddStatDependencies`.
    fn add_stat_dependencies(&self, sim: &mut Sim) {
        let unit = self.unit;
        let crit_per_agi = sim.crit_per_agi_max_level(unit);
        let sdm = &mut sim.unit_mut(unit).sdm;
        sdm.add_stat_dependency(Stat::Strength, Stat::AttackPower, 1.0);
        sdm.add_stat_dependency(Stat::Agility, Stat::AttackPower, 1.0);
        // A Classic hunter gets two ranged attack power per agility, not one.
        sdm.add_stat_dependency(Stat::Agility, Stat::RangedAttackPower, 2.0);
        sdm.add_stat_dependency(Stat::Agility, Stat::PhysicalCritPercent, crit_per_agi);
        // Classic's hunter dodges at twice its crit rate per agility.
        sdm.add_stat_dependency(
            Stat::Agility,
            Stat::DodgeRating,
            2.0 * crit_per_agi
                * crate::prepare::character::constants::DODGE_RATING_PER_DODGE_PERCENT,
        );
    }

    /// Go `Hunter.Initialize`.
    fn initialize_class(&mut self, sim: &mut Sim) {
        self.register_spells(sim);
        self.add_pvp_gloves(sim);
    }
}

impl PrepAgent for Hunter {
    /// pet.go trackPetCritOffset (patches 136 and 138): the pet's disable callback clears the
    /// crit offset its owner keeps for the inheritance and moves no stat.
    fn stat_callback_pet(&self, _sim: &Sim, pet: UnitId) -> bool {
        self.pet == Some(pet)
    }

    fn add_party_buffs(&self, _sim: &Sim, party_buffs: &mut Message) {
        if self.flag("trueshot_aura") {
            party_buffs.set_bool("trueshot_aura", true);
        }
    }

    fn apply_talents(&mut self, sim: &mut Sim, _unit: UnitId) {
        self.register_beast_mastery_talents(sim);
        self.register_marksmanship_talents(sim);
        self.register_survival_talents(sim);
    }

    fn initialize(&mut self, sim: &mut Sim, _unit: UnitId) {
        self.initialize_class(sim);
    }

    fn apply_item_effect(&mut self, sim: &mut Sim, unit: UnitId, item: i32) -> bool {
        self.apply_hunter_item_effect(sim, unit, item)
    }

    fn class_item_use_effect(
        &self,
        env: &Environment,
        spell: SpellId,
        item: i32,
    ) -> Option<serde_json::Value> {
        (item == 19953).then(|| self.renatakis_charm_effect(&env.sim, spell))
    }

    fn take_post_finalize_effects(&mut self) -> Vec<FinalizeEffect> {
        std::mem::take(&mut self.post_finalize_effects)
    }

    fn initialize_pet(&mut self, sim: &mut Sim, pet: UnitId) {
        self.initialize_hunter_pet(sim, pet);
    }

    fn reset_pet(&mut self, _sim: &mut Sim, _pet: UnitId) {
        // HunterPet.Reset only reads the pet's uptime.
    }

    fn reset_only_pets(&self) -> bool {
        true
    }

    fn talents(&self) -> &Message {
        &self.talents
    }

    fn class_spells(&self) -> &'static [ClassSpellName] {
        CLASS_SPELLS
    }

    fn unmasked_spell(&self, id: &crate::contracts::prepared_v2::ActionId) -> Option<&'static str> {
        // talents_beast_mastery.go registerIntimidation registers its cast without a class mask.
        (id.spell_id == 19577 && id.tag == 0 && id.item_id == 0 && id.other_id.is_empty())
            .then_some("intimidation")
    }

    fn stat_auras(&self, _sim: &Sim, _unit: UnitId) -> Vec<String> {
        vec![
            "Aspect of the Hawk".to_string(),
            "Aspect of the Beast".to_string(),
            // Furious Howl: the Wolf's attack power aura on its owner (the fork's patch 119).
            "Furious Howl".to_string(),
        ]
    }

    fn effects(&self, sim: &Sim, unit: UnitId) -> Vec<serde_json::Value> {
        self.export_effects(sim, unit)
    }

    fn unrepresented(&self, sim: &Sim, unit: UnitId) -> Vec<String> {
        self.export_notes(sim, unit)
    }

    fn swing_replacement_keeps_swing(&self) -> bool {
        self.swing_replacement_keeps_swing_impl()
    }
}
