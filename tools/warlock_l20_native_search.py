#!/usr/bin/env python3
"""Actual level-20 Troll Warlock search through pinned Rust prepare/simulate.

Rust get_options is the sole candidate source. Python mutates the documented
request structure, never certifies any Warcraft rule or invents a DPS score.
All scored rows are real native balanced simulations; refused rows are kept.
This is a bounded exploratory optimizer, not a proven global optimum.
"""
from __future__ import annotations

import argparse
import copy
import json
import math
import random
import subprocess
from collections import Counter
from pathlib import Path

SLOTS = {
    "head": (0,), "neck": (1,), "shoulder": (2,), "back": (3,),
    "chest": (4,), "wrist": (5,), "hands": (6,), "waist": (7,),
    "legs": (8,), "feet": (9,), "finger": (10, 11),
    "trinket": (12, 13), "one_hand": (14,), "main_hand": (14,),
    "two_hand": (14,), "off_hand": (15,), "ranged": (16,),
}
# Source spell candidates are discovered from Rust get_options(), not a
# Warlock-specific Python ability table or a fixed list of known spell IDs.


def invoke(command, *, timeout=30):
    result = subprocess.run(command, capture_output=True, text=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError(
            "exit=" + str(result.returncode) + " " +
            (result.stderr or result.stdout)[-450:])
    if result.stdout.strip():
        return json.loads(result.stdout)
    return None


def player(request):
    return request["raid"]["parties"][0]["players"][0]


def ranked_items(options):
    pools = {i: [] for i in range(17)}
    for item in options["items"]:
        native = item.get("native") if item.get("eligibility") == "native_eligible" else None
        if native is not None:
            slot = native.get("slot")
            positions = SLOTS.get(slot, ())
            record = native
        elif item.get("eligibility") == "proficiency_unverified":
            # No guessed proficiency or armor rule. Explore structurally; Rust
            # may refuse each submitted full equipped request.
            positions = tuple(range(17))
            record = item.get("unverified") or {}
        else:
            continue
        iid = item.get("item_id")
        if type(iid) is not int or iid <= 0:
            continue
        stats = record.get("stats") or {}
        spell = sum(float(stats.get(k, 0) or 0)
                    for k in ("SpellDamage", "ShadowDamage", "FireDamage"))
        ilvl = record.get("item_level") or 0
        priority = (spell, float(ilvl), iid)
        for index in positions:
            pools[index].append((priority, iid))
    return {
        index: list(dict.fromkeys(
            iid for _, iid in sorted(rows, reverse=True)))[:4]
        for index, rows in pools.items() if rows
    }


def talent_candidates(options):
    trees = options["talents"]["trees"]
    candidates = [("no_talents", "0-0-0")]
    focus = [
        ("affliction", ("improvedCorruption", "suppression", "improvedBaneOfAgony")),
        ("demonology", ("demonicEmbrace", "unholyPower", "felVitality")),
        ("destruction", ("bane", "improvedShadowBolt", "ruin")),
    ]
    budget = int(options["talents"]["points_available"])
    for tree_index, (label, names) in enumerate(focus):
        talents = trees[tree_index]["talents"]
        for variation in range(2):
            ranks = [0] * len(talents)
            remaining = budget
            ordered = names if variation == 0 else (names[1], names[0], names[2])
            for name in ordered:
                entry = next((row for row in talents if row["field_name"] == name), None)
                if entry is None or remaining <= 0:
                    continue
                allowed = min(int(entry["max_rank"]), remaining)
                ranks[int(entry["index"])] = allowed
                remaining -= allowed
            if remaining:
                for entry in talents:
                    if remaining == 0:
                        break
                    index = int(entry["index"])
                    # Source-provided thresholds only; native prepare remains
                    # sole legality authority for the complete talent string.
                    if int(entry["points_required_in_tree"]) > budget - remaining:
                        continue
                    available = int(entry["max_rank"]) - ranks[index]
                    add = min(remaining, available)
                    ranks[index] += add
                    remaining -= add
            whole = ["0", "0", "0"]
            whole[tree_index] = "".join(str(rank) for rank in ranks)
            candidate = "-".join(whole)
            if all(candidate != value for _, value in candidates):
                candidates.append((label + "_" + str(variation + 1), candidate))
    return candidates


def native_spell_candidates(options: dict, request: dict) -> list[dict]:
    """Collect possible APL actions exclusively from Rust get_options spell rows.

    Class-family discovery may contain triggers/passives/unlearned actions.
    We never proclaim them legal: Rust alone decides which ones execute.
    Prefer the highest available rank per source spell name, and also include
    any preexisting source APL spell identity for a reproducible baseline.
    """
    by_name = {}
    for source in options.get("spells", []):
        spell_id = source.get("spell_id")
        level = source.get("minimum_level")
        name = source.get("name")
        if (type(spell_id) is not int or spell_id <= 0
                or not isinstance(name, str) or not name.strip()
                or (level is not None and (type(level) is not int or level > options["level"]))):
            continue
        label = name.strip().casefold()
        old = by_name.get(label)
        if old is None or (level or 0, spell_id) > (old.get("minimum_level") or 0, old["spell_id"]):
            by_name[label] = source
    spells = sorted(by_name.values(), key=lambda x: (
        -(x.get("minimum_level") or 0), x["spell_id"]))
    used_ids = {row["spell_id"] for row in spells}
    for entry in player(request).get("rotation", {}).get("priorityList", []):
        identity = ((entry.get("action") or {}).get("castSpell") or {}).get("spellId") or {}
        spell_id = identity.get("spellId")
        if type(spell_id) is int and spell_id not in used_ids:
            spells.insert(0, {"spell_id": spell_id, "name": "source_apl_" + str(spell_id),
                              "minimum_level": None, "source": "existing_source_request"})
            used_ids.add(spell_id)
    return spells


def cast_apl(spell_id, *, guard=None):
    """Use MythicSim's native APL condition schema, never emulate a spell."""
    entry = {"action": {"castSpell": {"spellId": {"spellId": spell_id}}}}
    if guard == "dot_missing":
        entry["action"]["condition"] = {"not": {"val": {
            "dotIsActive": {"spellId": {"spellId": spell_id}}}}}
    elif guard == "aura_missing":
        entry["action"]["condition"] = {"not": {"val": {
            "auraIsActive": {"auraId": {"spellId": spell_id},
                             "sourceUnit": {"type": "CurrentTarget"}}}}}
    elif guard is not None:
        raise ValueError("Unknown native APL guard " + str(guard))
    return entry


def mana_conditioned_apl(spell_id, percent: int):
    entry = cast_apl(spell_id)
    entry["action"]["condition"] = {"cmp": {
        "lhs": {"currentManaPercent": {}}, "op": "OpLt",
        "rhs": {"const": {"val": str(percent) + "%"}}}}
    return entry


def multi_apl(spells: list[tuple[int, str | None]]) -> list[dict]:
    """Generate ordered APL priorities with optional Rust-evaluated guards."""
    return [cast_apl(spell_id, guard=guard) for spell_id, guard in spells]


def summarize(native, request):
    body = native["result"]
    actor = body["raidMetrics"]["parties"][0]["players"][0]
    dps = actor["dps"].get("avg")
    if not isinstance(dps, (float, int)) or not math.isfinite(float(dps)):
        raise ValueError("native score was not a finite DPS")
    actions = []
    for action in actor.get("actions", []):
        targets = action.get("targets") or []
        done = sum(t.get("casts", 0) or 0 for t in targets if isinstance(t, dict))
        damage = sum(t.get("damage", 0) or 0 for t in targets if isinstance(t, dict))
        if done or damage:
            actions.append({"id": action.get("id"), "casts": done, "damage": damage})
    return {
        "dps": float(dps),
        "duration_s": body.get("firstIterationDuration"),
        "iterations": body.get("iterationsDone"),
        "source_revision": native.get("source_revision"),
        "request_sha256": native.get("request_sha256"),
        "actions": actions,
        "talents": player(request).get("talentsString"),
        "equipment": player(request).get("equipment", {}).get("items"),
        "apl": player(request)["rotation"]["priorityList"],
    }


def choose_paid_gear_upgrade(baseline_dps, trials):
    """Only accept a Rust-paid item improvement over the actually equipped build.

    Each trial is (item_id, native-paid row or None). The engine decides gear
    validity and scores; this optimizer merely keeps the best measured gain.
    """
    winner_id = None
    winner_dps = baseline_dps
    for item_id, row in trials:
        if row is not None and row["dps"] > winner_dps:
            winner_id = item_id
            winner_dps = row["dps"]
    return winner_id, winner_dps


def native_gear_refinement_proposals(equipped_request, slot_index, source_item_ids):
    """Vary just one slot of a paid native build, preserving other axes.

    Item IDs come from Rust options and every full proposal goes back through
    native prepare/simulate. This helper never asserts item legality.
    """
    equipped = player(equipped_request)["equipment"]["items"]
    current_id = equipped[slot_index]["id"]
    for item_id in dict.fromkeys(source_item_ids):
        if item_id == current_id:
            continue
        proposal = copy.deepcopy(equipped_request)
        player(proposal)["equipment"]["items"][slot_index] = {"id": item_id}
        yield item_id, proposal


def refine_paid_gear(equipped_request, pools, score, *, passes=2, label_prefix="gear"):
    """Revisit earlier slots after later native-paid gear changes their context.

    Every candidate is a source item in the current full talents/APL build,
    scored by Rust. Stop on a no-upgrade pass; never accept a score regression.
    """
    working = copy.deepcopy(equipped_request)
    baseline = score(label_prefix + "_baseline", working)
    if baseline is None:
        raise RuntimeError("Native equipped gear baseline refused")
    current_dps = baseline["dps"]
    for pass_index in range(passes):
        upgraded = False
        for index in sorted(pools):
            trials = [
                (item_id, score(
                    label_prefix + "_pass_" + str(pass_index + 1) +
                    "_slot_" + str(index) + "_item_" + str(item_id),
                    proposal,
                ))
                for item_id, proposal in native_gear_refinement_proposals(
                    working, index, pools[index][:3])
            ]
            chosen_id, improved_dps = choose_paid_gear_upgrade(current_dps, trials)
            if chosen_id is not None:
                player(working)["equipment"]["items"][index] = {"id": chosen_id}
                current_dps = improved_dps
                upgraded = True
        if not upgraded:
            break
    return working, current_dps


def configured_request(request, *, duration, iterations):
    """Align a source-native candidate with the same paid search scenario.

    Preserve the full supplied equipment/talents/APL; Rust prepare and sim
    determine whether that candidate is executable and how much it scores.
    """
    aligned = copy.deepcopy(request)
    aligned["simOptions"].update({
        "iterations": iterations, "randomSeed": "4242",
        "debugFirstIteration": False,
    })
    aligned["encounter"]["duration"] = duration
    return aligned


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--request", type=Path, required=True)
    parser.add_argument("--incumbent-request", type=Path,
                        help="Previously paid full native build to improve, never an assumed score")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--duration", type=int, default=30)
    parser.add_argument("--iterations", type=int, default=8)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    base = configured_request(json.loads(args.request.read_text()),
                              duration=args.duration, iterations=args.iterations)
    player(base)["equipment"] = {"items": [{"id": 0} for _ in range(17)]}
    incumbent = (configured_request(json.loads(args.incumbent_request.read_text()),
                                    duration=args.duration, iterations=args.iterations)
                 if args.incumbent_request else None)

    option_data = invoke([str(args.binary), "options", "--class", "warlock", "--level", "20"])
    if (option_data["class"] != "warlock" or option_data["level"] != 20
            or option_data["talents"]["points_available"] != 11):
        raise RuntimeError("Rust returned wrong level/class source")

    pools = ranked_items(option_data)
    talents = talent_candidates(option_data)
    source_spells = native_spell_candidates(option_data, base)
    (args.out / "rust_spell_candidates.json").write_text(
        json.dumps(source_spells, indent=2) + "\n")
    print("WARLOCK_L20_RUST_SPELL_OPTIONS", json.dumps([
        {"name": row["name"], "spell_id": row["spell_id"],
         "minimum_level": row.get("minimum_level")}
        for row in source_spells
    ]), flush=True)
    total_attempts = 0
    accepted = []
    refused = []
    refused_reasons = Counter()
    seen = set()
    paid_cache = {}
    best = None
    best_request = None

    def score(label, request):
        nonlocal total_attempts, best, best_request
        key = json.dumps(request, sort_keys=True)
        if key in seen:
            # Reusing an already paid request must reuse its genuine native
            # score, not silently turn an executable build into a refusal.
            return paid_cache.get(key)
        seen.add(key)
        total_attempts += 1
        req = args.out / "candidate.json"
        prepared = args.out / "candidate_prepared.json"
        output = args.out / "candidate_paid.json"
        req.write_text(json.dumps(request))
        try:
            invoke([str(args.binary), "prepare", "--request", str(req),
                    "--actor-level", "20", "--outfile", str(prepared)])
            prepared_json = json.loads(prepared.read_text())
            prepared_actor = prepared_json["player"]
            if (prepared_actor.get("level") != 20
                    or prepared_actor.get("class") != "ClassWarlock"
                    or prepared_actor.get("race") != "RaceTroll"):
                raise RuntimeError("Rust prepared a different actor")
            invoke([str(args.binary), "sim", "--infile", str(prepared),
                    "--rng-mode", "balanced", "--outfile", str(output)])
            paid = json.loads(output.read_text())
            row = {"label": label, **summarize(paid, request)}
            accepted.append(row)
            paid_cache[key] = row
            if best is None or row["dps"] > best["dps"]:
                best = row
                best_request = copy.deepcopy(request)
                (args.out / "best_request.json").write_text(json.dumps(request, indent=2) + "\n")
                (args.out / "best_native_output.json").write_text(json.dumps(paid, indent=2) + "\n")
            print("PAID", label, "dps", round(row["dps"], 4),
                  "best", round(best["dps"], 4), flush=True)
            return row
        except (RuntimeError, ValueError, KeyError, subprocess.TimeoutExpired) as exc:
            paid_cache[key] = None
            reason = str(exc)[-320:]
            refused_reasons[reason] += 1
            if len(refused) < 40:
                refused.append({"label": label, "reason": reason})
            print("REFUSED", label, reason[-130:], flush=True)
            return None

    # Native get_options is the only spell candidate source. Discover real
    # castable and damaging actions by submitting each to Rust rather than
    # maintaining a parallel Python spell proficiency table.
    if incumbent is not None:
        # Never carry an old numeric score across source revisions, actor or
        # scenario changes. Pay the whole incumbent through the selected Rust.
        if score("equipped_incumbent", incumbent) is None:
            raise SystemExit("The source-native incumbent refused; no paid warm start")
    baseline = copy.deepcopy(base)
    score("source_baseline_apl", baseline)
    for source in source_spells:
        sid = source["spell_id"]
        proposal = copy.deepcopy(base)
        player(proposal)["rotation"]["priorityList"] = [cast_apl(sid)]
        score("spell_" + str(sid), proposal)

    positive_actions = {}
    for row in accepted:
        entries = row.get("apl") or []
        if len(entries) != 1:
            continue
        sid = ((entries[0].get("action") or {}).get("castSpell") or {}).get("spellId", {}).get("spellId")
        if type(sid) is not int or row["dps"] <= 0:
            continue
        if not any(a.get("casts", 0) > 0 and a.get("id", {}).get("spellId") == sid
                   for a in row["actions"]):
            continue
        if sid not in positive_actions or row["dps"] > positive_actions[sid]:
            positive_actions[sid] = row["dps"]
    # Actual native-paid single-action winners form the mutation frontier.
    # This is candidate ranking, not a claim that untested spell IDs are illegal.
    action_ids = sorted(positive_actions, key=lambda sid: -positive_actions[sid])[:8]
    print("WARLOCK_L20_NATIVE_CASTABLE_FRONTIER", json.dumps([
        {"spell_id": sid, "measured_dps": positive_actions[sid]}
        for sid in action_ids]), flush=True)

    # Every pair is an APL priority order, not two independent simulations.
    # Test unconditional, dot-maintenance and aura-maintenance behavior.
    for lead in action_ids:
        for fallback in action_ids:
            if lead == fallback:
                continue
            for condition in ("dot_missing", "aura_missing", None):
                proposal = copy.deepcopy(base)
                player(proposal)["rotation"]["priorityList"] = multi_apl([
                    (lead, condition), (fallback, None)])
                score("apl_pair_" + str(lead) + "_" + str(fallback) + "_" +
                      str(condition), proposal)

    # Three-step priorities: two native conditional spell maintenance actions
    # followed by a native-paid filler. Rust determines whether guards work.
    main_fillers = action_ids[:3]
    for filler in main_fillers:
        for i, lead in enumerate(action_ids[:6]):
            for second in action_ids[:6][i+1:]:
                if filler in (lead, second):
                    continue
                proposal = copy.deepcopy(base)
                player(proposal)["rotation"]["priorityList"] = multi_apl([
                    (lead, "dot_missing"), (second, "dot_missing"),
                    (filler, None)])
                score("apl_triple_" + str(lead) + "_" + str(second) +
                      "_" + str(filler), proposal)

    # Resource actions are discovered by Rust, and their mana policy is just
    # a generic native currentManaPercent condition. Test only source entries
    # that Rust identifies by the source spell name; never infer learnability.
    resource_spells = [row["spell_id"] for row in source_spells
                       if "life tap" in row["name"].casefold()]
    for resource in resource_spells[:2]:
        for threshold in (20, 40, 60):
            for filler in action_ids[:4]:
                proposal = copy.deepcopy(base)
                player(proposal)["rotation"]["priorityList"] = [
                    mana_conditioned_apl(resource, threshold), cast_apl(filler)]
                score("apl_mana_" + str(resource) + "_" + str(threshold) +
                      "_" + str(filler), proposal)

    # Evaluate source-native full builds under the actual selected talents/APL.
    # Revisit earlier slots if subsequent native-paid upgrades change context.
    if best is None:
        raise SystemExit("No executable level-20 Warlock candidate; see refusals")
    working, equipped_dps = refine_paid_gear(
        best_request, pools, score, passes=2, label_prefix="initial_gear")

    # Native-sourced talent tree candidates at the actual 11-point L20 budget.
    for label, encoded in talents:
        trial = copy.deepcopy(working)
        player(trial)["talentsString"] = encoded
        score("talent_" + label, trial)

    # Joint re-scoring must include *full source-derived priority lists*,
    # not collapse back to a fixed one-spell APL. Re-test the strongest actual
    # paid priorities with each of the strongest source-derived talent builds.
    paid_talents = list(dict.fromkeys(
        row["talents"] for row in sorted(accepted, key=lambda x: -x["dps"])
        if row["talents"] is not None))[:3]
    paid_apls = []
    paid_apl_keys = set()
    for row in sorted(accepted, key=lambda x: -x["dps"]):
        apl = row["apl"]
        key = json.dumps(apl, sort_keys=True)
        if key not in paid_apl_keys:
            paid_apl_keys.add(key)
            paid_apls.append(apl)
        if len(paid_apls) == 15:
            break
    for encoded in paid_talents:
        for index, apl in enumerate(paid_apls):
            trial = copy.deepcopy(working)
            player(trial)["talentsString"] = encoded
            player(trial)["rotation"]["priorityList"] = copy.deepcopy(apl)
            score("joint_apl_" + str(index) + "_" + encoded[:22], trial)

    # The native-paid joint APL/talent winner changes relative slot value;
    # bounded coordinate revisits keep the entire real prepared context.
    refined, refined_dps = refine_paid_gear(
        best_request, pools, score, passes=2, label_prefix="joint_gear")

    summary = {
        "source": "selected_native_get_options_and_paid_rust",
        "level": 20, "class": "ClassWarlock", "race": "RaceTroll",
        "duration_s": args.duration, "balanced_iterations": args.iterations,
        "native_option_items": len(option_data["items"]),
        "native_spell_options": len(source_spells),
        "native_executable_action_frontier": action_ids,
        "joint_apl_variants": len(paid_apls),
        "native_equipment_positions_with_options": len(pools),
        "candidate_talent_allocations": talents,
        "attempted": total_attempts, "paid": len(accepted),
        "refused": total_attempts - len(accepted),
        "top10": sorted(accepted, key=lambda x: -x["dps"])[:10],
        "best": best,
        "refusal_samples": refused, "refusal_counts": refused_reasons.most_common(20),
    }
    (args.out / "search_results.json").write_text(json.dumps(summary, indent=2) + "\n")
    print("WARLOCK_L20_NATIVE_SEARCH_RESULT", json.dumps({
        k: summary[k] for k in (
            "native_option_items", "attempted", "paid", "refused")}), flush=True)
    print("WARLOCK_L20_NATIVE_BEST", json.dumps({
        k: best[k] for k in ("label", "dps", "talents", "equipment", "apl", "actions")}), flush=True)
    print("WARLOCK_L20_NATIVE_TOP", json.dumps([
        {"label": x["label"], "dps": x["dps"]} for x in summary["top10"]
    ]), flush=True)
    if not accepted:
        raise SystemExit("No native-paid candidate")


if __name__ == "__main__":
    main()
