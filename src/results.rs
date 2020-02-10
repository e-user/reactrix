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
use log::{debug, error};
use std::collections::HashMap;
use std::ops::Deref;
use std::result::Result;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
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

impl<'a> From<PoisonError<MutexGuard<'a, i64>>> for RetrieveError {
    fn from(error: PoisonError<MutexGuard<'a, i64>>) -> Self {
        Self::Lock(error.to_string())
    }
}

#[derive(Clone)]
pub struct Channel<A: Aggregatrix>(Arc<Mutex<(Tx<A>, Rx<A>)>>);

impl<A: Aggregatrix> Deref for Channel<A> {
    type Target = Arc<Mutex<(Tx<A>, Rx<A>)>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<A: Aggregatrix> Channel<A> {
    pub fn new(tx: Tx<A>, rx: Rx<A>) -> Self {
        Channel(Arc::new(Mutex::new((tx, rx))))
    }
}

#[derive(Clone)]
pub struct Results<A: Aggregatrix + 'static> {
    pub channel: Channel<A>,
    pub sequence: Arc<(Mutex<i64>, Condvar)>,
}

impl<A: Aggregatrix> Results<A> {
    pub fn wait_for(&self, id: i64) -> Result<RxEvent<A>, RetrieveError> {
        let (sequence, condvar) = &*self.sequence;
        let mut sequence = sequence.lock()?;

        debug!("Waiting for {}", id);

        while *sequence < id {
            sequence = condvar.wait(sequence)?;
        }

        self.retrieve(id)
    }

    pub fn retrieve(&self, id: i64) -> Result<RxEvent<A>, RetrieveError> {
        match self.channel.lock() {
            Ok(guard) => {
                let (tx, rx) = &*guard;

                tx.send(TxEvent::Retrieve(id))?;
                Ok(rx.recv()?)
            }

            Err(e) => Err(RetrieveError::Lock(e.to_string())),
        }
    }
}

pub fn launch<A: Aggregatrix + 'static>() -> Results<A> {
    let sequence = Arc::new((Mutex::new(-1), Condvar::new()));
    let sequence_clone = sequence.clone();
    let mut results = HashMap::<i64, Result<A::Result, A::Error>>::new();
    let (tx_server, rx_server) = mpsc::channel();
    let (tx_client, rx_client) = mpsc::channel();

    thread::spawn(move || {
        let (sequence, condvar) = &*sequence;

        for msg in rx_server {
            match msg {
                TxEvent::Store(id, value) => {
                    debug!("Storing {}", id);
                    results.insert(id, value);
                    match sequence.lock() {
                        Ok(mut sequence) => {
                            debug!("Notifying {}", id);
                            *sequence = id;
                            condvar.notify_one();
                        }
                        Err(e) => {
                            error!("Couldn't lock mutex: {}", e);
                        }
                    }
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

    Results {
        channel: Channel::new(tx_server, rx_client),
        sequence: sequence_clone,
    }
}
