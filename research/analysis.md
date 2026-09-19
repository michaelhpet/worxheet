# Worxheet pipeline evaluation — log audit + edition history

**Date:** 2026-09-19 · **Source:** `core/logs/` (70 files × 4 stages) + git history (`main`, 68 commits)
**Scope:** what the logs can prove (measured) vs. what history implies (estimated). No benchmarks were
recorded for editions A–E, so cross-edition deltas are analytical, derived from code archaeology, and
labelled as such. Do not present estimates as measured A/B results.

## 1. Data inventory (measured)

| Stage | Files | What each record holds |
|---|---|---|
| `file_reads/` | 140 (70 `parse_started` + 70 completed) | extension, path, block counts by kind, `duration_ms` |
| `tokenization/` | 70 | `tokens`, tokenizer `calls`, `duration_ms` |
| `segmentation/` | 70 | `count` (segments), `total_tokens` |
| `generation/` | 1023 (406 requests + 617 responses) | request: system/user prompt, schema, seed, temp; response: `status`, `verdict`, `elapsed_ms`, `error`/`raw` |

4 distinct worksheets, 70 distinct files, all `pdf`. Raw tables: `metrics_ingestion.csv`,
`metrics_tokenization.csv`, `metrics_segmentation.csv`, `metrics_generation.csv`; aggregates in
`metrics_summary.{json,csv}`. Reproduce with `python3 /tmp/opencode/build_metrics.py` (stdlib only).

## 2. Headline KPIs (measured, current edition F)

**Ingestion (n=70 PDFs):** parse median **650 ms**, mean 2064 ms, p90 6930 ms, max 15.4 s
(total 144.5 s). Output: 9128 body + 2985 heading blocks.
**Segmentation:** 698 segments (mean 10.0/file, median 3.0 — four files contribute 114–132 each);
mean **479 tokens/segment**, median 411. Tokenizer: 291,352 tokens in 9128 calls, mean **98 ms/file**.
**Generation (617 responses):** transport success **47.2%** (291/617); of transported, accepted
**61.2%** (178/291: 89 MCQ + 89 Essay), rejected 34.0% (99), skipped 4.8% (14). End-to-end yield:
**28.9%** of responses become artifacts; 600 questions from 178 units (**3.37/unit**).
Latency: accepted-path median **10.1 s** (p90 31.6 s) vs error-path median **1.0 s**.
Retries: 419 attempt-0 / 101 attempt-1 / 97 attempt-2 responses.

Charts: `charts/01_outcome_distribution.svg` … `07_edition_requests.svg`.

## 3. Findings that matter for the thesis

1. **CompletionQuiz yields 0%.** All 95 transported replies carry `{"questions": []}` but the
   validator requires an `"items"` array → blanket `missing "items" array` rejection. Schema/task
   mismatch, not model failure. Fix: align prompt/example/schema on one field name, add a
   contract test. (MCQ/Essay acceptance is 92.7% of transported — the pipeline itself works.)
2. **Summary/MindMap yield 0/4 transported.** 2 rejections each (`expected … documented fields` /
   truncated JSON). n is tiny (one request per worksheet per type), but both failure modes point at
   max-token truncation and shape drift — worth a dedicated experiment with larger budgets.
3. **All 326 transport errors are operational.** 401 missing key (224), 402 model-not-in-free-tier
   (79), 429 rate-limited (20), network (3). None reflect generation quality; the test window simply
   lacked valid credentials/quota. Report transport success separately from validation acceptance.
4. **Empty-but-valid replies are handled, not hidden.** 14 `skipped` verdicts (`{"questions": []}`
   on front-matter/degenerate slices) show the grounding filters working as designed.
5. **Latency is provider-bound.** 10 s median per accepted unit × 92 units/worksheet ⇒ wall-clock is
   dominated by the cloud backend and concurrency cap, not local stages (parse + tokenize ≈ 0.75 s/file).

## 4. Edition history → approximate gains (estimated, not measured)

| Edition | Change | Approx. gain (analytical) |
|---|---|---|
| A → B (parallel pipeline, `475a455`) | 4-way embed + generation workers | ~4× throughput on multicore |
| B → C (cloud provider, `74eb556`) | delete llama-cpp local inference (~400MB models) | removes model download + local inference; quality unlocked via larger models; adds key/quota/network dependency |
| C → D (no-embed, `ee63151`) | delete embedder + `ort`; drop drift-splitting | eliminates O(sentences) embedding calls + BLOB storage; segmentation now ~98 ms/file |
| D → E (liteparse + robustness, `701bd3f` et al.) | OCR gated to ≥50% text-less pages; dedup; resumable; independent pipelines | born-digital PDFs skip OCR; re-runs ≈ 0 cost for unchanged inputs |
| E → F (fine-tuned gen, `dff1bb0` + `3cc4947`) | whole-material Summary/MindMap (8000-token cover) | merged requests 60 → 2 per 30-seg worksheet (−96.7% merged, −38.7% total) |

Detail: `edition_comparison.csv`. The user-described "first edition" (embed → HDBSCAN-cluster →
per-cluster quiz via embedded model) corresponds to edition A (`ce22c31`: `bge-small-en-v1.5` 384-dim
+ HDBSCAN `n/200` + `SmolLM2-360M-Instruct`, `MAX_CONTEXT_CHUNKS 10`).

## 5. Threats to validity

- Logs cover one day (2026-09-19), one corpus (CSC-404 ML slides, 4 worksheets), PDF-only.
- No timing logs exist for editions A–E: all cross-edition deltas are reasoned, not measured.
- Generation window is polluted by auth/quota failures; acceptance rates quoted both ways (§2).
- Question counts measure yield, not pedagogical quality (no HOTS/Bloom rubric scoring was logged).

## 6. Recommended next experiments (for the thesis)

1. Fix the CompletionQuiz contract, re-run, report yield delta (expect ≈ MCQ/Essay levels).
2. Score a sampled artifact set against a Bloom/HOTS rubric (Apply/Analyze/Evaluate counts) + grounding spot-checks.
3. Re-run the corpus with valid quota and report clean transport success + end-to-end latency per worksheet.
4. Ablate the front-matter/degenerate filters (items before/after) to quantify precision gain.
