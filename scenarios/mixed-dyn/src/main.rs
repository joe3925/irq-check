#![no_std]
#![no_main]

#[panic_handler]
fn panic_handler(_: &core::panic::PanicInfo<'_>) -> ! {
    loop { core::hint::spin_loop(); }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    core::hint::black_box(irq_entry());
    core::hint::black_box(irq_entry());
    core::hint::black_box(ordinary_entry());
    loop { core::hint::spin_loop(); }
}

pub trait Service {
    fn call(&self) -> usize;
}

pub struct Safe;
pub struct Bad;

impl Service for Safe {
    fn call(&self) -> usize { 7 }
}

impl Service for Bad {
    #[cfg_attr(irq_check, irq::forbidden)]
    fn call(&self) -> usize { 11 }
}

#[inline(never)]
pub fn shared(value: &dyn Service) -> usize { value.call() }

#[unsafe(no_mangle)]
pub fn ordinary_entry() -> usize { shared(&Bad) }

#[cfg_attr(irq_check, irq::context)]
#[unsafe(no_mangle)]
pub fn irq_entry() -> usize {
    #[cfg(feature = "negative")]
    let value: &dyn Service = &Bad;
    #[cfg(not(feature = "negative"))]
    let value: &dyn Service = &Safe;
    shared(value)
}
