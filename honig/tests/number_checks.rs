//! The number checks that generated code runs on every value allocate nothing when the value passes. A global
//! allocator counts the allocations. It is its own test binary because the allocator is process-wide.

#![expect(
    unsafe_code,
    reason = "a global allocator is an unsafe trait; this one only counts calls, then calls `System`"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use honig::__private::{Bounds, InstanceLocation, check_decimal_range, check_multiple, check_multiple_text};

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

/// The allocations that `run` makes on this thread.
fn allocations(run: impl FnOnce()) -> usize {
    let before = ALLOCATIONS.with(Cell::get);
    run();
    ALLOCATIONS.with(Cell::get) - before
}

#[test]
fn a_multiple_allocates_nothing() {
    let mut out = Vec::new();
    for (value, of) in [(19.99, "0.01"), (1e300, "1e299"), (5e-324, "5e-324"), (-0.0, "0.5"), (2.5, "0.5")] {
        assert_eq!(allocations(|| check_multiple(&mut out, &InstanceLocation::Root, value, of)), 0, "{value} of {of}");
    }
    assert_eq!(allocations(|| check_multiple(&mut out, &InstanceLocation::Root, i64::MIN, "2")), 0);
    assert_eq!(allocations(|| check_multiple(&mut out, &InstanceLocation::Root, u64::MAX, "5")), 0);
    assert_eq!(allocations(|| check_multiple_text(&mut out, &InstanceLocation::Root, "12.340", "0.01")), 0);
    assert!(out.is_empty(), "{out:?}");
}

#[test]
fn a_decimal_range_allocates_nothing() {
    let mut out = Vec::new();
    let bounds = Bounds { min: Some("0.01"), max: Some("1e6"), exclusive_max: Some("1000000.5"), ..Bounds::NONE };
    assert_eq!(allocations(|| check_decimal_range(&mut out, &InstanceLocation::Root, "12345.678", bounds)), 0);
    assert!(out.is_empty(), "{out:?}");
}

/// A value off by the last digit is still refused: the text that the check parses is the whole value.
#[test]
fn a_value_that_is_no_multiple_fails() {
    let mut out = Vec::new();
    check_multiple(&mut out, &InstanceLocation::Root, 19.995, "0.01");
    check_multiple(&mut out, &InstanceLocation::Root, 0.1 + 0.2, "0.1");
    check_multiple(&mut out, &InstanceLocation::Root, i64::MAX, "2");
    check_multiple(&mut out, &InstanceLocation::Root, 1e300 + 1e284, "1e300");
    assert_eq!(out.len(), 4, "{out:?}");
}
