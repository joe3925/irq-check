#![no_std]
#![no_main]

#[inline(never)]
fn safe_callback() {
    core::hint::black_box(3);
}

#[inline(never)]
fn guarded(condition: bool) {
    if condition { external::forbidden_contract(); }
}

#[cfg_attr(all(irq_check, feature = "invalid-function"), irq::allow_incomplete(unsafe))]
#[cfg_attr(irq_check, irq::context)]
#[unsafe(no_mangle)]
pub fn irq_entry() {
    #[cfg(feature = "direct-condition")]
    guarded(false);
    #[cfg(feature = "direct-condition-forbidden")]
    guarded(true);
    #[cfg(feature = "early-forbidden")]
    external::forbidden_contract();

    #[cfg(feature = "invalid-block")]
    #[cfg_attr(irq_check, irq::allow_incomplete(unsafe))]
    { external::operation(); }

    #[cfg(feature = "invalid-argument")]
    #[cfg_attr(irq_check, irq::allow_incomplete)]
    unsafe { external::operation(); }

    #[cfg(feature = "local-asm")]
    #[cfg_attr(irq_check, irq::trusted(unsafe))]
    unsafe { core::arch::asm!("nop", options(nomem, nostack, preserves_flags)); }

    #[cfg(feature = "trusted")]
    #[cfg_attr(irq_check, irq::trusted(unsafe))]
    unsafe { external::operation(); }

    #[cfg(feature = "trusted-condition")]
    {
        let condition;
        #[cfg_attr(irq_check, irq::allow_incomplete(unsafe))]
        unsafe { condition = external::condition(); }
        #[cfg_attr(irq_check, irq::trusted(unsafe))]
        unsafe {
            if condition { external::forbidden_contract(); }
        }
    }

    #[cfg(feature = "unmarked")]
    {
        external::operation();
        #[cfg(feature = "uncertain")]
        if external::condition() { external::forbidden_contract(); }
    }

    #[cfg(not(any(feature = "unmarked", feature = "trusted", feature = "trusted-condition", feature = "local-asm",
        feature = "invalid-block", feature = "invalid-function", feature = "invalid-argument")))]
    {
        let mut callback: fn() = safe_callback;
        #[cfg_attr(irq_check, irq::allow_incomplete(unsafe))]
        unsafe {
            #[cfg(feature = "forbidden")]
            { external::invoke(external::forbidden_contract); }
            #[cfg(not(feature = "forbidden"))]
            { external::invoke(safe_callback); }
            #[cfg(feature = "uncertain")]
            if external::condition() { external::forbidden_contract(); }
            #[cfg(feature = "effects")]
            external::update(&mut callback);
            #[cfg(feature = "returned")]
            { callback = external::returned(safe_callback); }
        }
        #[cfg(any(feature = "effects", feature = "returned"))]
        callback();
        #[cfg(feature = "repeated")]
        external::operation();
        #[cfg(feature = "escape")]
        {
            let later;
            #[cfg_attr(irq_check, irq::allow_incomplete(unsafe))]
            unsafe { later = || external::operation(); }
            later();
        }
        core::hint::black_box(&mut callback);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    irq_entry();
    loop { core::hint::spin_loop(); }
}

#[panic_handler]
fn panic_handler(_: &core::panic::PanicInfo<'_>) -> ! {
    loop { core::hint::spin_loop(); }
}
