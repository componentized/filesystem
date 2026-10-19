//! Tests for `latch-deny-symlink-at`, as the gate's latch.
//!
//! Each test preopens a fresh copy of this tree read-write as `/`, so any denial comes from the
//! latch rather than the host:
//!
//! ```text
//! file.txt          "hello"
//! link -> file.txt
//! ```

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;

use tempfile::TempDir;
use test_harness::bindings::exports::wasi::filesystem::types::{GuestDescriptor, NewTimestamp};
use test_harness::{
    Ctx, DescriptorFlags, DescriptorType, ErrorCode, Exports, Harness, LogEntry, OpenFlags,
    PathFlags, Perms, TestSubject,
};
use wasmtime::component::{Accessor, ResourceAny};

fn fixture() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("file.txt"), "hello").unwrap();
    symlink("file.txt", dir.path().join("link")).unwrap();
    dir
}

async fn gate(dir: &TempDir) -> TestSubject {
    Harness::new("gate")
        .preopen(dir.path(), "/", Perms::ReadWrite)
        .latch("latch-deny-symlink-at")
        .build()
        .await
        .expect("gate")
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
    root: ResourceAny,
    path_flags: PathFlags,
    path: &str,
) -> wasmtime::Result<Result<(), ErrorCode>> {
    let result = descriptor(exports)
        .call_open_at(
            accessor,
            root,
            path_flags,
            path.into(),
            OpenFlags::empty(),
            DescriptorFlags::READ,
        )
        .await?;
    Ok(result.map(|_| ()))
}

fn ok<T, E>(result: Result<T, E>) -> Result<(), E> {
    result.map(|_| ())
}

#[tokio::test(flavor = "multi_thread")]
async fn creating_symlinks_is_denied() -> wasmtime::Result<()> {
    let dir = fixture();
    let mut gate = gate(&dir).await;
    let result = gate
        .run(async |accessor, exports| {
            let root = root(accessor, exports).await?;
            descriptor(exports)
                .call_symlink_at(accessor, root, "file.txt".into(), "new-link".into())
                .await
        })
        .await?;
    assert_eq!(result, Err(ErrorCode::NotPermitted));
    assert!(fs::symlink_metadata(dir.path().join("new-link")).is_err());
    assert_eq!(
        gate.recorder().logs(),
        [LogEntry::warn(
            "componentized-gate",
            "Denied REASON=not-permitted OPERATION=wasi:filesystem/types#descriptor.symlink-at FD=/ OLD-PATH=file.txt NEW-PATH=new-link"
        )]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn existing_symlinks_are_followed() -> wasmtime::Result<()> {
    let dir = fixture();
    let mut gate = gate(&dir).await;
    let (opened, stat, hash, times) = gate
        .run(async |accessor, exports| {
            let d = descriptor(exports);
            let root = root(accessor, exports).await?;
            let follow = PathFlags::SYMLINK_FOLLOW;
            Ok((
                open(accessor, exports, root, follow, "link").await?,
                d.call_stat_at(accessor, root, follow, "link".into())
                    .await?
                    .map(|stat| stat.type_),
                ok(d.call_metadata_hash_at(accessor, root, follow, "link".into())
                    .await?),
                d.call_set_times_at(
                    accessor,
                    root,
                    follow,
                    "link".into(),
                    NewTimestamp::Now,
                    NewTimestamp::Now,
                )
                .await?,
            ))
        })
        .await?;
    assert_eq!(opened, Ok(()));
    assert_eq!(stat, Ok(DescriptorType::RegularFile));
    assert_eq!(hash, Ok(()));
    assert_eq!(times, Ok(()));
    assert_eq!(gate.recorder().logs(), []);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn symlinks_are_read_without_following() -> wasmtime::Result<()> {
    let dir = fixture();
    let mut gate = gate(&dir).await;
    let (stat, target) = gate
        .run(async |accessor, exports| {
            let d = descriptor(exports);
            let root = root(accessor, exports).await?;
            Ok((
                d.call_stat_at(accessor, root, PathFlags::empty(), "link".into())
                    .await?
                    .map(|stat| stat.type_),
                d.call_readlink_at(accessor, root, "link".into()).await?,
            ))
        })
        .await?;
    assert_eq!(stat, Ok(DescriptorType::SymbolicLink));
    assert_eq!(target, Ok("file.txt".into()));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn symlinks_can_be_removed() -> wasmtime::Result<()> {
    let dir = fixture();
    let mut gate = gate(&dir).await;
    let result = gate
        .run(async |accessor, exports| {
            let root = root(accessor, exports).await?;
            descriptor(exports)
                .call_unlink_file_at(accessor, root, "link".into())
                .await
        })
        .await?;
    assert_eq!(result, Ok(()));
    assert!(fs::symlink_metadata(dir.path().join("link")).is_err());
    Ok(())
}
