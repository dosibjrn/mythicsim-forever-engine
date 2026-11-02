//! The Druid talents proto (proto.DruidTalents) read as plain numbers.

use std::sync::OnceLock;

use crate::contracts::request::Message;
use crate::prepare::sim::Rules;
use crate::prepare::spelldata::Ladder;
use crate::prepare::talent_layout::Extras;

/// Genesis (1223081): 1% periodic damage and healing a rank in the frozen data, 2% under
/// Forever's rules since hotfix push 113531 (both effects' curves double).
pub(super) fn genesis(rules: Rules) -> &'static Ladder {
    static REFERENCE: OnceLock<Ladder> = OnceLock::new();
    static HOTFIX: OnceLock<Ladder> = OnceLock::new();
    match rules {
        Rules::GoReference => REFERENCE.get_or_init(|| Ladder::talent(1223081, 5)),
        Rules::Forever => HOTFIX.get_or_init(|| {
            let doubled = vec![2.0, 4.0, 6.0, 8.0, 10.0];
            Ladder::talent_with_curve(1223081, &[doubled.clone(), doubled])
        }),
    }
}

/// Naturalist (17069): its damage done bonus (the second curve) is 1% a rank in the frozen data
/// and 2% under Forever's rules since hotfix push 113531; the cast time curve stays.
pub(super) fn naturalist(rules: Rules) -> &'static Ladder {
    static REFERENCE: OnceLock<Ladder> = OnceLock::new();
    static HOTFIX: OnceLock<Ladder> = OnceLock::new();
    match rules {
        Rules::GoReference => REFERENCE.get_or_init(|| Ladder::talent(17069, 5)),
        Rules::Forever => HOTFIX.get_or_init(|| {
            Ladder::talent_with_curve(
                17069,
                &[
                    vec![-100.0, -200.0, -300.0, -400.0, -500.0],
                    vec![2.0, 4.0, 6.0, 8.0, 10.0],
                ],
            )
        }),
    }
}

/// The talents the Druid package reads, by the proto's field names. The ones Go leaves
/// unmodelled (Improved Entangling Roots, Overgrowth, Brutal Impact, Feral Charge, Nature's
/// Focus and the healing talents) have no field.
#[derive(Clone, Debug, Default)]
pub(crate) struct Talents {
    pub improved_wrath: i32,
    pub genesis: i32,
    pub moonglow: i32,
    pub improved_moonfire: i32,
    pub natures_majesty: i32,
    pub natures_reach: i32,
    pub natures_splendor: bool,
    pub insect_swarm: bool,
    pub vengeance: i32,
    pub improved_starfire: i32,
    pub natures_grace: bool,
    pub eclipse: i32,
    pub moonfury: i32,
    pub moonkin_form: bool,
    pub ferocity: i32,
    pub heart_of_the_wild: i32,
    pub feral_swiftness: i32,
    pub feral_instinct: i32,
    pub thick_hide: i32,
    pub shredding_attacks: i32,
    pub savage_fury: i32,
    pub sharpened_claws: i32,
    pub shifting_power: bool,
    pub primal_bite: bool,
    pub predatory_strikes: i32,
    pub blood_frenzy: i32,
    pub improved_shifting_power: i32,
    pub leader_of_the_pack: bool,
    pub natural_instinct: i32,
    pub natural_reaction: i32,
    pub rend_and_tear: i32,
    pub berserk: bool,
    pub furor: i32,
    pub naturalist: i32,
    pub subtlety: i32,
    pub natural_shapeshifter: i32,
    pub reflection: i32,
    pub living_spirit: i32,
    /// Hotfix push 113531's new talents, which the pinned proto has no field for: Forever's
    /// rules read them into the layout's extras, and the reference never sets them. Bloom
    /// (1328621) is read the same way: a Restoration druid's heals over time heal their player
    /// for the druid's Spirit when they end (`spells::bloom`).
    pub lacerate: bool,
    pub predatory_discipline: bool,
    pub bloom: bool,
}

impl Talents {
    pub(crate) fn from_message(m: &Message, extras: &Extras) -> Talents {
        let extra = |field: &str| extras.get(field).is_some_and(|&points| points == 1);
        Talents {
            improved_wrath: m.i32("improved_wrath"),
            genesis: m.i32("genesis"),
            moonglow: m.i32("moonglow"),
            improved_moonfire: m.i32("improved_moonfire"),
            natures_majesty: m.i32("natures_majesty"),
            natures_reach: m.i32("natures_reach"),
            natures_splendor: m.bool("natures_splendor"),
            insect_swarm: m.bool("insect_swarm"),
            vengeance: m.i32("vengeance"),
            improved_starfire: m.i32("improved_starfire"),
            natures_grace: m.bool("natures_grace"),
            eclipse: m.i32("eclipse"),
            moonfury: m.i32("moonfury"),
            moonkin_form: m.bool("moonkin_form"),
            ferocity: m.i32("ferocity"),
            heart_of_the_wild: m.i32("heart_of_the_wild"),
            feral_swiftness: m.i32("feral_swiftness"),
            feral_instinct: m.i32("feral_instinct"),
            thick_hide: m.i32("thick_hide"),
            shredding_attacks: m.i32("shredding_attacks"),
            savage_fury: m.i32("savage_fury"),
            sharpened_claws: m.i32("sharpened_claws"),
            shifting_power: m.bool("shifting_power"),
            primal_bite: m.bool("primal_bite"),
            predatory_strikes: m.i32("predatory_strikes"),
            blood_frenzy: m.i32("blood_frenzy"),
            improved_shifting_power: m.i32("improved_shifting_power"),
            leader_of_the_pack: m.bool("leader_of_the_pack"),
            natural_instinct: m.i32("natural_instinct"),
            natural_reaction: m.i32("natural_reaction"),
            rend_and_tear: m.i32("rend_and_tear"),
            berserk: m.bool("berserk"),
            furor: m.i32("furor"),
            naturalist: m.i32("naturalist"),
            subtlety: m.i32("subtlety"),
            natural_shapeshifter: m.i32("natural_shapeshifter"),
            reflection: m.i32("reflection"),
            living_spirit: m.i32("living_spirit"),
            lacerate: extra("lacerate"),
            predatory_discipline: extra("predatory_discipline"),
            bloom: extra("bloom"),
        }
    }
}
