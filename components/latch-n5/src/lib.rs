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
    fn authorize(operation: Operation<'_>) -> Option<Decision> {
        let authorizers = vec![
            latch0::authorize,
            latch1::authorize,
            latch2::authorize,
            latch3::authorize,
            latch4::authorize,
        ];
        latch_n::authorize(operation, authorizers)
    }
}

latch_n::export!(LatchN5 with_types_in latch_n::bindings);
