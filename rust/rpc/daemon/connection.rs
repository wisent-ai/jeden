use super::*;

/// A TLS record begins with its content type and its two-byte
/// legacy_record_version: <https://www.rfc-editor.org/rfc/rfc8446#section-5.1>.
const TLS_RECORD_PREFIX: usize = 3;
/// The handshake content type a ClientHello record carries:
/// <https://www.rfc-editor.org/rfc/rfc8446#section-5.1>.
const HANDSHAKE: u8 = 0x16;
/// The major byte of every TLS legacy_record_version:
/// <https://www.rfc-editor.org/rfc/rfc8446#section-5.1>.
const TLS_MAJOR: u8 = 0x03;

impl<B: SessionBackend> HeadlessDaemon<B> {
    /// Serve one connection; a connection that ends on an error is said
    /// with its cause, since the client sees only a dropped socket.
    pub(super) async fn serve_said(&self, stream: TcpStream) {
        if let Err(cause) = self.serve_connection(stream).await {
            eprintln!("jeden daemon: a connection ended on an error: {cause}");
        }
    }

    pub(super) async fn serve_connection(&self, mut stream: TcpStream) -> Result<(), String> {
        let mut record_prefix: [u8; TLS_RECORD_PREFIX] = [Default::default(); TLS_RECORD_PREFIX];
        loop {
            let prefix_len = stream
                .peek(&mut record_prefix)
                .await
                .map_err(|error| format!("TLS preface read failed: {error}"))?;
            if record_prefix[..prefix_len].is_empty() {
                return Err("connection closed before TLS preface".to_string());
            }
            if prefix_len >= record_prefix.len() {
                break;
            }
            tokio::task::yield_now().await;
        }
        if !matches!(record_prefix, [HANDSHAKE, TLS_MAJOR, _]) {
            let rejected = "plaintext or malformed TLS preface rejected";
            return Err(match stream.shutdown().await {
                Ok(()) => rejected.to_string(),
                Err(error) => format!("{rejected}; closing the connection failed: {error}"),
            });
        }
        let (stream, peer) = self.tls.accept(stream).await?;
        let (reader, mut writer) = tokio::io::split(stream);
        let mut reader = BufReader::new(reader);
        let connection = match peer {
            TlsPeer::Certificate(verified) => AuthenticatedConnection {
                identity: self
                    .directory
                    .resolve(&verified)
                    .map_err(|_| "certificate SAN is not mapped".to_string())?,
                trust_generation: verified.trust_generation,
            },
            TlsPeer::Anonymous { trust_generation } => {
                let frame = read_async_frame(&mut reader, self.config.max_frame_bytes).await?;
                let Some(frame) = frame else { return Ok(()) };
                match self.authenticate_member(&frame, trust_generation).await {
                    Ok((connection, response)) => {
                        write_async_frame(&mut writer, &response).await?;
                        connection
                    }
                    Err(response) => {
                        write_async_frame(&mut writer, &response).await?;
                        if let Err(error) = writer.shutdown().await {
                            return Err(format!(
                                "member authentication refused; closing the connection failed: {error}"
                            ));
                        }
                        return Ok(());
                    }
                }
            }
        };
        loop {
            let frame = match read_async_frame(&mut reader, self.config.max_frame_bytes).await {
                Ok(Some(frame)) => frame,
                Ok(None) => return Ok(()),
                Err(error) => {
                    let response = wire_error(
                        Value::Null,
                        ErrorV1 {
                            code: "malformed_frame".into(),
                            message: error,
                            retryable: false,
                            details: json!({}),
                        },
                    );
                    write_async_frame(&mut writer, &response).await?;
                    return Ok(());
                }
            };
            let request: RequestEnvelopeV1 = match serde_json::from_slice(&frame) {
                Ok(request) => request,
                Err(error) => {
                    let response = wire_error(
                        Value::Null,
                        ErrorV1 {
                            code: "malformed_json".into(),
                            message: error.to_string(),
                            retryable: false,
                            details: json!({}),
                        },
                    );
                    write_async_frame(&mut writer, &response).await?;
                    continue;
                }
            };
            let id = Value::String(request.id.clone());
            let response = match self.dispatch(&connection, request) {
                Ok(result) => json!({"id": id, "result": result}),
                Err(error) => wire_error(id, error),
            };
            write_async_frame(&mut writer, &response).await?;
        }
    }
}
/// Refuse a connection the daemon has no capacity to admit. The refusal and
/// the close are each said when they fail: the client then sees only a
/// dropped socket.
pub(super) async fn reject_overloaded(mut stream: TcpStream) {
    let response = wire_error(
        Value::Null,
        ErrorV1 {
            code: "backpressure".into(),
            message: "connection admission capacity exhausted".into(),
            retryable: true,
            details: json!({}),
        },
    );
    if let Err(error) = write_async_frame(&mut stream, &response).await {
        eprintln!("jeden daemon: the backpressure refusal could not be sent: {error}");
    }
    if let Err(error) = stream.shutdown().await {
        eprintln!("jeden daemon: a refused connection could not be closed: {error}");
    }
}

/// Read one newline-delimited frame. The peer sends it, or closes; a client
/// that is slow is still a client, and the frame limit is what protects the
/// process here.
async fn read_async_frame<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    max: usize,
) -> Result<Option<Vec<u8>>, String> {
    let mut frame = Vec::new();
    // A read that appended nothing is the peer closing.
    reader
        .read_until(b'\n', &mut frame)
        .await
        .map_err(|error| error.to_string())?;
    if frame.is_empty() {
        return Ok(None);
    }
    if frame.len() > max {
        return Err(format!("frame exceeds {max} bytes"));
    }
    while matches!(frame.last(), Some(b'\n' | b'\r')) {
        frame.pop();
    }
    if frame.is_empty() {
        return Err("empty frame".into());
    }
    Ok(Some(frame))
}

async fn write_async_frame<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
) -> Result<(), String> {
    let mut encoded = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    encoded.push(b'\n');
    writer
        .write_all(&encoded)
        .await
        .map_err(|error| error.to_string())
}
