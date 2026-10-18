//! Generate `error_code_from_name`, which parses the kebab-case name of every case of
//! `wasi:filesystem/types#error-code` without a payload, from the same WIT the bindings are
//! generated from, so a case added to the WIT is parsed without changing the latch.

use std::fmt::Write as _;
use std::path::PathBuf;

use heck::ToUpperCamelCase;
use wit_parser::{Resolve, TypeDefKind};

const WIT: &str = "../wit";

fn main() {
    println!("cargo:rerun-if-changed={WIT}");

    let mut resolve = Resolve::default();
    resolve.push_path(WIT).expect("WIT must parse");

    let (_, types) = resolve
        .interfaces
        .iter()
        .find(|(_, interface)| {
            interface.name.as_deref() == Some("types")
                && interface.package.is_some_and(|package| {
                    let name = &resolve.packages[package].name;
                    name.namespace == "wasi" && name.name == "filesystem"
                })
        })
        .expect("wasi:filesystem/types must be in the WIT");
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

    let source = format!(
        "/// The `error-code` case without a payload with the kebab-case name, e.g. `not-permitted`.\n\
         fn error_code_from_name(name: &str) -> Option<FilesystemErrorCode> {{\n    \
             match name {{\n{arms}        _ => None,\n    }}\n\
         }}\n"
    );
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("error_code.rs");
    std::fs::write(out, source).unwrap();
}
