#![no_main]

use crate::bindings::{
    componentized::filesystem::latch,
    exports::componentized::filesystem::latch::{
        Decision, DescriptorAdviseArgs, DescriptorCreateDirectoryAtArgs, DescriptorLinkAtArgs,
        DescriptorMetadataHashAtArgs, DescriptorOpenAtArgs, DescriptorOperation,
        DescriptorReadArgs, DescriptorReadViaStreamArgs, DescriptorReadlinkAtArgs,
        DescriptorRemoveDirectoryAtArgs, DescriptorRenameAtArgs, DescriptorSetSizeArgs,
        DescriptorSetTimesArgs, DescriptorSetTimesAtArgs, DescriptorStatAtArgs,
        DescriptorSymlinkAtArgs, DescriptorUnlinkFileAtArgs, DescriptorWriteArgs,
        DescriptorWriteViaStreamArgs, DirectoryEntryStreamOperation, Operation, PreopensOperation,
    },
};

pub fn check(
    operation: Operation,
    checks: Vec<fn(&latch::Operation<'_>) -> latch::Decision>,
) -> Decision {
    let operation = operation_map(operation);
    for check in checks {
        match check(&operation) {
            latch::Decision::Abstain => {}
            latch::Decision::Allow => return Decision::Allow,
            latch::Decision::Deny(error_code) => return Decision::Deny(error_code),
        }
    }
    Decision::Abstain
}

fn operation_map(operation: Operation) -> latch::Operation {
    match operation {
        Operation::Preopens(preopens_operation) => {
            latch::Operation::Preopens(preopens_operation_map(preopens_operation))
        }
        Operation::Descriptor((descriptor, descriptor_operation)) => latch::Operation::Descriptor(
            (descriptor, descriptor_operation_map(descriptor_operation)),
        ),
        Operation::DirectoryEntryStream((
            directory_entry_stream,
            directory_entry_stream_operation,
        )) => latch::Operation::DirectoryEntryStream((
            directory_entry_stream,
            directory_entry_stream_operation_map(directory_entry_stream_operation),
        )),
    }
}

fn preopens_operation_map(preopens_operation: PreopensOperation) -> latch::PreopensOperation {
    match preopens_operation {
        PreopensOperation::GetDirectoriesItem((fd, path)) => {
            latch::PreopensOperation::GetDirectoriesItem((fd, path))
        }
    }
}

fn descriptor_operation_map(
    descriptor_operation: DescriptorOperation,
) -> latch::DescriptorOperation {
    match descriptor_operation {
        DescriptorOperation::ReadViaStream(descriptor_read_via_stream_args) => {
            latch::DescriptorOperation::ReadViaStream(descriptor_read_via_stream_args_map(
                descriptor_read_via_stream_args,
            ))
        }
        DescriptorOperation::WriteViaStream(descriptor_write_via_stream_args) => {
            latch::DescriptorOperation::WriteViaStream(descriptor_write_via_stream_args_map(
                descriptor_write_via_stream_args,
            ))
        }
        DescriptorOperation::AppendViaStream => latch::DescriptorOperation::AppendViaStream,
        DescriptorOperation::Advise(descriptor_advise_args) => {
            latch::DescriptorOperation::Advise(descriptor_advise_args_map(descriptor_advise_args))
        }
        DescriptorOperation::SyncData => latch::DescriptorOperation::SyncData,
        DescriptorOperation::GetFlags => latch::DescriptorOperation::GetFlags,
        DescriptorOperation::GetType => latch::DescriptorOperation::GetType,
        DescriptorOperation::SetSize(descriptor_set_size_args) => {
            latch::DescriptorOperation::SetSize(descriptor_set_size_args_map(
                descriptor_set_size_args,
            ))
        }
        DescriptorOperation::SetTimes(descriptor_set_times_args) => {
            latch::DescriptorOperation::SetTimes(descriptor_set_times_args_map(
                descriptor_set_times_args,
            ))
        }
        DescriptorOperation::Read(descriptor_read_args) => {
            latch::DescriptorOperation::Read(descriptor_read_args_map(descriptor_read_args))
        }
        DescriptorOperation::Write(descriptor_write_args) => {
            latch::DescriptorOperation::Write(descriptor_write_args_map(descriptor_write_args))
        }
        DescriptorOperation::ReadDirectory => latch::DescriptorOperation::ReadDirectory,
        DescriptorOperation::Sync => latch::DescriptorOperation::Sync,
        DescriptorOperation::CreateDirectoryAt(descriptor_create_directory_at_args) => {
            latch::DescriptorOperation::CreateDirectoryAt(descriptor_create_directory_at_args_map(
                descriptor_create_directory_at_args,
            ))
        }
        DescriptorOperation::Stat => latch::DescriptorOperation::Stat,
        DescriptorOperation::StatAt(descriptor_stat_at_args) => {
            latch::DescriptorOperation::StatAt(descriptor_stat_at_args_map(descriptor_stat_at_args))
        }
        DescriptorOperation::SetTimesAt(descriptor_set_times_at_args) => {
            latch::DescriptorOperation::SetTimesAt(descriptor_set_times_at_args_map(
                descriptor_set_times_at_args,
            ))
        }
        DescriptorOperation::LinkAt(descriptor_link_at_args) => {
            latch::DescriptorOperation::LinkAt(descriptor_link_at_args_map(descriptor_link_at_args))
        }
        DescriptorOperation::OpenAt(descriptor_open_at_args) => {
            latch::DescriptorOperation::OpenAt(descriptor_open_at_args_map(descriptor_open_at_args))
        }
        DescriptorOperation::ReadlinkAt(descriptor_readlink_at_args) => {
            latch::DescriptorOperation::ReadlinkAt(descriptor_readlink_at_args_map(
                descriptor_readlink_at_args,
            ))
        }
        DescriptorOperation::RemoveDirectoryAt(descriptor_remove_directory_at_args) => {
            latch::DescriptorOperation::RemoveDirectoryAt(descriptor_remove_directory_at_args_map(
                descriptor_remove_directory_at_args,
            ))
        }
        DescriptorOperation::RenameAt(descriptor_rename_at_args) => {
            latch::DescriptorOperation::RenameAt(descriptor_rename_at_args_map(
                descriptor_rename_at_args,
            ))
        }
        DescriptorOperation::SymlinkAt(descriptor_symlink_at_args) => {
            latch::DescriptorOperation::SymlinkAt(descriptor_symlink_at_args_map(
                descriptor_symlink_at_args,
            ))
        }
        DescriptorOperation::UnlinkFileAt(descriptor_unlink_file_at_args) => {
            latch::DescriptorOperation::UnlinkFileAt(descriptor_unlink_file_at_args_map(
                descriptor_unlink_file_at_args,
            ))
        }
        DescriptorOperation::MetadataHash => latch::DescriptorOperation::MetadataHash,
        DescriptorOperation::MetadataHashAt(descriptor_metadata_hash_at_args) => {
            latch::DescriptorOperation::MetadataHashAt(descriptor_metadata_hash_at_args_map(
                descriptor_metadata_hash_at_args,
            ))
        }
    }
}

fn descriptor_read_via_stream_args_map(
    args: DescriptorReadViaStreamArgs,
) -> latch::DescriptorReadViaStreamArgs {
    latch::DescriptorReadViaStreamArgs {
        offset: args.offset,
    }
}

fn descriptor_write_via_stream_args_map(
    args: DescriptorWriteViaStreamArgs,
) -> latch::DescriptorWriteViaStreamArgs {
    latch::DescriptorWriteViaStreamArgs {
        offset: args.offset,
    }
}

fn descriptor_advise_args_map(args: DescriptorAdviseArgs) -> latch::DescriptorAdviseArgs {
    latch::DescriptorAdviseArgs {
        offset: args.offset,
        length: args.length,
        advice: args.advice,
    }
}

fn descriptor_set_size_args_map(args: DescriptorSetSizeArgs) -> latch::DescriptorSetSizeArgs {
    latch::DescriptorSetSizeArgs { size: args.size }
}

fn descriptor_set_times_args_map(args: DescriptorSetTimesArgs) -> latch::DescriptorSetTimesArgs {
    latch::DescriptorSetTimesArgs {
        data_access_timestamp: args.data_access_timestamp,
        data_modification_timestamp: args.data_modification_timestamp,
    }
}

fn descriptor_read_args_map(args: DescriptorReadArgs) -> latch::DescriptorReadArgs {
    latch::DescriptorReadArgs {
        length: args.length,
        offset: args.offset,
    }
}

fn descriptor_write_args_map(args: DescriptorWriteArgs) -> latch::DescriptorWriteArgs {
    latch::DescriptorWriteArgs {
        buffer_length: args.buffer_length,
        offset: args.offset,
    }
}

fn descriptor_create_directory_at_args_map(
    args: DescriptorCreateDirectoryAtArgs,
) -> latch::DescriptorCreateDirectoryAtArgs {
    latch::DescriptorCreateDirectoryAtArgs { path: args.path }
}

fn descriptor_stat_at_args_map(args: DescriptorStatAtArgs) -> latch::DescriptorStatAtArgs {
    latch::DescriptorStatAtArgs {
        path: args.path,
        path_flags: args.path_flags,
    }
}

fn descriptor_set_times_at_args_map(
    args: DescriptorSetTimesAtArgs,
) -> latch::DescriptorSetTimesAtArgs {
    latch::DescriptorSetTimesAtArgs {
        data_access_timestamp: args.data_access_timestamp,
        data_modification_timestamp: args.data_modification_timestamp,
        path: args.path,
        path_flags: args.path_flags,
    }
}

fn descriptor_link_at_args_map(args: DescriptorLinkAtArgs) -> latch::DescriptorLinkAtArgs {
    latch::DescriptorLinkAtArgs {
        new_descriptor: args.new_descriptor,
        new_path: args.new_path,
        old_path: args.old_path,
        old_path_flags: args.old_path_flags,
    }
}

fn descriptor_open_at_args_map(args: DescriptorOpenAtArgs) -> latch::DescriptorOpenAtArgs {
    latch::DescriptorOpenAtArgs {
        flags: args.flags,
        open_flags: args.open_flags,
        path: args.path,
        path_flags: args.path_flags,
    }
}

fn descriptor_readlink_at_args_map(
    args: DescriptorReadlinkAtArgs,
) -> latch::DescriptorReadlinkAtArgs {
    latch::DescriptorReadlinkAtArgs { path: args.path }
}

fn descriptor_remove_directory_at_args_map(
    args: DescriptorRemoveDirectoryAtArgs,
) -> latch::DescriptorRemoveDirectoryAtArgs {
    latch::DescriptorRemoveDirectoryAtArgs { path: args.path }
}

fn descriptor_rename_at_args_map(args: DescriptorRenameAtArgs) -> latch::DescriptorRenameAtArgs {
    latch::DescriptorRenameAtArgs {
        new_descriptor: args.new_descriptor,
        new_path: args.new_path,
        old_path: args.old_path,
    }
}

fn descriptor_symlink_at_args_map(args: DescriptorSymlinkAtArgs) -> latch::DescriptorSymlinkAtArgs {
    latch::DescriptorSymlinkAtArgs {
        new_path: args.new_path,
        old_path: args.old_path,
    }
}

fn descriptor_unlink_file_at_args_map(
    args: DescriptorUnlinkFileAtArgs,
) -> latch::DescriptorUnlinkFileAtArgs {
    latch::DescriptorUnlinkFileAtArgs { path: args.path }
}

fn descriptor_metadata_hash_at_args_map(
    args: DescriptorMetadataHashAtArgs,
) -> latch::DescriptorMetadataHashAtArgs {
    latch::DescriptorMetadataHashAtArgs {
        path: args.path,
        path_flags: args.path_flags,
    }
}

fn directory_entry_stream_operation_map(
    directory_entry_stream_operation: DirectoryEntryStreamOperation,
) -> latch::DirectoryEntryStreamOperation {
    match directory_entry_stream_operation {
        DirectoryEntryStreamOperation::ReadDirectoryEntry(directory_entry) => {
            latch::DirectoryEntryStreamOperation::ReadDirectoryEntry(directory_entry)
        }
    }
}

pub mod bindings {
    wit_bindgen::generate!({
        path: "../../wit",
        world: "filesystem-latch-n",
        pub_export_macro: true,
        generate_all
    });
}

#[macro_export]
macro_rules! export {
    ($($t:tt)*) => {
        $crate::bindings::export!($($t)*);
    };
}
