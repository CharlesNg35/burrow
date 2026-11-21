# Patched crates

## reactive_graph 0.2.16: dead subscribers and owners

`patches/reactive_graph/` is reactive_graph 0.2.16 from crates.io (Leptos 0.8's reactive system) with one upstream fix. The root `Cargo.toml` uses it through `[patch.crates-io]`.

**The bug** ([leptos-rs/leptos#4965](https://github.com/leptos-rs/leptos/issues/4965)): a view that mounts and unmounts under a long-lived owner leaves two dead entries behind:
- its effects and memos in the subscriber set of every signal they read;
- its owner in its parent's `children`.

Both stay until the signal or the parent goes away. In burrow admin, that is about 450 bytes for each list row, tab or popup that comes and goes, and every update of those signals walks the dead entries.

**The fix** is upstream's own ([leptos-rs/leptos#4966](https://github.com/leptos-rs/leptos/pull/4966), commits `915a70d2be` and `fd4b21709c` on `leptos_0.8_latest`, in reactive_graph 0.3.0-rc1). When a subscriber set or a `children` list is about to grow, it first drops the entries whose node is gone, then reserves room for the live ones. That keeps the cost amortized O(1), keeps the order, and adds no `unsafe`. It changes `src/graph/sets.rs` and `src/owner.rs` and adds upstream's tests (`src/owner.rs`, `src/signal/arc_rw.rs`).

burrow adds `tests/flat_memory.rs`: 1000 mount/unmount cycles must not grow the heap. Without the fix, they keep about 444 KB. The test's counting allocator is its only `unsafe`.

The whole change against the crates.io release is `reactive_graph-0.2.16-leak.patch`. The vendored `Cargo.toml` also gets an empty `[workspace]`, the `flat_memory` test target and `unused = "allow"`, since Cargo silences the warnings of crates.io dependencies but not of path ones.

### Check it
```bash
cd patches/reactive_graph && cargo test --locked --release --features effects
```

### When to delete it
Delete `patches/reactive_graph/`, this section, the patch file and the `reactive_graph` line in `[patch.crates-io]` as soon as a reactive_graph release that Leptos 0.8 uses contains the fix: 0.2.17 or newer, or Leptos 0.9 (reactive_graph 0.3). Check that `src/graph/sets.rs` of that release prunes dead subscribers (`strong_count() > 0` in `subscribe`), then run `cargo update -p reactive_graph`.

### Moving to a newer 0.2.x that still lacks the fix
```bash
v=0.2.17
cargo download reactive_graph@$v -x -o /tmp/rg   # or copy ~/.cargo/registry/src/*/reactive_graph-$v
rm -rf patches/reactive_graph && cp -r /tmp/rg patches/reactive_graph && rm -f patches/reactive_graph/.cargo-ok
cd patches/reactive_graph && patch -p1 < ../reactive_graph-0.2.16-leak.patch && cd ../..
```
Fix any rejected hunk by hand, keeping the upstream code. Then run the check above, write the patch file again with `diff -ruN` against the pristine release, and run `cargo update -p reactive_graph`.
