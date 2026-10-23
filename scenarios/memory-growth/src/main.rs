#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

#[repr(align(64))]
struct Arena(UnsafeCell<[u8; 128]>);

unsafe impl Sync for Arena {}

struct Allocator;

static ARENA: Arena = Arena(UnsafeCell::new([0; 128]));
static NEXT: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() > 64 || layout.size() > 64 {
            return core::ptr::null_mut();
        }
        match NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |offset| {
            (offset < 128).then_some(offset + 64)
        }) {
            Ok(offset) => unsafe { ARENA.0.get().cast::<u8>().add(offset) },
            Err(_) => core::ptr::null_mut(),
        }
    }

    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {}
}

struct Registry {
    head: UnsafeCell<*mut Record>,
}

unsafe impl Sync for Registry {}

static REGISTRY: Registry = Registry {
    head: UnsafeCell::new(core::ptr::null_mut()),
};

struct Record {
    previous: *mut Record,
    anchor: &'static (),
}

static ANCHOR: () = ();

fn register(previous: *mut Record, anchor: &'static ()) -> *mut Record {
    let record = Box::into_raw(Box::new(Record { previous, anchor }));
    unsafe { REGISTRY.head.get().write(record); }
    let exposed = context_growth_boundary::expose(record);
    unsafe { core::ptr::addr_of_mut!((*exposed).previous).write(previous); }
    record
}

#[cfg_attr(irq_check, irq::context)]
#[unsafe(no_mangle)]
pub fn irq_entry() -> usize {
    let mut value = 0usize;
    let mut pointer = unsafe { *REGISTRY.head.get() };
    while !pointer.is_null() {
        value = value.wrapping_add(1);
        pointer = unsafe { (*pointer).previous };
    }
    value
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let previous = register(core::ptr::null_mut(), &ANCHOR);
    register(previous, &ANCHOR);
    irq_entry();
    irq_entry();
    loop { core::hint::spin_loop(); }
}

#[panic_handler]
fn panic_handler(_: &core::panic::PanicInfo<'_>) -> ! {
    loop { core::hint::spin_loop(); }
}
