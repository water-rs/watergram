//! Thin TDLib lifecycle layer: client creation plus the dedicated receive
//! thread that forwards `Update`s into an `async_channel` consumed on the UI
//! executor.

use async_channel::Receiver;
use tdlib_rs::enums::Update;

/// Create a TDLib client and spawn the blocking receive loop.
///
/// `tdlib_rs::receive()` blocks the calling thread for up to two seconds and
/// returns `None` when the poll times out with no data, so the pump must loop
/// past `None`s rather than exit. Spontaneous updates are delivered to the UI
/// thread through the returned receiver, while request futures
/// (`functions::*`) resolve through the crate's internal `@extra` observer —
/// which is fed by this same `receive()` loop, so the loop must run for the
/// lifetime of the client. The channel is unbounded and send failures after
/// the receiver is dropped are ignored.
pub fn spawn_client() -> (i32, Receiver<Update>) {
    let client_id = tdlib_rs::create_client();
    let (tx, rx) = async_channel::unbounded::<Update>();
    std::thread::spawn(move || loop {
        let Some((update, _client_id)) = tdlib_rs::receive() else {
            continue;
        };
        if tx.try_send(update).is_err() {
            break;
        }
    });
    (client_id, rx)
}
