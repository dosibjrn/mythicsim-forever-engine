//! Go metrics_aggregator.go and the `RaidSimResult` subset Rust reports.
//!
//! Field names follow Go's protojson output and zero values are omitted, as protojson
//! does by default, so Rust and Go results can be compared field by field.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::{
    contracts::prepared_v2::ActionId,
    core::time::{milliseconds, seconds, NS_PER_SECOND},
};

use super::{Agent, AuraRef, DamageRange, Fight, Side};

/// Go `aggregator`: count, sum and sum of squares.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Aggregator {
    n: u32,
    sum: f64,
    sum_sq: f64,
}

impl Aggregator {
    pub(crate) fn add(&mut self, value: f64) {
        self.n += 1;
        self.sum += value;
        // The arm64 build fuses the square into the sum.
        self.sum_sq = value.mul_add(value, self.sum_sq);
    }

    /// Go `meanAndStdDev`, whose mean squared the arm64 build fuses into the subtraction.
    fn mean_and_stdev(&self) -> (f64, f64) {
        let mean = self.sum / f64::from(self.n);
        (
            mean,
            (-mean)
                .mul_add(mean, self.sum_sq / f64::from(self.n))
                .sqrt(),
        )
    }
}

/// Go `DistributionMetrics`.
#[derive(Clone, Debug)]
pub(crate) struct Distribution {
    pub(crate) total: f64,
    aggregate: Aggregator,
    max: f64,
    min: f64,
    max_seed: i64,
    min_seed: i64,
    hist: BTreeMap<i32, i32>,
    /// Go `sample`: every iteration's value, kept only under `saveAllValues`.
    values: Vec<f64>,
}

impl Default for Distribution {
    fn default() -> Self {
        Self {
            total: 0.0,
            aggregate: Aggregator::default(),
            max: 0.0,
            min: -1.0,
            max_seed: 0,
            min_seed: 0,
            hist: BTreeMap::new(),
            values: Vec::new(),
        }
    }
}

impl Distribution {
    /// Go `DistributionMetrics.doneIteration`; `keep` is Go `SaveAllValues`.
    pub(crate) fn done_iteration(&mut self, duration: i64, seed: i64, keep: bool) {
        let value = self.total / seconds(duration);
        self.aggregate.add(value);
        if keep {
            self.values.push(value);
        }
        if value > self.max {
            self.max = value;
            self.max_seed = seed;
        }
        if value <= self.min || self.min < 0.0 {
            self.min = value;
            self.min_seed = seed;
        }
        let rounded = ((value / 10.0).round() * 10.0) as i32;
        *self.hist.entry(rounded).or_default() += 1;
        self.total = 0.0;
    }

    pub(crate) fn report(&self) -> DistributionReport {
        let (avg, stdev) = self.aggregate.mean_and_stdev();
        DistributionReport {
            avg,
            stdev,
            max: self.max,
            min: self.min,
            max_seed: self.max_seed.to_string(),
            min_seed: self.min_seed.to_string(),
            hist: self.hist.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            all_values: self.values.clone(),
            aggregator_data: AggregatorData {
                n: self.aggregate.n,
                sum_sq: self.aggregate.sum_sq,
            },
        }
    }
}

/// Per-unit aggregates across iterations.
#[derive(Default)]
pub(crate) struct Totals {
    pub(crate) iteration_damage: f64,
    pub(crate) dps: Distribution,
    pub(crate) threat: Distribution,
    pub(crate) tto: Distribution,
    /// Each target's damage taken, by position.
    pub(crate) target_dtps: Vec<Distribution>,
    /// Damage the player takes, from its own spells and the target's swings.
    pub(crate) player_dtps: Distribution,
    /// Go `UnitMetrics.hps` of the player: its healing on units that are not opponents.
    pub(crate) player_hps: Distribution,
    /// A healer sim's best burst window each fight, over its length ([`super::burst`]).
    pub(crate) burst_hps: Distribution,
    /// Each target's damage and threat from its swings at the player, by position.
    pub(crate) target_dps: Vec<Distribution>,
    pub(crate) target_threat: Vec<Distribution>,
    /// Every distribution Go keeps that stays zero in scope: damage taken by the player, TMI,
    /// and the target's own output.
    pub(crate) zero: Distribution,
    pub(crate) oom_seconds: f64,
    pub(crate) iterations: u32,
}

fn is_zero_f(value: &f64) -> bool {
    *value == 0.0
}

fn is_zero_i(value: &i32) -> bool {
    *value == 0
}

fn is_zero_u8(value: &u8) -> bool {
    *value == 0
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

fn is_zero_string(value: &str) -> bool {
    value == "0"
}

/// A deviation as protojson writes it: a number, or "NaN" when cancellation left
/// `sqrt(sumSq/n - mean^2)` a negative residue, as Go reports it.
fn deviation<S: serde::Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    if value.is_nan() {
        serializer.serialize_str("NaN")
    } else {
        serializer.serialize_f64(*value)
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActionIdReport {
    #[serde(skip_serializing_if = "is_zero_i")]
    spell_id: i32,
    #[serde(skip_serializing_if = "is_zero_i")]
    item_id: i32,
    #[serde(skip_serializing_if = "String::is_empty")]
    other_id: String,
    #[serde(skip_serializing_if = "is_zero_i")]
    tag: i32,
}

impl From<&ActionId> for ActionIdReport {
    fn from(id: &ActionId) -> Self {
        ActionIdReport {
            spell_id: id.spell_id,
            item_id: id.item_id,
            other_id: id.other_id.clone(),
            tag: id.tag,
        }
    }
}

/// Go `TargetedActionMetrics`, accumulated over every iteration.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActionReport {
    #[serde(skip_serializing_if = "is_zero_i")]
    pub(crate) unit_index: i32,
    #[serde(skip_serializing_if = "is_zero_i")]
    pub(crate) casts: i32,
    #[serde(skip_serializing_if = "is_zero_i")]
    pub(crate) hits: i32,
    #[serde(skip_serializing_if = "is_zero_i")]
    pub(crate) resisted_hits: i32,
    #[serde(skip_serializing_if = "is_zero_i")]
    pub(crate) crits: i32,
    #[serde(skip_serializing_if = "is_zero_i")]
    pub(crate) resisted_crits: i32,
    #[serde(skip_serializing_if = "is_zero_i")]
    pub(crate) ticks: i32,
    #[serde(skip_serializing_if = "is_zero_i")]
    pub(crate) resisted_ticks: i32,
    #[serde(skip_serializing_if = "is_zero_i")]
    pub(crate) crit_ticks: i32,
    #[serde(skip_serializing_if = "is_zero_i")]
    pub(crate) resisted_crit_ticks: i32,
    #[serde(skip_serializing_if = "is_zero_i")]
    pub(crate) misses: i32,
    #[serde(skip_serializing_if = "is_zero_i")]
    pub(crate) dodges: i32,
    #[serde(skip_serializing_if = "is_zero_i")]
    pub(crate) parries: i32,
    #[serde(skip_serializing_if = "is_zero_i")]
    pub(crate) blocks: i32,
    #[serde(skip_serializing_if = "is_zero_i")]
    pub(crate) blocked_crits: i32,
    #[serde(skip_serializing_if = "is_zero_i")]
    pub(crate) glances: i32,
    #[serde(skip_serializing_if = "is_zero_i")]
    pub(crate) crushes: i32,
    #[serde(skip_serializing_if = "is_zero_f")]
    pub(crate) damage: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    pub(crate) resisted_damage: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    pub(crate) crit_damage: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    pub(crate) resisted_crit_damage: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    pub(crate) tick_damage: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    pub(crate) resisted_tick_damage: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    pub(crate) crit_tick_damage: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    pub(crate) resisted_crit_tick_damage: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    pub(crate) glance_damage: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    pub(crate) block_damage: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    pub(crate) blocked_crit_damage: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    pub(crate) crush_damage: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    pub(crate) threat: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    pub(crate) healing: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    pub(crate) crit_healing: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    pub(crate) overhealing: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    pub(crate) shielding: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    pub(crate) cast_time_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) hit_range: Option<DamageRangeReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) crit_range: Option<DamageRangeReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tick_range: Option<DamageRangeReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) crit_tick_range: Option<DamageRangeReport>,
    #[serde(skip)]
    cast_time: i64,
    #[serde(skip)]
    pub(crate) ranges: [DamageRange; 4],
}

/// Go `DamageRange` as protojson writes it: the average is total / count.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DamageRangeReport {
    #[serde(skip_serializing_if = "is_zero_i")]
    count: i32,
    #[serde(skip_serializing_if = "is_zero_f")]
    total: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    min: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    max: f64,
}

impl DamageRangeReport {
    /// Go `DamageRange.ToProto`: unset when no event of the kind landed.
    pub(crate) fn of(range: &DamageRange) -> Option<Self> {
        (range.count != 0).then_some(DamageRangeReport {
            count: range.count,
            total: range.total,
            min: range.min,
            max: range.max,
        })
    }
}

impl ActionReport {
    pub(crate) fn new(unit_index: i32) -> Self {
        ActionReport {
            unit_index,
            ..ActionReport::default()
        }
    }
}

/// An action's metrics on each unit a spell can hit, by [`super::Side::index`]; the report
/// gives each its Go unit index.
pub(crate) fn defender_reports() -> [ActionReport; super::DEFENDERS] {
    std::array::from_fn(|_| ActionReport::default())
}

/// Go `ActionMetrics`: one entry per action ID, shared by spells with that ID.
#[derive(Clone, Debug)]
pub(crate) struct ActionTotals {
    pub(crate) id: ActionId,
    pub(crate) melee: bool,
    pub(crate) passive: bool,
    pub(crate) school: u8,
    pub(crate) targets: [ActionReport; super::DEFENDERS],
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActionMetricsReport {
    id: ActionIdReport,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    is_melee: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    is_passive: bool,
    targets: Vec<ActionReport>,
    #[serde(skip_serializing_if = "is_zero_u8")]
    spell_school: u8,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AggregatorData {
    #[serde(skip_serializing_if = "is_zero_u32")]
    n: u32,
    #[serde(skip_serializing_if = "is_zero_f")]
    sum_sq: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuraMetricsReport {
    id: ActionIdReport,
    #[serde(skip_serializing_if = "is_zero_f")]
    uptime_seconds_avg: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    #[serde(serialize_with = "deviation")]
    uptime_seconds_stdev: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    procs_avg: f64,
    aggregator_data: AggregatorData,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    effects: Vec<AuraEffectMetricsReport>,
}

/// Go `AuraEffectMetrics`: an exclusive effect's average time holding its category.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuraEffectMetricsReport {
    category: String,
    #[serde(skip_serializing_if = "is_zero_f")]
    uptime_seconds_avg: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceMetricsReport {
    id: ActionIdReport,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "is_zero_i")]
    events: i32,
    #[serde(skip_serializing_if = "is_zero_f")]
    gain: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    actual_gain: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DistributionReport {
    #[serde(skip_serializing_if = "is_zero_f")]
    avg: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    #[serde(serialize_with = "deviation")]
    stdev: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    max: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    min: f64,
    #[serde(skip_serializing_if = "is_zero_string")]
    max_seed: String,
    #[serde(skip_serializing_if = "is_zero_string")]
    min_seed: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    hist: BTreeMap<String, i32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    all_values: Vec<f64>,
    aggregator_data: AggregatorData,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UnitReport {
    name: String,
    #[serde(skip_serializing_if = "is_zero_i")]
    unit_index: i32,
    dps: DistributionReport,
    threat: DistributionReport,
    dtps: DistributionReport,
    tmi: DistributionReport,
    hps: DistributionReport,
    /// MythicSim's healer sims: healing per second over the best burst window of each fight.
    #[serde(skip_serializing_if = "Option::is_none")]
    burst_hps: Option<DistributionReport>,
    tto: DistributionReport,
    #[serde(skip_serializing_if = "is_zero_f")]
    seconds_oom_avg: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    chance_of_death: f64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    death_seeds: Vec<String>,
    actions: Vec<ActionMetricsReport>,
    auras: Vec<AuraMetricsReport>,
    resources: Vec<ResourceMetricsReport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pets: Vec<UnitReport>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TargetReport {
    name: String,
    #[serde(skip_serializing_if = "is_zero_i")]
    unit_index: i32,
    dps: DistributionReport,
    threat: DistributionReport,
    dtps: DistributionReport,
    tmi: DistributionReport,
    hps: DistributionReport,
    tto: DistributionReport,
    actions: Vec<ActionMetricsReport>,
    auras: Vec<AuraMetricsReport>,
    /// The target's own resource metrics, as the heal of a Judgement of Light on it; protojson
    /// leaves the list out when it is empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    resources: Vec<ResourceMetricsReport>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PartyReport {
    dps: DistributionReport,
    hps: DistributionReport,
    players: Vec<UnitReport>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RaidReport {
    dps: DistributionReport,
    hps: DistributionReport,
    parties: Vec<PartyReport>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EncounterReport {
    targets: Vec<TargetReport>,
}

/// Go `RaidSimResult` for one player and one target.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FightReport {
    raid_metrics: RaidReport,
    encounter_metrics: EncounterReport,
    #[serde(skip_serializing_if = "String::is_empty")]
    logs: String,
    first_iteration_duration: f64,
    avg_iteration_duration: f64,
    iterations_done: u32,
    #[serde(skip)]
    pub(crate) elapsed_ns: u64,
    /// The rotation as MythicSim's Go CLI adapter presents it, which `sim --rotation-stats`
    /// adds to the player's metrics.
    #[serde(skip)]
    pub(crate) presentation: Option<super::rotation_checks::Presentation>,
    /// The logged fight's strikes that replaced an extra attack, which `sim --rotation-stats`
    /// adds to the player's metrics.
    #[serde(skip)]
    pub(crate) extra_attack_strikes: Vec<serde_json::Value>,
}

impl<A: Agent> Fight<A> {
    fn iteration_seed(&self) -> i64 {
        self.config.seed + i64::from(self.totals.iterations)
    }

    /// Go `manaBar.doneIteration`: out-of-mana time and mana-gain threat.
    pub(crate) fn player_done_iteration(&mut self) {
        self.mana_done_iteration(Side::Player);
    }

    /// Go `manaBar.doneIteration` of an acting unit.
    pub(crate) fn mana_done_iteration(&mut self, side: Side) {
        let now = self.now;
        let unit = self.unit_mut(side);
        if unit.waiting_for_mana != 0.0 {
            let duration = now - unit.waiting_for_mana_start;
            if duration > 0 {
                unit.oom_time += duration;
                if !unit.went_oom {
                    unit.went_oom = true;
                    unit.first_oom = now;
                }
            }
        }
        let gain_spell = match side {
            Side::Pet(_) => self.active_pet(side).mana_gain_spell,
            _ => self.mana_gain_spell,
        };
        let Some(gain_spell) = gain_spell else {
            return;
        };
        for index in 0..self.resources.len() {
            let resource = &self.resources[index];
            if resource.unit != side
                || resource.kind != super::ResourceKind::Mana
                || resource.is_mana_regen
                || resource.no_threat
                || resource.id.other_id == "OtherActionManaRegen"
            {
                continue;
            }
            let actual = resource.actual_gain - resource.previous_actual_gain;
            if actual <= 0.0 {
                continue;
            }
            let events = resource.events - resource.previous_events;
            self.spells[gain_spell].metrics[0].casts += events;
            // Go ApplyAOEThreatIgnoreMultipliers on every target, whose sum the arm64 build
            // fuses.
            for position in 0..self.targets.len() {
                let slot = Side::target(position).index();
                let threat = &mut self.spells[gain_spell].metrics[slot].total_threat;
                *threat = actual.mul_add(0.5, *threat);
            }
        }
    }

    /// Go `Spell.doneIteration` and `UnitMetrics.addSpellMetrics`, for the caster's metrics.
    pub(crate) fn spell_done_iteration(&mut self, spell: usize) {
        if self.spells[spell].action.is_none() {
            return;
        }
        if self.spells[spell].split_metrics.is_empty() {
            let action = self.spells[spell].action.expect("checked above");
            self.add_spell_metrics(spell, action, None);
            return;
        }
        // Go Spell.doneIteration: each split under its own tagged ID.
        let state = &mut self.spells[spell];
        state.split_metrics[state.split] = state.metrics;
        for split in 0..self.spells[spell].split_metrics.len() {
            let action = self.spells[spell].split_actions[split];
            self.add_spell_metrics(spell, action, Some(split));
        }
    }

    /// Go `UnitMetrics.addSpellMetrics` for the spell's metrics, or a split's.
    fn add_spell_metrics(&mut self, spell: usize, action: usize, split: Option<usize>) {
        let passive = self.spells[spell].flags.passive;
        let caster = self.spells[spell].caster;
        // Go folds each unit's metrics in unit index order: every target, then the player.
        for target in 0..self.defender_count() {
            let metrics = match split {
                None => self.spells[spell].metrics[target],
                Some(split) => self.spells[spell].split_metrics[split][target],
            };
            let actions = match caster {
                Side::Pet(_) => &mut self.active_pet_mut(caster).actions,
                _ => &mut self.actions,
            };
            let totals = &mut actions[action].targets[target];
            if !passive {
                totals.casts += metrics.casts;
            }
            totals.misses += metrics.misses;
            totals.dodges += metrics.dodges;
            totals.parries += metrics.parries;
            totals.blocks += metrics.blocks;
            totals.blocked_crits += metrics.blocked_crits;
            totals.glances += metrics.glances;
            totals.hits += metrics.hits;
            totals.resisted_hits += metrics.resisted_hits;
            totals.crits += metrics.crits;
            totals.resisted_crits += metrics.resisted_crits;
            totals.ticks += metrics.ticks;
            totals.resisted_ticks += metrics.resisted_ticks;
            totals.crit_ticks += metrics.crit_ticks;
            totals.resisted_crit_ticks += metrics.resisted_crit_ticks;
            totals.damage += metrics.total_damage;
            totals.resisted_damage += metrics.total_resisted_damage;
            totals.crit_damage += metrics.total_crit_damage;
            totals.resisted_crit_damage += metrics.total_resisted_crit_damage;
            totals.tick_damage += metrics.total_tick_damage;
            totals.resisted_tick_damage += metrics.total_resisted_tick_damage;
            totals.crit_tick_damage += metrics.total_crit_tick_damage;
            totals.resisted_crit_tick_damage += metrics.total_resisted_crit_tick_damage;
            totals.glance_damage += metrics.total_glance_damage;
            totals.block_damage += metrics.total_block_damage;
            totals.blocked_crit_damage += metrics.total_blocked_crit_damage;
            totals.crushes += metrics.crushes;
            totals.crush_damage += metrics.total_crush_damage;
            totals.threat += metrics.total_threat;
            totals.ranges[0].merge(&metrics.hit_range);
            totals.ranges[1].merge(&metrics.crit_range);
            totals.ranges[2].merge(&metrics.tick_range);
            totals.ranges[3].merge(&metrics.crit_tick_range);
            totals.healing += metrics.total_healing;
            totals.crit_healing += metrics.total_crit_healing;
            totals.overhealing += metrics.total_overhealing;
            totals.shielding += metrics.total_shielding;
            if !passive {
                totals.cast_time += metrics.total_cast_time;
            }
            if target == Side::Player.index() {
                // Go adds damage on any unit to that unit's damage taken; the player is no
                // opponent of its own, so its healing goes to healing done.
                self.totals.player_dtps.total += metrics.total_damage;
                // Go metrics_aggregator: hps.Total += TotalHealing + TotalShielding.
                self.totals.player_hps.total += metrics.total_healing + metrics.total_shielding;
            }
            if target != Side::Player.index() {
                let position = if target == Side::Target.index() {
                    0
                } else {
                    target - 1
                };
                self.totals.target_dtps[position].total += metrics.total_damage;
                match caster {
                    Side::Pet(_) => {
                        let totals = &mut self.active_pet_mut(caster).totals;
                        totals.dps.total += metrics.total_damage;
                        totals.threat.total += metrics.total_threat;
                    }
                    _ => {
                        self.totals.dps.total += metrics.total_damage;
                        self.totals.threat.total += metrics.total_threat;
                    }
                }
            }
        }
    }

    /// Go's time to out of mana for an acting unit at the end of a fight, its mana clamped to
    /// the lowest maximum of its aura teardown.
    pub(crate) fn time_to_oom(&self, side: Side, teardown_max_mana: f64) -> i64 {
        let unit = self.unit(side);
        let duration_seconds = seconds(self.duration);
        let time_to_oom = if unit.went_oom {
            unit.first_oom
        } else {
            let spent_per_second = (unit.mana_spent - unit.mana_gained) / duration_seconds;
            if spent_per_second > 0.0 {
                // Go's aura teardown clamps current mana to the falling maximum first.
                let mana = unit.mana.min(teardown_max_mana);
                let remaining = crate::core::time::from_seconds(mana / spent_per_second);
                // Go adds durations with int64 wraparound; an overflow turns negative and
                // falls back to 60 minutes below.
                crate::core::time::from_seconds(duration_seconds)
                    .wrapping_add(remaining)
                    .min(60 * 60 * NS_PER_SECOND)
            } else {
                60 * 60 * NS_PER_SECOND
            }
        };
        if time_to_oom < 0 {
            60 * 60 * NS_PER_SECOND
        } else {
            time_to_oom
        }
    }

    /// Go `UnitMetrics.doneIteration` for the player, its pets and the target.
    pub(crate) fn unit_done_iteration(&mut self, _damage: f64) {
        let seed = self.iteration_seed();
        let duration_seconds = seconds(self.duration);
        let time_to_oom = self.time_to_oom(Side::Player, self.config.teardown_max_mana);
        // Go infers time to out of mana only for a unit with a mana bar.
        self.totals.tto.total = if self.has_mana_bar() {
            seconds(time_to_oom) * duration_seconds
        } else {
            0.0
        };
        let duration = self.duration;
        let keep = self.config.save_all_values;
        self.totals.dps.done_iteration(duration, seed, keep);
        self.totals.threat.done_iteration(duration, seed, keep);
        self.totals.tto.done_iteration(duration, seed, keep);
        for dtps in &mut self.totals.target_dtps {
            dtps.done_iteration(duration, seed, keep);
        }
        self.totals.player_dtps.done_iteration(duration, seed, keep);
        self.totals.player_hps.done_iteration(duration, seed, keep);
        if let Some(burst) = &self.burst {
            self.totals.burst_hps.total = burst.best;
            self.totals
                .burst_hps
                .done_iteration(burst.length, seed, keep);
        }
        for dps in &mut self.totals.target_dps {
            dps.done_iteration(duration, seed, keep);
        }
        for threat in &mut self.totals.target_threat {
            threat.done_iteration(duration, seed, keep);
        }
        if self.death.died {
            self.death.iterations_dead += 1;
            // Seeds rise with the iteration, so the list only needs a sort when one does not.
            let sorted = self.death.seeds.last().is_none_or(|&last| last <= seed);
            self.death.seeds.push(seed);
            if !sorted {
                self.death.seeds.sort_unstable();
            }
        }
        self.totals.zero.done_iteration(duration, seed, keep);
        self.totals.oom_seconds += seconds(self.player.oom_time);
        self.pets_metrics_done_iteration(seed);
        self.totals.iterations += 1;
    }

    fn aura_reports(&self, side: Side) -> Vec<AuraMetricsReport> {
        self.trackers[side.index()]
            .auras
            .iter()
            .enumerate()
            .filter_map(|(index, aura)| {
                let id = aura.metrics_id.as_ref()?;
                let n = aura.aggregate.n;
                let (avg, stdev) = aura.aggregate.mean_and_stdev();
                Some(AuraMetricsReport {
                    id: id.into(),
                    uptime_seconds_avg: avg,
                    uptime_seconds_stdev: stdev,
                    procs_avg: aura.procs_sum as f64 / f64::from(n),
                    aggregator_data: AggregatorData {
                        n,
                        sum_sq: aura.aggregate.sum_sq,
                    },
                    effects: self
                        .exclusive_effect_reports(AuraRef { side, index })
                        .into_iter()
                        .map(|(category, uptime_seconds_avg)| AuraEffectMetricsReport {
                            category,
                            uptime_seconds_avg,
                        })
                        .collect(),
                })
            })
            .collect()
    }

    /// The pets' metrics, in Go's registration order.
    fn pet_reports(
        &self,
        action_report: &dyn Fn(&ActionTotals) -> ActionMetricsReport,
        zero: &DistributionReport,
        n: f64,
    ) -> Vec<UnitReport> {
        let mut reports: Vec<(i32, UnitReport)> = self
            .pets
            .iter()
            .map(|pet| {
                (
                    pet.unit_index,
                    UnitReport {
                        name: pet.name.clone(),
                        unit_index: pet.unit_index,
                        dps: zero.clone(),
                        threat: zero.clone(),
                        dtps: zero.clone(),
                        tmi: zero.clone(),
                        hps: zero.clone(),
                        burst_hps: None,
                        tto: pet
                            .tto
                            .as_ref()
                            .map_or_else(|| zero.clone(), Distribution::report),
                        seconds_oom_avg: 0.0,
                        chance_of_death: 0.0,
                        death_seeds: Vec::new(),
                        actions: pet.actions.iter().map(action_report).collect(),
                        auras: pet
                            .auras
                            .iter()
                            .enumerate()
                            .map(|(position, id)| {
                                let uptime = &pet.aura_uptime[position];
                                let (avg, stdev) = uptime.mean_and_stdev();
                                AuraMetricsReport {
                                    id: id.into(),
                                    uptime_seconds_avg: avg,
                                    uptime_seconds_stdev: stdev,
                                    procs_avg: pet.aura_procs[position] as f64 / n,
                                    aggregator_data: AggregatorData {
                                        n: uptime.n,
                                        sum_sq: uptime.sum_sq,
                                    },
                                    // Go `GetMetricsProto`: each effect's held time over
                                    // the iterations it saw.
                                    effects: if uptime.n == 0 {
                                        Vec::new()
                                    } else {
                                        pet.aura_effects[position]
                                            .iter()
                                            .map(|category| AuraEffectMetricsReport {
                                                category: category.clone(),
                                                uptime_seconds_avg: seconds(
                                                    pet.aura_effect_uptime[position],
                                                ) / f64::from(uptime.n),
                                            })
                                            .collect()
                                    },
                                }
                            })
                            .collect(),
                        resources: Vec::new(),
                        pets: Vec::new(),
                    },
                )
            })
            .collect();
        for side in self.pet_sides() {
            let pet = self.active_pet(side);
            reports.push((
                pet.unit_index,
                UnitReport {
                    name: pet.name.clone(),
                    unit_index: pet.unit_index,
                    dps: pet.totals.dps.report(),
                    threat: pet.totals.threat.report(),
                    dtps: zero.clone(),
                    tmi: zero.clone(),
                    hps: zero.clone(),
                    burst_hps: None,
                    tto: pet.totals.tto.report(),
                    seconds_oom_avg: pet.totals.oom_seconds / n,
                    chance_of_death: 0.0,
                    death_seeds: Vec::new(),
                    actions: pet.actions.iter().map(action_report).collect(),
                    auras: self.aura_reports(side),
                    resources: self.resource_reports(side),
                    pets: Vec::new(),
                },
            ));
        }
        reports.sort_by_key(|(unit, _)| *unit);
        reports.into_iter().map(|(_, report)| report).collect()
    }

    /// A unit's resource metrics that saw an event.
    fn resource_reports(&self, side: Side) -> Vec<ResourceMetricsReport> {
        self.resources
            .iter()
            .filter(|resource| resource.unit == side && resource.events > 0)
            .map(|resource| ResourceMetricsReport {
                id: (&resource.id).into(),
                kind: resource.kind.proto_name(),
                events: resource.events,
                gain: resource.gain,
                actual_gain: resource.actual_gain,
            })
            .collect()
    }

    pub(crate) fn report(
        &self,
        first_duration: i64,
        total_duration: i64,
        logs: String,
        elapsed_ns: u64,
    ) -> FightReport {
        // Go lists every unit in each action's targets, in unit index order: the targets, the
        // player, then the pets, which never take a hit in scope.
        let extra_units = self.extra_unit_indexes();
        let defenders: Vec<(usize, i32)> = (0..self.targets.len())
            .map(|position| (Side::target(position).index(), position as i32))
            .chain(std::iter::once((
                Side::Player.index(),
                self.config.player_index,
            )))
            .collect();
        let action_report = |action: &ActionTotals| ActionMetricsReport {
            id: (&action.id).into(),
            is_melee: action.melee,
            is_passive: action.passive,
            targets: defenders
                .iter()
                .map(|&(slot, unit)| {
                    let mut target = action.targets[slot].clone();
                    target.unit_index = unit;
                    target.cast_time_ms = milliseconds(target.cast_time) as f64;
                    target.hit_range = DamageRangeReport::of(&target.ranges[0]);
                    target.crit_range = DamageRangeReport::of(&target.ranges[1]);
                    target.tick_range = DamageRangeReport::of(&target.ranges[2]);
                    target.crit_tick_range = DamageRangeReport::of(&target.ranges[3]);
                    target
                })
                .chain(extra_units.iter().map(|&unit| ActionReport::new(unit)))
                .collect(),
            spell_school: action.school,
        };
        let actions = self.actions.iter().map(action_report).collect();
        let resources = self.resource_reports(Side::Player);
        let n = f64::from(self.totals.iterations);
        let zero = self.totals.zero.report();
        let player = UnitReport {
            name: self.config.player_name.clone(),
            unit_index: self.config.player_index,
            dps: self.totals.dps.report(),
            threat: self.totals.threat.report(),
            dtps: self.totals.player_dtps.report(),
            tmi: zero.clone(),
            hps: self.totals.player_hps.report(),
            burst_hps: self.burst.as_ref().map(|_| self.totals.burst_hps.report()),
            tto: self.totals.tto.report(),
            seconds_oom_avg: self.totals.oom_seconds / n,
            chance_of_death: f64::from(self.death.iterations_dead) / n,
            death_seeds: self.death.seeds.iter().map(i64::to_string).collect(),
            actions,
            auras: self.aura_reports(Side::Player),
            resources,
            pets: self.pet_reports(&action_report, &zero, n),
        };
        // A target's own damage and threat are its swings at a tank; without one they stay zero.
        let targets = self
            .target_sides()
            .enumerate()
            .map(|(position, side)| TargetReport {
                name: self.label_of(side),
                unit_index: position as i32,
                dps: self.totals.target_dps[position].report(),
                threat: self.totals.target_threat[position].report(),
                dtps: self.totals.target_dtps[position].report(),
                tmi: zero.clone(),
                hps: zero.clone(),
                tto: zero.clone(),
                actions: self.target_actions[position]
                    .iter()
                    .map(action_report)
                    .collect(),
                auras: self.aura_reports(side),
                resources: self.resource_reports(side),
            })
            .collect();
        let dps = self.totals.dps.report();
        FightReport {
            raid_metrics: RaidReport {
                dps: dps.clone(),
                hps: self.totals.player_hps.report(),
                parties: vec![PartyReport {
                    dps,
                    hps: self.totals.player_hps.report(),
                    players: vec![player],
                }],
            },
            encounter_metrics: EncounterReport { targets },
            logs,
            first_iteration_duration: seconds(first_duration),
            avg_iteration_duration: seconds(total_duration) / n,
            iterations_done: self.totals.iterations,
            elapsed_ns,
            presentation: self.presentation.clone(),
            extra_attack_strikes: self.extra_attack_strikes.clone(),
        }
    }
}
