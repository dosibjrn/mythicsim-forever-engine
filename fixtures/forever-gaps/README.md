# Forever gap requests

Production Quick Sim requests that the Rust engine refused between 5 and 27 October 2026, which then
ran in the Go engine or needed its exporter to prepare them. The character names are replaced and the iterations cut to 50. Each one
stands for a group of the refused requests and is read by
[tests/forever_gaps.rs](../../tests/forever_gaps.rs), which prepares it under Forever's rules,
checks the gate passes and runs it.

| Request | What Rust lacked |
|---|---|
| `protection-paladin-seal-of-light` | Seal of Light and Judgement of Light on a tank, the party's Retribution Aura striking back |
| `protection-paladin-retribution-aura` | the paladin's own Retribution Aura cast (rank 2, moved to 7 sec before the pull so it is not on the GCD) |
| `protection-warrior-thunder-clap` | Thunder Clap beside a permanent slow in its category |
| `enhancement-shaman-windfury-swap` | Windfury Weapon on a swap weapon against the party's Windfury Totem |
| `retribution-paladin-swap-blade-of-hanna` | a swap onto Blade of Hanna, a weapon with its own proc |
| `retribution-paladin-swap-sulfuras` | a swap onto Sulfuras, Hand of Ragnaros |
| `shadow-priest-vampiric-embrace` | Vampiric Embrace |
| `shadow-priest-prepull-sequence` | a strict sequence among the prepull actions, its first step on a time condition |
| `marksmanship-hunter-sequence-conditions` | sequence steps with conditions |
| `enhancement-shaman-many-stat-auras` | fourteen stat auras |
| `retribution-paladin-spell-value-fold` | a spell value condition the Rust preparation could not fold |
| `demonology-warlock-pet-values`, `demonology-warlock-pet-aura-time` | a demon's mana percent and aura time |
| `combat-rogue-aura-value-without-aura` | an aura time without an aura |
| `enhancement-shaman-stormcallers-garb` | Stormcaller's Garb (3) |
| `hunter-bear-swipe-three-targets` | the Bear's Swipe on three targets |
| `enhancement-shaman-frostbrand-swap` | Frostbrand Weapon on the swap weapon |
| `feral-druid-swap-heart-of-wyrmthalak` | a swap with Heart of Wyrmthalak's proc manager |
| `enhancement-shaman-swap-fiery-weapon` | a swap weapon with Fiery Weapon |
| `protection-paladin-judgement-of-light-three-targets` | the raid's Judgement of Light on three targets |
| `demonology-warlock-unsummoned-pet-mana` | the mana percent of a demon the fight never summons |
| `combat-rogue-darkmantle-armor` | Darkmantle Armor (4)'s energy |
| `combat-rogue-darkmantle-riposte` | Riposte without a tank |
| `dps-warrior-pummel` | Pummel |
| `dps-warrior-battlegear-of-heroism` | Battlegear of Heroism (4) |
| `elemental-shaman-the-elements` | The Furious Storm for a spec other than Restoration |
| `demonology-warlock-mob-type-bonus` | a bonus against demons on a target of another type |
| `combat-rogue-battle-squawk` | a fight that outlasts Battle Squawk |
| `demonology-warlock-assigned-curse` | the assigned curse at all targets |
| `retribution-paladin-swap-thunderfury` | a swap onto Thunderfury |
| `retribution-paladin-swap-crit-taken` | a swap that changes the crit the player takes |
| `retribution-paladin-swap-runeblade` | a swap onto the Runeblade of Baron Rivendare |
| `hunter-gorilla-thunderstomp` | the Gorilla's Thunderstomp on five targets |
| `hunter-prepull-group-move` | a prepull group reference that moves to a starting range |
| `protection-warrior-fire-hardened-hauberk` | a rage gain on use |
| `demonology-warlock-orb-of-soranruk` | a heal over time on use |
| `demonology-warlock-sequence-other-targets` | sequence steps at the next target |
| `balance-druid-change-target` | a change of target to the next target in the priority list |
| `demonology-warlock-sequence-change-target` | changes of target inside a strict sequence |
| `shadow-priest-vampiric-embrace-three-targets` | Vampiric Embrace on several targets (the Vampiric Embrace request with three targets and its casts, Shadow Word: Pain and Mind Blast at the next target) |
| `demonology-warlock-succubus-judgement-of-light` | the raid's Judgement of Light beside a Succubus, which strikes in melee (a production request with the debuff added) |
| `enhancement-shaman-swap-speed-enchant` | a swap onto a weapon with Iron Counterweight, an enchant speed aura (a Best Gear comparison, 27 October 2026) |
| `feral-bear-druid-swap-defense` | a tank's swap onto Headspike, whose Defense changes the target's swings (28 October 2026) |
| `dps-warrior-mark-of-the-champion-undead` | Mark of the Champion against an Undead target (28 October 2026) |
| `assassination-rogue-change-target-rupture` | a Rogue that moves to the next target to keep Rupture on four (28 October 2026) |
| `combat-rogue-change-target` | the same for a Combat Rogue |
| `shadow-priest-chastise` | Chastise in a Shadow Priest's rotation |
| `hunter-skysight` | Skysight, the Windshaper Skyborne racial, in a Hunter's rotation (28 October 2026) |
| `protection-paladin-devotion-aura` | a tank's own Devotion Aura cast (the Protection Paladin reference build with Devotion Aura rank 7 first in its rotation while the aura is down) |
| `protection-paladin-hammer-of-the-righteous` | Hammer of the Righteous in a Protection Paladin's rotation (the reference build with the cleave first in its rotation) |
| `retribution-paladin-seal-of-wisdom` | Seal of Wisdom rank 1 in a Retribution Paladin's rotation beside the raid's Judgement of Wisdom, whose mana shares one metric |
| `retribution-paladin-seal-of-justice` | Seal of Justice and its judgement in a Retribution Paladin's rotation |
| `protection-warrior-shield-bash` | Shield Bash in a Protection Warrior's rotation |
| `arms-warrior-mocking-blow` | Mocking Blow in an Arms Warrior's rotation, in Battle Stance |
| `protection-warrior-disarm` | Disarm in a Protection Warrior's rotation, in Defensive Stance |
| `warrior-intercept` | Intercept from 25 yards by a Night Elf warrior who changes to Berserker Stance first, so the run takes the racial's 2% speed |
| `arms-warrior-revenge` | Revenge in the rotation of an Arms Warrior off the target's threat, which nothing opens |
| `hunter-freezing-trap` | Freezing Trap in a Hunter's rotation |
| `feral-bear-druid-thorns` | a tank druid's own Thorns, first in the Bear reference build's rotation (its Crusader enchant and Swap1 set taken out), striking back a batch window after each swing it takes |
| `balance-druid-thorns` | Thorns first in a Balance Druid's rotation, off the target's threat |
| `shockadin-paladin-mcp-swap-one-hand-specialization` | One-Handed Weapon Specialization on a Shockadin whose Manual Crowd Pummeler opener swaps back to a two-hander (31 October 2026) |
| `retribution-paladin-swap-ironfoe` | a Manual Crowd Pummeler opener that swaps back to Ironfoe, whose Fury of Forgewright follows the weapon (1 November 2026) |
| `enhancement-shaman-beaststalker-armor` | Beaststalker Armor (5) on an Enhancement Shaman, whose mana proc only a Hunter's domain exported (2 November 2026) |
| `enhancement-shaman-swap-swarmguard` | a Manual Crowd Pummeler opener beside Badge of the Swarmguard, whose rate the swap rebuilds for the weapon in the hand (1 November 2026) |
| `retribution-paladin-swap-dark-iron-sunderer` | a Manual Crowd Pummeler opener that swaps onto Dark Iron Sunderer, whose Cleave Armor rolls only once it is in the hand (1 November 2026; its talents are in push 113554's tree already) |
| `combat-rogue-self-casts-at-player` | a strict sequence casting Vanish, Adrenaline Rush and Blade Flurry at the player (1 November 2026) |
| `protection-paladin-quel-serrar` | Quel'Serrar on a tank, whose Sanctuary's Defense and armor change the target's swings (1 November 2026) |
