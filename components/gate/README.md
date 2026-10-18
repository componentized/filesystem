# `gate`

Filesystem gate access control for preopens and descriptors.

Each operation is authorized by the imported latch before it reaches the filesystem, then the latch is told the final decision with `observe-decision`. Descriptor operations are authorized with the descriptor, its path, and the operation's arguments. The path of a preopen is its name, e.g. `/`, a descriptor opened with `open-at` has the path it was opened at, with `.` and `..` applied and symlinks followed where the host follows them. Testing whether two descriptors are the same object with `is-same-object` is not gated.

How a denial is enforced depends on the operation:

- Most operations fail with the error code the latch denied them with.
- `read-via-stream` and `read-directory` return an empty stream, and the future resolves to the error.
- `write-via-stream` and `append-via-stream` do not read the data stream, and the future resolves to the error.
- Getting the preopens is authorized once, a denial returns no preopens. Each preopen is then authorized individually, a denied preopen is left out and the other preopens are returned.
- Directory entries are authorized individually as they are read. A denied entry is left out of the stream and the read continues with the next entry.

Denials are logged as warnings, except denied preopens and directory entries, which are left out rather than failed and are logged at the trace level:

```
Denied REASON=access OPERATION=wasi:filesystem/types#descriptor.open-at FD=/ PATH-FLAGS=() PATH=file.txt OPEN-FLAGS=(create) FLAGS=(read|write)
Denied REASON=access OPERATION=wasi:filesystem/types#descriptor.read-directory FD=/ NAME=file.txt TYPE=regular-file
```

A latch error, or a failure to observe the decision, fails the operation with `other`, prefixed with `latch-error:`, and is logged as an error. A latch error for a directory entry ends the stream and fails the read. Getting the preopens can not fail, a latch error traps instead.

```
Latch error CODE=invalid-config<latch-path-glob> OPERATION=wasi:filesystem/types#descriptor.stat FD=/file.txt
```

## Interfaces

Imports:

- `componentized:filesystem/latch@0.3.0-dev`: decides whether each operation may proceed
- `wasi:logging/logging@0.1.0-draft`: logs denials and latch errors
- `wasi:clocks/system-clock@0.3.0`: types used by wasi:filesystem/types
- `wasi:filesystem/types@0.3.0`: the descriptors the gated descriptors wrap
- `wasi:filesystem/preopens@0.3.0`: the preopens the gated preopens wrap

Exports:

- `wasi:filesystem/types@0.3.0`: descriptors gated by the latch
- `wasi:filesystem/preopens@0.3.0`: preopens gated by the latch
