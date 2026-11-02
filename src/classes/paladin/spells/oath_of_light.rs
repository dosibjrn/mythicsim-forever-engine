//! Oath of Light (1328548, hotfix push 113554, Forever's rules), from upstream wowsims/forever
//! `oath_of_light.go`: the cast swears the Oath on the healed ally, and the paladin's Holy Light,
//! Flash of Light and Holy Shock heals on that ally add a flat bonus to their base amount, before
//! spell power. The sim's one heal target is the player, so the aura is the player's.

use crate::core::fight::{AuraRef, Fight, ModId, ModKind, Side};

use super::super::agent::PaladinAgent;

/// The Oath: its aura, the three flat bonuses and the crit Boundless Light gives a Holy Shock
/// heal on an ally without it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OathOfLight {
    pub(crate) aura: AuraRef,
    pub(crate) holy_light: f64,
    pub(crate) flash_of_light: f64,
    pub(crate) holy_shock: f64,
    /// The share of heals, while the Oath is up, that reach its holder (the tank, by the party's
    /// hit rates); the rest reach players without it.
    pub(crate) holder_share: f64,
    /// Boundless Light's crit on a Holy Shock heal on a player without the Oath, which the
    /// aura's mod takes off every Holy Shock heal.
    pub(crate) boundless_light_crit: f64,
    /// A crit mod that takes Boundless Light's bonus off the Holy Shock heal while the Oath
    /// holds, when the paladin has the talent.
    boundless_light: Option<ModId>,
}

impl OathOfLight {
    pub(crate) fn bind(
        fight: &mut Fight<PaladinAgent>,
        aura: &str,
        bonuses: (f64, f64, f64),
        boundless_light_crit: f64,
        holder_share: f64,
    ) -> Result<Self, String> {
        if !(0.0..=1.0).contains(&holder_share) {
            return Err(format!("Oath of Light holder share {holder_share}"));
        }
        let aura = fight.player_aura(aura)?;
        let boundless_light = (boundless_light_crit > 0.0).then(|| {
            let heals = fight.spells_with_class(&["holy_shock_heal"]);
            fight.register_mod(ModKind::BonusCritPercent, -boundless_light_crit, 0, heals)
        });
        Ok(OathOfLight {
            aura,
            holy_light: bonuses.0,
            flash_of_light: bonuses.1,
            holy_shock: bonuses.2,
            holder_share,
            boundless_light_crit,
            boundless_light,
        })
    }

    /// The cast: "only one Oath of Light may be sworn by the Paladin at any one time", so the
    /// Oath on the player ends, and it is sworn again when the ally is the player.
    pub(crate) fn apply(self, fight: &mut Fight<PaladinAgent>, target: Side) {
        if target == Side::Player {
            fight.activate_aura(self.aura);
        } else {
            fight.deactivate_aura(self.aura);
        }
    }

    /// Whether the ally `target` carries the Oath.
    pub(crate) fn on(self, fight: &Fight<PaladinAgent>, target: Side) -> bool {
        target == Side::Player && fight.aura(self.aura).active
    }

    pub(crate) fn on_gain(self, fight: &mut Fight<PaladinAgent>) {
        if let Some(boundless_light) = self.boundless_light {
            fight.activate_mod(boundless_light);
        }
    }

    pub(crate) fn on_expire(self, fight: &mut Fight<PaladinAgent>) {
        if let Some(boundless_light) = self.boundless_light {
            fight.deactivate_mod(boundless_light);
        }
    }
}
