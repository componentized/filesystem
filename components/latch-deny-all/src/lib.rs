use crate::exports::componentized::filesystem::latch::{
    Decision, ErrorCode, FilesystemErrorCode, Guest as Latch, Operation,
};

struct LatchDenyAll {}

impl Latch for LatchDenyAll {
    #[allow(async_fn_in_trait)]
    fn authorize(_operation: Operation) -> Result<Decision, ErrorCode> {
        Ok(Decision::Denied(FilesystemErrorCode::Access))
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
