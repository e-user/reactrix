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

use super::Aggregatrix;
use failure::Fail;
use log::error;
use std::collections::HashMap;
use std::error::Error;
use std::result::Result;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

pub enum TxEvent<A: Aggregatrix> {
    Store(i64, Result<A::Result, A::Error>),
    Retrieve(i64),
}

pub enum RxEvent<A: Aggregatrix> {
    Result(Result<A::Result, A::Error>),
    NoValue,
}

pub type Tx<A> = mpsc::Sender<TxEvent<A>>;
pub type Rx<A> = mpsc::Receiver<RxEvent<A>>;
pub type TxError<A> = mpsc::SendError<TxEvent<A>>;
pub type Channel<A> = Arc<Mutex<(Tx<A>, Rx<A>)>>;

#[derive(Debug, Fail)]
pub enum RetrieveError {
    #[fail(display = "Tx channel died: {:?}", _0)]
    TxDead(String),
    #[fail(display = "Rx channel died: {:?}", _0)]
    RxDead(mpsc::RecvError),
    #[fail(display = "Couldn't acquire lock: {}", _0)]
    Lock(String),
}

impl<A: Aggregatrix> From<TxError<A>> for RetrieveError {
    fn from(error: TxError<A>) -> Self {
        Self::TxDead(format!("{:?}", error))
    }
}

impl From<mpsc::RecvError> for RetrieveError {
    fn from(error: mpsc::RecvError) -> Self {
        Self::RxDead(error)
    }
}

pub fn retrieve<A: Aggregatrix>(id: i64, channel: Channel<A>) -> Result<RxEvent<A>, RetrieveError> {
    match channel.lock() {
        Ok(guard) => {
            let (tx, rx) = &*guard;

            tx.send(TxEvent::Retrieve(id))?;
            Ok(rx.recv()?)
        }

        Err(e) => Err(RetrieveError::Lock(e.description().to_string())),
    }
}

pub fn launch<A: Aggregatrix + 'static>() -> (Tx<A>, Rx<A>) {
    let mut results = HashMap::<i64, Result<A::Result, A::Error>>::new();
    let (tx_server, rx_server) = mpsc::channel();
    let (tx_client, rx_client) = mpsc::channel();

    thread::spawn(move || {
        for msg in rx_server {
            match msg {
                TxEvent::Store(id, value) => {
                    results.insert(id, value);
                }
                TxEvent::Retrieve(id) => match results.get(&id) {
                    Some(result) => {
                        if let Err(e) = tx_client.send(RxEvent::Result(result.clone())) {
                            error!("{}", e);
                            return;
                        }
                    }
                    None => {
                        if let Err(e) = tx_client.send(RxEvent::NoValue) {
                            error!("{}", e);
                            return;
                        }
                    }
                },
            }
        }
    });

    (tx_server, rx_client)
}
