#![no_main]

use latch_n::bindings::componentized::filesystem::{latch as latch0, latch1, latch2, latch3};
use latch_n::bindings::exports::componentized::filesystem::latch::{
    Decision, Guest as Latch, Operation,
};

struct LatchN4 {}

impl Latch for LatchN4 {
    #[allow(async_fn_in_trait)]
    fn check(operation: Operation<'_>) -> Decision {
        let checks = vec![latch0::check, latch1::check, latch2::check, latch3::check];
        latch_n::check(operation, checks)
    }
}

latch_n::export!(LatchN4 with_types_in latch_n::bindings);
