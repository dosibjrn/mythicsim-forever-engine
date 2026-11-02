//! The exporter's Warlock part, tools/oracle-v2/warlock.go: the client damage rows of the
//! Warlock's spells (`warlockDamageRows`), the stable names of the spells Go registers without a
//! class mask, and the effects whose parameters Go keeps in closures (`warlockEffects`).

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use serde_json::{json, Map, Value};

use crate::contracts::prepared_v2::ActionId;
use crate::data::spells::Effect;
use crate::prepare::aura_helpers::ProcTrigger;
use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::dbcenums;
use crate::prepare::env::Environment;
use crate::prepare::sim::{AuraId, Duration, Sim, SpellId, UnitId, MILLISECOND, NEVER_EXPIRES};
use crate::prepare::spell::{school, DefenseType};
use crate::prepare::spelldata::{Ladder, Spell as Row};
use crate::prepare::stats::{SchoolIndex, Stat};

use super::spell_data::spell_data;
use super::{masks, Warlock};

fn nanos(d: Duration) -> i64 {
    d
}

/// `unmaskedSpells`: talents_affliction.go registerAmplifyCurse registers its cast without a
/// class mask, talents_demonology.go applyDemonicBrand registers each demon's brand hit without
/// one, and talents_destruction.go applyBaneOfHavoc builds its cast from the client row alone.
pub(super) fn unmasked_spell(id: &ActionId) -> Option<&'static str> {
    if id.tag != 0 || id.item_id != 0 || !id.other_id.is_empty() {
        return None;
    }
    match id.spell_id {
        18288 => Some("amplify_curse"),
        1293697 | 1293698 => Some("demonic_brand"),
        1225228 => Some("bane_of_havoc"),
        _ => None,
    }
}

/// `warlockDamageRows`: the rows by spell ID.
fn damage_rows() -> &'static HashMap<i32, &'static Row> {
    static ROWS: OnceLock<HashMap<i32, &'static Row>> = OnceLock::new();
    ROWS.get_or_init(|| {
        let data = spell_data();
        let mut rows: HashMap<i32, &'static Row> = HashMap::new();
        // Every rank of Shadow Bolt and Immolate: Forever's rules cast the rank below the
        // Ahn'Qiraj 20 book (spelldata::book_ranks), the Go reference the book rank.
        for ladder in [&data.shadow_bolt, &data.immolate] {
            ladder.each(|_, row| {
                rows.insert(row.id, row);
            });
        }
        for ladder in [
            &data.conflagrate,
            &data.shadowburn,
            &data.searing_pain,
            &data.soul_fire,
            &data.incinerate,
        ] {
            let row = ladder.highest();
            if !row.is_nil() {
                rows.insert(row.id, row);
            }
        }
        rows
    })
}

/// Go `attachDamageEffects` for one spell: the roll of the client row, `{average, variance}`.
pub(super) fn damage_effect(sim: &Sim, spell: SpellId) -> Option<Value> {
    let action = &sim.spell(spell).action_id;
    if action.spell_id == 0 || action.tag != 0 {
        return None;
    }
    let row = damage_rows().get(&action.spell_id)?;
    let effect = row.damage_effect();
    if effect.is_nil() {
        return None;
    }
    Some(json!({"average": effect.average(CHARACTER_LEVEL), "variance": effect.variance}))
}

/// The spellbook positions of spells whose class mask is in a set.
fn spells_matching(sim: &Sim, unit: UnitId, mask: i64) -> Vec<usize> {
    sim.unit(unit)
        .spellbook
        .iter()
        .enumerate()
        .filter(|(_, spell)| sim.spell(**spell).matches(mask))
        .map(|(i, _)| i)
        .collect()
}

/// The spellbook positions of spells a proc trigger with a class mask listens to.
fn proc_trigger_spells(sim: &Sim, unit: UnitId, mask: i64) -> Vec<usize> {
    let trigger = ProcTrigger {
        class_spell_mask: mask,
        ..ProcTrigger::default()
    };
    sim.unit(unit)
        .spellbook
        .iter()
        .enumerate()
        .filter(|(_, spell)| trigger.matches_spell(sim.spell(**spell)))
        .map(|(i, _)| i)
        .collect()
}

/// Go `periodicRank`.
fn periodic_rank(row: &Row) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("spell_id".into(), json!(row.id));
    map.insert(
        "tick_base".into(),
        json!(row.periodic_effect().average(CHARACTER_LEVEL)),
    );
    map.insert(
        "tick_can_crit".into(),
        json!(row.periodic_can_crit() && row.defense_type_core() == DefenseType::Magic),
    );
    map
}

/// `warlockTick`: the periodic tick `periodicTickOutcome` picks; a physical crit is not exported.
fn warlock_tick(row: &Row, unrepresented: &mut Vec<String>) -> Map<String, Value> {
    if row.periodic_can_crit() && row.defense_type_core() != DefenseType::Magic {
        unrepresented.push(format!("spell {} ticks with a physical crit roll", row.id));
    }
    periodic_rank(row)
}

fn with_kind(kind: &str, mut fields: Map<String, Value>) -> Value {
    fields.insert("kind".into(), json!(kind));
    Value::Object(fields)
}

/// The Imp's Firebolt rank 7 damage effect (`impFireboltEffect`), client 1.60.1.70094 11763.
fn imp_firebolt_effect() -> Effect {
    Effect {
        base_points: 44.0,
        ppl: 0.6000000238418579,
        variance: 0.11363636702,
        spell_level: 58,
        max_level: 63,
        ..Effect::default()
    }
}

/// The names `resistanceStatNames` gives the stats a curse lowers.
const RESISTANCE_STATS: [(Stat, &str, i32); 5] = [
    (Stat::ArcaneResistance, "arcane", school::ARCANE as i32),
    (Stat::FireResistance, "fire", school::FIRE as i32),
    (Stat::FrostResistance, "frost", school::FROST as i32),
    (Stat::NatureResistance, "nature", school::NATURE as i32),
    (Stat::ShadowResistance, "shadow", school::SHADOW as i32),
];

/// `schoolNames`.
const SCHOOL_NAMES: [&str; 8] = [
    "none", "physical", "arcane", "fire", "frost", "holy", "nature", "shadow",
];

const SCHOOL_INDICES: [SchoolIndex; 8] = [
    SchoolIndex::None,
    SchoolIndex::Physical,
    SchoolIndex::Arcane,
    SchoolIndex::Fire,
    SchoolIndex::Frost,
    SchoolIndex::Holy,
    SchoolIndex::Nature,
    SchoolIndex::Shadow,
];

impl Warlock {
    /// Go `warlockEffects`.
    pub(super) fn warlock_effects(
        &self,
        env: &Environment,
        unrepresented: &mut Vec<String>,
    ) -> Vec<Value> {
        let sim = &env.sim;
        let unit = env.player;
        let data = spell_data();
        let talents = &self.talents;
        let target = env.encounter.targets[0];
        let mut effects: Vec<Value> = Vec::new();

        // shadowbolt.go: every rank, the hit lands after travel.
        effects.push(json!({"kind": "shadow_bolt"}));
        // immolate.go, corruption.go: a snapshot dot of the client's periodic effect.
        effects.push(with_kind(
            "immolate",
            warlock_tick(data.immolate.highest_for(env.rules), unrepresented),
        ));
        effects.push(with_kind(
            "corruption",
            warlock_tick(data.corruption.highest_for(env.rules), unrepresented),
        ));
        // doom.go: one snapshot tick a minute on, on the bane slot.
        effects.push(with_kind(
            "bane_of_doom",
            warlock_tick(data.bane_of_doom.highest(), unrepresented),
        ));
        // drain_life.go: a channeled dot scaled by Soul Siphon, which counts every registered
        // Affliction aura on the target (warlock.go AfflictionCount), and healing for each tick.
        let mut drain = warlock_tick(data.drain_life.highest(), unrepresented);
        let mut soul_siphon = 1.0;
        let soul_siphon_points = talents.i32("soul_siphon");
        if soul_siphon_points > 0 {
            // `1 + x*y` is one fused multiply-add in Go's exporter (warlock.go 161).
            soul_siphon = data
                .soul_siphon
                .fraction_at(soul_siphon_points)
                .mul_add(Warlock::affliction_count(sim, target).min(3.0), 1.0);
        }
        drain.insert("soul_siphon".into(), json!(soul_siphon));
        drain.insert(
            "self_healing_multiplier".into(),
            json!(sim.unit(unit).pseudo_stats.self_healing_multiplier),
        );
        effects.push(with_kind("drain_life", drain));
        if talents.bool("wrack") {
            // wrack.go: a channel scaled by Soul Siphon and a bonus on two dots' ticks
            let row = data.wrack.highest();
            let mut wrack = warlock_tick(row, unrepresented);
            wrack.insert("soul_siphon".into(), json!(soul_siphon));
            wrack.insert("dot_bonus".into(), json!(1.0 + row.effect_n(2).percent()));
            wrack.insert(
                "dot_spells".into(),
                json!(spells_matching(
                    sim,
                    unit,
                    masks::CORRUPTION | masks::CURSE_OF_AGONY
                )),
            );
            effects.push(with_kind("wrack", wrack));
        }
        // hellfire.go: each tick rolls the periodic effect's average on every target, then burns
        // the warlock; the area hits crit unless the client row of the triggered spell cannot.
        let hellfire = data.hellfire.highest();
        effects.push(json!({
            "kind": "hellfire", "spell_id": hellfire.id,
            "tick_base": hellfire.effect(dbcenums::A_PERIODIC_DAMAGE, 0).average(CHARACTER_LEVEL),
            "tick_can_crit": !hellfire
                .effect(dbcenums::A_PERIODIC_TRIGGER_SPELL, 0)
                .trigger()
                .cannot_crit(),
        }));
        // rain_of_fire.go: the cast hits every target without damage, then the channel casts the
        // triggered tick of its rank every period.
        let rain = data.rain_of_fire.highest();
        let rain_tick = data.rain_of_fire_triggered.rank(rain.rank_number());
        effects.push(json!({
            "kind": "rain_of_fire", "spell_id": rain.id, "tick_spell_id": rain_tick.id,
            "tick_base": rain_tick.damage_effect().average(CHARACTER_LEVEL),
        }));
        if talents.bool("bane_of_havoc") {
            // talents_destruction.go applyBaneOfHavoc
            let row = data.bane_of_havoc.highest();
            effects.push(json!({
                "kind": "bane_of_havoc", "spell_id": row.id,
                "aura": format!("Bane of Havoc-{}", sim.unit(unit).label),
                "copy_aura": "Bane of Havoc - Copy",
                "share": row.effect(dbcenums::A_DUMMY, 0).percent(),
            }));
        }
        // death_coil.go: the effect's average with its coefficient on the spell, landing after
        // travel, and a heal of the damage through its tagged healing spell on the warlock.
        // Go notes dynamic healing taken modifiers on the warlock; Rust registers none.
        let pseudo = &sim.unit(unit).pseudo_stats;
        effects.push(json!({
            "kind": "death_coil",
            "base_damage": data.death_coil.highest().effect_n(1).average(CHARACTER_LEVEL),
            "healing_dealt_multiplier": pseudo.healing_dealt_multiplier,
            "healing_taken_multiplier": pseudo.healing_taken_multiplier,
            "table_healing_dealt_multiplier": env.attack_table(unit, unit).healing_dealt_multiplier,
            "healing_power": sim.stat(unit, Stat::HealingPower) + pseudo.bonus_healing_taken,
        }));
        if talents.bool("incinerate") {
            // incinerate.go: the client roll, raised on a burning target
            effects.push(json!({
                "kind": "incinerate",
                "immolate_bonus": 1.0 + data.incinerate.highest().effect_n(2).percent(),
            }));
        }
        if talents.bool("siphon_life") {
            // siphon_life.go: Corruption's shape, healing for each tick
            let mut siphon = warlock_tick(data.siphon_life.highest(), unrepresented);
            // The ticks crit though the client row lacks periodic-can-crit (community #782).
            siphon.insert("tick_can_crit".into(), json!(true));
            siphon.insert(
                "self_healing_multiplier".into(),
                json!(pseudo.self_healing_multiplier),
            );
            effects.push(with_kind("siphon_life", siphon));
        }
        // agony.go: the snapshot pays half the tick and every fourth tick adds that half back,
        // Go literals.
        let mut agony = warlock_tick(data.bane_of_agony.highest(), unrepresented);
        agony.insert("ramp_share".into(), json!(0.5));
        agony.insert("ramp_every_ticks".into(), json!(4));
        if talents.bool("amplify_curse") {
            agony.insert(
                "amplify".into(),
                json!(1.0 + data.amplify_curse.effect_at(1).fraction_at(1)),
            );
        }
        effects.push(with_kind("bane_of_agony", agony));
        if talents.bool("amplify_curse") {
            // talents_affliction.go registerAmplifyCurse
            effects.push(json!({
                "kind": "amplify_curse", "spell_id": data.amplify_curse.highest().id,
                "aura": "Amplify Curse",
            }));
        }
        // curse_of_elements.go: core's debuff on the target, whose stat and damage taken changes
        // the exporter reads off the client row and checks against Go activating it.
        let target_index = sim.unit(target).unit_index as usize;
        if let Some(Some(aura)) = self.curse_of_elements_auras.get(target_index) {
            effects.push(curse_of_elements_effect(env, target, *aura, unrepresented));
        }
        if let Some(Some(aura)) = self.curse_of_recklessness_auras.get(target_index) {
            effects.push(curse_of_recklessness_effect(
                env,
                target,
                *aura,
                unrepresented,
            ));
        }
        // lifetap.go: (base + Spirit) * (1 + Improved Life Tap) health into as much mana.
        let life_tap = data.life_tap.highest();
        let mut tap = json!({
            "kind": "life_tap", "spell_id": life_tap.id,
            "base_amount": life_tap.effect_n(1).average(CHARACTER_LEVEL),
            "mana_multiplier": 1.0 + data.improved_life_tap.fraction_at(talents.i32("improved_life_tap")),
            // The client flags every rank No Threat: the mana adds no threat, the demon's share
            // neither.
            "no_threat": life_tap.no_threat(),
        });
        // Demonic Energies hands the summoned demon a share of the restore.
        let share = data
            .demonic_energies
            .effect_at(2)
            .fraction_at(talents.i32("demonic_energies"));
        if share > 0.0 && self.pets.active.is_some() {
            tap["pet_mana_share"] = json!(share);
        }
        effects.push(tap);
        if talents.bool("conflagrate") {
            // conflagrate.go
            effects.push(json!({
                "kind": "conflagrate", "spell_id": data.conflagrate.highest().id,
                "keep_immolate_chance": data
                    .shadow_and_flame
                    .effect_at(2)
                    .fraction_at(talents.i32("shadow_and_flame")),
                "rng_label": "Shadow and Flame",
            }));
        }
        // shadowburn.go, searing_pain.go, soulfire.go: one client damage roll each.
        if talents.bool("shadowburn") {
            effects.push(json!({"kind": "shadowburn"}));
        }
        effects.push(json!({"kind": "searing_pain"}));
        effects.push(json!({"kind": "soul_fire"}));
        let nightfall = talents.i32("nightfall");
        if nightfall > 0 {
            // talents_affliction.go applyNightfall
            effects.push(json!({
                "kind": "nightfall", "trigger_aura": "Nightfall", "aura": "Shadow Trance",
                "aura_spell_id": data.nightfall_triggered.highest().id,
                "proc_chance": data.nightfall.fraction_at(nightfall),
                "rng_label": "Nightfall",
                "trigger_spells": proc_trigger_spells(sim, unit, masks::NIGHTFALL_SPELLS),
                "consume_spells": proc_trigger_spells(sim, unit, masks::SHADOW_BOLT),
                "modded_spells": spells_matching(sim, unit, masks::SHADOW_BOLT),
                "cast_time_percent": -1.0,
            }));
        }
        // pets.go: the summoned demon casts the first of its abilities it can afford above
        // MinMana, and otherwise waits 100 ms, a Go literal.
        if let Some(pet) = self.pets.active {
            let state = self.pets.state(pet);
            let book = &sim.unit(pet).spellbook;
            let autocast: Vec<usize> = state
                .map(|state| {
                    state
                        .auto_cast_abilities
                        .iter()
                        .filter_map(|ability| book.iter().position(|spell| spell == ability))
                        .collect()
                })
                .unwrap_or_default();
            effects.push(json!({
                "kind": "warlock_pet", "pet": sim.unit(pet).label,
                "min_mana": state.map_or(0.0, |state| state.min_mana),
                "autocast_spells": autocast,
                "wait_ns": nanos(100 * MILLISECOND),
            }));
        }
        if self.pets.imp.is_some() {
            // pets.go registerFireboltSpell: impFireboltEffect, unexported, mirrored here
            let effect = imp_firebolt_effect();
            let average = effect.average(CHARACTER_LEVEL);
            effects.push(json!({
                "kind": "firebolt",
                "min_damage": average * (1.0 - effect.variance / 2.0),
                "max_damage": average * (1.0 + effect.variance / 2.0),
                "average": average, "variance": effect.variance,
            }));
        }
        if self.pets.succubus.is_some() {
            // pets.go registerLashOfPainSpell: a Go literal base
            effects.push(json!({"kind": "lash_of_pain", "base_damage": 50.0}));
        }
        // talents_demonology.go applyFelEnergy: the Voidwalker's sacrifice restores a share of
        // maximum mana every period, from a periodic action its permanent aura starts.
        if let Some(aura) = sim.get_aura(unit, "Demonic Sacrifice") {
            let aura = sim.aura(aura);
            if aura
                .action_id
                .as_ref()
                .is_some_and(|action| action.spell_id == 18792)
            {
                let row = data.demonic_sacrifice_triggered.by_id(18792);
                effects.push(json!({
                    "kind": "fel_energy", "aura": aura.label, "spell_id": row.id,
                    "mana_fraction": row.effect_n(1).percent(),
                    "period_ns": nanos(row.effect_n(1).period()),
                }));
            }
        }
        let decimation = talents.i32("decimation");
        if decimation > 0 {
            // talents_demonology.go applyDecimation
            let aura_label = self
                .decimation_aura
                .map(|aura| sim.aura(aura).label.clone())
                .unwrap_or_default();
            effects.push(json!({
                "kind": "decimation", "trigger_aura": "Decimation Trigger", "aura": aura_label,
                "execute_phase": 35,
                "trigger_spells": proc_trigger_spells(
                    sim, unit, masks::SHADOW_BOLT | masks::SEARING_PAIN),
                "damage_spells": spells_matching(
                    sim, unit, masks::SHADOW_BOLT | masks::SEARING_PAIN),
                "damage_done_flat": data.decimation.effect_at(4).fraction_at(decimation),
                "cast_spells": spells_matching(sim, unit, masks::SOUL_FIRE),
                "cast_time_percent": data.decimation.effect_at(1).fraction_at(decimation),
            }));
        }
        if talents.i32("demonic_brand") > 0 && !self.sacrifice_summon() {
            // talents_demonology.go applyDemonicBrand
            let brand_aura = self
                .demonic_brand_auras
                .get(target_index)
                .copied()
                .flatten()
                .map(|aura| sim.aura(aura));
            let mut brand = json!({
                "kind": "demonic_brand", "trigger_aura": "Demonic Brand Trigger",
                "target_aura": brand_aura.map(|aura| aura.label.clone()).unwrap_or_default(),
                "charges": brand_aura.map_or(0, |aura| aura.max_stacks),
                "trigger_spells": proc_trigger_spells(sim, unit, masks::SEARING_PAIN),
            });
            if let Some(pet) = self.pets.active {
                let (brand_id, power) = if Some(pet) == self.pets.imp {
                    (1293698, Stat::FireDamage)
                } else {
                    (1293697, Stat::ShadowDamage)
                };
                // The brand spell is the last one of the demon's spellbook with the id.
                if let Some(position) = sim
                    .unit(pet)
                    .spellbook
                    .iter()
                    .rposition(|spell| sim.spell(*spell).action_id.spell_id == brand_id)
                {
                    brand["brand_spell"] = json!(position);
                }
                // The hit's roll and spell power share are Go literals.
                let level_bonus = f64::from(CHARACTER_LEVEL - 26) * 1.5;
                brand["pet"] = json!(sim.unit(pet).label);
                brand["marker_aura"] = json!("Demonic Brand");
                brand["consumer_aura"] = json!("Demonic Brand consumer");
                brand["min_damage"] = json!(level_bonus + 14.0);
                brand["max_damage"] = json!(level_bonus + 17.0);
                brand["spell_power_coefficient"] = json!(0.078);
                brand["school_power_stat"] = json!(power.name());
            }
            effects.push(brand);
        }
        let improved_shadow_bolt = talents.i32("improved_shadow_bolt");
        if improved_shadow_bolt > 0 {
            // talents_destruction.go applyImprovedShadowBolt
            let aura_label = self
                .improved_shadow_bolt_auras
                .get(target_index)
                .copied()
                .flatten()
                .map(|aura| sim.aura(aura).label.clone())
                .unwrap_or_default();
            effects.push(json!({
                "kind": "improved_shadow_bolt", "trigger_aura": "Improved Shadow Bolt Trigger",
                "aura": aura_label,
                "spell_id": data.improved_shadow_bolt_triggered.highest().id,
                "multiplier": 1.0 + data.improved_shadow_bolt.fraction_at(improved_shadow_bolt),
                "trigger_spells": proc_trigger_spells(sim, unit, masks::SHADOW_BOLT),
            }));
        }
        let shadow_and_flame = talents.i32("shadow_and_flame");
        if shadow_and_flame > 0 {
            // talents_destruction.go applyShadowAndFlame
            effects.push(json!({
                "kind": "shadow_and_flame", "trigger_aura": "Shadow and Flame Trigger",
                "shadow_aura": "Shadow and Flame (Shadow)", "fire_aura": "Shadow and Flame (Fire)",
                "shadow_spell_id": data.shadow_and_flame_triggered.by_id(1293816).id,
                "fire_spell_id": data.shadow_and_flame_triggered.by_id(426311).id,
                "multiplier": 1.0 + data.shadow_and_flame.effect_at(3).fraction_at(shadow_and_flame),
                "trigger_spells": proc_trigger_spells(
                    sim, unit, masks::CONFLAGRATE | masks::SHADOW_BURN),
                "shadow_spells": spells_matching(sim, unit, masks::CONFLAGRATE),
            }));
        }
        effects
    }
}

/// Whether the client row's effect applies an aura: spelldata's `AppliesAura`.
fn applies_aura(effect_type: i32) -> bool {
    crate::prepare::spelldata::effect::applies_aura(effect_type)
}

/// Core's Curse of the Elements debuff (buffs/debuffs_auto_gen.go) parses the client row: a flat
/// change to the resistances its mask names and a damage taken multiplier on its schools,
/// applied on gain and undone on expire. A separate reset simulation activates the aura so the
/// exported values are checked against what Go actually changes.
fn curse_of_elements_effect(
    env: &Environment,
    target: UnitId,
    aura: AuraId,
    unrepresented: &mut Vec<String>,
) -> Value {
    let sim = &env.sim;
    let row = spell_data().curse_of_the_elements.highest();
    let mut resistance: BTreeMap<&'static str, f64> = BTreeMap::new();
    let mut multipliers: BTreeMap<&'static str, f64> = BTreeMap::new();
    let level = sim.unit(env.player).level;
    for effect in &row.effects {
        if !applies_aura(effect.effect_type) {
            continue;
        }
        let value = effect.average(level);
        match effect.aura {
            dbcenums::A_MOD_RESISTANCE => {
                for (_, name, school_mask) in RESISTANCE_STATS {
                    if effect.misc & school_mask != 0 {
                        resistance.insert(name, value);
                    }
                }
            }
            dbcenums::A_MOD_DAMAGE_PERCENT_TAKEN => {
                for (index, name) in SCHOOL_NAMES.iter().enumerate() {
                    if index >= 2 && effect.misc & (1 << (index - 1)) != 0 {
                        multipliers.insert(name, 1.0 + value / 100.0);
                    }
                }
            }
            other => unrepresented.push(format!("Curse of the Elements effect aura {other}")),
        }
    }
    // The raid's Curse of the Elements shares the curse's exclusive category. A permanent member
    // that already holds it, as the raid's own at the same rank does, blocks the warlock's curse
    // from activating at all, which Go still counts as a proc (as sunderBlocked reads it).
    let curse = sim.aura(aura);
    let mut held = false;
    for other in &sim.unit(target).auras {
        let other_aura = sim.aura(*other);
        if *other == aura || curse.tag.is_empty() || other_aura.tag != curse.tag {
            continue;
        }
        if other_aura.duration != NEVER_EXPIRES {
            unrepresented.push(format!(
                "target aura {} shares Curse of the Elements' category",
                other_aura.label
            ));
        } else if other_aura.active {
            held = true;
        }
    }
    let label = curse.label.clone();
    let blocked = held && sunder_blocked(env, &label);
    if held && !blocked {
        unrepresented.push(format!("{label} activates beside a member of its category"));
    }
    if !blocked {
        check_target_aura_changes(env, &label, &resistance, &multipliers, unrepresented);
    }
    let mut effect = json!({
        "kind": "curse_of_the_elements", "spell_id": row.id, "aura": label,
        "resistance_delta": resistance, "school_damage_taken_multiplier": multipliers,
    });
    if blocked {
        effect["blocked"] = json!(true);
    }
    effect
}

/// Whether activating the aura right after a reset fails, as an exclusive category with a
/// stronger active member makes it.
fn sunder_blocked(env: &Environment, label: &str) -> bool {
    let mut fresh = env.fresh();
    let target = fresh.encounter.targets[0];
    let aura = fresh
        .sim
        .get_aura(target, label)
        .expect("the aura exists in every reset");
    fresh.sim.activate(aura);
    !fresh.sim.aura(aura).active
}

/// Activate a target aura in a separate reset simulation and require that it changes exactly the
/// stated resistances, by the stated amounts, and the stated school damage taken multipliers.
fn check_target_aura_changes(
    env: &Environment,
    label: &str,
    resistance: &BTreeMap<&'static str, f64>,
    multipliers: &BTreeMap<&'static str, f64>,
    unrepresented: &mut Vec<String>,
) {
    let mut fresh = env.fresh();
    let unit = fresh.encounter.targets[0];
    let before_stats = fresh.sim.stats(unit);
    let before_pseudo = fresh.sim.unit(unit).pseudo_stats.clone();
    let aura = fresh
        .sim
        .get_aura(unit, label)
        .expect("the aura exists in every reset");
    fresh.sim.activate(aura);
    let after_stats = fresh.sim.stats(unit);
    let mut after_pseudo = fresh.sim.unit(unit).pseudo_stats.clone();
    for stat in Stat::ALL {
        let mut want = before_stats[stat];
        if let Some((_, name, _)) = RESISTANCE_STATS.iter().find(|(s, _, _)| *s == stat) {
            want += resistance.get(name).copied().unwrap_or(0.0);
        }
        if after_stats[stat] != want {
            unrepresented.push(format!(
                "{label} changes target {} unexpectedly",
                stat.name()
            ));
        }
    }
    for (index, name) in SCHOOL_NAMES.iter().enumerate() {
        let slot = crate::prepare::sim::school_array_index(SCHOOL_INDICES[index]);
        let mut want = before_pseudo.school_damage_taken_multiplier[slot];
        if let Some(multiplier) = multipliers.get(name) {
            want *= multiplier;
        }
        if after_pseudo.school_damage_taken_multiplier[slot] != want {
            unrepresented.push(format!(
                "{label} changes target {name} damage taken unexpectedly"
            ));
        }
    }
    after_pseudo.school_damage_taken_multiplier = before_pseudo.school_damage_taken_multiplier;
    if after_pseudo != before_pseudo {
        unrepresented.push(format!("{label} changes other target pseudo stats"));
    }
}

/// curse_of_recklessness.go: core's debuff lowers armor and raises attack power through the
/// per-stat exclusive Minor Armor Reduction category, where a permanent raid Faerie Fire may
/// already hold armor. A separate reset simulation activates it and reports the net change Go
/// makes, which deactivation undoes; anything else it changes is unrepresented, and so is an
/// expiring member of its category.
fn curse_of_recklessness_effect(
    env: &Environment,
    target: UnitId,
    aura: AuraId,
    unrepresented: &mut Vec<String>,
) -> Value {
    let sim = &env.sim;
    let curse = sim.aura(aura);
    for other in &sim.unit(target).auras {
        let other_aura = sim.aura(*other);
        if *other != aura
            && !curse.tag.is_empty()
            && other_aura.tag == curse.tag
            && other_aura.duration != NEVER_EXPIRES
        {
            unrepresented.push(format!(
                "target aura {} shares Curse of Recklessness' category",
                other_aura.label
            ));
        }
    }
    let label = curse.label.clone();
    let mut fresh = env.fresh();
    let unit = fresh.encounter.targets[0];
    let before_stats = fresh.sim.stats(unit);
    let before_pseudo = fresh.sim.unit(unit).pseudo_stats.clone();
    let fresh_aura = fresh
        .sim
        .get_aura(unit, &label)
        .expect("the aura exists in every reset");
    fresh.sim.activate(fresh_aura);
    let after_stats = fresh.sim.stats(unit);
    let after_pseudo = fresh.sim.unit(unit).pseudo_stats.clone();
    let mut effect = json!({
        "kind": "curse_of_recklessness",
        "spell_id": spell_data().curse_of_recklessness.highest().id,
        "aura": label,
        "armor_delta": after_stats[Stat::Armor] - before_stats[Stat::Armor],
    });
    let delta = after_stats[Stat::AttackPower] - before_stats[Stat::AttackPower];
    if delta != 0.0 {
        effect["attack_power_delta"] = json!(delta);
    }
    for stat in Stat::ALL {
        if stat != Stat::Armor
            && stat != Stat::AttackPower
            && after_stats[stat] != before_stats[stat]
        {
            unrepresented.push(format!("{label} changes target {}", stat.name()));
        }
    }
    if after_pseudo != before_pseudo {
        unrepresented.push(format!("{label} changes target pseudo stats"));
    }
    // Deactivation must restore the armor exactly.
    fresh.sim.deactivate(fresh_aura);
    if fresh.sim.stats(unit) != before_stats {
        unrepresented.push(format!("{label} does not restore the target's stats"));
    }
    effect
}

#[allow(dead_code)]
fn _ladder(_: &Ladder) {}
