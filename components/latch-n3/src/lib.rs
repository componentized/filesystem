#![no_main]

use latch_n::bindings::componentized::filesystem::{latch as latch0, latch1, latch2};
use latch_n::bindings::exports::componentized::filesystem::latch::{
    Decision, Guest as Latch, Operation,
};

struct LatchN3 {}

impl Latch for LatchN3 {
    #[allow(async_fn_in_trait)]
    fn check(operation: Operation<'_>) -> Decision {
        let checks = vec![latch0::check, latch1::check, latch2::check];
        latch_n::check(operation, checks)
    }
}

latch_n::export!(LatchN3 with_types_in latch_n::bindings);
