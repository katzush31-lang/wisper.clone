//! Other audio while dictating: pause playing media (via Windows media controls) or mute
//! apps that are currently making sound (via Core Audio), then restore exactly what we
//! changed. Muted sessions are remembered on disk so a crash can't leave apps silent.
//! Also plays Wisper's own start/stop sounds.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use windows::core::{Interface, GUID, PCWSTR};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager as MediaManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
    ISimpleAudioVolume, MMDeviceEnumerator, PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::GetCurrentProcessId;

use wisper_core::settings::OtherAudio;
use wisper_core::sounds::{self, Sound};

pub struct DuckToken {
    paused: Vec<String>,
    muted: Vec<String>,
}

fn com() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

fn pause_media() -> Vec<String> {
    let mut paused = Vec::new();
    let Ok(op) = MediaManager::RequestAsync() else { return paused };
    let Ok(mgr) = op.join() else { return paused };
    let Ok(sessions) = mgr.GetSessions() else { return paused };
    for s in sessions {
        let playing = s
            .GetPlaybackInfo()
            .and_then(|i| i.PlaybackStatus())
            .map(|st| st == PlaybackStatus::Playing)
            .unwrap_or(false);
        if playing {
            if let Ok(op) = s.TryPauseAsync() {
                if op.join().unwrap_or(false) {
                    paused.push(s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default());
                }
            }
        }
    }
    paused
}

fn resume_media(ids: &[String]) {
    if ids.is_empty() {
        return;
    }
    let Ok(op) = MediaManager::RequestAsync() else { return };
    let Ok(mgr) = op.join() else { return };
    let Ok(sessions) = mgr.GetSessions() else { return };
    for s in sessions {
        let id = s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default();
        let paused = s
            .GetPlaybackInfo()
            .and_then(|i| i.PlaybackStatus())
            .map(|st| st == PlaybackStatus::Paused)
            .unwrap_or(false);
        if ids.contains(&id) && paused {
            if let Ok(op) = s.TryPlayAsync() {
                let _ = op.join();
            }
        }
    }
}

unsafe fn sessions() -> Vec<(String, u32, bool, ISimpleAudioVolume)> {
    let mut out = Vec::new();
    unsafe {
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return out };
        let Ok(dev) = en.GetDefaultAudioEndpoint(eRender, eMultimedia) else { return out };
        let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { return out };
        let Ok(list) = mgr.GetSessionEnumerator() else { return out };
        let n = list.GetCount().unwrap_or(0);
        for i in 0..n {
            let Ok(ctl) = list.GetSession(i) else { continue };
            let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() else { continue };
            let Ok(vol) = ctl.cast::<ISimpleAudioVolume>() else { continue };
            let pid = ctl2.GetProcessId().unwrap_or(0);
            let active = ctl2.GetState().map(|s| s == AudioSessionStateActive).unwrap_or(false);
            let id = match ctl2.GetSessionInstanceIdentifier() {
                Ok(p) => {
                    let s = PCWSTR(p.0).to_string().unwrap_or_default();
                    CoTaskMemFree(Some(p.0 as _));
                    s
                }
                Err(_) => continue,
            };
            out.push((id, pid, active, vol));
        }
    }
    out
}

fn mute_active() -> Vec<String> {
    let me = unsafe { GetCurrentProcessId() };
    let mut muted = Vec::new();
    unsafe {
        for (id, pid, active, vol) in sessions() {
            if pid == me || pid == 0 || !active {
                continue;
            }
            if vol.GetMute().map(|b| b.as_bool()).unwrap_or(true) {
                continue; // already muted by the user: leave it
            }
            if vol.SetMute(true, &GUID::zeroed()).is_ok() {
                muted.push(id);
            }
        }
    }
    muted
}

fn unmute(ids: &[String]) {
    if ids.is_empty() {
        return;
    }
    unsafe {
        for (id, _, _, vol) in sessions() {
            if ids.contains(&id) {
                let _ = vol.SetMute(false, &GUID::zeroed());
            }
        }
    }
}

pub fn duck_other_audio(mode: OtherAudio, state_file: &Path) -> DuckToken {
    com();
    let token = match mode {
        OtherAudio::Off => DuckToken { paused: vec![], muted: vec![] },
        OtherAudio::Pause => DuckToken { paused: pause_media(), muted: vec![] },
        OtherAudio::Mute => DuckToken { paused: vec![], muted: mute_active() },
    };
    if !token.muted.is_empty() {
        let _ = std::fs::write(state_file, token.muted.join("\n"));
    }
    token
}

pub fn restore_other_audio(t: DuckToken, state_file: &Path) {
    com();
    resume_media(&t.paused);
    unmute(&t.muted);
    let _ = std::fs::remove_file(state_file);
}

/// Unmutes anything a previous run muted and never restored (crash, power loss).
pub fn restore_after_crash(state_file: &Path) {
    if let Ok(text) = std::fs::read_to_string(state_file) {
        com();
        let ids: Vec<String> = text.lines().map(String::from).filter(|l| !l.is_empty()).collect();
        unmute(&ids);
        let _ = std::fs::remove_file(state_file);
    }
}

/// Sounds are cached per (sound, volume%) and never freed: PlaySound reads the buffer
/// asynchronously, so it must outlive playback.
static SOUND_CACHE: OnceLock<Mutex<HashMap<(Sound, u8), &'static [u8]>>> = OnceLock::new();

pub fn play_sound(s: Sound, volume: f32) {
    let pct = (volume.clamp(0.0, 1.0) * 100.0).round() as u8;
    if pct == 0 {
        return;
    }
    let cache = SOUND_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let bytes: &'static [u8] = {
        let mut c = cache.lock().unwrap_or_else(|p| p.into_inner());
        c.entry((s, pct)).or_insert_with(|| Box::leak(sounds::wav(s, pct as f32 / 100.0).into_boxed_slice()))
    };
    unsafe {
        let _ = PlaySoundW(PCWSTR(bytes.as_ptr() as *const u16), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}
