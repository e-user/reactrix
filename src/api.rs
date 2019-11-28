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

use super::{ApiResult, Encrypted};
use failure::Fail;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::json;
use std::error::Error;
use std::ops::Deref;
use std::result;

#[derive(Debug, Fail)]
#[fail(display = "API error encountered: {}", 0)]
pub struct ApiError(String);

impl Deref for ApiError {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(error: reqwest::Error) -> Self {
        ApiError(error.description().to_string())
    }
}

type Result<T> = result::Result<T, ApiError>;

pub fn generate_key() -> Result<String> {
    let client = reqwest::Client::new();

    match client
        .get("http://localhost:8000/v1/generate-key/")
        .send()?
        .json::<ApiResult<String>>()?
    {
        ApiResult::Ok { data: Some(data) } => Ok(data),
        ApiResult::Ok { data: None } => Err(ApiError("Empty reponse".to_string())),
        ApiResult::Error { reason } => Err(ApiError(reason)),
    }
}

pub fn generate_nonce() -> Result<String> {
    let client = reqwest::Client::new();

    match client
        .get("http://localhost:8000/v1/generate-nonce/")
        .send()?
        .json::<ApiResult<String>>()?
    {
        ApiResult::Ok { data: Some(data) } => Ok(data),
        ApiResult::Ok { data: None } => Err(ApiError("Empty reponse".to_string())),
        ApiResult::Error { reason } => Err(ApiError(reason)),
    }
}

pub fn decrypt<T>(enc: &Encrypted) -> Result<T>
where
    T: DeserializeOwned,
{
    let client = reqwest::Client::new();
    let result = client
        .post(&format!("http://localhost:8000/v1/decrypt/{}", &enc.key_id))
        .json(&json!({
            "nonce": &enc.nonce,
            "data": &enc.data,
        }))
        .send()?
        .json::<ApiResult<T>>()?;

    match result {
        ApiResult::Ok { data: Some(data) } => Ok(data),
        ApiResult::Ok { data: None } => Err(ApiError("Empty reponse".to_string())),
        ApiResult::Error { reason } => Err(ApiError(reason)),
    }
}

pub fn encrypt<T>(key_id: &str, nonce: &str, data: T) -> Result<String>
where
    T: Serialize,
{
    let client = reqwest::Client::new();

    let result = client
        .post(&format!("http://localhost:8000/v1/encrypt/{}", key_id))
        .json(&json!({
            "nonce": nonce,
            "data": data
        }))
        .send()?
        .json::<ApiResult<String>>()?;

    match result {
        ApiResult::Ok { data: Some(data) } => Ok(data),
        ApiResult::Ok { data: None } => Err(ApiError("Empty reponse".to_string())),
        ApiResult::Error { reason } => Err(ApiError(reason)),
    }
}

pub fn create<T>(event: T) -> Result<i32>
where
    T: Serialize,
{
    let client = reqwest::Client::new();

    let result = client
        .post("http://localhost:8000/v1/create")
        .json(&event)
        .send()?
        .json::<ApiResult<i32>>()?;

    match result {
        ApiResult::Ok { data: Some(data) } => Ok(data),
        ApiResult::Ok { data: None } => Err(ApiError("Empty reponse".to_string())),
        ApiResult::Error { reason } => Err(ApiError(reason)),
    }
}
