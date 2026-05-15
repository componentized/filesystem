#![no_main]

use crate::{
    exports::componentized::filesystem::latch::{
        Decision::{self, Abstain, Deny},
        DescriptorOpenAtArgs, DescriptorOperation, Guest as Latch, Operation,
    },
    wasi::filesystem::types::{DescriptorFlags, ErrorCode::ReadOnly, OpenFlags},
};

struct ReadOnlyLatch {}

impl Latch for ReadOnlyLatch {
    fn check(operation: Operation) -> Decision {
        match operation {
            Operation::Descriptor((_, descriptor_operation)) => match descriptor_operation {
                DescriptorOperation::ReadViaStream(_) => Abstain,
                DescriptorOperation::WriteViaStream(_) => Deny(ReadOnly),
                DescriptorOperation::AppendViaStream => Deny(ReadOnly),
                DescriptorOperation::Advise(_) => Abstain,
                DescriptorOperation::SyncData => Deny(ReadOnly),
                DescriptorOperation::GetFlags => Abstain,
                DescriptorOperation::GetType => Abstain,
                DescriptorOperation::SetSize(_) => Deny(ReadOnly),
                DescriptorOperation::SetTimes(_) => Deny(ReadOnly),
                DescriptorOperation::Read(_) => Abstain,
                DescriptorOperation::Write(_) => Deny(ReadOnly),
                DescriptorOperation::ReadDirectory => Abstain,
                DescriptorOperation::Sync => Deny(ReadOnly),
                DescriptorOperation::CreateDirectoryAt(_) => Deny(ReadOnly),
                DescriptorOperation::Stat => Abstain,
                DescriptorOperation::StatAt(_) => Abstain,
                DescriptorOperation::SetTimesAt(_) => Deny(ReadOnly),
                DescriptorOperation::LinkAt(_) => Deny(ReadOnly),
                DescriptorOperation::OpenAt(DescriptorOpenAtArgs {
                    open_flags, flags, ..
                }) => {
                    if open_flags.intersects(
                        OpenFlags::CREATE
                            .union(OpenFlags::EXCLUSIVE)
                            .union(OpenFlags::TRUNCATE),
                    ) || flags.intersects(
                        DescriptorFlags::WRITE
                            .union(DescriptorFlags::FILE_INTEGRITY_SYNC)
                            .union(DescriptorFlags::DATA_INTEGRITY_SYNC)
                            .union(DescriptorFlags::REQUESTED_WRITE_SYNC),
                    ) {
                        Deny(ReadOnly)
                    } else {
                        Abstain
                    }
                }
                DescriptorOperation::ReadlinkAt(_) => Abstain,
                DescriptorOperation::RemoveDirectoryAt(_) => Deny(ReadOnly),
                DescriptorOperation::RenameAt(_) => Deny(ReadOnly),
                DescriptorOperation::SymlinkAt(_) => Deny(ReadOnly),
                DescriptorOperation::UnlinkFileAt(_) => Deny(ReadOnly),
                DescriptorOperation::MetadataHash => Abstain,
                DescriptorOperation::MetadataHashAt(_) => Abstain,
            },
        }
    }
}

wit_bindgen::generate!({
    path: "../../wit",
    world: "filesystem-latch",
    generate_all
});

export!(ReadOnlyLatch);
