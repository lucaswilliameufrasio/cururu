"""Aggregate matcher decisions with golden tp/fp labels into recall and match precision."""
import json, sys, pathlib
rb, run = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
tot = dict(prs=0, cand=0, gold_tp=0, gold_tp_found=0, matched_tp=0, matched_fp=0, novel=0)
rows = []
for f in sorted((run / "judge-outputs").glob("*.json")):
    key = f.stem
    gold = json.loads((rb / "golden" / f"{key}.json").read_text())["findings"]
    labels = [g["tp_fp"] for g in gold]
    dec = json.loads(f.read_text())["candidateFindings"]
    covered = set(); mtp = mfp = nov = 0
    for d in dec:
        idx = [i for i in d["matchedGoldenIndices"] if 0 <= i < len(gold)]
        covered.update(idx)
        if not idx: nov += 1
        elif any(labels[i] == "tp" for i in idx): mtp += 1
        else: mfp += 1
    gtp = sum(1 for l in labels if l == "tp"); found = sum(1 for i in covered if labels[i] == "tp")
    rows.append((key, len(dec), gtp, found, mtp, mfp, nov))
    for k, v in zip(["cand", "gold_tp", "gold_tp_found", "matched_tp", "matched_fp", "novel"], [len(dec), gtp, found, mtp, mfp, nov]): tot[k] += v
    tot["prs"] += 1
print("pr | candidates | golden TPs | TPs found | matched TP | matched FP | novel")
for r in rows: print(" | ".join(map(str, r)))
m = tot["matched_tp"] + tot["matched_fp"]
print(json.dumps(tot), "recall=%.1f%%" % (100 * tot["gold_tp_found"] / max(tot["gold_tp"], 1)), "matched_precision=%.1f%%" % (100 * tot["matched_tp"] / max(m, 1)))
