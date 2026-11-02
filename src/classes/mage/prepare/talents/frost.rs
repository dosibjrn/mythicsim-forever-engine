//! Go sim/mage/talents_frost.go.

use std::rc::Rc;

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::aura_helpers::AbsorptionAuraConfig;
use crate::prepare::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::dbcenums;
use crate::prepare::hotfix_113554;
use crate::prepare::resolve_aura::aura_config;
use crate::prepare::resolve_spell::{flags, spell_config};
use crate::prepare::sim::AuraId;
use crate::prepare::sim::{AuraConfig, EventCallbacks, Rules, Sim, UnitId};
use crate::prepare::spell::SpellFlag;
use crate::prepare::spell::{school, ProcMask};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::stats::{SchoolIndex, Stat};

use super::super::masks;
use super::super::spell_data::spell_data;
use super::super::spells::POINT_BLANK_RADIUS;
use super::super::Mage;
use super::millis;

impl Mage {
    /// Go `registerFrostTalents`.
    pub(super) fn register_frost_talents(&self, sim: &mut Sim, unit: UnitId) {
        // Tier 1: Frost Warding models nothing in Go either.
        self.register_improved_frostbolt(sim, unit);
        self.register_elemental_precision(sim, unit);

        // Tier 2: Permafrost and Frostbite model nothing in Go either.
        self.register_ice_shards(sim, unit);
        self.register_improved_frost_nova(sim, unit);

        // Tier 3 (Piercing Ice left the tree in hotfix push 112753, the fork's patch 186)
        self.register_frost_channeling(sim, unit);
        // Improved Blizzard: blizzard.go
        // Ice Lance: ice_lance.go

        // Tier 4: Ice Block models nothing in Go either.
        self.register_arctic_reach(sim, unit);
        // Shatter: with Fingers of Frost below

        // Tier 5
        self.register_improved_cone_of_cold(sim, unit);
        // Cold Snap: cold_snap.go
        self.register_fingers_of_frost(sim, unit);

        // Tier 6
        self.register_winter_chill(sim, unit);

        // Tier 7
        self.register_ice_barrier(sim, unit);
    }

    /// Go `registerIceBarrier` and ice_barrier.go `registerIceBarrierSpell` (the fork's patch
    /// 187): Frigid Power, then the shield, each with Piercing Ice's old Frost damage shape, then
    /// the highest rank as an APL spell.
    fn register_ice_barrier(&self, sim: &mut Sim, unit: UnitId) {
        if !self.talents.bool("ice_barrier") {
            return;
        }
        let data = spell_data();
        // Hotfix push 113531 raises both auras' Frost damage under Forever's rules.
        let rank = hotfix_113554::row(sim.rules, data.ice_barrier.highest().id);
        let frigid_rank =
            hotfix_113554::row(sim.rules, data.ice_barrier_triggered.by_id(1326891).id);
        let pct = |row: &'static crate::data::spells::Spell, misc: i32| {
            row.effect(dbcenums::A_ADD_PCT_MODIFIER, misc)
                .average(CHARACTER_LEVEL)
                / 100.0
        };

        let frigid_power = sim.register_aura(unit, aura_config(frigid_rank, &[]));
        attach_frost_damage_bonus(
            sim,
            frigid_power,
            pct(frigid_rank, dbcenums::SPELLMOD_DAMAGE),
            pct(frigid_rank, dbcenums::SPELLMOD_DOT),
        );

        let warding = 1.0
            + data
                .frost_warding
                .effect_at(3)
                .fraction_at(self.talents.i32("frost_warding"));
        let base = rank.effect_n(1).average(CHARACTER_LEVEL);
        let shield = sim.new_damage_absorption_aura(
            unit,
            AbsorptionAuraConfig {
                aura: aura_config(rank, &[]),
                shield_strength_calculator: Some(Rc::new(move |sim: &Sim, unit| {
                    warding * frost_spell_power(sim, unit).mul_add(0.1, base)
                })),
                ..AbsorptionAuraConfig::default()
            },
        );
        attach_frost_damage_bonus(
            sim,
            shield.aura,
            pct(rank, dbcenums::SPELLMOD_DAMAGE),
            pct(rank, dbcenums::SPELLMOD_DOT),
        );

        let mut config = spell_config(sim, unit, rank, &[flags(SpellFlag::APL)]);
        config.class_spell_mask = masks::ICE_BARRIER;
        config.related_self_buff = Some(shield.aura);
        sim.register_spell(unit, config);
    }

    fn register_improved_frostbolt(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("improved_frostbolt");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                // 11070's mask is Frostbolt's bit, which Frost Blast carries (MythicSim patch 153).
                class_mask: masks::FROSTBOLT | masks::FROST_BLAST,
                time_value: millis(
                    spell_data()
                        .improved_frostbolt
                        .effect(
                            dbcenums::A_ADD_FLAT_MODIFIER,
                            dbcenums::SPELLMOD_CASTING_TIME,
                        )
                        .value_at(rank),
                ),
                kind: SpellModType::CastTimeFlat,
                ..SpellModConfig::default()
            },
        );
    }

    fn register_elemental_precision(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("elemental_precision");
        if rank == 0 {
            return;
        }
        let hit = spell_data().elemental_precision.value_at(rank);
        // Client 1.60.1.70707 turns the talent (Precision, 29438, SpellEffect 704780) from a
        // miss chance modifier on Fire and Frost spells into spell hit chance (aura 55) for every
        // school, Arcane included. The Go reference keeps Fire and Frost.
        let schools: &[SchoolIndex] = match sim.rules {
            Rules::Forever => &[
                SchoolIndex::Arcane,
                SchoolIndex::Fire,
                SchoolIndex::Frost,
                SchoolIndex::Holy,
                SchoolIndex::Nature,
                SchoolIndex::Shadow,
            ],
            Rules::GoReference => &[SchoolIndex::Fire, SchoolIndex::Frost],
        };
        let pseudo = &mut sim.unit_mut(unit).pseudo_stats;
        for &school in schools {
            pseudo.school_bonus_hit_chance[school as usize] += hit;
        }
    }

    fn register_ice_shards(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("ice_shards");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::ALL,
                school: school::FROST,
                float_value: spell_data().ice_shards.fraction_at(rank),
                kind: SpellModType::CritMultiplierFlat,
                ..SpellModConfig::default()
            },
        );
    }

    /// Improved Frost Nova (11165): Frost Nova's cooldown -2/-4 s.
    fn register_improved_frost_nova(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("improved_frost_nova");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::FROST_NOVA,
                time_value: millis(
                    spell_data()
                        .improved_frost_nova
                        .effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_COOLDOWN)
                        .value_at(rank),
                ),
                kind: SpellModType::CooldownFlat,
                ..SpellModConfig::default()
            },
        );
    }

    fn register_frost_channeling(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("frost_channeling");
        if rank == 0 {
            return;
        }
        let data = &spell_data().frost_channeling;
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::ALL,
                school: school::FROST,
                float_value: data
                    .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_COST)
                    .fraction_at(rank),
                kind: SpellModType::PowerCostPctAdd,
                ..SpellModConfig::default()
            },
        );
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::ALL,
                school: school::FROST,
                float_value: -data.effect(dbcenums::A_MOD_THREAT, 16).fraction_at(rank),
                kind: SpellModType::ThreatMultiplierPct,
                ..SpellModConfig::default()
            },
        );
    }

    /// Arctic Reach, new in Forever: 16757 adds 10/20% to Frostbolt and Blizzard range and to Frost
    /// Nova and Cone of Cold radius (its two class masks). The sim has no percent range mod, so the
    /// share is taken of each spell's own range here. Go walks a map of the three, whose order no
    /// result reads.
    fn register_arctic_reach(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("arctic_reach");
        if rank == 0 {
            return;
        }
        let data = &spell_data().arctic_reach;
        let range_share = data
            .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_RANGE)
            .fraction_at(rank);
        let radius_share = data
            .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_RADIUS)
            .fraction_at(rank);
        let mut ranges = vec![
            (
                masks::FROSTBOLT,
                range_share * f64::from(spell_data().frostbolt.highest().max_range),
            ),
            // The range mask holds Frostbolt's bit, which Frost Blast carries (MythicSim patch 153).
            (
                masks::FROST_BLAST,
                range_share * f64::from(spell_data().frost_blast.highest().max_range),
            ),
            (
                masks::BLIZZARD,
                range_share * f64::from(spell_data().blizzard.highest().max_range),
            ),
            (
                masks::FROST_NOVA | masks::CONE_OF_COLD,
                radius_share * POINT_BLANK_RADIUS,
            ),
        ];
        // Hotfix push 113531 adds Ice Lance to the range mask (SpellEffect 694455) under Forever's
        // rules.
        if sim.rules == Rules::Forever {
            ranges.push((
                masks::ICE_LANCE,
                range_share * f64::from(spell_data().ice_lance.highest().max_range),
            ));
        }
        for (class_mask, yards) in ranges {
            sim.add_static_mod(
                unit,
                SpellModConfig {
                    class_mask,
                    float_value: yards,
                    kind: SpellModType::RangeFlat,
                    ..SpellModConfig::default()
                },
            );
        }
    }

    /// Improved Cone of Cold (11190): Cone of Cold damage +12/23/35%.
    fn register_improved_cone_of_cold(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("improved_cone_of_cold");
        if rank == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: masks::CONE_OF_COLD,
                float_value: spell_data()
                    .improved_cone_of_cold
                    .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_DAMAGE)
                    .fraction_at(rank),
                kind: SpellModType::DamageDoneFlat,
                ..SpellModConfig::default()
            },
        );
    }

    /// Raid bosses cannot be chilled or frozen, so Fingers of Frost is the only thing that gets
    /// Shatter and the Ice Lance bonus going on one; Shatter is folded in here because the two
    /// only ever fire together.
    fn register_fingers_of_frost(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("fingers_of_frost");
        if rank == 0 {
            return;
        }
        let data = spell_data();
        let proc_chance = data.fingers_of_frost.effect_at(2).fraction_at(rank);
        let fof_rank = data.fingers_of_frost_triggered.highest();
        let shatter_crit = data.shatter.value_at(self.talents.i32("shatter"));

        let shatter_mod = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                class_mask: masks::ALL,
                float_value: shatter_crit,
                kind: SpellModType::BonusCritPercent,
                ..SpellModConfig::default()
            },
        );

        // Go also holds a cast in flight out of the Shatter bonus when the aura is gained
        // mid-cast. Preparation never casts, so no cast is in flight when it gains the aura.
        sim.register_aura(
            unit,
            AuraConfig {
                label: "Fingers of Frost".to_string(),
                action_id: Some(ActionId {
                    spell_id: fof_rank.id,
                    ..ActionId::default()
                }),
                duration: fof_rank.duration(),
                max_stacks: data.fingers_of_frost.effect_at(1).value_at(rank) as i32,
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.activate_spell_mod(shatter_mod)
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.deactivate_spell_mod(shatter_mod)
                })),
                events: EventCallbacks {
                    on_cast_complete: true,
                    ..EventCallbacks::default()
                },
                ..AuraConfig::default()
            },
        );

        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Fingers of Frost Trigger".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                // Frostbolt and Frostfire Bolt roll at launch (community #776); the trigger
                // hears the chills that land at once.
                class_spell_mask: masks::CONE_OF_COLD | masks::IMPROVED_BLIZZARD,
                outcome: HitOutcome::LANDED,
                proc_chance,
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );
    }

    /// Client 70425 made Winter's Chill a Cone of Cold empowerment: each stack makes the target
    /// take 5% more damage from the mage's own Cone of Cold, up to 5 stacks, for 15 sec. Under
    /// Forever's rules, client 1.60.1.70630 makes the stacks "your next Cone of Cold" (consumed
    /// by it) and adds 7% of spell damage to that Cone of Cold per stack (see
    /// `classes::mage::talents::winters_chill`). The stacks stay on the mage, not on the target:
    /// the client puts 12579 on the target (ImplicitTarget 6), but the engine reads the aura by
    /// name from the mage's side, so custom rotations that test it keep working, and the Cone of
    /// Cold's own damage mod reads it there too.
    fn register_winter_chill(&self, sim: &mut Sim, unit: UnitId) {
        let rank = self.talents.i32("winters_chill");
        if rank == 0 {
            return;
        }
        let data = spell_data();
        // Hotfix push 113531 raises the stacks' Cone of Cold damage under Forever's rules.
        let chill_rank = hotfix_113554::row(sim.rules, data.winters_chill_triggered.highest().id);
        let damage_per_stack = chill_rank.effect_n(1).percent();

        let damage_mod = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                class_mask: masks::CONE_OF_COLD,
                kind: SpellModType::DamageDoneFlat,
                ..SpellModConfig::default()
            },
        );

        sim.register_aura(
            unit,
            AuraConfig {
                label: "Winter's Chill".to_string(),
                action_id: Some(ActionId {
                    spell_id: chill_rank.id,
                    ..ActionId::default()
                }),
                duration: chill_rank.duration(),
                max_stacks: data.winters_chill.effect_at(1).value_at(rank) as i32,
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.activate_spell_mod(damage_mod)
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.deactivate_spell_mod(damage_mod)
                })),
                on_stacks_change: Some(Rc::new(move |sim: &mut Sim, _, _, new_stacks| {
                    sim.update_spell_mod_float_value(
                        damage_mod,
                        damage_per_stack * f64::from(new_stacks),
                    )
                })),
                // Client 1.60.1.70630: "your next Cone of Cold" uses the stacks up, which the
                // runtime does when the cast completes (after its damage).
                events: EventCallbacks {
                    on_cast_complete: sim.rules == Rules::Forever,
                    ..EventCallbacks::default()
                },
                ..AuraConfig::default()
            },
        );

        // Forever states a flat SpellAuraOptions.ProcChance of 100 on the talent spell and puts
        // the real per-rank chance on effect 2 (by position); effect 1 is the stack count.
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Winters Chill Talent".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                proc_mask: ProcMask::SPELL_DAMAGE,
                outcome: HitOutcome::LANDED,
                proc_chance: data.winters_chill.effect_at(2).fraction_at(rank),
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );
    }
}

/// ice_barrier.go `attachFrostDamageBonus`, Piercing Ice's shape before patch 186: `hit` on every
/// Frost spell but Frostfire Bolt, `dot` on all of Frostfire Bolt and the rest of `hit` on its
/// direct damage alone.
fn attach_frost_damage_bonus(sim: &mut Sim, aura: AuraId, hit: f64, dot: f64) {
    sim.attach_spell_mod(
        aura,
        SpellModConfig {
            class_mask: masks::ALL & !masks::FROSTFIRE_BOLT,
            school: school::FROST,
            float_value: hit,
            kind: SpellModType::DamageDoneFlat,
            ..SpellModConfig::default()
        },
    );
    sim.attach_spell_mod(
        aura,
        SpellModConfig {
            class_mask: masks::FROSTFIRE_BOLT,
            float_value: dot,
            kind: SpellModType::DamageDoneFlat,
            ..SpellModConfig::default()
        },
    );
    sim.attach_spell_mod(
        aura,
        SpellModConfig {
            class_mask: masks::FROSTFIRE_BOLT,
            float_value: hit - dot,
            kind: SpellModType::DirectDamageDoneFlat,
            ..SpellModConfig::default()
        },
    );
}

/// ice_barrier.go `frostSpellPower` ($SPFR): the general spell damage plus the Frost one.
fn frost_spell_power(sim: &Sim, unit: UnitId) -> f64 {
    sim.stat(unit, Stat::SpellDamage) + sim.stat(unit, Stat::FrostDamage)
}
