use crate::{
    exports::componentized::filesystem::latch::{
        Decision, DescriptorOpenAtArgs, DescriptorOperation, Guest as Latch, Operation,
    },
    wasi::filesystem::types::{DescriptorFlags, ErrorCode, OpenFlags},
};

struct ReadOnlyLatch {}

impl Latch for ReadOnlyLatch {
    #[allow(async_fn_in_trait)]
    fn authorize(operation: Operation) -> Result<Decision, ErrorCode> {
        match operation {
            Operation::Preopens(_preopens_operation) => Ok(Decision::Deferred),
            Operation::Descriptor(descriptor_operation) => match descriptor_operation {
                DescriptorOperation::ReadViaStream(_) => Ok(Decision::Deferred),
                DescriptorOperation::WriteViaStream(_) => Ok(Decision::Denied(ErrorCode::ReadOnly)),
                DescriptorOperation::AppendViaStream(_) => {
                    Ok(Decision::Denied(ErrorCode::ReadOnly))
                }
                DescriptorOperation::Advise(_) => Ok(Decision::Deferred),
                DescriptorOperation::SyncData(_) => Ok(Decision::Denied(ErrorCode::ReadOnly)),
                DescriptorOperation::GetFlags(_) => Ok(Decision::Deferred),
                DescriptorOperation::GetType(_) => Ok(Decision::Deferred),
                DescriptorOperation::SetSize(_) => Ok(Decision::Denied(ErrorCode::ReadOnly)),
                DescriptorOperation::SetTimes(_) => Ok(Decision::Denied(ErrorCode::ReadOnly)),
                DescriptorOperation::ReadDirectory(_) => Ok(Decision::Deferred),
                DescriptorOperation::ReadDirectoryEntry(_) => Ok(Decision::Deferred),
                DescriptorOperation::Sync(_) => Ok(Decision::Denied(ErrorCode::ReadOnly)),
                DescriptorOperation::CreateDirectoryAt(_) => {
                    Ok(Decision::Denied(ErrorCode::ReadOnly))
                }
                DescriptorOperation::Stat(_) => Ok(Decision::Deferred),
                DescriptorOperation::StatAt(_) => Ok(Decision::Deferred),
                DescriptorOperation::SetTimesAt(_) => Ok(Decision::Denied(ErrorCode::ReadOnly)),
                DescriptorOperation::LinkAt(_) => Ok(Decision::Denied(ErrorCode::ReadOnly)),
                DescriptorOperation::OpenAt((
                    _,
                    DescriptorOpenAtArgs {
                        open_flags, flags, ..
                    },
                )) => {
                    if open_flags.contains(OpenFlags::CREATE)
                        || open_flags.contains(OpenFlags::EXCLUSIVE)
                        || open_flags.contains(OpenFlags::TRUNCATE)
                        || flags.contains(DescriptorFlags::WRITE)
                        || flags.contains(DescriptorFlags::FILE_INTEGRITY_SYNC)
                        || flags.contains(DescriptorFlags::DATA_INTEGRITY_SYNC)
                        || flags.contains(DescriptorFlags::REQUESTED_WRITE_SYNC)
                        || flags.contains(DescriptorFlags::MUTATE_DIRECTORY)
                    {
                        Ok(Decision::Denied(ErrorCode::ReadOnly))
                    } else {
                        Ok(Decision::Deferred)
                    }
                }
                DescriptorOperation::ReadlinkAt(_) => Ok(Decision::Deferred),
                DescriptorOperation::RemoveDirectoryAt(_) => {
                    Ok(Decision::Denied(ErrorCode::ReadOnly))
                }
                DescriptorOperation::RenameAt(_) => Ok(Decision::Denied(ErrorCode::ReadOnly)),
                DescriptorOperation::SymlinkAt(_) => Ok(Decision::Denied(ErrorCode::ReadOnly)),
                DescriptorOperation::UnlinkFileAt(_) => Ok(Decision::Denied(ErrorCode::ReadOnly)),
                DescriptorOperation::MetadataHash(_) => Ok(Decision::Deferred),
                DescriptorOperation::MetadataHashAt(_) => Ok(Decision::Deferred),
            },
            Operation::DirectoryEntryStream(_) => None,
        }
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

export!(ReadOnlyLatch);
