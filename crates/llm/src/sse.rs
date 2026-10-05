//! Server-Sent Events parser that keeps the `event:` name (Anthropic relies on it).

#[derive(Debug, Clone, PartialEq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

#[derive(Default)]
pub struct SseParser {
    buf: String,
}

impl SseParser {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buf.push_str(&String::from_utf8_lossy(bytes));
        if self.buf.contains('\r') {
            self.buf = self.buf.replace("\r\n", "\n");
        }
        let mut out = vec![];
        while let Some(i) = self.buf.find("\n\n") {
            let raw: String = self.buf.drain(..i + 2).collect();
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
}
