#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[repr(align(64))]
struct Arena(UnsafeCell<[u8; 65536]>);

unsafe impl Sync for Arena {}

struct Allocator;

static ARENA: Arena = Arena(UnsafeCell::new([0; 65536]));
static NEXT: AtomicUsize = AtomicUsize::new(0);

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

struct Registry {
    ready: AtomicBool,
    head: UnsafeCell<*mut Record>,
}

unsafe impl Sync for Registry {}

static REGISTRY: Registry = Registry {
    ready: AtomicBool::new(false),
    head: UnsafeCell::new(core::ptr::null_mut()),
};

struct Record {
    previous: *mut Record,
    callback: fn(&dyn Device, &mut dyn Write, usize) -> fmt::Result,
    device: &'static dyn Device,
    code: usize,
}

trait Device: Sync {
    fn service(&self, writer: &mut dyn Write, code: usize) -> fmt::Result;
}

struct Serial;
struct Timer;

static SERIAL: Serial = Serial;
static TIMER: Timer = Timer;

impl Device for Serial {
    fn service(&self, writer: &mut dyn Write, code: usize) -> fmt::Result {
        #[cfg(feature = "formatting")]
        return writeln!(writer, "serial irq={code:#x} ready={}", REGISTRY.ready.load(Ordering::Acquire));
        #[cfg(not(feature = "formatting"))]
        {
            core::hint::black_box(code);
            writer.write_str("serial")
        }
    }
}

impl Device for Timer {
    fn service(&self, writer: &mut dyn Write, code: usize) -> fmt::Result {
        #[cfg(feature = "formatting")]
        return writeln!(writer, "timer irq={code:04} ready={:?}", REGISTRY.ready.load(Ordering::Acquire));
        #[cfg(not(feature = "formatting"))]
        {
            core::hint::black_box(code);
            writer.write_str("timer")
        }
    }
}

struct PanicWriter {
    used: usize,
}

impl Write for PanicWriter {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            core::hint::black_box(byte);
            self.used = self.used.wrapping_add(1);
        }
        Ok(())
    }
}

#[inline(never)]
fn callback(device: &dyn Device, writer: &mut dyn Write, code: usize) -> fmt::Result {
    device.service(writer, code)
}

#[inline(never)]
fn register(previous: *mut Record, device: &'static dyn Device, code: usize) -> *mut Record {
    let record = Box::into_raw(Box::new(Record { previous, callback, device, code }));
    unsafe { REGISTRY.head.get().write(record); }
    #[cfg(feature = "escape")]
    {
        let exposed = context_growth_boundary::expose(record);
        unsafe { core::ptr::addr_of_mut!((*exposed).previous).write(previous); }
    }
    record
}

#[cfg_attr(irq_check, irq::context)]
#[unsafe(no_mangle)]
pub fn irq_entry() -> usize {
    let mut writer = PanicWriter { used: 0 };
    core::hint::black_box(REGISTRY.ready.load(Ordering::Acquire));
    #[cfg(feature = "dispatch")]
    {
        let mut pointer = unsafe { *REGISTRY.head.get() };
        while !pointer.is_null() {
            let record = unsafe { &*pointer };
            let _ = (record.callback)(record.device, &mut writer, record.code);
            pointer = record.previous;
        }
    }
    #[cfg(not(feature = "dispatch"))]
    {
        let _ = callback(&SERIAL, &mut writer, 0);
        let _ = callback(&TIMER, &mut writer, 1);
    }
    writer.used
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let previous = register(core::ptr::null_mut(), &SERIAL, 0);
    let previous = register(previous, &TIMER, 1);
    core::hint::black_box(irq_entry());
    core::hint::black_box(irq_entry());
    core::hint::black_box(previous);
    loop { core::hint::spin_loop(); }
}

#[panic_handler]
fn panic_handler(info: &core::panic::PanicInfo<'_>) -> ! {
    #[cfg(feature = "formatting")]
    {
        let mut writer = PanicWriter { used: 0 };
        let writer: &mut dyn Write = &mut writer;
        let _ = writeln!(writer, "early kernel panic: {info}");
    }
    #[cfg(not(feature = "formatting"))]
    core::hint::black_box(info);
    loop { core::hint::spin_loop(); }
}
