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

use core::ptr::NonNull;

#[derive(Clone, Copy)]
#[repr(C)]
struct Cell { prefix: usize, callback: unsafe extern "C" fn(*mut ()) -> usize }

#[repr(transparent)]
struct Wrapper(Cell);

unsafe extern "C" fn safe(data: *mut ()) -> usize {
    unsafe { (*data.cast::<Cell>()).prefix }
}

#[cfg_attr(irq_check, irq::forbidden)]
unsafe extern "C" fn forbidden_alias(_: *mut ()) -> usize { 23 }

#[cfg_attr(irq_check, irq::context)]
#[unsafe(no_mangle)]
pub fn irq_entry() -> usize {
    let mut original = Cell { prefix: 29, callback: safe };
    let mut destination = Wrapper(Cell { prefix: 0, callback: safe });
    unsafe {
        core::ptr::copy_nonoverlapping(&original, &mut destination.0, 1);
        let erased = NonNull::from(&mut destination).cast::<()>();
        let restored = erased.cast::<Cell>().as_ptr();
        let member = core::ptr::addr_of_mut!((*restored).callback);
        let owner = member.cast::<u8>().sub(core::mem::offset_of!(Cell, callback)).cast::<Cell>();
        #[cfg(feature = "negative")]
        core::ptr::write(core::ptr::addr_of_mut!((*owner).callback), forbidden_alias);
        #[cfg(not(feature = "negative"))]
        core::ptr::write(core::ptr::addr_of_mut!((*owner).callback), safe);
        let erased_callback: unsafe extern "C" fn() = core::mem::transmute((*restored).callback);
        let callback: unsafe extern "C" fn(*mut ()) -> usize = core::mem::transmute(erased_callback);
        original.prefix = callback(restored.cast());
    }
    original.prefix
}
