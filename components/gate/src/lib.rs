use std::fmt::Display;

use chrono::DateTime;
use heck::ToKebabCase;

use exports::wasi::filesystem::preopens::Guest as Preopens;
use exports::wasi::filesystem::types::{
    Advice, Descriptor, DescriptorBorrow, DescriptorFlags, DescriptorStat, DescriptorType,
    DirectoryEntry, ErrorCode, Filesize, Guest as Types, MetadataHashValue, NewTimestamp,
    OpenFlags, PathFlags,
};
use wasi::filesystem::preopens;
use wasi::filesystem::types;
use wasi::logging::logging::{log, Level};

mod path;

use path::Location;

use crate::componentized::filesystem::latch::{
    self, Decision, DescriptorAdviseArgs, DescriptorCreateDirectoryAtArgs, DescriptorLinkAtArgs,
    DescriptorMetadataHashAtArgs, DescriptorOpenAtArgs, DescriptorOperation,
    DescriptorReadlinkAtArgs, DescriptorRemoveDirectoryAtArgs, DescriptorRenameAtArgs,
    DescriptorSetSizeArgs, DescriptorSetTimesArgs, DescriptorSetTimesAtArgs, DescriptorStatAtArgs,
    DescriptorSymlinkAtArgs, DescriptorUnlinkFileAtArgs, PreopensOperation,
};
use crate::componentized::filesystem::latch::{
    Decision::{Deferred, Denied},
    DescriptorReadViaStreamArgs, ErrorCode as LatchErrorCode, Operation,
};

macro_rules! error {
    ($dst:expr, $($arg:tt)*) => {
        log(Level::Error, "componentized-gate", &format!($dst, $($arg)*));
    };
    ($dst:expr) => {
        log(Level::Error, "componentized-gate", &format!($dst));
    };
}

macro_rules! warn {
    ($dst:expr, $($arg:tt)*) => {
        log(Level::Warn, "componentized-gate", &format!($dst, $($arg)*));
    };
    ($dst:expr) => {
        log(Level::Warn, "componentized-gate", &format!($dst));
    };
}

macro_rules! trace {
    ($dst:expr, $($arg:tt)*) => {
        log(Level::Trace, "componentized-gate", &format!($dst, $($arg)*));
    };
    ($dst:expr) => {
        log(Level::Trace, "componentized-gate", &format!($dst));
    };
}

/// Authorize the operation with the latch, then report the decision back to the latch. Latches
/// act on the final decision in `observe-decision`, never while authorizing. Failing to observe
/// the decision fails the operation, the same as a latch error.
fn authorize(operation: &Operation) -> Result<Decision, LatchErrorCode> {
    let decision = latch::authorize(operation)?;
    latch::observe_decision(&decision, operation)?;
    Ok(decision)
}

struct GatedFilesystem {}

impl Preopens for GatedFilesystem {
    #[doc = "/ Return the set of preopened directories, and their paths."]
    #[allow(async_fn_in_trait)]
    fn get_directories() -> Vec<(Descriptor, String)> {
        let call_summary = || format!("OPERATION=wasi:filesystem/preopens#get-directories");
        match authorize(&Operation::Preopens(PreopensOperation::GetDirectories)) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                vec![]
            }
            Ok(Deferred) => preopens::get_directories()
                .into_iter()
                .filter(|(fd, path)| {
                    match authorize(&Operation::Preopens(PreopensOperation::GetDirectoriesItem(
                        (fd, path.clone()),
                    ))) {
                        Ok(Denied(reason)) => {
                            trace!("Denied REASON={reason} {} PATH={path}", call_summary());
                            false
                        }
                        _ => true,
                    }
                })
                .map(|(fd, path)| {
                    (
                        Descriptor::new(GatedDescriptor::new(fd, Location::preopen(&path))),
                        path,
                    )
                })
                .collect(),
            Err(code) => {
                panic!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                )
            }
        }
    }
}

impl Types for GatedFilesystem {
    type Descriptor = GatedDescriptor;
}

struct GatedDescriptor {
    fd: types::Descriptor,
    /// What the descriptor is, the base `PATH` arguments are resolved against.
    location: Location,
}

impl GatedDescriptor {
    fn new(fd: types::Descriptor, location: Location) -> Self {
        Self { fd, location }
    }
}

impl exports::wasi::filesystem::types::GuestDescriptor for GatedDescriptor {
    #[doc = "/ Return a stream for reading from a file."]
    #[doc = "/"]
    #[doc = "/ Multiple read, write, and append streams may be active on the same open"]
    #[doc = "/ file and they do not interfere with each other."]
    #[doc = "/"]
    #[doc = "/ This function returns a `stream` which provides the data received from the"]
    #[doc = "/ file, and a `future` providing additional error information in case an"]
    #[doc = "/ error is encountered."]
    #[doc = "/"]
    #[doc = "/ If no error is encountered, `stream.read` on the `stream` will return"]
    #[doc = "/ `read-status::closed` with no `error-context` and the future resolves to"]
    #[doc = "/ the value `ok`. If an error is encountered, `stream.read` on the"]
    #[doc = "/ `stream` returns `read-status::closed` with an `error-context` and the future"]
    #[doc = "/ resolves to `err` with an `error-code`."]
    #[doc = "/"]
    #[doc = "/ Note: This is similar to `pread` in POSIX."]
    #[allow(async_fn_in_trait)]
    fn read_via_stream(
        &self,
        offset: Filesize,
    ) -> (
        wit_bindgen::StreamReader<u8>,
        wit_bindgen::FutureReader<Result<(), ErrorCode>>,
    ) {
        let call_summary = || {
            format!(
                "OPERATION=wasi:filesystem/types#descriptor.read-via-stream FD={self} OFFSET={offset}"
            )
        };
        match authorize(&Operation::Descriptor(DescriptorOperation::ReadViaStream(
            (&self.fd, DescriptorReadViaStreamArgs { offset }),
        ))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)?
            }
            Ok(Deferred) => self.fd.read_via_stream(offset),
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Return a stream for writing to a file, if available."]
    #[doc = "/"]
    #[doc = "/ May fail with an error-code describing why the file cannot be written."]
    #[doc = "/"]
    #[doc = "/ It is valid to write past the end of a file; the file is extended to the"]
    #[doc = "/ extent of the write, with bytes between the previous end and the start of"]
    #[doc = "/ the write set to zero."]
    #[doc = "/"]
    #[doc = "/ This function returns once either full contents of the stream are"]
    #[doc = "/ written or an error is encountered."]
    #[doc = "/"]
    #[doc = "/ Note: This is similar to `pwrite` in POSIX."]
    #[allow(async_fn_in_trait)]
    fn write_via_stream(
        &self,
        data: wit_bindgen::StreamReader<u8>,
        offset: Filesize,
    ) -> wit_bindgen::FutureReader<Result<(), ErrorCode>> {
        let call_summary = || {
            format!(
                "OPERATION=wasi:filesystem/types#descriptor.write-via-stream FD={self} OFFSET={offset}"
            )
        };
        match authorize(&Operation::Descriptor(DescriptorOperation::WriteViaStream(
            (&self.fd, DescriptorWriteViaStreamArgs { offset }),
        ))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)?
            }
            Ok(Deferred) => self.fd.write_via_stream(data, offset),
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Return a stream for appending to a file, if available."]
    #[doc = "/"]
    #[doc = "/ May fail with an error-code describing why the file cannot be appended."]
    #[doc = "/"]
    #[doc = "/ This function returns once either full contents of the stream are"]
    #[doc = "/ written or an error is encountered."]
    #[doc = "/"]
    #[doc = "/ Note: This is similar to `write` with `O_APPEND` in POSIX."]
    #[allow(async_fn_in_trait)]
    fn append_via_stream(
        &self,
        data: wit_bindgen::StreamReader<u8>,
    ) -> wit_bindgen::FutureReader<Result<(), ErrorCode>> {
        let call_summary =
            || format!("OPERATION=wasi:filesystem/types#descriptor.append-via-stream FD={self}");
        match authorize(&Operation::Descriptor(
            DescriptorOperation::AppendViaStream((&self.fd,)),
        )) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)?
            }
            Ok(Deferred) => self.fd.append_via_stream(data),
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Provide file advisory information on a descriptor."]
    #[doc = "/"]
    #[doc = "/ This is similar to `posix_fadvise` in POSIX."]
    #[allow(async_fn_in_trait)]
    async fn advise(
        &self,
        offset: Filesize,
        length: Filesize,
        advice: Advice,
    ) -> Result<(), ErrorCode> {
        let call_summary = || {
            format!(
                "OPERATION=wasi:filesystem/types#descriptor.advise FD={self} OFFSET={offset} LENGTH={length} ADVICE={advice}"
            )
        };
        match authorize(&Operation::Descriptor(DescriptorOperation::Advise((
            &self.fd,
            DescriptorAdviseArgs {
                offset,
                length,
                advice,
            },
        )))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)?
            }
            Ok(Deferred) => self.fd.advise(offset, length, advice).await,
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Synchronize the data of a file to disk."]
    #[doc = "/"]
    #[doc = "/ This function succeeds with no effect if the file descriptor is not"]
    #[doc = "/ opened for writing."]
    #[doc = "/"]
    #[doc = "/ Note: This is similar to `fdatasync` in POSIX."]
    #[allow(async_fn_in_trait)]
    async fn sync_data(&self) -> Result<(), ErrorCode> {
        let call_summary =
            || format!("OPERATION=wasi:filesystem/types#descriptor.sync-data FD={self}");
        match authorize(&Operation::Descriptor(DescriptorOperation::SyncData((
            &self.fd,
        )))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)?
            }
            Ok(Deferred) => self.fd.sync_data().await,
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Get flags associated with a descriptor."]
    #[doc = "/"]
    #[doc = "/ Note: This returns similar flags to `fcntl(fd, F_GETFL)` in POSIX."]
    #[doc = "/"]
    #[doc = "/ Note: This returns the value that was the `fs_flags` value returned"]
    #[doc = "/ from `fdstat_get` in earlier versions of WASI."]
    #[allow(async_fn_in_trait)]
    async fn get_flags(&self) -> Result<DescriptorFlags, ErrorCode> {
        let call_summary =
            || format!("OPERATION=wasi:filesystem/types#descriptor.get-flags FD={self}");
        match authorize(&Operation::Descriptor(DescriptorOperation::GetFlags((
            &self.fd,
        )))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)
            }
            Ok(Deferred) => self.fd.get_flags().await,
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Get the dynamic type of a descriptor."]
    #[doc = "/"]
    #[doc = "/ Note: This returns the same value as the `type` field of the `fd-stat`"]
    #[doc = "/ returned by `stat`, `stat-at` and similar."]
    #[doc = "/"]
    #[doc = "/ Note: This returns similar flags to the `st_mode & S_IFMT` value provided"]
    #[doc = "/ by `fstat` in POSIX."]
    #[doc = "/"]
    #[doc = "/ Note: This returns the value that was the `fs_filetype` value returned"]
    #[doc = "/ from `fdstat_get` in earlier versions of WASI."]
    #[allow(async_fn_in_trait)]
    async fn get_type(&self) -> Result<DescriptorType, ErrorCode> {
        let call_summary =
            || format!("OPERATION=wasi:filesystem/types#descriptor.get-type FD={self}");
        match authorize(&Operation::Descriptor(DescriptorOperation::GetType((
            &self.fd,
        )))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)
            }
            Ok(Deferred) => self.fd.get_type().await,
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Adjust the size of an open file. If this increases the file\'s size, the"]
    #[doc = "/ extra bytes are filled with zeros."]
    #[doc = "/"]
    #[doc = "/ Note: This was called `fd_filestat_set_size` in earlier versions of WASI."]
    #[allow(async_fn_in_trait)]
    async fn set_size(&self, size: Filesize) -> Result<(), ErrorCode> {
        let call_summary =
            || format!("OPERATION=wasi:filesystem/types#descriptor.set-size FD={self} SIZE={size}");
        match authorize(&Operation::Descriptor(DescriptorOperation::SetSize((
            &self.fd,
            DescriptorSetSizeArgs { size },
        )))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)
            }
            Ok(Deferred) => self.fd.set_size(size).await,
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Adjust the timestamps of an open file or directory."]
    #[doc = "/"]
    #[doc = "/ Note: This is similar to `futimens` in POSIX."]
    #[doc = "/"]
    #[doc = "/ Note: This was called `fd_filestat_set_times` in earlier versions of WASI."]
    #[allow(async_fn_in_trait)]
    async fn set_times(
        &self,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), ErrorCode> {
        let call_summary = || {
            format!("OPERATION=wasi:filesystem/types#descriptor.set-times FD={self} ACCESS-TIMESTAMP={data_access_timestamp} MODIFICATION-TIMESTAMP={data_modification_timestamp}")
        };
        match authorize(&Operation::Descriptor(DescriptorOperation::SetTimes((
            &self.fd,
            DescriptorSetTimesArgs {
                data_access_timestamp,
                data_modification_timestamp,
            },
        )))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)
            }
            Ok(Deferred) => {
                self.fd
                    .set_times(data_access_timestamp, data_modification_timestamp)
                    .await
            }
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Read directory entries from a directory."]
    #[doc = "/"]
    #[doc = "/ On filesystems where directories contain entries referring to themselves"]
    #[doc = "/ and their parents, often named `.` and `..` respectively, these entries"]
    #[doc = "/ are omitted."]
    #[doc = "/"]
    #[doc = "/ This always returns a new stream which starts at the beginning of the"]
    #[doc = "/ directory. Multiple streams may be active on the same directory, and they"]
    #[doc = "/ do not interfere with each other."]
    #[doc = "/"]
    #[doc = "/ This function returns a future, which will resolve to an error code if"]
    #[doc = "/ reading full contents of the directory fails."]
    #[allow(async_fn_in_trait)]
    fn read_directory(
        &self,
    ) -> (
        wit_bindgen::StreamReader<DirectoryEntry>,
        wit_bindgen::FutureReader<Result<(), ErrorCode>>,
    ) {
        let call_summary =
            || format!("OPERATION=wasi:filesystem/types#descriptor.read-directory FD={self}");
        match authorize(&Operation::Descriptor(DescriptorOperation::ReadDirectory(
            (&self.fd,),
        ))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)
            }
            Ok(Deferred) => self.fd.read_directory(),
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Synchronize the data and metadata of a file to disk."]
    #[doc = "/"]
    #[doc = "/ This function succeeds with no effect if the file descriptor is not"]
    #[doc = "/ opened for writing."]
    #[doc = "/"]
    #[doc = "/ Note: This is similar to `fsync` in POSIX."]
    #[allow(async_fn_in_trait)]
    async fn sync(&self) -> Result<(), ErrorCode> {
        let call_summary = || format!("OPERATION=wasi:filesystem/types#descriptor.sync FD={self}");
        match authorize(&Operation::Descriptor(DescriptorOperation::Sync((
            &self.fd,
        )))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)
            }
            Ok(Deferred) => self.fd.sync().await,
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Create a directory."]
    #[doc = "/"]
    #[doc = "/ Note: This is similar to `mkdirat` in POSIX."]
    #[allow(async_fn_in_trait)]
    async fn create_directory_at(&self, path: String) -> Result<(), ErrorCode> {
        let call_summary = || {
            format!("OPERATION=wasi:filesystem/types#descriptor.create-directory-at FD={self} PATH={path}")
        };
        match authorize(&Operation::Descriptor(
            DescriptorOperation::CreateDirectoryAt((
                &self.fd,
                DescriptorCreateDirectoryAtArgs { path },
            )),
        )) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)
            }
            Ok(Deferred) => self.fd.create_directory_at(path).await,
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Return the attributes of an open file or directory."]
    #[doc = "/"]
    #[doc = "/ Note: This is similar to `fstat` in POSIX, except that it does not return"]
    #[doc = "/ device and inode information. For testing whether two descriptors refer to"]
    #[doc = "/ the same underlying filesystem object, use `is-same-object`. To obtain"]
    #[doc = "/ additional data that can be used do determine whether a file has been"]
    #[doc = "/ modified, use `metadata-hash`."]
    #[doc = "/"]
    #[doc = "/ Note: This was called `fd_filestat_get` in earlier versions of WASI."]
    #[allow(async_fn_in_trait)]
    async fn stat(&self) -> Result<DescriptorStat, ErrorCode> {
        let call_summary = || format!("OPERATION=wasi:filesystem/types#descriptor.stat FD={self}");
        match authorize(&Operation::Descriptor(DescriptorOperation::Stat((
            &self.fd,
        )))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)
            }
            Ok(Deferred) => self.fd.stat().await,
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Return the attributes of a file or directory."]
    #[doc = "/"]
    #[doc = "/ Note: This is similar to `fstatat` in POSIX, except that it does not"]
    #[doc = "/ return device and inode information. See the `stat` description for a"]
    #[doc = "/ discussion of alternatives."]
    #[doc = "/"]
    #[doc = "/ Note: This was called `path_filestat_get` in earlier versions of WASI."]
    #[allow(async_fn_in_trait)]
    async fn stat_at(
        &self,
        path_flags: PathFlags,
        path: String,
    ) -> Result<DescriptorStat, ErrorCode> {
        let call_summary = || {
            format!("OPERATION=wasi:filesystem/types#descriptor.stat-at FD={self} PATH-FLAGS={path_flags} PATH={path}")
        };
        match authorize(&Operation::Descriptor(DescriptorOperation::StatAt((
            &self.fd,
            DescriptorStatAtArgs {
                path_flags,
                path: path.clone(),
            },
        )))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)
            }
            Ok(Deferred) => self.fd.stat_at(path_flags, path).await,
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Adjust the timestamps of a file or directory."]
    #[doc = "/"]
    #[doc = "/ Note: This is similar to `utimensat` in POSIX."]
    #[doc = "/"]
    #[doc = "/ Note: This was called `path_filestat_set_times` in earlier versions of"]
    #[doc = "/ WASI."]
    #[allow(async_fn_in_trait)]
    async fn set_times_at(
        &self,
        path_flags: PathFlags,
        path: String,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), ErrorCode> {
        let call_summary = || {
            format!("OPERATION=wasi:filesystem/types#descriptor.set-times-at FD={self} PATH-FLAGS={path_flags} PATH={path} ACCESS-TIMESTAMP={data_access_timestamp} MODIFICATION-TIMESTAMP={data_modification_timestamp}")
        };
        match authorize(&Operation::Descriptor(DescriptorOperation::SetTimesAt((
            &self.fd,
            DescriptorSetTimesAtArgs {
                path_flags,
                path: path.clone(),
                data_access_timestamp,
                data_modification_timestamp,
            },
        )))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)
            }
            Ok(Deferred) => {
                self.fd
                    .set_times_at(
                        path_flags,
                        path,
                        data_access_timestamp,
                        data_modification_timestamp,
                    )
                    .await
            }
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Create a hard link."]
    #[doc = "/"]
    #[doc = "/ Fails with `error-code::no-entry` if the old path does not exist,"]
    #[doc = "/ with `error-code::exist` if the new path already exists, and"]
    #[doc = "/ `error-code::not-permitted` if the old path is not a file."]
    #[doc = "/"]
    #[doc = "/ Note: This is similar to `linkat` in POSIX."]
    #[allow(async_fn_in_trait)]
    async fn link_at(
        &self,
        old_path_flags: PathFlags,
        old_path: String,
        new_descriptor: DescriptorBorrow<'_>,
        new_path: String,
    ) -> Result<(), ErrorCode> {
        let new_descriptor: &Self = new_descriptor.get();
        let call_summary = || {
            format!("OPERATION=wasi:filesystem/types#descriptor.link-at FD={self} OLD-PATH-FLAGS={old_path_flags} OLD-PATH={old_path} NEW-DESCRIPTOR={new_descriptor} NEW-PATH={new_path}")
        };
        match authorize(&Operation::Descriptor(DescriptorOperation::LinkAt((
            &self.fd,
            DescriptorLinkAtArgs {
                old_path_flags,
                old_path: old_path.clone(),
                new_descriptor: &new_descriptor.fd,
                new_path: new_path.clone(),
            },
        )))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)
            }
            Ok(Deferred) => {
                self.fd
                    .link_at(old_path_flags, old_path, &new_descriptor.fd, new_path)
                    .await
            }
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Open a file or directory."]
    #[doc = "/"]
    #[doc = "/ If `flags` contains `descriptor-flags::mutate-directory`, and the base"]
    #[doc = "/ descriptor doesn\'t have `descriptor-flags::mutate-directory` set,"]
    #[doc = "/ `open-at` fails with `error-code::read-only`."]
    #[doc = "/"]
    #[doc = "/ If `flags` contains `write` or `mutate-directory`, or `open-flags`"]
    #[doc = "/ contains `truncate` or `create`, and the base descriptor doesn\'t have"]
    #[doc = "/ `descriptor-flags::mutate-directory` set, `open-at` fails with"]
    #[doc = "/ `error-code::read-only`."]
    #[doc = "/"]
    #[doc = "/ Note: This is similar to `openat` in POSIX."]
    #[allow(async_fn_in_trait)]
    async fn open_at(
        &self,
        path_flags: PathFlags,
        path: String,
        open_flags: OpenFlags,
        flags: DescriptorFlags,
    ) -> Result<Descriptor, ErrorCode> {
        let call_summary = || {
            format!("OPERATION=wasi:filesystem/types#descriptor.open-at FD={self} PATH-FLAGS={path_flags} PATH={path} OPEN-FLAGS={open_flags} FLAGS={flags}")
        };
        match authorize(&Operation::Descriptor(DescriptorOperation::OpenAt((
            &self.fd,
            DescriptorOpenAtArgs {
                path_flags,
                path: path.clone(),
                open_flags,
                flags,
            },
        )))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)
            }
            Ok(Deferred) => {
                let fd = self
                    .fd
                    .open_at(path_flags, path.clone(), open_flags, flags)
                    .await?;
                // the host opened the path, so it is resolved against this descriptor
                let follow = path_flags.contains(PathFlags::SYMLINK_FOLLOW);
                let location = self
                    .location
                    .open(&path, follow, async |path: &str| {
                        self.fd.readlink_at(path.to_string()).await.ok()
                    })
                    .await;
                Ok(Descriptor::new(GatedDescriptor::new(fd, location)))
            }
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Read the contents of a symbolic link."]
    #[doc = "/"]
    #[doc = "/ If the contents contain an absolute or rooted path in the underlying"]
    #[doc = "/ filesystem, this function fails with `error-code::not-permitted`."]
    #[doc = "/"]
    #[doc = "/ Note: This is similar to `readlinkat` in POSIX."]
    #[allow(async_fn_in_trait)]
    async fn readlink_at(&self, path: String) -> Result<String, ErrorCode> {
        let call_summary = || {
            format!("OPERATION=wasi:filesystem/types#descriptor.readlink-at FD={self} PATH={path}")
        };
        match authorize(&Operation::Descriptor(DescriptorOperation::ReadlinkAt((
            &self.fd,
            DescriptorReadlinkAtArgs { path: path.clone() },
        )))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)
            }
            Ok(Deferred) => self.fd.readlink_at(path).await,
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Remove a directory."]
    #[doc = "/"]
    #[doc = "/ Return `error-code::not-empty` if the directory is not empty."]
    #[doc = "/"]
    #[doc = "/ Note: This is similar to `unlinkat(fd, path, AT_REMOVEDIR)` in POSIX."]
    #[allow(async_fn_in_trait)]
    async fn remove_directory_at(&self, path: String) -> Result<(), ErrorCode> {
        let call_summary = || {
            format!("OPERATION=wasi:filesystem/types#descriptor.remove-directory-at FD={self} PATH={path}")
        };
        match authorize(&Operation::Descriptor(
            DescriptorOperation::RemoveDirectoryAt((
                &self.fd,
                DescriptorRemoveDirectoryAtArgs { path: path.clone() },
            )),
        )) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)
            }
            Ok(Deferred) => self.fd.remove_directory_at(path).await,
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Rename a filesystem object."]
    #[doc = "/"]
    #[doc = "/ Note: This is similar to `renameat` in POSIX."]
    #[allow(async_fn_in_trait)]
    async fn rename_at(
        &self,
        old_path: String,
        new_descriptor: DescriptorBorrow<'_>,
        new_path: String,
    ) -> Result<(), ErrorCode> {
        let new_descriptor: &Self = new_descriptor.get();
        let call_summary = || {
            format!("OPERATION=wasi:filesystem/types#descriptor.rename-at FD={self} OLD-PATH={old_path} NEW-DESCRIPTOR={new_descriptor} NEW-PATH={new_path}")
        };
        match authorize(&Operation::Descriptor(DescriptorOperation::RenameAt((
            &self.fd,
            DescriptorRenameAtArgs {
                old_path: old_path.clone(),
                new_descriptor: &new_descriptor.fd,
                new_path: new_path.clone(),
            },
        )))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)
            }
            Ok(Deferred) => {
                self.fd
                    .rename_at(old_path, &new_descriptor.fd, new_path)
                    .await
            }
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Create a symbolic link (also known as a \"symlink\")."]
    #[doc = "/"]
    #[doc = "/ If `old-path` starts with `/`, the function fails with"]
    #[doc = "/ `error-code::not-permitted`."]
    #[doc = "/"]
    #[doc = "/ Note: This is similar to `symlinkat` in POSIX."]
    #[allow(async_fn_in_trait)]
    async fn symlink_at(&self, old_path: String, new_path: String) -> Result<(), ErrorCode> {
        let call_summary = || {
            format!("OPERATION=wasi:filesystem/types#descriptor.symlink-at FD={self} OLD-PATH={old_path} NEW-PATH={new_path}")
        };
        match authorize(&Operation::Descriptor(DescriptorOperation::SymlinkAt((
            &self.fd,
            DescriptorSymlinkAtArgs {
                old_path: old_path.clone(),
                new_path: new_path.clone(),
            },
        )))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)
            }
            Ok(Deferred) => self.fd.symlink_at(old_path, new_path).await,
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Unlink a filesystem object that is not a directory."]
    #[doc = "/"]
    #[doc = "/ This is similar to `unlinkat(fd, path, 0)` in POSIX."]
    #[doc = "/"]
    #[doc = "/ Error returns are as specified by POSIX."]
    #[doc = "/"]
    #[doc = "/ If the filesystem object is a directory, `error-code::access` or"]
    #[doc = "/ `error-code::is-directory` may be returned instead of the"]
    #[doc = "/ POSIX-specified `error-code::not-permitted`."]
    #[allow(async_fn_in_trait)]
    async fn unlink_file_at(&self, path: String) -> Result<(), ErrorCode> {
        let call_summary = || {
            format!(
                "OPERATION=wasi:filesystem/types#descriptor.unlink-file-at FD={self} PATH={path}"
            )
        };
        match authorize(&Operation::Descriptor(DescriptorOperation::UnlinkFileAt((
            &self.fd,
            DescriptorUnlinkFileAtArgs { path: path.clone() },
        )))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)
            }
            Ok(Deferred) => self.fd.unlink_file_at(path).await,
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Test whether two descriptors refer to the same filesystem object."]
    #[doc = "/"]
    #[doc = "/ In POSIX, this corresponds to testing whether the two descriptors have the"]
    #[doc = "/ same device (`st_dev`) and inode (`st_ino` or `d_ino`) numbers."]
    #[doc = "/ wasi-filesystem does not expose device and inode numbers, so this function"]
    #[doc = "/ may be used instead."]
    #[allow(async_fn_in_trait)]
    async fn is_same_object(&self, other: DescriptorBorrow<'_>) -> bool {
        let other: &Self = other.get();
        self.fd.is_same_object(&other.fd).await
    }

    #[doc = "/ Return a hash of the metadata associated with a filesystem object referred"]
    #[doc = "/ to by a descriptor."]
    #[doc = "/"]
    #[doc = "/ This returns a hash of the last-modification timestamp and file size, and"]
    #[doc = "/ may also include the inode number, device number, birth timestamp, and"]
    #[doc = "/ other metadata fields that may change when the file is modified or"]
    #[doc = "/ replaced. It may also include a secret value chosen by the"]
    #[doc = "/ implementation and not otherwise exposed."]
    #[doc = "/"]
    #[doc = "/ Implementations are encouraged to provide the following properties:"]
    #[doc = "/"]
    #[doc = "/  - If the file is not modified or replaced, the computed hash value should"]
    #[doc = "/    usually not change."]
    #[doc = "/  - If the object is modified or replaced, the computed hash value should"]
    #[doc = "/    usually change."]
    #[doc = "/  - The inputs to the hash should not be easily computable from the"]
    #[doc = "/    computed hash."]
    #[doc = "/"]
    #[doc = "/ However, none of these is required."]
    #[allow(async_fn_in_trait)]
    async fn metadata_hash(&self) -> Result<MetadataHashValue, ErrorCode> {
        let call_summary =
            || format!("OPERATION=wasi:filesystem/types#descriptor.metadata-hash FD={self}");
        match authorize(&Operation::Descriptor(DescriptorOperation::MetadataHash((
            &self.fd,
        )))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)
            }
            Ok(Deferred) => self.fd.metadata_hash().await,
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }

    #[doc = "/ Return a hash of the metadata associated with a filesystem object referred"]
    #[doc = "/ to by a directory descriptor and a relative path."]
    #[doc = "/"]
    #[doc = "/ This performs the same hash computation as `metadata-hash`."]
    #[allow(async_fn_in_trait)]
    async fn metadata_hash_at(
        &self,
        path_flags: PathFlags,
        path: String,
    ) -> Result<MetadataHashValue, ErrorCode> {
        let call_summary = || {
            format!("OPERATION=wasi:filesystem/types#descriptor.metadata-hash-at FD={self} PATH-FLAGS={path_flags} PATH={path}")
        };
        match authorize(&Operation::Descriptor(DescriptorOperation::MetadataHashAt(
            (
                &self.fd,
                DescriptorMetadataHashAtArgs {
                    path_flags,
                    path: path.clone(),
                },
            ),
        ))) {
            Ok(Denied(reason)) => {
                warn!("Denied REASON={reason} {}", call_summary());
                Err(reason)
            }
            Ok(Deferred) => self.fd.metadata_hash_at(path_flags, path).await,
            Err(code) => {
                error!(
                    "Latch error CODE={code} {summary}",
                    summary = call_summary()
                );
                Err(code)?
            }
        }
    }
}

impl From<LatchErrorCode> for ErrorCode {
    fn from(value: LatchErrorCode) -> Self {
        match value {
            LatchErrorCode::InvalidConfig(latch) => {
                Self::Other(Some(format!("latch-error: invalid-config<{latch}>")))
            }
            LatchErrorCode::ObservationFailed(latch) => {
                Self::Other(Some(format!("latch-error: observation-failed<{latch}>")))
            }
            LatchErrorCode::Other(Some(message)) => {
                Self::Other(Some(format!("latch-error: {message}")))
            }
            LatchErrorCode::Other(None) => Self::Other(Some("latch-error".to_string())),
        }
    }
}

struct DisplayLatchError<'a>(&'a LatchErrorCode);
impl std::fmt::Display for DisplayLatchError<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self(LatchErrorCode::InvalidConfig(latch)) => {
                write!(f, "invalid-config<{latch}>")
            }
            Self(LatchErrorCode::ObservationFailed(latch)) => {
                write!(f, "observation-failed<{latch}>")
            }
            Self(LatchErrorCode::Other(Some(message))) => f.write_str(message),
            Self(LatchErrorCode::Other(None)) => f.write_str("other"),
        }
    }
}

impl Display for GatedDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.location.fmt(f)
    }
}

impl Display for types::Advice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            types::Advice::Normal => "normal",
            types::Advice::Sequential => "sequential",
            types::Advice::Random => "random",
            types::Advice::WillNeed => "will-need",
            types::Advice::DontNeed => "dont-need",
            types::Advice::NoReuse => "no-reuse",
        })
    }
}

impl Display for types::DescriptorFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<String> = self
            .iter_names()
            .map(|(name, _flags)| name.to_kebab_case())
            .collect();
        f.write_fmt(format_args!("({})", &names.join("|")))
    }
}

impl Display for types::OpenFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<String> = self
            .iter_names()
            .map(|(name, _flags)| name.to_kebab_case())
            .collect();
        f.write_fmt(format_args!("({})", &names.join("|")))
    }
}

impl Display for types::PathFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<String> = self
            .iter_names()
            .map(|(name, _flags)| name.to_kebab_case())
            .collect();
        f.write_fmt(format_args!("({})", &names.join("|")))
    }
}

impl Display for types::NewTimestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            types::NewTimestamp::NoChange => f.write_str("no-change"),
            types::NewTimestamp::Now => f.write_str("now"),
            types::NewTimestamp::Timestamp(instant) => {
                f.write_fmt(format_args!("timestamp<{instant}>"))
            }
        }
    }
}

impl Display for types::Instant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let date = DateTime::from_timestamp(self.seconds as i64, self.nanoseconds)
            .unwrap()
            .format("%Y-%m-%d %H:%M:%S.%3fZ");

        f.write_fmt(format_args!("{date}"))
    }
}

wit_bindgen::generate!({
    path: "../wit",
    world: "filesystem",
    merge_structurally_equal_types: true,
    generate_all
});

export!(GatedFilesystem);
