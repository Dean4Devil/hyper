//! Pieces pertaining to the HTTP message protocol.
use std::borrow::Cow;
use std::fmt;
use std::marker::PhantomData;
use std::sync::mpsc;

use tick;

use header::Connection;
use header::ConnectionOption::{KeepAlive, Close};
use header::Headers;
use method::Method;
use net::{Streaming, Paused};
use uri::RequestUri;
use version::HttpVersion;
use version::HttpVersion::{Http10, Http11};

#[cfg(feature = "serde-serialization")]
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub use self::conn::{Conn, Handler};
pub use self::events::Read;

pub mod conn;
mod events;
pub mod h1;
//pub mod h2;

// pub enum Transfer { Http11(h1::Transfer), Http2(h2::Transfer) }
pub use self::h1::Transfer;

/// Marker used with http::Transfer to define its Writer semantics.
#[derive(Debug)]
pub enum Request {}
/// Marker used with http::Transfer to define its Writer semantics.
#[derive(Debug)]
pub enum Response {}

/// An Incoming Message head. Includes request/status line, and headers.
#[derive(Debug)]
pub struct Incoming<S> {
    /// HTTP version of the message.
    pub version: HttpVersion,
    /// Subject (request line or status line) of Incoming message.
    pub subject: S,
    /// Headers of the Incoming message.
    pub headers: Headers
}

/// An incoming request message.
pub type IncomingRequest = Incoming<(Method, RequestUri)>;

/// An incoming response message.
pub type IncomingResponse = Incoming<RawStatus>;


/// The raw status code and reason-phrase.
#[derive(Clone, PartialEq, Debug)]
pub struct RawStatus(pub u16, pub Cow<'static, str>);

#[cfg(feature = "serde-serialization")]
impl Serialize for RawStatus {
    fn serialize<S>(&self, serializer: &mut S) -> Result<(), S::Error> where S: Serializer {
        (self.0, &self.1).serialize(serializer)
    }
}

#[cfg(feature = "serde-serialization")]
impl Deserialize for RawStatus {
    fn deserialize<D>(deserializer: &mut D) -> Result<RawStatus, D::Error> where D: Deserializer {
        let representation: (u16, String) = try!(Deserialize::deserialize(deserializer));
        Ok(RawStatus(representation.0, Cow::Owned(representation.1)))
    }
}

/// Checks if a connection should be kept alive.
#[inline]
pub fn should_keep_alive(version: HttpVersion, headers: &Headers) -> bool {
    trace!("should_keep_alive( {:?}, {:?} )", version, headers.get::<Connection>());
    match (version, headers.get::<Connection>()) {
        (Http10, None) => false,
        (Http10, Some(conn)) if !conn.contains(&KeepAlive) => false,
        (Http11, Some(conn)) if conn.contains(&Close)  => false,
        _ => true
    }
}

pub struct Stream {
    tx: mpsc::Sender<StreamState>,
    transfer: ::tick::Transfer,
    buf: Vec<u8>,
}

impl fmt::Debug for Stream {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("Stream")
    }
}

enum StreamState {
    Paused,
    Reading(Box<Read + Send +'static>)
}

impl Stream {
    fn new(tx: mpsc::Sender<StreamState>, transfer: ::tick::Transfer, buf: Vec<u8>) -> Stream {
        Stream {
            tx: tx,
            transfer: transfer,
            buf: buf,
        }
    }

    pub fn read(&mut self, mut on_read: Box<Read + Send + 'static>) {
        if !self.buf.is_empty() {
            trace!("buffer not empty, on_data that first");
            on_read.on_data(&self.buf);
            self.buf.truncate(0);
            on_read.on_eof();
            return;
        }
        let _ = self.tx.send(StreamState::Reading(on_read));
        let _ = self.transfer.resume();
    }

    pub fn pause(&mut self) {
        let _ = self.tx.send(StreamState::Paused);
        let _ = self.transfer.pause();
    }
}

pub struct AsyncWriter {
    transfer: ::tick::Transfer,
}

impl AsyncWriter {
    pub fn new(transfer: ::tick::Transfer) -> AsyncWriter {
        AsyncWriter { transfer: transfer }
    }

    pub fn get_mut(&mut self) -> &mut tick::Transfer {
        &mut self.transfer
    }
}

impl ::std::io::Write for AsyncWriter {
    fn write(&mut self, data: &[u8]) -> ::std::io::Result<usize> {
        let len = data.len();
        self.transfer.write(data);
        Ok(len)
    }

    fn flush(&mut self) -> ::std::io::Result<()> {
        Ok(())
    }
}

pub trait Parse {
    type Subject;
    fn parse(bytes: &[u8]) -> ParseResult<Self::Subject>;
}

pub type ParseResult<T> = ::Result<Option<(Incoming<T>, usize)>>;

pub fn parse<T: Parse<Subject=I>, I>(rdr: &[u8]) -> ParseResult<I> {
    //TODO: try h2::parse()
    h1::parse::<T, I>(rdr)
}

#[test]
fn test_should_keep_alive() {
    let mut headers = Headers::new();

    assert!(!should_keep_alive(Http10, &headers));
    assert!(should_keep_alive(Http11, &headers));

    headers.set(Connection::close());
    assert!(!should_keep_alive(Http10, &headers));
    assert!(!should_keep_alive(Http11, &headers));

    headers.set(Connection::keep_alive());
    assert!(should_keep_alive(Http10, &headers));
    assert!(should_keep_alive(Http11, &headers));
}
