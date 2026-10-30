#![no_std]
#![no_main]

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::Mutex;

type MetaSinkFn = fn(&[u8]);

static META_SINK: Mutex<Option<MetaSinkFn>> = Mutex::new(None);
static HOST_HELLO_RECEIVED: AtomicBool = AtomicBool::new(false);
static WRITTEN: AtomicUsize = AtomicUsize::new(0);

#[cfg(feature = "opaque-init")]
unsafe extern "C" {
    fn initialize_device() -> usize;
}

fn register_sink(sink: MetaSinkFn) {
    *META_SINK.lock() = Some(sink);
}

fn sink_write(bytes: &[u8]) {
    if !HOST_HELLO_RECEIVED.load(Ordering::Acquire) {
        return;
    }
    #[cfg(not(feature = "try-lock"))]
    let sink = *META_SINK.lock();
    #[cfg(feature = "try-lock")]
    let sink = match META_SINK.try_lock() {
        Some(guard) => *guard,
        None => return,
    };
    if let Some(sink) = sink {
        sink(bytes);
    }
}

fn emit(bytes: &[u8]) {
    WRITTEN.fetch_add(bytes.len(), Ordering::Relaxed);
}

#[cfg_attr(irq_check, irq::context)]
#[unsafe(no_mangle)]
pub fn irq_entry() {
    sink_write(b"module begin");
    sink_write(b"module end");
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    #[cfg(feature = "opaque-init")]
    unsafe {
        core::hint::black_box(initialize_device());
        core::hint::black_box(initialize_device());
    }
    register_sink(emit);
    register_sink(emit);
    HOST_HELLO_RECEIVED.store(true, Ordering::Release);
    irq_entry();
    irq_entry();
    loop { core::hint::spin_loop(); }
}

#[panic_handler]
fn panic_handler(_: &core::panic::PanicInfo<'_>) -> ! {
    loop { core::hint::spin_loop(); }
}
