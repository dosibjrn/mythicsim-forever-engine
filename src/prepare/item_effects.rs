//! Go sim/core/item_effects.go and item_sets.go: effects Go registers in code for items,
//! enchants and set bonuses.

use super::env::Environment;
use super::Refusal;

/// Go `Equipment.applyItemEffects` and `registerEquipSpeedAuras`.
pub(crate) fn apply_item_effects(env: &mut Environment) -> Result<(), Refusal> {
    let unit = env.player;
    let tables = crate::data::tables::tables();
    let class = env.sim.character(unit).class.clone();
    let mut seen_items = Vec::new();
    let mut seen_enchants = Vec::new();
    // The worn items, then with an item swap the swap set's (`unEquippedItems`), each item and
    // enchant effect once.
    let mut items: Vec<super::items::Item> = env.sim.character(unit).equipment.to_vec();
    if let Some(swap) = env.sim.item_swap(unit) {
        items.extend(swap.un_equipped.iter().cloned());
    }
    for item in items.iter() {
        let usable = item.class_allowlist.is_empty() || item.class_allowlist.contains(&class);
        // Under Forever's rules an item the reference has no effect for may still carry a
        // chance on hit (forever_items.rs `CHANCE_ON_HIT_ITEMS`).
        let has_effect = tables.item_effect_ids.contains(&item.id)
            || (env.sim.rules == super::sim::Rules::Forever
                && super::forever_items::has_chance_on_hit_item(item.id));
        if has_effect && !seen_items.contains(&item.id) && usable {
            seen_items.push(item.id);
            if !super::items_registry::apply_item_effect(env, item.id)? {
                return Err(Refusal::new(
                    "item_effect",
                    format!("item {} has an effect not prepared yet", item.id),
                ));
            }
        }
        let enchant = item.enchant.effect_id;
        if tables.enchant_effect_ids.contains(&enchant) && !seen_enchants.contains(&enchant) {
            seen_enchants.push(enchant);
            if !super::items_registry::apply_enchant_effect(env, enchant)? {
                return Err(Refusal::new(
                    "enchant_effect",
                    format!("enchant {enchant} has an effect not prepared yet"),
                ));
            }
        }
    }
    env.sim.register_equip_speed_auras(unit);
    audit_item_swap_effects(env, &seen_items, &seen_enchants);
    Ok(())
}

/// With an item swap, every item and enchant effect of a swapped slot's weapon, in either set,
/// must have registered its swap callback: a registration preparation follows. Any other is one
/// whose swap preparation does not know, and refuses the swap.
fn audit_item_swap_effects(env: &mut Environment, items: &[i32], enchants: &[i32]) {
    use super::item_swap::SwapCallback;
    let unit = env.player;
    let Some(swap) = env.sim.item_swap(unit) else {
        return;
    };
    let mut unported = Vec::new();
    for &slot in &swap.slots {
        for set in [&swap.original_equip, &swap.swap_equip] {
            let item = &set[slot];
            let follows =
                |pred: &dyn Fn(&SwapCallback) -> bool| swap.callbacks.iter().flatten().any(pred);
            if item.id != 0 && items.contains(&item.id) {
                let id = item.id;
                let registered = follows(
                    &|callback| matches!(callback, SwapCallback::ProcAura { item_id, .. } | SwapCallback::ItemUse { item_id, .. } if *item_id == id),
                );
                if !registered {
                    unported.push(format!("the effect of item {id}"));
                }
            }
            let enchant = item.enchant.effect_id;
            if enchant != 0 && enchants.contains(&enchant) {
                let registered = follows(&|callback| match callback {
                    SwapCallback::ProcAura { enchant_id, .. }
                    | SwapCallback::EnchantBuff { enchant_id, .. } => *enchant_id == enchant,
                    _ => false,
                });
                if !registered {
                    unported.push(format!("the effect of enchant {enchant}"));
                }
            }
        }
    }
    for what in unported {
        env.sim.item_swap_unported(unit, what);
    }
}

/// Go `applyItemSetBonusEffects`: in item_sets.rs.
pub(crate) fn apply_item_set_bonus_effects(env: &mut Environment) -> Result<(), Refusal> {
    super::item_sets::apply_item_set_bonus_effects(env)
}
