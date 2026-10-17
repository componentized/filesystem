//! Runtime support for components.
//!
//! Cooperative component model threads, and a background executor for futures started by sync
//! exports.

#[cfg(feature = "executor")]
pub mod executor;

/// Cooperative component model threads.
///
/// Threads are created with the component model's `thread.new-indirect` builtin, without any
/// support from a libc. Every thread runs on the component's one shadow stack, which is only
/// sound while at most one thread ever suspends with frames on that stack. Tasks which run to
/// completion, or which use wit-bindgen's callback based async exports, never do.
pub mod thread {
    #[cfg(target_family = "wasm")]
    #[link(wasm_import_module = "$root")]
    unsafe extern "C" {
        #[link_name = "[thread-new-indirect-v0]"]
        fn thread_new_indirect(start: unsafe extern "C" fn(*mut u8), arg: *mut u8) -> u32;
        #[link_name = "[thread-resume-later]"]
        fn thread_resume_later(thread: u32);
    }

    #[cfg(target_family = "wasm")]
    unsafe extern "C" fn start(arg: *mut u8) {
        let f = unsafe { Box::from_raw(arg as *mut Box<dyn FnOnce()>) };
        f()
    }

    /// Spawn a thread running `f`, it starts once the current thread yields or returns.
    ///
    /// Starting later means the current thread's frames have been popped from the shared stack
    /// before the new thread pushes its own.
    ///
    /// `thread.new-indirect` calls through the module's function table, so the component must be
    /// linked with `--export-table`, e.g. from its build script:
    /// `cargo:rustc-link-arg-cdylib=--export-table`.
    ///
    /// # Safety
    ///
    /// The new thread shares the component's shadow stack. If it may suspend with frames on the
    /// stack, for example by calling `wit_bindgen::block_on`, no other thread may do the same
    /// while it is alive, and every task must run to completion or use callback based async
    /// exports.
    pub unsafe fn spawn(f: impl FnOnce() + 'static) {
        let f: Box<Box<dyn FnOnce()>> = Box::new(Box::new(f));
        let arg = Box::into_raw(f) as *mut u8;
        #[cfg(target_family = "wasm")]
        unsafe {
            let thread = thread_new_indirect(start, arg);
            thread_resume_later(thread);
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let _ = arg;
            unimplemented!("component model threads are only available on wasm")
        }
    }
}
