use std::fmt;
use std::future::Future;
use std::net::IpAddr;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use futures_channel::{mpsc, oneshot};
use futures_util::{StreamExt, TryFutureExt};

use log::{error, trace};

use super::request::{Request, RequestBuilder};
use super::response::Response;
use super::wait;
use crate::{async_impl, header, IntoUrl, Method, Proxy, RedirectPolicy};
#[cfg(feature = "tls")]
use crate::{Certificate, Identity};

/// A `Client` to make Requests with.
///
/// The Client has various configuration values to tweak, but the defaults
/// are set to what is usually the most commonly desired value. To configure a
/// `Client`, use `Client::builder()`.
///
/// The `Client` holds a connection pool internally, so it is advised that
/// you create one and **reuse** it.
///
/// # Examples
///
/// ```rust
/// use reqwest::blocking::Client;
/// #
/// # fn run() -> Result<(), reqwest::Error> {
/// let client = Client::new();
/// let resp = client.get("http://httpbin.org/").send()?;
/// #   drop(resp);
/// #   Ok(())
/// # }
///
/// ```
#[derive(Clone)]
pub struct Client {
    inner: ClientHandle,
}

/// A `ClientBuilder` can be used to create a `Client` with  custom configuration.
///
/// # Example
///
/// ```
/// # fn run() -> Result<(), reqwest::Error> {
/// use std::time::Duration;
///
/// let client = reqwest::blocking::Client::builder()
///     .timeout(Duration::from_secs(10))
///     .build()?;
/// # Ok(())
/// # }
/// ```
pub struct ClientBuilder {
    inner: async_impl::ClientBuilder,
    timeout: Timeout,
}

impl ClientBuilder {
    /// Constructs a new `ClientBuilder`.
    ///
    /// This is the same as `Client::builder()`.
    pub fn new() -> ClientBuilder {
        ClientBuilder {
            inner: async_impl::ClientBuilder::new(),
            timeout: Timeout::default(),
        }
    }

    /// Returns a `Client` that uses this `ClientBuilder` configuration.
    ///
    /// # Errors
    ///
    /// This method fails if TLS backend cannot be initialized, or the resolver
    /// cannot load the system configuration.
    pub fn build(self) -> crate::Result<Client> {
        ClientHandle::new(self).map(|handle| Client { inner: handle })
    }


    // Higher-level options

    /// Sets the default headers for every request.
    ///
    /// # Example
    ///
    /// ```rust
    /// use reqwest::header;
    /// # fn build_client() -> Result<(), reqwest::Error> {
    /// let mut headers = header::HeaderMap::new();
    /// headers.insert(header::AUTHORIZATION, header::HeaderValue::from_static("secret"));
    ///
    /// // get a client builder
    /// let client = reqwest::blocking::Client::builder()
    ///     .default_headers(headers)
    ///     .build()?;
    /// let res = client.get("https://www.rust-lang.org").send()?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Override the default headers:
    ///
    /// ```rust
    /// use reqwest::header;
    /// # fn build_client() -> Result<(), reqwest::Error> {
    /// let mut headers = header::HeaderMap::new();
    /// headers.insert(header::AUTHORIZATION, header::HeaderValue::from_static("secret"));
    ///
    /// // get a client builder
    /// let client = reqwest::blocking::Client::builder()
    ///     .default_headers(headers)
    ///     .build()?;
    /// let res = client
    ///     .get("https://www.rust-lang.org")
    ///     .header(header::AUTHORIZATION, "token")
    ///     .send()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn default_headers(self, headers: header::HeaderMap) -> ClientBuilder {
        self.with_inner(move |inner| inner.default_headers(headers))
    }

    /// Enable a persistent cookie store for the client.
    ///
    /// Cookies received in responses will be preserved and included in
    /// additional requests.
    ///
    /// By default, no cookie store is used.
    ///
    /// # Optional
    ///
    /// This requires the optional `cookies` feature to be enabled.
    #[cfg(feature = "cookies")]
    pub fn cookie_store(self, enable: bool) -> ClientBuilder {
        self.with_inner(|inner| inner.cookie_store(enable))
    }

    /// Enable auto gzip decompression by checking the `Content-Encoding` response header.
    ///
    /// If auto gzip decompresson is turned on:
    ///
    /// - When sending a request and if the request's headers do not already contain
    ///   an `Accept-Encoding` **and** `Range` values, the `Accept-Encoding` header is set to `gzip`.
    ///   The request body is **not** automatically compressed.
    /// - When receiving a response, if it's headers contain a `Content-Encoding` value that
    ///   equals to `gzip`, both values `Content-Encoding` and `Content-Length` are removed from the
    ///   headers' set. The response body is automatically decompressed.
    ///
    /// If the `gzip` feature is turned on, the default option is enabled.
    ///
    /// # Optional
    ///
    /// This requires the optional `gzip` feature to be enabled
    #[cfg(feature = "gzip")]
    pub fn gzip(self, enable: bool) -> ClientBuilder {
        self.with_inner(|inner| inner.gzip(enable))
    }

    /// Disable auto response body gzip decompression.
    ///
    /// This method exists even if the optional `gzip` feature is not enabled.
    /// This can be used to ensure a `Client` doesn't use gzip decompression
    /// even if another dependency were to enable the optional `gzip` feature.
    pub fn no_gzip(self) -> ClientBuilder {
        self.with_inner(|inner| inner.no_gzip())
    }

    // Redirect options

    /// Set a `RedirectPolicy` for this client.
    ///
    /// Default will follow redirects up to a maximum of 10.
    pub fn redirect(self, policy: RedirectPolicy) -> ClientBuilder {
        self.with_inner(move |inner| inner.redirect(policy))
    }

    /// Enable or disable automatic setting of the `Referer` header.
    ///
    /// Default is `true`.
    pub fn referer(self, enable: bool) -> ClientBuilder {
        self.with_inner(|inner| inner.referer(enable))
    }

    // Proxy options

    /// Add a `Proxy` to the list of proxies the `Client` will use.
    pub fn proxy(self, proxy: Proxy) -> ClientBuilder {
        self.with_inner(move |inner| inner.proxy(proxy))
    }

    /// Disable proxy setting.
    pub fn no_proxy(self) -> ClientBuilder {
        self.with_inner(move |inner| inner.no_proxy())
    }

    /// Enable system proxy setting.
    pub fn use_sys_proxy(self) -> ClientBuilder {
        self.with_inner(move |inner| inner.use_sys_proxy())
    }

    // Timeout options

    /// Set a timeout for connect, read and write operations of a `Client`.
    ///
    /// Default is 30 seconds.
    ///
    /// Pass `None` to disable timeout.
    pub fn timeout<T>(mut self, timeout: T) -> ClientBuilder
    where
        T: Into<Option<Duration>>,
    {
        self.timeout = Timeout(timeout.into());
        self
    }

    /// Set a timeout for only the connect phase of a `Client`.
    ///
    /// Default is `None`.
    pub fn connect_timeout<T>(self, timeout: T) -> ClientBuilder
    where
        T: Into<Option<Duration>>,
    {
        let timeout = timeout.into();
        if let Some(dur) = timeout {
            self.with_inner(|inner| inner.connect_timeout(dur))
        } else {
            self
        }
    }

    // HTTP options

    /// Sets the maximum idle connection per host allowed in the pool.
    pub fn max_idle_per_host(self, max: usize) -> ClientBuilder {
        self.with_inner(move |inner| inner.max_idle_per_host(max))
    }

    /// Enable case sensitive headers.
    pub fn http1_title_case_headers(self) -> ClientBuilder {
        self.with_inner(|inner| inner.http1_title_case_headers())
    }

    /// Only use HTTP/2.
    pub fn http2_prior_knowledge(self) -> ClientBuilder {
        self.with_inner(|inner| inner.http2_prior_knowledge())
    }

    // TCP options

    /// Set that all sockets have `SO_NODELAY` set to `true`.
    pub fn tcp_nodelay(self) -> ClientBuilder {
        self.with_inner(move |inner| inner.tcp_nodelay())
    }

    /// Bind to a local IP Address.
    ///
    /// # Example
    ///
    /// ```
    /// use std::net::IpAddr;
    /// let local_addr = IpAddr::from([12, 4, 1, 8]);
    /// let client = reqwest::blocking::Client::builder()
    ///     .local_address(local_addr)
    ///     .build().unwrap();
    /// ```
    pub fn local_address<T>(self, addr: T) -> ClientBuilder
    where
        T: Into<Option<IpAddr>>,
    {
        self.with_inner(move |inner| inner.local_address(addr))
    }

    // TLS options

    /// Add a custom root certificate.
    ///
    /// This allows connecting to a server that has a self-signed
    /// certificate for example. This **does not** replace the existing
    /// trusted store.
    ///
    /// # Example
    ///
    /// ```
    /// # use std::fs::File;
    /// # use std::io::Read;
    /// # fn build_client() -> Result<(), Box<dyn std::error::Error>> {
    /// // read a local binary DER encoded certificate
    /// let der = std::fs::read("my-cert.der")?;
    ///
    /// // create a certificate
    /// let cert = reqwest::Certificate::from_der(&der)?;
    ///
    /// // get a client builder
    /// let client = reqwest::blocking::Client::builder()
    ///     .add_root_certificate(cert)
    ///     .build()?;
    /// # drop(client);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Optional
    ///
    /// This requires the optional `default-tls` or `rustls-tls` feature to be
    /// enabled.
    #[cfg(feature = "tls")]
    pub fn add_root_certificate(self, cert: Certificate) -> ClientBuilder {
        self.with_inner(move |inner| inner.add_root_certificate(cert))
    }

    /// Sets the identity to be used for client certificate authentication.
    ///
    /// # Example
    ///
    /// ```
    /// # use std::fs::File;
    /// # use std::io::Read;
    /// # fn build_client() -> Result<(), Box<std::error::Error>> {
    /// // read a local PKCS12 bundle
    /// let mut buf = Vec::new();
    ///
    /// #[cfg(feature = "default-tls")]
    /// File::open("my-ident.pfx")?.read_to_end(&mut buf)?;
    /// #[cfg(feature = "rustls-tls")]
    /// File::open("my-ident.pem")?.read_to_end(&mut buf)?;
    ///
    /// #[cfg(feature = "default-tls")]
    /// // create an Identity from the PKCS#12 archive
    /// let pkcs12 = reqwest::Identity::from_pkcs12_der(&buf, "my-privkey-password")?;
    /// #[cfg(feature = "rustls-tls")]
    /// // create an Identity from the PEM file
    /// let pkcs12 = reqwest::Identity::from_pem(&buf)?;
    ///
    /// // get a client builder
    /// let client = reqwest::blocking::Client::builder()
    ///     .identity(pkcs12)
    ///     .build()?;
    /// # drop(client);
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "tls")]
    pub fn identity(self, identity: Identity) -> ClientBuilder {
        self.with_inner(move |inner| inner.identity(identity))
    }

    /// Controls the use of hostname verification.
    ///
    /// Defaults to `false`.
    ///
    /// # Warning
    ///
    /// You should think very carefully before you use this method. If
    /// hostname verification is not used, any valid certificate for any
    /// site will be trusted for use from any other. This introduces a
    /// significant vulnerability to man-in-the-middle attacks.
    #[cfg(feature = "default-tls")]
    pub fn danger_accept_invalid_hostnames(self, accept_invalid_hostname: bool) -> ClientBuilder {
        self.with_inner(|inner| inner.danger_accept_invalid_hostnames(accept_invalid_hostname))
    }

    /// Controls the use of certificate validation.
    ///
    /// Defaults to `false`.
    ///
    /// # Warning
    ///
    /// You should think very carefully before using this method. If
    /// invalid certificates are trusted, *any* certificate for *any* site
    /// will be trusted for use. This includes expired certificates. This
    /// introduces significant vulnerabilities, and should only be used
    /// as a last resort.
    #[cfg(feature = "tls")]
    pub fn danger_accept_invalid_certs(self, accept_invalid_certs: bool) -> ClientBuilder {
        self.with_inner(|inner| inner.danger_accept_invalid_certs(accept_invalid_certs))
    }

    /// Force using the native TLS backend.
    ///
    /// Since multiple TLS backends can be optionally enabled, this option will
    /// force the `native-tls` backend to be used for this `Client`.
    ///
    /// # Optional
    ///
    /// This requires the optional `default-tls` feature to be enabled.
    #[cfg(feature = "default-tls")]
    pub fn use_default_tls(self) -> ClientBuilder {
        self.with_inner(move |inner| inner.use_default_tls())
    }

    /// Force using the Rustls TLS backend.
    ///
    /// Since multiple TLS backends can be optionally enabled, this option will
    /// force the `rustls` backend to be used for this `Client`.
    ///
    /// # Optional
    ///
    /// This requires the optional `rustls-tls` feature to be enabled.
    #[cfg(feature = "rustls-tls")]
    pub fn use_rustls_tls(self) -> ClientBuilder {
        self.with_inner(move |inner| inner.use_rustls_tls())
    }

    // private

    fn with_inner<F>(mut self, func: F) -> ClientBuilder
    where
        F: FnOnce(async_impl::ClientBuilder) -> async_impl::ClientBuilder,
    {
        self.inner = func(self.inner);
        self
    }
}

impl Client {
    /// Constructs a new `Client`.
    ///
    /// # Panic
    ///
    /// This method panics if TLS backend cannot initialized, or the resolver
    /// cannot load the system configuration.
    ///
    /// Use `Client::builder()` if you wish to handle the failure as an `Error`
    /// instead of panicking.
    pub fn new() -> Client {
        ClientBuilder::new().build().expect("Client::new()")
    }

    /// Creates a `ClientBuilder` to configure a `Client`.
    ///
    /// This is the same as `ClientBuilder::new()`.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    /// Convenience method to make a `GET` request to a URL.
    ///
    /// # Errors
    ///
    /// This method fails whenever supplied `Url` cannot be parsed.
    pub fn get<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::GET, url)
    }

    /// Convenience method to make a `POST` request to a URL.
    ///
    /// # Errors
    ///
    /// This method fails whenever supplied `Url` cannot be parsed.
    pub fn post<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::POST, url)
    }

    /// Convenience method to make a `PUT` request to a URL.
    ///
    /// # Errors
    ///
    /// This method fails whenever supplied `Url` cannot be parsed.
    pub fn put<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::PUT, url)
    }

    /// Convenience method to make a `PATCH` request to a URL.
    ///
    /// # Errors
    ///
    /// This method fails whenever supplied `Url` cannot be parsed.
    pub fn patch<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::PATCH, url)
    }

    /// Convenience method to make a `DELETE` request to a URL.
    ///
    /// # Errors
    ///
    /// This method fails whenever supplied `Url` cannot be parsed.
    pub fn delete<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::DELETE, url)
    }

    /// Convenience method to make a `HEAD` request to a URL.
    ///
    /// # Errors
    ///
    /// This method fails whenever supplied `Url` cannot be parsed.
    pub fn head<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.request(Method::HEAD, url)
    }

    /// Start building a `Request` with the `Method` and `Url`.
    ///
    /// Returns a `RequestBuilder`, which will allow setting headers and
    /// request body before sending.
    ///
    /// # Errors
    ///
    /// This method fails whenever supplied `Url` cannot be parsed.
    pub fn request<U: IntoUrl>(&self, method: Method, url: U) -> RequestBuilder {
        let req = url.into_url().map(move |url| Request::new(method, url));
        RequestBuilder::new(self.clone(), req)
    }

    /// Executes a `Request`.
    ///
    /// A `Request` can be built manually with `Request::new()` or obtained
    /// from a RequestBuilder with `RequestBuilder::build()`.
    ///
    /// You should prefer to use the `RequestBuilder` and
    /// `RequestBuilder::send()`.
    ///
    /// # Errors
    ///
    /// This method fails if there was an error while sending request,
    /// redirect loop was detected or redirect limit was exhausted.
    pub fn execute(&self, request: Request) -> crate::Result<Response> {
        self.inner.execute_request(request)
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Client")
            //.field("gzip", &self.inner.gzip)
            //.field("redirect_policy", &self.inner.redirect_policy)
            //.field("referer", &self.inner.referer)
            .finish()
    }
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.inner.fmt(f)
    }
}

#[derive(Clone)]
struct ClientHandle {
    timeout: Timeout,
    inner: Arc<InnerClientHandle>,
}

type OneshotResponse = oneshot::Sender<crate::Result<async_impl::Response>>;
type ThreadSender = mpsc::UnboundedSender<(async_impl::Request, OneshotResponse)>;

struct InnerClientHandle {
    tx: Option<ThreadSender>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Drop for InnerClientHandle {
    fn drop(&mut self) {
        self.tx.take();
        self.thread.take().map(|h| h.join());
    }
}

impl ClientHandle {
    fn new(builder: ClientBuilder) -> crate::Result<ClientHandle> {
        let timeout = builder.timeout;
        let builder = builder.inner;
        let (tx, rx) = mpsc::unbounded::<(async_impl::Request, OneshotResponse)>();
        let (spawn_tx, spawn_rx) = oneshot::channel::<crate::Result<()>>();
        let handle = thread::Builder::new()
            .name("reqwest-internal-sync-runtime".into())
            .spawn(move || {
                use tokio::runtime::current_thread::Runtime;

                let mut rt = match Runtime::new().map_err(crate::error::builder) {
                    Err(e) => {
                        if let Err(e) = spawn_tx.send(Err(e)) {
                            error!("Failed to communicate runtime creation failure: {:?}", e);
                        }
                        return;
                    }
                    Ok(v) => v,
                };

                let f = async move {
                    let client = match builder.build() {
                        Err(e) => {
                            if let Err(e) = spawn_tx.send(Err(e)) {
                                error!("Failed to communicate client creation failure: {:?}", e);
                            }
                            return;
                        }
                        Ok(v) => v,
                    };
                    if let Err(e) = spawn_tx.send(Ok(())) {
                        error!("Failed to communicate successful startup: {:?}", e);
                        return;
                    }

                    let mut rx = rx;

                    while let Some((req, req_tx)) = rx.next().await {
                        let req_fut = client.execute(req);
                        tokio::spawn(forward(req_fut, req_tx));
                    }

                    trace!("Receiver is shutdown");
                };

                rt.block_on(f)
            })
            .map_err(crate::error::builder)?;

        // Wait for the runtime thread to start up...
        match wait::timeout(spawn_rx, None) {
            Ok(Ok(())) => (),
            Ok(Err(err)) => return Err(err),
            Err(_canceled) => event_loop_panicked(),
        }

        let inner_handle = Arc::new(InnerClientHandle {
            tx: Some(tx),
            thread: Some(handle),
        });

        Ok(ClientHandle {
            timeout,
            inner: inner_handle,
        })
    }

    fn execute_request(&self, req: Request) -> crate::Result<Response> {
        let (tx, rx) = oneshot::channel();
        let (req, body) = req.into_async();
        let url = req.url().clone();
        self.inner
            .tx
            .as_ref()
            .expect("core thread exited early")
            .unbounded_send((req, tx))
            .expect("core thread panicked");

        let result: Result<crate::Result<async_impl::Response>, wait::Waited<crate::Error>> =
            if let Some(body) = body {
                let f = async move {
                    body.send().await?;
                    rx.await.map_err(|_canceled| event_loop_panicked())
                };
                wait::timeout(f, self.timeout.0)
            } else {
                wait::timeout(
                    rx.map_err(|_canceled| event_loop_panicked()),
                    self.timeout.0,
                )
            };

        match result {
            Ok(Err(err)) => Err(err.with_url(url)),
            Ok(Ok(res)) => Ok(Response::new(
                res,
                self.timeout.0,
                KeepCoreThreadAlive(Some(self.inner.clone())),
            )),
            Err(wait::Waited::TimedOut(e)) => Err(crate::error::request(e).with_url(url)),
            Err(wait::Waited::Executor(err)) => Err(crate::error::request(err).with_url(url)),
            Err(wait::Waited::Inner(err)) => Err(err.with_url(url)),
        }
    }
}

async fn forward<F>(fut: F, mut tx: OneshotResponse)
where
    F: Future<Output = crate::Result<async_impl::Response>>,
{
    use std::task::Poll;

    futures_util::pin_mut!(fut);

    // "select" on the sender being canceled, and the future completing
    let res = futures_util::future::poll_fn(|cx| {
        match fut.as_mut().poll(cx) {
            Poll::Ready(val) => Poll::Ready(Some(val)),
            Poll::Pending => {
                // check if the callback is canceled
                futures_core::ready!(tx.poll_cancel(cx));
                Poll::Ready(None)
            }
        }
    })
    .await;

    if let Some(res) = res {
        let _ = tx.send(res);
    }
    // else request is canceled
}

#[derive(Clone, Copy)]
struct Timeout(Option<Duration>);

impl Default for Timeout {
    fn default() -> Timeout {
        // default mentioned in ClientBuilder::timeout() doc comment
        Timeout(Some(Duration::from_secs(30)))
    }
}

pub(crate) struct KeepCoreThreadAlive(Option<Arc<InnerClientHandle>>);

impl KeepCoreThreadAlive {
    pub(crate) fn empty() -> KeepCoreThreadAlive {
        KeepCoreThreadAlive(None)
    }
}

#[cold]
#[inline(never)]
fn event_loop_panicked() -> ! {
    // The only possible reason there would be a Canceled error
    // is if the thread running the event loop panicked. We could return
    // an Err here, like a BrokenPipe, but the Client is not
    // recoverable. Additionally, the panic in the other thread
    // is not normal, and should likely be propagated.
    panic!("event loop thread panicked");
}