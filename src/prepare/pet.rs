//! Go sim/core/pet.go and focus.go: the pets a player owns, built at construction, enabled by
//! a reset, and described by the exporter (tools/oracle-v2/pets.go).
//!
//! A pet is a [`Unit`](super::sim::Unit) of type [`UnitType::Pet`] whose `pet` field holds what
//! Go's `Pet` adds to its `Character`. A class builds one with [`Sim::new_pet`], registers its
//! auras and spells like a player's, and adds it to its owner with [`Sim::add_pet`]; the
//! environment then initializes, finalizes and resets it with the owner (see the `Environment`
//! methods below) and calls the class's [`PrepAgent`](super::agent::PrepAgent) pet hooks.
//!
//! What only matters while a fight runs is left out: the pending actions that deliver a
//! dynamic pet its owner's later stat changes, the heartbeat that times them, the timeout a
//! guardian is summoned with, and the lists of an owner's dynamic pets. The exporter refuses
//! the pets whose preparation they would change (a pet that inherits speed or resource regen).

use std::rc::Rc;

use serde_json::{json, Map, Value};

use super::env::{reset_unit, Environment};
use super::export::{
    export_auras, export_melee_of, export_pseudo, export_spell_of, metrics_actions, stat_values,
    TimerNames,
};
use super::sim::{Duration, PowerBar, Sim, UnitId, UnitType};
use super::stats::{Stat, Stats};

/// Go `PetStatInheritance`: the stats a pet inherits from the owner stats it is given.
/// Go `PetStatInheritance`. It takes the simulation, since a class's closure may read and
/// keep the owner's state, as the hunter's crit offset does.
pub(crate) type PetStatInheritance = Rc<dyn Fn(&mut Sim, &Stats) -> Stats>;
/// Go `OnPetEnable` and `OnPetDisable`, with the pet they ran for.
pub(crate) type PetCallback = Rc<dyn Fn(&mut Sim, UnitId)>;

/// Go `PetConfig`.
#[derive(Clone)]
pub(crate) struct PetConfig {
    pub name: String,
    pub owner: UnitId,
    pub base_stats: Stats,
    pub stat_inheritance: PetStatInheritance,
    pub enabled_on_start: bool,
    pub is_guardian: bool,
    pub is_dynamic: bool,
    pub has_dynamic_melee_speed_inheritance: bool,
    pub has_dynamic_cast_speed_inheritance: bool,
    pub has_resource_regen_inheritance: bool,
    pub starts_at_owner_distance: bool,
}

/// What Go's `Pet` adds to its `Character`.
#[derive(Clone)]
pub(crate) struct Pet {
    pub name: String,
    pub base_stats: Stats,
    pub is_guardian: bool,
    pub enabled_on_start: bool,
    pub on_pet_enable: Option<PetCallback>,
    pub on_pet_disable: Option<PetCallback>,
    /// `PetAgent.Initialize` of a pet whose agent is not the class's: a pet a gear pet
    /// constructor adds (Go `RegisterGearPetConstructor`) is its own agent. The class's hooks
    /// skip it.
    pub on_initialize: Option<PetCallback>,
    /// Go `statInheritance`.
    pub stat_inheritance: PetStatInheritance,
    /// Whether Go's `dynamicStatInheritance` is set: it is while a dynamic pet is enabled.
    pub dynamic_stat_inheritance: bool,
    pub inherited_stats: Stats,
    pub is_dynamic: bool,
    pub has_dynamic_melee_speed_inheritance: bool,
    pub inherited_melee_speed_multiplier: f64,
    pub dynamic_melee_speed_enabled: bool,
    pub has_dynamic_cast_speed_inheritance: bool,
    pub inherited_cast_speed_multiplier: f64,
    pub dynamic_cast_speed_enabled: bool,
    pub has_resource_regen_inheritance: bool,
    pub is_reset: bool,
    pub start_attack_delay: Duration,
    /// Go `Character.spiritRegenBase` and `spiritRegenPerSpirit`, for a pet with a mana bar.
    pub spirit_regen_base: f64,
    pub spirit_regen_per_spirit: f64,
    /// Whether a reset has disabled the pet: Go logs "Pet dismissed" then.
    pub dismissed_at_reset: bool,
}

impl Sim {
    /// Go `NewPet`.
    pub(crate) fn new_pet(&mut self, config: PetConfig) -> UnitId {
        let owner = config.owner;
        let distance = if config.starts_at_owner_distance {
            self.unit(owner).start_distance_from_target
        } else {
            super::character::constants::MAX_MELEE_RANGE
        };
        // Go `Raid.getNextPetIndex`: pets number from the first index past the parties.
        let pet_index = self.first_pet_index
            + self
                .units
                .iter()
                .filter(|unit| unit.unit_type == UnitType::Pet)
                .count() as i32;
        let label = format!("{} - {}", self.unit(owner).label, config.name);
        let mut unit = super::sim::Unit::new(UnitType::Pet, label);
        unit.index = pet_index;
        // A native world pet must fight at the owner's actual prepared level.
        // The Go raid reference's fixed level 60 is only valid for L60 actors.
        unit.level = self.unit(owner).level;
        unit.reaction_time = self.unit(owner).reaction_time;
        unit.start_distance_from_target = distance;
        unit.distance_from_target = distance;
        unit.owner = Some(owner);
        unit.pet = Some(Box::new(Pet {
            name: config.name,
            base_stats: config.base_stats,
            is_guardian: config.is_guardian,
            enabled_on_start: config.enabled_on_start,
            on_pet_enable: None,
            on_pet_disable: None,
            on_initialize: None,
            stat_inheritance: config.stat_inheritance,
            dynamic_stat_inheritance: false,
            inherited_stats: Stats::default(),
            is_dynamic: config.is_dynamic,
            has_dynamic_melee_speed_inheritance: config.has_dynamic_melee_speed_inheritance
                && config.is_dynamic,
            inherited_melee_speed_multiplier: 1.0,
            dynamic_melee_speed_enabled: false,
            has_dynamic_cast_speed_inheritance: config.has_dynamic_cast_speed_inheritance
                && config.is_dynamic,
            inherited_cast_speed_multiplier: 1.0,
            dynamic_cast_speed_enabled: false,
            has_resource_regen_inheritance: config.has_resource_regen_inheritance,
            is_reset: false,
            start_attack_delay: 0,
            spirit_regen_base: 0.0,
            spirit_regen_per_spirit: 0.0,
            dismissed_at_reset: false,
        }));
        let owner_in_front = self.unit(owner).pseudo_stats.in_front_of_target;
        let pet = self.add_unit(unit);
        let gcd = self.new_timer(pet);
        let rotation = self.new_timer(pet);
        self.unit_mut(pet).gcd = Some(gcd);
        self.unit_mut(pet).rotation_timer = Some(rotation);
        let base = self.pet_data(pet).base_stats;
        self.add_stats(pet, &base);
        super::character::add_character_universal_stat_dependencies(self, pet);
        self.unit_mut(pet).pseudo_stats.in_front_of_target = owner_in_front;
        pet
    }

    /// Go `Character.AddPet`.
    pub(crate) fn add_pet(&mut self, owner: UnitId, pet: UnitId) {
        assert!(
            self.state == super::sim::EnvState::Created,
            "Pets must be added during construction!"
        );
        self.unit_mut(owner).pets.push(pet);
    }

    pub(crate) fn pet_data(&self, pet: UnitId) -> &Pet {
        self.unit(pet).pet.as_deref().expect("a pet unit")
    }

    pub(crate) fn pet_data_mut(&mut self, pet: UnitId) -> &mut Pet {
        self.unit_mut(pet).pet.as_deref_mut().expect("a pet unit")
    }

    /// Go `Unit.EnableFocusBar`.
    pub(crate) fn enable_focus_bar(&mut self, unit: UnitId, focus_regen_multiplier: f64) {
        let u = self.unit_mut(unit);
        u.current_power_bar = PowerBar::Focus;
        u.focus_bar = super::sim::FocusBar {
            enabled: true,
            max_focus: 100.0,
            current_focus: 0.0,
            focus_regen_per_tick: 2.5 * focus_regen_multiplier,
            focus_tick_duration: 250 * super::sim::MILLISECOND,
        };
    }

    /// Go `Pet.Initialize`.
    pub(crate) fn initialize_pet(&mut self, _pet: UnitId) {
        // Registering with the owner's regen inheritance list only matters in a fight.
    }

    /// The pet's `EnableManaBar`: a pet's own scaling is its own, so it gets no intellect
    /// dependencies.
    pub(crate) fn enable_pet_mana_bar(&mut self, pet: UnitId) {
        self.register_spell(
            pet,
            super::spell::SpellConfig {
                action_id: crate::contracts::prepared_v2::ActionId {
                    other_id: "OtherActionManaGain".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        // A pet's class is unknown, which gets the default coefficients.
        let data = self.pet_data_mut(pet);
        data.spirit_regen_base = 7.5;
        data.spirit_regen_per_spirit = 1.0 / 10.0;
        let base_mana = data.base_stats[Stat::Mana];
        let u = self.unit_mut(pet);
        u.mana_bar.enabled = true;
        u.mana_bar.base_mana = base_mana;
        u.current_power_bar = PowerBar::Mana;
    }

    /// Go `Pet.inheritOwnerStats`.
    fn inherit_owner_stats(&mut self, pet: UnitId) {
        let owner = self.unit(pet).owner.expect("a pet has an owner");
        let owner_stats = self.unit(owner).stats;
        let inherit = Rc::clone(&self.pet_data(pet).stat_inheritance);
        let inherited = inherit(self, &owner_stats);
        self.pet_data_mut(pet).inherited_stats = inherited;
        self.add_stats_dynamic(pet, &inherited);
    }

    /// Go `Pet.AddOwnerStats`: the pet inherits a change in its owner's stats.
    pub(crate) fn add_owner_stats(&mut self, pet: UnitId, added: &Stats) {
        let inherit = Rc::clone(&self.pet_data(pet).stat_inheritance);
        let change = inherit(self, added);
        self.pet_data_mut(pet).inherited_stats.add_inplace(&change);
        self.add_stats_dynamic(pet, &change);
    }

    fn reset_inherited_owner_stats(&mut self, pet: UnitId) {
        let inherited = self.pet_data(pet).inherited_stats;
        self.add_stats_dynamic(pet, &inherited.invert());
        self.pet_data_mut(pet).inherited_stats = Stats::default();
    }

    /// Go `Pet.Enable`.
    pub(crate) fn enable_pet(&mut self, pet: UnitId) {
        if self.unit(pet).enabled {
            return;
        }
        if self.pet_data(pet).is_dynamic {
            self.inherit_owner_stats(pet);
            self.pet_data_mut(pet).dynamic_stat_inheritance = true;
        } else {
            self.inherit_owner_stats(pet);
        }
        // Reset the current mana and health after applying stats.
        if self.unit(pet).mana_bar.enabled {
            let max = self.max_mana(pet);
            let bar = &mut self.unit_mut(pet).mana_bar;
            bar.current_mana = max;
            bar.mana_regen_multiplier = 1.0;
        }
        if self.unit(pet).health_bar {
            let max = self.unit(pet).stats[Stat::Health];
            self.unit_mut(pet).current_health = max;
        }
        self.unit_mut(pet).enabled = true;
        if let Some(on_enable) = self.pet_data(pet).on_pet_enable.clone() {
            on_enable(self, pet);
        }
        let owner = self.unit(pet).owner.expect("a pet has an owner");
        if self.pet_data(pet).has_dynamic_melee_speed_inheritance {
            let melee = self.unit(owner).pseudo_stats.melee_speed_multiplier;
            let attack = self.unit(owner).pseudo_stats.attack_speed_multiplier;
            for multiplier in [melee, attack] {
                self.pet_data_mut(pet).inherited_melee_speed_multiplier *= multiplier;
                self.multiply_melee_speed(pet, multiplier);
            }
            self.pet_data_mut(pet).dynamic_melee_speed_enabled = true;
        }
        if self.pet_data(pet).has_dynamic_cast_speed_inheritance {
            let multiplier = self.unit(owner).pseudo_stats.cast_speed_multiplier;
            self.pet_data_mut(pet).inherited_cast_speed_multiplier *= multiplier;
            self.multiply_cast_speed(pet, multiplier);
            self.pet_data_mut(pet).dynamic_cast_speed_enabled = true;
        }
        if self.unit(pet).focus_bar.enabled {
            let bar = &mut self.unit_mut(pet).focus_bar;
            bar.current_focus = bar.max_focus;
        }
        if self.unit(pet).energy_bar.enabled {
            let bar = &mut self.unit_mut(pet).energy_bar;
            bar.current_energy = bar.max_energy;
            if self.pet_data(pet).has_resource_regen_inheritance {
                let attack = self.unit(owner).pseudo_stats.attack_speed_multiplier;
                self.unit_mut(pet).energy_bar.energy_regen_multiplier *= attack;
            }
        }
    }

    /// Go `Pet.Disable`.
    pub(crate) fn disable_pet(&mut self, pet: UnitId) {
        if !self.unit(pet).enabled {
            return;
        }
        if self.pet_data(pet).is_dynamic {
            if self.pet_data(pet).dynamic_stat_inheritance {
                self.pet_data_mut(pet).dynamic_stat_inheritance = false;
                self.reset_inherited_owner_stats(pet);
            }
        } else {
            self.reset_inherited_owner_stats(pet);
        }
        if self.pet_data(pet).dynamic_melee_speed_enabled {
            let inherited = self.pet_data(pet).inherited_melee_speed_multiplier;
            self.pet_data_mut(pet).inherited_melee_speed_multiplier *= 1.0 / inherited;
            self.multiply_melee_speed(pet, 1.0 / inherited);
            self.pet_data_mut(pet).dynamic_melee_speed_enabled = false;
        }
        if self.pet_data(pet).dynamic_cast_speed_enabled {
            let inherited = self.pet_data(pet).inherited_cast_speed_multiplier;
            self.pet_data_mut(pet).inherited_cast_speed_multiplier *= 1.0 / inherited;
            self.multiply_cast_speed(pet, 1.0 / inherited);
            self.pet_data_mut(pet).dynamic_cast_speed_enabled = false;
        }
        self.unit_mut(pet).enabled = false;
        if let Some(on_disable) = self.pet_data(pet).on_pet_disable.clone() {
            on_disable(self, pet);
        }
        // auraTracker.expireAll: the active auras that expire. A permanent aura is not in
        // Go's `activeAuras` and stays up, such as a party aura on the pet (patch 131).
        for aura in self.unit(pet).auras.clone() {
            if self.aura(aura).duration != super::sim::NEVER_EXPIRES {
                self.deactivate(aura);
            }
        }
        self.pet_data_mut(pet).dismissed_at_reset = true;
    }
}

impl Environment {
    /// The pets of the player, in registration order.
    pub(crate) fn pets(&self) -> Vec<UnitId> {
        self.sim.unit(self.player).pets.clone()
    }

    /// The pets' half of `Environment.initialize`: Go runs each pet's `Initialize` after the
    /// player's.
    pub(crate) fn initialize_pets(&mut self) {
        for pet in self.pets() {
            self.sim.initialize_pet(pet);
            match self.sim.pet_data(pet).on_initialize.clone() {
                Some(initialize) => initialize(&mut self.sim, pet),
                None => self.agent.initialize_pet(&mut self.sim, pet),
            }
        }
    }

    /// The pets' half of `Character.Finalize`, run after the player's: Go's `Pet.Finalize`.
    pub(crate) fn finalize_pets(&mut self) {
        for pet in self.pets() {
            self.finalize_unit(pet);
        }
    }

    /// Go `Pet.reset`, run for each pet after its owner's reset.
    pub(crate) fn reset_pets(&mut self) {
        for pet in self.pets() {
            if self.sim.pet_data(pet).is_reset {
                continue;
            }
            self.sim.pet_data_mut(pet).is_reset = true;
            // Character.reset.
            reset_unit(&mut self.sim, pet);
            let default = self.sim.unit(pet).default_target;
            self.sim.unit_mut(pet).current_target = default;
            // A gear pet's own reset only disables the pet, which is not enabled yet.
            if self.sim.pet_data(pet).on_initialize.is_none() {
                self.agent.reset_pet(&mut self.sim, pet);
            }
            self.sim.unit_mut(pet).enabled = false;
            if self.sim.pet_data(pet).enabled_on_start {
                self.sim.enable_pet(pet);
            }
        }
    }
}

/// Guardians a common effect summons during a fight, by pet name: common/classic
/// emerald_dragon_whelp.go.
fn summoned_pet_name(name: &str) -> bool {
    name == "Emerald Dragon Whelp"
}

/// Go `summonedPet`: a guardian an exported effect summons during a fight, or a pet a class
/// effect summons.
pub(crate) fn summoned_pet(env: &Environment, pet: UnitId) -> bool {
    let data = env.sim.pet_data(pet);
    ((summoned_pet_name(&data.name) && data.is_guardian) || env.agent.summoned_pet(&env.sim, pet))
        && !data.enabled_on_start
}

/// Go `simulatedPet`: whether Rust simulates the pet, enabled at reset by a class whose pets
/// only change there, or a guardian an effect summons.
pub(crate) fn simulated_pet(env: &Environment, pet: UnitId) -> bool {
    (env.agent.reset_only_pets() && env.sim.pet_data(pet).enabled_on_start)
        || summoned_pet(env, pet)
}

/// Go `exportPets`' `statValues` of a stats array read as a name map.
fn nonzero_stats(stats: &Stats) -> Map<String, Value> {
    let mut out = Map::new();
    for stat in Stat::ALL {
        if stats[stat] != 0.0 {
            out.insert(stat.name().to_string(), json!(stats[stat]));
        }
    }
    out
}

/// Go `applyDependencies`: stat dependencies, then `FloorGameStats`, as `AddStatsDynamic`
/// recomputes a unit's stats.
pub(crate) fn apply_dependencies(without: Stats, deps: &[(Stat, Stat, f64, f64)]) -> Stats {
    let mut s = without;
    for (src, dst, amount, step) in deps {
        if src == dst {
            s[*dst] *= *amount;
        } else if *step != 0.0 {
            // Go's arm64 build fuses each sum into one multiply-add (pets.go 124, 126, 128).
            s[*dst] = ((s[*src] / *step).floor() * *step).mul_add(*amount, s[*dst]);
        } else if matches!(
            src,
            Stat::Strength | Stat::Agility | Stat::Stamina | Stat::Intellect | Stat::Spirit
        ) {
            s[*dst] = s[*src].floor().mul_add(*amount, s[*dst]);
        } else {
            s[*dst] = s[*src].mul_add(*amount, s[*dst]);
        }
    }
    s.floor_game_stats()
}

/// A dynamic pet's linear stat inheritance, as terms of one owner stat for each pet stat
/// (Go `petInheritance`). Anything the terms cannot describe is noted as unrepresented.
fn pet_inheritance(
    env: &Environment,
    index: usize,
    pet: UnitId,
    unrepresented: &mut Vec<String>,
) -> Vec<Value> {
    let label = env.sim.unit(pet).label.clone();
    let inherit = Rc::clone(&env.sim.pet_data(pet).stat_inheritance);
    // Go probes the exported simulation's closure; a class's closure may keep state, so the
    // probes run on a separate reset copy.
    let mut probe = env.fresh();
    let mut inherit = |stats: &Stats| inherit(&mut probe.sim, stats);
    let mut terms = Vec::new();
    let mut sources: Vec<(Stat, Stat, f64)> = Vec::new();
    for owner in Stat::ALL {
        let mut unit = Stats::default();
        unit[owner] = 1.0;
        let inherited = inherit(&unit);
        for pet_stat in Stat::ALL {
            if inherited[pet_stat] == 0.0 {
                continue;
            }
            sources.push((owner, pet_stat, inherited[pet_stat]));
            terms.push(json!({"owner": owner.name(), "pet": pet_stat.name(),
                "coefficient": inherited[pet_stat]}));
        }
    }
    // Linear, bit for bit, on changes of either sign. A pet stat several owner stats feed (the
    // Hunter pet's crit, patch 136) takes the sum of their terms, so each pair is checked too.
    for amount in [3.7, -12.25, 123.456, 0.1] {
        for (owner, pet_stat, coefficient) in &sources {
            let mut change = Stats::default();
            change[*owner] = amount;
            let got = inherit(&change)[*pet_stat];
            let want = amount * coefficient;
            if got != want && !(got.is_nan() && want.is_nan()) {
                unrepresented.push(format!(
                    "pet {label} inherits {} nonlinearly",
                    pet_stat.name()
                ));
            }
            for (other, other_stat, other_coefficient) in &sources {
                if other_stat != pet_stat || (*other as usize) <= (*owner as usize) {
                    continue;
                }
                let mut change = Stats::default();
                change[*owner] = amount;
                change[*other] = -2.5 * amount;
                let got = inherit(&change)[*pet_stat];
                let want = amount * coefficient + -2.5 * amount * other_coefficient;
                if got != want {
                    unrepresented.push(format!(
                        "pet {label} inherits {} as other than the sum of its terms",
                        pet_stat.name()
                    ));
                }
            }
        }
    }
    let deps = env.sim.unit(pet).sdm.enabled_dependencies();
    if apply_dependencies(env.sim.unit(pet).stats_without_deps, &deps) != env.sim.unit(pet).stats {
        unrepresented.push(format!(
            "pet {label}'s stats do not follow its dependencies"
        ));
    }
    let mut owners: Vec<Stat> = Vec::new();
    for (owner, _, _) in &sources {
        if !owners.contains(owner) {
            owners.push(*owner);
        }
    }
    for owner in owners {
        let mut fresh = env.fresh();
        let reset_pet = fresh.sim.unit(fresh.player).pets[index];
        let mut without = fresh.sim.unit(reset_pet).stats_without_deps;
        let mut change = Stats::default();
        change[owner] = 37.5;
        fresh.sim.add_owner_stats(reset_pet, &change);
        let inherited = inherit(&change);
        without.add_inplace(&inherited);
        if apply_dependencies(without, &deps) != fresh.sim.unit(reset_pet).stats {
            unrepresented.push(format!(
                "pet {label}'s inherited {} does not follow its dependencies",
                owner.name()
            ));
        }
    }
    terms
}

/// The stats Disable logs: the pet's stats once its inheritance is removed, from a separate
/// reset simulation. Go's `Pet.doneIteration` expires the pet's auras first, undoing their
/// stat buffs.
fn pet_dismiss_stats(env: &Environment, index: usize) -> Stats {
    let mut fresh = env.fresh();
    let pet = fresh.sim.unit(fresh.player).pets[index];
    for aura in fresh.sim.unit(pet).auras.clone() {
        if fresh.sim.aura(aura).active {
            fresh.sim.deactivate(aura);
        }
    }
    fresh.sim.disable_pet(pet);
    fresh.sim.unit(pet).stats
}

/// The stats Disable logs at the reset or during a fight, the pet's permanent auras still up
/// (Go `petActiveDismissStats`), from a separate reset simulation.
fn pet_active_dismiss_stats(env: &Environment, index: usize) -> Stats {
    let mut fresh = env.fresh();
    let pet = fresh.sim.unit(fresh.player).pets[index];
    fresh.sim.disable_pet(pet);
    fresh.sim.unit(pet).stats
}

/// Whether a reset logs the pet's dismissal (Go `dismissedAtReset`), read from a separate reset
/// simulation: only a pet whose agent's reset disables it does.
fn dismissed_at_reset(env: &Environment, index: usize) -> bool {
    let fresh = env.fresh();
    let pet = fresh.sim.unit(fresh.player).pets[index];
    fresh.sim.pet_data(pet).dismissed_at_reset
}

/// The changes the fight's end makes to a dynamic pet's stats before dependencies as each
/// permanent aura expires (Go `petAuraFadeStats`), from a separate reset simulation.
fn pet_aura_fade_stats(env: &Environment, index: usize) -> Vec<Value> {
    let mut fresh = env.fresh();
    let pet = fresh.sim.unit(fresh.player).pets[index];
    let mut changes = Vec::new();
    for aura in fresh.sim.unit(pet).auras.clone() {
        let a = fresh.sim.aura(aura);
        if !a.active || a.duration != super::sim::NEVER_EXPIRES {
            continue;
        }
        let label = a.label.clone();
        fresh.sim.unit_mut(pet).stats_without_deps = Stats::default();
        fresh.sim.deactivate(aura);
        let change = fresh.sim.unit(pet).stats_without_deps;
        let values = nonzero_stats(&change);
        if !values.is_empty() {
            changes.push(json!({"aura": label, "stats": values}));
        }
    }
    changes
}

/// What a pet's `OnPetDisable` adds to its stats before dependencies, from a separate reset
/// simulation.
fn pet_disable_stats(env: &Environment, index: usize) -> Map<String, Value> {
    let mut fresh = env.fresh();
    let pet = fresh.sim.unit(fresh.player).pets[index];
    fresh.sim.unit_mut(pet).stats_without_deps = Stats::default();
    if let Some(on_disable) = fresh.sim.pet_data(pet).on_pet_disable.clone() {
        on_disable(&mut fresh.sim, pet);
    }
    nonzero_stats(&fresh.sim.unit(pet).stats_without_deps)
}

/// Go `Unit.ManaRegenPerSecondWhileCasting` and `WhileNotCasting` of a pet.
fn pet_mana(env: &Environment, pet: UnitId) -> Value {
    let unit = env.sim.unit(pet);
    let data = env.sim.pet_data(pet);
    let spirit_regen =
        unit.stats[Stat::Spirit].mul_add(data.spirit_regen_per_spirit, data.spirit_regen_base);
    let mp5 = unit.stats[Stat::MP5] / 5.0;
    let pseudo = &unit.pseudo_stats;
    let casting = {
        let mut regen = mp5;
        let mut spirit = 0.0;
        if pseudo.spirit_regen_rate_casting != 0.0 || pseudo.force_full_spirit_regen {
            spirit = spirit_regen * pseudo.spirit_regen_multiplier;
            if !pseudo.force_full_spirit_regen {
                spirit *= pseudo.spirit_regen_rate_casting;
            }
        }
        regen += spirit;
        regen * unit.mana_bar.mana_regen_multiplier
    };
    let not_casting = spirit_regen.mul_add(pseudo.spirit_regen_multiplier, mp5)
        * unit.mana_bar.mana_regen_multiplier;
    json!({"max": unit.stats[Stat::Mana], "regen_per_second_casting": casting,
        "regen_per_second_not_casting": not_casting})
}

/// Go `exportPets`: the simulated pets of the player, in registration order. A feature Rust does
/// not model is named in `unrepresented`.
pub(crate) fn export_pets(
    env: &Environment,
    timers: &mut TimerNames,
    unrepresented: &mut Vec<String>,
) -> Vec<Value> {
    let target = env.encounter.targets[0];
    let mut pets = Vec::new();
    for (index, pet) in env.pets().into_iter().enumerate() {
        if !simulated_pet(env, pet) {
            continue;
        }
        let sim = &env.sim;
        let data = sim.pet_data(pet);
        let unit = sim.unit(pet);
        let label = unit.label.clone();
        let summoned = summoned_pet(env, pet);
        let class_summoned = env.agent.summoned_pet(sim, pet);
        let stat_callbacks = env.agent.stat_callback_pet(sim, pet);
        let mut note = |condition: bool, message: String| {
            if condition {
                unrepresented.push(message);
            }
        };
        note(
            data.is_guardian && !summoned,
            format!("pet {label} is a guardian"),
        );
        note(
            !unit.enabled && !summoned,
            format!("pet {label} is not enabled after the reset"),
        );
        note(
            unit.enabled && summoned,
            format!("summoned pet {label} is enabled after the reset"),
        );
        note(
            !unit.mana_bar.enabled && !unit.focus_bar.enabled && !class_summoned,
            format!("pet {label} has no mana bar"),
        );
        note(
            unit.mana_bar.enabled && unit.focus_bar.enabled,
            format!("pet {label} has both mana and focus"),
        );
        note(
            data.has_dynamic_melee_speed_inheritance,
            format!("pet {label} inherits melee speed"),
        );
        note(
            data.has_dynamic_cast_speed_inheritance,
            format!("pet {label} inherits cast speed"),
        );
        note(
            data.has_resource_regen_inheritance,
            format!("pet {label} inherits resource regeneration"),
        );
        note(
            data.start_attack_delay != 0,
            format!("pet {label} delays its first attack"),
        );
        note(
            (data.on_pet_enable.is_some() || data.on_pet_disable.is_some())
                && !class_summoned
                && !stat_callbacks,
            format!("pet {label} has enable or disable callbacks"),
        );
        note(
            unit.energy_bar.enabled,
            format!("pet {label} has an energy bar"),
        );
        note(!unit.pets.is_empty(), format!("pet {label} has pets"));
        note(
            unit.on_cast_speed_changed != 0 || !unit.on_temporary_stats_changes.is_empty(),
            format!("pet {label} has speed or stat listeners"),
        );
        note(
            sim.damage_done_by_caster_handlers(pet, target) != 0,
            format!("pet {label} has caster damage callbacks"),
        );
        let table = env.attack_table(pet, target).clone();
        let spells: Vec<Value> = unit
            .spellbook
            .iter()
            .map(|spell| export_spell_of(env, *spell, timers, unrepresented))
            .collect();
        let dismiss_stats = pet_dismiss_stats(env, index);
        let summon = [
            format!(
                "Pet stats: {}",
                super::common_effects::flat_string(&unit.stats)
            ),
            format!(
                "Pet inherited stats: {}",
                super::common_effects::flat_string(
                    &unit.sdm.apply_stat_dependencies(data.inherited_stats)
                ),
            ),
        ];
        let dismiss = super::common_effects::flat_string(&dismiss_stats);
        let mut exported = Map::new();
        exported.insert("index".into(), json!(unit.unit_index));
        exported.insert("label".into(), json!(label));
        exported.insert("level".into(), json!(unit.level));
        exported.insert("stats".into(), stat_values(&unit.stats));
        exported.insert("pseudo_stats".into(), export_pseudo(&unit.pseudo_stats));
        exported.insert("auras".into(), export_auras(sim, pet, timers));
        exported.insert("name".into(), json!(data.name));
        exported.insert("reaction_ns".into(), json!(unit.reaction_time));
        exported.insert("distance_yards".into(), json!(unit.distance_from_target));
        exported.insert("cast_speed".into(), json!(unit.cast_speed));
        exported.insert("mana".into(), pet_mana(env, pet));
        exported.insert(
            "attack_table".into(),
            json!({
                "base_spell_miss_chance": table.base_spell_miss_chance,
                "spell_crit_suppression": table.spell_crit_suppression,
                "bonus_spell_crit_percent": table.bonus_spell_crit_percent,
                "crit_multiplier": table.crit_multiplier,
                "damage_dealt_multiplier": table.damage_dealt_multiplier,
                "damage_taken_multiplier": table.damage_taken_multiplier,
            }),
        );
        exported.insert(
            "melee".into(),
            export_melee_of(env, pet, false, unrepresented),
        );
        exported.insert("spells".into(), Value::Array(spells));
        exported.insert("metrics_actions".into(), metrics_actions(env, pet));
        exported.insert("summon_log".into(), json!(summon));
        exported.insert("dismiss_log".into(), json!(dismiss));
        exported.insert("dynamic_stats".into(), json!(data.is_dynamic));
        if unit.focus_bar.enabled {
            exported.insert(
                "focus".into(),
                json!({"max": unit.focus_bar.max_focus,
                    "regen_per_tick": unit.focus_bar.focus_regen_per_tick,
                    "tick_duration_ns": unit.focus_bar.focus_tick_duration}),
            );
        }
        if unit.distance_from_target > super::character::constants::MAX_MELEE_RANGE {
            // Go `GetMovementSpeed` of a pet.
            exported.insert(
                "movement_speed".into(),
                json!(8.0 * unit.pseudo_stats.movement_speed_multiplier),
            );
        }
        if summoned {
            exported.insert("summoned".into(), json!(true));
        }
        if data.is_dynamic {
            let terms = pet_inheritance(env, index, pet, unrepresented);
            if !terms.is_empty() {
                exported.insert("inheritance".into(), Value::Array(terms));
            }
            let without = nonzero_stats(&unit.stats_without_deps);
            if !without.is_empty() {
                exported.insert("stats_without_deps".into(), Value::Object(without));
            }
            let deps: Vec<Value> = unit
                .sdm
                .enabled_dependencies()
                .into_iter()
                .map(|(src, dst, amount, step)| {
                    let mut dep = json!({"src": src.name(), "dst": dst.name(), "amount": amount});
                    if step != 0.0 {
                        dep["step"] = json!(step);
                    }
                    dep
                })
                .collect();
            if !deps.is_empty() {
                exported.insert("stat_dependencies".into(), Value::Array(deps));
            }
            if !summoned {
                let inherited = nonzero_stats(&data.inherited_stats);
                if !inherited.is_empty() {
                    exported.insert("inherited_stats".into(), Value::Object(inherited));
                }
                let fade = pet_aura_fade_stats(env, index);
                if !fade.is_empty() {
                    exported.insert("aura_stats".into(), Value::Array(fade));
                }
                if data.on_pet_disable.is_some() && env.agent.stat_callback_pet(sim, pet) {
                    let disable = pet_disable_stats(env, index);
                    if !disable.is_empty() {
                        exported.insert("disable_stats".into(), Value::Object(disable));
                    }
                }
                exported.insert(
                    "dismiss_stats".into(),
                    json!(pet_dismiss_stat_list(&dismiss_stats, &exported)),
                );
            }
        }
        if !exported.contains_key("dismiss_stats") {
            let active = super::common_effects::flat_string(&pet_active_dismiss_stats(env, index));
            if active != dismiss {
                exported.insert("active_dismiss_log".into(), json!(active));
            }
        }
        pets.push(Value::Object(exported));
    }
    pets
}

/// The stats of a dynamic pet's dismissal line, in Go's stat order: each stat `FlatString`
/// prints, and each stat the runtime can change during a fight (the tracked powers, the
/// inherited stats and their dependencies), which a float residue can bring into the line.
fn pet_dismiss_stat_list(dismiss: &Stats, exported: &Map<String, Value>) -> Vec<Value> {
    let mut tracked: Vec<String> = [
        "SpellDamage",
        "AttackPower",
        "RangedAttackPower",
        "SpellCritPercent",
        "PhysicalCritPercent",
    ]
    .iter()
    .map(|name| name.to_string())
    .collect();
    if let Some(Value::Array(terms)) = exported.get("inheritance") {
        for term in terms {
            tracked.push(term["pet"].as_str().unwrap_or_default().to_string());
        }
    }
    if let Some(Value::Array(deps)) = exported.get("stat_dependencies") {
        for dep in deps {
            tracked.push(dep["dst"].as_str().unwrap_or_default().to_string());
        }
    }
    // A permanent aura's stats count until the fight's end expires it.
    if let Some(Value::Array(auras)) = exported.get("aura_stats") {
        for aura in auras {
            if let Some(Value::Object(stats)) = aura.get("stats") {
                tracked.extend(stats.keys().cloned());
            }
        }
    }
    let mut list = Vec::new();
    for stat in Stat::ALL {
        let name = stat.name();
        if dismiss[stat] != 0.0 || tracked.iter().any(|tracked| tracked == name) {
            list.push(json!({"stat": name, "value": dismiss[stat]}));
        }
    }
    list
}

/// Go `inertPetEffect`: a pet that is registered but never enabled. Each reset enables its unit
/// and its agent's reset dismisses it, logging its stats; each fight's end logs that no pet is
/// summoned. Its metrics report zero, with every action and aura it registered.
pub(crate) fn inert_pet_effect(
    env: &Environment,
    index: usize,
    reason: &str,
    unrepresented: &mut Vec<String>,
) -> Value {
    let pet = env.pets()[index];
    let sim = &env.sim;
    let unit = sim.unit(pet);
    let mut auras = Vec::new();
    let mut permanent = Vec::new();
    // Go reports each exclusive effect's uptime with the aura. A permanent aura whose effects
    // are each alone in their category on the pet holds them for as long as it is up, so each
    // effect's uptime is the aura's: a party aura on the pet since patch 131, such as Mana
    // Spring Totem. Any other exclusive effect the inert pet's metrics do not carry.
    let mut exclusive = Vec::new();
    for aura in &unit.auras {
        let a = sim.aura(*aura);
        let id = super::export::action_id(a.action_id.as_ref());
        if !id.is_null() {
            auras.push(id.clone());
            if !a.exclusive_effects.is_empty() {
                let mut alone = a.active && a.duration == super::sim::NEVER_EXPIRES;
                let mut categories = Vec::new();
                for effect in &a.exclusive_effects {
                    let category = &sim.categories[sim.effects[effect.0].category.0];
                    alone = alone && category.effects.len() == 1;
                    categories.push(category.name.clone());
                }
                if alone {
                    exclusive.push(json!({"aura": id, "categories": categories}));
                } else {
                    unrepresented.push(format!(
                        "inert pet {}'s aura {} has exclusive effects",
                        unit.label, a.label
                    ));
                }
            }
        }
        if a.active {
            if a.duration != super::sim::NEVER_EXPIRES {
                unrepresented.push(format!(
                    "inert pet {} has the expiring aura {}",
                    unit.label, a.label
                ));
            }
            if !id.is_null() {
                permanent.push(id);
            }
        }
    }
    let mut effect = json!({
        "kind": "inert_pet", "name": sim.pet_data(pet).name, "label": unit.label,
        "unit_index": unit.unit_index, "metrics_actions": metrics_actions(env, pet),
        "auras": auras,
        "dismissed_log": super::common_effects::flat_string(&unit.stats), "reason": reason,
    });
    if !permanent.is_empty() {
        effect["permanent_auras"] = Value::Array(permanent);
    }
    if !exclusive.is_empty() {
        effect["exclusive_effects"] = Value::Array(exclusive);
    }
    // Only a pet whose agent's reset disables it logs its dismissal at each reset; a pet that is
    // simply not enabled on start, such as a warlock's other demons, logs nothing then.
    if !dismissed_at_reset(env, index) {
        effect["dismissed_at_reset"] = json!(false);
    }
    if unit.mana_bar.enabled {
        effect["mana_bar"] = json!(true);
        if sim.rules == super::sim::Rules::Forever {
            effect["max_mana"] = json!(unit.stats[Stat::Mana]);
        }
    }
    effect
}

/// The inert pet effects of the player's registered pets that Rust does not simulate, in
/// registration order. A pet without a reason is unsupported.
pub(crate) fn inert_pet_effects(env: &Environment, unrepresented: &mut Vec<String>) -> Vec<Value> {
    let mut effects = Vec::new();
    for (index, pet) in env.pets().into_iter().enumerate() {
        if simulated_pet(env, pet) {
            continue;
        }
        match env.agent.inert_pet(&env.sim, pet) {
            None => unrepresented.push("pets are unsupported".to_string()),
            Some(reason) => effects.push(inert_pet_effect(env, index, reason, unrepresented)),
        }
    }
    effects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepare::sim::{EnvState, Unit};

    fn owner_with_pet(inherit: PetStatInheritance) -> (Sim, UnitId, UnitId) {
        owner_with_pet_at_level(60, inherit)
    }

    fn owner_with_pet_at_level(level: i32, inherit: PetStatInheritance) -> (Sim, UnitId, UnitId) {
        let mut sim = Sim::new();
        let mut unit = Unit::new(UnitType::Player, "owner".to_string());
        unit.level = level;
        unit.reaction_time = 100_000_000;
        unit.start_distance_from_target = 12.0;
        unit.distance_from_target = 12.0;
        let owner = sim.add_unit(unit);
        let mut base_stats = Stats::default();
        base_stats[Stat::Stamina] = 50.0;
        let pet = sim.new_pet(PetConfig {
            name: "Wolf".to_string(),
            owner,
            base_stats,
            stat_inheritance: inherit,
            enabled_on_start: true,
            is_guardian: false,
            is_dynamic: true,
            has_dynamic_melee_speed_inheritance: false,
            has_dynamic_cast_speed_inheritance: false,
            has_resource_regen_inheritance: false,
            starts_at_owner_distance: true,
        });
        sim.add_pet(owner, pet);
        (sim, owner, pet)
    }

    #[test]
    fn a_pet_is_labeled_for_its_owner_and_starts_at_the_owners_distance() {
        let (sim, owner, pet) = owner_with_pet(Rc::new(|_, _| Stats::default()));
        assert_eq!(sim.unit(pet).label, "owner - Wolf");
        assert_eq!(sim.unit(pet).owner, Some(owner));
        assert_eq!(sim.unit(owner).pets, [pet]);
        assert_eq!(sim.unit(pet).distance_from_target, 12.0);
        assert_eq!(sim.unit(pet).reaction_time, 100_000_000);
        // The first pet takes the first index past the party.
        assert_eq!(sim.unit(pet).index, 5);
    }

    #[test]
    fn native_pets_match_prepared_owner_level_across_warlock_samples() {
        for level in [20, 40, 60] {
            let (sim, owner, pet) =
                owner_with_pet_at_level(level, Rc::new(|_, _| Stats::default()));
            assert_eq!(sim.unit(owner).level, level);
            assert_eq!(sim.unit(pet).level, level, "pet at L{level}");
            assert_eq!(sim.unit(pet).owner, Some(owner));
        }
    }

    #[test]
    fn enabling_a_pet_adds_the_stats_it_inherits_and_disabling_takes_them_away() {
        let (mut sim, owner, pet) = owner_with_pet(Rc::new(|_, owner: &Stats| {
            let mut inherited = Stats::default();
            inherited[Stat::Stamina] = owner[Stat::Stamina] * 0.3;
            inherited
        }));
        sim.unit_mut(owner).stats[Stat::Stamina] = 100.0;
        // Stats change through AddStatsDynamic once the stats are measured.
        sim.measuring_stats = true;
        sim.enable_pet(pet);
        assert!(sim.unit(pet).enabled);
        assert_eq!(sim.unit(pet).stats[Stat::Stamina], 80.0);
        assert_eq!(sim.pet_data(pet).inherited_stats[Stat::Stamina], 30.0);
        sim.disable_pet(pet);
        assert!(!sim.unit(pet).enabled);
        assert_eq!(sim.unit(pet).stats[Stat::Stamina], 50.0);
        assert!(sim.pet_data(pet).inherited_stats.is_zero());
        assert!(sim.pet_data(pet).dismissed_at_reset);
    }

    #[test]
    fn a_focus_bar_fills_when_the_pet_is_enabled() {
        let (mut sim, _, pet) = owner_with_pet(Rc::new(|_, _| Stats::default()));
        sim.enable_focus_bar(pet, 1.2);
        let bar = &sim.unit(pet).focus_bar;
        assert_eq!(bar.max_focus, 100.0);
        assert_eq!(bar.focus_regen_per_tick, 2.5 * 1.2);
        assert_eq!(
            bar.focus_tick_duration,
            250 * crate::prepare::sim::MILLISECOND
        );
        sim.measuring_stats = true;
        sim.enable_pet(pet);
        assert_eq!(sim.unit(pet).focus_bar.current_focus, 100.0);
    }

    #[test]
    #[should_panic(expected = "Pets must be added during construction")]
    fn a_pet_cannot_be_added_after_construction() {
        let (mut sim, owner, pet) = owner_with_pet(Rc::new(|_, _| Stats::default()));
        sim.state = EnvState::Constructed;
        sim.add_pet(owner, pet);
    }
}
