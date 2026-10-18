# `latch-deny-random`

Filesystem latch that randomly denies operations, to prove a component is resilient to failures.

Every operation is at risk: getting the preopens and each preopen, opening, reading and writing files, reading directories and each directory entry, and every other descriptor operation. A denied operation fails with `access`, or the configured reason, while a denied preopen or directory entry is left out.

The latch is configured with a wasi:config/store:

- `probability`: the fraction of operations denied, from `0` to `1`, defaults to `0.1`
- `probability.<interface>`: the fraction of the operations of an interface denied, `preopens` or `descriptor`, e.g. `probability.preopens`, defaults to `probability`
- `probability.<interface>.<operation>`: the fraction of an operation denied, named as in the latch WIT, e.g. `probability.descriptor.open-at` or `probability.preopens.get-directories-item`, defaults to the probability of its interface
- `seed`: an unsigned 64 bit integer, the same seed denies the same operations, defaults to a random seed from `wasi:random`
- `reason`: the wasi:filesystem `error-code` denied operations fail with, as written in the WIT, e.g. `io` or `no-entry`, defaults to `access`. Any other value is reported as `other` with the value as the message

The most specific probability decides, an unknown interface or operation is an invalid config. The preopens are often configured statically by the host, setting their probability to `0` puts only the guest's own operations at risk:

```
probability=0.2
probability.preopens=0
```

The seed is logged as a warning when the latch starts, set it in the config to reproduce a failure.

```
Randomly denying wasi:filesystem operations PROBABILITY=0.2 SEED=9383211634937261427 PROBABILITY.preopens=0
```

The decision for an operation depends only on the seed and the number of operations observed before it. Operations another latch denies first still count, so aggregating the latch with others does not change which operations it denies.

## Interfaces

Imports:

- `wasi:logging/logging@0.1.0-draft`: logs the seed, and an invalid config
- `wasi:config/store@0.2.0-rc.1`: the latch config
- `wasi:random/insecure@0.3.0`: seeds the decisions when no `seed` is configured
- `wasi:filesystem/types@0.3.0`: the descriptor types of the operations being authorized
- `wasi:clocks/system-clock@0.3.0`: the timestamp types of the operations being authorized

Exports:

- `componentized:filesystem/latch@0.3.0-dev`: the latch
