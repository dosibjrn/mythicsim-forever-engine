//! Totem slots, from Go sim/shaman/shaman.go `TotemExpirations` and sim/shaman/totems.go:
//! each cast records when its slot's totem expires, which `totemRemainingTime` reads, and an
//! earth totem replaces the shaman's previous one.

use crate::{
    core::{
        fight::{Agent, AuraRef, Fight},
        time::NS_PER_SECOND,
    },
    rotation::Totem,
};

/// Go `Shaman.Reset`: every slot expired ten hours before the fight.
pub(crate) const RESET_EXPIRATION: i64 = -10 * 3600 * NS_PER_SECOND;

/// When each slot's totem expires, by Go's slot order: air, earth, fire, water.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Expirations([i64; 4]);

impl Default for Expirations {
    fn default() -> Self {
        Expirations([RESET_EXPIRATION; 4])
    }
}

impl Expirations {
    fn slot(totem: Totem) -> usize {
        match totem {
            Totem::Air => 0,
            Totem::Earth => 1,
            Totem::Fire => 2,
            Totem::Water => 3,
        }
    }

    pub(crate) fn get(&self, totem: Totem) -> i64 {
        self.0[Self::slot(totem)]
    }

    pub(crate) fn set(&mut self, totem: Totem, expires: i64) {
        self.0[Self::slot(totem)] = expires;
    }
}

/// Strength of Earth Totem's cast.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StrengthOfEarth {
    pub(crate) aura: AuraRef,
    pub(crate) duration: i64,
}

/// The Restoration shaman's Mana Tide Totem (Forever's rules): its aura and duration, and the
/// Healing Stream Totem spells, by spellbook position, that share its water totem slot.
#[derive(Clone, Debug)]
pub(crate) struct ManaTide {
    pub(crate) totem: StrengthOfEarth,
    pub(crate) streams: Vec<crate::core::fight::SpellId>,
}

/// Go `registerStrengthOfEarthTotemSpell` ApplyEffects: the previous earth totem's aura goes,
/// the slot records the new one's expiry and its aura activates. The only earth totem in
/// scope is this one, so the previous aura is its own. Returns the new expiry.
pub(crate) fn strength_of_earth<A: Agent>(fight: &mut Fight<A>, totem: StrengthOfEarth) -> i64 {
    fight.deactivate_aura(totem.aura);
    let expires = fight.now + totem.duration;
    fight.activate_aura(totem.aura);
    expires
}

/// Flametongue Totem, from Go sim/shaman/fire_totems.go `registerFlametongueTotemSpell` and
/// sim/core/buffs/flametongue_totem.go: the totem's aura turns the trigger on while it holds
/// the Flametongue Totem category, which a Windfury Totem or a main hand Flametongue Weapon
/// takes from it, and the trigger casts the fire hit off landed main hand autos.
#[derive(Clone, Debug)]
pub(crate) struct FlametongueTotem {
    pub(crate) aura: AuraRef,
    pub(crate) trigger: AuraRef,
    pub(crate) attack: crate::core::fight::SpellId,
    pub(crate) attack_deals_damage: bool,
    pub(crate) attack_damage: f64,
    /// The spells whose landed hits cast the attack, by spellbook position.
    pub(crate) triggers: Vec<bool>,
    pub(crate) enabled: bool,
    pub(crate) duration: i64,
}
