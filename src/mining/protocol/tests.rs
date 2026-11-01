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
    let worker = connect(TcpStream::connect(address).unwrap(), worker_token, "chainv4", "rig 1", None);

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

    assert_eq!(node.err().as_deref(), Some("a worker without the token, or a hello changed on its way"));
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

/// A node and a worker with something on the path between them, which may change either's line of
/// the handshake: what each side's handshake came to.
fn through(rewrite_hello: impl Fn(&mut Value), rewrite_answer: impl Fn(&mut Value), me: &Hello) -> (Result<Hello, String>, Result<(), String>) {
    let (greeting, challenge) = greeting("chainv4").unwrap();
    let (mut hello, joining) = hello(&greeting, b"secret", "chainv4", me).unwrap();
    rewrite_hello(&mut hello);

    match welcome(b"secret", "chainv4", &challenge, &hello) {
        Ok((said, mut answer, ..)) => {
            rewrite_answer(&mut answer);

            (Ok(said), joining.finish(b"secret", &answer).map(|_| ()))
        }
        Err(why) => (Err(why), Ok(())),
    }
}

#[test]
fn nothing_on_the_path_changes_where_a_worker_s_work_pays() {
    let ours = crate::structs::address::Address::Ed25519([0xa; 32]);
    let theirs = crate::structs::address::Address::Ed25519([0xe; 32]);
    let me = Hello { name: "rig".to_string(), payout: Some(ours), solo: false };
    let (said, finished) = through(|_| {}, |_| {}, &me);

    assert_eq!(said.unwrap(), me, "untouched, it goes through");
    finished.unwrap();

    // Any field of the hello rewritten is a hello the node refuses: the payout, added, dropped;
    // mining alone or not; its name.
    let refused = "a worker without the token, or a hello changed on its way";
    assert_eq!(through(|hello| hello["payout"] = json!(theirs.to_string()), |_| {}, &me).0.unwrap_err(), refused);
    assert_eq!(through(|hello| { hello.as_object_mut().unwrap().remove("payout"); }, |_| {}, &me).0.unwrap_err(), refused);
    assert_eq!(through(|hello| hello["payout"] = json!(theirs.to_string()), |_| {}, &Hello { payout: None, ..me.clone() }).0.unwrap_err(), refused);
    assert_eq!(through(|hello| hello["solo"] = json!(true), |_| {}, &me).0.unwrap_err(), refused);
    assert_eq!(through(|hello| hello["name"] = json!("theirs"), |_| {}, &me).0.unwrap_err(), refused);

    // The answer's payout rewritten is an answer the worker refuses: its proof covers it.
    let (said, finished) = through(|_| {}, |answer| answer["payout"] = json!(theirs.to_string()), &me);
    said.unwrap();
    assert_eq!(finished.unwrap_err(), "the node does not know the token: not the node meant");
    let (_, finished) = through(|_| {}, |answer| { answer.as_object_mut().unwrap().remove("payout"); }, &me);
    finished.unwrap_err();

    // A node that does know the token, and says it pays someone else, is told apart.
    let (greeting, challenge) = greeting("chainv4").unwrap();
    let (hello, joining) = hello(&greeting, b"secret", "chainv4", &me).unwrap();
    let nonce: [u8; 32] = unhex(&hello["nonce"]).unwrap();
    let fields = hello_fields("chainv4-mining/2", "rig", &ours.to_string(), false);
    let answer = json!({ "proof": hex(&mac(b"secret", &[b"node", &challenge, &nonce, &fields, theirs.to_string().as_bytes()])), "payout": theirs.to_string() });
    assert!(joining.finish(b"secret", &answer).err().unwrap().starts_with(PAYOUT_DIFFERS));

    // And later, should its standing say another address.
    assert!(pays_another(&json!({ "payout": theirs.to_string() }), Some(&ours)).unwrap().starts_with(PAYOUT_DIFFERS));
    assert_eq!(pays_another(&json!({ "payout": ours.to_string() }), Some(&ours)), None);
    assert_eq!(pays_another(&json!({ "shares": 1 }), Some(&ours)), None);
}
