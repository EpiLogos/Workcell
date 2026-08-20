use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    time::Duration,
};

use epilogos_workcell_core::WorkcellControlPlane;

use crate::{ControlService, ControlTransport, TransportFailure};

const MAX_CONTROL_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// TCP carrier for the versioned Workcell control envelope.
///
/// TCP is a material transport only. The control protocol remains the JSON
/// envelope defined by `workcell.control/v1`, and the endpoint is never a
/// Workcell identity.
pub struct TcpControlTransport {
    address: String,
    timeout: Option<Duration>,
}

impl TcpControlTransport {
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            timeout: Some(Duration::from_secs(10)),
        }
    }

    pub fn with_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn address(&self) -> &str {
        &self.address
    }
}

impl ControlTransport for TcpControlTransport {
    fn round_trip(&mut self, request: &[u8]) -> Result<Vec<u8>, TransportFailure> {
        let mut stream = TcpStream::connect(&self.address).map_err(|error| {
            TransportFailure::new(format!(
                "connect Workcell control endpoint `{}`: {error}",
                self.address
            ))
        })?;
        stream
            .set_read_timeout(self.timeout)
            .map_err(transport_error("set control read timeout"))?;
        stream
            .set_write_timeout(self.timeout)
            .map_err(transport_error("set control write timeout"))?;
        write_frame(&mut stream, request).map_err(transport_error("write control request"))?;
        stream
            .flush()
            .map_err(transport_error("flush control request"))?;
        read_frame(&mut stream).map_err(transport_error("read control response"))
    }
}

/// Optional long-running host for the existing Workcell control plane.
///
/// The server owns no planner/runtime semantics. Each accepted connection
/// carries exactly one request/response envelope into `ControlService`.
pub struct TcpControlServer<C> {
    listener: TcpListener,
    service: ControlService<C>,
}

impl<C> TcpControlServer<C>
where
    C: WorkcellControlPlane,
{
    pub fn bind(address: &str, service: ControlService<C>) -> io::Result<Self> {
        Ok(Self {
            listener: TcpListener::bind(address)?,
            service,
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    pub fn service(&self) -> &ControlService<C> {
        &self.service
    }

    pub fn service_mut(&mut self) -> &mut ControlService<C> {
        &mut self.service
    }

    pub fn serve_once(&mut self) -> io::Result<()> {
        let (mut stream, _) = self.listener.accept()?;
        self.handle_stream(&mut stream)
    }

    pub fn serve_n(&mut self, requests: usize) -> io::Result<()> {
        for _ in 0..requests {
            self.serve_once()?;
        }
        Ok(())
    }

    pub fn serve(&mut self) -> io::Result<()> {
        loop {
            let (mut stream, _) = self.listener.accept()?;
            if let Err(error) = self.handle_stream(&mut stream) {
                eprintln!("workcell control connection failed: {error}");
            }
        }
    }

    fn handle_stream(&mut self, stream: &mut TcpStream) -> io::Result<()> {
        let request = read_frame(stream)?;
        let response = self.service.handle_bytes(&request);
        write_frame(stream, &response)?;
        stream.flush()
    }
}

fn transport_error(context: &'static str) -> impl FnOnce(io::Error) -> TransportFailure {
    move |error| TransportFailure::new(format!("{context}: {error}"))
}

fn write_frame(writer: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_CONTROL_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "control frame exceeds {} byte limit",
                MAX_CONTROL_FRAME_BYTES
            ),
        ));
    }
    let length = u32::try_from(payload.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "control frame length exceeds u32",
        )
    })?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(payload)
}

fn read_frame(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut prefix = [0_u8; 4];
    reader.read_exact(&mut prefix)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length > MAX_CONTROL_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "control frame declares {length} bytes, above {} byte limit",
                MAX_CONTROL_FRAME_BYTES
            ),
        ));
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    Ok(payload)
}
