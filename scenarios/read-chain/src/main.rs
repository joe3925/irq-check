#![no_std]
#![no_main]

pub struct Record {
    value: usize,
    callback: fn() -> usize,
}

#[inline(never)]
fn read_value(record: &Record) -> usize { record.value ^ 1 }

#[inline(never)]
fn forward(record: &Record) -> usize {
    read_value(record) ^ read_value(record)
}

fn safe<const N: usize>() -> usize { N }

#[cfg_attr(irq_check, irq::forbidden)]
fn forbidden_leaf() -> usize { 1 }

#[inline(never)]
fn dispatch<const N: usize>() {
    #[cfg(not(feature = "negative"))]
    let callback = safe::<N>;
    #[cfg(feature = "negative")]
    let callback = forbidden_leaf;
    let callback: fn() -> usize = core::hint::black_box(callback);
    let record = Record { value: 7, callback };
    core::hint::black_box(forward(&record));
    core::hint::black_box(forward(&record).wrapping_add((record.callback)()));
}

#[cfg_attr(irq_check, irq::context)]
#[unsafe(no_mangle)]
pub fn irq_entry() {
    macro_rules! calls {
        ($($value:expr),*) => { $(dispatch::<$value>();)* };
    }
    calls!(0, 1, 2, 3);
    #[cfg(feature = "large")]
    calls!(4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
        16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31);
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    irq_entry();
    irq_entry();
    loop { core::hint::spin_loop(); }
}

#[panic_handler]
fn panic_handler(_: &core::panic::PanicInfo<'_>) -> ! {
    loop { core::hint::spin_loop(); }
}
