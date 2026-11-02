//! Go sim/common/shared/spell_data_aura.go and spell_data_debuff_proc.go: item and enchant effects
//! whose spell applies auras to the wearer, its pets and an enemy.

use std::rc::Rc;

use crate::data::spells::Spell;

use super::aura_helpers::StatBuffAura;
use super::env::Environment;
use super::item_aura::equip_aura_row;
use super::parse_effects::{parse_effects, ParseOptions};
use super::resolve_aura::{aura_config, label};
use super::shared_items::{has_item_equipped, spell_data_proc_listener, SpellDataProc};
use super::sim::{
    AuraConfig, AuraId, Duration, EffectCallback, Sim, UnitId, UnitType, NEVER_EXPIRES,
};
use super::spell::{ProcMask, SpellConfig};
use super::spelldata::area::AreaType;
use super::spelldata::effect::{AURA_ON_ENEMY, AURA_ON_PET, AURA_ON_WEARER};
use super::stats::Stat;

/// An aura on each unit, by unit index: Go `core.AuraArray`.
pub(crate) type AuraArray = Vec<Option<AuraId>>;

/// Go `core.SlowedTimeMultiplier`: a slow the client states as a negative speed percentage makes
/// the time between attacks, or a cast time, that much longer: -20 is 20% longer, which divides
/// the speed by 1.2.
pub(crate) fn slowed_time_multiplier(speed_percent: f64) -> f64 {
    1.0 - speed_percent / 100.0
}

impl Sim {
    /// Go `core.CastSpeedReductionEffect`: a slow on casts alone, in the category Slow's cast
    /// and ranged slow takes: only the strongest applies.
    pub(crate) fn cast_speed_reduction_effect(&mut self, aura: AuraId, cast_time_multiplier: f64) {
        let on_gain: EffectCallback = Rc::new(move |sim: &mut Sim, effect| {
            let unit = sim.aura(sim.effects[effect.0].aura).unit;
            sim.multiply_cast_speed(unit, 1.0 / cast_time_multiplier);
        });
        let on_expire: EffectCallback = Rc::new(move |sim: &mut Sim, effect| {
            let unit = sim.aura(sim.effects[effect.0].aura).unit;
            sim.multiply_cast_speed(unit, cast_time_multiplier);
        });
        self.new_exclusive_effect(
            aura,
            "CastSpdReduction",
            false,
            1.0 - 1.0 / cast_time_multiplier,
            Some(on_gain),
            Some(on_expire),
        );
    }

    /// Go `core.AtkSpeedReductionEffect`: how far from 1 the applied factor is is the scale
    /// every member of the category bids on: a 20% slow outbids a 10% one.
    pub(crate) fn atk_speed_reduction_effect(&mut self, aura: AuraId, speed_multiplier: f64) {
        let on_gain: EffectCallback = Rc::new(move |sim: &mut Sim, effect| {
            let unit = sim.aura(sim.effects[effect.0].aura).unit;
            sim.multiply_attack_speed(unit, 1.0 / speed_multiplier);
        });
        let on_expire: EffectCallback = Rc::new(move |sim: &mut Sim, effect| {
            let unit = sim.aura(sim.effects[effect.0].aura).unit;
            sim.multiply_attack_speed(unit, speed_multiplier);
        });
        self.new_exclusive_effect(
            aura,
            "AtkSpdReduction",
            false,
            1.0 - 1.0 / speed_multiplier,
            Some(on_gain),
            Some(on_expire),
        );
    }
}

/// Go `spellDataAuras`: the auras a row puts on the wearer, on each of its pets and on an enemy,
/// each carrying the effects that land on that unit. The wearer's and the pets' are registered
/// from the config handed in; the enemy's is the row's own aura, one per enemy, shared by every
/// character that applies it, so the first to register it parses it. A row restricted to an
/// area puts nothing on anyone in an encounter outside it.
pub(crate) struct SpellDataAuras {
    pub wearer: Option<AuraId>,
    pub wearer_stats: Vec<Stat>,
    pub pets: Vec<AuraId>,
    pub enemies: Option<AuraArray>,
}

/// Go `Encounter.InArea`: whether the encounter is in an area of this type.
pub(crate) fn in_area(env: &Environment, area: AreaType) -> bool {
    env.encounter
        .area_types
        .iter()
        .any(|name| name == area.name())
}

/// Go `newSpellDataAuras`. A pet's aura is not registered: a request with a pet is refused.
pub(crate) fn new_spell_data_auras(
    env: &mut Environment,
    row: &'static Spell,
    config: AuraConfig,
) -> SpellDataAuras {
    let mut auras = SpellDataAuras {
        wearer: None,
        wearer_stats: Vec::new(),
        pets: Vec::new(),
        enemies: None,
    };
    let unit = env.player;
    if row.required_areas != 0 && !in_area(env, row.area_type()) {
        return auras;
    }

    let on_wearer = super::spelldata::effect::effects_on(row, AURA_ON_WEARER);
    if !on_wearer.is_empty() {
        let aura = env.sim.register_aura(unit, config.clone());
        let parsed = parse_effects(
            &mut env.sim,
            Some(unit),
            aura,
            row,
            ParseOptions {
                only: on_wearer,
                ..ParseOptions::default()
            },
        );
        auras.wearer = Some(aura);
        auras.wearer_stats = parsed.stats;
    }

    // The effects on a pet: an aura on each pet that is not a guardian, carrying the effects
    // that land on it.
    let on_pet = super::spelldata::effect::effects_on(row, AURA_ON_PET);
    if !on_pet.is_empty() {
        for pet in env.sim.unit(unit).pets.clone() {
            if env.sim.pet_data(pet).is_guardian {
                continue;
            }
            let aura = env.sim.register_aura(pet, config.clone());
            parse_effects(
                &mut env.sim,
                Some(pet),
                aura,
                row,
                ParseOptions {
                    only: on_pet.clone(),
                    ..ParseOptions::default()
                },
            );
            auras.pets.push(aura);
        }
    }

    let on_enemy = super::spelldata::effect::effects_on(row, AURA_ON_ENEMY);
    if !on_enemy.is_empty() {
        auras.enemies = Some(enemy_debuff_auras(env, row, config.duration, &on_enemy));
    }
    auras
}

impl SpellDataAuras {
    /// The wearer's aura as a stat buff aura carrying the stats the parse attached.
    pub(crate) fn wearer_stat_buff(&self) -> Option<StatBuffAura> {
        self.wearer
            .map(|aura| StatBuffAura::new(aura, self.wearer_stats.clone()))
    }
}

/// Go `enemyDebuffAuras`: the row's debuff on each enemy, carrying the effects at the given
/// positions, for the duration given. The aura is the row's rather than the wearer's, so every
/// character applying the row refreshes one debuff on the target, and the first to register it
/// parses it. Each slow takes its exclusive category, attacks Thunder Clap's and casts Slow's,
/// where only the strongest applies; every other effect applies to the target as the row states
/// it, per stack.
pub(crate) fn enemy_debuff_auras(
    env: &mut Environment,
    row: &'static Spell,
    duration: Duration,
    effects: &[i32],
) -> AuraArray {
    let mut config = aura_config(row, &[label(format!("{} {}", row.name, row.id))]);
    config.duration = duration;
    let slows = row.slow_effects();
    let level = env.sim.unit(env.player).level;

    let units = env.sim.all_units();
    let mut auras: AuraArray = vec![None; units.len()];
    for target in units {
        if env.sim.unit(target).unit_type != UnitType::Enemy {
            continue;
        }
        let aura = match env.sim.get_aura(target, &config.label) {
            Some(aura) => aura,
            None => {
                let aura = env.sim.register_aura(target, config.clone());
                let mut parsed = Vec::new();
                for &i in effects {
                    if !slows.contains(&i) {
                        parsed.push(i);
                        continue;
                    }
                    let effect = row.effect_n(i);
                    let slowed_time = slowed_time_multiplier(effect.average(level));
                    if effect.changes_cast_speed() {
                        env.sim.cast_speed_reduction_effect(aura, slowed_time);
                    } else {
                        env.sim.atk_speed_reduction_effect(aura, slowed_time);
                    }
                }
                if !parsed.is_empty() {
                    parse_effects(
                        &mut env.sim,
                        None,
                        aura,
                        row,
                        ParseOptions {
                            only: parsed,
                            ..ParseOptions::default()
                        },
                    );
                }
                aura
            }
        };
        let index = env.sim.unit(target).unit_index as usize;
        auras[index] = Some(aura);
    }
    auras
}

/// Go `debuffOnLanding`: the debuff a proc's damage spell puts on each enemy its hit lands on,
/// or none where the row states none.
pub(crate) fn debuff_on_landing(env: &mut Environment, row: &'static Spell) -> Option<AuraArray> {
    let effects = row.debuff_effects();
    if effects.is_empty() {
        return None;
    }
    Some(enemy_debuff_auras(env, row, row.duration(), &effects))
}

/// Go `applySpellDataDebuffProc`: a proc whose spell puts a debuff on the enemy it lands on.
pub(crate) fn apply_spell_data_debuff_proc(env: &mut Environment, cfg: &SpellDataProc) {
    let unit = env.player;
    let source = cfg.source();
    let (trigger, debuff) = cfg.rows();
    let auras = enemy_debuff_auras(env, debuff, debuff.duration(), &debuff.debuff_effects());
    // A Forever chance on hit's armor debuff that does not stack is one stack, which the
    // runtime's armor debuff proc counts its armor by (export_items.rs).
    if cfg.rateless && env.sim.rules == super::sim::Rules::Forever {
        for aura in auras.into_iter().flatten() {
            if env.sim.aura(aura).max_stacks == 0 {
                env.sim.aura_mut(aura).max_stacks = 1;
            }
        }
    }

    let mut config = spell_data_proc_listener(&env.sim, unit, cfg, source, trigger, None);
    config.trigger_immediately = true;
    source.register_trigger(env, &config);
}

/// Go `registerSpellDataAuraProc`: an item or enchant proc whose buff applies auras: the
/// wearer's, its pets' and the debuff on the unit the proc answers, for the buff's duration.
pub(crate) fn apply_spell_data_aura_proc(env: &mut Environment, cfg: &SpellDataProc) {
    let unit = env.player;
    let source = cfg.source();
    let (trigger, buff) = cfg.rows();

    let mut config = aura_config(buff, &[label(format!("{} Proc", cfg.name))]);
    config.duration = proc_buff_duration_of(cfg, trigger, buff);
    let auras = new_spell_data_auras(env, buff, config);

    let mut proc = spell_data_proc_listener(&env.sim, unit, cfg, source, trigger, None);
    if proc.icd == 0 && buff.id != trigger.id {
        proc.icd = buff.category_cooldown();
    }
    let trigger_aura = source.register_trigger(env, &proc);
    let Some(wearer) = auras.wearer else {
        return;
    };

    // The registrations applySpellDataProc makes for a stat buff: the stat-proc APL values find
    // a buff that moves stats.
    if !auras.wearer_stats.is_empty() {
        let icd = env.sim.aura(trigger_aura).icd;
        env.sim.aura_mut(wearer).icd = icd;
        let slots = source.eligible_slots(&env.sim, unit);
        super::shared_items::add_stat_proc_buff(
            &mut env.sim,
            unit,
            source.id,
            StatBuffAura::new(wearer, auras.wearer_stats.clone()),
            source.is_enchant,
            &slots,
        );
    }
}

/// Go `procBuffDuration`.
fn proc_buff_duration_of(
    cfg: &SpellDataProc,
    trigger: &'static Spell,
    buff: &'static Spell,
) -> Duration {
    if buff.duration_ms != 0 {
        return buff.duration();
    }
    if trigger.duration_ms != 0 {
        return trigger.duration();
    }
    panic!(
        "{} ({}): neither the proc's spell {} nor its buff {} states a duration for the aura it applies",
        cfg.name,
        cfg.source().id,
        trigger.id,
        buff.id
    );
}

/// Go `registerSpellDataEquipAura`: an item whose equip spell applies auras: the row's auras on
/// the wearer and on each summoned pet for as long as the item is worn. An equip spell that only
/// re-applies another aura on a period is read as that aura.
pub(crate) fn apply_spell_data_equip_aura(env: &mut Environment, cfg: &SpellDataProc) {
    let unit = env.player;
    let source = cfg.source();
    let row = equip_aura_row(super::spelldata::must_find(cfg.trigger_spell_id));
    let slots = source.eligible_slots(&env.sim, unit);

    // Checked on reset rather than made permanent: a pet resets after its owner's item swap has
    // settled, and would otherwise put the aura back on for an item only in the swap set.
    let mut config = aura_config(row, &[label(cfg.name)]);
    config.duration = NEVER_EXPIRES;
    let item_id = source.id;
    config.on_reset = Some(Rc::new(move |sim: &mut Sim, aura: AuraId| {
        if has_item_equipped(sim, unit, item_id, &slots) {
            sim.activate(aura);
        }
    }));

    new_spell_data_auras(env, row, config);
}

/// Go `spellDataOnUseAuraSpell`: an on-use item whose spell applies auras: the wearer's buff,
/// its pets' and the debuff on the enemy it is used on, for the row's duration.
pub(crate) fn spell_data_on_use_aura_spell(
    env: &mut Environment,
    row: &'static Spell,
) -> SpellConfig {
    new_spell_data_auras(env, row, aura_config(row, &[]));
    SpellConfig {
        spell_school: row.spell_school(),
        proc_mask: ProcMask::EMPTY,
        ..SpellConfig::default()
    }
}

/// Go `UnitId` helper kept for symmetry with the shared constructors.
#[allow(dead_code)]
fn unit_of(env: &Environment) -> UnitId {
    env.player
}
