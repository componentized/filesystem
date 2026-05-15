pub use crate::bindings::exports::componentized::filesystem::latch::{
    Decision, ErrorCode, Guest as Latch, Operation,
};
pub use crate::bindings::{latch0, latch1, latch2, latch3, latch4};

pub fn authorize(
    operation: Operation,
    authorizers: Vec<fn(&Operation<'_>) -> Result<Decision, ErrorCode>>,
) -> Result<Decision, ErrorCode> {
    for authorize in authorizers {
        match authorize(&operation)? {
            Decision::Deferred => {}
            Decision::Denied(error_code) => return Ok(Decision::Denied(error_code)),
        }
    }
    Ok(Decision::Deferred)
}

/// Pass the final decision to every nested latch, including latches that were not asked to
/// authorize the operation because an earlier latch denied it.
///
/// Every observer is called even if an earlier one fails, the result is the first error.
pub fn observe_decision(
    final_decision: Decision,
    operation: Operation,
    observers: Vec<fn(&Decision, &Operation<'_>) -> Result<(), ErrorCode>>,
) -> Result<(), ErrorCode> {
    let mut result = Ok(());
    for observe in observers {
        if let Err(err) = observe(&final_decision, &operation) {
            if result.is_ok() {
                result = Err(err);
            }
        }
    }
    result
}

pub mod bindings {
    wit_bindgen::generate!({
        path: "../../components/wit",
        world: "latch-n",
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
