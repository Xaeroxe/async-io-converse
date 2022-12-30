# async-io-converse

![ci status](https://github.com/Xaeroxe/async-io-converse/actions/workflows/rust.yml/badge.svg)

A wrapper over the [`async-io-typed`](https://github.com/Xaeroxe/async-io-typed) crate which allows
[`serde`](https://github.com/serde-rs/serde) compatible types to be sent over any duplex connection that has types that implement
`AsyncRead` and `AsyncWrite`. `async-io-converse` adds the ability to receive replies from the other process.

## Who needs this?

Anyone who wishes to send messages between two processes that have a duplex I/O connection, and get replies to those messages.

## Why shouldn't I just use `async-io-typed` directly?

It depends on what you want to send! `async-io-typed` allows you to send Rust types. Specifically, types that are serde-compatible.
`async-io-converse` then adds the ability to receive replies to your typed messages.

## Contributing

Contributions are welcome! Please ensure your changes to the code pass unit tests. If you're fixing a bug please
add a unit test so that someone doesn't un-fix the bug later.