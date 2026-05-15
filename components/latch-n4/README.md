# `latch-n4`

Fileystem latch that aggregates four other filesystem latches.

## Interfaces

Imports:

- `wasi:filesystem/types@0.3.0`: the socket types of the operations being authorized
- `latch0: componentized:filesystem/latch@0.3.0-dev`: an aggregated latch, consulted in order
- `latch1: componentized:filesystem/latch@0.3.0-dev`: an aggregated latch, consulted in order
- `latch2: componentized:filesystem/latch@0.3.0-dev`: an aggregated latch, consulted in order
- `latch3: componentized:filesystem/latch@0.3.0-dev`: an aggregated latch, consulted in order

Exports:

- `componentized:filesystem/latch@0.3.0-dev`: the latch, aggregating the imported latches
