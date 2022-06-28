// mod bump_port;
mod async_peek_stream;
mod clone_stream;
mod eventual;
mod eventual_base;
mod eventual_value;
mod eventual_value_clone;
mod ip_addr_port;
mod ip_extra;
mod log_thru;
mod must_join_handle;
mod must_join_single_future;
mod mutable_future;
// mod single_future;
mod single_shot_eventual;
mod split_url;
mod tick_task;
mod tools;

pub use cfg_if::*;
pub use futures_util::future::{select, Either};
pub use futures_util::select;
pub use futures_util::stream::FuturesUnordered;
pub use futures_util::{AsyncRead, AsyncWrite};
pub use log_thru::*;
pub use owo_colors::OwoColorize;
pub use parking_lot::*;
pub use split_url::*;
pub use static_assertions::*;
pub use stop_token::*;
pub use tracing::*;

pub type PinBox<T> = Pin<Box<T>>;
pub type PinBoxFuture<T> = PinBox<dyn Future<Output = T> + 'static>;
pub type PinBoxFutureLifetime<'a, T> = PinBox<dyn Future<Output = T> + 'a>;
pub type SendPinBoxFuture<T> = PinBox<dyn Future<Output = T> + Send + 'static>;
pub type SendPinBoxFutureLifetime<'a, T> = PinBox<dyn Future<Output = T> + Send + 'a>;

cfg_if! {
    if #[cfg(target_arch = "wasm32")] {
        pub use alloc::string::String;
        pub use alloc::vec::Vec;
        pub use alloc::collections::LinkedList;
        pub use alloc::collections::VecDeque;
        pub use alloc::collections::btree_map::BTreeMap;
        pub use alloc::collections::btree_set::BTreeSet;
        pub use hashbrown::hash_map::HashMap;
        pub use hashbrown::hash_set::HashSet;
        pub use alloc::boxed::Box;
        pub use alloc::borrow::{Cow, ToOwned};
        pub use wasm_bindgen::prelude::*;
        pub use core::cmp;
        pub use core::convert::{TryFrom, TryInto};
        pub use core::mem;
        pub use core::fmt;
        pub use alloc::rc::Rc;
        pub use core::cell::RefCell;
        pub use core::task;
        pub use core::future::Future;
        pub use core::time::Duration;
        pub use core::pin::Pin;
        pub use core::sync::atomic::{Ordering, AtomicBool};
        pub use alloc::sync::{Arc, Weak};
        pub use core::ops::{FnOnce, FnMut, Fn};
        pub use async_lock::Mutex as AsyncMutex;
        pub use async_lock::MutexGuard as AsyncMutexGuard;
        pub use no_std_net::{ SocketAddr, SocketAddrV4, SocketAddrV6, ToSocketAddrs, IpAddr, Ipv4Addr, Ipv6Addr };
        pub type SystemPinBoxFuture<T> = PinBox<dyn Future<Output = T> + 'static>;
        pub type SystemPinBoxFutureLifetime<'a, T> = PinBox<dyn Future<Output = T> + 'a>;
        pub use async_executors::JoinHandle as LowLevelJoinHandle;
    } else {
        pub use std::string::String;
        pub use std::vec::Vec;
        pub use std::collections::LinkedList;
        pub use std::collections::VecDeque;
        pub use std::collections::btree_map::BTreeMap;
        pub use std::collections::btree_set::BTreeSet;
        pub use std::collections::hash_map::HashMap;
        pub use std::collections::hash_set::HashSet;
        pub use std::boxed::Box;
        pub use std::borrow::{Cow, ToOwned};
        pub use std::cmp;
        pub use std::convert::{TryFrom, TryInto};
        pub use std::mem;
        pub use std::fmt;
        pub use std::sync::atomic::{Ordering, AtomicBool};
        pub use std::sync::{Arc, Weak};
        pub use std::rc::Rc;
        pub use std::cell::RefCell;
        pub use std::task;
        pub use std::future::Future;
        pub use std::time::Duration;
        pub use std::pin::Pin;
        pub use std::ops::{FnOnce, FnMut, Fn};
        cfg_if! {
            if #[cfg(feature="rt-async-std")] {
                pub use async_std::sync::Mutex as AsyncMutex;
                pub use async_std::sync::MutexGuard as AsyncMutexGuard;
                pub use async_std::task::JoinHandle as LowLevelJoinHandle;
            } else if #[cfg(feature="rt-tokio")] {
                pub use tokio::sync::Mutex as AsyncMutex;
                pub use tokio::sync::MutexGuard as AsyncMutexGuard;
                pub use tokio::task::JoinHandle as LowLevelJoinHandle;
            }
        }
        pub use std::net::{ SocketAddr, SocketAddrV4, SocketAddrV6, ToSocketAddrs, IpAddr, Ipv4Addr, Ipv6Addr };
        pub type SystemPinBoxFuture<T> = PinBox<dyn Future<Output = T> + Send + 'static>;
        pub type SystemPinBoxFutureLifetime<'a, T> = PinBox<dyn Future<Output = T> + Send + 'a>;
    }
}

// pub use bump_port::*;
pub use async_peek_stream::*;
pub use clone_stream::*;
pub use eventual::*;
pub use eventual_base::{EventualCommon, EventualResolvedFuture};
pub use eventual_value::*;
pub use eventual_value_clone::*;
pub use ip_addr_port::*;
pub use ip_extra::*;
pub use must_join_handle::*;
pub use must_join_single_future::*;
pub use mutable_future::*;
// pub use single_future::*;
pub use single_shot_eventual::*;
pub use tick_task::*;
pub use tools::*;
