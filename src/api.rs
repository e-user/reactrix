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

use super::model::NewEvent;
use failure::Fail;
use log::{debug, warn};
use serde::de::DeserializeOwned;
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

impl<T> From<T> for ApiError
where
    T: std::error::Error,
{
    fn from(error: T) -> Self {
        ApiError(error.to_string())
    }
}

#[derive(Clone)]
pub struct Api(Url);

impl Api {
    pub fn new(url: &str) -> result::Result<Self, ParseError> {
        let url = Url::parse(url)?;
        if url.cannot_be_a_base() {
            Err(ParseError::SetHostOnCannotBeABaseUrl)
        } else {
            Ok(Self(url))
        }
    }

    pub fn store<S: Serialize>(&self, data: &S) -> Result<String> {
        let client = reqwest::blocking::Client::new();
        let url = self.0.clone().join("v1/store").unwrap();

        debug!("store: {}", &url);

        let response = client.put(url).body(serde_json::to_vec(data)?).send()?;

        if response.status().is_success() {
            response.json::<ApiResult<String>>()?.into()
        } else {
            Err(ApiError(response.status().to_string()))
        }
    }

    pub fn retrieve<D: DeserializeOwned>(&self, id: &str) -> Result<D> {
        let client = reqwest::blocking::Client::new();
        let path = format!("v1/retrieve/{}", id);
        let url = self.0.clone().join(&path).unwrap();
        let mut data = Vec::<u8>::new();

        debug!("retrieve: {}", &url);

        let mut response = client.get(url).send()?;

        if response.status().is_success() {
            response.read_to_end(&mut data)?;
            let object = serde_json::from_slice(&data)?;
            Ok(object)
        } else {
            Err(ApiError(response.status().to_string()))
        }
    }

    pub fn create(&self, event: NewEvent) -> Result<f64> {
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

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredObject(String);

impl StoredObject {
    pub fn store<T: Serialize>(api: &Api, data: &T) -> Result<Self> {
        Ok(Self(api.store(data)?))
    }

    pub fn retrieve<D: DeserializeOwned>(&self, api: &Api) -> Option<D> {
        match api.retrieve(&self.0) {
            Ok(data) => Some(data),
            Err(e) => {
                warn!("Couldn't retrieve {}: {}", self.0, e);
                None
            }
        }
    }
}
