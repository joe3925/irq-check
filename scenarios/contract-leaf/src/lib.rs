#![no_std]

#[inline(never)]
pub fn operation() {
    unsafe { core::arch::asm!("nop", options(nomem, nostack, preserves_flags)); }
}

#[cfg_attr(irq_check, irq::forbidden)]
#[inline(never)]
pub fn forbidden_contract() {
    core::hint::black_box(7);
}

#[inline(never)]
pub fn condition() -> bool {
    let value: usize;
    unsafe { core::arch::asm!("xor {value}, {value}", value = out(reg) value, options(nomem, nostack)); }
    value != 0
}

#[inline(never)]
pub fn update(callback: &mut fn()) {
    unsafe { core::arch::asm!("nop", in("rax") callback, options(nostack, preserves_flags)); }
}

#[inline(never)]
pub fn returned(callback: fn()) -> fn() {
    let output: usize;
    unsafe {
        core::arch::asm!("mov {output}, {input}", output = out(reg) output,
            input = in(reg) callback as usize, options(nomem, nostack, preserves_flags));
        core::mem::transmute(output)
    }
}
