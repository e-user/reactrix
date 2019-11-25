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
use super::{Aggregatrix, ApiResult};
use juniper::{IntrospectionFormat, RootNode};
use juniper_rocket::{graphiql_source, GraphQLRequest};
use rocket::data::FromDataSimple;
use rocket::handler::{Handler, Outcome};
use rocket::http::{Method, Status};
use rocket::response::content::Html;
use rocket::response::Responder;
use rocket::Outcome::{Failure, Forward, Success};
use rocket::{catch, catchers, get, routes, Data, Request, Response, Rocket, Route, State};
use rocket_contrib::json;
use rocket_contrib::json::{Json, JsonValue};
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

struct GraphQLHandler<A: Aggregatrix> {
    schema: RootNode<'static, A::Query, A::Mutation>,
    context: A::Context,
}

impl<A: Aggregatrix> Clone for GraphQLHandler<A> {
    fn clone(&self) -> Self {
        Self {
            schema: A::schema(),
            context: self.context.clone(),
        }
    }
}

impl<A: 'static + Aggregatrix + Clone> Handler for GraphQLHandler<A> {
    fn handle<'r>(&self, request: &'r Request, data: Data) -> Outcome<'r> {
        let schema = &self.schema;
        let context = &self.context;

        if request.uri().path() == "/schema.json" {
            match juniper::introspect(schema, context, IntrospectionFormat::default()) {
                Ok((res, _errors)) => Outcome::from(request, Json(res)),
                Err(error) => Outcome::from(
                    request,
                    ProcessResponder::error(Status::InternalServerError, &format!("{:?}", error))
                        .respond_to(request),
                ),
            }
        } else {
            match GraphQLRequest::from_data(request, data) {
                Success(graphql_request) => Outcome::from(
                    request,
                    graphql_request.execute(schema, context).respond_to(request),
                ),
                Forward(data) => Outcome::Forward(data),
                Failure((status, error)) => Outcome::from(
                    request,
                    ProcessResponder::error(status, &error).respond_to(request),
                ),
            }
        }
    }
}

impl<A: 'static + Aggregatrix + Clone> Into<Vec<Route>> for GraphQLHandler<A> {
    fn into(self) -> Vec<Route> {
        vec![
            Route::new(Method::Get, "/graphql", self.clone()),
            Route::new(Method::Post, "/graphql", self.clone()),
            Route::new(Method::Get, "/schema.json", self),
        ]
    }
}

#[get("/")]
fn graphiql() -> Html<String> {
    graphiql_source("/graphql")
}

#[catch(404)]
fn not_found() -> ProcessResponder {
    ProcessResponder::error(Status::NotFound, &"No such resource")
}

pub fn ignite<A: 'static + Aggregatrix + Clone>(tx: Tx, rx: Rx, context: A::Context) -> Rocket {
    rocket::ignite()
        .manage(Mutex::new((tx, rx)))
        .mount("/", routes![retrieve, graphiql])
        .mount(
            "/",
            GraphQLHandler::<A> {
                schema: A::schema(),
                context,
            },
        )
        .register(catchers![not_found])
}
