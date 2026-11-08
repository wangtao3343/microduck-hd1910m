//! Just enough HTTP/1.1 to reach two services on the LAN.
//!
//! `duck-ear` talks to the host twice: a chat completion (Ollama) and a WAV (`serve.py`).
//! Both are plain HTTP/1.1 on a known address, one request per connection, no TLS, no
//! redirects, no pooling. `reqwest` would bring an async runtime into a daemon whose whole
//! loop is blocking audio reads; the standard library and thirty lines of framing are the
//! right size.
//!
//! `Connection: close` is sent and the body is read to end of stream, so a reply does not
//! need a correct `Content-Length` to be complete. Chunked framing is still decoded when the
//! server says it is using it, because `read to close` says where the body ends, not how it
//! is encoded. A **deadline**, not a per-read timeout, bounds the whole exchange: a server
//! that drips a byte a second must not hold the microphone closed forever.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// A host and port, parsed once from the command line.
#[derive(Clone, Debug)]
pub struct Service {
    host: String,
    port: u16,
}

impl Service {
    /// Parse `192.168.3.11:11434`. A missing or malformed port is a configuration error and
    /// is reported at startup, not at the first question.
    pub fn parse(addr: &str) -> Result<Self> {
        let (host, port) = addr
            .rsplit_once(':')
            .with_context(|| format!("{addr:?} is not host:port"))?;
        if host.is_empty() {
            bail!("{addr:?} has no host");
        }
        Ok(Self {
            host: host.to_owned(),
            port: port
                .parse()
                .with_context(|| format!("{addr:?} has no valid port"))?,
        })
    }

    pub fn post_json(&self, path: &str, body: &str, timeout: Duration) -> Result<Vec<u8>> {
        self.exchange(
            "POST",
            path,
            Some(("application/json", body.as_bytes())),
            timeout,
        )
    }

    pub fn get(&self, path: &str, timeout: Duration) -> Result<Vec<u8>> {
        self.exchange("GET", path, None, timeout)
    }

    fn exchange(
        &self,
        method: &str,
        path: &str,
        body: Option<(&str, &[u8])>,
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        // `connect_timeout`, not `connect`: a host that is powered off sends nothing back and
        // the kernel's own SYN timeout is over two minutes — long enough to keep the
        // microphone closed for the rest of the evening.
        let address = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .with_context(|| format!("cannot resolve {}:{}", self.host, self.port))?
            .next()
            .with_context(|| format!("{}:{} has no address", self.host, self.port))?;
        let mut stream = TcpStream::connect_timeout(&address, timeout)
            .with_context(|| format!("cannot reach {}:{}", self.host, self.port))?;
        stream.set_write_timeout(Some(timeout))?;

        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n",
            self.host, self.port
        );
        if let Some((kind, _)) = body {
            request.push_str(&format!("Content-Type: {kind}\r\n"));
        }
        if let Some((_, bytes)) = body {
            request.push_str(&format!("Content-Length: {}\r\n", bytes.len()));
        }
        request.push_str("\r\n");

        // Head and body in one `write_all` so a small POST is one packet.
        let mut packet = request.into_bytes();
        if let Some((_, bytes)) = body {
            packet.extend_from_slice(bytes);
        }
        stream
            .write_all(&packet)
            .with_context(|| format!("sending the request to {}:{}", self.host, self.port))?;

        let raw = read_to_deadline(&mut stream, deadline)?;
        split_response(raw)
    }
}

/// Read until the server closes, or until the deadline passes.
///
/// `set_read_timeout` is re-armed with the *remaining* time on every read, so one slow trickle
/// still ends at the deadline rather than `timeout` after the last byte.
fn read_to_deadline(stream: &mut TcpStream, deadline: Instant) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            bail!("the reply did not finish in time");
        }
        stream.set_read_timeout(Some(left))?;
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(data),
            Ok(n) => data.extend_from_slice(&chunk[..n]),
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error).context("reading the reply"),
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Status line, headers, then the body — erroring on a non-2xx with the body in the message,
/// because that is where Ollama puts its complaints.
fn split_response(raw: Vec<u8>) -> Result<Vec<u8>> {
    let end = find(&raw, b"\r\n\r\n").context("the reply has no end of headers")?;
    let head = std::str::from_utf8(&raw[..end]).context("the reply headers are not UTF-8")?;
    let body = &raw[end + 4..];

    let status: u16 = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .context("the reply has no status code")?;
    if !(200..300).contains(&status) {
        let detail: String = String::from_utf8_lossy(body).chars().take(300).collect();
        bail!("HTTP {status}: {detail}");
    }

    if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        dechunk(body)
    } else {
        Ok(body.to_vec())
    }
}

fn dechunk(mut rest: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let end = find(rest, b"\r\n").context("chunk header is not terminated")?;
        let line = std::str::from_utf8(&rest[..end]).context("chunk size is not text")?;
        let size = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16)
            .context("chunk size is not hex")?;
        rest = &rest[end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if rest.len() < size + 2 {
            bail!("chunk ended early");
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_reply_is_just_the_body() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi".to_vec();
        assert_eq!(split_response(raw).unwrap(), b"hi");
    }

    #[test]
    fn an_error_carries_the_servers_explanation() {
        let raw = b"HTTP/1.1 404 Not Found\r\n\r\nmodel not found".to_vec();
        let error = split_response(raw).unwrap_err().to_string();
        assert!(error.contains("404"), "{error}");
        assert!(error.contains("model not found"), "{error}");
    }

    #[test]
    fn chunked_bodies_are_unwrapped() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"
            .to_vec();
        assert_eq!(split_response(raw).unwrap(), b"hello world");
    }

    #[test]
    fn an_address_without_a_port_is_refused_at_startup() {
        assert!(Service::parse("192.168.3.11").is_err());
        assert!(Service::parse("").is_err());
        assert!(Service::parse("192.168.3.11:11434").is_ok());
    }
}
