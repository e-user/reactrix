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

use super::results::{Rx, RxEvent, Tx, TxEvent};
use super::ApiResult;
use rocket::http::Status;
use rocket::response::Responder;
use rocket::{catch, catchers, get, routes, Request, Response, Rocket, State};
use rocket_contrib::json;
use rocket_contrib::json::JsonValue;
use serde::Serialize;
use std::sync::Mutex;

#[derive(Debug)]
pub struct ProcessResponder(Status, JsonValue);

impl ProcessResponder {
    pub fn ok<T: Serialize>(status: Status, data: Option<&T>) -> Self {
        match data {
            None => ProcessResponder(status, json!(ApiResult::<()>::Ok { data: None })),
            Some(data) => ProcessResponder(status, json!(ApiResult::Ok { data: Some(data) })),
        }
    }

    pub fn error(status: Status, reason: &str) -> Self {
        ProcessResponder(
            status,
            json!(ApiResult::<()>::Error {
                reason: reason.to_string(),
            }),
        )
    }
}

impl<'r> Responder<'r> for ProcessResponder {
    fn respond_to(self, req: &Request) -> Result<Response<'r>, Status> {
        Response::build_from(self.1.respond_to(req)?)
            .status(self.0)
            .ok()
    }
}

#[get("/v1/retrieve/<id>")]
fn retrieve(id: i64, state: State<Mutex<(Tx, Rx)>>) -> ProcessResponder {
    match state.lock() {
        Ok(guard) => {
            let (tx, rx) = &*guard;

            if let Err(e) = tx.send(TxEvent::Retrieve(id)) {
                return ProcessResponder::error(
                    Status::InternalServerError,
                    &format!("Result tx channel died: {:?}", e),
                );
            }

            match rx.recv() {
                Ok(RxEvent::Result(data)) => ProcessResponder::ok(Status::Ok, Some(&data)),
                Ok(RxEvent::NoValue) => {
                    ProcessResponder::error(Status::BadRequest, "No result with id")
                }
                Err(e) => ProcessResponder::error(
                    Status::InternalServerError,
                    &format!("Result rx channel died: {:?}", e),
                ),
            }
        }
        Err(e) => ProcessResponder::error(
            Status::InternalServerError,
            &format!("Couldn't acquire lock: {:?}", e),
        ),
    }
}

#[catch(404)]
fn not_found() -> ProcessResponder {
    ProcessResponder::error(Status::NotFound, &"No such resource")
}

pub fn ignite<T>(tx: Tx, rx: Rx, context: T) -> Rocket
where
    T: Send + Sync + 'static,
{
    rocket::ignite()
        .manage(Mutex::new((tx, rx)))
        .manage(context)
        .mount("/", routes![retrieve])
        .register(catchers![not_found])
}
