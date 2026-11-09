# In-process Rust leveling evaluator (8 / 100 mobs)

## Explicit ownership

- **Rust/msim:** one deterministic (balanced RNG) or stochastic native combat actor,
  all spell/APL execution, pet actions, mob health and lethal transitions,
  2D world positions and movement, resource regeneration, food and drink actions,
  survival, cooldowns, and timestamps. One Rust invocation evaluates one
  build and returns one result, with no Python callback during a fight.
- **Python/optimizer:** select candidate equipment, 21 talent points, pet,
  unrestricted priority/conditional APL, and consumable/recovery strategy,
  call `msim eval-leveling`, consume returned Rust measurements,
  fit the surrogate, mutate candidates, choose native 100-mob finalists.
  It MUST NOT simulate combat, regenerate mana/HP, decide a native kill,
  execute movement, or supply an invented 100-mob completion.
- **Source:** Rust `get_options` provides allowed action/item/talent options,
  with Rust preparation the final legality and effect authority.

## Why change the current bridge?

The current `native-world-pulls` keeps one Rust process and one actor for an
8–100 mob chain, but on each world boundary Rust writes a
`native_rust_world_boundary_request`, reads a JSON `gsim_world_range_tick`,
writes `native_rust_world_receipt`, and reads a JSON
`gsim_native_damage_applied_ack` from the Python gsim controller.
During travel, native scheduler also samples boundaries each 100 ms.
Thus one candidate pays many interpreter/process crossings and JSON
serializations. Only the optimizer-candidate boundary should cross into Python.

## Fast evaluator contract

Proposed CLI (not implemented by this first spatial-geometry PR):

```text
forever-engine eval-leveling --request l30-warlock.json \
  --scenario ordinary-l30-spaced-pulls.json \
  --targets 8 --rng-mode balanced \
  --out results.json
```

`--targets 100` is the same evaluator, not a special scoring or recovery
implementation. The scenario must explicitly source target level, HP, location,
movement speed, equipment food/water inventory, and recovery effects.
Food/water must be *actual Rust scheduler actions* on the retained actor;
a passive 12-second interval is neither drinking nor eating.
When the next pull is not survivable, evaluate alternative paid recovery
actions (rest/eat/drink) under the same native semantics. HP and mana may
recover simultaneously, but never for free or via synthetic resets.

Results: `scenario_id`, `engine_sha`, `build_hash`, `targets_requested`,
`targets_killed`, `elapsed_s`, `died`, `death_index`, per-target
`{kill_clock, hp_before/after, mana_before/after, native_casts, target_xy,
travel_s, food_s, drink_s, recovery_receipts}`, and complete
`{gear, talents, pet, apl}` identity. The same request / seed must be
reproducible. Full 100-mob results are the only full-chain labels.

## Incremental native implementation

1. **Port spatial geometry with source parity tests** (this PR):
   Chebyshev square lattice at 20 yards; polar-angle ring order; greedy
   nearest-unvisited route with source index tie-break, 7 yards/s travel,
   source approach radius, and no assumed one-dimensional mob row.
2. **In-process world state:** position, facing, target selection, native mob
   health, event-time movement, and native damage journal application.
   Refactor the world boundary to select an external-gsim adapter or
   in-process adapter *without changing Fight::deal_damage or spell effects*.
3. **Native recovery:** replace fixed passive 12 s with actual food/drink
   effects and timing; select rests according to survivability and elapsed-time
   minimization, not HP/mana monotonicity.
4. **Add the CLI and results**, then call it from the Python optimizer and
   remove per-event `live_two_world_pulls.run` use from the **new** optimizer
   path only. Keep existing gsim bridge regression as independent oracle.
5. **Benchmark/publish:** same 8 and 100 L30 Warlock candidates with same
   Rust source, balanced RNG, monster HP and movement, comparing
   per-kill clocks, HP/mana, native casts, 2D distances, lethal ACK, and
   complete duration. Record wall-clock and process crossings; do not
   assert an unmeasured speedup or a parity pass.

## Acceptance gates

- Original external gsim bridge stays unchanged until independent parity tests
  pass on a selection of builds including Drain Life, Life Tap, ranged Shoot,
  pet choices, motion and deaths.
- In-process world delivers the same **native** damage, HP/mana,
  cooldowns, APL decisions and death semantics as the same Rust engine;
  no Python-supplied synthetic refills or surrogate combat.
- 8-mob optimizer ranking and real 100-mob finalist results are computed
  from *the new same-version* in-process evaluator; previous external-bridge
  numbers are reference comparisons, never silently mixed training labels.
- The change is an evaluation performance refactor: any gameplay outcome
  changes must be reported and traced, not accepted as a throughput fix.

## Scope limits

The first spatial module is only a data-structure port and unit test;
it does **not** claim the single-process evaluator exists yet.
Serial ordinary mobs with zero enemy movement are the first supported
spatial scenario. Aggro, multi-mob concurrent combat and full arbitrary
gsim world semantics need their own equivalence work, not silent simplification.
