//! A small HTTP/1.1 client: one keep-alive connection, one request at a time,
//! JSON or binary bodies, `Content-Length` or chunked responses. No TLS: the
//! generator talks to a server on loopback or a private network.

use std::io;
use std::net::{IpAddr, SocketAddr};

use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{TcpSocket, TcpStream};

/// Response headers larger than this are refused.
const MAX_HEAD: usize = 64 * 1024;

/// Open a TCP connection to `server`, from `source` when given (one of many
/// loopback addresses, so that each simulated client is its own address).
pub async fn connect(server: SocketAddr, source: Option<IpAddr>) -> io::Result<TcpStream> {
    let socket = if server.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    if let Some(ip) = source {
        socket.bind(SocketAddr::new(ip, 0))?;
    }
    socket.set_nodelay(true)?;
    socket.connect(server).await
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

pub struct Conn {
    io: BufReader<TcpStream>,
    host: String,
    /// `X-Forwarded-For`, when the server trusts the generator as a proxy.
    forwarded: Option<String>,
}

impl Conn {
    pub fn new(stream: TcpStream, host: String, forwarded: Option<String>) -> Self {
        Self {
            io: BufReader::new(stream),
            host,
            forwarded,
        }
    }

    pub async fn request(
        &mut self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> io::Result<Response> {
        let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {}\r\n", self.host);
        for (k, v) in headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        if let Some(f) = &self.forwarded {
            head.push_str(&format!("X-Forwarded-For: {f}\r\n"));
        }
        match body {
            Some(b) => head.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                b.len()
            )),
            None if method != "GET" => head.push_str("Content-Length: 0\r\n"),
            None => {}
        }
        head.push_str("\r\n");
        let stream = self.io.get_mut();
        stream.write_all(head.as_bytes()).await?;
        if let Some(b) = body {
            stream.write_all(b).await?;
        }
        stream.flush().await?;
        self.read_response(method == "HEAD").await
    }

    async fn read_response(&mut self, head_only: bool) -> io::Result<Response> {
        let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_owned());
        let mut line = String::new();
        let mut read = self.io.read_line(&mut line).await?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let status: u16 = line
            .split(' ')
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| bad("status line"))?;
        let (mut length, mut chunked) = (None, false);
        loop {
            line.clear();
            let n = self.io.read_line(&mut line).await?;
            read += n;
            if n == 0 || read > MAX_HEAD {
                return Err(bad("response head"));
            }
            let l = line.trim_end();
            if l.is_empty() {
                break;
            }
            let Some((name, value)) = l.split_once(':') else {
                return Err(bad("header"));
            };
            let value = value.trim();
            match name.to_ascii_lowercase().as_str() {
                "content-length" => {
                    length = Some(value.parse::<usize>().map_err(|_| bad("length"))?)
                }
                "transfer-encoding" => chunked = value.eq_ignore_ascii_case("chunked"),
                _ => {}
            }
        }
        let mut body = Vec::new();
        if head_only || status == 204 || status == 304 || (100..200).contains(&status) {
        } else if chunked {
            loop {
                line.clear();
                self.io.read_line(&mut line).await?;
                let size = usize::from_str_radix(line.trim().split(';').next().unwrap_or(""), 16)
                    .map_err(|_| bad("chunk size"))?;
                if size == 0 {
                    line.clear();
                    self.io.read_line(&mut line).await?;
                    break;
                }
                let start = body.len();
                body.resize(start + size, 0);
                self.io.read_exact(&mut body[start..]).await?;
                line.clear();
                self.io.read_line(&mut line).await?;
            }
        } else if let Some(n) = length {
            body.resize(n, 0);
            self.io.read_exact(&mut body).await?;
        } else {
            self.io.read_to_end(&mut body).await?;
        }
        Ok(Response { status, body })
    }
}
