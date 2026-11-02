//! Go common/shared/shared_utils.go `spellDataProcDamageSpell`: the spell a weapon or item proc of
//! a client row casts on the unit it hit. The row's direct hit rolls once, or for a chain once a
//! target on the amount the previous jump keeps, or for an area once a target, or once for a
//! split one, on the attack table of the spell's defense type; the damage over time the row carries goes on with the hit when it landed, or alone,
//! unrolled, where the row has no hit. A hit with a missile speed is dealt once it has flown.

use crate::contracts::prepared_v2::{ProcArea, ProcChain, ProcPeriodic};

use super::{melee::PhysicalOutcome, Agent, Fight, Outcome, Side, SpellId, SpellResult};

/// The attack table the direct hit rolls, by shared_utils.go `damageOutcome`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum HitTable {
    /// The magic hit table, with the crit roll unless the client bars the spell from critting.
    Magic { can_crit: bool },
    /// A melee special or ranged attack table, which keeps the school's partial resist.
    Physical(PhysicalOutcome),
}

impl HitTable {
    /// The table of an exported outcome name; absent, the magic table.
    pub(crate) fn of(name: Option<&str>, can_crit: bool) -> Result<Self, String> {
        match name {
            None => Ok(Self::Magic { can_crit }),
            Some("melee_special_hit_and_crit") => {
                Ok(Self::Physical(PhysicalOutcome::MeleeSpecialHitAndCrit {
                    count: true,
                }))
            }
            Some("melee_special_hit") => Ok(Self::Physical(PhysicalOutcome::MeleeSpecialHit {
                count: true,
            })),
            // A row with the always-hit attribute: the crit roll and no miss.
            Some("melee_special_crit_only") => {
                Ok(Self::Physical(PhysicalOutcome::MeleeSpecialCritOnly {
                    count: true,
                }))
            }
            Some("ranged_hit_and_crit") => Ok(Self::Physical(PhysicalOutcome::RangedHitAndCrit {
                count: true,
            })),
            Some("ranged_hit") => Ok(Self::Physical(PhysicalOutcome::RangedHit { count: true })),
            Some(other) => Err(format!("unknown damage proc outcome {other}")),
        }
    }
}

/// The outcome applier each tick of the dot rolls, spelldata `Spell.TickOutcome`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum TickTable {
    /// A tick that rolls the magic hit again, then the crit.
    MagicHitAndCrit,
    /// A tick that crits on the physical crit chance.
    PhysicalCrit,
    /// A plain tick.
    Plain,
}

impl TickTable {
    fn of(name: &str) -> Result<Self, String> {
        match name {
            "tick_magic_hit_and_crit" => Ok(Self::MagicHitAndCrit),
            "tick_physical_crit" => Ok(Self::PhysicalCrit),
            "tick" => Ok(Self::Plain),
            other => Err(format!("unknown damage proc tick outcome {other}")),
        }
    }

    pub(crate) fn outcome(self) -> Outcome {
        match self {
            Self::MagicHitAndCrit => Outcome::TickMagicHitAndCrit,
            Self::PhysicalCrit => Outcome::TickPhysicalCrit,
            Self::Plain => Outcome::Tick,
        }
    }
}

/// The row's direct hit: its client effect and the table it rolls on.
#[derive(Clone, Copy, Debug)]
struct Direct {
    average: f64,
    variance: f64,
    table: HitTable,
}

/// The damage over time: each tick's amount and outcome applier.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Periodic {
    pub(crate) tick_base: f64,
    pub(crate) tick: TickTable,
}

/// The targets an area hit reaches.
#[derive(Clone, Copy, Debug)]
struct Area {
    /// The row's cap, zero for every target.
    max_targets: usize,
    splits: bool,
    aoe_cap_multiplier: f64,
}

/// A weapon or item proc's damage spell.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ProcDamage {
    direct: Option<Direct>,
    chain: Option<(usize, f64)>,
    area: Option<Area>,
    periodic: Option<Periodic>,
    /// The hit roll a damage over time without a direct hit makes when it is applied.
    application: Option<Outcome>,
}

impl ProcDamage {
    /// The spell of an exported damage proc, or why the runtime cannot roll it.
    pub(crate) fn new(
        average: f64,
        variance: f64,
        can_crit: bool,
        outcome: Option<&str>,
        chain: Option<&ProcChain>,
        area: Option<&ProcArea>,
        periodic: Option<&ProcPeriodic>,
    ) -> Result<Self, String> {
        let periodic_hit = periodic.map(|periodic| periodic.with_direct);
        let direct = match periodic_hit {
            // Alone, the dot goes on unrolled: there is no hit to name a table for.
            Some(false) => None,
            _ => Some(Direct {
                average,
                variance,
                table: HitTable::of(outcome, can_crit)?,
            }),
        };
        if direct.is_some_and(|direct| direct.average == 0.0 && direct.variance == 0.0) {
            return Err("a client effect that deals nothing".to_string());
        }
        if direct.is_none() && (chain.is_some() || outcome.is_some()) {
            return Err("a damage over time alone has no hit to spread or roll".to_string());
        }
        // An area with no hit is a damage over time put on each target the area reaches, as
        // Seeping Willow's (17196) is on every enemy around the wielder.
        if direct.is_some() && periodic.is_some() && (chain.is_some() || area.is_some()) {
            return Err("a spread hit leaves no damage over time".to_string());
        }
        if chain.is_some() && area.is_some() {
            return Err("a hit is a chain or an area".to_string());
        }
        let area = area
            .map(|area| {
                usize::try_from(area.max_targets)
                    .map(|max_targets| Area {
                        max_targets,
                        splits: area.splits,
                        aoe_cap_multiplier: area.aoe_cap_multiplier,
                    })
                    .map_err(|_| format!("an area capped at {} targets", area.max_targets))
            })
            .transpose()?;
        let chain = chain
            .map(|chain| {
                usize::try_from(chain.targets)
                    .ok()
                    .filter(|targets| *targets > 1)
                    .map(|targets| (targets, chain.amp))
                    .ok_or_else(|| format!("a chain of {} targets", chain.targets))
            })
            .transpose()?;
        let application = match periodic.and_then(|periodic| periodic.application.as_deref()) {
            None => None,
            Some("magic_hit") if direct.is_none() => Some(Outcome::MagicHit),
            Some(other) => return Err(format!("unknown damage proc application {other}")),
        };
        let periodic = periodic
            .map(|periodic| {
                Ok::<_, String>(Periodic {
                    tick_base: periodic.tick_base,
                    tick: TickTable::of(&periodic.tick_outcome)?,
                })
            })
            .transpose()?;
        Ok(Self {
            direct,
            chain,
            area,
            periodic,
            application,
        })
    }
}

impl<A: Agent> Fight<A> {
    /// Go `CalcDamage` of the direct hit on a target for a base amount.
    fn calc_proc_hit(
        &mut self,
        spell: SpellId,
        target: Side,
        base: f64,
        table: HitTable,
    ) -> SpellResult {
        match table {
            HitTable::Magic { can_crit: true } => self.calc_damage(spell, target, base),
            HitTable::Magic { can_crit: false } => self.calc_damage_hit_only(spell, target, base),
            HitTable::Physical(outcome) => self.calc_physical_damage(spell, target, base, outcome),
        }
    }

    /// shared_utils.go `calcMultiTargetDamage` for an area: a split row rolls once and shares
    /// the roll among the targets it reaches; otherwise each target rolls its own, and an
    /// uncapped area takes the encounter's AoE cap. A capped area goes from the target out, an
    /// uncapped one over every target in unit index order. Nothing is dealt yet.
    fn calc_proc_area(
        &mut self,
        spell: SpellId,
        target: Side,
        direct: Direct,
        area: Area,
    ) -> Vec<SpellResult> {
        let capped = area.max_targets > 0;
        let sides: Vec<Side> = if capped {
            self.cleave_targets(target, area.max_targets)
        } else {
            self.target_sides().collect()
        };
        let share = area.splits.then(|| {
            let reached = sides.len();
            self.effect_roll(direct.average, direct.variance) / reached as f64
        });
        let mut results = Vec::with_capacity(sides.len());
        for side in sides {
            let base = match share {
                Some(share) => share,
                None if capped => self.effect_roll(direct.average, direct.variance),
                None => self.effect_roll(direct.average, direct.variance) * area.aoe_cap_multiplier,
            };
            results.push(self.calc_proc_hit(spell, side, base, direct.table));
        }
        results
    }

    /// The spell's `ApplyEffects`.
    pub(crate) fn apply_proc_damage(&mut self, spell: SpellId, target: Side, params: ProcDamage) {
        let flies = self.spells[spell].missile_speed > 0.0;
        let dot = params
            .periodic
            .and(self.spells[spell].dot)
            .map(|dot| self.dot_on(dot, target));
        let Some(direct) = params.direct else {
            // `dealOnArrival(..., nil, applyDotIfLanded)`: no result, so the dot goes on. A dot
            // that rolls its application, as Ebon Hilt's Corruption does, goes on if it landed
            // and deals the roll's result, which has no damage. An area dot goes on each target
            // the area reaches, from the target out where the row caps them.
            let Some(periodic_dot) = params.periodic.and(self.spells[spell].dot) else {
                return;
            };
            let sides: Vec<Side> = match params.area {
                Some(area) if area.max_targets > 0 => self.cleave_targets(target, area.max_targets),
                Some(_) => self.target_sides().collect(),
                None => vec![target],
            };
            for side in sides {
                let dot = self.dot_on(periodic_dot, side);
                match params.application {
                    Some(outcome) => {
                        let result = self.calc_outcome(spell, side, outcome);
                        if result.landed() {
                            self.apply_dot(dot);
                        }
                        self.deal_damage(spell, result, false);
                    }
                    None => self.apply_dot(dot),
                }
            }
            return;
        };
        if let Some((targets, amp)) = params.chain {
            // `calcMultiTargetDamage` for a chain: a roll for each target, scaled by what the
            // jumps before it kept, every hit calculated before any is dealt.
            let mut keep = 1.0;
            let mut results = Vec::with_capacity(targets);
            for side in self.cleave_targets(target, targets) {
                let base = self.effect_roll(direct.average, direct.variance) * keep;
                keep *= amp;
                results.push(self.calc_proc_hit(spell, side, base, direct.table));
            }
            if flies {
                self.deal_damage_after_travel_batch(spell, &results);
            } else {
                self.deal_batched_aoe_damage(spell, &results, false);
            }
            return;
        }
        if let Some(area) = params.area {
            let results = self.calc_proc_area(spell, target, direct, area);
            if flies {
                self.deal_damage_after_travel_batch(spell, &results);
            } else {
                self.deal_batched_aoe_damage(spell, &results, false);
            }
            return;
        }
        let base = self.effect_roll(direct.average, direct.variance);
        let result = self.calc_proc_hit(spell, target, base, direct.table);
        match (flies, dot) {
            (true, Some(dot)) => self.deal_damage_after_travel_then_dot(spell, result, dot),
            (true, None) => self.deal_damage_after_travel(spell, result),
            (false, dot) => {
                self.deal_damage(spell, result, false);
                if let (Some(dot), true) = (dot, result.landed()) {
                    self.apply_dot(dot);
                }
            }
        }
    }

    /// A tick of the dot: Go `CalcAndDealPeriodicDamage` of the effect's amount on current
    /// stats, on the outcome applier the row names.
    pub(crate) fn proc_damage_tick(&mut self, dot: super::DotId, params: ProcDamage) {
        if let Some(periodic) = params.periodic {
            self.periodic_damage_tick_outcome(dot, periodic.tick_base, periodic.tick.outcome());
        }
    }
}
