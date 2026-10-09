//! Requests to Studio over the operating system's network, encrypted with
//! rustls when Studio is served over HTTPS.

use std::io;
use std::time::Duration;

use futures_util::future::BoxFuture;
use http::{Request, Response};
use spiral_robotics_connector::LinkDriver;

/// Studio holds a poll open for up to 25 seconds; a link silent for longer
/// than this is lost, however long a large answer takes to send.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct HttpLink {
    client: reqwest::Client,
}

impl HttpLink {
    pub(crate) fn new() -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()?;
        Ok(Self { client })
    }
}

impl LinkDriver for HttpLink {
    fn send(&self, request: Request<Vec<u8>>) -> BoxFuture<'static, io::Result<Response<Vec<u8>>>> {
        let client = self.client.clone();
        Box::pin(async move {
            let request = reqwest::Request::try_from(request).map_err(io::Error::other)?;
            let response = client.execute(request).await.map_err(io::Error::other)?;
            let mut answer = Response::builder().status(response.status());
            if let Some(headers) = answer.headers_mut() {
                headers.extend(response.headers().clone());
            }
            let body = response.bytes().await.map_err(io::Error::other)?;
            answer.body(body.to_vec()).map_err(io::Error::other)
        })
    }
}
