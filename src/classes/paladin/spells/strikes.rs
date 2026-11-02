//! Holy Strike, Hammer of Wrath and Consecration, from Go sim/paladin/holy_strike.go,
//! hammer_of_wrath.go and consecration.go.

use crate::{
    contracts::prepared_v2::ConsecrationRank,
    core::fight::{melee::PhysicalOutcome, DotId, Fight, Outcome, Side, SpellId, OUTCOME_LANDED},
};

use super::super::agent::PaladinAgent;

/// Holy Strike: the rank's percent of the normalized swing plus the flat roll, on the melee
/// table.
pub(crate) fn holy_strike(
    fight: &mut Fight<PaladinAgent>,
    spell: SpellId,
    target: Side,
    weapon_percent: f64,
) {
    let attack_power = fight.melee_attack_power();
    let weapon = fight.mh_normalized_weapon_damage(attack_power);
    let flat = fight.roll_damage_effect(spell);
    let base = weapon_percent * (weapon + flat);
    let result = fight.calc_damage_with(
        spell,
        target,
        base,
        Outcome::Table(PhysicalOutcome::MeleeSpecialHitAndCrit { count: true }),
    );
    fight.deal_damage(spell, result, false);
    if fight.agent.shieldsworn && result.outcome & OUTCOME_LANDED != 0 {
        shieldsworn(fight, result.target);
    }
}

/// Shieldsworn (1328544, push 113554, Forever's rules; upstream #772's holy_shield.go and
/// talents_protection.go): a Holy Strike that lands strikes with the shield. The active Holy
/// Shield deals its damage without spending a charge, and Shield Specialization rolls its mana
/// chance without its cooldown. Upstream hears only a hit that lands (its TODO asks whether a
/// miss triggers them too).
fn shieldsworn(fight: &mut Fight<PaladinAgent>, target: Side) {
    for rank in 0..fight.agent.holy_shield.len() {
        let shield = std::rc::Rc::clone(&fight.agent.holy_shield[rank]);
        if fight.aura(shield.aura).active {
            fight.cast(shield.proc_spell, target);
        }
    }
    if let Some(specialization) = fight.agent.shield_specialization.clone() {
        if specialization.chance != 1.0
            && fight.random("Shield Specialization - Shieldsworn") > specialization.chance
        {
            return;
        }
        let amount = fight.player.powers.max_mana * specialization.mana_share;
        fight.add_mana(amount, specialization.metrics);
    }
}

/// Hammer of the Righteous: `weapon_dps` times the main hand's damage a second (Go
/// `Weapon.DPS`), on the melee special table, calculated and dealt at each target in turn.
pub(crate) fn hammer_of_the_righteous(
    fight: &mut Fight<PaladinAgent>,
    spell: SpellId,
    target: Side,
    weapon_dps: f64,
    max_targets: usize,
) {
    let weapon = &fight.autos.mh.weapon;
    let dps = if weapon.swing_speed == 0.0 {
        0.0
    } else {
        (weapon.base_damage_min + weapon.base_damage_max) / 2.0 / weapon.swing_speed
    };
    let base = weapon_dps * dps;
    for side in fight.cleave_targets(target, max_targets) {
        let result = fight.calc_damage_with(
            spell,
            side,
            base,
            Outcome::Table(PhysicalOutcome::MeleeSpecialHitAndCrit { count: true }),
        );
        fight.deal_damage(spell, result, false);
    }
}

/// Hammer of Wrath: the rolled damage on the ranged table, dealt when the hammer lands.
pub(crate) fn hammer_of_wrath(fight: &mut Fight<PaladinAgent>, spell: SpellId, target: Side) {
    let base = fight.roll_damage_effect(spell);
    let result = fight.calc_damage_with(
        spell,
        target,
        base,
        Outcome::Table(PhysicalOutcome::RangedHitAndCrit { count: true }),
    );
    fight.deal_damage_after_travel(spell, result);
}

/// Exorcism: the rolled damage, a magic hit and crit; it casts only at an Undead or Demon.
pub(crate) fn exorcism(fight: &mut Fight<PaladinAgent>, spell: SpellId, target: Side) {
    let base = fight.roll_damage_effect(spell);
    let result = fight.calc_damage(spell, target, base);
    fight.deal_damage(spell, result, false);
}

/// Holy Wrath: a roll of the rank's damage for each Undead or Demon target in unit index
/// order, every hit calculated before any is dealt when the bolts arrive; any other target
/// takes nothing.
pub(crate) fn holy_wrath(fight: &mut Fight<PaladinAgent>, spell: SpellId, hits: bool) {
    if !hits {
        return;
    }
    let sides: Vec<Side> = fight.target_sides().collect();
    let results: Vec<_> = sides
        .into_iter()
        .map(|side| {
            let base = fight.roll_damage_effect(spell);
            fight.calc_damage(spell, side, base)
        })
        .collect();
    fight.deal_damage_after_travel_batch(spell, &results);
}

/// Go `Paladin.holdMeleeForCast`, the `ModifyCast` of Hammer of Wrath, Holy Wrath, Flash of
/// Light, Holy Light and Light's Vigil: the Forever hard-cast rule, where the next swing comes a
/// full weapon speed after the cast completes. A cast made instant (Instrument of Law, a proc)
/// leaves the swing timer alone.
pub(crate) fn hold_melee_for_cast(fight: &mut Fight<PaladinAgent>, spell: SpellId) {
    let cast_time = fight.spells[spell].cur_cast.cast_time;
    let cast_time = fight.apply_cast_speed_for_spell(cast_time, spell);
    if cast_time > 0 {
        let cast_end = fight.now + cast_time;
        fight.hold_melee_for_cast(cast_end);
    }
}

/// Consecration's cast: one magic hit check, then the ground effect regardless.
pub(crate) fn consecration(fight: &mut Fight<PaladinAgent>, spell: SpellId, target: Side) {
    let result = fight.calc_outcome(spell, target, Outcome::MagicHit);
    fight.deal_damage(spell, result, false);
    let dot = fight.spells[spell].dot.expect("Consecration has a dot");
    fight.apply_dot(dot);
}

/// A Consecration tick, which Go deals to each target in unit index order: the first targets
/// take the bonus and have Consecrated Ground marked before their tick lands, and each hit is
/// dealt before the next target's is calculated.
pub(crate) fn consecration_tick(
    fight: &mut Fight<PaladinAgent>,
    dot: DotId,
    rank: &ConsecrationRank,
) {
    let spell = fight.dots[dot].spell;
    let sides: Vec<Side> = fight.target_sides().collect();
    for (position, side) in sides.into_iter().enumerate() {
        let mut damage = rank.tick;
        if i32::try_from(position).is_ok_and(|position| position < rank.bonus_targets) {
            // Go's arm64 build fuses the bonus's coefficient into its average, then adds it.
            damage += rank
                .bonus_coefficient
                .mul_add(fight.bonus_damage(spell, side), rank.bonus);
            if let Some((aura, _)) = fight.agent.consecrated_ground {
                let marked = fight.aura_on(aura, side);
                fight.activate_aura(marked);
            }
        }
        fight.periodic_damage_tick_with(dot, side, damage, Outcome::TickMagicHitAndCrit);
    }
}
