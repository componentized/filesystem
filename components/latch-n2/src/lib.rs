#![no_main]

use latch_n::bindings::componentized::filesystem::{latch as latch0, latch1};
use latch_n::bindings::exports::componentized::filesystem::latch::{
    Decision, Guest as Latch, Operation,
};

struct LatchN2 {}

impl Latch for LatchN2 {
    #[allow(async_fn_in_trait)]
    fn authorize(operation: Operation<'_>) -> Option<Decision> {
        let authorizers = vec![latch0::authorize, latch1::authorize];
        latch_n::authorize(operation, authorizers)
    }
}

latch_n::export!(LatchN2 with_types_in latch_n::bindings);
