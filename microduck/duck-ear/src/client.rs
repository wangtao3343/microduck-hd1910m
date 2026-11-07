//! A blocking JSON-RPC line connection to `robotd`.
//!
//! The same wire and the same handshake `robotctl` uses, reduced to what one long-lived
//! listener needs. Blocking `std::net` rather than tokio: this process runs a single loop —
//! read audio, decode, occasionally send a command — and an async runtime would buy it a
//! scheduler it has no use for.
//!
//! The connection is expected to outlive many commands and to be *lost* at least once: an
//! update restarts `robotd`, and a listener that treats that as fatal would stay deaf until
//! someone restarted it. [`Robot::call`] reconnects once on a broken pipe, which is the
//! recovery a robot actually needs.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use duck_ipc_proto::{self as proto, Call, Id, Request, Response};

/// How long to wait for a reply before treating the connection as dead.
///
/// Longer than any call this daemon makes — they are all queued, not executed — and short
/// enough that a wedged `robotd` does not leave the microphone deaf with no log line.
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

pub struct Robot {
    socket: PathBuf,
    connection: Option<Connection>,
    next_id: u64,
}

struct Connection {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Robot {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            connection: None,
            next_id: 1,
        }
    }

    /// Send a discrete call and return its answer.
    ///
    /// A connection that is gone — `robotd` restarted, or it was never up — is retried once
    /// on a fresh socket before the error reaches the caller. One retry, not a loop: the
    /// loop would turn a stopped `robotd` into a spinning listener, and the next command
    /// will try again anyway.
    pub fn call(&mut self, call: &Call) -> Result<Response> {
        match self.send_call(call) {
            Ok(response) => Ok(response),
            Err(first) => {
                self.connection = None;
                self.send_call(call).with_context(|| {
                    format!("robotd at {} (after reconnecting: {first})", self.socket.display())
                })
            }
        }
    }

    /// Send a continuous intent. Failures are logged, not returned: a dropped heartbeat is
    /// the next block's problem, and the deadman is the thing that makes that safe.
    pub fn notify(&mut self, call: &Call) {
        let attempt = (|| -> Result<()> {
            let connection = self.connect()?;
            write(&mut connection.writer, &Request::notify(call))
        })();
        if let Err(error) = attempt {
            tracing::debug!(%error, "could not send a notification");
            self.connection = None;
        }
    }

    fn send_call(&mut self, call: &Call) -> Result<Response> {
        let id = Id::Number(self.next_id);
        self.next_id += 1;
        let connection = self.connect()?;
        write(&mut connection.writer, &Request::call(id.clone(), call))?;

        loop {
            let mut line = String::new();
            let read = connection
                .reader
                .read_line(&mut line)
                .context("reading from robotd")?;
            if read == 0 {
                bail!("robotd closed the connection");
            }
            // Notifications carry no id and may be interleaved on a shared socket; nothing
            // here subscribes, but ignoring them is what keeps a future subscription from
            // corrupting the reply stream.
            if let Ok(request) = serde_json::from_str::<Request>(line.trim())
                && request.is_notification()
            {
                continue;
            }
            let response: Response = serde_json::from_str(line.trim())
                .with_context(|| format!("malformed reply: {}", line.trim()))?;
            if response.id.as_ref() == Some(&id) {
                return Ok(response);
            }
        }
    }

    fn connect(&mut self) -> Result<&mut Connection> {
        if self.connection.is_none() {
            let stream = UnixStream::connect(&self.socket)
                .with_context(|| format!("cannot reach robotd at {}", self.socket.display()))?;
            // A socket that accepts and then says nothing must fail, not hang the listener.
            // Every intent robotd answers is answered from its dispatch loop; nothing here
            // waits on hardware.
            stream
                .set_read_timeout(Some(REPLY_TIMEOUT))
                .context("setting the robotd read timeout")?;
            let writer = stream
                .try_clone()
                .context("cannot split the robotd socket")?;
            self.connection = Some(Connection {
                reader: BufReader::new(stream),
                writer,
            });
            self.hello()?;
            tracing::info!(socket = %self.socket.display(), "connected to robotd");
        }
        Ok(self
            .connection
            .as_mut()
            .expect("just connected in the branch above"))
    }

    /// The liveness handshake `robotctl` sends before its first command. No daemon refuses a
    /// call over the version number; what this buys is that a socket which accepts and then
    /// says nothing fails here rather than on the first real command.
    fn hello(&mut self) -> Result<()> {
        let id = Id::Number(self.next_id);
        self.next_id += 1;
        let connection = self
            .connection
            .as_mut()
            .expect("hello is only called just after connecting");
        write(
            &mut connection.writer,
            &Request::call(
                id.clone(),
                &Call::Hello(proto::HelloParams {
                    api_version: proto::API_VERSION,
                }),
            ),
        )?;
        let mut line = String::new();
        connection
            .reader
            .read_line(&mut line)
            .context("reading the hello reply")?;
        let response: Response =
            serde_json::from_str(line.trim()).context("malformed hello reply")?;
        if let Some(error) = response.error {
            bail!("robotd refused the handshake: {}", error.message);
        }
        Ok(())
    }
}

fn write(writer: &mut UnixStream, request: &Request) -> Result<()> {
    let mut line = serde_json::to_vec(request).context("encoding a request")?;
    line.push(b'\n');
    writer
        .write_all(&line)
        .and_then(|()| writer.flush())
        .context("sending to robotd")
}

/// Turn a reply into "accepted, or why not".
pub fn accepted(response: &Response) -> Result<()> {
    if let Some(error) = &response.error {
        bail!("{}", error.message);
    }
    let result: proto::IntentResult = response
        .result_as()
        .context("reply was not an intent result")?;
    if !result.accepted {
        bail!(
            "{}",
            result.reason.as_deref().unwrap_or("refused without a reason")
        );
    }
    Ok(())
}
