# `readonly`

Virtualizes the `wasi:filesystem` interfaces making the exported file system read-only.

Every operation that reads the filesystem is passed through, every operation that would change it fails with `read-only`, including opening a descriptor with flags that would allow a change. `sync` and `sync-data` are denied as well.

`write-via-stream` and `append-via-stream` are synchronous and resolve their result on a background thread, so the host must support component model threads, e.g. `wasmtime -W component-model-threading`.
