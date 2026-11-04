//! The weapon procs of sim/common/itemhelpers/weaponprocs.go: a chance on hit that casts the spell
//! of a client row, an extra attack, an armor debuff or an aura. Their Go results and first-fight
//! logs are compared with the rest of the fixture family; these tests keep what each mechanism
//! has to do in a fight, and what the gate refuses.

use forever_engine::{contracts::prepared_v2::PreparedV2, prepared_coverage};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::Path};

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("fixtures/mage/prepared-v2/{name}.prepared.json"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn first_fight_log(mut value: Value) -> String {
    value["sim"]["iterations"] = json!(1);
    value["sim"]["debug_first_iteration"] = json!(true);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    let report =
        forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
    report.result["logs"].as_str().unwrap().to_string()
}

/// The refusals of a valid prepared input, as `code: reason`.
fn refusals(value: Value) -> Vec<String> {
    crate::refusal_codes(value)
        .into_iter()
        .map(|(code, reason)| format!("{code}: {reason}"))
        .collect()
}

fn effect<'a>(value: &'a Value, aura: &str) -> &'a Value {
    value["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["trigger_aura"] == aura)
        .unwrap_or_else(|| panic!("no effect for {aura}"))
}

fn numbers(value: &Value) -> Vec<f64> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|number| number.as_f64().unwrap())
        .collect()
}

fn effect_index(value: &Value, aura: &str) -> usize {
    value["effects"]
        .as_array()
        .unwrap()
        .iter()
        .position(|effect| effect["trigger_aura"] == aura)
        .unwrap()
}

/// How many lines name the spell on each target, among those with the text.
fn hits_by_target(logs: &str, spell: &str, text: &str) -> BTreeMap<u32, usize> {
    let mut hits = BTreeMap::new();
    for target in 1..=5 {
        let prefix = format!("[Target {target}] {{SpellID: {spell}}} ");
        let count = logs
            .lines()
            .filter(|line| line.contains(&prefix) && line.contains(text))
            .count();
        if count > 0 {
            hits.insert(target, count);
        }
    }
    hits
}

/// Every weapon proc fixture and the trigger aura of its weapon.
const FIXTURES: [(&str, &str); 18] = [
    (
        "production-warrior-masterwork-stormhammer",
        "Masterwork Stormhammer Proc",
    ),
    (
        "warrior-3-targets-masterwork-stormhammer",
        "Masterwork Stormhammer Proc",
    ),
    ("warrior-bloodfist", "Bloodfist Proc"),
    ("warrior-alcors-sunrazor", "Alcor's Sunrazor Proc"),
    ("warrior-bonechill-hammer", "Bonechill Hammer Proc"),
    ("warrior-plaguefang", "Plaguefang Proc"),
    ("warrior-3-targets-stinging-viper", "Stinging Viper Proc"),
    ("warrior-flurry-axe", "Flurry Axe Proc"),
    ("warrior-3-targets-annihilator", "Annihilator"),
    (
        "warrior-sword-of-zeal-bloodfist-off-hand",
        "Sword of Zeal Proc",
    ),
    ("warrior-lobotomizer", "The Lobotomizer Proc"),
    ("warrior-ebon-hilt-of-marduk", "Ebon Hilt of Marduk Proc"),
    ("arms-warrior-3-targets-flame-wrath", "Flame Wrath Proc"),
    (
        "marksmanship-hunter-barbaric-crossbow",
        "Barbaric Crossbow Proc",
    ),
    ("marksmanship-hunter-venomstrike", "Venomstrike Proc"),
    ("retribution-paladin-wolfsbane", "Wolfsbane Proc"),
    (
        "combat-rogue-glacial-blade-electrified-dagger",
        "Glacial Blade Proc",
    ),
    (
        "protection-warrior-3-targets-masterwork-stormhammer",
        "Masterwork Stormhammer Proc",
    ),
];

/// Each weapon's trigger aura hears hits, so its effect has to claim it.
#[test]
fn a_weapon_proc_needs_its_effect() {
    for (name, aura) in FIXTURES {
        let original = fixture(name);
        let prepared: PreparedV2 = serde_json::from_value(original.clone()).unwrap();
        assert_eq!(prepared_coverage(&prepared), Vec::<String>::new(), "{name}");
        let mut value = original;
        let index = effect_index(&value, aura);
        value["effects"].as_array_mut().unwrap().remove(index);
        assert!(
            refusals(value).contains(&format!(
                "aura_listener_unclaimed: player aura \"{aura}\" listens to combat events \
                 without an effect"
            )),
            "{name}"
        );
    }
}

/// A chain rolls once for each target it reaches, from the hit target on: three targets take
/// the Chain Lightning of Masterwork Stormhammer once a proc, which flies to them first.
#[test]
fn a_chain_hits_every_target_it_reaches() {
    let value = fixture("warrior-3-targets-masterwork-stormhammer");
    let chain = &effect(&value, "Masterwork Stormhammer Proc")["chain"];
    assert_eq!(chain["targets"], 3);
    // The client's float32 70%, widened as Go widens it.
    assert_eq!(chain["amp"], f64::from(0.7_f32));
    let spell = effect(&value, "Masterwork Stormhammer Proc")["spell"]
        .as_u64()
        .unwrap() as usize;
    assert_eq!(value["player"]["spells"][spell]["missile_speed"], 20.0);

    let logs = first_fight_log(value);
    let hits = hits_by_target(&logs, "16921", "damage (SpellSchool");
    let missed = hits_by_target(&logs, "16921", "Miss");
    assert_eq!(hits.len(), 3, "{hits:?}");
    // Each jump rolls a hit, which may miss.
    let rolls: Vec<usize> = (1..=3)
        .map(|target| hits[&target] + missed.get(&target).copied().unwrap_or(0))
        .collect();
    assert!(rolls.iter().all(|count| *count == rolls[0]), "{rolls:?}");
}

/// An area hit is calculated on every target with a roll of its own: Flame Wrath's Fire hit
/// reaches the three targets alike.
#[test]
fn an_area_hit_rolls_every_target() {
    let value = fixture("arms-warrior-3-targets-flame-wrath");
    let area = &effect(&value, "Flame Wrath Proc")["area"];
    assert_eq!(area["max_targets"], 0);
    assert_eq!(area["splits"], false);
    assert_eq!(area["aoe_cap_multiplier"], 1.0);
    let logs = first_fight_log(value);
    let hits = hits_by_target(&logs, "16559", "damage (SpellSchool");
    let missed = hits_by_target(&logs, "16559", "Miss");
    assert_eq!(hits.len(), 3, "{hits:?}");
    let totals: Vec<usize> = (1..=3)
        .map(|target| hits[&target] + missed.get(&target).copied().unwrap_or(0))
        .collect();
    assert!(totals.iter().all(|total| *total == totals[0]), "{totals:?}");
}

/// A row with only a damage over time goes on unrolled and ticks: it deals no hit of its own.
#[test]
fn a_damage_over_time_alone_ticks_without_a_hit() {
    let value = fixture("warrior-plaguefang");
    let periodic = &effect(&value, "Plaguefang Proc")["periodic"];
    assert_eq!(periodic["with_direct"], false);
    assert_eq!(periodic["tick_outcome"], "tick_magic_hit_and_crit");
    let logs = first_fight_log(value);
    let lines: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("[Target 1] {SpellID: 1309315}"))
        .filter(|line| !line.contains("[DEBUG]"))
        .collect();
    assert!(!lines.is_empty());
    for line in lines {
        assert!(line.contains(" tick "), "{line}");
    }
}

/// The dot goes on the target the proc hit, whichever it is.
#[test]
fn a_damage_over_time_follows_the_target_it_hit() {
    let logs = first_fight_log(fixture("warrior-3-targets-stinging-viper"));
    let ticks = hits_by_target(&logs, "1291663", " tick ");
    assert!(ticks.len() >= 2, "{ticks:?}");
}

/// A hit of the melee defense type rolls the melee special table with the target's armor, and
/// one of the ranged defense type the ranged table.
#[test]
fn a_hit_rolls_the_table_of_its_defense_type() {
    for (name, aura, outcome, defense) in [
        (
            "warrior-bloodfist",
            "Bloodfist Proc",
            "melee_special_hit_and_crit",
            "DefenseTypeMelee",
        ),
        (
            "marksmanship-hunter-barbaric-crossbow",
            "Barbaric Crossbow Proc",
            "ranged_hit_and_crit",
            "DefenseTypeRanged",
        ),
        (
            "marksmanship-hunter-venomstrike",
            "Venomstrike Proc",
            "ranged_hit_and_crit",
            "DefenseTypeRanged",
        ),
    ] {
        let value = fixture(name);
        let proc = effect(&value, aura);
        assert_eq!(proc["outcome"], outcome, "{name}");
        let spell = proc["spell"].as_u64().unwrap() as usize;
        assert_eq!(
            value["player"]["spells"][spell]["defense_type"], defense,
            "{name}"
        );
        // The table must be the one the spell's defense type names.
        let mut wrong = value.clone();
        let index = effect_index(&wrong, aura);
        wrong["effects"][index]["outcome"] = json!(if defense == "DefenseTypeMelee" {
            "ranged_hit_and_crit"
        } else {
            "melee_special_hit_and_crit"
        });
        let reasons = refusals(wrong);
        assert!(
            reasons
                .iter()
                .any(|reason| reason.starts_with("proc_unsupported: ")
                    && reason.contains(&format!("{aura}'s hit is not rolled"))),
            "{name}: {reasons:?}"
        );
    }
}

/// The names the exporter writes are the only ones the runtime rolls, and a row with a spread
/// hit leaves no damage over time.
#[test]
fn a_damage_proc_names_only_what_the_runtime_rolls() {
    let reason = |value: Value, aura: &str| -> Vec<String> {
        refusals(value)
            .into_iter()
            .filter(|reason| reason.contains(&format!("{aura}'s hit is not rolled")))
            .collect()
    };
    let original = fixture("warrior-plaguefang");
    assert!(reason(original.clone(), "Plaguefang Proc").is_empty());

    let mut value = original.clone();
    let index = effect_index(&value, "Plaguefang Proc");
    value["effects"][index]["periodic"]["tick_outcome"] = json!("tick_magic_hit");
    assert_eq!(
        reason(value, "Plaguefang Proc"),
        [
            "proc_unsupported: Plaguefang Proc's hit is not rolled: unknown damage proc tick \
          outcome tick_magic_hit"
        ]
    );

    // A missile that carries only a damage over time is not a hit the runtime waits for.
    let mut value = original.clone();
    let index = effect_index(&value, "Plaguefang Proc");
    let spell = value["effects"][index]["spell"].as_u64().unwrap() as usize;
    value["player"]["spells"][spell]["missile_speed"] = json!(20.0);
    assert_eq!(
        reason(value, "Plaguefang Proc"),
        [
            "proc_unsupported: Plaguefang Proc's hit is not rolled: a damage over time alone \
          needs its dot and no missile"
        ]
    );

    // A dot with no dot on the spell has nothing to tick.
    let mut value = original;
    let index = effect_index(&value, "Plaguefang Proc");
    let spell = value["effects"][index]["spell"].as_u64().unwrap() as usize;
    value["player"]["spells"][spell]["dot"] = Value::Null;
    assert_eq!(
        reason(value, "Plaguefang Proc"),
        [
            "proc_unsupported: Plaguefang Proc's hit is not rolled: a damage over time alone \
          needs its dot and no missile"
        ]
    );

    // A chain and a damage over time together are not a row Go builds.
    let mut value = fixture("production-warrior-masterwork-stormhammer");
    let index = effect_index(&value, "Masterwork Stormhammer Proc");
    value["effects"][index]["periodic"] = json!({
        "tick_base": 1.0, "tick_outcome": "tick", "with_direct": true,
    });
    assert_eq!(
        reason(value, "Masterwork Stormhammer Proc"),
        [
            "proc_unsupported: Masterwork Stormhammer Proc's hit is not rolled: a spread hit \
          leaves no damage over time"
        ]
    );
}

/// Each hand rolls its own weapon's proc: the weapon proc manager gives a chance only to the
/// spells that swing the hand holding the weapon.
#[test]
fn a_weapon_proc_hears_the_hits_of_its_own_hand() {
    let value = fixture("warrior-sword-of-zeal-bloodfist-off-hand");
    let spells = value["player"]["spells"].as_array().unwrap();
    let masks_of = |position: &Value| -> Vec<&str> {
        spells[position.as_u64().unwrap() as usize]["proc_mask"]
            .as_array()
            .unwrap()
            .iter()
            .map(|mask| mask.as_str().unwrap())
            .collect()
    };
    for (aura, hands) in [
        (
            "Sword of Zeal Proc",
            ["ProcMaskMeleeMHAuto", "ProcMaskMeleeMHSpecial"],
        ),
        (
            "Bloodfist Proc",
            ["ProcMaskMeleeOHAuto", "ProcMaskMeleeOHSpecial"],
        ),
    ] {
        let proc = effect(&value, aura);
        let chances = proc["chances"].as_array().unwrap();
        assert!(!chances.is_empty(), "{aura}");
        for chance in chances {
            let masks = masks_of(&chance["spell"]);
            assert!(
                masks.iter().any(|mask| hands.contains(mask)),
                "{aura}: {masks:?}"
            );
        }
    }
}

/// Sword of Zeal's aura adds 10 physical damage, which the off hand's Wound takes as Go's
/// CalcDamage adds it to a physical spell, and the combinations carry it.
#[test]
fn a_stat_aura_of_a_weapon_adds_physical_damage() {
    let value = fixture("warrior-sword-of-zeal-bloodfist-off-hand");
    let stat_auras = value["effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|effect| effect["kind"] == "stat_auras")
        .unwrap();
    assert!(stat_auras["auras"]
        .as_array()
        .unwrap()
        .contains(&json!("Sword of Zeal")));
    assert!(stat_auras["changed"]
        .as_array()
        .unwrap()
        .contains(&json!("PhysicalDamage")));
    let zeal = effect(&value, "Sword of Zeal Proc");
    assert_eq!(zeal["kind"], "stat_proc");
    assert!(zeal.get("gain_log").is_none());

    let logs = first_fight_log(value);
    let bases: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("{SpellID: 16433} [DEBUG]"))
        .filter_map(|line| line.split("BaseDamage:").nth(1))
        .filter_map(|rest| rest.split(',').next())
        .collect();
    assert!(bases.contains(&"35.0"), "{bases:?}");
    assert!(bases.contains(&"45.0"), "{bases:?}");
}

/// Annihilator's Armor Shatter takes 165 armor a stack, up to 3, from the target the proc hit.
#[test]
fn an_armor_debuff_applies_to_the_target_it_hit_at_once() {
    let value = fixture("warrior-3-targets-annihilator");
    let proc = effect(&value, "Annihilator");
    assert_eq!(proc["kind"], "armor_debuff_proc");
    assert_eq!(proc["immediate"], true);
    assert_eq!(proc["aura"], "Armor Shatter 16928");
    assert_eq!(
        numbers(&proc["armor_by_stacks"]),
        [0.0, -165.0, -330.0, -495.0]
    );

    let logs = first_fight_log(value);
    let gained: Vec<u32> = (1..=3)
        .filter(|target| {
            let line = format!("[Target {target}] Aura gained: {{SpellID: 16928}}");
            logs.lines().any(|candidate| candidate.contains(&line))
        })
        .collect();
    assert!(gained.len() >= 2, "{gained:?}");
    assert!(logs.contains("{SpellID: 16928} stacks: 1 --> 2"));
}

/// Flurry Axe's proc casts a spell that grants one extra main hand attack at once.
#[test]
fn a_weapon_proc_grants_an_extra_main_hand_attack() {
    let value = fixture("warrior-flurry-axe");
    let proc = effect(&value, "Flurry Axe Proc");
    assert_eq!(proc["kind"], "extra_attack_proc");
    assert_eq!(proc["attacks"], 1);
    let logs = first_fight_log(value);
    let lines: Vec<&str> = logs.lines().collect();
    let procs: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains("Casting {SpellID: 18797}"))
        .map(|(position, _)| position)
        .collect();
    assert!(!procs.is_empty());
    // The main hand swings again at the very moment of the proc.
    for position in procs {
        let moment = lines[position].split(']').next().unwrap();
        assert!(
            lines[position..]
                .iter()
                .take_while(|line| line.starts_with(moment))
                .any(|line| line.contains("Casting {OtherID: 3, Tag: 1}")),
            "{}",
            lines[position]
        );
    }
}

/// The Lobotomizer rolls a Go literal 200 to 300 on the magic table with a crit, for a
/// physical spell of the melee defense type: its hits stay inside the range, less the armor the
/// physical spell takes, and it crits.
#[test]
fn the_lobotomizer_rolls_its_literal_range() {
    let mut value = fixture("warrior-lobotomizer");
    let proc = effect(&value, "The Lobotomizer Proc");
    assert_eq!(numbers(&proc["roll"]), [200.0, 300.0]);
    assert!(proc.get("outcome").is_none());
    value["sim"]["iterations"] = json!(300);
    let prepared: PreparedV2 = serde_json::from_value(value).unwrap();
    let report =
        forever_engine::simulate_prepared(&prepared, forever_engine::RngMode::Stochastic).unwrap();
    let action = report.result["raidMetrics"]["parties"][0]["players"][0]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["id"]["spellId"] == 1290950)
        .map(|action| &action["targets"][0])
        .unwrap();
    assert!(action["hits"].as_f64().unwrap() > 0.0, "{action}");
    assert!(action["crits"].as_f64().unwrap() > 0.0);
    // Armor takes about a tenth of the 200 and the target's modifiers add a few percent.
    let (low, high) = (
        action["hitRange"]["min"].as_f64().unwrap(),
        action["hitRange"]["max"].as_f64().unwrap(),
    );
    assert!((170.0..=200.0).contains(&low), "{low}");
    assert!((270.0..=330.0).contains(&high), "{high}");
}

/// Ebon Hilt's Corruption rolls only its application: it lands without damage and starts 3
/// ticks of 28.
#[test]
fn a_dot_that_rolls_its_application_ticks_after_it_landed() {
    let value = fixture("warrior-ebon-hilt-of-marduk");
    let periodic = &effect(&value, "Ebon Hilt of Marduk Proc")["periodic"];
    assert_eq!(periodic["application"], "magic_hit");
    assert_eq!(periodic["tick_base"], 28.0);
    let logs = first_fight_log(value);
    assert!(logs
        .lines()
        .any(|line| line.contains("{SpellID: 18656} Hit for 0.000 damage")));
    assert!(logs
        .lines()
        .any(|line| line.contains("{SpellID: 18656} tick Hit for 28.000 damage")));

    let mut wrong = fixture("warrior-ebon-hilt-of-marduk");
    let index = effect_index(&wrong, "Ebon Hilt of Marduk Proc");
    wrong["effects"][index]["periodic"]["application"] = json!("magic_hit_and_crit");
    assert!(refusals(wrong).iter().any(|reason| reason.contains(
        "Ebon Hilt of Marduk Proc's hit is not rolled: unknown damage proc application"
    )));
}

/// Two spells of one name put their dots on one aura in Go, as Plaguefang's and Stinging Viper's
/// Poison do when both are worn; the runtime binds an aura to one dot, so it refuses them.
#[test]
fn two_dots_that_share_an_aura_are_refused() {
    let original = fixture("warrior-plaguefang");
    assert!(!refusals(original.clone())
        .iter()
        .any(|reason| reason.contains("share the aura")));
    let mut value = original;
    let spells = value["player"]["spells"].as_array_mut().unwrap();
    let poison = spells
        .iter()
        .find(|spell| spell["action_id"]["spell_id"] == 1309315)
        .unwrap()
        .clone();
    let mut second = poison.clone();
    second["action_id"]["spell_id"] = json!(1291663);
    spells.push(second);
    let aura = poison["dot"]["aura_label"].as_str().unwrap();
    assert!(refusals(value).contains(&format!(
        "effect_unimplemented: the dots of spell 1309315 and spell 1291663 share the aura {aura}"
    )));
}

/// Fork patch 126: Frost Oil is a temporary enchantment whose client row casts a Frostbolt on a
/// landed hit of the oiled weapon, at the flat 10% the row states, at once on the unit hit. Each
/// oiled hand has its own listener, which hears the spells of that hand.
#[test]
fn frost_oil_casts_a_frostbolt_on_the_hits_of_its_hand() {
    for name in [
        "combat-rogue-frost-oil",
        "combat-rogue-frost-oil-beside-poisons",
    ] {
        let value = fixture(name);
        let main = effect(&value, "Frost Oil (main hand)");
        let off = effect(&value, "Frost Oil (off hand)");
        for oil in [main, off] {
            assert_eq!(oil["kind"], "spell_data_damage_proc", "{name}");
            assert_eq!(oil["proc_chance"], 0.1, "{name}");
            assert_eq!(oil["landed_only"], true, "{name}");
            assert!(oil.get("chances").is_none(), "{name}");
        }
        assert_eq!(main["spell"], off["spell"], "{name}");
        let masks = |oil: &Value| -> Vec<String> {
            oil["trigger_spells"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|position| {
                    value["player"]["spells"][position.as_u64().unwrap() as usize]["proc_mask"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|mask| mask.as_str().unwrap().to_string())
                })
                .collect()
        };
        assert!(
            masks(main)
                .iter()
                .all(|mask| mask.starts_with("ProcMaskMeleeMH")),
            "{name}"
        );
        assert!(
            masks(off)
                .iter()
                .all(|mask| mask.starts_with("ProcMaskMeleeOH")),
            "{name}"
        );
        assert!(!masks(main).is_empty() && !masks(off).is_empty(), "{name}");
        // The bolt follows a hit at the same instant.
        let logs = first_fight_log(value);
        let bolts = logs
            .lines()
            .filter(|line| line.contains("Casting {SpellID: 350025}"))
            .count();
        assert!(bolts > 0, "{name}");
    }
}

/// Each oiled hand's listener needs its effect.
#[test]
fn a_frost_oil_listener_needs_its_effect() {
    for aura in ["Frost Oil (main hand)", "Frost Oil (off hand)"] {
        let mut value = fixture("combat-rogue-frost-oil");
        let index = effect_index(&value, aura);
        value["effects"].as_array_mut().unwrap().remove(index);
        assert!(
            refusals(value).contains(&format!(
                "aura_listener_unclaimed: player aura \"{aura}\" listens to combat events \
                 without an effect"
            )),
            "{aura}"
        );
    }
}
