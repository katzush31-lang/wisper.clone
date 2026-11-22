//! Microphone capture with cpal on a dedicated thread (audio streams aren't Send on every
//! platform). Captured chunks go to whoever is listening (a recording or the mic test).
//! With "keep microphone ready" on, the stream stays open and the last ~0.5 s is kept, so a
//! recording starts with what was said just before the key press.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use serde::Serialize;

pub type Chunk = Vec<f32>;
pub type Sink = Box<dyn FnMut(Chunk) + Send>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Microphone {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub bluetooth: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Format {
    pub rate: u32,
    pub channels: usize,
}

struct Shared {
    sink: Mutex<Option<Sink>>,
    preroll: Mutex<VecDeque<f32>>,
    preroll_max: Mutex<usize>,
    error: Mutex<Option<String>>,
    /// The open stream hit a serious error (e.g. the mic was unplugged); reopen next time.
    broken: AtomicBool,
    /// The session currently using the microphone (0 = nobody). Set before the stream is
    /// opened, so a late stop or close from someone else can never close it.
    active: AtomicU64,
    /// Settings changed while the mic was in use: close the stream when that session ends.
    close_when_released: AtomicBool,
}

enum Cmd {
    Open { device: Option<String>, warm: bool, reply: Sender<Result<Format, String>> },
    /// A session finished. Closes the stream unless "keep microphone ready" is on, and only if
    /// no newer session has started in the meantime.
    Release,
    /// Close as soon as nobody is using the microphone (settings changed).
    CloseWhenFree,
    /// Close now, whoever is using it (app exiting).
    CloseNow,
}

#[derive(Clone)]
pub struct Capture {
    cmd: Sender<Cmd>,
    shared: Arc<Shared>,
    format: Arc<Mutex<Option<Format>>>,
    /// Hands out session ids.
    next_session: Arc<AtomicU64>,
}

impl Capture {
    pub fn spawn() -> Capture {
        let (tx, rx) = channel::<Cmd>();
        let shared = Arc::new(Shared {
            sink: Mutex::new(None),
            preroll: Mutex::new(VecDeque::new()),
            preroll_max: Mutex::new(0),
            error: Mutex::new(None),
            broken: AtomicBool::new(false),
            active: AtomicU64::new(0),
            close_when_released: AtomicBool::new(false),
        });
        let s2 = shared.clone();
        std::thread::Builder::new()
            .name("wisper-audio".into())
            .spawn(move || audio_thread(rx, s2))
            .expect("spawn audio thread");
        Capture { cmd: tx, shared, format: Arc::new(Mutex::new(None)), next_session: Arc::new(AtomicU64::new(1)) }
    }

    /// Opens the microphone (if not already open) and starts sending chunks to `sink`.
    /// Returns the stream format, any pre-roll audio captured before this call, and a
    /// session id for `stop`.
    pub fn start(&self, device: Option<String>, warm: bool, sink: Sink) -> Result<(Format, Chunk, u64), String> {
        // Claim the microphone before opening it, so a late stop from an earlier session
        // (queued behind this Open) is recognised as stale and ignored.
        let id = self.next_session.fetch_add(1, Ordering::SeqCst);
        self.shared.active.store(id, Ordering::SeqCst);
        let (rtx, rrx) = channel();
        let opened = self
            .cmd
            .send(Cmd::Open { device, warm, reply: rtx })
            .map_err(|e| e.to_string())
            .and_then(|_| {
                rrx.recv_timeout(Duration::from_secs(5))
                    .map_err(|_| "The microphone didn't respond. Check it's connected, or pick another one in Settings.".to_string())?
            });
        let format = match opened {
            Ok(f) => f,
            Err(e) => {
                let _ = self.shared.active.compare_exchange(id, 0, Ordering::SeqCst, Ordering::SeqCst);
                return Err(e);
            }
        };
        *self.format.lock() = Some(format);
        let pre: Chunk = self.shared.preroll.lock().drain(..).collect();
        *self.shared.error.lock() = None;
        let mut guard = self.shared.sink.lock();
        if self.shared.active.load(Ordering::SeqCst) == id {
            *guard = Some(sink);
        }
        drop(guard);
        Ok((format, pre, id))
    }

    /// Stops delivering audio to session `id`. Does nothing if a newer session has taken the
    /// microphone since. The stream stays open if "keep microphone ready" is on.
    pub fn stop(&self, id: u64) {
        let mut guard = self.shared.sink.lock();
        if self.shared.active.compare_exchange(id, 0, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return;
        }
        *guard = None;
        drop(guard);
        let _ = self.cmd.send(Cmd::Release);
    }

    /// Settings changed (microphone or "keep microphone ready"): close the stream now if it's
    /// free, otherwise as soon as the current recording ends. Never cuts a recording short.
    pub fn close_when_free(&self) {
        let _ = self.cmd.send(Cmd::CloseWhenFree);
    }

    /// Closes the stream immediately, even mid-recording (app exiting).
    #[allow(dead_code)]
    pub fn close_now(&self) {
        *self.shared.sink.lock() = None;
        self.shared.active.store(0, Ordering::SeqCst);
        let _ = self.cmd.send(Cmd::CloseNow);
    }

    pub fn take_error(&self) -> Option<String> {
        self.shared.error.lock().take()
    }
}

fn find_device(host: &cpal::Host, id: Option<&str>) -> Option<cpal::Device> {
    if let Some(id) = id {
        if let Ok(parsed) = id.parse::<cpal::DeviceId>() {
            if let Some(d) = host.device_by_id(&parsed) {
                return Some(d);
            }
        }
        tracing::warn!("chosen microphone not found; using the Windows default");
    }
    host.default_input_device()
}

fn audio_thread(rx: Receiver<Cmd>, shared: Arc<Shared>) {
    let host = cpal::default_host();
    let mut stream: Option<(cpal::Stream, Option<String>, Format)> = None;
    let mut warm = false;
    for cmd in rx {
        match cmd {
            Cmd::Open { device, warm: want_warm, reply } => {
                warm = want_warm;
                let broken = shared.broken.swap(false, Ordering::SeqCst);
                if let Some((_, open_dev, fmt)) = &stream {
                    if *open_dev == device && !broken {
                        let _ = reply.send(Ok(*fmt));
                        continue;
                    }
                    stream = None;
                }
                match open(&host, device.as_deref(), shared.clone()) {
                    Ok((s, fmt)) => {
                        *shared.preroll_max.lock() = if want_warm { (fmt.rate as usize) * fmt.channels / 2 } else { 0 };
                        stream = Some((s, device, fmt));
                        let _ = reply.send(Ok(fmt));
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e));
                    }
                }
            }
            Cmd::Release => {
                // A newer session already owns the microphone: leave it open.
                if shared.active.load(Ordering::SeqCst) != 0 {
                    continue;
                }
                if shared.close_when_released.swap(false, Ordering::SeqCst) || !warm {
                    stream = None;
                    shared.preroll.lock().clear();
                }
            }
            Cmd::CloseWhenFree => {
                if shared.active.load(Ordering::SeqCst) != 0 {
                    shared.close_when_released.store(true, Ordering::SeqCst);
                } else {
                    stream = None;
                    shared.preroll.lock().clear();
                }
            }
            Cmd::CloseNow => {
                stream = None;
                shared.preroll.lock().clear();
            }
        }
    }
}

fn open(host: &cpal::Host, device: Option<&str>, shared: Arc<Shared>) -> Result<(cpal::Stream, Format), String> {
    let dev = find_device(host, device).ok_or_else(|| "No microphone found. Plug one in or check Windows sound settings.".to_string())?;
    let config = dev
        .default_input_config()
        .map_err(|e| format!("The microphone can't be used ({e}). Check Windows privacy settings allow microphone access."))?;
    let fmt = Format { rate: config.sample_rate(), channels: config.channels() as usize };
    let err_shared = shared.clone();
    let on_error = move |e: cpal::Error| {
        tracing::warn!("microphone stream error: {e}");
        if !matches!(e.kind(), cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied) {
            err_shared.broken.store(true, Ordering::SeqCst);
        }
        *err_shared.error.lock() = Some(e.to_string());
    };
    macro_rules! build {
        ($t:ty, $conv:expr) => {{
            let sh = shared.clone();
            dev.build_input_stream(
                config.clone().into(),
                move |data: &[$t], _: &cpal::InputCallbackInfo| {
                    let chunk: Vec<f32> = data.iter().map($conv).collect();
                    deliver(&sh, chunk);
                },
                on_error,
                None,
            )
        }};
    }
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build!(f32, |&v: &f32| v),
        cpal::SampleFormat::I16 => build!(i16, |&v: &i16| v as f32 / 32768.0),
        cpal::SampleFormat::I32 => build!(i32, |&v: &i32| v as f32 / 2_147_483_648.0),
        cpal::SampleFormat::U16 => build!(u16, |&v: &u16| (v as f32 - 32768.0) / 32768.0),
        other => return Err(format!("This microphone uses an unsupported audio format ({other:?}).")),
    }
    .map_err(|e| format!("Couldn't open the microphone: {e}"))?;
    stream.play().map_err(|e| format!("Couldn't start the microphone: {e}"))?;
    Ok((stream, fmt))
}

fn deliver(shared: &Shared, chunk: Vec<f32>) {
    let mut sink = shared.sink.lock();
    if let Some(f) = sink.as_mut() {
        f(chunk);
        return;
    }
    drop(sink);
    let max = *shared.preroll_max.lock();
    if max > 0 {
        let mut pre = shared.preroll.lock();
        pre.extend(chunk);
        while pre.len() > max {
            pre.pop_front();
        }
    }
}

pub fn list_microphones() -> Vec<Microphone> {
    let host = cpal::default_host();
    let default_id = host.default_input_device().and_then(|d| d.id().ok()).map(|i| i.to_string());
    let mut out = Vec::new();
    if let Ok(devs) = host.input_devices() {
        for d in devs {
            let Ok(id) = d.id() else { continue };
            let id = id.to_string();
            let (name, bluetooth) = match d.description() {
                Ok(desc) => (desc.name().to_string(), desc.interface_type() == cpal::InterfaceType::Bluetooth),
                Err(_) => (id.clone(), false),
            };
            let lower = name.to_lowercase();
            let bluetooth = bluetooth || lower.contains("hands-free") || lower.contains("airpods") || lower.contains("bluetooth");
            out.push(Microphone { is_default: Some(&id) == default_id.as_ref(), id, name, bluetooth });
        }
    }
    out
}
