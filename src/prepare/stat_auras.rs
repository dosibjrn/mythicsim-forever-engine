//! tools/oracle-v2/melee_procs.go `characterStatAuras` and `statAurasEffect`: the stats that
//! change during a fight because an aura's gain and expiry change them through
//! `AddStatsDynamic`, read as a function of which of those auras are active.

use serde_json::{json, Map, Value};

use super::buffs::generated::MANA_TIDE_TOTEMS;
use super::env::Environment;
use super::sim::{AuraId, UnitId};
use super::stats::Stat;
use super::Refusal;

/// The combination key of Go `Character.SpiritManaRegenPerSecond`, which is not a stat.
const SPIRIT_REGEN_KEY: &str = "SpiritManaRegenPerSecond";

/// The stats the Rust runtime reads during a fight.
const DYNAMIC_READ_STATS: [Stat; 6] = [
    Stat::SpellDamage,
    Stat::AttackPower,
    Stat::RangedAttackPower,
    Stat::SpellCritPercent,
    Stat::PhysicalCritPercent,
    Stat::MP5,
];

/// Stats the Rust runtime reads during a fight that a combination carries only when one changes
/// them: maximum mana, healing power and health, Spirit, Intellect, whose change makes Go refresh
/// the cached mana regeneration rates, the school spell damage stats, the resistances a spell
/// that hits the player rolls against, the physical damage a physical spell adds, the armor
/// penetration taken off the target's armor and the spell piercing taken off its resistance.
const OPTIONAL_READ_STATS: [Stat; 21] = [
    Stat::Mana,
    Stat::HealingPower,
    Stat::Health,
    Stat::Spirit,
    Stat::Intellect,
    Stat::ArcaneDamage,
    Stat::FireDamage,
    Stat::FrostDamage,
    Stat::HolyDamage,
    Stat::NatureDamage,
    Stat::ShadowDamage,
    Stat::ArcaneResistance,
    Stat::FireResistance,
    Stat::FrostResistance,
    Stat::NatureResistance,
    Stat::ShadowResistance,
    Stat::PhysicalDamage,
    Stat::ArmorPenetration,
    Stat::SpellPiercing,
    Stat::PhysicalHitPercent,
    Stat::SpellHitPercent,
];

/// Auras of races, items and raid buffs whose gain and expiry change stats through
/// `AddStatsDynamic`. A class adds its own through `PrepAgent::stat_auras`.
const COMMON_STAT_AURA_LABELS: [&str; 11] = [
    "Blood Fury",
    "Elune's Light",
    "Holy Strength (MH)",
    "Holy Strength (OH)",
    "Windfury Totem (External)",
    "Battle Shout (External)",
    "Headmaster's Charge",
    "Crusader's Wrath",
    "Diamond Flask",
    "Mana Tide Totem (External)",
    "Perfected Form",
];

/// consumes.go: a potion's stat buff is a temporary stats aura named for the potion.
fn potion_stat_auras(env: &Environment) -> Vec<String> {
    use super::spell::SpellFlag;
    let mut labels = Vec::new();
    for spell in &env.sim.unit(env.player).spellbook {
        let spell = env.sim.spell(*spell);
        let item = spell.action_id.item_id;
        if item == 0 || !spell.flags.matches(SpellFlag::POTION) {
            continue;
        }
        let consumable = super::consumes::consumable_by_id(item);
        if consumable.buff_duration > 0 {
            labels.push(consumable.name);
        }
    }
    labels
}

/// The auras of `spellDataStatProcAuras`: item procs built from client spell data whose buff
/// changes stats.
fn spell_data_stat_proc_auras(env: &Environment) -> Vec<String> {
    super::export_items::spell_data_stat_proc_auras(env)
}

/// `lionHornProcAura`: The Lion Horn of Stormwind's proc aura.
fn lion_horn_proc_aura(env: &Environment) -> Vec<String> {
    super::export_items::lion_horn_proc_aura(env)
}

/// shared.NewSimpleStatActive: an item's on-use buff is a temporary stats aura, the related self
/// buff of the spells `simpleStatActive` recognizes.
fn simple_stat_active_auras(env: &Environment) -> Vec<String> {
    let mut labels = Vec::new();
    for spell in env.sim.unit(env.player).spellbook.iter().copied() {
        if super::export_items::simple_stat_active(env, spell).is_some() {
            if let Some(aura) = env.sim.spell(spell).related_self_buff {
                labels.push(env.sim.aura(aura).label.clone());
            }
        }
    }
    labels
}

/// Go `characterStatAuras`: the stat auras of the player, in the order a combination's bits
/// number them.
pub(crate) fn character_stat_auras(env: &Environment) -> Vec<String> {
    let mut candidates: Vec<String> = COMMON_STAT_AURA_LABELS
        .iter()
        .map(|label| label.to_string())
        .collect();
    candidates.extend(potion_stat_auras(env));
    candidates.extend(env.agent.stat_auras(&env.sim, env.player));
    candidates.extend(spell_data_stat_proc_auras(env));
    candidates.extend(lion_horn_proc_aura(env));
    candidates.extend(super::healer_gear::stat_auras(env));
    candidates.extend(simple_stat_active_auras(env));
    candidates.extend(
        super::export_items::WEAPON_AURA_PROC_AURAS
            .iter()
            .map(|label| label.to_string()),
    );
    // classic items_trinkets.go Jom Gabbar's stacking aura.
    candidates.push("Jom Gabbar".to_string());
    // Under Forever's rules, the stat buffs of the chance on hit items the reference leaves
    // unregistered.
    for label in super::export_items::chance_on_hit_stat_auras(env) {
        if !candidates.contains(&label) {
            candidates.push(label);
        }
    }
    // item_swaps.go: the auras a swap switches and its own equipment stats.
    for label in super::item_swap::item_swap_stat_auras(env) {
        if !candidates.contains(&label) {
            candidates.push(label);
        }
    }
    candidates
        .into_iter()
        .filter(|label| env.sim.get_aura(env.player, label).is_some())
        .collect()
}

/// The name to value map Go's `statValues` builds.
fn stat_map(env: &Environment, unit: UnitId) -> Vec<(&'static str, f64)> {
    Stat::ALL
        .iter()
        .map(|stat| (stat.name(), env.sim.unit(unit).stats[*stat]))
        .collect()
}

/// Go `statLayout`: the place of each stat aura in a combination's number. An aura read as
/// active or not takes one bit; an aura whose stats follow its stacks takes the bits that count
/// from none to its maximum stacks, and a count past the maximum reads as the maximum, as
/// `set_stacks` clamps it. A combination of auras without stacks is the mask of its active
/// auras.
#[derive(Clone, Debug)]
pub(crate) struct StatLayout {
    pub(crate) labels: Vec<String>,
    /// The maximum stacks of an aura whose stats follow them, else 0.
    pub(crate) stacks: Vec<i32>,
    /// The first bit of each aura.
    pub(crate) offsets: Vec<usize>,
    /// The bits all the auras take.
    pub(crate) bits: usize,
}

impl StatLayout {
    /// Go `newStatLayout`: finds the stat auras whose stats follow their stacks
    /// (`MakeStackingAura`: each stack adds the same bonus) by activating each at every stack
    /// count in a reset simulation of its own. An aura with stacks whose stats do not change with
    /// them, as one that counts charges, is read as active or not.
    pub(crate) fn new(env: &Environment, labels: Vec<String>) -> StatLayout {
        let mut layout = StatLayout {
            labels,
            stacks: Vec::new(),
            offsets: Vec::new(),
            bits: 0,
        };
        let probe = env.fresh();
        for label in layout.labels.clone() {
            let most = probe
                .sim
                .get_aura(probe.player, &label)
                .map_or(0, |aura| probe.sim.aura(aura).max_stacks);
            let mut stacks = 0;
            if most > 0 {
                let mut first: Option<Vec<(&'static str, f64)>> = None;
                for level in 1..=most {
                    let mut fresh = env.fresh();
                    let aura = fresh
                        .sim
                        .get_aura(fresh.player, &label)
                        .expect("the aura exists");
                    apply_level(&mut fresh, aura, level, true);
                    let values = stat_map(&fresh, fresh.player);
                    match &first {
                        None => first = Some(values),
                        Some(first) if *first != values => stacks = most,
                        Some(_) => {}
                    }
                }
            }
            layout.stacks.push(stacks);
            layout.offsets.push(layout.bits);
            layout.bits += layout.width(layout.stacks.len() - 1);
        }
        layout
    }

    /// The bits aura j takes in a combination's number.
    fn width(&self, j: usize) -> usize {
        if self.stacks[j] > 0 {
            (i32::BITS - self.stacks[j].leading_zeros()) as usize
        } else {
            1
        }
    }

    /// The level combination `mask` gives aura j: 1 for an active aura without stacks, else its
    /// stacks.
    pub(crate) fn level(&self, mask: usize, j: usize) -> i32 {
        let digit = (mask >> self.offsets[j] & ((1 << self.width(j)) - 1)) as i32;
        if self.stacks[j] > 0 {
            digit.min(self.stacks[j])
        } else {
            digit
        }
    }
}

/// Go `applyStatLevel`: activates a stat aura and, for one that stacks, adds a stack at a time up
/// to the level, as the procs that stack it do.
pub(crate) fn apply_level(env: &mut Environment, aura: AuraId, level: i32, stacking: bool) {
    if !env.sim.aura(aura).active {
        env.sim.activate(aura);
        super::item_swap::add_item_swap_equip_stats(env, aura);
    }
    if stacking {
        while env.sim.aura(aura).stacks < level {
            env.sim.add_stack(aura);
        }
    }
}

/// The layout of the player's stat auras.
pub(crate) fn stat_layout(env: &Environment) -> StatLayout {
    StatLayout::new(env, character_stat_auras(env))
}

/// Go `statAurasEffect`: each combination of the labels' auras is read from a separate reset
/// simulation with exactly those auras active: combination i has aura j at the level the layout
/// reads from i, and an aura active after the reset, as a druid's starting form, is deactivated
/// at level zero.
pub(crate) fn stat_auras_effect(env: &Environment) -> Result<Option<Value>, Refusal> {
    stat_auras_effect_reading(env, &stat_layout(env), None)
}

/// Go's `comboReader`: reads more of a combination's reset simulation, which the stat auras
/// effect has already set up. `exact` is false when its own setup left a different simulation
/// than `enemy::set_stat_auras` does, so the reader must set one up itself.
pub(crate) type ComboReader<'a> =
    &'a mut dyn FnMut(usize, &mut Environment, bool) -> Result<(), Refusal>;

/// `statAurasEffect` with its reader: a tank's swing is read under every combination from the
/// same simulations (`enemy::EnemyCombos::read`).
pub(crate) fn stat_auras_effect_reading(
    env: &Environment,
    layout: &StatLayout,
    mut reader: Option<ComboReader>,
) -> Result<Option<Value>, Refusal> {
    let labels = &layout.labels;
    if labels.is_empty() {
        return Ok(None);
    }
    if layout.bits > 14 {
        return Err(Refusal::new(
            "stat_auras",
            format!("{} stat auras exceed the combination limit", labels.len()),
        ));
    }
    let mut combos: Vec<Map<String, Value>> = Vec::new();
    // The MP5 of the raw stats, which `add_stats_dynamic` adds each bonus to and takes it from.
    let mut raw_mp5: Vec<f64> = Vec::new();
    let mut spirit_regens: Vec<f64> = Vec::new();
    let mut changed_spirit_regen = false;
    let mut changed: Vec<&'static str> = Vec::new();
    let mut base: Vec<(&'static str, f64)> = Vec::new();
    for mask in 0..(1usize << layout.bits) {
        let mut fresh = env.fresh();
        let player = fresh.player;
        // An aura up from the reset, such as the default stance, is down at level zero.
        for (j, label) in labels.iter().enumerate() {
            let aura = fresh.sim.get_aura(player, label).expect("the aura exists");
            if layout.level(mask, j) == 0 && fresh.sim.aura(aura).active {
                fresh.sim.deactivate(aura);
            }
        }
        let mut exact = true;
        for (j, label) in labels.iter().enumerate() {
            let aura = fresh.sim.get_aura(player, label).expect("the aura exists");
            let level = layout.level(mask, j);
            let active = fresh.sim.aura(aura).active;
            if level > 0 {
                apply_level(&mut fresh, aura, level, layout.stacks[j] > 0);
            } else if active {
                // An activation switched this aura on again: `set_stat_auras` would leave it up.
                exact = false;
                fresh.sim.deactivate(aura);
            }
        }
        let values = stat_map(&fresh, player);
        raw_mp5.push(fresh.sim.unit(player).stats_without_deps[Stat::MP5]);
        if mask == 0 {
            base = values.clone();
        }
        let mut combo = Map::new();
        for stat in DYNAMIC_READ_STATS {
            combo.insert(
                stat.name().to_string(),
                json!(fresh.sim.unit(player).stats[stat]),
            );
        }
        for stat in OPTIONAL_READ_STATS {
            combo.insert(
                stat.name().to_string(),
                json!(fresh.sim.unit(player).stats[stat]),
            );
        }
        // Spirit regeneration follows Intellect and Spirit, which mana.go UpdateManaRegenRates
        // rereads when a stat aura changes either.
        let character = fresh.sim.character(player);
        let spirit_regen = fresh.sim.unit(player).stats[Stat::Spirit].mul_add(
            character.spirit_regen_per_spirit,
            character.spirit_regen_base,
        );
        combo.insert(SPIRIT_REGEN_KEY.to_string(), json!(spirit_regen));
        if mask > 0 && spirit_regen != spirit_regens[0] {
            changed_spirit_regen = true;
        }
        spirit_regens.push(spirit_regen);
        combos.push(combo);
        for ((name, value), (_, base_value)) in values.iter().zip(&base) {
            if value != base_value && !changed.contains(name) {
                changed.push(name);
            }
        }
        if let Some(reader) = reader.as_mut() {
            reader(mask, &mut fresh, exact)?;
        }
    }
    changed.sort_unstable();
    // The optional stats stay out of the combinations unless one changes them, and so does
    // spirit regeneration.
    for stat in OPTIONAL_READ_STATS {
        if !changed.contains(&stat.name()) {
            for combo in &mut combos {
                combo.remove(stat.name());
            }
        }
    }
    if !changed_spirit_regen {
        for combo in &mut combos {
            combo.remove(SPIRIT_REGEN_KEY);
        }
    }
    let mut effect = json!({"kind": "stat_auras", "auras": labels,
        "combos": combos, "changed": changed});
    if layout.stacks.iter().any(|&stacks| stacks > 0) {
        effect["stacks"] = json!(layout.stacks);
    }
    let raw = raw_mp5_bonuses(layout, &raw_mp5)?;
    if !raw.is_empty() {
        effect["raw_mp5"] = json!(raw);
    }
    Ok(Some(effect))
}

/// The label `buffs.ManaTideTotemsAura` gives the external caster's copy.
const EXTERNAL_MANA_TIDE_LABEL: &str = "Mana Tide Totem (External)";
/// The label of the Restoration shaman's own Mana Tide Totem, whose MP5 is the same row's.
const SELF_MANA_TIDE_LABEL: &str = "Mana Tide Totem (Self)";

/// `rawMP5Bonuses`: `AddStatsDynamic` adds each bonus to the raw stats and takes it away again,
/// so a bonus that is not a whole number leaves the sum a few ulps off its start once the aura
/// expires (77 plus Mana Tide Totem's 483.33 and minus it again is 76.99999999999994), where the
/// combination reads the start. The raw MP5 bonus of such an aura lets the runtime add and take
/// it as Go does. A bonus that is a multiple of 2^-10 (a whole number, or the 31.25 of Mana
/// Spring Totem) is added and taken away exactly. Only Mana Tide Totem's is not, and a reset
/// simulation checks that its aura adds exactly its value; any other inexact MP5 bonus is
/// refused.
fn raw_mp5_bonuses(
    layout: &StatLayout,
    raw: &[f64],
) -> Result<std::collections::BTreeMap<String, f64>, Refusal> {
    let mut bonuses = std::collections::BTreeMap::new();
    for (j, label) in layout.labels.iter().enumerate() {
        let first = raw[1 << layout.offsets[j]];
        let delta = first - raw[0];
        if delta == 0.0
            || (delta * 1024.0 == (delta * 1024.0).trunc() && (raw[0] + delta) - delta == raw[0])
        {
            continue;
        }
        let value = MANA_TIDE_TOTEMS.value(0);
        if (label == EXTERNAL_MANA_TIDE_LABEL || label == SELF_MANA_TIDE_LABEL)
            && raw[0] + value == first
        {
            bonuses.insert(label.clone(), value);
            continue;
        }
        return Err(Refusal::new(
            "stat_auras",
            format!("{label} adds {delta} MP5, which is not added and taken away exactly"),
        ));
    }
    Ok(bonuses)
}
