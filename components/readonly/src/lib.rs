#![no_main]

use std::rc::Rc;

use exports::wasi::filesystem::preopens::Guest as Preopens;
use exports::wasi::filesystem::types::{
    Advice, Descriptor, DescriptorBorrow, DescriptorFlags, DescriptorStat, DescriptorType,
    DirectoryEntry, DirectoryEntryStream, Error, ErrorCode, Filesize, Guest as Types, InputStream,
    MetadataHashValue, NewTimestamp, OpenFlags, OutputStream, PathFlags,
};
use wasi::filesystem::preopens;
use wasi::filesystem::types;

#[derive(Debug, Clone)]
struct FilesystemReadOnly {}

impl Preopens for FilesystemReadOnly {
    #[doc = " Return the set of preopened directories, and their path."]
    fn get_directories() -> Vec<(Descriptor, String)> {
        preopens::get_directories()
            .into_iter()
            .map(|(fd, path)| {
                let fd = Descriptor::new(ReadOnlyDescriptor::new(fd));
                (fd, path)
            })
            .collect()
    }
}

impl Types for FilesystemReadOnly {
    type Descriptor = ReadOnlyDescriptor;
    type DirectoryEntryStream = ReadOnlyDirectoryEntryStream;

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
struct ReadOnlyDescriptor {
    fd: Rc<types::Descriptor>,
}

impl ReadOnlyDescriptor {
    fn new(fd: types::Descriptor) -> Self {
        Self { fd: Rc::new(fd) }
    }
}

impl exports::wasi::filesystem::types::GuestDescriptor for ReadOnlyDescriptor {
    #[doc = " Return a stream for reading from a file, if available."]
    #[doc = ""]
    #[doc = " May fail with an error-code describing why the file cannot be read."]
    #[doc = ""]
    #[doc = " Multiple read, write, and append streams may be active on the same open"]
    #[doc = " file and they do not interfere with each other."]
    #[doc = ""]
    #[doc = " Note: This allows using `read-stream`, which is similar to `read` in POSIX."]
    fn read_via_stream(&self, offset: Filesize) -> Result<InputStream, ErrorCode> {
        self.fd.read_via_stream(offset).map_err(error_code_map)
    }

    #[doc = " Return a stream for writing to a file, if available."]
    #[doc = ""]
    #[doc = " May fail with an error-code describing why the file cannot be written."]
    #[doc = ""]
    #[doc = " Note: This allows using `write-stream`, which is similar to `write` in"]
    #[doc = " POSIX."]
    fn write_via_stream(&self, _offset: Filesize) -> Result<OutputStream, ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    #[doc = " Return a stream for appending to a file, if available."]
    #[doc = ""]
    #[doc = " May fail with an error-code describing why the file cannot be appended."]
    #[doc = ""]
    #[doc = " Note: This allows using `write-stream`, which is similar to `write` with"]
    #[doc = " `O_APPEND` in in POSIX."]
    fn append_via_stream(&self) -> Result<OutputStream, ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    #[doc = " Provide file advisory information on a descriptor."]
    #[doc = ""]
    #[doc = " This is similar to `posix_fadvise` in POSIX."]
    fn advise(&self, offset: Filesize, length: Filesize, advice: Advice) -> Result<(), ErrorCode> {
        let advice = advice_map_in(advice);
        self.fd
            .advise(offset, length, advice)
            .map_err(error_code_map)
    }

    #[doc = " Synchronize the data of a file to disk."]
    #[doc = ""]
    #[doc = " This function succeeds with no effect if the file descriptor is not"]
    #[doc = " opened for writing."]
    #[doc = ""]
    #[doc = " Note: This is similar to `fdatasync` in POSIX."]
    fn sync_data(&self) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    #[doc = " Get flags associated with a descriptor."]
    #[doc = ""]
    #[doc = " Note: This returns similar flags to `fcntl(fd, F_GETFL)` in POSIX."]
    #[doc = ""]
    #[doc = " Note: This returns the value that was the `fs_flags` value returned"]
    #[doc = " from `fdstat_get` in earlier versions of WASI."]
    fn get_flags(&self) -> Result<DescriptorFlags, ErrorCode> {
        self.fd
            .get_flags()
            .map(descriptor_flags_map)
            .map(|flags| {
                flags
                    .difference(DescriptorFlags::WRITE)
                    .difference(DescriptorFlags::FILE_INTEGRITY_SYNC)
                    .difference(DescriptorFlags::DATA_INTEGRITY_SYNC)
                    .difference(DescriptorFlags::MUTATE_DIRECTORY)
            })
            .map_err(error_code_map)
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
        self.fd
            .get_type()
            .map(descriptor_type_map)
            .map_err(error_code_map)
    }

    #[doc = " Adjust the size of an open file. If this increases the file\'s size, the"]
    #[doc = " extra bytes are filled with zeros."]
    #[doc = ""]
    #[doc = " Note: This was called `fd_filestat_set_size` in earlier versions of WASI."]
    fn set_size(&self, _size: Filesize) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    #[doc = " Adjust the timestamps of an open file or directory."]
    #[doc = ""]
    #[doc = " Note: This is similar to `futimens` in POSIX."]
    #[doc = ""]
    #[doc = " Note: This was called `fd_filestat_set_times` in earlier versions of WASI."]
    fn set_times(
        &self,
        _data_access_timestamp: NewTimestamp,
        _data_modification_timestamp: NewTimestamp,
    ) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
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
        self.fd.read(length, offset).map_err(error_code_map)
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
    fn write(&self, _buffer: Vec<u8>, _offset: Filesize) -> Result<Filesize, ErrorCode> {
        Err(ErrorCode::ReadOnly)
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
        self.fd
            .read_directory()
            .map(directory_entry_stream_map)
            .map_err(error_code_map)
    }

    #[doc = " Synchronize the data and metadata of a file to disk."]
    #[doc = ""]
    #[doc = " This function succeeds with no effect if the file descriptor is not"]
    #[doc = " opened for writing."]
    #[doc = ""]
    #[doc = " Note: This is similar to `fsync` in POSIX."]
    fn sync(&self) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    #[doc = " Create a directory."]
    #[doc = ""]
    #[doc = " Note: This is similar to `mkdirat` in POSIX."]
    fn create_directory_at(&self, _path: String) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
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
        self.fd
            .stat()
            .map(descriptor_stat_map)
            .map_err(error_code_map)
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
        self.fd
            .stat_at(path_flags, &path)
            .map(descriptor_stat_map)
            .map_err(error_code_map)
    }

    #[doc = " Adjust the timestamps of a file or directory."]
    #[doc = ""]
    #[doc = " Note: This is similar to `utimensat` in POSIX."]
    #[doc = ""]
    #[doc = " Note: This was called `path_filestat_set_times` in earlier versions of"]
    #[doc = " WASI."]
    fn set_times_at(
        &self,
        _path_flags: PathFlags,
        _path: String,
        _data_access_timestamp: NewTimestamp,
        _data_modification_timestamp: NewTimestamp,
    ) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    #[doc = " Create a hard link."]
    #[doc = ""]
    #[doc = " Note: This is similar to `linkat` in POSIX."]
    fn link_at(
        &self,
        _old_path_flags: PathFlags,
        _old_path: String,
        _new_descriptor: DescriptorBorrow<'_>,
        _new_path: String,
    ) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
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
        if open_flags.contains(OpenFlags::CREATE)
            || open_flags.contains(OpenFlags::EXCLUSIVE)
            || open_flags.contains(OpenFlags::TRUNCATE)
            || flags.contains(DescriptorFlags::WRITE)
            || flags.contains(DescriptorFlags::FILE_INTEGRITY_SYNC)
            || flags.contains(DescriptorFlags::DATA_INTEGRITY_SYNC)
            || flags.contains(DescriptorFlags::REQUESTED_WRITE_SYNC)
        {
            return Err(ErrorCode::ReadOnly);
        }

        let path_flags = types::PathFlags::from_bits(path_flags.bits()).unwrap();
        let open_flags = types::OpenFlags::from_bits(open_flags.bits()).unwrap();
        let flags = types::DescriptorFlags::from_bits(flags.bits()).unwrap();
        self.fd
            .open_at(path_flags, &path, open_flags, flags)
            .map(descriptor_map)
            .map_err(error_code_map)
    }

    #[doc = " Read the contents of a symbolic link."]
    #[doc = ""]
    #[doc = " If the contents contain an absolute or rooted path in the underlying"]
    #[doc = " filesystem, this function fails with `error-code::not-permitted`."]
    #[doc = ""]
    #[doc = " Note: This is similar to `readlinkat` in POSIX."]
    fn readlink_at(&self, path: String) -> Result<String, ErrorCode> {
        self.fd.readlink_at(&path).map_err(error_code_map)
    }

    #[doc = " Remove a directory."]
    #[doc = ""]
    #[doc = " Return `error-code::not-empty` if the directory is not empty."]
    #[doc = ""]
    #[doc = " Note: This is similar to `unlinkat(fd, path, AT_REMOVEDIR)` in POSIX."]
    fn remove_directory_at(&self, _path: String) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    #[doc = " Rename a filesystem object."]
    #[doc = ""]
    #[doc = " Note: This is similar to `renameat` in POSIX."]
    fn rename_at(
        &self,
        _old_path: String,
        _new_descriptor: DescriptorBorrow<'_>,
        _new_path: String,
    ) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    #[doc = " Create a symbolic link (also known as a \"symlink\")."]
    #[doc = ""]
    #[doc = " If `old-path` starts with `/`, the function fails with"]
    #[doc = " `error-code::not-permitted`."]
    #[doc = ""]
    #[doc = " Note: This is similar to `symlinkat` in POSIX."]
    fn symlink_at(&self, _old_path: String, _new_path: String) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    #[doc = " Unlink a filesystem object that is not a directory."]
    #[doc = ""]
    #[doc = " Return `error-code::is-directory` if the path refers to a directory."]
    #[doc = " Note: This is similar to `unlinkat(fd, path, 0)` in POSIX."]
    fn unlink_file_at(&self, _path: String) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
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
        self.fd
            .metadata_hash()
            .map(metadata_hash_value_map)
            .map_err(error_code_map)
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
        self.fd
            .metadata_hash_at(path_flags, &path)
            .map(metadata_hash_value_map)
            .map_err(error_code_map)
    }
}

#[derive(Debug, Clone)]
struct ReadOnlyDirectoryEntryStream {
    des: Rc<types::DirectoryEntryStream>,
}

impl ReadOnlyDirectoryEntryStream {
    fn new(des: types::DirectoryEntryStream) -> Self {
        Self { des: Rc::new(des) }
    }
}

impl exports::wasi::filesystem::types::GuestDirectoryEntryStream for ReadOnlyDirectoryEntryStream {
    #[doc = " Read a single directory entry from a `directory-entry-stream`."]
    fn read_directory_entry(&self) -> Result<Option<DirectoryEntry>, ErrorCode> {
        self.des
            .read_directory_entry()
            .map(|de| de.map(directory_entry_map))
            .map_err(error_code_map)
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

fn descriptor_map(descriptor: types::Descriptor) -> Descriptor {
    Descriptor::new(ReadOnlyDescriptor::new(descriptor))
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
) -> DirectoryEntryStream {
    DirectoryEntryStream::new(ReadOnlyDirectoryEntryStream::new(directory_entry_stream))
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

fn metadata_hash_value_map(metadata_hash_value: types::MetadataHashValue) -> MetadataHashValue {
    MetadataHashValue {
        lower: metadata_hash_value.lower,
        upper: metadata_hash_value.upper,
    }
}

wit_bindgen::generate!({
    path: "../../wit",
    world: "filesystem",
    generate_all
});

export!(FilesystemReadOnly);
