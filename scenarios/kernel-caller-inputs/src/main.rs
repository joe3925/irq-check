#![no_std]
#![no_main]

use core::fmt::{self, Write};

struct Buffer {
    bytes: [u8; 512],
    used: usize,
}

struct BootWriter;

#[cfg(feature = "opaque-init")]
unsafe extern "C" {
    fn initialize_device() -> usize;
}

impl Write for Buffer {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            if self.used == self.bytes.len() {
                return Err(fmt::Error);
            }
            self.bytes[self.used] = byte;
            self.used += 1;
        }
        Ok(())
    }
}

impl Write for BootWriter {
    #[cfg_attr(irq_check, irq::forbidden)]
    fn write_str(&mut self, text: &str) -> fmt::Result {
        core::hint::black_box(text);
        Ok(())
    }
}

#[inline(never)]
fn emit_module(writer: &mut dyn Write, name: &str) {
    let _ = write!(writer, "module name={name}\n");
}

#[cfg_attr(irq_check, irq::context)]
#[unsafe(no_mangle)]
pub fn irq_entry() -> usize {
    let mut buffer = Buffer { bytes: [0; 512], used: 0 };
    emit_module(&mut buffer, "kernel");
    emit_module(&mut buffer, "module1");
    #[cfg(not(feature = "few-callers"))]
    {
        emit_module(&mut buffer, "module2");
        emit_module(&mut buffer, "module3");
        emit_module(&mut buffer, "module4");
        emit_module(&mut buffer, "module5");
        emit_module(&mut buffer, "module6");
        emit_module(&mut buffer, "module7");
        emit_module(&mut buffer, "module8");
        emit_module(&mut buffer, "module9");
        emit_module(&mut buffer, "module10");
        emit_module(&mut buffer, "module11");
        emit_module(&mut buffer, "module12");
        emit_module(&mut buffer, "module13");
        emit_module(&mut buffer, "module14");
        emit_module(&mut buffer, "module15");
        emit_module(&mut buffer, "module16");
        emit_module(&mut buffer, "module17");
        emit_module(&mut buffer, "module18");
        emit_module(&mut buffer, "module19");
    }
    buffer.used
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    #[cfg(feature = "opaque-init")]
    unsafe {
        core::hint::black_box(initialize_device());
        core::hint::black_box(initialize_device());
    }
    emit_module(&mut BootWriter, "boot");
    emit_module(&mut BootWriter, "loader");
    core::hint::black_box(irq_entry());
    core::hint::black_box(irq_entry());
    loop { core::hint::spin_loop(); }
}

#[panic_handler]
fn panic_handler(_: &core::panic::PanicInfo<'_>) -> ! {
    loop { core::hint::spin_loop(); }
}
