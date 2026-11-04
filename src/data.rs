//! Game data imported from the pinned Go reference by `tools/rust_data.py`.
//!
//! Each table is a JSON Lines file under `data/`, one row per line sorted by its key, with the
//! key first. A table is indexed on first use by reading only each line's key, and a row is
//! parsed only when asked for, so preparing a request touches the few rows it needs.

pub(crate) mod spells;
pub(crate) mod tables;

use std::collections::HashMap;
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
        let row: ItemRequiredLevelRow = serde_json::from_str(line)
            .unwrap_or_else(|error| panic!("data/item-required-levels.jsonl:{}: {error}", line_number + 1));
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

/// The native ItemSparse RequiredLevel field, keyed by item ID.
pub(crate) fn item_required_levels() -> &'static HashMap<i32, i32> {
    static LEVELS: OnceLock<HashMap<i32, i32>> = OnceLock::new();
    LEVELS.get_or_init(|| parse_item_required_levels(ITEM_REQUIRED_LEVELS))
}

#[cfg(test)]
mod tests {
    use super::*;

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
