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

use serde_json::Value;
use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;

pub enum TxEvent {
    Store(i64, Value),
    Retrieve(i64),
}

pub enum RxEvent {
    Result(Value),
    NoValue,
}

pub type Tx = mpsc::Sender<TxEvent>;
pub type Rx = mpsc::Receiver<RxEvent>;
pub type TxError = mpsc::SendError<TxEvent>;

pub fn launch() -> (Tx, Rx) {
    let mut results = HashMap::new();
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
                            println!("{:?}", e);
                            return;
                        }
                    }
                    None => {
                        if let Err(e) = tx_client.send(RxEvent::NoValue) {
                            println!("{:?}", e);
                            return;
                        }
                    }
                },
            }
        }
    });

    (tx_server, rx_client)
}
