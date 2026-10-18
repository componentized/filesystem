# `latch-path-glob`

Filesystem latch that uses glob patterns to defer or deny operations on a path.

The patterns are defined in a wasi:config/store. Keys starting with `deny` are parsed as globs with matching paths being denied. Multiple patterns are allowed by defining unique config keys (e.g. `deny-1`, `deny-2`, etc). Keys starting with `defer` are parsed as globs with matching paths deferring the decision.

A single `*` matches within a path segment, between `/` separators, while `**` spans segments.

```
/data/*

/data/file.txt -> MATCHES
/data/dir/file.txt -> NO MATCH
/other/file.txt -> NO MATCH
```

```
/data/**

/data/file.txt -> MATCHES
/data/dir/file.txt -> MATCHES
/other/file.txt -> NO MATCH
```

Operations are evaluated against the patterns with the base path for the operation joined with a relative path, if any. For example, the descriptor `open-at` operation joins the descriptor's path with the argument's path, and the resulting path is matched against the patterns. For operations with multiple paths, like `rename-at`, each path is decided individually and any denied path denies the operation.

When several patterns match a path, the most specific pattern decides, regardless of whether it is a `deny` or `defer` pattern or the order the keys are defined in. Patterns are compared segment by segment starting from the root (`data` in `/data/file.txt`), the first segment that differs decides. Within a segment, a literal (`file.txt`) is more specific than a partial wildcard (`*.txt`), which is more specific than `*`, which is more specific than `**`. Between two partial wildcards, the one with more literal characters is more specific. When the matching patterns are equally specific, `deny` wins.

```
deny-1=/**
defer-1=/data/**
deny-2=/data/secret.txt

/etc/passwd -> DENIED (deny-1)
/data/file.txt -> DEFERRED (defer-1)
/data/secret.txt -> DENIED (deny-2)
```

Whether the host filesystem is case sensitive is not known to the latch, so matching is case insensitive by default: `deny=/secret/**` also denies `/SECRET/file.txt`. Set `case-sensitive=true` when the host filesystem is known to be case sensitive. With case insensitive matching a `defer` pattern also matches other casings, which on a case sensitive filesystem are different paths, e.g. `default=deny` with `defer=/data/**` defers `/DATA/file.txt`.

Paths that do not match any pattern use the `default` decision, either `defer` (the default) or `deny`.

Denied operations fail with `not-permitted` unless the `reason` key is set. Any `wasi:filesystem` error code is supported, e.g. `access` or `read-only`, any other value is reported as `other` with the value as the message. The reason applies to both matching `deny` patterns and a `deny` default, and does not change the default decision.

```
default=deny
reason=access
defer=/data/**

/data/file.txt -> DEFERRED
/etc/passwd -> DENIED (access)
```

If the config is invalid (a pattern that does not parse as a glob, or an unknown `default` or `case-sensitive` value), the cause is logged when the config is loaded and every operation fails with an `invalid-config` latch error.

## Interfaces

Imports:

- `wasi:logging/logging@0.1.0-draft`: logs an invalid config
- `wasi:config/store@0.2.0-rc.1`: the glob patterns
- `wasi:filesystem/types@0.3.0`: the descriptor types of the operations being authorized

Exports:

- `componentized:filesystem/latch@0.3.0-dev`: the latch
