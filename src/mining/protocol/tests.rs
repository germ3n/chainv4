use {
    super::*,
    std::net::{TcpListener, TcpStream},
    std::thread,
};

/// What the node's side of a handshake came to, and the worker's.
type Handshakes = (Result<(Sender, Receiver, Hello), String>, Result<(Sender, Receiver), String>);

/// A node and a worker over a real local connection, each with its token: what each side's
/// handshake came to.
fn pair(node_token: &'static [u8], worker_token: &'static [u8]) -> Handshakes {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let node = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let accepted = accept(stream.try_clone().unwrap(), node_token, "chainv4");

        // A refused worker is hung up on, as the node does.
        if accepted.is_err() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }

        accepted
    });
    let worker = connect(TcpStream::connect(address).unwrap(), worker_token, "chainv4", "rig 1");

    (node.join().unwrap(), worker)
}

#[test]
fn a_worker_with_the_token_talks_both_ways_and_names_itself() {
    let (node, worker) = pair(b"secret", b"secret");
    let (mut to_worker, mut from_worker, hello) = node.unwrap();
    let (mut to_node, mut from_node) = worker.unwrap();

    assert_eq!(hello.name, "rig 1");

    to_worker.send(&json!({ "type": "work", "job": 1 })).unwrap();
    to_worker.send(&json!({ "type": "work", "job": 2 })).unwrap();
    to_node.send(&json!({ "type": "found", "nonce": 7 })).unwrap();

    assert_eq!(from_node.receive().unwrap()["job"], 1);
    assert_eq!(from_node.receive().unwrap()["job"], 2);
    assert_eq!(from_worker.receive().unwrap()["nonce"], 7);
}

#[test]
fn a_wrong_token_gets_nowhere_on_either_side() {
    let (node, worker) = pair(b"secret", b"guess");

    assert_eq!(node.err().as_deref(), Some("a worker without the token"));
    assert!(worker.is_err());

    // A node that does not know the worker's token cannot pass for one: the worker refuses it,
    // since the node's proof is under the token too.
    let challenge = [1; 32];
    let nonce = [2; 32];

    assert!(!checks(b"guess", &[b"node", &challenge, &nonce], &mac(b"secret", &[b"node", &challenge, &nonce])));
}

#[test]
fn a_message_changed_replayed_or_from_the_other_way_does_not_check() {
    let key = [9; 32];
    let text = json!({ "type": "found", "nonce": 7 }).to_string();
    let sealed = mac(&key, &[Way::ToNode.tag(), &0u64.to_le_bytes(), text.as_bytes()]);
    let changed = json!({ "type": "found", "nonce": 8 }).to_string();

    assert!(checks(&key, &[Way::ToNode.tag(), &0u64.to_le_bytes(), text.as_bytes()], &sealed));
    assert!(!checks(&key, &[Way::ToNode.tag(), &0u64.to_le_bytes(), changed.as_bytes()], &sealed), "changed");
    assert!(!checks(&key, &[Way::ToNode.tag(), &1u64.to_le_bytes(), text.as_bytes()], &sealed), "replayed as the next");
    assert!(!checks(&key, &[Way::ToWorker.tag(), &0u64.to_le_bytes(), text.as_bytes()], &sealed), "sent back the other way");
}
