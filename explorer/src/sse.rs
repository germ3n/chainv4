//! Server-sent events, as a node's GET /stream sends them: lines of `field: value`, each event
//! ending at a blank line. They arrive in pieces of any size, cut anywhere.

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
    use super::*;

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
