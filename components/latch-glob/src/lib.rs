#![no_main]

use std::path::Path;

use path_matchers::{glob, PathMatcher};

use crate::{
    exports::componentized::filesystem::latch::{
        Decision, DescriptorOperation, Guest as Latch, Operation, PreopensOperation,
    },
    wasi::filesystem::types::ErrorCode,
};

struct GlobLatch {}

struct Patterns {
    initialized: bool,
    denies: Option<Vec<Box<dyn PathMatcher>>>,
    grants: Option<Vec<Box<dyn PathMatcher>>>,
    deny_reason: ErrorCode,
}

impl Patterns {
    fn initialize() {
        if unsafe { STATE.initialized } {
            return;
        }

        let mut denies: Vec<Box<dyn PathMatcher>> = vec![];
        let mut grants: Vec<Box<dyn PathMatcher>> = vec![];
        let mut reason = ErrorCode::NotPermitted;

        for (key, value) in wasi::config::store::get_all().expect("config must be available") {
            if key.starts_with("deny") {
                denies.push(Box::new(
                    glob(&value).expect("config value must parse as a glob"),
                ));
            } else if key.starts_with("grant") {
                grants.push(Box::new(
                    glob(&value).expect("config value must parse as a glob"),
                ));
            } else if key == "reason" {
                reason = get_error_code(value);
            }
        }

        unsafe {
            STATE.denies = Some(denies);
            STATE.grants = Some(grants);
            STATE.deny_reason = reason;
            STATE.initialized = true;
        };
    }

    #[allow(static_mut_refs)]
    fn authorize(path: String, paths: Vec<String>) -> Option<Decision> {
        if paths.len() == 0 {
            let path = Path::new(&path);
            return unsafe { STATE.authorize_path(path) };
        }
        let mut decision = None;
        for p in paths {
            let path = Path::new(&path).join(p);
            match unsafe { STATE.authorize_path(&path) } {
                // return denies immediately, buffer grants
                Some(Decision::Denied(reason)) => return Some(Decision::Denied(reason)),
                Some(Decision::Granted) => decision = Some(Decision::Granted),
                None => {}
            }
        }
        decision
    }

    fn authorize_path(&self, path: &Path) -> Option<Decision> {
        for deny in self.denies.as_ref().unwrap() {
            if deny.matches(path) {
                return Some(Decision::Denied(self.deny_reason.clone()));
            }
        }
        for grant in self.grants.as_ref().unwrap() {
            if grant.matches(path) {
                return Some(Decision::Granted);
            }
        }
        None
    }
}

static mut STATE: Patterns = Patterns {
    initialized: false,
    denies: None,
    grants: None,
    deny_reason: ErrorCode::NotPermitted,
};

impl Latch for GlobLatch {
    fn authorize(operation: Operation) -> Option<Decision> {
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

fn get_error_code(value: String) -> ErrorCode {
    match value.as_str() {
        "access" => ErrorCode::Access,
        "already" => ErrorCode::Already,
        "bad-descriptor" => ErrorCode::BadDescriptor,
        "busy" => ErrorCode::Busy,
        "deadlock" => ErrorCode::Deadlock,
        "quota" => ErrorCode::Quota,
        "exist" => ErrorCode::Exist,
        "file-too-large" => ErrorCode::FileTooLarge,
        "illegal-byte-sequence" => ErrorCode::IllegalByteSequence,
        "in-progress" => ErrorCode::InProgress,
        "interrupted" => ErrorCode::Interrupted,
        "invalid" => ErrorCode::Invalid,
        "io" => ErrorCode::Io,
        "is-directory" => ErrorCode::IsDirectory,
        "loop" => ErrorCode::Loop,
        "too-many-links" => ErrorCode::TooManyLinks,
        "message-size" => ErrorCode::MessageSize,
        "name-too-long" => ErrorCode::NameTooLong,
        "no-device" => ErrorCode::NoDevice,
        "no-entry" => ErrorCode::NoEntry,
        "no-lock" => ErrorCode::NoLock,
        "insufficient-memory" => ErrorCode::InsufficientMemory,
        "insufficient-space" => ErrorCode::InsufficientSpace,
        "not-directory" => ErrorCode::NotDirectory,
        "not-empty" => ErrorCode::NotEmpty,
        "not-recoverable" => ErrorCode::NotRecoverable,
        "unsupported" => ErrorCode::Unsupported,
        "no-tty" => ErrorCode::NoTty,
        "no-such-device" => ErrorCode::NoSuchDevice,
        "overflow" => ErrorCode::Overflow,
        "not-permitted" => ErrorCode::NotPermitted,
        "pipe" => ErrorCode::Pipe,
        "read-only" => ErrorCode::ReadOnly,
        "invalid-seek" => ErrorCode::InvalidSeek,
        "text-file-busy" => ErrorCode::TextFileBusy,
        "cross-device" => ErrorCode::CrossDevice,
        "other" => ErrorCode::Other(None),
        _ => ErrorCode::Other(Some(value)),
    }
}

wit_bindgen::generate!({
    path: "../../wit",
    world: "filesystem-latch",
    merge_structurally_equal_types: true,
    generate_all
});

export!(GlobLatch);
