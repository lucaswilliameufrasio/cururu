"""Build one judge input per PR: candidate findings, labeled golden findings, matcher prompt."""
import json, re, sys, pathlib
rb, run = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
prompt = re.search(r"MATCHER_SYSTEM_PROMPT = `(.*?)`;", (rb / "scripts/eval/matcher.ts").read_text(), re.S).group(1)
out = run / "judge-inputs"; out.mkdir(exist_ok=True)
for f in sorted((run / "findings").glob("*.json")):
    key = f.stem
    cand = json.loads(f.read_text())["findings"]
    gold = json.loads((rb / "golden" / f"{key}.json").read_text())["findings"]
    doc = {
        "pr_key": key,
        "instructions": prompt,
        "candidate_findings": [{"index": i, "file": c["file"], "start_line": c["start_line"], "end_line": c["end_line"], "message": c["message"]} for i, c in enumerate(cand)],
        "golden_findings": [{"index": i, "file": g["file"], "start_line": g["start_line"], "end_line": g["end_line"], "message": g["message"]} for i, g in enumerate(gold)],
    }
    (out / f"{key}.json").write_text(json.dumps(doc, indent=1))
    print(key, len(cand), "candidates", len(gold), "golden")
