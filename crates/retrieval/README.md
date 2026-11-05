# onyx-retrieval

In-process tool, skill, and fact retrieval for the Onyx core. Register catalog
metadata once, then rank it per agent turn. Tools and skills are *pulled* on
relevance; facts are constant grounding content the higher layers *push* into
the context, always-on or retrieval-gated. BM25 is the model-free default;
semantic and hybrid retrieval use either an in-process Candle model or a
configured OpenAI-compatible embedding endpoint. The engine and its cache stay
in-process — no vector database and no service to deploy.

## Why this exists in the core

Onyx ships a growing tool surface (native tools, MCP servers, installed skills).
Sending the whole catalog every turn is what makes a capable CLI feel slow and
forgetful: the model spends its budget reading tool definitions instead of
working. This crate ranks the catalog against the current intent and hands the
model only the slice that matters, while keeping a stable prefix so provider
prompt caches survive across turns.

## Layout

| Path | What lives there |
| --- | --- |
| `src/tool_registry.rs` | Tool catalog: register/remove, search with a selectable method, adaptive ranking state |
| `src/skill_registry.rs` | Skill catalog, same search surface over skill metadata |
| `src/fact_registry.rs` | Fact catalog with pin modes (always-on vs retrieval-gated grounding) |
| `src/search.rs`, `src/method.rs` | BM25 scoring and the `SearchMethod` selection (`bm25` / `dense` / `hybrid`) |
| `src/dense_search.rs`, `src/embedding*.rs` | In-process + endpoint embeddings, artifact digests, warm cache |
| `src/fusion.rs` | Hybrid fusion weights between lexical and semantic arms |
| `src/usage.rs`, `src/usage_learner.rs` | Intent graph and the usage-adaptive ranking arm learned from real invocations |
| `src/rerank.rs` | Stage-one/stage-two reranking outcome types |
| `src/trace/` | Local append-only trace stream feeding the usage learner |
| `src/harness.rs` | `#[ignore]`d measurement harness and fixture regeneration |

## Tests

```bash
cargo test -p onyx-retrieval
```

`tests/tool_search.rs`, `tests/trace.rs` and `tests/usage_ranking.rs` are the
integration contracts; `src/harness.rs` entries are ignored by default because
they download models or rewrite fixtures.

## Embedding models

Dense retrieval needs a tokenizer plus model weights. The built-in default is
auto-downloaded on first use into the shared Hugging Face cache; anything else
must be pre-downloaded, passed with `download = true`, or served by an
OpenAI-compatible endpoint. Set `ONYX_EMBED_SLOW_MS` to change the threshold at
which a load is reported as slow.
