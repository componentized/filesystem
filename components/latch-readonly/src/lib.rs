#![no_main]

use crate::{
    exports::componentized::filesystem::latch::{
        Decision::{self, Denied},
        DescriptorOpenAtArgs, DescriptorOperation, Guest as Latch, Operation,
    },
    wasi::filesystem::types::{DescriptorFlags, ErrorCode::ReadOnly, OpenFlags},
};

struct ReadOnlyLatch {}

impl Latch for ReadOnlyLatch {
    fn authorize(operation: Operation) -> Option<Decision> {
        match operation {
            Operation::Preopens(_) => None,
            Operation::Descriptor((_, _, descriptor_operation)) => match descriptor_operation {
                DescriptorOperation::ReadViaStream(_) => None,
                DescriptorOperation::WriteViaStream(_) => Some(Denied(ReadOnly)),
                DescriptorOperation::AppendViaStream => Some(Denied(ReadOnly)),
                DescriptorOperation::Advise(_) => None,
                DescriptorOperation::SyncData => Some(Denied(ReadOnly)),
                DescriptorOperation::GetFlags => None,
                DescriptorOperation::GetType => None,
                DescriptorOperation::SetSize(_) => Some(Denied(ReadOnly)),
                DescriptorOperation::SetTimes(_) => Some(Denied(ReadOnly)),
                DescriptorOperation::Read(_) => None,
                DescriptorOperation::Write(_) => Some(Denied(ReadOnly)),
                DescriptorOperation::ReadDirectory => None,
                DescriptorOperation::Sync => Some(Denied(ReadOnly)),
                DescriptorOperation::CreateDirectoryAt(_) => Some(Denied(ReadOnly)),
                DescriptorOperation::Stat => None,
                DescriptorOperation::StatAt(_) => None,
                DescriptorOperation::SetTimesAt(_) => Some(Denied(ReadOnly)),
                DescriptorOperation::LinkAt(_) => Some(Denied(ReadOnly)),
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
                        Some(Denied(ReadOnly))
                    } else {
                        None
                    }
                }
                DescriptorOperation::ReadlinkAt(_) => None,
                DescriptorOperation::RemoveDirectoryAt(_) => Some(Denied(ReadOnly)),
                DescriptorOperation::RenameAt(_) => Some(Denied(ReadOnly)),
                DescriptorOperation::SymlinkAt(_) => Some(Denied(ReadOnly)),
                DescriptorOperation::UnlinkFileAt(_) => Some(Denied(ReadOnly)),
                DescriptorOperation::MetadataHash => None,
                DescriptorOperation::MetadataHashAt(_) => None,
            },
            Operation::DirectoryEntryStream(_) => None,
        }
    }
}

wit_bindgen::generate!({
    path: "../../wit",
    world: "filesystem-latch",
    generate_all
});

export!(ReadOnlyLatch);
