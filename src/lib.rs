// This file is part of reactrix.
//
// Copyright 2019 Alexander Dorn
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

pub mod keystore;
pub mod models;
pub mod schema;
mod server;

use diesel::pg::PgConnection;
use diesel::prelude::*;
use diesel::result::Error as DieselError;
use failure::Fail;
use redis::{ControlFlow, PubSubCommands};
use rocket::Rocket;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::fmt::{Debug, Display};
use std::sync::{mpsc, Arc, RwLock};
use std::thread;

pub use models::Event;
pub use reactrix_derive::Event;
pub use rocket;
pub use serde::Serialize;
pub use serde_json::Error as JsonError;
pub use serde_json::Value as JsonValue;
pub use server::{ProcessResponder, ServerState};

#[derive(Debug, Fail)]
#[fail(display = "Out of Sequence: {}", 0)]
pub struct OutOfSequenceError(i64);

#[derive(Debug, Fail)]
pub enum AggregatrixError {
    #[fail(display = "Environment variable is missing: {}", 0)]
    Var(env::VarError),
    #[fail(display = "Database connection failed: {}", 0)]
    Connection(ConnectionError),
    #[fail(display = "Redis connection failed: {}", 0)]
    Redis(redis::RedisError),
    #[fail(display = "Database error: {}", 0)]
    Database(DieselError),
    #[fail(display = "Event out of sequence: {}", 0)]
    OutOfSequence(OutOfSequenceError),
    #[fail(display = "TxStore error: {}", 0)]
    ResultProcessing(TxStoreError),
    #[fail(display = "Could not convert JSON: {}", 0)]
    JsonConversion(String),
}

impl From<env::VarError> for AggregatrixError {
    fn from(error: env::VarError) -> Self {
        AggregatrixError::Var(error)
    }
}

impl From<ConnectionError> for AggregatrixError {
    fn from(error: ConnectionError) -> Self {
        AggregatrixError::Connection(error)
    }
}

impl From<redis::RedisError> for AggregatrixError {
    fn from(error: redis::RedisError) -> Self {
        AggregatrixError::Redis(error)
    }
}

impl From<DieselError> for AggregatrixError {
    fn from(error: DieselError) -> Self {
        AggregatrixError::Database(error)
    }
}

impl From<TxStoreError> for AggregatrixError {
    fn from(error: TxStoreError) -> Self {
        AggregatrixError::ResultProcessing(error)
    }
}

impl From<serde_json::Error> for AggregatrixError {
    fn from(error: serde_json::Error) -> Self {
        AggregatrixError::JsonConversion(format!("{}", error))
    }
}

pub type Result<T> = std::result::Result<T, AggregatrixError>;

fn database_url() -> Result<String> {
    Ok(env::var("DATABASE_URL")?)
}

fn redis_url() -> Result<String> {
    Ok(env::var("REDIS_URL")?)
}

fn redis_client() -> Result<redis::Client> {
    Ok(redis::Client::open(&*redis_url()?)?)
}

pub type StateLock<T> = Arc<RwLock<T>>;

pub trait Aggregatrix {
    type State: Default + Debug + Send + Sync + 'static;
    type Error: Display + Serialize;

    fn dispatch(
        state: &StateLock<Self::State>,
        event: &Event,
    ) -> std::result::Result<Value, Self::Error>;
}

type Results = HashMap<i64, Value>;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ApiResult<T> {
    Ok { data: Option<T> },
    Error { reason: String },
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct Encrypted {
    pub key_id: String,
    pub nonce: String,
    pub data: String,
}

fn process_event<A: Aggregatrix>(
    sequence: i64,
    state: &StateLock<A::State>,
    tx: &TxStore,
    pg: &PgConnection,
) -> Result<()> {
    use schema::events::dsl;

    let event = dsl::events
        .filter(dsl::sequence.eq(sequence))
        .first::<Event>(pg)?;

    println!("Processing event {}", sequence);

    match A::dispatch(state, &event) {
        Ok(result) => {
            tx.send(TxStoreEvent::Store(
                sequence,
                json!({ "type": "ok", "data": result }),
            ))?;
        }
        Err(e) => {
            println!("Event error: {}", e);
            tx.send(TxStoreEvent::Store(
                sequence,
                json!({ "type": "error", "reason": e}),
            ))?;
        }
    }

    Ok(())
}

fn init<A: Aggregatrix>(tx: &TxStore, pg: &PgConnection) -> Result<(StateLock<A::State>, i64)> {
    use schema::events::dsl;

    let max = match dsl::events
        .select(dsl::sequence)
        .order(dsl::sequence.desc())
        .limit(1)
        .first::<i64>(pg)
    {
        Ok(id) => id,
        Err(DieselError::NotFound) => 0,
        Err(e) => return Err(e.into()),
    };

    let state = A::State::default();
    let lock = Arc::new(RwLock::new(state));

    for id in 1..=max {
        process_event::<A>(id, &lock, tx, pg)?;
    }

    Ok((lock, max))
}

pub enum TxStoreEvent {
    Store(i64, Value),
    Retrieve(i64),
}

pub enum RxStoreEvent {
    Result(Value),
    NoValue,
}

pub type TxStore = mpsc::Sender<TxStoreEvent>;
pub type RxStore = mpsc::Receiver<RxStoreEvent>;
type TxStoreError = mpsc::SendError<TxStoreEvent>;

pub fn ignite<A: Aggregatrix>() -> Result<Rocket> {
    let pg = PgConnection::establish(&database_url()?)?;
    let mut redis = redis_client()?.get_connection()?;
    let (tx_store, rx_store) = mpsc::channel();
    let (tx_server, rx_server) = mpsc::channel();
    let tx_store_rocket = tx_store.clone();

    let mut results: Results = HashMap::new();

    thread::spawn(move || {
        for msg in rx_store {
            match msg {
                TxStoreEvent::Store(id, value) => {
                    results.insert(id, value);
                    println!("{:?}", results);
                }
                TxStoreEvent::Retrieve(id) => match results.get(&id) {
                    Some(result) => {
                        if let Err(e) = tx_server.send(RxStoreEvent::Result(result.clone())) {
                            println!("{:?}", e);
                            return;
                        }
                    }
                    None => {
                        if let Err(e) = tx_server.send(RxStoreEvent::NoValue) {
                            println!("{:?}", e);
                            return;
                        }
                    }
                },
            }
        }
    });

    let (lock, mut sequence) = init::<A>(&tx_store, &pg)?;
    let server_lock = lock.clone();

    thread::spawn(move || {
        redis
            .subscribe("sequence", |msg| match msg.get_payload::<i64>() {
                Ok(id) => {
                    if id <= sequence {
                        return ControlFlow::Continue;
                    }

                    for id in sequence + 1..=id {
                        match process_event::<A>(id, &lock, &tx_store, &pg) {
                            Ok(()) => {
                                println!("{:?}", lock.read());
                            }
                            Err(e) => {
                                println!("{:?}", e);
                                return ControlFlow::Break(e);
                            }
                        }
                    }

                    sequence = id;

                    ControlFlow::Continue
                }
                Err(e) => {
                    println!("{:?}", e);
                    ControlFlow::Continue
                }
            })
            .unwrap();
    });

    Ok(server::ignite(tx_store_rocket, rx_server, server_lock))
}
