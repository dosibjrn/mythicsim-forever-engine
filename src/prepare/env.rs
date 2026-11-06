//! Go sim/core/environment.go and sim.go: construct, initialize, finalize and reset the one
//! player and its targets a request describes.

use crate::contracts::request::Message;

use super::agent::PrepAgent;
use super::attack::{new_attack_table, AttackTable};
use super::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use super::character::constants::CHARACTER_LEVEL;
use super::sim::{
    AuraConfig, BuildPhase, EnvState, EventCallbacks, Rules, Sim, UnitId, UnitType, NEVER_EXPIRES,
};
use super::stats::Stat;
use super::Refusal;
use std::rc::Rc;

/// Go `Encounter`, with the settings the export reads.
#[derive(Clone, Debug, Default)]
pub(crate) struct Encounter {
    pub duration: i64,
    pub duration_variation: i64,
    pub execute_proportion_20: f64,
    pub execute_proportion_25: f64,
    pub execute_proportion_35: f64,
    pub execute_proportion_45: f64,
    pub execute_proportion_90: f64,
    pub area_types: Vec<String>,
    pub targets: Vec<UnitId>,
    pub end_fight_at_health: f64,
    /// Drops the boss's 1% hit suppression on melee and ranged attacks (the fork's patch 125,
    /// the request's `Encounter.no_hit_suppression`).
    pub no_hit_suppression: bool,
}

/// A prepared simulation: the arena, its one player and the class agent.
#[derive(Clone)]
pub(crate) struct Environment {
    pub sim: Sim,
    pub encounter: Encounter,
    pub player: UnitId,
    pub agent: Box<dyn PrepAgent>,
    pub request: Message,
    /// The raid units in Go's `Raid.AllUnits` order.
    pub raid_units: Vec<UnitId>,
    /// Effects run after every unit is finalized.
    pub post_finalize: Vec<FinalizeEffect>,
    pub pre_finalize: Vec<FinalizeEffect>,
    /// `env.prepullActions`.
    pub prepull_actions: usize,
    /// The dot base durations the rotation's `dotBaseDuration` values captured as it was built.
    pub rotation_dot_base_durations: Vec<crate::contracts::prepared_v2::RotationDotBaseDuration>,
    /// What the rotation's values read that the rest of the export leaves out.
    pub rotation_reads: Option<crate::contracts::prepared_v2::RotationReads>,
    /// The factory that built the agent, for a separate simulation of the same request.
    pub factory: AgentFactory,
    /// The rules it was prepared with, for a separate simulation of the same request.
    pub rules: Rules,
    /// Selected native player level retained across pristine/fresh environments.
    pub actor_level: Option<i32>,
    /// Every attacker's table against every defender, by unit index.
    pub attack_tables: Vec<Vec<AttackTable>>,
    /// The party buffs the drivers read: the request's with what the agent adds.
    pub party_buffs: Message,
    /// What each external cooldown's driver computed once the environment finalized, by the
    /// aura it casts.
    pub external_activations: Vec<(
        super::sim::AuraId,
        crate::contracts::prepared_v2::ExternalActivation,
    )>,
    /// What a healer's gear changes under Forever's rules (`healer_gear`).
    pub healer_gear: super::healer_gear::HealerGear,
    /// The first [`Environment::fresh`] simulation, which every later one copies.
    pristine: Rc<std::cell::OnceCell<Environment>>,
}

/// Go `PostFinalizeEffect`: run once every unit is finalized, with the attack tables in place.
pub(crate) type FinalizeEffect = Rc<dyn Fn(&mut Environment)>;

/// The class agent factory: `classes::prepare_agent`.
pub(crate) type AgentFactory =
    fn(&mut Sim, UnitId, &Message) -> Result<Box<dyn PrepAgent>, Refusal>;

fn duration_from_seconds(seconds: f64) -> i64 {
    (seconds * 1e9) as i64
}

/// Go `NewEncounter` and `NewTarget`.
fn new_encounter(sim: &mut Sim, options: &Message) -> Result<Encounter, Refusal> {
    let p20 = options.f64("execute_proportion_20");
    let p25 = options.f64("execute_proportion_25").max(p20);
    let p35 = options.f64("execute_proportion_35").max(p25);
    let p45 = options.f64("execute_proportion_45").max(p35);
    let mut encounter = Encounter {
        duration: duration_from_seconds(options.f64("duration")),
        duration_variation: duration_from_seconds(options.f64("duration_variation")),
        execute_proportion_20: p20.max(0.0),
        execute_proportion_25: p25.max(0.0),
        execute_proportion_35: p35.max(0.0),
        execute_proportion_45: p45.max(0.0),
        execute_proportion_90: options.f64("execute_proportion_90").max(0.0),
        area_types: Vec::new(),
        targets: Vec::new(),
        end_fight_at_health: 0.0,
        no_hit_suppression: options.bool("no_hit_suppression"),
    };
    for area in options.enum_names("area_types") {
        if area != "AreaTypeUnknown" && !encounter.area_types.contains(&area) {
            encounter.area_types.push(area);
        }
    }
    let targets = options.messages("targets");
    if targets.is_empty() {
        return Err(Refusal::new(
            "encounter",
            "an encounter without targets".to_string(),
        ));
    }
    // tools/oracle-v2/targets.go targetNotes: up to five identical copies of the boss.
    if targets.len() > 5 {
        return Err(Refusal::new(
            "targets",
            format!("{} targets: at most 5 are supported", targets.len()),
        ));
    }
    if targets
        .iter()
        .skip(1)
        .any(|target| target.encode() != targets[0].encode())
    {
        return Err(Refusal::new(
            "targets",
            "only identical copies of the first target are supported".to_string(),
        ));
    }
    for (index, target) in targets.into_iter().enumerate() {
        let id = super::target::new_target(sim, target, index as i32)?;
        encounter.targets.push(id);
    }
    if options.bool("use_health") {
        return Err(Refusal::new(
            "encounter",
            "health-based fights are unsupported".to_string(),
        ));
    }
    Ok(encounter)
}

impl Environment {
    /// Go `NewSim` followed by `Reset`, with Forever's rules.
    pub(crate) fn new(request: &Message, factory: AgentFactory) -> Result<Environment, Refusal> {
        Environment::with_rules(request, factory, Rules::Forever)
    }

    /// Go `NewSim` followed by `Reset`, with the given rules.
    pub(crate) fn with_rules(
        request: &Message,
        factory: AgentFactory,
        rules: Rules,
    ) -> Result<Environment, Refusal> {
        Self::with_rules_at_level(request, factory, rules, None)
    }

    /// Allow a native-selected actor level while retaining the exact legacy
    /// L60 path for every existing request.
    pub(crate) fn with_rules_at_level(
        request: &Message,
        factory: AgentFactory,
        rules: Rules,
        actor_level: Option<i32>,
    ) -> Result<Environment, Refusal> {
        let mut sim = Sim::new();
        sim.rules = rules;
        let raid = request
            .message("raid")
            .ok_or_else(|| Refusal::new("request", "a request without a raid".to_string()))?;
        let encounter_options = request
            .message("encounter")
            .cloned()
            .unwrap_or_else(|| Message::empty("proto.Encounter"));

        // construct
        let encounter = new_encounter(&mut sim, &encounter_options)?;
        // Go `NewRaid`: the active parties are the first `num_active_parties`, all of them when
        // it is zero, and pets number from five indexes past each. The UI sends five.
        let active_parties = raid.i32("num_active_parties");
        if active_parties < 0 || raid.i32("target_dummies") != 0 {
            return Err(Refusal::new(
                "raid",
                "parties beyond one player are unsupported".to_string(),
            ));
        }
        sim.first_pet_index = 5 * active_parties.max(1);
        let parties = raid.messages("parties");
        let players: Vec<&Message> = parties
            .first()
            .map(|party| party.messages("players"))
            .unwrap_or_default();
        if parties.len() != 1 || players.len() != 1 || players[0].enum_number("class") == 0 {
            return Err(Refusal::new(
                "raid",
                "exactly one player is supported".to_string(),
            ));
        }
        let player_message = players[0].clone();
        let player = sim.new_character_at_level(
            0,
            0,
            &player_message,
            &encounter.area_types,
            actor_level,
        )?;
        let agent = factory(&mut sim, player, &player_message)?;
        // Go `NewAgent` then runs the gear pet constructors.
        super::classic_whelp::construct_gear_pets(&mut sim, player);
        // updatePlayersAndPets: the player, then its pets by index.
        let mut raid_units = vec![player];
        raid_units.extend(sim.unit(player).pets.clone());
        let mut all_units = encounter.targets.clone();
        all_units.extend(raid_units.iter().copied());
        for (index, unit) in all_units.iter().enumerate() {
            sim.unit_mut(*unit).unit_index = index as i32;
        }
        sim.env_units = all_units;
        for unit in &raid_units {
            sim.unit_mut(*unit).current_target = Some(encounter.targets[0]);
        }
        let mut env = Environment {
            pristine: Rc::default(),
            sim,
            encounter,
            player,
            agent,
            request: request.clone(),
            raid_units,
            post_finalize: Vec::new(),
            pre_finalize: Vec::new(),
            prepull_actions: 0,
            rotation_dot_base_durations: Vec::new(),
            rotation_reads: None,
            attack_tables: Vec::new(),
            party_buffs: Message::empty("proto.PartyBuffs"),
            external_activations: Vec::new(),
            factory,
            rules,
            actor_level,
            healer_gear: super::healer_gear::HealerGear::default(),
        };
        if let Some(debuffs) = raid.message("debuffs") {
            // The agent's constructor may have changed the raid's debuffs.
            let mut debuffs = debuffs.clone();
            env.agent.adjust_raid_debuffs(&mut debuffs);
            for (index, target) in env.encounter.targets.clone().into_iter().enumerate() {
                super::debuffs::apply_debuff_effects(&mut env, target, index, &debuffs, raid)?;
            }
        }
        env.setup_tank_targets(raid, &encounter_options)?;
        env.sim.state = EnvState::Constructed;

        // initialize
        for (index, target) in env.encounter.targets.clone().into_iter().enumerate() {
            let options = encounter_options.messages("targets")[index].clone();
            super::target::initialize_target(&mut env.sim, target, &options);
        }
        // Character.initialize: the major cooldown manager holds nothing yet, and the item swap
        // takes its character.
        env.sim.initialize_item_swap(env.player);
        env.apply_character_effects(raid, &player_message)?;
        let player = env.player;
        env.agent.initialize(&mut env.sim, player);
        env.initialize_pets();
        env.sim.state = EnvState::Initialized;

        env.finalize(&player_message)?;
        env.reset();
        Ok(env)
    }

    /// A separate reset simulation of the same request, as the exporter's `core.NewSim` followed
    /// by `Reset` for an effect read under other conditions. It cannot be refused: this
    /// request already prepared once.
    ///
    /// Building one reads only the request and static data, so each is a copy of the first,
    /// which spares the effects that read one per combination of auras a build for each. A
    /// copy shares the first's callbacks, so what a callback changes lives in the simulation
    /// ([`StateSlot`](super::sim::StateSlot)), never in what the callback captures; a captured
    /// cell is written only while the environment is built.
    pub(crate) fn fresh(&self) -> Environment {
        self.pristine
            .get_or_init(|| {
                Environment::with_rules_at_level(
                    &self.request,
                    self.factory,
                    self.rules,
                    self.actor_level,
                )
                .expect("the request prepared once already")
            })
            .clone()
    }

    fn setup_tank_targets(&mut self, raid: &Message, encounter: &Message) -> Result<(), Refusal> {
        let tanks = raid.messages("tanks");
        let mut tank_has_target = false;
        for (index, target) in self.encounter.targets.clone().into_iter().enumerate() {
            let options = encounter.messages("targets")[index];
            let first = options.i32("tank_index");
            let second = options.i32("second_tank_index");
            for (tank_index, is_first) in [(first, true), (second, false)] {
                if !is_first && second == first {
                    continue;
                }
                if tank_index < 0 || tank_index as usize >= tanks.len() {
                    continue;
                }
                let reference = tanks[tank_index as usize];
                let tank = match reference.enum_name("type").as_str() {
                    "Player" if reference.i32("index") == 0 => self.player,
                    // Go `setupTankTarget`: a raid index with no player behind it, as the UI's
                    // default tanks name in a one-player raid, leaves the target without a tank.
                    "Player" => continue,
                    _ => return Err(Refusal::new(
                        "tanks",
                        "tank assignments other than the player tanking the target are unsupported"
                            .to_string(),
                    )),
                };
                if is_first {
                    self.sim.unit_mut(target).current_target = Some(tank);
                } else {
                    self.sim.unit_mut(target).secondary_target = Some(tank);
                }
                if !tank_has_target {
                    self.sim.unit_mut(tank).current_target = Some(target);
                    tank_has_target = true;
                }
            }
        }
        Ok(())
    }

    /// Whether a target swings at the player: Go `Metrics.isTanking`.
    pub(crate) fn tanking(&self) -> bool {
        self.encounter.targets.iter().any(|target| {
            let unit = self.sim.unit(*target);
            unit.current_target == Some(self.player) || unit.secondary_target == Some(self.player)
        })
    }

    /// Go `Raid.applyCharacterEffects` for the one player.
    fn apply_character_effects(&mut self, raid: &Message, player: &Message) -> Result<(), Refusal> {
        let mut raid_buffs = raid
            .message("buffs")
            .cloned()
            .unwrap_or_else(|| Message::empty("proto.RaidBuffs"));
        self.agent.add_raid_buffs(&mut raid_buffs);
        let party = raid.messages("parties")[0];
        let mut party_buffs = party
            .message("buffs")
            .cloned()
            .unwrap_or_else(|| Message::empty("proto.PartyBuffs"));
        self.agent.add_party_buffs(&self.sim, &mut party_buffs);
        // Go `spreadRaidWidePartyBuffs`: Leader of the Pack and Moonkin Aura reach the whole raid
        // since the 16 October 2026 beta notes, so either on any party reaches the player.
        for field in ["leader_of_the_pack", "moonkin_aura"] {
            let anywhere = raid.messages("parties").iter().any(|other| {
                other
                    .message("buffs")
                    .is_some_and(|buffs| buffs.bool(field))
            });
            if anywhere && !party_buffs.bool(field) {
                party_buffs.set_bool(field, true);
            }
        }
        self.party_buffs = party_buffs.clone();
        let individual = player
            .message("buffs")
            .cloned()
            .unwrap_or_else(|| Message::empty("proto.IndividualBuffs"));
        let unit = self.player;
        self.sim.unit_mut(unit).health_bar = true;
        // health.go trackChanceOfDeath.
        // Go reads a healing model only for a player a target swings at (`trackChanceOfDeath`),
        // and runs a presim for one with a cadence but no healing (`GetPresimOptions`). Any other
        // model changes nothing: the UI's empty default, or a tank preset's in a fight the
        // player does not tank.
        let tanking = self.tanking();
        let healing_model = player.message("healing_model").filter(|model| {
            let presim = model.f64("hps") == 0.0 && model.f64("cadence_seconds") != 0.0;
            presim || (tanking && !model.set_fields().is_empty())
        });
        if healing_model.is_some() {
            return Err(Refusal::new(
                "healing_model",
                "healing models are unsupported".to_string(),
            ));
        }
        self.sim.register_aura(
            unit,
            AuraConfig {
                label: "Chance of Death".to_string(),
                duration: NEVER_EXPIRES,
                on_reset: Some(Rc::new(|sim: &mut Sim, aura| sim.activate(aura))),
                events: EventCallbacks {
                    on_spell_hit_taken: true,
                    on_periodic_damage_taken: true,
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        self.apply_all_effects(&raid_buffs, &party_buffs, &individual)?;
        self.sim
            .register_battle_squawk(unit, party.i32("battle_squawks"));
        for pet in self.sim.unit(unit).pets.clone() {
            self.sim.unit_mut(pet).health_bar = true;
            // A guardian is not in the party; a summoned pet is (fork patch 131).
            if !self.sim.pet_data(pet).is_guardian {
                super::buffs::pet_buffs::apply_pet_party_buffs(self, pet, &party_buffs)?;
            }
        }
        Ok(())
    }

    /// Go `Character.applyAllEffects`.
    fn apply_all_effects(
        &mut self,
        raid_buffs: &Message,
        party_buffs: &Message,
        individual: &Message,
    ) -> Result<(), Refusal> {
        let unit = self.player;
        // Go sets the hook here, not in NewCharacter, so the set bonuses' skill reaches the
        // character.
        self.sim.unit_mut(unit).on_weapon_skill_change = true;
        if !self.sim.character(unit).disable_racials {
            super::racials::apply_race_effects(self)?;
        }
        super::professions::apply_profession_effects(self);
        self.sim.apply_build_phase_auras(unit, BuildPhase::BASE);

        self.sim.apply_equipment(unit);
        super::item_effects::apply_item_effects(self)?;
        super::item_effects::apply_item_set_bonus_effects(self)?;
        super::healer_gear::apply(self)?;
        self.sim.apply_build_phase_auras(unit, BuildPhase::GEAR);

        self.agent.apply_talents(&mut self.sim, unit);
        let effects = self.agent.take_post_finalize_effects();
        self.post_finalize.extend(effects);
        self.sim.apply_build_phase_auras(unit, BuildPhase::TALENTS);

        super::buffs::apply_buff_effects(self, raid_buffs, party_buffs, individual)?;
        self.sim.apply_build_phase_auras(unit, BuildPhase::BUFFS);

        super::consumes::apply_consume_effects(self, party_buffs)?;
        self.sim.apply_build_phase_auras(unit, BuildPhase::CONSUMES);
        self.sim.clear_build_phase_auras(unit, BuildPhase::ALL);
        Ok(())
    }

    /// Go `Environment.finalize` without the fake prepull.
    fn finalize(&mut self, player: &Message) -> Result<(), Refusal> {
        for effect in std::mem::take(&mut self.pre_finalize) {
            effect(self);
        }
        for target in self.encounter.targets.clone() {
            self.finalize_unit(target);
        }
        self.finalize_character()?;
        let rotation = player.message("rotation").cloned();
        super::rotation::build_rotation(self, rotation.as_ref())?;
        self.setup_attack_tables();
        let mut i = 0;
        while i < self.post_finalize.len() {
            let effect = Rc::clone(&self.post_finalize[i]);
            effect(self);
            i += 1;
        }
        self.post_finalize.clear();
        self.sim.state = EnvState::Finalized;
        Ok(())
    }

    /// Go `Character.Finalize`.
    fn finalize_character(&mut self) -> Result<(), Refusal> {
        let unit = self.player;
        self.sim.unit_mut(unit).pseudo_stats.parry_haste =
            self.sim.unit(unit).pseudo_stats.can_parry;
        if self.tanking() {
            self.register_tanking_auras();
        }
        self.finalize_unit(unit);
        self.sim.finalize_major_cooldowns(unit);
        self.finalize_pets();
        Ok(())
    }

    /// The part of Go `Character.Finalize` for a unit a target swings at: the "Reduced
    /// avoidance" aura a hardcast holds, and the "Pushback trigger" proc trigger that pushes a
    /// hardcast back when a hit deals damage. The trigger's condition and handler only run in a
    /// fight; preparation records the aura's callbacks.
    fn register_tanking_auras(&mut self) {
        let unit = self.player;
        let refresh: super::sim::AuraCallback = Rc::new(|sim: &mut Sim, aura| {
            let unit = sim.aura(aura).unit;
            sim.refresh_incapacitate_state(unit);
        });
        let aura = self.sim.register_aura(
            unit,
            AuraConfig {
                label: "Reduced avoidance".to_string(),
                tag: super::incapacitate::REDUCED_AVOIDANCE_AURA_TAG.to_string(),
                duration: NEVER_EXPIRES,
                on_gain: Some(Rc::clone(&refresh)),
                on_expire: Some(refresh),
                ..Default::default()
            },
        );
        self.sim.character_mut(unit).hardcast_avoidance_aura = Some(aura);
        self.sim.make_proc_trigger_aura(
            unit,
            &ProcTrigger {
                name: "Pushback trigger".to_string(),
                callback: CallbackMask::ON_SPELL_HIT_TAKEN,
                outcome: HitOutcome::LANDED,
                require_damage_dealt: true,
                ..ProcTrigger::default()
            },
        );
    }

    /// Go `Unit.finalize`.
    pub(crate) fn finalize_unit(&mut self, unit: UnitId) {
        let sim = &mut self.sim;
        assert!(
            sim.unit(unit).initial_stats.is_zero(),
            "Initial stats may not be set before finalized"
        );
        assert!(sim.unit(unit).reaction_time != 0, "Unset unit.ReactionTime");
        let current = sim.unit(unit).current_target;
        sim.unit_mut(unit).default_target = current;
        sim.apply_parry_haste(unit);
        sim.update_cast_speed(unit);
        sim.update_attack_speed(unit);
        sim.update_melee_and_ranged_haste(unit);
        sim.init_movement(unit);
        let melee = sim.total_melee_haste_multiplier(unit);
        let ranged = sim.total_ranged_haste_multiplier(unit);
        let u = sim.unit_mut(unit);
        u.initial_stats_without_deps = u.stats;
        u.initial_cast_speed = u.cast_speed;
        u.initial_melee_swing_speed = melee;
        u.initial_ranged_swing_speed = ranged;
        u.sdm.finalize_stat_deps();
        u.initial_stats = u
            .sdm
            .apply_stat_dependencies(u.initial_stats_without_deps)
            .floor_game_stats();
        u.stats_without_deps = u.initial_stats_without_deps;
        u.stats = u.initial_stats;
        sim.update_reduced_crit_taken_percent(unit);
        let pseudo = sim.unit(unit).pseudo_stats.clone();
        sim.unit_mut(unit).initial_pseudo_stats = pseudo;
        sim.finalize_auto_attacks(unit);
        sim.finalize_spells(unit);
    }

    /// Go `setupAttackTables`.
    fn setup_attack_tables(&mut self) {
        let units = self.sim.env_units.clone();
        self.attack_tables = units
            .iter()
            .map(|attacker| {
                units
                    .iter()
                    .map(|defender| {
                        let defender = self.sim.unit(*defender);
                        let mut table = new_attack_table(
                            self.sim.unit(*attacker).level,
                            defender.unit_type,
                            defender.level,
                        );
                        if self.encounter.no_hit_suppression {
                            table.hit_suppression = 0.0;
                        }
                        table
                    })
                    .collect()
            })
            .collect();
    }

    /// The table of one attacker against one defender.
    pub(crate) fn attack_table(&self, attacker: UnitId, defender: UnitId) -> &AttackTable {
        let a = self.sim.unit(attacker).unit_index as usize;
        let d = self.sim.unit(defender).unit_index as usize;
        &self.attack_tables[a][d]
    }

    pub(crate) fn attack_table_mut(
        &mut self,
        attacker: UnitId,
        defender: UnitId,
    ) -> &mut AttackTable {
        let a = self.sim.unit(attacker).unit_index as usize;
        let d = self.sim.unit(defender).unit_index as usize;
        &mut self.attack_tables[a][d]
    }

    /// Go `Simulation.reset`: targets first, then the raid.
    fn reset(&mut self) {
        self.sim.current_time = 0;
        for target in self.encounter.targets.clone() {
            reset_unit(&mut self.sim, target);
            let default = self.sim.unit(target).default_target;
            self.sim.unit_mut(target).current_target = default;
        }
        let unit = self.player;
        reset_unit(&mut self.sim, unit);
        let default = self.sim.unit(unit).default_target;
        self.sim.unit_mut(unit).current_target = default;
        self.agent.reset(&mut self.sim, unit);
        // Go Character.reset: the item swap's reset, after the agent's.
        self.item_swap_reset();
        self.reset_pets();
    }
}

/// Go `Unit.reset` for what a reset leaves for the export.
pub(crate) fn reset_unit(sim: &mut Sim, unit: UnitId) {
    if sim.unit(unit).unit_type != UnitType::Enemy {
        sim.unit_mut(unit).enabled = true;
    }
    let u = sim.unit_mut(unit);
    u.distance_from_target = u.start_distance_from_target;
    u.sdm.reset_stat_deps();
    u.stats_without_deps = u.initial_stats_without_deps;
    u.stats = u.initial_stats;
    u.pseudo_stats = u.initial_pseudo_stats.clone();
    // auraTracker.reset: reset effects, then every aura.
    for effect in sim.unit(unit).reset_effects.clone() {
        effect(sim);
    }
    for aura in sim.unit(unit).auras.clone() {
        sim.reset_aura(aura);
    }
    // focusBar.reset: a pet's bar fills, its regeneration starts when the pet is enabled.
    if sim.unit(unit).focus_bar.enabled {
        let bar = &mut sim.unit_mut(unit).focus_bar;
        bar.current_focus = bar.max_focus;
    }
    // manaBar.reset runs after the auras reset.
    if sim.unit(unit).mana_bar.enabled {
        let max = sim.max_mana(unit);
        let bar = &mut sim.unit_mut(unit).mana_bar;
        bar.current_mana = max;
        bar.mana_regen_multiplier = 1.0;
    }
    // healthBar.reset: the current health is the maximum the reset's auras left.
    if sim.unit(unit).health_bar {
        let max = sim.unit(unit).stats[Stat::Health];
        sim.unit_mut(unit).current_health = max;
    }
    // unit.reset: the energy bar and the rage bar follow the mana bar.
    sim.reset_energy_bar(unit);
    sim.reset_rage_bar(unit);
    let _ = CHARACTER_LEVEL;
    let _ = Stat::Mana;
}
