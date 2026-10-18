//! Tests for `latch-readonly`, composed from `latch-readonly.wac`, as the gate's latch.
//!
//! Each test preopens a fresh copy of this tree read-write as `/`, so any denial comes from the
//! latch rather than the host:
//!
//! ```text
//! file.txt          "hello"
//! link -> file.txt
//! empty/
//! dir/nested.txt    "nested"
//! ```

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;

use tempfile::TempDir;
use test_harness::bindings::exports::wasi::filesystem::types::{GuestDescriptor, NewTimestamp};
use test_harness::{
    Ctx, DescriptorFlags, DescriptorType, ErrorCode, Exports, Harness, OpenFlags, PathFlags, Perms,
};
use wasmtime::component::{Accessor, ResourceAny};

fn fixture() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    fs::write(root.join("file.txt"), "hello").unwrap();
    symlink("file.txt", root.join("link")).unwrap();
    fs::create_dir(root.join("empty")).unwrap();
    fs::create_dir(root.join("dir")).unwrap();
    fs::write(root.join("dir/nested.txt"), "nested").unwrap();
    dir
}

fn descriptor(exports: &Exports) -> GuestDescriptor<'_> {
    exports.wasi_filesystem_types().descriptor()
}

async fn root(accessor: &Accessor<Ctx>, exports: &Exports) -> wasmtime::Result<ResourceAny> {
    let dirs = exports
        .wasi_filesystem_preopens()
        .call_get_directories(accessor)
        .await?;
    Ok(dirs.into_iter().next().expect("a preopen").0)
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

fn ok<T, E>(result: Result<T, E>) -> Result<(), E> {
    result.map(|_| ())
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_are_deferred() -> wasmtime::Result<()> {
    let dir = fixture();
    let mut gate = Harness::new("gate")
        .preopen(dir.path(), "/", Perms::ReadWrite)
        .latch("latch-readonly")
        .build()
        .await?;
    let (data, stat, target, nested) = gate
        .run(async |accessor, exports| {
            let d = descriptor(exports);
            let root = root(accessor, exports).await?;
            let read = DescriptorFlags::READ;
            let file = open(accessor, exports, root, "file.txt", OpenFlags::empty(), read)
                .await?
                .expect("open file.txt");
            let (data, _) = d.call_read_via_stream(accessor, file, 0).await?;
            let mut data = test_harness::collect(accessor, data)?;
            let mut bytes = vec![];
            while let Some(byte) = data.recv().await {
                bytes.push(byte);
            }
            let stat = d.call_stat(accessor, file).await?.expect("stat");
            let target = d.call_readlink_at(accessor, root, "link".into()).await?;
            let nested = ok(open(
                accessor,
                exports,
                root,
                "dir/nested.txt",
                OpenFlags::empty(),
                read,
            )
            .await?);
            Ok((bytes, stat, target, nested))
        })
        .await?;
    assert_eq!(data, b"hello");
    assert_eq!(stat.type_, DescriptorType::RegularFile);
    assert_eq!(target, Ok("file.txt".into()));
    assert_eq!(nested, Ok(()));
    assert_eq!(gate.recorder().logs(), []);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn mutations_are_denied_with_read_only() -> wasmtime::Result<()> {
    let dir = fixture();
    let mut gate = Harness::new("gate")
        .preopen(dir.path(), "/", Perms::ReadWrite)
        .latch("latch-readonly")
        .build()
        .await?;
    let results = gate
        .run(async |accessor, exports| {
            let d = descriptor(exports);
            let root = root(accessor, exports).await?;
            let read = DescriptorFlags::READ;
            let file = open(accessor, exports, root, "file.txt", OpenFlags::empty(), read)
                .await?
                .expect("open file.txt");
            let now = || NewTimestamp::Now;
            let mut results = vec![];
            for (path, open_flags, flags) in [
                ("file.txt", OpenFlags::empty(), read | DescriptorFlags::WRITE),
                ("file.txt", OpenFlags::TRUNCATE, read),
                ("new.txt", OpenFlags::CREATE, read),
                (".", OpenFlags::DIRECTORY, read | DescriptorFlags::MUTATE_DIRECTORY),
            ] {
                results.push(ok(
                    open(accessor, exports, root, path, open_flags, flags).await?
                ));
            }
            let data = test_harness::stream(accessor, b"changed".to_vec())?;
            let result = d.call_write_via_stream(accessor, file, data, 0).await?;
            results.push(test_harness::resolve(accessor, result).await?);
            let data = test_harness::stream(accessor, b"changed".to_vec())?;
            let result = d.call_append_via_stream(accessor, file, data).await?;
            results.push(test_harness::resolve(accessor, result).await?);
            results.push(d.call_set_size(accessor, file, 0).await?);
            results.push(d.call_set_times(accessor, root, now(), now()).await?);
            results.push(d.call_sync(accessor, file).await?);
            results.push(d.call_sync_data(accessor, file).await?);
            results.push(
                d.call_create_directory_at(accessor, root, "new".into())
                    .await?,
            );
            results.push(
                d.call_set_times_at(
                    accessor,
                    root,
                    PathFlags::empty(),
                    "file.txt".into(),
                    now(),
                    now(),
                )
                .await?,
            );
            results.push(
                d.call_link_at(
                    accessor,
                    root,
                    PathFlags::empty(),
                    "file.txt".into(),
                    root,
                    "hard.txt".into(),
                )
                .await?,
            );
            results.push(
                d.call_rename_at(accessor, root, "file.txt".into(), root, "moved.txt".into())
                    .await?,
            );
            results.push(
                d.call_symlink_at(accessor, root, "file.txt".into(), "new-link".into())
                    .await?,
            );
            results.push(
                d.call_remove_directory_at(accessor, root, "empty".into())
                    .await?,
            );
            results.push(
                d.call_unlink_file_at(accessor, root, "link".into())
                    .await?,
            );
            Ok(results)
        })
        .await?;
    for (index, result) in results.into_iter().enumerate() {
        assert_eq!(result, Err(ErrorCode::ReadOnly), "operation {index}");
    }

    // nothing was changed
    let root = dir.path();
    assert_eq!(fs::read(root.join("file.txt")).unwrap(), b"hello");
    assert!(root.join("empty").is_dir());
    assert!(root.join("link").is_symlink());
    for path in ["new.txt", "new", "hard.txt", "moved.txt", "new-link"] {
        assert!(!root.join(path).exists(), "{path}");
    }
    Ok(())
}
