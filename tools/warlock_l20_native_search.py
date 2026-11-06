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
CASTS = (
    ("shadow_bolt", 1088), ("corruption", 6222),
    ("immolate", 1094), ("agony", 1014),
    ("drain_life", 689), ("searing_pain", 5676),
    ("life_tap", 1455),
)


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


def cast_apl(spell_id):
    return [{"action": {"castSpell": {"spellId": {"spellId": spell_id}}}}]


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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--request", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--duration", type=int, default=30)
    parser.add_argument("--iterations", type=int, default=8)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    base = json.loads(args.request.read_text())
    player(base)["equipment"] = {"items": [{"id": 0} for _ in range(17)]}
    base["simOptions"].update({
        "iterations": args.iterations, "randomSeed": "4242", "debugFirstIteration": False,
    })
    base["encounter"]["duration"] = args.duration

    option_data = invoke([str(args.binary), "options", "--class", "warlock", "--level", "20"])
    if (option_data["class"] != "warlock" or option_data["level"] != 20
            or option_data["talents"]["points_available"] != 11):
        raise RuntimeError("Rust returned wrong level/class source")

    pools = ranked_items(option_data)
    talents = talent_candidates(option_data)
    total_attempts = 0
    accepted = []
    refused = []
    refused_reasons = Counter()
    seen = set()
    best = None

    def score(label, request):
        nonlocal total_attempts, best
        key = json.dumps(request, sort_keys=True)
        if key in seen:
            return None
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
            if best is None or row["dps"] > best["dps"]:
                best = row
                (args.out / "best_request.json").write_text(json.dumps(request, indent=2) + "\n")
                (args.out / "best_native_output.json").write_text(json.dumps(paid, indent=2) + "\n")
            print("PAID", label, "dps", round(row["dps"], 4),
                  "best", round(best["dps"], 4), flush=True)
            return row
        except (RuntimeError, ValueError, KeyError, subprocess.TimeoutExpired) as exc:
            reason = str(exc)[-320:]
            refused_reasons[reason] += 1
            if len(refused) < 40:
                refused.append({"label": label, "reason": reason})
            print("REFUSED", label, reason[-130:], flush=True)
            return None

    # First measure actual registered spell actions, with all other axes fixed.
    baseline = copy.deepcopy(base)
    player(baseline)["rotation"]["priorityList"] = cast_apl(1088)
    score("baseline_shadow_bolt", baseline)
    for label, sid in CASTS:
        proposal = copy.deepcopy(base)
        player(proposal)["rotation"]["priorityList"] = cast_apl(sid)
        score("rotation_" + label, proposal)

    # Observe native equipment effects. Every candidate still goes to Rust.
    # Keep the highest-paid candidate rather than trusting item-level ranking.
    if best is None:
        raise SystemExit("No executable level-20 Warlock candidate; see refusals")
    working = copy.deepcopy(base)
    player(working)["rotation"]["priorityList"] = copy.deepcopy(best["apl"])
    for index in sorted(pools):
        current = None
        for iid in pools[index][:3]:
            trial = copy.deepcopy(working)
            player(trial)["equipment"]["items"][index] = {"id": iid}
            row = score("gear_slot_" + str(index) + "_item_" + str(iid), trial)
            if row and (current is None or row["dps"] > current[0]):
                current = (row["dps"], iid)
        if current:
            player(working)["equipment"]["items"][index] = {"id": current[1]}

    # Native-sourced talent tree candidates at the actual 11-point L20 budget.
    for label, encoded in talents:
        trial = copy.deepcopy(working)
        player(trial)["talentsString"] = encoded
        score("talent_" + label, trial)

    # Joint re-scoring of real gear and talent builds with each native APL action.
    # Score each full request, not a sum of Python item/talent/spell weights.
    paid_talents = list(dict.fromkeys(
        row["talents"] for row in sorted(accepted, key=lambda x: -x["dps"])
        if row["talents"] is not None))[:3]
    for encoded in paid_talents:
        for label, sid in CASTS:
            trial = copy.deepcopy(working)
            player(trial)["talentsString"] = encoded
            player(trial)["rotation"]["priorityList"] = cast_apl(sid)
            score("joint_" + label + "_" + encoded[:22], trial)

    summary = {
        "source": "selected_native_get_options_and_paid_rust",
        "level": 20, "class": "ClassWarlock", "race": "RaceTroll",
        "duration_s": args.duration, "balanced_iterations": args.iterations,
        "native_option_items": len(option_data["items"]),
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
        k: best[k] for k in ("label", "dps", "talents", "equipment", "actions")}), flush=True)
    print("WARLOCK_L20_NATIVE_TOP", json.dumps([
        {"label": x["label"], "dps": x["dps"]} for x in summary["top10"]
    ]), flush=True)
    if not accepted:
        raise SystemExit("No native-paid candidate")


if __name__ == "__main__":
    main()
