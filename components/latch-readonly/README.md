# `latch-readonly`

Filesystem latch that denies operations which would mutate the filesystem.

`latch-path-glob` composed with [`latch-readonly-config`](../latch-readonly-config/). Operations that would change the filesystem are denied with `read-only`, every other operation is deferred. See [`latch-path-glob`](../latch-path-glob/) for the operations that change the filesystem.

## Interfaces

Imports:

- `wasi:logging/logging@0.1.0-draft`: logs an invalid config
- `wasi:filesystem/types@0.3.0`: the descriptor types of the operations being authorized
- `wasi:clocks/system-clock@0.3.0`: the timestamp types of the operations being authorized

Exports:

- `componentized:filesystem/latch@0.3.0-dev`: the latch
