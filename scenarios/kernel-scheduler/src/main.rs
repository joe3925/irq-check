#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use core::alloc::{GlobalAlloc, Layout};
#[cfg(not(feature = "native-alloc"))]
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::Once;

struct Allocator;

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

#[cfg(not(feature = "native-alloc"))]
#[repr(align(64))]
struct Arena(UnsafeCell<[u8; 65536]>);

#[cfg(not(feature = "native-alloc"))]
unsafe impl Sync for Arena {}

#[cfg(not(feature = "native-alloc"))]
static ARENA: Arena = Arena(UnsafeCell::new([0; 65536]));

#[cfg(not(feature = "native-alloc"))]
static NEXT: AtomicUsize = AtomicUsize::new(0);

#[cfg(feature = "native-alloc")]
unsafe extern "C" {
    fn mi_malloc_aligned(size: usize, alignment: usize) -> *mut u8;
    fn mi_free(pointer: *mut u8);
}

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        #[cfg(feature = "native-alloc")]
        { unsafe { mi_malloc_aligned(layout.size(), layout.align()) } }
        #[cfg(not(feature = "native-alloc"))]
        {
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
    }

    unsafe fn dealloc(&self, pointer: *mut u8, _: Layout) {
        #[cfg(feature = "native-alloc")]
        unsafe { mi_free(pointer) };
        #[cfg(not(feature = "native-alloc"))]
        let _ = pointer;
    }
}

#[cfg_attr(all(irq_check, feature = "trait-context"), irq::context)]
pub trait DomainOps: Send + Sync {
    fn enqueue(&self, task: usize, cpu: usize) -> usize;
    fn contains_cpu(&self, cpu: usize) -> bool;
    fn pick_next(&self, cpu: usize) -> Option<usize>;
    fn on_switch_out(&self, task: usize, cpu: usize);
    fn should_preempt(&self, task: usize) -> bool;
    fn maybe_balance(&self, tick: usize);
}

pub trait SchedulerClass: Send + Sync {
    type CpuState: Send + Sync;

    fn queued<'a>(&self, cpu: &'a Self::CpuState) -> &'a AtomicUsize;
}

struct FifoClass;

impl SchedulerClass for FifoClass {
    type CpuState = AtomicUsize;

    fn queued<'a>(&self, cpu: &'a Self::CpuState) -> &'a AtomicUsize {
        cpu
    }
}

struct Domain<C: SchedulerClass> {
    class: C,
    per_cpu: Box<[Option<C::CpuState>]>,
}

impl<C: SchedulerClass> Domain<C> {
    fn cpu_state(&self, cpu: usize) -> &C::CpuState {
        self.per_cpu.get(cpu).and_then(Option::as_ref)
            .unwrap_or_else(|| panic!("missing domain cpu state {cpu}"))
    }
}

impl<C: SchedulerClass> DomainOps for Domain<C> {
    #[cfg_attr(all(irq_check, feature = "direct-root"), irq::context)]
    fn enqueue(&self, task: usize, cpu: usize) -> usize {
        self.class.queued(self.cpu_state(cpu)).fetch_add(task, Ordering::Relaxed);
        cpu % 2
    }

    #[cfg_attr(all(irq_check, feature = "direct-root"), irq::context)]
    fn contains_cpu(&self, cpu: usize) -> bool {
        cpu < 2
    }

    #[cfg_attr(all(irq_check, feature = "direct-root"), irq::context)]
    fn pick_next(&self, cpu: usize) -> Option<usize> {
        let queued = self.class.queued(self.cpu_state(cpu)).load(Ordering::Relaxed);
        (queued != 0).then_some(queued.wrapping_add(cpu))
    }

    #[cfg_attr(all(irq_check, feature = "direct-root"), irq::context)]
    fn on_switch_out(&self, task: usize, cpu: usize) {
        self.class.queued(self.cpu_state(cpu)).store(task.wrapping_add(cpu), Ordering::Relaxed);
    }

    #[cfg_attr(all(irq_check, feature = "direct-root"), irq::context)]
    fn should_preempt(&self, task: usize) -> bool {
        self.class.queued(self.cpu_state(task % 2)).load(Ordering::Relaxed) != task
    }

    #[cfg_attr(all(irq_check, feature = "direct-root"), irq::context)]
    fn maybe_balance(&self, tick: usize) {
        for cpu in self.per_cpu.iter().flatten() {
            self.class.queued(cpu).fetch_xor(tick & 1, Ordering::Relaxed);
        }
    }
}

struct DomainEntry {
    id: usize,
    ops: Box<dyn DomainOps>,
}

struct DomainMaster {
    domains: Box<[DomainEntry]>,
    per_cpu_cursor: Box<[AtomicUsize]>,
}

impl DomainMaster {
    fn get(&self, id: usize) -> &dyn DomainOps {
        self.domains.iter().find(|domain| domain.id == id)
            .map(|domain| domain.ops.as_ref())
            .unwrap_or_else(|| panic!("unknown scheduler domain {id}"))
    }

    fn enqueue(&self, id: usize, task: usize, cpu: usize) -> usize {
        self.get(id).enqueue(task, cpu)
    }

    fn pick_next(&self, cpu: usize) -> Option<usize> {
        if self.domains.is_empty() {
            return None;
        }
        let cursor = self.per_cpu_cursor.get(cpu).expect("missing domain cursor");
        let start = cursor.load(Ordering::Relaxed) % self.domains.len();
        for offset in 0..self.domains.len() {
            let index = (start + offset) % self.domains.len();
            let domain = &self.domains[index].ops;
            if !domain.contains_cpu(cpu) {
                continue;
            }
            if let Some(task) = domain.pick_next(cpu) {
                cursor.store((index + 1) % self.domains.len(), Ordering::Relaxed);
                return Some(task);
            }
        }
        None
    }
}

struct Scheduler {
    domains: DomainMaster,
}

static SCHEDULER: Once<Scheduler> = Once::new();
static KERNEL_INITIALIZED: AtomicBool = AtomicBool::new(false);

fn scheduler() -> &'static Scheduler {
    SCHEDULER.get().expect("scheduler is not initialized")
}

struct TaskHandle {
    references: AtomicUsize,
    domain: usize,
}

impl Drop for TaskHandle {
    #[cfg_attr(all(irq_check, not(feature = "direct-root")), irq::context)]
    fn drop(&mut self) {
        if self.references.fetch_sub(1, Ordering::Release) == 1 {
            core::sync::atomic::fence(Ordering::Acquire);
            scheduler().domains.enqueue(self.domain, 1, 0);
        }
    }
}

#[cfg_attr(all(irq_check, not(feature = "direct-root")), irq::context)]
#[unsafe(no_mangle)]
pub fn irq_entry(cpu: usize) {
    if !KERNEL_INITIALIZED.load(Ordering::Relaxed) {
        return;
    }
    let domains = &scheduler().domains;
    let cpu = cpu % 2;
    #[cfg(feature = "task-scan")]
    for _ in 0..4096 {
        let Some(task) = domains.pick_next(cpu) else {
            break;
        };
        if domains.get(task % 2).should_preempt(task) {
            domains.get(task % 2).on_switch_out(task, cpu);
            core::hint::black_box(domains.enqueue(task % 2, task, cpu));
            continue;
        }
        core::hint::black_box(task);
        break;
    }
    for id in 0..2 {
        if domains.get(id).should_preempt(id) {
            if let Some(task) = domains.pick_next(cpu) {
                domains.get(id).on_switch_out(task, cpu);
                core::hint::black_box(domains.enqueue(id, task, cpu));
            }
        }
        for domain in domains.domains.iter() {
            domain.ops.maybe_balance(id);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    SCHEDULER.call_once(|| Scheduler {
        domains: DomainMaster {
            domains: alloc::vec![
                DomainEntry { id: 0, ops: Box::new(Domain {
                    class: FifoClass,
                    per_cpu: alloc::vec![Some(AtomicUsize::new(1)), Some(AtomicUsize::new(0))].into_boxed_slice(),
                }) },
                DomainEntry { id: 1, ops: Box::new(Domain {
                    class: FifoClass,
                    per_cpu: alloc::vec![Some(AtomicUsize::new(2)), Some(AtomicUsize::new(0))].into_boxed_slice(),
                }) },
            ].into_boxed_slice(),
            per_cpu_cursor: alloc::vec![AtomicUsize::new(0), AtomicUsize::new(0)].into_boxed_slice(),
        },
    });
    KERNEL_INITIALIZED.store(true, Ordering::Release);
    irq_entry(0);
    irq_entry(1);
    drop(TaskHandle { references: AtomicUsize::new(1), domain: 0 });
    drop(TaskHandle { references: AtomicUsize::new(1), domain: 1 });
    loop { core::hint::spin_loop(); }
}

#[panic_handler]
fn panic_handler(_: &core::panic::PanicInfo<'_>) -> ! {
    loop { core::hint::spin_loop(); }
}
