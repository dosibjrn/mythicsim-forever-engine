//! Go `sim/warlock` `agony.go`, `doom.go`, `curse_of_elements.go`, `curse_of_recklessness.go`
//! and the APL value and action the warlock adds for its assigned curse (`apl_values.go`).

use crate::contracts::request::Message;
use crate::prepare::buffs::generated::{CURSE_OF_ELEMENTS, CURSE_OF_RECKLESSNESS};
use crate::prepare::sim::{AuraConfig, AuraId, Cooldown, Sim, UnitId, UnitType};
use crate::prepare::spell::{
    CastConfig, DotConfig, LabeledAuraArrays, ProcMask, SpellConfig, SpellFlag,
};

use super::masks;
use super::spell_data::spell_data;
use super::spells::{actor_rank, damage_config, default_cast, flat_cost, longest_cooldown, spell_action};
use super::Warlock;

/// Go `NewEnemyAuraArray`: one aura for each enemy, by unit index.
pub(super) fn new_enemy_aura_array(
    sim: &mut Sim,
    mut make: impl FnMut(&mut Sim, UnitId) -> AuraId,
) -> Vec<Option<AuraId>> {
    let units = sim.env_units.clone();
    let mut auras = vec![None; units.len()];
    for target in units {
        if sim.unit(target).unit_type == UnitType::Enemy {
            let aura = make(sim, target);
            auras[sim.unit(target).unit_index as usize] = Some(aura);
        }
    }
    auras
}

/// Go `AuraArray.ToMap`: the array under the label of its first aura, or nothing when it holds
/// none.
pub(super) fn aura_array_to_map(sim: &Sim, auras: &[Option<AuraId>]) -> LabeledAuraArrays {
    let mut map = LabeledAuraArrays::new();
    if let Some(first) = auras.iter().flatten().next() {
        map.insert(sim.aura(*first).label.clone(), auras.to_vec());
    }
    map
}

impl Warlock {
    /// Forever folded Curse of Shadow into Curse of the Elements: its top rank drops every magic
    /// resistance and raises all magic damage taken by 10%, which is what core's aura already
    /// does.
    pub(super) fn register_curse_of_elements(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = actor_rank(&spell_data().curse_of_the_elements, sim, unit, false);
        if rank.is_nil() {
            return;
        }

        // The player copy (untagged 1311680, one label per target): the APLs ask for aura
        // 1311680 on the target, and every warlock of a raid shares it.
        let auras = new_enemy_aura_array(sim, |sim, target| {
            CURSE_OF_ELEMENTS.aura(sim, target, true, 0, 0.0)
        });
        self.curse_of_elements_auras = auras.clone();

        let related_aura_arrays = aura_array_to_map(sim, &auras);
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::APL,
                class_spell_mask: masks::CURSE_OF_ELEMENTS,
                cost: flat_cost(rank),
                cast: default_cast(rank.gcd(), 0),
                threat_multiplier: 1.0,
                related_aura_arrays,
                max_range: f64::from(rank.max_range),
                ..SpellConfig::default()
            },
        );
    }

    /// Forever renamed Curse of Doom to Bane of Doom (603): one tick after a minute, on the bane
    /// slot.
    pub(super) fn register_curse_of_doom(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = actor_rank(&spell_data().bane_of_doom, sim, unit, false);
        if rank.is_nil() {
            return;
        }
        let tick = rank.periodic_effect();
        let timer = sim.new_timer(unit);
        sim.register_spell(
            unit,
            SpellConfig {
                cost: flat_cost(rank),
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..default_cast(rank.gcd(), 0)
                },
                bonus_coefficient: tick.coeff(),
                dot: DotConfig {
                    aura: AuraConfig {
                        label: "Bane of Doom".to_string(),
                        tag: "Affliction".to_string(),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: (rank.duration() / tick.period()) as i32,
                    tick_length: tick.period(),
                    bonus_coefficient: tick.coeff(),
                    ..DotConfig::default()
                },
                max_range: f64::from(rank.max_range),
                ..damage_config(rank, masks::CURSE_OF_DOOM, SpellFlag::APL)
            },
        );
    }

    /// Forever renamed Curse of Agony to Bane of Agony and moved it onto the bane slot. It
    /// ramps: the snapshot pays half the tick, and the other half is added back every four
    /// ticks.
    pub(super) fn register_curse_of_agony(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = actor_rank(&spell_data().bane_of_agony, sim, unit, false);
        if rank.is_nil() {
            return;
        }
        let tick = rank.periodic_effect();
        sim.register_spell(
            unit,
            SpellConfig {
                cost: flat_cost(rank),
                cast: default_cast(rank.gcd(), 0),
                bonus_coefficient: tick.coeff(),
                dot: DotConfig {
                    aura: AuraConfig {
                        label: "Bane of Agony".to_string(),
                        tag: "Affliction".to_string(),
                        ..AuraConfig::default()
                    },
                    number_of_ticks: (rank.duration() / tick.period()) as i32,
                    tick_length: tick.period(),
                    bonus_coefficient: tick.coeff(),
                    ..DotConfig::default()
                },
                max_range: f64::from(rank.max_range),
                ..damage_config(rank, masks::CURSE_OF_AGONY, SpellFlag::APL)
            },
        );
    }

    pub(super) fn register_curse_of_recklessness(&mut self, sim: &mut Sim, unit: UnitId) {
        let rank = actor_rank(&spell_data().curse_of_recklessness, sim, unit, false);
        if rank.is_nil() {
            return;
        }

        let auras = new_enemy_aura_array(sim, |sim, target| {
            CURSE_OF_RECKLESSNESS.aura(sim, target, true, 0, 0.0)
        });
        self.curse_of_recklessness_auras = auras.clone();

        let related_aura_arrays = aura_array_to_map(sim, &auras);
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                spell_school: rank.spell_school(),
                defense_type: rank.defense_type_core(),
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::APL,
                class_spell_mask: masks::CURSE_OF_RECKLESSNESS,
                cost: flat_cost(rank),
                cast: default_cast(rank.gcd(), 0),
                threat_multiplier: 1.0,
                related_aura_arrays,
                max_range: f64::from(rank.max_range),
                ..SpellConfig::default()
            },
        );
    }

    /// Go `Warlock.NewAPLAction`: `CastWarlockAssignedCurse`, whose constructor returns nil for
    /// a target that does not resolve and registers nothing otherwise.
    pub(super) fn custom_apl_action_impl(
        &self,
        sim: &Sim,
        _unit: UnitId,
        action: &Message,
    ) -> Option<bool> {
        let (kind, _) = action.oneof("action")?;
        if kind != "cast_warlock_assigned_curse" {
            return None;
        }
        let config = action.message("cast_warlock_assigned_curse")?;
        let enemies = sim
            .env_units
            .iter()
            .filter(|unit| sim.unit(**unit).unit_type == UnitType::Enemy)
            .count();
        let resolves = match config.message("target") {
            None => true,
            Some(target) => match target.enum_name("type").as_str() {
                "Unknown" | "CurrentTarget" | "PreviousTarget" | "NextTarget" | "Self" => true,
                "Target" => (target.i32("index") as usize) < enemies,
                "Player" => target.i32("index") == 0,
                // Go `GetUnit` names no unit for the sets of all players and all targets, so the
                // constructor returns nil.
                "AllPlayers" | "AllTargets" => false,
                // A pet or owner reference is not prepared: the core action refuses it.
                _ => return None,
            },
        };
        Some(resolves)
    }
}
