//! Server-sent events, as a node's GET /stream sends them: lines of `field: value`, each event
//! ending at a blank line. They arrive in pieces of any size, cut anywhere.

use {
    crate::net::{Client, Piece},
    serde_json::Value,
    std::ops::ControlFlow,
    std::sync::atomic::{AtomicBool, Ordering},
    std::sync::{Arc, Mutex},
};

/// What a node's stream said.
#[derive(Debug)]
pub enum News {
    /// It is open: events follow.
    Open,
    Event { name: String, data: Value },
    /// It said nothing of note, only that it is still there.
    Alive,
    /// It ended. Unsupported when the node does not stream at all, so asking again is no use.
    Closed { why: String, unsupported: bool },
}

/// How long a stream may say nothing at all, in seconds, before it is taken for dead. A node says
/// something every 15 s, and a connection a phone left behind on another network never ends by itself.
pub const SILENT: f64 = 40.0;

/// Whether an event of this name means the chain moved, or that it may have, for a light client to
/// follow it now.
pub fn chain_moved(name: &str) -> bool {
    matches!(name, "block" | "reorganized" | "justified" | "finalized" | "lagged")
}

/// Opens a node's stream, GET url, and tells what it says from another thread until it ends or the
/// flag it answers with is set: nothing is sent, a stream being one long answer, and a proxy set
/// that does not build says so rather than going around it.
pub fn listen(client: &Result<Client, String>, url: String, tell: impl Fn(News) + Send + 'static) -> Arc<AtomicBool> {
    let hang_up = Arc::new(AtomicBool::new(false));
    let hung_up = hang_up.clone();
    let parser = Mutex::new(Parser::default());
    let on = move |piece: Result<Piece, String>| {
        if hung_up.load(Ordering::Relaxed) {
            return ControlFlow::Break(());
        }

        match piece {
            Ok(Piece::Head { ok: true, .. }) => tell(News::Open),
            Ok(Piece::Head { status, status_text, .. }) => {
                tell(News::Closed { why: format!("{status} {status_text}"), unsupported: status == 404 });

                return ControlFlow::Break(());
            }
            Ok(Piece::Chunk(bytes)) if bytes.is_empty() => {
                tell(News::Closed { why: "the node closed the stream".to_string(), unsupported: false });

                return ControlFlow::Break(());
            }
            Ok(Piece::Chunk(bytes)) => {
                let events = parser.lock().unwrap_or_else(std::sync::PoisonError::into_inner).feed(&bytes);

                if events.is_empty() {
                    tell(News::Alive);
                }

                for event in events {
                    let data = serde_json::from_str(&event.data).unwrap_or(Value::String(event.data));
                    tell(News::Event { name: event.name, data });
                }
            }
            Err(why) => {
                tell(News::Closed { why, unsupported: false });

                return ControlFlow::Break(());
            }
        }

        ControlFlow::Continue(())
    };

    match client {
        Ok(client) => client.stream(url, on),
        Err(why) => {
            let _ = on(Err(format!("the proxy set does not work: {why}")));
        }
    }

    hang_up
}

/// One event: its name, and its data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub name: String,
    pub data: String,
}

/// Reads events out of a stream that arrives in pieces.
#[derive(Default)]
pub struct Parser {
    /// What came after the last full line.
    pending: Vec<u8>,
    name: String,
    data: Vec<String>,
}

impl Parser {
    /// Takes the next piece and returns the events it completed.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        self.pending.extend_from_slice(bytes);
        let mut events = Vec::new();

        while let Some(end) = self.pending.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);

            if line.is_empty() {
                if !self.data.is_empty() {
                    let name = match std::mem::take(&mut self.name) {
                        name if name.is_empty() => "message".to_string(),
                        name => name,
                    };
                    events.push(Event {
                        name,
                        data: std::mem::take(&mut self.data).join("\n"),
                    });
                }

                self.name.clear();
                continue;
            }

            // A comment, which is how keep-alives come.
            if line.starts_with(':') {
                continue;
            }

            let (field, value) = match line.split_once(':') {
                Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                None => (line, ""),
            };

            match field {
                "event" => self.name = value.to_string(),
                "data" => self.data.push(value.to_string()),
                _ => {}
            }
        }

        events
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        std::io::{BufRead, BufReader, Write},
        std::net::{TcpListener, TcpStream},
        std::sync::mpsc::{Receiver, channel},
        std::time::Duration,
    };

    /// Opens a stream from a node of the test's own, and says what the test should read it by.
    fn opened() -> (TcpStream, Receiver<News>, Arc<AtomicBool>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/stream", listener.local_addr().unwrap());
        let (told, news) = channel();
        let hang_up = listen(&Ok(Client::direct()), url, move |said| {
            let _ = told.send(said);
        });
        let (stream, _) = listener.accept().unwrap();
        let mut request = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();

        while request.read_line(&mut line).unwrap() > 2 {
            line.clear();
        }

        (stream, news, hang_up)
    }

    fn next(news: &Receiver<News>) -> News {
        news.recv_timeout(Duration::from_secs(10)).unwrap()
    }

    #[test]
    fn a_stream_says_it_is_open_each_event_that_it_is_there_and_that_it_ended() {
        let (mut stream, news, _hang_up) = opened();
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n").unwrap();
        assert!(matches!(next(&news), News::Open));

        stream.write_all(b": keep-alive\n\n").unwrap();
        assert!(matches!(next(&news), News::Alive));

        stream.write_all(b"event: block\ndata: {\"height\":7}\n\n").unwrap();
        assert!(matches!(next(&news), News::Event { name, data } if name == "block" && data["height"] == 7));

        drop(stream);
        assert!(matches!(next(&news), News::Closed { unsupported: false, .. }));
    }

    #[test]
    fn hung_up_a_stream_says_no_more() {
        let (mut stream, news, hang_up) = opened();
        stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
        assert!(matches!(next(&news), News::Open));

        hang_up.store(true, Ordering::Relaxed);
        stream.write_all(b"event: block\ndata: 1\n\n").unwrap();
        drop(stream);

        assert!(news.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn only_a_node_that_has_no_stream_is_not_asked_again() {
        for (status, unsupported) in [("404 Not Found", true), ("502 Bad Gateway", false)] {
            let (mut stream, news, _hang_up) = opened();
            write!(stream, "HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n").unwrap();

            assert!(matches!(next(&news), News::Closed { why, unsupported: said } if said == unsupported && why.starts_with(&status[..3])), "{status}");
        }

        let (told, news) = channel();
        let _ = listen(&Err("not a port: 0".to_string()), "http://node/stream".to_string(), move |said| {
            let _ = told.send(said);
        });

        assert!(matches!(next(&news), News::Closed { why, unsupported: false } if why.contains("the proxy set does not work")));
    }

    #[test]
    fn what_says_the_chain_moved_is_what_changes_blocks_and_checkpoints() {
        for name in ["block", "reorganized", "justified", "finalized", "lagged"] {
            assert!(chain_moved(name), "{name}");
        }

        for name in ["queued", "peer_up", "peer_down", "message"] {
            assert!(!chain_moved(name), "{name}");
        }
    }

    #[test]
    fn events_are_read_however_the_stream_is_cut() {
        let stream = b": keep-alive\n\nevent: block\ndata: {\"height\":1}\n\nevent: peer_up\r\ndata: a\r\ndata: b\r\n\r\ndata: plain\n\n";
        let whole = Parser::default().feed(stream);
        let expected = vec![
            Event {
                name: "block".to_string(),
                data: "{\"height\":1}".to_string(),
            },
            Event {
                name: "peer_up".to_string(),
                data: "a\nb".to_string(),
            },
            Event {
                name: "message".to_string(),
                data: "plain".to_string(),
            },
        ];

        assert_eq!(whole, expected);

        for size in 1..stream.len() {
            let mut parser = Parser::default();
            let pieces: Vec<Event> = stream.chunks(size).flat_map(|piece| parser.feed(piece)).collect();

            assert_eq!(pieces, expected, "cut every {size} bytes");
        }
    }
}
