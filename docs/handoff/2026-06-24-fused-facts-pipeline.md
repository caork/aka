# Fused Facts Pipeline Handoff

Date: 2026-06-24

Branch: `codex/fused-facts-pipeline`

Goal: remove the `artifact/` / sidecar / engine SQLite path from the hot indexing architecture, fuse native AKA engine facts directly into Rust indexing, and make semantic producers reusable for SCIP / stack-graphs / LSP ecosystems.

## Current State

The fused facts runtime path is now implemented and verified for macOS/Linux product builds. The native AKA engine emits facts through callbacks, Rust can link the embedded engine static library, and the internal runtime used by the desktop shell/plugin host can index through `aka_engine_index_with_sink` without writing engine SQLite or NDJSON artifacts.

Windows intentionally remains on the bundled `aka-engine.exe` fallback path until an MSVC-compatible embedded static library is solved. Legacy sidecar/binary fallback also remains available on all platforms via `AKA_ENGINE_EMBEDDED=0|off|false|no` and for non-embedded builds.

Latest relevant main repo commits:

- `486cff8 feat: add embedded engine fact producer`
- `fe35a62 docs: hand off fused facts pipeline work`
- `26e3b67 infra: pin embedded facts engine`
- `9019e84 feat: add semantic fact producer seam`
- `b326c41 infra: pin fact sink engine`

Nested engine repo `engine/aka-engine-src` latest relevant commits:

- `f77d34f feat: add embedded facts api`
- `1d9c4f6 refactor: emit facts through sink`
- `f96acdd feat: emit direct facts sidecar`

Pinned engine ref:

- `engine/ENGINE_SHA`: `f77d34f853ca1252cde573ff4e49443f95d7efed`
- `Dockerfile` and `.github/workflows/release.yml` are pinned to the same SHA.
- The nested engine commits have been pushed to `caork/aka-engine main`.

## What Is Done

### Engine callback sink

Engine C has `cbm_fact_sink_t` in `src/pipeline/pipeline.h`.

The pipeline emits facts after predump passes and before SQLite persistence. JSONL sidecar output is implemented as a built-in sink, so sidecar is a transport/debug sink rather than the fact-emission architecture.

Important files:

- `engine/aka-engine-src/src/pipeline/pipeline.h`
- `engine/aka-engine-src/src/pipeline/pipeline.c`

### Engine embedded API

Engine has a public embedded API in:

- `engine/aka-engine-src/src/api/aka_engine.h`
- `engine/aka-engine-src/src/api/aka_engine.c`

The public API exposes:

- `aka_engine_index_options_t`
- `aka_engine_fact_sink_t`
- `aka_engine_index_with_sink(...)`

`direct_facts_only = true` sets `cbm_pipeline_set_skip_dump`, which skips engine SQLite discovery/reuse and final SQLite dump/persistence.

`Makefile.cbm` has:

- `make -f Makefile.cbm cbm`
- `make -f Makefile.cbm libaka-engine`

The static library target builds `build/c/libaka_engine.a` with non-override mimalloc (`MI_OVERRIDE=0`) so linking it into Rust/Tauri is less risky than linking the production binary allocator override.

### Rust embedded producer

`aka-core` now has:

- `embedded-engine` feature in `crates/aka-core/Cargo.toml`
- native link build script in `crates/aka-core/build.rs`
- handwritten FFI wrapper in `crates/aka-core/src/engine/embedded.rs`
- refactored `EngineFactProducer::produce(...)` seam in `crates/aka-core/src/engine/fact_producer.rs`

The wrapper:

- defines `#[repr(C)]` ABI structs matching `aka_engine.h`
- copies all borrowed C strings before returning from callbacks
- parses `properties_json` and `evidence_json` with `serde_json`
- stores callback errors and returns nonzero to native code
- catches callback panics so Rust never unwinds across C
- runs the native C pipeline on a 64 MB Rust thread stack to avoid default test/runtime stack overflow
- normalizes facts and synthesizes chunks through the existing helper before replaying into the caller sink

Runtime selection:

- macOS/Linux builds compiled with `embedded-engine` default to embedded facts when `AKA_ENGINE_EMBEDDED` is unset.
- `AKA_ENGINE_EMBEDDED=0|off|false|no` forces binary/sidecar fallback.
- `AKA_ENGINE_EMBEDDED=1|true` opts in explicitly.
- `AKA_ENGINE_EMBEDDED=require` fails if embedded is unavailable.
- Windows defaults to bundled binary fallback and does not compile the embedded module.

### Product build wiring

- `apps/cli` exposes `embedded-engine = ["aka-core/embedded-engine"]`.
- `apps/desktop/src-tauri` exposes `embedded-engine = ["aka-cli/embedded-engine"]`.
- macOS desktop packaging passes `--features embedded-engine`.
- `scripts/package-release.sh` now ensures `libaka_engine.a` exists and exports `AKA_ENGINE_LIB_DIR` before macOS Tauri builds.
- Windows desktop packaging does not pass `embedded-engine` and keeps the single-file `AKA.exe` + embedded `aka-engine.exe` resource behavior.
- Docker builds the engine binary and static library before the Rust runtime, then builds the Linux runtime with `--features embedded-engine`; `/opt/aka/engine/aka-engine` remains in the image as explicit fallback.
- CI keeps the default no-feature workspace test/clippy gates and adds an embedded Linux gate that clones the pinned engine ref, builds `libaka_engine.a`, then runs `cargo test -p aka-core --features embedded-engine` and `cargo test -p aka-cli --features embedded-engine`.

### Semantic producer seam

`aka-facts` has a public semantic producer seam:

- `ProducerContext`
- `ProducerCapability`
- `SemanticFactSink`
- `SemanticFactProducer`
- `SemanticFactBundleBuilder`
- `produce_semantic_batch`
- `produce_semantic_into`
- `replay_semantic_bundle_into`

`crates/aka-core/src/lib.rs` re-exports the semantic records and producer traits so future SCIP / stack-graphs / LSP adapters can write facts without depending on private engine internals.

Graph/search writers consume `FactSource`; they no longer require artifact files. They are still replay-based rather than a one-pass streaming writer because the current writer reads nodes more than once.

## Verification Run

Engine and facts:

```bash
make -C engine/aka-engine-src -f Makefile.cbm cbm
make -C engine/aka-engine-src -f Makefile.cbm libaka-engine
cargo test -p aka-facts
```

Rust default and embedded gates:

```bash
cargo fmt --check
cargo test -p aka-core
cargo test -p aka-core --features embedded-engine
cargo test -p aka-cli --features embedded-engine
cargo clippy -p aka-core --features embedded-engine --all-targets -- -D warnings
cargo clippy -p aka-cli --features embedded-engine --all-targets -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

Embedded native smoke:

```bash
cargo test -p aka-core --features embedded-engine \
  embedded_engine_indexes_tiny_repo_without_sqlite_dump -- --ignored
```

This verified native embedded indexing on a tiny repo and no engine `*.db` dump in the cache tree.

Internal runtime end-to-end smoke:

```bash
TMP_AKA_HOME=$(mktemp -d /tmp/aka-fused-smoke.XXXXXX)
AKA_HOME="$TMP_AKA_HOME" cargo run -p aka-cli --features embedded-engine -- analyze fixtures/demo-ts
AKA_HOME="$TMP_AKA_HOME" cargo run -p aka-cli --features embedded-engine -- repos
AKA_HOME="$TMP_AKA_HOME" cargo run -p aka-cli --features embedded-engine -- search user --repo demo-ts --limit 5
```

Observed evidence:

- analyze log included `embedded repo_path=...`, `aka-engine:index:embedded`, and native `pipeline.route path=direct_facts`
- graph/search indexes were built under the temp `AKA_HOME`
- `aka repos` listed `demo-ts`
- `aka search user --repo demo-ts --limit 5` returned real hits

Desktop web build:

```bash
npm --prefix apps/desktop run build
```

Docker:

```bash
docker build -t aka:embedded-smoke .
```

This could not be run locally because the Docker daemon was not running (`Cannot connect to the Docker daemon at unix:///var/run/docker.sock`). The Dockerfile has been statically inspected and CI/release will exercise it when Docker is available.

## Remaining Follow-Ups

These are not blockers for the fused hot path but remain useful next work:

- Implement an MSVC-compatible Windows embedded static library if we want Windows to use in-process engine callbacks instead of the current bundled binary fallback.
- Replace replayable `FactBatch` with a true one-pass graph/search writer after the writer no longer needs to reread nodes.
- Add real SCIP / stack-graphs / LSP adapters on top of `aka-facts::SemanticFactProducer`; the seam and fake producer tests already exist.
- Improve the C embedded API to take cache dir through pipeline state instead of process-global `AKA_ENGINE_CACHE_DIR`.
- Run Docker image smoke on a machine with Docker daemon available.

## Useful Commands

```bash
git status --short --branch
git -C engine/aka-engine-src status --short --branch

make -C engine/aka-engine-src -f Makefile.cbm libaka-engine

cargo fmt --check
cargo test -p aka-facts
cargo test -p aka-core
cargo test -p aka-core --features embedded-engine
cargo test -p aka-cli --features embedded-engine
cargo clippy --workspace --all-targets -- -D warnings
npm --prefix apps/desktop run build
```
