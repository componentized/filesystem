#![no_main]

use std::path::Path;

use path_matchers::{glob, PathMatcher};

use crate::{
    exports::componentized::filesystem::latch::{
        Decision, DescriptorOperation, DirectoryEntryStreamOperation, Guest as Latch, Operation,
        PreopensOperation,
    },
    wasi::filesystem::types::ErrorCode,
};

struct GlobLatch {}

struct Patterns {
    initialized: bool,
    denies: Option<Vec<Box<dyn PathMatcher>>>,
    grants: Option<Vec<Box<dyn PathMatcher>>>,
    deny_reason: Option<ErrorCode>,
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
                reason = get_error_code(value).unwrap_or(ErrorCode::NotPermitted);
            }
        }

        unsafe {
            STATE.denies = Some(denies);
            STATE.grants = Some(grants);
            STATE.deny_reason = Some(reason);
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
                return Some(Decision::Denied(self.deny_reason.unwrap()));
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
    deny_reason: None,
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
                DescriptorOperation::Read(_) => Patterns::authorize(path, vec![]),
                DescriptorOperation::Write(_) => Patterns::authorize(path, vec![]),
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
            Operation::DirectoryEntryStream((_, path, directory_entry_stream_operation)) => {
                match directory_entry_stream_operation {
                    DirectoryEntryStreamOperation::ReadDirectoryEntry(directory_entry) => {
                        Patterns::authorize(path, vec![directory_entry.name])
                    }
                }
            }
        }
    }
}

fn get_error_code(value: String) -> Option<ErrorCode> {
    match value.as_str() {
        "access" => Some(ErrorCode::Access),
        "wouldblock" => Some(ErrorCode::WouldBlock),
        "already" => Some(ErrorCode::Already),
        "bad-descriptor" => Some(ErrorCode::BadDescriptor),
        "busy" => Some(ErrorCode::Busy),
        "deadlock" => Some(ErrorCode::Deadlock),
        "quota" => Some(ErrorCode::Quota),
        "exist" => Some(ErrorCode::Exist),
        "file-too-large" => Some(ErrorCode::FileTooLarge),
        "illegal-byte-sequence" => Some(ErrorCode::IllegalByteSequence),
        "in-progress" => Some(ErrorCode::InProgress),
        "interrupted" => Some(ErrorCode::Interrupted),
        "invalid" => Some(ErrorCode::Invalid),
        "io" => Some(ErrorCode::Io),
        "is-directory" => Some(ErrorCode::IsDirectory),
        "loop" => Some(ErrorCode::Loop),
        "too-many-links" => Some(ErrorCode::TooManyLinks),
        "message-size" => Some(ErrorCode::MessageSize),
        "name-too-long" => Some(ErrorCode::NameTooLong),
        "no-device" => Some(ErrorCode::NoDevice),
        "no-entry" => Some(ErrorCode::NoEntry),
        "no-lock" => Some(ErrorCode::NoLock),
        "insufficient-memory" => Some(ErrorCode::InsufficientMemory),
        "insufficient-space" => Some(ErrorCode::InsufficientSpace),
        "not-directory" => Some(ErrorCode::NotDirectory),
        "not-empty" => Some(ErrorCode::NotEmpty),
        "not-recoverable" => Some(ErrorCode::NotRecoverable),
        "unsupported" => Some(ErrorCode::Unsupported),
        "no-tty" => Some(ErrorCode::NoTty),
        "no-such-device" => Some(ErrorCode::NoSuchDevice),
        "overflow" => Some(ErrorCode::Overflow),
        "not-permitted" => Some(ErrorCode::NotPermitted),
        "pipe" => Some(ErrorCode::Pipe),
        "read-only" => Some(ErrorCode::ReadOnly),
        "invalid-seek" => Some(ErrorCode::InvalidSeek),
        "text-file-busy" => Some(ErrorCode::TextFileBusy),
        "cross-device" => Some(ErrorCode::CrossDevice),
        _ => None,
    }
}

wit_bindgen::generate!({
    path: "../../wit",
    world: "filesystem-latch",
    generate_all
});

export!(GlobLatch);
