//! Class ranks taught by the Ahn'Qiraj 20 books, which Forever's rules leave out.
//!
//! Each rank listed is taught in client 1.60.1.70707 only by its book (ItemEffect trigger 6:
//! items 21214, 21279 to 21300, 21306 and 21307, all "Requires Level 60" raid drops from the
//! Ruins of Ahn'Qiraj), and no item teaches the rank below, which comes from the trainer. The
//! client carries every Classic raid, so it cannot say when Ahn'Qiraj opens; the sim models a
//! character without raid drops, so under Forever's rules a ladder's highest rank skips the
//! book ranks and a request that names one (a rotation written against the old top rank) is
//! read as the rank below. The Go reference keeps them, as the pinned exporter does.
//!
//! Deadly Poison V (book 21302) is no ladder: the rogue export states rank IV's tick under
//! Forever's rules. Multi-Shot V (book 21304) has no row in Forever, whose Multi-Shot has one
//! rank. Feint V (book 21303) is never simmed.

use crate::prepare::sim::Rules;

/// Each book rank with the rank a player knows without the book. Triggered spells and buff
/// auras a book rank owns are listed too, so a rotation or buff naming them follows the cast.
pub(crate) const BOOK_RANKS: &[(i32, i32)] = &[
    // Warrior: Heroic Strike IX, Revenge VI, Battle Shout VII.
    (25286, 11567),
    (25288, 11601),
    (25289, 11551),
    // Paladin: Blessing of Wisdom VI, Blessing of Might VII, Holy Light IX.
    (25290, 19854),
    (25291, 19838),
    (25292, 10329),
    // Hunter: Serpent Sting IX, Aspect of the Hawk VII.
    (25295, 13555),
    (25296, 14322),
    // Druid: Healing Touch XI, Starfire VII, Rejuvenation XI.
    (25297, 9889),
    (25298, 9876),
    (25299, 9841),
    // Rogue: Backstab IX.
    (25300, 11281),
    // Mage: Frostbolt XI, Fireball XII, Arcane Missiles VIII and its missile.
    (25304, 10181),
    (25306, 10151),
    (25345, 10212),
    (25346, 10274),
    // Warlock: Shadow Bolt X, Immolate VIII, Corruption VII.
    (25307, 11661),
    (25309, 11668),
    (25311, 11672),
    // Priest: Greater Heal V, Renew X, Prayer of Healing V.
    (25314, 10965),
    (25315, 10929),
    (25316, 10961),
    // Shaman: Healing Wave X, Grace of Air Totem III and its aura, Strength of Earth Totem V
    // and its aura.
    (25357, 10396),
    (25359, 10627),
    (25360, 10626),
    (25361, 10442),
    (25362, 10441),
];

/// The file `render_json` writes: what the app checks its player-facing data against, so a
/// page cannot link or name a rank the sim no longer casts. A test fails while it is stale.
pub(crate) const JSON_PATH: &str = "release/book-ranks.json";

/// [`BOOK_RANKS`] as the JSON artifact at [`JSON_PATH`]: each book rank with the rank below and
/// both ranks' names, in table order.
pub(crate) fn render_json() -> String {
    use crate::prepare::spelldata::store::must_find;
    let ranks: Vec<serde_json::Value> = BOOK_RANKS
        .iter()
        .map(|&(book, below)| {
            let (book_spell, below_spell) = (must_find(book), must_find(below));
            serde_json::json!({
                "book": book,
                "below": below,
                "name": book_spell.name,
                "bookRank": book_spell.rank,
                "belowRank": below_spell.rank,
            })
        })
        .collect();
    let file = serde_json::json!({
        "note": "Ahn'Qiraj 20 book ranks the engine reads as the rank below under Forever's rules \
                 (src/prepare/spelldata/book_ranks.rs). Generated: UPDATE_BOOK_RANKS=1 cargo test \
                 --lib book_ranks.",
        "ranks": ranks,
    });
    let mut out = serde_json::to_string_pretty(&file).expect("book ranks serialize");
    out.push('\n');
    out
}

/// Whether a spell is a book rank.
pub(crate) fn is_book_rank(id: i32) -> bool {
    BOOK_RANKS.iter().any(|&(book, _)| book == id)
}

/// The spell a player casts for `id` under these rules: the rank below for a book rank under
/// Forever's, the spell itself otherwise.
pub(crate) fn known_rank(rules: Rules, id: i32) -> i32 {
    if rules != Rules::Forever {
        return id;
    }
    BOOK_RANKS
        .iter()
        .find(|&&(book, _)| book == id)
        .map_or(id, |&(_, below)| below)
}

/// Rewrites every `spellId` a request names from a book rank to the rank below, in place.
/// Returns whether anything changed.
pub(crate) fn rewrite_request(value: &mut serde_json::Value) -> bool {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            let mut changed = false;
            for (key, field) in map.iter_mut() {
                if key == "spellId" {
                    if let Some(id) = field.as_i64().and_then(|id| i32::try_from(id).ok()) {
                        let known = known_rank(Rules::Forever, id);
                        if known != id {
                            *field = Value::from(known);
                            changed = true;
                            continue;
                        }
                    }
                }
                changed |= rewrite_request(field);
            }
            changed
        }
        Value::Array(items) => items
            .iter_mut()
            .fold(false, |changed, item| rewrite_request(item) | changed),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepare::spelldata::store::must_find;

    #[test]
    fn every_book_rank_and_its_rank_below_are_in_the_store() {
        for &(book, below) in BOOK_RANKS {
            let book = must_find(book);
            let below = must_find(below);
            assert_eq!(book.name, below.name, "{} and {}", book.id, below.id);
            assert!(below.spell_level <= book.spell_level);
        }
    }

    #[test]
    fn the_book_ranks_artifact_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(JSON_PATH);
        let want = render_json();
        if std::env::var_os("UPDATE_BOOK_RANKS").is_some() {
            std::fs::write(&path, &want).unwrap();
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            have == want,
            "{JSON_PATH} is stale; regenerate it with UPDATE_BOOK_RANKS=1 cargo test --lib book_ranks \
             and copy it into the app (scripts/sync-forever-book-ranks.mjs there)"
        );
    }

    #[test]
    fn only_forever_reads_a_book_rank_as_the_rank_below() {
        assert_eq!(known_rank(Rules::Forever, 25304), 10181);
        assert_eq!(known_rank(Rules::GoReference, 25304), 25304);
        assert_eq!(known_rank(Rules::Forever, 10181), 10181);
    }

    #[test]
    fn a_request_names_the_rank_below_after_the_rewrite() {
        let mut request = serde_json::json!({
            "rotation": {"priorityList": [
                {"action": {"castSpell": {"spellId": {"spellId": 25311}}}},
                {"action": {"castSpell": {"spellId": {"spellId": 11672}}}},
                {"action": {"castSpell": {"spellId": {"spellId": 25307, "tag": 1}}}},
            ]},
            "itemId": 25304,
        });
        assert!(rewrite_request(&mut request));
        let list = &request["rotation"]["priorityList"];
        assert_eq!(list[0]["action"]["castSpell"]["spellId"]["spellId"], 11672);
        assert_eq!(list[1]["action"]["castSpell"]["spellId"]["spellId"], 11672);
        assert_eq!(list[2]["action"]["castSpell"]["spellId"]["spellId"], 11661);
        assert_eq!(list[2]["action"]["castSpell"]["spellId"]["tag"], 1);
        assert_eq!(request["itemId"], 25304);
        assert!(!rewrite_request(&mut request));
    }
}
