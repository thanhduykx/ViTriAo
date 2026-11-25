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
    error::Error as StdError,
    ffi::{CStr, CString, c_char},
    io,
    net::{IpAddr, SocketAddr},
    str::FromStr,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use idevice::{
    IdeviceError, IdeviceService, RsdService,
    core_device_proxy::CoreDeviceProxy,
    dvt::{location_simulation::LocationSimulationClient, remote_server::RemoteServerClient},
    heartbeat::HeartbeatClient,
    lockdown::LockdownClient,
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
const CONNECT_TIMEOUT: Duration = Duration::from_secs(25);
const TCP_PROBE_TIMEOUT: Duration = Duration::from_secs(6);
/// iOS 17 bỏ simulatelocation cũ, chuyển sang CoreDeviceProxy + DVT.
const FIRST_CORE_DEVICE_IOS: u32 = 17;
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
    let ios_major = match preflight(&provider).await {
        Ok(v) => v,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };

    if ios_major < FIRST_CORE_DEVICE_IOS {
        return run_legacy(&provider, lat, lon, rx, ready).await;
    }
    match CoreDeviceProxy::connect(&provider as &dyn IdeviceProvider).await {
        Ok(proxy) => run_dvt(proxy, lat, lon, rx, ready).await,
        Err(e) => {
            let _ = ready.send(Err(format!(
                "[Bước 3/3] Mở CoreDeviceProxy thất bại: {}\n→ Pairing đã được chấp nhận nhưng iOS {ios_major} không mở được tunnel. Kiểm tra Developer Mode đang bật và iPhone đã mở khoá.",
                describe(&e)
            )));
        }
    }
}

/// Kiểm tra từng bước để báo đúng chỗ hỏng thay vì chỉ "socket io failed".
/// Trả về phiên bản iOS chính (vd. 17, 18, 26).
async fn preflight(provider: &TcpProvider) -> Result<u32, String> {
    let target = SocketAddr::new(provider.addr, LockdownClient::LOCKDOWND_PORT);

    // Bước 1: TCP thuần tới lockdownd qua VPN loopback.
    match tokio::time::timeout(TCP_PROBE_TIMEOUT, tokio::net::TcpStream::connect(target)).await {
        Err(_) => {
            return Err(format!(
                "[Bước 1/3] Không tới được {target} sau {}s.\n→ VPN chưa định tuyến: mở LocalDevVPN, chờ Connected rồi thử lại. Tắt các VPN/proxy khác.",
                TCP_PROBE_TIMEOUT.as_secs()
            ));
        }
        Ok(Err(e)) => {
            return Err(format!(
                "[Bước 1/3] Kết nối TCP tới {target} thất bại: {}\n→ {}",
                io_detail(&e),
                tcp_hint(&e, provider.addr)
            ));
        }
        Ok(Ok(stream)) => drop(stream),
    }

    // Bước 2: lockdownd chấp nhận pairing file (bắt tay TLS).
    let mut lockdown = LockdownClient::connect(provider as &dyn IdeviceProvider)
        .await
        .map_err(|e| format!("[Bước 2/3] Mở lockdownd thất bại: {}", describe(&e)))?;
    lockdown
        .start_session(&provider.pairing_file)
        .await
        .map_err(|e| {
            format!(
                "[Bước 2/3] lockdownd từ chối pairing file: {}\n→ {}",
                describe(&e),
                pairing_hint(&e)
            )
        })?;

    // Không đọc được phiên bản thì mặc định đi đường iOS 17+.
    let major = lockdown
        .get_value(Some("ProductVersion"), None)
        .await
        .ok()
        .and_then(|v| v.as_string().map(str::to_owned))
        .and_then(|v| v.split('.').next().and_then(|m| m.parse::<u32>().ok()))
        .unwrap_or(FIRST_CORE_DEVICE_IOS);
    Ok(major)
}

// ---------------------------------------------------------------------------
// Diễn giải lỗi
// ---------------------------------------------------------------------------

/// Nối toàn bộ chuỗi lỗi (idevice giấu lỗi gốc sau "device socket io failed").
fn describe<E: StdError + 'static>(e: &E) -> String {
    let mut out = e.to_string();
    let mut cur: Option<&(dyn StdError + 'static)> = e.source();
    while let Some(cause) = cur {
        out.push_str(": ");
        match cause.downcast_ref::<io::Error>() {
            Some(io_err) => out.push_str(&io_detail(io_err)),
            None => out.push_str(&cause.to_string()),
        }
        cur = cause.source();
    }
    out
}

fn io_detail(e: &io::Error) -> String {
    match e.raw_os_error() {
        Some(code) => format!("{e} [{:?}, errno {code}]", e.kind()),
        None => format!("{e} [{:?}]", e.kind()),
    }
}

/// errno trên Darwin.
mod errno {
    pub const EPERM: i32 = 1;
    pub const EACCES: i32 = 13;
    pub const ENETDOWN: i32 = 50;
    pub const ENETUNREACH: i32 = 51;
    pub const ETIMEDOUT: i32 = 60;
    pub const ECONNREFUSED: i32 = 61;
    pub const EHOSTUNREACH: i32 = 65;
}

fn tcp_hint(e: &io::Error, ip: IpAddr) -> String {
    const LOCAL_NETWORK: &str = "iOS có thể đang chặn quyền Mạng cục bộ của app: Cài đặt → Quyền riêng tư & Bảo mật → Mạng cục bộ → bật Vị Trí Ảo.";
    match e.raw_os_error() {
        Some(errno::EPERM | errno::EACCES) => LOCAL_NETWORK.to_string(),
        Some(errno::EHOSTUNREACH | errno::ENETUNREACH | errno::ENETDOWN) => format!(
            "Không có đường tới {ip}. VPN chưa Connected, hoặc {LOCAL_NETWORK}"
        ),
        Some(errno::ETIMEDOUT) => {
            "VPN chưa định tuyến: bật lại LocalDevVPN rồi thử lại.".to_string()
        }
        Some(errno::ECONNREFUSED) => format!(
            "Có đường tới {ip} nhưng cổng 62078 bị từ chối. IP trong Cài đặt có thể sai (LocalDevVPN dùng 10.7.0.1)."
        ),
        _ => "Kiểm tra VPN đã Connected và IP trong Cài đặt (mặc định 10.7.0.1).".to_string(),
    }
}

fn pairing_hint(e: &IdeviceError) -> &'static str {
    match e {
        IdeviceError::InvalidHostID => {
            "Pairing file không thuộc iPhone này, hoặc đã bị thu hồi (sau khi Đặt lại Vị trí & Quyền riêng tư). Tạo lại bằng iloader."
        }
        IdeviceError::Socket(_) => {
            "lockdownd ngắt kết nối khi bắt tay. Thường do pairing file sai máy hoặc đã hết hiệu lực: tạo lại bằng iloader."
        }
        _ => "Pairing file có thể hỏng hoặc không đúng máy: tạo lại bằng iloader.",
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
            .map_err(|e| format!("Không tạo được tunnel: {}", describe(&e)))?;
        let mut adapter = adapter.to_async_handle();
        let stream = adapter
            .connect(rsd_port)
            .await
            .map_err(|e| format!("Không kết nối được RSD: {}", describe(&e)))?;
        let mut handshake = RsdHandshake::new(stream)
            .await
            .map_err(|e| format!("RSD handshake lỗi: {}", describe(&e)))?;
        let mut server = RemoteServerClient::connect_rsd(&mut adapter, &mut handshake)
            .await
            .map_err(|e| {
                format!("Không mở được DVT ({}). Hãy bật Developer Mode trên iPhone.", describe(&e))
            })?;
        server
            .read_message(0)
            .await
            .map_err(|e| format!("DVT không phản hồi: {}", describe(&e)))?;
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
            let _ = ready.send(Err(format!("Không mở được LocationSimulation: {}", describe(&e))));
            return;
        }
    };

    if let Err(e) = client.set(lat, lon).await {
        let _ = ready.send(Err(format!("Đặt vị trí thất bại: {}", describe(&e))));
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
) {
    let mut client = match LocationSimulationService::connect(provider as &dyn IdeviceProvider).await
    {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(format!(
                "[iOS ≤16] Mở simulatelocation thất bại: {}\n→ iOS 16 trở xuống cần mount Developer Disk Image trước.",
                describe(&e)
            )));
            return;
        }
    };
    if let Err(e) = client.set(&lat.to_string(), &lon.to_string()).await {
        let _ = ready.send(Err(format!("Đặt vị trí thất bại: {}", describe(&e))));
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
