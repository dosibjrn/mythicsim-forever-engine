//! Game data imported from the pinned Go reference by `tools/rust_data.py`.
//!
//! Each table is a JSON Lines file under `data/`, one row per line sorted by its key, with the
//! key first. A table is indexed on first use by reading only each line's key, and a row is
//! parsed only when asked for, so preparing a request touches the few rows it needs.

pub(crate) mod spells;
pub(crate) mod tables;

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::contracts::request::Message;

/// A JSON Lines table indexed by the integer key that starts each line.
pub(crate) struct Table {
    name: &'static str,
    text: &'static str,
    index: OnceLock<Vec<(i64, usize, usize)>>,
}

impl Table {
    pub(crate) const fn new(name: &'static str, text: &'static str) -> Table {
        Table {
            name,
            text,
            index: OnceLock::new(),
        }
    }

    fn index(&self) -> &[(i64, usize, usize)] {
        self.index.get_or_init(|| {
            let mut index = Vec::new();
            let mut start = 0;
            for line in self.text.split_inclusive('\n') {
                let end = start + line.trim_end_matches('\n').len();
                if end > start {
                    let key = leading_key(&self.text[start..end])
                        .unwrap_or_else(|| panic!("data/{}: a row without a key", self.name));
                    index.push((key, start, end));
                }
                start += line.len();
            }
            assert!(
                index.windows(2).all(|pair| pair[0].0 < pair[1].0),
                "data/{} is not sorted by its key",
                self.name
            );
            index
        })
    }

    /// The row with this key, as its JSON text.
    pub(crate) fn row(&self, key: i64) -> Option<&'static str> {
        let index = self.index();
        let at = index.binary_search_by_key(&key, |entry| entry.0).ok()?;
        let (_, start, end) = index[at];
        Some(&self.text[start..end])
    }

    /// Every key, in order.
    pub(crate) fn keys(&self) -> impl Iterator<Item = i64> + '_ {
        self.index().iter().map(|entry| entry.0)
    }
}

/// The integer value of a line's first member, `{"key":123,...`.
fn leading_key(line: &str) -> Option<i64> {
    let rest = line.strip_prefix("{\"")?;
    let rest = &rest[rest.find("\":")? + 2..];
    let end = rest.find([',', '}'])?;
    rest[..end].parse().ok()
}

macro_rules! table {
    ($name:ident, $file:literal) => {
        pub(crate) static $name: Table =
            Table::new($file, include_str!(concat!("../data/", $file)));
    };
}

table!(ITEMS, "items.jsonl");
table!(RANDOM_SUFFIXES, "random-suffixes.jsonl");
table!(ENCHANTS, "enchants.jsonl");
table!(RAND_PROP_POINTS, "rand-prop-points.jsonl");
table!(CONSUMABLES, "consumables.jsonl");
table!(SPELL_EFFECTS, "spell-effects.jsonl");
table!(SPELL_ROWS, "spells.jsonl");

/// A database row as its proto message, such as a `proto.SimItem` from [`ITEMS`].
/// The decoded rows read so far, by table, message type and key.
type RowCache = HashMap<(&'static str, &'static str, i64), Option<std::rc::Rc<Message>>>;

pub(crate) fn proto_row(
    table: &Table,
    type_name: &'static str,
    key: i64,
) -> Option<std::rc::Rc<Message>> {
    // Preparation reads the same rows for every reset simulation it builds; each row is read
    // once per thread.
    thread_local! {
        static ROWS: std::cell::RefCell<RowCache> =
            std::cell::RefCell::new(HashMap::new());
    }
    ROWS.with(|rows| {
        rows.borrow_mut()
            .entry((table.name, type_name, key))
            .or_insert_with(|| {
                let text = table.row(key)?;
                Some(std::rc::Rc::new(
                    Message::from_json_text(type_name, text)
                        .unwrap_or_else(|err| panic!("data/{} row {key}: {err}", table.name)),
                ))
            })
            .clone()
    })
}

/// Required character levels projected from the pinned client's ItemSparse DB2 table.
const ITEM_REQUIRED_LEVELS: &str = include_str!("../data/item-required-levels.jsonl");

#[derive(serde::Deserialize)]
struct ItemRequiredLevelRow {
    id: i32,
    required_level: i32,
}

fn parse_item_required_levels(text: &str) -> HashMap<i32, i32> {
    let mut levels = HashMap::new();
    for (line_number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let row: ItemRequiredLevelRow = serde_json::from_str(line).unwrap_or_else(|error| {
            panic!(
                "data/item-required-levels.jsonl:{}: {error}",
                line_number + 1
            )
        });
        assert!(
            row.required_level >= 0,
            "data/item-required-levels.jsonl:{}: negative RequiredLevel for item {}",
            line_number + 1,
            row.id
        );
        assert!(
            levels.insert(row.id, row.required_level).is_none(),
            "data/item-required-levels.jsonl:{}: duplicate item ID {}",
            line_number + 1,
            row.id
        );
    }
    levels
}

/// The pinned client's class-wide Shaman equipment proficiencies.
const SHAMAN_EQUIPMENT_SKILLS: &str = include_str!("../data/shaman-equipment-skills.json");

#[derive(serde::Deserialize)]
struct ShamanSkillLineName {
    skill_id: i32,
    name: String,
}

#[derive(serde::Deserialize)]
struct ShamanEquipmentSkillRow {
    skill_id: i32,
    name: String,
    min_level: Option<i32>,
    race_unrestricted: bool,
}

#[derive(serde::Deserialize)]
struct ShamanEquipmentSkillsFile {
    schema_version: i32,
    client_build: String,
    class: String,
    class_id: i32,
    class_mask: i32,
    race_policy: String,
    skill_line_ids: Vec<i32>,
    skill_line_names: Vec<ShamanSkillLineName>,
    skills: Vec<ShamanEquipmentSkillRow>,
}

pub(crate) struct ShamanEquipmentSkills {
    skill_levels: HashMap<String, Option<i32>>,
    skill_line_names: HashSet<String>,
}

impl ShamanEquipmentSkills {
    pub(crate) fn has_skill_line(&self, name: &str) -> bool {
        self.skill_line_names.contains(name)
    }

    pub(crate) fn min_level(&self, name: &str) -> Option<i32> {
        self.skill_levels.get(name).copied().flatten()
    }
}

fn parse_shaman_equipment_skills(text: &str) -> Result<ShamanEquipmentSkills, ()> {
    let source: ShamanEquipmentSkillsFile = serde_json::from_str(text).map_err(|_| ())?;
    if source.schema_version != 2
        || source.client_build != crate::CLIENT_BUILD
        || source.class != "shaman"
        || source.class_id != 7
        || source.class_mask != 64
        || source.race_policy
            != "class-wide only; SkillRaceClassInfo race mask must be unrestricted"
        || source.skills.is_empty()
    {
        return Err(());
    }

    let mut skill_line_name_by_id = HashMap::new();
    let mut skill_line_names = HashSet::new();
    for line in source.skill_line_names {
        if line.skill_id <= 0
            || line.name.trim().is_empty()
            || skill_line_name_by_id
                .insert(line.skill_id, line.name.clone())
                .is_some()
        {
            return Err(());
        }
        skill_line_names.insert(line.name);
    }
    let mut sorted_skill_line_ids = source.skill_line_ids.clone();
    sorted_skill_line_ids.sort_unstable();
    sorted_skill_line_ids.dedup();
    if sorted_skill_line_ids != source.skill_line_ids
        || sorted_skill_line_ids.len() != skill_line_name_by_id.len()
        || sorted_skill_line_ids
            .iter()
            .any(|skill_id| !skill_line_name_by_id.contains_key(skill_id))
    {
        return Err(());
    }

    const REQUIRED_SKILL_LINE_NAMES: &[&str] = &[
        "Axes",
        "Swords",
        "Maces",
        "Daggers",
        "Fist Weapons",
        "Two-Handed Axes",
        "Two-Handed Swords",
        "Two-Handed Maces",
        "Polearms",
        "Staves",
        "Bows",
        "Guns",
        "Crossbows",
        "Thrown",
        "Cloth",
        "Leather",
        "Mail",
        "Plate Mail",
        "Shield",
    ];
    if REQUIRED_SKILL_LINE_NAMES
        .iter()
        .any(|name| !skill_line_names.contains(*name))
    {
        return Err(());
    }

    let mut skill_ids = HashMap::new();
    let mut skill_levels = HashMap::new();
    for row in source.skills {
        if row.skill_id <= 0
            || row.name.trim().is_empty()
            || row
                .min_level
                .is_some_and(|level| !(1..=60).contains(&level))
            || row.min_level.is_some() != row.race_unrestricted
            || skill_ids.insert(row.skill_id, ()).is_some()
        {
            return Err(());
        }
        if let Some(existing) = skill_levels.insert(row.name, row.min_level) {
            if existing != row.min_level {
                return Err(());
            }
        }
    }
    Ok(ShamanEquipmentSkills {
        skill_levels,
        skill_line_names,
    })
}

pub(crate) fn shaman_equipment_skills() -> Result<&'static ShamanEquipmentSkills, &'static str> {
    static SKILLS: OnceLock<Result<ShamanEquipmentSkills, ()>> = OnceLock::new();
    match SKILLS.get_or_init(|| parse_shaman_equipment_skills(SHAMAN_EQUIPMENT_SKILLS)) {
        Ok(skills) => Ok(skills),
        Err(()) => Err("the pinned Shaman SkillLine/SkillRaceClassInfo projection is missing, invalid, or for another client build"),
    }
}

const PALADIN_EQUIPMENT_SKILLS: &str = include_str!("../data/paladin-equipment-skills.json");
const ROGUE_EQUIPMENT_SKILLS: &str = include_str!("../data/rogue-equipment-skills.json");
const DRUID_EQUIPMENT_SKILLS: &str = include_str!("../data/druid-equipment-skills.json");
const WARLOCK_EQUIPMENT_SKILLS: &str = include_str!("../data/warlock-equipment-skills.json");

#[derive(serde::Deserialize)]
struct PaladinSkillLineName {
    skill_id: i32,
    name: String,
}

#[derive(serde::Deserialize)]
struct PaladinEquipmentSkillRow {
    skill_id: i32,
    name: String,
    min_level: Option<i32>,
    race_unrestricted: bool,
}

#[derive(serde::Deserialize)]
struct PaladinEquipmentSkillsFile {
    schema_version: i32,
    client_build: String,
    class: String,
    class_id: i32,
    class_mask: i32,
    race_policy: String,
    skill_line_ids: Vec<i32>,
    skill_line_names: Vec<PaladinSkillLineName>,
    skills: Vec<PaladinEquipmentSkillRow>,
}

pub(crate) struct PaladinEquipmentSkills {
    skill_levels: HashMap<String, Option<i32>>,
    skill_line_names: HashSet<String>,
}

impl PaladinEquipmentSkills {
    pub(crate) fn has_skill_line(&self, name: &str) -> bool {
        self.skill_line_names.contains(name)
    }

    pub(crate) fn min_level(&self, name: &str) -> Option<i32> {
        self.skill_levels.get(name).copied().flatten()
    }
}

fn parse_class_equipment_skills(
    text: &str,
    class: &str,
    class_id: i32,
) -> Result<PaladinEquipmentSkills, ()> {
    let source: PaladinEquipmentSkillsFile = serde_json::from_str(text).map_err(|_| ())?;
    if source.schema_version != 2
        || source.client_build != crate::CLIENT_BUILD
        || source.class != class
        || source.class_id != class_id
        || source.class_mask != (1 << (class_id - 1))
        || source.race_policy
            != "class-wide only; SkillRaceClassInfo race mask must be unrestricted"
        || source.skills.is_empty()
    {
        return Err(());
    }

    let mut skill_line_name_by_id = HashMap::new();
    let mut skill_line_names = HashSet::new();
    for line in source.skill_line_names {
        if line.skill_id <= 0
            || line.name.trim().is_empty()
            || skill_line_name_by_id
                .insert(line.skill_id, line.name.clone())
                .is_some()
        {
            return Err(());
        }
        skill_line_names.insert(line.name);
    }
    let mut sorted_skill_line_ids = source.skill_line_ids.clone();
    sorted_skill_line_ids.sort_unstable();
    sorted_skill_line_ids.dedup();
    if sorted_skill_line_ids != source.skill_line_ids
        || sorted_skill_line_ids.len() != skill_line_name_by_id.len()
        || sorted_skill_line_ids
            .iter()
            .any(|skill_id| !skill_line_name_by_id.contains_key(skill_id))
    {
        return Err(());
    }

    const REQUIRED_SKILL_LINE_NAMES: &[&str] = &[
        "Axes",
        "Swords",
        "Maces",
        "Daggers",
        "Fist Weapons",
        "Two-Handed Axes",
        "Two-Handed Swords",
        "Two-Handed Maces",
        "Polearms",
        "Staves",
        "Bows",
        "Guns",
        "Crossbows",
        "Thrown",
        "Cloth",
        "Leather",
        "Mail",
        "Plate Mail",
        "Shield",
    ];
    if REQUIRED_SKILL_LINE_NAMES
        .iter()
        .any(|name| !skill_line_names.contains(*name))
    {
        return Err(());
    }

    let mut skill_ids = HashMap::new();
    let mut skill_levels = HashMap::new();
    for row in source.skills {
        if row.skill_id <= 0
            || row.name.trim().is_empty()
            || row
                .min_level
                .is_some_and(|level| !(1..=60).contains(&level))
            || row.min_level.is_some() != row.race_unrestricted
            || skill_ids.insert(row.skill_id, ()).is_some()
        {
            return Err(());
        }
        if let Some(existing) = skill_levels.insert(row.name, row.min_level) {
            if existing != row.min_level {
                return Err(());
            }
        }
    }
    Ok(PaladinEquipmentSkills {
        skill_levels,
        skill_line_names,
    })
}

fn parse_paladin_equipment_skills(text: &str) -> Result<PaladinEquipmentSkills, ()> {
    parse_class_equipment_skills(text, "paladin", 2)
}

fn parse_rogue_equipment_skills(text: &str) -> Result<PaladinEquipmentSkills, ()> {
    parse_class_equipment_skills(text, "rogue", 4)
}

fn parse_druid_equipment_skills(text: &str) -> Result<PaladinEquipmentSkills, ()> {
    parse_class_equipment_skills(text, "druid", 11)
}

fn parse_warlock_equipment_skills(text: &str) -> Result<PaladinEquipmentSkills, ()> {
    parse_class_equipment_skills(text, "warlock", 9)
}

pub(crate) fn paladin_equipment_skills() -> Result<&'static PaladinEquipmentSkills, &'static str> {
    static SKILLS: OnceLock<Result<PaladinEquipmentSkills, ()>> = OnceLock::new();
    match SKILLS.get_or_init(|| parse_paladin_equipment_skills(PALADIN_EQUIPMENT_SKILLS)) {
        Ok(skills) => Ok(skills),
        Err(()) => Err("the pinned Paladin SkillLine/SkillRaceClassInfo projection is missing, invalid, or for another client build"),
    }
}

/// The same validated client SkillLine/SkillRaceClassInfo schema, scoped to Rogue mask 8.
pub(crate) fn rogue_equipment_skills() -> Result<&'static PaladinEquipmentSkills, &'static str> {
    static SKILLS: OnceLock<Result<PaladinEquipmentSkills, ()>> = OnceLock::new();
    match SKILLS.get_or_init(|| parse_rogue_equipment_skills(ROGUE_EQUIPMENT_SKILLS)) {
        Ok(skills) => Ok(skills),
        Err(()) => Err("the pinned Rogue SkillLine/SkillRaceClassInfo projection is missing, invalid, or for another client build"),
    }
}

/// The same pinned client class-wide skill projection, scoped to Druid mask 1024.
pub(crate) fn druid_equipment_skills() -> Result<&'static PaladinEquipmentSkills, &'static str> {
    static SKILLS: OnceLock<Result<PaladinEquipmentSkills, ()>> = OnceLock::new();
    match SKILLS.get_or_init(|| parse_druid_equipment_skills(DRUID_EQUIPMENT_SKILLS)) {
        Ok(skills) => Ok(skills),
        Err(()) => Err("the pinned Druid SkillLine/SkillRaceClassInfo projection is missing, invalid, or for another client build"),
    }
}

/// The pinned client-wide Warlock class-skill projection; never use another
/// caster's proficiency as a fallback.
pub(crate) fn warlock_equipment_skills() -> Result<&'static PaladinEquipmentSkills, &'static str> {
    static SKILLS: OnceLock<Result<PaladinEquipmentSkills, ()>> = OnceLock::new();
    match SKILLS.get_or_init(|| parse_warlock_equipment_skills(WARLOCK_EQUIPMENT_SKILLS)) {
        Ok(skills) => Ok(skills),
        Err(()) => Err("the pinned Warlock SkillLine/SkillRaceClassInfo projection is missing, invalid, or for another client build"),
    }
}

/// The native ItemSparse RequiredLevel field, keyed by item ID.
pub(crate) fn item_required_levels() -> &'static HashMap<i32, i32> {
    static LEVELS: OnceLock<HashMap<i32, i32>> = OnceLock::new();
    LEVELS.get_or_init(|| parse_item_required_levels(ITEM_REQUIRED_LEVELS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pinned_shaman_equipment_skills_and_keeps_race_restricted_skills_unavailable() {
        let skills = shaman_equipment_skills().expect("pinned Shaman equipment source");
        assert_eq!(skills.min_level("Axes"), Some(1));
        assert_eq!(skills.min_level("Two-Handed Axes"), Some(1));
        assert_eq!(skills.min_level("Leather"), Some(1));
        assert_eq!(skills.min_level("Mail"), Some(40));
        assert_eq!(skills.min_level("Maces"), Some(1));
        assert_eq!(skills.min_level("Staves"), Some(1));
        assert_eq!(skills.min_level("Daggers"), None);
        assert_eq!(skills.min_level("Two-Handed Maces"), None);
        assert!(skills.has_skill_line("Swords"));
        assert!(skills.has_skill_line("Plate Mail"));
        assert_eq!(skills.min_level("Swords"), None);
    }

    #[test]
    fn reads_pinned_paladin_equipment_skills_and_keeps_plate_level_gated() {
        let skills = paladin_equipment_skills().expect("pinned Paladin equipment source");
        assert_eq!(skills.min_level("Maces"), Some(1));
        assert_eq!(skills.min_level("Mail"), Some(1));
        assert_eq!(skills.min_level("Plate Mail"), Some(40));
        assert!(skills.has_skill_line("Axes"));
        assert!(skills.has_skill_line("Shield"));
    }

    #[test]
    fn reads_source_generated_rogue_equipment_projection() {
        let skills = rogue_equipment_skills().expect("pinned Rogue equipment source");
        assert_eq!(skills.min_level("Daggers"), Some(1));
        assert_eq!(skills.min_level("Leather"), Some(1));
        assert_eq!(skills.min_level("Cloth"), Some(1));
        assert_eq!(skills.min_level("Swords"), None); // race-restricted DB2 rows
        assert_eq!(skills.min_level("Maces"), None);
        assert!(skills.has_skill_line("Plate Mail"));
        let source: serde_json::Value =
            serde_json::from_str(ROGUE_EQUIPMENT_SKILLS).expect("checked-in DB2 projection");
        assert_eq!(source["class_mask"], 8);
        assert_eq!(source["client_build"], crate::CLIENT_BUILD);
        assert!(parse_class_equipment_skills(ROGUE_EQUIPMENT_SKILLS, "paladin", 2).is_err());
    }

    #[test]
    fn rejects_a_shaman_projection_with_a_missing_required_skill_line_name() {
        let mut source: serde_json::Value =
            serde_json::from_str(SHAMAN_EQUIPMENT_SKILLS).expect("valid pinned projection");
        source["skill_line_names"]
            .as_array_mut()
            .expect("skill line index")
            .retain(|line| line["name"] != "Axes");
        let source = serde_json::to_string(&source).expect("serialized test projection");
        assert!(parse_shaman_equipment_skills(&source).is_err());
    }

    #[test]
    fn finds_rows_by_key() {
        let item = proto_row(&ITEMS, "proto.SimItem", 647).expect("Destiny");
        assert_eq!(item.str("name"), "Destiny");
        assert!(proto_row(&ITEMS, "proto.SimItem", 1).is_none());
        let enchant = proto_row(&ENCHANTS, "proto.SimEnchant", 15).expect("Light Armor Kit");
        assert_eq!(enchant.str("name"), "Light Armor Kit");
        assert!(ITEMS.keys().count() > 10_000);
    }

    #[test]
    fn parses_native_item_required_levels_without_inferring_from_item_level() {
        let levels = parse_item_required_levels(concat!(
            r#"{"id":25,"required_level":1}"#,
            "\n",
            r#"{"id":647,"required_level":52}"#,
            "\n",
        ));
        assert_eq!(levels.get(&25), Some(&1));
        assert_eq!(levels.get(&647), Some(&52));
        assert_eq!(parse_item_required_levels("\n").len(), 0);
    }

    #[test]
    #[should_panic(expected = "duplicate item ID")]
    fn rejects_duplicate_native_item_required_levels() {
        parse_item_required_levels(concat!(
            r#"{"id":25,"required_level":1}"#,
            "\n",
            r#"{"id":25,"required_level":2}"#,
            "\n",
        ));
    }

    #[test]
    fn reads_a_leading_key() {
        assert_eq!(leading_key(r#"{"id":25,"name":"x"}"#), Some(25));
        assert_eq!(leading_key(r#"{"ilvl":-3}"#), Some(-3));
        assert_eq!(leading_key(r#"{"name":"x"}"#), None);
    }
}
