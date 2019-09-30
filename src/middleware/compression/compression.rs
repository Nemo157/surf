use crate::{http_client::HttpClient, middleware::{Body, Middleware, Next, Request, Response}, Exception};
use accept_encoding::Encoding;
use async_compression::bufread::{BrotliDecoder, DeflateDecoder, GzipDecoder, ZstdDecoder};
use futures::{future::BoxFuture, io::BufReader};
use http::{
    header::CONTENT_ENCODING,
    header::{HeaderValue, ACCEPT_ENCODING},
};
use super::compression_error::CompressionError;

static SUPPORTED_ENCODINGS: &str = "gzip, br, deflate, zstd";

#[derive(Debug)]
pub struct Compression;

use std::{pin::Pin, task::{Context, Poll}, io, any::type_name};

struct Dbg<T>(T);

impl<T: futures::io::AsyncRead> futures::io::AsyncRead for Dbg<T> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let reader = unsafe { Pin::new_unchecked(&mut self.get_unchecked_mut().0) };
        let res = dbg!((type_name::<T>(), reader.poll_read(cx, buf))).1?;
        dbg!(std::str::from_utf8(buf));
        res.map(Ok)
    }
}

impl<T: futures::io::AsyncBufRead> futures::io::AsyncBufRead for Dbg<T> {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        let reader = unsafe { Pin::new_unchecked(&mut self.get_unchecked_mut().0) };
        dbg!((type_name::<T>(), reader.poll_fill_buf(cx))).1
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        let reader = unsafe { Pin::new_unchecked(&mut self.get_unchecked_mut().0) };
        dbg!((type_name::<T>(), reader.consume(dbg!(amt)))).1
    }
}

impl Compression {
    pub fn new() -> Self {
        Self {}
    }

    fn parse_encoding(s: &str) -> Result<Encoding, Exception> {
        match s {
            "gzip" => Ok(Encoding::Gzip),
            "deflate" => Ok(Encoding::Deflate),
            "br" => Ok(Encoding::Brotli),
            "zstd" => Ok(Encoding::Zstd),
            "identity" => Ok(Encoding::Identity),
            _ => Err(Box::new(CompressionError::UnsupportedContentEncoding)),
        }
    }

    async fn decode(&self, req: &mut Response) -> Result<(),  Exception> {
        dbg!(req.headers());
        let encodings = if let Some(hval) = req.headers().get(CONTENT_ENCODING.as_str()) {
            let hval = match hval.to_str() {
                Ok(hval) => hval,
                Err(err) => {
                    let exception: Exception = Box::new(err);
                    return Err(exception);
                }
            };
            hval.split(',')
                .map(str::trim)
                .rev() // apply decodings in reverse order
                .map(Compression::parse_encoding)
                .collect::<Result<Vec<Encoding>, Exception>>()
        } else {
            // No decoding to do
            return Ok(());
        };

        for encoding in encodings? {
            match encoding {
                Encoding::Gzip => {
                    let body = std::mem::replace(req.body_mut(), Body::empty());
                    let async_decoder = Dbg(GzipDecoder::new(Dbg(BufReader::new(Dbg(body)))));
                    *req.body_mut() = Body::from_reader(async_decoder);
                }
                Encoding::Deflate => {
                    let body = std::mem::replace(req.body_mut(), Body::empty());
                    let async_decoder = DeflateDecoder::new(BufReader::new(body));
                    *req.body_mut() = Body::from_reader(async_decoder);
                }
                Encoding::Brotli => {
                    let body = std::mem::replace(req.body_mut(), Body::empty());
                    let async_decoder = BrotliDecoder::new(BufReader::new(body));
                    *req.body_mut() = Body::from_reader(async_decoder);
                }
                Encoding::Zstd => {
                    let body = std::mem::replace(req.body_mut(), Body::empty());
                    let async_decoder = ZstdDecoder::new(BufReader::new(body));
                    *req.body_mut() = Body::from_reader(async_decoder);
                }
                Encoding::Identity => (),
            }
        }
        // strip the content-encoding header
        // (could never fail since it returns early if it's not present)
        req.headers_mut().remove(CONTENT_ENCODING).unwrap();
        Ok(())
    }
}

impl<C: HttpClient> Middleware<C> for Compression {
    #[allow(missing_doc_code_examples)]
    fn handle<'a>(
        &'a self,
        mut req: Request,
        client: C,
        next: Next<'a, C>,
    ) -> BoxFuture<'a, Result<Response, Exception>> {
        Box::pin(async move {
            req.headers_mut().insert(
                ACCEPT_ENCODING,
                HeaderValue::from_static(SUPPORTED_ENCODINGS),
            );
            let mut res = next.run(req, client).await?;
            self.decode(&mut res).await?;
            Ok(res)
        })
    }
}
