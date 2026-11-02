//! Go sim/common/shared/shared_utils.go: the constructors that register item and enchant
//! effects from the client's spell rows and the item database's effect entries, and the plumbing
//! they share.
//!
//! Go registers an effect as a closure that runs once per character. Preparation applies the
//! effects of the equipped items only, so each constructor is two functions: `registers`, the
//! decision Go makes when it registers the effect at init, and `apply`, what the closure does
//! to the character.
//!
//! Handlers, extra conditions and item swap callbacks only run in a fight, or with item swapping
//! enabled, which preparation refuses; they are not carried. The stat proc registry
//! (`character.AddStatProcBuff`) only feeds rotation values and is not part of the prepared
//! state either.

use std::rc::Rc;

use crate::contracts::prepared_v2::ActionId;
use crate::contracts::request::Message;
use crate::data::spells::Spell;

use super::aura_helpers::{HitOutcome, ProcTrigger, StackingStatAura, StatBuffAura};
use super::dbcenums;
use super::env::Environment;
use super::item_swap::{ManagerChance, ManagerMask};
use super::items::{self, effect_stats};
use super::procs::DynamicProcManager;
use super::resolve_aura::{aura_config, dot_config, label, AuraOpt};
use super::resolve_proc::{item_proc_chance, proc_trigger, stated_chance, weapon_proc, ProcOpt};
use super::resolve_spell::{self, spell_config, SpellOpt};
use super::sim::{AuraConfig, AuraId, Cooldown, Duration, Sim, SpellId, UnitId};
use super::spell::{school, DefenseType, ProcMask, SpellConfig, SpellFlag};
use super::spelldata::{proc_chance_source, store::must_find};

/// Go `ItemVariant`: one item that carries the effect.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ItemVariant {
    pub item_id: i32,
    pub item_name: &'static str,
}

/// Go `SpellDataProc`: an item or enchant proc as the client's own rows state it.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SpellDataProc {
    /// Filled per variant. An enchant, which has no variants, states its own.
    pub name: &'static str,
    pub item_id: i32,
    /// Set for an enchant, which registers through the enchant registry.
    pub enchant_id: i32,
    /// The spell the item effect names: the one carrying the proc flags, the chance and the ICD.
    pub trigger_spell_id: i32,
    /// The buff the proc applies, where the client makes it a spell of its own.
    pub buff_spell_id: i32,
    /// A "Chance on hit" item effect or a combat enchant.
    pub is_weapon_proc: bool,
    /// A chance on hit the client states no rate for (its trigger's `ProcChance` is 101, the
    /// server's "chance on hit" sentinel, with no procs per minute), which the reference leaves
    /// unregistered. Under Forever's rules it rolls [`CHANCE_ON_HIT_PPM`].
    pub rateless: bool,
}

/// The rate MythicSim assumes for a chance on hit the client states no rate for: Classic servers
/// roll a `ProcChance` of 101 at 1.8% per second of the hitting weapon's speed, which is 1.08
/// procs a minute. No Blizzard source states Forever's; README, Rust-only mechanics.
pub(crate) const CHANCE_ON_HIT_PPM: f64 = 1.08;

impl SpellDataProc {
    /// The zero config the generated registrations fill in.
    pub(crate) const EMPTY: SpellDataProc = SpellDataProc {
        name: "",
        item_id: 0,
        enchant_id: 0,
        trigger_spell_id: 0,
        buff_spell_id: 0,
        is_weapon_proc: false,
        rateless: false,
    };

    /// Go `SpellDataProc.effectSource`.
    pub(crate) fn source(&self) -> EffectSource {
        if self.enchant_id != 0 {
            EffectSource {
                id: self.enchant_id,
                is_enchant: true,
            }
        } else {
            EffectSource {
                id: self.item_id,
                is_enchant: false,
            }
        }
    }

    /// The trigger row and the row the proc applies, the trigger's own where `BuffSpellID` is
    /// zero.
    pub(crate) fn rows(&self) -> (&'static Spell, &'static Spell) {
        let trigger = must_find(self.trigger_spell_id);
        let row = if self.buff_spell_id != 0 {
            must_find(self.buff_spell_id)
        } else {
            trigger
        };
        (trigger, row)
    }
}

/// Go `forEachSpellDataVariant`: the configs a registration expands to, one per variant with its
/// name and item, or the config itself where there are no variants.
pub(crate) fn expand_variants(cfg: SpellDataProc, variants: &[ItemVariant]) -> Vec<SpellDataProc> {
    if variants.is_empty() {
        return vec![cfg];
    }
    variants
        .iter()
        .map(|variant| SpellDataProc {
            name: variant.item_name,
            item_id: variant.item_id,
            ..cfg
        })
        .collect()
}

/// Go `effectSource`: which of the two registries an effect belongs to, item or enchant, and its
/// ID within it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EffectSource {
    pub id: i32,
    pub is_enchant: bool,
}

/// Go `enchantPlacement`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EnchantPlacement {
    Elsewhere,
    OnWeapon,
    InOffHand,
}

impl EffectSource {
    /// Go `effectSource.actionID`.
    pub(crate) fn action_id(self) -> ActionId {
        if self.is_enchant {
            ActionId::spell(self.id)
        } else {
            ActionId::item(self.id)
        }
    }

    /// Go `effectSource.eligibleSlots`: the slots the item or enchant can occupy that carry it,
    /// in either set of an item swap.
    pub(crate) fn eligible_slots(self, sim: &Sim, unit: UnitId) -> Vec<usize> {
        if self.is_enchant {
            return (0..items::NUM_ITEM_SLOTS)
                .filter(|slot| sim.item_swap_holds_enchant(unit, *slot, self.id))
                .collect();
        }
        eligible_slots_for_item(sim, unit, self.id)
    }

    /// Go `effectSource.procEffects` looked up by the aura each applies: the proc-carrying
    /// effect this item or enchant declares for the buff. A later entry for the same buff wins,
    /// as Go's map keeps it.
    pub(crate) fn proc_effect(self, buff_id: i32) -> Option<Message> {
        let declared: Vec<Message> = if self.is_enchant {
            items::enchant(self.id)
                .map(|enchant| enchant.enchant_effects)
                .unwrap_or_default()
        } else {
            items::database_item(self.id)
                .map(|item| item.item_effects)
                .unwrap_or_default()
        };
        declared
            .into_iter()
            .rfind(|effect| effect.has("proc") && effect.i32("buff_id") == buff_id)
    }

    /// Go `effectSource.enchantPlacement`: a shield or held-in-off-hand enchant shares the weapon
    /// type but sits on no weapon.
    pub(crate) fn enchant_placement(self) -> EnchantPlacement {
        if !self.is_enchant {
            return EnchantPlacement::Elsewhere;
        }
        let Some(enchant) = items::enchant(self.id) else {
            return EnchantPlacement::Elsewhere;
        };
        match enchant.item_type.as_str() {
            "ItemTypeRanged" => EnchantPlacement::OnWeapon,
            "ItemTypeWeapon" => {
                if enchant.enchant_type == "EnchantTypeShield"
                    || enchant.enchant_type == "EnchantTypeOffHand"
                {
                    EnchantPlacement::InOffHand
                } else {
                    EnchantPlacement::OnWeapon
                }
            }
            _ => EnchantPlacement::Elsewhere,
        }
    }

    /// Go `effectSource.registerWeaponEnchantBuff`: a weapon enchant's buff drops when the
    /// weapon carrying it is swapped out, and a shield or held-in-off-hand enchant's when its item
    /// leaves the off hand.
    pub(crate) fn register_weapon_enchant_buff(self, env: &mut Environment, aura: AuraId) {
        let unit = env.player;
        match self.enchant_placement() {
            EnchantPlacement::OnWeapon => env.sim.register_weapon_enchant_buff(unit, aura, self.id),
            EnchantPlacement::InOffHand => env.sim.register_enchant_buff_with_slots(
                unit,
                aura,
                self.id,
                vec![items::slot::OFF_HAND],
            ),
            EnchantPlacement::Elsewhere => {}
        }
    }

    /// Go `effectSource.registerTrigger`: the proc trigger aura, which an item swap toggles with
    /// the item or enchant (`registerProc`).
    pub(crate) fn register_trigger(self, env: &mut Environment, config: &ProcTrigger) -> AuraId {
        let unit = env.player;
        let aura = env.sim.make_proc_trigger_aura(unit, config);
        let slots = self.eligible_slots(&env.sim, unit);
        if self.is_enchant {
            env.sim
                .register_enchant_proc_with_slots(unit, self.id, aura, slots);
        } else {
            env.sim
                .register_item_proc_with_slots(unit, self.id, aura, slots);
        }
        aura
    }
}

fn contains_enchant(item: &items::Item, effect_id: i32) -> bool {
    item.enchant.effect_id == effect_id || item.temp_enchant == effect_id
}

/// Go `ItemSwap.EligibleSlotsForItem`: `EligibleSlotsForItem` filtered to the slots that hold
/// the item, in either set of an item swap.
pub(crate) fn eligible_slots_for_item(sim: &Sim, unit: UnitId, item_id: i32) -> Vec<usize> {
    use items::slot;
    let Some(item) = items::database_item(item_id) else {
        return Vec::new();
    };
    let candidates: Vec<usize> = match item.item_type.as_str() {
        "ItemTypeHead" => vec![slot::HEAD],
        "ItemTypeNeck" => vec![slot::NECK],
        "ItemTypeShoulder" => vec![slot::SHOULDER],
        "ItemTypeBack" => vec![slot::BACK],
        "ItemTypeChest" => vec![slot::CHEST],
        "ItemTypeWrist" => vec![slot::WRIST],
        "ItemTypeHands" => vec![slot::HANDS],
        "ItemTypeWaist" => vec![slot::WAIST],
        "ItemTypeLegs" => vec![slot::LEGS],
        "ItemTypeFeet" => vec![slot::FEET],
        "ItemTypeFinger" => vec![slot::FINGER1, slot::FINGER2],
        "ItemTypeTrinket" => vec![slot::TRINKET1, slot::TRINKET2],
        "ItemTypeRanged" => vec![slot::RANGED],
        "ItemTypeWeapon" => match item.hand_type.as_str() {
            "HandTypeTwoHand" | "HandTypeMainHand" => vec![slot::MAIN_HAND],
            "HandTypeOffHand" => vec![slot::OFF_HAND],
            "HandTypeOneHand" => vec![slot::MAIN_HAND, slot::OFF_HAND],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    candidates
        .into_iter()
        .filter(|slot| sim.item_swap_holds_item(unit, *slot, item_id))
        .collect()
}

/// Go `Character.AddStatProcBuff`: the buff joins the item proc buffs, swapped out unless the
/// item, or the enchant, is in one of its slots.
pub(crate) fn add_stat_proc_buff(
    sim: &mut Sim,
    unit: UnitId,
    effect_id: i32,
    mut buff: super::aura_helpers::StatBuffAura,
    is_enchant: bool,
    slots: &[usize],
) {
    let equipped = if is_enchant {
        let equipment = &sim.character(unit).equipment;
        slots
            .iter()
            .any(|slot| contains_enchant(&equipment[*slot], effect_id))
    } else {
        has_item_equipped(sim, unit, effect_id, slots)
    };
    buff.is_swapped = !equipped;
    sim.character_mut(unit).item_proc_buffs.push(buff);
}

/// Go `Character.HasItemEquipped`.
pub(crate) fn has_item_equipped(sim: &Sim, unit: UnitId, item_id: i32, slots: &[usize]) -> bool {
    let equipment = &sim.character(unit).equipment;
    slots.iter().any(|slot| equipment[*slot].id == item_id)
}

/// Go `getCurrentProcMaskFor`: the hands and ranged slot whose item the predicate picks.
pub(crate) fn current_proc_mask_for(
    sim: &Sim,
    unit: UnitId,
    pred: impl Fn(&items::Item) -> bool,
) -> ProcMask {
    let equipment = &sim.character(unit).equipment;
    let mut mask = ProcMask::UNKNOWN;
    if pred(&equipment[items::slot::RANGED]) {
        mask = mask | ProcMask::RANGED;
    }
    if pred(&equipment[items::slot::MAIN_HAND]) {
        mask = mask | ProcMask::MELEE_MH;
    }
    if pred(&equipment[items::slot::OFF_HAND]) {
        mask = mask | ProcMask::MELEE_OH;
    }
    mask
}

/// Go `getCurrentProcMaskForWeaponEnchant`.
pub(crate) fn proc_mask_for_weapon_enchant(sim: &Sim, unit: UnitId, effect_id: i32) -> ProcMask {
    current_proc_mask_for(sim, unit, |weapon| weapon.enchant.effect_id == effect_id)
}

/// Go `getCurrentProcMaskForWeaponEffect`.
pub(crate) fn proc_mask_for_weapon_effect(sim: &Sim, unit: UnitId, item_id: i32) -> ProcMask {
    current_proc_mask_for(sim, unit, |weapon| weapon.id == item_id)
}

/// Go `NewDynamicLegacyProcForEnchant`.
pub(crate) fn dynamic_legacy_proc_for_enchant(
    sim: &Sim,
    unit: UnitId,
    effect_id: i32,
    ppm: f64,
    fixed_proc_chance: f64,
) -> Rc<DynamicProcManager> {
    Rc::new(
        sim.new_dynamic_weapon_proc_manager(
            unit,
            ppm,
            fixed_proc_chance,
            proc_mask_for_weapon_enchant(sim, unit, effect_id),
        )
        .rebuilt_by_swap(
            sim,
            unit,
            manager_chance(ppm, fixed_proc_chance),
            ManagerMask::WeaponEnchant(effect_id),
        ),
    )
}

/// Go `NewDynamicLegacyProcForWeapon`.
pub(crate) fn dynamic_legacy_proc_for_weapon(
    sim: &Sim,
    unit: UnitId,
    item_id: i32,
    ppm: f64,
    fixed_proc_chance: f64,
) -> Rc<DynamicProcManager> {
    Rc::new(
        sim.new_dynamic_weapon_proc_manager(
            unit,
            ppm,
            fixed_proc_chance,
            proc_mask_for_weapon_effect(sim, unit, item_id),
        )
        .rebuilt_by_swap(
            sim,
            unit,
            manager_chance(ppm, fixed_proc_chance),
            ManagerMask::WeaponEffect(item_id),
        ),
    )
}

/// The chance of a manager Go builds with either a rate or a fixed chance.
fn manager_chance(ppm: f64, fixed_proc_chance: f64) -> ManagerChance {
    if ppm != 0.0 {
        ManagerChance::Ppm(ppm)
    } else {
        ManagerChance::Fixed(fixed_proc_chance)
    }
}

/// Go `dpmForMask`: the procs-per-minute manager for a rate and a mask, bound to the weapon or
/// enchant that carries it where the mask is not stated.
pub(crate) fn dpm_for_mask(
    sim: &Sim,
    unit: UnitId,
    source: EffectSource,
    ppm: f64,
    mask: ProcMask,
) -> Option<Rc<DynamicProcManager>> {
    if ppm <= 0.0 {
        return None;
    }
    if mask != ProcMask::UNKNOWN {
        let mut mask = mask;
        // A procs-per-minute enchant rolls on weapon hits only: spells and heals never proc it.
        if source.is_enchant {
            mask = ProcMask(mask.0 & ProcMask::MELEE_OR_RANGED.0);
        }
        if source.enchant_placement() == EnchantPlacement::OnWeapon {
            // NewDynamicLegacyProcForEnchantWithMask.
            let within = mask;
            let mask = ProcMask(mask.0 & proc_mask_for_weapon_enchant(sim, unit, source.id).0);
            return Some(Rc::new(
                sim.new_dynamic_weapon_proc_manager(unit, ppm, 0.0, mask)
                    .rebuilt_by_swap(
                        sim,
                        unit,
                        ManagerChance::Ppm(ppm),
                        ManagerMask::WeaponEnchantWithin(source.id, within),
                    ),
            ));
        }
        // NewLegacyPPMManager.
        return Some(Rc::new(
            sim.new_ppm_manager(unit, ppm, mask).rebuilt_by_swap(
                sim,
                unit,
                ManagerChance::Ppm(ppm),
                ManagerMask::Static(mask),
            ),
        ));
    }
    // With no mask of its own the rate has to be read off whatever the effect sits on.
    if source.is_enchant {
        return Some(dynamic_legacy_proc_for_enchant(
            sim, unit, source.id, ppm, 0.0,
        ));
    }
    Some(dynamic_legacy_proc_for_weapon(
        sim, unit, source.id, ppm, 0.0,
    ))
}

/// Go `damageDefenseType`: the defense type a proc's damage rolls with. A stated one wins;
/// otherwise the school decides, and `is_melee` stays honoured for a caller that means melee
/// damage without saying so through the school.
pub(crate) fn damage_defense_type(
    defense_type: DefenseType,
    spell_school: u8,
    is_melee: bool,
) -> DefenseType {
    if defense_type != DefenseType::None {
        return defense_type;
    }
    if is_melee || spell_school & school::PHYSICAL != 0 {
        return DefenseType::Melee;
    }
    DefenseType::Magic
}

// ---------------------------------------------------------------------------------------------
// Procs read from the spell data.
// ---------------------------------------------------------------------------------------------

/// The listener the trigger's row describes, without the handler, plus what the row cannot
/// state: the item's own name and action, which are what the sim keys the rolls and the metrics
/// by, and the rate the effect entry states. Go `spellDataProcListener`.
pub(crate) fn spell_data_proc_listener(
    sim: &Sim,
    unit: UnitId,
    cfg: &SpellDataProc,
    source: EffectSource,
    trigger: &'static Spell,
    proc: Option<&Message>,
) -> ProcTrigger {
    let ppm = proc.map_or(0.0, |proc| proc.f64("ppm"));
    let opts = [
        item_proc_chance(trigger),
        weapon_proc_shape(cfg),
        spell_data_proc_rate(source, trigger, ppm),
        stated_weapon_proc_chance(cfg.is_weapon_proc, source, trigger),
        chance_on_hit_rate(cfg, source),
    ];
    let mut config = proc_trigger(sim, Some(unit), trigger, &opts);
    config.name = cfg.name.to_string();
    config.action_id = source.action_id();
    config
}

/// A weapon proc's listener, which no row states; anything else keeps the one its row decodes
/// to. Go `weaponProcShape`.
fn weapon_proc_shape(cfg: &SpellDataProc) -> ProcOpt {
    if !cfg.is_weapon_proc {
        return Rc::new(|_, _, _| {});
    }
    weapon_proc()
}

/// The rate for a proc the client states none for. Procs per minute are not in the client's
/// spell data, so an item's reaches the sim through its effect entry, and the store carries one
/// only where an override put it there. Go `spellDataProcRate`.
fn spell_data_proc_rate(source: EffectSource, row: &'static Spell, entry_ppm: f64) -> ProcOpt {
    Rc::new(move |sim, character, trigger| {
        let mut ppm = entry_ppm;
        if ppm == 0.0 {
            ppm = f64::from(row.rppm);
        }
        let unit = character.expect("a rate needs a character");
        let Some(dpm) = dpm_for_mask(sim, unit, source, ppm, trigger.proc_mask) else {
            return;
        };
        // The callback rolls the chance first and the manager only after it, so a chance left
        // in place next to a manager would gate the rate twice.
        trigger.proc_chance = 0.0;
        trigger.dpm = Some(dpm);
    })
}

/// A rateless chance on hit's rate under Forever's rules ([`CHANCE_ON_HIT_PPM`]), in place of
/// whatever the row's sentinel reads as. A weapon rolls it on its own hits, by its own speed; any
/// other item on the wearer's landed melee and ranged hits, by the speed of the weapon that hit.
/// Spells never roll it.
fn chance_on_hit_rate(cfg: &SpellDataProc, source: EffectSource) -> ProcOpt {
    let rateless = cfg.rateless;
    Rc::new(move |sim, character, trigger| {
        if !rateless || sim.rules != super::sim::Rules::Forever {
            return;
        }
        let unit = character.expect("a rate needs a character");
        if !trigger.is_weapon_proc {
            trigger.proc_mask = ProcMask(trigger.proc_mask.0 & ProcMask::MELEE_OR_RANGED.0);
            trigger.outcome = HitOutcome::LANDED;
        }
        trigger.proc_chance = 0.0;
        trigger.dpm = dpm_for_mask(sim, unit, source, CHANCE_ON_HIT_PPM, trigger.proc_mask);
    })
}

/// A combat enchant's chance, which its row states in the column, rolled on the hits of the
/// enchanted weapon only: the weapon shape hears every hit, so a flat chance on the trigger
/// would also roll on the other hand's. Go `statedWeaponProcChance`.
fn stated_weapon_proc_chance(
    is_weapon_proc: bool,
    source: EffectSource,
    row: &'static Spell,
) -> ProcOpt {
    Rc::new(move |sim, character, trigger| {
        if !is_weapon_proc
            || !source.is_enchant
            || row.proc_chance_source != proc_chance_source::COLUMN
            || stated_chance(row) == 0.0
        {
            return;
        }
        let unit = character.expect("a rate needs a character");
        trigger.proc_chance = 0.0;
        trigger.dpm = Some(dynamic_legacy_proc_for_enchant(
            sim,
            unit,
            source.id,
            0.0,
            stated_chance(row),
        ));
    })
}

/// Go `procBuffDuration`: how long the buff lasts. The client leaves it off the buff's own row
/// on a fair few procs and states it on the trigger instead.
fn proc_buff_duration(
    cfg: &SpellDataProc,
    trigger: &'static Spell,
    buff: &'static Spell,
) -> Duration {
    if buff.duration_ms != 0 {
        return buff.duration();
    }
    if trigger.duration_ms != 0 {
        return trigger.duration();
    }
    panic!(
        "{} ({}): neither the proc's spell {} nor its buff {} states a duration for the aura it applies",
        cfg.name,
        cfg.source().id,
        trigger.id,
        buff.id
    );
}

/// Go `spellDataProcAura`: the buff the proc applies. The two counts the client keeps in one
/// field are not the same thing: a `CumulativeAura` count is stacks that each add their own
/// stats, a `ProcCharges` count is one buff at full stats that the game spends by uses.
fn spell_data_proc_aura(
    env: &mut Environment,
    cfg: &SpellDataProc,
    trigger: &'static Spell,
    buff: &'static Spell,
    effect: &Message,
) -> StatBuffAura {
    // A trinket whose trigger opens a window and whose stats accumulate on a second aura
    // resolves no stats on the aura the trigger applies.
    if effect.has("stacking_aura") {
        panic!(
            "{} ({}): a proc whose stats live on an accumulating aura needs the stacking constructor",
            cfg.name, cfg.item_id
        );
    }
    let unit = env.player;
    let label_opt: AuraOpt = label(format!("{} Proc", cfg.name));
    let mut aura = aura_config(buff, &[label_opt]);
    aura.duration = proc_buff_duration(cfg, trigger, buff);
    let buff_stats = effect_stats(effect);

    // The client states the count on whichever of the two rows carries the aura, and the item
    // effect entry takes the higher of them, so this does too.
    let stacks = buff.max_stack.max(trigger.max_stack);
    if stacks > 0 {
        aura.max_stacks = i32::from(stacks);
        return env.sim.make_stacking_aura(
            unit,
            StackingStatAura {
                aura,
                bonus_per_stack: buff_stats,
            },
        );
    }

    // Charges are the buff's own: they count how many times it acts before it drops.
    let max_stacks = i32::from(buff.proc_charges);
    let action_id = aura.action_id.clone().expect("the row's action");
    env.sim.new_temporary_stats_aura_wrapped(
        unit,
        &aura.label,
        &action_id,
        buff_stats,
        aura.duration,
        Some(&|config: &mut AuraConfig| config.max_stacks = max_stacks),
    )
}

/// Go `statesATrigger`: whether a trigger can be built from this row at all: it has to name hits
/// the sim hears and a rate that resolves to something.
fn states_a_trigger(s: &Spell) -> bool {
    let decoded = super::proc_type_mask::decode_proc_type_mask(
        s.proc_flags,
        super::proc_type_mask::ProcHint(s.proc_hint),
    );
    if decoded.callback == super::aura_helpers::CallbackMask::EMPTY {
        return false;
    }
    // A procs-per-minute rate is measured against the mask, which an empty one cannot do.
    if s.rppm > 0.0 {
        return decoded.proc_mask != ProcMask::UNKNOWN;
    }
    stated_chance(s) != 0.0
}

/// Go `attachChargeSpender`: what spends a charge. The buff's own row states which hits do, so
/// it is read as a listener of its own and attached to the buff, where it is live only while the
/// buff is up. Only a buff that is a spell of its own can say what spends a charge.
fn attach_charge_spender(
    env: &mut Environment,
    cfg: &SpellDataProc,
    trigger: &'static Spell,
    buff: &'static Spell,
    proc_aura: &StatBuffAura,
) {
    if buff.id == trigger.id
        || buff.proc_charges <= 0
        || buff.max_stack > 0
        || !states_a_trigger(buff)
    {
        return;
    }
    let unit = env.player;
    let mut spender = proc_trigger(&env.sim, Some(unit), buff, &[]);
    spender.name = format!("{} Charge", cfg.name);
    env.sim
        .attach_proc_trigger_callback(proc_aura.aura, unit, &spender);
}

/// Go `decodedCallback`.
fn decoded_callback(s: &Spell) -> super::aura_helpers::CallbackMask {
    super::proc_type_mask::decode_proc_type_mask(
        s.proc_flags,
        super::proc_type_mask::ProcHint(s.proc_hint),
    )
    .callback
}

/// The kinds of proc the shared constructors register, in Go's names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProcKind {
    /// `NewSpellDataProc`.
    Proc,
    /// `NewSpellDataDamageProc`.
    Damage,
    /// `NewSpellDataHealProc`.
    Heal,
    /// `NewSpellDataAbsorbProc`.
    Absorb,
    /// `NewSpellDataDebuffProc`.
    Debuff,
    /// `NewSpellDataAuraProc`.
    Aura,
    /// `NewSpellDataEquipAura`.
    EquipAura,
}

/// Go `registerSpellDataRowProc` and its siblings' registration conditions: whether the
/// constructor registers an effect at all. A proc is left unregistered where its row applies an
/// aura to an enemy (a buff proc), and where the trigger's row names no callback.
pub(crate) fn registers(kind: ProcKind, cfg: &SpellDataProc) -> bool {
    let (trigger, row) = cfg.rows();
    match kind {
        ProcKind::EquipAura => true,
        ProcKind::Proc if row.applies_an_aura_to_an_enemy() => false,
        _ => {
            cfg.is_weapon_proc
                || decoded_callback(trigger) != super::aura_helpers::CallbackMask::EMPTY
        }
    }
}

/// Applies the effect `kind` registers to the player.
pub(crate) fn apply(env: &mut Environment, kind: ProcKind, cfg: &SpellDataProc) {
    match kind {
        ProcKind::Proc => apply_spell_data_proc(env, cfg),
        ProcKind::Damage => apply_spell_data_damage_proc(env, cfg),
        ProcKind::Heal => apply_spell_data_heal_proc(env, cfg),
        ProcKind::Absorb => apply_spell_data_absorb_proc(env, cfg),
        ProcKind::Debuff => super::shared_auras::apply_spell_data_debuff_proc(env, cfg),
        ProcKind::Aura => super::shared_auras::apply_spell_data_aura_proc(env, cfg),
        ProcKind::EquipAura => super::shared_auras::apply_spell_data_equip_aura(env, cfg),
    }
}

/// Go `applySpellDataProc`: the buff the proc applies, the listener that applies it, and the
/// registrations that let an item swap and an APL find both.
fn apply_spell_data_proc(env: &mut Environment, cfg: &SpellDataProc) {
    let unit = env.player;
    let source = cfg.source();
    let (trigger, buff) = cfg.rows();
    let effect = source
        .proc_effect(buff.id)
        .unwrap_or_else(|| panic!("Error getting proc effects for item/enchant {}", source.id));

    let proc_aura = spell_data_proc_aura(env, cfg, trigger, buff, &effect);

    let mut listener =
        spell_data_proc_listener(&env.sim, unit, cfg, source, trigger, effect.message("proc"));

    // The same fallback the database layer applies: Bulwark of Azzinoth's armor buff sits in a
    // spell category with a 60s recovery while its trigger states nothing. Only a buff that is
    // a spell of its own counts.
    if listener.icd == 0 && buff.id != trigger.id {
        listener.icd = buff.category_cooldown();
    }
    let trigger_aura = source.register_trigger(env, &listener);

    attach_charge_spender(env, cfg, trigger, buff, &proc_aura);

    // What keeps the ICD-aware APL values from dropping the effect, not a gate on the proc. It
    // is assigned after the charge spender and whether or not there is one.
    let icd = env.sim.aura(trigger_aura).icd;
    env.sim.aura_mut(proc_aura.aura).icd = icd;
    source.register_weapon_enchant_buff(env, proc_aura.aura);
    let slots = source.eligible_slots(&env.sim, unit);
    add_stat_proc_buff(
        &mut env.sim,
        unit,
        source.id,
        proc_aura,
        source.is_enchant,
        &slots,
    );
}

/// Go `procDamageHandler`'s target is a fight matter. Go `applySpellDataDamageProc`: the spell
/// the proc casts, and the listener that casts it.
fn apply_spell_data_damage_proc(env: &mut Environment, cfg: &SpellDataProc) {
    let unit = env.player;
    let source = cfg.source();
    let (trigger, damage) = cfg.rows();
    let mut config = spell_data_proc_damage_spell(env, damage, true);
    // A Forever chance on hit's damage over time is the item's debuff, not the class spell its
    // row is named after: the Rend of Blood Talon (13318) is not the Warrior's.
    if cfg.rateless && env.sim.rules == super::sim::Rules::Forever && config.dot.number_of_ticks > 0
    {
        config.dot.aura.label = format!("{} {}", damage.name, damage.id);
        // A damage over time a missile carries alone goes on as the hit lands: its flight is
        // not modelled (README, Rust-only mechanics).
        if damage.damage_effect().is_nil() {
            config.missile_speed = 0.0;
        }
    }
    env.sim.register_spell(unit, config);

    // The proc's damage lands on the hit that caused it rather than on the next one.
    let mut listener = spell_data_proc_listener(&env.sim, unit, cfg, source, trigger, None);
    listener.trigger_immediately = true;
    source.register_trigger(env, &listener);
}

/// Go `registerSpellDataHealProc` and `applySpellDataSelfProc`.
fn apply_spell_data_heal_proc(env: &mut Environment, cfg: &SpellDataProc) {
    let (_, heal) = cfg.rows();
    let config = spell_data_proc_heal_spell(env, heal, true);
    apply_spell_data_self_proc(env, cfg, config);
}

/// Go `registerSpellDataAbsorbProc` and `applySpellDataSelfProc`.
fn apply_spell_data_absorb_proc(env: &mut Environment, cfg: &SpellDataProc) {
    let (_, absorb) = cfg.rows();
    let config = spell_data_absorb_spell(env, absorb, cfg.source().id, true);
    apply_spell_data_self_proc(env, cfg, config);
}

/// Go `applySpellDataSelfProc`: a proc that casts the row's spell on the wearer.
fn apply_spell_data_self_proc(env: &mut Environment, cfg: &SpellDataProc, config: SpellConfig) {
    let unit = env.player;
    let source = cfg.source();
    let (trigger, _) = cfg.rows();
    env.sim.register_spell(unit, config);

    let mut listener = spell_data_proc_listener(&env.sim, unit, cfg, source, trigger, None);
    listener.trigger_immediately = true;
    source.register_trigger(env, &listener);
}

// ---------------------------------------------------------------------------------------------
// The spells the procs and on-use items cast.
// ---------------------------------------------------------------------------------------------

/// Go `damageShape`: which multipliers and metrics bucket the damage belongs in. The row's
/// defense type decides, since that is what picks its hit table.
fn damage_shape(damage: &Spell) -> SpellOpt {
    if damage_defense_type(damage.defense_type_core(), damage.spell_school(), false)
        == DefenseType::Melee
    {
        return resolve_spell::melee(ProcMask::EMPTY);
    }
    resolve_spell::magic(ProcMask::EMPTY)
}

/// Go `castBy`: a proc's spell, or the spell an item use casts.
pub(crate) fn cast_by(as_proc: bool) -> SpellOpt {
    if as_proc {
        return Rc::new(proc_spell);
    }
    Rc::new(item_use_spell)
}

/// Go `procSpell`: the game casts a proc's spell off the hit that caused it. It spends neither
/// the player's global cooldown nor the resource bar the row prices the spell at, it has no cast
/// time to spend either, and it is a proc unless the row says it is not.
fn proc_spell(config: &mut SpellConfig, row: &Spell) {
    resolve_spell::proc()(config, row);
    if row.is_a_proc() {
        config.flags |= SpellFlag::PROC;
    }
}

/// Go `itemUseSpell`.
fn item_use_spell(config: &mut SpellConfig, _row: &Spell) {
    config.flags = SpellFlag(config.flags.0 & !(SpellFlag::APL.0 | SpellFlag::PASSIVE_SPELL.0));
    config.cost = Default::default();
}

/// Go `SpellDataProcDamageSpell`: the spell a proc of the row would cast, for a proc registered
/// outside the item effect registry.
pub(crate) fn spell_data_proc_damage_spell_config(
    env: &mut Environment,
    damage: &'static Spell,
) -> SpellConfig {
    spell_data_proc_damage_spell(env, damage, true)
}

/// Go `spellDataProcDamageSpell`: the spell the proc casts, as its row states it: school,
/// defense type, spell power share, travel time and the amount it rolls. What the row cannot
/// state is that it is a proc's spell: out of the rotation, not a cast of its own, and its hits
/// do not feed the damage-dealt listeners, which is what would have a weapon's own proc answer
/// itself.
pub(crate) fn spell_data_proc_damage_spell(
    env: &mut Environment,
    damage: &'static Spell,
    as_proc: bool,
) -> SpellConfig {
    let unit = env.player;
    let mut config = spell_config(
        &mut env.sim,
        unit,
        damage,
        &[damage_shape(damage), cast_by(as_proc)],
    );

    // The proc's own hits carry no mask: what hears them is the flags below, not a hit kind.
    config.proc_mask = ProcMask::EMPTY;
    if as_proc {
        config.flags |= SpellFlag::NO_ON_DAMAGE_DEALT;
    }
    config.defense_type = damage_defense_type(config.defense_type, config.spell_school, false);

    // The debuff the hit puts on each enemy it lands on registers the enemies' auras.
    super::shared_auras::debuff_on_landing(env, damage);
    let periodic = damage.periodic_damage_effect();
    if periodic.is_nil() {
        return config;
    }

    // Where the row also deals direct damage, the damage over time lands only with it.
    config.dot = dot_config(damage, periodic, &[]);
    config
}

/// Go `spellDataProcHealSpell`: the heal the proc casts, as its row states it: a share of the
/// target's maximum health or an amount the effect rolls, the spell power share the row states,
/// and a crit unless the row rules one out; or a heal over time where the row's heal is a
/// periodic aura.
pub(crate) fn spell_data_proc_heal_spell(
    env: &mut Environment,
    heal: &'static Spell,
    as_proc: bool,
) -> SpellConfig {
    let unit = env.player;
    let mut config = spell_config(
        &mut env.sim,
        unit,
        heal,
        &[
            resolve_spell::magic(ProcMask::SPELL_HEALING),
            cast_by(as_proc),
        ],
    );
    // A heal crits for the magic multiplier whatever the row files it under: 1248759 states no
    // defense type at all.
    config.defense_type = DefenseType::Magic;

    let effect = heal.proc_heal_effect();
    if effect.aura == dbcenums::A_PERIODIC_HEAL {
        // Go `spellDataProcHotSpell`: a heal over time on the wearer.
        config.bonus_coefficient = effect.coeff();
        let heal_label = format!("{} HoT", heal.name);
        config.hot = dot_config(heal, effect, &[label(heal_label)]);
        config.hot.self_only = true;
    }
    config
}

/// Go `spellDataAbsorbSpell`: the shield the row applies to the caster: the amount its absorb
/// effect rolls, taken off the damage of the schools the effect's Misc masks, for the row's
/// duration. A second cast replaces the shield left. The label carries the item or enchant,
/// since two of them may apply the same row.
pub(crate) fn spell_data_absorb_spell(
    env: &mut Environment,
    absorb: &'static Spell,
    source_id: i32,
    as_proc: bool,
) -> SpellConfig {
    let unit = env.player;
    let shield = env.sim.new_damage_absorption_aura(
        unit,
        super::aura_helpers::AbsorptionAuraConfig {
            aura: aura_config(absorb, &[label(format!("{} {}", absorb.name, source_id))]),
            ..Default::default()
        },
    );

    let mut config = spell_config(&mut env.sim, unit, absorb, &[cast_by(as_proc)]);
    config.proc_mask = ProcMask::EMPTY;
    config.related_self_buff = Some(shield.aura);
    config
}

/// Go `Cooldown{Timer: NewTimer(), Duration}`.
pub(crate) fn new_cooldown(sim: &mut Sim, unit: UnitId, duration: Duration) -> Cooldown {
    Cooldown {
        timer: Some(sim.new_timer(unit)),
        duration,
    }
}

/// Registers a spell and answers its id. Kept as a function so call sites read like Go's
/// `character.RegisterSpell(config)`.
pub(crate) fn register_spell(env: &mut Environment, config: SpellConfig) -> SpellId {
    let unit = env.player;
    env.sim.register_spell(unit, config)
}
