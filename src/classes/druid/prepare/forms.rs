//! Go sim/druid/forms.go: the form state, the paw weapons, the three form auras and the spells
//! that enter them.

use std::cell::Cell;
use std::rc::Rc;

use crate::classes::druid::forms::{ANY, BEAR, CAT, HUMANOID, MOONKIN};
use crate::contracts::prepared_v2::ActionId;
use crate::prepare::attack::Weapon;
use crate::prepare::character::constants::{
    CHARACTER_LEVEL, DEFAULT_ATTACK_POWER_PER_DPS, MAX_MELEE_RANGE,
};
use crate::prepare::sim::{
    AuraConfig, AuraId, BuildPhase, PowerBar, Sim, SpellId, StateSlot, TimeSlot, UnitId,
    MILLISECOND, NEVER_EXPIRES,
};
use crate::prepare::spell::{Cast, CastConfig, CostOptions, SpellConfig, SpellFlag, GCD_DEFAULT};
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::spelldata::Ladder;
use crate::prepare::stats::{Stat, Stats};

use super::{masks, Druid};

/// Go `AnimalSpiritRegenSuppression`.
pub(crate) const ANIMAL_SPIRIT_REGEN_SUPPRESSION: f64 = 0.911337;
/// Go `BaseBearArmorMulti`.
pub(crate) const BASE_BEAR_ARMOR_MULTI: f64 = 4.6;
/// Go `BearFormThreatMultiplier`: client 70374 raised Classic's 30% to 50% (8 October beta
/// notes, community #798).
pub(crate) const BEAR_FORM_THREAT_MULTIPLIER: f64 = 1.5;

/// Bear Form's threat multiplier under these rules: Blizzard's development notes of 2 November
/// 2026 raise the bonus from 50% to 115% (wowsims/forever #822 takes 2.15 too). The Go reference
/// keeps [`BEAR_FORM_THREAT_MULTIPLIER`].
pub(crate) fn bear_form_threat_multiplier(rules: crate::prepare::sim::Rules) -> f64 {
    match rules {
        crate::prepare::sim::Rules::Forever => 2.15,
        crate::prepare::sim::Rules::GoReference => BEAR_FORM_THREAT_MULTIPLIER,
    }
}
/// Go `CatFormThreatMultiplier`.
pub(crate) const CAT_FORM_THREAT_MULTIPLIER: f64 = 0.71;
/// Go `MoonkinFormArmorMultiplier`.
pub(crate) const MOONKIN_FORM_ARMOR_MULTIPLIER: f64 = 4.6;

/// The druid state Go keeps on the `Druid` and its callbacks read and write, shared with them.
pub(crate) struct State {
    /// Go `druid.form`.
    pub form: StateSlot,
    pub starting_form: u8,
    pub cat_form_aura: Cell<Option<AuraId>>,
    pub bear_form_aura: Cell<Option<AuraId>>,
    pub moonkin_form_aura: Cell<Option<AuraId>>,
    pub enrage_aura: Cell<Option<AuraId>>,
    pub frenzied_regeneration_aura: Cell<Option<AuraId>>,
    pub maul_queue_aura: Cell<Option<AuraId>>,
    /// Go `WolfsheadShiftingPowerEnergy` and `WolfsheadEnrageRage`.
    pub wolfshead_shifting_power_energy: StateSlot,
    pub wolfshead_enrage_rage: StateSlot,
    pub last_cat_form_energy: StateSlot,
    pub last_cat_form_exit_at: TimeSlot,
}

impl State {
    /// The state, with what Go's callbacks change kept in `sim` (see [`StateSlot`]).
    pub(crate) fn new(sim: &mut Sim, starting_form: u8) -> State {
        State {
            form: sim.new_state_slot(f64::from(starting_form)),
            starting_form,
            cat_form_aura: Cell::new(None),
            bear_form_aura: Cell::new(None),
            moonkin_form_aura: Cell::new(None),
            enrage_aura: Cell::new(None),
            frenzied_regeneration_aura: Cell::new(None),
            maul_queue_aura: Cell::new(None),
            wolfshead_shifting_power_energy: sim.new_state_slot(0.0),
            wolfshead_enrage_rage: sim.new_state_slot(0.0),
            last_cat_form_energy: sim.new_state_slot(0.0),
            last_cat_form_exit_at: sim.new_time_slot(0),
        }
    }

    /// Go `druid.form`.
    pub(crate) fn form(&self, sim: &Sim) -> u8 {
        sim.state(self.form) as u8
    }

    pub(crate) fn set_form(&self, sim: &mut Sim, form: u8) {
        sim.set_state(self.form, f64::from(form));
    }

    /// Go `druid.InForm`.
    pub(crate) fn in_form(&self, sim: &Sim, form: u8) -> bool {
        self.form(sim) & form != 0
    }

    /// Go `druid.ClearForm`.
    pub(crate) fn clear_form(&self, sim: &mut Sim, unit: UnitId) {
        if self.in_form(sim, CAT) {
            if let Some(aura) = self.cat_form_aura.get() {
                sim.deactivate(aura);
            }
        } else if self.in_form(sim, BEAR) {
            if let Some(aura) = self.bear_form_aura.get() {
                sim.deactivate(aura);
            }
        } else if self.in_form(sim, MOONKIN) {
            if let Some(aura) = self.moonkin_form_aura.get() {
                sim.deactivate(aura);
            }
        }
        self.set_form(sim, HUMANOID);
        sim.unit_mut(unit).current_power_bar = PowerBar::Mana;
    }
}

/// The form names the exporter writes, in its order: `formNames`.
pub(crate) fn form_names(form: u8) -> Vec<&'static str> {
    [
        (HUMANOID, "humanoid"),
        (BEAR, "bear"),
        (CAT, "cat"),
        (MOONKIN, "moonkin"),
        (crate::classes::druid::forms::TREE, "tree"),
    ]
    .into_iter()
    .filter(|(bit, _)| form & bit != 0)
    .map(|(_, name)| name)
    .collect()
}

/// Go `Character.WeaponFromMainHand`.
pub(crate) fn weapon_from_main_hand(sim: &Sim, unit: UnitId) -> Weapon {
    match sim.mh_weapon(unit) {
        Some(item) => Weapon::from_item(item, sim.unit(unit).pseudo_stats.bonus_mh_dps),
        None => Weapon::unarmed(),
    }
}

/// Go `imbueFlatWeaponDamage` of the main hand imbue.
fn mh_imbue_flat_weapon_damage(sim: &Sim, unit: UnitId) -> f64 {
    match sim.character(unit).consumables.i32("mhImbue_id") {
        16138 | 16622 => 8.0,
        _ => 0.0,
    }
}

/// Go `druid.formWeapon`: the equipped weapon's damage range and bonus DPS rescaled to the
/// form's swing, with the flat weapon damage added after.
fn form_weapon(sim: &Sim, unit: UnitId, swing_speed: f64) -> Weapon {
    let weapon = weapon_from_main_hand(sim, unit);
    let mut scale = 1.0;
    if weapon.swing_speed > 0.0 {
        scale = swing_speed / weapon.swing_speed;
    }
    let (mut enchant, mut flat) = (0.0, 0.0);
    if let Some(item) = sim.mh_weapon(unit) {
        enchant = item.enchant.weapon_damage;
        flat = enchant + mh_imbue_flat_weapon_damage(sim, unit);
    }
    Weapon {
        // forms.go 79 and 80 are fused multiply-adds.
        base_damage_min: (weapon.base_damage_min - enchant).mul_add(scale, flat),
        base_damage_max: (weapon.base_damage_max - enchant).mul_add(scale, flat),
        swing_speed,
        normalized_swing_speed: swing_speed,
        attack_power_per_dps: DEFAULT_ATTACK_POWER_PER_DPS,
        max_range: MAX_MELEE_RANGE,
        ..Weapon::default()
    }
}

/// Go `GetCatWeapon`.
pub(crate) fn cat_weapon(sim: &Sim, unit: UnitId) -> Weapon {
    form_weapon(sim, unit, 1.0)
}

/// Go `GetBearWeapon`.
pub(crate) fn bear_weapon(sim: &Sim, unit: UnitId) -> Weapon {
    form_weapon(sim, unit, 2.5)
}

/// Predatory Strikes: attack power off level while in Cat or Bear Form.
fn predatory_strikes_ap_per_level(points: i32) -> f64 {
    Ladder::talent(16972, 3).effect_at(1).fraction_at(points)
}

/// Sharpened Claws: the critical strike chance a rank in Cat or Bear Form.
fn sharpened_claws_crit_percent(points: i32) -> f64 {
    Ladder::talent(16942, 2)
        .effect(crate::prepare::dbcenums::A_MOD_CRIT_PCT, 0)
        .value_at(points)
}

/// Heart of the Wild: +2% Strength a rank in Cat Form, which the client files under a dummy.
fn heart_of_the_wild_form_multiplier(points: i32) -> f64 {
    Ladder::talent(17003, 5).effect_at(3).multiplier_at(points)
}

/// Heart of the Wild: +4% Stamina a rank in Bear Form.
fn heart_of_the_wild_bear_stamina_multiplier(points: i32) -> f64 {
    Ladder::talent(17003, 5).effect_at(2).multiplier_at(points)
}

impl Druid {
    /// Go `druid.formShiftStats`.
    fn form_shift_stats(&self) -> Stats {
        let mut stats = Stats::default();
        stats[Stat::AttackPower] =
            predatory_strikes_ap_per_level(self.tal.predatory_strikes) * f64::from(CHARACTER_LEVEL);
        stats[Stat::PhysicalCritPercent] = sharpened_claws_crit_percent(self.tal.sharpened_claws);
        stats[Stat::SpellCritPercent] = sharpened_claws_crit_percent(self.tal.sharpened_claws);
        stats
    }

    /// Go `attachFormFaerieFireMods`.
    fn attach_form_faerie_fire_mods(&self, sim: &mut Sim, aura: AuraId) {
        sim.attach_spell_mod(
            aura,
            SpellModConfig {
                kind: SpellModType::PowerCostPctAdd,
                class_mask: masks::FAERIE_FIRE,
                float_value: -1.0,
                ..SpellModConfig::default()
            },
        );
    }

    /// Go `RegisterCatFormAura`.
    pub(super) fn register_cat_form_aura(&mut self, sim: &mut Sim) {
        let unit = self.unit;
        let st = Rc::clone(&self.st);
        let mut stat_bonus = self.form_shift_stats();
        let mut extra = Stats::default();
        extra[Stat::AttackPower] = 2.0 * f64::from(CHARACTER_LEVEL);
        stat_bonus = stat_bonus.add(&extra);

        // In Cat Form each point of Agility gives 1 AP, and Feral Attack Power converts 1:1.
        let agi_ap_dep =
            sim.new_dynamic_stat_dependency(unit, Stat::Agility, Stat::AttackPower, 1.0);
        let feral_ap_dep =
            sim.new_dynamic_stat_dependency(unit, Stat::FeralAttackPower, Stat::AttackPower, 1.0);
        let hotw_dep = (self.tal.heart_of_the_wild > 0).then(|| {
            sim.new_dynamic_multiply_stat(
                unit,
                Stat::Strength,
                heart_of_the_wild_form_multiplier(self.tal.heart_of_the_wild),
            )
        });

        let gain_state = Rc::clone(&st);
        let expire_state = Rc::clone(&st);
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Cat Form".to_string(),
                action_id: Some(ActionId::spell(768)),
                duration: NEVER_EXPIRES,
                build_phase: if st.starting_form & CAT != 0 {
                    BuildPhase::BASE
                } else {
                    BuildPhase::NONE
                },
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    if !sim.measuring_stats && gain_state.form(sim) != HUMANOID {
                        gain_state.clear_form(sim, unit);
                    }
                    gain_state.set_form(sim, CAT);
                    sim.unit_mut(unit).current_power_bar = PowerBar::Energy;

                    sim.unit_mut(unit).pseudo_stats.threat_multiplier *= CAT_FORM_THREAT_MULTIPLIER;
                    sim.unit_mut(unit).pseudo_stats.spirit_regen_multiplier *=
                        ANIMAL_SPIRIT_REGEN_SUPPRESSION;

                    sim.add_stats_dynamic(unit, &stat_bonus);
                    sim.enable_build_phase_stat_dep(unit, agi_ap_dep);
                    sim.enable_build_phase_stat_dep(unit, feral_ap_dep);
                    if let Some(dep) = hotw_dep {
                        sim.enable_build_phase_stat_dep(unit, dep);
                    }

                    if !sim.measuring_stats {
                        let weapon = cat_weapon(sim, unit);
                        sim.unit_mut(unit).auto_attacks.mh = weapon;
                    }
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    expire_state.set_form(sim, HUMANOID);

                    sim.unit_mut(unit).pseudo_stats.threat_multiplier /= CAT_FORM_THREAT_MULTIPLIER;
                    sim.unit_mut(unit).pseudo_stats.spirit_regen_multiplier /=
                        ANIMAL_SPIRIT_REGEN_SUPPRESSION;

                    sim.add_stats_dynamic(unit, &stat_bonus.invert());
                    sim.disable_build_phase_stat_dep(unit, agi_ap_dep);
                    sim.disable_build_phase_stat_dep(unit, feral_ap_dep);
                    if let Some(dep) = hotw_dep {
                        sim.disable_build_phase_stat_dep(unit, dep);
                    }

                    if !sim.measuring_stats {
                        let energy = sim.unit(unit).energy_bar.current_energy;
                        sim.set_state(expire_state.last_cat_form_energy, energy);
                        let now = sim.current_time;
                        sim.set_time_state(expire_state.last_cat_form_exit_at, now);

                        let weapon = weapon_from_main_hand(sim, unit);
                        sim.unit_mut(unit).auto_attacks.mh = weapon;
                    }
                })),
                ..AuraConfig::default()
            },
        );
        self.st.cat_form_aura.set(Some(aura));
        self.cat_form_aura = Some(aura);

        // movement.go NewPassiveMovementSpeedEffect(0.25).
        self.new_passive_movement_speed_effect(sim, aura, 0.25);

        // Cat Form (Passive) 3025: Faerie Fire costs nothing and its global cooldown is 0.5 sec
        // shorter in the form.
        self.attach_form_faerie_fire_mods(sim, aura);
        sim.attach_spell_mod(
            aura,
            SpellModConfig {
                kind: SpellModType::GlobalCooldownFlat,
                class_mask: masks::FAERIE_FIRE,
                time_value: -500 * MILLISECOND,
                ..SpellModConfig::default()
            },
        );
    }

    /// Go `aura.NewPassiveMovementSpeedEffect`.
    pub(super) fn new_passive_movement_speed_effect(
        &self,
        sim: &mut Sim,
        aura: AuraId,
        multiplier: f64,
    ) {
        let unit = self.unit;
        sim.new_exclusive_effect(
            aura,
            "PassiveMovementSpeed",
            true,
            multiplier,
            Some(Rc::new(move |sim: &mut Sim, _| {
                sim.multiply_movement_speed(unit, 1.0 + multiplier)
            })),
            Some(Rc::new(move |sim: &mut Sim, _| {
                sim.multiply_movement_speed(unit, 1.0 / (1.0 + multiplier))
            })),
        );
    }

    /// Go `registerCatFormSpell`.
    pub(super) fn register_cat_form_spell(&mut self, sim: &mut Sim) {
        let row = Ladder::ranked(&[768]).highest();
        let mana = row.mana_cost();
        let id = self.register_spell(
            sim,
            ANY,
            SpellConfig {
                action_id: ActionId::spell(768),
                class_spell_mask: masks::CAT_FORM,
                flags: SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::APL,
                cost: CostOptions {
                    mana_base_cost_percent: mana.base_cost_percent,
                    mana_flat_cost: mana.flat_cost,
                    ..CostOptions::default()
                },
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: GCD_DEFAULT,
                        ..Cast::default()
                    },
                    ignore_haste: true,
                    ..CastConfig::default()
                },
                ..SpellConfig::default()
            },
        );
        self.cat_form = Some(id);
    }

    /// Go `RegisterBearFormAura`.
    pub(super) fn register_bear_form_aura(&mut self, sim: &mut Sim) {
        let unit = self.unit;
        let st = Rc::clone(&self.st);
        // Dire Bear Form: 180 attack power at level 60 and 1240 health, both flat.
        let mut extra = Stats::default();
        extra[Stat::AttackPower] = 3.0 * f64::from(CHARACTER_LEVEL);
        extra[Stat::Health] = 1240.0;
        let stat_bonus = self.form_shift_stats().add(&extra);

        let feral_ap_dep =
            sim.new_dynamic_stat_dependency(unit, Stat::FeralAttackPower, Stat::AttackPower, 1.0);
        let hotw_dep = (self.tal.heart_of_the_wild > 0).then(|| {
            sim.new_dynamic_multiply_stat(
                unit,
                Stat::Stamina,
                heart_of_the_wild_bear_stamina_multiplier(self.tal.heart_of_the_wild),
            )
        });

        let bear_threat = bear_form_threat_multiplier(sim.rules);
        let gain_state = Rc::clone(&st);
        let expire_state = Rc::clone(&st);
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Bear Form".to_string(),
                action_id: Some(ActionId::spell(9634)),
                duration: NEVER_EXPIRES,
                build_phase: if st.starting_form & BEAR != 0 {
                    BuildPhase::BASE
                } else {
                    BuildPhase::NONE
                },
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    if !sim.measuring_stats && gain_state.form(sim) != HUMANOID {
                        gain_state.clear_form(sim, unit);
                    }
                    gain_state.set_form(sim, BEAR);
                    sim.unit_mut(unit).current_power_bar = PowerBar::Rage;

                    sim.unit_mut(unit).pseudo_stats.threat_multiplier *= bear_threat;
                    sim.unit_mut(unit).pseudo_stats.spirit_regen_multiplier *=
                        ANIMAL_SPIRIT_REGEN_SUPPRESSION;

                    sim.add_stats_dynamic(unit, &stat_bonus);
                    sim.apply_dynamic_equip_scaling(unit, Stat::Armor, BASE_BEAR_ARMOR_MULTI);
                    sim.apply_dynamic_equip_scaling(unit, Stat::BonusArmor, BASE_BEAR_ARMOR_MULTI);
                    sim.enable_build_phase_stat_dep(unit, feral_ap_dep);

                    if let Some(dep) = hotw_dep {
                        sim.enable_build_phase_stat_dep(unit, dep);
                    }

                    // The health fraction a shift keeps only matters once time has passed.
                    if !sim.measuring_stats {
                        let weapon = bear_weapon(sim, unit);
                        sim.unit_mut(unit).auto_attacks.mh = weapon;
                    }
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    expire_state.set_form(sim, HUMANOID);

                    sim.unit_mut(unit).pseudo_stats.threat_multiplier /= bear_threat;
                    sim.unit_mut(unit).pseudo_stats.spirit_regen_multiplier /=
                        ANIMAL_SPIRIT_REGEN_SUPPRESSION;

                    sim.add_stats_dynamic(unit, &stat_bonus.invert());
                    sim.apply_dynamic_equip_scaling(unit, Stat::Armor, 1.0 / BASE_BEAR_ARMOR_MULTI);
                    sim.apply_dynamic_equip_scaling(
                        unit,
                        Stat::BonusArmor,
                        1.0 / BASE_BEAR_ARMOR_MULTI,
                    );
                    sim.disable_build_phase_stat_dep(unit, feral_ap_dep);

                    if let Some(dep) = hotw_dep {
                        sim.disable_build_phase_stat_dep(unit, dep);
                    }

                    if !sim.measuring_stats {
                        for aura in [
                            expire_state.enrage_aura.get(),
                            expire_state.frenzied_regeneration_aura.get(),
                            expire_state.maul_queue_aura.get(),
                        ]
                        .into_iter()
                        .flatten()
                        {
                            sim.deactivate(aura);
                        }
                        let weapon = weapon_from_main_hand(sim, unit);
                        sim.unit_mut(unit).auto_attacks.mh = weapon;
                    }
                })),
                ..AuraConfig::default()
            },
        );
        self.st.bear_form_aura.set(Some(aura));
        self.bear_form_aura = Some(aura);

        self.attach_form_faerie_fire_mods(sim, aura);
    }

    /// Go `registerBearFormSpell`.
    pub(super) fn register_bear_form_spell(&mut self, sim: &mut Sim) {
        let row = Ladder::ranked(&[9634]).highest();
        let mana = row.mana_cost();
        let id = self.register_spell(
            sim,
            ANY,
            SpellConfig {
                action_id: ActionId::spell(9634),
                class_spell_mask: masks::BEAR_FORM,
                flags: SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::APL,
                cost: CostOptions {
                    mana_base_cost_percent: mana.base_cost_percent,
                    mana_flat_cost: mana.flat_cost,
                    ..CostOptions::default()
                },
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: GCD_DEFAULT,
                        ..Cast::default()
                    },
                    ignore_haste: true,
                    ..CastConfig::default()
                },
                ..SpellConfig::default()
            },
        );
        self.bear_form = Some(id);
    }

    /// Go `RegisterMoonkinFormAura`.
    pub(super) fn register_moonkin_form_aura(&mut self, sim: &mut Sim) {
        if !self.tal.moonkin_form {
            return;
        }
        let unit = self.unit;
        let st = Rc::clone(&self.st);
        let gain_state = Rc::clone(&st);
        let expire_state = Rc::clone(&st);
        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Moonkin Form".to_string(),
                action_id: Some(ActionId::spell(24858)),
                duration: NEVER_EXPIRES,
                build_phase: if st.starting_form & MOONKIN != 0 {
                    BuildPhase::BASE
                } else {
                    BuildPhase::NONE
                },
                on_gain: Some(Rc::new(move |sim: &mut Sim, _| {
                    if !sim.measuring_stats && gain_state.form(sim) != MOONKIN {
                        gain_state.clear_form(sim, unit);
                    }

                    sim.apply_dynamic_equip_scaling(
                        unit,
                        Stat::Armor,
                        MOONKIN_FORM_ARMOR_MULTIPLIER,
                    );

                    gain_state.set_form(sim, MOONKIN);
                    sim.unit_mut(unit).current_power_bar = PowerBar::Mana;
                })),
                on_expire: Some(Rc::new(move |sim: &mut Sim, _| {
                    sim.apply_dynamic_equip_scaling(
                        unit,
                        Stat::Armor,
                        1.0 / MOONKIN_FORM_ARMOR_MULTIPLIER,
                    );
                    expire_state.set_form(sim, HUMANOID);
                })),
                ..AuraConfig::default()
            },
        );
        self.st.moonkin_form_aura.set(Some(aura));
        self.moonkin_form_aura = Some(aura);
    }

    /// Go `RegisterMoonkinFormSpell`.
    pub(super) fn register_moonkin_form_spell(&mut self, sim: &mut Sim) {
        if !self.tal.moonkin_form {
            return;
        }
        let row = Ladder::ranked(&[24858]).highest();
        let mana = row.mana_cost();
        let id = self.register_spell(
            sim,
            ANY,
            SpellConfig {
                action_id: ActionId::spell(24858),
                class_spell_mask: masks::MOONKIN_FORM,
                flags: SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::APL,
                cost: CostOptions {
                    mana_base_cost_percent: mana.base_cost_percent,
                    mana_flat_cost: mana.flat_cost,
                    ..CostOptions::default()
                },
                cast: CastConfig {
                    default_cast: Cast {
                        gcd: GCD_DEFAULT,
                        ..Cast::default()
                    },
                    ignore_haste: true,
                    ..CastConfig::default()
                },
                ..SpellConfig::default()
            },
        );
        self.moonkin_form = Some(id);
    }
}

/// The spell a form id names in the spellbook: used by the exporter.
pub(crate) fn spell_position(sim: &Sim, unit: UnitId, spell: SpellId) -> i64 {
    sim.unit(unit)
        .spellbook
        .iter()
        .position(|s| *s == spell)
        .map_or(-1, |p| p as i64)
}
