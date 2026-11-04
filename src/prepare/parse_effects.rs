//! Go sim/core/spelldata parse_effects.go, parse_effects_table.go, parse_effects_exclusive.go and
//! buff.go: a client row's aura effects attached to an aura, or applied at once for a passive.
//!
//! Each attachment is a closure that turns its effect on, off or up to a level, as Go's
//! `attachment.set`; the state Go keeps in captured variables is kept in `Cell`s.

use std::rc::Rc;

use crate::data::spells::{ClassFlags, Effect, Spell};

use super::character::constants::CHARACTER_LEVEL;
use super::dbcenums;
use super::sim::{AuraId, Duration, Sim, UnitId, UnitType, MILLISECOND};
use super::spell::{school, ProcMask, Resource};
use super::spell_mod::{ModId, SpellModConfig, SpellModType};
use super::stats::{PseudoStats, SchoolIndex, Stat, Stats, SCHOOL_LEN};

/// A `PseudoStats` field Go hands the parser a pointer to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PseudoField {
    DamageDealtMultiplier,
    SchoolDamageDealtMultiplier(usize),
    DamageTakenMultiplier,
    SchoolDamageTakenMultiplier(usize),
    BonusPhysicalDamageTaken,
    BonusSpellDamageTaken,
    ThreatMultiplier,
    BonusRangedAttackPower,
    HealingDealtMultiplier,
    HealingTakenMultiplier,
    BonusHealingTaken,
    PushbackChance,
    FearDurationMultiplier,
    StunDurationMultiplier,
}

impl PseudoField {
    pub(crate) fn get(self, p: &mut PseudoStats) -> &mut f64 {
        match self {
            PseudoField::DamageDealtMultiplier => &mut p.damage_dealt_multiplier,
            PseudoField::SchoolDamageDealtMultiplier(i) => &mut p.school_damage_dealt_multiplier[i],
            PseudoField::DamageTakenMultiplier => &mut p.damage_taken_multiplier,
            PseudoField::SchoolDamageTakenMultiplier(i) => &mut p.school_damage_taken_multiplier[i],
            PseudoField::BonusPhysicalDamageTaken => &mut p.bonus_physical_damage_taken,
            PseudoField::BonusSpellDamageTaken => &mut p.bonus_spell_damage_taken,
            PseudoField::ThreatMultiplier => &mut p.threat_multiplier,
            PseudoField::BonusRangedAttackPower => &mut p.bonus_ranged_attack_power,
            PseudoField::HealingDealtMultiplier => &mut p.healing_dealt_multiplier,
            PseudoField::HealingTakenMultiplier => &mut p.healing_taken_multiplier,
            PseudoField::BonusHealingTaken => &mut p.bonus_healing_taken,
            PseudoField::PushbackChance => &mut p.pushback_chance,
            PseudoField::FearDurationMultiplier => &mut p.fear_duration_multiplier,
            PseudoField::StunDurationMultiplier => &mut p.stun_duration_multiplier,
        }
    }
}

/// Go `parseOptions`, built with the `ParseOpt` constructors' fields.
#[derive(Clone, Default)]
pub(crate) struct ParseOptions {
    pub only: Vec<i32>,
    pub skip: Vec<i32>,
    pub skip_auras: Vec<i32>,
    /// Go `Conditional`: preparation has no conditional parse yet.
    pub conditional: bool,
    pub ignore_stacks: bool,
    pub level: i32,
    pub scale: Option<&'static Effect>,
    /// Go `ScaledByRanks`: each amount is this share of the row's, rounded half away from zero;
    /// 0 reads the row's amount.
    pub rank_fraction: f64,
    pub combo_points: bool,
    pub count: f64,
    pub buff_auras: bool,
    /// Go `Exclusive(category, singleAura)`.
    pub exclusive: Option<(String, bool)>,
    pub per_stat_category: Option<String>,
    pub school_resistances: bool,
    /// Go `ReadAuraAs`: every effect of the first aura is read as if the row stated the second,
    /// at the row's own amount.
    pub read_as: Vec<(i32, i32)>,
    /// Effect positions (1 based) whose amount is read as the second value instead of the
    /// row's, for a client change the frozen spell data does not carry yet.
    pub amount_as: Vec<(i32, f64)>,
}

impl ParseOptions {
    /// Go `RaidBuffOptions`.
    pub(crate) fn raid_buff(skip: &[i32], full_combo_points: bool) -> ParseOptions {
        ParseOptions {
            level: CHARACTER_LEVEL,
            buff_auras: true,
            skip_auras: skip.to_vec(),
            combo_points: full_combo_points,
            ..ParseOptions::default()
        }
    }

    fn reads(&self, position: i32) -> bool {
        if !self.only.is_empty() && !self.only.contains(&position) {
            return false;
        }
        !self.skip.contains(&position)
    }
}

/// One attachment the parse made: Go `Applied`.
#[derive(Clone, Debug)]
pub(crate) struct Applied {
    pub kind: String,
    pub value: f64,
}

type SetFn = Rc<dyn Fn(&mut Sim, bool, f64)>;
type ScaleFn = Rc<dyn Fn(&mut Sim, ModId, f64)>;

/// Go `attachment`. `set(sim, live, level)`: `live` says whether Go's `sim` is non-nil.
#[derive(Clone)]
struct Attachment {
    kind: String,
    value: f64,
    set: Option<SetFn>,
    needs_sim: bool,
    multiplicative: bool,
    key: String,
    parts: Vec<Attachment>,
    stat: Option<Stat>,
    stat_bid: bool,
    stats: Vec<Stat>,
}

impl Attachment {
    fn new(kind: String, value: f64) -> Attachment {
        Attachment {
            kind,
            value,
            set: None,
            needs_sim: false,
            multiplicative: false,
            key: String::new(),
            parts: Vec::new(),
            stat: None,
            stat_bid: false,
            stats: Vec::new(),
        }
    }

    fn apply(&self, sim: &mut Sim, live: bool, level: f64) {
        if let Some(set) = &self.set {
            set(sim, live, level);
        }
    }
}

/// Go `Parsed`.
pub(crate) struct Parsed {
    pub applied: Vec<Applied>,
    pub skipped: Vec<(i32, i32)>,
    pub stats: Vec<Stat>,
}

struct Parser {
    unit: UnitId,
    character: Option<UnitId>,
    class_flags: ClassFlags,
    is_static: bool,
    conditional: bool,
    stacking: bool,
    dry: bool,
    has_rage_bar: bool,
    is_pet: bool,
}

const MISC_ALL_SCHOOLS: i32 = 127;
const MISC_MAGIC_SCHOOL: i32 = 126;
const MISC_ARMOR: i32 = 1;
const MISC_ALL_STATS: i32 = -1;
const CLIENT_STATS: [Stat; 5] = [
    Stat::Strength,
    Stat::Agility,
    Stat::Stamina,
    Stat::Intellect,
    Stat::Spirit,
];
const FULL_COMBO_POINTS: f64 = 5.0;

/// Go `Scaled`: an improving talent's modifier on an amount, truncated.
pub(crate) fn scaled(amount: f64, modifier: &Effect) -> f64 {
    if modifier.aura == dbcenums::A_ADD_PCT_MODIFIER {
        (amount * (1.0 + modifier.base_value() / 100.0)).trunc()
    } else {
        (amount + modifier.base_value()).trunc()
    }
}

/// Go `AppliesAura`.
pub(crate) fn applies_aura(effect_type: i32) -> bool {
    effect_type == dbcenums::E_APPLY_AURA
        || effect_type == dbcenums::E_APPLY_AREA_AURA_PARTY
        || effect_type == dbcenums::E_APPLY_AREA_AURA_RAID
}

/// Go `foldedDotEffects`: the indexes of dot modifiers a hit modifier of the parse folds in.
fn folded_dot_effects(row: &Spell, options: &ParseOptions) -> Vec<usize> {
    let mut folded = Vec::new();
    for (i, dot) in row.effects.iter().enumerate() {
        if dot.aura != dbcenums::A_ADD_PCT_MODIFIER || dot.misc != dbcenums::SPELLMOD_DOT {
            continue;
        }
        for (j, hit) in row.effects.iter().enumerate() {
            if hit.aura != dbcenums::A_ADD_PCT_MODIFIER
                || !applies_aura(hit.effect_type)
                || !options.reads(j as i32 + 1)
                || (hit.misc != dbcenums::SPELLMOD_DAMAGE
                    && hit.misc != dbcenums::SPELLMOD_ALL_EFFECTS)
            {
                continue;
            }
            if hit.class_flags == dot.class_flags && hit.base_points == dot.base_points {
                folded.push(i);
                break;
            }
        }
    }
    folded
}

/// Go `ParseEffects`: the row's aura effects attached to `aura`.
pub(crate) fn parse_effects(
    sim: &mut Sim,
    character: Option<UnitId>,
    aura: AuraId,
    row: &'static Spell,
    options: ParseOptions,
) -> Parsed {
    let unit = sim.aura(aura).unit;
    parse(sim, unit, character, Some(aura), row, options, false)
}

/// Go `ParseStatic`: the row's aura effects applied to the character now.
pub(crate) fn parse_static(
    sim: &mut Sim,
    character: UnitId,
    row: &'static Spell,
    options: ParseOptions,
) -> Parsed {
    parse(sim, character, Some(character), None, row, options, false)
}

/// Go `DryRun`: what the parse would attach, registering nothing.
pub(crate) fn dry_run(row: &'static Spell, options: ParseOptions) -> Parsed {
    let mut scratch = Sim::new();
    let mut unit = super::sim::Unit::new(UnitType::Player, String::new());
    unit.level = CHARACTER_LEVEL;
    let id = scratch.add_unit(unit);
    parse(&mut scratch, id, None, None, row, options, true)
}

fn parse(
    sim: &mut Sim,
    unit: UnitId,
    character: Option<UnitId>,
    aura: Option<AuraId>,
    row: &'static Spell,
    options: ParseOptions,
    dry: bool,
) -> Parsed {
    let mut parsed = Parsed {
        applied: Vec::new(),
        skipped: Vec::new(),
        stats: Vec::new(),
    };
    if row.effects.is_empty() {
        return parsed;
    }
    let stacking = row.max_stack > 0 && !options.ignore_stacks;
    let parser = Parser {
        unit,
        character,
        class_flags: row.class_flags,
        is_static: aura.is_none() && !dry,
        conditional: options.conditional,
        stacking: stacking || options.count != 0.0,
        dry,
        has_rage_bar: sim.unit(unit).rage_bar.enabled,
        is_pet: sim.unit(unit).unit_type == UnitType::Pet,
    };
    let folded = folded_dot_effects(row, &options);
    let mut level = sim.unit(unit).level;
    if let Some(character) = character {
        level = sim.unit(character).level;
    }
    if options.level != 0 {
        level = options.level;
    }
    let mut scale = options.scale;
    let mut attachments: Vec<Option<Attachment>> = Vec::new();
    for (i, effect) in row.effects.iter().enumerate() {
        if !options.reads(i as i32 + 1)
            || options.skip_auras.contains(&effect.aura)
            || !applies_aura(effect.effect_type)
        {
            continue;
        }
        let mut value = effect.average(level);
        if let Some(&(_, amount)) = options
            .amount_as
            .iter()
            .find(|(position, _)| *position == i as i32 + 1)
        {
            value = amount;
        }
        if effect.points_per_resource != 0.0 && value == 0.0 {
            if !options.combo_points {
                parsed.skipped.push((i as i32 + 1, effect.aura));
                continue;
            }
            value = f64::from(effect.points_per_resource) * FULL_COMBO_POINTS;
        }
        if folded.contains(&i) {
            parsed.applied.push(Applied {
                kind: "folded-into SpellMod_DamageDone_Flat".into(),
                value: value / 100.0,
            });
            attachments.push(None);
            continue;
        }
        if let Some(modifier) = scale {
            value = scaled(value, modifier);
        }
        if options.rank_fraction > 0.0 {
            value = (value * options.rank_fraction).round();
        }
        let read = options
            .read_as
            .iter()
            .find(|(from, _)| *from == effect.aura)
            .map(|&(_, to)| Effect {
                aura: to,
                ..effect.clone()
            });
        let attached = parser.row(
            sim,
            read.as_ref().unwrap_or(effect),
            value,
            options.buff_auras,
        );
        let Some(mut attachment) = attached else {
            parsed.skipped.push((i as i32 + 1, effect.aura));
            continue;
        };
        scale = None;
        if attachment.key.is_empty() {
            attachment.key = attachment.kind.clone();
        }
        parsed.applied.push(Applied {
            kind: attachment.kind.clone(),
            value: attachment.value,
        });
        parsed.stats.extend(attachment.stats.iter().copied());
        attachments.push(Some(attachment));
    }
    let count = if options.count != 0.0 {
        options.count
    } else {
        1.0
    };
    if dry {
        return parsed;
    }
    let level_of = move |sim: &Sim| -> f64 {
        if let Some(aura) = aura {
            if !sim.aura(aura).active {
                return 0.0;
            }
            if stacking {
                let stacks = sim.aura(aura).stacks;
                if stacks > 0 {
                    return f64::from(stacks) * count;
                }
            }
        }
        count
    };
    let Some(aura) = aura else {
        let live: Vec<Attachment> = attachments.into_iter().flatten().collect();
        let level = level_of(sim);
        for attachment in &live {
            attachment.apply(sim, false, level);
        }
        return parsed;
    };
    let live = Rc::new(bid(sim, aura, &options, &attachments, stacking, count));
    let gain = Rc::clone(&live);
    sim.apply_on_gain(
        aura,
        Rc::new(move |sim: &mut Sim, _| {
            let level = level_of(sim);
            for attachment in gain.iter() {
                attachment.apply(sim, true, level);
            }
        }),
    );
    let expire = Rc::clone(&live);
    sim.apply_on_expire(
        aura,
        Rc::new(move |sim: &mut Sim, _| {
            for attachment in expire.iter() {
                attachment.apply(sim, true, 0.0);
            }
        }),
    );
    if stacking {
        let stacks = Rc::clone(&live);
        sim.apply_on_stacks_change(
            aura,
            Rc::new(move |sim: &mut Sim, _, _, _| {
                let level = level_of(sim);
                for attachment in stacks.iter() {
                    attachment.apply(sim, true, level);
                }
            }),
        );
    }
    if sim.aura(aura).active {
        let level = level_of(sim);
        for attachment in live.iter() {
            if !attachment.needs_sim {
                attachment.apply(sim, false, level);
            }
        }
    }
    parsed
}

/// Go `schoolResistanceCategories`.
fn school_resistance_category(stat: Stat) -> Option<&'static str> {
    Some(match stat {
        Stat::ArcaneResistance => "ResistanceArcane",
        Stat::FireResistance => "ResistanceFire",
        Stat::FrostResistance => "ResistanceFrost",
        Stat::NatureResistance => "ResistanceNature",
        Stat::ShadowResistance => "ResistanceShadow",
        _ => return None,
    })
}

/// Go `ExclusiveStatCategory`.
pub(crate) fn exclusive_stat_category(category: &str, key: &str, multiplicative: bool) -> String {
    format!(
        "{category}{key}{}",
        if multiplicative { "Mul" } else { "Add" }
    )
}

fn magnitude(attachment: &Attachment) -> f64 {
    let mut value = attachment.value;
    if attachment.multiplicative {
        value -= 1.0;
    }
    value.abs()
}

fn split(attachments: Vec<Attachment>) -> Vec<Attachment> {
    let mut out = Vec::new();
    for attachment in attachments {
        if attachment.parts.is_empty() {
            out.push(attachment);
        } else {
            out.extend(attachment.parts);
        }
    }
    out
}

fn bid_alone(
    sim: &mut Sim,
    aura: AuraId,
    category: &str,
    priority: f64,
    attachment: Attachment,
    count: f64,
) {
    let gain = attachment.clone();
    let expire = attachment;
    sim.new_exclusive_effect(
        aura,
        category,
        false,
        priority,
        Some(Rc::new(move |sim: &mut Sim, _| {
            gain.apply(sim, true, count)
        })),
        Some(Rc::new(move |sim: &mut Sim, _| {
            expire.apply(sim, true, 0.0)
        })),
    );
}

fn bid_school_resistances(
    sim: &mut Sim,
    aura: AuraId,
    attachment: &Attachment,
    options: &ParseOptions,
    count: f64,
) -> Vec<Attachment> {
    if !options.school_resistances || attachment.parts.is_empty() {
        return vec![attachment.clone()];
    }
    let mut rest = Vec::new();
    for part in &attachment.parts {
        let category = part.stat.and_then(school_resistance_category);
        match category {
            Some(category) if !part.multiplicative => {
                let name = exclusive_stat_category(category, &part.key, false);
                bid_alone(sim, aura, &name, part.value, part.clone(), count);
            }
            _ => rest.push(part.clone()),
        }
    }
    if rest.len() == attachment.parts.len() {
        return vec![attachment.clone()];
    }
    rest
}

/// Go `Parsed.bid`: the attachments the aura's own gain and expiry drive.
fn bid(
    sim: &mut Sim,
    aura: AuraId,
    options: &ParseOptions,
    attachments: &[Option<Attachment>],
    stacking: bool,
    count: f64,
) -> Vec<Attachment> {
    let mut live = Vec::new();
    let mut whole = Vec::new();
    for attachment in attachments.iter().flatten() {
        let rest = bid_school_resistances(sim, aura, attachment, options, count);
        if options.exclusive.is_some() {
            whole.extend(rest);
        } else if let Some(category) = &options.per_stat_category {
            for part in split(rest) {
                let priority = if part.stat_bid {
                    part.value
                } else {
                    magnitude(&part)
                };
                let name = exclusive_stat_category(category, &part.key, part.multiplicative);
                bid_alone(sim, aura, &name, priority, part, count);
            }
        } else {
            live.extend(rest);
        }
    }
    if let Some((category, single_aura)) = &options.exclusive {
        let per_stack = attachments.iter().flatten().next().map_or(0.0, magnitude);
        let priority = if stacking { 0.0 } else { per_stack };
        let whole = Rc::new(whole);
        let gain = Rc::clone(&whole);
        let expire = Rc::clone(&whole);
        let effect = sim.new_exclusive_effect(
            aura,
            category,
            *single_aura,
            priority,
            Some(Rc::new(move |sim: &mut Sim, effect| {
                let mut level = count;
                if stacking && per_stack != 0.0 {
                    level *= sim.effects[effect.0].priority / per_stack;
                }
                for attachment in gain.iter() {
                    attachment.apply(sim, true, level);
                }
            })),
            Some(Rc::new(move |sim: &mut Sim, _| {
                for attachment in expire.iter() {
                    attachment.apply(sim, true, 0.0);
                }
            })),
        );
        if stacking {
            sim.apply_on_stacks_change(
                aura,
                Rc::new(move |sim: &mut Sim, _, _, new_stacks| {
                    sim.set_effect_priority(effect, per_stack * f64::from(new_stacks));
                }),
            );
        }
    }
    live
}

fn additive(sim: &mut Sim, kind: String, value: f64, apply: SetFn) -> Attachment {
    let current = sim.new_state_slot(0.0);
    let mut attachment = Attachment::new(kind, value);
    attachment.set = Some(Rc::new(move |sim: &mut Sim, live, level| {
        // Go fuses v*level - current into one multiply-subtract (parse_effects_table.go 692).
        let delta = value.mul_add(level, -sim.state(current));
        if delta == 0.0 {
            return;
        }
        sim.set_state(current, value * level);
        apply(sim, live, delta);
    }));
    attachment
}

fn multiplier(sim: &mut Sim, kind: String, mult: f64, apply: SetFn) -> Attachment {
    let active = sim.new_state_slot(0.0);
    let mut attachment = Attachment::new(kind, mult);
    attachment.multiplicative = true;
    attachment.set = Some(Rc::new(move |sim: &mut Sim, live, level| {
        if (level > 0.0) == sim.state_flag(active) {
            return;
        }
        sim.set_state_flag(active, level > 0.0);
        let factor = if sim.state_flag(active) {
            mult
        } else {
            1.0 / mult
        };
        apply(sim, live, factor);
    }));
    attachment
}

fn percent_multiplier(value: f64) -> f64 {
    1.0 + value / 100.0
}

/// Go `speedMultiplier`.
fn speed_multiplier(value: f64) -> f64 {
    if value < 0.0 {
        1.0 / (1.0 - value / 100.0)
    } else {
        1.0 + value / 100.0
    }
}

fn stat_names(stats: &[Stat]) -> String {
    stats
        .iter()
        .map(|stat| stat.name())
        .collect::<Vec<_>>()
        .join("+")
}

fn client_stat_list(misc: i32) -> Vec<Stat> {
    if misc == MISC_ALL_STATS {
        return CLIENT_STATS.to_vec();
    }
    if !(0..5).contains(&misc) {
        return Vec::new();
    }
    vec![CLIENT_STATS[misc as usize]]
}

/// Go `SpellSchool.SchoolDamage`.
fn school_damage(mask: i32) -> Stat {
    match mask as u8 {
        school::ARCANE => Stat::ArcaneDamage,
        school::FIRE => Stat::FireDamage,
        school::FROST => Stat::FrostDamage,
        school::HOLY => Stat::HolyDamage,
        school::NATURE => Stat::NatureDamage,
        school::SHADOW => Stat::ShadowDamage,
        _ => Stat::SpellDamage,
    }
}

fn is_one_school(mask: i32) -> bool {
    mask > 0 && mask & (mask - 1) == 0 && mask <= i32::from(school::ARCANE)
}

fn damage_done_stats(mask: i32) -> Vec<Stat> {
    if mask == MISC_ALL_SCHOOLS {
        vec![Stat::PhysicalDamage, Stat::SpellDamage]
    } else if mask == MISC_MAGIC_SCHOOL {
        vec![Stat::SpellDamage]
    } else if mask == i32::from(school::PHYSICAL) {
        vec![Stat::PhysicalDamage]
    } else if is_one_school(mask) {
        vec![school_damage(mask)]
    } else {
        Vec::new()
    }
}

/// Go `SpellSchool.ResistanceStat` for single schools; zero (Strength) means none.
fn resistance_stat(bit: i32) -> Option<Stat> {
    match bit as u8 {
        school::ARCANE => Some(Stat::ArcaneResistance),
        school::FIRE => Some(Stat::FireResistance),
        school::FROST => Some(Stat::FrostResistance),
        school::NATURE => Some(Stat::NatureResistance),
        school::SHADOW => Some(Stat::ShadowResistance),
        _ => None,
    }
}

fn resistance_stats(mask: i32) -> Vec<Stat> {
    let mut out = Vec::new();
    if mask & MISC_ARMOR != 0 {
        out.push(Stat::Armor);
    }
    let mut bit = i32::from(school::HOLY);
    while bit <= i32::from(school::ARCANE) {
        if mask & bit != 0 {
            if let Some(stat) = resistance_stat(bit) {
                out.push(stat);
            }
        }
        bit <<= 1;
    }
    out
}

fn rating_stats(mask: i32) -> Vec<Stat> {
    match mask {
        2 => vec![Stat::DefenseRating],
        4 => vec![Stat::DodgeRating],
        8 => vec![Stat::ParryRating],
        16 => vec![Stat::BlockRating],
        96 => vec![Stat::MeleeHitRating],
        128 => vec![Stat::SpellHitRating],
        256 | 768 => vec![Stat::MeleeCritRating],
        1024 => vec![Stat::SpellCritRating],
        131072 | 393216 | 524288 => vec![Stat::MeleeHasteRating],
        _ => Vec::new(),
    }
}

/// Go `SpellSchool.SchoolIndex` for a single school bit.
fn school_index_of_bit(bit: i32) -> usize {
    let index = match bit as u8 {
        school::PHYSICAL => SchoolIndex::Physical,
        school::ARCANE => SchoolIndex::Arcane,
        school::FIRE => SchoolIndex::Fire,
        school::FROST => SchoolIndex::Frost,
        school::HOLY => SchoolIndex::Holy,
        school::NATURE => SchoolIndex::Nature,
        school::SHADOW => SchoolIndex::Shadow,
        _ => SchoolIndex::Physical,
    };
    index as usize
}

fn school_indexes(mask: i32) -> Vec<usize> {
    let mut out = Vec::new();
    let mut bit = 1;
    while bit <= i32::from(school::ARCANE) {
        if mask & bit != 0 {
            out.push(school_index_of_bit(bit));
        }
        bit <<= 1;
    }
    out
}

/// Go `percentStatRowsNamingNoStat`: rows whose stat the client left unnamed, with the stat their
/// own tooltip gives. Client 70425 names Increased Spirit's stat again, in the stat mask alone, so
/// none is left.
fn percent_stat_row_naming_no_stat(_spell_id: i32) -> Option<Stat> {
    None
}

/// Go `clientStatMask`: the stats a client stat mask names, bit i for `CLIENT_STATS[i]`.
fn client_stat_mask(mask: i32) -> Vec<Stat> {
    CLIENT_STATS
        .iter()
        .enumerate()
        .filter(|(i, _)| mask & (1 << i) != 0)
        .map(|(_, stat)| *stat)
        .collect()
}

impl Parser {
    fn row(
        &self,
        sim: &mut Sim,
        effect: &Effect,
        value: f64,
        buff_auras: bool,
    ) -> Option<Attachment> {
        if buff_auras {
            if effect.aura == dbcenums::A_PERIODIC_ENERGIZE {
                if effect.misc != dbcenums::POWER_MANA || effect.period_ms <= 0 {
                    return None;
                }
                return self.stats_buff(
                    sim,
                    vec![Stat::MP5],
                    value * 5000.0 / f64::from(effect.period_ms),
                );
            }
            if effect.aura == dbcenums::A_MOD_ATTACK_POWER_PCT {
                return self.stat_multiplier(
                    sim,
                    vec![Stat::AttackPower],
                    percent_multiplier(value),
                );
            }
        }
        self.aura_row(sim, effect, value)
    }

    fn aura_row(&self, sim: &mut Sim, e: &Effect, v: f64) -> Option<Attachment> {
        use dbcenums::*;
        let a = e.aura;
        if a == A_ADD_FLAT_MODIFIER {
            return self.flat_mod(sim, e, v);
        }
        if a == A_ADD_PCT_MODIFIER {
            return self.pct_mod(sim, e, v / 100.0);
        }
        if a == A_MOD_DAMAGE_PERCENT_DONE {
            return self.school_multiplier(
                sim,
                "damage-dealt",
                "DamageDealtMultiplier",
                e.misc,
                percent_multiplier(v),
                PseudoField::DamageDealtMultiplier,
                PseudoField::SchoolDamageDealtMultiplier,
            );
        }
        if a == A_MOD_DAMAGE_PERCENT_TAKEN {
            return self.school_multiplier(
                sim,
                "damage-taken",
                "DamageTakenMultiplier",
                e.misc,
                percent_multiplier(v),
                PseudoField::DamageTakenMultiplier,
                PseudoField::SchoolDamageTakenMultiplier,
            );
        }
        if a == A_MOD_DAMAGE_DONE {
            return self.stats_buff(sim, damage_done_stats(e.misc), v);
        }
        if a == A_MOD_POWER_COST_SCHOOL_PCT {
            if self.stacking || e.misc == 0 {
                return None;
            }
            return self.mod_float(
                sim,
                "SpellMod_PowerCost_Pct",
                SpellModConfig {
                    kind: SpellModType::PowerCostPct,
                    school: e.misc as u8,
                    ..Default::default()
                },
                v / 100.0,
            );
        }
        if a == A_MOD_DAMAGE_TAKEN {
            let physical = |sim: &mut Sim| {
                self.pseudo_add(
                    sim,
                    "physical-damage-taken-flat",
                    "BonusPhysicalDamageTaken",
                    PseudoField::BonusPhysicalDamageTaken,
                    v,
                )
            };
            let spell = |sim: &mut Sim| {
                self.pseudo_add(
                    sim,
                    "spell-damage-taken-flat",
                    "BonusSpellDamageTaken",
                    PseudoField::BonusSpellDamageTaken,
                    v,
                )
            };
            if e.misc & MISC_ALL_SCHOOLS == MISC_ALL_SCHOOLS {
                let parts = vec![physical(sim), spell(sim)];
                let mut attachment = Attachment::new("damage-taken-flat".into(), v);
                let run = parts.clone();
                attachment.parts = parts;
                attachment.set = Some(Rc::new(move |sim: &mut Sim, live, level| {
                    for part in &run {
                        part.apply(sim, live, level);
                    }
                }));
                return Some(attachment);
            }
            if e.misc & i32::from(school::PHYSICAL) != 0 {
                return Some(physical(sim));
            }
            if e.misc & MISC_MAGIC_SCHOOL == MISC_MAGIC_SCHOOL {
                return Some(spell(sim));
            }
            return None;
        }
        if a == A_MOD_THREAT {
            return self.pseudo_multiplier(
                sim,
                "threat",
                "ThreatMultiplier",
                vec![PseudoField::ThreatMultiplier],
                percent_multiplier(v),
            );
        }
        if a == A_MOD_ATTACK_POWER {
            return self.stats_buff(sim, vec![Stat::AttackPower], v);
        }
        if a == A_MOD_RANGED_ATTACK_POWER {
            return self.stats_buff(sim, vec![Stat::RangedAttackPower], v);
        }
        if a == A_RANGED_ATTACK_POWER_ATTACKER_BONUS {
            return Some(self.pseudo_add(
                sim,
                "ranged-attack-power-attacker-bonus",
                "BonusRangedAttackPower",
                PseudoField::BonusRangedAttackPower,
                v,
            ));
        }
        if a == A_MOD_RATING {
            return self.stats_buff(sim, rating_stats(e.misc), v);
        }
        if a == A_MOD_STAT {
            return self.stats_buff(sim, client_stat_list(e.misc), v);
        }
        if a == A_MOD_PERCENT_STAT || a == A_MOD_TOTAL_STAT_PERCENTAGE {
            // A_MOD_TOTAL_STAT_PERCENTAGE names its stats in a mask in MiscValue_1 as well as, on
            // older rows, a stat in MiscValue_0; client 70425's rows carry the mask alone, so the
            // mask wins when it is set.
            if a == A_MOD_TOTAL_STAT_PERCENTAGE && e.misc2 != 0 {
                let list = client_stat_mask(e.misc2);
                if !list.is_empty() {
                    return self.stat_multiplier(sim, list, percent_multiplier(v));
                }
            }
            if a == A_MOD_TOTAL_STAT_PERCENTAGE && e.misc == 0 && e.misc2 == 0 {
                if let Some(stat) = percent_stat_row_naming_no_stat(e.spell_id) {
                    return self.stat_multiplier(sim, vec![stat], percent_multiplier(v));
                }
            }
            return self.stat_multiplier(sim, client_stat_list(e.misc), percent_multiplier(v));
        }
        if a == A_MOD_HIT_CHANCE {
            return self.stats_buff(sim, vec![Stat::PhysicalHitPercent], v);
        }
        if a == A_MOD_SPELL_HIT_CHANCE {
            return self.stats_buff(sim, vec![Stat::SpellHitPercent], v);
        }
        if a == A_MOD_WEAPON_CRIT_PERCENT {
            return self.stats_buff(sim, vec![Stat::PhysicalCritPercent], v);
        }
        if a == A_MOD_SPELL_CRIT_CHANCE {
            return self.stats_buff(sim, vec![Stat::SpellCritPercent], v);
        }
        if a == A_MOD_CRIT_PCT {
            return self.stats_buff(
                sim,
                vec![Stat::PhysicalCritPercent, Stat::SpellCritPercent],
                v,
            );
        }
        if a == A_MOD_CASTING_SPEED_NOT_STACK {
            return self.speed(
                sim,
                "cast-speed",
                "CastSpeedMultiplier",
                SpeedKind::Cast,
                speed_multiplier(v),
            );
        }
        if a == A_MOD_MELEE_HASTE_3 {
            return self.speed(
                sim,
                "melee-speed",
                "MeleeSpeedMultiplier",
                SpeedKind::Melee,
                speed_multiplier(v),
            );
        }
        if a == A_MOD_ATTACKSPEED {
            return self.speed(
                sim,
                "attack-speed",
                "AttackSpeedMultiplier",
                SpeedKind::Attack,
                speed_multiplier(v),
            );
        }
        if a == A_MOD_HEALING_DONE {
            return self.stats_buff(sim, vec![Stat::HealingPower], v);
        }
        if a == A_MOD_HEALING_DONE_PERCENT {
            return self.pseudo_multiplier(
                sim,
                "healing-dealt",
                "HealingDealtMultiplier",
                vec![PseudoField::HealingDealtMultiplier],
                percent_multiplier(v),
            );
        }
        if a == A_MOD_HEALING_PCT {
            return self.pseudo_multiplier(
                sim,
                "healing-taken",
                "HealingTakenMultiplier",
                vec![PseudoField::HealingTakenMultiplier],
                percent_multiplier(v),
            );
        }
        if a == A_MOD_HEALING {
            return Some(self.pseudo_add(
                sim,
                "healing-taken-flat",
                "BonusHealingTaken",
                PseudoField::BonusHealingTaken,
                v,
            ));
        }
        if a == A_REDUCE_PUSHBACK {
            return Some(self.pseudo_add(
                sim,
                "pushback",
                "PushbackChance",
                PseudoField::PushbackChance,
                -v / 100.0,
            ));
        }
        if a == A_MOD_POWER_REGEN {
            if e.misc != POWER_MANA {
                return None;
            }
            return self.stats_buff(sim, vec![Stat::MP5], v);
        }
        if a == A_MOD_INCREASE_HEALTH {
            return self.stats_buff(sim, vec![Stat::Health], v);
        }
        if a == A_MOD_INCREASE_HEALTH_PERCENT {
            return self.stat_multiplier(sim, vec![Stat::Health], percent_multiplier(v));
        }
        if a == A_MOD_RESISTANCE {
            return self.stats_buff(sim, resistance_stats(e.misc), v);
        }
        if a == A_MOD_BASE_RESISTANCE_PCT {
            if e.misc & MISC_ARMOR == 0 {
                return None;
            }
            return self.equip_scaling(sim, Stat::Armor, percent_multiplier(v));
        }
        if a == A_MOD_BLOCK_PERCENT {
            return self.stats_buff(sim, vec![Stat::BlockPercent], v);
        }
        if a == A_MOD_DODGE_PERCENT {
            return self.stats_buff(sim, vec![Stat::DodgePercent], v);
        }
        if a == A_MOD_PARRY_PERCENT {
            return self.stats_buff(sim, vec![Stat::ParryPercent], v);
        }
        if a == A_MOD_OFFHAND_DAMAGE_PCT {
            return self.mod_float(
                sim,
                "SpellMod_DamageDone_Pct",
                SpellModConfig {
                    kind: SpellModType::DamageDonePct,
                    proc_mask: ProcMask::MELEE_OH,
                    ..Default::default()
                },
                v / 100.0,
            );
        }
        if a == A_MOD_EXPERTISE {
            return self.stats_buff(sim, vec![Stat::ExpertisePercent], v);
        }
        if a == A_MECHANIC_DURATION_MOD {
            if e.misc == MECHANIC_FEAR {
                return self.pseudo_multiplier(
                    sim,
                    "fear-duration",
                    "FearDurationMultiplier",
                    vec![PseudoField::FearDurationMultiplier],
                    percent_multiplier(v),
                );
            }
            if e.misc == MECHANIC_STUN {
                return self.pseudo_multiplier(
                    sim,
                    "stun-duration",
                    "StunDurationMultiplier",
                    vec![PseudoField::StunDurationMultiplier],
                    percent_multiplier(v),
                );
            }
            return None;
        }
        None
    }

    fn flat_mod(&self, sim: &mut Sim, e: &Effect, mut v: f64) -> Option<Attachment> {
        use dbcenums::*;
        let op = e.misc;
        if op == SPELLMOD_COST {
            if self.has_rage_bar {
                v /= 10.0;
            }
            return self.mod_int(
                sim,
                "SpellMod_PowerCost_Flat",
                self.mod_config(e, SpellModType::PowerCostFlat),
                v,
            );
        }
        if op == SPELLMOD_CASTING_TIME {
            return self.mod_time(
                sim,
                "SpellMod_CastTime_Flat",
                self.mod_config(e, SpellModType::CastTimeFlat),
                v,
            );
        }
        if op == SPELLMOD_COOLDOWN {
            return self.mod_time(
                sim,
                "SpellMod_Cooldown_Flat",
                self.mod_config(e, SpellModType::CooldownFlat),
                v,
            );
        }
        if op == SPELLMOD_GLOBAL_COOLDOWN {
            return self.mod_time(
                sim,
                "SpellMod_GlobalCooldown_Flat",
                self.mod_config(e, SpellModType::GlobalCooldownFlat),
                v,
            );
        }
        if op == SPELLMOD_CRITICAL_CHANCE {
            return self.mod_float(
                sim,
                "SpellMod_BonusCrit_Percent",
                self.mod_config(e, SpellModType::BonusCritPercent),
                v,
            );
        }
        if op == SPELLMOD_RESIST_MISS_CHANCE {
            return self.mod_float(
                sim,
                "SpellMod_BonusHit_Percent",
                self.mod_config(e, SpellModType::BonusHitPercent),
                v,
            );
        }
        if op == SPELLMOD_DURATION {
            return self.mod_time(
                sim,
                "SpellMod_Duration_Flat",
                self.mod_config(e, SpellModType::DurationFlat),
                v,
            );
        }
        if op == SPELLMOD_CHARGES {
            return self.mod_int(
                sim,
                "SpellMod_BuffMaxStacks_Flat",
                self.mod_config(e, SpellModType::BuffMaxStacksFlat),
                v,
            );
        }
        if op == SPELLMOD_RANGE {
            return self.mod_float(
                sim,
                "SpellMod_Range_Flat",
                self.mod_config(e, SpellModType::RangeFlat),
                v,
            );
        }
        if op == SPELLMOD_EFFECT1 || op == SPELLMOD_EFFECT2 || op == SPELLMOD_EFFECT3 {
            return self.mod_float(
                sim,
                &effect_assumed_kind(op, "SpellMod_BaseDamage_Flat"),
                self.mod_config(e, SpellModType::BaseDamageFlat),
                v,
            );
        }
        None
    }

    fn pct_mod(&self, sim: &mut Sim, e: &Effect, v: f64) -> Option<Attachment> {
        use dbcenums::*;
        let op = e.misc;
        if op == SPELLMOD_DAMAGE || op == SPELLMOD_ALL_EFFECTS {
            return self.mod_float(
                sim,
                "SpellMod_DamageDone_Flat",
                self.mod_config(e, SpellModType::DamageDoneFlat),
                v,
            );
        }
        if op == SPELLMOD_DOT {
            return self.mod_float(
                sim,
                "SpellMod_DotDamageDone_Pct",
                self.mod_config(e, SpellModType::DotDamageDonePct),
                v,
            );
        }
        if op == SPELLMOD_COST {
            return self.mod_float(
                sim,
                "SpellMod_PowerCost_Pct_Add",
                self.mod_config(e, SpellModType::PowerCostPctAdd),
                v,
            );
        }
        if op == SPELLMOD_CASTING_TIME {
            return self.mod_float(
                sim,
                "SpellMod_CastTime_Pct",
                self.mod_config(e, SpellModType::CastTimePct),
                v,
            );
        }
        if op == SPELLMOD_COOLDOWN {
            return self.mod_multiplier(
                sim,
                "SpellMod_Cooldown_Multiplier",
                self.mod_config(e, SpellModType::CooldownMultiplier),
                1.0 + v,
            );
        }
        if op == SPELLMOD_CRIT_DAMAGE_BONUS {
            return self.mod_float(
                sim,
                "SpellMod_CritMultiplier_Flat",
                self.mod_config(e, SpellModType::CritMultiplierFlat),
                v,
            );
        }
        if op == SPELLMOD_THREAT {
            return self.mod_float(
                sim,
                "SpellMod_ThreatMultiplier_Pct",
                self.mod_config(e, SpellModType::ThreatMultiplierPct),
                v,
            );
        }
        if op == SPELLMOD_DURATION {
            return self.mod_float(
                sim,
                "SpellMod_DotBaseDuration_Pct",
                self.mod_config(e, SpellModType::DotBaseDurationPct),
                v,
            );
        }
        if op == SPELLMOD_EFFECT1 || op == SPELLMOD_EFFECT2 || op == SPELLMOD_EFFECT3 {
            return self.mod_float(
                sim,
                &effect_assumed_kind(op, "SpellMod_DamageDone_Flat"),
                self.mod_config(e, SpellModType::DamageDoneFlat),
                v,
            );
        }
        None
    }

    /// Go `parser.modConfig`.
    fn mod_config(&self, e: &Effect, kind: SpellModType) -> SpellModConfig {
        let mut flags = e.class_flags;
        if flags.is_zero() && self.class_flags.family != 0 {
            flags = ClassFlags {
                family: self.class_flags.family,
                mask: [u32::MAX; 4],
            };
        }
        SpellModConfig {
            kind,
            class_flags: flags,
            ..Default::default()
        }
    }

    fn mod_attachment(
        &self,
        sim: &mut Sim,
        kind: &str,
        config: SpellModConfig,
        value: f64,
        scale: ScaleFn,
    ) -> Attachment {
        let mut attachment = Attachment::new(kind.to_string(), value);
        if self.dry {
            return attachment;
        }
        let id = sim.add_dynamic_mod(self.unit, config);
        attachment.set = Some(Rc::new(move |sim: &mut Sim, _, level| {
            if level <= 0.0 {
                sim.deactivate_spell_mod(id);
                return;
            }
            scale(sim, id, level);
            sim.activate_spell_mod(id);
        }));
        attachment
    }

    fn mod_float(
        &self,
        sim: &mut Sim,
        kind: &str,
        mut config: SpellModConfig,
        v: f64,
    ) -> Option<Attachment> {
        config.float_value = v;
        Some(self.mod_attachment(
            sim,
            kind,
            config,
            v,
            Rc::new(move |sim: &mut Sim, id, level| {
                sim.update_spell_mod_float_value(id, v * level)
            }),
        ))
    }

    fn mod_int(
        &self,
        sim: &mut Sim,
        kind: &str,
        mut config: SpellModConfig,
        v: f64,
    ) -> Option<Attachment> {
        config.int_value = v as i32;
        Some(self.mod_attachment(
            sim,
            kind,
            config,
            v,
            Rc::new(move |sim: &mut Sim, id, level| {
                sim.update_spell_mod_int_value(id, (v * level) as i32)
            }),
        ))
    }

    fn mod_time(
        &self,
        sim: &mut Sim,
        kind: &str,
        mut config: SpellModConfig,
        ms: f64,
    ) -> Option<Attachment> {
        config.time_value = duration_from_millis(ms);
        Some(self.mod_attachment(
            sim,
            kind,
            config,
            ms,
            Rc::new(move |sim: &mut Sim, id, level| {
                sim.update_spell_mod_time_value(id, duration_from_millis(ms * level))
            }),
        ))
    }

    fn mod_multiplier(
        &self,
        sim: &mut Sim,
        kind: &str,
        mut config: SpellModConfig,
        mult: f64,
    ) -> Option<Attachment> {
        if self.stacking {
            return None;
        }
        config.float_value = mult;
        let mut attachment = self.mod_attachment(
            sim,
            kind,
            config,
            mult,
            Rc::new(move |sim: &mut Sim, id, _| sim.update_spell_mod_float_value(id, mult)),
        );
        attachment.multiplicative = true;
        Some(attachment)
    }

    fn add_stats(&self, stats: Vec<Stat>) -> SetFn {
        let unit = self.unit;
        Rc::new(move |sim: &mut Sim, live, delta| {
            let mut bonus = Stats::default();
            for stat in &stats {
                bonus[*stat] = delta;
            }
            if live {
                sim.add_stats_dynamic(unit, &bonus);
            } else {
                sim.add_stats(unit, &bonus);
            }
        })
    }

    /// Go `statsBuff`.
    fn stats_buff(&self, sim: &mut Sim, stats: Vec<Stat>, v: f64) -> Option<Attachment> {
        if stats.is_empty() {
            return None;
        }
        let mut attachment = additive(
            sim,
            format!("stat {}", stat_names(&stats)),
            v,
            self.add_stats(stats.clone()),
        );
        attachment.stats = stats.clone();
        for stat in stats {
            let mut part = additive(
                sim,
                format!("stat {}", stat.name()),
                v,
                self.add_stats(vec![stat]),
            );
            part.key = stat.name().to_string();
            part.stat = Some(stat);
            part.stat_bid = true;
            attachment.parts.push(part);
        }
        Some(attachment)
    }

    /// Go `statMultiplier`.
    fn stat_multiplier(&self, sim: &mut Sim, stats: Vec<Stat>, mult: f64) -> Option<Attachment> {
        if stats.is_empty() || self.stacking {
            return None;
        }
        let kind = format!("multiply-stat {}", stat_names(&stats));
        let unit = self.unit;
        if self.is_static {
            if self.conditional {
                return None;
            }
            let applied = sim.new_state_slot(0.0);
            let mut attachment = Attachment::new(kind, mult);
            attachment.multiplicative = true;
            attachment.stats = stats.clone();
            attachment.set = Some(Rc::new(move |sim: &mut Sim, _, level| {
                if level <= 0.0 || sim.state_flag(applied) {
                    return;
                }
                sim.set_state_flag(applied, true);
                for stat in &stats {
                    sim.unit_mut(unit).sdm.multiply_stat(*stat, mult);
                }
            }));
            return Some(attachment);
        }
        let mut attachment = Attachment::new(kind, mult);
        attachment.multiplicative = true;
        attachment.stats = stats.clone();
        for stat in &stats {
            let mut part = Attachment::new(format!("multiply-stat {}", stat.name()), mult);
            part.multiplicative = true;
            part.key = stat.name().to_string();
            part.stat = Some(*stat);
            part.stat_bid = true;
            if !self.dry {
                let dep = sim
                    .unit_mut(unit)
                    .sdm
                    .new_dynamic_multiply_stat(*stat, mult);
                let active = sim.new_state_slot(0.0);
                part.set = Some(Rc::new(move |sim: &mut Sim, _, level| {
                    if (level > 0.0) == sim.state_flag(active) {
                        return;
                    }
                    sim.set_state_flag(active, level > 0.0);
                    if sim.state_flag(active) {
                        sim.enable_build_phase_stat_dep(unit, dep);
                    } else {
                        sim.disable_build_phase_stat_dep(unit, dep);
                    }
                }));
            }
            attachment.parts.push(part);
        }
        let parts = attachment.parts.clone();
        let active = sim.new_state_slot(0.0);
        attachment.set = Some(Rc::new(move |sim: &mut Sim, live, level| {
            if (level > 0.0) == sim.state_flag(active) {
                return;
            }
            sim.set_state_flag(active, level > 0.0);
            assert!(
                !live || sim.unit(unit).on_temporary_stats_changes.is_empty(),
                "temporary stat listeners are not prepared"
            );
            for part in &parts {
                part.apply(sim, live, level);
            }
        }));
        Some(attachment)
    }

    /// Go `equipScaling`.
    fn equip_scaling(&self, sim: &mut Sim, stat: Stat, mult: f64) -> Option<Attachment> {
        let character = self.character?;
        if self.is_pet || self.stacking || (self.is_static && self.conditional) || mult <= 0.0 {
            return None;
        }
        Some(multiplier(
            sim,
            format!("equip-scaling {}", stat.name()),
            mult,
            Rc::new(move |sim: &mut Sim, live, factor| {
                if live {
                    sim.apply_dynamic_equip_scaling(character, stat, factor);
                } else {
                    sim.apply_equip_scaling(character, stat, factor);
                }
            }),
        ))
    }

    /// Go `pseudoMultiplier`.
    fn pseudo_multiplier(
        &self,
        sim: &mut Sim,
        kind: &str,
        key: &str,
        fields: Vec<PseudoField>,
        mult: f64,
    ) -> Option<Attachment> {
        if self.stacking || mult <= 0.0 {
            return None;
        }
        let unit = self.unit;
        let mut attachment = multiplier(
            sim,
            kind.to_string(),
            mult,
            Rc::new(move |sim: &mut Sim, _, factor| {
                for field in &fields {
                    *field.get(&mut sim.unit_mut(unit).pseudo_stats) *= factor;
                }
            }),
        );
        attachment.key = key.to_string();
        Some(attachment)
    }

    /// Go `pseudoAdd`.
    fn pseudo_add(
        &self,
        sim: &mut Sim,
        kind: &str,
        key: &str,
        field: PseudoField,
        v: f64,
    ) -> Attachment {
        let unit = self.unit;
        let mut attachment = additive(
            sim,
            kind.to_string(),
            v,
            Rc::new(move |sim: &mut Sim, _, delta| {
                *field.get(&mut sim.unit_mut(unit).pseudo_stats) += delta;
            }),
        );
        attachment.key = key.to_string();
        attachment
    }

    /// Go `parser.speed`.
    fn speed(
        &self,
        sim: &mut Sim,
        kind: &str,
        key: &str,
        speed: SpeedKind,
        mult: f64,
    ) -> Option<Attachment> {
        if self.is_static || self.stacking {
            return None;
        }
        let unit = self.unit;
        let mut attachment = multiplier(
            sim,
            kind.to_string(),
            mult,
            Rc::new(move |sim: &mut Sim, _, factor| match speed {
                SpeedKind::Cast => sim.multiply_cast_speed(unit, factor),
                SpeedKind::Melee => sim.multiply_melee_speed(unit, factor),
                SpeedKind::Attack => sim.multiply_attack_speed(unit, factor),
            }),
        );
        attachment.key = key.to_string();
        attachment.needs_sim = true;
        Some(attachment)
    }

    /// Go `schoolMultiplier`.
    #[allow(clippy::too_many_arguments)]
    fn school_multiplier(
        &self,
        sim: &mut Sim,
        kind: &str,
        key: &str,
        mask: i32,
        mult: f64,
        all: PseudoField,
        per_school: fn(usize) -> PseudoField,
    ) -> Option<Attachment> {
        if mask == MISC_ALL_SCHOOLS {
            return self.pseudo_multiplier(sim, kind, key, vec![all], mult);
        }
        let fields: Vec<PseudoField> = school_indexes(mask).into_iter().map(per_school).collect();
        if fields.is_empty() {
            return None;
        }
        self.pseudo_multiplier(
            sim,
            &format!("{kind}-by-school"),
            &format!("School{key}"),
            fields,
            mult,
        )
    }
}

#[derive(Clone, Copy)]
enum SpeedKind {
    Cast,
    Melee,
    Attack,
}

/// Go `effectAssumedKind`.
fn effect_assumed_kind(op: i32, kind: &str) -> String {
    let n = if op == dbcenums::SPELLMOD_EFFECT2 {
        2
    } else if op == dbcenums::SPELLMOD_EFFECT3 {
        3
    } else {
        1
    };
    format!("effect{n}-assumed-damage {kind}")
}

/// Go `core.DurationFromMillis` of a float: `time.Duration(ms * float64(time.Millisecond))`.
fn duration_from_millis(ms: f64) -> Duration {
    (ms * MILLISECOND as f64) as Duration
}

const _: () = assert!(SCHOOL_LEN == 8);
#[allow(dead_code)]
fn _resource_marker(_: Resource) {}
