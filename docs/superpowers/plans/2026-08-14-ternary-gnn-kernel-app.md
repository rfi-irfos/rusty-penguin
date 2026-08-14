# Ternary GNN Kernel App Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run Kindom's DINGIR reasoning layer (2-layer GNN + DistMult relation scoring) as native ternary inference inside the Rusty Penguin bare-metal kernel desktop, as a new app.

**Architecture:** Python-side export script quantizes the trained model + graph to a flat binary snapshot, embedded into the RP ISO via the existing CPIO ramfs mechanism. A new `ai-runtime::graph` module adds neighbor-aggregation + DistMult scoring on top of the existing `TernaryLinear`/`sparse_dot` primitives. A new `desktop-metal` app (implementing the existing `App` trait, following the `Calculator` app's compute+render pattern) loads the snapshot via the existing `sys_open`/`sys_read` syscalls (no new syscall needed — `vfs::open` already resolves any ramfs path generically) and renders ranked relation predictions for a fixed node pair.

**Tech Stack:** Rust nightly (no_std, `-Zbuild-std=core,alloc,compiler_builtins`), x86_64 bare-metal target, QEMU for verification. Python 3.14 + PyTorch for the export script (reuses `kindom/execution`'s existing `quantize_weights` MCP tool).

**Spec:** `docs/superpowers/specs/2026-08-14-ternary-gnn-kernel-app-design.md`

## Global Constraints

- No new syscall — reuse `sys_open` (#2) / `sys_read` (#0) / `sys_close` (#3), confirmed generic over any ramfs path via `vfs::open` → `ramfs::find` (`kernel/src/vfs.rs:17-21`).
- `ai-runtime` stays `#![no_std]` — no new dependency on `std`, `alloc`-only where needed (already available to both `kernel` and, after Task 4, `desktop-metal`).
- Sum-aggregation-with-saturation for graph propagation, NOT degree-normalized mean (no clean division in ternary integer arithmetic) — this is a deliberate, stated approximation of the Python model, not a precision-loss port. Every place this matters must say so, not imply parity.
- v1 scope: one hardcoded node pair, no interactive graph browsing (separate future work), no live graph updates (rebuild ISO to refresh the snapshot).
- Every milestone gets QEMU-verified before being claimed done, matching this repo's own stated convention (README: "every milestone is verified in QEMU before we claim it").
- Follow existing file/module conventions exactly: `mod` line added to the relevant `main.rs` for new files (no other build-system change needed for pure Rust modules); CPIO embedding via `iso/build.sh`'s existing `cp ... "$BARE_INITRAMFS_DIR/bin/..."` pattern.

---

## Task 1: Python export script — model + graph → flat binary snapshot

**Files:**
- Create: `/home/eri-irfos/Desktop/kindom/execution/scripts/export_rp_snapshot.py`
- Test: inline assert-based check (matches this repo's own `acceptance_test_campaign_N.py` convention — no pytest anywhere in kindom)

**Interfaces:**
- Produces: a binary file `rp_snapshot.bin` with this exact layout (little-endian throughout):
  ```
  Header:  u32 num_nodes, u32 num_edges, u32 num_relations, u32 embed_dim, u32 hidden_dim
  NodeIds: num_nodes × u32 (node index -> stable u32 id, for reference only; the
           snapshot itself is index-based, node embeddings are NOT included —
           v1 runs the GNN cold from zero node-bias features + the trained
           per-node embedding table, matching KindomGNN's own embed table)
  NodeEmb: num_nodes × embed_dim × i8 (ternary trits: -1/0/1 — the trained
           per-node id embedding table, `KindomGNN.embed`, quantized)
  Edges:   num_edges × (u32 src_idx, u32 dst_idx, u16 relation_id)
  RelEmb:  num_relations × hidden_dim × i8 (ternary trits — `rel_embed`, quantized)
  Lin1W:   (hidden_dim) × (embed_dim + embed_dim) × i8  (KindomGNN.lin1.weight, quantized;
           in_dim = type_onehot+degree+signal features(+2) + embed_dim — for v1,
           SKIP the non-embedding features entirely, in_dim reduces to embed_dim
           only, i.e. treat lin1 as operating on the embedding alone. This is a
           second stated simplification alongside sum-aggregation — document it
           in the header comment of this script AND in the app's rendered output,
           never silently.)
  Lin2W:   hidden_dim × hidden_dim × i8 (KindomGNN.lin2.weight, quantized)
  ```
  (Exact byte layout may be adjusted during implementation if `KindomGNN`'s actual
  weight shapes require it — the implementer must print the real shapes from a
  loaded `ReasoningLayer` first and adjust this header comment to match reality,
  not force reality to match this guess. This is exactly the kind of place where
  an implementer should say "the plan's byte layout assumption was wrong, here's
  what I found and what I built instead" rather than silently deviating.)
- Consumes: `ReasoningLayer.build(g)` (from `kindom/execution/intelligence/reasoning_layer.py`, already on `master` after today's DistMult merge) for the trained model; the MCP tool `mcp__ternlang-engram__quantize_weights` for float→trit conversion (BitNet-style auto-threshold, verified working this session — call once per weight tensor, flattened to a 1D list of floats, reshape the returned `trits` array back to the original shape).

- [ ] **Step 1: Write the script**

```python
#!/usr/bin/env python3
"""
Export a Kindom ReasoningLayer to a flat binary snapshot for Rusty Penguin's
ai-runtime ternary inference (no JSON parser exists in the no_std kernel).

Every float is quantized to a ternary trit (-1/0/+1) via BitNet-style
auto-threshold quantization (same method as mcp__ternlang-engram__quantize_weights)
BEFORE being written -- Rusty Penguin never sees a float.

KNOWN SIMPLIFICATIONS (stated here, not hidden):
  1. Graph propagation on RP uses sum-aggregation-with-saturation, not the
     Python model's degree-normalized mean -- RP's predictions will genuinely
     differ from Python's, not just be less precise.
  2. v1 skips KindomGNN's non-embedding input features (type one-hot, degree,
     SIGNAL flag) entirely -- lin1 on RP operates on the embedding table
     output alone. This further diverges RP's output from Python's.
Both are acceptable for a first version per the design spec
(docs/superpowers/specs/2026-08-14-ternary-gnn-kernel-app-design.md), on the
condition that the actual divergence gets MEASURED (Task 7), not assumed away.
"""
import struct
import sys
from pathlib import Path

REPO = Path("/home/eri-irfos/Desktop/kindom/execution")
sys.path.insert(0, str(REPO))

from intelligence.graph.evidence_graph import EvidenceGraph
from intelligence.reasoning_layer import ReasoningLayer

OUT = REPO.parent.parent / "rusty-penguin" / "docs" / "superpowers" / "specs" / "rp_snapshot.bin"


def quantize_flat(values: list[float]) -> list[int]:
    """Threshold-quantize a flat float list to trits in {-1,0,1}.
    Mirrors mcp__ternlang-engram__quantize_weights' BitNet formula:
    threshold = 0.75 * mean(|v|); |v| > threshold -> sign(v), else 0."""
    if not values:
        return []
    mean_abs = sum(abs(v) for v in values) / len(values)
    threshold = 0.75 * mean_abs
    out = []
    for v in values:
        if v > threshold:
            out.append(1)
        elif v < -threshold:
            out.append(-1)
        else:
            out.append(0)
    return out


def main():
    g = EvidenceGraph().load_all()
    rl = ReasoningLayer.build(g)
    rl.train_link_prediction(epochs=150)

    num_nodes = len(rl.inv_idx)
    num_relations = len(rl.rel_vocab)
    embed_dim = rl.model.embed.embedding_dim
    hidden_dim = rl.model.hidden

    print(f"Shapes: nodes={num_nodes} relations={num_relations} "
          f"embed_dim={embed_dim} hidden_dim={hidden_dim}")
    print(f"lin1.weight shape: {tuple(rl.model.lin1.weight.shape)}")
    print(f"lin2.weight shape: {tuple(rl.model.lin2.weight.shape)}")
    print(f"rel_embed.weight shape: {tuple(rl.model.rel_embed.weight.shape)}")
    # STOP: verify these printed shapes match the header comment in this file's
    # docstring before writing the binary. If lin1's in_dim is NOT embed_dim
    # (it likely also includes type-onehot+degree+signal bias features per
    # build_features() in reasoning_layer.py), either slice out just the
    # embed_dim-wide sub-block of lin1's weight matrix that multiplies the
    # embedding portion of the input, or extend the snapshot format to carry
    # the additional bias features too. Make this call explicitly and document
    # it in a comment right here once decided -- do not silently pick one.

    node_emb_flat = rl.model.embed.weight.detach().flatten().tolist()
    rel_emb_flat = rl.model.rel_embed.weight.detach().flatten().tolist()
    lin2_flat = rl.model.lin2.weight.detach().flatten().tolist()

    node_emb_trits = quantize_flat(node_emb_flat)
    rel_emb_trits = quantize_flat(rel_emb_flat)
    lin2_trits = quantize_flat(lin2_flat)

    with open(OUT, "wb") as f:
        f.write(struct.pack("<IIIII", num_nodes, len(rl.edge_relations),
                            num_relations, embed_dim, hidden_dim))
        for nid in rl.inv_idx:
            f.write(struct.pack("<I", rl.idx[nid]))
        f.write(bytes((t & 0xFF) for t in node_emb_trits))
        inv_rel = {v: k for k, v in rl.rel_vocab.items()}
        for s, o, r in zip(rl.edge_index[0].tolist(), rl.edge_index[1].tolist(),
                           rl.rel_index.tolist()):
            f.write(struct.pack("<IIH", s, o, r))
        f.write(bytes((t & 0xFF) for t in rel_emb_trits))
        f.write(bytes((t & 0xFF) for t in lin2_trits))
        # lin1 write deferred to the in_dim decision above

    print(f"Wrote {OUT} ({OUT.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Run it and inspect the printed shapes**

```bash
cd /home/eri-irfos/Desktop/kindom/execution && python3 scripts/export_rp_snapshot.py
```

Expected: prints real tensor shapes. Resolve the `lin1` in_dim question flagged in the script's comment based on what's actually printed, complete the `lin1` write, and re-run.

- [ ] **Step 3: Verify the file is well-formed**

```bash
python3 -c "
import struct
with open('/home/eri-irfos/projects/rusty-penguin/docs/superpowers/specs/rp_snapshot.bin', 'rb') as f:
    data = f.read()
num_nodes, num_edges, num_rel, embed_dim, hidden_dim = struct.unpack('<IIIII', data[:20])
print(f'nodes={num_nodes} edges={num_edges} rel={num_rel} embed_dim={embed_dim} hidden_dim={hidden_dim}')
assert num_nodes > 0 and num_edges > 0 and num_rel > 0
print('OK')
"
```

Expected: `OK` with sane, non-zero counts matching Task 1's earlier printed shapes.

- [ ] **Step 4: Commit**

```bash
cd /home/eri-irfos/Desktop/kindom && git add execution/scripts/export_rp_snapshot.py && git -c user.name="simeon-kepp" -c user.email="rfi.irfos@gmail.com" commit -m "feat: export Kindom GNN to flat ternary binary snapshot for Rusty Penguin"
```

---

## Task 2: `ai-runtime::graph` — propagation + DistMult scoring

**Files:**
- Create: `ai-runtime/src/graph.rs`
- Modify: `ai-runtime/src/lib.rs` (add `pub mod graph;`)

**Interfaces:**
- Consumes: `TernaryTensor`, `TernaryLinear` (existing, `ai-runtime/src/lib.rs`), `Trit` (from `ternary-core`, already a dependency of `ai-runtime` per its `Cargo.toml`).
- Produces: `propagate(node_embeddings: &[TernaryTensor], edges: &[(u32, u32)]) -> Vec<TernaryTensor>`, `distmult_score(node_a: &TernaryTensor, rel: &TernaryTensor, node_b: &TernaryTensor) -> i32`.

- [ ] **Step 1: Write the failing test**

In `ai-runtime/src/graph.rs`, start with:

```rust
use crate::TernaryTensor;
use ternary_core::Trit;
use alloc::vec::Vec;
use alloc::vec;

/// Sum-aggregate each node's neighbor embeddings, with saturation at [-1,+1]
/// per dimension (via the same step-function convention KindomGNN's own
/// TernaryLinear::forward uses: sum>0 -> Pos, sum<0 -> Neg, else Zero).
/// This is NOT degree-normalized mean -- ternary integer arithmetic has no
/// clean division. Divergence from a float degree-normalized GCN is expected
/// and must be measured (Task 7), not assumed negligible.
pub fn propagate(node_embeddings: &[TernaryTensor], edges: &[(u32, u32)]) -> Vec<TernaryTensor> {
    let n = node_embeddings.len();
    if n == 0 {
        return Vec::new();
    }
    let dim = node_embeddings[0].shape[0];
    let mut sums: Vec<Vec<i32>> = vec![vec![0i32; dim]; n];
    for &(src, dst) in edges {
        let (s, d) = (src as usize, dst as usize);
        if s >= n || d >= n {
            continue;
        }
        for k in 0..dim {
            sums[d][k] += node_embeddings[s].data[k].to_i8() as i32;
        }
    }
    sums.into_iter()
        .map(|row| {
            let trits: Vec<Trit> = row
                .into_iter()
                .map(|v| if v > 0 { Trit::Pos } else if v < 0 { Trit::Neg } else { Trit::Zero })
                .collect();
            TernaryTensor::new(trits, vec![dim])
        })
        .collect()
}

/// DistMult trilinear score: sum(node_a * rel * node_b), elementwise.
/// An INFERENCE score -- callers must not treat this as an observed fact.
pub fn distmult_score(node_a: &TernaryTensor, rel: &TernaryTensor, node_b: &TernaryTensor) -> i32 {
    let n = node_a.data.len().min(rel.data.len()).min(node_b.data.len());
    let mut sum = 0i32;
    for i in 0..n {
        sum += (node_a.data[i].to_i8() as i32)
            * (rel.data[i].to_i8() as i32)
            * (node_b.data[i].to_i8() as i32);
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_propagate_two_node_chain() {
        // node 0 --edge--> node 1. node1's aggregated embedding should equal
        // node0's embedding (single neighbor, sum == that neighbor's values).
        let n0 = TernaryTensor::new(vec![Trit::Pos, Trit::Neg, Trit::Zero], vec![3]);
        let n1 = TernaryTensor::zeros(vec![3]);
        let out = propagate(&[n0.clone(), n1], &[(0, 1)]);
        assert_eq!(out[1].data, n0.data);
        // node 0 has no incoming edges -> stays zero
        assert_eq!(out[0].data, vec![Trit::Zero, Trit::Zero, Trit::Zero]);
    }

    #[test]
    fn test_propagate_saturation() {
        // two Pos-valued neighbors sum to +2 on that dim -> still saturates to Pos, not overflow
        let a = TernaryTensor::new(vec![Trit::Pos], vec![1]);
        let b = TernaryTensor::new(vec![Trit::Pos], vec![1]);
        let c = TernaryTensor::zeros(vec![1]);
        let out = propagate(&[a, b, c], &[(0, 2), (1, 2)]);
        assert_eq!(out[2].data, vec![Trit::Pos]);
    }

    #[test]
    fn test_distmult_score_matches_true_relation() {
        let node_a = TernaryTensor::new(vec![Trit::Pos, Trit::Pos], vec![2]);
        let node_b = TernaryTensor::new(vec![Trit::Pos, Trit::Neg], vec![2]);
        let true_rel = TernaryTensor::new(vec![Trit::Pos, Trit::Neg], vec![2]);
        // Pos*Pos*Pos=1, Pos*Neg*Neg=1 -> sum=2
        assert_eq!(distmult_score(&node_a, &true_rel, &node_b), 2);
        let wrong_rel = TernaryTensor::new(vec![Trit::Zero, Trit::Zero], vec![2]);
        assert_eq!(distmult_score(&node_a, &wrong_rel, &node_b), 0);
    }
}
```

Add `pub mod graph;` to `ai-runtime/src/lib.rs` right after the existing `use` lines at the top of the file.

- [ ] **Step 2: Run it to confirm it fails to compile first (module not wired), then passes**

```bash
cd /home/eri-irfos/projects/rusty-penguin && cargo test -p ai-runtime 2>&1 | tail -30
```

Expected first (before adding `pub mod graph;` to lib.rs): a compile error that `graph` isn't a module. After adding the `pub mod graph;` line: `test graph::tests::test_propagate_two_node_chain ... ok`, `test graph::tests::test_propagate_saturation ... ok`, `test graph::tests::test_distmult_score_matches_true_relation ... ok`, plus the two pre-existing tests in `lib.rs` still passing.

- [ ] **Step 3: Commit**

```bash
git add ai-runtime/src/graph.rs ai-runtime/src/lib.rs && git -c user.name="simeon-kepp" -c user.email="rfi.irfos@gmail.com" commit -m "feat: add graph propagation + DistMult scoring to ai-runtime"
```

---

## Task 3: De-risk ring-3 ramfs access (spike, QEMU-verified)

**Files:**
- Modify: `iso/build.sh` (add one `cp` line)
- Create: a tiny throwaway test file, `docs/superpowers/specs/rp_probe.txt` containing exactly the bytes `PROBE-OK` (8 bytes)
- Modify: `desktop-metal/src/main.rs` (temporary debug print — this task exists specifically to verify the Global Constraint that no new syscall is needed, before Task 5 builds on top of that assumption)

**Interfaces:**
- Consumes: `sys_open` (#2), `sys_read` (#0), `sys_close` (#3) via the exact raw-asm calling convention already used elsewhere in `desktop-metal/src/app.rs:11-19` (`inout("rax") N => n, in("rdi") arg1, in("rsi") arg2, in("rdx") arg3, out("rcx") _, out("r11") _, options(nostack)`).

- [ ] **Step 1: Embed the probe file**

In `iso/build.sh`, find the block that copies other non-ELF blobs into `$BARE_INITRAMFS_DIR/bin/` (near where `bin/dingir.ppm` / `bin/meta.rpv` get copied). Add directly before the `cpio` pack invocation:

```bash
cp "$REPO_ROOT/docs/superpowers/specs/rp_probe.txt" "$BARE_INITRAMFS_DIR/bin/rp_probe.txt"
```

(Use the exact `$REPO_ROOT`/`$BARE_INITRAMFS_DIR` variable names already used by the surrounding lines in this script — read the actual surrounding lines first, don't guess the variable names.)

- [ ] **Step 2: Add a temporary probe call in desktop-metal**

In `desktop-metal/src/main.rs`, near existing boot-time initialization (find where other one-shot startup calls happen, e.g. near `sys_meminfo`/`sys_cpu` usage patterns already in the codebase), add:

```rust
unsafe fn probe_ramfs_read() -> bool {
    let path = b"bin/rp_probe.txt";
    let fd: u64;
    core::arch::asm!("syscall", inout("rax") 2u64 => fd, in("rdi") path.as_ptr(),
        in("rsi") path.len() as u64, out("rcx") _, out("r11") _, options(nostack));
    if fd == u64::MAX {
        return false;
    }
    let mut buf = [0u8; 8];
    let n: u64;
    core::arch::asm!("syscall", inout("rax") 0u64 => n, in("rdi") fd,
        in("rsi") buf.as_mut_ptr(), in("rdx") buf.len() as u64,
        out("rcx") _, out("r11") _, options(nostack));
    core::arch::asm!("syscall", inout("rax") 3u64 => _, in("rdi") fd,
        out("rcx") _, out("r11") _, options(nostack));
    n == 8 && &buf == b"PROBE-OK"
}
```

Call it once at startup and write the boolean result to the serial port (find the existing serial-write helper already used elsewhere in `desktop-metal` — do not invent a new one) so it's visible in the QEMU serial log without needing the framebuffer UI running.

- [ ] **Step 3: Build and verify in QEMU**

```bash
cd /home/eri-irfos/projects/rusty-penguin && bash iso/build.sh 2>&1 | tail -20 && timeout 30 bash launch.sh -serial file:/tmp/rp-probe.log -display none 2>&1 | tail -5
grep -i probe /tmp/rp-probe.log
```

(Adapt the exact QEMU/launch invocation to however `launch.sh` actually accepts serial+headless flags — read `launch.sh` first if the flags above don't match its actual argument parsing.)

Expected: the serial log shows the probe result is `true`. If `false` or the read returns fewer than 8 bytes, STOP — this means the Global Constraint ("no new syscall needed") is wrong, and Task 5's design needs to add a new syscall instead. Do not proceed to Task 4/5 on an unverified assumption.

- [ ] **Step 4: Remove the temporary probe code, keep the probe file embedding**

Revert the `desktop-metal/src/main.rs` debug call (the permanent version lives in Task 5's app, not here) but leave the `iso/build.sh` line and `rp_probe.txt` — cheap to keep as a regression check reusable by a future test.

- [ ] **Step 5: Commit**

```bash
git add iso/build.sh docs/superpowers/specs/rp_probe.txt && git -c user.name="simeon-kepp" -c user.email="rfi.irfos@gmail.com" commit -m "test: verify ring-3 ramfs read via existing sys_open/sys_read syscalls"
```

---

## Task 4: Add `ai-runtime` as a `desktop-metal` dependency

**Files:**
- Modify: `desktop-metal/Cargo.toml`

**Interfaces:**
- Produces: `desktop-metal` can `use ai_runtime::{TernaryTensor, TernaryLinear};` and `use ai_runtime::graph::{propagate, distmult_score};`.

- [ ] **Step 1: Add the dependency**

In `desktop-metal/Cargo.toml`, under `[dependencies]` (currently only `libm`), add:

```toml
ai-runtime = { path = "../ai-runtime" }
```

- [ ] **Step 2: Verify it cross-compiles**

Add a temporary smoke-test line in `desktop-metal/src/main.rs` (e.g. right after existing top-level `use` statements): `use ai_runtime::TernaryTensor as _;` — enough to force the compiler to resolve the new dependency without adding real logic yet.

```bash
cd /home/eri-irfos/projects/rusty-penguin/desktop-metal && cargo +nightly build --release -Zbuild-std=core,alloc,compiler_builtins --target x86_64-user-psh.json 2>&1 | tail -30
```

Expected: builds clean. If it fails on a missing `alloc`/`core` symbol, that's real information — report it rather than working around it blindly (per the plan's own honesty norm), since the design spec assumed this would "just work" based on `desktop-metal` already building `alloc`.

- [ ] **Step 3: Remove the smoke-test `use` line (Task 5 adds the real usage), commit**

```bash
cd /home/eri-irfos/projects/rusty-penguin && git add desktop-metal/Cargo.toml && git -c user.name="simeon-kepp" -c user.email="rfi.irfos@gmail.com" commit -m "feat: add ai-runtime as a desktop-metal dependency"
```

---

## Task 5: `DingirGnnApp` — the desktop app itself

**Files:**
- Modify: `desktop-metal/src/app.rs` (add the new `DingirGnnApp` struct + `App` impl, following `Calculator`'s pattern at `app.rs:1293-1571`)

**Interfaces:**
- Consumes: `ai_runtime::{TernaryTensor, TernaryLinear}`, `ai_runtime::graph::{propagate, distmult_score}` (Task 2), the `App` trait (`app.rs:89-118`, unchanged), the syscall pattern verified in Task 3, the snapshot binary format from Task 1.
- Produces: `pub struct DingirGnnApp { ... }` implementing `App`, with a constructor `DingirGnnApp::new() -> Self` that loads the snapshot and runs inference once at construction time (v1: not re-run on tick/interaction).

- [ ] **Step 1: Implement snapshot loading + parsing**

```rust
struct DingirSnapshot {
    num_nodes: u32,
    num_edges: u32,
    num_relations: u32,
    embed_dim: u32,
    hidden_dim: u32,
    node_embeddings: Vec<TernaryTensor>,
    edges: Vec<(u32, u32, u16)>,
    rel_embeddings: Vec<TernaryTensor>,
    lin2: TernaryLinear,
}

impl DingirSnapshot {
    unsafe fn load() -> Option<Self> {
        let path = b"bin/dingir_snapshot.bin";
        let fd: u64;
        core::arch::asm!("syscall", inout("rax") 2u64 => fd, in("rdi") path.as_ptr(),
            in("rsi") path.len() as u64, out("rcx") _, out("r11") _, options(nostack));
        if fd == u64::MAX {
            return None;
        }
        // Snapshot files are small (a few KB for this graph size) -- read into
        // a fixed-size stack/heap buffer in one or a few syscalls, matching
        // the byte layout Task 1 actually produced (read the real header
        // format from export_rp_snapshot.py's final Step 2 output before
        // writing the parsing offsets here -- do not guess field sizes).
        // ... parse header, node embeddings, edges, rel embeddings, lin2 ...
        core::arch::asm!("syscall", inout("rax") 3u64 => _, in("rdi") fd,
            out("rcx") _, out("r11") _, options(nostack));
        // return Some(DingirSnapshot { ... })
        None // placeholder until the real parse is written against Task 1's actual header
    }
}
```

(This step's exact byte-offset parsing code depends on Task 1's final, verified header layout — do not write speculative offsets; read `export_rp_snapshot.py`'s final struct.pack calls first.)

- [ ] **Step 2: Implement the `App` trait**

```rust
pub struct DingirGnnApp {
    snapshot: Option<DingirSnapshot>,
    predictions: Vec<(u16, i32)>, // (relation_id, distmult_score), sorted descending
    dirty: bool,
}

impl DingirGnnApp {
    pub fn new() -> Self {
        let snapshot = unsafe { DingirSnapshot::load() };
        let predictions = match &snapshot {
            Some(s) => Self::run_inference(s),
            None => Vec::new(),
        };
        Self { snapshot, predictions, dirty: true }
    }

    fn run_inference(s: &DingirSnapshot) -> Vec<(u16, i32)> {
        // Layer 1: propagate raw node embeddings, then lin2 (v1 skips lin1
        // per Task 1's in_dim decision -- if Task 1 kept lin1 in scope,
        // apply it here as an additional TernaryLinear::forward call).
        let layer1 = ai_runtime::graph::propagate(&s.node_embeddings,
            &s.edges.iter().map(|&(a, b, _)| (a, b)).collect::<Vec<_>>());
        // Fixed node pair for v1 -- indices 0 and 1 into the snapshot's node list.
        let (a, b) = (0usize, 1usize);
        if a >= layer1.len() || b >= layer1.len() {
            return Vec::new();
        }
        let h_a = s.lin2.forward(&layer1[a]).0;
        let h_b = s.lin2.forward(&layer1[b]).0;
        let mut scored: Vec<(u16, i32)> = s.rel_embeddings.iter().enumerate()
            .map(|(i, rel)| (i as u16, ai_runtime::graph::distmult_score(&h_a, rel, &h_b)))
            .collect();
        scored.sort_by(|x, y| y.1.cmp(&x.1));
        scored
    }
}

impl App for DingirGnnApp {
    fn title(&self) -> &str { "DINGIR GNN" }

    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        if !self.dirty { return; }
        // Follow Calculator's render pattern (app.rs ~1493-1506): draw the
        // snapshot-load status and, if loaded, the ranked (relation_id, score)
        // list as text lines via fb.draw_aa(...). If snapshot is None, render
        // a clear "snapshot not found" message -- never a silent blank window.
        self.dirty = false;
    }

    fn on_key(&mut self, _key: u8) {}

    fn wants_close(&self) -> bool { false }
}
```

- [ ] **Step 3: Register the app**

In `desktop-metal/src/main.rs`, add a constructor function `open_dingir_gnn(w, h, n) -> Option<TermWin>` following `open_tis_console`'s exact shape (`main.rs:2354-2375`), then wire it into all three dispatch points the Explore report identified: `open_app_by_index` (`main.rs:255-277`), `launch_menu_item` (`main.rs:2405-2430`), and the desktop-icon dispatch (`main.rs:3320-3336`) — pick the next unused index number (read the current highest index in `open_app_by_index` first, do not assume `6`/`7` without checking, since other apps may have been added since the Explore report was generated). Add one `MenuItem` entry to `MENU_ITEMS` (`main.rs:1757-1778`) reusing an existing icon constant from `icons.rs` (e.g. `icons::IC_TIS` or another existing science/compute-flavored icon) rather than regenerating the auto-generated icon file for v1.

- [ ] **Step 4: Build and commit**

```bash
cd /home/eri-irfos/projects/rusty-penguin/desktop-metal && cargo +nightly build --release -Zbuild-std=core,alloc,compiler_builtins --target x86_64-user-psh.json 2>&1 | tail -30
```

Expected: clean build.

```bash
cd /home/eri-irfos/projects/rusty-penguin && git add desktop-metal/src/app.rs desktop-metal/src/main.rs && git -c user.name="simeon-kepp" -c user.email="rfi.irfos@gmail.com" commit -m "feat: add DingirGnnApp -- ternary GNN inference desktop app"
```

---

## Task 6: Embed the real snapshot into the ISO

**Files:**
- Modify: `iso/build.sh`

- [ ] **Step 1: Add the embed line**

Add, alongside Task 3's probe-file line and before the `cpio` pack:

```bash
cp "$REPO_ROOT/docs/superpowers/specs/rp_snapshot.bin" "$BARE_INITRAMFS_DIR/bin/dingir_snapshot.bin"
```

- [ ] **Step 2: Full build + boot in QEMU**

```bash
cd /home/eri-irfos/projects/rusty-penguin && bash iso/build.sh 2>&1 | tail -30 && bash launch.sh
```

Manually: launch the new app from the start menu, confirm the window opens and shows either a ranked relation list or a clear "snapshot not found" message (never a blank/crashed window). Screenshot or describe what's actually rendered in the report.

- [ ] **Step 3: Commit**

```bash
git add iso/build.sh && git -c user.name="simeon-kepp" -c user.email="rfi.irfos@gmail.com" commit -m "feat: embed the trained ternary GNN snapshot into the RP ISO"
```

---

## Task 7: End-to-end verification against the Python reference

**Files:**
- Create: `kindom/execution/scripts/verify_rp_snapshot.py`

**Interfaces:**
- Consumes: the same quantized trits Task 1 wrote (re-derive them the same way, or better, load them back out of `rp_snapshot.bin` to guarantee it's testing what actually shipped, not a fresh re-quantization that could silently drift).
- Produces: a printed/logged comparison — for the same fixed node pair used in `DingirGnnApp` (indices 0 and 1), run the identical propagate+lin2+distmult_score computation in Python (integer arithmetic on the trits, mirroring `ai-runtime/src/graph.rs`'s logic exactly) and compare against what RP's QEMU serial/framebuffer output actually showed in Task 6.

- [ ] **Step 1: Write the Python-side reference computation**

```python
#!/usr/bin/env python3
"""
Recompute the same ternary forward pass ai-runtime::graph runs on Rusty
Penguin, in Python, against the SAME rp_snapshot.bin that got embedded into
the ISO -- so this is a true apples-to-apples comparison, not a re-derivation
that could drift from what actually shipped.

Per the design spec: RP's sum-aggregation + skipped lin1 features are KNOWN
to diverge from the original float model. This script does not compare
against the original PyTorch model -- it compares RP's actual on-device
output against what the same ternary math SHOULD produce, to catch bugs in
the Rust port, not to re-litigate the sum-vs-mean design choice.
"""
import struct
from pathlib import Path

SNAPSHOT = Path("/home/eri-irfos/projects/rusty-penguin/docs/superpowers/specs/rp_snapshot.bin")


def step(x: int) -> int:
    return 1 if x > 0 else (-1 if x < 0 else 0)


def main():
    with open(SNAPSHOT, "rb") as f:
        data = f.read()
    num_nodes, num_edges, num_rel, embed_dim, hidden_dim = struct.unpack("<IIIII", data[:20])
    print(f"Loaded snapshot: nodes={num_nodes} edges={num_edges} "
          f"rel={num_rel} embed_dim={embed_dim} hidden_dim={hidden_dim}")
    # Parse node embeddings, edges, rel embeddings, lin2 using the SAME offset
    # arithmetic Task 5's Rust parser uses (copy the final, verified offsets
    # from app.rs's DingirSnapshot::load once Task 5 is done, don't
    # re-derive independently -- divergent offset math between the two would
    # make this "verification" meaningless).
    # ... reproduce propagate() + lin2 + distmult_score for node pair (0, 1) ...
    # print the ranked (relation_id, score) list


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Run and compare against Task 6's actual RP output**

```bash
python3 /home/eri-irfos/Desktop/kindom/execution/scripts/verify_rp_snapshot.py
```

Compare the printed ranking against what was observed running in QEMU (Task 6, Step 2). Document the result plainly in the report: exact match, partial match, or mismatch — and if mismatch, whether it traces to a bug (fix it) or is the expected consequence of a stated design simplification (document it, don't silently patch over it by changing the "expected" side to match a buggy actual).

- [ ] **Step 3: Commit**

```bash
cd /home/eri-irfos/Desktop/kindom && git add execution/scripts/verify_rp_snapshot.py && git -c user.name="simeon-kepp" -c user.email="rfi.irfos@gmail.com" commit -m "test: verify RP ternary GNN output against Python reference computation"
```
