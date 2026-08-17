# Generation Pipeline

Zoom into `pipeline::generate_artifacts` (`core/src/pipeline/generate.rs`). This is where
RAG meets generation: it exhausts the material by producing one unit per HDBSCAN
topic cluster, generates grammar-constrained JSON per unit, then assembles and
persists artifacts. The LLM is `SmolLM2-360M-Instruct` (Q8_0, 8K context). It
runs as part of the automatic pipeline job for each of the five artifact types.

```mermaid
flowchart TB
    START(["generate_artifacts(worksheet_id, type, on_progress)"])
    RESET["emit pipeline-progress {phase:generating, types_done, types_total}"]
    QCH["SELECT chunks (position, text, cluster_index) ORDER BY position"]
    CHK{chunks empty?}
    UNITS{"any chunk has cluster_index >= 0?"}
    CTX1["cluster_contexts(labels, positions, MAX_CONTEXT_CHUNKS=10)<br/>one unit per topic cluster, noise skipped, by source position"]
    CTX2["fallback: single unit = pick_evenly(all, 10)"]
    PREP["build contexts + unit_sources (chunk ids per unit)<br/>emit pipeline-progress {done:0, total: units}"]

    START --> RESET --> QCH --> CHK
    CHK -- "yes" --> ERR["return 'No chunks found'"]
    CHK -- "no" --> UNITS
    UNITS -- "yes" --> CTX1
    UNITS -- "no (legacy)" --> CTX2
    CTX1 --> PREP
    CTX2 --> PREP

    subgraph LOOP["per unit (best-effort)"]
        direction TB
        SEED["params.seed + unit_index (batch never repeats)"]
        PROMPT["apply_chat_template(system_prompt_for, user_message_for)"]
        GEN["generator.generate(prompt, schema, params)<br/>GBNF grammar + sampler chain"]
        PARSE{"output_parses?<br/>valid JSON + item array"}
        RETRY["retry: max_tokens doubled (cap 4096)"]
        OK["push output + source"]
        FAIL["record failure, skip unit"]
        PROGE["emit pipeline-progress {done: index+1}"]

        SEED --> PROMPT --> GEN --> PARSE
        PARSE -- "no" --> RETRY --> GEN
        PARSE -- "yes" --> OK
        RETRY -. "still invalid" .-> FAIL
        FAIL --> PROGE
        OK --> PROGE
    end

    PREP --> SEED

    FAILED{any output at all?}
    ASSEMBLE["assemble_artifacts(type, outputs, unit_sources)"]
    PERSIST["for each artifact: INSERT artifacts (id, type, source, content)"]
    DONE(["return artifacts[]"])

    PROGE --> FAILED
    FAILED -- "no" --> ALLFAIL["return 'all units failed'"]
    FAILED -- "yes" --> ASSEMBLE --> PERSIST --> DONE

    ASSEMBLE --> SPLIT
    subgraph ASSEMBLE_DETAIL["assemble_artifacts branching"]
        direction LR
        SPLIT{"items_field_for(type)?"}
        SPLIT -- "question types (MCQ/Essay/Completion)" --> ITEM["one artifact per item<br/>from questions[] / items[]"]
        SPLIT -- "Summary / MindMap" --> MERGE["merge per-cluster sections<br/>into one worksheet-wide artifact"]
    end
```

## The moving parts

1. **Fetch** — all chunks are read with their optional `cluster_index`. The
   auto-embed step runs before generation in `process_files`, so every chunk
   already has a vector.

2. **Unit selection** — `cluster_contexts` (`pipeline/cluster.rs`) groups chunks
   by cluster, orders clusters by the source position of their earliest chunk,
   and samples up to `MAX_CONTEXT_CHUNKS=10` members per cluster evenly by
   position. HDBSCAN noise (`cluster_index = -1`) is skipped. Legacy worksheets
   with no clusters fall back to a single unit spread evenly across the whole
   material.

3. **Prompt building** — `pipeline/generate.rs` provides a shared system prompt
   (`system_prompt_for`) and a per-type task prompt (`user_message_for`) that
   embeds the retrieved passages. The model's built-in chat template wraps the
   system + user messages (`apply_chat_template`).

4. **Grammar-constrained generation** — each unit's JSON schema (`schema_for`)
   is compiled into a GBNF grammar via `json_schema_to_grammar` and sampled
   under a chain `[grammar?, temp?, top_p, dist(seed)]`. The model can only emit
   schema-valid JSON. Sampling uses the apply-sampler path
   (`LlamaTokenDataArray::apply_sampler`) to avoid a known `sampler.sample`
   crash in llama-cpp-2.

5. **Best-effort retry** — if a unit's output truncates or fails to parse
   (`output_parses`), it is retried once with a doubled (capped 4096) token
   budget, then skipped so one bad cluster never discards an otherwise healthy
   batch.

6. **Artifact assembly** — `assemble_artifacts` branches on artifact type:
   - **Question types** (`MultipleChoiceQuiz`, `EssayQuiz`, `CompletionQuiz`):
     each unit's item array (`questions[]` / `items[]`) is split so every item
     becomes its own persisted artifact.
   - **Summary / MindMap**: every unit's section is merged into a single
     worksheet-wide artifact, ordered by source position, with a shared
     title/topic.

7. **Persist** — each artifact (new ULID id, type, comma-joined source chunk
   ids, JSON content) is inserted into the `artifacts` table and returned to the
   frontend. `pipeline-progress` is emitted once per unit plus a per-type reset
   at the start of each artifact type.

## Sampling parameters (`GenerationParams`, defaults)

| Param | Default | Notes |
|-------|---------|-------|
| `temperature` | 0.7 | skipped from sampler chain when `<= 0` |
| `top_p` | 0.9 | |
| `max_tokens` | 1024 | capped by `N_CTX(8192) - prompt_tokens - 1` |
| `seed` | 1234 | + unit index per unit to avoid batch repetition |

Context window `N_CTX = 8192`; max prompt `MAX_PROMPT_TOKENS = 6144`.
