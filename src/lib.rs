// This file is part of reactrix.
//
// Copyright 2019-2020 Alexander Dorn
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! # reactrix
//!
//! reactrix is an event sourcing framework with built-in mechanisms for GDPR
//! compliance.

#![allow(clippy::single_component_path_imports)]

#[macro_use]
extern crate diesel;

#[macro_use]
extern crate lazy_static;

mod api;
mod error;
pub mod graphql;
mod model;
mod results;
pub mod schema;

use diesel::pg::PgConnection;
use diesel::prelude::*;
use log::{error, info, warn};
use results::Tx;
use serde::Serialize;
use std::collections::LinkedList;
use std::env;
use std::fmt::Display;
use std::result;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

pub use api::{Api, ApiError, ApiResult, StoredObject};
pub use error::AggregatrixError;
pub use juniper;
pub use model::{Data, Event, NewEvent};
pub use results::{Results, RxEvent, TxEvent};
pub use rmp_serde as rmp;
pub use warp;

type Result<T> = std::result::Result<T, AggregatrixError>;

fn database_url() -> Result<String> {
    env::var("DATABASE_URL").or_else(|_| Err(AggregatrixError::Var("DATABASE_URL".to_string())))
}

fn zmq_url() -> Result<String> {
    env::var("ZMQ_URL").or_else(|_| Err(AggregatrixError::Var("ZMQ_URL".to_string())))
}

fn reactrix_url() -> Result<String> {
    env::var("REACTRIX_URL").or_else(|_| Err(AggregatrixError::Var("REACTRIX_URL".to_string())))
}

/// Create a ØMQ socket in subscriber mode
fn zmq_client() -> Result<zmq::Socket> {
    let context = zmq::Context::new();
    let socket = context.socket(zmq::SUB)?;
    socket.connect(&zmq_url()?)?;
    Ok(socket)
}

/// Central type to implement for materialized view implementations
///
/// `State`: Corresponds to the actual database in a classic, non event sourced
/// application. A reference is passed to `dispatch` for each new event to be
/// processed. Must be wrapped for thread-safety,
/// e.g. `Arc<RwLock<InnerState>>`.
///
/// `Result` and `Error`: Instances of these types get stored as `dispatch`
/// invocation results and can be obtained through the `Results` channel.
///
/// `dispatch`: Gets invoked for each incoming `Event` and should modify `State`
/// accordingly.
pub trait Aggregatrix: 'static {
    type State: Default + Clone + Send + Sync;
    type Result: Clone + Send;
    type Error: Clone + Display + Serialize + Send;

    fn dispatch(state: &Self::State, event: &Event) -> result::Result<Self::Result, Self::Error>;
}

/// Simple compound struct returned by `launch`
///
/// It can be used both for custom implementations for the materialized view
/// server, or passed down to a provided implementation, currently only
/// `graphql::setup`.
#[derive(Clone)]
pub struct AggregatrixState<A: Aggregatrix> {
    pub state: A::State,
    pub results: Results<A>,
    pub api: Api,
}

/// Process a single event from the event store
///
/// The event itself is dispatched to the concrete `Aggregatrix` implementation
/// and the evaluation result stored in the `Results` store.
fn process_event<A: Aggregatrix>(
    sequence: i64,
    state: &A::State,
    tx: &Tx<A>,
    pg: &PgConnection,
) -> Result<()> {
    use schema::events::dsl;

    info!("Processing event {}", sequence);

    let event = dsl::events
        .filter(dsl::sequence.eq(sequence))
        .first::<Event>(pg)?;

    match A::dispatch(state, &event) {
        Ok(result) => {
            if let Err(e) = tx.send(TxEvent::Store(sequence, Ok(result))) {
                return Err(AggregatrixError::ResultProcessing(format!("{:?}", e)));
            }
        }
        Err(e) => {
            warn!("Event {} error: {}", sequence, e);
            if let Err(e) = tx.send(TxEvent::Store(sequence, Err(e))) {
                return Err(AggregatrixError::ResultProcessing(format!("{:?}", e)));
            }
        }
    }

    Ok(())
}

/// Initialize `Aggregatrix` state from the current state of the event store
fn init<A: Aggregatrix>(tx: &Tx<A>, pg: &PgConnection) -> Result<(A::State, i64)> {
    use schema::events::dsl::*;

    let ids = events
        .select(sequence)
        .order(sequence.asc())
        .load::<i64>(pg)?;

    let state = A::State::default();

    for id in ids.iter() {
        process_event::<A>(*id, &state, tx, pg)?;
    }

    Ok((state, *ids.last().unwrap_or(&0)))
}

/// Start the ØMQ event queue
///
/// Iterates over events coming from the `sequence` channel and pushes them into
/// the provided queue. Notifies the receiving end through the `Condvar`.
fn zmq_queue(queue: Arc<(Mutex<LinkedList<i64>>, Condvar)>) -> Result<()> {
    let socket = zmq_client()?;
    socket.set_subscribe(b"sequence")?;

    thread::spawn(move || {
        let (queue, condvar) = &*queue;
        loop {
            let _topic = match socket.recv_bytes(0) {
                Ok(bytes) => bytes,
                Err(e) => {
                    warn!("{:?}", e);
                    continue;
                }
            };

            let data = match socket.recv_bytes(0) {
                Ok(bytes) => bytes,
                Err(e) => {
                    error!("{:?}", e);
                    continue;
                }
            };

            match rmp::from_read_ref(&data) {
                Ok(id) => match queue.lock() {
                    Ok(mut queue) => {
                        queue.push_back(id);
                        condvar.notify_one();
                    }
                    Err(e) => {
                        error!("Couldn't lock mutex: {}", e);
                        return;
                    }
                },
                Err(e) => {
                    error!("{:?}", e);
                }
            }
        }
    });

    Ok(())
}

/// Start all reactrix subsystems and return initial state
///
/// The subsystems include a ØMQ queue talking to the event store, a
/// complementary queue consumer thread which triggers event processing and the
/// `Results` channel server.
pub fn launch<A: Aggregatrix>() -> Result<AggregatrixState<A>> {
    let api = Api::new(&reactrix_url()?)?;
    let pg = PgConnection::establish(&database_url()?)?;

    let queue = Arc::new((Mutex::new(LinkedList::<i64>::new()), Condvar::new()));
    zmq_queue(queue.clone())?;

    let results = results::launch::<A>();
    let tx_results_events = results.channel.lock().unwrap().0.clone();

    let (state, mut sequence) = init::<A>(&tx_results_events, &pg)?;
    let server_state = state.clone();

    thread::spawn(move || {
        let (queue, condvar) = &*queue;

        loop {
            let mut queue = queue.lock().unwrap();
            if queue.len() == 0 {
                queue = condvar.wait(queue).unwrap();
            }

            if let Some(id) = queue.pop_front() {
                drop(queue);
                if id > sequence {
                    process_event::<A>(id, &state, &tx_results_events, &pg).unwrap();
                    sequence = id;
                }
            }
        }
    });

    Ok(AggregatrixState {
        state: server_state,
        results,
        api,
    })
}
