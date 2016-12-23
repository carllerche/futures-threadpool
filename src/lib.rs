//! A simple crate for executing work on a thread pool, and getting back a
//! future.
//!
//! This crate provides a simple thread pool abstraction for running work
//! externally from the current thread that's running. An instance of `Future`
//! is handed back to represent that the work may be done later, and further
//! computations can be chained along with it as well.
//!
//! ```rust
//! extern crate futures;
//! extern crate futures_spawn;
//! extern crate futures_threadpool;
//!
//! use futures::Future;
//! use futures_spawn::SpawnHelper;
//! use futures_threadpool::ThreadPool;
//!
//! # fn long_running_future(a: u32) -> futures::future::BoxFuture<u32, ()> {
//! #     futures::future::result(Ok(a)).boxed()
//! # }
//! # fn main() {
//!
//! // Create a worker thread pool with four threads
//! let pool = ThreadPool::new(4);
//!
//! // Execute some work on the thread pool, optionally closing over data.
//! let a = pool.spawn(long_running_future(2));
//! let b = pool.spawn(long_running_future(100));
//!
//! // Express some further computation once the work is completed on the thread
//! // pool.
//! let c = a.join(b).map(|(a, b)| a + b).wait().unwrap();
//!
//! // Print out the result
//! println!("{:?}", c);
//! # }
//! ```

#![deny(warnings, missing_docs)]

extern crate futures_spawn;
extern crate crossbeam;

#[macro_use]
extern crate futures;
extern crate num_cpus;

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use crossbeam::sync::MsQueue;
use futures::{Future, Poll, Async};
use futures::executor::{self, Run, Executor};
use futures_spawn::Spawn;

/// A thread pool intended to run CPU intensive work.
///
/// This thread pool will hand out futures representing the completed work
/// that happens on the thread pool itself, and the futures can then be later
/// composed with other work as part of an overall computation.
///
/// The worker threads associated with a thread pool are kept alive so long as
/// there is an open handle to the `ThreadPool` or there is work running on them. Once
/// all work has been drained and all references have gone away the worker
/// threads will be shut down.
///
/// Currently `ThreadPool` implements `Clone` which just clones a new reference to
/// the underlying thread pool.
///
/// **Note:** if you use ThreadPool inside a library it's better accept a
/// `Builder` object for thread configuration rather than configuring just
/// pool size.  This not only future proof for other settings but also allows
/// user to attach monitoring tools to lifecycle hooks.
pub struct ThreadPool {
    inner: Arc<Inner>,
}

/// Thread pool configuration object
///
/// Builder starts with a number of workers equal to the number
/// of CPUs on the host. But you can change it until you call `create()`.
pub struct Builder {
    pool_size: usize,
    name_prefix: Option<String>,
    after_start: Option<Arc<Fn() + Send + Sync>>,
    before_stop: Option<Arc<Fn() + Send + Sync>>,
}

struct MySender<F> {
    fut: F,
}

fn _assert() {
    fn _assert_send<T: Send>() {}
    fn _assert_sync<T: Sync>() {}
    _assert_send::<ThreadPool>();
    _assert_sync::<ThreadPool>();
}

struct Inner {
    queue: MsQueue<Message>,
    cnt: AtomicUsize,
    size: usize,
    after_start: Option<Arc<Fn() + Send + Sync>>,
    before_stop: Option<Arc<Fn() + Send + Sync>>,
}

enum Message {
    Run(Run),
    Close,
}

impl ThreadPool {
    /// Creates a new thread pool with `size` worker threads associated with it.
    ///
    /// The returned handle can use `execute` to run work on this thread pool,
    /// and clones can be made of it to get multiple references to the same
    /// thread pool.
    ///
    /// This is a shortcut for:
    /// ```rust
    /// Builder::new().pool_size(size).create()
    /// ```
    pub fn new(size: usize) -> ThreadPool {
        Builder::new().pool_size(size).create()
    }

    /// Creates a new thread pool with a number of workers equal to the number
    /// of CPUs on the host.
    ///
    /// This is a shortcut for:
    /// ```rust
    /// Builder::new().create()
    /// ```
    pub fn new_num_cpus() -> ThreadPool {
        Builder::new().create()
    }
}

impl<T> Spawn<T> for ThreadPool
    where T: Future<Item = (), Error = ()> + Send + 'static,
{
    fn spawn_detached(&self, f: T) {
        // AssertUnwindSafe is used here becuase `Send + 'static` is basically
        // an alias for an implementation of the `UnwindSafe` trait but we can't
        // express that in the standard library right now.
        let f = AssertUnwindSafe(f).catch_unwind();
        let sender = MySender { fut: f };
        executor::spawn(sender).execute(self.inner.clone());
    }
}

fn work(inner: &Inner) {
    inner.after_start.as_ref().map(|fun| fun());
    loop {
        match inner.queue.pop() {
            Message::Run(r) => r.run(),
            Message::Close => break,
        }
    }
    inner.before_stop.as_ref().map(|fun| fun());
}

impl Clone for ThreadPool {
    fn clone(&self) -> ThreadPool {
        self.inner.cnt.fetch_add(1, Ordering::Relaxed);
        ThreadPool { inner: self.inner.clone() }
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        if self.inner.cnt.fetch_sub(1, Ordering::Relaxed) == 1 {
            for _ in 0..self.inner.size {
                self.inner.queue.push(Message::Close);
            }
        }
    }
}

impl Executor for Inner {
    fn execute(&self, run: Run) {
        self.queue.push(Message::Run(run))
    }
}

impl<F: Future> Future for MySender<F> {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<(), ()> {
        match self.fut.poll() {
            Ok(Async::NotReady) => return Ok(Async::NotReady),
            _ => {}
        }

        Ok(Async::Ready(()))
    }
}

impl Builder {
    /// Create a builder a number of workers equal to the number
    /// of CPUs on the host.
    pub fn new() -> Builder {
        Builder {
            pool_size: num_cpus::get(),
            name_prefix: None,
            after_start: None,
            before_stop: None,
        }
    }

    /// Set size of a future ThreadPool
    ///
    /// The size of a thread pool is the number of worker threads spawned
    pub fn pool_size(&mut self, size: usize) -> &mut Self {
        self.pool_size = size;
        self
    }

    /// Set thread name prefix of a future ThreadPool
    ///
    /// Thread name prefix is used for generating thread names. For example, if prefix is
    /// `my-pool-`, then threads in the pool will get names like `my-pool-1` etc.
    pub fn name_prefix<S: Into<String>>(&mut self, name_prefix: S) -> &mut Self {
        self.name_prefix = Some(name_prefix.into());
        self
    }

    /// Execute function `f` right after each thread is started but before
    /// running any jobs on it
    ///
    /// This is initially intended for bookkeeping and monitoring uses
    pub fn after_start<F>(&mut self, f: F) -> &mut Self
        where F: Fn() + Send + Sync + 'static
    {
        self.after_start = Some(Arc::new(f));
        self
    }

    /// Execute function `f` before each worker thread stops
    ///
    /// This is initially intended for bookkeeping and monitoring uses
    pub fn before_stop<F>(&mut self, f: F) -> &mut Self
        where F: Fn() + Send + Sync + 'static
    {
        self.before_stop = Some(Arc::new(f));
        self
    }

    /// Create ThreadPool with configured parameters
    pub fn create(&mut self) -> ThreadPool {
        let pool = ThreadPool {
            inner: Arc::new(Inner {
                queue: MsQueue::new(),
                cnt: AtomicUsize::new(1),
                size: self.pool_size,
                after_start: self.after_start.clone(),
                before_stop: self.before_stop.clone(),
            }),
        };
        assert!(self.pool_size > 0);

        for counter in 0..self.pool_size {
            let inner = pool.inner.clone();
            let mut thread_builder = thread::Builder::new();
            if let Some(ref name_prefix) = self.name_prefix {
                thread_builder = thread_builder.name(format!("{}{}", name_prefix, counter));
            }
            thread_builder.spawn(move || work(&inner)).unwrap();
        }

        return pool
    }
}
