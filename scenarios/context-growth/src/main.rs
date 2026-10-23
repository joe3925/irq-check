#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[repr(align(64))]
struct Arena(UnsafeCell<[u8; 65536]>);

unsafe impl Sync for Arena {}

struct Allocator;

static ARENA: Arena = Arena(UnsafeCell::new([0; 65536]));
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Registry {
    flag: AtomicBool,
    head: UnsafeCell<*mut Record>,
}

unsafe impl Sync for Registry {}

static REGISTRY: Registry = Registry {
    flag: AtomicBool::new(false),
    head: UnsafeCell::new(core::ptr::null_mut()),
};

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() > 64 {
            return core::ptr::null_mut();
        }
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

#[repr(C)]
struct Record {
    previous: *mut Record,
    callback: fn(bool) -> usize,
    words: [usize; 8],
}

fn safe_callback(value: bool) -> usize {
    usize::from(value)
}

#[cfg_attr(irq_check, irq::forbidden)]
fn forbidden_growth(value: bool) -> usize {
    usize::from(value).wrapping_add(1)
}

#[inline(never)]
fn load_flag() -> bool {
    REGISTRY.flag.load(Ordering::Acquire)
}

#[inline(never)]
fn register(previous: *mut Record, index: usize) -> *mut Record {
    let record = Box::into_raw(Box::new(Record {
        previous,
        callback: safe_callback,
        words: [index; 8],
    }));
    unsafe { REGISTRY.head.get().write(record); }
    let exposed = context_growth_boundary::expose(record);
    unsafe { core::ptr::addr_of_mut!((*exposed).previous).write(previous); }
    record
}

#[cfg_attr(irq_check, irq::context)]
#[unsafe(no_mangle)]
pub fn irq_entry() -> usize {
    #[cfg(feature = "negative")]
    let selected_callback: fn(bool) -> usize = forbidden_growth;
    #[cfg(not(feature = "negative"))]
    let selected_callback: fn(bool) -> usize = safe_callback;
    let value = load_flag();
    let callback = core::hint::black_box(selected_callback);
    callback(value)
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let mut previous = register(core::ptr::null_mut(), 0);
    #[cfg(feature = "chain-8")]
    {
        macro_rules! chain {
            ($($index:expr),*) => {
                $(
                    previous = register(previous, $index);
                    core::hint::black_box(load_flag());
                    core::hint::black_box(irq_entry());
                )*
            };
        }
        chain!(0, 1, 2, 3, 4, 5, 6, 7);
        #[cfg(feature = "chain-16")]
        chain!(8, 9, 10, 11, 12, 13, 14, 15);
        #[cfg(feature = "chain-64")]
        chain!(16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
            32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47,
            48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63);
    }
    #[cfg(not(feature = "chain-8"))]
    for index in 0..16 {
        previous = register(previous, index);
        core::hint::black_box(load_flag());
        core::hint::black_box(irq_entry());
    }
    core::hint::black_box(previous);
    loop { core::hint::spin_loop(); }
}

#[panic_handler]
fn panic_handler(_: &core::panic::PanicInfo<'_>) -> ! {
    loop { core::hint::spin_loop(); }
}
