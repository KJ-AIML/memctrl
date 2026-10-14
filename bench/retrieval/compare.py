"""Head-to-head retrieval on identical suites: memctrl vs the approaches other memory tools use.

No network, no LLM. Every system sees the same memories and questions and is scored by the same
code. memctrl rankings come from `memctrl eval` on all-negative copies (every returned list is
reported as a failure, which exposes the full ranking). The vector threshold is tuned on the
built-in suite only and then applied unchanged to the others.

MODELS_DIR holds ONNX exports with their tokenizer.json: e5-small/onnx/model_quantized.onnx
(intfloat/multilingual-e5-small) and gemma/onnx/model_q4.onnx (EmbeddingGemma 300M). Run
bm_run.py first to include basic-memory. Needs numpy, onnxruntime and tokenizers.
usage: compare.py OUT_DIR MODELS_DIR MEMCTRL_NEW MEMCTRL_OLD
"""
import json
import re
import statistics
import subprocess
import sys
import time
from collections import defaultdict
from pathlib import Path

import numpy as np

C, MODELS = Path(sys.argv[1]).resolve(), Path(sys.argv[2])
SUITES_DIR = Path(__file__).resolve().parent / "suites"
BINS = {"memctrl (new)": sys.argv[3], "memctrl (old)": sys.argv[4]}
HOME = str(C / "home")
SUITES = ["realdev", "builtin", "thai", "thai-heldout", "en-mirror"]
K = 5
RRF_K = 60

# Words an agent would not grep for.
STOP = set("""a an the of to in on for is are was were be been do does did we i you our us how what
which where when why who whom it its this that these those with and or by at from as about can
should would will use used using into than then there their they them my me your any all""".split())
SPLIT = re.compile(r"[\s.,;:!?\"'()\[\]{}<>/\\|`~@#$%^&*+=]+")


def l2(x):
    x = np.asarray(x, dtype=np.float32)
    return x / np.maximum(np.linalg.norm(x, axis=1, keepdims=True), 1e-12)


class Onnx:
    def __init__(self, name, file, qprefix, dprefix, pooled=None):
        import onnxruntime as ort
        from tokenizers import Tokenizer

        self.tok = Tokenizer.from_file(str(MODELS / name / "tokenizer.json"))
        self.tok.enable_truncation(512)
        opts = ort.SessionOptions()
        opts.intra_op_num_threads = 4
        self.sess = ort.InferenceSession(str(MODELS / name / file), opts, providers=["CPUExecutionProvider"])
        self.inputs = {i.name for i in self.sess.get_inputs()}
        self.outputs = [o.name for o in self.sess.get_outputs()]
        self.qp, self.dp, self.pooled = qprefix, dprefix, pooled

    def _run(self, texts):
        out = []
        for t in texts:
            enc = self.tok.encode(t)
            ids = np.array([enc.ids], dtype=np.int64)
            mask = np.array([enc.attention_mask], dtype=np.int64)
            feed = {"input_ids": ids, "attention_mask": mask}
            if "token_type_ids" in self.inputs:
                feed["token_type_ids"] = np.zeros_like(ids)
            res = self.sess.run(None, feed)
            if self.pooled:
                vec = res[self.outputs.index(self.pooled)][0]
            else:
                m = mask[0][:, None].astype(np.float32)
                vec = (res[0][0] * m).sum(0) / m.sum()
            out.append(vec)
        return l2(np.stack(out))

    def query(self, texts):
        return self._run([self.qp + t for t in texts])

    def doc(self, texts):
        return self._run([self.dp + t for t in texts])


MODELS_USED = {
    "e5-small": lambda: Onnx("e5-small", "onnx/model_quantized.onnx", "query: ", "passage: "),
    "gemma": lambda: Onnx("gemma", "onnx/model_q4.onnx", "task: search result | query: ",
                          "title: none | text: ", pooled="sentence_embedding"),
}


def load(suite):
    return json.load(open(SUITES_DIR / f"{suite}.json", encoding="utf-8"))


def memctrl_lists(binary, suite, work):
    work.mkdir(parents=True, exist_ok=True)
    src = load(suite)
    neg = dict(src, cases=[dict(c, expect=[]) for c in src["cases"]])
    path = work / f"{suite}.allneg.json"
    json.dump(neg, open(path, "w", encoding="utf-8"), ensure_ascii=False)
    p = subprocess.run([binary, "eval", "--file", str(path), "--retriever", "lexical", "--top-k", "10"],
                       cwd=work, capture_output=True, text=True, env={"HOME": HOME, "PATH": "/usr/bin:/bin"})
    if p.returncode != 0:
        raise SystemExit(f"{binary} eval {suite}: {p.stderr[:300]}")
    lex = json.loads(p.stdout)["results"]["lexical"]
    got = defaultdict(list)
    for f in lex["failures"]:
        got[f["query"]] = f["got"]
    return got, lex["p50_ms"]


def grep_rank(query, mems):
    words = {w for w in SPLIT.split(query.lower()) if w and w not in STOP and len(w) > 1}
    scored = []
    for i, m in enumerate(mems):
        text = m["content"].lower()
        n = sum(1 for w in words if w in text)
        if n:
            scored.append((-n, i))
    scored.sort()
    return [mems[i]["key"] for _, i in scored]


def rrf(lists):
    score = defaultdict(float)
    for ranked in lists:
        for rank, key in enumerate(ranked):
            score[key] += 1.0 / (RRF_K + rank + 1)
    return [k for k, _ in sorted(score.items(), key=lambda kv: (-kv[1], kv[0]))]


def score(rows, mems):
    size = {m["key"]: len(m["content"]) for m in mems}
    pos = [r for r in rows if r["expect"]]
    neg = [r for r in rows if not r["expect"]]
    hit1 = sum(bool(r["got"][:1]) and r["got"][0] in r["expect"] for r in pos)
    hitk = sum(any(k in r["expect"] for k in r["got"][:K]) for r in pos)
    mrr = 0.0
    for r in pos:
        for i, k in enumerate(r["got"][:K]):
            if k in r["expect"]:
                mrr += 1 / (i + 1)
                break
    chars = [sum(size.get(k, 0) for k in r["got"][:K]) for r in rows]
    return {
        "pos": len(pos), "neg": len(neg),
        "hit@1": round(hit1 / len(pos), 3) if pos else None,
        "hit@5": round(hitk / len(pos), 3) if pos else None,
        "mrr": round(mrr / len(pos), 3) if pos else None,
        "neg_acc": round(sum(not r["got"] for r in neg) / len(neg), 3) if neg else None,
        "chars_per_query": round(statistics.mean(chars)),
    }


def best_tau(cases, keys, sims):
    """Threshold maximizing the mean of hit@5 and negative accuracy (built-in suite only)."""
    best = (-1, 0.0)
    for tau in np.unique(np.round(sims.max(axis=1), 3)):
        rows = []
        for c, s in zip(cases, sims):
            order = np.argsort(-s)[:K]
            rows.append({"expect": c["expect"], "got": [keys[i] for i in order if s[i] >= tau]})
        m = score(rows, [])
        val = ((m["hit@5"] or 0) + (m["neg_acc"] or 0)) / 2
        if val > best[0]:
            best = (val, float(tau))
    return best[1]


def main():
    (C / "out").mkdir(parents=True, exist_ok=True)
    (C / "home").mkdir(parents=True, exist_ok=True)
    report = {"suites": {}, "latency_ms": {}, "notes": []}
    lists = {name: {} for name in BINS}
    for name, binary in BINS.items():
        lat = []
        for suite in SUITES:
            got, p50 = memctrl_lists(binary, suite, C / "work" / name.replace(" ", "-"))
            lists[name][suite] = got
            lat.append(p50)
        report["latency_ms"][name] = round(statistics.median(lat), 2)

    dense = {}
    for model_name, make in MODELS_USED.items():
        model = make()
        qlat = []
        per = {}
        for suite in SUITES:
            data = load(suite)
            keys = [m["key"] for m in data["memories"]]
            D = model.doc([m["content"] for m in data["memories"]])
            Q = []
            for c in data["cases"]:
                t = time.perf_counter()
                Q.append(model.query([c["query"]])[0])
                qlat.append((time.perf_counter() - t) * 1000)
            per[suite] = (keys, np.stack(Q) @ D.T)
        tau = best_tau(load("builtin")["cases"], *per["builtin"])
        dense[model_name] = (per, tau)
        report["latency_ms"][f"vector {model_name}"] = round(statistics.median(qlat), 2)
        report["notes"].append(f"{model_name}: threshold {tau:.3f} tuned on builtin only")

    glat = []
    bm_lat = defaultdict(list)
    for suite in SUITES:
        data = load(suite)
        mems, cases = data["memories"], data["cases"]
        systems = defaultdict(list)
        # basic-memory 0.23.2 results from bm_run.py (same suites, one note per memory).
        for mode in ("text", "vector", "hybrid"):
            path = C / "out" / f"bm-{suite}-{mode}.json"
            if not path.exists():
                continue
            by_query = {r["query"]: r for r in json.load(open(path, encoding="utf-8"))}
            for c in cases:
                r = by_query[c["query"]]
                systems[f"basic-memory ({mode})"].append({"expect": c["expect"], "got": r["got"]})
                bm_lat[mode].append(r["ms"])
        for qi, c in enumerate(cases):
            base = {"expect": c["expect"]}
            for name in BINS:
                systems[name].append(dict(base, got=lists[name][suite].get(c["query"], [])))
            t = time.perf_counter()
            g = grep_rank(c["query"], mems)
            glat.append((time.perf_counter() - t) * 1000)
            systems["keyword grep"].append(dict(base, got=g))
            for model_name, (per, tau) in dense.items():
                keys, S = per[suite]
                s = S[qi]
                order = list(np.argsort(-s)[:10])
                top = [keys[i] for i in order]
                above = [keys[i] for i in order if s[i] >= tau]
                systems[f"vector {model_name} (top-5, no threshold)"].append(dict(base, got=top))
                systems[f"vector {model_name} (+ threshold)"].append(dict(base, got=above))
                lex = lists["memctrl (new)"][suite].get(c["query"], [])
                systems[f"memctrl + {model_name} fallback"].append(dict(base, got=lex or above))
                systems[f"hybrid RRF memctrl + {model_name}"].append(
                    dict(base, got=rrf([lex, above]) if (lex or above) else []))
        report["suites"][suite] = {
            "memories": len(mems), "cases": len(cases),
            "systems": {**{name: score(rows, mems) for name, rows in systems.items()},
                        # Auto memory does no retrieval: every memory is in context every session
                        # (up to its 200-line cap), so nothing is missed and nothing is ever "none".
                        "load everything (auto memory)": {
                            "pos": sum(1 for c in cases if c["expect"]), "neg": sum(1 for c in cases if not c["expect"]),
                            "hit@1": None, "hit@5": 1.0 if any(c["expect"] for c in cases) else None, "mrr": None,
                            "neg_acc": 0.0 if any(not c["expect"] for c in cases) else None,
                            "chars_per_query": sum(len(m["content"]) for m in mems)}},
            "all_chars": sum(len(m["content"]) for m in mems),
        }
    report["latency_ms"]["keyword grep"] = round(statistics.median(glat), 3)
    for mode, xs in bm_lat.items():
        report["latency_ms"][f"basic-memory {mode} (CLI process)"] = round(statistics.median(xs), 1)
    json.dump(report, open(C / "out" / "compare.json", "w"), ensure_ascii=False, indent=1)

    for suite, r in report["suites"].items():
        print(f"\n## {suite}: {r['memories']} memories, {r['cases']} questions, all memories = {r['all_chars']} chars")
        print(f"{'system':44} {'hit@1':>6} {'hit@5':>6} {'mrr':>6} {'neg_acc':>7} {'chars/q':>8}")
        for name, m in r["systems"].items():
            print(f"{name:44} {str(m['hit@1']):>6} {str(m['hit@5']):>6} {str(m['mrr']):>6} "
                  f"{str(m['neg_acc']):>7} {m['chars_per_query']:>8}")
    print("\nlatency p50 (ms, in-process per query):", report["latency_ms"])
    print("\n".join(report["notes"]))


if __name__ == "__main__":
    main()
