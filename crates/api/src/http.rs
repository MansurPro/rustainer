//! Enough HTTP/1.1 to serve one page and a JSON API, and no more.
//!
//! Hand-written for the same reason the rest of this engine is: the project
//! owns its mechanisms. But the honest reason it is *small enough* to hand
//! write is that it serves one origin, on loopback, to one page — so the
//! parts of HTTP that are genuinely hard are parts it does not need.
//! Chunked request bodies, pipelining, content negotiation, ranges,
//! multipart, compression and caching are all absent on purpose, and each
//! one is absent because nothing here sends it, not because it was
//! forgotten.
//!
//! What is *not* skipped is the part that makes a parser dangerous:
//! everything is bounded. A header block over [`MAX_HEADERS`], a body over
//! [`MAX_BODY`], a line over [`MAX_LINE`], or a client that opens a
//! connection and says nothing, all end the connection rather than growing
//! a buffer. A server with no limits is a memory bug waiting for a bored
//! person with `nc`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// The whole request head. 16 KiB is more than any browser sends and far
/// less than anything that could hurt.
pub const MAX_HEADERS: usize = 16 * 1024;
/// A single line within it.
pub const MAX_LINE: usize = 8 * 1024;
/// A request body. Ours carry a container spec, never a file.
pub const MAX_BODY: usize = 1024 * 1024;
/// How long a connection may stay silent before we give up on it.
pub const IO_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Request {
    pub method: String,
    /// Path with the query string removed.
    pub path: String,
    /// Raw query string, empty when there was none.
    pub query: String,
    pub body: Vec<u8>,
}

impl Request {
    /// One `name=value` pair from the query string, percent-decoded.
    pub fn param(&self, name: &str) -> Option<String> {
        self.query.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            (k == name).then(|| percent_decode(v))
        })
    }
}

/// Minimal percent-decoding, including `+` as space.
///
/// Written out rather than pulled in: the only values that reach it are an
/// image reference and a container id, and a dependency whose job is to
/// decode six characters is a dependency whose advisories we would have to
/// track forever.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < b.len() => {
                let hex = |c: u8| match c {
                    b'0'..=b'9' => Some(c - b'0'),
                    b'a'..=b'f' => Some(c - b'a' + 10),
                    b'A'..=b'F' => Some(c - b'A' + 10),
                    _ => None,
                };
                match (hex(b[i + 1]), hex(b[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push(h << 4 | l);
                        i += 3;
                    }
                    // Not an escape after all. Kept literally rather than
                    // dropped: a `%` in a tag is a bad reference, and a
                    // bad reference should fail as itself.
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("the request head is larger than {MAX_HEADERS} bytes")]
    HeadTooLarge,
    #[error("the request body is larger than {MAX_BODY} bytes")]
    BodyTooLarge,
    #[error("malformed request line")]
    Malformed,
}

/// Read one request. Returns `Ok(None)` when the peer closed cleanly.
pub fn read_request(stream: &TcpStream) -> Result<Option<Request>, Error> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut r = BufReader::new(stream);

    let mut line = String::new();
    let mut head_bytes = 0;
    if read_line(&mut r, &mut line, &mut head_bytes)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().ok_or(Error::Malformed)?.to_owned();
    let target = parts.next().ok_or(Error::Malformed)?.to_owned();

    let mut content_length = 0usize;
    loop {
        line.clear();
        if read_line(&mut r, &mut line, &mut head_bytes)? == 0 {
            break;
        }
        let t = line.trim_end();
        if t.is_empty() {
            break;
        }
        if let Some((k, v)) = t.split_once(':')
            && k.eq_ignore_ascii_case("content-length")
        {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    if content_length > MAX_BODY {
        return Err(Error::BodyTooLarge);
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        r.read_exact(&mut body)?;
    }

    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_owned(), q.to_owned()),
        None => (target, String::new()),
    };
    Ok(Some(Request {
        method,
        path,
        query,
        body,
    }))
}

fn read_line(
    r: &mut BufReader<&TcpStream>,
    buf: &mut String,
    head_bytes: &mut usize,
) -> Result<usize, Error> {
    buf.clear();
    // `take` bounds ONE line; `head_bytes` bounds the whole head. Both are
    // needed: a thousand short headers is as effective as one long one.
    let mut limited = r.take(MAX_LINE as u64);
    let n = limited.read_line(buf)?;
    *head_bytes += n;
    if *head_bytes > MAX_HEADERS {
        return Err(Error::HeadTooLarge);
    }
    Ok(n)
}

/// Write a response and close. No keep-alive: this server answers a polling
/// page on loopback, where a connection per request costs a syscall and
/// buys the simplicity of never tracking connection state.
pub fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        _ => "Unknown",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Cache-Control: no-store\r\n\
         \r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

pub fn json(stream: &mut TcpStream, status: u16, value: &serde_json::Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    respond(stream, status, "application/json; charset=utf-8", &body)
}

pub fn error(stream: &mut TcpStream, status: u16, message: &str) -> std::io::Result<()> {
    json(stream, status, &serde_json::json!({ "error": message }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decoding_handles_the_three_cases_that_matter() {
        assert_eq!(percent_decode("alpine%3A3.21"), "alpine:3.21");
        assert_eq!(percent_decode("a+b"), "a b");
        // A stray `%` is kept rather than dropped: a `%` in a reference is
        // a bad reference, and it should fail as the thing the user typed.
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn a_query_parameter_is_found_and_decoded() {
        let r = Request {
            method: "GET".into(),
            path: "/api/pull".into(),
            query: "reference=alpine%3A3.21&force=1".into(),
            body: Vec::new(),
        };
        assert_eq!(r.param("reference").as_deref(), Some("alpine:3.21"));
        assert_eq!(r.param("force").as_deref(), Some("1"));
        assert_eq!(r.param("missing"), None);
    }
}
