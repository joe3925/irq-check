#![no_std]
#![no_main]

use core::fmt::{self, Write};

#[cfg(not(feature = "large"))]
const TEXT: [u8; 64] = [b'x'; 64];
#[cfg(feature = "large")]
const TEXT: [u8; 1024] = [b'x'; 1024];

struct Writer {
    used: usize,
}

#[cfg_attr(irq_check, irq::forbidden)]
fn forbidden_write(value: u8) {
    core::hint::black_box(value);
}

impl Write for Writer {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            #[cfg(feature = "negative")]
            forbidden_write(byte);
            core::hint::black_box(byte);
            self.used = self.used.wrapping_add(1);
        }
        Ok(())
    }
}

#[cfg_attr(irq_check, irq::context)]
#[unsafe(no_mangle)]
pub fn irq_entry() -> usize {
    let mut writer = Writer { used: 0 };
    let text = unsafe { core::str::from_utf8_unchecked(&TEXT) };
    let _ = write!(&mut writer as &mut dyn Write, "{text}");
    writer.used
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    core::hint::black_box(irq_entry());
    loop { core::hint::spin_loop(); }
}

#[panic_handler]
fn panic_handler(_: &core::panic::PanicInfo<'_>) -> ! {
    loop { core::hint::spin_loop(); }
}
