//! Test harness for the filesystem components.
//!
//! A [`Harness`] instantiates a component (`target/components/*/*.wasm`) with real directories
//! preopened from the host by wasmtime-wasi, values for `wasi:config/store`, and captured
//! `wasi:logging` output. Tests call the component's exported `wasi:filesystem` interfaces.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use tokio::sync::{mpsc, oneshot};
use wasmtime::component::{
    Accessor, Component, FutureConsumer, FutureReader, HasSelf, Lift, Linker, ResourceTable,
    Source, StreamConsumer, StreamReader, StreamResult,
};
use wasmtime::error::Context as _;
use wasmtime::{Config, Engine, Result, Store, StoreContextMut, bail, format_err};
use wasmtime_wasi::{FsPerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::bindings::exports::wasi::filesystem::{preopens, types};
use crate::bindings::wasi::config::store;
use crate::bindings::wasi::logging::logging;

pub mod bindings {
    wasmtime::component::bindgen!({
        path: "../../components/wit",
        inline: "
            package componentized:test-harness;

            world harness {
                import wasi:config/store@0.2.0-rc.1;
                import wasi:logging/logging@0.1.0-draft;
                export wasi:filesystem/types@0.3.0;
                export wasi:filesystem/preopens@0.3.0;
            }
        ",
        world: "componentized:test-harness/harness",
        exports: { default: async | store },
        additional_derives: [PartialEq],
    });
}

pub use bindings::exports::wasi::filesystem::types::{
    DescriptorFlags, DescriptorStat, DescriptorType, ErrorCode, OpenFlags, PathFlags,
};
pub use bindings::wasi::logging::logging::Level;
pub use wasmtime_wasi::FsPerms as Perms;

/// Receive the items written to a guest stream on a channel.
///
/// Only for streams the guest writes itself, see [`resolve`].
pub fn collect<T: Lift + Send + Sync + 'static>(
    accessor: &Accessor<Ctx>,
    stream: StreamReader<T>,
) -> Result<mpsc::UnboundedReceiver<T>> {
    let (tx, rx) = mpsc::unbounded_channel();
    accessor.with(|store| stream.pipe(store, ChannelConsumer(tx)))?;
    Ok(rx)
}

/// Wait for the value of a guest future.
///
/// Only for futures the guest writes itself. A future the guest passes through from a wasmtime-wasi
/// host import is transferred host to host, which requires the same Rust type on both ends, and
/// the harness' exported types are generated separately from wasmtime-wasi's.
pub async fn resolve<T: Lift + Send + Sync + 'static>(
    accessor: &Accessor<Ctx>,
    future: FutureReader<T>,
) -> Result<T> {
    let (tx, rx) = oneshot::channel();
    accessor.with(|store| future.pipe(store, OneshotConsumer(Some(tx))))?;
    rx.await
        .map_err(|_| format_err!("future closed without a value"))
}

struct OneshotConsumer<T>(Option<oneshot::Sender<T>>);

impl<D, T: Lift + Send + Sync + 'static> FutureConsumer<D> for OneshotConsumer<T> {
    type Item = T;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        store: StoreContextMut<D>,
        mut source: Source<'_, T>,
        _finish: bool,
    ) -> Poll<Result<()>> {
        let mut item = None;
        source.read(store, &mut item)?;
        if let (Some(item), Some(tx)) = (item, self.get_mut().0.take()) {
            // the receiver is only dropped when the test stopped waiting
            let _ = tx.send(item);
        }
        Poll::Ready(Ok(()))
    }
}

struct ChannelConsumer<T>(mpsc::UnboundedSender<T>);

impl<D, T: Lift + Send + Sync + 'static> StreamConsumer<D> for ChannelConsumer<T> {
    type Item = T;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        store: StoreContextMut<D>,
        mut source: Source<'_, T>,
        _finish: bool,
    ) -> Poll<Result<StreamResult>> {
        let mut item = None;
        source.read(store, &mut item)?;
        if let Some(item) = item {
            if self.0.send(item).is_err() {
                return Poll::Ready(Ok(StreamResult::Dropped));
            }
        }
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

/// Root of the workspace, where the Makefile lives.
fn workspace_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Path containing the built component.
pub fn component_path(name: &str) -> PathBuf {
    workspace_dir().join(format!("target/components/{name}/{name}.wasm"))
}

/// Rebuild the named component in `target/components/` with make, unless it was already built by
/// this process.
fn ensure_built(name: &str) -> Result<()> {
    static BUILT: Mutex<Option<HashSet<String>>> = Mutex::new(None);

    // hold the lock while building so concurrent tests don't run make over each other
    let mut built = BUILT.lock().unwrap_or_else(|err| err.into_inner());
    let built = built.get_or_insert_with(HashSet::new);
    if built.contains(name) {
        return Ok(());
    }

    let target = format!("target/components/{name}/{name}.wasm");
    let output = Command::new("make")
        .arg("-C")
        .arg(workspace_dir())
        .arg(&target)
        .output()
        .context("failed to run make")?;
    if !output.status.success() {
        bail!(
            "failed to build {target}:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    built.insert(name.to_string());
    Ok(())
}

/// A message logged by the test subject via `wasi:logging`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub level: Level,
    pub context: String,
    pub message: String,
}

/// Observations recorded while the test subject runs.
#[derive(Clone, Default)]
pub struct Recorder {
    logs: Arc<Mutex<Vec<LogEntry>>>,
}

impl Recorder {
    /// Messages logged by the test subject, in order.
    pub fn logs(&self) -> Vec<LogEntry> {
        self.logs.lock().unwrap().clone()
    }
}

pub struct Ctx {
    wasi: WasiCtx,
    table: ResourceTable,
    config: Vec<(String, String)>,
    recorder: Recorder,
}

impl WasiView for Ctx {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl store::Host for Ctx {
    fn get(&mut self, key: String) -> Result<Option<String>, store::Error> {
        Ok(self
            .config
            .iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.clone()))
    }

    fn get_all(&mut self) -> Result<Vec<(String, String)>, store::Error> {
        Ok(self.config.clone())
    }
}

impl logging::Host for Ctx {
    fn log(&mut self, level: Level, context: String, message: String) {
        self.recorder.logs.lock().unwrap().push(LogEntry {
            level,
            context,
            message,
        });
    }
}

/// Builds a subject instance for a test.
pub struct Harness {
    subject: String,
    config: Vec<(String, String)>,
    preopens: Vec<(PathBuf, String, FsPerms)>,
}

impl Harness {
    /// Test the named component from `target/components/`, e.g. `chroot`.
    pub fn new(component_name: &str) -> Self {
        Self {
            subject: component_name.to_string(),
            config: vec![],
            preopens: vec![],
        }
    }

    /// Add a `wasi:config/store` value.
    pub fn config(mut self, key: &str, value: &str) -> Self {
        self.config.push((key.to_string(), value.to_string()));
        self
    }

    /// Preopen a host directory for the test subject under the guest path.
    pub fn preopen(
        mut self,
        host_path: impl AsRef<Path>,
        guest_path: &str,
        perms: FsPerms,
    ) -> Self {
        self.preopens.push((
            host_path.as_ref().to_path_buf(),
            guest_path.to_string(),
            perms,
        ));
        self
    }

    /// Instantiate the test subject.
    pub async fn build(self) -> Result<TestSubject> {
        ensure_built(&self.subject)?;

        let mut config = Config::new();
        config.wasm_component_model_async(true);
        // componentized-rt's background executor runs on a component model thread
        config.wasm_component_model_threading(true);
        let engine = Engine::new(&config)?;

        let path = component_path(&self.subject);
        let component = Component::from_file(&engine, &path)
            .with_context(|| format!("failed to read {}", path.display()))?;

        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p3::add_to_linker(&mut linker)?;
        store::add_to_linker::<_, HasSelf<Ctx>>(&mut linker, |ctx| ctx)?;
        logging::add_to_linker::<_, HasSelf<Ctx>>(&mut linker, |ctx| ctx)?;

        let mut wasi = WasiCtxBuilder::new();
        wasi.inherit_stdio();
        for (host_path, guest_path, perms) in &self.preopens {
            wasi.preopened_dir(host_path, guest_path, *perms)
                .with_context(|| format!("failed to preopen {}", host_path.display()))?;
        }

        let recorder = Recorder::default();
        let mut store = Store::new(
            &engine,
            Ctx {
                wasi: wasi.build(),
                table: ResourceTable::new(),
                config: self.config,
                recorder: recorder.clone(),
            },
        );
        let instance_pre = linker.instantiate_pre(&component)?;
        let instance = instance_pre
            .instantiate_async(&mut store)
            .await
            .with_context(|| format!("failed to instantiate {}", self.subject))?;
        let exports = Exports {
            types: types::GuestIndices::new(&instance_pre)?.load(&mut store, &instance)?,
            preopens: preopens::GuestIndices::new(&instance_pre)?.load(&mut store, &instance)?,
        };

        Ok(TestSubject {
            store,
            exports,
            recorder,
        })
    }
}

/// The test subject's exported interfaces.
pub struct Exports {
    types: types::Guest,
    preopens: preopens::Guest,
}

impl Exports {
    /// The exported `wasi:filesystem/types` interface.
    pub fn wasi_filesystem_types(&self) -> &types::Guest {
        &self.types
    }

    /// The exported `wasi:filesystem/preopens` interface.
    pub fn wasi_filesystem_preopens(&self) -> &preopens::Guest {
        &self.preopens
    }
}

/// An instantiated test subject.
pub struct TestSubject {
    store: Store<Ctx>,
    exports: Exports,
    recorder: Recorder,
}

impl TestSubject {
    /// Observations recorded by the logger.
    pub fn recorder(&self) -> Recorder {
        self.recorder.clone()
    }

    /// Run a test body against the test subject's exports.
    pub async fn run<R: Send + 'static>(
        &mut self,
        f: impl AsyncFnOnce(&Accessor<Ctx>, &Exports) -> Result<R> + Send,
    ) -> Result<R> {
        let exports = &self.exports;
        self.store
            .run_concurrent(async move |accessor| f(accessor, exports).await)
            .await?
    }
}
