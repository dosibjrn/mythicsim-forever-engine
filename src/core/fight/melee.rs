//! Go attack.go and the physical half of spell_outcome.go and spell_resistances.go: each
//! acting unit's main and off hand swings, weapon damage, armor and the physical attack table.
//! The player's and its pet's swings share Go's one simulation weapon attack list.
//!
//! Go runs weapon attacks outside the pending-action queue: before each action, every swing
//! due at or before it fires first. The step loop mirrors that through
//! [`Fight::due_weapon_attack`].

use crate::contracts::prepared_v2::{Weapon, WeaponSkillTable};
use crate::core::queue::Handle;

use super::{
    damage::{
        OUTCOME_BLOCK, OUTCOME_CRIT, OUTCOME_DODGE, OUTCOME_GLANCE, OUTCOME_HIT, OUTCOME_MISS,
        OUTCOME_PARRY, OUTCOME_PARTIAL,
    },
    Action, Agent, Fight, Side, SkillHand, SpellId, SpellResult, PRIORITY_AUTO,
};
use crate::core::time::NEVER_EXPIRES;

/// Go `MaxMeleeRange`.
const MAX_MELEE_RANGE: f64 = 5.0;

/// Go `RangedAutoRetryInterval`: how often a ranged auto that cannot fire while its unit moves
/// looks again.
const RANGED_AUTO_RETRY_INTERVAL: i64 = 500 * crate::core::time::NS_PER_MILLISECOND;
/// Go `RangedAutoStandStill`: how long a unit stands still before its ranged auto can fire (16
/// October 2026 beta notes: "Auto Shot now requires 0.5 seconds of standing still").
const RANGED_AUTO_STAND_STILL: i64 = 500 * crate::core::time::NS_PER_MILLISECOND;

/// Go `heldSwingLag`: a held swing lands this long after the moment it waits for.
const HELD_SWING_LAG: i64 = 1;

/// Go `PhysicalHasteRatingPerHastePercent`.
const PHYSICAL_HASTE_RATING_PER_PERCENT: f64 = 10.0;

/// Which weapon a swing uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hand {
    Main,
    Off,
    /// The ranged slot's auto attack.
    Ranged,
    /// The target's main hand, when it swings at the player.
    Enemy,
}

/// Go `WeaponAttack`.
#[derive(Clone, Debug, Default)]
pub(crate) struct WeaponAttack {
    pub(crate) weapon: Weapon,
    pub(crate) spell: Option<SpellId>,
    pub(crate) swing_at: i64,
    pub(crate) previous_swing: i64,
    pub(crate) natural_ready_at: i64,
    /// Go `extraAttacks`: extra attacks still owed after the one pulled to now.
    pub(crate) extra_attacks: i32,
    /// Go `pendingSwingDelay`: how late the last swing fired against `natural_ready_at`.
    pub(crate) pending_swing_delay: i64,
    cur_swing_speed: f64,
    pub(crate) cur_swing_duration: i64,
    pub(crate) enabled: bool,
    /// Go `castResetAt`: the end of a hard cast in progress ([`Fight::hold_melee_for_cast`]). A
    /// swing landing before it puts the next one a full swing after it.
    cast_reset_at: i64,
}

impl WeaponAttack {
    /// Go `updateSwingDuration`.
    fn update_swing_duration(&mut self, speed: f64) {
        self.cur_swing_speed = speed;
        self.cur_swing_duration = duration_from_seconds(self.weapon.swing_speed / speed);
    }

    /// Go `curSwingSpeed`.
    pub(crate) fn cur_swing_speed(&self) -> f64 {
        self.cur_swing_speed
    }

    /// Go `updateSwingDuration`, for a caller outside the module.
    pub(crate) fn set_swing_speed(&mut self, speed: f64) {
        self.update_swing_duration(speed);
    }
}

/// Go `DurationFromSeconds`.
fn duration_from_seconds(seconds: f64) -> i64 {
    (crate::core::time::NS_PER_SECOND as f64 * seconds) as i64
}

/// Go `AutoAttacks` with the simulation's weapon attack list.
#[derive(Clone, Debug, Default)]
pub(crate) struct AutoAttacks {
    pub(crate) melee: bool,
    /// Go `AutoSwingRanged`.
    pub(crate) ranged_auto: bool,
    pub(crate) dual_wielding: bool,
    pub(crate) mh: WeaponAttack,
    pub(crate) oh: WeaponAttack,
    pub(crate) ranged: WeaponAttack,
    pub(crate) enemy: WeaponAttack,
    /// The main hand timers of the targets past the first, by position among the copies. A
    /// copy swings at a tank as the first target does; otherwise it only keeps the timer its
    /// parry haste reads and moves.
    pub(crate) extra_enemies: Vec<WeaponAttack>,
    /// Go `sim.weaponAttacks`, in the order swings were added, by unit. The player's copy
    /// holds the simulation's list; a pet's stays empty.
    attacks: WeaponAttackList,
    /// Go `sim.minWeaponAttackTime`, likewise kept in the player's copy.
    pub(crate) min_time: i64,
    /// Go `WeaponAttack.replaceSwing` on the main hand for a replacement that returns the swing
    /// itself, as Prowl's does: the rotation acts before each main hand swing.
    pub(crate) react_before_mh_swing: bool,
    /// Go `meleeWeaveWakeup`: the action that wakes a weaver's rotation when its out-of-range
    /// main hand swing comes ready. Go keeps it after it ran, until the next one replaces it.
    weave_wakeup: Option<Handle>,
    /// Whether the unit's melee attack is on: an Auto Shot turns it off, and entering melee
    /// range or a walk toward it turns it on. Only Forever's off hand rule reads it
    /// ([`Fight::start_attack_for_walk_in`]).
    melee_attack_on: bool,
}

impl AutoAttacks {
    pub(crate) fn attack(&mut self, hand: Hand) -> &mut WeaponAttack {
        match hand {
            Hand::Main => &mut self.mh,
            Hand::Off => &mut self.oh,
            Hand::Ranged => &mut self.ranged,
            Hand::Enemy => &mut self.enemy,
        }
    }

    /// The main hand swing of a target: the first target's, or its copy's timer, which opens
    /// with the copy's reset.
    pub(crate) fn enemy_attack(&mut self, target: Side) -> &mut WeaponAttack {
        match target {
            Side::Extra(extra) => {
                let extra = usize::from(extra);
                if self.extra_enemies.len() <= extra {
                    self.extra_enemies
                        .resize_with(extra + 1, WeaponAttack::default);
                }
                &mut self.extra_enemies[extra]
            }
            _ => &mut self.enemy,
        }
    }

    /// Go `AutoAttacks.anyEnabled`.
    fn any_enabled(&self) -> bool {
        self.mh.enabled || self.oh.enabled || self.ranged.enabled
    }
}

/// Go `sim.weaponAttacks`: a slice whose removal swaps the last entry into the gap. Go's range
/// loop over it captures the length and reads the backing array, so a removal during a swing
/// leaves the old last entry visible to the loop; the list keeps the backing array to match.
#[derive(Clone, Debug, Default)]
pub(crate) struct WeaponAttackList {
    backing: Vec<(Side, Hand)>,
    len: usize,
}

impl WeaponAttackList {
    fn clear(&mut self) {
        self.len = 0;
    }

    /// Go `append`, which writes into the backing array while it has room.
    fn push(&mut self, entry: (Side, Hand)) {
        if self.len < self.backing.len() {
            self.backing[self.len] = entry;
        } else {
            self.backing.push(entry);
        }
        self.len += 1;
    }

    /// Go `removeWeaponAttack`.
    fn remove(&mut self, entry: (Side, Hand)) {
        if let Some(index) = self.backing[..self.len].iter().position(|&e| e == entry) {
            self.backing[index] = self.backing[self.len - 1];
            self.len -= 1;
        }
    }

    fn len(&self) -> usize {
        self.len
    }
}

impl std::ops::Index<usize> for WeaponAttackList {
    type Output = (Side, Hand);

    fn index(&self, index: usize) -> &(Side, Hand) {
        &self.backing[index]
    }
}

/// Go `WeaponAttack.IsInRange` for a weapon at the given distance.
fn in_range(weapon: &Weapon, distance: f64) -> bool {
    (weapon.min_range == 0.0 || weapon.min_range < distance)
        && (weapon.max_range == 0.0 || weapon.max_range >= distance)
}

impl<A: Agent> Fight<A> {
    /// The weapon attacks of an acting unit.
    pub(crate) fn autos_of(&self, side: Side) -> &AutoAttacks {
        match side {
            Side::Pet(_) => &self.active_pet(side).autos,
            _ => &self.autos,
        }
    }

    /// [`Self::autos_of`], mutable.
    pub(crate) fn autos_of_mut(&mut self, side: Side) -> &mut AutoAttacks {
        match side {
            Side::Pet(_) => &mut self.active_pet_mut(side).autos,
            _ => &mut self.autos,
        }
    }

    /// Go `TotalMeleeHasteMultiplier`.
    pub(crate) fn melee_haste_multiplier(&self) -> f64 {
        self.melee_haste_multiplier_of(Side::Player)
    }

    /// Go `TotalMeleeHasteMultiplier` of an acting unit.
    pub(crate) fn melee_haste_multiplier_of(&self, side: Side) -> f64 {
        let unit = self.unit(side);
        unit.attack_speed_multiplier
            * unit.melee_speed_multiplier
            * (1.0
                + self.unit_config(side).melee_haste_rating
                    / (PHYSICAL_HASTE_RATING_PER_PERCENT * 100.0))
    }

    /// Go `TotalRealRangedHasteMultiplier` of an acting unit: its attack speed and melee haste
    /// rating with `RangedHasteMultiplier`, which nothing at the pin changes from 1 (the exporter
    /// refuses a character where it differs).
    pub(crate) fn real_ranged_haste_multiplier_of(&self, side: Side) -> f64 {
        self.unit(side).attack_speed_multiplier
            * (1.0
                + self.unit_config(side).melee_haste_rating
                    / (PHYSICAL_HASTE_RATING_PER_PERCENT * 100.0))
    }

    /// Go `Unit.MultiplyMeleeSpeed`.
    #[allow(dead_code)] // Shared with the class domains in progress.
    pub(crate) fn multiply_melee_speed(&mut self, amount: f64) {
        self.player.melee_speed_multiplier *= amount;
        self.update_swing_timers();
    }

    /// Go `Unit.MultiplyAttackSpeed`.
    #[allow(dead_code)] // Shared with the class domains in progress.
    pub(crate) fn multiply_attack_speed(&mut self, amount: f64) {
        self.player.attack_speed_multiplier *= amount;
        self.update_swing_timers();
    }

    /// Go `Unit.MultiplyAttackSpeed` of an acting unit. A pet has no ranged swing.
    pub(crate) fn multiply_attack_speed_of(&mut self, side: Side, amount: f64) {
        if !side.is_pet() {
            return self.multiply_attack_speed(amount);
        }
        self.unit_mut(side).attack_speed_multiplier *= amount;
        let autos = self.autos_of(side);
        if !autos.any_enabled() || !autos.melee || !autos.mh.enabled {
            return;
        }
        assert!(!autos.dual_wielding, "a pet does not dual wield");
        let haste = self.melee_haste_multiplier_of(side);
        let now = self.now;
        let mh = &mut self.autos_of_mut(side).mh;
        let old = mh.cur_swing_speed;
        mh.update_swing_duration(haste);
        let factor = old / mh.cur_swing_speed;
        let remaining = mh.swing_at - now;
        if remaining > 0 {
            mh.swing_at = now + (remaining as f64 * factor) as i64;
        }
        let swing_at = mh.swing_at;
        self.autos.min_time = self.autos.min_time.min(swing_at);
    }

    /// Go `TotalRangedHasteMultiplier`.
    pub(crate) fn ranged_haste_multiplier(&self) -> f64 {
        self.player.attack_speed_multiplier
            * self.player.ranged_speed_multiplier
            * (1.0 + self.config.melee_haste_rating / (PHYSICAL_HASTE_RATING_PER_PERCENT * 100.0))
    }

    /// Go `Unit.MultiplyRangedSpeed`.
    pub(crate) fn multiply_ranged_speed(&mut self, amount: f64) {
        self.player.ranged_speed_multiplier *= amount;
        self.update_swing_timers();
    }

    /// Go `AutoAttacks.UpdateSwingTimers`: the remaining part of each pending melee swing
    /// scales with the change in speed, the off hand by the main hand's factor. A ranged
    /// speed change waits for the next shot.
    pub(crate) fn update_swing_timers(&mut self) {
        if !self.autos.any_enabled() {
            return;
        }
        if self.autos.ranged_auto && self.autos.ranged.enabled {
            let haste = self.ranged_haste_multiplier();
            self.autos.ranged.update_swing_duration(haste);
        }
        if !self.autos.melee || !self.autos.mh.enabled {
            return;
        }
        let haste = self.melee_haste_multiplier();
        let now = self.now;
        let old = self.autos.mh.cur_swing_speed;
        self.autos.mh.update_swing_duration(haste);
        let factor = old / self.autos.mh.cur_swing_speed;
        let remaining = self.autos.mh.swing_at - now;
        if remaining > 0 {
            self.autos.mh.swing_at = now + (remaining as f64 * factor) as i64;
        }
        self.autos.min_time = self.autos.min_time.min(self.autos.mh.swing_at);
        if self.autos.dual_wielding && self.autos.oh.enabled {
            self.autos.oh.update_swing_duration(haste);
            let remaining = self.autos.oh.swing_at - now;
            if remaining > 0 {
                self.autos.oh.swing_at = now + (remaining as f64 * factor) as i64;
            }
            self.autos.min_time = self.autos.min_time.min(self.autos.oh.swing_at);
        }
    }

    /// Go `AutoAttacks.reset` and the simulation's weapon attack reset.
    pub(crate) fn reset_auto_attacks(&mut self) {
        self.autos.attacks.clear();
        self.autos.min_time = NEVER_EXPIRES;
        self.reset_auto_attacks_of(Side::Player);
    }

    /// Go `AutoAttacks.reset` for one acting unit.
    pub(crate) fn reset_auto_attacks_of(&mut self, side: Side) {
        if !self.autos_of(side).melee && !self.autos_of(side).ranged_auto {
            return;
        }
        let haste = self.melee_haste_multiplier_of(side);
        let ranged_haste = self.ranged_haste_multiplier();
        let autos = self.autos_of_mut(side);
        autos.weave_wakeup = None;
        autos.melee_attack_on = false;
        for attack in [&mut autos.mh, &mut autos.oh, &mut autos.ranged] {
            attack.enabled = false;
        }
        for attack in [&mut autos.mh, &mut autos.oh] {
            attack.previous_swing = -NEVER_EXPIRES;
            attack.swing_at = NEVER_EXPIRES;
        }
        autos.mh.extra_attacks = 0;
        autos.mh.cast_reset_at = 0;
        autos.oh.cast_reset_at = 0;
        if autos.melee {
            autos.mh.update_swing_duration(haste);
            autos.mh.previous_swing = -autos.mh.cur_swing_duration;
            autos.mh.swing_at = 0;
            autos.mh.natural_ready_at = 0;
            if autos.dual_wielding {
                autos.oh.update_swing_duration(haste);
                autos.oh.previous_swing = -autos.oh.cur_swing_duration;
                autos.oh.swing_at = duration_from_seconds(autos.oh.weapon.swing_speed / 2.0);
            }
        }
        autos.ranged.previous_swing = -NEVER_EXPIRES;
        autos.ranged.swing_at = NEVER_EXPIRES;
        if autos.ranged_auto {
            autos.ranged.update_swing_duration(ranged_haste);
            autos.ranged.previous_swing = -autos.ranged.cur_swing_duration;
            autos.ranged.swing_at = 0;
            autos.ranged.natural_ready_at = 0;
        }
    }

    /// Go `AutoAttacks.StopRangedUntil`: the ranged timer restarts as if a shot fired at
    /// `ready`.
    pub(crate) fn stop_ranged_until(&mut self, ready: i64) {
        if !self.autos.ranged_auto {
            return;
        }
        self.autos.ranged.swing_at = ready + self.autos.ranged.cur_swing_duration;
        self.autos.min_time = self.autos.min_time.min(self.autos.ranged.swing_at);
    }

    /// Go `AutoAttacks.DelayRangedUntil`.
    pub(crate) fn delay_ranged_until(&mut self, ready: i64) {
        if ready <= self.autos.ranged.swing_at {
            return;
        }
        self.autos.ranged.swing_at = ready;
        self.autos.min_time = self.autos.min_time.min(ready);
    }

    /// When the rotation's `autoTimeToNext` reads the next auto attack of a kind: Go
    /// `NextAttackAt`, `MainhandSwingAt`, `OffhandSwingAt`, `NextRangedAttackAt` and
    /// `NextAnyAttackAt`.
    /// Go `APLValueAutoTimeSinceLast`: the time since the hands it reads last swung, each a
    /// wrapping difference as Go's durations are.
    pub(crate) fn time_since_last_auto(&self, kind: crate::rotation::AutoAttackType) -> i64 {
        use crate::rotation::AutoAttackType;
        let autos = &self.autos;
        let since = |attack: &WeaponAttack| self.now.wrapping_sub(attack.previous_swing);
        match kind {
            AutoAttackType::MainHand => since(&autos.mh),
            AutoAttackType::OffHand => since(&autos.oh),
            AutoAttackType::Ranged => since(&autos.ranged),
            AutoAttackType::Melee | AutoAttackType::Any => {
                let mut value = since(&autos.mh);
                if autos.oh.enabled {
                    value = value.min(since(&autos.oh));
                }
                if kind == AutoAttackType::Any && autos.ranged.enabled {
                    value = value.min(since(&autos.ranged));
                }
                value
            }
        }
    }

    pub(crate) fn next_auto_attack_at(&self, kind: crate::rotation::AutoAttackType) -> i64 {
        use crate::rotation::AutoAttackType;
        let autos = &self.autos;
        match kind {
            AutoAttackType::Melee => autos.mh.swing_at.min(autos.oh.swing_at),
            AutoAttackType::MainHand => autos.mh.swing_at,
            AutoAttackType::OffHand => autos.oh.swing_at,
            AutoAttackType::Ranged => autos.ranged.swing_at,
            AutoAttackType::Any => autos
                .mh
                .swing_at
                .min(autos.oh.swing_at)
                .min(autos.ranged.swing_at),
        }
    }

    /// Go `RandomizeMeleeTiming` at encounter start: delay the first swings by a random whole
    /// number of milliseconds below the reaction time, unless out of melee range.
    pub(crate) fn randomize_melee_timing(&mut self, side: Side) {
        let config = self.unit_config(side);
        if !self.autos_of(side).melee || config.distance > MAX_MELEE_RANGE {
            return;
        }
        let reaction_ms = config.reaction / crate::core::time::NS_PER_MILLISECOND;
        let roll = self.random("Melee Timing");
        let delay = (roll * reaction_ms as f64) as i64 * crate::core::time::NS_PER_MILLISECOND;
        // Go DelayMeleeBy.
        if delay <= 0 {
            return;
        }
        let autos = self.autos_of_mut(side);
        autos.mh.swing_at += delay;
        let mut earliest = autos.mh.swing_at;
        if autos.dual_wielding {
            autos.oh.swing_at += delay;
            earliest = earliest.min(autos.oh.swing_at);
        }
        self.autos.min_time = self.autos.min_time.min(earliest);
    }

    /// Forever's rule for using an item while swinging (Oil of Immolation): each of the player's
    /// melee hands that is swinging, with its swing not yet due, comes `delay` later. A hand
    /// that is not swinging (out of range, or before the pull) has no timer running and loses
    /// nothing, and the ranged auto is left alone.
    pub(crate) fn delay_running_melee_swings(&mut self, delay: i64) {
        if delay <= 0 || !self.autos.melee {
            return;
        }
        let now = self.now;
        let hands: &[Hand] = if self.autos.dual_wielding {
            &[Hand::Main, Hand::Off]
        } else {
            &[Hand::Main]
        };
        // The list's next time stays at the older, earlier one: the swing it wakes for finds
        // itself not due and reports its new time, as with a swing the weapon sync delays.
        for &hand in hands {
            let attack = self.autos.attack(hand);
            if attack.enabled && attack.swing_at > now {
                attack.swing_at += delay;
            }
        }
    }

    /// Go `AutoAttacks.startPull` for the targets, whose swings are added in unit index order
    /// before the player's.
    pub(crate) fn start_enemy_attacks(&mut self) {
        for position in 0..self.enemies.len() {
            let target = Side::target(position);
            let haste = self.enemy_melee_haste(target);
            let attack = self.autos.enemy_attack(target);
            attack.enabled = true;
            attack.update_swing_duration(haste);
            let swing_at = attack.swing_at;
            self.autos.attacks.push((target, Hand::Enemy));
            self.autos.min_time = self.autos.min_time.min(swing_at);
        }
    }

    /// Go `AutoAttacks.reset` for a target: its main hand opens at a random point of its swing
    /// timer, from the roll Go draws at the target's reset. A target the player does not tank
    /// only keeps the timer, which its parry haste reads.
    pub(crate) fn reset_enemy_attack(&mut self, target: Side, roll: f64) {
        let position = target.target_position().expect("the unit is a target");
        let Some(enemy) = self.enemies.get_mut(position) else {
            if let Some((speed, haste)) = self.untanked_swing {
                Self::open_enemy_swing(self.autos.enemy_attack(target), speed, haste, roll);
            }
            return;
        };
        // Go Unit.reset restores the target's pseudo stats first.
        enemy.melee_speed_multiplier = enemy.values.melee_speed_multiplier.unwrap_or(1.0);
        enemy.attack_speed_multiplier = enemy.values.attack_speed_multiplier.unwrap_or(1.0);
        let speed = enemy.values.swing_speed;
        let haste = self.enemy_melee_haste(target);
        Self::open_enemy_swing(self.autos.enemy_attack(target), speed, haste, roll);
    }

    /// A target's main hand at its reset: disabled, opening at the roll's point of its swing.
    fn open_enemy_swing(attack: &mut WeaponAttack, speed: f64, haste: f64, roll: f64) {
        attack.weapon.swing_speed = speed;
        attack.enabled = false;
        attack.update_swing_duration(haste);
        attack.previous_swing = -attack.cur_swing_duration;
        attack.swing_at = 0;
        attack.natural_ready_at = 0;
        let offset = (roll * attack.cur_swing_duration as f64) as i64;
        attack.previous_swing += offset;
        attack.swing_at += offset;
        attack.natural_ready_at += offset;
    }

    /// Go `AutoAttacks.SetMH`: the new weapon, timed at the current swing speed.
    pub(crate) fn set_main_hand(&mut self, weapon: Weapon) {
        self.autos.mh.weapon = weapon;
        let speed = self.autos.mh.cur_swing_speed;
        self.autos.mh.update_swing_duration(speed);
    }

    /// Go `AutoAttacks.EnableAutoSwing` for a main hand: nothing in the prepull; otherwise the
    /// next swing no earlier than now, and a disabled main hand in range starts swinging. The
    /// supported callers do not dual wield.
    pub(crate) fn enable_auto_swing(&mut self) {
        if !self.autos.melee || self.in_prepull {
            return;
        }
        assert!(
            !self.autos.dual_wielding,
            "dual wield swings are not re-enabled"
        );
        let now = self.now;
        self.autos.mh.swing_at = self.autos.mh.swing_at.max(now).max(0);
        if in_range(&self.autos.mh.weapon, self.config.distance) && !self.autos.mh.enabled {
            self.autos.mh.enabled = true;
            if self.autos.mh.weapon.swing_speed <= 0.0 {
                self.autos.mh.enabled = false;
                return;
            }
            let haste = self.melee_haste_multiplier();
            self.autos.mh.update_swing_duration(haste);
            self.autos.attacks.push((Side::Player, Hand::Main));
            self.autos.min_time = self.autos.min_time.min(self.autos.mh.swing_at);
        }
    }

    /// Go `AutoAttacks.startPull`: the off hand is added first.
    pub(crate) fn start_auto_attacks(&mut self, side: Side) {
        let autos = self.autos_of(side);
        if (!autos.melee && !autos.ranged_auto) || autos.any_enabled() {
            return;
        }
        if self.autos_of(side).melee {
            self.start_melee_attacks(side);
        }
        if self.autos_of(side).ranged_auto {
            let distance = self.config.distance;
            let haste = self.ranged_haste_multiplier();
            let ranged = &mut self.autos.ranged;
            if ranged.swing_at == NEVER_EXPIRES {
                ranged.swing_at = 0;
                ranged.natural_ready_at = 0;
            }
            if in_range(&ranged.weapon, distance) {
                // Go addWeaponAttack: an empty slot never swings.
                if ranged.weapon.swing_speed <= 0.0 {
                    return;
                }
                ranged.enabled = true;
                ranged.update_swing_duration(haste);
                let swing_at = ranged.swing_at;
                self.autos.attacks.push((side, Hand::Ranged));
                self.autos.min_time = self.autos.min_time.min(swing_at);
            }
        }
    }

    /// The melee half of Go `AutoAttacks.startPull`.
    fn start_melee_attacks(&mut self, side: Side) {
        let haste = self.melee_haste_multiplier_of(side);
        let distance = self.unit_config(side).distance;
        let in_range = |weapon: &Weapon| in_range(weapon, distance);
        let autos = self.autos_of_mut(side);
        if autos.mh.swing_at == NEVER_EXPIRES {
            autos.mh.swing_at = 0;
            autos.mh.natural_ready_at = 0;
        }
        let hands: &[Hand] = if autos.dual_wielding {
            &[Hand::Off, Hand::Main]
        } else {
            &[Hand::Main]
        };
        for &hand in hands {
            let attack = self.autos_of_mut(side).attack(hand);
            if !in_range(&attack.weapon) {
                continue;
            }
            attack.enabled = true;
            // Go addWeaponAttack: an empty slot never swings.
            if attack.weapon.swing_speed <= 0.0 {
                attack.enabled = false;
                continue;
            }
            attack.update_swing_duration(haste);
            let swing_at = attack.swing_at;
            self.autos.attacks.push((side, hand));
            self.autos.min_time = self.autos.min_time.min(swing_at);
        }
        // A unit that starts in melee range starts its attack at the pull.
        let autos = self.autos_of_mut(side);
        autos.melee_attack_on = autos.mh.enabled;
    }

    /// Go `Simulation.Step`'s weapon check: whether swings are due before the next action at
    /// `next`.
    pub(crate) fn due_weapon_attack(&self, next: i64) -> bool {
        next >= self.autos.min_time
    }

    /// Go `advanceWeaponAttacks`.
    pub(crate) fn advance_weapon_attacks(&mut self) {
        if self.autos.min_time > self.now {
            let time = self.autos.min_time;
            self.advance_to(time);
        }
        self.autos.min_time = NEVER_EXPIRES;
        for position in 0..self.autos.attacks.len() {
            let (side, hand) = self.autos.attacks[position];
            let next = self.try_swing(side, hand);
            self.autos.min_time = self.autos.min_time.min(next);
        }
    }

    /// Go `WeaponAttack.trySwing` and `swing`. The next swing time is read after the swing,
    /// which a melee speed change during it can move.
    fn try_swing(&mut self, side: Side, hand: Hand) -> i64 {
        let now = self.now;
        if hand == Hand::Enemy && now < self.autos.enemy_attack(side).swing_at {
            return self.autos.enemy_attack(side).swing_at;
        }
        if hand != Hand::Enemy && now < self.autos_of_mut(side).attack(hand).swing_at {
            return self.autos_of_mut(side).attack(hand).swing_at;
        }
        // Fork patch 191: a moving unit's position is brought up to date only when its rotation
        // runs, and a rotation that casts on the GCD as it walks away is not checked again until
        // the GCD ends. A melee swing that comes due during the walk reads where the unit is now:
        // past melee range the update stops both hands and the swing does not happen.
        if matches!(hand, Hand::Main | Hand::Off)
            && self.autos_of_mut(side).attack(hand).enabled
            && self.unit(side).moving
        {
            self.update_position(side, false);
            if !self.autos_of_mut(side).attack(hand).enabled {
                return NEVER_EXPIRES;
            }
        }
        // A ranged auto cannot fire while moving. The hidden retry timer looks again every
        // 500ms. Casts never hold it.
        if hand == Hand::Ranged && self.unit(side).moving {
            let attack = self.autos_of_mut(side).attack(hand);
            attack.swing_at = now + RANGED_AUTO_RETRY_INTERVAL;
            return attack.swing_at;
        }
        // The 16 October 2026 beta notes: a shot that comes due (or is retried) less than 0.5 s
        // after the last move ended waits until it has passed. A shot with no move before it
        // keeps its timing.
        if hand == Hand::Ranged {
            let ready_at = self.unit(side).movement_ended_at + RANGED_AUTO_STAND_STILL;
            if now < ready_at {
                let attack = self.autos_of_mut(side).attack(hand);
                attack.swing_at = ready_at;
                return attack.swing_at;
            }
        }
        if hand == Hand::Enemy {
            let attack = self.autos.enemy_attack(side);
            attack.previous_swing = attack.swing_at;
            attack.swing_at = now + attack.cur_swing_duration;
            attack.natural_ready_at = attack.swing_at;
            // The target's own reaction runs no rotation in scope.
            self.enemy_swing(side);
            return self.autos.enemy_attack(side).swing_at;
        }
        let mut spell = self
            .autos_of_mut(side)
            .attack(hand)
            .spell
            .expect("an enabled weapon attack has a spell");
        // Go: with a replacer set, the rotation runs first, then the class may replace the
        // main hand swing, as Heroic Strike does.
        // Prowl's replacement returns the swing itself, so only the rotation runs.
        if side == Side::Player
            && hand == Hand::Main
            && (self.config.melee.replace_main_hand_swing || self.autos.react_before_mh_swing)
        {
            self.react_to_event_now();
            spell = A::replace_mh_swing(self, spell);
        }
        let attack = self.autos_of_mut(side).attack(hand);
        attack.previous_swing = attack.swing_at;
        attack.swing_at = now + attack.cur_swing_duration;
        // A hard cast that started as this swing landed restarts the timer when it completes.
        if attack.cast_reset_at > now {
            attack.swing_at = attack
                .swing_at
                .max(attack.cast_reset_at + attack.cur_swing_duration);
        }
        if attack.extra_attacks > 0 {
            attack.extra_attacks -= 1;
            attack.swing_at = now;
        }
        attack.pending_swing_delay = (now - attack.natural_ready_at).max(0);
        attack.natural_ready_at = attack.swing_at;
        // A melee swing resets the ranged auto timer, as if the shot had just fired. Forever's
        // rule leaves the timer alone (README, Rust-only mechanics).
        if side == Side::Player
            && hand != Hand::Ranged
            && self.autos.ranged_auto
            && !self.melee_keeps_auto_shot_timer
        {
            self.stop_ranged_until(now);
        }
        // An Auto Shot puts the bow out: the melee attack is off until it starts again.
        if hand == Hand::Ranged {
            self.autos_of_mut(side).melee_attack_on = false;
        }
        // Go swings at the unit's current target; a pet's stays the first.
        let target = if side == Side::Player {
            self.current_target_side()
        } else {
            Side::Target
        };
        self.cast(spell, target);
        // Go ReactToEvent(false, true) after the swing, unless the player is tanking.
        if side != Side::Player || !self.tanked() {
            self.react_to_event(side);
        }
        self.autos_of_mut(side).attack(hand).swing_at
    }

    /// Go `AutoAttacks.CancelMeleeSwing` of an acting unit: the swings leave the simulation's
    /// list. The next weapon attack time stays, as Go only lowers it.
    pub(crate) fn cancel_melee_swing(&mut self, side: Side) {
        let autos = self.autos_of_mut(side);
        if !autos.melee {
            return;
        }
        let mut removed = Vec::new();
        if autos.mh.enabled {
            autos.mh.enabled = false;
            removed.push((side, Hand::Main));
        }
        if autos.dual_wielding && autos.oh.enabled {
            autos.oh.enabled = false;
            removed.push((side, Hand::Off));
        }
        for entry in removed {
            self.autos.attacks.remove(entry);
        }
    }

    /// Go `AutoAttacks.CancelRangedSwing` of an acting unit.
    pub(crate) fn cancel_ranged_swing(&mut self, side: Side) {
        let autos = self.autos_of_mut(side);
        if !autos.ranged_auto || !autos.ranged.enabled {
            return;
        }
        autos.ranged.enabled = false;
        self.autos.attacks.remove((side, Hand::Ranged));
    }

    /// Go `AutoAttacks.EnableRangedSwing` of an acting unit: the shot resumes no earlier than
    /// now, and rejoins the simulation's list when in range.
    pub(crate) fn enable_ranged_swing(&mut self, side: Side) {
        let held_back = if side == Side::Player {
            self.in_prepull
        } else {
            self.now < 0
        };
        let autos = self.autos_of(side);
        if !autos.ranged_auto || autos.ranged.enabled || held_back {
            return;
        }
        let now = self.now;
        let in_range = self.ranged_in_range(side);
        let haste = self.ranged_haste_multiplier();
        let ranged = &mut self.autos_of_mut(side).ranged;
        ranged.swing_at = ranged.swing_at.max(now).max(0);
        ranged.natural_ready_at = ranged.swing_at;
        if in_range {
            ranged.enabled = true;
            self.add_weapon_attack(side, Hand::Ranged, haste);
        }
    }

    /// Go `AutoAttacks.scheduleMeleeWeaveWakeup`: wake the rotation when the out-of-range main
    /// hand swing comes ready, so a weaver parked behind its GCD can move back into melee.
    pub(crate) fn schedule_melee_weave_wakeup(&mut self, side: Side) {
        let autos = self.autos_of(side);
        if !autos.melee || !autos.ranged_auto {
            return;
        }
        self.cancel_melee_weave_wakeup(side);
        let at = self.autos_of(side).mh.swing_at.max(self.now);
        let handle = self.schedule(at, PRIORITY_AUTO, Action::MeleeWeaveWakeup(side));
        self.autos_of_mut(side).weave_wakeup = Some(handle);
    }

    /// Go `AutoAttacks.cancelMeleeWeaveWakeup`: a wakeup that already ran stays recorded.
    pub(crate) fn cancel_melee_weave_wakeup(&mut self, side: Side) {
        if let Some(handle) = self.autos_of_mut(side).weave_wakeup.take() {
            self.queue.cancel(handle);
        }
    }

    /// Go `AutoAttacks.weaveWakeupAfterCast`: a hardcast shorter than the GCD leaves a weaver
    /// free to move before the GCD frees, so the rotation wakes to land a pending swing.
    pub(crate) fn weave_wakeup_after_cast(&mut self, side: Side) {
        let autos = self.autos_of(side);
        if autos.weave_wakeup.is_some() && autos.mh.swing_at <= self.now {
            self.react_to_event(side);
        }
    }

    /// Go `AutoAttacks.holdArrivalSwingForRotation`: the rotation acts before the swing that
    /// lands the moment a weaver steps into melee range, so an on-next-swing ability pressed
    /// on arrival replaces that first swing instead of waiting behind a white hit. Only the
    /// player's rotation can be held here, and not while a Wait or WaitUntil holds it.
    pub(crate) fn hold_arrival_swing_for_rotation(&mut self, side: Side) {
        let autos = self.autos_of(side);
        let replaces = side == Side::Player
            && (self.config.melee.replace_main_hand_swing || autos.react_before_mh_swing);
        if !autos.ranged_auto || !replaces || !autos.mh.enabled || autos.mh.swing_at > self.now {
            return;
        }
        if self.rotation_blocked() {
            return;
        }
        let swing_at = self.now + HELD_SWING_LAG;
        self.autos.mh.swing_at = swing_at;
        self.autos.min_time = self.autos.min_time.min(swing_at);
        self.react_to_event_now();
    }

    /// Go `AutoAttacks.EnableMeleeSwing` of an acting unit: each hand resumes no earlier than
    /// now, and a stopped hand in range rejoins the simulation's list.
    pub(crate) fn enable_melee_swing(&mut self, side: Side) {
        // Go reads `isInPrepull`, which holds only while the simulation resets and schedules the
        // prepull; the player's own moves run after it. A pet's enabling at a negative time is
        // held back, as it always has been.
        let held_back = if side == Side::Player {
            self.in_prepull
        } else {
            self.now < 0
        };
        if !self.autos_of(side).melee || held_back {
            return;
        }
        let now = self.now;
        let distance = self.unit_config(side).distance;
        let haste = self.melee_haste_multiplier_of(side);
        let from_attack_start = self.off_hand_waits_from_attack_start;
        let autos = self.autos_of_mut(side);
        autos.mh.swing_at = autos.mh.swing_at.max(now).max(0);
        if in_range(&autos.mh.weapon, distance) && !autos.mh.enabled {
            autos.mh.enabled = true;
            self.add_weapon_attack(side, Hand::Main, haste);
        }
        let autos = self.autos_of_mut(side);
        // Forever's rule: an attack still on, started by the walk in or left on since the last
        // time in melee, adds no new wait.
        let attack_was_on = from_attack_start && autos.melee_attack_on;
        autos.melee_attack_on = true;
        if autos.dual_wielding && !autos.oh.enabled {
            // A unit that also shoots enters melee with the off hand half a swing behind.
            let mut offset = 0;
            if autos.ranged_auto && !attack_was_on {
                autos.oh.update_swing_duration(haste);
                offset = autos.oh.cur_swing_duration / 2;
            }
            autos.oh.swing_at = autos.oh.swing_at.max(now + offset).max(0);
            if in_range(&autos.oh.weapon, distance) {
                autos.oh.enabled = true;
                self.add_weapon_attack(side, Hand::Off, haste);
            }
        }
    }

    /// Forever's rule: a walk from outside the main hand's range to a destination inside it
    /// starts the melee attack as it begins, as a weave's `/startattack` does, so the off hand's
    /// half swing wait runs during the walk. An attack already on is left alone.
    pub(crate) fn start_attack_for_walk_in(&mut self, side: Side, destination: f64) {
        let autos = self.autos_of(side);
        if !self.off_hand_waits_from_attack_start
            || !autos.melee
            || !autos.ranged_auto
            || autos.melee_attack_on
            || self.main_hand_in_range(side)
            || !in_range(&autos.mh.weapon, destination)
        {
            return;
        }
        let now = self.now;
        let haste = self.melee_haste_multiplier_of(side);
        let autos = self.autos_of_mut(side);
        autos.melee_attack_on = true;
        if autos.dual_wielding {
            autos.oh.update_swing_duration(haste);
            let wait = autos.oh.cur_swing_duration / 2;
            autos.oh.swing_at = autos.oh.swing_at.max(now + wait).max(0);
        }
    }

    /// Go `WeaponAttack.addWeaponAttack` for an enabled attack: an empty slot never swings;
    /// otherwise the swing joins the simulation's list at its current time.
    pub(crate) fn add_weapon_attack(&mut self, side: Side, hand: Hand, haste: f64) {
        let attack = self.autos_of_mut(side).attack(hand);
        if !attack.enabled {
            return;
        }
        if attack.weapon.swing_speed <= 0.0 {
            attack.enabled = false;
            return;
        }
        attack.update_swing_duration(haste);
        let swing_at = attack.swing_at;
        self.autos.attacks.push((side, hand));
        self.autos.min_time = self.autos.min_time.min(swing_at);
    }

    /// Note, in the logged fight, a queued strike that went out on an extra attack in its
    /// place. Go logs only the strike's cast, so the application could not tell it from a
    /// strike that took a regular swing.
    pub(crate) fn note_extra_attack_strike(&mut self, extra: SpellId, strike: SpellId) {
        if self.log.is_none() || strike == extra {
            return;
        }
        let entry = serde_json::json!({
            "time": crate::core::time::seconds(self.now),
            "strike": super::log::action_string(&self.spells[strike].id),
            "extraAttack": super::log::action_string(&self.spells[extra].id),
        });
        self.extra_attack_strikes.push(entry);
    }

    /// Go `AutoAttacks.ExtraMHAttacks`: the main hand swings now, then again for each extra
    /// attack still owed.
    pub(crate) fn extra_mh_attacks(&mut self, count: i32) {
        if count <= 0 || !self.autos.melee || !self.autos.mh.enabled {
            return;
        }
        self.autos.mh.extra_attacks += count - 1;
        // Go ExtraMHAttack.
        self.autos.mh.swing_at = self.now;
        self.autos.min_time = self.autos.min_time.min(self.now);
    }

    /// Go `AutoAttacks.InstantMHAttack`: a main hand swing right now that leaves the swing timer
    /// alone (Forever Reckoning).
    pub(crate) fn instant_mh_attack(&mut self) {
        if !self.autos.melee
            || !self.autos.mh.enabled
            || !in_range(&self.autos.mh.weapon, self.config.distance)
        {
            return;
        }
        let spell = self
            .autos
            .mh
            .spell
            .expect("an enabled weapon attack has a spell");
        self.cast(spell, self.current_target_side());
    }

    /// Go `Unit.ReactToEvent(sim, false, false)`: the rotation runs, then evaluates again now,
    /// unless a Wait or WaitUntil holds it.
    pub(crate) fn react_to_event_now(&mut self) {
        self.do_next_action();
        if self.rotation_blocked() {
            return;
        }
        if self.player.rotation_timer > self.now {
            self.set_rotation_timer(self.now);
        }
    }

    /// Go `AutoAttacks.HoldMeleeForCast`: the next swing comes one full weapon speed after the
    /// cast completes, whether or not a swing came due during it. A swing landing now, as the
    /// cast starts, still lands, and `cast_reset_at` pushes the one after it past the cast.
    pub(crate) fn hold_melee_for_cast(&mut self, cast_end: i64) {
        if !self.autos.melee {
            return;
        }
        let now = self.now;
        let hands: &[Hand] = if self.autos.dual_wielding {
            &[Hand::Main, Hand::Off]
        } else {
            &[Hand::Main]
        };
        for &hand in hands {
            let attack = self.autos.attack(hand);
            attack.cast_reset_at = attack.cast_reset_at.max(cast_end);
            if attack.swing_at <= now + HELD_SWING_LAG {
                continue;
            }
            attack.swing_at = cast_end + attack.cur_swing_duration;
            // Go rescheduleWeaponAttack.
            let swing_at = attack.swing_at;
            self.autos.min_time = self.autos.min_time.min(swing_at);
        }
    }

    /// Go `Unit.ReactToEvent(sim, false, true)`.
    pub(crate) fn react_to_event(&mut self, side: Side) {
        self.do_next_action_of(side);
        // Go: a Wait or WaitUntil holding the rotation keeps the time it wakes at.
        if side == Side::Player && self.rotation_blocked() {
            return;
        }
        let evaluation = self.now + self.unit_config(side).reaction;
        if self.unit(side).rotation_timer > evaluation {
            self.set_rotation_timer_of(side, evaluation);
        }
    }

    /// The auto attack spell's `ApplyEffects`: weapon damage on the white hit table.
    pub(crate) fn apply_melee_auto(&mut self, spell: SpellId, target: Side, hand: Hand) {
        if hand == Hand::Ranged {
            return self.apply_ranged_auto(spell, target);
        }
        A::before_melee_auto(self, spell, hand);
        let caster = self.caster(spell);
        let attack_power = self.melee_attack_power_of(caster);
        let weapon = self.autos_of_mut(caster).attack(hand).weapon.clone();
        let mut base = self.weapon_damage(&weapon, attack_power);
        if hand == Hand::Off {
            base *= 0.5;
        }
        let result = self.calc_physical_damage(spell, target, base, PhysicalOutcome::MeleeWhite);
        self.deal_damage(spell, result, false);
    }

    /// The ranged auto attack spell's `ApplyEffects`: the late-shot line, weapon damage on the
    /// ranged table dealt after travel, and a reaction for movement actions.
    fn apply_ranged_auto(&mut self, spell: SpellId, target: Side) {
        let delay = self.autos.ranged.pending_swing_delay;
        let ready_at = self.now - delay;
        if self.log.is_some() && delay > crate::core::time::NS_PER_MILLISECOND && ready_at > 0 {
            let line = format!(
                "{} delayed by {}, was ready at {}",
                super::log::action_string(&self.spells[spell].id),
                crate::core::time::go_string(delay),
                crate::core::time::go_string(ready_at)
            );
            self.player_log(&line);
        }
        let attack_power = self.ranged_attack_power();
        let weapon = self.autos.ranged.weapon.clone();
        let base = self.weapon_damage(&weapon, attack_power);
        let result = self.calc_physical_damage(
            spell,
            target,
            base,
            PhysicalOutcome::RangedHitAndCrit { count: true },
        );
        self.deal_damage_after_travel(spell, result);
        self.react_to_event(Side::Player);
    }

    /// Go `Spell.RangedAttackPower`, with the player's mob type bonus.
    pub(crate) fn ranged_attack_power(&self) -> f64 {
        self.player.powers.ranged_attack_power
            + self.config.defender_bonus_ranged_attack_power
            + self.mob_type_attack_power()
    }

    /// Go `Weapon.CalculateNormalizedWeaponDamage` for the ranged weapon.
    pub(crate) fn ranged_normalized_weapon_damage(&mut self, attack_power: f64) -> f64 {
        let weapon = self.autos.ranged.weapon.clone();
        self.normalized_weapon_damage(&weapon, attack_power)
    }

    /// Go `Unit.MHWeaponDamage`.
    #[allow(dead_code)] // Shared with the class domains in progress.
    pub(crate) fn mh_weapon_damage(&mut self, attack_power: f64) -> f64 {
        let weapon = self.autos.mh.weapon.clone();
        self.weapon_damage(&weapon, attack_power)
    }

    /// Go `Unit.OHWeaponDamage`: half the off hand's roll.
    #[allow(dead_code)] // Shared with the class domains in progress.
    pub(crate) fn oh_weapon_damage(&mut self, attack_power: f64) -> f64 {
        let weapon = self.autos.oh.weapon.clone();
        0.5 * self.weapon_damage(&weapon, attack_power)
    }

    /// Go `Unit.MHNormalizedWeaponDamage`.
    #[allow(dead_code)] // Shared with the class domains in progress.
    pub(crate) fn mh_normalized_weapon_damage(&mut self, attack_power: f64) -> f64 {
        let weapon = self.autos.mh.weapon.clone();
        self.normalized_weapon_damage(&weapon, attack_power)
    }

    /// Go `Unit.OHNormalizedWeaponDamage`: half the off hand's normalized roll.
    #[allow(dead_code)] // Shared with the class domains in progress.
    pub(crate) fn oh_normalized_weapon_damage(&mut self, attack_power: f64) -> f64 {
        let weapon = self.autos.oh.weapon.clone();
        0.5 * self.normalized_weapon_damage(&weapon, attack_power)
    }

    /// Go `Weapon.CalculateNormalizedWeaponDamage`.
    #[allow(dead_code)] // Shared with the class domains in progress.
    pub(crate) fn normalized_weapon_damage(&mut self, weapon: &Weapon, attack_power: f64) -> f64 {
        let roll = self.random("Weapon Base Damage");
        // Go Weapon.BaseDamage, which the arm64 build fuses.
        (weapon.base_damage_max - weapon.base_damage_min).mul_add(roll, weapon.base_damage_min)
            + (weapon.normalized_swing_speed * attack_power) / weapon.attack_power_per_dps
    }

    /// Go `CalcOutcome` with a physical outcome applier: no damage, modifiers or debug line.
    #[allow(dead_code)] // Shared with the class domains in progress.
    pub(crate) fn calc_physical_outcome(
        &mut self,
        spell: SpellId,
        target: Side,
        outcome: PhysicalOutcome,
    ) -> SpellResult {
        let mut result = SpellResult {
            armor_multiplier: 0.0,
            target,
            attacker: self.spells[spell].caster,
            outcome: 0,
            damage: 0.0,
            threat: 0.0,
        };
        self.apply_physical_outcome(spell, &mut result, outcome);
        result.threat = self.threat_of(spell, &result);
        result
    }

    /// Go `calcDamageInternal` for a physical periodic tick on its inputs: bleeds ignore
    /// armor, and the tick rolls physical crit when it can crit.
    pub(crate) fn calc_physical_periodic(
        &mut self,
        spell: SpellId,
        target: Side,
        base: f64,
        attacker: f64,
        can_crit: bool,
    ) -> SpellResult {
        let mut result = SpellResult {
            armor_multiplier: 0.0,
            target,
            attacker: self.spells[spell].caster,
            outcome: 0,
            damage: base * attacker,
            threat: 0.0,
        };
        let after_attacker = result.damage;
        let after_resistances = result.damage;
        if !self.spells[spell].flags.ignore_target_modifiers {
            result.damage += self.config.melee.defender_bonus_physical_damage_taken;
            result.damage *= self.target_multiplier(spell, target);
        }
        let after_target = result.damage;
        // Go OutcomeTickPhysicalCrit, or Dot.OutcomeTick.
        let index = target.index();
        if can_crit && self.proc(self.physical_crit_chance(spell), "Physical Crit Roll") {
            result.outcome = OUTCOME_CRIT;
            result.damage *= self.crit_multiplier(spell);
            self.spells[spell].metrics[index].crit_ticks += 1;
        } else {
            result.outcome = OUTCOME_HIT;
            self.spells[spell].metrics[index].ticks += 1;
        }
        let after_outcome = result.damage;
        self.apply_post_outcome_modifiers(spell, &mut result);
        if self.log.is_some() {
            self.log_damage_debug(
                spell,
                result.target,
                base,
                [
                    after_attacker,
                    after_resistances,
                    after_target,
                    after_outcome,
                ],
                result.damage,
            );
        }
        result.threat = self.threat_of(spell, &result);
        result
    }

    /// Go `CalcPeriodicDamage` with `OutcomeExpectedMagicAlwaysHit` for a physical tick: no
    /// roll, and the damage only.
    pub(crate) fn expected_physical_periodic(
        &mut self,
        spell: SpellId,
        target: Side,
        base: f64,
        attacker: f64,
    ) -> f64 {
        let mut result = SpellResult {
            armor_multiplier: 0.0,
            target,
            attacker: self.spells[spell].caster,
            outcome: 0,
            damage: base * attacker,
            threat: 0.0,
        };
        let after_attacker = result.damage;
        if !self.spells[spell].flags.ignore_target_modifiers {
            result.damage += self.config.melee.defender_bonus_physical_damage_taken;
            result.damage *= self.target_multiplier(spell, target);
        }
        let after_target = result.damage;
        self.apply_post_outcome_modifiers(spell, &mut result);
        if self.log.is_some() {
            self.log_damage_debug(
                spell,
                result.target,
                base,
                [after_attacker, after_attacker, after_target, after_target],
                result.damage,
            );
        }
        result.damage
    }

    /// Go `Spell.PhysicalCritChance`, for class expectations.
    pub(crate) fn spell_physical_crit_chance(&self, spell: SpellId) -> f64 {
        self.physical_crit_chance(spell)
    }

    /// Go `Weapon.CalculateWeaponDamage`.
    pub(crate) fn weapon_damage(&mut self, weapon: &Weapon, attack_power: f64) -> f64 {
        let roll = self.random("Weapon Base Damage");
        // Go Weapon.BaseDamage, which the arm64 build fuses.
        (weapon.base_damage_max - weapon.base_damage_min).mul_add(roll, weapon.base_damage_min)
            + (weapon.swing_speed * attack_power) / weapon.attack_power_per_dps
    }

    /// Go `getAttackPowerValueImpl` for the player, with no mob type bonus.
    pub(crate) fn melee_attack_power(&self) -> f64 {
        self.melee_attack_power_of(Side::Player)
    }

    /// Go `getAttackPowerValueImpl` of an acting unit, with the player's mob type bonus.
    pub(crate) fn melee_attack_power_of(&self, side: Side) -> f64 {
        let bonus = if side == Side::Player {
            self.mob_type_attack_power() + self.mob_type_melee_attack_power()
        } else {
            0.0
        };
        self.unit(side).powers.attack_power
            + self.unit_config(side).melee.defender_bonus_attack_power
            + bonus
    }

    /// Go `AttackTable.MobTypeBonusStats[Defender.MobType][RangedAttackPower]` of the player,
    /// which its melee and ranged attack power both read.
    pub(crate) fn mob_type_attack_power(&self) -> f64 {
        self.mob_type_bonuses
            .iter()
            .filter(|(aura, _, _)| self.aura(*aura).active)
            .map(|(_, attack_power, _)| attack_power)
            .sum()
    }

    /// Under Forever's rules, the player's melee attack power against the targets' type alone.
    pub(crate) fn mob_type_melee_attack_power(&self) -> f64 {
        self.mob_type_melee_bonuses
            .iter()
            .filter(|(aura, _)| self.aura(*aura).active)
            .map(|(_, attack_power)| attack_power)
            .sum()
    }

    /// Go `GetArmorDamageModifier` for the attacker against a target.
    pub(crate) fn armor_modifier(&self, side: Side, target: Side) -> f64 {
        let config = self.unit_config(side);
        let melee = &config.melee;
        if melee.ignore_armor {
            return 1.0;
        }
        let ignore = melee.armor_ignore_factor.clamp(0.0, 1.0);
        let constant = 400.0 + 85.0 * f64::from(config.player_level);
        // The arm64 build fuses the ignored share into the subtraction.
        let target_armor = self.target_unit(target).armor;
        let armor = (-target_armor).mul_add(ignore, target_armor);
        let armor = (armor - self.unit(side).powers.armor_penetration).max(0.0);
        (1.0 - armor / (armor + constant)).max(0.25)
    }

    /// Go `CalcDamage` for a weapon spell with a physical outcome applier. A weapon of a magic
    /// school, such as the Shadowfiend's, takes spell power by its coefficient and the partial
    /// resist roll instead of armor.
    pub(crate) fn calc_physical_damage(
        &mut self,
        spell: SpellId,
        target: Side,
        base_damage: f64,
        outcome: PhysicalOutcome,
    ) -> SpellResult {
        let attacker = self.attacker_multiplier(spell, false);
        let mut base = base_damage;
        let coefficient = self.spells[spell].bonus_coefficient;
        let physical = self.spells[spell].school & 1 != 0;
        if !physical {
            return self.calc_magic_weapon_damage(spell, target, base, attacker, outcome);
        }
        if coefficient > 0.0 {
            base += coefficient * self.physical_bonus_damage(spell);
        } else {
            base += self.physical_bonus_damage(spell);
        }
        let mut result = SpellResult {
            armor_multiplier: 0.0,
            target,
            attacker: self.spells[spell].caster,
            outcome: 0,
            damage: base * attacker,
            threat: 0.0,
        };
        let after_attacker = result.damage;
        let caster = self.caster(spell);
        result.armor_multiplier = 1.0;
        if !self.spells[spell].flags.ignore_resists {
            result.armor_multiplier = self.armor_modifier(caster, target);
            result.damage *= result.armor_multiplier;
        }
        let after_resistances = result.damage;
        if !self.spells[spell].flags.ignore_target_modifiers {
            result.damage += self
                .unit_config(caster)
                .melee
                .defender_bonus_physical_damage_taken;
            result.damage *= self.target_multiplier(spell, target);
        }
        let after_target = result.damage;
        self.apply_physical_outcome(spell, &mut result, outcome);
        let after_outcome = result.damage;
        self.apply_post_outcome_modifiers(spell, &mut result);
        if self.log.is_some() {
            self.log_damage_debug(
                spell,
                result.target,
                base,
                [
                    after_attacker,
                    after_resistances,
                    after_target,
                    after_outcome,
                ],
                result.damage,
            );
        }
        result.threat = self.threat_of(spell, &result);
        result
    }

    /// [`Self::calc_physical_damage`] for a weapon of a magic school: Go `BonusDamage`'s spell
    /// power by the coefficient, the partial resist roll, spell damage taken, then the physical
    /// outcome, which keeps the partial resist.
    fn calc_magic_weapon_damage(
        &mut self,
        spell: SpellId,
        target: Side,
        base_damage: f64,
        attacker: f64,
        outcome: PhysicalOutcome,
    ) -> SpellResult {
        let mut base = base_damage;
        let coefficient = self.spells[spell].bonus_coefficient;
        if coefficient > 0.0 {
            base += coefficient * self.bonus_damage(spell, target);
        }
        let mut result = SpellResult {
            armor_multiplier: 0.0,
            target,
            attacker: self.spells[spell].caster,
            outcome: 0,
            damage: base * attacker,
            threat: 0.0,
        };
        let after_attacker = result.damage;
        result.armor_multiplier = 1.0;
        if !self.spells[spell].flags.ignore_resists {
            // Spell Vulnerability multiplies the partial resist, and a binary spell's whole hit.
            let vulnerability = self.spell_vulnerability(spell, target);
            let (multiplier, partial) = if self.spells[spell].flags.binary {
                (vulnerability, 0)
            } else {
                let (partial, outcome) = self.partial_resist(spell, target);
                (partial * vulnerability, outcome)
            };
            result.armor_multiplier = multiplier;
            result.damage *= multiplier;
            result.outcome |= partial;
        }
        let after_resistances = result.damage;
        if !self.spells[spell].flags.ignore_target_modifiers {
            if self.spells[spell].school_index > 1 {
                result.damage += self.config.target_bonus_spell_damage_taken;
            }
            result.damage *= self.target_multiplier(spell, target);
        }
        let after_target = result.damage;
        let partial = result.outcome & OUTCOME_PARTIAL;
        self.apply_physical_outcome(spell, &mut result, outcome);
        result.outcome |= partial;
        let after_outcome = result.damage;
        self.apply_post_outcome_modifiers(spell, &mut result);
        if self.log.is_some() {
            self.log_damage_debug(
                spell,
                result.target,
                base,
                [
                    after_attacker,
                    after_resistances,
                    after_target,
                    after_outcome,
                ],
                result.damage,
            );
        }
        result.threat = self.threat_of(spell, &result);
        result
    }

    /// Go `CalcPeriodicDamage` for a physical dot with `Dot.OutcomeTick` or
    /// `OutcomeTickPhysicalCrit`: the periodic attacker multiplier, no armor for a bleed, the
    /// target's physical modifiers and a tick counter.
    pub(crate) fn calc_physical_periodic_damage(
        &mut self,
        spell: SpellId,
        target: Side,
        base_damage: f64,
        crit: bool,
    ) -> SpellResult {
        let dot = self.spells[spell].dot.expect("a periodic spell has a dot");
        let coefficient = self.dots[dot].bonus_coefficient;
        let mut base = base_damage;
        if coefficient > 0.0 {
            base += coefficient * self.physical_bonus_damage(spell);
        }
        let attacker =
            self.attacker_multiplier(spell, true) * self.dots[dot].periodic_damage_multiplier;
        let mut result = SpellResult {
            armor_multiplier: 0.0,
            target,
            attacker: self.spells[spell].caster,
            outcome: 0,
            damage: base * attacker,
            threat: 0.0,
        };
        let after_attacker = result.damage;
        // Go ResistanceMultiplier: physical dots ignore armor.
        let after_resistances = result.damage;
        if !self.spells[spell].flags.ignore_target_modifiers {
            result.damage += self.config.melee.defender_bonus_physical_damage_taken;
            result.damage *= self.target_multiplier(spell, target);
        }
        let after_target = result.damage;
        // Go Dot.OutcomeTick, or OutcomeTickPhysicalCrit for a tick that can crit.
        if crit {
            self.outcome_tick_physical_crit(spell, &mut result);
        } else {
            result.outcome = OUTCOME_HIT;
            self.spells[spell].metrics[target.index()].ticks += 1;
        }
        let after_outcome = result.damage;
        self.apply_post_outcome_modifiers(spell, &mut result);
        if self.log.is_some() {
            self.log_damage_debug(
                spell,
                result.target,
                base,
                [
                    after_attacker,
                    after_resistances,
                    after_target,
                    after_outcome,
                ],
                result.damage,
            );
        }
        result.threat = self.threat_of(spell, &result);
        result
    }

    /// Go `Spell.BonusDamage` for a physical spell.
    fn physical_bonus_damage(&self, spell: SpellId) -> f64 {
        self.spells[spell].bonus_base_damage + self.unit(self.caster(spell)).powers.physical_damage
    }

    /// Go `PhysicalHitChance`: ranged attacks add ranged hit.
    fn physical_hit_chance(&self, spell: SpellId) -> f64 {
        let caster = self.caster(spell);
        let config = self.unit_config(caster);
        let mut hit = self.unit(caster).powers.physical_hit_percent
            + self.spells[spell].bonus_hit_percent
            - config.melee.defender_reduced_physical_hit_taken;
        if self.spells[spell].ranged_proc {
            hit += config.ranged_hit_percent;
        }
        let suppression = self
            .skill_table(spell)
            .map_or(config.melee.hit_suppression, |table| table.hit_suppression);
        (hit / 100.0 - suppression).max(0.0)
    }

    /// Go `PhysicalCritChance`: ranged attacks add ranged crit.
    pub(crate) fn physical_crit_chance(&self, spell: SpellId) -> f64 {
        let caster = self.caster(spell);
        let config = self.unit_config(caster);
        let mut crit = self.unit(caster).powers.physical_crit_percent
            + self.spells[spell].bonus_crit_percent
            - config.target_reduced_crit_taken_percent;
        if self.spells[spell].ranged_proc {
            crit += config.ranged_crit_percent;
        }
        (crit / 100.0 - config.melee.melee_crit_suppression).max(0.0)
    }

    /// Go `AutoAttacks.StopMeleeUntil`: the swings restart a full swing after `ready_at`.
    pub(crate) fn stop_melee_until(&mut self, ready_at: i64) {
        self.stop_melee_until_of(Side::Player, ready_at);
    }

    /// [`Self::stop_melee_until`] for an acting unit.
    pub(crate) fn stop_melee_until_of(&mut self, side: Side, ready_at: i64) {
        let autos = self.autos_of_mut(side);
        if !autos.melee {
            return;
        }
        autos.mh.swing_at = ready_at + autos.mh.cur_swing_duration;
        let mut min_time = autos.mh.swing_at;
        if autos.dual_wielding {
            autos.oh.swing_at = ready_at + autos.oh.cur_swing_duration;
            min_time = min_time.min(autos.oh.swing_at);
        }
        self.autos.min_time = self.autos.min_time.min(min_time);
    }

    /// Go `DodgeParrySuppression`.
    fn dodge_parry_suppression(&self, spell: SpellId) -> f64 {
        (self.unit_config(self.caster(spell)).expertise_percent
            + self.spells[spell].bonus_expertise_percent)
            / 100.0
    }

    /// The physical attack table of a spell's caster against the target.
    fn melee_table(&self, spell: SpellId) -> &crate::contracts::prepared_v2::Melee {
        &self.unit_config(self.caster(spell)).melee
    }

    /// Go weapon_skill.go: the table an attack reads in place of [`Self::melee_table`]'s miss,
    /// hit suppression, dodge and glance fields, where its hand's weapon skill moves them.
    fn skill_table(&self, spell: SpellId) -> Option<&WeaponSkillTable> {
        let melee = self.melee_table(spell);
        match self.spells[spell].skill_hand? {
            SkillHand::MainHand => melee.main_hand_skill.as_ref(),
            SkillHand::OffHand => melee.off_hand_skill.as_ref(),
            SkillHand::Ranged => melee.ranged_skill.as_ref(),
        }
    }

    /// Go `applyAttackTableMiss` and `applyAttackTableMissNoDWPenalty`.
    fn table_miss(
        &mut self,
        spell: SpellId,
        result: &mut SpellResult,
        roll: f64,
        chance: &mut f64,
        dw_penalty: bool,
    ) -> bool {
        let base = self
            .skill_table(spell)
            .map_or(self.melee_table(spell).base_miss_chance, |table| {
                table.miss_chance
            });
        let mut miss = base - self.physical_hit_chance(spell);
        if dw_penalty
            && self.autos_of(self.caster(spell)).dual_wielding
            && !self.unit(self.caster(spell)).disable_dw_miss_penalty
        {
            miss += 0.19;
        }
        *chance = miss.max(0.0);
        if roll < *chance {
            result.outcome = OUTCOME_MISS;
            self.spells[spell].metrics[result.target.index()].misses += 1;
            result.damage = 0.0;
            return true;
        }
        false
    }

    /// Go `applyAttackTableDodge`.
    fn table_dodge(
        &mut self,
        spell: SpellId,
        result: &mut SpellResult,
        roll: f64,
        chance: &mut f64,
    ) -> bool {
        if self.spells[spell].flags.cannot_be_dodged {
            return false;
        }
        let melee = self.melee_table(spell);
        let dodge = self
            .skill_table(spell)
            .map_or(melee.defender_dodge, |table| table.defender_dodge);
        *chance += (dodge - self.dodge_parry_suppression(spell) - melee.dodge_reduction).max(0.0);
        if roll < *chance {
            result.outcome = OUTCOME_DODGE;
            self.spells[spell].metrics[result.target.index()].dodges += 1;
            result.damage = 0.0;
            return true;
        }
        false
    }

    /// Go `applyAttackTableParry`.
    fn table_parry(
        &mut self,
        spell: SpellId,
        result: &mut SpellResult,
        roll: f64,
        chance: &mut f64,
    ) -> bool {
        *chance +=
            (self.melee_table(spell).defender_parry - self.dodge_parry_suppression(spell)).max(0.0);
        if roll < *chance {
            result.outcome = OUTCOME_PARRY;
            self.spells[spell].metrics[result.target.index()].parries += 1;
            result.damage = 0.0;
            return true;
        }
        false
    }

    /// Go `applyAttackTableGlance`.
    fn table_glance(
        &mut self,
        spell: SpellId,
        result: &mut SpellResult,
        roll: f64,
        chance: &mut f64,
    ) -> bool {
        *chance += self.melee_table(spell).base_glance_chance;
        if roll < *chance {
            result.outcome = OUTCOME_GLANCE;
            self.spells[spell].metrics[result.target.index()].glances += 1;
            let spread = 2.0 * self.random("Glance Damage") - 1.0;
            let melee = self.melee_table(spell);
            let (multiplier, glance_spread) = self
                .skill_table(spell)
                .map_or((melee.glance_multiplier, melee.glance_spread), |table| {
                    (table.glance_multiplier, table.glance_spread)
                });
            // The arm64 build fuses the spread into the multiplier.
            result.damage *= glance_spread.mul_add(spread, multiplier);
            return true;
        }
        false
    }

    /// Go `applyAttackTableBlock`: a blocked crit moves from crits to blocked crits.
    fn table_block(
        &mut self,
        spell: SpellId,
        result: &mut SpellResult,
        roll: f64,
        chance: &mut f64,
    ) -> bool {
        *chance += self.melee_table(spell).defender_block.max(0.0);
        if roll < *chance {
            let partial = result.outcome & OUTCOME_PARTIAL != 0;
            result.outcome |= OUTCOME_BLOCK;
            let metrics = &mut self.spells[spell].metrics[result.target.index()];
            if result.outcome & OUTCOME_CRIT != 0 {
                metrics.crits -= 1;
                metrics.blocked_crits += 1;
                if partial {
                    metrics.resisted_crits -= 1;
                }
            } else {
                metrics.blocks += 1;
            }
            result.damage = if self.spells[spell].flags.binary {
                0.0
            } else {
                (result.damage - self.melee_table(spell).defender_block_reduction).max(0.0)
            };
            return true;
        }
        false
    }

    /// Go `applyAttackTableCrit` on the shared roll.
    fn table_crit(
        &mut self,
        spell: SpellId,
        result: &mut SpellResult,
        roll: f64,
        chance: &mut f64,
        count: bool,
    ) -> bool {
        *chance += self.physical_crit_chance(spell);
        if roll < *chance {
            self.land_crit(spell, result, count);
            return true;
        }
        false
    }

    /// Go `applyAttackTableCritSeparateRoll`.
    fn table_crit_separate(
        &mut self,
        spell: SpellId,
        result: &mut SpellResult,
        count: bool,
    ) -> bool {
        if self.proc(self.physical_crit_chance(spell), "Physical Crit Roll") {
            self.land_crit(spell, result, count);
            return true;
        }
        false
    }

    fn land_crit(&mut self, spell: SpellId, result: &mut SpellResult, count: bool) {
        let partial = result.outcome & OUTCOME_PARTIAL != 0;
        result.outcome = OUTCOME_CRIT;
        if count {
            let metrics = &mut self.spells[spell].metrics[result.target.index()];
            metrics.crits += 1;
            if partial {
                metrics.resisted_crits += 1;
            }
        }
        result.damage *= self.crit_multiplier(spell);
    }

    /// Go `applyAttackTableHit`.
    fn table_hit(&mut self, spell: SpellId, result: &mut SpellResult, count: bool) {
        let partial = result.outcome & OUTCOME_PARTIAL != 0;
        result.outcome = OUTCOME_HIT;
        if count {
            let metrics = &mut self.spells[spell].metrics[result.target.index()];
            metrics.hits += 1;
            if partial {
                metrics.resisted_hits += 1;
            }
        }
    }

    /// Apply a physical outcome, composed as Go composes each applier.
    pub(crate) fn apply_physical_outcome(
        &mut self,
        spell: SpellId,
        result: &mut SpellResult,
        outcome: PhysicalOutcome,
    ) {
        let front = self.melee_table(spell).in_front_of_target;
        let mut chance = 0.0;
        match outcome {
            PhysicalOutcome::MeleeWhite => {
                let roll = self.random("White Hit Table");
                let _ = self.table_miss(spell, result, roll, &mut chance, true)
                    || self.table_dodge(spell, result, roll, &mut chance)
                    || (front && self.table_parry(spell, result, roll, &mut chance))
                    || self.table_glance(spell, result, roll, &mut chance)
                    || (front && self.table_block(spell, result, roll, &mut chance))
                    || self.table_crit(spell, result, roll, &mut chance, true)
                    || {
                        self.table_hit(spell, result, true);
                        true
                    };
            }
            PhysicalOutcome::MeleeSpecialHit { count } => {
                let roll = self.random("White Hit Table");
                let _ = self.table_miss(spell, result, roll, &mut chance, false)
                    || self.table_dodge(spell, result, roll, &mut chance)
                    || (front && self.table_parry(spell, result, roll, &mut chance))
                    || {
                        self.table_hit(spell, result, count);
                        true
                    };
            }
            PhysicalOutcome::MeleeSpecialHitAndCrit { count } => {
                let roll = self.random("White Hit Table");
                if self.table_miss(spell, result, roll, &mut chance, false)
                    || self.table_dodge(spell, result, roll, &mut chance)
                {
                    return;
                }
                if front {
                    if self.table_parry(spell, result, roll, &mut chance) {
                        return;
                    }
                    if self.table_crit_separate(spell, result, count) {
                        self.table_block(spell, result, roll, &mut chance);
                    } else if !self.table_block(spell, result, roll, &mut chance) {
                        self.table_hit(spell, result, count);
                    }
                } else if !self.table_crit_separate(spell, result, count) {
                    self.table_hit(spell, result, count);
                }
            }
            PhysicalOutcome::MeleeWeaponSpecialHitAndCrit { count } => {
                if !front {
                    return self.apply_physical_outcome(
                        spell,
                        result,
                        PhysicalOutcome::MeleeSpecialHitAndCrit { count },
                    );
                }
                let roll = self.random("White Hit Table");
                let _ = self.table_miss(spell, result, roll, &mut chance, false)
                    || self.table_dodge(spell, result, roll, &mut chance)
                    || self.table_parry(spell, result, roll, &mut chance)
                    || self.table_block(spell, result, roll, &mut chance)
                    || self.table_crit_separate(spell, result, count)
                    || {
                        self.table_hit(spell, result, count);
                        true
                    };
            }
            PhysicalOutcome::MeleeSpecialBlockAndCrit { count } => {
                if front {
                    let roll = self.random("White Hit Table");
                    let _ = self.table_crit_separate(spell, result, count)
                        || self.table_block(spell, result, roll, &mut chance)
                        || {
                            self.table_hit(spell, result, count);
                            true
                        };
                } else if !self.table_crit_separate(spell, result, count) {
                    self.table_hit(spell, result, count);
                }
            }
            PhysicalOutcome::MeleeSpecialNoBlockDodgeParryNoCrit { count } => {
                let roll = self.random("White Hit Table");
                let _ = self.table_miss(spell, result, roll, &mut chance, false) || {
                    self.table_hit(spell, result, count);
                    true
                };
            }
            PhysicalOutcome::MeleeSpecialNoBlockDodgeParry { count } => {
                let roll = self.random("White Hit Table");
                let _ = self.table_miss(spell, result, roll, &mut chance, false)
                    || self.table_crit_separate(spell, result, count)
                    || {
                        self.table_hit(spell, result, count);
                        true
                    };
            }
            PhysicalOutcome::MeleeSpecialCritOnly { count } => {
                if !self.table_crit_separate(spell, result, count) {
                    self.table_hit(spell, result, count);
                }
            }
            PhysicalOutcome::RangedHit { count } => {
                let roll = self.random("White Hit Table");
                if !self.table_miss(spell, result, roll, &mut chance, false) {
                    self.table_hit(spell, result, count);
                }
            }
            PhysicalOutcome::RangedHitAndCrit { count } => {
                let roll = self.random("White Hit Table");
                if self.table_miss(spell, result, roll, &mut chance, false) {
                    return;
                }
                if front {
                    if self.table_crit_separate(spell, result, count) {
                        self.table_block(spell, result, roll, &mut chance);
                    } else if !self.table_block(spell, result, roll, &mut chance) {
                        self.table_hit(spell, result, count);
                    }
                } else if !self.table_crit_separate(spell, result, count) {
                    self.table_hit(spell, result, count);
                }
            }
        }
    }
}

/// Go physical outcome appliers the runtime implements; `count` is false for the
/// `NoHitCounter` variants. The names follow Go's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names, dead_code)] // Shared with the class domains in progress.
pub(crate) enum PhysicalOutcome {
    MeleeWhite,
    MeleeSpecialHit {
        count: bool,
    },
    MeleeSpecialHitAndCrit {
        count: bool,
    },
    MeleeWeaponSpecialHitAndCrit {
        count: bool,
    },
    MeleeSpecialBlockAndCrit {
        count: bool,
    },
    MeleeSpecialNoBlockDodgeParry {
        count: bool,
    },
    /// Go `OutcomeMeleeSpecialNoBlockDodgeParryNoCrit`: a miss roll, else a hit.
    MeleeSpecialNoBlockDodgeParryNoCrit {
        count: bool,
    },
    MeleeSpecialCritOnly {
        count: bool,
    },
    RangedHit {
        count: bool,
    },
    RangedHitAndCrit {
        count: bool,
    },
}

impl PhysicalOutcome {
    /// The melee special applier an item proc's damage rolls, by the name the exporter writes
    /// for shared_utils.go `damageOutcome` of a spell with the melee defense type: with a crit,
    /// or without when the client bars the spell from critting. Both count their hits.
    pub(crate) fn of_melee_proc(name: &str) -> Option<Self> {
        match name {
            "melee_special_hit_and_crit" => Some(Self::MeleeSpecialHitAndCrit { count: true }),
            "melee_special_hit" => Some(Self::MeleeSpecialHit { count: true }),
            // A proc whose client row carries the always-hit attribute: the crit roll and no miss.
            "melee_special_crit_only" => Some(Self::MeleeSpecialCritOnly { count: true }),
            _ => None,
        }
    }
}
