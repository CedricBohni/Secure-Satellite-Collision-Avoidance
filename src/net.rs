//! The point-to-point channel between the two parties.
//!
//! Party 0 listens, party 1 dials; after the handshake the link is symmetric.
//! Every round of the protocol is one [`Channel::exchange`]: both parties send
//! their half of the round and read the peer's half.  Writing happens on a
//! helper thread so that a round never deadlocks, no matter how large the
//! batch of openings is.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use crate::metrics::Transcript;

const CONNECT_RETRIES: u32 = 50;
const CONNECT_BACKOFF: Duration = Duration::from_millis(200);

/// A framed, bidirectional link to the other party.
pub struct Channel {
    stream: TcpStream,
    transcript: Transcript,
}

impl Channel {
    /// Establish the link. `party` 0 binds `addr`, party 1 connects to it and
    /// retries for a while so the two terminals can be started in any order.
    pub fn connect(party: u8, addr: &str) -> io::Result<Channel> {
        let stream = match party {
            0 => {
                let listener = TcpListener::bind(addr)?;
                eprintln!("[party 0] listening on {addr}, waiting for party 1");
                let (stream, peer) = listener.accept()?;
                eprintln!("[party 0] party 1 connected from {peer}");
                stream
            }
            _ => {
                let mut last = None;
                let mut stream = None;
                for _ in 0..CONNECT_RETRIES {
                    match TcpStream::connect(addr) {
                        Ok(s) => {
                            stream = Some(s);
                            break;
                        }
                        Err(e) => {
                            last = Some(e);
                            thread::sleep(CONNECT_BACKOFF);
                        }
                    }
                }
                let stream = match stream {
                    Some(s) => s,
                    None => return Err(last.unwrap()),
                };
                eprintln!("[party 1] connected to party 0 at {addr}");
                stream
            }
        };
        stream.set_nodelay(true)?;
        Ok(Channel {
            stream,
            transcript: Transcript::default(),
        })
    }

    /// Send one message and receive the peer's message of the same round.
    pub fn exchange(&mut self, payload: &[u8]) -> io::Result<Vec<u8>> {
        let mut writer = self.stream.try_clone()?;
        let mut reader = &self.stream;

        let received = thread::scope(|scope| -> io::Result<Vec<u8>> {
            let sender = scope.spawn(move || -> io::Result<()> {
                writer.write_all(&(payload.len() as u32).to_be_bytes())?;
                writer.write_all(payload)?;
                writer.flush()
            });

            let mut header = [0u8; 4];
            reader.read_exact(&mut header)?;
            let len = u32::from_be_bytes(header) as usize;
            let mut buf = vec![0u8; len];
            reader.read_exact(&mut buf)?;

            sender
                .join()
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "sender thread panicked"))??;
            Ok(buf)
        })?;

        self.transcript.rounds += 1;
        self.transcript.bytes_sent += payload.len();
        self.transcript.bytes_received += received.len();
        Ok(received)
    }

    /// Communication cost so far.
    pub fn transcript(&self) -> Transcript {
        self.transcript
    }
}
