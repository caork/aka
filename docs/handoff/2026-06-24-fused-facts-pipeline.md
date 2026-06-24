# Fused Facts Pipeline Handoff

Date: 2026-06-24

Branch: `codex/fused-facts-pipeline`

Goal: remove the `artifact/` / sidecar / engine SQLite path from the hot indexing architecture, fuse native AKA engine facts directly into Rust indexing, and make semantic producers reusable for SCIP / stack-graphs / LSP ecosystems.

## Current State

The work is partially complete and intentionally not marked done. The native engine now has a real embedded facts API and can emit facts through callbacks without writing its SQLite DB, but Rust has not yet switched runtime indexing to the embedded path.

Main repo latest relevant commits:

- `26e3b67 infra: pin embedded facts engine`
- `9019e84 feat: add semantic fact producer seam`
- `b326c41 infra: pin fact sink engine`
- Previous groundwork on this branch: `514ef95`, `e473de7`, `e162e9f`, `84dad4d`, `a44fce3`, `fe40ec2`, `ce1f8b4`, `4967063`, `b4f86f2`, `3b33227`

Nested engine repo `engine/aka-engine-src` latest relevant commits:

- `f77d34f feat: add embedded facts api`
- `1d9c4f6 refactor: emit facts through sink`
- `f96acdd feat: emit direct facts sidecar`

Pinned engine ref:

- `engine/ENGINE_SHA`: `f77d34f853ca1252cde573ff4e49443f95d7efed`
- `Dockerfile` and `.github/workflows/release.yml` are pinned to the same SHA.

## What Is Done

### Engine callback sink

Engine C now has `cbm_fact_sink_t` in `src/pipeline/pipeline.h`.

The pipeline emits facts after predump passes and before SQLite persistence. JSONL sidecar output is now implemented as a built-in sink, so sidecar is a transport/debug sink rather than the fact-emission architecture.

Important files:

- `engine/aka-engine-src/src/pipeline/pipeline.h`
- `engine/aka-engine-src/src/pipeline/pipeline.c`

### Engine embedded API

Engine now has a public embedded API in:

- `engine/aka-engine-src/src/api/aka_engine.h`
- `engine/aka-engine-src/src/api/aka_engine.c`

The public API exposes:

- `aka_engine_index_options_t`
- `aka_engine_fact_sink_t`
- `aka_engine_index_with_sink(...)`

`direct_facts_only = true` sets `cbm_pipeline_set_skip_dump`, which skips engine SQLite discovery/reuse and final SQLite dump/persistence. This is the key proof that the native engine can emit facts without the artifact/SQLite hot path.

`Makefile.cbm` now has:

- `make -f Makefile.cbm cbm`
- `make -f Makefile.cbm libaka-engine`

The static library target builds `build/c/libaka_engine.a`. It intentionally uses a non-override mimalloc object (`MI_OVERRIDE=0`) so linking it into Rust/Tauri is less risky than linking the production binary allocator override.

### Semantic producer seam

`aka-facts` now has a public semantic producer seam:

- `crates/aka-facts/src/producer.rs`

It defines:

- `ProducerContext`
- `ProducerCapability`
- `SemanticFactSink`
- `SemanticFactProducer`
- `SemanticFactBundleBuilder`
- `produce_semantic_batch`
- `produce_semantic_into`
- `replay_semantic_bundle_into`

`crates/aka-core/src/lib.rs` re-exports the semantic records and producer traits so future SCIP / stack-graphs / LSP adapters can write facts without depending on private engine internals.

This does not change graph/search writer behavior. The writer still consumes `FactSource`.

## Verification Already Run

Engine build:

```bash
make -C engine/aka-engine-src -f Makefile.cbm cbm
make -C engine/aka-engine-src -f Makefile.cbm libaka-engine
```

Engine JSONL sidecar smoke:

- Built tiny `/tmp` Rust repo.
- Ran `aka-engine cli --json index_repository` with `facts_output_path`.
- Parsed `facts.jsonl`.
- Verified `manifest`, node records, edge records, and `done`.

Embedded C API smoke:

- Compiled a temporary C program linking `engine/aka-engine-src/build/c/libaka_engine.a`.
- Called `aka_engine_index_with_sink` with `direct_facts_only = true`.
- Verified callback counts: one manifest, nodes > 0, edges > 0, one done.
- Verified no `*.db` file was produced in the engine cache directory.

Rust facts tests:

```bash
cargo test -p aka-facts
cargo fmt --check
```

## What Is Not Done Yet

Rust runtime still defaults to `SidecarEngineFactProducer`.

Current hot path in Rust:

- `EngineRunner::analyze_facts`
- `crates/aka-core/src/engine/fact_producer.rs`
- sidecar JSONL direct facts, then DB fallback if sidecar missing

The branch is therefore not complete against the original goal. The engine can now do direct embedded facts, but Rust has not yet linked or selected that path.

## Recommended Next Steps

### 1. Add Rust embedded-engine feature and link step

Add to `crates/aka-core/Cargo.toml`:

- feature `embedded-engine`
- probably no default enablement at first

Add `crates/aka-core/build.rs`:

- Only run when `embedded-engine` is enabled.
- Link `engine/aka-engine-src/build/c/libaka_engine.a` or copied `engine/lib/...` output.
- Add native search paths and link libs: `stdc++`, `pthread`, `z`, `m` as needed on Unix.
- Keep Windows on binary fallback for now unless MSVC-compatible engine library is solved.

Do not use bindgen initially. The C ABI is small and stable enough for handwritten `extern "C"` bindings.

### 2. Add Rust FFI wrapper

Suggested file:

- `crates/aka-core/src/engine/embedded.rs`

Implement handwritten `#[repr(C)]` equivalents of:

- `aka_engine_mode_t`
- `aka_engine_index_options_t`
- `aka_engine_fact_sink_t`
- `aka_engine_index_with_sink`

Callback rules:

- Never unwind across C. Use `catch_unwind` or store errors in userdata and return nonzero.
- Build node IDs as `cbm:{cbm_id}:{qualified_name}`.
- Build edge IDs as `cbm-edge:{edge_id}`.
- Parse `properties_json` / `evidence_json` with `serde_json`.
- Write into `dyn FactSink<Error = FactSourceError>`.
- Reuse existing normalization/chunk synthesis helpers where possible.

### 3. Reshape engine producer interface

Current `EngineFactProducer` is still `prepare -> run_engine_index -> finish`, which fits sidecar but not embedded.

Recommended shape:

```rust
trait EngineFactProducer {
    fn produce(
        &self,
        runner: &EngineRunner,
        repo: &Path,
        options: AnalyzeFactsOptions<'_>,
        sink: &mut dyn FactSink<Error = FactSourceError>,
        on_event: &mut dyn FnMut(&EngineEvent),
    ) -> Result<ProducedEngineFacts, EngineError>;
}
```

Then:

- `SidecarEngineFactProducer::produce` keeps current behavior.
- `EmbeddedEngineFactProducer::produce` calls `aka_engine_index_with_sink`.
- `EngineRunner::analyze_facts` chooses embedded only when explicitly enabled.

Suggested selection:

- `embedded-engine` cargo feature required.
- `AKA_ENGINE_EMBEDDED=1` opts in.
- `AKA_ENGINE_EMBEDDED=require` fails instead of falling back.
- Otherwise sidecar/binary path remains default during stabilization.

### 4. Add embedded-vs-sidecar tests

Good first tests:

- Tiny repo fixture.
- Run embedded producer into `FactBatchBuilder`.
- Run sidecar producer.
- Compare stats and key node/edge IDs.
- Assert embedded direct mode does not create engine `*.db`.

Keep this focused in `aka-core`; do not change graph/search writer.

### 5. SCIP / stack-graphs / LSP adapters

Use `crates/aka-facts/src/producer.rs` as the common seam.

Recommended modules:

- `crates/aka-core/src/semantic/scip.rs`
- `crates/aka-core/src/semantic/stack_graphs.rs`
- `crates/aka-core/src/semantic/lsp.rs`

Start with fixture/no-op adapters if dependency scope is too large. The important invariant is: adapters produce semantic facts, then lower to existing `FactSource`. Do not teach the graph/search writer about individual ecosystems.

## Risks And Notes

- Do not claim full fusion until Rust actually uses the embedded path in process.
- Do not remove sidecar/binary fallback yet. It is still the default and still useful for Windows/release stabilization.
- The embedded C API currently uses an environment variable to pass `AKA_ENGINE_CACHE_DIR`; this is acceptable for a first API but not ideal for concurrent multi-engine calls. A future engine API can take cache dir directly through pipeline state.
- The engine has a global pipeline lock. Parallel per-file Rust scheduling is still future work; current engine pipeline has internal parallel extraction but Rust does not yet own task scheduling.
- The product framing remains desktop app + plugin packages. If mentioning `aka-cli`/`apps/cli`, call it the internal runtime/host crate only.

## Useful Commands

```bash
git status --short --branch
git -C engine/aka-engine-src status --short --branch

make -C engine/aka-engine-src -f Makefile.cbm cbm
make -C engine/aka-engine-src -f Makefile.cbm libaka-engine

cargo fmt --check
cargo test -p aka-facts
cargo test -p aka-core
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
npm --prefix apps/desktop run build
```

If `cargo test --workspace` hits sandbox socket permission errors in `aka-mcp --test http`, rerun with elevated permissions; this happened earlier and passed when rerun outside the sandbox.
