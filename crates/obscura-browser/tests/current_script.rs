use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::thread;

use obscura_browser::{BrowserContext, Page, WaitUntil};
use serde_json::Value;
use tiny_http::{Header, Response, Server};

struct TestServer {
    base_url: String,
    _shutdown: Arc<AtomicU16>,
    _thread: Option<thread::JoinHandle<()>>,
}

impl TestServer {
    fn start<F>(handler: F) -> Self
    where
        F: Fn(&str) -> Option<(u16, &'static str, String)> + Send + Sync + 'static,
    {
        let server = Server::http("127.0.0.1:0").expect("bind tiny_http");
        let port = server.server_addr().to_ip().unwrap().port();
        let base_url = format!("http://127.0.0.1:{}", port);

        let shutdown = Arc::new(AtomicU16::new(0));
        let shutdown_clone = shutdown.clone();
        let thread = thread::spawn(move || {
            for request in server.incoming_requests() {
                if shutdown_clone.load(Ordering::Relaxed) != 0 {
                    break;
                }
                let url = request.url().to_string();
                let result = handler(&url);
                let response = match result {
                    Some((status, content_type, body)) => Response::from_string(body)
                        .with_status_code(status)
                        .with_header(
                            Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes())
                                .unwrap(),
                        ),
                    None => Response::from_string("not found").with_status_code(404),
                };
                let _ = request.respond(response);
            }
        });

        TestServer {
            base_url,
            _shutdown: shutdown,
            _thread: Some(thread),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

fn make_page() -> Page {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_IPS", "1");
    let ctx = Arc::new(BrowserContext::new("ctx".into()));
    Page::new("p1".into(), ctx)
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn current_script_is_element_during_inline_classic() {
    let server = TestServer::start(|url| match url {
        "/" => Some((
            200,
            "text/html",
            "<!doctype html><html><body>\
                <div id=out></div>\
                <script id=\"the-script\" data-marker=\"hello\">\
                    document.getElementById('out').textContent = \
                        document.currentScript.getAttribute('data-marker');\
                </script>\
                </body></html>"
                .into(),
        )),
        _ => None,
    });

    rt().block_on(async {
        let mut page = make_page();
        page.navigate_with_wait(&server.url("/"), WaitUntil::Load)
            .await
            .unwrap();
        let v = page.evaluate("document.getElementById('out').textContent");
        assert_eq!(v, Value::String("hello".into()));
    });
}

#[test]
fn current_script_is_element_during_external_classic() {
    let server = TestServer::start(|url| match url {
        "/" => Some((
            200,
            "text/html",
            "<!doctype html><html><body>\
                <div id=out></div>\
                <script src=\"/x.js\" data-marker=\"ext-hello\"></script>\
                </body></html>"
                .into(),
        )),
        "/x.js" => Some((
            200,
            "application/javascript",
            "document.getElementById('out').textContent = \
                document.currentScript.getAttribute('data-marker');"
                .into(),
        )),
        _ => None,
    });

    rt().block_on(async {
        let mut page = make_page();
        page.navigate_with_wait(&server.url("/"), WaitUntil::Load)
            .await
            .unwrap();
        let v = page.evaluate("document.getElementById('out').textContent");
        assert_eq!(v, Value::String("ext-hello".into()));
    });
}

#[test]
fn current_script_is_null_in_module_execution() {
    let server = TestServer::start(|url| match url {
        "/" => Some((
            200,
            "text/html",
            "<!doctype html><html><body>\
                <div id=out></div>\
                <script type=\"module\">\
                    document.getElementById('out').textContent = \
                        (document.currentScript === null) ? 'null-in-module' : 'wrong';\
                </script>\
                </body></html>"
                .into(),
        )),
        _ => None,
    });

    rt().block_on(async {
        let mut page = make_page();
        page.navigate_with_wait(&server.url("/"), WaitUntil::Load)
            .await
            .unwrap();
        let v = page.evaluate("document.getElementById('out').textContent");
        assert_eq!(v, Value::String("null-in-module".into()));
    });
}

#[test]
fn current_script_is_null_outside_script_execution() {
    let server = TestServer::start(|url| match url {
        "/" => Some((
            200,
            "text/html",
            "<!doctype html><html><body><div id=out></div></body></html>".into(),
        )),
        _ => None,
    });

    rt().block_on(async {
        let mut page = make_page();
        page.navigate_with_wait(&server.url("/"), WaitUntil::Load)
            .await
            .unwrap();
        let v = page.evaluate(
            "(document.currentScript === null) ? 'null-outside' : String(typeof document.currentScript)",
        );
        assert_eq!(v, Value::String("null-outside".into()));
    });
}

#[test]
fn current_script_pops_after_classic_execution() {
    let server = TestServer::start(|url| match url {
        "/" => Some((
            200,
            "text/html",
            "<!doctype html><html><body>\
                <div id=out></div>\
                <script>window.__seen_during = document.currentScript ? 'el' : 'null';</script>\
                <script>document.getElementById('out').textContent = \
                    window.__seen_during + '|' + (document.currentScript ? 'el' : 'null');</script>\
                </body></html>"
                .into(),
        )),
        _ => None,
    });

    rt().block_on(async {
        let mut page = make_page();
        page.navigate_with_wait(&server.url("/"), WaitUntil::Load)
            .await
            .unwrap();
        let v = page.evaluate("document.getElementById('out').textContent");
        assert_eq!(v, Value::String("el|el".into()));
        let outside = page.evaluate("document.currentScript === null ? 'null' : 'leaked'");
        assert_eq!(outside, Value::String("null".into()));
    });
}
