//! The Go simulation objects preparation builds: units, auras, spells, timers and exclusive
//! effects, held in one arena and addressed by index, as Go addresses them by pointer.
//!
//! This mirrors sim/core's unit.go, aura.go, spell.go, cooldown.go and exclusive_effect.go for
//! the part of a simulation that construction, initialization, finalization and one reset
//! touch. Lifecycle callbacks a reset runs (OnInit, OnReset, OnGain, OnExpire,
//! OnStacksChange) are Rust closures; callbacks that only react to combat events are kept as
//! the names the prepared contract lists, since preparation never runs a fight.

use std::rc::Rc;

use crate::contracts::prepared_v2::ActionId;

use super::procs::DynamicProcManager;
use super::stats::{PseudoStats, SchoolIndex, Stat, StatDependencyManager, Stats, SCHOOL_LEN};

/// `time.Duration` in nanoseconds.
pub(crate) type Duration = i64;
/// Go's `NeverExpires`.
pub(crate) const NEVER_EXPIRES: Duration = i64::MAX;
pub(crate) const SECOND: Duration = 1_000_000_000;
pub(crate) const MILLISECOND: Duration = 1_000_000;

pub(crate) fn seconds(value: f64) -> Duration {
    (value * SECOND as f64) as Duration
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct UnitId(pub usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct AuraId(pub usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct SpellId(pub usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct TimerId(pub usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct EffectId(pub usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct CategoryId(pub usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct DotId(pub usize);

/// Go `Cooldown`: a shared timer and a duration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Cooldown {
    pub timer: Option<TimerId>,
    pub duration: Duration,
}

pub(crate) type AuraCallback = Rc<dyn Fn(&mut Sim, AuraId)>;
pub(crate) type StacksCallback = Rc<dyn Fn(&mut Sim, AuraId, i32, i32)>;
pub(crate) type EffectCallback = Rc<dyn Fn(&mut Sim, EffectId)>;
pub(crate) type ResetEffect = Rc<dyn Fn(&mut Sim)>;

/// Callbacks that react to combat events. Preparation records which ones an aura has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EventCallbacks {
    pub on_apply_effects: bool,
    pub on_cast_complete: bool,
    pub on_spell_hit_dealt: bool,
    pub on_spell_hit_taken: bool,
    pub on_periodic_damage_dealt: bool,
    pub on_periodic_damage_taken: bool,
    pub on_heal_dealt: bool,
    pub on_heal_taken: bool,
    pub on_periodic_heal_dealt: bool,
    pub on_periodic_heal_taken: bool,
    pub on_encounter_start: bool,
}

/// Go `CharacterBuildPhase`, a bit set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BuildPhase(pub u8);

impl BuildPhase {
    pub const NONE: BuildPhase = BuildPhase(0);
    pub const BASE: BuildPhase = BuildPhase(1);
    pub const GEAR: BuildPhase = BuildPhase(2);
    pub const TALENTS: BuildPhase = BuildPhase(4);
    pub const BUFFS: BuildPhase = BuildPhase(8);
    pub const CONSUMES: BuildPhase = BuildPhase(16);
    pub const ALL: BuildPhase = BuildPhase(31);

    pub fn matches(self, other: BuildPhase) -> bool {
        self.0 & other.0 != 0
    }
}

/// The configuration a unit registers an aura with: Go's `Aura` literal.
#[derive(Clone, Default)]
pub(crate) struct AuraConfig {
    pub label: String,
    pub tag: String,
    pub action_id: Option<ActionId>,
    pub action_id_for_proc: Option<ActionId>,
    pub icd: Option<Cooldown>,
    pub duration: Duration,
    pub max_stacks: i32,
    pub build_phase: BuildPhase,
    pub on_init: Option<AuraCallback>,
    pub on_reset: Option<AuraCallback>,
    pub on_done_iteration: bool,
    pub on_gain: Option<AuraCallback>,
    pub on_expire: Option<AuraCallback>,
    pub on_stacks_change: Option<StacksCallback>,
    pub events: EventCallbacks,
    /// The aura's dynamic proc manager, if it has one.
    pub dpm: Option<Rc<DynamicProcManager>>,
    /// The lines Go logs as the aura is gained and fades, as `NewTemporaryStatsAura`'s
    /// "Gained" and "Lost" lines.
    pub stat_logs: Option<(String, String)>,
}

#[derive(Clone)]
pub(crate) struct Aura {
    pub label: String,
    pub tag: String,
    pub action_id: Option<ActionId>,
    pub action_id_for_proc: Option<ActionId>,
    pub icd: Option<Cooldown>,
    pub duration: Duration,
    pub unit: UnitId,
    pub active: bool,
    pub stacks: i32,
    pub max_stacks: i32,
    pub exclusive_effects: Vec<EffectId>,
    pub build_phase: BuildPhase,
    pub on_init: Option<AuraCallback>,
    pub on_reset: Option<AuraCallback>,
    pub on_done_iteration: bool,
    pub on_gain: Option<AuraCallback>,
    pub on_expire: Option<AuraCallback>,
    pub on_stacks_change: Option<StacksCallback>,
    pub events: EventCallbacks,
    pub dpm: Option<Rc<DynamicProcManager>>,
    /// See [`AuraConfig::stat_logs`].
    pub stat_logs: Option<(String, String)>,
    /// The action ID the aura's metrics were registered with.
    pub metrics_id: Option<ActionId>,
    /// Activations this iteration: `metrics.Procs`.
    pub procs: u32,
    pub initialized: bool,
    pub start_time: Duration,
    pub expires: Duration,
    /// Go `DamageAbsorptionAura.ShieldStrength`, for an absorption aura. It lives here, not in
    /// a closure, so a copy of the simulation keeps its own.
    pub shield_strength: f64,
}

impl Aura {
    /// The callbacks the prepared contract lists, in Go's order.
    pub(crate) fn callback_names(&self) -> Vec<String> {
        let e = &self.events;
        [
            (self.on_init.is_some(), "on_init"),
            (self.on_reset.is_some(), "on_reset"),
            (self.on_done_iteration, "on_done_iteration"),
            (self.on_gain.is_some(), "on_gain"),
            (self.on_expire.is_some(), "on_expire"),
            (self.on_stacks_change.is_some(), "on_stacks_change"),
            (e.on_apply_effects, "on_apply_effects"),
            (e.on_cast_complete, "on_cast_complete"),
            (e.on_spell_hit_dealt, "on_spell_hit_dealt"),
            (e.on_spell_hit_taken, "on_spell_hit_taken"),
            (e.on_periodic_damage_dealt, "on_periodic_damage_dealt"),
            (e.on_periodic_damage_taken, "on_periodic_damage_taken"),
            (e.on_heal_dealt, "on_heal_dealt"),
            (e.on_heal_taken, "on_heal_taken"),
            (e.on_periodic_heal_dealt, "on_periodic_heal_dealt"),
            (e.on_periodic_heal_taken, "on_periodic_heal_taken"),
            (e.on_encounter_start, "on_encounter_start"),
        ]
        .into_iter()
        .filter(|(set, _)| *set)
        .map(|(_, name)| name.to_string())
        .collect()
    }
}

#[derive(Clone)]
pub(crate) struct ExclusiveEffect {
    pub aura: AuraId,
    pub priority: f64,
    pub on_gain: Option<EffectCallback>,
    pub on_expire: Option<EffectCallback>,
    pub category: CategoryId,
    pub is_enabled: bool,
}

#[derive(Clone)]
pub(crate) struct ExclusiveCategory {
    pub name: String,
    pub single_aura: bool,
    pub effects: Vec<EffectId>,
    pub active_effect: Option<EffectId>,
    pub unit: UnitId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnitType {
    Player,
    Enemy,
    Pet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PowerBar {
    Mana,
    Energy,
    Rage,
    Focus,
}

/// Go `manaBar` state read at preparation.
#[derive(Clone, Debug, Default)]
pub(crate) struct ManaBar {
    pub enabled: bool,
    pub base_mana: f64,
    pub current_mana: f64,
    pub mana_regen_multiplier: f64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct EnergyBar {
    pub enabled: bool,
    pub max_energy: f64,
    pub current_energy: f64,
    pub energy_regen_multiplier: f64,
    pub max_combo_points: i32,
    pub tick_duration: Duration,
    pub energy_per_tick: f64,
    /// Go `hasNoRegen`.
    pub has_no_regen: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RageBar {
    pub enabled: bool,
    /// Go `maxRage`: at least 100 once the class enables the bar.
    pub max_rage: f64,
    /// Go `startingRage`: clamped to the bar.
    pub starting_rage: f64,
    pub current_rage: f64,
    /// Go `offHandRageMultiplier`.
    pub off_hand_rage_multiplier: f64,
    /// The `BaseRageMultiplier` the bar's listener was built with.
    pub base_rage_multiplier: f64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FocusBar {
    pub enabled: bool,
    /// Go `focusBar.maxFocus`.
    pub max_focus: f64,
    pub current_focus: f64,
    pub focus_regen_per_tick: f64,
    pub focus_tick_duration: Duration,
}

/// One unit: Go's `Unit` with the fields preparation reads.
#[derive(Clone)]
pub(crate) struct Unit {
    pub unit_type: UnitType,
    pub index: i32,
    pub unit_index: i32,
    pub label: String,
    pub level: i32,
    pub mob_type: String,
    pub reaction_time: Duration,
    pub channel_clip_delay: Duration,
    pub start_distance_from_target: f64,
    pub distance_from_target: f64,
    pub enabled: bool,
    /// Go `Unit.ShapeshiftForm`: the client form the unit is in, 0 for none. Only a Shadow
    /// priest's Shadowform sets one at the pin.
    pub shapeshift_form: super::dbcenums::ShapeshiftForm,
    pub initial_stats: Stats,
    pub initial_stats_without_deps: Stats,
    pub initial_pseudo_stats: PseudoStats,
    pub initial_cast_speed: f64,
    pub initial_melee_swing_speed: f64,
    pub initial_ranged_swing_speed: f64,
    pub stats_without_deps: Stats,
    pub stats: Stats,
    pub sdm: StatDependencyManager,
    pub pseudo_stats: PseudoStats,
    pub auras: Vec<AuraId>,
    pub reset_effects: Vec<ResetEffect>,
    pub categories: Vec<CategoryId>,
    pub spellbook: Vec<SpellId>,
    pub timers: Vec<TimerId>,
    /// Go `unit.categoryTimers`: the timer a spell category shares, by category.
    pub category_timers: std::collections::BTreeMap<i32, TimerId>,
    pub cast_speed: f64,
    pub melee_attack_speed: f64,
    pub ranged_attack_speed: f64,
    pub melee_and_ranged_haste: f64,
    pub current_target: Option<UnitId>,
    pub default_target: Option<UnitId>,
    pub secondary_target: Option<UnitId>,
    pub current_power_bar: PowerBar,
    pub mana_bar: ManaBar,
    pub energy_bar: EnergyBar,
    pub rage_bar: RageBar,
    pub focus_bar: FocusBar,
    pub gcd: Option<TimerId>,
    pub rotation_timer: Option<TimerId>,
    /// How many dynamic damage taken modifiers the unit registered.
    pub dynamic_damage_taken_modifiers: usize,
    pub on_cast_speed_changed: usize,
    pub on_temporary_stats_changes: Vec<super::aura_helpers::TemporaryStatsListener>,
    /// The owner of a pet.
    pub owner: Option<UnitId>,
    pub pets: Vec<UnitId>,
    pub auto_attacks: super::attack::AutoAttacks,
    pub spell_registration_handlers: Vec<super::spell::SpellRegisteredHandler>,
    /// The character half of a player.
    pub character: Option<Box<super::character::Character>>,
    /// Go `HasHealthBar`.
    pub health_bar: bool,
    /// The auras of the unit's damage absorption shields, whose dynamic damage taken modifier
    /// acts only while its aura is active.
    pub absorption_auras: Vec<AuraId>,
    /// Go `healthBar.currentHealth`.
    pub current_health: f64,
    /// The pet half of a pet unit (pet.rs).
    pub pet: Option<Box<super::pet::Pet>>,
    /// Go `Unit.weaponSkill`: the skill above base attacks with the main hand, off hand and
    /// ranged weapon take (weapon_skill.rs).
    pub weapon_skill: [f64; super::weapon_skill::HANDS],
    /// Go `Unit.weaponSkillBonus`: skill from auras such as set bonuses, by skill line.
    pub weapon_skill_bonus: [f64; super::weapon_skill::NUM_WEAPON_SKILLS],
    /// Go `Unit.onWeaponSkillChange`, which a character sets as its effects apply: a skill
    /// bonus recomputes the hands.
    pub on_weapon_skill_change: bool,
}

impl Unit {
    pub(crate) fn new(unit_type: UnitType, label: String) -> Unit {
        Unit {
            unit_type,
            index: 0,
            unit_index: 0,
            label,
            level: 0,
            mob_type: "MobTypeUnknown".to_string(),
            reaction_time: 0,
            channel_clip_delay: 0,
            start_distance_from_target: 0.0,
            distance_from_target: 0.0,
            enabled: false,
            shapeshift_form: 0,
            initial_stats: Stats::default(),
            initial_stats_without_deps: Stats::default(),
            initial_pseudo_stats: PseudoStats::new(),
            initial_cast_speed: 0.0,
            initial_melee_swing_speed: 0.0,
            initial_ranged_swing_speed: 0.0,
            stats_without_deps: Stats::default(),
            stats: Stats::default(),
            sdm: StatDependencyManager::default(),
            pseudo_stats: PseudoStats::new(),
            auras: Vec::new(),
            reset_effects: Vec::new(),
            categories: Vec::new(),
            spellbook: Vec::new(),
            timers: Vec::new(),
            category_timers: std::collections::BTreeMap::new(),
            cast_speed: 0.0,
            melee_attack_speed: 0.0,
            ranged_attack_speed: 0.0,
            melee_and_ranged_haste: 0.0,
            current_target: None,
            default_target: None,
            secondary_target: None,
            current_power_bar: PowerBar::Mana,
            mana_bar: ManaBar::default(),
            energy_bar: EnergyBar::default(),
            rage_bar: RageBar::default(),
            focus_bar: FocusBar::default(),
            gcd: None,
            rotation_timer: None,
            dynamic_damage_taken_modifiers: 0,
            on_cast_speed_changed: 0,
            on_temporary_stats_changes: Vec::new(),
            owner: None,
            pets: Vec::new(),
            auto_attacks: super::attack::AutoAttacks::default(),
            spell_registration_handlers: Vec::new(),
            character: None,
            health_bar: false,
            absorption_auras: Vec::new(),
            current_health: 0.0,
            pet: None,
            weapon_skill: [0.0; super::weapon_skill::HANDS],
            weapon_skill_bonus: [0.0; super::weapon_skill::NUM_WEAPON_SKILLS],
            on_weapon_skill_change: false,
        }
    }
}

/// Go `EnvironmentState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EnvState {
    Created,
    Constructed,
    Initialized,
    Finalized,
}

/// The simulation preparation builds.
#[derive(Clone)]
pub(crate) struct Sim {
    pub state: EnvState,
    pub measuring_stats: bool,
    pub current_time: Duration,
    pub units: Vec<Unit>,
    pub auras: Vec<Aura>,
    pub effects: Vec<ExclusiveEffect>,
    pub categories: Vec<ExclusiveCategory>,
    pub spells: Vec<super::spell::Spell>,
    pub dots: Vec<super::spell::Dot>,
    /// Every spell mod the units built, by `ModId`.
    pub spell_mods: Vec<super::spell_mod::SpellMod>,
    /// Each timer's owning unit.
    pub timers: Vec<UnitId>,
    /// Go `env.AllUnits`: the targets, then the raid's units, by unit index.
    pub env_units: Vec<UnitId>,
    /// Go `AttackTable.DamageDoneByCasterExtraMultiplier`: per (attacker, defender), which
    /// handler slots are set. See aura_helpers.rs `attach_ddbc`.
    pub damage_done_by_caster: std::collections::BTreeMap<(UnitId, UnitId), Vec<bool>>,
    /// Go `Raid.nextPetIndex` before the first pet: five indexes for each active party.
    pub first_pet_index: i32,
    /// What effects' callbacks keep between calls (see [`StateSlot`]).
    pub state_slots: Vec<f64>,
    /// The times effects' callbacks keep between calls (see [`TimeSlot`]).
    pub time_slots: Vec<Duration>,
    /// The rules preparation follows where Rust departs from the Go reference.
    pub rules: Rules,
    /// Each item or enchant speed aura (Go `registerSpeedAura`) and the melee, ranged and cast
    /// speed multipliers it applies while up.
    pub speed_auras: Vec<(AuraId, [f64; 3])>,
}

/// Which rules preparation follows where Rust departs from the Go reference on purpose: Forever's
/// own, which production prepares with, or the reference's, which the Go goldens and exporter
/// comparisons check. The departures are listed in the README's Rust-only mechanics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rules {
    /// Forever as its developers state it.
    #[default]
    Forever,
    /// The Go reference at the pinned revision.
    GoReference,
}

/// A value an effect's callbacks keep between calls. Go keeps it in the closure; here it lives
/// in the simulation, so a copy of the simulation keeps its own instead of sharing it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StateSlot(usize);

/// A time an effect's callbacks keep between calls, as a [`StateSlot`] keeps a value.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TimeSlot(usize);

impl Sim {
    pub(crate) fn new() -> Sim {
        Sim {
            state: EnvState::Created,
            measuring_stats: false,
            current_time: 0,
            units: Vec::new(),
            auras: Vec::new(),
            effects: Vec::new(),
            categories: Vec::new(),
            spells: Vec::new(),
            dots: Vec::new(),
            spell_mods: Vec::new(),
            timers: Vec::new(),
            env_units: Vec::new(),
            damage_done_by_caster: std::collections::BTreeMap::new(),
            first_pet_index: 5,
            state_slots: Vec::new(),
            time_slots: Vec::new(),
            rules: Rules::Forever,
            speed_auras: Vec::new(),
        }
    }

    /// A new [`TimeSlot`] holding `time`.
    pub(crate) fn new_time_slot(&mut self, time: Duration) -> TimeSlot {
        self.time_slots.push(time);
        TimeSlot(self.time_slots.len() - 1)
    }

    pub(crate) fn set_time_state(&mut self, slot: TimeSlot, time: Duration) {
        self.time_slots[slot.0] = time;
    }

    /// A new [`StateSlot`] holding `value`.
    pub(crate) fn new_state_slot(&mut self, value: f64) -> StateSlot {
        self.state_slots.push(value);
        StateSlot(self.state_slots.len() - 1)
    }

    pub(crate) fn state(&self, slot: StateSlot) -> f64 {
        self.state_slots[slot.0]
    }

    pub(crate) fn set_state(&mut self, slot: StateSlot, value: f64) {
        self.state_slots[slot.0] = value;
    }

    /// A slot read as a flag: set when it is not zero.
    pub(crate) fn state_flag(&self, slot: StateSlot) -> bool {
        self.state(slot) != 0.0
    }

    pub(crate) fn set_state_flag(&mut self, slot: StateSlot, set: bool) {
        self.set_state(slot, if set { 1.0 } else { 0.0 });
    }

    /// Go `env.AllUnits`.
    pub(crate) fn all_units(&self) -> Vec<UnitId> {
        self.env_units.clone()
    }

    pub(crate) fn is_finalized(&self) -> bool {
        self.state == EnvState::Finalized
    }

    pub(crate) fn add_unit(&mut self, unit: Unit) -> UnitId {
        self.units.push(unit);
        UnitId(self.units.len() - 1)
    }

    pub(crate) fn unit(&self, id: UnitId) -> &Unit {
        &self.units[id.0]
    }

    pub(crate) fn unit_mut(&mut self, id: UnitId) -> &mut Unit {
        &mut self.units[id.0]
    }

    pub(crate) fn aura(&self, id: AuraId) -> &Aura {
        &self.auras[id.0]
    }

    pub(crate) fn aura_mut(&mut self, id: AuraId) -> &mut Aura {
        &mut self.auras[id.0]
    }

    // Timers.

    /// Go `unit.NewTimer`.
    pub(crate) fn new_timer(&mut self, unit: UnitId) -> TimerId {
        assert!(
            self.unit(unit).timers.len() <= 100,
            "Over 100 timers! There is probably one being registered every iteration."
        );
        self.timers.push(unit);
        let id = TimerId(self.timers.len() - 1);
        self.unit_mut(unit).timers.push(id);
        id
    }

    /// Go `unit.CategoryTimer` and `Character.GetOrInitSpellCategoryTimer`: the timer every
    /// spell of a client category shares, created on first use.
    pub(crate) fn category_timer(&mut self, unit: UnitId, category: i32) -> TimerId {
        if let Some(timer) = self.unit(unit).category_timers.get(&category) {
            return *timer;
        }
        let timer = self.new_timer(unit);
        self.unit_mut(unit).category_timers.insert(category, timer);
        timer
    }

    // Stats.

    pub(crate) fn stats(&self, unit: UnitId) -> Stats {
        self.unit(unit).stats
    }

    pub(crate) fn stat(&self, unit: UnitId, stat: Stat) -> f64 {
        self.unit(unit).stats[stat]
    }

    /// Go `unit.AddStats`: before finalization.
    pub(crate) fn add_stats(&mut self, unit: UnitId, stats: &Stats) {
        assert!(
            !self.is_finalized(),
            "Already finalized, use AddStatsDynamic instead!"
        );
        let unit = self.unit_mut(unit);
        unit.stats = unit.stats.add(stats);
    }

    /// Go `unit.AddStat`.
    pub(crate) fn add_stat(&mut self, unit: UnitId, stat: Stat, amount: f64) {
        assert!(
            !self.is_finalized(),
            "Already finalized, use AddStatDynamic instead!"
        );
        self.unit_mut(unit).stats[stat] += amount;
    }

    /// Go `unit.AddStatsDynamic`.
    pub(crate) fn add_stats_dynamic(&mut self, unit_id: UnitId, bonus: &Stats) {
        assert!(
            self.is_finalized() || self.measuring_stats,
            "Not finalized, use AddStats instead!"
        );
        let finalized_or_live = !self.measuring_stats || self.is_finalized();
        let unit = self.unit_mut(unit_id);
        unit.stats_without_deps.add_inplace(bonus);
        let applied = if finalized_or_live {
            let new_stats = unit
                .sdm
                .apply_stat_dependencies(unit.stats_without_deps)
                .floor_game_stats();
            let applied = new_stats.subtract(&unit.stats);
            unit.stats = new_stats;
            applied
        } else {
            unit.stats.add_inplace(bonus);
            *bonus
        };
        self.process_dynamic_bonus(unit_id, &applied);
    }

    pub(crate) fn add_stat_dynamic(&mut self, unit: UnitId, stat: Stat, amount: f64) {
        let mut bonus = Stats::default();
        bonus[stat] = amount;
        self.add_stats_dynamic(unit, &bonus);
    }

    /// Go `processDynamicBonus`, for what a reset can observe.
    pub(crate) fn process_dynamic_bonus(&mut self, unit_id: UnitId, bonus: &Stats) {
        if bonus[Stat::Mana] != 0.0 && self.unit(unit_id).mana_bar.enabled {
            let max = self.max_mana(unit_id);
            let unit = self.unit_mut(unit_id);
            if unit.mana_bar.current_mana > max {
                unit.mana_bar.current_mana = max;
            }
        }
        if bonus[Stat::MeleeHasteRating] != 0.0 {
            self.update_attack_speed(unit_id);
            self.update_melee_and_ranged_haste(unit_id);
        }
        if bonus[Stat::SpellHasteRating] != 0.0 {
            self.update_cast_speed(unit_id);
        }
        if bonus[Stat::ReducedCritTakenPercent] != 0.0 {
            self.update_reduced_crit_taken_percent(unit_id);
        }
    }

    fn reapply_dependencies(&mut self, unit_id: UnitId) {
        let unit = self.unit_mut(unit_id);
        let old = unit.stats;
        unit.stats = unit
            .sdm
            .apply_stat_dependencies(unit.stats_without_deps)
            .floor_game_stats();
        let change = unit.stats.subtract(&old);
        self.process_dynamic_bonus(unit_id, &change);
    }

    /// Go `unit.EnableDynamicStatDep`.
    pub(crate) fn enable_dynamic_stat_dep(&mut self, unit: UnitId, dep: super::stats::DepId) {
        if self.unit_mut(unit).sdm.enable_dynamic_stat_dep(dep) {
            self.reapply_dependencies(unit);
        }
    }

    /// Go `unit.DisableDynamicStatDep`.
    pub(crate) fn disable_dynamic_stat_dep(&mut self, unit: UnitId, dep: super::stats::DepId) {
        if self.unit_mut(unit).sdm.disable_dynamic_stat_dep(dep) {
            self.reapply_dependencies(unit);
        }
    }

    /// Go `unit.UpdateDynamicStatDep`.
    pub(crate) fn update_dynamic_stat_dep(
        &mut self,
        unit: UnitId,
        dep: super::stats::DepId,
        amount: f64,
    ) {
        self.unit_mut(unit).sdm.update_value(dep, amount);
        if self.is_finalized() {
            self.reapply_dependencies(unit);
        }
    }

    /// Go `unit.EnableBuildPhaseStatDep`.
    pub(crate) fn enable_build_phase_stat_dep(&mut self, unit: UnitId, dep: super::stats::DepId) {
        if self.measuring_stats && !self.is_finalized() {
            self.unit_mut(unit).sdm.enable_dynamic_stat_dep(dep);
        } else {
            self.enable_dynamic_stat_dep(unit, dep);
        }
    }

    /// Go `unit.DisableBuildPhaseStatDep`.
    pub(crate) fn disable_build_phase_stat_dep(&mut self, unit: UnitId, dep: super::stats::DepId) {
        if self.measuring_stats && !self.is_finalized() {
            self.unit_mut(unit).sdm.disable_dynamic_stat_dep(dep);
        } else {
            self.disable_dynamic_stat_dep(unit, dep);
        }
    }

    // Speeds.

    pub(crate) fn total_spell_haste_multiplier(&self, unit: UnitId) -> f64 {
        let unit = self.unit(unit);
        unit.pseudo_stats.cast_speed_multiplier
            * (1.0
                + unit.stats[Stat::SpellHasteRating]
                    / (SPELL_HASTE_RATING_PER_HASTE_PERCENT * 100.0))
    }

    pub(crate) fn update_cast_speed(&mut self, unit: UnitId) {
        let speed = 1.0 / self.total_spell_haste_multiplier(unit);
        self.unit_mut(unit).cast_speed = speed;
    }

    pub(crate) fn multiply_cast_speed(&mut self, unit: UnitId, amount: f64) {
        self.unit_mut(unit).pseudo_stats.cast_speed_multiplier *= amount;
        self.update_cast_speed(unit);
    }

    pub(crate) fn total_melee_haste_multiplier(&self, unit: UnitId) -> f64 {
        let unit = self.unit(unit);
        unit.pseudo_stats.attack_speed_multiplier
            * unit.pseudo_stats.melee_speed_multiplier
            * (1.0
                + (unit.stats[Stat::MeleeHasteRating]
                    / (PHYSICAL_HASTE_RATING_PER_HASTE_PERCENT * 100.0)))
    }

    pub(crate) fn total_real_haste_multiplier(&self, unit: UnitId) -> f64 {
        let unit = self.unit(unit);
        unit.pseudo_stats.attack_speed_multiplier
            * (1.0
                + (unit.stats[Stat::MeleeHasteRating]
                    / (PHYSICAL_HASTE_RATING_PER_HASTE_PERCENT * 100.0)))
    }

    pub(crate) fn total_ranged_haste_multiplier(&self, unit: UnitId) -> f64 {
        let unit = self.unit(unit);
        unit.pseudo_stats.attack_speed_multiplier
            * unit.pseudo_stats.ranged_speed_multiplier
            * (1.0
                + (unit.stats[Stat::MeleeHasteRating]
                    / (PHYSICAL_HASTE_RATING_PER_HASTE_PERCENT * 100.0)))
    }

    pub(crate) fn update_attack_speed(&mut self, unit: UnitId) {
        let melee = self.total_melee_haste_multiplier(unit);
        let ranged = self.total_ranged_haste_multiplier(unit);
        let unit = self.unit_mut(unit);
        unit.melee_attack_speed = melee;
        unit.ranged_attack_speed = ranged;
    }

    pub(crate) fn update_melee_and_ranged_haste(&mut self, unit: UnitId) {
        let haste = self.total_real_haste_multiplier(unit);
        self.unit_mut(unit).melee_and_ranged_haste = haste;
    }

    pub(crate) fn multiply_attack_speed(&mut self, unit: UnitId, amount: f64) {
        self.unit_mut(unit).pseudo_stats.attack_speed_multiplier *= amount;
        self.update_attack_speed(unit);
        self.update_melee_and_ranged_haste(unit);
    }

    pub(crate) fn multiply_melee_speed(&mut self, unit: UnitId, amount: f64) {
        self.unit_mut(unit).pseudo_stats.melee_speed_multiplier *= amount;
        let melee = self.total_melee_haste_multiplier(unit);
        self.unit_mut(unit).melee_attack_speed = melee;
    }

    pub(crate) fn multiply_ranged_speed(&mut self, unit: UnitId, amount: f64) {
        self.unit_mut(unit).pseudo_stats.ranged_speed_multiplier *= amount;
        let ranged = self.total_ranged_haste_multiplier(unit);
        self.unit_mut(unit).ranged_attack_speed = ranged;
    }

    /// Go `updateReducedCritTakenPercent`.
    pub(crate) fn update_reduced_crit_taken_percent(&mut self, unit: UnitId) {
        let unit = self.unit_mut(unit);
        unit.pseudo_stats.reduced_crit_taken_percent =
            unit.pseudo_stats.base_reduced_crit_taken_percent
                + unit.stats[Stat::ReducedCritTakenPercent] / 100.0;
    }

    // Mana.

    pub(crate) fn max_mana(&self, unit: UnitId) -> f64 {
        self.unit(unit).stats[Stat::Mana]
    }

    // Auras.

    /// Go `unit.RegisterAura`.
    pub(crate) fn register_aura(&mut self, unit: UnitId, config: AuraConfig) -> AuraId {
        assert!(
            !self.is_finalized(),
            "Tried to add new aura in a finalized environment!"
        );
        assert!(!config.label.is_empty(), "Aura label is required!");
        assert!(
            self.get_aura(unit, &config.label).is_none(),
            "Aura {} already registered!",
            config.label
        );
        let aura = Aura {
            label: config.label,
            tag: config.tag,
            metrics_id: config.action_id.clone(),
            action_id: config.action_id,
            action_id_for_proc: config.action_id_for_proc,
            icd: config.icd,
            duration: config.duration,
            unit,
            active: false,
            stacks: 0,
            max_stacks: config.max_stacks,
            exclusive_effects: Vec::new(),
            build_phase: config.build_phase,
            on_init: config.on_init,
            on_reset: config.on_reset,
            on_done_iteration: config.on_done_iteration,
            on_gain: config.on_gain,
            on_expire: config.on_expire,
            on_stacks_change: config.on_stacks_change,
            events: config.events,
            dpm: config.dpm,
            stat_logs: config.stat_logs,
            procs: 0,
            initialized: false,
            start_time: 0,
            expires: 0,
            shield_strength: 0.0,
        };
        self.auras.push(aura);
        let id = AuraId(self.auras.len() - 1);
        self.unit_mut(unit).auras.push(id);
        id
    }

    /// Go `unit.GetOrRegisterAura`.
    pub(crate) fn get_or_register_aura(&mut self, unit: UnitId, config: AuraConfig) -> AuraId {
        match self.get_aura(unit, &config.label) {
            None => self.register_aura(unit, config),
            Some(id) => {
                let aura = self.aura_mut(id);
                aura.icd = config.icd;
                aura.events.on_cast_complete = config.events.on_cast_complete;
                aura.events.on_spell_hit_dealt = config.events.on_spell_hit_dealt;
                aura.events.on_spell_hit_taken = config.events.on_spell_hit_taken;
                aura.events.on_periodic_damage_dealt = config.events.on_periodic_damage_dealt;
                aura.events.on_periodic_damage_taken = config.events.on_periodic_damage_taken;
                aura.events.on_heal_dealt = config.events.on_heal_dealt;
                aura.events.on_heal_taken = config.events.on_heal_taken;
                aura.events.on_periodic_heal_dealt = config.events.on_periodic_heal_dealt;
                aura.events.on_periodic_heal_taken = config.events.on_periodic_heal_taken;
                aura.events.on_encounter_start = config.events.on_encounter_start;
                id
            }
        }
    }

    pub(crate) fn get_aura(&self, unit: UnitId, label: &str) -> Option<AuraId> {
        self.unit(unit)
            .auras
            .iter()
            .copied()
            .find(|id| self.aura(*id).label == label)
    }

    pub(crate) fn has_active_aura(&self, unit: UnitId, label: &str) -> bool {
        self.get_aura(unit, label)
            .is_some_and(|id| self.aura(id).active)
    }

    pub(crate) fn auras_with_tag(&self, unit: UnitId, tag: &str) -> Vec<AuraId> {
        self.unit(unit)
            .auras
            .iter()
            .copied()
            .filter(|id| self.aura(*id).tag == tag)
            .collect()
    }

    /// Go `ApplyOnInit`.
    pub(crate) fn apply_on_init(&mut self, aura: AuraId, callback: AuraCallback) {
        let aura = self.aura_mut(aura);
        aura.on_init = Some(chain(aura.on_init.take(), callback));
    }

    /// Go `ApplyOnGain`.
    pub(crate) fn apply_on_gain(&mut self, aura: AuraId, callback: AuraCallback) {
        let aura = self.aura_mut(aura);
        aura.on_gain = Some(chain(aura.on_gain.take(), callback));
    }

    /// Go `ApplyOnExpire`.
    pub(crate) fn apply_on_expire(&mut self, aura: AuraId, callback: AuraCallback) {
        let aura = self.aura_mut(aura);
        aura.on_expire = Some(chain(aura.on_expire.take(), callback));
    }

    /// Go `ApplyOnReset`.
    pub(crate) fn apply_on_reset(&mut self, aura: AuraId, callback: AuraCallback) {
        let aura = self.aura_mut(aura);
        aura.on_reset = Some(chain(aura.on_reset.take(), callback));
    }

    /// Go `ApplyOnStacksChange`.
    pub(crate) fn apply_on_stacks_change(&mut self, aura: AuraId, callback: StacksCallback) {
        let aura = self.aura_mut(aura);
        aura.on_stacks_change = Some(match aura.on_stacks_change.take() {
            None => callback,
            Some(old) => Rc::new(move |sim: &mut Sim, id, from, to| {
                old(sim, id, from, to);
                callback(sim, id, from, to);
            }),
        });
    }

    /// Go `ApplyOnEncounterStart`: preparation only records it.
    pub(crate) fn apply_on_encounter_start(&mut self, aura: AuraId) {
        self.aura_mut(aura).events.on_encounter_start = true;
    }

    /// Go `AttachDependentAura`.
    pub(crate) fn attach_dependent_aura(&mut self, aura: AuraId, sibling: AuraId) {
        self.apply_on_gain(aura, Rc::new(move |sim: &mut Sim, _| sim.activate(sibling)));
        self.apply_on_expire(
            aura,
            Rc::new(move |sim: &mut Sim, _| sim.deactivate(sibling)),
        );
        self.apply_on_stacks_change(
            aura,
            Rc::new(move |sim: &mut Sim, _, _, new| {
                if sim.aura(sibling).max_stacks == 0 {
                    return;
                }
                sim.set_stacks(sibling, new);
            }),
        );
    }

    /// Go `RegisterResetEffect`.
    pub(crate) fn register_reset_effect(&mut self, unit: UnitId, effect: ResetEffect) {
        self.unit_mut(unit).reset_effects.push(effect);
    }

    /// Go `aura.init`.
    pub(crate) fn init_aura(&mut self, id: AuraId) {
        if self.aura(id).initialized {
            return;
        }
        self.aura_mut(id).initialized = true;
        if let Some(on_init) = self.aura(id).on_init.clone() {
            on_init(self, id);
        }
    }

    /// Go `aura.reset`.
    pub(crate) fn reset_aura(&mut self, id: AuraId) {
        self.init_aura(id);
        assert!(
            !self.aura(id).active,
            "Active aura during reset: {}",
            self.aura(id).label
        );
        self.aura_mut(id).procs = 0;
        if let Some(on_reset) = self.aura(id).on_reset.clone() {
            on_reset(self, id);
        }
    }

    /// Go `aura.Activate`.
    pub(crate) fn activate(&mut self, id: AuraId) {
        self.aura_mut(id).procs += 1;
        if self.aura(id).active {
            self.refresh(id);
            return;
        }
        assert!(self.aura(id).duration != 0, "Aura with 0 duration");
        let effects = self.aura(id).exclusive_effects.clone();
        for (i, effect) in effects.iter().enumerate() {
            if !self.activate_effect(*effect) {
                for earlier in &effects[..i] {
                    self.deactivate_effect(*earlier);
                }
                return;
            }
        }
        let now = self.current_time;
        let aura = self.aura_mut(id);
        aura.active = true;
        aura.start_time = now;
        self.refresh(id);
        if let Some(on_gain) = self.aura(id).on_gain.clone() {
            on_gain(self, id);
        }
    }

    /// Go `aura.Refresh`.
    pub(crate) fn refresh(&mut self, id: AuraId) {
        let now = self.current_time;
        let aura = self.aura_mut(id);
        aura.expires = if aura.duration == NEVER_EXPIRES {
            NEVER_EXPIRES
        } else {
            now + aura.duration
        };
    }

    /// Go `aura.Deactivate`.
    pub(crate) fn deactivate(&mut self, id: AuraId) {
        if !self.aura(id).active {
            return;
        }
        let aura = self.aura_mut(id);
        aura.active = false;
        aura.expires = 0;
        if self.aura(id).stacks != 0 {
            self.set_stacks(id, 0);
        }
        for effect in self.aura(id).exclusive_effects.clone() {
            self.deactivate_effect(effect);
        }
        if let Some(on_expire) = self.aura(id).on_expire.clone() {
            on_expire(self, id);
        }
    }

    /// Go `aura.SetStacks`.
    pub(crate) fn set_stacks(&mut self, id: AuraId, new_stacks: i32) {
        let aura = self.aura(id);
        assert!(
            aura.active || new_stacks == 0,
            "Trying to set non-zero stacks on inactive aura!"
        );
        assert!(new_stacks >= 0, "SetStacks newStacks cannot be negative");
        assert!(
            aura.max_stacks != 0,
            "MaxStacks required to set Aura stacks: {}",
            aura.label
        );
        let old = aura.stacks;
        let new_stacks = new_stacks.min(aura.max_stacks);
        if old == new_stacks {
            return;
        }
        self.aura_mut(id).stacks = new_stacks;
        if let Some(callback) = self.aura(id).on_stacks_change.clone() {
            callback(self, id, old, new_stacks);
        }
        if self.aura(id).stacks == 0 {
            self.deactivate(id);
        }
    }

    pub(crate) fn add_stack(&mut self, id: AuraId) {
        let stacks = self.aura(id).stacks + 1;
        self.set_stacks(id, stacks);
    }

    /// Go `MakePermanent`: an aura with no expiry that a reset activates. Its reset callback
    /// restores the duration, runs the aura's earlier reset callback, then activates it.
    pub(crate) fn make_permanent(&mut self, id: AuraId) -> AuraId {
        let aura = self.aura_mut(id);
        aura.duration = NEVER_EXPIRES;
        let old = aura.on_reset.take();
        aura.on_reset = Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
            sim.aura_mut(aura).duration = NEVER_EXPIRES;
            if let Some(old) = &old {
                old(sim, aura);
            }
            sim.activate(aura);
        }));
        id
    }

    // Exclusive effects.

    fn exclusive_category(&mut self, unit: UnitId, name: &str) -> CategoryId {
        if let Some(found) = self
            .unit(unit)
            .categories
            .iter()
            .copied()
            .find(|id| self.categories[id.0].name == name)
        {
            return found;
        }
        self.categories.push(ExclusiveCategory {
            name: name.to_string(),
            single_aura: false,
            effects: Vec::new(),
            active_effect: None,
            unit,
        });
        let id = CategoryId(self.categories.len() - 1);
        self.unit_mut(unit).categories.push(id);
        id
    }

    /// Go `aura.NewExclusiveEffect`.
    pub(crate) fn new_exclusive_effect(
        &mut self,
        aura: AuraId,
        category_name: &str,
        single_aura: bool,
        priority: f64,
        on_gain: Option<EffectCallback>,
        on_expire: Option<EffectCallback>,
    ) -> EffectId {
        let unit = self.aura(aura).unit;
        let category = self.exclusive_category(unit, category_name);
        self.categories[category.0].single_aura = single_aura;
        if let Some(existing) = self.categories[category.0]
            .effects
            .iter()
            .copied()
            .find(|effect| self.effects[effect.0].aura == aura)
        {
            return existing;
        }
        self.effects.push(ExclusiveEffect {
            aura,
            priority,
            on_gain,
            on_expire,
            category,
            is_enabled: false,
        });
        let id = EffectId(self.effects.len() - 1);
        self.categories[category.0].effects.push(id);
        self.aura_mut(aura).exclusive_effects.push(id);
        id
    }

    fn remaining_duration(&self, aura: AuraId) -> Duration {
        let aura = self.aura(aura);
        if !aura.active {
            0
        } else if aura.expires == NEVER_EXPIRES {
            NEVER_EXPIRES
        } else {
            aura.expires - self.current_time
        }
    }

    fn outlasts(&self, effect: EffectId, newcomer: EffectId) -> bool {
        self.remaining_duration(self.effects[effect.0].aura)
            > self.aura(self.effects[newcomer.0].aura).duration
    }

    fn keeps_tie(&self, effect: EffectId, newcomer: EffectId) -> bool {
        let spell = |id: EffectId| {
            self.aura(self.effects[id.0].aura)
                .action_id
                .as_ref()
                .map_or(0, |action| action.spell_id)
        };
        spell(effect) != spell(newcomer) && self.outlasts(effect, newcomer)
    }

    fn set_active_effect(&mut self, category: CategoryId, effect: Option<EffectId>) {
        let current = self.categories[category.0].active_effect;
        if current == effect {
            return;
        }
        if let Some(current) = current {
            if let Some(on_expire) = self.effects[current.0].on_expire.clone() {
                on_expire(self, current);
            }
        }
        self.categories[category.0].active_effect = effect;
        if let Some(effect) = effect {
            if let Some(on_gain) = self.effects[effect.0].on_gain.clone() {
                on_gain(self, effect);
            }
        }
    }

    /// Go `ExclusiveEffect.Activate`: whether the effect is active.
    fn activate_effect(&mut self, effect: EffectId) -> bool {
        if self.effects[effect.0].is_enabled {
            return true;
        }
        let category = self.effects[effect.0].category;
        let priority = self.effects[effect.0].priority;
        let single = self.categories[category.0].single_aura;
        let active = self.categories[category.0].active_effect;
        if let Some(active) = active {
            let active_priority = self.effects[active.0].priority;
            if single
                && active != effect
                && (active_priority > priority
                    || (priority == active_priority && self.outlasts(active, effect)))
            {
                return false;
            }
        }
        self.effects[effect.0].is_enabled = true;
        match active {
            None => self.set_active_effect(category, Some(effect)),
            Some(active) => {
                let active_priority = self.effects[active.0].priority;
                if priority > active_priority
                    || (priority == active_priority && !self.keeps_tie(active, effect))
                {
                    if single && active != effect {
                        let displaced = self.effects[active.0].aura;
                        self.deactivate(displaced);
                    }
                    self.set_active_effect(category, Some(effect));
                }
            }
        }
        true
    }

    fn highest_priority_enabled(&self, category: CategoryId) -> Option<EffectId> {
        let mut best: Option<EffectId> = None;
        for effect in &self.categories[category.0].effects {
            let candidate = &self.effects[effect.0];
            if candidate.is_enabled
                && best.is_none_or(|best| candidate.priority > self.effects[best.0].priority)
            {
                best = Some(*effect);
            }
        }
        best
    }

    /// Go `ExclusiveEffect.Deactivate`.
    fn deactivate_effect(&mut self, effect: EffectId) {
        if !self.effects[effect.0].is_enabled {
            return;
        }
        self.effects[effect.0].is_enabled = false;
        let category = self.effects[effect.0].category;
        if self.categories[category.0].active_effect == Some(effect) {
            let next = self.highest_priority_enabled(category);
            self.set_active_effect(category, next);
        }
    }

    /// Go `ExclusiveEffect.SetPriority`.
    pub(crate) fn set_effect_priority(&mut self, effect: EffectId, priority: f64) {
        if !self.effects[effect.0].is_enabled {
            self.effects[effect.0].priority = priority;
            return;
        }
        let category = self.effects[effect.0].category;
        let current = self.categories[category.0].active_effect;
        let old = self.effects[effect.0].priority;
        self.effects[effect.0].priority = priority;
        let next = self.highest_priority_enabled(category);
        self.effects[effect.0].priority = old;
        if current == Some(effect) && next == Some(effect) {
            if let Some(on_expire) = self.effects[effect.0].on_expire.clone() {
                on_expire(self, effect);
            }
            self.effects[effect.0].priority = priority;
            if let Some(on_gain) = self.effects[effect.0].on_gain.clone() {
                on_gain(self, effect);
            }
        } else if current != Some(effect) && next != Some(effect) {
            self.effects[effect.0].priority = priority;
        } else if current == Some(effect) {
            self.set_active_effect(category, next);
            self.effects[effect.0].priority = priority;
        } else {
            self.effects[effect.0].priority = priority;
            self.set_active_effect(category, next);
        }
    }

    /// Go `ExclusiveEffect.IsActive`.
    pub(crate) fn effect_is_active(&self, effect: EffectId) -> bool {
        let category = self.effects[effect.0].category;
        self.categories[category.0].active_effect == Some(effect)
    }

    /// Go `ExclusiveCategory.GetActiveAura`.
    pub(crate) fn category_active_aura(&self, category: CategoryId) -> Option<AuraId> {
        self.categories[category.0]
            .active_effect
            .map(|effect| self.effects[effect.0].aura)
    }

    /// The school a spell's schools index, as Go's RegisterSpell picks it.
    pub(crate) fn school_index(school: u8) -> SchoolIndex {
        const ORDER: [(u8, SchoolIndex); 7] = [
            (1 << 0, SchoolIndex::Physical),
            (1 << 1, SchoolIndex::Holy),
            (1 << 2, SchoolIndex::Fire),
            (1 << 3, SchoolIndex::Nature),
            (1 << 4, SchoolIndex::Frost),
            (1 << 5, SchoolIndex::Shadow),
            (1 << 6, SchoolIndex::Arcane),
        ];
        // Go checks Physical, Arcane, Fire, Frost, Holy, Nature, Shadow in that order.
        for wanted in [
            SchoolIndex::Physical,
            SchoolIndex::Arcane,
            SchoolIndex::Fire,
            SchoolIndex::Frost,
            SchoolIndex::Holy,
            SchoolIndex::Nature,
            SchoolIndex::Shadow,
        ] {
            let bit = ORDER.iter().find(|(_, index)| *index == wanted).unwrap().0;
            if school & bit != 0 {
                return wanted;
            }
        }
        SchoolIndex::None
    }
}

/// Go's rating conversion constants (base_stats_auto_gen.go).
pub(crate) const SPELL_HASTE_RATING_PER_HASTE_PERCENT: f64 = 10.0;
pub(crate) const PHYSICAL_HASTE_RATING_PER_HASTE_PERCENT: f64 = 10.0;

fn chain(old: Option<AuraCallback>, new: AuraCallback) -> AuraCallback {
    match old {
        None => new,
        Some(old) => Rc::new(move |sim: &mut Sim, id| {
            old(sim, id);
            new(sim, id);
        }),
    }
}

/// Schools as Go's per-school arrays index them.
pub(crate) fn school_array_index(index: SchoolIndex) -> usize {
    index as usize
}

const _: () = assert!(SCHOOL_LEN == 8);
