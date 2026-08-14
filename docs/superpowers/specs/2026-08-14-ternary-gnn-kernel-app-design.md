# Rusty Penguin — Ternary GNN Kernel App: Design Spec

**Date:** 2026-08-14
**Status:** DRAFT — for team peer-review before any build starts
**Author:** Claude (architect role, per Simeon's 2026-08-14 standing directive)
**Approach chosen:** direct bare-metal kernel (Simeon's explicit call, against the lower-risk "Linux track first" recommendation — see Risk Assessment)

## 1. Context

Kindom's DINGIR reasoning layer (`~/Desktop/kindom/execution/intelligence/reasoning_layer.py`) currently runs a 2-layer residual GCN + DistMult relation scoring on PyTorch, float weights, full autodiff training. Verified: link-prediction test AUC 0.993, relation-MRR 0.9387 (see `~/Desktop/kindom/execution/datasets/acceptance_test_campaign_4_report.json`).

Simeon's direction (2026-08-14): this reasoning layer should run natively on ternary logic, on Rusty Penguin (github.com/rfi-irfos/rusty-penguin, v3.8.0) as the OS it all runs on. This is Sub-project 1 of 3 identified during brainstorming — the others (native training on RP, DINGIR orchestration itself running on RP) are separate, future spec/plan cycles, not in scope here.

**What already exists and is real, not aspirational:**
- `ai-runtime/src/lib.rs` (RP repo): `#![no_std]` crate with `TernaryTensor`, `sparse_dot` (the `@sparseskip` patent A50296/2026 primitive — skips zero-weight multiplications), and `TernaryLinear` (single dense layer, step-function activation). Forward-inference only — **no training, no autodiff, no backprop**.
- RP's CPIO ramfs (`kernel/src/ramfs.rs`): GRUB loads a CPIO archive as a Multiboot2 module; `ramfs::find(b"path")` returns embedded file bytes. Already used to embed `bin/desktop`, `bin/psh` ELF binaries.
- RP desktop apps (`desktop-metal/src/`): existing pattern for a windowed app (e.g. TIS console) — window manager, framebuffer text/pixel rendering, no external UI toolkit.
- The MCP tool `quantize_weights` (BitNet-style auto-threshold float→trit quantization) — verified working this session, solves the weight-conversion problem without new code.

**What does NOT exist and must be built:**
- Any graph message-passing / neighbor-aggregation logic in `ai-runtime` (it only has a single dense-layer forward pass, no concept of a graph).
- Any binary graph-snapshot format or loader (no JSON parser exists in the no_std kernel context, and building one is out of scope — avoided by design, see §2).
- A DistMult trilinear-scoring primitive in `ai-runtime`.
- The kernel desktop app itself.

## 2. Data transport: Python → RP

No JSON parser in the no_std kernel. Instead, a Python-side export script (new, in `kindom/execution/scripts/export_rp_snapshot.py`) produces a **flat, fixed-width binary format**:

```
Header:  [u32 num_nodes][u32 num_edges][u32 num_relations][u32 embed_dim]
Nodes:   num_nodes × [u32 node_id][i8 × embed_dim ternary node-embedding-bias trits]
Edges:   num_edges × [u32 src][u32 dst][u16 relation_id]
RelEmb:  num_relations × [i8 × hidden_dim ternary relation-embedding trits]
Weights: lin1, lin2 weight matrices, each fully ternary (i8, one byte per trit: -1/0/1)
```

Every float value in this file is already quantized via `quantize_weights` (BitNet-style threshold) on the Python side — RP never sees a float. This file gets embedded into the RP ISO the same way `bin/desktop`/`bin/psh` are: added to the CPIO archive in `iso/build.sh`, loaded via `ramfs::find(b"kindom.snapshot")` at boot.

**Known limitation, stated plainly:** this is a static snapshot baked in at build time, not a live connection to the running Kindom graph. Re-running the export + rebuilding the ISO is required to update it. Live ingestion is out of scope for this spec.

## 3. Graph propagation in `ai-runtime` (new code)

`ai-runtime` gains a new module, `graph.rs`, with a function shaped like:

```rust
pub fn propagate(nodes: &[TernaryTensor], edges: &[(u32, u32, u16)]) -> Vec<TernaryTensor> {
    // For each node, sum neighbor embeddings (ternary element-wise add with
    // saturation at [-1,+1], NOT degree-normalized mean — no clean division
    // in ternary integer arithmetic). This is a deliberate approximation of
    // the Python model's degree-normalized mean aggregation, not a 1:1 port.
}
```

**This is the single largest known-unknown in this spec.** The Python model's residual GCN divides by node degree; ternary sum-aggregation with saturation is a different function, not a decimal-precision-loss version of the same function. Expected consequence: RP's predictions will diverge from Python's, not just be "slightly less precise" — this needs empirical measurement (§5), not an assumption that it'll be "close enough."

## 4. DistMult scoring in `ai-runtime` (new code)

```rust
pub fn distmult_score(node_emb: &TernaryTensor, rel_emb: &TernaryTensor, other_emb: &TernaryTensor) -> i32 {
    // trilinear product via three-way sparse_dot-style accumulation, reusing
    // the existing sparse-skip pattern from TernaryTensor::sparse_dot
}
```

## 5. Desktop app + verification

**Correction after real codebase exploration (2026-08-14, post-approval):** the app belongs in `desktop-metal/src/app.rs` (ring-3 GUI, implements the `App` trait — `render`/`on_key`/`tick`/`title`), NOT in `kernel/src/apps/` as originally guessed — the kernel itself has no windowing concept, only `desktop-metal` does. `ai-runtime` is currently NOT a dependency of `desktop-metal` (which is a workspace-excluded crate depending only on `libm`) — this needs a `Cargo.toml` edit to add it. `desktop-metal` already builds with `alloc` (`-Zbuild-std=core,alloc,compiler_builtins`), which is what `ai-runtime`'s `extern crate alloc` needs, so this should cross-compile without new build flags. TIS Console's existing `infer` command (`app.rs:911-917`) is a **hardcoded demo string, not real inference** — it is not being extended or reused as real integration, just noted as prior art for the window/REPL pattern. The new app follows Calculator's pattern instead (`app.rs:1293-1571`): a struct holding computed state, dirty-flag redraw, real compute triggered from input, results rendered directly.

Data access from ring-3: `desktop-metal` cannot call `ramfs::find` directly (kernel-only). The embedded snapshot is read via the *existing* `sys_open`/`sys_read` syscalls, which already route arbitrary ramfs paths generically through `vfs.rs` (confirmed: `vfs.rs` calls `ramfs::find` for any path, not just ELF exec paths) — no new syscall should be needed. This gets verified as its own step before the full app is built (§ implementation plan), not assumed.

On launch, the app loads the embedded snapshot via open+read, runs `propagate()` for the 2 layers + `distmult_score()` for a preselected node pair (hardcoded pair for v1 — no interactive graph browsing yet, that's a separate later effort), renders the ranked relation list as text in the window.

**Verification (matches RP's own "every milestone QEMU-verified" convention, and Kindom's acceptance-campaign convention):** a new script, `export_rp_snapshot.py`'s companion `verify_rp_snapshot.py`, runs the **same** forward pass in Python (using the quantized trits directly, not the original floats) and records the expected output. Boot RP in QEMU, capture the app's rendered/serial output, compare against the Python-computed expected output. Document the actual agreement/divergence rate — do not claim parity without measuring it, per §3's stated risk.

## 6. Risk Assessment (why this is the higher-risk path)

Approach A (RP Linux-track first) was recommended because it decouples two risks — no_std kernel constraints, and graph-GNN logic correctness — so a bug can be isolated to one or the other. Approach B (this spec) stacks both at once: if RP's forward pass gives wrong-looking results, the cause could be the no_std graph/memory model, the sum-vs-mean aggregation approximation, a snapshot-format bug, or a kernel rendering bug — harder to isolate. Simeon chose B explicitly, informed of this tradeoff. Team reviewers: if you think this risk stacking is a problem, say so — this is exactly what the peer-review step is for.

## 7. Out of scope for this spec

- Live/interactive graph browsing on RP (separate future work, related to the earlier osiris-panel visualization track for Kindom's Python side — different track, don't conflate).
- Training or autodiff on RP (Sub-project 2, separate spec).
- DINGIR's Python orchestration loop itself running on RP (Sub-project 3, separate spec).
- Updating the snapshot without a full ISO rebuild.

## 8. Open questions for reviewers

1. Is sum-aggregation-with-saturation an acceptable design choice for v1, or should we look at a fixed-point (not pure ternary) degree normalization instead, accepting more complexity for closer parity to the Python model?
2. Hardcoded node pair for v1 display — acceptable, or does the first version need at least a small fixed set of pairs to cycle through?
3. Any objection to the CPIO-embedded static-snapshot approach, or does live ingestion need to be pulled into scope now rather than later?
