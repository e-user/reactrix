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

use crate::schema::*;
use chrono::{DateTime, Utc};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{json, Value};

/// Event store entry (query)
#[derive(Queryable)]
pub struct Event {
    pub sequence: i64,
    pub version: i32,
    pub data: Value,
    pub timestamp: DateTime<Utc>,
}

impl Serialize for Event {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut event = serializer.serialize_struct("Event", 4)?;
        event.serialize_field("sequence", &self.sequence)?;
        event.serialize_field("version", &self.version)?;
        event.serialize_field("data", &self.data)?;
        event.serialize_field("timestamp", &self.timestamp.timestamp())?;
        event.end()
    }
}

/// Event store entry (insert)
#[derive(Insertable, Serialize, Deserialize)]
#[table_name = "events"]
pub struct NewEvent {
    pub version: i32,
    pub data: Value,
}

impl NewEvent {
    pub fn create<T: Serialize>(version: i32, data: T) -> Self {
        Self {
            version,
            data: json!(data),
        }
    }
}

#[doc(hidden)]
#[derive(Queryable, Insertable)]
#[table_name = "datastore"]
pub struct Data {
    pub hash: Vec<u8>,
    pub data: Vec<u8>,
}
