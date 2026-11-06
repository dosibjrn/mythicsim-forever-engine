//! Go `sim/warlock/armors.go`.

use crate::prepare::sim::{AuraConfig, Sim, UnitId, NEVER_EXPIRES};
use crate::prepare::stats::Stat;

use super::spell_data::{prepared_rank, spell_data};
use super::spells::spell_action;
use super::Warlock;

impl Warlock {
    /// The client ships Demon Skin and Demon Armor and no Fel Armor, so the Fel Armor option
    /// buffs nothing. Demonic Aegis raises both halves by 15% a point (1235316).
    pub(super) fn register_armors(&mut self, sim: &mut Sim, unit: UnitId) {
        let data = spell_data();
        let level = sim.unit(unit).level;
        let Some(rank) = prepared_rank(&data.demon_armor, sim, unit) else {
            // A level-limited actor cannot inherit an unlearned L60 armor.
            return;
        };
        let aegis = data
            .demonic_aegis
            .multiplier_at(self.talents.i32("demonic_aegis"));

        let armor_bonus = rank.effect_n(1).average(level) * aegis;
        let shadow_res_bonus = rank.effect_n(2).average(level) * aegis;

        let aura = sim.register_aura(
            unit,
            AuraConfig {
                label: "Demon Armor".to_string(),
                action_id: Some(spell_action(rank.id)),
                duration: NEVER_EXPIRES,
                ..AuraConfig::default()
            },
        );
        sim.attach_stat_buff(aura, Stat::Armor, armor_bonus);
        sim.attach_stat_buff(aura, Stat::ShadowResistance, shadow_res_bonus);

        if self.options.enum_name("armor") == "DemonArmor" {
            sim.make_permanent(aura);
        }
    }
}
