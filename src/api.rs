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

use super::ApiResult;
use failure::Fail;
use serde::Serialize;
use std::error::Error;
use std::io::Read;
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

impl From<std::io::Error> for ApiError {
    fn from(error: std::io::Error) -> Self {
        ApiError(error.description().to_string())
    }
}

type Result<T> = result::Result<T, ApiError>;

pub fn store(data: &[u8]) -> Result<String> {
    let client = reqwest::Client::new();

    Ok(client
        .put("http://localhost:8000/v1/store")
        .body(data.to_owned())
        .send()?
        .text()?)
}

pub fn retrieve(id: &str) -> Result<Vec<u8>> {
    let client = reqwest::Client::new();
    let mut data = Vec::<u8>::new();

    client
        .get(&format!("http://localhost:8000/v1/retrieve/{}", id))
        .send()?
        .read(&mut data)?;

    Ok(data)
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
