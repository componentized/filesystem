use crate::exports::componentized::filesystem::latch::{
    Decision, DescriptorOperation, ErrorCode, FilesystemErrorCode, Guest as Latch, Operation,
};

struct DenySymlinkAtLatch {}

impl Latch for DenySymlinkAtLatch {
    fn authorize(operation: Operation) -> Result<Decision, ErrorCode> {
        match operation {
            Operation::Descriptor(DescriptorOperation::SymlinkAt(_)) => {
                Ok(Decision::Denied(FilesystemErrorCode::NotPermitted))
            }
            _ => Ok(Decision::Deferred),
        }
    }

    fn observe_decision(
        _final_decision: Decision,
        _operation: Operation<'_>,
    ) -> Result<(), ErrorCode> {
        // no side effects
        Ok(())
    }
}

wit_bindgen::generate!({
    path: "../wit",
    world: "latch",
    merge_structurally_equal_types: true,
    generate_all
});

export!(DenySymlinkAtLatch);
