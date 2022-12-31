#![doc = include_str!("../README.md")]

use async_io_typed::{AsyncReadTyped, AsyncWriteTyped};
use futures_io::{AsyncRead, AsyncWrite};
use futures_util::{SinkExt, Stream, StreamExt};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, Mutex};

#[derive(Deserialize, Serialize)]
struct InternalMessage<T> {
    user_message: T,
    conversation_id: u64,
    is_reply: bool,
}

/// A message received from the connected peer, which you may choose to reply to.
pub struct ReceivedMessage<W: AsyncWrite + Unpin, T: Serialize + DeserializeOwned + Unpin> {
    message: Option<T>,
    conversation_id: u64,
    raw_write: Arc<Mutex<AsyncWriteTyped<W, InternalMessage<T>>>>,
}

impl<W: AsyncWrite + Unpin, T: Serialize + DeserializeOwned + Unpin> ReceivedMessage<W, T> {
    /// Peeks at the message, panicking if the message had already been taken prior.
    pub fn message(&self) -> &T {
        self.message_opt().expect("message already taken")
    }

    /// Peeks at the message, returning `None` if the message had already been taken prior.
    pub fn message_opt(&self) -> Option<&T> {
        self.message.as_ref()
    }

    /// Pulls the message from this, panicking if the message had already been taken prior.
    pub fn take_message(&mut self) -> T {
        self.take_message_opt().expect("message already taken")
    }

    /// Pulls the message from this, returning `None` if the message had already been taken prior.
    pub fn take_message_opt(&mut self) -> Option<T> {
        self.message.take()
    }

    /// Sends the given message as a reply to this one. There are two ways for the peer to receive this reply
    ///
    /// 1. `.await` both layers of [AsyncWriteConverse::send] or [AsyncWriteConverse::send_timeout]
    /// 2. They'll receive it as the return value of [AsyncWriteConverse::ask] or [AsyncWriteConverse::ask_timeout].
    pub async fn reply(self, reply: T) -> Result<(), Error> {
        SinkExt::send(
            &mut *self.raw_write.lock().await,
            InternalMessage {
                user_message: reply,
                is_reply: true,
                conversation_id: self.conversation_id,
            },
        )
        .await
        .map_err(Into::into)
    }
}

struct ReplySender<T> {
    reply_sender: Option<oneshot::Sender<Result<T, Error>>>,
    conversation_id: u64,
}

/// Errors which can occur on an `async-io-converse` connection.
#[derive(Debug)]
pub enum Error {
    /// Error from `std::io`
    Io(io::Error),
    /// Error from the `bincode` crate
    Bincode(bincode::Error),
    /// A message was received that exceeded the configured length limit
    ReceivedMessageTooLarge,
    /// A message was sent that exceeded the configured length limit
    SentMessageTooLarge,
    /// A reply wasn't received within the timeout specified
    Timeout,
    /// The read half was dropped, crippling the ability to receive replies.
    ReadHalfDropped,
}

impl From<async_io_typed::Error> for Error {
    fn from(e: async_io_typed::Error) -> Self {
        match e {
            async_io_typed::Error::Io(e) => Error::Io(e),
            async_io_typed::Error::Bincode(e) => Error::Bincode(e),
            async_io_typed::Error::ReceivedMessageTooLarge => Error::ReceivedMessageTooLarge,
            async_io_typed::Error::SentMessageTooLarge => Error::SentMessageTooLarge,
        }
    }
}

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

pub fn new_duplex_connection_with_limit<
    T: DeserializeOwned + Serialize + Unpin,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
>(
    size_limit: u64,
    raw_read: R,
    raw_write: W,
) -> (AsyncReadConverse<R, W, T>, AsyncWriteConverse<W, T>) {
    let write = Arc::new(Mutex::new(AsyncWriteTyped::new_with_limit(
        raw_write, size_limit,
    )));
    let write_clone = Arc::clone(&write);
    let (reply_data_sender, reply_data_receiver) = mpsc::unbounded_channel();
    let read = AsyncReadConverse {
        raw: AsyncReadTyped::new_with_limit(raw_read, size_limit),
        raw_write: write_clone,
        reply_data_receiver,
        pending_reply: Vec::new(),
    };
    let write = AsyncWriteConverse {
        raw: write,
        reply_data_sender,
        next_id: 0,
    };
    (read, write)
}

pub fn new_duplex_connection<
    T: DeserializeOwned + Serialize + Unpin,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
>(
    raw_read: R,
    raw_write: W,
) -> (AsyncReadConverse<R, W, T>, AsyncWriteConverse<W, T>) {
    new_duplex_connection_with_limit(1024u64.pow(2), raw_read, raw_write)
}

/// Used to receive messages from the connected peer. ***You must drive this in order to receive replies on [AsyncWriteConverse]***
pub struct AsyncReadConverse<
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    T: Serialize + DeserializeOwned + Unpin,
> {
    raw: AsyncReadTyped<R, InternalMessage<T>>,
    raw_write: Arc<Mutex<AsyncWriteTyped<W, InternalMessage<T>>>>,
    reply_data_receiver: mpsc::UnboundedReceiver<ReplySender<T>>,
    pending_reply: Vec<ReplySender<T>>,
}

impl<
R: AsyncRead + Unpin,
W: AsyncWrite + Unpin,
T: Serialize + DeserializeOwned + Unpin,
> AsyncReadConverse<R, W, T> {
    pub fn inner(&self) -> &R {
        self.raw.inner()
    }
}

impl<
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
        T: Serialize + DeserializeOwned + Unpin + Send + 'static,
    > AsyncReadConverse<R, W, T>
{
    /// Spawns a future onto the tokio runtime that will drive the receive mechanism.
    /// This allows you to receive replies to your messages, while completely ignoring any non-reply messages you get.
    ///
    /// If instead you'd like to see the non-reply messages then you'll need to drive the `Stream` implementation
    /// for `AsyncReadConverse`.
    pub fn drive_forever(mut self) {
        tokio::spawn(async move { while StreamExt::next(&mut self).await.is_some() {} });
    }
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin, T: Serialize + DeserializeOwned + Unpin> Stream
    for AsyncReadConverse<R, W, T>
{
    type Item = Result<ReceivedMessage<W, T>, Error>;

    fn poll_next(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let Self {
            ref mut raw,
            ref mut reply_data_receiver,
            ref mut pending_reply,
            ref raw_write,
        } = self.get_mut();
        loop {
            match futures_core::ready!(Pin::new(&mut *raw).poll_next(cx)) {
                Some(r) => {
                    let i = r?;
                    while let Ok(reply_data) = reply_data_receiver.try_recv() {
                        pending_reply.push(reply_data);
                    }
                    let start_len = pending_reply.len();
                    let mut user_message = Some(i.user_message);
                    pending_reply.retain_mut(|pending_reply| {
                        if let Some(reply_sender) = pending_reply.reply_sender.as_ref() {
                            if reply_sender.is_closed() {
                                return false;
                            }
                        }
                        let matches =
                            i.is_reply && pending_reply.conversation_id == i.conversation_id;
                        if matches {
                            let _ = pending_reply
                                .reply_sender
                                .take()
                                .expect("infallible")
                                .send(Ok(user_message.take().expect("infallible")));
                        }
                        !matches
                    });
                    if start_len == pending_reply.len() {
                        return Poll::Ready(Some(Ok(ReceivedMessage {
                            message: Some(user_message.take().expect("infallible")),
                            conversation_id: i.conversation_id,
                            raw_write: Arc::clone(raw_write),
                        })));
                    } else {
                        continue;
                    }
                },
                None => return Poll::Ready(None),
            }
        }
    }
}

/// Used to send messages to the connected peer. You may optionally receive replies to your messages as well.
///
/// ***You must drive the corresponding [AsyncReadConverse] in order to receive replies to your messages.***
/// You can do this either by driving the `Stream` implementation, or calling [AsyncReadConverse::drive_forever].
pub struct AsyncWriteConverse<W: AsyncWrite + Unpin, T: Serialize + DeserializeOwned + Unpin> {
    raw: Arc<Mutex<AsyncWriteTyped<W, InternalMessage<T>>>>,
    reply_data_sender: mpsc::UnboundedSender<ReplySender<T>>,
    next_id: u64,
}

impl<
W: AsyncWrite + Unpin,
T: Serialize + DeserializeOwned + Unpin,
> AsyncWriteConverse<W, T> {
    pub async fn with_inner<F: FnOnce(&W) -> R, R>(&self, f: F) -> R {
        f(self.raw.lock().await.inner())
    }
}

impl<W: AsyncWrite + Unpin, T: Serialize + DeserializeOwned + Unpin> AsyncWriteConverse<W, T> {
    /// Send a message, and wait for a reply, with the default timeout. Shorthand for `.await`ing both layers of `.send(message)`.
    pub async fn ask(&mut self, message: T) -> Result<T, Error> {
        self.ask_timeout(DEFAULT_TIMEOUT, message).await
    }

    /// Send a message, and wait for a reply, up to timeout. Shorthand for `.await`ing both layers of `.send_timeout(message)`.
    pub async fn ask_timeout(&mut self, timeout: Duration, message: T) -> Result<T, Error> {
        match self.send_timeout(timeout, message).await {
            Ok(fut) => fut.await,
            Err(e) => Err(e),
        }
    }

    /// Sends a message to the peer on the other side of the connection. This returns a future wrapped in a future. You must
    /// `.await` the first layer to send the message, however `.await`ing the second layer is optional. You only need to
    /// `.await` the second layer if you care about the reply to your message. Waits up to the default timeout for a reply.
    pub async fn send(
        &mut self,
        message: T,
    ) -> Result<impl Future<Output = Result<T, Error>>, Error> {
        self.send_timeout(DEFAULT_TIMEOUT, message).await
    }

    /// Sends a message to the peer on the other side of the connection, waiting up to the specified timeout for a reply.
    /// This returns a future wrapped in a future. You must  `.await` the first layer to send the message, however
    /// `.await`ing the second layer is optional. You only need to  `.await` the second layer if you care about the
    /// reply to your message.
    pub async fn send_timeout(
        &mut self,
        timeout: Duration,
        message: T,
    ) -> Result<impl Future<Output = Result<T, Error>>, Error> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        let read_half_dropped = self
            .reply_data_sender
            .send(ReplySender {
                reply_sender: Some(reply_sender),
                conversation_id: self.next_id,
            })
            .is_err();
        SinkExt::send(
            &mut *self.raw.lock().await,
            InternalMessage {
                user_message: message,
                conversation_id: self.next_id,
                is_reply: false,
            },
        )
        .await?;
        self.next_id = self.next_id.wrapping_add(1);
        Ok(async move {
            if read_half_dropped {
                return Err(Error::ReadHalfDropped);
            }
            let res = tokio::time::timeout(timeout, reply_receiver).await;
            match res {
                Ok(Ok(Ok(value))) => Ok(value),
                Ok(Ok(Err(e))) => Err(e),
                Ok(Err(_)) => Err(Error::ReadHalfDropped),
                Err(_) => Err(Error::Timeout),
            }
        })
    }
}
