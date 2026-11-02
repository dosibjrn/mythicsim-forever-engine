//! Shaman preparation: Go sim/shaman's construction and initialization (shaman.go, the spells,
//! totems, shields, imbues and talents, and the three specs) and the exporter's Shaman
//! description (tools/oracle-v2/shaman.go).

mod export;
mod heals;
mod imbues;
pub(crate) mod items;
pub(crate) mod masks;
mod shields;
mod spell_data;
mod spells;
mod talents;
mod totems;

use crate::contracts::request::{Message, Value};
use crate::prepare::agent::{fill_talents, ClassSpellName, PrepAgent};
use crate::prepare::attack::{AutoAttackOptions, Weapon};
use crate::prepare::character::constants::DODGE_RATING_PER_DODGE_PERCENT;
use crate::prepare::env::Environment;
use crate::prepare::sim::{AuraId, Rules, Sim, SpellId, StateSlot, UnitId};
use crate::prepare::spell::ProcMask;
use crate::prepare::stats::Stat;
use crate::prepare::Refusal;

use spell_data::spell_data;

/// Go shaman.TalentTreeSizes.
const TALENT_TREE_SIZES: [usize; 3] = [16, 18, 16];

/// tools/oracle-v2/shaman.go `shamanClassSpells`.
static CLASS_SPELLS: &[ClassSpellName] = &[
    // MythicSim's healer sims (Rust only): the Restoration shaman's heals.
    ClassSpellName {
        mask: masks::HEAL,
        name: "heal",
    },
    ClassSpellName {
        mask: masks::FLAME_SHOCK_DIRECT,
        name: "flame_shock_direct",
    },
    ClassSpellName {
        mask: masks::FLAME_SHOCK_DOT,
        name: "flame_shock_dot",
    },
    ClassSpellName {
        mask: masks::LIGHTNING_BOLT,
        name: "lightning_bolt",
    },
    ClassSpellName {
        mask: masks::LIGHTNING_BOLT_OVERLOAD,
        name: "lightning_bolt_overload",
    },
    ClassSpellName {
        mask: masks::CHAIN_LIGHTNING,
        name: "chain_lightning",
    },
    ClassSpellName {
        mask: masks::CHAIN_LIGHTNING_OVERLOAD,
        name: "chain_lightning_overload",
    },
    ClassSpellName {
        mask: masks::EARTH_SHOCK,
        name: "earth_shock",
    },
    ClassSpellName {
        mask: masks::LIGHTNING_SHIELD,
        name: "lightning_shield",
    },
    ClassSpellName {
        mask: masks::MAGMA_TOTEM,
        name: "magma_totem",
    },
    ClassSpellName {
        mask: masks::SEARING_TOTEM,
        name: "searing_totem",
    },
    ClassSpellName {
        mask: masks::FIRE_NOVA,
        name: "fire_nova",
    },
    ClassSpellName {
        mask: masks::FLAMETONGUE_TOTEM,
        name: "flametongue_totem",
    },
    ClassSpellName {
        mask: masks::STORMSTRIKE_CAST,
        name: "stormstrike_cast",
    },
    ClassSpellName {
        mask: masks::STORMSTRIKE_DAMAGE,
        name: "stormstrike_damage",
    },
    ClassSpellName {
        mask: masks::EARTH_SHIELD,
        name: "earth_shield",
    },
    ClassSpellName {
        mask: masks::FROST_SHOCK,
        name: "frost_shock",
    },
    ClassSpellName {
        mask: masks::FLAMETONGUE_WEAPON,
        name: "flametongue_weapon",
    },
    ClassSpellName {
        mask: masks::WINDFURY_WEAPON,
        name: "windfury_weapon",
    },
    ClassSpellName {
        mask: masks::FROSTBRAND_WEAPON,
        name: "frostbrand_weapon",
    },
    ClassSpellName {
        mask: masks::ROCKBITER_WEAPON,
        name: "rockbiter_weapon",
    },
    ClassSpellName {
        mask: masks::ELEMENTAL_MASTERY,
        name: "elemental_mastery",
    },
    ClassSpellName {
        mask: masks::SHAMANISTIC_RAGE,
        name: "shamanistic_rage",
    },
    ClassSpellName {
        mask: masks::BASIC_TOTEM,
        name: "basic_totem",
    },
    ClassSpellName {
        mask: masks::SHIELD_SELF_PROC,
        name: "shield_self_proc",
    },
    ClassSpellName {
        mask: masks::LAVA_BURST,
        name: "lava_burst",
    },
];

/// Go `SpellFlagShamanSpell`, `SpellFlagShock`, `SpellFlagInstant` and `SpellFlagFocusable`: the
/// agent reserved flags the class uses.
pub(crate) mod flags {
    use crate::prepare::spell::SpellFlag;

    pub(crate) const SHAMAN_SPELL: SpellFlag = SpellFlag::AGENT_RESERVED1;
    pub(crate) const SHOCK: SpellFlag = SpellFlag::AGENT_RESERVED2;
    pub(crate) const INSTANT: SpellFlag = SpellFlag::AGENT_RESERVED3;
    pub(crate) const FOCUSABLE: SpellFlag = SpellFlag::AGENT_RESERVED4;
}

/// Go `CastTagLightningOverload`: "1 to 5 are used by MaelstromWeapon Stacks".
pub(crate) const CAST_TAG_LIGHTNING_OVERLOAD: i32 = 6;

/// Which Shaman spec the player is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Spec {
    Elemental,
    Enhancement,
    Restoration,
}

/// Go `SelfBuffs`: which buffs this shaman is using, as the enums' names.
#[derive(Clone, Debug, Default)]
pub(crate) struct SelfBuffs {
    pub shield_procrate: f64,
    pub imbue_mh: String,
    pub imbue_oh: String,
    pub imbue_mh_swap: String,
    pub imbue_oh_swap: String,
}

/// Go `Shaman`: the talents and buffs the registrations read, and what Go keeps on the struct
/// that a later registration or the export reads.
#[derive(Clone)]
pub(crate) struct Shaman {
    talents: Message,
    spec: Spec,
    self_buffs: SelfBuffs,
    /// Go `EnhancementShaman.Options` is set (the exporter's `swingReplacementKeepsSwing`).
    enhancement_options: bool,
    /// Go `ShamanSyncType`, as the enum's name, for the weapon sync effect.
    sync_type: String,
    /// Go `WindfuryAPBonus`.
    windfury_ap_bonus: f64,
    /// Go `ChainLightningBounceBonus`, which Gift of the Gathering Storm's 3 piece raises.
    chain_lightning_bounce_bonus: StateSlot,
    /// Go `FlameShock`.
    flame_shock: Option<SpellId>,
    /// Go `MagmaTotem`.
    magma_totem: Option<SpellId>,
    /// Go `FlametongueTotemAura`.
    flametongue_totem_aura: Option<AuraId>,
    /// A Restoration shaman's heals, for the export.
    heals: Vec<crate::prepare::heals::HealRank>,
    /// Spirit Weapons' threat multiplier on a Rockbiter Weapon aura that follows the weapon in
    /// the hand, 0 when it is permanent or the talent is not taken.
    rockbiter_swap_threat: StateSlot,
}

/// Go `Character.WeaponFromMainHand`.
fn weapon_from_main_hand(sim: &Sim, unit: UnitId) -> Weapon {
    match sim.mh_weapon(unit) {
        Some(item) => Weapon::from_item(item, sim.unit(unit).pseudo_stats.bonus_mh_dps),
        None => Weapon::unarmed(),
    }
}

/// Go `Character.WeaponFromOffHand`.
fn weapon_from_off_hand(sim: &Sim, unit: UnitId) -> Weapon {
    match sim.oh_weapon(unit) {
        Some(item) => Weapon::from_item(item, sim.unit(unit).pseudo_stats.bonus_oh_dps),
        None => Weapon::default(),
    }
}

/// Go `NewElementalShaman`, `NewEnhancementShaman` and `NewRestorationShaman`, with
/// `NewShaman`.
pub(crate) fn new_shaman(
    sim: &mut Sim,
    unit: UnitId,
    player: &Message,
) -> Result<Box<dyn PrepAgent>, Refusal> {
    let (spec, spec_options) = match player.oneof("spec") {
        Some(("elemental_shaman", Value::Message(spec))) => (Spec::Elemental, spec),
        Some(("enhancement_shaman", Value::Message(spec))) => (Spec::Enhancement, spec),
        Some(("restoration_shaman", Value::Message(spec))) => (Spec::Restoration, spec),
        _ => {
            return Err(Refusal::new(
                "spec",
                "a shaman without a Shaman spec".to_string(),
            ))
        }
    };
    // Go reads Options.ClassOptions of an Elemental and an Enhancement shaman without a check,
    // and a Restoration shaman reads no options at all.
    let options = spec_options.message("options");
    let class_options = options.and_then(|options| options.message("class_options"));
    if spec != Spec::Restoration && class_options.is_none() {
        return Err(Refusal::new(
            "class_option",
            "a shaman without its class options".to_string(),
        ));
    }
    let mut self_buffs = SelfBuffs::default();
    let mut sync_type = String::new();
    if let (Some(class_options), Some(options)) = (class_options, options) {
        self_buffs.shield_procrate = class_options.f64("shield_procrate");
        self_buffs.imbue_mh = class_options.enum_name("imbue_mh");
        self_buffs.imbue_oh = "NoImbue".to_string();
        self_buffs.imbue_mh_swap = class_options.enum_name("imbue_mh_swap");
        self_buffs.imbue_oh_swap = "NoImbue".to_string();
        if spec == Spec::Enhancement {
            self_buffs.imbue_oh = options.enum_name("imbue_oh");
            self_buffs.imbue_oh_swap = options.enum_name("imbue_oh_swap");
            sync_type = options.enum_name("sync_type");
        }
    }
    if self_buffs.imbue_mh.is_empty() {
        self_buffs.imbue_mh = "NoImbue".to_string();
        self_buffs.imbue_mh_swap = "NoImbue".to_string();
        self_buffs.imbue_oh = "NoImbue".to_string();
        self_buffs.imbue_oh_swap = "NoImbue".to_string();
    }

    let talents = fill_talents(
        "proto.ShamanTalents",
        player.str("talents_string"),
        TALENT_TREE_SIZES,
    )
    .map_err(|err| Refusal::new("talents", err))?;

    // Go NewShaman.
    sim.unit_mut(unit)
        .sdm
        .add_stat_dependency(Stat::BonusArmor, Stat::Armor, 1.0);
    let crit_per_agi = sim.crit_per_agi_max_level(unit);
    sim.unit_mut(unit).sdm.add_stat_dependency(
        Stat::Agility,
        Stat::PhysicalCritPercent,
        crit_per_agi,
    );
    // Dodge per agility equals crit per agility, as in Classic (20 agility at 60, not TBC's 25).
    sim.unit_mut(unit).sdm.add_stat_dependency(
        Stat::Agility,
        Stat::DodgeRating,
        crit_per_agi * DODGE_RATING_PER_DODGE_PERCENT,
    );
    sim.enable_mana_bar(unit);
    sim.unit_mut(unit)
        .sdm
        .add_stat_dependency(Stat::Strength, Stat::AttackPower, 2.0);
    // 16361, 333 at 60.
    let windfury_ap_bonus = spell_data()
        .windfury_weapon_triggered
        .highest()
        .effect_n(1)
        .average(crate::prepare::character::constants::CHARACTER_LEVEL);

    let mut shaman = Shaman {
        talents,
        spec,
        self_buffs,
        enhancement_options: spec == Spec::Enhancement && options.is_some(),
        sync_type,
        windfury_ap_bonus,
        chain_lightning_bounce_bonus: sim.new_state_slot(0.0),
        rockbiter_swap_threat: sim.new_state_slot(0.0),
        flame_shock: None,
        magma_totem: None,
        flametongue_totem_aura: None,
        heals: Vec::new(),
    };

    match spec {
        Spec::Elemental => {
            // Some spells use weapon damage (Unleash Wind, ...).
            let main_hand = weapon_from_main_hand(sim, unit);
            sim.enable_auto_attacks(
                unit,
                AutoAttackOptions {
                    main_hand,
                    auto_swing_melee: false,
                    proc_mask: ProcMask::UNKNOWN,
                    ..AutoAttackOptions::default()
                },
            );
        }
        Spec::Enhancement => {
            let main_hand = weapon_from_main_hand(sim, unit);
            let off_hand = weapon_from_off_hand(sim, unit);
            // ApplySyncType sets a main hand swing replacement for every sync type but none.
            let replace_mh_swing = matches!(
                shaman.sync_type.as_str(),
                "Auto" | "SyncMainhandOffhandSwings" | "DelayOffhandSwings"
            );
            sim.enable_auto_attacks(
                unit,
                AutoAttackOptions {
                    main_hand,
                    off_hand,
                    auto_swing_melee: true,
                    replace_mh_swing,
                    proc_mask: ProcMask::UNKNOWN,
                    ..AutoAttackOptions::default()
                },
            );
            if sim.mh_weapon(unit).is_none() {
                shaman.self_buffs.imbue_mh = "NoImbue".to_string();
            }
            if sim.oh_weapon(unit).is_none() {
                shaman.self_buffs.imbue_oh = "NoImbue".to_string();
            }
        }
        Spec::Restoration => {}
    }
    Ok(Box::new(shaman))
}

impl Shaman {
    fn talent(&self, name: &str) -> i32 {
        self.talents.i32(name)
    }

    fn has_talent(&self, name: &str) -> bool {
        self.talents.bool(name)
    }

    /// Go `GetImbueProcMask`.
    fn imbue_proc_mask(&self, imbue: &str) -> ProcMask {
        let mut mask = ProcMask::UNKNOWN;
        if self.self_buffs.imbue_mh == imbue || self.self_buffs.imbue_mh_swap == imbue {
            mask = ProcMask(mask.0 | ProcMask::MELEE_MH.0);
        }
        if self.self_buffs.imbue_oh == imbue {
            mask = ProcMask(mask.0 | ProcMask::MELEE_OH.0);
        }
        mask
    }

    /// Go `GetOverloadChance`.
    fn overload_chance(&self) -> f64 {
        if self.talent("lightning_overload") == 0 {
            return 0.0;
        }
        spell_data()
            .lightning_overload
            .fraction_at(self.talent("lightning_overload"))
    }
}

impl PrepAgent for Shaman {
    /// The talented Mana Tide Totem (16190) is the party buff's totem, dropped by this shaman.
    fn add_party_buffs(&self, _sim: &Sim, party_buffs: &mut Message) {
        if self.has_talent("mana_tide_totem") {
            party_buffs.set_i32("mana_tide_totems", party_buffs.i32("mana_tide_totems") + 1);
        }
    }

    fn apply_talents(&mut self, sim: &mut Sim, unit: UnitId) {
        self.register_elemental_talents(sim, unit);
        self.register_enhancement_talents(sim, unit);
        self.register_restoration_talents(sim, unit);
        if self.spec == Spec::Restoration {
            self.apply_heal_talents(sim, unit);
        }
    }

    /// Go `Shaman.Initialize` and the specs' own: the spells, then the imbues of an Enhancement
    /// shaman.
    fn initialize(&mut self, sim: &mut Sim, unit: UnitId) {
        self.register_chain_lightning_spell(sim, unit);
        self.register_lightning_bolt_spell(sim, unit);
        self.register_shields_spells(sim, unit);
        self.register_magma_totem_spell(sim, unit);
        self.register_searing_totem_spell(sim, unit);
        self.register_flametongue_totem_spell(sim, unit);
        self.register_fire_nova_spell(sim, unit);
        // A Restoration shaman has no weapon swing for Windfury Totem's attack, and heals.
        if self.spec == Spec::Restoration {
            self.register_heals(sim, unit);
        } else {
            self.register_windfury_totem_spell(sim, unit);
        }
        self.register_strength_of_earth_totem_spell(sim, unit);
        self.register_grace_of_air_totem_spell(sim, unit);
        self.register_mana_spring_totem_spell(sim, unit);
        // MythicSim's healer sims under Forever's rules: the Restoration shaman's own Mana Tide.
        if self.spec == Spec::Restoration
            && sim.rules == Rules::Forever
            && self.has_talent("mana_tide_totem")
        {
            self.register_mana_tide_totem_spell(sim, unit);
        }
        self.register_shocks(sim, unit);

        if self.spec == Spec::Enhancement {
            // In the Initialize due to frost brand adding the aura to the enemy.
            let frostbrand = self.imbue_proc_mask("FrostbrandWeapon");
            self.register_frostbrand_imbue(sim, unit, frostbrand);
            let flametongue = self.imbue_proc_mask("FlametongueWeapon");
            self.register_flametongue_imbue(sim, unit, flametongue);
            let windfury = self.imbue_proc_mask("WindfuryWeapon");
            self.register_windfury_imbue(sim, unit, windfury);
            let rockbiter = self.imbue_proc_mask("RockbiterWeapon");
            self.register_rockbiter_imbue(sim, unit, rockbiter);
        }
    }

    /// Go `Shaman.Reset`: the totem expirations are not part of the prepared state.
    fn reset(&mut self, _sim: &mut Sim, _unit: UnitId) {}

    fn apply_item_effect(&mut self, sim: &mut Sim, unit: UnitId, item: i32) -> bool {
        items::apply_item_effect(sim, unit, item)
    }

    fn talents(&self) -> &Message {
        &self.talents
    }

    fn class_spells(&self) -> &'static [ClassSpellName] {
        CLASS_SPELLS
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn swing_replacement_keeps_swing(&self) -> bool {
        self.enhancement_options
    }

    /// Go `buffs.SetFlametongueAttackTraits` for the Shaman: a hit carries Flametongue
    /// Weapon's class mask and the shaman spell flag.
    fn flametongue_attack_traits(
        &self,
    ) -> crate::prepare::buffs::flametongue::FlametongueAttackTraits {
        flametongue_traits()
    }

    fn export_effects(&self, env: &Environment, notes: &mut Vec<String>) -> Vec<serde_json::Value> {
        self.shaman_effects(env, notes)
    }

    fn unrepresented(&self, _sim: &Sim, _unit: UnitId) -> Vec<String> {
        self.shaman_unrepresented()
    }

    /// The Shaman auras whose gain and expiry change stats through `AddStatsDynamic`.
    fn stat_auras(&self, sim: &Sim, _unit: UnitId) -> Vec<String> {
        let mut labels: Vec<String> = [
            "Strength Of Earth Totem (Self)",
            "Grace Of Air Totem (Self)",
            "Mana Spring Totem (Self)",
            "Mana Tide Totem (Self)",
            "Windfury Totem Proc (Self)",
        ]
        .iter()
        .map(|label| label.to_string())
        .collect();
        // Stormcaller's Garb's proc aura, which Forever's rules describe.
        if sim.rules == crate::prepare::sim::Rules::Forever {
            labels.push("Stormcaller's Wrath".to_string());
        }
        labels
    }

    fn damage_effect(&self, sim: &Sim, spell: SpellId) -> Option<serde_json::Value> {
        export::damage_effect(sim, spell)
    }
}

/// What a Shaman's own Flametongue Attack carries (Go `init` in weapon_imbues.go).
pub(crate) fn flametongue_traits() -> crate::prepare::buffs::flametongue::FlametongueAttackTraits {
    crate::prepare::buffs::flametongue::FlametongueAttackTraits {
        class_spell_mask: 0,
        flags: flags::SHAMAN_SPELL,
    }
}
