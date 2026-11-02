//! Warrior preparation: Go sim/warrior's construction and initialization (warrior.go, the
//! abilities, stances and talents) and the exporter's Warrior description
//! (tools/oracle-v2/warrior.go).

mod export;
mod helpers;
pub(crate) mod items;
pub(crate) mod masks;
mod shouts;
mod spell_data;

/// The ranks of Charge and Intercept, which run the warrior to the target.
pub(crate) const RUN_SPELLS: [i32; 6] = [100, 6178, 11578, 20252, 20616, 20617];
mod spells;
mod stances;
mod talents;

use crate::contracts::request::Message;
use crate::prepare::agent::{fill_talents, ClassSpellName, PrepAgent};
use crate::prepare::attack::{AutoAttackOptions, Weapon};
use crate::prepare::character::constants::DODGE_RATING_PER_DODGE_PERCENT;
use crate::prepare::env::Environment;
use crate::prepare::rage::RageBarOptions;
use crate::prepare::sim::{Sim, StateSlot, UnitId};
use crate::prepare::spell::ProcMask;
use crate::prepare::stats::Stat;
use crate::prepare::Refusal;

use crate::prepare::common_effects::DIAMOND_FLASK_ITEM;

/// Go warrior.TalentTreeSizes.
const TALENT_TREE_SIZES: [usize; 3] = [17, 17, 18];

/// The exporter's `warriorClassSpells`: every class mask bit by a stable name.
static CLASS_SPELLS: &[ClassSpellName] = &[
    ClassSpellName {
        mask: masks::BATTLE_SHOUT,
        name: "battle_shout",
    },
    ClassSpellName {
        mask: masks::BERSERKER_RAGE,
        name: "berserker_rage",
    },
    ClassSpellName {
        mask: masks::RECKLESSNESS,
        name: "recklessness",
    },
    ClassSpellName {
        mask: masks::DEATH_WISH,
        name: "death_wish",
    },
    ClassSpellName {
        mask: masks::RETALIATION,
        name: "retaliation",
    },
    ClassSpellName {
        mask: masks::RETALIATION_HIT,
        name: "retaliation_hit",
    },
    ClassSpellName {
        mask: masks::SHIELD_WALL,
        name: "shield_wall",
    },
    ClassSpellName {
        mask: masks::LAST_STAND,
        name: "last_stand",
    },
    ClassSpellName {
        mask: masks::CHARGE,
        name: "charge",
    },
    ClassSpellName {
        mask: masks::INTERCEPT,
        name: "intercept",
    },
    ClassSpellName {
        mask: masks::DEMORALIZING_SHOUT,
        name: "demoralizing_shout",
    },
    ClassSpellName {
        mask: masks::BATTLE_STANCE,
        name: "battle_stance",
    },
    ClassSpellName {
        mask: masks::BERSERKER_STANCE,
        name: "berserker_stance",
    },
    ClassSpellName {
        mask: masks::DEFENSIVE_STANCE,
        name: "defensive_stance",
    },
    ClassSpellName {
        mask: masks::REND,
        name: "rend",
    },
    ClassSpellName {
        mask: masks::DEEP_WOUNDS,
        name: "deep_wounds",
    },
    ClassSpellName {
        mask: masks::SWEEPING_STRIKES,
        name: "sweeping_strikes",
    },
    ClassSpellName {
        mask: masks::SWEEPING_STRIKES_HIT,
        name: "sweeping_strikes_hit",
    },
    ClassSpellName {
        mask: masks::SWEEPING_STRIKES_NORMALIZED_HIT,
        name: "sweeping_strikes_normalized_hit",
    },
    ClassSpellName {
        mask: masks::HEROIC_STRIKE,
        name: "heroic_strike",
    },
    ClassSpellName {
        mask: masks::CLEAVE,
        name: "cleave",
    },
    ClassSpellName {
        mask: masks::EXECUTE,
        name: "execute",
    },
    ClassSpellName {
        mask: masks::OVERPOWER,
        name: "overpower",
    },
    ClassSpellName {
        mask: masks::REVENGE,
        name: "revenge",
    },
    ClassSpellName {
        mask: masks::SLAM,
        name: "slam",
    },
    ClassSpellName {
        mask: masks::SUNDER_ARMOR,
        name: "sunder_armor",
    },
    ClassSpellName {
        mask: masks::THUNDER_CLAP,
        name: "thunder_clap",
    },
    ClassSpellName {
        mask: masks::WHIRLWIND,
        name: "whirlwind",
    },
    ClassSpellName {
        mask: masks::WHIRLWIND_OH,
        name: "whirlwind_off_hand",
    },
    ClassSpellName {
        mask: masks::SHIELD_SLAM,
        name: "shield_slam",
    },
    ClassSpellName {
        mask: masks::CONCUSSION_BLOW,
        name: "concussion_blow",
    },
    ClassSpellName {
        mask: masks::SHIELD_BASH,
        name: "shield_bash",
    },
    ClassSpellName {
        mask: masks::BLOODTHIRST,
        name: "bloodthirst",
    },
    ClassSpellName {
        mask: masks::MORTAL_STRIKE,
        name: "mortal_strike",
    },
    ClassSpellName {
        mask: masks::SHIELD_BLOCK,
        name: "shield_block",
    },
    ClassSpellName {
        mask: masks::HAMSTRING,
        name: "hamstring",
    },
    ClassSpellName {
        mask: masks::PUMMEL,
        name: "pummel",
    },
    ClassSpellName {
        mask: masks::MOCKING_BLOW,
        name: "mocking_blow",
    },
    ClassSpellName {
        mask: masks::CHALLENGING_SHOUT,
        name: "challenging_shout",
    },
    ClassSpellName {
        mask: masks::INTIMIDATING_SHOUT,
        name: "intimidating_shout",
    },
    ClassSpellName {
        mask: masks::DISARM,
        name: "disarm",
    },
    ClassSpellName {
        mask: masks::TAUNT,
        name: "taunt",
    },
    ClassSpellName {
        mask: masks::VICTORY_RUSH,
        name: "victory_rush",
    },
    ClassSpellName {
        mask: masks::SPEARING_STRIKE,
        name: "spearing_strike",
    },
];

/// Go `WarriorInputs`, from the spec's class options.
#[derive(Clone, Debug)]
pub(crate) struct Inputs {
    pub use_battle_shout: bool,
    /// Go `proto.WarriorStance`, as the enum's name.
    pub default_stance: String,
    pub starting_rage: f64,
    pub queue_delay: i32,
    pub stance_snapshot: bool,
    pub has_bs_t2: bool,
}

/// Go `Warrior`: the talents and inputs the registrations read, and the spec.
#[derive(Clone)]
pub(crate) struct Warrior {
    talents: Message,
    inputs: Inputs,
    /// Go `Spec == proto.Spec_SpecDpsWarrior`.
    dps_spec: bool,
    /// Go `HasBsT2`, which Battlegear of Wrath's 3 piece sets when its aura is gained.
    has_bs_t2: StateSlot,
    /// Go `thunderClapEffectBonus`, which Conqueror's Battlegear's 5 piece raises.
    thunder_clap_effect_bonus: StateSlot,
    /// Effects on the attack tables, which exist only once the environment finalizes.
    post_finalize: std::cell::RefCell<Vec<crate::prepare::env::FinalizeEffect>>,
}

impl Warrior {
    fn talent(&self, name: &str) -> i32 {
        self.talents.i32(name)
    }

    fn has_talent(&self, name: &str) -> bool {
        self.talents.bool(name)
    }
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

/// Go `NewDpsWarrior` and `NewProtectionWarrior`, with `NewWarrior`.
pub(crate) fn new_warrior(
    sim: &mut Sim,
    unit: UnitId,
    player: &Message,
) -> Result<Box<dyn PrepAgent>, Refusal> {
    let (dps_spec, class_options) = match player.oneof("spec") {
        Some(("dps_warrior", crate::contracts::request::Value::Message(spec))) => (true, spec),
        Some(("protection_warrior", crate::contracts::request::Value::Message(spec))) => {
            (false, spec)
        }
        _ => {
            return Err(Refusal::new(
                "spec",
                "a warrior without a Warrior spec".to_string(),
            ))
        }
    };
    // Go reads Options.ClassOptions without a check, so a request without them is a failure.
    let class_options = class_options
        .message("options")
        .and_then(|options| options.message("class_options"))
        .ok_or_else(|| {
            Refusal::new(
                "class_option",
                "a warrior without its class options".to_string(),
            )
        })?;
    let inputs = Inputs {
        use_battle_shout: class_options.bool("use_battle_shout"),
        default_stance: class_options.enum_name("default_stance"),
        starting_rage: class_options.f64("starting_rage"),
        queue_delay: class_options.i32("queue_delay"),
        stance_snapshot: class_options.bool("stance_snapshot"),
        has_bs_t2: class_options.bool("has_bs_t2"),
    };
    let talents = fill_talents(
        "proto.WarriorTalents",
        player.str("talents_string"),
        TALENT_TREE_SIZES,
    )
    .map_err(|err| Refusal::new("talents", err))?;
    sim.enable_rage_bar(
        unit,
        RageBarOptions {
            max_rage: 100.0,
            base_rage_multiplier: 1.0,
            starting_rage: inputs.starting_rage,
        },
    );

    let main_hand = weapon_from_main_hand(sim, unit);
    let off_hand = weapon_from_off_hand(sim, unit);
    sim.enable_auto_attacks(
        unit,
        AutoAttackOptions {
            main_hand,
            off_hand,
            auto_swing_melee: true,
            replace_mh_swing: true,
            proc_mask: ProcMask::UNKNOWN,
            ..AutoAttackOptions::default()
        },
    );

    sim.unit_mut(unit).pseudo_stats.can_parry = true;

    // Strength is two attack power, a block value of a twentieth less one, and the usual
    // agility and armor dependencies.
    sim.unit_mut(unit)
        .sdm
        .add_stat_dependency(Stat::Strength, Stat::AttackPower, 2.0);
    sim.unit_mut(unit)
        .sdm
        .add_stat_dependency(Stat::Strength, Stat::BlockValue, 1.0 / 20.0);
    sim.add_stat(unit, Stat::BlockValue, -1.0);
    let crit_per_agi = sim.crit_per_agi_max_level(unit);
    sim.unit_mut(unit).sdm.add_stat_dependency(
        Stat::Agility,
        Stat::PhysicalCritPercent,
        crit_per_agi,
    );
    sim.unit_mut(unit).sdm.add_stat_dependency(
        Stat::Agility,
        Stat::DodgeRating,
        crit_per_agi * DODGE_RATING_PER_DODGE_PERCENT,
    );
    sim.unit_mut(unit)
        .sdm
        .add_stat_dependency(Stat::BonusArmor, Stat::Armor, 1.0);

    // warrior.queuedRealismICD's timer.
    sim.new_timer(unit);

    Ok(Box::new(Warrior {
        has_bs_t2: sim.new_state_slot(if inputs.has_bs_t2 { 1.0 } else { 0.0 }),
        thunder_clap_effect_bonus: sim.new_state_slot(0.0),
        post_finalize: std::cell::RefCell::default(),
        talents,
        inputs,
        dps_spec,
    }))
}

impl PrepAgent for Warrior {
    fn take_post_finalize_effects(&mut self) -> Vec<crate::prepare::env::FinalizeEffect> {
        self.post_finalize.take()
    }

    fn apply_talents(&mut self, sim: &mut Sim, unit: UnitId) {
        self.register_arms_talents(sim, unit);
        self.register_fury_talents(sim, unit);
        self.register_protection_talents(sim, unit);
    }

    /// Go `Warrior.Initialize`.
    fn initialize(&mut self, sim: &mut Sim, unit: UnitId) {
        self.register_recklessness(sim, unit);
        self.register_shield_wall(sim, unit);
        self.register_retaliation(sim, unit);

        self.register_berserker_rage(sim, unit);
        self.register_bloodrage(sim, unit);
        self.register_charge(sim, unit);
        self.register_intercept(sim, unit);
        self.register_pummel(sim, unit);
        self.register_hamstring(sim, unit);
        self.register_disarm(sim, unit);
        self.register_taunt(sim, unit);

        self.register_rend(sim, unit);
        self.register_sunder_armor(sim, unit);
        self.register_heroic_strike(sim, unit);
        self.register_cleave(sim, unit);
        self.register_overpower(sim, unit);
        self.register_slam(sim, unit);
        self.register_whirlwind(sim, unit);
        self.register_execute(sim, unit);
        self.register_thunder_clap(sim, unit);
        self.register_revenge(sim, unit);
        self.register_shield_block(sim, unit);
        self.register_shield_bash(sim, unit);
        self.register_mocking_blow(sim, unit);
        self.register_victory_rush(sim, unit);

        self.register_stances(sim, unit);
        self.register_battle_shout(sim, unit);
        self.register_demoralizing_shout(sim, unit);
        self.register_challenging_shout(sim, unit);
        self.register_intimidating_shout(sim, unit);
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

    fn export_effects(&self, env: &Environment, notes: &mut Vec<String>) -> Vec<serde_json::Value> {
        self.all_effects(env, notes)
    }

    fn unrepresented(&self, _sim: &Sim, _unit: UnitId) -> Vec<String> {
        // warrior.go warriorUnrepresented: stance snapshots.
        if self.inputs.stance_snapshot {
            return vec!["warrior stance snapshots are unsupported".to_string()];
        }
        Vec::new()
    }

    /// The Warrior auras whose gain and expiry change stats through `AddStatsDynamic`, and
    /// Shield Block's block chance when the rotation names it.
    fn stat_auras(&self, sim: &Sim, unit: UnitId) -> Vec<String> {
        let mut labels: Vec<String> = [
            "Recklessness",
            "Berserker Stance",
            "Battle Shout (Player)",
            "Last Stand",
        ]
        .iter()
        .map(|label| label.to_string())
        .collect();
        if export::rotation_names_shield_block(sim, unit) {
            labels.push("Shield Block".to_string());
        }
        labels
    }

    fn eureka_spells(&self) -> Option<crate::prepare::racials::EurekaSpells> {
        Some(items::eureka_spells())
    }

    fn apply_item_effect(&mut self, sim: &mut Sim, unit: UnitId, item: i32) -> bool {
        items::apply_item_effect(sim, unit, item)
    }

    /// sim/warrior/items.go registers the Diamond Flask's use with the exporter's class item
    /// hook.
    fn class_item_use_effect(
        &self,
        env: &Environment,
        spell: crate::prepare::sim::SpellId,
        item: i32,
    ) -> Option<serde_json::Value> {
        (item == DIAMOND_FLASK_ITEM)
            .then(|| items::diamond_flask_use(env, spell))
            .flatten()
    }
}
