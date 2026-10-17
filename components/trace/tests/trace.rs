//! The trace component logs every call at the trace level, in the `filesystem` context, and
//! passes it through.
//!
//! Each call is logged with `FD=`, the path of the descriptor it was made on, the directory or file
//! a `PATH` argument is resolved against. A descriptor is labeled with its physical path: `.` and
//! `..` are applied, and symlinks are followed as the host follows them. `PATH` arguments are
//! logged as given.
//!
//! Each test preopens a fresh copy of this tree as `/`:
//!
//! ```text
//! file.txt                "hello"
//! dir/sub/nested.txt      "nested"
//! dir/up -> ../other
//! dir/self -> .
//! dir/sub-link -> sub/
//! other/
//! link-dir -> dir
//! link-chain -> link-dir
//! link-file -> file.txt
//! link-absolute -> /etc
//! ```

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;

use tempfile::TempDir;
use test_harness::bindings::exports::wasi::filesystem::types::{
    Advice, GuestDescriptor, NewTimestamp,
};
use test_harness::{
    Ctx, DescriptorFlags, ErrorCode, Exports, Harness, Level, LogEntry, OpenFlags, PathFlags,
    Perms, TestSubject,
};
use wasmtime::component::{Accessor, ResourceAny};

const TYPES: &str = "CALL wasi:filesystem/types#descriptor";

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("file.txt"), "hello").unwrap();
        fs::create_dir_all(root.join("dir/sub")).unwrap();
        fs::write(root.join("dir/sub/nested.txt"), "nested").unwrap();
        fs::create_dir(root.join("other")).unwrap();
        symlink("../other", root.join("dir/up")).unwrap();
        symlink(".", root.join("dir/self")).unwrap();
        symlink("sub/", root.join("dir/sub-link")).unwrap();
        symlink("dir", root.join("link-dir")).unwrap();
        symlink("link-dir", root.join("link-chain")).unwrap();
        symlink("file.txt", root.join("link-file")).unwrap();
        symlink("/etc", root.join("link-absolute")).unwrap();
        Self { dir }
    }

    async fn trace(&self) -> TestSubject {
        self.trace_as("/").await
    }

    async fn trace_as(&self, preopen: &str) -> TestSubject {
        Harness::new("trace")
            .preopen(self.dir.path(), preopen, Perms::ReadWrite)
            .build()
            .await
            .expect("trace")
    }
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
    path_flags: PathFlags,
    path: &str,
) -> wasmtime::Result<Result<ResourceAny, ErrorCode>> {
    descriptor(exports)
        .call_open_at(
            accessor,
            fd,
            path_flags,
            path.into(),
            OpenFlags::empty(),
            DescriptorFlags::READ,
        )
        .await
}

/// The messages logged, each must be a trace in the `filesystem` context.
fn messages(subject: &TestSubject) -> Vec<String> {
    subject
        .recorder()
        .logs()
        .into_iter()
        .map(
            |LogEntry {
                 level,
                 context,
                 message,
             }| {
                assert_eq!(level, Level::Trace, "{message}");
                assert_eq!(context, "filesystem", "{message}");
                message
            },
        )
        .collect()
}

/// The messages logged after the first `skip`.
fn messages_after(subject: &TestSubject, skip: usize) -> Vec<String> {
    messages(subject).into_iter().skip(skip).collect()
}

/// Open each path from the root, or from the descriptor opened for the `from` path, and return the
/// label each new descriptor is logged with.
async fn labels(
    fixture: &Fixture,
    preopen: &str,
    opens: &[(Option<&str>, PathFlags, &str)],
) -> Vec<String> {
    let opens: Vec<(Option<String>, PathFlags, String)> = opens
        .iter()
        .map(|(from, flags, path)| (from.map(String::from), *flags, path.to_string()))
        .collect();
    let mut trace = fixture.trace_as(preopen).await;
    let fds = trace
        .run(async move |accessor, exports| {
            let root = root(accessor, exports).await?;
            let mut fds = vec![];
            for (from, path_flags, path) in opens {
                let from = match from {
                    Some(from) => open(accessor, exports, root, PathFlags::SYMLINK_FOLLOW, &from)
                        .await?
                        .unwrap_or_else(|err| panic!("open {from}: {err:?}")),
                    None => root,
                };
                let fd = open(accessor, exports, from, path_flags, &path)
                    .await?
                    .unwrap_or_else(|err| panic!("open {path}: {err:?}"));
                fds.push(fd);
            }
            Ok(fds)
        })
        .await
        .expect("run");
    let mut labels = vec![];
    for fd in fds {
        let skip = messages(&trace).len();
        trace
            .run(async move |accessor, exports| {
                descriptor(exports)
                    .call_get_type(accessor, fd)
                    .await?
                    .expect("get-type");
                Ok(())
            })
            .await
            .expect("run");
        let logged = messages_after(&trace, skip);
        assert_eq!(logged.len(), 1, "{logged:?}");
        let label = logged[0]
            .strip_prefix(&format!("{TYPES}.get-type FD="))
            .unwrap_or_else(|| panic!("unexpected message {}", logged[0]));
        labels.push(label.to_string());
    }
    labels
}

const FOLLOW: PathFlags = PathFlags::SYMLINK_FOLLOW;

// --- descriptor labels ---

#[tokio::test(flavor = "multi_thread")]
async fn preopen_is_labeled_with_its_name() {
    let fixture = Fixture::new();
    for preopen in ["/", ".", "/data"] {
        let mut trace = fixture.trace_as(preopen).await;
        trace
            .run(async |accessor, exports| {
                let root = root(accessor, exports).await?;
                descriptor(exports)
                    .call_get_type(accessor, root)
                    .await?
                    .expect("get-type");
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(
            messages(&trace),
            [
                "CALL wasi:filesystem/preopens#get-directories".to_string(),
                format!("{TYPES}.get-type FD={preopen}"),
            ]
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn relative_paths_are_joined_to_the_descriptor() {
    let fixture = Fixture::new();
    let labels = labels(
        &fixture,
        "/",
        &[
            (None, PathFlags::empty(), "dir"),
            (None, PathFlags::empty(), "dir/sub"),
            (None, PathFlags::empty(), "dir/sub/nested.txt"),
            (None, PathFlags::empty(), "file.txt"),
            (Some("dir"), PathFlags::empty(), "sub"),
            (Some("dir/sub"), PathFlags::empty(), "nested.txt"),
        ],
    )
    .await;
    assert_eq!(
        labels,
        [
            "/dir",
            "/dir/sub",
            "/dir/sub/nested.txt",
            "/file.txt",
            "/dir/sub",
            "/dir/sub/nested.txt"
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn dots_and_separators_are_resolved() {
    let fixture = Fixture::new();
    let labels = labels(
        &fixture,
        "/",
        &[
            (None, PathFlags::empty(), "."),
            (None, PathFlags::empty(), "./dir/./sub/."),
            (None, PathFlags::empty(), "dir//sub/"),
            (None, PathFlags::empty(), "dir/sub/.."),
            (None, PathFlags::empty(), "dir/../other"),
            (None, PathFlags::empty(), "dir/sub/../.."),
            (Some("dir"), PathFlags::empty(), "."),
            (Some("dir"), PathFlags::empty(), "sub/.."),
            (Some("dir/sub"), PathFlags::empty(), "./nested.txt"),
        ],
    )
    .await;
    assert_eq!(
        labels,
        [
            "/",
            "/dir/sub",
            "/dir/sub",
            "/dir",
            "/other",
            "/",
            "/dir",
            "/dir",
            "/dir/sub/nested.txt"
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn other_preopens_label_their_descriptors() {
    let fixture = Fixture::new();
    let dot = labels(
        &fixture,
        ".",
        &[
            (None, PathFlags::empty(), "dir/sub"),
            (None, FOLLOW, "dir/up"),
        ],
    )
    .await;
    assert_eq!(dot, ["./dir/sub", "./other"]);
    let data = labels(
        &fixture,
        "/data",
        &[
            (None, PathFlags::empty(), "dir/sub"),
            (None, FOLLOW, "link-dir"),
        ],
    )
    .await;
    assert_eq!(data, ["/data/dir/sub", "/data/dir"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn symlinks_are_followed() {
    let fixture = Fixture::new();
    let labels = labels(
        &fixture,
        "/",
        &[
            (None, FOLLOW, "link-dir"),
            (None, FOLLOW, "link-file"),
            (None, FOLLOW, "link-chain"),
            (None, FOLLOW, "link-chain/sub/nested.txt"),
            (None, FOLLOW, "dir/up"),
            (None, FOLLOW, "dir/sub-link"),
            (None, FOLLOW, "dir/self/self/sub"),
        ],
    )
    .await;
    assert_eq!(
        labels,
        [
            "/dir",
            "/file.txt",
            "/dir",
            "/dir/sub/nested.txt",
            "/other",
            "/dir/sub",
            "/dir/sub"
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn parent_of_a_symlink_is_the_parent_of_its_target() {
    let fixture = Fixture::new();
    let labels = labels(
        &fixture,
        "/",
        &[
            // the parent of /dir/sub, not of /dir where the link is
            (None, FOLLOW, "dir/sub-link/.."),
            (None, FOLLOW, "dir/up/.."),
            (None, FOLLOW, "link-dir/sub/../.."),
        ],
    )
    .await;
    assert_eq!(labels, ["/dir", "/", "/"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn intermediate_symlinks_are_followed_without_the_flag() {
    let fixture = Fixture::new();
    let labels = labels(
        &fixture,
        "/",
        &[
            (None, PathFlags::empty(), "link-dir/sub"),
            (None, PathFlags::empty(), "link-chain/sub/nested.txt"),
            (None, PathFlags::empty(), "dir/up/."),
        ],
    )
    .await;
    assert_eq!(labels, ["/dir/sub", "/dir/sub/nested.txt", "/other"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn symlinks_resolve_from_the_descriptor() {
    let fixture = Fixture::new();
    let labels = labels(
        &fixture,
        "/",
        &[
            (Some("dir"), FOLLOW, "sub-link"),
            (Some("dir"), FOLLOW, "self"),
            (Some("dir"), FOLLOW, "sub-link/nested.txt"),
            // the descriptor itself was opened through a symlink
            (Some("link-dir"), FOLLOW, "sub"),
            (Some("link-chain"), FOLLOW, "sub-link"),
        ],
    )
    .await;
    assert_eq!(
        labels,
        [
            "/dir/sub",
            "/dir",
            "/dir/sub/nested.txt",
            "/dir/sub",
            "/dir/sub"
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_opens_create_no_descriptor() {
    let fixture = Fixture::new();
    let mut trace = fixture.trace().await;
    let results = trace
        .run(async |accessor, exports| {
            let root = root(accessor, exports).await?;
            let dir = open(accessor, exports, root, PathFlags::empty(), "dir")
                .await?
                .expect("open dir");
            let mut results = vec![];
            for (fd, path_flags, path) in [
                (root, FOLLOW, "missing"),
                (root, FOLLOW, "/dir"),
                (root, FOLLOW, "../dir"),
                (dir, FOLLOW, "../file.txt"),
                (root, FOLLOW, "link-absolute"),
                (root, PathFlags::empty(), "link-dir"),
            ] {
                results.push(
                    open(accessor, exports, fd, path_flags, path)
                        .await?
                        .map(|_| ()),
                );
            }
            Ok(results)
        })
        .await
        .unwrap();
    for result in &results {
        assert!(result.is_err(), "{results:?}");
    }
    // the calls are logged as made, with the path as given
    assert_eq!(
        messages_after(&trace, 2),
        [
            format!("{TYPES}.open-at FD=/ PATH-FLAGS=(symlink-follow) PATH=missing OPEN-FLAGS=() FLAGS=(read)"),
            format!("{TYPES}.open-at FD=/ PATH-FLAGS=(symlink-follow) PATH=/dir OPEN-FLAGS=() FLAGS=(read)"),
            format!("{TYPES}.open-at FD=/ PATH-FLAGS=(symlink-follow) PATH=../dir OPEN-FLAGS=() FLAGS=(read)"),
            format!("{TYPES}.open-at FD=/dir PATH-FLAGS=(symlink-follow) PATH=../file.txt OPEN-FLAGS=() FLAGS=(read)"),
            format!("{TYPES}.open-at FD=/ PATH-FLAGS=(symlink-follow) PATH=link-absolute OPEN-FLAGS=() FLAGS=(read)"),
            format!("{TYPES}.open-at FD=/ PATH-FLAGS=() PATH=link-dir OPEN-FLAGS=() FLAGS=(read)"),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn resolving_a_label_is_not_traced() {
    let fixture = Fixture::new();
    let mut trace = fixture.trace().await;
    trace
        .run(async |accessor, exports| {
            let root = root(accessor, exports).await?;
            open(accessor, exports, root, FOLLOW, "link-chain/sub")
                .await?
                .expect("open");
            Ok(())
        })
        .await
        .unwrap();
    // the readlink calls that resolved the label are made on the host directly
    assert_eq!(
        messages(&trace),
        [
            "CALL wasi:filesystem/preopens#get-directories".to_string(),
            format!("{TYPES}.open-at FD=/ PATH-FLAGS=(symlink-follow) PATH=link-chain/sub OPEN-FLAGS=() FLAGS=(read)"),
        ]
    );
}

// --- calls are logged and passed through ---

/// Run the calls on descriptors for `/`, `/dir` opened through `link-dir`, and `/file.txt`, return
/// the messages they logged, without the three logged opening the descriptors.
async fn logged<R: Send + 'static>(
    fixture: &Fixture,
    f: impl AsyncFnOnce(&Accessor<Ctx>, &Exports, [ResourceAny; 3]) -> wasmtime::Result<R> + Send,
) -> (R, Vec<String>) {
    let mut trace = fixture.trace().await;
    let result = trace
        .run(async |accessor, exports| {
            let root = root(accessor, exports).await?;
            let dir = open(accessor, exports, root, FOLLOW, "link-dir")
                .await?
                .expect("open dir");
            let file = descriptor(exports)
                .call_open_at(
                    accessor,
                    root,
                    PathFlags::empty(),
                    "file.txt".into(),
                    OpenFlags::empty(),
                    DescriptorFlags::READ | DescriptorFlags::WRITE,
                )
                .await?
                .expect("open file.txt");
            f(accessor, exports, [root, dir, file]).await
        })
        .await
        .expect("run");
    (result, messages_after(&trace, 3))
}

#[tokio::test(flavor = "multi_thread")]
async fn descriptor_calls_are_logged() {
    let fixture = Fixture::new();
    let (results, logs) = logged(&fixture, async |accessor, exports, [root, dir, file]| {
        let d = descriptor(exports);
        Ok((
            d.call_get_flags(accessor, root).await?,
            d.call_get_type(accessor, dir).await?.is_ok(),
            d.call_stat(accessor, file).await?.map(|stat| stat.size),
            d.call_metadata_hash(accessor, dir).await?.is_ok(),
            d.call_sync(accessor, file).await?,
            d.call_sync_data(accessor, file).await?,
            d.call_set_size(accessor, file, 3).await?,
        ))
    })
    .await;
    assert_eq!(
        results,
        (
            Ok(DescriptorFlags::READ | DescriptorFlags::MUTATE_DIRECTORY),
            true,
            Ok(5),
            true,
            Ok(()),
            Ok(()),
            Ok(()),
        )
    );
    assert_eq!(
        fs::read_to_string(fixture.dir.path().join("file.txt")).unwrap(),
        "hel"
    );
    assert_eq!(
        logs,
        [
            format!("{TYPES}.get-flags FD=/"),
            format!("{TYPES}.get-type FD=/dir"),
            format!("{TYPES}.stat FD=/file.txt"),
            format!("{TYPES}.metadata-hash FD=/dir"),
            format!("{TYPES}.sync FD=/file.txt"),
            format!("{TYPES}.sync-data FD=/file.txt"),
            format!("{TYPES}.set-size FD=/file.txt SIZE=3"),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn path_calls_are_logged_with_the_path_as_given() {
    let fixture = Fixture::new();
    let (results, logs) = logged(&fixture, async |accessor, exports, [root, dir, _]| {
        let d = descriptor(exports);
        Ok((
            d.call_stat_at(
                accessor,
                dir,
                PathFlags::empty(),
                "sub/../sub/nested.txt".into(),
            )
            .await?
            .map(|stat| stat.size),
            d.call_stat_at(accessor, root, FOLLOW, "link-chain/sub/nested.txt".into())
                .await?
                .map(|stat| stat.size),
            d.call_metadata_hash_at(accessor, dir, FOLLOW, "./self".into())
                .await?
                .is_ok(),
            d.call_readlink_at(accessor, dir, "up".into()).await?,
            d.call_readlink_at(accessor, root, "dir/sub-link".into())
                .await?,
        ))
    })
    .await;
    assert_eq!(
        results,
        (Ok(6), Ok(6), true, Ok("../other".into()), Ok("sub/".into()))
    );
    assert_eq!(
        logs,
        [
            format!("{TYPES}.stat-at FD=/dir PATH-FLAGS=() PATH=sub/../sub/nested.txt"),
            format!(
                "{TYPES}.stat-at FD=/ PATH-FLAGS=(symlink-follow) PATH=link-chain/sub/nested.txt"
            ),
            format!("{TYPES}.metadata-hash-at FD=/dir PATH-FLAGS=(symlink-follow) PATH=./self"),
            format!("{TYPES}.readlink-at FD=/dir PATH=up"),
            format!("{TYPES}.readlink-at FD=/ PATH=dir/sub-link"),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mutations_are_logged_and_passed_through() {
    let fixture = Fixture::new();
    let (results, logs) = logged(&fixture, async |accessor, exports, [root, dir, _]| {
        let d = descriptor(exports);
        Ok(vec![
            d.call_create_directory_at(accessor, dir, "new".into())
                .await?,
            d.call_remove_directory_at(accessor, root, "dir/new".into())
                .await?,
            d.call_symlink_at(accessor, dir, "sub".into(), "new-link".into())
                .await?,
            d.call_link_at(
                accessor,
                root,
                PathFlags::empty(),
                "file.txt".into(),
                dir,
                "hard.txt".into(),
            )
            .await?,
            d.call_rename_at(accessor, dir, "hard.txt".into(), root, "renamed.txt".into())
                .await?,
            d.call_unlink_file_at(accessor, root, "renamed.txt".into())
                .await?,
        ])
    })
    .await;
    assert_eq!(results, [Ok(()), Ok(()), Ok(()), Ok(()), Ok(()), Ok(())]);
    let root = fixture.dir.path();
    assert!(!root.join("dir/new").exists());
    assert_eq!(
        fs::read_link(root.join("dir/new-link")).unwrap().to_str(),
        Some("sub")
    );
    assert!(!root.join("dir/hard.txt").exists());
    assert!(!root.join("renamed.txt").exists());
    assert_eq!(
        logs,
        [
            format!("{TYPES}.create-directory-at FD=/dir PATH=new"),
            format!("{TYPES}.remove-directory-at FD=/ PATH=dir/new"),
            format!("{TYPES}.symlink-at FD=/dir OLD-PATH=sub NEW-PATH=new-link"),
            format!("{TYPES}.link-at FD=/ OLD-PATH-FLAGS=() OLD-PATH=file.txt NEW-DESCRIPTOR=/dir NEW-PATH=hard.txt"),
            format!("{TYPES}.rename-at FD=/dir OLD-PATH=hard.txt NEW-DESCRIPTOR=/ NEW-PATH=renamed.txt"),
            format!("{TYPES}.unlink-file-at FD=/ PATH=renamed.txt"),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn open_flags_are_logged() {
    let fixture = Fixture::new();
    let (results, logs) = logged(&fixture, async |accessor, exports, [_, dir, _]| {
        let d = descriptor(exports);
        Ok((
            d.call_open_at(
                accessor,
                dir,
                FOLLOW,
                "created.txt".into(),
                OpenFlags::CREATE | OpenFlags::EXCLUSIVE,
                DescriptorFlags::READ | DescriptorFlags::WRITE,
            )
            .await?
            .is_ok(),
            d.call_open_at(
                accessor,
                dir,
                PathFlags::empty(),
                "sub".into(),
                OpenFlags::DIRECTORY,
                DescriptorFlags::READ | DescriptorFlags::MUTATE_DIRECTORY,
            )
            .await?
            .is_ok(),
        ))
    })
    .await;
    assert_eq!(results, (true, true));
    assert!(fixture.dir.path().join("dir/created.txt").is_file());
    assert_eq!(
        logs,
        [
            format!("{TYPES}.open-at FD=/dir PATH-FLAGS=(symlink-follow) PATH=created.txt OPEN-FLAGS=(create|exclusive) FLAGS=(read|write)"),
            format!("{TYPES}.open-at FD=/dir PATH-FLAGS=() PATH=sub OPEN-FLAGS=(directory) FLAGS=(read|mutate-directory)"),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn comparisons_are_logged() {
    let fixture = Fixture::new();
    let (results, logs) = logged(&fixture, async |accessor, exports, [root, dir, _]| {
        let again = open(accessor, exports, root, PathFlags::empty(), "dir")
            .await?
            .expect("open dir");
        let d = descriptor(exports);
        Ok((
            d.call_is_same_object(accessor, dir, again).await?,
            d.call_is_same_object(accessor, dir, root).await?,
        ))
    })
    .await;
    assert_eq!(results, (true, false));
    assert_eq!(
        logs,
        [
            format!("{TYPES}.open-at FD=/ PATH-FLAGS=() PATH=dir OPEN-FLAGS=() FLAGS=(read)"),
            format!("{TYPES}.is-same-object FD=/dir OTHER=/dir"),
            format!("{TYPES}.is-same-object FD=/dir OTHER=/"),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn advice_is_logged() {
    let fixture = Fixture::new();
    let (result, logs) = logged(&fixture, async |accessor, exports, [_, _, file]| {
        Ok(descriptor(exports)
            .call_advise(accessor, file, 1, 2, Advice::Sequential)
            .await?)
    })
    .await;
    assert_eq!(result, Ok(()));
    assert_eq!(
        logs,
        [format!(
            "{TYPES}.advise FD=/file.txt OFFSET=1 LENGTH=2 ADVICE=sequential"
        )]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn times_are_logged() {
    use test_harness::bindings::wasi::clocks::system_clock::Instant;
    let fixture = Fixture::new();
    let (results, logs) = logged(&fixture, async |accessor, exports, [root, _, file]| {
        let d = descriptor(exports);
        let timestamp = NewTimestamp::Timestamp(Instant {
            seconds: 1_700_000_000,
            nanoseconds: 123_000_000,
        });
        Ok((
            d.call_set_times(accessor, file, NewTimestamp::NoChange, NewTimestamp::Now)
                .await?,
            d.call_set_times_at(
                accessor,
                root,
                FOLLOW,
                "link-file".into(),
                timestamp,
                NewTimestamp::NoChange,
            )
            .await?,
        ))
    })
    .await;
    assert_eq!(results, (Ok(()), Ok(())));
    assert_eq!(
        logs,
        [
            format!("{TYPES}.set-times FD=/file.txt ACCESS-TIMESTAMP=no-change MODIFICATION-TIMESTAMP=now"),
            format!("{TYPES}.set-times-at FD=/ PATH-FLAGS=(symlink-follow) PATH=link-file ACCESS-TIMESTAMP=timestamp<2023-11-14 22:13:20.123Z> MODIFICATION-TIMESTAMP=no-change"),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn streams_are_logged() {
    let fixture = Fixture::new();
    let (data, logs) = logged(&fixture, async |accessor, exports, [_, dir, file]| {
        let d = descriptor(exports);
        let (data, _) = d.call_read_via_stream(accessor, file, 1).await?;
        let mut data = test_harness::collect(accessor, data)?;
        let mut bytes = vec![];
        while let Some(byte) = data.recv().await {
            bytes.push(byte);
        }
        // the entries are passed through from the host, the harness can't read them, see
        // `test_harness::resolve`
        d.call_read_directory(accessor, dir).await?;
        Ok(String::from_utf8(bytes)?)
    })
    .await;
    assert_eq!(data, "ello");
    assert_eq!(
        logs,
        [
            format!("{TYPES}.read-via-stream FD=/file.txt OFFSET=1"),
            format!("{TYPES}.read-directory FD=/dir"),
        ]
    );
}
