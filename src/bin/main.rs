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

use base64;
use diesel::prelude::*;
use diesel::result::DatabaseErrorKind;
use diesel::result::Error as DieselError;
use hex;
use reactrix::keystore::{KeyStore, KeyStoreError};
use reactrix::{models, schema};
use redis;
use redis::Commands;
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
use std::result;
use structopt::StructOpt;

#[derive(Debug)]
enum StoreError {
    RocketConfig(rocket::config::ConfigError),
    Redis(redis::RedisError),
    Var(env::VarError),
}

impl From<rocket::config::ConfigError> for StoreError {
    fn from(error: rocket::config::ConfigError) -> Self {
        StoreError::RocketConfig(error)
    }
}

impl From<redis::RedisError> for StoreError {
    fn from(error: redis::RedisError) -> Self {
        StoreError::Redis(error)
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

#[derive(Serialize, Deserialize, Debug)]
struct Event {
    version: i32,
    r#type: String,
    data: Map<String, Value>,
}

impl From<Event> for models::NewEvent {
    fn from(event: Event) -> Self {
        Self {
            version: event.version,
            type_: event.r#type,
            data: event.data.into(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct Data {
    nonce: String,
    data: String,
}

#[database("events")]
struct StoreDbConn(PgConnection);

fn notify(redis: State<redis::Client>, sequence: i64) -> Result<()> {
    let mut redis = redis.get_connection()?;
    let _: () = redis.publish("sequence", sequence)?;
    Ok(())
}

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
    redis: State<redis::Client>,
    event: result::Result<Json<Event>, JsonError>,
) -> StoreResponder {
    let event = match event {
        Ok(event) => event.into_inner(),
        Err(e) => return e.into(),
    };

    let result = diesel::insert_into(schema::events::table)
        .values::<models::NewEvent>(event.into())
        .get_result::<models::Event>(&conn.0);

    match result {
        Ok(event) => match notify(redis, event.sequence) {
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
    let store = KeyStore::new(&conn.0);
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
    let store = KeyStore::new(&conn.0);
    let id = match hex_decode("id", id.as_bytes()) {
        Ok(id) => id,
        Err(e) => return e,
    };

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
    data: result::Result<Json<Data>, JsonError>,
) -> StoreResponder {
    let store = KeyStore::new(&conn.0);
    let id = match hex_decode("id", id.as_bytes()) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let data = match data {
        Ok(data) => data.into_inner(),
        Err(e) => return e.into(),
    };
    let nonce = match base64_decode("nonce", data.nonce.as_bytes()) {
        Ok(nonce) => nonce,
        Err(e) => return e,
    };

    match store.encrypt(&id, &nonce, &data.data.as_bytes()) {
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
    data: result::Result<Json<Data>, JsonError>,
) -> StoreResponder {
    let store = KeyStore::new(&conn.0);
    let id = match hex_decode("id", id.as_bytes()) {
        Ok(id) => id,
        Err(e) => return e,
    };
    let data = match data {
        Ok(data) => data.into_inner(),
        Err(e) => return e.into(),
    };
    let nonce = match base64_decode("nonce", data.nonce.as_bytes()) {
        Ok(nonce) => nonce,
        Err(e) => return e,
    };
    let data = match base64_decode("data", data.data.as_bytes()) {
        Ok(data) => data,
        Err(e) => return e,
    };

    match store.decrypt(&id, &nonce, &data) {
        Ok(data) => match String::from_utf8(data) {
            Ok(data) => StoreResponder::ok(Status::Ok, Some(&data)),
            Err(_) => StoreResponder::error(
                Status::InternalServerError,
                "Could not encode decoded data to UTF-8",
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

struct Redis;

impl Fairing for Redis {
    fn info(&self) -> fairing::Info {
        fairing::Info {
            name: "Redis Client",
            kind: fairing::Kind::Attach,
        }
    }

    fn on_attach(&self, rocket: Rocket) -> result::Result<Rocket, Rocket> {
        match rocket.config().get_string("redis") {
            Ok(url) => match redis::Client::open(&*url) {
                Ok(client) => Ok(rocket.manage(client)),
                Err(e) => {
                    println!("Couldn't connect to Redis: {:?}", e);
                    Err(rocket)
                }
            },
            Err(_) => {
                println!("Redis is not configured");
                Err(rocket)
            }
        }
    }
}

/// reactrix-store
#[derive(Debug, StructOpt)]
#[structopt(raw(setting = "structopt::clap::AppSettings::ColoredHelp"))]
struct Cli {}

fn main() {
    Cli::from_args();

    rocket::ignite()
        .attach(StoreDbConn::fairing())
        .attach(Redis)
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
        .launch();
}
