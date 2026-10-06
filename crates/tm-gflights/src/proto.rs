//! A minimal protobuf wire codec: varints and length-delimited fields,
//! the only two wire types Google Flights' `tfs`, `tfu` and price
//! tokens use. No schema: a message is an ordered list of fields.

use base64::{
    Engine as _,
    engine::general_purpose::{URL_SAFE_NO_PAD, URL_SAFE_NO_PAD_INDIFFERENT},
};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProtoError {
    #[error("truncated input at byte {0}")]
    Truncated(usize),
    #[error("varint longer than 10 bytes at byte {0}")]
    VarintTooLong(usize),
    #[error("unsupported wire type {wire} for field {field}")]
    WireType { field: u32, wire: u8 },
    #[error("field number 0 at byte {0}")]
    FieldZero(usize),
    #[error("invalid base64: {0}")]
    Base64(String),
}

/// A field's value on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Value {
    /// Wire type 0.
    Varint(u64),
    /// Wire type 2: a string, bytes or a nested message.
    Bytes(Vec<u8>),
}

/// One field: its number and value. Messages keep fields in wire order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Field {
    pub number: u32,
    pub value: Value,
}

impl Field {
    pub fn varint(number: u32, v: u64) -> Self {
        Self {
            number,
            value: Value::Varint(v),
        }
    }

    pub fn bytes(number: u32, b: impl Into<Vec<u8>>) -> Self {
        Self {
            number,
            value: Value::Bytes(b.into()),
        }
    }

    pub fn message(number: u32, fields: &[Field]) -> Self {
        Self::bytes(number, encode(fields))
    }

    pub fn as_varint(&self) -> Option<u64> {
        match self.value {
            Value::Varint(v) => Some(v),
            Value::Bytes(_) => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match &self.value {
            Value::Bytes(b) => Some(b),
            Value::Varint(_) => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        self.as_bytes().and_then(|b| std::str::from_utf8(b).ok())
    }
}

pub fn write_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Reads a varint at `*pos`, advancing it.
pub fn read_varint(input: &[u8], pos: &mut usize) -> Result<u64, ProtoError> {
    let start = *pos;
    let mut result: u64 = 0;
    for i in 0..10 {
        let Some(&byte) = input.get(*pos) else {
            return Err(ProtoError::Truncated(*pos));
        };
        *pos += 1;
        let bits = u64::from(byte & 0x7f);
        // The tenth byte may only carry the top bit of a u64.
        if i == 9 && bits > 1 {
            return Err(ProtoError::VarintTooLong(start));
        }
        result |= bits << (7 * i);
        if byte & 0x80 == 0 {
            return Ok(result);
        }
    }
    Err(ProtoError::VarintTooLong(start))
}

pub fn encode(fields: &[Field]) -> Vec<u8> {
    let mut out = Vec::new();
    for field in fields {
        match &field.value {
            Value::Varint(v) => {
                write_varint(&mut out, u64::from(field.number) << 3);
                write_varint(&mut out, *v);
            }
            Value::Bytes(b) => {
                write_varint(&mut out, (u64::from(field.number) << 3) | 2);
                write_varint(&mut out, b.len() as u64);
                out.extend_from_slice(b);
            }
        }
    }
    out
}

pub fn decode(input: &[u8]) -> Result<Vec<Field>, ProtoError> {
    let mut pos = 0;
    let mut fields = Vec::new();
    while pos < input.len() {
        let key_at = pos;
        let key = read_varint(input, &mut pos)?;
        let wire = (key & 7) as u8;
        let number = u32::try_from(key >> 3)
            .map_err(|_| ProtoError::FieldZero(key_at))?;
        if number == 0 {
            return Err(ProtoError::FieldZero(key_at));
        }
        let value = match wire {
            0 => Value::Varint(read_varint(input, &mut pos)?),
            2 => {
                let len = read_varint(input, &mut pos)?;
                let len = usize::try_from(len)
                    .map_err(|_| ProtoError::Truncated(pos))?;
                let end =
                    pos.checked_add(len).ok_or(ProtoError::Truncated(pos))?;
                let bytes =
                    input.get(pos..end).ok_or(ProtoError::Truncated(pos))?;
                pos = end;
                Value::Bytes(bytes.to_vec())
            }
            _ => {
                return Err(ProtoError::WireType {
                    field: number,
                    wire,
                });
            }
        };
        fields.push(Field { number, value });
    }
    Ok(fields)
}

/// URL-safe base64 without padding, as in `tfs=`.
pub fn b64_encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Accepts the URL-safe and the standard alphabet, with or without
/// padding (`tfs` uses the first, price tokens the second).
pub fn b64_decode(text: &str) -> Result<Vec<u8>, ProtoError> {
    let normalized: String = text
        .trim()
        .chars()
        .filter(|c| *c != '=')
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            other => other,
        })
        .collect();
    URL_SAFE_NO_PAD_INDIFFERENT
        .decode(normalized.as_bytes())
        .map_err(|e| ProtoError::Base64(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_boundaries() {
        for v in [0, 1, 127, 128, 300, 16_383, 16_384, u64::MAX] {
            let mut out = Vec::new();
            write_varint(&mut out, v);
            let mut pos = 0;
            assert_eq!(read_varint(&out, &mut pos), Ok(v));
            assert_eq!(pos, out.len());
        }
        let mut out = Vec::new();
        write_varint(&mut out, u64::MAX);
        assert_eq!(out.len(), 10);
        assert_eq!(out.last(), Some(&1));
    }

    #[test]
    fn rejects_overlong_and_truncated() {
        let mut pos = 0;
        assert!(read_varint(&[0xff; 11], &mut pos).is_err());
        let mut pos = 0;
        assert_eq!(
            read_varint(&[0x80], &mut pos),
            Err(ProtoError::Truncated(1))
        );
        assert!(decode(&[0x0a, 0x05, b'a']).is_err());
        assert!(decode(&[0x0b]).is_err(), "wire type 3 is not supported");
    }

    #[test]
    fn decodes_standard_base64_with_padding() {
        let bytes = b64_decode("CAISA0JSTBoECOzcGA==").unwrap_or_default();
        assert!(!bytes.is_empty());
        assert_eq!(b64_decode("CAISA0JSTBoECOzcGA").ok(), Some(bytes));
    }
}
