//! The common string `format` checks allocate nothing when the value passes. A global allocator counts the
//! allocations. It is its own test binary because the allocator is process-wide.

#![expect(
    unsafe_code,
    reason = "a global allocator is an unsafe trait; this one only counts calls, then calls `System`"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use honig::__private::{InstancePath, check_format};

/// Allocations of this thread. The test harness allocates on threads of its own.
struct Counting;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

// SAFETY: every call goes to `System`; the counter only observes calls.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.with(|n| n.set(n.get() + 1));
        // SAFETY: the caller's contract, passed on.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract, passed on.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

#[test]
fn a_common_format_allocates_nothing() {
    let mut out = Vec::new();
    for (format, value) in [
        ("email", "joe.bloggs@example.com"),
        ("email", "Joe+tag@Mail.Example-1.COM"),
        ("hostname", "api.eu-west-1.example.com"),
        ("hostname", "localhost"),
        ("idn-hostname", "api.example.com"),
        ("date-time", "2026-11-12T10:28:00.123Z"),
        ("date-time", "2026-11-12t10:28:00+01:00"),
        ("time", "23:59:60Z"),
        ("ipv4", "192.168.0.1"),
        ("ipv4-cidr", "10.0.0.0/8"),
        ("duration", "P1Y2M3DT4H5M6S"),
        ("duration", "p2w"),
    ] {
        let before = ALLOCATIONS.with(Cell::get);
        check_format(&mut out, &InstancePath::Root, value, format);
        assert_eq!(ALLOCATIONS.with(Cell::get) - before, 0, "{format}: {value}");
    }
    assert!(out.is_empty(), "{out:?}");
}
