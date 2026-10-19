# `latch-deny-all`

Filesystem latch that implicitly denies all operations.

Every operation is denied with `access`. Getting the preopens returns no preopens, so no descriptor can be obtained.
