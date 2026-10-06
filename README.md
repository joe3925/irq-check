# irq-check

## Install

```text
git clone https://github.com/joe3925/irq-check.git
cd irq-check
rustup toolchain install nightly-2026-04-07 --component rustc-dev --component rust-src --component llvm-tools-preview
cargo +nightly-2026-04-07 install --locked --path .
```

Add the Cargo bin directory to `PATH`. The rustOS `xtask` uses the installed checker.

## Mark functions

```rust
#[cfg_attr(irq_check, irq::context)]
fn interrupt_handler() {}

#[cfg_attr(irq_check, irq::forbidden)]
fn heap_operation() {}

trait Device {
    #[cfg_attr(irq_check, irq::context)]
    fn service(&self);
}
```

Mark actual callback functions that can run in interrupt context, not function-pointer fields. A marked trait method checks its concrete implementations. Mark the trait itself to check all its methods. Helpers need no mark. This convention does not restrict which functions a pointer can store.

Use `#[cfg_attr(irq_check, irq::trusted)]` only on a function whose full call path you have checked. This mark stops analysis at that function.
