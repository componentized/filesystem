//! Bindings and helpers for latch components, which implement the `latch` world.
//!
//! The bindings are generated once here and shared by every latch component. A latch implements
//! [`Latch`] and exports it with [`export!`]:
//!
//! ```ignore
//! struct MyLatch {}
//!
//! impl filesystem_latch::Latch for MyLatch {
//!     // ...
//! }
//!
//! filesystem_latch::export!(MyLatch with_types_in filesystem_latch::bindings);
//! ```
//!
//! Imports a latch does not use, like `wasi:config/store` or the wrapped latch, are dropped from
//! the built component.

pub mod bindings {
    wit_bindgen::generate!({
        path: "../../components/wit",
        world: "latch",
        pub_export_macro: true,
        merge_structurally_equal_types: true,
        generate_all
    });
}

#[macro_export]
macro_rules! export {
    ($($t:tt)*) => {
        $crate::bindings::export!($($t)*);
    };
}

pub use bindings::exports::componentized::filesystem::latch::{
    Decision, DescriptorLinkAtArgs, DescriptorMetadataHashAtArgs, DescriptorOpenAtArgs,
    DescriptorOperation, DescriptorSetTimesAtArgs, DescriptorStatAtArgs, ErrorCode,
    FilesystemErrorCode, Guest as Latch, Operation, PreopensOperation,
};
pub use bindings::wasi::filesystem::types::{DescriptorFlags, OpenFlags, PathFlags};

/// Log a message from a latch with `wasi:logging`.
pub fn log(level: bindings::wasi::logging::logging::Level, message: &str) {
    bindings::wasi::logging::logging::log(level, "componentized-latch", message);
}

/// Log a critical message from a latch, formatted like `format!`.
#[macro_export]
macro_rules! critical {
    ($($arg:tt)*) => {
        $crate::log($crate::bindings::wasi::logging::logging::Level::Critical, &format!($($arg)*))
    };
}

/// Log a warning from a latch, formatted like `format!`.
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        $crate::log($crate::bindings::wasi::logging::logging::Level::Warn, &format!($($arg)*))
    };
}

/// Load the latch's config from `wasi:config/store` and parse it.
///
/// When the config cannot be read or parsed, the cause is logged and an `invalid-config` error
/// naming the latch is returned. Parse errors describe the offending entry, e.g.
/// `KEY=<key> VALUE=<value> ERROR=<error>`.
pub fn load_config<T>(
    latch_name: &str,
    parse: impl FnOnce(Vec<(String, String)>) -> Result<T, String>,
) -> Result<T, ErrorCode> {
    use bindings::wasi::config::store;

    store::get_all()
        .map_err(|err| match err {
            store::Error::Upstream(message) => format!("ERROR=upstream: {message}"),
            store::Error::Io(message) => format!("ERROR=io: {message}"),
        })
        .and_then(parse)
        .map_err(|message| {
            critical!("Invalid config LATCH={latch_name} {message}");
            ErrorCode::InvalidConfig(latch_name.to_string())
        })
}

/// The reason to deny with, configured with a `reason` key, `None` when the value is empty. Any
/// value that is not the name of an `error-code` case, e.g. `access` or `no-entry`, is `other`
/// with the value as the message.
pub fn reason(value: String) -> Option<FilesystemErrorCode> {
    match value.as_str() {
        "" => None,
        "other" => Some(FilesystemErrorCode::Other(None)),
        name => Some(error_code_from_name(name).unwrap_or(FilesystemErrorCode::Other(Some(value)))),
    }
}

include!(concat!(env!("OUT_DIR"), "/error_code.rs"));
include!(concat!(env!("OUT_DIR"), "/operation.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operations_are_named_by_interface() {
        assert_eq!(INTERFACE_NAMES, ["preopens", "descriptor"]);
        assert!(OPERATION_NAMES.contains(&"preopens.get-directories"));
        assert!(OPERATION_NAMES.contains(&"preopens.get-directories-item"));
        assert!(OPERATION_NAMES.contains(&"descriptor.open-at"));
        assert!(OPERATION_NAMES.contains(&"descriptor.read-directory-entry"));
        assert!(OPERATION_NAMES.iter().all(|name| {
            INTERFACE_NAMES
                .iter()
                .any(|interface| name.starts_with(&format!("{interface}.")))
        }));
    }

    #[test]
    fn reasons_are_parsed_by_name() {
        for (name, code) in [
            ("not-permitted", FilesystemErrorCode::NotPermitted),
            ("access", FilesystemErrorCode::Access),
            ("cross-device", FilesystemErrorCode::CrossDevice),
            ("illegal-byte-sequence", FilesystemErrorCode::IllegalByteSequence),
            ("other", FilesystemErrorCode::Other(None)),
            (
                "custom-reason",
                FilesystemErrorCode::Other(Some("custom-reason".to_string())),
            ),
        ] {
            assert_eq!(
                format!("{:?}", reason(name.to_string())),
                format!("{:?}", Some(code)),
                "{name}"
            );
        }
        assert!(reason(String::new()).is_none());
    }
}
