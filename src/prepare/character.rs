//! Go sim/core/character.go: a player built from its request, with equipment, base stats,
//! build phases and major cooldowns.

use std::collections::BTreeMap;
use std::rc::Rc;

use crate::contracts::prepared_v2::ActionId;
use crate::contracts::request::Message;

use super::items::{self, Equipment};
use super::sim::{
    AuraConfig, AuraId, BuildPhase, Duration, Rules, Sim, SpellId, TimerId, Unit, UnitId, UnitType,
    MILLISECOND,
};
use super::stats::{PseudoStats, Stat, Stats};
use super::Refusal;

/// The spell category of Forever's conjured items: mana gems, healthstones, Demonic and Dark
/// Rune and Thistle Tea all share its 2 minute cooldown (client 1.60.1.70532, ItemEffect).
pub(crate) const CONJURED_CATEGORY: i32 = 1153;

/// Go constants from constants.go and base_stats_auto_gen.go.
pub(crate) mod constants {
    pub const CHARACTER_LEVEL: i32 = 60;
    pub const EXPERTISE_RATING_PER_EXPERTISE_PERCENT: f64 = 10.0;
    pub const DEFENSE_RATING_PER_DEFENSE_LEVEL: f64 = 1.0;
    pub const DODGE_RATING_PER_DODGE_PERCENT: f64 = 12.0;
    pub const PARRY_RATING_PER_PARRY_PERCENT: f64 = 15.0;
    pub const BLOCK_RATING_PER_BLOCK_PERCENT: f64 = 5.0;
    pub const PHYSICAL_HIT_RATING_PER_HIT_PERCENT: f64 = 10.0;
    pub const SPELL_HIT_RATING_PER_HIT_PERCENT: f64 = 10.0;
    pub const PHYSICAL_CRIT_RATING_PER_CRIT_PERCENT: f64 = 14.0;
    pub const SPELL_CRIT_RATING_PER_CRIT_PERCENT: f64 = 14.0;
    pub const MISS_DODGE_PARRY_BLOCK_CRIT_CHANCE_PER_DEFENSE: f64 = 0.04;
    pub const DEFENSE_RATING_PER_AVOIDANCE_PERCENT: f64 =
        DEFENSE_RATING_PER_DEFENSE_LEVEL / MISS_DODGE_PARRY_BLOCK_CRIT_CHANCE_PER_DEFENSE;
    pub const MAX_MELEE_RANGE: f64 = 5.0;
    /// Go `MinRangedRange`, from the target's center like every distance here: the client's
    /// 8 yards count from the hitbox edges, and logged shots stop about 10.5 yards from a mob's
    /// center (fork patch 190, Arc's logs).
    pub const MIN_RANGED_RANGE: f64 = 10.5;
    pub const DEFAULT_ATTACK_POWER_PER_DPS: f64 = 14.0;
}

/// Go `MajorCooldown` as registered: the spell, priority, type and timings.
#[derive(Clone, Debug)]
pub(crate) struct MajorCooldown {
    pub spell: SpellId,
    pub priority: i32,
    /// Go `CooldownType` bits.
    pub cooldown_type: u32,
    pub allow_spell_queueing: bool,
    pub timings: Vec<Duration>,
}

/// Go `CooldownType` bits.
pub(crate) mod cooldown_type {
    pub const UNKNOWN: u32 = 0;
    pub const MANA: u32 = 1 << 0;
    pub const DPS: u32 = 1 << 1;
    pub const EXPLOSIVE: u32 = 1 << 2;
    pub const SURVIVAL: u32 = 1 << 3;
    pub const NAMES: [&str; 4] = [
        "CooldownTypeMana",
        "CooldownTypeDPS",
        "CooldownTypeExplosive",
        "CooldownTypeSurvival",
    ];
}

/// The character half of a player unit.
#[derive(Clone)]
pub(crate) struct Character {
    pub name: String,
    pub race: String,
    pub class: String,
    pub disable_racials: bool,
    pub equipment: Equipment,
    pub consumables: Message,
    pub base_stats: Stats,
    pub bonus_stats: Stats,
    pub professions: [String; 2],
    pub item_stat_multipliers: BTreeMap<usize, f64>,
    pub cached_equip_stats: Stats,
    pub equip_stats_applied: bool,
    pub equip_cache_valid: bool,
    pub cooldown_configs: Vec<Message>,
    pub hp_percent_for_defensives: f64,
    pub initial_major_cooldowns: Vec<MajorCooldown>,
    /// Go `Character.ItemProcBuffs`: the stat buffs of proc items and enchants, in the order
    /// `AddStatProcBuff` registered them.
    pub item_proc_buffs: Vec<super::aura_helpers::StatBuffAura>,
    /// Go `MajorCooldown.BuffAura` of each major cooldown that has one, by its spell.
    pub cooldown_buff_auras: Vec<(SpellId, super::aura_helpers::StatBuffAura)>,
    pub party_index: i32,
    pub spirit_regen_base: f64,
    pub spirit_regen_per_spirit: f64,
    pub rotation_transformations: usize,
    pub player: Message,
    /// Spells registered through `OnSpellRegistered`.
    pub spell_registration_handlers: Vec<super::spell::SpellRegisteredHandler>,
    /// Go `HardcastAvoidanceAura`: the tank's "Reduced avoidance" aura, which `Finalize`
    /// registers for a unit a target swings at.
    pub hardcast_avoidance_aura: Option<AuraId>,
    /// Go `ItemSwap`, when the request swaps items.
    pub item_swap: Option<super::item_swap::ItemSwap>,
}

/// Go `UnitLevelFloat64`.
pub(crate) fn unit_level_f64(
    level: i32,
    minus2: f64,
    plus0: f64,
    plus1: f64,
    plus2: f64,
    plus3: f64,
) -> f64 {
    use constants::CHARACTER_LEVEL;
    if level == CHARACTER_LEVEL - 2 {
        minus2
    } else if level == CHARACTER_LEVEL {
        plus0
    } else if level == CHARACTER_LEVEL + 1 {
        plus1
    } else if level == CHARACTER_LEVEL + 2 {
        plus2
    } else {
        plus3
    }
}

/// Go `RatingConversions` and `AddRatingConversions`.
pub(crate) fn add_rating_conversions(sim: &mut Sim, unit: UnitId) {
    use constants::*;
    let conversions: [(Stat, Stat, f64, f64); 12] = [
        (
            Stat::MeleeHitRating,
            Stat::PhysicalHitPercent,
            PHYSICAL_HIT_RATING_PER_HIT_PERCENT,
            0.0,
        ),
        (
            Stat::SpellHitRating,
            Stat::SpellHitPercent,
            SPELL_HIT_RATING_PER_HIT_PERCENT,
            0.0,
        ),
        (
            Stat::MeleeCritRating,
            Stat::PhysicalCritPercent,
            PHYSICAL_CRIT_RATING_PER_CRIT_PERCENT,
            0.0,
        ),
        (
            Stat::SpellCritRating,
            Stat::SpellCritPercent,
            SPELL_CRIT_RATING_PER_CRIT_PERCENT,
            0.0,
        ),
        (
            Stat::ExpertiseRating,
            Stat::ExpertisePercent,
            EXPERTISE_RATING_PER_EXPERTISE_PERCENT,
            0.0,
        ),
        (
            Stat::DodgeRating,
            Stat::DodgePercent,
            DODGE_RATING_PER_DODGE_PERCENT,
            0.0,
        ),
        (
            Stat::ParryRating,
            Stat::ParryPercent,
            PARRY_RATING_PER_PARRY_PERCENT,
            0.0,
        ),
        (
            Stat::BlockRating,
            Stat::BlockPercent,
            BLOCK_RATING_PER_BLOCK_PERCENT,
            0.0,
        ),
        (
            Stat::DefenseRating,
            Stat::ReducedCritTakenPercent,
            DEFENSE_RATING_PER_AVOIDANCE_PERCENT,
            DEFENSE_RATING_PER_DEFENSE_LEVEL,
        ),
        (
            Stat::DefenseRating,
            Stat::DodgePercent,
            DEFENSE_RATING_PER_AVOIDANCE_PERCENT,
            0.0,
        ),
        (
            Stat::DefenseRating,
            Stat::ParryPercent,
            DEFENSE_RATING_PER_AVOIDANCE_PERCENT,
            0.0,
        ),
        (
            Stat::DefenseRating,
            Stat::BlockPercent,
            DEFENSE_RATING_PER_AVOIDANCE_PERCENT,
            0.0,
        ),
    ];
    let sdm = &mut sim.unit_mut(unit).sdm;
    for (rating, percent, per_percent, step) in conversions {
        if step != 0.0 {
            sdm.add_floored_stat_dependency(rating, percent, step, 1.0 / per_percent);
        } else {
            sdm.add_stat_dependency(rating, percent, 1.0 / per_percent);
        }
    }
}

/// Go `Character.addUniversalStatDependencies`.
pub(crate) fn add_character_universal_stat_dependencies(sim: &mut Sim, unit: UnitId) {
    add_rating_conversions(sim, unit);
    sim.add_stat(unit, Stat::Health, 20.0 - 10.0 * 20.0);
    let sdm = &mut sim.unit_mut(unit).sdm;
    sdm.add_stat_dependency(Stat::Stamina, Stat::Health, 10.0);
    sdm.add_stat_dependency(Stat::Agility, Stat::Armor, 2.0);
}

/// Go `BaseStats[{race, class}]`.
fn base_stats(class: &str, race: &str) -> Option<Stats> {
    let values = crate::data::tables::tables()
        .base_stats
        .get(class)?
        .get(race)?;
    let mut out = Stats::default();
    for (name, value) in values {
        out[Stat::by_name(name).expect("a Go stat name")] = *value;
    }
    Some(out)
}

/// Source-backed classic-proxy Warlock L1-59 base stats, all levels from
/// one imported pinned Classic table, not a growing per-level allowlist.
/// Troll/Warlock has no direct historical row: project the source Orc primaries
/// with this native engine's Troll-minus-Orc primary race delta.
fn base_stats_at_level(class: &str, race: &str, level: i32) -> Option<Stats> {
    if level == constants::CHARACTER_LEVEL {
        return base_stats(class, race);
    }
    if class != "ClassWarlock" || race != "RaceTroll" {
        return None;
    }
    let source: serde_json::Value =
        serde_json::from_str(include_str!("../../data/classic-warlock-level-sources.json")).ok()?;
    if source["class"].as_str()? != class || source["race"].as_str()? != race
        || source["reference_race"].as_str()? != "RaceOrc"
    {
        return None;
    }
    let row = source["levels"].as_array()?.iter()
        .find(|row| row["level"].as_i64() == Some(i64::from(level)))?;
    let mut troll = base_stats(class, race)?;
    let orc = base_stats(class, "RaceOrc")?;
    let source_orc = &row["reference_race"];
    for (key, stat) in [
        ("strength", Stat::Strength),
        ("agility", Stat::Agility),
        ("stamina", Stat::Stamina),
        ("intellect", Stat::Intellect),
        ("spirit", Stat::Spirit),
    ] {
        troll[stat] = source_orc[key].as_f64()? + troll[stat] - orc[stat];
    }
    troll[Stat::Health] = row["class_base"]["health"].as_f64()?;
    troll[Stat::Mana] = row["class_base"]["mana"].as_f64()?;
    Some(troll)
}

impl Sim {
    pub(crate) fn character(&self, unit: UnitId) -> &Character {
        self.unit(unit).character.as_deref().expect("a player unit")
    }

    pub(crate) fn character_mut(&mut self, unit: UnitId) -> &mut Character {
        self.unit_mut(unit)
            .character
            .as_deref_mut()
            .expect("a player unit")
    }

    pub(crate) fn has_profession(&self, unit: UnitId, profession: &str) -> bool {
        self.character(unit)
            .professions
            .iter()
            .any(|p| p == profession)
    }

    /// Go `NewCharacter`.
    pub(crate) fn new_character(
        &mut self,
        party_index: i32,
        player_index: i32,
        player: &Message,
        area_types: &[String],
    ) -> Result<UnitId, Refusal> {
        self.new_character_at_level(party_index, player_index, player, area_types, None)
    }

    /// Explicit actor level is a native preparation input, not a protojson field.
    /// Unsupported class/race/level combinations fail closed instead of using L60 stats.
    pub(crate) fn new_character_at_level(
        &mut self,
        party_index: i32,
        player_index: i32,
        player: &Message,
        area_types: &[String],
        actor_level: Option<i32>,
    ) -> Result<UnitId, Refusal> {
        if player.has("database") {
            return Err(Refusal::new(
                "request",
                "a player database is unsupported".to_string(),
            ));
        }
        let index = party_index * 5 + player_index;
        let name = player.str("name").to_string();
        let mut unit = Unit::new(UnitType::Player, format!("{} (#{})", name, index + 1));
        unit.index = index;
        unit.level = actor_level.unwrap_or(constants::CHARACTER_LEVEL);
        unit.reaction_time = Duration::from(player.i32("reaction_time_ms").max(10)) * MILLISECOND;
        unit.channel_clip_delay =
            (Duration::from(player.i32("channel_clip_delay_ms")) * MILLISECOND).max(0);
        unit.start_distance_from_target = player.f64("distance_from_target");
        unit.distance_from_target = player.f64("distance_from_target");
        let race = player.enum_name("race");
        let class = player.enum_name("class");
        let equipment = items::equipment(player.message("equipment"), area_types)?;
        let base_stats = base_stats_at_level(&class, &race, unit.level).ok_or_else(|| {
            Refusal::new(
                "actor_level_source",
                format!("no selected native level {} base stats for {race} {class}", unit.level),
            )
        })?;
        unit.character = Some(Box::new(Character {
            name,
            race,
            class,
            disable_racials: player.bool("disable_racials"),
            equipment,
            consumables: player
                .message("consumables")
                .cloned()
                .unwrap_or_else(|| Message::empty("proto.ConsumesSpec")),
            base_stats,
            bonus_stats: Stats::default(),
            professions: [
                player.enum_name("profession1"),
                player.enum_name("profession2"),
            ],
            item_stat_multipliers: BTreeMap::new(),
            cached_equip_stats: Stats::default(),
            equip_stats_applied: false,
            equip_cache_valid: false,
            cooldown_configs: Vec::new(),
            hp_percent_for_defensives: 0.0,
            initial_major_cooldowns: Vec::new(),
            item_proc_buffs: Vec::new(),
            cooldown_buff_auras: Vec::new(),
            party_index: player_index,
            spirit_regen_base: 0.0,
            spirit_regen_per_spirit: 0.0,
            rotation_transformations: 0,
            player: player.clone(),
            spell_registration_handlers: Vec::new(),
            hardcast_avoidance_aura: None,
            item_swap: None,
        }));
        let id = self.add_unit(unit);
        if let Some(cooldowns) = player.message("cooldowns") {
            let character = self.character_mut(id);
            character.hp_percent_for_defensives = cooldowns.f64("hp_percent_for_defensives");
            character.cooldown_configs = cooldowns
                .messages("cooldowns")
                .into_iter()
                .filter(|config| config.has("id"))
                .cloned()
                .collect();
        }
        let gcd = self.new_timer(id);
        let rotation = self.new_timer(id);
        self.unit_mut(id).gcd = Some(gcd);
        self.unit_mut(id).rotation_timer = Some(rotation);

        self.add_stats(id, &base_stats);
        add_character_universal_stat_dependencies(self, id);

        if let Some(bonus) = player.message("bonus_stats") {
            // Go: a stats array makes the bonus FromUnitStatsProto, stats and percent pseudo stats.
            if bonus.has("stats") {
                let stats = Stats::from_proto_array(&bonus.f64s("stats"))
                    .add(&items::stats_from_pseudo_stats(&bonus.f64s("pseudo_stats")));
                self.character_mut(id).bonus_stats = stats;
            }
            // Go sets weapon damage and school hit from the pseudo stats; zeros, which the UI
            // sends by default, change nothing.
            if bonus.f64s("pseudo_stats").iter().any(|value| *value != 0.0) {
                return Err(Refusal::new(
                    "bonus_stats",
                    "bonus pseudo stats are unsupported".to_string(),
                ));
            }
        }
        let off_hand_is_shield = {
            let character = self.character(id);
            let off_hand = &character.equipment[items::slot::OFF_HAND];
            !off_hand.is_empty() && off_hand.weapon_type == "WeaponTypeShield"
        };
        if off_hand_is_shield {
            self.unit_mut(id).pseudo_stats.can_block = true;
        }
        self.unit_mut(id).pseudo_stats.in_front_of_target = player.bool("in_front_of_target");
        if player.bool("enable_item_swap") {
            if let Some(item_swap) = player.message("item_swap") {
                self.enable_item_swap(id, item_swap, area_types)?;
            }
        }
        let cached = self.compute_equip_stats(id);
        let character = self.character_mut(id);
        character.cached_equip_stats = cached;
        character.equip_cache_valid = true;
        if player.enum_number("weapon_type_override") != 0 {
            return Err(Refusal::new(
                "weapon_type_override",
                "weapon type overrides are unsupported".to_string(),
            ));
        }
        self.update_weapon_skills(id);
        Ok(id)
    }

    fn compute_equip_stats(&self, unit: UnitId) -> Stats {
        let character = self.character(unit);
        items::equipment_stats(&character.equipment).add(&character.bonus_stats)
    }

    /// Go `updateCachedEquipStats`.
    fn update_cached_equip_stats(&mut self, unit: UnitId) {
        if !self.character(unit).equip_cache_valid {
            let cached = self.compute_equip_stats(unit);
            let character = self.character_mut(unit);
            character.cached_equip_stats = cached;
            character.equip_cache_valid = true;
        }
    }

    /// Go `EquipStats`.
    pub(crate) fn equip_stats(&mut self, unit: UnitId) -> Stats {
        self.update_cached_equip_stats(unit);
        let character = self.character(unit);
        let mut out = character.cached_equip_stats;
        for (stat, multiplier) in &character.item_stat_multipliers {
            out.0[*stat] *= multiplier;
        }
        out
    }

    fn apply_equip_scaling_internal(&mut self, unit: UnitId, stat: Stat, multiplier: f64) -> f64 {
        self.update_cached_equip_stats(unit);
        let character = self.character_mut(unit);
        let old = character
            .item_stat_multipliers
            .get(&(stat as usize))
            .copied()
            .unwrap_or(1.0);
        let new = old * multiplier;
        character.item_stat_multipliers.insert(stat as usize, new);
        // Go fuses newMultiplier - oldMultiplier into old*multiplier - old (character.go 240).
        character.cached_equip_stats[stat] * old.mul_add(multiplier, -old)
    }

    /// Go `ApplyEquipScaling`.
    pub(crate) fn apply_equip_scaling(&mut self, unit: UnitId, stat: Stat, multiplier: f64) {
        let diff = self.apply_equip_scaling_internal(unit, stat, multiplier);
        if self.character(unit).equip_stats_applied {
            self.add_stat(unit, stat, diff);
        }
    }

    /// Go `ApplyDynamicEquipScaling`.
    pub(crate) fn apply_dynamic_equip_scaling(
        &mut self,
        unit: UnitId,
        stat: Stat,
        multiplier: f64,
    ) {
        if self.measuring_stats && !self.is_finalized() {
            self.apply_equip_scaling(unit, stat, multiplier);
        } else {
            let diff = self.apply_equip_scaling_internal(unit, stat, multiplier);
            self.add_stat_dynamic(unit, stat, diff);
        }
    }

    /// Go `applyEquipment`.
    pub(crate) fn apply_equipment(&mut self, unit: UnitId) {
        assert!(
            !self.character(unit).equip_stats_applied,
            "Equipment stats already applied to character!"
        );
        let stats = self.equip_stats(unit);
        self.add_stats(unit, &stats);
        self.character_mut(unit).equip_stats_applied = true;
    }

    /// Go `applyBuildPhaseAuras`.
    pub(crate) fn apply_build_phase_auras(&mut self, unit: UnitId, phase: BuildPhase) {
        self.measuring_stats = true;
        for aura in self.unit(unit).auras.clone() {
            if self.aura(aura).build_phase.matches(phase) {
                self.activate(aura);
            }
        }
        self.measuring_stats = false;
    }

    /// Go `clearBuildPhaseAuras`.
    pub(crate) fn clear_build_phase_auras(&mut self, unit: UnitId, phase: BuildPhase) {
        self.measuring_stats = true;
        for aura in self.unit(unit).auras.clone() {
            if self.aura(aura).build_phase.matches(phase) {
                self.deactivate(aura);
            }
        }
        self.measuring_stats = false;
    }

    /// Go `Character.EnableManaBar`.
    pub(crate) fn enable_mana_bar(&mut self, unit: UnitId) {
        if self.unit(unit).unit_type == UnitType::Pet {
            return self.enable_pet_mana_bar(unit);
        }
        if self.unit(unit).unit_type == UnitType::Player {
            let class = self.character(unit).class.clone();
            let crit_per_int = crate::data::tables::tables()
                .crit_per_int_max_level
                .get(&class)
                .copied()
                .unwrap_or(0.0);
            self.unit_mut(unit).sdm.add_stat_dependency(
                Stat::Intellect,
                Stat::SpellCritPercent,
                crit_per_int,
            );
            self.add_stat(unit, Stat::Mana, 20.0 - 15.0 * 20.0);
            self.unit_mut(unit)
                .sdm
                .add_stat_dependency(Stat::Intellect, Stat::Mana, 15.0);
        }
        self.register_spell(
            unit,
            super::spell::SpellConfig {
                action_id: ActionId {
                    other_id: "OtherActionManaGain".to_string(),
                    ..ActionId::default()
                },
                ..Default::default()
            },
        );
        let class = self.character(unit).class.clone();
        let (base, per_spirit) = if class == "ClassPriest" || class == "ClassMage" {
            (6.25, 1.0 / 8.0)
        } else {
            (7.5, 1.0 / 10.0)
        };
        let base_mana = self.character(unit).base_stats[Stat::Mana];
        let character = self.character_mut(unit);
        character.spirit_regen_base = base;
        character.spirit_regen_per_spirit = per_spirit;
        let u = self.unit_mut(unit);
        u.mana_bar.enabled = true;
        u.mana_bar.base_mana = base_mana;
        u.current_power_bar = super::sim::PowerBar::Mana;
    }

    /// Go `CritPerAgiMaxLevel[class]`.
    pub(crate) fn crit_per_agi_max_level(&self, unit: UnitId) -> f64 {
        crate::data::tables::tables()
            .crit_per_agi_max_level
            .get(&self.character(unit).class)
            .copied()
            .unwrap_or(0.0)
    }

    /// Go `AddMajorCooldown`.
    pub(crate) fn add_major_cooldown(&mut self, unit: UnitId, mut mcd: MajorCooldown) {
        assert!(
            !self.is_finalized(),
            "Major cooldowns may not be added once finalized!"
        );
        use super::spell::SpellFlag;
        // item_swaps.go RegisterActive: an item's use follows the item.
        let item_id = self.spell(mcd.spell).action_id.item_id;
        if item_id != 0 {
            let slots = super::shared_items::eligible_slots_for_item(self, unit, item_id);
            self.register_item_swap_callback(
                unit,
                &slots,
                super::item_swap::SwapCallback::ItemUse {
                    item_id,
                    slots: slots.clone(),
                },
            );
        }
        let spell = self.spell_mut(mcd.spell);
        spell.flags |= SpellFlag::APL | SpellFlag::MCD;
        let effective = spell.default_cast.effective_time();
        if (mcd.cooldown_type & cooldown_type::SURVIVAL != 0 && effective == 0)
            || mcd.allow_spell_queueing
        {
            spell.flags |= SpellFlag::REACTIVE;
        }
        mcd.timings = Vec::new();
        self.character_mut(unit).initial_major_cooldowns.push(mcd);
    }

    /// Go `majorCooldownManager.finalize`: the configured timings.
    pub(crate) fn finalize_major_cooldowns(&mut self, unit: UnitId) {
        let configs = self.character(unit).cooldown_configs.clone();
        let mcds = self.character(unit).initial_major_cooldowns.clone();
        let mut out = Vec::new();
        for mut mcd in mcds {
            mcd.timings = Vec::new();
            let action = self.spell(mcd.spell).action_id.clone();
            for config in &configs {
                let id = super::agent::proto_to_action_id(config.message("id").expect("filtered"));
                if id == action {
                    mcd.timings = config
                        .f64s("timings")
                        .into_iter()
                        .map(super::sim::seconds)
                        .collect();
                    break;
                }
            }
            out.push(mcd);
        }
        self.character_mut(unit).initial_major_cooldowns = out;
    }

    /// Go `removeInitialMajorCooldown`.
    pub(crate) fn remove_initial_major_cooldown(&mut self, unit: UnitId, action: &ActionId) {
        let mcds = self.character(unit).initial_major_cooldowns.clone();
        let Some(position) = mcds
            .iter()
            .position(|mcd| &self.spell(mcd.spell).action_id == action)
        else {
            return;
        };
        let removed_timer = self.spell(mcds[position].spell).cd.timer;
        let mut mcds = mcds;
        mcds.remove(position);
        if let Some(timer) = removed_timer {
            let mut i = mcds.len();
            while i > 0 {
                i -= 1;
                let spell = self.spell(mcds[i].spell);
                if spell.cd.timer == Some(timer) && spell.action_id.same_action_ignore_tag(action) {
                    mcds.remove(i);
                }
            }
        }
        self.character_mut(unit).initial_major_cooldowns = mcds;
    }

    /// Go `Character.GetOffensiveTrinketCD`.
    pub(crate) fn get_offensive_trinket_cd(&mut self, unit: UnitId) -> TimerId {
        self.category_timer(unit, 1141)
    }

    /// Go `Character.GetConjuredCD`, the timer the mana gems share. The reference gives it
    /// category 30, which no Forever item has: Forever's client (1.60.1.70532, ItemEffect)
    /// puts every mana gem in spell category 1153, with the healthstones, Demonic and Dark Rune
    /// and Thistle Tea, so under Forever rules the gems share the timer of the conjured items
    /// consumes register (README, Rust-only mechanics).
    pub(crate) fn get_conjured_cd(&mut self, unit: UnitId) -> TimerId {
        match self.rules {
            Rules::Forever => self.category_timer(unit, CONJURED_CATEGORY),
            Rules::GoReference => self.category_timer(unit, 30),
        }
    }

    /// Go `Character.GetPotionCD`.
    pub(crate) fn get_potion_cd(&mut self, unit: UnitId) -> TimerId {
        self.category_timer(unit, 4)
    }

    /// Go `Character.GetMHWeapon` and friends.
    pub(crate) fn mh_weapon(&self, unit: UnitId) -> Option<&items::Item> {
        let item = &self.character(unit).equipment[items::slot::MAIN_HAND];
        (!item.is_empty()).then_some(item)
    }

    pub(crate) fn oh_weapon(&self, unit: UnitId) -> Option<&items::Item> {
        let item = &self.character(unit).equipment[items::slot::OFF_HAND];
        (!item.is_empty()
            && item.weapon_type != "WeaponTypeShield"
            && item.weapon_type != "WeaponTypeOffHand")
            .then_some(item)
    }

    pub(crate) fn ranged_weapon(&self, unit: UnitId) -> Option<&items::Item> {
        let item = &self.character(unit).equipment[items::slot::RANGED];
        (!item.is_empty()
            && item.ranged_weapon_type != "RangedWeaponTypeIdol"
            && item.ranged_weapon_type != "RangedWeaponTypeLibram"
            && item.ranged_weapon_type != "RangedWeaponTypeTotem"
            && item.ranged_weapon_type != "RangedWeaponTypeSigil")
            .then_some(item)
    }

    /// Go `HasTrinketEquipped`.
    pub(crate) fn has_trinket_equipped(&self, unit: UnitId, item: i32) -> bool {
        let equipment = &self.character(unit).equipment;
        equipment[items::slot::TRINKET1].id == item || equipment[items::slot::TRINKET2].id == item
    }

    /// Registers a never-expiring aura active from the gear build phase: Go's status auras.
    pub(crate) fn register_permanent_gear_aura(&mut self, unit: UnitId, label: String) -> AuraId {
        let aura = self.get_or_register_aura(
            unit,
            AuraConfig {
                label,
                build_phase: BuildPhase::GEAR,
                duration: super::sim::NEVER_EXPIRES,
                ..Default::default()
            },
        );
        self.make_permanent(aura)
    }

    /// Go `OnSpellRegistered`.
    pub(crate) fn on_spell_registered(
        &mut self,
        unit: UnitId,
        handler: super::spell::SpellRegisteredHandler,
    ) {
        for spell in self.unit(unit).spellbook.clone() {
            handler(self, spell);
        }
        if self.unit(unit).character.is_some() {
            self.character_mut(unit)
                .spell_registration_handlers
                .push(Rc::clone(&handler));
        } else {
            self.unit_mut(unit)
                .spell_registration_handlers
                .push(handler);
        }
    }

    pub(crate) fn spell_registered_handlers(
        &self,
        unit: UnitId,
    ) -> Vec<super::spell::SpellRegisteredHandler> {
        let mut handlers = self.unit(unit).spell_registration_handlers.clone();
        if let Some(character) = self.unit(unit).character.as_deref() {
            handlers.extend(character.spell_registration_handlers.iter().cloned());
        }
        handlers
    }
}

/// The pseudo stats a fresh player has.
pub(crate) fn fresh_pseudo_stats() -> PseudoStats {
    PseudoStats::new()
}

#[cfg(test)]
mod warlock_l20_actor_source_tests {
    use super::*;

    #[test]
    fn troll_warlock_l20_has_actual_level_bound_resources_and_native_racial_projection() {
        let actor = base_stats_at_level("ClassWarlock", "RaceTroll", 20).unwrap();
        assert_eq!(actor[Stat::Health], 204.0);
        assert_eq!(actor[Stat::Mana], 383.0);
        assert_eq!(actor[Stat::Strength], 27.0);
        assert_eq!(actor[Stat::Agility], 30.0);
        assert_eq!(actor[Stat::Stamina], 33.0);
        assert_eq!(actor[Stat::Intellect], 42.0);
        assert_eq!(actor[Stat::Spirit], 47.0);
        assert!(base_stats_at_level("ClassWarlock", "RaceTroll", 19).is_some());
        assert!(base_stats_at_level("ClassWarlock", "RaceTroll", 29).is_some());
        assert!(base_stats_at_level("ClassWarlock", "RaceOrc", 20).is_none());
        assert!(base_stats_at_level("ClassMage", "RaceTroll", 20).is_none());
        let old = base_stats("ClassWarlock", "RaceTroll").unwrap();
        let unchanged = base_stats_at_level("ClassWarlock", "RaceTroll", 60).unwrap();
        assert_eq!(old[Stat::Health], unchanged[Stat::Health]);
        assert_eq!(old[Stat::Mana], unchanged[Stat::Mana]);
    }

    #[test]
    fn sourced_l30_troll_warlock_uses_actual_class_resources_and_l30_orc_projection() {
        let actor = base_stats_at_level("ClassWarlock", "RaceTroll", 30).unwrap();
        assert_eq!(actor[Stat::Health], 379.0);
        assert_eq!(actor[Stat::Mana], 638.0);
        assert_eq!(actor[Stat::Strength], 31.0);
        assert_eq!(actor[Stat::Agility], 35.0);
        assert_eq!(actor[Stat::Stamina], 41.0);
        assert_eq!(actor[Stat::Intellect], 57.0);
        assert_eq!(actor[Stat::Spirit], 61.0);
        assert!(base_stats_at_level("ClassWarlock", "RaceOrc", 30).is_none());
        assert!(base_stats_at_level("ClassMage", "RaceTroll", 30).is_none());
        assert!(base_stats_at_level("ClassWarlock", "RaceTroll", 31).is_some());
    }

    #[test]
    fn all_classic_source_levels_use_one_shared_native_actor_curve() {
        for level in 1..60 {
            let actor = base_stats_at_level("ClassWarlock", "RaceTroll", level)
                .expect("every imported source Warlock L1-59 row exists");
            assert!(actor[Stat::Health] > 0.0, "L{level}");
            assert!(actor[Stat::Mana] > 0.0, "L{level}");
        }
        assert!(base_stats_at_level("ClassWarlock", "RaceTroll", 61).is_none());
    }
}
