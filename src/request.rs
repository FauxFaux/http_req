//! creating and sending HTTP requests
use crate::{
    error,
    response::{Headers, Response, CR_LF_2},
    uri::Uri,
};
use native_tls::TlsConnector;
use std::{
    fmt,
    io::{self, Read, Write},
    net::TcpStream,
};

const CR_LF: &str = "\r\n";
const HTTP_V: &str = "HTTP/1.1";

///Copies data from `reader` to `writer` until the specified `val`ue is reached.
///Returns how many bytes has been read.
pub fn copy_until<R, W>(reader: &mut R, writer: &mut W, val: &[u8]) -> Result<usize, io::Error>
where
    R: Read + ?Sized,
    W: Write + ?Sized,
{
    let mut buf = Vec::with_capacity(200);

    let mut pre_buf = [0; 10];
    let mut read = reader.read(&mut pre_buf)?;
    buf.extend(&pre_buf[..read]);

    for byte in reader.bytes() {
        buf.push(byte?);
        read += 1;

        if &buf[(buf.len() - val.len())..] == val {
            break;
        }
    }

    writer.write_all(&buf)?;
    writer.flush()?;

    Ok(read)
}

///HTTP request methods
#[derive(Debug, PartialEq, Clone)]
pub enum Method {
    GET,
    HEAD,
    POST,
    PUT,
    DELETE,
    OPTIONS,
    PATCH,
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use self::Method::*;

        let method = match self {
            GET => "GET",
            HEAD => "HEAD",
            POST => "POST",
            PUT => "PUT",
            DELETE => "DELETE",
            OPTIONS => "OPTIONS",
            PATCH => "PATCH",
        };

        write!(f, "{}", method)
    }
}

///Relatively low-level struct for making HTTP requests.
///
///It can work with any stream that implements `Read` and `Write`.
///By default it does not close the connection after completion of the response.
#[derive(Clone, Debug, PartialEq)]
pub struct RequestBuilder<'a> {
    uri: &'a Uri,
    method: Method,
    version: &'a str,
    headers: Headers,
    body: Option<&'a [u8]>,
}

impl<'a> RequestBuilder<'a> {
    ///Creates new `RequestBuilder` with default parameters
    pub fn new(uri: &'a Uri) -> RequestBuilder<'a> {
        RequestBuilder {
            headers: Headers::default_http(&uri),
            uri,
            method: Method::GET,
            version: HTTP_V,
            body: None,
        }
    }

    ///Sets request method
    pub fn method<T>(&mut self, method: T) -> &mut Self
    where
        Method: From<T>,
    {
        self.method = Method::from(method);
        self
    }

    ///Replaces all it's headers with headers passed to the function
    pub fn headers<T>(&mut self, headers: T) -> &mut Self
    where
        Headers: From<T>,
    {
        self.headers = Headers::from(headers);
        self
    }

    ///Adds new header to existing/default headers
    pub fn header<T, U>(&mut self, key: &T, val: &U) -> &mut Self
    where
        T: ToString + ?Sized,
        U: ToString + ?Sized,
    {
        self.headers.insert(key, val);
        self
    }

    ///Sets body for request
    pub fn body(&mut self, body: &'a [u8]) -> &mut Self {
        self.body = Some(body);
        self
    }

    ///Sends HTTP request.
    ///
    ///Writes request message to `stream`. Returns response for this request.
    ///Writes response's body to `writer`.
    pub fn send<T, U>(&self, stream: &mut T, writer: &mut U) -> Result<Response, error::Error>
    where
        T: Write + Read,
        U: Write,
    {
        self.write_msg(stream, &self.parse_msg())?;
        let res = self.read_head(stream)?;

        if self.method != Method::HEAD {
            io::copy(stream, writer)?;
        }

        Ok(res)
    }

    ///Writes message to `stream` and flashes it
    pub fn write_msg<T, U>(&self, stream: &mut T, msg: &U) -> Result<(), io::Error>
    where
        T: Write,
        U: AsRef<[u8]>,
    {
        stream.write_all(msg.as_ref())?;
        stream.flush()?;

        Ok(())
    }

    ///Reads head of server's response
    pub fn read_head<T: Read>(&self, stream: &mut T) -> Result<Response, error::Error> {
        let mut head = Vec::with_capacity(200);
        copy_until(stream, &mut head, &CR_LF_2)?;

        Response::from_head(&head)
    }

    ///Parses request message for this `RequestBuilder`
    pub fn parse_msg(&self) -> Vec<u8> {
        let request_line = format!(
            "{} {} {}{}",
            self.method,
            self.uri.resource(),
            self.version,
            CR_LF
        );

        let headers: String = self
            .headers
            .iter()
            .map(|(k, v)| format!("{}: {}{}", k, v, CR_LF))
            .collect();

        let mut request_msg = (request_line + &headers + CR_LF).as_bytes().to_vec();

        if let Some(b) = &self.body {
            request_msg.extend(*b);
        }

        request_msg
    }
}

///Relatively higher-level struct for making HTTP requests.
///
///It creates stream (`TcpStream` or `TlsStream`) appropriate for the type of uri (`http`/`https`)
///By default it closes connection after completion of the response.
#[derive(Clone, Debug, PartialEq)]
pub struct Request<'a> {
    inner: RequestBuilder<'a>,
}

impl<'a> Request<'a> {
    ///Creates new `Request` with default parameters
    pub fn new(uri: &'a Uri) -> Request<'a> {
        let mut builder = RequestBuilder::new(&uri);
        builder.header("Connection", "Close");

        Request { inner: builder }
    }

    ///Replaces all it's headers with headers passed to the function
    pub fn headers<T>(&mut self, headers: T) -> &mut Self
    where
        Headers: From<T>,
    {
        self.inner.headers(headers);

        self
    }

    ///Adds header to existing/default headers
    pub fn header<T, U>(&mut self, key: &T, val: &U) -> &mut Self
    where
        T: ToString + ?Sized,
        U: ToString + ?Sized,
    {
        self.inner.header(key, val);

        self
    }

    ///Changes request's method
    pub fn set_method<T>(&mut self, method: T)
    where
        Method: From<T>,
    {
        self.inner.method(method);
    }

    ///Sends HTTP request.
    ///
    ///Creates `TcpStream` (and wraps it with `TlsStream` if needed). Writes request message
    ///to created stream. Returns response for this request. Writes response's body to `writer`.
    pub fn send<T: Write>(&self, writer: &mut T) -> Result<Response, error::Error> {
        let mut stream = TcpStream::connect((
            self.inner.uri.host().unwrap_or(""),
            self.inner.uri.corr_port(),
        ))?;

        if self.inner.uri.scheme() == "https" {
            let connector = TlsConnector::new()?;
            let mut stream = connector.connect(self.inner.uri.host().unwrap_or(""), stream)?;

            self.inner.send(&mut stream, writer)
        } else {
            self.inner.send(&mut stream, writer)
        }
    }
}

///Creates and sends GET request. Returns response for this request.
pub fn get<T: AsRef<str>, U: Write>(uri: T, writer: &mut U) -> Result<Response, error::Error> {
    let uri = uri.as_ref().parse::<Uri>()?;
    let request = Request::new(&uri);

    request.send(writer)
}

///Creates and sends HEAD request. Returns response for this request.
pub fn head<T: AsRef<str>>(uri: T) -> Result<Response, error::Error> {
    let mut writer = Vec::new();
    let uri = uri.as_ref().parse::<Uri>()?;

    let mut request = Request::new(&uri);
    request.set_method(Method::HEAD);

    request.send(&mut writer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const UNSUCCESS_CODE: u16 = 400;
    const URI: &str = "http://doc.rust-lang.org/std/string/index.html";
    const URI_S: &str = "https://doc.rust-lang.org/std/string/index.html";
    const BODY: [u8; 14] = [78, 97, 109, 101, 61, 74, 97, 109, 101, 115, 43, 74, 97, 121];

    const RESPONSE: &'static [u8; 129] = b"HTTP/1.1 200 OK\r\n\
                                         Date: Sat, 11 Jan 2003 02:44:04 GMT\r\n\
                                         Content-Type: text/html\r\n\
                                         Content-Length: 100\r\n\r\n\
                                         <html>hello</html>\r\n\r\nhello";

    const RESPONSE_H: &'static [u8; 102] = b"HTTP/1.1 200 OK\r\n\
                                           Date: Sat, 11 Jan 2003 02:44:04 GMT\r\n\
                                           Content-Type: text/html\r\n\
                                           Content-Length: 100\r\n\r\n";

    #[test]
    fn copy_data_until() {
        let mut reader = Vec::new();
        reader.extend(&RESPONSE[..]);

        let mut reader = Cursor::new(reader);
        let mut writer = Vec::new();

        copy_until(&mut reader, &mut writer, &CR_LF_2).unwrap();
        assert_eq!(writer, &RESPONSE_H[..]);
    }

    #[test]
    fn method_display() {
        const METHOD: Method = Method::HEAD;
        assert_eq!(&format!("{}", METHOD), "HEAD");
    }

    #[test]
    fn request_b_new() {
        RequestBuilder::new(&URI.parse().unwrap());
        RequestBuilder::new(&URI_S.parse().unwrap());
    }

    #[test]
    fn request_b_method() {
        let uri: Uri = URI.parse().unwrap();
        let mut req = RequestBuilder::new(&uri);
        let req = req.method(Method::HEAD);

        assert_eq!(req.method, Method::HEAD);
    }

    #[test]
    fn request_b_headers() {
        let mut headers = Headers::new();
        headers.insert("Accept-Charset", "utf-8");
        headers.insert("Accept-Language", "en-US");
        headers.insert("Host", "doc.rust-lang.org");
        headers.insert("Connection", "Close");

        let uri: Uri = URI.parse().unwrap();
        let mut req = RequestBuilder::new(&uri);
        let req = req.headers(headers.clone());

        assert_eq!(req.headers, headers);
    }

    #[test]
    fn request_b_header() {
        let uri: Uri = URI.parse().unwrap();
        let mut req = RequestBuilder::new(&uri);
        let k = "Connection";
        let v = "Close";

        let mut expect_headers = Headers::new();
        expect_headers.insert("Host", "doc.rust-lang.org");
        expect_headers.insert("Referer", "http://doc.rust-lang.org/std/string/index.html");
        expect_headers.insert(k, v);

        let req = req.header(k, v);

        assert_eq!(req.headers, expect_headers);
    }

    #[test]
    fn request_b_body() {
        let uri: Uri = URI.parse().unwrap();
        let mut req = RequestBuilder::new(&uri);
        let req = req.body(&BODY);

        assert_eq!(req.body, Some(BODY.as_ref()));
    }

    #[ignore]
    #[test]
    fn request_b_send() {
        let mut writer = Vec::new();
        let uri: Uri = URI.parse().unwrap();
        let mut stream = TcpStream::connect((uri.host().unwrap_or(""), uri.corr_port())).unwrap();

        RequestBuilder::new(&URI.parse().unwrap())
            .header("Connection", "Close")
            .send(&mut stream, &mut writer)
            .unwrap();
    }

    #[ignore]
    #[test]
    fn request_b_send_secure() {
        let mut writer = Vec::new();
        let uri: Uri = URI_S.parse().unwrap();

        let stream = TcpStream::connect((uri.host().unwrap_or(""), uri.corr_port())).unwrap();
        let connector = TlsConnector::new().unwrap();
        let mut secure_stream = connector.connect(uri.host().unwrap_or(""), stream).unwrap();

        RequestBuilder::new(&URI_S.parse().unwrap())
            .header("Connection", "Close")
            .send(&mut secure_stream, &mut writer)
            .unwrap();
    }

    #[test]
    fn request_b_parse_msg() {
        let uri = URI.parse().unwrap();
        let req = RequestBuilder::new(&uri);

        const DEFAULT_MSG: &str = "GET /std/string/index.html HTTP/1.1\r\n\
                                   Referer: http://doc.rust-lang.org/std/string/index.html\r\n\
                                   Host: doc.rust-lang.org\r\n\r\n";
        let msg = req.parse_msg();
        let msg = String::from_utf8_lossy(&msg).into_owned();

        for line in DEFAULT_MSG.lines() {
            assert!(msg.contains(line));
        }

        for line in msg.lines() {
            assert!(DEFAULT_MSG.contains(line));
        }
    }

    #[test]
    fn request_new() {
        let uri = URI.parse().unwrap();
        Request::new(&uri);
    }

    #[test]
    fn request_method() {
        let uri = URI.parse().unwrap();
        let mut req = Request::new(&uri);
        req.set_method(Method::HEAD);

        assert_eq!(req.inner.method, Method::HEAD);
    }

    #[test]
    fn request_headers() {
        let mut headers = Headers::new();
        headers.insert("Accept-Charset", "utf-8");
        headers.insert("Accept-Language", "en-US");
        headers.insert("Host", "doc.rust-lang.org");
        headers.insert("Connection", "Close");

        let uri: Uri = URI.parse().unwrap();
        let mut req = Request::new(&uri);
        let req = req.headers(headers.clone());

        assert_eq!(req.inner.headers, headers);
    }

    #[test]
    fn request_header() {
        let uri: Uri = URI.parse().unwrap();
        let mut req = Request::new(&uri);
        let k = "Accept-Language";
        let v = "en-US";

        let mut expect_headers = Headers::new();
        expect_headers.insert("Host", "doc.rust-lang.org");
        expect_headers.insert("Referer", "http://doc.rust-lang.org/std/string/index.html");
        expect_headers.insert("Connection", "Close");
        expect_headers.insert(k, v);

        let req = req.header(k, v);

        assert_eq!(req.inner.headers, expect_headers);
    }

    #[test]
    fn request_send() {
        let mut writer = Vec::new();

        let uri = URI.parse().unwrap();
        let res = Request::new(&uri).send(&mut writer).unwrap();

        assert_ne!(u16::from(res.status_code()), UNSUCCESS_CODE);
    }

    #[ignore]
    #[test]
    fn request_get() {
        let mut writer = Vec::new();
        let res = get(URI, &mut writer).unwrap();

        assert_ne!(u16::from(res.status_code()), UNSUCCESS_CODE);

        let mut writer = Vec::with_capacity(200);
        let res = get(URI_S, &mut writer).unwrap();

        assert_ne!(u16::from(res.status_code()), UNSUCCESS_CODE);
    }

    #[ignore]
    #[test]
    fn request_head() {
        let res = head(URI).unwrap();
        assert_ne!(u16::from(res.status_code()), UNSUCCESS_CODE);

        let res = head(URI_S).unwrap();
        assert_ne!(u16::from(res.status_code()), UNSUCCESS_CODE);
    }
}
