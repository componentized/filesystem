//! Test harness for the filesystem components.
//!
//! A [`Harness`] instantiates a component (`target/components/*/*.wasm`) with real directories
//! preopened from the host by wasmtime-wasi, a scripted [`HostLatch`], values for
//! `wasi:config/store`, and captured `wasi:logging` output. Latch components from
//! `target/components/` can be installed in place of, or alongside, the host latch, they are
//! composed into the component before it is instantiated. Tests call the component's exported
//! `wasi:filesystem` interfaces.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use tokio::sync::{mpsc, oneshot};
use wac_graph::{
    CompositionGraph, EncodeOptions,
    types::{Package, Types},
};
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
use crate::latch_bindings::componentized::filesystem::latch::{
    self, DescriptorOperation, Operation, PreopensOperation,
};

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

/// The latch the test subject imports, generated apart from [`bindings`], its operations borrow
/// host descriptors, which aren't comparable.
pub mod latch_bindings {
    wasmtime::component::bindgen!({
        path: "../../components/wit",
        inline: "
            package componentized:test-harness;

            world latch {
                import componentized:filesystem/latch@0.3.0-dev;
            }
        ",
        world: "componentized:test-harness/latch",
        require_store_data_send: true,
        with: {
            "wasi:filesystem": wasmtime_wasi::p3::bindings::filesystem,
            "wasi:clocks": wasmtime_wasi::p3::bindings::clocks,
        },
    });
}

pub use bindings::exports::wasi::filesystem::types::{
    DescriptorFlags, DescriptorStat, DescriptorType, ErrorCode, OpenFlags, PathFlags,
};
pub use bindings::wasi::logging::logging::Level;
pub use latch_bindings::componentized::filesystem::latch::{
    Decision, ErrorCode as LatchErrorCode, FilesystemErrorCode,
};
pub use wasmtime_wasi::FsPerms as Perms;

/// Receive the items written to a guest stream on a channel.
///
/// Only for streams the guest writes itself, or byte streams, see [`resolve`].
pub fn collect<T: Lift + Send + Sync + 'static>(
    accessor: &Accessor<Ctx>,
    stream: StreamReader<T>,
) -> Result<mpsc::UnboundedReceiver<T>> {
    let (tx, rx) = mpsc::unbounded_channel();
    accessor.with(|store| stream.pipe(store, ChannelConsumer(tx)))?;
    Ok(rx)
}

/// Create a stream for the guest that yields the items, then closes.
pub fn stream<T: wasmtime::component::Lower + Lift + Unpin + Send + Sync + 'static>(
    accessor: &Accessor<Ctx>,
    items: Vec<T>,
) -> Result<StreamReader<T>> {
    accessor.with(|store| StreamReader::new(store, items))
}

/// Wait for the value of a guest future.
///
/// Only for futures the guest writes itself. A future the guest passes through from a wasmtime-wasi
/// host import is transferred host to host, which requires the same Rust type on both ends, and
/// the harness' exported types are generated separately from wasmtime-wasi's. Bindgen's `with`
/// doesn't apply to exports, and wasmtime won't convert a host stream or future to another Rust
/// type. A byte stream has the same Rust type on both ends.
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

/// Rebuild the named components in `target/components/` with make, unless they were already built
/// by this process.
fn ensure_built(names: &[&str]) -> Result<()> {
    static BUILT: Mutex<Option<HashSet<String>>> = Mutex::new(None);

    // hold the lock while building so concurrent tests don't run make over each other
    let mut built = BUILT.lock().unwrap_or_else(|err| err.into_inner());
    let built = built.get_or_insert_with(HashSet::new);
    let targets: Vec<String> = names
        .iter()
        .filter(|name| !built.contains(**name))
        .map(|name| format!("target/components/{name}/{name}.wasm"))
        .collect();
    if targets.is_empty() {
        return Ok(());
    }

    let output = Command::new("make")
        .arg("-C")
        .arg(workspace_dir())
        .args(&targets)
        .output()
        .context("failed to run make")?;
    if !output.status.success() {
        bail!(
            "failed to build {}:\n{}{}",
            targets.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    built.extend(names.iter().map(|name| name.to_string()));
    Ok(())
}

/// An operation the host latch was asked to authorize.
///
/// Resource handles are not retained, only the operation name and a debug rendering of its
/// arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authorization {
    /// Operation name, e.g. `descriptor.open-at`
    pub operation: String,
    /// Debug rendering of the operation arguments, excluding the descriptor operated on
    pub args: String,
}

/// A message logged by the test subject via `wasi:logging`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub level: Level,
    pub context: String,
    pub message: String,
}

impl LogEntry {
    /// A trace message logged by the test subject.
    pub fn trace(context: impl Into<String>, message: impl Into<String>) -> Self {
        Self::log(Level::Trace, context, message)
    }

    /// A warning message logged by the test subject.
    pub fn warn(context: impl Into<String>, message: impl Into<String>) -> Self {
        Self::log(Level::Warn, context, message)
    }

    /// An error message logged by the test subject.
    pub fn error(context: impl Into<String>, message: impl Into<String>) -> Self {
        Self::log(Level::Error, context, message)
    }

    fn log(level: Level, context: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            level,
            context: context.into(),
            message: message.into(),
        }
    }
}

type Policy = dyn FnMut(&Authorization) -> Result<Decision, LatchErrorCode> + Send;

/// Scripted latch implemented by the host.
///
/// It is the test subject's latch unless latch components are installed, see [`Harness::latch`].
pub struct HostLatch {
    policy: Box<Policy>,
    fail_observe: bool,
}

impl HostLatch {
    /// Defer every operation.
    pub fn defer() -> Self {
        Self::new(|_| Ok(Decision::Deferred))
    }

    /// Deny the named operations with `access`, defer the rest.
    pub fn deny(operations: &[&str]) -> Self {
        let operations: Vec<String> = operations.iter().map(|s| s.to_string()).collect();
        Self::new(move |auth| {
            if operations.contains(&auth.operation) {
                Ok(Decision::Denied(FilesystemErrorCode::Access))
            } else {
                Ok(Decision::Deferred)
            }
        })
    }

    /// Fail the named operations with a latch error, defer the rest.
    pub fn error(operations: &[&str], message: &str) -> Self {
        let operations: Vec<String> = operations.iter().map(|s| s.to_string()).collect();
        let message = message.to_string();
        Self::new(move |auth| {
            if operations.contains(&auth.operation) {
                Err(LatchErrorCode::Other(Some(message.clone())))
            } else {
                Ok(Decision::Deferred)
            }
        })
    }

    /// Decide each operation with a custom policy.
    pub fn new(
        policy: impl FnMut(&Authorization) -> Result<Decision, LatchErrorCode> + Send + 'static,
    ) -> Self {
        Self {
            policy: Box::new(policy),
            fail_observe: false,
        }
    }

    /// Fail every `observe-decision` call.
    pub fn fail_observe(mut self) -> Self {
        self.fail_observe = true;
        self
    }
}

/// A decision the host latch was told about with `observe-decision`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    /// Operation name, e.g. `descriptor.open-at`
    pub operation: String,
    /// Whether the final decision denied the operation
    pub denied: bool,
}

/// Observations recorded while the test subject runs.
#[derive(Clone, Default)]
pub struct Recorder {
    authorizations: Arc<Mutex<Vec<Authorization>>>,
    observations: Arc<Mutex<Vec<Observation>>>,
    logs: Arc<Mutex<Vec<LogEntry>>>,
}

impl Recorder {
    /// Operations the host latch was asked to authorize, in order.
    pub fn authorizations(&self) -> Vec<Authorization> {
        self.authorizations.lock().unwrap().clone()
    }

    /// Names of the operations the host latch was asked to authorize, in order.
    pub fn operations(&self) -> Vec<String> {
        self.authorizations()
            .into_iter()
            .map(|a| a.operation)
            .collect()
    }

    /// Decisions the host latch observed, in order.
    pub fn observations(&self) -> Vec<Observation> {
        self.observations.lock().unwrap().clone()
    }

    /// Messages logged by the test subject, in order.
    pub fn logs(&self) -> Vec<LogEntry> {
        self.logs.lock().unwrap().clone()
    }
}

pub struct Ctx {
    wasi: WasiCtx,
    table: ResourceTable,
    latch: HostLatch,
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

impl latch::Host for Ctx {
    fn authorize(&mut self, operation: Operation) -> Result<Decision, LatchErrorCode> {
        let authorization = describe(&operation);
        self.recorder
            .authorizations
            .lock()
            .unwrap()
            .push(authorization.clone());
        (self.latch.policy)(&authorization)
    }

    fn observe_decision(
        &mut self,
        decision: Decision,
        operation: Operation,
    ) -> Result<(), LatchErrorCode> {
        self.recorder
            .observations
            .lock()
            .unwrap()
            .push(Observation {
                operation: describe(&operation).operation,
                denied: matches!(decision, Decision::Denied(_)),
            });
        match self.latch.fail_observe {
            true => Err(LatchErrorCode::ObservationFailed("host".to_string())),
            false => Ok(()),
        }
    }
}

/// Define the host latch under a named import, e.g. `latch1` of a `latch-n` component.
fn add_named_latch_to_linker(linker: &mut Linker<Ctx>, name: &str) -> Result<()> {
    let mut instance = linker.instance(name)?;
    instance.func_wrap(
        "authorize",
        |mut store: StoreContextMut<'_, Ctx>, (operation,): (Operation,)| {
            Ok((latch::Host::authorize(store.data_mut(), operation),))
        },
    )?;
    instance.func_wrap(
        "observe-decision",
        |mut store: StoreContextMut<'_, Ctx>, (decision, operation): (Decision, Operation)| {
            Ok((latch::Host::observe_decision(
                store.data_mut(),
                decision,
                operation,
            ),))
        },
    )?;
    Ok(())
}

fn describe(operation: &Operation) -> Authorization {
    let (operation, args) = match operation {
        Operation::Preopens(op) => match op {
            PreopensOperation::GetDirectories => ("preopens.get-directories", String::new()),
            PreopensOperation::GetDirectoriesItem((_, path)) => {
                ("preopens.get-directories-item", format!("{path:?}"))
            }
        },
        Operation::Descriptor(op) => match op {
            DescriptorOperation::ReadViaStream((_, _, args)) => {
                ("descriptor.read-via-stream", format!("{args:?}"))
            }
            DescriptorOperation::WriteViaStream((_, _, args)) => {
                ("descriptor.write-via-stream", format!("{args:?}"))
            }
            DescriptorOperation::AppendViaStream(_) => {
                ("descriptor.append-via-stream", String::new())
            }
            DescriptorOperation::Advise((_, _, args)) => ("descriptor.advise", format!("{args:?}")),
            DescriptorOperation::SyncData(_) => ("descriptor.sync-data", String::new()),
            DescriptorOperation::GetFlags(_) => ("descriptor.get-flags", String::new()),
            DescriptorOperation::GetType(_) => ("descriptor.get-type", String::new()),
            DescriptorOperation::SetSize((_, _, args)) => {
                ("descriptor.set-size", format!("{args:?}"))
            }
            DescriptorOperation::SetTimes((_, _, args)) => {
                ("descriptor.set-times", format!("{args:?}"))
            }
            DescriptorOperation::ReadDirectory(_) => ("descriptor.read-directory", String::new()),
            DescriptorOperation::ReadDirectoryEntry((_, _, entry)) => {
                ("descriptor.read-directory-entry", format!("{entry:?}"))
            }
            DescriptorOperation::Sync(_) => ("descriptor.sync", String::new()),
            DescriptorOperation::CreateDirectoryAt((_, _, args)) => {
                ("descriptor.create-directory-at", format!("{args:?}"))
            }
            DescriptorOperation::Stat(_) => ("descriptor.stat", String::new()),
            DescriptorOperation::StatAt((_, _, args)) => {
                ("descriptor.stat-at", format!("{args:?}"))
            }
            DescriptorOperation::SetTimesAt((_, _, args)) => {
                ("descriptor.set-times-at", format!("{args:?}"))
            }
            DescriptorOperation::LinkAt((_, _, args)) => (
                "descriptor.link-at",
                format!(
                    "old_path_flags: {:?}, old_path: {:?}, new_path: {:?}",
                    args.old_path_flags, args.old_path, args.new_path
                ),
            ),
            DescriptorOperation::OpenAt((_, _, args)) => {
                ("descriptor.open-at", format!("{args:?}"))
            }
            DescriptorOperation::ReadlinkAt((_, _, args)) => {
                ("descriptor.readlink-at", format!("{args:?}"))
            }
            DescriptorOperation::RemoveDirectoryAt((_, _, args)) => {
                ("descriptor.remove-directory-at", format!("{args:?}"))
            }
            DescriptorOperation::RenameAt((_, _, args)) => (
                "descriptor.rename-at",
                format!(
                    "old_path: {:?}, new_path: {:?}",
                    args.old_path, args.new_path
                ),
            ),
            DescriptorOperation::SymlinkAt((_, _, args)) => {
                ("descriptor.symlink-at", format!("{args:?}"))
            }
            DescriptorOperation::UnlinkFileAt((_, _, args)) => {
                ("descriptor.unlink-file-at", format!("{args:?}"))
            }
            DescriptorOperation::MetadataHash(_) => ("descriptor.metadata-hash", String::new()),
            DescriptorOperation::MetadataHashAt((_, _, args)) => {
                ("descriptor.metadata-hash-at", format!("{args:?}"))
            }
        },
    };
    Authorization {
        operation: operation.to_string(),
        args,
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
    latches: Vec<String>,
    host_latch: Option<HostLatch>,
    config: Vec<(String, String)>,
    preopens: Vec<(PathBuf, String, FsPerms)>,
}

impl Harness {
    /// Test the named component from `target/components/`, e.g. `chroot`.
    pub fn new(component_name: &str) -> Self {
        Self {
            subject: component_name.to_string(),
            latches: vec![],
            host_latch: None,
            config: vec![],
            preopens: vec![],
        }
    }

    /// Install a latch component from `target/components/`.
    ///
    /// Without latch components the host latch is the test subject's latch. A single latch
    /// component replaces it, unless the latch imports a latch, then it wraps the host latch.
    /// Otherwise several latches, including the host latch when one is set with
    /// [`Harness::host_latch`], are aggregated with the `latch-n` component of the same size,
    /// in the order installed with the host latch last.
    pub fn latch(mut self, name: &str) -> Self {
        self.latches.push(name.to_string());
        self
    }

    /// Replace the default deferring host latch, it is aggregated with any latch components.
    pub fn host_latch(mut self, latch: HostLatch) -> Self {
        self.host_latch = Some(latch);
        self
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

    /// Compose and instantiate the test subject.
    pub async fn build(self) -> Result<TestSubject> {
        let mut config = Config::new();
        config.wasm_component_model_async(true);
        // componentized-rt's background executor runs on a component model thread
        config.wasm_component_model_threading(true);
        // wac-graph encodes the named `latch{i}` imports of an aggregated latch-n with `implements`
        config.wasm_component_model_implements(true);
        let engine = Engine::new(&config)?;

        let bytes = self.compose()?;
        let component = Component::new(&engine, &bytes)?;

        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p3::add_to_linker(&mut linker)?;
        latch::add_to_linker::<_, HasSelf<Ctx>>(&mut linker, |ctx| ctx)?;
        for slot in 0..LATCH_N_MAX {
            add_named_latch_to_linker(&mut linker, &format!("latch{slot}"))?;
        }
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
                latch: self.host_latch.unwrap_or_else(HostLatch::defer),
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

impl Harness {
    fn compose(&self) -> Result<Vec<u8>> {
        let mut names = vec![self.subject.as_str()];
        names.extend(self.latches.iter().map(String::as_str));
        ensure_built(&names)?;

        let read = |name: &str| {
            let path = component_path(name);
            std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))
        };

        let bytes = read(&self.subject)?;
        let latch = match self.latches.as_slice() {
            [] => return Ok(bytes),
            // a latch that wraps another latch wraps the host latch, its import is left for the host
            [latch] if self.host_latch.is_none() || imports_latch(&read(latch)?)? => read(latch)?,
            latches => {
                // the host latch takes the last slot of latch-n, left unsatisfied it is imported
                let slots = latches.len() + usize::from(self.host_latch.is_some());
                if slots > LATCH_N_MAX {
                    bail!("at most {LATCH_N_MAX} latches can be aggregated, got {slots}");
                }
                let latch_n = format!("latch-n{slots}");
                ensure_built(&[&latch_n])?;
                let latches = latches
                    .iter()
                    .map(|name| read(name))
                    .collect::<Result<Vec<_>>>()?;
                aggregate(read(&latch_n)?, latches)?
            }
        };
        plug(&self.subject, bytes, latch)
    }
}

/// The most latches a `latch-n` component aggregates.
const LATCH_N_MAX: usize = 5;

const LATCH_INTERFACE: &str = "componentized:filesystem/latch@0.3.0-dev";

/// Whether the latch component imports a latch, which it wraps.
fn imports_latch(latch: &[u8]) -> Result<bool> {
    let mut types = Types::default();
    let package = Package::from_bytes("test:latch", None, latch.to_vec(), &mut types)
        .map_err(|err| format_err!("{err:#}"))?;
    Ok(types[package.ty()].imports.contains_key(LATCH_INTERFACE))
}

/// Satisfy the leading `latch{i}` imports of a `latch-n` component with the latches, any
/// remaining slot is left for the host.
fn aggregate(latch_n: Vec<u8>, latches: Vec<Vec<u8>>) -> Result<Vec<u8>> {
    let mut graph = CompositionGraph::new();
    let latch_n = Package::from_bytes("test:latch-n", None, latch_n, graph.types_mut())
        .map_err(|err| format_err!("{err:#}"))?;
    let latch_n = graph.register_package(latch_n)?;
    let latch_n = graph.instantiate(latch_n);
    for (slot, latch) in latches.into_iter().enumerate() {
        let latch =
            Package::from_bytes(&format!("test:latch{slot}"), None, latch, graph.types_mut())
                .map_err(|err| format_err!("{err:#}"))?;
        let latch = graph.register_package(latch)?;
        let latch = graph.instantiate(latch);
        let export = graph.alias_instance_export(latch, LATCH_INTERFACE)?;
        graph.set_instantiation_argument(latch_n, &format!("latch{slot}"), export)?;
    }
    let export = graph.alias_instance_export(latch_n, LATCH_INTERFACE)?;
    graph.export(export, LATCH_INTERFACE)?;
    Ok(graph.encode(EncodeOptions::default())?)
}

/// Satisfy the socket's imports with the plug's exports.
fn plug(name: &str, socket: Vec<u8>, plug: Vec<u8>) -> Result<Vec<u8>> {
    let mut graph = CompositionGraph::new();
    let socket = Package::from_bytes("test:socket", None, socket, graph.types_mut())
        .map_err(|err| format_err!("{err:#}"))?;
    let socket = graph.register_package(socket)?;
    let plug = Package::from_bytes("test:plug", None, plug, graph.types_mut())
        .map_err(|err| format_err!("{err:#}"))?;
    let plug = graph.register_package(plug)?;
    if let Err(err) = wac_graph::plug(&mut graph, vec![plug], socket) {
        bail!("failed to plug latch into {name}: {err}");
    }
    Ok(graph.encode(EncodeOptions::default())?)
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
    /// Observations recorded by the host latch and logger.
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
