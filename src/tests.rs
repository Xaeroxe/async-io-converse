use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use futures_io::{AsyncRead, AsyncWrite};
use futures_util::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::{self, Receiver};
use tokio_util::sync::PollSender;

// What follows is an intentionally obnoxious `AsyncRead` and `AsyncWrite` implementation. Please don't use this outside of tests.
struct BasicChannelSender {
    sender: PollSender<Vec<u8>>,
}

impl AsyncWrite for BasicChannelSender {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<futures_io::Result<usize>> {
        if futures_core::ready!(self.sender.poll_reserve(cx)).is_err() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "remote hung up",
            )));
        }
        let write_len = buf.len();
        self.sender
            .send_item(buf.to_vec())
            .expect("receiver hung up!");
        Poll::Ready(Ok(write_len))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<futures_io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<futures_io::Result<()>> {
        self.sender.close();
        Poll::Ready(Ok(()))
    }
}

struct BasicChannelReceiver {
    receiver: Receiver<Vec<u8>>,
    last_received: Vec<u8>,
}

impl AsyncRead for BasicChannelReceiver {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<futures_io::Result<usize>> {
        let mut len_written = 0;
        loop {
            if self.last_received.len() > 0 {
                let copy_len = self.last_received.len().min(buf.len() - len_written);
                buf[len_written..(len_written + copy_len)]
                    .copy_from_slice(&self.last_received[..copy_len]);
                self.last_received = self.last_received.split_off(copy_len);
                len_written += copy_len;
                if len_written == buf.len() {
                    return Poll::Ready(Ok(buf.len()));
                }
            } else {
                self.last_received = match self.receiver.poll_recv(cx) {
                    Poll::Ready(Some(v)) => v,
                    Poll::Ready(None) => {
                        return if len_written > 0 {
                            Poll::Ready(Ok(len_written))
                        } else {
                            Poll::Pending
                        }
                    }
                    Poll::Pending => {
                        return if len_written > 0 {
                            Poll::Ready(Ok(len_written))
                        } else {
                            Poll::Pending
                        }
                    }
                }
            }
        }
    }
}

fn basic_channel() -> (BasicChannelSender, BasicChannelReceiver) {
    let (sender, receiver) = mpsc::channel(32);
    (
        BasicChannelSender {
            sender: PollSender::new(sender),
        },
        BasicChannelReceiver {
            receiver,
            last_received: Vec::new(),
        },
    )
}

// This tests our testing equipment, just makes sure the above implementations are correct.
#[tokio::test(flavor = "multi_thread")]
async fn basic_channel_test() {
    {
        let (mut sender, mut receiver) = basic_channel();
        let message = "Hello World!".as_bytes();
        let mut read_buf = vec![0; message.len()];
        let write = tokio::spawn(async move { sender.write_all(message).await });
        tokio::time::timeout(Duration::from_secs(2), receiver.read_exact(&mut read_buf))
            .await
            .unwrap()
            .unwrap();
        write.await.unwrap().unwrap();
        assert_eq!(message, read_buf);
    }
    {
        let (sender, mut receiver) = basic_channel();
        let mut sender = Some(sender);
        for _ in 0..10 {
            let message = (0..255).collect::<Vec<u8>>();
            let mut read_buf = vec![0; message.len()];
            let message_clone = message.clone();
            let mut sender_inner = sender.take().unwrap();
            let write = tokio::spawn(async move {
                sender_inner.write_all(&message_clone).await.unwrap();
                sender_inner
            });
            tokio::time::timeout(Duration::from_secs(2), receiver.read_exact(&mut read_buf))
                .await
                .unwrap()
                .unwrap();
            sender = Some(write.await.unwrap());
            assert_eq!(message, read_buf);
        }
    }
}

use std::{
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::Instant,
};

use serde::{Deserialize, Serialize};

use futures_util::StreamExt;
use rand::Rng;

use super::*;

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum TestMessage {
    HelloThere,
    GeneralKenobiYouAreABoldOne,
}

#[tokio::test]
async fn basic_dialogue() {
    let (server_write, client_read) = basic_channel();
    let (client_write, server_read) = basic_channel();
    let (server_read, mut server_write) =
        new_duplex_connection(ChecksumEnabled::Yes, server_read, server_write);
    let (mut client_read, _client_write) =
        new_duplex_connection(ChecksumEnabled::Yes, client_read, client_write);
    server_read.drive_forever();
    tokio::spawn(async move {
        while let Some(message) = client_read.next().await {
            let mut received_message = message.unwrap();
            let message = received_message.take_message();
            match message {
                TestMessage::HelloThere => received_message
                    .reply(TestMessage::GeneralKenobiYouAreABoldOne)
                    .await
                    .unwrap(),
                TestMessage::GeneralKenobiYouAreABoldOne => panic!("Wait, that's my line!"),
            }
        }
    });
    assert_eq!(
        server_write.ask(TestMessage::HelloThere).await.unwrap(),
        TestMessage::GeneralKenobiYouAreABoldOne
    );
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum IdentifiableMessage {
    FromServer(u32),
    FromClient(u32),
}

#[tokio::test(flavor = "multi_thread")]
async fn flurry_of_communication() {
    const TEST_DURATION: Duration = Duration::from_secs(1);
    const TEST_COUNT: u32 = 5;
    let tests_complete = Arc::new(AtomicU32::new(0));
    let start = Instant::now();
    for _ in 0..TEST_COUNT {
        let tests_complete = Arc::clone(&tests_complete);
        tokio::spawn(async move {
            let (server_write, client_read) = basic_channel();
            let (client_write, server_read) = basic_channel();
            let (mut server_read, mut server_write) =
                new_duplex_connection(ChecksumEnabled::Yes, server_read, server_write);
            let (mut client_read, mut client_write) =
                new_duplex_connection(ChecksumEnabled::Yes, client_read, client_write);
            tokio::spawn(async move {
                while let Some(message) = client_read.next().await {
                    let mut received_message = message.unwrap();
                    let message = received_message.take_message();
                    match message {
                        IdentifiableMessage::FromServer(u) => received_message
                            .reply(IdentifiableMessage::FromClient(u))
                            .await
                            .unwrap(),
                        IdentifiableMessage::FromClient(_) => panic!(
                            "Received message from client as client, this should never happen"
                        ),
                    }
                }
            });
            tokio::spawn(async move {
                while let Some(message) = server_read.next().await {
                    let mut received_message = message.unwrap();
                    let message = received_message.take_message();
                    match message {
                        IdentifiableMessage::FromClient(u) => received_message
                            .reply(IdentifiableMessage::FromServer(u))
                            .await
                            .unwrap(),
                        IdentifiableMessage::FromServer(_) => panic!(
                            "Received message from server as server, this should never happen"
                        ),
                    }
                }
            });
            let start = Instant::now();
            while start.elapsed() < TEST_DURATION {
                let code = rand::thread_rng().gen::<u32>();
                if rand::thread_rng().gen::<bool>() {
                    assert_eq!(
                        server_write
                            .ask(IdentifiableMessage::FromServer(code))
                            .await
                            .unwrap(),
                        IdentifiableMessage::FromClient(code)
                    );
                } else {
                    assert_eq!(
                        client_write
                            .ask(IdentifiableMessage::FromClient(code))
                            .await
                            .unwrap(),
                        IdentifiableMessage::FromServer(code)
                    );
                }
            }
            tests_complete.fetch_add(1, Ordering::Relaxed);
        });
    }
    while tests_complete.load(Ordering::Relaxed) < TEST_COUNT && start.elapsed() < TEST_DURATION * 2
    {
    }
    assert!(start.elapsed() >= TEST_DURATION);
    assert!(start.elapsed() < TEST_DURATION * 2);
}

#[tokio::test]
async fn timeout_check() {
    let (server_write, client_read) = basic_channel();
    let (client_write, server_read) = basic_channel();
    let (server_read, mut server_write) =
        new_duplex_connection(ChecksumEnabled::Yes, server_read, server_write);
    let (mut client_read, _client_write) =
        new_duplex_connection(ChecksumEnabled::Yes, client_read, client_write);
    server_read.drive_forever();
    tokio::spawn(async move {
        while let Some(message) = client_read.next().await {
            let mut received_message = message.unwrap();
            let message = received_message.take_message();
            match message {
                TestMessage::HelloThere => received_message
                    .reply(TestMessage::GeneralKenobiYouAreABoldOne)
                    .await
                    .unwrap(),
                TestMessage::GeneralKenobiYouAreABoldOne => {
                    // Do nothing.
                }
            }
        }
    });
    let start = Instant::now();
    let timeout = Duration::from_secs(1);
    assert!(matches!(
        server_write
            .ask_timeout(timeout, TestMessage::GeneralKenobiYouAreABoldOne)
            .await,
        Err(Error::Timeout)
    ));
    let elapsed = start.elapsed();
    assert!(elapsed < timeout * 2);
    assert!(elapsed >= timeout);
    assert!(matches!(
        server_write
            .ask_timeout(timeout, TestMessage::HelloThere)
            .await,
        Ok(TestMessage::GeneralKenobiYouAreABoldOne)
    ));
}
