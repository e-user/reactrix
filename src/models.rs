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

use crate::schema::*;
use chrono::{DateTime, Utc};
use serde_json::Value;

#[derive(Queryable)]
pub struct Event {
    pub sequence: i64,
    pub version: i32,
    pub type_: String,
    pub data: Value,
    pub timestamp: DateTime<Utc>,
}

#[derive(Insertable)]
#[table_name = "events"]
pub struct NewEvent {
    pub version: i32,
    pub type_: String,
    pub data: Value,
}

#[derive(Queryable, Insertable)]
#[table_name = "keystore"]
pub struct Key {
    pub hash: Vec<u8>,
    pub key: Vec<u8>,
}
