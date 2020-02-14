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

use diesel::prelude::*;
use diesel::result::Error as DieselError;
use failure::Fail;
use std::fmt::Debug;

/// Aggregate error type
#[derive(Debug, Fail)]
pub enum AggregatrixError {
    #[fail(display = "Environment variable {} is missing", 0)]
    Var(String),
    #[fail(display = "Database connection failed: {}", 0)]
    Connection(ConnectionError),
    #[fail(display = "Database error: {}", 0)]
    Database(DieselError),
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
