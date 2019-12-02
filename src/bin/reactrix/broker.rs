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

use rmp_serde as rmp;
use std::sync::mpsc;
use std::thread;
use zmq::Context;

pub type Tx = mpsc::Sender<i64>;
pub type Rx = mpsc::Receiver<i64>;
pub type TxError = mpsc::SendError<i64>;

pub fn launch() -> Result<Tx, failure::Error> {
    let (tx, rx) = mpsc::channel::<i64>();
    let context = Context::new();
    let socket = context.socket(zmq::PUB)?;
    socket.bind("tcp://*:5660")?;

    thread::spawn(move || {
        for id in rx {
            let bytes = match rmp::to_vec(&id) {
                Ok(bytes) => bytes,
                Err(e) => {
                    println!("{}", e);
                    continue;
                }
            };

            if let Err(e) = socket
                .send("sequence", zmq::SNDMORE)
                .and_then(|_| socket.send(&bytes, 0))
            {
                println!("{}", e);
            }
        }
    });

    Ok(tx)
}
