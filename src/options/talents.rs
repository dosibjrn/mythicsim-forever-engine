//! TalentsForever client-tree facts attached to native get_options responses.
//! These are *discovery* prerequisites, never a new simulator build validator.

use super::{Class, Prerequisite, TalentOption};
use serde_json::Value;
use std::sync::OnceLock;

const SOURCE: &str = include_str!("../../data/talentsforever_talent_options.json");

fn catalog() -> &'static Value {
    static DATA: OnceLock<Value> = OnceLock::new();
    DATA.get_or_init(|| serde_json::from_str(SOURCE).expect("pinned TalentsForever talent data"))
}

pub(super) fn source_revision() -> &'static str {
    catalog()["source_commit"]
        .as_str()
        .expect("pinned TalentsForever commit")
}

fn class_key(class: Class) -> &'static str {
    match class {
        Class::Druid => "druid",
        Class::Hunter => "hunter",
        Class::Mage => "mage",
        Class::Paladin => "paladin",
        Class::Priest => "priest",
        Class::Rogue => "rogue",
        Class::Shaman => "shaman",
        Class::Warlock => "warlock",
        Class::Warrior => "warrior",
    }
}

fn snake_case(field: &str) -> String {
    let mut result = String::with_capacity(field.len() + 5);
    for character in field.chars() {
        if character.is_ascii_uppercase() {
            result.push('_');
            result.push(character.to_ascii_lowercase());
        } else {
            result.push(character);
        }
    }
    result
}

/// The two native layouts already exposed by the engine provide native proto
/// field names for their positions (which are not always client display fields).
fn native_layout(class: Class) -> Option<Value> {
    match class {
        Class::Druid => Some(crate::forever_druid_talent_layout()),
        Class::Mage => Some(crate::classes::mage::prepare::forever_talent_layout_json()),
        _ => None,
    }
}

pub(super) fn options(class: Class) -> Vec<TalentOption> {
    let source = &catalog()["classes"][class_key(class)];
    let layout = native_layout(class);
    let mut result = Vec::new();

    for (tree_index, tree) in source.as_array().expect("three class talent trees").iter().enumerate() {
        let tree_name = tree["name"].as_str().expect("tree name").to_owned();
        let entries = tree["talents"].as_array().expect("talent slots");
        for (position, entry) in entries.iter().enumerate() {
            let source_field = entry["field_name"].as_str().expect("talent field");
            let native = layout.as_ref().and_then(|value| {
                value["trees"].get(tree_index)?.get("talents")?.get(position)
            });
            let field = native
                .and_then(|slot| slot["field"].as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| snake_case(source_field));
            let in_proto = native
                .and_then(|slot| slot["in_proto"].as_bool())
                .unwrap_or_else(|| {
                    let proto_type = format!("proto.{}Talents", class.proto_name().trim_start_matches("Class"));
                    crate::contracts::request::field_by_name(&proto_type, &field).is_some()
                });
            let tier_points = entry["points_required_in_tree"].as_u64().expect("tier points") as u8;
            let min_level = entry["minimum_level"].as_u64().expect("minimum level") as u8;
            let mut prerequisites = vec![
                Prerequisite::MinimumLevel { level: min_level },
                Prerequisite::PointsInTree {
                    tree: tree_name.clone(),
                    points: tier_points,
                },
            ];
            if let Some(parent) = entry.get("prerequisite") {
                let parent_field = parent["field_name"].as_str().expect("parent field");
                let parent_position = entries
                    .iter()
                    .position(|talent| talent["field_name"] == parent_field)
                    .expect("parent is in same tree");
                prerequisites.push(Prerequisite::TalentRank {
                    source_field: parent_field.to_owned(),
                    position: parent_position,
                    required_rank: parent["required_rank"].as_u64().expect("parent rank") as u32,
                });
            }
            result.push(TalentOption {
                tree: tree_name.clone(),
                position,
                field,
                source_field: source_field.to_owned(),
                name: entry["name"].as_str().expect("talent name").to_owned(),
                max_ranks: entry["max_rank"].as_u64().expect("max rank") as u32,
                in_proto,
                prerequisites,
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_contains_all_nine_class_trees_and_explicit_arrows() {
        let classes = [
            Class::Druid, Class::Hunter, Class::Mage, Class::Paladin, Class::Priest,
            Class::Rogue, Class::Shaman, Class::Warlock, Class::Warrior,
        ];
        let mut count = 0;
        let mut arrows = 0;
        for class in classes {
            let trees = catalog()["classes"][class_key(class)].as_array().unwrap();
            assert_eq!(trees.len(), 3);
            let talents = options(class);
            count += talents.len();
            for talent in &talents {
                for prerequisite in &talent.prerequisites {
                    if let Prerequisite::TalentRank {
                        source_field,
                        position,
                        required_rank,
                    } = prerequisite
                    {
                        arrows += 1;
                        let parent = talents.iter().find(|t| {
                            t.tree == talent.tree && t.position == *position
                        }).unwrap();
                        assert_eq!(&parent.source_field, source_field);
                        assert_eq!(parent.max_ranks, *required_rank);
                    }
                }
            }
        }
        assert_eq!(count, 467);
        assert_eq!(arrows, 72);
    }

    #[test]
    fn current_paladin_holy_same_row_arrow_is_preserved() {
        let talents = options(Class::Paladin);
        let boundless = talents.iter().find(|t| t.source_field == "boundlessLight").unwrap();
        assert!(boundless.prerequisites.iter().any(|p| matches!(
            p,
            Prerequisite::TalentRank {
                source_field, required_rank: 1, ..
            } if source_field == "holyShock"
        )));
        assert!(boundless.prerequisites.iter().any(|p| matches!(
            p,
            Prerequisite::PointsInTree { points: 20, .. }
        )));
    }
}
