#![no_main]

use crate::{
    exports::componentized::filesystem::latch::{Decision, Guest as Latch, Operation},
    wasi::filesystem::types::ErrorCode,
};

struct DenyAllLatch {}

impl Latch for DenyAllLatch {
    fn authorize(_: Operation) -> Option<Decision> {
        Some(Decision::Denied(ErrorCode::NotPermitted))
    }
}

wit_bindgen::generate!({
    path: "../../wit",
    world: "filesystem-latch",
    merge_structurally_equal_types: true,
    generate_all
});

export!(DenyAllLatch);
