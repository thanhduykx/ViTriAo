//! Lõi giả lập vị trí cho ViTriAo.
//!
//! App chạy ngay trên iPhone và nói chuyện với chính iPhone đó qua một VPN loopback
//! (LocalDevVPN / StosVPN), dùng pairing file mà iloader tạo ra. Từ đó:
//!   - iOS 17+:  lockdown -> CoreDeviceProxy -> tunnel phần mềm -> RSD -> DVT
//!               -> com.apple.instruments.server.services.LocationSimulation
//!   - iOS < 17: lockdown -> com.apple.dt.simulatelocation (cần Developer Disk Image đã mount)
//!
//! Kết nối DVT phải được giữ mở, nếu đóng thì iOS sẽ huỷ vị trí ảo. Vì vậy mỗi phiên
//! chạy trong một task nền, nhận lệnh qua channel và gửi lại toạ độ định kỳ.

use std::{
    ffi::{CStr, CString, c_char},
    net::IpAddr,
    str::FromStr,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use idevice::{
    IdeviceService, RsdService,
    core_device_proxy::CoreDeviceProxy,
    dvt::{location_simulation::LocationSimulationClient, remote_server::RemoteServerClient},
    heartbeat::HeartbeatClient,
    pairing_file::PairingFile,
    provider::{IdeviceProvider, TcpProvider},
    rsd::RsdHandshake,
    services::simulate_location::LocationSimulationService,
};
use tokio::{
    runtime::Runtime,
    sync::{mpsc, oneshot},
};

const RESEND_INTERVAL: Duration = Duration::from_secs(4);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const LABEL: &str = "ViTriAo";

enum Cmd {
    Set(f64, f64, oneshot::Sender<Result<(), String>>),
    Clear(oneshot::Sender<Result<(), String>>),
}

struct Session {
    tx: mpsc::Sender<Cmd>,
    cancel: oneshot::Sender<()>,
    done: Arc<AtomicBool>,
}

impl Session {
    fn alive(&self) -> bool {
        !self.done.load(Ordering::SeqCst)
    }
    fn abort(self) {
        let _ = self.cancel.send(());
    }
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("không tạo được tokio runtime")
    })
}

// ---------------------------------------------------------------------------
// Rust API
// ---------------------------------------------------------------------------

/// Đặt vị trí ảo. Kết nối mới nếu chưa có phiên (hoặc phiên cũ đã chết).
pub fn set_location(pairing_path: &str, ip: &str, lat: f64, lon: f64) -> Result<(), String> {
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return Err(format!("Toạ độ không hợp lệ: {lat}, {lon}"));
    }

    let rt = runtime();
    let mut guard = SESSION.lock().map_err(|_| "lock bị hỏng".to_string())?;

    // Phiên đang chạy: chỉ cần gửi toạ độ mới.
    if let Some(session) = guard.as_ref().filter(|s| s.alive()) {
        let (reply_tx, reply_rx) = oneshot::channel();
        let tx = session.tx.clone();
        let res = rt.block_on(async move {
            tx.send(Cmd::Set(lat, lon, reply_tx))
                .await
                .map_err(|_| "phiên đã đóng".to_string())?;
            reply_rx.await.map_err(|_| "phiên đã đóng".to_string())?
        });
        if res.is_ok() {
            return Ok(());
        }
        // Phiên lỗi -> bỏ và kết nối lại ở dưới.
    }
    if let Some(old) = guard.take() {
        old.abort();
    }

    let addr = IpAddr::from_str(ip.trim()).map_err(|e| format!("IP không hợp lệ '{ip}': {e}"))?;
    let make_provider = || -> Result<TcpProvider, String> {
        let pairing_file = PairingFile::read_from_file(pairing_path)
            .map_err(|e| format!("Không đọc được pairing file: {e}"))?;
        Ok(TcpProvider {
            addr,
            scope_id: None,
            pairing_file,
            label: LABEL.to_string(),
        })
    };
    let provider = make_provider()?;
    let hb_provider = make_provider()?;

    let (tx, rx) = mpsc::channel(8);
    let (ready_tx, ready_rx) = oneshot::channel();

    let (cancel, cancel_rx) = oneshot::channel::<()>();
    let done = Arc::new(AtomicBool::new(false));
    let done_flag = done.clone();
    // Chạy trên thread riêng (block_on không yêu cầu future là Send).
    std::thread::Builder::new()
        .name("vitriao-session".into())
        .spawn(move || {
            runtime().block_on(async move {
                tokio::select! {
                    _ = session_main(provider, lat, lon, rx, ready_tx) => {}
                    _ = heartbeat_loop(hb_provider) => {}
                    _ = cancel_rx => {}
                }
            });
            done_flag.store(true, Ordering::SeqCst);
        })
        .map_err(|e| format!("Không tạo được thread: {e}"))?;

    let ready = rt.block_on(async {
        match tokio::time::timeout(CONNECT_TIMEOUT, ready_rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err("phiên kết thúc bất ngờ".to_string()),
            Err(_) => Err(
                "Hết thời gian kết nối. Kiểm tra VPN (LocalDevVPN/StosVPN) đã bật và IP đúng."
                    .to_string(),
            ),
        }
    });

    let session = Session { tx, cancel, done };
    match ready {
        Ok(()) => {
            *guard = Some(session);
            Ok(())
        }
        Err(e) => {
            session.abort();
            Err(e)
        }
    }
}

/// Tắt vị trí ảo, trả iPhone về GPS thật.
pub fn clear_location() -> Result<(), String> {
    let rt = runtime();
    let mut guard = SESSION.lock().map_err(|_| "lock bị hỏng".to_string())?;
    let Some(session) = guard.take() else {
        return Ok(());
    };
    let res = if session.alive() {
        let (reply_tx, reply_rx) = oneshot::channel();
        let tx = session.tx.clone();
        rt.block_on(async move {
            tx.send(Cmd::Clear(reply_tx))
                .await
                .map_err(|_| "phiên đã đóng".to_string())?;
            match tokio::time::timeout(Duration::from_secs(8), reply_rx).await {
                Ok(Ok(r)) => r,
                _ => Err("không nhận được phản hồi khi tắt".to_string()),
            }
        })
    } else {
        Ok(())
    };
    session.abort();
    res
}

pub fn is_active() -> bool {
    SESSION
        .lock()
        .map(|g| g.as_ref().is_some_and(|s| s.alive()))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Phiên làm việc
// ---------------------------------------------------------------------------

async fn heartbeat_loop(provider: TcpProvider) {
    // Giữ cho kết nối lockdown qua mạng không bị iOS ngắt. Lỗi ở đây không chặn phiên chính.
    loop {
        if let Ok(mut hb) = HeartbeatClient::connect(&provider).await {
            let mut interval = 15;
            loop {
                match hb.get_marco(interval).await {
                    Ok(i) => interval = i + 5,
                    Err(_) => break,
                }
                if hb.send_polo().await.is_err() {
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn session_main(
    provider: TcpProvider,
    lat: f64,
    lon: f64,
    rx: mpsc::Receiver<Cmd>,
    ready: oneshot::Sender<Result<(), String>>,
) {
    match CoreDeviceProxy::connect(&provider as &dyn IdeviceProvider).await {
        Ok(proxy) => run_dvt(proxy, lat, lon, rx, ready).await,
        Err(proxy_err) => run_legacy(&provider, lat, lon, rx, ready, proxy_err.to_string()).await,
    }
}

/// iOS 17 trở lên.
async fn run_dvt(
    proxy: CoreDeviceProxy,
    lat: f64,
    lon: f64,
    mut rx: mpsc::Receiver<Cmd>,
    ready: oneshot::Sender<Result<(), String>>,
) {
    let setup = async {
        let rsd_port = proxy.tunnel_info().server_rsd_port;
        let adapter = proxy
            .create_software_tunnel()
            .map_err(|e| format!("Không tạo được tunnel: {e}"))?;
        let mut adapter = adapter.to_async_handle();
        let stream = adapter
            .connect(rsd_port)
            .await
            .map_err(|e| format!("Không kết nối được RSD: {e}"))?;
        let mut handshake = RsdHandshake::new(stream)
            .await
            .map_err(|e| format!("RSD handshake lỗi: {e}"))?;
        let mut server = RemoteServerClient::connect_rsd(&mut adapter, &mut handshake)
            .await
            .map_err(|e| {
                format!("Không mở được DVT ({e}). Hãy bật Developer Mode trên iPhone.")
            })?;
        server
            .read_message(0)
            .await
            .map_err(|e| format!("DVT không phản hồi: {e}"))?;
        Ok::<_, String>((adapter, server))
    };

    let (_adapter, mut server) = match setup.await {
        Ok(v) => v,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };

    let mut client = match LocationSimulationClient::new(&mut server).await {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(format!("Không mở được LocationSimulation: {e}")));
            return;
        }
    };

    if let Err(e) = client.set(lat, lon).await {
        let _ = ready.send(Err(format!("Đặt vị trí thất bại: {e}")));
        return;
    }
    let _ = ready.send(Ok(()));

    let mut current = (lat, lon);
    let mut ticker = tokio::time::interval(RESEND_INTERVAL);
    ticker.tick().await;
    loop {
        tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(Cmd::Set(la, lo, reply)) => {
                    let r = client.set(la, lo).await.map_err(|e| e.to_string());
                    let failed = r.is_err();
                    if !failed { current = (la, lo); }
                    let _ = reply.send(r);
                    if failed { return; }
                }
                Some(Cmd::Clear(reply)) => {
                    let _ = reply.send(client.clear().await.map_err(|e| e.to_string()));
                    return;
                }
                None => {
                    let _ = client.clear().await;
                    return;
                }
            },
            _ = ticker.tick() => {
                if client.set(current.0, current.1).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// iOS 16 trở xuống (cần Developer Disk Image đã được mount).
async fn run_legacy(
    provider: &TcpProvider,
    lat: f64,
    lon: f64,
    mut rx: mpsc::Receiver<Cmd>,
    ready: oneshot::Sender<Result<(), String>>,
    proxy_err: String,
) {
    let mut client = match LocationSimulationService::connect(provider as &dyn IdeviceProvider).await
    {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(format!(
                "Không kết nối được thiết bị.\n• CoreDeviceProxy (iOS 17+): {proxy_err}\n• simulatelocation (iOS ≤16, cần mount DDI): {e}\nKiểm tra: VPN đã bật, pairing file đúng của máy này, Developer Mode đang bật."
            )));
            return;
        }
    };
    if let Err(e) = client.set(&lat.to_string(), &lon.to_string()).await {
        let _ = ready.send(Err(format!("Đặt vị trí thất bại: {e}")));
        return;
    }
    let _ = ready.send(Ok(()));

    while let Some(cmd) = rx.recv().await {
        match cmd {
            Cmd::Set(la, lo, reply) => {
                let r = client
                    .set(&la.to_string(), &lo.to_string())
                    .await
                    .map_err(|e| e.to_string());
                let failed = r.is_err();
                let _ = reply.send(r);
                if failed {
                    return;
                }
            }
            Cmd::Clear(reply) => {
                let _ = reply.send(client.clear().await.map_err(|e| e.to_string()));
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// C ABI cho Swift
// ---------------------------------------------------------------------------

fn write_err(err_out: *mut *mut c_char, msg: String) {
    if err_out.is_null() {
        return;
    }
    let c = CString::new(msg.replace('\0', " ")).unwrap_or_default();
    unsafe { *err_out = c.into_raw() };
}

unsafe fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        None
    } else {
        unsafe { CStr::from_ptr(p) }.to_str().ok()
    }
}

/// # Safety
/// `pairing_file_path` và `device_ip` phải là chuỗi C hợp lệ (UTF-8, kết thúc bằng NUL).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn locsim_set(
    pairing_file_path: *const c_char,
    device_ip: *const c_char,
    latitude: f64,
    longitude: f64,
    err_out: *mut *mut c_char,
) -> i32 {
    let (Some(path), Some(ip)) = (unsafe { cstr(pairing_file_path) }, unsafe { cstr(device_ip) })
    else {
        write_err(err_out, "Tham số rỗng hoặc không phải UTF-8".into());
        return -1;
    };
    match std::panic::catch_unwind(|| set_location(path, ip, latitude, longitude)) {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            write_err(err_out, e);
            -1
        }
        Err(_) => {
            write_err(err_out, "Lỗi nội bộ (panic)".into());
            -2
        }
    }
}

/// # Safety
/// `err_out` có thể NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn locsim_clear(err_out: *mut *mut c_char) -> i32 {
    match std::panic::catch_unwind(clear_location) {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            write_err(err_out, e);
            -1
        }
        Err(_) => {
            write_err(err_out, "Lỗi nội bộ (panic)".into());
            -2
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn locsim_is_active() -> i32 {
    is_active() as i32
}

/// # Safety
/// `s` phải là con trỏ do thư viện này trả về (hoặc NULL).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn locsim_free_string(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}
