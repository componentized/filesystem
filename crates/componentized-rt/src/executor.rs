//! Runs background futures that outlive the export which started them.
//!
//! Sync exports, like `wasi:sockets/types#tcp-socket.listen`, have no component model task to
//! spawn futures into. A single cooperative thread runs `wit_bindgen::block_on` for the life of
//! the component and polls every background future. It is the only thread that suspends with
//! frames on the component's shadow stack, which is what makes sharing that stack sound, see
//! [`crate::thread::spawn`].
//!
//! Requires the `executor` feature. Like [`crate::thread::spawn`], the component must be linked
//! with `--export-table`.

use core::cell::RefCell;
use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::pin::pin;
use core::task::{Poll, Waker};

struct Executor {
    /// Futures waiting to be picked up by the executor thread.
    queue: Vec<Pin<Box<dyn Future<Output = ()>>>>,
    /// `None` until the executor thread is started, then the waker to call when the queue grows.
    waker: Option<Option<Waker>>,
}

struct State(RefCell<Executor>);

// components are single threaded, cooperative threads only switch at explicit points
unsafe impl Sync for State {}

static EXECUTOR: State = State(RefCell::new(Executor {
    queue: Vec::new(),
    waker: None,
}));

/// Run `future` in the background until it completes.
///
/// The component may not spawn any other threads, and its exports must either run to completion
/// or be callback based async exports, see [`crate::thread::spawn`].
pub fn spawn(future: impl Future<Output = ()> + 'static) {
    let mut executor = EXECUTOR.0.borrow_mut();
    executor.queue.push(Box::pin(future));
    match &mut executor.waker {
        None => {
            executor.waker = Some(None);
            // SAFETY: the executor thread is the only thread the component spawns, and every
            // other task either runs to completion or is a callback based async export
            unsafe { crate::thread::spawn(|| wit_bindgen::block_on(run())) };
        }
        Some(waker) => {
            if let Some(waker) = waker.take() {
                drop(executor);
                waker.wake();
            }
        }
    }
}

/// Hand queued futures to wit-bindgen's local task set, forever.
async fn run() {
    // `wit_bindgen::block_on` unwraps its waitable set when the task yields, and the set is only
    // created once a waitable is registered. Spawning a future wakes the executor, so if every
    // spawned future completes without registering a waitable the next yield panics. A read
    // which never completes, the writer is never written or dropped, keeps a waitable registered
    // from the first poll.
    let (_writer, mut reader) = wit_bindgen::UnitStreamOps::new();
    let mut idle = pin!(reader.read(Vec::new()));

    poll_fn(|cx| {
        let _ = idle.as_mut().poll(cx);
        let mut executor = EXECUTOR.0.borrow_mut();
        for future in executor.queue.drain(..) {
            // the future runs until it completes
            wit_bindgen::spawn_local(future);
        }
        executor.waker = Some(Some(cx.waker().clone()));
        Poll::<()>::Pending
    })
    .await
}
