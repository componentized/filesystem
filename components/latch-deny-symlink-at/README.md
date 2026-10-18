# `latch-deny-symlink-at`

Filesystem latch that denies creating symlinks.

`symlink-at` fails with `not-permitted`, every other operation is deferred. Existing symlinks may still be read, followed, renamed and removed.

Path based latches, like `latch-path-glob`, match the paths an operation is given, not the paths symlinks resolve to. A symlink inside a preopen can reach a path a pattern protects through a path the pattern does not match. Symlinks outside the preopen are not followed by the host. When the preopened directories are known to not contain symlinks, this latch keeps it that way, so the guest can not create a symlink around the patterns.

## Interfaces

Imports:

- `wasi:filesystem/types@0.3.0`: the descriptor types of the operations being authorized
- `wasi:clocks/system-clock@0.3.0`: the timestamp types of the operations being authorized

Exports:

- `componentized:filesystem/latch@0.3.0-dev`: the latch
