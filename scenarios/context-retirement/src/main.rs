#![no_std]
#![no_main]

use core::sync::atomic::{AtomicUsize, Ordering};

static VALUE: AtomicUsize = AtomicUsize::new(0);

#[inline(never)]
fn scalar(value: usize) -> usize { value ^ 1 }

#[inline(never)]
#[cfg_attr(all(irq_check, feature = "negative"), irq::forbidden)]
fn shared() -> usize {
    let value = VALUE.load(Ordering::Relaxed);
    scalar(value) ^ scalar(value)
}

#[inline(never)]
fn leaf<const A: usize, const B: usize>() {
    VALUE.store(A * SIDE + B, Ordering::Relaxed);
    let callback: fn() -> usize = core::hint::black_box(shared);
    core::hint::black_box((A, B, callback()));
}

#[cfg(not(feature = "large"))]
const SIDE: usize = 8;
#[cfg(feature = "large")]
const SIDE: usize = 64;

macro_rules! row {
    ($a:expr; $($b:expr),*) => { [$(leaf::<$a, $b> as fn()),*] };
}

#[cfg(not(feature = "large"))]
macro_rules! rows {
    ($($a:expr),*) => { [$(row!($a; 0, 1, 2, 3, 4, 5, 6, 7)),*] };
}
#[cfg(feature = "large")]
macro_rules! rows {
    ($($a:expr),*) => { [$(row!($a; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63)),*] };
}

#[cfg(not(feature = "large"))]
const HANDLERS: [[fn(); SIDE]; SIDE] = rows!(0, 1, 2, 3, 4, 5, 6, 7);
#[cfg(feature = "large")]
const HANDLERS: [[fn(); SIDE]; SIDE] = rows!(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63);

#[cfg_attr(irq_check, irq::context)]
#[unsafe(no_mangle)]
pub fn irq_entry(index: usize) {
    HANDLERS[index % SIDE][index / SIDE % SIDE]();
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    irq_entry(0);
    irq_entry(1);
    loop { core::hint::spin_loop(); }
}

#[panic_handler]
fn panic_handler(_: &core::panic::PanicInfo<'_>) -> ! {
    loop { core::hint::spin_loop(); }
}
