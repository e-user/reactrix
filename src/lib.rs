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

mod api;
mod datastore;
pub mod models;
pub mod results;
pub mod schema;
pub mod server;

use diesel::pg::PgConnection;
use diesel::prelude::*;
use diesel::result::Error as DieselError;
use failure::Fail;
use juniper::{GraphQLType, RootNode};
use log::{error, info, warn};
use results::{Results, Tx, TxEvent};
use serde::Serialize;
use std::collections::LinkedList;
use std::env;
use std::fmt::{Debug, Display};
use std::result;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use warp::filters::BoxedFilter;
use warp::Reply;

pub use api::{Api, ApiError, ApiResult, StoredObject};
pub use datastore::{DataStore, DataStoreError};
pub use juniper;
pub use models::{Event, NewEvent};
pub use rmp_serde as rmp;
pub use warp::serve;

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
    ResultProcessing(String),
    #[fail(display = "Could not convert JSON: {}", 0)]
    JsonConversion(String),
    #[fail(display = "Could not connect to ØMQ: {}", 0)]
    ZmqConnect(String),
    #[fail(display = "Could not parse URL: {}", 0)]
    UrlParse(url::ParseError),
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

impl From<url::ParseError> for AggregatrixError {
    fn from(error: url::ParseError) -> Self {
        AggregatrixError::UrlParse(error)
    }
}

pub type Result<T> = std::result::Result<T, AggregatrixError>;

fn database_url() -> Result<String> {
    env::var("DATABASE_URL").or_else(|_| Err(AggregatrixError::Var("DATABASE_URL".to_string())))
}

fn zmq_url() -> Result<String> {
    env::var("ZMQ_URL").or_else(|_| Err(AggregatrixError::Var("ZMQ_URL".to_string())))
}

fn reactrix_url() -> Result<String> {
    env::var("REACTRIX_URL").or_else(|_| Err(AggregatrixError::Var("REACTRIX_URL".to_string())))
}

fn zmq_client() -> Result<zmq::Socket> {
    let context = zmq::Context::new();
    let socket = context.socket(zmq::SUB)?;
    socket.connect(&zmq_url()?)?;
    Ok(socket)
}

pub trait Aggregatrix: Sized {
    type State: Default + Clone + Send + Sync + 'static;
    type Error: Clone + Display + Serialize + Send + 'static;
    type Result: Clone + Send + 'static;

    type Context: Clone + Send + Sync + 'static;
    type Query: GraphQLType<Context = Self::Context, TypeInfo = ()> + Send + Sync + 'static;
    type Mutation: GraphQLType<Context = Self::Context, TypeInfo = ()> + Send + Sync + 'static;

    fn dispatch(state: &Self::State, event: &Event) -> result::Result<Self::Result, Self::Error>;
    fn schema() -> RootNode<'static, Self::Query, Self::Mutation>;
    fn context(state: Self::State, results: Results<Self>, api: Api) -> Self::Context;
}

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

pub fn launch<A: 'static + Aggregatrix + Clone>() -> Result<BoxedFilter<(impl Reply,)>> {
    let api = Api::new(&reactrix_url()?)?;
    let pg = PgConnection::establish(&database_url()?)?;

    let queue = Arc::new((Mutex::new(LinkedList::<i64>::new()), Condvar::new()));
    zmq_queue(queue.clone())?;

    let results = results::launch::<A>();
    let tx_results_events = results.channel.lock().unwrap().0.clone();

    let (state, mut sequence) = init::<A>(&tx_results_events, &pg)?;

    let context = A::context(state.clone(), results, api);

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

    Ok(server::prepare::<A>(context))
}
