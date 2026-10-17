use std::cell::OnceCell;
use std::rc::Rc;

use exports::wasi::filesystem::preopens::Guest as Preopens;
use exports::wasi::filesystem::types::{
    Advice, Descriptor, DescriptorBorrow, DescriptorFlags, DescriptorStat, DescriptorType,
    DirectoryEntry, ErrorCode, Filesize, Guest as Types, GuestDescriptor, MetadataHashValue,
    NewTimestamp, OpenFlags, PathFlags,
};
use wasi::filesystem::preopens;
use wasi::filesystem::types;
use wit_bindgen::{FutureReader, StreamReader, StreamResult, StreamWriter};

mod path;

use path::Base;

const PATH_KEY: &str = "path";

struct FilesystemChroot {}

impl Preopens for FilesystemChroot {
    fn get_directories() -> Vec<(Descriptor, String)> {
        let config = wasi::config::store::get(PATH_KEY)
            .expect("Config must resolve")
            .unwrap_or_else(|| panic!("Config must contain '{PATH_KEY}'"));
        let components = path::config_components(&config)
            .unwrap_or_else(|| panic!("chroot path '{config}' must not contain '..'"));

        let mut dirs = preopens::get_directories();
        let names: Vec<&str> = dirs.iter().map(|(_, name)| name.as_str()).collect();
        let (index, path) = path::find_preopen(&components, &names).unwrap_or_else(|| {
            panic!("chroot path '{config}' must be within a preopened directory")
        });
        let (preopen, _) = dirs.swap_remove(index);

        let root = FilesystemChrootDescriptor {
            fd: None,
            jail: Rc::new(Jail {
                preopen,
                path,
                root: OnceCell::new(),
            }),
            at_root: true,
        };
        // a virtual scope rather than the guest's root, wasi preopens need not be rooted
        vec![(Descriptor::new(root), String::from("."))]
    }
}

impl Types for FilesystemChroot {
    type Descriptor = FilesystemChrootDescriptor;
}

/// The directory the chroot is confined to.
///
/// It is opened by the first operation that needs it, `get-directories` is synchronous and can't
/// wait for the host to open it.
struct Jail {
    /// The preopen containing the jail.
    preopen: types::Descriptor,
    /// The path of the jail within the preopen, without `..`.
    path: String,
    root: OnceCell<types::Descriptor>,
}

impl Jail {
    /// The root of the chroot, opened when first needed.
    async fn root(&self) -> Result<&types::Descriptor, ErrorCode> {
        if let Some(root) = self.root.get() {
            return Ok(root);
        }
        // the chroot is no more capable than the preopen it is in
        let flags = self.preopen.get_flags().await?
            & (types::DescriptorFlags::READ | types::DescriptorFlags::MUTATE_DIRECTORY);
        let root = self
            .preopen
            .open_at(
                types::PathFlags::SYMLINK_FOLLOW,
                self.path.clone(),
                types::OpenFlags::DIRECTORY,
                flags,
            )
            .await?;
        // another operation may have opened the root while this one waited, either will do
        Ok(self.root.get_or_init(|| root))
    }
}

struct FilesystemChrootDescriptor {
    /// The host descriptor, `None` for the root of the chroot, which is opened by the jail.
    fd: Option<types::Descriptor>,
    jail: Rc<Jail>,
    /// Whether the descriptor is known to be the root, `..` from the root is the root.
    at_root: bool,
}

impl FilesystemChrootDescriptor {
    async fn fd(&self) -> Result<&types::Descriptor, ErrorCode> {
        match &self.fd {
            Some(fd) => Ok(fd),
            None => self.jail.root().await,
        }
    }

    /// Resolve a path into the host descriptor to use and a path relative to it, which is never
    /// absolute and never contains `..`.
    async fn resolve(&self, path: String) -> Result<(&types::Descriptor, Base, String), ErrorCode> {
        let (base, path) = path::resolve(&path, self.at_root).map_err(|err| match err {
            path::Error::Empty => ErrorCode::NoEntry,
            path::Error::Escape => ErrorCode::NotPermitted,
            path::Error::Nul => ErrorCode::IllegalByteSequence,
        })?;
        let fd = match base {
            Base::Root => self.jail.root().await?,
            Base::Descriptor => self.fd().await?,
        };
        Ok((fd, base, path))
    }

    /// Run a synchronous operation that returns a future for its result.
    ///
    /// A synchronous function can't wait for the root to be opened, once it is open the operation
    /// is passed through, until then the operation runs in the background and its result is
    /// forwarded.
    fn with_fd(
        &self,
        op: impl FnOnce(&types::Descriptor) -> FutureReader<Result<(), ErrorCode>> + 'static,
    ) -> FutureReader<Result<(), ErrorCode>> {
        if let Some(fd) = self.opened_fd() {
            return op(fd);
        }
        let jail = self.jail.clone();
        let (tx, rx) = result_future();
        componentized_rt::executor::spawn(async move {
            let result = match jail.root().await {
                Ok(root) => op(root).await,
                Err(err) => Err(err),
            };
            let _ = tx.write(result).await;
        });
        rx
    }

    /// Run a synchronous operation that returns a stream and a future for its result, see
    /// [`Self::with_fd`].
    fn with_fd_stream<T: 'static>(
        &self,
        new_stream: fn() -> (StreamWriter<T>, StreamReader<T>),
        op: impl FnOnce(&types::Descriptor) -> (StreamReader<T>, FutureReader<Result<(), ErrorCode>>)
            + 'static,
    ) -> (StreamReader<T>, FutureReader<Result<(), ErrorCode>>) {
        if let Some(fd) = self.opened_fd() {
            return op(fd);
        }
        let jail = self.jail.clone();
        let (mut stream_tx, stream_rx) = new_stream();
        let (tx, rx) = result_future();
        componentized_rt::executor::spawn(async move {
            let result = match jail.root().await {
                Ok(root) => {
                    let (stream, result) = op(root);
                    forward(stream, &mut stream_tx).await;
                    result.await
                }
                Err(err) => Err(err),
            };
            // the stream ends before the result is known
            drop(stream_tx);
            let _ = tx.write(result).await;
        });
        (stream_rx, rx)
    }

    /// The host descriptor, when it is open.
    fn opened_fd(&self) -> Option<&types::Descriptor> {
        self.fd.as_ref().or_else(|| self.jail.root.get())
    }
}

fn result_future() -> (
    wit_bindgen::FutureWriter<Result<(), ErrorCode>>,
    FutureReader<Result<(), ErrorCode>>,
) {
    // the default is only written if the write is cancelled
    wit_future::new(|| Err(ErrorCode::Other(None)))
}

/// Forward the items of one stream to another, until either end is dropped.
async fn forward<T: 'static>(from: StreamReader<T>, to: &mut StreamWriter<T>) {
    let mut from = from;
    loop {
        let (result, items) = from.read(Vec::with_capacity(4096)).await;
        if !items.is_empty() && !to.write_all(items).await.is_empty() {
            // the reader was dropped
            return;
        }
        if !matches!(result, StreamResult::Complete(_)) {
            return;
        }
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
        self.with_fd_stream(wit_stream::new, move |fd| fd.read_via_stream(offset))
    }

    fn write_via_stream(
        &self,
        data: wit_bindgen::StreamReader<u8>,
        offset: Filesize,
    ) -> wit_bindgen::FutureReader<Result<(), ErrorCode>> {
        self.with_fd(move |fd| fd.write_via_stream(data, offset))
    }

    fn append_via_stream(
        &self,
        data: wit_bindgen::StreamReader<u8>,
    ) -> wit_bindgen::FutureReader<Result<(), ErrorCode>> {
        self.with_fd(move |fd| fd.append_via_stream(data))
    }

    async fn advise(
        &self,
        offset: Filesize,
        length: Filesize,
        advice: Advice,
    ) -> Result<(), ErrorCode> {
        self.fd().await?.advise(offset, length, advice).await
    }

    async fn sync_data(&self) -> Result<(), ErrorCode> {
        self.fd().await?.sync_data().await
    }

    async fn get_flags(&self) -> Result<DescriptorFlags, ErrorCode> {
        self.fd().await?.get_flags().await
    }

    async fn get_type(&self) -> Result<DescriptorType, ErrorCode> {
        self.fd().await?.get_type().await
    }

    async fn set_size(&self, size: Filesize) -> Result<(), ErrorCode> {
        self.fd().await?.set_size(size).await
    }

    async fn set_times(
        &self,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), ErrorCode> {
        self.fd()
            .await?
            .set_times(data_access_timestamp, data_modification_timestamp)
            .await
    }

    fn read_directory(
        &self,
    ) -> (
        wit_bindgen::StreamReader<DirectoryEntry>,
        wit_bindgen::FutureReader<Result<(), ErrorCode>>,
    ) {
        self.with_fd_stream(wit_stream::new, |fd| fd.read_directory())
    }

    async fn sync(&self) -> Result<(), ErrorCode> {
        self.fd().await?.sync().await
    }

    async fn create_directory_at(&self, path: String) -> Result<(), ErrorCode> {
        let (fd, _, path) = self.resolve(path).await?;
        fd.create_directory_at(path).await
    }

    async fn stat(&self) -> Result<DescriptorStat, ErrorCode> {
        self.fd().await?.stat().await
    }

    async fn stat_at(
        &self,
        path_flags: PathFlags,
        path: String,
    ) -> Result<DescriptorStat, ErrorCode> {
        let (fd, _, path) = self.resolve(path).await?;
        fd.stat_at(path_flags, path).await
    }

    async fn set_times_at(
        &self,
        path_flags: PathFlags,
        path: String,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), ErrorCode> {
        let (fd, _, path) = self.resolve(path).await?;
        fd.set_times_at(
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
        let (old_fd, _, old_path) = self.resolve(old_path).await?;
        let new_descriptor: &Self = new_descriptor.get();
        let (new_fd, _, new_path) = new_descriptor.resolve(new_path).await?;
        old_fd
            .link_at(old_path_flags, old_path, new_fd, new_path)
            .await
    }

    async fn open_at(
        &self,
        path_flags: PathFlags,
        path: String,
        open_flags: OpenFlags,
        flags: DescriptorFlags,
    ) -> Result<Descriptor, ErrorCode> {
        let (fd, base, path) = self.resolve(path).await?;
        // the root itself, or the descriptor itself when it is the root
        let at_root = path == "." && (base == Base::Root || self.at_root);
        let fd = fd.open_at(path_flags, path, open_flags, flags).await?;
        Ok(Descriptor::new(FilesystemChrootDescriptor {
            fd: Some(fd),
            jail: self.jail.clone(),
            at_root,
        }))
    }

    async fn readlink_at(&self, path: String) -> Result<String, ErrorCode> {
        let (fd, _, path) = self.resolve(path).await?;
        fd.readlink_at(path).await
    }

    async fn remove_directory_at(&self, path: String) -> Result<(), ErrorCode> {
        let (fd, _, path) = self.resolve(path).await?;
        fd.remove_directory_at(path).await
    }

    async fn rename_at(
        &self,
        old_path: String,
        new_descriptor: DescriptorBorrow<'_>,
        new_path: String,
    ) -> Result<(), ErrorCode> {
        let (old_fd, _, old_path) = self.resolve(old_path).await?;
        let new_descriptor: &Self = new_descriptor.get();
        let (new_fd, _, new_path) = new_descriptor.resolve(new_path).await?;
        old_fd.rename_at(old_path, new_fd, new_path).await
    }

    async fn symlink_at(&self, old_path: String, new_path: String) -> Result<(), ErrorCode> {
        // the target is stored as given, it is resolved when the link is followed and the host
        // confines it to the descriptor the link is followed from
        let (fd, _, new_path) = self.resolve(new_path).await?;
        fd.symlink_at(old_path, new_path).await
    }

    async fn unlink_file_at(&self, path: String) -> Result<(), ErrorCode> {
        let (fd, _, path) = self.resolve(path).await?;
        fd.unlink_file_at(path).await
    }

    async fn is_same_object(&self, other: DescriptorBorrow<'_>) -> bool {
        let other: &Self = other.get();
        match (self.fd().await, other.fd().await) {
            (Ok(fd), Ok(other)) => fd.is_same_object(other).await,
            _ => false,
        }
    }

    async fn metadata_hash(&self) -> Result<MetadataHashValue, ErrorCode> {
        self.fd().await?.metadata_hash().await
    }

    async fn metadata_hash_at(
        &self,
        path_flags: PathFlags,
        path: String,
    ) -> Result<MetadataHashValue, ErrorCode> {
        let (fd, _, path) = self.resolve(path).await?;
        fd.metadata_hash_at(path_flags, path).await
    }
}

wit_bindgen::generate!({
    path: "../wit",
    world: "filesystem",
    merge_structurally_equal_types: true,
    generate_all
});

export!(FilesystemChroot);
