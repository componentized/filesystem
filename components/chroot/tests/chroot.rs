//! The chroot confines every operation to the configured directory.
//!
//! Each test preopens a fresh copy of this tree as `/`, with the chroot at `/jail`:
//!
//! ```text
//! secret.txt                     "outside secret", must never be reachable
//! jail-sibling/secret.txt        shares the jail's name as a prefix
//! jail/
//!   secret.txt                   "jail", the file `/secret.txt` in the chroot
//!   inside.txt                   "inside"
//!   sub/nested.txt               "nested!"
//!   sub/deeper/
//!   sub/link-up -> ../inside.txt
//!   sub/link-escape -> ../../secret.txt
//!   link-inside -> inside.txt
//!   link-outside -> ../secret.txt
//!   link-absolute -> /secret.txt
//!   link-host-absolute -> <host path>/secret.txt
//!   link-dir-outside -> ..
//! ```
//!
//! Files have distinct sizes, a stat tells which file was reached.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use test_harness::{
    DescriptorFlags, DescriptorType, ErrorCode, Exports, Harness, OpenFlags, PathFlags, Perms,
    TestSubject,
};
use wasmtime::component::{Accessor, ResourceAny};

const OUTSIDE_SECRET: &str = "outside secret";
const JAIL_SECRET: &str = "jail";
const INSIDE: &str = "inside";
const NESTED: &str = "nested!";

fn size(content: &str) -> u64 {
    content.len() as u64
}

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("secret.txt"), OUTSIDE_SECRET).unwrap();
        fs::create_dir_all(root.join("jail-sibling")).unwrap();
        fs::write(root.join("jail-sibling/secret.txt"), OUTSIDE_SECRET).unwrap();

        let jail = root.join("jail");
        fs::create_dir_all(jail.join("sub/deeper")).unwrap();
        fs::write(jail.join("secret.txt"), JAIL_SECRET).unwrap();
        fs::write(jail.join("inside.txt"), INSIDE).unwrap();
        fs::write(jail.join("sub/nested.txt"), NESTED).unwrap();
        symlink("../inside.txt", jail.join("sub/link-up")).unwrap();
        symlink("../../secret.txt", jail.join("sub/link-escape")).unwrap();
        symlink("inside.txt", jail.join("link-inside")).unwrap();
        symlink("../secret.txt", jail.join("link-outside")).unwrap();
        symlink("/secret.txt", jail.join("link-absolute")).unwrap();
        symlink(root.join("secret.txt"), jail.join("link-host-absolute")).unwrap();
        symlink("..", jail.join("link-dir-outside")).unwrap();
        Self { dir }
    }

    fn host(&self, path: &str) -> PathBuf {
        self.dir.path().join(path)
    }

    fn jail(&self, path: &str) -> PathBuf {
        self.dir.path().join("jail").join(path)
    }

    /// The files outside the jail are untouched.
    fn assert_outside_untouched(&self) {
        assert_eq!(
            fs::read_to_string(self.host("secret.txt")).unwrap(),
            OUTSIDE_SECRET
        );
        assert_eq!(
            fs::read_to_string(self.host("jail-sibling/secret.txt")).unwrap(),
            OUTSIDE_SECRET
        );
        let mut entries: Vec<String> = fs::read_dir(self.dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        entries.sort();
        assert_eq!(entries, ["jail", "jail-sibling", "secret.txt"]);
    }

    fn harness(&self) -> Harness {
        Harness::new("chroot").preopen(self.dir.path(), "/", Perms::ReadWrite)
    }

    async fn chroot(&self) -> TestSubject {
        self.harness()
            .config("path", "/jail")
            .build()
            .await
            .expect("chroot")
    }
}

/// The single preopen of the chroot, its root.
async fn root(
    accessor: &Accessor<test_harness::Ctx>,
    exports: &Exports,
) -> wasmtime::Result<ResourceAny> {
    let dirs = exports
        .wasi_filesystem_preopens()
        .call_get_directories(accessor)
        .await?;
    assert_eq!(dirs.len(), 1);
    let (fd, name) = dirs.into_iter().next().unwrap();
    assert_eq!(name, ".");
    Ok(fd)
}

/// Stat a path from the root, following symlinks.
async fn stat_size(
    accessor: &Accessor<test_harness::Ctx>,
    exports: &Exports,
    fd: ResourceAny,
    path: &str,
) -> wasmtime::Result<Result<u64, ErrorCode>> {
    Ok(exports
        .wasi_filesystem_types()
        .descriptor()
        .call_stat_at(accessor, fd, PathFlags::SYMLINK_FOLLOW, path.into())
        .await?
        .map(|stat| stat.size))
}

/// Stat each path from the root, returning the size of the file reached or the error.
async fn stat_all(fixture: &Fixture, paths: &[&str]) -> Vec<(String, Result<u64, ErrorCode>)> {
    let paths: Vec<String> = paths.iter().map(|path| path.to_string()).collect();
    let mut chroot = fixture.chroot().await;
    chroot
        .run(async move |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let mut results = vec![];
            for path in paths {
                let result = stat_size(accessor, exports, fd, &path).await?;
                results.push((path, result));
            }
            Ok(results)
        })
        .await
        .expect("run")
}

fn expect_all(results: Vec<(String, Result<u64, ErrorCode>)>, expected: Result<u64, ErrorCode>) {
    for (path, result) in results {
        assert_eq!(result, expected, "{path:?}");
    }
}

/// Open a path from the root.
async fn open(
    accessor: &Accessor<test_harness::Ctx>,
    exports: &Exports,
    fd: ResourceAny,
    path: &str,
    open_flags: OpenFlags,
    flags: DescriptorFlags,
) -> wasmtime::Result<Result<ResourceAny, ErrorCode>> {
    exports
        .wasi_filesystem_types()
        .descriptor()
        .call_open_at(
            accessor,
            fd,
            PathFlags::SYMLINK_FOLLOW,
            path.into(),
            open_flags,
            flags,
        )
        .await
}

// --- lookups ---

#[tokio::test(flavor = "multi_thread")]
async fn files_in_the_jail_are_reachable() {
    let fixture = Fixture::new();
    let results = stat_all(
        &fixture,
        &[
            "inside.txt",
            "/inside.txt",
            "./inside.txt",
            "sub/nested.txt",
            "/sub/nested.txt",
        ],
    )
    .await;
    assert_eq!(
        results,
        [
            ("inside.txt".into(), Ok(size(INSIDE))),
            ("/inside.txt".into(), Ok(size(INSIDE))),
            ("./inside.txt".into(), Ok(size(INSIDE))),
            ("sub/nested.txt".into(), Ok(size(NESTED))),
            ("/sub/nested.txt".into(), Ok(size(NESTED))),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn redundant_separators_and_dots() {
    let fixture = Fixture::new();
    let results = stat_all(
        &fixture,
        &[
            "sub//nested.txt",
            "//sub///nested.txt",
            "./sub/./nested.txt",
            "sub/deeper/../nested.txt",
            "sub/deeper/../../sub/nested.txt",
        ],
    )
    .await;
    expect_all(results, Ok(size(NESTED)));
}

#[tokio::test(flavor = "multi_thread")]
async fn parent_of_root_is_root() {
    let fixture = Fixture::new();
    // each names `/secret.txt` in the chroot, the jail's secret, never the host's
    let results = stat_all(
        &fixture,
        &[
            "secret.txt",
            "/secret.txt",
            "../secret.txt",
            "/../secret.txt",
            "../../../../../../secret.txt",
            "/../../../../../../secret.txt",
            "sub/../../secret.txt",
            "sub/deeper/../../../../secret.txt",
            "./../secret.txt",
            "..//..//secret.txt",
        ],
    )
    .await;
    expect_all(results, Ok(size(JAIL_SECRET)));
    fixture.assert_outside_untouched();
}

#[tokio::test(flavor = "multi_thread")]
async fn parent_of_root_is_root_for_directories() {
    let fixture = Fixture::new();
    let results = stat_all(
        &fixture,
        &["..", "/..", "../..", "sub/../..", "../jail/secret.txt"],
    )
    .await;
    // `..` is the jail, there is no `jail` directory in the jail
    assert_eq!(results[3].1, results[0].1);
    assert_eq!(
        results[4],
        ("../jail/secret.txt".into(), Err(ErrorCode::NoEntry))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sibling_with_a_shared_prefix_is_unreachable() {
    let fixture = Fixture::new();
    let results = stat_all(
        &fixture,
        &[
            "../jail-sibling/secret.txt",
            "/../jail-sibling/secret.txt",
            "../jail-sibling",
        ],
    )
    .await;
    expect_all(results, Err(ErrorCode::NoEntry));
}

#[tokio::test(flavor = "multi_thread")]
async fn host_paths_are_not_reachable() {
    let fixture = Fixture::new();
    let host_secret = fixture.host("secret.txt").to_string_lossy().into_owned();
    let host_jail = fixture.jail("inside.txt").to_string_lossy().into_owned();
    let results = stat_all(
        &fixture,
        &[&host_secret, &host_jail, "/etc/passwd", "/jail/inside.txt"],
    )
    .await;
    // absolute paths resolve within the jail, where none of these exist
    expect_all(results, Err(ErrorCode::NoEntry));
}

#[tokio::test(flavor = "multi_thread")]
async fn odd_paths_are_not_traversals() {
    let fixture = Fixture::new();
    let results = stat_all(
        &fixture,
        &["..\\secret.txt", "...", ".../secret.txt", ".. /secret.txt"],
    )
    .await;
    expect_all(results, Err(ErrorCode::NoEntry));
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_path_is_an_error() {
    let fixture = Fixture::new();
    let results = stat_all(&fixture, &[""]).await;
    expect_all(results, Err(ErrorCode::NoEntry));
}

#[tokio::test(flavor = "multi_thread")]
async fn nul_byte_is_an_error() {
    let fixture = Fixture::new();
    // C would see `inside.txt`, resolving the rest would reach `secret.txt`
    let results = stat_all(
        &fixture,
        &["inside.txt\0/../secret.txt", "inside.txt\0", "\0"],
    )
    .await;
    expect_all(results, Err(ErrorCode::IllegalByteSequence));
}

#[tokio::test(flavor = "multi_thread")]
async fn trailing_slash_requires_a_directory() {
    let fixture = Fixture::new();
    let results = stat_all(
        &fixture,
        &[
            "inside.txt/",
            "inside.txt/.",
            "sub/",
            "sub/.",
            "sub/deeper/..",
        ],
    )
    .await;
    assert!(results[0].1.is_err(), "{:?}", results[0]);
    assert!(results[1].1.is_err(), "{:?}", results[1]);
    assert!(results[2].1.is_ok(), "{:?}", results[2]);
    assert!(results[3].1.is_ok(), "{:?}", results[3]);
    assert!(results[4].1.is_ok(), "{:?}", results[4]);
}

// --- symlinks ---

#[tokio::test(flavor = "multi_thread")]
async fn symlinks_within_the_jail_are_followed() {
    let fixture = Fixture::new();
    let results = stat_all(&fixture, &["link-inside", "sub/link-up", "/sub/link-up"]).await;
    expect_all(results, Ok(size(INSIDE)));
}

#[tokio::test(flavor = "multi_thread")]
async fn symlinks_out_of_the_jail_are_not_followed() {
    let fixture = Fixture::new();
    let results = stat_all(
        &fixture,
        &[
            "link-outside",
            "link-absolute",
            "link-host-absolute",
            "sub/link-escape",
            "link-dir-outside",
            "link-dir-outside/secret.txt",
            "link-dir-outside/jail/inside.txt",
            "sub/../link-outside",
        ],
    )
    .await;
    for (path, result) in results {
        assert!(
            result != Ok(size(OUTSIDE_SECRET)),
            "{path:?} reached the secret outside the jail"
        );
        assert!(result.is_err(), "{path:?} -> {result:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn symlinks_are_not_followed_without_the_flag() {
    let fixture = Fixture::new();
    let mut chroot = fixture.chroot().await;
    let types = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let mut types = vec![];
            for path in ["link-outside", "link-absolute", "link-dir-outside"] {
                let stat = exports
                    .wasi_filesystem_types()
                    .descriptor()
                    .call_stat_at(accessor, fd, PathFlags::empty(), path.into())
                    .await?
                    .expect("lstat");
                types.push(stat.type_);
            }
            Ok(types)
        })
        .await
        .unwrap();
    for type_ in types {
        assert_eq!(type_, DescriptorType::SymbolicLink);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn readlink_returns_the_target_as_stored() {
    let fixture = Fixture::new();
    let mut chroot = fixture.chroot().await;
    let targets = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let mut targets = vec![];
            for path in ["link-inside", "/sub/link-up", "../link-outside"] {
                targets.push(
                    exports
                        .wasi_filesystem_types()
                        .descriptor()
                        .call_readlink_at(accessor, fd, path.into())
                        .await?,
                );
            }
            Ok(targets)
        })
        .await
        .unwrap();
    assert_eq!(
        targets,
        [
            Ok("inside.txt".into()),
            Ok("../inside.txt".into()),
            Ok("../secret.txt".into())
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn created_symlinks_store_the_target_unchanged() {
    let fixture = Fixture::new();
    let mut chroot = fixture.chroot().await;
    let (created, followed) = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let descriptor = exports.wasi_filesystem_types().descriptor();
            let mut created = vec![];
            for (target, link) in [
                ("inside.txt", "new-inside"),
                ("../secret.txt", "new-outside"),
                ("/secret.txt", "../new-absolute"),
            ] {
                created.push(
                    descriptor
                        .call_symlink_at(accessor, fd, target.into(), link.into())
                        .await?,
                );
            }
            let mut followed = vec![];
            for link in ["new-inside", "new-outside", "new-absolute"] {
                followed.push(stat_size(accessor, exports, fd, link).await?);
            }
            Ok((created, followed))
        })
        .await
        .unwrap();
    assert_eq!(created[0], Ok(()));
    assert_eq!(created[1], Ok(()));
    // the host refuses absolute targets
    assert!(created[2].is_err(), "{:?}", created[2]);
    // the targets are not rewritten with the jail's host path
    assert_eq!(
        fs::read_link(fixture.jail("new-inside")).unwrap(),
        Path::new("inside.txt")
    );
    assert_eq!(
        fs::read_link(fixture.jail("new-outside")).unwrap(),
        Path::new("../secret.txt")
    );
    assert!(fs::symlink_metadata(fixture.jail("new-absolute")).is_err());
    assert!(fs::symlink_metadata(fixture.host("new-absolute")).is_err());
    // a planted link out of the jail is never followed out of it
    assert_eq!(followed[0], Ok(size(INSIDE)));
    assert!(followed[1].is_err(), "{:?}", followed[1]);
    assert_eq!(followed[2], Err(ErrorCode::NoEntry));
    fixture.assert_outside_untouched();
}

// --- descriptors ---

#[tokio::test(flavor = "multi_thread")]
async fn opened_directories_resolve_relative_to_themselves() {
    let fixture = Fixture::new();
    let mut chroot = fixture.chroot().await;
    let results = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let sub = open(
                accessor,
                exports,
                fd,
                "sub",
                OpenFlags::DIRECTORY,
                DescriptorFlags::READ,
            )
            .await?
            .expect("open sub");
            let deeper = open(
                accessor,
                exports,
                sub,
                "deeper",
                OpenFlags::DIRECTORY,
                DescriptorFlags::READ,
            )
            .await?
            .expect("open deeper");
            Ok(vec![
                stat_size(accessor, exports, sub, "nested.txt").await?,
                stat_size(accessor, exports, sub, "./nested.txt").await?,
                stat_size(accessor, exports, sub, "deeper/../nested.txt").await?,
                // the link names a parent of the descriptor it is followed from
                stat_size(accessor, exports, sub, "link-up").await?,
                // absolute paths resolve from the root of the chroot
                stat_size(accessor, exports, sub, "/inside.txt").await?,
                stat_size(accessor, exports, deeper, "/sub/nested.txt").await?,
                stat_size(accessor, exports, deeper, "/../../secret.txt").await?,
            ])
        })
        .await
        .unwrap();
    assert_eq!(
        results,
        [
            Ok(size(NESTED)),
            Ok(size(NESTED)),
            Ok(size(NESTED)),
            Err(ErrorCode::NotPermitted),
            Ok(size(INSIDE)),
            Ok(size(NESTED)),
            Ok(size(JAIL_SECRET)),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn parent_of_opened_directory_is_not_permitted() {
    let fixture = Fixture::new();
    let mut chroot = fixture.chroot().await;
    let results = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let sub = open(
                accessor,
                exports,
                fd,
                "sub",
                OpenFlags::DIRECTORY,
                DescriptorFlags::READ,
            )
            .await?
            .expect("open sub");
            let mut results = vec![];
            for path in [
                "..",
                "../inside.txt",
                "../../secret.txt",
                "deeper/../../inside.txt",
            ] {
                results.push(stat_size(accessor, exports, sub, path).await?);
            }
            Ok(results)
        })
        .await
        .unwrap();
    expect_all(
        results.into_iter().map(|r| (String::new(), r)).collect(),
        Err(ErrorCode::NotPermitted),
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn reopened_root_is_still_the_root() {
    let fixture = Fixture::new();
    let mut chroot = fixture.chroot().await;
    let results = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let mut results = vec![];
            for path in [".", "/", "..", "sub/.."] {
                let reopened = open(
                    accessor,
                    exports,
                    fd,
                    path,
                    OpenFlags::DIRECTORY,
                    DescriptorFlags::READ,
                )
                .await?
                .expect("reopen root");
                results.push(stat_size(accessor, exports, reopened, "../secret.txt").await?);
            }
            Ok(results)
        })
        .await
        .unwrap();
    // `sub/..` resolves lexically to the root of the chroot
    expect_all(
        results.into_iter().map(|r| (String::new(), r)).collect(),
        Ok(size(JAIL_SECRET)),
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn symlinked_directory_out_of_the_jail_cannot_be_opened() {
    let fixture = Fixture::new();
    let mut chroot = fixture.chroot().await;
    let result = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            open(
                accessor,
                exports,
                fd,
                "link-dir-outside",
                OpenFlags::DIRECTORY,
                DescriptorFlags::READ,
            )
            .await
        })
        .await
        .unwrap();
    assert!(result.is_err(), "{result:?}");
}

// --- mutations ---

#[tokio::test(flavor = "multi_thread")]
async fn files_are_created_in_the_jail() {
    let fixture = Fixture::new();
    let mut chroot = fixture.chroot().await;
    let results = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let mut results = vec![];
            for path in [
                "created.txt",
                "/created-absolute.txt",
                "../created-parent.txt",
                "sub/../../created-sub.txt",
            ] {
                results.push(
                    open(
                        accessor,
                        exports,
                        fd,
                        path,
                        OpenFlags::CREATE,
                        DescriptorFlags::READ | DescriptorFlags::WRITE,
                    )
                    .await?
                    .map(|_| ()),
                );
            }
            Ok(results)
        })
        .await
        .unwrap();
    assert_eq!(results, [Ok(()), Ok(()), Ok(()), Ok(())]);
    for name in [
        "created.txt",
        "created-absolute.txt",
        "created-parent.txt",
        "created-sub.txt",
    ] {
        assert!(fixture.jail(name).is_file(), "{name}");
    }
    fixture.assert_outside_untouched();
}

#[tokio::test(flavor = "multi_thread")]
async fn files_are_not_created_through_symlinks_out_of_the_jail() {
    let fixture = Fixture::new();
    let mut chroot = fixture.chroot().await;
    let results = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let mut results = vec![];
            for path in ["link-dir-outside/created.txt", "link-outside"] {
                results.push(
                    open(
                        accessor,
                        exports,
                        fd,
                        path,
                        OpenFlags::CREATE | OpenFlags::TRUNCATE,
                        DescriptorFlags::WRITE,
                    )
                    .await?
                    .map(|_| ()),
                );
            }
            Ok(results)
        })
        .await
        .unwrap();
    for result in results {
        assert!(result.is_err(), "{result:?}");
    }
    fixture.assert_outside_untouched();
}

#[tokio::test(flavor = "multi_thread")]
async fn directories_are_created_and_removed_in_the_jail() {
    let fixture = Fixture::new();
    let mut chroot = fixture.chroot().await;
    let results = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let descriptor = exports.wasi_filesystem_types().descriptor();
            Ok(vec![
                descriptor
                    .call_create_directory_at(accessor, fd, "../escaped".into())
                    .await?,
                descriptor
                    .call_create_directory_at(accessor, fd, "/../../escaped-too".into())
                    .await?,
                descriptor
                    .call_create_directory_at(accessor, fd, "link-dir-outside/escaped".into())
                    .await?,
                descriptor
                    .call_remove_directory_at(accessor, fd, "../escaped-too".into())
                    .await?,
            ])
        })
        .await
        .unwrap();
    assert_eq!(results[0], Ok(()));
    assert_eq!(results[1], Ok(()));
    assert!(results[2].is_err(), "{:?}", results[2]);
    assert_eq!(results[3], Ok(()));
    assert!(fixture.jail("escaped").is_dir());
    assert!(!fixture.jail("escaped-too").exists());
    fixture.assert_outside_untouched();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_jail_itself_cannot_be_removed() {
    let fixture = Fixture::new();
    let mut chroot = fixture.chroot().await;
    let results = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let descriptor = exports.wasi_filesystem_types().descriptor();
            let mut results = vec![];
            for path in ["/", ".", "..", "../jail", "/../.."] {
                results.push(
                    descriptor
                        .call_remove_directory_at(accessor, fd, path.into())
                        .await?,
                );
            }
            Ok(results)
        })
        .await
        .unwrap();
    for result in results {
        assert!(result.is_err(), "{result:?}");
    }
    assert!(fixture.jail("inside.txt").is_file());
    fixture.assert_outside_untouched();
}

#[tokio::test(flavor = "multi_thread")]
async fn unlink_stays_in_the_jail() {
    let fixture = Fixture::new();
    let mut chroot = fixture.chroot().await;
    let results = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let descriptor = exports.wasi_filesystem_types().descriptor();
            Ok(vec![
                // the jail's secret, not the host's
                descriptor
                    .call_unlink_file_at(accessor, fd, "../secret.txt".into())
                    .await?,
                // removes the link itself, not its target
                descriptor
                    .call_unlink_file_at(accessor, fd, "link-outside".into())
                    .await?,
                descriptor
                    .call_unlink_file_at(accessor, fd, "link-dir-outside/secret.txt".into())
                    .await?,
                descriptor
                    .call_unlink_file_at(accessor, fd, "../jail-sibling/secret.txt".into())
                    .await?,
            ])
        })
        .await
        .unwrap();
    assert_eq!(results[0], Ok(()));
    assert_eq!(results[1], Ok(()));
    assert!(results[2].is_err(), "{:?}", results[2]);
    assert_eq!(results[3], Err(ErrorCode::NoEntry));
    assert!(!fixture.jail("secret.txt").exists());
    assert!(fs::symlink_metadata(fixture.jail("link-outside")).is_err());
    fixture.assert_outside_untouched();
}

#[tokio::test(flavor = "multi_thread")]
async fn rename_stays_in_the_jail() {
    let fixture = Fixture::new();
    let mut chroot = fixture.chroot().await;
    let results = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let sub = open(
                accessor,
                exports,
                fd,
                "sub",
                OpenFlags::DIRECTORY,
                DescriptorFlags::READ | DescriptorFlags::MUTATE_DIRECTORY,
            )
            .await?
            .expect("open sub");
            let descriptor = exports.wasi_filesystem_types().descriptor();
            Ok(vec![
                descriptor
                    .call_rename_at(accessor, fd, "inside.txt".into(), fd, "../moved.txt".into())
                    .await?,
                // the new path is relative to the new descriptor
                descriptor
                    .call_rename_at(accessor, fd, "moved.txt".into(), sub, "moved.txt".into())
                    .await?,
                descriptor
                    .call_rename_at(
                        accessor,
                        sub,
                        "moved.txt".into(),
                        fd,
                        "/../../moved-back.txt".into(),
                    )
                    .await?,
                descriptor
                    .call_rename_at(
                        accessor,
                        fd,
                        "moved-back.txt".into(),
                        fd,
                        "link-dir-outside/stolen.txt".into(),
                    )
                    .await?,
                descriptor
                    .call_rename_at(
                        accessor,
                        fd,
                        "../secret.txt".into(),
                        fd,
                        "link-dir-outside/secret.txt".into(),
                    )
                    .await?,
                descriptor
                    .call_rename_at(accessor, sub, "../inside.txt".into(), fd, "x.txt".into())
                    .await?,
            ])
        })
        .await
        .unwrap();
    assert_eq!(results[0], Ok(()));
    assert_eq!(results[1], Ok(()));
    assert_eq!(results[2], Ok(()));
    assert!(results[3].is_err(), "{:?}", results[3]);
    assert!(results[4].is_err(), "{:?}", results[4]);
    assert_eq!(results[5], Err(ErrorCode::NotPermitted));
    assert_eq!(
        fs::read_to_string(fixture.jail("moved-back.txt")).unwrap(),
        INSIDE
    );
    assert_eq!(
        fs::read_to_string(fixture.jail("secret.txt")).unwrap(),
        JAIL_SECRET
    );
    fixture.assert_outside_untouched();
}

#[tokio::test(flavor = "multi_thread")]
async fn hard_links_stay_in_the_jail() {
    let fixture = Fixture::new();
    let mut chroot = fixture.chroot().await;
    let results = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let descriptor = exports.wasi_filesystem_types().descriptor();
            Ok(vec![
                descriptor
                    .call_link_at(
                        accessor,
                        fd,
                        PathFlags::empty(),
                        "../secret.txt".into(),
                        fd,
                        "../hard.txt".into(),
                    )
                    .await?,
                descriptor
                    .call_link_at(
                        accessor,
                        fd,
                        PathFlags::SYMLINK_FOLLOW,
                        "link-outside".into(),
                        fd,
                        "hard-outside.txt".into(),
                    )
                    .await?,
                descriptor
                    .call_link_at(
                        accessor,
                        fd,
                        PathFlags::empty(),
                        "inside.txt".into(),
                        fd,
                        "link-dir-outside/hard.txt".into(),
                    )
                    .await?,
            ])
        })
        .await
        .unwrap();
    assert_eq!(results[0], Ok(()));
    assert!(results[1].is_err(), "{:?}", results[1]);
    assert!(results[2].is_err(), "{:?}", results[2]);
    // a link to the jail's secret, not the host's
    assert_eq!(
        fs::read_to_string(fixture.jail("hard.txt")).unwrap(),
        JAIL_SECRET
    );
    assert!(!fixture.jail("hard-outside.txt").exists());
    fixture.assert_outside_untouched();
}

#[tokio::test(flavor = "multi_thread")]
async fn path_metadata_operations_stay_in_the_jail() {
    let fixture = Fixture::new();
    let mut chroot = fixture.chroot().await;
    let (set_times, hashes) = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let descriptor = exports.wasi_filesystem_types().descriptor();
            use test_harness::bindings::exports::wasi::filesystem::types::NewTimestamp;
            let set_times = vec![
                descriptor
                    .call_set_times_at(
                        accessor,
                        fd,
                        PathFlags::SYMLINK_FOLLOW,
                        "link-outside".into(),
                        NewTimestamp::Now,
                        NewTimestamp::Now,
                    )
                    .await?,
                descriptor
                    .call_set_times_at(
                        accessor,
                        fd,
                        PathFlags::SYMLINK_FOLLOW,
                        "../secret.txt".into(),
                        NewTimestamp::Now,
                        NewTimestamp::Now,
                    )
                    .await?,
            ];
            let hashes = vec![
                descriptor
                    .call_metadata_hash_at(
                        accessor,
                        fd,
                        PathFlags::SYMLINK_FOLLOW,
                        "../secret.txt".into(),
                    )
                    .await?,
                descriptor
                    .call_metadata_hash_at(
                        accessor,
                        fd,
                        PathFlags::SYMLINK_FOLLOW,
                        "secret.txt".into(),
                    )
                    .await?,
                descriptor
                    .call_metadata_hash_at(
                        accessor,
                        fd,
                        PathFlags::SYMLINK_FOLLOW,
                        "link-outside".into(),
                    )
                    .await?,
            ];
            Ok((set_times, hashes))
        })
        .await
        .unwrap();
    assert!(set_times[0].is_err(), "{:?}", set_times[0]);
    assert_eq!(set_times[1], Ok(()));
    assert!(hashes[0].is_ok());
    assert_eq!(hashes[0], hashes[1]);
    assert!(hashes[2].is_err(), "{:?}", hashes[2]);
}

// --- configuration ---

#[tokio::test(flavor = "multi_thread")]
async fn config_path_may_be_relative() {
    let fixture = Fixture::new();
    let mut chroot = fixture
        .harness()
        .config("path", "jail")
        .build()
        .await
        .unwrap();
    let result = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            stat_size(accessor, exports, fd, "/inside.txt").await
        })
        .await
        .unwrap();
    assert_eq!(result, Ok(size(INSIDE)));
}

#[tokio::test(flavor = "multi_thread")]
async fn config_path_may_be_within_a_relative_preopen() {
    let fixture = Fixture::new();
    let mut chroot = Harness::new("chroot")
        .preopen(fixture.dir.path(), ".", Perms::ReadWrite)
        .config("path", "jail")
        .build()
        .await
        .unwrap();
    let result = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            stat_size(accessor, exports, fd, "../secret.txt").await
        })
        .await
        .unwrap();
    assert_eq!(result, Ok(size(JAIL_SECRET)));
}

#[tokio::test(flavor = "multi_thread")]
async fn config_path_selects_the_longest_preopen() {
    let fixture = Fixture::new();
    let mut chroot = fixture
        .harness()
        .preopen(fixture.host("jail"), "/data", Perms::ReadWrite)
        .config("path", "/data/sub")
        .build()
        .await
        .unwrap();
    let result = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            stat_size(accessor, exports, fd, "/nested.txt").await
        })
        .await
        .unwrap();
    assert_eq!(result, Ok(size(NESTED)));
}

#[tokio::test(flavor = "multi_thread")]
async fn chroot_is_no_more_capable_than_its_preopen() {
    let fixture = Fixture::new();
    let mut chroot = Harness::new("chroot")
        .preopen(fixture.dir.path(), "/", Perms::ReadOnly)
        .config("path", "/jail")
        .build()
        .await
        .unwrap();
    let (flags, created) = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let descriptor = exports.wasi_filesystem_types().descriptor();
            let flags = descriptor.call_get_flags(accessor, fd).await?;
            let created = descriptor
                .call_create_directory_at(accessor, fd, "new".into())
                .await?;
            Ok((flags, created))
        })
        .await
        .unwrap();
    assert_eq!(flags, Ok(DescriptorFlags::READ));
    assert_eq!(created, Err(ErrorCode::NotPermitted));
    assert!(!fixture.jail("new").exists());
}

/// The chroot traps rather than expose a directory it was not configured with.
async fn assert_config_traps(harness: Harness) {
    let mut chroot = harness.build().await.unwrap();
    let result = chroot
        .run(async |accessor, exports| {
            exports
                .wasi_filesystem_preopens()
                .call_get_directories(accessor)
                .await
        })
        .await;
    assert!(
        result.is_err(),
        "expected a trap, got {} preopens",
        result.unwrap().len()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn config_path_must_not_contain_parents() {
    let fixture = Fixture::new();
    for path in ["..", "/jail/..", "/jail/../jail-sibling", "jail/../../"] {
        assert_config_traps(fixture.harness().config("path", path)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn config_path_must_be_within_a_preopen() {
    let fixture = Fixture::new();
    assert_config_traps(fixture.harness()).await;
    assert_config_traps(
        Harness::new("chroot")
            .preopen(fixture.host("jail"), "/data", Perms::ReadWrite)
            .config("path", "/jail"),
    )
    .await;
}

/// The root of a chroot whose directory can't be opened fails every operation, the directory is
/// opened by the first operation that needs it.
async fn assert_root_unusable(harness: Harness) {
    let mut chroot = harness.build().await.unwrap();
    let results = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let descriptor = exports.wasi_filesystem_types().descriptor();
            Ok(vec![
                stat_size(accessor, exports, fd, "secret.txt")
                    .await?
                    .map(|_| ()),
                stat_size(accessor, exports, fd, "/inside.txt")
                    .await?
                    .map(|_| ()),
                descriptor.call_stat(accessor, fd).await?.map(|_| ()),
                open(
                    accessor,
                    exports,
                    fd,
                    ".",
                    OpenFlags::DIRECTORY,
                    DescriptorFlags::READ,
                )
                .await?
                .map(|_| ()),
            ])
        })
        .await
        .unwrap();
    for result in results {
        assert!(result.is_err(), "{result:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn config_path_must_be_a_directory() {
    let fixture = Fixture::new();
    assert_root_unusable(fixture.harness().config("path", "/missing")).await;
    assert_root_unusable(fixture.harness().config("path", "/jail/inside.txt")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn config_path_must_not_leave_the_preopen_through_a_symlink() {
    let fixture = Fixture::new();
    assert_root_unusable(
        Harness::new("chroot")
            .preopen(fixture.host("jail"), "/", Perms::ReadWrite)
            .config("path", "/link-dir-outside"),
    )
    .await;
}

/// List the root before anything opened it, the listing is forwarded by the chroot rather than
/// passed through from the host.
async fn list_unopened_root(
    fixture: &Fixture,
    harness: Harness,
) -> (Vec<String>, Result<(), ErrorCode>) {
    let _ = fixture;
    let mut chroot = harness.build().await.unwrap();
    chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let (entries, result) = exports
                .wasi_filesystem_types()
                .descriptor()
                .call_read_directory(accessor, fd)
                .await?;
            let mut entries = test_harness::collect(accessor, entries)?;
            let result = test_harness::resolve(accessor, result).await?;
            let mut names = vec![];
            while let Some(entry) = entries.recv().await {
                names.push(entry.name);
            }
            names.sort();
            Ok((names, result))
        })
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn root_is_listed_before_it_is_opened() {
    let fixture = Fixture::new();
    let (names, result) =
        list_unopened_root(&fixture, fixture.harness().config("path", "/jail")).await;
    assert_eq!(result, Ok(()));
    assert_eq!(
        names,
        [
            "inside.txt",
            "link-absolute",
            "link-dir-outside",
            "link-host-absolute",
            "link-inside",
            "link-outside",
            "secret.txt",
            "sub",
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_root_fails_to_list() {
    let fixture = Fixture::new();
    let (names, result) =
        list_unopened_root(&fixture, fixture.harness().config("path", "/missing")).await;
    assert_eq!(result, Err(ErrorCode::NoEntry));
    assert!(names.is_empty(), "{names:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn root_is_not_a_file_before_it_is_opened() {
    // `write-via-stream` and `append-via-stream` are forwarded the same way, but the harness can't
    // give them data, a stream passed through to wasmtime-wasi needs its own Rust type, see
    // `test_harness::resolve`
    let fixture = Fixture::new();
    let mut chroot = fixture.chroot().await;
    let (data, result) = chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let (data, result) = exports
                .wasi_filesystem_types()
                .descriptor()
                .call_read_via_stream(accessor, fd, 0)
                .await?;
            let mut data = test_harness::collect(accessor, data)?;
            let result = test_harness::resolve(accessor, result).await?;
            let mut bytes = vec![];
            while let Some(byte) = data.recv().await {
                bytes.push(byte);
            }
            Ok((bytes, result))
        })
        .await
        .unwrap();
    // the root is a directory, reading it fails rather than trapping
    assert!(result.is_err(), "{result:?}");
    assert!(data.is_empty(), "{data:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn synchronous_operations_work_once_the_root_is_opened() {
    let fixture = Fixture::new();
    let mut chroot = fixture.chroot().await;
    chroot
        .run(async |accessor, exports| {
            let fd = root(accessor, exports).await?;
            let descriptor = exports.wasi_filesystem_types().descriptor();
            descriptor.call_stat(accessor, fd).await?.expect("stat");
            descriptor.call_read_directory(accessor, fd).await?;
            Ok(())
        })
        .await
        .unwrap();
}
