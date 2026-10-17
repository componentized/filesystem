//! The physical path of a descriptor, for its label in the trace.
//!
//! A descriptor opened with `open-at` is labeled with the path of the directory or file it is, the
//! path `PATH` arguments of later calls are resolved against: `.` and `..` are applied, and
//! symlinks are followed where the host follows them, every component but the last, and the last
//! with `symlink-follow`.

/// The most symlinks followed resolving one path, more is a loop, see `ELOOP` in POSIX.
const MAX_SYMLINKS: usize = 40;

/// Where a descriptor is, the preopen it was opened from and the components of its path within it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    preopen: String,
    components: Vec<String>,
}

impl Location {
    /// The location of a preopen, labeled with its name, e.g. `/` or `.`.
    pub fn preopen(name: &str) -> Self {
        Self {
            preopen: name.to_string(),
            components: vec![],
        }
    }

    /// Resolve a path opened from this location.
    ///
    /// `readlink` reads a symlink at a path relative to this location, `None` when the path is not
    /// a symlink. The host already opened the path, so it names a location below this one; a
    /// symlink with an absolute target, which the host doesn't follow, is left unresolved.
    pub async fn open(
        &self,
        path: &str,
        follow: bool,
        mut readlink: impl AsyncFnMut(&str) -> Option<String>,
    ) -> Self {
        // components relative to this location, still to be resolved, and resolved
        let mut pending: Vec<String> = split(path).rev().map(String::from).collect();
        let mut resolved: Vec<String> = vec![];
        let mut symlinks = 0;
        while let Some(component) = pending.pop() {
            match component.as_str() {
                "." => {}
                ".." => {
                    resolved.pop();
                }
                _ => {
                    resolved.push(component);
                    // the last component is only followed with `symlink-follow`
                    if (!pending.is_empty() || follow) && symlinks < MAX_SYMLINKS {
                        if let Some(target) = readlink(&resolved.join("/")).await {
                            if !target.starts_with('/') {
                                symlinks += 1;
                                // the target is relative to the directory containing the link
                                resolved.pop();
                                pending.extend(split(&target).rev().map(String::from));
                            }
                        }
                    }
                }
            }
        }
        let mut components = self.components.clone();
        components.extend(resolved);
        Self {
            preopen: self.preopen.clone(),
            components,
        }
    }
}

/// The components of a path, without empty components from repeated or trailing separators.
fn split(path: &str) -> impl DoubleEndedIterator<Item = &str> {
    path.split('/').filter(|component| !component.is_empty())
}

impl std::fmt::Display for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.preopen)?;
        for (index, component) in self.components.iter().enumerate() {
            if index > 0 || !self.preopen.ends_with('/') {
                f.write_str("/")?;
            }
            f.write_str(component)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use super::*;

    /// Run a future that never waits, as the fake readlink never does.
    fn now<T>(future: impl Future<Output = T>) -> T {
        match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("the future waited"),
        }
    }

    /// Open `path` from `from`, with the symlinks of a filesystem, by their path relative to `from`.
    fn open(from: &Location, path: &str, follow: bool, links: &[(&str, &str)]) -> String {
        let links: HashMap<&str, &str> = links.iter().copied().collect();
        let location = now(from.open(path, follow, async |path: &str| {
            links.get(path).map(|target| target.to_string())
        }));
        location.to_string()
    }

    fn root() -> Location {
        Location::preopen("/")
    }

    #[test]
    fn preopens_are_labeled_with_their_name() {
        assert_eq!(Location::preopen("/").to_string(), "/");
        assert_eq!(Location::preopen(".").to_string(), ".");
        assert_eq!(Location::preopen("/data").to_string(), "/data");
    }

    #[test]
    fn relative_paths_are_joined() {
        assert_eq!(open(&root(), "dir", true, &[]), "/dir");
        assert_eq!(open(&root(), "dir/sub", true, &[]), "/dir/sub");
        let data = Location::preopen("/data");
        assert_eq!(open(&data, "dir", true, &[]), "/data/dir");
        let dot = Location::preopen(".");
        assert_eq!(open(&dot, "dir/sub", true, &[]), "./dir/sub");
    }

    #[test]
    fn dots_and_separators_are_resolved() {
        assert_eq!(open(&root(), ".", true, &[]), "/");
        assert_eq!(open(&root(), "./dir/./sub/.", true, &[]), "/dir/sub");
        assert_eq!(open(&root(), "dir//sub/", true, &[]), "/dir/sub");
        assert_eq!(open(&root(), "dir/sub/..", true, &[]), "/dir");
        assert_eq!(open(&root(), "dir/../other", true, &[]), "/other");
        assert_eq!(open(&root(), "dir/sub/../..", true, &[]), "/");
    }

    #[test]
    fn paths_resolve_from_the_descriptor() {
        let dir = now(root().open("dir", true, async |_: &str| None));
        assert_eq!(open(&dir, "sub", true, &[]), "/dir/sub");
        assert_eq!(open(&dir, ".", true, &[]), "/dir");
        assert_eq!(open(&dir, "sub/..", true, &[]), "/dir");
    }

    #[test]
    fn symlinks_are_followed() {
        let links = [("link", "dir")];
        assert_eq!(open(&root(), "link", true, &links), "/dir");
        assert_eq!(open(&root(), "link/sub", true, &links), "/dir/sub");
        // `..` of the link's target, not of the directory containing the link
        let links = [("link", "dir/sub")];
        assert_eq!(open(&root(), "link/..", true, &links), "/dir");
    }

    #[test]
    fn the_last_symlink_is_only_followed_with_the_flag() {
        let links = [("link", "dir")];
        assert_eq!(open(&root(), "link", false, &links), "/link");
        // intermediate symlinks are always followed
        assert_eq!(open(&root(), "link/sub", false, &links), "/dir/sub");
    }

    #[test]
    fn symlink_targets_are_relative_to_the_link() {
        let links = [("dir/up", "../other")];
        assert_eq!(open(&root(), "dir/up", true, &links), "/other");
        let links = [("dir/self", ".")];
        assert_eq!(open(&root(), "dir/self/sub", true, &links), "/dir/sub");
        let links = [("dir/sub-link", "sub/")];
        assert_eq!(open(&root(), "dir/sub-link", true, &links), "/dir/sub");
    }

    #[test]
    fn chained_symlinks_are_followed() {
        let links = [
            ("first", "second"),
            ("second", "dir/third"),
            ("dir/third", "../target"),
        ];
        assert_eq!(open(&root(), "first", true, &links), "/target");
        assert_eq!(
            open(&root(), "first/file.txt", true, &links),
            "/target/file.txt"
        );
    }

    #[test]
    fn symlinks_resolve_from_the_descriptor() {
        let dir = now(root().open("dir", true, async |_: &str| None));
        // readlink is given paths relative to the descriptor
        let links = [("link", "sub")];
        assert_eq!(open(&dir, "link", true, &links), "/dir/sub");
    }

    #[test]
    fn absolute_symlinks_are_not_followed() {
        let links = [("link", "/etc")];
        assert_eq!(open(&root(), "link", true, &links), "/link");
    }

    #[test]
    fn symlink_loops_end() {
        let links = [("a", "b"), ("b", "a")];
        // the host fails to open a loop, but resolution must still end
        let _ = open(&root(), "a", true, &links);
    }
}
