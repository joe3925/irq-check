#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

#[repr(align(64))]
struct Arena(UnsafeCell<[u8; 65536]>);
unsafe impl Sync for Arena {}
static ARENA: Arena = Arena(UnsafeCell::new([0; 65536]));
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Allocator;
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() > 64 { return core::ptr::null_mut(); }
        let Some(size) = layout.size().checked_add(63).map(|size| size & !63) else {
            return core::ptr::null_mut();
        };
        match NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |offset| {
            offset.checked_add(size).filter(|end| *end <= 65536)
        }) {
            Ok(offset) => unsafe { ARENA.0.get().cast::<u8>().add(offset) },
            Err(_) => core::ptr::null_mut(),
        }
    }
    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {}
}

struct Record {
    value: usize,
    callback: fn(usize) -> usize,
}

fn safe(value: usize) -> usize { value }

#[cfg_attr(irq_check, irq::forbidden)]
fn forbidden_heap(value: usize) -> usize { value }

#[inline(never)]
fn read_value(record: &Record) -> usize { record.value }

#[inline(never)]
fn assign(record: &mut Record, callback: fn(usize) -> usize) {
    record.callback = callback;
}

#[inline(never)]
fn invoke(record: &Record) -> usize { (record.callback)(read_value(record)) }

#[cfg_attr(irq_check, irq::context)]
#[unsafe(no_mangle)]
pub fn irq_entry(value: usize) -> usize {
    let first = Box::into_raw(Box::new(Record { value, callback: safe }));
    let second = Box::into_raw(Box::new(Record { value: value.wrapping_add(1), callback: safe }));
    let first = unsafe { &mut *first };
    let second = unsafe { &mut *second };
    assign(first, safe);
    #[cfg(feature = "negative")]
    assign(first, forbidden_heap);
    #[cfg(not(feature = "negative"))]
    assign(second, forbidden_heap);
    core::hint::black_box(read_value(first));
    core::hint::black_box(read_value(second));
    invoke(first).wrapping_add(invoke(first))
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    macro_rules! calls {
        ($($value:expr),*) => { $(core::hint::black_box(irq_entry($value));)* };
    }
    calls!(0, 1, 2, 3);
    #[cfg(feature = "large")]
    calls!(4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
        16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31);
    loop { core::hint::spin_loop(); }
}

#[panic_handler]
fn panic_handler(_: &core::panic::PanicInfo<'_>) -> ! {
    loop { core::hint::spin_loop(); }
}
