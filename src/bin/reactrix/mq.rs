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

use log::{debug, error};
use rmp_serde as rmp;
use std::sync::mpsc;
use std::thread;
use zmq::{Context, SocketEvent};

pub type Tx = mpsc::Sender<i64>;
pub type Rx = mpsc::Receiver<i64>;
pub type TxError = mpsc::SendError<i64>;

enum Publish {
    Set(i64),
    Send,
}

fn poll(context: &Context) -> Result<mpsc::Sender<Publish>, failure::Error> {
    let (tx, rx) = mpsc::channel::<Publish>();

    let socket = context.socket(zmq::PUB)?;
    socket.monitor("inproc://monitor", SocketEvent::ACCEPTED as i32)?;
    socket.bind("tcp://*:5660")?;
    let mut id = 0;

    thread::spawn(move || {
        for event in rx {
            match event {
                Publish::Set(n) => id = n,
                Publish::Send => {
                    debug!("Notify sequence {}", id);

                    let bytes = match rmp::to_vec(&id) {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            error!("{}", e);
                            continue;
                        }
                    };

                    if let Err(e) = socket
                        .send("sequence", zmq::SNDMORE)
                        .and_then(|_| socket.send(&bytes, 0))
                    {
                        error!("{}", e);
                    }
                }
            }
        }
    });

    Ok(tx)
}

pub fn launch() -> Result<Tx, failure::Error> {
    let (tx, rx) = mpsc::channel::<i64>();
    let context = Context::new();
    let poll_tx = poll(&context)?;
    let poll_tx_monitor = poll_tx.clone();

    let monitor = context.socket(zmq::PAIR)?;
    monitor.connect("inproc://monitor")?;

    thread::spawn(move || {
        for id in rx {
            poll_tx.send(Publish::Set(id)).unwrap();
            poll_tx.send(Publish::Send).unwrap();
        }
    });

    thread::spawn(move || loop {
        let _event = monitor.recv_msg(0);
        let address = monitor.recv_string(0).unwrap().unwrap();

        debug!("New connection at {}", &address);
        poll_tx_monitor.send(Publish::Send).unwrap();
    });

    Ok(tx)
}
