# `trace`

Virtualizes the wasi:filesystem interfaces logging all method calls at the TRACE level and 'filesystem' component.

Each call is logged as `CALL <interface>#<function>` with its arguments, e.g. `CALL wasi:filesystem/types#descriptor.stat-at FD=/dir PATH-FLAGS=(symlink-follow) PATH=sub/file.txt`.

`FD` is the path of the descriptor the call is made on, the directory or file a `PATH` argument is resolved against. A preopen is labeled with its name, e.g. `/`, and a descriptor opened with `open-at` with its physical path: `.` and `..` are applied, and symlinks are followed as the host follows them, every component but the last, and the last with `symlink-follow`. Opening `link/sub` from `/`, where `link` is a symlink to `dir`, is labeled `/dir/sub`. The symlinks are read directly from the host, those calls are not logged. `PATH` arguments are logged as given.
