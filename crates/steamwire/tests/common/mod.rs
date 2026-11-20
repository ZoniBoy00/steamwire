//! A small HTTP server standing in for Steam's websites, and a service
//! transport standing in for the client connection.

#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use prost::Message;
use steamwire::community::{Urls, WebSession};
use steamwire::protos::ServiceTransport;
use steamwire::{EResult, SteamId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub const ME: SteamId = SteamId::from_u64(76561198000000001);

/// A request the server received.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, Vec<String>>,
    pub headers: HashMap<String, String>,
    pub form: HashMap<String, Vec<String>>,
}

impl Request {
    pub fn query(&self, key: &str) -> Option<&str> {
        self.query.get(key).and_then(|v| v.first()).map(String::as_str)
    }

    pub fn field(&self, key: &str) -> Option<&str> {
        self.form.get(key).and_then(|v| v.first()).map(String::as_str)
    }

    pub fn cookie(&self, name: &str) -> Option<String> {
        self.headers.get("cookie")?.split("; ").find_map(|c| {
            let (n, v) = c.split_once('=')?;
            (n == name).then(|| v.to_string())
        })
    }
}

/// An answer: status, extra headers and body.
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Answer {
    pub fn json(body: &str) -> Self {
        Self { status: 200, headers: vec![("Content-Type".into(), "application/json".into())], body: body.into() }
    }

    pub fn status(status: u16, body: &str) -> Self {
        Self { status, headers: Vec::new(), body: body.into() }
    }

    pub fn redirect(location: &str) -> Self {
        Self { status: 302, headers: vec![("Location".into(), location.into())], body: Vec::new() }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

type Handler = dyn Fn(&Request) -> Answer + Send + Sync;

pub struct Server {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
}

impl Server {
    pub async fn start(handler: impl Fn(&Request) -> Answer + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let seen = requests.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else { return };
                let handler = handler.clone();
                let seen = seen.clone();
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut stream).await else { return };
                    let answer = handler(&request);
                    seen.lock().unwrap().push(request);
                    let mut head = format!(
                        "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                        answer.status,
                        answer.body.len()
                    );
                    for (name, value) in &answer.headers {
                        head.push_str(&format!("{name}: {value}\r\n"));
                    }
                    head.push_str("\r\n");
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(&answer.body).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        Self { url, requests }
    }

    pub fn session(&self) -> WebSession {
        WebSession::builder(ME).urls(Urls::all(&self.url)).access_token("header.payload.signature").unwrap()
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    pub fn last(&self) -> Request {
        self.requests().pop().expect("a request")
    }
}

fn parse_pairs(text: &str) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for (k, v) in form_urlencoded(text) {
        map.entry(k).or_default().push(v);
    }
    map
}

fn form_urlencoded(text: &str) -> Vec<(String, String)> {
    text.split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(k), decode(v))
        })
        .collect()
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                out.push(u8::from_str_radix(&text[i + 1..i + 3], 16).unwrap());
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8(out).unwrap()
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<Request> {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    let head_end = loop {
        let n = stream.read(&mut buf).await.ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&buf[..n]);
        if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&data[..head_end]).to_string();
    let mut lines = head.lines();
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let target = first.next()?.to_string();
    let headers: HashMap<String, String> =
        lines.filter_map(|l| l.split_once(": ")).map(|(k, v)| (k.to_ascii_lowercase(), v.to_string())).collect();
    let length: usize = headers.get("content-length").and_then(|l| l.parse().ok()).unwrap_or(0);
    while data.len() < head_end + length {
        let n = stream.read(&mut buf).await.ok()?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
    }
    let body = String::from_utf8_lossy(&data[head_end..]).to_string();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    Some(Request { method, path: path.to_string(), query: parse_pairs(query), headers, form: parse_pairs(&body) })
}

/// Answers service calls from a queue, and records them.
#[derive(Default)]
pub struct FakeTransport {
    pub answers: Mutex<VecDeque<(EResult, Vec<u8>)>>,
    pub calls: Mutex<Vec<(&'static str, Vec<u8>)>>,
}

impl FakeTransport {
    pub fn answer(&self, result: EResult, body: impl Message) {
        self.answers.lock().unwrap().push_back((result, body.encode_to_vec()));
    }

    pub fn call_bodies<M: Message + Default>(&self, method: &str) -> Vec<M> {
        let calls = self.calls.lock().unwrap();
        calls.iter().filter(|(m, _)| *m == method).map(|(_, b)| M::decode(b.as_slice()).unwrap()).collect()
    }
}

impl ServiceTransport for FakeTransport {
    type Error = steamwire::Error;

    fn call<Req, Resp>(
        &self,
        method: &'static str,
        request: Req,
    ) -> impl std::future::Future<Output = Result<Resp, Self::Error>> + Send
    where
        Req: Message + Send + 'static,
        Resp: Message + Default + Send + 'static,
    {
        self.calls.lock().unwrap().push((method, request.encode_to_vec()));
        let answer = self.answers.lock().unwrap().pop_front();
        async move {
            let (result, body) = answer.expect("an answer is queued");
            if result != EResult::OK {
                return Err(steamwire::Error::Steam(result));
            }
            Ok(Resp::decode(body.as_slice())?)
        }
    }

    fn notify<Req>(&self, _method: &'static str, _request: Req) -> Result<(), Self::Error>
    where
        Req: Message + Send + 'static,
    {
        Ok(())
    }
}
