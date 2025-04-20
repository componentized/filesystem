#![no_main]

use exports::wasi::filesystem::preopens::Guest as Preopens;
use exports::wasi::filesystem::types::{
    Advice, Descriptor, DescriptorBorrow, DescriptorFlags, DescriptorStat, DescriptorType,
    DirectoryEntry, DirectoryEntryStream, Error, ErrorCode, Filesize, Guest as Types,
    GuestDescriptor, GuestDirectoryEntryStream, InputStream, MetadataHashValue, NewTimestamp,
    OpenFlags, OutputStream, PathFlags,
};
use std::path::Path;
use std::rc::Rc;
use wasi::filesystem::preopens;
use wasi::filesystem::types;

const PATH_KEY: &str = "path";

fn prefix_path(path: String) -> String {
    let path_prefix = wasi::config::store::get(PATH_KEY)
        .expect("Config must resolve")
        .expect(format!("Config must contain '{}'", PATH_KEY).as_str());

    let path = match path.strip_prefix("/") {
        Some(p) => String::from(p),
        None => path,
    };

    // TODO unsafe, path traversals are possible
    String::from(Path::new("").join(path_prefix).join(path).to_str().unwrap())
}

#[derive(Debug, Clone)]
struct FilesystemChroot {}

impl Preopens for FilesystemChroot {
    fn get_directories() -> Vec<(Descriptor, String)> {
        let dirs = preopens::get_directories();
        // TODO find the correct preopen directory, for now assume the first is correct
        let (fd, path) = dirs.first().expect("Must have a preopened directory");

        let path_flags = types::PathFlags::SYMLINK_FOLLOW;
        let open_flags = types::OpenFlags::DIRECTORY;
        let flags = types::DescriptorFlags::READ;

        let chroot = &prefix_path(String::from(path));
        // TODO should we create the directory if it doesn't exist
        let chroot_fd = fd
            .open_at(path_flags, chroot, open_flags, flags)
            .expect(format!("chroot directory '{}' must exist", chroot).as_str());

        vec![(descriptor_map(chroot_fd), String::from("/"))]
    }
}

impl Types for FilesystemChroot {
    type Descriptor = FilesystemChrootDescriptor;
    type DirectoryEntryStream = FilesystemChrootDirectoryEntryStream;

    fn filesystem_error_code(err: &Error) -> Option<ErrorCode> {
        types::filesystem_error_code(err).map(error_code_map)
    }
}

#[derive(Debug, Clone)]
struct FilesystemChrootDescriptor {
    fd: Rc<types::Descriptor>,
}

impl FilesystemChrootDescriptor {
    fn new(fd: types::Descriptor) -> Self {
        Self { fd: Rc::new(fd) }
    }
}

impl GuestDescriptor for FilesystemChrootDescriptor {
    fn read_via_stream(&self, offset: Filesize) -> Result<InputStream, ErrorCode> {
        self.fd.read_via_stream(offset).map_err(error_code_map)
    }

    fn write_via_stream(&self, offset: Filesize) -> Result<OutputStream, ErrorCode> {
        self.fd.write_via_stream(offset).map_err(error_code_map)
    }

    fn append_via_stream(&self) -> Result<OutputStream, ErrorCode> {
        self.fd.append_via_stream().map_err(error_code_map)
    }

    fn advise(&self, offset: Filesize, length: Filesize, advice: Advice) -> Result<(), ErrorCode> {
        let advice = advice_map_in(advice);

        self.fd
            .advise(offset, length, advice)
            .map_err(error_code_map)
    }

    fn sync_data(&self) -> Result<(), ErrorCode> {
        self.fd.sync_data().map_err(error_code_map)
    }

    fn get_flags(&self) -> Result<DescriptorFlags, ErrorCode> {
        self.fd
            .get_flags()
            .map(descriptor_flags_map)
            .map_err(error_code_map)
    }

    fn get_type(&self) -> Result<DescriptorType, ErrorCode> {
        self.fd
            .get_type()
            .map(descriptor_type_map)
            .map_err(error_code_map)
    }

    fn set_size(&self, size: Filesize) -> Result<(), ErrorCode> {
        self.fd.set_size(size).map_err(error_code_map)
    }

    fn set_times(
        &self,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), ErrorCode> {
        let data_access_timestamp = new_timestamp_map_in(data_access_timestamp);
        let data_modification_timestamp = new_timestamp_map_in(data_modification_timestamp);

        self.fd
            .set_times(data_access_timestamp, data_modification_timestamp)
            .map_err(error_code_map)
    }

    fn read(&self, length: Filesize, offset: Filesize) -> Result<(Vec<u8>, bool), ErrorCode> {
        self.fd.read(length, offset).map_err(error_code_map)
    }

    fn write(&self, buffer: Vec<u8>, offset: Filesize) -> Result<Filesize, ErrorCode> {
        self.fd
            .write(buffer.as_slice(), offset)
            .map_err(error_code_map)
    }

    fn read_directory(&self) -> Result<DirectoryEntryStream, ErrorCode> {
        self.fd
            .read_directory()
            .map(directory_entry_stream_map)
            .map_err(error_code_map)
    }

    fn sync(&self) -> Result<(), ErrorCode> {
        self.fd.sync().map_err(error_code_map)
    }

    fn create_directory_at(&self, path: String) -> Result<(), ErrorCode> {
        self.fd.create_directory_at(&path).map_err(error_code_map)
    }

    fn stat(&self) -> Result<DescriptorStat, ErrorCode> {
        self.fd
            .stat()
            .map(descriptor_stat_map)
            .map_err(error_code_map)
    }

    fn stat_at(&self, path_flags: PathFlags, path: String) -> Result<DescriptorStat, ErrorCode> {
        let path_flags = types::PathFlags::from_bits(path_flags.bits()).unwrap();

        self.fd
            .stat_at(path_flags, &path)
            .map(descriptor_stat_map)
            .map_err(error_code_map)
    }

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

        self.fd
            .set_times_at(
                path_flags,
                &path,
                data_access_timestamp,
                data_modification_timestamp,
            )
            .map_err(error_code_map)
    }

    fn link_at(
        &self,
        old_path_flags: PathFlags,
        old_path: String,
        new_descriptor: DescriptorBorrow<'_>,
        new_path: String,
    ) -> Result<(), ErrorCode> {
        let old_path_flags = types::PathFlags::from_bits(old_path_flags.bits()).unwrap();
        let new_descriptor: &Self = new_descriptor.get();

        self.fd
            .link_at(old_path_flags, &old_path, &new_descriptor.fd, &new_path)
            .map_err(error_code_map)
    }

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

        self.fd
            .open_at(path_flags, &path, open_flags, flags)
            .map(descriptor_map)
            .map_err(error_code_map)
    }

    fn readlink_at(&self, path: String) -> Result<String, ErrorCode> {
        self.fd.readlink_at(&path).map_err(error_code_map)
    }

    fn remove_directory_at(&self, path: String) -> Result<(), ErrorCode> {
        self.fd.remove_directory_at(&path).map_err(error_code_map)
    }

    fn rename_at(
        &self,
        old_path: String,
        new_descriptor: DescriptorBorrow<'_>,
        new_path: String,
    ) -> Result<(), ErrorCode> {
        let new_descriptor: &Self = new_descriptor.get();

        self.fd
            .rename_at(&old_path, &new_descriptor.fd, &new_path)
            .map_err(error_code_map)
    }

    fn symlink_at(&self, old_path: String, new_path: String) -> Result<(), ErrorCode> {
        self.fd
            .symlink_at(&old_path, &new_path)
            .map_err(error_code_map)
    }

    fn unlink_file_at(&self, path: String) -> Result<(), ErrorCode> {
        self.fd.unlink_file_at(&path).map_err(error_code_map)
    }

    fn is_same_object(&self, other: DescriptorBorrow<'_>) -> bool {
        let other: &Self = other.get();

        self.fd.is_same_object(&other.fd)
    }

    fn metadata_hash(&self) -> Result<MetadataHashValue, ErrorCode> {
        self.fd
            .metadata_hash()
            .map(metadata_hash_value_map)
            .map_err(error_code_map)
    }

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
struct FilesystemChrootDirectoryEntryStream {
    des: Rc<types::DirectoryEntryStream>,
}

impl FilesystemChrootDirectoryEntryStream {
    fn new(des: types::DirectoryEntryStream) -> Self {
        Self { des: Rc::new(des) }
    }
}

impl GuestDirectoryEntryStream for FilesystemChrootDirectoryEntryStream {
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
    Descriptor::new(FilesystemChrootDescriptor::new(descriptor))
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
    DirectoryEntryStream::new(FilesystemChrootDirectoryEntryStream::new(
        directory_entry_stream,
    ))
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

fn new_timestamp_map_in(new_timestamp: NewTimestamp) -> types::NewTimestamp {
    match new_timestamp {
        NewTimestamp::NoChange => types::NewTimestamp::NoChange,
        NewTimestamp::Now => types::NewTimestamp::Now,
        NewTimestamp::Timestamp(datetime) => types::NewTimestamp::Timestamp(datetime),
    }
}

wit_bindgen::generate!({
    path: "../../wit",
    world: "filesystem",
    generate_all
});

export!(FilesystemChroot);
