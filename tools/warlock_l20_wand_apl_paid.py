#!/usr/bin/env python3
"""Actually pay balanced Rust combat for equipped L20 Warlock wand/APL variants.

The existing committed Rust-paid 17-slot winner supplies the starting build.
The engine's prepared player spell list supplies Shoot's action identity.
Python only edits APL proposal ordering and requests native prepare/sim; it
never substitutes game mechanics, candidate legality, or fabricated damage.
"""
import copy
import json
import math
import subprocess
import sys
from pathlib import Path


def execute(args, *, timeout=60):
    p = subprocess.run([str(x) for x in args], text=True, capture_output=True, timeout=timeout)
    if p.returncode:
        raise RuntimeError("exit=" + str(p.returncode) + ": " + (p.stderr or p.stdout)[-600:])
    return json.loads(p.stdout) if p.stdout.strip() else None


def actor(request):
    return request["raid"]["parties"][0]["players"][0]


def score(binary, request, root, label):
    req_file = root / (label + "-request.json")
    prep_file = root / (label + "-prepared.json")
    paid_file = root / (label + "-paid.json")
    req_file.write_text(json.dumps(request, indent=2, sort_keys=True) + "\n")
    try:
        execute([binary, "prepare", "--request", req_file, "--actor-level", "20",
                 "--outfile", prep_file])
        prepared = json.loads(prep_file.read_text())
        assert prepared["player"]["level"] == 20
        assert prepared["player"]["class"] == "ClassWarlock"
        assert prepared["player"]["race"] == "RaceTroll"
        assert prepared["melee"]["auto_swing_ranged"] is True
        assert prepared["melee"]["ranged"]["swing_speed"] > 0
        execute([binary, "sim", "--infile", prep_file, "--rng-mode", "balanced",
                 "--outfile", paid_file])
        raw = json.loads(paid_file.read_text())
        body = raw["result"]
        m = body["raidMetrics"]["parties"][0]["players"][0]
        dps = m["dps"]["avg"]
        assert isinstance(dps, (int, float)) and math.isfinite(dps) and dps > 0
        actions = []
        for action in m.get("actions") or []:
            targets = action.get("targets") or []
            actions.append({
                "id": action.get("id"),
                "casts": sum(t.get("casts", 0) or 0 for t in targets),
                "damage": sum(t.get("damage", 0) or 0 for t in targets),
            })
        shoot = [a for a in actions
                 if (a["id"] or {}).get("otherId") == "OtherActionShoot"]
        out = {
            "label": label, "status": "PAID",
            "native_dps": dps, "iterations": body["iterationsDone"],
            "duration_s": body["firstIterationDuration"],
            "shoot_casts": sum(a["casts"] for a in shoot),
            "shoot_damage": sum(a["damage"] for a in shoot),
            "actions": [a for a in actions if a["damage"] > 0 or a["casts"] > 0],
            "request_sha256": raw.get("request_sha256"),
            "source_revision": raw.get("source_revision"),
            "apl": actor(request)["rotation"]["priorityList"],
            "equipment": actor(request)["equipment"]["items"],
            "talents": actor(request)["talentsString"],
        }
        print("WARLOCK_L20_NATIVE_APL_PAID", json.dumps({k: out[k] for k in (
            "label", "native_dps", "shoot_damage", "shoot_casts", "iterations",
            "duration_s", "actions")}), flush=True)
        return out
    except (RuntimeError, AssertionError, KeyError, ValueError, subprocess.TimeoutExpired) as err:
        out = {"label": label, "status": "RUST_REFUSED", "error": str(err)[-1000:]}
        print("WARLOCK_L20_NATIVE_APL_REFUSED", json.dumps(out), flush=True)
        return out


def main():
    import argparse
    p = argparse.ArgumentParser()
    p.add_argument("--binary", type=Path, required=True)
    p.add_argument("--request", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--iterations", type=int, default=8)
    p.add_argument("--duration", type=int, default=30)
    args = p.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    binary = args.binary.resolve()
    base = json.loads(args.request.read_text())
    base["simOptions"]["iterations"] = args.iterations
    base["simOptions"]["debugFirstIteration"] = False
    base["encounter"]["duration"] = args.duration
    a = actor(base)
    assert a["class"] == "ClassWarlock" and a["race"] == "RaceTroll"
    assert len(a["equipment"]["items"]) == 17
    dot_maintenance = copy.deepcopy(a["rotation"]["priorityList"][:2])
    native_filler = copy.deepcopy(a["rotation"]["priorityList"][2])
    # Prepared native Rust source, not a homegrown spell allowlist or action registry.
    discovery = args.out / "discover-prepared.json"
    req = args.out / "discover-request.json"
    req.write_text(json.dumps(base))
    execute([binary, "prepare", "--request", req, "--actor-level", "20",
             "--outfile", discovery])
    prepared = json.loads(discovery.read_text())
    native_shoot_ids = [
        (spell.get("action_id") or {})
        for spell in prepared["player"]["spells"]
        if (spell.get("action_id") or {}).get("other_id") == "OtherActionShoot"
    ]
    assert len(native_shoot_ids) == 1, "Rust did not register Shoot on equipped L20 Warlock wand"
    assert prepared["melee"]["auto_swing_ranged"] is True, "Native Shoot swing not enabled"
    shoot_action = {"action": {"castSpell": {
        "spellId": {"otherId": "OtherActionShoot"}
    }}}
    apls = {
        "dots_shadow_bolt": dot_maintenance + [native_filler],
        "dots_shoot": dot_maintenance + [shoot_action],
        "dots_passive_wand": dot_maintenance,
        "shoot_only": [shoot_action],
        "dots_shadow_bolt_shoot_fallback":
            dot_maintenance + [native_filler, shoot_action],
        "shadow_bolt_only": [native_filler],
    }
    results = []
    for label, apl in apls.items():
        request = copy.deepcopy(base)
        actor(request)["rotation"]["priorityList"] = apl
        results.append(score(binary, request, args.out, label))
    success = [r for r in results if r["status"] == "PAID"]
    summary = {
        "goal": "native_equipped_warlock_l20_apl_shoot_comparison",
        "engine": "mythicsim-forever-engine",
        "actor_level": 20, "rng_mode": "balanced",
        "compared": len(results), "paid": len(success),
        "results": results,
        "winner": max(success, key=lambda x: x["native_dps"]) if success else None,
    }
    (args.out / "comparison.json").write_text(json.dumps(summary, indent=2) + "\n")
    print("WARLOCK_L20_SHOOT_APL_COMPARISON",
          json.dumps({"paid": len(success), "winner": (
              {"label": summary["winner"]["label"],
               "dps": summary["winner"]["native_dps"],
               "shoot_damage": summary["winner"]["shoot_damage"]}
              if summary["winner"] else None)}), flush=True)
    if not success:
        raise SystemExit("No real balanced Rust sim scored an equipped L20 Warlock")


if __name__ == "__main__":
    main()
