//! Go sim/core/spell_mod.go and `AttachSpellMod` from aura_helpers.go: modifiers of the spells
//! a unit registers, applied to the spells they name now and to those registered later.
//!
//! A mod lives in the simulation's arena (`Sim::spell_mods`) and is addressed by `ModId`, as Go
//! shares one `*SpellMod` between the unit's handlers, the aura that toggles it and its custom
//! callbacks. Every apply and remove below keeps Go's operation order, so a value a mod leaves
//! on a spell matches Go bit for bit: multiplicative kinds divide on removal, additive ones
//! subtract, and the integer kinds wrap as Go's do.
//!
//! A talent registers a static mod with `Sim::add_static_mod`: the mod is active at once, applies
//! to the spells the unit has registered already and, through the unit's spell registration
//! handler, to each one it registers later. A mod an aura toggles is built with
//! `Sim::add_dynamic_mod` or `Sim::attach_spell_mod` and starts inactive.

use std::rc::Rc;

use crate::data::spells::ClassFlags;

use super::sim::{AuraId, Duration, Sim, SpellId, UnitId};
use super::spell::{DefenseType, ProcMask, Resource, SpellFlag};

/// Index of a mod in `Sim::spell_mods`: Go's `*SpellMod`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ModId(pub usize);

/// Go `SpellModApply` and `SpellModRemove`.
pub(crate) type SpellModApply = Rc<dyn Fn(&mut Sim, ModId, SpellId)>;
/// Go `SpellModOnReset`.
pub(crate) type SpellModOnReset = Rc<dyn Fn(&mut Sim, ModId)>;
/// A further test of the spells a mod names, beyond Go's filters.
pub(crate) type SpellModFilter = Rc<dyn Fn(&Sim, SpellId) -> bool>;

/// Go `SpellModType`. The default, `Custom`, needs `apply_custom` and `remove_custom`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SpellModType {
    /// Multiplies `spell.damage_multiplier`. +5% = 0.05. Uses `float_value`.
    DamageDonePct,
    /// Adds to `spell.damage_multiplier_additive`. Uses `float_value`.
    DamageDoneFlat,
    /// Multiplies `spell.cost.percent_modifier` by 1 + value. Uses `float_value`.
    PowerCostPct,
    /// Adds to `spell.cost.additive_percent_modifier`. Uses `float_value`.
    PowerCostPctAdd,
    /// Adds to `spell.cost.flat_modifier`. Uses `int_value`.
    PowerCostFlat,
    /// Adds to `spell.cd.duration`. Uses `time_value`.
    CooldownFlat,
    /// Multiplies `spell.cd_multiplier`. -5% = 0.95. Uses `float_value`.
    CooldownMultiplier,
    /// Adds to `spell.crit_multiplier_additive`. Uses `float_value`.
    CritMultiplierFlat,
    /// Multiplies `spell.crit_multiplier_pct` by 1 + value. Uses `float_value`.
    CritMultiplierPct,
    /// Adds to `spell.cast_time_multiplier`. Uses `float_value`.
    CastTimePct,
    /// Adds to `spell.default_cast.cast_time`. Uses `time_value`.
    CastTimeFlat,
    /// Adds to `spell.bonus_crit_percent`. Uses `float_value`.
    BonusCritPercent,
    /// Adds to `spell.bonus_hit_percent`. Uses `float_value`.
    BonusHitPercent,
    /// Adds to the base tick count of the spell's dots. Uses `int_value`.
    DotNumberOfTicksFlat,
    /// Adds to `spell.default_cast.gcd`. Uses `time_value`.
    GlobalCooldownFlat,
    /// Adds to the base tick length of the spell's dots. Uses `time_value`.
    DotTickLengthFlat,
    /// Adds to `spell.bonus_coefficient`. Uses `float_value`.
    BonusCoeffecientFlat,
    /// Sets `CAN_CAST_WHILE_MOVING`; removal toggles it, as Go's does.
    AllowCastWhileMoving,
    /// Go declares it without an implementation, so a mod of this kind cannot be built.
    AllowCastWhileChanneling,
    /// Adds to `spell.bonus_spell_damage`. Uses `float_value`.
    BonusSpellDamageFlat,
    /// Adds to `spell.bonus_expertise_percent`. Uses `float_value`.
    BonusExpertisePercent,
    /// Adds to the duration of the debuff auras under `key_value`. Uses `key_value`, `time_value`.
    DebuffDurationFlat,
    /// Adds to the shared cooldown and the self buff's duration. Uses `time_value`.
    BuffDurationFlat,
    /// User-defined: `apply_custom` and `remove_custom`.
    #[default]
    Custom,
    /// Adds to `spell.max_charges`. Uses `int_value`.
    ModChargesFlat,
    /// Multiplies the periodic damage multiplier of the spell's dots. Uses `float_value`.
    DotDamageDonePct,
    /// Multiplies the base duration multiplier of the spell's dots. Uses `float_value`.
    DotBaseDurationPct,
    /// Adds to the bonus coefficient of the spell's dots. Uses `float_value`.
    DotBonusCoeffecientFlat,
    /// Multiplies `spell.threat_multiplier` by 1 + value. Uses `float_value`.
    ThreatMultiplierPct,
    /// Adds to `spell.bonus_base_damage`. Uses `float_value`.
    BaseDamageFlat,
    /// Multiplies `spell.flat_threat_bonus` by 1 + value. Uses `float_value`.
    FlatThreatBonusPct,
    /// Adds to every duration the spell has: dots, self buff and related aura arrays.
    /// Uses `time_value`.
    DurationFlat,
    /// Adds to the max stacks of the self buff and the stacking auras of the aura arrays.
    /// Uses `int_value`.
    BuffMaxStacksFlat,
    /// Adds to `spell.max_range`; inert on spells without a range. Uses `float_value`.
    RangeFlat,
    /// Adds to `spell.direct_damage_multiplier_additive`. Uses `float_value`.
    DirectDamageDoneFlat,
}

impl SpellModType {
    /// Whether Go's `spellModMap` has the kind, so `buildMod` accepts it.
    fn is_implemented(self) -> bool {
        self != SpellModType::AllowCastWhileChanneling
    }
}

/// Go `SpellModConfig`.
#[derive(Clone, Default)]
pub(crate) struct SpellModConfig {
    pub class_mask: i64,
    /// The client's EffectSpellClassMask: the spells this mod names. A mod that sets both this
    /// and `class_mask` applies only to the spells both name.
    pub class_flags: ClassFlags,
    pub kind: SpellModType,
    pub school: u8,
    pub defense_type: DefenseType,
    pub proc_mask: ProcMask,
    pub spell_flag: SpellFlag,
    /// Go `ResourceType`; `None` is no filter. Go accepts only these four.
    pub resource_type: Option<Resource>,
    pub int_value: i32,
    pub time_value: Duration,
    pub float_value: f64,
    pub key_value: String,
    pub apply_custom: Option<SpellModApply>,
    pub remove_custom: Option<SpellModApply>,
    pub reset_custom: Option<SpellModOnReset>,
    /// Names only the spells it admits among those the filters above name. A mod of a built-in
    /// kind with a filter stays one an item swap can switch, where a custom mod does not.
    pub spell_filter: Option<SpellModFilter>,
    pub should_apply_to_pets: bool,
}

/// Go `SpellMod`.
#[derive(Clone)]
pub(crate) struct SpellMod {
    pub class_mask: i64,
    pub class_flags: ClassFlags,
    pub kind: SpellModType,
    pub school: u8,
    pub defense_type: DefenseType,
    pub proc_mask: ProcMask,
    pub spell_flag: SpellFlag,
    pub resource_type: Option<Resource>,
    spell_filter: Option<SpellModFilter>,
    float_value: f64,
    int_value: i32,
    time_value: Duration,
    key_value: String,
    apply_custom: Option<SpellModApply>,
    remove_custom: Option<SpellModApply>,
    on_reset: Option<SpellModOnReset>,
    pub is_active: bool,
    pub affected_spells: Vec<SpellId>,
    /// The auras this mod has written to, for the kinds that change an aura rather than the
    /// spell pointing at it. One aura is routinely shared by every rank of a family and a class
    /// mask names every rank, so the value would otherwise land on that aura once per rank.
    touched_auras: Vec<AuraId>,
}

impl SpellMod {
    /// Whether the mod may write to this aura, which it may once until it gives the aura up
    /// again.
    fn claim_aura(&mut self, aura: AuraId) -> bool {
        if self.touched_auras.contains(&aura) {
            return false;
        }
        self.touched_auras.push(aura);
        true
    }

    /// Whether the mod has written to this aura, giving it up if it has, so that turning the
    /// mod on again writes to it again.
    fn release_aura(&mut self, aura: AuraId) -> bool {
        match self
            .touched_auras
            .iter()
            .position(|touched| *touched == aura)
        {
            None => false,
            Some(index) => {
                self.touched_auras.remove(index);
                true
            }
        }
    }

    /// Go `GetIntValue`.
    pub fn int_value(&self) -> i32 {
        self.int_value
    }

    /// Go `GetFloatValue`.
    pub fn float_value(&self) -> f64 {
        self.float_value
    }

    /// Go `GetTimeValue`.
    pub fn time_value(&self) -> Duration {
        self.time_value
    }

    /// The mod's `KeyValue`.
    pub fn key_value(&self) -> &str {
        &self.key_value
    }
}

/// Go `ClassFlags.IsZero`.
pub(crate) fn class_flags_is_zero(flags: &ClassFlags) -> bool {
    flags.family == 0 && flags.mask == [0; 4]
}

/// Go `ClassFlags.Matches`: the families agree and a mask word overlaps.
pub(crate) fn class_flags_matches(flags: &ClassFlags, other: &ClassFlags) -> bool {
    if flags.family != other.family {
        return false;
    }
    flags.mask.iter().zip(&other.mask).any(|(a, b)| a & b != 0)
}

/// Go `shouldApply`: whether the mod names the spell.
pub(crate) fn should_apply(sim: &Sim, spell_id: SpellId, spell_mod: &SpellMod) -> bool {
    let spell = sim.spell(spell_id);
    if spell.flags.matches(SpellFlag::NO_SPELL_MODS) {
        return false;
    }

    if let Some(resource) = spell_mod.resource_type {
        match &spell.cost {
            None => return false,
            Some(cost) => {
                if cost.resource != resource {
                    return false;
                }
            }
        }
    }

    if spell_mod.class_mask > 0 && !spell.matches(spell_mod.class_mask) {
        return false;
    }

    if !class_flags_is_zero(&spell_mod.class_flags)
        && !class_flags_matches(&spell_mod.class_flags, &spell.class_flags)
    {
        return false;
    }

    if spell_mod.school > 0 && spell_mod.school & spell.spell_school == 0 {
        return false;
    }

    if spell_mod.defense_type != DefenseType::None && spell.defense_type != spell_mod.defense_type {
        return false;
    }

    if spell_mod.proc_mask.0 > 0 && !spell_mod.proc_mask.matches(spell.proc_mask) {
        return false;
    }

    // A modifier on the off-hand's hits alone is one on the off-hand weapon's, and an off-hand
    // hit with no weapon behind it, such as a shield's, takes none of them.
    if spell_mod.proc_mask.0 > 0
        && spell_mod.proc_mask.0 & !ProcMask::MELEE_OH.0 == 0
        && !sim.unit(spell.unit).auto_attacks.is_dual_wielding
    {
        return false;
    }

    if spell_mod.spell_flag.0 > 0 && !spell_mod.spell_flag.matches(spell.flags) {
        return false;
    }

    if let Some(filter) = &spell_mod.spell_filter {
        return filter(sim, spell_id);
    }

    true
}

impl Sim {
    /// The mod with this id.
    pub(crate) fn spell_mod(&self, id: ModId) -> &SpellMod {
        &self.spell_mods[id.0]
    }

    fn spell_mod_mut(&mut self, id: ModId) -> &mut SpellMod {
        &mut self.spell_mods[id.0]
    }

    /// Go `buildMod`.
    fn build_mod(&mut self, unit: UnitId, config: SpellModConfig) -> ModId {
        assert!(
            config.kind.is_implemented(),
            "SpellMod {:?} not implemented",
            config.kind
        );

        let (apply_custom, remove_custom, on_reset) = if config.kind == SpellModType::Custom {
            assert!(
                config.apply_custom.is_some() && config.remove_custom.is_some(),
                "ApplyCustom and RemoveCustom are mandatory fields for SpellMod_Custom"
            );
            (
                config.apply_custom,
                config.remove_custom,
                config.reset_custom,
            )
        } else {
            (None, None, builtin_on_reset(config.kind))
        };

        assert!(
            !(config.school > super::spell::school::NONE
                && config.kind == SpellModType::BonusHitPercent),
            "For Spell school specific hit modifiers use PseudoStats.SchoolBonusHitChance"
        );

        let id = ModId(self.spell_mods.len());
        self.spell_mods.push(SpellMod {
            class_mask: config.class_mask,
            class_flags: config.class_flags,
            kind: config.kind,
            school: config.school,
            defense_type: config.defense_type,
            proc_mask: config.proc_mask,
            spell_flag: config.spell_flag,
            resource_type: config.resource_type,
            spell_filter: config.spell_filter,
            float_value: config.float_value,
            int_value: config.int_value,
            time_value: config.time_value,
            key_value: config.key_value,
            apply_custom,
            remove_custom,
            on_reset,
            is_active: false,
            affected_spells: Vec::new(),
            touched_auras: Vec::new(),
        });

        self.on_spell_registered(
            unit,
            Rc::new(move |sim: &mut Sim, spell| {
                sim.mod_spell_registered(id, spell);
            }),
        );

        let has_reset = self.spell_mod(id).on_reset.is_some();
        if has_reset {
            self.register_reset_effect(unit, Rc::new(move |sim: &mut Sim| sim.mod_on_reset(id)));
        }

        if config.should_apply_to_pets {
            for pet in self.unit(unit).pets.clone() {
                self.on_spell_registered(
                    pet,
                    Rc::new(move |sim: &mut Sim, spell| {
                        sim.mod_spell_registered(id, spell);
                    }),
                );
                if has_reset {
                    self.register_reset_effect(
                        pet,
                        Rc::new(move |sim: &mut Sim| sim.mod_on_reset(id)),
                    );
                }
            }
        }

        id
    }

    /// The unit's spell registration handler of a mod: a spell the mod names joins its
    /// affected spells and takes the mod at once when it is active.
    fn mod_spell_registered(&mut self, id: ModId, spell: SpellId) {
        if should_apply(self, spell, self.spell_mod(id)) {
            self.spell_mod_mut(id).affected_spells.push(spell);
            if self.spell_mod(id).is_active {
                self.apply_spell_mod(id, spell);
            }
        }
    }

    fn mod_on_reset(&mut self, id: ModId) {
        if let Some(on_reset) = self.spell_mod(id).on_reset.clone() {
            on_reset(self, id);
        }
    }

    /// Go `unit.AddStaticMod`: a mod that is active from the start.
    pub(crate) fn add_static_mod(&mut self, unit: UnitId, config: SpellModConfig) {
        let id = self.build_mod(unit, config);
        self.activate_spell_mod(id);
    }

    /// Go `unit.AddDynamicMod`: a mod that starts inactive.
    pub(crate) fn add_dynamic_mod(&mut self, unit: UnitId, config: SpellModConfig) -> ModId {
        self.build_mod(unit, config)
    }

    /// Go `SpellMod.Activate`.
    pub(crate) fn activate_spell_mod(&mut self, id: ModId) {
        if self.spell_mod(id).is_active {
            return;
        }
        for spell in self.spell_mod(id).affected_spells.clone() {
            self.apply_spell_mod(id, spell);
        }
        self.spell_mod_mut(id).is_active = true;
    }

    /// Go `SpellMod.Deactivate`.
    pub(crate) fn deactivate_spell_mod(&mut self, id: ModId) {
        if !self.spell_mod(id).is_active {
            return;
        }
        for spell in self.spell_mod(id).affected_spells.clone() {
            self.remove_spell_mod(id, spell);
        }
        self.spell_mod_mut(id).is_active = false;
    }

    /// Go `SpellMod.UpdateIntValue`.
    pub(crate) fn update_spell_mod_int_value(&mut self, id: ModId, value: i32) {
        let active = self.spell_mod(id).is_active;
        if active {
            self.deactivate_spell_mod(id);
        }
        self.spell_mod_mut(id).int_value = value;
        if active {
            self.activate_spell_mod(id);
        }
    }

    /// Go `SpellMod.UpdateTimeValue`.
    pub(crate) fn update_spell_mod_time_value(&mut self, id: ModId, value: Duration) {
        let active = self.spell_mod(id).is_active;
        if active {
            self.deactivate_spell_mod(id);
        }
        self.spell_mod_mut(id).time_value = value;
        if active {
            self.activate_spell_mod(id);
        }
    }

    /// Go `SpellMod.UpdateFloatValue`.
    pub(crate) fn update_spell_mod_float_value(&mut self, id: ModId, value: f64) {
        let active = self.spell_mod(id).is_active;
        if active {
            self.deactivate_spell_mod(id);
        }
        self.spell_mod_mut(id).float_value = value;
        if active {
            self.activate_spell_mod(id);
        }
    }

    /// Go `aura.AttachSpellMod`: a mod that is active while the aura is, returning the aura.
    pub(crate) fn attach_spell_mod(&mut self, aura: AuraId, config: SpellModConfig) -> AuraId {
        let unit = self.aura(aura).unit;
        let id = self.add_dynamic_mod(unit, config);
        self.apply_on_gain(
            aura,
            Rc::new(move |sim: &mut Sim, _| sim.activate_spell_mod(id)),
        );
        self.apply_on_expire(
            aura,
            Rc::new(move |sim: &mut Sim, _| sim.deactivate_spell_mod(id)),
        );
        aura
    }

    /// Go `mod.Apply(mod, spell)`.
    fn apply_spell_mod(&mut self, id: ModId, spell: SpellId) {
        let m = self.spell_mod(id);
        if m.kind == SpellModType::Custom {
            if let Some(apply) = m.apply_custom.clone() {
                apply(self, id, spell);
            }
            return;
        }
        let (float, int, time) = (m.float_value, m.int_value, m.time_value);
        match m.kind {
            SpellModType::DamageDonePct => {
                self.spell_mut(spell).damage_multiplier *= 1.0 + float;
            }
            SpellModType::DamageDoneFlat => {
                self.spell_mut(spell).damage_multiplier_additive += float;
            }
            SpellModType::PowerCostPct => {
                if let Some(cost) = &mut self.spell_mut(spell).cost {
                    cost.percent_modifier *= 1.0 + float;
                }
            }
            SpellModType::PowerCostPctAdd => {
                if let Some(cost) = &mut self.spell_mut(spell).cost {
                    cost.additive_percent_modifier += float;
                }
            }
            SpellModType::PowerCostFlat => {
                if let Some(cost) = &mut self.spell_mut(spell).cost {
                    cost.flat_modifier = cost.flat_modifier.wrapping_add(int);
                }
            }
            SpellModType::CooldownFlat => {
                let cd = &mut self.spell_mut(spell).cd;
                cd.duration = cd.duration.wrapping_add(time);
            }
            SpellModType::CooldownMultiplier => {
                self.spell_mut(spell).cd_multiplier *= float;
            }
            SpellModType::CritMultiplierFlat => {
                self.spell_mut(spell).crit_multiplier_additive += float;
            }
            SpellModType::CritMultiplierPct => {
                self.spell_mut(spell).crit_multiplier_pct *= 1.0 + float;
            }
            SpellModType::CastTimePct => {
                self.spell_mut(spell).cast_time_multiplier += float;
            }
            SpellModType::CastTimeFlat => {
                let cast = &mut self.spell_mut(spell).default_cast;
                cast.cast_time = cast.cast_time.wrapping_add(time);
            }
            SpellModType::BonusCritPercent => {
                self.spell_mut(spell).bonus_crit_percent += float;
            }
            SpellModType::BonusHitPercent => {
                self.spell_mut(spell).bonus_hit_percent += float;
            }
            SpellModType::DotNumberOfTicksFlat => {
                for dot in self.spell_dots(spell) {
                    let dot = &mut self.dots[dot.0];
                    dot.base_tick_count = dot.base_tick_count.wrapping_add(int);
                }
            }
            SpellModType::GlobalCooldownFlat => {
                let cast = &mut self.spell_mut(spell).default_cast;
                cast.gcd = cast.gcd.wrapping_add(time);
            }
            SpellModType::DotTickLengthFlat => {
                for dot in self.spell_dots(spell) {
                    let dot = &mut self.dots[dot.0];
                    dot.base_tick_length = dot.base_tick_length.wrapping_add(time);
                }
            }
            SpellModType::BonusCoeffecientFlat => {
                self.spell_mut(spell).bonus_coefficient += float;
            }
            SpellModType::AllowCastWhileMoving => {
                self.spell_mut(spell).flags |= SpellFlag::CAN_CAST_WHILE_MOVING;
            }
            SpellModType::BonusSpellDamageFlat => {
                self.spell_mut(spell).bonus_spell_damage += float;
            }
            SpellModType::BonusExpertisePercent => {
                self.spell_mut(spell).bonus_expertise_percent += float;
            }
            SpellModType::DebuffDurationFlat => {
                self.mod_debuff_duration_flat(id, spell, time, true);
            }
            SpellModType::BuffDurationFlat => {
                self.mod_buff_duration_flat(id, spell, time, true);
            }
            SpellModType::ModChargesFlat => {
                let spell = self.spell_mut(spell);
                spell.max_charges = spell.max_charges.wrapping_add(int);
                assert!(
                    spell.max_charges >= 0,
                    "Reducing the charges below 0 is not supported. Something seems wrong."
                );
                if int > 0 {
                    spell.charges = spell.charges.wrapping_add(int);
                }
                if spell.charges > spell.max_charges {
                    spell.charges = spell.max_charges;
                }
            }
            SpellModType::DotDamageDonePct => {
                for dot in self.spell_dots(spell) {
                    self.dots[dot.0].periodic_damage_multiplier *= 1.0 + float;
                }
            }
            SpellModType::DotBaseDurationPct => {
                for dot in self.spell_dots(spell) {
                    self.dots[dot.0].base_duration_multiplier *= 1.0 + float;
                }
            }
            SpellModType::DotBonusCoeffecientFlat => {
                for dot in self.spell_dots(spell) {
                    self.dots[dot.0].bonus_coefficient += float;
                }
            }
            SpellModType::ThreatMultiplierPct => {
                self.spell_mut(spell).threat_multiplier *= 1.0 + float;
            }
            SpellModType::BaseDamageFlat => {
                self.spell_mut(spell).bonus_base_damage += float;
            }
            SpellModType::FlatThreatBonusPct => {
                self.spell_mut(spell).flat_threat_bonus *= 1.0 + float;
            }
            SpellModType::DurationFlat => {
                self.mod_duration_flat(id, spell, time, true);
            }
            SpellModType::BuffMaxStacksFlat => {
                self.mod_buff_max_stacks_flat(id, spell, int, true);
            }
            SpellModType::RangeFlat => {
                // MaxRange 0 is no range check at all, so there is nothing for the mod to move:
                // writing to it would hand the spell a range the client never gave it. A spell
                // that does state one may not be shortened past nothing.
                let spell = self.spell_mut(spell);
                if spell.max_range == 0.0 {
                    return;
                }
                assert!(
                    spell.max_range + float > 0.0,
                    "Spell mod would leave {} at {:.1} yards of range. Something seems wrong.",
                    spell.action_id,
                    spell.max_range + float
                );
                spell.max_range += float;
            }
            SpellModType::DirectDamageDoneFlat => {
                self.spell_mut(spell).direct_damage_multiplier_additive += float;
            }
            SpellModType::Custom | SpellModType::AllowCastWhileChanneling => {}
        }
    }

    /// Go `mod.Remove(mod, spell)`.
    fn remove_spell_mod(&mut self, id: ModId, spell: SpellId) {
        let m = self.spell_mod(id);
        if m.kind == SpellModType::Custom {
            if let Some(remove) = m.remove_custom.clone() {
                remove(self, id, spell);
            }
            return;
        }
        let (float, int, time) = (m.float_value, m.int_value, m.time_value);
        match m.kind {
            SpellModType::DamageDonePct => {
                self.spell_mut(spell).damage_multiplier /= 1.0 + float;
            }
            SpellModType::DamageDoneFlat => {
                self.spell_mut(spell).damage_multiplier_additive -= float;
            }
            SpellModType::PowerCostPct => {
                if let Some(cost) = &mut self.spell_mut(spell).cost {
                    cost.percent_modifier /= 1.0 + float;
                }
            }
            SpellModType::PowerCostPctAdd => {
                if let Some(cost) = &mut self.spell_mut(spell).cost {
                    cost.additive_percent_modifier -= float;
                }
            }
            SpellModType::PowerCostFlat => {
                if let Some(cost) = &mut self.spell_mut(spell).cost {
                    cost.flat_modifier = cost.flat_modifier.wrapping_sub(int);
                }
            }
            SpellModType::CooldownFlat => {
                let cd = &mut self.spell_mut(spell).cd;
                cd.duration = cd.duration.wrapping_sub(time);
            }
            SpellModType::CooldownMultiplier => {
                self.spell_mut(spell).cd_multiplier /= float;
            }
            SpellModType::CritMultiplierFlat => {
                self.spell_mut(spell).crit_multiplier_additive -= float;
            }
            SpellModType::CritMultiplierPct => {
                self.spell_mut(spell).crit_multiplier_pct /= 1.0 + float;
            }
            SpellModType::CastTimePct => {
                self.spell_mut(spell).cast_time_multiplier -= float;
            }
            SpellModType::CastTimeFlat => {
                let cast = &mut self.spell_mut(spell).default_cast;
                cast.cast_time = cast.cast_time.wrapping_sub(time);
            }
            SpellModType::BonusCritPercent => {
                self.spell_mut(spell).bonus_crit_percent -= float;
            }
            SpellModType::BonusHitPercent => {
                self.spell_mut(spell).bonus_hit_percent -= float;
            }
            SpellModType::DotNumberOfTicksFlat => {
                for dot in self.spell_dots(spell) {
                    let dot = &mut self.dots[dot.0];
                    dot.base_tick_count = dot.base_tick_count.wrapping_sub(int);
                }
            }
            SpellModType::GlobalCooldownFlat => {
                let cast = &mut self.spell_mut(spell).default_cast;
                cast.gcd = cast.gcd.wrapping_sub(time);
            }
            SpellModType::DotTickLengthFlat => {
                for dot in self.spell_dots(spell) {
                    let dot = &mut self.dots[dot.0];
                    dot.base_tick_length = dot.base_tick_length.wrapping_sub(time);
                }
            }
            SpellModType::BonusCoeffecientFlat => {
                self.spell_mut(spell).bonus_coefficient -= float;
            }
            SpellModType::AllowCastWhileMoving => {
                // Go XORs the flag away, so removing a mod from a spell that already had the
                // flag sets it and a second mod's removal clears it.
                self.spell_mut(spell).flags.0 ^= SpellFlag::CAN_CAST_WHILE_MOVING.0;
            }
            SpellModType::BonusSpellDamageFlat => {
                self.spell_mut(spell).bonus_spell_damage -= float;
            }
            SpellModType::BonusExpertisePercent => {
                self.spell_mut(spell).bonus_expertise_percent -= float;
            }
            SpellModType::DebuffDurationFlat => {
                self.mod_debuff_duration_flat(id, spell, time.wrapping_neg(), false);
            }
            SpellModType::BuffDurationFlat => {
                self.mod_buff_duration_flat(id, spell, time.wrapping_neg(), false);
            }
            SpellModType::ModChargesFlat => {
                let spell = self.spell_mut(spell);
                spell.max_charges = spell.max_charges.wrapping_sub(int);
                assert!(
                    spell.max_charges >= 0,
                    "Reducing the charges below 0 is not supported. Something seems wrong."
                );
                if int < 0 {
                    spell.charges = spell.charges.wrapping_sub(int);
                }
                if spell.charges > spell.max_charges {
                    spell.charges = spell.max_charges;
                }
            }
            SpellModType::DotDamageDonePct => {
                for dot in self.spell_dots(spell) {
                    self.dots[dot.0].periodic_damage_multiplier /= 1.0 + float;
                }
            }
            SpellModType::DotBaseDurationPct => {
                for dot in self.spell_dots(spell) {
                    self.dots[dot.0].base_duration_multiplier /= 1.0 + float;
                }
            }
            SpellModType::DotBonusCoeffecientFlat => {
                for dot in self.spell_dots(spell) {
                    self.dots[dot.0].bonus_coefficient -= float;
                }
            }
            SpellModType::ThreatMultiplierPct => {
                self.spell_mut(spell).threat_multiplier /= 1.0 + float;
            }
            SpellModType::BaseDamageFlat => {
                self.spell_mut(spell).bonus_base_damage -= float;
            }
            SpellModType::FlatThreatBonusPct => {
                self.spell_mut(spell).flat_threat_bonus /= 1.0 + float;
            }
            SpellModType::DurationFlat => {
                self.mod_duration_flat(id, spell, time.wrapping_neg(), false);
            }
            SpellModType::BuffMaxStacksFlat => {
                self.mod_buff_max_stacks_flat(id, spell, int.wrapping_neg(), false);
            }
            SpellModType::RangeFlat => {
                let spell = self.spell_mut(spell);
                if spell.max_range == 0.0 {
                    return;
                }
                spell.max_range -= float;
            }
            SpellModType::DirectDamageDoneFlat => {
                self.spell_mut(spell).direct_damage_multiplier_additive -= float;
            }
            SpellModType::Custom | SpellModType::AllowCastWhileChanneling => {}
        }
    }

    /// The dots a spell owns: one per target, then the aoe dot, as Go's loops visit them.
    fn spell_dots(&self, spell: SpellId) -> Vec<super::sim::DotId> {
        let spell = self.spell(spell);
        spell
            .dots
            .iter()
            .flatten()
            .copied()
            .chain(spell.aoe_dot)
            .collect()
    }

    /// Whether the mod may write to the aura (claiming it) or gives it up (releasing it).
    fn claim_or_release(&mut self, id: ModId, aura: AuraId, claim: bool) -> bool {
        let m = self.spell_mod_mut(id);
        if claim {
            m.claim_aura(aura)
        } else {
            m.release_aura(aura)
        }
    }

    fn add_aura_duration(&mut self, aura: AuraId, value: Duration) {
        let aura = self.aura_mut(aura);
        aura.duration = aura.duration.wrapping_add(value);
    }

    /// Go `modDebuffDurationFlat`.
    fn mod_debuff_duration_flat(
        &mut self,
        id: ModId,
        spell: SpellId,
        value: Duration,
        claim: bool,
    ) {
        let key = self.spell_mod(id).key_value.clone();
        let array = match self.spell(spell).related_aura_arrays.get(&key) {
            Some(array) => array.clone(),
            None => panic!("No debuff found for key: {key}"),
        };
        for aura in array.into_iter().flatten() {
            if self.claim_or_release(id, aura, claim) {
                self.add_aura_duration(aura, value);
            }
        }
    }

    /// Go `modBuffDurationFlat`. The shared cooldown belongs to the spell rather than to the
    /// buff, so every spell the mod names takes it while the buff takes the duration once.
    fn mod_buff_duration_flat(&mut self, id: ModId, spell: SpellId, value: Duration, claim: bool) {
        let spell_ref = self.spell_mut(spell);
        if spell_ref.shared_cd.duration != 0 {
            spell_ref.shared_cd.duration = spell_ref.shared_cd.duration.wrapping_add(value);
        }
        if let Some(buff) = self.spell(spell).related_self_buff {
            if self.claim_or_release(id, buff, claim) {
                self.add_aura_duration(buff, value);
            }
        }
    }

    /// Go `modDurationFlat`. A dot belongs to the spell that ticks it, so every spell the mod
    /// names takes the value; the auras are shared between the ranks of a family and take it
    /// once each.
    fn mod_duration_flat(&mut self, id: ModId, spell: SpellId, value: Duration, claim: bool) {
        for dot in self.spell_dots(spell) {
            let dot = &mut self.dots[dot.0];
            dot.base_duration_flat = dot.base_duration_flat.wrapping_add(value);
        }
        if let Some(buff) = self.spell(spell).related_self_buff {
            if self.claim_or_release(id, buff, claim) {
                self.add_aura_duration(buff, value);
            }
        }
        let arrays: Vec<AuraId> = self
            .spell(spell)
            .related_aura_arrays
            .values()
            .flat_map(|array| array.iter().flatten().copied())
            .collect();
        for aura in arrays {
            if self.claim_or_release(id, aura, claim) {
                self.add_aura_duration(aura, value);
            }
        }
    }

    /// Go `modBuffMaxStacksFlat`. An aura the client never gives stacks stays at max stacks 0,
    /// which is what `set_stacks` refuses to touch.
    fn mod_buff_max_stacks_flat(&mut self, id: ModId, spell: SpellId, value: i32, claim: bool) {
        if let Some(buff) = self.spell(spell).related_self_buff {
            if self.aura(buff).max_stacks > 0 && self.claim_or_release(id, buff, claim) {
                self.add_buff_max_stacks(buff, value);
            }
        }
        let arrays: Vec<AuraId> = self
            .spell(spell)
            .related_aura_arrays
            .values()
            .flat_map(|array| array.iter().flatten().copied())
            .collect();
        for aura in arrays {
            if self.aura(aura).max_stacks > 0 && self.claim_or_release(id, aura, claim) {
                self.add_buff_max_stacks(aura, value);
            }
        }
    }

    /// Go `addBuffMaxStacks`. Both loops skip an aura at 0 stacks, so a mod that took one there
    /// could not give them back when it is removed; refusing it keeps apply and remove
    /// symmetric.
    fn add_buff_max_stacks(&mut self, aura: AuraId, value: i32) {
        let aura = self.aura_mut(aura);
        let stacks = aura.max_stacks.wrapping_add(value);
        assert!(
            stacks > 0,
            "Spell mod would leave the aura {} at {} max stacks. Something seems wrong.",
            aura.label,
            stacks
        );
        aura.max_stacks = stacks;
    }
}

/// The reset hook Go's `spellModMap` gives a kind: the additive damage kinds round away the
/// floating point error that many additions and subtractions in random order leave behind.
fn builtin_on_reset(kind: SpellModType) -> Option<SpellModOnReset> {
    match kind {
        SpellModType::DamageDoneFlat => Some(Rc::new(|sim: &mut Sim, id: ModId| {
            for spell in sim.spell_mod(id).affected_spells.clone() {
                let spell = sim.spell_mut(spell);
                spell.damage_multiplier_additive =
                    (spell.damage_multiplier_additive * 10000.0).round() / 10000.0;
            }
        })),
        SpellModType::DirectDamageDoneFlat => Some(Rc::new(|sim: &mut Sim, id: ModId| {
            for spell in sim.spell_mod(id).affected_spells.clone() {
                let spell = sim.spell_mut(spell);
                spell.direct_damage_multiplier_additive =
                    (spell.direct_damage_multiplier_additive * 10000.0).round() / 10000.0;
            }
        })),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::contracts::prepared_v2::ActionId;
    use crate::prepare::sim::{AuraConfig, Cooldown, Unit, UnitType, SECOND};
    use crate::prepare::spell::{school, Cast, CastConfig, CostOptions, DotConfig, SpellConfig};

    fn new_unit(sim: &mut Sim) -> UnitId {
        let mut unit = Unit::new(UnitType::Player, "mage".to_string());
        unit.mana_bar.base_mana = 1000.0;
        sim.add_unit(unit)
    }

    fn action(spell_id: i32) -> ActionId {
        ActionId {
            spell_id,
            ..Default::default()
        }
    }

    fn config(spell_id: i32) -> SpellConfig {
        SpellConfig {
            action_id: action(spell_id),
            ..Default::default()
        }
    }

    fn flags(family: i32, word0: u32, word1: u32) -> ClassFlags {
        ClassFlags {
            family,
            mask: [word0, word1, 0, 0],
        }
    }

    fn mod_of(kind: SpellModType, float_value: f64) -> SpellModConfig {
        SpellModConfig {
            kind,
            float_value,
            ..Default::default()
        }
    }

    // Go TestFlatThreatBonusPctMod: the Rust spell carries the same flat threat bonus Go's
    // ThreatFromDamage reads, so the assertions are on the value that function returns.
    #[test]
    fn flat_threat_bonus_pct_mod() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                class_spell_mask: 1,
                flat_threat_bonus: 100.0,
                ..config(11597)
            },
        );
        assert!((sim.spell(spell).flat_threat_bonus - 100.0).abs() < 0.001);

        let id = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                class_mask: 1,
                ..mod_of(SpellModType::FlatThreatBonusPct, 0.15)
            },
        );
        sim.activate_spell_mod(id);
        assert!((sim.spell(spell).flat_threat_bonus - 115.0).abs() < 0.001);

        sim.deactivate_spell_mod(id);
        assert!((sim.spell(spell).flat_threat_bonus - 100.0).abs() < 0.001);
    }

    // Go TestClassFlagsModMatching.
    #[test]
    fn class_flags_mod_matching() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let id = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                class_flags: flags(4, 0b0110, 0),
                ..mod_of(SpellModType::DamageDonePct, 0.0)
            },
        );
        let cases = [
            ("overlapping mask", flags(4, 0b0100, 0), true),
            (
                "disjoint mask in the same family",
                flags(4, 0b1000, 0),
                false,
            ),
            ("same mask in another family", flags(9, 0b0110, 0), false),
            ("overlap in a later word", flags(4, 0, 1), false),
            ("no class flags at all", ClassFlags::default(), false),
        ];
        for (index, (name, class_flags, want)) in cases.into_iter().enumerate() {
            let spell = sim.register_spell(
                unit,
                SpellConfig {
                    class_flags,
                    ..config(100 + index as i32)
                },
            );
            assert_eq!(should_apply(&sim, spell, sim.spell_mod(id)), want, "{name}");
        }
    }

    // Go TestModWithBothMasksRequiresBoth.
    #[test]
    fn mod_with_both_masks_requires_both() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let id = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                class_mask: 1 << 3,
                class_flags: flags(4, 0b0001, 0),
                ..mod_of(SpellModType::DamageDonePct, 0.0)
            },
        );
        let cases = [
            ("both", 1 << 3, flags(4, 0b0001, 0), true),
            ("only the sim tag", 1 << 3, ClassFlags::default(), false),
            ("only the class flags", 0, flags(4, 0b0001, 0), false),
        ];
        for (index, (name, class_spell_mask, class_flags, want)) in cases.into_iter().enumerate() {
            let spell = sim.register_spell(
                unit,
                SpellConfig {
                    class_spell_mask,
                    class_flags,
                    ..config(200 + index as i32)
                },
            );
            assert_eq!(should_apply(&sim, spell, sim.spell_mod(id)), want, "{name}");
        }
    }

    // Go TestModWithoutMasksAppliesToEverything.
    #[test]
    fn mod_without_masks_applies_to_everything() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let id = sim.add_dynamic_mod(unit, mod_of(SpellModType::DamageDonePct, 0.0));
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                class_flags: flags(4, 1, 0),
                ..config(300)
            },
        );
        assert!(should_apply(&sim, spell, sim.spell_mod(id)));
    }

    fn damage_spell(spell_id: i32) -> SpellConfig {
        SpellConfig {
            spell_school: school::FIRE,
            proc_mask: ProcMask::SPELL_DAMAGE,
            damage_multiplier: 1.0,
            ..config(spell_id)
        }
    }

    #[test]
    fn filters_school_defense_proc_flag_and_no_spell_mods() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let id = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                school: school::FIRE | school::FROST,
                defense_type: DefenseType::Magic,
                proc_mask: ProcMask::SPELL_DAMAGE,
                spell_flag: SpellFlag::BINARY,
                ..mod_of(SpellModType::DamageDonePct, 0.1)
            },
        );
        let base = SpellConfig {
            defense_type: DefenseType::Magic,
            flags: SpellFlag::BINARY,
            ..damage_spell(1)
        };
        let cases = [
            ("matches", base.clone(), true),
            (
                "other school",
                SpellConfig {
                    spell_school: school::NATURE,
                    ..base.clone()
                },
                false,
            ),
            (
                "other defense type",
                SpellConfig {
                    defense_type: DefenseType::Melee,
                    ..base.clone()
                },
                false,
            ),
            (
                "other proc mask",
                SpellConfig {
                    proc_mask: ProcMask::SPELL_HEALING,
                    ..base.clone()
                },
                false,
            ),
            (
                "missing flag",
                SpellConfig {
                    flags: SpellFlag::NONE,
                    ..base.clone()
                },
                false,
            ),
            (
                "no spell mods",
                SpellConfig {
                    flags: SpellFlag::BINARY | SpellFlag::NO_SPELL_MODS,
                    ..base.clone()
                },
                false,
            ),
        ];
        for (index, (name, spell_config, want)) in cases.into_iter().enumerate() {
            let spell = sim.register_spell(
                unit,
                SpellConfig {
                    action_id: action(400 + index as i32),
                    ..spell_config
                },
            );
            assert_eq!(should_apply(&sim, spell, sim.spell_mod(id)), want, "{name}");
        }
    }

    #[test]
    fn off_hand_mods_need_a_dual_wielding_unit() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let off_hand = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                proc_mask: ProcMask::MELEE_OH,
                ..mod_of(SpellModType::DamageDonePct, 0.1)
            },
        );
        let both = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                proc_mask: ProcMask::MELEE,
                ..mod_of(SpellModType::DamageDonePct, 0.1)
            },
        );
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                spell_school: school::PHYSICAL,
                proc_mask: ProcMask::MELEE_OH_AUTO,
                damage_multiplier: 1.0,
                ..config(500)
            },
        );
        assert!(!should_apply(&sim, spell, sim.spell_mod(off_hand)));
        assert!(should_apply(&sim, spell, sim.spell_mod(both)));
        sim.unit_mut(unit).auto_attacks.is_dual_wielding = true;
        assert!(should_apply(&sim, spell, sim.spell_mod(off_hand)));
    }

    #[test]
    fn resource_filter_names_the_cost_kind() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let mana = sim.register_spell(
            unit,
            SpellConfig {
                cost: CostOptions {
                    mana_flat_cost: 100,
                    ..Default::default()
                },
                cast: CastConfig {
                    default_cast: Cast {
                        cost: 0.0,
                        gcd: SECOND,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..config(600)
            },
        );
        let free = sim.register_spell(unit, config(601));
        let energy_mod = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                resource_type: Some(Resource::Energy),
                ..mod_of(SpellModType::PowerCostPct, 0.1)
            },
        );
        let mana_mod = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                resource_type: Some(Resource::Mana),
                ..mod_of(SpellModType::PowerCostPct, 0.1)
            },
        );
        assert!(!should_apply(&sim, mana, sim.spell_mod(energy_mod)));
        assert!(should_apply(&sim, mana, sim.spell_mod(mana_mod)));
        assert!(!should_apply(&sim, free, sim.spell_mod(mana_mod)));
    }

    fn mana_spell(spell_id: i32) -> SpellConfig {
        SpellConfig {
            cost: CostOptions {
                mana_flat_cost: 200,
                ..Default::default()
            },
            cast: CastConfig {
                default_cast: Cast {
                    gcd: SECOND,
                    cast_time: 2 * SECOND,
                    ..Default::default()
                },
                cd: Cooldown {
                    timer: None,
                    duration: 0,
                },
                ..Default::default()
            },
            ..damage_spell(spell_id)
        }
    }

    #[test]
    fn static_mod_reaches_spells_registered_later() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let early = sim.register_spell(unit, mana_spell(700));
        sim.add_static_mod(
            unit,
            SpellModConfig {
                class_mask: 4,
                ..mod_of(SpellModType::DamageDonePct, 0.25)
            },
        );
        let late = sim.register_spell(
            unit,
            SpellConfig {
                class_spell_mask: 4,
                ..mana_spell(701)
            },
        );
        let other = sim.register_spell(
            unit,
            SpellConfig {
                class_spell_mask: 8,
                ..mana_spell(702)
            },
        );
        assert_eq!(sim.spell(early).damage_multiplier, 1.0);
        assert_eq!(sim.spell(late).damage_multiplier, 1.0 * (1.0 + 0.25));
        assert_eq!(sim.spell(other).damage_multiplier, 1.0);
    }

    #[test]
    fn dynamic_mod_applies_to_late_spells_only_while_active() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let id = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                class_mask: 4,
                ..mod_of(SpellModType::DamageDoneFlat, 0.1)
            },
        );
        let first = sim.register_spell(
            unit,
            SpellConfig {
                class_spell_mask: 4,
                ..mana_spell(710)
            },
        );
        assert_eq!(sim.spell(first).damage_multiplier_additive, 1.0);
        sim.activate_spell_mod(id);
        // Activating twice is a no-op.
        sim.activate_spell_mod(id);
        assert_eq!(sim.spell(first).damage_multiplier_additive, 1.0 + 0.1);
        let second = sim.register_spell(
            unit,
            SpellConfig {
                class_spell_mask: 4,
                ..mana_spell(711)
            },
        );
        assert_eq!(sim.spell(second).damage_multiplier_additive, 1.0 + 0.1);
        sim.deactivate_spell_mod(id);
        sim.deactivate_spell_mod(id);
        assert_eq!(
            sim.spell(first).damage_multiplier_additive,
            (1.0 + 0.1) - 0.1
        );
        assert_eq!(
            sim.spell(second).damage_multiplier_additive,
            (1.0 + 0.1) - 0.1
        );
    }

    #[test]
    fn spell_field_kinds_apply_and_remove_in_go_order() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                bonus_coefficient: 0.5,
                threat_multiplier: 1.0,
                flat_threat_bonus: 10.0,
                ..mana_spell(720)
            },
        );
        let float = 0.07;
        let kinds = [
            SpellModType::DamageDonePct,
            SpellModType::DamageDoneFlat,
            SpellModType::PowerCostPct,
            SpellModType::PowerCostPctAdd,
            SpellModType::CooldownMultiplier,
            SpellModType::CritMultiplierFlat,
            SpellModType::CritMultiplierPct,
            SpellModType::CastTimePct,
            SpellModType::BonusCritPercent,
            SpellModType::BonusHitPercent,
            SpellModType::BonusCoeffecientFlat,
            SpellModType::BonusSpellDamageFlat,
            SpellModType::BonusExpertisePercent,
            SpellModType::ThreatMultiplierPct,
            SpellModType::BaseDamageFlat,
            SpellModType::FlatThreatBonusPct,
            SpellModType::DirectDamageDoneFlat,
        ];
        let mut ids = Vec::new();
        for kind in kinds {
            ids.push(sim.add_dynamic_mod(unit, mod_of(kind, float)));
        }
        for id in &ids {
            sim.activate_spell_mod(*id);
        }
        let s = sim.spell(spell);
        assert_eq!(s.damage_multiplier, 1.0 * (1.0 + float));
        assert_eq!(s.damage_multiplier_additive, 1.0 + float);
        let cost = s.cost.as_ref().expect("a mana spell has a cost");
        assert_eq!(cost.percent_modifier, 1.0 * (1.0 + float));
        assert_eq!(cost.additive_percent_modifier, 1.0 + float);
        assert_eq!(s.cd_multiplier, 1.0 * float);
        assert_eq!(s.crit_multiplier_additive, 0.0 + float);
        assert_eq!(s.crit_multiplier_pct, 1.0 * (1.0 + float));
        assert_eq!(s.cast_time_multiplier, 1.0 + float);
        assert_eq!(s.bonus_crit_percent, float);
        assert_eq!(s.bonus_hit_percent, float);
        assert_eq!(s.bonus_coefficient, 0.5 + float);
        assert_eq!(s.bonus_spell_damage, float);
        assert_eq!(s.bonus_expertise_percent, float);
        assert_eq!(s.threat_multiplier, 1.0 * (1.0 + float));
        assert_eq!(s.bonus_base_damage, float);
        assert_eq!(s.flat_threat_bonus, 10.0 * (1.0 + float));
        assert_eq!(s.direct_damage_multiplier_additive, float);

        // Removal in the same order: multiplicative kinds divide.
        for id in &ids {
            sim.deactivate_spell_mod(*id);
        }
        let s = sim.spell(spell);
        assert_eq!(s.damage_multiplier, (1.0 * (1.0 + float)) / (1.0 + float));
        assert_eq!(s.damage_multiplier_additive, (1.0 + float) - float);
        assert_eq!(s.cd_multiplier, (1.0 * float) / float);
        assert_eq!(s.flat_threat_bonus, (10.0 * (1.0 + float)) / (1.0 + float));
        assert_eq!(s.bonus_coefficient, (0.5 + float) - float);
        assert_eq!(s.bonus_crit_percent, 0.0);
    }

    #[test]
    fn time_and_int_kinds_apply_and_remove() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let spell = sim.register_spell(unit, mana_spell(730));
        let cost_flat = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::PowerCostFlat,
                int_value: -50,
                ..Default::default()
            },
        );
        let cast_flat = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::CastTimeFlat,
                time_value: -500_000_000,
                ..Default::default()
            },
        );
        let gcd_flat = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::GlobalCooldownFlat,
                time_value: -250_000_000,
                ..Default::default()
            },
        );
        let cd_flat = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::CooldownFlat,
                time_value: 3 * SECOND,
                ..Default::default()
            },
        );
        for id in [cost_flat, cast_flat, gcd_flat, cd_flat] {
            sim.activate_spell_mod(id);
        }
        let s = sim.spell(spell);
        assert_eq!(s.cost.as_ref().map(|c| c.flat_modifier), Some(-50));
        assert_eq!(s.default_cast.cast_time, 1_500_000_000);
        assert_eq!(s.default_cast.gcd, 750_000_000);
        assert_eq!(s.cd.duration, 3 * SECOND);
        for id in [cost_flat, cast_flat, gcd_flat, cd_flat] {
            sim.deactivate_spell_mod(id);
        }
        let s = sim.spell(spell);
        assert_eq!(s.cost.as_ref().map(|c| c.flat_modifier), Some(0));
        assert_eq!(s.default_cast.cast_time, 2 * SECOND);
        assert_eq!(s.default_cast.gcd, SECOND);
        assert_eq!(s.cd.duration, 0);
    }

    fn dot_spell(spell_id: i32) -> SpellConfig {
        SpellConfig {
            dot: DotConfig {
                tick_length: 3 * SECOND,
                number_of_ticks: 4,
                is_aoe: true,
                aura: AuraConfig {
                    label: format!("Dot {spell_id}"),
                    ..Default::default()
                },
                bonus_coefficient: 0.2,
                ..Default::default()
            },
            ..mana_spell(spell_id)
        }
    }

    #[test]
    fn dot_kinds_apply_and_remove() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let spell = sim.register_spell(unit, dot_spell(740));
        let dot = sim.spell(spell).aoe_dot.expect("the spell has an aoe dot");

        let mods = [
            sim.add_dynamic_mod(
                unit,
                SpellModConfig {
                    kind: SpellModType::DotNumberOfTicksFlat,
                    int_value: 2,
                    ..Default::default()
                },
            ),
            sim.add_dynamic_mod(
                unit,
                SpellModConfig {
                    kind: SpellModType::DotTickLengthFlat,
                    time_value: -SECOND,
                    ..Default::default()
                },
            ),
            sim.add_dynamic_mod(unit, mod_of(SpellModType::DotDamageDonePct, 0.1)),
            sim.add_dynamic_mod(unit, mod_of(SpellModType::DotBaseDurationPct, 0.2)),
            sim.add_dynamic_mod(unit, mod_of(SpellModType::DotBonusCoeffecientFlat, 0.05)),
            sim.add_dynamic_mod(
                unit,
                SpellModConfig {
                    kind: SpellModType::DurationFlat,
                    time_value: 2 * SECOND,
                    ..Default::default()
                },
            ),
        ];
        for id in mods {
            sim.activate_spell_mod(id);
        }
        let d = &sim.dots[dot.0];
        assert_eq!(d.base_tick_count, 6);
        assert_eq!(d.base_tick_length, 2 * SECOND);
        assert_eq!(d.periodic_damage_multiplier, 1.0 * (1.0 + 0.1));
        assert_eq!(d.base_duration_multiplier, 1.0 * (1.0 + 0.2));
        assert_eq!(d.bonus_coefficient, 0.2 + 0.05);
        assert_eq!(d.base_duration_flat, 2 * SECOND);
        for id in mods {
            sim.deactivate_spell_mod(id);
        }
        let d = &sim.dots[dot.0];
        assert_eq!(d.base_tick_count, 4);
        assert_eq!(d.base_tick_length, 3 * SECOND);
        assert_eq!(
            d.periodic_damage_multiplier,
            (1.0 * (1.0 + 0.1)) / (1.0 + 0.1)
        );
        assert_eq!(d.bonus_coefficient, (0.2 + 0.05) - 0.05);
        assert_eq!(d.base_duration_flat, 0);
    }

    #[test]
    fn allow_cast_while_moving_removes_by_xor() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let spell = sim.register_spell(unit, mana_spell(750));
        let id = sim.add_dynamic_mod(unit, mod_of(SpellModType::AllowCastWhileMoving, 0.0));
        sim.activate_spell_mod(id);
        assert!(sim
            .spell(spell)
            .flags
            .matches(SpellFlag::CAN_CAST_WHILE_MOVING));
        sim.deactivate_spell_mod(id);
        assert!(!sim
            .spell(spell)
            .flags
            .matches(SpellFlag::CAN_CAST_WHILE_MOVING));

        // A spell that has the flag already loses it on removal, as Go's XOR does.
        let had = sim.register_spell(
            unit,
            SpellConfig {
                flags: SpellFlag::CAN_CAST_WHILE_MOVING,
                ..mana_spell(751)
            },
        );
        sim.activate_spell_mod(id);
        sim.deactivate_spell_mod(id);
        assert!(!sim
            .spell(had)
            .flags
            .matches(SpellFlag::CAN_CAST_WHILE_MOVING));
    }

    #[test]
    fn charges_mod_clamps_and_restores() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                charges: 2,
                recharge_time: 10 * SECOND,
                ..mana_spell(760)
            },
        );
        let id = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::ModChargesFlat,
                int_value: 1,
                ..Default::default()
            },
        );
        sim.activate_spell_mod(id);
        assert_eq!(
            (sim.spell(spell).max_charges, sim.spell(spell).charges),
            (3, 3)
        );
        sim.deactivate_spell_mod(id);
        assert_eq!(
            (sim.spell(spell).max_charges, sim.spell(spell).charges),
            (2, 2)
        );
    }

    #[test]
    #[should_panic(expected = "Reducing the charges below 0")]
    fn charges_mod_refuses_negative_max() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        sim.register_spell(
            unit,
            SpellConfig {
                charges: 1,
                recharge_time: 10 * SECOND,
                ..mana_spell(761)
            },
        );
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::ModChargesFlat,
                int_value: -2,
                ..Default::default()
            },
        );
    }

    #[test]
    fn range_mod_is_inert_without_a_range() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let none = sim.register_spell(unit, mana_spell(770));
        let ranged = sim.register_spell(
            unit,
            SpellConfig {
                max_range: 30.0,
                ..mana_spell(771)
            },
        );
        let id = sim.add_dynamic_mod(unit, mod_of(SpellModType::RangeFlat, 5.0));
        sim.activate_spell_mod(id);
        assert_eq!(sim.spell(none).max_range, 0.0);
        assert_eq!(sim.spell(ranged).max_range, 35.0);
        sim.deactivate_spell_mod(id);
        assert_eq!(sim.spell(none).max_range, 0.0);
        assert_eq!(sim.spell(ranged).max_range, 30.0);
    }

    #[test]
    #[should_panic(expected = "yards of range")]
    fn range_mod_refuses_to_shorten_past_nothing() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        sim.register_spell(
            unit,
            SpellConfig {
                max_range: 5.0,
                ..mana_spell(772)
            },
        );
        sim.add_static_mod(unit, mod_of(SpellModType::RangeFlat, -5.0));
    }

    fn buff(sim: &mut Sim, unit: UnitId, label: &str, max_stacks: i32) -> AuraId {
        sim.register_aura(
            unit,
            AuraConfig {
                label: label.to_string(),
                duration: 10 * SECOND,
                max_stacks,
                ..Default::default()
            },
        )
    }

    #[test]
    fn shared_auras_take_a_duration_mod_once_per_claim() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let shared = buff(&mut sim, unit, "Shared", 0);
        let rank1 = sim.register_spell(
            unit,
            SpellConfig {
                class_spell_mask: 1,
                related_self_buff: Some(shared),
                cast: CastConfig {
                    shared_cd: Cooldown::default(),
                    ..mana_spell(780).cast
                },
                ..mana_spell(780)
            },
        );
        let rank2 = sim.register_spell(
            unit,
            SpellConfig {
                class_spell_mask: 1,
                related_self_buff: Some(shared),
                ..mana_spell(781)
            },
        );
        let id = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                class_mask: 1,
                kind: SpellModType::BuffDurationFlat,
                time_value: 2 * SECOND,
                ..Default::default()
            },
        );
        sim.activate_spell_mod(id);
        assert_eq!(sim.aura(shared).duration, 12 * SECOND);
        sim.deactivate_spell_mod(id);
        assert_eq!(sim.aura(shared).duration, 10 * SECOND);
        // Turning it on again writes again.
        sim.activate_spell_mod(id);
        assert_eq!(sim.aura(shared).duration, 12 * SECOND);
        assert_eq!(sim.spell_mod(id).affected_spells, vec![rank1, rank2]);
    }

    #[test]
    fn duration_mod_touches_buffs_and_aura_arrays() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let self_buff = buff(&mut sim, unit, "Self", 0);
        let debuff_a = buff(&mut sim, unit, "Debuff A", 0);
        let debuff_b = buff(&mut sim, unit, "Debuff B", 0);
        let mut arrays = std::collections::BTreeMap::new();
        arrays.insert(
            "debuff".to_string(),
            vec![Some(debuff_a), None, Some(debuff_b)],
        );
        sim.register_spell(
            unit,
            SpellConfig {
                related_self_buff: Some(self_buff),
                related_aura_arrays: arrays,
                ..dot_spell(790)
            },
        );
        let all = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::DurationFlat,
                time_value: SECOND,
                ..Default::default()
            },
        );
        let keyed = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::DebuffDurationFlat,
                key_value: "debuff".to_string(),
                time_value: 3 * SECOND,
                ..Default::default()
            },
        );
        sim.activate_spell_mod(all);
        sim.activate_spell_mod(keyed);
        assert_eq!(sim.aura(self_buff).duration, 11 * SECOND);
        assert_eq!(sim.aura(debuff_a).duration, 14 * SECOND);
        assert_eq!(sim.aura(debuff_b).duration, 14 * SECOND);
        sim.deactivate_spell_mod(all);
        sim.deactivate_spell_mod(keyed);
        assert_eq!(sim.aura(self_buff).duration, 10 * SECOND);
        assert_eq!(sim.aura(debuff_a).duration, 10 * SECOND);
    }

    #[test]
    #[should_panic(expected = "No debuff found for key: missing")]
    fn debuff_duration_needs_its_key() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        sim.register_spell(unit, mana_spell(791));
        sim.add_static_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::DebuffDurationFlat,
                key_value: "missing".to_string(),
                ..Default::default()
            },
        );
    }

    #[test]
    fn max_stacks_mod_skips_non_stacking_auras() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let stacking = buff(&mut sim, unit, "Stacking", 3);
        let flat = buff(&mut sim, unit, "Flat", 0);
        let mut arrays = std::collections::BTreeMap::new();
        arrays.insert("x".to_string(), vec![Some(flat)]);
        sim.register_spell(
            unit,
            SpellConfig {
                related_self_buff: Some(stacking),
                related_aura_arrays: arrays,
                ..mana_spell(795)
            },
        );
        let id = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::BuffMaxStacksFlat,
                int_value: 2,
                ..Default::default()
            },
        );
        sim.activate_spell_mod(id);
        assert_eq!(sim.aura(stacking).max_stacks, 5);
        assert_eq!(sim.aura(flat).max_stacks, 0);
        sim.deactivate_spell_mod(id);
        assert_eq!(sim.aura(stacking).max_stacks, 3);
    }

    #[test]
    fn updating_a_value_reapplies_an_active_mod() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let spell = sim.register_spell(unit, mana_spell(800));
        let id = sim.add_dynamic_mod(unit, mod_of(SpellModType::BonusCritPercent, 5.0));
        sim.update_spell_mod_float_value(id, 6.0);
        assert_eq!(sim.spell(spell).bonus_crit_percent, 0.0);
        sim.activate_spell_mod(id);
        sim.update_spell_mod_float_value(id, 8.0);
        assert_eq!(sim.spell(spell).bonus_crit_percent, 8.0);
        assert_eq!(sim.spell_mod(id).float_value(), 8.0);
    }

    #[test]
    fn additive_damage_kinds_round_on_reset() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let spell = sim.register_spell(unit, mana_spell(810));
        sim.add_static_mod(unit, mod_of(SpellModType::DamageDoneFlat, 0.1));
        sim.add_static_mod(unit, mod_of(SpellModType::DirectDamageDoneFlat, 0.2));
        sim.spell_mut(spell).damage_multiplier_additive = 1.100_000_000_000_1;
        sim.spell_mut(spell).direct_damage_multiplier_additive = 0.300_000_000_000_4;
        for effect in sim.unit(unit).reset_effects.clone() {
            effect(&mut sim);
        }
        assert_eq!(sim.spell(spell).damage_multiplier_additive, 1.1);
        assert_eq!(sim.spell(spell).direct_damage_multiplier_additive, 0.3);
    }

    #[test]
    fn custom_mods_run_their_closures() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let spell = sim.register_spell(unit, mana_spell(820));
        let resets = Rc::new(Cell::new(0));
        let counted = Rc::clone(&resets);
        let id = sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                kind: SpellModType::Custom,
                float_value: 4.0,
                apply_custom: Some(Rc::new(|sim: &mut Sim, id, spell| {
                    let value = sim.spell_mod(id).float_value();
                    sim.spell_mut(spell).bonus_spell_damage += value;
                })),
                remove_custom: Some(Rc::new(|sim: &mut Sim, id, spell| {
                    let value = sim.spell_mod(id).float_value();
                    sim.spell_mut(spell).bonus_spell_damage -= value;
                })),
                reset_custom: Some(Rc::new(move |_: &mut Sim, _| {
                    counted.set(counted.get() + 1);
                })),
                ..Default::default()
            },
        );
        sim.activate_spell_mod(id);
        assert_eq!(sim.spell(spell).bonus_spell_damage, 4.0);
        sim.deactivate_spell_mod(id);
        assert_eq!(sim.spell(spell).bonus_spell_damage, 0.0);
        for effect in sim.unit(unit).reset_effects.clone() {
            effect(&mut sim);
        }
        assert_eq!(resets.get(), 1);
    }

    #[test]
    #[should_panic(expected = "ApplyCustom and RemoveCustom are mandatory")]
    fn custom_mods_need_both_closures() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        sim.add_dynamic_mod(unit, SpellModConfig::default());
    }

    #[test]
    #[should_panic(expected = "PseudoStats.SchoolBonusHitChance")]
    fn school_hit_mods_are_refused() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        sim.add_dynamic_mod(
            unit,
            SpellModConfig {
                school: school::FIRE,
                ..mod_of(SpellModType::BonusHitPercent, 1.0)
            },
        );
    }

    #[test]
    #[should_panic(expected = "not implemented")]
    fn kinds_without_an_implementation_are_refused() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        sim.add_dynamic_mod(unit, mod_of(SpellModType::AllowCastWhileChanneling, 0.0));
    }

    #[test]
    fn attached_mod_follows_its_aura() {
        let mut sim = Sim::new();
        let unit = new_unit(&mut sim);
        let spell = sim.register_spell(
            unit,
            SpellConfig {
                class_spell_mask: 2,
                ..mana_spell(830)
            },
        );
        let aura = buff(&mut sim, unit, "Buff", 0);
        let returned = sim.attach_spell_mod(
            aura,
            SpellModConfig {
                class_mask: 2,
                ..mod_of(SpellModType::DamageDonePct, 0.3)
            },
        );
        assert_eq!(returned, aura);
        assert_eq!(sim.spell(spell).damage_multiplier, 1.0);
        sim.activate(aura);
        assert_eq!(sim.spell(spell).damage_multiplier, 1.0 * (1.0 + 0.3));
        // A spell registered while the aura is up takes the mod at once.
        let late = sim.register_spell(
            unit,
            SpellConfig {
                class_spell_mask: 2,
                ..mana_spell(831)
            },
        );
        assert_eq!(sim.spell(late).damage_multiplier, 1.0 * (1.0 + 0.3));
        sim.deactivate(aura);
        assert_eq!(
            sim.spell(spell).damage_multiplier,
            (1.0 * (1.0 + 0.3)) / (1.0 + 0.3)
        );
        assert_eq!(sim.spell(late).damage_multiplier, 1.0);
    }

    #[test]
    fn pet_mods_follow_the_owners_config() {
        let mut sim = Sim::new();
        let owner = new_unit(&mut sim);
        let mut pet_unit = Unit::new(UnitType::Pet, "pet".to_string());
        pet_unit.mana_bar.base_mana = 100.0;
        let pet = sim.add_unit(pet_unit);
        sim.unit_mut(owner).pets.push(pet);
        let pet_spell = sim.register_spell(pet, mana_spell(840));
        sim.add_static_mod(
            owner,
            SpellModConfig {
                should_apply_to_pets: true,
                ..mod_of(SpellModType::DamageDonePct, 0.5)
            },
        );
        sim.add_static_mod(owner, mod_of(SpellModType::BonusCritPercent, 2.0));
        assert_eq!(sim.spell(pet_spell).damage_multiplier, 1.0 * (1.0 + 0.5));
        assert_eq!(sim.spell(pet_spell).bonus_crit_percent, 0.0);
    }
}
