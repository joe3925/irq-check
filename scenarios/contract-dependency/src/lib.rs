#![no_std]

pub use leaf::{condition, forbidden_contract, operation, returned, update};

#[inline(never)]
pub fn invoke(callback: fn()) {
    leaf::operation();
    callback();
}
