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

#![feature(proc_macro_hygiene, decl_macro)]

#[macro_use]
extern crate diesel;

pub mod api;
pub mod datastore;
pub mod models;
pub mod results;
pub mod schema;

use diesel::pg::PgConnection;
use diesel::prelude::*;
use diesel::result::Error as DieselError;
use failure::Fail;
use log::{error, info, warn};
use results::{Channel, Tx, TxError, TxEvent};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::LinkedList;
use std::env;
use std::fmt::{Debug, Display};
use std::result;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

pub use models::{Event, NewEvent};
pub use rmp_serde as rmp;

#[derive(Debug, Fail)]
#[fail(display = "Out of Sequence: {}", 0)]
pub struct OutOfSequenceError(i64);

#[derive(Debug, Fail)]
pub enum AggregatrixError {
    #[fail(display = "Environment variable {} is missing", 0)]
    Var(String),
    #[fail(display = "Database connection failed: {}", 0)]
    Connection(ConnectionError),
    #[fail(display = "Database error: {}", 0)]
    Database(DieselError),
    #[fail(display = "Event out of sequence: {}", 0)]
    OutOfSequence(OutOfSequenceError),
    #[fail(display = "Tx error: {}", 0)]
    ResultProcessing(TxError),
    #[fail(display = "Could not convert JSON: {}", 0)]
    JsonConversion(String),
    #[fail(display = "Could not connect to ØMQ: {}", 0)]
    ZmqConnect(String),
}

impl From<ConnectionError> for AggregatrixError {
    fn from(error: ConnectionError) -> Self {
        AggregatrixError::Connection(error)
    }
}

impl From<DieselError> for AggregatrixError {
    fn from(error: DieselError) -> Self {
        AggregatrixError::Database(error)
    }
}

impl From<TxError> for AggregatrixError {
    fn from(error: TxError) -> Self {
        AggregatrixError::ResultProcessing(error)
    }
}

impl From<serde_json::Error> for AggregatrixError {
    fn from(error: serde_json::Error) -> Self {
        AggregatrixError::JsonConversion(format!("{}", error))
    }
}

impl From<zmq::Error> for AggregatrixError {
    fn from(error: zmq::Error) -> Self {
        AggregatrixError::ZmqConnect(format!("{}", error))
    }
}

pub type Result<T> = std::result::Result<T, AggregatrixError>;

fn database_url() -> Result<String> {
    env::var("DATABASE_URL").or_else(|_| Err(AggregatrixError::Var("DATABASE_URL".to_string())))
}

fn zmq_url() -> Result<String> {
    env::var("ZMQ_URL").or_else(|_| Err(AggregatrixError::Var("ZMQ_URL".to_string())))
}

fn zmq_client() -> Result<zmq::Socket> {
    let context = zmq::Context::new();
    let socket = context.socket(zmq::SUB)?;
    socket.connect(&zmq_url()?)?;
    Ok(socket)
}

pub trait Aggregatrix {
    type State: Default + Clone + Send + Sync + 'static;
    type Error: Display + Serialize;

    fn dispatch(state: &Self::State, event: &Event) -> result::Result<Value, Self::Error>;
}

pub struct Reactrix<A: Aggregatrix> {
    pub state: A::State,
    pub results: Channel,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ApiResult<T> {
    Ok { data: Option<T> },
    Error { reason: String },
}

fn process_event<A: Aggregatrix>(
    sequence: i64,
    state: &A::State,
    tx: &Tx,
    pg: &PgConnection,
) -> Result<()> {
    use schema::events::dsl;

    info!("Processing event {}", sequence);

    let event = dsl::events
        .filter(dsl::sequence.eq(sequence))
        .first::<Event>(pg)?;

    match A::dispatch(state, &event) {
        Ok(result) => {
            tx.send(TxEvent::Store(
                sequence,
                ApiResult::Ok { data: Some(result) },
            ))?;
        }
        Err(e) => {
            warn!("Event {} error: {}", sequence, e);
            tx.send(TxEvent::Store(
                sequence,
                ApiResult::Error {
                    reason: e.to_string(),
                },
            ))?;
        }
    }

    Ok(())
}

fn init<A: Aggregatrix>(tx: &Tx, pg: &PgConnection) -> Result<(A::State, i64)> {
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

pub fn launch<A: 'static + Aggregatrix + Clone>() -> Result<Reactrix<A>> {
    let pg = PgConnection::establish(&database_url()?)?;

    let queue = Arc::new((Mutex::new(LinkedList::<i64>::new()), Condvar::new()));
    zmq_queue(queue.clone())?;

    let (tx_results, rx_results) = results::launch();
    let tx_results_events = tx_results.clone();

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

    Ok(Reactrix {
        state: server_state,
        results: Arc::new(Mutex::new((tx_results, rx_results))),
    })
}
