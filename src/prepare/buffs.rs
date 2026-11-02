//! Go sim/core/buffs (meta.go, amounts.go, drivers.go and the generated tables) and
//! sim/core/buffs.go: the raid, party and individual buffs and the raid's debuffs.

pub(crate) mod drivers;
mod effects;
pub(crate) mod flametongue;
pub(crate) mod generated;
pub(crate) mod paladin;
pub(crate) mod pet_buffs;
pub(crate) mod support;
mod windfury_effect;

use crate::contracts::prepared_v2::ActionId;
use crate::contracts::request::Message;
use crate::data::spells::{Effect, Spell};

use super::dbcenums;
use super::env::Environment;
use super::parse_effects::{dry_run, parse_effects, ParseOptions};
use super::sim::{
    AuraConfig, AuraId, BuildPhase, Duration, Rules, Sim, UnitId, MILLISECOND, NEVER_EXPIRES,
};
use super::spelldata::{must_find, Ladder};
use super::Refusal;

pub(crate) use effects::{
    aura_should_refresh_effects, battle_shout_effect, exclusive_refresh,
    external_expose_armor_effect, judgement_of_light_effects, judgement_of_wisdom_effects,
    sunder_armor_effect,
};
pub(crate) use generated::{apply_generated_buffs, apply_generated_debuffs, GIFT_OF_ARTHAS};
pub(crate) use windfury_effect::{windfury_totem_effect, windfury_trigger_spells};

/// Which constructor a generated buff uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MetaKind {
    Buff,
    Debuff,
    ItemCountBuff,
    DamageShield,
}

/// Go `buffs.Meta`: the client row a buff reads and how it bids.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Meta {
    pub kind: MetaKind,
    pub label: &'static str,
    /// The paladin rank the label names (Go `paladinRankName`); 0 names none.
    pub rank: i32,
    pub spell: i32,
    pub cast: Option<i32>,
    pub category: &'static str,
    pub shared_category: &'static str,
    pub single_aura: bool,
    pub per_stat: bool,
    /// The improving talent: its spell and maximum ranks, Go `spelldata.Talent`.
    pub talent: Option<(i32, i32)>,
    pub talent_effect: i32,
    pub talent_scales_duration: bool,
    pub skip_auras: &'static [i32],
    pub full_combo_points: bool,
}

impl Meta {
    pub(crate) const DEFAULT: Meta = Meta {
        kind: MetaKind::Buff,
        label: "",
        rank: 0,
        spell: 0,
        cast: None,
        category: "",
        shared_category: "",
        single_aura: false,
        per_stat: false,
        talent: None,
        talent_effect: 0,
        talent_scales_duration: false,
        skip_auras: &[],
        full_combo_points: false,
    };

    fn row(&self) -> &'static Spell {
        must_find(self.spell)
    }

    /// The talent's modifier at a rank: `talentMod`. `None` is Go's NilEffect.
    fn talent_mod(&self, talent_points: i32) -> Option<&'static Effect> {
        let (spell, ranks) = self.talent?;
        let effect = Ladder::talent(spell, ranks)
            .rank(talent_points)
            .effect_n(self.talent_effect);
        (!effect.is_nil()).then_some(effect)
    }

    /// Go `Meta.Options`.
    pub(crate) fn options(&self, talent_points: i32) -> ParseOptions {
        let mut options = ParseOptions::raid_buff(self.skip_auras, self.full_combo_points);
        if !self.talent_scales_duration {
            options.scale = self.talent_mod(talent_points);
        }
        options
    }

    /// Go `Meta.Value`.
    pub(crate) fn value(&self, talent_points: i32) -> f64 {
        let row = self.row();
        for effect in &row.effects {
            if effect.aura == dbcenums::A_DAMAGE_SHIELD {
                let mut value = amount(effect);
                if let Some(talent) = self.talent_mod(talent_points) {
                    if !self.talent_scales_duration {
                        value = super::parse_effects::scaled(value, talent);
                    }
                }
                return value;
            }
        }
        dry_run(row, self.options(talent_points))
            .applied
            .first()
            .map_or(0.0, |applied| applied.value)
    }

    /// Go `Meta.Duration`.
    pub(crate) fn duration(&self, talent_points: i32) -> Duration {
        if self.talent_scales_duration {
            let row = self.row();
            if row.duration_ms <= 0 {
                return NEVER_EXPIRES;
            }
            let modifier = self.talent_mod(talent_points);
            let ms = match modifier {
                Some(modifier) => {
                    super::parse_effects::scaled(f64::from(row.duration_ms), modifier)
                }
                None => f64::from(row.duration_ms),
            };
            return (ms as Duration) * MILLISECOND;
        }
        if self.row().duration_ms <= 0 {
            if let Some(cast) = self.cast {
                return aura_duration(must_find(cast));
            }
        }
        aura_duration(self.row())
    }

    /// Go `Meta.Cooldown`.
    pub(crate) fn cooldown(&self) -> Duration {
        let row = self.cast.map_or(self.row(), must_find);
        row.cooldown().max(row.category_cooldown())
    }

    /// Go `Meta.label`: the label, with the rank a paladin cast it at, and who cast it.
    fn label_for(&self, is_player: bool) -> String {
        format!(
            "{} ({})",
            paladin::rank_name(self.label, self.rank),
            if is_player { "Player" } else { "External" }
        )
    }

    fn action_id(&self, is_player: bool) -> ActionId {
        ActionId {
            spell_id: self.spell,
            tag: if is_player { 0 } else { -1 },
            ..ActionId::default()
        }
    }

    /// Go `newBuff`, `newDebuff`, `newItemCountBuff` or `newDamageShield`, as the generated
    /// constructor of this buff calls it.
    pub(crate) fn aura(
        &self,
        sim: &mut Sim,
        unit: UnitId,
        is_player: bool,
        talent_points: i32,
        count: f64,
    ) -> AuraId {
        if sim.rules == Rules::Forever {
            if let Some(spell) = forever_buff_rank(self.spell) {
                let meta = Meta { spell, ..*self };
                return meta.aura(sim, unit, is_player, talent_points, count);
            }
        }
        match self.kind {
            MetaKind::Buff => self.buff(
                sim,
                unit,
                is_player,
                talent_points,
                self.options(talent_points),
            ),
            MetaKind::ItemCountBuff => {
                let mut options = self.options(0);
                options.count = count;
                self.buff(sim, unit, is_player, 0, options)
            }
            MetaKind::Debuff => {
                let mut options = self.options(talent_points);
                if sim.rules == Rules::Forever {
                    options.amount_as = forever_amounts(self.spell);
                }
                if !self.category.is_empty() {
                    if self.per_stat {
                        options.per_stat_category = Some(self.category.to_string());
                    } else {
                        options.exclusive = Some((self.category.to_string(), self.single_aura));
                    }
                }
                self.parsed_aura(
                    sim,
                    unit,
                    is_player,
                    talent_points,
                    BuildPhase::NONE,
                    options,
                )
            }
            MetaKind::DamageShield => self.damage_shield(sim, unit, is_player, talent_points),
        }
    }

    /// Go `newDamageShield`: `NewDamageShield` with the spell's school and `Value` as its damage.
    /// Thorns scales with the druid's spell power at 6% though its row carries no coefficient
    /// (Go `ThornsSpellPowerCoefficient`, beta notes of 24 September and 8 October 2026); only the
    /// druid's own copy carries it.
    fn damage_shield(
        &self,
        sim: &mut Sim,
        unit: UnitId,
        is_player: bool,
        talent_points: i32,
    ) -> AuraId {
        let aura = support::new_damage_shield(
            sim,
            unit,
            support::DamageShield {
                label: self.label_for(is_player),
                action_id: self.action_id(is_player),
                duration: self.duration(talent_points),
                category: self.category,
                single_aura: self.single_aura,
                school: self.row().spell_school(),
                damage: self.value(talent_points),
                bonus_coefficient: if is_player && std::ptr::eq(self, &generated::THORNS) {
                    0.06
                } else {
                    0.0
                },
            },
        );
        support::join_shared_category(sim, aura, self.shared_category, is_player);
        aura
    }

    pub(crate) fn buff(
        &self,
        sim: &mut Sim,
        unit: UnitId,
        is_player: bool,
        talent_points: i32,
        mut options: ParseOptions,
    ) -> AuraId {
        options.school_resistances = true;
        if !self.category.is_empty() {
            if self.single_aura && !self.per_stat {
                options.exclusive = Some((self.category.to_string(), true));
            } else {
                options.per_stat_category = Some(self.category.to_string());
            }
        }
        let phase = if is_player {
            BuildPhase::NONE
        } else {
            BuildPhase::BUFFS
        };
        self.parsed_aura(sim, unit, is_player, talent_points, phase, options)
    }

    fn parsed_aura(
        &self,
        sim: &mut Sim,
        unit: UnitId,
        is_player: bool,
        talent_points: i32,
        phase: BuildPhase,
        options: ParseOptions,
    ) -> AuraId {
        let aura = sim.get_or_register_aura(
            unit,
            AuraConfig {
                label: self.label_for(is_player),
                tag: self.category.to_string(),
                action_id: Some(self.action_id(is_player)),
                duration: self.duration(talent_points),
                max_stacks: i32::from(self.row().max_stack),
                build_phase: phase,
                ..Default::default()
            },
        );
        parse_effects(sim, None, aura, self.row(), options);
        support::join_shared_category(sim, aura, self.shared_category, is_player);
        aura
    }
}

/// Go `amount`: the caster's amount at the character level.
fn amount(effect: &Effect) -> f64 {
    effect.average(super::character::constants::CHARACTER_LEVEL)
}

/// Go `auraDuration`.
fn aura_duration(row: &Spell) -> Duration {
    if row.duration_ms <= 0 {
        NEVER_EXPIRES
    } else {
        row.duration()
    }
}

/// Go `core.GetTristateValueInt32`.
pub(crate) fn tristate(effect: i32, regular: i32, improved: i32) -> i32 {
    match effect {
        1 => regular,
        2 => improved,
        _ => 0,
    }
}

/// Rows the frozen spell data (client 1.60.1.70425) states differently from Forever's current
/// client, as (effect position, amount) pairs a debuff reads under Forever's rules. The Go
/// reference keeps the data's amounts.
///
/// Client 1.60.1.70630 raises Hunter's Mark rank 4 (14325, SpellEffect 690991, aura 127: ranged
/// Attack Power for every attacker) from 71 to 143.
fn forever_amounts(spell: i32) -> Vec<(i32, f64)> {
    match spell {
        14325 => vec![(2, 143.0)],
        _ => Vec::new(),
    }
}

/// Buffs whose reference row is not the rank a level 60 caster has under Forever's rules, as the
/// row to read instead. The Go reference keeps its row.
///
/// Trueshot Aura: the reference reads rank 4 (20905, level 50). Its 75 ranged Attack Power was
/// above rank 5's (20906, level 60, 50) until client 1.60.1.70707 lowered ranks 1 to 4 to 15, 22,
/// 30 and 40 (SpellEffect 698474 for rank 4), so a level 60 Hunter's aura is rank 5's 50.
fn forever_buff_rank(spell: i32) -> Option<i32> {
    match spell {
        20905 => Some(20906),
        // Battle Shout VII, Strength of Earth V and Grace of Air III come from Ahn'Qiraj 20
        // books, so the raid's shout and totems are the rank below.
        _ => {
            let known = super::spelldata::book_ranks::known_rank(Rules::Forever, spell);
            (known != spell).then_some(known)
        }
    }
}

/// `core.MakePermanent(<Buff>Aura(unit, isPlayer, talentPoints))`.
pub(crate) fn permanent(
    env: &mut Environment,
    unit: UnitId,
    meta: &Meta,
    is_player: bool,
    talent_points: i32,
) -> Result<AuraId, Refusal> {
    let aura = meta.aura(&mut env.sim, unit, is_player, talent_points, 0.0);
    Ok(env.sim.make_permanent(aura))
}

/// `core.MakePermanent` of a buff worth its amounts once per item in the party, such as Atiesh.
pub(crate) fn permanent_with_count(
    env: &mut Environment,
    unit: UnitId,
    meta: &Meta,
    count: f64,
) -> Result<AuraId, Refusal> {
    let aura = meta.aura(&mut env.sim, unit, false, 0, count);
    Ok(env.sim.make_permanent(aura))
}

/// Go `applyBuffEffects`.
pub(crate) fn apply_buff_effects(
    env: &mut Environment,
    raid_buffs: &Message,
    party_buffs: &Message,
    individual: &Message,
) -> Result<(), Refusal> {
    let unit = env.player;
    if env.sim.rules == Rules::Forever {
        // Hotfix push 113554 removes Greater Blessing of Salvation (25895) and Greater Blessing of
        // Light (25890) from the game, so under Forever's rules saved settings that still ask for
        // them get nothing, as wowsims/forever #770 keeps the fields as inert flags.
        let mut individual = individual.clone();
        for removed in ["greater_blessing_of_salvation", "greater_blessing_of_light"] {
            individual.set_bool(removed, false);
        }
        return apply_generated_buffs(env, unit, raid_buffs, party_buffs, &individual);
    }
    apply_generated_buffs(env, unit, raid_buffs, party_buffs, individual)
}

#[cfg(test)]
mod tests;
