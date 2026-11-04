//! MythicSim's healer sims, which Rust runs alone (no Go counterpart): a healer casts every heal
//! on a player who needs it, and each heal counts in full, so its healing per second is what it
//! can put out with its mana. A class binds the [`Effect::Heals`](crate::contracts::prepared_v2::Effect::Heals) its preparation exports and
//! hands each cast and tick here.
//!
//! A direct heal rolls the client row and adds the spell's coefficient on healing power, then
//! the caster and player multipliers and the healing crit roll, as Go `CalcAndDealHealing` does.
//! A party heal lands that roll on each player of the party, a chain heal on each jump at the
//! falloff's share of the one before, each with its own roll and crit. A heal over time
//! snapshots its tick and the caster's multipliers when it goes up, as Go `Dot.SnapshotHeal`
//! does. Under Forever's rules each tick rolls the healing crit where the row states Periodic
//! Can Crit (README, Rust-only mechanics); the Go reference's ticks never crit. Cast again while
//! it runs, it goes on another player: the
//! one running is not cut short, and its ticks left are counted when the new one goes up. A shield counts its absorb, with its coefficient and the
//! caster's multipliers, as shielding when it goes up; nothing in the fight hits the shield.
//!
//! Under Forever's rules two heals follow the party's damage ([`PartyDamage`]): each member is
//! hit at its own rate, at random. Chain Heal lands on the tank, then jumps only to the other
//! members hit within the injured window. Prayer of Mending waits on a member until that member
//! is hit; its class runs the buff with [`HealBook::mending`], [`HealBook::mending_wait`],
//! [`HealBook::mending_heal`] and [`HealBook::next_holder`].

use crate::contracts::prepared_v2::{HealModifiers, HealSpell, PartyDamage, SmartHeal};

use crate::core::time::NS_PER_SECOND;

use super::{
    damage::{OUTCOME_CRIT, OUTCOME_HIT},
    healing::Healing,
    log::action_string,
    Agent, DotId, Fight, Side, SpellId,
};

/// One heal rank, bound to its spell and, for a heal over time, its dot on the player.
#[derive(Clone, Debug)]
pub(crate) struct BoundHeal {
    pub(crate) spell: SpellId,
    direct: Option<(f64, f64)>,
    targets: u32,
    jump_multiplier: f64,
    tick: Option<f64>,
    tick_can_crit: bool,
    pub(crate) dot: Option<DotId>,
    absorb: Option<f64>,
    smart: Option<SmartHeal>,
    past_first_multiplier: f64,
    /// The heal over time rank, by position, that goes up with this heal at no cost.
    then_hot: Option<usize>,
    /// The absorb a crit of the direct heal puts on its player.
    crit_shield: Option<f64>,
    /// The share of a crit a heal turns into an absorb (Divine Aegis), the spell the shielding
    /// counts on, and whether the ticks of a channel shield too.
    crit_absorb: Option<(f64, SpellId, bool)>,
    /// The direct heal's multiplier while any of these heal over time ranks is up, and whether
    /// it is the first player's alone.
    hot_bonus: Option<(Vec<usize>, f64, bool)>,
    /// What each heal over time up takes off this spell's cooldown when it is cast.
    cooldown_per_hot: Option<i64>,
}

/// Every heal rank of the healer and the player's healing modifiers.
#[derive(Clone, Debug)]
pub(crate) struct HealBook {
    pub(crate) heals: Vec<BoundHeal>,
    modifiers: HealModifiers,
    bonus_healing_taken: f64,
    /// The party's damage, under Forever's rules when a heal follows it.
    party: Option<PartyDamage>,
}

impl HealBook {
    /// Binds the exported heals: each rank to the spell of its action id and that spell's dot.
    pub(crate) fn bind<A: Agent>(
        fight: &Fight<A>,
        heals_spec: &[HealSpell],
        modifiers: HealModifiers,
        bonus_healing_taken: f64,
        party: Option<&PartyDamage>,
    ) -> Result<Self, String> {
        if let Some(party) = party {
            if party.hit_rates.len() < 2
                || party
                    .hit_rates
                    .iter()
                    .any(|rate| rate.is_nan() || *rate <= 0.0)
            {
                return Err(format!("party hit rates {:?}", party.hit_rates));
            }
        }
        let heals = heals_spec
            .iter()
            .map(|heal| {
                let spell = fight
                    .spells
                    .iter()
                    .position(|state| state.id.spell_id == heal.spell_id && state.id.tag == 0)
                    .ok_or_else(|| format!("heal {} has no spell", heal.spell_id))?;
                let dot = fight.spells[spell].dot;
                if heal.tick.is_some() && dot.is_none() {
                    return Err(format!("heal over time {} has no dot", heal.spell_id));
                }
                if heal.targets == 0 {
                    return Err(format!("heal {} lands on no player", heal.spell_id));
                }
                let crit_absorb = heal
                    .crit_absorb
                    .map(|absorb| {
                        fight
                            .spells
                            .iter()
                            .position(|state| {
                                state.id.spell_id == absorb.spell_id && state.id.tag == 0
                            })
                            .map(|shield| (absorb.share, shield, absorb.ticks))
                            .ok_or_else(|| {
                                format!(
                                    "heal {} shields on a crit with spell {}, no spell",
                                    heal.spell_id, absorb.spell_id
                                )
                            })
                    })
                    .transpose()?;
                Ok(BoundHeal {
                    spell,
                    direct: heal.direct.map(|roll| (roll.average, roll.variance)),
                    targets: heal.targets,
                    jump_multiplier: heal.jump_multiplier,
                    tick: heal.tick,
                    tick_can_crit: heal.tick_can_crit,
                    dot: heal.tick.and(dot),
                    absorb: heal.absorb,
                    smart: heal.smart,
                    past_first_multiplier: heal.past_first_multiplier,
                    then_hot: None,
                    crit_shield: heal.crit_shield,
                    crit_absorb,
                    hot_bonus: None,
                    cooldown_per_hot: heal.cooldown_per_hot_ns,
                })
            })
            .collect::<Result<Vec<BoundHeal>, String>>()?;
        // A heal's links to other ranks, by spell ID, once every rank is bound.
        let rank = |spell_id: i32| heals_spec.iter().position(|heal| heal.spell_id == spell_id);
        let mut heals = heals;
        for (index, heal) in heals_spec.iter().enumerate() {
            if let Some(id) = heal.then_hot {
                let hot = rank(id)
                    .ok_or_else(|| format!("heal {} goes up with {id}, no heal", heal.spell_id))?;
                if heals[hot].dot.is_none() {
                    return Err(format!(
                        "heal {} goes up with {id}, no heal over time",
                        heal.spell_id
                    ));
                }
                heals[index].then_hot = Some(hot);
            }
            if let Some(bonus) = &heal.hot_bonus {
                let ranks = bonus
                    .spell_ids
                    .iter()
                    .filter_map(|id| rank(*id))
                    .filter(|&hot| heals[hot].dot.is_some())
                    .collect();
                heals[index].hot_bonus = Some((ranks, bonus.multiplier, bonus.first_target_only));
            }
        }
        Ok(HealBook {
            heals,
            modifiers,
            bonus_healing_taken,
            party: party.cloned(),
        })
    }

    /// The player's healing modifiers, with the healing power the debug line shows.
    fn healing<A: Agent>(&self, fight: &Fight<A>, caster: Side) -> Healing {
        Healing {
            dealt_multiplier: self.modifiers.healing_dealt_multiplier,
            taken_multiplier: self.modifiers.healing_taken_multiplier,
            table_multiplier: self.modifiers.table_healing_dealt_multiplier,
            healing_power: fight.unit(caster).powers.healing_power + self.bonus_healing_taken,
        }
    }

    /// The rank of a spell, when the spell is one of the heals.
    pub(crate) fn rank_of(&self, spell: SpellId) -> Option<usize> {
        self.heals.iter().position(|heal| heal.spell == spell)
    }

    /// A cast of rank `rank`: the direct heal on each player it reaches, the heal over time
    /// going up, and the shield.
    pub(crate) fn cast<A: Agent>(&self, fight: &mut Fight<A>, rank: usize) {
        self.cast_scaled(fight, rank, 1.0);
    }

    /// [`Self::cast`] with every part of the heal scaled, for a class bonus on one cast.
    pub(crate) fn cast_scaled<A: Agent>(&self, fight: &mut Fight<A>, rank: usize, scale: f64) {
        self.cast_with_crit(fight, rank, scale, 0.0);
    }

    /// [`Self::cast_scaled`] with `first_crit` more crit chance, as a fraction, on the heal that
    /// lands on the first player: a talent that reads who the heal was cast on.
    pub(crate) fn cast_with_crit<A: Agent>(
        &self,
        fight: &mut Fight<A>,
        rank: usize,
        scale: f64,
        first_crit: f64,
    ) {
        let heal = &self.heals[rank];
        let caster = fight.spells[heal.spell].caster;
        let healing = self.healing(fight, caster);
        if let Some((average, variance)) = heal.direct {
            let mut share = scale;
            // The first player's alone (Riptide on Chain Heal), else every player's.
            let mut first_bonus = 1.0;
            if let Some((hots, multiplier, first_only)) = &heal.hot_bonus {
                if hots.iter().any(|&hot| {
                    let dot = self.heals[hot]
                        .dot
                        .expect("a bonus's heal over time has a dot");
                    fight.aura(fight.dots[dot].aura).active
                }) {
                    if *first_only {
                        first_bonus = *multiplier;
                    } else {
                        share *= multiplier;
                    }
                }
            }
            let targets = match (heal.smart, &self.party) {
                (Some(SmartHeal::Chain), Some(party)) => {
                    1 + self
                        .injured_others(fight, party)
                        .min(heal.targets.saturating_sub(1))
                }
                _ => heal.targets,
            };
            for jump in 0..targets {
                fight.heal_crit_bonus = if jump == 0 { first_crit } else { 0.0 };
                let base = fight.effect_roll(average, variance);
                let past_first = if jump > 0 {
                    heal.past_first_multiplier
                } else {
                    first_bonus
                };
                let result = fight.calc_and_deal_healing_scaled(
                    heal.spell,
                    Side::Player,
                    base,
                    healing,
                    self.bonus_healing_taken,
                    share * past_first,
                );
                fight.heal_crit_bonus = 0.0;
                if let (Some(absorb), true) = (heal.crit_shield, result.outcome == OUTCOME_CRIT) {
                    fight.deal_shield(heal.spell, absorb);
                }
                if result.outcome == OUTCOME_CRIT {
                    Self::shield_crit(fight, heal, result.damage, false);
                }
                share *= heal.jump_multiplier;
            }
        }
        self.apply_hot(fight, rank, scale, healing);
        if let Some(then) = heal.then_hot {
            self.apply_hot(fight, then, 1.0, healing);
        }
        if let Some(absorb) = heal.absorb {
            let amount = self.shield_amount(fight, heal.spell, absorb, healing) * scale;
            fight.deal_shield(heal.spell, amount);
        }
    }

    /// A crit of `heal` that healed `landed` puts its absorb on the player, when the heal has
    /// one (`tick` for the tick of a channel). The share is taken from the heal the spell
    /// computed, before the share real healers overheal comes off, and the absorb is full: it
    /// is shielding, which has no overhealing.
    fn shield_crit<A: Agent>(fight: &mut Fight<A>, heal: &BoundHeal, landed: f64, tick: bool) {
        let Some((share, shield, ticks)) = heal.crit_absorb else {
            return;
        };
        if tick && !ticks {
            return;
        }
        let amount = share * fight.heal_before_overheal(heal.spell, landed);
        fight.deal_shield(shield, amount);
    }

    /// A rank's heal over time going up, `scale` on its tick: one running already goes on another
    /// player who needs it, its ticks left counted now.
    fn apply_hot<A: Agent>(&self, fight: &mut Fight<A>, rank: usize, scale: f64, healing: Healing) {
        let heal = &self.heals[rank];
        if let (Some(tick), Some(dot)) = (heal.tick, heal.dot) {
            // A heal over time cast again while it runs goes on another player who needs it, so
            // the one running keeps healing its own: its ticks left are counted now.
            if fight.aura(fight.dots[dot].aura).active {
                let left = fight.dots[dot].remaining_ticks.max(0);
                let snapshot = fight.dots[dot].snapshot_base;
                for _ in 0..left {
                    for _ in 0..self.tick_players(dot) {
                        fight.deal_heal_tick(heal.spell, snapshot, healing, heal.tick_can_crit);
                    }
                }
            }
            let snapshot = self.snapshot_tick(fight, heal.spell, dot, tick, healing) * scale;
            fight.apply_dot(dot);
            fight.dots[dot].snapshot_base = snapshot;
        }
    }

    /// The other members a chain heal from the tank can jump to: each hit within the injured
    /// window, at its rate.
    fn injured_others<A: Agent>(&self, fight: &mut Fight<A>, party: &PartyDamage) -> u32 {
        let window = party.injured_window as f64 / NS_PER_SECOND as f64;
        let mut injured = 0;
        for &rate in &party.hit_rates[1..] {
            if fight.proc(1.0 - (-rate * window).exp(), "Party Hit") {
                injured += 1;
            }
        }
        injured
    }

    /// A rank that waits on the party's damage (Prayer of Mending) under Forever's rules: its
    /// charges and how long it waits on each member.
    pub(crate) fn mending(&self, rank: usize) -> Option<(u32, i64)> {
        match (self.heals[rank].smart, &self.party) {
            (Some(SmartHeal::Mending { charges, duration }), Some(_)) => Some((charges, duration)),
            _ => None,
        }
    }

    /// How long until `member` is next hit: its hits arrive at random at its rate.
    pub(crate) fn mending_wait<A: Agent>(&self, fight: &mut Fight<A>, member: usize) -> i64 {
        let rate = self
            .party
            .as_ref()
            .expect("a party for a mending heal")
            .hit_rates[member];
        let roll = fight.random("Party Hit");
        (-(1.0 - roll).ln() / rate * NS_PER_SECOND as f64) as i64
    }

    /// Another member of the party than `member`, at random, for the charge to move to.
    pub(crate) fn next_holder<A: Agent>(&self, fight: &mut Fight<A>, member: usize) -> usize {
        let members = self
            .party
            .as_ref()
            .expect("a party for a mending heal")
            .hit_rates
            .len();
        let pick = (fight.random("Party Hit") * (members - 1) as f64) as usize;
        let pick = pick.min(members - 2);
        if pick >= member {
            pick + 1
        } else {
            pick
        }
    }

    /// One charge of a mending heal on the member hit: its roll, crit and overhealing share.
    pub(crate) fn mending_heal<A: Agent>(&self, fight: &mut Fight<A>, rank: usize) {
        let heal = &self.heals[rank];
        let Some((average, variance)) = heal.direct else {
            return;
        };
        let caster = fight.spells[heal.spell].caster;
        let healing = self.healing(fight, caster);
        let base = fight.effect_roll(average, variance);
        let result = fight.calc_and_deal_healing_scaled(
            heal.spell,
            Side::Player,
            base,
            healing,
            self.bonus_healing_taken,
            1.0,
        );
        if result.outcome == OUTCOME_CRIT {
            Self::shield_crit(fight, heal, result.damage, false);
        }
    }

    /// Go `Dot.SnapshotHeal`: the tick plus its coefficient on healing power, times the
    /// caster's healing and periodic healing multipliers and the dot's own periodic multiplier.
    /// A talent's periodic modifier (`SPELLMOD_DOT`) lands on the dot; the spell's own
    /// multipliers, which take the direct modifiers, stay with the direct heal, as the client
    /// keeps the two apart.
    fn snapshot_tick<A: Agent>(
        &self,
        fight: &Fight<A>,
        spell: SpellId,
        dot: DotId,
        tick: f64,
        healing: Healing,
    ) -> f64 {
        let coefficient = fight.dots[dot].bonus_coefficient;
        let mut base = tick;
        if coefficient > 0.0 {
            base = coefficient.mul_add(healing.healing_power, base);
        }
        let caster = if fight.spells[spell].flags.ignore_attacker_modifiers {
            1.0
        } else {
            healing.dealt_multiplier * fight.healing_dealt_factor
        };
        base * caster
            * self.modifiers.periodic_healing_dealt_multiplier
            * fight.dots[dot].periodic_damage_multiplier
    }

    /// A shield's absorb plus the spell's coefficient on healing power, times the caster's
    /// healing multipliers.
    fn shield_amount<A: Agent>(
        &self,
        fight: &Fight<A>,
        spell: SpellId,
        absorb: f64,
        healing: Healing,
    ) -> f64 {
        let coefficient = fight.spells[spell].bonus_coefficient;
        let mut base = absorb;
        if coefficient > 0.0 {
            base = coefficient.mul_add(healing.healing_power, base);
        }
        base * self.caster_multiplier(fight, spell, healing)
    }

    fn caster_multiplier<A: Agent>(
        &self,
        fight: &Fight<A>,
        spell: SpellId,
        healing: Healing,
    ) -> f64 {
        let state = &fight.spells[spell];
        if state.flags.ignore_attacker_modifiers {
            1.0
        } else {
            state.damage_multiplier
                * state.damage_multiplier_additive
                * (healing.dealt_multiplier * fight.healing_dealt_factor)
        }
    }

    /// A tick of a heal over time: the snapshot, times the player's healing taken, on each
    /// player a party heal over time reaches (Wild Growth, Healing Stream Totem).
    pub(crate) fn tick<A: Agent>(&self, fight: &mut Fight<A>, dot: DotId) {
        let spell = fight.dots[dot].spell;
        let caster = fight.spells[spell].caster;
        let healing = self.healing(fight, caster);
        let snapshot = fight.dots[dot].snapshot_base;
        let can_crit = self.tick_can_crit(dot);
        let channel = self
            .heals
            .iter()
            .find(|heal| heal.dot == Some(dot) && heal.direct.is_some());
        for _ in 0..self.tick_players(dot) {
            let (crit, landed) = fight.deal_heal_tick(spell, snapshot, healing, can_crit);
            if let (true, Some(heal)) = (crit, channel) {
                Self::shield_crit(fight, heal, landed, true);
            }
        }
    }

    /// A rank cast with a cooldown cut per heal over time (Idol of Synthesis on Swiftmend):
    /// its cooldown less the cut for each of the healer's heals over time up now.
    pub(crate) fn cut_cooldown<A: Agent>(&self, fight: &mut Fight<A>, rank: usize) {
        let heal = &self.heals[rank];
        let Some(per_hot) = heal.cooldown_per_hot else {
            return;
        };
        let up = self
            .heals
            .iter()
            .filter_map(|other| other.dot)
            .filter(|&dot| fight.aura(fight.dots[dot].aura).active)
            .count() as i64;
        if let (Some((timer, _)), true) = (fight.spells[heal.spell].cd, up > 0) {
            let ready = fight.timers[timer];
            fight.timers[timer] = (ready - per_hot * up).max(fight.now);
        }
    }

    /// The players each tick of a heal over time reaches: a party heal over time's party, or one.
    fn tick_players(&self, dot: DotId) -> u32 {
        self.heals
            .iter()
            .find(|heal| heal.dot == Some(dot) && heal.direct.is_none())
            .map_or(1, |heal| heal.targets)
    }

    /// Whether the ticks of a heal over time roll the healing crit.
    fn tick_can_crit(&self, dot: DotId) -> bool {
        self.heals
            .iter()
            .any(|heal| heal.dot == Some(dot) && heal.tick_can_crit)
    }

    /// A direct heal of `amount`, which already holds the caster's healing multipliers: the
    /// spell's own multipliers, the player's healing taken and the healing crit roll. Swiftmend
    /// heals what the heal over time it consumes would have.
    pub(crate) fn heal_for<A: Agent>(&self, fight: &mut Fight<A>, spell: SpellId, amount: f64) {
        let caster = fight.spells[spell].caster;
        let healing = self.healing(fight, caster);
        let state = &fight.spells[spell];
        let own = state.damage_multiplier * state.damage_multiplier_additive;
        fight.calc_and_deal_healing_with(
            spell,
            Side::Player,
            amount,
            healing,
            self.bonus_healing_taken,
            1.0,
            Some(own),
        );
    }

    /// The first heal over time up on the player among `ranks`, with what the rest of its ticks
    /// would heal, for a spell that consumes one (Swiftmend).
    pub(crate) fn hot_up<A: Agent>(
        &self,
        fight: &Fight<A>,
        ranks: &[usize],
    ) -> Option<(DotId, f64)> {
        ranks.iter().find_map(|&rank| {
            let dot = self.heals[rank].dot?;
            let state = &fight.dots[dot];
            fight
                .aura(state.aura)
                .active
                .then(|| (dot, state.snapshot_base * f64::from(state.base_tick_count)))
        })
    }
}

impl<A: Agent> Fight<A> {
    /// Go `CalcAndDealPeriodicSnapshotHealing` with `OutcomeTick` on the player: the snapshot
    /// times the player's healing taken, the tick counted, the health gained and the log line.
    pub(crate) fn deal_heal_tick(
        &mut self,
        spell: SpellId,
        snapshot: f64,
        healing: Healing,
        can_crit: bool,
    ) -> (bool, f64) {
        let state = &self.spells[spell];
        let mut amount = snapshot;
        if !state.flags.ignore_target_modifiers {
            amount = amount * healing.taken_multiplier * healing.table_multiplier;
        }
        // Forever's ticks roll the healing crit the direct heal does: spell crit and the spell's
        // bonus, less what a dot's tick does not take, at the heal's crit multiplier.
        let crit = can_crit && self.proc(self.heal_tick_crit_chance(spell), "Healing Crit Roll");
        if crit {
            amount *= self.heal_crit_multiplier(spell);
        }
        let target = Side::Player.index();
        let (amount, overhealing) = self.landed_healing(spell, target, amount.max(0.0));
        let result = super::SpellResult {
            armor_multiplier: 0.0,
            target: Side::Player,
            attacker: self.spells[spell].caster,
            outcome: if crit { OUTCOME_CRIT } else { OUTCOME_HIT },
            damage: amount,
            threat: 0.0,
        };
        let threat = self.threat_of(spell, &result);
        let metrics = &mut self.spells[spell].metrics[target];
        if crit {
            metrics.crit_ticks += 1;
            metrics.total_crit_healing += amount;
        } else {
            metrics.ticks += 1;
        }
        metrics.total_healing += amount;
        metrics.total_threat += threat;
        self.note_burst(amount);
        let health = self.self_health_metrics_of(spell);
        self.gain_health(amount, health);
        if self.log.is_some() && !self.spells[spell].flags.no_logs {
            let line = format!(
                "[{}] {} tick {} for {amount:.3} healing{}. (Threat: {threat:.3})",
                self.config.player_label,
                action_string(&self.spells[spell].id),
                if crit { "Crit" } else { "Hit" },
                super::healing::overhealing_note(overhealing),
            );
            self.player_log(&line);
        }
        (crit, amount)
    }

    /// What a heal of `spell` that landed `landed` healing was before the share real healers
    /// overheal was taken off: the heal the spell computed.
    pub(crate) fn heal_before_overheal(&self, spell: SpellId, landed: f64) -> f64 {
        landed / (1.0 - self.spells[spell].overheal)
    }

    /// A heal of exactly `amount` on the player that nothing modifies: no spell power, no healing
    /// multiplier of the caster or the player, no crit roll and no threat. It still goes through
    /// the spell's overhealing share, the burst window, the health gained, the log and the
    /// caster's heal listeners, and counts as a hit of the spell (Bloom).
    pub(crate) fn deal_flat_heal(&mut self, spell: SpellId, amount: f64) {
        let target = Side::Player.index();
        let (amount, overhealing) = self.landed_healing(spell, target, amount.max(0.0));
        let result = super::SpellResult {
            armor_multiplier: 0.0,
            target: Side::Player,
            attacker: self.spells[spell].caster,
            outcome: OUTCOME_HIT,
            damage: amount,
            threat: 0.0,
        };
        let metrics = &mut self.spells[spell].metrics[target];
        metrics.hits += 1;
        metrics.total_healing += amount;
        self.note_burst(amount);
        let health = self.self_health_metrics_of(spell);
        self.gain_health(amount, health);
        if self.log.is_some() && !self.spells[spell].flags.no_logs {
            let line = format!(
                "[{}] {} Hit for {amount:.3} healing{}. (Threat: 0.000)",
                self.config.player_label,
                action_string(&self.spells[spell].id),
                super::healing::overhealing_note(overhealing),
            );
            self.player_log(&line);
        }
        self.on_heal_dealt(spell, &result);
    }

    /// A shield on the player: its absorb counted as the spell's shielding, which healing per
    /// second includes as Go's does.
    pub(crate) fn deal_shield(&mut self, spell: SpellId, amount: f64) {
        let target = Side::Player.index();
        let metrics = &mut self.spells[spell].metrics[target];
        metrics.hits += 1;
        metrics.total_shielding += amount;
        self.note_burst(amount);
        if self.log.is_some() && !self.spells[spell].flags.no_logs {
            let line = format!(
                "[{}] {} Shield for {amount:.3}.",
                self.config.player_label,
                action_string(&self.spells[spell].id),
            );
            self.player_log(&line);
        }
    }

    /// The health metrics a spell's heals on the player go to, made on first use.
    fn self_health_metrics_of(&mut self, spell: SpellId) -> usize {
        match self.spells[spell].self_health_metrics {
            Some(index) => index,
            None => {
                let index = self.new_health_metrics(self.spells[spell].id.clone());
                self.spells[spell].self_health_metrics = Some(index);
                index
            }
        }
    }
}
