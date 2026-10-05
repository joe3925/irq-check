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
#[cfg_attr(irq_check, irq::handler)]
fn interrupt_handler() {}

#[cfg_attr(irq_check, irq::forbidden)]
fn heap_operation() {}
```

The check starts at each handler. It rejects call paths that reach a forbidden function. Helpers need no mark.

Use `#[cfg_attr(irq_check, irq::trusted)]` only on a function whose full call path you have checked. This mark stops analysis at that function.
