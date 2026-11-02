//! Oath of Light (1328548), the Holy talent push 113554 adds (Forever's rules only; the reference
//! never sets it, as `Paladin::new_talent` reads 0 there).
//!
//! The client text: "Swear an Oath to protect target ally for 5 min, increasing the healing of
//! your Holy Light on them by up to 400, your Flash of Light by up to 115, and your Holy Shock by
//! up to 229. Only one Oath of Light may be sworn by the Paladin at any one time." Upstream
//! wowsims/forever's `oath_of_light.go` is the shape followed here: a castable helpful spell that
//! puts a 5 minute aura on the ally, and the three heals add the flat bonus to the heal's base
//! amount, before spell power, when the ally they heal carries it. Boundless Light's Holy Shock
//! crit holds only on an ally without the Oath.
//!
//! MythicSim's healer model heals the player itself as a stand-in for a player who needs it, so
//! the aura lives on the player: a rotation casts the Oath on the heal target (`Self`), meaning
//! the tank. While it is up, a Holy Light, Flash of Light or Holy Shock heal reaches the tank
//! only with the tank's share of the party's hits (`prepare::party_damage::tank_share`, 5 in 9);
//! that share carries the bonus, and the rest reach players without the Oath, where Boundless
//! Light's crit holds. A cast on any other unit swears the Oath on an ally the sim does not
//! model: it costs mana and the global cooldown, replaces the Oath on the player and bonuses
//! nothing.

use serde_json::{json, Value};

use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::sim::{AuraConfig, Sim, UnitId};
use crate::prepare::spell::SpellConfig;
use crate::prepare::spell::{ProcMask, SpellFlag};

use super::hotfix_113554 as hotfix;
use super::masks;
use super::util::{gcd_cast, mana_cost, spell_action};
use super::Paladin;

impl Paladin {
    /// The talent's cast: instant, on the global cooldown, for the row's mana, with the aura it
    /// puts on the ally. The spell carries no proc mask, as upstream's does not (it heals
    /// nothing, so it takes no overhealing share and no heal listener hears it).
    pub(super) fn register_oath_of_light(&mut self, sim: &mut Sim, unit: UnitId) {
        let row = hotfix::oath_of_light();
        let label = sim.unit(unit).label.clone();
        sim.register_aura(
            unit,
            AuraConfig {
                label: format!("Oath of Light{label}"),
                action_id: Some(spell_action(row.id)),
                duration: row.duration(),
                ..AuraConfig::default()
            },
        );
        sim.register_spell(
            unit,
            SpellConfig {
                action_id: spell_action(row.id),
                spell_school: row.spell_school(),
                defense_type: row.defense_type_core(),
                proc_mask: ProcMask::EMPTY,
                flags: SpellFlag::APL | SpellFlag::HELPFUL,
                class_spell_mask: masks::OATH_OF_LIGHT,
                max_range: f64::from(row.max_range),
                cost: mana_cost(row),
                cast: gcd_cast(row.gcd(), row.cast_time(), None),
                ..SpellConfig::default()
            },
        );
    }

    /// The `oath_of_light` effect: the spell, its aura, the three bonuses read at level 60 off the
    /// row's points per level, and the Holy Shock crit Boundless Light gives without the Oath.
    pub(super) fn oath_of_light_effect(&self, label: &str) -> Option<Value> {
        if self.new_talent("oath_of_light") == 0 {
            return None;
        }
        let row = hotfix::oath_of_light();
        let bonus = |n| row.effect_n(n).average(CHARACTER_LEVEL);
        let boundless = self.new_talent("boundless_light");
        Some(json!({
            "kind": "oath_of_light", "spell_id": row.id, "aura": format!("Oath of Light{label}"),
            "holy_light": bonus(1), "flash_of_light": bonus(2), "holy_shock": bonus(3),
            "boundless_light_crit": hotfix::at(&hotfix::BOUNDLESS_LIGHT, boundless),
            "holder_share": crate::prepare::party_damage::tank_share(),
        }))
    }
}
