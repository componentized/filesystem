//! Lexical path resolution for the chroot, independent of the filesystem.
//!
//! Every path the guest passes is normalized before it reaches the host, so no path given to the
//! host contains a `..` or is absolute. The host's descriptors confine symlinks, which can't be
//! resolved here without racing the filesystem.

/// The descriptor a path is resolved relative to.
#[derive(Debug, PartialEq, Eq)]
pub enum Base {
    /// The root of the chroot.
    Root,
    /// The descriptor the path was given to.
    Descriptor,
}

/// Why a path can't be resolved.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// The path is empty.
    Empty,
    /// The path names a parent of a descriptor that is not the root of the chroot.
    Escape,
    /// The path contains a NUL byte, which ends the path for C, but not for wasi.
    Nul,
}

/// Resolve a path given to a descriptor into a path relative to `Base`.
///
/// Like the root of a GNU chroot, the parent of the root is the root itself. An absolute path is
/// resolved from the root, as is a relative path given to a descriptor at the root, and `..` never
/// goes above the root. A descriptor below the root doesn't know where it is, it may have been moved,
/// so like any wasi descriptor, a path naming one of its parents is an error.
///
/// `..` is resolved lexically, `a/b/..` is `a` even when `b` is a symlink.
pub fn resolve(path: &str, at_root: bool) -> Result<(Base, String), Error> {
    if path.is_empty() {
        return Err(Error::Empty);
    }
    if path.contains('\0') {
        return Err(Error::Nul);
    }
    let absolute = path.starts_with('/');
    let base = if absolute || at_root {
        Base::Root
    } else {
        Base::Descriptor
    };

    let mut components: Vec<&str> = vec![];
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if components.pop().is_none() && base == Base::Descriptor {
                    return Err(Error::Escape);
                }
            }
            component => components.push(component),
        }
    }

    if components.is_empty() {
        return Ok((base, String::from(".")));
    }
    let mut resolved = components.join("/");
    // a trailing slash, `.` or `..` requires the path to be a directory
    if path.ends_with('/') || path.ends_with("/.") || path.ends_with("/..") {
        resolved.push('/');
    }
    Ok((base, resolved))
}

/// Split the configured path of the chroot into its components, it must not contain `..`.
pub fn config_components(path: &str) -> Option<Vec<&str>> {
    let mut components = vec![];
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => return None,
            component => components.push(component),
        }
    }
    Some(components)
}

/// Find the preopen containing the chroot, the preopen with the longest name the configured path
/// starts with. Returns the index of the preopen and the path of the chroot within it.
pub fn find_preopen<'a>(config: &[&str], preopens: &[&'a str]) -> Option<(usize, String)> {
    preopens
        .iter()
        .enumerate()
        .filter_map(|(index, name)| {
            // `/` and `.` both name the root
            let name = config_components(name)?;
            config.starts_with(&name).then(|| (index, name.len()))
        })
        .max_by_key(|(_, len)| *len)
        .map(|(index, len)| {
            let path = config[len..].join("/");
            (index, if path.is_empty() { ".".into() } else { path })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(path: &str) -> Result<(Base, String), Error> {
        resolve(path, true)
    }

    fn child(path: &str) -> Result<(Base, String), Error> {
        resolve(path, false)
    }

    fn ok(base: Base, path: &str) -> Result<(Base, String), Error> {
        Ok((base, path.into()))
    }

    #[test]
    fn plain_paths() {
        assert_eq!(root("a"), ok(Base::Root, "a"));
        assert_eq!(root("a/b/c.txt"), ok(Base::Root, "a/b/c.txt"));
        assert_eq!(child("a/b"), ok(Base::Descriptor, "a/b"));
    }

    #[test]
    fn empty_path() {
        assert_eq!(root(""), Err(Error::Empty));
        assert_eq!(child(""), Err(Error::Empty));
    }

    #[test]
    fn root_itself() {
        for path in ["/", ".", "./", "//", "/.", "/./.", ".."] {
            assert_eq!(root(path), ok(Base::Root, "."), "{path}");
        }
        assert_eq!(child("."), ok(Base::Descriptor, "."));
        assert_eq!(child("/"), ok(Base::Root, "."));
    }

    #[test]
    fn redundant_separators_and_dots() {
        assert_eq!(root("a//b"), ok(Base::Root, "a/b"));
        assert_eq!(root("./a/./b"), ok(Base::Root, "a/b"));
        assert_eq!(root("a/b/../c"), ok(Base::Root, "a/c"));
        assert_eq!(child("a/../b"), ok(Base::Descriptor, "b"));
    }

    #[test]
    fn absolute_paths_resolve_from_root() {
        assert_eq!(root("/a/b"), ok(Base::Root, "a/b"));
        assert_eq!(child("/a/b"), ok(Base::Root, "a/b"));
        assert_eq!(child("///a"), ok(Base::Root, "a"));
    }

    #[test]
    fn parent_of_root_is_root() {
        assert_eq!(root("../a"), ok(Base::Root, "a"));
        assert_eq!(root("../../../../a"), ok(Base::Root, "a"));
        assert_eq!(root("/../a"), ok(Base::Root, "a"));
        assert_eq!(root("a/../../b"), ok(Base::Root, "b"));
        assert_eq!(root("a/b/../../../../c"), ok(Base::Root, "c"));
        assert_eq!(child("/../../etc/passwd"), ok(Base::Root, "etc/passwd"));
    }

    #[test]
    fn parent_of_child_is_escape() {
        assert_eq!(child(".."), Err(Error::Escape));
        assert_eq!(child("../a"), Err(Error::Escape));
        assert_eq!(child("a/../../b"), Err(Error::Escape));
        assert_eq!(child("./.."), Err(Error::Escape));
    }

    #[test]
    fn trailing_directory_markers_are_kept() {
        assert_eq!(root("a/"), ok(Base::Root, "a/"));
        assert_eq!(root("a/."), ok(Base::Root, "a/"));
        assert_eq!(root("a/b/.."), ok(Base::Root, "a/"));
        assert_eq!(root("a//"), ok(Base::Root, "a/"));
    }

    #[test]
    fn no_resolved_path_is_absolute_or_has_parents() {
        let paths = [
            "/",
            "..",
            "../",
            "/..",
            "a/..",
            "../a/../..",
            "//..//a",
            "a/./../..",
            "/a/b/../../..",
        ];
        for path in paths {
            let (_, resolved) = root(path).unwrap();
            assert!(!resolved.starts_with('/'), "{path} -> {resolved}");
            assert!(
                !resolved.split('/').any(|c| c == ".."),
                "{path} -> {resolved}"
            );
        }
    }

    #[test]
    fn other_bytes_are_not_separators() {
        // only `/` separates components in wasi, the host rejects what it can't name
        assert_eq!(root("..\\a"), ok(Base::Root, "..\\a"));
        assert_eq!(root("..."), ok(Base::Root, "..."));
        assert_eq!(root(".. /a"), ok(Base::Root, ".. /a"));
    }

    #[test]
    fn nul_bytes_are_rejected() {
        // `a\0/../b` is `a` to C, but `b` once resolved
        assert_eq!(root("a\0/../b"), Err(Error::Nul));
        assert_eq!(root("a\0"), Err(Error::Nul));
        assert_eq!(child("\0"), Err(Error::Nul));
    }

    #[test]
    fn config_rejects_parents() {
        assert_eq!(config_components("jail"), Some(vec!["jail"]));
        assert_eq!(config_components("/srv/./jail/"), Some(vec!["srv", "jail"]));
        assert_eq!(config_components("/"), Some(vec![]));
        assert_eq!(config_components(".."), None);
        assert_eq!(config_components("jail/../.."), None);
        assert_eq!(config_components("/srv/../etc"), None);
    }

    #[test]
    fn preopen_is_longest_match() {
        let find = |config: &str, preopens: &[&str]| {
            find_preopen(&config_components(config).unwrap(), preopens)
        };
        assert_eq!(find("jail", &["/"]), Some((0, "jail".into())));
        assert_eq!(find("/jail", &["."]), Some((0, "jail".into())));
        assert_eq!(
            find("/data/jail", &["/", "/data"]),
            Some((1, "jail".into()))
        );
        assert_eq!(
            find("/data/jail", &["/data", "/"]),
            Some((0, "jail".into()))
        );
        assert_eq!(find("/data", &["/", "/data"]), Some((1, ".".into())));
        // a preopen matches whole components, `/data` doesn't contain `/database`
        assert_eq!(find("/database/jail", &["/data"]), None);
        assert_eq!(find("/jail", &["/data"]), None);
        assert_eq!(find("/jail", &[]), None);
        // a preopen named with `..` is never used
        assert_eq!(find("/jail", &["/..", "/"]), Some((1, "jail".into())));
    }
}
