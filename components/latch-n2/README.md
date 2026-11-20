# `latch-n2`

Filesystem latch that aggregates two other filesystem latches.

## Interfaces

Imports:

- `wasi:filesystem/types@0.3.0`: the socket types of the operations being authorized
- `latch0: componentized:filesystem/latch@0.3.0`: an aggregated latch, consulted in order
- `latch1: componentized:filesystem/latch@0.3.0`: an aggregated latch, consulted in order

Exports:

- `componentized:filesystem/latch@0.3.0`: the latch, aggregating the imported latches
