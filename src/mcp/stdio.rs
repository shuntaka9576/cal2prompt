use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::sync::broadcast;

use tokio::io::AsyncBufReadExt;

#[derive(thiserror::Error, Debug, Clone)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Other error: {0}")]
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    Request {
        jsonrpc: String,
        method: String,
        id: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        params: Option<serde_json::Value>,
    },
    Notification {
        jsonrpc: String,
        method: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        params: Option<serde_json::Value>,
    },
    Response {
        jsonrpc: String,
        id: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<serde_json::Value>,
    },
}

#[allow(dead_code)]
#[async_trait]
pub trait Transport: Send + Sync {
    async fn send(&self, message: Message) -> Result<(), Error>;
    fn receive(&self) -> Pin<Box<dyn Stream<Item = Result<Message, Error>> + Send>>;
    async fn close(&self) -> Result<(), Error>;
}

pub struct StdioTransport {
    stdout: Arc<Mutex<std::io::Stdout>>,
    receiver: broadcast::Receiver<Result<Message, Error>>,
}

impl StdioTransport {
    pub fn new() -> (Self, broadcast::Sender<Result<Message, Error>>) {
        let (sender, receiver) = broadcast::channel(100);
        let transport = Self {
            stdout: Arc::new(Mutex::new(std::io::stdout())),
            receiver,
        };

        let stdin = tokio::io::stdin();
        let mut reader = tokio::io::BufReader::new(stdin);
        let sender_clone = sender.clone();

        tokio::spawn(async move {
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => {
                        let parsed = match serde_json::from_str::<Message>(&line) {
                            Ok(msg) => Ok(msg),
                            Err(e) => Err(Error::Serialization(e.to_string())),
                        };
                        if sender_clone.send(parsed).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = sender_clone.send(Err(Error::Io(e.to_string())));
                        break;
                    }
                }
            }
        });

        (transport, sender)
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn send(&self, message: Message) -> Result<(), Error> {
        let mut stdout = self
            .stdout
            .lock()
            .map_err(|_| Error::Other("Failed to lock stdout".into()))?;
        let json =
            serde_json::to_string(&message).map_err(|e| Error::Serialization(e.to_string()))?;

        writeln!(stdout, "{}", json).map_err(|e| Error::Io(e.to_string()))?;
        stdout.flush().map_err(|e| Error::Io(e.to_string()))?;
        Ok(())
    }

    fn receive(&self) -> Pin<Box<dyn Stream<Item = Result<Message, Error>> + Send>> {
        let rx = self.receiver.resubscribe();
        Box::pin(futures::stream::unfold(rx, |mut rx| async move {
            match rx.recv().await {
                Ok(msg) => Some((msg, rx)),
                Err(_) => None,
            }
        }))
    }

    async fn close(&self) -> Result<(), Error> {
        Ok(())
    }
}
