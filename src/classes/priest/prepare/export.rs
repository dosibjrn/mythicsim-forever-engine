//! The exporter's Priest part, tools/oracle-v2/priest.go: the client damage rows of the
//! Priest's spells (`priestDamageRows`) and the effects whose parameters Go keeps in closures
//! (`priestEffects`), each noting what it cannot describe as unrepresented (`classNotes`).

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::{json, Value};

use crate::contracts::prepared_v2::ActionId;
use crate::prepare::aura_helpers::{CallbackMask, ProcTrigger};
use crate::prepare::character::constants::CHARACTER_LEVEL;
use crate::prepare::consumable_effects::proc_trigger_spells;
use crate::prepare::dbcenums;
use crate::prepare::env::Environment;
use crate::prepare::export::action_id;
use crate::prepare::export_items::{callback_names, outcome_names};
use crate::prepare::pet::apply_dependencies;
use crate::prepare::sim::{Rules, Sim, SpellId};
use crate::prepare::spell::{school, SpellFlag};
use crate::prepare::spelldata::{Ladder, Spell as Row};
use crate::prepare::stats::{Stat, Stats};

use super::masks;
use super::spell_data::spell_data;
use super::Priest;

/// `priestDamageRows`: the rows by spell ID.
fn damage_rows() -> &'static HashMap<i32, &'static Row> {
    static ROWS: OnceLock<HashMap<i32, &'static Row>> = OnceLock::new();
    ROWS.get_or_init(|| {
        let data = spell_data();
        let mut rows: HashMap<i32, &'static Row> = HashMap::new();
        // mind_blast.go, shadow_word_death.go, smite.go, holy_fire.go and talents_holy.go
        // registerHolyNovaSpell: every rank rolls its own row.
        for ladder in [
            &data.mind_blast,
            &data.shadow_word_death,
            &data.smite,
            &data.holy_fire,
            &data.holy_nova,
        ] {
            ladder.each(|_, row| {
                rows.insert(row.id, row);
            });
        }
        rows
    })
}

/// Go `attachDamageEffects` for one spell: the roll of the client row, `{average, variance}`.
pub(super) fn damage_effect(sim: &Sim, spell: SpellId) -> Option<Value> {
    let action = &sim.spell(spell).action_id;
    if action.spell_id == 0 || action.tag != 0 {
        return None;
    }
    let row = match damage_rows().get(&action.spell_id) {
        Some(row) => *row,
        // chastise.go, under Forever's rules (Go's exporter leaves the cast unknown): every
        // rank rolls its own row.
        None if sim.rules == crate::prepare::sim::Rules::Forever => {
            let mut found = None;
            spell_data().chastise.each(|_, row| {
                if row.id == action.spell_id {
                    found = Some(row);
                }
            });
            found?
        }
        None => return None,
    };
    let effect = row.damage_effect();
    if effect.is_nil() {
        return None;
    }
    Some(json!({"average": effect.average(CHARACTER_LEVEL), "variance": effect.variance}))
}

/// `priestDotRank`: the base a priest dot rank's ticks snapshot and whether `priestTickOutcome`
/// rolls a crit, which reads only the client's Periodic Can Crit attribute.
fn dot_rank(row: &Row) -> Value {
    json!({
        "spell_id": row.id,
        "tick_base": row.periodic_effect().average(CHARACTER_LEVEL),
        "tick_can_crit": row.periodic_can_crit(),
    })
}

fn dot_ranks(ladder: &Ladder) -> Vec<Value> {
    let mut ranks = Vec::new();
    ladder.each(|_, row| ranks.push(dot_rank(row)));
    ranks
}

/// `spellsMatching`: the spellbook positions of the spells with a class mask bit in common.
fn spells_matching(sim: &Sim, unit: crate::prepare::sim::UnitId, mask: i64) -> Vec<usize> {
    sim.unit(unit)
        .spellbook
        .iter()
        .enumerate()
        .filter(|(_, spell)| sim.spell(**spell).matches(mask))
        .map(|(position, _)| position)
        .collect()
}

/// The spellbook positions of the healer's heals (the heal mask) that a client effect's class
/// flags name.
fn heals_named(
    sim: &Sim,
    unit: crate::prepare::sim::UnitId,
    flags: &crate::data::spells::ClassFlags,
) -> Vec<usize> {
    sim.unit(unit)
        .spellbook
        .iter()
        .enumerate()
        .filter(|(_, spell)| {
            let spell = sim.spell(**spell);
            spell.matches(masks::HEAL)
                && crate::prepare::spell_mod::class_flags_matches(flags, &spell.class_flags)
        })
        .map(|(position, _)| position)
        .collect()
}

/// `priestModSpells`: the spellbook positions a spell modifier with a class mask and school
/// applies to, by core `shouldApply` for the fields priest modifiers set.
fn mod_spells(
    sim: &Sim,
    unit: crate::prepare::sim::UnitId,
    mask: i64,
    spell_school: u8,
) -> Vec<usize> {
    let mut positions = Vec::new();
    for (position, spell) in sim.unit(unit).spellbook.iter().enumerate() {
        let spell = sim.spell(*spell);
        if spell.flags.matches(SpellFlag::NO_SPELL_MODS) {
            continue;
        }
        if mask > 0 && !spell.matches(mask) {
            continue;
        }
        if spell_school > 0 && spell.spell_school & spell_school == 0 {
            continue;
        }
        positions.push(position);
    }
    positions
}

/// `priestPowerInfusionEffect`: the priest's own Power Infusion, a cooldown that activates the
/// client-parsed Power Infusions aura, which the shared `power_infusion_effect` describes.
fn power_infusion_effect(env: &Environment, notes: &mut Vec<String>) -> Option<Value> {
    let rank = spell_data().power_infusion.highest();
    crate::prepare::power_infusion::power_infusion_effect(env, &ActionId::spell(rank.id), notes)
}

/// `priestShadowfiendEffect`: the summon enables the pet for its timeline aura's duration, and
/// the pet inherits attack power from the priest's spell damage and shadow damage at each
/// summon. Each enable activates the pet's mana restore aura, whose landed hits give the
/// priest 5% of maximum mana.
fn shadowfiend_effect(priest: &Priest, env: &Environment, notes: &mut Vec<String>) -> Value {
    let mut note = |condition: bool, message: &str| {
        if condition {
            notes.push(message.to_string());
        }
    };
    let pet = priest.shadowfiend_pet;
    let rank = spell_data().shadowfiend.highest();
    // The inheritance: attack power from the sum of spell damage and shadow damage, bit for bit.
    let inherit = std::rc::Rc::clone(&env.sim.pet_data(pet).stat_inheritance);
    let mut probe = env.fresh();
    let mut inherit = |stats: &Stats| inherit(&mut probe.sim, stats);
    let mut unit_change = Stats::default();
    unit_change[Stat::SpellDamage] = 1.0;
    let coefficient = inherit(&unit_change)[Stat::AttackPower];
    for (spell_damage, shadow_damage) in [
        (1.0, 0.0),
        (0.0, 1.0),
        (3.7, 12.25),
        (123.456, 0.1),
        (812.3, 47.9),
        (-12.5, 3.0),
    ] {
        let mut change = Stats::default();
        change[Stat::SpellDamage] = spell_damage;
        change[Stat::ShadowDamage] = shadow_damage;
        let got = inherit(&change);
        let mut want = Stats::default();
        want[Stat::AttackPower] = (spell_damage + shadow_damage) * coefficient;
        note(
            got != want,
            "the Shadowfiend's stat inheritance is not attack power from spell and shadow damage",
        );
    }
    // The stats at a summon from a separate reset simulation, and the inherited stats line.
    let mut fresh = env.fresh();
    let reset_priest = fresh.player;
    let reset_pet = fresh.sim.unit(reset_priest).pets[0];
    let before = fresh.sim.unit(reset_pet).stats;
    let reset_without = fresh.sim.unit(reset_pet).stats_without_deps;
    // Each attack power dependency adds a term that inheritance never changes.
    let mut terms: Vec<f64> = Vec::new();
    let mut s = reset_without;
    for (src, dst, amount, step) in fresh.sim.unit(reset_pet).sdm.enabled_dependencies() {
        note(
            src == Stat::AttackPower || (src == dst && dst == Stat::AttackPower),
            "the Shadowfiend's attack power feeds or scales a dependency",
        );
        if dst == Stat::AttackPower {
            // Go adds the term to the attack power before it, as ApplyStatDependencies does.
            let term = if step != 0.0 {
                (s[src] / step).floor() * step * amount
            } else if matches!(
                src,
                Stat::Strength | Stat::Agility | Stat::Stamina | Stat::Intellect | Stat::Spirit
            ) {
                s[src].floor() * amount
            } else {
                s[src] * amount
            };
            terms.push(term);
        }
        s = apply_dependencies(s, &[(src, dst, amount, step)]);
    }
    let owner = fresh.sim.unit(reset_priest).stats;
    fresh.sim.enable_pet(reset_pet);
    let after = fresh.sim.unit(reset_pet).stats;
    // The inheritance Go stored, which Rust computes as the closure does.
    let inherited = fresh.sim.pet_data(reset_pet).inherited_stats[Stat::AttackPower];
    note(
        inherited != (owner[Stat::SpellDamage] + owner[Stat::ShadowDamage]) * coefficient,
        "the Shadowfiend's inheritance at the summon is not attack power from spell and shadow damage",
    );
    let fold = |mut base: f64| {
        for term in &terms {
            base += *term;
        }
        base
    };
    note(
        fold(reset_without[Stat::AttackPower]) != before[Stat::AttackPower],
        "the Shadowfiend's attack power does not follow its dependencies",
    );
    note(
        fold(reset_without[Stat::AttackPower] + inherited) != after[Stat::AttackPower],
        "the Shadowfiend's summoned attack power does not follow its inheritance",
    );
    for stat in Stat::ALL {
        if stat != Stat::AttackPower && before[stat] != after[stat] {
            notes.push(format!("the Shadowfiend's summon changes {}", stat.name()));
        }
    }
    let mut probe = Stats::default();
    probe[Stat::AttackPower] = 123.25;
    let mut want = Stats::default();
    want[Stat::AttackPower] = 123.25;
    if fresh.sim.unit(reset_pet).sdm.apply_stat_dependencies(probe) != want {
        notes.push("the Shadowfiend's inherited stats line is not its attack power".to_string());
    }
    // The pet's stats in Go's order, which its summon and dismissal lines print.
    let order: Vec<Value> = Stat::ALL
        .into_iter()
        .filter(|stat| before[*stat] != 0.0 || *stat == Stat::AttackPower)
        .map(|stat| json!({"stat": stat.name(), "value": before[stat]}))
        .collect();
    // The stats the dismissal at the fight's end prints: Go's doneIteration expires the pet's
    // permanent auras before it dismisses the pet, so a party aura on the pet (patch 131), such
    // as Mana Spring Totem, has left the line then, from a separate reset simulation.
    let mut fade = env.fresh();
    let fade_pet = fade.sim.unit(fade.player).pets[0];
    for aura in fade.sim.unit(fade_pet).auras.clone() {
        let state = fade.sim.aura(aura);
        if state.active && state.duration == crate::prepare::sim::NEVER_EXPIRES {
            fade.sim.deactivate(aura);
        }
    }
    let faded = fade.sim.unit(fade_pet).stats;
    if faded[Stat::AttackPower] != before[Stat::AttackPower] {
        notes.push("the Shadowfiend's permanent auras change its attack power".to_string());
    }
    let mut fight_end = Vec::new();
    for stat in Stat::ALL {
        if before[stat] != 0.0 || stat == Stat::AttackPower {
            fight_end.push(json!({"stat": stat.name(), "value": faded[stat]}));
        } else if faded[stat] != 0.0 {
            notes.push(format!(
                "the Shadowfiend's permanent auras' expiry adds {}",
                stat.name()
            ));
        }
    }
    let restore = env.sim.get_aura(pet, "Shadowfiend Mana Restore");
    if restore.is_none() {
        notes.push("the Shadowfiend has no mana restore aura".to_string());
    }
    let label = restore.map_or_else(String::new, |aura| env.sim.aura(aura).label.clone());
    let aura = priest
        .shadowfiend_aura
        .map(|aura| env.sim.aura(aura))
        .expect("the summon registered its aura");
    let mut effect = json!({
        "kind": "shadowfiend", "spell_id": rank.id, "aura": aura.label,
        "duration_ns": aura.duration, "pet": env.sim.unit(pet).label,
        "attack_power_coefficient": coefficient,
        "attack_power_without_deps": reset_without[Stat::AttackPower],
        "attack_power_dependency_terms": terms,
        "stats": order, "mana_restore_aura": label, "mana_restore_fraction": 0.05,
        "mana_restore_action_id": 401988,
    });
    if faded != before {
        effect["fight_end_stats"] = Value::Array(fight_end);
    }
    effect
}

/// `priestEffects`.
pub(super) fn effects(priest: &Priest, env: &Environment, notes: &mut Vec<String>) -> Vec<Value> {
    let sim = &env.sim;
    let player = env.player;
    let talents = &priest.talents;
    let data = spell_data();
    let mut effects: Vec<Value> = Vec::new();

    // MythicSim's healer sims (Rust only): the healer's heals.
    if priest.healer {
        effects.push(crate::prepare::heals::heals_effect(env, &priest.heals));
        effects.extend(priest.heal_talents(&env.sim));
    }

    // mind_blast.go: a direct hit on the rank's own row.
    effects.push(json!({"kind": "mind_blast"}));
    // shadow_word_death.go: Early Demise adds crit inside the 20% execute phase.
    effects.push(json!({
        "kind": "shadow_word_death",
        "early_demise_crit": data.early_demise.effect_at(1).value_at(talents.i32("early_demise")),
    }));
    // shadow_word_pain.go: a hit roll, then a snapshotting dot.
    effects.push(json!({"kind": "shadow_word_pain", "ranks": dot_ranks(&data.shadow_word_pain)}));
    // devouring_plague.go: Shadow Word: Pain's shape; each tick heals the priest for its damage.
    effects.push(json!({
        "kind": "devouring_plague", "ranks": dot_ranks(&data.devouring_plague),
        "heal_metrics_tag": 1,
    }));
    // talents_holy.go registerHolyNovaSpell: a rolled hit on each target, then the triggered heal
    // on each player of the priest's party, with the heal's own crit roll.
    if talents.bool("holy_nova") {
        let mut ranks = Vec::new();
        data.holy_nova.each(|_, row| {
            let heal = data.holy_nova_triggered.rank(row.rank_number());
            ranks.push(json!({
                "spell_id": row.id, "heal_spell_id": heal.id,
                "heal_base": heal.heal_effect().average(CHARACTER_LEVEL),
            }));
        });
        // The party is the priest alone: Rust prepares one player.
        let unit = sim.unit(player);
        effects.push(json!({
            "kind": "holy_nova", "ranks": ranks,
            "healing_dealt_multiplier": unit.pseudo_stats.healing_dealt_multiplier,
            "healing_taken_multiplier": unit.pseudo_stats.healing_taken_multiplier,
            "table_healing_dealt_multiplier": env.attack_table(player, player).healing_dealt_multiplier,
            "healing_power": unit.stats[Stat::HealingPower] + unit.pseudo_stats.bonus_healing_taken,
        }));
    }
    if talents.bool("power_infusion") {
        if let Some(effect) = power_infusion_effect(env, notes) {
            effects.push(effect);
        }
    }
    if priest.shadowfiend.is_some() {
        effects.push(shadowfiend_effect(priest, env, notes));
    }
    // starshards.go, the Night Elf priest's racial: a hit roll, then a snapshotting channel.
    if !spells_matching(sim, player, masks::STARSHARDS).is_empty() {
        effects.push(json!({"kind": "starshards", "ranks": dot_ranks(&data.starshards)}));
    }
    if talents.bool("mind_flay") {
        // talents_shadow.go registerMindFlaySpell: a binary hit roll, then a channel
        effects.push(json!({"kind": "mind_flay", "ranks": dot_ranks(&data.mind_flay)}));
        // Improved Mind Flay (client 70425): each tick deals its share more than the one before.
        // The tooltip, "Each instance of damage of Mind Flay deals 20% more damage than its
        // previous instance of damage", compounds tick over tick (1, 1.2, 1.44 at two points);
        // the Go reference adds the step per tick (1, 1.2, 1.4). Compounding is Rust only.
        let ramp = talents.i32("improved_mind_flay");
        if ramp > 0 {
            let mut effect = json!({
                "kind": "improved_mind_flay",
                "ramp_per_tick": data.improved_mind_flay.fraction_at(ramp),
            });
            if sim.rules == Rules::Forever {
                effect["compounding"] = json!(true);
            }
            effects.push(effect);
        }
    }
    if talents.i32("dark_inspiration") > 0 {
        // talents_shadow.go applyDarkInspiration: a Spirit stat aura from Mind Blast and a full
        // Mind Flay channel.
        effects.push(json!({
            "kind": "dark_inspiration", "aura": "Dark Inspiration",
            "mind_blast_trigger": "Dark Inspiration Mind Blast",
            "mind_flay_trigger": "Dark Inspiration Mind Flay",
        }));
    }
    if let Some(aura) = priest.shadowform_aura {
        // talents_shadow.go applyShadowform. A helpful Holy cast no longer ends it; its form
        // refuses Holy Nova and Chastise through their cast requirements.
        let rank = data.shadowform.highest();
        effects.push(json!({
            "kind": "shadowform", "spell_id": rank.id, "aura": sim.aura(aura).label,
            "damage_percent": rank.effect(dbcenums::A_MOD_DAMAGE_PERCENT_DONE, 32).average(CHARACTER_LEVEL) / 100.0,
            "cost_percent": rank.effect(dbcenums::A_MOD_POWER_COST_SCHOOL_PCT, 32).average(CHARACTER_LEVEL) / 100.0,
            "crit_multiplier": rank
                .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_CRIT_DAMAGE_BONUS)
                .average(CHARACTER_LEVEL) / 100.0,
            "school_spells": mod_spells(sim, player, masks::ALL, school::SHADOW),
            "crit_spells": mod_spells(
                sim,
                player,
                masks::MIND_BLAST | masks::MIND_FLAY | masks::SHADOW_WORD_PAIN
                    | masks::DEVOURING_PLAGUE | masks::SHADOW_WORD_DEATH,
                0,
            ),
        }));
    }
    if let Some(aura) = priest.inner_focus_aura {
        // talents_discipline.go applyInnerFocus
        let rank = data.inner_focus.highest();
        let cost = rank.effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_COST);
        let crit = rank.effect(
            dbcenums::A_ADD_FLAT_MODIFIER,
            dbcenums::SPELLMOD_CRITICAL_CHANCE,
        );
        // The healer's heals (Rust only) carry the heal mask, outside Go's masks::ALL. The
        // client's masks on Inner Focus's two effects name them: every heal spends its one
        // charge, and the direct heals take its crit (Renew and Power Word: Shield do not).
        // Without them a healer's Inner Focus never ended and made every heal free.
        let mut spenders = spells_matching(sim, player, masks::ALL);
        spenders.extend(heals_named(sim, player, &cost.class_flags));
        let mut crit_spells = mod_spells(
            sim,
            player,
            masks::ALL
                & !(masks::SHADOW_WORD_DEATH | masks::DEVOURING_PLAGUE | masks::SHADOW_WORD_PAIN),
            0,
        );
        crit_spells.extend(heals_named(sim, player, &crit.class_flags));
        effects.push(json!({
            "kind": "inner_focus", "spell_id": rank.id, "aura": sim.aura(aura).label,
            "cost_percent": cost.average(CHARACTER_LEVEL) as i32,
            "crit_percent": crit.average(CHARACTER_LEVEL),
            "crit_spells": crit_spells,
            "spender_spells": spenders,
        }));
    }
    if let Some(aura) = priest.shadow_weaving_aura {
        // talents_shadow.go applyShadowWeaving
        let stack = data.shadow_weaving_triggered.highest();
        let trigger = ProcTrigger {
            name: "Shadow Weaving Trigger".to_string(),
            callback: CallbackMask::ON_SPELL_HIT_DEALT,
            class_spell_mask: masks::SHADOW_SPELLS,
            outcome: crate::prepare::aura_helpers::HitOutcome::LANDED,
            proc_chance: data
                .shadow_weaving
                .fraction_at(talents.i32("shadow_weaving")),
            trigger_immediately: true,
            ..ProcTrigger::default()
        };
        effects.push(json!({
            "kind": "shadow_weaving", "trigger_aura": trigger.name, "aura": sim.aura(aura).label,
            "callbacks": callback_names(trigger.callback), "outcome": outcome_names(trigger.outcome),
            "trigger_immediately": trigger.trigger_immediately, "proc_chance": trigger.proc_chance,
            "trigger_spells": proc_trigger_spells(env, &trigger),
            // The debuff on each target, multiplying this priest's Shadow damage on it.
            "damage_per_stack": stack
                .effect(dbcenums::A_MOD_SCHOOL_MASK_DAMAGE_FROM_CASTER, 32)
                .average(CHARACTER_LEVEL) / 100.0,
        }));
    }
    // talents_shadow.go applyVampiricEmbrace, under Forever's rules (Go's exporter leaves the
    // cast unknown): the debuff on each target, the first's copied to the others, heals the
    // priest for a share of the Shadow damage the priest deals that target. Its party share is
    // the player's alone.
    if talents.bool("vampiric_embrace") && sim.rules == Rules::Forever {
        let rank = data.vampiric_embrace.highest();
        let target = env.encounter.targets[0];
        let aura = sim
            .unit(target)
            .auras
            .iter()
            .map(|&aura| sim.aura(aura))
            .find(|aura| aura.label.starts_with("Vampiric Embrace - "));
        if let Some(aura) = aura {
            effects.push(json!({
                "kind": "vampiric_embrace", "spell_id": rank.id, "aura": aura.label,
                "heal_fraction": rank.effect(dbcenums::A_DUMMY, 0).average(CHARACTER_LEVEL) / 100.0,
                "metrics_action_id": action_id(Some(&ActionId::spell(rank.id))),
            }));
        }
    }
    // smite.go: a cast on the rank's own row.
    effects.push(json!({"kind": "smite"}));
    // holy_fire.go: the hit rolls, a landed hit applies the snapshotting dot, then it is dealt.
    effects.push(json!({"kind": "holy_fire", "ranks": dot_ranks(&data.holy_fire)}));
    // penance.go: every rank, on the shared category cooldown, fires its own damage bolt, the
    // first the cast's tooltip names, on application and a channel tick a second.
    if talents.bool("penance") && !priest.healer {
        let mut ranks = Vec::new();
        data.penance.each(|_, rank| {
            let bolt = rank.refs()[0];
            ranks.push(json!({
                "spell_id": rank.id,
                "tick_base": bolt.damage_effect().average(CHARACTER_LEVEL), "tick_can_crit": true,
            }));
        });
        effects.push(json!({"kind": "penance", "ranks": ranks}));
    }
    let power_in_light = talents.i32("power_in_light");
    if power_in_light > 0 {
        // talents_discipline.go applyPowerInLight
        // Holy Fire, and since the 16 October 2026 beta notes Shadow Word: Pain.
        let dots: Vec<usize> = sim
            .unit(player)
            .spellbook
            .iter()
            .enumerate()
            .filter(|(_, spell)| {
                priest.holy_fire.contains(spell) || priest.shadow_word_pain.contains(spell)
            })
            .map(|(position, _)| position)
            .collect();
        effects.push(json!({
            "kind": "power_in_light",
            "multiplier": data.power_in_light.multiplier_at(power_in_light),
            "spells": spells_matching(sim, player, masks::SMITE | masks::PENANCE),
            "dot_spells": dots,
        }));
    }
    if let Some(aura) = priest.searing_light_aura {
        // talents_holy.go applySearingLight
        let free = data.searing_light_triggered.highest();
        let trigger = ProcTrigger {
            name: "Searing Light Trigger".to_string(),
            callback: CallbackMask::ON_PERIODIC_DAMAGE_DEALT,
            class_spell_mask: masks::HOLY_FIRE,
            proc_chance: data
                .searing_light
                .effect_at(2)
                .fraction_at(talents.i32("searing_light")),
            trigger_immediately: true,
            ..ProcTrigger::default()
        };
        effects.push(json!({
            "kind": "searing_light", "trigger_aura": trigger.name, "aura": sim.aura(aura).label,
            "callbacks": callback_names(trigger.callback), "outcome": outcome_names(trigger.outcome),
            "trigger_immediately": trigger.trigger_immediately, "proc_chance": trigger.proc_chance,
            "trigger_spells": proc_trigger_spells(env, &trigger),
            "cost_percent_add": free
                .effect(dbcenums::A_ADD_PCT_MODIFIER, dbcenums::SPELLMOD_COST)
                .average(CHARACTER_LEVEL) / 100.0,
            "cost_spells": mod_spells(sim, player, masks::HOLY_NOVA, 0),
            "cancel_spells": spells_matching(sim, player, masks::HOLY_NOVA),
        }));
    }
    // dark_sacrifice.go: the Undead priest's racial, registered for Undead only. Each tick pays
    // the client base plus a fifth of Spirit; the cooldown manager uses it once the whole gain
    // fits in the mana bar.
    let rank = data.dark_sacrifice.highest();
    if let Some(spell) = sim.get_spell(player, &ActionId::spell(rank.id)) {
        let hot = sim
            .spell(spell)
            .aoe_dot
            .map(|dot| sim.aura(sim.dots[dot.0].aura).label.clone())
            .unwrap_or_default();
        effects.push(json!({
            "kind": "dark_sacrifice", "spell_id": rank.id, "aura": hot,
            "tick_base": rank.proc_energize_effect().average(sim.unit(player).level),
            "spirit_divisor": 5.0, "no_threat": rank.no_threat(),
            "metrics_action_id": action_id(Some(&ActionId::spell(rank.id))),
        }));
    }
    effects
}
