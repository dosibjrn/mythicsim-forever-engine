//! The Shaman's talents, registered in Go's order: `talents_elemental.go`,
//! `talents_enhancement.go` and `talents_restoration.go`.

use std::rc::Rc;

use crate::Rules;

use crate::prepare::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use crate::prepare::character::constants::{
    DODGE_RATING_PER_DODGE_PERCENT, PHYSICAL_HIT_RATING_PER_HIT_PERCENT,
    SPELL_HIT_RATING_PER_HIT_PERCENT,
};
use crate::prepare::character::{cooldown_type, MajorCooldown};
use crate::prepare::dbcenums;
use crate::prepare::sim::{
    school_array_index, AuraConfig, Cooldown, Duration, EventCallbacks, Sim, UnitId, MILLISECOND,
    NEVER_EXPIRES, SECOND,
};
use crate::prepare::spell::{school, CastConfig, DefenseType, ProcMask, SpellConfig, SpellFlag};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::stats::{SchoolIndex, Stat};

use super::spell_data::spell_data;
use super::spells::{longest_cooldown, spell_action};
use super::{flags, masks, Shaman};

/// Call of Thunder's critical strike chance for Lightning Bolt and Chain Lightning (notes of 2
/// November 2026).
pub(crate) const CALL_OF_THUNDER_CRIT_PERCENT: f64 = 5.0;

/// Elemental Mastery's aura label: the notes of 2 November 2026 rename the talent Lightning
/// Mastery (the name only, the icon aside). The Go reference keeps the old name.
pub(super) fn elemental_mastery_label(rules: Rules) -> &'static str {
    match rules {
        Rules::Forever => "Lightning Mastery",
        Rules::GoReference => "Elemental Mastery",
    }
}

/// `time.Millisecond * time.Duration(value)`: the float is cut to whole milliseconds.
pub(super) fn millis(value: f64) -> Duration {
    (value as i64).wrapping_mul(MILLISECOND)
}

/// A static mod on the class spells a mask names; the caller states its value.
fn static_mod(sim: &mut Sim, unit: UnitId, kind: SpellModType, class_mask: i64, value: f64) {
    sim.add_static_mod(
        unit,
        SpellModConfig {
            kind,
            class_mask,
            float_value: value,
            ..SpellModConfig::default()
        },
    );
}

/// A static time mod on the class spells a mask names.
fn static_time_mod(
    sim: &mut Sim,
    unit: UnitId,
    kind: SpellModType,
    class_mask: i64,
    value: Duration,
) {
    sim.add_static_mod(
        unit,
        SpellModConfig {
            kind,
            class_mask,
            time_value: value,
            ..SpellModConfig::default()
        },
    );
}

impl Shaman {
    /// Go `registerElementalTalents`.
    pub(super) fn register_elemental_talents(&mut self, sim: &mut Sim, unit: UnitId) {
        // Tier 1
        self.apply_convection(sim, unit);
        self.apply_concussion(sim, unit);

        // Tier 2
        self.apply_elemental_warding(sim, unit);
        self.apply_reverberation(sim, unit);
        self.apply_call_of_flame(sim, unit);
        self.apply_elemental_devastation(sim, unit);

        // Tier 3
        self.apply_elemental_focus(sim, unit);
        self.apply_elemental_alacrity(sim, unit);

        // Tier 4
        self.apply_trial_by_fire(sim, unit);
        // Eye of the Storm (pushback resistance) changes no number.

        // Tier 5
        self.apply_elemental_reach(sim, unit);
        // Lightning Overload (shaman.go) and Earthbound change no prepared state.
        // Tier 6
        self.apply_elemental_fury(sim, unit);

        // Tier 5 (client 70425 moved Lava Burst up from tier 7). The notes of 2 November 2026
        // replace it with Call of Thunder in the same slot under Forever's rules.
        self.register_lava_burst_spell(sim, unit);
        self.apply_call_of_thunder(sim, unit);

        // Tier 7
        self.apply_elemental_mastery(sim, unit);
    }

    /// Call of Thunder, which the development notes of 2 November 2026 put in Lava Burst's talent
    /// slot (a saved talent string keeps its positions, so a point there now means this talent):
    /// +5% critical strike chance for Lightning Bolt and Chain Lightning, and so for Lightning
    /// Overload's copies, which are the same spells. One rank, no client row yet. The Go
    /// reference keeps Lava Burst.
    fn apply_call_of_thunder(&self, sim: &mut Sim, unit: UnitId) {
        if sim.rules != Rules::Forever || !self.has_talent("lava_burst") {
            return;
        }
        static_mod(
            sim,
            unit,
            SpellModType::BonusCritPercent,
            masks::LIGHTNING_BOLT
                | masks::CHAIN_LIGHTNING
                | masks::LIGHTNING_BOLT_OVERLOAD
                | masks::CHAIN_LIGHTNING_OVERLOAD,
            CALL_OF_THUNDER_CRIT_PERCENT,
        );
    }

    /// Elemental Mastery, the tier 7 Elemental talent of client 70425: for 15 sec, Lightning
    /// Bolt and Lava Burst cast 0.5 sec faster and Chain Lightning has no cooldown, on a 3 min
    /// cooldown. Chain Lightning's cooldown is cancelled with a flat mod of its own length.
    fn apply_elemental_mastery(&self, sim: &mut Sim, unit: UnitId) {
        if !self.has_talent("elemental_mastery") {
            return;
        }
        let data = spell_data();
        let rank = data.elemental_mastery.highest();
        let cast_time = millis(
            rank.effect(
                dbcenums::A_ADD_FLAT_MODIFIER,
                dbcenums::SPELLMOD_CASTING_TIME,
            )
            .base_points,
        );
        let chain_lightning = data.chain_lightning.highest();
        let chain_lightning_cooldown = chain_lightning
            .cooldown()
            .max(chain_lightning.category_cooldown());

        let cast_mod = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::CastTimeFlat,
                class_mask: masks::LIGHTNING_BOLT | masks::LAVA_BURST,
                time_value: cast_time,
                ..SpellModConfig::default()
            },
        );
        let cooldown_mod = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::CooldownFlat,
                class_mask: masks::CHAIN_LIGHTNING,
                time_value: -chain_lightning_cooldown,
                ..SpellModConfig::default()
            },
        );
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: elemental_mastery_label(sim.rules).to_string(),
                action_id: Some(spell_action(rank.id)),
                duration: rank.duration(),
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.activate_spell_mod(cast_mod);
                    sim.activate_spell_mod(cooldown_mod);
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.deactivate_spell_mod(cast_mod);
                    sim.deactivate_spell_mod(cooldown_mod);
                })),
                ..AuraConfig::default()
            },
        );
        let timer = sim.new_timer(unit);
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                flags: SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::APL,
                class_spell_mask: masks::ELEMENTAL_MASTERY,
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: rank.cooldown().max(rank.category_cooldown()),
                    },
                    ..CastConfig::default()
                },
                related_self_buff: Some(aura),
                ..SpellConfig::default()
            },
        );
        sim.add_major_cooldown(
            unit,
            MajorCooldown {
                spell,
                priority: 0,
                cooldown_type: cooldown_type::DPS,
                allow_spell_queueing: false,
                timings: Vec::new(),
            },
        );
    }

    fn apply_call_of_flame(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("call_of_flame");
        if points == 0 {
            return;
        }
        static_mod(
            sim,
            unit,
            SpellModType::DamageDoneFlat,
            masks::FIRE_TOTEM | masks::FLAME_SHOCK | masks::FIRE_NOVA | masks::LAVA_BURST,
            spell_data()
                .call_of_flame
                .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_DAMAGE)
                .fraction_at(points),
        );
        // "Call of Flame now also increases the duration of Flame Shock by 3/6/9 sec" (16 October
        // 2026 beta notes). The row has no duration modifier yet, so the time is added as ticks of
        // the dot's own period: 3 sec a rank.
        const CALL_OF_FLAME_FLAME_SHOCK_SECONDS: i64 = 3 * SECOND;
        let period = spell_data()
            .flame_shock
            .highest()
            .periodic_effect()
            .period();
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::DotNumberOfTicksFlat,
                int_value: (CALL_OF_FLAME_FLAME_SHOCK_SECONDS * i64::from(points) / period) as i32,
                class_mask: masks::FLAME_SHOCK_DOT,
                ..SpellModConfig::default()
            },
        );
    }

    fn apply_concussion(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("concussion");
        if points == 0 {
            return;
        }
        // Client 16035's mask is Lightning Bolt, Chain Lightning and Earth Shock: not Flame or
        // Frost Shock.
        static_mod(
            sim,
            unit,
            SpellModType::DamageDoneFlat,
            masks::LIGHTNING_BOLT | masks::CHAIN_LIGHTNING | masks::OVERLOAD | masks::EARTH_SHOCK,
            spell_data()
                .concussion
                .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_DAMAGE)
                .fraction_at(points),
        );
    }

    fn apply_convection(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("convection");
        if points == 0 {
            return;
        }
        static_mod(
            sim,
            unit,
            SpellModType::PowerCostPctAdd,
            masks::LIGHTNING_BOLT
                | masks::CHAIN_LIGHTNING
                | masks::OVERLOAD
                | masks::SHOCK
                | masks::LAVA_BURST,
            spell_data()
                .convection
                .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_COST)
                .fraction_at(points),
        );
    }

    fn apply_elemental_devastation(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("elemental_devastation");
        if points == 0 {
            return;
        }
        let data = spell_data();
        let crit_buff_aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Elemental Devastation".to_string(),
                action_id: Some(spell_action(29178)),
                duration: 10 * SECOND,
                ..AuraConfig::default()
            },
        );
        sim.attach_spell_mod(
            crit_buff_aura,
            SpellModConfig {
                kind: SpellModType::BonusCritPercent,
                float_value: data
                    .elemental_devastation
                    .effect(dbcenums::A_DUMMY, 0)
                    .value_at(points),
                proc_mask: ProcMask::MELEE,
                ..SpellModConfig::default()
            },
        );
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Elemental Devastation Trigger".to_string(),
                // Forever's 30160 lacks the bit (Era's 29179/29180 carry it): overload crits
                // don't count.
                can_proc_from_procs: data.elemental_devastation.highest().can_proc_from_procs(),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                proc_mask: ProcMask::SPELL_DAMAGE,
                outcome: HitOutcome::CRIT,
                ..ProcTrigger::default()
            },
        );
    }

    fn apply_elemental_focus(&self, sim: &mut Sim, unit: UnitId) {
        if !self.has_talent("elemental_focus") {
            return;
        }
        let data = spell_data();
        // 16246's class mask: Lightning Bolt, Chain Lightning, Lava Burst, the shocks and Fire
        // Nova (408345).
        let can_consume_spells = masks::CLEARCASTING_SPELLS;
        let clearcasting = data.elemental_focus_triggered.highest();
        let max_stacks = i32::from(clearcasting.proc_charges);

        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Clearcasting".to_string(),
                action_id: Some(spell_action(16246)),
                duration: clearcasting.duration(),
                max_stacks,
                events: EventCallbacks {
                    on_cast_complete: true,
                    ..EventCallbacks::default()
                },
                ..AuraConfig::default()
            },
        );
        sim.attach_spell_mod(
            aura,
            SpellModConfig {
                kind: SpellModType::PowerCostPctAdd,
                class_mask: can_consume_spells,
                float_value: clearcasting
                    .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_COST)
                    .percent(),
                ..SpellModConfig::default()
            },
        );

        // Client 16164: "a chance to enter a Clearcasting state after casting any Fire, Frost,
        // or Nature damage spell", so it rolls when the cast completes, hit or miss.
        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Elemental Focus".to_string(),
                proc_chance: f64::from(data.elemental_focus.rank(1).proc_chance) / 100.0,
                callback: CallbackMask::ON_CAST_COMPLETE,
                proc_mask: ProcMask::SPELL_DAMAGE,
                // 16164 carries the bit.
                can_proc_from_procs: true,
                trigger_immediately: true,
                ..ProcTrigger::default()
            },
        );
    }

    fn apply_elemental_fury(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("elemental_fury");
        if points == 0 {
            return;
        }
        // The talent's class mask (16089) also covers Flametongue Attack (bit 21) and
        // Frostbrand Attack (bit 24): shamans' Flametongue Weapon hits crit for 2.0x in logs.
        // Bit 10 is Lightning Shield, the orbs (26363..26370) included.
        static_mod(
            sim,
            unit,
            SpellModType::CritMultiplierFlat,
            masks::FIRE_TOTEM
                | masks::FIRE
                | masks::NATURE
                | masks::FROST
                | masks::FLAMETONGUE_WEAPON
                | masks::FROSTBRAND_WEAPON
                | masks::LIGHTNING_SHIELD,
            spell_data()
                .elemental_fury
                .effect(
                    dbcenums::A_ADD_PCT_MODIFIER,
                    dbcenums::SPELLMOD_CRIT_DAMAGE_BONUS,
                )
                .fraction_at(points),
        );
    }

    fn apply_reverberation(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("reverberation");
        if points == 0 {
            return;
        }
        static_time_mod(
            sim,
            unit,
            SpellModType::CooldownFlat,
            masks::SHOCK,
            millis(
                spell_data()
                    .reverberation
                    .effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_COOLDOWN)
                    .value_at(points),
            ),
        );
    }

    /// Elemental Reach is new in Forever: 28999 adds 3/6 yards to Lightning Bolt, Chain Lightning
    /// and Lava Burst range and 8/15 to Flame Shock's (its two class masks).
    fn apply_elemental_reach(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("elemental_reach");
        if points == 0 {
            return;
        }
        static_mod(
            sim,
            unit,
            SpellModType::RangeFlat,
            masks::LIGHTNING_BOLT | masks::CHAIN_LIGHTNING | masks::LAVA_BURST,
            spell_data().elemental_reach.effect_at(1).value_at(points),
        );
        static_mod(
            sim,
            unit,
            SpellModType::RangeFlat,
            masks::FLAME_SHOCK,
            spell_data().elemental_reach.effect_at(2).value_at(points),
        );
    }

    /// Elemental Alacrity is new in Forever: a flat cast-time cut.
    fn apply_elemental_alacrity(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("elemental_alacrity");
        if points == 0 {
            return;
        }
        static_time_mod(
            sim,
            unit,
            SpellModType::CastTimeFlat,
            masks::LIGHTNING_BOLT | masks::CHAIN_LIGHTNING | masks::LAVA_BURST,
            millis(
                spell_data()
                    .elemental_alacrity
                    .effect(
                        dbcenums::A_ADD_FLAT_MODIFIER,
                        dbcenums::SPELLMOD_CASTING_TIME,
                    )
                    .value_at(points),
            ),
        );
    }

    /// Elemental Warding is new in Forever: less fire, frost and nature damage taken (the
    /// client states one modifier over the three-school mask 28).
    fn apply_elemental_warding(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("elemental_warding");
        if points == 0 {
            return;
        }
        let multiplier = spell_data()
            .elemental_warding
            .effect(dbcenums::A_MOD_DAMAGE_PERCENT_TAKEN, 28)
            .multiplier_at(points);
        for index in [SchoolIndex::Fire, SchoolIndex::Frost, SchoolIndex::Nature] {
            sim.unit_mut(unit)
                .pseudo_stats
                .school_damage_taken_multiplier[school_array_index(index)] *= multiplier;
        }
    }

    /// Trial by Fire (16086, Improved Fire Nova until client 70425): more Fire Nova damage, a
    /// shorter cooldown on it and a longer Magma Totem, which registerMagmaTotemSpell reads.
    fn apply_trial_by_fire(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("trial_by_fire");
        if points == 0 {
            return;
        }
        let data = &spell_data().trial_by_fire;
        static_mod(
            sim,
            unit,
            SpellModType::DamageDoneFlat,
            masks::FIRE_NOVA,
            data.effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_DAMAGE)
                .fraction_at(points),
        );
        static_time_mod(
            sim,
            unit,
            SpellModType::CooldownFlat,
            masks::FIRE_NOVA,
            millis(
                data.effect(dbcenums::A_ADD_FLAT_MODIFIER, dbcenums::SPELLMOD_COOLDOWN)
                    .value_at(points),
            ),
        );
    }

    /// Go `registerEnhancementTalents`.
    pub(super) fn register_enhancement_talents(&mut self, sim: &mut Sim, unit: UnitId) {
        // Tier 1
        // Earth's Grasp changes no prepared state.
        self.apply_thundering_strikes(sim, unit);
        self.apply_ancestral_knowledge(sim, unit);

        // Tier 2
        // Guardian Totems and Improved Ghost Wolf change no prepared state.
        self.apply_mental_dexterity(sim, unit);
        self.apply_improved_lightning_shield(sim, unit);

        // Tier 3
        self.apply_elemental_weapons(sim, unit);
        self.apply_shamanistic_focus(sim, unit);
        self.apply_anticipation(sim, unit);

        // Tier 4
        self.apply_toughness(sim, unit);
        self.apply_flurry(sim, unit);
        self.apply_stormstrike(sim, unit);

        // Tier 5
        self.apply_spirit_weapons(sim, unit);
        self.apply_mental_quickness(sim, unit);
        self.apply_improved_stormstrike(sim, unit);

        // Tier 6
        self.apply_maelstrom_weapon(sim, unit);

        // Tier 7
        self.apply_rage_of_the_farseer(sim, unit);
    }

    fn apply_ancestral_knowledge(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("ancestral_knowledge");
        if points == 0 {
            return;
        }
        sim.unit_mut(unit).sdm.multiply_stat(
            Stat::Intellect,
            spell_data().ancestral_knowledge.multiplier_at(points),
        );
    }

    fn apply_elemental_weapons(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("elemental_weapons");
        if points == 0 {
            return;
        }
        // Effect 1 (Rockbiter's attack power) is read in RegisterRockbiterImbue.
        static_mod(
            sim,
            unit,
            SpellModType::DamageDoneFlat,
            masks::FLAMETONGUE_WEAPON | masks::FROSTBRAND_WEAPON,
            spell_data()
                .elemental_weapons
                .effect_at(2)
                .fraction_at(points),
        );
    }

    fn apply_flurry(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("flurry");
        if points == 0 {
            return;
        }
        let data = spell_data();
        // The 500 ms charge cooldown's timer.
        sim.new_timer(unit);

        let buff = data.flurry_triggered.highest();
        let attack_speed =
            crate::prepare::hotfix_113554::talent(sim.rules, 16256, 5).multiplier_at(points);

        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Flurry".to_string(),
                action_id: Some(spell_action(buff.id)),
                duration: buff.duration(),
                max_stacks: i32::from(buff.proc_charges),
                ..AuraConfig::default()
            },
        );
        sim.attach_multiply_melee_speed(aura, attack_speed);

        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Flurry Trigger".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                proc_mask: ProcMask::MELEE,
                // Forever's 16256 lacks the bit (Era's 16281-16284 carry it).
                can_proc_from_procs: data.flurry.highest().can_proc_from_procs(),
                ..ProcTrigger::default()
            },
        );
    }

    fn apply_improved_lightning_shield(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("improved_lightning_shield");
        if points == 0 {
            return;
        }
        static_mod(
            sim,
            unit,
            SpellModType::DamageDoneFlat,
            masks::LIGHTNING_SHIELD,
            spell_data().improved_lightning_shield.fraction_at(points),
        );
    }

    /// Forever's Mental Quickness (30812) turns Intellect into spell damage.
    fn apply_mental_quickness(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("mental_quickness");
        if points == 0 {
            return;
        }
        sim.unit_mut(unit).sdm.add_stat_dependency(
            Stat::Intellect,
            Stat::SpellDamage,
            spell_data()
                .mental_quickness
                .effect(dbcenums::A_MOD_SPELL_DAMAGE_OF_STAT_PERCENT, 126)
                .fraction_at(points),
        );
    }

    fn apply_shamanistic_focus(&self, sim: &mut Sim, unit: UnitId) {
        if !self.has_talent("shamanistic_focus") {
            return;
        }
        // 1223030: Shock and Lightning Shield cost 45% less, always.
        static_mod(
            sim,
            unit,
            SpellModType::PowerCostPctAdd,
            masks::SHOCK | masks::LIGHTNING_SHIELD,
            -0.45,
        );
    }

    fn apply_spirit_weapons(&self, sim: &mut Sim, unit: UnitId) {
        if !self.has_talent("spirit_weapons") {
            return;
        }
        // Client 16268: parry and -30% threat; its Rockbiter half (eff 1) is in
        // RegisterRockbiterImbue.
        let pseudo = &mut sim.unit_mut(unit).pseudo_stats;
        pseudo.can_parry = true;
        pseudo.threat_multiplier *= spell_data()
            .spirit_weapons
            .effect(dbcenums::A_MOD_THREAT, 127)
            .multiplier_at(1);
    }

    fn apply_stormstrike(&self, sim: &mut Sim, unit: UnitId) {
        if !self.has_talent("stormstrike") {
            return;
        }
        self.register_stormstrike_spell(sim, unit);
    }

    fn apply_thundering_strikes(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("thundering_strikes");
        if points == 0 {
            return;
        }
        static_mod(
            sim,
            unit,
            SpellModType::BonusCritPercent,
            0,
            spell_data().thundering_strikes.value_at(points),
        );
    }

    /// Mental Dexterity is new in Forever: Intellect into attack power.
    fn apply_mental_dexterity(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("mental_dexterity");
        if points == 0 {
            return;
        }
        let fraction = crate::prepare::hotfix_113554::talent(sim.rules, 415140, 3)
            .effect_at(1)
            .fraction_at(points);
        sim.unit_mut(unit)
            .sdm
            .add_stat_dependency(Stat::Intellect, Stat::AttackPower, fraction);
    }

    /// Anticipation is new in Forever: flat dodge. Go fuses the product into `AddStat`.
    fn apply_anticipation(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("anticipation");
        if points == 0 {
            return;
        }
        let value = spell_data()
            .anticipation
            .effect(dbcenums::A_MOD_DODGE_PERCENT, 0)
            .value_at(points);
        let stats = &mut sim.unit_mut(unit).stats;
        stats[Stat::DodgeRating] =
            DODGE_RATING_PER_DODGE_PERCENT.mul_add(value, stats[Stat::DodgeRating]);
    }

    /// Toughness is new in Forever: more Stamina.
    fn apply_toughness(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("toughness");
        if points == 0 {
            return;
        }
        sim.unit_mut(unit).sdm.multiply_stat(
            Stat::Stamina,
            spell_data()
                .toughness
                .effect(dbcenums::A_MOD_TOTAL_STAT_PERCENTAGE, 0)
                .multiplier_at(points),
        );
    }

    /// Improved Stormstrike is new in Forever: a chance on Stormstrike to regain mana while
    /// casting, and a chance for a dodge or parry taken to reset its cooldown.
    fn apply_improved_stormstrike(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("improved_stormstrike");
        if !self.has_talent("stormstrike") || points == 0 {
            return;
        }
        let data = spell_data();
        let chance = data.improved_stormstrike.effect_at(1).fraction_at(points);
        let buff = data.improved_stormstrike_triggered.highest();
        let regen_rate = buff
            .effect(dbcenums::A_MOD_MANA_REGEN_INTERRUPT, 0)
            .percent();

        sim.register_aura(
            unit,
            AuraConfig {
                label: "Improved Stormstrike".to_string(),
                action_id: Some(spell_action(buff.id)),
                duration: buff.duration(),
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.unit_mut(unit).pseudo_stats.spirit_regen_rate_casting += regen_rate;
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.unit_mut(unit).pseudo_stats.spirit_regen_rate_casting -= regen_rate;
                })),
                ..AuraConfig::default()
            },
        );

        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Improved Stormstrike Trigger".to_string(),
                callback: CallbackMask::ON_CAST_COMPLETE,
                class_spell_mask: masks::STORMSTRIKE_CAST,
                proc_chance: chance,
                ..ProcTrigger::default()
            },
        );

        sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Improved Stormstrike Reset".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_TAKEN,
                outcome: HitOutcome::DODGE | HitOutcome::PARRY,
                proc_chance: chance,
                ..ProcTrigger::default()
            },
        );
    }

    /// Maelstrom Weapon is new in Forever: melee hits stack a buff that makes the next Lightning
    /// Bolt faster and cheaper, and the cast consumes it. The client states no proc rate; beta
    /// logs give 10 PPM whatever the points, Season of Discovery's base (community #775).
    fn apply_maelstrom_weapon(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("maelstrom_weapon");
        if points == 0 {
            return;
        }
        let data = spell_data();
        let buff = data.maelstrom_weapon_triggered.highest();
        let max_stacks = 5;
        let per_stack = data.maelstrom_weapon.effect_at(1).fraction_at(points);

        // NewLegacyPPMManager: an item swap rebuilds it. The development notes of 2 November 2026
        // double the chance to trigger, so 20 PPM under Forever's rules.
        let ppm = match sim.rules {
            Rules::Forever => 20.0,
            Rules::GoReference => 10.0,
        };
        let ppmm = Rc::new(
            sim.new_ppm_manager(unit, ppm, ProcMask::MELEE)
                .rebuilt_by_swap(
                    sim,
                    unit,
                    crate::prepare::item_swap::ManagerChance::Ppm(ppm),
                    crate::prepare::item_swap::ManagerMask::Static(ProcMask::MELEE),
                ),
        );

        let cast_mod = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::CastTimePct,
                class_mask: masks::LIGHTNING_BOLT,
                ..SpellModConfig::default()
            },
        );
        let cost_mod = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::PowerCostPctAdd,
                class_mask: masks::LIGHTNING_BOLT,
                ..SpellModConfig::default()
            },
        );

        sim.register_aura(
            unit,
            AuraConfig {
                label: "Maelstrom Weapon".to_string(),
                action_id: Some(spell_action(buff.id)),
                duration: buff.duration(),
                max_stacks,
                on_stacks_change: Some(Rc::new(move |sim: &mut Sim, _, _, new_stacks| {
                    sim.update_spell_mod_float_value(cast_mod, per_stack * f64::from(new_stacks));
                    sim.update_spell_mod_float_value(cost_mod, per_stack * f64::from(new_stacks));
                    sim.activate_spell_mod(cast_mod);
                    sim.activate_spell_mod(cost_mod);
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.deactivate_spell_mod(cast_mod);
                    sim.deactivate_spell_mod(cost_mod);
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
                name: "Maelstrom Weapon Trigger".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_DEALT,
                proc_mask: ProcMask::MELEE,
                outcome: HitOutcome::LANDED,
                dpm: Some(ppmm),
                ..ProcTrigger::default()
            },
        );
    }

    /// Rage of the Farseer is new in Forever: a melee haste cooldown (425336). Build 70009
    /// dropped its cast-speed effect.
    fn apply_rage_of_the_farseer(&self, sim: &mut Sim, unit: UnitId) {
        if !self.has_talent("rage_of_the_farseer") {
            return;
        }
        let rank = spell_data().rage_of_the_farseer.highest();
        let multiplier = 1.0
            + rank
                .effect(dbcenums::A_MOD_MELEE_RANGED_HASTE_2, 0)
                .percent();

        let buff_aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Rage of the Farseer".to_string(),
                action_id: Some(spell_action(rank.id)),
                duration: rank.duration(),
                ..AuraConfig::default()
            },
        );
        sim.attach_multiply_melee_speed(buff_aura, multiplier);

        let timer = sim.new_timer(unit);
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(rank.id),
                flags: SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::APL,
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: longest_cooldown(rank),
                    },
                    ..CastConfig::default()
                },
                related_self_buff: Some(buff_aura),
                ..SpellConfig::default()
            },
        );
        sim.add_major_cooldown(
            unit,
            MajorCooldown {
                spell,
                priority: 0,
                cooldown_type: cooldown_type::DPS,
                allow_spell_queueing: false,
                timings: Vec::new(),
            },
        );
    }

    /// Go `registerRestorationTalents`.
    pub(super) fn register_restoration_talents(&mut self, sim: &mut Sim, unit: UnitId) {
        // Tier 1
        // Improved Healing Wave changes no number.
        self.apply_totemic_focus(sim, unit);

        // Tier 2
        self.apply_mindfulness(sim, unit);
        self.apply_natural_grace(sim, unit);
        self.apply_tidal_focus(sim, unit);
        self.apply_improved_reincarnation(sim, unit);

        // Tier 3
        // Ancestral Healing, Healing Focus and Water Shield (shields.rs) register nothing here.

        // Tier 4
        self.apply_tidal_mastery(sim, unit);
        // Restorative Totems (totems.rs) and Mana Tide Totem (the party buff) register nothing.

        // Tier 5
        self.apply_natures_swiftness(sim, unit);
        // Healing Way, Purification and Riptide change no number.
    }

    fn apply_natures_swiftness(&self, sim: &mut Sim, unit: UnitId) {
        if !self.has_talent("natures_swiftness") {
            return;
        }
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Nature's Swiftness".to_string(),
                action_id: Some(spell_action(16188)),
                // A Restoration healer sim's is up until a heal spends it (Rust only); Go
                // registers it without a duration.
                duration: if self.spec == super::Spec::Restoration {
                    NEVER_EXPIRES
                } else {
                    0
                },
                events: EventCallbacks {
                    on_cast_complete: true,
                    ..EventCallbacks::default()
                },
                ..AuraConfig::default()
            },
        );
        sim.attach_spell_mod(
            aura,
            SpellModConfig {
                kind: SpellModType::CastTimePct,
                float_value: -100.0,
                // The heals of a Restoration healer sim are Nature spells too.
                class_mask: masks::CHAIN_LIGHTNING | masks::LIGHTNING_BOLT | masks::HEAL,
                ..SpellModConfig::default()
            },
        );

        let timer = sim.new_timer(unit);
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(16188),
                spell_school: school::PHYSICAL,
                defense_type: DefenseType::Magic,
                flags: SpellFlag::APL | SpellFlag::NO_ON_CAST_COMPLETE | flags::INSTANT,
                cast: CastConfig {
                    cd: Cooldown {
                        timer: Some(timer),
                        duration: 180 * SECOND,
                    },
                    ..CastConfig::default()
                },
                ..SpellConfig::default()
            },
        );
    }

    fn apply_tidal_mastery(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("tidal_mastery");
        if points == 0 {
            return;
        }
        // Client 16194's mask is the heals, Lightning Shield and Rolling Thunder - no Lightning
        // Bolt or Chain Lightning.
        static_mod(
            sim,
            unit,
            SpellModType::BonusCritPercent,
            masks::LIGHTNING_SHIELD,
            spell_data()
                .tidal_mastery
                .effect(
                    dbcenums::A_ADD_FLAT_MODIFIER,
                    dbcenums::SPELLMOD_CRITICAL_CHANCE,
                )
                .value_at(points),
        );
    }

    fn apply_totemic_focus(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("totemic_focus");
        if points == 0 {
            return;
        }
        static_mod(
            sim,
            unit,
            SpellModType::PowerCostPctAdd,
            masks::TOTEM,
            spell_data()
                .totemic_focus
                .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_COST)
                .fraction_at(points),
        );
    }

    /// Mindfulness is new in Forever: mana regeneration continues while casting.
    fn apply_mindfulness(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("mindfulness");
        if points == 0 {
            return;
        }
        sim.unit_mut(unit).pseudo_stats.spirit_regen_rate_casting += spell_data()
            .mindfulness
            .effect(dbcenums::A_MOD_MANA_REGEN_INTERRUPT, 0)
            .fraction_at(points);
    }

    /// Tidal Focus is new in Forever: cheaper heals plus flat melee and spell hit. Go fuses
    /// each product into `AddStat`.
    fn apply_tidal_focus(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("tidal_focus");
        if points == 0 {
            return;
        }
        let data = &spell_data().tidal_focus;
        let melee = data.effect(dbcenums::A_MOD_HIT_CHANCE, 0).value_at(points);
        let spell = data
            .effect(dbcenums::A_MOD_SPELL_HIT_CHANCE, 0)
            .value_at(points);
        let stats = &mut sim.unit_mut(unit).stats;
        stats[Stat::MeleeHitRating] =
            PHYSICAL_HIT_RATING_PER_HIT_PERCENT.mul_add(melee, stats[Stat::MeleeHitRating]);
        stats[Stat::SpellHitRating] =
            SPELL_HIT_RATING_PER_HIT_PERCENT.mul_add(spell, stats[Stat::SpellHitRating]);
    }

    /// Natural Grace is new in Forever: less threat from the shaman's spells.
    fn apply_natural_grace(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("natural_grace");
        if points == 0 {
            return;
        }
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::ThreatMultiplierPct,
                float_value: spell_data()
                    .natural_grace
                    .effect(dbcenums::A_MOD_THREAT, 126)
                    .fraction_at(points),
                spell_flag: flags::SHAMAN_SPELL,
                ..SpellModConfig::default()
            },
        );
    }

    /// Improved Reincarnation is new in Forever. Only the maximum health half is modelled.
    fn apply_improved_reincarnation(&self, sim: &mut Sim, unit: UnitId) {
        let points = self.talent("improved_reincarnation");
        if points == 0 {
            return;
        }
        sim.unit_mut(unit).sdm.multiply_stat(
            Stat::Health,
            spell_data()
                .improved_reincarnation
                .effect_at(2)
                .multiplier_at(points),
        );
    }
}
