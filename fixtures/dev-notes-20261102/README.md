# Development notes of 2 November 2026

`arcane-mage.request.json` is the request behind the Arcane Mage trinket board (the live
reference build, 10,000 iterations), read by [tests/dev_notes_20261102_rest.rs](../../tests/dev_notes_20261102_rest.rs)
to check that Arcane Missiles' missiles roll for Clearcasting and that an Arcane Missiles cast
beside Missile Barrage keeps it. The talent string is in client 1.60.1.70707's order, so only
Forever's rules read it.
