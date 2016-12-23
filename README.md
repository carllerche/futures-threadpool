# futures-threadpool

A library for creating futures representing work happening concurrently on a
dedicated thread pool.

## Usage

First, add this to your `Cargo.toml`:

```toml
[dependencies]
futures = "0.1"
futures-threadpool = "0.1"
```

Next, add this to your crate:

```rust
extern crate futures;
extern crate futures_threadpool;

use futures_threadpool::ThreadPool;
```

# License

`futures-threadpool` is primarily distributed under the terms of both the MIT
license and the Apache License (Version 2.0), with portions covered by various
BSD-like licenses.

See LICENSE-APACHE, and LICENSE-MIT for details.
