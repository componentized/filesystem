fn main() {
    // componentized_rt::executor's thread is started with thread.new-indirect, which calls through
    // the exported function table
    if std::env::var("CARGO_CFG_TARGET_FAMILY").as_deref() == Ok("wasm") {
        println!("cargo:rustc-link-arg-cdylib=--export-table");
    }
}
