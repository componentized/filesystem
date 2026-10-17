//! The readonly component allows every operation that reads the filesystem and denies every
//! operation that would change it with `read-only`.
//!
//! Each test preopens a fresh copy of this tree read-write, so any denial comes from the component
//! rather than the host:
//!
//! ```text
//! file.txt          "hello world"
//! link -> file.txt
//! empty/
//! dir/nested.txt    "nested"
//! ```
//!
//! Tests of mutations compare a snapshot of the tree before and after, nothing may change.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tempfile::TempDir;
use test_harness::bindings::exports::wasi::filesystem::types::{
    Advice, GuestDescriptor, NewTimestamp,
};
use test_harness::{
    Ctx, DescriptorFlags, DescriptorType, ErrorCode, Exports, Harness, OpenFlags, PathFlags, Perms,
    TestSubject,
};
use wasmtime::component::{Accessor, ResourceAny};

const FILE: &str = "hello world";
const NESTED: &str = "nested";

struct Fixture {
    dir: TempDir,
}

/// What a test can observe about one entry of the tree.
#[derive(Debug, PartialEq, Eq)]
enum Entry {
    // not the access time, reads may change it, as does taking the snapshot
    File {
        content: Vec<u8>,
        modified: SystemTime,
    },
    Dir {
        modified: SystemTime,
    },
    Link {
        target: PathBuf,
    },
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("file.txt"), FILE).unwrap();
        symlink("file.txt", root.join("link")).unwrap();
        fs::create_dir(root.join("empty")).unwrap();
        fs::create_dir(root.join("dir")).unwrap();
        fs::write(root.join("dir/nested.txt"), NESTED).unwrap();
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Every entry of the tree, by its path relative to the root.
    fn snapshot(&self) -> BTreeMap<PathBuf, Entry> {
        fn walk(root: &Path, dir: &Path, entries: &mut BTreeMap<PathBuf, Entry>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let meta = fs::symlink_metadata(&path).unwrap();
                let relative = path.strip_prefix(root).unwrap().to_path_buf();
                if meta.is_symlink() {
                    let target = fs::read_link(&path).unwrap();
                    entries.insert(relative, Entry::Link { target });
                } else if meta.is_dir() {
                    let modified = meta.modified().unwrap();
                    entries.insert(relative, Entry::Dir { modified });
                    walk(root, &path, entries);
                } else {
                    let content = fs::read(&path).unwrap();
                    let modified = meta.modified().unwrap();
                    entries.insert(relative, Entry::File { content, modified });
                }
            }
        }
        let mut entries = BTreeMap::new();
        walk(self.path(), self.path(), &mut entries);
        entries
    }

    async fn readonly(&self) -> TestSubject {
        Harness::new("readonly")
            .preopen(self.path(), "/", Perms::ReadWrite)
            .build()
            .await
            .expect("readonly")
    }
}

/// Descriptors opened through the readonly component.
#[derive(Clone, Copy)]
struct Handles {
    /// The preopen
    root: ResourceAny,
    /// `dir`, opened from the root
    dir: ResourceAny,
    /// `file.txt`, opened from the root
    file: ResourceAny,
    /// `dir/nested.txt`, opened from `dir`
    nested: ResourceAny,
}

fn descriptor(exports: &Exports) -> GuestDescriptor<'_> {
    exports.wasi_filesystem_types().descriptor()
}

async fn open(
    accessor: &Accessor<Ctx>,
    exports: &Exports,
    fd: ResourceAny,
    path: &str,
    open_flags: OpenFlags,
    flags: DescriptorFlags,
) -> wasmtime::Result<Result<ResourceAny, ErrorCode>> {
    descriptor(exports)
        .call_open_at(
            accessor,
            fd,
            PathFlags::empty(),
            path.into(),
            open_flags,
            flags,
        )
        .await
}

async fn handles(accessor: &Accessor<Ctx>, exports: &Exports) -> wasmtime::Result<Handles> {
    let dirs = exports
        .wasi_filesystem_preopens()
        .call_get_directories(accessor)
        .await?;
    let (root, _) = dirs.into_iter().next().expect("a preopen");
    let read = DescriptorFlags::READ;
    let dir = open(accessor, exports, root, "dir", OpenFlags::DIRECTORY, read)
        .await?
        .expect("open dir");
    let file = open(
        accessor,
        exports,
        root,
        "file.txt",
        OpenFlags::empty(),
        read,
    )
    .await?
    .expect("open file.txt");
    let nested = open(
        accessor,
        exports,
        dir,
        "nested.txt",
        OpenFlags::empty(),
        read,
    )
    .await?
    .expect("open dir/nested.txt");
    Ok(Handles {
        root,
        dir,
        file,
        nested,
    })
}

/// Run the operations, each must be denied with `read-only` and leave the tree unchanged.
async fn assert_denied(
    f: impl AsyncFnOnce(
            &Accessor<Ctx>,
            &Exports,
            Handles,
        ) -> wasmtime::Result<Vec<Result<(), ErrorCode>>>
        + Send,
) {
    let fixture = Fixture::new();
    let before = fixture.snapshot();
    let mut readonly = fixture.readonly().await;
    let results = readonly
        .run(async |accessor, exports| {
            let handles = handles(accessor, exports).await?;
            f(accessor, exports, handles).await
        })
        .await
        .expect("run");
    assert!(!results.is_empty());
    for (index, result) in results.into_iter().enumerate() {
        assert_eq!(result, Err(ErrorCode::ReadOnly), "operation {index}");
    }
    assert_eq!(fixture.snapshot(), before);
}

/// Run reads against the readonly component, the tree must be unchanged after.
async fn read<R: Send + 'static>(
    f: impl AsyncFnOnce(&Accessor<Ctx>, &Exports, Handles) -> wasmtime::Result<R> + Send,
) -> R {
    let fixture = Fixture::new();
    let before = fixture.snapshot();
    let mut readonly = fixture.readonly().await;
    let result = readonly
        .run(async |accessor, exports| {
            let handles = handles(accessor, exports).await?;
            f(accessor, exports, handles).await
        })
        .await
        .expect("run");
    assert_eq!(fixture.snapshot(), before);
    result
}

fn ok<T, E>(result: Result<T, E>) -> Result<(), E> {
    result.map(|_| ())
}

// --- reads are allowed ---

#[tokio::test(flavor = "multi_thread")]
async fn preopens_are_passed_through() {
    let fixture = Fixture::new();
    let mut readonly = Harness::new("readonly")
        .preopen(fixture.path(), "/", Perms::ReadWrite)
        .preopen(fixture.path().join("dir"), "/data", Perms::ReadWrite)
        .build()
        .await
        .unwrap();
    let names = readonly
        .run(async |accessor, exports| {
            let dirs = exports
                .wasi_filesystem_preopens()
                .call_get_directories(accessor)
                .await?;
            Ok(dirs.into_iter().map(|(_, name)| name).collect::<Vec<_>>())
        })
        .await
        .unwrap();
    assert_eq!(names, ["/", "/data"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn files_and_directories_are_opened_for_reading() {
    let results = read(async |accessor, exports, h| {
        let read = DescriptorFlags::READ;
        let mut results = vec![];
        for (fd, path, open_flags) in [
            (h.root, "file.txt", OpenFlags::empty()),
            (h.root, ".", OpenFlags::DIRECTORY),
            (h.root, "empty", OpenFlags::DIRECTORY),
            (h.root, "dir/nested.txt", OpenFlags::empty()),
            (h.dir, "nested.txt", OpenFlags::empty()),
            (h.dir, ".", OpenFlags::DIRECTORY),
        ] {
            results.push(ok(
                open(accessor, exports, fd, path, open_flags, read).await?
            ));
        }
        // following a symlink is a read
        results.push(ok(descriptor(exports)
            .call_open_at(
                accessor,
                h.root,
                PathFlags::SYMLINK_FOLLOW,
                "link".into(),
                OpenFlags::empty(),
                read,
            )
            .await?));
        Ok(results)
    })
    .await;
    for (index, result) in results.into_iter().enumerate() {
        assert_eq!(result, Ok(()), "open {index}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn opening_a_missing_file_is_the_host_error() {
    let result = read(async |accessor, exports, h| {
        Ok(ok(open(
            accessor,
            exports,
            h.root,
            "missing.txt",
            OpenFlags::empty(),
            DescriptorFlags::READ,
        )
        .await?))
    })
    .await;
    assert_eq!(result, Err(ErrorCode::NoEntry));
}

#[tokio::test(flavor = "multi_thread")]
async fn files_are_read() {
    let (whole, offset, nested) = read(async |accessor, exports, h| {
        async fn read_all(
            accessor: &Accessor<Ctx>,
            exports: &Exports,
            fd: ResourceAny,
            offset: u64,
        ) -> wasmtime::Result<String> {
            let (data, _) = descriptor(exports)
                .call_read_via_stream(accessor, fd, offset)
                .await?;
            let mut data = test_harness::collect(accessor, data)?;
            let mut bytes = vec![];
            while let Some(byte) = data.recv().await {
                bytes.push(byte);
            }
            Ok(String::from_utf8(bytes)?)
        }
        Ok((
            read_all(accessor, exports, h.file, 0).await?,
            read_all(accessor, exports, h.file, 6).await?,
            read_all(accessor, exports, h.nested, 0).await?,
        ))
    })
    .await;
    assert_eq!(whole, FILE);
    assert_eq!(offset, "world");
    assert_eq!(nested, NESTED);
}

#[tokio::test(flavor = "multi_thread")]
async fn directories_are_listed() {
    // the entries and result are passed through from the host, which the harness can't read, see
    // `test_harness::resolve`, the entries are listed from the opened directories instead
    let opened = read(async |accessor, exports, h| {
        let d = descriptor(exports);
        for fd in [h.root, h.dir] {
            d.call_read_directory(accessor, fd).await?;
        }
        let read = DescriptorFlags::READ;
        let mut opened = vec![];
        for path in ["dir", "empty", "file.txt", "dir/nested.txt"] {
            let open_flags = match path {
                "dir" | "empty" => OpenFlags::DIRECTORY,
                _ => OpenFlags::empty(),
            };
            opened.push(ok(
                open(accessor, exports, h.root, path, open_flags, read).await?
            ));
        }
        Ok(opened)
    })
    .await;
    assert!(opened.iter().all(Result::is_ok), "{opened:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn metadata_is_read() {
    let results = read(async |accessor, exports, h| {
        let d = descriptor(exports);
        let file = d.call_stat(accessor, h.file).await?.expect("stat");
        let root = d.call_stat(accessor, h.root).await?.expect("stat root");
        let file_at = d
            .call_stat_at(accessor, h.root, PathFlags::empty(), "file.txt".into())
            .await?
            .expect("stat-at");
        let link = d
            .call_stat_at(accessor, h.root, PathFlags::empty(), "link".into())
            .await?
            .expect("stat-at link");
        let followed = d
            .call_stat_at(accessor, h.root, PathFlags::SYMLINK_FOLLOW, "link".into())
            .await?
            .expect("stat-at followed");
        let types = (
            d.call_get_type(accessor, h.file).await?,
            d.call_get_type(accessor, h.dir).await?,
        );
        Ok((file, root, file_at, link, followed, types))
    })
    .await;
    let (file, root, file_at, link, followed, types) = results;
    assert_eq!(file.type_, DescriptorType::RegularFile);
    assert_eq!(file.size, FILE.len() as u64);
    assert_eq!(root.type_, DescriptorType::Directory);
    assert_eq!(file_at, file);
    assert_eq!(link.type_, DescriptorType::SymbolicLink);
    assert_eq!(followed, file);
    assert_eq!(
        types,
        (
            Ok(DescriptorType::RegularFile),
            Ok(DescriptorType::Directory)
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn metadata_hashes_are_read() {
    let (file, file_at, followed, other) = read(async |accessor, exports, h| {
        let d = descriptor(exports);
        Ok((
            d.call_metadata_hash(accessor, h.file).await?,
            d.call_metadata_hash_at(accessor, h.root, PathFlags::empty(), "file.txt".into())
                .await?,
            d.call_metadata_hash_at(accessor, h.root, PathFlags::SYMLINK_FOLLOW, "link".into())
                .await?,
            d.call_metadata_hash(accessor, h.nested).await?,
        ))
    })
    .await;
    assert!(file.is_ok(), "{file:?}");
    assert_eq!(file_at, file);
    assert_eq!(followed, file);
    assert_ne!(other, file);
}

#[tokio::test(flavor = "multi_thread")]
async fn links_are_read() {
    let target = read(async |accessor, exports, h| {
        Ok(descriptor(exports)
            .call_readlink_at(accessor, h.root, "link".into())
            .await?)
    })
    .await;
    assert_eq!(target, Ok("file.txt".into()));
}

#[tokio::test(flavor = "multi_thread")]
async fn descriptors_are_compared() {
    let (same, different) = read(async |accessor, exports, h| {
        let again = open(
            accessor,
            exports,
            h.root,
            "dir",
            OpenFlags::DIRECTORY,
            DescriptorFlags::READ,
        )
        .await?
        .expect("open dir");
        let d = descriptor(exports);
        Ok((
            d.call_is_same_object(accessor, h.dir, again).await?,
            d.call_is_same_object(accessor, h.dir, h.root).await?,
        ))
    })
    .await;
    assert!(same);
    assert!(!different);
}

#[tokio::test(flavor = "multi_thread")]
async fn advice_is_given() {
    let result = read(async |accessor, exports, h| {
        Ok(descriptor(exports)
            .call_advise(accessor, h.file, 0, 0, Advice::Sequential)
            .await?)
    })
    .await;
    assert_eq!(result, Ok(()));
}

#[tokio::test(flavor = "multi_thread")]
async fn flags_are_read_only() {
    let flags = read(async |accessor, exports, h| {
        let d = descriptor(exports);
        Ok(vec![
            d.call_get_flags(accessor, h.root).await?,
            d.call_get_flags(accessor, h.dir).await?,
            d.call_get_flags(accessor, h.file).await?,
            d.call_get_flags(accessor, h.nested).await?,
        ])
    })
    .await;
    // the host preopen is writable, the readonly descriptors are not
    for (index, flags) in flags.into_iter().enumerate() {
        assert_eq!(flags, Ok(DescriptorFlags::READ), "descriptor {index}");
    }
}

// --- mutations are denied ---

/// Every combination of open flags and descriptor flags that would let a descriptor change the
/// filesystem.
fn mutating_opens() -> Vec<(OpenFlags, DescriptorFlags)> {
    let read = DescriptorFlags::READ;
    vec![
        (OpenFlags::CREATE, read),
        (OpenFlags::CREATE | OpenFlags::EXCLUSIVE, read),
        (OpenFlags::EXCLUSIVE, read),
        (OpenFlags::TRUNCATE, read),
        (OpenFlags::empty(), DescriptorFlags::WRITE),
        (OpenFlags::empty(), read | DescriptorFlags::WRITE),
        (
            OpenFlags::empty(),
            read | DescriptorFlags::FILE_INTEGRITY_SYNC,
        ),
        (
            OpenFlags::empty(),
            read | DescriptorFlags::DATA_INTEGRITY_SYNC,
        ),
        (
            OpenFlags::empty(),
            read | DescriptorFlags::REQUESTED_WRITE_SYNC,
        ),
        (OpenFlags::empty(), read | DescriptorFlags::MUTATE_DIRECTORY),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn opening_existing_files_for_mutation_is_denied() {
    assert_denied(async |accessor, exports, h| {
        let mut results = vec![];
        for (open_flags, flags) in mutating_opens() {
            for (fd, path) in [
                (h.root, "file.txt"),
                (h.root, "link"),
                (h.dir, "nested.txt"),
            ] {
                results.push(ok(
                    open(accessor, exports, fd, path, open_flags, flags).await?
                ));
            }
        }
        Ok(results)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn opening_new_files_is_denied() {
    assert_denied(async |accessor, exports, h| {
        let mut results = vec![];
        for (open_flags, flags) in mutating_opens() {
            for (fd, path) in [
                (h.root, "new.txt"),
                (h.dir, "new.txt"),
                (h.root, "empty/new.txt"),
            ] {
                results.push(ok(
                    open(accessor, exports, fd, path, open_flags, flags).await?
                ));
            }
        }
        Ok(results)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn opening_directories_for_mutation_is_denied() {
    assert_denied(async |accessor, exports, h| {
        let read = DescriptorFlags::READ;
        let mut results = vec![];
        for flags in [
            read | DescriptorFlags::MUTATE_DIRECTORY,
            read | DescriptorFlags::WRITE,
        ] {
            for (fd, path) in [(h.root, "."), (h.root, "empty"), (h.dir, ".")] {
                results.push(ok(open(
                    accessor,
                    exports,
                    fd,
                    path,
                    OpenFlags::DIRECTORY,
                    flags,
                )
                .await?));
            }
        }
        Ok(results)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn writing_is_denied() {
    assert_denied(async |accessor, exports, h| {
        let d = descriptor(exports);
        let mut results = vec![];
        for (fd, offset) in [(h.file, 0), (h.file, 6), (h.nested, 0)] {
            let data = test_harness::stream(accessor, b"changed".to_vec())?;
            let result = d.call_write_via_stream(accessor, fd, data, offset).await?;
            results.push(test_harness::resolve(accessor, result).await?);
        }
        Ok(results)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn appending_is_denied() {
    assert_denied(async |accessor, exports, h| {
        let d = descriptor(exports);
        let mut results = vec![];
        for fd in [h.file, h.nested] {
            let data = test_harness::stream(accessor, b"changed".to_vec())?;
            let result = d.call_append_via_stream(accessor, fd, data).await?;
            results.push(test_harness::resolve(accessor, result).await?);
        }
        Ok(results)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn resizing_is_denied() {
    assert_denied(async |accessor, exports, h| {
        let d = descriptor(exports);
        Ok(vec![
            d.call_set_size(accessor, h.file, 0).await?,
            d.call_set_size(accessor, h.file, 1024).await?,
            d.call_set_size(accessor, h.nested, 0).await?,
        ])
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn setting_times_is_denied() {
    assert_denied(async |accessor, exports, h| {
        let d = descriptor(exports);
        let now = || NewTimestamp::Now;
        let mut results = vec![];
        for fd in [h.file, h.root, h.dir, h.nested] {
            results.push(d.call_set_times(accessor, fd, now(), now()).await?);
            results.push(
                d.call_set_times(accessor, fd, NewTimestamp::NoChange, NewTimestamp::Now)
                    .await?,
            );
        }
        for (fd, path_flags, path) in [
            (h.root, PathFlags::empty(), "file.txt"),
            (h.root, PathFlags::SYMLINK_FOLLOW, "link"),
            (h.root, PathFlags::empty(), "link"),
            (h.root, PathFlags::empty(), "empty"),
            (h.dir, PathFlags::empty(), "nested.txt"),
        ] {
            results.push(
                d.call_set_times_at(accessor, fd, path_flags, path.into(), now(), now())
                    .await?,
            );
        }
        Ok(results)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn syncing_is_denied() {
    assert_denied(async |accessor, exports, h| {
        let d = descriptor(exports);
        let mut results = vec![];
        for fd in [h.file, h.root, h.nested] {
            results.push(d.call_sync(accessor, fd).await?);
            results.push(d.call_sync_data(accessor, fd).await?);
        }
        Ok(results)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn creating_directories_is_denied() {
    assert_denied(async |accessor, exports, h| {
        let d = descriptor(exports);
        Ok(vec![
            d.call_create_directory_at(accessor, h.root, "new".into())
                .await?,
            d.call_create_directory_at(accessor, h.root, "empty/new".into())
                .await?,
            d.call_create_directory_at(accessor, h.dir, "new".into())
                .await?,
            // already exists, denied before the host is asked
            d.call_create_directory_at(accessor, h.root, "empty".into())
                .await?,
        ])
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_directories_is_denied() {
    assert_denied(async |accessor, exports, h| {
        let d = descriptor(exports);
        Ok(vec![
            d.call_remove_directory_at(accessor, h.root, "empty".into())
                .await?,
            d.call_remove_directory_at(accessor, h.root, "dir".into())
                .await?,
            d.call_remove_directory_at(accessor, h.root, "missing".into())
                .await?,
        ])
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unlinking_is_denied() {
    assert_denied(async |accessor, exports, h| {
        let d = descriptor(exports);
        Ok(vec![
            d.call_unlink_file_at(accessor, h.root, "file.txt".into())
                .await?,
            d.call_unlink_file_at(accessor, h.root, "link".into())
                .await?,
            d.call_unlink_file_at(accessor, h.dir, "nested.txt".into())
                .await?,
            d.call_unlink_file_at(accessor, h.root, "missing.txt".into())
                .await?,
        ])
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn renaming_is_denied() {
    assert_denied(async |accessor, exports, h| {
        let d = descriptor(exports);
        Ok(vec![
            d.call_rename_at(
                accessor,
                h.root,
                "file.txt".into(),
                h.root,
                "renamed.txt".into(),
            )
            .await?,
            d.call_rename_at(
                accessor,
                h.root,
                "file.txt".into(),
                h.dir,
                "moved.txt".into(),
            )
            .await?,
            d.call_rename_at(
                accessor,
                h.dir,
                "nested.txt".into(),
                h.root,
                "file.txt".into(),
            )
            .await?,
            d.call_rename_at(accessor, h.root, "empty".into(), h.root, "renamed".into())
                .await?,
            d.call_rename_at(
                accessor,
                h.root,
                "link".into(),
                h.root,
                "renamed-link".into(),
            )
            .await?,
        ])
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn hard_linking_is_denied() {
    assert_denied(async |accessor, exports, h| {
        let d = descriptor(exports);
        Ok(vec![
            d.call_link_at(
                accessor,
                h.root,
                PathFlags::empty(),
                "file.txt".into(),
                h.root,
                "hard.txt".into(),
            )
            .await?,
            d.call_link_at(
                accessor,
                h.root,
                PathFlags::SYMLINK_FOLLOW,
                "link".into(),
                h.dir,
                "hard.txt".into(),
            )
            .await?,
            d.call_link_at(
                accessor,
                h.dir,
                PathFlags::empty(),
                "nested.txt".into(),
                h.root,
                "hard.txt".into(),
            )
            .await?,
        ])
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn symlinking_is_denied() {
    assert_denied(async |accessor, exports, h| {
        let d = descriptor(exports);
        Ok(vec![
            d.call_symlink_at(accessor, h.root, "file.txt".into(), "new-link".into())
                .await?,
            d.call_symlink_at(accessor, h.dir, "nested.txt".into(), "new-link".into())
                .await?,
            // already exists, denied before the host is asked
            d.call_symlink_at(accessor, h.root, "file.txt".into(), "link".into())
                .await?,
        ])
    })
    .await;
}
