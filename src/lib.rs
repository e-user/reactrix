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

mod api;
pub mod keystore;
pub mod models;
mod results;
pub mod schema;
mod server;

use diesel::pg::PgConnection;
use diesel::prelude::*;
use diesel::result::Error as DieselError;
use failure::Fail;
use juniper::{GraphQLObject, GraphQLType, RootNode};
use redis::{ControlFlow, PubSubCommands};
use results::{Tx, TxError, TxEvent};
use rocket::Rocket;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::LinkedList;
use std::env;
use std::fmt::{Debug, Display};
use std::result;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

pub use api::*;
pub use models::Event;
pub use reactrix_derive::Event;
pub use rocket;
pub use serde::Serialize;
pub use serde_json::Error as JsonError;
pub use serde_json::Value as JsonValue;
pub use server::ProcessResponder;

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
    #[fail(display = "Tx error: {}", 0)]
    ResultProcessing(TxError),
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

pub trait Aggregatrix {
    type Context: Default + Clone + Send + Sync + 'static;
    type Error: Display + Serialize;
    type Query: GraphQLType<Context = Self::Context, TypeInfo = ()> + Send + Sync + Clone;
    type Mutation: GraphQLType<Context = Self::Context, TypeInfo = ()> + Send + Sync + Clone;

    fn dispatch(context: &Self::Context, event: &Event) -> result::Result<Value, Self::Error>;
    fn schema() -> RootNode<'static, Self::Query, Self::Mutation>;
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ApiResult<T> {
    Ok { data: Option<T> },
    Error { reason: String },
}

#[derive(Debug, Deserialize, Serialize, Clone, GraphQLObject)]
#[serde(rename_all = "kebab-case")]
pub struct Encrypted {
    pub key_id: String,
    pub nonce: String,
    pub data: String,
}

fn process_event<A: Aggregatrix>(
    sequence: i64,
    context: &A::Context,
    tx: &Tx,
    pg: &PgConnection,
) -> Result<()> {
    use schema::events::dsl;

    println!("Processing event {}", sequence);

    let event = dsl::events
        .filter(dsl::sequence.eq(sequence))
        .first::<Event>(pg)?;

    match A::dispatch(context, &event) {
        Ok(result) => {
            tx.send(TxEvent::Store(
                sequence,
                json!({ "type": "ok", "data": result }),
            ))?;
        }
        Err(e) => {
            println!("Event {} error: {}", sequence, e);
            tx.send(TxEvent::Store(
                sequence,
                json!({ "type": "error", "reason": e}),
            ))?;
        }
    }

    Ok(())
}

fn init<A: Aggregatrix>(tx: &Tx, pg: &PgConnection) -> Result<(A::Context, i64)> {
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

    let context = A::Context::default();

    for id in 1..=max {
        process_event::<A>(id, &context, tx, pg)?;
    }

    Ok((context, max))
}

fn redis_queue(queue: Arc<(Mutex<LinkedList<i64>>, Condvar)>) -> Result<()> {
    let mut redis = redis_client()?.get_connection()?;
    thread::spawn(move || {
        let (queue, condvar) = &*queue;
        redis.subscribe("sequence", |msg| match msg.get_payload::<i64>() {
            Ok(id) => match queue.lock() {
                Ok(mut queue) => {
                    queue.push_back(id);
                    condvar.notify_one();
                    ControlFlow::Continue
                }
                Err(e) => ControlFlow::Break(format!("Couldn't lock mutex: {}", e)),
            },
            Err(e) => {
                println!("{:?}", e);
                ControlFlow::Continue
            }
        })
    });

    Ok(())
}

pub fn ignite<A: 'static + Aggregatrix + Clone>() -> Result<Rocket> {
    let pg = PgConnection::establish(&database_url()?)?;

    let queue = Arc::new((Mutex::new(LinkedList::<i64>::new()), Condvar::new()));
    redis_queue(queue.clone())?;

    let (tx_results, rx_results) = results::launch();
    let tx_results_rocket = tx_results.clone();

    let (context, mut sequence) = init::<A>(&tx_results, &pg)?;
    let server_context = context.clone();

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
                    for id in sequence + 1..=id {
                        process_event::<A>(id, &context, &tx_results, &pg).unwrap();
                        sequence = id;
                    }
                }
            }
        }
    });

    Ok(server::ignite::<A>(
        tx_results_rocket,
        rx_results,
        server_context,
    ))
}
