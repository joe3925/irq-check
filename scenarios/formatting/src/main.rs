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
    loop { core::hint::spin_loop(); }
}

use core::fmt::{self, Write};

pub struct Buffer { bytes: [u8; 512], used: usize }

impl Write for Buffer {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            if self.used == self.bytes.len() { return Err(fmt::Error); }
            self.bytes[self.used] = byte;
            self.used += 1;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct StartupError { cpu: u16, reason: Reason }

#[derive(Debug)]
struct Reason { code: u8, label: &'static str }

struct Label(&'static str);

#[cfg_attr(irq_check, irq::forbidden)]
fn forbidden_format(value: &str) -> usize { value.len() }

impl fmt::Display for Label {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[cfg(feature = "negative")]
        core::hint::black_box(forbidden_format(self.0));
        formatter.pad(self.0)
    }
}

#[inline(never)]
fn panic_message(writer: &mut dyn Write, args: fmt::Arguments<'_>) -> fmt::Result {
    writer.write_fmt(args)
}

#[inline(never)]
fn panic_common(writer: &mut dyn Write, error: &StartupError) -> fmt::Result {
    panic_message(writer, format_args!("early kernel panic: {error:#?}\n"))?;
    panic_message(writer, format_args!("{:>12.5} cpu={:04} code={:#x} {}\n", Label(error.reason.label), error.cpu, error.reason.code, true))
}

#[cfg_attr(irq_check, irq::context)]
#[unsafe(no_mangle)]
pub fn irq_entry() -> usize {
    let mut writer = Buffer { bytes: [0; 512], used: 0 };
    let error = StartupError { cpu: 2, reason: Reason { code: 9, label: "startup" } };
    let _ = panic_common(&mut writer, &error);
    writer.used
}
