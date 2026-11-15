use std::io::{self, Write};

use buffa::bytes::Bytes;
use buffa::{HasMessageView, Message};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

struct InterruptedWriter {
    bytes: Vec<u8>,
    capacity: usize,
}

impl Write for InterruptedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let accepted = bytes.len().min(self.capacity - self.bytes.len());
        if accepted == 0 && !bytes.is_empty() {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        self.bytes.extend_from_slice(&bytes[..accepted]);
        Ok(accepted)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn assert_write_errors(message: &impl Serialize, expected: &Value) {
    let complete = serde_json::to_vec(message).unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&complete).unwrap(), *expected);
    for capacity in 0..complete.len() {
        let mut writer = InterruptedWriter {
            bytes: Vec::new(),
            capacity,
        };
        let error = serde_json::to_writer(&mut writer, message).unwrap_err();
        assert_eq!(error.io_error_kind(), Some(io::ErrorKind::BrokenPipe));
        assert_eq!(writer.bytes, complete[..capacity]);
    }
    let mut exact = InterruptedWriter {
        bytes: Vec::new(),
        capacity: complete.len(),
    };
    serde_json::to_writer(&mut exact, message).unwrap();
    exact.flush().unwrap();
    assert_eq!(exact.bytes, complete);
}

pub(super) fn assert_message_writes<M>(fixture: &Value)
where
    M: Message + HasMessageView + Serialize + DeserializeOwned,
    for<'a> M::View<'a>: Serialize,
    M::ViewHandle: Serialize,
{
    let message: M = serde_json::from_value(fixture.clone()).unwrap();
    assert_write_errors(&message, fixture);
    let wire = message.encode_to_vec();
    let view = M::decode_view(&wire).unwrap();
    assert_write_errors(&view, fixture);
    drop(view);
    let retained = M::decode_view_handle(Bytes::from(wire)).unwrap();
    assert_write_errors(&retained, fixture);
}
