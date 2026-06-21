#![no_main]

use std::fmt::Display;
use std::path::PathBuf;

use chrono::DateTime;
use heck::ToKebabCase;

use crate::componentized::filesystem::latch::Decision::Denied;
use crate::componentized::filesystem::latch::{
    self, authorize, DescriptorOperation, Operation, PreopensOperation,
};
use crate::exports::wasi::filesystem::preopens::Guest as Preopens;
use crate::exports::wasi::filesystem::types::{
    Advice, Descriptor, DescriptorBorrow, DescriptorFlags, DescriptorStat, DescriptorType,
    DirectoryEntry, ErrorCode, Filesize, Guest as Types, MetadataHashValue, NewTimestamp,
    OpenFlags, PathFlags,
};
use crate::wasi::filesystem::preopens;
use crate::wasi::filesystem::types;
use crate::wasi::logging::logging::{log, Level};

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

struct GatedFilesystem {}

impl Preopens for GatedFilesystem {
    #[doc = "/ Return the set of preopened directories, and their paths."]
    #[allow(async_fn_in_trait)]
    fn get_directories() -> Vec<(Descriptor, String)> {
        preopens::get_directories()
            .into_iter()
            .filter(|(fs, path)| {
                match authorize(&Operation::Preopens(PreopensOperation::GetDirectoriesItem((fs, path.clone())))) {
                    Some(Denied(reason)) => {
                        trace!("Denied REASON={reason} OPERATION=wasi:filesystem/preopens#get-directories PATH={path}");
                        false
                    }
                    _ => true,
                }
            })
            .map(|(fd, path)| {
                let fd = Descriptor::new(GatedFileDescriptor::new(fd, PathBuf::from(path.clone())));
                (fd, path)
            })
            .collect()
    }
}

impl Types for GatedFilesystem {
    type Descriptor = GatedFileDescriptor;
}

struct GatedFileDescriptor {
    fd: types::Descriptor,
    path: PathBuf,
}

impl GatedFileDescriptor {
    fn new(fd: types::Descriptor, path: PathBuf) -> Self {
        Self { fd, path }
    }
}

impl exports::wasi::filesystem::types::GuestDescriptor for GatedFileDescriptor {
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::ReadViaStream(latch::DescriptorReadViaStreamArgs { offset }),
        ))) {
            Some(Denied(reason)) => {
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.read-via-stream FD={self} OFFSET={offset}");
                let (_, data_reader) = wit_stream::new();
                let (result_writer, result_reader) =
                    wit_future::new(|| Err(ErrorCode::Other(None)));
                result_writer.write(Err(reason));
                (data_reader, result_reader)
            }
            _ => self.fd.read_via_stream(offset),
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::WriteViaStream(latch::DescriptorWriteViaStreamArgs { offset }),
        ))) {
            Some(Denied(reason)) => {
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.write-via-stream FD={self} OFFSET={offset}");
                let (result_writer, result_reader) =
                    wit_future::new(|| Err(ErrorCode::Other(None)));
                result_writer.write(Err(reason));
                result_reader
            }
            _ => self.fd.write_via_stream(data, offset),
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::AppendViaStream,
        ))) {
            Some(Denied(reason)) => {
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.append-via-stream FD={self}");
                let (result_writer, result_reader) =
                    wit_future::new(|| Err(ErrorCode::Other(None)));
                result_writer.write(Err(reason));
                result_reader
            }
            _ => self.fd.append_via_stream(data),
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::Advise(latch::DescriptorAdviseArgs {
                offset,
                length,
                advice,
            }),
        ))) {
            Some(Denied(reason)) => {
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.advise FD={self} OFFSET={offset} LENGTH={length} ADVICE={advice}");
                Err(reason)
            }
            _ => self.fd.advise(offset, length, advice).await,
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::SyncData,
        ))) {
            Some(Denied(reason)) => {
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.sync-data FD={self}");
                Err(reason)
            }
            _ => self.fd.sync_data().await,
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::GetFlags,
        ))) {
            Some(Denied(reason)) => {
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.get-flags FD={self}");
                Err(reason)
            }
            _ => self.fd.get_flags().await,
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::GetType,
        ))) {
            Some(Denied(reason)) => {
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.get-type FD={self}");
                Err(reason)
            }
            _ => self.fd.get_type().await,
        }
    }

    #[doc = "/ Adjust the size of an open file. If this increases the file\'s size, the"]
    #[doc = "/ extra bytes are filled with zeros."]
    #[doc = "/"]
    #[doc = "/ Note: This was called `fd_filestat_set_size` in earlier versions of WASI."]
    #[allow(async_fn_in_trait)]
    async fn set_size(&self, size: Filesize) -> Result<(), ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::SetSize(latch::DescriptorSetSizeArgs { size }),
        ))) {
            Some(Denied(reason)) => {
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.set-size FD={self} SIZE={size}");
                Err(reason)
            }
            _ => self.fd.set_size(size).await,
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::SetTimes(latch::DescriptorSetTimesArgs {
                data_access_timestamp,
                data_modification_timestamp,
            }),
        ))) {
            Some(Denied(reason)) => {
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.set-times FD={self} ACCESS-TIME={data_access_timestamp:?} MODIFIED-TIME={data_modification_timestamp:?}");
                Err(reason)
            }
            _ => {
                self.fd
                    .set_times(data_access_timestamp, data_modification_timestamp)
                    .await
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::ReadDirectory,
        ))) {
            Some(Denied(reason)) => {
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.read-directory FD={self}");
                let (_, dir_reader) = wit_stream::new();
                let (result_writer, result_reader) =
                    wit_future::new(|| Err(ErrorCode::Other(None)));
                result_writer.write(Err(reason));
                (dir_reader, result_reader)
            }
            _ => {
                // TODO authorize individual directory entries
                self.fd.read_directory()
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::Sync,
        ))) {
            Some(Denied(reason)) => {
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.sync FD={self}");
                Err(reason)
            }
            _ => self.fd.sync().await,
        }
    }

    #[doc = "/ Create a directory."]
    #[doc = "/"]
    #[doc = "/ Note: This is similar to `mkdirat` in POSIX."]
    #[allow(async_fn_in_trait)]
    async fn create_directory_at(&self, path: String) -> Result<(), ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::CreateDirectoryAt(latch::DescriptorCreateDirectoryAtArgs {
                path: path.clone(),
            }),
        ))) {
            Some(Denied(reason)) => {
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.create-directory-at FD={self} PATH={path}");
                Err(reason)
            }
            _ => self.fd.create_directory_at(path).await,
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::Stat,
        ))) {
            Some(Denied(reason)) => {
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.stat FD={self}");
                Err(reason)
            }
            _ => self.fd.stat().await,
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::StatAt(latch::DescriptorStatAtArgs {
                path_flags,
                path: path.clone(),
            }),
        ))) {
            Some(Denied(reason)) => {
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.stat-at FD={self} PATH-FLAGS={path_flags} PATH={path}");
                Err(reason)
            }
            _ => self.fd.stat_at(path_flags, path).await,
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::SetTimesAt(latch::DescriptorSetTimesAtArgs {
                path_flags,
                path: path.clone(),
                data_access_timestamp,
                data_modification_timestamp,
            }),
        ))) {
            Some(Denied(reason)) => {
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.set-times-at FD={self} PATH-FLAGS={path_flags} PATH={path} ACCESS-TIME={data_access_timestamp:?} MODIFIED-TIME={data_modification_timestamp:?}");
                Err(reason)
            }
            _ => {
                self.fd
                    .set_times_at(
                        path_flags,
                        path,
                        data_access_timestamp,
                        data_modification_timestamp,
                    )
                    .await
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::LinkAt(latch::DescriptorLinkAtArgs {
                old_path_flags,
                old_path: old_path.clone(),
                new_descriptor: &new_descriptor.fd,
                new_path: new_path.clone(),
            }),
        ))) {
            Some(Denied(reason)) => {
                warn!(
                    "Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.link-at FD={self} OLD-PATH={old_path} OLD-PATH-FLAGS={old_path_flags} NEW-PATH={new_path}",
                );
                Err(reason)
            }
            _ => {
                self.fd
                    .link_at(old_path_flags, old_path, &new_descriptor.fd, new_path)
                    .await
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::OpenAt(latch::DescriptorOpenAtArgs {
                path_flags,
                path: path.clone(),
                open_flags,
                flags,
            }),
        ))) {
            Some(Denied(reason)) => {
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.open-at FD={self} PATH-FLAGS={path_flags} PATH={path} OPEN-FLAGS={open_flags} FLAGS={flags}");
                Err(reason)
            }
            _ => self
                .fd
                .open_at(path_flags, path.clone(), open_flags, flags)
                .await
                .map(|fd| Descriptor::new(GatedFileDescriptor::new(fd, self.path.join(path)))),
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::ReadlinkAt(latch::DescriptorReadlinkAtArgs { path: path.clone() }),
        ))) {
            Some(Denied(reason)) => {
                warn!(
                    "Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.readlink-at FD={self} PATH={path}",
                );
                Err(reason)
            }
            _ => self.fd.readlink_at(path).await,
        }
    }

    #[doc = "/ Remove a directory."]
    #[doc = "/"]
    #[doc = "/ Return `error-code::not-empty` if the directory is not empty."]
    #[doc = "/"]
    #[doc = "/ Note: This is similar to `unlinkat(fd, path, AT_REMOVEDIR)` in POSIX."]
    #[allow(async_fn_in_trait)]
    async fn remove_directory_at(&self, path: String) -> Result<(), ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::RemoveDirectoryAt(latch::DescriptorRemoveDirectoryAtArgs {
                path: path.clone(),
            }),
        ))) {
            Some(Denied(reason)) => {
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.remove-directory-at FD={self} PATH={path}");
                Err(reason)
            }
            _ => self.fd.remove_directory_at(path).await,
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::RenameAt(latch::DescriptorRenameAtArgs {
                old_path: old_path.clone(),
                new_descriptor: &new_descriptor.get::<Self>().fd,
                new_path: new_path.clone(),
            }),
        ))) {
            Some(Denied(reason)) => {
                warn!(
                    "Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.rename-at FD={self} OLD-PATH={old_path} NEW-PATH={new_path}",
                );
                Err(reason)
            }
            _ => {
                let new_descriptor: &Self = new_descriptor.get();
                self.fd
                    .rename_at(old_path, &new_descriptor.fd, new_path)
                    .await
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::SymlinkAt(latch::DescriptorSymlinkAtArgs {
                old_path: old_path.clone(),
                new_path: new_path.clone(),
            }),
        ))) {
            Some(Denied(reason)) => {
                warn!(
                    "Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.symlink-at FD={self} OLD-PATH={old_path} NEW-PATH={new_path}",
                );
                Err(reason)
            }
            _ => self.fd.symlink_at(old_path, new_path).await,
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::UnlinkFileAt(latch::DescriptorUnlinkFileAtArgs {
                path: path.clone(),
            }),
        ))) {
            Some(Denied(reason)) => {
                warn!(
                    "Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.unlink-file-at FD={self} PATH={path}",
                );
                Err(reason)
            }
            _ => self.fd.unlink_file_at(path).await,
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::MetadataHash,
        ))) {
            Some(Denied(reason)) => {
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.metadata-hash FD={self}");
                Err(reason)
            }
            _ => self.fd.metadata_hash().await,
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
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::MetadataHashAt(latch::DescriptorMetadataHashAtArgs {
                path_flags,
                path: path.clone(),
            }),
        ))) {
            Some(Denied(reason)) => {
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.metadata-hash-at FD={self} PATH={path} PATH-FLAGS={path_flags}");
                Err(reason)
            }
            _ => self.fd.metadata_hash_at(path_flags, path).await,
        }
    }
}

impl Display for GatedFileDescriptor {
    fn fmt(&self, d: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        d.write_fmt(format_args!("{}", self.path.display()))
    }
}

impl Display for types::Advice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_string().to_kebab_case())
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

impl Display for types::DescriptorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            types::DescriptorType::Other(Some(type_)) => {
                f.write_fmt(format_args!("other<{type_}>"))
            }
            _ => f.write_str(&self.to_string().to_kebab_case()),
        }
    }
}

wit_bindgen::generate!({
    path: "../../wit",
    world: "filesystem",
    merge_structurally_equal_types: true,
    generate_all
});

export!(GatedFilesystem);
