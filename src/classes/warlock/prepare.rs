//! Warlock preparation: Go sim/warlock's construction and initialization, and the exporter's
//! Warlock description (tools/oracle-v2/warlock.go).

mod armors;
mod curses;
mod export;
pub(crate) mod items;
pub(crate) mod masks;
mod pets;
mod spell_data;
mod spells;
mod talents;
mod weapon_stones;

use crate::contracts::request::Message;
use crate::prepare::agent::{fill_talents, ClassSpellName, PrepAgent};
use crate::prepare::attack::{AutoAttackOptions, Weapon};
use crate::prepare::env::Environment;
use crate::prepare::sim::{AuraId, Sim, SpellId, UnitId};
use crate::prepare::stats::Stat;
use crate::prepare::Refusal;

pub(crate) use pets::registered_native_summons;
use pets::WarlockPets;

/// Go warlock.TalentTreeSizes.
const TALENT_TREE_SIZES: [usize; 3] = [17, 19, 16];

/// tools/oracle-v2/warlock.go `warlockClassSpells`.
static CLASS_SPELLS: &[ClassSpellName] = &[
    ClassSpellName {
        mask: masks::CONFLAGRATE,
        name: "conflagrate",
    },
    ClassSpellName {
        mask: masks::SHADOW_BOLT,
        name: "shadow_bolt",
    },
    ClassSpellName {
        mask: masks::IMMOLATE,
        name: "immolate",
    },
    ClassSpellName {
        mask: masks::IMMOLATE_DOT,
        name: "immolate_dot",
    },
    ClassSpellName {
        mask: masks::INCINERATE,
        name: "incinerate",
    },
    ClassSpellName {
        mask: masks::SOUL_FIRE,
        name: "soul_fire",
    },
    ClassSpellName {
        mask: masks::SHADOW_BURN,
        name: "shadowburn",
    },
    ClassSpellName {
        mask: masks::LIFE_TAP,
        name: "life_tap",
    },
    ClassSpellName {
        mask: masks::CORRUPTION,
        name: "corruption",
    },
    ClassSpellName {
        mask: masks::CURSE_OF_AGONY,
        name: "bane_of_agony",
    },
    ClassSpellName {
        mask: masks::CURSE_OF_ELEMENTS,
        name: "curse_of_the_elements",
    },
    ClassSpellName {
        mask: masks::DRAIN_LIFE,
        name: "drain_life",
    },
    ClassSpellName {
        mask: masks::HELLFIRE,
        name: "hellfire",
    },
    ClassSpellName {
        mask: masks::IMMOLATION_AURA,
        name: "immolation_aura",
    },
    ClassSpellName {
        mask: masks::SEARING_PAIN,
        name: "searing_pain",
    },
    ClassSpellName {
        mask: masks::SUMMON_DOOMGUARD,
        name: "summon_doomguard",
    },
    ClassSpellName {
        mask: masks::DOOMGUARD_DOOM_BOLT,
        name: "doomguard_doom_bolt",
    },
    ClassSpellName {
        mask: masks::SUMMON_IMP,
        name: "summon_imp",
    },
    ClassSpellName {
        mask: masks::IMP_FIRE_BOLT,
        name: "imp_firebolt",
    },
    ClassSpellName {
        mask: masks::SUMMON_FELHUNTER,
        name: "summon_felhunter",
    },
    ClassSpellName {
        mask: masks::FELHUNTER_SHADOW_BITE,
        name: "felhunter_shadow_bite",
    },
    ClassSpellName {
        mask: masks::SUMMON_SUCCUBUS,
        name: "summon_succubus",
    },
    ClassSpellName {
        mask: masks::SUCCUBUS_LASH_OF_PAIN,
        name: "succubus_lash_of_pain",
    },
    ClassSpellName {
        mask: masks::VOIDWALKER_TORMENT,
        name: "voidwalker_torment",
    },
    ClassSpellName {
        mask: masks::SUMMON_INFERNAL,
        name: "summon_infernal",
    },
    ClassSpellName {
        mask: masks::RAIN_OF_FIRE,
        name: "rain_of_fire",
    },
    ClassSpellName {
        mask: masks::CURSE_OF_DOOM,
        name: "bane_of_doom",
    },
    ClassSpellName {
        mask: masks::CURSE_OF_RECKLESSNESS,
        name: "curse_of_recklessness",
    },
    ClassSpellName {
        mask: masks::CURSE_OF_WEAKNESS,
        name: "curse_of_weakness",
    },
    ClassSpellName {
        mask: masks::SIPHON_LIFE,
        name: "siphon_life",
    },
    ClassSpellName {
        mask: masks::DRAIN_SOUL,
        name: "drain_soul",
    },
    ClassSpellName {
        mask: masks::DEATH_COIL,
        name: "death_coil",
    },
    ClassSpellName {
        mask: masks::WRACK,
        name: "wrack",
    },
];

/// Go `Warlock`: the talents and options the registrations read, and the spells, auras and
/// demons Go keeps on the struct.
#[derive(Clone)]
pub(crate) struct Warlock {
    pub(super) talents: Message,
    /// Go `Options`: the `WarlockOptions` of the player.
    pub(super) options: Message,
    /// Go `BasePets`, `ActivePet` and the demons by name.
    pub(super) pets: WarlockPets,
    /// Go `CurseOfElementsAuras` and `CurseOfRecklessnessAuras`, by unit index.
    pub(super) curse_of_elements_auras: Vec<Option<AuraId>>,
    pub(super) curse_of_recklessness_auras: Vec<Option<AuraId>>,
    /// Go `DecimationAura`, `ImprovedShadowBoltAuras` and `DemonicBrandAuras`.
    pub(super) decimation_aura: Option<AuraId>,
    pub(super) improved_shadow_bolt_auras: Vec<Option<AuraId>>,
    pub(super) demonic_brand_auras: Vec<Option<AuraId>>,
    /// The spells the exporter reads the options of: Go `Warlock.Wrack` and friends.
    pub(super) wrack: Option<SpellId>,
}

/// Go `NewWarlock`.
pub(crate) fn new_warlock(
    sim: &mut Sim,
    unit: UnitId,
    player: &Message,
) -> Result<Box<dyn PrepAgent>, Refusal> {
    // Go reads options.GetWarlock().Options.ClassOptions, and the registrations read it without
    // a check.
    let options = player
        .message("warlock")
        .and_then(|warlock| warlock.message("options"))
        .and_then(|options| options.message("class_options"))
        .ok_or_else(|| {
            Refusal::new(
                "class_option",
                "a warlock without its class options".to_string(),
            )
        })?
        .clone();
    let talents = fill_talents(
        "proto.WarlockTalents",
        player.str("talents_string"),
        TALENT_TREE_SIZES,
    )
    .map_err(|err| Refusal::new("talents", err))?;

    sim.enable_mana_bar(unit);

    // Shoot is a shared ranged auto-attack (OtherActionShoot), not a Warlock
    // spell-family rank. Register the native ranged weapon only when an actual
    // wand is equipped; otherwise a caster with an empty ranged slot must not
    // acquire a phantom ranged attack. The common fight engine owns the hit
    // table, cast interruption and weapon damage.
    let wand = sim
        .ranged_weapon(unit)
        .filter(|item| item.ranged_weapon_type == "RangedWeaponTypeWand")
        .map(|item| Weapon::from_item(item, sim.unit(unit).pseudo_stats.bonus_ranged_dps));
    if let Some(ranged) = wand {
        sim.enable_auto_attacks(
            unit,
            AutoAttackOptions {
                ranged,
                auto_swing_ranged: true,
                ..AutoAttackOptions::default()
            },
        );
    }
    sim.unit_mut(unit)
        .sdm
        .add_stat_dependency(Stat::Strength, Stat::AttackPower, 1.0);
    let crit_per_agi = sim.crit_per_agi_max_level(unit);
    sim.unit_mut(unit).sdm.add_stat_dependency(
        Stat::Agility,
        Stat::PhysicalCritPercent,
        crit_per_agi,
    );

    let mut warlock = Warlock {
        talents,
        options,
        pets: WarlockPets::default(),
        curse_of_elements_auras: Vec::new(),
        curse_of_recklessness_auras: Vec::new(),
        decimation_aura: None,
        improved_shadow_bolt_auras: Vec::new(),
        demonic_brand_auras: Vec::new(),
        wrack: None,
    };
    if !warlock.options.bool("sacrifice_summon") {
        warlock.register_pets(sim, unit);
    }
    Ok(Box::new(warlock))
}

impl Warlock {
    /// Go `WarlockOptions.SacrificeSummon`.
    pub(super) fn sacrifice_summon(&self) -> bool {
        self.options.bool("sacrifice_summon")
    }

    /// Go `Warlock.AfflictionCount`: the Affliction aura of every kind the target holds, active
    /// or not.
    pub(super) fn affliction_count(sim: &Sim, target: UnitId) -> f64 {
        sim.auras_with_tag(target, "Affliction").len() as f64
    }
}

impl PrepAgent for Warlock {
    /// A summoned Imp gives the party Blood Pact (11767). Forever's Improved Imp (18694) no
    /// longer raises it.
    fn add_party_buffs(&self, _sim: &Sim, party_buffs: &mut Message) {
        if self.options.enum_name("summon") == "Imp" && !self.sacrifice_summon() {
            party_buffs.set_bool("blood_pact", true);
        }
    }

    /// `NewWarlock` drops the raid's own curse the warlock casts.
    fn adjust_raid_debuffs(&self, debuffs: &mut Message) {
        match self.options.enum_name("curse_options").as_str() {
            "Elements" => debuffs.set_bool("curse_of_elements", false),
            "Recklessness" => debuffs.set_bool("curse_of_recklessness", false),
            _ => {}
        }
    }

    fn apply_talents(&mut self, sim: &mut Sim, unit: UnitId) {
        self.register_affliction_talents(sim, unit);
        self.register_demonology_talents(sim, unit);
        self.register_destruction_talents(sim, unit);
    }

    /// Go `Warlock.Initialize`: curses and banes, the spells, the armors and the demons'
    /// abilities.
    fn initialize(&mut self, sim: &mut Sim, unit: UnitId) {
        self.register_weapon_stone(sim, unit);

        self.register_curse_of_elements(sim, unit);
        self.register_curse_of_doom(sim, unit);
        self.register_curse_of_agony(sim, unit);
        self.register_curse_of_recklessness(sim, unit);

        self.register_corruption(sim, unit);
        self.register_death_coil(sim, unit);
        self.register_drain_life(sim, unit);
        self.register_hellfire(sim, unit);
        self.register_immolate(sim, unit);
        self.register_incinerate(sim, unit);
        self.register_life_tap(sim, unit);
        self.register_rain_of_fire(sim, unit);
        self.register_shadow_bolt(sim, unit);
        self.register_searing_pain(sim, unit);
        self.register_siphon_life_spell(sim, unit);
        self.register_soulfire(sim, unit);
        self.register_wrack(sim, unit);

        self.register_armors(sim, unit);
        self.register_pet_abilities(sim);

        sim.unit_mut(unit).pseudo_stats.self_healing_multiplier = 1.0;
    }

    /// Under Forever's rules, The Black Book's use: the demon's aura and the damage done
    /// multiplier its row states for every school.
    fn class_item_use_effect(
        &self,
        env: &Environment,
        _spell: SpellId,
        item: i32,
    ) -> Option<serde_json::Value> {
        if item != 19337 || env.sim.rules != crate::prepare::sim::Rules::Forever {
            return None;
        }
        let row = crate::prepare::spelldata::must_find(items::BLACK_BOOK_BUFF);
        let multiplier: f64 = row
            .effects
            .iter()
            .filter(|effect| {
                effect.aura == crate::prepare::dbcenums::A_MOD_DAMAGE_PERCENT_DONE
                    && effect.misc == 127
            })
            .map(|effect| 1.0 + effect.base_points / 100.0)
            .product();
        Some(serde_json::json!({
            "kind": "black_book", "item_id": item, "aura": items::BLACK_BOOK_DEMON_AURA,
            "damage_multiplier": multiplier,
        }))
    }

    /// The warlock package's `core.NewItemEffect` calls.
    fn apply_item_effect(&mut self, sim: &mut Sim, unit: UnitId, item: i32) -> bool {
        match item {
            19337 => {
                items::black_book(sim, unit);
                true
            }
            19957 => {
                items::hazzarahs_charm_of_destruction(sim, unit);
                true
            }
            _ => false,
        }
    }

    fn talents(&self) -> &Message {
        &self.talents
    }

    fn class_spells(&self) -> &'static [ClassSpellName] {
        CLASS_SPELLS
    }

    fn export_effects(&self, env: &Environment, notes: &mut Vec<String>) -> Vec<serde_json::Value> {
        self.warlock_effects(env, notes)
    }

    fn unmasked_spell(&self, id: &crate::contracts::prepared_v2::ActionId) -> Option<&'static str> {
        export::unmasked_spell(id)
    }

    /// Go `Warlock.EurekaSpells`.
    fn eureka_spells(&self) -> Option<crate::prepare::racials::EurekaSpells> {
        let direct = masks::CONFLAGRATE
            | masks::DEATH_COIL
            | masks::IMMOLATE
            | masks::RAIN_OF_FIRE
            | masks::SEARING_PAIN
            | masks::SHADOW_BOLT
            | masks::SHADOW_BURN
            | masks::SOUL_FIRE;
        let channels = masks::DRAIN_LIFE | masks::DRAIN_SOUL | masks::WRACK | masks::HELLFIRE;
        Some(crate::prepare::racials::EurekaSpells {
            cost: direct | channels,
            damage: direct,
            tick: channels,
        })
    }

    fn damage_effect(&self, sim: &Sim, spell: SpellId) -> Option<serde_json::Value> {
        export::damage_effect(sim, spell)
    }

    fn reset_only_pets(&self) -> bool {
        true
    }

    /// The exporter's `warlockInertPet`: every demon is registered at construction and only the
    /// summoned one is enabled, at reset; the sim has no summon spells.
    fn inert_pet(&self, sim: &Sim, pet: UnitId) -> Option<&'static str> {
        if sim.pet_data(pet).enabled_on_start {
            None
        } else {
            Some("the warlock summoned another demon or none")
        }
    }

    fn custom_apl_action(&self, sim: &Sim, unit: UnitId, action: &Message) -> Option<bool> {
        self.custom_apl_action_impl(sim, unit, action)
    }
}
