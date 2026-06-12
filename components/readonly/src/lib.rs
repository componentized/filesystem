#![no_main]

use std::path::PathBuf;

use exports::wasi::filesystem::preopens::Guest as Preopens;
use exports::wasi::filesystem::types::{
    Advice, Descriptor, DescriptorBorrow, DescriptorFlags, DescriptorStat, DescriptorType,
    DirectoryEntry, ErrorCode, Filesize, Guest as Types, MetadataHashValue, NewTimestamp,
    OpenFlags, PathFlags,
};
use wasi::filesystem::preopens;
use wasi::filesystem::types;

struct FilesystemReadOnly {}

impl Preopens for FilesystemReadOnly {
    #[doc = " Return the set of preopened directories, and their path."]
    fn get_directories() -> Vec<(Descriptor, String)> {
        preopens::get_directories()
            .into_iter()
            .map(|(fd, path)| {
                let fd = Descriptor::new(ReadOnlyDescriptor::new(fd, path.clone().into()));
                (fd, path)
            })
            .collect()
    }
}

impl Types for FilesystemReadOnly {
    type Descriptor = ReadOnlyDescriptor;
}

struct ReadOnlyDescriptor {
    fd: types::Descriptor,
    path: PathBuf,
}

impl ReadOnlyDescriptor {
    fn new(fd: types::Descriptor, path: PathBuf) -> Self {
        Self { fd, path }
    }
}

impl exports::wasi::filesystem::types::GuestDescriptor for ReadOnlyDescriptor {
    fn read_via_stream(
        &self,
        offset: Filesize,
    ) -> (
        wit_bindgen::StreamReader<u8>,
        wit_bindgen::FutureReader<Result<(), ErrorCode>>,
    ) {
        self.fd.read_via_stream(offset)
    }

    fn write_via_stream(
        &self,
        _data: wit_bindgen::StreamReader<u8>,
        _offset: Filesize,
    ) -> wit_bindgen::FutureReader<Result<(), ErrorCode>> {
        let (tx, rx) = wit_future::new(|| Err(ErrorCode::ReadOnly));
        tx.write(Err(ErrorCode::ReadOnly));
        rx
    }

    fn append_via_stream(
        &self,
        _data: wit_bindgen::StreamReader<u8>,
    ) -> wit_bindgen::FutureReader<Result<(), ErrorCode>> {
        let (tx, rx) = wit_future::new(|| Err(ErrorCode::ReadOnly));
        tx.write(Err(ErrorCode::ReadOnly));
        rx
    }

    async fn advise(
        &self,
        offset: Filesize,
        length: Filesize,
        advice: Advice,
    ) -> Result<(), ErrorCode> {
        self.fd.advise(offset, length, advice).await
    }

    async fn sync_data(&self) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    async fn get_flags(&self) -> Result<DescriptorFlags, ErrorCode> {
        self.fd.get_flags().await.map(|flags| {
            flags
                .difference(DescriptorFlags::WRITE)
                .difference(DescriptorFlags::FILE_INTEGRITY_SYNC)
                .difference(DescriptorFlags::DATA_INTEGRITY_SYNC)
                .difference(DescriptorFlags::MUTATE_DIRECTORY)
        })
    }

    async fn get_type(&self) -> Result<DescriptorType, ErrorCode> {
        self.fd.get_type().await
    }

    async fn set_size(&self, _size: Filesize) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    async fn set_times(
        &self,
        _data_access_timestamp: NewTimestamp,
        _data_modification_timestamp: NewTimestamp,
    ) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    fn read_directory(
        &self,
    ) -> (
        wit_bindgen::StreamReader<DirectoryEntry>,
        wit_bindgen::FutureReader<Result<(), ErrorCode>>,
    ) {
        self.fd.read_directory()
    }

    async fn sync(&self) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    async fn create_directory_at(&self, _path: String) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    async fn stat(&self) -> Result<DescriptorStat, ErrorCode> {
        self.fd.stat().await
    }

    async fn stat_at(
        &self,
        path_flags: PathFlags,
        path: String,
    ) -> Result<DescriptorStat, ErrorCode> {
        self.fd.stat_at(path_flags, path).await
    }

    async fn set_times_at(
        &self,
        _path_flags: PathFlags,
        _path: String,
        _data_access_timestamp: NewTimestamp,
        _data_modification_timestamp: NewTimestamp,
    ) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    async fn link_at(
        &self,
        _old_path_flags: PathFlags,
        _old_path: String,
        _new_descriptor: DescriptorBorrow<'_>,
        _new_path: String,
    ) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    async fn open_at(
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

        match self
            .fd
            .open_at(path_flags, path.clone(), open_flags, flags)
            .await
        {
            Ok(fd) => Ok(Descriptor::new(ReadOnlyDescriptor::new(
                fd,
                self.path.clone().join(path),
            ))),
            Err(error_code) => Err(error_code),
        }
    }

    async fn readlink_at(&self, path: String) -> Result<String, ErrorCode> {
        self.fd.readlink_at(path).await
    }

    async fn remove_directory_at(&self, _path: String) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    async fn rename_at(
        &self,
        _old_path: String,
        _new_descriptor: DescriptorBorrow<'_>,
        _new_path: String,
    ) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    async fn symlink_at(&self, _old_path: String, _new_path: String) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    async fn unlink_file_at(&self, _path: String) -> Result<(), ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    async fn is_same_object(&self, other: DescriptorBorrow<'_>) -> bool {
        let other: &Self = other.get();
        self.fd.is_same_object(&other.fd).await
    }

    async fn metadata_hash(&self) -> Result<MetadataHashValue, ErrorCode> {
        self.fd.metadata_hash().await
    }

    async fn metadata_hash_at(
        &self,
        path_flags: PathFlags,
        path: String,
    ) -> Result<MetadataHashValue, ErrorCode> {
        self.fd.metadata_hash_at(path_flags, path).await
    }
}

wit_bindgen::generate!({
    path: "../../wit",
    world: "filesystem",
    merge_structurally_equal_types: true,
    generate_all
});

export!(FilesystemReadOnly);
