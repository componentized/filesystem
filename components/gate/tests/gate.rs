//! The gate asks its latch to authorize every filesystem call, and passes the call through unless
//! the latch denies it. A denied call fails with the reason the latch gave, a latch error fails it
//! with `other`, and both are logged in the `componentized-gate` context.
//!
//! The `latch-deny-all` and `latch-defer-all` components are the gate's latch, alone and
//! aggregated. A scripted host latch denies or fails individual operations, and records the
//! operations it was asked about.
//!
//! Each test preopens a fresh copy of this tree read-write as `/`:
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
use std::path::Path;

use tempfile::TempDir;
use test_harness::bindings::exports::wasi::filesystem::types::{
    Advice, DirectoryEntry, GuestDescriptor, NewTimestamp,
};
use test_harness::{
    Ctx, DescriptorFlags, DescriptorType, ErrorCode, Exports, Harness, HostLatch, LogEntry,
    Observation, OpenFlags, PathFlags, Perms, TestSubject,
};
use wasmtime::component::{Accessor, ResourceAny};

const CONTEXT: &str = "componentized-gate";
const TYPES: &str = "OPERATION=wasi:filesystem/types#descriptor";

/// Every descriptor operation but `open-at`, which the tests need to open their handles.
const DESCRIPTOR_OPERATIONS: &[&str] = &[
    "descriptor.read-via-stream",
    "descriptor.write-via-stream",
    "descriptor.append-via-stream",
    "descriptor.advise",
    "descriptor.sync-data",
    "descriptor.get-flags",
    "descriptor.get-type",
    "descriptor.set-size",
    "descriptor.set-times",
    "descriptor.read-directory",
    "descriptor.sync",
    "descriptor.create-directory-at",
    "descriptor.stat",
    "descriptor.stat-at",
    "descriptor.set-times-at",
    "descriptor.link-at",
    "descriptor.readlink-at",
    "descriptor.remove-directory-at",
    "descriptor.rename-at",
    "descriptor.symlink-at",
    "descriptor.unlink-file-at",
    "descriptor.metadata-hash",
    "descriptor.metadata-hash-at",
];

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("file.txt"), "hello").unwrap();
        symlink("file.txt", root.join("link")).unwrap();
        fs::create_dir(root.join("empty")).unwrap();
        fs::create_dir(root.join("dir")).unwrap();
        fs::write(root.join("dir/nested.txt"), "nested").unwrap();
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The gate with the fixture preopened as `/`.
    fn gate(&self) -> Harness {
        Harness::new("gate").preopen(self.path(), "/", Perms::ReadWrite)
    }
}

/// Descriptors opened through the gate.
#[derive(Clone, Copy)]
struct Handles {
    /// The preopen, `/`
    root: ResourceAny,
    /// `/dir`
    dir: ResourceAny,
    /// `/file.txt`, opened for reading and writing
    file: ResourceAny,
}

fn descriptor(exports: &Exports) -> GuestDescriptor<'_> {
    exports.wasi_filesystem_types().descriptor()
}

async fn preopens(
    accessor: &Accessor<Ctx>,
    exports: &Exports,
) -> wasmtime::Result<Vec<(ResourceAny, String)>> {
    exports
        .wasi_filesystem_preopens()
        .call_get_directories(accessor)
        .await
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
    let (root, _) = preopens(accessor, exports)
        .await?
        .into_iter()
        .next()
        .expect("a preopen");
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
        read | DescriptorFlags::WRITE,
    )
    .await?
    .expect("open file.txt");
    Ok(Handles { root, dir, file })
}

/// Build the gate and run the test body against handles opened through it.
async fn run<R: Send + 'static>(
    gate: Harness,
    f: impl AsyncFnOnce(&Accessor<Ctx>, &Exports, Handles) -> wasmtime::Result<R> + Send,
) -> (R, TestSubject) {
    let mut gate = gate.build().await.expect("gate");
    let result = gate
        .run(async |accessor, exports| {
            let handles = handles(accessor, exports).await?;
            f(accessor, exports, handles).await
        })
        .await
        .expect("run");
    (result, gate)
}

/// The bytes of a stream the gate returned.
async fn read_bytes(
    accessor: &Accessor<Ctx>,
    data: wasmtime::component::StreamReader<u8>,
) -> wasmtime::Result<Vec<u8>> {
    let mut data = test_harness::collect(accessor, data)?;
    let mut bytes = vec![];
    while let Some(byte) = data.recv().await {
        bytes.push(byte);
    }
    Ok(bytes)
}

/// The entries of a directory stream the gate returned, sorted by name, and the read's result.
async fn read_entries(
    accessor: &Accessor<Ctx>,
    exports: &Exports,
    fd: ResourceAny,
) -> wasmtime::Result<(Vec<String>, Result<(), ErrorCode>)> {
    let (entries, result) = descriptor(exports)
        .call_read_directory(accessor, fd)
        .await?;
    let mut entries = test_harness::collect(accessor, entries)?;
    let mut names = vec![];
    while let Some(DirectoryEntry { name, .. }) = entries.recv().await {
        names.push(name);
    }
    names.sort();
    Ok((names, test_harness::resolve(accessor, result).await?))
}

fn ok<T, E>(result: Result<T, E>) -> Result<(), E> {
    result.map(|_| ())
}

fn warn(message: impl Into<String>) -> LogEntry {
    LogEntry::warn(CONTEXT, message)
}

fn trace(message: impl Into<String>) -> LogEntry {
    LogEntry::trace(CONTEXT, message)
}

fn error(message: impl Into<String>) -> LogEntry {
    LogEntry::error(CONTEXT, message)
}

// --- latch-deny-all ---

#[tokio::test(flavor = "multi_thread")]
async fn deny_all_hides_every_preopen() {
    let fixture = Fixture::new();
    let mut gate = fixture
        .gate()
        .preopen(fixture.path().join("dir"), "/data", Perms::ReadWrite)
        .latch("latch-deny-all")
        .build()
        .await
        .unwrap();
    let names = gate
        .run(async |accessor, exports| {
            let dirs = preopens(accessor, exports).await?;
            Ok(dirs.into_iter().map(|(_, name)| name).collect::<Vec<_>>())
        })
        .await
        .unwrap();
    assert!(names.is_empty(), "{names:?}");
    assert_eq!(
        gate.recorder().logs(),
        [warn(
            "Denied REASON=access OPERATION=wasi:filesystem/preopens#get-directories"
        )]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn deny_all_wins_over_defer_all() {
    let fixture = Fixture::new();
    let mut gate = fixture
        .gate()
        .latch("latch-defer-all")
        .latch("latch-deny-all")
        .build()
        .await
        .unwrap();
    let dirs = gate
        .run(async |accessor, exports| Ok(preopens(accessor, exports).await?.len()))
        .await
        .unwrap();
    assert_eq!(dirs, 0);
    assert_eq!(
        gate.recorder().logs(),
        [warn(
            "Denied REASON=access OPERATION=wasi:filesystem/preopens#get-directories"
        )]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn deny_all_decision_is_observed_by_every_latch() {
    let fixture = Fixture::new();
    let mut gate = fixture
        .gate()
        .latch("latch-deny-all")
        .host_latch(HostLatch::defer())
        .build()
        .await
        .unwrap();
    gate.run(async |accessor, exports| Ok(preopens(accessor, exports).await?.len()))
        .await
        .unwrap();
    let recorder = gate.recorder();
    // the host latch is aggregated after latch-deny-all, it is not asked once the call is denied
    assert!(recorder.operations().is_empty(), "{:?}", recorder.operations());
    assert_eq!(
        recorder.observations(),
        [Observation {
            operation: "preopens.get-directories".into(),
            denied: true,
        }]
    );
}

// --- latch-defer-all ---

#[tokio::test(flavor = "multi_thread")]
async fn defer_all_passes_preopens_through() {
    let fixture = Fixture::new();
    let mut gate = fixture
        .gate()
        .preopen(fixture.path().join("dir"), "/data", Perms::ReadWrite)
        .latch("latch-defer-all")
        .build()
        .await
        .unwrap();
    let names = gate
        .run(async |accessor, exports| {
            let dirs = preopens(accessor, exports).await?;
            Ok(dirs.into_iter().map(|(_, name)| name).collect::<Vec<_>>())
        })
        .await
        .unwrap();
    assert_eq!(names, ["/", "/data"]);
    assert_eq!(gate.recorder().logs(), []);
}

#[tokio::test(flavor = "multi_thread")]
async fn defer_all_passes_reads_through() {
    let fixture = Fixture::new();
    let ((data, offset, entries, stat, stat_at, kind, hashes, target, advised), gate) = run(
        fixture.gate().latch("latch-defer-all"),
        async |accessor, exports, h| {
            let d = descriptor(exports);
            let (data, _) = d.call_read_via_stream(accessor, h.file, 0).await?;
            let data = read_bytes(accessor, data).await?;
            let (offset, _) = d.call_read_via_stream(accessor, h.file, 1).await?;
            let offset = read_bytes(accessor, offset).await?;
            let entries = read_entries(accessor, exports, h.root).await?;
            let stat = d.call_stat(accessor, h.file).await?.expect("stat");
            let stat_at = d
                .call_stat_at(accessor, h.root, PathFlags::empty(), "link".into())
                .await?
                .expect("stat-at");
            let kind = d.call_get_type(accessor, h.dir).await?;
            let hashes = (
                d.call_metadata_hash(accessor, h.file).await?,
                d.call_metadata_hash_at(accessor, h.root, PathFlags::empty(), "file.txt".into())
                    .await?,
            );
            let target = d.call_readlink_at(accessor, h.root, "link".into()).await?;
            let advised = d
                .call_advise(accessor, h.file, 0, 0, Advice::Sequential)
                .await?;
            Ok((
                data, offset, entries, stat, stat_at, kind, hashes, target, advised,
            ))
        },
    )
    .await;
    assert_eq!(data, b"hello");
    assert_eq!(offset, b"ello");
    assert_eq!(
        entries,
        (
            vec!["dir".into(), "empty".into(), "file.txt".into(), "link".into()],
            Ok(())
        )
    );
    assert_eq!(stat.type_, DescriptorType::RegularFile);
    assert_eq!(stat.size, 5);
    assert_eq!(stat_at.type_, DescriptorType::SymbolicLink);
    assert_eq!(kind, Ok(DescriptorType::Directory));
    assert!(hashes.0.is_ok(), "{hashes:?}");
    assert_eq!(hashes.0, hashes.1);
    assert_eq!(target, Ok("file.txt".into()));
    assert_eq!(advised, Ok(()));
    assert_eq!(gate.recorder().logs(), []);
}

#[tokio::test(flavor = "multi_thread")]
async fn defer_all_passes_writes_through() {
    let fixture = Fixture::new();
    let (results, gate) = run(
        fixture.gate().latch("latch-defer-all"),
        async |accessor, exports, h| {
            let d = descriptor(exports);
            let now = || NewTimestamp::Now;
            Ok(vec![
                ok(d.call_get_flags(accessor, h.file).await?),
                d.call_set_size(accessor, h.file, 2).await?,
                d.call_set_times(accessor, h.file, now(), now()).await?,
                d.call_sync_data(accessor, h.file).await?,
                d.call_sync(accessor, h.file).await?,
                d.call_create_directory_at(accessor, h.root, "new".into())
                    .await?,
                d.call_set_times_at(
                    accessor,
                    h.root,
                    PathFlags::empty(),
                    "new".into(),
                    now(),
                    now(),
                )
                .await?,
                d.call_symlink_at(accessor, h.root, "file.txt".into(), "new-link".into())
                    .await?,
                d.call_link_at(
                    accessor,
                    h.root,
                    PathFlags::empty(),
                    "file.txt".into(),
                    h.dir,
                    "hard.txt".into(),
                )
                .await?,
                d.call_rename_at(accessor, h.root, "new".into(), h.dir, "renamed".into())
                    .await?,
                d.call_remove_directory_at(accessor, h.dir, "renamed".into())
                    .await?,
                d.call_unlink_file_at(accessor, h.root, "new-link".into())
                    .await?,
                d.call_remove_directory_at(accessor, h.root, "empty".into())
                    .await?,
            ])
        },
    )
    .await;
    for (index, result) in results.into_iter().enumerate() {
        assert_eq!(result, Ok(()), "operation {index}");
    }
    let root = fixture.path();
    assert_eq!(fs::read(root.join("file.txt")).unwrap(), b"he");
    assert_eq!(fs::read(root.join("dir/hard.txt")).unwrap(), b"he");
    assert!(!root.join("new").exists());
    assert!(!root.join("dir/renamed").exists());
    assert!(!root.join("new-link").exists());
    assert!(!root.join("empty").exists());
    assert_eq!(gate.recorder().logs(), []);
}

// --- authorization ---

#[tokio::test(flavor = "multi_thread")]
async fn every_operation_is_authorized_and_observed() {
    let fixture = Fixture::new();
    let (_, gate) = run(
        fixture.gate().host_latch(HostLatch::defer()),
        async |accessor, exports, h| {
            let d = descriptor(exports);
            let now = || NewTimestamp::Now;
            let (data, _) = d.call_read_via_stream(accessor, h.file, 1).await?;
            read_bytes(accessor, data).await?;
            let data = test_harness::stream(accessor, b"!".to_vec())?;
            let _ = d.call_write_via_stream(accessor, h.file, data, 5).await?;
            let data = test_harness::stream(accessor, b"!".to_vec())?;
            let _ = d.call_append_via_stream(accessor, h.file, data).await?;
            let _ = d.call_advise(accessor, h.file, 0, 0, Advice::Random).await?;
            let _ = d.call_sync_data(accessor, h.file).await?;
            let _ = d.call_get_flags(accessor, h.file).await?;
            let _ = d.call_get_type(accessor, h.file).await?;
            let _ = d.call_set_size(accessor, h.file, 5).await?;
            let _ = d.call_set_times(accessor, h.file, now(), NewTimestamp::NoChange)
                .await?;
            let _ = read_entries(accessor, exports, h.dir).await?;
            let _ = d.call_sync(accessor, h.file).await?;
            let _ = d.call_create_directory_at(accessor, h.root, "new".into())
                .await?;
            let _ = d.call_stat(accessor, h.file).await?;
            let _ = d.call_stat_at(accessor, h.root, PathFlags::SYMLINK_FOLLOW, "link".into())
                .await?;
            let _ = d.call_set_times_at(
                accessor,
                h.root,
                PathFlags::empty(),
                "new".into(),
                NewTimestamp::NoChange,
                now(),
            )
            .await?;
            let _ = d.call_link_at(
                accessor,
                h.root,
                PathFlags::empty(),
                "file.txt".into(),
                h.dir,
                "hard.txt".into(),
            )
            .await?;
            let _ = d.call_readlink_at(accessor, h.root, "link".into()).await?;
            let _ = d.call_remove_directory_at(accessor, h.root, "empty".into())
                .await?;
            let _ = d.call_rename_at(accessor, h.root, "new".into(), h.dir, "renamed".into())
                .await?;
            let _ = d.call_symlink_at(accessor, h.root, "file.txt".into(), "new-link".into())
                .await?;
            let _ = d.call_unlink_file_at(accessor, h.root, "new-link".into())
                .await?;
            let _ = d.call_metadata_hash(accessor, h.file).await?;
            let _ = d.call_metadata_hash_at(accessor, h.root, PathFlags::empty(), "file.txt".into())
                .await?;
            Ok(())
        },
    )
    .await;
    let recorder = gate.recorder();
    let operations = recorder.operations();
    let expected: Vec<&str> = [
        "preopens.get-directories",
        "preopens.get-directories-item",
        "descriptor.open-at",
        "descriptor.open-at",
    ]
    .into_iter()
    .chain(DESCRIPTOR_OPERATIONS.iter().copied().flat_map(|op| match op {
        // the entries of `dir` are authorized one by one
        "descriptor.read-directory" => vec![op, "descriptor.read-directory-entry"],
        _ => vec![op],
    }))
    .collect();
    assert_eq!(operations, expected);
    // every decision is observed, in the order it was made
    assert_eq!(
        recorder.observations(),
        operations
            .iter()
            .map(|operation| Observation {
                operation: operation.clone(),
                denied: false,
            })
            .collect::<Vec<_>>()
    );
    assert_eq!(recorder.logs(), []);
}

#[tokio::test(flavor = "multi_thread")]
async fn operations_are_authorized_with_their_args() {
    let fixture = Fixture::new();
    let (_, gate) = run(
        fixture.gate().host_latch(HostLatch::defer()),
        async |accessor, exports, h| {
            let d = descriptor(exports);
            let (data, _) = d.call_read_via_stream(accessor, h.file, 1).await?;
            read_bytes(accessor, data).await?;
            let _ = d.call_set_size(accessor, h.file, 5).await?;
            let _ = read_entries(accessor, exports, h.dir).await?;
            let _ = d.call_stat_at(accessor, h.root, PathFlags::SYMLINK_FOLLOW, "link".into())
                .await?;
            let _ = d.call_rename_at(accessor, h.root, "empty".into(), h.dir, "renamed".into())
                .await?;
            Ok(())
        },
    )
    .await;
    let args: Vec<(String, String)> = gate
        .recorder()
        .authorizations()
        .into_iter()
        .map(|auth| (auth.operation, auth.args))
        .collect();
    let args: Vec<(&str, &str)> = args
        .iter()
        .map(|(operation, args)| (operation.as_str(), args.as_str()))
        .collect();
    assert_eq!(
        args,
        [
            ("preopens.get-directories", ""),
            ("preopens.get-directories-item", "\"/\""),
            (
                "descriptor.open-at",
                "DescriptorOpenAtArgs { path-flags: (), path: \"dir\", open-flags: (DIRECTORY), flags: (READ) }"
            ),
            (
                "descriptor.open-at",
                "DescriptorOpenAtArgs { path-flags: (), path: \"file.txt\", open-flags: (), flags: (READ|WRITE) }"
            ),
            (
                "descriptor.read-via-stream",
                "DescriptorReadViaStreamArgs { offset: 1 }"
            ),
            ("descriptor.set-size", "DescriptorSetSizeArgs { size: 5 }"),
            ("descriptor.read-directory", ""),
            (
                "descriptor.read-directory-entry",
                "DirectoryEntry { type: DescriptorType::RegularFile, name: \"nested.txt\" }"
            ),
            (
                "descriptor.stat-at",
                "DescriptorStatAtArgs { path-flags: (SYMLINK_FOLLOW), path: \"link\" }"
            ),
            (
                "descriptor.rename-at",
                "old_path: \"empty\", new_path: \"renamed\""
            ),
        ]
    );
}

// --- denied ---

#[tokio::test(flavor = "multi_thread")]
async fn denied_operations_fail_with_the_reason_and_are_logged() {
    let fixture = Fixture::new();
    let (results, gate) = run(
        fixture.gate().host_latch(HostLatch::deny(DESCRIPTOR_OPERATIONS)),
        async |accessor, exports, h| {
            let d = descriptor(exports);
            let now = || NewTimestamp::Now;
            let mut results = vec![];

            let (data, result) = d.call_read_via_stream(accessor, h.file, 1).await?;
            assert_eq!(read_bytes(accessor, data).await?, b"", "read-via-stream data");
            results.push(test_harness::resolve(accessor, result).await?);
            let data = test_harness::stream(accessor, b"!".to_vec())?;
            let result = d.call_write_via_stream(accessor, h.file, data, 5).await?;
            results.push(test_harness::resolve(accessor, result).await?);
            let data = test_harness::stream(accessor, b"!".to_vec())?;
            let result = d.call_append_via_stream(accessor, h.file, data).await?;
            results.push(test_harness::resolve(accessor, result).await?);
            results.push(d.call_advise(accessor, h.file, 0, 0, Advice::Random).await?);
            results.push(d.call_sync_data(accessor, h.file).await?);
            results.push(ok(d.call_get_flags(accessor, h.file).await?));
            results.push(ok(d.call_get_type(accessor, h.file).await?));
            results.push(d.call_set_size(accessor, h.file, 0).await?);
            results.push(
                d.call_set_times(accessor, h.file, now(), NewTimestamp::NoChange)
                    .await?,
            );
            let (entries, result) = read_entries(accessor, exports, h.dir).await?;
            assert!(entries.is_empty(), "read-directory entries {entries:?}");
            results.push(result);
            results.push(d.call_sync(accessor, h.file).await?);
            results.push(
                d.call_create_directory_at(accessor, h.root, "new".into())
                    .await?,
            );
            results.push(ok(d.call_stat(accessor, h.file).await?));
            results.push(ok(d
                .call_stat_at(accessor, h.root, PathFlags::SYMLINK_FOLLOW, "link".into())
                .await?));
            results.push(
                d.call_set_times_at(
                    accessor,
                    h.root,
                    PathFlags::empty(),
                    "file.txt".into(),
                    NewTimestamp::NoChange,
                    now(),
                )
                .await?,
            );
            results.push(
                d.call_link_at(
                    accessor,
                    h.root,
                    PathFlags::empty(),
                    "file.txt".into(),
                    h.dir,
                    "hard.txt".into(),
                )
                .await?,
            );
            results.push(ok(d
                .call_readlink_at(accessor, h.root, "link".into())
                .await?));
            results.push(
                d.call_remove_directory_at(accessor, h.root, "empty".into())
                    .await?,
            );
            results.push(
                d.call_rename_at(accessor, h.root, "empty".into(), h.dir, "renamed".into())
                    .await?,
            );
            results.push(
                d.call_symlink_at(accessor, h.root, "file.txt".into(), "new-link".into())
                    .await?,
            );
            results.push(
                d.call_unlink_file_at(accessor, h.root, "link".into())
                    .await?,
            );
            results.push(ok(d.call_metadata_hash(accessor, h.file).await?));
            results.push(ok(d
                .call_metadata_hash_at(accessor, h.root, PathFlags::empty(), "file.txt".into())
                .await?));
            Ok(results)
        },
    )
    .await;
    assert_eq!(results.len(), DESCRIPTOR_OPERATIONS.len());
    for (operation, result) in DESCRIPTOR_OPERATIONS.iter().zip(results) {
        assert_eq!(result, Err(ErrorCode::Access), "{operation}");
    }

    // nothing was changed
    let root = fixture.path();
    assert_eq!(fs::read(root.join("file.txt")).unwrap(), b"hello");
    assert!(root.join("empty").is_dir());
    assert!(root.join("link").is_symlink());
    assert!(!root.join("new").exists());
    assert!(!root.join("dir/hard.txt").exists());

    let denied = |call: &str| warn(format!("Denied REASON=access {TYPES}.{call}"));
    assert_eq!(
        gate.recorder().logs(),
        [
            denied("read-via-stream FD=/file.txt OFFSET=1"),
            denied("write-via-stream FD=/file.txt OFFSET=5"),
            denied("append-via-stream FD=/file.txt"),
            denied("advise FD=/file.txt OFFSET=0 LENGTH=0 ADVICE=random"),
            denied("sync-data FD=/file.txt"),
            denied("get-flags FD=/file.txt"),
            denied("get-type FD=/file.txt"),
            denied("set-size FD=/file.txt SIZE=0"),
            denied(
                "set-times FD=/file.txt ACCESS-TIMESTAMP=now MODIFICATION-TIMESTAMP=no-change"
            ),
            denied("read-directory FD=/dir"),
            denied("sync FD=/file.txt"),
            denied("create-directory-at FD=/ PATH=new"),
            denied("stat FD=/file.txt"),
            denied("stat-at FD=/ PATH-FLAGS=(symlink-follow) PATH=link"),
            denied(
                "set-times-at FD=/ PATH-FLAGS=() PATH=file.txt ACCESS-TIMESTAMP=no-change MODIFICATION-TIMESTAMP=now"
            ),
            denied(
                "link-at FD=/ OLD-PATH-FLAGS=() OLD-PATH=file.txt NEW-DESCRIPTOR=/dir NEW-PATH=hard.txt"
            ),
            denied("readlink-at FD=/ PATH=link"),
            denied("remove-directory-at FD=/ PATH=empty"),
            denied("rename-at FD=/ OLD-PATH=empty NEW-DESCRIPTOR=/dir NEW-PATH=renamed"),
            denied("symlink-at FD=/ OLD-PATH=file.txt NEW-PATH=new-link"),
            denied("unlink-file-at FD=/ PATH=link"),
            denied("metadata-hash FD=/file.txt"),
            denied("metadata-hash-at FD=/ PATH-FLAGS=() PATH=file.txt"),
        ]
    );
    // denials are observed too
    let observations = gate.recorder().observations();
    let denied: Vec<&str> = observations
        .iter()
        .filter(|observation| observation.denied)
        .map(|observation| observation.operation.as_str())
        .collect();
    assert_eq!(denied, DESCRIPTOR_OPERATIONS);
}

#[tokio::test(flavor = "multi_thread")]
async fn denied_open_fails_with_the_reason_and_is_logged() {
    let fixture = Fixture::new();
    let mut gate = fixture
        .gate()
        .host_latch(HostLatch::deny(&["descriptor.open-at"]))
        .build()
        .await
        .unwrap();
    let result = gate
        .run(async |accessor, exports| {
            let (root, _) = preopens(accessor, exports).await?.into_iter().next().unwrap();
            Ok(ok(open(
                accessor,
                exports,
                root,
                "file.txt",
                OpenFlags::CREATE,
                DescriptorFlags::READ | DescriptorFlags::WRITE,
            )
            .await?))
        })
        .await
        .unwrap();
    assert_eq!(result, Err(ErrorCode::Access));
    assert_eq!(
        gate.recorder().logs(),
        [warn(format!(
            "Denied REASON=access {TYPES}.open-at FD=/ PATH-FLAGS=() PATH=file.txt OPEN-FLAGS=(create) FLAGS=(read|write)"
        ))]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn denied_preopens_are_omitted_and_logged() {
    let fixture = Fixture::new();
    let mut gate = fixture
        .gate()
        .preopen(fixture.path().join("dir"), "/data", Perms::ReadWrite)
        .host_latch(HostLatch::new(|auth| {
            if auth.operation == "preopens.get-directories-item" && auth.args == "\"/data\"" {
                Ok(test_harness::Decision::Denied(
                    test_harness::FilesystemErrorCode::Access,
                ))
            } else {
                Ok(test_harness::Decision::Deferred)
            }
        }))
        .build()
        .await
        .unwrap();
    let names = gate
        .run(async |accessor, exports| {
            let dirs = preopens(accessor, exports).await?;
            Ok(dirs.into_iter().map(|(_, name)| name).collect::<Vec<_>>())
        })
        .await
        .unwrap();
    assert_eq!(names, ["/"]);
    assert_eq!(
        gate.recorder().logs(),
        [trace(
            "Denied REASON=access OPERATION=wasi:filesystem/preopens#get-directories PATH=/data"
        )]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn denied_directory_entries_are_omitted_and_logged() {
    let fixture = Fixture::new();
    let (entries, gate) = run(
        fixture.gate().host_latch(HostLatch::new(|auth| {
            if auth.operation == "descriptor.read-directory-entry"
                && (auth.args.contains("\"file.txt\"") || auth.args.contains("\"empty\""))
            {
                Ok(test_harness::Decision::Denied(
                    test_harness::FilesystemErrorCode::Access,
                ))
            } else {
                Ok(test_harness::Decision::Deferred)
            }
        })),
        async |accessor, exports, h| read_entries(accessor, exports, h.root).await,
    )
    .await;
    assert_eq!(entries, (vec!["dir".into(), "link".into()], Ok(())));
    let mut logs = gate.recorder().logs();
    // the order of the entries is up to the host
    logs.sort_by(|a, b| a.message.cmp(&b.message));
    assert_eq!(
        logs,
        [
            trace(format!(
                "Denied REASON=access {TYPES}.read-directory FD=/ NAME=empty TYPE=directory"
            )),
            trace(format!(
                "Denied REASON=access {TYPES}.read-directory FD=/ NAME=file.txt TYPE=regular-file"
            )),
        ]
    );
}

// --- latch errors ---

#[tokio::test(flavor = "multi_thread")]
async fn latch_errors_fail_operations_and_are_logged() {
    let fixture = Fixture::new();
    let (results, gate) = run(
        fixture.gate().host_latch(HostLatch::error(
            &[
                "descriptor.read-via-stream",
                "descriptor.write-via-stream",
                "descriptor.read-directory",
                "descriptor.stat",
            ],
            "boom",
        )),
        async |accessor, exports, h| {
            let d = descriptor(exports);
            let mut results = vec![];
            let (data, result) = d.call_read_via_stream(accessor, h.file, 0).await?;
            assert_eq!(read_bytes(accessor, data).await?, b"");
            results.push(test_harness::resolve(accessor, result).await?);
            let data = test_harness::stream(accessor, b"!".to_vec())?;
            let result = d.call_write_via_stream(accessor, h.file, data, 0).await?;
            results.push(test_harness::resolve(accessor, result).await?);
            let (entries, result) = read_entries(accessor, exports, h.dir).await?;
            assert!(entries.is_empty(), "{entries:?}");
            results.push(result);
            results.push(ok(d.call_stat(accessor, h.file).await?));
            Ok(results)
        },
    )
    .await;
    let failed = Err(ErrorCode::Other(Some("latch-error: boom".into())));
    assert_eq!(results, [failed.clone(), failed.clone(), failed.clone(), failed]);
    let latch_error = |call: &str| error(format!("Latch error CODE=boom {TYPES}.{call}"));
    assert_eq!(
        gate.recorder().logs(),
        [
            latch_error("read-via-stream FD=/file.txt OFFSET=0"),
            latch_error("write-via-stream FD=/file.txt OFFSET=0"),
            latch_error("read-directory FD=/dir"),
            latch_error("stat FD=/file.txt"),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn directory_entry_latch_error_fails_the_read() {
    let fixture = Fixture::new();
    let (entries, gate) = run(
        fixture
            .gate()
            .host_latch(HostLatch::error(&["descriptor.read-directory-entry"], "boom")),
        async |accessor, exports, h| read_entries(accessor, exports, h.dir).await,
    )
    .await;
    assert_eq!(
        entries,
        (
            vec![],
            Err(ErrorCode::Other(Some("latch-error: boom".into())))
        )
    );
    assert_eq!(
        gate.recorder().logs(),
        [error(format!(
            "Latch error CODE=boom {TYPES}.read-directory FD=/dir NAME=nested.txt TYPE=regular-file"
        ))]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_observation_fails_the_operation() {
    let fixture = Fixture::new();
    let mut gate = fixture
        .gate()
        .host_latch(HostLatch::defer().fail_observe())
        .build()
        .await
        .unwrap();
    // a latch error getting the preopens can't be returned, the gate panics
    let result = gate
        .run(async |accessor, exports| Ok(preopens(accessor, exports).await?.len()))
        .await;
    assert!(result.is_err(), "{result:?}");
}
