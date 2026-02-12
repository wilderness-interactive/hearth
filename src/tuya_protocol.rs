use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, BlockDecryptMut, KeyInit};
use std::fmt;

type Aes128EcbEnc = ecb::Encryptor<aes::Aes128>;
type Aes128EcbDec = ecb::Decryptor<aes::Aes128>;

const AES_BLOCK_SIZE: usize = 16;

// Frame markers
pub const PREFIX: u32 = 0x000055AA;
pub const SUFFIX: u32 = 0x0000AA55;

// Sizes
pub const HEADER_SIZE: usize = 16; // prefix(4) + seqno(4) + cmd(4) + length(4)
pub const CRC_SIZE: usize = 4;
pub const SUFFIX_SIZE: usize = 4;
pub const FOOTER_SIZE: usize = CRC_SIZE + SUFFIX_SIZE; // 8
pub const RETCODE_SIZE: usize = 4;

// Command codes
pub const CMD_CONTROL: u32 = 0x07;
#[allow(dead_code)]
pub const CMD_STATUS: u32 = 0x08;
pub const CMD_HEART_BEAT: u32 = 0x09;
pub const CMD_DP_QUERY: u32 = 0x0A;
pub const CMD_UPDATEDPS: u32 = 0x12;

// Version header: "3.3" + 12 zero bytes
const VERSION_HEADER: [u8; 15] = *b"3.3\0\0\0\0\0\0\0\0\0\0\0\0";

// Commands that skip the version header
const NO_HEADER_CMDS: &[u32] = &[CMD_DP_QUERY, CMD_UPDATEDPS, CMD_HEART_BEAT];

// -- Data types --

/// A framed Tuya packet ready to send over TCP.
pub struct TuyaFrame {
    pub bytes: Vec<u8>,
}

/// A parsed Tuya message received from the device.
#[derive(Debug)]
#[allow(dead_code)]
pub struct TuyaMessage {
    pub seqno: u32,
    pub cmd: u32,
    pub retcode: u32,
    pub payload: Vec<u8>,
}

#[derive(Debug)]
pub enum ProtocolError {
    InvalidPrefix(u32),
    InvalidSuffix(u32),
    CrcMismatch { expected: u32, actual: u32 },
    PayloadTooShort,
    DecryptionFailed,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolError::InvalidPrefix(v) => write!(f, "Invalid prefix: {v:#010x}"),
            ProtocolError::InvalidSuffix(v) => write!(f, "Invalid suffix: {v:#010x}"),
            ProtocolError::CrcMismatch { expected, actual } => {
                write!(f, "CRC mismatch: expected {expected:#010x}, got {actual:#010x}")
            }
            ProtocolError::PayloadTooShort => write!(f, "Payload too short"),
            ProtocolError::DecryptionFailed => write!(f, "AES decryption failed"),
        }
    }
}

impl std::error::Error for ProtocolError {}

// -- Pure functions: encryption --

pub fn encrypt_payload(plaintext: &[u8], local_key: &[u8; 16]) -> Vec<u8> {
    // PKCS7 padded size: next multiple of 16
    let padded_len = (plaintext.len() / AES_BLOCK_SIZE + 1) * AES_BLOCK_SIZE;
    let mut buf = vec![0u8; padded_len];
    buf[..plaintext.len()].copy_from_slice(plaintext);

    let encrypted = Aes128EcbEnc::new(local_key.into())
        .encrypt_padded_mut::<Pkcs7>(&mut buf, plaintext.len())
        .expect("buffer is correctly sized for PKCS7 padding");

    encrypted.to_vec()
}

pub fn decrypt_payload(ciphertext: &[u8], local_key: &[u8; 16]) -> Result<Vec<u8>, ProtocolError> {
    let mut buf = ciphertext.to_vec();

    let decrypted = Aes128EcbDec::new(local_key.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|_| ProtocolError::DecryptionFailed)?;

    Ok(decrypted.to_vec())
}

// -- Pure functions: framing --

/// Build a complete 55AA frame for sending to the device.
///
/// For CONTROL: encrypts JSON, prepends "3.3" version header in the clear.
/// For DP_QUERY/HEART_BEAT/UPDATEDPS: encrypts JSON without version header.
pub fn build_frame(seqno: u32, cmd: u32, json_payload: &[u8], local_key: &[u8; 16]) -> TuyaFrame {
    let encrypted = encrypt_payload(json_payload, local_key);

    let payload = if NO_HEADER_CMDS.contains(&cmd) {
        encrypted
    } else {
        let mut buf = Vec::with_capacity(VERSION_HEADER.len() + encrypted.len());
        buf.extend_from_slice(&VERSION_HEADER);
        buf.extend_from_slice(&encrypted);
        buf
    };

    // length = payload + CRC(4) + suffix(4)
    let length = (payload.len() + FOOTER_SIZE) as u32;

    // Assemble everything before the CRC
    let mut frame = Vec::with_capacity(HEADER_SIZE + payload.len() + FOOTER_SIZE);
    frame.extend_from_slice(&PREFIX.to_be_bytes());
    frame.extend_from_slice(&seqno.to_be_bytes());
    frame.extend_from_slice(&cmd.to_be_bytes());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&payload);

    // CRC32 over everything so far
    let crc = crc32fast::hash(&frame);
    frame.extend_from_slice(&crc.to_be_bytes());
    frame.extend_from_slice(&SUFFIX.to_be_bytes());

    TuyaFrame { bytes: frame }
}

/// Parse a raw byte buffer into a TuyaMessage.
/// Validates prefix, suffix, CRC32. Decrypts payload.
pub fn parse_frame(data: &[u8], local_key: &[u8; 16]) -> Result<TuyaMessage, ProtocolError> {
    if data.len() < HEADER_SIZE + FOOTER_SIZE {
        return Err(ProtocolError::PayloadTooShort);
    }

    // Validate prefix
    let prefix = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    if prefix != PREFIX {
        return Err(ProtocolError::InvalidPrefix(prefix));
    }

    let seqno = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let cmd = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
    let length = u32::from_be_bytes([data[12], data[13], data[14], data[15]]) as usize;

    let total_size = HEADER_SIZE + length;
    if data.len() < total_size {
        return Err(ProtocolError::PayloadTooShort);
    }

    // Validate suffix
    let suffix_offset = total_size - SUFFIX_SIZE;
    let suffix = u32::from_be_bytes([
        data[suffix_offset],
        data[suffix_offset + 1],
        data[suffix_offset + 2],
        data[suffix_offset + 3],
    ]);
    if suffix != SUFFIX {
        return Err(ProtocolError::InvalidSuffix(suffix));
    }

    // Validate CRC32
    let crc_offset = suffix_offset - CRC_SIZE;
    let expected_crc = u32::from_be_bytes([
        data[crc_offset],
        data[crc_offset + 1],
        data[crc_offset + 2],
        data[crc_offset + 3],
    ]);
    let actual_crc = crc32fast::hash(&data[..crc_offset]);
    if expected_crc != actual_crc {
        return Err(ProtocolError::CrcMismatch {
            expected: expected_crc,
            actual: actual_crc,
        });
    }

    // Extract retcode and raw payload
    // Device responses: [header:16][retcode:4][encrypted_payload:N][crc:4][suffix:4]
    let retcode = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let raw_payload = &data[HEADER_SIZE + RETCODE_SIZE..crc_offset];

    // Empty payload (e.g. heartbeat response)
    if raw_payload.is_empty() {
        return Ok(TuyaMessage {
            seqno,
            cmd,
            retcode,
            payload: Vec::new(),
        });
    }

    // Check for "3.3" version header in the clear — strip before decrypting
    let ciphertext = if raw_payload.len() >= VERSION_HEADER.len()
        && &raw_payload[..3] == b"3.3"
    {
        &raw_payload[VERSION_HEADER.len()..]
    } else {
        raw_payload
    };

    if ciphertext.is_empty() {
        return Ok(TuyaMessage {
            seqno,
            cmd,
            retcode,
            payload: Vec::new(),
        });
    }

    let payload = decrypt_payload(ciphertext, local_key)?;

    Ok(TuyaMessage {
        seqno,
        cmd,
        retcode,
        payload,
    })
}

// -- Pure functions: JSON payload builders --

pub fn build_dp_query_json(device_id: &str) -> Vec<u8> {
    let ts = timestamp_str();
    serde_json::to_vec(&serde_json::json!({
        "gwId": device_id,
        "devId": device_id,
        "uid": device_id,
        "t": ts,
    }))
    .expect("JSON serialization cannot fail for known-good data")
}

pub fn build_control_json(device_id: &str, dps: &serde_json::Value) -> Vec<u8> {
    let ts = timestamp_str();
    serde_json::to_vec(&serde_json::json!({
        "devId": device_id,
        "uid": device_id,
        "t": ts,
        "dps": dps,
    }))
    .expect("JSON serialization cannot fail for known-good data")
}

pub fn build_heartbeat_json() -> Vec<u8> {
    Vec::new()
}


fn timestamp_str() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key: [u8; 16] = *b"0123456789abcdef";
        let plaintext = b"hello tuya world";

        let encrypted = encrypt_payload(plaintext, &key);
        let decrypted = decrypt_payload(&encrypted, &key).unwrap();
        assert_eq!(&decrypted, plaintext);
    }

    #[test]
    fn build_frame_has_correct_structure() {
        let key: [u8; 16] = *b"0123456789abcdef";
        let json = b"{\"dps\":{\"1\":true}}";

        let frame = build_frame(1, CMD_CONTROL, json, &key);
        let data = &frame.bytes;

        // Check prefix
        let prefix = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        assert_eq!(prefix, PREFIX);

        // Check seqno
        let seqno = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        assert_eq!(seqno, 1);

        // Check command
        let cmd = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
        assert_eq!(cmd, CMD_CONTROL);

        // Check suffix at end
        let suffix = u32::from_be_bytes([
            data[data.len() - 4],
            data[data.len() - 3],
            data[data.len() - 2],
            data[data.len() - 1],
        ]);
        assert_eq!(suffix, SUFFIX);

        // CONTROL frame should have "3.3" version header after the 16-byte header
        assert_eq!(&data[HEADER_SIZE..HEADER_SIZE + 3], b"3.3");
    }

    #[test]
    fn dp_query_frame_has_no_version_header() {
        let key: [u8; 16] = *b"0123456789abcdef";
        let json = build_dp_query_json("test_device");

        let frame = build_frame(2, CMD_DP_QUERY, &json, &key);
        let data = &frame.bytes;

        // DP_QUERY should NOT have "3.3" version header
        assert_ne!(&data[HEADER_SIZE..HEADER_SIZE + 3], b"3.3");
    }

    #[test]
    fn parse_device_response() {
        // Simulate a device response: [header][retcode][version_header + ciphertext][crc][suffix]
        let key: [u8; 16] = *b"0123456789abcdef";
        let json_payload = b"{\"dps\":{\"1\":true,\"6\":55}}";
        let encrypted = encrypt_payload(json_payload, &key);

        // Build a fake device response with retcode
        let mut payload_section = Vec::new();
        payload_section.extend_from_slice(&0u32.to_be_bytes()); // retcode = 0 (success)
        payload_section.extend_from_slice(&VERSION_HEADER);
        payload_section.extend_from_slice(&encrypted);

        let length = (payload_section.len() + FOOTER_SIZE) as u32;

        let mut frame = Vec::new();
        frame.extend_from_slice(&PREFIX.to_be_bytes());
        frame.extend_from_slice(&42u32.to_be_bytes()); // seqno
        frame.extend_from_slice(&CMD_STATUS.to_be_bytes()); // cmd
        frame.extend_from_slice(&length.to_be_bytes());
        frame.extend_from_slice(&payload_section);

        let crc = crc32fast::hash(&frame);
        frame.extend_from_slice(&crc.to_be_bytes());
        frame.extend_from_slice(&SUFFIX.to_be_bytes());

        // Parse it
        let msg = parse_frame(&frame, &key).unwrap();
        assert_eq!(msg.seqno, 42);
        assert_eq!(msg.cmd, CMD_STATUS);
        assert_eq!(msg.retcode, 0);
        assert_eq!(&msg.payload, json_payload);
    }
}
