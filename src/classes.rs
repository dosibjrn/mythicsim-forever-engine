//! Class domains. A module exists only when it contains implemented behavior.

pub(crate) mod druid;
pub(crate) mod hunter;
pub(crate) mod mage;
pub(crate) mod paladin;
pub(crate) mod priest;
pub(crate) mod rogue;
pub(crate) mod shaman;
pub(crate) mod warlock;
pub(crate) mod warrior;

/// Run a prepared v2 input that passed the coverage gate with its class's agent.
pub(crate) fn run_prepared(
    prepared: &crate::contracts::prepared_v2::PreparedV2,
    rng_mode: crate::core::rng::RngMode,
) -> Result<crate::core::fight::FightReport, String> {
    match prepared.player.class.as_str() {
        "ClassMage" => mage::prepared::run_prepared(prepared, rng_mode),
        "ClassDruid" => druid::prepared::run_prepared(prepared, rng_mode),
        "ClassWarlock" => warlock::prepared::run_prepared(prepared, rng_mode),
        "ClassShaman" => shaman::prepared::run_prepared(prepared, rng_mode),
        "ClassPaladin" => paladin::prepared::run_prepared(prepared, rng_mode),
        "ClassPriest" => priest::prepared::run_prepared(prepared, rng_mode),
        "ClassRogue" => rogue::prepared::run_prepared(prepared, rng_mode),
        "ClassWarrior" => warrior::prepared::run_prepared(prepared, rng_mode),
        "ClassHunter" => hunter::prepared::run_prepared(prepared, rng_mode),
        other => Err(format!("class {other} has no fight agent")),
    }
}


/// Multiple distinct world ACKs with one source-native actor.
pub(crate) fn native_two_world_targets(
    prepared: &crate::contracts::prepared_v2::PreparedV2,
    target_count: usize,
    rng_mode: crate::core::rng::RngMode,
) -> Result<serde_json::Value, String> {
    match prepared.player.class.as_str() {
        "ClassMage" => mage::agent::MageAgent::fight(prepared, rng_mode)?.native_two_world_targets(target_count).map_err(str::to_string),
        "ClassDruid" => druid::agent::DruidAgent::fight(prepared, rng_mode)?.native_two_world_targets(target_count).map_err(str::to_string),
        "ClassWarlock" => warlock::agent::WarlockAgent::fight(prepared, rng_mode)?.native_two_world_targets(target_count).map_err(str::to_string),
        "ClassShaman" => shaman::agent::ShamanAgent::fight(prepared, rng_mode)?.native_two_world_targets(target_count).map_err(str::to_string),
        "ClassPaladin" => paladin::agent::PaladinAgent::fight(prepared, rng_mode)?.native_two_world_targets(target_count).map_err(str::to_string),
        "ClassPriest" => priest::agent::PriestAgent::fight(prepared, rng_mode)?.native_two_world_targets(target_count).map_err(str::to_string),
        "ClassRogue" => rogue::agent::RogueAgent::fight(prepared, rng_mode)?.native_two_world_targets(target_count).map_err(str::to_string),
        "ClassWarrior" => warrior::agent::WarriorAgent::fight(prepared, rng_mode)?.native_two_world_targets(target_count).map_err(str::to_string),
        "ClassHunter" => hunter::agent::HunterAgent::fight(prepared, rng_mode)?.native_two_world_targets(target_count).map_err(str::to_string),
        other => Err(format!("class {other} has no native fight agent")),
    }
}

/// Progress one logical native actor through consecutive prepared level segments.
pub(crate) fn native_level_route(
    segments: &[crate::contracts::prepared_v2::PreparedV2],
    target_count_per_level: usize,
    rng_mode: crate::core::rng::RngMode,
) -> Result<serde_json::Value, String> {
    macro_rules! run_route {
        ($make_fight:expr) => {{
            let mut make_fight = $make_fight;
            let mut carry: Option<crate::core::fight::RouteCarry> = None;
            let mut target_offset = 0usize;
            let mut reports = Vec::with_capacity(segments.len());
            for (segment_index, prepared) in segments.iter().enumerate() {
                let mut fight = make_fight(prepared, rng_mode)?;
                let (report, next_carry) = fight.native_world_route_segment(
                    target_count_per_level,
                    target_offset,
                    segment_index,
                    Some(prepared.player.level),
                    Some(&prepared.request_sha256),
                    carry,
                    segment_index + 1 == segments.len(),
                ).map_err(str::to_string)?;
                reports.push(report);
                carry = Some(next_carry);
                target_offset += target_count_per_level;
            }
            Ok(serde_json::json!({
                "scope": "MULTI_LEVEL_GSIM_2D_ROUTE_RESOURCE_CARRY_NOT_GATE",
                "actor_identity_continues": true,
                "resource_carry_across_levels": true,
                "world_ack_chain_continuous": true,
                "actual_2d_ack": true,
                "combat_instance_rebuilt_at_level_transition": true,
                "class_state_reinitialized_per_level": true,
                "cooldowns_carried": false,
                "paid_recovery": false,
                "targets_per_level": target_count_per_level,
                "world_targets_killed": target_offset,
                "levels": segments.iter().map(|segment| segment.player.level).collect::<Vec<_>>(),
                "request_sha256s": segments.iter().map(|segment| segment.request_sha256.clone()).collect::<Vec<_>>(),
                "segments": reports,
                "checkpoint_certified": false,
            }))
        }};
    }

    match segments.first().map(|segment| segment.player.class.as_str()) {
        Some("ClassMage") => run_route!(|prepared, mode| mage::agent::MageAgent::fight(prepared, mode)),
        Some("ClassDruid") => run_route!(|prepared, mode| druid::agent::DruidAgent::fight(prepared, mode)),
        Some("ClassWarlock") => run_route!(|prepared, mode| warlock::agent::WarlockAgent::fight(prepared, mode)),
        Some("ClassShaman") => run_route!(|prepared, mode| shaman::agent::ShamanAgent::fight(prepared, mode)),
        Some("ClassPaladin") => run_route!(|prepared, mode| paladin::agent::PaladinAgent::fight(prepared, mode)),
        Some("ClassPriest") => run_route!(|prepared, mode| priest::agent::PriestAgent::fight(prepared, mode)),
        Some("ClassRogue") => run_route!(|prepared, mode| rogue::agent::RogueAgent::fight(prepared, mode)),
        Some("ClassWarrior") => run_route!(|prepared, mode| warrior::agent::WarriorAgent::fight(prepared, mode)),
        Some("ClassHunter") => run_route!(|prepared, mode| hunter::agent::HunterAgent::fight(prepared, mode)),
        Some(other) => Err(format!("class {other} has no native route agent")),
        None => Err("native level route has no prepared segments".to_string()),
    }
}

/// Run the same native actor through two source-backed combat windows.
/// This is a bounded diagnostic, not a live-2D or leveling gate.
pub(crate) fn native_two_pull_windows(
    prepared: &crate::contracts::prepared_v2::PreparedV2,
    rng_mode: crate::core::rng::RngMode,
) -> Result<serde_json::Value, String> {
    match prepared.player.class.as_str() {
        "ClassMage" => mage::agent::MageAgent::fight(prepared, rng_mode)?.native_two_pull_windows().map_err(str::to_string),
        "ClassDruid" => druid::agent::DruidAgent::fight(prepared, rng_mode)?.native_two_pull_windows().map_err(str::to_string),
        "ClassWarlock" => warlock::agent::WarlockAgent::fight(prepared, rng_mode)?.native_two_pull_windows().map_err(str::to_string),
        "ClassShaman" => shaman::agent::ShamanAgent::fight(prepared, rng_mode)?.native_two_pull_windows().map_err(str::to_string),
        "ClassPaladin" => paladin::agent::PaladinAgent::fight(prepared, rng_mode)?.native_two_pull_windows().map_err(str::to_string),
        "ClassPriest" => priest::agent::PriestAgent::fight(prepared, rng_mode)?.native_two_pull_windows().map_err(str::to_string),
        "ClassRogue" => rogue::agent::RogueAgent::fight(prepared, rng_mode)?.native_two_pull_windows().map_err(str::to_string),
        "ClassWarrior" => warrior::agent::WarriorAgent::fight(prepared, rng_mode)?.native_two_pull_windows().map_err(str::to_string),
        "ClassHunter" => hunter::agent::HunterAgent::fight(prepared, rng_mode)?.native_two_pull_windows().map_err(str::to_string),
        other => Err(format!("class {other} has no native fight agent")),
    }
}

/// The class agent Rust preparation builds for a player: Go's agent factory.
pub(crate) fn prepare_agent(
    sim: &mut crate::prepare::sim::Sim,
    unit: crate::prepare::sim::UnitId,
    player: &crate::contracts::request::Message,
) -> Result<Box<dyn crate::prepare::agent::PrepAgent>, crate::prepare::Refusal> {
    match player.enum_name("class").as_str() {
        "ClassMage" => mage::prepare::new_mage(sim, unit, player),
        "ClassRogue" => rogue::prepare::new_rogue(sim, unit, player),
        "ClassWarlock" => warlock::prepare::new_warlock(sim, unit, player),
        "ClassHunter" => hunter::prepare::new_hunter(sim, unit, player),
        "ClassDruid" => druid::prepare::new_druid(sim, unit, player),
        "ClassWarrior" => warrior::prepare::new_warrior(sim, unit, player),
        "ClassPaladin" => paladin::prepare::new_paladin(sim, unit, player),
        "ClassPriest" => priest::prepare::new_priest(sim, unit, player),
        "ClassShaman" => shaman::prepare::new_shaman(sim, unit, player),
        other => Err(crate::prepare::Refusal::new(
            "class",
            format!("{other} is not prepared in Rust yet"),
        )),
    }
}

/// The item sets the classes implement: Go's `core.NewItemSet` calls in each class package.
pub(crate) fn item_sets() -> Vec<&'static crate::prepare::item_sets::ItemSet> {
    let mut sets = Vec::new();
    sets.extend(mage::prepare::items::ITEM_SETS);
    sets.extend(rogue::prepare::items::ITEM_SETS);
    sets.extend(warlock::prepare::items::ITEM_SETS);
    sets.extend(hunter::prepare::items::ITEM_SETS);
    sets.extend(druid::prepare::items::ITEM_SETS);
    sets.extend(warrior::prepare::items::sets::ITEM_SETS);
    sets.extend(paladin::prepare::sets::ITEM_SETS);
    sets.extend(priest::prepare::items::ITEM_SETS);
    sets.extend(shaman::prepare::items::ITEM_SETS);
    sets
}

/// The item effects of a class package that a character of any class wears, since Go registers
/// them against the item and they read nothing of their own class's agent: the idols, librams and
/// totems that add a spell mod by class mask. The effects that read the agent (Wolfshead Helm,
/// Libram of Light) panic for another class in Go, and are not here.
pub(crate) fn agentless_item_effect(
    sim: &mut crate::prepare::sim::Sim,
    unit: crate::prepare::sim::UnitId,
    item: i32,
) -> bool {
    druid::prepare::items::apply_agentless_idol_effect(sim, unit, item)
        || paladin::prepare::items::apply_agentless_libram_effect(sim, unit, item)
        || shaman::prepare::items::apply_agentless_totem_effect(sim, unit, item)
}
