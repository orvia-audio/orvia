//! Exercise the Linux audio client's connection lifetime without opening a device.
#![cfg(target_os = "linux")]

use pulseaudio::{Client, protocol};
use std::{
    io::{BufReader, ErrorKind, Read},
    os::unix::net::UnixStream,
    thread,
    time::Duration,
};

#[test]
fn dropping_the_last_idle_client_closes_its_socket() -> anyhow::Result<()> {
    let (client_socket, peer) = UnixStream::pair()?;
    let bound = Some(Duration::from_secs(3));
    client_socket.set_read_timeout(bound)?;
    peer.set_read_timeout(bound)?;
    let client =
        thread::spawn(move || Client::new_unix(c"orvia-test", client_socket, None::<&[u8]>));
    let mut peer = BufReader::new(peer);
    let version = protocol::MAX_VERSION;
    let (seq, auth) = protocol::read_command_message(&mut peer, version)?;
    assert!(matches!(auth, protocol::Command::Auth(_)));
    protocol::write_reply_message(
        peer.get_mut(),
        seq,
        &protocol::AuthReply {
            version,
            ..Default::default()
        },
        version,
    )?;
    let (seq, name) = protocol::read_command_message(&mut peer, version)?;
    assert!(matches!(name, protocol::Command::SetClientName(_)));
    protocol::write_reply_message(
        peer.get_mut(),
        seq,
        &protocol::SetClientNameReply { client_id: 1 },
        version,
    )?;

    let client = client.join().unwrap()?;
    let last_owner = client.clone();
    drop(client);
    // A quiet, authenticated connection stays open as long as one owner remains.
    peer.get_ref()
        .set_read_timeout(Some(Duration::from_millis(100)))?;
    let mut byte = [0];
    assert!(matches!(
        peer.read(&mut byte).unwrap_err().kind(),
        ErrorKind::WouldBlock | ErrorKind::TimedOut
    ));
    drop(last_owner);
    peer.get_ref().set_read_timeout(bound)?;
    assert_eq!(peer.read(&mut byte)?, 0);
    Ok(())
}
