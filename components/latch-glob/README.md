# `latch-glob`

Filesystem latch that uses glob patterns to grant or deny operations on a path.

The patterns are defined in a wasi:config/store. Keys starting with `deny` are parsed as globs with matching path operations being denied. Multiple patterns are allowed by defining unique config keys (e.g. `deny-1`, `deny-2`, etc). Keys starting with `grant` are parsed as globs with matching path operations being granted.

Operations are evaluated against the pattern with the base path for the operation joined with a relative path, if any. For example, the file descriptor open-at operation will join the descriptor's path with the argument's path, the resulting path is matched to the patterns.

For operations with multiple paths, each path is evaluated individually. Any path matching a deny pattern will result in a denial.
