//! Mage preparation: Go sim/mage's construction and initialization, and the exporter's Mage
//! description (tools/oracle-v2/mage.go).

mod export;
pub(crate) mod items;
pub(crate) mod masks;
mod spell_data;
mod spells;
mod talents;

use crate::contracts::request::Message;
use crate::prepare::agent::{fill_talents, fill_talents_by_layout, ClassSpellName, PrepAgent};
use crate::prepare::sim::{Rules, Sim, UnitId};
use crate::prepare::stats::Stat;
use crate::prepare::Refusal;

/// Go mage.TalentTreeSizes: Frost has 18 talents since client hotfix push 112753, which removed
/// Piercing Ice (the fork's patch 186).
const TALENT_TREE_SIZES: [usize; 3] = [19, 17, 18];

/// A talent of client 1.60.1.70707's Mage tree: its proto.MageTalents field, its client name and
/// its ranks (TraitNodeEntry MaxRanks).
struct ForeverTalent {
    field: &'static str,
    name: &'static str,
    max_ranks: u32,
}

const fn talent(field: &'static str, name: &'static str, max_ranks: u32) -> ForeverTalent {
    ForeverTalent {
        field,
        name,
        max_ranks,
    }
}

/// Client 1.60.1.70707's Mage tree (TraitTree 1112), which only Forever's rules read: each tree's
/// talents in the client's order (row, then column). Arcane Focus (TraitNode 105814) is gone and
/// Improved Channeling (105813) takes its square, Arcane Resilience (105802) takes Improved
/// Channeling's, and Missile Barrage and Arcane Resilience swap TraitDefinitions 135332 and
/// 135339, so Missile Barrage sits in the second row. Hotfix push 113531 gives Arcane Resilience a
/// third rank.
const FOREVER_ARCANE: &[ForeverTalent] = &[
    talent("wand_specialization", "Wand Specialization", 2),
    talent("improved_channeling", "Improved Channeling", 5),
    talent("arcane_resilience", "Arcane Resilience", 3),
    talent("arcane_subtlety", "Arcane Subtlety", 2),
    talent("magic_absorption", "Magic Absorption", 2),
    talent("arcane_concentration", "Arcane Concentration", 5),
    talent("missile_barrage", "Missile Barrage", 2),
    talent("arcane_geometry", "Arcane Geometry", 2),
    talent("arcane_impact", "Arcane Impact", 3),
    talent("arcane_blast", "Arcane Blast", 1),
    talent("arcane_shielding", "Arcane Shielding", 2),
    talent("improved_counterspell", "Improved Counterspell", 2),
    talent("arcane_meditation", "Arcane Meditation", 3),
    talent("arcane_wrath", "Arcane Wrath", 1),
    talent("presence_of_mind", "Presence of Mind", 1),
    talent("arcane_mind", "Arcane Mind", 5),
    talent("arcane_instability", "Arcane Instability", 3),
    talent("arcane_power", "Arcane Power", 1),
];

/// Improved Fire Blast (TraitNode 105797) moves from the first row's first column (5020, 2130)
/// to the third row's fourth (6820, 3330), after Pyroblast. Improved Fireball (11069) is called
/// Accelerant from this build on. Incineration has 5 ranks (TraitNodeEntry 130525, was 3) and
/// Critical Mass 3 (130513, was 5).
const FOREVER_FIRE: &[ForeverTalent] = &[
    talent("incineration", "Incineration", 5),
    talent("improved_fireball", "Accelerant", 5),
    talent("ignite", "Ignite", 5),
    talent("flame_throwing", "Flame Throwing", 2),
    talent("impact", "Impact", 3),
    talent("burning_soul", "Burning Soul", 3),
    talent("improved_flamestrike", "Improved Flamestrike", 3),
    talent("pyroblast", "Pyroblast", 1),
    talent("improved_fire_blast", "Improved Fire Blast", 2),
    talent("improved_scorch", "Improved Scorch", 3),
    talent("improved_fire_ward", "Improved Fire Ward", 2),
    talent("heating_up", "Heating Up", 1),
    talent("master_of_elements", "Master of Elements", 3),
    talent("critical_mass", "Critical Mass", 3),
    talent("blast_wave", "Blast Wave", 1),
    talent("fire_power", "Fire Power", 3),
    talent("combustion", "Combustion", 1),
];

/// Unchanged from the Go reference's order. Elemental Precision is called Precision.
const FOREVER_FROST: &[ForeverTalent] = &[
    talent("frost_warding", "Frost Warding", 2),
    talent("improved_frostbolt", "Improved Frostbolt", 5),
    talent("elemental_precision", "Precision", 3),
    talent("ice_shards", "Ice Shards", 5),
    talent("frostbite", "Frostbite", 3),
    talent("improved_frost_nova", "Improved Frost Nova", 2),
    talent("permafrost", "Permafrost", 3),
    talent("frost_channeling", "Frost Channeling", 3),
    talent("improved_blizzard", "Improved Blizzard", 3),
    talent("ice_lance", "Ice Lance", 1),
    talent("arctic_reach", "Arctic Reach", 2),
    talent("ice_block", "Ice Block", 1),
    talent("shatter", "Shatter", 3),
    talent("improved_cone_of_cold", "Improved Cone of Cold", 3),
    talent("cold_snap", "Cold Snap", 1),
    talent("fingers_of_frost", "Fingers of Frost", 2),
    talent("winters_chill", "Winter's Chill", 5),
    talent("ice_barrier", "Ice Barrier", 1),
];

const FOREVER_TREES: [&[ForeverTalent]; 3] = [FOREVER_ARCANE, FOREVER_FIRE, FOREVER_FROST];

/// Under Forever's rules a talent above its ranks in client 1.60.1.70707 is refused, as Go refuses
/// Elemental Precision above 3: an old string read in the new order can put five points in a two
/// rank talent, and a ladder has no rank past its last.
fn check_forever_ranks(talents: &str) -> Result<(), String> {
    for (tree, digits) in talents.split('-').enumerate() {
        for (talent, digit) in FOREVER_TREES[tree].iter().zip(digits.chars()) {
            // As Go reads a non-digit: 0.
            let rank = digit.to_digit(10).unwrap_or(0);
            if rank > talent.max_ranks {
                let ranks = if talent.max_ranks == 1 {
                    "rank"
                } else {
                    "ranks"
                };
                return Err(format!(
                    "mage talents {talents:?}: {} has {} {ranks}, the string gives {rank}",
                    talent.name, talent.max_ranks
                ));
            }
        }
    }
    Ok(())
}

/// Go `checkTalentsString` (the fork's patch 188): a string with more than three trees, or a tree
/// longer than the tree, such as a 19 digit Frost string from before the hotfix, is refused.
/// Forever's rules measure the trees of client 1.60.1.70707, where a 19 digit Arcane string from
/// before Arcane Focus's removal is refused the same way.
fn check_talents_string(talents: &str, tree_sizes: [usize; 3]) -> Result<(), String> {
    let trees: Vec<&str> = talents.split('-').collect();
    if trees.len() > tree_sizes.len() {
        return Err(format!(
            "mage talents {talents:?}: {} trees, a mage has {}",
            trees.len(),
            tree_sizes.len()
        ));
    }
    for (i, tree) in trees.iter().enumerate() {
        // Go len counts bytes.
        if tree.len() > tree_sizes[i] {
            return Err(format!(
                "mage talents {talents:?}: tree {} has {} talents, the string gives {}",
                i + 1,
                tree_sizes[i],
                tree.len()
            ));
        }
    }
    Ok(())
}

/// tools/oracle-v2/mage.go `mageClassSpells`.
static CLASS_SPELLS: &[ClassSpellName] = &[
    ClassSpellName {
        mask: masks::ARCANE_BLAST,
        name: "arcane_blast",
    },
    ClassSpellName {
        mask: masks::ARCANE_EXPLOSION,
        name: "arcane_explosion",
    },
    ClassSpellName {
        mask: masks::ARCANE_POWER,
        name: "arcane_power",
    },
    ClassSpellName {
        mask: masks::ARCANE_MISSILES_CAST,
        name: "arcane_missiles_cast",
    },
    ClassSpellName {
        mask: masks::ARCANE_MISSILES_TICK,
        name: "arcane_missiles_tick",
    },
    ClassSpellName {
        mask: masks::BLAST_WAVE,
        name: "blast_wave",
    },
    ClassSpellName {
        mask: masks::BLIZZARD,
        name: "blizzard",
    },
    ClassSpellName {
        mask: masks::COLD_SNAP,
        name: "cold_snap",
    },
    ClassSpellName {
        mask: masks::CONE_OF_COLD,
        name: "cone_of_cold",
    },
    ClassSpellName {
        mask: masks::EVOCATION,
        name: "evocation",
    },
    ClassSpellName {
        mask: masks::FIRE_BLAST,
        name: "fire_blast",
    },
    ClassSpellName {
        mask: masks::FIREBALL,
        name: "fireball",
    },
    ClassSpellName {
        mask: masks::FLAMESTRIKE,
        name: "flamestrike",
    },
    ClassSpellName {
        mask: masks::FLAMESTRIKE_DOT,
        name: "flamestrike_dot",
    },
    ClassSpellName {
        mask: masks::FROST_ARMOR,
        name: "frost_armor",
    },
    ClassSpellName {
        mask: masks::FROSTBOLT,
        name: "frostbolt",
    },
    ClassSpellName {
        mask: masks::FROST_BLAST,
        name: "frost_blast",
    },
    ClassSpellName {
        mask: masks::FROST_NOVA,
        name: "frost_nova",
    },
    ClassSpellName {
        mask: masks::ICE_BARRIER,
        name: "ice_barrier",
    },
    ClassSpellName {
        mask: masks::ICE_BLOCK,
        name: "ice_block",
    },
    ClassSpellName {
        mask: masks::ICE_LANCE,
        name: "ice_lance",
    },
    ClassSpellName {
        mask: masks::IGNITE,
        name: "ignite",
    },
    ClassSpellName {
        mask: masks::MAGE_ARMOR,
        name: "mage_armor",
    },
    ClassSpellName {
        mask: masks::MANA_GEMS,
        name: "mana_gems",
    },
    ClassSpellName {
        mask: masks::MOLTEN_ARMOR,
        name: "molten_armor",
    },
    ClassSpellName {
        mask: masks::PRESENCE_OF_MIND,
        name: "presence_of_mind",
    },
    ClassSpellName {
        mask: masks::PYROBLAST,
        name: "pyroblast",
    },
    ClassSpellName {
        mask: masks::PYROBLAST_DOT,
        name: "pyroblast_dot",
    },
    ClassSpellName {
        mask: masks::SCORCH,
        name: "scorch",
    },
    ClassSpellName {
        mask: masks::MANA_GEM,
        name: "mana_gem",
    },
    ClassSpellName {
        mask: masks::COMBUSTION,
        name: "combustion",
    },
    ClassSpellName {
        mask: masks::IMPROVED_BLIZZARD,
        name: "improved_blizzard",
    },
    ClassSpellName {
        mask: masks::FROSTFIRE_BOLT,
        name: "frostfire_bolt",
    },
    ClassSpellName {
        mask: masks::ARCANE_WRATH,
        name: "arcane_wrath",
    },
];

/// Go `Mage`: the talents and the class option the registrations read.
#[derive(Clone)]
pub(crate) struct Mage {
    talents: Message,
    /// Go `Options.DefaultMageArmor`, as the enum's name.
    default_mage_armor: String,
}

/// Go `NewMage`.
pub(crate) fn new_mage(
    sim: &mut Sim,
    unit: UnitId,
    player: &Message,
) -> Result<Box<dyn PrepAgent>, Refusal> {
    // Go reads options.GetMage().Options.ClassOptions, and Initialize reads it without a check.
    let class_options = player
        .message("mage")
        .and_then(|mage| mage.message("options"))
        .and_then(|options| options.message("class_options"))
        .ok_or_else(|| {
            Refusal::new(
                "class_option",
                "a mage without its class options".to_string(),
            )
        })?;
    let default_mage_armor = class_options.enum_name("default_mage_armor");
    let talents_string = player.str("talents_string");
    let talents = match sim.rules {
        Rules::Forever => {
            check_talents_string(talents_string, FOREVER_TREES.map(<[ForeverTalent]>::len))
                .and_then(|()| check_forever_ranks(talents_string))
                .and_then(|()| {
                    fill_talents_by_layout("proto.MageTalents", talents_string, |tree, i| {
                        Some(FOREVER_TREES.get(tree)?.get(i)?.field)
                    })
                })
        }
        Rules::GoReference => check_talents_string(talents_string, TALENT_TREE_SIZES)
            .and_then(|()| fill_talents("proto.MageTalents", talents_string, TALENT_TREE_SIZES)),
    }
    .map_err(|err| Refusal::new("talents", err))?;
    // Go NewMage: Elemental Precision above its three ranks (a pre-hotfix 4/5 or 5/5) panics.
    // Forever's rules have refused it with every other talent above its ranks.
    let max_ranks = spell_data::spell_data().elemental_precision.len();
    let elemental_precision = talents.i32("elemental_precision");
    if elemental_precision > max_ranks {
        return Err(Refusal::new(
            "talents",
            format!(
                "mage talents {talents_string:?}: Elemental Precision has {max_ranks} ranks, the string gives {elemental_precision}"
            ),
        ));
    }
    sim.enable_mana_bar(unit);
    let crit_per_agi = sim.crit_per_agi_max_level(unit);
    sim.unit_mut(unit).sdm.add_stat_dependency(
        Stat::Agility,
        Stat::PhysicalCritPercent,
        crit_per_agi,
    );
    // Forever has no Water Elemental, so no pet is created.
    Ok(Box::new(Mage {
        talents,
        default_mage_armor,
    }))
}

impl PrepAgent for Mage {
    fn add_raid_buffs(&self, raid_buffs: &mut Message) {
        raid_buffs.set_bool("arcane_brilliance", true);
    }

    fn apply_talents(&mut self, sim: &mut Sim, unit: UnitId) {
        self.apply_mage_talents(sim, unit);
    }

    /// Go `Mage.Initialize`: the passives, then the spells.
    fn initialize(&mut self, sim: &mut Sim, unit: UnitId) {
        self.register_arcane_charges(sim, unit);
        self.register_spells(sim, unit);
    }

    /// The mage package's `core.NewItemEffect(19959, ...)`: Hazza'rah's Charm of Magic.
    fn apply_item_effect(&mut self, sim: &mut Sim, unit: UnitId, item: i32) -> bool {
        match item {
            19959 => {
                items::hazzarahs_charm(sim, unit);
                true
            }
            _ => false,
        }
    }

    /// arcane_charge.go: Arcane Blast's stacks give spell damage from Intellect, a stacking stat
    /// aura.
    fn stat_auras(&self, _sim: &Sim, _unit: UnitId) -> Vec<String> {
        if self.talents.bool("arcane_blast") {
            vec!["Arcane Blast".to_string()]
        } else {
            Vec::new()
        }
    }

    fn talents(&self) -> &Message {
        &self.talents
    }

    fn class_spells(&self) -> &'static [ClassSpellName] {
        CLASS_SPELLS
    }

    fn effects(&self, sim: &Sim, unit: UnitId) -> Vec<serde_json::Value> {
        export::effects(&self.talents, sim, unit)
    }

    /// `effects`, with the Intellect an iteration's end leaves for Arcane Blast's last log line:
    /// Go's doneIteration deactivates the player's auras in registration order, so those
    /// registered before Arcane Blast's have fallen when its stacks do.
    fn export_effects(
        &self,
        env: &crate::prepare::env::Environment,
        _notes: &mut Vec<String>,
    ) -> Vec<serde_json::Value> {
        let mut effects = self.effects(&env.sim, env.player);
        if let Some(effect) = effects
            .iter_mut()
            .find(|effect| effect["kind"] == "arcane_blast")
        {
            let mut fresh = env.fresh();
            let player = fresh.player;
            for aura in fresh.sim.unit(player).auras.clone() {
                if fresh.sim.aura(aura).label == "Arcane Blast" {
                    break;
                }
                if fresh.sim.aura(aura).active {
                    fresh.sim.deactivate(aura);
                }
            }
            effect["done_intellect"] = serde_json::json!(fresh.sim.stat(player, Stat::Intellect));
        }
        effects
    }

    /// Go `Mage.EurekaSpells`: the spells the client's Eureka! rows name for cost and damage, and
    /// the missile tick on the damage list; no spell is in Tick. Frost Blast carries Frostbolt's bit,
    /// which both masks name (MythicSim patch 153).
    fn eureka_spells(&self) -> Option<crate::prepare::racials::EurekaSpells> {
        let cost = masks::ARCANE_BLAST
            | masks::ARCANE_EXPLOSION
            | masks::ARCANE_MISSILES_CAST
            | masks::BLAST_WAVE
            | masks::BLIZZARD
            | masks::CONE_OF_COLD
            | masks::FIRE_BLAST
            | masks::FIREBALL
            | masks::FLAMESTRIKE
            | masks::FROST_NOVA
            | masks::FROSTBOLT
            | masks::FROSTFIRE_BOLT
            | masks::ICE_LANCE
            | masks::PYROBLAST
            | masks::SCORCH
            | masks::FROST_BLAST;
        Some(crate::prepare::racials::EurekaSpells {
            cost,
            damage: cost | masks::ARCANE_MISSILES_TICK,
            tick: 0,
        })
    }

    /// Go `spell.Matches(mage.MageSpellManaGem)`.
    fn is_mana_gem(&self, sim: &Sim, spell: crate::prepare::sim::SpellId) -> bool {
        sim.spell(spell).matches(masks::MANA_GEM)
    }

    fn damage_effect(
        &self,
        sim: &Sim,
        spell: crate::prepare::sim::SpellId,
    ) -> Option<serde_json::Value> {
        export::damage_effect(sim, spell)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ranks the Forever layout allows are the ranks its ladders have.
    #[test]
    fn forever_ranks_match_the_ladders() {
        let data = spell_data::spell_data();
        let ranks = |field: &str| {
            FOREVER_TREES
                .iter()
                .flat_map(|tree| tree.iter())
                .find(|talent| talent.field == field)
                .map(|talent| talent.max_ranks as i32)
                .unwrap()
        };
        assert_eq!(ranks("critical_mass"), data.critical_mass_70707.len());
        assert_eq!(ranks("incineration"), data.incineration_70707.len());
        assert_eq!(
            ranks("arcane_resilience"),
            data.arcane_resilience_113531.len()
        );
        assert_eq!(ranks("elemental_precision"), data.elemental_precision.len());
        assert!(FOREVER_TREES
            .iter()
            .flat_map(|tree| tree.iter())
            .all(|talent| talent.field != "arcane_focus"));
    }
}
