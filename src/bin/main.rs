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

use diesel::prelude::*;
use diesel::result::DatabaseErrorKind;
use diesel::result::Error as DieselError;
use reactrix::{models, schema};
use redis;
use redis::Commands;
use rocket::fairing;
use rocket::fairing::Fairing;
use rocket::http::Status;
use rocket::response::Responder;
use rocket::{catch, catchers, post, routes, Request, Response, Rocket, State};
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

#[database("events")]
struct StoreDbConn(PgConnection);

fn notify(redis: State<redis::Client>, sequence: i64) -> Result<()> {
    let mut redis = redis.get_connection()?;
    let _: () = redis.publish("sequence", sequence)?;
    Ok(())
}

#[post("/v1/create", format = "application/json", data = "<event>")]
fn create(
    conn: StoreDbConn,
    redis: State<redis::Client>,
    event: result::Result<Json<Event>, JsonError>,
) -> StoreResponder {
    let event = match event {
        Ok(event) => event.into_inner(),
        Err(JsonError::Io(error)) => {
            return StoreResponder::error(Status::BadRequest, &format!("{:?}", error))
        }
        Err(JsonError::Parse(_, error)) => {
            return StoreResponder::error(Status::BadRequest, &format!("{:?}", error))
        }
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
        .mount("/", routes![create])
        .register(catchers![not_found, internal_server_error])
        .launch();
}
