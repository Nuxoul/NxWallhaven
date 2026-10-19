//! Application-owned WebView2 session. Cookies never leave this browser profile.
use crate::browser_profile::{self, BrowserProfile};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::platform::windows::{
    EventLoopBuilderExtWindows, WindowBuilderExtWindows, WindowExtWindows,
};
use tao::window::WindowBuilder;
use webview2_com::{
    ClearBrowsingDataCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Profile2, ICoreWebView2Settings4, ICoreWebView2_13,
    },
};
use windows::core::Interface;
use wry::{WebContext, WebViewBuilder, WebViewExtWindows};

const ORIGIN: &str = "https://wallhaven.cc";
const MAX_BODY: usize = 2 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub struct BrowserResponse {
    pub status: u16,
    pub url: String,
    pub body: String,
}
#[derive(Clone)]
pub struct BrowserSession {
    inner: Arc<Inner>,
}
struct Inner {
    proxy: EventLoopProxy<Command>,
    closed: Arc<AtomicBool>,
    serial: Mutex<()>,
    counter: AtomicU64,
    origin: String,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.proxy.send_event(Command::Shutdown);
        if let Ok(thread) = self.thread.get_mut() {
            if let Some(thread) = thread.take() {
                let _ = thread.join();
            }
        }
    }
}
enum Command {
    Show(mpsc::Sender<Result<(), String>>),
    Hide(mpsc::Sender<Result<(), String>>),
    Navigate(String, mpsc::Sender<Result<(), String>>),
    Prepare(mpsc::Sender<Result<(), String>>),
    Clear(mpsc::Sender<Result<(), String>>),
    Cleared(bool),
    Evaluate(u64, String, mpsc::Sender<Result<BrowserResponse, String>>),
    Ipc(String, String),
    Shutdown,
}
struct Pending {
    reply: mpsc::Sender<Result<BrowserResponse, String>>,
    deadline: Instant,
}

impl BrowserSession {
    pub fn new() -> Result<Self, String> {
        Self::with_profile(
            ORIGIN.to_string(),
            BrowserProfile::persistent(browser_profile::saved_profile_path()?)?,
        )
    }
    fn with_profile(origin: String, profile: BrowserProfile) -> Result<Self, String> {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let closed = Arc::new(AtomicBool::new(false));
        let thread_closed = closed.clone();
        let thread_origin = origin.clone();
        let thread = std::thread::Builder::new()
            .name("wallhaven-webview".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    browser_thread(ready_tx.clone(), thread_closed, thread_origin, profile)
                }));
                if result.is_err() {
                    let _ = ready_tx.send(Err("无法初始化 WebView2 窗口".into()));
                }
            })
            .map_err(|_| "无法启动浏览器线程".to_string())?;
        let proxy = ready_rx
            .recv_timeout(Duration::from_secs(40))
            .map_err(|_| "创建 WebView2 窗口超时".to_string())??;
        Ok(Self {
            inner: Arc::new(Inner {
                proxy,
                closed,
                serial: Mutex::new(()),
                counter: AtomicU64::new(1),
                origin,
                thread: Mutex::new(Some(thread)),
            }),
        })
    }
    pub fn prepare_restore(&self) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        self.inner
            .proxy
            .send_event(Command::Prepare(tx))
            .map_err(|_| "浏览器会话已退出")?;
        rx.recv_timeout(REQUEST_TIMEOUT)
            .map_err(|_| "加载已保存会话超时")?
    }
    pub fn clear_data(&self) -> Result<(), String> {
        let _serial = self.inner.serial.lock().map_err(|_| "浏览器会话不可用")?;
        let (tx, rx) = mpsc::channel();
        self.inner
            .proxy
            .send_event(Command::Clear(tx))
            .map_err(|_| "浏览器会话已退出")?;
        rx.recv_timeout(REQUEST_TIMEOUT)
            .map_err(|_| "清除浏览器登录数据超时，尚未确认完成")?
    }
    pub fn shutdown(&self) {
        let _ = self.inner.proxy.send_event(Command::Shutdown);
        if let Ok(mut thread) = self.inner.thread.lock() {
            if let Some(thread) = thread.take() {
                let _ = thread.join();
            }
        }
    }
    pub fn show_login(&self) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        self.inner
            .proxy
            .send_event(Command::Show(tx))
            .map_err(|_| "浏览器窗口已退出".to_string())?;
        rx.recv_timeout(REQUEST_TIMEOUT)
            .map_err(|_| "打开浏览器窗口超时".to_string())?
    }
    pub fn show_path(&self, path: &str) -> Result<(), String> {
        if !safe_instance_path(path, &self.inner.origin) {
            return Err("拒绝非同源管理地址".into());
        }
        let (tx, rx) = mpsc::channel();
        self.inner
            .proxy
            .send_event(Command::Navigate(
                format!("{}{path}", self.inner.origin),
                tx,
            ))
            .map_err(|_| "浏览器会话已退出".to_string())?;
        rx.recv_timeout(REQUEST_TIMEOUT)
            .map_err(|_| "打开管理页面超时".to_string())?
    }
    pub fn hide(&self) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        self.inner
            .proxy
            .send_event(Command::Hide(tx))
            .map_err(|_| "浏览器窗口已退出".to_string())?;
        rx.recv_timeout(REQUEST_TIMEOUT)
            .map_err(|_| "隐藏浏览器窗口超时".to_string())?
    }
    pub fn was_closed(&self) -> bool {
        self.inner.closed.load(Ordering::Acquire)
    }
    pub fn current_html(&self) -> Result<String, String> {
        Ok(self.evaluate(Operation::Current)?.body)
    }
    pub fn get(&self, path: &str) -> Result<BrowserResponse, String> {
        if !safe_instance_path(path, &self.inner.origin) {
            return Err("拒绝非同源网站请求".into());
        }
        self.evaluate(Operation::Fetch {
            url: format!("{}{path}", self.inner.origin),
            fields: None,
            csrf: None,
        })
    }
    pub fn post(&self, url: &str, csrf: &str) -> Result<BrowserResponse, String> {
        if !same_origin(url, &self.inner.origin)
            || csrf.is_empty()
            || csrf.len() > 8192
            || csrf.chars().any(char::is_control)
        {
            return Err("收藏请求参数无效".into());
        }
        self.evaluate(Operation::Fetch {
            url: url.into(),
            csrf: Some(csrf.into()),
            fields: None,
        })
    }
    pub fn post_form(
        &self,
        url: &str,
        fields: &[(String, String)],
        csrf: &str,
    ) -> Result<BrowserResponse, String> {
        if !same_origin(url, &self.inner.origin)
            || csrf.is_empty()
            || csrf.len() > 8192
            || csrf.chars().any(char::is_control)
            || fields.len() > 100
            || fields.iter().any(|(k, v)| k.len() > 256 || v.len() > 16384)
        {
            return Err("管理表单参数无效".into());
        }
        self.evaluate(Operation::Fetch {
            url: url.into(),
            csrf: Some(csrf.into()),
            fields: Some(fields.to_vec()),
        })
    }
    fn evaluate(&self, operation: Operation) -> Result<BrowserResponse, String> {
        let _serial = self
            .inner
            .serial
            .lock()
            .map_err(|_| "浏览器会话不可用".to_string())?;
        let id = self.inner.counter.fetch_add(1, Ordering::AcqRel);
        let script = make_script_for_origin(id, operation, &self.inner.origin)?;
        let (tx, rx) = mpsc::channel();
        self.inner
            .proxy
            .send_event(Command::Evaluate(id, script, tx))
            .map_err(|_| "浏览器窗口已退出".to_string())?;
        rx.recv_timeout(REQUEST_TIMEOUT + Duration::from_secs(2))
            .map_err(|_| "浏览器请求超时；写操作未自动重试".to_string())?
    }
}

enum Operation {
    Current,
    Fetch {
        url: String,
        csrf: Option<String>,
        fields: Option<Vec<(String, String)>>,
    },
}
#[cfg(test)]
fn safe_url(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("wallhaven.cc")
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none()
    })
}
#[cfg(test)]
fn safe_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && !path.contains('\\')
        && safe_url(&format!("{ORIGIN}{path}"))
}
fn same_origin(value: &str, origin: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    let Ok(expected) = reqwest::Url::parse(origin) else {
        return false;
    };
    url.origin() == expected.origin() && url.username().is_empty() && url.password().is_none()
}
fn safe_instance_path(path: &str, origin: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && !path.contains('\\')
        && same_origin(&format!("{origin}{path}"), origin)
}
#[cfg(test)]
fn make_script(id: u64, operation: Operation) -> Result<String, String> {
    make_script_for_origin(id, operation, ORIGIN)
}
fn make_script_for_origin(id: u64, operation: Operation, origin: &str) -> Result<String, String> {
    let task = match operation {
        Operation::Current => r#"
          const panel=document.querySelector('#userpanel');
          const out=document.createElement('div');
          if(panel){const p=panel.cloneNode(true);p.querySelectorAll('input,textarea,script').forEach(e=>e.remove());out.appendChild(p);}
          const challenge=document.querySelector('[name="cf-turnstile-response"],.cf-turnstile,.g-recaptcha,#challenge-form');
          if(challenge||/just a moment|请稍候|安全验证/i.test(document.title)){out.appendChild(document.createTextNode(' cf-chl-human-verification '));}
          send({status:200,url:location.href,body:out.innerHTML});
        "#.to_string(),
        Operation::Fetch { url, csrf, fields } => {
            let target = serde_json::to_string(&url).map_err(|_| "请求编码失败".to_string())?;
            let token = serde_json::to_string(&csrf).map_err(|_| "请求编码失败".to_string())?;
            let fields = serde_json::to_string(&fields).map_err(|_| "表单编码失败".to_string())?;
            format!(r#"
              const target={target}, token={token}, fields={fields};
              if(new URL(target).origin!==expected)throw Error('origin');
              const headers={{'Accept':token?'application/json':'text/html,application/xhtml+xml'}};
              if(token){{headers['X-CSRF-TOKEN']=token;headers['X-Requested-With']='XMLHttpRequest';}}
              if(fields) headers['Content-Type']='application/x-www-form-urlencoded;charset=UTF-8';
              const abort=new AbortController();const timer=setTimeout(()=>abort.abort(),25000);
              try{{
                const r=await fetch(target,{{method:token?'POST':'GET',headers,body:fields?new URLSearchParams(fields).toString():undefined,credentials:'same-origin',redirect:token?'error':'follow',signal:abort.signal}});
                if(new URL(r.url).origin!==expected)throw Error('origin');
                const reader=r.body?.getReader();const decoder=new TextDecoder();let body='',size=0;
                if(reader){{while(true){{const part=await reader.read();if(part.done)break;size+=part.value.byteLength;if(size>limit){{await reader.cancel();throw Error('limit');}}body+=decoder.decode(part.value,{{stream:true}});}}body+=decoder.decode();}}
                send({{status:r.status,url:r.url,body}});
              }}finally{{clearTimeout(timer);}}
            "#)
        }
    };
    let origin_json = serde_json::to_string(origin).map_err(|_| "来源编码失败".to_string())?;
    Ok(format!(
        r#"(async()=>{{const id={id},expected={origin_json},limit={MAX_BODY};const send=(result)=>window.ipc.postMessage(JSON.stringify({{id,...result}}));try{{if(location.origin!==expected)throw Error('origin');{task}}}catch(_){{send({{error:true}});}}}})();"#
    ))
}
#[cfg(test)]
fn decode_message(text: &str) -> Option<(u64, Result<BrowserResponse, String>)> {
    decode_message_for_origin(text, ORIGIN)
}
fn decode_message_for_origin(
    text: &str,
    origin: &str,
) -> Option<(u64, Result<BrowserResponse, String>)> {
    if text.len() > MAX_BODY * 6 + 4096 {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let id = value.get("id")?.as_u64()?;
    if value.get("error").and_then(|v| v.as_bool()) == Some(true) {
        return Some((
            id,
            Err("浏览器请求未完成；请在登录窗口检查验证状态，写操作未自动重试".into()),
        ));
    }
    let url = value.get("url")?.as_str()?;
    let body = value.get("body")?.as_str()?;
    let status = u16::try_from(value.get("status")?.as_u64()?).ok()?;
    if !same_origin(url, origin) || body.len() > MAX_BODY || !(100..=599).contains(&status) {
        return Some((id, Err("浏览器返回了无效响应".into())));
    }
    Some((
        id,
        Ok(BrowserResponse {
            status,
            url: url.into(),
            body: body.into(),
        }),
    ))
}

fn browser_thread(
    ready: mpsc::SyncSender<Result<EventLoopProxy<Command>, String>>,
    closed: Arc<AtomicBool>,
    origin: String,
    profile: BrowserProfile,
) {
    let mut builder = EventLoopBuilder::<Command>::with_user_event();
    builder.with_any_thread(true);
    let mut event_loop = builder.build();
    let proxy = event_loop.create_proxy();
    let profile_path = profile.path.clone();
    let mut context = WebContext::new(Some(profile_path));
    let window = match WindowBuilder::new()
        .with_title("WallHaven Studio — 网站登录")
        .with_window_classname("WallHavenStudio.WebLogin.WebView2")
        .with_inner_size(tao::dpi::LogicalSize::new(1000.0, 760.0))
        .with_visible(false)
        .build(&event_loop)
    {
        Ok(window) => window,
        Err(_) => {
            let _ = ready.send(Err("无法创建网页登录窗口".into()));
            return;
        }
    };
    let ipc_proxy = proxy.clone();
    let navigation_origin = origin.clone();
    let ipc_origin = origin.clone();
    let webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_url("about:blank")
        .with_navigation_handler(move |url| {
            same_origin(&url, &navigation_origin) || url == "about:blank"
        })
        .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
        .with_ipc_handler(move |request| {
            if same_origin(&request.uri().to_string(), &ipc_origin) {
                let _ = ipc_proxy
                    .send_event(Command::Ipc(request.uri().to_string(), request.into_body()));
            }
        })
        .build(&window);
    let webview = match webview {
        Ok(view) => view,
        Err(_) => {
            let _ = ready.send(Err("无法初始化 WebView2，请检查运行时".into()));
            return;
        }
    };
    // A persistent login profile must not start storing passwords/autofill data.
    let privacy = unsafe {
        webview
            .webview()
            .Settings()
            .and_then(|s| s.cast::<ICoreWebView2Settings4>())
            .and_then(|s| {
                s.SetIsPasswordAutosaveEnabled(false)?;
                s.SetIsGeneralAutofillEnabled(false)
            })
    };
    if privacy.is_err() || profile.mark_initialized().is_err() {
        let _ = ready.send(Err("无法安全初始化持久网页登录资料".into()));
        return;
    }
    if ready.send(Ok(proxy.clone())).is_err() {
        return;
    }
    let mut pending: HashMap<u64, Pending> = HashMap::new();
    let mut login_loaded = false;
    let mut clear_reply: Option<mpsc::Sender<Result<(), String>>> = None;
    event_loop.run_return(|event, _, flow| {
        *flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(250));
        pending.retain(|_, p| {
            if Instant::now() >= p.deadline {
                let _ = p.reply.send(Err("浏览器请求超时；未自动重试".into()));
                false
            } else {
                true
            }
        });
        match event {
            Event::UserEvent(Command::Prepare(reply)) => {
                let result = webview
                    .load_url(&origin)
                    .map_err(|_| "无法加载已保存会话".to_string());
                if result.is_ok() {
                    login_loaded = true;
                }
                let _ = reply.send(result);
            }
            Event::UserEvent(Command::Clear(reply)) => {
                if clear_reply.is_some() {
                    let _ = reply.send(Err("正在清除会话".into()));
                    return;
                }
                let _ = webview.load_url("about:blank");
                window.set_visible(false);
                let clear_proxy = proxy.clone();
                let result = unsafe {
                    webview
                        .webview()
                        .cast::<ICoreWebView2_13>()
                        .and_then(|v| v.Profile())
                        .and_then(|p| p.cast::<ICoreWebView2Profile2>())
                        .and_then(|p| {
                            p.ClearBrowsingDataAll(&ClearBrowsingDataCompletedHandler::create(
                                Box::new(move |result| {
                                    let _ =
                                        clear_proxy.send_event(Command::Cleared(result.is_ok()));
                                    Ok(())
                                }),
                            ))
                        })
                };
                if result.is_err() {
                    let _ = reply.send(Err("无法清除已保存的网站登录数据".into()));
                } else {
                    clear_reply = Some(reply);
                }
            }
            Event::UserEvent(Command::Cleared(success)) => {
                login_loaded = false;
                let result = if success {
                    profile.forget_marker()
                } else {
                    Err("网站登录数据清理未完成".into())
                };
                if let Some(reply) = clear_reply.take() {
                    let _ = reply.send(result);
                }
            }
            Event::UserEvent(Command::Show(reply)) => {
                closed.store(false, Ordering::Release);
                crate::window_chrome::center_native_on_app_window(window.hwnd() as _);
                crate::window_chrome::install_icon(window.hwnd() as _);
                window.set_visible(true);
                window.set_focus();
                let current_is_home = webview
                    .url()
                    .ok()
                    .and_then(|u| reqwest::Url::parse(&u).ok())
                    .is_some_and(|u| u.path() == "/");
                let result = if !login_loaded || current_is_home {
                    webview
                        .load_url(&format!("{origin}/login"))
                        .map_err(|_| "无法打开网站登录页".to_string())
                } else {
                    Ok(())
                };
                if result.is_ok() {
                    login_loaded = true;
                    let _ = profile.mark_initialized();
                }
                let _ = reply.send(result);
            }
            Event::UserEvent(Command::Navigate(url, reply)) => {
                closed.store(false, Ordering::Release);
                crate::window_chrome::center_native_on_app_window(window.hwnd() as _);
                crate::window_chrome::install_icon(window.hwnd() as _);
                window.set_visible(true);
                window.set_focus();
                let result = webview
                    .load_url(&url)
                    .map_err(|_| "无法打开站内收藏管理页".to_string());
                if result.is_ok() {
                    login_loaded = true;
                    let _ = profile.mark_initialized();
                }
                let _ = reply.send(result);
            }
            Event::UserEvent(Command::Hide(reply)) => {
                window.set_visible(false);
                let _ = reply.send(Ok(()));
            }
            Event::UserEvent(Command::Evaluate(id, script, reply)) => {
                let allowed = webview.url().is_ok_and(|url| same_origin(&url, &origin));
                if !allowed {
                    let _ = reply.send(Err("登录页面尚未就绪，请在网页登录窗口完成加载".into()));
                } else {
                    pending.insert(
                        id,
                        Pending {
                            reply,
                            deadline: Instant::now() + REQUEST_TIMEOUT,
                        },
                    );
                    if webview.evaluate_script(&script).is_err() {
                        if let Some(p) = pending.remove(&id) {
                            let _ = p.reply.send(Err("浏览器脚本执行失败".into()));
                        }
                    }
                }
            }
            Event::UserEvent(Command::Ipc(source, text)) => {
                if same_origin(&source, &origin) {
                    if let Some((id, result)) = decode_message_for_origin(&text, &origin) {
                        if let Some(p) = pending.remove(&id) {
                            let _ = p.reply.send(result);
                        }
                    }
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                closed.store(true, Ordering::Release);
                window.set_visible(false);
            }
            Event::UserEvent(Command::Shutdown) => {
                *flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
    for (_, p) in pending {
        let _ = p.reply.send(Err("浏览器会话已结束".into()));
    }
    drop(webview);
    drop(window);
    drop(context);
    if let Some(reply) = clear_reply {
        let _ = reply.send(Err("浏览器已关闭，未确认清理完成".into()));
    }
    drop(profile);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_routes_reject_external_or_credential_urls() {
        for url in [
            "https://evil.test/",
            "https://wallhaven.cc.evil.test/",
            "http://wallhaven.cc/",
            "https://a@wallhaven.cc/",
            "https://wallhaven.cc:444/",
        ] {
            assert!(!safe_url(url));
        }
        for path in ["//evil.test", "https://evil.test", "/\\evil.test"] {
            assert!(!safe_path(path));
        }
        assert!(safe_path("/user/Test/favorites?page=2"));
    }
    #[test]
    fn scripts_serialize_arguments_and_do_not_export_cookies_or_passwords() {
        let script = make_script(
            7,
            Operation::Fetch {
                url: "https://wallhaven.cc/x\";bad()".into(),
                fields: None,
                csrf: Some("a\";bad()".into()),
            },
        )
        .unwrap();
        assert!(script.contains("a\\\";bad()"));
        assert!(script.contains("credentials:'same-origin'"));
        assert!(script.contains("redirect:token?'error'"));
        let read = make_script(8, Operation::Current).unwrap();
        assert!(read.contains("#userpanel"));
        assert!(!read.contains("document.cookie"));
        assert!(!read.contains("document.documentElement.outerHTML"));
    }
    #[test]
    fn invalid_ipc_cannot_complete_with_external_or_oversized_result() {
        let result =
            decode_message(r#"{"id":1,"status":200,"url":"https://evil.test/","body":"secret"}"#)
                .unwrap();
        assert!(result.1.is_err());
        assert!(decode_message("not-json").is_none());
        assert!(decode_message(r#"{"id":2,"error":true}"#)
            .unwrap()
            .1
            .is_err());
    }
    #[test]
    #[ignore = "opens native WebView2; run explicitly on Windows desktop"]
    fn native_webview_session() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = stop.clone();
        let posts = Arc::new(AtomicU64::new(0));
        let server_posts = posts.clone();
        let submitted_form = Arc::new(Mutex::new(None::<String>));
        let server_form = submitted_form.clone();
        let server = std::thread::spawn(move || {
            while !server_stop.load(Ordering::Acquire) {
                let (mut socket, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(_) => {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    let n = socket.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                    if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&buffer[..end]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.split_once(':')
                                    .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if buffer.len() >= end + 4 + length {
                            break;
                        }
                    }
                    if buffer.len() > 65536 {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&buffer);
                let logged = request.to_ascii_lowercase().contains("wh_fixture=ready");
                let login = request.starts_with("GET /login");
                let body = if login || logged && request.starts_with("GET / ") {
                    r#"<div id="userpanel"><a href="/user/Fixture">Fixture</a><form action="/logout"></form></div>"#
                } else if logged && request.starts_with("POST /form") {
                    server_posts.fetch_add(1, Ordering::AcqRel);
                    *server_form.lock().unwrap() = Some(request.to_string());
                    "form-cookie-ok"
                } else if logged && request.starts_with("POST /favorite") {
                    server_posts.fetch_add(1, Ordering::AcqRel);
                    "post-cookie-ok"
                } else if logged {
                    "get-cookie-ok"
                } else {
                    "missing-cookie"
                };
                let cookie = if login {
                    "Set-Cookie: wh_fixture=ready; Path=/; HttpOnly; SameSite=Lax; Max-Age=3600\r\n"
                } else {
                    ""
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n{}",
                    body.len(),
                    cookie,
                    body
                );
                let _ = socket.write_all(response.as_bytes());
            }
        });
        let profile = BrowserProfile::temporary().unwrap();
        let profile_path = profile.path.clone();
        drop(profile);
        let session = BrowserSession::with_profile(
            origin.clone(),
            BrowserProfile::persistent(profile_path.clone()).unwrap(),
        )
        .unwrap();
        session.show_login().unwrap();
        let started = Instant::now();
        loop {
            if session
                .current_html()
                .is_ok_and(|html| html.contains("Fixture"))
            {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "native bridge failed to read fixture panel"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        session.hide().unwrap();
        assert_eq!(session.get("/read").unwrap().body, "get-cookie-ok");
        assert_eq!(
            session
                .post(&format!("{origin}/favorite"), "fixture-csrf")
                .unwrap()
                .body,
            "post-cookie-ok"
        );
        let fields = vec![
            ("label".into(), "中文 收藏夹 & +".into()),
            ("public".into(), "0".into()),
            ("_method".into(), "PATCH".into()),
            ("_token".into(), "fixture-csrf".into()),
        ];
        assert_eq!(
            session
                .post_form(&format!("{origin}/form"), &fields, "fixture-csrf")
                .unwrap()
                .body,
            "form-cookie-ok"
        );
        let request = submitted_form
            .lock()
            .unwrap()
            .clone()
            .expect("form was not received");
        let (headers, body) = request.split_once("\r\n\r\n").unwrap();
        assert!(headers
            .to_ascii_lowercase()
            .contains("application/x-www-form-urlencoded"));
        assert!(headers
            .to_ascii_lowercase()
            .contains("x-csrf-token: fixture-csrf"));
        let decoded = reqwest::Url::parse(&format!("http://fixture.test/?{body}"))
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect::<Vec<_>>();
        assert_eq!(decoded, fields);
        session.show_login().unwrap();
        assert!(session.current_html().unwrap().contains("Fixture"));
        session.hide().unwrap();
        session.shutdown();
        drop(session);
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "browser_session::tests::native_profile_child",
                "--nocapture",
            ])
            .env("WH_FIXTURE_PROFILE", &profile_path)
            .env("WH_FIXTURE_ORIGIN", &origin)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "cross-process profile restore failed: {}",
            String::from_utf8_lossy(&child.stdout)
        );
        let reopened = BrowserSession::with_profile(
            origin.clone(),
            BrowserProfile::persistent(profile_path.clone()).unwrap(),
        )
        .unwrap();
        reopened.prepare_restore().unwrap();
        let started = Instant::now();
        while !reopened
            .current_html()
            .is_ok_and(|html| html.contains("Fixture"))
        {
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "saved login did not survive browser recreation"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(reopened.get("/read").unwrap().body, "get-cookie-ok");
        reopened.clear_data().unwrap();
        reopened.prepare_restore().unwrap();
        let started = Instant::now();
        while reopened.get("/read").is_err() {
            assert!(started.elapsed() < Duration::from_secs(15));
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(
            reopened.get("/read").unwrap().body,
            "missing-cookie",
            "logout kept persistent cookie"
        );
        reopened.shutdown();
        drop(reopened);
        let after_logout = BrowserSession::with_profile(
            origin.clone(),
            BrowserProfile::persistent(profile_path.clone()).unwrap(),
        )
        .unwrap();
        after_logout.prepare_restore().unwrap();
        let started = Instant::now();
        while after_logout.get("/read").is_err() {
            assert!(started.elapsed() < Duration::from_secs(15));
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(after_logout.get("/read").unwrap().body, "missing-cookie");
        after_logout.shutdown();
        drop(after_logout);
        for _ in 0..20 {
            if std::fs::remove_dir_all(&profile_path).is_ok() || !profile_path.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        stop.store(true, Ordering::Release);
        server.join().unwrap();
        assert_eq!(posts.load(Ordering::Acquire), 2, "browser replayed a POST");
    }
    #[test]
    #[ignore = "helper process for native_webview_session"]
    fn native_profile_child() {
        let (Some(profile), Some(origin)) = (
            std::env::var_os("WH_FIXTURE_PROFILE"),
            std::env::var_os("WH_FIXTURE_ORIGIN"),
        ) else {
            return;
        };
        let path = std::path::PathBuf::from(profile);
        assert!(path.starts_with(std::env::temp_dir()));
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("wh-webview-test-"));
        let origin = origin.into_string().unwrap();
        let parsed = reqwest::Url::parse(&origin).unwrap();
        assert_eq!(parsed.host_str(), Some("127.0.0.1"));
        assert_eq!(parsed.scheme(), "http");
        let session =
            BrowserSession::with_profile(origin, BrowserProfile::persistent(path).unwrap())
                .unwrap();
        session.prepare_restore().unwrap();
        let started = Instant::now();
        while !session
            .current_html()
            .is_ok_and(|html| html.contains("Fixture"))
        {
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "persistent cookie not restored in independent process"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(session.get("/read").unwrap().body, "get-cookie-ok");
        session.shutdown();
    }
}
