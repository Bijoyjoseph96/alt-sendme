use serde::{Deserialize, Serialize};

pub const CONTROL_ALPN: &[u8] = b"altsendme/control/1";

pub const AUTH_LABEL: &[u8] = b"altsendme-device-auth-v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ControlMessage {
    PairingInfo {
        endpoint_id: String,
        display_name: String,
        device_type: String,
        /// OS family exchanged at pair time (`macos`, `linux`, …). Optional for
        /// backward compatibility with older peers.
        #[serde(default)]
        os: String,
        signature: String,
    },
    RememberVote {
        session_id: String,
        vote: RememberVote,
    },
    Invite {
        blob_ticket: String,
        file_count: u32,
        total_size: u64,
        sender_name: String,
    },
    InviteResponse {
        session_id: String,
        response: InviteResponse,
    },
    Recognition {
        signature: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RememberVote {
    Remember,
    No,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InviteResponse {
    /// Control plane received the invite and surfaced it to the UI.
    Delivered,
    Accepted,
    Declined,
}

/// Pairing join payload encoded in QR / paste code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingTicket {
    pub v: u32,
    pub kind: String,
    pub endpoint_id: String,
    pub relay_url: Option<String>,
}

impl PairingTicket {
    pub const KIND: &'static str = "pair";

    pub fn encode(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    pub fn decode(s: &str) -> anyhow::Result<Self> {
        let trimmed = s.trim();
        if let Ok(ticket) = serde_json::from_str::<Self>(trimmed) {
            anyhow::ensure!(ticket.kind == Self::KIND, "not a pairing ticket");
            return Ok(ticket);
        }
        // Allow bare endpoint id hex for manual entry.
        if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(Self {
                v: 1,
                kind: Self::KIND.to_string(),
                endpoint_id: trimmed.to_string(),
                relay_url: None,
            });
        }
        anyhow::bail!("invalid pairing ticket")
    }
}

pub async fn write_message(
    send: &mut (impl tokio::io::AsyncWrite + Unpin),
    message: &ControlMessage,
) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    let body = serde_json::to_vec(message)?;
    const MAX: usize = 1024 * 1024;
    anyhow::ensure!(body.len() <= MAX, "control message too large");
    let len = (body.len() as u32).to_be_bytes();
    send.write_all(&len).await?;
    send.write_all(&body).await?;
    send.flush().await?;
    Ok(())
}

pub async fn read_message(
    recv: &mut (impl tokio::io::AsyncRead + Unpin),
) -> anyhow::Result<ControlMessage> {
    use tokio::io::AsyncReadExt;
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    anyhow::ensure!(len > 0 && len <= 1024 * 1024, "invalid control message length");
    let mut body = vec![0u8; len];
    recv.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body)?)
}
