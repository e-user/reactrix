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

#![feature(proc_macro_hygiene, decl_macro, try_trait)]

mod broker;

use base64;
use broker::Tx;
use diesel::prelude::*;
use diesel::result::DatabaseErrorKind;
use diesel::result::Error as DieselError;
use exitfailure::ExitFailure;
use hex;
use reactrix::keystore::{KeyStore, KeyStoreError};
use reactrix::{models, schema};
use rocket::fairing;
use rocket::fairing::Fairing;
use rocket::http::Status;
use rocket::response::Responder;
use rocket::{catch, catchers, delete, get, post, routes, Request, Response, Rocket, State};
use rocket_contrib::database;
use rocket_contrib::databases::diesel::PgConnection;
use rocket_contrib::json;
use rocket_contrib::json::{Json, JsonError, JsonValue};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::env;
use std::error::Error;
use std::ops::Try;
use std::result;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use structopt::StructOpt;

#[derive(Debug)]
enum StoreError {
    RocketConfig(rocket::config::ConfigError),
    Var(env::VarError),
}

impl From<rocket::config::ConfigError> for StoreError {
    fn from(error: rocket::config::ConfigError) -> Self {
        StoreError::RocketConfig(error)
    }
}

impl From<env::VarError> for StoreError {
    fn from(error: env::VarError) -> Self {
        StoreError::Var(error)
    }
}

type Result<T> = result::Result<T, StoreError>;

#[derive(Debug)]
struct StoreResponder(Status, JsonValue);

impl StoreResponder {
    fn ok<T: Serialize>(status: Status, data: Option<&T>) -> Self {
        match data {
            None => StoreResponder(status, json!({ "status": "ok" })),
            Some(d) => StoreResponder(status, json!({ "status": "ok", "data": d })),
        }
    }

    fn error(status: Status, reason: &str) -> Self {
        StoreResponder(status, json!({ "status": "error", "reason": reason }))
    }
}

impl<'r> Responder<'r> for StoreResponder {
    fn respond_to(self, req: &Request) -> result::Result<Response<'r>, Status> {
        Response::build_from(self.1.respond_to(req)?)
            .status(self.0)
            .ok()
    }
}

impl Try for StoreResponder {
    type Ok = Self;
    type Error = Self;

    fn into_result(self) -> result::Result<Self, Self> {
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
            &format!("Couldn't obtain lock: {}", error.description().to_string()),
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

fn hex_decode(name: &str, data: &[u8]) -> result::Result<Vec<u8>, StoreResponder> {
    hex::decode(data).or_else(|e| {
        Err(StoreResponder::error(
            Status::BadRequest,
            &format!("Couldn't decode {}: {}", name, e),
        ))
    })
}

fn base64_decode(name: &str, data: &[u8]) -> result::Result<Vec<u8>, StoreResponder> {
    base64::decode(data).or_else(|e| {
        Err(StoreResponder::error(
            Status::BadRequest,
            &format!("Couldn't decode {}: {}", name, e),
        ))
    })
}

#[post("/v1/create", format = "application/json", data = "<event>")]
fn create(
    conn: StoreDbConn,
    tx: State<Arc<Mutex<Tx>>>,
    event: result::Result<Json<Event>, JsonError>,
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

#[get("/v1/generate-key")]
fn generate_key(conn: StoreDbConn) -> StoreResponder {
    let store = KeyStore::new(&conn);
    match store.generate_key() {
        Ok(key) => StoreResponder::ok(Status::Created, Some(&hex::encode(key))),
        Err(e) => StoreResponder::error(
            Status::InternalServerError,
            &format!("Couldn't generate key: {}", e),
        ),
    }
}

#[get("/v1/generate-nonce")]
fn generate_nonce() -> StoreResponder {
    StoreResponder::ok(
        Status::Ok,
        Some(&base64::encode(&KeyStore::generate_nonce())),
    )
}

#[delete("/v1/delete-key/<id>")]
fn delete_key(conn: StoreDbConn, id: String) -> StoreResponder {
    let store = KeyStore::new(&conn);
    let id = hex_decode("id", id.as_bytes())?;

    match store.delete_key(&id) {
        Ok(_) | Err(KeyStoreError::NoKey) => StoreResponder::ok::<()>(Status::Ok, None),
        Err(e) => StoreResponder::error(
            Status::InternalServerError,
            &format!("Couldn't delete key: {}", e),
        ),
    }
}

#[post("/v1/encrypt/<id>", format = "application/json", data = "<data>")]
fn encrypt(
    conn: StoreDbConn,
    id: String,
    data: result::Result<Json<Encrypted<Value>>, JsonError>,
) -> StoreResponder {
    let store = KeyStore::new(&conn);
    let id = hex_decode("id", id.as_bytes())?;
    let data = data?;
    let nonce = base64_decode("nonce", data.nonce.as_bytes())?;
    let data = serde_json::to_string(&data.data)?;

    match store.encrypt(&id, &nonce, &data.as_bytes()) {
        Ok(data) => StoreResponder::ok(Status::Ok, Some(&base64::encode(&data))),
        Err(KeyStoreError::NoKey) => StoreResponder::error(Status::NotFound, "No such key"),
        Err(e) => StoreResponder::error(
            Status::InternalServerError,
            &format!("Couldn't encrypt data: {}", e),
        ),
    }
}

#[post("/v1/decrypt/<id>", format = "application/json", data = "<data>")]
fn decrypt(
    conn: StoreDbConn,
    id: String,
    data: result::Result<Json<Encrypted<String>>, JsonError>,
) -> StoreResponder {
    let store = KeyStore::new(&conn);
    let id = hex_decode("id", id.as_bytes())?;
    let data = data?;
    let nonce = base64_decode("nonce", data.nonce.as_bytes())?;
    let data = base64_decode("data", data.data.as_bytes())?;

    match store.decrypt(&id, &nonce, &data) {
        Ok(data) => match serde_json::from_slice::<Value>(&data) {
            Ok(data) => StoreResponder::ok(Status::Ok, Some(&data)),
            Err(_) => StoreResponder::error(
                Status::InternalServerError,
                "Could not decode decrypted data as JSON",
            ),
        },
        Err(KeyStoreError::NoKey) => StoreResponder::error(Status::NotFound, "No such key"),
        Err(e) => StoreResponder::error(
            Status::InternalServerError,
            &format!("Couldn't decrypt data: {}", e),
        ),
    }
}

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

    fn on_attach(&self, rocket: Rocket) -> result::Result<Rocket, Rocket> {
        Ok(rocket.manage(self.tx.clone()))
    }
}

/// reactrix-store
#[derive(Debug, StructOpt)]
#[structopt(raw(setting = "structopt::clap::AppSettings::ColoredHelp"))]
struct Cli {}

fn main() -> result::Result<(), ExitFailure> {
    Cli::from_args();
    env_logger::builder().format_timestamp(None).init();
    let tx = broker::launch()?;

    Err(rocket::ignite()
        .attach(StoreDbConn::fairing())
        .attach(Zmq {
            tx: Arc::new(Mutex::new(tx)),
        })
        .mount(
            "/",
            routes![
                create,
                generate_key,
                generate_nonce,
                delete_key,
                encrypt,
                decrypt
            ],
        )
        .register(catchers![not_found, internal_server_error])
        .launch()
        .into())
}