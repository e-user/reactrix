// This file is part of reactrix.
//
// Copyright 2020 Alexander Dorn
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

use super::{models, schema};
use blake2::{Blake2s, Digest};
use diesel::prelude::*;
use diesel::result::Error as DieselError;
use failure::Fail;
use rocket_contrib::databases::diesel::PgConnection;

#[derive(Debug, Fail)]
pub enum DataStoreError {
    #[fail(display = "Database error: {}", 0)]
    Database(DieselError),
    #[fail(display = "Record not found")]
    NoRecord,
    #[fail(display = "Identical hash for different data detected: {}", 0)]
    Collision(String),
}

impl From<DieselError> for DataStoreError {
    fn from(error: DieselError) -> Self {
        DataStoreError::Database(error)
    }
}

pub struct DataStore<'a>(&'a PgConnection);

impl<'a> DataStore<'a> {
    pub fn new(conn: &'a PgConnection) -> Self {
        Self(conn)
    }

    pub fn store(&self, data: &[u8]) -> Result<Vec<u8>, DataStoreError> {
        let hash = Blake2s::digest(data);

        if let Ok(stored) = self.retrieve(&hash) {
            if data == &stored[..] {
                return Ok(hash.to_owned().to_vec());
            } else {
                return Err(DataStoreError::Collision(hex::encode(hash)));
            }
        }

        match diesel::insert_into(schema::datastore::table)
            .values(models::Data {
                hash: hash.to_vec(),
                data: data.to_vec(),
            })
            .get_result::<models::Data>(self.0)
        {
            Ok(data) => Ok(data.hash),
            Err(e) => Err(e.into()),
        }
    }

    pub fn retrieve(&self, id: &[u8]) -> Result<Vec<u8>, DataStoreError> {
        use schema::datastore::dsl;

        match dsl::datastore
            .select(dsl::data)
            .filter(dsl::hash.eq(id))
            .first::<Vec<u8>>(self.0)
        {
            Ok(data) => Ok(data),
            Err(DieselError::NotFound) => Err(DataStoreError::NoRecord),
            Err(e) => Err(e.into()),
        }
    }
}
