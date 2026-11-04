use forever_engine::native_player_context;
use serde_json::Value;
use std::process::Command;

fn number(value: &Value) -> f64 {
    value.as_f64().expect("expected numeric source-context value")
}

#[test]
fn dwarf_paladin_l20_context_resolves_source_stats_and_worn_mace() {
    let context = native_player_context("paladin", "dwarf", 20);
    assert_eq!(context["status"], "ok");
    assert_eq!(context["context_kind"], "source_backed_baseline_context_not_full_native_sim_actor");
    assert_eq!(context["source_scope"], "inherited_vanilla_classic_not_forever_verified");
    assert_eq!(context["class"], "paladin");
    assert_eq!(context["race"], "dwarf");
    assert_eq!(context["level"], 20);
    assert_eq!(number(&context["base_stats"]["strength"]), 47.0);
    assert_eq!(number(&context["base_stats"]["agility"]), 28.0);
    assert_eq!(number(&context["base_stats"]["stamina"]), 46.0);
    assert_eq!(number(&context["base_stats"]["intellect"]), 32.0);
    assert_eq!(number(&context["base_stats"]["spirit"]), 35.0);
    assert_eq!(number(&context["base_stats"]["attack_power"]), 134.0);
    assert_eq!(number(&context["base_stats"]["armor"]), 56.0);
    assert_eq!(number(&context["resources"]["base_health"]), 191.0);
    assert_eq!(number(&context["resources"]["base_mana"]), 412.0);
    assert_eq!(number(&context["resources"]["max_health"]), 471.0);
    assert_eq!(number(&context["resources"]["max_mana"]), 612.0);
    assert_eq!(context["weapon"]["item_id"], 36);
    assert_eq!(context["weapon"]["name"], "Worn Mace");
    assert_eq!(context["weapon"]["required_level"], 1);
    assert_eq!(context["weapon"]["weapon_type"], "WeaponTypeMace");
    assert_eq!(context["weapon"]["hand_type"], "HandTypeMainHand");
    assert_eq!(context["weapon"]["proficiency_spell_id"], 198);
    assert_eq!(context["weapon"]["proficiency_name"], "One-Handed Maces");
    assert_eq!(number(&context["weapon"]["weapon_speed_s"]), 1.9);
    assert_eq!(number(&context["weapon"]["weapon_damage_min"]), 1.0);
    assert_eq!(number(&context["weapon"]["weapon_damage_max"]), 3.0);
    assert!((number(&context["weapon"]["weapon_dps"]) - (2.0 / 1.9)).abs() < 1e-12);
    assert_eq!(
        context["provenance"]["primary_stats"]["commit"],
        "8ec338a1704e7dcb1c0213eb7ed58f9231ade40f"
    );
}

#[test]
fn human_paladin_l20_context_is_also_supported() {
    let context = native_player_context("paladin", "human", 20);
    assert_eq!(context["status"], "ok");
    assert_eq!(number(&context["base_stats"]["strength"]), 45.0);
    assert_eq!(number(&context["base_stats"]["agility"]), 32.0);
    assert_eq!(number(&context["base_stats"]["stamina"]), 43.0);
    assert_eq!(number(&context["base_stats"]["intellect"]), 33.0);
    assert_eq!(number(&context["base_stats"]["spirit"]), 35.0);
    assert_eq!(number(&context["base_stats"]["attack_power"]), 130.0);
    assert_eq!(number(&context["base_stats"]["armor"]), 64.0);
    assert_eq!(number(&context["resources"]["max_health"]), 441.0);
    assert_eq!(number(&context["resources"]["max_mana"]), 627.0);
    assert_eq!(context["weapon"]["item_id"], 36);
}

#[test]
fn player_context_refuses_unsupported_classes_races_and_levels() {
    for (class, race, level) in [
        ("priest", "dwarf", 20),
        ("paladin", "gnome", 20),
        ("paladin", "dwarf", 19),
        ("paladin", "dwarf", 60),
    ] {
        let context = native_player_context(class, race, level);
        assert_eq!(context["status"], "refused", "{class}/{race}/{level}");
        assert!(context["refusal"]["code"].is_string());
    }
}

#[test]
fn player_context_cli_defaults_to_the_documented_dwarf_baseline() {
    let output = Command::new(env!("CARGO_BIN_EXE_forever-engine"))
        .args(["player-context", "--class", "paladin", "--level", "20"])
        .output()
        .expect("native player-context CLI should start");
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let context: Value = serde_json::from_slice(&output.stdout).expect("CLI emits JSON");
    assert_eq!(context["status"], "ok");
    assert_eq!(context["race"], "dwarf");
    assert_eq!(context["weapon"]["item_id"], 36);
}
