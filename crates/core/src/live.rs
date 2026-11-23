//! Live transcription: streams the recording to OpenAI's Realtime API while the user talks, so
//! the text is ready almost as soon as they stop.
//!
//! It is purely an accelerator layered on top of the normal pipeline:
//! - the recording is still written to disk first and cut into pieces by the same segmenter;
//! - each piece's audio is streamed (24 kHz) and committed at exactly the piece boundaries the
//!   segmenter chose, so live item N is piece N;
//! - a piece's job waits for its live transcript; if the connection fails, drops, reports an
//!   error for that piece, or the result is late, the job transcribes that piece the normal way
//!   (file endpoint, from the saved audio). A late live result after that is ignored, so text is
//!   never inserted twice.
//!
//! Pieces are streamed only up to the segmenter's "safe point" (the earliest place the open
//! piece could still be cut), so audio never crosses a boundary decided later.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::audio::{self, resample::Resampler, segment::SegmentPlan};
use crate::providers::realtime::{self, ServerEvent, SessionConfig, Socket};
use crate::providers::{ErrorKind, ProviderError};

/// Where to connect and how patient to be.
#[derive(Debug, Clone)]
pub struct LiveEndpoint {
    pub url: String,
    pub tuning: LiveTuning,
    /// Turns live transcription off for a while after it couldn't connect or stalled.
    pub breaker: Arc<Breaker>,
}

impl LiveEndpoint {
    /// OpenAI, with the app-wide breaker.
    pub fn openai() -> Self {
        static BREAKER: std::sync::OnceLock<Arc<Breaker>> = std::sync::OnceLock::new();
        Self { url: realtime::URL.into(), tuning: LiveTuning::default(), breaker: BREAKER.get_or_init(Default::default).clone() }
    }

    /// Any URL, with its own breaker (tests).
    pub fn new(url: &str, tuning: LiveTuning) -> Self {
        Self { url: url.into(), tuning, breaker: Arc::default() }
    }
}

/// Circuit breaker for bad networks: after live transcription failed to connect (or its
/// session stalled), it's skipped for a while and dictations use the normal path directly,
/// instead of paying for another doomed attempt each time. In memory only.
#[derive(Debug, Default)]
pub struct Breaker {
    until: Mutex<Option<std::time::Instant>>,
}

impl Breaker {
    pub fn trip(&self, cooldown: Duration, why: &str) {
        tracing::info!("live transcription paused for {} min: {why}", cooldown.as_secs() / 60);
        *self.until.lock().unwrap_or_else(|p| p.into_inner()) = Some(std::time::Instant::now() + cooldown);
    }

    /// True while live transcription should be skipped.
    pub fn tripped(&self) -> bool {
        self.until.lock().unwrap_or_else(|p| p.into_inner()).is_some_and(|u| std::time::Instant::now() < u)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LiveTuning {
    /// Connecting plus session set-up. Audio is buffered meanwhile.
    pub connect_timeout: Duration,
    /// After the user stops: how long to wait for live results before transcribing the
    /// missing pieces the normal way. Plus a little per piece still waiting and per second of
    /// audio still waiting (a long last piece takes a moment longer to transcribe).
    pub stop_grace: Duration,
    pub per_piece_grace: Duration,
    pub per_audio_second: Duration,
    /// While still recording: a piece whose result hasn't come this long after it was sent
    /// is transcribed the normal way.
    pub piece_timeout: Duration,
    /// Reconnects after the connection drops mid-recording (later pieces go live again).
    pub max_reconnects: u32,
    /// Start a fresh connection (at a piece boundary) once one is this old. Realtime sessions
    /// have a maximum length; this stays well under it.
    pub rotate_after: Duration,
    /// Most audio (24 kHz samples) held while the connection is slow or still opening.
    pub max_backlog_samples: usize,
    /// A session with pieces waiting that hasn't said anything for this long has stalled.
    pub stall_timeout: Duration,
    /// How long live transcription is skipped after a failed connect or a stall.
    pub breaker_cooldown: Duration,
}

impl Default for LiveTuning {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(6),
            stop_grace: Duration::from_millis(2_500),
            per_piece_grace: Duration::from_millis(250),
            per_audio_second: Duration::from_millis(40),
            piece_timeout: Duration::from_secs(20),
            max_reconnects: 2,
            rotate_after: Duration::from_secs(25 * 60),
            max_backlog_samples: 60 * realtime::SAMPLE_RATE as usize,
            stall_timeout: Duration::from_secs(10),
            breaker_cooldown: Duration::from_secs(10 * 60),
        }
    }
}

/// Everything needed to open live sessions for one dictation.
#[derive(Clone)]
pub struct LiveSetup {
    pub endpoint: LiveEndpoint,
    pub key: String,
    pub session: SessionConfig,
}

impl std::fmt::Debug for LiveSetup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveSetup")
            .field("endpoint", &self.endpoint)
            .field("key", &if self.key.is_empty() { "(none)" } else { "(redacted)" })
            .field("session", &self.session)
            .finish()
    }
}

/// For History: whether live transcription was used and how it went.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSummary {
    /// A live connection was opened for this dictation.
    pub connected: bool,
    /// Pieces whose text came from the live connection.
    pub live_pieces: usize,
    /// Pieces transcribed the normal way instead.
    pub fallback_pieces: usize,
    /// Live results that arrived after their piece had already been transcribed another way.
    pub late_ignored: usize,
}

// ---------------------------------------------------------------------------- shared state

#[derive(Debug, Clone)]
enum Slot {
    Waiting { sent_at: Instant, audio_ms: u64 },
    Ready(Result<String, String>),
    /// Its job has taken a result (live or not); anything later is ignored.
    Settled,
}

#[derive(Default)]
struct State {
    slots: HashMap<usize, Slot>,
    stop_deadline: Option<Instant>,
    stopped_at: Option<Instant>,
    summary: LiveSummary,
}

/// Results of the live connection, shared between the connection task and the piece jobs.
pub struct LiveShared {
    state: Mutex<State>,
    tx: watch::Sender<u64>,
    tuning: LiveTuning,
}

impl LiveShared {
    fn new(tuning: LiveTuning) -> Arc<Self> {
        Arc::new(Self { state: Mutex::new(State::default()), tx: watch::channel(0).0, tuning })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn bump(&self) {
        self.tx.send_modify(|v| *v = v.wrapping_add(1));
    }

    fn register(&self, index: usize, audio_ms: u64) {
        self.lock().slots.insert(index, Slot::Waiting { sent_at: Instant::now(), audio_ms });
        self.bump();
    }

    /// Records the live outcome of a piece. Ignored (returns false) once its job has moved on.
    fn resolve(&self, index: usize, r: Result<String, String>) -> bool {
        let applied = {
            let mut st = self.lock();
            match st.slots.get_mut(&index) {
                Some(s @ Slot::Waiting { .. }) => {
                    *s = Slot::Ready(r);
                    true
                }
                Some(Slot::Settled) => {
                    if r.is_ok() {
                        st.summary.late_ignored += 1;
                        tracing::info!("live result for piece {index} arrived after it was transcribed another way; ignored");
                    }
                    false
                }
                _ => false,
            }
        };
        if applied {
            self.bump();
        }
        applied
    }

    fn mark_connected(&self) {
        self.lock().summary.connected = true;
    }

    /// The user stopped: from now on, waiting is bounded by a short deadline.
    fn stop(&self) {
        {
            let mut st = self.lock();
            if st.stopped_at.is_some() {
                return;
            }
            let now = Instant::now();
            let (n, audio_ms) = st.slots.values().fold((0u32, 0u64), |(n, a), s| match s {
                Slot::Waiting { audio_ms, .. } => (n + 1, a + audio_ms),
                _ => (n, a),
            });
            st.stopped_at = Some(now);
            // Never connected: nothing is coming, so don't make the user wait for it.
            let allowance = if st.summary.connected { stop_allowance(&self.tuning, n, audio_ms) } else { Duration::ZERO };
            st.stop_deadline = Some(now + allowance);
        }
        self.bump();
    }

    fn abandon_all(&self) {
        {
            let mut st = self.lock();
            for s in st.slots.values_mut() {
                *s = Slot::Settled;
            }
            st.stopped_at.get_or_insert_with(Instant::now);
        }
        self.bump();
    }

    fn waiting(&self) -> usize {
        self.lock().slots.values().filter(|s| matches!(s, Slot::Waiting { .. })).count()
    }

    fn stopped_for(&self) -> Option<Duration> {
        self.lock().stopped_at.map(|t| t.elapsed())
    }

    pub fn summary(&self) -> LiveSummary {
        self.lock().summary.clone()
    }

    /// Waits for piece `index`'s live transcript. None means "transcribe it the normal way"
    /// (failed, connection lost, not sent live, or too late); a live result arriving after
    /// that is ignored.
    pub async fn wait(&self, index: usize) -> Option<String> {
        loop {
            // Subscribe before looking, so a change between the look and the wait isn't missed.
            let mut rx = self.tx.subscribe();
            let deadline = {
                let mut st = self.lock();
                let now = Instant::now();
                match st.slots.get(&index).cloned() {
                    Some(Slot::Ready(Ok(t))) if !t.trim().is_empty() => {
                        st.slots.insert(index, Slot::Settled);
                        st.summary.live_pieces += 1;
                        return Some(t);
                    }
                    Some(Slot::Ready(Ok(_))) => {
                        // The piece has speech, so an empty transcript is suspect: get a second
                        // opinion from the normal path rather than saving nothing.
                        st.slots.insert(index, Slot::Settled);
                        st.summary.fallback_pieces += 1;
                        tracing::info!("live transcript of piece {index} was empty; transcribing it from the saved audio");
                        return None;
                    }
                    Some(Slot::Ready(Err(e))) => {
                        st.slots.insert(index, Slot::Settled);
                        st.summary.fallback_pieces += 1;
                        tracing::info!("live transcription of piece {index} failed ({e}); transcribing it from the saved audio");
                        return None;
                    }
                    Some(Slot::Waiting { sent_at, .. }) => {
                        let mut d = sent_at + self.tuning.piece_timeout;
                        if let Some(sd) = st.stop_deadline {
                            d = d.min(sd);
                        }
                        if now >= d {
                            st.slots.insert(index, Slot::Settled);
                            st.summary.fallback_pieces += 1;
                            tracing::info!("live result for piece {index} is late; transcribing it from the saved audio");
                            return None;
                        }
                        d
                    }
                    Some(Slot::Settled) | None => {
                        st.summary.fallback_pieces += 1;
                        return None;
                    }
                }
            };
            tokio::select! {
                _ = rx.changed() => {}
                _ = tokio::time::sleep_until(deadline) => {}
            }
        }
    }
}

/// What History records about live transcription for a finished dictation (merged into its
/// `timings`; `transcribeWaitMs` there is the wait after the user stopped).
pub fn timings_json(live: Option<&LiveShared>) -> serde_json::Value {
    match live {
        None => serde_json::json!({ "live": false, "liveTried": false }),
        Some(l) => {
            let s = l.summary();
            serde_json::json!({
                "live": s.live_pieces > 0,
                "liveTried": true,
                "liveConnected": s.connected,
                "livePieces": s.live_pieces,
                "fallbackPieces": s.fallback_pieces,
                "liveLateIgnored": s.late_ignored,
            })
        }
    }
}

/// How long to wait for live results after the user stops.
pub fn stop_allowance(t: &LiveTuning, pieces_waiting: u32, audio_ms_waiting: u64) -> Duration {
    t.stop_grace + t.per_piece_grace * pieces_waiting + t.per_audio_second.mul_f64(audio_ms_waiting as f64 / 1000.0)
}

// ---------------------------------------------------------------------------- the feed

enum Cmd {
    Audio(Vec<i16>),
    Commit(usize),
    Clear(usize),
    Stop,
}

/// Lives in the recording `Session` (audio thread). Never blocks: everything goes through a
/// channel to the connection task.
pub struct LiveFeed {
    shared: Arc<LiveShared>,
    tx: mpsc::UnboundedSender<Cmd>,
    resampler: Resampler,
    /// 24 kHz audio not yet handed to the connection; `buf[0]` is sample number `released`.
    buf: Vec<i16>,
    released: usize,
    backlog: Arc<AtomicUsize>,
    max_backlog: usize,
    gave_up: bool,
}

impl LiveFeed {
    /// Starts connecting in the background straight away; audio is buffered until ready.
    pub fn start(setup: LiveSetup, device_rate: u32, rt: &tokio::runtime::Handle) -> LiveFeed {
        let tuning = setup.endpoint.tuning;
        let shared = LiveShared::new(tuning);
        let (tx, rx) = mpsc::unbounded_channel();
        let backlog = Arc::new(AtomicUsize::new(0));
        let worker = Worker::new(Arc::new(setup), shared.clone(), backlog.clone());
        rt.spawn(worker.run(rx));
        LiveFeed {
            shared,
            tx,
            resampler: Resampler::new(device_rate, realtime::SAMPLE_RATE),
            buf: Vec::new(),
            released: 0,
            backlog,
            max_backlog: tuning.max_backlog_samples,
            gave_up: false,
        }
    }

    pub fn shared(&self) -> Arc<LiveShared> {
        self.shared.clone()
    }

    /// Mono audio at the device rate (the same audio the recording file gets).
    pub fn audio(&mut self, mono: &[f32]) {
        if self.gave_up {
            return;
        }
        let out = self.resampler.process(mono);
        self.buf.extend(out.iter().map(|&x| audio::f32_to_i16(x)));
    }

    /// Streams audio up to `end16` (a sample position in the 16 kHz recording).
    pub fn release_upto(&mut self, end16: usize) {
        if self.gave_up {
            return;
        }
        let target = to_24k(end16);
        let n = target.saturating_sub(self.released).min(self.buf.len());
        if n == 0 {
            return;
        }
        let chunk: Vec<i16> = self.buf.drain(..n).collect();
        self.released += n;
        let queued = self.backlog.fetch_add(chunk.len(), Ordering::SeqCst) + chunk.len();
        if queued > self.max_backlog {
            tracing::warn!("live transcription is falling behind; finishing this dictation the normal way");
            self.give_up();
            return;
        }
        self.send(Cmd::Audio(chunk));
    }

    /// A piece closed: send the rest of its audio and commit it (or drop it if silent).
    pub fn segment(&mut self, plan: &SegmentPlan) {
        self.release_upto(plan.end);
        if self.gave_up {
            return;
        }
        if plan.has_speech {
            self.shared.register(plan.index, audio::samples_to_ms(plan.end.saturating_sub(plan.start)));
            self.send(Cmd::Commit(plan.index));
        } else {
            self.send(Cmd::Clear(plan.index));
        }
    }

    /// The recording ended (after its last piece was committed).
    pub fn finish(self) -> Arc<LiveShared> {
        self.shared.stop();
        let _ = self.tx.send(Cmd::Stop);
        self.shared
    }

    pub fn cancel(self) {
        self.shared.abandon_all();
        let _ = self.tx.send(Cmd::Stop);
    }

    fn send(&mut self, cmd: Cmd) {
        if self.tx.send(cmd).is_err() {
            self.give_up();
        }
    }

    fn give_up(&mut self) {
        self.gave_up = true;
        self.buf = Vec::new();
    }
}

fn to_24k(samples16: usize) -> usize {
    (samples16 as u64 * realtime::SAMPLE_RATE as u64 / audio::SAMPLE_RATE as u64) as usize
}

// ---------------------------------------------------------------------------- connection task

type Sink = SplitSink<Socket, Message>;

enum Out {
    Audio(Vec<i16>),
    Commit(usize),
    Clear(usize),
}

enum ConnState {
    Opening,
    Ready(Box<Sink>),
}

struct Conn {
    state: ConnState,
    queued: VecDeque<Out>,
    queued_samples: usize,
    opened_at: Instant,
    /// Commits sent, waiting for `input_audio_buffer.committed` (they arrive in order).
    fifo: VecDeque<usize>,
    items: HashMap<String, usize>,
    /// Results that arrived before their `committed` event (shouldn't happen; kept to be safe).
    early: HashMap<String, Result<String, String>>,
    /// Pieces sent on this connection still without a result.
    outstanding: HashSet<usize>,
    last_item: Option<String>,
    /// When the server last said anything (or a wait for it began), to spot a stalled session.
    last_heard: Instant,
    draining: bool,
    reader: JoinHandle<()>,
}

enum Event {
    Opened(Box<Sink>),
    OpenFailed(ProviderError),
    Server(ServerEvent),
    Closed(String),
}

struct Worker {
    setup: Arc<LiveSetup>,
    shared: Arc<LiveShared>,
    backlog: Arc<AtomicUsize>,
    conns: HashMap<u32, Conn>,
    /// Newest connection new pieces go to.
    active: Option<u32>,
    next_gen: u32,
    /// Connection the open piece streams on; None = the open piece isn't live.
    current: Option<u32>,
    /// The open piece was streaming on this connection when something went wrong there; its
    /// partial audio is cleared from the server's buffer at the piece's end.
    taint: Option<u32>,
    bound: bool,
    ever_connected: bool,
    reconnects_left: u32,
    fatal: bool,
    stopping: bool,
    evt_tx: mpsc::UnboundedSender<(u32, Event)>,
    evt_rx: mpsc::UnboundedReceiver<(u32, Event)>,
}

impl Worker {
    fn new(setup: Arc<LiveSetup>, shared: Arc<LiveShared>, backlog: Arc<AtomicUsize>) -> Worker {
        let (evt_tx, evt_rx) = mpsc::unbounded_channel();
        let reconnects_left = setup.endpoint.tuning.max_reconnects;
        Worker {
            setup,
            shared,
            backlog,
            conns: HashMap::new(),
            active: None,
            next_gen: 0,
            current: None,
            taint: None,
            bound: false,
            ever_connected: false,
            reconnects_left,
            fatal: false,
            stopping: false,
            evt_tx,
            evt_rx,
        }
    }

    fn tuning(&self) -> LiveTuning {
        self.setup.endpoint.tuning
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Cmd>) {
        let first = self.open_conn();
        self.active = Some(first);
        let mut cmds_open = true;
        loop {
            tokio::select! {
                cmd = rx.recv(), if cmds_open => match cmd {
                    Some(c) => self.on_cmd(c).await,
                    None => {
                        cmds_open = false;
                        self.stopping = true;
                    }
                },
                Some((gen, ev)) = self.evt_rx.recv() => self.on_event(gen, ev).await,
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
            self.check_stalls().await;
            if self.stopping {
                let waited = self.shared.stopped_for().unwrap_or_default();
                if self.shared.waiting() == 0 || waited > Duration::from_secs(60) {
                    break;
                }
            }
        }
        for (_, c) in self.conns.drain() {
            close(c).await;
        }
    }

    fn open_conn(&mut self) -> u32 {
        let gen = self.next_gen;
        self.next_gen += 1;
        let setup = self.setup.clone();
        let tx = self.evt_tx.clone();
        let reader = tokio::spawn(async move {
            let t = setup.endpoint.tuning;
            match realtime::open_session(&setup.endpoint.url, &setup.key, &setup.session, t.connect_timeout).await {
                Ok(ws) => {
                    let (sink, mut stream) = ws.split();
                    if tx.send((gen, Event::Opened(Box::new(sink)))).is_err() {
                        return;
                    }
                    let reason = loop {
                        match stream.next().await {
                            Some(Ok(Message::Text(t))) => {
                                if tx.send((gen, Event::Server(realtime::parse_event(&t)))).is_err() {
                                    return;
                                }
                            }
                            Some(Ok(Message::Close(f))) => break format!("closed by the server: {f:?}"),
                            Some(Ok(_)) => {}
                            Some(Err(e)) => break e.to_string(),
                            None => break "connection closed".to_string(),
                        }
                    };
                    let _ = tx.send((gen, Event::Closed(reason)));
                }
                Err(e) => {
                    // Tripped here, not in the worker, so a connect that is still trying when the
                    // dictation ends is still noticed.
                    setup.endpoint.breaker.trip(t.breaker_cooldown, &format!("couldn't connect ({e})"));
                    let _ = tx.send((gen, Event::OpenFailed(e)));
                }
            }
        });
        self.conns.insert(
            gen,
            Conn {
                state: ConnState::Opening,
                queued: VecDeque::new(),
                queued_samples: 0,
                opened_at: Instant::now(),
                fifo: VecDeque::new(),
                items: HashMap::new(),
                early: HashMap::new(),
                outstanding: HashSet::new(),
                last_item: None,
                last_heard: Instant::now(),
                draining: false,
                reader,
            },
        );
        gen
    }

    /// Decides which connection the piece that is starting now streams on.
    fn bind(&mut self) {
        self.bound = true;
        let tuning = self.tuning();
        let healthy = self.active.filter(|g| self.conns.contains_key(g));
        self.current = match healthy {
            Some(g) => {
                let c = self.conns.get_mut(&g).expect("checked");
                if matches!(c.state, ConnState::Ready(_)) && c.opened_at.elapsed() >= tuning.rotate_after && !self.stopping {
                    tracing::info!("live transcription: starting a fresh session (the current one is {} min old)", c.opened_at.elapsed().as_secs() / 60);
                    c.draining = true;
                    let n = self.open_conn();
                    self.active = Some(n);
                    Some(n)
                } else {
                    Some(g)
                }
            }
            None if self.ever_connected && !self.fatal && !self.stopping && self.reconnects_left > 0 && !self.setup.endpoint.breaker.tripped() => {
                self.reconnects_left -= 1;
                tracing::info!("live transcription: reconnecting for the next piece");
                let n = self.open_conn();
                self.active = Some(n);
                Some(n)
            }
            None => None,
        };
    }

    async fn on_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Audio(samples) => {
                self.backlog.fetch_sub(samples.len(), Ordering::SeqCst);
                if !self.bound {
                    self.bind();
                }
                if let Some(g) = self.current {
                    self.out(g, Out::Audio(samples)).await;
                }
            }
            Cmd::Commit(index) => {
                if !self.bound {
                    self.bind();
                }
                match self.current {
                    Some(g) => self.out(g, Out::Commit(index)).await,
                    None => {
                        self.shared.resolve(index, Err("not streamed live".into()));
                        self.clear_taint(index).await;
                    }
                }
                self.end_piece().await;
            }
            Cmd::Clear(index) => {
                match self.current.filter(|_| self.bound) {
                    Some(g) => self.out(g, Out::Clear(index)).await,
                    None => self.clear_taint(index).await,
                }
                self.end_piece().await;
            }
            Cmd::Stop => self.stopping = true,
        }
    }

    /// Drops a broken piece's partial audio from the server's buffer, so it can't end up in the
    /// next piece.
    async fn clear_taint(&mut self, index: usize) {
        if let Some(g) = self.taint.take() {
            if self.conns.contains_key(&g) {
                self.out(g, Out::Clear(index)).await;
            }
        }
    }

    async fn end_piece(&mut self) {
        self.bound = false;
        self.taint = None;
        let draining: Vec<u32> = self.conns.iter().filter(|(_, c)| c.draining).map(|(g, _)| *g).collect();
        for g in draining {
            self.close_if_drained(g).await;
        }
    }

    /// Sends (or queues, while the connection is opening) one message on connection `g`.
    async fn out(&mut self, g: u32, o: Out) {
        let max = self.tuning().max_backlog_samples;
        let Some(c) = self.conns.get_mut(&g) else {
            if let Out::Commit(i) = o {
                self.shared.resolve(i, Err("connection lost".into()));
            }
            return;
        };
        match &mut c.state {
            ConnState::Opening => {
                if let Out::Audio(a) = &o {
                    c.queued_samples += a.len();
                }
                c.queued.push_back(o);
                if c.queued_samples > max {
                    self.fail_conn(g, "connection too slow to open").await;
                }
            }
            ConnState::Ready(sink) => {
                if let Out::Commit(i) = o {
                    if c.outstanding.is_empty() {
                        c.last_heard = Instant::now();
                    }
                    c.fifo.push_back(i);
                    c.outstanding.insert(i);
                }
                if let Err(e) = sink.send(Message::Text(message(&o).into())).await {
                    self.fail_conn(g, &format!("send failed: {e}")).await;
                }
            }
        }
    }

    async fn on_event(&mut self, g: u32, ev: Event) {
        match ev {
            Event::Opened(sink) => {
                self.ever_connected = true;
                self.shared.mark_connected();
                let Some(c) = self.conns.get_mut(&g) else { return };
                let queued = std::mem::take(&mut c.queued);
                c.queued_samples = 0;
                c.opened_at = Instant::now();
                c.state = ConnState::Ready(sink);
                // Send what piled up while connecting, joining audio into larger messages.
                let mut pending: Vec<i16> = Vec::new();
                for o in queued {
                    match o {
                        Out::Audio(a) => {
                            pending.extend_from_slice(&a);
                            if pending.len() >= realtime::SAMPLE_RATE as usize {
                                self.out(g, Out::Audio(std::mem::take(&mut pending))).await;
                            }
                        }
                        other => {
                            if !pending.is_empty() {
                                self.out(g, Out::Audio(std::mem::take(&mut pending))).await;
                            }
                            self.out(g, other).await;
                        }
                    }
                }
                if !pending.is_empty() {
                    self.out(g, Out::Audio(pending)).await;
                }
            }
            Event::OpenFailed(e) => {
                tracing::info!("live transcription unavailable ({e}); using normal transcription");
                if matches!(e.kind, ErrorKind::Auth | ErrorKind::Quota | ErrorKind::BadRequest) {
                    self.fatal = true;
                }
                self.fail_conn(g, &e.message).await;
            }
            Event::Closed(reason) => {
                tracing::info!("live transcription connection closed: {reason}");
                self.fail_conn(g, &reason).await;
            }
            Event::Server(se) => self.on_server(g, se).await,
        }
    }

    async fn on_server(&mut self, g: u32, se: ServerEvent) {
        let Some(c) = self.conns.get_mut(&g) else { return };
        c.last_heard = Instant::now();
        match se {
            ServerEvent::Committed { item_id, previous_item_id } => {
                let Some(index) = c.fifo.pop_front() else {
                    // Pieces and items no longer line up; nothing on this connection can be
                    // trusted to be the right piece.
                    return self.fail_conn(g, &format!("unexpected commit {item_id}")).await;
                };
                // Only a real conflict counts: if the server leaves previous_item_id empty we
                // rely on commits arriving in order (they do), rather than dropping live.
                if c.last_item.is_some() && previous_item_id.is_some() && previous_item_id != c.last_item {
                    let why = format!("item {item_id} follows {previous_item_id:?}, expected {:?}", c.last_item);
                    c.fifo.push_front(index);
                    return self.fail_conn(g, &why).await;
                }
                c.last_item = Some(item_id.clone());
                if let Some(r) = c.early.remove(&item_id) {
                    c.outstanding.remove(&index);
                    self.shared.resolve(index, r);
                } else {
                    c.items.insert(item_id, index);
                }
            }
            ServerEvent::Completed { item_id, transcript } => match c.items.remove(&item_id) {
                Some(index) => {
                    c.outstanding.remove(&index);
                    self.shared.resolve(index, Ok(transcript));
                }
                None => {
                    c.early.insert(item_id, Ok(transcript));
                }
            },
            ServerEvent::Failed { item_id, message } => match c.items.remove(&item_id) {
                Some(index) => {
                    c.outstanding.remove(&index);
                    self.shared.resolve(index, Err(message));
                }
                None => {
                    c.early.insert(item_id, Err(message));
                }
            },
            ServerEvent::Error { event_id, code, message } => {
                if let Some(index) = event_id.as_deref().and_then(realtime::commit_index) {
                    // The commit failed, so its audio is still in the server's buffer. Any
                    // piece committed after it (before we heard) got that audio glued on, and so
                    // may the piece being recorded: all of those go the normal way, and the
                    // buffer is cleared so the leftover can't reach later pieces.
                    tracing::info!("live transcription: commit of piece {index} failed ({code:?}: {message})");
                    let pos = c.fifo.iter().position(|&i| i == index);
                    let glued: Vec<usize> = pos.map(|p| c.fifo.iter().skip(p + 1).copied().collect()).unwrap_or_default();
                    if let Some(p) = pos {
                        c.fifo.remove(p);
                    }
                    c.outstanding.remove(&index);
                    self.shared.resolve(index, Err(message));
                    for i in glued {
                        c.outstanding.remove(&i);
                        self.shared.resolve(i, Err("sent after a failed piece".into()));
                    }
                    if self.current == Some(g) && self.bound {
                        self.current = None;
                        self.taint = Some(g);
                    }
                    self.out(g, Out::Clear(index)).await;
                } else if event_id.as_deref().is_some_and(|e| e.starts_with("clear-")) {
                    tracing::debug!("live transcription: clear failed: {message}");
                } else {
                    // Can't tell what it hit (an audio append, the session itself): the
                    // pieces on this connection may be missing audio or mismatched, so they all
                    // go the normal way and the connection is dropped.
                    return self.fail_conn(g, &format!("error ({code:?}): {message}")).await;
                }
            }
            ServerEvent::Delta { .. } | ServerEvent::SessionUpdated | ServerEvent::Other(_) => {}
        }
        self.close_if_drained(g).await;
    }

    /// A session with pieces waiting that has gone quiet has stalled: its pieces go the normal
    /// way, and live transcription is paused for a while.
    async fn check_stalls(&mut self) {
        let limit = self.tuning().stall_timeout;
        let stalled: Vec<u32> = self
            .conns
            .iter()
            .filter(|(_, c)| matches!(c.state, ConnState::Ready(_)) && !c.outstanding.is_empty() && c.last_heard.elapsed() >= limit)
            .map(|(g, _)| *g)
            .collect();
        for g in stalled {
            self.setup.endpoint.breaker.trip(self.tuning().breaker_cooldown, "the live session stopped answering");
            self.fail_conn(g, "stalled").await;
        }
    }

    async fn close_if_drained(&mut self, g: u32) {
        let done = self.conns.get(&g).is_some_and(|c| c.draining && c.outstanding.is_empty() && c.queued.is_empty());
        if done {
            if let Some(c) = self.conns.remove(&g) {
                close(c).await;
            }
        }
    }

    /// The connection is gone: every piece still waiting on it is transcribed the normal way.
    async fn fail_conn(&mut self, g: u32, reason: &str) {
        let Some(c) = self.conns.remove(&g) else { return };
        let mut lost: Vec<usize> = c.outstanding.iter().copied().collect();
        lost.extend(c.queued.iter().filter_map(|o| if let Out::Commit(i) = o { Some(*i) } else { None }));
        for i in lost {
            self.shared.resolve(i, Err(format!("connection lost: {reason}")));
        }
        if self.current == Some(g) {
            self.current = None;
        }
        if self.active == Some(g) {
            self.active = None;
        }
        close(c).await;
    }
}

fn message(o: &Out) -> String {
    match o {
        Out::Audio(a) => realtime::append_message(a),
        Out::Commit(i) => realtime::commit_message(*i),
        Out::Clear(i) => realtime::clear_message(*i),
    }
}

async fn close(c: Conn) {
    match c.state {
        ConnState::Ready(mut sink) => {
            let _ = tokio::time::timeout(Duration::from_secs(2), sink.close()).await;
            c.reader.abort();
        }
        // Still connecting: let the attempt finish on its own (bounded by the connect timeout)
        // so a failure still trips the breaker; if it succeeds, it closes itself.
        ConnState::Opening => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tuning() -> LiveTuning {
        LiveTuning { stop_grace: Duration::from_millis(300), piece_timeout: Duration::from_secs(5), ..Default::default() }
    }

    #[test]
    fn stop_allowance_grows_with_waiting_work() {
        let t = LiveTuning::default();
        assert_eq!(stop_allowance(&t, 0, 0), Duration::from_millis(2_500));
        assert_eq!(stop_allowance(&t, 2, 10_000), Duration::from_millis(2_500 + 500 + 400));
        assert!(stop_allowance(&t, 1, 45_000) < Duration::from_secs(5));
    }

    #[test]
    fn rates_map_between_recordings() {
        assert_eq!(to_24k(16_000), 24_000);
        assert_eq!(to_24k(3), 4);
    }

    #[tokio::test]
    async fn result_before_wait_is_used_once() {
        let s = LiveShared::new(tuning());
        s.register(0, 1000);
        assert!(s.resolve(0, Ok("hi".into())));
        assert_eq!(s.wait(0).await.as_deref(), Some("hi"));
        // Anything later for the same piece is ignored.
        assert!(!s.resolve(0, Ok("again".into())));
        assert_eq!(s.summary(), LiveSummary { connected: false, live_pieces: 1, fallback_pieces: 0, late_ignored: 1 });
    }

    #[tokio::test]
    async fn waiting_wakes_on_result_and_failure() {
        let s = LiveShared::new(tuning());
        s.register(0, 1000);
        s.register(1, 1000);
        let s2 = s.clone();
        let h = tokio::spawn(async move { (s2.wait(0).await, s2.wait(1).await) });
        tokio::time::sleep(Duration::from_millis(20)).await;
        s.resolve(1, Err("boom".into()));
        s.resolve(0, Ok("zero".into()));
        let (a, b) = h.await.unwrap();
        assert_eq!(a.as_deref(), Some("zero"));
        assert_eq!(b, None);
        assert_eq!(s.summary().fallback_pieces, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn gives_up_shortly_after_stop_and_ignores_late_results() {
        let s = LiveShared::new(tuning());
        s.mark_connected();
        s.register(3, 2000);
        let t0 = Instant::now();
        s.stop();
        assert_eq!(s.wait(3).await, None);
        let waited = t0.elapsed();
        assert!(waited >= Duration::from_millis(300) && waited < Duration::from_millis(900), "{waited:?}");
        assert!(!s.resolve(3, Ok("late".into())));
        assert_eq!(s.summary().late_ignored, 1);
        // Never-registered pieces (not sent live) fall back immediately.
        assert_eq!(s.wait(9).await, None);
    }

    #[tokio::test(start_paused = true)]
    async fn never_connected_means_no_wait_after_stop() {
        let s = LiveShared::new(tuning());
        s.register(0, 3000);
        let t0 = Instant::now();
        s.stop();
        assert_eq!(s.wait(0).await, None);
        assert_eq!(t0.elapsed(), Duration::ZERO);
        assert_eq!(s.summary().fallback_pieces, 1);
    }

    #[tokio::test]
    async fn empty_live_transcript_counts_as_a_fallback() {
        let s = LiveShared::new(tuning());
        s.register(0, 1000);
        s.resolve(0, Ok("  ".into()));
        assert_eq!(s.wait(0).await, None);
        assert_eq!(s.summary().live_pieces, 0);
        assert_eq!(s.summary().fallback_pieces, 1);
    }

    #[test]
    fn debug_output_never_shows_the_key() {
        let setup = LiveSetup {
            endpoint: LiveEndpoint::openai(),
            key: "sk-secret-123".into(),
            session: SessionConfig { model: "gpt-transcribe".into(), prompt: None, keywords: vec![], languages: vec![] },
        };
        let d = format!("{setup:?}");
        assert!(!d.contains("sk-secret-123"), "{d}");
        assert!(d.contains("redacted"));
    }

    #[test]
    fn breaker_pauses_then_recovers() {
        let b = Breaker::default();
        assert!(!b.tripped());
        b.trip(Duration::from_millis(30), "test");
        assert!(b.tripped());
        std::thread::sleep(Duration::from_millis(40));
        assert!(!b.tripped());
        // The app-wide OpenAI endpoint shares one breaker.
        assert!(Arc::ptr_eq(&LiveEndpoint::openai().breaker, &LiveEndpoint::openai().breaker));
    }

    #[tokio::test(start_paused = true)]
    async fn a_piece_is_not_awaited_forever_while_recording() {
        let s = LiveShared::new(tuning());
        s.register(0, 1000);
        let t0 = Instant::now();
        assert_eq!(s.wait(0).await, None);
        assert!(t0.elapsed() >= Duration::from_secs(5));
    }
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod pipeline_tests;
