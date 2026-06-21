#![no_main]

use crate::bindings::exports::componentized::filesystem::latch::{Decision, Operation};

pub fn authorize(
    operation: Operation,
    authorizers: Vec<fn(&Operation<'_>) -> Option<Decision>>,
) -> Option<Decision> {
    for authorize in authorizers {
        match authorize(&operation) {
            None => {}
            Some(Decision::Granted) => return Some(Decision::Granted),
            Some(Decision::Denied(error_code)) => return Some(Decision::Denied(error_code)),
        }
    }
    None
}

pub mod bindings {
    wit_bindgen::generate!({
        path: "../../wit",
        world: "filesystem-latch-n",
        pub_export_macro: true,
        merge_structurally_equal_types: true,
        generate_all
    });
}

#[macro_export]
macro_rules! export {
    ($($t:tt)*) => {
        $crate::bindings::export!($($t)*);
    };
}
