//! Druid preparation: Go sim/druid's construction and initialization (the class package and its
//! Balance, Feral Cat and Feral Bear specs) and the exporter's Druid description
//! (tools/oracle-v2/druid.go, druid_feral.go, druid_bear.go and druid_items.go).
//!
//! Go keeps the druid's state in struct fields its callbacks close over. Here the state a
//! callback reads is [`forms::State`], shared by reference counting; the rest stays on
//! [`Druid`], which is the class agent.

mod balance;
mod baseline;
mod bear;
mod export;
mod feral;
pub(crate) mod forms;
pub(crate) mod heals;
pub(crate) mod items;
pub(crate) mod masks;
mod restoration;
mod talents;

use std::rc::Rc;

use crate::classes::druid::forms::{BEAR, CAT, HUMANOID, MOONKIN};
use crate::contracts::prepared_v2::ActionId;
use crate::contracts::request::Message;
use crate::prepare::agent::{fill_talents, ClassSpellName, PrepAgent};
use crate::prepare::attack::AutoAttackOptions;
use crate::prepare::energy::EnergyBarOptions;
use crate::prepare::rage::RageBarOptions;
use crate::prepare::sim::{AuraId, PowerBar, Rules, Sim, SpellId, UnitId};
use crate::prepare::spell::SpellConfig;
use crate::prepare::stats::Stat;
use crate::prepare::talent_layout::{self, new_talent, talent, LayoutTalent};
use crate::prepare::Refusal;

use talents::Talents;

/// Go druid.TalentTreeSizes.
const TALENT_TREE_SIZES: [usize; 3] = [16, 20, 16];

/// Hotfix push 113531's Druid tree, which only Forever's rules read: each tree's talents in the
/// client's order (row, then column), with upstream's proto field names (wowsims/forever #769).
/// Balance keeps the Go reference's order.
const FOREVER_BALANCE: &[LayoutTalent] = &[
    talent("improved_wrath", "Improved Wrath", 5),
    talent("genesis", "Genesis", 5),
    talent("moonglow", "Moonglow", 3),
    talent("improved_moonfire", "Improved Moonfire", 2),
    talent("natures_majesty", "Nature's Majesty", 2),
    talent("natures_reach", "Nature's Reach", 2),
    talent("improved_entangling_roots", "Improved Entangling Roots", 3),
    talent("natures_splendor", "Nature's Splendor", 1),
    talent("insect_swarm", "Insect Swarm", 1),
    talent("vengeance", "Vengeance", 5),
    talent("improved_starfire", "Improved Starfire", 5),
    talent("overgrowth", "Overgrowth", 2),
    talent("natures_grace", "Nature's Grace", 1),
    talent("eclipse", "Eclipse", 3),
    talent("moonfury", "Moonfury", 5),
    talent("moonkin_form", "Moonkin Form", 1),
];

/// Berserk (417141) and Improved Shifting Power (1322670) are gone. Furor (17056) takes Heart of
/// the Wild's square in the first row; Predatory Strikes (16972) moves to the fourth row's first
/// column, beside the new Lacerate (1328617, after Savage Fury) and Predatory Discipline
/// (1328613); Rend and Tear (1223246) and Natural Reaction (417051) swap columns in the sixth
/// row; Primal Bite (407995, after Leader of the Pack) and Shifting Power (1322605, after
/// Predatory Discipline) close the tree in the seventh.
const FOREVER_FERAL_COMBAT: &[LayoutTalent] = &[
    talent("ferocity", "Ferocity", 5),
    talent("furor", "Furor", 5),
    talent("feral_swiftness", "Feral Swiftness", 2),
    talent("feral_instinct", "Feral Instinct", 3),
    talent("brutal_impact", "Brutal Impact", 2),
    talent("thick_hide", "Thick Hide", 3),
    talent("shredding_attacks", "Shredding Attacks", 3),
    talent("savage_fury", "Savage Fury", 2),
    talent("feral_charge", "Feral Charge", 1),
    talent("sharpened_claws", "Sharpened Claws", 2),
    talent("predatory_strikes", "Predatory Strikes", 3),
    new_talent("lacerate", "Lacerate", 1),
    new_talent("predatory_discipline", "Predatory Discipline", 1),
    talent("blood_frenzy", "Blood Frenzy", 2),
    talent("leader_of_the_pack", "Leader of the Pack", 1),
    talent("natural_instinct", "Natural Instinct", 2),
    talent("rend_and_tear", "Rend and Tear", 5),
    talent("natural_reaction", "Natural Reaction", 5),
    talent("primal_bite", "Primal Bite", 1),
    talent("shifting_power", "Shifting Power", 1),
];

/// Gift of the Earthmother (414673) is gone and Bloom (1328621) takes its square. Heart of the
/// Wild (17003) takes Furor's square, and Reflection (17106) and Subtlety (17118) swap.
const FOREVER_RESTORATION: &[LayoutTalent] = &[
    talent("natures_focus", "Nature's Focus", 5),
    talent("heart_of_the_wild", "Heart of the Wild", 5),
    talent("naturalist", "Naturalist", 5),
    talent("reflection", "Reflection", 3),
    talent("natural_shapeshifter", "Natural Shapeshifter", 3),
    talent("subtlety", "Subtlety", 3),
    talent("gift_of_nature", "Gift of Nature", 5),
    new_talent("bloom", "Bloom", 1),
    talent("tranquil_spirit", "Tranquil Spirit", 5),
    talent("improved_rejuvenation", "Improved Rejuvenation", 3),
    talent("swiftmend", "Swiftmend", 1),
    talent("natures_swiftness", "Nature's Swiftness", 1),
    talent("living_spirit", "Living Spirit", 3),
    talent("improved_tranquility", "Improved Tranquility", 2),
    talent("improved_regrowth", "Improved Regrowth", 5),
    talent("wild_growth", "Wild Growth", 1),
];

pub(crate) const FOREVER_TREES: [&[LayoutTalent]; 3] =
    [FOREVER_BALANCE, FOREVER_FERAL_COMBAT, FOREVER_RESTORATION];

/// Which of Go's three registered Druid specs the player is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Spec {
    Balance,
    FeralCat,
    FeralBear,
    /// MythicSim's healer sims (Rust only): the druid heals in caster form.
    Restoration,
}

/// tools/oracle-v2/druid.go `druidClassSpells`.
static CLASS_SPELLS: &[ClassSpellName] = &[
    // MythicSim's healer sims (Rust only): the Restoration druid's heals and Swiftmend.
    ClassSpellName {
        mask: masks::TRANQUILITY,
        name: "heal",
    },
    ClassSpellName {
        mask: masks::HEALING_TOUCH,
        name: "heal",
    },
    ClassSpellName {
        mask: masks::REJUVENATION,
        name: "heal",
    },
    ClassSpellName {
        mask: masks::REGROWTH,
        name: "heal",
    },
    ClassSpellName {
        mask: masks::WILD_GROWTH,
        name: "heal",
    },
    ClassSpellName {
        mask: masks::SWIFTMEND,
        name: "swiftmend",
    },
    ClassSpellName {
        mask: masks::BLOOM,
        name: "bloom",
    },
    ClassSpellName {
        mask: masks::ENTANGLING_ROOTS,
        name: "entangling_roots",
    },
    ClassSpellName {
        mask: masks::CLAW,
        name: "claw",
    },
    ClassSpellName {
        mask: masks::DEMORALIZING_ROAR,
        name: "demoralizing_roar",
    },
    ClassSpellName {
        mask: masks::FAERIE_FIRE,
        name: "faerie_fire",
    },
    ClassSpellName {
        mask: masks::FAERIE_FIRE_FERAL,
        name: "faerie_fire_feral",
    },
    ClassSpellName {
        mask: masks::HURRICANE,
        name: "hurricane",
    },
    ClassSpellName {
        mask: masks::FEROCIOUS_BITE,
        name: "ferocious_bite",
    },
    ClassSpellName {
        mask: masks::FRENZIED_REGENERATION,
        name: "frenzied_regeneration",
    },
    ClassSpellName {
        mask: masks::INNERVATE,
        name: "innervate",
    },
    ClassSpellName {
        mask: masks::INSECT_SWARM,
        name: "insect_swarm",
    },
    ClassSpellName {
        mask: masks::LACERATE,
        name: "lacerate",
    },
    ClassSpellName {
        mask: masks::PRIMAL_BITE,
        name: "primal_bite",
    },
    ClassSpellName {
        mask: masks::MAUL,
        name: "maul",
    },
    ClassSpellName {
        mask: masks::MOONFIRE_INITIAL,
        name: "moonfire",
    },
    ClassSpellName {
        mask: masks::MOONFIRE_DOT,
        name: "moonfire_dot",
    },
    ClassSpellName {
        mask: masks::RAKE,
        name: "rake",
    },
    ClassSpellName {
        mask: masks::RAVAGE,
        name: "ravage",
    },
    ClassSpellName {
        mask: masks::RIP,
        name: "rip",
    },
    ClassSpellName {
        mask: masks::SHRED,
        name: "shred",
    },
    ClassSpellName {
        mask: masks::STARFIRE,
        name: "starfire",
    },
    ClassSpellName {
        mask: masks::SWIPE,
        name: "swipe",
    },
    ClassSpellName {
        mask: masks::THORNS,
        name: "thorns",
    },
    ClassSpellName {
        mask: masks::WRATH,
        name: "wrath",
    },
    ClassSpellName {
        mask: masks::ENRAGE,
        name: "enrage",
    },
    ClassSpellName {
        mask: masks::SHIFTING_POWER,
        name: "shifting_power",
    },
    ClassSpellName {
        mask: masks::CAT_FORM,
        name: "cat_form",
    },
    ClassSpellName {
        mask: masks::BEAR_FORM,
        name: "bear_form",
    },
    ClassSpellName {
        mask: masks::MOONKIN_FORM,
        name: "moonkin_form",
    },
    ClassSpellName {
        mask: masks::HEALING_TOUCH,
        name: "healing_touch",
    },
    ClassSpellName {
        mask: masks::REGROWTH,
        name: "regrowth",
    },
    ClassSpellName {
        mask: masks::LIFEBLOOM,
        name: "lifebloom",
    },
    ClassSpellName {
        mask: masks::REJUVENATION,
        name: "rejuvenation",
    },
    ClassSpellName {
        mask: masks::TRANQUILITY,
        name: "tranquility",
    },
    ClassSpellName {
        mask: masks::MARK_OF_THE_WILD,
        name: "mark_of_the_wild",
    },
    ClassSpellName {
        mask: masks::SWIFTMEND,
        name: "swiftmend",
    },
    ClassSpellName {
        mask: masks::CENARION_WARD,
        name: "cenarion_ward",
    },
    ClassSpellName {
        mask: masks::REVIVE,
        name: "revive",
    },
];

/// Go `Druid`, with the `FeralDruid`, `GuardianDruid` and `BalanceDruid` embedding it.
#[derive(Clone)]
pub(crate) struct Druid {
    pub(super) unit: UnitId,
    pub(super) spec: Spec,
    pub(super) talents: Message,
    pub(super) tal: Talents,
    pub(super) st: Rc<forms::State>,
    /// The forms each spell registered through `druid.RegisterSpell` may be cast in, in
    /// registration order.
    pub(super) form_masks: Vec<(SpellId, u8)>,
    /// Go `CannotShredTarget`.
    pub(super) cannot_shred_target: bool,
    /// Go `FurorProcChance`.
    pub(super) furor_proc_chance: f64,
    /// Go `IntensityEnrageRageBonus`.
    pub(super) intensity_enrage_rage_bonus: f64,
    pub(super) cat_form_aura: Option<AuraId>,
    pub(super) bear_form_aura: Option<AuraId>,
    pub(super) moonkin_form_aura: Option<AuraId>,
    pub(super) clearcasting_aura: Option<AuraId>,
    pub(super) berserk_aura: Option<AuraId>,
    pub(super) eclipse_aura: Option<AuraId>,
    pub(super) prowl_aura: Option<AuraId>,
    pub(super) cat_form: Option<SpellId>,
    pub(super) bear_form: Option<SpellId>,
    pub(super) moonkin_form: Option<SpellId>,
    pub(super) starfire: Vec<SpellId>,
    pub(super) wrath: Option<SpellId>,
    pub(super) moonfire: Option<SpellId>,
    pub(super) insect_swarm: Option<SpellId>,
    pub(super) hurricane: Option<SpellId>,
    pub(super) faerie_fire: Option<SpellId>,
    /// Go `FaerieFireAuras`, by unit index.
    pub(super) faerie_fire_auras: Vec<Option<AuraId>>,
    pub(super) claw: Option<SpellId>,
    pub(super) shred: Option<SpellId>,
    pub(super) ravage: Option<SpellId>,
    pub(super) rake: Option<SpellId>,
    pub(super) rip: Option<SpellId>,
    pub(super) ferocious_bite: Option<SpellId>,
    pub(super) shifting_power: Option<SpellId>,
    pub(super) prowl: Option<SpellId>,
    pub(super) berserk: Option<SpellId>,
    pub(super) maul: Option<SpellId>,
    pub(super) maul_strike: Option<SpellId>,
    pub(super) lacerate: Option<SpellId>,
    pub(super) primal_bite: Option<SpellId>,
    pub(super) swipe: Option<SpellId>,
    pub(super) enrage: Option<SpellId>,
    pub(super) frenzied_regeneration: Option<SpellId>,
    pub(super) demoralizing_roar: Option<SpellId>,
    /// Go `DemoralizingRoarAuras`, by unit index.
    pub(super) demoralizing_roar_auras: Vec<Option<AuraId>>,
    pub(super) barkskin: Option<SpellId>,
    pub(super) options: SpecOptions,
    /// A Restoration druid's heals, for the export.
    pub(super) heals: Vec<crate::prepare::heals::HealRank>,
}

/// The spec options the construction reads.
#[derive(Clone, Debug, Default)]
pub(super) struct SpecOptions {
    pub starting_rage: f64,
}

/// Go `druid.New`, `NewBalanceDruid`, `NewFeralCatDruid` and `NewFeralBearDruid`.
pub(crate) fn new_druid(
    sim: &mut Sim,
    unit: UnitId,
    player: &Message,
) -> Result<Box<dyn PrepAgent>, Refusal> {
    let Some((field, spec_value)) = player.oneof("spec") else {
        return Err(Refusal::new(
            "spec",
            "a Druid without a spec is not prepared".to_string(),
        ));
    };
    let spec = match field {
        "balance_druid" => Spec::Balance,
        "feral_cat_druid" => Spec::FeralCat,
        "feral_bear_druid" => Spec::FeralBear,
        "restoration_druid" => Spec::Restoration,
        other => {
            return Err(Refusal::new(
                "spec",
                format!("the {other} spec is not prepared in Rust yet"),
            ))
        }
    };
    let spec_message = match spec_value {
        crate::contracts::request::Value::Message(message) => message,
        _ => {
            return Err(Refusal::new(
                "spec",
                "a Druid spec without options".to_string(),
            ))
        }
    };
    let options = spec_message.message("options");
    if let Some(target) = options
        .and_then(|options| options.message("class_options"))
        .and_then(|class| class.message("innervate_target"))
    {
        // Only an empty reference and the player itself resolve to the druid's own unit.
        let kind = target.enum_name("type");
        let own =
            kind == "Unknown" || kind == "Self" || (kind == "Player" && target.i32("index") == 0);
        if !own {
            return Err(Refusal::new(
                "innervate_target",
                "an Innervate target other than the druid is not prepared yet".to_string(),
            ));
        }
    }

    let talents_string = player.str("talents_string");
    let (talents, extras) = match sim.rules {
        Rules::Forever => talent_layout::fill(
            "proto.DruidTalents",
            "druid",
            talents_string,
            &FOREVER_TREES,
        ),
        Rules::GoReference => fill_talents("proto.DruidTalents", talents_string, TALENT_TREE_SIZES)
            .map(|talents| (talents, talent_layout::Extras::new())),
    }
    .map_err(|err| Refusal::new("talents", err))?;
    let tal = Talents::from_message(&talents, &extras);

    // druid.New.
    sim.enable_mana_bar(unit);
    let crit_per_agi = sim.crit_per_agi_max_level(unit);
    {
        let sdm = &mut sim.unit_mut(unit).sdm;
        // Two attack power a point of Strength in every form, as on master.
        sdm.add_stat_dependency(Stat::Strength, Stat::AttackPower, 2.0);
        sdm.add_stat_dependency(Stat::BonusArmor, Stat::Armor, 1.0);
        sdm.add_stat_dependency(Stat::Agility, Stat::PhysicalCritPercent, crit_per_agi);
        // Dodge is Classic's at level 60: 0.9% base and 20 Agility a percent.
        sdm.add_stat_dependency(
            Stat::Agility,
            Stat::DodgeRating,
            crit_per_agi * crate::prepare::character::constants::DODGE_RATING_PER_DODGE_PERCENT,
        );
    }
    sim.unit_mut(unit).pseudo_stats.base_dodge_chance += 0.009;

    let starting_form = match spec {
        Spec::Balance => MOONKIN,
        Spec::FeralCat => CAT,
        Spec::FeralBear => BEAR,
        Spec::Restoration => HUMANOID,
    };
    let mut druid = Druid {
        unit,
        spec,
        talents,
        tal,
        st: Rc::new(forms::State::new(sim, starting_form)),
        form_masks: Vec::new(),
        cannot_shred_target: false,
        furor_proc_chance: 0.0,
        intensity_enrage_rage_bonus: 0.0,
        cat_form_aura: None,
        bear_form_aura: None,
        moonkin_form_aura: None,
        clearcasting_aura: None,
        berserk_aura: None,
        eclipse_aura: None,
        prowl_aura: None,
        cat_form: None,
        bear_form: None,
        moonkin_form: None,
        starfire: Vec::new(),
        wrath: None,
        moonfire: None,
        insect_swarm: None,
        hurricane: None,
        faerie_fire: None,
        faerie_fire_auras: Vec::new(),
        claw: None,
        shred: None,
        ravage: None,
        rake: None,
        rip: None,
        ferocious_bite: None,
        shifting_power: None,
        prowl: None,
        berserk: None,
        maul: None,
        maul_strike: None,
        lacerate: None,
        primal_bite: None,
        swipe: None,
        enrage: None,
        frenzied_regeneration: None,
        demoralizing_roar: None,
        demoralizing_roar_auras: Vec::new(),
        barkskin: None,
        options: SpecOptions::default(),
        heals: Vec::new(),
    };

    match spec {
        Spec::Balance => {
            // moonkin.RegisterMoonkinFormSpell() then RegisterMoonkinFormAura().
            druid.register_moonkin_form_spell(sim);
            druid.register_moonkin_form_aura(sim);
        }
        Spec::FeralCat => {
            druid.cannot_shred_target = spec_message
                .message("options")
                .is_some_and(|options| options.bool("cannot_shred_target"));
            sim.enable_energy_bar(
                unit,
                EnergyBarOptions {
                    max_combo_points: 5,
                    max_energy: 100.0,
                    has_no_regen: false,
                },
            );
            // The Cat never swings in Bear Form unless a custom rotation shifts. Its bar pays
            // the 75% crit Rage bonus like the Bear's.
            sim.enable_rage_bar(
                unit,
                RageBarOptions {
                    base_rage_multiplier: 1.0,
                    ..RageBarOptions::default()
                },
            );
            let paw = forms::cat_weapon(sim, unit);
            sim.enable_auto_attacks(
                unit,
                AutoAttackOptions {
                    main_hand: paw,
                    auto_swing_melee: true,
                    ..AutoAttackOptions::default()
                },
            );
            druid.register_cat_form_aura(sim);
            druid.register_bear_form_aura(sim);
        }
        Spec::FeralBear => {
            druid.options.starting_rage = spec_message
                .message("options")
                .map_or(0.0, |options| options.f64("starting_rage"));
            sim.enable_energy_bar(
                unit,
                EnergyBarOptions {
                    max_combo_points: 5,
                    max_energy: 100.0,
                    has_no_regen: false,
                },
            );
            sim.enable_rage_bar(
                unit,
                RageBarOptions {
                    max_rage: 0.0,
                    starting_rage: druid.options.starting_rage,
                    base_rage_multiplier: 1.0,
                },
            );
            let paw = forms::bear_weapon(sim, unit);
            sim.enable_auto_attacks(
                unit,
                AutoAttackOptions {
                    main_hand: paw,
                    auto_swing_melee: true,
                    replace_mh_swing: true,
                    ..AutoAttackOptions::default()
                },
            );
            druid.register_bear_form_aura(sim);
            druid.register_cat_form_aura(sim);
        }
        // Caster form: the healer never shifts, and has no auto attacks.
        Spec::Restoration => {}
    }
    Ok(Box::new(druid))
}

impl Druid {
    /// Go `druid.RegisterSpell`: a spell with the forms it may be cast in. The wrapper gives
    /// every one of them an extra cast condition.
    pub(super) fn register_spell(
        &mut self,
        sim: &mut Sim,
        form_mask: u8,
        mut config: SpellConfig,
    ) -> SpellId {
        config.has_extra_cast_condition = true;
        let id = sim.register_spell(self.unit, config);
        self.form_masks.push((id, form_mask));
        id
    }

    /// Go `Druid.Initialize`.
    fn initialize_druid(&mut self, sim: &mut Sim) {
        self.st.set_form(sim, self.st.starting_form);
        // The item swap rebuilds the paw from the weapon in the hand.
        sim.register_item_swap_callback(
            self.unit,
            &[crate::prepare::items::slot::MAIN_HAND],
            crate::prepare::item_swap::SwapCallback::DruidPaw,
        );
        // RegisterBaselineSpells.
        self.register_innervate_cd(sim);
        self.register_thorns_spell(sim);
    }
}

impl PrepAgent for Druid {
    fn add_party_buffs(&self, sim: &Sim, party_buffs: &mut Message) {
        match self.spec {
            // BalanceDruid does not override AddPartyBuffs: Druid's does. A Restoration druid
            // in caster form adds nothing.
            Spec::Balance | Spec::Restoration => {
                if self.st.in_form(sim, BEAR | CAT) && self.tal.leader_of_the_pack {
                    party_buffs.set_bool("leader_of_the_pack", true);
                } else if self.st.in_form(sim, MOONKIN) && self.tal.moonkin_form {
                    party_buffs.set_bool("moonkin_aura", true);
                }
            }
            Spec::FeralCat | Spec::FeralBear => {
                if self.tal.leader_of_the_pack {
                    party_buffs.set_bool("leader_of_the_pack", true);
                }
            }
        }
    }

    fn apply_talents(&mut self, sim: &mut Sim, _unit: UnitId) {
        // Omen of Clarity is a baseline passive in Forever, wired in here with the procs.
        self.apply_omen_of_clarity(sim);
        self.register_balance_talents(sim);
        self.register_feral_combat_talents(sim);
        self.register_restoration_talents(sim);
        if self.spec == Spec::Restoration {
            self.apply_heal_talents(sim);
        }
    }

    /// druid.go Initialize's item swap callback: in Cat or Bear Form the paw takes the weapon
    /// now in the hand.
    fn item_swap_main_hand(&mut self, sim: &mut Sim, unit: UnitId) {
        if self.st.in_form(sim, CAT) {
            sim.unit_mut(unit).auto_attacks.mh = forms::cat_weapon(sim, unit);
        } else if self.st.in_form(sim, BEAR) {
            sim.unit_mut(unit).auto_attacks.mh = forms::bear_weapon(sim, unit);
        }
    }

    fn initialize(&mut self, sim: &mut Sim, _unit: UnitId) {
        self.initialize_druid(sim);
        match self.spec {
            Spec::Balance => self.register_balance_spells(sim),
            Spec::FeralCat => self.register_feral_cat_spells(sim),
            Spec::FeralBear => self.register_feral_tank_spells(sim),
            Spec::Restoration => {
                self.register_heals(sim);
                self.register_bloom(sim);
                self.register_natures_swiftness(sim);
            }
        }
    }

    fn reset(&mut self, sim: &mut Sim, unit: UnitId) {
        self.st.set_form(sim, self.st.starting_form);
        match self.spec {
            Spec::Balance | Spec::Restoration => {}
            Spec::FeralCat => {
                self.st.clear_form(sim, unit);
                if let Some(aura) = self.cat_form_aura {
                    sim.activate(aura);
                }
            }
            Spec::FeralBear => {
                self.st.clear_form(sim, unit);
                if let Some(aura) = self.bear_form_aura {
                    sim.activate(aura);
                }
            }
        }
        let _ = (HUMANOID, PowerBar::Mana);
    }

    fn apply_item_effect(&mut self, sim: &mut Sim, unit: UnitId, item: i32) -> bool {
        self.apply_idol_effect(sim, unit, item)
    }

    fn talents(&self) -> &Message {
        &self.talents
    }

    fn class_spells(&self) -> &'static [ClassSpellName] {
        CLASS_SPELLS
    }

    fn export_effects(
        &self,
        env: &crate::prepare::env::Environment,
        notes: &mut Vec<String>,
    ) -> Vec<serde_json::Value> {
        self.druid_effects(env, notes)
    }

    fn damage_effect(&self, sim: &Sim, spell: SpellId) -> Option<serde_json::Value> {
        self.druid_damage_effect(sim, spell)
    }

    fn custom_apl_action(&self, _sim: &Sim, _unit: UnitId, action: &Message) -> Option<bool> {
        // Go: the cat's and the bear's NewAPLAction build their optimal rotation action; the
        // other specs build nothing for it.
        match action.oneof("action") {
            Some(("cat_optimal_rotation_action", _)) => Some(self.spec == Spec::FeralCat),
            Some(("bear_optimal_rotation_action", _)) => Some(self.spec == Spec::FeralBear),
            _ => None,
        }
    }

    fn swing_replacement_keeps_swing(&self) -> bool {
        false
    }

    /// tools/oracle-v2/druid_feral.go `druidStatAuras`: Cat Form changes stats through
    /// AddStatsDynamic and its stat dependencies, and a bear's Enrage cuts its armor. Bear Form
    /// stays up for the whole fight of a bear, so its stats are the base ones.
    fn stat_auras(&self, sim: &Sim, unit: UnitId) -> Vec<String> {
        let mut labels = Vec::new();
        if self.cat_form.is_some() && sim.get_aura(unit, "Cat Form").is_some() {
            labels.push("Cat Form".to_string());
        }
        if let Some(aura) = self.st.enrage_aura.get() {
            labels.push(sim.aura(aura).label.clone());
        }
        // barkskin.go: the physical damage taken cut changes the target's swings.
        if self.barkskin.is_some() && sim.get_aura(unit, "Barkskin").is_some() {
            labels.push("Barkskin".to_string());
        }
        // forms.go: Bear Form's stats, armor and health, which the target's swings read.
        if let (Some(_), Some(aura)) = (self.bear_form, self.bear_form_aura) {
            labels.push(sim.aura(aura).label.clone());
        }
        labels
    }

    /// Rend and Tear registers one dynamic damage taken modifier on every target.
    fn damage_taken_modifiers(&self) -> usize {
        usize::from(self.tal.rend_and_tear > 0)
    }
}

/// The action ID Go writes for a spell with a tag.
pub(super) fn with_tag(id: i32, tag: i32) -> ActionId {
    ActionId {
        spell_id: id,
        tag,
        ..ActionId::default()
    }
}
