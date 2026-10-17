# `chroot`

Virtualizes the `wasi:filesystem` interfaces limiting all access to a specific directory defined by the 'path' key in a `wasi:config/store`, mimicking the GNU chroot command.

The chroot is exported as a single preopened directory, `.`, a virtual scope rather than a root, since a wasi preopen need not be rooted. The `path` is a path within one of the imported preopens, the preopen whose name is the longest prefix of the path, e.g. `/srv/jail` with a preopen `/` is the directory `srv/jail` of that preopen. The path must not contain `..`. The directory is opened by the first operation on the root that needs it, an operation on the root fails when the directory doesn't exist.

Paths are resolved like a GNU chroot:

- an absolute path is resolved from the root of the chroot, the exported `.`, from any descriptor
- `..` of the root is the root, e.g. `/../../etc/passwd` is `/etc/passwd` within the chroot
- `..` is resolved lexically before the path is given to the host, `a/b/..` is `a` even when `b` is a symlink
- a descriptor below the root doesn't know where it is, it may have been moved, so like any wasi descriptor a path naming one of its parents, e.g. `../file`, fails with `not-permitted`
- a path containing a NUL byte fails with `illegal-byte-sequence`

Symlinks are followed by the host, which confines them to the descriptor they are followed from, a symlink leading out of the chroot can be created and read, but not followed. A symlink with an absolute target is not followed, it would be resolved from the host's root rather than the chroot's.

The synchronous `read-via-stream`, `write-via-stream`, `append-via-stream` and `read-directory` can't wait for the root to be opened, until it is they run on a background thread and their results are forwarded, so the host must support component model threads, e.g. `wasmtime -W component-model-threading`.
