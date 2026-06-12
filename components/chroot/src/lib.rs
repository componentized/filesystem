#![no_main]

use std::path::{Component, Path, PathBuf};

use exports::wasi::filesystem::preopens::Guest as Preopens;
use exports::wasi::filesystem::types::{
    Advice, Descriptor, DescriptorBorrow, DescriptorFlags, DescriptorStat, DescriptorType,
    DirectoryEntry, ErrorCode, Filesize, Guest as Types, GuestDescriptor, MetadataHashValue,
    NewTimestamp, OpenFlags, PathFlags,
};
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

struct FilesystemChroot {}

impl Preopens for FilesystemChroot {
    fn get_directories() -> Vec<(Descriptor, String)> {
        let dirs = preopens::get_directories();
        // TODO find the correct preopen directory, for now assume the first is correct
        let (fd, path) = dirs.first().expect("Must have a preopened directory");

        let path_flags = types::PathFlags::SYMLINK_FOLLOW;
        let open_flags = types::OpenFlags::DIRECTORY;
        let flags = types::DescriptorFlags::READ;

        let root = prefix_path(String::from(path));

        let fd = wit_bindgen::block_on(async {
            fd.open_at(path_flags, root.clone(), open_flags, flags)
                .await
                // TODO should we create the directory if it doesn't exist
                .expect(format!("chroot directory '{}' must exist", root.clone()).as_str())
        });

        let chroot_fd =
            Descriptor::new(FilesystemChrootDescriptor::new(fd, root.into(), "/".into()));
        vec![(chroot_fd, String::from("/"))]
    }
}

impl Types for FilesystemChroot {
    type Descriptor = FilesystemChrootDescriptor;
}

struct FilesystemChrootDescriptor {
    fd: types::Descriptor,
    root: PathBuf,
    path: PathBuf,
}

impl FilesystemChrootDescriptor {
    fn new(fd: types::Descriptor, root: PathBuf, path: PathBuf) -> Self {
        Self { fd, root, path }
    }

    fn internal_path(&self, path: String) -> Result<PathBuf, ErrorCode> {
        let mut internal = self.path.clone();
        for component in PathBuf::from(path).components() {
            if component == Component::RootDir {
                // treat root dir as a relative path
                internal = PathBuf::from(".")
            } else if component == Component::ParentDir {
                if !internal.pop() {
                    // attempt to escape root
                    return Err(ErrorCode::NotPermitted);
                };
            } else if let Component::Normal(c) = component {
                internal.push(c);
            }
        }
        Ok(internal)
    }

    fn external_path(&self, path: String) -> Result<String, ErrorCode> {
        let path = self.root.clone().join(self.internal_path(path)?);
        Ok(path.to_string_lossy().into_owned())
    }
}

impl GuestDescriptor for FilesystemChrootDescriptor {
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
        data: wit_bindgen::StreamReader<u8>,
        offset: Filesize,
    ) -> wit_bindgen::FutureReader<Result<(), ErrorCode>> {
        self.fd.write_via_stream(data, offset)
    }

    fn append_via_stream(
        &self,
        data: wit_bindgen::StreamReader<u8>,
    ) -> wit_bindgen::FutureReader<Result<(), ErrorCode>> {
        self.fd.append_via_stream(data)
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
        self.fd.sync_data().await
    }

    async fn get_flags(&self) -> Result<DescriptorFlags, ErrorCode> {
        self.fd.get_flags().await
    }

    async fn get_type(&self) -> Result<DescriptorType, ErrorCode> {
        self.fd.get_type().await
    }

    async fn set_size(&self, size: Filesize) -> Result<(), ErrorCode> {
        self.fd.set_size(size).await
    }

    async fn set_times(
        &self,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), ErrorCode> {
        self.fd
            .set_times(data_access_timestamp, data_modification_timestamp)
            .await
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
        self.fd.sync().await
    }

    async fn create_directory_at(&self, path: String) -> Result<(), ErrorCode> {
        let path = self.external_path(path)?;
        self.fd.create_directory_at(path).await
    }

    async fn stat(&self) -> Result<DescriptorStat, ErrorCode> {
        self.fd.stat().await
    }

    async fn stat_at(
        &self,
        path_flags: PathFlags,
        path: String,
    ) -> Result<DescriptorStat, ErrorCode> {
        let path = self.external_path(path)?;
        self.fd.stat_at(path_flags, path).await
    }

    async fn set_times_at(
        &self,
        path_flags: PathFlags,
        path: String,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), ErrorCode> {
        let path = self.external_path(path)?;
        self.fd
            .set_times_at(
                path_flags,
                path,
                data_access_timestamp,
                data_modification_timestamp,
            )
            .await
    }

    async fn link_at(
        &self,
        old_path_flags: PathFlags,
        old_path: String,
        new_descriptor: DescriptorBorrow<'_>,
        new_path: String,
    ) -> Result<(), ErrorCode> {
        let old_path = self.external_path(old_path)?;
        let new_descriptor: &FilesystemChrootDescriptor = new_descriptor.get();
        let new_path = self.external_path(new_path)?;
        self.fd
            .link_at(old_path_flags, old_path, &new_descriptor.fd, new_path)
            .await
    }

    async fn open_at(
        &self,
        path_flags: PathFlags,
        path: String,
        open_flags: OpenFlags,
        flags: DescriptorFlags,
    ) -> Result<Descriptor, ErrorCode> {
        let internal_path = self.internal_path(path.clone())?;
        let external_path = self.external_path(path.clone())?;
        self.fd
            .open_at(path_flags, external_path, open_flags, flags)
            .await
            .map(|fd| {
                Descriptor::new(FilesystemChrootDescriptor::new(
                    fd,
                    self.root.clone(),
                    internal_path,
                ))
            })
    }

    async fn readlink_at(&self, path: String) -> Result<String, ErrorCode> {
        let path = self.external_path(path)?;
        self.fd.readlink_at(path).await
    }

    async fn remove_directory_at(&self, path: String) -> Result<(), ErrorCode> {
        let path = self.external_path(path)?;
        self.fd.remove_directory_at(path).await
    }

    async fn rename_at(
        &self,
        old_path: String,
        new_descriptor: DescriptorBorrow<'_>,
        new_path: String,
    ) -> Result<(), ErrorCode> {
        let old_path = self.external_path(old_path)?;
        let new_descriptor: &Self = new_descriptor.get();
        let new_path = self.external_path(new_path)?;

        self.fd
            .rename_at(old_path, &new_descriptor.fd, new_path)
            .await
    }

    async fn symlink_at(&self, old_path: String, new_path: String) -> Result<(), ErrorCode> {
        let old_path = self.external_path(old_path)?;
        let new_path = self.external_path(new_path)?;
        self.fd.symlink_at(old_path, new_path).await
    }

    async fn unlink_file_at(&self, path: String) -> Result<(), ErrorCode> {
        let path = self.external_path(path)?;
        self.fd.unlink_file_at(path).await
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
        let path = self.external_path(path)?;
        self.fd.metadata_hash_at(path_flags, path).await
    }
}

wit_bindgen::generate!({
    path: "../../wit",
    world: "filesystem",
    merge_structurally_equal_types: true,
    generate_all
});

export!(FilesystemChroot);
