#![no_main]

use crate::{
    exports::componentized::filesystem::latch::{Decision, Guest as Latch, Operation},
    wasi::filesystem::types::ErrorCode,
};

struct DenyLatch {}

impl Latch for DenyLatch {
    fn check(_: Operation) -> Decision {
        Decision::Deny(ErrorCode::NotPermitted)
    }
}

wit_bindgen::generate!({
    path: "../../wit",
    world: "filesystem-latch",
    generate_all
});

export!(DenyLatch);
