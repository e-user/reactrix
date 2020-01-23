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
use std::io::Read;
use std::ops::Deref;
use std::result;
use url::{ParseError, Url};

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
        ApiError(error.to_string())
    }
}

impl From<std::io::Error> for ApiError {
    fn from(error: std::io::Error) -> Self {
        ApiError(error.to_string())
    }
}

#[derive(Clone)]
pub struct Api(Url);

impl Api {
    pub fn new(url: &str) -> result::Result<Self, ParseError> {
        let url = Url::parse(url)?;
        match url.cannot_be_a_base() {
            true => Err(ParseError::SetHostOnCannotBeABaseUrl),
            false => Ok(Self(url)),
        }
    }

    pub fn store(&self, data: &[u8]) -> Result<String> {
        let client = reqwest::blocking::Client::new();
        let url = self.0.clone().join("v1/store").unwrap();

        debug!("store: {}", &url);

        let response = client.put(url).body(data.to_owned()).send()?;

        if response.status().is_success() {
            response.json::<ApiResult<String>>()?.into()
        } else {
            Err(ApiError(response.status().to_string()))
        }
    }

    pub fn retrieve(&self, id: &str) -> Result<Vec<u8>> {
        let client = reqwest::blocking::Client::new();
        let path = format!("v1/retrieve/{}", id);
        let url = self.0.clone().join(&path).unwrap();
        let mut data = Vec::<u8>::new();

        debug!("retrieve: {}", &url);

        let mut response = client.get(url).send()?;

        if response.status().is_success() {
            response.read_to_end(&mut data)?;
            Ok(data)
        } else {
            Err(ApiError(response.status().to_string()))
        }
    }

    pub fn create<T>(&self, event: T) -> Result<f64>
    where
        T: Serialize,
    {
        let client = reqwest::blocking::Client::new();
        let url = self.0.clone().join("v1/create").unwrap();

        debug!("create: {}", &url);

        let response = client.post(url).json(&event).send()?;

        if response.status().is_success() {
            response.json::<ApiResult<f64>>()?.into()
        } else {
            Err(ApiError(response.status().to_string()))
        }
    }
}
