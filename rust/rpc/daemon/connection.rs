use super::*;

impl<B: SessionBackend> HeadlessDaemon<B> {
    pub(super) async fn serve_connection(&self, mut stream: TcpStream) -> Result<(), String> {
        let mut record_prefix = [0_u8; 3];
        tokio::time::timeout(self.config.read_timeout, async {
            loop {
                let prefix_len = stream
                    .peek(&mut record_prefix)
                    .await
                    .map_err(|error| format!("TLS preface read failed: {error}"))?;
                if prefix_len == 0 {
                    return Err("connection closed before TLS preface".to_string());
                }
                if prefix_len >= record_prefix.len() {
                    return Ok(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| "TLS preface deadline exceeded".to_string())??;
        if record_prefix[0] != 0x16 || record_prefix[1] != 0x03 {
            let _ = stream.shutdown().await;
            return Err("plaintext or malformed TLS preface rejected".into());
        }
        let (stream, verified) =
            tokio::time::timeout(self.config.read_timeout, self.tls.accept(stream))
                .await
                .map_err(|_| "TLS handshake deadline exceeded".to_string())??;
        let identity = self
            .directory
            .resolve(&verified)
            .map_err(|_| "certificate SAN is not mapped".to_string())?;
        let connection = AuthenticatedConnection {
            identity,
            trust_generation: verified.trust_generation,
        };
        let (reader, mut writer) = tokio::io::split(stream);
        let mut reader = BufReader::new(reader);
        loop {
            let frame = match read_async_frame(
                &mut reader,
                self.config.max_frame_bytes,
                self.config.read_timeout,
            )
            .await
            {
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
                    write_async_frame(&mut writer, &response, self.config.write_timeout).await?;
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
                    write_async_frame(&mut writer, &response, self.config.write_timeout).await?;
                    continue;
                }
            };
            let id = Value::String(request.id.clone());
            let response = match self.dispatch(&connection, request) {
                Ok(result) => json!({"id": id, "result": result}),
                Err(error) => wire_error(id, error),
            };
            write_async_frame(&mut writer, &response, self.config.write_timeout).await?;
        }
    }
}
pub(super) async fn reject_overloaded(mut stream: TcpStream, timeout: Duration) {
    let response = wire_error(
        Value::Null,
        ErrorV1 {
            code: "backpressure".into(),
            message: "connection admission capacity exhausted".into(),
            retryable: true,
            details: json!({"retryAfterMillis": 100}),
        },
    );
    let _ = write_async_frame(&mut stream, &response, timeout).await;
    let _ = stream.shutdown().await;
}

async fn read_async_frame<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    max: usize,
    deadline: Duration,
) -> Result<Option<Vec<u8>>, String> {
    let mut frame = Vec::new();
    let bytes = tokio::time::timeout(deadline, reader.read_until(b'\n', &mut frame))
        .await
        .map_err(|_| "frame read deadline exceeded".to_string())?
        .map_err(|error| error.to_string())?;
    if bytes == 0 {
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
    deadline: Duration,
) -> Result<(), String> {
    let mut encoded = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    encoded.push(b'\n');
    tokio::time::timeout(deadline, writer.write_all(&encoded))
        .await
        .map_err(|_| "frame write deadline exceeded".to_string())?
        .map_err(|error| error.to_string())
}
