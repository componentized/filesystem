#![no_main]

use std::fmt::Display;
use std::path::PathBuf;
use std::rc::Rc;

use heck::ToKebabCase;

use crate::componentized::filesystem::latch::Decision::Denied;
use crate::componentized::filesystem::latch::{
    self, authorize, DescriptorOperation, DirectoryEntryStreamOperation, Operation,
    PreopensOperation,
};
use crate::exports::wasi::filesystem::preopens::Guest as Preopens;
use crate::exports::wasi::filesystem::types::{
    Advice, Descriptor, DescriptorBorrow, DescriptorFlags, DescriptorStat, DescriptorType,
    DirectoryEntry, DirectoryEntryStream, Error, ErrorCode, Filesize, Guest as Types, InputStream,
    MetadataHashValue, NewTimestamp, OpenFlags, OutputStream, PathFlags,
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

#[derive(Debug, Clone)]
struct GatedFilesystem {}

impl Preopens for GatedFilesystem {
    #[doc = " Return the set of preopened directories, and their path."]
    fn get_directories() -> Vec<(Descriptor, String)> {
        preopens::get_directories()
            .into_iter()
            .filter(|(fs, path)| {
                match authorize(&Operation::Preopens(PreopensOperation::GetDirectoriesItem((fs, path.clone())))) {
                    Some(Denied(code)) => {
                        let reason = error_code_display(code);
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
    type DirectoryEntryStream = GatedDirectoryEntryStream;

    #[doc = " Attempts to extract a filesystem-related `error-code` from the stream"]
    #[doc = " `error` provided."]
    #[doc = ""]
    #[doc = " Stream operations which return `stream-error::last-operation-failed`"]
    #[doc = " have a payload with more information about the operation that failed."]
    #[doc = " This payload can be passed through to this function to see if there\'s"]
    #[doc = " filesystem-related information about the error to return."]
    #[doc = ""]
    #[doc = " Note that this function is fallible because not all stream-related"]
    #[doc = " errors are filesystem-related errors."]
    fn filesystem_error_code(err: &Error) -> Option<ErrorCode> {
        types::filesystem_error_code(err).map(error_code_map)
    }
}

#[derive(Debug, Clone)]
struct GatedFileDescriptor {
    fd: Rc<types::Descriptor>,
    path: PathBuf,
}

impl GatedFileDescriptor {
    fn new(fd: types::Descriptor, path: PathBuf) -> Self {
        Self {
            fd: Rc::new(fd),
            path,
        }
    }
}

impl Display for GatedFileDescriptor {
    fn fmt(&self, d: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        d.write_fmt(format_args!("{}", self.path.display()))
    }
}

impl exports::wasi::filesystem::types::GuestDescriptor for GatedFileDescriptor {
    #[doc = " Return a stream for reading from a file, if available."]
    #[doc = ""]
    #[doc = " May fail with an error-code describing why the file cannot be read."]
    #[doc = ""]
    #[doc = " Multiple read, write, and append streams may be active on the same open"]
    #[doc = " file and they do not interfere with each other."]
    #[doc = ""]
    #[doc = " Note: This allows using `read-stream`, which is similar to `read` in POSIX."]
    fn read_via_stream(&self, offset: Filesize) -> Result<InputStream, ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::ReadViaStream(latch::DescriptorReadViaStreamArgs { offset }),
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.read-via-stream FD={self} OFFSET={offset}");
                Err(error_code_map(code))
            }
            _ => self.fd.read_via_stream(offset).map_err(error_code_map),
        }
    }

    #[doc = " Return a stream for writing to a file, if available."]
    #[doc = ""]
    #[doc = " May fail with an error-code describing why the file cannot be written."]
    #[doc = ""]
    #[doc = " Note: This allows using `write-stream`, which is similar to `write` in"]
    #[doc = " POSIX."]
    fn write_via_stream(&self, offset: Filesize) -> Result<OutputStream, ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::WriteViaStream(latch::DescriptorWriteViaStreamArgs { offset }),
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.write-via-stream FD={self} OFFSET={offset}");
                Err(error_code_map(code))
            }
            _ => self.fd.write_via_stream(offset).map_err(error_code_map),
        }
    }

    #[doc = " Return a stream for appending to a file, if available."]
    #[doc = ""]
    #[doc = " May fail with an error-code describing why the file cannot be appended."]
    #[doc = ""]
    #[doc = " Note: This allows using `write-stream`, which is similar to `write` with"]
    #[doc = " `O_APPEND` in in POSIX."]
    fn append_via_stream(&self) -> Result<OutputStream, ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::AppendViaStream,
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.append-via-stream FD={self}");
                Err(error_code_map(code))
            }
            _ => self.fd.append_via_stream().map_err(error_code_map),
        }
    }

    #[doc = " Provide file advisory information on a descriptor."]
    #[doc = ""]
    #[doc = " This is similar to `posix_fadvise` in POSIX."]
    fn advise(&self, offset: Filesize, length: Filesize, advice: Advice) -> Result<(), ErrorCode> {
        let advice = advice_map_in(advice);

        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::Advise(latch::DescriptorAdviseArgs {
                offset,
                length,
                advice,
            }),
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.advise FD={self} OFFSET={offset} LENGTH={length} ADVICE={advice}");
                Err(error_code_map(code))
            }
            _ => self
                .fd
                .advise(offset, length, advice)
                .map_err(error_code_map),
        }
    }

    #[doc = " Synchronize the data of a file to disk."]
    #[doc = ""]
    #[doc = " This function succeeds with no effect if the file descriptor is not"]
    #[doc = " opened for writing."]
    #[doc = ""]
    #[doc = " Note: This is similar to `fdatasync` in POSIX."]
    fn sync_data(&self) -> Result<(), ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::SyncData,
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.sync-data FD={self}");
                Err(error_code_map(code))
            }
            _ => self.fd.sync_data().map_err(error_code_map),
        }
    }

    #[doc = " Get flags associated with a descriptor."]
    #[doc = ""]
    #[doc = " Note: This returns similar flags to `fcntl(fd, F_GETFL)` in POSIX."]
    #[doc = ""]
    #[doc = " Note: This returns the value that was the `fs_flags` value returned"]
    #[doc = " from `fdstat_get` in earlier versions of WASI."]
    fn get_flags(&self) -> Result<DescriptorFlags, ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::GetFlags,
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.get-flags FD={self}");
                Err(error_code_map(code))
            }
            _ => self
                .fd
                .get_flags()
                .map(descriptor_flags_map)
                .map_err(error_code_map),
        }
    }

    #[doc = " Get the dynamic type of a descriptor."]
    #[doc = ""]
    #[doc = " Note: This returns the same value as the `type` field of the `fd-stat`"]
    #[doc = " returned by `stat`, `stat-at` and similar."]
    #[doc = ""]
    #[doc = " Note: This returns similar flags to the `st_mode & S_IFMT` value provided"]
    #[doc = " by `fstat` in POSIX."]
    #[doc = ""]
    #[doc = " Note: This returns the value that was the `fs_filetype` value returned"]
    #[doc = " from `fdstat_get` in earlier versions of WASI."]
    fn get_type(&self) -> Result<DescriptorType, ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::GetType,
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.get-type FD={self}");
                Err(error_code_map(code))
            }
            _ => self
                .fd
                .get_type()
                .map(descriptor_type_map)
                .map_err(error_code_map),
        }
    }

    #[doc = " Adjust the size of an open file. If this increases the file\'s size, the"]
    #[doc = " extra bytes are filled with zeros."]
    #[doc = ""]
    #[doc = " Note: This was called `fd_filestat_set_size` in earlier versions of WASI."]
    fn set_size(&self, size: Filesize) -> Result<(), ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::SetSize(latch::DescriptorSetSizeArgs { size }),
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.set-size FD={self} SIZE={size}");
                Err(error_code_map(code))
            }
            _ => self.fd.set_size(size).map_err(error_code_map),
        }
    }

    #[doc = " Adjust the timestamps of an open file or directory."]
    #[doc = ""]
    #[doc = " Note: This is similar to `futimens` in POSIX."]
    #[doc = ""]
    #[doc = " Note: This was called `fd_filestat_set_times` in earlier versions of WASI."]
    fn set_times(
        &self,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), ErrorCode> {
        let data_access_timestamp = new_timestamp_map_in(data_access_timestamp);
        let data_modification_timestamp = new_timestamp_map_in(data_modification_timestamp);

        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::SetTimes(latch::DescriptorSetTimesArgs {
                data_access_timestamp,
                data_modification_timestamp,
            }),
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.set-times FD={self} ACCESS-TIME={data_access_timestamp:?} MODIFIED-TIME={data_modification_timestamp:?}");
                Err(error_code_map(code))
            }
            _ => self
                .fd
                .set_times(data_access_timestamp, data_modification_timestamp)
                .map_err(error_code_map),
        }
    }

    #[doc = " Read from a descriptor, without using and updating the descriptor\'s offset."]
    #[doc = ""]
    #[doc = " This function returns a list of bytes containing the data that was"]
    #[doc = " read, along with a bool which, when true, indicates that the end of the"]
    #[doc = " file was reached. The returned list will contain up to `length` bytes; it"]
    #[doc = " may return fewer than requested, if the end of the file is reached or"]
    #[doc = " if the I/O operation is interrupted."]
    #[doc = ""]
    #[doc = " In the future, this may change to return a `stream<u8, error-code>`."]
    #[doc = ""]
    #[doc = " Note: This is similar to `pread` in POSIX."]
    fn read(&self, length: Filesize, offset: Filesize) -> Result<(Vec<u8>, bool), ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::Read(latch::DescriptorReadArgs { length, offset }),
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.read FD={self} LENGTH={length} OFFSET={offset}");
                Err(error_code_map(code))
            }
            _ => self.fd.read(length, offset).map_err(error_code_map),
        }
    }

    #[doc = " Write to a descriptor, without using and updating the descriptor\'s offset."]
    #[doc = ""]
    #[doc = " It is valid to write past the end of a file; the file is extended to the"]
    #[doc = " extent of the write, with bytes between the previous end and the start of"]
    #[doc = " the write set to zero."]
    #[doc = ""]
    #[doc = " In the future, this may change to take a `stream<u8, error-code>`."]
    #[doc = ""]
    #[doc = " Note: This is similar to `pwrite` in POSIX."]
    fn write(&self, buffer: Vec<u8>, offset: Filesize) -> Result<Filesize, ErrorCode> {
        let buffer_length: u64 = buffer
            .len()
            .try_into()
            .expect("buffer length 64-bits or less");
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::Write(latch::DescriptorWriteArgs {
                buffer_length,
                offset,
            }),
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.write FD={self} BUFFER-LENGTH={buffer_length} OFFSET={offset}");
                Err(error_code_map(code))
            }
            _ => self.fd.write(&buffer, offset).map_err(error_code_map),
        }
    }

    #[doc = " Read directory entries from a directory."]
    #[doc = ""]
    #[doc = " On filesystems where directories contain entries referring to themselves"]
    #[doc = " and their parents, often named `.` and `..` respectively, these entries"]
    #[doc = " are omitted."]
    #[doc = ""]
    #[doc = " This always returns a new stream which starts at the beginning of the"]
    #[doc = " directory. Multiple streams may be active on the same directory, and they"]
    #[doc = " do not interfere with each other."]
    fn read_directory(&self) -> Result<DirectoryEntryStream, ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::ReadDirectory,
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.read-directory FD={self}");
                Err(error_code_map(code))
            }
            _ => self
                .fd
                .read_directory()
                .map(|des| directory_entry_stream_map(des, self.path.clone()))
                .map_err(error_code_map),
        }
    }

    #[doc = " Synchronize the data and metadata of a file to disk."]
    #[doc = ""]
    #[doc = " This function succeeds with no effect if the file descriptor is not"]
    #[doc = " opened for writing."]
    #[doc = ""]
    #[doc = " Note: This is similar to `fsync` in POSIX."]
    fn sync(&self) -> Result<(), ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::Sync,
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.sync FD={self}");
                Err(error_code_map(code))
            }
            _ => self.fd.sync().map_err(error_code_map),
        }
    }

    #[doc = " Create a directory."]
    #[doc = ""]
    #[doc = " Note: This is similar to `mkdirat` in POSIX."]
    fn create_directory_at(&self, path: String) -> Result<(), ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::CreateDirectoryAt(latch::DescriptorCreateDirectoryAtArgs {
                path: path.clone(),
            }),
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.create-directory-at FD={self} PATH={path}");
                Err(error_code_map(code))
            }
            _ => self.fd.create_directory_at(&path).map_err(error_code_map),
        }
    }

    #[doc = " Return the attributes of an open file or directory."]
    #[doc = ""]
    #[doc = " Note: This is similar to `fstat` in POSIX, except that it does not return"]
    #[doc = " device and inode information. For testing whether two descriptors refer to"]
    #[doc = " the same underlying filesystem object, use `is-same-object`. To obtain"]
    #[doc = " additional data that can be used do determine whether a file has been"]
    #[doc = " modified, use `metadata-hash`."]
    #[doc = ""]
    #[doc = " Note: This was called `fd_filestat_get` in earlier versions of WASI."]
    fn stat(&self) -> Result<DescriptorStat, ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::Stat,
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.stat FD={self}");
                Err(error_code_map(code))
            }
            _ => self
                .fd
                .stat()
                .map(descriptor_stat_map)
                .map_err(error_code_map),
        }
    }

    #[doc = " Return the attributes of a file or directory."]
    #[doc = ""]
    #[doc = " Note: This is similar to `fstatat` in POSIX, except that it does not"]
    #[doc = " return device and inode information. See the `stat` description for a"]
    #[doc = " discussion of alternatives."]
    #[doc = ""]
    #[doc = " Note: This was called `path_filestat_get` in earlier versions of WASI."]
    fn stat_at(&self, path_flags: PathFlags, path: String) -> Result<DescriptorStat, ErrorCode> {
        let path_flags = types::PathFlags::from_bits(path_flags.bits()).unwrap();

        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::StatAt(latch::DescriptorStatAtArgs {
                path_flags,
                path: path.clone(),
            }),
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.stat-at FD={self} PATH-FLAGS={path_flags} PATH={path}");
                Err(error_code_map(code))
            }
            _ => self
                .fd
                .stat_at(path_flags, &path)
                .map(descriptor_stat_map)
                .map_err(error_code_map),
        }
    }

    #[doc = " Adjust the timestamps of a file or directory."]
    #[doc = ""]
    #[doc = " Note: This is similar to `utimensat` in POSIX."]
    #[doc = ""]
    #[doc = " Note: This was called `path_filestat_set_times` in earlier versions of"]
    #[doc = " WASI."]
    fn set_times_at(
        &self,
        path_flags: PathFlags,
        path: String,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), ErrorCode> {
        let path_flags = types::PathFlags::from_bits(path_flags.bits()).unwrap();
        let data_access_timestamp = new_timestamp_map_in(data_access_timestamp);
        let data_modification_timestamp = new_timestamp_map_in(data_modification_timestamp);
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
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.set-times-at FD={self} PATH-FLAGS={path_flags} PATH={path} ACCESS-TIME={data_access_timestamp:?} MODIFIED-TIME={data_modification_timestamp:?}");
                Err(error_code_map(code))
            }
            _ => self
                .fd
                .set_times_at(
                    path_flags,
                    &path,
                    data_access_timestamp,
                    data_modification_timestamp,
                )
                .map_err(error_code_map),
        }
    }

    #[doc = " Create a hard link."]
    #[doc = ""]
    #[doc = " Note: This is similar to `linkat` in POSIX."]
    fn link_at(
        &self,
        old_path_flags: PathFlags,
        old_path: String,
        new_descriptor: DescriptorBorrow<'_>,
        new_path: String,
    ) -> Result<(), ErrorCode> {
        let old_path_flags = types::PathFlags::from_bits(old_path_flags.bits()).unwrap();
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::LinkAt(latch::DescriptorLinkAtArgs {
                old_path_flags,
                old_path: old_path.clone(),
                new_descriptor: &new_descriptor.get::<Self>().fd,
                new_path: new_path.clone(),
            }),
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!(
                    "Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.link-at FD={self} OLD-PATH={old_path} OLD-PATH-FLAGS={old_path_flags} NEW-PATH={new_path}",
                );
                Err(error_code_map(code))
            }
            _ => self
                .fd
                .link_at(
                    old_path_flags,
                    &old_path,
                    &new_descriptor.get::<Self>().fd,
                    &new_path,
                )
                .map_err(error_code_map),
        }
    }

    #[doc = " Open a file or directory."]
    #[doc = ""]
    #[doc = " The returned descriptor is not guaranteed to be the lowest-numbered"]
    #[doc = " descriptor not currently open/ it is randomized to prevent applications"]
    #[doc = " from depending on making assumptions about indexes, since this is"]
    #[doc = " error-prone in multi-threaded contexts. The returned descriptor is"]
    #[doc = " guaranteed to be less than 2**31."]
    #[doc = ""]
    #[doc = " If `flags` contains `descriptor-flags::mutate-directory`, and the base"]
    #[doc = " descriptor doesn\'t have `descriptor-flags::mutate-directory` set,"]
    #[doc = " `open-at` fails with `error-code::read-only`."]
    #[doc = ""]
    #[doc = " If `flags` contains `write` or `mutate-directory`, or `open-flags`"]
    #[doc = " contains `truncate` or `create`, and the base descriptor doesn\'t have"]
    #[doc = " `descriptor-flags::mutate-directory` set, `open-at` fails with"]
    #[doc = " `error-code::read-only`."]
    #[doc = ""]
    #[doc = " Note: This is similar to `openat` in POSIX."]
    fn open_at(
        &self,
        path_flags: PathFlags,
        path: String,
        open_flags: OpenFlags,
        flags: DescriptorFlags,
    ) -> Result<Descriptor, ErrorCode> {
        let path_flags = types::PathFlags::from_bits(path_flags.bits()).unwrap();
        let open_flags = types::OpenFlags::from_bits(open_flags.bits()).unwrap();
        let flags = types::DescriptorFlags::from_bits(flags.bits()).unwrap();
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
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.open-at FD={self} PATH-FLAGS={path_flags} PATH={path} OPEN-FLAGS={open_flags} FLAGS={flags}");
                Err(error_code_map(code))
            }
            _ => self
                .fd
                .open_at(path_flags, &path.clone(), open_flags, flags)
                .map(|descriptor| descriptor_map(descriptor, self.path.join(path)))
                .map_err(error_code_map),
        }
    }

    #[doc = " Read the contents of a symbolic link."]
    #[doc = ""]
    #[doc = " If the contents contain an absolute or rooted path in the underlying"]
    #[doc = " filesystem, this function fails with `error-code::not-permitted`."]
    #[doc = ""]
    #[doc = " Note: This is similar to `readlinkat` in POSIX."]
    fn readlink_at(&self, path: String) -> Result<String, ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::ReadlinkAt(latch::DescriptorReadlinkAtArgs { path: path.clone() }),
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!(
                    "Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.readlink-at FD={self} PATH={path}",
                );
                Err(error_code_map(code))
            }
            _ => self.fd.readlink_at(&path).map_err(error_code_map),
        }
    }

    #[doc = " Remove a directory."]
    #[doc = ""]
    #[doc = " Return `error-code::not-empty` if the directory is not empty."]
    #[doc = ""]
    #[doc = " Note: This is similar to `unlinkat(fd, path, AT_REMOVEDIR)` in POSIX."]
    fn remove_directory_at(&self, path: String) -> Result<(), ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::RemoveDirectoryAt(latch::DescriptorRemoveDirectoryAtArgs {
                path: path.clone(),
            }),
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.remove-directory-at FD={self} PATH={path}");
                Err(error_code_map(code))
            }
            _ => self.fd.remove_directory_at(&path).map_err(error_code_map),
        }
    }

    #[doc = " Rename a filesystem object."]
    #[doc = ""]
    #[doc = " Note: This is similar to `renameat` in POSIX."]
    fn rename_at(
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
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!(
                    "Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.rename-at FD={self} OLD-PATH={old_path} NEW-PATH={new_path}",
                );
                Err(error_code_map(code))
            }
            _ => {
                let new_descriptor: &Self = new_descriptor.get();
                self.fd
                    .rename_at(&old_path, &new_descriptor.fd, &new_path)
                    .map_err(error_code_map)
            }
        }
    }

    #[doc = " Create a symbolic link (also known as a \"symlink\")."]
    #[doc = ""]
    #[doc = " If `old-path` starts with `/`, the function fails with"]
    #[doc = " `error-code::not-permitted`."]
    #[doc = ""]
    #[doc = " Note: This is similar to `symlinkat` in POSIX."]
    fn symlink_at(&self, old_path: String, new_path: String) -> Result<(), ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::SymlinkAt(latch::DescriptorSymlinkAtArgs {
                old_path: old_path.clone(),
                new_path: new_path.clone(),
            }),
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!(
                    "Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.symlink-at FD={self} OLD-PATH={old_path} NEW-PATH={new_path}",
                );
                Err(error_code_map(code))
            }
            _ => self
                .fd
                .symlink_at(&old_path, &new_path)
                .map_err(error_code_map),
        }
    }

    #[doc = " Unlink a filesystem object that is not a directory."]
    #[doc = ""]
    #[doc = " Return `error-code::is-directory` if the path refers to a directory."]
    #[doc = " Note: This is similar to `unlinkat(fd, path, 0)` in POSIX."]
    fn unlink_file_at(&self, path: String) -> Result<(), ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::UnlinkFileAt(latch::DescriptorUnlinkFileAtArgs {
                path: path.clone(),
            }),
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!(
                    "Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.unlink-file-at FD={self} PATH={path}",
                );
                Err(error_code_map(code))
            }
            _ => self.fd.unlink_file_at(&path).map_err(error_code_map),
        }
    }

    #[doc = " Test whether two descriptors refer to the same filesystem object."]
    #[doc = ""]
    #[doc = " In POSIX, this corresponds to testing whether the two descriptors have the"]
    #[doc = " same device (`st_dev`) and inode (`st_ino` or `d_ino`) numbers."]
    #[doc = " wasi-filesystem does not expose device and inode numbers, so this function"]
    #[doc = " may be used instead."]
    fn is_same_object(&self, other: DescriptorBorrow<'_>) -> bool {
        let other: &Self = other.get();
        self.fd.is_same_object(&other.fd)
    }

    #[doc = " Return a hash of the metadata associated with a filesystem object referred"]
    #[doc = " to by a descriptor."]
    #[doc = ""]
    #[doc = " This returns a hash of the last-modification timestamp and file size, and"]
    #[doc = " may also include the inode number, device number, birth timestamp, and"]
    #[doc = " other metadata fields that may change when the file is modified or"]
    #[doc = " replaced. It may also include a secret value chosen by the"]
    #[doc = " implementation and not otherwise exposed."]
    #[doc = ""]
    #[doc = " Implementations are encourated to provide the following properties:"]
    #[doc = ""]
    #[doc = " - If the file is not modified or replaced, the computed hash value should"]
    #[doc = " usually not change."]
    #[doc = " - If the object is modified or replaced, the computed hash value should"]
    #[doc = " usually change."]
    #[doc = " - The inputs to the hash should not be easily computable from the"]
    #[doc = " computed hash."]
    #[doc = ""]
    #[doc = " However, none of these is required."]
    fn metadata_hash(&self) -> Result<MetadataHashValue, ErrorCode> {
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::MetadataHash,
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.metadata-hash FD={self}");
                Err(error_code_map(code))
            }
            _ => self
                .fd
                .metadata_hash()
                .map(metadata_hash_value_map)
                .map_err(error_code_map),
        }
    }

    #[doc = " Return a hash of the metadata associated with a filesystem object referred"]
    #[doc = " to by a directory descriptor and a relative path."]
    #[doc = ""]
    #[doc = " This performs the same hash computation as `metadata-hash`."]
    fn metadata_hash_at(
        &self,
        path_flags: PathFlags,
        path: String,
    ) -> Result<MetadataHashValue, ErrorCode> {
        let path_flags = types::PathFlags::from_bits(path_flags.bits()).unwrap();
        match authorize(&Operation::Descriptor((
            &self.fd,
            self.path.to_string_lossy().into_owned(),
            DescriptorOperation::MetadataHashAt(latch::DescriptorMetadataHashAtArgs {
                path_flags,
                path: path.clone(),
            }),
        ))) {
            Some(Denied(code)) => {
                let reason = error_code_display(code);
                warn!("Denied REASON={reason} OPERATION=wasi:filesystem/types#descriptor.metadata-hash-at FD={self} PATH={path} PATH-FLAGS={path_flags}");
                Err(error_code_map(code))
            }
            _ => self
                .fd
                .metadata_hash_at(path_flags, &path)
                .map(metadata_hash_value_map)
                .map_err(error_code_map),
        }
    }
}

#[derive(Debug, Clone)]
struct GatedDirectoryEntryStream {
    des: Rc<types::DirectoryEntryStream>,
    path: PathBuf,
}

impl GatedDirectoryEntryStream {
    fn new(des: types::DirectoryEntryStream, path: PathBuf) -> Self {
        Self {
            des: Rc::new(des),
            path,
        }
    }
}

impl Display for GatedDirectoryEntryStream {
    fn fmt(&self, d: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        d.write_fmt(format_args!("{}", self.path.display()))
    }
}

impl exports::wasi::filesystem::types::GuestDirectoryEntryStream for GatedDirectoryEntryStream {
    #[doc = " Read a single directory entry from a `directory-entry-stream`."]
    fn read_directory_entry(&self) -> Result<Option<DirectoryEntry>, ErrorCode> {
        match self.des.read_directory_entry() {
            Ok(Some(de)) => {
                match authorize(&Operation::DirectoryEntryStream((
                    &self.des,
                    self.path.to_string_lossy().into_owned(),
                    DirectoryEntryStreamOperation::ReadDirectoryEntry(de.clone()),
                ))) {
                    Some(Denied(code)) => {
                        let reason = error_code_display(code);
                        trace!("Denied REASON={reason} OPERATION=wasi:filesystem/types#directory-entry-stream.read-directory-entry STREAM={} ENTRY={}", self, de.name);
                        // continue reading the next entry transparently
                        self.read_directory_entry()
                    }
                    _ => Ok(Some(directory_entry_map(de))),
                }
            }
            Ok(None) => Ok(None),
            Err(code) => Err(error_code_map(code)),
        }
    }
}

fn advice_map_in(advice: Advice) -> types::Advice {
    match advice {
        Advice::Normal => types::Advice::Normal,
        Advice::Sequential => types::Advice::Sequential,
        Advice::Random => types::Advice::Random,
        Advice::WillNeed => types::Advice::WillNeed,
        Advice::DontNeed => types::Advice::DontNeed,
        Advice::NoReuse => types::Advice::NoReuse,
    }
}

fn descriptor_map(descriptor: types::Descriptor, path: PathBuf) -> Descriptor {
    Descriptor::new(GatedFileDescriptor::new(descriptor, path))
}

fn descriptor_flags_map(descriptor_flags: types::DescriptorFlags) -> DescriptorFlags {
    DescriptorFlags::from_bits(descriptor_flags.bits()).unwrap()
}

fn descriptor_stat_map(descriptor_stat: types::DescriptorStat) -> DescriptorStat {
    DescriptorStat {
        type_: descriptor_type_map(descriptor_stat.type_),
        link_count: descriptor_stat.link_count,
        size: descriptor_stat.size,
        data_access_timestamp: descriptor_stat.data_access_timestamp,
        data_modification_timestamp: descriptor_stat.data_modification_timestamp,
        status_change_timestamp: descriptor_stat.status_change_timestamp,
    }
}

fn descriptor_type_map(descriptor_type: types::DescriptorType) -> DescriptorType {
    match descriptor_type {
        types::DescriptorType::Unknown => DescriptorType::Unknown,
        types::DescriptorType::BlockDevice => DescriptorType::BlockDevice,
        types::DescriptorType::CharacterDevice => DescriptorType::CharacterDevice,
        types::DescriptorType::Directory => DescriptorType::Directory,
        types::DescriptorType::Fifo => DescriptorType::Fifo,
        types::DescriptorType::SymbolicLink => DescriptorType::SymbolicLink,
        types::DescriptorType::RegularFile => DescriptorType::RegularFile,
        types::DescriptorType::Socket => DescriptorType::Socket,
    }
}

fn directory_entry_map(directory_entry: types::DirectoryEntry) -> DirectoryEntry {
    DirectoryEntry {
        name: directory_entry.name,
        type_: descriptor_type_map(directory_entry.type_),
    }
}

fn directory_entry_stream_map(
    directory_entry_stream: types::DirectoryEntryStream,
    path: PathBuf,
) -> DirectoryEntryStream {
    DirectoryEntryStream::new(GatedDirectoryEntryStream::new(directory_entry_stream, path))
}

fn error_code_map(error_code: types::ErrorCode) -> ErrorCode {
    match error_code {
        types::ErrorCode::Access => ErrorCode::Access,
        types::ErrorCode::WouldBlock => ErrorCode::WouldBlock,
        types::ErrorCode::Already => ErrorCode::Already,
        types::ErrorCode::BadDescriptor => ErrorCode::BadDescriptor,
        types::ErrorCode::Busy => ErrorCode::Busy,
        types::ErrorCode::Deadlock => ErrorCode::Deadlock,
        types::ErrorCode::Quota => ErrorCode::Quota,
        types::ErrorCode::Exist => ErrorCode::Exist,
        types::ErrorCode::FileTooLarge => ErrorCode::FileTooLarge,
        types::ErrorCode::IllegalByteSequence => ErrorCode::IllegalByteSequence,
        types::ErrorCode::InProgress => ErrorCode::InProgress,
        types::ErrorCode::Interrupted => ErrorCode::Interrupted,
        types::ErrorCode::Invalid => ErrorCode::Invalid,
        types::ErrorCode::Io => ErrorCode::Io,
        types::ErrorCode::IsDirectory => ErrorCode::IsDirectory,
        types::ErrorCode::Loop => ErrorCode::Loop,
        types::ErrorCode::TooManyLinks => ErrorCode::TooManyLinks,
        types::ErrorCode::MessageSize => ErrorCode::MessageSize,
        types::ErrorCode::NameTooLong => ErrorCode::NameTooLong,
        types::ErrorCode::NoDevice => ErrorCode::NoDevice,
        types::ErrorCode::NoEntry => ErrorCode::NoEntry,
        types::ErrorCode::NoLock => ErrorCode::NoLock,
        types::ErrorCode::InsufficientMemory => ErrorCode::InsufficientMemory,
        types::ErrorCode::InsufficientSpace => ErrorCode::InsufficientSpace,
        types::ErrorCode::NotDirectory => ErrorCode::NotDirectory,
        types::ErrorCode::NotEmpty => ErrorCode::NotEmpty,
        types::ErrorCode::NotRecoverable => ErrorCode::NotRecoverable,
        types::ErrorCode::Unsupported => ErrorCode::Unsupported,
        types::ErrorCode::NoTty => ErrorCode::NoTty,
        types::ErrorCode::NoSuchDevice => ErrorCode::NoSuchDevice,
        types::ErrorCode::Overflow => ErrorCode::Overflow,
        types::ErrorCode::NotPermitted => ErrorCode::NotPermitted,
        types::ErrorCode::Pipe => ErrorCode::Pipe,
        types::ErrorCode::ReadOnly => ErrorCode::ReadOnly,
        types::ErrorCode::InvalidSeek => ErrorCode::InvalidSeek,
        types::ErrorCode::TextFileBusy => ErrorCode::TextFileBusy,
        types::ErrorCode::CrossDevice => ErrorCode::CrossDevice,
    }
}

fn error_code_display(error_code: types::ErrorCode) -> String {
    error_code
        .to_string()
        .splitn(2, ' ')
        .next()
        .unwrap_or("")
        .to_kebab_case()
}

fn metadata_hash_value_map(metadata_hash_value: types::MetadataHashValue) -> MetadataHashValue {
    MetadataHashValue {
        lower: metadata_hash_value.lower,
        upper: metadata_hash_value.upper,
    }
}

fn new_timestamp_map_in(timestamp: NewTimestamp) -> types::NewTimestamp {
    match timestamp {
        NewTimestamp::NoChange => types::NewTimestamp::NoChange,
        NewTimestamp::Now => types::NewTimestamp::Now,
        NewTimestamp::Timestamp(dt) => types::NewTimestamp::Timestamp(dt),
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

wit_bindgen::generate!({
    path: "../../wit",
    world: "filesystem",
    generate_all
});

export!(GatedFilesystem);
