// burrow: 1000 mount/unmount cycles under a long-lived owner and signal keep
// the heap flat. Fails on reactive_graph 0.2.16 without leptos-rs/leptos#4966.

use reactive_graph::{
    computed::Memo,
    owner::Owner,
    signal::RwSignal,
    traits::{Get, GetUntracked, Set},
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicIsize, Ordering::Relaxed},
};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

// SAFETY: forwards every call to `System` unchanged and only counts sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size() as isize, Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn mount_and_unmount(parent: &Owner, signal: RwSignal<u32>, times: usize) {
    parent.with(|| {
        for _ in 0..times {
            let row = Owner::new();
            row.with(|| {
                let memo = Memo::new(move |_| signal.get() + 1);
                assert_eq!(memo.get(), signal.get_untracked() + 1);
            });
            row.cleanup();
            drop(row);
            signal.set(signal.get_untracked() + 1);
        }
    });
}

#[test]
fn a_thousand_mount_and_unmount_cycles_keep_memory_flat() {
    let parent = Owner::new();
    let signal = parent.with(|| RwSignal::new(0u32));
    mount_and_unmount(&parent, signal, 1000);
    let warm = LIVE.load(Relaxed);
    mount_and_unmount(&parent, signal, 1000);
    let grown = LIVE.load(Relaxed) - warm;
    assert!(
        grown < 64 * 1024,
        "1000 more cycles kept {grown} bytes (reactive_graph 0.2.16 keeps about 450 per cycle)"
    );
}
