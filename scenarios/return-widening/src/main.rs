#![no_std]
#![no_main]

use core::sync::atomic::{AtomicUsize, Ordering};

static VALUE: AtomicUsize = AtomicUsize::new(0);

#[inline(never)]
fn scalar(value: usize) -> usize { value ^ 1 }

#[inline(never)]
fn read_value() -> usize {
    let value = VALUE.load(Ordering::Relaxed);
    scalar(value) ^ scalar(value)
}

fn safe() -> usize { 0 }

#[cfg_attr(irq_check, irq::forbidden)]
fn forbidden_return() -> usize { 1 }

#[cfg_attr(irq_check, irq::context)]
#[unsafe(no_mangle)]
pub fn irq_entry() -> usize {
    #[cfg(not(feature = "negative"))]
    let callback: fn() -> usize = safe;
    #[cfg(feature = "negative")]
    let callback: fn() -> usize = forbidden_return;
    read_value() ^ core::hint::black_box(callback)()
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    core::hint::black_box(irq_entry());
    macro_rules! changes {
        ($($value:expr),*) => { $(
            VALUE.store($value, Ordering::Relaxed);
            core::hint::black_box(irq_entry());
        )* };
    }
    changes!(1, 2, 3, 4, 5, 6, 7, 8, 9);
    loop { core::hint::spin_loop(); }
}

#[panic_handler]
fn panic_handler(_: &core::panic::PanicInfo<'_>) -> ! {
    loop { core::hint::spin_loop(); }
}
