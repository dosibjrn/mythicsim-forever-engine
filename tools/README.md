# Reference and validation tools

These tools are development helpers. The Rust engine does not call Go during a
fight. Ordinary Rust tests use frozen data and need no Go checkout.

| Tool | Purpose | Requirements |
| --- | --- | --- |
| [compare.py](compare.py) | Compare the Rust kernel with the actual pinned Go engine | Rust, Go, Python, Git, protoc |
| [fair_compare.py](fair_compare.py) | Time equivalent Go/Rust kernels with work checks and a full-engine oracle | Same tools as the comparison |
| [inventory.py](inventory.py) | Audit the first-build inventory offline; optionally recapture and compare Go output | Python for audit; Go and pinned source checkouts for capture |
| [oracle/main.go](oracle/main.go) | Prepare restricted reference cases and run the actual Go engine | Built by the comparison tool in isolated scratch |
| [prepared_v2.py](prepared_v2.py) | Audit prepared v2 fixtures offline; re-export them from the pinned engine into scratch | Python for audit; Go, Git and protoc for capture |
| [census.py](census.py) | Count which prepared inputs or requests Rust would run and rank the reasons blocking the rest | Python and Rust for prepared inputs; Go, Git and protoc to export requests |
| [production_bench.py](production_bench.py) | Time the pinned Go engine and two Rust builds on the production prepared v2 fixtures, checking the Rust results agree | Python, two Rust release binaries and the oracle `prepared_v2.py` builds |
| [architecture_check.py](architecture_check.py) | Run every accepted Go golden against the pinned engine built for linux/arm64 or linux/amd64 in a Docker image of that platform, and record the differences | Python, Go, Docker and the oracle cache |
| [route.py](route.py) | Run one production request in Rust, or report a fallback with refusal codes or a fault, for the application worker ([routing guide](../docs/routing.md)) | Python and a bundle from `shadow.py build` |
| [board.py](board.py) | Render the ordered contributor board (issue #25) from milestones, priority labels, "Depends on" sections and `Status:` comments, list record refusals no issue states, and check or update the issue | Python and the GitHub CLI |
| [validation_summary.py](validation_summary.py) | Summarize every validation and benchmark record into [docs/validation-summary.md](../docs/validation-summary.md), with totals | Python |
| [boards.py](boards.py) | Write the application's published race board requests, one per spec and race, for a comparison | Python |
| [sweep_record.py](sweep_record.py) | Write a sweep's validation record from a comparison run | Python |
| [sweep.py](sweep.py) | Generate seeded randomized variants of one request for a compatibility sweep | Python |
| [copy_targets.py](copy_targets.py) | Write requests against 2 to 5 copies of the boss from requests against one, as the application's Advanced targets setting does, optionally only the tank builds | Python |
| [preset_matrix.py](preset_matrix.py) | Cross every class's upstream UI preset rotations, talents and gear sets into requests over the production requests | Python |
| [dev_notes_check.py](dev_notes_check.py) | Check the numbers Forever's rules take from the development notes of 2 November 2026 against a live client spell store (wowsims/forever's `spell_store_inputs.json`), as live, still old or differing | Python |
| [upstream.py](upstream.py) | Audit the [upstream ledger](../UPSTREAM.md#ledger), optionally against a community clone | Python; Git for the range check |
| [oracle-v2/main.go](oracle-v2/main.go) | Export a reset Go simulation as [prepared v2](../docs/prepared-v2.md) and run the full Go engine | Built by prepared_v2.py in isolated scratch |
| [matched-go/](matched-go/) | Go implementation of the same narrow Rust kernel for fair timing | Go |
| [reference-capture/](reference-capture/) | Standalone request and observation helper programs | Built in temporary modules by inventory capture |
| [forever_set_spells.py](forever_set_spells.py) | Write `src/prepare/forever_set_spells_generated.rs`, the client's set bonus rows (ItemSet, ItemSetSpell from wago.tools at the data's client build) of every set the item data holds, for the healer sims' gear | Python and the two CSV tables |
| [app-timeline/main.go](app-timeline/main.go) | Parse Go and Rust logs with the application's timeline helpers and compare | Go and an application checkout, in a scratch module |

Run the lightweight checks from the repository root:

```sh
python3 -m unittest discover -s tools -p '*_test.py'
python3 tools/inventory.py check
python3 tools/prepared_v2.py check
python3 tools/upstream.py check
```

Survey a set of inputs, such as exported application requests, before routing them:

```sh
python3 tools/census.py --output output/census/report.json REQUESTS_OR_DIRECTORIES...
```

The [kernel guide](../docs/kernel.md), [inventory guide](../docs/first-frost-inventory.md)
and [Go parity report](../docs/go-parity-validation-2026-10-03.md) describe scope and
reproduction commands. Comparisons write scratch results under ignored `output/`
and build the oracle under ignored `oracle-cache/`. They do not update accepted
fixtures, engine pins or production jobs.

New benchmark source digests include every nested Rust source module, Cargo
metadata and the matched Go implementation. Historical digests remain unchanged.
