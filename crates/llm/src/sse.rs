//! Server-Sent Events parser that keeps the `event:` name (Anthropic relies on it).

#[derive(Debug, Clone, PartialEq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
    after_cr: bool,
}

impl SseParser {
    /// Append `bytes` with CR, LF, and CRLF all normalized to LF. A trailing CR
    /// is remembered so a CRLF split across chunks stays one line ending.
    fn push_normalized(&mut self, bytes: &[u8]) {
        for &b in bytes {
            match b {
                b'\r' => self.buf.push(b'\n'),
                b'\n' if self.after_cr => {}
                _ => self.buf.push(b),
            }
            self.after_cr = b == b'\r';
        }
    }

    /// The next complete event block, decoded once whole so multibyte
    /// characters split across chunks survive.
    fn next_block(&mut self) -> Option<String> {
        let i = self.buf.windows(2).position(|w| w == b"\n\n")?;
        let raw: Vec<u8> = self.buf.drain(..i + 2).collect();
        Some(String::from_utf8_lossy(&raw).into_owned())
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.push_normalized(bytes);
        let mut out = vec![];
        while let Some(raw) = self.next_block() {
            if let Some(e) = parse(&raw) {
                out.push(e);
            }
        }
        out
    }
}

fn parse(raw: &str) -> Option<SseEvent> {
    let mut event = String::from("message");
    let mut data = vec![];
    for line in raw.lines() {
        if let Some(d) = line.strip_prefix("data:") {
            data.push(d.strip_prefix(' ').unwrap_or(d));
        } else if let Some(e) = line.strip_prefix("event:") {
            event = e.trim().to_string();
        }
    }
    (!data.is_empty()).then(|| SseEvent {
        event,
        data: data.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_event_names_across_chunks() {
        let mut p = SseParser::default();
        assert!(p.feed(b"event: message_start\ndata: {\"a\"").is_empty());
        let e = p.feed(b":1}\n\nevent: ping\ndata: {}\r\n\r\ndata: [DONE]\n\n");
        assert_eq!(
            e[0],
            SseEvent {
                event: "message_start".into(),
                data: "{\"a\":1}".into()
            }
        );
        assert_eq!(e[1].event, "ping");
        assert_eq!(
            e[2],
            SseEvent {
                event: "message".into(),
                data: "[DONE]".into()
            }
        );
    }

    /// A bare CR is a line ending. `CR CR` dispatchs one event.
    #[test]
    fn bare_cr_ends_an_event() {
        let mut p = SseParser::default();
        let e = p.feed(b"event: ping\rdata: hello\r\r");
        assert!(
            e.len() == 1 && e[0].event == "ping" && e[0].data == "hello",
            "{e:?}"
        );
    }

    #[test]
    fn multibyte_character_split_across_chunks() {
        let mut p = SseParser::default();
        let bytes = "data: 東京\n\n".as_bytes();
        assert!(p.feed(&bytes[..7]).is_empty());
        let e = p.feed(&bytes[7..]);
        assert_eq!(e[0].data, "東京");
    }
}
