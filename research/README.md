# Research — pipeline evaluation metrics

Evaluation evidence for the final-year project, built from `core/logs/` (excluded from git by
`core/.gitignore` — re-run the pipeline to regenerate) plus git-history archaeology.

| File | Contents |
|---|---|
| `analysis.md` | full write-up: KPIs, findings, edition gains, threats, next steps |
| `metrics_ingestion.csv` | per-file parse durations + block counts (n=70) |
| `metrics_tokenization.csv` | per-file tokens, calls, throughput (n=70) |
| `metrics_segmentation.csv` | per-file segment counts + tokens/segment (n=70) |
| `metrics_generation.csv` | per-response status/verdict/latency/error-class/items (n=617) |
| `metrics_summary.{json,csv}` | aggregate KPIs |
| `edition_comparison.csv` | editions A–F with measured-vs-estimated basis |
| `charts/*.svg` | 01 outcome split · 02 verdicts by type · 03 error taxonomy · 04 latency · 05 parse hist · 06 segments hist · 07 request reduction |

Regenerate: `python3 /tmp/opencode/build_metrics.py` (CSVs + summary) and
`python3 /tmp/opencode/build_charts.py` (SVGs). Stdlib only.
