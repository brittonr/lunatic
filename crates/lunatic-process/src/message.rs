/*!
The [`Message`] is a special variant of a [`Signal`](crate::Signal) that can be sent to
processes. The most common kind of Message is a [`DataMessage`], but there are also some special
kinds of messages, like the [`Message::LinkDied`], that is received if a linked process dies.
*/

use std::{
    any::Any,
    fmt::Debug,
    io::{Read, Write},
    sync::Arc,
};

use lunatic_networking_api::{TcpConnection, TlsConnection};
use tokio::net::UdpSocket;

use crate::runtimes::wasmtime::WasmtimeCompiledModule;

pub type Resource = dyn Any + Send + Sync;

/// Can be sent between processes by being embedded into a  [`Signal::Message`][0]
///
/// A [`Message`] has 2 variants:
/// * Data - Regular message containing a tag, buffer and resources.
/// * LinkDied - A `LinkDied` signal that was turned into a message.
///
/// [0]: crate::Signal
#[derive(Debug)]
pub enum Message {
    Data(DataMessage),
    LinkDied(Option<i64>),
    ProcessDied(u64),
}

impl Message {
    pub fn tag(&self) -> Option<i64> {
        match self {
            Message::Data(message) => message.tag(),
            Message::LinkDied(tag) => *tag,
            Message::ProcessDied(_) => None,
        }
    }

    pub fn process_id(&self) -> Option<u64> {
        match self {
            Message::Data(_) => None,
            Message::LinkDied(_) => None,
            Message::ProcessDied(process_id) => Some(*process_id),
        }
    }

    #[cfg(feature = "metrics")]
    pub fn write_metrics(&self) {
        match self {
            Message::Data(message) => message.write_metrics(),
            Message::LinkDied(_) => {
                metrics::counter!("lunatic.process.messages.link_died.count").increment(1);
            }
            Message::ProcessDied(_) => {}
        }
    }
}

/// A variant of a [`Message`] that has a buffer of data and resources attached to it.
///
/// It implements the [`Read`](std::io::Read) and [`Write`](std::io::Write) traits.
#[derive(Debug, Default)]
pub struct DataMessage {
    tag: Option<i64>,
    read_ptr: usize,
    buffer: Vec<u8>,
    resources: Vec<Option<Arc<Resource>>>,
}

impl DataMessage {
    pub fn tag(&self) -> Option<i64> {
        self.tag
    }

    pub fn buffer(&self) -> &[u8] {
        &self.buffer
    }

    pub fn resources_is_empty(&self) -> bool {
        self.resources.is_empty()
    }

    /// Consumes the message and returns its tag and buffer.
    pub fn into_parts(self) -> (Option<i64>, Vec<u8>) {
        (self.tag, self.buffer)
    }

    /// Create a new message.
    pub fn new(tag: Option<i64>, buffer_capacity: usize) -> Self {
        Self {
            tag,
            read_ptr: 0,
            buffer: Vec::with_capacity(buffer_capacity),
            resources: Vec::new(),
        }
    }

    /// Create a new message from a vec.
    pub fn new_from_vec(tag: Option<i64>, buffer: Vec<u8>) -> Self {
        Self {
            tag,
            read_ptr: 0,
            buffer,
            resources: Vec::new(),
        }
    }

    /// Adds a resource to the message and returns the index of it inside of the message.
    ///
    /// The resource is `Any` and is downcasted when accessing later.
    pub fn add_resource(&mut self, resource: Arc<Resource>) -> usize {
        self.resources.push(Some(resource));
        self.resources.len() - 1
    }

    /// Takes a module from the message, but preserves the indexes of all others.
    ///
    /// If the index is out of bound or the resource is not a module the function will return
    /// None.
    pub fn take_module<T: 'static>(
        &mut self,
        index: usize,
    ) -> Option<Arc<WasmtimeCompiledModule<T>>> {
        self.take_downcast(index)
    }

    /// Takes a TCP stream from the message, but preserves the indexes of all others.
    ///
    /// If the index is out of bound or the resource is not a tcp stream the function will return
    /// None.
    pub fn take_tcp_stream(&mut self, index: usize) -> Option<Arc<TcpConnection>> {
        self.take_downcast(index)
    }

    /// Takes a UDP Socket from the message, but preserves the indexes of all others.
    ///
    /// If the index is out of bound or the resource is not a tcp stream the function will return
    /// None.
    pub fn take_udp_socket(&mut self, index: usize) -> Option<Arc<UdpSocket>> {
        self.take_downcast(index)
    }

    /// Takes a TLS stream from the message, but preserves the indexes of all others.
    ///
    /// If the index is out of bound or the resource is not a tcp stream the function will return
    /// None.
    pub fn take_tls_stream(&mut self, index: usize) -> Option<Arc<TlsConnection>> {
        self.take_downcast(index)
    }

    /// Moves read pointer to index.
    pub fn seek(&mut self, index: usize) {
        self.read_ptr = index;
    }

    pub fn size(&self) -> usize {
        self.buffer.len()
    }

    #[cfg(feature = "metrics")]
    pub fn write_metrics(&self) {
        metrics::counter!("lunatic.process.messages.data.count").increment(1);
        metrics::histogram!("lunatic.process.messages.data.resources.count")
            .record(self.resources.len() as f64);
        metrics::histogram!("lunatic.process.messages.data.size").record(self.size() as f64);
    }

    fn take_downcast<T: Send + Sync + 'static>(&mut self, index: usize) -> Option<Arc<T>> {
        let resource = self.resources.get_mut(index);
        match resource {
            Some(resource_ref) => {
                let resource_any = std::mem::take(resource_ref).map(|resource| resource.downcast());
                match resource_any {
                    Some(Ok(resource)) => Some(resource),
                    Some(Err(resource)) => {
                        *resource_ref = Some(resource);
                        None
                    }
                    None => None,
                }
            }
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_message_has_correct_tag() {
        let msg = DataMessage::new(Some(42), 0);
        assert_eq!(msg.tag(), Some(42));
    }

    #[test]
    fn new_message_none_tag() {
        let msg = DataMessage::new(None, 0);
        assert_eq!(msg.tag(), None);
    }

    #[test]
    fn new_message_buffer_is_empty() {
        let msg = DataMessage::new(Some(1), 64);
        assert!(msg.buffer().is_empty());
        assert_eq!(msg.size(), 0);
    }

    #[test]
    fn new_from_vec_preserves_buffer() {
        let data = vec![1, 2, 3, 4, 5];
        let msg = DataMessage::new_from_vec(Some(10), data.clone());
        assert_eq!(msg.buffer(), &data);
        assert_eq!(msg.size(), 5);
    }

    #[test]
    fn resources_is_empty_on_new_message() {
        let msg = DataMessage::new(None, 0);
        assert!(msg.resources_is_empty());
    }

    #[test]
    fn resources_is_not_empty_after_add() {
        let mut msg = DataMessage::new(None, 0);
        let resource: Arc<Resource> = Arc::new(42_i32);
        msg.add_resource(resource);
        assert!(!msg.resources_is_empty());
    }

    #[test]
    fn into_parts_returns_tag_and_buffer() {
        let data = vec![10, 20, 30];
        let msg = DataMessage::new_from_vec(Some(99), data.clone());
        let (tag, buffer) = msg.into_parts();
        assert_eq!(tag, Some(99));
        assert_eq!(buffer, data);
    }

    #[test]
    fn into_parts_with_none_tag() {
        let msg = DataMessage::new_from_vec(None, vec![1]);
        let (tag, buffer) = msg.into_parts();
        assert_eq!(tag, None);
        assert_eq!(buffer, vec![1]);
    }

    #[test]
    fn write_appends_to_buffer() {
        let mut msg = DataMessage::new(None, 0);
        msg.write_all(&[1, 2, 3]).unwrap();
        msg.write_all(&[4, 5]).unwrap();
        assert_eq!(msg.buffer(), &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn read_returns_buffer_contents() {
        let mut msg = DataMessage::new_from_vec(None, vec![10, 20, 30, 40]);
        let mut buf = [0u8; 4];
        let n = msg.read(&mut buf).unwrap();
        assert_eq!(n, 4);
        assert_eq!(buf, [10, 20, 30, 40]);
    }

    #[test]
    fn read_advances_pointer() {
        let mut msg = DataMessage::new_from_vec(None, vec![1, 2, 3, 4]);
        let mut buf = [0u8; 2];
        msg.read(&mut buf).unwrap();
        assert_eq!(buf, [1, 2]);
        msg.read(&mut buf).unwrap();
        assert_eq!(buf, [3, 4]);
    }

    #[test]
    fn seek_resets_read_position() {
        let mut msg = DataMessage::new_from_vec(None, vec![1, 2, 3]);
        let mut buf = [0u8; 3];
        msg.read(&mut buf).unwrap();
        assert_eq!(buf, [1, 2, 3]);
        msg.seek(0);
        msg.read(&mut buf).unwrap();
        assert_eq!(buf, [1, 2, 3]);
    }

    #[test]
    fn default_message_is_empty() {
        let msg = DataMessage::default();
        assert_eq!(msg.tag(), None);
        assert!(msg.buffer().is_empty());
        assert!(msg.resources_is_empty());
        assert_eq!(msg.size(), 0);
    }

    #[test]
    fn message_enum_tag_delegates_to_data_message() {
        let data = DataMessage::new(Some(77), 0);
        let msg = Message::Data(data);
        assert_eq!(msg.tag(), Some(77));
    }

    #[test]
    fn message_enum_link_died_tag() {
        let msg = Message::LinkDied(Some(55));
        assert_eq!(msg.tag(), Some(55));
    }

    #[test]
    fn message_enum_process_died_has_no_tag() {
        let msg = Message::ProcessDied(123);
        assert_eq!(msg.tag(), None);
        assert_eq!(msg.process_id(), Some(123));
    }

    #[test]
    fn write_then_read_roundtrip() {
        let mut msg = DataMessage::new(Some(42), 0);
        let payload = b"hello lunatic";
        msg.write_all(payload).unwrap();

        assert_eq!(msg.buffer(), payload);
        assert_eq!(msg.size(), payload.len());

        let mut out = vec![0u8; payload.len()];
        msg.read_exact(&mut out).unwrap();
        assert_eq!(out, payload);
    }

    #[test]
    fn into_parts_after_write() {
        let mut msg = DataMessage::new(Some(7), 0);
        msg.write_all(b"abc").unwrap();
        msg.write_all(b"def").unwrap();
        let (tag, buffer) = msg.into_parts();
        assert_eq!(tag, Some(7));
        assert_eq!(buffer, b"abcdef");
    }

    #[test]
    fn resources_not_included_in_into_parts() {
        let mut msg = DataMessage::new(None, 0);
        msg.write_all(b"data").unwrap();
        let resource: Arc<Resource> = Arc::new(String::from("a resource"));
        msg.add_resource(resource);
        assert!(!msg.resources_is_empty());
        let (tag, buffer) = msg.into_parts();
        assert_eq!(tag, None);
        assert_eq!(buffer, b"data");
    }

    #[test]
    fn multiple_resources_tracked() {
        let mut msg = DataMessage::new(None, 0);
        assert!(msg.resources_is_empty());
        let r1: Arc<Resource> = Arc::new(1_u32);
        let r2: Arc<Resource> = Arc::new(2_u32);
        let idx1 = msg.add_resource(r1);
        let idx2 = msg.add_resource(r2);
        assert_eq!(idx1, 0);
        assert_eq!(idx2, 1);
        assert!(!msg.resources_is_empty());
    }

    #[test]
    fn read_empty_buffer_returns_zero() {
        let mut msg = DataMessage::new(None, 0);
        let mut buf = [0u8; 4];
        let n = msg.read(&mut buf).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn partial_read_then_seek_back() {
        let mut msg = DataMessage::new_from_vec(None, vec![10, 20, 30, 40, 50]);
        let mut buf = [0u8; 3];
        msg.read(&mut buf).unwrap();
        assert_eq!(buf, [10, 20, 30]);
        msg.seek(1);
        let mut buf2 = [0u8; 2];
        msg.read(&mut buf2).unwrap();
        assert_eq!(buf2, [20, 30]);
    }

    #[test]
    fn size_reflects_written_data() {
        let mut msg = DataMessage::new(None, 128);
        assert_eq!(msg.size(), 0);
        msg.write_all(&[0; 50]).unwrap();
        assert_eq!(msg.size(), 50);
        msg.write_all(&[0; 30]).unwrap();
        assert_eq!(msg.size(), 80);
    }

    #[test]
    fn data_message_with_negative_tag() {
        let msg = DataMessage::new(Some(-1), 0);
        assert_eq!(msg.tag(), Some(-1));
        let (tag, _) = msg.into_parts();
        assert_eq!(tag, Some(-1));
    }
}

impl Write for DataMessage {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buffer.extend(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Read for DataMessage {
    fn read(&mut self, mut buf: &mut [u8]) -> std::io::Result<usize> {
        let slice = if let Some(slice) = self.buffer.get(self.read_ptr..) {
            slice
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::OutOfMemory,
                "Reading outside message buffer",
            ));
        };
        let bytes = buf.write(slice)?;
        self.read_ptr += bytes;
        Ok(bytes)
    }
}
