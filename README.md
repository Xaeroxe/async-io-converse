# async-io-converse [![Build Status]][actions] [![Latest Version]][crates.io]

[Build Status]: https://img.shields.io/github/actions/workflow/status/Xaeroxe/async-io-converse/rust.yml?branch=main
[actions]: https://github.com/Xaeroxe/async-io-converse/actions?query=branch%3Amain
[Latest Version]: https://img.shields.io/crates/v/async-io-converse.svg
[crates.io]: https://crates.io/crates/async-io-converse

[Documentation](https://docs.rs/async-io-converse)

A wrapper over the [`async-io-typed`](https://github.com/Xaeroxe/async-io-typed) crate which allows
[`serde`](https://github.com/serde-rs/serde) compatible types to be sent over any duplex connection that has types that implement
`AsyncRead` and `AsyncWrite`. `async-io-converse` adds the ability to receive replies from the other process.

## Who needs this?

If you have two endpoints that need to communicate with each other and

- You can establish some kind of duplex I/O connection between them (i.e. TCP, named pipes, or a unix socket)
- You need clear message boundaries
- You're not trying to conform to an existing wire format such as HTTP or protobufs. This crate uses a custom format.
- The data you wish to send can be easily represented in a Rust type, and that type implements serde's `Deserialize` and `Serialize` traits.

Then this crate might be useful to you!

## Who doesn't need this?

If the endpoints are in the same process then you should not use this crate. You're better served by existing async mpsc channels.
Many crates provide async mpsc channels, including `futures` and `tokio`. Pick your favorite implementation. Additionally, if you're
trying to interface with a process that doesn't have Rust code, and can't adopt a Rust portion, this crate will hurt much more than
it will help. Consider using protobufs or JSON if Rust adoption is a blocker.

## Why shouldn't I just use `async-io-typed` directly?

`async-io-converse` is built on top of `async-io-typed` and provides the ability to create and send replies, which
the peer can then act on. `async-io-converse` also requires a duplex connection, unlike `async-io-typed`.

## Contributing

Contributions are welcome! Please ensure your changes to the code pass unit tests. If you're fixing a bug please
add a unit test so that someone doesn't un-fix the bug later.