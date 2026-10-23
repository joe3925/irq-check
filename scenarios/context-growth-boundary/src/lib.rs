#![no_std]

#[inline(never)]
pub fn expose<T>(pointer: *mut T) -> *mut T {
    core::hint::black_box(pointer)
}
