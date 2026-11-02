//! Go sim/druid/talents_restoration.go: the Restoration talents the sim models.

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::dbcenums::{
    A_ADD_PCT_MODIFIER, A_MOD_DAMAGE_PERCENT_DONE, A_MOD_MANA_REGEN_INTERRUPT, A_MOD_THREAT,
    A_MOD_TOTAL_STAT_PERCENTAGE, SPELLMOD_COST,
};
use crate::prepare::sim::{AuraConfig, Sim};
use crate::prepare::spell::school;
use crate::prepare::spell_mod::{SpellModConfig, SpellModType};
use crate::prepare::spelldata::Ladder;
use crate::prepare::stats::Stat;

use super::{masks, Druid};

impl Druid {
    /// Go `registerRestorationTalents`. The healing talents are not modelled.
    pub(super) fn register_restoration_talents(&mut self, sim: &mut Sim) {
        // Tier 1
        self.apply_furor(sim);

        // Tier 2
        self.apply_naturalist(sim);
        self.apply_subtlety(sim);
        self.apply_natural_shapeshifter(sim);

        // Tier 3
        self.apply_reflection(sim);

        // Tier 5
        self.apply_living_spirit(sim);
    }

    fn apply_natural_shapeshifter(&mut self, sim: &mut Sim) {
        if self.tal.natural_shapeshifter == 0 {
            return;
        }
        // Client 16833: the mask covers Cat, Bear and Moonkin Form, and since client 70170
        // Shifting Power.
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                class_mask: masks::CAT_FORM
                    | masks::BEAR_FORM
                    | masks::MOONKIN_FORM
                    | masks::SHIFTING_POWER,
                kind: SpellModType::PowerCostPctAdd,
                float_value: Ladder::talent(16833, 3)
                    .effect(A_ADD_PCT_MODIFIER, SPELLMOD_COST)
                    .fraction_at(self.tal.natural_shapeshifter),
                ..SpellModConfig::default()
            },
        );
    }

    fn apply_naturalist(&mut self, sim: &mut Sim) {
        if self.tal.naturalist == 0 {
            return;
        }
        // Forever states the damage bonus against every school (mask 127), not physical only,
        // and hotfix push 113531 doubles it under Forever's rules (2% a rank).
        let multiplier = super::talents::naturalist(sim.rules)
            .effect(A_MOD_DAMAGE_PERCENT_DONE, 127)
            .multiplier_at(self.tal.naturalist);
        sim.unit_mut(self.unit).pseudo_stats.damage_dealt_multiplier *= multiplier;
    }

    /// Reduces the threat of the Arcane and Nature spells (school mask 72) by 10% a rank.
    fn apply_subtlety(&mut self, sim: &mut Sim) {
        if self.tal.subtlety == 0 {
            return;
        }
        let threat_reduction = Ladder::talent(17118, 3)
            .effect(A_MOD_THREAT, 72)
            .fraction_at(self.tal.subtlety);
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                school: school::ARCANE | school::NATURE,
                kind: SpellModType::ThreatMultiplierPct,
                float_value: threat_reduction,
                ..SpellModConfig::default()
            },
        );
        sim.add_static_mod(
            self.unit,
            SpellModConfig {
                school: school::ARCANE | school::NATURE,
                kind: SpellModType::FlatThreatBonusPct,
                float_value: threat_reduction,
                ..SpellModConfig::default()
            },
        );
    }

    fn apply_living_spirit(&mut self, sim: &mut Sim) {
        if self.tal.living_spirit == 0 {
            return;
        }
        let multiplier = Ladder::talent(1309631, 3)
            .effect(A_MOD_TOTAL_STAT_PERCENTAGE, 0)
            .multiplier_at(self.tal.living_spirit);
        sim.unit_mut(self.unit)
            .sdm
            .multiply_stat(Stat::Spirit, multiplier);
    }

    /// Furor: a chance at 10 Rage when shifting into Bear Form, and Forever's Energy carry-over
    /// on a Cat powershift. The chance is kept here; a permanent aura lets an APL check for
    /// Furor.
    fn apply_furor(&mut self, sim: &mut Sim) {
        if self.tal.furor == 0 {
            return;
        }
        let ladder = Ladder::talent(17056, 5);
        // Both dummy effects carry the same ladder, one per form, so either answers the chance.
        self.furor_proc_chance = ladder.effect_at(1).fraction_at(self.tal.furor);
        let aura = sim.register_aura(
            self.unit,
            AuraConfig {
                label: "Furor".to_string(),
                action_id: Some(ActionId::spell(ladder.highest().id)),
                ..AuraConfig::default()
            },
        );
        sim.make_permanent(aura);
    }

    /// Reflection, new in Forever: a share of Spirit regeneration continues while casting.
    fn apply_reflection(&mut self, sim: &mut Sim) {
        if self.tal.reflection == 0 {
            return;
        }
        let fraction = Ladder::talent(17106, 3)
            .effect(A_MOD_MANA_REGEN_INTERRUPT, 0)
            .fraction_at(self.tal.reflection);
        sim.unit_mut(self.unit)
            .pseudo_stats
            .spirit_regen_rate_casting += fraction;
    }
}
