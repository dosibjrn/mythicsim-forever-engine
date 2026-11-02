//! Go sim/core/item_swaps.go: a character with an item swap set, the request's `itemSwap`.
//!
//! The character starts on its equipment, Go's Main set. The swap set's items register their
//! effects beside the worn ones' (`unEquippedItems.applyItemEffects`), the slots an item or enchant
//! is eligible in span both sets, and every swap-aware registration adds a callback for its slots.
//! A reset swaps back to the Main set and runs the swapped slots' callbacks. Preparation runs the
//! callbacks itself: at each reset, and in a simulation swapped to the Swap1 set at the pull, whose
//! prepared state the `item_swap` effect gives as its difference from the reset's.
//!
//! A callback is described, not captured as a closure, so a copy of the simulation runs it on its
//! own state and the export can say what it does under either set.

use serde_json::{json, Value};

use crate::contracts::request::Message;

use super::attack::Weapon;
use super::env::Environment;
use super::items::{self, slot, Equipment, NUM_ITEM_SLOTS};
use super::sim::{AuraConfig, AuraId, Sim, UnitId, NEVER_EXPIRES};
use super::spell_mod::ModId;
use super::stats::Stats;
use super::Refusal;
use crate::contracts::prepared_v2::ActionId;

/// Go `AllWeaponSlots`.
pub(crate) const WEAPON_SLOTS: [usize; 3] = [slot::MAIN_HAND, slot::OFF_HAND, slot::RANGED];

/// The set auras' labels.
pub(crate) const MAIN_LABEL: &str = "Item Swap: Main";
pub(crate) const SWAPPED_LABEL: &str = "Item Swap: Swapped";

/// How a dynamic proc manager's mask is found again at a swap: Go procs.go's mask closures.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ManagerMask {
    /// `getCurrentProcMaskForWeaponEnchant`.
    WeaponEnchant(i32),
    /// `getCurrentProcMaskForWeaponEnchant` narrowed to a mask.
    WeaponEnchantWithin(i32, super::spell::ProcMask),
    /// `getCurrentProcMaskForWeaponTempEnchant`.
    WeaponTempEnchant(i32),
    /// `getCurrentProcMaskForWeaponEffect`.
    WeaponEffect(i32),
    /// A static mask, as `NewLegacyPPMManager` keeps.
    Static(super::spell::ProcMask),
}

/// A fixed chance a manager takes for its mask: none, a constant, or a shaman imbue's, which is
/// Windfury's 0.2 whatever the mask.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ManagerChance {
    Ppm(f64),
    Fixed(f64),
}

/// A callback Go registers for item swap slots, as what it does.
#[derive(Clone, Debug)]
pub(crate) enum SwapCallback {
    /// item_swaps.go RegisterActive: the item's use.
    ItemUse { item_id: i32, slots: Vec<usize> },
    /// item_swaps.go registerProcInternal: the trigger aura is up while the item or enchant is.
    ProcAura {
        aura: AuraId,
        item_id: i32,
        enchant_id: i32,
        slots: Vec<usize>,
    },
    /// item_swaps.go RegisterEnchantBuffWithSlots: the buff falls off once no slot carries it.
    EnchantBuff {
        aura: AuraId,
        enchant_id: i32,
        slots: Vec<usize>,
    },
    /// A weapon specialization's modifier, active with a main hand of the hand type.
    HandMod { id: ModId, hand_type: String },
    /// talents_arms.go registerWeaponmaster.
    Weaponmaster {
        armor_aura: AuraId,
        armor_ignore: f64,
        mh_crit: ModId,
        oh_crit: ModId,
    },
    /// totems.go registerWindfuryTotemSpell: the totem's intermediate aura falls at any swap.
    Deactivate { aura: AuraId },
    /// weapon_imbues.go RegisterOnItemSwapWithImbue: Activate while a hand carries the imbue.
    ImbueAura { aura: AuraId, enchant_id: i32 },
    /// weapon_imbues.go RegisterRockbiterImbue.
    Rockbiter { aura: AuraId },
    /// weapon_imbues.go makeFTProcTriggerAura: off when the weapon now in the hand lacks
    /// Flametongue, Activate when it carries it and the weapon it replaced did not.
    Flametongue { aura: AuraId, slot: usize },
    /// imbue_swap.go registerPerWeaponMainHandImbue: each stone's auras follow its weapon.
    MainHandImbue {
        pull_id: i32,
        swap_id: i32,
        effects: Vec<(Vec<AuraId>, bool, bool)>,
    },
    /// druid.go Initialize: the paw is rebuilt from the weapon in the hand.
    DruidPaw,
    /// shield_block.go deactivateWithoutShield: the aura falls once no shield is worn.
    WithoutShield { aura: AuraId },
}

/// Go `ItemSwap`.
#[derive(Clone, Debug)]
pub(crate) struct ItemSwap {
    /// Go `swap.character != nil`: set by `Character.initialize`.
    pub enabled: bool,
    pub slots: Vec<usize>,
    pub original_equip: Equipment,
    pub swap_equip: Equipment,
    pub un_equipped: Equipment,
    /// Go `swapSet == Swap1`.
    pub swapped: bool,
    pub all_slot_stats: Stats,
    pub weapon_slot_stats: Stats,
    /// The set auras, Main then Swap1.
    pub auras: [AuraId; 2],
    pub initialized: bool,
    pub is_feral_druid: bool,
    /// Each slot's callbacks, in registration order.
    pub callbacks: Vec<Vec<SwapCallback>>,
    /// What Go does at a swap that preparation does not follow, which refuses the request.
    pub unported: Vec<String>,
}

impl ItemSwap {
    /// Go `IsEnabled`.
    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled && !self.slots.is_empty()
    }

    /// The equipment of a set as the character wears it, imbues included.
    pub(crate) fn set(&self, swapped: bool) -> Equipment {
        let mut equipment = self.original_equip.clone();
        if swapped {
            for &slot in &self.slots {
                equipment[slot] = self.swap_equip[slot].clone();
            }
        }
        equipment
    }
}

/// Go `toItem(itemSpec).inArea(areaTypes)`.
fn swap_item(spec: &Message, area_types: &[String]) -> Result<items::Item, Refusal> {
    if spec.i32("id") == 0 {
        return Ok(items::Item::default());
    }
    let item = items::new_item(spec)?;
    Ok(items::in_area(item, area_types))
}

/// Go `calcItemSwapStatsOffset`.
fn stats_offset(
    original: &Equipment,
    swap: &Equipment,
    prepull: Stats,
    slots: &[usize],
) -> (Stats, Stats) {
    let mut swap_stat_equipment = original.clone();
    let mut weapon_stat_equipment = original.clone();
    for &slot in slots {
        swap_stat_equipment[slot] = swap[slot].clone();
        if [slot::MAIN_HAND, slot::OFF_HAND, slot::RANGED].contains(&slot) {
            weapon_stat_equipment[slot] = swap[slot].clone();
        }
    }
    let original_stats = items::equipment_stats(original);
    let all = prepull.add(&items::equipment_stats(&swap_stat_equipment).subtract(&original_stats));
    let weapons = items::equipment_stats(&weapon_stat_equipment).subtract(&original_stats);
    (all, weapons)
}

impl Sim {
    /// Go `Character.enableItemSwap`, from `NewCharacter`.
    pub(crate) fn enable_item_swap(
        &mut self,
        unit: UnitId,
        item_swap: &Message,
        area_types: &[String],
    ) -> Result<(), Refusal> {
        let mut swap_items: Equipment = Default::default();
        let mut has_item_swap = [false; NUM_ITEM_SLOTS];
        for (index, spec) in item_swap.messages("items").into_iter().enumerate() {
            if index >= NUM_ITEM_SLOTS {
                return Err(Refusal::new(
                    "item_swap",
                    "more swap items than slots".to_string(),
                ));
            }
            has_item_swap[index] = spec.i32("id") != 0;
            swap_items[index] = swap_item(spec, area_types)?;
        }
        let two_hand_swap = swap_items[slot::MAIN_HAND].hand_type == "HandTypeTwoHand";
        let has_mh = self.mh_weapon(unit).is_some();
        let has_oh = !self.character(unit).equipment[slot::OFF_HAND].is_empty();
        if has_item_swap[slot::OFF_HAND] && has_mh {
            has_item_swap[slot::MAIN_HAND] = true;
        }
        if two_hand_swap && has_oh {
            has_item_swap[slot::OFF_HAND] = true;
        }
        let slots: Vec<usize> = (0..NUM_ITEM_SLOTS)
            .filter(|slot| has_item_swap[*slot])
            .collect();
        if slots.is_empty() {
            return Ok(());
        }
        let prepull = match item_swap.message("prepull_bonus_stats") {
            Some(bonus) => Stats::from_proto_array(&bonus.f64s("stats"))
                .add(&items::stats_from_pseudo_stats(&bonus.f64s("pseudo_stats"))),
            None => Stats::default(),
        };
        let original = self.character(unit).equipment.clone();
        let (all, weapons) = stats_offset(&original, &swap_items, prepull, &slots);
        let mut auras = [AuraId(0); 2];
        for (index, swapped) in [false, true].into_iter().enumerate() {
            let aura = self.get_or_register_aura(
                unit,
                AuraConfig {
                    label: if swapped { SWAPPED_LABEL } else { MAIN_LABEL }.to_string(),
                    action_id: Some(ActionId {
                        other_id: "OtherActionItemSwap".to_string(),
                        tag: index as i32 + 1,
                        ..ActionId::default()
                    }),
                    duration: NEVER_EXPIRES,
                    ..AuraConfig::default()
                },
            );
            if !swapped {
                self.make_permanent(aura);
            }
            auras[index] = aura;
        }
        let class = self.character(unit).class.clone();
        let spec = self
            .character(unit)
            .player
            .oneof("spec")
            .map(|(name, _)| name);
        let is_feral_druid =
            class == "ClassDruid" && matches!(spec, Some("feral_cat_druid" | "feral_bear_druid"));
        self.character_mut(unit).item_swap = Some(ItemSwap {
            enabled: false,
            slots,
            original_equip: original,
            un_equipped: swap_items.clone(),
            swap_equip: swap_items,
            swapped: false,
            all_slot_stats: all,
            weapon_slot_stats: weapons,
            auras,
            initialized: false,
            is_feral_druid,
            callbacks: vec![Vec::new(); NUM_ITEM_SLOTS],
            unported: Vec::new(),
        });
        Ok(())
    }

    /// The character's item swap, when it is enabled.
    pub(crate) fn item_swap(&self, unit: UnitId) -> Option<&ItemSwap> {
        self.unit(unit)
            .character
            .as_ref()?
            .item_swap
            .as_ref()
            .filter(|swap| swap.is_enabled())
    }

    fn item_swap_mut(&mut self, unit: UnitId) -> Option<&mut ItemSwap> {
        self.unit_mut(unit)
            .character
            .as_mut()?
            .item_swap
            .as_mut()
            .filter(|swap| swap.is_enabled())
    }

    /// Go `ItemSwap.initialize`.
    pub(crate) fn initialize_item_swap(&mut self, unit: UnitId) {
        if let Some(swap) = self.character_mut(unit).item_swap.as_mut() {
            swap.enabled = true;
        }
    }

    /// Go `RegisterItemSwapCallback`: nothing without an item swap.
    pub(crate) fn register_item_swap_callback(
        &mut self,
        unit: UnitId,
        slots: &[usize],
        callback: SwapCallback,
    ) {
        assert!(
            !self.is_finalized(),
            "an item swap callback registered in a finalized environment"
        );
        let Some(swap) = self.item_swap_mut(unit) else {
            return;
        };
        for &slot in slots {
            swap.callbacks[slot].push(callback.clone());
        }
    }

    /// Notes a swap-aware Go registration preparation does not follow: the request is refused.
    pub(crate) fn item_swap_unported(&mut self, unit: UnitId, what: impl Into<String>) {
        if let Some(swap) = self.item_swap_mut(unit) {
            swap.unported.push(what.into());
        }
    }

    /// Go `ItemSwap.AddTempEnchant`.
    pub(crate) fn item_swap_add_temp_enchant(
        &mut self,
        unit: UnitId,
        enchant_id: i32,
        slot: usize,
        swapped: bool,
    ) {
        let Some(swap) = self.item_swap_mut(unit) else {
            return;
        };
        if swapped {
            swap.un_equipped[slot].temp_enchant = enchant_id;
            swap.swap_equip[slot].temp_enchant = enchant_id;
        } else {
            swap.original_equip[slot].temp_enchant = enchant_id;
        }
    }

    /// Go `ItemSwap.EligibleSlotsForItem` filter: the item is in the slot in either set.
    pub(crate) fn item_swap_holds_item(&self, unit: UnitId, slot: usize, item_id: i32) -> bool {
        match self.item_swap(unit) {
            Some(swap) => {
                swap.original_equip[slot].id == item_id || swap.swap_equip[slot].id == item_id
            }
            None => self.character(unit).equipment[slot].id == item_id,
        }
    }

    /// Go `ItemSwap.EligibleSlotsForEffect` filter: the enchant is in the slot in either set.
    pub(crate) fn item_swap_holds_enchant(
        &self,
        unit: UnitId,
        slot: usize,
        effect_id: i32,
    ) -> bool {
        let carries = |item: &items::Item| {
            item.enchant.effect_id == effect_id || item.temp_enchant == effect_id
        };
        match self.item_swap(unit) {
            Some(swap) => carries(&swap.original_equip[slot]) || carries(&swap.swap_equip[slot]),
            None => carries(&self.character(unit).equipment[slot]),
        }
    }

    /// Go `ItemSwap.GetUnequippedItemBySlot`.
    pub(crate) fn unequipped_item(&self, unit: UnitId, slot: usize) -> Option<&items::Item> {
        self.item_swap(unit).map(|swap| &swap.un_equipped[slot])
    }
}

/// Go `Character.WeaponFromMainHand`.
pub(crate) fn weapon_from_main_hand(sim: &Sim, unit: UnitId) -> Weapon {
    match sim.mh_weapon(unit) {
        Some(item) => Weapon::from_item(item, sim.unit(unit).pseudo_stats.bonus_mh_dps),
        None => Weapon::unarmed(),
    }
}

/// Go `Character.WeaponFromOffHand`.
pub(crate) fn weapon_from_off_hand(sim: &Sim, unit: UnitId) -> Weapon {
    match sim.oh_weapon(unit) {
        Some(item) => Weapon::from_item(item, sim.unit(unit).pseudo_stats.bonus_oh_dps),
        None => Weapon::default(),
    }
}

/// The set auras and states a swap leaves, for the export.
pub(crate) fn swap_labels() -> Value {
    json!({"main_aura": MAIN_LABEL, "swapped_aura": SWAPPED_LABEL})
}

fn weapon_type_in_hand(item: &items::Item, types: &[&str]) -> bool {
    !item.is_empty() && types.contains(&item.weapon_type.as_str())
}

fn carries_enchant(equipment: &Equipment, effect_id: i32, slots: &[usize]) -> bool {
    slots.iter().any(|&slot| {
        equipment[slot].enchant.effect_id == effect_id || equipment[slot].temp_enchant == effect_id
    })
}

fn holds_item(equipment: &Equipment, item_id: i32, slots: &[usize]) -> bool {
    slots.iter().any(|&slot| equipment[slot].id == item_id)
}

/// Go `GetMainHandType` of the paladin and the warrior.
fn main_hand_type(equipment: &Equipment) -> &'static str {
    let mh = &equipment[slot::MAIN_HAND];
    if !mh.is_empty() && mh.hand_type == "HandTypeTwoHand" {
        "HandTypeTwoHand"
    } else {
        "HandTypeOneHand"
    }
}

impl super::env::Environment {
    /// Go `ItemSwap.SwapItems`.
    pub(crate) fn swap_items(&mut self, to_swapped: bool, is_reset: bool) {
        let unit = self.player;
        let Some(swap) = self.sim.item_swap(unit) else {
            return;
        };
        let valid = swap.swapped != to_swapped;
        if !valid && !is_reset {
            return;
        }
        let prepull = self.sim.current_time < 0;
        for slot in swap.slots.clone() {
            let weapon = [slot::MAIN_HAND, slot::OFF_HAND, slot::RANGED].contains(&slot);
            if !weapon && !is_reset && !prepull {
                continue;
            }
            self.swap_item(slot, prepull, is_reset);
            let callbacks = self.sim.item_swap(unit).expect("enabled").callbacks[slot].clone();
            for callback in &callbacks {
                self.run_swap_callback(callback);
            }
            // procs.go: each proc manager's callback rebuilds it for the weapons in the hands.
            if [slot::MAIN_HAND, slot::OFF_HAND, slot::RANGED].contains(&slot) {
                self.rebuild_proc_managers();
            }
        }
        // Go patch 135: the hands' weapon skill follows the equipment now worn.
        self.sim.update_weapon_skills(unit);
        if !valid {
            return;
        }
        let swap = self.sim.item_swap(unit).expect("enabled");
        let mut stats = if prepull {
            swap.all_slot_stats
        } else {
            swap.weapon_slot_stats
        };
        if swap.swapped {
            stats = stats.invert();
        }
        let (from, to) = (
            swap.auras[usize::from(swap.swapped)],
            swap.auras[usize::from(to_swapped)],
        );
        self.sim.add_dynamic_equip_stats(unit, &stats);
        // The swings stop and the global cooldown extends; preparation runs no fight.
        self.sim.deactivate(from);
        self.sim.activate(to);
        self.sim.item_swap_mut(unit).expect("enabled").swapped = to_swapped;
    }

    /// Go `ItemSwap.swapItem`.
    fn swap_item(&mut self, slot: usize, prepull: bool, is_reset: bool) {
        let unit = self.player;
        let swap = self.sim.item_swap(unit).expect("enabled");
        let old = self.sim.character(unit).equipment[slot].clone();
        let new = if is_reset {
            swap.original_equip[slot].clone()
        } else {
            swap.un_equipped[slot].clone()
        };
        let feral = swap.is_feral_druid;
        self.sim.character_mut(unit).equipment[slot] = new;
        self.sim.item_swap_mut(unit).expect("enabled").un_equipped[slot] = old;
        if prepull {
            return;
        }
        if slot == slot::MAIN_HAND && self.sim.unit(unit).auto_attacks.auto_swing_melee && !feral {
            let weapon = weapon_from_main_hand(&self.sim, unit);
            self.sim.unit_mut(unit).auto_attacks.mh = weapon;
        }
        // The off hand's attack follows the dual wield the swap leaves, and a shield blocks.
        if slot == slot::OFF_HAND && self.sim.unit(unit).auto_attacks.auto_swing_melee {
            let weapon = weapon_from_off_hand(&self.sim, unit);
            let dual_wielding = weapon.swing_speed != 0.0;
            let shield = self.sim.character(unit).equipment[slot::OFF_HAND].weapon_type
                == "WeaponTypeShield";
            let unit_state = self.sim.unit_mut(unit);
            unit_state.auto_attacks.oh = weapon;
            unit_state.auto_attacks.is_dual_wielding = dual_wielding;
            unit_state.pseudo_stats.can_block = shield;
        }
    }

    /// One callback, for the set now worn.
    fn run_swap_callback(&mut self, callback: &SwapCallback) {
        let unit = self.player;
        let equipment = self.sim.character(unit).equipment.clone();
        let sim = &mut self.sim;
        let set_aura = |sim: &mut Sim, aura: AuraId, on: bool, refresh: bool| {
            if !on {
                sim.deactivate(aura);
            } else if refresh || !sim.aura(aura).active {
                sim.activate(aura);
            }
        };
        match callback {
            SwapCallback::ItemUse { item_id, slots } => {
                let action = ActionId::item(*item_id);
                let Some(spell) = sim.get_spell(unit, &action) else {
                    return;
                };
                let aura = sim
                    .unit(unit)
                    .auras
                    .iter()
                    .copied()
                    .find(|aura| sim.aura(*aura).action_id.as_ref() == Some(&action));
                if let Some(aura) = aura {
                    sim.deactivate(aura);
                }
                if !holds_item(&equipment, *item_id, slots) {
                    sim.spell_mut(spell).flags.0 |= super::spell::SpellFlag::SWAPPED.0;
                    return;
                }
                sim.spell_mut(spell).flags.0 &= !super::spell::SpellFlag::SWAPPED.0;
                // An initialized swap sets the use's cooldown, which the export does not carry.
            }
            SwapCallback::ProcAura {
                aura,
                item_id,
                enchant_id,
                slots,
            } => {
                let on = if *item_id != 0 {
                    holds_item(&equipment, *item_id, slots)
                } else {
                    *enchant_id != 0 && carries_enchant(&equipment, *enchant_id, slots)
                };
                set_aura(sim, *aura, on, false);
            }
            SwapCallback::EnchantBuff {
                aura,
                enchant_id,
                slots,
            } => {
                if !carries_enchant(&equipment, *enchant_id, slots) {
                    sim.deactivate(*aura);
                }
            }
            SwapCallback::HandMod { id, hand_type } => {
                if main_hand_type(&equipment) == hand_type {
                    sim.activate_spell_mod(*id);
                } else {
                    sim.deactivate_spell_mod(*id);
                }
            }
            SwapCallback::Weaponmaster {
                armor_aura,
                armor_ignore,
                mh_crit,
                oh_crit,
            } => {
                let crit = ["WeaponTypeAxe", "WeaponTypePolearm"];
                for (id, hand) in [(*mh_crit, slot::MAIN_HAND), (*oh_crit, slot::OFF_HAND)] {
                    if weapon_type_in_hand(&equipment[hand], &crit) {
                        sim.activate_spell_mod(id);
                    } else {
                        sim.deactivate_spell_mod(id);
                    }
                }
                let on = weapon_type_in_hand(
                    &equipment[slot::MAIN_HAND],
                    &["WeaponTypeMace", "WeaponTypeStaff"],
                );
                let was = sim.aura(*armor_aura).active;
                set_aura(sim, *armor_aura, on, true);
                // The aura's gain and expiry move every attack table the warrior owns, which
                // the environment keeps.
                let now = self.sim.aura(*armor_aura).active;
                if was != now {
                    let delta = if now { *armor_ignore } else { -*armor_ignore };
                    let attacker = self.sim.unit(unit).unit_index as usize;
                    for table in &mut self.attack_tables[attacker] {
                        table.armor_ignore_factor += delta;
                    }
                }
            }
            SwapCallback::Deactivate { aura } => sim.deactivate(*aura),
            SwapCallback::WithoutShield { aura } => {
                if !sim.unit(unit).pseudo_stats.can_block {
                    sim.deactivate(*aura);
                }
            }
            SwapCallback::ImbueAura { aura, enchant_id } => {
                let on = equipment[slot::MAIN_HAND].temp_enchant == *enchant_id
                    || equipment[slot::OFF_HAND].temp_enchant == *enchant_id;
                set_aura(sim, *aura, on, true);
            }
            SwapCallback::Rockbiter { aura } => {
                let on = equipment[slot::MAIN_HAND].temp_enchant == 3021
                    || equipment[slot::OFF_HAND].temp_enchant == 3021;
                set_aura(sim, *aura, on, false);
            }
            SwapCallback::Flametongue { aura, slot } => {
                let swapped_out = sim
                    .unequipped_item(unit, *slot)
                    .map_or(0, |item| item.temp_enchant);
                if equipment[*slot].temp_enchant != 5 {
                    sim.deactivate(*aura);
                } else if swapped_out != 5 {
                    sim.activate(*aura);
                }
            }
            SwapCallback::MainHandImbue {
                pull_id,
                swap_id,
                effects,
            } => {
                let in_hand = equipment[slot::MAIN_HAND].id;
                for (auras, pull, swap) in effects {
                    let on = (in_hand == *pull_id && *pull) || (in_hand == *swap_id && *swap);
                    for aura in auras {
                        set_aura(sim, *aura, on, false);
                    }
                }
            }
            SwapCallback::DruidPaw => self.agent.item_swap_main_hand(&mut self.sim, unit),
        }
    }

    /// Rebuilds the proc managers on the player's auras an item swap's callbacks rebuild.
    fn rebuild_proc_managers(&mut self) {
        use super::procs::Rebuild;
        let unit = self.player;
        for aura in self.sim.unit(unit).auras.clone() {
            let Some(Rebuild::Rebuilt(chance, mask)) = self
                .sim
                .aura(aura)
                .dpm
                .as_ref()
                .and_then(|dpm| dpm.rebuild.clone())
            else {
                continue;
            };
            let sim = &self.sim;
            let current = |pred: &dyn Fn(&items::Item) -> bool| {
                super::shared_items::current_proc_mask_for(sim, unit, pred)
            };
            let mask_now = match &mask {
                ManagerMask::Static(mask) => *mask,
                ManagerMask::WeaponEnchant(id) => current(&|item| item.enchant.effect_id == *id),
                ManagerMask::WeaponEnchantWithin(id, within) => super::spell::ProcMask(
                    within.0 & current(&|item| item.enchant.effect_id == *id).0,
                ),
                ManagerMask::WeaponTempEnchant(id) => current(&|item| item.temp_enchant == *id),
                ManagerMask::WeaponEffect(id) => current(&|item| item.id == *id),
            };
            let (ppm, fixed) = match chance {
                ManagerChance::Ppm(ppm) => (ppm, 0.0),
                ManagerChance::Fixed(fixed) => (0.0, fixed),
            };
            let mut dpm = self
                .sim
                .new_dynamic_weapon_proc_manager(unit, ppm, fixed, mask_now);
            dpm.rebuild = Some(Rebuild::Rebuilt(chance, mask));
            self.sim.aura_mut(aura).dpm = Some(std::rc::Rc::new(dpm));
        }
    }

    /// Go `ItemSwap.reset`: back to the Main set as a reset swaps.
    pub(crate) fn item_swap_reset(&mut self) {
        let unit = self.player;
        let Some(swap) = self.sim.item_swap_mut(unit) else {
            return;
        };
        swap.initialized = false;
        self.swap_items(false, true);
        let swap = self.sim.item_swap_mut(unit).expect("enabled");
        swap.un_equipped = swap.swap_equip.clone();
        swap.initialized = true;
    }
}

impl Sim {
    /// Go `Character.AddDynamicEquipStats`.
    pub(crate) fn add_dynamic_equip_stats(&mut self, unit: UnitId, equip_stats: &Stats) {
        let mut stats = *equip_stats;
        for (stat, multiplier) in &self.character(unit).item_stat_multipliers {
            stats.0[*stat] *= multiplier;
        }
        self.add_stats_dynamic(unit, &stats);
        self.character_mut(unit).equip_cache_valid = false;
    }
}

/// Go `jsonDiff`: the changes from `a` to `b` as JSON pointers, an object's keys in byte order.
/// A key either side lacks, an array whose length differs and a value that differs are each one
/// change to `b`'s value; numbers compare as Go's float64 do.
fn json_diff(path: &str, a: &Value, b: &Value, out: &mut Vec<Value>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
            keys.sort();
            keys.dedup();
            for key in keys {
                let child = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
                match (x.get(key), y.get(key)) {
                    (Some(x), Some(y)) => json_diff(&child, x, y, out),
                    (_, y) => {
                        out.push(json!({"path": child, "value": y.cloned().unwrap_or(Value::Null)}))
                    }
                }
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            for (index, (x, y)) in x.iter().zip(y).enumerate() {
                json_diff(&format!("{path}/{index}"), x, y, out);
            }
        }
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (
                x.as_f64().unwrap_or(f64::NAN),
                y.as_f64().unwrap_or(f64::NAN),
            );
            if x.to_bits() != y.to_bits() {
                out.push(json!({"path": path, "value": b}));
            }
        }
        _ => {
            if a != b {
                out.push(json!({"path": path, "value": b}));
            }
        }
    }
}

/// Go `auraState`.
fn aura_state(on: bool, activate: &str, otherwise: &str) -> String {
    if on { activate } else { otherwise }.to_string()
}

/// Go `spellModOf`.
fn spell_mod_of(sim: &Sim, unit: UnitId, id: ModId) -> Result<Value, Refusal> {
    let modifier = sim.spell_mod(id);
    let spellbook = &sim.unit(unit).spellbook;
    let mut spells = Vec::new();
    for spell in &modifier.affected_spells {
        let position = spellbook
            .iter()
            .position(|own| own == spell)
            .ok_or_else(|| {
                Refusal::new(
                    "item_swap",
                    "an item swap switches a spell modifier on a pet's spell, which is unsupported"
                        .to_string(),
                )
            })?;
        spells.push(position);
    }
    let kind = match modifier.kind {
        super::spell_mod::SpellModType::DamageDonePct => "damage_done_pct",
        super::spell_mod::SpellModType::BonusCritPercent => "bonus_crit_percent",
        // Two-Handed Weapon Specialization's threat under push 113554 (Forever's rules).
        super::spell_mod::SpellModType::ThreatMultiplierPct => "threat_multiplier_pct",
        _ => {
            return Err(Refusal::new(
                "item_swap",
                "an item swap switches a spell modifier the runtime does not follow".to_string(),
            ))
        }
    };
    Ok(
        json!({"kind": kind, "value": modifier.float_value(), "spells": spells,
              "active": modifier.is_active}),
    )
}

/// Go `itemSwapCallbacks`: what each callback of a swapped slot does under each set.
fn export_callbacks(env: &Environment) -> Result<Vec<Value>, Refusal> {
    let unit = env.player;
    let sim = &env.sim;
    let swap = sim.item_swap(unit).expect("enabled");
    let (main, swapped) = (swap.set(false), swap.set(true));
    let label = |aura: &AuraId| sim.aura(*aura).label.clone();
    let mut out = Vec::new();
    // A callback registered for both hands runs for each, to the same end: it is exported once.
    let mut seen: Vec<String> = Vec::new();
    for &slot_index in &swap.slots {
        let slot = slot_name(slot_index);
        for callback in &swap.callbacks[slot_index] {
            let key = format!("{callback:?}");
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            let both = |state: &dyn Fn(&Equipment) -> String| (state(&main), state(&swapped));
            match callback {
                SwapCallback::DruidPaw => {}
                SwapCallback::ItemUse { item_id, slots } => {
                    let action = ActionId::item(*item_id);
                    let spell = sim.get_spell(unit, &action).ok_or_else(|| {
                        Refusal::new(
                            "item_swap",
                            format!("item {item_id}'s use is not registered"),
                        )
                    })?;
                    let position = sim
                        .unit(unit)
                        .spellbook
                        .iter()
                        .position(|own| *own == spell)
                        .expect("a registered spell");
                    let mut op = json!({"slot": slot, "kind": "item_use", "spell": position,
                        "main": holds_item(&main, *item_id, slots),
                        "swapped": holds_item(&swapped, *item_id, slots)});
                    if let Some(aura) = sim
                        .unit(unit)
                        .auras
                        .iter()
                        .find(|aura| sim.aura(**aura).action_id.as_ref() == Some(&action))
                    {
                        op["aura"] = json!(label(aura));
                    }
                    out.push(op);
                }
                SwapCallback::ProcAura {
                    aura,
                    item_id,
                    enchant_id,
                    slots,
                } => {
                    let (m, s) = both(&|e| {
                        let on = if *item_id != 0 {
                            holds_item(e, *item_id, slots)
                        } else {
                            *enchant_id != 0 && carries_enchant(e, *enchant_id, slots)
                        };
                        aura_state(on, "on", "off")
                    });
                    out.push(json!({"slot": slot, "kind": "aura", "aura": label(aura), "main": m, "swapped": s}));
                }
                SwapCallback::EnchantBuff {
                    aura,
                    enchant_id,
                    slots,
                } => {
                    let (m, s) = both(&|e| {
                        aura_state(carries_enchant(e, *enchant_id, slots), "keep", "off")
                    });
                    out.push(json!({"slot": slot, "kind": "aura", "aura": label(aura), "main": m, "swapped": s}));
                }
                SwapCallback::HandMod { id, hand_type } => {
                    out.push(json!({"slot": slot, "kind": "spell_mod", "mod": spell_mod_of(sim, unit, *id)?,
                        "main": main_hand_type(&main) == hand_type,
                        "swapped": main_hand_type(&swapped) == hand_type}));
                }
                SwapCallback::Weaponmaster {
                    armor_aura,
                    mh_crit,
                    oh_crit,
                    ..
                } => {
                    let mace = ["WeaponTypeMace", "WeaponTypeStaff"];
                    out.push(json!({"slot": slot, "kind": "aura", "aura": label(armor_aura),
                        "main": aura_state(weapon_type_in_hand(&main[slot::MAIN_HAND], &mace), "activate", "off"),
                        "swapped": aura_state(weapon_type_in_hand(&swapped[slot::MAIN_HAND], &mace), "activate", "off")}));
                    let crit = ["WeaponTypeAxe", "WeaponTypePolearm"];
                    for (id, hand) in [(*mh_crit, slot::MAIN_HAND), (*oh_crit, slot::OFF_HAND)] {
                        out.push(json!({"slot": slot, "kind": "spell_mod", "mod": spell_mod_of(sim, unit, id)?,
                            "main": weapon_type_in_hand(&main[hand], &crit),
                            "swapped": weapon_type_in_hand(&swapped[hand], &crit)}));
                    }
                }
                SwapCallback::Deactivate { aura } => {
                    out.push(json!({"slot": slot, "kind": "aura", "aura": label(aura), "main": "off", "swapped": "off"}));
                }
                SwapCallback::WithoutShield { aura } => {
                    let (m, s) = both(&|e| {
                        let shield = e[slot::OFF_HAND].weapon_type == "WeaponTypeShield";
                        aura_state(shield, "keep", "off")
                    });
                    out.push(json!({"slot": slot, "kind": "aura", "aura": label(aura), "main": m, "swapped": s}));
                }
                SwapCallback::ImbueAura { aura, enchant_id } => {
                    let (m, s) = both(&|e| {
                        let on = e[slot::MAIN_HAND].temp_enchant == *enchant_id
                            || e[slot::OFF_HAND].temp_enchant == *enchant_id;
                        aura_state(on, "activate", "off")
                    });
                    out.push(json!({"slot": slot, "kind": "aura", "aura": label(aura), "main": m, "swapped": s}));
                }
                SwapCallback::Rockbiter { aura } => {
                    let (m, s) = both(&|e| {
                        let on = e[slot::MAIN_HAND].temp_enchant == 3021
                            || e[slot::OFF_HAND].temp_enchant == 3021;
                        aura_state(on, "on", "off")
                    });
                    out.push(json!({"slot": slot, "kind": "aura", "aura": label(aura), "main": m, "swapped": s}));
                }
                SwapCallback::Flametongue { aura, .. } => {
                    out.push(
                        json!({"slot": slot, "kind": "aura_on_change", "aura": label(aura),
                        "main": main[slot::MAIN_HAND].temp_enchant == 5,
                        "swapped": swapped[slot::MAIN_HAND].temp_enchant == 5}),
                    );
                }
                SwapCallback::MainHandImbue {
                    pull_id,
                    swap_id,
                    effects,
                } => {
                    for (auras, pull, swap) in effects {
                        let in_hand = |e: &Equipment| {
                            let id = e[slot::MAIN_HAND].id;
                            (id == *pull_id && *pull) || (id == *swap_id && *swap)
                        };
                        for aura in auras {
                            out.push(json!({"slot": slot, "kind": "aura", "aura": label(aura),
                                "main": aura_state(in_hand(&main), "on", "off"),
                                "swapped": aura_state(in_hand(&swapped), "on", "off")}));
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Go's `proto.ItemSlot` names.
fn slot_name(slot_index: usize) -> &'static str {
    const NAMES: [&str; NUM_ITEM_SLOTS] = [
        "ItemSlotHead",
        "ItemSlotNeck",
        "ItemSlotShoulder",
        "ItemSlotBack",
        "ItemSlotChest",
        "ItemSlotWrist",
        "ItemSlotHands",
        "ItemSlotWaist",
        "ItemSlotLegs",
        "ItemSlotFeet",
        "ItemSlotFinger1",
        "ItemSlotFinger2",
        "ItemSlotTrinket1",
        "ItemSlotTrinket2",
        "ItemSlotMainHand",
        "ItemSlotOffHand",
        "ItemSlotRanged",
    ];
    NAMES[slot_index]
}

/// Go `itemSwapStatAuras`: the auras a callback switches whose gain or expiry changes the
/// character's stats, in callback order, each read from a reset simulation of its own, then the
/// stat aura of the swap's own equipment stats.
pub(crate) fn item_swap_stat_auras(env: &Environment) -> Vec<String> {
    if env.sim.item_swap(env.player).is_none() {
        return Vec::new();
    }
    let mut labels: Vec<String> = Vec::new();
    for op in export_callbacks(env).unwrap_or_default() {
        let Some(label) = op["aura"].as_str() else {
            continue;
        };
        if labels.iter().any(|known| known == label) {
            continue;
        }
        let mut fresh = env.fresh();
        let player = fresh.player;
        let aura = fresh.sim.get_aura(player, label).expect("the aura exists");
        let before = fresh.sim.unit(player).stats;
        if fresh.sim.aura(aura).active {
            fresh.sim.deactivate(aura);
        } else {
            fresh.sim.activate(aura);
        }
        if fresh.sim.unit(player).stats != before {
            labels.push(label.to_string());
        }
    }
    labels.push(SWAPPED_LABEL.to_string());
    labels
}

/// `addItemSwapEquipStats`: activating the swap's stat aura in a combination's simulation stands
/// for `AddDynamicEquipStats` with the swap's weapon slot stats.
pub(crate) fn add_item_swap_equip_stats(env: &mut Environment, aura: AuraId) {
    if env.sim.aura(aura).label != SWAPPED_LABEL {
        return;
    }
    let unit = env.player;
    let stats = env
        .sim
        .character(unit)
        .item_swap
        .as_ref()
        .expect("an item swap")
        .weapon_slot_stats;
    env.sim.add_dynamic_equip_stats(unit, &stats);
}

/// Go `itemSwapEffect`, or none without an item swap.
/// The classes whose weapon-dependent effects follow a swap: the Paladin's seals, the Warrior's
/// specializations, the Shaman's imbues and totems and the Druid's paws.
pub(crate) const ITEM_SWAP_CLASSES: [&str; 4] =
    ["ClassPaladin", "ClassWarrior", "ClassShaman", "ClassDruid"];

pub(crate) fn item_swap_effect(
    env: &Environment,
    digest: &str,
    scenario: &str,
    main: &Value,
) -> Result<Option<Value>, Refusal> {
    let unit = env.player;
    let Some(swap) = env.sim.item_swap(unit) else {
        return Ok(None);
    };
    let refuse = |reason: &str| Err(Refusal::new("item_swap", reason.to_string()));
    let class = env.sim.character(unit).class.as_str();
    if swap.slots != [slot::MAIN_HAND] {
        if swap.slots != [slot::MAIN_HAND, slot::OFF_HAND] || class != "ClassWarrior" {
            return refuse("an item swap of a slot other than the main hand alone is unsupported");
        }
        // A Warrior's two-hander for a one-hander and shield, or the other way: neither set
        // swings an off hand, so the swap changes the main hand's attack and what a shield gives.
        let dual_wields = |swapped: bool| {
            let equipment = swap.set(swapped);
            let off_hand = &equipment[slot::OFF_HAND];
            !off_hand.is_empty()
                && !matches!(
                    off_hand.weapon_type.as_str(),
                    "WeaponTypeShield" | "WeaponTypeOffHand"
                )
        };
        if dual_wields(false) || dual_wields(true) {
            return refuse("an item swap of an off hand weapon is unsupported");
        }
    }
    if !ITEM_SWAP_CLASSES.contains(&class) {
        return Err(Refusal::new(
            "item_swap",
            format!("an item swap of a {class} is not prepared yet"),
        ));
    }
    for aura in &env.sim.unit(unit).auras {
        let unaudited = env
            .sim
            .aura(*aura)
            .dpm
            .as_ref()
            .is_some_and(|dpm| matches!(dpm.rebuild, Some(super::procs::Rebuild::Unaudited)));
        if unaudited {
            return Err(Refusal::new(
                "item_swap",
                format!(
                    "an item swap with the proc manager of {} is not prepared yet",
                    env.sim.aura(*aura).label
                ),
            ));
        }
    }
    if let Some(what) = swap.unported.first() {
        return Err(Refusal::new(
            "item_swap",
            format!("an item swap with {what} is not prepared yet"),
        ));
    }
    let weapon_stats = swap.weapon_slot_stats;
    let callbacks = export_callbacks(env)?;
    // The prepared state with the swap set worn at the pull, as its difference from the reset's.
    let mut worn = env.fresh();
    worn.swap_items(true, false);
    let (swapped_state, _) = super::export::export_state(&mut worn, digest, scenario)?;
    let mut swapped = Vec::new();
    json_diff("", main, &swapped_state, &mut swapped);
    let flat = super::common_effects::flat_string;
    let mut effect = json!({
        "kind": "item_swap",
        "slots": swap.slots.iter().map(|&slot| slot_name(slot)).collect::<Vec<_>>(),
        "main_aura": MAIN_LABEL, "swapped_aura": SWAPPED_LABEL,
        "to_swapped_log": format!("Item Swap - Stats Change: {}", flat(&weapon_stats)),
        "to_main_log": format!("Item Swap - Stats Change: {}", flat(&weapon_stats.invert())),
        "callbacks": callbacks, "swapped": swapped,
    });
    let mut logs = Vec::new();
    for label in item_swap_stat_auras(env) {
        if label == SWAPPED_LABEL {
            continue;
        }
        let aura = env.sim.get_aura(unit, &label).expect("the aura exists");
        if let Some((gain, expire)) = &env.sim.aura(aura).stat_logs {
            logs.push(json!({"aura": label, "gain_logs": [gain], "expire_logs": [expire]}));
        }
    }
    effect["stat_aura_logs"] = json!(logs);
    // An aura the unit's reset activates that the swap's reset switches off for the Main set.
    let mut reset_activates: Vec<String> = Vec::new();
    for op in effect["callbacks"].as_array().expect("a list") {
        let (Some(label), true) = (op["aura"].as_str(), op["main"] == "off") else {
            continue;
        };
        if reset_activates.iter().any(|known| known == label) {
            continue;
        }
        let aura = env.sim.get_aura(unit, label).expect("the aura exists");
        let Some(on_reset) = env.sim.aura(aura).on_reset.clone() else {
            continue;
        };
        if env.sim.aura(aura).active {
            continue;
        }
        let mut probe = env.fresh();
        let probe_aura = probe
            .sim
            .get_aura(probe.player, label)
            .expect("the aura exists");
        on_reset(&mut probe.sim, probe_aura);
        if probe.sim.aura(probe_aura).active {
            reset_activates.push(label.to_string());
        }
    }
    effect["reset_activates"] = json!(reset_activates);
    let mut aura_effects = Vec::new();
    if let Some(aura) = env.sim.get_aura(unit, "Weaponmaster (Mace/Staff)") {
        let label = env.sim.aura(aura).label.clone();
        if effect["callbacks"]
            .as_array()
            .expect("a list")
            .iter()
            .any(|op| op["aura"] == label.as_str())
        {
            let factor = swap
                .callbacks
                .iter()
                .flatten()
                .find_map(|callback| match callback {
                    SwapCallback::Weaponmaster { armor_ignore, .. } => Some(*armor_ignore),
                    _ => None,
                })
                .unwrap_or_default();
            aura_effects.push(json!({"aura": label, "armor_ignore_factor": factor}));
        }
    }
    effect["aura_effects"] = json!(aura_effects);
    Ok(Some(effect))
}

impl Sim {
    /// Go `ItemSwap.EligibleSlotsForEffect`.
    pub(crate) fn eligible_slots_for_effect(&self, unit: UnitId, effect_id: i32) -> Vec<usize> {
        (0..NUM_ITEM_SLOTS)
            .filter(|slot| self.item_swap_holds_enchant(unit, *slot, effect_id))
            .collect()
    }

    /// Go `ItemSwap.RegisterProc`: the trigger aura is up while the item is worn.
    pub(crate) fn register_item_proc(&mut self, unit: UnitId, item_id: i32, aura: AuraId) {
        let slots = super::shared_items::eligible_slots_for_item(self, unit, item_id);
        self.register_item_proc_with_slots(unit, item_id, aura, slots);
    }

    /// Go `ItemSwap.RegisterProcWithSlots`.
    pub(crate) fn register_item_proc_with_slots(
        &mut self,
        unit: UnitId,
        item_id: i32,
        aura: AuraId,
        slots: Vec<usize>,
    ) {
        self.register_item_swap_callback(
            unit,
            &slots.clone(),
            SwapCallback::ProcAura {
                aura,
                item_id,
                enchant_id: 0,
                slots,
            },
        );
    }

    /// Go `ItemSwap.RegisterEnchantProc`: the trigger aura is up while a slot carries the
    /// enchant.
    pub(crate) fn register_enchant_proc(&mut self, unit: UnitId, enchant_id: i32, aura: AuraId) {
        let slots = self.eligible_slots_for_effect(unit, enchant_id);
        self.register_enchant_proc_with_slots(unit, enchant_id, aura, slots);
    }

    /// Go `ItemSwap.RegisterEnchantProcWithSlots`.
    pub(crate) fn register_enchant_proc_with_slots(
        &mut self,
        unit: UnitId,
        enchant_id: i32,
        aura: AuraId,
        slots: Vec<usize>,
    ) {
        self.register_item_swap_callback(
            unit,
            &slots.clone(),
            SwapCallback::ProcAura {
                aura,
                item_id: 0,
                enchant_id,
                slots,
            },
        );
    }

    /// Go `ItemSwap.RegisterWeaponEnchantBuff`.
    pub(crate) fn register_weapon_enchant_buff(
        &mut self,
        unit: UnitId,
        aura: AuraId,
        enchant_id: i32,
    ) {
        self.register_enchant_buff_with_slots(unit, aura, enchant_id, WEAPON_SLOTS.to_vec());
    }

    /// Go `ItemSwap.RegisterEnchantBuffWithSlots`.
    pub(crate) fn register_enchant_buff_with_slots(
        &mut self,
        unit: UnitId,
        aura: AuraId,
        enchant_id: i32,
        slots: Vec<usize>,
    ) {
        self.register_item_swap_callback(
            unit,
            &slots.clone(),
            SwapCallback::EnchantBuff {
                aura,
                enchant_id,
                slots,
            },
        );
    }
}
