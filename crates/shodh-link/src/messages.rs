//! What the phone and the desktop say to each other, and how it is framed.
//!
//! One iroh bidirectional stream per connection carries length-prefixed JSON
//! frames: a 4-byte big-endian length, then that many bytes of JSON. A frame
//! larger than [`MAX_FRAME_BYTES`] is refused on both sides: the writer
//! never sends one, and the reader refuses the length before reading (or
//! allocating) the body.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The version of the link: the Shodh version. The phone and the desktop must
/// be the same version, because the bridge forwards the desktop's own
/// commands (the app crate's test checks this equals the app version).
pub const LINK_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The largest frame either side sends or accepts: 4 MiB.
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// What the phone sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Request {
    /// The first frame of a paired phone's connection.
    Hello {
        version: String,
        device_name: String,
    },
    /// The first frame of a phone that pairs: the one-time secret of the QR
    /// code it scanned ([`crate::pairing::PairingOffer`]), in base32.
    Pair {
        version: String,
        device_name: String,
        secret: String,
    },
    /// Run one of the desktop's commands (only those on
    /// [`crate::allowlist::ALLOWED`]). `args` are the command's arguments as
    /// the desktop's own window sends them (camelCase keys).
    Invoke {
        command: String,
        args: Value,
        call_id: u64,
    },
    /// The agent sessions whose events the phone wants; replaces the earlier
    /// list. Events of every other session are never sent.
    Subscribe { session_ids: Vec<String> },
}

/// What the desktop sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Response {
    /// The phone is paired (or was just paired) and may send requests.
    Welcome { desktop: String },
    /// The answer to [`Request::Invoke`] `call_id`: the command's value, or
    /// its error as the desktop's window would receive it (for refusals,
    /// `{ "code": "refused", "message": … }`).
    Result {
        call_id: u64,
        value: Result<Value, Value>,
    },
    /// A desktop event (`agent_event` of a subscribed session).
    Event { name: String, payload: Value },
    /// The phone runs another Shodh version than the desktop (`desktop`).
    /// The connection closes after it.
    VersionMismatch { desktop: String },
    /// The connection is refused (an unknown or removed phone, a pairing
    /// code that expired or was used). Plain words for the phone to show; the
    /// connection closes after it.
    Refused { reason: String },
}

/// Why a frame could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("a frame of {0} bytes is larger than the {MAX_FRAME_BYTES}-byte limit")]
    TooLarge(usize),
    #[error("the frame is not valid: {0}")]
    Invalid(#[from] serde_json::Error),
    #[error("the connection failed: {0}")]
    Io(#[from] std::io::Error),
    /// The other side finished the stream between frames.
    #[error("the connection was closed")]
    Closed,
}

/// `message` as a frame: its length (big-endian `u32`), then its JSON.
pub fn encode_frame<T: Serialize>(message: &T) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(message)?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(body.len()));
    }
    let length = u32::try_from(body.len()).map_err(|_| FrameError::TooLarge(body.len()))?;
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

/// Write `message` as one frame. Nothing is written when it is too large.
pub async fn write_frame<W, T>(writer: &mut W, message: &T) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let frame = encode_frame(message)?;
    writer.write_all(&frame).await?;
    writer.flush().await?;
    Ok(())
}

/// Read one frame. A length over [`MAX_FRAME_BYTES`] is refused before its
/// body is read. A stream finished cleanly between frames is
/// [`FrameError::Closed`].
pub async fn read_frame<R, T>(reader: &mut R) -> Result<T, FrameError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut header = [0u8; 4];
    match reader.read_exact(&mut header).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(FrameError::Closed),
        Err(e) => return Err(FrameError::Io(e)),
    }
    let length = u32::from_be_bytes(header) as usize;
    if length > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(length));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn requests() -> Vec<Request> {
        vec![
            Request::Hello {
                version: LINK_VERSION.to_string(),
                device_name: "Pixel 8".into(),
            },
            Request::Pair {
                version: LINK_VERSION.to_string(),
                device_name: "Pixel 8".into(),
                secret: "ABCDEFGH".into(),
            },
            Request::Invoke {
                command: "agent_send".into(),
                args: json!({ "sessionId": "s1", "text": "What is Self-RAG?" }),
                call_id: 7,
            },
            Request::Subscribe {
                session_ids: vec!["s1".into(), "s2".into()],
            },
        ]
    }

    fn responses() -> Vec<Response> {
        vec![
            Response::Welcome {
                desktop: LINK_VERSION.to_string(),
            },
            Response::Result {
                call_id: 7,
                value: Ok(json!("run-1")),
            },
            Response::Result {
                call_id: 8,
                value: Err(json!({ "code": "refused", "message": "no" })),
            },
            Response::Event {
                name: "agent_event".into(),
                payload: json!({ "sessionId": "s1", "event": { "type": "text_delta" } }),
            },
            Response::VersionMismatch {
                desktop: "9.9.9".into(),
            },
            Response::Refused {
                reason: "This phone is not paired.".into(),
            },
        ]
    }

    #[tokio::test]
    async fn every_message_round_trips_through_a_frame() {
        for request in requests() {
            let frame = encode_frame(&request).unwrap();
            let back: Request = read_frame(&mut frame.as_slice()).await.unwrap();
            assert_eq!(back, request);
        }
        for response in responses() {
            let frame = encode_frame(&response).unwrap();
            let back: Response = read_frame(&mut frame.as_slice()).await.unwrap();
            assert_eq!(back, response);
        }
    }

    #[test]
    fn the_wire_names_are_the_ones_the_phone_reads() {
        let invoke = serde_json::to_value(&requests()[2]).unwrap();
        assert_eq!(invoke["type"], "invoke");
        assert_eq!(invoke["callId"], 7);
        let hello = serde_json::to_value(&requests()[0]).unwrap();
        assert_eq!(hello["deviceName"], "Pixel 8");
        let result = serde_json::to_value(&responses()[1]).unwrap();
        assert_eq!(result["type"], "result");
        assert_eq!(result["value"], json!({ "Ok": "run-1" }));
        let mismatch = serde_json::to_value(&responses()[4]).unwrap();
        assert_eq!(mismatch["type"], "version_mismatch");
    }

    #[tokio::test]
    async fn an_over_cap_frame_is_refused() {
        // Writing: a message over the cap is never sent.
        let huge = Response::Result {
            call_id: 1,
            value: Ok(Value::String("x".repeat(MAX_FRAME_BYTES))),
        };
        let mut sink = Vec::new();
        assert!(matches!(
            write_frame(&mut sink, &huge).await,
            Err(FrameError::TooLarge(n)) if n > MAX_FRAME_BYTES
        ));
        assert!(sink.is_empty(), "nothing is written");

        // Reading: a length over the cap is refused before the body is read,
        // so the reader never waits for (or allocates) the body.
        let length = (MAX_FRAME_BYTES as u32) + 1;
        let header = length.to_be_bytes();
        let result: Result<Request, _> = read_frame(&mut header.as_slice()).await;
        assert!(matches!(result, Err(FrameError::TooLarge(n)) if n == MAX_FRAME_BYTES + 1));

        // A frame exactly at the cap is read.
        let at_cap = Value::String("y".repeat(MAX_FRAME_BYTES - 2));
        let frame = encode_frame(&at_cap).unwrap();
        assert_eq!(frame.len(), 4 + MAX_FRAME_BYTES);
        let back: Value = read_frame(&mut frame.as_slice()).await.unwrap();
        assert_eq!(back, at_cap);
    }

    #[tokio::test]
    async fn a_stream_closed_between_frames_is_closed_not_an_error() {
        let empty: &[u8] = &[];
        let result: Result<Request, _> = read_frame(&mut &*empty).await;
        assert!(matches!(result, Err(FrameError::Closed)));
        // Closed inside a frame is an error.
        let mut cut = encode_frame(&requests()[0]).unwrap();
        cut.truncate(cut.len() - 1);
        let result: Result<Request, _> = read_frame(&mut cut.as_slice()).await;
        assert!(matches!(result, Err(FrameError::Io(_))));
    }
}
