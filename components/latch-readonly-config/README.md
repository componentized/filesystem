# `latch-readonly-config`

Config for `latch-path-glob` that makes every path readonly.

Every operation that would change the filesystem is denied with `read-only`, every other operation is deferred.

```properties
readonly=**
```

## Interfaces

Exports:

- `wasi:config/store@0.2.0-rc.1`: the config
