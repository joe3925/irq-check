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

#[derive(Clone, Copy)]
pub struct Record {
    data: *const (),
    invoke: unsafe fn(*const ()) -> usize,
}

struct First { value: usize }
struct Second { value: u16 }

#[cfg_attr(irq_check, irq::forbidden)]
fn forbidden_record(value: usize) -> usize { value.wrapping_add(1) }

unsafe fn first(data: *const ()) -> usize {
    unsafe { (*data.cast::<First>()).value }
}

unsafe fn second(data: *const ()) -> usize {
    let value = unsafe { (*data.cast::<Second>()).value as usize };
    #[cfg(feature = "negative")]
    return forbidden_record(value);
    #[cfg(not(feature = "negative"))]
    value
}

#[inline(never)]
pub fn transfer(record: Record) -> Record { record }

#[cfg_attr(irq_check, irq::context)]
#[unsafe(no_mangle)]
pub fn irq_entry(index: bool) -> usize {
    let a = First { value: 17 };
    let b = Second { value: 19 };
    let entries = [
        transfer(Record { data: (&a as *const First).cast(), invoke: first }),
        transfer(Record { data: (&b as *const Second).cast(), invoke: second }),
    ];
    let copied = entries;
    let slice = &copied[..];
    let record = slice[index as usize];
    unsafe { (record.invoke)(record.data) }
}
