//! Tests for `latch-deny-random`, as the gate's latch.
//!
//! Each test preopens an empty directory as `/`, and gets the preopens repeatedly. Getting the
//! preopens and the preopen are each authorized, a denial of either returns no preopens.

use tempfile::TempDir;
use test_harness::{ErrorCode, Harness, Level, LogEntry, Perms, TestSubject};

const LATCH: &str = "latch-deny-random";

/// A gate with the latch, configured with the entries.
async fn gate(dir: &TempDir, config: &[(&str, &str)]) -> wasmtime::Result<TestSubject> {
    let mut harness = Harness::new("gate")
        .preopen(dir.path(), "/", Perms::ReadWrite)
        .latch(LATCH);
    for (key, value) in config {
        harness = harness.config(key, value);
    }
    harness.build().await
}

/// Get the preopens, whether each get was denied.
async fn gets(gate: &mut TestSubject, count: usize) -> wasmtime::Result<Vec<bool>> {
    gate.run(async move |accessor, exports| {
        let mut denied = vec![];
        for _ in 0..count {
            let dirs = exports
                .wasi_filesystem_preopens()
                .call_get_directories(accessor)
                .await?;
            denied.push(dirs.is_empty());
        }
        Ok(denied)
    })
    .await
}

/// The messages the latch logged.
fn latch_logs(gate: &TestSubject) -> Vec<LogEntry> {
    gate.recorder()
        .logs()
        .into_iter()
        .filter(|entry| entry.context == "componentized-latch")
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn always_denies() -> wasmtime::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut gate = gate(&dir, &[("probability", "1"), ("seed", "7")]).await?;
    assert_eq!(gets(&mut gate, 3).await?, vec![true, true, true]);
    assert_eq!(
        latch_logs(&gate),
        vec![LogEntry::warn(
            "componentized-latch",
            "Randomly denying wasi:filesystem operations PROBABILITY=1 SEED=7"
        )]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn denies_with_the_configured_reason() -> wasmtime::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut gate = gate(&dir, &[("probability", "1"), ("reason", "io")]).await?;
    gets(&mut gate, 1).await?;
    assert_eq!(
        gate.recorder()
            .logs()
            .into_iter()
            .filter(|entry| entry.context == "componentized-gate")
            .collect::<Vec<_>>(),
        vec![LogEntry::warn(
            "componentized-gate",
            "Denied REASON=io OPERATION=wasi:filesystem/preopens#get-directories"
        )]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn never_denies() -> wasmtime::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut gate = gate(&dir, &[("probability", "0")]).await?;
    assert_eq!(gets(&mut gate, 3).await?, vec![false, false, false]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn same_seed_denies_the_same_operations() -> wasmtime::Result<()> {
    let dir = tempfile::tempdir()?;
    let config = [("probability", "0.5"), ("seed", "1234")];
    let first = gets(&mut gate(&dir, &config).await?, 32).await?;
    let again = gets(&mut gate(&dir, &config).await?, 32).await?;
    assert_eq!(first, again);
    // some of each, two coin flips for each of 32 gets
    assert!(first.contains(&true) && first.contains(&false), "{first:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn random_seed_is_logged() -> wasmtime::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut logged = vec![];
    for _ in 0..2 {
        let mut gate = gate(&dir, &[]).await?;
        gets(&mut gate, 1).await?;
        let logs = latch_logs(&gate);
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].level, Level::Warn);
        let seed = logs[0]
            .message
            .strip_prefix("Randomly denying wasi:filesystem operations PROBABILITY=0.1 SEED=")
            .expect("seed is logged")
            .parse::<u64>()?;
        logged.push(seed);
    }
    // each instance draws its own seed
    assert_ne!(logged[0], logged[1]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_config_is_logged() -> wasmtime::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut gate = gate(&dir, &[("probability", "often")]).await?;
    // a latch error getting the preopens can't be returned, the gate traps
    assert!(gets(&mut gate, 1).await.is_err());
    assert_eq!(
        latch_logs(&gate),
        vec![LogEntry::critical(
            "componentized-latch",
            "Invalid config LATCH=latch-deny-random KEY=probability VALUE=often ERROR=expected a number from 0 to 1"
        )]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn probability_varies_by_operation() -> wasmtime::Result<()> {
    let dir = tempfile::tempdir()?;
    let mut gate = gate(
        &dir,
        &[
            ("probability", "1"),
            ("probability.preopens", "0"),
            ("seed", "7"),
        ],
    )
    .await?;
    let (preopens, stat) = gate
        .run(async |accessor, exports| {
            let dirs = exports
                .wasi_filesystem_preopens()
                .call_get_directories(accessor)
                .await?;
            let count = dirs.len();
            let (root, _) = dirs.into_iter().next().expect("a preopen");
            let stat = exports
                .wasi_filesystem_types()
                .descriptor()
                .call_stat(accessor, root)
                .await?;
            Ok((count, stat.map(|_| ())))
        })
        .await?;
    // the preopens are never denied, every other operation is
    assert_eq!(preopens, 1);
    assert_eq!(stat, Err(ErrorCode::Access));
    assert_eq!(
        latch_logs(&gate),
        vec![LogEntry::warn(
            "componentized-latch",
            "Randomly denying wasi:filesystem operations PROBABILITY=1 SEED=7 PROBABILITY.preopens=0"
        )]
    );
    Ok(())
}
