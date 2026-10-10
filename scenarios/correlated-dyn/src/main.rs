#![no_std]
#![no_main]

#[panic_handler]
fn panic_handler(_: &core::panic::PanicInfo<'_>) -> ! {
    loop { core::hint::spin_loop(); }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    core::hint::black_box(irq_entry(true));
    core::hint::black_box(irq_entry(false));
    loop { core::hint::spin_loop(); }
}

trait Service {
    fn call(&self) -> usize;
}

struct Safe;
struct Bad;

impl Service for Safe {
    fn call(&self) -> usize { 3 }
}

impl Service for Bad {
    #[cfg_attr(irq_check, irq::forbidden)]
    fn call(&self) -> usize { 5 }
}

#[cfg_attr(irq_check, irq::context)]
#[unsafe(no_mangle)]
pub fn irq_entry(input: bool) -> usize {
    let (receiver, allowed): (&dyn Service, bool) = if input {
        (&Safe, true)
    } else {
        (&Bad, false)
    };
    #[cfg(feature = "negative")]
    let invoke = !allowed;
    #[cfg(not(feature = "negative"))]
    let invoke = allowed;
    if invoke { receiver.call() } else { 0 }
}
