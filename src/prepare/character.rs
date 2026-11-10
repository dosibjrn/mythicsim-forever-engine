//! Go sim/core/character.go: a player built from its request, with equipment, base stats,
//! build phases and major cooldowns.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::OnceLock;

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
    // These are max-level Go constants. Never apply them to the sourced L20 Priest actor.
    if sim.unit(unit).level != CHARACTER_LEVEL {
        return;
    }
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

const LEVEL_DEPENDENT_RATING_STATS: [Stat; 11] = [
    Stat::MeleeHitRating,
    Stat::SpellHitRating,
    Stat::MeleeCritRating,
    Stat::SpellCritRating,
    Stat::MeleeHasteRating,
    Stat::SpellHasteRating,
    Stat::ExpertiseRating,
    Stat::DefenseRating,
    Stat::BlockRating,
    Stat::DodgeRating,
    Stat::ParryRating,
];

fn has_level_dependent_rating_stats(stats: &Stats) -> bool {
    LEVEL_DEPENDENT_RATING_STATS
        .into_iter()
        .any(|stat| stats[stat] != 0.0)
}

/// The imported Go item stat array uses rating-named indices as transport for
/// Forever's direct hit/crit percentages. The existing Forever equipment
/// unification uses these exact Go unit constants regardless of actor level.
/// For a lower-level actor, move those bonuses to explicit percent stats rather
/// than applying max-level stat dependencies or refusing a real item.
pub(crate) fn normalize_forever_hit_crit_item_percents(stats: &mut Stats, level: i32) {
    if level == constants::CHARACTER_LEVEL {
        return;
    }
    use constants::*;
    for (rating, percent, per_percent) in [
        (
            Stat::MeleeHitRating,
            Stat::PhysicalHitPercent,
            PHYSICAL_HIT_RATING_PER_HIT_PERCENT,
        ),
        (
            Stat::SpellHitRating,
            Stat::SpellHitPercent,
            SPELL_HIT_RATING_PER_HIT_PERCENT,
        ),
        (
            Stat::MeleeCritRating,
            Stat::PhysicalCritPercent,
            PHYSICAL_CRIT_RATING_PER_CRIT_PERCENT,
        ),
        (
            Stat::SpellCritRating,
            Stat::SpellCritPercent,
            SPELL_CRIT_RATING_PER_CRIT_PERCENT,
        ),
    ] {
        let amount = stats[rating] / per_percent;
        stats[percent] += amount;
        stats[rating] = 0.0;
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

/// Legacy requests omit level and remain level 60. Sourced lower-level actors
/// use CMaNGOS base stats and their level-specific class conversions.
fn requested_player_level(player: &Message, actor_level: Option<i32>) -> Result<i32, Refusal> {
    // Rust also accepts a Forever-only request-level scalar (field 1001),
    // outside the Go descriptor. Preserve existing native callers; a separately
    // supplied CLI actor level must agree with that scalar when present.
    let embedded = player.i32("level");
    if let Some(external) = actor_level {
        if embedded != 0 && embedded != external {
            return Err(Refusal::new(
                "level",
                format!("native actor level {external} conflicts with request level {embedded}"),
            ));
        }
    }
    let level = actor_level.unwrap_or(if embedded == 0 {
        constants::CHARACTER_LEVEL
    } else {
        embedded
    });
    if level == constants::CHARACTER_LEVEL {
        return Ok(level);
    }
    let class = player.enum_name("class");
    let race = player.enum_name("race");
    if level != constants::CHARACTER_LEVEL && base_stats_at_level(&class, &race, level).is_some() {
        return Ok(level);
    }
    Err(Refusal::new(
        "level",
        format!(
            "player level {level} is unsupported: no source-backed native preparation for {race} {class} (supported: level 60, Priest/Rogue L20 and Druid L20/L30 for sourced Classic races, or Troll Warlock L20)"
        ),
    ))
}

#[derive(serde::Deserialize)]
struct PriestL20CritSource {
    spell_base_percent: f64,
    spell_intellect_rate0: f64,
    spell_intellect_rate_per_level: f64,
    melee_agility_rate_level_1: f64,
    melee_agility_rate_level_60: f64,
}

#[derive(serde::Deserialize)]
struct PriestL20Source {
    class: String,
    level: i32,
    base_health: f64,
    base_mana: f64,
    crit: PriestL20CritSource,
    races: BTreeMap<String, BTreeMap<String, f64>>,
}

fn priest_l20_source() -> &'static PriestL20Source {
    static SOURCE: OnceLock<PriestL20Source> = OnceLock::new();
    SOURCE.get_or_init(|| {
        let parsed: PriestL20Source =
            serde_json::from_str(include_str!("../../data/priest-l20-base-stats.json"))
                .expect("Priest L20 source rows and conversions parse");
        assert_eq!(parsed.class, "ClassPriest");
        assert_eq!(parsed.level, 20);
        parsed
    })
}

/// CMaNGOS GetSpellCritFromIntellect: base + intellect / (rate0 + rate1 * level).
fn priest_l20_crit_per_int() -> f64 {
    let crit = &priest_l20_source().crit;
    1.0 / (crit.spell_intellect_rate0
        + crit.spell_intellect_rate_per_level * priest_l20_source().level as f64)
}

/// CMaNGOS GetMeleeCritFromAgility: agility / the level-interpolated class rate.
fn priest_l20_crit_per_agi() -> f64 {
    let source = priest_l20_source();
    let level = source.level as f64;
    let crit = &source.crit;
    let agility_rate = crit.melee_agility_rate_level_1 * (60.0 - level) / 59.0
        + crit.melee_agility_rate_level_60 * (level - 1.0) / 59.0;
    1.0 / agility_rate
}

/// Source-backed Priest L20 primary stats, class resources and base spell crit.
fn priest_l20_base_stats(race: &str) -> Option<Stats> {
    let source = priest_l20_source();
    let values = source.races.get(race)?;
    let mut out = Stats::default();
    for (name, value) in values {
        out[Stat::by_name(name)?] = *value;
    }
    out[Stat::Health] = source.base_health;
    out[Stat::Mana] = source.base_mana;
    out[Stat::SpellCritPercent] = source.crit.spell_base_percent;
    Some(out)
}

#[derive(serde::Deserialize)]
struct RogueL20CritSource {
    melee_agility_rate_level_1: f64,
    melee_agility_rate_level_60: f64,
}

#[derive(serde::Deserialize)]
struct RogueL20Source {
    class: String,
    level: i32,
    base_health: f64,
    crit: RogueL20CritSource,
    races: BTreeMap<String, BTreeMap<String, f64>>,
}

fn rogue_l20_source() -> &'static RogueL20Source {
    static SOURCE: OnceLock<RogueL20Source> = OnceLock::new();
    SOURCE.get_or_init(|| {
        let parsed: RogueL20Source =
            serde_json::from_str(include_str!("../../data/rogue-l20-base-stats.json"))
                .expect("Rogue L20 sourced attributes and conversion parse");
        assert_eq!(parsed.class, "ClassRogue");
        assert_eq!(parsed.level, 20);
        parsed
    })
}

/// CMaNGOS Player::GetMeleeCritFromAgility, using the Rogue's level-interpolated rate.
fn rogue_l20_crit_per_agi() -> f64 {
    let source = rogue_l20_source();
    let level = source.level as f64;
    let crit = &source.crit;
    let agility_rate = crit.melee_agility_rate_level_1 * (60.0 - level) / 59.0
        + crit.melee_agility_rate_level_60 * (level - 1.0) / 59.0;
    1.0 / agility_rate
}

/// Source-backed Rogue L20 primary attributes and class HP (no mana bar).
fn rogue_l20_base_stats(race: &str) -> Option<Stats> {
    let source = rogue_l20_source();
    let values = source.races.get(race)?;
    let mut out = Stats::default();
    for (name, value) in values {
        out[Stat::by_name(name)?] = *value;
    }
    out[Stat::Health] = source.base_health;
    Some(out)
}

#[derive(serde::Deserialize)]
struct DruidLevelCritSource {
    spell_base_percent: f64,
    spell_intellect_rate0: f64,
    spell_intellect_rate_per_level: f64,
    melee_agility_rate_level_1: f64,
    melee_agility_rate_level_60: f64,
}

#[derive(serde::Deserialize)]
struct DruidLevelSource {
    class: String,
    level: i32,
    base_health: f64,
    base_mana: f64,
    crit: DruidLevelCritSource,
    races: BTreeMap<String, BTreeMap<String, f64>>,
}

fn druid_l20_source() -> &'static DruidLevelSource {
    static SOURCE: OnceLock<DruidLevelSource> = OnceLock::new();
    SOURCE.get_or_init(|| {
        let parsed: DruidLevelSource =
            serde_json::from_str(include_str!("../../data/druid-l20-base-stats.json"))
                .expect("Druid L20 source attributes and conversions parse");
        assert_eq!(parsed.class, "ClassDruid");
        assert_eq!(parsed.level, 20);
        parsed
    })
}

fn druid_l30_source() -> &'static DruidLevelSource {
    static SOURCE: OnceLock<DruidLevelSource> = OnceLock::new();
    SOURCE.get_or_init(|| {
        let parsed: DruidLevelSource =
            serde_json::from_str(include_str!("../../data/druid-l30-base-stats.json"))
                .expect("Druid L30 source attributes and conversions parse");
        assert_eq!(parsed.class, "ClassDruid");
        assert_eq!(parsed.level, 30);
        parsed
    })
}

fn druid_level_source(level: i32) -> Option<&'static DruidLevelSource> {
    match level {
        20 => Some(druid_l20_source()),
        30 => Some(druid_l30_source()),
        _ => None,
    }
}

fn druid_crit_per_int_at(level: i32) -> f64 {
    let source = druid_level_source(level).expect("supported Druid source level");
    let crit = &source.crit;
    1.0 / (crit.spell_intellect_rate0 + crit.spell_intellect_rate_per_level * source.level as f64)
}

fn druid_crit_per_agi_at(level: i32) -> f64 {
    let source = druid_level_source(level).expect("supported Druid source level");
    let crit = &source.crit;
    let level = source.level as f64;
    let agility_rate = crit.melee_agility_rate_level_1 * (60.0 - level) / 59.0
        + crit.melee_agility_rate_level_60 * (level - 1.0) / 59.0;
    1.0 / agility_rate
}

fn druid_base_stats(race: &str, level: i32) -> Option<Stats> {
    let source = druid_level_source(level)?;
    let values = source.races.get(race)?;
    let mut out = Stats::default();
    for (name, value) in values {
        out[Stat::by_name(name)?] = *value;
    }
    out[Stat::Health] = source.base_health;
    out[Stat::Mana] = source.base_mana;
    out[Stat::SpellCritPercent] = source.crit.spell_base_percent;
    Some(out)
}

/// Go `BaseStats[{race, class}]` for L60; CMaNGOS lower-level rows otherwise.
fn base_stats_for_level(class: &str, race: &str, level: i32) -> Option<Stats> {
    if level == 20 || level == 30 {
        if class == "ClassDruid" {
            return druid_base_stats(race, level);
        }
        if level == 20 {
            return match class {
                "ClassPriest" => priest_l20_base_stats(race),
                "ClassRogue" => rogue_l20_base_stats(race),
                _ => None,
            };
        }
    }
    if level != constants::CHARACTER_LEVEL {
        return None;
    }
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

/// Preserve the original level-60 Go base table for existing callers and the
/// source-backed Troll L20 race projection.
fn base_stats(class: &str, race: &str) -> Option<Stats> {
    base_stats_for_level(class, race, constants::CHARACTER_LEVEL)
}

/// Source-backed level-20 Warlock base resources with a transparent Troll race projection.
/// The historical Classic SQL has no Troll/Warlock row. Only the racial primary-stat
/// delta is projected from this exact native engine's Troll and Orc L60 tables;
/// the Orc L20 primaries and class L20 base resources come from pinned CMaNGOS rows.
/// This does not invent an independently certified Troll source row.
fn base_stats_at_level(class: &str, race: &str, level: i32) -> Option<Stats> {
    if level == constants::CHARACTER_LEVEL {
        return base_stats(class, race);
    }
    if (class, race, level) != ("ClassWarlock", "RaceTroll", 20) {
        return base_stats_for_level(class, race, level);
    }
    let source: serde_json::Value =
        serde_json::from_str(include_str!("../../data/warlock-l20-actor-source.json")).ok()?;
    if source["class"].as_str()? != class
        || source["race"].as_str()? != race
        || source["level"].as_i64()? != i64::from(level)
        || source["reference_race_l20"]["race"].as_str()? != "RaceOrc"
    {
        return None;
    }
    let mut troll = base_stats(class, race)?;
    let orc = base_stats(class, "RaceOrc")?;
    let source_orc = &source["reference_race_l20"];
    for (key, stat) in [
        ("strength", Stat::Strength),
        ("agility", Stat::Agility),
        ("stamina", Stat::Stamina),
        ("intellect", Stat::Intellect),
        ("spirit", Stat::Spirit),
    ] {
        troll[stat] = source_orc[key].as_f64()? + troll[stat] - orc[stat];
    }
    troll[Stat::Health] = source["class_base"]["health"].as_f64()?;
    troll[Stat::Mana] = source["class_base"]["mana"].as_f64()?;
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
        let level = requested_player_level(player, actor_level)?;
        let index = party_index * 5 + player_index;
        let name = player.str("name").to_string();
        let mut unit = Unit::new(UnitType::Player, format!("{} (#{})", name, index + 1));
        unit.index = index;
        unit.level = level;
        unit.reaction_time = Duration::from(player.i32("reaction_time_ms").max(10)) * MILLISECOND;
        unit.channel_clip_delay =
            (Duration::from(player.i32("channel_clip_delay_ms")) * MILLISECOND).max(0);
        unit.start_distance_from_target = player.f64("distance_from_target");
        unit.distance_from_target = player.f64("distance_from_target");
        let race = player.enum_name("race");
        let class = player.enum_name("class");
        let equipment = items::equipment(player.message("equipment"), area_types)?;
        let base_stats = base_stats_at_level(&class, &race, level)
            .ok_or_else(|| Refusal::new("race", format!("no base stats for {race} {class}")))?;
        unit.character = Some(Box::new(Character {
            name,
            race,
            class: class.clone(),
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
        if level != constants::CHARACTER_LEVEL && has_level_dependent_rating_stats(&cached) {
            return Err(Refusal::new(
                "level",
                format!(
                    "player level {level} {class} loadout contains combat-rating stats but no level-{level} rating conversion source is available"
                ),
            ));
        }
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
        // Only the imported Forever *equipment* indices denote direct hit/crit
        // percentages. Explicit player bonus_stats are not item source data:
        // retain their rating fields so native preparation can refuse them
        // when the actor level has no sourced rating conversion.
        let mut item_stats = items::equipment_stats(&character.equipment);
        normalize_forever_hit_crit_item_percents(&mut item_stats, self.unit(unit).level);
        item_stats.add(&character.bonus_stats)
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
            let crit_per_int = self.crit_per_int(unit);
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

    /// Go `CritPerIntMaxLevel[class]`, with sourced lower-level class conversions.
    pub(crate) fn crit_per_int(&self, unit: UnitId) -> f64 {
        let level = self.unit(unit).level;
        if level == 20 && self.character(unit).class == "ClassPriest" {
            return priest_l20_crit_per_int();
        }
        if (level == 20 || level == 30) && self.character(unit).class == "ClassDruid" {
            return druid_crit_per_int_at(level);
        }
        crate::data::tables::tables()
            .crit_per_int_max_level
            .get(&self.character(unit).class)
            .copied()
            .unwrap_or(0.0)
    }

    /// Level-aware agility-to-crit conversion for sourced lower-level actors.
    pub(crate) fn crit_per_agi(&self, unit: UnitId) -> f64 {
        let level = self.unit(unit).level;
        if (level == 20 || level == 30) && self.character(unit).class == "ClassDruid" {
            return druid_crit_per_agi_at(level);
        }
        if level == 20 {
            return match self.character(unit).class.as_str() {
                "ClassPriest" => priest_l20_crit_per_agi(),
                "ClassRogue" => rogue_l20_crit_per_agi(),
                _ => 0.0,
            };
        }
        self.crit_per_agi_max_level(unit)
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
mod level_contract_tests {
    use super::{
        constants, druid_crit_per_agi_at, druid_crit_per_int_at, has_level_dependent_rating_stats,
        priest_l20_crit_per_agi, priest_l20_crit_per_int, requested_player_level,
        rogue_l20_crit_per_agi, Sim,
    };
    use crate::contracts::request::Message;
    use crate::prepare::stats::{Stat, Stats};

    #[test]
    fn omitted_and_explicit_60_keep_legacy_level() {
        let player = Message::from_json_text("proto.Player", "{}").unwrap();
        assert_eq!(
            requested_player_level(&player, None).unwrap(),
            constants::CHARACTER_LEVEL
        );
        assert_eq!(
            requested_player_level(&player, Some(60)).unwrap(),
            constants::CHARACTER_LEVEL
        );
        let extended = Message::from_json_text(
            "proto.Player",
            r#"{"class":"ClassDruid","race":"RaceNightElf","level":20}"#,
        )
        .unwrap();
        assert_eq!(requested_player_level(&extended, None).unwrap(), 20);
        assert_eq!(requested_player_level(&extended, Some(20)).unwrap(), 20);
        assert_eq!(
            requested_player_level(&extended, Some(60))
                .unwrap_err()
                .code,
            "level"
        );
    }

    #[test]
    fn only_sourced_priest_classic_races_accept_level_20() {
        let priest = Message::from_json_text(
            "proto.Player",
            r#"{"class":"ClassPriest","race":"RaceDwarf"}"#,
        )
        .unwrap();
        assert_eq!(requested_player_level(&priest, Some(20)).unwrap(), 20);

        for (json, level) in [
            (r#"{}"#, 20),
            (r#"{"class":"ClassMage","race":"RaceHuman"}"#, 20),
            (r#"{"class":"ClassPriest","race":"RaceGnome"}"#, 20),
            (r#"{"class":"ClassPriest","race":"RaceDwarf"}"#, 19),
            (r#"{"class":"ClassPriest","race":"RaceDwarf"}"#, -1),
        ] {
            let player = Message::from_json_text("proto.Player", json).unwrap();
            let refusal = requested_player_level(&player, Some(level)).unwrap_err();
            assert_eq!(refusal.code, "level");
        }
    }

    #[test]
    fn rogue_l20_prepares_with_sourced_health_primary_stats_and_agility_conversion() {
        let player = Message::from_json_text(
            "proto.Player",
            r#"{"class":"ClassRogue","race":"RaceHuman"}"#,
        )
        .unwrap();
        assert_eq!(requested_player_level(&player, Some(20)).unwrap(), 20);
        let mut sim = Sim::new();
        let unit = sim
            .new_character_at_level(0, 0, &player, &[], Some(20))
            .unwrap();
        assert_eq!(sim.unit(unit).level, 20);
        let stats = sim.character(unit).base_stats;
        assert_eq!(stats[Stat::Strength], 37.0);
        assert_eq!(stats[Stat::Agility], 53.0);
        assert_eq!(stats[Stat::Stamina], 35.0);
        assert_eq!(stats[Stat::Intellect], 23.0);
        assert_eq!(stats[Stat::Spirit], 28.0);
        assert_eq!(stats[Stat::Health], 233.0);
        assert_eq!(stats[Stat::Mana], 0.0);
        assert_eq!(sim.crit_per_agi(unit), rogue_l20_crit_per_agi());
        assert_ne!(sim.crit_per_agi(unit), sim.crit_per_agi_max_level(unit));
        assert_eq!(
            sim.unit(unit)
                .sdm
                .stat_dependency_coeff(Stat::MeleeCritRating, Stat::PhysicalCritPercent),
            0.0
        );
    }

    #[test]
    fn rogue_l20_unsupported_race_and_level_fail_closed() {
        for (json, level) in [
            (r#"{"class":"ClassRogue","race":"RaceBloodElf"}"#, 20),
            (r#"{"class":"ClassRogue","race":"RaceHuman"}"#, 19),
            (r#"{"class":"ClassRogue","race":"RaceHuman"}"#, 30),
        ] {
            let player = Message::from_json_text("proto.Player", json).unwrap();
            assert_eq!(
                requested_player_level(&player, Some(level))
                    .unwrap_err()
                    .code,
                "level"
            );
        }
    }

    #[test]
    fn priest_l20_is_prepared_as_a_source_backed_native_actor() {
        let player = Message::from_json_text(
            "proto.Player",
            r#"{"class":"ClassPriest","race":"RaceDwarf"}"#,
        )
        .unwrap();
        let mut sim = Sim::new();
        let unit = sim
            .new_character_at_level(0, 0, &player, &[], Some(20))
            .unwrap();

        assert_eq!(sim.unit(unit).level, 20);
        let base = sim.character(unit).base_stats;
        assert_eq!(base[Stat::Strength], 25.0);
        assert_eq!(base[Stat::Agility], 21.0);
        assert_eq!(base[Stat::Stamina], 31.0);
        assert_eq!(base[Stat::Intellect], 48.0);
        assert_eq!(base[Stat::Spirit], 50.0);
        assert_eq!(base[Stat::Health], 237.0);
        assert_eq!(base[Stat::Mana], 437.0);
        assert_eq!(base[Stat::SpellCritPercent], 2.97);
        assert_eq!(base[Stat::PhysicalCritPercent], 0.0);

        // L20 never inherits the pinned level-60 combat-rating conversions.
        assert_eq!(
            sim.unit(unit)
                .sdm
                .stat_dependency_coeff(Stat::MeleeHitRating, Stat::PhysicalHitPercent),
            0.0
        );
        sim.enable_mana_bar(unit);
        let crit_per_int = priest_l20_crit_per_int();
        assert_eq!(
            sim.unit(unit)
                .sdm
                .stat_dependency_coeff(Stat::Intellect, Stat::SpellCritPercent),
            crit_per_int
        );
        assert_eq!(
            sim.unit(unit)
                .sdm
                .stat_dependency_coeff(Stat::Intellect, Stat::Mana),
            15.0
        );
        assert_eq!(
            sim.unit(unit)
                .sdm
                .stat_dependency_coeff(Stat::Stamina, Stat::Health),
            10.0
        );

        // This is the same class-specific dependency installed by NewPriest.
        let crit_per_agi = sim.crit_per_agi(unit);
        assert_eq!(crit_per_agi, priest_l20_crit_per_agi());
        sim.unit_mut(unit).sdm.add_stat_dependency(
            Stat::Agility,
            Stat::PhysicalCritPercent,
            crit_per_agi,
        );

        let raw = sim.unit(unit).stats;
        let applied = sim.unit_mut(unit).sdm.sort_and_apply_stat_dependencies(raw);
        assert_eq!(applied[Stat::Health], 367.0);
        assert_eq!(applied[Stat::Mana], 877.0);
        assert!((applied[Stat::SpellCritPercent] - (2.97 + 48.0 * crit_per_int)).abs() < 1e-10);
        assert!((applied[Stat::PhysicalCritPercent] - 21.0 * crit_per_agi).abs() < 1e-10);
    }

    #[test]
    fn druid_l20_uses_source_actor_stats_and_class_crit_conversions() {
        for (race, expected_primary) in [
            ("RaceNightElf", [30.0, 35.0, 32.0, 43.0, 46.0]),
            ("RaceTauren", [37.0, 26.0, 35.0, 39.0, 48.0]),
        ] {
            let player = Message::from_json_text(
                "proto.Player",
                &format!(r#"{{"class":"ClassDruid","race":"{race}"}}"#),
            )
            .unwrap();
            assert_eq!(requested_player_level(&player, Some(20)).unwrap(), 20);
            let mut sim = Sim::new();
            let unit = sim
                .new_character_at_level(0, 0, &player, &[], Some(20))
                .unwrap();
            assert_eq!(sim.unit(unit).level, 20);
            let stats = sim.character(unit).base_stats;
            for (stat, value) in [
                Stat::Strength,
                Stat::Agility,
                Stat::Stamina,
                Stat::Intellect,
                Stat::Spirit,
            ]
            .into_iter()
            .zip(expected_primary)
            {
                assert_eq!(stats[stat], value);
            }
            assert_eq!(stats[Stat::Health], 243.0);
            assert_eq!(stats[Stat::Mana], 354.0);
            assert_eq!(stats[Stat::SpellCritPercent], 3.33);
            assert_eq!(sim.crit_per_int(unit), druid_crit_per_int_at(20));
            assert_eq!(sim.crit_per_agi(unit), druid_crit_per_agi_at(20));
            assert_ne!(sim.crit_per_agi(unit), sim.crit_per_agi_max_level(unit));
            sim.enable_mana_bar(unit);
            assert_eq!(
                sim.unit(unit)
                    .sdm
                    .stat_dependency_coeff(Stat::Intellect, Stat::SpellCritPercent),
                druid_crit_per_int_at(20)
            );
        }
    }

    #[test]
    fn druid_l30_prepares_sourced_night_elf_and_tauren_actors() {
        for (race, expected_primary) in [
            ("RaceNightElf", [37.0, 42.0, 41.0, 56.0, 61.0]),
            ("RaceTauren", [44.0, 32.0, 44.0, 52.0, 63.0]),
        ] {
            let player = Message::from_json_text(
                "proto.Player",
                &format!(r#"{{"class":"ClassDruid","race":"{race}","level":30}}"#),
            )
            .unwrap();
            assert_eq!(requested_player_level(&player, Some(30)).unwrap(), 30);
            let mut sim = Sim::new();
            let unit = sim
                .new_character_at_level(0, 0, &player, &[], Some(30))
                .unwrap();
            assert_eq!(sim.unit(unit).level, 30);
            let stats = sim.character(unit).base_stats;
            for (stat, value) in [
                Stat::Strength,
                Stat::Agility,
                Stat::Stamina,
                Stat::Intellect,
                Stat::Spirit,
            ]
            .into_iter()
            .zip(expected_primary)
            {
                assert_eq!(stats[stat], value);
            }
            assert_eq!(stats[Stat::Health], 418.0);
            assert_eq!(stats[Stat::Mana], 614.0);
            assert_eq!(stats[Stat::SpellCritPercent], 3.33);
            assert_eq!(sim.crit_per_int(unit), druid_crit_per_int_at(30));
            assert_eq!(sim.crit_per_agi(unit), druid_crit_per_agi_at(30));
            assert_ne!(sim.crit_per_agi(unit), sim.crit_per_agi_max_level(unit));
            sim.enable_mana_bar(unit);
            assert_eq!(
                sim.unit(unit)
                    .sdm
                    .stat_dependency_coeff(Stat::Intellect, Stat::SpellCritPercent),
                druid_crit_per_int_at(30)
            );
        }
    }

    #[test]
    fn druid_l20_unsupported_races_and_levels_refuse_in_native() {
        for (json, level) in [
            (r#"{"class":"ClassDruid","race":"RaceHuman"}"#, 20),
            (r#"{"class":"ClassDruid","race":"RaceNightElf"}"#, 19),
            (r#"{"class":"ClassDruid","race":"RaceTauren"}"#, 31),
        ] {
            let player = Message::from_json_text("proto.Player", json).unwrap();
            assert_eq!(
                requested_player_level(&player, Some(level))
                    .unwrap_err()
                    .code,
                "level"
            );
        }
    }

    #[test]
    fn level_20_rating_stats_are_detected_instead_of_using_level_60_rates() {
        let mut stats = Stats::default();
        assert!(!has_level_dependent_rating_stats(&stats));
        stats[Stat::SpellHitRating] = 1.0;
        assert!(has_level_dependent_rating_stats(&stats));
    }
}

#[cfg(test)]
mod warlock_l20_actor_source_tests {
    use super::*;

    #[test]
    fn l20_warlock_does_not_inherit_unsourced_level_60_rating_conversions() {
        let player = Message::from_json_text(
            "proto.Player",
            r#"{"class":"ClassWarlock","race":"RaceTroll"}"#,
        )
        .unwrap();
        let mut sim = Sim::new();
        let unit = sim
            .new_character_at_level(0, 0, &player, &[], Some(20))
            .unwrap();
        assert_eq!(sim.unit(unit).level, 20);
        assert_eq!(
            sim.unit(unit)
                .sdm
                .stat_dependency_coeff(Stat::SpellHitRating, Stat::SpellHitPercent,),
            0.0,
        );
        let mut max_level = Sim::new();
        let max_unit = max_level
            .new_character_at_level(0, 0, &player, &[], Some(60))
            .unwrap();
        assert!(
            max_level
                .unit(max_unit)
                .sdm
                .stat_dependency_coeff(Stat::SpellHitRating, Stat::SpellHitPercent,)
                > 0.0,
            "the existing sourced level-60 conversion must remain available",
        );
        let mut equipment = Stats::default();
        assert!(!has_level_dependent_rating_stats(&equipment));
        equipment[Stat::SpellHitRating] = 1.0;
        assert!(has_level_dependent_rating_stats(&equipment));
    }

    #[test]
    fn forever_l20_hit_crit_fields_are_direct_percent_bonuses() {
        let mut stats = Stats::default();
        stats[Stat::SpellHitRating] = constants::SPELL_HIT_RATING_PER_HIT_PERCENT * 2.0;
        stats[Stat::SpellCritRating] = constants::SPELL_CRIT_RATING_PER_CRIT_PERCENT * 3.0;
        stats[Stat::SpellHitPercent] = 0.5;
        normalize_forever_hit_crit_item_percents(&mut stats, 20);
        assert_eq!(stats[Stat::SpellHitPercent], 2.5);
        assert_eq!(stats[Stat::SpellCritPercent], 3.0);
        assert_eq!(stats[Stat::SpellHitRating], 0.0);
        assert_eq!(stats[Stat::SpellCritRating], 0.0);
        assert!(!has_level_dependent_rating_stats(&stats));

        let mut max_level = Stats::default();
        max_level[Stat::SpellHitRating] = constants::SPELL_HIT_RATING_PER_HIT_PERCENT;
        normalize_forever_hit_crit_item_percents(&mut max_level, 60);
        assert_eq!(
            max_level[Stat::SpellHitRating],
            constants::SPELL_HIT_RATING_PER_HIT_PERCENT
        );
        assert_eq!(max_level[Stat::SpellHitPercent], 0.0);
    }

    #[test]
    fn player_bonus_rating_is_not_silently_converted_to_item_hit_percent_at_l20() {
        // The imported item index is a direct Forever percentage, but a
        // separately requested player bonus rating is not an equipped item.
        // Without a sourced L20 conversion it must still receive the existing
        // native 'level' refusal, even alongside an accepted hit chest.
        let mut bonus = vec![0.0; Stats::default().0.len()];
        bonus[Stat::SpellHitRating as usize] = constants::SPELL_HIT_RATING_PER_HIT_PERCENT;
        let request = serde_json::json!({
            "class": "ClassWarlock",
            "race": "RaceTroll",
            "equipment": {"items": [{"id": 20033}]},
            "bonus_stats": {"stats": bonus}
        })
        .to_string();
        let player = Message::from_json_text("proto.Player", &request).unwrap();
        let mut sim = Sim::new();
        let refusal = sim
            .new_character_at_level(0, 0, &player, &[], Some(20))
            .unwrap_err();
        assert_eq!(refusal.code, "level");
        assert!(refusal.reason.contains("combat-rating stats"));
    }

    #[test]
    fn real_forever_crit_and_hit_chests_prepare_at_l20_with_percent_stats() {
        for (item_id, request, relevant_percent) in [
            (
                21183,
                r#"{"class":"ClassWarlock","race":"RaceTroll","equipment":{"items":[{"id":21183}]}}"#,
                Stat::SpellCritPercent,
            ),
            (
                20033,
                r#"{"class":"ClassWarlock","race":"RaceTroll","equipment":{"items":[{"id":20033}]}}"#,
                Stat::SpellHitPercent,
            ),
        ] {
            let player = Message::from_json_text("proto.Player", request).unwrap();
            let mut sim = Sim::new();
            let unit = sim
                .new_character_at_level(0, 0, &player, &[], Some(20))
                .unwrap_or_else(|refusal| {
                    panic!("Forever item {item_id} must prepare at L20: {refusal:?}")
                });
            let stats = sim.equip_stats(unit);
            assert!(
                stats[relevant_percent] > 0.0,
                "item {item_id} must retain native direct % bonus"
            );
            assert_eq!(stats[Stat::SpellHitRating], 0.0);
            assert_eq!(stats[Stat::SpellCritRating], 0.0);
        }
    }

    #[test]
    fn explicit_warlock_l20_actor_level_preserves_default_l60_and_refuses_unsourced_levels() {
        let player = Message::from_json_text(
            "proto.Player",
            r#"{"class":"ClassWarlock","race":"RaceTroll"}"#,
        )
        .unwrap();
        assert_eq!(requested_player_level(&player, None).unwrap(), 60);
        assert_eq!(requested_player_level(&player, Some(20)).unwrap(), 20);
        assert_eq!(requested_player_level(&player, Some(60)).unwrap(), 60);
        let unsupported = requested_player_level(&player, Some(19)).unwrap_err();
        assert_eq!(unsupported.code, "level");
        assert!(unsupported.reason.contains("Troll Warlock L20"));

        // The selected engine accepts a Rust-only embedded level. It must not
        // silently prepare a source L20 request at the legacy L60 default.
        let embedded = Message::from_json_text(
            "proto.Player",
            r#"{"class":"ClassWarlock","race":"RaceTroll","level":20}"#,
        )
        .unwrap();
        assert_eq!(requested_player_level(&embedded, None).unwrap(), 20);
        assert_eq!(requested_player_level(&embedded, Some(20)).unwrap(), 20);
        assert_eq!(
            requested_player_level(&embedded, Some(60))
                .unwrap_err()
                .code,
            "level",
        );
        assert_eq!(
            requested_player_level(&embedded, Some(19))
                .unwrap_err()
                .code,
            "level",
        );
        let mut embedded_sim = Sim::new();
        let embedded_unit = embedded_sim.new_character(0, 0, &embedded, &[]).unwrap();
        assert_eq!(embedded_sim.unit(embedded_unit).level, 20);
        assert_eq!(
            embedded_sim.character(embedded_unit).base_stats[Stat::Health],
            204.0
        );

        let mut sim = Sim::new();
        let unit = sim
            .new_character_at_level(0, 0, &player, &[], Some(20))
            .unwrap();
        assert_eq!(sim.unit(unit).level, 20);
        assert_eq!(sim.character(unit).base_stats[Stat::Health], 204.0);

        let mut default_sim = Sim::new();
        let default_unit = default_sim.new_character(0, 0, &player, &[]).unwrap();
        assert_eq!(default_sim.unit(default_unit).level, 60);
        assert_eq!(
            default_sim.character(default_unit).base_stats[Stat::Health],
            base_stats("ClassWarlock", "RaceTroll").unwrap()[Stat::Health],
        );
    }

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
        assert!(base_stats_at_level("ClassWarlock", "RaceTroll", 19).is_none());
        assert!(base_stats_at_level("ClassWarlock", "RaceOrc", 20).is_none());
        assert!(base_stats_at_level("ClassMage", "RaceTroll", 20).is_none());
        let old = base_stats("ClassWarlock", "RaceTroll").unwrap();
        let unchanged = base_stats_at_level("ClassWarlock", "RaceTroll", 60).unwrap();
        assert_eq!(old[Stat::Health], unchanged[Stat::Health]);
        assert_eq!(old[Stat::Mana], unchanged[Stat::Mana]);
    }
}
