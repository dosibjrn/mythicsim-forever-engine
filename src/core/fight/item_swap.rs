//! Go sim/core/item_swaps.go: a character that swaps the main hand weapon during the fight.
//!
//! The fight starts on the request's equipment, Go's Main set, and the rotation's `itemSwap`
//! action changes to the item swap's Swap1 set. `SwapItems` puts the other weapon in the hand,
//! runs the callbacks registered for the slot in Go's order, changes the stats by the swap's
//! equipment stats, stops the swings and extends the global cooldown, and moves the set auras.
//! The iteration's end swaps back as a reset, and the next reset runs the callbacks again.
//!
//! The exporter describes each callback as the state its target takes under either set, and the
//! prepared state with the swap set worn as its difference from the reset's. What the swap
//! changes that a callback does not do itself, such as a proc manager's chances or the weapon a
//! seal reads, the runtime takes from that difference.

use crate::contracts::prepared_v2::{
    Effect, PreparedV2, SwapAuraState, SwapCallback, SwapChange, Weapon, WeaponSkillTable,
};
use crate::core::time::{go_string, NS_PER_MILLISECOND};

use super::{reads, spell_mod::ModKind, Agent, AuraRef, BuildError, Fight, SpellId};

/// Go `GCDDefault`.
const GCD_DEFAULT: i64 = 1500 * NS_PER_MILLISECOND;

/// Go `RegisterActive`'s cooldown: an item put on during the fight is unusable for 30 seconds.
const ITEM_SWAP_COOLDOWN: i64 = 30_000 * NS_PER_MILLISECOND;

/// What a callback does, by the set worn after the swap: index 0 for Main, 1 for Swap1.
#[derive(Clone, Debug)]
pub(crate) enum SwapOp {
    Aura {
        aura: AuraRef,
        states: [SwapAuraState; 2],
    },
    ItemUse {
        spell: SpellId,
        aura: Option<AuraRef>,
        worn: [bool; 2],
    },
    AuraOnChange {
        aura: AuraRef,
        carries: [bool; 2],
    },
    Mod {
        id: super::spell_mod::ModId,
        active: [bool; 2],
    },
}

/// Go `ItemSwap` for a swap of the main hand alone.
#[derive(Clone, Debug)]
pub(crate) struct ItemSwapState {
    /// Go `swapSet == Swap1`.
    pub(crate) swapped: bool,
    /// Go `ItemSwap.initialized`: false while a reset swaps.
    initialized: bool,
    /// The set auras, Main then Swap1.
    auras: [AuraRef; 2],
    /// The stat aura bit that carries the swap's equipment stats.
    stat_bit: u32,
    /// "Item Swap - Stats Change" toward Main and toward Swap1.
    logs: [String; 2],
    ops: Vec<SwapOp>,
    /// The main hand weapon of each set.
    weapons: [Weapon; 2],
    /// Go `Unit.weaponSkill` as each set's tables read it: main hand, off hand, ranged.
    skills: [[Option<WeaponSkillTable>; 3]; 2],
    /// Go `swapItem` sets the main hand weapon, except for a feral druid, whose form rebuilds
    /// its paw in a callback.
    sets_main_hand: bool,
    /// The prepared input as it reads with the swap set worn.
    pub(crate) worn: Box<PreparedV2>,
    /// The core effects' values that follow the weapon in the hand, for each set.
    variants: Variants,
    /// The attack tables' armor ignore factor after the reset.
    pub(crate) armor_ignore_reset: f64,
}

/// A damage proc's trigger spells and its proc manager's chances by spell.
type ProcHears = (Vec<bool>, Option<Vec<Option<f64>>>);

/// The core effects' values of each set, Main then Swap1, that differ between them.
#[derive(Clone, Debug, Default)]
struct Variants {
    /// Crusader's chances by spell, which its proc manager rebuilds for the weapons in the hand.
    crusader: Option<[Vec<Option<f64>>; 2]>,
    /// The damage procs whose trigger or chances follow the weapon, by their position.
    damage_procs: Vec<(usize, [ProcHears; 2])>,
    /// The rage a main hand and an off hand swing gives, and a crit's on top, which the rage bar
    /// reads from the weapon in the hand.
    rage: Option<[[f64; 4]; 2]>,
    /// The stat procs whose proc manager the swap rebuilds for the weapons in the hands, by
    /// their position: each spell's chance under either set.
    stat_procs: Vec<(usize, [Vec<Option<f64>>; 2])>,
    /// Sulfuras' chances, which its weapon proc manager rebuilds.
    sulfuras: Option<[Vec<Option<f64>>; 2]>,
    /// Thunderfury's chances, likewise.
    thunderfury: Option<[Vec<Option<f64>>; 2]>,
    /// The stacking on-use trinkets whose proc manager the swap rebuilds, as Badge of the
    /// Swarmguard's rate for the main hand's speed, by their position.
    stacking_cds: Vec<(usize, [Vec<Option<f64>>; 2])>,
    /// The armor debuff procs whose weapon proc manager the swap rebuilds, as a swap onto Dark
    /// Iron Sunderer, by their position.
    armor_debuff_procs: Vec<(usize, [Vec<Option<f64>>; 2])>,
    /// The crit the player takes under either set, which a hit on the player reads.
    reduced_crit_taken: Option<[f64; 2]>,
    /// Damage procs whose effect roll reads the weapon in the hand (the party's Flametongue
    /// Totem hits for the main hand's speed): the proc spell and its average under either set.
    effect_rolls: Vec<(SpellId, [f64; 2])>,
}

/// The prepared input with the swap set worn: the reset's with each changed value set.
pub(crate) fn swapped_prepared(
    prepared: &PreparedV2,
    changes: &[SwapChange],
) -> Result<PreparedV2, BuildError> {
    let mut value = serde_json::to_value(prepared).map_err(|err| err.to_string())?;
    for change in changes {
        set_pointer(&mut value, &change.path, change.value.clone())?;
    }
    serde_json::from_value(value).map_err(|err| format!("the swapped prepared input: {err}"))
}

/// Sets the value at a JSON pointer, adding an object key that is absent.
fn set_pointer(
    root: &mut serde_json::Value,
    path: &str,
    value: serde_json::Value,
) -> Result<(), BuildError> {
    let mut parts: Vec<String> = path
        .split('/')
        .skip(1)
        .map(|part| part.replace("~1", "/").replace("~0", "~"))
        .collect();
    let last = parts.pop().ok_or("an empty swap change path")?;
    let mut node = root;
    for part in parts {
        node = match node {
            serde_json::Value::Object(map) => map
                .get_mut(&part)
                .ok_or_else(|| format!("swap change path {path} names no value"))?,
            serde_json::Value::Array(items) => part
                .parse::<usize>()
                .ok()
                .and_then(|index| items.get_mut(index))
                .ok_or_else(|| format!("swap change path {path} names no value"))?,
            _ => return Err(format!("swap change path {path} names no value")),
        };
    }
    match node {
        serde_json::Value::Object(map) => {
            if value.is_null() {
                map.remove(&last);
            } else {
                map.insert(last, value);
            }
        }
        serde_json::Value::Array(items) => {
            let index: usize = last
                .parse()
                .map_err(|_| format!("swap change path {path} names no value"))?;
            *items
                .get_mut(index)
                .ok_or_else(|| format!("swap change path {path} names no value"))? = value;
        }
        _ => return Err(format!("swap change path {path} names no value")),
    }
    Ok(())
}

/// The item swap effect of a prepared input.
pub(crate) fn item_swap_effect(prepared: &PreparedV2) -> Option<&Effect> {
    prepared
        .effects
        .iter()
        .find(|effect| matches!(effect, Effect::ItemSwap { .. }))
}

fn mod_kind(kind: &str) -> Result<ModKind, BuildError> {
    match kind {
        "damage_done_pct" => Ok(ModKind::DamageDonePercent),
        "bonus_crit_percent" => Ok(ModKind::BonusCritPercent),
        "threat_multiplier_pct" => Ok(ModKind::ThreatMultiplierPercent),
        other => Err(format!("an item swap spell modifier of kind {other}")),
    }
}

impl<A: Agent> Fight<A> {
    /// Binds the item swap of a prepared input, when it has one.
    pub(crate) fn bind_item_swap(&mut self, prepared: &PreparedV2) -> Result<(), BuildError> {
        let Some(Effect::ItemSwap {
            main_aura,
            swapped_aura,
            to_swapped_log,
            to_main_log,
            callbacks,
            swapped,
            reset_activates,
            aura_effects,
            ..
        }) = item_swap_effect(prepared)
        else {
            return Ok(());
        };
        let worn = swapped_prepared(prepared, swapped)?;
        for effect in aura_effects {
            let aura = self.player_aura(&effect.aura)?;
            if !matches!(self.aura(aura).behavior, super::aura::AuraBehavior::Static) {
                return Err(format!("{} has a behavior of its own", effect.aura));
            }
            self.aura_mut(aura).behavior =
                super::aura::AuraBehavior::ArmorIgnoreFactor(effect.armor_ignore_factor);
        }
        // Go's unit reset activates them before the swap's reset switches them off.
        for label in reset_activates {
            let aura = self.player_aura(label)?;
            self.set_aura_permanent(aura, true);
        }
        let stat_bit = Self::stat_aura_bit(&prepared.effects, swapped_aura)
            .ok_or("the item swap's stat aura is not a stat aura")?;
        let mut ops = Vec::new();
        for callback in callbacks {
            ops.push(match callback {
                SwapCallback::Aura {
                    aura,
                    main,
                    swapped,
                    ..
                } => SwapOp::Aura {
                    aura: self.player_aura(aura)?,
                    states: [*main, *swapped],
                },
                SwapCallback::AuraOnChange {
                    aura,
                    main,
                    swapped,
                    ..
                } => SwapOp::AuraOnChange {
                    aura: self.player_aura(aura)?,
                    carries: [*main, *swapped],
                },
                SwapCallback::ItemUse {
                    spell,
                    aura,
                    main,
                    swapped,
                    ..
                } => SwapOp::ItemUse {
                    spell: *spell,
                    aura: aura
                        .as_ref()
                        .map(|label| self.player_aura(label))
                        .transpose()?,
                    worn: [*main, *swapped],
                },
                SwapCallback::SpellMod {
                    modifier,
                    main,
                    swapped,
                    ..
                } => {
                    let id = self.register_mod(
                        mod_kind(&modifier.kind)?,
                        modifier.value,
                        0,
                        modifier.spells.clone(),
                    );
                    // A modifier active after the reset is already in the exported spells.
                    self.set_mod_active(id, modifier.active);
                    SwapOp::Mod {
                        id,
                        active: [*main, *swapped],
                    }
                }
            });
        }
        let feral = matches!(prepared.player.class.as_str(), "ClassDruid")
            && prepared
                .effects
                .iter()
                .any(|effect| matches!(effect, Effect::CatForm { .. } | Effect::BearForm { .. }));
        self.item_swap = Some(ItemSwapState {
            swapped: false,
            initialized: true,
            auras: [
                self.player_aura(main_aura)?,
                self.player_aura(swapped_aura)?,
            ],
            stat_bit,
            logs: [to_main_log.clone(), to_swapped_log.clone()],
            ops,
            weapons: [
                prepared.melee.main_hand.clone(),
                worn.melee.main_hand.clone(),
            ],
            skills: [&prepared.melee, &worn.melee].map(|melee| {
                [
                    melee.main_hand_skill.clone(),
                    melee.off_hand_skill.clone(),
                    melee.ranged_skill.clone(),
                ]
            }),
            sets_main_hand: !feral,
            worn: Box::new(worn),
            variants: Variants::default(),
            armor_ignore_reset: prepared.melee.armor_ignore_factor,
        });
        Ok(())
    }

    /// Binds what the core effects read for each set, once the fight has bound them.
    pub(crate) fn bind_item_swap_variants(&mut self, effects: &[Effect]) {
        let Some(swap) = self.item_swap.as_ref() else {
            return;
        };
        let worn = swap.worn.effects.clone();
        let worn_crit_taken = swap.worn.player.pseudo_stats.reduced_crit_taken_percent;
        let spells = self.spells.len();
        let mut variants = Variants::default();
        if let Some(main) = self
            .self_target
            .as_ref()
            .map(|target| target.reduced_crit_taken_percent)
        {
            if main != worn_crit_taken {
                variants.reduced_crit_taken = Some([main, worn_crit_taken]);
            }
        }
        let crusader = |effects: &[Effect]| {
            effects.iter().find_map(|effect| match effect {
                Effect::Crusader { chances, .. } => Some(super::chances_by_spell(chances, spells)),
                _ => None,
            })
        };
        if let (Some(main), Some(swapped)) = (crusader(effects), crusader(&worn)) {
            if main != swapped {
                variants.crusader = Some([main, swapped]);
            }
        }
        let procs = |effects: &[Effect]| -> Vec<ProcHears> {
            effects
                .iter()
                .filter_map(|effect| super::damage_proc_of(effect, spells))
                .map(|proc| (proc.trigger_spells, proc.chances))
                .collect()
        };
        for (index, (main, swapped)) in procs(effects).into_iter().zip(procs(&worn)).enumerate() {
            if main != swapped {
                variants.damage_procs.push((index, [main, swapped]));
            }
        }
        let rage = |effects: &[Effect]| {
            effects.iter().find_map(|effect| match effect {
                Effect::RageBar {
                    main_hand_rage,
                    off_hand_rage,
                    main_hand_crit_rage,
                    off_hand_crit_rage,
                    ..
                } => Some([
                    *main_hand_rage,
                    *off_hand_rage,
                    *main_hand_crit_rage,
                    *off_hand_crit_rage,
                ]),
                _ => None,
            })
        };
        if let (Some(main), Some(swapped)) = (rage(effects), rage(&worn)) {
            if main != swapped {
                variants.rage = Some([main, swapped]);
            }
        }
        let stat_procs = |effects: &[Effect]| -> Vec<Vec<Option<f64>>> {
            effects
                .iter()
                .filter_map(|effect| match effect {
                    Effect::StatProc { chances, .. } => {
                        Some(super::chances_by_spell(chances, spells))
                    }
                    _ => None,
                })
                .collect()
        };
        for (index, (main, swapped)) in stat_procs(effects)
            .into_iter()
            .zip(stat_procs(&worn))
            .enumerate()
        {
            if main != swapped {
                variants.stat_procs.push((index, [main, swapped]));
            }
        }
        let rolls = |effects: &[Effect]| -> Vec<(SpellId, f64)> {
            effects
                .iter()
                .filter_map(|effect| match effect {
                    Effect::SpellDataDamageProc {
                        spell,
                        average,
                        roll: None,
                        ..
                    } => Some((*spell, *average)),
                    _ => None,
                })
                .collect()
        };
        for ((spell, main), (worn_spell, swapped)) in rolls(effects).into_iter().zip(rolls(&worn)) {
            if spell == worn_spell && main.to_bits() != swapped.to_bits() {
                variants.effect_rolls.push((spell, [main, swapped]));
            }
        }
        let sulfuras = |effects: &[Effect]| {
            effects.iter().find_map(|effect| match effect {
                Effect::SulfurasHandOfRagnaros { chances, .. } => {
                    Some(super::chances_by_spell(chances, spells))
                }
                _ => None,
            })
        };
        if let (Some(main), Some(swapped)) = (sulfuras(effects), sulfuras(&worn)) {
            if main != swapped {
                variants.sulfuras = Some([main, swapped]);
            }
        }
        let thunderfury = |effects: &[Effect]| {
            effects.iter().find_map(|effect| match effect {
                Effect::Thunderfury { chances, .. } => {
                    Some(super::chances_by_spell(chances, spells))
                }
                _ => None,
            })
        };
        if let (Some(main), Some(swapped)) = (thunderfury(effects), thunderfury(&worn)) {
            if main != swapped {
                variants.thunderfury = Some([main, swapped]);
            }
        }
        // The chances `bind` gives a stacking trinket: the manager's own, else its flat chance
        // on each spell it hears.
        let stacking_cds = |effects: &[Effect]| -> Vec<Vec<Option<f64>>> {
            effects
                .iter()
                .filter_map(|effect| match effect {
                    Effect::StackingCdOnUse {
                        chances: Some(manager),
                        ..
                    } => Some(super::chances_by_spell(manager, spells)),
                    Effect::StackingCdOnUse {
                        trigger_spells,
                        proc_chance,
                        ..
                    } => {
                        let mut chances = vec![None; spells];
                        for &spell in trigger_spells {
                            if let Some(slot) = chances.get_mut(spell) {
                                *slot = Some(*proc_chance);
                            }
                        }
                        Some(chances)
                    }
                    _ => None,
                })
                .collect()
        };
        for (index, (main, swapped)) in stacking_cds(effects)
            .into_iter()
            .zip(stacking_cds(&worn))
            .enumerate()
        {
            if main != swapped {
                variants.stacking_cds.push((index, [main, swapped]));
            }
        }
        let armor_debuff_procs = |effects: &[Effect]| -> Vec<Vec<Option<f64>>> {
            effects
                .iter()
                .filter_map(|effect| match effect {
                    Effect::ArmorDebuffProc { chances, .. } => {
                        Some(super::chances_by_spell(chances, spells))
                    }
                    _ => None,
                })
                .collect()
        };
        for (index, (main, swapped)) in armor_debuff_procs(effects)
            .into_iter()
            .zip(armor_debuff_procs(&worn))
            .enumerate()
        {
            if main != swapped {
                variants.armor_debuff_procs.push((index, [main, swapped]));
            }
        }
        self.item_swap.as_mut().expect("bound").variants = variants;
    }

    /// The effects as they read with the swap set worn, when the character swaps items.
    pub(crate) fn item_swap_worn_effects(&self) -> Option<Vec<Effect>> {
        self.item_swap
            .as_ref()
            .map(|swap| swap.worn.effects.clone())
    }

    /// Whether the swap set is worn.
    pub(crate) fn item_swapped(&self) -> bool {
        self.item_swap.as_ref().is_some_and(|swap| swap.swapped)
    }

    /// Go `APLActionItemSwap.IsReady`.
    pub(crate) fn item_swap_ready(&self, to_swapped: bool) -> bool {
        self.item_swap
            .as_ref()
            .is_some_and(|swap| swap.swapped != to_swapped)
    }

    /// Go `APLActionItemSwap.Execute`.
    pub(crate) fn item_swap_action(&mut self, to_swapped: bool) {
        if self.log.is_some() {
            let set = if to_swapped { "Swap1" } else { "Main" };
            let line = if self.item_swapped() == to_swapped {
                format!("Item Swap already set to {set}")
            } else {
                format!("Item Swap to set {set}")
            };
            self.player_log(&line);
        }
        self.swap_items(to_swapped, false);
    }

    /// Go `ItemSwap.reset`, at the reset and at the iteration's end: back to the Main set, with
    /// the callbacks run as a reset runs them.
    pub(crate) fn item_swap_reset(&mut self) {
        let Some(swap) = self.item_swap.as_mut() else {
            return;
        };
        swap.initialized = false;
        self.swap_items(false, true);
        if let Some(swap) = self.item_swap.as_mut() {
            swap.initialized = true;
        }
    }

    /// Go `ItemSwap.SwapItems` for the main hand alone.
    pub(crate) fn swap_items(&mut self, to_swapped: bool, is_reset: bool) {
        let Some(swap) = self.item_swap.as_ref() else {
            return;
        };
        let valid = swap.swapped != to_swapped;
        if !valid && !is_reset {
            return;
        }
        let prepull = self.now < 0;
        // swapItem: a reset puts the Main set's weapon in the hand, any other swap the other set's.
        let worn = if is_reset { false } else { !swap.swapped };
        let index = usize::from(worn);
        let (sets_main_hand, weapon) = (swap.sets_main_hand, swap.weapons[index].clone());
        let initialized = swap.initialized;
        // The weapon the swap takes out of the hand: the set worn until now.
        let previous = usize::from(swap.swapped);
        let ops = swap.ops.clone();
        if !prepull && sets_main_hand && self.autos.melee {
            self.set_main_hand(weapon);
        }
        for op in &ops {
            self.run_swap_op(op, index, previous, initialized);
        }
        A::on_item_swap(self, worn);
        self.swap_core_effects(worn);
        // Go `updateWeaponSkills`, after the callbacks and before the swap's validity is checked.
        let [main_hand, off_hand, ranged] = self
            .item_swap
            .as_ref()
            .expect("the item swap is bound")
            .skills[index]
            .clone();
        let melee = &mut self.config.melee;
        (
            melee.main_hand_skill,
            melee.off_hand_skill,
            melee.ranged_skill,
        ) = (main_hand, off_hand, ranged);
        if !valid {
            return;
        }
        let swap = self.item_swap.as_ref().expect("the item swap is bound");
        // The stats change toward the set the swap leaves for: Swap1's are added, Main's are
        // Swap1's taken away.
        let (line, bit) = (swap.logs[usize::from(to_swapped)].clone(), swap.stat_bit);
        if self.log.is_some() {
            self.sim_log(&line);
        }
        let mask = if to_swapped {
            self.stat_mask | bit
        } else {
            self.stat_mask & !bit
        };
        self.set_stat_mask(mask);
        if !prepull && !is_reset {
            let now = self.now;
            self.stop_melee_until(now);
            self.stop_ranged_until(now);
            let ready = self.player.gcd.max(now + GCD_DEFAULT);
            self.extend_gcd_until(ready);
        }
        let swap = self.item_swap.as_ref().expect("the item swap is bound");
        let (from, to) = (
            swap.auras[usize::from(swap.swapped)],
            swap.auras[usize::from(to_swapped)],
        );
        self.deactivate_aura(from);
        self.activate_aura(to);
        self.item_swap
            .as_mut()
            .expect("the item swap is bound")
            .swapped = to_swapped;
    }

    /// One callback for the set now worn.
    fn run_swap_op(&mut self, op: &SwapOp, index: usize, previous: usize, initialized: bool) {
        match *op {
            SwapOp::Aura { aura, states } => match states[index] {
                SwapAuraState::On => {
                    if !self.aura(aura).active {
                        self.activate_aura(aura);
                    }
                }
                SwapAuraState::Activate => self.activate_aura(aura),
                SwapAuraState::Off => self.deactivate_aura(aura),
                SwapAuraState::Keep => {}
            },
            SwapOp::AuraOnChange { aura, carries } => {
                if !carries[index] {
                    self.deactivate_aura(aura);
                } else if !carries[previous] {
                    self.activate_aura(aura);
                }
            }
            SwapOp::ItemUse { spell, aura, worn } => {
                if let Some(aura) = aura {
                    if self.aura(aura).active {
                        self.deactivate_aura(aura);
                    }
                }
                self.changed(reads::SPELLS);
                if !worn[index] {
                    self.spells[spell].flags.swapped = true;
                    return;
                }
                self.spells[spell].flags.swapped = false;
                if !initialized {
                    return;
                }
                if let Some((timer, _)) = self.spells[spell].cd {
                    let remaining = (self.timers[timer] - self.now).max(0);
                    self.timers[timer] = self.now + remaining.max(ITEM_SWAP_COOLDOWN);
                    self.changed(reads::TIMERS);
                }
            }
            SwapOp::Mod { id, active } => {
                if active[index] {
                    self.activate_mod(id);
                } else {
                    self.deactivate_mod(id);
                }
            }
        }
    }

    /// Go `Unit.ExtendGCDUntil`.
    pub(crate) fn extend_gcd_until(&mut self, ready: i64) {
        assert!(ready >= self.now, "cannot wait negative time");
        self.set_gcd_timer(ready);
        if self.log.is_some() && ready > self.now {
            let line = format!(
                "Extending GCD for {} due to rotation / CDs.",
                go_string(ready - self.now)
            );
            self.player_log(&line);
        }
    }

    /// The core effects whose parameters follow the weapon in the hand: what the swapped
    /// state's effects say for the set now worn.
    fn swap_core_effects(&mut self, worn: bool) {
        let Some(swap) = self.item_swap.as_ref() else {
            return;
        };
        let index = usize::from(worn);
        let variants = swap.variants.clone();
        if let (Some(by_set), Some(crusader)) = (variants.crusader, self.crusader.as_mut()) {
            crusader.chances = by_set[index].clone();
        }
        if let (Some(by_set), Some(bar)) = (variants.rage, self.rage.as_mut()) {
            let [main_hand, off_hand, main_crit, off_crit] = by_set[index];
            bar.main_hand_rage = main_hand;
            bar.off_hand_rage = off_hand;
            bar.main_hand_crit_rage = main_crit;
            bar.off_hand_crit_rage = off_crit;
        }
        for (spell, by_set) in variants.effect_rolls {
            if let super::SpellBehavior::EffectRoll { average, .. } =
                &mut self.spells[spell].behavior
            {
                *average = by_set[index];
            }
        }
        for (proc, by_set) in variants.stat_procs {
            self.stat_procs[proc].chances = by_set[index].clone();
        }
        if let (Some(by_set), Some(sulfuras)) = (variants.sulfuras, self.sulfuras.as_mut()) {
            sulfuras.chances = by_set[index].clone();
        }
        if let (Some(by_set), Some(thunderfury)) = (variants.thunderfury, self.thunderfury.as_mut())
        {
            thunderfury.chances = by_set[index].clone();
        }
        if let (Some(by_set), Some(target)) =
            (variants.reduced_crit_taken, self.self_target.as_mut())
        {
            target.reduced_crit_taken_percent = by_set[index];
        }
        for (proc, by_set) in variants.stacking_cds {
            self.stacking_cds[proc].chances = by_set[index].clone();
        }
        for (proc, by_set) in variants.armor_debuff_procs {
            self.armor_debuff_procs[proc].chances = by_set[index].clone();
        }
        for (proc, by_set) in variants.damage_procs {
            let (trigger_spells, chances) = by_set[index].clone();
            let state = &mut self.damage_procs[proc];
            state.trigger_spells = trigger_spells;
            state.chances = chances;
        }
    }
}
