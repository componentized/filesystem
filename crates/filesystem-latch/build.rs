//! Generate code from the same WIT the bindings are generated from, so a case added to the WIT is
//! handled without changing the latches:
//!
//! - `error_code_from_name`, which parses the kebab-case name of every case of
//!   `wasi:filesystem/types#error-code` without a payload.
//! - `operation_name`, the name of every latch operation, e.g. `descriptor.open-at`, and
//!   `INTERFACE_NAMES` and `OPERATION_NAMES`, every interface and operation name.

use std::fmt::Write as _;
use std::path::PathBuf;

use heck::ToUpperCamelCase;
use wit_parser::{Interface, Resolve, Type, TypeDefKind};

const WIT: &str = "../../components/wit";

fn main() {
    println!("cargo:rerun-if-changed={WIT}");

    let mut resolve = Resolve::default();
    resolve.push_path(WIT).expect("WIT must parse");

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::write(out.join("error_code.rs"), error_code(&resolve)).unwrap();
    std::fs::write(out.join("operation.rs"), operation(&resolve)).unwrap();
}

fn interface<'a>(resolve: &'a Resolve, namespace: &str, package: &str, name: &str) -> &'a Interface {
    resolve
        .interfaces
        .iter()
        .map(|(_, interface)| interface)
        .find(|interface| {
            interface.name.as_deref() == Some(name)
                && interface.package.is_some_and(|id| {
                    let package_name = &resolve.packages[id].name;
                    package_name.namespace == namespace && package_name.name == package
                })
        })
        .unwrap_or_else(|| panic!("{namespace}:{package}/{name} must be in the WIT"))
}

fn error_code(resolve: &Resolve) -> String {
    let types = interface(resolve, "wasi", "filesystem", "types");
    let error_code = types.types["error-code"];
    let TypeDefKind::Variant(variant) = &resolve.types[error_code].kind else {
        panic!("wasi:filesystem/types#error-code must be a variant");
    };

    let mut arms = String::new();
    for case in &variant.cases {
        match (&case.ty, case.name.as_str()) {
            (None, name) => {
                // the case name as wit-bindgen generates it
                let ident = name.to_upper_camel_case();
                writeln!(arms, "        {name:?} => Some(FilesystemErrorCode::{ident}),").unwrap();
            }
            // `other` carries a message, it is parsed by hand
            (Some(_), "other") => {}
            (Some(_), name) => panic!(
                "error-code case `{name}` has a payload, extend build.rs to parse it"
            ),
        }
    }

    format!(
        "/// The `error-code` case without a payload with the kebab-case name, e.g. `not-permitted`.\n\
         pub fn error_code_from_name(name: &str) -> Option<FilesystemErrorCode> {{\n    \
             match name {{\n{arms}        _ => None,\n    }}\n\
         }}\n"
    )
}

/// The latch's `operation` variant has a case for each interface, its payload is a variant with a
/// case for each operation of the interface, e.g. `operation::descriptor(descriptor-operation::open-at)`.
fn operation(resolve: &Resolve) -> String {
    let latch = interface(resolve, "componentized", "filesystem", "latch");
    let TypeDefKind::Variant(operation) = &resolve.types[latch.types["operation"]].kind else {
        panic!("componentized:filesystem/latch#operation must be a variant");
    };

    let mut interfaces = vec![];
    let mut operations = vec![];
    let mut arms = String::new();
    for interface_case in &operation.cases {
        let Some(Type::Id(id)) = interface_case.ty else {
            panic!("operation case `{}` must have a variant payload", interface_case.name);
        };
        let typedef = &resolve.types[id];
        let TypeDefKind::Variant(variant) = &typedef.kind else {
            panic!("operation case `{}` must have a variant payload", interface_case.name);
        };
        let interface_name = &interface_case.name;
        let interface_ident = interface_name.to_upper_camel_case();
        let variant_ident = typedef.name.as_deref().expect("a named variant").to_upper_camel_case();
        interfaces.push(format!("{interface_name:?}"));
        writeln!(arms, "        Operation::{interface_ident}(operation) => match operation {{").unwrap();
        for case in &variant.cases {
            let name = format!("{interface_name}.{}", case.name);
            let ident = case.name.to_upper_camel_case();
            let payload = if case.ty.is_some() { "(_)" } else { "" };
            writeln!(arms, "            {variant_ident}::{ident}{payload} => {name:?},").unwrap();
            operations.push(format!("{name:?}"));
        }
        writeln!(arms, "        }},").unwrap();
    }

    format!(
        "/// The name of the operation, the interface and operation cases of the latch WIT, e.g.\n\
         /// `descriptor.open-at`.\n\
         pub fn operation_name(operation: &Operation) -> &'static str {{\n    \
             match operation {{\n{arms}    }}\n\
         }}\n\n\
         /// The name of every interface of the operations, e.g. `descriptor`.\n\
         pub const INTERFACE_NAMES: &[&str] = &[{interfaces}];\n\n\
         /// The name of every operation, e.g. `descriptor.open-at`.\n\
         pub const OPERATION_NAMES: &[&str] = &[{operations}];\n",
        interfaces = interfaces.join(", "),
        operations = operations.join(", "),
    )
}
