#![no_std]
#![no_main]

#[cfg(feature = "heap-registry")]
extern crate alloc;

#[cfg(feature = "heap-registry")]
mod heap {
    use core::alloc::{GlobalAlloc, Layout};
    use core::cell::UnsafeCell;
    use core::sync::atomic::{AtomicUsize, Ordering};

    #[repr(align(64))]
    struct Arena(UnsafeCell<[u8; 65536]>);
    unsafe impl Sync for Arena {}
    static ARENA: Arena = Arena(UnsafeCell::new([0; 65536]));
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Allocator;
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
}

trait Device: Sync {
    fn service(&self) -> usize;
}

struct Ready;
struct Forbidden;

#[cfg(feature = "registry")]
struct Registry {
    entries: &'static [(usize, &'static dyn Device)],
}

#[cfg(all(feature = "registry", not(feature = "global-registry")))]
static DOMAINS: Registry = Registry {
    entries: &[(0, &Ready), (1, &Forbidden)],
};

#[cfg(feature = "global-registry")]
struct GlobalRegistry(core::cell::UnsafeCell<Registry>);

#[cfg(feature = "global-registry")]
unsafe impl Sync for GlobalRegistry {}

#[cfg(feature = "global-registry")]
static DOMAINS: GlobalRegistry = GlobalRegistry(core::cell::UnsafeCell::new(Registry {
    entries: &[(0, &Ready), (1, &Forbidden)],
}));

#[cfg(feature = "global-registry")]
fn initialize_registry() {
    #[cfg(feature = "heap-registry")]
    let entries: &'static [(usize, &'static dyn Device)] = {
        use alloc::boxed::Box;
        Box::leak(Box::new([
            (0, Box::leak(Box::new(Ready)) as &dyn Device),
            (1, Box::leak(Box::new(Forbidden)) as &dyn Device),
        ]))
    };
    #[cfg(not(feature = "heap-registry"))]
    let entries: &'static [(usize, &'static dyn Device)] = &[(0, &Ready), (1, &Forbidden)];
    unsafe {
        *DOMAINS.0.get() = Registry { entries };
    }
}

#[cfg(feature = "registry")]
fn registry_wake(registry: &Registry, remaining: usize) {
    #[cfg(feature = "global-update")]
    if remaining == 1 {
        #[cfg(feature = "negative")]
        let entries: &'static [(usize, &'static dyn Device)] = &[(0, &Forbidden)];
        #[cfg(not(feature = "negative"))]
        let entries: &'static [(usize, &'static dyn Device)] = &[(0, &Ready)];
        unsafe {
            *DOMAINS.0.get() = Registry { entries };
        }
    }
    #[cfg(feature = "global-registry")]
    let selected_registry = unsafe { &*DOMAINS.0.get() };
    #[cfg(not(feature = "global-registry"))]
    let selected_registry = registry;
    #[cfg(any(
        not(feature = "negative"),
        feature = "global-update",
        feature = "registry-predicate"
    ))]
    let id = 0;
    #[cfg(all(
        feature = "negative",
        not(any(feature = "global-update", feature = "registry-predicate"))
    ))]
    let id = 1;
    let device = selected_registry
        .entries
        .iter()
        .find(|entry| {
            #[cfg(all(feature = "registry-predicate", feature = "negative"))]
            core::hint::black_box(Forbidden.service());
            entry.0 == id
        })
        .map(|entry| entry.1)
        .unwrap();
    core::hint::black_box(device.service());
    if remaining != 0 {
        registry_wake(registry, remaining - 1);
    }
}

#[cfg(feature = "scalar-growth")]
fn scalar_wake(device: &dyn Device, remaining: usize, pc: usize, frames: [usize; 8]) {
    #[cfg(feature = "trace-loop")]
    {
        let mut trace = [pc; 64];
        let mut depth = 1;
        while depth < trace.len() {
            let current = trace[depth - 1];
            let step = Some(device).map(|device| {
                if current & 15 == 0 {
                    core::hint::black_box(device.service());
                }
                current.checked_add(16)
            });
            let Some(Some(next)) = step else { break };
            trace[depth] = next;
            depth += 1;
        }
        core::hint::black_box(trace);
    }
    #[cfg(feature = "formatting")]
    {
        use core::fmt::Write;
        #[cfg(not(feature = "multi-format"))]
        let _ = write!(&mut Sink, "{}", pc);
        #[cfg(feature = "multi-format")]
        let _ = write!(&mut Sink, "{:#X} {}", pc, frames[0]);
    }
    core::hint::black_box(frames);
    core::hint::black_box(device.service());
    if remaining != 0 {
        scalar_wake(device, remaining - 1, pc.wrapping_add(16), [pc; 8]);
    }
}

impl Device for Ready {
    fn service(&self) -> usize {
        core::hint::black_box(7)
    }
}

impl Device for Forbidden {
    #[cfg_attr(irq_check, irq::forbidden)]
    fn service(&self) -> usize {
        core::hint::black_box(11)
    }
}

struct Task<'a> {
    device: &'a dyn Device,
    remaining: usize,
}

#[cfg(all(feature = "formatting", not(feature = "panic-format")))]
struct Sink;

#[cfg(feature = "panic-format")]
struct Sink {
    in_panic: bool,
    device: &'static dyn Device,
}

#[cfg(feature = "formatting")]
impl core::fmt::Write for Sink {
    fn write_str(&mut self, value: &str) -> core::fmt::Result {
        core::hint::black_box(value);
        #[cfg(feature = "closure-write")]
        for byte in core::hint::black_box([1u8, 2u8]) {
            core::hint::black_box(byte);
        }
        #[cfg(feature = "panic-format")]
        {
            if self.in_panic {
                core::hint::black_box(self.device.service());
            }
            use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
            static BUFFER: [AtomicU8; 16] = [const { AtomicU8::new(0) }; 16];
            static CURSOR: AtomicUsize = AtomicUsize::new(16);
            for byte in value.bytes() {
                let index = CURSOR.fetch_add(1, Ordering::Relaxed);
                BUFFER[index].store(byte, Ordering::Relaxed);
            }
        }
        #[cfg(feature = "buffered-format")]
        {
            use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
            static BUFFER: [AtomicU8; 16] = [const { AtomicU8::new(0) }; 16];
            static CURSOR: AtomicUsize = AtomicUsize::new(0);
            for byte in value.bytes() {
                let index = CURSOR.fetch_add(1, Ordering::Relaxed) % BUFFER.len();
                BUFFER[index].store(byte, Ordering::Relaxed);
            }
        }
        #[cfg(feature = "local-callback")]
        {
            let binding = core::cell::Cell::new(&Ready as &dyn Device);
            #[cfg(feature = "negative")]
            binding.set(&Forbidden);
            #[cfg(not(feature = "negative"))]
            binding.set(&Ready);
            core::hint::black_box(binding.get().service());
        }
        Ok(())
    }
}

#[cfg(feature = "formatting")]
impl core::fmt::Display for Task<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        #[cfg(feature = "panic-format")]
        { formatter.write_str("task") }
        #[cfg(not(feature = "panic-format"))]
        { write!(formatter, "{}", self.device.service()) }
    }
}

impl Drop for Task<'_> {
    fn drop(&mut self) {
        #[cfg(feature = "cut")]
        if self.remaining == 0 {
            #[cfg(not(feature = "escaping-cut"))]
            #[cfg_attr(irq_check, irq::unreachable(unsafe))]
            unsafe {
                loop {
                    core::hint::spin_loop();
                }
            }
            #[cfg(feature = "escaping-cut")]
            #[cfg_attr(irq_check, irq::unreachable(unsafe))]
            unsafe {
                'exit: {
                    break 'exit;
                    loop {
                        core::hint::spin_loop();
                    }
                }
            }
        }
        if self.remaining != 0 {
            #[cfg(all(feature = "negative", not(any(feature = "local-callback", feature = "panic-format"))))]
            let next: &dyn Device = if self.remaining == 1 {
                &Forbidden
            } else {
                self.device
            };
            #[cfg(any(not(feature = "negative"), feature = "local-callback", feature = "panic-format"))]
            let next = self.device;
            #[cfg(not(feature = "stable-cycle"))]
            wake(next, self.remaining - 1);
            #[cfg(feature = "stable-cycle")]
            wake(next, self.remaining);
        }
        #[cfg(feature = "formatting")]
        {
            use core::fmt::Write;
            #[cfg(not(feature = "panic-format"))]
            let mut sink = Sink;
            #[cfg(feature = "panic-format")]
            let mut sink = Sink { in_panic: false, device: &Ready };
            let _ = write!(&mut sink, "{}", self);
        }
        #[cfg(not(feature = "formatting"))]
        core::hint::black_box(self.device.service());
    }
}

#[inline(never)]
fn wake(device: &dyn Device, remaining: usize) {
    drop(Task { device, remaining });
}

#[cfg(feature = "erased")]
fn erased_wake(binding: *const (), remaining: usize) {
    if remaining != 0 {
        #[cfg(feature = "negative")]
        let replacement = &Forbidden as &dyn Device;
        #[cfg(feature = "negative")]
        let next = if remaining == 1 {
            &replacement as *const &dyn Device as *const ()
        } else {
            binding
        };
        #[cfg(not(feature = "negative"))]
        let next = binding;
        erased_wake(next, remaining - 1);
    }
    core::hint::black_box(unsafe { &*(binding as *const &dyn Device) }.service());
}

#[cfg(feature = "mutable")]
fn mutable_wake(binding: &core::cell::Cell<&dyn Device>, remaining: usize) {
    if remaining == 1 {
        #[cfg(feature = "negative")]
        binding.set(&Forbidden);
        #[cfg(not(feature = "negative"))]
        binding.set(&Ready);
    }
    if remaining != 0 {
        mutable_wake(binding, remaining - 1);
    }
    core::hint::black_box(binding.get().service());
}

#[cfg_attr(irq_check, irq::context)]
#[unsafe(no_mangle)]
pub fn irq_entry(input: *const (), initial: usize) {
    core::hint::black_box(input);
    #[cfg(feature = "coverage-local")]
    {
        let mut device: &dyn Device = &Ready;
        #[cfg(feature = "coverage-write")]
        {
            let address = &raw mut device as usize;
            let pointer = address as *mut &dyn Device;
            #[cfg(feature = "negative")]
            unsafe {
                *pointer = &Forbidden;
            }
            #[cfg(not(feature = "negative"))]
            unsafe {
                *pointer = &Ready;
            }
        }
        #[cfg(all(feature = "negative", not(feature = "coverage-write")))]
        {
            device = &Forbidden;
        }
        #[cfg(feature = "coverage-unknown")]
        {
            device = unsafe { *(input as *const &dyn Device) };
        }
        core::hint::black_box(device.service());
    }
    #[cfg(all(feature = "registry", not(feature = "global-registry")))]
    registry_wake(&DOMAINS, 4);
    #[cfg(feature = "global-registry")]
    {
        #[cfg(not(feature = "boot-registry"))]
        initialize_registry();
        registry_wake(&Registry { entries: &[] }, 4);
    }
    #[cfg(feature = "scalar-growth")]
    {
        #[cfg(not(feature = "negative"))]
        let device: &dyn Device = &Ready;
        #[cfg(feature = "negative")]
        let device: &dyn Device = &Forbidden;
        scalar_wake(device, 512, 4096, [0; 8]);
    }
    #[cfg(feature = "mutable")]
    mutable_wake(&core::cell::Cell::new(&Ready as &dyn Device), 4);
    #[cfg(feature = "erased")]
    {
        let binding = &Ready as &dyn Device;
        erased_wake(&binding as *const &dyn Device as *const (), 4);
    }
    #[cfg(not(feature = "unbounded"))]
    core::hint::black_box(initial);
    #[cfg(not(any(
        feature = "unbounded",
        feature = "erased",
        feature = "mutable",
        feature = "registry",
        feature = "scalar-growth",
        feature = "coverage-local"
    )))]
    wake(&Ready, 3);
    #[cfg(all(
        feature = "unbounded",
        not(any(feature = "erased", feature = "mutable"))
    ))]
    wake(&Ready, initial);
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    #[cfg(feature = "boot-registry")]
    initialize_registry();
    irq_entry(core::ptr::null(), 3);
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic_handler(info: &core::panic::PanicInfo<'_>) -> ! {
    #[cfg(feature = "panic-format")]
    {
        use core::fmt::Write;
        #[cfg(feature = "negative")]
        let device: &'static dyn Device = &Forbidden;
        #[cfg(not(feature = "negative"))]
        let device: &'static dyn Device = &Ready;
        let mut sink = Sink { in_panic: true, device };
        let _ = write!(&mut sink, "{}", info);
    }
    #[cfg(not(feature = "panic-format"))]
    core::hint::black_box(info);
    loop {
        core::hint::spin_loop();
    }
}
