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

#![feature(proc_macro_hygiene, decl_macro, try_trait)]

mod mq;

use diesel::prelude::*;
use diesel::result::DatabaseErrorKind;
use diesel::result::Error as DieselError;
use dotenv::dotenv;
use exitfailure::ExitFailure;
use log::{error, warn};
use mq::Tx;
use reactrix::api::ApiResult;
use reactrix::datastore::{DataStore, DataStoreError};
use reactrix::{models, schema};
use rocket::fairing;
use rocket::fairing::Fairing;
use rocket::http::Status;
use rocket::response::Responder;
use rocket::{catch, catchers, get, post, put, routes, Request, Response, Rocket, State};
use rocket_contrib::database;
use rocket_contrib::databases::diesel::PgConnection;
use rocket_contrib::json;
use rocket_contrib::json::{Json, JsonError, JsonValue};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::ops::Try;
use std::result::Result;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use structopt::StructOpt;

#[derive(Debug)]
struct StoreResponder(Status, JsonValue);

impl StoreResponder {
    fn ok<T: Serialize>(status: Status, data: Option<&T>) -> Self {
        match data {
            None => StoreResponder(status, json!(ApiResult::<Option<()>>::Ok { data: None })),
            Some(d) => StoreResponder(status, json!(ApiResult::Ok { data: Some(d) })),
        }
    }

    fn error(status: Status, reason: &str) -> Self {
        StoreResponder(
            status,
            json!(ApiResult::<()>::Error {
                reason: reason.to_string()
            }),
        )
    }
}

impl<'r> Responder<'r> for StoreResponder {
    fn respond_to(self, req: &Request) -> Result<Response<'r>, Status> {
        Response::build_from(self.1.respond_to(req)?)
            .status(self.0)
            .ok()
    }
}

impl Try for StoreResponder {
    type Ok = Self;
    type Error = Self;

    fn into_result(self) -> Result<Self, Self> {
        if self.0.code < 300 {
            Ok(self)
        } else {
            Err(self)
        }
    }

    fn from_ok(ok: Self::Ok) -> Self {
        ok
    }

    fn from_error(error: Self::Error) -> Self {
        error
    }
}

impl<'a> From<JsonError<'a>> for StoreResponder {
    fn from(error: JsonError) -> Self {
        match error {
            JsonError::Io(error) => {
                StoreResponder::error(Status::BadRequest, &format!("{:?}", error))
            }
            JsonError::Parse(_, error) => {
                StoreResponder::error(Status::BadRequest, &format!("{:?}", error))
            }
        }
    }
}

impl From<serde_json::Error> for StoreResponder {
    fn from(error: serde_json::Error) -> Self {
        StoreResponder::error(Status::BadRequest, &format!("{:?}", error))
    }
}

impl<'a> From<PoisonError<MutexGuard<'a, Tx>>> for StoreResponder {
    fn from(error: PoisonError<MutexGuard<'a, Tx>>) -> Self {
        StoreResponder::error(
            Status::InternalServerError,
            &format!("Couldn't obtain lock: {}", error.to_string()),
        )
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct Event {
    version: i32,
    data: Map<String, Value>,
}

impl From<Event> for models::NewEvent {
    fn from(event: Event) -> Self {
        Self {
            version: event.version,
            data: event.data.into(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Encrypted<T> {
    pub nonce: String,
    pub data: T,
}

#[database("events")]
struct StoreDbConn(PgConnection);

#[post("/v1/create", format = "application/json", data = "<event>")]
fn create(
    conn: StoreDbConn,
    tx: State<Arc<Mutex<Tx>>>,
    event: Result<Json<Event>, JsonError>,
) -> StoreResponder {
    let event = event?.into_inner();

    let result = diesel::insert_into(schema::events::table)
        .values::<models::NewEvent>(event.into())
        .get_result::<models::Event>(&*conn);

    match result {
        Ok(event) => match tx.lock()?.send(event.sequence) {
            Ok(()) => StoreResponder::ok(Status::Created, Some(&event.sequence)),
            Err(e) => StoreResponder::error(
                Status::InternalServerError,
                &format!("Created but couldn't notify: {:?}", e),
            ),
        },
        Err(DieselError::DatabaseError(DatabaseErrorKind::UniqueViolation, _)) => {
            StoreResponder::error(Status::BadRequest, &"Out of Sequence")
        }
        Err(e) => StoreResponder::error(Status::InternalServerError, &format!("{:?}", e)),
    }
}

#[put("/v1/store", data = "<data>")]
fn store(conn: StoreDbConn, data: Vec<u8>) -> StoreResponder {
    let store = DataStore::new(&conn);
    match store.store(&data) {
        Ok(hash) => StoreResponder::ok(Status::Ok, Some(&hex::encode(hash))),
        Err(e) => StoreResponder::error(
            Status::InternalServerError,
            &format!("Couldn't store data: {}", e),
        ),
    }
}

#[get("/v1/retrieve/<id>")]
fn retrieve(conn: StoreDbConn, id: String) -> Option<Vec<u8>> {
    let store = DataStore::new(&conn);
    let hash = match hex::decode(id.as_bytes()) {
        Ok(hash) => hash,
        Err(e) => {
            warn!("Couldn't decode hash: {}", e);
            return None;
        }
    };

    match store.retrieve(&hash) {
        Ok(data) => Some(data),
        Err(DataStoreError::NoRecord) => None,
        Err(e) => {
            error!("Couldn't retrieve data hash {}: {}", id, e);
            None
        }
    }
}

// TODO config handler (ZMQ port etc)

#[catch(404)]
fn not_found() -> StoreResponder {
    StoreResponder::error(Status::NotFound, &"No such resource")
}

#[catch(500)]
fn internal_server_error() -> StoreResponder {
    StoreResponder::error(Status::InternalServerError, &"Internal server error")
}

struct Zmq {
    tx: Arc<Mutex<Tx>>,
}

impl Fairing for Zmq {
    fn info(&self) -> fairing::Info {
        fairing::Info {
            name: "ØMQ Channel",
            kind: fairing::Kind::Attach,
        }
    }

    fn on_attach(&self, rocket: Rocket) -> Result<Rocket, Rocket> {
        Ok(rocket.manage(self.tx.clone()))
    }
}

/// reactrix-store
#[derive(Debug, StructOpt)]
#[structopt(raw(setting = "structopt::clap::AppSettings::ColoredHelp"))]
struct Cli {}

fn main() -> Result<(), ExitFailure> {
    Cli::from_args();
    dotenv()?;
    env_logger::builder().format_timestamp(None).init();
    let tx = mq::launch()?;

    Err(rocket::ignite()
        .attach(StoreDbConn::fairing())
        .attach(Zmq {
            tx: Arc::new(Mutex::new(tx)),
        })
        .mount("/", routes![create, store, retrieve])
        .register(catchers![not_found, internal_server_error])
        .launch()
        .into())
}
