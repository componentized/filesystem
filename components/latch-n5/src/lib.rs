#![no_main]

use latch_n::bindings::componentized::filesystem::{
    latch as latch0, latch1, latch2, latch3, latch4,
};
use latch_n::bindings::exports::componentized::filesystem::latch::{
    Decision, Guest as Latch, Operation,
};

struct LatchN5 {}

impl Latch for LatchN5 {
    #[allow(async_fn_in_trait)]
    fn check(operation: Operation<'_>) -> Decision {
        let checks = vec![
            latch0::check,
            latch1::check,
            latch2::check,
            latch3::check,
            latch4::check,
        ];
        latch_n::check(operation, checks)
    }
}

latch_n::export!(LatchN5 with_types_in latch_n::bindings);
