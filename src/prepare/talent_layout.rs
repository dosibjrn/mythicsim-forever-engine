//! A class's talent tree in the client's own order (row, then column), which Forever's rules read
//! once a hotfix moves, adds or removes talents: each position names the talent's proto field,
//! its client name and its ranks. The proto keeps the Go reference's field numbers, which only
//! the reference reads.
//!
//! A talent the pinned proto has no field for (one the client added after the pin) is read into
//! the extras map instead of the message.

use std::collections::BTreeMap;

use crate::contracts::request::Message;

use super::agent::fill_talents_by_layout;

/// One position of a tree.
pub(crate) struct LayoutTalent {
    /// The proto field, snake case, as upstream's proto names it.
    pub field: &'static str,
    /// The client's name, for refusals.
    pub name: &'static str,
    /// TraitNodeEntry MaxRanks.
    pub max_ranks: u32,
    /// Whether the pinned proto has the field; a talent new since the pin has not.
    pub in_proto: bool,
}

/// A talent the pinned proto has a field for.
pub(crate) const fn talent(
    field: &'static str,
    name: &'static str,
    max_ranks: u32,
) -> LayoutTalent {
    LayoutTalent {
        field,
        name,
        max_ranks,
        in_proto: true,
    }
}

/// A talent new since the pin: no proto field, so it is read into the extras.
pub(crate) const fn new_talent(
    field: &'static str,
    name: &'static str,
    max_ranks: u32,
) -> LayoutTalent {
    LayoutTalent {
        field,
        name,
        max_ranks,
        in_proto: false,
    }
}

/// The points of the talents the proto has no field for, by field.
pub(crate) type Extras = BTreeMap<&'static str, i32>;

/// Reads a talent string in the layout's order. A string with more trees than the class has, a
/// tree longer than the layout's, or a talent above its ranks is refused with the reason: an old
/// string read in a new order can put five points in a one rank talent.
pub(crate) fn fill(
    type_name: &str,
    class: &str,
    talents: &str,
    trees: &[&[LayoutTalent]; 3],
) -> Result<(Message, Extras), String> {
    let digits: Vec<&str> = talents.split('-').collect();
    if digits.len() > trees.len() {
        return Err(format!(
            "{class} talents {talents:?}: {} trees, a {class} has {}",
            digits.len(),
            trees.len()
        ));
    }
    let mut extras = Extras::new();
    let mut proto_digits: Vec<String> = Vec::new();
    let mut proto_fields: Vec<Vec<&'static str>> = Vec::new();
    for (tree, tree_digits) in digits.iter().enumerate() {
        let layout = trees[tree];
        // Go len counts bytes.
        if tree_digits.len() > layout.len() {
            return Err(format!(
                "{class} talents {talents:?}: tree {} has {} talents, the string gives {}",
                tree + 1,
                layout.len(),
                tree_digits.len()
            ));
        }
        let mut kept = String::new();
        let mut fields = Vec::new();
        for (talent, digit) in layout.iter().zip(tree_digits.chars()) {
            // As Go reads a non-digit: 0.
            let rank = digit.to_digit(10).unwrap_or(0);
            if rank > talent.max_ranks {
                let ranks = if talent.max_ranks == 1 {
                    "rank"
                } else {
                    "ranks"
                };
                return Err(format!(
                    "{class} talents {talents:?}: {} has {} {ranks}, the string gives {rank}",
                    talent.name, talent.max_ranks
                ));
            }
            if talent.in_proto {
                kept.push(digit);
                fields.push(talent.field);
            } else if rank > 0 {
                extras.insert(talent.field, rank as i32);
            }
        }
        proto_digits.push(kept);
        proto_fields.push(fields);
    }
    let message = fill_talents_by_layout(type_name, &proto_digits.join("-"), |tree, i| {
        proto_fields.get(tree)?.get(i).copied()
    })?;
    Ok((message, extras))
}
