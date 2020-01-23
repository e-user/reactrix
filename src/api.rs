// This file is part of reactrix.
//
// Copyright 2019-2010 Alexander Dorn
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

use failure::Fail;
use log::debug;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::io::Read;
use std::ops::Deref;
use std::result;

type Result<T> = result::Result<T, ApiError>;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ApiResult<T> {
    Ok { data: T },
    Error { reason: String },
}

impl<T> From<ApiResult<T>> for Result<T> {
    fn from(result: ApiResult<T>) -> Self {
        match result {
            ApiResult::Ok { data } => Ok(data),
            ApiResult::Error { reason } => Err(ApiError(reason)),
        }
    }
}

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

impl From<std::io::Error> for ApiError {
    fn from(error: std::io::Error) -> Self {
        ApiError(error.description().to_string())
    }
}

pub fn store(data: &[u8]) -> Result<String> {
    let client = reqwest::blocking::Client::new();

    client
        .put("http://localhost:8000/v1/store")
        .body(data.to_owned())
        .send()?
        .json::<ApiResult<String>>()?
        .into()
}

pub fn retrieve(id: &str) -> Result<Vec<u8>> {
    let client = reqwest::blocking::Client::new();
    let mut data = Vec::<u8>::new();

    client
        .get(&format!("http://localhost:8000/v1/retrieve/{}", id))
        .send()?
        .read_to_end(&mut data)?;

    Ok(data)
}

pub fn create<T>(event: T) -> Result<f64>
where
    T: Serialize,
{
    let client = reqwest::blocking::Client::new();

    client
        .post("http://localhost:8000/v1/create")
        .json(&event)
        .send()?
        .json::<ApiResult<f64>>()?
        .into()
}
