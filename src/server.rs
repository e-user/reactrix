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

use super::{RxStore, RxStoreEvent, TxStore, TxStoreEvent};
use rocket;
use rocket::http::Status;
use rocket::response::Responder;
use rocket::{catch, catchers, get, routes, Request, Response, State};
use rocket_contrib::json;
use rocket_contrib::json::JsonValue;
use serde::Serialize;
use std::sync::Mutex;

#[derive(Debug)]
struct ProcessResponder(Status, JsonValue);

impl ProcessResponder {
    fn ok<T: Serialize>(status: Status, data: Option<&T>) -> Self {
        match data {
            None => ProcessResponder(status, json!({ "status": "ok" })),
            Some(d) => ProcessResponder(status, json!({ "status": "ok", "data": d })),
        }
    }

    fn error(status: Status, reason: &str) -> Self {
        ProcessResponder(status, json!({ "status": "error", "reason": reason }))
    }
}

impl<'r> Responder<'r> for ProcessResponder {
    fn respond_to(self, req: &Request) -> Result<Response<'r>, Status> {
        Response::build_from(self.1.respond_to(req)?)
            .status(self.0)
            .ok()
    }
}

#[get("/retrieve/<id>")]
fn retrieve(id: i64, state: State<Mutex<(TxStore, RxStore)>>) -> ProcessResponder {
    match state.inner().lock() {
        Ok(guard) => {
            let (tx, rx) = &*guard;

            if let Err(e) = tx.send(TxStoreEvent::Retrieve(id)) {
                return ProcessResponder::error(
                    Status::InternalServerError,
                    &format!("Result tx channel died: {:?}", e),
                );
            }

            match rx.recv() {
                Ok(RxStoreEvent::Result(data)) => ProcessResponder::ok(Status::Ok, Some(&data)),
                Ok(RxStoreEvent::NoValue) => {
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

pub fn launch(tx: TxStore, rx: RxStore) {
    rocket::ignite()
        .manage(Mutex::new((tx, rx)))
        .mount("/", routes![retrieve])
        .register(catchers![not_found])
        .launch();
}
