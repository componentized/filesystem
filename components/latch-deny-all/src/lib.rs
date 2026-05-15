use crate::{
    exports::componentized::filesystem::latch::{Decision, Guest as Latch, Operation},
    wasi::filesystem::types::ErrorCode,
};

struct LatchDenyAll {}

impl Latch for LatchDenyAll {
    #[allow(async_fn_in_trait)]
    fn authorize(_operation: Operation) -> Result<Decision, ErrorCode> {
        Ok(Decision::Denied(ErrorCode::NotPermitted))
    }

    #[allow(async_fn_in_trait)]
    fn observe_decision(
        _final_decision: Decision,
        _operation: Operation<'_>,
    ) -> Result<(), ErrorCode> {
        Ok(())
    }
}

wit_bindgen::generate!({
    path: "../wit",
    world: "latch",
    generate_all
});

export!(LatchDenyAll);
