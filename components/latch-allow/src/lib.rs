#![no_main]

use crate::exports::componentized::filesystem::latch::{Decision, Guest as Latch, Operation};

struct AllowLatch {}

impl Latch for AllowLatch {
    fn check(_: Operation) -> Decision {
        Decision::Allow
    }
}

wit_bindgen::generate!({
    path: "../../wit",
    world: "filesystem-latch",
    generate_all
});

export!(AllowLatch);
