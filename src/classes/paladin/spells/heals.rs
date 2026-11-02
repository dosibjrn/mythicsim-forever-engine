//! Paladin heals, from Go sim/paladin/holy_light.go, flash_of_light.go, holy_shock.go and
//! lay_on_hands.go, and Illumination from talents_holy.go: each rank's roll and bonuses,
//! healed on the unit the cast names with the healing crit roll, and a crit's chance to
//! return part of the cost.

use crate::{
    contracts::prepared_v2::{PaladinHealRank, PaladinHealUnit},
    core::fight::{
        healing::Healing, AuraRef, Fight, ModId, ModKind, Side, SpellId, SpellResult, OUTCOME_CRIT,
    },
};

use super::super::agent::PaladinAgent;
use super::holy;

/// Which heal a rank is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HealKind {
    HolyLight,
    FlashOfLight,
    HolyShockHeal,
}

impl HealKind {
    fn parse(name: &str) -> Result<Self, String> {
        match name {
            "holy_light" => Ok(HealKind::HolyLight),
            "flash_of_light" => Ok(HealKind::FlashOfLight),
            "holy_shock_heal" => Ok(HealKind::HolyShockHeal),
            other => Err(format!("unknown paladin heal {other}")),
        }
    }
}

/// One rank: which heal and its roll.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HealRank {
    pub(crate) kind: HealKind,
    pub(crate) min: f64,
    pub(crate) max: f64,
    /// The client row's average and variance, which Go `Effect.Roll` reads.
    pub(crate) roll: Option<(f64, f64)>,
}

/// The paladin's healing modifiers on a unit and Blessing of Light's presence.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HealUnit {
    pub(crate) healing: Healing,
    pub(crate) bonus_healing_taken: f64,
    pub(crate) blessing_of_light: bool,
}

impl HealUnit {
    fn new(unit: &PaladinHealUnit) -> Self {
        HealUnit {
            healing: Healing {
                dealt_multiplier: unit.healing_dealt_multiplier,
                taken_multiplier: unit.healing_taken_multiplier,
                table_multiplier: unit.table_healing_dealt_multiplier,
                healing_power: unit.healing_power,
            },
            bonus_healing_taken: unit.bonus_healing_taken,
            blessing_of_light: unit.blessing_of_light,
        }
    }
}

/// Every heal rank and the bonuses Go adds to the roll.
#[derive(Clone, Debug)]
pub(crate) struct Heals {
    pub(crate) ranks: Vec<HealRank>,
    pub(crate) flash_of_light_bonus: f64,
    pub(crate) blessing_holy_light: f64,
    pub(crate) blessing_flash_of_light: f64,
    pub(crate) player: HealUnit,
    pub(crate) target: HealUnit,
}

impl Heals {
    pub(crate) fn bind(
        ranks: &[PaladinHealRank],
        flash_of_light_bonus: f64,
        blessing: (f64, f64),
        player: &PaladinHealUnit,
        target: &PaladinHealUnit,
    ) -> Result<Self, String> {
        Ok(Heals {
            ranks: ranks
                .iter()
                .map(|rank| {
                    Ok(HealRank {
                        kind: HealKind::parse(&rank.heal)?,
                        min: rank.min,
                        max: rank.max,
                        roll: rank.average.zip(rank.variance),
                    })
                })
                .collect::<Result<_, String>>()?,
            flash_of_light_bonus,
            blessing_holy_light: blessing.0,
            blessing_flash_of_light: blessing.1,
            player: HealUnit::new(player),
            target: HealUnit::new(target),
        })
    }

    /// The heal's `ApplyEffects`: the rank's roll, which draws only with a variance, the
    /// Flash of Light bonus and Blessing of Light on the healed unit, then the heal.
    pub(crate) fn cast(
        &self,
        fight: &mut Fight<PaladinAgent>,
        spell: SpellId,
        target: Side,
        rank: usize,
    ) {
        let rank = self.ranks[rank];
        let mut amount = match rank.roll {
            Some((average, variance)) => fight.effect_roll(average, variance),
            None if rank.min == rank.max => rank.min,
            None => rank.min + (rank.max - rank.min) * fight.random("Damage Roll"),
        };
        let unit = match target {
            Side::Player => self.player,
            _ => self.target,
        };
        match rank.kind {
            HealKind::HolyLight if unit.blessing_of_light => amount += self.blessing_holy_light,
            HealKind::FlashOfLight => {
                amount += self.flash_of_light_bonus;
                if unit.blessing_of_light {
                    amount += self.blessing_flash_of_light;
                }
            }
            _ => {}
        }
        // Oath of Light (Forever's rules): while the Oath is up, the heal reaches the Oath's
        // holder or another player (`OathOfLight::holder_share`). The holder's heal adds the flat
        // bonus, before spell power; any other player's Holy Shock heal has Boundless Light's
        // crit, which the Oath's aura took off.
        let mut unsworn_crit = 0.0;
        if let Some(oath) = fight.agent.oath.filter(|oath| oath.on(fight, target)) {
            if fight.random("Oath of Light") < oath.holder_share {
                amount += match rank.kind {
                    HealKind::HolyLight => oath.holy_light,
                    HealKind::FlashOfLight => oath.flash_of_light,
                    HealKind::HolyShockHeal => oath.holy_shock,
                };
            } else if rank.kind == HealKind::HolyShockHeal {
                unsworn_crit = oath.boundless_light_crit;
            }
        }
        fight.spells[spell].bonus_crit_percent += unsworn_crit;
        fight.calc_and_deal_healing(
            spell,
            target,
            amount,
            unit.healing,
            unit.bonus_healing_taken,
        );
        fight.spells[spell].bonus_crit_percent -= unsworn_crit;
        // Light's Vigil on an ally: a Holy Shock heal on the player spends it and heals the party.
        if rank.kind == HealKind::HolyShockHeal && target == Side::Player {
            holy::vigil_party_heal(fight, spell, unit.healing, unit.bonus_healing_taken);
        }
    }
}

impl Heals {
    /// Lay on Hands' `ApplyEffects`: the paladin spends all its mana, the healed unit regains
    /// the rank's mana when it has a mana bar, which in scope only the paladin does, then the
    /// heal of the paladin's maximum health.
    pub(crate) fn lay_on_hands(
        &self,
        fight: &mut Fight<PaladinAgent>,
        spell: SpellId,
        target: Side,
        (mana, metrics): (f64, usize),
    ) {
        let current = fight.player.mana;
        fight.spend_mana(current, metrics);
        if target == Side::Player {
            fight.add_mana(mana, metrics);
        }
        let unit = match target {
            Side::Player => self.player,
            _ => self.target,
        };
        let amount = fight.player_max_health();
        fight.calc_and_deal_healing(
            spell,
            target,
            amount,
            unit.healing,
            unit.bonus_healing_taken,
        );
    }
}

/// Illumination: a heal crit's chance to return a share of the heal's base cost, a batch
/// window later.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Illumination {
    pub(crate) chance: f64,
    pub(crate) refund: f64,
    pub(crate) metrics: usize,
}

/// The heals Go's `SpellMaskHealingSpells` names.
fn is_heal(fight: &Fight<PaladinAgent>, spell: SpellId) -> bool {
    matches!(
        fight.spells[spell].class_spell.as_deref(),
        Some("holy_light" | "flash_of_light" | "holy_shock_heal")
    )
}

impl Illumination {
    /// The trigger: a crit from a heal the mask names, not a proc, then the chance.
    pub(crate) fn on_heal_dealt(
        self,
        fight: &mut Fight<PaladinAgent>,
        trigger: AuraRef,
        spell: SpellId,
        result: &SpellResult,
    ) {
        if fight.spells[spell].flags.proc
            || !is_heal(fight, spell)
            || result.outcome & OUTCOME_CRIT == 0
        {
            return;
        }
        if self.chance != 1.0 && fight.random_for_aura(trigger) > self.chance {
            return;
        }
        fight.schedule_delayed_proc(trigger, spell, *result);
    }

    /// The handler: the share of the heal's base cost.
    pub(crate) fn on_delayed_proc(self, fight: &mut Fight<PaladinAgent>, spell: SpellId) {
        let base = fight.spells[spell].cost.map_or(0, |cost| cost.base);
        fight.add_mana(f64::from(base) * self.refund, self.metrics);
    }
}

/// Infusion of Light's or Holy Alacrity's aura: Holy Light's cast time mod while it holds,
/// spent by a Holy Light cast.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HolyLightHaste {
    pub(crate) aura: AuraRef,
    pub(crate) on_crit: bool,
    pub(crate) cast_time_mod: ModId,
}

impl HolyLightHaste {
    pub(crate) fn bind(
        fight: &mut Fight<PaladinAgent>,
        aura: &str,
        on_crit: bool,
        cast_time_ns: i64,
        holy_lights: &[usize],
    ) -> Result<Self, String> {
        let aura = fight.player_aura(aura)?;
        let cast_time_mod = fight.register_mod(
            ModKind::CastTimeFlat,
            0.0,
            cast_time_ns,
            holy_lights.to_vec(),
        );
        Ok(HolyLightHaste {
            aura,
            on_crit,
            cast_time_mod,
        })
    }

    pub(crate) fn toggle(self, fight: &mut Fight<PaladinAgent>, active: bool) {
        if active {
            fight.activate_mod(self.cast_time_mod);
        } else {
            fight.deactivate_mod(self.cast_time_mod);
        }
    }

    /// The aura's own trigger, at once: a Holy Light cast spends it.
    pub(crate) fn on_cast_complete(self, fight: &mut Fight<PaladinAgent>, spell: SpellId) {
        if fight.spells[spell].class_spell.as_deref() == Some("holy_light") {
            fight.deactivate_aura(self.aura);
        }
    }
}
