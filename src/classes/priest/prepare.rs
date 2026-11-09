//! Priest preparation: Go sim/priest's construction and initialization, and the exporter's
//! Priest description (tools/oracle-v2/priest.go).

mod export;
mod heals;
pub(crate) mod items;
pub(crate) mod masks;
mod shadowfiend;
mod spell_data;
mod spells;
mod talents;

use crate::contracts::request::Message;
use crate::prepare::agent::{fill_talents, ClassSpellName, PrepAgent};
use crate::prepare::sim::{AuraId, Sim, SpellId, UnitId};
use crate::prepare::stats::Stat;
use crate::prepare::Refusal;

/// Go priest.TalentTreeSizes.
const TALENT_TREE_SIZES: [usize; 3] = [18, 17, 17];

/// Go `Inner Fire` rank 7's armor, which the charges never take away on a caster that is not
/// hit.
const INNER_FIRE_ARMOR: f64 = 1580.0;

/// tools/oracle-v2/priest.go `priestClassSpells`.
static CLASS_SPELLS: &[ClassSpellName] = &[
    ClassSpellName {
        mask: masks::DEVOURING_PLAGUE,
        name: "devouring_plague",
    },
    ClassSpellName {
        mask: masks::DEVOURING_PLAGUE_DOT,
        name: "devouring_plague_dot",
    },
    ClassSpellName {
        mask: masks::DEVOURING_PLAGUE_HEAL,
        name: "devouring_plague_heal",
    },
    ClassSpellName {
        mask: masks::HOLY_NOVA,
        name: "holy_nova",
    },
    ClassSpellName {
        mask: masks::HOLY_FIRE,
        name: "holy_fire",
    },
    ClassSpellName {
        mask: masks::MIND_BLAST,
        name: "mind_blast",
    },
    ClassSpellName {
        mask: masks::MIND_FLAY,
        name: "mind_flay",
    },
    ClassSpellName {
        mask: masks::PENANCE,
        name: "penance",
    },
    ClassSpellName {
        mask: masks::POWER_INFUSION,
        name: "power_infusion",
    },
    ClassSpellName {
        mask: masks::STARSHARDS,
        name: "starshards",
    },
    ClassSpellName {
        mask: masks::SHADOWFORM,
        name: "shadowform",
    },
    ClassSpellName {
        mask: masks::SHADOW_WORD_DEATH,
        name: "shadow_word_death",
    },
    ClassSpellName {
        mask: masks::SHADOW_WORD_PAIN,
        name: "shadow_word_pain",
    },
    ClassSpellName {
        mask: masks::SHADOWFIEND,
        name: "shadowfiend",
    },
    ClassSpellName {
        mask: masks::VAMPIRIC_EMBRACE,
        name: "vampiric_embrace",
    },
    ClassSpellName {
        mask: masks::FADE,
        name: "fade",
    },
    ClassSpellName {
        mask: masks::SMITE,
        name: "smite",
    },
    ClassSpellName {
        mask: masks::CHASTISE,
        name: "chastise",
    },
    ClassSpellName {
        mask: masks::CONFOUNDING_FLASH,
        name: "confounding_flash",
    },
    ClassSpellName {
        mask: masks::CONTINGENCY_PLAN,
        name: "contingency_plan",
    },
    ClassSpellName {
        mask: masks::DARK_SACRIFICE,
        name: "dark_sacrifice",
    },
    ClassSpellName {
        mask: masks::DIVINE_GRACE,
        name: "divine_grace",
    },
    ClassSpellName {
        mask: masks::HEAL,
        name: "heal",
    },
];

/// Go `Priest`: the talents, the class options and what the registrations keep.
#[derive(Clone)]
pub(crate) struct Priest {
    talents: Message,
    /// Go `SelfBuffs.UseShadowfiend`.
    use_shadowfiend: bool,
    /// Go `SelfBuffs.PreShadowform`.
    pre_shadowform: bool,
    /// Go `ShadowfiendPet`.
    shadowfiend_pet: UnitId,
    /// Go `ShadowfiendAura`, `Shadowfiend`, `InnerFocusAura`, `VampiricEmbrace`,
    /// `SearingLightAura`, `ShadowformAura` and `ShadowWeavingAura`.
    shadowfiend_aura: Option<AuraId>,
    shadowfiend: Option<SpellId>,
    inner_focus_aura: Option<AuraId>,
    vampiric_embrace: Option<SpellId>,
    searing_light_aura: Option<AuraId>,
    shadowform_aura: Option<AuraId>,
    shadow_weaving_aura: Option<AuraId>,
    /// Go `HolyFire`: every Holy Fire rank the priest knows.
    holy_fire: Vec<SpellId>,
    /// Every Shadow Word: Pain rank the priest knows, which Power in Light also reads.
    shadow_word_pain: Vec<SpellId>,
    /// A `healer_priest`: the heals and heal talents of MythicSim's healer sims (Rust only).
    healer: bool,
    heals: Vec<crate::prepare::heals::HealRank>,
}

/// Go `NewPriest` and `NewHealerPriest`: the spec decides where the class options are read.
pub(crate) fn new_priest(
    sim: &mut Sim,
    unit: UnitId,
    player: &Message,
) -> Result<Box<dyn PrepAgent>, Refusal> {
    let (use_shadowfiend, pre_shadowform, armor) = if let Some(dps) = player.message("dps_priest") {
        let options = dps
            .message("options")
            .and_then(|options| options.message("class_options"));
        match options {
            Some(options) => (
                options.bool("use_shadowfiend"),
                options.bool("pre_shadowform"),
                options.enum_name("armor"),
            ),
            None => (false, false, "NoArmor".to_string()),
        }
    } else if let Some(healer) = player.message("healer_priest") {
        // Gear planner only: the healer always has the Shadowfiend and takes only the armor.
        let armor = healer
            .message("options")
            .and_then(|options| options.message("class_options"))
            .map_or_else(
                || "NoArmor".to_string(),
                |options| options.enum_name("armor"),
            );
        (true, false, armor)
    } else {
        return Err(Refusal::new(
            "class_option",
            "a priest without a spec".to_string(),
        ));
    };

    let talents = fill_talents(
        "proto.PriestTalents",
        player.str("talents_string"),
        TALENT_TREE_SIZES,
    )
    .map_err(|err| Refusal::new("talents", err))?;
    sim.enable_mana_bar(unit);
    if armor == "InnerFire" {
        sim.add_stat(unit, Stat::Armor, INNER_FIRE_ARMOR);
    }
    let crit_per_agi = sim.crit_per_agi(unit);
    sim.unit_mut(unit).sdm.add_stat_dependency(
        Stat::Agility,
        Stat::PhysicalCritPercent,
        crit_per_agi,
    );
    let shadowfiend_pet = shadowfiend::new_shadowfiend(sim, unit);

    Ok(Box::new(Priest {
        talents,
        use_shadowfiend,
        pre_shadowform,
        shadowfiend_pet,
        shadowfiend_aura: None,
        shadowfiend: None,
        inner_focus_aura: None,
        vampiric_embrace: None,
        searing_light_aura: None,
        shadowform_aura: None,
        shadow_weaving_aura: None,
        holy_fire: Vec::new(),
        shadow_word_pain: Vec::new(),
        healer: player.message("healer_priest").is_some(),
        heals: Vec::new(),
    }))
}

impl PrepAgent for Priest {
    /// Divine Spirit is baseline in Forever, and the raid gets plain Prayer of Fortitude.
    fn add_raid_buffs(&self, raid_buffs: &mut Message) {
        raid_buffs.set_bool("prayer_of_shadow_protection", true);
        raid_buffs.set_bool("prayer_of_spirit", true);
        raid_buffs.set_bool("prayer_of_fortitude", true);
    }

    fn apply_talents(&mut self, sim: &mut Sim, unit: UnitId) {
        self.apply_priest_talents(sim, unit);
        if self.healer {
            self.apply_heal_talents(sim, unit);
        }
    }

    /// Go `Priest.Initialize`, and the healer's heals.
    fn initialize(&mut self, sim: &mut Sim, unit: UnitId) {
        self.register_spells(sim, unit);
        if self.healer {
            self.register_heals(sim, unit);
        }
    }

    /// Go `Shadowfiend.Initialize`.
    fn initialize_pet(&mut self, sim: &mut Sim, pet: UnitId) {
        shadowfiend::initialize(sim, pet);
    }

    /// Go `Shadowfiend.Reset`: the pet is dismissed.
    fn reset_pet(&mut self, sim: &mut Sim, pet: UnitId) {
        shadowfiend::reset(sim, pet);
    }

    /// talents_shadow.go applyDarkInspiration: its Spirit multiplier is a stat aura.
    fn stat_auras(&self, _sim: &Sim, _unit: UnitId) -> Vec<String> {
        if self.talents.i32("dark_inspiration") > 0 {
            vec!["Dark Inspiration".to_string()]
        } else {
            Vec::new()
        }
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
        unrepresented: &mut Vec<String>,
    ) -> Vec<serde_json::Value> {
        export::effects(self, env, unrepresented)
    }

    /// tools/oracle-v2/priest.go `priestSummonedShadowfiend`: the Shadowfiend is summoned during
    /// a fight when the summon spell exists.
    fn summoned_pet(&self, _sim: &Sim, pet: UnitId) -> bool {
        pet == self.shadowfiend_pet && self.shadowfiend.is_some()
    }

    /// tools/oracle-v2/priest.go `priestInertPet`.
    fn inert_pet(&self, sim: &Sim, pet: UnitId) -> Option<&'static str> {
        if pet != self.shadowfiend_pet
            || self.shadowfiend.is_some()
            || sim.pet_data(pet).enabled_on_start
        {
            return None;
        }
        Some("no registered spell summons the Shadowfiend")
    }

    /// Go `Priest.EurekaSpells`: Eureka! takes the direct spells and the channels off the cost
    /// and damage lists and the channels off the periodic one.
    fn eureka_spells(&self) -> Option<crate::prepare::racials::EurekaSpells> {
        let direct = masks::HOLY_FIRE
            | masks::HOLY_NOVA
            | masks::MIND_BLAST
            | masks::SHADOW_WORD_DEATH
            | masks::SMITE;
        let channels = masks::MIND_FLAY | masks::PENANCE | masks::STARSHARDS;
        Some(crate::prepare::racials::EurekaSpells {
            cost: direct | channels,
            // Penance's first bolt lands with the cast, so it is on the direct list too.
            damage: direct | masks::PENANCE,
            tick: channels,
        })
    }

    fn damage_effect(
        &self,
        sim: &Sim,
        spell: crate::prepare::sim::SpellId,
    ) -> Option<serde_json::Value> {
        export::damage_effect(sim, spell)
    }
}
