//! Go sim/core/consumes.go: flasks, elixirs, food, the classic buffs, imbues, scrolls, potions,
//! conjured items and explosives, and the consumable and spell effect rows they read
//! (database.go `ConsumableFromProto`, `GetConsumableByID` and `GetSpellEffectByID`).
//!
//! Closures that only run in a fight (a spell's `ApplyEffects`, a cooldown's `ShouldActivate`,
//! the proc handlers) are not carried; the exporter describes them from the rows and literals
//! this file reads, in `consumable_effects.rs`.

use std::rc::Rc;

use crate::contracts::prepared_v2::ActionId;
use crate::contracts::request::Message;
use crate::data::{proto_row, CONSUMABLES, SPELL_EFFECTS};

use super::aura_helpers::{CallbackMask, HitOutcome, ProcTrigger};
use super::buffs;
use super::character::{cooldown_type, MajorCooldown};
use super::common_effects::GOBLIN_SAPPER_ITEM;
use super::env::Environment;
use super::items::slot;
use super::major_cooldown::COOLDOWN_PRIORITY_LOW;
use super::sim::{
    AuraConfig, AuraId, BuildPhase, Cooldown, Duration, EffectId, Sim, SpellId, TimerId, UnitId,
    UnitType, NEVER_EXPIRES, SECOND,
};
use super::spell::{school, Cast, CastConfig, DefenseType, ProcMask, SpellConfig, SpellFlag};
use super::stats::{Stat, Stats};
use super::Refusal;

const MINUTE: Duration = 60 * SECOND;

/// `proto.ConsumableType` names the sim compares.
const CONSUMABLE_TYPE_POTION: &str = "ConsumableTypePotion";

/// Go `Consumable`.
#[derive(Clone, Debug, Default)]
pub(crate) struct Consumable {
    pub id: i32,
    pub kind: String,
    pub stats: Stats,
    pub name: String,
    pub buff_duration: Duration,
    pub cooldown_duration: Duration,
    pub category_cooldown_duration: Duration,
    pub category_id: i32,
    pub effect_ids: Vec<i32>,
}

/// Go `GetConsumableByID`: the zero consumable for an id the database does not hold.
pub(crate) fn consumable_by_id(id: i32) -> Consumable {
    let Some(row) = proto_row(&CONSUMABLES, "proto.Consumable", i64::from(id)) else {
        return Consumable::default();
    };
    // Go `ConsumableFromProto`.
    Consumable {
        id: row.i32("id"),
        kind: row.enum_name("type"),
        stats: Stats::from_proto_array(&row.f64s("stats")),
        name: row.str("name").to_string(),
        buff_duration: SECOND * Duration::from(row.i32("buff_duration")),
        cooldown_duration: SECOND * Duration::from(row.i32("cooldown_duration")),
        category_cooldown_duration: SECOND * Duration::from(row.i32("category_cooldown_duration")),
        category_id: row.i32("category_id"),
        effect_ids: row
            .ints("effect_ids")
            .into_iter()
            .map(|id| id as i32)
            .collect(),
    }
}

/// `proto.EffectType` numbers.
pub(crate) const EFFECT_TYPE_HEAL: i32 = 10;
pub(crate) const EFFECT_TYPE_RESOURCE_GAIN: i32 = 30;

/// `proto.ResourceType` numbers and names.
pub(crate) const RESOURCE_MANA: i32 = 1;
pub(crate) const RESOURCE_ENERGY: i32 = 2;
pub(crate) const RESOURCE_RAGE: i32 = 3;
pub(crate) const RESOURCE_HEALTH: i32 = 6;

/// Go `proto.SpellEffect`.
#[derive(Clone, Debug)]
pub(crate) struct SpellEffect {
    pub spell_id: i32,
    pub kind: i32,
    pub min_effect_size: f64,
    pub effect_spread: f64,
    /// `GetResourceType`: zero unless the misc value is a resource type.
    pub resource_type: i32,
    pub resource_name: String,
    pub aura_period_ms: i32,
    pub stats: Stats,
    pub duration_ms: i32,
}

/// Go `GetSpellEffectByID`.
pub(crate) fn spell_effect_by_id(id: i32) -> Option<SpellEffect> {
    let row = proto_row(&SPELL_EFFECTS, "proto.SpellEffect", i64::from(id))?;
    let resource = match row.oneof("misc_value0") {
        Some(("resource_type", _)) => row.enum_number("resource_type"),
        _ => 0,
    };
    Some(SpellEffect {
        spell_id: row.i32("spell_id"),
        kind: row.enum_number("type"),
        min_effect_size: row.f64("min_effect_size"),
        effect_spread: row.f64("effect_spread"),
        resource_type: resource,
        resource_name: if resource == 0 {
            "ResourceTypeNone".to_string()
        } else {
            row.enum_name("resource_type")
        },
        aura_period_ms: row.i32("aura_period_ms"),
        stats: Stats::from_proto_array(&row.f64s("stats")),
        duration_ms: row.i32("duration_ms"),
    })
}

/// Go `GetSpellEffectByID` for a row the consumable names: Go dereferences it, so a missing row
/// is a defect there, and a refusal here.
fn required_spell_effect(consumable: &Consumable, id: i32) -> Result<SpellEffect, Refusal> {
    spell_effect_by_id(id).ok_or_else(|| {
        Refusal::new(
            "consumable",
            format!(
                "consumable {} names the missing spell effect {id}",
                consumable.id
            ),
        )
    })
}

/// `stats.Stats{} != stats`.
fn any_stat(stats: &Stats) -> bool {
    !stats.is_zero()
}

/// Go `AlchStoneItemIDs`: empty, as no alchemist stone is in this client's item database.
const ALCH_STONE_ITEM_IDS: [i32; 0] = [];

impl Sim {
    /// Go `Character.HasAlchStone`.
    pub(crate) fn has_alch_stone(&self, unit: UnitId) -> bool {
        let mut equipped = false;
        for item in ALCH_STONE_ITEM_IDS {
            equipped = equipped || self.has_trinket_equipped(unit, item);
        }
        self.has_profession(unit, "Alchemy") && equipped
    }

    /// Go `Character.MHImbueFlatWeaponDamage`.
    pub(crate) fn mh_imbue_flat_weapon_damage(&self, unit: UnitId) -> f64 {
        let consumables = &self.character(unit).consumables;
        imbue_flat_weapon_damage(consumables.i32("mhImbue_id"))
            + imbue_flat_weapon_damage(consumables.i32("mh_temp_enchant_id"))
    }
}

/// Go `imbueFlatWeaponDamage`.
fn imbue_flat_weapon_damage(imbue_id: i32) -> f64 {
    match imbue_id {
        // Dense Sharpening Stone, Dense Weightstone.
        16138 | 16622 => 8.0,
        _ => 0.0,
    }
}

/// Go `StatBuffCategory`.
pub(crate) const STAT_BUFF_CATEGORY: &str = "StatBuff";

/// Go `ExclusiveStatCategory`.
pub(crate) fn exclusive_stat_category(category: &str, key: &str, multiplicative: bool) -> String {
    format!(
        "{category}{key}{}",
        if multiplicative { "Mul" } else { "Add" }
    )
}

impl Sim {
    /// Go `makeExclusiveFlatStatBuff`.
    pub(crate) fn make_exclusive_flat_stat_buff(
        &mut self,
        aura: AuraId,
        stat: Stat,
        value: f64,
        exclusive_category: &str,
    ) -> EffectId {
        let category = exclusive_stat_category(exclusive_category, stat.name(), false);
        self.new_exclusive_effect(
            aura,
            &category,
            false,
            value,
            Some(Rc::new(move |sim: &mut Sim, effect| {
                let unit = sim.aura(sim.effects[effect.0].aura).unit;
                sim.add_stat_dynamic(unit, stat, value);
            })),
            Some(Rc::new(move |sim: &mut Sim, effect| {
                let unit = sim.aura(sim.effects[effect.0].aura).unit;
                sim.add_stat_dynamic(unit, stat, -value);
            })),
        )
    }

    /// Go `registerScrollAura`: scrolls share `StatBuffCategory` with the raid buffs granting
    /// the same stat, so only the strongest source of that stat applies.
    fn register_scroll_aura(
        &mut self,
        unit: UnitId,
        label: &str,
        item_id: i32,
        stat: Stat,
        amount: f64,
    ) -> AuraId {
        let aura = self.get_or_register_aura(
            unit,
            AuraConfig {
                label: label.to_string(),
                action_id: Some(ActionId::item(item_id)),
                duration: NEVER_EXPIRES,
                build_phase: BuildPhase::CONSUMES,
                ..AuraConfig::default()
            },
        );
        self.make_exclusive_flat_stat_buff(aura, stat, amount, STAT_BUFF_CATEGORY);
        self.make_permanent(aura)
    }
}

/// Adds stats against one mob type to every attack table the character owns once the tables
/// exist, as Go's `RegisterPostFinalizeEffect` over `character.AttackTables` does.
fn add_mob_type_bonus_stats(env: &mut Environment, mob_type: &'static str, bonus: Stats) {
    let unit = env.player;
    env.post_finalize
        .push(Rc::new(move |env: &mut Environment| {
            let index = env.sim.unit(unit).unit_index as usize;
            for table in &mut env.attack_tables[index] {
                table
                    .mob_type_bonus_stats
                    .entry(mob_type.to_string())
                    .or_default()
                    .add_inplace(&bonus);
            }
        }));
}

/// Go `consumableCritRiderItems` (MythicSim patch 171): the elixirs whose identical +2% crit rider
/// applies once, however many are taken.
const CONSUMABLE_CRIT_RIDER_ITEMS: [i32; 5] = [13452, 250337, 250338, 250348, 250351];

/// Go `consumableStats` with `ConsumableCritIsAllCrit` on (MythicSim patch 166): a consumable's crit
/// rating with no spell crit of its own counts for spells too, at the same percent.
fn consumable_stats(mut stats: Stats) -> Stats {
    let melee = stats[Stat::MeleeCritRating];
    if melee != 0.0 && stats[Stat::SpellCritRating] == 0.0 {
        stats[Stat::SpellCritRating] = melee
            / super::character::constants::PHYSICAL_CRIT_RATING_PER_CRIT_PERCENT
            * super::character::constants::SPELL_CRIT_RATING_PER_CRIT_PERCENT;
    }
    stats
}

/// Go consumes_forever_potions.go: the Merchant's Favor potions whose effect is not a stat buff
/// (MythicSim patch 167).
pub(crate) const DRAGONFIRE_POTION: i32 = 250953;
pub(crate) const CAUSTIC_SMOG_POTION: i32 = 250951;
pub(crate) const ELEMENTAL_PURGING_POTION: i32 = 250954;

/// Go `applyConsumeEffects`.
pub(crate) fn apply_consume_effects(
    env: &mut Environment,
    _party_buffs: &Message,
) -> Result<(), Refusal> {
    let unit = env.player;
    let consumables = env.sim.character(unit).consumables.clone();

    // Go `addConsumable`: the crit rider counts once (`ConsumableCritRiderStacks` off), then
    // `consumableStats`.
    let mut riders = 0;
    let mut add_consumable_stats = |env: &mut Environment, id: i32| {
        let mut stats = consumable_by_id(id).stats;
        if CONSUMABLE_CRIT_RIDER_ITEMS.contains(&id) {
            if riders > 0 {
                stats[Stat::MeleeCritRating] = 0.0;
            }
            riders += 1;
        }
        env.sim.add_stats(unit, &consumable_stats(stats));
    };

    let flask = consumables.i32("flask_id");
    if flask != 0 {
        add_consumable_stats(env, flask);
    }

    let battle_elixir = consumables.i32("battle_elixir_id");
    if battle_elixir != 0 {
        // Elixir of Demonslaying.
        if battle_elixir == 9224 {
            add_mob_type_bonus_stats(
                env,
                "MobTypeDemon",
                Stats::from_pairs(&[(Stat::AttackPower, 265.0), (Stat::RangedAttackPower, 265.0)]),
            );
        } else {
            add_consumable_stats(env, battle_elixir);
        }
    }

    let guardian_elixir = consumables.i32("guardian_elixir_id");
    if guardian_elixir != 0 {
        // Gift of Arthas.
        if guardian_elixir == 9088 {
            register_gift_of_arthas(env)?;
        } else {
            add_consumable_stats(env, guardian_elixir);
        }
    }

    let food = consumables.i32("food_id");
    if food != 0 {
        add_consumable_stats(env, food);
    }

    // Classic buffs that stack beside the elixirs: jujus, Blasted Lands/Zanza, alcohol, the
    // school power and armor elixirs. Their stats come from the client like every other
    // consumable.
    for field in [
        "strength_buff_id",
        "attack_power_buff_id",
        "zanza_id",
        "alcohol_id",
        "spell_power_elixir_id",
        "school_elixir_id",
        "defense_elixir_id",
    ] {
        let id = consumables.i32(field);
        if id != 0 {
            add_consumable_stats(env, id);
        }
    }
    if consumables.bool("dragonbreath_chili") {
        register_dragonbreath_chili(env);
    }
    // MythicSim Rust-only, beyond the Go reference pin: Oil of Immolation (client 70707).
    if consumables.bool("oil_of_immolation") {
        super::oil_of_immolation::register_oil_of_immolation(env);
    }

    // Static imbues. Forever's Windfury Totem is a party aura, not Era's main-hand enchant
    // (#550), so it no longer displaces a main-hand stone or oil.
    // A stone or oil of its own on the item swap's main-hand weapon (mythicsim patch 133).
    let mh_imbue = consumables.i32("mhImbue_id");
    if !register_per_weapon_main_hand_imbue(env, &consumables) && mh_imbue != 0 {
        register_static_imbue(env, mh_imbue, Hand::Main);
    }
    let oh_imbue = consumables.i32("ohImbue_id");
    if oh_imbue != 0 {
        register_static_imbue(env, oh_imbue, Hand::Off);
    }
    // A stone or oil beside a class imbue on the same hand (mythicsim patch 100).
    let mh_temp_enchant = consumables.i32("mh_temp_enchant_id");
    if mh_temp_enchant != 0 {
        register_static_imbue(env, mh_temp_enchant, Hand::Main);
    }
    let oh_temp_enchant = consumables.i32("oh_temp_enchant_id");
    if oh_temp_enchant != 0 {
        register_static_imbue(env, oh_temp_enchant, Hand::Off);
    }

    // Scrolls.
    for (field, label, item, stat, amount) in [
        (
            "scroll_agi",
            "Scroll of Agility IV",
            10309,
            Stat::Agility,
            17.0,
        ),
        (
            "scroll_str",
            "Scroll of Strength IV",
            10310,
            Stat::Strength,
            17.0,
        ),
        (
            "scroll_int",
            "Scroll of Intellect IV",
            10308,
            Stat::Intellect,
            16.0,
        ),
        (
            "scroll_spi",
            "Scroll of Spirit IV",
            10306,
            Stat::Spirit,
            15.0,
        ),
        (
            "scroll_arm",
            "Scroll of Protection IV",
            10305,
            Stat::Armor,
            240.0,
        ),
    ] {
        if consumables.bool(field) {
            env.sim
                .register_scroll_aura(unit, label, item, stat, amount);
        }
    }

    // Bogling Root: +1 physical damage for 10 min (item 5206, spell 5665).
    if consumables.bool("bogling_root") {
        env.sim.add_stat(unit, Stat::PhysicalDamage, 1.0);
    }

    let explosives_shared_timer = env.sim.new_timer(unit);

    register_potion_cd(env, &consumables)?;
    register_conjured_cd(env, &consumables)?;
    register_explosives_cd(env, &consumables, explosives_shared_timer);
    Ok(())
}

/// Go `registeredBuffs().GiftOfArthasAura`, `RegisterSpell` and `MakeProcTriggerAura`: the
/// guardian elixir that debuffs an enemy it is hit by.
fn register_gift_of_arthas(env: &mut Environment) -> Result<(), Refusal> {
    let unit = env.player;
    env.sim.add_stat(unit, Stat::ShadowResistance, 10.0);
    // Go `NewEnemyAuraArray`: one debuff for each enemy, in unit order.
    for target in env.sim.env_units.clone() {
        if env.sim.unit(target).unit_type == UnitType::Enemy {
            buffs::GIFT_OF_ARTHAS.aura(&mut env.sim, target, true, 0, 0.0);
        }
    }
    env.sim.register_spell(
        unit,
        SpellConfig {
            action_id: ActionId::spell(11374),
            spell_school: school::NATURE,
            proc_mask: ProcMask::EMPTY,
            flat_threat_bonus: 90.0,
            ..SpellConfig::default()
        },
    );
    env.sim.make_proc_trigger_aura(
        unit,
        &ProcTrigger {
            name: "Gift of Arthas - Trigger".to_string(),
            icd: 3 * SECOND,
            proc_chance: 0.3,
            outcome: HitOutcome::LANDED,
            callback: CallbackMask::ON_SPELL_HIT_TAKEN,
            ..ProcTrigger::default()
        },
    );
    Ok(())
}

/// Go `registerDragonbreathChili`: its aura (15852) has a 5% chance, 10 s cooldown, on landed
/// melee hits to cast 15851, 65 Fire damage (+-12.3%, SP coefficient 1) on every enemy, all
/// client values.
fn register_dragonbreath_chili(env: &mut Environment) {
    let unit = env.player;
    env.sim.register_spell(
        unit,
        SpellConfig {
            action_id: ActionId::spell(15851),
            spell_school: school::FIRE,
            defense_type: DefenseType::Magic,
            proc_mask: ProcMask::SPELL_DAMAGE_PROC,
            flags: SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::PASSIVE_SPELL,
            damage_multiplier: 1.0,
            threat_multiplier: 1.0,
            bonus_coefficient: 1.0,
            ..SpellConfig::default()
        },
    );
    env.sim.make_proc_trigger_aura(
        unit,
        &ProcTrigger {
            name: "Dragonbreath Chili".to_string(),
            action_id: ActionId::spell(15852),
            callback: CallbackMask::ON_SPELL_HIT_DEALT,
            proc_mask: ProcMask::MELEE,
            outcome: HitOutcome::LANDED,
            proc_chance: 0.05,
            icd: 10 * SECOND,
            ..ProcTrigger::default()
        },
    );
}

#[derive(Clone, Copy)]
enum Hand {
    Main,
    Off,
}

/// Go imbue_swap.go `staticImbueStats`: the stats a stone or oil `registerStaticImbue` adds as
/// plain stats gives.
fn static_imbue_stats(imbue_id: i32) -> Stats {
    match imbue_id {
        25123 => Stats::from_pairs(&[(Stat::HealingPower, 30.0), (Stat::MP5, 15.0)]),
        25122 | 20749 => Stats::from_pairs(&[
            (Stat::SpellDamage, 36.0),
            (Stat::HealingPower, 36.0),
            (Stat::SpellCritPercent, 1.0),
        ]),
        25121 => Stats::from_pairs(&[(Stat::SpellDamage, 24.0)]),
        22756 | 18262 => Stats::from_pairs(&[
            (Stat::PhysicalCritPercent, 2.0),
            (Stat::RangedCritPercent, -2.0),
        ]),
        _ => Stats::default(),
    }
}

/// Go imbue_swap.go `registerPerWeaponMainHandImbue`: `mh_imbue_id` on the weapon worn at the
/// pull and `mh_imbue_swap_id` on the item swap's main hand, each counting only while its weapon
/// is in the main hand: a stat stone or oil as an aura, a proc oil as its proc aura switched on and
/// off. Any other stone counts all fight. False, leaving the main hand to `registerStaticImbue`,
/// when `mh_imbue_swap_id` is unset or the swap has no main-hand weapon of its own.
fn register_per_weapon_main_hand_imbue(env: &mut Environment, consumables: &Message) -> bool {
    let unit = env.player;
    let swap_imbue = consumables.i32("mh_imbue_swap_id");
    let Some(swap) = env.sim.item_swap(unit) else {
        return false;
    };
    if swap_imbue == 0 {
        return false;
    }
    let pull_id = env.sim.character(unit).equipment[slot::MAIN_HAND].id;
    let swap_id = swap.un_equipped[slot::MAIN_HAND].id;
    if swap_id == 0 || swap_id == pull_id {
        return false;
    }
    // Each stone with the auras it counts through and whether each weapon carries it.
    let mut effects: Vec<(i32, Vec<AuraId>, bool, bool)> = Vec::new();
    for (id, pull, swap) in [
        (consumables.i32("mhImbue_id"), true, false),
        (swap_imbue.max(0), false, true),
    ] {
        if id == 0 {
            continue;
        }
        if let Some(effect) = effects.iter_mut().find(|effect| effect.0 == id) {
            // The same stone on both weapons: one effect, in hand all fight.
            effect.2 |= pull;
            effect.3 |= swap;
            continue;
        }
        let flat = imbue_flat_weapon_damage(id);
        let mut known = flat != 0.0;
        let mut auras = Vec::new();
        let stats = static_imbue_stats(id);
        if !stats.is_zero() {
            known = true;
            let mut action = ActionId::item(id);
            action.tag = 133;
            let aura = env.sim.new_temporary_stats_aura(
                unit,
                &format!("Main Hand Imbue {id}"),
                &action,
                stats,
                NEVER_EXPIRES,
            );
            auras.push(aura.aura);
        }
        if let Some(oil) = super::proc_oil::static_imbue_proc(id) {
            known = true;
            let before = env.sim.unit(unit).auras.len();
            super::proc_oil::register_proc_oil(env, oil, ProcMask::MELEE_MH);
            auras.extend(env.sim.unit(unit).auras[before..].iter().copied());
        }
        if !known {
            register_static_imbue(env, id, Hand::Main);
            continue;
        }
        if flat != 0.0 {
            // A Dense stone's damage on its weapon's swing, which the swap rebuilds.
            env.sim
                .item_swap_unported(unit, format!("the flat weapon damage of stone {id}"));
        }
        effects.push((id, auras, pull, swap));
    }
    env.sim.register_item_swap_callback(
        unit,
        &[slot::MAIN_HAND],
        super::item_swap::SwapCallback::MainHandImbue {
            pull_id,
            swap_id,
            effects: effects
                .into_iter()
                .map(|(_, auras, pull, swap)| (auras, pull, swap))
                .collect(),
        },
    );
    true
}

/// Go `registerStaticImbue`.
fn register_static_imbue(env: &mut Environment, imbue_id: i32, hand: Hand) {
    let unit = env.player;
    // `StaticImbueProcs`: an imbue whose effect is a combat proc.
    if let Some(oil) = super::proc_oil::static_imbue_proc(imbue_id) {
        super::proc_oil::register_proc_oil(
            env,
            oil,
            match hand {
                Hand::Main => ProcMask::MELEE_MH,
                Hand::Off => ProcMask::MELEE_OH,
            },
        );
    }
    match imbue_id {
        // Mana Oil.
        25123 => {
            env.sim.add_stat(unit, Stat::HealingPower, 30.0);
            env.sim.add_stat(unit, Stat::MP5, 15.0);
        }
        // Brilliant Wizard Oil (20749: the item id Forever saved before the merge).
        25122 | 20749 => {
            env.sim.add_stat(unit, Stat::SpellDamage, 36.0);
            env.sim.add_stat(unit, Stat::HealingPower, 36.0);
            env.sim.add_stat(unit, Stat::SpellCritPercent, 1.0);
        }
        // Wizard Oil: 24 in the client (enchant 2627, spell 25111), reverted 2026-09-24.
        25121 => env.sim.add_stat(unit, Stat::SpellDamage, 24.0),
        // Elemental Sharpening Stone (18262: the item id Forever saved before the merge).
        22756 | 18262 => {
            // RangedCritPercent is the ranged offset from PhysicalCritPercent, so the
            // melee-only crit has to be cancelled there.
            env.sim.add_stat(unit, Stat::PhysicalCritPercent, 2.0);
            env.sim.add_stat(unit, Stat::RangedCritPercent, -2.0);
        }
        // Blessed Wizard Oil.
        28898 => add_mob_type_bonus_stats(
            env,
            "MobTypeUndead",
            Stats::from_pairs(&[(Stat::SpellDamage, 58.0)]),
        ),
        // Consecrated Sharpening Stone.
        28891 => add_mob_type_bonus_stats(
            env,
            "MobTypeUndead",
            Stats::from_pairs(&[(Stat::AttackPower, 100.0), (Stat::RangedAttackPower, 100.0)]),
        ),
        _ => {}
    }

    let flat = imbue_flat_weapon_damage(imbue_id);
    if flat != 0.0 {
        let attacks = &mut env.sim.unit_mut(unit).auto_attacks;
        let weapon = match hand {
            Hand::Main => &mut attacks.mh,
            Hand::Off => &mut attacks.oh,
        };
        weapon.base_damage_min += flat;
        weapon.base_damage_max += flat;
    }
}

/// Go `resourceGainConfig`: one resource effect of a potion or conjured item.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ResourceGain {
    pub resource: i32,
    pub min: f64,
    pub spread: f64,
    /// Duration between ticks; 0 for one-shot gains.
    pub period: Duration,
    /// Total duration of periodic gains.
    pub duration: Duration,
}

/// Go `makePotionActivationSpellInternal`'s cooldown type for one resource gain.
fn resource_cooldown_type(current: u32, resource: i32) -> u32 {
    if resource == RESOURCE_MANA && current != cooldown_type::SURVIVAL {
        cooldown_type::MANA
    } else if resource == RESOURCE_HEALTH {
        cooldown_type::SURVIVAL
    } else {
        cooldown_type::DPS
    }
}

/// Go `registerPotionCD`.
fn register_potion_cd(env: &mut Environment, consumes: &Message) -> Result<(), Refusal> {
    let unit = env.player;
    let default_potion = consumes.i32("pot_id");
    for potion_id in consumes.ints("potions") {
        let potion = consumable_by_id(potion_id as i32);
        if potion.kind == CONSUMABLE_TYPE_POTION {
            let mcd = make_potion_activation_spell(env, potion.id)?;
            if default_potion == potion.id {
                env.sim.spell_mut(mcd.spell).flags |= SpellFlag::COMBAT_POTION;
                env.sim.add_major_cooldown(unit, mcd);
            }
        }
    }
    Ok(())
}

/// Go `makePotionActivationSpell`.
fn make_potion_activation_spell(env: &mut Environment, id: i32) -> Result<MajorCooldown, Refusal> {
    let potion = consumable_by_id(id);
    let mcd = make_potion_activation_spell_internal(env, &potion)?;
    // Marked 'Encounter Only' so that users select the generic Potion placeholder action
    // instead of specific potion spells in APL prepull.
    env.sim.spell_mut(mcd.spell).flags |=
        SpellFlag::ENCOUNTER_ONLY | SpellFlag::POTION | SpellFlag::APL;
    Ok(mcd)
}

/// Go `makePotionActivationSpellInternal`.
fn make_potion_activation_spell_internal(
    env: &mut Environment,
    potion: &Consumable,
) -> Result<MajorCooldown, Refusal> {
    let unit = env.player;
    let cooldown_duration = if potion.cooldown_duration > 0 {
        potion.cooldown_duration
    } else {
        2 * MINUTE
    };
    let cd_timer = env.sim.new_timer(unit);
    let shared_timer = env.sim.category_timer(unit, 4);
    let action_id = ActionId::item(potion.id);
    let spell = env.sim.get_or_register_spell(
        unit,
        SpellConfig {
            action_id: action_id.clone(),
            flags: SpellFlag::NO_ON_CAST_COMPLETE,
            cast: CastConfig {
                cd: Cooldown {
                    timer: Some(cd_timer),
                    duration: cooldown_duration,
                },
                shared_cd: Cooldown {
                    timer: Some(shared_timer),
                    duration: cooldown_duration,
                },
                ..CastConfig::default()
            },
            ..SpellConfig::default()
        },
    );
    let mut mcd = MajorCooldown {
        spell,
        priority: 0,
        cooldown_type: cooldown_type::UNKNOWN,
        allow_spell_queueing: false,
        timings: Vec::new(),
    };
    if let Some(related_aura) = forever_potion_effect(env, potion.id) {
        mcd.cooldown_type = cooldown_type::DPS;
        env.sim.spell_mut(spell).related_self_buff = related_aura;
        return Ok(mcd);
    }
    if potion.buff_duration > 0 {
        // Add stat buff aura if applicable.
        let aura = env.sim.new_temporary_stats_aura(
            unit,
            &potion.name,
            &action_id,
            potion.stats,
            potion.buff_duration,
        );
        env.sim.spell_mut(spell).related_self_buff = Some(aura.aura);
        mcd.cooldown_type = aura.infer_cd_type();
        // Go `mcd.BuffAura = aura`.
        let buffs = &mut env.sim.character_mut(unit).cooldown_buff_auras;
        buffs.retain(|(known, _)| *known != spell);
        buffs.push((spell, aura));
    }

    // Stats applied by triggered auras (e.g. Fel Mana Potion's spell damage reduction). These
    // may be positive or negative.
    let mut aura_stats = Stats::default();
    let mut aura_duration: Duration = 0;
    let mut aura_spell_id = 0;
    for effect_id in &potion.effect_ids {
        let e = required_spell_effect(potion, *effect_id)?;
        let is_periodic = e.aura_period_ms > 0;
        if e.resource_type != 0 && (is_periodic || e.kind == EFFECT_TYPE_RESOURCE_GAIN) {
            mcd.cooldown_type = resource_cooldown_type(mcd.cooldown_type, e.resource_type);
            continue;
        }
        if any_stat(&e.stats) {
            aura_stats = aura_stats.add(&e.stats);
            aura_duration =
                aura_duration.max(Duration::from(e.duration_ms) * super::sim::MILLISECOND);
            if aura_spell_id == 0 {
                aura_spell_id = e.spell_id;
            }
        }
    }

    if aura_duration > 0 && any_stat(&aura_stats) {
        env.sim.new_temporary_stats_aura(
            unit,
            &format!("{} Debuff ({aura_spell_id})", potion.name),
            &ActionId::spell(aura_spell_id),
            aura_stats,
            aura_duration,
        );
    }
    Ok(mcd)
}

/// Go `Character.foreverPotionEffect`: registers what one of the Forever potions does when drunk
/// and returns the aura to show as its buff, `Some(None)` for a potion without one; `None` for
/// every other potion.
fn forever_potion_effect(env: &mut Environment, item_id: i32) -> Option<Option<AuraId>> {
    let unit = env.player;
    let damage_spell =
        |spell_id: i32, spell_school: u8, label: &str, tick_length, ticks| SpellConfig {
            action_id: ActionId::spell(spell_id),
            spell_school,
            defense_type: DefenseType::Magic,
            proc_mask: ProcMask::EMPTY,
            flags: SpellFlag::NO_ON_CAST_COMPLETE | SpellFlag::PASSIVE_SPELL,
            damage_multiplier: 1.0,
            threat_multiplier: 1.0,
            dot: super::spell::DotConfig {
                aura: AuraConfig {
                    label: label.to_string(),
                    ..AuraConfig::default()
                },
                tick_length,
                number_of_ticks: ticks,
                ..super::spell::DotConfig::default()
            },
            ..SpellConfig::default()
        };
    match item_id {
        // newDragonfirePotionSpell (1252554): 280 Fire at once, then 35 every 2 sec for 8 sec.
        DRAGONFIRE_POTION => {
            let config = damage_spell(1252554, school::FIRE, "Dragonfire Potion", 2 * SECOND, 4);
            env.sim.get_or_register_spell(unit, config);
            Some(None)
        }
        // newCausticSmogPotionSpell (1252543): 33 Nature every sec for 12 sec.
        CAUSTIC_SMOG_POTION => {
            let config = damage_spell(1252543, school::NATURE, "Caustic Smog Potion", SECOND, 12);
            env.sim.get_or_register_spell(unit, config);
            Some(None)
        }
        // newElementalPurgingAura: 105 spell damage against Elementals for 5 min.
        ELEMENTAL_PURGING_POTION => {
            let noop: super::sim::AuraCallback = Rc::new(|_: &mut Sim, _| {});
            let aura = env.sim.get_or_register_aura(
                unit,
                AuraConfig {
                    label: "Potion of Elemental Purging".to_string(),
                    action_id: Some(ActionId::item(ELEMENTAL_PURGING_POTION)),
                    duration: 300 * SECOND,
                    on_gain: Some(noop.clone()),
                    on_expire: Some(noop),
                    ..AuraConfig::default()
                },
            );
            Some(Some(aura))
        }
        _ => None,
    }
}

/// Go `registerConjuredCD`.
fn register_conjured_cd(env: &mut Environment, consumes: &Message) -> Result<(), Refusal> {
    let unit = env.player;
    for conjured_id in consumes.ints("conjured_items") {
        // The UI sends its whole eligible list, unfiltered by the consumable database.
        if consumable_by_id(conjured_id as i32).id == 0 {
            continue;
        }
        let mcd = make_conjured_activation_spell(env, conjured_id as i32)?;
        env.sim.add_major_cooldown(unit, mcd);
    }
    Ok(())
}

/// Go `makeConjuredActivationSpell`.
fn make_conjured_activation_spell(
    env: &mut Environment,
    conjured_id: i32,
) -> Result<MajorCooldown, Refusal> {
    let conjured = consumable_by_id(conjured_id);
    let mcd = make_conjured_activation_spell_internal(env, &conjured)?;
    env.sim.spell_mut(mcd.spell).flags |= SpellFlag::CONJURED | SpellFlag::APL;
    Ok(mcd)
}

/// Go `makeConjuredActivationSpellInternal`.
fn make_conjured_activation_spell_internal(
    env: &mut Environment,
    conjured: &Consumable,
) -> Result<MajorCooldown, Refusal> {
    let unit = env.player;
    let cooldown_duration = if conjured.cooldown_duration > 0 {
        conjured.cooldown_duration
    } else {
        2 * MINUTE
    };
    let cd_timer = env.sim.new_timer(unit);
    let shared_timer = env.sim.category_timer(unit, conjured.category_id);
    let action_id = ActionId::item(conjured.id);
    let spell = env.sim.get_or_register_spell(
        unit,
        SpellConfig {
            action_id: action_id.clone(),
            flags: SpellFlag::NO_ON_CAST_COMPLETE,
            cast: CastConfig {
                cd: Cooldown {
                    timer: Some(cd_timer),
                    duration: cooldown_duration,
                },
                shared_cd: Cooldown {
                    timer: Some(shared_timer),
                    duration: 2 * MINUTE,
                },
                ..CastConfig::default()
            },
            ..SpellConfig::default()
        },
    );
    let mut mcd = MajorCooldown {
        spell,
        priority: 0,
        cooldown_type: cooldown_type::UNKNOWN,
        allow_spell_queueing: false,
        timings: Vec::new(),
    };
    if conjured.buff_duration > 0 {
        // Add stat buff aura if applicable.
        let aura = env.sim.new_temporary_stats_aura(
            unit,
            &conjured.name,
            &action_id,
            conjured.stats,
            conjured.buff_duration,
        );
        env.sim.spell_mut(spell).related_self_buff = Some(aura.aura);
        mcd.cooldown_type = aura.infer_cd_type();
    }
    for effect_id in &conjured.effect_ids {
        let e = required_spell_effect(conjured, *effect_id)?;
        if (e.kind == EFFECT_TYPE_RESOURCE_GAIN || e.kind == EFFECT_TYPE_HEAL)
            && e.resource_type != 0
        {
            mcd.cooldown_type = resource_cooldown_type(mcd.cooldown_type, e.resource_type);
        }
    }
    Ok(mcd)
}

const EZ_THRO_DYNAMITE_TWO_ITEM: i32 = 18588;
const CRYSTAL_CHARGE_ITEM: i32 = 11566;
const THORIUM_GRENADE_ITEM: i32 = 15993;
const DENSE_DYNAMITE_ITEM: i32 = 18641;
const CRYOBLAST_ITEM: i32 = 217495;

/// Scroll of Cryoblast's use spell, the value the explosives picker saves for it.
const CRYOBLAST_SPELL_ID: i32 = 440212;

/// One basic explosive: the item it registers, its school, damage roll and missile speed. The
/// exporter restates this table (explosives.go `basicExplosives`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BasicExplosive {
    pub item_id: i32,
    pub school: u8,
    pub min: f64,
    pub max: f64,
    pub speed: f64,
}

/// Forever's SAF-T / EZ-Thro bombs, keyed on their use spell (the picker's value). ItemSparse
/// (70009) gives them required level 1 and no RequiredSkill, so anyone can throw them. Each is
/// a 1 s cast of Fire damage shared on the 1 min explosives category; damage is the client's
/// base points +- half its variance, with no spell power coefficient. EZ-Thro Bronze Mortar
/// (own 10 min cooldown) and EZ-Thro Mana Bomb (a mana burn) are left out.
const SAFT_BOMBS: [(i32, BasicExplosive); 11] = [
    (1269161, bomb(260793, 22.0, 28.0, 14.0)), // SAF-T Copper Bomb: 25, variance .24
    (1269155, bomb(260792, 26.0, 34.0, 14.0)), // SAF-T Dynamite: 30, .267
    (1269192, bomb(260795, 43.0, 57.0, 14.0)), // EZ-Thro Copper Bomb XL: 50, .28
    (1269216, bomb(260797, 73.0, 97.0, 14.0)), // SAF-T Bronze Bomb: 85, .282
    (1269264, bomb(260798, 128.0, 172.0, 14.0)), // SAF-T Jumbo Dynamite: 150, .293
    (1269272, bomb(260805, 149.0, 201.0, 14.0)), // SAF-T Bomb: 175, .297
    (1269278, bomb(260809, 149.0, 201.0, 14.0)), // Tru-Trigger Frag Bomb: 175, .297
    (1269270, bomb(260803, 213.0, 287.0, 14.0)), // EZ-Thro Grenade: 250, .296
    (1269282, bomb(260814, 340.0, 460.0, 14.0)), // SAF-T Clever Dynamite: 400, .3
    (1269330, bomb(260816, 300.0, 500.0, 25.0)), // EZ-Thro Thorium Grenade: 400, .5
    (1269334, bomb(260817, 225.0, 675.0, 14.0)), // EZ-Thro Dark Bomb: 450, 1
];

const fn bomb(item_id: i32, min: f64, max: f64, speed: f64) -> BasicExplosive {
    BasicExplosive {
        item_id,
        school: school::FIRE,
        min,
        max,
        speed,
    }
}

/// The Goblin Sapper Charge: its damage roll is the exporter's `goblin_sapper` literals.
const GOBLIN_SAPPER: BasicExplosive = bomb(GOBLIN_SAPPER_ITEM, 450.0, 750.0, 0.0);

/// The named explosives of consumes.go, by item id: the constructors `newEzThroDynamiteTwoSpell`,
/// `newCrystalChargeSpell`, `newThoriumGrenadeSpell`, `newDenseDynamiteSpell` and
/// `newCryoblastSpell`, and the Goblin Sapper Charge.
const NAMED_EXPLOSIVES: [BasicExplosive; 5] = [
    bomb(EZ_THRO_DYNAMITE_TWO_ITEM, 213.0, 287.0, 14.0),
    bomb(CRYSTAL_CHARGE_ITEM, 383.0, 517.0, 0.0),
    bomb(THORIUM_GRENADE_ITEM, 300.0, 500.0, 25.0),
    bomb(DENSE_DYNAMITE_ITEM, 340.0, 460.0, 14.0),
    // Client 1.60.1.70009: 215 Frost damage with 0.2977 variance (183 - 247) in 5 yards and no
    // spell power coefficient. Beta logs agree: 23 non-crit hits of two level 20 mages
    // average 209.
    BasicExplosive {
        item_id: CRYOBLAST_ITEM,
        school: school::FROST,
        min: 183.0,
        max: 247.0,
        speed: 0.0,
    },
];

/// The basic explosive a spell of this item id is, or `None`: the exporter's `basicExplosives`.
pub(crate) fn basic_explosive(item_id: i32) -> Option<BasicExplosive> {
    NAMED_EXPLOSIVES
        .iter()
        .copied()
        .chain(SAFT_BOMBS.iter().map(|(_, bomb)| *bomb))
        .find(|explosive| explosive.item_id == item_id)
}

impl Sim {
    /// Go `newBasicExplosiveSpellConfig`. A Goblin Sapper Charge registers the half of the
    /// charge that goes off in the thrower's face first, as Go does.
    fn new_basic_explosive_spell_config(
        &mut self,
        unit: UnitId,
        shared_timer: TimerId,
        explosive: BasicExplosive,
        cast_time: Duration,
        cooldown: Cooldown,
    ) -> SpellConfig {
        let BasicExplosive {
            item_id,
            school,
            speed,
            ..
        } = explosive;
        let action_id = ActionId::item(item_id);
        if action_id == ActionId::item(GOBLIN_SAPPER_ITEM) {
            self.new_sapper_self_damage_spell(unit, &action_id, school);
        }
        SpellConfig {
            action_id,
            spell_school: school,
            // Every explosive's damage spell is Magic in SpellCategories, so they crit for 150%.
            defense_type: DefenseType::Magic,
            proc_mask: ProcMask::EMPTY,
            flags: SpellFlag::EXPLOSIVE,
            missile_speed: speed,
            cast: CastConfig {
                default_cast: Cast {
                    cast_time,
                    ..Cast::default()
                },
                cd: cooldown,
                shared_cd: Cooldown {
                    timer: Some(shared_timer),
                    duration: MINUTE,
                },
                ..CastConfig::default()
            },
            // Explosives always have 1% resist chance, so just give them hit cap.
            bonus_hit_percent: 100.0,
            damage_multiplier: 1.0,
            threat_multiplier: 1.0,
            ..SpellConfig::default()
        }
    }

    /// Go `newSapperSelfDamageSpell`: the half of a sapper charge that goes off in the thrower's
    /// face, its own spell so that the hit carries the kind it is.
    fn new_sapper_self_damage_spell(
        &mut self,
        unit: UnitId,
        action_id: &ActionId,
        school: u8,
    ) -> SpellId {
        self.get_or_register_spell(
            unit,
            SpellConfig {
                action_id: ActionId {
                    tag: 1,
                    ..action_id.clone()
                },
                spell_school: school,
                defense_type: DefenseType::Magic,
                proc_mask: ProcMask::SPELL_DAMAGE,
                flags: SpellFlag::EXPLOSIVE,
                bonus_hit_percent: 100.0,
                damage_multiplier: 1.0,
                threat_multiplier: 1.0,
                ..SpellConfig::default()
            },
        )
    }

    /// One of consumes.go's `new...Spell` constructors for a basic explosive of the table.
    fn new_explosive_spell(
        &mut self,
        unit: UnitId,
        shared_timer: TimerId,
        explosive: BasicExplosive,
        cast_time: Duration,
        cooldown: Cooldown,
    ) -> SpellId {
        let config = self.new_basic_explosive_spell_config(
            unit,
            shared_timer,
            explosive,
            cast_time,
            cooldown,
        );
        self.get_or_register_spell(unit, config)
    }
}

/// Go `registerExplosivesCD`.
fn register_explosives_cd(env: &mut Environment, consumes: &Message, shared_timer: TimerId) {
    let unit = env.player;
    let engineer = env.sim.has_profession(unit, "Engineering");
    let explosive_id = consumes.i32("explosive_id");
    let sim = &mut env.sim;

    if consumes.bool("goblinSapper") && engineer {
        // Go builds the cooldown's timer before the config, as an argument of the call.
        let cd = Cooldown {
            timer: Some(sim.new_timer(unit)),
            duration: 5 * MINUTE,
        };
        let config = sim.new_basic_explosive_spell_config(unit, shared_timer, GOBLIN_SAPPER, 0, cd);
        let spell = sim.get_or_register_spell(unit, config);
        sim.add_major_cooldown(
            unit,
            MajorCooldown {
                spell,
                priority: COOLDOWN_PRIORITY_LOW + 20,
                cooldown_type: cooldown_type::DPS | cooldown_type::EXPLOSIVE,
                allow_spell_queueing: false,
                timings: Vec::new(),
            },
        );
    }

    let named = |item: i32| basic_explosive(item).expect("a named explosive");
    let filler = if let Some((_, bomb)) = SAFT_BOMBS.iter().find(|(id, _)| *id == explosive_id) {
        Some(sim.new_explosive_spell(unit, shared_timer, *bomb, SECOND, Cooldown::default()))
    } else if explosive_id == CRYOBLAST_SPELL_ID {
        // A mage's vendor scroll, not an engineer's bomb, but it shares their 1 min cooldown.
        (sim.character(unit).class == "ClassMage").then(|| {
            sim.new_explosive_spell(
                unit,
                shared_timer,
                named(CRYOBLAST_ITEM),
                0,
                Cooldown::default(),
            )
        })
    } else if explosive_id == 18588 {
        // Ez-Thro Dynamite II and Crystal Charge state no RequiredSkill in ItemSparse (70009):
        // anyone can throw them.
        Some(sim.new_explosive_spell(
            unit,
            shared_timer,
            named(EZ_THRO_DYNAMITE_TWO_ITEM),
            SECOND,
            Cooldown::default(),
        ))
    } else if explosive_id == 15239 {
        Some(sim.new_explosive_spell(
            unit,
            shared_timer,
            named(CRYSTAL_CHARGE_ITEM),
            0,
            Cooldown::default(),
        ))
    } else if !engineer {
        None
    } else if explosive_id == 19769 {
        Some(sim.new_explosive_spell(
            unit,
            shared_timer,
            named(THORIUM_GRENADE_ITEM),
            SECOND,
            Cooldown::default(),
        ))
    } else if explosive_id == 23063 || explosive_id == 18641 {
        // 18641: the item id Forever saved before the merge.
        Some(sim.new_explosive_spell(
            unit,
            shared_timer,
            named(DENSE_DYNAMITE_ITEM),
            SECOND,
            Cooldown::default(),
        ))
    } else {
        None
    };
    if let Some(spell) = filler {
        sim.add_major_cooldown(
            unit,
            MajorCooldown {
                spell,
                priority: COOLDOWN_PRIORITY_LOW + 10,
                cooldown_type: cooldown_type::DPS | cooldown_type::EXPLOSIVE,
                allow_spell_queueing: false,
                timings: Vec::new(),
            },
        );
    }
}

#[cfg(test)]
mod tests;
