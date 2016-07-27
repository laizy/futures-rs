extern crate futures;
extern crate env_logger;
extern crate futuremio;
extern crate ssl;

#[macro_use]
extern crate cfg_if;

use std::io::{Write, ErrorKind, Error};
use std::net::ToSocketAddrs;

use futures::Future;
use futures::stream::Stream;
use futures::io::{self, Ready};

macro_rules! t {
    ($e:expr) => (match $e {
        Ok(e) => e,
        Err(e) => panic!("{} failed with {:?}", stringify!($e), e),
    })
}

cfg_if! {
    if #[cfg(any(feature = "force-openssl",
                 all(not(target_os = "macos"),
                     not(target_os = "windows"))))] {
        extern crate openssl;

        use openssl::ssl as ossl;

        fn assert_bad_hostname_error(err: &Error) {
            let err = err.get_ref().unwrap();
            let errs = match *err.downcast_ref::<ossl::Error>().unwrap() {
                ossl::Error::Ssl(ref v) => v,
                ref e => panic!("not an ssl eror: {:?}", e),
            };
            assert!(errs.iter().any(|e| {
                e.reason() == "certificate verify failed"
            }), "bad errors: {:?}", errs);
        }
    } else if #[cfg(target_os = "macos")] {
        extern crate security_framework;

        use security_framework::base::Error as SfError;

        fn assert_bad_hostname_error(err: &Error) {
            let err = err.get_ref().unwrap();
            let err = err.downcast_ref::<SfError>().unwrap();
            assert_eq!(err.message().unwrap(), "invalid certificate chain");
        }
    } else {
        extern crate winapi;

        fn assert_bad_hostname_error(err: &Error) {
            let code = err.raw_os_error().unwrap();
            assert_eq!(code as usize, winapi::CERT_E_CN_NO_MATCH as usize);
        }
    }
}

#[test]
fn fetch_google() {
    drop(env_logger::init());

    // First up, resolve google.com
    let addr = t!("google.com:443".to_socket_addrs()).next().unwrap();

    // Create an event loop and connect a socket to our resolved address.c
    let mut l = t!(futuremio::Loop::new());
    let client = l.handle().tcp_connect(&addr);

    // Send off the request by first negotiating an SSL handshake, then writing
    // of our request, then flushing, then finally read off the response.
    let data = client.and_then(move |socket| {
        t!(ssl::ClientContext::new()).handshake("google.com", socket)
    }).and_then(|socket| {
        write_all(socket, b"GET / HTTP/1.0\r\n\r\n")
    }).and_then(|socket| {
        io::flush(socket)
    }).and_then(|socket| {
        io::read_to_end(socket, Vec::new())
    });

    let data = t!(l.run(data));
    assert!(data.starts_with(b"HTTP/1.0 200 OK"));
    assert!(data.ends_with(b"</html>"));
}

#[test]
fn wrong_hostname_error() {
    drop(env_logger::init());

    let addr = t!("google.com:443".to_socket_addrs()).next().unwrap();

    let mut l = t!(futuremio::Loop::new());
    let client = l.handle().tcp_connect(&addr);
    let data = client.and_then(move |socket| {
        t!(ssl::ClientContext::new()).handshake("rust-lang.org", socket)
    });

    let res = l.run(data);
    assert!(res.is_err());
    assert_bad_hostname_error(&res.err().unwrap());
}

fn write_all<W>(mut socket: W, mut data: &'static [u8])
                -> Box<Future<Item=W, Error=Error>>
    where W: Write + Stream<Item=Ready, Error=Error>,
{
    while data.len() > 0 {
        match socket.write(data) {
            Ok(n) => {
                println!("wrote: {}", n);
                data = &data[n..];
            }
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) => return futures::failed(e).boxed(),
        }
    }
    if data.len() == 0 {
        futures::finished(socket).boxed()
    } else {
        socket.into_future().map_err(|e| e.0).and_then(move |(_, w)| {
            write_all(w, data)
        }).boxed()
    }
}
