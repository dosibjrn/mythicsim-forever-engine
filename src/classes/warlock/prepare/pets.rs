//! Go sim/warlock/pets.go: the four demons, registered at construction with only the summoned
//! one enabled at reset, and the abilities the Imp and the Succubus carry. The demons' rotation
//! only runs in a fight.

use std::rc::Rc;
use std::sync::OnceLock;

use serde_json::Value;

use crate::prepare::attack::{AutoAttackOptions, Weapon};
use crate::prepare::pet::PetConfig;
use crate::prepare::sim::{Cooldown, Rules, Sim, SpellId, UnitId, MILLISECOND, SECOND};
use crate::prepare::spell::{
    school, Cast, CastConfig, CostOptions, DefenseType, ProcMask, SpellConfig, GCD_DEFAULT, GCD_MIN,
};
use crate::prepare::stats::{Stat, Stats};

use super::masks;

use super::spells::spell_action;
use super::Warlock;

/// Go `petSpellDamageFraction` and `petAttackPowerFraction`: what a Forever demon gets from its
/// warlock through the client's hidden Warlock Pet Scaling aura (416189).
const PET_SPELL_DAMAGE_FRACTION: f64 = 0.10;
const PET_ATTACK_POWER_FRACTION: f64 = 0.17;

/// A demon's melee hit for each point of its warlock's spell hit: the 8% melee hit cap over the
/// 17% spell hit cap. Forever's developers on their tracker (22 October 2026): "Warlocks will give
/// spell hit equal to their spell hit, and melee hit equal to 8/17ths of their spell hit."
pub(crate) const DEMON_MELEE_HIT_PER_SPELL_HIT: f64 = 8.0 / 17.0;

/// Source-exported WoW Forever beta pet rank rows (not an alternate pet
/// simulator). The pinned core spelldata store lacks rank 1 Firebolt and some
/// lower pet spells, so keeping their verbatim client metadata here avoids
/// borrowing L60 rank 7 on an L30 actor.
pub(super) fn native_pet_rank(ability: &str, level: i32) -> Option<&'static Value> {
    static RANKS: OnceLock<Value> = OnceLock::new();
    let rows = RANKS.get_or_init(|| serde_json::from_str(
        include_str!("../../../../data/forever-warlock-pet-ability-levels.json"))
        .expect("Forever beta pet spell rank table must parse"));
    rows.get(ability)?.as_array()?.iter().rev().find(|r| {
        r.get(1).and_then(Value::as_i64).is_some_and(|min| min <= i64::from(level))
    })
}

/// Go `proto.WarlockOptions_Summon` of the four demons the warlock registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Summon {
    Imp,
    Voidwalker,
    Succubus,
    Felhunter,
}

/// Native Warlock pet choices from the pinned Forever beta 1.60.1.70170
/// spellbook (Talents Forever export, 2026-10-03):
/// <https://questing.wiki/wow-forever/spells/warlock/>
///
/// The generated combat-spell store omits some summon spell IDs, so querying
/// spelldata::find(688) silently hides Imp from an otherwise usable L20 actor.
/// These are source-advertised level thresholds, not a second trainer/quest
/// acquisition validator. Rust prepare and world combat still judge execution.
pub(crate) fn native_summons_at_level(level: u8) -> Vec<&'static str> {
    let mut choices = vec!["NoSummon"];
    for (summon, minimum_level) in [
        (Summon::Imp, 1),         // Summon Imp, spell 688
        (Summon::Voidwalker, 10), // Summon Voidwalker, spell 697
        (Summon::Succubus, 20),   // Summon Succubus, spell 712
        (Summon::Felhunter, 30),  // Summon Felhunter, spell 691
    ] {
        if level >= minimum_level {
            choices.push(summon.name());
        }
    }
    choices
}

impl Summon {
    /// `proto.WarlockOptions_Summon_name`.
    fn name(self) -> &'static str {
        match self {
            Summon::Imp => "Imp",
            Summon::Voidwalker => "Voidwalker",
            Summon::Succubus => "Succubus",
            Summon::Felhunter => "Felhunter",
        }
    }

    /// A lower-level demon uses the sourced Classic pet-level rows. These are explicitly
    /// Classic proxies (not observed Forever numbers), and stay inside Rust mechanics.
    /// For level 60 retain frozen original Forever/Go reference stats unmodified.
    fn level_row(self, level: i32) -> Option<&'static Vec<Value>> {
        static DATA: OnceLock<Value> = OnceLock::new();
        let data = DATA.get_or_init(|| serde_json::from_str(
            include_str!("../../../../data/classic-warlock-pet-level-sources.json"))
            .expect("sourced Classic pet levels must parse"));
        data["pets"][self.name()][level.to_string()].as_array()
    }

    fn base_stats(self, level: i32) -> Stats {
        if level != 60 {
            let row = self.level_row(level)
                .expect("missing sourced Classic pet level row");
            let number = |index: usize| row[index].as_f64()
                .expect("sourced Classic pet stat must be numeric");
            // The SQL row is [HP, mana, armor, melee_min, melee_max, AP,
            // strength, agility, stamina, intellect, spirit]. Health derives
            // from stamina in the engine; do not double-add final source HP.
            let mut stats = Stats::from_pairs(&[
                (Stat::Armor, number(2)),
                (Stat::Strength, number(6)),
                (Stat::Agility, number(7)),
                (Stat::Stamina, number(8)),
                (Stat::Intellect, number(9)),
                (Stat::Spirit, number(10)),
                (Stat::Mana, number(1)),
            ]);
            if self == Summon::Succubus {
                stats[Stat::SpellCritPercent] = 6.2;
            }
            return stats;
        }
        match self {
            Summon::Imp => Stats::from_pairs(&[
                (Stat::Strength, 122.0),
                (Stat::Agility, 35.0),
                (Stat::Stamina, 86.0),
                (Stat::Intellect, 264.0),
                (Stat::Spirit, 260.0),
                (Stat::Mana, 576.0),
            ]),
            Summon::Voidwalker | Summon::Felhunter => Stats::from_pairs(&[
                (Stat::Strength, 129.0),
                (Stat::Agility, 85.0),
                (Stat::Stamina, 234.0),
                (Stat::Intellect, 70.0),
                (Stat::Spirit, 150.0),
                (Stat::Mana, 1066.0),
            ]),
            // The Succubus' Lash of Pain crits about 6% whatever its master's crit: demons carry
            // their own base crit (community #790).
            Summon::Succubus => Stats::from_pairs(&[
                (Stat::Strength, 129.0),
                (Stat::Agility, 85.0),
                (Stat::Stamina, 234.0),
                (Stat::Intellect, 70.0),
                (Stat::Spirit, 150.0),
                (Stat::Mana, 1066.0),
                (Stat::SpellCritPercent, 6.2),
            ]),
        }
    }

    /// Melee ranges come from the same pet level row below 60; retain
    /// frozen Go-reference/Forever ranges at 60.
    fn auto_attack(self, level: i32) -> Option<(f64, f64)> {
        if self == Summon::Imp {
            return None;
        }
        if level != 60 {
            let row = self.level_row(level)?;
            let min = row[3].as_f64()?;
            let max = row[4].as_f64()?;
            return (max > min && max > 0.0).then_some((min, max));
        }
        match self {
            Summon::Imp => None,
            Summon::Voidwalker => Some((31.0, 46.0)),
            Summon::Succubus => Some((95.0, 131.0)),
            Summon::Felhunter => Some((70.0, 97.0)),
        }
    }
}

/// Go `petStatInheritance`: linear in the owner's stats, which the dynamic inheritance needs.
/// Melee hit is Rust only: Go gives the demon its master's whole spell hit as melee hit, Forever
/// scales it to the melee cap ([`DEMON_MELEE_HIT_PER_SPELL_HIT`]).
fn pet_stat_inheritance(sim: &mut Sim, owner: &Stats) -> Stats {
    let mut inherited = Stats::default();
    inherited[Stat::SpellHitPercent] = owner[Stat::SpellHitPercent];
    inherited[Stat::PhysicalHitPercent] = match sim.rules {
        Rules::Forever => owner[Stat::SpellHitPercent] * DEMON_MELEE_HIT_PER_SPELL_HIT,
        Rules::GoReference => owner[Stat::SpellHitPercent],
    };
    inherited[Stat::SpellCritPercent] = owner[Stat::SpellCritPercent];
    inherited[Stat::PhysicalCritPercent] = owner[Stat::SpellCritPercent];
    inherited[Stat::SpellDamage] = owner[Stat::SpellDamage] * PET_SPELL_DAMAGE_FRACTION;
    inherited[Stat::AttackPower] = owner[Stat::SpellDamage] * PET_ATTACK_POWER_FRACTION;
    inherited
}

/// What Go's `WarlockPet` adds to its `core.Pet`.
#[derive(Clone, Debug)]
pub(crate) struct WarlockPet {
    pub unit: UnitId,
    /// Go `MinMana`: the minimum mana the AI keeps before it casts again.
    pub min_mana: f64,
    /// Go `AutoCastAbilities`.
    pub auto_cast_abilities: Vec<SpellId>,
}

/// Go `Warlock.BasePets`, `ActivePet` and the demons by name.
#[derive(Clone, Debug, Default)]
pub(crate) struct WarlockPets {
    pub base: Vec<UnitId>,
    pub pets: Vec<WarlockPet>,
    pub imp: Option<UnitId>,
    pub voidwalker: Option<UnitId>,
    pub succubus: Option<UnitId>,
    pub felhunter: Option<UnitId>,
    pub active: Option<UnitId>,
}

impl WarlockPets {
    /// The state of a demon.
    pub(super) fn state(&self, unit: UnitId) -> Option<&WarlockPet> {
        self.pets.iter().find(|pet| pet.unit == unit)
    }

    fn state_mut(&mut self, unit: UnitId) -> Option<&mut WarlockPet> {
        self.pets.iter_mut().find(|pet| pet.unit == unit)
    }
}

impl Warlock {
    /// Go `Warlock.registerPets`.
    pub(super) fn register_pets(&mut self, sim: &mut Sim, owner: UnitId) {
        let imp = self.register_pet(sim, owner, Summon::Imp);
        let voidwalker = self.register_pet(sim, owner, Summon::Voidwalker);
        let succubus = self.register_pet(sim, owner, Summon::Succubus);
        let felhunter = self.register_pet(sim, owner, Summon::Felhunter);
        self.pets.imp = Some(imp);
        self.pets.voidwalker = Some(voidwalker);
        self.pets.succubus = Some(succubus);
        self.pets.felhunter = Some(felhunter);
        self.pets.base = vec![imp, voidwalker, succubus, felhunter];
    }

    /// Go `Warlock.registerPet`.
    fn register_pet(&mut self, sim: &mut Sim, owner: UnitId, summon: Summon) -> UnitId {
        let enabled_on_start = self.options.enum_name("summon") == summon.name();

        let pet = sim.new_pet(PetConfig {
            name: summon.name().to_string(),
            owner,
            base_stats: summon.base_stats(sim.unit(owner).level),
            stat_inheritance: Rc::new(pet_stat_inheritance),
            enabled_on_start,
            is_guardian: false,
            is_dynamic: true,
            has_dynamic_melee_speed_inheritance: false,
            has_dynamic_cast_speed_inheritance: false,
            has_resource_regen_inheritance: false,
            starts_at_owner_distance: false,
        });

        sim.enable_pet_mana_bar(pet);
        let tables = crate::data::tables::tables();
        let crit = |table: &std::collections::BTreeMap<String, f64>, class: &str| {
            table.get(class).copied().unwrap_or(0.0)
        };
        if summon == Summon::Imp {
            sim.unit_mut(pet).sdm.add_stat_dependency(
                Stat::Intellect,
                Stat::SpellCritPercent,
                crit(&tables.crit_per_int_max_level, "ClassMage"),
            );
        } else {
            let sdm = &mut sim.unit_mut(pet).sdm;
            sdm.add_stat_dependency(Stat::Strength, Stat::AttackPower, 2.0);
            sdm.add_stat_dependency(
                Stat::Agility,
                Stat::PhysicalCritPercent,
                crit(&tables.crit_per_agi_max_level, "ClassWarrior"),
            );
            sdm.add_stat_dependency(
                Stat::Intellect,
                Stat::SpellCritPercent,
                crit(&tables.crit_per_int_max_level, "ClassWarrior"),
            );

            if let Some((min, max)) = summon.auto_attack(sim.unit(owner).level) {
                sim.enable_auto_attacks(
                    pet,
                    AutoAttackOptions {
                        main_hand: Weapon {
                            base_damage_min: min,
                            base_damage_max: max,
                            swing_speed: 2.0,
                            ..Weapon::default()
                        },
                        auto_swing_melee: true,
                        ..AutoAttackOptions::default()
                    },
                );
            }
        }

        if enabled_on_start {
            self.pets.active = Some(pet);
            sim.register_reset_effect(owner, Rc::new(|_: &mut Sim| {}));
        }

        sim.add_pet(owner, pet);
        self.pets.pets.push(WarlockPet {
            unit: pet,
            min_mana: 0.0,
            auto_cast_abilities: Vec::new(),
        });
        pet
    }

    /// Go `Warlock.registerPetAbilities`. The Voidwalker's Torment is a threat ability and is not
    /// modelled.
    pub(super) fn register_pet_abilities(&mut self, sim: &mut Sim) {
        if let Some(imp) = self.pets.imp {
            self.register_firebolt_spell(sim, imp);
        }
        if let Some(succubus) = self.pets.succubus {
            self.register_lash_of_pain_spell(sim, succubus);
        }
    }

    /// The generator makes no table for pet spells, so Firebolt carries our client-verified
    /// beta 1.60.1 values (rank 7). Firebolt's cooldown is the Imp's pause between casts: beta
    /// logs time 281 Firebolts 2.435 sec apart (median, 2.0 sec cast, next one starting 0.43 sec
    /// after the last lands). The rotation polls every 100 ms, so 400 ms lands the next cast
    /// 2.4 sec after the last; 430 would round up to 2.5. Improved Imp rides on its talent as a
    /// SpellMod.
    fn register_firebolt_spell(&mut self, sim: &mut Sim, pet: UnitId) {
        let Some(rank) = native_pet_rank("firebolt", sim.unit(pet).level) else {
            return;
        };
        let spell_id = rank[0].as_i64().expect("Firebolt client ID") as i32;
        let mana_cost = rank[4].as_i64().expect("Firebolt client cost") as i32;
        let coefficient = rank[5].as_f64().expect("Firebolt client SP coefficient");
        if let Some(state) = self.pets.state_mut(pet) {
            state.min_mana = f64::from(mana_cost);
        }
        let timer = sim.new_timer(pet);
        let spell = sim.register_spell(
            pet,
            SpellConfig {
                action_id: spell_action(spell_id),
                spell_school: school::FIRE,
                defense_type: DefenseType::Magic,
                proc_mask: ProcMask::SPELL_DAMAGE,
                class_spell_mask: masks::IMP_FIRE_BOLT,
                cost: CostOptions {
                    mana_flat_cost: mana_cost,
                    ..CostOptions::default()
                },
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: GCD_MIN,
                        cast_time: 2 * SECOND,
                        ..Cast::default()
                    },
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: 400 * MILLISECOND,
                    },
                    ..CastConfig::default()
                },
                damage_multiplier_additive: 1.0,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                bonus_coefficient: coefficient,
                ..SpellConfig::default()
            },
        );
        if let Some(state) = self.pets.state_mut(pet) {
            state.auto_cast_abilities.push(spell);
        }
    }

    /// Lash of Pain carries our client-verified beta 1.60.1 values (rank 6). Improved Sayaad
    /// rides on its talent as a SpellMod.
    fn register_lash_of_pain_spell(&mut self, sim: &mut Sim, pet: UnitId) {
        let Some(rank) = native_pet_rank("lash_of_pain", sim.unit(pet).level) else {
            return;
        };
        let spell_id = rank[0].as_i64().expect("Lash client ID") as i32;
        let mana_cost = rank[4].as_i64().expect("Lash client cost") as i32;
        let coefficient = rank[5].as_f64().expect("Lash client SP coefficient");
        if let Some(state) = self.pets.state_mut(pet) {
            state.min_mana = f64::from(mana_cost);
        }
        let timer = sim.new_timer(pet);
        let spell = sim.register_spell(
            pet,
            SpellConfig {
                action_id: spell_action(spell_id),
                spell_school: school::SHADOW,
                defense_type: DefenseType::Magic,
                proc_mask: ProcMask::SPELL_DAMAGE,
                class_spell_mask: masks::SUCCUBUS_LASH_OF_PAIN,
                cost: CostOptions {
                    mana_flat_cost: mana_cost,
                    ..CostOptions::default()
                },
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: GCD_DEFAULT,
                        ..Cast::default()
                    },
                    ignore_haste: true,
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: 12 * SECOND,
                    },
                    ..CastConfig::default()
                },
                damage_multiplier_additive: 1.0,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                bonus_coefficient: coefficient,
                ..SpellConfig::default()
            },
        );
        if let Some(state) = self.pets.state_mut(pet) {
            state.auto_cast_abilities.push(spell);
        }
    }
}

#[cfg(test)]
mod sourced_pet_level_tests {
    use super::*;
    #[test]
    fn lower_level_warlock_pets_use_client_spell_ranks_and_sourced_stats() {
        for (level, firebolt, lash) in [(20, 7800, 7814), (40, 7802, 7816), (60, 11763, 11780)] {
            assert_eq!(native_pet_rank("firebolt", level).unwrap()[0].as_i64(), Some(firebolt));
            assert_eq!(native_pet_rank("lash_of_pain", level).unwrap()[0].as_i64(), Some(lash));
        }
        assert!(Summon::Imp.base_stats(20)[Stat::Intellect] < Summon::Imp.base_stats(40)[Stat::Intellect]);
        assert_eq!(Summon::Imp.base_stats(60)[Stat::Intellect], 264.0);
    }
}
