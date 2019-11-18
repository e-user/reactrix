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

use super::{models, schema};
use blake2::{Blake2s, Digest};
use diesel::prelude::*;
use diesel::result::Error as DieselError;
use failure::Fail;
use rocket_contrib::databases::diesel::PgConnection;
use sodiumoxide::crypto::secretbox;
use sodiumoxide::crypto::secretbox::{Key, Nonce};

#[derive(Debug, Fail)]
pub enum KeyStoreError {
    #[fail(display = "Database error: {}", 0)]
    Database(DieselError),
    #[fail(display = "Key not found")]
    NoKey,
    #[fail(display = "Decryption failed")]
    Decryption,
    #[fail(display = "Illegal key")]
    IllegalKey,
    #[fail(display = "Illegal nonce")]
    IllegalNonce,
}

impl From<DieselError> for KeyStoreError {
    fn from(error: DieselError) -> Self {
        KeyStoreError::Database(error)
    }
}

pub struct KeyStore<'a>(&'a PgConnection);

impl<'a> KeyStore<'a> {
    pub fn new(conn: &'a PgConnection) -> Self {
        Self(conn)
    }

    pub fn generate_key(&self) -> Result<Vec<u8>, KeyStoreError> {
        let key = secretbox::gen_key();
        let hash = Blake2s::digest(&key[..]);

        match diesel::insert_into(schema::keystore::table)
            .values(models::Key {
                hash: hash[..].to_vec(),
                key: key[..].to_vec(),
            })
            .get_result::<models::Key>(self.0)
        {
            Ok(key) => Ok(key.hash),
            Err(e) => Err(e.into()),
        }
    }

    pub fn generate_nonce() -> Nonce {
        secretbox::gen_nonce()
    }

    fn get_key(&self, id: &[u8]) -> Result<Key, KeyStoreError> {
        use schema::keystore::dsl;

        match dsl::keystore
            .select(dsl::key)
            .filter(dsl::hash.eq(id))
            .first::<Vec<u8>>(self.0)
        {
            Ok(key) => Key::from_slice(&key).ok_or(KeyStoreError::IllegalKey),
            Err(DieselError::NotFound) => Err(KeyStoreError::NoKey),
            Err(e) => Err(e.into()),
        }
    }

    pub fn delete_key(&self, id: &[u8]) -> Result<(), KeyStoreError> {
        use schema::keystore::dsl;

        match diesel::delete(dsl::keystore)
            .filter(dsl::hash.eq(id))
            .execute(self.0)
        {
            Ok(_) => Ok(()),
            Err(DieselError::NotFound) => Err(KeyStoreError::NoKey),
            Err(e) => Err(e.into()),
        }
    }

    pub fn encrypt(&self, id: &[u8], nonce: &[u8], data: &[u8]) -> Result<Vec<u8>, KeyStoreError> {
        let key = self.get_key(id)?;
        let nonce = Nonce::from_slice(nonce).ok_or(KeyStoreError::IllegalNonce)?;
        Ok(secretbox::seal(data, &nonce, &key))
    }

    pub fn decrypt(&self, id: &[u8], nonce: &[u8], data: &[u8]) -> Result<Vec<u8>, KeyStoreError> {
        let key = self.get_key(id)?;
        let nonce = Nonce::from_slice(nonce).ok_or(KeyStoreError::IllegalNonce)?;
        secretbox::open(data, &nonce, &key).or(Err(KeyStoreError::Decryption))
    }
}
