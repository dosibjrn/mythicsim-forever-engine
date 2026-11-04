# MythicSim Forever Engine

A Rust simulation engine for WoW Forever. Every Forever sim on
[MythicSim](https://mythicsim.com/wow-forever/sim) runs on it: Quick Sims, stat weights, Best
Gear, the trinket and race boards, the Build Lab and the healer sims. [Coverage](#coverage) says
what that covers and what it refuses.

The engine began as an exact port of [MythicSim's Go fork](https://github.com/sage3648/mythicsim-forever-engine-go)
of [wowsims Forever](https://github.com/ElliotWood/Forever): event order, random numbers and
float math match, so a sim the Go engine covers gives the same result and the same combat log
in either engine. Rust is 1.3x to 2x faster. Since October 2026 new Forever work happens here
only: the Go fork no longer takes patches and its frozen goldens guard what it covered, and
mechanics Go never had, starting with the healer sims, are built and checked in Rust alone
(see [Rust-only mechanics](#rust-only-mechanics)).

## Quick start

Install stable Rust, then run:

```sh
git clone https://github.com/sage3648/mythicsim-forever-engine.git
cd mythicsim-forever-engine
cargo test --locked
cargo run --locked --release -- check --infile fixtures/mage/prepared-v2/frost-reference.prepared.json
cargo run --locked --release -- sim --infile fixtures/mage/prepared-v2/frost-reference.prepared.json
```

`check` lists anything in an input that Rust does not support. `sim` runs the fight
and prints the same JSON result that Go prints. You do not need Go to build or test
the engine.

For a bounded comparison, `sim --iterations N` runs only the first N iterations of a prepared
request without rewriting its source fields or `request_sha256`. N must be between 1 and the
request's original `simOptions.iterations`.

## How it works

1. Rust prepares the character from the application's request: gear, stats, auras,
   spells and rotation ([prepared v2](docs/prepared-v2.md),
   [Rust preparation](docs/rust-preparation.md)).
2. A coverage gate checks the prepared input and refuses anything Rust cannot simulate
   exactly, with a reason for each refusal.
3. Rust runs the fight.

[`tools/route.py`](docs/routing.md) is what the MythicSim worker calls. It runs a request in
Rust, or says why it cannot. With the worker's `FOREVER_GO_FALLBACK` off, as on MythicSim, a
request Rust refuses fails with that reason; nothing runs in Go.

## Coverage

Every class and spec MythicSim offers runs in Rust: the Druid, Hunter, Mage, Paladin, Priest,
Rogue, Shaman, Warlock and Warrior damage and tank specs, with their talents, gear, buffs,
consumables, pets, tanking, item swaps and 2 to 5 targets, and the Holy Paladin, Priest,
Restoration Shaman and Restoration Druid healer sims. Rust prepares the character as well as
running the fight, so no part of a request goes through Go. Holy Paladin heal sims match Go
too, under the reference's rules: heals cast on the player and counted in full, as Go counts
them. Production prepares them with Forever's rules, which leave out overhealing.

The 812 production requests that fell back to Go, or needed the Go exporter to prepare them,
between 24 and 27 October 2026 all prepare and run in Rust: tank damage shields, Seal of Light,
Vampiric Embrace, item swaps onto weapons with procs, custom rotation sequences and the rest.
The coverage gate still refuses what Rust cannot simulate exactly, each refusal with a stable
reason code (see [routing](docs/routing.md)), and MythicSim fails such a request with that
reason rather than run it in Go.

A Quick Sim's timeline and combat log show the run's first fight. Rust keeps no per-iteration
DPS, so the fight nearest the median is not picked.

## The Go reference

The reference pin stays at Go revision `a667dce8492bd9fb94269a9f864289cb74006775`, the fork's
patches 190 to 192 on the last Go release. Every accepted fixture keeps its Go golden, so a change to a mechanic Go covers
still has to match it or explain why it no longer should. A match means the same behavior as
that Go engine, not proof of the live game. [UPSTREAM.md](UPSTREAM.md) records how the pin
moved while Go led, and [docs/validation-summary.md](docs/validation-summary.md) lists the
comparison records.

## Rust-only mechanics

Mechanics the Go engine never had have no Go golden. They are checked against the game's own
numbers instead: the client spell rows, Wowhead Forever tooltips and logs, with focused tests
that pin the expected heal, cost or timing from those numbers. A request that reaches one runs
in Rust or is refused; it never falls back to Go, which would skip the spells it does not know.
The healer sims are the first: every Holy Paladin, Priest, Restoration Shaman and Restoration
Druid heal lands on a player who needs it, less the share real healers overheal (below), so
healing per second is what the healer's mana heals in a dungeon.

Where Rust departs from a mechanic the Go reference has, preparation follows Forever by default
and the reference with `Rules::GoReference` (`prepare --go-rules`), which every comparison with
the Go goldens and exporter uses. The departures:

- Pet hit, from Forever's developers on their tracker (22 October 2026): a Hunter's pet takes
  17/8 of its master's ranged hit as spell hit (Lightning Breath and the other magic pet hits),
  and a Warlock's demon takes 8/17 of its master's spell hit as melee hit, where the reference
  gives a Hunter pet no spell hit and a demon the whole spell hit.
- Improved Mind Flay, from its Forever tooltip ("Each instance of damage of Mind Flay deals 20%
  more damage than its previous instance of damage"): each tick deals the step more than the tick
  before it, compounding (1, 1.2 and 1.44 times the tick at two points), where the reference adds
  the step per tick (1, 1.2 and 1.4). The talent is a dummy aura run by a server-side script, so
  the client data has no stacking rule of its own.
- Level partial resists, from Forever's beta logs on foreverlogs.gg (23 October 2026): a target
  above the caster's level partially resists nothing, dot ticks and direct hits alike, so only
  resistance past the caster's Spell Piercing does. About 1,000 dot ticks and 100 direct
  non-binary hits on Razorfen Kraul and Scarlet Monastery bosses, which miss level 30 casters'
  Shadow Bolts a third of the time, all landed whole. The reference's level 63 boss partially
  resists 18% of them, 6% of the damage on average. The prepared target carries
  `no_level_partial_resist`.
- The raid's Expose Armor, from MythicSim's Discord (28 October 2026, Bae and Vrelk): a player
  who picks the raid's Expose Armor instead of its Sunder Armor should not gain from the pick, but
  the reference applies Expose Armor at full from the pull while Sunder Armor ramps a stack every
  default GCD, so a Hunter simmed 1,241 DPS with Expose Armor against 1,226 with Sunder Armor.
  Under Forever's rules, Expose Armor alone lands at full 3 s into the fight, which leaves the same
  6,750 armor-seconds of its 2,250 armor unapplied as Sunder Armor's ramp: the Marksmanship
  production request sims 609.87 against 609.86. With both, Expose Armor stays up from the pull.
  Forever preparation writes `external_expose_armor`.
- Flurry's charges, from three level 30 Warrior logs on build 1.60.1.70450 (magey/forever-warrior
  discussion 30, counted in wowsims/forever PR 745): with Flurry up, every white swing spends a
  charge whatever its outcome (misses 180 of 180, dodges 75 of 75, parries 72 of 72), and a
  Heroic Strike spends one only when it misses (23 of 23; landed ones 3 of 265, each sharing its
  moment with a white swing, dodged or parried ones 0 of 38). Cleave follows Heroic Strike, an
  assumption, as the logs hold one Cleave. The reference spends a charge on each landed white
  hit, Heroic Strike and Cleave included, and never on a miss. The prepared `warrior_flurry`
  effect carries `every_swing_spends`.
- Casting regeneration auras, from the client's mana regeneration interrupt aura (type 134),
  which acts as it is applied: Resourcefulness, Rapid Recuperation and Improved Stormstrike let
  more spirit regeneration continue while casting from the moment they are up. The reference
  raises the rate without refreshing its cached mana tick, so the buff acts only once a change of
  MP5, Spirit or Intellect refreshes it, which on a Survival Hunter is almost never. Each one's
  mana lands in a resource row of its own, named after the aura, as Innervate's does. The
  prepared player carries `casting_regen_at_once`.
- Overhealing, from Forever's level 30 beta logs on foreverlogs.gg (1 to 24 October 2026, 74
  public reports): a healer sim counts each heal less the share real Holy and Discipline
  Priests, Holy Paladins and Restoration Druids and Shamans overhealed of it, such as Flash Heal
  1%, Holy Light 10%, Renew 16%, Prayer of Healing 23% and Healing Stream Totem 63%. Heals the
  logs do not hold yet (Greater Heal, Chain Heal, Prayer of Mending and the other heals above
  level 30) take the pooled share of their kind: 5.4% on one player at once, 16.7% over time,
  24.6% on a group. Shields keep their whole absorb. The shares are dungeon play at level 30 and
  will move with level 60 raids. The reference, like Go, counts every heal in full. The prepared
  player carries `overheal`, and each heal's overhealing is reported beside its healing
  (`src/prepare/overheal.rs`, `validation/2026-10-24-healer-overheal.json`).
- Heal over time crits, from Forever's level 30 beta logs on foreverlogs.gg (1 to 24 October
  2026) and the client rows: Renew, Rejuvenation, Regrowth and Healing Stream Totem ticks crit
  for 1.5 times the tick, between normal ticks on the same player (a Holy Priest's Renew ticks
  97 and crits 145 and 146, a Restoration Druid's Rejuvenation ticks 95 and crits 141 and 142).
  Pooled over 104 reports, 7.0% of 8,245 Renew ticks and 5.7% of 7,171 Rejuvenation ticks crit,
  near the casters' direct heal crit (9.4% and 5.9%); a tick that wholly overheals loses its crit
  flag, so heavy overhealers read low. Every heal over time row the healer sims use states
  Periodic Can Crit, the client bit damage ticks already follow. A tick rolls the healing crit on
  spell crit and the spell's crit bonus, as a direct heal does (Improved Regrowth's reaches
  Regrowth's ticks, as the talent names the whole spell), less what a damage tick does not take
  (Inner Focus). The reference, like Go, never crits a heal tick. Each prepared heal over time
  whose row says so carries `tick_can_crit`
  (`validation/2026-10-25-heal-tick-crits.json`).
- Conjured cooldown, from the client's item effects (1.60.1.70532, ItemEffect and
  ItemXItemEffect): every mana gem (Agate, Jade, Citrine, Ruby) is in spell category 1153 with a
  2 minute category cooldown, with Demonic and Dark Rune, the healthstones and Thistle Tea, so a
  mage who uses a gem cannot use a rune for 2 minutes, and the reverse. The reference gives the
  gems a category 30 timer of their own, which no Forever item has, so a gem and a rune went back
  to back. Forever preparation puts the gems on the category 1153 timer.
- A queued spell holds a rotation's move, from MythicSim's Discord (25 October 2026, Bae and
  MuRRRkn on the Survival weave): a player who has pressed a shot or strike in the spell queue
  window does not walk off before it fires. The reference starts the move at once, so a shot
  queued at the end of the global cooldown fired from inside its 10.5 yard minimum range and a
  Strider Kick queued as the weave stepped out fired from outside melee range, and both failed.
  On the Survival light weave that cost a Strider Kick or a shot every few cycles, and 3% haste
  lost 19 DPS because it moved the Auto Shot onto the global cooldown more often. Forever
  preparation writes `move_waits_for_queued_cast`; a spell out of range where the player stands
  does not hold the move.
- A melee swing leaves the Auto Shot timer alone, from Forever's bug tracker: the beta restarts
  the timer on every melee swing ("Ranged swing timer is reset by melee",
  https://github.com/ClassicWoWCommunity/forever-bugs/issues/14), and a Forever developer
  answered on 6 October 2026 "For Hunters, this is a Bug with Auto Shot"
  (https://github.com/ClassicWoWCommunity/forever-bugs/issues/14#issuecomment-6025122239), so the
  rule models the game as intended. The reference restarts the timer as if a shot had just fired
  on every white main or off hand swing and every Raptor Strike, so a weaving Hunter's next shot
  came a full shot after the last melee hit instead of when it was due: on the Survival weave
  reference request 32.8 Auto Shots a fight, now 39.3. Forever preparation writes
  `melee_keeps_auto_shot_timer`.
- A move arrives when its time is up (`move_arrives_at_destination`, Forever preparation): a
  rotation woken at the instant a move ends, before its end action runs, reads the destination.
  The reference reads the walk rounded to the millisecond, 5.001 yards for a step in to 5, takes
  the Hunter out of melee range with no move left to bring them back, and the rest of the
  iteration has no swings or strikes: the Go golden of the Survival weave
  (`survival-hunter-aspect-of-the-beast-over-hawk`) has iterations at 400 to 510 DPS from it, and
  a held move makes it far more common.
- A rotation with a move keeps looking through the global cooldown: in game moving needs no
  global cooldown, only casting waits on it, so a player watching a swing timer starts a walk
  when its condition turns true. The reference's idle rotation sleeps until the global cooldown
  ends unless a swing or a shot wakes it, so a walk whose condition turned true under the global
  cooldown started at its end. A rotation that looks under the global cooldown queues the spell
  for its end, so it also goes on from a queued instant as it goes off (one that cast; a queued
  spell that fails does not run it again): the reference runs no rotation then, and a step
  out waiting on that spell started a global cooldown late. Forever preparation writes
  `rotation_with_moves_looks_through_gcd`; it acts only on a rotation with a Move in its priority
  list, a group or a schedule, so every other rotation waits as before.
- The off hand's half swing wait counts from the start of the melee attack, from swing probe logs
  on the Forever beta (16 to 17 October 2026, dual wield, 2.72 main hand and 2.33 off hand): an
  Auto Shot turns the melee attack off, and a weave's `/startattack` turns it back on as the step
  in begins. The off hand's first swing came exactly half its swing after the attack went on in
  11 of 19 attack starts, and never half a swing after the Auto Shot turned it off (0 of 17,
  always 0.06 to 0.15 seconds later). On six walks in longer than half the off hand swing, it
  swung within 0.2 seconds of arriving, so the wait runs during the walk, and with the attack
  left on for 21 seconds over five walks out and back in without a shot, both hands swung on
  arrival every time. Forever preparation writes `off_hand_waits_from_attack_start`:
  a walk from outside melee range to a destination inside it starts the attack as it begins, the
  off hand swings no sooner than half its swing after that, and an attack still on adds no wait
  when the Hunter steps back in. The reference adds the half swing on every arrival, so on a dual
  wield weave that steps in after each Auto Shot and out after the main hand swing the off hand
  almost never swung: 0.0 swings a fight on our weave request, now 28.3.
- A critical white hit's rage, from Forever's combat logs of 9 October 2026 (counted in
  wowsims/forever 7c28cfa73): two Warriors' two-hand crits gave exactly twice the swing's rage, 21
  of 21 (16.2 to 32.4 over 13 crits, 14.8 to 29.7 over 8). The reference adds the rage of the
  swing before a two-hander's factor, so a two-hand crit gave 1.77 times its swing: 27.86 rage
  on the Arms production request's 15.75 swing, now 31.5. A one-hander's crit, either hand,
  already gave twice its swing and does not move. A bear's crit follows the same rule (Forever's
  development notes of 1 and 8 October raise the crit rage of warriors and bears alike), and
  moves nothing either: a bear's paw takes no two-hander factor here (8.65 rage a 2.5 second
  swing), so its crit already doubled. The prepared `rage_bar` effect carries
  `crit_multiplies_swing`. Arms gains 0.6% to 1.3% (the production request 703.68 to 708.83 DPS
  at 10,000 iterations); Fury and bears sim the same to the cent.
- Multi-Shot's cast, from our Forever beta combat logs on Warcraft Logs (reports
  Par9kghHbAVYptwm, TBqwNKxVW8H3narp and mJGyPgZAtr2T4YpX, quiver at 15% speed): all 121
  Multi-Shots have a cast start, a median 0.45, 0.45 and 0.44 seconds before the shot, and 0.32
  under Rapid Fire in the third. Its client row (2643) has no cast time, so the reference casts
  it at once.
  Forever preparation gives it a 0.5 second cast, which the ranged haste multiplier divides as it
  does Aimed Shot's (0.5 / 1.15 is 0.435, and 0.5 / (1.15 x 1.4) is 0.31 under Rapid Fire); its
  global cooldown and cooldown are unchanged, and like any cast it cannot start while moving.
- Autocast looks past a cooldown the GCD holds back (`autocast_skips_unready_cooldowns`,
  Forever preparation), as wowsims/forever does since 21 and 22 October 2026 (10b0e65f2,
  fbf5d98e8). Autocast Other Cooldowns took the first ready major cooldown and then applied the
  GCD rule to it alone (an explosive only while the GCD runs, any other only once it is ready),
  so a cooldown waiting for the GCD held back an explosive sorted after it, and through a run of
  hardcasts an explosive held back every cooldown behind it. Now each ready cooldown that fails
  the rule is passed over, and an explosive whose cast fits in the shortest GCD also waits while
  less of the GCD is left than its cast, so it never delays the next spell. The reference stops at
  the first ready cooldown.
- Idol of the Dream (220606) adds 2 seconds to Rip (Improved Rip, 446212, client 1.60.1.70532:
  a flat duration modifier on Rip's class mask), so Rip ticks once more at the same period; ported
  from wowsims/forever 8d879a401. The reference registers no effect for it.
- Jewel of the Guard Captain (270225) and Cleansed Vilebranch Icon (270276) give 8 spell damage
  and healing, tripled in Forest and Grassland areas (Wowhead Forever); the imported data had 32.
- Water Shield gives mana (`water_shield`, Forever preparation), from the client (1.60.1.70532)
  and MythicSim's Discord (26 October 2026, Maro): a critical spell of the shaman's, damage or
  heal, spends one of three globes for 2% of maximum mana, one globe every 3.5 seconds at most,
  and the cast has no cooldown since 70532 (it had 15 seconds). The reference registers the talent
  but no build could cast it.
- The racial pass of Forever's development notes of 26 October 2026 (Forever preparation; the
  reference keeps the 16 October racials). Human Ruthless gives 1% crit (was 2%) and Human Spirit
  10% Spirit (was 5%). Dwarf Earth Fury is gone and Artisan's Precision gives 1% hit with
  attacks and spells. Tauren Endurance is 5% health only. Orc Savage is gone and Command gives
  Hunter and Warlock pets 5% more damage. Skyborne Wind Blessed is 2% haste (was 3%). Read
  Ley Line's Energized is 50% more regeneration for 15 seconds (70532 had 100% for 30), cannot
  be cast while an Innervate is up, and an Innervate that lands removes it
  (`read_ley_line.innervate_replaces`). Skysight's Elemental Blessing is 15% movement speed for
  15 seconds (was 10% for 30). Attuned and Charged, which a Ley Line or an elemental convergence
  grants before the pull and which double those two, are not assumed. Client 1.60.1.70532 still
  has the old rows; the notes are the source until a client build carries the new ones. The
  notes' additions of 27 October (post 8): Orc Blood Fury's cooldown is 2 minutes (was 3),
  Troll Berserking lasts 15 seconds (was 10) and Touch of the Grave's internal cooldown is 15
  seconds (was 25); Attuned and Charged end on entering a raid, so they stay unassumed.
- The non-Mage rows of client 1.60.1.70630 that change a number (Forever preparation; the data is
  1.60.1.70425 and the reference keeps its values). Hunter's Mark rank 4 (14325, SpellEffect
  690991) gives every attacker 143 ranged Attack Power (was 71). The Spellstone's damage effects
  (1233765, 1233768, 1233770, "Increase Shadow Dam") name the Shadow school alone (mask 32; was
  Fire and Shadow, 36). Blood Craze (16488) carries the No Threat attribute (Attributes_1 0x400),
  so its heal over time puts out no threat. The Mage Revelation buff (1248808, SpellEffect
  1251976) also names bit 32768 of the second class mask word, which holds Arcane Barrage and
  Arcane Wrath. Rows that need nothing: Bloodthrill's marker aura (1289681) now lasts 5 seconds,
  the window the engine already opens (Overpower's 5 seconds, 1282733); Elemental Focus keeps its
  10% (ProcChance 10 moved to the effect as 100 and base 10); Amplify Curse hands Bane of Agony's
  50% to an aura on the target (a class mask of Agony's own, tooltip `$o1*($w2/100+1)`), which
  the whole tick factor of patch 177 already is. Holy Fire rank 1 (54), Twilight Focus's class
  mask, Fire Nova's line of sight bit and Dark Inspiration's aura type (80 to 137, still 15%
  Spirit) already agree with the data, and the engine starts Elemental Mastery's cooldown at the
  cast, as 70630 has it (the cooldown-on-event bit is gone). Lava Burst rank 3 and Windfury
  Weapon rank 2 are not the ranks cast, Multi-Shot 1289053 is a mob's bolt barrage, Feral
  Swiftness is movement speed, Gore Drinker is not modelled, and the Touch of the Grave
  attribute clears and the Curse, Rogue and Power Word: Shield attribute bits are read by
  nothing the engine computes.
- Winter's Chill is "your next Cone of Cold" (`winters_chill`, Forever preparation), from client
  1.60.1.70630 (talent 11180 and aura 12579): only Frostbolt, Frost Blast and a Fingers of Frost
  empowered Ice Lance can apply it, each stack makes the Cone of Cold deal 5% more damage and
  gain 7% of the mage's spell damage (12579 and 11180 effect 3, a flat share added before the
  hit is worked out), and the Cone of Cold uses every stack up as its cast completes. The stacks
  stay on the mage rather than the target (the client puts 12579 on the target), so rotations
  that test the aura keep working, and an area Cone of Cold gives every target the bonus. The
  reference stacks from any landed Frost damage and never consumes. The rest of the Mage rows
  of that build (Ice Barrier and Frigid Power, Frost Warding's absorb, Frost Blast's 0.271
  coefficient, Arcane Wrath's 6 second cooldown, Missile Barrage ignoring missiles and hearing
  Frost Blast) were already in the imported data and the code through hotfix push 112753.
- The rows of client 1.60.1.70707 (30 October 2026, which also carries the 28 October hotfix) that
  change a number (Forever preparation; the data is 1.60.1.70425 and the reference keeps its
  values). Trueshot Aura ranks 1 to 4 fall to 15, 22, 30 and 40 ranged Attack Power (rank 4, 20905,
  SpellEffect 698474, was 75), below rank 5's (20906) 50, so the aura is rank 5. Improved Fireball
  (11069, SpellEffect 688786) also shortens Arcane Wrath and Arcane Barrage (bit 32768 of the second
  mask word). Efficiency (19416, SpellEffect 696992) also cheapens Flanking Strike and Strider Kick
  (bit 2097152 of the third word). Every "Increased Hit Chance" row (15464 to 15468, 18066) gains a
  spell hit effect beside its melee hit, which gear hit already is under Forever's rules; Black
  Dragon Mail's two piece bonus (15464) now gives both. The Mage tree (TraitTree 1112) moves: Arcane
  Focus (11222, TraitNode 105814) is removed, so the Arcane tree has 18 talents; Improved Channeling
  (105813) takes its square and Arcane Resilience (105802) Improved Channeling's, and Missile
  Barrage and Arcane Resilience swap TraitDefinitions 135332 and 135339, which puts Missile Barrage
  in the second row. Improved Fire Blast (105797) moves from the Fire tree's first row to the third,
  after Pyroblast. Forever preparation reads the talent string in that order (a 19 digit Arcane tree
  is refused) and the reference in Go's. Critical Mass (11115) falls to 3 ranks (TraitNodeEntry
  130513) and Incineration (18459) rises to 5 (130525); their curves (83083, 83095) keep 2% a rank.
  A string with a talent above its ranks in this build is refused, since an old string read in the
  new order can put five points in a two rank talent. Precision (29438, Elemental Precision until
  this build; SpellEffect 704780) turns from a miss chance modifier on Fire and Frost spells into
  spell hit chance (aura 55) for every school, Arcane included. Improved Fireball is renamed
  Accelerant. Frost Blast's three SkillLineAbility rows (1326053 to 1326055) leave the Frost skill
  line without reaching another skill line or the trait tables, while its spell rows and the talent
  tooltips naming it stay; the engine keeps it until the data says whether a level 60 mage still
  learns it. Rows that need nothing: Aspect of the Hawk ranks 1 to 5 fall to 15, 22, 30, 38 and 47
  but the engine casts rank 7 (25296, unchanged 120); Missile Barrage's channel time effect and
  Fengus' Ferocity's effects become dummy auras that keep their amounts (a server script), Lesser
  Intellect (Elixir of Wisdom, 6 to 3) and Well Fed (1249521) are not consumables the engine offers,
  Tauren Endurance's crit damage row names every school (the racial follows Blizzard's 16 October
  notes, health only), and the Bane of Agony marker, Surefooted's new row id and the creature,
  transmute and event rows are read by nothing the engine computes. Ice Lance's new 8 second
  cooldown is on rank 1 (1312002) alone, not the rank cast at level 60.
- Live server hotfix pushes 113531 and 113554, which reached client 1.60.1.70707 on 30 October
  2026 through the DBCache rather than a client build, for every class but Paladin and Priest
  (Forever preparation, `prepare::hotfix_113554`; the reference keeps the frozen rows). The rows
  are wowsims/forever #769 and #770's, regenerated from Raidbots' `forever` DBCache.bin, and the
  trees agree with Wowhead's Forever talent data. The Druid tree is read in the hotfix's order:
  Berserk (417141), Improved Shifting Power (1322670) and Gift of the Earthmother (414673) are
  gone, Furor and Heart of the Wild swap trees, Reflection and Subtlety swap, and the new
  Lacerate (1328617), Predatory Discipline (1328613) and Bloom (1328621) are talents the pinned
  proto has no field for (`prepare::talent_layout` reads them aside). Lacerate needs the talent;
  with Predatory Discipline a landed Ferocious Bite keeps the energy left after its cost; Primal
  Bite's level 60 rank is 1238070 (1238073 is gone, same 77 flat damage); Shifting Power's
  cooldown is 8 seconds; Wild Growth takes a 3 second cast that Naturalist shortens; Naturalist's
  damage and Genesis are 2% a rank. Bloom is modelled from its text alone (below).
  Moonkin Aura and Leader of the Pack are both 3% of every kind of crit, and one counts (the
  `RaidCrit` category, as #769 rules). Mage: Arcane Resilience has 3 ranks, Ice Lance's Frozen
  bonus is 300% and every rank has an 8 second cooldown on Fire Blast's category 19, Winter's
  Chill is 20% a stack, Arcane Blast's buff is 12% of Intellect, 100% more cost and 18 seconds,
  Ice Barrier's Frost damage is 5% and Frigid Power's 15%, and Arctic Reach lengthens Ice Lance.
  Hunter: Pathfinding and Improved Revive Pet swap squares in Beast Mastery, and Freezing Trap
  shares Explosive and Immolation Trap's category 411. Warlock: Improved Drains leaves Wrack out
  (its second effect raises Drain Mana, which the engine does not cast). Greater Blessing of
  Salvation and Greater Blessing of Light are gone from the game and apply nothing. Rows that
  need nothing: Boosted Critical Chance (1229519) and the Minor Speed and Speed boot enchants
  (911, 8737) are not read by the engine, Summon Arcane Elemental's middle spell is not modelled,
  Ferocious Bite's new rank 1 and the two low Lacerate ranks leave the level 60 ranks, and about
  1,400 rows only gain Attributes_14 bit 64.
- A damage bonus applied after the calculation is threat too (`threat_follows_damage_bonus`,
  Forever preparation), from MythicSim's Discord (26 October 2026, Vrelk: a Shadow Priest with
  no threat talents had less threat than damage). Improved Mind Flay's ramp, Ice Lance on a
  frozen target, Soul Siphon on Drain Life and Wrack, Amplify Curse on Curse of Agony and
  Incinerate on an Immolated target raise a hit's damage after its threat is worked out; the
  reference keeps the threat of the damage before the bonus, so Mind Flay made about 0.82
  threat per damage with no threat talents.
- The Paladin tree and Holy Binding of live server hotfix push 113554 (30 October 2026, applied
  to client 1.60.1.70707 by DBCache, which wago.tools' tables do not carry), from upstream
  wowsims/forever #772 and #773's regenerated rows and Wowhead's Forever talent data (Forever
  preparation; the reference keeps the 70425 tree). The Paladin string reads in the client's order,
  17 talents a tree (`src/classes/paladin/prepare/hotfix_113554.rs`), and a talent above its new
  ranks or a longer tree is refused, so every 70707 string is. Twist of Light, Instrument of Law,
  One-Handed Weapon Specialization, Improved Seal of Fury and Voice of Truth are gone. Unsealed
  Wrath (1310735) stores the active seal in its Echo on each landed melee auto (15 sec, 3 stacks)
  and a swing under a different seal spends every other Echo, each stack a roll of its seal.
  Swift Judgement is a second Judgement on its own 60 sec cooldown. Shieldsworn takes 4 sec off
  Holy Strike, whose landed hits fire the active Holy Shield without a charge and Shield
  Specialization's mana without its cooldown. Conviction is 2/4/6% crit with every spell and
  attack; Holy Power is 10-50% more crit healing; Two-Handed Weapon Specialization also cuts all
  threat 7/14/20% with a two-hander; Champion of the Light is 5 ranks of 12% of Intellect and 0.2
  sec off Hammer of Wrath; Righteous Fury adds 33% to all threat; Vindication gives the paladin no
  attack power; Sacred Duty's Stamina becomes 5/10 sec off Forbearance; Holy Shield's ranks deal
  and cost what the rank below did (Rank 3: 153 damage for 195 mana). Divine Precision 4/8%,
  Improved Seals 3-15%, Reverence 15/30%, Spiritual Focus 14-70%, Redoubt 6-30%, Shield
  Specialization 15/30% and 50/100%, Improved Righteous Fury 3/6/9%, Iron Creed 10-30% and
  2-6%, Boundless Light 15/30% Holy Shock heal crit. Oath of Light is a castable spell (below), and
  Boundless Light holds on a Holy Shock heal only while the Oath is not on the heal target;
  Light's Vigil, now friendly only, refuses a rotation that reaches it, since its party
  heal is not modelled yet; Shieldsworn's spell block, Stoicism and Turn Evil change nothing
  the sim models. Holy Binding (1328569) takes Binding Heal's field: a healer Priest's Flash Heal
  heals the priest for half as much, as a second jump, and Binding Heal's ranks leave the tree.
  Shadow Word: Death's backlash (10% to 30% of maximum health) is self damage the sim does not
  deal.
- Oath of Light (1328548) and Bloom (1328621), the two healer talents of hotfix pushes 113554 and
  113531, from the client text and the rows the pushes carry (wago.tools has neither, and
  upstream wowsims/forever models only the Oath). Oath of Light is an instant 8% base mana cast
  that swears a 5 minute Oath on the heal target, which is the player in the healer model and
  stands for the tank; while it holds, each Holy Light heal that reaches the tank heals 400 more,
  each Flash of Light 115 and each Holy Shock 229 (the rows' 6.67, 1.92 and 3.83 a level at
  level 60), added to the heal's base amount before healing power. The sim heals "a player who
  needs it", so a heal reaches the tank with the tank's share of the party's hits under
  `prepare::party_damage` (5 in 9, `party_damage::tank_share`, carried as the effect's
  `holder_share`); the rest reach players without the Oath, get no bonus and, for Holy Shock,
  keep Boundless Light's crit, which the aura takes off the tank's. A rotation casts it on the
  player (`castFriendlySpell` before the pull, `castSpell` in the list) and again before it ends.
  Bloom heals the target of a Rejuvenation or Regrowth that ends by its duration for 100% of the
  druid's Spirit. The client rows are two dummy effects (100 and 50) and no heal spell, so the
  coefficient, crit and modifiers are not stated: the heal is the Spirit exactly, no healing
  power, no multipliers, no crit, with the overhealing share of a heal at once, and shows as its
  own action (1328621). A heal over time cast again while it runs keeps ticking on its first
  player, so that cast blooms as the new one goes up; one Swiftmend consumes, or the fight cuts
  short, blooms nothing. The dispel case (50% of Spirit) never happens in the sim.
- Party damage for heals that follow it, an assumption until the logs hold them: Forever's level
  30 beta logs have no Prayer of Mending or Chain Heal yet. A healer's party of five takes hits
  at random, the tank about once a second and each of the other four about once every five
  seconds. Prayer of Mending goes on the tank and heals only when the player holding it is hit,
  then moves to another member with 30 seconds again (its buff row, 401877), so a cast recast
  every ten seconds heals about three of its five charges. Chain Heal lands on the tank and jumps
  only to members hit in the last four seconds, 2.72 players a cast on average. Holy Priest
  healing falls about 13%, Restoration Shaman's about 0.5%. Recalibrate the rates once the logs
  reach these heals. The reference heals every charge and jump at once. The prepared `heals`
  effect carries `party`, and each such heal `smart` (`src/prepare/party_damage.rs`).
- Burst healing, a MythicSim measure rather than a game mechanic: a healer sim reports, beside
  its healing per second over the fight (sustained, which its mana spreads thin), the most it
  heals in any ten seconds of each fight over ten seconds (burst), counted as healing per second
  is: heals, ticks and shields on the healer, less overhealing. Ten seconds is a judgement: a few
  casts and a heal over time's ticks, short enough that mana does not decide it. The fixtures'
  bursts run 3 to 4.7 times their healing per second. The prepared player carries
  `burst_window`, and the report's player `burstHps` (`src/prepare/burst_hps.rs`,
  `src/core/fight/burst.rs`). The reference, like Go, reports none.
- A healer's gear, from the client rows (1.60.1.70425): the reference counts many healing items
  and set bonuses for nothing, as it registers neither them nor their sets, and Rust refused a
  healer wearing a set it registers but whose listener Rust left unclaimed. In a healer sim,
  Forever preparation (`src/prepare/healer_gear.rs`) reads the bonus rows (ItemSetSpell, through
  `tools/forever_set_spells.py`) of every set the reference does not register: healing power,
  spell damage, mana per 5 seconds, spell crit, stats, casting regeneration and spell mods, as
  Vestments of Prophecy's Flash Heal and Prayer of Healing bonuses, Vestments of Faith's Renew
  cost or the PvP sets' healing; procs and conditional bonuses are left out. It adds the healing
  bonuses the reference leaves empty: Stormrage Raiment's Regrowth cast time and Rejuvenation
  duration, The Earthfury's Mana Surge (a 25% chance after Healing Wave or Lesser Healing Wave to
  regain 35% of its base cost) and Healing Wave jumping to two more players at 20% of the heal
  before each, and The Ten Storms' 30% more Chain Heal past the first player. It wires the healing
  idols and totems (Idol of Health, Idol of Longevity, Idol of Swiftness, Totem of Life, Totem of
  Flowing Water, Tidal Totem, Totem of Urgency) and Hazza'rah's Charm of Healing and Wushoolay's
  Charm of Nature, whose use gives their rows' cast time and cost mods for 15 seconds. And it
  claims Sudden Insight (Vestments of the Devout and Magister's Regalia, 5 pieces: a cast's 5%
  chance at 200 mana) and The Furious Storm (The Elements 5, The Five Thunders 4: a cast's 4%
  chance at 65 spell power for 10 seconds), which the reference registers. Totem of Life's 80 is
  added to Lesser Healing Wave's roll, as the reference adds Libram of Light's to Flash of Light.
  More set and item bonuses that need the fight: Vestments of Faith's Armor of Faith (a Greater
  Heal crit shields its player for 500) and Epiphany (a cast's 5% chance at 24 mana per 5
  seconds for 30 seconds), Dreamwalker Raiment's Healing Touch crit returning 30% of its cost,
  Vestments of Transcendence's rank 5 Renew on each Greater Heal, and Idol of Rejuvenation's 5%
  on Regrowth's direct heal while a Rejuvenation of the druid's is up (read as "on a target
  under your Rejuvenation", since heals land on whoever needs them), Idol of Synthesis (a second
  off Swiftmend's cooldown for each heal over time up as it is cast), Garments of the Oracle (3:
  a heal's 20% chance to heal the healer for 10% of it), Rotmender's Raiment (5: a heal's cast,
  at most every 2 seconds, puts up Rotmending's 40 every 3 seconds for 15 seconds, counted at
  once), The Earthshatterer (4: the party's Mana Spring 25% stronger; 8: Lightning Shield's 15
  mana per 5 seconds, read as kept up) and Redemption Armor (4: Lay on Hands 12 minutes sooner).
  On use, Blessed Prayer Beads heal for 648 and Enamored Water Spirit's totem restores 33 mana
  every 2 seconds for 24 seconds; Wushoolay's Charm of Spirits, a Lightning Shield trinket, is
  worn without effect. Bonuses that help only the heal's target (Dreamwalker's Rejuvenation
  mana, Totemic Power, Holy Power, Totemic Power's armor) change nothing the healer heals, and
  The Eye of Divinity has no effect in Forever (its tooltip is flavour text).
  The healer trinkets on use: Talisman of Ascendance, which the reference registers, stacks
  Ascendance on each landed heal; Zandalarian Hero Charm, which it does not, starts at twelve
  stacks of 17 spell damage and 34 healing (its tooltip; the row is a dummy) and loses one each
  spell cast; Scarab Brooch shields the player each direct heal lands on for 15% of the healing
  it did, counted on the item; Natural Alignment Crystal gives heals and their ticks 20% and
  spells 20% more cost. A stacking trinket whose stacks only melee or ranged hits move, as Badge
  of the Swarmguard, is worn without effect. The prepared effects are `mana_on_cast`,
  `spell_mod_aura_on_use`, `stacking_cd_on_use`, `shield_on_heal` and the heals' shapes; a sim
  that is not a healer's keeps the reference's gear (`tests/healer_gear.rs`).
- A healer's spells past its heals, from the client rows (1.60.1.70425): the reference has no
  heal for them. A Discipline priest's Penance heals at once and once a second for two more,
  each the bolt row's heal, and deals no damage. A Restoration druid channels Tranquility, its
  periodic heal on the party of five every 2 seconds for 10 seconds, on a 5 minute cooldown.
  Nature's Swiftness (the Druid's and the Shaman's) makes the next heal with a cast time instant,
  and that cast spends it. Water Shield's three globes are each spent, at most once every 3.5
  seconds, by a direct heal of the shaman's that crits, for 2% of the maximum mana (the 16
  October 2026 beta notes); the globes taken spell hits spend are not modelled, since the healer
  takes none. The prepared effects are `instant_cast_aura`, the shaman's `water_shield` and the
  heals' channel shapes (`tests/healer_spells.rs`).
- A healer Priest's talents that read a heal's target or its history, from the client rows and
  tooltips: Divine Aegis (431622) turns each crit of a direct heal, Penance's bolts and Prayer of
  Mending's charges into an absorb of 5%, 10% or 15% of the heal, counted as the absorb aura's
  (431624) shielding; the share is taken before the overhealing share comes off and the absorb
  is full, like Power Word: Shield's. A heal over time never shields. Renewed Hope (425280) gives
  Lesser Heal, Heal, Greater Heal, Flash Heal and Penance 2% crit a point on a member of the
  party with Weakened Soul (6788, 15 seconds) and cuts that debuff by 1 second a point: a heal
  goes to a member at random in proportion to the party's hit rates (the assumption above), and
  Power Word: Shield goes to the first member in the party's order without the debuff. Litany of
  Light (1317006) returns 5% or 10% of a heal's base mana cost when the heal before it was a
  different spell, not another rank of the same one. Desperate Prayer (19243) is a self heal at
  no cost on a 10 minute cooldown that a rotation casts when it names it. The prepared effect is
  `priest_heal_talents`, and a heal's `crit_absorb` (`tests/healer_spells.rs`).
- Healer talents past the heals, from the client rows and Forever's tooltips: Riptide's third
  effect makes a Chain Heal cast directly on its target heal 25% more. The sim has no
  per-player auras (a heal lands on a player who needs it), so a Chain Heal's first player is
  read as the Riptide holder while a Riptide of the shaman's is up, a healer swinging Chain Heal
  at the player it Riptided and the party model's Chain Heal starting on the tank; the jumps
  keep their falloff of the unboosted heal, and Riptide is not spent (its row states no
  consumption). Genesis (1223081) is 2% a rank on the periodic healing of Rejuvenation,
  Regrowth and Wild Growth, its first effect, and on Swiftmend, its second, which the
  reference's Genesis (Rejuvenation and Regrowth alone) did not reach. The prepared effect is the
  heal's `hot_bonus` with `first_target_only` (`tests/healer_talents.rs`).
- Mana Tide Totem and Light's Vigil, the last two healer talents, from the client rows.
  The Restoration shaman's own Mana Tide Totem (rank 4, 1325256; Forever's rules, the talent
  taken) costs the row's 60 mana, stands 13 seconds on a 5 minute cooldown and gives its Mana
  Tide row's (1325257) 290 mana every 3 seconds to its group; the sim models the shaman's own
  mana, as MP5 the way Mana Spring Totem's ticks are (483.3 MP5 for 13 seconds, 1,257 mana
  against four whole ticks' 1,160), the same MP5 as the raid's external Mana Tide Totem option,
  which it stacks with. It stands in the water totem slot: its cast ends a Healing Stream Totem
  and its heals over time, and a Healing Stream Totem cast ends it, so a rotation recasts neither
  while the other stands. A cast takes no target. Light's Vigil on an ally (rank 3, 1311595 at
  level 60; push 113554, the Holy Paladin healer sim) costs the row's 1,340 mana on a 1.5 second
  cast with a 6 second cooldown and puts a 30 second vigil on the player, one at a time; the next
  Holy Shock heal on the player spends it, triggers no cooldown and heals the player and the four
  others of the party for 704 plus 0.143 of healing power, each with its own roll and crit (Holy
  Power's crit healing reaches it and a group heal's overhealing share comes off; Healing Light
  does not, as its mask omits the vigil, and Illumination does not, since the heal has no cost
  to refund). The enemy strike and the refund are gone. Any other build's rotation that casts it is
  refused as before. The prepared effects are `mana_tide_totem` and `lights_vigil_ally`
  (`tests/healer_talents.rs`).
- Chance on hit items with no stated rate, a MythicSim assumption: Blizzard states no rate for
  them. Their client trigger rows read `ProcChance` 101, the server's "chance on hit" sentinel,
  with no procs per minute, and the reference leaves them unregistered (its generator's "states
  no rate" blocks, and rows it had no constructor for), so a sim wearing Seeping Willow,
  Empyrean Demolisher or Felstriker ran as if the effect did not exist and said nothing. Forever
  preparation registers them from those blocks and the client rows (`CHANCE_ON_HIT_ITEMS`,
  `tools/rust_item_effects.py`) at the rate Classic servers give a `ProcChance` of 101: 1.8% per
  landed hit for each second of the hitting weapon's speed, 1.08 procs a minute. A weapon procs on
  its own hits, special attacks included; any other item on the wearer's landed melee and ranged
  hits; spells never. A damage over time the proc puts on alone rolls the magic hit as it goes on
  where it is magic and its ticks deal their amount, one around the wielder goes on every target,
  and one a missile carries goes on as the hit lands, its flight not modelled.
  `docs/forever-chance-on-hit.md` lists the items by kind and those left out with the reason
  (crowd control, threat, mana, heals, debuffs only a tank feels). A wearer buff that changes
  stats (Destiny's Strength, Quel'Serrar's Armor and Attack Power, Ragehammer's Attack Power) is
  a stat aura, so its stats reach the runtime and a tank's swing rolls are read with it up
  (`chance_on_hit_stat_auras`, `src/prepare/export_items.rs`). Quel'Serrar's Sanctuary (22850)
  also grants 13 Defense (`A_MOD_SKILL` of skill line 95), which the effect parser leaves out;
  it is added to the buff (`src/prepare/forever_items.rs`). A tank's swing rolls read it, and
  the stat aura combinations carry the crit the player takes when one changes it, which the
  runtime sets on a Goblin Sapper Charge's hit on its thrower. Its 36 melee Attack Power against
  Dragonkin (aura 102, creature mask 2) is a `mob_type_bonus` with `melee_attack_power` while the
  buff is up and every target is Dragonkin; ranged attacks do not read it.
- Stat aura combinations up to 16 bits, where the reference refuses past 14: an Orc
  Enhancement Shaman with Jom Gabbar takes 15 (the totems, the swap, the imbues and the potion
  10, Blood Fury 1, Jom Gabbar's stacks 4). Each bit doubles the reset simulations: 15 bits
  prepare in about 5 seconds (`src/prepare/stat_auras.rs`).
- The Black Book (19337) as the Forever client states it: Blessing of the Black Book (23720)
  lands on the demon that is out, every effect of its row on the pet, 100% more damage done in
  every school (aura 79) and 100% more armor (aura 101) for 30 sec, and the item's use is on a 5
  minute cooldown and its category's 30 seconds (client 70707 ItemEffect 102765). The pinned
  reference registers Classic's book instead (200 spell damage, 325 attack power and 1600 armor
  on each demon, a 3 minute cooldown, a warlock aura no effect describes), which the reference
  rules keep and still refuse. Under Forever's rules the use waits for a demon out and doubles its
  damage (`black_book`, `src/classes/warlock/prepare/items.rs`); the armor is read by nothing, as
  nothing swings at the demon.
- Forever's Engineering ammo, from the Wowhead Forever tooltips: Swiftfeather Arrow (274387) and
  Swiftstrike Shot (274388), item level 65 and requiring level 60, each "Adds 24.5 damage per
  second". The reference's ammo stops at Miniature Cannon Balls, so the request schema adds them
  to `HunterOptions.Ammo` as `SwiftfeatherArrow` (13) and `SwiftstrikeShot` (14)
  (`src/contracts/request/schema.rs`), leaving `data/proto-schema.json` as the pinned reference
  wrote it. They are data, not a departure, so both rule sets add the 24.5.
- Shadow Oil, from the client 1.60.1.70707 data: item 3824 (use spell 3594, level 24) puts
  temporary enchantment 25 on a melee weapon, a proc effect at the 15% its `EffectPointsMin`
  states, casting spell 350026, a Shadow Bolt of Shadow damage that rolls 52 plus 0.9 a level with
  0.15 variance and takes 0.56 of spell power. It is Frost Oil's twin (item 3829, use spell 3595,
  enchantment 26, 10% of Frostbolt 350025, the fork's patch 126): a flat chance on each landed hit
  of the oiled hand, not procs per minute, with no proc manager, so `mhImbueId` and `ohImbueId`
  take 3594 or 3824 as they take 3595 or 3829, the oil on the main hand's swap weapon follows the
  item swap as Frost Oil does, and each oiled hand has its own listener (`Shadow Oil (main hand)`,
  `Shadow Oil (off hand)`) with the bolt's row `ProcChance` as its chance. The Go reference has no
  Shadow Oil and ignores the id, so both rule sets register it, as with the ammo above
  (`src/prepare/proc_oil.rs`, `tests/classes/proc_oil.rs`).
- Oil of Immolation, from the client 1.60.1.70707 data: item 8956 (level 31) uses spell 11350
  "Fire Shield", a 15 second aura with a periodic trigger spell effect every 3 seconds that casts
  spell 11351 on the user, a Fire school damage effect of 50 base points (a flat 50, no spell
  power) on the enemies within 5 yards of the caster. The use has no cooldown and no global
  cooldown, and its category (79, 3 seconds) is not the potion category (4), so it shares nothing
  with the potions. Neither row is in the pinned game data, so `src/prepare/oil_of_immolation.rs`
  restates them as literals. A request opts in with `consumables.oilOfImmolation: true`, a
  Rust-only boolean of the consumables message (`FOREVER_BOOL_FIELDS` in
  `src/contracts/request/schema.rs`, field 1000; the Go reference refuses it as an unknown
  field). The character uses the oil 0.5 s before the pull, so the shield is up at the pull, and
  again each time the shield ends, all fight. Every 3 seconds from each use (the first tick 2.5 s
  into the fight), the fifth on the moment the shield ends, spell 11351 deals its 50 as a magic
  hit with a crit to every target, rolled for each target in turn, and shows in the actions as
  spell 11351; the shield's uptime shows as aura 11350 ("Oil of Immolation"). The client rows
  have no tick at the moment of the use (no `START_PERIODIC_AT_APPLY` on spell 11350), so a use
  loses no tick. Using the oil while a swing timer is running stalls the auto attack for 0.5 s,
  which is why players use it before the pull, where none is running (a report from Bae, a
  Classic player on the project's Discord, not a client row: "0.5 sec swingtime stunt when using
  it during swings"). Each use in combat therefore pushes the player's running melee swings, the
  main hand and a dual wielder's off hand, back by 0.5 s (`swing_delay_ns` on the effect); a hand
  that is not swinging (out of melee range, or before the pull) loses nothing and the ranged auto
  is not delayed. Bae also said a prepot "loses 1 of the 5 ticks"; with the client's timing a use
  at -0.5 s loses none, so the rules do not drop one. The fight models one distance between the player and the targets
  (`distanceFromTarget`), so the tick reaches all of the targets or none: a character beyond 5
  yards, such as a ranged character at range, takes none. Nothing changes without the field
  (the prepared effect is `oil_of_immolation`, `src/core/fight/oil_of_immolation.rs`,
  `tests/classes/oil_of_immolation.rs`).
- The Zul'Gurub crafted sets, from the client 1.60.1.70425 ItemSet and ItemSetSpell rows: Blood
  Tiger Harness needs Leatherworking and The Darksoul Blacksmithing (RequiredSkill 165 and 164
  at 300), as Bloodvine Garb needs Tailoring, and Bloodsoul Embrace and The Darksoul give their
  bonus at 3 pieces, not the reference's 2 (`data/go-tables.json` carries the 3). They are data,
  so both rule sets follow them; no Go golden wears enough pieces to reach either count
  (`src/prepare/forever_item_sets.rs`).
- Beaststalker Armor (Dungeon Set 1) on a class other than a Hunter: Forever does not lock the
  set to its class (classMask 0), and its five piece bonus reads "Your normal ranged and melee
  attacks have a chance of restoring 200 mana" (trigger row 450577, 5% on landed autoattacks).
  The reference attaches the trigger to any wearer but its handler only in the Hunter package, so
  on anyone else the listener did nothing and Rust refused it. Under Forever's rules it restores
  the 200 mana to any wearer with a mana bar, a spell batch window after the hit, as the Hunter's
  does (the prepared effect is `mana_proc`, `src/prepare/export_items.rs`); a Hunter keeps
  `hunter_set_mana_proc`, and the reference rules still refuse it.

## Repository layout

| Path | Contents |
| --- | --- |
| `src/core`, `src/mechanics` | Shared fight runtime and combat rules |
| `src/classes/<class>` | Class spells and talents, with specs under `specs/` |
| `src/prepare` | Preparing a character from a request |
| `src/engine` | Coverage gate and orchestration |
| `src/contracts` | Input and output types |
| `tests/classes` | Tests, one folder per class |
| `fixtures` | Prepared inputs and Go results |
| `tools` | Exporter, routing, comparison and validation tools |

The [contributor code map](docs/contributor-guide.md) shows where each mechanic lives.

## Contribute

Read [CONTRIBUTING.md](CONTRIBUTING.md) for the checks to run. A mechanics change needs a
fixture or test that shows it matches Go. Open work is on the
[contributor board](https://github.com/sage3648/mythicsim-forever-engine/issues/25).

## License and acknowledgments

MIT licensed, with new project work credited to MythicSim contributors.
Upstream wowsims notices are retained for adapted portions in [LICENSE](LICENSE).
RNG behavior, combat semantics and reference tooling derive from
[wowsims Forever](https://github.com/ElliotWood/Forever) and
[MythicSim's Go fork](https://github.com/sage3648/mythicsim-forever-engine-go).
See [NOTICE.md](NOTICE.md) for provenance. This project is not affiliated with
Blizzard Entertainment or the WoW Forever server team.
