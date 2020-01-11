use std::marker::PhantomData;
use std::fmt;
use std::mem;
use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use futures::future::poll_fn;

use crossbeam::utils::CachePadded;

use std::task::Waker;
use std::task::Context;
use std::task::Poll;

pub mod error {
    use thiserror::Error;
    use std::fmt;
    #[derive(Error, Debug, Clone, Copy, Eq, PartialEq)]
    pub enum PopError {
        #[error("Queue is empty")]
        Empty,
        #[error("Producer disconnected")]
        Disconnect,
    }

    #[derive(Error, Clone, Copy, Eq, PartialEq)]
    #[error("{source}")]
    pub struct PushError<T> {
        pub val: T, 
        source: PushErrorKind,
    }

    impl<T> fmt::Debug for PushError<T> {
        fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
            write!(fmt, "{:?}", self.source)
        }
    }

    impl<T> PushError<T> {
        pub fn disconnected(val: T) -> Self {
            Self {
                val,
                source: PushErrorKind::Disconnected,
            }
        }

        pub fn full(val: T) -> Self {
            Self {
                val,
                source: PushErrorKind::Full,
            }
        }
    }

    #[derive(Error, Debug, Clone, Copy, Eq, PartialEq)]
    pub enum PushErrorKind {
        #[error("Queue is full")]
        Full,
        #[error("Consumer disconnected")]
        Disconnected
    }
}

use error::{PopError, PushError};

/// The inner representation of a single-producer single-consumer queue.
struct Inner<T> {
    /// The head of the queue.
    ///
    /// This integer is in range `0 .. 2 * cap`.
    head: CachePadded<AtomicUsize>,

    /// The tail of the queue.
    ///
    /// This integer is in range `0 .. 2 * cap`.
    tail: CachePadded<AtomicUsize>,

    /// The buffer holding slots.
    buffer: *mut T,

    /// The queue capacity.
    cap: usize,

    /// Waker for Consumer task
    pop_waker: Mutex<Option<Waker>>,

    push_waker: Mutex<Option<Waker>>,

    disconnected: CachePadded<AtomicBool>,

    /// Indicates that dropping a `Buffer<T>` may drop elements of type `T`.
    _marker: PhantomData<T>,
}

impl<T> Inner<T> {
    /// Returns a pointer to the slot at position `pos`.
    ///
    /// The position must be in range `0 .. 2 * cap`.
    #[inline]
    unsafe fn slot(&self, pos: usize) -> *mut T {
        if pos < self.cap {
            self.buffer.add(pos)
        } else {
            self.buffer.add(pos - self.cap)
        }
    }

    /// Increments a position by going one slot forward.
    ///
    /// The position must be in range `0 .. 2 * cap`.
    #[inline]
    fn increment(&self, pos: usize) -> usize {
        if pos < 2 * self.cap - 1 {
            pos + 1
        } else {
            0
        }
    }

    /// Returns the distance between two positions.
    ///
    /// Positions must be in range `0 .. 2 * cap`.
    #[inline]
    fn distance(&self, a: usize, b: usize) -> usize {
        if a <= b {
            b - a
        } else {
            2 * self.cap - a + b
        }
    }
}

impl<T> Drop for Inner<T> {
    fn drop(&mut self) {
        let mut head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);

        // Loop over all slots that hold a value and drop them.
        while head != tail {
            unsafe {
                self.slot(head).drop_in_place();
            }
            head = self.increment(head);
        }

        // Finally, deallocate the buffer, but don't run any destructors.
        unsafe {
            Vec::from_raw_parts(self.buffer, 0, self.cap);
        }
    }
}

/// Creates a bounded single-producer single-consumer queue with the given capacity.
///
/// Returns the producer and the consumer side for the queue.
///
/// # Panics
///
/// Panics if the capacity is zero.
///
/// # Examples
///
/// ```
/// use crossbeam_queue::spsc;
///
/// let (p, c) = spsc::new::<i32>(100);
/// ```
pub fn new<T>(cap: usize) -> (Producer<T>, Consumer<T>) {
    assert!(cap > 0, "capacity must be non-zero");

    // Allocate a buffer of length `cap`.
    let buffer = {
        let mut v = Vec::<T>::with_capacity(cap);
        let ptr = v.as_mut_ptr();
        mem::forget(v);
        ptr
    };

    let inner = Arc::new(Inner {
        head: CachePadded::new(AtomicUsize::new(0)),
        tail: CachePadded::new(AtomicUsize::new(0)),
        buffer,
        cap,
        pop_waker: Mutex::new(None),
        push_waker: Mutex::new(None),
        disconnected: CachePadded::new(AtomicBool::new(false)),
        _marker: PhantomData,
    });

    let p = Producer {
        inner: inner.clone(),
        head: 0,
        tail: 0,
    };

    let c = Consumer {
        inner,
        head: 0,
        tail: 0,
    };

    (p, c)
}

/// The producer side of a bounded single-producer single-consumer queue.
///
/// # Examples
///
/// ```
/// use crossbeam_queue::{spsc, PushError};
///
/// let (p, c) = spsc::new::<i32>(1);
///
/// assert_eq!(p.push(10), Ok(()));
/// assert_eq!(p.push(20), Err(PushError(20)));
/// ```
pub struct Producer<T> {
    /// The inner representation of the queue.
    inner: Arc<Inner<T>>,

    /// A copy of `inner.head` for quick access.
    ///
    /// This value can be stale and sometimes needs to be resynchronized with `inner.head`.
    head: usize,

    /// A copy of `inner.tail` for quick access.
    ///
    /// This value is always in sync with `inner.tail`.
    tail: usize,
}

unsafe impl<T: Send> Send for Producer<T> {}
unsafe impl<T: Sync> Sync for Producer<T> {}

impl<T> Producer<T> {
    /// Attempts to push an element into the queue.
    ///
    /// If the queue is full, the element is returned back as an error.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::{spsc, PushError};
    ///
    /// let (p, c) = spsc::new(1);
    ///
    /// assert_eq!(p.push(10), Ok(()));
    /// assert_eq!(p.push(20), Err(PushError(20)));
    /// ```
    pub fn push(&mut self, value: T) -> Result<(), PushError<T>> {

        // Check if the queue is *possibly* full.
        if self.inner.distance(self.head, self.tail) == self.inner.cap {
            // We need to refresh the head and check again if the queue is *really* full.
            let new_head = self.inner.head.load(Ordering::Acquire);
            self.head = new_head;

            // Is the queue *really* full?
            if self.inner.distance(self.head, self.tail) == self.inner.cap {
                if self.inner.disconnected.load(Ordering::Relaxed) {
                    return Err(PushError::disconnected(value));
                }
                return Err(PushError::full(value));
            }
        }

        // Write the value into the tail slot.
        unsafe {
            self.inner.slot(self.tail).write(value);
        }

        // Move the tail one slot forward.
        let new_tail = self.inner.increment(self.tail);
        self.inner.tail.store(new_tail, Ordering::Release);
        self.tail = new_tail;

        Ok(())
    }

    pub async fn send(&mut self, value: T) -> Result<(), PushError<T>> {
        if poll_fn(|cx| self.poll_ready(cx)).await {
            unsafe { self.push_unchecked(value) };
            Ok(())
        } else {
            Err(PushError::disconnected(value))
        }
        
    }

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<bool> {
        // Check if the queue is *possibly* full.
        if self.inner.distance(self.head, self.tail) == self.inner.cap {
            // We need to refresh the head and check again if the queue is *really* full.
            let new_head = self.inner.head.load(Ordering::Relaxed);
            self.head = new_head;

            // Is the queue *really* full?
            if self.inner.distance(self.head, self.tail) == self.inner.cap {
                if self.inner.disconnected.load(Ordering::Relaxed) {
                    return Poll::Ready(false);
                }
                *self.inner.push_waker.lock().unwrap() = Some(cx.waker().clone());
                return Poll::Pending;
            }
        }
        Poll::Ready(true)
    }

    unsafe fn push_unchecked(&mut self, value: T) {
        self.inner.slot(self.tail).write(value);

        // Move the tail one slot forward.
        let new_tail = self.inner.increment(self.tail);
        self.inner.tail.store(new_tail, Ordering::Relaxed);
        self.tail = new_tail;
    } 

    pub fn poll_send(&mut self, cx: &mut Context<'_>, value: T) -> Poll<Result<(), PushError<T>>> {

        // Check if the queue is *possibly* full.
        if self.inner.distance(self.head, self.tail) == self.inner.cap {
            // We need to refresh the head and check again if the queue is *really* full.
            let new_head = self.inner.head.load(Ordering::Acquire);
            self.head = new_head;

            // Is the queue *really* full?
            if self.inner.distance(self.head, self.tail) == self.inner.cap {
                if self.inner.disconnected.load(Ordering::Relaxed) {
                    return Poll::Ready(Err(PushError::disconnected(value)));
                }
                *self.inner.push_waker.lock().unwrap() = Some(cx.waker().clone());
                return Poll::Pending;
            }
        }

        // Write the value into the tail slot.
        unsafe {
            self.inner.slot(self.tail).write(value);
        }

        // Move the tail one slot forward.
        let new_tail = self.inner.increment(self.tail);
        self.inner.tail.store(new_tail, Ordering::Release);
        self.tail = new_tail;

        Poll::Ready(Ok(()))
    }

    /// Returns the capacity of the queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::spsc;
    ///
    /// let (p, c) = spsc::new::<i32>(100);
    ///
    /// assert_eq!(p.capacity(), 100);
    /// ```
    pub fn capacity(&self) -> usize {
        self.inner.cap
    }

    /// Notify consumer that values are ready
    pub fn notify(&mut self) {
        if let Some(w) = self.inner.pop_waker.lock().unwrap().take() {
            w.wake()
        }
    }
}

impl<T> Drop for Producer<T> {
    fn drop(&mut self) {
        self.inner.disconnected.store(true, Ordering::Relaxed);
        self.notify();
    }
}

impl<T> fmt::Debug for Producer<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("Producer { .. }")
    }
}

/// The consumer side of a bounded single-producer single-consumer queue.
///
/// # Examples
///
/// ```
/// use crossbeam_queue::{spsc, PopError};
///
/// let (p, c) = spsc::new(1);
/// assert_eq!(p.push(10), Ok(()));
///
/// assert_eq!(c.pop(), Ok(10));
/// assert_eq!(c.pop(), Err(PopError));
/// ```
pub struct Consumer<T> {
    /// The inner representation of the queue.
    inner: Arc<Inner<T>>,

    /// A copy of `inner.head` for quick access.
    ///
    /// This value is always in sync with `inner.head`.
    head: usize,

    /// A copy of `inner.tail` for quick access.
    ///
    /// This value can be stale and sometimes needs to be resynchronized with `inner.tail`.
    tail: usize,
}

unsafe impl<T: Send> Send for Consumer<T> {}
unsafe impl<T: Sync> Sync for Consumer<T> {}

impl<T> Consumer<T> {
    /// Attempts to pop an element from the queue.
    ///
    /// If the queue is empty, an error is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::{spsc, PopError};
    ///
    /// let (p, c) = spsc::new(1);
    /// assert_eq!(p.push(10), Ok(()));
    ///
    /// assert_eq!(c.pop(), Ok(10));
    /// assert_eq!(c.pop(), Err(PopError));
    /// ```
    pub fn pop(&mut self) -> Result<T, PopError> {

        // Check if the queue is *possibly* empty.
        if self.head == self.tail {
            // We need to refresh the tail and check again if the queue is *really* empty.
            let new_tail = self.inner.tail.load(Ordering::Acquire);
            self.tail = new_tail;

            // Is the queue *really* empty?
            if self.head == self.tail {
                if self.inner.disconnected.load(Ordering::Relaxed) {
                    return Err(PopError::Disconnect);
                }
                return Err(PopError::Empty);
            }
        }

        // Read the value from the head slot.
        let value = unsafe { self.inner.slot(self.head).read() };

        // Move the head one slot forward.
        let new_head = self.inner.increment(self.head);
        self.inner.head.store(new_head, Ordering::Release);
        self.head = new_head;

        Ok(value)
    }

    /// Returns the capacity of the queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::spsc;
    ///
    /// let (p, c) = spsc::new::<i32>(100);
    ///
    /// assert_eq!(c.capacity(), 100);
    /// ```
    pub fn capacity(&self) -> usize {
        self.inner.cap
    }

    pub async fn recv(&mut self) -> Result<T, PopError> {
        poll_fn(|cx| self.poll_recv(cx)).await
    }

    pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Result<T, PopError>> {

        // Check if the queue is *possibly* empty.
        if self.head == self.tail {
            // We need to refresh the tail and check again if the queue is *really* empty.
            let new_tail = self.inner.tail.load(Ordering::Acquire);
            self.tail = new_tail;

            // Is the queue *really* empty?
            if self.head == self.tail {
                if self.inner.disconnected.load(Ordering::Relaxed) {
                    return Poll::Ready(Err(PopError::Disconnect));
                }
                *self.inner.pop_waker.lock().unwrap() = Some(cx.waker().clone());
                return Poll::Pending;
            }
        }

        // Read the value from the head slot.
        let value = unsafe { self.inner.slot(self.head).read() };

        // Move the head one slot forward.
        let new_head = self.inner.increment(self.head);
        self.inner.head.store(new_head, Ordering::Release);
        self.head = new_head;

        Poll::Ready(Ok(value))
    }

    pub fn notify(&mut self) {
        if let Some(w) = self.inner.push_waker.lock().unwrap().take() {
            w.wake()
        }
    }
}

impl<T> fmt::Debug for Consumer<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("Consumer { .. }")
    }
}
