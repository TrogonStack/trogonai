use std::collections::BTreeMap;

use buffa::{Message, MessageName};
use trogon_proto_preflight::{WireLimits, WireSchemaPool, preflight};

use crate::SessionError;

pub(crate) struct Wire {
    schema: WireSchemaPool,
}

impl Wire {
    pub fn new() -> Result<Self, SessionError> {
        Ok(Self {
            schema: WireSchemaPool::from_descriptor_set_json(
                trogonai_proto::session_contracts::FILE_DESCRIPTOR_SET_JSON,
            )
            .map_err(|_| SessionError::MalformedWire)?,
        })
    }

    pub fn decode<T: Message + MessageName>(&self, original: &[u8]) -> Result<T, SessionError> {
        let descriptor = self
            .schema
            .message_by_name(T::FULL_NAME)
            .ok_or(SessionError::MalformedWire)?;
        preflight(
            &descriptor,
            original,
            WireLimits::new(131072, 24, 4096).map_err(|_| SessionError::Limit)?,
        )
        .map_err(|_| SessionError::MalformedWire)?;
        T::decode_from_slice(original).map_err(|_| SessionError::MalformedWire)
    }
}

pub(crate) struct OriginalFields<'a>(BTreeMap<u32, (u8, &'a [u8])>);

impl<'a> OriginalFields<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, SessionError> {
        let mut offset = 0;
        let mut fields = BTreeMap::new();
        while offset < bytes.len() {
            let tag = read_varint(bytes, &mut offset)?;
            let number = u32::try_from(tag >> 3).map_err(|_| SessionError::MalformedWire)?;
            let wire = (tag & 7) as u8;
            let value = match wire {
                0 => {
                    let start = offset;
                    read_varint(bytes, &mut offset)?;
                    &bytes[start..offset]
                }
                2 => {
                    let length =
                        usize::try_from(read_varint(bytes, &mut offset)?).map_err(|_| SessionError::MalformedWire)?;
                    let start = offset;
                    offset = offset
                        .checked_add(length)
                        .filter(|end| *end <= bytes.len())
                        .ok_or(SessionError::MalformedWire)?;
                    &bytes[start..offset]
                }
                _ => return Err(SessionError::MalformedWire),
            };
            if number == 0 || fields.insert(number, (wire, value)).is_some() {
                return Err(SessionError::MalformedWire);
            }
        }
        Ok(Self(fields))
    }

    pub fn message(&self, number: u32) -> Result<&'a [u8], SessionError> {
        match self.0.get(&number) {
            Some((2, value)) => Ok(value),
            _ => Err(SessionError::MalformedWire),
        }
    }

    pub fn copy(&self, source: u32, destination: u32, output: &mut Vec<u8>) -> Result<(), SessionError> {
        let (wire, value) = self.0.get(&source).ok_or(SessionError::MalformedWire)?;
        write_varint(u64::from(destination) << 3 | u64::from(*wire), output);
        if *wire == 2 {
            write_varint(value.len() as u64, output);
        }
        output.extend_from_slice(value);
        Ok(())
    }

    pub fn copy_optional(&self, source: u32, destination: u32, output: &mut Vec<u8>) -> Result<(), SessionError> {
        if self.0.contains_key(&source) {
            self.copy(source, destination, output)?;
        }
        Ok(())
    }
}

pub(crate) fn bytes_field(number: u32, value: &[u8], output: &mut Vec<u8>) {
    write_varint(u64::from(number) << 3 | 2, output);
    write_varint(value.len() as u64, output);
    output.extend_from_slice(value);
}

pub(crate) fn write_varint(mut value: u64, output: &mut Vec<u8>) {
    while value >= 128 {
        output.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    output.push(value as u8);
}

fn read_varint(bytes: &[u8], offset: &mut usize) -> Result<u64, SessionError> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *bytes.get(*offset).ok_or(SessionError::MalformedWire)?;
        *offset += 1;
        if shift == 63 && byte > 1 {
            return Err(SessionError::MalformedWire);
        }
        value |= u64::from(byte & 127) << shift;
        if byte & 128 == 0 {
            return Ok(value);
        }
    }
    Err(SessionError::MalformedWire)
}
