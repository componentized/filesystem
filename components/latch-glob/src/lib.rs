#![no_main]

use std::path::Path;

use path_matchers::{glob, PathMatcher};

use crate::exports::componentized::filesystem::latch::{
    Decision, DescriptorOperation, ErrorCode, FilesystemErrorCode, Guest as Latch, Operation,
    PreopensOperation,
};

struct GlobLatch {}

struct Patterns {
    initialized: bool,
    denies: Option<Vec<Box<dyn PathMatcher>>>,
    deny_reason: FilesystemErrorCode,
}

impl Patterns {
    fn initialize() {
        if unsafe { STATE.initialized } {
            return;
        }

        let mut denies: Vec<Box<dyn PathMatcher>> = vec![];
        let mut reason = FilesystemErrorCode::NotPermitted;

        for (key, value) in wasi::config::store::get_all().expect("config must be available") {
            if key.starts_with("deny") {
                denies.push(Box::new(
                    glob(&value).expect("config value must parse as a glob"),
                ));
            } else if key == "reason" {
                reason = get_error_code(value);
            }
        }

        unsafe {
            STATE.denies = Some(denies);
            STATE.deny_reason = reason;
            STATE.initialized = true;
        };
    }

    #[allow(static_mut_refs)]
    fn authorize(path: String, paths: Vec<String>) -> Result<Decision, ErrorCode> {
        if paths.len() == 0 {
            let path = Path::new(&path);
            return unsafe { STATE.authorize_path(path) };
        }
        let mut decision = Decision::Abstained;
        for p in paths {
            let path = Path::new(&path).join(p);
            match unsafe { STATE.authorize_path(&path)? } {
                // return denies immediately, buffer grants
                Decision::Denied(reason) => return Ok(Decision::Denied(reason)),
                Decision::Abstained => {}
            }
        }
        Ok(decision)
    }

    fn authorize_path(&self, path: &Path) -> Result<Decision, ErrorCode> {
        for deny in self.denies.as_ref().unwrap() {
            if deny.matches(path) {
                return Ok(Decision::Denied(self.deny_reason.clone()));
            }
        }
        Ok(Decision::Abstained)
    }
}

static mut STATE: Patterns = Patterns {
    initialized: false,
    denies: None,
    deny_reason: FilesystemErrorCode::NotPermitted,
};

impl Latch for GlobLatch {
    fn authorize(operation: Operation) -> Result<Decision, ErrorCode> {
        Patterns::initialize();

        match operation {
            Operation::Preopens(preopens_operation) => match preopens_operation {
                PreopensOperation::GetDirectoriesItem((_, path)) => {
                    Patterns::authorize(path, vec![])
                }
            },
            Operation::Descriptor((_, path, descriptor_operation)) => match descriptor_operation {
                DescriptorOperation::ReadViaStream(_) => Patterns::authorize(path, vec![]),
                DescriptorOperation::WriteViaStream(_) => Patterns::authorize(path, vec![]),
                DescriptorOperation::AppendViaStream => Patterns::authorize(path, vec![]),
                DescriptorOperation::Advise(_) => Patterns::authorize(path, vec![]),
                DescriptorOperation::SyncData => Patterns::authorize(path, vec![]),
                DescriptorOperation::GetFlags => Patterns::authorize(path, vec![]),
                DescriptorOperation::GetType => Patterns::authorize(path, vec![]),
                DescriptorOperation::SetSize(_) => Patterns::authorize(path, vec![]),
                DescriptorOperation::SetTimes(_) => Patterns::authorize(path, vec![]),
                DescriptorOperation::ReadDirectory => Patterns::authorize(path, vec![]),
                DescriptorOperation::Sync => Patterns::authorize(path, vec![]),
                DescriptorOperation::CreateDirectoryAt(args) => {
                    Patterns::authorize(path, vec![args.path])
                }
                DescriptorOperation::Stat => Patterns::authorize(path, vec![]),
                DescriptorOperation::StatAt(args) => Patterns::authorize(path, vec![args.path]),
                DescriptorOperation::SetTimesAt(args) => Patterns::authorize(path, vec![args.path]),
                DescriptorOperation::LinkAt(args) => {
                    Patterns::authorize(path, vec![args.old_path, args.new_path])
                }
                DescriptorOperation::OpenAt(args) => Patterns::authorize(path, vec![args.path]),
                DescriptorOperation::ReadlinkAt(args) => Patterns::authorize(path, vec![args.path]),
                DescriptorOperation::RemoveDirectoryAt(args) => {
                    Patterns::authorize(path, vec![args.path])
                }
                DescriptorOperation::RenameAt(args) => {
                    Patterns::authorize(path, vec![args.old_path, args.new_path])
                }
                DescriptorOperation::SymlinkAt(args) => {
                    Patterns::authorize(path, vec![args.old_path, args.new_path])
                }
                DescriptorOperation::UnlinkFileAt(args) => {
                    Patterns::authorize(path, vec![args.path])
                }
                DescriptorOperation::MetadataHash => Patterns::authorize(path, vec![]),
                DescriptorOperation::MetadataHashAt(args) => {
                    Patterns::authorize(path, vec![args.path])
                }
            },
        }
    }
}

fn get_error_code(value: String) -> FilesystemErrorCode {
    match value.as_str() {
        "access" => FilesystemErrorCode::Access,
        "already" => FilesystemErrorCode::Already,
        "bad-descriptor" => FilesystemErrorCode::BadDescriptor,
        "busy" => FilesystemErrorCode::Busy,
        "deadlock" => FilesystemErrorCode::Deadlock,
        "quota" => FilesystemErrorCode::Quota,
        "exist" => FilesystemErrorCode::Exist,
        "file-too-large" => FilesystemErrorCode::FileTooLarge,
        "illegal-byte-sequence" => FilesystemErrorCode::IllegalByteSequence,
        "in-progress" => FilesystemErrorCode::InProgress,
        "interrupted" => FilesystemErrorCode::Interrupted,
        "invalid" => FilesystemErrorCode::Invalid,
        "io" => FilesystemErrorCode::Io,
        "is-directory" => FilesystemErrorCode::IsDirectory,
        "loop" => FilesystemErrorCode::Loop,
        "too-many-links" => FilesystemErrorCode::TooManyLinks,
        "message-size" => FilesystemErrorCode::MessageSize,
        "name-too-long" => FilesystemErrorCode::NameTooLong,
        "no-device" => FilesystemErrorCode::NoDevice,
        "no-entry" => FilesystemErrorCode::NoEntry,
        "no-lock" => FilesystemErrorCode::NoLock,
        "insufficient-memory" => FilesystemErrorCode::InsufficientMemory,
        "insufficient-space" => FilesystemErrorCode::InsufficientSpace,
        "not-directory" => FilesystemErrorCode::NotDirectory,
        "not-empty" => FilesystemErrorCode::NotEmpty,
        "not-recoverable" => FilesystemErrorCode::NotRecoverable,
        "unsupported" => FilesystemErrorCode::Unsupported,
        "no-tty" => FilesystemErrorCode::NoTty,
        "no-such-device" => FilesystemErrorCode::NoSuchDevice,
        "overflow" => FilesystemErrorCode::Overflow,
        "not-permitted" => FilesystemErrorCode::NotPermitted,
        "pipe" => FilesystemErrorCode::Pipe,
        "read-only" => FilesystemErrorCode::ReadOnly,
        "invalid-seek" => FilesystemErrorCode::InvalidSeek,
        "text-file-busy" => FilesystemErrorCode::TextFileBusy,
        "cross-device" => FilesystemErrorCode::CrossDevice,
        "other" => FilesystemErrorCode::Other(None),
        _ => FilesystemErrorCode::Other(Some(value)),
    }
}

wit_bindgen::generate!({
    path: "../../wit",
    world: "filesystem-latch",
    merge_structurally_equal_types: true,
    generate_all
});

export!(GlobLatch);
